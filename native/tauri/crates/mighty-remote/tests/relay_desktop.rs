//! Computers paired through the relay ("페어링으로 연결 (Tailscale 없이)", docs/remote-workspaces.md; `relay::client`,
//! `remote::paired`): the pairing link read back, the identity binding a computer's token to its role, the Rust client's handshake and
//! cipher against the host's own `RelayHost` over an in-process pairing relay, the mirror's routes for a paired computer (the shared
//! workspaces only, the full-access switch and the Windows Codex rule as over Tailscale, no runs), a phone that gains nothing, the
//! attachment limit of one frame, and the pairing's secret kept in the vault and never in the connections file; the pairing the
//! security review asked for — the computers' own short-lived code (a phone's link pairs no computer, nor the other way round), the
//! person's approval (allowed, refused, not answered), no computer while "페어링한 컴퓨터의 연결 받기" is off, the computer's role kept
//! across a restart, and 새 키 만들기 renewing the host's keys and relay name. Loopback port 0 only; every folder is a temp folder; no
//! check reaches a real relay.

mod support;

use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use indexmap::IndexMap;
use mighty_core::attachment::RunAttachment;
use mighty_core::model::{LogEntry, MobileRemoteSettings, RunSession};
use mighty_platform::fake::MemoryVault;
use mighty_platform::{BoxFuture, Os, SecretVault, fake};
use mighty_remote::identity::{AuthDecision, AuthPolicy, AuthRefusal, AuthRequest, Identity, PairingKind, ROLES_FILE};
use mighty_remote::relay::cipher::{self, MAXIMUM_PLAINTEXT, RelayCipher};
use mighty_remote::relay::client::{ClientTimings, Credential, PairingLink, RelayClient, RelayFailure, RequestError, new_client_id};
use mighty_remote::relay::connection::{MobileDevice, MobileReply, RequestHandler};
use mighty_remote::relay::host::{ApprovalRequest, RelayHost, RelayOptions, dial};
use mighty_remote::remote::client::{ClientError, ClientOptions, RemoteClient};
use mighty_remote::remote::host::RemoteHost;
use mighty_remote::remote::mirror::client::refusal_status;
use mighty_remote::remote::mirror::host::{MirrorHost, RELAY_DETAIL_BYTES, Refusal};
use mighty_remote::remote::mirror::{MirrorPane, MirrorPaneInfo, MirrorSummary, support as mirror};
use mighty_remote::remote::paired::{self, DesktopRouter, DirectProxy, PairedSecret};
use mighty_remote::remote::wire::RemoteConnectionStatus;
use mighty_run::plan_mode::PlanDecision;
use mighty_run::questionnaire::UserQuestionAnswer;
use mighty_run::strings::{get, get_with};
use serde_json::{Value, json};
use support::fake_host::{MakeHost, make_host, temp, until};
use support::fake_relay::FileStore;
use support::pairing_relay::PairingRelay;
use tokio_tungstenite::tungstenite::Message;

/// The phone's router as these checks see it: counts what reaches it, answers every route 200 `{"phone":true}`.
#[derive(Default)]
struct Phones(AtomicUsize);

impl RequestHandler for Phones {
    fn handle(
        &self,
        device: MobileDevice,
        _: String,
        _: String,
        _: Option<Box<serde_json::value::RawValue>>,
    ) -> BoxFuture<'static, MobileReply> {
        assert!(!device.desktop, "a computer never reaches the phone's router");
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { MobileReply::new(200, r#"{"phone":true}"#.into()) })
    }
}

/// The host's panes for the mirror: two in the shared workspace (the second's agent switchable), one in the private one; what was done.
struct Panes {
    shared: String,
    hidden: String,
    provider: Mutex<String>,
    mode: Mutex<String>,
    logs: usize,
    asked: AtomicUsize,
    actions: Mutex<Vec<String>>,
}

impl Panes {
    fn new(shared: &str, hidden: &str, logs: usize) -> Arc<Panes> {
        Arc::new(Panes {
            shared: shared.into(),
            hidden: hidden.into(),
            provider: Mutex::new("claude".into()),
            mode: Mutex::new("manual".into()),
            logs,
            asked: AtomicUsize::new(0),
            actions: Mutex::new(vec![]),
        })
    }

    fn summary(&self, id: &str, workspace: &str) -> MirrorSummary {
        MirrorSummary {
            id: id.into(),
            workspace_id: workspace.into(),
            title: format!("Pane {id}"),
            kind: "claude".into(),
            provider: if id == "pane-2" { self.provider.lock().unwrap().clone() } else { "claude".into() },
            model: "default".into(),
            status: "idle".into(),
            revision: format!("rev-{id}"),
            pending: 0,
            queued: 0,
            branch: None,
        }
    }

    fn pane(&self, id: &str) -> RunSession {
        // Entries of 40 KB each: 80 of them are over 3 MB before the details are fitted.
        let text = "x".repeat(40 * 1024);
        RunSession {
            id: Some(id.into()),
            workspace_id: Some(self.shared.clone()),
            title: Some("Host pane".into()),
            status: Some("idle".into()),
            provider: Some("claude".into()),
            logs: Some(
                (0..self.logs)
                    .map(|i| LogEntry::new(format!("log-{i}"), "assistant", format!("{i} {text}"), "2026-11-13T01:00:00Z".into(), None))
                    .collect(),
            ),
            ..RunSession::default()
        }
    }

    fn did(&self, what: String) -> Result<(), Refusal> {
        self.actions.lock().unwrap().push(what);
        Ok(())
    }

