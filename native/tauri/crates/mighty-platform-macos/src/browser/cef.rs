//! M-E: a browser pane in CEF, through the Swift app's own C bridge: `Contents/Frameworks/MightyCEFBridge.dylib` (`cef_bridge.m`
//! built unchanged, as `scripts/build-macos.sh` builds it) by `dlopen`, CEF 152 (`BrowserEngine.lock`) with `no_sandbox` and
//! `MightyCode Helper.app` as its subprocess, a request context per pane whose cache is the pane's profile folder
//! (`<browser data>/browser-profiles/<key>`, so 0.4.x's CEF sign-ins survive where a release adopts the data folder), a 60 Hz
//! `mighty_cef_work` in the run loop's common modes, `mighty_cef_shutdown` before the app exits. Swift `CefBrowserRuntime.swift`
//! (the process's runtime: load, initialize before the run loop, pump, close, shut down) and `CefBrowserEngine.swift` (a pane: its
//! host view, its history, its loading state).
//!
//! The bridge tells a pane nothing of where its browser went, so, as Swift's pane (`BrowserHistory.visit` at `loadURL`), this one
//! keeps the history of the addresses it was given (back, forward and reload load an entry again) and reports each navigation as the
//! bridge takes it; a link followed inside the page is not seen. The bridge loads a popup asked for by a click in the pane itself and cancels any
//! other, so no popup is reported. CEF downloads nothing without a download handler, which the bridge gives none.

use std::cell::RefCell;
use std::ffi::{CString, c_char, c_int, c_void};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use core_foundation::base::TCFType as _;
use core_foundation::date::CFDate;
use core_foundation::runloop::{CFRunLoop, CFRunLoopTimer, CFRunLoopTimerInvalidate, CFRunLoopTimerRef, kCFRunLoopCommonModes};
use mighty_platform::{
    BoxFuture, BrowserEvent, BrowserListener, BrowserOptions, BrowserSurface, DeviceRect, NativeSurface, PlatformError, Result,
    SurfaceState,
};
use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly as _, Message as _};
use objc2_app_kit::{NSView, NSWindow};

use super::{BLANK, refused};

/// What the bundle's `Contents/Frameworks` holds for CEF (`scripts/build-macos.sh:111-178`; the names `cef_bridge.m` expects).
pub const FRAMEWORK: &str = "Chromium Embedded Framework.framework";
pub const BRIDGE: &str = "MightyCEFBridge.dylib";
pub const HELPER: &str = "MightyCode Helper.app";

/// Swift `BrowserEngineLocator.reportMissing()`'s meaning: what a build without the engine says.
pub const MISSING: &str = "the browser engine (CEF) is not in this build";

/// The bridge's exports (`cef_bridge.m`).
#[derive(Clone, Copy)]
struct Bridge {
    load: unsafe extern "C" fn(*const c_char) -> c_int,
    initialize: unsafe extern "C" fn(*const c_char) -> c_int,
    is_initialized: unsafe extern "C" fn() -> c_int,
    show: unsafe extern "C" fn(*mut c_void, c_int, c_int, *const c_char, *const c_char) -> c_int,
    close: unsafe extern "C" fn(*mut c_void),
    is_browser_closed: unsafe extern "C" fn(*mut c_void) -> c_int,
    work: unsafe extern "C" fn(),
    shutdown: unsafe extern "C" fn(),
}

static ENGINE: OnceLock<Bridge> = OnceLock::new();
/// The setting asked for CEF at launch.
static ASKED: AtomicBool = AtomicBool::new(false);
/// CEF initialized and is not shut down.
static RUNNING: AtomicBool = AtomicBool::new(false);
/// The shutdown began: the pump does nothing more.
static STOPPING: AtomicBool = AtomicBool::new(false);
/// The open panes (and those whose browser is still closing).
static PANES: Mutex<Vec<Arc<Pane>>> = Mutex::new(Vec::new());

thread_local! {
    /// The pump's timer (the main thread's).
    static TIMER: RefCell<Option<CFRunLoopTimer>> = const { RefCell::new(None) };
}

pub fn asked() -> bool {
    ASKED.load(Ordering::SeqCst)
}

pub fn running() -> bool {
    RUNNING.load(Ordering::SeqCst)
}

