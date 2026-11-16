//! What the smoke records of the app update box (C# `AppUpdateSmokeOutcome`, `AppUpdateSmoke`, AppUpdateCoordinator.cs:465-556; plan
//! W9-C). Owner: W9-A (the fixtures and the decision); W9-C's `smoke_app_update.rs` walks the real box through them. Fixture data only:
//! no manifest is fetched, no package downloaded, no disk written.

use std::collections::BTreeMap;

use chrono::{DateTime, Local, Utc};
use mighty_core::app_update_view::describe;
use mighty_core::locale::Locale;
use serde::Serialize;

use super::Phase;
use super::coordinator::UpdateState;
use super::manifest::{Asset, Availability, Manifest};

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

/// C#'s `FixtureAvailability`: version 9.9.9 (build 1, no notes) for both architectures at `https://example.invalid/…`, beside 0.1.0.
fn fixture_availability() -> Availability {
    let asset = |architecture: &str, digit: char| Asset {
        url: format!("https://example.invalid/MightyCode-windows-{architecture}.zip"),
        sha256: digit.to_string().repeat(64),
        size: 1024,
    };
    Availability {
        current: "0.1.0".into(),
        manifest: Manifest {
            version: "9.9.9".into(),
            build: Some(1),
            notes: None,
            published_at: None,
            windows: BTreeMap::from([("x64".to_string(), asset("x64", 'a')), ("arm64".to_string(), asset("arm64", 'b'))]),
            macos: None,
            minimum_system_version: None,
        },
    }
}

/// C# `AppUpdateSmoke.FixtureStates`: the phases the box is walked through (checking, available, downloading at 0.42, staging, ready,
/// installing, up to date, failed, ready beside a failed check), their texts in `locale`'s language.
pub fn fixture_states(locale: &Locale) -> Vec<UpdateState> {
    let available = || Some(fixture_availability());
    let check_failed = locale.get("appUpdate.error.checkFailed");
    vec![
        UpdateState { phase: Phase::Checking, ..UpdateState::default() },
        UpdateState { phase: Phase::Available, availability: available(), ..UpdateState::default() },
        UpdateState { phase: Phase::Downloading, download_fraction: 0.42, availability: available(), ..UpdateState::default() },
        UpdateState { phase: Phase::Staging, availability: available(), ..UpdateState::default() },
        UpdateState { phase: Phase::Ready, availability: available(), ..UpdateState::default() },
        UpdateState { phase: Phase::Installing, availability: available(), ..UpdateState::default() },
        UpdateState { phase: Phase::UpToDate, ..UpdateState::default() },
        UpdateState { phase: Phase::Failed, error_message: Some(check_failed.clone()), ..UpdateState::default() },
        // A failed background check beside a staged update: it stays ready.
        UpdateState { phase: Phase::Ready, availability: available(), last_error: Some(check_failed), ..UpdateState::default() },
    ]
}

/// C#'s `Format`: the local time as `HH:mm`.
fn format(time: &DateTime<Utc>) -> String {
    time.with_timezone(&Local).format("%H:%M").to_string()
}

/// C# `ExpectedStatuses`: the status sentence the box must show for each fixture phase (a signed build, no built-in address, the
/// switch on), in order.
pub fn expected_statuses(locale: &Locale) -> Vec<String> {
    fixture_states(locale).iter().map(|state| describe(locale, &state.view(), true, None, format, true).status_text).collect()
}

/// C# `ExpectedButtons`.
pub fn expected_buttons(locale: &Locale) -> Vec<String> {
    fixture_states(locale).iter().map(|state| describe(locale, &state.view(), true, None, format, true).button_label).collect()
}

/// C# `AppUpdateSmoke.RunAsync`: the smoke's decision — the box rendered every fixture phase, a key-less build refused to check, and
/// the saved switch, flipped, came back. `write_auto_check` saves the switch; the original is written back whatever happened.
pub fn decide(
    locale: &Locale,
    rendered_statuses: &[String],
    rendered_buttons: &[String],
    refused_without_public_key: bool,
    read_auto_check: &dyn Fn() -> bool,
    write_auto_check: &mut dyn FnMut(bool),
) -> Result<SmokeOutcome, String> {
    let (statuses, buttons) = (expected_statuses(locale), expected_buttons(locale));
    if rendered_statuses != statuses.as_slice() {
        return Err(format!(
            "the app update section must show every fixture status: expected {} but showed {}",
            statuses.join(" | "),
            rendered_statuses.join(" | ")
        ));
    }
    if rendered_buttons != buttons.as_slice() {
        return Err(format!(
            "the app update section must show every fixture button: expected {} but showed {}",
            buttons.join(" | "),
            rendered_buttons.join(" | ")
        ));
    }
    if !refused_without_public_key {
        return Err("a build without a public key must refuse to check".into());
    }
    let original = read_auto_check();
    let flipped = !original;
    write_auto_check(flipped);
    let persisted = read_auto_check() == flipped;
    write_auto_check(original);
    if !persisted {
        return Err("the automatic check switch did not persist to saved state".into());
    }
    if read_auto_check() != original {
        return Err("the automatic check switch was not restored".into());
    }
    Ok(SmokeOutcome {
        statuses: rendered_statuses.to_vec(),
        buttons: rendered_buttons.to_vec(),
        refused_without_public_key: true,
        toggled_to: flipped,
        restored: true,
    })
}