    fn actions(&self) -> Vec<String> {
        self.actions.lock().unwrap().clone()
    }
}

impl MirrorHost for Panes {
    fn summaries(&self, workspace_ids: HashSet<String>) -> BoxFuture<'_, Vec<MirrorSummary>> {
        self.asked.fetch_add(1, Ordering::SeqCst);
        let all = [self.summary("pane-1", &self.shared), self.summary("pane-2", &self.shared), self.summary("pane-hidden", &self.hidden)];
        Box::pin(async move { all.into_iter().filter(|s| workspace_ids.contains(&s.workspace_id)).collect() })
    }
    fn detail(&self, session_id: String, _: HashSet<String>) -> BoxFuture<'_, Option<MirrorPane>> {
        self.asked.fetch_add(1, Ordering::SeqCst);
        let pane = (session_id != "pane-hidden").then(|| MirrorPane {
            revision: format!("rev-{session_id}"),
            session: mirror::session(&self.pane(&session_id), &[], &[], None, None),
        });
        Box::pin(async move { pane })
    }
    fn pane_info(&self, session_id: String) -> BoxFuture<'_, Option<MirrorPaneInfo>> {
        let info = |workspace: &str, mode: String, provider: String| MirrorPaneInfo {
            workspace_id: workspace.into(),
            permission_mode: mode,
            provider,
        };
        let found = match session_id.as_str() {
            "pane-1" => Some(info(&self.shared, self.mode.lock().unwrap().clone(), "claude".into())),
            "pane-2" => Some(info(&self.shared, "manual".into(), self.provider.lock().unwrap().clone())),
            "pane-hidden" => Some(info(&self.hidden, "manual".into(), "claude".into())),
            _ => None,
        };
        Box::pin(async move { found })
    }
    fn submit(&self, session_id: String, text: String, attachments: Vec<RunAttachment>) -> BoxFuture<'_, Result<String, Refusal>> {
        let done = self.did(format!("submit {session_id} {text} {}", attachments.len())).map(|()| "queued".to_string());
        Box::pin(async move { done })
    }
    fn stop(&self, session_id: String) -> BoxFuture<'_, Result<(), Refusal>> {
        let done = self.did(format!("stop {session_id}"));
        Box::pin(async move { done })
    }
    fn permission(
        &self,
        session_id: String,
        request_id: String,
        _: String,
        allow: bool,
        always: bool,
    ) -> BoxFuture<'_, Result<(), Refusal>> {
        let done = self.did(format!("permission {session_id} {request_id} {allow} {always}"));
        Box::pin(async move { done })
    }
    fn answers(
        &self,
        session_id: String,
        _: String,
        _: String,
        _: IndexMap<String, UserQuestionAnswer>,
    ) -> BoxFuture<'_, Result<(), Refusal>> {
        let done = self.did(format!("answers {session_id}"));
        Box::pin(async move { done })
    }
    fn plan(&self, session_id: String, _: String, _: String, _: PlanDecision) -> BoxFuture<'_, Result<(), Refusal>> {
        let done = self.did(format!("plan {session_id}"));
        Box::pin(async move { done })
    }
    fn create_session(&self, workspace_id: String, provider: String) -> BoxFuture<'_, Result<String, Refusal>> {
        let done = self.did(format!("create {workspace_id} {provider}")).map(|()| "pane-3".to_string());
        Box::pin(async move { done })
    }
    fn close(&self, session_id: String) -> BoxFuture<'_, Result<(), Refusal>> {
        let done = self.did(format!("close {session_id}"));
        Box::pin(async move { done })
    }
}

fn identity_in(folder: &Path) -> Arc<Identity> {
    Arc::new(Identity::load_or_create(Arc::new(FileStore(folder.to_path_buf())), true).unwrap())
}

/// The phone remote's settings with this relay; `phones` its switch.
fn settings(relay: &str, phones: bool) -> MobileRemoteSettings {
    MobileRemoteSettings { enabled: phones, relay_url: Some(relay.into()), direct_enabled: false, ..MobileRemoteSettings::default() }
}

/// A relay host over `identity` whose handler is the app's: computers to `sharing`, phones to `phones`; the person allows every new
/// device at once.
fn relay_host(folder: &Path, identity: Arc<Identity>, phones: Arc<Phones>, sharing: Option<RemoteHost>) -> RelayHost {
    let options = RelayOptions {
        approver: Some(support::fake_relay::allow_all(Arc::default())),
        host_name: "studio-pc".into(),
        watches_network: false,
        ..RelayOptions::default()
    };
    relay_host_with(folder, identity, phones, sharing, options)
}

/// [`relay_host`] with its options given (the person's answers among them).
fn relay_host_with(
    folder: &Path,
    identity: Arc<Identity>,
    phones: Arc<Phones>,
    sharing: Option<RemoteHost>,
    options: RelayOptions,
) -> RelayHost {
    let router = DesktopRouter::new(phones, Arc::new(move || sharing.clone()));
    RelayHost::with_options(identity, MobileRemoteSettings::default(), Arc::new(router), fake::platform(folder, Os::Windows), options)
}

const ALL: AuthPolicy = AuthPolicy { allow_legacy: true, allow_desktops: true };

/// An auth decided under `policy`, a registration allowed by the person at once.
fn allowed(identity: &Identity, auth: &Value, policy: AuthPolicy) -> AuthDecision {
    match identity.authorize(&AuthRequest::from_json(auth), policy).unwrap() {
        AuthDecision::Pending(pending) => identity.register(&pending).unwrap(),
        decided => decided,
    }
}

fn quick() -> ClientTimings {
    ClientTimings { backoff_min: Duration::from_millis(100), backoff_max: Duration::from_millis(400), ..ClientTimings::default() }
}

fn client_in(folder: &Path, vault: Arc<dyn SecretVault>) -> RemoteClient {
    let store = Arc::new(Mutex::new(mighty_core::store::Store::new(folder.to_path_buf(), None, true)));
    store.lock().unwrap().load(&mighty_core::locale::Locale::new("en")).unwrap();
    let options = ClientOptions {
        allow_loopback_for_tests: true,
        relay_proxy: Some(Arc::new(DirectProxy)),
        client_name: "laptop".into(),
        relay_timings: quick(),
        ..ClientOptions::default()
    };
    RemoteClient::new(support::fake_host::approver(&store), folder.to_path_buf(), vault, options)
}

/// `size` bytes that read as a PNG (its signature, then zeros).
fn png(size: usize) -> Vec<u8> {
    let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
    bytes.resize(size, 0);
    bytes
}

/// The text a pairing link carries under `name`, decoded.
fn member(link: &str, name: &str) -> String {
    url::Url::parse(link).unwrap().query_pairs().find(|(k, _)| k == name).map(|(_, v)| v.into_owned()).unwrap()
}

/// A phone's handshake and auth on the relay by hand (the phone app's transport): its socket and cipher once `auth` went out, the
/// relay's close code when the host closed it before answering.
type PhoneSocket = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn phone(relay: &str, link: &str, auth: Value) -> Result<(Value, RelayCipher, PhoneSocket), Option<u16>> {
    let url = format!("{relay}/ws?serverId={}&role=client&connectionId={}&v=1", member(link, "sid"), uuid::Uuid::new_v4());
    let mut socket = dial(&url, &DirectProxy).await.map_err(|_| None)?;
    let b64 = base64::engine::general_purpose::STANDARD;
    let secret = [7u8; 32];
    let nonce = [9u8; 16];
    let hello = json!({"type":"hello","v":1,"clientKey": b64.encode(cipher::public_key(&secret)),"nonce": b64.encode(nonce)});
    socket.send(Message::text(hello.to_string())).await.unwrap();
    // A host that refuses closes before `ready` (its close code is the answer); a socket that ends otherwise is tried again.
    let ready = match socket.next().await {
        Some(Ok(Message::Text(ready))) => ready,
        Some(Ok(Message::Close(frame))) => return Err(frame.map(|f| u16::from(f.code))),
        _ => return Err(None),
    };
    let ready: Value = serde_json::from_str(ready.as_str()).unwrap();
    // Anything but `ready` (the host's link still settling after a settings change): no handshake this time.
    let Some(server_key) = ready["serverKey"].as_str() else { return Err(None) };
    let server_key = b64.decode(server_key).unwrap();
    let cipher = RelayCipher::new(&secret, &server_key, &nonce, &b64.decode(ready["nonce"].as_str().unwrap()).unwrap(), false).unwrap();
    socket.send(Message::binary(cipher.seal(auth.to_string().as_bytes()).unwrap())).await.unwrap();
    loop {
        match socket.next().await {
            Some(Ok(Message::Binary(frame))) => {
                let answer: Value = serde_json::from_slice(&cipher.open(&frame).unwrap()).unwrap();
                // A registration is told to wait for the person first: the answer comes after.
                if answer["type"] == "auth_pending" {
                    continue;
                }
                return Ok((answer, cipher, socket));
            }
            Some(Ok(Message::Close(frame))) => return Err(frame.map(|f| u16::from(f.code))),
            Some(Ok(_)) => continue,
            _ => return Err(None),
        }
    }
}

