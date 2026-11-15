//! M-D: a terminal pane's native view (the Mac's Ghostty, `mighty-platform-macos` `terminal_surface`), fed by the pane's PTY, which
//! the host owns on both OSes (Mac plan D2). Where the page draws terminals (Windows: xterm.js) none of this runs: `SurfaceManager`
//! never asks for a terminal view there.
//!
//! The seams, Step 0's stubs for M-D to fill:
//! - [`open`]: the view for a terminal pane's slot, the first time the slot can show (surface.rs calls it; none before M-D, so the
//!   slot stays empty);
//! - [`feed`]: the pane's PTY output for its view, as terminal.rs hands the same bytes to mighty-terminal's pane buffer (the history,
//!   prompts, footer and activity keep working as on Windows);
//! - the view's [`mighty_platform::TerminalEvent`]s back to the PTY: input and grid to `PtyControl::write`/`resize`, title and working
//!   folder to the pane's `terminalMessage` handling.

use std::sync::Arc;

use mighty_platform::{DeviceRect, SurfaceFactory, TerminalSurface};

/// Opens `pane`'s Ghostty view at `rect` over the page: its PTY found through terminal.rs, its events wired back to it. None until
/// M-D (the slot stays empty).
pub async fn open(_factory: &Arc<dyn SurfaceFactory>, _pane: &str, _rect: DeviceRect) -> Option<Arc<dyn TerminalSurface>> {
    None
}

/// `pane`'s PTY output for its view (nothing to feed until M-D opens views).
#[allow(dead_code, reason = "M-D: terminal.rs feeds a pane's PTY output to its native view here")]
pub fn feed(_pane: &str, _output: &[u8]) {}
