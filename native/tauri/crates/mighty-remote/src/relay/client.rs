//! The relay's client side in Rust (docs/relay.md; the phone's `mobile/src/api/relay/transport.ts` is the other client): another
//! computer's Mighty Code reaching a host it paired with, through the same relay, keypair and pairing link the phone uses, with no
//! Tailscale. Over `<relay>/ws?serverId=…&role=client&connectionId=<uuid>&v=1`: the plaintext `hello`/`ready`, the host's key checked
//! against the one the pairing link carried (another key is a stale pairing: final), the session key (X25519 → HKDF-SHA256, the host's
//! own [`RelayCipher`] with the client's directions), the encrypted `auth` with `"role":"desktop"` and a client id (a pairing key the
//! first time, the device token the host gave back ever after), then requests `{id, method, path, body}` answered `{id, status, body}`
//! (at most [`MAXIMUM_IN_FLIGHT`] at once, each frame at most 1 MiB), the host's encrypted `ping` answered `pong`, this side's own ping
//! every 20 s and a 60 s read idle limit. A session that ends is dialled again at once, then after 1.5 s doubling to 30 s while dials
//! fail; a final refusal (`pairing-key`, `device-revoked`, `approval-denied`, `desktops-off`, another host key) stops for good.
//!
//! A registration with a pairing code waits for the person at the host to allow it: the host says `auth_pending`, and the answer may
//! then take up to [`ClientTimings::approval`].
//!
//! The relay URL (it names the host), the pairing key and the device token never reach a log, an error or `Debug`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use mighty_core::json::JsonOut;
use mighty_platform::ProxyResolver;
use serde_json::Value;
use serde_json::value::RawValue;
use tokio::sync::{Semaphore, mpsc, oneshot, watch};
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;
use zeroize::Zeroizing;

use crate::identity::{DESKTOP_ROLE, escape_data};
use crate::relay::cipher::{self, MAXIMUM_FRAME, MAXIMUM_PLAINTEXT, RelayCipher};
use crate::relay::connection::{HEARTBEAT, MAXIMUM_IN_FLIGHT, READ_IDLE};
use crate::relay::host::{dial, normalize_relay};

/// The pairing link's scheme and host (`mightycode://pair?…`).
pub const LINK_PREFIX: &str = "mightycode://pair?";
/// How long a registration waits for the host's answer after `auth_pending` (the host's 60 s and some slack).
pub const APPROVAL_WAIT: Duration = Duration::from_secs(75);

/// What is wrong with a pasted pairing link (never its contents).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkError {
    /// Not a `mightycode://pair?v=2&…` link.
    NotALink,
    /// A member is missing or malformed.
    Malformed,
}

impl std::fmt::Display for LinkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            LinkError::NotALink => "not a pairing link",
            LinkError::Malformed => "the pairing link is incomplete",
        })
    }
}

impl std::error::Error for LinkError {}

/// Where a paired host is reached: its relay name (`serverId`), the public key the pairing link carried and the relay. The relay URL
/// never reaches `Debug`.
#[derive(Clone, PartialEq, Eq)]
pub struct RelayEndpoint {
    pub server_id: String,
    pub public_key: [u8; 32],
    pub relay: String,
}

impl std::fmt::Debug for RelayEndpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RelayEndpoint").field("server_id", &self.server_id).finish_non_exhaustive()
    }
}

/// A pasted pairing link (`identity::Identity::desktop_pairing_url`'s v2 link; an older host gives the phone's): the endpoint, the
/// pairing code and the host's name, and whether the link says it is for a computer (`for=desktop`; an older host's never does). The
/// key never reaches `Debug`.
#[derive(Clone)]
pub struct PairingLink {
    pub endpoint: RelayEndpoint,
    pub pairing_key: Zeroizing<String>,
    pub name: String,
    pub for_desktop: bool,
}

impl std::fmt::Debug for PairingLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PairingLink").field("endpoint", &self.endpoint).field("name", &self.name).finish_non_exhaustive()
    }
}

/// `^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$`, the relay's rule for a `serverId`.
pub fn server_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 128
        && bytes[0].is_ascii_alphanumeric()
        && bytes[1..].iter().all(|b| b.is_ascii_alphanumeric() || b"._:-".contains(b))
}

