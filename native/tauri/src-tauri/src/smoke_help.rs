//! The smoke's help capture step (Rust-only; plan W9-H): a two-shot capture (`MIGHTY_HELP_SHOTS`) in `ko`, both pictures at the shot's
//! size, `manifest.json` listing them. Owner: W9-H.

use std::sync::Arc;

use serde_json::Value;

use crate::host::Host;

/// Starts the step after the companion step (smoke.rs `Stage::Help`): true when it started; it reports once through `Smoke::probed`.
/// Step 0 stub (W9-H writes it): it does not start.
pub fn start(_host: &Arc<Host>) -> bool {
    false
}

/// The step's checks from its report. Step 0 stub (W9-H writes it): none.
pub fn checks(_report: &Value) -> Vec<(&'static str, bool)> {
    Vec::new()
}
