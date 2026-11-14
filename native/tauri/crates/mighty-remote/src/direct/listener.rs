//! The direct listeners (C# `MobileDirectListener`, R\MobileDirectListener.cs; plan §1.4): one per Tailscale address through
//! mighty-platform's `Network::bind_exclusive` (never a wildcard; tests bind loopback port 0 through the fake), each accepted socket
//! admitted ([`DirectSink::admit`]) or reset unread, upgraded (`upgrade`) and handed to the relay host ([`DirectSink::accept`],
//! W8-A's `RelayHost::accept_direct`). Started only when the app may listen (`Host::may_listen`). Owner: W8-C.
//!
//! At rest (W8 Step 0): [`DirectListener::start`] listens on nothing.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use mighty_platform::{BoxFuture, Network};
use tokio::net::TcpStream;
use tokio_tungstenite::WebSocketStream;

use crate::direct::status::MobileDirectStatus;

/// Where admitted, upgraded sockets go (W8-A's relay host): admission is one check-and-insert under the host's lock, with the counts
/// of its own clients.
pub trait DirectSink: Send + Sync {
    /// `MobileDirect.Admit`: true reserves an unauthenticated slot for `peer` (released when its connection authenticates or ends).
    fn admit(&self, peer: IpAddr) -> bool;
    /// The upgraded socket, run as a direct connection.
    fn accept(&self, socket: WebSocketStream<TcpStream>, peer: SocketAddr) -> BoxFuture<'static, ()>;
}

/// The running listeners; dropping it closes them.
pub struct DirectListener {
    status: MobileDirectStatus,
}

impl DirectListener {
    /// Listens on `port` at each of `addresses` (Tailscale's, canonical) and serves until dropped.
    pub fn start(network: Arc<dyn Network>, addresses: Vec<IpAddr>, port: u16, sink: Arc<dyn DirectSink>) -> DirectListener {
        let _ = (network, addresses, port, sink);
        DirectListener { status: MobileDirectStatus::default() }
    }

    /// What it listens on, or why not.
    pub fn status(&self) -> MobileDirectStatus {
        self.status.clone()
    }
}
