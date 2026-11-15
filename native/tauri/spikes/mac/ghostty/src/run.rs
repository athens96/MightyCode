//! The spike itself (see main.rs). Everything AppKit and Ghostty runs on the main thread through `on_main`; the script thread
//! sleeps between the steps; the PTY's reader thread feeds Ghostty.

use std::collections::BTreeMap;
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicPtr, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock, mpsc};
use std::time::Duration;

use objc2::{MainThreadMarker, MainThreadOnly};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::NSView;
use objc2_foundation::{NSPoint, NSRect, NSSize};
use serde_json::{Value, json};
use spike_common::capture;
use tauri::{WebviewUrl, WebviewWindowBuilder};

use crate::{pty, sys};

static HANDLE: OnceLock<tauri::AppHandle> = OnceLock::new();

/// What the callbacks, the reader and the main thread share.
struct Shared {
    app: AtomicPtr<c_void>,
    surface: AtomicPtr<c_void>,
    /// The Swift route's handle.
    swift: AtomicPtr<c_void>,
    master: AtomicI32,
    tick_queued: AtomicBool,
    draw_queued: AtomicBool,
    wakeups: AtomicU64,
    ticks: AtomicU64,
    draws: AtomicU64,
    clipboard_writes: AtomicU64,
    closes: AtomicU64,
    output: Mutex<Vec<u8>>,
    input: Mutex<Vec<u8>>,
    resizes: Mutex<Vec<Value>>,
    actions: Mutex<BTreeMap<String, u64>>,
    titles: Mutex<Vec<String>>,
    pwds: Mutex<Vec<String>>,
}

static SHARED: Shared = Shared {
    app: AtomicPtr::new(std::ptr::null_mut()),
    surface: AtomicPtr::new(std::ptr::null_mut()),
    swift: AtomicPtr::new(std::ptr::null_mut()),
    master: AtomicI32::new(-1),
    tick_queued: AtomicBool::new(false),
    draw_queued: AtomicBool::new(false),
    wakeups: AtomicU64::new(0),
    ticks: AtomicU64::new(0),
    draws: AtomicU64::new(0),
    clipboard_writes: AtomicU64::new(0),
    closes: AtomicU64::new(0),
    output: Mutex::new(Vec::new()),
    input: Mutex::new(Vec::new()),
    resizes: Mutex::new(Vec::new()),
    actions: Mutex::new(BTreeMap::new()),
    titles: Mutex::new(Vec::new()),
    pwds: Mutex::new(Vec::new()),
};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Route {
    Capi,
    Swift,
}

/// A pointer handed between threads (only ever used on the main thread).
#[derive(Clone, Copy)]
struct Ptr(*mut c_void);
unsafe impl Send for Ptr {}

/// Where the terminal's view lies, in the content view's points (top left 40, 60).
const FIRST: (f64, f64, f64, f64) = (40.0, 60.0, 640.0, 360.0);
const SECOND: (f64, f64, f64, f64) = (40.0, 60.0, 760.0, 440.0);

pub fn main() {
    let out = spike_common::out_dir("ghostty-spike");
    let route = match spike_common::arg("--route").as_deref() {
        Some("swift") => Route::Swift,
        _ => Route::Capi,
    };
    let context = tauri::generate_context!();
    let app = tauri::Builder::default()
        .setup(move |app| {
            let _ = HANDLE.set(app.handle().clone());
            let window = WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                .title("Ghostty spike")
                .inner_size(1000.0, 700.0)
                .build()?;
            std::thread::spawn(move || script(window, out, route));
            Ok(())
        })
        .build(context)
        .expect("the Tauri app");
    app.run(|_, _| {});
}

/// Runs `f` on the main thread and waits for its answer (30 s at most).
fn on_main<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, String> {
    let (tx, rx) = mpsc::channel();
    let handle = HANDLE.get().ok_or("no app handle")?;
    handle
        .run_on_main_thread(move || {
            let _ = tx.send(f());
        })
        .map_err(|e| e.to_string())?;
    rx.recv_timeout(Duration::from_secs(30)).map_err(|e| format!("the main thread did not answer: {e}"))
}

