//! The sharing host, "이 컴퓨터 공유" (C# `RemoteHostService`, `RemoteHostOptions`, `RemoteRateLimiter`, R\RemoteHostService.cs;
//! M/Remote/RemoteHostService.swift; plan §1.6): serves the chosen local workspaces to other Mighty Code apps on this PC's Tailscale
//! addresses only. The key is 32 random bytes in base64url, new on every start, in memory only, revoked at stop, compared as SHA-256 in
//! fixed time. Every request is authorized in C#'s order (503 stopped, 403 origin or peer, 429 per peer, 401 key, 429 overall, 426
//! version), then routed (400 `badPath`, `GET /v2/info`, the runs). Runs go through this PC's own runner ([`runs`]); a run its client
//! stops polling for the heartbeat is stopped. Sharing never survives the app. Started only from the Start button when the app may
//! listen (`Host::may_listen`). Owner: W8-D.

pub mod runs;

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, Weak};
use std::time::Duration;

use base64::Engine;
use mighty_core::model::Workspace;
use mighty_core::wire;
use mighty_platform::{BoxFuture, Network};
use mighty_run::antigravity::AntigravityRuleLeases;
use mighty_run::model::{RuntimeInfo, StartRunRequest};
use mighty_run::strings::get;
use serde_json::Value;
use tokio::sync::watch;

use crate::remote::address::{RemoteAddress, canonical, peer_allowed, peer_ip_allowed};
use crate::remote::http_server::{Failed, HttpListener, HttpRequest, HttpResponse, ListenerOptions};
use crate::remote::strings;
use crate::remote::text;
use crate::remote::validation;
use crate::remote::wire::{RemoteHostState, RemoteSharedWorkspace, RemoteWireError, RemoteWireInfo, protocol};

pub use runs::{Cancel, HostRunner, RunnerSinks, host_runner};

/// Time on the host's clock: since an origin the clock picks (C# `Func<DateTimeOffset>`; the checks move it by hand).
pub type Clock = Arc<dyn Fn() -> Duration + Send + Sync>;

/// Why this PC cannot start a client's run now.
pub type Admission = Arc<dyn Fn(&StartRunRequest) -> Option<String> + Send + Sync>;
/// This PC's Tailscale addresses, Err when the adapters cannot be read.
pub type Addresses = Arc<dyn Fn() -> Result<Vec<String>, String> + Send + Sync>;
/// A check's hold (between a listener's making and its recording; at a launch's start).
pub type Hook<A> = Arc<dyn Fn(A) -> BoxFuture<'static, ()> + Send + Sync>;

/// A clock that reads the process's monotonic time.
pub fn monotonic() -> Clock {
    static ORIGIN: OnceLock<std::time::Instant> = OnceLock::new();
    let origin = *ORIGIN.get_or_init(std::time::Instant::now);
    Arc::new(move || origin.elapsed())
}

/// C# `RemoteHostOptions`.
#[derive(Clone)]
pub struct HostOptions {
    /// The saved workspaces; only local ones the user picked are ever shared.
    pub workspaces: Arc<dyn Fn() -> Vec<Workspace> + Send + Sync>,
    /// A registered local workspace whose folder exists, or why not.
    pub resolve_local: Arc<dyn Fn(String) -> BoxFuture<'static, Result<Workspace, String>> + Send + Sync>,
    /// This PC's CLIs as /info reports them, with the app's version and its platform.
    pub runtime: Arc<dyn Fn() -> BoxFuture<'static, RuntimeInfo> + Send + Sync>,
    /// The runner client runs go through: no pane MCP, no delegation, no picture cache.
    pub create_runner: Arc<dyn Fn(RunnerSinks) -> Arc<dyn HostRunner> + Send + Sync>,
    pub host_name: String,
    /// Why this PC cannot start a client's run now (its CLIs or plugins are changing); None when it can.
    pub admission: Admission,
    /// Told how many client runs go on this PC whenever that changes (outside the host's lock); while any does, CLI updates wait.
    pub runs_changed: Arc<dyn Fn(usize) + Send + Sync>,
    /// Loopback stands in for Tailscale; only the checks and the smoke set it, in code.
    pub allow_loopback_for_tests: bool,
    pub heartbeat: Duration,
    /// The addresses to listen on: this PC's Tailscale addresses (the app reads them from the `Network` port), Err when the adapters
    /// cannot be read. None: loopback where it is injected, else none. Any that is not a Tailscale address (or loopback, when injected)
    /// is still left out.
    pub addresses: Option<Addresses>,
    pub clock: Clock,
    /// This PC runs Windows, where Codex and Antigravity are not confined as on a Mac (`full_access_refusal`).
    pub windows_host: bool,
    /// agy's settings on this PC, shared with its own panes: an allowed card's continuation puts its rules here. None refuses every grant.
    pub antigravity_rules: Option<AntigravityRuleLeases>,
    /// The system folder Windows reports, where a command's PowerShell comes from (its command line must fit); None: Windows' usual one.
    pub system_folder: Option<String>,
    /// Where listeners bind (`Network::bind_exclusive`).
    pub network: Arc<dyn Network>,
}

