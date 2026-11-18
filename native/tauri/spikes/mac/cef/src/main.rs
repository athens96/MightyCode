//! Mac Step 0, spike D3 (.omc/research/tauri/mac-plan.md §3 0b-2): CEF inside Tauri's NSApplication, through the Swift app's own C
//! bridge (`native/macos/BrowserBridge/cef_bridge.m`, built unchanged as `MightyCEFBridge.dylib` and loaded with `dlopen`).
//!
//! `assemble.sh` puts the binary in `CefSpike.app` with CEF's framework, the bridge and the four helper apps (as
//! `scripts/build-macos.sh:111-178`). Then `CefSpike.app/Contents/MacOS/cef-spike --out <folder>`:
//!
//! 1. after Tauri built its event loop (tao's `TaoApp` exists) and before it runs, gives that class CEF's `CrAppProtocol` methods
//!    (`isHandlingSendEvent`, `setHandlingSendEvent:`, and `sendEvent:` wrapped to set the flag), loads CEF and initializes it (the
//!    bridge requires that before the run loop starts);
//! 2. once the window is up, pumps CEF at 60 Hz in the common run loop modes, lays an NSView over the page and opens a browser in it
//!    on a red fixture page served on 127.0.0.1 with a per-pane cache folder;
//! 3. waits for the load, pictures the window (red pixels over the page's blue), closes the browser and shuts CEF down.
//!
//! It works when CEF initialized, the page loaded and showed on screen, the browser closed and the shutdown completed in time.

fn main() {
    #[cfg(target_os = "macos")]
    run::main();
    #[cfg(not(target_os = "macos"))]
    {
        eprintln!("cef-spike runs on macOS only");
        std::process::exit(2);
    }
}

#[cfg(target_os = "macos")]
mod run {
    use std::ffi::{CString, c_char, c_int, c_void};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::{Mutex, OnceLock, mpsc};
    use std::time::{Duration, Instant};

    use objc2::{MainThreadMarker, MainThreadOnly};
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2_app_kit::NSView;
    use objc2_foundation::{NSPoint, NSRect, NSSize};
    use serde_json::{Value, json};
    use spike_common::{Answer, capture};
    use tauri::{RunEvent, WebviewUrl, WebviewWindowBuilder};

    static HANDLE: OnceLock<tauri::AppHandle> = OnceLock::new();
    static BRIDGE: OnceLock<Bridge> = OnceLock::new();
    static PREPARED: Mutex<Option<Value>> = Mutex::new(None);
    static HANDLING: AtomicBool = AtomicBool::new(false);
    static ORIGINAL_SEND_EVENT: OnceLock<usize> = OnceLock::new();
    static PUMPS: AtomicU64 = AtomicU64::new(0);
    static SHUT_DOWN: AtomicBool = AtomicBool::new(false);

    #[derive(Clone, Copy)]
    struct Ptr(*mut c_void);
    unsafe impl Send for Ptr {}

    /// The bridge's exports (cef_bridge.m).
    struct Bridge {
        load: unsafe extern "C" fn(*const c_char) -> c_int,
        initialize: unsafe extern "C" fn(*const c_char) -> c_int,
        is_initialized: unsafe extern "C" fn() -> c_int,
        show: unsafe extern "C" fn(*mut c_void, c_int, c_int, *const c_char, *const c_char) -> c_int,
        has_browser: unsafe extern "C" fn(*mut c_void) -> c_int,
        is_loading: unsafe extern "C" fn(*mut c_void) -> c_int,
        close: unsafe extern "C" fn(*mut c_void),
        is_browser_closed: unsafe extern "C" fn(*mut c_void) -> c_int,
        work: unsafe extern "C" fn(),
        shutdown: unsafe extern "C" fn(),
    }