fn script(window: tauri::WebviewWindow, out: PathBuf, route: Route) {
    let mut report = json!({"route": format!("{route:?}").to_lowercase(), "steps": []});
    let step = |report: &mut Value, name: &str, value: Value| {
        report["steps"].as_array_mut().unwrap().push(json!({"step": name, "result": value.clone()}));
        eprintln!("ghostty-spike: {name}: {value}");
    };
    std::thread::sleep(Duration::from_millis(2500));
    let ns_view = Ptr(window.ns_view().unwrap_or(std::ptr::null_mut()));
    let ns_window = Ptr(window.ns_window().unwrap_or(std::ptr::null_mut()));
    let started = on_main(move || unsafe { start(route, ns_view) }).and_then(|r| r);
    step(&mut report, "start", started.clone().unwrap_or_else(|e| json!({"error": e})));
    let Ok(started) = started else {
        finish(report, &out, 1);
        return;
    };
    report["started"] = started;
    // Ghostty's ticks and draws at 60 Hz from here on.
    std::thread::spawn(|| {
        loop {
            std::thread::sleep(Duration::from_millis(16));
            queue_draw();
        }
    });
    std::thread::sleep(Duration::from_millis(2000));
    let typed = on_main(move || unsafe { type_line(route, "echo typed-$((6*7))") });
    step(&mut report, "typed", json!(typed.is_ok()));
    std::thread::sleep(Duration::from_millis(1500));
    let resized = on_main(move || unsafe { resize(route, ns_view) }).and_then(|r| r);
    step(&mut report, "resized", resized.clone().unwrap_or_else(|e| json!({"error": e})));
    std::thread::sleep(Duration::from_millis(1200));
    let _ = on_main(move || unsafe { type_line(route, "stty size") });
    std::thread::sleep(Duration::from_millis(2000));
    let pictures = out.clone();
    let collected = on_main(move || unsafe { collect(route, ns_window, pictures) });
    let collected = collected.unwrap_or_else(|e| json!({"error": e}));
    report["collected"] = collected;
    report["screencapture"] = screencapture(&out, &report);
    let code = verdict(&mut report);
    let _ = on_main(move || unsafe { teardown(route) });
    finish(report, &out, code);
}

fn finish(mut report: Value, out: &Path, code: i32) {
    report["counters"] = json!({
        "wakeups": SHARED.wakeups.load(Ordering::Relaxed),
        "ticks": SHARED.ticks.load(Ordering::Relaxed),
        "draws": SHARED.draws.load(Ordering::Relaxed),
        "clipboardWrites": SHARED.clipboard_writes.load(Ordering::Relaxed),
        "closes": SHARED.closes.load(Ordering::Relaxed),
        "inputBytes": SHARED.input.lock().unwrap().len(),
        "outputBytes": SHARED.output.lock().unwrap().len(),
    });
    report["actions"] = json!(*SHARED.actions.lock().unwrap());
    report["titles"] = json!(*SHARED.titles.lock().unwrap());
    report["pwds"] = json!(*SHARED.pwds.lock().unwrap());
    report["resizeCallbacks"] = json!(*SHARED.resizes.lock().unwrap());
    report["input"] = json!(String::from_utf8_lossy(&SHARED.input.lock().unwrap()));
    report["output"] = json!(String::from_utf8_lossy(&SHARED.output.lock().unwrap()));
    report["exitCode"] = json!(code);
    spike_common::write_report(out, &report);
    std::process::exit(code);
}

// ── the main thread's steps ───────────────────────────────────────────────────────────────────────────────────────────────────────

/// The NSView's frame for `(x, top, width, height)` in `parent`, whose y axis may point up (AppKit's default) or down.
unsafe fn frame_in(parent: &NSView, (x, top, width, height): (f64, f64, f64, f64)) -> NSRect {
    let bounds = parent.bounds();
    let y = if parent.isFlipped() { top } else { bounds.size.height - top - height };
    NSRect::new(NSPoint::new(x, y), NSSize::new(width, height))
}

