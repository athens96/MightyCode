//! The in-process relay (W8-A, D15): a tokio-tungstenite server on loopback port 0 speaking `relay/src/protocol.ts`'s query validation
//! and close codes (4400 for a malformed query, 4401 for a host socket whose `hostToken` does not hash to its `serverId`), the control
//! socket's `connected` notice, and a scripted phone (hello → ready → auth → requests) on each host data socket. It never forwards to a
//! real phone: a host data socket is the phone's end itself, as C#'s checks' Kestrel relay does it. Owner: W8-A.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use mighty_platform::BoxFuture;
use mighty_remote::relay::cipher::{self, RelayCipher};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

pub type Socket = WebSocketStream<TcpStream>;

/// `mobile-identity.json` in a folder through the shared codec (the Windows store's format on any OS), written as C#'s `Save` writes
/// it: beside the file, then moved over it (so a folder that may not be written refuses the save).
pub struct FileStore(pub std::path::PathBuf);

impl FileStore {
    pub fn file(&self) -> std::path::PathBuf {
        self.0.join(mighty_platform::pairing::identity_file::FILE)
    }
}

impl mighty_platform::PairingStore for FileStore {
    fn load(&self) -> mighty_platform::Result<Option<mighty_platform::PairingIdentity>> {
        match std::fs::read_to_string(self.file()) {
            Ok(text) => mighty_platform::pairing::identity_file::decode(&text).map(Some),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(mighty_platform::PlatformError::Io(error)),
        }
    }
    fn save(&self, identity: &mighty_platform::PairingIdentity) -> mighty_platform::Result<()> {
        let temporary = self.0.join(format!("{}.{}", mighty_platform::pairing::identity_file::FILE, uuid::Uuid::new_v4().simple()));
        let written = std::fs::write(&temporary, mighty_platform::pairing::identity_file::encode(identity)?)
            .and_then(|_| std::fs::rename(&temporary, self.file()));
        if written.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        Ok(written?)
    }
}
/// What one control socket does once open: its number (1 for the first) and the socket.
pub type ControlScript = Arc<dyn Fn(usize, Socket) -> BoxFuture<'static, ()> + Send + Sync>;
/// What the phone does on one host data socket: the connection id and the socket.
pub type PhoneScript = Arc<dyn Fn(String, Socket) -> BoxFuture<'static, ()> + Send + Sync>;

/// A socket's validated query (`parseSocketParams`), or the close code it gets.
fn validate(query: &str) -> Result<(String, Option<String>), u16> {
    let mut params: HashMap<String, String> = HashMap::new();
    for pair in query.split('&').filter(|p| !p.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let decode = |text: &str| percent_decode(text);
        params.entry(decode(key)).or_insert_with(|| decode(value));
    }
    let server_id_ok = |v: &str| {
        let b = v.as_bytes();
        !b.is_empty()
            && b.len() <= 128
            && b[0].is_ascii_alphanumeric()
            && b[1..].iter().all(|c| c.is_ascii_alphanumeric() || b"._:-".contains(c))
    };
    let connection_ok = |v: &str| (8..=64).contains(&v.len()) && v.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-');
    if params.get("v").map(String::as_str) != Some("1") {
        return Err(4400);
    }
    let server_id = params.get("serverId").filter(|s| server_id_ok(s)).ok_or(4400u16)?.clone();
    let role = params.get("role").map(String::as_str).ok_or(4400u16)?;
    let connection = params.get("connectionId").cloned();
    if connection.as_deref().is_some_and(|c| !connection_ok(c)) || !matches!(role, "server" | "client") {
        return Err(4400);
    }
    if role == "server" {
        let token = params.get("hostToken");
        if token.is_some_and(|t| !((32..=64).contains(&t.len()) && t.bytes().all(|c| c.is_ascii_hexdigit()))) {
            return Err(4400);
        }
        // `ownsServerId`: a host socket speaks for its server id only with the token that hashes to it.
        let owns = token.is_some_and(|t| Sha256::digest(t.as_bytes()).iter().map(|b| format!("{b:02x}")).collect::<String>() == server_id);
        if !owns {
            return Err(4401);
        }
    }
    Ok((server_id, connection))
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        let escaped = (bytes[at] == b'%' && at + 2 < bytes.len())
            .then(|| std::str::from_utf8(&bytes[at + 1..at + 3]).ok().and_then(|hex| u8::from_str_radix(hex, 16).ok()))
            .flatten();
        match escaped {
            Some(byte) => {
                out.push(byte);
                at += 3;
            }
            None => {
                out.push(bytes[at]);
                at += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A running fake relay; dropping it stops accepting (sockets already open run until their scripts end).
pub struct FakeRelay {
    /// `ws://127.0.0.1:<port>`.
    pub address: String,
    controls: Arc<AtomicUsize>,
    refused: Arc<std::sync::Mutex<Vec<u16>>>,
    accept: JoinHandle<()>,
}

impl Drop for FakeRelay {
    fn drop(&mut self) {
        self.accept.abort();
    }
}

impl FakeRelay {
    /// Listens on loopback port 0: each control socket runs `control`, each host data socket `phone`.
    pub async fn start(control: ControlScript, phone: PhoneScript) -> FakeRelay {
        let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0))).await.unwrap();
        let address = format!("ws://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let controls = Arc::new(AtomicUsize::new(0));
        let refused = Arc::new(std::sync::Mutex::new(vec![]));
        let (counter, refusals) = (controls.clone(), refused.clone());
        let accept = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let (control, phone, counter, refusals) = (control.clone(), phone.clone(), counter.clone(), refusals.clone());
                tokio::spawn(async move {
                    let mut query = String::new();
                    #[allow(clippy::result_large_err, reason = "tungstenite's own callback type")]
                    let callback = |request: &Request, response: Response| {
                        query = request.uri().query().unwrap_or("").to_string();
                        Ok(response)
                    };
                    let Ok(mut socket) = tokio_tungstenite::accept_hdr_async(stream, callback).await else { return };
                    match validate(&query) {
                        Err(code) => {
                            refusals.lock().unwrap().push(code);
                            let frame = CloseFrame { code: CloseCode::from(code), reason: "".into() };
                            let _ = socket.close(Some(frame)).await;
                        }
                        Ok((_, None)) => {
                            let number = counter.fetch_add(1, Ordering::SeqCst) + 1;
                            control(number, socket).await;
                        }
                        Ok((_, Some(connection))) => phone(connection, socket).await,
                    }
                });
            }
        });
        FakeRelay { address, controls, refused, accept }
    }

    /// How many control sockets were opened.
    pub fn controls(&self) -> usize {
        self.controls.load(Ordering::SeqCst)
    }

    /// The close codes sockets were refused with.
    pub fn refused(&self) -> Vec<u16> {
        self.refused.lock().unwrap().clone()
    }
}

