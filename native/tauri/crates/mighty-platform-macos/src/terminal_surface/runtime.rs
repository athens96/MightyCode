//! The one libghostty app of the process and what every surface shares with its callbacks (the vendored `TerminalController`,
//! `TerminalController+Callbacks.swift`, `TerminalSurfaceCoordinator.swift`, `InMemoryTerminalSurfaceAccess.swift`, as the Step 0
//! spike drove them):
//! - the app, made on the main thread the first time a view opens, from Mighty's config ([`super::config`]), never the user's own
//!   Ghostty config; a theme or font change makes a new config and hands it to the app and every surface. An app that could not be
//!   made is not tried again: the error is kept.
//! - the PTY's output goes into a surface from a writer thread of its own, through a bounded queue (Swift's serial write queue), never
//!   from the main thread: a write can wait for libghostty's mailbox, which only the main thread's `ghostty_app_tick` drains. A full
//!   queue makes the feeder wait (backpressure). Closing a surface stops its writer and waits for it while the main thread keeps
//!   ticking, then frees the surface.
//! - frames: libghostty's wakeups (any thread) owe a frame; while any surface lives, a pacer thread queues one tick and one draw of
//!   every shown surface on the main thread at most every 16 ms while frames are owed (the Swift view's display link), and a tick
//!   every 100 ms after 30 idle frames, so a hidden terminal's titles, folder and bell still drain (Swift's 100 ms
//!   `controller.tick()`); the pacer ends with the last surface. The app's focus follows the application being active.
//! - the callbacks ([`super::clipboard`]'s rules): what the user typed (`receive_buffer`) and the grid (`receive_resize`) to the view's
//!   listener; the title, the working folder, the bell and the mouse's shape (actions); a paste reads the system clipboard, a copy
//!   binding writes it, and what a program asks through OSC 52 is refused. A panic in a callback never crosses into libghostty.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicPtr, AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, Once, OnceLock};
use std::time::{Duration, Instant};

use mighty_platform::{TerminalEvent, TerminalListener};
use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSPasteboard, NSPasteboardTypeString};
use objc2_foundation::NSString;

use super::clipboard;
use super::config;
use super::ghostty::*;

static APP: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
static APP_FAILED: OnceLock<String> = OnceLock::new();
static INIT: Once = Once::new();
static OWED: AtomicBool = AtomicBool::new(false);
static FRAME_QUEUED: AtomicBool = AtomicBool::new(false);
static PACING: AtomicBool = AtomicBool::new(false);
static SURFACES: AtomicUsize = AtomicUsize::new(0);
static WAKE: (Mutex<()>, Condvar) = (Mutex::new(()), Condvar::new());
static CONFIGS: AtomicU64 = AtomicU64::new(0);
/// The application's active state the app's focus last followed (0 unknown, 1 inactive, 2 active).
static ACTIVE: AtomicU8 = AtomicU8::new(0);
/// Clipboard writes that landed (the headless test counts them).
pub static CLIPBOARD_WRITES: AtomicU64 = AtomicU64::new(0);

/// The most PTY output waiting for a surface's writer before the feeder waits (bytes).
pub const WRITE_QUEUE_LIMIT: usize = 4 << 20;
/// How long a close waits for its writer, ticking meanwhile, before it leaves the surface unfreed rather than free it under a write.
const CLOSE_WAIT: Duration = Duration::from_secs(5);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[link(name = "System", kind = "dylib")]
unsafe extern "C" {
    static _dispatch_main_q: u8;
    fn dispatch_async_f(queue: *const c_void, context: *mut c_void, work: unsafe extern "C" fn(*mut c_void));
}

unsafe extern "C" fn run_boxed(context: *mut c_void) {
    // SAFETY: the box `on_main` leaked for this one call.
    let work = unsafe { Box::from_raw(context as *mut Box<dyn FnOnce() + Send>) };
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work));
}

/// Queues `f` on the main thread, after everything queued there before it, even when called from the main thread (never inline).
pub fn on_main(f: impl FnOnce() + Send + 'static) {
    let work: Box<Box<dyn FnOnce() + Send>> = Box::new(Box::new(f));
    // SAFETY: the main dispatch queue (serviced by the main run loop) and a context `run_boxed` takes back once.
    unsafe { dispatch_async_f((&raw const _dispatch_main_q).cast(), Box::into_raw(work).cast(), run_boxed) };
}