unsafe fn start(route: Route, parent: Ptr) -> Result<Value, String> {
    let mtm = MainThreadMarker::new().ok_or("not on the main thread")?;
    if parent.0.is_null() {
        return Err("the window has no NSView".into());
    }
    // SAFETY: Tauri's content view of the main window, alive with it.
    let parent: &NSView = unsafe { &*(parent.0 as *const NSView) };
    let scale = parent.window().map(|w| w.backingScaleFactor()).unwrap_or(2.0);
    let frame = unsafe { frame_in(parent, FIRST) };
    let pty = pty::open(80, 24)?;
    SHARED.master.store(pty.master, Ordering::SeqCst);
    let parent_class = unsafe { (*(parent as *const NSView as *const AnyObject)).class().name().to_string_lossy().into_owned() };
    let subviews_before = parent.subviews().len();
    let mut started = json!({"scale": scale, "parentClass": parent_class, "parentFlipped": parent.isFlipped(),
        "subviewsBefore": subviews_before, "ptyPid": pty.pid});
    match route {
        Route::Capi => {
            let view = NSView::initWithFrame(NSView::alloc(mtm), frame);
            view.setWantsLayer(true);
            parent.addSubview(&view);
            let view = Retained::into_raw(view) as *mut c_void;
            CAPI_VIEW.store(view, Ordering::SeqCst);
            let ghostty = unsafe { start_ghostty(view, frame, scale)? };
            started["ghostty"] = ghostty;
            started["view"] = json!(format!("{view:p}"));
        }
        Route::Swift => {
            #[cfg(spike_swift)]
            {
                let handle = unsafe {
                    sys::swift::mg_spike_open(
                        parent as *const NSView as *mut c_void,
                        frame.origin.x,
                        frame.origin.y,
                        frame.size.width,
                        frame.size.height,
                        std::ptr::null_mut(),
                        swift_input,
                        swift_resize,
                    )
                };
                if handle.is_null() {
                    return Err("mg_spike_open gave no terminal".into());
                }
                SHARED.swift.store(handle, Ordering::SeqCst);
            }
            #[cfg(not(spike_swift))]
            return Err("this binary was built without the Swift bridge (MIGHTY_SPIKE_SWIFT=1)".into());
        }
    }
    started["subviewsAfter"] = json!(parent.subviews().len());
    // The PTY's output goes to Ghostty from the reader thread (Ghostty queues it; Swift's in-memory session does the same).
    pty::spawn_reader(pty.master, move |bytes| {
        SHARED.output.lock().unwrap().extend_from_slice(bytes);
        let surface = SHARED.surface.load(Ordering::SeqCst);
        if !surface.is_null() {
            // SAFETY: the live surface; libghostty takes a copy.
            unsafe { sys::ghostty_surface_write_buffer(surface, bytes.as_ptr(), bytes.len()) };
        }
        #[cfg(spike_swift)]
        {
            let handle = SHARED.swift.load(Ordering::SeqCst);
            if !handle.is_null() {
                // SAFETY: the live Swift terminal; its session copies the bytes onto its queue.
                unsafe { sys::swift::mg_spike_write(handle, bytes.as_ptr(), bytes.len() as isize) };
            }
        }
        queue_tick();
    });
    Ok(started)
}