/// The pairing link names the relay, the host's key and relay name and the pairing key; it reads back exactly, and anything else is
/// refused without echoing it.
#[test]
fn a_pairing_link_reads_back_and_anything_else_is_refused() {
    let folder = temp("pair-link");
    let identity = identity_in(&folder);
    let link = identity.desktop_pairing_url("wss://relay.example:8443", "studio-pc");
    let read = PairingLink::parse(&format!("  \"{link}\"\n")).unwrap();
    assert_eq!(read.endpoint.server_id, identity.server_id());
    assert_eq!(base64::engine::general_purpose::STANDARD.encode(read.endpoint.public_key), identity.public_key());
    assert_eq!(read.endpoint.relay, "wss://relay.example:8443");
    assert_eq!(read.pairing_key.as_str(), identity.pairing_code(PairingKind::Desktop).as_str(), "the computers' code");
    assert!(read.for_desktop, "said to be a computer's");
    assert_ne!(read.pairing_key.as_str(), identity.pairing_key().as_str(), "never the saved key");
    assert_eq!(read.name, "studio-pc");
    assert!(!format!("{read:?}").contains(read.pairing_key.as_str()), "the key never reaches Debug");
    let phone_link = identity.pairing_url("wss://relay.example:8443", "studio-pc", &["100.101.102.103:43139".into()]);
    let phone = PairingLink::parse(&phone_link).unwrap();
    assert!(!phone.for_desktop && phone.pairing_key != read.pairing_key, "the phone's link has a code of its own");
    // A plain ws:// relay is taken only on this computer itself.
    assert!(PairingLink::parse(&link.replace("wss%3A%2F%2Frelay.example%3A8443", "ws%3A%2F%2Frelay.example%3A8443")).is_err());
    assert!(PairingLink::parse(&link.replace("wss%3A%2F%2Frelay.example%3A8443", "ws%3A%2F%2F127.0.0.1%3A8443")).is_ok());
    for bad in [
        "",
        "https://example.com",
        "mightycode://pair?v=1&sid=a&pk=b&relay=wss://x&key=k",
        &link.replace("&key=", "&nokey="),
        &link.replace("relay=", "relay=ftp%3A%2F%2F"),
        &link.replace("pk=", "pk=AAAA"),
    ] {
        assert!(PairingLink::parse(bad).is_err(), "refused: {bad:?}");
    }
    let _ = std::fs::remove_dir_all(folder);
}

/// A computer pairs with the computers' own code and its token is bound to its role: it never opens a phone session (nor a phone's
/// token a computer's), a computer always names itself, a phone's code pairs no computer and the computers' code no phone, no computer
/// registers while computers are not taken, the identity file is the one older apps read, and the role is kept beside it so the
/// computer is still one after a restart.
#[test]
fn a_computers_token_is_bound_to_its_role() {
    let folder = temp("pair-identity");
    let roles = folder.join(ROLES_FILE);
    let identity = Identity::load_or_create(Arc::new(FileStore(folder.clone())), true).unwrap().with_roles_file(roles.clone());
    let phone_code = identity.pairing_code(PairingKind::Phone).to_string();
    let desktop_code = identity.pairing_code(PairingKind::Desktop).to_string();
    let desktop = new_client_id();
    let as_computer = |code: &str| json!({"type":"auth","pairingKey":code,"clientId":desktop,"clientName":"laptop","role":"desktop"});
    assert_eq!(
        allowed(&identity, &as_computer(&phone_code), ALL),
        AuthDecision::Refused(AuthRefusal::PairingKey),
        "a phone's link pairs no computer"
    );
    assert_eq!(
        allowed(&identity, &as_computer(&desktop_code), AuthPolicy { allow_legacy: true, allow_desktops: false }),
        AuthDecision::Refused(AuthRefusal::DesktopsOff),
        "no computer while computers are not taken"
    );
    let paired = allowed(&identity, &as_computer(&desktop_code), ALL);
    let AuthDecision::Accepted { device_token: Some(token), .. } = paired else { panic!("a computer pairs: {paired:?}") };
    let as_desktop = json!({"type":"auth","deviceToken":token,"clientId":desktop,"role":"desktop"});
    let as_phone = json!({"type":"auth","deviceToken":token,"clientId":desktop});
    let off = AuthPolicy::default();
    assert!(
        matches!(identity.authorize(&AuthRequest::from_json(&as_desktop), off).unwrap(), AuthDecision::Accepted { .. }),
        "a token needs no switch"
    );
    assert_eq!(identity.authorize(&AuthRequest::from_json(&as_phone), off).unwrap(), AuthDecision::Refused(AuthRefusal::DeviceRevoked));
    let phone = new_client_id();
    let desktop_code = identity.pairing_code(PairingKind::Desktop).to_string();
    assert_eq!(
        allowed(&identity, &json!({"type":"auth","pairingKey":desktop_code,"clientId":phone}), ALL),
        AuthDecision::Refused(AuthRefusal::PairingKey),
        "the computers' link pairs no phone"
    );
    let AuthDecision::Accepted { device_token: Some(phone_token), .. } =
        allowed(&identity, &json!({"type":"auth","pairingKey":phone_code,"clientId":phone}), off)
    else {
        panic!("a phone pairs")
    };
    let claimed = json!({"type":"auth","deviceToken":phone_token,"clientId":phone,"role":"desktop"});
    assert_eq!(
        identity.authorize(&AuthRequest::from_json(&claimed), off).unwrap(),
        AuthDecision::Refused(AuthRefusal::DeviceRevoked),
        "a phone's token is no computer's"
    );
    let nameless = json!({"type":"auth","pairingKey":desktop_code,"role":"desktop"});
    assert_eq!(identity.authorize(&AuthRequest::from_json(&nameless), ALL).unwrap(), AuthDecision::Refused(AuthRefusal::Malformed));
    // The file is still the shared codec's (no member of its own): it reads back with both devices, and the role beside it.
    let reloaded = Identity::load_or_create(Arc::new(FileStore(folder.clone())), true).unwrap().with_roles_file(roles.clone());
    let devices = reloaded.devices();
    assert_eq!(devices.len(), 2);
    assert!(devices.iter().any(|d| d.id == desktop && d.desktop), "the computer is one after a restart: {devices:?}");
    assert!(devices.iter().any(|d| d.id == phone && !d.desktop));
    let file = std::fs::read_to_string(FileStore(folder.clone()).file()).unwrap();
    assert!(!file.contains("desktop") && !file.contains(token.as_str()), "neither the role nor the token is written there");
    let kept = std::fs::read_to_string(&roles).unwrap();
    assert!(kept.contains(&desktop) && !kept.contains(&phone) && !kept.contains(token.as_str()), "{kept}");
    // Without the roles file the identity reads as before (a computer as a phone until it connects).
    assert!(identity_in(&folder).devices().iter().all(|d| !d.desktop));
    // Unpaired, the computer leaves the file too.
    reloaded.revoke(&desktop).unwrap();
    assert!(!std::fs::read_to_string(&roles).unwrap().contains(&desktop));
    let _ = std::fs::remove_dir_all(folder);
}

