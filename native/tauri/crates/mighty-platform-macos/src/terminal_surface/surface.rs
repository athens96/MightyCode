//! `MacTerminalSurface`: one Ghostty view over the page's NativeSlot (plan §1.4, docs/tauri-architecture.md §3.1). Every call returns
//! at once; the AppKit and libghostty work is queued on the main thread in the order it was asked (`runtime::on_main`), and what has
//! an answer (the picture, the state, the grid's text) is awaited from there. The PTY's output goes through the surface's own writer
//! (`runtime::Shared::feed`), never the main thread.
//!
//! - Placement: the slot's device pixels divided by the window's backing scale, y counted from the content view's bottom (Tao's
//!   content view is not flipped); the view is the content view's last subview, over the page's WKWebView.
//! - Shown only while the slot shows; hidden, Ghostty is told it is occluded (it stops drawing) and, when it had the keyboard, the
//!   page's own webview gets it back.
//! - The snapshot is AppKit's picture of the view (`cacheDisplayInRect`, which holds Ghostty's frame: Step 0's answer).

use std::ffi::c_void;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use mighty_platform::{BoxFuture, DeviceRect, NativeSurface, PlatformError, Result, SurfaceState, TerminalOptions, TerminalSurface};
use objc2::rc::Retained;
use objc2::{MainThreadMarker, Message};
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

/// A main-thread object held by an owner other threads share: read on the main thread only, and released there.
struct OnMain<T: Message + 'static>(Option<Retained<T>>);

// SAFETY: the object is only reached through `get`, which needs the main thread, and released on the main thread (`Drop`).
unsafe impl<T: Message + 'static> Send for OnMain<T> {}
// SAFETY: as above.
unsafe impl<T: Message + 'static> Sync for OnMain<T> {}

/// A retained object carried to the main thread to be released there.
struct Release<T: Message + 'static>(Retained<T>);
// SAFETY: carried only, and dropped on the main thread.
unsafe impl<T: Message + 'static> Send for Release<T> {}

impl<T: Message + 'static> OnMain<T> {
    fn new(object: Retained<T>) -> OnMain<T> {
        OnMain(Some(object))
    }

    fn get(&self, _main: MainThreadMarker) -> Option<&T> {
        self.0.as_deref()
    }
}

impl<T: Message + 'static> Drop for OnMain<T> {
    fn drop(&mut self) {
        let Some(object) = self.0.take() else { return };
        if MainThreadMarker::new().is_some() {
            drop(object);
        } else {
            let release = Release(object);
            runtime::on_main(move || drop(release));
        }
    }
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

/// What a surface keeps: its view, the window's content view and the page's own webview (all retained, main thread only), what it
/// shares with libghostty, and the userdata libghostty holds (`Arc::into_raw`, released once the surface is freed).
struct Inner {
    shared: Arc<Shared>,
    view: OnMain<TerminalView>,
    content: OnMain<NSView>,
    page: Option<OnMain<NSView>>,
    userdata: usize,
    closed: AtomicBool,
}

impl Inner {
    /// Hands the keyboard back to the page's own webview (else the content view's first view that is not a terminal).
    fn give_keyboard_to_page(&self, main: MainThreadMarker) {
        let Some(content) = self.content.get(main) else { return };
        let Some(window) = content.window() else { return };
        // Only the page's own webview, found when the view was made: guessing among the content view's subviews could hand the
        // keyboard to another native view (Mac integration review 16). Without it the window keeps its first responder.
        if let Some(page) = self.page.as_ref().and_then(|page| page.get(main)) {
            let _ = window.makeFirstResponder(Some(page));
        }
    }
}

pub struct MacTerminalSurface {
    inner: Arc<Inner>,
}

