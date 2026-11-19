//! The relay host (C# `MobileRelayHost`'s lifecycle and control loop, R\MobileRelayHost.cs:54-560; plan §1.1): the settings applied
//! under one lifecycle lock with a generation counter, the control socket `<relay>/ws?serverId=…&role=server&v=1&hostToken=…` with
//! its keepalive and redial pacing (`policy`), a data socket per phone (`&connectionId=<id>`, never for a `direct-` id; ignored at 32
//! clients or 4 unauthenticated relay phones), each run by `connection`; the status (`status`); notify to every authenticated phone;
//! rotation (a new pairing key, every connection cancelled, push forgetting its views through the identity's rotation listeners); the
//! direct path's sockets handed in by W8-C's listener ([`crate::direct::listener::DirectSink`]: admission counted under the host's
//! table lock, the socket run as a direct connection); dispose. The relay URL is never logged. Owner: W8-A.
//!
//! Paired computers (the remote mirror, `remote::paired`) reach this same host: the link runs while the phone remote is on or while
//! this PC shares with paired computers ([`RelayHost::set_desktops`]); in the second case alone a phone is turned away as if the host
//! were offline (`connection::PHONES_OFF_CLOSE`), so turning on sharing never opens the phone's door.
//!
//! The direct listener's own lifecycle (Tailscale addresses, binding, the 15 s re-detection) is W8-C's (src-tauri `direct.rs` over
//! `direct::listener`); this host keeps C#'s half of it that touches the phone table: admission, the failed-auth lockout, the direct
//! phones dropped when the remote or its direct switch goes off, and the endpoints `auth_ok` and the pairing link carry.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, RwLock};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use mighty_core::model::MobileRemoteSettings;
use mighty_core::normalize;
use mighty_platform::{BoxFuture, NetworkCallback, Platform, Proxy, Subscription};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use crate::direct::listener::{DirectReservation, DirectSocket};
use crate::identity::{Identity, MobileDeviceInfo, escape_data};
use crate::relay::cipher::MAXIMUM_FRAME;
use crate::relay::connection::{Connection, ConnectionHost, RequestHandler};
use crate::relay::policy::{self, LinkAction, NetworkPath, Pacing, Window};
use crate::relay::status::{MobileRelayStatus, close_reason};

/// `MobileRelayHost.DefaultRelay`: the relay on Cloudflare Workers.
pub const DEFAULT_RELAY: &str = "wss://mighty-relay.athens96.workers.dev";
/// `RetiredDefaultRelays`: a relay field holding one falls back to [`DEFAULT_RELAY`].
pub const RETIRED_DEFAULT_RELAYS: [&str; 1] = ["wss://mightyclaude.duckdns.org"];
/// At most this many phones at once.
pub const MAXIMUM_CLIENTS: usize = 32;
/// `MaximumRelayUnauthenticated`: phones inside the handshake on the relay path (the direct path has a budget of its own).
pub const MAXIMUM_RELAY_UNAUTHENTICATED: usize = 4;
/// A direct connection's id starts so; relay notices never open or close one.
pub const DIRECT_PREFIX: &str = "direct-";

/// Why the host refused a call (C#'s `ObjectDisposedException`, `ArgumentException`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelayError {
    /// The host was disposed: nothing reopens.
    Disposed,
    /// No usable relay address.
    InvalidRelay,
    /// The identity could not be saved (a rotation).
    Unsaved,
}

impl std::fmt::Display for RelayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            RelayError::Disposed => "the relay host was shut down",
            RelayError::InvalidRelay => "Invalid relay address.",
            RelayError::Unsaved => "the pairing identity could not be saved",
        })
    }
}

impl std::error::Error for RelayError {}

/// `MobileRelayHost.NormalizeRelay` (mighty-core's `normalize_relay`).
pub fn normalize_relay(value: &str) -> Option<String> {
    normalize::normalize_relay(value)
}

/// `EffectiveRelay(user, fallback)`: the user's own address, or `fallback` when the field is empty, invalid or holds a built-in address
/// the app no longer ships (compared without case after normalizing). None when neither is usable.
pub fn effective_relay(user: Option<&str>, fallback: &str) -> Option<String> {
    if let Some(own) = normalize_relay(user.unwrap_or(""))
        && !RETIRED_DEFAULT_RELAYS.contains(&own.to_lowercase().as_str())
    {
        return Some(own);
    }
    normalize_relay(fallback)
}

/// The host's settings that are not the user's (C#'s constructor arguments and init properties, the checks' seams).
#[derive(Clone)]
pub struct RelayOptions {
    /// `auth_ok`'s `hostName` and the pairing link's `name` (C# `Environment.MachineName`).
    pub host_name: String,
    pub app_version: String,
    /// `auth_ok`'s capabilities (the router's `CAPABILITIES`).
    pub capabilities: Vec<String>,
    /// More capabilities read at each auth (`ExtraCapabilities`); `screenShare` is dropped while screen share is off.
    pub extra_capabilities: Option<Arc<dyn Fn() -> Vec<String> + Send + Sync>>,
    /// Whether the network port's address changes and resumes are followed (`WatchesNetwork`); off, only
    /// [`RelayHost::network_path_changed`] and [`RelayHost::reconnect_soon`] move the link (the checks).
    pub watches_network: bool,
    pub keepalive_interval: Duration,
    pub keepalive_timeout: Duration,
    pub path_settle: Duration,
    pub handshake_deadline: Duration,
    /// The direct path admits any peer, not only Tailscale's (C#'s `SetDirectAddressSource` seam: loopback in the checks; never set by
    /// the app).
    pub any_direct_peer: bool,
    /// Where the direct path's endpoints are read (src-tauri: W8-C's `direct::endpoints`); none: those set by
    /// [`RelayHost::set_direct_endpoints`].
    pub direct_endpoints: Option<Arc<dyn Fn() -> Vec<String> + Send + Sync>>,
    /// The runtime the host's tasks run on; none: the one [`RelayHost::with_options`] is called on. The network port's callbacks come
    /// on the OS's own threads, which have no runtime of their own (review M1).
    pub runtime: Option<tokio::runtime::Handle>,
}

