//! Native views the host lays over the page (docs/tauri-architecture.md §3, NativeSlot). The page reserves an empty slot and
//! reports where it is (`surfaceBounds`); the host's `SurfaceManager` asks the platform for a view of the slot's kind and moves,
//! shows and hides it there. Which kinds an OS draws natively is its [`SurfaceFactory::drawing`]:
//!
//! | kind | Windows | Mac |
//! |---|---|---|
//! | terminal | `web` (xterm.js in the page, ConPTY) | `native` (Ghostty's NSView, libghostty) |
//! | browser | `native` (a WebView2 child webview) | `native` (CEF or WKWebView, chosen by `browser.engineEnabled`) |

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::{BoxFuture, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SurfaceKind {
    Terminal,
    Browser,
}

impl SurfaceKind {
    pub const ALL: [SurfaceKind; 2] = [SurfaceKind::Terminal, SurfaceKind::Browser];

    /// The name the protocol uses (`surface` of `surfaceBounds`, the keys of the `host` push's `surfaces`).
    pub fn name(self) -> &'static str {
        match self {
            SurfaceKind::Terminal => "terminal",
            SurfaceKind::Browser => "browser",
        }
    }

    pub fn parse(name: &str) -> Option<SurfaceKind> {
        SurfaceKind::ALL.into_iter().find(|kind| kind.name() == name)
    }
}

/// Who draws a kind of pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Drawing {
    /// The page draws it (xterm.js); the host only feeds it.
    Web,
    /// The platform lays a native view over the page's slot.
    Native,
}

impl Drawing {
    pub fn name(self) -> &'static str {
        match self {
            Drawing::Web => "web",
            Drawing::Native => "native",
        }
    }
}

/// A rectangle of the page in device pixels (CSS pixels × devicePixelRatio, rounded outwards to whole pixels as the page's
/// `slotBounds` does), from the page's top left, which is the window's client area's top left.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DeviceRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl DeviceRect {
    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    /// The same rectangle in logical units (points) at `scale` device pixels per point: what an NSView frame takes. (Tauri's
    /// webviews take physical pixels, so Windows passes the device rectangle through unchanged.)
    pub fn to_logical(&self, scale: f64) -> LogicalRect {
        let scale = if scale > 0.0 { scale } else { 1.0 };
        LogicalRect {
            x: f64::from(self.x) / scale,
            y: f64::from(self.y) / scale,
            width: f64::from(self.width) / scale,
            height: f64::from(self.height) / scale,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LogicalRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// What the OS's view really is, read back from the OS (not what was last asked): the smoke compares it with the page's slot.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SurfaceState {
    pub bounds: DeviceRect,
    pub visible: bool,
    /// A browser's current address.
    pub url: Option<String>,
}

/// One native view over the page.
///
/// Contract: every call returns without waiting for the UI thread to draw (the view follows the page's frame); `place` with an
/// unchanged rectangle and `set_visible` with the current state do nothing visible; after `close` the view is gone and every call
/// is an error.
pub trait NativeSurface: Send + Sync {
    fn place(&self, rect: DeviceRect) -> Result<()>;
    fn set_visible(&self, visible: bool) -> Result<()>;
    /// Gives the view the keyboard (the page asked: a click in the slot, `surfaceAction{action:'focus'}`).
    fn focus(&self) -> Result<()>;
    /// The view's picture now, as a PNG `data:` URL, taken just before it hides under a page overlay so the slot can show it
    /// meanwhile; None when this platform has no picture to give.
    fn snapshot(&self) -> BoxFuture<'_, Result<Option<String>>>;
    fn inspect(&self) -> BoxFuture<'_, Result<SurfaceState>>;
    fn close(&self) -> Result<()>;
}

/// What a browser view tells its host as it browses (the engine's events C#'s pane follows, MainWindow.WebShell.Panes.cs:385-395).
/// Sent on the OS's UI thread: a listener must not block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowserEvent {
    /// A navigation ended: the address the engine reports (else the one it started with) and whether it succeeded. The host's
    /// history follows these (C# `BrowserHistory.Visit` on `NavigationCompleted`).
    Navigated { url: Option<String>, success: bool },
    /// The page asked for a new window (`window.open`, a link to one): refused, nothing opened.
    PopupRefused,
    /// A download started: cancelled.
    DownloadRefused,
    /// The view took the keyboard (a press in the page): its pane becomes the active one.
    Focused,
}