/// C# `RemoteRateLimiter`: each peer has 300 requests a second; only requests with the key count toward the shared 600, so a device
/// without it cannot crowd the real client out. At most 64 peers are remembered.
pub struct RateLimiter {
    clock: Clock,
    state: Mutex<Rates>,
}

#[derive(Default)]
struct Rates {
    peers: HashMap<String, (Duration, u32)>,
    window: Option<Duration>,
    keyed: u32,
}

impl RateLimiter {
    pub fn new(clock: Clock) -> RateLimiter {
        RateLimiter { clock, state: Mutex::new(Rates::default()) }
    }

    pub fn allow_peer(&self, peer: &str) -> bool {
        let now = (self.clock)();
        let mut state = lock(&self.state);
        let peers = &mut state.peers;
        let mut entry = peers.get(peer).copied().unwrap_or((now, 0));
        if now.saturating_sub(entry.0) >= Duration::from_secs(1) {
            entry = (now, 0);
        }
        entry.1 += 1;
        if !peers.contains_key(peer) && peers.len() >= 64 {
            peers.clear();
        }
        peers.insert(peer.to_string(), entry);
        entry.1 <= 300
    }

    pub fn allow_keyed(&self) -> bool {
        let now = (self.clock)();
        let mut state = lock(&self.state);
        if state.window.is_none_or(|window| now.saturating_sub(window) >= Duration::from_secs(1)) {
            state.window = Some(now);
            state.keyed = 0;
        }
        state.keyed += 1;
        state.keyed <= 600
    }
}

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What a route refuses with: its status and the words the client reads.
#[derive(Debug, Clone)]
pub(crate) struct HostFailure {
    pub status: u16,
    pub message: String,
}

impl HostFailure {
    pub(crate) fn new(status: u16, message: impl Into<String>) -> HostFailure {
        HostFailure { status, message: message.into() }
    }
}

fn stopped() -> HostFailure {
    HostFailure::new(503, get("remote.host.error.stopped"))
}

pub(crate) struct Gate {
    pub state: RemoteHostState,
    pub runner: Option<Arc<dyn HostRunner>>,
    pub generation: u64,
    pub lease: Option<watch::Sender<bool>>,
    pub changing: bool,
    pub closed: bool,
    pub allows_full_access: bool,
    pub servers: Vec<HttpListener>,
    pub jobs: HashMap<String, runs::Job>,
    /// The Antigravity cards this host's runs reported, by card id, with when (for a day, the newest 128).
    pub reported_rules: HashMap<String, (HashSet<String>, Duration)>,
}

pub(crate) struct HostInner {
    pub options: HostOptions,
    pub gate: Mutex<Gate>,
    /// Serializes `announce_runs`, so a count read earlier is never told after one read later.
    pub announcing: Mutex<()>,
    pub host_id: String,
    pub rates: RateLimiter,
    pub generations: AtomicU64,
    pub serials: AtomicU64,
    /// Awaited between making each address's listener and recording it (checks only).
    pub before_bind: Mutex<Option<Hook<String>>>,
    /// Awaited at the start of each launch (checks only): a stop let in here must keep the run from the runner.
    pub before_launch: Mutex<Option<Hook<runs::Cancel>>>,
}

