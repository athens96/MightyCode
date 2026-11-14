//! The companion pet's window on Windows (C# WinUI\CompanionOverlay.cs; plan §0.6, §1.7, D8, W8-E): a
//! `WS_EX_NOACTIVATE|WS_EX_LAYERED|WS_EX_TOOLWINDOW` popup, a window class per instance, `WM_MOUSEACTIVATE` answered `MA_NOACTIVATE`,
//! kept topmost with `SetWindowPos(HWND_TOPMOST, SWP_NOACTIVATE)`, per-monitor DPI clamped to 1–4 with the re-entry guard on
//! `WM_DPICHANGED`, mighty-platform's `PetScene` drawn with Direct2D and DirectWrite (a software fallback) onto a DIB section and put
//! on screen with `UpdateLayeredWindow` (premultiplied BGRA: clicks pass through where alpha is 0), the window procedure catching
//! panics. Owner: W8-E.
//!
//! At rest (W8 Step 0): no window opens; `open` answers `Unsupported`, as `open_pet` did before W8.

use mighty_platform::{PetWindow, PlatformError, Result};

/// A new pet window (not shown until its first `show`).
pub fn open() -> Result<Box<dyn PetWindow>> {
    Err(PlatformError::unsupported("SurfaceFactory", "open_pet"))
}