/// A 32-byte key in base64url (the link's) or standard base64 (the handshake's), padded or not.
pub fn decode_key(value: &str) -> Option<[u8; 32]> {
    let trimmed = value.trim_end_matches('=');
    let engines = [base64::engine::general_purpose::URL_SAFE_NO_PAD, base64::engine::general_purpose::STANDARD_NO_PAD];
    engines.iter().find_map(|engine| engine.decode(trimmed).ok()).and_then(|bytes| bytes.try_into().ok())
}

impl PairingLink {
    /// The link as pasted (spaces, quotes and angle brackets around it let go). Its `direct=` endpoints are the phone's and are ignored:
    /// a computer reaches the host through the relay only.
    pub fn parse(text: &str) -> Result<PairingLink, LinkError> {
        let text = text.trim().trim_matches(|c: char| c == '"' || c == '\'' || c == '<' || c == '>' || c.is_whitespace());
        if !text.get(..LINK_PREFIX.len()).is_some_and(|head| head.eq_ignore_ascii_case(LINK_PREFIX)) {
            return Err(LinkError::NotALink);
        }
        let parsed = url::Url::parse(text).map_err(|_| LinkError::NotALink)?;
        let mut members: HashMap<String, String> = HashMap::new();
        for (name, value) in parsed.query_pairs() {
            members.entry(name.into_owned()).or_insert_with(|| value.into_owned());
        }
        let member = |name: &str| members.get(name).map(String::as_str);
        if member("v") != Some("2") {
            return Err(LinkError::NotALink);
        }
        let sid = member("sid").filter(|s| server_id(s)).ok_or(LinkError::Malformed)?;
        let public_key = member("pk").and_then(decode_key).ok_or(LinkError::Malformed)?;
        let relay = member("relay").and_then(normalize_relay).ok_or(LinkError::Malformed)?;
        let key = member("key").filter(|k| !k.is_empty() && k.encode_utf16().count() <= 256).ok_or(LinkError::Malformed)?;
        let name = mighty_core::wire::clean(member("name").unwrap_or_default(), 120);
        Ok(PairingLink {
            endpoint: RelayEndpoint { server_id: sid.to_string(), public_key, relay },
            pairing_key: Zeroizing::new(key.to_string()),
            name,
            for_desktop: member("for") == Some("desktop"),
        })
    }
}

/// How this computer proves itself: its client id with the pairing key (the first connection) or with the device token the host gave
/// back. Neither secret reaches `Debug`.
#[derive(Clone, PartialEq, Eq)]
pub enum Credential {
    PairingKey { client_id: String, key: Zeroizing<String> },
    Token { client_id: String, token: Zeroizing<String> },
}

impl Credential {
    pub fn client_id(&self) -> &str {
        match self {
            Credential::PairingKey { client_id, .. } | Credential::Token { client_id, .. } => client_id,
        }
    }

    /// Whether the host gave this computer its token already.
    pub fn has_token(&self) -> bool {
        matches!(self, Credential::Token { .. })
    }
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kind = if self.has_token() { "Token" } else { "PairingKey" };
        f.debug_struct(kind).field("client_id", &self.client_id()).finish_non_exhaustive()
    }
}

/// A new client id: 16 random bytes in base64url (the 22 characters the host's device list takes).
pub fn new_client_id() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("the OS random source");
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Why a session could not be had (never a secret or the relay's address).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelayFailure {
    /// The relay could not be reached (no network, a proxy, TLS, an HTTP refusal).
    Unreachable,
    /// The relay has no host under this name now (4404, 4410, 4504): the host is off, asleep, or not sharing.
    HostOffline,
    /// The host has as many connections as it takes (4429).
    TooMany,
    /// The host answered with another key than the pairing link's: the pairing is stale (final).
    KeyMismatch,
    /// The host refused the auth with this reason; `last` for `pairing-key`, `device-revoked`, `approval-denied` and `desktops-off`
    /// (pair again, with a new link).
    Refused { reason: String, last: bool },
    /// The host's answer was not the relay protocol.
    Protocol,
    /// The session ended (the network, the host's link, the idle limit).
    Lost,
    /// This client was closed.
    Closed,
}

impl RelayFailure {
    /// Pairing again is the only way back: nothing is tried again.
    pub fn is_final(&self) -> bool {
        matches!(self, RelayFailure::KeyMismatch | RelayFailure::Refused { last: true, .. })
    }