    #[link(name = "objc")]
    unsafe extern "C" {
        fn object_getClass(object: *mut c_void) -> *mut c_void;
        fn class_getName(class: *mut c_void) -> *const c_char;
        fn sel_registerName(name: *const c_char) -> *mut c_void;
        fn class_addMethod(class: *mut c_void, selector: *mut c_void, imp: *const c_void, types: *const c_char) -> bool;
        fn class_getMethodImplementation(class: *mut c_void, selector: *mut c_void) -> *const c_void;
        fn class_replaceMethod(class: *mut c_void, selector: *mut c_void, imp: *const c_void, types: *const c_char) -> *const c_void;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFRunLoopGetMain() -> *mut c_void;
        fn CFAbsoluteTimeGetCurrent() -> f64;
        fn CFRunLoopTimerCreate(
            allocator: *const c_void,
            fire: f64,
            interval: f64,
            flags: u64,
            order: isize,
            callout: unsafe extern "C" fn(*mut c_void, *mut c_void),
            context: *mut c_void,
        ) -> *mut c_void;
        fn CFRunLoopAddTimer(run_loop: *mut c_void, timer: *mut c_void, mode: *const c_void);
        static kCFRunLoopCommonModes: *const c_void;
    }

    fn c(text: &str) -> CString {
        CString::new(text).unwrap()
    }

    pub fn main() {
        let out = spike_common::out_dir("cef-spike");
        let context = tauri::generate_context!();
        let app = tauri::Builder::default()
            .setup(move |app| {
                let _ = HANDLE.set(app.handle().clone());
                let window = WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                    .title("CEF spike")
                    .inner_size(1000.0, 700.0)
                    .build()?;
                std::thread::spawn(move || script(window, out));
                Ok(())
            })
            .build(context)
            .expect("the Tauri app");
        // Tao's NSApplication exists now; its run loop does not run yet: CEF's methods, its load and its initialization go here.
        let prepared = prepare();
        eprintln!("cef-spike: prepared {prepared}");
        *PREPARED.lock().unwrap() = Some(prepared);
        app.run(|_, event| {
            if let RunEvent::Exit = event {
                shutdown();
            }
        });
    }

    unsafe extern "C" fn is_handling_send_event(_this: *mut c_void, _cmd: *mut c_void) -> bool {
        HANDLING.load(Ordering::SeqCst)
    }

    unsafe extern "C" fn set_handling_send_event(_this: *mut c_void, _cmd: *mut c_void, handling: bool) {
        HANDLING.store(handling, Ordering::SeqCst);
    }

