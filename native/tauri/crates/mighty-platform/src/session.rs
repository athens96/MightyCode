//! The user's session (W9, plan §1.4; C# `MainWindow.AppUpdate.cs`'s `WM_QUERYENDSESSION` watch and `GetLastInputInfo` in
//! `CollectIdleFacts`). Owner: W9-C.
//!
//! The automatic update needs two facts from the OS: whether Windows is signing out, restarting or shutting down (an install on quit is
//! then skipped: the swap could be cut off), and how long ago the user last touched the keyboard or mouse anywhere in the session (the
//! `UserActive` idle blocker). Windows watches the main window (`SetWindowSubclass` on its HWND); the Mac answers `Unsupported` until its
//! stage.

use std::sync::Arc;
use std::time::Duration;

use crate::{Result, Subscription};

/// What the session told the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionEnding {
    /// Windows asks whether the session may end (`WM_QUERYENDSESSION`).
    Query,
    /// The session ends (`WM_ENDSESSION` with `ending` true) or the end was cancelled (false).
    End { ending: bool },
}

/// Called on any thread with each message.
pub type SessionCallback = Arc<dyn Fn(SessionEnding) + Send + Sync>;

pub trait SessionWatch: Send + Sync {
    /// Calls `callback` with each session message until the subscription is dropped.
    fn on_ending(&self, callback: SessionCallback) -> Result<Subscription>;
    /// How long ago the user's last keyboard or mouse input was, anywhere in the session (Windows: `GetLastInputInfo`).
    fn since_last_input(&self) -> Result<Duration>;
}