/// The page's own webview (the window's webview of the window's label) as an NSView, retained.
async fn page_view(window: &tauri::Window) -> Option<Retained<NSView>> {
    let page = window.webviews().into_iter().find(|webview| webview.label() == window.label())?;
    let (sender, receiver) = tokio::sync::oneshot::channel();
    page.with_webview(move |platform| {
        let view = platform.inner() as *mut NSView;
        // SAFETY: the page's WKWebView (an NSView), alive with its window; retained here, on the main thread.
        let _ = sender.send(unsafe { Retained::retain(view) }.map(Release));
    })
    .ok()?;
    receiver.await.ok().flatten().map(|release| release.0)
}

/// Retains `window`'s content `NSView` synchronously on the main thread so no raw unretained view pointer ever crosses an `.await`.
async fn content_view(window: &tauri::Window) -> Result<OnMain<NSView>> {
    if MainThreadMarker::new().is_some() {
        let raw = window.ns_view().map_err(|e| PlatformError::Refused(format!("the window has no view: {e}")))? as *mut NSView;
        // SAFETY: Tauri's content view of `window`, synchronously retained on the main thread before any `.await`.
        let retained = unsafe { Retained::retain(raw) }.ok_or_else(|| PlatformError::Refused("no content view".into()))?;
        return Ok(OnMain::new(retained));
    }
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let handle = window.clone();
    window
        .run_on_main_thread(move || {
            let result = handle.ns_view().map_err(|e| PlatformError::Refused(format!("the window has no view: {e}"))).and_then(|raw| {
                // SAFETY: Tauri's content view of `handle`, retained synchronously on the main thread while `handle` is live.
                unsafe { Retained::retain(raw as *mut NSView) }
                    .map(OnMain::new)
                    .ok_or_else(|| PlatformError::Refused("no content view".into()))
            });
            let _ = sender.send(result);
        })
        .map_err(|e| PlatformError::Refused(format!("the window has no main thread: {e}")))?;
    receiver.await.map_err(|_| PlatformError::Invalid("the main thread did not answer".into()))?
}

/// Opens a view at `options.bounds` over `window`'s page (main thread work, awaited).
pub async fn open(window: &tauri::Window, options: TerminalOptions) -> Result<Box<dyn TerminalSurface>> {
    let content = content_view(window).await?;
    let page = page_view(window).await.map(OnMain::new);
    let inner = ask(move || create(content, page, options)).await??;
    Ok(Box::new(MacTerminalSurface { inner: Arc::new(inner) }))
}

/// A surface for `view` with the host-managed backend, so the host's PTY is what it draws, its writer started; the userdata it holds
/// (`Arc::into_raw`), released after the surface is freed. Main thread.
fn make_surface(app: ghostty_app_t, view: &TerminalView, scale: f64, font_size: f64) -> Result<*mut c_void> {
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
    if let Err(error) = view.shared().attach(surface) {
        if view.shared().close(app) {
            // SAFETY: the surface is freed: its userdata goes.
            drop(unsafe { Arc::from_raw(userdata as *const Shared) });
        } else {
            // Keep the view alive alongside any leaked surface/userdata.
            std::mem::forget(view.retain());
        }
        return Err(PlatformError::Io(error));
    }
    // SAFETY: the new surface on the main thread.
    unsafe { ghostty_surface_set_occlusion(surface, true) };
    view.sync_size();
    // SAFETY: as above.
    unsafe { ghostty_surface_refresh(surface) };
    Ok(userdata)
}

/// Makes the view over the page and its surface (main thread).
fn create(content: OnMain<NSView>, page: Option<OnMain<NSView>>, options: TerminalOptions) -> Result<Inner> {
    let main = MainThreadMarker::new().ok_or_else(|| PlatformError::Invalid("not on the main thread".into()))?;
    let content_ref = content.get(main).ok_or_else(|| PlatformError::Refused("no content view".into()))?;
    let look = runtime::look();
    let app = runtime::app(&look).map_err(PlatformError::Refused)?;
    let shared = Arc::new(Shared::new(options.listener));
    let view = TerminalView::new(main, frame_in(content_ref, options.bounds), shared.clone());
    content_ref.addSubview(&view);
    let userdata = match make_surface(app, &view, scale_of(content_ref), look.size) {
        Ok(userdata) => userdata,
        Err(error) => {
            view.removeFromSuperview();
            return Err(error);
        }
    };
    runtime::register(&shared);
    runtime::owe();
    Ok(Inner { shared, view: OnMain::new(view), content, page, userdata: userdata as usize, closed: AtomicBool::new(false) })
}

