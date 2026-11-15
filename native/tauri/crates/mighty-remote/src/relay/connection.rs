//! One phone's connection, shared by the relay and the direct path (C# `MobileRelayHost`'s per-connection half, R\MobileRelayHost.cs:
//! 565-701; plan §1.1): over any `WebSocketStream`, the plaintext hello and ready, the X25519 key, the encrypted `auth` with its 10 s
//! deadline (`auth_ok` / `auth_error` and a close; on the direct path the reason is always `unauthorized`), then the request pump with
//! 8 slots (a duplicate id or a ninth answers 429), one writer, `ping` → `pong`, the host's encrypted ping every 20 s with a 60 s read
//! idle limit, notify frames, and the host-level errors (414 path, 413 reply, 500). Owner: W8-A.
//!
//! Written whole in W8 Step 0: [`RequestHandler`] (what the m1 router is to a connection: `phone::router::Router` implements it, A's
//! round-trip test a stub), [`MobileReply`] and [`MobileDevice`].
//!
//! One writer: every frame after the handshake goes through one task that seals and sends in the order frames were handed to it, so
//! the phone sees the send counter rise frame by frame. Ending the connection (a cancel, the socket's end, the idle limit, a frame that
//! does not open) drops the requests still in flight (their futures are dropped, which cancels them) and wipes the session key.

use std::collections::HashSet;
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use indexmap::IndexMap;
use mighty_core::json::JsonOut;
use mighty_platform::BoxFuture;
use serde_json::value::RawValue;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::Instant;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;

use crate::identity::{AuthDecision, AuthRequest};
use crate::relay::cipher::{MAXIMUM_FRAME, MAXIMUM_PLAINTEXT, RelayCipher};

/// What runs once a reply's frame was sent (C# `MobileReply.AfterSend`: an app update that restarts the app after answering).
pub type AfterSend = Box<dyn FnOnce() -> BoxFuture<'static, ()> + Send>;

/// C# `MobileReply(Status, Body, AfterSend?)`: a route's answer. `body` is the JSON text as C#'s `Wire.Json` writes it (written with
/// `mighty_core::json::JsonOut`, never serde: its escaping decides the 1 MiB limit, plan §5 R2).
pub struct MobileReply {
    pub status: u16,
    pub body: String,
    pub after_send: Option<AfterSend>,
}

impl MobileReply {
    pub fn new(status: u16, body: String) -> MobileReply {
        MobileReply { status, body, after_send: None }
    }

    /// `{protocol:1, error, code?}` (C# `Error(status, text, code)`; the host's errors, R\MobileRelayHost.cs:638-682, and the router's).
    pub fn error(status: u16, error: &str, code: Option<&str>) -> MobileReply {
        let mut out = JsonOut::new();
        out.begin_object();
        out.prop_int("protocol", 1);
        out.prop_str("error", Some(error));
        out.prop_str("code", code);
        out.end_object();
        MobileReply::new(status, out.finish())
    }
}

impl std::fmt::Debug for MobileReply {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MobileReply")
            .field("status", &self.status)
            .field("body_bytes", &self.body.len())
            .field("after_send", &self.after_send.is_some())
            .finish()
    }
}

/// The phone a request comes from, once its `auth` passed: its 22-character client id (or `legacy`), its name, and whether it came
/// over the direct path. Never its token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobileDevice {
    pub client_id: String,
    pub name: String,
    pub direct: bool,
}

/// What answers an authenticated phone's requests (C#'s `Func<string, string, JsonElement?, string, CancellationToken,
/// Task<MobileReply>>` handed to `MobileRelayHost`: WinUI `MobileRoute` over `MobileDesktopRouter.RouteAsync`).
///
/// Contract: `method` is `GET` or `POST`, `path` starts with `/` and is at most 16384 bytes (the connection answered 414 before);
/// `body` is the request's `body` member as the phone sent it (its raw JSON text, so the router can measure it), None when absent.
/// The future is `'static`: the connection runs it on a task of its own (8 at a time), and dropping it cancels the request (the
/// connection closed). It never panics on a phone's input; a failure is a reply (C#'s 500 "Desktop request failed.").
pub trait RequestHandler: Send + Sync {
    fn handle(&self, device: MobileDevice, method: String, path: String, body: Option<Box<RawValue>>) -> BoxFuture<'static, MobileReply>;
}