/// A frame is owed (output, a resize, a focus change, libghostty's wakeup).
pub fn owe() {
    OWED.store(true, Ordering::SeqCst);
    WAKE.1.notify_all();
}

/// Runs a callback's body, a panic answering `fallback` instead of unwinding into libghostty or AppKit (the C callbacks here and the
/// view's Objective-C methods, review M-D 16).
pub(super) fn guarded<R>(fallback: R, body: impl FnOnce() -> R) -> R {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(body)).unwrap_or(fallback)
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

#[derive(Default)]
struct Queue {
    chunks: VecDeque<Vec<u8>>,
    bytes: usize,
    closing: bool,
}

/// What a surface shares with libghostty's callbacks, its writer and the host's threads.
pub struct Shared {
    listener: Option<TerminalListener>,
    /// The live surface for drawing, input and writes; null before it is made and from the moment its close begins.
    pub surface: AtomicPtr<c_void>,
    /// The surface for answering libghostty's requests: null only once it is freed.
    raw: AtomicPtr<c_void>,
    pub visible: AtomicBool,
    /// The mouse's shape Ghostty asked for (`ghostty_action_mouse_shape_e`).
    pub mouse_shape: AtomicI32,
    /// The view the surface draws in (set by the view; it outlives the surface; main thread only).
    pub view: AtomicPtr<c_void>,
    queue: Mutex<Queue>,
    changed: Condvar,
    writer: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl Shared {
    pub fn new(listener: Option<TerminalListener>) -> Shared {
        Shared {
            listener,
            surface: AtomicPtr::new(std::ptr::null_mut()),
            raw: AtomicPtr::new(std::ptr::null_mut()),
            visible: AtomicBool::new(true),
            mouse_shape: AtomicI32::new(GHOSTTY_MOUSE_SHAPE_TEXT),
            view: AtomicPtr::new(std::ptr::null_mut()),
            queue: Mutex::new(Queue::default()),
            changed: Condvar::new(),
            writer: Mutex::new(None),
        }
    }

    pub fn surface(&self) -> Option<ghostty_surface_t> {
        let surface = self.surface.load(Ordering::SeqCst);
        (!surface.is_null()).then_some(surface)
    }

    fn raw(&self) -> Option<ghostty_surface_t> {
        let surface = self.raw.load(Ordering::SeqCst);
        (!surface.is_null()).then_some(surface)
    }

    pub fn send(&self, event: TerminalEvent) {
        if let Some(listener) = &self.listener {
            listener.send(event);
        }
    }

    /// The surface is made: it takes writes from its own writer thread from now on.
    pub fn attach(self: &Arc<Self>, surface: ghostty_surface_t) -> std::io::Result<()> {
        self.raw.store(surface, Ordering::SeqCst);
        self.surface.store(surface, Ordering::SeqCst);
        let shared = self.clone();
        let writer = std::thread::Builder::new().name("ghostty-writer".into()).spawn(move || shared.write_loop())?;
        *lock(&self.writer) = Some(writer);
        Ok(())
    }

    /// The writer: each chunk in order into the live surface, until the close.
    fn write_loop(&self) {
        loop {
            let chunk = {
                let mut queue = lock(&self.queue);
                while queue.chunks.is_empty() && !queue.closing {
                    queue = self.changed.wait(queue).unwrap_or_else(|p| p.into_inner());
                }
                if queue.closing {
                    return;
                }
                let chunk = queue.chunks.pop_front().unwrap_or_default();
                queue.bytes -= chunk.len();
                self.changed.notify_all();
                chunk
            };
            let Some(surface) = self.surface() else { return };
            // SAFETY: the surface is freed only after this thread has ended (`close`); libghostty copies the bytes. A write may wait
            // for the main thread's tick, which never waits for this thread except while it keeps ticking.
            unsafe { ghostty_surface_write_buffer(surface, chunk.as_ptr(), chunk.len()) };
            owe();
        }
    }

    /// The PTY's output, in order: queued for the writer, the caller waiting while [`WRITE_QUEUE_LIMIT`] bytes already wait (a chunk
    /// larger than that goes alone). Nothing is taken once the close began.
    pub fn feed(&self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let mut queue = lock(&self.queue);
        while !queue.closing && queue.bytes > 0 && queue.bytes + bytes.len() > WRITE_QUEUE_LIMIT {
            queue = self.changed.wait(queue).unwrap_or_else(|p| p.into_inner());
        }
        if queue.closing {
            return;
        }
        queue.bytes += bytes.len();
        queue.chunks.push_back(bytes.to_vec());
        self.changed.notify_all();
    }

    /// Ends the surface (main thread): no more writes, the writer waited for while `app` keeps ticking (its write may wait for the
    /// mailbox), then the surface freed. False when the writer did not end in time: the surface is then left unfreed (never freed
    /// under a write), and the caller must keep `TerminalView` and `userdata` alive alongside it so libghostty pointers never dangle.
    pub fn close(&self, app: ghostty_app_t) -> bool {
        let Some(surface) = self.raw() else { return true };
        self.surface.store(std::ptr::null_mut(), Ordering::SeqCst);
        {
            let mut queue = lock(&self.queue);
            queue.closing = true;
            queue.chunks.clear();
            queue.bytes = 0;
            self.changed.notify_all();
        }
        let writer = lock(&self.writer).take();
        if let Some(writer) = writer {
            let deadline = Instant::now() + CLOSE_WAIT;
            while !writer.is_finished() {
                if Instant::now() > deadline {
                    return false;
                }
                if !app.is_null() {
                    // SAFETY: the live app on the main thread.
                    unsafe { ghostty_app_tick(app) };
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            let _ = writer.join();
        }
        // SAFETY: the live surface and app on the main thread: the mailbox drained, then the surface freed once.
        unsafe {
            if !app.is_null() {
                ghostty_app_tick(app);
            }
            ghostty_surface_set_focus(surface, false);
            ghostty_surface_free(surface);
        }
        self.raw.store(std::ptr::null_mut(), Ordering::SeqCst);
        true
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

/// The folder the config files are written in: the user's own temporary folder (`$TMPDIR`, else the user's caches), a subfolder only
/// this user may enter.
fn config_folder() -> Result<std::path::PathBuf, String> {
    use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _};
    let base =
        std::env::var_os("TMPDIR").map(std::path::PathBuf::from).or_else(dirs::cache_dir).ok_or("no folder for the terminal's config")?;
    // SAFETY: getuid has no preconditions.
    let uid = unsafe { libc::getuid() };
    let folder = base.join(format!("mighty-ghostty-{uid}"));
    match std::fs::DirBuilder::new().mode(0o700).create(&folder) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.to_string()),
    }
    let meta = std::fs::symlink_metadata(&folder).map_err(|e| e.to_string())?;
    if !meta.is_dir() || meta.uid() != uid || meta.mode() & 0o077 != 0 {
        return Err("the terminal's config folder is not this user's own".into());
    }
    Ok(folder)
}

/// A config made from `look`: written to a new file of its own (created exclusively, readable by this user only), loaded, finalized,
/// the file removed.
fn make_config(look: &Look) -> Result<ghostty_config_t, String> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
    let name = format!("{}-{}-{nanos:09}.conf", std::process::id(), CONFIGS.fetch_add(1, Ordering::Relaxed));
    let file = config_folder()?.join(name);
    let mut written = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&file).map_err(|e| e.to_string())?;
    let result = written.write_all(config::config_text(look.light, look.family.as_deref(), look.size).as_bytes());
    drop(written);
    if let Err(error) = result {
        let _ = std::fs::remove_file(&file);
        return Err(error.to_string());
    }
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

/// The app, made the first time (main thread only); an app that could not be made gives the same error from then on.
pub fn app(look: &Look) -> Result<ghostty_app_t, String> {
    let existing = APP.load(Ordering::SeqCst);
    if !existing.is_null() {
        return Ok(existing);
    }
    if let Some(error) = APP_FAILED.get() {
        return Err(error.clone());
    }
    let made = make_app(look);
    if let Err(error) = &made {
        let _ = APP_FAILED.set(error.clone());
    }
    made
}

fn make_app(look: &Look) -> Result<ghostty_app_t, String> {
    let mut init = 0;
    INIT.call_once(|| {
        // An empty argument list (libghostty takes a pointer it may read, never null).
        let mut argv: [*mut c_char; 1] = [std::ptr::null_mut()];
        // SAFETY: libghostty's documented order on the main thread: init once, config, app.
        init = unsafe { ghostty_init(0, argv.as_mut_ptr()) };
    });
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
    APP.store(app, Ordering::SeqCst);
    MAIN.with(|main| *main.borrow_mut() = Some(Main { config, look: look.clone(), surfaces: Vec::new() }));
    follow_activity(app);
    Ok(app)
}

/// The app's focus follows the application being active (main thread).
fn follow_activity(app: ghostty_app_t) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let active = NSApplication::sharedApplication(mtm).isActive();
    let state = if active { 2 } else { 1 };
    if ACTIVE.swap(state, Ordering::SeqCst) != state {
        // SAFETY: the live app on the main thread.
        unsafe { ghostty_app_set_focus(app, active) };
    }
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

/// A surface lives (main thread): it is drawn, and the pacer runs while any does.
pub fn register(shared: &Arc<Shared>) {
    MAIN.with(|main| {
        if let Some(main) = main.borrow_mut().as_mut() {
            main.surfaces.push(shared.clone());
        }
    });
    SURFACES.fetch_add(1, Ordering::SeqCst);
    start_pacer();
}

pub fn unregister(shared: &Arc<Shared>) {
    let removed = MAIN.with(|main| {
        main.borrow_mut().as_mut().is_some_and(|main| {
            let before = main.surfaces.len();
            main.surfaces.retain(|each| !Arc::ptr_eq(each, shared));
            main.surfaces.len() < before
        })
    });
    if removed {
        SURFACES.fetch_sub(1, Ordering::SeqCst);
        WAKE.1.notify_all();
    }
}

/// The app, for a close (null before it is made).
pub fn current_app() -> ghostty_app_t {
    APP.load(Ordering::SeqCst)
}

fn start_pacer() {
    if !PACING.swap(true, Ordering::SeqCst) {
        let spawned = std::thread::Builder::new().name("ghostty-frames".into()).spawn(pace);
        if spawned.is_err() {
            PACING.store(false, Ordering::SeqCst);
        }
    }
}

/// One frame on the main thread: the app's tick (its mailbox: titles, folders, the bell), its focus, and, when `draw`, a draw of every
/// shown surface.
fn frame(draw: bool) {
    FRAME_QUEUED.store(false, Ordering::SeqCst);
    let app = APP.load(Ordering::SeqCst);
    if app.is_null() {
        return;
    }
    follow_activity(app);
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

/// The pacer (see the module comment): ends once no surface lives.
fn pace() {
    let mut idle = 0u32;
    loop {
        if SURFACES.load(Ordering::SeqCst) == 0 {
            PACING.store(false, Ordering::SeqCst);
            // A surface registered between the look and the store: keep pacing for it.
            if SURFACES.load(Ordering::SeqCst) == 0 || PACING.swap(true, Ordering::SeqCst) {
                return;
            }
        }
        let wait = if idle < 30 { Duration::from_millis(16) } else { Duration::from_millis(100) };
        {
            let guard = lock(&WAKE.0);
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
    guarded(false, || {
        if target.tag != GHOSTTY_TARGET_SURFACE || target.surface.is_null() {
            return false;
        }
        // SAFETY: the target surface is live for the call; its userdata is our `Shared`.
        let userdata = unsafe { ghostty_surface_userdata(target.surface) };
        // SAFETY: as above.
        let Some(shared) = (unsafe { shared_of(userdata) }) else { return false };
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
            ACTION_DESKTOP_NOTIFICATION => {
                // `ghostty_action_desktop_notification_s { const char* title; const char* body; }`, both owned by libghostty for the
                // call (MC-26: the host marks the pane and decides on the OS notification).
                // SAFETY: the action's payload for the call.
                let (title, body) = unsafe { (action_text(action.payload[0]), action_text(action.payload[1])) };
                shared.send(TerminalEvent::Notification { title: title.unwrap_or_default(), body: body.unwrap_or_default() });
                true
            }
            ACTION_MOUSE_SHAPE => {
                // The payload's first member is the shape (a C enum).
                shared.mouse_shape.store(action.payload[0] as u32 as i32, Ordering::SeqCst);
                // SAFETY: the userdata is an `Arc<Shared>` libghostty holds; one more owner for the main thread's turn.
                let owned = unsafe {
                    Arc::increment_strong_count(userdata as *const Shared);
                    Arc::from_raw(userdata as *const Shared)
                };
                on_main(move || super::view::mouse_shape_changed(&owned));
                true
            }
            ACTION_RENDER => {
                owe();
                true
            }
            ACTION_CELL_SIZE => true,
            _ => false,
        }
    })
}

/// The system clipboard's text.
fn clipboard_text() -> Option<String> {
    // SAFETY: the general pasteboard's string; `NSPasteboardTypeString` is AppKit's constant.
    let text = unsafe { NSPasteboard::generalPasteboard().stringForType(NSPasteboardTypeString) }?;
    Some(text.to_string()).filter(|text| !text.is_empty())
}

/// Writes `text` to the system clipboard (main thread).
pub fn write_clipboard_text(text: &str) -> bool {
    let pasteboard = NSPasteboard::generalPasteboard();
    pasteboard.clearContents();
    CLIPBOARD_WRITES.fetch_add(1, Ordering::SeqCst);
    // SAFETY: `NSPasteboardTypeString` is AppKit's constant.
    pasteboard.setString_forType(&NSString::from_str(text), unsafe { NSPasteboardTypeString })
}

/// Completes a clipboard read: `text` as `text/plain` (none for a list request) and the kinds there are.
unsafe fn complete(surface: ghostty_surface_t, state: *mut c_void, text: Option<&str>, available: &[&str], confirmed: bool) {
    let mime = c"text/plain";
    let content = text.map(|text| ghostty_clipboard_content_s { mime: mime.as_ptr(), data: text.as_ptr().cast(), len: text.len() });
    let names: Vec<CString> = available.iter().filter_map(|kind| CString::new(*kind).ok()).collect();
    let pointers: Vec<*const c_char> = names.iter().map(|name| name.as_ptr()).collect();
    let complete = ghostty_clipboard_complete_s {
        contents: content.as_ref().map_or(std::ptr::null(), |content| content as *const _),
        contents_len: usize::from(content.is_some()),
        available: if pointers.is_empty() { std::ptr::null() } else { pointers.as_ptr() },
        available_len: pointers.len(),
        confirmed,
        remember: false,
    };
    // SAFETY: the live surface and its pending request; libghostty borrows the payload for the call.
    unsafe { ghostty_surface_complete_clipboard_request(surface, &complete, state) };
}

/// The MIME types a read asked for.
unsafe fn mime_list(mimes: *const *const c_char, count: usize) -> Vec<String> {
    if mimes.is_null() {
        return Vec::new();
    }
    // SAFETY: `count` pointers, each NUL-terminated or null, borrowed for the call.
    (0..count)
        .filter_map(|index| {
            let mime = unsafe { *mimes.add(index) };
            (!mime.is_null()).then(|| unsafe { CStr::from_ptr(mime) }.to_string_lossy().into_owned())
        })
        .collect()
}

/// A paste (or a program's read, which libghostty then asks to confirm), as [`clipboard::read`] decides.
unsafe extern "C" fn read_clipboard(
    userdata: *mut c_void,
    _clipboard: c_int,
    state: *mut c_void,
    mimes: *const *const c_char,
    mimes_len: usize,
    list_available: bool,
) -> c_int {
    guarded(GHOSTTY_CLIPBOARD_READ_UNSUPPORTED, || {
        // SAFETY: the surface's userdata.
        let Some(surface) = (unsafe { shared_of(userdata) }).and_then(Shared::raw) else { return GHOSTTY_CLIPBOARD_READ_UNSUPPORTED };
        if state.is_null() {
            return GHOSTTY_CLIPBOARD_READ_UNSUPPORTED;
        }
        // SAFETY: the request's MIME list, borrowed for the call.
        let asked = unsafe { mime_list(mimes, mimes_len) };
        let list_only = asked.is_empty() && list_available;
        let text = if list_only { clipboard_text().map(|_| String::new()) } else { clipboard_text() };
        match clipboard::read(&asked, list_available, text.is_some()) {
            clipboard::Read::Unsupported => GHOSTTY_CLIPBOARD_READ_UNSUPPORTED,
            clipboard::Read::Unavailable => GHOSTTY_CLIPBOARD_READ_UNAVAILABLE,
            clipboard::Read::List { available } => {
                // SAFETY: the live surface and the request's state.
                unsafe { complete(surface, state, None, &available, false) };
                GHOSTTY_CLIPBOARD_READ_STARTED
            }
            clipboard::Read::Text { available } => {
                // SAFETY: as above.
                unsafe { complete(surface, state, text.as_deref(), &available, false) };
                GHOSTTY_CLIPBOARD_READ_STARTED
            }
        }
    })
}

/// The first text-like (else the first) representation's bytes, copied out (a null data pointer is empty, whatever its length).
unsafe fn first_text(contents: *const ghostty_clipboard_content_s, count: usize) -> Option<String> {
    if contents.is_null() || count == 0 {
        return None;
    }
    // SAFETY: `count` representations borrowed for the call, each with its own length.
    let items: Vec<&ghostty_clipboard_content_s> = (0..count).map(|index| unsafe { &*contents.add(index) }).collect();
    let is_text = |item: &&ghostty_clipboard_content_s| {
        !item.mime.is_null() && clipboard::is_text_mime(&unsafe { CStr::from_ptr(item.mime) }.to_string_lossy())
    };
    let chosen = items.iter().copied().find(is_text).or(items.first().copied())?;
    if chosen.data.is_null() || chosen.len == 0 {
        return Some(String::new());
    }
    // SAFETY: as above.
    Some(String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(chosen.data.cast::<u8>(), chosen.len) }).into_owned())
}

/// libghostty asks whether a read may reach the program: [`clipboard::allow_confirm`] (a paste), else it is denied. Every request is
/// answered while the surface lives.
unsafe extern "C" fn confirm_read_clipboard(
    userdata: *mut c_void,
    confirm: *const ghostty_clipboard_confirm_s,
    state: *mut c_void,
    request: c_int,
) {
    guarded((), || {
        // SAFETY: the surface's userdata.
        let Some(surface) = (unsafe { shared_of(userdata) }).and_then(Shared::raw) else { return };
        if state.is_null() {
            return;
        }
        if !clipboard::allow_confirm(request) || confirm.is_null() {
            // SAFETY: the live surface and its pending request.
            unsafe { ghostty_surface_deny_clipboard_request(surface, state) };
            return;
        }
        // SAFETY: the confirmation's contents, borrowed for the call: copied out before anything else.
        let text = unsafe { first_text((*confirm).contents, (*confirm).contents_len) }.unwrap_or_default();
        // SAFETY: the live surface and its pending request.
        unsafe { complete(surface, state, Some(&text), &["text/plain"], true) };
    })
}

/// A copy binding (written) or a program's OSC 52 write (refused): [`clipboard::accept_write`].
unsafe extern "C" fn write_clipboard(
    _userdata: *mut c_void,
    clipboard_kind: c_int,
    contents: *const ghostty_clipboard_content_s,
    count: usize,
    confirm: bool,
) {
    guarded((), || {
        if !clipboard::accept_write(clipboard_kind, confirm) {
            return;
        }
        // SAFETY: the contents, binary-safe with their lengths, borrowed for the call.
        if let Some(text) = unsafe { first_text(contents, count) } {
            on_main(move || {
                write_clipboard_text(&text);
            });
        }
    })
}

/// The host owns the PTY: a surface asking to close itself changes nothing (the pane's exit notice and restart are the host's).
unsafe extern "C" fn close_surface(_userdata: *mut c_void, _process_alive: bool) {}

/// What the user typed, encoded by the terminal: to the listener (the host writes it to the PTY).
pub unsafe extern "C" fn receive_buffer(userdata: *mut c_void, bytes: *const u8, len: usize) {
    guarded((), || {
        if bytes.is_null() || len == 0 {
            return;
        }
        // SAFETY: the surface's receive userdata and libghostty's buffer for the call.
        if let Some(shared) = unsafe { shared_of(userdata) } {
            shared.send(TerminalEvent::Input(unsafe { std::slice::from_raw_parts(bytes, len) }.to_vec()));
        }
    })
}

/// The grid the surface settled on: to the listener (the host's PTY takes it).
pub unsafe extern "C" fn receive_resize(userdata: *mut c_void, columns: u16, rows: u16, width: u32, height: u32) {
    guarded((), || {
        // SAFETY: the surface's receive userdata.
        if let Some(shared) = unsafe { shared_of(userdata) } {
            shared.send(TerminalEvent::Resized { columns, rows, width, height });
        }
    })
}
