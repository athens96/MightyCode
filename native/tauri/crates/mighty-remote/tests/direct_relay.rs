//! The direct path's checks that need the relay host's connection (W8-A over W8-C's `direct::listener::DirectPath`; MightyCode.Core.Tests
//! `MobileDirectVerification`), ported by C# name: L492 "mobile direct: a phone connects directly on the relay's handshake", L500
//! "mobile direct: relay notices carrying a direct- id leave the direct phone alone", L502 "mobile direct: a relay auth frame that is no
//! JSON is answered malformed", and the relay halves of L493 (a relay redial leaves a direct phone alone), L494 (the QR and auth_ok carry
//! the endpoints) and L501 (the cipher's 28 bytes of the 1 MiB frame). The direct path listens on loopback through its address seam and
//! the fake network (which binds loopback alone); the relay is the in-process fake.

mod support;

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mighty_core::model::MobileRemoteSettings;
use mighty_platform::fake::FakeNetwork;
use mighty_platform::{BoxFuture, Os, fake};
use mighty_remote::direct::listener::{DirectPath, DirectPathOptions, DirectSettings};
use mighty_remote::direct::wire;
use mighty_remote::identity::{Identity, escape_data};
use mighty_remote::relay::cipher;
use mighty_remote::relay::connection::{MobileDevice, MobileReply, RequestHandler};
use mighty_remote::relay::host::{RelayHost, RelayOptions};
use serde_json::{Value, json};
use support::fake_relay::{self, FakeRelay, FileStore, ScriptedPhone};
use tokio::sync::Notify;

