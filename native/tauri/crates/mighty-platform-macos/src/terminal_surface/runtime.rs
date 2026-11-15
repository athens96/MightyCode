//! The one libghostty app of the process and what every surface shares with its callbacks (the vendored `TerminalController`,
//! `TerminalController+Callbacks.swift`, `TerminalSurfaceCoordinator.swift`, as the Step 0 spike drove them):
//! - the app, made on the main thread the first time a view opens, from Mighty's config ([`super::config`]), never the user's own
//!   Ghostty config; a theme or font change makes a new config and hands it to the app and every surface;
//! - frames: libghostty's wakeups (any thread) owe a frame; a pacer thread hands the main thread one tick and one draw of every shown
//!   surface at most every 16 ms while frames are owed (the Swift view's display link), and a tick every 100 ms after 30 idle frames,
//!   so a hidden terminal's titles, folder and bell still drain (Swift's 100 ms `controller.tick()`);
//! - the callbacks: what the user typed (`receive_buffer`) and the grid (`receive_resize`) to the view's listener, the title, the
//!   working folder and the bell (actions), the clipboard: a paste reads the system clipboard, a copy writes it, and what a program
//!   asks through OSC 52 is refused (`clipboard-write = ask`, answered as Swift's `LocalTerminalSession` answered it: only a paste is
//!   allowed).

use std::cell::RefCell;
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, Once, OnceLock};
use std::time::Duration;

use mighty_platform::{TerminalEvent, TerminalListener};
use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
use objc2_foundation::NSString;

use super::config;
use super::ghostty::*;

/// The app's handle, for the main thread (set by the first view's open).
pub static HANDLE: OnceLock<tauri::AppHandle> = OnceLock::new();
static APP: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
static OWED: AtomicBool = AtomicBool::new(false);
static FRAME_QUEUED: AtomicBool = AtomicBool::new(false);
static PACER: Once = Once::new();
static WAKE: (Mutex<()>, Condvar) = (Mutex::new(()), Condvar::new());
static CONFIGS: AtomicU64 = AtomicU64::new(0);

/// Runs `f` on the main thread, after everything queued there before it.
pub fn on_main(f: impl FnOnce() + Send + 'static) {
    if let Some(handle) = HANDLE.get() {
        let _ = handle.run_on_main_thread(f);
    }
}

/// A frame is owed (output, a resize, a focus change, libghostty's wakeup).
pub fn owe() {
    OWED.store(true, Ordering::SeqCst);
    WAKE.1.notify_one();
}

/// What the terminals look like (the app's theme and the text setting).
#[derive(Debug, Clone, PartialEq)]
pub struct Look {
    pub light: bool,
    pub family: Option<String>,
    pub size: f64,
}

impl Default for Look {
    fn default() -> Look {
        Look { light: false, family: None, size: 12.0 }
    }
}

/// What a surface shares with libghostty's callbacks and the host's threads.
pub struct Shared {
    listener: Option<TerminalListener>,
    /// The live surface; null before it is made and once freed (main thread only for anything but reading it).
    pub surface: AtomicPtr<c_void>,
    pub visible: AtomicBool,
    pending: Mutex<Vec<u8>>,
    feed_queued: AtomicBool,
}

impl Shared {
    pub fn new(listener: Option<TerminalListener>) -> Shared {
        Shared {
            listener,
            surface: AtomicPtr::new(std::ptr::null_mut()),
            visible: AtomicBool::new(true),
            pending: Mutex::new(Vec::new()),
            feed_queued: AtomicBool::new(false),
        }
    }

    pub fn surface(&self) -> Option<ghostty_surface_t> {
        let surface = self.surface.load(Ordering::SeqCst);
        (!surface.is_null()).then_some(surface)
    }

    pub fn send(&self, event: TerminalEvent) {
        if let Some(listener) = &self.listener {
            listener.send(event);
        }
    }

