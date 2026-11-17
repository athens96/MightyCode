//! The smoke's app update step (C# `RunAppUpdateSectionSmoke`, `RunAppUpdateStagedSmoke`, `AppUpdateSmokeOutcome`; plan W9-C). Owner:
//! W9-C.
//!
//! - `appUpdateSection` (C#'s key): the real box (settings/app_update.rs, on a surface of the step's own) refuses to check on this
//!   key-less build first; then, rendered as a signed build would be, it shows each of mighty-updates' `smoke::fixture_states` (its
//!   status and button read back off the box and judged by A's `smoke::decide`, which also flips the saved switch and restores it); a
//!   composer draft holds the automatic restart (`IdleBlocker::Draft` in the host's idle facts); the switch is named for its label.
//! - `appUpdateStaged` (`RunAppUpdateStagedSmoke`, host side): a ready update with the switch on puts "installs at the next launch" in
//!   the status bar instead of the badge, a countdown gives the banner its words with 60 (or 59) seconds, and with no countdown and a
//!   newer version not yet staged the banner goes and the badge shows.
//! - `appUpdateBanner` (the page's half): the page draws the countdown banner and the status bar's staged words; its "later" calls
//!   `postponeUpdate` (the banner goes, a snooze), the status bar's "restart now" calls `restartForUpdate`, which reaches the coordinator
//!   (the fixture has no install plan: `appUpdate.error.noInstallPlan`, no helper started); then everything is put back.
//!
//! Fixture state only: no manifest fetched, no package downloaded, nothing written to disk, nothing installed.

use std::sync::Arc;
use std::time::Duration;

use mighty_core::app_update_view::{Phase, text};
use mighty_native::surface::{SurfaceModel, Surfaces};
use mighty_native::ui::Ui;
use mighty_native::view::{Place, Section, View};
use mighty_updates::app_update::auto::{self, IdleBlocker, RestartState};
use mighty_updates::app_update::smoke;
use serde_json::{Value, json};

use crate::app_update::AppUpdate;
use crate::host::Host;
use crate::settings::app_update::{BUTTON_ID, STATUS_ID, TOGGLE_ID};

/// The page's half: the banner and the status bar read, "later" and "restart now" pressed, everything put back by the host.
const PROBE: &str = r#"(async () => {
  const until = async (test, ms) => { const end = performance.now() + ms; while (performance.now() < end) { const v = await test(); if (v) return v; await new Promise((r) => setTimeout(r, 50)); } return test(); };
  const invoke = window.__TAURI_INTERNALS__.invoke;
  const report = { host: __HOST__, page: {} };
  try {
    const page = report.page;
    const banner = () => document.querySelector('[data-testid=update-countdown]');
    const staged = () => [...document.querySelectorAll('.status-update')].find((e) => e.querySelector('.link-button'));
    page.bannerShown = !!(await until(() => banner() && staged(), 10000));
    page.bannerText = (banner()?.querySelector('.banner-text')?.textContent ?? '').trim();
    page.bannerButtons = [...(banner()?.querySelectorAll('button') ?? [])].map((b) => b.textContent.trim());
    page.stagedText = (staged()?.childNodes[0]?.textContent ?? '').trim();
    page.restartLink = (staged()?.querySelector('.link-button')?.textContent ?? '').trim();
    banner()?.querySelectorAll('button')[0]?.click();
    page.bannerHidden = !!(await until(() => !banner(), 10000));
    page.stagedAfterLater = !!staged();
    // The host sees the snooze (it polls every 50 ms) before "restart now" clears it.
    await new Promise((r) => setTimeout(r, 1000));
    staged()?.querySelector('.link-button')?.click();
    report.after = (await until(() => window.__mightyAppUpdateHost, 25000)) ?? null;
    page.restored = !!(await until(() => !banner() && !staged(), 10000));
  } catch (error) {
    report.error = String(error);
  }
  await invoke('smoke_probe', { probe: report });
})();"#;

/// The box as Settings shows it, on a surface of the step's own.
struct UpdateBox(Arc<Host>);

impl SurfaceModel for UpdateBox {
    fn view(&mut self, ui: &mut Ui) -> View {
        let body = crate::settings::app_update::section(&self.0, ui);
        View::Settings {
            title: String::new(),
            close: String::new(),
            nav: vec![],
            selected: "about".into(),
            sections: vec![Section { title: None, beta: false, node: Some(ui.section_body(body)) }],
            width: 1.0,
            height: 1.0,
        }
    }
}