/// Swift `BrowserEngineLock` (`native/macos/BrowserEngine.lock`): the pinned CEF (and Node) archives the build fetches
/// (`scripts/fetch-browser-engine.sh`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineLock {
    pub arch: String,
    pub cef: LockEntry,
    pub node: LockEntry,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockEntry {
    pub version: String,
    pub url: String,
    pub sha256: String,
}

impl EngineLock {
    pub fn parse(text: &str) -> std::result::Result<EngineLock, String> {
        let value: serde_json::Value = serde_json::from_str(text).map_err(|error| error.to_string())?;
        let text = |value: &serde_json::Value, name: &str| {
            value.get(name).and_then(serde_json::Value::as_str).map(str::to_string).ok_or_else(|| format!("no {name}"))
        };
        let entry = |name: &str| -> std::result::Result<LockEntry, String> {
            let entry = value.get(name).ok_or_else(|| format!("no {name}"))?;
            Ok(LockEntry { version: text(entry, "version")?, url: text(entry, "url")?, sha256: text(entry, "sha256")? })
        };
        Ok(EngineLock { arch: text(&value, "arch")?, cef: entry("cef")?, node: entry("node")? })
    }
}

/// The app bundle's `Contents/Frameworks` (beside `Contents/MacOS`, where the executable is).
pub fn frameworks_dir() -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_default();
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
    exe.parent().and_then(Path::parent).map(|contents| contents.join("Frameworks")).unwrap_or_default()
}

/// Swift `BrowserEngineLocator.locate`: CEF's framework in `frameworks`, or why there is none.
pub fn locate(frameworks: &Path) -> std::result::Result<PathBuf, &'static str> {
    let framework = frameworks.join(FRAMEWORK);
    if framework.exists() { Ok(framework) } else { Err(MISSING) }
}

/// Swift `BrowserProfileSupport.clearStaleLock`: removes only the Chromium singleton files a crash left in a profile folder, never
/// its data. (Swift's app never calls it, "another tab can be using this same workspace profile", and neither does this one.)
pub fn clear_stale_lock(profile: &Path) {
    for name in ["SingletonLock", "SingletonSocket", "SingletonCookie"] {
        let _ = std::fs::remove_file(profile.join(name));
    }
}

fn c_text(text: &str) -> Result<CString> {
    CString::new(text).map_err(|error| PlatformError::Invalid(error.to_string()))
}

fn c_path(path: &Path) -> Result<CString> {
    c_text(&path.to_string_lossy())
}

/// The bridge, `dlopen`ed for the process's life (Swift: "CEF may already have registered Objective-C classes"; never unloaded).
fn load_bridge(path: &Path) -> std::result::Result<Bridge, String> {
    let name = c_path(path).map_err(|error| error.to_string())?;
    // SAFETY: dlopen of the app's own bridge; every symbol below is one of its exported C functions with the type it is given.
    unsafe {
        let library = libc::dlopen(name.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL);
        if library.is_null() {
            let error = libc::dlerror();
            let error = if error.is_null() { String::new() } else { std::ffi::CStr::from_ptr(error).to_string_lossy().into_owned() };
            return Err(format!("cannot load {}: {error}", path.display()));
        }
        let symbol = |name: &str| -> std::result::Result<*mut c_void, String> {
            let text = CString::new(name).map_err(|error| error.to_string())?;
            let found = libc::dlsym(library, text.as_ptr());
            if found.is_null() { Err(format!("the bridge lacks {name}")) } else { Ok(found) }
        };
        Ok(Bridge {
            load: std::mem::transmute::<*mut c_void, unsafe extern "C" fn(*const c_char) -> c_int>(symbol("mighty_cef_load")?),
            initialize: std::mem::transmute::<*mut c_void, unsafe extern "C" fn(*const c_char) -> c_int>(symbol("mighty_cef_initialize")?),
            is_initialized: std::mem::transmute::<*mut c_void, unsafe extern "C" fn() -> c_int>(symbol("mighty_cef_is_initialized")?),
            show: std::mem::transmute::<*mut c_void, unsafe extern "C" fn(*mut c_void, c_int, c_int, *const c_char, *const c_char) -> c_int>(
                symbol("mighty_cef_show")?,
            ),
            close: std::mem::transmute::<*mut c_void, unsafe extern "C" fn(*mut c_void)>(symbol("mighty_cef_close")?),
            is_browser_closed: std::mem::transmute::<*mut c_void, unsafe extern "C" fn(*mut c_void) -> c_int>(symbol(
                "mighty_cef_is_browser_closed",
            )?),
            work: std::mem::transmute::<*mut c_void, unsafe extern "C" fn()>(symbol("mighty_cef_work")?),
            shutdown: std::mem::transmute::<*mut c_void, unsafe extern "C" fn()>(symbol("mighty_cef_shutdown")?),
        })
    }
}