impl Default for RelayOptions {
    fn default() -> Self {
        RelayOptions {
            host_name: String::new(),
            app_version: "0.0.0".into(),
            capabilities: vec![],
            extra_capabilities: None,
            watches_network: true,
            keepalive_interval: policy::KEEPALIVE_INTERVAL,
            keepalive_timeout: policy::KEEPALIVE_TIMEOUT,
            path_settle: policy::PATH_SETTLE,
            handshake_deadline: crate::direct::wire::HANDSHAKE_DEADLINE,
            any_direct_peer: false,
            direct_endpoints: None,
            runtime: None,
        }
    }
}

type StatusListener = Arc<dyn Fn(&MobileRelayStatus) + Send + Sync>;

struct Table {
    clients: HashMap<String, Arc<Connection>>,
    /// Direct sockets admitted and not yet handed over (their upgrade under way), each with when it was admitted: W8-C's path hands
    /// each back exactly once, with its socket ([`DirectSink::accept`]) or without ([`DirectSink::refused`]).
    reserved: Vec<Reservation>,
    next_reservation: u64,
}

/// One admitted direct socket's slot: its id (W8-C's `DirectReservation`, so a late hand-back cannot drop a newer reservation of the
/// same address) and its peer.
#[derive(Debug, Clone, Copy)]
struct Reservation {
    id: u64,
    peer: IpAddr,
}

struct Lifecycle {
    lifetime: Option<watch::Sender<bool>>,
    task: Option<JoinHandle<()>>,
}

#[derive(Default)]
struct PathState {
    watching: bool,
    network_path: Option<NetworkPath>,
    window: Window,
    pacing: Pacing,
    /// The settle wait now scheduled; a newer one makes it stale.
    settle: u64,
    subscriptions: Vec<Subscription>,
}

/// The host's shared half (what connections, the control loop and the path watcher hold).
pub(crate) struct Shared {
    identity: Arc<Identity>,
    handler: Arc<dyn RequestHandler>,
    platform: Platform,
    options: RelayOptions,
    lifecycle: tokio::sync::Mutex<Lifecycle>,
    settings: RwLock<MobileRemoteSettings>,
    generation: AtomicU64,
    disposed: AtomicBool,
    /// This PC shares with paired computers: the link runs even while the phone remote is off.
    desktops: AtomicBool,
    table: Mutex<Table>,
    /// C#'s `DirectAuthFailures` (W8-C's `direct::admit`).
    failures: crate::direct::admit::DirectAuthFailures,
    status: Mutex<MobileRelayStatus>,
    listeners: Mutex<Vec<(u64, StatusListener)>>,
    next_listener: AtomicU64,
    path: Mutex<PathState>,
    direct_endpoints: RwLock<Vec<String>>,
    epoch: Instant,
    /// Where the host's tasks are spawned (review M1).
    runtime: Option<tokio::runtime::Handle>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// One relay host for the app's lifetime (src-tauri `mobile.rs` makes it only when `Host::may_reach_relay`).
pub struct RelayHost {
    shared: Arc<Shared>,
}

/// A status listener's registration ([`RelayHost::on_status`]); dropping it unregisters.
pub struct StatusSubscription {
    shared: std::sync::Weak<Shared>,
    id: u64,
}

impl Drop for StatusSubscription {
    fn drop(&mut self) {
        if let Some(shared) = self.shared.upgrade() {
            lock(&shared.listeners).retain(|(id, _)| *id != self.id);
        }
    }
}

impl RelayHost {
    /// `new MobileRelayHost(directory, hostName, capabilities, route)` with the saved settings, not yet applied, and C#'s defaults.
    pub fn new(identity: Arc<Identity>, settings: MobileRemoteSettings, handler: Arc<dyn RequestHandler>, platform: Platform) -> RelayHost {
        RelayHost::with_options(identity, settings, handler, platform, RelayOptions::default())
    }

    /// [`RelayHost::new`] with the host's name, version and capabilities, and the checks' seams.
    pub fn with_options(
        identity: Arc<Identity>,
        settings: MobileRemoteSettings,
        handler: Arc<dyn RequestHandler>,
        platform: Platform,
        options: RelayOptions,
    ) -> RelayHost {
        // Not applied yet: C#'s `settings` starts as `new()` (off) until `ApplyAsync`.
        let _ = settings;
        RelayHost {
            shared: Arc::new(Shared {
                identity,
                handler,
                platform,
                options: options.clone(),
                lifecycle: tokio::sync::Mutex::new(Lifecycle { lifetime: None, task: None }),
                settings: RwLock::new(MobileRemoteSettings { enabled: false, ..MobileRemoteSettings::default() }),
                generation: AtomicU64::new(0),
                disposed: AtomicBool::new(false),
                desktops: AtomicBool::new(false),
                table: Mutex::new(Table { clients: HashMap::new(), reserved: vec![], next_reservation: 1 }),
                failures: crate::direct::admit::DirectAuthFailures::default(),
                status: Mutex::new(MobileRelayStatus::disabled()),
                listeners: Mutex::new(vec![]),
                next_listener: AtomicU64::new(1),
                path: Mutex::new(PathState::default()),
                direct_endpoints: RwLock::new(vec![]),
                epoch: Instant::now(),
                runtime: options.runtime.clone().or_else(|| tokio::runtime::Handle::try_current().ok()),
            }),
        }
    }

