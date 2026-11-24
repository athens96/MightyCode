//! MC-34: the app's side of the OS sandbox (mighty-run's `sandbox`): which runs are sandboxed (a local workspace with it turned on,
//! on the Mac and, since MC-42, on Windows), where the run's profile is written (`<data>/sandbox`, 0700, each profile 0600 and gone
//! with its run), what was refused (kept per pane for the settings box and the phone), and the reviewer: the `/advisor` plumbing's
//! read-only run of a cheap model, its answer read by `reviewer::parse` (the run engine decides what may follow from it).
//!
//! MC-42, Windows: turning the sandbox on lists the changes to the person's folders first (labels and access-list entries,
//! `sandbox::windows::plan`); only `sandboxSave` with `approve` makes them, and they are kept with what they replaced in
//! `<data>/sandbox/windows-changes.json` ([`windows::Record`]). Turning it off undoes the workspace's; `sandboxUndoAll` undoes every
//! one (before the app is removed). A run checks its plan against the approval and makes what is within it ([`SandboxHost::windows_ready`]).
//!
//! The page's methods: `sandboxGet` (a workspace's settings and what this computer supports), `sandboxSave`, `sandboxUndoAll`. The
//! network card's 항상 is `permissionAnswer` with `always` (cards.rs → [`allow_always`]).

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, LazyLock, Mutex, Weak};
use std::time::Duration;

use mighty_core::model::{SandboxConfig, Workspace};
use mighty_platform::BoxFuture;
use mighty_run::model::StartRunRequest;
use mighty_run::sandbox::hosts::{HostRule, Target};
use mighty_run::sandbox::{self, Block, SandboxHost, SandboxSetup, WindowsSetup, profile, reviewer, windows};
use serde_json::{Value, json};

use crate::host::{Host, Registry};
use crate::methods::{flag, text};

/// The most refusals kept per pane.
const BLOCKS_KEPT: usize = 20;
/// The agents a reviewer may be (read-only one-shot runs: advisor.rs).
pub const REVIEWERS: [&str; 2] = ["claude", "codex"];

static BLOCKS: LazyLock<Mutex<HashMap<String, VecDeque<Block>>>> = LazyLock::new(Default::default);
/// MC-42: one change of the Windows record at a time (runs start together; the settings box approves and undoes).
static RECORD: Mutex<()> = Mutex::new(());

fn blocks() -> std::sync::MutexGuard<'static, HashMap<String, VecDeque<Block>>> {
    BLOCKS.lock().unwrap_or_else(|p| p.into_inner())
}

/// What the sandbox refused in pane `pane_id`, oldest first.
pub fn blocked_in(pane_id: &str) -> Vec<Block> {
    blocks().get(pane_id).map(|list| list.iter().cloned().collect()).unwrap_or_default()
}

/// The model a reviewer runs when none is named: Claude's light one; the others' default.
pub fn reviewer_model(config: &SandboxConfig) -> Option<String> {
    let agent = config.reviewer_agent.as_deref().unwrap_or("claude");
    config.reviewer_model.clone().filter(|m| !m.trim().is_empty()).or_else(|| (agent == "claude").then(|| "haiku".to_string()))
}

struct AppSandbox {
    host: Weak<Host>,
}

impl SandboxHost for AppSandbox {
    fn setup(&self, workspace: &Workspace, _: &StartRunRequest) -> Option<SandboxSetup> {
        // Off the Mac the setup still goes back: the run engine refuses a sandboxed run it cannot sandbox (never runs it unsandboxed).
        let config = workspace.sandbox_on()?.clone();
        let host = self.host.upgrade()?;
        let home = std::env::home_dir()?;
        let windows = cfg!(windows).then(|| WindowsSetup {
            workspace_id: workspace.id().to_string(),
            workspace_folder: workspace.path.clone().map(std::path::PathBuf::from).unwrap_or_default(),
            private_root: private_root(&home),
        });
        Some(SandboxSetup { config, home, data_dir: Some(host.data_dir.clone()), profile_dir: host.data_dir.join("sandbox"), windows })
    }