    /// CEF's `CefScopedSendingEvent`: the flag set while the original `sendEvent:` (tao's) runs.
    unsafe extern "C" fn send_event(this: *mut c_void, cmd: *mut c_void, event: *mut c_void) {
        let previous = HANDLING.swap(true, Ordering::SeqCst);
        if let Some(original) = ORIGINAL_SEND_EVENT.get() {
            // SAFETY: the class's own `sendEvent:` implementation, called as AppKit calls it.
            let original: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) = unsafe { std::mem::transmute(*original) };
            unsafe { original(this, cmd, event) };
        }
        HANDLING.store(previous, Ordering::SeqCst);
    }

    fn frameworks() -> PathBuf {
        let exe = std::env::current_exe().unwrap_or_default();
        exe.parent().and_then(Path::parent).map(|contents| contents.join("Frameworks")).unwrap_or_default()
    }

    fn prepare() -> Value {
        let mut report = json!({});
        // SAFETY: the Objective-C runtime on the main thread, before the run loop; methods are added to tao's class once.
        unsafe {
            let app: *mut AnyObject = objc2::msg_send![objc2::class!(NSApplication), sharedApplication];
            let class = object_getClass(app.cast());
            report["appClass"] = json!(std::ffi::CStr::from_ptr(class_getName(class)).to_string_lossy());
            let added_is = class_addMethod(class, sel_registerName(c("isHandlingSendEvent").as_ptr()), is_handling_send_event as *const c_void, c("B@:").as_ptr());
            let added_set =
                class_addMethod(class, sel_registerName(c("setHandlingSendEvent:").as_ptr()), set_handling_send_event as *const c_void, c("v@:B").as_ptr());
            let send = sel_registerName(c("sendEvent:").as_ptr());
            let original = class_getMethodImplementation(class, send);
            let _ = ORIGINAL_SEND_EVENT.set(original as usize);
            class_replaceMethod(class, send, send_event as *const c_void, c("v@:@").as_ptr());
            report["methodsAdded"] = json!({"isHandlingSendEvent": added_is, "setHandlingSendEvent": added_set, "sendEventWrapped": !original.is_null()});
        }
        let frameworks = frameworks();
        let bridge_path = frameworks.join("MightyCEFBridge.dylib");
        let path = c(&bridge_path.to_string_lossy());
        // SAFETY: dlopen of the bridge built for this spike; every symbol is the bridge's exported C function.
        let bridge = unsafe {
            let library = libc::dlopen(path.as_ptr(), libc::RTLD_NOW | libc::RTLD_GLOBAL);
            if library.is_null() {
                let error = std::ffi::CStr::from_ptr(libc::dlerror()).to_string_lossy().into_owned();
                report["error"] = json!(format!("cannot load {}: {error}", bridge_path.display()));
                return report;
            }
            let symbol = |name: &str| libc::dlsym(library, c(name).as_ptr());
            let names = [
                "mighty_cef_load",
                "mighty_cef_initialize",
                "mighty_cef_is_initialized",
                "mighty_cef_show",
                "mighty_cef_has_browser",
                "mighty_cef_is_loading",
                "mighty_cef_close",
                "mighty_cef_is_browser_closed",
                "mighty_cef_work",
                "mighty_cef_shutdown",
            ];
            let missing: Vec<&str> = names.iter().copied().filter(|name| symbol(name).is_null()).collect();
            if !missing.is_empty() {
                report["error"] = json!(format!("the bridge lacks {missing:?}"));
                return report;
            }
            Bridge {
                load: std::mem::transmute(symbol(names[0])),
                initialize: std::mem::transmute(symbol(names[1])),
                is_initialized: std::mem::transmute(symbol(names[2])),
                show: std::mem::transmute(symbol(names[3])),
                has_browser: std::mem::transmute(symbol(names[4])),
                is_loading: std::mem::transmute(symbol(names[5])),
                close: std::mem::transmute(symbol(names[6])),
                is_browser_closed: std::mem::transmute(symbol(names[7])),
                work: std::mem::transmute(symbol(names[8])),
                shutdown: std::mem::transmute(symbol(names[9])),
            }
        };
        let framework = frameworks.join("Chromium Embedded Framework.framework/Chromium Embedded Framework");
        let root = std::env::temp_dir().join(format!("cef-spike-{}", std::process::id())).join("root");
        let _ = std::fs::create_dir_all(&root);
        // SAFETY: the bridge's own contract: load, then initialize once, before the run loop.
        let (loaded, initialized) = unsafe {
            let loaded = (bridge.load)(c(&framework.to_string_lossy()).as_ptr());
            let initialized = if loaded == 1 { (bridge.initialize)(c(&root.to_string_lossy()).as_ptr()) } else { 0 };
            (loaded, initialized)
        };
        report["loaded"] = json!(loaded == 1);
        report["initialized"] = json!(initialized == 1);
        report["rootCache"] = json!(root);
        let _ = BRIDGE.set(bridge);
        report
    }

    fn shutdown() -> Option<u128> {
        if SHUT_DOWN.swap(true, Ordering::SeqCst) {
            return None;
        }
        let bridge = BRIDGE.get()?;
        let started = Instant::now();
        // SAFETY: the bridge's shutdown on the main thread (it closes what is left and pumps until CEF let go, 5 s at most).
        unsafe { (bridge.shutdown)() };
        Some(started.elapsed().as_millis())
    }

    unsafe extern "C" fn pump(_timer: *mut c_void, _info: *mut c_void) {
        if SHUT_DOWN.load(Ordering::SeqCst) {
            return;
        }
        if let Some(bridge) = BRIDGE.get() {
            PUMPS.fetch_add(1, Ordering::Relaxed);
            // SAFETY: the bridge's message loop work on the main thread (it refuses to re-enter itself).
            unsafe { (bridge.work)() };
        }
    }

    fn on_main<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, String> {
        let (tx, rx) = mpsc::channel();
        HANDLE
            .get()
            .ok_or("no app handle")?
            .run_on_main_thread(move || {
                let _ = tx.send(f());
            })
            .map_err(|e| e.to_string())?;
        rx.recv_timeout(Duration::from_secs(30)).map_err(|e| format!("the main thread did not answer: {e}"))
    }

    fn poll(limit: Duration, mut test: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + limit;
        while Instant::now() < deadline {
            if test() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        false
    }

    fn script(window: tauri::WebviewWindow, out: PathBuf) {
        std::thread::sleep(Duration::from_millis(2000));
        let mut report = json!({"prepared": PREPARED.lock().unwrap().clone()});
        let finish = |mut report: Value, code: i32| -> ! {
            report["pumps"] = json!(PUMPS.load(Ordering::Relaxed));
            report["exitCode"] = json!(code);
            spike_common::write_report(&out, &report);
            std::process::exit(code);
        };
        if BRIDGE.get().is_none() || report["prepared"]["initialized"] != json!(true) {
            report["error"] = json!("CEF did not initialize");
            finish(report, 1);
        }
        let (base, _seen) = match spike_common::serve(|_| {
            Answer::html("<html><body style='margin:0;background:#ff0000'><h1 style='color:#ffffff'>cef fixture</h1></body></html>")
        }) {
            Ok(served) => served,
            Err(error) => {
                report["error"] = json!(format!("no server: {error}"));
                finish(report, 1);
            }
        };
        let ns_view = Ptr(window.ns_view().unwrap_or(std::ptr::null_mut()));
        let ns_window = Ptr(window.ns_window().unwrap_or(std::ptr::null_mut()));
        let cache = std::env::temp_dir().join(format!("cef-spike-{}", std::process::id())).join("browser-profiles").join("workspace-a");
        let _ = std::fs::create_dir_all(&cache);
        let url = format!("{base}/");
        let opened = on_main(move || unsafe { open(ns_view, &cache, &url) }).and_then(|r| r);
        report["opened"] = opened.clone().unwrap_or_else(|e| json!({"error": e}));
        let Ok(view) = opened.and_then(|o| o["view"].as_u64().map(|v| Ptr(v as usize as *mut c_void)).ok_or("no view".into())) else {
            finish(report, 1);
        };
        let loaded = poll(Duration::from_secs(30), || {
            on_main(move || {
                let bridge = BRIDGE.get().unwrap();
                // SAFETY: the bridge's queries on the main thread.
                let view = view;
                unsafe { (bridge.has_browser)(view.0) == 1 && (bridge.is_loading)(view.0) == 0 }
            })
            .unwrap_or(false)
        });
        report["loaded"] = json!(loaded);
        std::thread::sleep(Duration::from_millis(2000));
        let pictures = out.clone();
        let collected = on_main(move || unsafe { pictures_of(ns_window, view, &pictures) }).unwrap_or_else(|e| json!({"error": e}));
        report["pictures"] = collected;
        let _ = on_main(move || {
            let view = view;
            // SAFETY: the bridge's close on the main thread.
            unsafe { (BRIDGE.get().unwrap().close)(view.0) }
        });
        let closed = poll(Duration::from_secs(10), || {
            on_main(move || {
                let view = view;
                // SAFETY: the bridge's query on the main thread.
                unsafe { (BRIDGE.get().unwrap().is_browser_closed)(view.0) == 1 }
            }).unwrap_or(false)
        });
        report["browserClosed"] = json!(closed);
        let shutdown_ms = on_main(shutdown).ok().flatten();
        let still = on_main(|| unsafe { (BRIDGE.get().unwrap().is_initialized)() }).unwrap_or(-1);
        report["shutdownMs"] = json!(shutdown_ms);
        report["initializedAfterShutdown"] = json!(still);
        let red = report["pictures"]["window"]["counts"]["red"].as_u64().unwrap_or(0);
        let red_view = report["pictures"]["viewCache"]["counts"]["red"].as_u64().unwrap_or(0);
        let page = report["pictures"]["window"]["counts"]["page"].as_u64().unwrap_or(0);
        let shown = red > 2000 || red_view > 2000;
        let works = loaded && shown && closed && shutdown_ms.is_some_and(|ms| ms < 6000) && still == 0;
        report["verdict"] = json!({"initialized": true, "loaded": loaded, "shownOnScreen": red > 2000, "shownInView": red_view > 2000,
            "pageAround": page > 2000, "browserClosed": closed, "shutDown": still == 0, "works": works});
        finish(report, if works { 0 } else { 1 });
    }

    /// The pump timer, the NSView over the page and the browser in it (points, as `cef_window_info_t` takes them on the Mac).
    unsafe fn open(parent: Ptr, cache: &Path, url: &str) -> Result<Value, String> {
        let mtm = MainThreadMarker::new().ok_or("not on the main thread")?;
        let bridge = BRIDGE.get().ok_or("no bridge")?;
        // SAFETY: a repeating CF timer in the main run loop's common modes (it fires inside nested loops too, as the bridge expects).
        unsafe {
            let timer = CFRunLoopTimerCreate(std::ptr::null(), CFAbsoluteTimeGetCurrent() + 0.016, 1.0 / 60.0, 0, 0, pump, std::ptr::null_mut());
            CFRunLoopAddTimer(CFRunLoopGetMain(), timer, kCFRunLoopCommonModes);
        }
        // SAFETY: Tauri's content view.
        let parent: &NSView = unsafe { &*(parent.0 as *const NSView) };
        let (width, height) = (600.0, 400.0);
        let bounds = parent.bounds();
        let y = if parent.isFlipped() { 60.0 } else { bounds.size.height - 60.0 - height };
        let view = NSView::initWithFrame(NSView::alloc(mtm), NSRect::new(NSPoint::new(40.0, y), NSSize::new(width, height)));
        parent.addSubview(&view);
        let raw = Retained::into_raw(view) as *mut c_void;
        // SAFETY: the bridge's show on the main thread with a live NSView.
        let shown = unsafe { (bridge.show)(raw, width as c_int, height as c_int, c(&cache.to_string_lossy()).as_ptr(), c(url).as_ptr()) };
        if shown != 1 {
            return Err("mighty_cef_show refused".into());
        }
        Ok(json!({"view": raw as usize as u64, "url": url}))
    }

    unsafe fn pictures_of(ns_window: Ptr, view: Ptr, out: &Path) -> Value {
        let red: fn(u8, u8, u8) -> bool = |r, g, b| r > 200 && g < 70 && b < 70;
        let page: fn(u8, u8, u8) -> bool = |r, g, b| r < 70 && (60..=120).contains(&g) && b > 200;
        let colours: [(&str, fn(u8, u8, u8) -> bool); 2] = [("red", red), ("page", page)];
        // SAFETY: Tauri's NSWindow and the browser's host view.
        let number: isize = unsafe { objc2::msg_send![ns_window.0 as *mut AnyObject, windowNumber] };
        let window = capture::window(number);
        let cached = unsafe { capture::view_cache(view.0.cast()) };
        json!({"window": capture::summary("window", out, &window, &colours), "viewCache": capture::summary("view-cache", out, &cached, &colours)})
    }
}
