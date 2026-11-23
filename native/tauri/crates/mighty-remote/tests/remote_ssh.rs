//! MC-33: the SSH tunnel's host (`RemoteHost::new_ssh_tunnel`) and the client's connection through it (`RemoteClient::connect_ssh`),
//! in one process over loopback. The "forward" is no forward at all: the tunnel listens on loopback already, and the app's hook here
//! hands the client its port and a code the host issued, as the app's ssh would. No ssh, no host, no Tailscale.

mod support;

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mighty_platform::SecretVault;
use mighty_platform::fake::MemoryVault;
use mighty_remote::remote::client::ClientOptions;
use mighty_remote::remote::host::{Clock, RemoteHost, tunnel_peer};
use mighty_remote::remote::ssh_tunnel::{SshOpening, SshTunnel, ssh_address};
use mighty_remote::remote::transport::{self, Method, Request};
use mighty_remote::remote::wire::{RemoteConnectionStatus, RemoteWireInfo, protocol};
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