/// A pairing code lives while it is shown and runs out after: each draw keeps it alive for its lifetime from then (here 300 ms), a
/// code not drawn for longer pairs nothing, the next draw makes a new one, and the key-only legacy auth is refused while legacy apps
/// are off (the default).
#[test]
fn a_pairing_code_lives_while_shown_and_runs_out() {
    let folder = temp("pair-expiry");
    let identity =
        Identity::load_or_create(Arc::new(FileStore(folder.clone())), true).unwrap().with_code_lifetime(Duration::from_millis(300));
    let code = identity.pairing_code(PairingKind::Phone).to_string();
    for _ in 0..4 {
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(*identity.pairing_code(PairingKind::Phone), code, "shown again, kept alive");
    }
    assert!(identity.pairing_code_left(PairingKind::Phone).is_some_and(|left| left <= Duration::from_millis(300)));
    let auth = json!({"type":"auth","pairingKey":code,"clientId":new_client_id(),"clientName":"Phone"});
    assert!(matches!(identity.authorize(&AuthRequest::from_json(&auth), ALL).unwrap(), AuthDecision::Pending(_)), "valid while shown");
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(identity.pairing_code_left(PairingKind::Phone), None, "run out");
    assert_eq!(identity.authorize(&AuthRequest::from_json(&auth), ALL).unwrap(), AuthDecision::Refused(AuthRefusal::PairingKey));
    assert_ne!(*identity.pairing_code(PairingKind::Phone), code, "the next draw makes a new one");
    let legacy = json!({"type":"auth","pairingKey":*identity.pairing_key()});
    let off = AuthPolicy { allow_legacy: MobileRemoteSettings::default().allow_legacy_phones, allow_desktops: false };
    assert_eq!(identity.authorize(&AuthRequest::from_json(&legacy), off).unwrap(), AuthDecision::Refused(AuthRefusal::LegacyRefused));
    let _ = std::fs::remove_dir_all(folder);
}

/// The person at the host decides: a registration is asked about once (a computer's said to be one), refused it is `approval-denied`
/// for good and its code spent; not answered in time it is refused alike; a second device while one is being asked about is turned
/// away; a device that leaves while asked finds the answer on its next try; tokens are never asked about.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_person_at_the_host_allows_or_refuses_a_new_device() {
    let relay = PairingRelay::start().await;
    let folder = temp("pair-approval");
    let identity = identity_in(&folder);
    let phones = Arc::new(Phones::default());
    let asked: Arc<Mutex<Vec<ApprovalRequest>>> = Arc::default();
    // Refuses: the client is told `approval-denied`, for good, and the code it held pairs nothing any more.
    let options = |approver, timeout| RelayOptions {
        approver: Some(approver),
        approval_timeout: timeout,
        host_name: "studio-pc".into(),
        watches_network: false,
        ..RelayOptions::default()
    };
    let refusing = relay_host_with(
        &folder,
        identity.clone(),
        phones.clone(),
        None,
        options(support::fake_relay::answering(asked.clone(), false), Duration::from_secs(5)),
    );
    refusing.apply(settings(&relay.url, true)).await.unwrap();
    refusing.set_desktops(true).await.unwrap();
    until(|| relay.host_attached(), 5000, "the host's link").await;
    let link = PairingLink::parse(&refusing.desktop_pairing_url()).unwrap();
    let client = RelayClient::start(
        link.endpoint.clone(),
        Credential::PairingKey { client_id: new_client_id(), key: link.pairing_key.clone() },
        "laptop",
        Arc::new(DirectProxy),
        quick(),
        None,
    );
    let failure = client.ready(Duration::from_secs(10)).await.unwrap_err();
    assert!(matches!(&failure, RelayFailure::Refused { reason, last: true } if reason == "approval-denied"), "{failure:?}");
    assert_eq!(asked.lock().unwrap().clone(), [ApprovalRequest { name: "laptop".into(), desktop: true, replaces: false }]);
    assert!(identity.devices().is_empty(), "nothing registered");
    assert_ne!(PairingLink::parse(&refusing.desktop_pairing_url()).unwrap().pairing_key, link.pairing_key, "the refused code is spent");
    client.close();
    // A phone, refused alike (the phone app's words), and its code spent.
    let phone_link = refusing.pairing_url();
    let auth = json!({"type":"auth","pairingKey":member(&phone_link, "key"),"clientId":new_client_id(),"clientName":"Phone"});
    let (answer, _, _) = phone(&relay.url, &phone_link, auth).await.unwrap();
    assert_eq!((answer["type"].as_str(), answer["reason"].as_str()), (Some("auth_error"), Some("approval-denied")));
    assert_eq!(asked.lock().unwrap().last().map(|a| a.desktop), Some(false), "said to be a phone");
    refusing.dispose().await;

    // Never answered: refused when the host's time runs out (here 300 ms), the person's question dropped.
    let dropped = Arc::new(AtomicUsize::new(0));
    let silent: mighty_remote::relay::host::Approver = {
        let dropped = dropped.clone();
        Arc::new(move |_| {
            struct Counts(Arc<AtomicUsize>);
            impl Drop for Counts {
                fn drop(&mut self) {
                    self.0.fetch_add(1, Ordering::SeqCst);
                }
            }
            let counts = Counts(dropped.clone());
            Box::pin(async move {
                let _counts = counts;
                std::future::pending::<bool>().await
            })
        })
    };
    let silent_host = relay_host_with(&folder, identity.clone(), phones.clone(), None, options(silent, Duration::from_millis(300)));
    silent_host.apply(settings(&relay.url, true)).await.unwrap();
    until(|| relay.host_attached(), 5000, "the host's link").await;
    let phone_link = silent_host.pairing_url();
    let phone_id = new_client_id();
    let auth = json!({"type":"auth","pairingKey":member(&phone_link, "key"),"clientId":phone_id,"clientName":"Phone"});
    let (answer, _, _) = phone(&relay.url, &phone_link, auth).await.unwrap();
    assert_eq!(answer["reason"], "approval-denied", "not answered in time is refused");
    until(|| dropped.load(Ordering::SeqCst) == 1, 2000, "the question dropped (its dialog closes)").await;
    assert!(identity.devices().is_empty());
    silent_host.dispose().await;

    // Allowed: one device asked about at a time, and one that left while asked finds the answer on its next try.
    let gate = Arc::new(tokio::sync::Notify::new());
    let allowing: mighty_remote::relay::host::Approver = {
        let (gate, asked) = (gate.clone(), asked.clone());
        Arc::new(move |request| {
            asked.lock().unwrap().push(request);
            let gate = gate.clone();
            Box::pin(async move {
                gate.notified().await;
                true
            })
        })
    };
    asked.lock().unwrap().clear();
    let host = relay_host_with(&folder, identity.clone(), phones.clone(), None, options(allowing, Duration::from_secs(10)));
    host.apply(settings(&relay.url, true)).await.unwrap();
    until(|| relay.host_attached(), 5000, "the host's link").await;
    let phone_link = host.pairing_url();
    let key = member(&phone_link, "key");
    let first = json!({"type":"auth","pairingKey":key,"clientId":phone_id,"clientName":"Phone"});
    let waiting = tokio::spawn({
        let (url, link, first) = (relay.url.clone(), phone_link.clone(), first.clone());
        async move { phone(&url, &link, first).await }
    });
    until(|| asked.lock().unwrap().len() == 1, 5000, "the person asked").await;
    let other = json!({"type":"auth","pairingKey":key,"clientId":new_client_id(),"clientName":"Other"});
    let (busy, _, _) = phone(&relay.url, &phone_link, other).await.unwrap();
    assert_eq!(busy["reason"], "device-limit", "one device is asked about at a time");
    // The first phone gives up (an older app's 20 s); the question stays open.
    waiting.abort();
    let _ = waiting.await;
    until(|| host.connection_ids().is_empty(), 5000, "the host saw it go").await;
    gate.notify_one();
    let (answer, _, _) = phone(&relay.url, &phone_link, first).await.unwrap();
    assert_eq!(answer["type"], "auth_ok", "the answer kept for its next try: {answer}");
    assert!(answer["deviceToken"].as_str().is_some());
    assert_eq!(asked.lock().unwrap().len(), 1, "asked once");
    // A token never asks.
    let token = answer["deviceToken"].as_str().unwrap().to_string();
    let (again, _, _) = phone(&relay.url, &phone_link, json!({"type":"auth","deviceToken":token,"clientId":phone_id})).await.unwrap();
    assert_eq!(again["type"], "auth_ok");
    assert_eq!(asked.lock().unwrap().len(), 1, "a token is not asked about");
    host.dispose().await;
    let _ = std::fs::remove_dir_all(folder);
}

