//! The direct listeners and the direct path (C# `MobileDirectListener`, `DirectIncoming`, R\MobileDirectListener.cs; the direct half of
//! `MobileRelayHost`: `RefreshDirect`, `ReadDirect`, `ApplyDirect`, `BindDirect`, `StopDirect`, the watch and `AcceptDirect`,
//! R\MobileRelayHost.cs:118-317; plan §1.4). Owner: W8-C.
//!
//! - [`DirectListener`]: one listener per Tailscale address through mighty-platform's `Network::bind_exclusive` (never a wildcard; the
//!   checks bind loopback port 0 through the fake), together holding at most 36 connections (one over is reset before anything is read).
//! - [`DirectPath`]: listens while the remote and its direct switch are on, on the addresses Tailscale has now — read off the caller and
//!   outside its lock (L503), again every 15 s and on every address change —, keeps the listeners (and the phones on them) whose address
//!   and port stay, says what it is doing ([`MobileDirectStatus`]) and advertises its endpoints. Each accepted socket from a Tailscale
//!   peer is offered to the relay host ([`DirectSink::admit`]: its counts, one check-and-insert under its lock), upgraded within the
//!   handshake deadline and handed over ([`DirectSink::accept`]); anything refused is reset unread. Its phones live apart from the relay's:
//!   a relay redial never touches them; the switch or the remote turned off ends them.
//!
//! src-tauri's `direct.rs` builds the path only when the app may listen (`Host::may_listen`); the relay host (W8-A) is its sink.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Duration;

use mighty_platform::{BoxFuture, Network, NetworkCallback, Subscription};
use tokio::net::{TcpListener, TcpStream};
use tokio::runtime::Handle;
use tokio::task::{AbortHandle, JoinHandle};
use tokio_tungstenite::WebSocketStream;

use crate::direct::status::{self, DirectState, MobileDirectStatus};
use crate::direct::wire::{self, BACKLOG, CONNECTION_LIMIT};
use crate::tailscale;

/// Where admitted, upgraded sockets go (W8-A's relay host).
///
/// The listener has already refused a peer outside Tailscale's ranges (a loopback peer only through the checks' address seam), so
/// `admit` judges the counts: C#'s `MobileDirect.Admit` as one check-and-insert under the host's lock — room in the phone table (relay
/// and direct), fewer than 4 direct handshakes and 2 from `peer`, fewer than 5 failed auths from it in 60 s
/// (`direct::admit::{admit, DirectAuthFailures}`; call `admit(Some(&canonical(peer)), true, …)`). An admission is a reservation of an
/// unauthenticated slot with an id of the sink's own ([`DirectReservation`]); it is handed over exactly once — to `accept` with the
/// upgraded socket, or back through `refused` when the upgrade failed, ran out of time or the path stopped first — so a late `refused`
/// can only ever give back its own reservation, never a newer one of the same address.
pub trait DirectSink: Send + Sync {
    /// `MobileDirect.Admit` for a socket just accepted from `peer` (canonical: an IPv4-mapped address as its IPv4): its reservation, or
    /// none when it is refused (the socket is then reset unread).
    fn admit(&self, peer: IpAddr) -> Option<DirectReservation>;
    /// The upgraded socket of an admitted peer, run as a direct connection (id `direct-…`, the same handshake, auth and request limits
    /// as the relay's; a refused auth says only [`wire::GENERIC_REFUSAL`] and counts as a failure of `peer`). One deadline covers the
    /// socket, the upgrade, hello and auth (C#'s 10 s from the accept to auth_ok): `deadline` was taken when the socket was accepted, and
    /// the connection must be authenticated by then (`tokio::time::timeout_at(deadline, …)`) or ended. The future runs the connection to
    /// its end; dropping it ends the connection (the path drops it when the switch, the remote or the sink goes, or the app quits), and
    /// must leave the host's table as if it had ended.
    fn accept(
        &self,
        reservation: DirectReservation,
        socket: DirectSocket,
        peer: SocketAddr,
        deadline: tokio::time::Instant,
    ) -> BoxFuture<'static, ()>;
    /// An admitted peer's reservation given back: its upgrade did not happen (no phone's upgrade, the connection ended, the deadline).
    fn refused(&self, reservation: DirectReservation);
}