/// Ends the surface (main thread, once): the keyboard back to the page, the view off the window, the writer stopped and the surface
/// freed while the app keeps ticking, then the userdata. If the writer stalls and `Shared::close` returns `false`, `TerminalView` is
/// kept alive (retained and leaked) alongside the unfreed `ghostty_surface_t` and `userdata`.
fn free(inner: &Inner, main: MainThreadMarker) {
    runtime::unregister(&inner.shared);
    if let Some(view) = inner.view.get(main) {
        if view.has_keyboard() {
            inner.give_keyboard_to_page(main);
        }
        view.removeFromSuperview();
    }
    if inner.shared.close(runtime::current_app()) {
        // SAFETY: the userdata kept at the making, released once, after the surface that used it.
        drop(unsafe { Arc::from_raw(inner.userdata as *const Shared) });
    } else if let Some(view) = inner.view.get(main) {
        // Writer stalled: keep `TerminalView` alive alongside the leaked `ghostty_surface_t` so libghostty's stored view pointer
        // cannot dangle.
        std::mem::forget(view.retain());
    }
}

impl MacTerminalSurface {
    fn live(&self) -> Result<()> {
        if self.inner.closed.load(Ordering::SeqCst) { Err(closed()) } else { Ok(()) }
    }

    /// Queues `f` with the view and the content view on the main thread.
    fn on_view(&self, f: impl FnOnce(&TerminalView, &NSView) + Send + 'static) -> Result<()> {
        self.live()?;
        let inner = self.inner.clone();
        runtime::on_main(move || {
            let Some(main) = MainThreadMarker::new() else { return };
            // Closed meanwhile: the view is off the window.
            if inner.shared.surface().is_none() {
                return;
            }
            if let (Some(view), Some(content)) = (inner.view.get(main), inner.content.get(main)) {
                f(view, content);
            }
        });
        Ok(())
    }

