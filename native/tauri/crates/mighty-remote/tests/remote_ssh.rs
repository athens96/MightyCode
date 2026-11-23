//! MC-33: the SSH tunnel's host (`RemoteHost::new_ssh_tunnel`) and the client's connection through it (`RemoteClient::connect_ssh`),
//! in one process over loopback. The "forward" is no forward at all: the tunnel listens on loopback already, and the app's hook here
//! hands the client its port and a code the host issued, as the app's ssh would. No ssh, no host, no Tailscale. MC-40's checks put
//! a loopback relay in the forward's place (it keeps going to the old port when the far side restarts, as `ssh -L` does).

mod support;

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use indexmap::IndexMap;
use mighty_core::attachment::RunAttachment;
use mighty_core::model::AppSnapshot;
use mighty_core::permission::ToolPermissionRequest;
use mighty_core::shared_memory::{MemoryScope, SharedMemoryBook, UpsertMemoryInput, infer_project_slug};
use mighty_platform::fake::MemoryVault;
use mighty_platform::{BoxFuture, SecretVault};
use mighty_remote::remote::client::ClientOptions;
use mighty_remote::remote::host::{Clock, RemoteHost, tunnel_peer};
use mighty_remote::remote::memory::{BookMemoryHost, CHANGES_PATH, MemoryChangesWire, PUSH_PATH};
use mighty_remote::remote::mirror::desk::{MirrorDesk, MirrorStage, Spawn};
use mighty_remote::remote::mirror::host::{MirrorHost, Refusal};
use mighty_remote::remote::mirror::{MirrorPane, MirrorPaneInfo, MirrorSummary};
use mighty_remote::remote::ssh_tunnel::{SshLost, SshOpening, SshTunnel, ssh_address};
use mighty_remote::remote::transport::{self, Method, Request};
use mighty_remote::remote::wire::{RemoteConnectionStatus, RemoteWireInfo, protocol};
use mighty_run::plan_mode::PlanDecision;
use mighty_run::questionnaire::UserQuestionAnswer;
use serde_json::Value;
use support::fake_host::*;
use zeroize::Zeroizing;

const MACHINE: &str = "0123456789abcdef0123456789abcdef";

/// A clock the check moves by hand (whole seconds).
fn manual_clock() -> (Clock, Arc<AtomicU64>) {
    let seconds = Arc::new(AtomicU64::new(1000));
    let read = seconds.clone();
    (Arc::new(move || Duration::from_secs(read.load(Ordering::SeqCst))), seconds)
}

async fn pair(port: u16, code: &str) -> Result<Vec<u8>, transport::TransportError> {
    transport::request(&loopback(port), code, Method::Post, "/v2/ssh/pair", Request::default()).await
}