/// The Rust client against the host's own `RelayHost` through a pairing relay: the handshake and cipher, the token kept on the first
/// connection and used ever after, requests by id with the host's encrypted pings answered; a stale key, a wrong pairing key and an
/// absent host each said, the first two for good.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_client_handshakes_with_the_hosts_own_code() {
    let relay = PairingRelay::start().await;
    let folder = temp("pair-handshake");
    let identity = identity_in(&folder);
    let phones = Arc::new(Phones::default());
    // Nothing on the relay yet: the host is offline (and that is not final).
    let link = PairingLink::parse(&identity.desktop_pairing_url(&relay.url, "studio-pc")).unwrap();
    let client_id = new_client_id();
    let offline = RelayClient::start(
        link.endpoint.clone(),
        Credential::PairingKey { client_id: client_id.clone(), key: link.pairing_key.clone() },
        "laptop",
        Arc::new(DirectProxy),
        quick(),
        None,
    );
    assert_eq!(offline.ready(Duration::from_secs(5)).await, Err(RelayFailure::HostOffline));
    assert!(!RelayFailure::HostOffline.is_final());
    offline.close();

    let sharing = make_host(MakeHost::default()).await;
    let host = relay_host(&folder, identity.clone(), phones.clone(), Some(sharing.host.clone()));
    host.apply(settings(&relay.url, false)).await.unwrap();
    host.set_desktops(true).await.unwrap();
    until(|| relay.host_attached(), 5000, "the host's link for paired computers").await;

    let tokens = Arc::new(Mutex::new(vec![]));
    let told = tokens.clone();
    let client = RelayClient::start(
        link.endpoint.clone(),
        Credential::PairingKey { client_id: client_id.clone(), key: link.pairing_key.clone() },
        "laptop",
        Arc::new(DirectProxy),
        ClientTimings { heartbeat: Duration::from_millis(200), ..quick() },
        Some(Arc::new(move |credential: &Credential| told.lock().unwrap().push(credential.clone()))),
    );
    let hello = client.ready(Duration::from_secs(5)).await.unwrap();
    assert_eq!(hello.host_name, "studio-pc");
    assert!(client.credential().has_token(), "the host gave its token back once");
    assert_eq!(tokens.lock().unwrap().len(), 1);
    assert_eq!(host.connected_desktops(), ["laptop"]);
    // Sharing is off: the host says so, through the relay, in its own words.
    let stopped = client.request("GET", "/v2/info", None, Duration::from_secs(5)).await.unwrap();
    assert_eq!(stopped.status, 503);
    // Several at once, by id, while this side's pings and the host's go back and forth.
    let asks = (0..8).map(|_| client.request("GET", "/v2/mirror", None, Duration::from_secs(5)));
    for answer in futures_util::future::join_all(asks).await {
        assert_eq!(answer.unwrap().status, 503);
    }
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(client.request("GET", "/v2/info", None, Duration::from_secs(5)).await.unwrap().status, 503, "alive past pings");
    // A body that does not fit one frame never leaves.
    let huge = format!("{{\"text\":\"{}\"}}", "a".repeat(MAXIMUM_PLAINTEXT));
    assert_eq!(client.request("POST", "/v2/mirror", Some(huge.as_bytes()), Duration::from_secs(5)).await, Err(RequestError::TooLarge));
    assert_eq!(phones.0.load(Ordering::SeqCst), 0, "a computer never reaches the phone's routes");
    // The token alone opens it again.
    let token = client.credential();
    client.close();
    let again = RelayClient::start(link.endpoint.clone(), token, "laptop", Arc::new(DirectProxy), quick(), None);
    assert!(again.ready(Duration::from_secs(5)).await.is_ok());
    again.close();
    // Another key under the same relay name: a stale pairing, final.
    let mut stale = link.endpoint.clone();
    stale.public_key = [1u8; 32];
    let wrong = RelayClient::start(stale, client.credential(), "laptop", Arc::new(DirectProxy), quick(), None);
    let failure = wrong.ready(Duration::from_secs(5)).await.unwrap_err();
    assert!(failure == RelayFailure::KeyMismatch && failure.is_final());
    // A pairing key the host does not have: refused for good.
    let refused = RelayClient::start(
        link.endpoint.clone(),
        Credential::PairingKey { client_id: new_client_id(), key: zeroize::Zeroizing::new("nope".into()) },
        "laptop",
        Arc::new(DirectProxy),
        quick(),
        None,
    );
    let failure = refused.ready(Duration::from_secs(5)).await.unwrap_err();
    assert!(matches!(&failure, RelayFailure::Refused { reason, last: true } if reason == "pairing-key"), "{failure:?}");
    assert!(refused.final_failure().is_some());
    host.dispose().await;
    sharing.dispose().await;
    let _ = std::fs::remove_dir_all(folder);
}

