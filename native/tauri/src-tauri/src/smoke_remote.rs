//! The remote step's remote workspaces part (C# WinUI/MainWindow.RemoteWorkspacesSmoke.cs; live key `remoteWorkspaces`), second of
//! `Stage::Remote`: a sharing host and a client in this process over loopback port 0 (an in-memory vault, nothing written to the
//! profile but its own), one run end to end, the settings rows. Its report is the stage report's `remoteWorkspaces`. Owner: W8-D.

#![allow(dead_code, reason = "a W8 part's seam, at rest until it fills it")]

use std::sync::Arc;

use serde_json::Value;

use crate::host::Host;

/// Starts the remote workspaces part; false when it does not run (then the smoke goes on to the phone part).
/// It reports once through `Smoke::probed`. At rest: it does not run.
pub fn start(host: &Arc<Host>) -> bool {
    let _ = host;
    false
}

/// The remote workspaces part's checks (C#'s `remoteWorkspaces*` keys) from its report. None at rest (a step that never started adds none).
pub fn checks(report: &Value) -> Vec<(&'static str, bool)> {
    let _ = report;
    vec![]
}
