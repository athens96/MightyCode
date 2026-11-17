//! C#'s `RemoteFakeHost` (a scripted HTTP/1.1 server on loopback for the client's checks: one request per connection, answers built by
//! the check), the host checks' `FakeRemoteRunner` and `MakeHost` (RemoteHostVerification.cs), and what the client checks share
//! (`Collected`, `Loopback`, `ClientStore`). Loopback port 0 only. Owner: W8-D.

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use indexmap::IndexMap;
use mighty_core::json::Wire;
use mighty_core::locale::Locale;
use mighty_core::model::{AppSnapshot, PlanRecord, Workspace};
use mighty_core::permission::ToolPermissionRequest;
use mighty_core::store::Store;
use mighty_graph::node::ExecutionGraphNode;
use mighty_platform::BoxFuture;
use mighty_platform::fake::FakeNetwork;
use mighty_remote::remote::address::RemoteAddress;
use mighty_remote::remote::host::{Clock, HostOptions, HostRunner, RemoteHost, RunnerSinks};
use mighty_remote::remote::transport::RemoteTarget;
use mighty_remote::remote::wire::{RemoteWireInfo, protocol};
use mighty_run::model::{RunEvent, RuntimeInfo, StartRunRequest};
use mighty_run::plan_mode::PlanDecision;
use mighty_run::questionnaire::UserQuestionAnswer;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

pub const KEY: &str = "kkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkk";

pub fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../contracts/fixtures/remote-v2")
}

pub fn fixture(name: &str) -> Value {
    serde_json::from_slice(&std::fs::read(fixtures().join(name)).unwrap()).unwrap()
}

pub fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mighty-remote-{name}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A fixture's runtime (`info.json`'s, or another /info sample's).
pub fn runtime(name: &str) -> RuntimeInfo {
    RemoteWireInfo::decode(&serde_json::to_vec(&fixture(name)).unwrap()).unwrap().runtime
}

/// C# `RemoteClientVerification.Loopback(port)`: a target on loopback with the injected flag.
pub fn loopback(port: u16) -> RemoteTarget {
    RemoteTarget::new(RemoteAddress { host: "127.0.0.1".into(), port }, "127.0.0.1", port, true)
}

