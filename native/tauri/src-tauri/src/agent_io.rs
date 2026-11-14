//! The window's Agent IO (C# WinUI/MainWindow.AgentIO.cs: `InitializeAgentIO`, `HandleAgentIO`, `OpenAgentUrl`, `ReconcileAgentIO`,
//! the shutdown; MainWindow.WebShell.Cards.cs:51, 140-144, 399; MainWindow.WebShell.Panes.cs:441-454) over mighty-delegation's
//! `agent_io`. Owner: W7-C.
//!
//! W7 Step 0 wrote every seam the shared host files call, each at rest (no listener, no binding, no prompt), so main.rs, runs.rs,
//! host.rs and panes.rs need no further W7 change. What W7-C fills here (W7 plan, W7-C): the listener started at app start after the
//! first render, never in a smoke (C#'s `InitializeAgentIO`); `factory` (local workspaces, claude or codex, the delegation server only
//! while `delegation_on` and [`Host::delegation_live`]); the `TerminalHandler` over terminal.rs's `Terminals::agent` with the
//! `agent-terminal:<pane>` pane beside the agent; `open_url` through the saved `AgentWebOpenChoices[workspace]` or the prompt card
//! (inApp → `agent-browser:<pane>`, external → the OS's handler); `webOpenChoose`; `reconcile` and the revocations; the shutdown's
//! `clear`. Agent IO writes no config file: its servers are passed per run (W7 plan D3).

use std::sync::Arc;

use mighty_run::manager::BindingFactory;
use serde_json::Value;

use crate::host::{Host, Reply};

/// The app's listener, started once the page first rendered (C# `InitializeAgentIO`); not in a smoke. At rest: nothing.
pub fn start(host: &Arc<Host>) {
    let _ = host;
}

/// The app quits: every binding revoked and the listener closed (C# `agentIO.Bindings.Clear()`, `DisposeAsync`).
pub fn shutdown(host: &Host) {
    let _ = host;
}

/// The run service's binding factory (mighty-run `RunBinding`: `MIGHTY_PANE_TOKEN`, the transport's key, `--mcp-config` or Codex's
/// `-c` list, the token's redaction), read once as runs.rs makes the service. None: runs get no MCP server.
pub fn factory(host: &Arc<Host>) -> Option<BindingFactory> {
    let _ = host;
    None
}

/// C# `ReconcileAgentIO`: an agent pane that left the state loses its binding (and its queued calls). Called on the host's 100 ms poll,
/// without the state's lock held: it must stay cheap when nothing changed.
pub fn reconcile(host: &Host) {
    let _ = host;
}

/// The pane's waiting open_url prompts as the cards read them (`(requestId, url)`, mighty-composer `CardInputs::web_open`), read with
/// the state locked.
pub fn web_open_cards(host: &Host, pane: &str) -> Vec<(String, String)> {
    let _ = (host, pane);
    vec![]
}

/// `webOpenChoose{paneId, requestId, destination, remember}` (web/cards/Cards.tsx:647): the prompt answered (remember writes the state,
/// gated by `--allow-state-write` as every state write is). Answers null.
pub async fn web_open_choose(host: Arc<Host>, args: Value) -> Reply {
    let _ = (host, args);
    Ok(Value::Null)
}
