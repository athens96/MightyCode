//! The OS transport between the app and the MCP servers its agent runs start (C# `AgentIOPipe`, Core/AgentIO/AgentIOPipe.cs; the Mac's
//! `AgentIOSocketServer.swift`). Owner from W7 Step 0: W7-C.
//!
//! Only the bytes are the OS's: Windows a named pipe `mighty-agent-io-<32 hex>` with .NET's `PipeOptions.CurrentUserOnly` semantics
//! (mighty-platform-windows `agent_io.rs`), the Mac a Unix socket (the Mac stage; `Unsupported` until then). The framing (Windows a
//! 4-byte big-endian length, the Mac one JSON line), the limits, the token, the busy count and the deadlines are shared code in
//! mighty-delegation's `agent_io`, chosen by [`crate::McpTransport`].

use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite};

use crate::{BoxFuture, McpTransport, Result};

/// One connection's two directions, as the OS opened it.
pub trait AgentIoStream: AsyncRead + AsyncWrite + Send + Unpin {}

impl<T: AsyncRead + AsyncWrite + Send + Unpin> AgentIoStream for T {}

/// One accepted or opened connection: one request and one answer travel over it, then it closes.
pub type AgentIoConnection = Box<dyn AgentIoStream>;

/// The app's side: one listener per app (C# `AgentIOPipe`'s waiting listeners).
pub trait AgentIoListener: Send + Sync {
    /// Starts listening once and gives the name a client connects to (Windows: `mighty-agent-io-` + a GUID in `N` format, which the
    /// run's environment carries; the Mac: the socket's path). Later calls give the same name.
    fn bind(&self) -> Result<String>;
    /// The next client, only from the current user (Windows: the pipe's DACL grants only the current user's SID and remote clients
    /// are refused). Callable from several tasks at once (C# keeps 4 waiting); fails once the listener is closed.
    fn accept(&self) -> BoxFuture<'_, Result<AgentIoConnection>>;
    /// Stops listening: every waiting `accept` fails, the name is free.
    fn close(&self);
    /// How a message is framed on this transport and which environment key carries the name: the named pipe's unless the transport
    /// says otherwise (the Mac's socket, its stage).
    fn transport(&self) -> McpTransport {
        McpTransport::NamedPipe
    }
}

/// The MCP server's side (`--agent-io-mcp`, `--agent-delegation-mcp`): opens a connection to the app's listener.
pub trait AgentIoConnector: Send + Sync {
    /// Connects to `name` within `timeout`. Windows opens with `SECURITY_IDENTIFICATION` and checks that the pipe's owner is the
    /// current user (what .NET's `CurrentUserOnly` client does); a name the transport does not take is `Invalid`.
    fn connect(&self, name: &str, timeout: Duration) -> BoxFuture<'_, Result<AgentIoConnection>>;
    /// The transport the listener this connects to speaks ([`AgentIoListener::transport`]).
    fn transport(&self) -> McpTransport {
        McpTransport::NamedPipe
    }
}