/// C# `Verification.Until`: waits for `condition`, failing after `millis`.
pub async fn until(condition: impl Fn() -> bool, millis: u64, what: &str) {
    let end = tokio::time::Instant::now() + Duration::from_millis(millis);
    while !condition() {
        assert!(tokio::time::Instant::now() < end, "timed out waiting: {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// One request as the fake host saw it.
#[derive(Debug, Clone)]
pub struct Seen {
    pub method: String,
    pub target: String,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

/// C# `RemoteFakeHost.Reply`: a stall before the head, no Content-Length (the body ends with the connection), a stall after the head.
#[derive(Debug, Clone, Default)]
pub struct Reply {
    pub status: u16,
    pub body: String,
    pub headers: Vec<(String, String)>,
    pub no_version: bool,
    pub delay: Option<Duration>,
    pub omit_length: bool,
    pub body_delay: Option<Duration>,
}

impl Reply {
    pub fn new(status: u16, body: &str) -> Reply {
        Reply { status, body: body.into(), ..Reply::default() }
    }

    /// `RemoteFakeHost.Fixture(name, status)`.
    pub fn fixture(name: &str, status: u16) -> Reply {
        Reply::new(status, &fixture(name).to_string())
    }
}

pub type Answer = Arc<dyn Fn(&Seen) -> BoxFuture<'static, Reply> + Send + Sync>;

/// C# `RemoteFakeHost`.
pub struct FakeHost {
    pub port: u16,
    pub seen: Arc<Mutex<Vec<Seen>>>,
    task: tokio::task::JoinHandle<()>,
}

impl FakeHost {
    pub async fn new(answer: impl Fn(&Seen) -> Reply + Send + Sync + 'static) -> FakeHost {
        let answer = Arc::new(answer);
        FakeHost::with(Arc::new(move |seen: &Seen| {
            let reply = answer(seen);
            Box::pin(async move { reply })
        }))
        .await
    }

    pub async fn with(answer: Answer) -> FakeHost {
        let listener = TcpListener::bind(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = Arc::new(Mutex::new(vec![]));
        let record = seen.clone();
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let (answer, record) = (answer.clone(), record.clone());
                tokio::spawn(async move { serve(stream, answer, record).await });
            }
        });
        FakeHost { port, seen, task }
    }

    pub fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}

impl Drop for FakeHost {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn serve(mut stream: TcpStream, answer: Answer, record: Arc<Mutex<Vec<Seen>>>) {
    let mut buffer = vec![];
    let mut chunk = [0u8; 8192];
    let end = loop {
        if let Some(at) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
            break at;
        }
        match stream.read(&mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(read) => buffer.extend_from_slice(&chunk[..read]),
        }
    };
    let head = String::from_utf8_lossy(&buffer[..end]).into_owned();
    let mut lines = head.split("\r\n");
    let line: Vec<String> = lines.next().unwrap_or_default().split(' ').map(str::to_string).collect();
    let headers: HashMap<String, String> =
        lines.filter_map(|h| h.split_once(':')).map(|(name, value)| (name.trim().to_lowercase(), value.trim().to_string())).collect();
    let length: usize = headers.get("content-length").and_then(|l| l.parse().ok()).unwrap_or(0);
    let mut body = buffer[end + 4..].to_vec();
    while body.len() < length {
        match stream.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(read) => body.extend_from_slice(&chunk[..read]),
        }
    }
    let seen = Seen { method: line[0].clone(), target: line.get(1).cloned().unwrap_or_default(), headers, body };
    record.lock().unwrap().push(seen.clone());
    let reply = answer(&seen).await;
    if let Some(delay) = reply.delay {
        tokio::time::sleep(delay).await;
    }
    let mut response = format!("HTTP/1.1 {} X\r\nContent-Type: application/json\r\nConnection: close\r\n", reply.status);
    if !reply.omit_length {
        response.push_str(&format!("Content-Length: {}\r\n", reply.body.len()));
    }
    if !reply.no_version {
        response.push_str("x-mighty-remote-version: 2\r\n");
    }
    for (name, value) in &reply.headers {
        response.push_str(&format!("{name}: {value}\r\n"));
    }
    response.push_str("\r\n");
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.flush().await;
    if let Some(stall) = reply.body_delay {
        tokio::time::sleep(stall).await;
    }
    let _ = stream.write_all(reply.body.as_bytes()).await;
    let _ = stream.shutdown().await;
}

/// The host checks' scripted runner (C# `FakeRemoteRunner`): "hold" waits for a stop, "ask" asks one tool request, "graph" sends a node
/// and a graph run, "flood" and "bulk" send more than the host keeps (in events and in bytes).
pub struct FakeRunner {
    sinks: RunnerSinks,
    pub started: Mutex<Vec<StartRunRequest>>,
    pub stopped: Mutex<Vec<String>>,
    pub answers: Mutex<Vec<(String, String, bool)>>,
    waiting: Mutex<HashMap<String, bool>>,
    pub disposed: AtomicBool,
}

impl FakeRunner {
    pub fn new(sinks: RunnerSinks) -> Arc<FakeRunner> {
        Arc::new(FakeRunner {
            sinks,
            started: Mutex::new(vec![]),
            stopped: Mutex::new(vec![]),
            answers: Mutex::new(vec![]),
            waiting: Mutex::new(HashMap::new()),
            disposed: AtomicBool::new(false),
        })
    }

    fn event(&self, value: RunEvent) {
        (self.sinks.event)(value);
    }

    fn stop_now(&self, id: &str) {
        self.stopped.lock().unwrap().push(id.to_string());
        if self.waiting.lock().unwrap().remove(id).is_some() {
            self.event(RunEvent::state(id, "stopped", None));
        }
    }
}

impl HostRunner for FakeRunner {
    fn start(&self, request: StartRunRequest) -> BoxFuture<'_, mighty_run::Result<()>> {
        let id = request.session_id.clone();
        self.started.lock().unwrap().push(request.clone());
        self.event(RunEvent::state(&id, "running", None));
        self.event(RunEvent::log(&id, "output", &format!("echo:{}", request.input), None));
        match request.input.as_str() {
            "hold" => {
                self.waiting.lock().unwrap().insert(id.clone(), true);
            }
            "ask" => {
                self.waiting.lock().unwrap().insert(id.clone(), true);
                let mut ask = ToolPermissionRequest::new("perm-1", &id, "toolu_1", "Bash", "{\"command\":\"ls\"}", "ls");
                ask.received_at = Some("2026-10-17T01:02:05.000Z".into());
                (self.sinks.permission)(ask);
            }
            "graph" => {
                let node = ExecutionGraphNode::new("node-1".into(), "run-1".into(), None, "main", "completed", "Request".into());
                (self.sinks.graph_node)(&id, serde_json::from_str(&node.to_json()).unwrap());
                let run = mighty_core::graph::MightyGraphRun::read(&fixture("graph-run-event.json")["event"]["graphRun"]).unwrap();
                let mut event = RunEvent::new(&id, "graph_run");
                event.graph_run = Some(run);
                self.event(event);
                self.event(RunEvent::state(&id, "completed", None));
            }
            "flood" => {
                for i in 0..5000 {
                    self.event(RunEvent::log(&id, "output", &format!("line {i}"), None));
                }
                self.event(RunEvent::state(&id, "completed", None));
            }
            "bulk" => {
                for i in 0..200u32 {
                    let letter = char::from(b'a' + (i % 26) as u8);
                    self.event(RunEvent::log(&id, "output", &letter.to_string().repeat(30_000), None));
                }
                self.event(RunEvent::state(&id, "completed", None));
            }
            _ => self.event(RunEvent::state(&id, "completed", None)),
        }
        Box::pin(async { Ok(()) })
    }
    fn stop(&self, session_id: String) -> BoxFuture<'_, mighty_run::Result<()>> {
        self.stop_now(&session_id);
        Box::pin(async { Ok(()) })
    }
    fn respond_to_tool_permission(&self, session_id: &str, request_id: &str, allow: bool) -> mighty_run::Result<()> {
        self.answers.lock().unwrap().push((session_id.into(), request_id.into(), allow));
        let mut settled = ToolPermissionRequest::new(request_id, session_id, "toolu_1", "Bash", "{}", "ls");
        settled.state = if allow { "allowed" } else { "denied" }.into();
        (self.sinks.permission)(settled);
        if self.waiting.lock().unwrap().remove(session_id).is_some() {
            self.event(RunEvent::state(session_id, "completed", None));
        }
        Ok(())
    }
    fn answer_questionnaire(&self, session_id: &str, request_id: &str, _: &IndexMap<String, UserQuestionAnswer>) -> mighty_run::Result<()> {
        self.answers.lock().unwrap().push((session_id.into(), request_id.into(), true));
        Ok(())
    }
    fn answer_plan(&self, session_id: &str, request_id: &str, decision: &PlanDecision) -> mighty_run::Result<PlanRecord> {
        self.answers.lock().unwrap().push((session_id.into(), request_id.into(), decision.kind != "cancel"));
        let now = mighty_core::time::now_o();
        Ok(PlanRecord {
            id: Some(request_id.into()),
            run_id: Some(session_id.into()),
            plan: Some("plan".into()),
            received_at: Some(now.clone()),
            decided_at: Some(now),
            outcome: Some(decision.outcome().into()),
            feedback: None,
            plan_truncated: None,
            graph_run_id: None,
            launch_override: None,
        })
    }
    fn dispose(&self) -> BoxFuture<'_, ()> {
        self.disposed.store(true, Ordering::SeqCst);
        let waiting: Vec<String> = self.waiting.lock().unwrap().keys().cloned().collect();
        for id in waiting {
            self.stop_now(&id);
        }
        Box::pin(async {})
    }
}