/// Loads and initializes CEF and starts its pump (Swift `CefBrowserEngine.bootstrapRuntime`): on the main thread, before the run
/// loop starts (the bridge's rule: CEF's run loop observers must see the loop's first entry).
pub(super) fn start(root_cache: &Path) -> std::result::Result<(), String> {
    ASKED.store(true, Ordering::SeqCst);
    MainThreadMarker::new().ok_or("not on the main thread")?;
    let frameworks = frameworks_dir();
    let framework = locate(&frameworks)?;
    let bridge = load_bridge(&frameworks.join(BRIDGE))?;
    super::app_class::add_cef_methods().map_err(|error| error.to_string())?;
    let binary = c_path(&framework.join("Chromium Embedded Framework")).map_err(|error| error.to_string())?;
    // SAFETY: the bridge's contract: load once, then initialize once, on the main thread before the run loop.
    if unsafe { (bridge.load)(binary.as_ptr()) } != 1 {
        return Err("CEF did not load".into());
    }
    let _ = ENGINE.set(bridge);
    std::fs::create_dir_all(root_cache).map_err(|error| format!("{}: {error}", root_cache.display()))?;
    let root = c_path(root_cache).map_err(|error| error.to_string())?;
    // SAFETY: as above.
    if unsafe { (bridge.initialize)(root.as_ptr()) } != 1 || unsafe { (bridge.is_initialized)() } != 1 {
        return Err("CEF did not initialize".into());
    }
    // A repeating timer in the main run loop's common modes: it fires inside the loops Chromium spins itself (menus, drags).
    let timer = CFRunLoopTimer::new(CFDate::now().abs_time() + 0.016, 1.0 / 60.0, 0, 0, pump, std::ptr::null_mut());
    // SAFETY: CoreFoundation's own mode constant.
    CFRunLoop::get_main().add_timer(&timer, unsafe { kCFRunLoopCommonModes });
    TIMER.with(|kept| *kept.borrow_mut() = Some(timer));
    RUNNING.store(true, Ordering::SeqCst);
    Ok(())
}

/// Closes what is left and shuts CEF down (Swift `CefBrowserRuntime.shutDown`; the bridge pumps until every browser let go, 5 s at
/// most, and leaves CEF loaded if one never does). On the main thread, as the app ends; what happened, for the log.
pub(super) fn stop() -> Option<String> {
    if !RUNNING.swap(false, Ordering::SeqCst) {
        return None;
    }
    STOPPING.store(true, Ordering::SeqCst);
    TIMER.with(|kept| {
        if let Some(timer) = kept.borrow_mut().take() {
            // SAFETY: the pump's own timer, on the main thread.
            unsafe { CFRunLoopTimerInvalidate(timer.as_concrete_TypeRef()) };
        }
    });
    let bridge = ENGINE.get()?;
    let open = PANES.lock().unwrap_or_else(|p| p.into_inner()).iter().filter(|pane| !pane.lock().closing).count();
    for pane in PANES.lock().unwrap_or_else(|p| p.into_inner()).iter() {
        pane.lock().closing = true;
    }
    let started = Instant::now();
    // SAFETY: the bridge's shutdown on the main thread.
    unsafe { (bridge.shutdown)() };
    let took = started.elapsed().as_millis();
    release_closed(bridge);
    // SAFETY: the bridge's query on the main thread.
    Some(if unsafe { (bridge.is_initialized)() } == 0 {
        format!("CEF shut down in {took} ms ({open} panes open)")
    } else {
        format!("CEF kept a browser past its shutdown's {took} ms ({open} panes open): left loaded until the process ends")
    })
}

