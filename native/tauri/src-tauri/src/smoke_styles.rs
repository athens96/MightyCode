//! The smoke's styles step (Rust-only; plan W9-D): the bundled styles registered (read, never written: a smoke saves no style file and
//! no approval), the approval card's sections, the bundled Ouroboros style chosen on a Claude pane in the Mighty view, its guide panel
//! drawn, `guidedAction{what:'action'}` sending the style's own command, a rescan, and the pane put back as it was. Owner: W9-D.

use std::sync::Arc;
use std::time::Duration;

use mighty_core::model::RunSession;
use serde_json::{Map, Value, json};

use crate::host::Host;

const KEYS: [&str; 7] = ["registered", "approvalCard", "styleChosen", "guidedPanel", "guidedAction", "rescanned", "restored"];

/// Starts the step after the settings step (smoke.rs `Stage::Styles`): true when it started; it reports once through `Smoke::probed`.
pub fn start(host: &Arc<Host>) -> bool {
    if host.smoke.is_none() {
        return false;
    }
    host.log.line("smoke: styles step (bundled styles, read only)");
    let host = host.clone();
    tauri::async_runtime::spawn(async move {
        let mut report = Map::new();
        if let Err(error) = run(&host, &mut report).await {
            host.log.line(&format!("smoke: styles step failed: {error}"));
            report.insert("error".into(), json!(error));
        }
        let passed = KEYS.iter().all(|k| report.get(*k) == Some(&Value::Bool(true)));
        report.insert("passed".into(), json!(passed));
        if let Some(smoke) = &host.smoke {
            smoke.probed(&host, Value::Object(report));
        }
    });
    true
}

async fn until(check: impl Fn() -> bool, seconds: u64) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_secs(seconds);
    while std::time::Instant::now() < deadline {
        if check() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    check()
}

async fn run(host: &Arc<Host>, report: &mut Map<String, Value>) -> Result<(), String> {
    let l = host.locale;
    let registry = crate::styles::settings_registry(host).await;
    let bundled: Vec<&str> = registry.styles.iter().filter(|s| s.source == "bundled" && s.runnable()).map(|s| s.id()).collect();
    report.insert("bundled".into(), json!(bundled));
    report.insert("registered".into(), json!(bundled.contains(&"ouroboros") && bundled.contains(&"claude-plan")));
    let ouroboros = registry.styles.iter().find(|s| s.id() == "ouroboros").cloned().ok_or("no bundled ouroboros")?;
    let sections: Vec<String> = mighty_styles::presentation::approval(&l, &ouroboros).into_iter().map(|s| s.id).collect();
    report
        .insert("approvalCard".into(), json!(sections.iter().take(4).map(String::as_str).eq(["origin", "autoAllow", "install", "enter"])));
    // A local Claude pane that is not running.
    let original: RunSession = {
        let view = host.view();
        let snapshot = &view.snapshot;
        snapshot
            .sessions()
            .iter()
            .find(|p| {
                p.kind() == "claude"
                    && p.provider() == "claude"
                    && p.status() != "running"
                    && crate::styles::local_workspace(snapshot, p).is_some()
            })
            .map(|p| (**p).clone())
            .ok_or("no local Claude pane")?
    };
    let id = original.id().to_string();
    {
        let mut view = host.view();
        crate::panes::ensure(&mut view, Some(&id))?;
        crate::composer::change(&mut view, &id, |p| RunSession {
            agent_view_mode: Some("mighty".into()),
            mighty_style: Some("ouroboros".into()),
            mighty_style_hash: None,
            mighty_style_since: None,
            ..p.clone()
        })?;
    }
    crate::styles::reload(host, &id, false);
    let chosen = until(|| crate::styles::loaded(&id).1.as_deref() == Some("ouroboros"), 15).await;
    report.insert("styleChosen".into(), json!(chosen));
    let guided = {
        let view = host.view();
        let pane = view.snapshot.session(&id).cloned().ok_or("pane gone")?;
        crate::styles::window(host, &view, &pane).1
    };
    let phases = guided.as_ref().and_then(|g| g.get("phases")).and_then(Value::as_array).map_or(0, Vec::len);
    let actions: Vec<String> = guided
        .as_ref()
        .and_then(|g| g.get("actions"))
        .and_then(Value::as_array)
        .map(|a| a.iter().filter(|x| x["enabled"] == json!(true)).filter_map(|x| x["id"].as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    report.insert("phases".into(), json!(phases));
    report.insert("actions".into(), json!(actions));
    report.insert("guidedPanel".into(), json!(phases == 6 && !actions.is_empty()));
    // The press sends a request: only to the stand-in CLI (a page smoke checks the press is answered without sending).
    let stand_in = host.smoke.as_ref().is_some_and(|s| s.fake_record().is_some());
    report.insert("standIn".into(), json!(stand_in));
    let before = host.view().snapshot.session(&id).map_or(0, |p| p.logs.as_ref().map_or(0, Vec::len));
    let action = actions.first().cloned().unwrap_or_default();
    let acted = if stand_in {
        crate::styles::guided_action(host, &id, "action", Some(&action)).await
    } else {
        crate::styles::guided_action(host, &id, "group", None).await
    };
    report.insert("guidedAction".into(), json!(acted.is_ok()));
    if let Err(error) = &acted {
        report.insert("guidedActionError".into(), json!(error));
    }
    // With the stand-in CLI the command reaches the pane's transcript as the style's own slash command.
    let sent = stand_in
        && until(
            || {
                host.view().snapshot.session(&id).is_some_and(|p| {
                    p.logs
                        .as_deref()
                        .unwrap_or(&[])
                        .iter()
                        .skip(before)
                        .any(|l| l.kind.as_deref() == Some("user") && l.text.as_deref().is_some_and(|t| t.starts_with("/ouroboros:")))
                })
            },
            10,
        )
        .await;
    report.insert("sentStyleCommand".into(), json!(sent));
    until(|| host.view().snapshot.session(&id).is_none_or(|p| p.status() != "running"), 30).await;
    crate::styles::guided_action(host, &id, "recheck", None).await?;
    let rescanned = until(|| crate::styles::loaded(&id).0 && crate::styles::loaded(&id).2 >= 2, 15).await;
    report.insert("rescanned".into(), json!(rescanned));
    {
        let mut view = host.view();
        crate::composer::change(&mut view, &id, |p| RunSession {
            agent_view_mode: original.agent_view_mode.clone(),
            mighty_style: original.mighty_style.clone(),
            mighty_style_hash: original.mighty_style_hash.clone(),
            mighty_style_since: original.mighty_style_since.clone(),
            ..p.clone()
        })?;
    }
    let restored = host
        .view()
        .snapshot
        .session(&id)
        .is_some_and(|p| p.mighty_style == original.mighty_style && p.agent_view_mode == original.agent_view_mode);
    report.insert("restored".into(), json!(restored));
    host.schedule();
    Ok(())
}

/// The step's checks from its report.
pub fn checks(report: &Value) -> Vec<(&'static str, bool)> {
    const NAMES: [&str; 7] = [
        "stylesRegistered",
        "stylesApprovalCard",
        "stylesChosen",
        "stylesGuidedPanel",
        "stylesGuidedAction",
        "stylesRescanned",
        "stylesRestored",
    ];
    NAMES.iter().zip(KEYS).map(|(name, key)| (*name, report.get(key) == Some(&Value::Bool(true)))).collect()
}