unsafe fn start_ghostty(view: *mut c_void, frame: NSRect, scale: f64) -> Result<Value, String> {
    // SAFETY: libghostty's documented order: init, config, app, surface, all on the main thread.
    unsafe {
        let init = sys::ghostty_init(0, std::ptr::null_mut());
        let info = sys::ghostty_info();
        let version = if info.version.is_null() {
            String::new()
        } else {
            String::from_utf8_lossy(std::slice::from_raw_parts(info.version.cast::<u8>(), info.version_len)).into_owned()
        };
        let config = sys::ghostty_config_new();
        if config.is_null() {
            return Err("ghostty_config_new gave nothing".into());
        }
        // Swift's settings (AppStore+Terminals.swift:64-105), never the user's own Ghostty config.
        let file = std::env::temp_dir().join(format!("ghostty-spike-{}.conf", std::process::id()));
        std::fs::write(
            &file,
            "font-size = 14\nbackground = 202020\nforeground = dedcd8\ncursor-color = e1ab91\nwindow-padding-x = 10\n\
             window-padding-y = 10\nclipboard-read = deny\nclipboard-write = ask\nscrollback-limit = 10000000\n",
        )
        .map_err(|e| e.to_string())?;
        let path = CString::new(file.to_string_lossy().into_owned()).unwrap();
        sys::ghostty_config_load_file(config, path.as_ptr());
        sys::ghostty_config_finalize(config);
        let mut diagnostics = Vec::new();
        for index in 0..sys::ghostty_config_diagnostics_count(config) {
            let message = sys::ghostty_config_get_diagnostic(config, index).message;
            if !message.is_null() {
                diagnostics.push(CStr::from_ptr(message).to_string_lossy().into_owned());
            }
        }
        let runtime = sys::ghostty_runtime_config_s {
            userdata: std::ptr::null_mut(),
            supports_selection_clipboard: false,
            wakeup_cb: Some(wakeup),
            action_cb: Some(action),
            read_clipboard_cb: Some(read_clipboard),
            confirm_read_clipboard_cb: Some(confirm_read_clipboard),
            write_clipboard_cb: Some(write_clipboard),
            close_surface_cb: Some(close_surface),
        };
        let app = sys::ghostty_app_new(&runtime, config);
        if app.is_null() {
            return Err(format!("ghostty_app_new gave nothing (init {init}, diagnostics {diagnostics:?})"));
        }
        SHARED.app.store(app, Ordering::SeqCst);
        let mut surface_config = sys::ghostty_surface_config_new();
        surface_config.platform_tag = sys::GHOSTTY_PLATFORM_MACOS;
        surface_config.platform = view;
        surface_config.userdata = std::ptr::null_mut();
        surface_config.backend = sys::GHOSTTY_SURFACE_IO_BACKEND_HOST_MANAGED;
        surface_config.receive_userdata = std::ptr::null_mut();
        surface_config.receive_buffer = Some(receive_buffer);
        surface_config.receive_resize = Some(receive_resize);
        surface_config.scale_factor = scale;
        surface_config.font_size = 14.0;
        surface_config.context = sys::GHOSTTY_SURFACE_CONTEXT_WINDOW;
        let surface = sys::ghostty_surface_new(app, &surface_config);
        if surface.is_null() {
            return Err("ghostty_surface_new gave nothing".into());
        }
        sys::ghostty_surface_set_content_scale(surface, scale, scale);
        sys::ghostty_surface_set_size(surface, (frame.size.width * scale) as u32, (frame.size.height * scale) as u32);
        sys::ghostty_app_set_focus(app, true);
        sys::ghostty_surface_set_focus(surface, true);
        sys::ghostty_surface_set_occlusion(surface, true);
        SHARED.surface.store(surface, Ordering::SeqCst);
        sys::ghostty_surface_refresh(surface);
        let size = sys::ghostty_surface_size(surface);
        let (layer, _) = capture::layer_surface(view.cast());
        Ok(json!({"init": init, "version": version, "buildMode": info.build_mode, "diagnostics": diagnostics,
            "grid": {"columns": size.columns, "rows": size.rows, "cellWidth": size.cell_width_px, "cellHeight": size.cell_height_px},
            "layer": layer}))
    }
}

