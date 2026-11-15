//! The sharing host's strict HTTP/1.1 server (C# `RemoteHttpListener`, `RemoteHttpRequest`, `RemoteHttpResponse`,
//! R\RemoteHttpListener.cs; M/Remote/HTTPServer.swift; plan §1.6, D7), hand-written over tokio TCP, never hyper. One listener per
//! address through `Network::bind_exclusive` (backlog 64, `SO_EXCLUSIVEADDRUSE` on Windows). Each connection carries one request and
//! closes. A peer is reset at accept, before anything is read, when the filter says no (or fails) or it already holds its share; the
//! 65th connection likewise. The head decides how much body is read: 0 means none, and the handler answers the head alone. The
//! listener's own refusals (400, 413, 431) carry no protocol header: they come before the protocol is reached. 15 s for head and body
//! (60 s for a body over 512 KiB), then 60 s for the handler and the answer (a handler still running then is left to finish alone, its
//! answer dropped); an answer given with request bytes possibly still arriving is followed by discarding at most 64 KiB of them for a
//! second. 50 accept failures in a row end the listener. Owner: W8-D.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use mighty_platform::{BoxFuture, Network};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::{Instant, timeout_at};

use crate::remote::address::{canonical, unmapped};
use crate::remote::wire::{protocol, to_bytes};

/// `HeaderLimit`: the head may be this many bytes before its blank line.
pub const MAXIMUM_HEAD_BYTES: usize = 8192;
/// The request line and at most 33 header lines, as on the Mac.
pub const MAXIMUM_HEAD_LINES: usize = 34;
pub const BACKLOG: u32 = 64;
/// `MaximumOpenConnections`: connections open at once across every peer, as on the Mac.
pub const MAXIMUM_CONNECTIONS: usize = 64;
pub const MAXIMUM_CONNECTIONS_PER_PEER: usize = 8;
pub const READ_DEADLINE: Duration = Duration::from_secs(15);
pub const HANDLER_DEADLINE: Duration = Duration::from_secs(60);
pub const MAXIMUM_ACCEPT_FAILURES: usize = 50;
/// `DefaultBodyLimit`.
pub const DEFAULT_BODY_LIMIT: usize = 512 * 1024;
const DRAIN_BYTES: usize = 64 * 1024;
const FIRST_BODY_BUFFER: usize = 64 * 1024;
/// The most body any head may ask for (C# `AttachmentSupport.MaximumRequestBytes`).
const MAXIMUM_BODY: usize = mighty_run::attachments::MAXIMUM_REQUEST_BYTES;
/// Headers the listener writes itself; a handler's copy is dropped so it can never change how the answer is framed.
const FRAMING_HEADERS: [&str; 4] = ["content-length", "connection", "transfer-encoding", "content-type"];

/// C# `RemoteHttpRequest(Method, Target, Headers, Body, RemoteAddress)`: header names lower-case; the peer's canonical spelling.
#[derive(Clone)]
pub struct HttpRequest {
    pub method: String,
    pub target: String,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
    pub remote_address: String,
}

/// The headers are left out: an `Authorization` is a key.
impl std::fmt::Debug for HttpRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpRequest").field("method", &self.method).field("target", &self.target).field("body", &self.body.len()).finish()
    }
}

/// C# `RemoteHttpResponse(Status, Body, Headers?)`.
#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
    pub headers: Vec<(String, String)>,
}

impl HttpResponse {
    /// `RemoteHttpResponse.Json(status, value, headers)`: the value as the remote wire writes it.
    pub fn json(status: u16, value: &Value, headers: &[(&str, &str)]) -> HttpResponse {
        HttpResponse { status, body: to_bytes(value), headers: headers.iter().map(|(n, v)| (n.to_string(), v.to_string())).collect() }
    }
}

/// What a filter, a limit or a handler that failed gives: its connection ends, nothing said (C#'s exception, traced by type only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failed;

pub type Handler = Arc<dyn Fn(HttpRequest) -> BoxFuture<'static, Result<HttpResponse, Failed>> + Send + Sync>;
pub type BodyLimit = Arc<dyn Fn(&HttpRequest) -> Result<usize, Failed> + Send + Sync>;
pub type AcceptPeer = Arc<dyn Fn(IpAddr) -> Result<bool, Failed> + Send + Sync>;

