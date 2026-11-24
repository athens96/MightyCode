//! MC-9: the control API's answers from the app's host: the state as the window has it, a send through the composer's path (the
//! phone's own send: queued when busy, the pane's style and "send to" choice), a new pane as the phone makes one.

use std::sync::{Arc, Weak};

use mighty_core::control::events::PaneState;
use mighty_core::control::protocol::{Command, ControlError, ErrorCode};
use mighty_core::control::text::transcript_lines;
use mighty_platform::BoxFuture;
use serde_json::{Value, json};
use tokio::sync::broadcast;

use super::bus::{self, BusEvent};
use super::server::Backend;
use crate::host::Host;

pub struct HostBackend {
    host: Weak<Host>,
}

impl HostBackend {
    pub fn new(host: &Arc<Host>) -> HostBackend {
        HostBackend { host: Arc::downgrade(host) }
    }

    fn host(&self) -> Result<Arc<Host>, ControlError> {
        self.host.upgrade().filter(|h| !h.closing()).ok_or_else(|| ControlError::new(ErrorCode::Failed, "the app is closing"))
    }
}

/// A refusal of the app's (an HTTP-like status and its words) as the control API says it.
fn refused((status, message): (u16, String)) -> ControlError {
    let code = match status {
        400 => ErrorCode::BadRequest,
        404 => ErrorCode::NotFound,
        _ => ErrorCode::Failed,
    };
    ControlError::new(code, message)
}

fn workspace_json(snapshot: &mighty_core::model::AppSnapshot, workspace: &mighty_core::model::Workspace) -> Value {
    json!({
        "id": workspace.id(),
        "name": workspace.name(),
        "path": workspace.path(),
        "remote": workspace.remote.is_some(),
        "active": snapshot.active_workspace_id.as_deref() == Some(workspace.id()),
        "panes": snapshot.sessions().iter().filter(|s| s.workspace_id() == workspace.id()).count(),
    })
}

impl Backend for HostBackend {
    fn resolve(&self, pane: &str) -> Result<String, ControlError> {
        let host = self.host()?;
        let view = host.view();
        view.snapshot.bot_pane(pane).map(|p| p.id().to_string()).ok_or_else(|| ControlError::not_found("bot"))
    }