/// Types `text` and presses Return: the C API's text input and key event (macOS key code 36), or the Swift view's paste and key.
unsafe fn type_line(route: Route, text: &str) {
    let line = CString::new(text).unwrap();
    match route {
        Route::Capi => {
            let surface = SHARED.surface.load(Ordering::SeqCst);
            if surface.is_null() {
                return;
            }
            // SAFETY: the live surface on the main thread.
            unsafe {
                sys::ghostty_surface_text(surface, line.as_ptr(), text.len());
                let enter = CString::new("\r").unwrap();
                for action in [sys::GHOSTTY_ACTION_PRESS, sys::GHOSTTY_ACTION_RELEASE] {
                    let key = sys::ghostty_input_key_s {
                        action,
                        mods: 0,
                        consumed_mods: 0,
                        keycode: 36,
                        text: if action == sys::GHOSTTY_ACTION_PRESS { enter.as_ptr() } else { std::ptr::null() },
                        unshifted_codepoint: 13,
                        composing: false,
                    };
                    sys::ghostty_surface_key(surface, key);
                }
            }
        }
        Route::Swift => {
            #[cfg(spike_swift)]
            {
                let handle = SHARED.swift.load(Ordering::SeqCst);
                if !handle.is_null() {
                    // SAFETY: the live Swift terminal on the main thread.
                    unsafe {
                        sys::swift::mg_spike_type(handle, line.as_ptr());
                        sys::swift::mg_spike_enter(handle);
                    }
                }
            }
        }
    }
}

unsafe fn resize(route: Route, parent: Ptr) -> Result<Value, String> {
    // SAFETY: Tauri's content view, alive with the window.
    let parent: &NSView = unsafe { &*(parent.0 as *const NSView) };
    let frame = unsafe { frame_in(parent, SECOND) };
    let scale = parent.window().map(|w| w.backingScaleFactor()).unwrap_or(2.0);
    let view = unsafe { terminal_view(route) };
    if view.is_null() {
        return Err("no terminal view".into());
    }
    // SAFETY: the terminal's NSView.
    let ns_view: &NSView = unsafe { &*(view as *const NSView) };
    ns_view.setFrame(frame);
    if route == Route::Capi {
        let surface = SHARED.surface.load(Ordering::SeqCst);
        // SAFETY: the live surface.
        unsafe { sys::ghostty_surface_set_size(surface, (frame.size.width * scale) as u32, (frame.size.height * scale) as u32) };
    }
    Ok(json!({"points": [frame.size.width, frame.size.height], "pixels": [frame.size.width * scale, frame.size.height * scale]}))
}

unsafe fn terminal_view(route: Route) -> *mut c_void {
    match route {
        Route::Capi => {
            // The view Ghostty draws in is the surface's platform view; the spike keeps it as the parent's last subview.
            let surface = SHARED.surface.load(Ordering::SeqCst);
            if surface.is_null() {
                return std::ptr::null_mut();
            }
            CAPI_VIEW.load(Ordering::SeqCst)
        }
        Route::Swift => {
            #[cfg(spike_swift)]
            {
                let handle = SHARED.swift.load(Ordering::SeqCst);
                if handle.is_null() {
                    return std::ptr::null_mut();
                }
                // SAFETY: the live Swift terminal.
                return unsafe { sys::swift::mg_spike_view(handle) };
            }
            #[cfg(not(spike_swift))]
            std::ptr::null_mut()
        }
    }
}

static CAPI_VIEW: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());

/// The grid read back row by row (Swift `readViewportText`), and the three pictures.
unsafe fn collect(route: Route, ns_window: Ptr, out: PathBuf) -> Value {
    let text = unsafe { viewport_text(route) };
    let view = unsafe { terminal_view(route) };
    let red: fn(u8, u8, u8) -> bool = |r, g, b| r > 200 && g < 70 && b < 70;
    let page: fn(u8, u8, u8) -> bool = |r, g, b| r < 70 && (60..=120).contains(&g) && b > 200;
    let dark: fn(u8, u8, u8) -> bool = |r, g, b| (16..=52).contains(&r) && (16..=52).contains(&g) && (16..=52).contains(&b);
    let colours: [(&str, fn(u8, u8, u8) -> bool); 3] = [("red", red), ("page", page), ("terminalBackground", dark)];
    let number: isize = if ns_window.0.is_null() {
        0
    } else {
        // SAFETY: Tauri's NSWindow.
        unsafe { objc2::msg_send![ns_window.0 as *mut AnyObject, windowNumber] }
    };
    let window = capture::window(number);
    let (layer, surface) = if view.is_null() { ("none".into(), Err("no view".into())) } else { unsafe { capture::layer_surface(view.cast()) } };
    let cached = if view.is_null() { Err("no view".into()) } else { unsafe { capture::view_cache(view.cast()) } };
    json!({
        "viewport": text,
        "windowNumber": number,
        "layer": layer,
        "window": capture::summary("window", &out, &window, &colours),
        "viewCache": capture::summary("view-cache", &out, &cached, &colours),
        "ioSurface": capture::summary("io-surface", &out, &surface, &colours),
    })
}

