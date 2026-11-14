//! The sharing host's strict HTTP/1.1 server (C# `RemoteHttpListener`, R\RemoteHttpListener.cs; plan §1.6, D7), hand-written over
//! tokio TCP, never hyper: one listener per Tailscale address through `Network::bind_exclusive` (backlog 64), a peer that is not
//! Tailscale, the 65th connection or a peer's 9th reset at accept; the head at most 8192 bytes (431) and 34 lines; a bad request line,
//! a control character, a folded line, a duplicate header or any `Transfer-Encoding` 400; a bad or too large `Content-Length` 413
//! before a body is read (a 0 limit never reads it); 15 s for head and body, then 60 s for the handler (over: dropped unanswered); the
//! fixed answer headers; 50 consecutive accept failures end sharing. Owner: W8-D.
//!
//! At rest (W8 Step 0): the limits only.

use std::time::Duration;

pub const MAXIMUM_HEAD_BYTES: usize = 8192;
pub const MAXIMUM_HEAD_LINES: usize = 34;
pub const BACKLOG: u32 = 64;
pub const MAXIMUM_CONNECTIONS: usize = 64;
pub const MAXIMUM_CONNECTIONS_PER_PEER: usize = 8;
pub const READ_DEADLINE: Duration = Duration::from_secs(15);
pub const HANDLER_DEADLINE: Duration = Duration::from_secs(60);
pub const MAXIMUM_ACCEPT_FAILURES: usize = 50;
