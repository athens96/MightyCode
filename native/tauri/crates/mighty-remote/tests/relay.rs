//! The relay's checks, ported by C# name (W8-A; MightyCode.Core.Tests `MobileRemoteVerification`, `MobileHostVerification`): the
//! shared cipher vectors, device trust and rotation, the encrypted round trip over the in-process relay, a disposed host, the link
//! policy, the redial after a wake and without a network, a half-dead control socket, and the device list. Every socket is loopback
//! port 0 (`support::fake_relay`); no check reaches a real relay.

mod support;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use mighty_core::locale::Locale;
use mighty_core::model::MobileRemoteSettings;
use mighty_platform::pairing::identity_file;
use mighty_platform::{Os, fake};
use mighty_remote::identity::{AuthRequest, Identity, is_new};
use mighty_remote::relay::cipher::{self, CipherError, RelayCipher};
use mighty_remote::relay::connection::{MobileDevice, MobileReply, RequestHandler};
use mighty_remote::relay::host::{self, RelayError, RelayHost, RelayOptions};
use mighty_remote::relay::policy::{self, LinkAction, LinkKind, NetworkPath, Pacing, Window};
use mighty_remote::relay::status::{self, MobileRelayStatus};
use serde_json::json;
use support::fake_relay::{self, FakeRelay, FileStore, ScriptedPhone};
use tokio::sync::Notify;