extern "C" fn pump(_timer: CFRunLoopTimerRef, _info: *mut c_void) {
    if STOPPING.load(Ordering::SeqCst) {
        return;
    }
    let Some(bridge) = ENGINE.get() else { return };
    // SAFETY: the bridge's message loop work on the main thread (it refuses to re-enter itself).
    unsafe { (bridge.work)() };
    tick(bridge);
}

/// Each pane after a pump (Swift's observers): CEF's own view keeps filling the pane's, and a closed browser's view goes.
fn tick(bridge: &Bridge) {
    let panes: Vec<Arc<Pane>> = PANES.lock().unwrap_or_else(|p| p.into_inner()).clone();
    for pane in panes {
        if !pane.lock().closing {
            // SAFETY: the pane's view is alive (released only once its browser closed), on the main thread.
            fill(unsafe { &*(pane.view as *const NSView) });
        }
    }
    release_closed(bridge);
}

/// The views of panes whose browser CEF let go of, removed and released (Swift `releaseClosedViews`).
fn release_closed(bridge: &Bridge) {
    let closed: Vec<Arc<Pane>> = {
        let mut panes = PANES.lock().unwrap_or_else(|p| p.into_inner());
        let (closed, open): (Vec<_>, Vec<_>) = panes.drain(..).partition(|pane| {
            // SAFETY: the bridge's query on the main thread.
            pane.lock().closing && unsafe { (bridge.is_browser_closed)(pane.view as *mut c_void) } == 1
        });
        *panes = open;
        closed
    };
    for pane in closed {
        // SAFETY: the reference `make` kept, given back once.
        if let Some(view) = unsafe { Retained::from_raw(pane.view as *mut NSView) } {
            view.removeFromSuperview();
        }
    }
}

/// CEF parents its own NSView in the pane's: it fills the pane's bounds (Swift `BrowserHostView.layout`).
fn fill(view: &NSView) {
    let bounds = view.bounds();
    for child in view.subviews().iter() {
        let frame = child.frame();
        if frame.origin.x != bounds.origin.x
            || frame.origin.y != bounds.origin.y
            || frame.size.width != bounds.size.width
            || frame.size.height != bounds.size.height
        {
            child.setFrame(bounds);
        }
    }
}

/// One pane: its host view and its navigation state (Swift `CefBrowserEngine`).
struct Pane {
    /// The host NSView (its address), one reference kept until CEF let go of it.
    view: usize,
    /// Its profile folder (the request context's cache).
    cache: CString,
    listener: Option<BrowserListener>,
    state: Mutex<PaneState>,
}

#[derive(Default)]
struct PaneState {
    /// The addresses it was given (Swift `BrowserHistory`), and the one shown.
    history: Vec<String>,
    at: usize,
    closing: bool,
}

impl Pane {
    fn lock(&self) -> std::sync::MutexGuard<'_, PaneState> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }
}

/// Runs `work` on the main thread and waits for its answer.
async fn on_main<T: Send + 'static>(
    window: &tauri::Window,
    what: &str,
    work: impl FnOnce(MainThreadMarker) -> Result<T> + Send + 'static,
) -> Result<T> {
    let (sender, answer) = tokio::sync::oneshot::channel();
    window
        .run_on_main_thread(move || {
            let result = match MainThreadMarker::new() {
                Some(mtm) => work(mtm),
                None => Err(PlatformError::Refused("not on the main thread".into())),
            };
            let _ = sender.send(result);
        })
        .map_err(|error| refused(what, error))?;
    answer.await.map_err(|error| refused(what, error))?
}

/// Runs `work` on the main thread without waiting.
fn on_main_later(window: &tauri::Window, what: &str, work: impl FnOnce(MainThreadMarker) + Send + 'static) -> Result<()> {
    window
        .run_on_main_thread(move || {
            if let Some(mtm) = MainThreadMarker::new() {
                work(mtm);
            }
        })
        .map_err(|error| refused(what, error))
}

