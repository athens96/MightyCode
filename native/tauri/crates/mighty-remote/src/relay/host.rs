//! The relay host (C# `MobileRelayHost`'s lifecycle and control loop, R\MobileRelayHost.cs:54-560; plan §1.1): the settings applied
//! under one lifecycle lock with a generation counter, the control socket `<relay>/ws?serverId=…&role=server&v=1&hostToken=…` with
//! its keepalive and redial pacing (`policy`), a data socket per phone (`&connectionId=<id>`, never for a `direct-` id; ignored at 32
//! clients or 4 unauthenticated relay phones), each run by `connection`; the status (`status`); notify to every authenticated phone;
//! rotation (a new pairing key, every connection cancelled, push forgetting its views); the direct path's sockets handed in by W8-C's
//! listener (`accept_direct`, admission counted under the host's lock: [`crate::direct::listener::DirectSink`]); dispose. The relay URL
//! is never logged. Owner: W8-A.
//!
//! At rest (W8 Step 0): a host that never connects: its status is `disabled`, and every call does nothing.

use std::net::SocketAddr;
use std::sync::Arc;

use mighty_core::model::MobileRemoteSettings;
use mighty_platform::Platform;
use tokio::net::TcpStream;
use tokio_tungstenite::WebSocketStream;

use crate::identity::Identity;
use crate::relay::connection::RequestHandler;
use crate::relay::status::MobileRelayStatus;

/// `MobileRelayHost.DefaultRelay`: the relay on Cloudflare Workers.
pub const DEFAULT_RELAY: &str = "wss://mighty-relay.athens96.workers.dev";
/// `RetiredDefaultRelays`: a relay field holding one falls back to [`DEFAULT_RELAY`].
pub const RETIRED_DEFAULT_RELAYS: [&str; 1] = ["wss://mightyclaude.duckdns.org"];
/// At most this many phones at once.
pub const MAXIMUM_CLIENTS: usize = 32;
/// `MaximumRelayUnauthenticated`.
pub const MAXIMUM_RELAY_UNAUTHENTICATED: usize = 4;

/// One relay host for the app's lifetime (src-tauri `mobile.rs` makes it only when `Host::may_reach_relay`).
pub struct RelayHost {
    identity: Arc<Identity>,
    handler: Arc<dyn RequestHandler>,
    platform: Platform,
}

impl RelayHost {
    /// `new MobileRelayHost(directory, hostName, capabilities, route)` with the saved settings, not yet applied.
    pub fn new(identity: Arc<Identity>, settings: MobileRemoteSettings, handler: Arc<dyn RequestHandler>, platform: Platform) -> RelayHost {
        let _ = settings;
        RelayHost { identity, handler, platform }
    }

    /// `ApplyAsync`: the settings normalized and applied (the link started, redialled or stopped).
    pub async fn apply(&self, settings: MobileRemoteSettings) {
        let _ = (settings, &self.identity, &self.handler, &self.platform);
    }

    /// `ReconnectSoon`: a redial without pacing (a wake, the Settings button).
    pub fn reconnect_soon(&self) {}

    /// `RotateKeyAsync(expectedDeviceId)`: a new pairing key, every device and connection dropped; with an expected device, only when
    /// that device is still paired (a revoke). True when it rotated.
    pub async fn rotate(&self, expected_device: Option<String>) -> bool {
        let _ = expected_device;
        false
    }

    /// `Status`.
    pub fn status(&self) -> MobileRelayStatus {
        MobileRelayStatus::disabled()
    }

    /// `NotifyAsync(scope, revision)`: `{type:"notify", scope, revision}` to every authenticated phone (`state` before `session:<id>`).
    pub async fn notify(&self, scope: &str, revision: i64) {
        let _ = (scope, revision);
    }

    /// `AcceptDirect`: an upgraded direct socket from `peer` (admitted already), run as a direct connection (auth reason
    /// `unauthorized` on any failure).
    pub async fn accept_direct(&self, socket: WebSocketStream<TcpStream>, peer: SocketAddr) {
        drop(socket);
        let _ = peer;
    }

    /// `DisposeAsync`: the link and every connection ended; nothing restarts afterwards (L460).
    pub async fn dispose(&self) {}
}