    /// The PTY's output, kept in order and written into the surface on the main thread (libghostty's mailbox drains only in the main
    /// thread's tick, so a write is never made where it could wait for that tick).
    pub fn feed(self: &Arc<Self>, bytes: &[u8]) {
        self.pending.lock().unwrap_or_else(|p| p.into_inner()).extend_from_slice(bytes);
        if self.feed_queued.swap(true, Ordering::SeqCst) {
            return;
        }
        let shared = self.clone();
        on_main(move || {
            shared.feed_queued.store(false, Ordering::SeqCst);
            let bytes = std::mem::take(&mut *shared.pending.lock().unwrap_or_else(|p| p.into_inner()));
            if let Some(surface) = shared.surface()
                && !bytes.is_empty()
            {
                // SAFETY: the live surface on the main thread; libghostty copies the bytes.
                unsafe { ghostty_surface_write_buffer(surface, bytes.as_ptr(), bytes.len()) };
                owe();
            }
        });
    }
}

/// The main thread's part: the app's config, the look it was made from, the live surfaces.
struct Main {
    config: ghostty_config_t,
    look: Look,
    surfaces: Vec<Arc<Shared>>,
}

thread_local! {
    static MAIN: RefCell<Option<Main>> = const { RefCell::new(None) };
}

/// A config made from `look`: written to a file of its own, loaded, finalized, the file removed.
fn make_config(look: &Look) -> Result<ghostty_config_t, String> {
    let file = std::env::temp_dir().join(format!("mighty-ghostty-{}-{}.conf", std::process::id(), CONFIGS.fetch_add(1, Ordering::Relaxed)));
    std::fs::write(&file, config::config_text(look.light, look.family.as_deref(), look.size)).map_err(|e| e.to_string())?;
    let path = CString::new(file.to_string_lossy().into_owned()).map_err(|e| e.to_string())?;
    // SAFETY: libghostty's config calls on the main thread with a NUL-terminated absolute path.
    let config = unsafe {
        let config = ghostty_config_new();
        if !config.is_null() {
            ghostty_config_load_file(config, path.as_ptr());
            ghostty_config_finalize(config);
        }
        config
    };
    let _ = std::fs::remove_file(&file);
    if config.is_null() {
        return Err("ghostty_config_new gave nothing".into());
    }
    Ok(config)
}

/// Config diagnostics (for an error that says why the app could not be made).
fn diagnostics(config: ghostty_config_t) -> Vec<String> {
    // SAFETY: a live config; each message is NUL-terminated and owned by it.
    unsafe {
        (0..ghostty_config_diagnostics_count(config))
            .filter_map(|index| {
                let message = ghostty_config_get_diagnostic(config, index).message;
                (!message.is_null()).then(|| CStr::from_ptr(message).to_string_lossy().into_owned())
            })
            .collect()
    }
}

/// The app, made the first time (main thread only).
pub fn app(look: &Look) -> Result<ghostty_app_t, String> {
    let existing = APP.load(Ordering::SeqCst);
    if !existing.is_null() {
        return Ok(existing);
    }
    // SAFETY: libghostty's documented order on the main thread: init once, config, app.
    let init = unsafe { ghostty_init(0, std::ptr::null_mut()) };
    if init != 0 {
        return Err(format!("ghostty_init failed ({init})"));
    }
    let config = make_config(look)?;
    let runtime = ghostty_runtime_config_s {
        userdata: std::ptr::null_mut(),
        supports_selection_clipboard: true,
        wakeup_cb: Some(wakeup),
        action_cb: Some(action),
        read_clipboard_cb: Some(read_clipboard),
        confirm_read_clipboard_cb: Some(confirm_read_clipboard),
        write_clipboard_cb: Some(write_clipboard),
        close_surface_cb: Some(close_surface),
    };
    // SAFETY: a finalized config and callbacks that live for the program.
    let app = unsafe { ghostty_app_new(&runtime, config) };
    if app.is_null() {
        let issues = diagnostics(config);
        // SAFETY: the config the app did not take.
        unsafe { ghostty_config_free(config) };
        return Err(format!("ghostty_app_new gave nothing ({issues:?})"));
    }
    // SAFETY: the live app on the main thread.
    unsafe { ghostty_app_set_focus(app, true) };
    APP.store(app, Ordering::SeqCst);
    MAIN.with(|main| *main.borrow_mut() = Some(Main { config, look: look.clone(), surfaces: Vec::new() }));
    PACER.call_once(|| {
        let _ = std::thread::Builder::new().name("ghostty-frames".into()).spawn(pace);
    });
    Ok(app)
}