    fn windows_ready(&self, workspace_id: &str, plan: &windows::Plan) -> Result<(), String> {
        let host = self.host.upgrade().ok_or_else(|| "the app is closing".to_string())?;
        let _held = RECORD.lock().unwrap_or_else(|p| p.into_inner());
        let mut record = load_record(&host);
        let result = record.ensure(workspace_id, plan, crate::platform::sandbox_acl().as_ref(), &mighty_core::time::now_o());
        save_record(&host, &record);
        match result {
            Ok(outcome) => {
                if outcome.applied > 0 {
                    host.log.line(&format!("sandbox: {} approved change(s) made for a run of {workspace_id}", outcome.applied));
                }
                Ok(())
            }
            Err(refusal) => Err(refusal_text(&host.locale, &refusal)),
        }
    }

    fn blocked(&self, session_id: &str, block: &Block) {
        {
            let mut all = blocks();
            let list = all.entry(session_id.to_string()).or_default();
            list.push_back(block.clone());
            while list.len() > BLOCKS_KEPT {
                list.pop_front();
            }
        }
        if let Some(host) = self.host.upgrade() {
            host.log.line(&format!("sandbox: blocked {} ({}) in {session_id}", block.what, block.kind));
        }
    }

    fn review(&self, config: &SandboxConfig, review: reviewer::Review) -> Option<BoxFuture<'static, Result<reviewer::Opinion, String>>> {
        let host = self.host.upgrade()?;
        let agent = config.reviewer_agent.clone().filter(|a| REVIEWERS.contains(&a.as_str())).unwrap_or_else(|| "claude".into());
        let model = reviewer_model(config);
        Some(Box::pin(async move {
            let prompt = reviewer::prompt(&review);
            let folder = review.folder.to_string_lossy().into_owned();
            // The run engine bounds the wait; this only keeps a stuck CLI from living on.
            let limit = reviewer::TIME_LIMIT + Duration::from_secs(5);
            let answer = crate::advisor::ask_within(&host, &agent, model.as_deref(), &prompt, &folder, limit).await?;
            let workspace = host.view().snapshot.session(&review.pane).map(|p| p.workspace_id().to_string());
            if let Some(workspace) = workspace {
                let shown = answer.model.clone().or(model).unwrap_or_else(|| "default".into());
                crate::usage_report::record_advisor(&host, &workspace, &agent, &shown, answer.cost_usd, answer.tokens);
            }
            reviewer::parse(&answer.body)
        }))
    }
}

