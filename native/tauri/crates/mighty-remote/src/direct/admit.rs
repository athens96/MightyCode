//! Direct admission (C# `MobileDirect.Admit`, `DirectAuthFailures`, R\MobileDirect.cs; plan §1.4): whether a socket the listeners just
//! accepted may start its handshake — a peer in Tailscale's ranges (any peer only through the checks' seam), room in the phone table,
//! fewer than 4 handshakes on the direct path and 2 from this address, and fewer than 5 failed auths from it in the last 60 s (256
//! addresses tracked). A refused socket is reset unread (linger 0). The count, the check and the insert are one step under the relay
//! host's lock (its `DirectSink::admit`); the adapters are read outside it (L503). Owner: W8-C.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Phones inside the handshake on the direct path, counted apart from the relay's.
pub const MAXIMUM_UNAUTHENTICATED: usize = 4;
/// Of those, at most this many from one remote address.
pub const MAXIMUM_UNAUTHENTICATED_PER_PEER: usize = 2;
/// Failed authentications one address may have inside [`FAILURE_WINDOW`] before its new sockets are reset unread.
pub const MAXIMUM_FAILURES: usize = 5;
pub const FAILURE_WINDOW: Duration = Duration::from_secs(60);
pub const TRACKED_PEERS: usize = 256;

/// `MobileDirect.Admit(peer, allowAnyPeer, clients, pendingPeers, recentFailures)`: `peer` canonical (`tailscale::canonical_text`);
/// `clients` every phone in the table (relay and direct); `pending_peers` the direct ones not yet authenticated (their peers);
/// `recent_failures` [`DirectAuthFailures::recent`] for `peer`.
pub fn admit(peer: Option<&str>, allow_any_peer: bool, clients: usize, pending_peers: &[String], recent_failures: usize) -> bool {
    let Some(peer) = peer.filter(|p| !p.is_empty()) else { return false };
    (allow_any_peer || crate::tailscale::contains(peer))
        && clients < crate::direct::wire::MAXIMUM_CLIENTS
        && pending_peers.len() < MAXIMUM_UNAUTHENTICATED
        && pending_peers.iter().filter(|pending| pending.as_str() == peer).count() < MAXIMUM_UNAUTHENTICATED_PER_PEER
        && recent_failures < MAXIMUM_FAILURES
}

/// C# `DirectAuthFailures`: failed direct authentications per remote address, kept for [`FAILURE_WINDOW`]; at most 256 addresses
/// unless all are recent.
#[derive(Debug, Default)]
pub struct DirectAuthFailures {
    failures: Mutex<HashMap<String, Vec<Instant>>>,
}

impl DirectAuthFailures {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Vec<Instant>>> {
        self.failures.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Addresses tracked now.
    pub fn peers(&self) -> usize {
        self.lock().len()
    }

    /// The failures of `peer` inside the window ending `now`; an address left with none is forgotten.
    pub fn recent(&self, peer: &str, now: Instant) -> usize {
        let mut failures = self.lock();
        let Some(times) = failures.get_mut(peer) else { return 0 };
        times.retain(|time| now.saturating_duration_since(*time) < FAILURE_WINDOW);
        let count = times.len();
        if count == 0 {
            failures.remove(peer);
        }
        count
    }

    /// One more failure of `peer` at `now`; a full table first drops the addresses whose failures are all old.
    pub fn record(&self, peer: &str, now: Instant) {
        let mut failures = self.lock();
        if !failures.contains_key(peer) && failures.len() >= TRACKED_PEERS {
            failures.retain(|_, times| times.iter().any(|time| now.saturating_duration_since(*time) < FAILURE_WINDOW));
        }
        failures.entry(peer.to_string()).or_default().push(now);
    }
}