    /// `ApplyAsync`: the settings normalized and applied (the link started, redialled or stopped; the direct phones dropped when the
    /// remote or its direct switch went off).
    pub async fn apply(&self, settings: MobileRemoteSettings) -> Result<(), RelayError> {
        let shared = &self.shared;
        let lifecycle = shared.lifecycle.lock().await;
        if shared.disposed.load(Ordering::SeqCst) {
            return Err(RelayError::Disposed);
        }
        shared.clone().restart(normalize::mobile_remote(Some(&settings)), lifecycle).await
    }

    /// `ReconnectAsync`: the control socket replaced now (the Settings button).
    pub async fn reconnect(&self) -> Result<(), RelayError> {
        let shared = &self.shared;
        let lifecycle = shared.lifecycle.lock().await;
        if shared.disposed.load(Ordering::SeqCst) {
            return Err(RelayError::Disposed);
        }
        let settings = shared.settings();
        shared.clone().restart(settings, lifecycle).await
    }

    /// Whether paired computers keep the link up (this PC shares with them): on, the link runs even while the phone remote is off, and
    /// phones are then turned away as from an offline host; the phone remote's own settings are never changed. Off again, the link
    /// follows the phone remote alone.
    pub async fn set_desktops(&self, on: bool) -> Result<(), RelayError> {
        let shared = &self.shared;
        let lifecycle = shared.lifecycle.lock().await;
        if shared.disposed.load(Ordering::SeqCst) {
            return Err(RelayError::Disposed);
        }
        let before = shared.desktops.swap(on, Ordering::SeqCst);
        let settings = shared.settings();
        // The phone remote's link runs (and stays) either way.
        if before == on || settings.enabled {
            return Ok(());
        }
        shared.clone().restart(settings, lifecycle).await
    }

    /// Whether paired computers keep the link up ([`RelayHost::set_desktops`]).
    pub fn desktops(&self) -> bool {
        self.shared.desktops.load(Ordering::SeqCst)
    }

    /// Whether the link runs or should (the phone remote on, or this PC sharing with paired computers).
    pub fn link_on(&self) -> bool {
        self.shared.link_on(&self.shared.settings())
    }

    /// The names of the paired computers connected now (never their ids or tokens).
    pub fn connected_desktops(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .shared
            .clients()
            .into_iter()
            .filter(|c| c.authenticated())
            .filter_map(|c| c.device())
            .filter(|d| d.desktop)
            .map(|d| d.name)
            .collect();
        names.sort();
        names
    }

    /// `ReconnectSoon`: dials again once the path settles, whatever it then is (a wake from sleep: asleep, the PC missed the relay's
    /// pings). Never paced, and it still waits for a network; nothing while the remote is off.
    pub fn reconnect_soon(&self) {
        self.shared.reconnect_soon();
    }

    /// `NetworkPathChanged`: a path report (the network port's, or a check's), acted on once reports stop for the settle time.
    pub fn network_path_changed(&self, path: NetworkPath) {
        self.shared.network_path_changed(path);
    }

    /// `RotateKeyAsync`: with no device (키 다시 만들기), a new pairing key, every device and connection dropped. With a device (a
    /// row's 연결 해제), only that device is unpaired, while it is still paired ([`RelayHost::revoke`]). True when it changed anything.
    pub async fn rotate(&self, expected_device: Option<String>) -> Result<bool, RelayError> {
        if let Some(device) = expected_device {
            return self.revoke(&device).await;
        }
        let shared = &self.shared;
        {
            let _lifecycle = shared.lifecycle.lock().await;
            if shared.disposed.load(Ordering::SeqCst) {
                return Err(RelayError::Disposed);
            }
            shared.identity.rotate().map_err(|_| RelayError::Unsaved)?;
            for client in shared.clients() {
                client.cancel();
            }
        }
        shared.republish();
        Ok(true)
    }

    /// One device unpaired ([`Identity::revoke`]): its row and token gone, the pairing key made anew (the QR and link change), and only
    /// its connections closed (and the legacy apps', whose row goes with the old key); every other phone and computer stays paired and
    /// connected. False when it is not paired (a stale press: nothing changes).
    pub async fn revoke(&self, device: &str) -> Result<bool, RelayError> {
        let shared = &self.shared;
        {
            let _lifecycle = shared.lifecycle.lock().await;
            if shared.disposed.load(Ordering::SeqCst) {
                return Err(RelayError::Disposed);
            }
            let removed = shared.identity.revoke(device).map_err(|_| RelayError::Unsaved)?;
            if removed.is_empty() {
                return Ok(false);
            }
            for client in shared.clients() {
                if removed.contains(&client.device_id()) {
                    client.cancel();
                }
            }
        }
        shared.republish();
        Ok(true)
    }

    /// `Status`.
    pub fn status(&self) -> MobileRelayStatus {
        lock(&self.shared.status).clone()
    }

    /// `StatusChanged`: `listener` is told every status the host publishes (on the publishing task; it must return quickly).
    pub fn on_status(&self, listener: impl Fn(&MobileRelayStatus) + Send + Sync + 'static) -> StatusSubscription {
        let id = self.shared.next_listener.fetch_add(1, Ordering::SeqCst);
        lock(&self.shared.listeners).push((id, Arc::new(listener)));
        StatusSubscription { shared: Arc::downgrade(&self.shared), id }
    }

    /// `NotifyAsync(scope, revision)`: `{type:"notify", scope, revision}` to every authenticated phone (`state` or `session:<id>`).
    pub async fn notify(&self, scope: &str, revision: i64) {
        if scope != "state" && !scope.starts_with("session:") {
            return;
        }
        let mut out = mighty_core::json::JsonOut::new();
        out.begin_object();
        out.prop_str("type", Some("notify"));
        out.prop_str("scope", Some(scope));
        out.prop_int("revision", revision);
        out.end_object();
        let frame = out.finish().into_bytes();
        let clients: Vec<Arc<Connection>> = self.shared.clients().into_iter().filter(|c| c.authenticated()).collect();
        futures_util::future::join_all(clients.iter().map(|client| client.send(frame.clone()))).await;
    }

