//! Screen share's checks (W8-F), ported 1:1 by name from C#'s ScreenShareVerification.cs (L514-L519), ScreenReferenceSceneVerification.cs
//! (L520) and MobileHostVerification.cs's screen viewing (L484), over a fake platform (C#'s `Platform` and `Screen`). C# polls with real
//! 250 ms delays; here the clock is tokio's paused one (the hub's watcher sleeps on it) and, where C# injects one, the hub's own clock.
//! Nothing here captures, injects or opens a transport: the platform is a recorder.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use chrono::{DateTime, TimeDelta, Utc};
use mighty_platform::{BoxFuture, PlatformError, Result};
use mighty_remote::screen::clipboard::{self, ScreenClipboard};
use mighty_remote::screen::hub::{ConfirmKey, DeviceSend, ScreenSafety, ScreenShareHub, ScreenSharePlatform, TrustedDevice};
use mighty_remote::screen::policy::{self, ScreenDeviceGrant, ScreenDisplay, ScreenLiveSession, ScreenRegion};
use mighty_remote::screen::wire::{self, ScreenIceServer};
use mighty_remote::screen::{ENABLED, scene};
use p256::ecdsa::signature::Signer as _;
use p256::ecdsa::{Signature, SigningKey};
use serde_json::{Value, json};

const PHONE: &str = "abcdefghijklmnopqrstuv";

/// C#'s fake platforms in one: `Platform` (ScreenShareVerification.cs:11-32) and `Screen` (MobileHostVerification.cs:658-684), whose
/// `vanish` displays go away just as capture moves to them, as undocking takes one monitor after another.
struct Fake {
    attached: Mutex<Vec<ScreenDisplay>>,
    safety: Mutex<ScreenSafety>,
    started: Mutex<Vec<String>>,
    stopped: Mutex<Vec<String>>,
    inputs: Mutex<Vec<String>>,
    released: Mutex<Vec<String>>,
    switched: Mutex<Vec<i32>>,
    clipboard: Mutex<String>,
    halts: Mutex<usize>,
    vanish: Mutex<usize>,
    fail_switch: AtomicBool,
    /// `safety()` panics this many more times (a probe that breaks).
    panic_safety: Mutex<usize>,
}

impl Fake {
    fn new() -> Arc<Fake> {
        Arc::new(Fake {
            attached: Mutex::new(vec![
                ScreenDisplay::new(1, 1920, 1080, true),
                ScreenDisplay { left: -1280, ..ScreenDisplay::new(2, 1280, 1024, false) },
            ]),
            safety: Mutex::new(calm()),
            started: Mutex::default(),
            stopped: Mutex::default(),
            inputs: Mutex::default(),
            released: Mutex::default(),
            switched: Mutex::default(),
            clipboard: Mutex::default(),
            halts: Mutex::new(0),
            vanish: Mutex::new(0),
            fail_switch: AtomicBool::new(false),
            panic_safety: Mutex::new(0),
        })
    }

    fn set_safety(&self, change: impl FnOnce(&mut ScreenSafety)) {
        change(&mut self.safety.lock().unwrap());
    }

    fn stopped(&self, id: &str) -> bool {
        self.stopped.lock().unwrap().iter().any(|s| s == id)
    }

    fn halts(&self) -> usize {
        *self.halts.lock().unwrap()
    }

    fn switched(&self) -> Vec<i32> {
        self.switched.lock().unwrap().clone()
    }
}

/// `new ScreenSafety(false, false, UtcNow.AddMinutes(-1))`.
fn calm() -> ScreenSafety {
    ScreenSafety { locked: false, secure_input: false, last_local_input: Utc::now() - TimeDelta::minutes(1) }
}

fn done<'a, T: Send + 'a>(value: T) -> BoxFuture<'a, Result<T>> {
    Box::pin(async move { Ok(value) })
}

impl ScreenSharePlatform for Fake {
    fn available(&self) -> bool {
        true
    }
    fn displays(&self) -> Vec<ScreenDisplay> {
        self.attached.lock().unwrap().clone()
    }
    fn safety(&self) -> ScreenSafety {
        let panics = {
            let mut left = self.panic_safety.lock().unwrap();
            let panics = *left > 0;
            *left = left.saturating_sub(1);
            panics
        };
        assert!(!panics, "the fixture's safety probe broke");
        *self.safety.lock().unwrap()
    }
    fn start_peer(&self, session: ScreenLiveSession, _ice: Vec<ScreenIceServer>) -> BoxFuture<'_, Result<()>> {
        self.started.lock().unwrap().push(session.session_id);
        done(())
    }
    fn receive_signal(&self, _session_id: String, _signal: Value) -> BoxFuture<'_, Result<()>> {
        done(())
    }
    fn stop_peer(&self, session_id: String) -> BoxFuture<'_, Result<()>> {
        self.stopped.lock().unwrap().push(session_id);
        done(())
    }
    fn halt(&self) -> Result<()> {
        *self.halts.lock().unwrap() += 1;
        Ok(())
    }
    fn set_display(&self, display_id: i32) -> BoxFuture<'_, Result<bool>> {
        if self.fail_switch.load(Ordering::SeqCst) {
            return Box::pin(async { Err(PlatformError::Io(std::io::Error::other("capture did not start"))) });
        }
        let mut vanish = self.vanish.lock().unwrap();
        if *vanish > 0 {
            *vanish -= 1;
            self.attached.lock().unwrap().retain(|d| d.display_id != display_id);
            return done(false);
        }
        self.switched.lock().unwrap().push(display_id);
        done(true)
    }
    fn set_region(&self, _region: ScreenRegion) {}
    fn inject(&self, _session_id: String, input: Value) -> BoxFuture<'_, Result<()>> {
        self.inputs.lock().unwrap().push(input["t"].as_str().unwrap_or_default().to_string());
        done(())
    }
    fn release_buttons(&self, session_id: String) -> BoxFuture<'_, Result<()>> {
        self.released.lock().unwrap().push(session_id);
        done(())
    }
    fn show_marker(&self, _session_id: String, _input: Value) -> BoxFuture<'_, Result<bool>> {
        done(true)
    }
    fn read_clipboard(&self) -> BoxFuture<'_, Result<Option<String>>> {
        done(Some(self.clipboard.lock().unwrap().clone()))
    }
    fn write_clipboard(&self, text: String) -> BoxFuture<'_, Result<()>> {
        *self.clipboard.lock().unwrap() = text;
        done(())
    }
    fn send_data(&self, _session_id: String, _message: String) -> BoxFuture<'_, Result<()>> {
        done(())
    }
}