/// The sharing host.
#[derive(Clone)]
pub struct RemoteHost(pub(crate) Arc<HostInner>);

/// The key as base64url without padding (43 characters).
fn new_key() -> Option<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).ok()?;
    Some(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
}

/// A target the Mac's `URLComponents(string:)` reads: URL characters only, no fragment.
fn target_text(target: &str) -> bool {
    let allowed = |b: u8, query: bool| b.is_ascii_alphanumeric() || b"-._~!$&'()*+,;=:@/%".contains(&b) || query && b == b'?';
    let (path, query) = match target.find('?') {
        Some(mark) => (&target[..mark], Some(&target[mark + 1..])),
        None => (target, None),
    };
    path.starts_with('/') && path.bytes().all(|b| allowed(b, false)) && query.is_none_or(|q| q.bytes().all(|b| allowed(b, true)))
}

impl RemoteHost {
    pub fn new(options: HostOptions) -> RemoteHost {
        let rates = RateLimiter::new(options.clock.clone());
        RemoteHost(Arc::new(HostInner {
            options,
            gate: Mutex::new(Gate {
                state: RemoteHostState::default(),
                runner: None,
                generation: 0,
                lease: None,
                changing: false,
                closed: false,
                allows_full_access: false,
                servers: vec![],
                jobs: HashMap::new(),
                reported_rules: HashMap::new(),
            }),
            announcing: Mutex::new(()),
            host_id: wire::new_id(),
            rates,
            generations: AtomicU64::new(1),
            serials: AtomicU64::new(1),
            before_bind: Mutex::new(None),
            before_launch: Mutex::new(None),
        }))
    }

    /// The checks' hold between making a listener and recording it.
    pub fn set_before_bind(&self, hook: Option<Hook<String>>) {
        *lock(&self.0.before_bind) = hook;
    }

    /// The checks' hold at the start of each launch.
    pub fn set_before_launch(&self, hook: Option<Hook<runs::Cancel>>) {
        *lock(&self.0.before_launch) = hook;
    }

    /// The listeners of the sharing now on, one per address (checks only).
    pub fn listeners(&self) -> Vec<HttpListener> {
        lock(&self.0.gate).servers.clone()
    }