    /// `AcceptDirect`'s hand-over: the upgraded direct socket of the admission `reservation` (from [`DirectSink::admit`]), run as a
    /// direct connection (auth reason `unauthorized` on any failure) that must reach `auth_ok` by `deadline` (C#'s one 10 s from the
    /// accept, taken at admission). Answers once the connection has ended; dropping the future ends the connection, which leaves the
    /// table as an end does. A reservation this host does not hold (unknown, or given back already), or a socket arriving while the
    /// host is off or disposed, is dropped unread.
    pub async fn accept_direct(
        &self,
        reservation: DirectReservation,
        socket: WebSocketStream<TcpStream>,
        peer: SocketAddr,
        deadline: Instant,
    ) {
        let shared = self.shared.clone();
        let connection = {
            let mut table = lock(&shared.table);
            let Some(at) = table.reserved.iter().position(|r| r.id == reservation.id()) else { return };
            table.reserved.remove(at);
            let settings = shared.settings();
            if shared.disposed.load(Ordering::SeqCst) || !settings.enabled || !settings.direct_enabled {
                return;
            }
            let connection = Connection::new(format!("{DIRECT_PREFIX}{}", uuid::Uuid::new_v4()), true, Some(canonical(peer.ip())));
            table.clients.insert(connection.id.clone(), connection.clone());
            connection
        };
        let open: BoxFuture<'static, Option<WebSocketStream<TcpStream>>> = Box::pin(async move { Some(socket) });
        /// Ends the connection when the caller lets go of the future first.
        struct Ends(Arc<Connection>);
        impl Drop for Ends {
            fn drop(&mut self) {
                self.0.cancel();
            }
        }
        let _ends = Ends(connection.clone());
        let served = shared.spawn(shared.clone().serve_until(connection, open, deadline));
        let _ = served.await;
    }

    /// The reservations held now (the checks).
    pub fn direct_reservations(&self) -> usize {
        lock(&self.shared.table).reserved.len()
    }

    /// The direct path's endpoints now (W8-C's listener sets them), as `auth_ok`, `/m1/info` and the pairing link carry them.
    pub fn set_direct_endpoints(&self, endpoints: Vec<String>) {
        *self.shared.direct_endpoints.write().unwrap_or_else(|p| p.into_inner()) = endpoints;
    }

    pub fn direct_endpoints(&self) -> Vec<String> {
        self.shared.direct_endpoints()
    }

    /// `Devices`: the paired phones and computers with those connected now marked (a connected computer marked as one).
    pub fn devices(&self) -> Vec<MobileDeviceInfo> {
        let connected: Vec<_> = self.shared.clients().iter().filter(|c| c.authenticated()).filter_map(|c| c.device()).collect();
        self.shared
            .identity
            .devices()
            .into_iter()
            .map(|d| {
                let live: Vec<_> = connected.iter().filter(|c| c.client_id == d.id).collect();
                MobileDeviceInfo { connected: !live.is_empty(), desktop: d.desktop || live.iter().any(|c| c.desktop), ..d }
            })
            .collect()
    }

    /// `HostId`.
    pub fn host_id(&self) -> String {
        self.shared.identity.host_id()
    }

    /// `PairingServerId`.
    pub fn pairing_server_id(&self) -> String {
        self.shared.identity.server_id()
    }

    /// `PairingKeyForDisplay` (never logged; shown on the user's click).
    pub fn pairing_key_for_display(&self) -> zeroize::Zeroizing<String> {
        self.shared.identity.pairing_key()
    }

    /// `RelayForPush`: the relay this host dials; the push route sits beside it.
    pub fn relay_for_push(&self) -> String {
        self.shared.relay_for_push()
    }

    /// `Enabled`: the applied settings' switch.
    pub fn enabled(&self) -> bool {
        self.shared.settings().enabled
    }

    /// The applied settings.
    pub fn settings(&self) -> MobileRemoteSettings {
        self.shared.settings()
    }

    /// `PairingUrl`: the link (and QR) for the relay this host dials, its name and the direct endpoints.
    pub fn pairing_url(&self) -> String {
        self.shared.identity.pairing_url(&self.shared.relay_for_push(), &self.shared.options.host_name, &self.shared.direct_endpoints())
    }

    pub fn identity(&self) -> &Arc<Identity> {
        &self.shared.identity
    }

    /// The connection ids in the table now (the checks).
    pub fn connection_ids(&self) -> Vec<String> {
        lock(&self.shared.table).clients.keys().cloned().collect()
    }

    /// This host as W8-C's listener's sink.
    pub fn direct_sink(&self) -> Arc<dyn crate::direct::listener::DirectSink> {
        Arc::new(DirectHost(self.shared.clone()))
    }

