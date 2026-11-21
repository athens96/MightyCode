//! The session on Windows (W9-C; C# `MainWindow.AppUpdate.cs`'s session-ending watch and `GetLastInputInfo`; plan §W9-C):
//! `WM_QUERYENDSESSION` / `WM_ENDSESSION` through `SetWindowSubclass` on the main window's HWND, and `GetLastInputInfo` against
//! `GetTickCount`. The window arrives after the platform is made: [`watch_window`] hands this port the main window once it exists
//! (src-tauri's `platform::watch_session`, from main.rs's setup on the window's own thread).
//!
//! C#'s rule (`WatchSessionEnding`): the window procedure is subclassed so the flag is set on the UI thread before anything closes.
//! `WM_QUERYENDSESSION` reaches every top-level window; whatever its reason (sign-out, restart, shutdown, an installer closing apps), a
//! quit that follows leaves the staged update for the next quit. `WM_ENDSESSION` with wParam 0 means the end was called off. The
//! subclass removes itself at `WM_NCDESTROY`.

use std::sync::Mutex;
use std::time::Duration;

use mighty_platform::{PlatformError, Result, SessionCallback, SessionEnding, SessionWatch, Subscription};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::SystemInformation::GetTickCount;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{WM_ENDSESSION, WM_NCDESTROY, WM_QUERYENDSESSION};

/// C#'s `SessionEndingSubclassId`.
const SUBCLASS_ID: usize = 0x4D43_5155;

/// The subscribers to the session's messages, with the id each subscription removes.
struct Watchers {
    next: u64,
    callbacks: Vec<(u64, SessionCallback)>,
    /// The window subclassed (its HWND as a number), so a second call does nothing.
    window: Option<isize>,
}

static WATCHERS: Mutex<Watchers> = Mutex::new(Watchers { next: 0, callbacks: Vec::new(), window: None });

fn watchers() -> std::sync::MutexGuard<'static, Watchers> {
    WATCHERS.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Each subscriber called with `event`, with no lock held.
fn deliver(event: SessionEnding) {
    let callbacks: Vec<SessionCallback> = watchers().callbacks.iter().map(|(_, callback)| callback.clone()).collect();
    for callback in callbacks {
        callback(event);
    }
}

/// C#'s subclass procedure: the session's messages reported, every message passed on. Wrapped in `catch_unwind` so a panicking
/// subscriber never unwinds across the Win32 FFI boundary.
unsafe extern "system" fn subclass(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM, id: usize, _data: usize) -> LRESULT {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match message {
        WM_QUERYENDSESSION => deliver(SessionEnding::Query),
        WM_ENDSESSION => deliver(SessionEnding::End { ending: wparam != 0 }),
        WM_NCDESTROY => {
            // SAFETY: this procedure's own subclass on the window being destroyed, removed on its own thread.
            unsafe { RemoveWindowSubclass(hwnd, Some(subclass), id) };
            watchers().window = None;
        }
        _ => {}
    }));
    // SAFETY: the message as it arrived, handed to the next procedure in the window's chain.
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}

/// The main window whose session messages [`WindowsSession::on_ending`] reports. Call it on the window's own thread (main.rs's setup):
/// `SetWindowSubclass` cannot subclass a window across threads. Once is enough; a later call does nothing.
pub fn watch_window(window: &tauri::Window) -> Result<()> {
    let hwnd = window.hwnd().map_err(|error| PlatformError::Refused(format!("the main window's handle: {error}")))?;
    watch_hwnd(hwnd.0 as HWND)
}

/// [`watch_window`] on a window handle (on the window's thread).
fn watch_hwnd(hwnd: HWND) -> Result<()> {
    let mut watchers = watchers();
    if watchers.window.is_some() {
        return Ok(());
    }
    // SAFETY: a live top-level window of this process, subclassed on its own thread with a procedure that lives as long as the process.
    if unsafe { SetWindowSubclass(hwnd, Some(subclass), SUBCLASS_ID, 0) } == 0 {
        return Err(PlatformError::Refused("SetWindowSubclass failed".into()));
    }
    watchers.window = Some(hwnd as isize);
    Ok(())
}

