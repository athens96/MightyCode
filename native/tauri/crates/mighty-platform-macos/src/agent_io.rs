//! M-C: Agent IO over a Unix socket (`transport()` = `UnixSocket`; the line framing is shared, W7 D4). Swift
//! `AgentIOSocketServer.swift` (264 lines), `AgentPaneMCPBinding.swift` (284), `MCPStdioServer.swift` (155): `<data>/agent-io/io.sock`,
//! else `$TMPDIR/mc-<uid>/<fnv1a32>.sock` under the 104-byte limit; folder 0700, socket 0600; `getpeereid`; one JSON line per message
//! of 1 MiB at most; 10 s; `MIGHTY_AGENT_IO_SOCKET`, `MIGHTY_PANE_TOKEN`. Contract `mighty_platform::agent_io`.

use std::sync::Arc;

use mighty_platform::unsupported::Unsupported;
use mighty_platform::{AgentIoConnector, AgentIoListener};

/// The app's listener (Unsupported until M-C).
pub fn agent_io_listener() -> Arc<dyn AgentIoListener> {
    Arc::new(Unsupported)
}

/// The exe's MCP modes' client (Unsupported until M-C).
pub fn agent_io_connector() -> Arc<dyn AgentIoConnector> {
    Arc::new(Unsupported)
}
