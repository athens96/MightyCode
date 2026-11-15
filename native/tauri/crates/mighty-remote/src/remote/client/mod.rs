//! "다른 컴퓨터 연결", the client (C# `RemoteClientService`, `RemoteClientOptions`, R\RemoteClientService.cs;
//! M/Remote/RemoteClientService.swift; plan §1.6): saved connections to other computers' Mighty Code (their keys in the vault, read at
//! first use), the remote workspaces opened through them, and the runs of their panes ([`runs`]), which make it mighty-run's
//! `RemoteRunner`. Every change to which connections are listed, its vault call and the file write after it go one at a time, in the
//! order the list changes. A connection's requests leave only through this PC's Tailscale tunnel (the route is asked before each one).
//! A lost connection ends its runs with `remote.run.error.connectionLost`; a 401 on a start fails the connection. Owner: W8-D.

pub mod runs;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use mighty_core::model::Workspace;
use mighty_core::wire;
use mighty_platform::SecretVault;
use mighty_run::model::RunEvent;
use mighty_run::strings::{get, get_with};
use mighty_run::{RunError, permissions::ToolPermissionRequest};

use crate::remote::address::RemoteAddress;
use crate::remote::events::PendingPermissions;
use crate::remote::saved::{self, SavedConnection};
use crate::remote::strings;
use crate::remote::text;
use crate::remote::transport::{self, Method, RemoteTarget, Request, TransportError};
use crate::remote::validation;
use crate::remote::wire::{RemoteConnectionInfo, RemoteConnectionStatus, RemoteWireInfo, protocol};

/// Where the client hands a run's events and tool requests (C#'s `onEvent` and `onPermission`: the desktop service's `Receive` and
/// `ToolPermissionChanged`). Called on the client's tasks while a poll is applied: brief, never waiting on the window.
#[derive(Clone)]
pub struct ClientSinks {
    pub event: Arc<dyn Fn(RunEvent) + Send + Sync>,
    pub permission: Arc<dyn Fn(ToolPermissionRequest) + Send + Sync>,
}

/// Lists a host's shared workspace as a remote workspace of this app (C# `StateStore.ApproveRemote(connectionId, hostWorkspaceId, name,
/// path, hostName)`); None when the store refuses it.
pub type ApproveRemote = Arc<dyn Fn(&str, &str, &str, &str, &str) -> Option<Workspace> + Send + Sync>;
/// Whether the route to an IP still leaves through this PC's tunnel; Err when it cannot be told (no tunnel either).
pub type RouteCheck = Arc<dyn Fn(&str) -> Result<bool, String> + Send + Sync>;
/// This PC's Tailscale addresses (empty while Tailscale is off); Err when the adapters cannot be read.
pub type LocalAddresses = Arc<dyn Fn() -> Result<Vec<String>, String> + Send + Sync>;

/// C# `RemoteClientOptions`.
#[derive(Clone)]
pub struct ClientOptions {
    /// Loopback stands in for Tailscale; only the checks and the smoke set it, in code.
    pub allow_loopback_for_tests: bool,
    /// Replaces the real route check (`RemoteRoute.ThroughTailscale`) when set; the app sets it from this PC's adapters.
    pub route_check: Option<RouteCheck>,
    pub local_addresses: Option<LocalAddresses>,
    pub poll_retry_window: Duration,
    /// How long a connect waits for a saved-connections file held open by another program before it is refused.
    pub saved_read_wait: Duration,
    /// The connections file may be written (`--allow-state-write`); else the list lives in memory for this launch.
    pub writes: bool,
}

impl Default for ClientOptions {
    fn default() -> Self {
        ClientOptions {
            allow_loopback_for_tests: false,
            route_check: None,
            local_addresses: None,
            poll_retry_window: protocol::POLL_RETRY_WINDOW,
            saved_read_wait: Duration::from_secs(5),
            writes: true,
        }
    }
}

/// Why a connection call failed: the transport's own failure (a host's status among them), or the words to say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientError {
    Transport(TransportError),
    /// C#'s `InvalidOperationException` / `ArgumentException` / `InvalidDataException` with its message.
    Message(String),
    /// C#'s `OperationCanceledException`: stopped before the host took it.
    Cancelled,
}

