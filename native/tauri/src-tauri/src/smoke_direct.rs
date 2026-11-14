//! The remote step's direct part (C# WinUI/MainWindow.MobileDirectSmoke.cs; live key `mobileDirect`), first of `Stage::Remote` (after
//! the updates step): the direct listener on loopback port 0 through the fake network (never a real interface), admission, the upgrade,
//! the relay handshake inside it with the smoke's own identity, the firewall rows over the recording runner (nothing elevates). Its report
//! is the stage report's `mobileDirect`. Owner: W8-C.

#![allow(dead_code, reason = "a W8 part's seam, at rest until it fills it")]

use std::sync::Arc;

use serde_json::Value;

use crate::host::Host;

/// Starts the direct part; false when it does not run (then the smoke goes on to the remote workspaces part).
/// It reports once through `Smoke::probed`. At rest: it does not run.
pub fn start(host: &Arc<Host>) -> bool {
    let _ = host;
    false
}

/// The direct part's checks (C#'s `mobileDirect*` keys) from its report. None at rest (a step that never started adds none).
pub fn checks(report: &Value) -> Vec<(&'static str, bool)> {
    let _ = report;
    vec![]
}