#[tokio::test(flavor = "multi_thread")]
async fn the_tunnel_listens_on_loopback_shares_every_local_workspace_and_pairs_once() {
    let (clock, seconds) = manual_clock();
    let fixture = make_host(MakeHost { ssh_tunnel: true, clock: Some(clock), ..MakeHost::default() }).await;
    assert!(fixture.host.is_ssh_tunnel());
    let state = fixture.host.start(&[], 0).await.expect("the tunnel starts with no workspace named, on any port");
    let port = state.port.unwrap();
    assert!(state.allows_full_access, "the SSH user is this Mac's own user");
    assert_eq!(state.addresses, [format!("127.0.0.1:{port}")], "loopback alone");
    let key = state.key.clone().unwrap();

    // No code yet, and a wrong one, are refused; a wrong try does not take the code from its owner; a newer code replaces the older.
    let wrong = "w".repeat(43);
    assert_eq!(pair(port, &wrong).await.unwrap_err().status(), Some(401));
    let older = fixture.host.issue_ssh_code().unwrap();
    let code = fixture.host.issue_ssh_code().unwrap();
    assert_eq!(pair(port, &older).await.unwrap_err().status(), Some(401), "only the newest code works");
    assert_eq!(pair(port, &wrong).await.unwrap_err().status(), Some(401));

    // The code gives the key once.
    let answer: Value = serde_json::from_slice(&pair(port, &code).await.unwrap()).unwrap();
    assert_eq!(answer["key"].as_str(), Some(key.as_str()));
    assert_eq!(pair(port, &code).await.unwrap_err().status(), Some(401), "one use only");

    // A code a minute old no longer works.
    let code = fixture.host.issue_ssh_code().unwrap();
    seconds.fetch_add(61, Ordering::SeqCst);
    assert_eq!(pair(port, &code).await.unwrap_err().status(), Some(401));

    // A browser page is refused before anything else.
    let code = fixture.host.issue_ssh_code().unwrap();
    let head = format!(
        "POST /v2/ssh/pair HTTP/1.1\r\nHost: 127.0.0.1\r\nOrigin: http://evil.example\r\nAuthorization: Bearer {code}\r\n{}: 2\r\nConnection: close\r\n\r\n",
        protocol::HEADER
    );
    assert!(raw_text(port, &head).await.starts_with("HTTP/1.1 403"));

    // The key reads /info: every local workspace, never another computer's.
    let info = transport::request(&loopback(port), &key, Method::Get, "/v2/info", Request::default()).await.unwrap();
    let info = RemoteWireInfo::decode(&info).unwrap();
    let ids: Vec<&str> = info.workspaces.iter().map(|w| w.id.as_str()).collect();
    assert!(ids.contains(&fixture.shared.id()) && ids.contains(&fixture.private.id()), "{ids:?}");
    assert!(!ids.contains(&fixture.remote.id()));

    // Only loopback is a peer of the tunnel.
    for (peer, allowed) in
        [("127.0.0.1", true), ("::1", true), ("[::ffff:127.0.0.1]", true), ("100.64.0.1", false), ("fd7a:115c:a1e0::1", false)]
    {
        assert_eq!(tunnel_peer(peer), allowed, "{peer}");
    }
    fixture.dispose().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_tailscale_host_has_no_pairing_route_and_issues_no_code() {
    let fixture = make_host(MakeHost::default()).await;
    let state = fixture.host.start(&[fixture.shared.id().into()], 0).await.unwrap();
    assert!(!fixture.host.is_ssh_tunnel() && fixture.host.issue_ssh_code().is_none());
    assert!(!state.allows_full_access);
    assert_eq!(pair(state.port.unwrap(), &"c".repeat(43)).await.unwrap_err().status(), Some(401), "only the key opens it");
    assert!(RemoteHost::new_ssh_tunnel(fixture_options_unused()).issue_ssh_code().is_some());
    fixture.dispose().await;
}

/// A tunnel host's options are never read for its code: any host made with the tunnel constructor issues one.
fn fixture_options_unused() -> mighty_remote::remote::host::HostOptions {
    let network = mighty_platform::fake::FakeNetwork::default();
    mighty_remote::remote::host::HostOptions {
        workspaces: Arc::new(Vec::new),
        resolve_local: Arc::new(|_| Box::pin(async { Err("none".to_string()) })),
        runtime: Arc::new(|| Box::pin(async { runtime("info.json") })),
        create_runner: Arc::new(|sinks| FakeRunner::new(sinks)),
        host_name: "mac".into(),
        admission: Arc::new(|_| None),
        runs_changed: Arc::new(|_| {}),
        allow_loopback_for_tests: false,
        heartbeat: protocol::HEARTBEAT_TIMEOUT,
        addresses: None,
        clock: mighty_remote::remote::host::monotonic(),
        windows_host: false,
        decodes_paths: false,
        graph_runs: false,
        antigravity_rules: None,
        system_folder: None,
        network: Arc::new(network),
    }
}

/// The app's hook as the check plays it: the tunnel's port and a fresh code, counted; or the words of a failure.
fn hook(host: Arc<Mutex<RemoteHost>>, port: Arc<AtomicU64>, opened: Arc<AtomicUsize>, failing: Arc<Mutex<Option<String>>>) -> SshTunnel {
    Arc::new(move |machine: String| {
        let (host, port, opened, failing) = (host.clone(), port.clone(), opened.clone(), failing.clone());
        Box::pin(async move {
            assert_eq!(machine, MACHINE);
            opened.fetch_add(1, Ordering::SeqCst);
            if let Some(words) = failing.lock().unwrap().clone() {
                return Err(words);
            }
            let code = host.lock().unwrap().issue_ssh_code().ok_or("no code")?;
            Ok(SshOpening { port: port.load(Ordering::SeqCst) as u16, code: Zeroizing::new(code) })
        })
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn the_client_connects_through_the_tunnel_and_pairs_again_on_every_reconnect() {
    let fixture = make_host(MakeHost { ssh_tunnel: true, ..MakeHost::default() }).await;
    let state = fixture.host.start(&[], 0).await.unwrap();
    let port = Arc::new(AtomicU64::new(u64::from(state.port.unwrap())));
    let opened = Arc::new(AtomicUsize::new(0));
    let failing: Arc<Mutex<Option<String>>> = Arc::default();
    let shared = Arc::new(Mutex::new(fixture.host.clone()));
    // Neither loopback injected nor any Tailscale: the SSH connection needs neither.
    let options =
        ClientOptions { ssh_tunnel: Some(hook(shared.clone(), port.clone(), opened.clone(), failing.clone())), ..ClientOptions::default() };
    let (store, folder) = client_store();
    let seen = Arc::new(Collected::default());
    let vault = Arc::new(MemoryVault::default());
    let client = client_with(&store, &folder, vault.clone(), &seen, options);

    let info = client.connect_ssh("studio", MACHINE).await.unwrap();
    assert_eq!(info.status, RemoteConnectionStatus::Connected);
    assert_eq!(info.address, ssh_address(MACHINE));
    assert_eq!(info.name, "studio");
    assert_eq!(info.workspaces.as_ref().map(Vec::len), Some(2), "both local workspaces of the far side");
    assert!(client.is_ssh(&info.id));
    assert!(vault.load(&info.id).is_none(), "the key is never kept in the vault");
    let saved = std::fs::read_to_string(folder.join("remote-connections.json")).unwrap();
    assert!(saved.contains(&ssh_address(MACHINE)) && !saved.contains(state.key.as_deref().unwrap()));
    // No route to ask: a send's check passes as it is.
    assert_eq!(client.ensure_connected(&info.id).await.unwrap().status, RemoteConnectionStatus::Connected);
    assert_eq!(opened.load(Ordering::SeqCst), 1);
    // A pane opens on the far side's workspace as any remote one.
    let workspace = client.open_workspace(&info.id, fixture.private.id()).unwrap();
    assert!(workspace.remote.is_some());

    // The far side's app restarted: a new key, on another port. A reconnect opens the way again and pairs again.
    fixture.host.stop().await;
    let state = fixture.host.start(&[], 0).await.unwrap();
    port.store(u64::from(state.port.unwrap()), Ordering::SeqCst);
    let again = client.refresh(&info.id).await.unwrap();
    assert_eq!(again.status, RemoteConnectionStatus::Connected);
    assert_eq!(again.id, info.id, "the same connection");
    assert_eq!(opened.load(Ordering::SeqCst), 2);
    // Connecting to the same machine again keeps its id.
    assert_eq!(client.connect_ssh("", MACHINE).await.unwrap().id, info.id);

    // The way cannot be opened (ssh down): the connection says why and is disconnected.
    *failing.lock().unwrap() = Some("SSH 연결이 끊겼습니다.".into());
    assert!(client.refresh(&info.id).await.is_err());
    let now = client.connection(&info.id).unwrap();
    assert_eq!(now.status, RemoteConnectionStatus::Disconnected);
    assert_eq!(now.detail.as_deref(), Some("SSH 연결이 끊겼습니다."));

    // A client the app gave no way there refuses at once; a machine id that is not one too.
    let (other_store, other_folder) = client_store();
    let bare = client_with(&other_store, &other_folder, Arc::new(MemoryVault::default()), &seen, ClientOptions::default());
    assert!(bare.connect_ssh("", MACHINE).await.is_err());
    assert!(client.connect_ssh("", "../etc").await.is_err());

    client.dispose().await;
    bare.dispose().await;
    fixture.dispose().await;
    let _ = std::fs::remove_dir_all(&folder);
    let _ = std::fs::remove_dir_all(&other_folder);
}

// ── MC-40: the server behind a live forward restarted ────────────────────────────────────────────────────────────────────────

/// The app's `ssh -L` as the check plays it: a loopback listener whose every connection goes on to the far side's `remote` port,
/// and ends at once when that port refuses (as ssh ends a forwarded connection the far side refused). Dropped: no longer listening.
struct Forward {
    remote: u16,
    local: u16,
    task: tokio::task::JoinHandle<()>,
}

impl Forward {
    async fn open(remote: u16) -> Forward {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local = listener.local_addr().unwrap().port();
        let task = tokio::spawn(async move {
            while let Ok((mut inbound, _)) = listener.accept().await {
                tokio::spawn(async move {
                    if let Ok(mut outbound) = tokio::net::TcpStream::connect(("127.0.0.1", remote)).await {
                        let _ = tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await;
                    }
                });
            }
        });
        Forward { remote, local, task }
    }
}

impl Drop for Forward {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// The far side's panes: none (the mirror's list is its shared workspaces alone), every action refused.
struct NoPanes;

impl MirrorHost for NoPanes {
    fn summaries(&self, _: HashSet<String>) -> BoxFuture<'_, Vec<MirrorSummary>> {
        Box::pin(async { vec![] })
    }
    fn detail(&self, _: String, _: HashSet<String>) -> BoxFuture<'_, Option<MirrorPane>> {
        Box::pin(async { None })
    }
    fn pane_info(&self, _: String) -> BoxFuture<'_, Option<MirrorPaneInfo>> {
        Box::pin(async { None })
    }
    fn submit(&self, _: String, _: String, _: Vec<RunAttachment>) -> BoxFuture<'_, Result<String, Refusal>> {
        Box::pin(async { Err("no".into()) })
    }
    fn stop(&self, _: String) -> BoxFuture<'_, Result<(), Refusal>> {
        Box::pin(async { Err("no".into()) })
    }
    fn permission(&self, _: String, _: String, _: String, _: bool, _: bool) -> BoxFuture<'_, Result<(), Refusal>> {
        Box::pin(async { Err("no".into()) })
    }
    fn answers(&self, _: String, _: String, _: String, _: IndexMap<String, UserQuestionAnswer>) -> BoxFuture<'_, Result<(), Refusal>> {
        Box::pin(async { Err("no".into()) })
    }
    fn plan(&self, _: String, _: String, _: String, _: PlanDecision) -> BoxFuture<'_, Result<(), Refusal>> {
        Box::pin(async { Err("no".into()) })
    }
    fn create_session(&self, _: String, _: String) -> BoxFuture<'_, Result<String, Refusal>> {
        Box::pin(async { Err("no".into()) })
    }
    fn close(&self, _: String) -> BoxFuture<'_, Result<(), Refusal>> {
        Box::pin(async { Err("no".into()) })
    }
}

