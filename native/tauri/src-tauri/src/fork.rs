//! MC-28: a pane's conversation forked into a new pane in a new worktree (the pane menu's "새 worktree로 갈라내기", a result block's
//! "여기서 갈라내기"; the page's `paneFork`). The new pane is `fork-<short id>` (so its worktree folder and branch are
//! `mighty/fork-<short id>`, the pane worktree rules unchanged), made at the source pane's HEAD and merged where the source merges,
//! with the workspace's copy rules, `.mighty.yaml` setup and MC-25 port block as any worktree pane. The source's uncommitted changes
//! come along when the person leaves 커밋 안 된 변경 옮기기 on (`git stash create` there, `stash apply` here; a failure leaves the
//! source as it was and says why).
//!
//! The conversation: Claude and Codex's `exec` fork the source's saved session in the new pane's first run (`--fork-session`,
//! `exec fork`; mighty-run's `service::launch` and `providers::cli_fork`), only when the fork is at the last request (the CLI carries
//! the whole session). Otherwise, and as the fallback when the CLI cannot find the session, the first request carries a handoff
//! summary (mighty-run's `fork::handoff_summary`: the last request, the plan, the last result and the changed files, cut to 12,000
//! characters, secret-looking lines redacted). The pane records `forkedFrom { paneId, requestId }`, shown in the sidebar, over the pane
//! and in the phone's pane summary.

use std::path::Path;
use std::sync::Arc;

use mighty_core::model::{ForkedFrom, PaneWorktree, RunSession};
use mighty_core::wire;
use mighty_delegation::git::worktree::{ChildWorktreeCheck, ChildWorktreeOutcome};
use mighty_delegation::pane_worktree::{self as core, CarryOutcome};
use mighty_delegation::text;
use mighty_run::fork::{self as conversation, ForkPoint};
use serde_json::{Value, json};

use crate::frame::{self, act};
use crate::host::{Host, Registry};
use crate::methods::text as arg;
use crate::pane_worktree as worktrees;

pub fn register(methods: &mut Registry) {
    methods.add("paneFork", |host: Arc<Host>, args| async move {
        let Some(id) = arg(&args, "paneId").map(str::to_string) else { return Ok(Value::Null) };
        let request = arg(&args, "requestId").filter(|r| wire::identifier(r)).map(str::to_string);
        act(&host, async { fork(&host, &id, request.as_deref()).await }).await
    });
    // MC-28: an opinion card's 닫기 (advisor.rs).
    methods.add("opinionDismiss", |host: Arc<Host>, args| async move {
        if let (Some(pane), Some(card)) = (arg(&args, "paneId"), arg(&args, "cardId")) {
            crate::advisor::dismiss(&host, pane, card);
        }
        Ok(Value::Null)
    });
}

/// What the person chose in the fork sheet: whether the uncommitted changes come along. None when they cancelled.
async fn ask_fork(host: &Arc<Host>, here: bool, can_carry: bool) -> Option<bool> {
    let locale = host.locale;
    let dialog = json!({
        "type": "paneFork",
        "title": locale.get(if here { "fork.menu.here" } else { "fork.menu.pane" }),
        "message": locale.get(if here { "fork.dialog.messageHere" } else { "fork.dialog.message" }),
        "carry": locale.get("fork.dialog.carry"),
        "canCarry": can_carry,
        "noChanges": locale.get("fork.dialog.noChanges"),
        "fork": locale.get("fork.dialog.fork"),
        "cancel": locale.get("resume.cancel"),
    });
    let answer = worktrees::ask(host, dialog).await;
    answer.is_object().then(|| answer.get("carry").and_then(Value::as_bool).unwrap_or(false) && can_carry)
}

/// The files the source's work changed, for the summary: its commits against its base when it has a worktree, and what it has not
/// committed.
async fn changed_files(places: &worktrees::Places, folder: &str, worktree: Option<&PaneWorktree>) -> Vec<String> {
    let mut files: Vec<String> = match worktree {
        Some(worktree) => core::changed_files(&places.git, worktree).await.unwrap_or_default().into_iter().map(|f| f.path).collect(),
        None => vec![],
    };
    for path in core::uncommitted_paths(&places.git, folder).await {
        if !files.contains(&path) {
            files.push(path);
        }
    }
    files
}