/// The run engine's sandbox host (runs.rs). Profiles a crashed app left behind go now.
pub fn host_hooks(host: &Arc<Host>) -> Arc<dyn SandboxHost> {
    if let Ok(entries) = std::fs::read_dir(host.data_dir.join("sandbox")) {
        for entry in entries.flatten() {
            if entry.path().extension().is_some_and(|e| e == "sb") {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    // The agents' pipe follows the approvals from the start (a machine that never approved the sandbox keeps it owner-only).
    let _ = load_record(host);
    Arc::new(AppSandbox { host: Arc::downgrade(host) })
}

/// MC-48: the same sandbox host for a verification's commands, without the start's clean-up (a running run's profile stays).
pub fn verify_hooks(host: &Arc<Host>) -> Arc<dyn SandboxHost> {
    Arc::new(AppSandbox { host: Arc::downgrade(host) })
}

/// MC-42: `%USERPROFILE%\\AppData\\LocalLow\\MightyCode\\sandbox`: a folder Windows labels low already, where each workspace's runs keep
/// their temporary files and package caches.
pub fn private_root(home: &std::path::Path) -> std::path::PathBuf {
    windows::under(home, "AppData/LocalLow/MightyCode/sandbox")
}

fn record_path(host: &Host) -> std::path::PathBuf {
    host.data_dir.join("sandbox").join("windows-changes.json")
}

/// The Windows sandbox's record of every change it made (empty when there is none).
fn load_record(host: &Host) -> windows::Record {
    let record = std::fs::read_to_string(record_path(host)).map(|t| windows::Record::parse(&t)).unwrap_or_default();
    agent_pipe_follows(&record);
    record
}

/// The agents' pipe lets sandboxed runs in only while some workspace's Windows sandbox is approved (owner-only otherwise).
fn agent_pipe_follows(record: &windows::Record) {
    crate::platform::agent_pipe_for_sandbox(!record.approvals.is_empty());
}

/// Writes the record through a file beside it (a crash never leaves half of it).
fn save_record(host: &Host, record: &windows::Record) {
    agent_pipe_follows(record);
    let path = record_path(host);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let temp = path.with_extension("json.tmp");
    if std::fs::write(&temp, record.text()).and_then(|()| std::fs::rename(&temp, &path)).is_err() {
        host.log.line("sandbox: the Windows change record could not be written");
    }
}

fn refusal_text(locale: &mighty_core::locale::Locale, refusal: &windows::Refusal) -> String {
    match refusal {
        windows::Refusal::NotApproved(paths) => {
            let shown: Vec<String> = paths.iter().take(5).map(|p| p.display().to_string()).collect();
            locale.get_with("sandbox.windows.notApproved", &[("paths", &shown.join(", "))])
        }
        windows::Refusal::Failed(path, reason) => {
            locale.get_with("sandbox.windows.changeFailed", &[("path", &path.display().to_string()), ("reason", reason)])
        }
    }
}

/// The changes turning the sandbox on for `ws` with `config` makes (every agent's folders listed; the workspace folder is the run's).
fn approval_plan(host: &Host, ws: &Workspace, config: &SandboxConfig) -> Result<windows::Plan, String> {
    let home = std::env::home_dir().ok_or_else(|| "no home folder".to_string())?;
    let folder = std::path::PathBuf::from(ws.path.clone().unwrap_or_default());
    let input = windows::PlanInput {
        workspace_id: ws.id().to_string(),
        provider: None,
        home: home.clone(),
        workspace_folder: folder.clone(),
        run_folder: folder,
        data_dir: Some(host.data_dir.clone()),
        private_dir: private_root(&home).join(windows::workspace_key(ws.id())),
        state_overrides: vec![],
        protect_instructions: config.protect_instructions_on(),
        read_only: config.read_only_paths.clone(),
    };
    windows::plan(&input).map_err(|e| host.locale.get_with("sandbox.error.prepare", &[("reason", &e.to_string())]))
}

/// The Windows part of the settings box: the approval, and the changes listed for it (each with whether it is made).
fn windows_status(host: &Host, ws: &Workspace, config: &SandboxConfig) -> Value {
    if !cfg!(windows) || ws.is_ssh() || ws.remote.is_some() {
        return Value::Null;
    }
    let record = load_record(host);
    let approval = record.approvals.get(ws.id());
    let (changes, error) = match approval_plan(host, ws, config) {
        Ok(plan) => {
            let pending: Vec<String> = record.pending(ws.id(), &plan).iter().map(|c| c.id()).collect();
            (plan.changes.iter().map(|c| change_json(c, !pending.contains(&c.id()))).collect::<Vec<_>>(), None)
        }
        Err(error) => (vec![], Some(error)),
    };
    json!({
        "approved": approval.is_some(),
        "approvedAt": approval.map(|a| a.at.clone()),
        "changes": changes,
        "error": error,
        "recorded": record.applied.len(),
    })
}

fn change_json(change: &windows::Change, made: bool) -> Value {
    json!({
        "path": change.path.display().to_string(),
        "kind": change.kind,
        "op": change.op,
        "agent": change.agent,
        "made": made,
        "exists": change.path.exists(),
    })
}

/// The network card's 항상: the connection it asks about is added to its pane's workspace's allow list (before the run's own allow).
pub fn allow_always(host: &Arc<Host>, pane_id: &str, target: &Target) {
    let rule = HostRule { host: target.host.clone(), wildcard: false, port: target.port }.text();
    let workspace = host.view().snapshot.session(pane_id).map(|p| p.workspace_id().to_string());
    let Some(workspace) = workspace else { return };
    let _ = host.view().update(|state| {
        if let Some(ws) = state.workspaces_mut().iter_mut().find(|w| w.id() == workspace) {
            let config = ws.sandbox.get_or_insert_with(SandboxConfig::default);
            if !config.allowed_hosts.iter().any(|h| HostRule::parse(h).is_some_and(|r| r.matches(target))) {
                config.allowed_hosts.push(rule.clone());
            }
        }
    });
    host.schedule();
}

fn workspace(host: &Host, id: Option<&str>) -> Result<Workspace, String> {
    let view = host.view();
    id.filter(|id| !id.is_empty())
        .or(view.snapshot.active_workspace_id.as_deref())
        .and_then(|id| view.snapshot.workspace(id))
        .cloned()
        .ok_or_else(|| host.locale.get("window.error.addWorkspaceFirst"))
}

fn lines(args: &Value, name: &str) -> Vec<String> {
    let items: Vec<String> = match args.get(name) {
        Some(Value::Array(items)) => items.iter().filter_map(Value::as_str).map(str::to_string).collect(),
        Some(Value::String(text)) => text.lines().map(str::to_string).collect(),
        _ => vec![],
    };
    let mut out: Vec<String> = Vec::new();
    for item in items.iter().map(|i| i.trim()).filter(|i| !i.is_empty()) {
        if !out.iter().any(|o| o == item) {
            out.push(item.to_string());
        }
    }
    out
}

/// A workspace's sandbox as the settings box shows it.
fn status(host: &Arc<Host>, ws: &Workspace) -> Value {
    let config = ws.sandbox.clone().unwrap_or_default();
    let panes: Vec<String> =
        host.view().snapshot.sessions().iter().filter(|p| p.workspace_id() == ws.id()).map(|p| p.id().to_string()).collect();
    let mut blocked: Vec<Value> = Vec::new();
    for pane in &panes {
        for block in blocked_in(pane) {
            blocked.push(json!({ "pane": pane, "kind": block.kind, "what": block.what, "reason": block.reason, "at": block.at }));
        }
    }
    blocked.sort_by(|a, b| b["at"].as_str().cmp(&a["at"].as_str()));
    blocked.truncate(BLOCKS_KEPT);
    json!({
        "workspaceId": ws.id(),
        "name": ws.name(),
        "platform": if cfg!(windows) { "windows" } else { "mac" },
        "supported": sandbox::supported(),
        "available": sandbox::available(),
        "experimental": sandbox::experimental(),
        "guarantees": windows::guarantees(cfg!(windows)),
        "windows": windows_status(host, ws, &config),
        "ssh": ws.is_ssh(),
        "remote": ws.remote.is_some(),
        "enabled": config.enabled,
        "limitNetwork": config.limit_network_on(),
        "allowedHosts": config.allowed_hosts,
        "protectInstructions": config.protect_instructions_on(),
        "readOnlyPaths": config.read_only_paths,
        "reviewer": config.reviewer_on(),
        "reviewerAgent": config.reviewer_agent.clone().unwrap_or_else(|| "claude".into()),
        "reviewerModel": config.reviewer_model.clone().unwrap_or_default(),
        "reviewerAgents": REVIEWERS,
        "blocked": blocked,
    })
}

/// The settings `args` give, checked: every host an allow-list entry, every path one inside the workspace. Err with the sentence.
pub fn config_from(host_locale: &mighty_core::locale::Locale, args: &Value, ws: &Workspace) -> Result<SandboxConfig, String> {
    let hosts = lines(args, "allowedHosts");
    let bad: Vec<&str> = hosts.iter().filter(|h| HostRule::parse(h).is_none()).map(String::as_str).collect();
    if !bad.is_empty() {
        return Err(host_locale.get_with("sandbox.settings.invalidHosts", &[("hosts", &bad.join(", "))]));
    }
    let paths = lines(args, "readOnlyPaths");
    let bad: Vec<&str> = paths.iter().filter(|p| !profile::relative_ok(p)).map(String::as_str).collect();
    if !bad.is_empty() {
        return Err(host_locale.get_with("sandbox.settings.invalidPaths", &[("paths", &bad.join(", "))]));
    }
    let enabled = flag(args, "enabled");
    if enabled && !sandbox::supported() {
        return Err(host_locale.get("sandbox.settings.unsupported"));
    }
    if enabled && ws.is_ssh() {
        return Err(host_locale.get("sandbox.settings.ssh"));
    }
    if enabled && ws.remote.is_some() {
        return Err(host_locale.get("sandbox.settings.remote"));
    }
    let agent = text(args, "reviewerAgent").filter(|a| REVIEWERS.contains(a)).unwrap_or("claude");
    let model = text(args, "reviewerModel").map(str::trim).filter(|m| !m.is_empty() && mighty_core::wire::model(m));
    Ok(SandboxConfig {
        enabled,
        limit_network: Some(flag(args, "limitNetwork")),
        allowed_hosts: hosts.iter().filter_map(|h| HostRule::parse(h).map(|r| r.text())).fold(Vec::new(), |mut kept, h| {
            if !kept.contains(&h) {
                kept.push(h);
            }
            kept
        }),
        protect_instructions: Some(flag(args, "protectInstructions")),
        read_only_paths: paths.iter().map(|p| p.trim().trim_matches('/').to_string()).collect(),
        reviewer: Some(flag(args, "reviewer")),
        reviewer_agent: Some(agent.to_string()),
        reviewer_model: model.map(str::to_string),
    })
}

pub fn register(methods: &mut Registry) {
    methods.add("sandboxGet", |host: Arc<Host>, args| async move {
        let ws = workspace(&host, text(&args, "workspaceId"))?;
        Ok(status(&host, &ws))
    });
    methods.add("sandboxSave", |host: Arc<Host>, args| async move {
        let ws = workspace(&host, text(&args, "workspaceId"))?;
        let config = config_from(&host.locale, &args, &ws)?;
        let id = ws.id().to_string();
        // MC-42: on Windows, turning on asks first (the changes listed) and makes them on `approve`; turning off undoes them.
        let mut report = Value::Null;
        if cfg!(windows) {
            let (on, approve) = (config.enabled, flag(&args, "approve"));
            let worker = host.clone();
            let ws_for = ws.clone();
            let config_for = config.clone();
            let answer = tauri::async_runtime::spawn_blocking(move || windows_turn(&worker, &ws_for, &config_for, on, approve))
                .await
                .map_err(|e| e.to_string())??;
            match answer {
                Turn::Ask => {
                    let mut shown = status(&host, &ws);
                    shown["needsApproval"] = json!(true);
                    return Ok(shown);
                }
                Turn::Done(value) => report = value,
            }
        }
        host.view().update(|state| {
            if let Some(ws) = state.workspaces_mut().iter_mut().find(|w| w.id() == id) {
                ws.sandbox = Some(config.clone());
            }
        })?;
        host.schedule();
        let ws = workspace(&host, Some(&id))?;
        let mut shown = status(&host, &ws);
        shown["report"] = report;
        Ok(shown)
    });
    // MC-42: every change the Windows sandbox made is undone and every workspace's sandbox turned off (before the app is removed).
    methods.add("sandboxUndoAll", |host: Arc<Host>, _args| async move {
        let worker = host.clone();
        let outcome = tauri::async_runtime::spawn_blocking(move || {
            let _held = RECORD.lock().unwrap_or_else(|p| p.into_inner());
            let mut record = load_record(&worker);
            let outcome = record.release_all(crate::platform::sandbox_acl().as_ref());
            save_record(&worker, &record);
            outcome
        })
        .await
        .map_err(|e| e.to_string())?;
        if cfg!(windows) {
            host.view().update(|state| {
                for ws in state.workspaces_mut().iter_mut() {
                    if let Some(config) = ws.sandbox.as_mut() {
                        config.enabled = false;
                    }
                }
            })?;
            host.schedule();
        }
        host.log.line(&format!("sandbox: undo all: {} undone, {} failed", outcome.undone, outcome.failures.len()));
        Ok(outcome_json(&outcome))
    });
}

/// What turning the Windows sandbox on or off for a workspace came to.
enum Turn {
    /// The changes are not approved yet: the box lists them and asks.
    Ask,
    /// Done (what was made or undone, for the box's line).
    Done(Value),
}

fn outcome_json(outcome: &windows::Outcome) -> Value {
    let failures: Vec<Value> =
        outcome.failures.iter().map(|(path, reason)| json!({ "path": path.display().to_string(), "reason": reason })).collect();
    json!({ "applied": outcome.applied, "undone": outcome.undone, "failures": failures })
}

/// MC-42: turning the Windows sandbox of `ws` on (with `approve`: the listed changes made and the approval kept) or off (its changes
/// undone). Err with the sentence: nothing is saved.
fn windows_turn(host: &Host, ws: &Workspace, config: &SandboxConfig, on: bool, approve: bool) -> Result<Turn, String> {
    let _held = RECORD.lock().unwrap_or_else(|p| p.into_inner());
    let mut record = load_record(host);
    let acl = crate::platform::sandbox_acl();
    if !on {
        if !record.approved(ws.id()) {
            return Ok(Turn::Done(Value::Null));
        }
        let outcome = record.release(ws.id(), acl.as_ref());
        save_record(host, &record);
        host.log.line(&format!("sandbox: turned off for {}: {} change(s) undone", ws.id(), outcome.undone));
        return Ok(Turn::Done(outcome_json(&outcome)));
    }
    let plan = approval_plan(host, ws, config)?;
    if record.approved(ws.id()) && record.covers(ws.id(), &plan) {
        return Ok(Turn::Done(Value::Null));
    }
    if !approve {
        return Ok(Turn::Ask);
    }
    let result = record.approve(ws.id(), &plan, acl.as_ref(), &mighty_core::time::now_o());
    save_record(host, &record);
    match result {
        Ok(outcome) => {
            host.log.line(&format!("sandbox: approved for {}: {} change(s) made", ws.id(), outcome.applied));
            Ok(Turn::Done(outcome_json(&outcome)))
        }
        Err(refusal) => Err(refusal_text(&host.locale, &refusal)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ws() -> Workspace {
        Workspace { path: Some("/w".into()), ..Workspace::default() }
    }

    #[test]
    fn settings_are_checked_before_they_are_kept() {
        let l = mighty_core::locale::Locale::new("ko");
        let args = json!({
            "enabled": sandbox::supported(), "limitNetwork": true, "allowedHosts": "GitHub.com\n\n registry.npmjs.org \nexample.com:8080\ngithub.com",
            "protectInstructions": true, "readOnlyPaths": ["config/prod.yaml", "secrets/"], "reviewer": true, "reviewerAgent": "opencode", "reviewerModel": " ",
        });
        let config = config_from(&l, &args, &ws()).unwrap();
        assert_eq!(config.allowed_hosts, ["github.com", "registry.npmjs.org", "example.com:8080"]);
        assert_eq!(config.read_only_paths, ["config/prod.yaml", "secrets"]);
        assert_eq!(config.reviewer_agent.as_deref(), Some("claude"), "only a read-only reviewer");
        assert_eq!(config.reviewer_model, None);
        assert_eq!(reviewer_model(&config).as_deref(), Some("haiku"));
        let bad = config_from(&l, &json!({ "allowedHosts": ["ok.com", "not a host"] }), &ws()).unwrap_err();
        assert!(bad.contains("not a host"));
        let bad = config_from(&l, &json!({ "readOnlyPaths": ["../up"] }), &ws()).unwrap_err();
        assert!(bad.contains("../up"));
        let ssh = Workspace { ssh: Some(Default::default()), ..ws() };
        assert!(config_from(&l, &json!({ "enabled": true }), &ssh).is_err(), "never on an SSH workspace");
        assert!(sandbox::supported(), "the Mac and Windows both have a sandbox");
        assert!(config_from(&l, &json!({ "enabled": true }), &ws()).is_ok());
        assert_eq!(
            private_root(std::path::Path::new("/h")),
            windows::under(std::path::Path::new("/h"), "AppData/LocalLow/MightyCode/sandbox")
        );
    }
}