/// A store the host checks read their workspaces from (C# `StateStore` with `ResolveLocalAsync`).
pub type SharedStore = Arc<Mutex<Store>>;

/// C# `StateStore.ResolveLocalAsync`: a local workspace the state has, its folder there.
pub fn resolve_local(store: &SharedStore, id: &str) -> Result<Workspace, String> {
    let store = store.lock().unwrap();
    let workspace =
        store.snapshot.workspace(id).cloned().ok_or_else(|| mighty_run::strings::get("run.error.localWorkspaceNotRegistered"))?;
    if workspace.remote.is_some() {
        return Err(mighty_run::strings::get("run.error.approvedWorkspaceOnly"));
    }
    if !Path::new(workspace.path()).is_dir() {
        return Err(mighty_run::strings::get("store.error.workspaceFolderNotFound"));
    }
    Ok(workspace)
}

/// C# `HostFixture`.
pub struct HostFixture {
    pub directory: PathBuf,
    pub store: SharedStore,
    pub shared: Workspace,
    pub private: Workspace,
    pub remote: Workspace,
    pub host: RemoteHost,
    pub runner: Arc<Mutex<Option<Arc<FakeRunner>>>>,
    pub network: FakeNetwork,
}

impl HostFixture {
    pub fn runner(&self) -> Option<Arc<FakeRunner>> {
        self.runner.lock().unwrap().clone()
    }

