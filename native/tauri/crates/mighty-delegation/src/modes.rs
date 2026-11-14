//! The exe's MCP modes (C# WinUI/Program.cs:142-151; `AgentIOMcp.MainAsync`, Core/AgentIO/AgentIOMcp.cs:9-17;
//! `DelegationMcp.MainAsync`, Core/DelegationMcp.cs:25-32): `MightyCode.exe --agent-io-mcp` and `--agent-delegation-mcp`, which an agent
//! CLI starts from the run's `--mcp-config` (or Codex's `-c mcp_servers.*`). src-tauri main.rs calls them before parsing its arguments,
//! before Tauri, WebView2, the profile, the log or any mutex. Owner: W7-C.
//!
//! What they must keep (W7 plan section 1.1): stdin strict UTF-8 without a BOM, stdout UTF-8 without a BOM flushed per line and nothing
//! on it but protocol lines (no logging); exit 0 at stdin's end; the pipe's name and the pane's token from the environment
//! (`McpTransport::env_key`, `MIGHTY_PANE_TOKEN`). Until W7-C fills them each ends at once with 0.

use std::sync::Arc;

use mighty_platform::AgentIoConnector;

/// `--agent-io-mcp` (exactly this one argument).
pub const AGENT_IO_MCP_ARGUMENT: &str = "--agent-io-mcp";
/// `DelegationMcp.HeadlessArgument`: `--agent-delegation-mcp` (exactly this one argument).
pub const DELEGATION_MCP_ARGUMENT: &str = "--agent-delegation-mcp";

/// `AgentIOMcp.MainAsync`: the `mighty-terminal` server over stdio until stdin ends; the process's exit code.
pub fn agent_io_mcp(connector: Arc<dyn AgentIoConnector>) -> i32 {
    let _ = connector;
    0
}

/// `DelegationMcp.MainAsync`: the `mighty-delegation` server over stdio until stdin ends; the process's exit code.
pub fn delegation_mcp(connector: Arc<dyn AgentIoConnector>) -> i32 {
    let _ = connector;
    0
}
