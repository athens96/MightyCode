//! `MacTerminalSurface`: one Ghostty view over the page's NativeSlot (plan §1.4, docs/tauri-architecture.md §3.1). Every call returns
//! at once; the AppKit and libghostty work runs on the main thread in the order it was asked (`runtime::on_main`), and what has an
//! answer (the picture, the state, the grid's text) is awaited from there.
//!
//! - Placement: the slot's device pixels divided by the window's backing scale, y counted from the content view's bottom (Tao's
//!   content view is not flipped); the view is the content view's last subview, over the page's WKWebView.
//! - Shown only while the slot shows; hidden, Ghostty is told it is occluded (it stops drawing) and, when it had the keyboard, the
//!   page gets it back.
//! - The snapshot is AppKit's picture of the view (`cacheDisplayInRect`, which holds Ghostty's frame: Step 0's answer).

use std::ffi::c_void;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use mighty_platform::{BoxFuture, DeviceRect, NativeSurface, PlatformError, Result, SurfaceState, TerminalOptions, TerminalSurface};
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::NSView;
use objc2_foundation::{NSPoint, NSRect, NSSize};

use super::config::normalized_family;
use super::ghostty::*;
use super::keys::{self, Typed};
use super::runtime::{self, Shared};
use super::view::TerminalView;

fn closed() -> PlatformError {
    PlatformError::Invalid("the terminal view is closed".into())
}

/// Runs `f` on the main thread and waits for its answer.
async fn ask<R: Send + 'static>(f: impl FnOnce() -> R + Send + 'static) -> Result<R> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    runtime::on_main(move || {
        let _ = sender.send(f());
    });
    receiver.await.map_err(|_| PlatformError::Invalid("the main thread did not answer".into()))
}

/// The content view behind `content` (main thread; the window keeps it alive).
fn content_view<'a>(content: usize) -> &'a NSView {
    // SAFETY: Tauri's content view of the window, alive with the window, used on the main thread only.
    unsafe { &*(content as *const NSView) }
}

/// The view behind `view` (main thread; retained until the surface closes).
fn terminal_view<'a>(view: usize) -> &'a TerminalView {
    // SAFETY: the view `Retained::into_raw` kept, released only by `free` on the main thread.
    unsafe { &*(view as *const TerminalView) }
}

fn scale_of(view: &NSView) -> f64 {
    view.window().map_or(1.0, |window| window.backingScaleFactor())
}

/// The frame in `content` (points) of a slot at `rect` (device pixels from the page's top left).
fn frame_in(content: &NSView, rect: DeviceRect) -> NSRect {
    let logical = rect.to_logical(scale_of(content));
    let y = if content.isFlipped() { logical.y } else { content.bounds().size.height - logical.y - logical.height };
    NSRect::new(NSPoint::new(logical.x, y), NSSize::new(logical.width, logical.height))
}

/// The view's frame back as the page's device pixels.
fn device_rect(content: &NSView, frame: NSRect) -> DeviceRect {
    let scale = scale_of(content);
    let top = if content.isFlipped() { frame.origin.y } else { content.bounds().size.height - frame.origin.y - frame.size.height };
    DeviceRect {
        x: (frame.origin.x * scale).round() as i32,
        y: (top * scale).round() as i32,
        width: (frame.size.width * scale).round().max(0.0) as u32,
        height: (frame.size.height * scale).round().max(0.0) as u32,
    }
}

/// Hands the keyboard back to the page (the content view's first subview that is not a terminal: the page's WKWebView).
fn give_keyboard_to_page(content: &NSView) {
    let Some(window) = content.window() else { return };
    let page = content.subviews().iter().find(|view| !TerminalView::is_terminal(view));
    let _ = window.makeFirstResponder(page.as_deref().map(|view| view.as_ref()));
}

pub struct MacTerminalSurface {
    shared: Arc<Shared>,
    /// The `TerminalView`, retained (`Retained::into_raw`).
    view: usize,
    /// The `Arc<Shared>` libghostty holds as the surface's userdata (`Arc::into_raw`).
    userdata: usize,
    /// The window's content view.
    content: usize,
    closed: AtomicBool,
}

/// Opens a view at `options.bounds` over `window`'s page (main thread work, awaited).
pub async fn open(window: &tauri::Window, options: TerminalOptions) -> Result<Box<dyn TerminalSurface>> {
    use tauri::Manager as _;
    let _ = runtime::HANDLE.set(window.app_handle().clone());
    let content = window.ns_view().map_err(|e| PlatformError::Refused(format!("the window has no view: {e}")))? as usize;
    let surface = ask(move || create(content, options)).await??;
    Ok(Box::new(surface))
}

