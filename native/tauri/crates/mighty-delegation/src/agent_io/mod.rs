//! Agent IO (C# Core/AgentIO/*.cs): the pane binding a run gets, the tools' validation, the transport's framing, the app's server, the
//! servers' client. The OS bytes are mighty-platform's `AgentIoListener`/`AgentIoConnector`. Owner: W7-C.

pub mod binding;
pub mod client;
pub mod mcp_config;
pub mod server;
pub mod tools;
pub mod wire;