    /// `DisposeAsync`: the link and every connection ended; nothing restarts afterwards (L460). Idempotent.
    pub async fn dispose(&self) {
        let shared = &self.shared;
        {
            let mut guard = shared.lifecycle.lock().await;
            if shared.disposed.swap(true, Ordering::SeqCst) {
                return;
            }
            shared.stop_watching();
            shared.stop_locked(&mut guard).await;
        }
        let all = shared.clients();
        for client in &all {
            client.cancel();
        }
        for client in all {
            client.completion().await;
        }
    }
}

impl Shared {
    fn settings(&self) -> MobileRemoteSettings {
        self.settings.read().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// The link runs while the phone remote is on or while paired computers are shared with.
    fn link_on(&self, settings: &MobileRemoteSettings) -> bool {
        settings.enabled || self.desktops.load(Ordering::SeqCst)
    }

    fn direct_endpoints(&self) -> Vec<String> {
        match &self.options.direct_endpoints {
            Some(source) => source(),
            None => self.direct_endpoints.read().unwrap_or_else(|p| p.into_inner()).clone(),
        }
    }

    fn relay_for_push(&self) -> String {
        effective_relay(self.settings().relay_url.as_deref(), DEFAULT_RELAY).unwrap_or_else(|| DEFAULT_RELAY.into())
    }

    fn clients(&self) -> Vec<Arc<Connection>> {
        lock(&self.table).clients.values().cloned().collect()
    }

    /// A task on the host's runtime, whatever thread asks (a network callback comes on the OS's own).
    fn spawn<F>(&self, future: F) -> tokio::task::JoinHandle<F::Output>
    where
        F: std::future::Future + Send + 'static,
        F::Output: Send + 'static,
    {
        match &self.runtime {
            Some(runtime) => runtime.spawn(future),
            None => tokio::spawn(future),
        }
    }

    fn now_ms(&self) -> u64 {
        self.epoch.elapsed().as_millis() as u64
    }

    fn publish(&self, connected: bool, detail: &str, error: Option<&str>, retry: Option<u32>) {
        let connections = self.clients().iter().filter(|c| c.authenticated()).count();
        let status = MobileRelayStatus::new(connected, connections, detail, error, retry);
        *lock(&self.status) = status.clone();
        let listeners: Vec<StatusListener> = lock(&self.listeners).iter().map(|(_, l)| l.clone()).collect();
        for listener in listeners {
            listener(&status);
        }
    }

    /// `Republish`: the same state again, with the phones counted anew.
    fn republish(&self) {
        let status = lock(&self.status).clone();
        self.publish(status.connected, &status.detail, status.error.as_deref(), status.retry_seconds);
    }

    /// `RestartInternal` (under the lifecycle lock, which `guard` is).
    async fn restart(
        self: Arc<Self>,
        value: MobileRemoteSettings,
        mut guard: tokio::sync::MutexGuard<'_, Lifecycle>,
    ) -> Result<(), RelayError> {
        self.stop_locked(&mut guard).await;
        *self.settings.write().unwrap_or_else(|p| p.into_inner()) = value.clone();
        // The direct path follows its own switch, not the relay's state: off (or the remote off), its phones go.
        if !value.enabled || !value.direct_enabled {
            let direct: Vec<Arc<Connection>> = self.clients().into_iter().filter(|c| c.direct).collect();
            lock(&self.table).reserved.clear();
            for client in direct {
                client.cancel();
            }
        }
        if !self.link_on(&value) {
            self.stop_watching();
            self.publish(false, "disabled", None, None);
            return Ok(());
        }
        let relay = effective_relay(value.relay_url.as_deref(), DEFAULT_RELAY).ok_or(RelayError::InvalidRelay)?;
        let (lifetime, cancelled) = watch::channel(false);
        let current = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.publish(false, "connecting", None, None);
        let shared = self.clone();
        guard.task = Some(self.spawn(async move { shared.run(relay, current, cancelled).await }));
        guard.lifetime = Some(lifetime);
        drop(guard);
        self.start_watching();
        Ok(())
    }

    /// `StopInternal`: the relay link and the relay's phones stopped; the direct phones stay.
    async fn stop_locked(&self, guard: &mut tokio::sync::MutexGuard<'_, Lifecycle>) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        if let Some(lifetime) = guard.lifetime.take() {
            lifetime.send_replace(true);
        }
        let relay: Vec<Arc<Connection>> = self.clients().into_iter().filter(|c| !c.direct).collect();
        for client in &relay {
            client.cancel();
        }
        if let Some(task) = guard.task.take() {
            let _ = task.await;
        }
        for client in relay {
            client.completion().await;
        }
    }

    /// The control socket's address (`SocketUri`): never logged.
    fn socket_url(&self, relay: &str, connection: Option<&str>) -> String {
        let mut query = format!(
            "serverId={}&role=server&v=1&hostToken={}",
            escape_data(&self.identity.server_id()),
            escape_data(&self.identity.host_token())
        );
        if let Some(connection) = connection {
            query.push_str("&connectionId=");
            query.push_str(&escape_data(connection));
        }
        format!("{relay}/ws?{query}")
    }

