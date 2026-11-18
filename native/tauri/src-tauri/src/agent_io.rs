//! The window's Agent IO (C# WinUI/MainWindow.AgentIO.cs: `InitializeAgentIO`, `HandleAgentIO`, `OpenAgentUrl`, `ReconcileAgentIO`,
//! the shutdown; MainWindow.WebShell.Cards.cs:51, 140-144, 399; MainWindow.WebShell.Panes.cs:441-454) over mighty-delegation's
//! `agent_io`. Owner: W7-C.
//!
//! - The listener ([`start`]) is one named pipe per app, never in a smoke (C#'s `InitializeAgentIO`). Agent IO writes no config file:
//!   its servers are passed per run (W7 plan D3), through [`factory`] — a Claude or Codex run of a local workspace gets its pane's
//!   binding, with the delegation server while the switch is on and delegation is live ([`Host::delegation_live`]), answered by the
//!   handler W7-D's delegation.rs sets ([`set_delegation_handler`]).
//! - `run_in_terminal`, `read_latest_output`, `stop` run in the agent pane's own terminal (terminal.rs's `Terminals::agent`), its
//!   `agent-terminal:<pane>` pane opening beside the agent; `open_url` opens the page in the user's default browser at once (the OS's
//!   handler). C# asked with a card where to open it and could open it in the app's own browser pane; 0.5 has no in-app browser.
//! - A binding goes when its agent pane leaves the state ([`reconcile`]), when the pane's next run binds it again, when a delegated
//!   child's terminal ends ([`end_agent_terminal`]) and when the app quits ([`shutdown`]); its calls still waiting are dropped.

use std::collections::HashSet;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, Weak};

use mighty_delegation::agent_io::binding::AgentIoBinding;
use mighty_delegation::agent_io::server::{AgentIo, DelegationLink, TerminalCall, TerminalHandler, logged_binding_factory};
use mighty_delegation::agent_io::tools::absolute_uri;
use mighty_delegation::request::DelegationHandler;
use mighty_platform::BoxFuture;
use mighty_run::manager::BindingFactory;
use serde_json::{Value, json};
use tauri::Manager;

use crate::host::Host;
use crate::terminal::{AGENT_TERMINAL, show_agent_io_pane, terminals};

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The window's Agent IO state (C# `agentIO`, `RunManager.DelegationHandler`), managed by the app.
struct AgentIoState {
    io: Mutex<Option<Arc<AgentIo>>>,
    handler: Mutex<Option<Arc<dyn DelegationHandler>>>,
    /// The agent panes the last reconcile saw (C# `agentBoundPanes`): only a pane seen and then gone is revoked.
    seen: Mutex<HashSet<String>>,
}

/// The state, made the first time it is asked for.
fn state(host: &Host) -> Arc<AgentIoState> {
    if let Some(state) = host.app.try_state::<Arc<AgentIoState>>() {
        return state.inner().clone();
    }
    let state = Arc::new(AgentIoState { io: Mutex::new(None), handler: Mutex::new(None), seen: Mutex::new(HashSet::new()) });
    // Another thread may have managed one meanwhile: that one is the state.
    host.app.manage(state.clone());
    host.app.try_state::<Arc<AgentIoState>>().map(|s| s.inner().clone()).unwrap_or(state)
}

/// The listener, when it runs.
pub fn io(host: &Host) -> Option<Arc<AgentIo>> {
    lock(&state(host).io).clone()
}

/// The app's listener, started with the app (C# `InitializeAgentIO`); not in a smoke.
pub fn start(host: &Arc<Host>) {
    if host.smoke.is_none() {
        listen(host);
    }
}

