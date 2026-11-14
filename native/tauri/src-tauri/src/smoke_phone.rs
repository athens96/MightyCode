//! The remote step's phone part (W8-B's round trip; W7 plan D8, W8 plan D16), last of `Stage::Remote`: the in-process fake relay on
//! loopback (never a real relay), a scripted phone paired with the smoke profile's identity, `/m1/info`, `/m1/state`, a session, a
//! submit through `Router::route` with a smoke device. Its report is the stage report's `phone`. Owner: W8-B (the fake relay is W8-A's).

#![allow(dead_code, reason = "a W8 part's seam, at rest until it fills it")]

use std::sync::Arc;

use serde_json::Value;

use crate::host::Host;

/// Starts the phone part; false when it does not run (then the smoke goes on to the next step).
/// It reports once through `Smoke::probed`. At rest: it does not run.
pub fn start(host: &Arc<Host>) -> bool {
    let _ = host;
    false
}

/// The phone part's checks from its report. None at rest (a step that never started adds none).
pub fn checks(report: &Value) -> Vec<(&'static str, bool)> {
    let _ = report;
    vec![]
}