/// At most this many requests run at once on one connection.
pub const MAXIMUM_IN_FLIGHT: usize = 8;
/// A path longer than this (UTF-8 bytes) is answered 414.
pub const MAXIMUM_PATH_BYTES: usize = 16384;
/// The plaintext hello's limit.
pub const MAXIMUM_HELLO_BYTES: usize = 4096;
/// The host's encrypted ping.
pub const HEARTBEAT: Duration = Duration::from_secs(20);
/// A connection that hears nothing for this long ends.
pub const READ_IDLE: Duration = Duration::from_secs(60);
/// `MobileDirect.GenericRefusal`: every refusal on the direct path, so a tailnet peer learns nothing of which ids or keys this PC knows.
pub const GENERIC_REFUSAL: &str = "unauthorized";

/// What a connection reads of its host (the relay host's shared half).
pub(crate) trait ConnectionHost: Send + Sync + 'static {
    fn identity(&self) -> &crate::identity::Identity;
    fn allow_legacy(&self) -> bool;
    fn handler(&self) -> Arc<dyn RequestHandler>;
    /// `auth_ok`'s `hostName`, `appVersion`, `capabilities` and `direct`.
    fn host_name(&self) -> String;
    fn app_version(&self) -> String;
    fn capabilities(&self) -> Vec<String>;
    fn direct_endpoints(&self) -> Vec<String>;
    /// A direct phone's refused auth, counted against its address.
    fn record_direct_failure(&self, peer: IpAddr);
    /// A phone authenticated: the status's count moves.
    fn authenticated(&self);
}

struct Outgoing {
    plaintext: Vec<u8>,
    sent: Option<oneshot::Sender<bool>>,
}