    fn next_generation(&self) -> u64 {
        self.0.generations.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// `Current()`: what Settings shows. A listener that has stopped accepting for good ends the whole sharing first.
    pub fn state(&self) -> RemoteHostState {
        self.stop_if_listener_failed();
        let gate = lock(&self.0.gate);
        RemoteHostState {
            active_runs: gate.jobs.values().filter(|j| !j.done).count(),
            allows_full_access: gate.allows_full_access,
            ..gate.state.clone()
        }
    }

    /// `SetAllowsFullAccess(allowed)`: whether clients may run commands and full-access agents here, from their next run.
    pub fn set_allows_full_access(&self, allowed: bool) -> RemoteHostState {
        lock(&self.0.gate).allows_full_access = allowed;
        self.state()
    }

    /// `FullAccessRefusal(request, allowed, windowsHost)`: what a client may not run here while full access is off: a command pane, an
    /// agent in full access, and on a Windows host a Codex or Antigravity agent in any mode; None when it may.
    pub fn full_access_refusal(request: &StartRunRequest, allowed: bool, windows_host: bool) -> Option<String> {
        if allowed {
            None
        } else if request.kind == "shell" {
            Some(get("remote.host.error.commandsOff"))
        } else if request.provider != "claude" && windows_host {
            Some(strings::host_agent_needs_full_access(&request.provider))
        } else if request.settings.as_ref().is_some_and(|s| s.permission_mode == "fullAccess") {
            Some(get("remote.host.error.fullAccessOff"))
        } else {
            None
        }
    }

    /// `StartAsync(workspaceIds, port)`: starts sharing on every Tailscale address this PC has, with a new key. Err is what the user reads.
    pub async fn start(&self, workspace_ids: &[String], port: u16) -> Result<RemoteHostState, String> {
        let revision;
        {
            let mut gate = lock(&self.0.gate);
            if gate.changing || gate.state.enabled || !gate.servers.is_empty() {
                return Err(get("remote.host.error.busy"));
            }
            let distinct: HashSet<&String> = workspace_ids.iter().collect();
            if workspace_ids.is_empty()
                || workspace_ids.len() > protocol::MAXIMUM_SHARED_WORKSPACES
                || distinct.len() != workspace_ids.len()
                || !workspace_ids.iter().all(|id| wire::identifier(id))
            {
                return Err(get("remote.host.error.pickWorkspace"));
            }
            if !(port >= 1024 || self.0.options.allow_loopback_for_tests && port == 0) {
                return Err(get("remote.connect.error.badPort"));
            }
            if gate.closed {
                return Err(get("common.appClosing"));
            }
            // Taken before the first wait: a stop meanwhile changes it and ends this start.
            gate.changing = true;
            revision = self.next_generation();
            gate.generation = revision;
        }
        let mut binding = false;
        let result = self.start_inner(workspace_ids, port, revision, &mut binding).await;
        let outcome = match result {
            Ok(()) => Ok(self.state()),
            Err(error) => {
                let mine = lock(&self.0.gate).generation == revision;
                // A stop that came in meanwhile already closed what this start had opened.
                if mine {
                    self.stop_now(Some(revision)).await;
                }
                Err(match error {
                    StartFailure::Cancelled => get("remote.host.error.startCancelled"),
                    _ if binding && !mine => get("remote.host.error.startCancelled"),
                    StartFailure::Bind(error) => mighty_run::strings::get_with("remote.host.error.listenFailed", &[("error", &error)]),
                    StartFailure::Message(message) => message,
                })
            }
        };
        lock(&self.0.gate).changing = false;
        outcome
    }

    fn checked(&self, revision: u64) -> Result<(), StartFailure> {
        let gate = lock(&self.0.gate);
        if gate.generation != revision || gate.closed { Err(StartFailure::Cancelled) } else { Ok(()) }
    }

    async fn start_inner(&self, workspace_ids: &[String], port: u16, revision: u64, binding: &mut bool) -> Result<(), StartFailure> {
        let options = &self.0.options;
        let saved = (options.workspaces)();
        for id in workspace_ids {
            if !saved.iter().any(|w| w.id() == id && w.remote.is_none()) {
                return Err(StartFailure::Message(strings::host_local_only()));
            }
            (options.resolve_local)(id.clone()).await.map_err(StartFailure::Message)?;
        }
        self.checked(revision)?;
        let addresses = self.listen_addresses()?;
        if addresses.is_empty() {
            return Err(StartFailure::Message(strings::host_no_tailscale()));
        }
        let key = new_key().ok_or_else(|| StartFailure::Message(get("remote.host.error.keyFailed")))?;
        let created = (options.create_runner)(self.sinks(revision));
        let current = {
            let mut gate = lock(&self.0.gate);
            let current = gate.generation == revision && !gate.closed;
            if current {
                gate.runner = Some(created.clone());
            }
            current
        };
        if !current {
            created.dispose().await;
            return Err(StartFailure::Cancelled);
        }
        let mut bound = port;
        *binding = true;
        for address in &addresses {
            let ip: IpAddr = address.parse().map_err(|_| StartFailure::Message(strings::host_no_tailscale()))?;
            let server = self.listener(ip, bound, revision, &key);
            // The window a check-then-add in two steps would leave open; the checks hold a stop here.
            let hook = lock(&self.0.before_bind).clone();
            if let Some(hook) = hook {
                hook(address.clone()).await;
            }
            // Recorded under the same lock as the check, so a stop meanwhile always closes it; one refused here was never started.
            let refused = {
                let mut gate = lock(&self.0.gate);
                let refused = gate.generation != revision || gate.closed;
                if !refused {
                    gate.servers.push(server.clone());
                }
                refused
            };
            if refused {
                server.dispose().await;
                return Err(StartFailure::Cancelled);
            }
            bound = server.start().map_err(|e| StartFailure::Bind(e.to_string()))?;
        }
        *binding = false;
        let mut gate = lock(&self.0.gate);
        if gate.generation != revision || gate.closed {
            return Err(StartFailure::Cancelled);
        }
        gate.state = RemoteHostState {
            enabled: true,
            key: Some(key),
            port: Some(bound),
            workspace_ids: workspace_ids.to_vec(),
            addresses: addresses.iter().map(|a| RemoteAddress { host: a.clone(), port: bound }.display()).collect(),
            ..RemoteHostState::default()
        };
        gate.allows_full_access = false;
        let (stop, stopping) = watch::channel(false);
        gate.lease = Some(stop);
        let host = self.clone();
        tokio::spawn(async move { host.lease_loop(revision, stopping).await });
        Ok(())
    }

    fn listener(&self, ip: IpAddr, port: u16, revision: u64, key: &str) -> HttpListener {
        let weak: Weak<HostInner> = Arc::downgrade(&self.0);
        let handler: crate::remote::http_server::Handler = Arc::new(move |request: HttpRequest| {
            let weak = weak.clone();
            Box::pin(async move {
                match weak.upgrade() {
                    Some(inner) => Ok(RemoteHost(inner).serve(request, revision).await),
                    None => Err(Failed),
                }
            })
        });
        let loopback = self.0.options.allow_loopback_for_tests;
        let key = key.to_string();
        let body_limit: crate::remote::http_server::BodyLimit = Arc::new(move |head: &HttpRequest| Ok(body_limit(head, &key, loopback)));
        let accept_peer: crate::remote::http_server::AcceptPeer = Arc::new(move |peer: IpAddr| Ok(peer_ip_allowed(peer, loopback)));
        let options = ListenerOptions {
            body_limit: Some(body_limit),
            accept_peer: Some(accept_peer),
            maximum_connections_per_peer: Some(protocol::MAXIMUM_CONNECTIONS_PER_PEER),
            ..ListenerOptions::default()
        };
        HttpListener::new(self.0.options.network.clone(), ip, port, handler, options)
    }

    /// This PC's Tailscale addresses (or the injected ones), each once; never any other address.
    fn listen_addresses(&self) -> Result<Vec<String>, StartFailure> {
        let options = &self.0.options;
        let found = match &options.addresses {
            Some(addresses) => addresses().map_err(|_| StartFailure::Message(strings::host_no_tailscale()))?,
            None if options.allow_loopback_for_tests => vec!["127.0.0.1".into()],
            None => vec![],
        };
        let mut kept: Vec<String> = vec![];
        for address in found.iter().filter(|a| peer_allowed(a, options.allow_loopback_for_tests)).map(|a| canonical(a)) {
            if !kept.contains(&address) {
                kept.push(address);
            }
        }
        Ok(kept)
    }

    /// `StopAsync()`: stops sharing: every remote run here stops and the key is gone for good.
    pub async fn stop(&self) -> RemoteHostState {
        self.stop_now(None).await;
        self.state()
    }

    /// Ends the sharing (only `only`'s, when given): the state, key and switch are gone before the first wait.
    pub(crate) async fn stop_now(&self, only: Option<u64>) {
        if let Some(taken) = self.take(only) {
            self.finish(taken).await;
        }
    }

    /// The part of a stop that happens at once, under the gate: a new generation, the listeners, the runner, the jobs and the key taken.
    fn take(&self, only: Option<u64>) -> Option<Taken> {
        let mut gate = lock(&self.0.gate);
        if only.is_some_and(|expected| expected != gate.generation) {
            return None;
        }
        gate.generation = self.next_generation();
        let servers = std::mem::take(&mut gate.servers);
        let runner = gate.runner.take();
        gate.state = RemoteHostState::default();
        gate.allows_full_access = false;
        if let Some(lease) = gate.lease.take() {
            lease.send_replace(true);
        }
        let mut leases = vec![];
        let mut launches = vec![];
        for (_, mut job) in gate.jobs.drain() {
            launches.push(job.cancel.clone());
            if let Some(lease) = job.lease.take() {
                leases.push(lease);
            }
        }
        gate.reported_rules.clear();
        Some(Taken { servers, runner, leases, launches })
    }

    async fn finish(&self, taken: Taken) {
        // The temporary Antigravity rules of the runs that end here leave agy's settings with them.
        if let Some(rules) = &self.0.options.antigravity_rules {
            for held in &taken.leases {
                rules.release(held);
            }
        }
        // A run not handed to the runner yet never is; the runner's own runs end with its disposal below.
        for launch in taken.launches {
            launch.send_replace(true);
        }
        // Every live connection is reset, and a request still being handled answers 503: its generation is gone.
        for server in taken.servers {
            server.dispose().await;
        }
        if let Some(runner) = taken.runner {
            runner.dispose().await;
        }
        self.announce_runs();
    }

    /// `AnnounceRuns`: tells the app how many client runs go on now, serialized and read at the moment it is told, never under the gate;
    /// a listener of it that fails changes nothing.
    pub(crate) fn announce_runs(&self) {
        let _serial = lock(&self.0.announcing);
        let count = lock(&self.0.gate).jobs.values().filter(|j| !j.done).count();
        let told = self.0.options.runs_changed.clone();
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| told(count)));
    }

    /// `StopIfListenerFailed`: a listener that stopped accepting for good ends the whole sharing, so it never shows as on while an address
    /// is dead: the key is revoked and the other addresses close. True when it did.
    pub(crate) fn stop_if_listener_failed(&self) -> bool {
        let revision = {
            let gate = lock(&self.0.gate);
            if !gate.state.enabled || gate.servers.iter().all(HttpListener::is_listening) {
                return false;
            }
            gate.generation
        };
        let Ok(handle) = tokio::runtime::Handle::try_current() else { return false };
        let Some(taken) = self.take(Some(revision)) else { return false };
        let host = self.clone();
        handle.spawn(async move { host.finish(taken).await });
        true
    }

    /// `DisposeAsync`: the app quits.
    pub async fn dispose(&self) {
        lock(&self.0.gate).closed = true;
        self.stop_now(None).await;
    }

    /// `Authorize`: sharing on, no browser origin, a Tailscale peer, within the rate limits, the right key, the protocol version.
    fn authorize(&self, request: &HttpRequest, revision: u64) -> Result<(), HostFailure> {
        let key = {
            let gate = lock(&self.0.gate);
            match &gate.state.key {
                Some(key) if gate.state.enabled && gate.generation == revision && gate.servers.iter().all(HttpListener::is_listening) => {
                    key.clone()
                }
                _ => return Err(stopped()),
            }
        };
        if request.headers.contains_key("origin") || !peer_allowed(&request.remote_address, self.0.options.allow_loopback_for_tests) {
            return Err(HostFailure::new(403, get("remote.host.error.tailscaleOnly")));
        }
        if !self.0.rates.allow_peer(&request.remote_address) {
            return Err(HostFailure::new(429, get("remote.host.error.tooMany")));
        }
        if !key_presented(&request.headers, &key) {
            return Err(HostFailure::new(401, get("remote.host.error.badKey")));
        }
        if !self.0.rates.allow_keyed() {
            return Err(HostFailure::new(429, get("remote.host.error.tooMany")));
        }
        if request.headers.get(protocol::HEADER).map(String::as_str) != Some("2") {
            return Err(HostFailure::new(426, get("remote.host.error.version")));
        }
        Ok(())
    }

    async fn serve(&self, request: HttpRequest, revision: u64) -> HttpResponse {
        match self.route(&request, revision).await {
            Ok(response) => response,
            Err(failure) => failure_response(failure.status, &failure.message),
        }
    }

    async fn route(&self, request: &HttpRequest, revision: u64) -> Result<HttpResponse, HostFailure> {
        self.authorize(request, revision)?;
        // The path is read as written, as the Mac reads it: no dot segment is resolved and no escape decoded.
        let (path, query) = match request.target.find('?') {
            Some(mark) => (&request.target[..mark], &request.target[mark..]),
            None => (request.target.as_str(), ""),
        };
        if !target_text(&request.target) || !path.starts_with(protocol::PATH_PREFIX) {
            return Err(HostFailure::new(400, get("remote.host.error.badPath")));
        }
        let route: Vec<&str> = path[protocol::PATH_PREFIX.len()..].split('/').collect();
        if request.method == "GET" && route == ["info"] && query.is_empty() {
            return self.info(revision).await;
        }
        self.serve_runs(request, &route, query, revision).await
    }

    pub(crate) fn shared_workspaces(&self) -> Vec<Workspace> {
        let ids = lock(&self.0.gate).state.workspace_ids.clone();
        (self.0.options.workspaces)()
            .into_iter()
            .filter(|w| {
                w.remote.is_none()
                    && ids.iter().any(|id| id == w.id())
                    && validation::shared_workspace(&RemoteSharedWorkspace {
                        id: w.id().into(),
                        name: w.name().into(),
                        path: w.path().into(),
                    })
            })
            .collect()
    }

    /// This PC's CLIs within the 30 s /info waits for them.
    pub(crate) async fn runtime(&self) -> Result<RuntimeInfo, HostFailure> {
        tokio::time::timeout(Duration::from_secs(30), (self.0.options.runtime)())
            .await
            .map_err(|_| HostFailure::new(503, strings::timeout()))
    }

    async fn info(&self, revision: u64) -> Result<HttpResponse, HostFailure> {
        let shared = self.shared_workspaces();
        // Each provider's list as long as a client keeps it.
        let runtime = validation::bounded(&self.runtime().await?);
        if lock(&self.0.gate).generation != revision {
            return Err(stopped());
        }
        let name = text::cut(&self.0.options.host_name, 120);
        let info = RemoteWireInfo {
            protocol: protocol::VERSION,
            host_id: self.0.host_id.clone(),
            host_name: if name.is_empty() { "Windows".into() } else { name },
            workspaces: shared
                .iter()
                .map(|w| RemoteSharedWorkspace { id: w.id().into(), name: w.name().into(), path: w.path().into() })
                .collect(),
            runtime,
        };
        Ok(reply(200, &info.to_value()))
    }
}

