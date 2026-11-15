//! M-D: the Ghostty view over the page's NativeSlot, drawn by libghostty's C API from Rust (plan D1 as the user chose it; the Step 0
//! spike `native/tauri/spikes/mac/ghostty` is the proof and the starting point). Rust owns the PTY (plan D2, [`crate::pty`]): the
//! surface uses the host-managed I/O backend (`GHOSTTY_SURFACE_IO_BACKEND_HOST_MANAGED`), the PTY's output goes in through
//! `ghostty_surface_write_buffer` ([`mighty_platform::TerminalSurface::feed`]) and what the user types and the grid come back
//! through `receive_buffer` and `receive_resize` ([`mighty_platform::TerminalEvent`]).
//!
//! The behaviour reference, read and ported, never linked: Swift `LocalTerminalSession.swift` (263 lines), `HostTerminalView.swift`
//! (37), `LocalTerminalView.swift` (198), `AppStore+Terminals.swift` (124: the settings below), `AgentTerminalPaneView.swift` (87),
//! `ApplicationCopyRouter.swift` (96), `TerminalInputPolicy.swift`, and the vendored `GhosttyTerminal`'s AppKit view (its key
//! handling, `NSTextInputClient` and IME preedit, display link, clipboard callbacks, snapshot): what the Swift view did, this view
//! does in Rust over the same C functions. Settings: light `FAF9F6`, dark `202020`/`DEDCD8`/cursor `E1AB91`, the code font,
//! padding 10, `clipboard-read=deny`, `clipboard-write=ask`, `scrollback-limit=10000000`, `super+` t/n/o/k/q/comma/=/-/0 unbound,
//! Ghostty's own user config never loaded. libghostty is linked by `build.rs` with `MIGHTY_MAC_NATIVE=1` (`cfg(mighty_ghostty)`).

use mighty_platform::{PlatformError, Result, TerminalOptions, TerminalSurface};

/// Opens the Ghostty view of a terminal pane over `window`'s page (Unsupported until M-D).
pub async fn open(_window: &tauri::Window, _options: TerminalOptions) -> Result<Box<dyn TerminalSurface>> {
    Err(PlatformError::unsupported("SurfaceFactory", "open_terminal"))
}
