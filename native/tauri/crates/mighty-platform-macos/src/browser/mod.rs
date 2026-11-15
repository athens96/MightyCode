//! M-E: browser panes. With the browser engine off at launch (`browserEngineEnabled`, plan D4) a pane is WKWebView, a Tauri child
//! webview like Windows' WebView2 panes, in its workspace's own website data store ([`webkit`]); with it on and CEF in the bundle, CEF
//! through the Swift app's own C bridge ([`cef`], `MightyCEFBridge.dylib` by `dlopen`, CEF's methods added to tao's NSApplication class
//! by [`app_class`]). Swift `CefBrowserEngine.swift` (196 lines), `CefBrowserRuntime.swift` (174), `BrowserPaneView.swift` (124),
//! `MightyApplication.swift`, `BrowserBridge/cef_bridge.m` (364), `helper.c` (35), `BrowserEngine.swift`, `BrowserEngine.lock`. The
//! Step 0 spikes `native/tauri/spikes/mac/webkit` and `native/tauri/spikes/mac/cef` proved both on `macos-latest`.
//!
//! CEF can only start before NSApplication's run loop: [`start_engine`] runs between `tauri::Builder::build` and `App::run` (main.rs
//! through platform.rs) and decides the engine for the launch; [`stop_engine`] shuts CEF down at `RunEvent::Exit`. A press inside a
//! pane's view (either engine) reports it focused: a local event monitor hit-tests the window's views ([`watch_focus`]).

pub mod app_class;
pub mod cef;
pub mod webkit;

use std::cell::RefCell;
use std::path::Path;
use std::ptr::NonNull;

use base64::Engine as _;
use block2::RcBlock;
use mighty_platform::{BrowserEvent, BrowserListener, BrowserOptions, BrowserSurface, DeviceRect, PlatformError, Result};
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSEvent, NSEventMask, NSImage, NSView};
use objc2_foundation::{NSDictionary, NSPoint, NSRect, NSSize};
use tauri::{Manager as _, Url};

/// The address a view is made with before its first one: its events are attached before anything real loads.
pub(crate) const BLANK: &str = "about:blank";

/// Opens a browser pane over `window`'s page: CEF when it started at launch, else WKWebView.
pub async fn open(window: &tauri::Window, options: BrowserOptions) -> Result<Box<dyn BrowserSurface>> {
    if cef::running() { cef::open(window, options).await } else { webkit::open(window, options).await }
}

/// Starts the engine the setting asks for (call on the main thread, after `tauri::Builder::build` and before `App::run`): CEF when
/// `enabled` (Swift `CefBrowserEngine.bootstrapRuntime`: `browser.engineEnabled`, read once per launch), its profiles under
/// `root_cache` (`<browser data>/browser-profiles`, so 0.4.x's `<data>/browser-profiles/<key>` where a release adopts the data folder);
/// else nothing, and panes are WKWebView. `temporary_stores`: the WKWebView data stores this run opens are emptied when it ends (the
/// smoke, which never changes the machine).
pub fn start_engine(enabled: bool, root_cache: &Path, temporary_stores: bool) {
    webkit::set_temporary(temporary_stores);
    if !enabled {
        return;
    }
    if let Err(error) = cef::start(root_cache) {
        // Swift's bridge says so on stderr too; the pane then says "failed" (no secret in the reason: paths and CEF's own words).
        eprintln!("MightyCode: the browser engine did not start: {error}");
    }
}

/// Shuts CEF down before the app exits (Swift `CefBrowserRuntime.shutDown` on `willTerminate`: open panes closed, CEF's shutdown
/// within about 5 s), and empties a smoke's WKWebView stores. On the main thread (`RunEvent::Exit`). What CEF's shutdown did, for the
/// log; None when CEF was not running.
pub fn stop_engine() -> Option<String> {
    let stopped = cef::stop();
    webkit::clear_temporary_stores();
    stopped
}

/// A new pane's status on the Mac (mighty-platform `SurfaceFactory::browser_status`): pages show with the setting off (WKWebView,
/// plan D4, where Swift said "disabled"); with CEF asked for at launch, `ready` once it started, else `failed` (no install to offer:
/// Windows' `missing` is its WebView2 installer's). The setting the host latches later changes nothing until a restart, as CEF cannot.
pub fn status(_enabled: bool) -> &'static str {
    if cef::asked() && !cef::running() { "failed" } else { "ready" }
}

/// The engine browser panes use in this launch (the smoke's report).
pub fn engine_name() -> &'static str {
    if cef::running() { "cef" } else { "webkit" }
}

/// The custom protocols every webview of the app answers (its page `tauri`, its `ipc`, the frames' `mightyhtml`, `asset`).
const APP_PROTOCOLS: [&str; 4] = ["tauri", "ipc", "mightyhtml", "asset"];