/// Every object in `value`, depth first.
fn objects<'a>(value: &'a Value, out: &mut Vec<&'a Value>) {
    match value {
        Value::Object(map) => {
            out.push(value);
            map.values().for_each(|v| objects(v, out));
        }
        Value::Array(items) => items.iter().for_each(|v| objects(v, out)),
        _ => {}
    }
}

/// The step's eyes on [`UpdateBox`]: the box read as the page would get it.
struct Eyes {
    surfaces: Surfaces,
    env: mighty_native::ui::Env,
}

impl Eyes {
    const VIEW: &'static str = "smoke-app-update";

    fn new(host: &Arc<Host>) -> Eyes {
        let surfaces = Surfaces::new();
        surfaces.open(Self::VIEW, Place::Surface, Box::new(UpdateBox(host.clone())));
        Eyes { surfaces, env: crate::native::env(host) }
    }

    fn read(&self) -> Value {
        self.surfaces.forget_sent();
        self.surfaces.render(Self::VIEW, &self.env).and_then(|push| serde_json::from_str(&push).ok()).unwrap_or(Value::Null)
    }

    fn node(&self, aid: &str) -> Option<Value> {
        let read = self.read();
        let mut all = vec![];
        objects(&read, &mut all);
        all.into_iter().find(|n| n.get("aid").and_then(Value::as_str) == Some(aid)).cloned()
    }

    /// The words shown under `aid` ("" when it is not shown): its texts and its buttons' labels.
    fn words(&self, aid: &str) -> String {
        let Some(node) = self.node(aid) else { return String::new() };
        let mut all = vec![];
        objects(&node, &mut all);
        // A text's words, a button's label.
        all.iter().filter_map(|n| n.get("text").or_else(|| n.get("label")).and_then(Value::as_str)).collect::<Vec<_>>().join(" ")
    }

    fn enabled(&self, aid: &str) -> bool {
        self.node(aid).is_some_and(|node| node.get("disabled") != Some(&Value::Bool(true)))
    }

    /// Every text the box shows.
    fn texts(&self) -> Vec<String> {
        let read = self.read();
        let mut all = vec![];
        objects(&read, &mut all);
        all.iter().filter_map(|n| n.get("text").and_then(Value::as_str).map(str::to_string)).collect()
    }
}

/// Starts the step: its host half now, then the page's probe; it reports once through the probe.
pub fn start(host: &Arc<Host>) -> bool {
    if host.smoke.is_none() {
        return false;
    }
    let Some(update) = crate::app_update::of(host) else {
        host.log.line("smoke: no app update step (the app update was not started)");
        return false;
    };
    host.log.line("smoke: app update steps");
    let host = host.clone();
    tauri::async_runtime::spawn(async move {
        let report = host_half(&host, &update);
        let page = page_phase(&host, &update);
        if let Some(smoke) = &host.smoke {
            smoke.eval(&host, &PROBE.replace("__HOST__", &report.to_string()));
        }
        let after = page.await;
        let script = format!("window.__mightyAppUpdateHost = {after};");
        if let Some(page) = tauri::Manager::get_webview(&host.app, crate::host::WINDOW) {
            let _ = page.eval(&script);
        }
    });
    true
}