/// One admission's reservation (C#'s pending entry in the phone table), identified by an id the sink chose. It is not `Clone`: it is
/// handed back exactly once, to [`DirectSink::accept`] or [`DirectSink::refused`].
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct DirectReservation(u64);

impl DirectReservation {
    pub fn new(id: u64) -> DirectReservation {
        DirectReservation(id)
    }

    pub fn id(&self) -> u64 {
        self.0
    }
}

/// An upgraded direct socket, as [`DirectSink::accept`] gets it.
pub type DirectSocket = WebSocketStream<TcpStream>;

/// What the path logs (the OS's own words for a bind or read failure; never shown).
pub type DirectLog = Arc<dyn Fn(&str) + Send + Sync>;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// One of the listeners' connection slots, given back once when dropped.
struct Slot(Arc<AtomicUsize>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// C# `DirectIncoming`: one connection a listener delivered. It holds one of the listeners' slots until it is refused or dropped
/// (with the connection it became), and gives it back once.
pub struct DirectIncoming {
    stream: TcpStream,
    remote: Option<SocketAddr>,
    slot: Slot,
}

impl DirectIncoming {
    /// `DirectIncoming.PeerOf`: the remote IP, an IPv4-mapped one as its IPv4 (one spelling per address, so the per-address limits never
    /// split across forms of one address).
    pub fn peer_address(&self) -> Option<IpAddr> {
        self.remote.map(|remote| tailscale::peer(remote.ip()))
    }

    /// The remote IP in its canonical spelling (lowercase, compressed, no zone).
    pub fn peer(&self) -> Option<String> {
        self.peer_address().map(tailscale::canonical)
    }

    /// The remote endpoint with [`DirectIncoming::peer_address`] for its IP.
    pub fn remote(&self) -> Option<SocketAddr> {
        self.remote.map(|remote| SocketAddr::new(tailscale::peer(remote.ip()), remote.port()))
    }

    /// The connection's stream; its slot goes back when the stream's owner drops the returned guard.
    pub fn into_parts(self) -> (TcpStream, impl Send + 'static) {
        (self.stream, self.slot)
    }

    /// Drops the connection unread: a reset (linger 0), nothing said; the slot goes back as it goes.
    pub fn refuse(self) {
        let _ = self.stream.set_zero_linger();
    }
}

struct Entry {
    host: String,
    port: u16,
    accept: JoinHandle<()>,
}

/// Accept loops just aborted, by `address|port`: an abort ends the loop (and closes its socket) on a worker a moment later, so a bind
/// of the same address and port waits for it first instead of failing as in use.
static CLOSING: Mutex<Vec<(String, JoinHandle<()>)>> = Mutex::new(Vec::new());

/// Aborts an accept loop and keeps its handle until it has ended.
fn close(key: String, accept: JoinHandle<()>) {
    accept.abort();
    let mut closing = lock(&CLOSING);
    closing.retain(|(_, handle)| !handle.is_finished());
    closing.push((key, accept));
}

/// Waits (at most a second) for the accept loops of `key` that were just aborted to end, so their socket is closed.
fn wait_closed(key: &str) {
    let started = std::time::Instant::now();
    loop {
        let open = {
            let mut closing = lock(&CLOSING);
            closing.retain(|(_, handle)| !handle.is_finished());
            closing.iter().any(|(closing, _)| closing == key)
        };
        if !open || started.elapsed() > Duration::from_secs(1) {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[derive(Default)]
struct ListenerState {
    listeners: HashMap<String, Entry>,
    stopped: bool,
}

/// Called with each connection a listener delivers (C# `onConnection`): it refuses it, or keeps it running.
pub type OnConnection = Arc<dyn Fn(DirectIncoming) + Send + Sync>;

struct ListenerInner {
    network: Arc<dyn Network>,
    runtime: Handle,
    on_connection: OnConnection,
    log: DirectLog,
    state: Mutex<ListenerState>,
    live: Arc<AtomicUsize>,
}

/// C# `MobileDirectListener`: one listener per address, bound to that address alone (never a wildcard), with exclusive use of the port
/// on Windows (`Network::bind_exclusive`); together they hold at most [`CONNECTION_LIMIT`] connections. Dropping it stops it.
pub struct DirectListener {
    inner: Arc<ListenerInner>,
}

fn key(address: &str, port: u16) -> String {
    format!("{address}|{port}")
}

impl DirectListener {
    pub fn new(network: Arc<dyn Network>, runtime: Handle, on_connection: OnConnection, log: DirectLog) -> DirectListener {
        DirectListener {
            inner: Arc::new(ListenerInner {
                network,
                runtime,
                on_connection,
                log,
                state: Mutex::default(),
                live: Arc::new(AtomicUsize::new(0)),
            }),
        }
    }

    /// Connections delivered and not yet ended, across every listener.
    pub fn live_connections(&self) -> usize {
        self.inner.live.load(Ordering::SeqCst)
    }

    /// `Update`: makes the listeners match `addresses` on `port` — stops the ones no longer wanted, keeps the ones already up (and the
    /// phones on them), binds the rest. Port 0 picks a free port, kept on later updates with 0. Answers every listener up afterwards with
    /// its port, and the first error as a code (`status::*`; the OS's own words go to the log only).
    pub fn update(&self, addresses: &[String], port: u16) -> (Vec<(String, u16)>, Option<&'static str>) {
        let inner = &self.inner;
        let mut state = lock(&inner.state);
        if state.stopped {
            return (vec![], None);
        }
        let wanted: std::collections::HashSet<String> = addresses.iter().map(|address| key(address, port)).collect();
        let gone: Vec<String> = state.listeners.keys().filter(|key| !wanted.contains(*key)).cloned().collect();
        for key in gone {
            if let Some(entry) = state.listeners.remove(&key) {
                close(key, entry.accept);
            }
        }
        let mut error = None;
        for address in addresses {
            let key = key(address, port);
            if state.listeners.contains_key(&key) {
                continue;
            }
            let Ok(ip) = address.parse::<IpAddr>() else {
                error = error.or(Some(status::BIND_FAILED));
                continue;
            };
            if port != 0 {
                wait_closed(&key);
            }
            let bound = inner.network.bind_exclusive(SocketAddr::new(ip, port), BACKLOG).and_then(|listener| {
                listener.set_nonblocking(true)?;
                let _entered = inner.runtime.enter();
                Ok(TcpListener::from_std(listener)?)
            });
            let listener = match bound {
                Ok(listener) => listener,
                Err(failure) => {
                    error = error.or(Some(status::error_of(&failure)));
                    (inner.log)(&format!("mobile direct: an address could not be listened on: {failure}"));
                    continue;
                }
            };
            let bound_port = listener.local_addr().map(|a| a.port()).unwrap_or(port);
            let accept = inner.runtime.spawn(accept_loop(Arc::downgrade(inner), listener));
            state.listeners.insert(key, Entry { host: address.clone(), port: bound_port, accept });
        }
        let bound = addresses
            .iter()
            .filter_map(|address| state.listeners.get(&key(address, port)).map(|entry| (entry.host.clone(), entry.port)))
            .collect();
        (bound, error)
    }

    /// Stops accepting. Connections already delivered stay up; their owners end them.
    pub fn stop(&self) {
        let mut state = lock(&self.inner.state);
        state.stopped = true;
        for (key, entry) in state.listeners.drain() {
            close(key, entry.accept);
        }
    }
}

impl Drop for DirectListener {
    fn drop(&mut self) {
        self.stop();
    }
}

async fn accept_loop(inner: Weak<ListenerInner>, listener: TcpListener) {
    loop {
        let accepted = listener.accept().await;
        let Some(inner) = inner.upgrade() else { return };
        match accepted {
            Ok((socket, remote)) => deliver(&inner, socket, Some(remote)),
            Err(_) => {
                drop(inner);
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

/// `Deliver`: the slot is taken before anything else; one over the limit, or one after the stop, is reset unread; a handler that panics
/// has its connection dropped. Every path gives the slot back exactly once (the incoming's own drop).
fn deliver(inner: &Arc<ListenerInner>, socket: TcpStream, remote: Option<SocketAddr>) {
    let count = inner.live.fetch_add(1, Ordering::SeqCst) + 1;
    let incoming = DirectIncoming { stream: socket, remote, slot: Slot(inner.live.clone()) };
    let refuse = lock(&inner.state).stopped || count > CONNECTION_LIMIT;
    if refuse {
        incoming.refuse();
        return;
    }
    let handler = inner.on_connection.clone();
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || handler(incoming))).is_err() {
        (inner.log)("mobile direct: a connection's handler failed");
    }
}

/// Where the path reads its addresses instead of this PC's Tailscale adapters (C#'s internal `SetDirectAddressSource` seam: loopback in
/// the checks and the smoke, never in the app): `Err` stands for adapters that cannot be read. Only loopback addresses are taken from
/// it ([`DirectPath::set_address_source`]).
pub type AddressSource = Arc<dyn Fn() -> Result<Vec<String>, String> + Send + Sync>;

/// What the path is told of the saved settings (C# `MobileRemoteSettings`' direct half).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirectSettings {
    pub enabled: bool,
    pub direct_enabled: bool,
    pub port: u16,
}

impl Default for DirectSettings {
    fn default() -> DirectSettings {
        DirectSettings { enabled: false, direct_enabled: true, port: wire::DEFAULT_PORT }
    }
}

impl DirectSettings {
    fn on(&self) -> bool {
        self.enabled && self.direct_enabled
    }
}

/// The path's tunables (C#'s `HandshakeDeadline`, `DirectRedetectInterval` init properties; the profile's adapter rule).
#[derive(Debug, Clone, Copy)]
pub struct DirectPathOptions {
    /// `PlatformProfile::tailscale_needs_utun`.
    pub needs_utun: bool,
    pub handshake_deadline: Duration,
    pub redetect_interval: Duration,
}

impl Default for DirectPathOptions {
    fn default() -> DirectPathOptions {
        DirectPathOptions { needs_utun: false, handshake_deadline: wire::HANDSHAKE_DEADLINE, redetect_interval: wire::REDETECT_INTERVAL }
    }
}

#[derive(Clone)]
struct Source {
    id: u64,
    read: AddressSource,
    port: u16,
}

#[derive(Default)]
struct PathState {
    settings: DirectSettings,
    disposed: bool,
    sink: Option<Arc<dyn DirectSink>>,
    source: Option<Source>,
    listener: Option<DirectListener>,
    bound_addresses: Option<Vec<String>>,
    bound_port: Option<u16>,
    endpoints: Vec<String>,
    status: MobileDirectStatus,
    watch: Option<AbortHandle>,
    address_changes: Option<Subscription>,
    connections: Vec<AbortHandle>,
}

impl PathState {
    /// Whether the path listens: the remote and its direct switch on, and a relay host to run what it admits (none: nothing listens,
    /// nothing is advertised).
    fn wanted(&self) -> bool {
        !self.disposed && self.settings.on() && self.sink.is_some()
    }
}

struct PathInner {
    network: Arc<dyn Network>,
    runtime: Handle,
    log: DirectLog,
    options: DirectPathOptions,
    state: Mutex<PathState>,
    published: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    sources: AtomicU64,
    passes: AtomicUsize,
    /// One pass (a read and its apply) at a time, so an older read never lands over a newer one.
    pass: tokio::sync::Mutex<()>,
}

/// What one pass read, outside the lock (C# `DirectRead`): the source it read from and the addresses, or why they could not be read.
/// Not `wanted` when the path was off, and then nothing was read.
struct Read {
    wanted: bool,
    source: Option<u64>,
    addresses: Option<Result<Vec<String>, String>>,
}

/// The relay host's direct half (see the module's notes). Cheap to clone; every clone is the same path.
#[derive(Clone)]
pub struct DirectPath {
    inner: Arc<PathInner>,
}

impl DirectPath {
    pub fn new(network: Arc<dyn Network>, runtime: Handle, options: DirectPathOptions, log: DirectLog) -> DirectPath {
        DirectPath {
            inner: Arc::new(PathInner {
                network,
                runtime,
                log,
                options,
                state: Mutex::default(),
                published: Mutex::new(None),
                sources: AtomicU64::new(1),
                passes: AtomicUsize::new(0),
                pass: tokio::sync::Mutex::new(()),
            }),
        }
    }

    /// Told after every pass that changed the status (C# `Republish`: Settings, the QR, `/m1/info`). Called with no lock held.
    pub fn on_published(&self, published: impl Fn() + Send + Sync + 'static) {
        *lock(&self.inner.published) = Some(Arc::new(published));
    }

    /// `DirectStatus`.
    pub fn status(&self) -> MobileDirectStatus {
        lock(&self.inner.state).status.clone()
    }

    /// `DirectEndpoints`: what auth_ok, `/m1/info` and the QR carry; empty while nothing listens.
    pub fn endpoints(&self) -> Vec<String> {
        lock(&self.inner.state).endpoints.clone()
    }

    /// Connections the listeners hold now (in their upgrade or running), for the checks.
    pub fn live_connections(&self) -> usize {
        lock(&self.inner.state).listener.as_ref().map_or(0, DirectListener::live_connections)
    }

    /// How many passes ran (a read and its apply), for the checks.
    pub fn passes(&self) -> usize {
        self.inner.passes.load(Ordering::SeqCst)
    }

    /// The relay host that runs admitted sockets, kept at once (the caller then runs a forced [`DirectPath::refresh`]). Without one the
    /// path does not listen: no endpoint, status off, every direct phone ended.
    pub fn set_sink(&self, sink: Option<Arc<dyn DirectSink>>) {
        lock(&self.inner.state).sink = sink;
    }

    /// C#'s `SetDirectAddressSource(source, port)`: the checks' and the smoke's seam (port 0 picks any), then a forced pass. Only its
    /// loopback addresses are taken (anything else is dropped and logged), so the seam can never open a real interface or a wildcard; a
    /// loopback peer is admitted and a loopback host advertised through it. The app never sets it: it binds only Tailscale's addresses.
    #[doc(hidden)]
    pub async fn set_address_source(&self, source: Option<AddressSource>, port: u16) {
        let source = source.map(|read| {
            let log = self.inner.log.clone();
            let loopback: AddressSource = Arc::new(move || {
                let (kept, dropped): (Vec<String>, Vec<String>) =
                    read()?.into_iter().partition(|address| address.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback()));
                if !dropped.is_empty() {
                    log(&format!("mobile direct: the address seam takes loopback only; {} other dropped", dropped.len()));
                }
                Ok(kept)
            });
            Source { id: self.inner.sources.fetch_add(1, Ordering::SeqCst), read: loopback, port }
        });
        lock(&self.inner.state).source = source;
        self.refresh(true).await;
    }

    /// The saved settings, kept at once (so writes land in the order they were made, whatever order their passes finish); answers
    /// whether the next pass must be forced (the switch, the remote or the port changed).
    pub fn store_settings(&self, settings: DirectSettings) -> bool {
        let mut state = lock(&self.inner.state);
        let before = std::mem::replace(&mut state.settings, settings);
        before.enabled != settings.enabled || before.direct_enabled != settings.direct_enabled || before.port != settings.port
    }

    /// `ApplyAsync`'s direct half: the saved settings kept, then a pass (forced when the switch, the remote or the port changed); the
    /// adapters are read off the caller and outside the lock.
    pub async fn apply(&self, settings: DirectSettings) -> bool {
        let force = self.store_settings(settings);
        self.refresh(force).await
    }

    /// `RefreshDirect`: reads the addresses again and makes the listeners match when they or the port changed; with the remote or the
    /// switch off, stops and ends the direct phones. `force` rebinds even when nothing seems changed. Answers whether the status
    /// changed (then it is published).
    pub async fn refresh(&self, force: bool) -> bool {
        let pass = self.inner.pass.lock().await;
        let read = self.read().await;
        let changed = self.apply_read(force, read);
        drop(pass);
        self.inner.passes.fetch_add(1, Ordering::SeqCst);
        if changed {
            let published = lock(&self.inner.published).clone();
            if let Some(published) = published {
                published();
            }
        }
        changed
    }

    /// `ReadDirect`: off the caller (a blocking thread) and outside the lock: reading the adapters takes a while, so it never holds the
    /// lock the listeners' accepts take.
    async fn read(&self) -> Read {
        let (wanted, source) = {
            let state = lock(&self.inner.state);
            (state.wanted(), state.source.clone())
        };
        let id = source.as_ref().map(|s| s.id);
        if !wanted {
            return Read { wanted: false, source: id, addresses: None };
        }
        let (network, needs_utun) = (self.inner.network.clone(), self.inner.options.needs_utun);
        let reading = self.inner.runtime.spawn_blocking(move || match source {
            Some(source) => (source.read)(),
            None => read_tailscale(network.as_ref(), needs_utun),
        });
        let addresses = reading.await.unwrap_or_else(|_| Err("the address read did not finish".into()));
        Read { wanted: true, source: id, addresses: Some(addresses) }
    }

    /// `ApplyDirect`: makes the listeners match what `read` found, under the lock; answers whether the status changed.
    fn apply_read(&self, force: bool, read: Read) -> bool {
        let mut dropped = vec![];
        let changed = {
            let mut state = lock(&self.inner.state);
            let before = state.status.clone();
            let settings = state.settings;
            if !state.wanted() {
                dropped = self.stop_direct(&mut state);
            } else {
                self.start_watch(&mut state);
                // A read made while the path was still off, or from a source since replaced, says nothing of now: it is left alone,
                // and the pass that turned the path on or replaced the source (or the next watch tick) reads again.
                if read.wanted && read.source == state.source.as_ref().map(|s| s.id) {
                    match read.addresses {
                        Some(Err(fault)) => self.read_failed(&mut state, &fault),
                        Some(Ok(addresses)) => {
                            let seam = state.source.as_ref().map(|s| s.port);
                            let port = seam.unwrap_or(settings.port);
                            if force || state.bound_addresses.as_ref() != Some(&addresses) || state.bound_port != Some(port) {
                                self.bind_direct(&mut state, addresses, port, seam.is_some());
                            }
                        }
                        None => {}
                    }
                }
            }
            state.status != before
        };
        for connection in dropped {
            connection.abort();
        }
        changed
    }

    /// `ReadFailed`: the path stops listening and says why in words, as when binding fails; the next pass reads again. The phones already
    /// in stay. The fault's own words go to the log only.
    fn read_failed(&self, state: &mut PathState, fault: &str) {
        (self.inner.log)(&format!("mobile direct: the addresses could not be read: {fault}"));
        if let Some(listener) = state.listener.take() {
            listener.stop();
        }
        state.bound_addresses = None;
        state.bound_port = None;
        state.endpoints.clear();
        state.status = MobileDirectStatus::new(DirectState::Failed, vec![], Some(status::ADAPTERS_UNREADABLE));
    }

    /// `BindDirect`.
    fn bind_direct(&self, state: &mut PathState, addresses: Vec<String>, port: u16, seam: bool) {
        state.bound_addresses = Some(addresses.clone());
        state.bound_port = Some(port);
        if addresses.is_empty() {
            if let Some(listener) = state.listener.take() {
                listener.stop();
            }
            state.endpoints.clear();
            state.status = MobileDirectStatus::new(DirectState::NotFound, vec![], None);
            return;
        }
        if state.listener.is_none() {
            let path = Arc::downgrade(&self.inner);
            let on_connection: OnConnection = Arc::new(move |incoming| match path.upgrade() {
                Some(inner) => accept_direct(&inner, incoming),
                None => incoming.refuse(),
            });
            state.listener =
                Some(DirectListener::new(self.inner.network.clone(), self.inner.runtime.clone(), on_connection, self.inner.log.clone()));
        }
        let Some(listener) = state.listener.as_ref() else { return };
        let (bound, error) = listener.update(&addresses, port);
        state.endpoints = wire::advertised(&bound, seam);
        // Anything not bound is tried again on the next pass rather than taken as settled.
        if bound.len() < addresses.len() {
            state.bound_addresses = None;
        }
        if bound.is_empty() {
            if let Some(listener) = state.listener.take() {
                listener.stop();
            }
            state.status = MobileDirectStatus::new(DirectState::Failed, vec![], error);
        } else {
            state.status = MobileDirectStatus::new(DirectState::Listening, state.endpoints.clone(), None);
        }
    }

    /// `StopDirect`: stops listening and answers every phone that came in directly, for the caller to end once it let go of the lock.
    fn stop_direct(&self, state: &mut PathState) -> Vec<AbortHandle> {
        if let Some(watch) = state.watch.take() {
            watch.abort();
        }
        state.address_changes = None;
        if let Some(listener) = state.listener.take() {
            listener.stop();
        }
        state.bound_addresses = None;
        state.bound_port = None;
        state.endpoints.clear();
        let disabled = !state.disposed && state.sink.is_some() && state.settings.enabled && !state.settings.direct_enabled;
        state.status = if disabled { MobileDirectStatus::new(DirectState::Disabled, vec![], None) } else { MobileDirectStatus::off() };
        std::mem::take(&mut state.connections)
    }

    /// `StartDirectWatch`: the addresses read again every interval while the path is on (Tailscale coming up does not always raise an
    /// address change), and on every address change the network reports. One pass that fails never ends the watch.
    fn start_watch(&self, state: &mut PathState) {
        if state.watch.is_some() {
            return;
        }
        let interval = if self.inner.options.redetect_interval > Duration::ZERO {
            self.inner.options.redetect_interval
        } else {
            wire::REDETECT_INTERVAL
        };
        let path = Arc::downgrade(&self.inner);
        state.watch = Some(
            self.inner
                .runtime
                .spawn(async move {
                    let mut timer = tokio::time::interval_at(tokio::time::Instant::now() + interval, interval);
                    loop {
                        timer.tick().await;
                        let Some(inner) = path.upgrade() else { return };
                        DirectPath { inner }.refresh(false).await;
                    }
                })
                .abort_handle(),
        );
        let path = Arc::downgrade(&self.inner);
        let runtime = self.inner.runtime.clone();
        let changed = NetworkCallback::new(move || {
            if let Some(inner) = path.upgrade() {
                runtime.spawn(async move { DirectPath { inner }.refresh(false).await });
            }
        });
        state.address_changes = match self.inner.network.on_change(changed) {
            Ok(subscription) => Some(subscription),
            Err(error) => {
                (self.inner.log)(&format!("mobile direct: address changes are not reported: {error}"));
                None
            }
        };
    }

    /// The path ends: nothing listens, every direct phone ends, no pass publishes after it returned.
    pub fn dispose(&self) {
        let dropped = {
            let mut state = lock(&self.inner.state);
            state.disposed = true;
            state.sink = None;
            self.stop_direct(&mut state)
        };
        *lock(&self.inner.published) = None;
        for connection in dropped {
            connection.abort();
        }
    }
}

/// The adapters as Tailscale's addresses (C# `TailscaleAddresses.Current`); `Err` when they could not be read.
fn read_tailscale(network: &dyn Network, needs_utun: bool) -> Result<Vec<String>, String> {
    let adapters = network.adapters().map_err(|error| error.to_string())?;
    let selection = tailscale::select(&adapters, needs_utun);
    if selection.unreadable {
        return Err(format!("none of {} adapters could be read", adapters.unreadable));
    }
    Ok(selection.addresses.into_iter().map(tailscale::canonical).collect())
}

/// A reservation the sink gave; handed back through `refused` unless it was handed over with the socket.
struct Reservation {
    sink: Arc<dyn DirectSink>,
    held: Option<DirectReservation>,
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if let Some(reservation) = self.held.take() {
            self.sink.refused(reservation);
        }
    }
}

/// `AcceptDirect`: one socket a listener accepted, before a byte of it is read. A peer outside Tailscale's ranges (any peer only through
/// the seam), a path that is off or has no sink, or a peer the sink does not admit is reset unread; an admitted one is upgraded within
/// the handshake deadline and handed to the sink, its listener slot held until the connection ends.
fn accept_direct(inner: &Arc<PathInner>, incoming: DirectIncoming) {
    // One deadline from the accept to auth_ok: the upgrade takes its share, the sink's hello and auth the rest.
    let deadline = tokio::time::Instant::now() + inner.options.handshake_deadline;
    let (sink, seam) = {
        let state = lock(&inner.state);
        if state.listener.is_none() || !state.wanted() {
            drop(state);
            incoming.refuse();
            return;
        }
        (state.sink.clone(), state.source.is_some())
    };
    let (Some(sink), Some(peer), Some(remote)) = (sink, incoming.peer_address(), incoming.remote()) else {
        incoming.refuse();
        return;
    };
    if !(tailscale::is_tailscale(peer) || (seam && peer.is_loopback())) {
        incoming.refuse();
        return;
    }
    let Some(admitted) = sink.admit(peer) else {
        incoming.refuse();
        return;
    };
    let mut reservation = Reservation { sink: sink.clone(), held: Some(admitted) };
    let (stream, slot) = incoming.into_parts();
    let task = inner.runtime.spawn(async move {
        let upgraded = tokio::time::timeout_at(deadline, crate::direct::upgrade::accept_web_socket(stream)).await.ok().flatten();
        if let Some(socket) = upgraded
            && let Some(admitted) = reservation.held.take()
        {
            sink.accept(admitted, socket, remote, deadline).await;
        }
        drop(reservation);
        drop(slot);
    });
    let mut state = lock(&inner.state);
    state.connections.retain(|connection| !connection.is_finished());
    if !state.wanted() {
        drop(state);
        task.abort();
    } else {
        state.connections.push(task.abort_handle());
    }
}