struct Taken {
    servers: Vec<HttpListener>,
    runner: Option<Arc<dyn HostRunner>>,
    leases: Vec<mighty_run::antigravity::Lease>,
    launches: Vec<runs::Cancel>,
}

enum StartFailure {
    Cancelled,
    Bind(String),
    Message(String),
}

/// `BodyLimit(head, key, allowLoopback)`: how much body is read, decided from the head before any of it: none without the key (the
/// head alone gets its 401), the larger attachment body only for an authenticated run request.
pub fn body_limit(head: &HttpRequest, key: &str, allow_loopback: bool) -> usize {
    if head.headers.contains_key("origin")
        || !peer_allowed(&head.remote_address, allow_loopback)
        || head.headers.get(protocol::HEADER).map(String::as_str) != Some("2")
        || !key_presented(&head.headers, key)
    {
        return 0;
    }
    if head.method == "POST" && head.target == format!("{}runs", protocol::PATH_PREFIX) {
        mighty_run::attachments::MAXIMUM_REQUEST_BYTES
    } else {
        protocol::MAXIMUM_REQUEST_BYTES
    }
}

/// The request's bearer key is the sharing's (at most 256 characters in all, compared in fixed time).
fn key_presented(headers: &HashMap<String, String>, key: &str) -> bool {
    let authorization = headers.get("authorization").map_or("", String::as_str);
    authorization.len() <= 256 && authorization.strip_prefix("Bearer ").is_some_and(|presented| validation::keys_match(presented, key))
}