    fn bot_command(&self, command: Command) -> BoxFuture<'_, Result<Value, ControlError>> {
        Box::pin(async move {
            let host = self.host()?;
            let failed = |words: String| ControlError::new(ErrorCode::Failed, words);
            match command {
                Command::BotList => Ok(json!({ "bots": crate::bot::list(&host, "") })),
                Command::BotNew { spec, update } => {
                    crate::bot::propose(&host, &spec, update, host.locale.get("bot.approve.fromCli")).map_err(failed)?;
                    Ok(json!({ "pending": true, "message": host.locale.get("bot.approve.pending") }))
                }
                Command::BotSleep { bot, asleep } => {
                    let pane = host.view().snapshot.bot_pane(&bot).map(|p| p.id().to_string()).ok_or_else(|| ControlError::not_found("bot"))?;
                    crate::bot::set_asleep(&host, &pane, asleep).await;
                    Ok(json!({ "bot": bot, "asleep": asleep }))
                }
                Command::BotRun { bot, job } => {
                    let jobs = crate::automation::jobs_of_bot(&bot);
                    let chosen = match &job {
                        Some(name) => jobs.iter().find(|j| j.name() == name.as_str()),
                        None if jobs.len() == 1 => jobs.first(),
                        None => None,
                    };
                    let Some(chosen) = chosen else {
                        let names = jobs.iter().map(|j| j.name().to_string()).collect::<Vec<_>>().join(", ");
                        return Err(ControlError::bad(if jobs.is_empty() { format!("{bot} has no schedule or trigger") } else { format!("which job? --job one of: {names}") }));
                    };
                    crate::automation::run_now(&host, &chosen.key).await.map_err(failed)?;
                    Ok(json!({ "bot": bot, "job": chosen.name() }))
                }
                Command::EventEmit { name, data } => {
                    let data = data.as_deref().map(serde_json::from_str::<Value>).transpose().map_err(|_| ControlError::bad("--data is not JSON"))?;
                    let fired = crate::automation::chain::emit(&host, &name, data.as_ref()).map_err(ControlError::bad)?;
                    Ok(json!({ "event": name, "fired": fired }))
                }
                _ => Err(ControlError::bad("not a bot command")),
            }
        })
    }

    fn bot_refusal(&self, handle: &str) -> Option<String> {
        let host = self.host().ok()?;
        crate::automation::bot_unavailable(&host, handle)
    }

    fn status(&self) -> Result<Value, ControlError> {
        let host = self.host()?;
        let view = host.view();
        let snapshot = &view.snapshot;
        let panes: Vec<Value> = snapshot.sessions().iter().map(|s| super::pane_json(&host, &view, s)).collect();
        let waiting = panes.iter().filter(|p| !p["attention"].is_null()).count();
        Ok(json!({
            "app": { "name": "Mighty Code", "version": host.version, "pid": std::process::id() },
            "activeWorkspace": snapshot.active_workspace_id,
            "activePane": snapshot.active_session_id,
            "workspaces": snapshot.workspaces().iter().map(|w| workspace_json(snapshot, w)).collect::<Vec<_>>(),
            "panes": panes,
            "waiting": waiting,
        }))
    }

    fn workspaces(&self) -> Result<Value, ControlError> {
        let host = self.host()?;
        let view = host.view();
        let snapshot = &view.snapshot;
        Ok(json!({ "workspaces": snapshot.workspaces().iter().map(|w| workspace_json(snapshot, w)).collect::<Vec<_>>() }))
    }

    fn panes(&self, workspace: Option<&str>) -> Result<Value, ControlError> {
        let host = self.host()?;
        let view = host.view();
        if let Some(workspace) = workspace
            && view.snapshot.workspace(workspace).is_none()
        {
            return Err(ControlError::not_found("workspace"));
        }
        let panes: Vec<Value> = view
            .snapshot
            .sessions()
            .iter()
            .filter(|s| workspace.is_none_or(|w| s.workspace_id() == w))
            .map(|s| super::pane_json(&host, &view, s))
            .collect();
        Ok(json!({ "panes": panes }))
    }

    fn read(&self, pane: &str, lines: usize) -> BoxFuture<'_, Result<Value, ControlError>> {
        let pane = pane.to_string();
        Box::pin(async move {
            let host = self.host()?;
            // A pane whose body the load left on disk is read whole first (off the async threads).
            let loader = host.clone();
            let id = pane.clone();
            let _ = tauri::async_runtime::spawn_blocking(move || crate::bodies::ensure(&loader, &[id.as_str()])).await;
            let view = host.view();
            let session = view.snapshot.session(&pane).ok_or_else(|| ControlError::not_found("pane"))?;
            let kind = mighty_core::control::events::pane_kind(session.kind()).to_string();
            let (text, truncated) = if session.kind() == "claude" {
                let entries = session.logs().iter().map(|e| (e.kind(), e.text()));
                transcript_lines(entries, lines, &crate::git_workbench::redact_secret_text)
            } else {
                let tail: Vec<String> =
                    super::terminal_tail(&pane, lines + 1).into_iter().map(|l| crate::git_workbench::redact_secret_text(&l)).collect();
                let truncated = tail.len() > lines;
                (tail.into_iter().rev().take(lines).collect::<Vec<_>>().into_iter().rev().collect(), truncated)
            };
            Ok(json!({ "pane": pane, "kind": kind, "lines": text, "truncated": truncated }))
        })
    }

    fn send(&self, pane: &str, text: &str) -> BoxFuture<'_, Result<Value, ControlError>> {
        let (pane, text) = (pane.to_string(), text.to_string());
        Box::pin(async move {
            let host = self.host()?;
            {
                let view = host.view();
                let session = view.snapshot.session(&pane).ok_or_else(|| ControlError::not_found("pane"))?;
                if session.kind() != "claude" {
                    return Err(ControlError::bad("only an agent pane takes a request"));
                }
            }
            let outcome = crate::phone::control_send(&host, &pane, &text).await.map_err(refused)?;
            let state = self.state(&pane).map(PaneState::name);
            Ok(json!({ "pane": pane, "outcome": outcome, "state": state }))
        })
    }

    fn create(&self, workspace: &str, agent: &str, worktree: bool) -> BoxFuture<'_, Result<Value, ControlError>> {
        let (workspace, agent) = (workspace.to_string(), agent.to_string());
        Box::pin(async move {
            let host = self.host()?;
            if !mighty_core::wire::PROVIDERS.contains(&agent.as_str()) {
                return Err(ControlError::bad(format!("unknown agent '{agent}': use {}", mighty_core::wire::PROVIDERS.join(", "))));
            }
            {
                let view = host.view();
                let found = view.snapshot.workspace(&workspace).ok_or_else(|| ControlError::not_found("workspace"))?;
                if found.remote.is_some() {
                    return Err(ControlError::bad("a remote workspace's panes are made on its own computer"));
                }
            }
            let id = crate::phone::control_create(host.clone(), workspace, agent, worktree).await.map_err(refused)?;
            let view = host.view();
            let session = view.snapshot.session(&id).ok_or_else(|| ControlError::not_found("pane"))?;
            Ok(json!({ "pane": super::pane_json(&host, &view, session) }))
        })
    }

    fn state(&self, pane: &str) -> Option<PaneState> {
        let host = self.host().ok()?;
        let view = host.view();
        let session = view.snapshot.session(pane)?;
        Some(super::pane_state(&host, &view, session).0)
    }

    fn subscribe(&self) -> broadcast::Receiver<BusEvent> {
        bus::global().subscribe()
    }

    fn redact(&self, text: &str) -> String {
        crate::git_workbench::redact_secret_text(text)
    }

    fn server_status(&self) -> Result<Value, ControlError> {
        let host = self.host()?;
        Ok(crate::server::status(&host))
    }

    /// Answered first; the quit begins a moment later, so this answer reaches the CLI before the listener closes.
    fn server_stop(&self, stop_all: bool) -> Result<Value, ControlError> {
        let host = self.host()?;
        let keeps = !stop_all && crate::daemon::enabled(&host) && matches!(crate::daemon::phase(), crate::daemon::Phase::Connected);
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            crate::server::stop(&host, stop_all, "mighty server stop");
        });
        Ok(json!({ "stopping": true, "pid": std::process::id(), "keptDaemonWork": keeps }))
    }

    fn show_window(&self) -> Result<Value, ControlError> {
        let host = self.host()?;
        let done = crate::server::show_window(&host);
        Ok(json!({ "window": done, "pid": std::process::id() }))
    }

    /// The answer carries the code: it goes back to the CLI's caller (another computer's app, over its ssh) and nowhere else.
    fn ssh_pair(&self) -> BoxFuture<'_, Result<Value, ControlError>> {
        Box::pin(async move {
            let host = self.host()?;
            crate::remote::ssh_pair(&host).await.map_err(|message| ControlError::new(ErrorCode::Failed, message))
        })
    }
}