unsafe fn viewport_text(route: Route) -> Value {
    match route {
        Route::Capi => {
            let surface = SHARED.surface.load(Ordering::SeqCst);
            if surface.is_null() {
                return Value::Null;
            }
            // SAFETY: the live surface on the main thread; each text is freed by libghostty.
            unsafe {
                let size = sys::ghostty_surface_size(surface);
                let mut lines = Vec::new();
                for row in 0..u32::from(size.rows) {
                    let point = |x| sys::ghostty_point_s { tag: sys::GHOSTTY_POINT_VIEWPORT, coord: sys::GHOSTTY_POINT_COORD_EXACT, x, y: row };
                    let selection = sys::ghostty_selection_s {
                        top_left: point(0),
                        bottom_right: point(u32::from(size.columns).saturating_sub(1)),
                        rectangle: false,
                    };
                    let mut text: sys::ghostty_text_s = std::mem::zeroed();
                    if !sys::ghostty_surface_read_text(surface, selection, &mut text) {
                        lines.push(String::from("<unread>"));
                        continue;
                    }
                    let line = if text.text.is_null() || text.text_len == 0 {
                        String::new()
                    } else {
                        String::from_utf8_lossy(std::slice::from_raw_parts(text.text.cast::<u8>(), text.text_len)).into_owned()
                    };
                    sys::ghostty_surface_free_text(surface, &mut text);
                    lines.push(line);
                }
                json!({"grid": [size.columns, size.rows], "text": lines.join("\n")})
            }
        }
        Route::Swift => {
            #[cfg(spike_swift)]
            {
                let handle = SHARED.swift.load(Ordering::SeqCst);
                let mut buffer = vec![0u8; 256 * 1024];
                // SAFETY: the live Swift terminal and a buffer of the given capacity.
                let length = unsafe { sys::swift::mg_spike_read_text(handle, buffer.as_mut_ptr(), buffer.len() as isize) };
                return if length < 0 {
                    json!({"error": "no surface"})
                } else {
                    json!({"text": String::from_utf8_lossy(&buffer[..length as usize])})
                };
            }
            #[cfg(not(spike_swift))]
            Value::Null
        }
    }
}

unsafe fn teardown(route: Route) {
    match route {
        Route::Capi => {
            let surface = SHARED.surface.swap(std::ptr::null_mut(), Ordering::SeqCst);
            let app = SHARED.app.swap(std::ptr::null_mut(), Ordering::SeqCst);
            // SAFETY: freed once, surface before its app, on the main thread.
            unsafe {
                if !surface.is_null() {
                    sys::ghostty_surface_free(surface);
                }
                if !app.is_null() {
                    sys::ghostty_app_free(app);
                }
            }
        }
        Route::Swift => {
            #[cfg(spike_swift)]
            {
                let handle = SHARED.swift.swap(std::ptr::null_mut(), Ordering::SeqCst);
                if !handle.is_null() {
                    // SAFETY: freed once on the main thread.
                    unsafe { sys::swift::mg_spike_free(handle) };
                }
            }
        }
    }
}

// ── ticks and draws ───────────────────────────────────────────────────────────────────────────────────────────────────────────────

fn queue_tick() {
    if SHARED.tick_queued.swap(true, Ordering::SeqCst) {
        return;
    }
    if let Some(handle) = HANDLE.get() {
        let _ = handle.run_on_main_thread(|| {
            SHARED.tick_queued.store(false, Ordering::SeqCst);
            let app = SHARED.app.load(Ordering::SeqCst);
            if !app.is_null() {
                // SAFETY: the live app on the main thread.
                unsafe { sys::ghostty_app_tick(app) };
                SHARED.ticks.fetch_add(1, Ordering::Relaxed);
            }
        });
    }
}

