//! The remote step's phone part (W8 plan D16), last of `Stage::Remote`: the m1 router in-process, from the smoke's own device, as a
//! phone's connection hands it requests (the relay and its handshake are W8-A's, checked over its in-process fake relay in mighty-remote's
//! tests; a smoke never reaches a relay). Its report is the stage report's `phone`, under these keys: `info` (`/m1/info` names this host,
//! its platform and capabilities, never screen share), `state` (the list with this profile's local panes), `session` (a pane's detail
//! with the role view and the structure tag), `entries`, `files` (the workspace's listing), `unknownRoute` (404 in C#'s words),
//! `notified` (the router tells what moved, the list among it), `created` (a pane made from the phone), `submitted` (a request sent to
//! it started at once, with its entry id), `entries` (its words in the transcript the phone pages), `stopped` (the phone's stop answers
//! that it ran, and the run ends), `closed` (the phone's close, which never asks). Without the stand-in CLI (`-NoRuns`, a real Claude)
//! only the reading keys run (`runs: false`): no pane is made in the profile and no CLI starts. Owner: W8-B.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};

use crate::host::Host;

/// The keys of a smoke that reads only (no stand-in CLI): nothing is made in the profile and no CLI starts.
const READ_KEYS: [&str; 6] = ["info", "state", "session", "files", "unknownRoute", "notified"];

const KEYS: [&str; 11] =
    ["info", "state", "session", "entries", "files", "unknownRoute", "notified", "created", "submitted", "stopped", "closed"];

/// Starts the phone part; false when it does not run (then the smoke goes on to the next step). It reports once through
/// `Smoke::probed`.
pub fn start(host: &Arc<Host>) -> bool {
    if host.smoke.is_none() {
        return false;
    }
    host.log.line("smoke: remote step, phone part (the m1 router in-process, from the smoke's own device)");
    let host = host.clone();
    tauri::async_runtime::spawn(async move {
        let mut checks = Map::new();
        let outcome = run(&host, &mut checks).await;
        let mut report = Value::Object(checks);
        let keys: &[&str] = if report.get("runs") == Some(&Value::Bool(false)) { &READ_KEYS } else { &KEYS };
        report["passed"] = json!(outcome.is_ok() && keys.iter().all(|key| report.get(*key) == Some(&Value::Bool(true))));
        if let Err(error) = outcome {
            host.log.line(&format!("smoke: phone part failed: {error}"));
            report["error"] = json!(error);
        }
        if let Some(smoke) = &host.smoke {
            smoke.probed(&host, report);
        }
    });
    true
}

/// The phone part's checks from its report; each fails when the report is missing. A smoke without the stand-in CLI (`-NoRuns`, a real
/// Claude) reads only: its report says `runs: false` and has no pane made, sent to, stopped or closed.
pub fn checks(report: &Value) -> Vec<(&'static str, bool)> {
    let yes = |key: &str| report.get(key) == Some(&Value::Bool(true));
    if report.get("runs") == Some(&Value::Bool(false)) {
        return vec![
            ("phoneInfo", yes("info")),
            ("phoneState", yes("state")),
            ("phoneSession", yes("session")),
            ("phoneFiles", yes("files")),
            ("phoneUnknownRoute", yes("unknownRoute")),
            ("phoneNotified", yes("notified")),
            ("phonePassed", yes("passed") && report.get("error").is_none()),
        ];
    }
    vec![
        ("phoneInfo", yes("info")),
        ("phoneState", yes("state")),
        ("phoneSession", yes("session")),
        ("phoneEntries", yes("entries")),
        ("phoneFiles", yes("files")),
        ("phoneUnknownRoute", yes("unknownRoute")),
        ("phoneNotified", yes("notified")),
        ("phoneCreated", yes("created")),
        ("phoneSubmitted", yes("submitted")),
        ("phoneStopped", yes("stopped")),
        ("phoneClosed", yes("closed")),
        ("phonePassed", yes("passed") && report.get("error").is_none()),
    ]
}

fn require(holds: bool, why: impl Into<String>) -> Result<(), String> {
    if holds { Ok(()) } else { Err(why.into()) }
}

async fn get(host: &Arc<Host>, path: &str) -> Result<Value, String> {
    let reply = crate::phone::route(host, "GET", path, None).await;
    require(reply.status == 200, format!("{path} answered {} {}", reply.status, reply.body))?;
    serde_json::from_str(&reply.body).map_err(|e| format!("{path} is no JSON: {e}"))
}