    /// `RunAsync`: the control loop of one generation — dial, keep alive, open a data socket per `connected` notice, close one per
    /// `disconnected`; after a socket ends, say why and redial after 1 s doubling to 30 s (reset after a connect).
    async fn run(self: Arc<Self>, relay: String, current: u64, mut cancelled: watch::Receiver<bool>) {
        let mut delay: u32 = 1;
        while !*cancelled.borrow() && current == self.generation.load(Ordering::SeqCst) {
            let mut connected = false;
            let mut close: Option<u16> = None;
            let url = self.socket_url(&relay, None);
            let dialled = tokio::select! {
                dialled = dial(&url, self.platform.proxy.as_ref()) => dialled.ok(),
                _ = until_true(&mut cancelled) => None,
            };
            if let Some(mut socket) = dialled {
                connected = true;
                delay = 1;
                self.publish(true, "connected", None, None);
                close = self.control(&mut socket, &relay, &mut cancelled).await;
            }
            // The relay's phones end with their control socket.
            let ended: Vec<Arc<Connection>> = self.clients().into_iter().filter(|c| !c.direct).collect();
            for client in &ended {
                client.cancel();
            }
            for client in ended {
                client.completion().await;
            }
            if *cancelled.borrow() || current != self.generation.load(Ordering::SeqCst) {
                break;
            }
            // The relay says why it closed; a socket that ended without a word had stopped answering.
            self.publish(false, "reconnecting", Some(close_reason(close, connected)), Some(delay));
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(delay as u64)) => {}
                _ = until_true(&mut cancelled) => break,
            }
            delay = policy::next_reconnect(delay);
        }
        self.publish(false, "disabled", None, None);
    }

    /// One control socket until it ends: its close code, None when it ended without one (a ping unanswered, an error, the stream's
    /// end, or this generation cancelled).
    async fn control(
        self: &Arc<Self>,
        socket: &mut WebSocketStream<MaybeTlsStream<TcpStream>>,
        relay: &str,
        cancelled: &mut watch::Receiver<bool>,
    ) -> Option<u16> {
        let interval = self.options.keepalive_interval.max(Duration::from_millis(1));
        let mut ping = tokio::time::interval_at(Instant::now() + interval, interval);
        let mut awaiting: Option<Instant> = None;
        loop {
            let deadline = awaiting.unwrap_or_else(|| Instant::now() + Duration::from_secs(86_400));
            tokio::select! {
                _ = until_true(cancelled) => return None,
                _ = ping.tick() => {
                    if socket.send(Message::Ping(Vec::new().into())).await.is_err() {
                        return None;
                    }
                    awaiting.get_or_insert(Instant::now() + self.options.keepalive_timeout);
                }
                _ = tokio::time::sleep_until(deadline), if awaiting.is_some() => return None,
                message = socket.next() => match message {
                    Some(Ok(Message::Pong(_))) => awaiting = None,
                    Some(Ok(Message::Text(text))) => {
                        if !self.notice(text.as_str(), relay) {
                            return None;
                        }
                    }
                    Some(Ok(Message::Close(frame))) => return frame.map(|f| u16::from(f.code)),
                    Some(Ok(_)) => {}
                    Some(Err(_)) | None => return None,
                },
            }
        }
    }

    /// One control notice: `connected` opens a data socket for its connection id (unless the table is full), `disconnected` closes
    /// it; a `direct-` id is never the relay's to open or close. False for a notice that is not JSON (the socket is replaced).
    fn notice(self: &Arc<Self>, text: &str, relay: &str) -> bool {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else { return false };
        let kind = value.get("type").and_then(serde_json::Value::as_str);
        let Some(id) = value.get("connectionId").and_then(serde_json::Value::as_str).filter(|id| connection_id(id)) else { return true };
        let direct = id.starts_with(DIRECT_PREFIX);
        if kind == Some("disconnected") {
            if !direct && let Some(prior) = lock(&self.table).clients.get(id).cloned() {
                prior.cancel();
            }
            return true;
        }
        if kind != Some("connected") || direct {
            return true;
        }
        let connection = {
            let mut table = lock(&self.table);
            let unauthenticated = table.clients.values().filter(|c| !c.authenticated() && !c.direct).count();
            if table.clients.len() >= MAXIMUM_CLIENTS || unauthenticated >= MAXIMUM_RELAY_UNAUTHENTICATED || table.clients.contains_key(id)
            {
                return true;
            }
            let connection = Connection::new(id.to_string(), false, None);
            table.clients.insert(id.to_string(), connection.clone());
            connection
        };
        let url = self.socket_url(relay, Some(id));
        let shared = self.clone();
        let open: BoxFuture<'static, Option<WebSocketStream<MaybeTlsStream<TcpStream>>>> =
            Box::pin(async move { dial(&url, shared.platform.proxy.as_ref()).await.ok() });
        let shared = self.clone();
        self.spawn(async move { shared.serve(connection, open).await });
        true
    }

    /// `Serve`: the connection run, then taken out of the table and the phones counted anew.
    async fn serve<S>(self: Arc<Self>, connection: Arc<Connection>, open: BoxFuture<'static, Option<WebSocketStream<S>>>)
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
    {
        let deadline = Instant::now() + self.options.handshake_deadline;
        self.serve_until(connection, open, deadline).await;
    }

    /// [`Shared::serve`] with the handshake's deadline given.
    async fn serve_until<S>(
        self: Arc<Self>,
        connection: Arc<Connection>,
        open: BoxFuture<'static, Option<WebSocketStream<S>>>,
        deadline: Instant,
    ) where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
    {
        let run = self.spawn(connection.clone().run(self.clone(), open, deadline));
        let _ = run.await;
        {
            let mut table = lock(&self.table);
            if table.clients.get(&connection.id).is_some_and(|c| Arc::ptr_eq(c, &connection)) {
                table.clients.remove(&connection.id);
            }
        }
        connection.finish();
        self.republish();
    }

    /// `StartWatching`: the network port followed while the remote is on.
    fn start_watching(self: &Arc<Self>) {
        {
            let mut path = lock(&self.path);
            if path.watching {
                return;
            }
            *path = PathState { watching: true, ..PathState::default() };
            if !self.options.watches_network {
                return;
            }
        }
        let network = self.platform.network.clone();
        let weak = Arc::downgrade(self);
        let on_change = NetworkCallback::new({
            let weak = weak.clone();
            move || {
                if let Some(shared) = weak.upgrade() {
                    shared.read_path();
                }
            }
        });
        let on_resume = NetworkCallback::new(move || {
            if let Some(shared) = weak.upgrade() {
                shared.reconnect_soon();
            }
        });
        let mut subscriptions = vec![];
        if let Ok(subscription) = network.on_change(on_change) {
            subscriptions.push(subscription);
        }
        if let Ok(subscription) = network.on_resume(on_resume) {
            subscriptions.push(subscription);
        }
        lock(&self.path).subscriptions = subscriptions;
        self.read_path();
    }

    /// Reports the path as the network port reads it now (off the caller's thread: reading the adapters takes a while); one that could
    /// not be read is no report at all.
    fn read_path(self: &Arc<Self>) {
        let shared = self.clone();
        self.spawn(async move {
            let network = shared.platform.network.clone();
            if let Ok(Ok(adapters)) = tokio::task::spawn_blocking(move || network.adapters()).await {
                shared.network_path_changed(NetworkPath::from_adapters(&adapters));
            }
        });
    }

    fn stop_watching(&self) {
        let subscriptions = {
            let mut path = lock(&self.path);
            let subscriptions = std::mem::take(&mut path.subscriptions);
            let settle = path.settle + 1;
            *path = PathState { settle, ..PathState::default() };
            subscriptions
        };
        drop(subscriptions);
    }

    fn network_path_changed(self: &Arc<Self>, report: NetworkPath) {
        let mut path = lock(&self.path);
        if !path.watching {
            return;
        }
        let now = self.now_ms();
        let settled = path.network_path.clone();
        path.window.report(report, settled.as_ref(), now);
        self.schedule_settle(&mut path, None);
    }

    fn reconnect_soon(self: &Arc<Self>) {
        let mut path = lock(&self.path);
        if !path.watching {
            return;
        }
        let now = self.now_ms();
        path.window.force(now);
        self.schedule_settle(&mut path, None);
    }

    /// `ScheduleSettle` (under the path lock): the decision after the settle delay (or `after`), off this caller.
    fn schedule_settle(self: &Arc<Self>, path: &mut PathState, after: Option<Duration>) {
        path.settle += 1;
        let ticket = path.settle;
        let delay = after.unwrap_or_else(|| path.window.delay(self.now_ms()).min(self.options.path_settle));
        let shared = self.clone();
        self.spawn(async move { shared.settle(delay, ticket).await });
    }

    async fn settle(self: Arc<Self>, delay: Duration, ticket: u64) {
        tokio::time::sleep(delay).await;
        let action = {
            let mut path = lock(&self.path);
            if !path.watching || path.settle != ticket || path.window.is_empty() {
                return;
            }
            let window = path.window.clone();
            let action = window.action(path.network_path.as_ref());
            let now = self.now_ms();
            // A flapping link must not redial on every flap. The window stays open meanwhile, so later reports still count.
            let pause = path.pacing.wait(now);
            if action == LinkAction::Reconnect && !window.forced && !pause.is_zero() {
                self.schedule_settle(&mut path, Some(pause));
                return;
            }
            path.window = Window::default();
            if let Some(latest) = window.latest {
                path.network_path = Some(latest);
            }
            if action == LinkAction::Reconnect && !window.forced {
                path.pacing.record(now);
            }
            action
        };
        if action == LinkAction::None {
            return;
        }
        let mut guard = self.lifecycle.lock().await;
        if self.disposed.load(Ordering::SeqCst) || !self.link_on(&self.settings()) {
            return;
        }
        if action == LinkAction::Offline {
            self.stop_locked(&mut guard).await;
            self.publish(false, "offline", None, None);
        } else {
            let settings = self.settings();
            let _ = self.clone().restart(settings, guard).await;
        }
    }

    /// `MobileDirect.Admit` under the table lock (W8-C's `direct::admit::admit`; the listener refused a peer outside Tailscale's ranges
    /// already, but the check runs here too: any peer only through the checks' seam, review L3): room in the table, fewer than four
    /// handshakes on the direct path and two from this address, fewer than five failed auths from it in the last minute. The
    /// reservation holds the slot until the path hands it back.
    fn admit_direct(&self, peer: IpAddr) -> Option<DirectReservation> {
        let peer = canonical(peer);
        let settings = self.settings();
        if self.disposed.load(Ordering::SeqCst) || !settings.enabled || !settings.direct_enabled {
            return None;
        }
        let text = crate::tailscale::canonical(peer);
        let failures = self.failures.recent(&text, std::time::Instant::now());
        let mut table = lock(&self.table);
        let mut pending: Vec<String> = table
            .clients
            .values()
            .filter(|c| c.direct && !c.authenticated())
            .map(|c| c.peer.map(crate::tailscale::canonical).unwrap_or_default())
            .collect();
        pending.extend(table.reserved.iter().map(|r| crate::tailscale::canonical(r.peer)));
        let clients = table.clients.len() + table.reserved.len();
        if !crate::direct::admit::admit(Some(&text), self.options.any_direct_peer, clients, &pending, failures) {
            return None;
        }
        let id = table.next_reservation;
        table.next_reservation += 1;
        table.reserved.push(Reservation { id, peer });
        Some(DirectReservation::new(id))
    }

    /// `reservation` given back (its upgrade did not happen); one this host does not hold is ignored.
    fn refused_direct(&self, reservation: DirectReservation) {
        let mut table = lock(&self.table);
        if let Some(at) = table.reserved.iter().position(|r| r.id == reservation.id()) {
            table.reserved.remove(at);
        }
    }
}