pub async fn open(window: &tauri::Window, options: BrowserOptions) -> Result<Box<dyn BrowserSurface>> {
    let url = super::first_address(window, &options.url)?;
    std::fs::create_dir_all(&options.data_dir).map_err(|error| refused("the browser profile", error))?;
    let cache = c_path(&options.data_dir)?;
    let BrowserOptions { bounds, listener, .. } = options;
    let first = url.to_string();
    let ns_window = window.ns_window().map_err(|error| refused("the window", error))? as usize;
    let pane = on_main(window, "the browser view", move |mtm| {
        let bridge = ENGINE.get().ok_or_else(|| PlatformError::Refused("CEF is not running".into()))?;
        // SAFETY: Tauri's NSWindow of the main window, alive while the app runs, on the main thread.
        let ns_window = unsafe { &*(ns_window as *const NSWindow) };
        let content = ns_window.contentView().ok_or_else(|| PlatformError::Refused("the window has no content view".into()))?;
        let frame = super::frame_in(&content, bounds, ns_window.backingScaleFactor());
        let view = NSView::initWithFrame(NSView::alloc(mtm), frame);
        content.addSubview(&view);
        let address = c_text(&first)?;
        let size = view.bounds().size;
        let pane = Arc::new(Pane {
            view: Retained::into_raw(view) as usize,
            cache,
            listener: listener.clone(),
            state: Mutex::new(PaneState { history: if first == BLANK { Vec::new() } else { vec![first.clone()] }, ..PaneState::default() }),
        });
        let raw = pane.view as *mut c_void;
        // SAFETY: the bridge's show on the main thread with a live NSView (points, as `cef_window_info_t` takes them on the Mac).
        let shown = unsafe { (bridge.show)(raw, points(size.width), points(size.height), pane.cache.as_ptr(), address.as_ptr()) };
        if shown != 1 {
            // SAFETY: the reference just kept, given back.
            if let Some(view) = unsafe { Retained::from_raw(raw.cast::<NSView>()) } {
                view.removeFromSuperview();
            }
            return Err(PlatformError::Refused("CEF refused the browser".into()));
        }
        PANES.lock().unwrap_or_else(|p| p.into_inner()).push(pane.clone());
        if let Some(listener) = listener {
            listener.send(BrowserEvent::Navigated { url: Some(first), success: true });
            // SAFETY: the view just made, kept until its browser closed.
            super::watch_focus(unsafe { &*(raw as *const NSView) }, listener, mtm);
        }
        Ok(pane)
    })
    .await?;
    Ok(Box::new(CefBrowser { window: window.clone(), pane }) as Box<dyn BrowserSurface>)
}

/// A size in points as the bridge takes it (at least 1).
fn points(value: f64) -> c_int {
    if value.is_finite() { value.clamp(1.0, f64::from(c_int::MAX)) as c_int } else { 1 }
}

struct CefBrowser {
    window: tauri::Window,
    pane: Arc<Pane>,
}

impl CefBrowser {
    fn open_pane(&self) -> Result<()> {
        if self.pane.lock().closing { Err(PlatformError::Invalid("the view is closed".into())) } else { Ok(()) }
    }

    /// Loads `url` in the pane's browser (the bridge navigates the browser it has, or keeps the address until it is made), and tells
    /// the host the pane went there once the bridge took it.
    fn present(&self, url: String) -> Result<()> {
        let pane = self.pane.clone();
        on_main_later(&self.window, "navigate", move |_| {
            let (Some(bridge), Ok(address)) = (ENGINE.get(), c_text(&url)) else { return };
            // SAFETY: the pane's view is alive until its browser closed; the bridge on the main thread.
            let size = unsafe { &*(pane.view as *const NSView) }.bounds().size;
            let shown = unsafe {
                (bridge.show)(pane.view as *mut c_void, points(size.width), points(size.height), pane.cache.as_ptr(), address.as_ptr())
            };
            if let Some(listener) = &pane.listener {
                listener.send(BrowserEvent::Navigated { url: Some(url), success: shown == 1 });
            }
        })
    }

    /// The history's entry `by` steps away, if it has one: shown again (Swift `goBack`/`goForward`).
    fn step(&self, by: isize) -> Result<()> {
        self.open_pane()?;
        let url = {
            let mut state = self.pane.lock();
            let Some(at) = state.at.checked_add_signed(by).filter(|at| *at < state.history.len()) else { return Ok(()) };
            state.at = at;
            state.history[at].clone()
        };
        self.present(url)
    }
}