/// End to end: a computer pairs with the link, its pairing kept in the vault (the host's token, never the key) and only a `relay:`
/// address in the connections file; it mirrors the shared workspace's panes with the same refusals as over Tailscale (a private pane
/// unknown, full access and a Windows host's Codex behind the switch), a pane's detail fits one frame, attachments past a frame are
/// refused here, the runs are not carried; a disconnect and a reconnect use the token; the relay switch off answers 503.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_paired_computer_mirrors_the_shared_panes_through_the_relay() {
    let relay = PairingRelay::start().await;
    let folder = temp("pair-mirror");
    let identity = identity_in(&folder);
    let phones = Arc::new(Phones::default());
    let sharing = make_host(MakeHost { windows_host: Some(true), ..MakeHost::default() }).await;
    let panes = Panes::new(sharing.shared.id(), sharing.private.id(), 80);
    sharing.host.set_mirror(Some(panes.clone()));
    sharing.host.set_relay(true);
    sharing.host.start(&[sharing.shared.id().to_string()], 0).await.unwrap();
    let host = relay_host(&folder, identity.clone(), phones.clone(), Some(sharing.host.clone()));
    host.apply(settings(&relay.url, false)).await.unwrap();
    host.set_desktops(true).await.unwrap();
    until(|| relay.host_attached(), 5000, "the host's link for paired computers").await;

    let client_folder = temp("pair-client");
    let vault = Arc::new(MemoryVault::default());
    let client = client_in(&client_folder, vault.clone());
    let bad = client.pair("", "mightycode://pair?v=2").await.unwrap_err();
    assert_eq!(bad.message(), get("remote.pair.error.badLink"));
    // The phone's link pairs no computer, said as a link that cannot be used.
    let phone_link = host.pairing_url();
    let refused = client.pair("", &phone_link).await.unwrap_err();
    assert_eq!(refused.message(), get("remote.pair.error.codeRefused"));
    let link = host.desktop_pairing_url();
    let info = client.pair("", &link).await.unwrap();
    assert_eq!(info.status, RemoteConnectionStatus::Connected);
    assert_eq!(info.name, "studio-pc", "named after the host's link");
    assert!(paired::is_relay_address(&info.address) && client.is_paired(&info.id));
    assert_eq!(info.shared().iter().map(|w| w.id.as_str()).collect::<Vec<_>>(), [sharing.shared.id()]);
    // The vault keeps the token, never the key; the connections file names the relay address only.
    let secret = PairedSecret::decode(&vault.load(&info.id).unwrap()).unwrap();
    assert!(secret.credential.has_token());
    let key = member(&link, "key");
    assert!(!vault.load(&info.id).unwrap().contains(&key));
    assert!(client.pair("", &link).await.is_err(), "the link was spent by its pairing");
    let file = std::fs::read_to_string(client_folder.join(mighty_remote::remote::saved::FILE_NAME)).unwrap();
    assert!(file.contains(&info.address) && !file.contains(&key) && !file.contains(&member(&link, "relay")));
    let Credential::Token { token, .. } = &secret.credential else { unreachable!() };
    assert!(!file.contains(token.as_str()));

    // The shared workspace's panes, as over Tailscale.
    let state = client.mirror_state(&info.id, None).await.unwrap();
    assert_eq!(state.host_name, "studio-pc");
    assert_eq!(state.sessions.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["pane-1", "pane-2"]);
    assert!(client.mirror_state(&info.id, Some(&state.revision)).await.unwrap().unchanged);
    let detail = client.mirror_detail(&info.id, "pane-1").await.unwrap();
    assert!(detail.session.has_older && !detail.session.logs.is_empty(), "the detail is fitted, the newest entries kept");
    let hidden = client.mirror_detail(&info.id, "pane-hidden").await.unwrap_err();
    assert_eq!(refusal_status(&hidden), Some(404), "a pane outside the shared workspaces is unknown");
    // Full access and a Windows host's Codex need the switch, through the relay too.
    *panes.mode.lock().unwrap() = "fullAccess".into();
    let refused = client.mirror_submit(&info.id, "pane-1", "go", vec![]).await.unwrap_err();
    assert_eq!((refusal_status(&refused), refused.message()), (Some(403), get("remote.host.error.fullAccessOff")));
    *panes.provider.lock().unwrap() = "codex".into();
    assert_eq!(refusal_status(&client.mirror_submit(&info.id, "pane-2", "go", vec![]).await.unwrap_err()), Some(403));
    assert_eq!(refusal_status(&client.mirror_permission(&info.id, "pane-2", "r", "run", true, true).await.unwrap_err()), Some(403));
    sharing.host.set_allows_full_access(true);
    assert_eq!(client.mirror_submit(&info.id, "pane-1", "go", vec![]).await.unwrap(), "queued");
    assert_eq!(client.mirror_submit(&info.id, "pane-2", "go", vec![]).await.unwrap(), "queued");
    client.mirror_stop(&info.id, "pane-1").await.unwrap();
    assert_eq!(client.mirror_create_session(&info.id, sharing.shared.id(), "claude").await.unwrap(), "pane-3");
    // An attachment that fits a frame goes; one that does not is refused here, before anything is sent.
    let small = RunAttachment {
        id: "a1".into(),
        name: "a.png".into(),
        media_type: "image/png".into(),
        data_base64: base64::engine::general_purpose::STANDARD.encode(png(1000)),
    };
    assert_eq!(client.mirror_submit(&info.id, "pane-1", "see", vec![small.clone()]).await.unwrap(), "queued");
    let before = panes.actions().len();
    let big = RunAttachment { data_base64: base64::engine::general_purpose::STANDARD.encode(png(900 * 1024)), ..small };
    let too_big = client.mirror_submit(&info.id, "pane-1", "see", vec![big]).await.unwrap_err();
    assert!(
        matches!(&too_big, ClientError::Message(words) if *words == get_with("remote.relay.error.attachmentsTooLarge", &[("size", "1 MB")]))
    );
    assert_eq!(panes.actions().len(), before, "the host never saw it");
    assert_eq!(
        panes.actions()[..4],
        ["submit pane-1 go 0", "submit pane-2 go 0", "stop pane-1", &format!("create {} claude", sharing.shared.id())]
    );
    // The connection stays the mirror's: its runs are not carried.
    let raw = sharing.host.serve_relay("device", "GET", "/v2/runs/x/poll", None).await;
    assert_eq!(raw.status, 404);
    let off_route = sharing.host.serve_relay("device", "GET", "/m1/state", None).await;
    assert_eq!(off_route.status, 400, "a phone route is no computer's");
    assert_eq!(phones.0.load(Ordering::SeqCst), 0);

    // Disconnected, then connected again with the token from the vault.
    client.disconnect(&info.id).await;
    assert_eq!(client.connection(&info.id).unwrap().status, RemoteConnectionStatus::Disconnected);
    let again = client.refresh(&info.id).await.unwrap();
    assert_eq!(again.status, RemoteConnectionStatus::Connected);
    assert_eq!(identity.devices().len(), 1, "the reconnect registered nothing new");
    // The relay switch off: the host answers 503 at once.
    sharing.host.set_relay(false);
    let stopped = client.mirror_state(&info.id, None).await.unwrap_err();
    assert_eq!(refusal_status(&stopped), None, "503 is a passing refusal");
    assert_eq!(stopped.status(), Some(503));
    host.dispose().await;
    sharing.dispose().await;
    let _ = std::fs::remove_dir_all(folder);
    let _ = std::fs::remove_dir_all(client_folder);
}