/// The look the app has now (the default before the first view).
pub fn look() -> Look {
    MAIN.with(|main| main.borrow().as_ref().map(|m| m.look.clone())).unwrap_or_default()
}

/// Changes the look: a new config for the app and every surface (the old one freed after), and each surface's font size (the size a
/// surface was made with is its own: `set_font_size`, as Swift's sessions did).
pub fn set_look(change: impl FnOnce(&mut Look)) {
    let Some((surfaces, size, changed)) = MAIN.with(|main| {
        let mut main = main.borrow_mut();
        let main = main.as_mut()?;
        let mut look = main.look.clone();
        change(&mut look);
        if look == main.look {
            return Some((Vec::new(), look.size, None));
        }
        let config = make_config(&look).ok()?;
        let app = APP.load(Ordering::SeqCst);
        // SAFETY: the live app and surfaces on the main thread; the previous config is freed once nothing holds it.
        unsafe {
            ghostty_app_update_config(app, config);
            for surface in main.surfaces.iter().filter_map(|shared| shared.surface()) {
                ghostty_surface_update_config(surface, config);
            }
            ghostty_config_free(main.config);
        }
        let size_changed = look.size != main.look.size;
        main.config = config;
        main.look = look.clone();
        Some((main.surfaces.clone(), look.size, Some(size_changed)))
    }) else {
        return;
    };
    if changed == Some(true) {
        let action = format!("set_font_size:{}", size_literal(size));
        for surface in surfaces.iter().filter_map(|shared| shared.surface()) {
            // SAFETY: the live surface on the main thread; the action's text for the call.
            unsafe { ghostty_surface_binding_action(surface, action.as_ptr().cast(), action.len()) };
        }
    }
    owe();
}

