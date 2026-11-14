//! The Agent IO transport on Windows (C# `AgentIOPipe`, Core/AgentIO/AgentIOPipe.cs:15-161): a named pipe `mighty-agent-io-` + a GUID
//! in `N` format. Owner from W7 Step 0: W7-C.
//!
//! What it must do (W7 plan §1.5): tokio `named_pipe::ServerOptions` with `security_attributes_raw` — a DACL granting only the current
//! user's SID, the owner set to the current user —, `reject_remote_clients(true)`, `first_pipe_instance` for the first; the client
//! opens with `SECURITY_IDENTIFICATION` and checks that the server's owner SID is the current user (.NET's `CurrentUserOnly`); the
//! client validates the prefix and exactly 32 hex characters. Everything above the bytes is mighty-delegation's `agent_io::server`.
//! Until then both refuse as `Unsupported`.

use mighty_platform::{AgentIoConnection, AgentIoConnector, AgentIoListener, BoxFuture, PlatformError, Result};

/// The app's listener (one per app).
#[derive(Debug, Default)]
pub struct NamedPipeListener;

impl AgentIoListener for NamedPipeListener {
    fn bind(&self) -> Result<String> {
        Err(PlatformError::unsupported("AgentIoListener", "bind"))
    }
    fn accept(&self) -> BoxFuture<'_, Result<AgentIoConnection>> {
        Box::pin(async { Err(PlatformError::unsupported("AgentIoListener", "accept")) })
    }
    fn close(&self) {}
}

/// The MCP servers' client.
#[derive(Debug, Default, Clone, Copy)]
pub struct NamedPipeConnector;

impl AgentIoConnector for NamedPipeConnector {
    fn connect(&self, _name: &str, _timeout: std::time::Duration) -> BoxFuture<'_, Result<AgentIoConnection>> {
        Box::pin(async { Err(PlatformError::unsupported("AgentIoConnector", "connect")) })
    }
}