/// A pane's detail through the relay fits one frame, however long its transcript (the HTTP detail is fitted to 1.5 MiB).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_details_answer_through_the_relay_fits_one_frame() {
    let sharing = make_host(MakeHost::default()).await;
    let panes = Panes::new(sharing.shared.id(), sharing.private.id(), 80);
    sharing.host.set_mirror(Some(panes));
    sharing.host.set_relay(true);
    sharing.host.start(&[sharing.shared.id().to_string()], 0).await.unwrap();
    let answer = sharing.host.serve_relay("device", "GET", "/v2/mirror/sessions/pane-1", None).await;
    assert_eq!(answer.status, 200);
    let frame = mighty_remote::relay::connection::reply_frame("id-1", &answer);
    assert!(frame.len() <= MAXIMUM_PLAINTEXT, "{} bytes", frame.len());
    let body: Value = serde_json::from_str(&answer.body).unwrap();
    assert!(serde_json::to_vec(&body["session"]).unwrap().len() <= RELAY_DETAIL_BYTES);
    sharing.dispose().await;
}

/// A phone gains nothing: while only computers keep the link up (the phone remote off) a phone is turned away as from an offline host
/// before its auth is read; with the phone remote on it pairs as before and its `/v2/mirror` goes to the phone's router, never the
/// sharing host.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_phone_gains_nothing() {
    let relay = PairingRelay::start().await;
    let folder = temp("pair-phone");
    let identity = identity_in(&folder);
    let phones = Arc::new(Phones::default());
    let sharing = make_host(MakeHost::default()).await;
    let panes = Panes::new(sharing.shared.id(), sharing.private.id(), 2);
    sharing.host.set_mirror(Some(panes.clone()));
    sharing.host.set_relay(true);
    sharing.host.start(&[sharing.shared.id().to_string()], 0).await.unwrap();
    let host = relay_host(&folder, identity.clone(), phones.clone(), Some(sharing.host.clone()));
    host.apply(settings(&relay.url, false)).await.unwrap();
    host.set_desktops(true).await.unwrap();
    until(|| relay.host_attached(), 5000, "the host's link").await;
    let link = host.pairing_url();
    let key = member(&link, "key");
    let auth = json!({"type":"auth","pairingKey":key,"clientId":new_client_id(),"clientName":"Phone"});
    let turned_away = phone(&relay.url, &link, auth.clone()).await.err();
    assert_eq!(turned_away, Some(Some(mighty_remote::relay::connection::PHONES_OFF_CLOSE)), "the phone meets an offline host");
    assert!(identity.devices().is_empty(), "nothing was registered");

    // The phone remote on: the phone pairs, and the mirror's path is the phone router's.
    host.apply(settings(&relay.url, true)).await.unwrap();
    until(|| relay.host_attached(), 5000, "the host's link").await;
    // The host's link may still be settling after the change: a handshake that meets no `ready` is tried again.
    let mut tries = 0;
    let (answer, cipher, mut socket) = loop {
        match phone(&relay.url, &link, auth.clone()).await {
            Ok(done) => break done,
            // 4404: the relay has no host for the id yet (its link is coming back after the change), as a socket ended early.
            Err(None | Some(4404)) if tries < 50 => {
                tries += 1;
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            Err(code) => panic!("the phone could not pair: {code:?}"),
        }
    };
    assert_eq!(answer["type"], "auth_ok");
    let asked = panes.asked.load(Ordering::SeqCst);
    let request = json!({"id":"r1","method":"GET","path":"/v2/mirror"});
    socket.send(Message::binary(cipher.seal(request.to_string().as_bytes()).unwrap())).await.unwrap();
    let reply = loop {
        let Some(Ok(Message::Binary(frame))) = socket.next().await else { panic!("no reply") };
        let value: Value = serde_json::from_slice(&cipher.open(&frame).unwrap()).unwrap();
        if value["id"] == "r1" {
            break value;
        }
    };
    assert_eq!(reply["body"], json!({"phone": true}), "the phone's router answered");
    assert_eq!(panes.asked.load(Ordering::SeqCst), asked, "the sharing host never heard of it");
    // The router is what decides: a phone device's request never reaches the sharing host.
    let router = DesktopRouter::new(phones.clone(), {
        let host = sharing.host.clone();
        Arc::new(move || Some(host.clone()))
    });
    let device = MobileDevice { client_id: "phone".into(), name: "Phone".into(), direct: false, desktop: false };
    let reply = router.handle(device, "GET".into(), "/v2/mirror".into(), None).await;
    assert_eq!((reply.status, reply.body.as_str()), (200, r#"{"phone":true}"#));
    assert_eq!(panes.asked.load(Ordering::SeqCst), asked, "the sharing host never heard of it");
    let computer = MobileDevice { client_id: "laptop".into(), name: "laptop".into(), direct: false, desktop: true };
    let phones_before = phones.0.load(Ordering::SeqCst);
    let reply = router.handle(computer, "GET".into(), "/v2/mirror".into(), None).await;
    assert_eq!(reply.status, 200);
    assert_eq!(phones.0.load(Ordering::SeqCst), phones_before, "a computer's request never reaches the phone's router");
    host.dispose().await;
    sharing.dispose().await;
    let _ = std::fs::remove_dir_all(folder);
}

/// With "페어링한 컴퓨터의 연결 받기" on, a PC without Tailscale shares with paired computers alone (on no address, no port); off, it is
/// refused as before. The switch outlives a stop and a start.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sharing_starts_without_tailscale_only_for_paired_computers() {
    let none: mighty_remote::remote::host::Addresses = Arc::new(|| Ok(vec![]));
    let sharing = make_host(MakeHost { addresses: Some(none), ..MakeHost::default() }).await;
    let ids = [sharing.shared.id().to_string()];
    assert_eq!(sharing.host.start(&ids, 0).await.unwrap_err(), mighty_remote::remote::strings::host_no_tailscale());
    sharing.host.set_relay(true);
    let state = sharing.host.start(&ids, 0).await.unwrap();
    assert!(state.enabled && state.addresses.is_empty() && state.port.is_none() && sharing.host.listeners().is_empty());
    assert_eq!(sharing.host.serve_relay("device", "GET", "/v2/info", None).await.status, 200, "a paired computer is served");
    sharing.host.stop().await;
    assert!(sharing.host.relay(), "the switch outlives the sharing");
    assert_eq!(sharing.host.serve_relay("device", "GET", "/v2/info", None).await.status, 503, "nothing is served once it stops");
    sharing.dispose().await;
}

/// A row's 연결 해제 unpairs that device alone: the phone's connection is closed and its token refused, the paired computer stays
/// connected (and its token keeps opening the host), the pairing key is made anew so the old one pairs nothing, and the link shows the
/// new key. 키 다시 만들기 still unpairs everyone. The device list marks the computer as one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn revoking_one_device_keeps_the_others_paired_and_connected() {
    let relay = PairingRelay::start().await;
    let folder = temp("pair-revoke-one");
    let identity = identity_in(&folder);
    let phones = Arc::new(Phones::default());
    let sharing = make_host(MakeHost::default()).await;
    let host = relay_host(&folder, identity.clone(), phones.clone(), Some(sharing.host.clone()));
    host.apply(settings(&relay.url, true)).await.unwrap();
    host.set_desktops(true).await.unwrap();
    until(|| relay.host_attached(), 5000, "the host's link").await;
    let link = host.pairing_url();
    let old_key = member(&link, "key");

    // A computer pairs through the relay and keeps its token.
    let parsed = PairingLink::parse(&host.desktop_pairing_url()).unwrap();
    let computer = RelayClient::start(
        parsed.endpoint.clone(),
        Credential::PairingKey { client_id: new_client_id(), key: parsed.pairing_key.clone() },
        "laptop",
        Arc::new(DirectProxy),
        quick(),
        None,
    );
    computer.ready(Duration::from_secs(5)).await.unwrap();
    let computer_token = computer.credential();
    assert!(computer_token.has_token());
    // A phone pairs and stays connected.
    let phone_id = new_client_id();
    let auth = json!({"type":"auth","pairingKey":old_key,"clientId":phone_id,"clientName":"Phone"});
    let (answer, _cipher, mut socket) = phone(&relay.url, &link, auth).await.unwrap();
    assert_eq!(answer["type"], "auth_ok");
    let phone_token = answer["deviceToken"].as_str().unwrap().to_string();
    until(|| host.devices().iter().filter(|d| d.connected).count() == 2, 5000, "both connected").await;
    let listed = host.devices();
    assert!(listed.iter().any(|d| d.id == computer_token.client_id() && d.desktop), "the computer is listed as one: {listed:?}");
    assert!(listed.iter().any(|d| d.id == phone_id && !d.desktop), "the phone as a phone");

    assert!(host.revoke(&phone_id).await.unwrap(), "the phone was paired");
    // Only the phone's connection ends.
    let closed = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match socket.next().await {
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(_)) => continue,
            }
        }
    })
    .await;
    assert!(closed.is_ok(), "the revoked phone's connection was closed");
    let devices = host.devices();
    assert_eq!(devices.iter().map(|d| d.id.as_str()).collect::<Vec<_>>(), [computer_token.client_id()], "only the computer is left");
    assert!(devices[0].connected && devices[0].desktop, "and it is still connected");
    assert_eq!(host.connected_desktops(), ["laptop"]);
    assert_eq!(computer.request("GET", "/v2/info", None, Duration::from_secs(5)).await.unwrap().status, 503, "it still asks");
    // The pairing link carries a new code; the old one pairs nothing, the phone's token opens nothing.
    let new_link = host.pairing_url();
    assert_ne!(member(&new_link, "key"), old_key);
    assert_eq!(member(&new_link, "key"), host.pairing_key_for_display().as_str());
    let again = json!({"type":"auth","pairingKey":old_key,"clientId":new_client_id(),"clientName":"Stranger"});
    let (refused, _, _) = phone(&relay.url, &new_link, again).await.unwrap();
    assert_eq!((refused["type"].as_str(), refused["reason"].as_str()), (Some("auth_error"), Some("pairing-key")));
    let token_auth = json!({"type":"auth","deviceToken":phone_token,"clientId":phone_id});
    let (refused, _, _) = phone(&relay.url, &new_link, token_auth).await.unwrap();
    assert_eq!((refused["type"].as_str(), refused["reason"].as_str()), (Some("auth_error"), Some("device-revoked")));
    // The computer's token still opens the host on a new connection.
    let reopened = RelayClient::start(parsed.endpoint.clone(), computer_token.clone(), "laptop", Arc::new(DirectProxy), quick(), None);
    assert!(reopened.ready(Duration::from_secs(5)).await.is_ok(), "the computer's token is untouched");
    reopened.close();
    assert!(!host.revoke(&phone_id).await.unwrap(), "a stale press changes nothing");
    assert_eq!(host.pairing_url(), new_link);

    // 키 다시 만들기: everyone goes, and the host is made anew — a new relay name and key, the link registered again under them.
    let (server_id, public_key) = (identity.server_id(), identity.public_key());
    assert!(host.rotate(None).await.unwrap());
    assert!(host.devices().is_empty(), "every device unpaired");
    assert!(identity.server_id() != server_id && identity.public_key() != public_key, "new keys");
    until(|| host.connected_desktops().is_empty(), 5000, "the computer's connection closed").await;
    computer.close();
    // The old pairing opens nothing: no host under the old name (this test relay hands it to the one host, whose key is new).
    let gone = RelayClient::start(parsed.endpoint.clone(), computer_token, "laptop", Arc::new(DirectProxy), quick(), None);
    let failure = gone.ready(Duration::from_secs(5)).await.unwrap_err();
    assert!(matches!(failure, RelayFailure::HostOffline | RelayFailure::KeyMismatch), "{failure:?}");
    gone.close();
    // The new link, under the new name, pairs again.
    let fresh = PairingLink::parse(&host.desktop_pairing_url()).unwrap();
    assert_eq!(fresh.endpoint.server_id, identity.server_id());
    let renewed = RelayClient::start(
        fresh.endpoint.clone(),
        Credential::PairingKey { client_id: new_client_id(), key: fresh.pairing_key.clone() },
        "laptop",
        Arc::new(DirectProxy),
        quick(),
        None,
    );
    renewed.ready(Duration::from_secs(10)).await.expect("the host answers under its new name");
    renewed.close();
    host.dispose().await;
    sharing.dispose().await;
    let _ = std::fs::remove_dir_all(folder);
}