/// How a listener is made (C#'s constructor parameters and its `internal init` deadlines, which the checks shorten).
#[derive(Clone)]
pub struct ListenerOptions {
    pub body_limit: Option<BodyLimit>,
    pub accept_peer: Option<AcceptPeer>,
    pub maximum_connections_per_peer: Option<usize>,
    pub request_deadline: Duration,
    pub handler_deadline: Duration,
    pub drain_deadline: Duration,
    /// Called after a connection gives back its slot; checks wait on it instead of sleeping.
    pub slot_released: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl Default for ListenerOptions {
    fn default() -> Self {
        ListenerOptions {
            body_limit: None,
            accept_peer: None,
            maximum_connections_per_peer: None,
            request_deadline: protocol::REQUEST_TIMEOUT,
            handler_deadline: HANDLER_DEADLINE,
            drain_deadline: Duration::from_secs(1),
            slot_released: None,
        }
    }
}

#[derive(Default)]
struct Slots {
    active: usize,
    peers: HashMap<String, usize>,
    open: HashMap<u64, JoinHandle<()>>,
    started: bool,
    closing: bool,
    accepting: Option<JoinHandle<()>>,
}

struct Inner {
    network: Arc<dyn Network>,
    address: IpAddr,
    port: u16,
    handler: Handler,
    options: ListenerOptions,
    slots: Mutex<Slots>,
    stop: watch::Sender<bool>,
    accept_ended: AtomicBool,
    bound: AtomicU16,
    next: AtomicU64,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// C# `RemoteHttpListener`: a bounded HTTP/1.1 endpoint on one address.
#[derive(Clone)]
pub struct HttpListener(Arc<Inner>);

/// Why a listener did not start.
#[derive(Debug)]
pub enum StartError {
    /// `http.error.alreadyStarted`: a listener starts once, and never after it was disposed.
    AlreadyStarted,
    /// The bind failed (its words, never a path).
    Bind(String),
}

impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StartError::AlreadyStarted => f.write_str(&mighty_run::strings::get("http.error.alreadyStarted")),
            StartError::Bind(error) => f.write_str(error),
        }
    }
}

impl HttpListener {
    pub fn new(network: Arc<dyn Network>, address: IpAddr, port: u16, handler: Handler, options: ListenerOptions) -> HttpListener {
        HttpListener(Arc::new(Inner {
            network,
            address,
            port,
            handler,
            options,
            slots: Mutex::new(Slots::default()),
            stop: watch::channel(false).0,
            accept_ended: AtomicBool::new(false),
            bound: AtomicU16::new(0),
            next: AtomicU64::new(0),
        }))
    }

    /// The port bound (0 before `start`).
    pub fn port(&self) -> u16 {
        self.0.bound.load(Ordering::SeqCst)
    }

    /// `IsListening`: accepting connections; false before `start`, after dispose, and once accepting has failed for good.
    pub fn is_listening(&self) -> bool {
        let slots = lock(&self.0.slots);
        slots.started && !slots.closing && !self.0.accept_ended.load(Ordering::SeqCst)
    }

    /// `Start()`: binds the one address (no dual-stack, no shared port) and starts accepting; the port bound. Called inside a tokio
    /// runtime.
    pub fn start(&self) -> Result<u16, StartError> {
        let inner = &self.0;
        let mut slots = lock(&inner.slots);
        if slots.started || slots.closing {
            return Err(StartError::AlreadyStarted);
        }
        let std_listener = inner
            .network
            .bind_exclusive(SocketAddr::new(inner.address, inner.port), BACKLOG)
            .map_err(|e| StartError::Bind(e.to_string()))?;
        std_listener.set_nonblocking(true).map_err(|e| StartError::Bind(e.to_string()))?;
        let listener = TcpListener::from_std(std_listener).map_err(|e| StartError::Bind(e.to_string()))?;
        let port = listener.local_addr().map_err(|e| StartError::Bind(e.to_string()))?.port();
        inner.bound.store(port, Ordering::SeqCst);
        slots.started = true;
        let accepting = self.clone();
        slots.accepting = Some(tokio::spawn(async move { accepting.accept_loop(listener).await }));
        Ok(port)
    }