/// `Reply(status, value)`: a route's answer, with the protocol header.
pub(crate) fn reply(status: u16, value: &Value) -> HttpResponse {
    HttpResponse::json(status, value, &[(protocol::HEADER, "2")])
}

/// `Failure(status, message)`: `{protocol, error}` with the message cut at 1000 characters.
pub(crate) fn failure_response(status: u16, message: &str) -> HttpResponse {
    reply(status, &RemoteWireError { protocol: Some(protocol::VERSION), error: text::cut(message, 1000) }.to_value())
}

type Reader = Arc<dyn Fn() -> BoxFuture<'static, Result<RuntimeInfo, String>> + Send + Sync>;

/// One reading of this PC's CLIs as [`HostRuntime::read`] hands it out: every caller of the same reading waits for the same answer.
pub struct Reading {
    answer: tokio::sync::watch::Receiver<Option<Result<RuntimeInfo, String>>>,
}

impl Reading {
    /// The reading's answer, once it has one.
    pub async fn wait(&self) -> Result<RuntimeInfo, String> {
        let mut answer = self.answer.clone();
        match answer.wait_for(Option::is_some).await {
            Ok(value) => value.clone().unwrap_or_else(|| Err("the reading ended".into())),
            Err(_) => Err("the reading ended".into()),
        }
    }