    /// Worth asking again for a while.
    pub fn transient(&self) -> bool {
        matches!(self, RelayFailure::Unreachable | RelayFailure::HostOffline | RelayFailure::TooMany | RelayFailure::Lost)
    }
}

impl std::fmt::Display for RelayFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RelayFailure::Unreachable => f.write_str("the relay could not be reached"),
            RelayFailure::HostOffline => f.write_str("the host is not connected to the relay"),
            RelayFailure::TooMany => f.write_str("the host takes no more connections"),
            RelayFailure::KeyMismatch => f.write_str("the host's key is not the paired one"),
            RelayFailure::Refused { reason, .. } => write!(f, "the host refused: {reason}"),
            RelayFailure::Protocol => f.write_str("the host's answer was not the relay protocol"),
            RelayFailure::Lost => f.write_str("the relay connection ended"),
            RelayFailure::Closed => f.write_str("the relay client was closed"),
        }
    }
}

impl std::error::Error for RelayFailure {}

/// The relay's close codes as failures (`mapCloseCode` in the phone's transport).
pub fn close_failure(code: Option<u16>) -> RelayFailure {
    match code {
        Some(4404 | 4410 | 4504) => RelayFailure::HostOffline,
        Some(4429) => RelayFailure::TooMany,
        Some(4401) => RelayFailure::Refused { reason: "pairing-key".into(), last: true },
        Some(4413) => RelayFailure::Protocol,
        _ => RelayFailure::Lost,
    }
}

/// `auth_ok`'s description of the host.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostHello {
    pub host_name: String,
    pub host_id: String,
    pub app_version: String,
    pub capabilities: Vec<String>,
}

/// One answer: its status and its body's JSON text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayReply {
    pub status: u16,
    pub body: Vec<u8>,
}

/// Why one request got no answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestError {
    /// No session (the latest dial's failure).
    Failure(RelayFailure),
    /// The request does not fit one relay frame (1 MiB with its envelope): never sent.
    TooLarge,
    /// The body is not JSON: never sent.
    BadBody,
    /// No answer within the request's time.
    Timeout,
    /// The session ended before the answer came.
    Lost,
}

/// The client's clocks (the checks shorten them).
#[derive(Debug, Clone, Copy)]
pub struct ClientTimings {
    pub handshake: Duration,
    /// How long a registration waits for the person at the host once it said `auth_pending`.
    pub approval: Duration,
    pub heartbeat: Duration,
    pub idle: Duration,
    pub backoff_min: Duration,
    pub backoff_max: Duration,
}

impl Default for ClientTimings {
    fn default() -> Self {
        ClientTimings {
            handshake: crate::direct::wire::HANDSHAKE_DEADLINE,
            approval: APPROVAL_WAIT,
            heartbeat: HEARTBEAT,
            idle: READ_IDLE,
            backoff_min: Duration::from_millis(1500),
            backoff_max: Duration::from_secs(30),
        }
    }
}

/// Told the device token the host gave back on the first connection (the caller keeps it in the vault); called on the client's task,
/// so it must return quickly.
pub type TokenSink = Arc<dyn Fn(&Credential) + Send + Sync>;

#[derive(Clone)]
enum Phase {
    Connecting,
    Ready { outbox: mpsc::Sender<Vec<u8>>, hello: Arc<HostHello> },
    Failed(RelayFailure),
    Closed,
}

struct Inner {
    endpoint: RelayEndpoint,
    client_name: String,
    proxy: Arc<dyn ProxyResolver>,
    timings: ClientTimings,
    credential: Mutex<Credential>,
    on_token: Option<TokenSink>,
    phase: watch::Sender<Phase>,
    pending: Mutex<HashMap<String, oneshot::Sender<RelayReply>>>,
    slots: Semaphore,
    closed: watch::Sender<bool>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Closes the client once its last handle goes.
struct Handle(Arc<Inner>);

impl Drop for Handle {
    fn drop(&mut self) {
        self.0.close();
    }
}

/// One paired host's relay link: started at once, dialled again whenever it ends, closed by [`RelayClient::close`] or when the last
/// clone goes.
#[derive(Clone)]
pub struct RelayClient(Arc<Handle>);

impl std::fmt::Debug for RelayClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RelayClient").field("endpoint", &self.0.0.endpoint).finish_non_exhaustive()
    }
}

