//! The smoke's styles step (Rust-only; plan W9-D): the `deck` fixture registered into the smoke's profile and approved, the guide panel
//! drawn, `guidedAction{what:'action'}` sending the rewritten slash command to the fake Claude, a rescan, the removal. Owner: W9-D.

use std::sync::Arc;

use serde_json::Value;

use crate::host::Host;

/// Starts the step after the settings step (smoke.rs `Stage::Styles`): true when it started; it reports once through `Smoke::probed`.
/// Step 0 stub (W9-D writes it): it does not start.
pub fn start(_host: &Arc<Host>) -> bool {
    false
}

/// The step's checks from its report. Step 0 stub (W9-D writes it): none.
pub fn checks(_report: &Value) -> Vec<(&'static str, bool)> {
    Vec::new()
}