/// How long ago the last input arrived, from the 32-bit tick counts `GetLastInputInfo` and `GetTickCount` share (the difference stays
/// right across their wrap, as C#'s `unchecked` subtraction).
fn since_last_input() -> Option<Duration> {
    let mut info = LASTINPUTINFO { cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
    // SAFETY: an out-parameter sized as the call asks.
    if unsafe { GetLastInputInfo(&mut info) } == 0 {
        return None;
    }
    // SAFETY: no arguments.
    let now = unsafe { GetTickCount() };
    Some(Duration::from_millis(u64::from(now.wrapping_sub(info.dwTime))))
}

/// The Windows session watch: the main window's session messages ([`watch_window`]) and `GetLastInputInfo`.
#[derive(Debug, Clone, Copy, Default)]
pub struct WindowsSession;

impl SessionWatch for WindowsSession {
    /// Subscribes to the main window's session messages; before [`watch_window`] none arrive (C#: "session ending not watched", the
    /// update goes on).
    fn on_ending(&self, callback: SessionCallback) -> Result<Subscription> {
        let id = {
            let mut watchers = watchers();
            watchers.next += 1;
            let id = watchers.next;
            watchers.callbacks.push((id, callback));
            id
        };
        Ok(Subscription::new(move || watchers().callbacks.retain(|(each, _)| *each != id)))
    }

    /// A failed call is an error; the caller reads it as input just now (C#), so the app never restarts on it.
    fn since_last_input(&self) -> Result<Duration> {
        since_last_input().ok_or_else(|| PlatformError::Refused("GetLastInputInfo failed".into()))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use windows_sys::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, SendMessageW, WS_OVERLAPPED};

    use super::*;

    /// The watchers are the process's: one test at a time.
    static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

    /// The last input is within the tick count's 49.7-day wrap (or refused: a session with no desktop input), and the subscribers hear
    /// each message until their subscription goes.
    #[test]
    fn the_last_input_is_read_and_messages_reach_live_subscribers_only() {
        let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        match WindowsSession.since_last_input() {
            Ok(since) => assert!(since <= Duration::from_millis(u64::from(u32::MAX))),
            Err(error) => assert_eq!(error.to_string(), PlatformError::Refused("GetLastInputInfo failed".into()).to_string()),
        }
        let heard = Arc::new(Mutex::new(Vec::new()));
        let into = heard.clone();
        let subscription =
            WindowsSession.on_ending(Arc::new(move |event| into.lock().unwrap().push(event))).expect("subscribing never fails");
        deliver(SessionEnding::Query);
        deliver(SessionEnding::End { ending: false });
        drop(subscription);
        deliver(SessionEnding::End { ending: true });
        assert_eq!(*heard.lock().unwrap(), vec![SessionEnding::Query, SessionEnding::End { ending: false }]);
    }

    /// C#'s subclass on a real (hidden) window: `WM_QUERYENDSESSION` and `WM_ENDSESSION(0)` reach the subscribers as the session's
    /// messages, every message still reaches the window, and the window's end removes the subclass.
    #[test]
    fn the_subclass_reports_the_sessions_messages_and_goes_with_its_window() {
        let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
        // SAFETY: a hidden top-level window of the stock STATIC class, made, messaged and destroyed on this thread.
        let hwnd = unsafe {
            CreateWindowExW(
                0,
                class.as_ptr(),
                std::ptr::null(),
                WS_OVERLAPPED,
                0,
                0,
                10,
                10,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            )
        };
        assert!(!hwnd.is_null(), "a hidden window");
        let heard = Arc::new(Mutex::new(Vec::new()));
        let into = heard.clone();
        let _subscription = WindowsSession.on_ending(Arc::new(move |event| into.lock().unwrap().push(event))).unwrap();
        watch_hwnd(hwnd).expect("subclassed");
        assert_eq!(watchers().window, Some(hwnd as isize));
        // SAFETY: messages sent to the window made above, on its thread.
        unsafe {
            SendMessageW(hwnd, WM_QUERYENDSESSION, 0, 0);
            SendMessageW(hwnd, WM_ENDSESSION, 0, 0);
        }
        assert_eq!(*heard.lock().unwrap(), vec![SessionEnding::Query, SessionEnding::End { ending: false }]);
        // SAFETY: the window made above, destroyed on its thread.
        assert!(unsafe { DestroyWindow(hwnd) } != 0);
        assert_eq!(watchers().window, None, "WM_NCDESTROY removed the subclass");
    }
}