fn temp(name: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let path = std::env::temp_dir().join(format!("mighty-mobile-{name}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn auth(value: serde_json::Value) -> AuthRequest {
    AuthRequest::from_json(&value)
}

/// A pairing link's query value, unescaped.
fn query(link: &str, name: &str) -> String {
    let query = link.split_once('?').unwrap().1;
    let raw = query.split('&').find_map(|pair| pair.strip_prefix(&format!("{name}="))).unwrap();
    let bytes = raw.as_bytes();
    let mut out = vec![];
    let mut at = 0;
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

fn en() -> Locale {
    Locale::new("en")
}

struct Answer;

impl RequestHandler for Answer {
    fn handle(
        &self,
        _: MobileDevice,
        _: String,
        _: String,
        _: Option<Box<serde_json::value::RawValue>>,
    ) -> mighty_platform::BoxFuture<'static, MobileReply> {
        Box::pin(async { MobileReply::new(200, r#"{"protocol":1,"ok":true}"#.into()) })
    }
}

fn identity_in(folder: &Path) -> Arc<Identity> {
    Arc::new(Identity::load_or_create(Arc::new(FileStore(folder.to_path_buf())), true).unwrap())
}

fn relay_host(folder: &Path, options: RelayOptions) -> RelayHost {
    let platform = fake::platform(folder, Os::Windows);
    RelayHost::with_options(identity_in(folder), MobileRemoteSettings::default(), Arc::new(Answer), platform, options)
}

fn on(address: &str) -> MobileRemoteSettings {
    MobileRemoteSettings {
        enabled: true,
        relay_url: Some(address.into()),
        allow_legacy_phones: false,
        direct_enabled: false,
        ..MobileRemoteSettings::default()
    }
}

/// `Verification.Until`: polls `condition` every 20 ms for at most `limit`.
async fn until(limit: Duration, condition: impl Fn() -> bool) {
    let deadline = tokio::time::Instant::now() + limit;
    while !condition() {
        assert!(tokio::time::Instant::now() < deadline, "timed out waiting");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn recorder(host: &RelayHost) -> (Arc<Mutex<Vec<MobileRelayStatus>>>, host::StatusSubscription) {
    let seen = Arc::new(Mutex::new(vec![]));
    let record = seen.clone();
    let subscription = host.on_status(move |status| record.lock().unwrap().push(status.clone()));
    (seen, subscription)
}

/// L437 "Mobile remote SharedCipherVectors": the host's public key, both directions' frames byte for byte, a forged tag refused without
/// moving the counter, a replay refused, a disposed key never reused.
#[test]
fn mobile_remote_shared_cipher_vectors() {
    let text = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../contracts/relay-vectors.json")).unwrap();
    let root: serde_json::Value = serde_json::from_str(&text).unwrap();
    let b64 = base64::engine::general_purpose::STANDARD;
    let bytes = |key: &str| b64.decode(root[key].as_str().unwrap()).unwrap();
    let host_secret: [u8; 32] = bytes("hostSecretKeyB64").try_into().unwrap();
    assert_eq!(cipher::public_key(&host_secret).to_vec(), bytes("hostPublicKeyB64"), "host X25519 public key matches Mac/mobile vector");
    let host =
        RelayCipher::new(&host_secret, &bytes("clientPublicKeyB64"), &bytes("clientNonceB64"), &bytes("serverNonceB64"), true).unwrap();
    let client = RelayCipher::new(
        &bytes("clientSecretKeyB64"),
        &bytes("hostPublicKeyB64"),
        &bytes("clientNonceB64"),
        &bytes("serverNonceB64"),
        false,
    )
    .unwrap();
    for i in 0..2 {
        let c_plain = root["clientToHostPlaintexts"][i].as_str().unwrap().as_bytes();
        let c_frame = client.seal(c_plain).unwrap();
        assert_eq!(
            b64.encode(&c_frame),
            root["clientToHostFramesB64"][i].as_str().unwrap(),
            "client AEAD bytes match shared golden vector"
        );
        let mut tampered = c_frame.clone();
        *tampered.last_mut().unwrap() ^= 1;
        assert_eq!(host.open(&tampered), Err(CipherError::Forged), "forged tag accepted");
        assert_eq!(host.open(&c_frame).unwrap(), c_plain, "bad authentication does not advance receive counter");
        assert_eq!(host.open(&c_frame), Err(CipherError::Replayed), "replay accepted");
        let h_plain = root["hostToClientPlaintexts"][i].as_str().unwrap().as_bytes();
        let h_frame = host.seal(h_plain).unwrap();
        assert_eq!(b64.encode(&h_frame), root["hostToClientFramesB64"][i].as_str().unwrap(), "host AEAD bytes match shared golden vector");
        assert_eq!(client.open(&h_frame).unwrap(), h_plain, "phone opens host ciphertext");
    }
    host.dispose();
    assert_eq!(host.seal(&[]), Err(CipherError::Disposed), "disposed key reused");
    // The receiver's checks, in C#'s order: size, direction, the zero bytes.
    let other =
        RelayCipher::new(&host_secret, &bytes("clientPublicKeyB64"), &bytes("clientNonceB64"), &bytes("serverNonceB64"), true).unwrap();
    assert_eq!(other.open(&[1u8; 27]), Err(CipherError::Frame));
    assert_eq!(other.open(&vec![1u8; cipher::MAXIMUM_FRAME + 1]), Err(CipherError::Frame));
    let mut wrong = client.seal(b"x").unwrap();
    wrong[0] = cipher::HOST_TO_PHONE;
    assert_eq!(other.open(&wrong), Err(CipherError::Frame), "the host reads only the phone's direction");
    let mut nonzero = client.seal(b"x").unwrap();
    nonzero[2] = 1;
    assert_eq!(other.open(&nonzero), Err(CipherError::Frame));
    assert_eq!(RelayCipher::new(&host_secret, &[0u8; 32], &[0; 16], &[0; 16], true).err(), Some(CipherError::Agreement), "a low-order key");
    assert_eq!(RelayCipher::new(&host_secret, &[1u8; 31], &[0; 16], &[0; 16], true).err(), Some(CipherError::Lengths));
    assert_eq!(client.seal(&vec![0u8; cipher::MAXIMUM_PLAINTEXT + 1]), Err(CipherError::TooLarge));
}

/// L455 "Mobile remote DeviceTrustAndRotation" (with `NormalizeRelay` and `EffectiveRelay`).
#[test]
fn mobile_remote_device_trust_and_rotation() {
    let directory = temp("trust");
    let identity = identity_in(&directory);
    let key = query(&identity.pairing_url(host::DEFAULT_RELAY, "Fixture", &[]), "key");
    let id = "abcdefghijklmnopqrstuv";
    let decide = |identity: &Identity, value: serde_json::Value, legacy: bool| identity.authorize(&auth(value), legacy).unwrap();
    assert_eq!(
        decide(&identity, json!({"type":"auth","pairingKey":key,"clientId":"bad"}), true).error(),
        Some("malformed"),
        "malformed client id cannot downgrade to key-only"
    );
    assert_eq!(
        decide(&identity, json!({"type":"auth","pairingKey":key}), false).error(),
        Some("legacy-refused"),
        "legacy clients can be disabled"
    );
    let joined = decide(&identity, json!({"type":"auth","pairingKey":key,"clientId":id,"clientName":"Phone"}), false);
    assert!(joined.device_id() == Some(id) && joined.device_token().is_some(), "first pairing issues device token");
    let token = joined.device_token().unwrap().to_string();
    let file = std::fs::read_to_string(directory.join(identity_file::FILE)).unwrap();
    assert!(!file.contains(&token), "only device token hash is persisted");
    // The phone scanned the QR again: the key and the same id re-pair it, the row keeps its first-seen time and loses its push handle,
    // the new token works and the old one no longer does.
    let first_seen = identity.devices().into_iter().find(|d| d.id == id).unwrap().first_seen;
    assert!(
        identity.set_push(id, &"h".repeat(43), &mighty_remote::push::message::KINDS.map(String::from))
            == mighty_remote::identity::PushRegistrationResult::Saved
            && identity.push_devices().len() == 1,
        "paired phone registers a push handle"
    );
    let again = decide(&identity, json!({"type":"auth","pairingKey":key,"clientId":id,"clientName":"Galaxy"}), false);
    assert!(
        again.error().is_none() && again.device_id() == Some(id) && again.device_token().is_some_and(|t| t != token),
        "pairing again with the key issues the same row a new token"
    );
    let devices = identity.devices();
    assert!(
        devices.len() == 1 && devices[0].first_seen == first_seen && devices[0].name == "Galaxy",
        "re-pairing keeps one row with its first-seen time and the new name"
    );
    assert!(identity.push_devices().is_empty(), "re-pairing drops the push registration");
    assert_eq!(
        decide(&identity, json!({"type":"auth","clientId":id,"deviceToken":token}), false).error(),
        Some("device-revoked"),
        "the token from before re-pairing stops working"
    );
    assert_eq!(
        decide(&identity, json!({"type":"auth","pairingKey":"not-the-key-not-the-key-not-the-key-not-the","clientId":id}), false).error(),
        Some("pairing-key"),
        "re-pairing without the key is refused"
    );
    let token = again.device_token().unwrap().to_string();
    let reopened = identity_in(&directory);
    assert_eq!(
        decide(&reopened, json!({"type":"auth","clientId":id,"deviceToken":token}), false).device_id(),
        Some(id),
        "returning device authenticates after app restart"
    );
    reopened.rotate().unwrap();
    assert!(reopened.devices().is_empty(), "key rotation erases every device credential");
    assert_eq!(
        decide(&reopened, json!({"type":"auth","clientId":id,"deviceToken":token}), false).error(),
        Some("device-revoked"),
        "rotated device token is rejected"
    );
    assert_eq!(
        decide(&reopened, json!({"type":"auth","pairingKey":key,"clientId":id}), false).error(),
        Some("pairing-key"),
        "old QR key is rejected"
    );
    let fresh = query(&reopened.pairing_url(host::DEFAULT_RELAY, "Fixture", &[]), "key");
    for n in 0..8 {
        let client = format!("{n:a>22}");
        assert!(
            decide(&reopened, json!({"type":"auth","pairingKey":fresh,"clientId":client}), false).error().is_none(),
            "registration below rate limit succeeds"
        );
    }
    assert_eq!(
        decide(&reopened, json!({"type":"auth","pairingKey":fresh,"clientId":"zzzzzzzzzzzzzzzzzzzzzz"}), false).error(),
        Some("device-limit"),
        "new-device rate limit is enforced"
    );
    assert_eq!(
        host::normalize_relay("https://relay.example/path?token=x").as_deref(),
        Some("wss://relay.example"),
        "relay origin does not retain path/query"
    );
    assert_eq!(host::normalize_relay("wss://user:password@relay.example"), None, "relay URL credentials rejected");
    assert_eq!(host::DEFAULT_RELAY, "wss://mighty-relay.athens96.workers.dev", "default relay is the Cloudflare relay");
    assert!(
        host::effective_relay(Some(""), host::DEFAULT_RELAY).as_deref() == Some(host::DEFAULT_RELAY)
            && host::effective_relay(Some("not a url"), host::DEFAULT_RELAY).as_deref() == Some(host::DEFAULT_RELAY),
        "empty or invalid relay field uses the default"
    );
    assert_eq!(
        host::effective_relay(Some("relay.example"), host::DEFAULT_RELAY).as_deref(),
        Some("wss://relay.example"),
        "user relay field wins over the default"
    );
    assert_eq!(
        host::effective_relay(Some("wss://mightyclaude.duckdns.org"), "wss://default.example").as_deref(),
        Some("wss://default.example"),
        "retired default relay falls back to the default"
    );
    assert_eq!(
        host::effective_relay(Some("MightyClaude.DuckDNS.org"), "wss://default.example").as_deref(),
        Some("wss://default.example"),
        "retired default relay matches without case after normalizing"
    );
    assert_eq!(host::effective_relay(Some(""), ""), None, "no usable relay is null");
    // Verification mode: no identity is made without the right to write, and a loaded one writes nothing.
    let empty = temp("readonly");
    assert!(
        Identity::load_or_create(Arc::new(FileStore(empty.clone())), false).is_err() && !empty.join(identity_file::FILE).exists(),
        "nothing is created"
    );
    let read_only = Identity::load_or_create(Arc::new(FileStore(directory.clone())), false).unwrap();
    let before = std::fs::read(directory.join(identity_file::FILE)).unwrap();
    assert!(
        read_only.authorize(&auth(json!({"type":"auth","pairingKey":fresh,"clientId":"yyyyyyyyyyyyyyyyyyyyyy"})), false).is_err(),
        "a pairing that would write fails"
    );
    assert!(
        read_only.rotate().is_err() && std::fs::read(directory.join(identity_file::FILE)).unwrap() == before,
        "and the file is unchanged"
    );
    let _ = std::fs::remove_dir_all(&directory);
    let _ = std::fs::remove_dir_all(&empty);
}

/// L459 "Mobile remote EncryptedRelayRoundTrip": the control socket's notice opens a data socket, the phone pins the QR's key, pairs
/// over the encrypted relay and gets a correlated reply; a reconnect keeps the pairing; a live rotation clears the devices; a stale
/// revoke cannot rotate a newer key; dispose.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mobile_remote_encrypted_relay_round_trip() {
    let _lane = support::lane::enter().await;
    let directory = temp("roundtrip");
    let completed = Arc::new(Notify::new());
    let reconnected = Arc::new(Notify::new());
    let link: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let failure: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let control: fake_relay::ControlScript = {
        let reconnected = reconnected.clone();
        Arc::new(move |number, socket| {
            let reconnected = reconnected.clone();
            Box::pin(async move {
                if number == 1 {
                    fake_relay::notice_then_listen(socket, "fixture-connection").await;
                } else {
                    reconnected.notify_one();
                    fake_relay::listen(socket).await;
                }
            })
        })
    };
    let phone: fake_relay::PhoneScript = {
        let (completed, link, failure) = (completed.clone(), link.clone(), failure.clone());
        Arc::new(move |_, socket| {
            let (completed, link, failure) = (completed.clone(), link.clone(), failure.clone());
            Box::pin(async move {
                let pairing = link.lock().unwrap().clone().unwrap();
                let mut phone = ScriptedPhone::new(socket);
                let ready = phone.hello().await.clone();
                let pin = query(&pairing, "pk").replace('-', "+").replace('_', "/") + "=";
                if pin != ready["serverKey"].as_str().unwrap() {
                    *failure.lock().unwrap() = Some("relay host key matches QR pin".into());
                }
                let request = json!({"type":"auth","pairingKey":query(&pairing, "key"),"clientId":"abcdefghijklmnopqrstuv","clientName":"Fixture Phone"});
                phone.send(&request.to_string()).await;
                let answer = phone.receive().await.unwrap();
                if answer["type"] != "auth_ok" || answer["deviceToken"].as_str().is_none() {
                    *failure.lock().unwrap() = Some("phone authenticates over encrypted relay".into());
                }
                // Screen share is paused: a host asked to add it leaves it out of auth_ok.
                let screen = answer["capabilities"].as_array().unwrap().iter().any(|c| c == "screenShare");
                if screen != mighty_remote::screen::ENABLED {
                    *failure.lock().unwrap() = Some("auth_ok follows the screen-share switch".into());
                }
                phone.send(r#"{"id":"request-1","method":"GET","path":"/m1/info"}"#).await;
                let response = phone.receive_reply().await.unwrap();
                if response["id"] != "request-1" || response["status"] != 200 || response["body"]["ok"] != true {
                    *failure.lock().unwrap() = Some("encrypted request response is correlated".into());
                }
                completed.notify_one();
                phone.until_closed().await;
            })
        })
    };
    let relay = FakeRelay::start(control, phone).await;
    let options = RelayOptions {
        host_name: "Fixture".into(),
        watches_network: false,
        extra_capabilities: Some(Arc::new(|| vec!["screenShare".into()])),
        ..RelayOptions::default()
    };
    let host = relay_host(&directory, options);
    *link.lock().unwrap() = Some(host.pairing_url());
    host.apply(on(&relay.address)).await.unwrap();
    tokio::time::timeout(Duration::from_secs(15), completed.notified()).await.expect("the phone's round trip");
    assert_eq!(*failure.lock().unwrap(), None);
    assert_eq!(host.devices().len(), 1, "device registered through live WebSocket handshake");
    let pairing = host.pairing_url();
    let server_id = host.pairing_server_id();
    assert_eq!(host.pairing_key_for_display().as_str(), query(&pairing, "key"), "manual pairing key matches QR key");
    host.reconnect().await.unwrap();
    tokio::time::timeout(Duration::from_secs(15), reconnected.notified()).await.expect("the second control socket");
    assert!(
        host.devices().len() == 1 && host.pairing_url() == pairing && host.pairing_server_id() == server_id && relay.controls() == 2,
        "reconnect replaces relay socket while retaining pairing and trusted devices"
    );
    assert!(host.rotate(None).await.unwrap() && host.devices().is_empty(), "live key rotation clears device registry");
    let rotated = host.pairing_url();
    assert!(
        !host.rotate(Some("abcdefghijklmnopqrstuv".into())).await.unwrap() && host.pairing_url() == rotated,
        "stale device revoke cannot rotate a newer pairing key"
    );
    assert!(relay.refused().is_empty(), "every socket's query was the relay's: {:?}", relay.refused());
    host.dispose().await;
    let _ = std::fs::remove_dir_all(&directory);
}

/// L460 "Mobile relay shutdown rejects reconnect and rotation".
#[tokio::test]
async fn mobile_relay_shutdown_rejects_reconnect_and_rotation() {
    let directory = temp("disposed");
    let host = relay_host(&directory, RelayOptions { host_name: "Fixture".into(), watches_network: false, ..RelayOptions::default() });
    let pairing = host.pairing_url();
    host.dispose().await;
    host.dispose().await;
    assert_eq!(host.reconnect().await, Err(RelayError::Disposed), "disposed relay host must reject reconnect");
    assert_eq!(host.apply(MobileRemoteSettings::default()).await, Err(RelayError::Disposed), "and apply");
    assert_eq!(host.rotate(None).await, Err(RelayError::Disposed), "and key rotation");
    assert!(
        !host.status().connected && host.devices().is_empty() && host.pairing_url() == pairing,
        "shutdown retains identity and cannot reopen sockets"
    );
    let _ = std::fs::remove_dir_all(&directory);
}

/// L479 "Mobile host: the link policy decides when to redial". C#'s `RelayNetworkPath.Current()` reads .NET's adapters; here the path
/// is read from the network port (`NetworkPath::from_adapters`), checked to read the same twice and to leave loopback and link-local
/// addresses out.
#[test]
fn mobile_host_the_link_policy_decides_when_to_redial() {
    let wifi = NetworkPath::new(true, &["wifi"], &["192.168.0.1"]);
    let other_wifi = NetworkPath::new(true, &["wifi"], &["10.0.0.1"]);
    let vpn = NetworkPath::new(true, &["wifi", "vpn"], &["192.168.0.1"]);
    let none = NetworkPath::new(false, &[], &[]);
    let action = |old: Option<&NetworkPath>, next: Option<&NetworkPath>, forced: bool| policy::action(old, next, forced, false);
    assert!(
        action(None, Some(&wifi), false) == LinkAction::None && action(None, Some(&none), false) == LinkAction::Offline,
        "the first report is only a baseline"
    );
    assert!(
        action(Some(&wifi), Some(&other_wifi), false) == LinkAction::Reconnect
            && action(Some(&wifi), Some(&vpn), false) == LinkAction::Reconnect
            && action(Some(&none), Some(&wifi), false) == LinkAction::Reconnect
            && action(Some(&wifi), Some(&NetworkPath::new(true, &["wifi"], &["192.168.0.1"])), false) == LinkAction::None,
        "a different network redials, the same one does not"
    );
    assert!(
        action(Some(&wifi), Some(&none), false) == LinkAction::Offline
            && action(Some(&none), Some(&none), false) == LinkAction::None
            && action(Some(&wifi), Some(&none), true) == LinkAction::Offline,
        "no network is offline, not a retry — also after a wake"
    );
    assert!(
        action(Some(&wifi), None, true) == LinkAction::Reconnect
            && action(Some(&wifi), Some(&wifi), true) == LinkAction::Reconnect
            && action(Some(&wifi), None, false) == LinkAction::None,
        "a wake redials on the same network"
    );
    let now = 1_000_000u64;
    let mut dropped = Window::default();
    for path in [&wifi, &none, &wifi] {
        dropped.report(path.clone(), Some(&wifi), now);
    }
    let mut quiet = Window::default();
    quiet.report(wifi.clone(), Some(&wifi), now);
    let mut first = Window::default();
    for path in [&other_wifi, &wifi] {
        first.report(path.clone(), None, now);
    }
    assert!(
        dropped.action(Some(&wifi)) == LinkAction::Reconnect
            && quiet.action(Some(&wifi)) == LinkAction::None
            && first.action(None) == LinkAction::None,
        "the window remembers a drop that ended on the same path; before any baseline nothing counts as a change"
    );
    let mut woken = Window::default();
    assert!(woken.is_empty(), "a new window is empty");
    woken.force(now);
    assert!(
        !woken.is_empty() && woken.forced && woken.latest.is_none() && woken.action(Some(&wifi)) == LinkAction::Reconnect,
        "a wake opens the window by itself"
    );
    assert!(
        quiet.delay(now) == policy::PATH_SETTLE
            && quiet.delay(now + 4000) == Duration::from_secs(1)
            && quiet.delay(now + 6000) == Duration::ZERO,
        "a stream of reports still settles after five seconds"
    );
    let mut pacing = Pacing::default();
    let s = |seconds: u64| now + seconds * 1000;
    assert_eq!(pacing.wait(now), Duration::ZERO, "the first redial is not paced");
    pacing.record(now);
    assert_eq!(pacing.wait(s(1)), Duration::from_secs(4), "the next waits out the five-second gap");
    pacing.record(s(5));
    assert_eq!(pacing.wait(s(5)), Duration::from_secs(10), "the gap doubles while redials keep coming");
    pacing.record(s(15));
    assert_eq!(pacing.wait(s(15)), Duration::from_secs(20), "and doubles again");
    pacing.record(s(35));
    assert_eq!(pacing.wait(s(35)), policy::REDIAL_GAP_MAXIMUM, "up to thirty seconds");
    pacing.record(s(65));
    assert_eq!(pacing.wait(s(65)), policy::REDIAL_GAP_MAXIMUM, "where it stays");
    pacing.record(s(200));
    assert_eq!(pacing.wait(s(200)), policy::REDIAL_GAP, "calm for a while: back to the shortest gap");
    assert!(
        policy::KEEPALIVE_INTERVAL == Duration::from_secs(20)
            && policy::KEEPALIVE_TIMEOUT == Duration::from_secs(10)
            && policy::PATH_SETTLE == Duration::from_millis(1500),
        "the control socket is pinged every 20 s with 10 s to answer; a path settles in 1.5 s"
    );
    // The network port's path: sorted, the same twice, loopback and link-local left out, nothing routable offline.
    let adapters = mighty_platform::Adapters {
        adapters: vec![
            mighty_platform::Adapter { name: "wifi".into(), addresses: vec!["192.168.0.20".parse().unwrap(), "fe80::1".parse().unwrap()] },
            mighty_platform::Adapter { name: "loopback".into(), addresses: vec!["127.0.0.1".parse().unwrap(), "::1".parse().unwrap()] },
            mighty_platform::Adapter { name: "apipa".into(), addresses: vec!["169.254.1.2".parse().unwrap()] },
            mighty_platform::Adapter { name: "ethernet".into(), addresses: vec!["10.0.0.5".parse().unwrap()] },
        ],
        unreadable: 0,
    };
    let read = NetworkPath::from_adapters(&adapters);
    assert!(
        read == NetworkPath::from_adapters(&adapters)
            && read == NetworkPath::new(true, &["ethernet", "wifi"], &["10.0.0.5", "192.168.0.20"]),
        "{read:?}"
    );
    assert!(
        !NetworkPath::from_adapters(&mighty_platform::Adapters { adapters: adapters.adapters[1..3].to_vec(), unreadable: 0 }).satisfied
    );
    // Only adapters that can carry the socket out make the path: one with a default gateway, loopback and tunnels aside.
    let ip = |text: &str| -> std::net::IpAddr { text.parse().unwrap() };
    let wifi_gateways = [ip("192.168.0.1"), ip("fe80::1")];
    let docker = [std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED)];
    let teredo = [ip("2001::1")];
    let loopback = [ip("127.0.0.1")];
    let vpn_gateway = [ip("10.8.0.1")];
    let base: Vec<(&str, LinkKind, &[std::net::IpAddr])> = vec![
        ("wifi", LinkKind::Other, &wifi_gateways),
        ("hyper-v", LinkKind::Other, &[]),
        ("docker", LinkKind::Other, &docker),
        ("teredo", LinkKind::Tunnel, &teredo),
        ("loopback", LinkKind::Loopback, &loopback),
    ];
    let carried = NetworkPath::from(base.clone(), true);
    assert_eq!(
        carried,
        NetworkPath::new(true, &["wifi"], &["192.168.0.1", "fe80::1"]),
        "a switch with no gateway, a tunnel and loopback are no part of the path"
    );
    let mut wsl = base.clone();
    wsl.push(("wsl", LinkKind::Other, &[]));
    let mut with_vpn = base.clone();
    with_vpn.push(("vpn", LinkKind::Other, &vpn_gateway));
    assert!(
        NetworkPath::from(wsl, true) == carried && NetworkPath::from(with_vpn, true) != carried,
        "starting WSL or Docker is no new path; a VPN that takes the default route is"
    );
    assert!(
        !NetworkPath::from(base[1..].to_vec(), true).satisfied && !NetworkPath::from(base.clone(), false).satisfied,
        "no adapter with a gateway, or no network at all, is offline"
    );

    let locale = en();
    let detail = |status: &MobileRelayStatus| status::detail(&locale, status);
    let plain = |detail: &str, error: Option<&str>, retry: Option<u32>| MobileRelayStatus::new(false, 0, detail, error, retry);
    assert!(
        detail(&plain("disabled", None, None)) == locale.get("mobileRemote.status.off")
            && detail(&plain("connecting", None, None)) == locale.get("mobileRemote.status.connecting")
            && detail(&MobileRelayStatus::new(true, 1, "connected", None, None)) == locale.get("mobileRemote.status.scanQR")
            && detail(&plain("offline", None, None)) == locale.get("settings.mobileRemote.detail.networkOffline"),
        "off, connecting, connected and offline each have their line"
    );
    let retry = locale.get_with("mobileRemote.status.retryIn", &[("seconds", "8")]);
    assert!(
        detail(&plain("reconnecting", None, Some(8))) == locale.get("mobileRemote.status.disconnected") + " " + &retry
            && detail(&plain("reconnecting", Some("host-id-taken"), Some(8)))
                == locale.get("mobileRemote.error.hostIdTaken") + " · " + &retry,
        "a lost link says when it dials again, after why the relay refused it"
    );
    let reasons: Vec<Option<String>> =
        ["rejected", "closed", "unresponsive", "connect-failed"].iter().map(|r| status::reason(&locale, Some(r))).collect();
    assert!(
        reasons
            == [
                "mobileRemote.error.relayRejected",
                "mobileRemote.error.relayClosed",
                "settings.mobileRemote.detail.relayUnresponsive",
                "mobileRemote.error.relayConnectFailed"
            ]
            .map(|key| Some(locale.get(key)))
            && status::reason(&locale, None).is_none(),
        "every reason has its words"
    );
    assert!(
        status::close_reason(Some(4400), true) == "rejected"
            && status::close_reason(Some(4409), true) == "host-id-taken"
            && status::close_reason(Some(1000), true) == "closed"
    );
}

/// L480 "Mobile host: the link redials after a wake and waits out no network".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mobile_host_the_link_redials_after_a_wake_and_waits_out_no_network() {
    let _lane = support::lane::enter().await;
    let directory = temp("wake");
    let relay = FakeRelay::start(Arc::new(|_, socket| Box::pin(fake_relay::listen(socket))), Arc::new(|_, _| Box::pin(async {}))).await;
    let host = relay_host(
        &directory,
        RelayOptions {
            host_name: "Fixture".into(),
            watches_network: false,
            path_settle: Duration::from_millis(60),
            ..RelayOptions::default()
        },
    );
    let (seen, _subscription) = recorder(&host);
    host.reconnect_soon();
    host.network_path_changed(NetworkPath::new(false, &[], &[]));
    host.apply(on(&relay.address)).await.unwrap();
    until(Duration::from_secs(10), || host.status().connected && relay.controls() == 1).await;
    assert!(
        host.status().detail == "connected" && seen.lock().unwrap().iter().any(|s| s.detail == "connecting"),
        "the host says it is connecting, then connected; nothing reported while it was off is acted on"
    );
    // A wake dials again on the same network, once the path has settled.
    host.reconnect_soon();
    host.reconnect_soon();
    until(Duration::from_secs(10), || relay.controls() == 2 && host.status().connected).await;
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert_eq!(relay.controls(), 2, "two wakes within the settle are one redial");
    // No network: the host stops dialling and says so.
    host.network_path_changed(NetworkPath::new(false, &[], &[]));
    until(Duration::from_secs(10), || host.status().detail == "offline").await;
    tokio::time::sleep(Duration::from_millis(1300)).await;
    assert!(
        !host.status().connected
            && relay.controls() == 2
            && status::detail(&en(), &host.status()) == en().get("settings.mobileRemote.detail.networkOffline"),
        "without a network nothing is dialled"
    );
    // The network returns: dialled at once.
    host.network_path_changed(NetworkPath::new(true, &["wifi"], &["192.168.0.1"]));
    until(Duration::from_secs(10), || relay.controls() == 3 && host.status().connected).await;
    // The same network again is no reason to redial.
    host.network_path_changed(NetworkPath::new(true, &["wifi"], &["192.168.0.1"]));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(relay.controls(), 3, "the same path redials nothing");
    host.apply(MobileRemoteSettings { enabled: false, ..on(&relay.address) }).await.unwrap();
    host.reconnect_soon();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(relay.controls() == 3 && host.status().detail == "disabled", "a host that was turned off is not woken");
    host.dispose().await;
    let _ = std::fs::remove_dir_all(&directory);
}

/// L481 "Mobile host: a half-dead control socket is replaced", and a relay nobody listens on retried after 1 s, then 2 s.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mobile_host_a_half_dead_control_socket_is_replaced() {
    let _lane = support::lane::enter().await;
    let directory = temp("halfdead");
    // The first control socket is accepted and then never read, so its pings go unanswered.
    let relay = FakeRelay::start(
        Arc::new(
            |number, socket| if number == 1 { Box::pin(fake_relay::never_read(socket)) } else { Box::pin(fake_relay::listen(socket)) },
        ),
        Arc::new(|_, _| Box::pin(async {})),
    )
    .await;
    let host = relay_host(
        &directory,
        RelayOptions {
            host_name: "Fixture".into(),
            watches_network: false,
            keepalive_interval: Duration::from_millis(300),
            keepalive_timeout: Duration::from_millis(300),
            ..RelayOptions::default()
        },
    );
    let (seen, _subscription) = recorder(&host);
    host.apply(on(&relay.address)).await.unwrap();
    until(Duration::from_secs(15), || relay.controls() == 2 && host.status().connected).await;
    let lost = seen.lock().unwrap().iter().find(|s| s.detail == "reconnecting").cloned().unwrap();
    assert!(
        !lost.connected && lost.error.as_deref() == Some("unresponsive") && lost.retry_seconds == Some(1),
        "an unanswered ping closes the socket, and the host says the relay did not answer and when it dials again: {lost:?}"
    );
    assert_eq!(
        status::detail(&en(), &lost),
        en().get("settings.mobileRemote.detail.relayUnresponsive")
            + " · "
            + &en().get_with("mobileRemote.status.retryIn", &[("seconds", "1")]),
        "in the Mac's words"
    );
    host.dispose().await;
    // Nobody listening at all: the relay could not be reached, and the wait doubles.
    let alone_folder = temp("alone");
    let alone = relay_host(&alone_folder, RelayOptions { host_name: "Fixture".into(), watches_network: false, ..RelayOptions::default() });
    let (refused, _alone_subscription) = recorder(&alone);
    alone.apply(on("ws://127.0.0.1:1")).await.unwrap();
    until(Duration::from_secs(20), || refused.lock().unwrap().iter().filter(|s| s.detail == "reconnecting").count() >= 2).await;
    let first_two: Vec<(Option<String>, Option<u32>)> =
        refused.lock().unwrap().iter().filter(|s| s.detail == "reconnecting").take(2).map(|s| (s.error.clone(), s.retry_seconds)).collect();
    assert_eq!(
        first_two,
        vec![(Some("connect-failed".to_string()), Some(1)), (Some("connect-failed".to_string()), Some(2))],
        "a relay that cannot be reached is retried after 1 s, then 2 s"
    );
    alone.dispose().await;
    let _ = std::fs::remove_dir_all(&directory);
    let _ = std::fs::remove_dir_all(&alone_folder);
}

/// L482 "Mobile host: the device list marks arrivals and names the legacy row".
#[test]
fn mobile_host_the_device_list_marks_arrivals_and_names_the_legacy_row() {
    let directory = temp("devices");
    let identity = identity_in(&directory);
    let key = query(&identity.pairing_url(host::DEFAULT_RELAY, "Fixture", &[]), "key");
    assert!(
        identity.authorize(&auth(json!({"type":"auth","pairingKey":key})), true).unwrap().device_id() == Some("legacy")
            && identity
                .authorize(&auth(json!({"type":"auth","pairingKey":key,"clientId":"abcdefghijklmnopqrstuv"})), true)
                .unwrap()
                .device_id()
                .is_some(),
        "an old app and a phone pair"
    );
    let devices = identity.devices();
    let legacy = devices.iter().find(|d| d.id == "legacy").unwrap();
    let phone = devices.iter().find(|d| d.id != "legacy").unwrap();
    assert!(
        legacy.legacy && legacy.is_new && !legacy.connected && legacy.name == en().get("mobileRemote.device.legacyName"),
        "the row that stands for every app without a token is named in the user's language: {}",
        legacy.name
    );
    assert!(
        !phone.legacy && phone.is_new && phone.name == en().get("mobileRemote.device.defaultName"),
        "a phone that gave no name gets the default one: {}",
        phone.name
    );
    let now = mighty_core::time::DateTimeOffset::now();
    let now_ms = mighty_core::time::clock_now().timestamp_millis();
    let hours = |h: i64| mighty_core::time::DateTimeOffset::from_utc_ticks(now.utc_ticks() + h * 36_000_000_000).to_o();
    assert!(
        is_new(&hours(-23), now_ms)
            && !is_new(&hours(-25), now_ms)
            && !is_new(&mighty_core::time::DateTimeOffset::from_utc_ticks(now.utc_ticks() + 3_000_000_000).to_o(), now_ms)
            && !is_new("not a date", now_ms),
        "an arrival is marked for a day"
    );
    let _ = std::fs::remove_dir_all(&directory);
}

/// A relay socket whose query the relay refuses is closed 4400 or 4401 (`relay/src/protocol.ts`), as the host's own queries never are.
#[tokio::test]
async fn the_fake_relay_refuses_a_bad_query_as_the_relay_does() {
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    let relay = FakeRelay::start(
        Arc::new(move |_, socket| {
            counted.fetch_add(1, Ordering::SeqCst);
            Box::pin(fake_relay::listen(socket))
        }),
        Arc::new(|_, _| Box::pin(async {})),
    )
    .await;
    let proxy = fake::FakeProxy::default();
    for (url, code) in [
        (format!("{}/ws?serverId=abc&role=server&v=2", relay.address), 4400u16),
        (format!("{}/ws?serverId=abc&role=server&v=1&hostToken={}", relay.address, "a".repeat(64)), 4401),
    ] {
        let mut socket = host::dial(&url, &proxy).await.unwrap();
        use futures_util::StreamExt;
        let closed = loop {
            match socket.next().await {
                Some(Ok(tokio_tungstenite::tungstenite::Message::Close(frame))) => break frame.map(|f| u16::from(f.code)),
                Some(Ok(_)) => continue,
                _ => break None,
            }
        };
        assert_eq!(closed, Some(code));
    }
    assert_eq!(relay.refused(), vec![4400, 4401]);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

/// Review M1: the network port calls back on the OS's own threads, which have no runtime: a wake reported from a plain thread still
/// redials once the path settles, and an address change that leaves no network still takes the link offline.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mobile_host_a_wake_from_an_os_thread_still_redials() {
    let _lane = support::lane::enter().await;
    let directory = temp("os-thread");
    let relay = FakeRelay::start(Arc::new(|_, socket| Box::pin(fake_relay::listen(socket))), Arc::new(|_, _| Box::pin(async {}))).await;
    let network = fake::FakeNetwork::default();
    let wifi = mighty_platform::Adapters {
        adapters: vec![mighty_platform::Adapter { name: "wifi".into(), addresses: vec!["192.168.0.20".parse().unwrap()] }],
        unreadable: 0,
    };
    network.set_adapters(wifi);
    let mut platform = fake::platform(&directory, Os::Windows);
    platform.network = Arc::new(network.clone());
    let options = RelayOptions { host_name: "Fixture".into(), path_settle: Duration::from_millis(60), ..RelayOptions::default() };
    let host = RelayHost::with_options(identity_in(&directory), MobileRemoteSettings::default(), Arc::new(Answer), platform, options);
    host.apply(on(&relay.address)).await.unwrap();
    until(Duration::from_secs(10), || host.status().connected && relay.controls() == 1).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let resume = network.clone();
    std::thread::spawn(move || resume.trigger_resume()).join().unwrap();
    until(Duration::from_secs(10), || relay.controls() == 2 && host.status().connected).await;
    network.set_adapters(mighty_platform::Adapters::default());
    let change = network.clone();
    std::thread::spawn(move || change.trigger_change()).join().unwrap();
    until(Duration::from_secs(10), || host.status().detail == "offline").await;
    host.dispose().await;
    let _ = std::fs::remove_dir_all(&directory);
}
