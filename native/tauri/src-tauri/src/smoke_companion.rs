//! The companion step (C# MainWindow.Companion.cs's smoke; live key `companion`), `Stage::Companion` after the remote step: only its own
//! test overlay (NOACTIVATE, the focus compared around each step as C#'s `Kept()`, closed in `finally`), never the real profile's
//! settings; the popover, the paw and the capsule through the page. Owner: W8-E.

#![allow(dead_code, reason = "a W8 part's seam, at rest until it fills it")]

use std::sync::Arc;

use serde_json::Value;

use crate::host::Host;

/// Starts the step; false when it does not run (then the smoke goes on to the next one).
/// It reports once through `Smoke::probed`. At rest: it does not run.
pub fn start(host: &Arc<Host>) -> bool {
    let _ = host;
    false
}

/// The step's checks (C#'s `companion*` keys) from its report. None at rest (a step that never started adds none).
pub fn checks(report: &Value) -> Vec<(&'static str, bool)> {
    let _ = report;
    vec![]
}