impl NativeSurface for CefBrowser {
    fn place(&self, rect: DeviceRect) -> Result<()> {
        self.open_pane()?;
        let view = self.pane.view;
        on_main_later(&self.window, "place", move |_| {
            // SAFETY: the pane's view is alive until its browser closed, on the main thread.
            let view = unsafe { &*(view as *const NSView) };
            // SAFETY: AppKit's own superview of a live view, on the main thread.
            let (Some(parent), Some(window)) = (unsafe { view.superview() }, view.window()) else { return };
            view.setFrame(super::frame_in(&parent, rect, window.backingScaleFactor()));
            fill(view);
        })
    }

    fn set_visible(&self, visible: bool) -> Result<()> {
        self.open_pane()?;
        let view = self.pane.view;
        // SAFETY: as in `place`.
        on_main_later(&self.window, "show", move |_| unsafe { &*(view as *const NSView) }.setHidden(!visible))
    }

    fn focus(&self) -> Result<()> {
        self.open_pane()?;
        let view = self.pane.view;
        on_main_later(&self.window, "focus", move |_| {
            // SAFETY: as in `place`.
            let view = unsafe { &*(view as *const NSView) };
            let target = view.subviews().firstObject().unwrap_or_else(|| view.retain());
            if let Some(window) = view.window() {
                window.makeFirstResponder(Some(&target));
            }
        })
    }

    /// AppKit's picture of the pane's view (`cacheDisplayInRect:`, which holds CEF's frame: Step 0's cef spike), as a PNG `data:` URL.
    fn snapshot(&self) -> BoxFuture<'_, Result<Option<String>>> {
        Box::pin(async move {
            self.open_pane()?;
            let view = self.pane.view;
            let png = on_main(&self.window, "snapshot", move |_| {
                // SAFETY: as in `place`.
                let view = unsafe { &*(view as *const NSView) };
                let bounds = view.bounds();
                let Some(rep) = view.bitmapImageRepForCachingDisplayInRect(bounds) else { return Ok(None) };
                view.cacheDisplayInRect_toBitmapImageRep(bounds, &rep);
                Ok(super::png_of(&rep))
            })
            .await?;
            Ok(png.filter(|png| !png.is_empty()).map(|png| super::data_url(&png)))
        })
    }

    fn inspect(&self) -> BoxFuture<'_, Result<SurfaceState>> {
        Box::pin(async move {
            self.open_pane()?;
            let view = self.pane.view;
            // SAFETY: as in `place`.
            let (bounds, visible) =
                on_main(&self.window, "inspect", move |_| Ok(super::device_state(unsafe { &*(view as *const NSView) }))).await?;
            let url = {
                let state = self.pane.lock();
                state.history.get(state.at).cloned()
            };
            Ok(SurfaceState { bounds, visible, url })
        })
    }

    /// Out of the window at once, as Swift's pane left SwiftUI's hierarchy before its engine closed; CEF closes its browser
    /// asynchronously and the pump releases the view once it did (Swift `close(view:)`, `releaseClosedViews`).
    fn close(&self) -> Result<()> {
        {
            let mut state = self.pane.lock();
            if state.closing {
                return Err(PlatformError::Invalid("the view is closed".into()));
            }
            state.closing = true;
        }
        let view = self.pane.view;
        on_main_later(&self.window, "close", move |_| {
            super::forget_focus(view);
            // SAFETY: as in `place` (the pane keeps its own reference until CEF let go); the bridge's close on the main thread.
            unsafe { &*(view as *const NSView) }.removeFromSuperview();
            if let Some(bridge) = ENGINE.get().filter(|_| running()) {
                unsafe { (bridge.close)(view as *mut c_void) };
            }
        })
    }
}

impl BrowserSurface for CefBrowser {
    /// Swift `loadURL`: a visit of the history, then shown.
    fn navigate(&self, url: &str) -> Result<()> {
        self.open_pane()?;
        let url = tauri::Url::parse(url).map_err(|error| PlatformError::Invalid(format!("address: {error}")))?;
        if super::app_address(&url, super::dev_server(&self.window).as_ref()) {
            return Err(PlatformError::Invalid("address: the app's own".into()));
        }
        let url = url.to_string();
        {
            let mut state = self.pane.lock();
            let keep = if state.history.is_empty() { 0 } else { state.at + 1 };
            state.history.truncate(keep);
            state.history.push(url.clone());
            state.at = state.history.len() - 1;
        }
        self.present(url)
    }