    fn done(&self) -> bool {
        self.answer.borrow().is_some()
    }
}

#[derive(Default)]
struct Readings {
    current: Option<Arc<Reading>>,
    /// When the current reading finished well; None while it reads or after it failed.
    read_at: Option<std::time::Instant>,
    /// Moves on each new reading and each forget, so a reading that finishes after either never stamps itself fresh.
    generation: u64,
}

/// C# `DesktopService.HostRuntimeAsync`, `ForgetHostRuntime`, `HostRuntimeFresh` and `HostRuntimeReader`: this PC's CLIs as a sharing
/// host reports them. Every /info and every client agent start asks; one reading is shared by all who ask while it goes, and kept for
/// the fresh time after it finished; one that failed is read again by the next caller.
pub struct HostRuntime {
    reader: Reader,
    app_version: String,
    fresh: Mutex<Duration>,
    readings: Arc<Mutex<Readings>>,
}

impl HostRuntime {
    /// `reader` reads this PC's CLIs (C#: `Providers.GetRuntimeAsync()`); each answer carries `app_version`.
    pub fn new(reader: Reader, app_version: &str) -> HostRuntime {
        HostRuntime { reader, app_version: app_version.into(), fresh: Mutex::new(Duration::from_secs(30)), readings: Arc::default() }
    }