impl ClientError {
    pub fn message(&self) -> String {
        match self {
            ClientError::Transport(error) => error.message().to_string(),
            ClientError::Message(message) => message.clone(),
            ClientError::Cancelled => String::new(),
        }
    }

    /// The host's status, for a host's error status.
    pub fn status(&self) -> Option<u16> {
        match self {
            ClientError::Transport(error) => error.status(),
            _ => None,
        }
    }

    pub(crate) fn run_error(self) -> RunError {
        match self {
            ClientError::Cancelled => RunError::Cancelled(String::new()),
            other => RunError::State(other.message()),
        }
    }
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::error::Error for ClientError {}

fn message(text: String) -> ClientError {
    ClientError::Message(text)
}

/// One listed connection.
pub(crate) struct Entry {
    pub info: RemoteConnectionInfo,
    pub key: Option<String>,
    pub target: Option<RemoteTarget>,
    /// Saved with a key in the vault that was not read yet (read on first use).
    pub key_pending: bool,
    /// Which entry this is (C#'s reference): a connection replaced by a connect is another entry under the same id.
    pub serial: u64,
    pub revision: u64,
}

/// The key is left out.
impl std::fmt::Debug for Entry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Entry").field("info", &self.info).field("serial", &self.serial).field("revision", &self.revision).finish()
    }
}

#[derive(Default)]
pub(crate) struct Gate {
    pub connections: HashMap<String, Entry>,
    pub order: Vec<String>,
    pub connecting: HashSet<String>,
    pub loaded: bool,
    pub closing: bool,
    /// The saved file could not be read yet: read again from this moment on, and written by nobody until read.
    pub load_retry_at: Option<Instant>,
    pub load_failed: bool,
    /// A caller is reading the saved file now (with the gate free).
    pub loading: bool,
    pub runs: HashMap<String, Arc<runs::Run>>,
    /// Runs whose follow loop has not ended yet.
    pub followers: Vec<Arc<runs::Run>>,
}

/// An address being connected to (`connecting`), given back when the connect ends however it ends: done, failed, dropped or panicked.
struct Connecting {
    client: RemoteClient,
    address: String,
}

impl Drop for Connecting {
    fn drop(&mut self) {
        lock(&self.client.0.gate).connecting.remove(&self.address);
    }
}

pub(crate) struct ClientInner {
    pub approve: ApproveRemote,
    pub data_directory: PathBuf,
    pub keys: Arc<dyn SecretVault>,
    pub sinks: Mutex<Option<ClientSinks>>,
    pub options: ClientOptions,
    pub gate: Mutex<Gate>,
    /// Every change to which connections are listed, its vault call and the file write that follows go through here one at a time.
    pub listing: tokio::sync::Mutex<()>,
    pub changed: Mutex<Vec<Arc<dyn Fn() + Send + Sync>>>,
    pub pending: PendingPermissions,
    pub serials: AtomicU64,
}

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Every saved connection and the runs on them.
#[derive(Clone)]
pub struct RemoteClient(pub(crate) Arc<ClientInner>);

impl RemoteClient {
    pub fn new(approve: ApproveRemote, data_directory: PathBuf, keys: Arc<dyn SecretVault>, options: ClientOptions) -> RemoteClient {
        RemoteClient(Arc::new(ClientInner {
            approve,
            data_directory,
            keys,
            sinks: Mutex::new(None),
            options,
            gate: Mutex::new(Gate::default()),
            listing: tokio::sync::Mutex::new(()),
            changed: Mutex::new(vec![]),
            pending: PendingPermissions::default(),
            serials: AtomicU64::new(1),
        }))
    }

    /// Where the runs' events and tool requests go (the desktop service attaches itself once it exists).
    pub fn set_sinks(&self, sinks: ClientSinks) {
        *lock(&self.0.sinks) = Some(sinks);
    }

    /// C#'s `Changed` event: told after a change to the listed connections, outside every lock.
    pub fn on_changed(&self, listener: Arc<dyn Fn() + Send + Sync>) {
        lock(&self.0.changed).push(listener);
    }