fn temp(name: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let path = std::env::temp_dir().join(format!("mighty-direct-{name}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn query(link: &str, name: &str) -> String {
    let raw = link.split_once('?').unwrap().1.split('&').find_map(|pair| pair.strip_prefix(&format!("{name}="))).unwrap();
    let bytes = raw.as_bytes();
    let (mut out, mut at) = (vec![], 0);
    while at < bytes.len() {
        if bytes[at] == b'%' {
            out.push(u8::from_str_radix(&raw[at + 1..at + 3], 16).unwrap());
            at += 3;
        } else {
            out.push(bytes[at]);
            at += 1;
        }
    }
    String::from_utf8(out).unwrap()
}

/// Every request answered 200 with its path (C#'s fixture route).
struct Paths;

impl RequestHandler for Paths {
    fn handle(
        &self,
        _: MobileDevice,
        _: String,
        path: String,
        _: Option<Box<serde_json::value::RawValue>>,
    ) -> BoxFuture<'static, MobileReply> {
        Box::pin(async move { MobileReply::new(200, json!({"protocol":1,"path":path}).to_string()) })
    }
}

/// A relay host whose direct phones come in through a direct path on loopback (C#'s `SetDirectAddressSource(() => ["127.0.0.1"])`).
struct Fixture {
    host: Arc<RelayHost>,
    path: DirectPath,
    directory: PathBuf,
}

impl Fixture {
    async fn new(name: &str, capabilities: &[&str]) -> Fixture {
        Fixture::with_handler(name, capabilities, Arc::new(Paths)).await
    }

    async fn with_handler(name: &str, capabilities: &[&str], handler: Arc<dyn RequestHandler>) -> Fixture {
        let directory = temp(name);
        let identity = Arc::new(Identity::load_or_create(Arc::new(FileStore(directory.clone())), true).unwrap());
        let path = DirectPath::new(
            Arc::new(FakeNetwork::default()),
            tokio::runtime::Handle::current(),
            DirectPathOptions::default(),
            Arc::new(|_| {}),
        );
        let endpoints = path.clone();
        let options = RelayOptions {
            host_name: "Fixture".into(),
            watches_network: false,
            capabilities: capabilities.iter().map(|c| c.to_string()).collect(),
            direct_endpoints: Some(Arc::new(move || endpoints.endpoints())),
            // Loopback stands in for Tailscale (C#'s address seam).
            any_direct_peer: true,
            ..RelayOptions::default()
        };
        let platform = fake::platform(&directory, Os::Windows);
        let host = Arc::new(RelayHost::with_options(identity, MobileRemoteSettings::default(), handler, platform, options));
        path.set_sink(Some(host.direct_sink()));
        path.set_address_source(Some(Arc::new(|| Ok(vec!["127.0.0.1".to_string()]))), 0).await;
        Fixture { host, path, directory }
    }

    /// The settings applied to both halves, as src-tauri's write applies them (the relay host, then `direct::apply`).
    async fn apply(&self, settings: MobileRemoteSettings) {
        self.host.apply(settings.clone()).await.unwrap();
        self.path.apply(DirectSettings { enabled: settings.enabled, direct_enabled: settings.direct_enabled, port: 0 }).await;
    }

    fn endpoint(&self) -> String {
        self.path.endpoints().pop().expect("an endpoint")
    }

    async fn done(self) {
        self.path.dispose();
        self.host.dispose().await;
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

fn settings(relay: &str, direct: bool) -> MobileRemoteSettings {
    MobileRemoteSettings { enabled: true, relay_url: Some(relay.into()), direct_enabled: direct, ..MobileRemoteSettings::default() }
}

/// A phone on the direct path: `ws://endpoint/ws?v=1`, hello, the paired key checked.
async fn direct_phone(endpoint: &str, pairing: &str) -> ScriptedPhone {
    let tcp = tokio::net::TcpStream::connect(endpoint).await.unwrap();
    let (socket, _) = tokio_tungstenite::client_async(format!("ws://{endpoint}/ws?v=1"), tcp).await.unwrap();
    let mut phone = ScriptedPhone::new(socket);
    let ready = phone.hello().await.clone();
    assert_eq!(
        query(pairing, "pk").replace('-', "+").replace('_', "/") + "=",
        ready["serverKey"].as_str().unwrap(),
        "the direct path answers with the paired public key"
    );
    phone
}

async fn exchange(phone: &mut ScriptedPhone, message: Value) -> Value {
    phone.send(&message.to_string()).await;
    phone.receive_reply().await.expect("an answer")
}

fn auth(pairing: &str, client: &str) -> Value {
    json!({"type":"auth","pairingKey":query(pairing, "key"),"clientId":client,"clientName":"Fixture Phone"})
}

async fn until(limit: Duration, condition: impl Fn() -> bool) {
    let deadline = tokio::time::Instant::now() + limit;
    while !condition() {
        assert!(tokio::time::Instant::now() < deadline, "timed out waiting");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// L492 "mobile direct: a phone connects directly on the relay's handshake": while the relay (a closed local port) fails and redials, a
/// phone pairs over the direct path, gets the endpoints in auth_ok, tunnels requests and joins the table with a `direct-` id; a wrong
/// key and an auth frame that is no JSON each hear the one generic word.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mobile_direct_a_phone_connects_directly_on_the_relays_handshake() {
    let _lane = support::lane::enter().await;
    let fixture = Fixture::new("handshake", &[]).await;
    assert!(fixture.path.endpoints().is_empty(), "nothing listens while the remote is off");
    fixture.apply(settings("ws://127.0.0.1:9", true)).await;
    let endpoint = fixture.endpoint();
    assert!(endpoint.starts_with("127.0.0.1:"), "the seam's address is listened on and advertised: {endpoint}");
    let pairing = fixture.host.pairing_url();
    let mut phone = direct_phone(&endpoint, &pairing).await;
    let answer = exchange(&mut phone, auth(&pairing, "abcdefghijklmnopqrstuv")).await;
    assert!(answer["type"] == "auth_ok" && answer["deviceToken"].is_string(), "a phone pairs over the direct path");
    assert_eq!(answer["direct"], json!(fixture.path.endpoints()), "auth_ok carries the direct endpoints");
    let info = exchange(&mut phone, json!({"id":"request-1","method":"GET","path":"/m1/info"})).await;
    assert!(info["id"] == "request-1" && info["status"] == 200, "requests are tunnelled as on the relay");
    let ids = fixture.host.connection_ids();
    assert!(
        ids.len() == 1
            && wire::is_direct(&ids[0])
            && fixture.host.devices().iter().all(|d| d.connected)
            && fixture.host.status().connections == 1,
        "the phone joins the same table with a direct- id"
    );
    drop(phone);
    until(Duration::from_secs(10), || fixture.host.connection_ids().is_empty()).await;
    let mut stranger = direct_phone(&endpoint, &pairing).await;
    let refused = exchange(
        &mut stranger,
        json!({"type":"auth","pairingKey":"wrong-key-0000000000","clientId":"zyxwvutsrqponmlkjihgfe","clientName":"Stranger"}),
    )
    .await;
    assert!(refused["type"] == "auth_error" && refused["reason"] == wire::GENERIC_REFUSAL, "a direct refusal is the generic unauthorized");
    let mut garbled = direct_phone(&endpoint, &pairing).await;
    garbled.send_bytes(b"not json {").await;
    let refused = garbled.receive_reply().await.unwrap();
    assert!(
        refused["type"] == "auth_error" && refused["reason"] == wire::GENERIC_REFUSAL,
        "an auth frame that is no JSON hears the generic unauthorized"
    );
    fixture.done().await;
}

/// L493 "mobile direct: only who may gets in, and relay redials leave them alone", the relay host's half: a paired direct phone stays
/// through a relay redial; the direct switch off ends it; five refused keys from one address shut it out.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mobile_direct_relay_redials_leave_a_direct_phone_alone() {
    let _lane = support::lane::enter().await;
    let fixture = Fixture::new("redial", &[]).await;
    fixture.apply(settings("ws://127.0.0.1:9", true)).await;
    let endpoint = fixture.endpoint();
    let pairing = fixture.host.pairing_url();
    let mut phone = direct_phone(&endpoint, &pairing).await;
    assert_eq!(exchange(&mut phone, auth(&pairing, "abcdefghijklmnopqrstuv")).await["type"], "auth_ok", "the phone is in");
    fixture.host.reconnect().await.unwrap();
    let after = exchange(&mut phone, json!({"id":"after-redial","method":"GET","path":"/m1/info"})).await;
    assert_eq!(after["id"], "after-redial", "a relay redial leaves a direct phone connected");
    fixture.apply(settings("ws://127.0.0.1:9", false)).await;
    until(Duration::from_secs(10), || !fixture.host.connection_ids().iter().any(|id| wire::is_direct(id))).await;
    assert!(phone.ended().await, "the direct switch off ends the direct phones");
    fixture.apply(settings("ws://127.0.0.1:9", true)).await;
    let endpoint = fixture.endpoint();
    for i in 0..mighty_remote::direct::admit::MAXIMUM_FAILURES {
        let mut stranger = direct_phone(&endpoint, &pairing).await;
        let refused = exchange(&mut stranger, json!({"type":"auth","pairingKey":"wrong-key-0000000000","clientId":format!("zyxwvutsrqponmlkjihgf{i}"),"clientName":"Stranger"})).await;
        assert_eq!(refused["reason"], wire::GENERIC_REFUSAL, "every refusal is the same word");
        drop(stranger);
        until(Duration::from_secs(10), || fixture.host.connection_ids().is_empty()).await;
    }
    // Reset at accept: where the reset arrives before the connect is seen to complete (macOS) the connect itself fails with it.
    let locked = match tokio::net::TcpStream::connect(&endpoint).await {
        Ok(tcp) => tokio_tungstenite::client_async(format!("ws://{endpoint}/ws?v=1"), tcp).await.is_err(),
        Err(error) => matches!(
            error.kind(),
            std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted | std::io::ErrorKind::BrokenPipe
        ),
    };
    assert!(locked, "an address that keeps failing is shut out");
    fixture.done().await;
}

/// L494 "mobile direct: the host advertises its endpoints", the relay host's half: no `direct=` while nothing listens, the QR carrying
/// the endpoint once it does (`:`, `,`, `[` and `]` percent-encoded as the Mac writes them), auth_ok listing the direct capability.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mobile_direct_the_host_advertises_its_endpoints() {
    let _lane = support::lane::enter().await;
    let fixture = Fixture::new("advertises", &mighty_remote::phone::router::CAPABILITIES).await;
    assert!(mighty_remote::phone::router::CAPABILITIES.contains(&"direct"), "the host advertises the direct capability");
    assert!(!fixture.host.pairing_url().contains("direct="), "no direct= while nothing listens");
    fixture.apply(settings("ws://127.0.0.1:9", true)).await;
    let endpoint = fixture.endpoint();
    let pairing = fixture.host.pairing_url();
    assert!(
        pairing.ends_with(&format!("&direct={}", escape_data(&endpoint))) && query(&pairing, "direct") == endpoint,
        "the QR carries the endpoints"
    );
    let other = Identity::load_or_create(Arc::new(fake::MemoryPairing::default()), true).unwrap();
    let both = ["100.101.102.103:43139".to_string(), "[fd7a:115c:a1e0::1]:43139".to_string()];
    assert!(
        other
            .pairing_url("wss://relay.example", "Fixture", &both)
            .ends_with("&direct=100.101.102.103%3A43139%2C%5Bfd7a%3A115c%3Aa1e0%3A%3A1%5D%3A43139"),
        "':', ',', '[' and ']' are percent-encoded as the Mac writes them"
    );
    let mut phone = direct_phone(&endpoint, &pairing).await;
    let answer = exchange(&mut phone, auth(&pairing, "abcdefghijklmnopqrstuv")).await;
    assert!(answer["capabilities"].as_array().unwrap().iter().any(|c| c == "direct"), "auth_ok lists the direct capability");
    drop(phone);
    fixture.done().await;
}

/// L500 "mobile direct: relay notices carrying a direct- id leave the direct phone alone": the relay's disconnected and connected notices
/// for the direct phone's id, and a connected notice for another direct- id, dial nothing and leave the phone in the table, answering.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mobile_direct_relay_notices_carrying_a_direct_id_leave_the_direct_phone_alone() {
    let _lane = support::lane::enter().await;
    let direct_id: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let ready = Arc::new(Notify::new());
    let dialled: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(vec![]));
    let control: fake_relay::ControlScript = {
        let (direct_id, ready) = (direct_id.clone(), ready.clone());
        Arc::new(move |number, mut socket| {
            let (direct_id, ready) = (direct_id.clone(), ready.clone());
            Box::pin(async move {
                use futures_util::SinkExt;
                if number == 1 {
                    ready.notified().await;
                    let id = direct_id.lock().unwrap().clone().unwrap();
                    let other = format!("{}{}", wire::CONNECTION_PREFIX, uuid::Uuid::new_v4());
                    for (kind, connection) in [
                        ("disconnected", id.as_str()),
                        ("connected", id.as_str()),
                        ("connected", other.as_str()),
                        ("connected", "fixture-after-notices"),
                    ] {
                        let notice = json!({"type": kind, "connectionId": connection}).to_string();
                        let _ = socket.send(tokio_tungstenite::tungstenite::Message::text(notice)).await;
                    }
                }
                fake_relay::listen(socket).await;
            })
        })
    };
    let phone_script: fake_relay::PhoneScript = {
        let dialled = dialled.clone();
        Arc::new(move |id, socket| {
            dialled.lock().unwrap().push(id);
            Box::pin(fake_relay::listen(socket))
        })
    };
    let relay = FakeRelay::start(control, phone_script).await;
    let fixture = Fixture::new("notices", &[]).await;
    fixture.apply(settings(&relay.address, true)).await;
    until(Duration::from_secs(10), || fixture.host.status().connected && relay.controls() == 1).await;
    let pairing = fixture.host.pairing_url();
    let mut phone = direct_phone(&fixture.endpoint(), &pairing).await;
    assert_eq!(exchange(&mut phone, auth(&pairing, "abcdefghijklmnopqrstuv")).await["type"], "auth_ok", "the phone is in");
    let ids = fixture.host.connection_ids();
    assert!(ids.len() == 1 && wire::is_direct(&ids[0]), "the phone holds a direct- id");
    *direct_id.lock().unwrap() = Some(ids[0].clone());
    ready.notify_one();
    until(Duration::from_secs(10), || !dialled.lock().unwrap().is_empty()).await;
    assert_eq!(*dialled.lock().unwrap(), vec!["fixture-after-notices"], "a connected notice for a direct- id dials nothing");
    let now = fixture.host.connection_ids();
    assert!(
        now.contains(&ids[0]) && now.iter().filter(|id| wire::is_direct(id)).count() == 1,
        "the notices leave the phone in the table and add no other direct- id"
    );
    let after = exchange(&mut phone, json!({"id":"after-notices","method":"GET","path":"/m1/info"})).await;
    assert!(after["id"] == "after-notices" && after["status"] == 200, "the direct phone still answers after the relay's notices");
    drop(phone);
    fixture.done().await;
}