/// `RunAppUpdateSectionSmoke` and `RunAppUpdateStagedSmoke`'s host side.
fn host_half(host: &Arc<Host>, update: &Arc<AppUpdate>) -> Value {
    let locale = host.locale;
    let core = &update.core;
    let eyes = Eyes::new(host);
    // Windows rule 1 first, on the real box: a build with no public key shows the one sentence with the button disabled.
    let has_key = core.coordinator.has_public_key();
    let notice = mighty_core::app_update_view::text_in(&locale, crate::app_update::words(), "NoPublicKeyNotice");
    let refused_shown = has_key || (eyes.words(STATUS_ID) == notice && !eyes.enabled(BUTTON_ID));
    let refused = mighty_core::app_update_view::describe(
        &locale,
        crate::app_update::words(),
        &Default::default(),
        false,
        None,
        |_| "00:00".into(),
        true,
    );
    let refused_without_key = !refused.section_enabled && !refused.button_enabled && refused.status_text == notice && refused_shown;

    let before = core.coordinator.state();
    let (mut statuses, mut buttons) = (vec![], vec![]);
    core.set_smoke_signed_build(true);
    for fixture in smoke::fixture_states(&locale) {
        core.coordinator.show_for_smoke(fixture);
        statuses.push(eyes.words(STATUS_ID));
        buttons.push(eyes.words(BUTTON_ID));
    }
    // The switch is named for its label, and the label stands beside it (read while the box draws as a signed build: a key-less one
    // hides the row, which the page then never gets; C# reads its collapsed control).
    let label = text(&locale, "AutoUpdateToggle");
    let named = eyes.node(TOGGLE_ID).is_some_and(|node| node.get("name").and_then(Value::as_str) == Some(label.as_str()));
    let switch_named = named && eyes.texts().contains(&label);
    core.set_smoke_signed_build(false);
    core.coordinator.show_for_smoke(before.clone());

    let staged = staged_half(host, update, &before);

    let outcome =
        smoke::decide(&locale, &statuses, &buttons, refused_without_key, &|| host.view().snapshot.app_update_auto_check, &mut |on| {
            crate::app_update::write_automatic(host, on)
        });
    let mut section = match outcome {
        Ok(outcome) => serde_json::to_value(&outcome).unwrap_or(Value::Null),
        Err(error) => json!({"error": error, "statuses": statuses, "buttons": buttons}),
    };
    // The idle facts read once (no helper, no countdown): a composer draft holds the restart.
    section["draftHoldsRestart"] = json!(draft_holds_restart(host, update));
    section["switchNamed"] = json!(switch_named);
    json!({"section": section, "staged": staged})
}

/// `RunAppUpdateStagedSmoke`'s host side over the frame model (`AppUpdateFrame`, what the shell model sends).
fn staged_half(host: &Arc<Host>, update: &Arc<AppUpdate>, before: &mighty_updates::app_update::coordinator::UpdateState) -> Value {
    let (locale, core) = (host.locale, &update.core);
    let saved_automatic = host.view().snapshot.app_update_auto_check;
    let saved_restart = core.restart();
    let fixtures = smoke::fixture_states(&locale);
    let ready = fixtures.iter().find(|s| s.phase == Phase::Ready && s.last_error.is_none()).cloned().unwrap_or_default();
    let available = fixtures.iter().find(|s| s.phase == Phase::Available).cloned().unwrap_or_default();
    let version = ready.availability.as_ref().map(|a| a.manifest.version.clone()).unwrap_or_default();
    let message =
        |seconds: u32| locale.get_with("appUpdate.countdown.message", &[("version", version.as_str()), ("seconds", &seconds.to_string())]);
    if !saved_automatic {
        crate::app_update::write_automatic(host, true);
    }
    core.coordinator.show_for_smoke(ready);
    let frame = core.frame(&locale);
    let staged_words = locale.get_with("window.status.updateStaged", &[("version", version.as_str())]);
    let staged_shown = frame.staged.as_deref() == Some(staged_words.as_str()) && frame.update.is_none();
    core.set_restart(RestartState::CountingDown(chrono::Utc::now() + chrono::Duration::from_std(auto::COUNTDOWN).unwrap_or_default()));
    let countdown = core.frame(&locale).countdown.unwrap_or_default();
    let counting = countdown == message(60) || countdown == message(59);
    core.set_restart(RestartState::Idle);
    core.coordinator.show_for_smoke(available);
    let frame = core.frame(&locale);
    let badge = locale.get_with("window.status.updateBadge", &[("version", version.as_str())]);
    let hidden = frame.countdown.is_none() && frame.staged.is_none() && frame.update.as_deref() == Some(badge.as_str());
    core.set_restart(saved_restart);
    core.coordinator.show_for_smoke(before.clone());
    if !saved_automatic {
        crate::app_update::write_automatic(host, false);
    }
    // What the page must draw (its half, `appUpdateBanner`): the banner's words a few seconds into the countdown at most, its two
    // buttons, the staged words and the status bar's "restart now".
    let banner: Vec<String> = (40..=60).map(message).collect();
    json!({
        "stagedShown": staged_shown,
        "countdown": countdown,
        "countingDown": counting,
        "badgeWithoutCountdown": hidden,
        "expected": {
            "banner": banner,
            "buttons": [locale.get("appUpdate.countdown.later"), locale.get("appUpdate.countdown.restartNow")],
            "staged": staged_words,
            "restartLink": locale.get("window.status.updateRestartNow"),
        },
    })
}

