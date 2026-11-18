//! The exe's MCP modes (C# WinUI/Program.cs:142-151; `AgentIOMcp.MainAsync`, Core/AgentIO/AgentIOMcp.cs:9-17;
//! `DelegationMcp.MainAsync`, Core/DelegationMcp.cs:25-32): `MightyCode.exe --agent-io-mcp` and `--agent-delegation-mcp`, which an agent
//! CLI starts from the run's `--mcp-config` (or Codex's `-c mcp_servers.*`). src-tauri main.rs calls them before parsing its arguments,
//! before Tauri, WebView2, the profile, the log or any mutex. Owner: W7-C.
//!
//! What they keep (W7 plan section 1.1): stdin strict UTF-8 (a leading byte order mark skipped), stdout UTF-8 without a BOM flushed
//! per line and nothing on it but protocol lines (no logging); exit 0 at stdin's end (1 when stdin was not UTF-8, where C# dies); the
//! pipe's name and the pane's token from the environment (`McpTransport::env_key`, `MIGHTY_PANE_TOKEN`), never written anywhere.

use std::sync::Arc;

use mighty_platform::AgentIoConnector;

use crate::agent_io::binding::TOKEN_KEY;
use crate::agent_io::client;
use crate::mcp::delegation::{DelegationServer, DelegationTransport};
use crate::mcp::serve::{self, Handle};
use crate::mcp::terminal::{TerminalServer, TerminalTransport};

/// `--agent-io-mcp` (exactly this one argument).
pub const AGENT_IO_MCP_ARGUMENT: &str = "--agent-io-mcp";
/// `DelegationMcp.HeadlessArgument`: `--agent-delegation-mcp` (exactly this one argument).
pub const DELEGATION_MCP_ARGUMENT: &str = "--agent-delegation-mcp";

/// C# Program.cs:146, `Locale.PreferredLanguages = WindowsPreferredLanguages` before any mode: the process's language read from the
/// OS's preferred languages once, so whatever copy a mode reads follows them as the app's "system" does.
fn locale_first() {
    let _ = mighty_run::strings::locale();
}

/// The server over stdin and stdout until stdin ends, on a runtime of its own; the exit code.
fn run(handler: Handle) -> i32 {
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(_) => return 1,
    };
    match runtime.block_on(serve::serve(serve::stdin_chunks(), serve::stdout(), handler)) {
        Ok(()) => 0,
        Err(_) => 1,
    }
}

/// `AgentIOMcp.MainAsync`: the `mighty-terminal` server over stdio until stdin ends; the process's exit code.
pub fn agent_io_mcp(connector: Arc<dyn AgentIoConnector>) -> i32 {
    locale_first();
    let pipe = std::env::var(connector.transport().env_key()).unwrap_or_default();
    let token = std::env::var(TOKEN_KEY).unwrap_or_default();
    run(TerminalServer::new(terminal_transport(connector, pipe, token)).handler())
}

/// `DelegationMcp.MainAsync`: the `mighty-delegation` server over stdio until stdin ends; the process's exit code. Without the pipe's
/// name or the token every call is answered by the server itself.
pub fn delegation_mcp(connector: Arc<dyn AgentIoConnector>) -> i32 {
    locale_first();
    let pipe = std::env::var(connector.transport().env_key()).unwrap_or_default();
    let token = std::env::var(TOKEN_KEY).unwrap_or_default();
    let transport = (!pipe.is_empty() && !token.is_empty()).then(|| delegation_transport(connector, pipe, token));
    run(DelegationServer::new(transport).handler())
}

/// `AgentIOPipe.Call` on this pane's pipe and token.
pub fn terminal_transport(connector: Arc<dyn AgentIoConnector>, pipe: String, token: String) -> TerminalTransport {
    Arc::new(move |tool, arguments| {
        let (connector, pipe, token) = (connector.clone(), pipe.clone(), token.clone());
        Box::pin(async move { client::call(connector.as_ref(), &pipe, &token, &tool, &arguments).await })
    })
}

/// `AgentIOPipe.CallDelegation` on this pane's pipe and token.
pub fn delegation_transport(connector: Arc<dyn AgentIoConnector>, pipe: String, token: String) -> DelegationTransport {
    Arc::new(move |request| {
        let (connector, pipe, token) = (connector.clone(), pipe.clone(), token.clone());
        Box::pin(async move { client::call_delegation(connector.as_ref(), &pipe, &token, &request).await })
    })
}