fn size_literal(size: f64) -> String {
    let text = format!("{size:.2}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

pub fn register(shared: &Arc<Shared>) {
    MAIN.with(|main| {
        if let Some(main) = main.borrow_mut().as_mut() {
            main.surfaces.push(shared.clone());
        }
    });
}

pub fn unregister(shared: &Arc<Shared>) {
    MAIN.with(|main| {
        if let Some(main) = main.borrow_mut().as_mut() {
            main.surfaces.retain(|each| !Arc::ptr_eq(each, shared));
        }
    });
}

/// One frame on the main thread: the app's tick (its mailbox: titles, folders, the bell) and, when `draw`, a draw of every shown
/// surface.
fn frame(draw: bool) {
    FRAME_QUEUED.store(false, Ordering::SeqCst);
    let app = APP.load(Ordering::SeqCst);
    if app.is_null() {
        return;
    }
    // SAFETY: the live app on the main thread.
    unsafe { ghostty_app_tick(app) };
    if !draw {
        return;
    }
    let surfaces = MAIN.with(|main| main.borrow().as_ref().map(|m| m.surfaces.clone()).unwrap_or_default());
    for shared in surfaces.iter().filter(|shared| shared.visible.load(Ordering::SeqCst)) {
        if let Some(surface) = shared.surface() {
            // SAFETY: the live surface on the main thread.
            unsafe {
                ghostty_surface_refresh(surface);
                ghostty_surface_draw(surface);
            }
        }
    }
}

/// The pacer (see the module comment).
fn pace() {
    let mut idle = 0u32;
    loop {
        let wait = if idle < 30 { Duration::from_millis(16) } else { Duration::from_millis(100) };
        {
            let guard = WAKE.0.lock().unwrap_or_else(|p| p.into_inner());
            // A wakeup that lands between the look and the wait is late by one wait at most.
            let _ = WAKE.1.wait_timeout(guard, wait);
        }
        // The main thread has not run the last frame yet: what is owed stays owed for the next one.
        if FRAME_QUEUED.load(Ordering::SeqCst) {
            continue;
        }
        let draw = OWED.swap(false, Ordering::SeqCst);
        idle = if draw { 0 } else { idle.saturating_add(1) };
        if draw || idle >= 30 {
            FRAME_QUEUED.store(true, Ordering::SeqCst);
            on_main(move || frame(draw));
        }
    }
}

// ── libghostty's callbacks ──────────────────────────────────────────────────────────────────────────────────────────────────────

/// The `Shared` a surface was made with (its userdata), alive until the surface is freed.
unsafe fn shared_of<'a>(userdata: *mut c_void) -> Option<&'a Shared> {
    // SAFETY: the pointer `Arc::into_raw` gave at the surface's making, released only after `ghostty_surface_free`.
    (!userdata.is_null()).then(|| unsafe { &*(userdata as *const Shared) })
}

unsafe extern "C" fn wakeup(_userdata: *mut c_void) {
    owe();
}

/// A NUL-terminated string an action carries for the call.
unsafe fn action_text(pointer: u64) -> Option<String> {
    let pointer = pointer as usize as *const c_char;
    // SAFETY: SET_TITLE's and PWD's payload is a NUL-terminated string libghostty owns for the call.
    (!pointer.is_null()).then(|| unsafe { CStr::from_ptr(pointer) }.to_string_lossy().into_owned())
}

unsafe extern "C" fn action(_app: ghostty_app_t, target: ghostty_target_s, action: ghostty_action_s) -> bool {
    if target.tag != GHOSTTY_TARGET_SURFACE || target.surface.is_null() {
        return false;
    }
    // SAFETY: the target surface is live for the call; its userdata is our `Shared`.
    let Some(shared) = (unsafe { shared_of(ghostty_surface_userdata(target.surface)) }) else { return false };
    match action.tag {
        ACTION_SET_TITLE => {
            // SAFETY: the action's payload for the call.
            if let Some(title) = unsafe { action_text(action.payload[0]) } {
                shared.send(TerminalEvent::Title(title));
            }
            true
        }
        ACTION_PWD => {
            // SAFETY: the action's payload for the call.
            if let Some(folder) = unsafe { action_text(action.payload[0]) } {
                shared.send(TerminalEvent::WorkingDirectory(folder));
            }
            true
        }
        ACTION_RING_BELL => {
            shared.send(TerminalEvent::Bell);
            true
        }
        ACTION_RENDER => {
            owe();
            true
        }
        ACTION_CELL_SIZE => true,
        _ => false,
    }
}

/// The system clipboard's text (main thread).
fn clipboard_text() -> Option<String> {
    // SAFETY: the general pasteboard's string; `NSPasteboardTypeString` is AppKit's constant.
    let text = unsafe { NSPasteboard::generalPasteboard().stringForType(NSPasteboardTypeString) }?;
    Some(text.to_string()).filter(|text| !text.is_empty())
}

/// Writes `text` to the system clipboard (main thread).
pub fn write_clipboard_text(text: &str) -> bool {
    let pasteboard = NSPasteboard::generalPasteboard();
    pasteboard.clearContents();
    // SAFETY: `NSPasteboardTypeString` is AppKit's constant.
    pasteboard.setString_forType(&NSString::from_str(text), unsafe { NSPasteboardTypeString })
}

/// Completes a clipboard read with `text` as `text/plain`.
unsafe fn complete(surface: ghostty_surface_t, state: *mut c_void, text: &str, confirmed: bool) {
    let mime = c"text/plain";
    let content = ghostty_clipboard_content_s { mime: mime.as_ptr(), data: text.as_ptr().cast(), len: text.len() };
    let available = [mime.as_ptr()];
    let complete = ghostty_clipboard_complete_s {
        contents: &content,
        contents_len: 1,
        available: available.as_ptr(),
        available_len: 1,
        confirmed,
        remember: false,
    };
    // SAFETY: the live surface and its pending request; libghostty borrows the payload for the call.
    unsafe { ghostty_surface_complete_clipboard_request(surface, &complete, state) };
}

/// A paste (or a program's read, which libghostty then asks to confirm): the clipboard's text.
unsafe extern "C" fn read_clipboard(
    userdata: *mut c_void,
    _clipboard: c_int,
    state: *mut c_void,
    _mimes: *const *const c_char,
    _mimes_len: usize,
    _list_available: bool,
) -> c_int {
    // SAFETY: the surface's userdata.
    let Some(surface) = (unsafe { shared_of(userdata) }).and_then(Shared::surface) else { return GHOSTTY_CLIPBOARD_READ_UNSUPPORTED };
    if state.is_null() {
        return GHOSTTY_CLIPBOARD_READ_UNSUPPORTED;
    }
    let Some(text) = clipboard_text() else { return GHOSTTY_CLIPBOARD_READ_UNAVAILABLE };
    // SAFETY: the live surface and the request's state.
    unsafe { complete(surface, state, &text, false) };
    GHOSTTY_CLIPBOARD_READ_STARTED
}

/// libghostty asks whether a read may reach the program: a paste may, anything else (OSC 52, kitty's protocol) is refused.
unsafe extern "C" fn confirm_read_clipboard(
    userdata: *mut c_void,
    confirm: *const ghostty_clipboard_confirm_s,
    state: *mut c_void,
    request: c_int,
) {
    // SAFETY: the surface's userdata.
    let Some(surface) = (unsafe { shared_of(userdata) }).and_then(Shared::surface) else { return };
    if state.is_null() {
        return;
    }
    if request != GHOSTTY_CLIPBOARD_REQUEST_PASTE || confirm.is_null() {
        // SAFETY: the live surface and its pending request.
        unsafe { ghostty_surface_deny_clipboard_request(surface, state) };
        return;
    }
    // SAFETY: the confirmation's contents, borrowed for the call: copied out before anything else.
    let text = unsafe {
        let confirm = &*confirm;
        (0..confirm.contents_len)
            .map(|index| &*confirm.contents.add(index))
            .find(|content| !content.data.is_null())
            .map(|content| String::from_utf8_lossy(std::slice::from_raw_parts(content.data.cast::<u8>(), content.len)).into_owned())
            .unwrap_or_default()
    };
    // SAFETY: the live surface and its pending request.
    unsafe { complete(surface, state, &text, true) };
}

/// A copy binding (written at once) or a program's OSC 52 write (`clipboard-write = ask`: refused, as Swift answered it).
unsafe extern "C" fn write_clipboard(
    _userdata: *mut c_void,
    clipboard: c_int,
    contents: *const ghostty_clipboard_content_s,
    count: usize,
    confirm: bool,
) {
    if clipboard != GHOSTTY_CLIPBOARD_STANDARD || confirm || contents.is_null() {
        return;
    }
    // SAFETY: the contents, binary-safe with their lengths, borrowed for the call.
    let text = unsafe {
        (0..count)
            .map(|index| &*contents.add(index))
            .find(|content| {
                !content.mime.is_null() && CStr::from_ptr(content.mime).to_bytes().starts_with(b"text/plain") && !content.data.is_null()
            })
            .map(|content| String::from_utf8_lossy(std::slice::from_raw_parts(content.data.cast::<u8>(), content.len)).into_owned())
    };
    if let Some(text) = text {
        on_main(move || {
            write_clipboard_text(&text);
        });
    }
}

/// The host owns the PTY: a surface asking to close itself changes nothing (the pane's exit notice and restart are the host's).
unsafe extern "C" fn close_surface(_userdata: *mut c_void, _process_alive: bool) {}

/// What the user typed, encoded by the terminal: to the listener (the host writes it to the PTY).
pub unsafe extern "C" fn receive_buffer(userdata: *mut c_void, bytes: *const u8, len: usize) {
    if bytes.is_null() || len == 0 {
        return;
    }
    // SAFETY: the surface's receive userdata and libghostty's buffer for the call.
    if let Some(shared) = unsafe { shared_of(userdata) } {
        shared.send(TerminalEvent::Input(unsafe { std::slice::from_raw_parts(bytes, len) }.to_vec()));
    }
}

/// The grid the surface settled on: to the listener (the host's PTY takes it).
pub unsafe extern "C" fn receive_resize(userdata: *mut c_void, columns: u16, rows: u16, width: u32, height: u32) {
    // SAFETY: the surface's receive userdata.
    if let Some(shared) = unsafe { shared_of(userdata) } {
        shared.send(TerminalEvent::Resized { columns, rows, width, height });
    }
}