    async fn accept_loop(self, listener: TcpListener) {
        let mut stop = self.0.stop.subscribe();
        let mut failures = 0;
        loop {
            if *stop.borrow() {
                break;
            }
            let accepted = tokio::select! {
                accepted = listener.accept() => accepted,
                _ = stop.changed() => break,
            };
            match accepted {
                Ok((stream, peer)) => {
                    failures = 0;
                    self.admit(stream, peer);
                }
                // A connection reset or aborted while it waited says nothing about the listener: the next one is accepted at once.
                Err(error) if !counts_as_accept_failure(error.kind()) => continue,
                Err(_) => {
                    failures += 1;
                    if failures >= MAXIMUM_ACCEPT_FAILURES {
                        break;
                    }
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_millis(100)) => {}
                        _ = stop.changed() => break,
                    }
                }
            }
        }
        // Accepting failed for good (or the listener is stopping): the port is closed instead of left taking connections.
        drop(listener);
        self.0.accept_ended.store(true, Ordering::SeqCst);
    }

    /// Takes the connection or drops it unread: the filter first, then the overall and per-peer caps.
    fn admit(&self, stream: TcpStream, peer: SocketAddr) {
        let inner = &self.0;
        let remote = unmapped(peer.ip());
        let key = canonical(&remote.to_string());
        let allowed = inner.options.accept_peer.as_ref().is_none_or(|filter| filter(remote).unwrap_or(false));
        let mut slots = lock(&inner.slots);
        let within = inner.options.maximum_connections_per_peer.is_none_or(|cap| slots.peers.get(&key).copied().unwrap_or(0) < cap);
        if !(allowed && !slots.closing && slots.active < MAXIMUM_CONNECTIONS && within) {
            drop(slots);
            abort(stream);
            return;
        }
        slots.active += 1;
        *slots.peers.entry(key.clone()).or_default() += 1;
        let id = inner.next.fetch_add(1, Ordering::SeqCst);
        let serving = self.clone();
        // The task cannot finish before it is recorded: its last step takes this lock.
        let task = tokio::spawn(async move { serving.serve(stream, key, id).await });
        slots.open.insert(id, task);
    }

    async fn serve(self, mut stream: TcpStream, peer: String, id: u64) {
        let mut slot = Slot { listener: self.clone(), peer, id, released: false, closed: false };
        let mut stop = self.0.stop.subscribe();
        if *stop.borrow() {
            abort(stream);
            return;
        }
        let deadline = Instant::now() + self.0.options.request_deadline;
        let answered = tokio::select! {
            answered = self.read_and_answer(&mut stream, &slot.peer, deadline) => answered,
            _ = stop.changed() => { abort(stream); return; }
        };
        let Some((response, unread, deadline)) = answered else { return };
        let written = tokio::select! {
            written = timeout_at(deadline, write(&mut stream, &response)) => matches!(written, Ok(Ok(()))),
            _ = stop.changed() => { abort(stream); return; }
        };
        let drain = written && unread;
        // A connection that is done gives its slot back before the peer can see the end of the answer, so the peer's next connection
        // is never refused for this one. A draining one holds its slot until the drain ends.
        if !drain {
            slot.release();
        }
        let _ = stream.shutdown().await;
        if drain {
            tokio::select! {
                _ = self.drain(&mut stream) => {}
                _ = stop.changed() => { abort(stream); return; }
            }
        }
        slot.release();
    }

    /// Discards what is still arriving after an early answer (at most 64 KiB, for the drain deadline), so the peer reads the answer
    /// instead of a reset. Nothing read here is parsed.
    async fn drain(&self, stream: &mut TcpStream) {
        let until = Instant::now() + self.0.options.drain_deadline;
        let mut sink = [0u8; 8192];
        let mut total = 0;
        while total < DRAIN_BYTES {
            match timeout_at(until, stream.read(&mut sink)).await {
                Ok(Ok(read)) if read > 0 => total += read,
                _ => break,
            }
        }
    }

    /// Reads the head, asks how much body it may bring, reads that much and no more, and asks the handler. None: nothing is answered
    /// (the peer went away, a deadline passed, or a filter, limit or handler failed). Unread is true when the answer is given with
    /// request bytes possibly still arriving; the deadline is the one the answer is written within.
    async fn read_and_answer(&self, stream: &mut TcpStream, peer: &str, mut deadline: Instant) -> Option<(HttpResponse, bool, Instant)> {
        let refuse = |status, key: &str| Some((refusal(status, key), true, deadline));
        let mut buffer = vec![0u8; MAXIMUM_HEAD_BYTES + 4];
        let mut filled = 0;
        let mut scanned = 0;
        let end = loop {
            if let Some(end) = head_end(&buffer[..filled], &mut scanned) {
                break end;
            }
            if filled == buffer.len() {
                return refuse(431, "http.error.headerTooLarge");
            }
            let read = timeout_at(deadline, stream.read(&mut buffer[filled..])).await.ok()?.ok()?;
            if read == 0 {
                return None;
            }
            filled += read;
        };
        let Ok(text) = std::str::from_utf8(&buffer[..end]) else { return refuse(400, "http.error.badHeaders") };
        let lines: Vec<&str> = text.split("\r\n").collect();
        let start: Vec<&str> = lines[0].split(' ').collect();
        if lines.len() > MAXIMUM_HEAD_LINES
            || start.len() != 3
            || !matches!(start[0], "GET" | "POST")
            || start[2] != "HTTP/1.1"
            || !start[1].starts_with('/')
            || has_control(lines[0])
        {
            return refuse(400, "http.error.badRequest");
        }
        let mut headers = HashMap::new();
        for line in &lines[1..] {
            // No folded lines, and no bare CR, LF or other control character smuggled inside a line.
            let Some(colon) = line.find(':') else { return refuse(400, "http.error.badHeader") };
            if line.starts_with([' ', '\t']) || has_control(line) {
                return refuse(400, "http.error.badHeader");
            }
            let raw = &line[..colon];
            let name = raw.to_lowercase();
            if !header_name(raw) || headers.contains_key(&name) {
                return refuse(400, "http.error.duplicateHeader");
            }
            headers.insert(name, line[colon + 1..].trim_matches([' ', '\t']).to_string());
        }
        if headers.contains_key("transfer-encoding") {
            return refuse(400, "http.error.lengthRequired");
        }
        let mut head = HttpRequest { method: start[0].into(), target: start[1].into(), headers, body: vec![], remote_address: peer.into() };
        let asked = match &self.0.options.body_limit {
            Some(limit) => limit(&head).ok()?,
            None => DEFAULT_BODY_LIMIT,
        };
        let limit = asked.min(MAXIMUM_BODY);
        let length_text = head.headers.get("content-length").map_or("0", String::as_str);
        let length = match length_text.parse::<usize>() {
            Ok(length) if (1..=9).contains(&length_text.len()) && length_text.bytes().all(|b| b.is_ascii_digit()) => length,
            _ => return refuse(413, "http.error.badLength"),
        };
        if length > limit && limit != 0 {
            return refuse(413, "http.error.badLength");
        }
        // Nothing of the body is read for a head the server refuses it to: the handler answers the head alone.
        if limit == 0 && length > 0 {
            return self.handle(head, true).await;
        }
        if length > DEFAULT_BODY_LIMIT {
            deadline = Instant::now() + protocol::UPLOAD_TIMEOUT;
        }
        let already = filled - (end + 4);
        if already > length {
            return Some((refusal(400, "http.error.oneRequest"), true, deadline));
        }
        // The body buffer grows as bytes arrive, up to the length: a head alone never makes the listener hold its whole length.
        let mut body = vec![0u8; length.min(FIRST_BODY_BUFFER).max(already)];
        body[..already].copy_from_slice(&buffer[end + 4..filled]);
        let mut got = already;
        while got < length {
            if got == body.len() {
                body.resize(length.min(body.len() * 2), 0);
            }
            let read = timeout_at(deadline, stream.read(&mut body[got..])).await.ok()?.ok()?;
            if read == 0 {
                return None;
            }
            got += read;
        }
        body.truncate(length);
        let mut probe = [0u8; 1];
        if matches!(stream.try_read(&mut probe), Ok(read) if read > 0) {
            return Some((refusal(400, "http.error.oneRequest"), true, deadline));
        }
        head.body = body;
        self.handle(head, false).await
    }

    /// The arrival deadline covered slow delivery; the handler and the answer get their own. A handler still running at it is left to
    /// finish on its own and its answer dropped.
    async fn handle(&self, request: HttpRequest, unread: bool) -> Option<(HttpResponse, bool, Instant)> {
        let deadline = Instant::now() + self.0.options.handler_deadline;
        let task = tokio::spawn((self.0.handler)(request));
        match timeout_at(deadline, task).await {
            Ok(Ok(Ok(response))) => Some((response, unread, deadline)),
            _ => None,
        }
    }

    /// `DisposeAsync`: stops accepting, resets every live connection and waits for their tasks; a handler still running is not waited
    /// for.
    pub async fn dispose(&self) {
        let (accepting, open) = {
            let mut slots = lock(&self.0.slots);
            slots.closing = true;
            (slots.accepting.take(), slots.open.drain().map(|(_, task)| task).collect::<Vec<_>>())
        };
        self.0.stop.send_replace(true);
        if let Some(accepting) = accepting {
            let _ = accepting.await;
        }
        for task in open {
            let _ = task.await;
        }
    }
}