/// The source pane forked into a new pane in a new worktree (see the module's words).
async fn fork(host: &Arc<Host>, source_id: &str, request: Option<&str>) -> Result<(), String> {
    let locale = host.locale;
    crate::bodies::ensure(host, &[source_id]);
    let (source, workspace) = {
        let view = host.view();
        let source = view.snapshot.session(source_id).cloned().ok_or_else(|| locale.get("fork.error.noConversation"))?;
        let workspace = view.snapshot.workspace(source.workspace_id()).cloned().ok_or_else(|| locale.get("fork.error.noConversation"))?;
        (source, workspace)
    };
    if source.kind() != "claude" {
        return Err(locale.get("fork.error.noConversation"));
    }
    if workspace.remote.is_some() || crate::mirror::is_mirror_workspace(&host.view().snapshot, workspace.id()) {
        return Err(locale.get("fork.error.remote"));
    }
    // MC-47: an SSH workspace's pane is not forked into a worktree (its uncommitted changes and conversation are on its machine).
    if workspace.is_ssh() {
        return Err(locale.get("fork.error.ssh"));
    }
    if source.status() == "running" || host.runs().is_some_and(|r| r.service.is_session_running(source_id)) {
        return Err(locale.get("fork.error.running"));
    }
    let point = conversation::fork_point(&source, request).ok_or_else(|| locale.get("fork.error.noConversation"))?;
    let places = worktrees::places().ok_or_else(|| text::reason(mighty_delegation::request::DelegationReasonCode::NotGit, &locale))?;
    let folder = source.working_folder.clone().unwrap_or_else(|| workspace.path().to_string());
    let read = match places.maker.check(&folder).await {
        ChildWorktreeCheck::Ready(read) => read,
        ChildWorktreeCheck::Refused(reason) => {
            return Err(locale.get_with("fork.error.cannotMake", &[("reason", &text::reason(reason, &locale))]));
        }
    };
    let start = core::fork_start(&read, source.pane_worktree.as_ref());
    if !worktrees::may_write(host, &[&folder, &start.repository, &places.root]) {
        return Err(locale.get_with("fork.error.cannotMake", &[("reason", &crate::setup::verification_mode())]));
    }
    let can_carry = !core::uncommitted_paths(&places.git, &folder).await.is_empty();
    let Some(carry) = ask_fork(host, request.is_some(), can_carry).await else { return Ok(()) };

    let (approved_cfg, cfg_note) = worktrees::resolve_worktree_repo_config(host, &workspace, true).await;
    let id = loop {
        let candidate = core::fork_session_id(&wire::new_id());
        if host.view().snapshot.session(&candidate).is_none() && !Path::new(&places.maker.worktree_path(&candidate)).exists() {
            break candidate;
        }
    };
    let made = match core::make_pane(&places.local, &id, &start, 300.0).await {
        ChildWorktreeOutcome::Created(made) => made,
        ChildWorktreeOutcome::Refused(reason) => {
            return Err(locale.get_with("fork.error.cannotMake", &[("reason", &text::reason(reason, &locale))]));
        }
        ChildWorktreeOutcome::Failed(message) => return Err(locale.get_with("fork.error.cannotMake", &[("reason", &message)])),
    };
    let mut notes = Vec::new();
    // Carried before anything else is put in the new worktree (its copied files, its setup), so a failed apply can put it back.
    if carry {
        match core::carry_changes(&places.git, &folder, &made.working_folder).await {
            CarryOutcome::Carried { .. } => notes.push(locale.get("fork.log.carried")),
            CarryOutcome::Nothing => {}
            CarryOutcome::Failed(reason) => notes.push(locale.get_with("fork.log.carryFailed", &[("reason", &reason)])),
        }
    }
    let files = changed_files(&places, &folder, source.pane_worktree.as_ref()).await;
    let handoff = conversation::handoff_summary(source.title(), &point, &files, &crate::git_workbench::redact_secret_text);
    let cli = cli_fork(&source, &point);
    if cli && source.provider() == "claude" {
        let (from, to) = (folder.clone(), made.working_folder.clone());
        let session = source.resume_id.clone().unwrap_or_default();
        let _ = tauri::async_runtime::spawn_blocking(move || copy_claude_transcript(&session, &from, &to)).await;
    }

    let port = core::port(&id, &worktrees::taken_ports(host), |port| worktrees::port_free(host, port));
    let setup = crate::repo_tasks::combine_setup_commands(
        approved_cfg.as_ref().and_then(|c| c.config.worktree.setup.as_deref()),
        workspace.worktree_setup_command.as_deref(),
    );
    let mut record = core::from_made(&made, &start.repository, port, setup);
    crate::worktree_lifecycle::assign_worktree(host, &mut record, crate::worktree_lifecycle::port_count(approved_cfg.as_ref()));
    let port_text = record.port.to_string();
    let made_line = locale
        .get_with("paneWorktree.log.made", &[("branch", record.branch()), ("base", record.base_branch()), ("port", port_text.as_str())]);
    let title = source.title().to_string();
    let how = if cli {
        locale.get_with("fork.log.cli", &[("title", &title), ("agent", mighty_run::providers::name(source.provider()))])
    } else {
        locale.get_with("fork.log.handoff", &[("title", &title)])
    };
    let link = forked_link(&source, request, cli, handoff);
    let (shaped_id, shaped_record, shaped_source) = (id.clone(), record.clone(), source.clone());
    let added = frame::add_pane_to(host, Some(workspace.id()), "claude", source.provider(), None, move |pane| {
        let provider = shaped_source.provider.clone();
        let mut pane = RunSession {
            id: Some(shaped_id),
            title: Some(shaped_source.title().to_string()),
            title_mode: shaped_source.title_mode.clone(),
            model: shaped_source.model.clone(),
            settings: shaped_source.settings.clone(),
            agent_view_mode: shaped_source.agent_view_mode.clone(),
            resume_id: if link.pending() { shaped_source.resume_id.clone() } else { None },
            working_folder: Some(shaped_record.run_folder().to_string()),
            pane_worktree: Some(shaped_record),
            linked_task: shaped_source.linked_task.clone(),
            forked_from: Some(link),
            ..pane
        };
        let logs = pane.logs.get_or_insert_with(Vec::new);
        logs.push(worktrees::system_log(made_line, provider.clone()));
        logs.push(worktrees::system_log(how, provider.clone()));
        for note in notes {
            logs.push(worktrees::system_log(note, provider.clone()));
        }
        pane
    });
    if host.view().snapshot.session(&id).is_none() {
        // The pane could not be added (the pane limit): the worktree made for it goes again, its ports with it.
        core::discard(places.local.machine.as_ref(), &record, &id, &places.root, &locale).await;
        crate::worktree_lifecycle::release_worktree(host, &record);
        return added.map(|_| ());
    }
    if let Some(note) = cfg_note {
        worktrees::append_system_note(host, &id, note);
    }
    let here = worktrees::Place { worktrees: places.local.clone(), ssh: None };
    worktrees::prepare(host, &id, &workspace, &here, record, approved_cfg);
    host.schedule();
    Ok(())
}