impl ConnectionHost for Shared {
    fn identity(&self) -> &Identity {
        &self.identity
    }
    fn allow_legacy(&self) -> bool {
        self.settings().allow_legacy_phones
    }
    fn handler(&self) -> Arc<dyn RequestHandler> {
        self.handler.clone()
    }
    fn host_name(&self) -> String {
        self.options.host_name.clone()
    }
    fn app_version(&self) -> String {
        self.options.app_version.clone()
    }
    fn capabilities(&self) -> Vec<String> {
        // `ScreenShareAvailability.Extra`: the extras, `screenShare` dropped while screen share is off.
        let extra = self.options.extra_capabilities.as_ref().map(|extra| extra()).unwrap_or_default();
        let mut all: Vec<String> = vec![];
        for capability in
            self.options.capabilities.iter().cloned().chain(extra.into_iter().filter(|c| crate::screen::ENABLED || c != "screenShare"))
        {
            if !all.contains(&capability) {
                all.push(capability);
            }
        }
        all
    }
    fn direct_endpoints(&self) -> Vec<String> {
        Shared::direct_endpoints(self)
    }
    fn record_direct_failure(&self, peer: IpAddr) {
        self.failures.record(&crate::tailscale::canonical(canonical(peer)), std::time::Instant::now());
    }
    fn authenticated(&self) {
        self.republish();
    }
    fn phones_allowed(&self) -> bool {
        self.settings().enabled
    }
}