/// L501 "mobile direct: a direct frame over 1 MiB is refused", the relay connection's half: the seal takes 28 bytes of the 1 MiB frame;
/// a frame of exactly 1 MiB is read and the connection goes on; one byte over ends it, and the listener stays up.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mobile_direct_a_direct_frame_over_one_mib_is_refused() {
    let _lane = support::lane::enter().await;
    let fixture = Fixture::new("frame", &[]).await;
    fixture.apply(settings("ws://127.0.0.1:9", true)).await;
    let pairing = fixture.host.pairing_url();
    let mut phone = direct_phone(&fixture.endpoint(), &pairing).await;
    assert_eq!(exchange(&mut phone, auth(&pairing, "abcdefghijklmnopqrstuv")).await["type"], "auth_ok", "the phone is in");
    assert_eq!(cipher::MAXIMUM_PLAINTEXT + 28, wire::MAXIMUM_FRAME_BYTES, "the seal takes 28 bytes of the 1 MiB frame");
    let (head, tail) = (br#"{"type":"fixture-padding","pad":""#.to_vec(), br#""}"#.to_vec());
    let mut padded = vec![b'a'; cipher::MAXIMUM_PLAINTEXT];
    padded[..head.len()].copy_from_slice(&head);
    let end = padded.len() - tail.len();
    padded[end..].copy_from_slice(&tail);
    phone.send_bytes(&padded).await;
    let info = exchange(&mut phone, json!({"id":"after-full-frame","method":"GET","path":"/m1/info"})).await;
    assert_eq!(info["id"], "after-full-frame", "a frame of exactly 1 MiB is read and the connection goes on");
    // Bounded: the host refuses on the frame's length and drops the connection, so the rest of the write may fail or wait (a Mac's small
    // loopback buffers); either way the phone then reads the end.
    let _ = tokio::time::timeout(Duration::from_secs(20), phone.send_frame(vec![0u8; wire::MAXIMUM_FRAME_BYTES + 1])).await;
    let ended = tokio::time::timeout(Duration::from_secs(20), phone.ended()).await;
    assert!(ended.unwrap_or(false), "a frame one byte over 1 MiB ends the connection");
    until(Duration::from_secs(10), || fixture.host.connection_ids().is_empty()).await;
    assert!(fixture.path.status().listening(), "the listener stays up for the next phone");
    fixture.done().await;
}