/// Whether `url` is one of the app's own addresses, which a browser pane never loads (as mighty-platform-windows' rule): a custom
/// protocol of the app (as a scheme, or as the first label of an http host), or the dev server of a dev build.
pub(crate) fn app_address(url: &Url, dev_server: Option<&Url>) -> bool {
    let named = |name: &str| APP_PROTOCOLS.iter().any(|p| p.eq_ignore_ascii_case(name));
    if named(url.scheme()) {
        return true;
    }
    let first_label = url.host_str().and_then(|host| host.split_once('.')).map(|(first, _)| first);
    if url.scheme() == "http" && first_label.is_some_and(named) {
        return true;
    }
    dev_server.is_some_and(|dev| {
        dev.scheme() == url.scheme() && dev.host_str() == url.host_str() && dev.port_or_known_default() == url.port_or_known_default()
    })
}

/// A dev build's page comes from the dev server, which a pane never loads either.
pub(crate) fn dev_server(window: &tauri::Window) -> Option<Url> {
    if tauri::is_dev() { window.config().build.dev_url.clone() } else { None }
}

/// A pane's first address, refused when it is the app's own.
pub(crate) fn first_address(window: &tauri::Window, address: &str) -> Result<Url> {
    let url = Url::parse(address).map_err(|error| PlatformError::Invalid(format!("address: {error}")))?;
    if app_address(&url, dev_server(window).as_ref()) {
        return Err(PlatformError::Invalid("address: the app's own".into()));
    }
    Ok(url)
}

pub(crate) fn refused(what: &str, error: impl std::fmt::Display) -> PlatformError {
    PlatformError::Refused(format!("{what}: {error}"))
}

/// A Tauri label (letters, digits, `-`, `_`) for a pane's webview.
pub(crate) fn label(pane_id: &str, serial: u64) -> String {
    let pane: String = pane_id.chars().take(64).map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect();
    format!("browser-{pane}-{serial}")
}

pub(crate) fn data_url(png: &[u8]) -> String {
    format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(png))
}

/// A bitmap's PNG bytes.
pub(crate) fn png_of(rep: &NSBitmapImageRep) -> Option<Vec<u8>> {
    // SAFETY: no properties; AppKit encodes the bitmap it holds.
    unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new()) }.map(|data| data.to_vec())
}

/// An image's PNG bytes (WKWebView's snapshot).
pub(crate) fn png_of_image(image: &NSImage) -> Option<Vec<u8>> {
    let tiff = image.TIFFRepresentation()?;
    let rep = NSBitmapImageRep::imageRepWithData(&tiff)?;
    png_of(&rep)
}

/// What AppKit says of `view`: its frame in its superview as a rectangle of the page in device pixels (from the top left, at the
/// window's backing scale), and whether it shows (not hidden, in a visible window).
pub(crate) fn device_state(view: &NSView) -> (DeviceRect, bool) {
    let frame = view.frame();
    // SAFETY: AppKit's own superview of a live view, on the main thread (the caller's).
    let superview = unsafe { view.superview() };
    let top = match &superview {
        Some(parent) if !parent.isFlipped() => parent.bounds().size.height - frame.origin.y - frame.size.height,
        _ => frame.origin.y,
    };
    let window = view.window();
    let scale = window.as_ref().map_or(1.0, |window| window.backingScaleFactor());
    let pixels = |points: f64| (points * scale).round();
    let rect = DeviceRect {
        x: pixels(frame.origin.x) as i32,
        y: pixels(top) as i32,
        width: pixels(frame.size.width).max(0.0) as u32,
        height: pixels(frame.size.height).max(0.0) as u32,
    };
    let visible = superview.is_some() && !view.isHiddenOrHasHiddenAncestor() && window.is_some_and(|window| window.isVisible());
    (rect, visible)
}

/// The frame (points, in `parent`'s coordinates) of a page rectangle in device pixels at `scale`.
pub(crate) fn frame_in(parent: &NSView, rect: DeviceRect, scale: f64) -> NSRect {
    let logical = rect.to_logical(scale);
    let y = if parent.isFlipped() { logical.y } else { parent.bounds().size.height - logical.y - logical.height };
    NSRect::new(NSPoint::new(logical.x, y), NSSize::new(logical.width.max(1.0), logical.height.max(1.0)))
}

thread_local! {
    /// The panes' views (their address) and where a press inside one goes; the main thread's only.
    static FOCUS: RefCell<Vec<(usize, BrowserListener)>> = const { RefCell::new(Vec::new()) };
    /// The local event monitor, installed with the first pane.
    static MONITOR: RefCell<Option<Retained<AnyObject>>> = const { RefCell::new(None) };
}