/// A surface for `view` with the host-managed backend, so the host's PTY is what it draws; the userdata it holds (`Arc::into_raw`),
/// released after the surface is freed. Main thread.
fn make_surface(app: ghostty_app_t, view: &TerminalView, scale: f64, font_size: f64) -> Result<(ghostty_surface_t, *mut c_void)> {
    let userdata = Arc::into_raw(view.shared().clone()) as *mut c_void;
    // SAFETY: libghostty's surface config, filled as the header lays it out; the view and the userdata outlive the surface.
    let surface = unsafe {
        let mut config = ghostty_surface_config_new();
        config.platform_tag = GHOSTTY_PLATFORM_MACOS;
        config.platform = view as *const TerminalView as *mut c_void;
        config.userdata = userdata;
        config.backend = GHOSTTY_SURFACE_IO_BACKEND_HOST_MANAGED;
        config.receive_userdata = userdata;
        config.receive_buffer = Some(runtime::receive_buffer);
        config.receive_resize = Some(runtime::receive_resize);
        config.scale_factor = scale;
        config.font_size = font_size as f32;
        config.context = GHOSTTY_SURFACE_CONTEXT_WINDOW;
        ghostty_surface_new(app, &config)
    };
    if surface.is_null() {
        // SAFETY: the userdata no surface took.
        drop(unsafe { Arc::from_raw(userdata as *const Shared) });
        return Err(PlatformError::Refused("ghostty_surface_new gave nothing".into()));
    }
    view.shared().surface.store(surface, Ordering::SeqCst);
    // SAFETY: the new surface on the main thread.
    unsafe { ghostty_surface_set_occlusion(surface, true) };
    view.sync_size();
    // SAFETY: as above.
    unsafe { ghostty_surface_refresh(surface) };
    Ok((surface, userdata))
}

/// Makes the view over the page and its surface (main thread).
fn create(content: usize, options: TerminalOptions) -> Result<MacTerminalSurface> {
    let mtm = MainThreadMarker::new().ok_or_else(|| PlatformError::Invalid("not on the main thread".into()))?;
    let parent = content_view(content);
    let look = runtime::look();
    let app = runtime::app(&look).map_err(PlatformError::Refused)?;
    let shared = Arc::new(Shared::new(options.listener));
    let view = TerminalView::new(mtm, frame_in(parent, options.bounds), shared.clone());
    parent.addSubview(&view);
    let (_, userdata) = match make_surface(app, &view, scale_of(parent), look.size) {
        Ok(made) => made,
        Err(error) => {
            view.removeFromSuperview();
            return Err(error);
        }
    };
    runtime::register(&shared);
    runtime::owe();
    Ok(MacTerminalSurface {
        shared,
        view: Retained::into_raw(view) as usize,
        userdata: userdata as usize,
        content,
        closed: AtomicBool::new(false),
    })
}

/// What [`headless_round_trip`] saw.
#[derive(Debug, Default)]
pub struct Headless {
    /// The grid read back after the bytes were written.
    pub text: String,
    /// What the surface handed back as typed, and the grids it reported.
    pub input: Vec<u8>,
    pub grids: Vec<(u16, u16)>,
}

/// The bridge's ABI without a window (the Mac's `cargo test`, on the main thread, with no Tauri app): the app made from Mighty's
/// config, a host-managed surface on a view in no window, bytes written in and read back from the grid, text typed coming back
/// through `receive_buffer`, the grid reported through `receive_resize`, and the surface freed.
pub fn headless_round_trip(written: &str, typed: &str) -> std::result::Result<Headless, String> {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    let mtm = MainThreadMarker::new().ok_or("not on the main thread")?;
    let app = runtime::app(&runtime::Look::default())?;
    let seen: Arc<Mutex<Headless>> = Arc::default();
    let sink = seen.clone();
    let listener = mighty_platform::TerminalListener::new(move |event| {
        let mut seen = sink.lock().unwrap_or_else(|p| p.into_inner());
        match event {
            mighty_platform::TerminalEvent::Input(bytes) => seen.input.extend(bytes),
            mighty_platform::TerminalEvent::Resized { columns, rows, .. } => seen.grids.push((columns, rows)),
            _ => {}
        }
    });
    let view =
        TerminalView::new(mtm, NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(640.0, 360.0)), Arc::new(Shared::new(Some(listener))));
    let (surface, userdata) = make_surface(app, &view, 1.0, 13.0).map_err(|e| e.to_string())?;
    // SAFETY: the live surface on the main thread; libghostty copies the bytes.
    unsafe { ghostty_surface_write_buffer(surface, written.as_ptr(), written.len()) };
    let deadline = Instant::now() + Duration::from_secs(10);
    let wanted: String = written.chars().filter(|c| !c.is_control()).collect();
    let mut text = String::new();
    while Instant::now() < deadline {
        // SAFETY: the live app on the main thread.
        unsafe { ghostty_app_tick(app) };
        text = view.viewport_text();
        if text.contains(wanted.trim()) {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    view.surface_text(typed);
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && !String::from_utf8_lossy(&seen.lock().unwrap_or_else(|p| p.into_inner()).input).contains(typed) {
        // SAFETY: as above.
        unsafe { ghostty_app_tick(app) };
        std::thread::sleep(Duration::from_millis(20));
    }
    view.shared().surface.store(std::ptr::null_mut(), Ordering::SeqCst);
    // SAFETY: the surface freed once, then the userdata it held.
    unsafe {
        ghostty_surface_free(surface);
        drop(Arc::from_raw(userdata as *const Shared));
    }
    let mut report = std::mem::take(&mut *seen.lock().unwrap_or_else(|p| p.into_inner()));
    report.text = text;
    Ok(report)
}

/// Frees the surface, then the view and the userdata (main thread, once).
fn free(shared: Arc<Shared>, view: usize, userdata: usize, content: usize) {
    runtime::unregister(&shared);
    let terminal = terminal_view(view);
    if terminal.has_keyboard() {
        give_keyboard_to_page(content_view(content));
    }
    let surface = shared.surface.swap(std::ptr::null_mut(), Ordering::SeqCst);
    if !surface.is_null() {
        // SAFETY: the live surface, freed once on the main thread (its threads end with it).
        unsafe {
            ghostty_surface_set_focus(surface, false);
            ghostty_surface_free(surface);
        }
    }
    terminal.removeFromSuperview();
    // SAFETY: the view and the userdata kept at the making, released once, after the surface that used them.
    unsafe {
        drop(Retained::from_raw(view as *mut TerminalView));
        drop(Arc::from_raw(userdata as *const Shared));
    }
}

impl MacTerminalSurface {
    fn live(&self) -> Result<()> {
        if self.closed.load(Ordering::SeqCst) { Err(closed()) } else { Ok(()) }
    }

    /// Runs `f` with the view and the content view on the main thread.
    fn on_view(&self, f: impl FnOnce(&TerminalView, &NSView) + Send + 'static) -> Result<()> {
        self.live()?;
        let (view, content, shared) = (self.view, self.content, self.shared.clone());
        runtime::on_main(move || {
            // Closed meanwhile: the view is gone.
            if shared.surface().is_some() {
                f(terminal_view(view), content_view(content));
            }
        });
        Ok(())
    }

    async fn ask_view<R: Send + 'static>(&self, f: impl FnOnce(&TerminalView, &NSView) -> R + Send + 'static) -> Result<R> {
        self.live()?;
        let (view, content, shared) = (self.view, self.content, self.shared.clone());
        ask(move || shared.surface().map(|_| f(terminal_view(view), content_view(content)))).await?.ok_or_else(closed)
    }
}