/// L502 "mobile direct: a relay auth frame that is no JSON is answered malformed" (the relay path says why; only the direct path says
/// one word).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mobile_direct_a_relay_auth_frame_that_is_no_json_is_answered_malformed() {
    let _lane = support::lane::enter().await;
    let answer: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let answered = Arc::new(Notify::new());
    let pairing: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    let tries = Arc::new(AtomicUsize::new(0));
    let phone_script: fake_relay::PhoneScript = {
        let (answer, answered, tries) = (answer.clone(), answered.clone(), tries.clone());
        Arc::new(move |_, socket| {
            let (answer, answered) = (answer.clone(), answered.clone());
            tries.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                let mut phone = ScriptedPhone::new(socket);
                phone.hello().await;
                phone.send_bytes(b"not json {").await;
                let refused = phone.receive().await.unwrap();
                *answer.lock().unwrap() = Some(format!("{} {}", refused["type"].as_str().unwrap(), refused["reason"].as_str().unwrap()));
                answered.notify_one();
            })
        })
    };
    let relay =
        FakeRelay::start(Arc::new(|_, socket| Box::pin(fake_relay::notice_then_listen(socket, "fixture-relay-phone"))), phone_script).await;
    let fixture = Fixture::new("malformed", &[]).await;
    *pairing.lock().unwrap() = fixture.host.pairing_url();
    fixture.apply(settings(&relay.address, false)).await;
    tokio::time::timeout(Duration::from_secs(15), answered.notified()).await.expect("the relay phone's answer");
    assert_eq!(
        answer.lock().unwrap().as_deref(),
        Some("auth_error malformed"),
        "a relay phone's auth frame that is no JSON is answered malformed"
    );
    assert_eq!(tries.load(Ordering::SeqCst), 1);
    fixture.done().await;
}