/// One frame: a tick and a draw on the main thread (the vendored Swift view draws from its display link the same way).
fn queue_draw() {
    if SHARED.draw_queued.swap(true, Ordering::SeqCst) {
        return;
    }
    if let Some(handle) = HANDLE.get() {
        let _ = handle.run_on_main_thread(|| {
            SHARED.draw_queued.store(false, Ordering::SeqCst);
            let (app, surface) = (SHARED.app.load(Ordering::SeqCst), SHARED.surface.load(Ordering::SeqCst));
            if !app.is_null() && !surface.is_null() {
                // SAFETY: the live app and surface on the main thread.
                unsafe {
                    sys::ghostty_app_tick(app);
                    sys::ghostty_surface_draw(surface);
                }
                SHARED.draws.fetch_add(1, Ordering::Relaxed);
            }
        });
    }
}

// ── libghostty's callbacks ────────────────────────────────────────────────────────────────────────────────────────────────────────

unsafe extern "C" fn wakeup(_userdata: *mut c_void) {
    SHARED.wakeups.fetch_add(1, Ordering::Relaxed);
    queue_tick();
}

unsafe extern "C" fn action(_app: sys::ghostty_app_t, _target: sys::ghostty_target_s, action: sys::ghostty_action_s) -> bool {
    let name = usize::try_from(action.tag).ok().and_then(|tag| sys::ACTION_NAMES.get(tag)).copied().unwrap_or("UNKNOWN");
    *SHARED.actions.lock().unwrap().entry(name.to_string()).or_default() += 1;
    let text = || {
        let pointer = action.payload[0] as usize as *const c_char;
        // SAFETY: SET_TITLE's and PWD's payload is a NUL-terminated string Ghostty owns for the call.
        (!pointer.is_null()).then(|| unsafe { CStr::from_ptr(pointer) }.to_string_lossy().into_owned())
    };
    match action.tag {
        sys::ACTION_SET_TITLE => {
            if let Some(title) = text() {
                SHARED.titles.lock().unwrap().push(title);
            }
            true
        }
        sys::ACTION_PWD => {
            if let Some(pwd) = text() {
                SHARED.pwds.lock().unwrap().push(pwd);
            }
            true
        }
        sys::ACTION_CELL_SIZE => true,
        _ => false,
    }
}

unsafe extern "C" fn read_clipboard(_: *mut c_void, _: c_int, _: *mut c_void, _: *const *const c_char, _: usize, _: bool) -> c_int {
    sys::GHOSTTY_CLIPBOARD_READ_UNAVAILABLE
}

unsafe extern "C" fn confirm_read_clipboard(_: *mut c_void, _: *const c_void, _: *mut c_void, _: c_int) {}

unsafe extern "C" fn write_clipboard(_: *mut c_void, _: c_int, _: *const sys::ghostty_clipboard_content_s, _: usize, _: bool) {
    SHARED.clipboard_writes.fetch_add(1, Ordering::Relaxed);
}

unsafe extern "C" fn close_surface(_: *mut c_void, _: bool) {
    SHARED.closes.fetch_add(1, Ordering::Relaxed);
}

/// What the user typed, as Ghostty encoded it: to the PTY.
unsafe extern "C" fn receive_buffer(_userdata: *mut c_void, bytes: *const u8, len: usize) {
    if bytes.is_null() || len == 0 {
        return;
    }
    // SAFETY: Ghostty's buffer for the call.
    let bytes = unsafe { std::slice::from_raw_parts(bytes, len) };
    SHARED.input.lock().unwrap().extend_from_slice(bytes);
    let master = SHARED.master.load(Ordering::SeqCst);
    if master >= 0 {
        pty::write_all(master, bytes);
    }
}

