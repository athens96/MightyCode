//! The session on Windows (W9-C; C# `MainWindow.AppUpdate.cs`'s session-ending watch and `GetLastInputInfo`; plan §W9-C):
//! `WM_QUERYENDSESSION` / `WM_ENDSESSION` through `SetWindowSubclass` on the main window's HWND, and `GetLastInputInfo` against
//! `GetTickCount`. The window arrives after the platform is made: W9-C gives this port its HWND (its own function in this file).
//!
//! Step 0: both answer `Unsupported`.

use std::time::Duration;

use mighty_platform::{PlatformError, Result, SessionCallback, SessionWatch, Subscription};

/// The Windows session watch (W9-C fills it).
#[derive(Debug, Clone, Copy, Default)]
pub struct WindowsSession;

impl SessionWatch for WindowsSession {
    fn on_ending(&self, _callback: SessionCallback) -> Result<Subscription> {
        Err(PlatformError::unsupported("SessionWatch", "on_ending"))
    }
    fn since_last_input(&self) -> Result<Duration> {
        Err(PlatformError::unsupported("SessionWatch", "since_last_input"))
    }
}