/// This computer's state as the mirror lays the far side's over it.
#[derive(Default)]
struct Kept(Mutex<AppSnapshot>);

impl MirrorStage for Kept {
    fn snapshot(&self) -> AppSnapshot {
        self.0.lock().unwrap().clone()
    }
    fn change(&self, change: &mut dyn FnMut(&AppSnapshot) -> Option<AppSnapshot>) -> bool {
        let mut kept = self.0.lock().unwrap();
        match change(&kept) {
            Some(next) => {
                *kept = next;
                true
            }
            None => false,
        }
    }
    fn permission(&self, _: ToolPermissionRequest) {}
    fn changed(&self) {}
    fn failed(&self, _: &str, _: &str) {}
}

/// What the check's stand-in for the app saw and does.
#[derive(Default)]
struct Side {
    /// The forward open now.
    forward: tokio::sync::Mutex<Option<Forward>>,
    /// Every opening asked for (each a pairing over "ssh").
    opened: AtomicUsize,
    /// Every code issued, to look for where it must never be.
    codes: Mutex<Vec<String>>,
    /// The ssh is down: the way cannot be opened.
    failing: Mutex<Option<String>>,
    /// The machines told lost, in order.
    lost: Mutex<Vec<String>>,
    /// The desk the app's reconnect drives (its 연결).
    desk: std::sync::OnceLock<MirrorDesk>,
}

