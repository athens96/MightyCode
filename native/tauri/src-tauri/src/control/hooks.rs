//! MC-9: status hooks. When an agent pane reaches a chosen state, the person's command runs: the one in Settings › 자동화 (the
//! states chosen there), and the workspace's `.mighty.local.yaml` `hooks.command` (its `on` states) once that configuration is
//! approved. The event goes in `MIGHTY_*` variables (`mighty_core::control::hooks::env`), never in arguments, and never a prompt.
//! Each run has a time limit (20 s, or `hooks.timeoutSeconds`), runs on a task of its own through the platform's process host (off
//! the UI thread), at most four at once; its output is not kept, the log names its exit only.

use std::path::Path;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use mighty_core::control::events::{ControlEvent, StateEvent};
use tokio::sync::Semaphore;

use crate::host::Host;

/// How long the Settings hook may run.
pub const SETTINGS_TIMEOUT: Duration = Duration::from_secs(20);
/// The hooks running at once; one more is skipped.
const AT_ONCE: usize = 4;

fn slots() -> &'static Arc<Semaphore> {
    static SLOTS: OnceLock<Arc<Semaphore>> = OnceLock::new();
    SLOTS.get_or_init(|| Arc::new(Semaphore::new(AT_ONCE)))
}

/// A bus event: the hooks that follow its state run.
pub fn on_event(host: &Arc<Host>, event: &Arc<ControlEvent>, settings_command: Option<String>, chosen: Vec<StateEvent>) {
    let Some(state_event) = event.state_event() else { return };
    if !(event.pane.is_agent() || event.is_automation()) || host.smoke.is_some() || host.closing() {
        return;
    }
    let (host, event) = (host.clone(), event.clone());
    tauri::async_runtime::spawn(async move {
        let Some((folder, workspace)) = place(&host, &event.pane.pane, &event.pane.workspace) else { return };
        if let Some(command) = settings_command.filter(|c| !c.trim().is_empty() && chosen.contains(&state_event)) {
            run(&host, &event, &command, &folder, workspace.path(), SETTINGS_TIMEOUT, "settings").await;
        }
        // The workspace's own, from `.mighty.local.yaml`, only once its configuration is approved.
        if let Ok(Some(status)) = crate::repo_tasks::load_workspace_config(&host, &workspace).await
            && status.approved
            && let Some(command) = status.loaded.config.hooks.command.clone()
            && status.loaded.config.hooks.events().iter().any(|e| e == state_event.name())
        {
            let timeout = status.loaded.config.hooks.timeout();
            run(&host, &event, &command, &folder, workspace.path(), timeout, ".mighty.local.yaml").await;
        }
    });
}

/// Where a pane's hook runs (its worktree's run folder, else the workspace folder) and its workspace; None for a remote one.
fn place(host: &Host, pane: &str, workspace: &str) -> Option<(String, mighty_core::model::Workspace)> {
    let view = host.view();
    let workspace = view.snapshot.workspace(workspace).filter(|w| w.remote.is_none() && !w.path().is_empty())?.clone();
    let folder = view
        .snapshot
        .session(pane)
        .and_then(|s| s.pane_worktree.as_ref())
        .map(|w| w.run_folder().to_string())
        .filter(|f| Path::new(f).is_dir())
        .unwrap_or_else(|| workspace.path().to_string());
    Some((folder, workspace))
}

/// One hook run, within `timeout`.
async fn run(host: &Arc<Host>, event: &ControlEvent, command: &str, folder: &str, workspace_path: &str, timeout: Duration, from: &str) {
    let Ok(_slot) = slots().clone().try_acquire_owned() else {
        host.log.line(&format!("status hook ({from}) skipped: {AT_ONCE} already run"));
        return;
    };
    let mut request = crate::repo_tasks::shell_request(command, folder);
    for (key, value) in crate::repo_tasks::login_shell_env(host, Some(Path::new(workspace_path))).await {
        request = request.env(key, value);
    }
    for (key, value) in mighty_core::control::hooks::env(event, workspace_path) {
        request = request.env(key, value);
    }
    let outcome = mighty_delegation::run_actions::run_bounded(host.processes.as_ref(), request, timeout).await;
    let how = match (outcome.timed_out, outcome.exit_code, &outcome.error) {
        (true, _, _) => format!("stopped after {} s", timeout.as_secs()),
        (_, _, Some(error)) => format!("did not start: {error}"),
        (_, Some(code), _) => format!("exit {code}"),
        _ => "ended".into(),
    };
    host.log.line(&format!("status hook ({from}) for {} {}: {how}", event.pane.pane, event.state_event().map_or("", |e| e.name())));
}
