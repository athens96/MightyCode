//! Direct admission (C# `MobileDirect.Admit`, under one lock with the relay host's clients; plan §1.4): the peer is Tailscale, fewer than
//! 32 clients, fewer than 4 unauthenticated direct connections, fewer than 2 unauthenticated from that peer, fewer than 5 auth
//! failures from it in 60 s (256 peers tracked); a refused socket is reset unread (linger 0). The adapters are read outside the lock
//! (L503). Owner: W8-C.
//!
//! At rest (W8 Step 0): the limits only.

use std::time::Duration;

pub const MAXIMUM_UNAUTHENTICATED: usize = 4;
pub const MAXIMUM_UNAUTHENTICATED_PER_PEER: usize = 2;
pub const MAXIMUM_FAILURES: usize = 5;
pub const FAILURE_WINDOW: Duration = Duration::from_secs(60);
pub const TRACKED_PEERS: usize = 256;