/// Where a browser view sends its [`BrowserEvent`]s.
#[derive(Clone)]
pub struct BrowserListener(Arc<dyn Fn(BrowserEvent) + Send + Sync>);

impl BrowserListener {
    pub fn new(listener: impl Fn(BrowserEvent) + Send + Sync + 'static) -> BrowserListener {
        BrowserListener(Arc::new(listener))
    }

    pub fn send(&self, event: BrowserEvent) {
        (self.0)(event);
    }
}

impl std::fmt::Debug for BrowserListener {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("BrowserListener")
    }
}

impl PartialEq for BrowserListener {
    fn eq(&self, other: &BrowserListener) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for BrowserListener {}

/// A browser pane's view: it refuses popups and downloads, shows no script dialog, and tells its listener where it went.
pub trait BrowserSurface: NativeSurface {
    /// Loads `url` (already resolved: C# `BrowserAddress.Resolve`).
    fn navigate(&self, url: &str) -> Result<()>;
    /// The engine's own back and forward (C# `CoreWebView2.GoBack`/`GoForward`); the host's history moves with them.
    fn go_back(&self) -> Result<()>;
    fn go_forward(&self) -> Result<()>;
    fn reload(&self) -> Result<()>;
}

/// A terminal the platform draws itself (the Mac's Ghostty view, which owns its PTY). Output never crosses the bridge; title,
/// working folder and the clipboard come back through the host's `terminal` pushes as for the web terminal.
pub trait TerminalSurface: NativeSurface {
    /// Text as if typed or pasted (the composer's "run in terminal", a paste the page forwards).
    fn send_text(&self, text: &str) -> Result<()>;
    fn set_font(&self, family: &str, size: f64) -> Result<()>;
    fn set_theme(&self, light: bool) -> Result<()>;
}

/// A point in DIPs (screen points on the Mac, device-independent pixels on Windows).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LogicalPoint {
    pub x: f64,
    pub y: f64,
}

/// Where the pointer was: in the pet window (DIPs from its top left, at the scale of the scene last drawn) and on the screen (the
/// desktop's own units: physical pixels on Windows, points on the Mac), so a drag follows the screen while the window moves under it.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PetPointer {
    pub window: LogicalPoint,
    pub screen: LogicalPoint,
}

/// What the pet window tells its owner (C# `CompanionOverlay`'s pointer, wheel, DPI and display handlers), in order, from the window's
/// own thread through [`PetWindow::events`]. Only presses where the drawn alpha is not 0 reach the window (clicks pass through
/// elsewhere); the owner decides what a press means (a drag after 4 px, a resize strip, a button, the menu).
#[derive(Debug, Clone, PartialEq)]
pub enum PetEvent {
    /// The left button went down (the window holds the pointer until it goes up).
    Pressed(PetPointer),
    /// The pointer moved while over the window or while the left button is held (captured).
    Moved(PetPointer),
    /// The left button went up.
    Released(PetPointer),
    DoubleClicked(PetPointer),
    /// The right button went up (the self-drawn menu).
    RightClicked(PetPointer),
    /// The wheel turned: `delta` in Windows' units (120 a notch; the bubble pages each 120).
    Wheel {
        pointer: PetPointer,
        delta: i32,
    },
    /// A press ended without a release (capture lost, Esc, the window hidden).
    Cancelled,
    /// The window's monitor scale changed (Windows `WM_DPICHANGED`, clamped to 1–4): the owner lays the scene out again.
    DpiChanged {
        scale: f64,
    },
    /// A display came, went or moved (the work area may have changed: the owner keeps the window on screen).
    DisplayChanged,
    /// The system kept refusing the window's frames (C#: 36 in a row, three seconds' worth): the owner ends the pet.
    Failed {
        message: String,
    },
}