/// Whether the new pane's first run forks the source's session with the CLI: an agent that can ([`mighty_run::providers::cli_fork`]),
/// a session to fork, and the fork at the source's last request.
pub fn cli_fork(source: &RunSession, point: &ForkPoint) -> bool {
    let mode = source.settings.as_ref().map(|s| s.permission_mode.as_str()).unwrap_or("manual");
    point.is_last && source.resume_id.is_some() && mighty_run::providers::cli_fork(source.provider(), mode)
}

/// The new pane's link to its source.
pub fn forked_link(source: &RunSession, request: Option<&str>, cli: bool, handoff: String) -> ForkedFrom {
    ForkedFrom {
        pane_id: source.id().to_string(),
        request_id: request.map(str::to_string),
        title: Some(source.title().to_string()).filter(|t| !t.is_empty()),
        mode: Some(if cli { ForkedFrom::CLI } else { ForkedFrom::HANDOFF }.into()),
        fork_pending: cli.then_some(true),
        handoff: Some(handoff),
    }
}

/// Claude Code keeps a session under the folder it ran in (`<config>/projects/<folder name>/<id>.jsonl`): a copy of the source's
/// transcript under the new worktree's folder name lets `--resume <id> --fork-session` find it there whatever this Claude Code version
/// does across folders. The source's own file is only read; an existing file is never replaced.
fn copy_claude_transcript(session: &str, from: &str, to: &str) {
    if !wire::identifier(session) {
        return;
    }
    let home = std::path::PathBuf::from(crate::accounts::home());
    let environment: std::collections::HashMap<String, String> = std::env::vars().collect();
    let Some(source) = mighty_history::history::locate("claude", session, from, &environment, &home) else { return };
    let folder = mighty_history::history::claude_config(&home, &environment)
        .join("projects")
        .join(mighty_history::history::claude_project_folder(to));
    let target = folder.join(format!("{session}.jsonl"));
    if target.exists() || std::fs::create_dir_all(&folder).is_err() {
        return;
    }
    let _ = std::fs::copy(source, target);
}