/// A connection's slot, always given back: on the normal path, and when its task ends any other way.
struct Slot {
    listener: HttpListener,
    peer: String,
    id: u64,
    released: bool,
    closed: bool,
}

impl Slot {
    fn release(&mut self) {
        if self.released {
            return;
        }
        self.released = true;
        {
            let mut slots = lock(&self.listener.0.slots);
            slots.active = slots.active.saturating_sub(1);
            if let Some(count) = slots.peers.get_mut(&self.peer) {
                *count -= 1;
                if *count == 0 {
                    slots.peers.remove(&self.peer);
                }
            }
        }
        if let Some(released) = &self.listener.0.options.slot_released {
            released();
        }
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.release();
        if !self.closed {
            self.closed = true;
            lock(&self.listener.0.slots).open.remove(&self.id);
        }
    }
}

/// `CountsAsAcceptFailure`: an accept error counts toward ending the listener, unless it is one waiting connection's own reset or abort.
pub fn counts_as_accept_failure(kind: std::io::ErrorKind) -> bool {
    !matches!(kind, std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted)
}

/// `PeerAddress`: the peer as the filter sees it: an IPv4-mapped address as its IPv4, no IPv6 scope.
pub fn peer_address(peer: SocketAddr) -> IpAddr {
    unmapped(peer.ip())
}