/// The pointer's shape over the pet window (C#'s `WM_SETCURSOR`: the arrow, or a resize strip's).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PetCursor {
    #[default]
    Arrow,
    /// A side strip (`IDC_SIZEWE`).
    SizeWestEast,
    /// The top strip (`IDC_SIZENS`).
    SizeNorthSouth,
    /// The top-left corner (`IDC_SIZENWSE`).
    SizeNorthWestSouthEast,
    /// The top-right corner (`IDC_SIZENESW`).
    SizeNorthEastSouthWest,
}

/// What the desktop's pointer and keyboard did since the last look (C#'s `GetCursorPos` and `GetAsyncKeyState` in `Draw`): the window
/// never holds the keyboard or the pointer, so it looks to learn of a press elsewhere that puts its menu away.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PetPoll {
    /// The pointer, in the desktop's own units.
    pub cursor: LogicalPoint,
    /// A mouse button is down now, or was pressed and let go since the last look.
    pub pressed: bool,
    /// Esc is down now, or was since the last look.
    pub escape: bool,
}

/// The window's last picture as it drew it (C# `CaptureForSmoke`: its own buffer, never the desktop): premultiplied BGRA rows.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct PetCapture {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
    /// How many frames the window has drawn (a caller waits for its draw to have been made).
    pub frames: u64,
}

impl std::fmt::Debug for PetCapture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PetCapture")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("frames", &self.frames)
            .finish_non_exhaustive()
    }
}

/// Whether the pet window took the focus (C#'s smoke: `NonActivating`, `ForegroundWindow`, `TookThread`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PetFocus {
    /// The window has the no-activate style (`WS_EX_NOACTIVATE`).
    pub non_activating: bool,
    /// The desktop's front window now (an opaque handle, compared around each step).
    pub foreground: u64,
    /// The pet window's own handle.
    pub own: u64,
    /// The window's thread made it active or gave it the keyboard (`GetActiveWindow`/`GetFocus` of that thread).
    pub took_thread: bool,
}

/// Input as a pointer or a display would send it, through the window's own procedure (C#'s smoke `SendMessage`s): the points are in the
/// window's DIPs at the scale of the scene last drawn.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PetInput {
    /// The left button down and up at one point (`WM_LBUTTONDOWN`, `WM_LBUTTONUP`).
    Click(LogicalPoint),
    Press(LogicalPoint),
    /// The pointer moved, the left button held, to a point of the window.
    Drag(LogicalPoint),
    Release(LogicalPoint),
    /// `WM_LBUTTONDBLCLK`.
    DoubleClick(LogicalPoint),
    /// `WM_RBUTTONUP`.
    RightClick(LogicalPoint),
    /// `WM_MOUSEWHEEL` of `delta` (120 a notch) at a point.
    Wheel {
        at: LogicalPoint,
        delta: i32,
    },
    /// `WM_DPICHANGED` to this many dots an inch.
    Dpi(u32),
}