#[cfg(test)]
mod tests {
    use super::*;
    use mighty_core::graph::MightyGraphRun;
    use mighty_core::model::RunSettings;

    fn source(provider: &str, mode: &str) -> RunSession {
        RunSession {
            id: Some("source".into()),
            title: Some("Login fix".into()),
            kind: Some("claude".into()),
            provider: Some(provider.into()),
            resume_id: Some("8cc9f04d-1f6e-487a-a773-41586e4f7f12".into()),
            settings: Some(RunSettings { permission_mode: mode.into(), ..RunSettings::default() }),
            graph_runs: Some(vec![
                MightyGraphRun { id: Some("r1".into()), input: Some("first".into()), ..MightyGraphRun::default() },
                MightyGraphRun { id: Some("r2".into()), input: Some("second".into()), ..MightyGraphRun::default() },
            ]),
            ..RunSession::default()
        }
    }

    #[test]
    fn the_cli_forks_only_at_the_last_request_of_an_agent_that_can() {
        let claude = source("claude", "manual");
        assert!(cli_fork(&claude, &conversation::fork_point(&claude, None).unwrap()));
        assert!(!cli_fork(&claude, &conversation::fork_point(&claude, Some("r1")).unwrap()), "an earlier request gets the summary");
        assert!(cli_fork(&source("codex", "acceptEdits"), &conversation::fork_point(&claude, None).unwrap()));
        assert!(!cli_fork(&source("codex", "onRequest"), &conversation::fork_point(&claude, None).unwrap()));
        for provider in ["gemini", "pi", "opencode", "hermes"] {
            assert!(!cli_fork(&source(provider, "manual"), &conversation::fork_point(&claude, None).unwrap()), "{provider}");
        }
        let fresh = RunSession { resume_id: None, ..claude.clone() };
        assert!(!cli_fork(&fresh, &conversation::fork_point(&fresh, None).unwrap()), "no session to fork");
    }

    #[test]
    fn the_handoff_summary_redacts_secrets_with_the_hosts_patterns() {
        let point = ForkPoint {
            request: "deploy it".into(),
            plan: None,
            result: "Set AWS_SECRET_ACCESS_KEY=AKIAABCDEFGHIJKLMNOP in the env\nThen ran it".into(),
            is_last: true,
        };
        let summary = conversation::handoff_summary("t", &point, &[], &crate::git_workbench::redact_secret_text);
        assert!(!summary.contains("AKIAABCDEFGHIJKLMNOP"), "{summary}");
        assert!(summary.contains("[redacted:") && summary.contains("Then ran it"));
    }

    /// The fork link persists through a save and a load (normalize included) with what the first run needs.
    #[test]
    fn the_fork_link_persists() {
        let claude = source("claude", "manual");
        let link = forked_link(&claude, Some("r2"), true, "summary".into());
        let pane =
            RunSession { id: Some("fork-abcdef12".into()), workspace_id: Some("w".into()), forked_from: Some(link.clone()), ..claude };
        use mighty_core::json::Wire;
        let written = pane.to_json();
        let read = RunSession::read(&serde_json::from_str(&written).unwrap()).unwrap();
        assert_eq!(read.forked_from.as_ref(), Some(&link));
        let kept = mighty_core::normalize::forked_from(read.forked_from.as_ref(), read.id()).unwrap();
        assert_eq!((kept.pane_id.as_str(), kept.request_id.as_deref(), kept.pending()), ("source", Some("r2"), true));
        assert!(mighty_core::normalize::forked_from(read.forked_from.as_ref(), "source").is_none(), "never a link to itself");
    }
}