    pub async fn dispose(self) {
        self.host.dispose().await;
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

pub type RunnerFactory = Arc<dyn Fn(&RemoteHost, RunnerSinks) -> Arc<dyn HostRunner> + Send + Sync>;

/// C# `MakeHost`'s parameters.
#[derive(Default)]
pub struct MakeHost {
    pub heartbeat: Option<Duration>,
    pub admission: Option<mighty_remote::remote::host::Admission>,
    pub runner: Option<RunnerFactory>,
    pub addresses: Option<mighty_remote::remote::host::Addresses>,
    pub clock: Option<Clock>,
    pub runs_changed: Option<Arc<dyn Fn(usize) + Send + Sync>>,
    pub windows_host: Option<bool>,
    /// `HostOptions::decodes_paths` (default: Windows', false).
    pub decodes_paths: Option<bool>,
    /// `HostOptions::graph_runs` (default: Windows', true).
    pub graph_runs: Option<bool>,
    pub runtime: Option<&'static str>,
}

/// C# `MakeHost`: a store with a shared and a private local workspace and a remote one, and a host over it on loopback.
pub async fn make_host(make: MakeHost) -> HostFixture {
    let directory = temp("host");
    let mut store = Store::new(directory.join("host"), None, true);
    store.load(&Locale::new("en")).unwrap();
    std::fs::create_dir_all(directory.join("project")).unwrap();
    std::fs::create_dir_all(directory.join("private")).unwrap();
    let shared = store.approve_local(&directory.join("project")).unwrap();
    let hidden = store.approve_local(&directory.join("private")).unwrap();
    let remote = store.approve_remote("conn-x", "ws-x", "far", "C:\\far", "other").unwrap();
    store.save(&AppSnapshot { workspaces: Some(vec![shared.clone(), hidden.clone(), remote.clone()]), ..AppSnapshot::default() }).unwrap();
    let store: SharedStore = Arc::new(Mutex::new(store));
    let runner: Arc<Mutex<Option<Arc<FakeRunner>>>> = Arc::new(Mutex::new(None));
    let network = FakeNetwork::default();
    let host_cell: Arc<Mutex<Option<RemoteHost>>> = Arc::new(Mutex::new(None));
    let (workspaces_store, resolve_store, made, cell) = (store.clone(), store.clone(), runner.clone(), host_cell.clone());
    let factory = make.runner.clone();
    let runtime_name = make.runtime.unwrap_or("info.json");
    let options = HostOptions {
        workspaces: Arc::new(move || workspaces_store.lock().unwrap().snapshot.workspaces().to_vec()),
        resolve_local: Arc::new(move |id: String| {
            let found = resolve_local(&resolve_store, &id);
            Box::pin(async move { found })
        }),
        runtime: Arc::new(move || {
            let value = runtime(runtime_name);
            Box::pin(async move { value })
        }),
        create_runner: Arc::new(move |sinks| {
            if let Some(factory) = &factory {
                let host = cell.lock().unwrap().clone().unwrap();
                return factory(&host, sinks);
            }
            let fake = FakeRunner::new(sinks);
            *made.lock().unwrap() = Some(fake.clone());
            fake
        }),
        host_name: "studio-pc".into(),
        admission: make.admission.unwrap_or_else(|| Arc::new(|_| None)),
        runs_changed: make.runs_changed.unwrap_or_else(|| Arc::new(|_| {})),
        allow_loopback_for_tests: true,
        heartbeat: make.heartbeat.unwrap_or(protocol::HEARTBEAT_TIMEOUT),
        addresses: make.addresses,
        clock: make.clock.unwrap_or_else(mighty_remote::remote::host::monotonic),
        windows_host: make.windows_host.unwrap_or(cfg!(windows)),
        decodes_paths: make.decodes_paths.unwrap_or(false),
        graph_runs: make.graph_runs.unwrap_or(true),
        antigravity_rules: None,
        system_folder: None,
        network: Arc::new(network.clone()),
    };
    let host = RemoteHost::new(options);
    *host_cell.lock().unwrap() = Some(host.clone());
    HostFixture { directory, store, shared, private: hidden, remote, host, runner, network }
}

/// C# `Raw(port, head, body, address)`: one raw exchange; "" for a peer cut off at accept.
pub async fn raw(port: u16, head: &[u8], body: Option<&[u8]>, v6: bool) -> String {
    let address: std::net::IpAddr = if v6 { std::net::Ipv6Addr::LOCALHOST.into() } else { Ipv4Addr::LOCALHOST.into() };
    let Ok(mut stream) = TcpStream::connect((address, port)).await else { return String::new() };
    let _ = stream.write_all(head).await;
    if let Some(body) = body {
        let _ = stream.write_all(body).await;
    }
    read_all(&mut stream).await
}

/// `Raw` with a text head on IPv4 loopback.
pub async fn raw_text(port: u16, head: &str) -> String {
    raw(port, head.as_bytes(), None, false).await
}

/// C# `Open(port, head)`: a connection with `head` sent, left open.
pub async fn open(port: u16, head: &str) -> TcpStream {
    let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).await.unwrap();
    stream.write_all(head.as_bytes()).await.unwrap();
    stream
}

/// C# `ReadAll`: until the server ends the connection (or 10 s pass).
pub async fn read_all(stream: &mut TcpStream) -> String {
    let mut answer = vec![];
    let mut buffer = [0u8; 8192];
    let end = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        match tokio::time::timeout_at(end, stream.read(&mut buffer)).await {
            Ok(Ok(read)) if read > 0 => answer.extend_from_slice(&buffer[..read]),
            _ => break,
        }
    }
    String::from_utf8_lossy(&answer).into_owned()
}