/// The companion pet (W8-E, D8): a window of its own over every app, never activated, clicks passing through where it draws nothing
/// (Windows: a `WS_EX_NOACTIVATE|WS_EX_LAYERED|WS_EX_TOOLWINDOW` popup updated with `UpdateLayeredWindow`; the Mac: a non-activating
/// NSPanel). The shared logic lays out a [`crate::PetScene`]; the platform draws it.
///
/// Contract: `show` draws `scene` first and only then shows the window (C#: the first frame before Show, so no frame passes clicks
/// through everywhere), without taking the focus; `draw` replaces the picture, the window's size and its place together (the scene's
/// size, scale and origin); `place` moves the window's top left to `at` (the desktop's own units) without activating it (a carry);
/// `work_area` is the work area of the monitor nearest `near` and `scale_at` that monitor's scale (1 to 4), in the desktop's own units;
/// `events` hands over the window's [`PetEvent`]s and may be taken once (a second call is an error); after `close` the window is gone and
/// every call is an error. Every call but `send` returns without waiting for the window's thread to draw.
pub trait PetWindow: Send + Sync {
    fn show(&self, scene: &crate::PetScene) -> Result<()>;
    fn draw(&self, scene: &crate::PetScene) -> Result<()>;
    fn place(&self, at: LogicalPoint) -> Result<()>;
    fn work_area(&self, near: LogicalPoint) -> Result<LogicalRect>;
    fn events(&self) -> Result<tokio::sync::mpsc::UnboundedReceiver<PetEvent>>;
    fn close(&self) -> Result<()>;
    /// The scale of the monitor nearest `near` (C#: `GetDpiForWindow` of the window made where it was left), clamped to 1–4.
    fn scale_at(&self, near: LogicalPoint) -> Result<f64> {
        let _ = near;
        Ok(1.0)
    }
    /// The words measured as this window draws them.
    fn measure(&self) -> Arc<dyn crate::pet::PetMeasure> {
        Arc::new(crate::pet::ApproximateMeasure)
    }
    /// What the pointer and the keyboard did since the last look.
    fn poll(&self) -> Result<PetPoll> {
        Ok(PetPoll::default())
    }
    /// The pointer's shape while it is over the window.
    fn set_cursor(&self, cursor: PetCursor) -> Result<()> {
        let _ = cursor;
        Ok(())
    }
    /// The window's last picture (the smoke's capture).
    fn capture(&self) -> Result<PetCapture> {
        Err(crate::PlatformError::unsupported("PetWindow", "capture"))
    }
    /// Whether the window took the focus (the smoke's checks around each step).
    fn focus(&self) -> Result<PetFocus> {
        Err(crate::PlatformError::unsupported("PetWindow", "focus"))
    }
    /// Input sent through the window's own procedure (the smoke), returning once the window handled it.
    fn send(&self, input: PetInput) -> Result<()> {
        let _ = input;
        Err(crate::PlatformError::unsupported("PetWindow", "send"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserOptions {
    pub pane_id: String,
    /// The first address.
    pub url: String,
    /// Where its web data lives (cookies, storage): its workspace's profile (C# `BrowserProfile`), shared by that workspace's panes.
    pub data_dir: PathBuf,
    /// The slot it opens over (it opens shown).
    pub bounds: DeviceRect,
    /// Where its events go, from before its first address loads.
    pub listener: Option<BrowserListener>,
}

#[derive(Clone, PartialEq, Eq)]
pub struct TerminalOptions {
    pub pane_id: String,
    pub cwd: PathBuf,
    pub program: Option<PathBuf>,
    pub args: Vec<String>,
    /// The whole environment (the resolved CLI environment).
    pub env: BTreeMap<String, String>,
    pub bounds: DeviceRect,
}

impl std::fmt::Debug for TerminalOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TerminalOptions")
            .field("pane_id", &self.pane_id)
            .field("cwd", &self.cwd)
            .field("bounds", &self.bounds)
            .finish_non_exhaustive()
    }
}

/// Makes the native views of one window, and says which kinds the page must draw itself.
pub trait SurfaceFactory: Send + Sync {
    fn drawing(&self, kind: SurfaceKind) -> Drawing;
    fn open_browser(&self, options: BrowserOptions) -> BoxFuture<'_, Result<Box<dyn BrowserSurface>>>;
    fn open_terminal(&self, options: TerminalOptions) -> BoxFuture<'_, Result<Box<dyn TerminalSurface>>>;
    fn open_pet(&self) -> BoxFuture<'_, Result<Box<dyn PetWindow>>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_have_the_protocol_names() {
        assert_eq!(SurfaceKind::parse("browser"), Some(SurfaceKind::Browser));
        assert_eq!(SurfaceKind::parse("terminal"), Some(SurfaceKind::Terminal));
        assert_eq!(SurfaceKind::parse("pet"), None);
        assert_eq!((Drawing::Web.name(), Drawing::Native.name()), ("web", "native"));
    }

    #[test]
    fn device_pixels_become_points() {
        let rect = DeviceRect { x: 30, y: 15, width: 300, height: 150 };
        assert_eq!(rect.to_logical(1.5), LogicalRect { x: 20.0, y: 10.0, width: 200.0, height: 100.0 });
        assert_eq!(rect.to_logical(0.0), LogicalRect { x: 30.0, y: 15.0, width: 300.0, height: 150.0 });
        assert!(DeviceRect { width: 0, ..rect }.is_empty());
    }
}
