//! Screen share's ports (W8-F, `.omc/research/tauri/w8-plan.md` §1.8, D11): the displays and their pictures, and the phone's input on
//! them. Screen share is switched off in both apps (`ScreenShareAvailability.Enabled` is false: the Cloudflare relay has no TURN), so
//! these are signatures only: every OS answers `Unsupported` until the native capture (Windows Graphics Capture, ScreenCaptureKit) and
//! input (`SendInput` and the low-level hooks, CGEvent) are written after 0.5. Owner: W8-F (the shapes, as its pure crate needs them).
//!
//! mighty-remote's screen hub drives a `ScreenSharePlatform` (C#'s `IScreenSharePlatform`: capture, input and the WebRTC peer), which
//! the native screen share will build over these two ports and the transport; until then nothing builds one.

use crate::{BoxFuture, DeviceRect, Result};

/// One display (C# `ScreenDisplay`, WindowsScreenPlatform.cs).
#[derive(Debug, Clone, PartialEq)]
pub struct ScreenDisplay {
    pub id: String,
    pub name: String,
    /// In the virtual desktop's device pixels.
    pub bounds: DeviceRect,
    /// Device pixels per point.
    pub scale: f64,
    pub primary: bool,
}

/// One captured picture: `width × height` pixels, BGRA, rows tightly packed.
#[derive(Clone, PartialEq, Eq)]
pub struct ScreenFrame {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

impl std::fmt::Debug for ScreenFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScreenFrame").field("width", &self.width).field("height", &self.height).finish_non_exhaustive()
    }
}

/// Contract: `displays` lists the displays now, the primary first; `capture` takes one picture of a display (an unknown id is
/// `PlatformError::Invalid`). Nothing is captured while screen share is off.
pub trait ScreenCapture: Send + Sync {
    fn displays(&self) -> Result<Vec<ScreenDisplay>>;
    fn capture(&self, display: &str) -> BoxFuture<'_, Result<ScreenFrame>>;
}

/// The phone's typed input on a display (the `screen-control` channel's `tap|drag|scroll|text|key`), in the display's device pixels.
#[derive(Clone, PartialEq)]
pub enum RemoteInputEvent {
    Tap {
        x: f64,
        y: f64,
    },
    Drag {
        from_x: f64,
        from_y: f64,
        to_x: f64,
        to_y: f64,
    },
    Scroll {
        x: f64,
        y: f64,
        dx: f64,
        dy: f64,
    },
    /// Typed text: never logged (it may be a password).
    Text(String),
    Key {
        key: String,
        modifiers: Vec<String>,
    },
}

impl std::fmt::Debug for RemoteInputEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RemoteInputEvent::Text(_) => f.write_str("Text(..)"),
            RemoteInputEvent::Tap { .. } => f.write_str("Tap"),
            RemoteInputEvent::Drag { .. } => f.write_str("Drag"),
            RemoteInputEvent::Scroll { .. } => f.write_str("Scroll"),
            RemoteInputEvent::Key { key, .. } => f.debug_struct("Key").field("key", key).finish_non_exhaustive(),
        }
    }
}

/// Contract: `send` performs one input on `display` as the logged-in user would, and refuses it while a secure desktop or a password
/// field has the focus (C#'s UIA probe). Nothing is sent while screen share is off.
pub trait RemoteInput: Send + Sync {
    fn send(&self, display: &str, event: RemoteInputEvent) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::unsupported::Unsupported;

    /// Every OS refuses: nothing is listed, captured or sent while screen share is paused (D11).
    #[test]
    fn the_ports_refuse_until_the_native_screen_share() {
        assert!(Unsupported.displays().is_err_and(|e| e.is_unsupported()));
        let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
        assert!(runtime.block_on(Unsupported.capture("1")).is_err_and(|e| e.is_unsupported()));
        assert!(Unsupported.send("1", RemoteInputEvent::Text("SECRET".into())).is_err_and(|e| e.is_unsupported()));
        assert!(!format!("{:?}", RemoteInputEvent::Text("SECRET".into())).contains("SECRET"), "typed text never reaches Debug");
    }
}