    /// `HostRuntimeFresh`: how long one reading, from the moment it finished, answers every caller (checks shorten it).
    pub fn set_fresh(&self, fresh: Duration) {
        *lock(&self.fresh) = fresh;
    }

    /// `HostRuntimeAsync(appVersion)`: the reading going on or kept, else a new one.
    pub fn read(&self) -> Arc<Reading> {
        let fresh = *lock(&self.fresh);
        let mut readings = lock(&self.readings);
        if let Some(known) = &readings.current
            && (!known.done() || readings.read_at.is_some_and(|at| at.elapsed() < fresh))
        {
            return known.clone();
        }
        readings.generation += 1;
        readings.read_at = None;
        let generation = readings.generation;
        let (answer, receiver) = tokio::sync::watch::channel(None);
        let reading = Arc::new(Reading { answer: receiver });
        readings.current = Some(reading.clone());
        let (reader, version, shared) = (self.reader.clone(), self.app_version.clone(), self.readings.clone());
        tokio::spawn(async move {
            let value = reader().await.map(|runtime| RuntimeInfo { app_version: version, ..runtime });
            {
                let mut readings = lock(&shared);
                if value.is_ok() && readings.generation == generation {
                    readings.read_at = Some(std::time::Instant::now());
                }
            }
            let _ = answer.send(Some(value));
        });
        reading
    }

    /// `ForgetHostRuntime`: a CLI was updated, installed or signed in: the next caller reads them again.
    pub fn forget(&self) {
        let mut readings = lock(&self.readings);
        readings.generation += 1;
        readings.current = None;
        readings.read_at = None;
    }
}
