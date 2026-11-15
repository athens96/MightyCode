//! The smoke's app update step (C# `RunAppUpdateSectionSmoke`, `RunAppUpdateStagedSmoke`, `AppUpdateSmokeOutcome`; plan W9-C). Owner:
//! W9-C.
//!
//! `appUpdateSection` (C#'s key): a key-less refusal on the real control first, then the signed build's rendering of each of
//! mighty-updates' `app_update::smoke::fixture_states` (`statuses`, `buttons`, `refusedWithoutPublicKey`, `toggledTo`, `restored`), and the
//! staged half (a composer draft holds the automatic restart: `AppIdleBlocker.Draft`). No network, no disk, nothing installed.

use std::sync::Arc;

use serde_json::Value;

use crate::host::Host;

/// Starts the step after the updates step (smoke.rs `Stage::AppUpdate`): true when it started; it reports once through
/// `Smoke::probed`. Step 0 stub (W9-C writes it): it does not start.
pub fn start(_host: &Arc<Host>) -> bool {
    false
}

/// The step's checks from its report. Step 0 stub (W9-C writes it): none.
pub fn checks(_report: &Value) -> Vec<(&'static str, bool)> {
    Vec::new()
}