/// Review L4 (with W8-C's reservation ids): a hand-back of a reservation the host does not hold is ignored, so it cannot drop a newer
/// one of the same address; an `accept` with an unknown reservation is dropped unread; the real reservation's hand-back frees its slot.
/// And review L3: without the checks' seam only Tailscale's addresses are admitted.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mobile_direct_a_reservation_is_given_back_exactly_once() {
    use mighty_remote::direct::listener::DirectReservation;
    let _lane = support::lane::enter().await;
    let fixture = Fixture::new("reservations", &[]).await;
    fixture.apply(settings("ws://127.0.0.1:9", true)).await;
    let sink = fixture.host.direct_sink();
    let peer: std::net::IpAddr = "100.101.102.103".parse().unwrap();
    let first = sink.admit(peer).expect("a first handshake is admitted");
    sink.refused(DirectReservation::new(first.id() + 1000));
    assert_eq!(fixture.host.direct_reservations(), 1, "an unknown hand-back changes nothing");
    let second = sink.admit(peer).expect("a second handshake from the address is admitted");
    assert!(sink.admit(peer).is_none(), "the address's two slots are still both held");
    // An accept with a reservation the host never gave: the socket is dropped unread and nothing joins the table.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let client = tokio::spawn(async move {
        let tcp = tokio::net::TcpStream::connect(address).await.unwrap();
        tokio_tungstenite::client_async(format!("ws://{address}/ws"), tcp).await.ok()
    });
    let (tcp, remote) = listener.accept().await.unwrap();
    let socket = tokio_tungstenite::accept_async(tcp).await.unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let ended = tokio::time::timeout(Duration::from_secs(5), sink.accept(DirectReservation::new(9_999), socket, remote, deadline)).await;
    assert!(ended.is_ok() && fixture.host.connection_ids().is_empty(), "an unknown reservation's socket is not run");
    let _ = client.await;
    assert_eq!(fixture.host.direct_reservations(), 2, "and the held ones stay");
    sink.refused(first);
    sink.refused(second);
    assert_eq!(fixture.host.direct_reservations(), 0, "each real hand-back frees its own slot");
    fixture.done().await;

    // Without the seam a peer outside Tailscale's ranges is refused by the host too.
    let directory = temp("tailnet-only");
    let identity = Arc::new(Identity::load_or_create(Arc::new(FileStore(directory.clone())), true).unwrap());
    let options = RelayOptions { watches_network: false, ..RelayOptions::default() };
    let platform = fake::platform(&directory, Os::Windows);
    let host = RelayHost::with_options(identity, MobileRemoteSettings::default(), Arc::new(Paths), platform, options);
    host.apply(settings("ws://127.0.0.1:9", true)).await.unwrap();
    let sink = host.direct_sink();
    assert!(sink.admit("127.0.0.1".parse().unwrap()).is_none(), "loopback is no Tailscale peer");
    assert!(sink.admit("100.101.102.103".parse().unwrap()).is_some(), "a Tailscale peer is");
    host.dispose().await;
    let _ = std::fs::remove_dir_all(&directory);
}