    pub(crate) fn tell_changed(&self) {
        let listeners = lock(&self.0.changed).clone();
        for listener in listeners {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| listener()));
        }
    }

    fn next_serial(&self) -> u64 {
        self.0.serials.fetch_add(1, Ordering::SeqCst)
    }

    /// `List()`: the saved connections with their state, for the shell model and Settings › 원격 연결.
    pub fn connections(&self) -> Vec<RemoteConnectionInfo> {
        self.load_saved();
        let gate = lock(&self.0.gate);
        gate.order.iter().filter_map(|id| gate.connections.get(id)).map(|c| c.info.clone()).collect()
    }

    /// `Connection(id)`.
    pub fn connection(&self, id: &str) -> Option<RemoteConnectionInfo> {
        self.load_saved();
        lock(&self.0.gate).connections.get(id).map(|c| c.info.clone())
    }

    /// `ConnectAsync(name, address, key)`: connects with a new address and key, or replaces the key of the saved connection at the same
    /// address (a host that restarted sharing has a new one).
    pub async fn connect(&self, name: &str, address: &str, key: &str) -> Result<RemoteConnectionInfo, ClientError> {
        // A connect never adds an entry while the file is unread, so an address it holds keeps its id. A typo is said at once.
        let label = name.trim();
        let trimmed = key.trim();
        if text::count(label, saved::MAXIMUM_NAME + 1) > saved::MAXIMUM_NAME || !validation::key(Some(trimmed)) {
            return Err(message(get("remote.connect.error.badKey")));
        }
        let parsed = RemoteAddress::parse(address).map_err(message)?;
        let display = parsed.display();
        self.saved_read().await?;
        let existing = {
            let mut gate = lock(&self.0.gate);
            if gate.closing {
                return Err(message(get("common.appClosing")));
            }
            let existing =
                gate.connections.values().find(|c| c.info.address == display).map(|c| (c.info.id.clone(), c.info.name.clone(), c.serial));
            if existing.is_none() && gate.connections.len() >= protocol::MAXIMUM_CONNECTIONS {
                return Err(too_many());
            }
            if !gate.connecting.insert(display.clone()) {
                return Err(message(get("remote.connect.error.busy")));
            }
            existing
        };
        let _connecting = Connecting { client: self.clone(), address: display };
        self.connect_reached(label, trimmed, &parsed, existing).await
    }

    async fn connect_reached(
        &self,
        label: &str,
        key: &str,
        parsed: &RemoteAddress,
        existing: Option<(String, String, u64)>,
    ) -> Result<RemoteConnectionInfo, ClientError> {
        let (target, info) = self.reach(parsed, key).await?;
        if lock(&self.0.gate).closing {
            return Err(message(get("common.appClosing")));
        }
        let id = existing.as_ref().map(|(id, _, _)| id.clone()).unwrap_or_else(wire::new_id);
        if let Some((_, _, serial)) = &existing {
            // The old key's connection ends, and its runs are taken, in one step before their stops.
            let taken = {
                let mut gate = lock(&self.0.gate);
                if let Some(old) = gate.connections.get_mut(&id).filter(|c| c.serial == *serial) {
                    disconnected(old, self.next_serial());
                }
                self.take_runs(&mut gate, &id)
            };
            self.end_runs(taken, None).await;
        }
        let view;
        {
            let _listing = self.0.listing.lock().await;
            {
                let gate = lock(&self.0.gate);
                if gate.closing {
                    return Err(message(get("common.appClosing")));
                }
                // Another connect may have filled the list while this one waited on the host: refused before its key is saved.
                if !gate.connections.contains_key(&id) && gate.connections.len() >= protocol::MAXIMUM_CONNECTIONS {
                    return Err(too_many());
                }
            }
            // A vault that cannot keep the key leaves it in memory for this launch, and any key it kept before stays.
            let saved = self.vault_save(&id, key).await;
            let name = saved::name_of(Some(if !label.is_empty() {
                label
            } else if let Some((_, old, _)) = &existing {
                old.as_str()
            } else {
                info.host_name.as_str()
            }));
            view = RemoteConnectionInfo {
                id: id.clone(),
                name,
                address: parsed.display(),
                status: RemoteConnectionStatus::Connected,
                host_name: Some(info.host_name.clone()),
                workspaces: Some(info.workspaces.clone()),
                runtime: Some(info.runtime.clone()),
                detail: Some(if saved { strings::connected_saved() } else { strings::connected_memory() }),
            };
            {
                let mut gate = lock(&self.0.gate);
                let serial = self.next_serial();
                gate.connections.insert(
                    id.clone(),
                    Entry {
                        info: view.clone(),
                        key: Some(key.to_string()),
                        target: Some(target),
                        key_pending: false,
                        serial,
                        revision: serial,
                    },
                );
                if !gate.order.contains(&id) {
                    gate.order.push(id.clone());
                }
            }
            self.write_saved().await;
        }
        self.tell_changed();
        Ok(view)
    }

    /// `RefreshAsync(id)`: reconnects a saved connection; its key is read from the vault here the first time, never at launch.
    pub async fn refresh(&self, id: &str) -> Result<RemoteConnectionInfo, ClientError> {
        self.load_saved();
        let (serial, pending) = {
            let gate = lock(&self.0.gate);
            if gate.closing {
                return Err(message(get("common.appClosing")));
            }
            let current = gate.connections.get(id).ok_or_else(|| message(get("remote.connect.error.unknown")))?;
            (current.serial, current.key.is_none() && current.key_pending)
        };
        if pending {
            // Read off the caller's task and outside the lock; whatever changed meanwhile is looked at again below.
            let read = self.vault_load(id).await;
            let mut gate = lock(&self.0.gate);
            if let Some(current) = gate.connections.get_mut(id).filter(|c| c.serial == serial && c.key.is_none() && c.key_pending) {
                current.key = read;
                current.key_pending = false;
            }
        }
        let (key, address, revision, serial) = {
            let mut gate = lock(&self.0.gate);
            if gate.closing {
                return Err(message(get("common.appClosing")));
            }
            let Some(current) = gate.connections.get(id) else { return Err(message(get("remote.connect.error.unknown"))) };
            let (key, address, revision, serial) = (current.key.clone(), current.info.address.clone(), current.revision, current.serial);
            if key.is_some() && !gate.connecting.insert(address.clone()) {
                return Err(message(get("remote.connect.error.busy")));
            }
            if key.is_none()
                && let Some(current) = gate.connections.get_mut(id)
            {
                current.info.status = RemoteConnectionStatus::Disconnected;
                current.info.detail = Some(get("remote.connect.error.keyMissing"));
            }
            (key, address, revision, serial)
        };
        let Some(key) = key else {
            self.tell_changed();
            return Err(message(get("remote.connect.error.keyMissing")));
        };
        let _connecting = Connecting { client: self.clone(), address: address.clone() };
        let result = async {
            let parsed = RemoteAddress::parse(&address).map_err(message)?;
            let (target, info) = self.reach(&parsed, &key).await?;
            let result = {
                let mut gate = lock(&self.0.gate);
                let current_now = !gate.closing && unchanged(&gate, id, serial, revision);
                // Disconnected, removed, replaced or lost while this one waited on the host: that newer state stands.
                match gate.connections.get_mut(id) {
                    Some(current) if current_now => {
                        current.target = Some(target);
                        current.info.status = RemoteConnectionStatus::Connected;
                        current.info.host_name = Some(info.host_name.clone());
                        current.info.workspaces = Some(info.workspaces.clone());
                        current.info.runtime = Some(info.runtime.clone());
                        current.info.detail = Some(get("remote.connect.detail.connected"));
                        current.info.clone()
                    }
                    _ => return Err(message(get("remote.connect.error.changed"))),
                }
            };
            self.tell_changed();
            Ok(result)
        }
        .await;
        if let Err(error) = &result {
            self.fail(id, &error.message(), Some((serial, revision))).await;
        }
        result
    }

    /// `EnsureConnectedAsync(id)`: connected now, reconnecting first when it is not (a remote pane's send does this).
    pub async fn ensure_connected(&self, id: &str) -> Result<RemoteConnectionInfo, ClientError> {
        self.load_saved();
        // Two panes sending at once after launch share one connect.
        let deadline = Instant::now() + protocol::REQUEST_TIMEOUT + Duration::from_secs(5);
        while Instant::now() < deadline {
            let waiting = {
                let gate = lock(&self.0.gate);
                gate.connections.get(id).is_some_and(|c| gate.connecting.contains(&c.info.address))
            };
            if !waiting {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let found = {
            let gate = lock(&self.0.gate);
            gate.connections.get(id).map(|c| (c.info.clone(), c.target.clone(), c.serial, c.revision))
        };
        if let Some((info, Some(target), serial, revision)) = found
            && info.status == RemoteConnectionStatus::Connected
        {
            self.ensure_route(id, serial, revision, &target).await?;
            return Ok(info);
        }
        self.refresh(id).await
    }

    /// `DisconnectAsync(id)`: stops this connection's runs and keeps it saved; a refresh connects again. What happens at once (the
    /// connection disconnected, its runs taken and told they stopped) is done before this returns; the future stops them on the host.
    pub fn disconnect(&self, id: &str) -> mighty_platform::BoxFuture<'static, ()> {
        let leaving = self.leave(id);
        let client = self.clone();
        Box::pin(async move {
            leaving.await;
            client.tell_changed();
        })
    }

    /// `LeaveAsync(id)`: disconnected, and its runs taken, in one step before their stops; each pane says it stopped (at once).
    fn leave(&self, id: &str) -> mighty_platform::BoxFuture<'static, ()> {
        let taken = {
            let mut gate = lock(&self.0.gate);
            let serial = self.next_serial();
            if let Some(entry) = gate.connections.get_mut(id) {
                disconnected(entry, serial);
            }
            self.take_runs(&mut gate, id)
        };
        let stops = self.say_ended(&taken, None);
        let client = self.clone();
        Box::pin(async move { client.stop_all_on_host(stops).await })
    }

    /// `RemoveAsync(id)`: forgets the connection and its key. Its workspaces stay listed until removed. The runs end at once.
    pub fn remove(&self, id: &str) -> mighty_platform::BoxFuture<'static, ()> {
        let leaving = self.leave(id);
        let client = self.clone();
        let id = id.to_string();
        Box::pin(async move {
            leaving.await;
            {
                let _listing = client.0.listing.lock().await;
                {
                    let mut gate = lock(&client.0.gate);
                    gate.connections.remove(&id);
                    gate.order.retain(|o| o != &id);
                }
                client.vault_delete(&id).await;
                client.write_saved().await;
            }
            client.tell_changed();
        })
    }

    /// `OpenWorkspace(connectionId, hostWorkspaceId)`: lists the host's shared workspace in this app as a remote workspace.
    pub fn open_workspace(&self, connection_id: &str, host_workspace_id: &str) -> Result<Workspace, ClientError> {
        let not_shared = || message(get("remote.workspace.error.notShared"));
        let info = {
            let gate = lock(&self.0.gate);
            match gate.connections.get(connection_id) {
                Some(c) if c.info.status == RemoteConnectionStatus::Connected => c.info.clone(),
                _ => return Err(not_shared()),
            }
        };
        let shared = info.shared().iter().find(|w| w.id == host_workspace_id).ok_or_else(not_shared)?;
        if !validation::shared_workspace(shared) {
            return Err(not_shared());
        }
        let host = info.host_name.clone().unwrap_or_else(|| info.name.clone());
        (self.0.approve)(connection_id, &shared.id, &shared.name, &shared.path, &host).ok_or_else(not_shared)
    }

    async fn vault_save(&self, id: &str, key: &str) -> bool {
        let (keys, id, key) = (self.0.keys.clone(), id.to_string(), key.to_string());
        tokio::task::spawn_blocking(move || keys.save(&id, &key).is_ok()).await.unwrap_or(false)
    }

    async fn vault_load(&self, id: &str) -> Option<String> {
        let (keys, id) = (self.0.keys.clone(), id.to_string());
        tokio::task::spawn_blocking(move || keys.load(&id)).await.ok().flatten()
    }

    async fn vault_delete(&self, id: &str) {
        let (keys, id) = (self.0.keys.clone(), id.to_string());
        let _ = tokio::task::spawn_blocking(move || keys.delete(&id)).await;
    }

    /// The route check, where one that fails is no tunnel: never a passing failure, never a request.
    pub(crate) fn routed(&self, ip: &str) -> bool {
        let options = &self.0.options;
        match &options.route_check {
            Some(check) => std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| check(ip))).ok().and_then(Result::ok).unwrap_or(false),
            None if options.allow_loopback_for_tests => true,
            None => false,
        }
    }

    /// `ReachAsync(address, key)`: this PC on Tailscale, the host resolved to Tailscale only, the route through the tunnel, its /info read
    /// strictly and checked as the Mac checks it (an older host's long lists cut first).
    async fn reach(&self, address: &RemoteAddress, key: &str) -> Result<(RemoteTarget, RemoteWireInfo), ClientError> {
        let options = &self.0.options;
        // While testing (loopback injected) this PC's own Tailscale is not asked, unless a check gives its addresses.
        if options.local_addresses.is_some() || !options.allow_loopback_for_tests {
            let local = match &options.local_addresses {
                Some(read) => match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| read())) {
                    Ok(Ok(local)) => local,
                    Ok(Err(error)) => return Err(message(error)),
                    Err(_) => vec![],
                },
                None => vec![],
            };
            if local.is_empty() {
                return Err(message(strings::local_tailscale_off()));
            }
        }
        let target = transport::resolve(address, options.allow_loopback_for_tests).await.map_err(ClientError::Transport)?;
        // The peer must be reached through this PC's own tunnel: a carrier-grade NAT network can route 100.64/10 over Wi-Fi.
        if !self.routed(&target.ip) {
            return Err(message(get("remote.connect.error.notViaTailscale")));
        }
        let data = transport::request(&target, key, Method::Get, &format!("{}info", protocol::PATH_PREFIX), Request::default())
            .await
            .map_err(ClientError::Transport)?;
        let mut info = RemoteWireInfo::decode(&data).map_err(|_| message(get("remote.connect.error.notMighty")))?;
        // An older host may send a provider's whole list: its first models are kept, the host is not refused.
        info.runtime = validation::bounded(&info.runtime);
        validation::info(&info).map_err(|e| message(e.0))?;
        Ok((target, info))
    }

    /// `EnsureRouteAsync`: before each request to the host the route still goes through the tunnel, or the connection is lost (never
    /// retried). When the connection changed since `revision`, that newer state stands and only this request is refused.
    pub(crate) async fn ensure_route(&self, id: &str, serial: u64, revision: u64, target: &RemoteTarget) -> Result<(), ClientError> {
        let client = self.clone();
        let ip = target.ip.clone();
        // The check may block (it reads this PC's adapters): off the caller's task.
        let routed = tokio::task::spawn_blocking(move || client.routed(&ip)).await.unwrap_or(false);
        if routed {
            return Ok(());
        }
        let not_via = get("remote.connect.error.notViaTailscale");
        if self.fail(id, &not_via, Some((serial, revision))).await {
            return Err(message(not_via));
        }
        Err(message(get("remote.connect.error.changed")))
    }

    /// `FailAsync(id, message, expected, revision)`: the network or the host failed: the connection shows the error and its runs end in
    /// error. With `expected`, only while the connection is still that entry at that revision; false when it was not.
    pub(crate) async fn fail(&self, id: &str, words: &str, expected: Option<(u64, u64)>) -> bool {
        let taken = {
            let mut gate = lock(&self.0.gate);
            if expected.is_some_and(|(serial, revision)| !unchanged(&gate, id, serial, revision)) {
                return false;
            }
            let serial = self.next_serial();
            let Some(entry) = gate.connections.get_mut(id) else { return false };
            entry.revision = serial;
            entry.target = None;
            entry.info.status = RemoteConnectionStatus::Disconnected;
            entry.info.detail = Some(text::cut(words, text::DEFAULT_CUT));
            // Taken with the revision: a poll that lands in between finds its run gone and leaves the pane's last word to this.
            self.take_runs(&mut gate, id)
        };
        self.end_runs(taken, Some(words)).await;
        self.tell_changed();
        true
    }

    /// `LoadSaved`: the saved connections, read once on first use; a file that is there but cannot be read now is read again a second
    /// later rather than taken as empty, and once it is read late the window is told.
    pub(crate) fn load_saved(&self) {
        // The file is read with the gate free (the window's redraws read the list under it), by one caller at a time: another caller
        // meanwhile goes on with the list as it is.
        {
            let mut gate = lock(&self.0.gate);
            if gate.loaded || gate.loading || gate.load_retry_at.is_some_and(|at| Instant::now() < at) {
                return;
            }
            gate.loading = true;
        }
        let read = saved::try_read(&self.0.data_directory);
        let late = {
            let mut gate = lock(&self.0.gate);
            gate.loading = false;
            let items = match read {
                Ok(items) => items,
                Err(_) => {
                    gate.load_failed = true;
                    gate.load_retry_at = Some(Instant::now() + Duration::from_secs(1));
                    return;
                }
            };
            gate.loaded = true;
            let late = gate.load_failed;
            for item in items {
                let Ok(address) = RemoteAddress::parse(&item.address) else { continue };
                let display = address.display();
                if gate.connections.contains_key(&item.id)
                    || gate.connections.values().any(|c| c.info.address == display)
                    || gate.connections.len() >= protocol::MAXIMUM_CONNECTIONS
                {
                    continue;
                }
                let serial = self.next_serial();
                let info = RemoteConnectionInfo {
                    id: item.id.clone(),
                    name: item.name.clone(),
                    address: display,
                    detail: Some(get("remote.connect.detail.saved")),
                    ..RemoteConnectionInfo::default()
                };
                gate.connections
                    .insert(item.id.clone(), Entry { info, key: None, target: None, key_pending: true, serial, revision: serial });
                gate.order.push(item.id);
            }
            late
        };
        if late {
            self.tell_changed();
        }
    }

    /// `SavedReadAsync`: waits, at most the saved-read wait, until the saved-connections file is read; then refuses.
    async fn saved_read(&self) -> Result<(), ClientError> {
        let end = Instant::now() + self.0.options.saved_read_wait;
        loop {
            self.load_saved();
            {
                let gate = lock(&self.0.gate);
                if gate.closing {
                    return Err(message(get("common.appClosing")));
                }
                if gate.loaded {
                    return Ok(());
                }
            }
            if Instant::now() >= end {
                return Err(message(strings::saved_unreadable()));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
            // The next try is not held back a second when a connect is waiting on it.
            lock(&self.0.gate).load_retry_at = None;
        }
    }

    /// `WriteSavedAsync`: id, name and address of every listed connection, never a key. The caller holds the listing.
    async fn write_saved(&self) {
        // Never over a file not read yet: what it holds would be lost.
        self.load_saved();
        let list = {
            let gate = lock(&self.0.gate);
            if !gate.loaded {
                return;
            }
            gate.order
                .iter()
                .filter_map(|id| gate.connections.get(id))
                .map(|c| SavedConnection::new(&c.info.id, &c.info.name, &c.info.address))
                .collect::<Vec<_>>()
        };
        if !self.0.options.writes {
            return;
        }
        let directory = self.0.data_directory.clone();
        // The connection still works for this launch when the file cannot be written; only its listing is not kept.
        let _ = tokio::task::spawn_blocking(move || saved::write(&directory, &list)).await;
    }
}

/// `Disconnected(entry)`: a new revision, no target. Under the gate.
fn disconnected(entry: &mut Entry, revision: u64) {
    entry.revision = revision;
    entry.target = None;
    entry.info.status = RemoteConnectionStatus::Disconnected;
    entry.info.detail = Some(get("remote.connect.detail.disconnected"));
}

/// `Unchanged(id, entry, revision)`: the entry listed under `id` is still `serial` at `revision`. Under the gate.
pub(crate) fn unchanged(gate: &Gate, id: &str, serial: u64, revision: u64) -> bool {
    gate.connections.get(id).is_some_and(|c| c.serial == serial && c.revision == revision)
}

fn too_many() -> ClientError {
    message(get_with("remote.connect.error.tooMany", &[("count", &protocol::MAXIMUM_CONNECTIONS.to_string())]))
}