impl Side {
    fn lost(&self) -> usize {
        self.lost.lock().unwrap().len()
    }
}

/// The app's `ssh_tunnel` as the check plays it, as `open_tunnel` does: `mighty server ssh-pair` (the far side's current port and a
/// fresh code), the forward kept when it goes to that port already, else the old one closed and a new one opened.
fn moving_hook(host: RemoteHost, side: Arc<Side>) -> SshTunnel {
    Arc::new(move |machine: String| {
        let (host, side) = (host.clone(), side.clone());
        Box::pin(async move {
            assert_eq!(machine, MACHINE);
            side.opened.fetch_add(1, Ordering::SeqCst);
            if let Some(words) = side.failing.lock().unwrap().clone() {
                return Err(words);
            }
            let port = host.state().port.ok_or("Mighty Code is not running")?;
            let code = host.issue_ssh_code().ok_or("no code")?;
            side.codes.lock().unwrap().push(code.clone());
            let mut held = side.forward.lock().await;
            if held.as_ref().is_none_or(|f| f.remote != port) {
                *held = None;
                *held = Some(Forward::open(port).await);
            }
            Ok(SshOpening::new(held.as_ref().unwrap().local, code))
        })
    })
}

/// The app's `ssh_lost` as the check plays it: one round of the app's reconnect a moment later (its gaps are the app's own check).
fn lost_hook(side: Arc<Side>) -> SshLost {
    Arc::new(move |machine: String| {
        side.lost.lock().unwrap().push(machine.clone());
        let side = side.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let Some(desk) = side.desk.get() else { return };
            let address = ssh_address(&machine);
            if let Some(connection) = desk.client().connections().into_iter().find(|c| c.address == address) {
                let _ = desk.connect(&connection.id).await;
            }
        });
    })
}