/// A route whose reply has an after-send (an app update's restart) that marks when it ran to its end.
struct AfterSend(Arc<std::sync::atomic::AtomicBool>);

impl RequestHandler for AfterSend {
    fn handle(
        &self,
        _: MobileDevice,
        _: String,
        _: String,
        _: Option<Box<serde_json::value::RawValue>>,
    ) -> BoxFuture<'static, MobileReply> {
        let done = self.0.clone();
        Box::pin(async move {
            let mut reply = MobileReply::new(200, r#"{"protocol":1}"#.into());
            reply.after_send = Some(Box::new(move || {
                Box::pin(async move {
                    tokio::time::sleep(Duration::from_millis(400)).await;
                    done.store(true, Ordering::SeqCst);
                })
            }));
            reply
        })
    }
}

/// Review M2: a reply's after-send runs to its end even when the phone goes right after the reply (C# awaits it whatever happens next).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mobile_direct_an_after_send_survives_the_phone_leaving() {
    let _lane = support::lane::enter().await;
    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let fixture = Fixture::with_handler("after-send", &[], Arc::new(AfterSend(done.clone()))).await;
    fixture.apply(settings("ws://127.0.0.1:9", true)).await;
    let pairing = fixture.host.pairing_url();
    let mut phone = direct_phone(&fixture.endpoint(), &pairing).await;
    assert_eq!(exchange(&mut phone, auth(&pairing, "abcdefghijklmnopqrstuv")).await["type"], "auth_ok");
    assert_eq!(exchange(&mut phone, json!({"id":"update","method":"POST","path":"/m1/app-update"})).await["status"], 200);
    drop(phone);
    until(Duration::from_secs(10), || fixture.host.connection_ids().is_empty()).await;
    assert!(!done.load(Ordering::SeqCst), "the connection ended before the after-send did");
    until(Duration::from_secs(5), || done.load(Ordering::SeqCst)).await;
    fixture.done().await;
}

/// Review L2: a phone that reads nothing holds its senders back: the host's queue for it is bounded, so a stream of notifications stops
/// being taken once the socket and the queue are full, instead of growing in memory.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mobile_direct_a_phone_that_reads_nothing_holds_its_senders_back() {
    let _lane = support::lane::enter().await;
    let fixture = Fixture::new("backpressure", &[]).await;
    fixture.apply(settings("ws://127.0.0.1:9", true)).await;
    let pairing = fixture.host.pairing_url();
    let mut phone = direct_phone(&fixture.endpoint(), &pairing).await;
    assert_eq!(exchange(&mut phone, auth(&pairing, "abcdefghijklmnopqrstuv")).await["type"], "auth_ok");
    // The phone stops reading. Each notification is about 64 KiB, so the socket's buffers fill after a few hundred.
    let scope = format!("session:{}", "s".repeat(64 * 1024));
    let mut taken = 0;
    let mut held = false;
    for revision in 0..20_000 {
        if tokio::time::timeout(Duration::from_millis(500), fixture.host.notify(&scope, revision)).await.is_err() {
            held = true;
            break;
        }
        taken += 1;
    }
    assert!(held, "the notifications were held back after {taken}");
    drop(phone);
    fixture.done().await;
}