impl RelayClient {
    /// Starts the link on the current runtime: `client_name` is how the host's device list names this computer; `on_token` hears of
    /// the token the host gives back once.
    pub fn start(
        endpoint: RelayEndpoint,
        credential: Credential,
        client_name: &str,
        proxy: Arc<dyn ProxyResolver>,
        timings: ClientTimings,
        on_token: Option<TokenSink>,
    ) -> RelayClient {
        let inner = Arc::new(Inner {
            endpoint,
            client_name: mighty_core::wire::clean(client_name, 40),
            proxy,
            timings,
            credential: Mutex::new(credential),
            on_token,
            phase: watch::channel(Phase::Connecting).0,
            pending: Mutex::new(HashMap::new()),
            slots: Semaphore::new(MAXIMUM_IN_FLIGHT),
            closed: watch::channel(false).0,
        });
        tokio::spawn(supervise(inner.clone()));
        RelayClient(Arc::new(Handle(inner)))
    }

    fn inner(&self) -> &Arc<Inner> {
        &self.0.0
    }

    /// The first session's outcome: the host's `auth_ok`, or why there was none (a pair or a reconnect says it to the user). Waits at
    /// most `limit`.
    pub async fn ready(&self, limit: Duration) -> Result<HostHello, RelayFailure> {
        let mut phase = self.inner().phase.subscribe();
        let settled = tokio::time::timeout(limit, phase.wait_for(|p| !matches!(p, Phase::Connecting))).await;
        let outcome = match settled {
            Ok(Ok(phase)) => phase.clone(),
            Ok(Err(_)) => Phase::Closed,
            Err(_) => return Err(RelayFailure::HostOffline),
        };
        match outcome {
            Phase::Ready { hello, .. } => Ok((*hello).clone()),
            Phase::Failed(failure) => Err(failure),
            Phase::Connecting | Phase::Closed => Err(RelayFailure::Closed),
        }
    }

    /// The credential now (the token once the host gave one).
    pub fn credential(&self) -> Credential {
        lock(&self.inner().credential).clone()
    }

    /// The final failure that stopped the link, if one did.
    pub fn final_failure(&self) -> Option<RelayFailure> {
        match &*self.inner().phase.borrow() {
            Phase::Failed(failure) if failure.is_final() => Some(failure.clone()),
            _ => None,
        }
    }

    /// One request (`method` GET or POST, `path` from `/`, `body` JSON) answered within `timeout`; it waits for a session while one is
    /// being dialled, and fails at once while the latest dial failed.
    pub async fn request(&self, method: &str, path: &str, body: Option<&[u8]>, timeout: Duration) -> Result<RelayReply, RequestError> {
        let inner = self.inner();
        let deadline = Instant::now() + timeout;
        let raw = match body {
            Some(bytes) => Some(serde_json::from_slice::<Box<RawValue>>(bytes).map_err(|_| RequestError::BadBody)?),
            None => None,
        };
        let id = uuid::Uuid::new_v4().to_string();
        let mut out = JsonOut::new();
        out.begin_object();
        out.prop_str("id", Some(&id));
        out.prop_str("method", Some(method));
        out.prop_str("path", Some(path));
        if let Some(raw) = &raw {
            out.name("body");
            out.raw(raw.get());
        }
        out.end_object();
        let frame = out.finish().into_bytes();
        if frame.len() > MAXIMUM_PLAINTEXT {
            return Err(RequestError::TooLarge);
        }
        let _slot = tokio::time::timeout_at(deadline, inner.slots.acquire()).await.map_err(|_| RequestError::Timeout)?;
        let mut phase = inner.phase.subscribe();
        let current = tokio::time::timeout_at(deadline, phase.wait_for(|p| !matches!(p, Phase::Connecting)))
            .await
            .map_err(|_| RequestError::Timeout)?
            .map(|p| p.clone())
            .unwrap_or(Phase::Closed);
        let outbox = match current {
            Phase::Ready { outbox, .. } => outbox,
            Phase::Failed(failure) => return Err(RequestError::Failure(failure)),
            Phase::Connecting | Phase::Closed => return Err(RequestError::Failure(RelayFailure::Closed)),
        };
        /// The request's slot in the pending table, taken back however it ends.
        struct Waiting<'a>(&'a Inner, String);
        impl Drop for Waiting<'_> {
            fn drop(&mut self) {
                lock(&self.0.pending).remove(&self.1);
            }
        }
        let (answer, answered) = oneshot::channel();
        lock(&inner.pending).insert(id.clone(), answer);
        let _waiting = Waiting(inner, id);
        tokio::time::timeout_at(deadline, outbox.send(frame)).await.map_err(|_| RequestError::Timeout)?.map_err(|_| RequestError::Lost)?;
        match tokio::time::timeout_at(deadline, answered).await {
            Ok(Ok(reply)) => Ok(reply),
            Ok(Err(_)) => Err(RequestError::Lost),
            Err(_) => Err(RequestError::Timeout),
        }
    }

    /// Ends the link: the session closes, the requests waiting fail, nothing is dialled again.
    pub fn close(&self) {
        self.inner().close();
    }
}