/// The far side's app started again, on a port other than `old` (the system picks one).
async fn restart_elsewhere(host: &RemoteHost, old: u16) -> u16 {
    loop {
        host.stop().await;
        let port = host.start(&[], 0).await.unwrap().port.unwrap();
        if port != old {
            return port;
        }
    }
}

/// MC-40: the far side's app restarts while ssh stays up (its forward still listens here, to the old port): the mirror's requests
/// fail through the forward, the connection is told lost, and the app's reconnect pairs again over ssh, moves the forward to the new
/// port and connects — nobody clicks. The same for a restart on the same port (only the key changed: a 401). A person's 연결 끊기
/// or removal, and a reconnect that cannot open the way, are never told lost; the codes are never kept or shown.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_server_restarted_behind_a_live_forward_is_paired_again_without_a_click() {
    let fixture = make_host(MakeHost { ssh_tunnel: true, ..MakeHost::default() }).await;
    fixture.host.set_mirror(Some(Arc::new(NoPanes)));
    let first = fixture.host.start(&[], 0).await.unwrap().port.unwrap();
    let side = Arc::new(Side::default());
    let options = ClientOptions {
        ssh_tunnel: Some(moving_hook(fixture.host.clone(), side.clone())),
        ssh_lost: Some(lost_hook(side.clone())),
        poll_retry_window: Duration::from_millis(300),
        ..ClientOptions::default()
    };
    let (store, folder) = client_store();
    let seen = Arc::new(Collected::default());
    let client = client_with(&store, &folder, Arc::new(MemoryVault::default()), &seen, options);
    let stage = Arc::new(Kept::default());
    let spawn: Spawn = Arc::new(|future| {
        tokio::spawn(future);
    });
    let desk = MirrorDesk::new(client.clone(), stage.clone(), spawn);
    desk.set_intervals(Duration::from_millis(50), Duration::from_millis(100));
    desk.attach();
    assert!(side.desk.set(desk.clone()).is_ok());

    let info = client.connect_ssh("studio", MACHINE).await.unwrap();
    let id = info.id.clone();
    let live = |desk: &MirrorDesk| desk.link(&id).is_some_and(|l| l.phase == "live" && l.error.is_none());
    let connected = || client.connection(&id).is_some_and(|c| c.status == RemoteConnectionStatus::Connected);
    until(|| live(&desk) && !stage.snapshot().workspaces().is_empty(), 10_000, "the far side's workspaces are mirrored").await;
    assert_eq!(side.opened.load(Ordering::SeqCst), 1);
    let forwarded = |port: u16| side.forward.try_lock().is_ok_and(|f| f.as_ref().is_some_and(|f| f.remote == port));
    assert!(forwarded(first));

    // Restarted on another port; the forward still listens and goes to the old one.
    let second = restart_elsewhere(&fixture.host, first).await;
    until(|| side.lost() == 1, 10_000, "the connection is told lost").await;
    until(|| connected() && live(&desk) && forwarded(second), 10_000, "paired again on the new port, the mirror live again").await;
    assert_eq!(*side.lost.lock().unwrap(), [MACHINE], "told once, with its machine");
    assert_eq!(side.opened.load(Ordering::SeqCst), 2, "one new pairing over ssh");

    // Restarted on the same port: the forward is kept, only the key is new (a 401 with the old one).
    let local = side.forward.lock().await.as_ref().unwrap().local;
    fixture.host.stop().await;
    assert_eq!(fixture.host.start(&[], second).await.unwrap().port, Some(second));
    until(|| side.lost() == 2, 10_000, "a key the server no longer knows is lost too").await;
    until(|| connected() && live(&desk), 10_000, "paired again through the same forward").await;
    assert_eq!(side.opened.load(Ordering::SeqCst), 3);
    assert_eq!(side.forward.lock().await.as_ref().map(|f| (f.remote, f.local)), Some((second, local)), "the same forward");

    // 연결 끊기: never lost, nothing paired again.
    client.disconnect(&id).await;
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert_eq!((side.lost(), side.opened.load(Ordering::SeqCst)), (2, 3));
    assert_eq!(client.connection(&id).unwrap().status, RemoteConnectionStatus::Disconnected);

    // The person's 연결 again; then the server goes while the way there cannot be opened: told lost once, and the failed reconnect
    // is not told again (no loop of its own).
    desk.connect(&id).await.unwrap();
    until(|| live(&desk), 10_000, "connected by the person").await;
    assert_eq!(side.opened.load(Ordering::SeqCst), 4);
    *side.failing.lock().unwrap() = Some("SSH 연결이 끊겼습니다.".into());
    restart_elsewhere(&fixture.host, second).await;
    until(|| side.lost() == 3, 10_000, "lost").await;
    until(|| side.opened.load(Ordering::SeqCst) == 5, 10_000, "the reconnect's one try").await;
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert_eq!((side.lost(), side.opened.load(Ordering::SeqCst)), (3, 5), "a failed reconnect is not a new loss");
    assert_eq!(client.connection(&id).unwrap().detail.as_deref(), Some("SSH 연결이 끊겼습니다."));

    // No code in the connection's words, its saved line or anything shown of it.
    let codes = side.codes.lock().unwrap().clone();
    assert_eq!(codes.len(), 4);
    let saved = std::fs::read_to_string(folder.join("remote-connections.json")).unwrap();
    let shown = format!("{:?} {:?}", client.connections(), desk.link(&id));
    for code in &codes {
        assert!(!saved.contains(code.as_str()) && !shown.contains(code.as_str()));
    }

    // Removed: never lost.
    *side.failing.lock().unwrap() = None;
    client.remove(&id).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(side.lost(), 3);

    desk.dispose();
    client.dispose().await;
    fixture.dispose().await;
    let _ = std::fs::remove_dir_all(&folder);
}

