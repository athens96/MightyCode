//! MC-34: the app's side of the OS sandbox (mighty-run's `sandbox`): which runs are sandboxed (a local workspace with it turned on,
//! on the Mac), where the run's profile is written (`<data>/sandbox`, 0700, each profile 0600 and gone with its run), what was
//! refused (kept per pane for the settings box and the phone), and the reviewer: the `/advisor` plumbing's read-only run of a cheap
//! model, its answer read by `reviewer::parse` (the run engine decides what may follow from it).
//!
//! The page's methods: `sandboxGet` (a workspace's settings and what this computer supports) and `sandboxSave`. The network card's
//! 항상 is `permissionAnswer` with `always` (cards.rs → [`allow_always`]).

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, LazyLock, Mutex, Weak};
use std::time::Duration;

use mighty_core::model::{SandboxConfig, Workspace};
use mighty_platform::BoxFuture;
use mighty_run::model::StartRunRequest;
use mighty_run::sandbox::hosts::{HostRule, Target};
use mighty_run::sandbox::{self, Block, SandboxHost, SandboxSetup, profile, reviewer};
use serde_json::{Value, json};

use crate::host::{Host, Registry};
use crate::methods::{flag, text};

/// The most refusals kept per pane.
const BLOCKS_KEPT: usize = 20;
/// The agents a reviewer may be (read-only one-shot runs: advisor.rs).
pub const REVIEWERS: [&str; 2] = ["claude", "codex"];

static BLOCKS: LazyLock<Mutex<HashMap<String, VecDeque<Block>>>> = LazyLock::new(Default::default);

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
        Some(SandboxSetup {
            config,
            home: std::env::home_dir()?,
            data_dir: Some(host.data_dir.clone()),
            profile_dir: host.data_dir.join("sandbox"),
        })
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
    Arc::new(AppSandbox { host: Arc::downgrade(host) })
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
        "supported": sandbox::supported(),
        "available": sandbox::available(),
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
        return Err(host_locale.get("sandbox.settings.windows"));
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
        host.view().update(|state| {
            if let Some(ws) = state.workspaces_mut().iter_mut().find(|w| w.id() == id) {
                ws.sandbox = Some(config.clone());
            }
        })?;
        host.schedule();
        let ws = workspace(&host, Some(&id))?;
        Ok(status(&host, &ws))
    });
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
        if !sandbox::supported() {
            assert_eq!(config_from(&l, &json!({ "enabled": true }), &ws()).unwrap_err(), l.get("sandbox.settings.windows"));
        }
    }
}