/// Drops a connection with a reset, nothing said.
fn abort(stream: TcpStream) {
    let _ = stream.set_zero_linger();
    drop(stream);
}

fn refusal(status: u16, key: &str) -> HttpResponse {
    HttpResponse::json(status, &serde_json::json!({ "error": mighty_run::strings::get(key) }), &[])
}

fn has_control(text: &str) -> bool {
    text.chars().any(|c| c < ' ' && c != '\t' || c == '\x7f')
}

fn header_name(name: &str) -> bool {
    !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

fn head_end(buffer: &[u8], scanned: &mut usize) -> Option<usize> {
    let from = scanned.saturating_sub(3);
    if let Some(at) = buffer.get(from..).and_then(|rest| rest.windows(4).position(|w| w == b"\r\n\r\n")) {
        return Some(from + at);
    }
    *scanned = buffer.len();
    None
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        202 => "Accepted",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        409 => "Conflict",
        413 => "Payload Too Large",
        415 => "Unsupported Media Type",
        426 => "Upgrade Required",
        429 => "Too Many Requests",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Response",
    }
}

async fn write(stream: &mut TcpStream, response: &HttpResponse) -> std::io::Result<()> {
    let mut head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\n",
        response.status,
        reason(response.status),
        response.body.len()
    );
    for (name, value) in &response.headers {
        if header_name(name) && !has_control(value) && !FRAMING_HEADERS.iter().any(|f| f.eq_ignore_ascii_case(name)) {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(&response.body).await?;
    stream.flush().await
}