/// The relay host as W8-C's listener's sink.
struct DirectHost(Arc<Shared>);

impl crate::direct::listener::DirectSink for DirectHost {
    fn admit(&self, peer: IpAddr) -> Option<DirectReservation> {
        self.0.admit_direct(peer)
    }

    fn accept(&self, reservation: DirectReservation, socket: DirectSocket, peer: SocketAddr, deadline: Instant) -> BoxFuture<'static, ()> {
        let host = RelayHost { shared: self.0.clone() };
        Box::pin(async move { host.accept_direct(reservation, socket, peer, deadline).await })
    }

    fn refused(&self, reservation: DirectReservation) {
        self.0.refused_direct(reservation);
    }
}

/// A mapped IPv6 address counts as its IPv4 one.
fn canonical(address: IpAddr) -> IpAddr {
    match address {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(address),
        v4 => v4,
    }
}

/// `^[A-Za-z0-9-]{8,64}$` (.NET's `$` taking a final line feed).
fn connection_id(id: &str) -> bool {
    let rule = |v: &str| (8..=64).contains(&v.len()) && v.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
    rule(id) || id.strip_suffix('\n').is_some_and(rule)
}

/// Why a dial failed, as one word a log may carry (tungstenite's and native-tls's own texts can carry the request, so never them).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialError {
    Address,
    Proxy,
    Connect,
    Tls,
    Http(u16),
    Protocol,
}

impl std::fmt::Display for DialError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DialError::Address => f.write_str("address"),
            DialError::Proxy => f.write_str("proxy"),
            DialError::Connect => f.write_str("connect"),
            DialError::Tls => f.write_str("tls"),
            DialError::Http(status) => write!(f, "http {status}"),
            DialError::Protocol => f.write_str("protocol"),
        }
    }
}

fn kind(error: &tokio_tungstenite::tungstenite::Error) -> DialError {
    use tokio_tungstenite::tungstenite::Error;
    match error {
        Error::Io(_) | Error::ConnectionClosed | Error::AlreadyClosed => DialError::Connect,
        Error::Tls(_) => DialError::Tls,
        Error::Http(response) => DialError::Http(response.status().as_u16()),
        Error::Url(_) => DialError::Address,
        _ => DialError::Protocol,
    }
}

/// The WebSocket's limits: a message or frame over 1 MiB ends the socket, as C#'s `Receive` refuses one.
pub fn socket_config() -> WebSocketConfig {
    WebSocketConfig::default().max_message_size(Some(MAXIMUM_FRAME)).max_frame_size(Some(MAXIMUM_FRAME))
}

/// Dials a `ws://` or `wss://` URL as .NET's `ClientWebSocket` does: through the proxy the system names for it (a `CONNECT` tunnel),
/// else straight; TLS by native-tls (the system's trust store). The URL is never logged nor put into the error.
pub async fn dial(url: &str, proxy: &dyn mighty_platform::ProxyResolver) -> Result<WebSocketStream<MaybeTlsStream<TcpStream>>, DialError> {
    let parsed = url::Url::parse(url).map_err(|_| DialError::Address)?;
    let default = match parsed.scheme() {
        "ws" => 80,
        "wss" => 443,
        _ => return Err(DialError::Address),
    };
    let host = parsed.host_str().filter(|h| !h.is_empty()).ok_or(DialError::Address)?.to_string();
    let port = parsed.port().unwrap_or(default);
    // A system proxy that cannot be read is no proxy (.NET goes straight then).
    let route = proxy.resolve(url).await.unwrap_or(Proxy::Direct);
    let tcp = match route {
        Proxy::Direct => TcpStream::connect(format!("{host}:{port}")).await.map_err(|_| DialError::Connect)?,
        Proxy::Http { host: proxy_host, port: proxy_port } => {
            let mut tunnel = TcpStream::connect(format!("{proxy_host}:{proxy_port}")).await.map_err(|_| DialError::Proxy)?;
            connect_tunnel(&mut tunnel, &host, port).await?;
            tunnel
        }
    };
    let _ = tcp.set_nodelay(true);
    let (socket, _) =
        tokio_tungstenite::client_async_tls_with_config(url, tcp, Some(socket_config()), None).await.map_err(|error| kind(&error))?;
    Ok(socket)
}

/// `CONNECT host:port HTTP/1.1` through an HTTP proxy, answered 2xx before anything else is sent.
async fn connect_tunnel(stream: &mut TcpStream, host: &str, port: u16) -> Result<(), DialError> {
    let request = format!("CONNECT {host}:{port} HTTP/1.1\r\nHost: {host}:{port}\r\n\r\n");
    stream.write_all(request.as_bytes()).await.map_err(|_| DialError::Proxy)?;
    let mut head = Vec::with_capacity(256);
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if head.len() > 16 * 1024 || stream.read(&mut byte).await.map_err(|_| DialError::Proxy)? == 0 {
            return Err(DialError::Proxy);
        }
        head.push(byte[0]);
    }
    let line = String::from_utf8_lossy(&head);
    let status = line.split_whitespace().nth(1).and_then(|s| s.parse::<u16>().ok()).ok_or(DialError::Proxy)?;
    if (200..300).contains(&status) { Ok(()) } else { Err(DialError::Http(status)) }
}

/// Waits until the flag reads true (or its sender is gone); the borrow it takes ends before this returns, so a `select!` over it stays
/// `Send`.
async fn until_true(flag: &mut watch::Receiver<bool>) {
    let _ = flag.wait_for(|set| *set).await;
}