async fn run(host: &Arc<Host>, checks: &mut Map<String, Value>) -> Result<(), String> {
    let info = get(host, "/m1/info").await?;
    let capabilities: Vec<&str> = info["capabilities"].as_array().map(|c| c.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    let platform = if cfg!(windows) { "win32" } else { "darwin" };
    checks.insert(
        "info".into(),
        json!(
            info["protocol"] == 1
                && info["platform"] == platform
                && info["appVersion"] == host.version.as_str()
                && capabilities == mighty_remote::phone::router::CAPABILITIES
                && !capabilities.contains(&"screenShare")
        ),
    );
    let (workspace, pane) = {
        let view = host.view();
        let workspace = view.snapshot.workspaces().iter().find(|w| w.remote.is_none()).map(|w| w.id().to_string());
        let pane = view
            .snapshot
            .sessions()
            .iter()
            .find(|p| p.kind() == "claude" && workspace.as_deref() == Some(p.workspace_id()))
            .map(|p| p.id().to_string());
        (workspace.ok_or("the smoke has no local workspace")?, pane)
    };
    let state = get(host, "/m1/state").await?;
    let listed: Vec<&str> = state["sessions"].as_array().map(|s| s.iter().filter_map(|p| p["id"].as_str()).collect()).unwrap_or_default();
    checks.insert(
        "state".into(),
        json!(
            state["revision"].as_i64().is_some_and(|r| r >= 1)
                && pane.as_deref().is_none_or(|p| listed.contains(&p))
                && state["workspaces"].is_array()
        ),
    );
    // Without the stand-in CLI (`-NoRuns`, a real Claude) the phone only reads: no pane is made in the profile and no CLI starts.
    let fake = host.smoke.as_ref().is_some_and(|smoke| smoke.fake_record().is_some());
    if !fake {
        checks.insert("runs".into(), json!(false));
        let session = match &pane {
            Some(id) => {
                let detail = get(host, &format!("/m1/sessions/{id}?since=0&wait=0&roles=1&structure=0")).await?;
                detail["session"]["id"] == id.as_str() && detail["session"]["styleId"] == "cli"
            }
            None => true,
        };
        checks.insert("session".into(), json!(session));
        let files = get(host, &format!("/m1/workspaces/{workspace}/files")).await?;
        checks.insert("files".into(), json!(files["workspaceId"] == workspace.as_str() && files["entries"].is_array()));
        let unknown = crate::phone::route(host, "GET", "/m1/nothing", None).await;
        checks.insert("unknownRoute".into(), json!(unknown.status == 404));
        let notified = crate::phone::take_notifications(host);
        checks.insert("notified".into(), json!(notified.iter().any(|(scope, _)| scope == "state")));
        return Ok(());
    }
    // A pane made from the phone: the create route, then its detail.
    let created = crate::phone::route(host, "POST", &format!("/m1/workspaces/{workspace}/sessions"), Some(json!({"kind": "claude"}))).await;
    let made: Value = serde_json::from_str(&created.body).unwrap_or(Value::Null);
    let id = made["sessionId"].as_str().map(str::to_string);
    checks
        .insert("created".into(), json!(created.status == 201 && id.as_ref().is_some_and(|id| host.view().snapshot.session(id).is_some())));
    let id = id.ok_or_else(|| format!("the create route answered {} {}", created.status, created.body))?;
    let detail = get(host, &format!("/m1/sessions/{id}?since=0&wait=0&roles=1&structure=0")).await?;
    checks.insert(
        "session".into(),
        json!(
            detail["session"]["id"] == id.as_str()
                && detail["mighty"]["runs"].is_array()
                && detail["session"]["styleId"] == "cli"
                && detail["settings"]["options"]["models"].is_array()
        ),
    );
    let files = get(host, &format!("/m1/workspaces/{workspace}/files")).await?;
    checks.insert("files".into(), json!(files["workspaceId"] == workspace.as_str() && files["entries"].is_array()));
    let unknown = crate::phone::route(host, "GET", "/m1/nothing", None).await;
    checks.insert(
        "unknownRoute".into(),
        json!(unknown.status == 404 && unknown.body == r#"{"protocol":1,"error":"Mobile route not found."}"#),
    );
    // The router tells what moved since it was made: the list among it (each reading tells `state` before its panes).
    let notified = crate::phone::take_notifications(host);
    checks.insert("notified".into(), json!(notified.iter().any(|(scope, _)| scope == "state")));
    // A request sent from the phone starts in the new pane (the smoke's fake Claude holds this one until it is stopped); its words are in
    // the transcript the phone pages; the phone's stop answers that it ran.
    let sent =
        crate::phone::route(host, "POST", &format!("/m1/sessions/{id}/submit"), Some(json!({"text": crate::smoke_runs::STOP_REQUEST})))
            .await;
    let answer: Value = serde_json::from_str(&sent.body).unwrap_or(Value::Null);
    let started = sent.status == 202 && answer["outcome"] == "started" && answer["entryId"].is_string();
    let running = |host: &Arc<Host>| host.runs().is_some_and(|r| r.service.is_session_running(&id));
    let deadline = Instant::now() + Duration::from_secs(30);
    while started && !running(host) && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let entries = get(host, &format!("/m1/sessions/{id}/entries?limit=50")).await?;
    let rows = entries["entries"].as_array().cloned().unwrap_or_default();
    checks.insert(
        "entries".into(),
        json!(entries["hasMore"] == false && rows.iter().any(|e| e["kind"] == "user" && e["text"] == crate::smoke_runs::STOP_REQUEST)),
    );
    let stopped = crate::phone::route(host, "POST", &format!("/m1/sessions/{id}/stop"), None).await;
    let deadline = Instant::now() + Duration::from_secs(30);
    while running(host) && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    checks.insert("submitted".into(), json!(started));
    checks.insert("stopped".into(), json!(stopped.status == 200 && stopped.body == r#"{"protocol":1,"stopped":true}"# && !running(host)));
    require(started, format!("the phone's submit answered {} {}", sent.status, sent.body))?;
    // The phone's close never asks; the pane is gone.
    let closed = crate::phone::route(host, "POST", &format!("/m1/sessions/{id}/close"), None).await;
    checks.insert("closed".into(), json!(closed.status == 200 && host.view().snapshot.session(&id).is_none()));
    Ok(())
}