/// `SmokeDraftHoldsRestart`: words in a pane's composer hold the automatic restart; the composer's text put back unsaved.
fn draft_holds_restart(host: &Arc<Host>, update: &Arc<AppUpdate>) -> bool {
    let Some(id) = host.view().panes.keys().next().cloned() else { return false };
    let was = host.view().panes.get(&id).map(|ui| ui.view.draft.clone()).unwrap_or_default();
    if let Some(ui) = host.view().panes.get_mut(&id) {
        ui.view.draft = "draft holds the restart".into();
    }
    let holds = update.core.idle_blockers().contains(&IdleBlocker::Draft);
    if let Some(ui) = host.view().panes.get_mut(&id) {
        ui.view.draft = was;
    }
    holds
}

/// The page's phase: a ready update with the switch on and a countdown running for the probe to see; then (waited for in turn) the
/// probe's "later" snoozing it and its "restart now" reaching the coordinator, and everything put back. Answers what the host saw.
fn page_phase(host: &Arc<Host>, update: &Arc<AppUpdate>) -> impl std::future::Future<Output = Value> + use<> {
    let (host, update) = (host.clone(), update.clone());
    let locale = host.locale;
    let core = update.core.clone();
    let before = core.coordinator.state();
    let saved_restart = core.restart();
    let saved_automatic = host.view().snapshot.app_update_auto_check;
    let fixtures = smoke::fixture_states(&locale);
    let ready = fixtures.into_iter().find(|s| s.phase == Phase::Ready && s.last_error.is_none()).unwrap_or_default();
    if !saved_automatic {
        crate::app_update::write_automatic(&host, true);
    }
    core.coordinator.show_for_smoke(ready);
    core.set_restart(RestartState::CountingDown(chrono::Utc::now() + chrono::Duration::from_std(auto::COUNTDOWN).unwrap_or_default()));
    async move {
        let postponed = until(|| matches!(core.restart(), RestartState::Snoozed(_)), 20_000).await;
        let failed = until(|| core.coordinator.state().phase == Phase::Failed, 20_000).await;
        let failure = core.coordinator.state().error_message.unwrap_or_default();
        core.set_restart(saved_restart);
        core.coordinator.show_for_smoke(before);
        if !saved_automatic {
            crate::app_update::write_automatic(&host, false);
        }
        json!({
            "postponed": postponed,
            "restartReached": failed && failure == locale.get("appUpdate.error.noInstallPlan"),
            "failure": failure,
        })
    }
}

/// Polls `test` every 50 ms for up to `milliseconds`.
async fn until(mut test: impl FnMut() -> bool, milliseconds: u64) -> bool {
    for _ in 0..milliseconds / 50 {
        if test() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    test()
}

/// The step's checks from its report.
pub fn checks(report: &Value) -> Vec<(&'static str, bool)> {
    let at = |pointer: &str| report.pointer(pointer).cloned().unwrap_or(Value::Null);
    let yes = |pointer: &str| at(pointer) == Value::Bool(true);
    let section = at("/host/section");
    let fixtures = smoke::fixture_states(&mighty_core::locale::Locale::new("en")).len();
    let listed = |key: &str| section.get(key).and_then(Value::as_array).is_some_and(|all| all.len() == fixtures);
    vec![
        (
            smoke::RESULT_KEY,
            report.get("error").is_none()
                && section.get("error").is_none()
                && listed("statuses")
                && listed("buttons")
                && yes("/host/section/refusedWithoutPublicKey")
                && yes("/host/section/restored")
                && yes("/host/section/draftHoldsRestart")
                && yes("/host/section/switchNamed"),
        ),
        (
            "appUpdateStaged",
            yes("/host/staged/stagedShown") && yes("/host/staged/countingDown") && yes("/host/staged/badgeWithoutCountdown"),
        ),
        (
            "appUpdateBanner",
            yes("/page/bannerShown")
                && at("/host/staged/expected/banner").as_array().is_some_and(|all| all.contains(&at("/page/bannerText")))
                && at("/page/bannerButtons") == at("/host/staged/expected/buttons")
                && at("/page/stagedText") == at("/host/staged/expected/staged")
                && at("/page/restartLink") == at("/host/staged/expected/restartLink")
                && yes("/page/bannerHidden")
                && yes("/page/stagedAfterLater")
                && yes("/after/postponed")
                && yes("/after/restartReached")
                && yes("/page/restored"),
        ),
    ]
}