// ── MC-40: shared memory over the tunnel ─────────────────────────────────────────────────────────────────────────────────────

/// MC-40: the SSH tunnel's host serves shared memory sync (`/v2/memory/*`, MC-13) behind its key like its other routes: with the
/// key (through the client's SSH connection) the far side's local workspaces' entries, never another computer's; without it, with
/// a wrong one or with a pairing code in its place, 401.
#[tokio::test(flavor = "multi_thread")]
async fn memory_sync_over_the_tunnel_needs_its_key() {
    let fixture = make_host(MakeHost { ssh_tunnel: true, ..MakeHost::default() }).await;
    let book = Arc::new(Mutex::new(SharedMemoryBook::new("studio-dev", "Studio")));
    {
        let mut book = book.lock().unwrap();
        for (workspace, key) in [(&fixture.private, "local-note"), (&fixture.remote, "far-note")] {
            book.upsert(UpsertMemoryInput {
                id: None,
                scope: MemoryScope::Project {
                    workspace_id: workspace.id().to_string(),
                    project_slug: infer_project_slug(workspace.path(), workspace.name()),
                },
                key: key.into(),
                title: key.into(),
                content: "Keep the pool small.".into(),
                tags: vec![],
                auto_inject: true,
                pinned: false,
            })
            .unwrap();
        }
    }
    let workspaces = fixture.store.clone();
    fixture.host.set_memory(Some(Arc::new(BookMemoryHost::new(
        book.clone(),
        "Studio",
        Arc::new(move || workspaces.lock().unwrap().snapshot.workspaces().to_vec()),
    ))));
    let state = fixture.host.start(&[], 0).await.unwrap();
    let port = state.port.unwrap();
    let key = state.key.clone().unwrap();
    let changes = format!("{CHANGES_PATH}?since=0");

    // Without the key, with a wrong one, with a code: refused before the memory is read; with the key, read.
    let code = fixture.host.issue_ssh_code().unwrap();
    let get = |authorization: String| {
        format!("GET {changes} HTTP/1.1\r\nHost: 127.0.0.1\r\n{authorization}{}: 2\r\nConnection: close\r\n\r\n", protocol::HEADER)
    };
    for (authorization, why) in [
        (String::new(), "no key"),
        (format!("Authorization: Bearer {}\r\n", "w".repeat(43)), "a wrong key"),
        (format!("Authorization: Bearer {code}\r\n"), "a pairing code is no key"),
    ] {
        assert!(raw_text(port, &get(authorization)).await.starts_with("HTTP/1.1 401"), "{why}");
    }
    let head = format!(
        "POST {PUSH_PATH} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: 2\r\n{}: 2\r\nConnection: close\r\n\r\n{{}}",
        protocol::HEADER
    );
    assert!(raw_text(port, &head).await.starts_with("HTTP/1.1 401"), "a push without the key");
    assert!(raw_text(port, &get(format!("Authorization: Bearer {key}\r\n"))).await.starts_with("HTTP/1.1 200"), "with the key");

    // Through the client's SSH connection, with the key the pairing gave.
    let shared = Arc::new(Mutex::new(fixture.host.clone()));
    let tunnel_port = Arc::new(AtomicU64::new(u64::from(port)));
    let options = ClientOptions { ssh_tunnel: Some(hook(shared, tunnel_port, Arc::default(), Arc::default())), ..ClientOptions::default() };
    let (store, folder) = client_store();
    let seen = Arc::new(Collected::default());
    let client = client_with(&store, &folder, Arc::new(MemoryVault::default()), &seen, options);
    let info = client.connect_ssh("studio", MACHINE).await.unwrap();
    let (status, body) = client.memory_request(&info.id, "GET", &changes, None).await.unwrap();
    assert_eq!(status, 200);
    let read: MemoryChangesWire = serde_json::from_slice(&body).unwrap();
    let keys: Vec<&str> = read.entries.iter().map(|e| e.key.as_str()).collect();
    assert_eq!(keys, ["local-note"], "the far side's own workspaces, never another computer's");
    let (status, _) = client.memory_request(&info.id, "POST", PUSH_PATH, Some(b"{}".to_vec())).await.unwrap();
    assert_ne!(status, 401, "the push passes the key check");
    assert_ne!(status, 404, "served on the tunnel");

    client.dispose().await;
    fixture.dispose().await;
    let _ = std::fs::remove_dir_all(&folder);
}