/// The listener, started now unless it runs (its exe's own path is needed for the runs' configs); None when it could not start.
/// [`start`] calls it outside a smoke; W7-D's delegation smoke calls it itself for its Rust-only `delegateTool` step, whose fake
/// Claude reaches the delegation server through the real exe mode and the real pipe.
pub fn listen(host: &Arc<Host>) -> Option<Arc<AgentIo>> {
    let state = state(host);
    let mut current = lock(&state.io);
    if let Some(io) = current.as_ref() {
        return Some(io.clone());
    }
    std::env::current_exe().ok()?;
    let tools: Arc<dyn TerminalHandler> = Arc::new(WindowTools { host: Arc::downgrade(host) });
    let listener = crate::platform::agent_io_listener(&host.data_dir);
    // The listener's tasks run on the app's runtime, whether this is called from it or from the setup's thread.
    let runtime = tauri::async_runtime::handle();
    let _entered = runtime.inner().enter();
    match AgentIo::start(listener, tools) {
        Ok(io) => {
            host.log.line("agent io: listening");
            *current = Some(io.clone());
            Some(io)
        }
        Err(error) => {
            host.log.line(&format!("agent io: not started: {error}"));
            None
        }
    }
}

/// The app quits: every binding revoked and the listener closed (C# `ShutdownAgentIO`).
pub fn shutdown(host: &Host) {
    let state = state(host);
    let io = lock(&state.io).take();
    if let Some(io) = io {
        tauri::async_runtime::block_on(async move {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(3), io.close()).await;
        });
    }
}

/// W7-D's seam: who answers the runs' delegation calls from now on (C# `local.DelegationHandler = coordinator`, or null when
/// delegation stops). A run binds with the handler set when it starts.
#[allow(dead_code, reason = "W7-D's delegation.rs sets it when delegation starts and stops")]
pub fn set_delegation_handler(host: &Host, handler: Option<Arc<dyn DelegationHandler>>) {
    *lock(&state(host).handler) = handler;
}

/// The run service's binding factory (C# `ConfigureAgentIO`'s factory and RunManager.cs:374-380), read once as runs.rs makes the
/// service; each run start asks the listener then (none before it starts, nor in a smoke: such a run gets no MCP server).
pub fn factory(host: &Arc<Host>) -> Option<BindingFactory> {
    let weak = Arc::downgrade(host);
    let executable = std::env::current_exe().ok()?.to_string_lossy().into_owned();
    Some(Arc::new(move |request, workspace| {
        let host = weak.upgrade()?;
        let state = state(&host);
        let io = lock(&state.io).clone()?;
        let link_host = Arc::downgrade(&host);
        let link_state = state.clone();
        let link: DelegationLink = Arc::new(move || {
            let handler = lock(&link_state.handler).clone();
            let enabled = link_host.upgrade().is_some_and(|host| crate::settings::general::delegation_on(&host) && host.delegation_live());
            (handler, enabled)
        });
        let log_host = Arc::downgrade(&host);
        let log = Arc::new(move |line: &str| {
            if let Some(host) = log_host.upgrade() {
                host.log.line(line);
            }
        });
        logged_binding_factory(io, executable.clone(), link, log)(request, workspace)
    }))
}

/// C# `ReconcileAgentIO`: an agent pane that left the state loses its binding (and its queued calls); its terminal goes with it
/// (terminal.rs). Only a pane an earlier look saw is revoked: the page's copy of the state may not have taken a run's new pane yet,
/// and its binding stays. Called on the host's 100 ms poll without the state's lock held: one look at the pane ids, nothing more.
pub fn reconcile(host: &Host) {
    let Some(state) = host.app.try_state::<Arc<AgentIoState>>().map(|state| state.inner().clone()) else { return };
    let Some(io) = lock(&state.io).clone() else { return };
    let agents: HashSet<String> =
        host.view().snapshot.sessions().iter().filter(|s| s.kind() == "claude").map(|s| s.id().to_string()).collect();
    let gone = retire(&mut lock(&state.seen), agents);
    for pane in gone {
        io.revoke(&pane);
    }
}

/// `agentBoundPanes.Except(ids)`, then `agentBoundPanes` made `ids`: the panes seen before and gone now.
fn retire(seen: &mut HashSet<String>, agents: HashSet<String>) -> Vec<String> {
    let gone = seen.difference(&agents).cloned().collect();
    *seen = agents;
    gone
}

/// C# `EndAgentTerminal` (W7-D's seam for a delegated child's discard and close cleanup): the pane's binding revoked, so no tool call
/// opens another terminal, then its terminal and everything it started ended; returns once they have.
#[allow(dead_code, reason = "W7-D's delegation.rs calls it for a child's discard and close cleanup")]
pub async fn end_agent_terminal(host: &Arc<Host>, pane: &str) -> bool {
    if let Some(io) = io(host) {
        io.revoke(pane);
    }
    if let Ok(terminals) = terminals(host) {
        terminals.end_agent(pane).await;
    }
    true
}