/// Reports a press inside `view` (or a view in it, as CEF's and WKWebView's own) as [`BrowserEvent::Focused`] (WebView2's
/// `GotFocus`; C# makes the pane the active one). On the main thread; [`forget_focus`] before the view goes.
pub(crate) fn watch_focus(view: &NSView, listener: BrowserListener, _mtm: MainThreadMarker) {
    FOCUS.with(|focus| focus.borrow_mut().push((view as *const NSView as usize, listener)));
    MONITOR.with(|monitor| {
        if monitor.borrow().is_some() {
            return;
        }
        let block = RcBlock::new(|event: NonNull<NSEvent>| -> *mut NSEvent {
            // SAFETY: AppKit's event, alive for the call.
            pressed(unsafe { event.as_ref() });
            event.as_ptr()
        });
        // SAFETY: a block that returns the event it was given (nothing is swallowed); kept for the app's life.
        let installed = unsafe {
            NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::LeftMouseDown | NSEventMask::RightMouseDown, &block)
        };
        *monitor.borrow_mut() = installed;
    });
}

/// Stops reporting presses inside the view at `view` (its address). On the main thread.
pub(crate) fn forget_focus(view: usize) {
    FOCUS.with(|focus| focus.borrow_mut().retain(|(watched, _)| *watched != view));
}

/// A mouse press in one of the app's windows: the pane whose view holds the view under it hears of it.
fn pressed(event: &NSEvent) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let Some(window) = event.window(mtm) else { return };
    let Some(content) = window.contentView() else { return };
    // The content view's superview (the window's frame view) has the window's own coordinates.
    let Some(hit) = content.hitTest(event.locationInWindow()) else { return };
    let listeners: Vec<BrowserListener> = FOCUS.with(|focus| {
        focus
            .borrow()
            .iter()
            .filter(|(view, _)| {
                // SAFETY: a registered view is alive: it is forgotten before it goes.
                hit.isDescendantOf(unsafe { &*(*view as *const NSView) })
            })
            .map(|(_, listener)| listener.clone())
            .collect()
    });
    for listener in listeners {
        listener.send(BrowserEvent::Focused);
    }
}

/// The website data store of the panes whose profile folder is `data_dir` (macOS 14's `WKWebsiteDataStore(forIdentifier:)`): a
/// version-5 UUID of the folder's path, so a workspace's panes share theirs (the Windows profile keys, mighty-core's
/// `browser::profile_folder`), two workspaces never do, and a profile of its own (`--profile`, a smoke's) has stores of its own.
pub(crate) fn store_identifier(data_dir: &Path) -> [u8; 16] {
    use sha1::Digest as _;
    let mut hash = sha1::Sha1::new();
    hash.update(STORE_NAMESPACE);
    hash.update(data_dir.to_string_lossy().as_bytes());
    let digest = hash.finalize();
    let mut id = [0u8; 16];
    id.copy_from_slice(&digest[..16]);
    id[6] = (id[6] & 0x0F) | 0x50;
    id[8] = (id[8] & 0x3F) | 0x80;
    id
}

/// The namespace of the stores' UUIDs (Mighty Code's own: `5d3c3a0e-6b1f-4c55-9f0e-6d2a1b7c9e41`).
const STORE_NAMESPACE: [u8; 16] = [0x5d, 0x3c, 0x3a, 0x0e, 0x6b, 0x1f, 0x4c, 0x55, 0x9f, 0x0e, 0x6d, 0x2a, 0x1b, 0x7c, 0x9e, 0x41];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_are_tauri_labels_and_unique() {
        assert_eq!(label("a1:b/c", 3), "browser-a1_b_c-3");
        assert_ne!(label("p", 1), label("p", 2));
    }

    /// The app's own addresses (its page, its IPC, the frames' protocol, the dev server) are never a pane's; the web is.
    #[test]
    fn a_pane_never_loads_the_app_itself() {
        let url = |text: &str| Url::parse(text).unwrap();
        let dev = url("http://localhost:5173");
        for own in ["tauri://localhost/", "ipc://localhost/", "mightyhtml://h1/", "http://tauri.localhost/", "http://localhost:5173/x"] {
            assert!(app_address(&url(own), Some(&dev)), "{own}");
        }
        for web in ["https://example.com/", "http://127.0.0.1:5173/", "about:blank", "https://tauri.app/"] {
            assert!(!app_address(&url(web), Some(&dev)), "{web}");
        }
    }

    /// A workspace's panes share one store, two workspaces (or two profiles) never do, and every identifier is a version-5 UUID.
    #[test]
    fn stores_follow_the_profile_folder() {
        let a = store_identifier(Path::new("/p/browser-profiles/ws-a"));
        assert_eq!(a, store_identifier(Path::new("/p/browser-profiles/ws-a")));
        assert_ne!(a, store_identifier(Path::new("/p/browser-profiles/ws-b")));
        assert_ne!(a, store_identifier(Path::new("/q/browser-profiles/ws-a")));
        assert_eq!(a[6] >> 4, 5);
        assert_eq!(a[8] & 0xC0, 0x80);
    }
}