/// The grid Ghostty settled on: to the PTY's window size.
unsafe extern "C" fn receive_resize(_userdata: *mut c_void, columns: u16, rows: u16, width: u32, height: u32) {
    let master = SHARED.master.load(Ordering::SeqCst);
    let applied = master >= 0 && pty::resize(master, columns, rows, width, height);
    SHARED.resizes.lock().unwrap().push(json!({"columns": columns, "rows": rows, "width": width, "height": height, "applied": applied}));
}

#[cfg(spike_swift)]
unsafe extern "C" fn swift_input(_context: *mut c_void, bytes: *const u8, len: isize) {
    // SAFETY: the same contract as `receive_buffer`.
    unsafe { receive_buffer(std::ptr::null_mut(), bytes, len.max(0) as usize) };
}

#[cfg(spike_swift)]
unsafe extern "C" fn swift_resize(_context: *mut c_void, columns: u16, rows: u16) {
    // SAFETY: the same contract as `receive_resize`.
    unsafe { receive_resize(std::ptr::null_mut(), columns, rows, 0, 0) };
}

// ── the verdict ───────────────────────────────────────────────────────────────────────────────────────────────────────────────────

/// `screencapture -l <window>`: the window as the system's own tool pictures it (it may need the Screen Recording permission, which a
/// runner may not have; reported either way).
fn screencapture(out: &Path, report: &Value) -> Value {
    let number = report["collected"]["windowNumber"].as_i64().unwrap_or(0);
    if number == 0 {
        return json!({"ok": false, "error": "no window number"});
    }
    let file = out.join("screencapture.png");
    match std::process::Command::new("/usr/sbin/screencapture").args(["-x", "-o", &format!("-l{number}")]).arg(&file).output() {
        Ok(output) => json!({"ok": output.status.success() && file.is_file(), "status": output.status.code(),
            "stderr": String::from_utf8_lossy(&output.stderr), "png": "screencapture.png"}),
        Err(error) => json!({"ok": false, "error": error.to_string()}),
    }
}

fn verdict(report: &mut Value) -> i32 {
    let viewport = report["collected"]["viewport"]["text"].as_str().unwrap_or("").to_string();
    let output = String::from_utf8_lossy(&SHARED.output.lock().unwrap()).into_owned();
    let parses = viewport.contains("spike-marker-42");
    let typed_in_grid = viewport.contains("typed-42");
    let typed_reached_shell = output.contains("typed-42");
    // The last `rows columns` line the shell printed (the typed `stty size`) against the last grid Ghostty reported.
    let stty = output
        .split(['\n', '\r'])
        .map(str::trim)
        .filter(|line| {
            let parts: Vec<&str> = line.split(' ').collect();
            parts.len() == 2 && parts.iter().all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
        })
        .next_back()
        .map(str::to_string);
    let last_resize = SHARED.resizes.lock().unwrap().last().cloned();
    let resize = match (&stty, &last_resize) {
        (Some(stty), Some(last)) => *stty == format!("{} {}", last["rows"], last["columns"]),
        _ => false,
    };
    let count = |picture: &str, colour: &str| report["collected"][picture]["counts"][colour].as_u64().unwrap_or(0);
    let window_ok = report["collected"]["window"]["ok"].as_bool().unwrap_or(false);
    let rendered_window = count("window", "red") > 200;
    let rendered_view = count("viewCache", "red") > 200 || count("ioSurface", "red") > 200;
    let page_drawn = count("window", "page") > 2000;
    let over_page = rendered_window && page_drawn;
    let works = parses && typed_reached_shell && typed_in_grid && resize && (rendered_window || rendered_view) && (over_page || !window_ok);
    report["verdict"] = json!({
        "parses": parses,
        "typedReachedShell": typed_reached_shell,
        "typedInGrid": typed_in_grid,
        "sttySize": stty,
        "lastResize": last_resize,
        "resizeReachedPty": resize,
        "renderedOnScreen": rendered_window,
        "renderedInViewOrSurface": rendered_view,
        "pageDrawn": page_drawn,
        "overThePage": over_page,
        "windowPictureTaken": window_ok,
        "works": works,
    });
    if works { 0 } else { 1 }
}