/// `Directory.CreateTempSubdirectory("mighty-screen-")`.
fn temp() -> PathBuf {
    let directory = std::env::temp_dir().join(format!("mighty-screen-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&directory).unwrap();
    directory
}

fn sends_nothing() -> DeviceSend {
    Arc::new(|_, _| done(()))
}

fn confirms(answer: bool) -> ConfirmKey {
    Arc::new(move |_, _| Box::pin(async move { answer }))
}

fn trusts_all() -> TrustedDevice {
    Arc::new(|_| true)
}

fn hub(
    directory: &std::path::Path,
    platform: &Arc<Fake>,
    send: DeviceSend,
    confirm: ConfirmKey,
    trusted: TrustedDevice,
) -> Arc<ScreenShareHub> {
    Arc::new(ScreenShareHub::new(directory, platform.clone(), send, confirm, trusted, None).unwrap())
}

fn key() -> SigningKey {
    loop {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).unwrap();
        if let Ok(key) = SigningKey::from_slice(&bytes) {
            return key;
        }
    }
}

/// `Public(key)`: `04 ‖ X ‖ Y`.
fn public(key: &SigningKey) -> Vec<u8> {
    key.verifying_key().to_encoded_point(false).as_bytes().to_vec()
}

/// `SignData(…, SHA256, Rfc3279DerSequence)`.
fn sign(key: &SigningKey, data: &[u8]) -> Vec<u8> {
    let signature: Signature = key.sign(data);
    signature.to_der().as_bytes().to_vec()
}

fn body(reply: &mighty_remote::relay::connection::MobileReply) -> Value {
    serde_json::from_str(&reply.body).unwrap()
}

async fn route(
    hub: &ScreenShareHub,
    method: &str,
    leaf: &str,
    body: Option<Value>,
    device: &str,
) -> mighty_remote::relay::connection::MobileReply {
    let body = body.map(|b| b.to_string());
    hub.route(method, leaf, body.as_deref(), device).await.unwrap()
}

async fn status(hub: &ScreenShareHub, method: &str, leaf: &str, body: Option<Value>, device: &str) -> u16 {
    route(hub, method, leaf, body, device).await.status
}

fn sessions_request(mode: &str, display: i32, signature: Option<&str>) -> Value {
    let mut request = json!({ "mode": mode, "network": "wifi", "displayId": display, "decodes": ["H264"] });
    if let Some(signature) = signature {
        request["controlSignatureB64"] = json!(signature);
    }
    request
}

/// ScreenShareVerification's `Start`: a session accepted, its peer started once the reply went, connected.
async fn start(hub: &ScreenShareHub, phone: &str, mode: &str, signature: Option<&str>) -> String {
    let mut reply = route(hub, "POST", "sessions", Some(sessions_request(mode, 1, signature)), phone).await;
    assert!(reply.status == 200, "authorized session accepted");
    let id = body(&reply)["sessionId"].as_str().unwrap().to_string();
    (reply.after_send.take().unwrap())().await;
    hub.peer_connected(&id);
    id
}

/// Enrolls `key` for `PHONE` and signs a fresh challenge with it.
async fn enrolled_signature(hub: &ScreenShareHub, key: &SigningKey) -> String {
    let enrolled = route(hub, "POST", "control-key", Some(json!({ "publicKeyB64": STANDARD.encode(public(key)) })), PHONE).await;
    assert_eq!(enrolled.status, 200);
    let state = body(&route(hub, "GET", "state", None, PHONE).await);
    let challenge = STANDARD.decode(state["screenShare"]["controlChallengeB64"].as_str().unwrap()).unwrap();
    STANDARD.encode(sign(key, &challenge))
}

fn text_input(text: &str) -> String {
    json!({ "t": "text", "text": text }).to_string()
}

/// "Screen sharing policy and typed wire boundaries" (L514).
#[test]
fn screen_sharing_policy_and_typed_wire_boundaries() {
    assert!(!policy::grant_allows(Some(&ScreenDeviceGrant::new(PHONE)), "view"), "fresh phone has no screen grant");
    assert!(policy::grant_allows(Some(&ScreenDeviceGrant::with(PHONE, true, "control")), "view"), "control includes viewing");
    assert!(!policy::grant_allows(Some(&ScreenDeviceGrant::with(PHONE, true, "view")), "control"), "view does not include input");
    assert!(wire::parse(r#"{"t":"tap","displayId":1,"x":1e999,"y":0,"button":"left"}"#).is_none(), "non-finite coordinate rejected");
    assert!(wire::parse(&json!({ "t": "key", "combo": "cmd+cmd+c" }).to_string()).is_none(), "duplicate modifiers rejected");
    assert!(wire::parse(&json!({ "t": "key", "combo": "win+r" }).to_string()).is_none(), "OS keycodes and unknown modifiers rejected");
    assert!(wire::parse(&text_input(&"한".repeat(1366))).is_none(), "text limit measured in UTF-8 bytes");
    assert!(wire::parse(&text_input("한글 😀")).is_some(), "committed Unicode text accepted");
    assert!(
        ScreenRegion::parse(&json!({ "x": -0.1, "y": 0.9, "width": 0.5, "height": 0.2 }))
            == Some(ScreenRegion { x: 0.0, y: 0.9, width: 0.4, height: 0.09999999999999998 }),
        "zoom clips to display bounds"
    );
    let key = key();
    let public_key = public(&key);
    let mut challenge = [0u8; 40];
    getrandom::fill(&mut challenge).unwrap();
    let mut signature = sign(&key, &challenge);
    assert!(policy::verify(&challenge, &signature, &public_key), "P-256 DER matches mobile Secure Enclave/Keystore protocol");
    *signature.last_mut().unwrap() ^= 1;
    assert!(!policy::verify(&challenge, &signature, &public_key), "altered signature refused");
    assert!(!policy::valid_key(&[0u8; 65]), "invalid curve point refused");
}

/// "Screen sharing clipboard bounds and replay" (L515).
#[test]
fn screen_sharing_clipboard_bounds_and_replay() {
    let now = Utc::now();
    let text = "한글 👨‍👩‍👧‍👦\n".repeat(4000);
    let mut receiver = ScreenClipboard::default();
    let mut decoded = None;
    for frame in ScreenClipboard::encode(Some(&text)) {
        let mut node: Value = serde_json::from_str(&frame).unwrap();
        node["dir"] = json!("to-mac");
        decoded = receiver.receive(&node, now);
    }
    assert!(decoded.as_deref() == Some(text.as_str()), "bounded zstd clipboard round trips full Unicode");
    let raw = json!({ "t": "clipboard", "dir": "to-mac", "id": "a", "enc": "raw", "bytes": 2, "seq": 0, "total": 2, "data": "YQ==" });
    let with = |changes: Value| {
        let mut frame = raw.clone();
        for (name, value) in changes.as_object().unwrap() {
            frame[name] = value.clone();
        }
        frame
    };
    assert!(receiver.receive(&raw, now).is_none(), "partial clipboard is not published");
    assert!(
        receiver.receive(&with(json!({ "seq": 1, "data": "Yg==" })), now + TimeDelta::seconds(31)).is_none(),
        "expired assembly refused"
    );
    assert!(receiver.receive(&raw, now).is_none(), "new transfer starts");
    assert!(receiver.receive(&with(json!({ "seq": 1, "bytes": 1 })), now).is_none(), "metadata change invalidates assembly");
    assert!(receiver.receive(&with(json!({ "seq": 1, "data": "Yg==" })), now).is_none(), "invalidated transfer cannot resume");
    let mut compressed: Value = serde_json::from_str(&ScreenClipboard::encode(Some(&"x".repeat(50000)))[0]).unwrap();
    compressed["dir"] = json!("to-mac");
    compressed["bytes"] = json!(1);
    assert!(receiver.receive(&compressed, now).is_none(), "zstd expansion cannot exceed declared output bound");
    let concealed: Value = serde_json::from_str(&ScreenClipboard::encode(None)[0]).unwrap();
    assert!(concealed["concealed"] == json!(true), "concealed source emits no plaintext");
    assert!(ScreenClipboard::encode(Some(&"x".repeat(clipboard::MAXIMUM_BYTES + 1))).is_empty(), "oversize clipboard never leaves host");
}

/// "Screen sharing enrollment challenge and response ordering" (L516).
#[tokio::test(start_paused = true)]
async fn screen_sharing_enrollment_challenge_and_response_ordering() {
    let directory = temp();
    let platform = Fake::new();
    let hub = hub(&directory, &platform, sends_nothing(), confirms(true), Arc::new(|p| p == PHONE));
    let view = json!({ "mode": "view", "network": "wifi", "displayId": 1 });
    assert!(status(&hub, "POST", "sessions", Some(view), PHONE).await == 403, "pairing alone never authorizes pixels");
    hub.set_grant(PHONE, true, "control").await.unwrap();
    let key = key();
    let enrolled = status(&hub, "POST", "control-key", Some(json!({ "publicKeyB64": STANDARD.encode(public(&key)) })), PHONE).await;
    assert!(enrolled == 200, "host confirms new control key");
    let other = self::key();
    let replaced = status(&hub, "POST", "control-key", Some(json!({ "publicKeyB64": STANDARD.encode(public(&other)) })), PHONE).await;
    assert!(replaced == 409, "phone cannot silently replace enrolled identity");
    let state = body(&route(&hub, "GET", "state", None, PHONE).await);
    let challenge = STANDARD.decode(state["screenShare"]["controlChallengeB64"].as_str().unwrap()).unwrap();
    let signature = STANDARD.encode(sign(&key, &challenge));
    let request = json!({ "mode": "control", "network": "wifi", "displayId": 1, "controlSignatureB64": signature });
    let mut reply = route(&hub, "POST", "sessions", Some(request.clone()), PHONE).await;
    assert!(reply.status == 200 && platform.started.lock().unwrap().is_empty(), "peer offer waits until m1 response has been sent");
    (reply.after_send.take().unwrap())().await;
    assert!(platform.started.lock().unwrap().len() == 1, "response completion starts peer exactly once");
    hub.kill_all("kill-switch").await;
    assert!(status(&hub, "POST", "sessions", Some(request), PHONE).await == 403, "consumed biometric challenge cannot be replayed");
    hub.remove_control_key(PHONE).await.unwrap();
    let grants = hub.grants();
    assert!(grants.len() == 1 && grants[0].control_key_public.is_none(), "remove key preserves grant but requires local enrollment again");
    hub.dispose().await;
    std::fs::remove_dir_all(&directory).unwrap();
}

/// "Screen sharing revocation and input safety" (L517).
#[tokio::test(start_paused = true)]
async fn screen_sharing_revocation_and_input_safety() {
    let directory = temp();
    let platform = Fake::new();
    let trusted = Arc::new(AtomicBool::new(true));
    let trust = trusted.clone();
    // C# compares with the wall clock; here the hub's clock stands still, so the two-second pause is decided by the test alone.
    let at = Utc::now();
    platform.set_safety(|s| s.last_local_input = at - TimeDelta::minutes(1));
    let trust: TrustedDevice = Arc::new(move |_| trust.load(Ordering::SeqCst));
    let hub = Arc::new(
        ScreenShareHub::new(&directory, platform.clone(), sends_nothing(), confirms(true), trust, Some(Arc::new(move || at))).unwrap(),
    );
    hub.set_grant(PHONE, true, "control").await.unwrap();
    let signature = enrolled_signature(&hub, &key()).await;
    let id = start(&hub, PHONE, "control", Some(&signature)).await;
    let input = text_input("한글");
    let inputs = || platform.inputs.lock().unwrap().len();
    hub.input(&id, &input).await;
    assert!(inputs() == 1, "approved control delivers input");
    platform.set_safety(|s| s.last_local_input = at - TimeDelta::milliseconds(1999));
    hub.input(&id, &input).await;
    assert!(inputs() == 1, "physical input pauses remote control");
    hub.input(&id, &json!({ "t": "drag", "phase": "end", "displayId": 1, "x": 0.1, "y": 0.1 }).to_string()).await;
    assert!(platform.released.lock().unwrap().contains(&id), "drag release is honored even during a safety pause");
    platform.set_safety(|s| s.last_local_input = at - TimeDelta::seconds(2));
    trusted.store(false, Ordering::SeqCst);
    hub.input(&id, &input).await;
    assert!(inputs() == 1, "revoked relay identity immediately blocks input");
    trusted.store(true, Ordering::SeqCst);
    hub.set_grant(PHONE, true, "view").await.unwrap();
    assert!(hub.sessions().is_empty() && platform.stopped(&id), "grant downgrade cancels control before returning");
    let view = start(&hub, PHONE, "view", None).await;
    hub.input(&view, &input).await;
    assert!(inputs() == 1, "view-only data channel cannot inject");
    platform.set_safety(|s| s.secure_input = true);
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert!(hub.sessions().is_empty() && platform.halts() > 0, "password focus closes pixel gate and kills live peers within one second");
    hub.dispose().await;
    std::fs::remove_dir_all(&directory).unwrap();
}

/// "Screen sharing enrollment race and session limits" (L518).
#[tokio::test(start_paused = true)]
async fn screen_sharing_enrollment_race_and_session_limits() {
    let directory = temp();
    let platform = Fake::new();
    let (approve, approval) = tokio::sync::watch::channel(None::<bool>);
    let confirm: ConfirmKey = Arc::new(move |_, _| {
        let mut approval = approval.clone();
        Box::pin(async move { approval.wait_for(Option::is_some).await.map(|answer| answer.unwrap_or(false)).unwrap_or(false) })
    });
    let clock = Arc::new(Mutex::new(Utc::now()));
    let now = clock.clone();
    let hub = Arc::new(
        ScreenShareHub::new(
            &directory,
            platform.clone(),
            sends_nothing(),
            confirm,
            trusts_all(),
            Some(Arc::new(move || *now.lock().unwrap())),
        )
        .unwrap(),
    );
    hub.set_grant(PHONE, true, "control").await.unwrap();
    let request = json!({ "publicKeyB64": STANDARD.encode(public(&key())) }).to_string();
    let enrolling = hub.clone();
    let enrollment = tokio::spawn(async move { enrolling.route("POST", "control-key", Some(&request), PHONE).await.unwrap() });
    while !hub.input_paused() {
        tokio::task::yield_now().await;
    }
    hub.set_grant(PHONE, false, "none").await.unwrap();
    approve.send_replace(Some(true));
    let refused = enrollment.await.unwrap();
    assert!(
        refused.status == 403 && hub.grants().len() == 1 && hub.grants()[0].control_key_public.is_none(),
        "consent response cannot resurrect a revoked grant"
    );
    const SECOND: &str = "bbbbbbbbbbbbbbbbbbbbbb";
    const THIRD: &str = "cccccccccccccccccccccc";
    for phone in [PHONE, SECOND, THIRD] {
        hub.set_grant(phone, true, "view").await.unwrap();
    }
    let first = start(&hub, PHONE, "view", None).await;
    let second = start(&hub, SECOND, "view", None).await;
    let third = status(&hub, "POST", "sessions", Some(json!({ "mode": "view", "network": "wifi", "displayId": 1 })), THIRD).await;
    assert!(third == 403, "third concurrent viewer refused");
    hub.stop_session(&second).await;
    hub.stop_session(&second).await;
    let live = hub.sessions();
    assert!(
        live.len() == 1
            && live[0].session_id == first
            && platform.stopped(&second)
            && platform.released.lock().unwrap().contains(&second)
            && !platform.stopped(&first),
        "per-session Stop tears down only the selected peer and tolerates a stale click"
    );
    start(&hub, SECOND, "view", None).await;
    hub.signal(PHONE, &json!({ "type": "screen-background", "sessionId": first, "background": true }).to_string()).await.unwrap();
    *clock.lock().unwrap() += TimeDelta::seconds(31);
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(hub.sessions().len() == 1 && platform.stopped(&first), "backgrounded phone stops after thirty seconds");
    hub.reset("rekey-pairing").await.unwrap();
    assert!(hub.grants().is_empty() && hub.sessions().is_empty(), "pairing rotation clears screen trust and active peers");
    hub.dispose().await;
    std::fs::remove_dir_all(&directory).unwrap();
}

/// "Screen sharing teardown survives relay and settings failures" (L519).
#[tokio::test(start_paused = true)]
async fn screen_sharing_teardown_survives_relay_and_settings_failures() {
    let directory = temp();
    let platform = Fake::new();
    let broken = Arc::new(AtomicBool::new(false));
    // C#'s `blocked` task: the relay's teardown notices never finish until the fixture disconnects.
    let (disconnect, disconnected) = tokio::sync::watch::channel(false);
    std::fs::write(
        directory.join("screen-devices.json"),
        format!(r#"[null,{{"deviceId":"{PHONE}","allowed":true,"grant":"control","controlKeyPublic":"malformed"}}]"#),
    )
    .unwrap();
    let blocking = broken.clone();
    let send: DeviceSend = Arc::new(move |_, message| {
        let kind = serde_json::from_str::<Value>(&message).unwrap()["type"].as_str().unwrap_or_default().to_string();
        let mut disconnected = disconnected.clone();
        let blocks = blocking.load(Ordering::SeqCst) && matches!(kind.as_str(), "screen-kill" | "screen-session-end");
        Box::pin(async move {
            if blocks {
                let _ = disconnected.wait_for(|gone| *gone).await;
                return Err(PlatformError::Io(std::io::Error::other("fixture disconnected")));
            }
            Ok(())
        })
    });
    let hub = hub(&directory, &platform, send, confirms(true), trusts_all());
    assert!(hub.grants().is_empty(), "malformed persisted identity never enables screen access");
    hub.set_grant(PHONE, true, "view").await.unwrap();
    let id = start(&hub, PHONE, "view", None).await;
    broken.store(true, Ordering::SeqCst);
    let revoking = hub.clone();
    let revoke = tokio::spawn(async move { revoking.set_grant(PHONE, false, "none").await });
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert!(platform.stopped(&id) && hub.sessions().is_empty(), "native pixels close before blocked relay notification completes");
    tokio::time::timeout(Duration::from_secs(2), revoke).await.unwrap().unwrap().unwrap();
    hub.set_grant(PHONE, true, "view").await.unwrap();
    *platform.safety.lock().unwrap() = calm();
    start(&hub, PHONE, "view", None).await;
    platform.set_safety(|s| s.secure_input = true);
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(hub.sessions().is_empty(), "privacy watcher survives blocked relay notification");
    platform.set_safety(|s| s.secure_input = false);
    let again = start(&hub, PHONE, "view", None).await;
    platform.set_safety(|s| s.locked = true);
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(platform.stopped(&again) && hub.sessions().is_empty(), "privacy watcher continues enforcing after earlier teardown failure");
    disconnect.send_replace(true);
    hub.dispose().await;
    std::fs::remove_dir_all(&directory).unwrap();

    let directory = temp();
    std::fs::write(directory.join("screen-devices.json"), "invalid json").unwrap();
    let hub = self::hub(&directory, &Fake::new(), sends_nothing(), confirms(false), trusts_all());
    assert!(hub.grants().is_empty(), "corrupted persistence starts with no grants");
    hub.dispose().await;
    std::fs::remove_dir_all(&directory).unwrap();
}

/// "Screen reference scene phase timing and frozen frame" (L520).
#[test]
fn screen_reference_scene_phase_timing_and_frozen_frame() {
    let moment = |phase, seconds_left, motion_elapsed| scene::Moment { phase, seconds_left, motion_elapsed };
    let diverges = "Measurement scene diverges from macOS timing.";
    assert!(scene::at(-1.0) == moment("preroll", 3, 0.0), "{diverges}");
    assert!(scene::at(f64::NAN) == scene::at(0.0), "{diverges}");
    assert!(scene::at(3.0) == moment("motion", 60, 0.0), "{diverges}");
    assert!(scene::at(62.5) == moment("motion", 1, 59.5), "{diverges}");
    assert!(scene::at(63.0) == moment("still", 30, 60.0), "{diverges}");
    assert!(scene::at(93.0) == moment("done", 0, 60.0), "{diverges}");
    for elapsed in [63.0, 70.0, 80.0, 92.9] {
        let moment = scene::at(elapsed);
        assert!(moment.motion_elapsed == 60.0, "{diverges}");
        assert!(
            scene::typed_count(moment.motion_elapsed) == 1080 && scene::scroll_offset(moment.motion_elapsed, 6000.0) == 5400.0,
            "{diverges}"
        );
    }
    for phase in ["preroll", "motion", "still", "done"] {
        assert!(scene::announcement(phase).is_some_and(|a| a.contains(&("phase", phase))), "{diverges}");
    }
}

type Sent = Arc<Mutex<Vec<(String, Value)>>>;

fn kind(message: &Value) -> &str {
    message["type"].as_str().unwrap_or_default()
}

/// "Mobile host: a display that disappears moves its viewers to the main one" (L484).
#[tokio::test(start_paused = true)]
async fn mobile_host_a_display_that_disappears_moves_its_viewers_to_the_main_one() {
    const FIRST: &str = "abcdefghijklmnopqrstuv";
    const SECOND: &str = "bbbbbbbbbbbbbbbbbbbbbb";
    let both = [ScreenDisplay::new(1, 1920, 1080, true), ScreenDisplay::new(2, 1280, 1024, false)];
    let id = |display: Option<ScreenDisplay>| display.map(|d| d.display_id);
    assert!(
        id(policy::display_for(&both, 2)) == Some(2)
            && id(policy::display_for(&both, 7)) == Some(1)
            && id(policy::display_for(&both[1..], 7)) == Some(2)
            && policy::display_for(&[], 1).is_none(),
        "a display that is not attached reads as the main one; only no display at all is none"
    );
    let directory = temp();
    let platform = Fake::new();
    let sent: Sent = Arc::default();
    let record = sent.clone();
    let send: DeviceSend = Arc::new(move |device, message| {
        record.lock().unwrap().push((device, serde_json::from_str(&message).unwrap()));
        done(())
    });
    let hub = hub(&directory, &platform, send, confirms(true), trusts_all());
    hub.set_grant(FIRST, true, "view").await.unwrap();
    hub.set_grant(SECOND, true, "view").await.unwrap();
    let start = async |phone: &str, display: i32| {
        let mut reply = route(&hub, "POST", "sessions", Some(sessions_request("view", display, None)), phone).await;
        assert!(reply.status == 200, "a view session starts");
        (reply.after_send.take().unwrap())().await;
        body(&reply)
    };
    let clear = || sent.lock().unwrap().clear();
    let sent_now = || sent.lock().unwrap().clone();
    let ended = || {
        let ends: Vec<Value> =
            sent_now().into_iter().map(|(_, m)| m).filter(|m| matches!(kind(m), "screen-session-end" | "screen-kill")).collect();
        ends.len() == 1 && kind(&ends[0]) == "screen-session-end" && ends[0]["reason"] == "display-gone"
    };

    let one = start(FIRST, 2).await;
    let two = start(SECOND, 1).await;
    assert!(
        one["displayId"] == 2 && two["displayId"] == 2,
        "one capture serves every phone: a phone that joins sees the display already captured"
    );
    clear();
    // The second display is unplugged.
    platform.attached.lock().unwrap().remove(1);
    hub.display_lost().await;
    assert!(
        hub.sessions().len() == 2
            && hub.sessions().iter().all(|s| s.display_id == 1)
            && platform.switched() == [1]
            && platform.halts() == 0
            && platform.stopped.lock().unwrap().is_empty(),
        "the viewers are moved to the main display, not stopped"
    );
    let mut devices: Vec<String> = sent_now().into_iter().map(|(d, _)| d).collect();
    devices.sort();
    assert!(
        sent_now().len() == 2
            && sent_now().iter().all(|(_, m)| kind(m) == "screen-grant"
                && m["grant"] == "view"
                && m["allowed"] == true
                && m["displays"].as_array().unwrap().len() == 1)
            && devices == [FIRST, SECOND],
        "each phone is told the displays there are now, with its own grant"
    );
    // The stream was taken away but its display is still there: capture starts on it again.
    hub.display_lost().await;
    assert!(platform.switched() == [1, 1] && hub.sessions().len() == 2, "a stream Windows took away is started again on the same display");
    // A locked PC is never captured, and capture has already stopped: the sessions end at once, as the watcher would end them.
    *platform.safety.lock().unwrap() = ScreenSafety { locked: true, secure_input: false, last_local_input: Utc::now() };
    hub.display_lost().await;
    assert!(
        platform.switched().len() == 2 && hub.sessions().is_empty(),
        "a locked PC is never switched to, and nobody is left watching a halted capture"
    );
    *platform.safety.lock().unwrap() = calm();
    let kills: Vec<Value> = sent_now().into_iter().map(|(_, m)| m).filter(|m| kind(m) == "screen-kill").collect();
    assert!(
        kills.iter().all(|m| m["reason"] == "lock-screen") && kills.len() == 2,
        "locking still ends every session, as a kill that says why"
    );

    // Capture cannot start on what is left: the sessions end, and say the display is gone.
    start(FIRST, 1).await;
    clear();
    platform.fail_switch.store(true, Ordering::SeqCst);
    hub.display_lost().await;
    assert!(
        hub.sessions().is_empty() && ended(),
        "with nowhere to capture the session ends as display-gone, an ordinary end and not a kill"
    );
    platform.fail_switch.store(false, Ordering::SeqCst);
    start(FIRST, 1).await;
    platform.attached.lock().unwrap().clear();
    clear();
    hub.display_lost().await;
    assert!(
        hub.sessions().is_empty() && sent_now().iter().any(|(_, m)| m["reason"] == "display-gone"),
        "and so it does with no display left"
    );
    hub.display_lost().await;
    assert!(hub.sessions().is_empty(), "with nobody watching a lost display is nothing");

    // Undocking takes one monitor after another: the display capture moves to may go as well, and the next is picked once.
    *platform.attached.lock().unwrap() =
        vec![ScreenDisplay::new(1, 1920, 1080, true), ScreenDisplay::new(2, 1280, 1024, false), ScreenDisplay::new(3, 800, 600, false)];
    platform.switched.lock().unwrap().clear();
    start(FIRST, 3).await;
    platform.attached.lock().unwrap().remove(2);
    *platform.vanish.lock().unwrap() = 1;
    clear();
    hub.display_lost().await;
    assert!(
        platform.switched() == [2]
            && hub.sessions().len() == 1
            && hub.sessions()[0].display_id == 2
            && sent_now().len() == 1
            && kind(&sent_now()[0].1) == "screen-grant",
        "a display that went away while capture moved to it is passed over for the next one"
    );
    platform.attached.lock().unwrap().insert(0, ScreenDisplay::new(1, 1920, 1080, true));
    *platform.vanish.lock().unwrap() = 2;
    clear();
    hub.display_lost().await;
    assert!(
        hub.sessions().is_empty() && platform.switched() == [2] && *platform.vanish.lock().unwrap() == 0 && ended(),
        "when the one picked next goes too, the sessions end as display-gone rather than watch a halted capture"
    );

    // A capture that keeps ending is started again three times within ten seconds, not a fourth.
    let mut recent = Vec::new();
    let at = DateTime::<Utc>::UNIX_EPOCH;
    assert!(
        policy::may_restart_capture(&mut recent, at)
            && policy::may_restart_capture(&mut recent, at + TimeDelta::seconds(1))
            && policy::may_restart_capture(&mut recent, at + TimeDelta::seconds(2))
            && !policy::may_restart_capture(&mut recent, at + TimeDelta::seconds(9))
            && policy::may_restart_capture(&mut recent, at + TimeDelta::seconds(10)),
        "three restarts in ten seconds, then another once the first has aged out"
    );
    *platform.attached.lock().unwrap() = vec![ScreenDisplay::new(1, 1920, 1080, true)];
    start(FIRST, 1).await;
    platform.switched.lock().unwrap().clear();
    for _ in 0..policy::CAPTURE_RESTARTS {
        hub.display_lost().await;
    }
    assert!(platform.switched().len() == 3 && hub.sessions().len() == 1, "a stream that ends now and then is started again each time");
    clear();
    hub.display_lost().await;
    assert!(
        hub.sessions().is_empty() && platform.switched().len() == 3 && ended(),
        "one that keeps ending is not started a fourth time: the sessions end as display-gone"
    );
    // A frame that fails is not the stream ending: capture is not started again.
    start(FIRST, 1).await;
    clear();
    hub.capture_failed().await;
    assert!(
        hub.sessions().is_empty() && platform.switched().len() == 3 && ended(),
        "failed capture ends the sessions as display-gone without starting it again"
    );
    hub.dispose().await;
    std::fs::remove_dir_all(&directory).unwrap();
}

/// Screen share stays switched off in 0.5 (D11): the hub above is never built by the app.
#[test]
fn screen_share_stays_switched_off() {
    const { assert!(!ENABLED) };
}

/// native/contracts/fixtures/screen-share.json, written by C# (its `about` says how).
fn fixture() -> Value {
    let path = format!("{}/../../../contracts/fixtures/screen-share.json", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// Byte fidelity with C#: ZstdSharp's clipboard frames read here, and `screen-devices.json` is written as C#'s hub writes it (and the
/// fingerprint is C#'s) after the same grants and enrollment.
#[tokio::test(start_paused = true)]
async fn screen_share_reads_and_writes_what_csharp_does() {
    let fixture = fixture();
    let decode = |frames: &Value| {
        let mut receiver = ScreenClipboard::default();
        let mut decoded = None;
        for frame in frames.as_array().unwrap() {
            let mut node: Value = serde_json::from_str(frame.as_str().unwrap()).unwrap();
            node["dir"] = json!("to-mac");
            decoded = receiver.receive(&node, Utc::now());
        }
        decoded
    };
    let text = fixture["line"].as_str().unwrap().repeat(fixture["repeat"].as_u64().unwrap() as usize);
    assert_eq!(decode(&fixture["frames"]).as_deref(), Some(text.as_str()), "C#'s zstd clipboard reads");
    let varied: Vec<String> = (0..15000i64).map(|i| format!("{}{}", i * 7919 % 100003, if i % 7 == 0 { "한" } else { "" })).collect();
    assert_eq!(decode(&fixture["variedFrames"]).as_deref(), Some(varied.join(" ").as_str()), "C#'s zstd with its tables reads");

    let directory = temp();
    let platform = Fake::new();
    let hub = hub(&directory, &platform, sends_nothing(), confirms(true), trusts_all());
    hub.set_grant("bbbbbbbbbbbbbbbbbbbbbb", true, "view").await.unwrap();
    hub.set_grant(PHONE, true, "control").await.unwrap();
    let key = fixture["controlKeyPublic"].as_str().unwrap();
    let reply = route(&hub, "POST", "control-key", Some(json!({ "publicKeyB64": key })), PHONE).await;
    assert_eq!((reply.status, body(&reply)["fingerprint"].as_str()), (200, fixture["fingerprint"].as_str()));
    let written = std::fs::read_to_string(directory.join("screen-devices.json")).unwrap();
    assert_eq!(written, fixture["screenDevices"].as_str().unwrap(), "screen-devices.json byte for byte");
    hub.dispose().await;
    // And C#'s file reads back whole.
    let again = self::hub(&directory, &platform, sends_nothing(), confirms(true), trusts_all());
    assert!(again.grants().len() == 2 && again.grants()[1].control_key_public.as_deref() == Some(key));
    again.dispose().await;
    std::fs::remove_dir_all(&directory).unwrap();
}

/// The concatenated `data` of one transfer's frames.
fn payload(frames: &[String]) -> (Vec<u8>, u64, String) {
    let mut bytes = Vec::new();
    let (mut declared, mut encoding) = (0, String::new());
    for frame in frames {
        let frame: Value = serde_json::from_str(frame).unwrap();
        bytes.extend(STANDARD.decode(frame["data"].as_str().unwrap()).unwrap());
        declared = frame["bytes"].as_u64().unwrap();
        encoding = frame["enc"].as_str().unwrap().to_string();
    }
    (bytes, declared, encoding)
}

/// The phone takes a zstd transfer only when its frame declares exactly `bytes` (clipboard.ts `decodeClipboardPayload`): C#'s frames
/// do (`28 B5 2F FD A0`: single segment, a 4-byte size) and so do ours (ruzstd's header given the 4-byte size).
#[test]
fn the_phone_accepts_our_zstd_frames_as_it_accepts_csharps() {
    let fixture = fixture();
    for name in ["frames", "variedFrames"] {
        let frames: Vec<String> = fixture[name].as_array().unwrap().iter().map(|f| f.as_str().unwrap().to_string()).collect();
        let (bytes, declared, encoding) = payload(&frames);
        assert!(encoding == "zstd" && bytes[..5] == [0x28, 0xB5, 0x2F, 0xFD, 0xA0]);
        assert_eq!(clipboard::frame_content_size(&bytes), Some(declared), "C#'s {name}");
    }
    let varied: String = (0..60_000u32).map(|i| char::from(b'a' + (i * 7919 % 26) as u8)).collect();
    for text in ["한글 👨‍👩‍👧‍👦\n".repeat(4000), varied] {
        let (bytes, declared, encoding) = payload(&ScreenClipboard::encode(Some(&text)));
        assert_eq!((encoding.as_str(), declared), ("zstd", text.len() as u64));
        assert_eq!(clipboard::frame_content_size(&bytes), Some(declared), "our frames declare their size");
    }
}

/// A route dropped while the user is asked (the phone's connection closed) leaves no key pending: input resumes, and the next
/// enrollment asks again.
#[tokio::test(start_paused = true)]
async fn dropping_an_enrollment_clears_the_pending_key() {
    let directory = temp();
    let platform = Fake::new();
    let asked = Arc::new(Mutex::new(0usize));
    let count = asked.clone();
    let confirm: ConfirmKey = Arc::new(move |_, _| {
        *count.lock().unwrap() += 1;
        Box::pin(std::future::pending())
    });
    let hub = Arc::new(ScreenShareHub::new(&directory, platform.clone(), sends_nothing(), confirm, trusts_all(), None).unwrap());
    hub.set_grant(PHONE, true, "control").await.unwrap();
    let request = json!({ "publicKeyB64": STANDARD.encode(public(&key())) }).to_string();
    let enrolling = hub.clone();
    let body = request.clone();
    let enrollment = tokio::spawn(async move { enrolling.route("POST", "control-key", Some(&body), PHONE).await });
    while !hub.input_paused() {
        tokio::task::yield_now().await;
    }
    enrollment.abort();
    let _ = enrollment.await;
    assert!(!hub.input_paused(), "a dropped enrollment pauses nothing");
    let again = hub.clone();
    let second = tokio::spawn(async move { again.route("POST", "control-key", Some(&request), PHONE).await });
    while *asked.lock().unwrap() < 2 {
        tokio::task::yield_now().await;
    }
    second.abort();
    let _ = second.await;
    hub.dispose().await;
    std::fs::remove_dir_all(&directory).unwrap();
}

/// A watcher tick whose probe panics fails closed (every session killed as `secure-input`), and the watcher goes on: a later lock
/// still ends the sessions.
#[tokio::test(start_paused = true)]
async fn the_privacy_watcher_fails_closed_and_survives_a_panicking_probe() {
    let directory = temp();
    let platform = Fake::new();
    let sent: Arc<Mutex<Vec<Value>>> = Arc::default();
    let record = sent.clone();
    let send: DeviceSend = Arc::new(move |_, message| {
        record.lock().unwrap().push(serde_json::from_str(&message).unwrap());
        done(())
    });
    let hub = hub(&directory, &platform, send, confirms(true), trusts_all());
    hub.set_grant(PHONE, true, "view").await.unwrap();
    let first = start(&hub, PHONE, "view", None).await;
    *platform.panic_safety.lock().unwrap() = 1;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let killed = |reason: &str| sent.lock().unwrap().iter().any(|m| m["type"] == "screen-kill" && m["reason"] == reason);
    assert!(hub.sessions().is_empty() && platform.stopped(&first) && killed("secure-input"), "a broken probe kills every session");
    let second = start(&hub, PHONE, "view", None).await;
    platform.set_safety(|s| s.locked = true);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(hub.sessions().is_empty() && platform.stopped(&second) && killed("lock-screen"), "the watcher still runs after the panic");
    hub.dispose().await;
    std::fs::remove_dir_all(&directory).unwrap();
}

/// The control signature is strict DER, as .NET's `Rfc3279DerSequence` reads it: no trailing byte, no long-form length, no integer
/// with a needless leading zero.
#[test]
fn a_control_signature_is_strict_der() {
    let key = key();
    let public_key = public(&key);
    let challenge = b"screen-control-challenge:fixture";
    let signature = sign(&key, challenge);
    assert!(policy::verify(challenge, &signature, &public_key));
    let trailing = [&signature[..], &[0]].concat();
    assert!(!policy::verify(challenge, &trailing, &public_key), "a trailing byte is refused");
    let long_form = [&[0x30, 0x81, signature[1]][..], &signature[2..]].concat();
    assert!(!policy::verify(challenge, &long_form, &public_key), "a long-form length is refused");
    // r with a needless 00 in front: a signature whose r starts below 0x80 (and is not zero-led), so the zero is no sign byte.
    let (challenge, signature) = (0u32..)
        .map(|i| format!("screen-control-challenge:{i}").into_bytes())
        .map(|challenge| {
            let signature = sign(&key, &challenge);
            (challenge, signature)
        })
        .find(|(_, signature)| (1..0x80).contains(&signature[4]))
        .unwrap();
    let r_length = usize::from(signature[3]);
    let padded = [&[0x30, signature[1] + 1, 0x02, signature[3] + 1, 0x00][..], &signature[4..]].concat();
    assert!(policy::verify(&challenge, &signature, &public_key) && padded.len() == signature.len() + 1 && r_length <= 32);
    assert!(!policy::verify(&challenge, &padded, &public_key), "a non-minimal integer is refused");
}