/// A control socket that reads (answering pings) until the host closes it.
pub async fn listen(mut socket: Socket) {
    while let Some(Ok(message)) = socket.next().await {
        if matches!(message, Message::Close(_)) {
            break;
        }
    }
}

/// A control socket that tells the host of `connection` first, then listens.
pub async fn notice_then_listen(mut socket: Socket, connection: &str) {
    let notice = format!(r#"{{"type":"connected","connectionId":"{connection}"}}"#);
    let _ = socket.send(Message::text(notice)).await;
    listen(socket).await;
}

/// A control socket accepted and then never read: its pings go unanswered (a socket the network left half-open).
pub async fn never_read(socket: Socket) {
    let _keep = socket;
    std::future::pending::<()>().await;
}

/// A control socket closed by the relay with `code`.
pub async fn close_with(mut socket: Socket, code: u16) {
    let _ = socket.close(Some(CloseFrame { code: CloseCode::from(code), reason: "".into() })).await;
    while let Some(Ok(_)) = socket.next().await {}
}

/// A scripted phone on a host data socket: the hello with a fresh key, the host's ready, then encrypted frames each way.
pub struct ScriptedPhone {
    socket: Socket,
    secret: [u8; 32],
    nonce: [u8; 16],
    cipher: Option<RelayCipher>,
    /// The host's ready frame (`serverKey`, `nonce`).
    pub ready: Value,
}

impl ScriptedPhone {
    pub fn new(socket: Socket) -> ScriptedPhone {
        let mut secret = [0u8; 32];
        let mut nonce = [0u8; 16];
        getrandom::fill(&mut secret).unwrap();
        getrandom::fill(&mut nonce).unwrap();
        ScriptedPhone { socket, secret, nonce, cipher: None, ready: Value::Null }
    }

    /// Sends the plaintext hello and reads the ready; the session key made from it.
    pub async fn hello(&mut self) -> &Value {
        let b64 = base64::engine::general_purpose::STANDARD;
        let hello = serde_json::json!({"type":"hello","v":1,"clientKey": b64.encode(cipher::public_key(&self.secret)),"nonce": b64.encode(self.nonce)});
        self.socket.send(Message::text(hello.to_string())).await.unwrap();
        let ready = self.next_text().await;
        let server = b64.decode(ready["serverKey"].as_str().unwrap()).unwrap();
        let server_nonce = b64.decode(ready["nonce"].as_str().unwrap()).unwrap();
        self.cipher = Some(RelayCipher::new(&self.secret, &server, &self.nonce, &server_nonce, false).unwrap());
        self.ready = ready;
        &self.ready
    }

    /// One encrypted frame of JSON text.
    pub async fn send(&mut self, json: &str) {
        let frame = self.cipher.as_ref().unwrap().seal(json.as_bytes()).unwrap();
        self.socket.send(Message::binary(frame)).await.unwrap();
    }

    /// The next encrypted frame, opened; None when the socket ended first.
    pub async fn receive(&mut self) -> Option<Value> {
        loop {
            match self.socket.next().await? {
                Ok(Message::Binary(frame)) => {
                    let plain = self.cipher.as_ref().unwrap().open(&frame).unwrap();
                    return Some(serde_json::from_slice(&plain).unwrap());
                }
                Ok(Message::Close(_)) | Err(_) => return None,
                Ok(_) => continue,
            }
        }
    }

    /// The next frame that is not the host's ping.
    pub async fn receive_reply(&mut self) -> Option<Value> {
        loop {
            let value = self.receive().await?;
            if value["type"] != "ping" {
                return Some(value);
            }
        }
    }

    async fn next_text(&mut self) -> Value {
        loop {
            if let Some(Ok(Message::Text(text))) = self.socket.next().await {
                return serde_json::from_str(text.as_str()).unwrap();
            }
        }
    }

    /// The socket's end: reads until the host closes it.
    pub async fn until_closed(&mut self) {
        while let Some(Ok(message)) = self.socket.next().await {
            if matches!(message, Message::Close(_)) {
                break;
            }
        }
    }
}