impl NativeSurface for MacTerminalSurface {
    fn place(&self, rect: DeviceRect) -> Result<()> {
        self.on_view(move |view, content| {
            view.setFrame(frame_in(content, rect));
            view.sync_size();
        })
    }

    fn set_visible(&self, visible: bool) -> Result<()> {
        self.on_view(move |view, content| {
            view.shared().visible.store(visible, Ordering::SeqCst);
            if !visible && view.has_keyboard() {
                give_keyboard_to_page(content);
            }
            view.setHidden(!visible);
            if let Some(surface) = view.shared().surface() {
                // SAFETY: the live surface on the main thread.
                unsafe { ghostty_surface_set_occlusion(surface, visible) };
            }
            runtime::owe();
        })
    }

    fn focus(&self) -> Result<()> {
        self.on_view(|view, _| {
            view.take_keyboard();
        })
    }

    fn snapshot(&self) -> BoxFuture<'_, Result<Option<String>>> {
        Box::pin(async move { self.ask_view(|view, _| view.snapshot()).await })
    }

    fn inspect(&self) -> BoxFuture<'_, Result<SurfaceState>> {
        Box::pin(async move {
            self.ask_view(|view, content| SurfaceState { bounds: device_rect(content, view.frame()), visible: !view.isHidden(), url: None })
                .await
        })
    }

    fn close(&self) -> Result<()> {
        if self.closed.swap(true, Ordering::SeqCst) {
            return Err(closed());
        }
        let (shared, view, userdata, content) = (self.shared.clone(), self.view, self.userdata, self.content);
        runtime::on_main(move || free(shared, view, userdata, content));
        Ok(())
    }
}

impl TerminalSurface for MacTerminalSurface {
    fn send_text(&self, text: &str) -> Result<()> {
        let steps = keys::typed(text);
        self.on_view(move |view, _| {
            for step in steps {
                match step {
                    Typed::Text(text) => view.surface_text(&text),
                    Typed::Key { key_code, mods, text, unshifted } => view.press(key_code, mods, text, unshifted),
                }
            }
        })
    }

    fn set_font(&self, family: &str, size: f64) -> Result<()> {
        let family = normalized_family(Some(family));
        self.on_view(move |_, _| {
            runtime::set_look(|look| {
                look.family = family;
                look.size = if size.is_finite() && size > 0.0 { size } else { look.size };
            })
        })
    }

    fn set_theme(&self, light: bool) -> Result<()> {
        self.on_view(move |_, _| runtime::set_look(|look| look.light = light))
    }

    fn feed(&self, output: &[u8]) -> Result<()> {
        self.live()?;
        self.shared.feed(output);
        Ok(())
    }

    fn text(&self) -> BoxFuture<'_, Result<String>> {
        Box::pin(async move { self.ask_view(|view, _| view.viewport_text()).await })
    }
}

impl Drop for MacTerminalSurface {
    fn drop(&mut self) {
        let _ = self.close();
    }
}