/// C# `ClosedWithin`: the server ends the connection (end of stream, a reset) within `within`.
pub async fn closed_within(stream: &mut TcpStream, within: Duration) -> bool {
    let mut buffer = [0u8; 1024];
    let end = tokio::time::Instant::now() + within;
    loop {
        match tokio::time::timeout_at(end, stream.read(&mut buffer)).await {
            Err(_) => return false,
            Ok(Ok(0)) | Ok(Err(_)) => return true,
            Ok(Ok(_)) => {}
        }
    }
}

/// A check's look at each event a client hands its pane.
pub type EventCheck = Arc<dyn Fn(&RunEvent) -> bool + Send + Sync>;
pub type EventHook = Arc<dyn Fn(&RunEvent) + Send + Sync>;

/// C# `RemoteClientVerification.Collected`: the events and tool requests a client handed its panes. `hold`: when it matches an event,
/// the caller's thread waits there (after the event is recorded) until `release`; `after` runs on the caller's thread after each event
/// is recorded (a panic in it is the pane's callback failing).
#[derive(Default)]
pub struct Collected {
    pub events: Mutex<Vec<RunEvent>>,
    pub permissions: Mutex<Vec<ToolPermissionRequest>>,
    pub hold: Mutex<Option<EventCheck>>,
    pub held: AtomicBool,
    released: (Mutex<bool>, std::sync::Condvar),
    pub after: Mutex<Option<EventHook>>,
}