/// One phone's connection (C#'s private `Connection`).
pub(crate) struct Connection {
    pub id: String,
    /// Came in on the direct path: kept through relay redials, refused with one word, its failed auths counted against `peer`.
    pub direct: bool,
    pub peer: Option<IpAddr>,
    authenticated: AtomicBool,
    device: Mutex<Option<MobileDevice>>,
    cancel: watch::Sender<bool>,
    done: watch::Sender<bool>,
    outbox: Mutex<Option<mpsc::Sender<Outgoing>>>,
    inflight: Mutex<HashSet<String>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Why the session ended early: nothing to tell anyone (C# catches these and lets the connection go).
struct Ended;

type Step<T> = Result<T, Ended>;

impl Connection {
    pub fn new(id: String, direct: bool, peer: Option<IpAddr>) -> Arc<Connection> {
        Arc::new(Connection {
            id,
            direct,
            peer,
            authenticated: AtomicBool::new(false),
            device: Mutex::new(None),
            cancel: watch::channel(false).0,
            done: watch::channel(false).0,
            outbox: Mutex::new(None),
            inflight: Mutex::new(HashSet::new()),
        })
    }

    pub fn authenticated(&self) -> bool {
        self.authenticated.load(Ordering::SeqCst)
    }

    /// The authenticated phone's id ("" before its auth passed).
    pub fn device_id(&self) -> String {
        lock(&self.device).as_ref().map(|d| d.client_id.clone()).unwrap_or_default()
    }

    /// `Cancel`: the connection ends (its socket closed, its requests dropped).
    pub fn cancel(&self) {
        self.cancel.send_replace(true);
    }

    pub fn cancelled(&self) -> bool {
        *self.cancel.borrow()
    }

    /// Waits until the connection has ended and let go of everything (C#'s `Completion`).
    pub async fn completion(&self) {
        let mut done = self.done.subscribe();
        let _ = until_true(&mut done).await;
    }

    pub(crate) fn finish(&self) {
        self.done.send_replace(true);
    }

    /// `Send`: a JSON frame for the phone, sealed by the writer in order; it waits while [`OUTBOX`] frames are queued (C#'s one writer
    /// lock: a phone that reads nothing holds its senders back rather than the host's memory). Dropped silently before the handshake
    /// ended or after the
    /// connection did.
    pub async fn send(&self, plaintext: Vec<u8>) {
        let outbox = lock(&self.outbox).clone();
        if let Some(outbox) = outbox {
            let _ = outbox.send(Outgoing { plaintext, sent: None }).await;
        }
    }

    /// [`Connection::send`], answering once the frame was written (true) or the connection ended first (false).
    async fn send_and_wait(&self, plaintext: Vec<u8>) -> bool {
        let (sent, written) = oneshot::channel();
        let outbox = lock(&self.outbox).clone();
        let Some(outbox) = outbox else { return false };
        outbox.send(Outgoing { plaintext, sent: Some(sent) }).await.is_ok() && written.await.unwrap_or(false)
    }

    /// `Run`: the whole connection, from opening its socket (inside the handshake deadline) to its end; the caller (the host's `Serve`)
    /// removes it from the table afterwards.
    pub async fn run<S, H>(self: Arc<Self>, host: Arc<H>, open: BoxFuture<'static, Option<WebSocketStream<S>>>, deadline: Instant)
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
        H: ConnectionHost,
    {
        let mut cancelled = self.cancel.subscribe();
        let mut requests = tokio::task::JoinSet::new();
        let mut tasks = tokio::task::JoinSet::new();
        {
            let session = self.clone().session(host, open, deadline, &mut requests, &mut tasks);
            tokio::select! {
                _ = session => {}
                _ = until_true(&mut cancelled) => {}
            }
        }
        // Ended on every way out: nothing more is written, the requests still running are dropped, the writer and heartbeat stop.
        self.cancel();
        lock(&self.outbox).take();
        requests.abort_all();
        while requests.join_next().await.is_some() {}
        tasks.abort_all();
        while tasks.join_next().await.is_some() {}
    }

    async fn session<S, H>(
        self: Arc<Self>,
        host: Arc<H>,
        open: BoxFuture<'static, Option<WebSocketStream<S>>>,
        deadline: Instant,
        requests: &mut tokio::task::JoinSet<()>,
        tasks: &mut tokio::task::JoinSet<()>,
    ) where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
        H: ConnectionHost,
    {
        let _ = self.handshake_and_serve(host, open, deadline, requests, tasks).await;
    }

    async fn handshake_and_serve<S, H>(
        self: Arc<Self>,
        host: Arc<H>,
        open: BoxFuture<'static, Option<WebSocketStream<S>>>,
        deadline: Instant,
        requests: &mut tokio::task::JoinSet<()>,
        tasks: &mut tokio::task::JoinSet<()>,
    ) -> Step<()>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
        H: ConnectionHost,
    {
        // One deadline from the socket to auth_ok (`deadline`, set by the host): the relay path dials its data socket, the direct
        // path's upgrade was read since its admission, and both then read hello and auth, all within it.
        let socket = tokio::time::timeout_at(deadline, open).await.map_err(|_| Ended)?.ok_or(Ended)?;
        if self.cancelled() {
            return Err(Ended);
        }
        let (mut sink, mut stream) = socket.split();
        let hello = within(deadline, next(&mut stream)).await?;
        let Message::Text(hello) = hello else { return Err(Ended) };
        if hello.len() > MAXIMUM_HELLO_BYTES {
            return Err(Ended);
        }
        let hello: serde_json::Value = serde_json::from_str(hello.as_str()).map_err(|_| Ended)?;
        let version = hello.get("v").and_then(serde_json::Value::as_i64);
        if hello.get("type").and_then(serde_json::Value::as_str) != Some("hello") || version != Some(1) {
            return Err(Ended);
        }
        let b64 = base64::engine::general_purpose::STANDARD;
        let text = |name: &str| hello.get(name).and_then(serde_json::Value::as_str).unwrap_or("").to_string();
        let client_key = b64.decode(text("clientKey")).map_err(|_| Ended)?;
        let client_nonce = b64.decode(text("nonce")).map_err(|_| Ended)?;
        let mut nonce = [0u8; 16];
        getrandom::fill(&mut nonce).map_err(|_| Ended)?;
        let cipher =
            Arc::new(RelayCipher::new(host.identity().secret().as_ref(), &client_key, &client_nonce, &nonce, true).map_err(|_| Ended)?);
        let mut ready = JsonOut::new();
        ready.begin_object();
        ready.prop_str("type", Some("ready"));
        ready.prop_int("v", 1);
        ready.prop_str("serverKey", Some(&host.identity().public_key()));
        ready.prop_str("nonce", Some(&b64.encode(nonce)));
        ready.end_object();
        tokio::time::timeout_at(deadline, sink.send(Message::text(ready.finish()))).await.map_err(|_| Ended)?.map_err(|_| Ended)?;
        let auth = within(deadline, next(&mut stream)).await?;
        let Message::Binary(auth) = auth else { return Err(Ended) };
        let plaintext = cipher.open(&auth).map_err(|_| Ended)?;
        // A frame that is not JSON reads as an empty object, so it is refused as malformed (generically and counted on the direct
        // path) rather than ending the connection unanswered.
        let request = AuthRequest::from_bytes(&plaintext);
        let decision = host.identity().authorize(&request, host.allow_legacy()).map_err(|_| Ended)?;

        // From here every frame is sealed by the one writer.
        let (outbox, inbox) = mpsc::channel(OUTBOX);
        *lock(&self.outbox) = Some(outbox);
        tasks.spawn(writer(sink, cipher.clone(), inbox, self.clone()));

        let device_id = match decision {
            AuthDecision::Refused(refusal) => {
                if self.direct
                    && let Some(peer) = self.peer
                {
                    host.record_direct_failure(peer);
                }
                let reason = if self.direct { GENERIC_REFUSAL } else { refusal.reason() };
                let mut out = JsonOut::new();
                out.begin_object();
                out.prop_str("type", Some("auth_error"));
                out.prop_str("reason", Some(reason));
                out.end_object();
                self.send_and_wait(out.finish().into_bytes()).await;
                return Err(Ended);
            }
            AuthDecision::Accepted { device_id, device_token } => {
                let name = host.identity().devices().into_iter().find(|d| d.id == device_id).map(|d| d.name).unwrap_or_default();
                *lock(&self.device) = Some(MobileDevice { client_id: device_id.clone(), name, direct: self.direct });
                self.authenticated.store(true, Ordering::SeqCst);
                let mut out = JsonOut::new();
                out.begin_object();
                out.prop_str("type", Some("auth_ok"));
                out.prop_str("hostName", Some(&host.host_name()));
                out.prop_str("hostId", Some(&host.identity().host_id()));
                out.prop_str("appVersion", Some(&host.app_version()));
                out.prop_strings("capabilities", Some(&host.capabilities()));
                out.prop_strings("direct", Some(&host.direct_endpoints()));
                out.prop_str("deviceToken", device_token.as_deref());
                out.end_object();
                self.send(out.finish().into_bytes()).await;
                device_id
            }
        };
        host.authenticated();
        tasks.spawn(heartbeat(self.clone()));

        loop {
            let frame = match tokio::time::timeout(READ_IDLE, next(&mut stream)).await {
                Ok(Ok(frame)) => frame,
                _ => return Err(Ended),
            };
            let Message::Binary(frame) = frame else { continue };
            let plain = cipher.open(&frame).map_err(|_| Ended)?;
            let root: IndexMap<String, Box<RawValue>> = serde_json::from_slice(&plain).map_err(|_| Ended)?;
            let text = |name: &str| root.get(name).and_then(|raw| serde_json::from_str::<String>(raw.get()).ok());
            if text("type").as_deref() == Some("ping") {
                self.send(br#"{"type":"pong"}"#.to_vec()).await;
                continue;
            }
            // Screen share's signals (`screen-*`) are dropped while it is off (`screen::ENABLED`), and any other typed frame ignored.
            if root.contains_key("type") {
                continue;
            }
            let (Some(id), Some(method), Some(path)) = (text("id"), text("method"), text("path")) else { continue };
            if !(1..=64).contains(&id.encode_utf16().count()) || !matches!(method.as_str(), "GET" | "POST") || !path.starts_with('/') {
                continue;
            }
            if path.len() > MAXIMUM_PATH_BYTES {
                self.send(reply_frame(&id, &MobileReply::error(414, "Path exceeds limit.", Some("badPath")))).await;
                continue;
            }
            // Inserted before the request can complete and remove itself; the lock never held across the await below.
            let admitted = {
                let mut inflight = lock(&self.inflight);
                let free = !inflight.contains(&id) && inflight.len() < MAXIMUM_IN_FLIGHT;
                if free {
                    inflight.insert(id.clone());
                }
                free
            };
            if !admitted {
                self.send(reply_frame(&id, &MobileReply::error(429, "Too many pending requests.", None))).await;
                continue;
            }
            let body = root.get("body").cloned();
            requests.spawn(handle(self.clone(), host.clone(), device_id.clone(), id, method, path, body));
        }
    }
}

/// One request (C# `Handle`): a phone no longer paired is not answered; the reply framed, refused with 413 when the relay would close
/// the connection for it, and its after-send run once it was written; a route that panics answers 500.
async fn handle<H: ConnectionHost>(
    connection: Arc<Connection>,
    host: Arc<H>,
    device_id: String,
    id: String,
    method: String,
    path: String,
    body: Option<Box<RawValue>>,
) {
    struct Slot(Arc<Connection>, String);
    impl Drop for Slot {
        fn drop(&mut self) {
            lock(&self.0.inflight).remove(&self.1);
        }
    }
    let _slot = Slot(connection.clone(), id.clone());
    if connection.cancelled() || !host.identity().contains(&device_id, host.allow_legacy()) {
        return;
    }
    let device =
        lock(&connection.device).clone().unwrap_or(MobileDevice { client_id: device_id, name: String::new(), direct: connection.direct });
    let route = host.handler().handle(device, method, path, body);
    // The route on a task of its own: a panic is a 500, never the connection's end; dropping this request (the connection ended)
    // aborts it.
    struct Abort(tokio::task::AbortHandle);
    impl Drop for Abort {
        fn drop(&mut self) {
            self.0.abort();
        }
    }
    let task = tokio::spawn(route);
    let _abort = Abort(task.abort_handle());
    let reply = match task.await {
        Ok(reply) => reply,
        Err(error) if error.is_cancelled() => return,
        Err(_) => {
            connection.send(reply_frame(&id, &MobileReply::error(500, "The mobile request failed.", None))).await;
            return;
        }
    };
    let frame = reply_frame(&id, &reply);
    // A reply the relay would close the connection for is refused instead, so the phone is told and keeps its tunnel.
    if frame.len() > MAXIMUM_PLAINTEXT {
        connection.send(reply_frame(&id, &MobileReply::error(413, "Response exceeds the mobile display limit.", None))).await;
        return;
    }
    match reply.after_send {
        None => connection.send(frame).await,
        Some(after) => {
            if connection.send_and_wait(frame).await && !connection.cancelled() {
                // Once started it runs to its end on a task of its own (C# awaits it whatever the connection does next): the
                // connection ending aborts this request, never the after-send (an app update's restart).
                let _ = tokio::spawn(after()).await;
            }
        }
    }
}

/// `{"id":…,"status":…,"body":…}` as `Wire.Json` writes C#'s anonymous object, the body being the route's JSON text.
pub fn reply_frame(id: &str, reply: &MobileReply) -> Vec<u8> {
    let mut out = JsonOut::new();
    out.begin_object();
    out.prop_str("id", Some(id));
    out.prop_int("status", reply.status as i64);
    out.name("body");
    out.raw(&reply.body);
    out.end_object();
    out.finish().into_bytes()
}

/// The one writer: seals each frame in the order it came and sends it; a frame over the limit or a write that fails ends the
/// connection.
async fn writer<S>(
    mut sink: futures_util::stream::SplitSink<WebSocketStream<S>, Message>,
    cipher: Arc<RelayCipher>,
    mut inbox: mpsc::Receiver<Outgoing>,
    connection: Arc<Connection>,
) where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    while let Some(outgoing) = inbox.recv().await {
        let sealed = (outgoing.plaintext.len() <= MAXIMUM_PLAINTEXT).then(|| cipher.seal(&outgoing.plaintext).ok()).flatten();
        let written = match sealed {
            Some(frame) => sink.send(Message::binary(frame)).await.is_ok(),
            None => false,
        };
        if let Some(sent) = outgoing.sent {
            let _ = sent.send(written);
        }
        if !written {
            connection.cancel();
            break;
        }
    }
    cipher.dispose();
    let _ = sink.close().await;
}

/// How many frames may wait for the writer before a sender waits too.
pub const OUTBOX: usize = 16;

/// The host's encrypted ping every 20 s.
async fn heartbeat(connection: Arc<Connection>) {
    loop {
        tokio::time::sleep(HEARTBEAT).await;
        connection.send(br#"{"type":"ping"}"#.to_vec()).await;
    }
}

/// The next data frame (a close, an error or the end of the stream ends the session; pings and pongs are the stack's).
async fn next<S>(stream: &mut futures_util::stream::SplitStream<WebSocketStream<S>>) -> Step<Message>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    loop {
        match stream.next().await {
            Some(Ok(message @ (Message::Text(_) | Message::Binary(_)))) => {
                if message.len() > MAXIMUM_FRAME {
                    return Err(Ended);
                }
                return Ok(message);
            }
            Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_))) => continue,
            Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return Err(Ended),
        }
    }
}

async fn within<T>(deadline: Instant, step: impl std::future::Future<Output = Step<T>>) -> Step<T> {
    tokio::time::timeout_at(deadline, step).await.map_err(|_| Ended)?
}

/// Waits until the flag reads true (or its sender is gone); the borrow it takes ends before this returns, so a `select!` over it stays
/// `Send`.
async fn until_true(flag: &mut watch::Receiver<bool>) {
    let _ = flag.wait_for(|set| *set).await;
}
