//! What the smoke records of the app update box (C# `AppUpdateSmokeOutcome`, `AppUpdateSmoke`, AppUpdateCoordinator.cs:465-556; plan
//! W9-C). Owner: W9-A (the fixtures); W9-C's `smoke_app_update.rs` walks the real box through them. Fixture data only: no manifest is
//! fetched, no package downloaded, no disk written.

use serde::Serialize;

use super::coordinator::UpdateState;

/// The smoke result's key (C# `AppUpdateSmokeOutcome.ResultKey`).
pub const RESULT_KEY: &str = "appUpdateSection";

/// C# `AppUpdateSmokeOutcome`, serialized with C#'s names.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SmokeOutcome {
    pub statuses: Vec<String>,
    pub buttons: Vec<String>,
    pub refused_without_public_key: bool,
    pub toggled_to: bool,
    pub restored: bool,
}

/// C# `AppUpdateSmoke.FixtureStates`: the phases the box is walked through (checking, available, downloading at 0.42, staging, ready,
/// installing, up to date, failed, ready beside a failed check), their texts in `locale`'s language. Step 0 stub (W9-A writes it): none.
pub fn fixture_states(_locale: &mighty_core::locale::Locale) -> Vec<UpdateState> {
    Vec::new()
}