impl Inner {
    fn close(&self) {
        self.closed.send_replace(true);
        self.phase.send_replace(Phase::Closed);
        self.fail_pending();
    }

    fn closed(&self) -> bool {
        *self.closed.borrow()
    }

    /// Every request waiting on the session that ended hears so (its answer channel dropped).
    fn fail_pending(&self) {
        lock(&self.pending).clear();
    }

    fn set_phase(&self, phase: Phase) {
        // A closed client stays closed: a session that ends afterwards never reopens the phase.
        if !self.closed() {
            self.phase.send_replace(phase);
        }
    }
}

/// The link's life: a session, then the next after the backoff (1.5 s after a session that was up, doubling to 30 s while dials fail).
/// Between sessions the phase is the last failure, so a request then fails at once rather than waiting out its time.
async fn supervise(inner: Arc<Inner>) {
    let mut closed = inner.closed.subscribe();
    let mut delay = inner.timings.backoff_min;
    loop {
        if inner.closed() {
            break;
        }
        inner.set_phase(Phase::Connecting);
        let outcome = tokio::select! {
            outcome = session(&inner) => outcome,
            _ = until_true(&mut closed) => break,
        };
        inner.fail_pending();
        let failure = match outcome {
            Ok(()) => {
                delay = inner.timings.backoff_min;
                RelayFailure::Lost
            }
            Err(failure) => failure,
        };
        let last = failure.is_final();
        inner.set_phase(Phase::Failed(failure));
        if last {
            return;
        }
        tokio::select! {
            _ = tokio::time::sleep(delay) => {}
            _ = until_true(&mut closed) => break,
        }
        delay = (delay * 2).min(inner.timings.backoff_max);
    }
    inner.phase.send_replace(Phase::Closed);
}

/// Waits until the flag reads true (or its sender is gone); the borrow it takes ends before this returns, so a `select!` over it stays
/// `Send`.
async fn until_true(flag: &mut watch::Receiver<bool>) {
    let _ = flag.wait_for(|set| *set).await;
}

type Socket = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
type Stream = futures_util::stream::SplitStream<Socket>;

/// What the socket brought next while the handshake runs.
enum Next {
    Text(String),
    Binary(Vec<u8>),
}

/// The next data frame before `deadline`; a close says why (its code), a timeout or an error is a lost session.
async fn next_frame(stream: &mut Stream, deadline: Instant) -> Result<Next, RelayFailure> {
    loop {
        let message = tokio::time::timeout_at(deadline, stream.next()).await.map_err(|_| RelayFailure::HostOffline)?;
        match message {
            Some(Ok(Message::Text(text))) if text.len() <= MAXIMUM_FRAME => return Ok(Next::Text(text.as_str().to_string())),
            Some(Ok(Message::Binary(bytes))) if bytes.len() <= MAXIMUM_FRAME => return Ok(Next::Binary(bytes.to_vec())),
            Some(Ok(Message::Text(_) | Message::Binary(_))) => return Err(RelayFailure::Protocol),
            Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_))) => continue,
            Some(Ok(Message::Close(frame))) => return Err(close_failure(frame.map(|f| u16::from(f.code)))),
            Some(Err(_)) | None => return Err(RelayFailure::Lost),
        }
    }
}

fn random<const N: usize>() -> Zeroizing<[u8; N]> {
    let mut bytes = Zeroizing::new([0u8; N]);
    getrandom::fill(bytes.as_mut()).expect("the OS random source");
    bytes
}