    async fn ask_view<R: Send + 'static>(&self, f: impl FnOnce(&TerminalView, &NSView) -> R + Send + 'static) -> Result<R> {
        self.live()?;
        let inner = self.inner.clone();
        ask(move || {
            let main = MainThreadMarker::new()?;
            inner.shared.surface()?;
            Some(f(inner.view.get(main)?, inner.content.get(main)?))
        })
        .await?
        .ok_or_else(closed)
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
        let inner = self.inner.clone();
        self.on_view(move |view, _| {
            view.shared().visible.store(visible, Ordering::SeqCst);
            if !visible
                && view.has_keyboard()
                && let Some(main) = MainThreadMarker::new()
            {
                inner.give_keyboard_to_page(main);
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
            self.ask_view(|view, content| SurfaceState { bounds: device_rect(content, view.frame()), visible: !view.isHidden() }).await
        })
    }

    fn close(&self) -> Result<()> {
        if self.inner.closed.swap(true, Ordering::SeqCst) {
            return Err(closed());
        }
        let inner = self.inner.clone();
        runtime::on_main(move || {
            if let Some(main) = MainThreadMarker::new() {
                free(&inner, main);
            }
        });
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

    /// Queued for the surface's writer; waits while its queue is full (backpressure).
    fn feed(&self, output: &[u8]) -> Result<()> {
        self.live()?;
        self.inner.shared.feed(output);
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

/// What [`headless_round_trip`] saw.
#[derive(Debug, Default)]
pub struct Headless {
    /// The grid read back after the bytes were written.
    pub text: String,
    /// What the surface handed back (typed text, and any reply a program's request got), and the grids it reported.
    pub input: Vec<u8>,
    pub grids: Vec<(u16, u16)>,
    /// Bells rung and titles set while a write full of them went in.
    pub bells: usize,
    pub titles: usize,
    /// Clipboard writes that landed while a program asked for one (OSC 52).
    pub clipboard_writes: u64,
    /// Whether the surface was freed (its writer ended in time).
    pub freed: bool,
}

/// The bridge's ABI without a window (the Mac's `cargo test`, on the main thread, with no Tauri app): the app made from Mighty's
/// config, a host-managed surface on a view in no window; `written` fed through the surface's writer and read back from the grid,
/// then `flood` (one write of many bells and titles: more than libghostty's mailbox holds) fed while the main thread ticks, then a
/// program's OSC 52 read and write (no reply, no clipboard write), text typed coming back through `receive_buffer`, the grid reported
/// through `receive_resize`, and the surface closed (writer stopped, surface freed).
pub fn headless_round_trip(written: &str, flood: &str, typed: &str) -> std::result::Result<Headless, String> {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    let main = MainThreadMarker::new().ok_or("not on the main thread")?;
    let app = runtime::app(&runtime::Look::default())?;
    let seen: Arc<Mutex<Headless>> = Arc::default();
    let sink = seen.clone();
    let listener = mighty_platform::TerminalListener::new(move |event| {
        let mut seen = sink.lock().unwrap_or_else(|p| p.into_inner());
        match event {
            mighty_platform::TerminalEvent::Input(bytes) => seen.input.extend(bytes),
            mighty_platform::TerminalEvent::Resized { columns, rows, .. } => seen.grids.push((columns, rows)),
            mighty_platform::TerminalEvent::Bell => seen.bells += 1,
            mighty_platform::TerminalEvent::Title(_) => seen.titles += 1,
            _ => {}
        }
    });
    let shared = Arc::new(Shared::new(Some(listener)));
    let view = TerminalView::new(main, NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(640.0, 360.0)), shared.clone());
    let userdata = make_surface(app, &view, 1.0, 13.0).map_err(|e| e.to_string())?;
    let tick_until = |seconds: u64, done: &dyn Fn() -> bool| {
        let deadline = Instant::now() + Duration::from_secs(seconds);
        while Instant::now() < deadline && !done() {
            // SAFETY: the live app on the main thread.
            unsafe { ghostty_app_tick(app) };
            std::thread::sleep(Duration::from_millis(10));
        }
    };
    shared.feed(written.as_bytes());
    let wanted: String = written.chars().filter(|c| !c.is_control()).collect();
    let marker = wanted.split_whitespace().next().unwrap_or("").to_string();
    tick_until(10, &|| view.viewport_text().contains(&marker));
    let text = view.viewport_text();
    let writes_before = runtime::CLIPBOARD_WRITES.load(Ordering::SeqCst);
    shared.feed(flood.as_bytes());
    shared.feed(b"\x1b]52;c;?\x07\x1b]52;c;aGVhZGxlc3M=\x07after-flood\r\n");
    tick_until(20, &|| view.viewport_text().contains("after-flood"));
    // A reply or a clipboard write would come through the next ticks.
    tick_until(1, &|| false);
    view.surface_text(typed);
    tick_until(5, &|| String::from_utf8_lossy(&seen.lock().unwrap_or_else(|p| p.into_inner()).input).contains(typed));
    let freed = shared.close(app);
    if freed {
        // SAFETY: the surface is freed: the userdata it held goes.
        drop(unsafe { Arc::from_raw(userdata as *const Shared) });
    } else {
        // Writer stalled: keep `TerminalView` alive alongside the leaked `ghostty_surface_t` and `userdata`.
        std::mem::forget(view);
    }
    let mut report = std::mem::take(&mut *seen.lock().unwrap_or_else(|p| p.into_inner()));
    report.text = text;
    report.clipboard_writes = runtime::CLIPBOARD_WRITES.load(Ordering::SeqCst) - writes_before;
    report.freed = freed;
    Ok(report)
}