/// What serves the terminal and web tools (C# `HandleAgentIO`).
struct WindowTools {
    host: Weak<Host>,
}

impl TerminalHandler for WindowTools {
    fn handle(&self, call: TerminalCall) -> BoxFuture<'static, Result<Value, String>> {
        let host = self.host.clone();
        Box::pin(async move {
            let host = host.upgrade().ok_or("the app is closing")?;
            handle_tool(&host, &call).await
        })
    }
}

/// C# `RequireActiveAgentBinding`: the call's binding is still its pane's.
fn require_active(host: &Host, binding: &Arc<AgentIoBinding>) -> Result<(), String> {
    let current = io(host).is_some_and(|io| io.bindings().is_current(binding));
    if current { Ok(()) } else { Err("Agent binding was replaced.".into()) }
}

/// The binding's agent pane is still in its workspace.
fn pane_open(host: &Host, call: &TerminalCall) -> bool {
    !host.closing() && host.view().snapshot.sessions().iter().any(|p| p.id() == call.pane_id && p.workspace_id() == call.workspace_id)
}

async fn handle_tool(host: &Arc<Host>, call: &TerminalCall) -> Result<Value, String> {
    require_active(host, &call.binding)?;
    if !pane_open(host, call) {
        return Err("Agent pane closed.".into());
    }
    if call.tool == "open_url" {
        return open_url(host, call).await;
    }
    let terminals = terminals(host)?;
    let terminal = match terminals.existing_agent(&call.pane_id) {
        Some(terminal) => terminal,
        None if call.tool == "run_in_terminal" => terminals.agent(&call.pane_id, Path::new(&call.workspace_path)),
        None => return Err("Unknown terminal handle.".into()),
    };
    if call.tool == "run_in_terminal" {
        terminals.watch_prompts();
        show_agent_io_pane(host, &call.pane_id, &call.workspace_id, AGENT_TERMINAL)?;
        require_active(host, &call.binding)?;
        host.view().shows_dashboard = false;
        host.schedule();
    }
    require_active(host, &call.binding)?;
    let result = match call.tool.as_str() {
        "run_in_terminal" => terminal.run(&call.value).await,
        "read_latest_output" => terminal.read(&call.value),
        "stop" => terminal.stop(&call.value).await,
        _ => return Err("Unknown agent tool.".into()),
    };
    result.map(|result| serde_json::to_value(result).unwrap_or(Value::Null)).map_err(|error| error.to_string())
}

/// C# `OpenAgentUrl` without its choice: the page in the user's default browser (the OS's handler), at once. (C# asked with a card,
/// remembered the workspace's answer and could open the agent's own browser pane; 0.5 has no in-app browser.)
async fn open_url(host: &Arc<Host>, call: &TerminalCall) -> Result<Value, String> {
    let url = absolute_uri(&call.value);
    require_active(host, &call.binding)?;
    // ShellExecute on a blocking thread of its own (it may wait on the shell, and wants a single-threaded apartment there).
    let opening = url.clone();
    tauri::async_runtime::spawn_blocking(move || crate::platform::open_url(&opening)).await.map_err(|error| error.to_string())??;
    Ok(json!({"destination": "external", "url": url}))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// L2: only a pane seen before and gone now is revoked; a run's pane the page's copy has not taken yet keeps its binding.
    #[test]
    fn only_a_pane_seen_and_then_gone_is_revoked() {
        let set = |ids: &[&str]| ids.iter().map(|id| id.to_string()).collect::<HashSet<String>>();
        let mut seen = HashSet::new();
        assert!(retire(&mut seen, set(&["a"])).is_empty(), "a pane not seen yet is never revoked");
        assert!(retire(&mut seen, set(&["a", "b"])).is_empty());
        assert_eq!(retire(&mut seen, set(&["b"])), ["a"], "a pane seen and then gone is");
        assert!(retire(&mut seen, set(&["b"])).is_empty() && seen == set(&["b"]), "once");
    }
}
