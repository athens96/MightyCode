//! The app's Agent IO server (C# Core/AgentIO/AgentIOPipe.cs:7-137): one listener per app, 4 waiting and 64 connections, one request
//! and one answer per connection, the token resolved by a fixed-time compare, 4 delegation calls in flight per token, the deadlines
//! (terminal 30 s, `open_url` 60 s once authenticated, delegation 60 s), the call dropped when its binding is revoked, the answers'
//! texts. The bytes come from mighty-platform's `AgentIoListener`. Owner: W7-C.
//!
//! The seams (W7 plan, "Seams"): `AgentIo::{start(listener), bindings(), bind(...), revoke(pane), clear()}` for src-tauri agent_io.rs and
//! W7-D (`EndAgentTerminal` revokes a child's binding). The two handlers' signatures are final from W7 Step 0.

use mighty_platform::BoxFuture;
use serde_json::Value;

/// `IDelegationRequestHandler` (the coordinator implements it): defined beside the request it serves.
pub use crate::request::DelegationHandler;

/// One terminal or web tool call once its token resolved to a live binding and its one argument passed `AgentIOTools.Validate` (C#
/// `handle(binding, tool, value, token)`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalCall {
    /// The agent pane the binding belongs to.
    pub pane_id: String,
    pub workspace_id: String,
    pub workspace_path: String,
    pub provider: String,
    /// `run_in_terminal`, `read_latest_output`, `stop` or `open_url`.
    pub tool: String,
    /// Its one argument (the command, the handle or the URL).
    pub value: String,
}

/// What serves the terminal and web tools (C# `MainWindow.HandleAgentIO`; src-tauri agent_io.rs over terminal.rs's `Terminals::agent`
/// and the open_url prompt). The result is the tool's JSON (`AgentTerminalResult`, `{destination, url, …}`); an `Err` is answered with
/// the one refusal text and its detail never reaches the agent. The server drops the future at its deadline or when the binding is
/// revoked, which cancels the call.
pub trait TerminalHandler: Send + Sync {
    fn handle(&self, call: TerminalCall) -> BoxFuture<'static, Result<Value, String>>;
}