impl Collected {
    pub fn events(&self) -> Vec<RunEvent> {
        self.events.lock().unwrap().clone()
    }
    pub fn permissions(&self) -> Vec<ToolPermissionRequest> {
        self.permissions.lock().unwrap().clone()
    }

    pub fn event(&self, value: RunEvent) {
        self.events.lock().unwrap().push(value.clone());
        let hold = self.hold.lock().unwrap().clone();
        if hold.is_some_and(|h| h(&value)) {
            self.held.store(true, Ordering::SeqCst);
            let (lock, released) = &self.released;
            let guard = lock.lock().unwrap();
            let _ = released.wait_timeout_while(guard, Duration::from_secs(10), |r| !*r).unwrap();
        }
        let after = self.after.lock().unwrap().clone();
        if let Some(after) = after {
            after(&value);
        }
    }

    pub fn release(&self) {
        *self.released.0.lock().unwrap() = true;
        self.released.1.notify_all();
    }

    /// The client's sinks, recording here.
    pub fn sinks(self: &Arc<Self>) -> mighty_remote::remote::client::ClientSinks {
        let (events, permissions) = (self.clone(), self.clone());
        mighty_remote::remote::client::ClientSinks {
            event: Arc::new(move |value| events.event(value)),
            permission: Arc::new(move |value| permissions.permissions.lock().unwrap().push(value)),
        }
    }

    /// Each event's text or status, as C#'s checks list them.
    pub fn words(&self, pane: Option<&str>) -> Vec<String> {
        self.events()
            .iter()
            .filter(|e| pane.is_none_or(|p| e.session_id == p))
            .map(|e| e.entry.as_ref().and_then(|l| l.text.clone()).or_else(|| e.status.clone()).unwrap_or_default())
            .collect()
    }
}

/// C# `ClientStore()`: a loaded store in a folder of its own, shared with the client's approvals.
pub fn client_store() -> (SharedStore, PathBuf) {
    let directory = temp("client");
    let mut store = Store::new(directory.clone(), None, true);
    store.load(&Locale::new("en")).unwrap();
    (Arc::new(Mutex::new(store)), directory)
}

/// The store's `ApproveRemote` as the client calls it.
pub fn approver(store: &SharedStore) -> mighty_remote::remote::client::ApproveRemote {
    let store = store.clone();
    Arc::new(move |connection, workspace, name, path, host| store.lock().unwrap().approve_remote(connection, workspace, name, path, host))
}

/// A client over `store` and `directory` with `vault`, its sinks recording into `seen`.
pub fn client_with(
    store: &SharedStore,
    directory: &Path,
    vault: Arc<dyn mighty_platform::SecretVault>,
    seen: &Arc<Collected>,
    options: mighty_remote::remote::client::ClientOptions,
) -> mighty_remote::remote::client::RemoteClient {
    let client = mighty_remote::remote::client::RemoteClient::new(approver(store), directory.to_path_buf(), vault, options);
    client.set_sinks(seen.sinks());
    client
}

/// The checks' usual options: loopback injected.
pub fn loopback_options() -> mighty_remote::remote::client::ClientOptions {
    mighty_remote::remote::client::ClientOptions { allow_loopback_for_tests: true, ..Default::default() }
}