fn text_of(value: &Value, name: &str) -> String {
    value.get(name).and_then(Value::as_str).unwrap_or_default().to_string()
}

/// One session: dial, handshake, auth, then requests until it ends. `Ok` when it was up and ended; `Err` when it never came up.
async fn session(inner: &Arc<Inner>) -> Result<(), RelayFailure> {
    let deadline = Instant::now() + inner.timings.handshake;
    let url = format!(
        "{}/ws?serverId={}&role=client&connectionId={}&v=1",
        inner.endpoint.relay,
        escape_data(&inner.endpoint.server_id),
        uuid::Uuid::new_v4()
    );
    let socket = match tokio::time::timeout_at(deadline, dial(&url, inner.proxy.as_ref())).await {
        Ok(Ok(socket)) => socket,
        _ => return Err(RelayFailure::Unreachable),
    };
    let (mut sink, mut stream) = socket.split();
    let b64 = base64::engine::general_purpose::STANDARD;
    let secret = random::<32>();
    let client_nonce = random::<16>();
    let mut hello = JsonOut::new();
    hello.begin_object();
    hello.prop_str("type", Some("hello"));
    hello.prop_int("v", 1);
    hello.prop_str("clientKey", Some(&b64.encode(cipher::public_key(&secret))));
    hello.prop_str("nonce", Some(&b64.encode(client_nonce.as_ref())));
    hello.end_object();
    let sent = tokio::time::timeout_at(deadline, sink.send(Message::text(hello.finish()))).await;
    if !matches!(sent, Ok(Ok(()))) {
        return Err(RelayFailure::Lost);
    }
    let Next::Text(ready) = next_frame(&mut stream, deadline).await? else { return Err(RelayFailure::Protocol) };
    let ready: Value = serde_json::from_str(&ready).map_err(|_| RelayFailure::Protocol)?;
    if text_of(&ready, "type") != "ready" || ready.get("v").and_then(Value::as_i64) != Some(1) {
        return Err(RelayFailure::Protocol);
    }
    let server_key = decode_key(&text_of(&ready, "serverKey")).ok_or(RelayFailure::Protocol)?;
    // The relay names the host by its serverId, so another key is no hiccup: this pairing no longer opens it.
    if !bool::from(subtle::ConstantTimeEq::ct_eq(&server_key[..], &inner.endpoint.public_key[..])) {
        return Err(RelayFailure::KeyMismatch);
    }
    let server_nonce = b64.decode(text_of(&ready, "nonce")).map_err(|_| RelayFailure::Protocol)?;
    let cipher =
        RelayCipher::new(secret.as_ref(), &server_key, client_nonce.as_ref(), &server_nonce, false).map_err(|_| RelayFailure::Protocol)?;
    let credential = lock(&inner.credential).clone();
    let auth = {
        let mut out = JsonOut::new();
        out.begin_object();
        out.prop_str("type", Some("auth"));
        out.prop_str("clientId", Some(credential.client_id()));
        out.prop_str("clientName", Some(&inner.client_name));
        out.prop_str("role", Some(DESKTOP_ROLE));
        match &credential {
            Credential::PairingKey { key, .. } => out.prop_str("pairingKey", Some(key.as_str())),
            Credential::Token { token, .. } => out.prop_str("deviceToken", Some(token.as_str())),
        }
        out.end_object();
        Zeroizing::new(out.finish())
    };
    let sealed = cipher.seal(auth.as_bytes()).map_err(|_| RelayFailure::Protocol)?;
    let sent = tokio::time::timeout_at(deadline, sink.send(Message::binary(sealed))).await;
    if !matches!(sent, Ok(Ok(()))) {
        return Err(RelayFailure::Lost);
    }
    // A registration waits for the person at the host: `auth_pending` first, then the answer within the approval time.
    let mut deadline = deadline;
    let answer = loop {
        let Next::Binary(answer) = next_frame(&mut stream, deadline).await? else { return Err(RelayFailure::Protocol) };
        let answer: Value =
            serde_json::from_slice(&cipher.open(&answer).map_err(|_| RelayFailure::Protocol)?).map_err(|_| RelayFailure::Protocol)?;
        match text_of(&answer, "type").as_str() {
            "auth_pending" if !credential.has_token() => deadline = Instant::now() + inner.timings.approval,
            "auth_ok" => break answer,
            "auth_error" => {
                let reason = text_of(&answer, "reason");
                let last = matches!(reason.as_str(), "pairing-key" | "device-revoked" | "approval-denied" | "desktops-off");
                return Err(RelayFailure::Refused { reason: mighty_core::wire::clean(&reason, 40), last });
            }
            _ => return Err(RelayFailure::Protocol),
        }
    };
    // The token comes once, on the connection that registered this computer: from here on it is the credential.
    if let Some(token) = answer.get("deviceToken").and_then(Value::as_str).filter(|t| !t.is_empty() && t.len() <= 256) {
        let next = Credential::Token { client_id: credential.client_id().to_string(), token: Zeroizing::new(token.to_string()) };
        *lock(&inner.credential) = next.clone();
        if let Some(sink) = &inner.on_token {
            let told = sink.clone();
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| told(&next)));
        }
    }
    let hello = Arc::new(HostHello {
        host_name: mighty_core::wire::clean(&text_of(&answer, "hostName"), 120),
        host_id: text_of(&answer, "hostId"),
        app_version: text_of(&answer, "appVersion"),
        capabilities: answer
            .get("capabilities")
            .and_then(Value::as_array)
            .map(|list| list.iter().filter_map(Value::as_str).map(str::to_string).collect())
            .unwrap_or_default(),
    });
    let (outbox, mut inbox) = mpsc::channel::<Vec<u8>>(16);
    inner.set_phase(Phase::Ready { outbox, hello });
    let timings = inner.timings;
    let mut ping = tokio::time::interval_at(Instant::now() + timings.heartbeat, timings.heartbeat.max(Duration::from_millis(1)));
    let mut idle = Instant::now() + timings.idle;
    let mut closed = inner.closed.subscribe();
    // Sends one sealed frame; false when the socket is gone or stalls past the idle limit.
    async fn send(
        sink: &mut futures_util::stream::SplitSink<Socket, Message>,
        cipher: &RelayCipher,
        plain: &[u8],
        limit: Duration,
    ) -> bool {
        let Ok(sealed) = cipher.seal(plain) else { return false };
        matches!(tokio::time::timeout(limit, sink.send(Message::binary(sealed))).await, Ok(Ok(())))
    }
    loop {
        tokio::select! {
            _ = until_true(&mut closed) => break,
            out = inbox.recv() => match out {
                Some(plain) => if !send(&mut sink, &cipher, &plain, timings.idle).await { break },
                None => break,
            },
            _ = ping.tick() => if !send(&mut sink, &cipher, br#"{"type":"ping"}"#, timings.idle).await { break },
            _ = tokio::time::sleep_until(idle) => break,
            message = stream.next() => {
                let frame = match message {
                    Some(Ok(Message::Binary(bytes))) if bytes.len() <= MAXIMUM_FRAME => bytes,
                    Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_) | Message::Text(_))) => {
                        idle = Instant::now() + timings.idle;
                        continue;
                    }
                    _ => break,
                };
                idle = Instant::now() + timings.idle;
                let Ok(plain) = cipher.open(&frame) else { break };
                let Ok(root) = serde_json::from_slice::<HashMap<String, Box<RawValue>>>(&plain) else { break };
                let text = |name: &str| root.get(name).and_then(|raw| serde_json::from_str::<String>(raw.get()).ok());
                match text("type").as_deref() {
                    Some("ping") => if !send(&mut sink, &cipher, br#"{"type":"pong"}"#, timings.idle).await { break },
                    // A pong, the host's notify (the phone's: the mirror polls) and any word this build does not know.
                    Some(_) => {}
                    None => {
                        let Some(id) = text("id") else { continue };
                        let status = root.get("status").and_then(|raw| serde_json::from_str::<u16>(raw.get()).ok()).unwrap_or(0);
                        let body = root.get("body").map(|raw| raw.get().as_bytes().to_vec()).unwrap_or_else(|| b"null".to_vec());
                        let waiting = lock(&inner.pending).remove(&id);
                        if let Some(waiting) = waiting {
                            let _ = waiting.send(RelayReply { status, body });
                        }
                    }
                }
            }
        }
    }
    cipher.dispose();
    let _ = tokio::time::timeout(Duration::from_secs(1), sink.close()).await;
    Ok(())
}
