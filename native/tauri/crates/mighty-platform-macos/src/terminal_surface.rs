//! M-D: the Ghostty view over the page's NativeSlot, drawn by libghostty's C API from Rust (plan D1 as the user chose it; the Step 0
//! spike `native/tauri/spikes/mac/ghostty` is the proof and the starting point). Rust owns the PTY (plan D2): the surface uses the
//! host-managed I/O backend (`GHOSTTY_SURFACE_IO_BACKEND_HOST_MANAGED`), the PTY's output goes in through
//! `ghostty_surface_write_buffer` ([`mighty_platform::TerminalSurface::feed`]) and what the user types and the grid come back
//! through `receive_buffer` and `receive_resize` ([`mighty_platform::TerminalEvent`]).
//!
//! The behaviour reference, read and ported, never linked: Swift `LocalTerminalSession.swift`, `HostTerminalView.swift`,
//! `LocalTerminalView.swift`, `AppStore+Terminals.swift` (the settings: [`config`]), `AgentTerminalPaneView.swift`,
//! `ClipboardShortcut.swift`, `TerminalInputPolicy.swift`, and the vendored `GhosttyTerminal`'s AppKit view (its key handling,
//! `NSTextInputClient` and IME preedit, display link, clipboard callbacks, snapshot): what the Swift view did, this view does in Rust
//! over the same C functions.
//!
//! - `ghostty`: the C API's declarations; `runtime`: the one libghostty app, its config, frames and callbacks; `view`: the NSView
//!   (input, IME, mouse, copy and paste); `surface`: [`mighty_platform::TerminalSurface`] over them. These link libghostty, so they
//!   build only with `cfg(mighty_ghostty)` (`build.rs`, `MIGHTY_MAC_NATIVE=1` and `MIGHTY_GHOSTTY_XCFRAMEWORK`); without it [`open`]
//!   answers `Unsupported`, as before M-D.
//! - [`config`] and [`keys`]: the config text and the keyboard rules, with no AppKit or libghostty in them (tested as they are).
//! - [`pty`]: a minimal `forkpty` PTY host behind the same port shape, wired in until M-A's `MacPtyHost` lands.

#[cfg_attr(not(mighty_ghostty), allow(dead_code))]
pub mod config;
#[cfg(mighty_ghostty)]
mod ghostty;
#[cfg_attr(not(mighty_ghostty), allow(dead_code))]
pub mod keys;
pub mod pty;
#[cfg(mighty_ghostty)]
mod runtime;
#[cfg(mighty_ghostty)]
mod surface;
#[cfg(mighty_ghostty)]
mod view;

use mighty_platform::{Result, TerminalOptions, TerminalSurface};

/// The bridge's ABI without a window, for the Mac's `cargo test` (`tests/ghostty_headless.rs`).
#[cfg(mighty_ghostty)]
pub use surface::{Headless, headless_round_trip};

/// Opens the Ghostty view of a terminal pane over `window`'s page, at `options.bounds`, shown; its events go to `options.listener`.
pub async fn open(window: &tauri::Window, options: TerminalOptions) -> Result<Box<dyn TerminalSurface>> {
    #[cfg(mighty_ghostty)]
    {
        surface::open(window, options).await
    }
    #[cfg(not(mighty_ghostty))]
    {
        let _ = (window, options);
        Err(mighty_platform::PlatformError::unsupported("SurfaceFactory", "open_terminal"))
    }
}