    fn go_back(&self) -> Result<()> {
        self.step(-1)
    }

    fn go_forward(&self) -> Result<()> {
        self.step(1)
    }

    /// Swift `reload`: the address shown, loaded again.
    fn reload(&self) -> Result<()> {
        self.open_pane()?;
        let current = {
            let state = self.pane.lock();
            state.history.get(state.at).cloned()
        };
        match current {
            Some(url) => self.present(url),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        let dir = std::env::temp_dir().join(format!("mighty-cef-{name}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// `BrowserEngineTests.swift:6 "browserEngineLockParses"`.
    #[test]
    fn browser_engine_lock_parses() {
        let text = r#"{"arch":"arm64","cef":{"version":"152.0.9+g07f67cd+chromium-152.0.7977.134","url":"https://cef-builds.spotifycdn.com/cef_binary_152.0.9%2Bg07f67cd%2Bchromium-152.0.7977.134_macosarm64_minimal.tar.bz2","sha256":"f4efc5e8447e37d3dd1a982047310f1af16d08c5e81136d33a92e6cbff2717f7"},"node":{"version":"22.23.2","url":"https://nodejs.org/dist/v22.23.2/node-v22.23.2-darwin-arm64.tar.gz","sha256":"61130f394c1630d211dd50aecc4353d379480f36d3ac913cd85dbba1aed585c6"}}"#;
        let lock = EngineLock::parse(text).unwrap();
        assert_eq!(lock.arch, "arm64");
        assert_eq!(lock.cef.version, "152.0.9+g07f67cd+chromium-152.0.7977.134");
        assert_eq!(lock.cef.sha256.len(), 64);
        assert_eq!(lock.node.version, "22.23.2");
        assert_eq!(lock.node.sha256.len(), 64);
        // The repository's own lock reads too (the one the build fetches by).
        let ours = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../../macos/BrowserEngine.lock")).unwrap();
        assert_eq!(EngineLock::parse(&ours).unwrap().arch, "arm64");
        assert!(EngineLock::parse(r#"{"arch":"arm64"}"#).is_err());
    }

    /// `BrowserEngineTests.swift:18 "browserEngineLocatorReportsMissingEngine"`: a build without the engine (the test binary's folder
    /// has no CEF) says it is missing, with the one reason a missing engine gives, rather than a bogus path.
    #[test]
    fn browser_engine_locator_reports_missing_engine() {
        assert_eq!(locate(&frameworks_dir()), Err(MISSING));
        assert!(!MISSING.is_empty());
        let bundle = folder("frameworks");
        std::fs::create_dir_all(bundle.join(FRAMEWORK)).unwrap();
        assert_eq!(locate(&bundle), Ok(bundle.join(FRAMEWORK)));
        let _ = std::fs::remove_dir_all(&bundle);
    }

    /// `BrowserProfileTests.swift:6 "clearStaleLockRemovesOnlyLockFiles"`.
    #[test]
    fn clear_stale_lock_removes_only_lock_files() {
        let dir = folder("lock");
        for name in ["SingletonLock", "SingletonSocket", "SingletonCookie", "Cookies"] {
            std::fs::write(dir.join(name), b"lock").unwrap();
        }
        clear_stale_lock(&dir);
        for name in ["SingletonLock", "SingletonSocket", "SingletonCookie"] {
            assert!(!dir.join(name).exists(), "expected {name} to be removed");
        }
        assert!(dir.join("Cookies").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `BrowserProfileTests.swift:26 "clearStaleLockKeepsProfileData"`.
    #[test]
    fn clear_stale_lock_keeps_profile_data() {
        let dir = folder("data");
        for name in ["SingletonLock", "SingletonSocket", "SingletonCookie"] {
            std::fs::write(dir.join(name), b"lock").unwrap();
        }
        let data = ["Cookies", "History", "Local State", "Preferences"];
        for name in data {
            std::fs::write(dir.join(name), b"data").unwrap();
        }
        clear_stale_lock(&dir);
        for name in data {
            assert!(dir.join(name).exists(), "expected {name} to be kept");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sizes_reach_the_bridge_as_whole_points() {
        assert_eq!((points(300.6), points(0.0), points(f64::NAN), points(-5.0)), (300, 1, 1, 1));
    }
}
