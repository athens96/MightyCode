//! MC-32's generic webhook listener: off by default, on `127.0.0.1:<port>` only (never another address). A trigger's address is
//! `http://127.0.0.1:<port>/hooks/<id>` (`id`: 16 hex digits of the trigger's key). Each POST must be signed with the trigger's own
//! secret (kept in the OS keychain): `X-Mighty-Signature-256: sha256=<hex HMAC-SHA256 of the body>` (GitHub's
//! `X-Hub-Signature-256` is read too). The body is capped at [`MAX_BODY`]; a delivery id (`X-Mighty-Delivery`, `X-GitHub-Delivery`,
//! else the body's hash) seen before is answered and dropped. People who want the outside world to reach it put their own tunnel in
//! front; the app never opens one.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mighty_core::automation::watch::Seen;
use sha2::{Digest as _, Sha256};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};

/// The largest body taken, and the largest head (request line and headers).
pub const MAX_BODY: usize = 256 * 1024;
const MAX_HEAD: usize = 16 * 1024;
/// How long a connection may take to send its request.
const READ_TIMEOUT: Duration = Duration::from_secs(10);
/// The most connections served at once.
const AT_ONCE: usize = 8;

/// HMAC-SHA256 (RFC 2104).
pub fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut block = [0u8; 64];
    if key.len() > 64 {
        block[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(block.map(|b| b ^ 0x36));
    inner.update(message);
    let inner = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(block.map(|b| b ^ 0x5c));
    outer.update(inner);
    outer.finalize().into()
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Whether `header` (`sha256=<hex>`) is the body's signature under `secret`; compared in constant time.
pub fn verify(secret: &str, body: &[u8], header: &str) -> bool {
    let Some(given) = header.trim().strip_prefix("sha256=") else { return false };
    let expected = hex(&hmac_sha256(secret.as_bytes(), body));
    let given = given.trim().to_ascii_lowercase();
    if given.len() != expected.len() {
        return false;
    }
    given.bytes().zip(expected.bytes()).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0
}

/// A trigger's path id: 16 hex digits of its key.
pub fn path_id(key: &str) -> String {
    hex(&Sha256::digest(key.as_bytes()))[..16].to_string()
}

/// A new secret: 64 hex digits.
pub fn new_secret() -> String {
    format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple())
}

/// A delivery that passed its checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delivery {
    pub id: String,
    pub delivery: String,
    /// `X-Mighty-Event` or `X-GitHub-Event`, if sent.
    pub event: Option<String>,
    pub body: Vec<u8>,
}

/// What the listener asks of the app.
pub trait Router: Send + Sync {
    /// The secret of the trigger at `id`, if one is listening there.
    fn secret(&self, id: &str) -> Option<String>;
    /// A checked, new delivery.
    fn deliver(&self, delivery: Delivery);
}

/// The ids each trigger has seen, shared by the connections.
#[derive(Default)]
pub struct Deliveries {
    seen: Mutex<HashMap<String, Seen>>,
}

impl Deliveries {
    fn first(&self, id: &str, delivery: &str) -> bool {
        self.seen.lock().unwrap_or_else(|p| p.into_inner()).entry(id.to_string()).or_default().first(delivery)
    }
}

/// The request as read: method, path, headers (lower-case names) and body; or the status to answer.
struct Request {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

async fn read_request(stream: &mut TcpStream) -> Result<Request, u16> {
    let mut buffer = Vec::with_capacity(2048);
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        if let Some(at) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
            break at;
        }
        if buffer.len() > MAX_HEAD {
            return Err(431);
        }
        let read = stream.read(&mut chunk).await.map_err(|_| 400u16)?;
        if read == 0 {
            return Err(400);
        }
        buffer.extend_from_slice(&chunk[..read]);
    };
    let head = std::str::from_utf8(&buffer[..head_end]).map_err(|_| 400u16)?;
    let mut lines = head.split("\r\n");
    let mut first = lines.next().unwrap_or("").split(' ');
    let (method, path) = (first.next().unwrap_or("").to_string(), first.next().unwrap_or("").to_string());
    let mut headers = HashMap::new();
    for line in lines {
        let (name, value) = line.split_once(':').ok_or(400u16)?;
        headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
    }
    if method != "POST" {
        return Err(405);
    }
    if headers.contains_key("transfer-encoding") {
        return Err(411);
    }
    let length: usize = headers.get("content-length").and_then(|l| l.parse().ok()).ok_or(411u16)?;
    if length > MAX_BODY {
        return Err(413);
    }
    let mut body = buffer[head_end + 4..].to_vec();
    if body.len() > length {
        return Err(400);
    }
    while body.len() < length {
        let read = stream.read(&mut chunk).await.map_err(|_| 400u16)?;
        if read == 0 {
            return Err(400);
        }
        body.extend_from_slice(&chunk[..read]);
        if body.len() > length {
            return Err(400);
        }
    }
    Ok(Request { method, path, headers, body })
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        202 => "Accepted",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        405 => "Method Not Allowed",
        411 => "Length Required",
        413 => "Payload Too Large",
        431 => "Request Header Fields Too Large",
        _ => "Error",
    }
}

async fn answer(stream: &mut TcpStream, status: u16, text: &str) {
    let response = format!(
        "HTTP/1.1 {status} {}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
        reason(status),
        text.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
    // A lingering close: what the client still sends (a refused body) is read and dropped for a moment, so the answer is not lost
    // to a reset.
    let mut sink = [0u8; 8192];
    let _ = tokio::time::timeout(Duration::from_secs(1), async {
        let mut drained = 0usize;
        while drained < MAX_BODY {
            match stream.read(&mut sink).await {
                Ok(0) | Err(_) => break,
                Ok(read) => drained += read,
            }
        }
    })
    .await;
}

/// One connection: read, check, deliver.
async fn serve_one(mut stream: TcpStream, router: Arc<dyn Router>, deliveries: Arc<Deliveries>) {
    let request = match tokio::time::timeout(READ_TIMEOUT, read_request(&mut stream)).await {
        Ok(Ok(request)) => request,
        Ok(Err(status)) => return answer(&mut stream, status, reason(status)).await,
        Err(_) => return answer(&mut stream, 400, "timeout").await,
    };
    debug_assert_eq!(request.method, "POST");
    let Some(id) = request.path.strip_prefix("/hooks/").map(|p| p.split('?').next().unwrap_or("").to_string()) else {
        return answer(&mut stream, 404, "not found").await;
    };
    let Some(secret) = router.secret(&id) else { return answer(&mut stream, 404, "not found").await };
    let signature = request.headers.get("x-mighty-signature-256").or_else(|| request.headers.get("x-hub-signature-256"));
    if !signature.is_some_and(|s| verify(&secret, &request.body, s)) {
        return answer(&mut stream, 401, "bad signature").await;
    }
    let delivery = ["x-mighty-delivery", "x-github-delivery"]
        .iter()
        .find_map(|h| request.headers.get(*h))
        .map(|d| d.chars().filter(|c| !c.is_control()).take(120).collect::<String>())
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| hex(&Sha256::digest(&request.body)));
    if !deliveries.first(&id, &delivery) {
        return answer(&mut stream, 200, "duplicate").await;
    }
    let event = ["x-mighty-event", "x-github-event"]
        .iter()
        .find_map(|h| request.headers.get(*h))
        .map(|e| e.chars().filter(|c| !c.is_control()).take(80).collect());
    router.deliver(Delivery { id, delivery, event, body: request.body });
    answer(&mut stream, 202, "accepted").await;
}

/// Binds `127.0.0.1:port` (0: any free port); the listener and the port it got.
pub async fn bind(port: u16) -> std::io::Result<(TcpListener, u16)> {
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).await?;
    let port = listener.local_addr()?.port();
    Ok((listener, port))
}

/// Serves `listener` until the task is dropped, at most [`AT_ONCE`] connections at a time.
pub async fn serve(listener: TcpListener, router: Arc<dyn Router>, deliveries: Arc<Deliveries>) {
    let slots = Arc::new(tokio::sync::Semaphore::new(AT_ONCE));
    loop {
        let Ok((stream, peer)) = listener.accept().await else { continue };
        if !peer.ip().is_loopback() {
            continue;
        }
        let Ok(slot) = slots.clone().try_acquire_owned() else { continue };
        let (router, deliveries) = (router.clone(), deliveries.clone());
        tokio::spawn(async move {
            serve_one(stream, router, deliveries).await;
            drop(slot);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hmac_matches_rfc_4231() {
        // Test case 2.
        assert_eq!(
            hex(&hmac_sha256(b"Jefe", b"what do ya want for nothing?")),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        // Test case 6: a key longer than the block.
        assert_eq!(
            hex(&hmac_sha256(&[0xaa; 131], b"Test Using Larger Than Block-Size Key - Hash Key First")),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
        let body = br#"{"ref":"main"}"#;
        let signature = format!("sha256={}", hex(&hmac_sha256(b"s3cret", body)));
        assert!(verify("s3cret", body, &signature));
        assert!(verify("s3cret", body, &signature.to_ascii_uppercase().replace("SHA256=", "sha256=")));
        assert!(!verify("other", body, &signature));
        assert!(!verify("s3cret", b"{}", &signature));
        assert!(!verify("s3cret", body, &signature[..signature.len() - 1]));
        assert!(!verify("s3cret", body, "md5=abc"));
        assert_eq!(path_id("w1:app:t:deploy").len(), 16);
        assert_ne!(new_secret(), new_secret());
    }

    struct Fixed {
        delivered: Mutex<Vec<Delivery>>,
    }

    impl Router for Fixed {
        fn secret(&self, id: &str) -> Option<String> {
            (id == "abc").then(|| "s3cret".to_string())
        }
        fn deliver(&self, delivery: Delivery) {
            self.delivered.lock().unwrap().push(delivery);
        }
    }

    async fn post(port: u16, path: &str, headers: &[(&str, String)], body: &[u8], length: Option<usize>) -> u16 {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        let mut head = format!("POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: {}\r\n", length.unwrap_or(body.len()));
        for (name, value) in headers {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        head.push_str("\r\n");
        stream.write_all(head.as_bytes()).await.unwrap();
        let _ = stream.write_all(body).await;
        let mut answer = String::new();
        let _ = stream.read_to_string(&mut answer).await;
        answer.split(' ').nth(1).and_then(|s| s.parse().ok()).unwrap_or(0)
    }

    #[tokio::test]
    async fn the_listener_takes_signed_new_deliveries_only() {
        let router = Arc::new(Fixed { delivered: Mutex::new(vec![]) });
        let (listener, port) = bind(0).await.unwrap();
        let task = tokio::spawn(serve(listener, router.clone(), Arc::new(Deliveries::default())));
        let body = br#"{"action":"deployed"}"#;
        let signed = format!("sha256={}", hex(&hmac_sha256(b"s3cret", body)));
        let headers =
            vec![("X-Mighty-Signature-256", signed.clone()), ("X-Mighty-Delivery", "d-1".into()), ("X-Mighty-Event", "deploy".into())];
        assert_eq!(post(port, "/hooks/abc", &headers, body, None).await, 202);
        assert_eq!(post(port, "/hooks/abc", &headers, body, None).await, 200, "the same delivery again");
        // A wrong signature, no signature, an unknown trigger.
        let wrong = vec![("X-Mighty-Signature-256", format!("sha256={}", hex(&hmac_sha256(b"guess", body))))];
        assert_eq!(post(port, "/hooks/abc", &wrong, body, None).await, 401);
        assert_eq!(post(port, "/hooks/abc", &[], body, None).await, 401);
        assert_eq!(post(port, "/hooks/nope", &headers, body, None).await, 404);
        // GitHub's own header names work too; without a delivery id the body's hash dedupes.
        let github = vec![("X-Hub-Signature-256", signed.clone())];
        assert_eq!(post(port, "/hooks/abc", &github, body, None).await, 202);
        assert_eq!(post(port, "/hooks/abc", &github, body, None).await, 200);
        // An oversized body is refused before it is read.
        let big = vec![b'x'; 1024];
        assert_eq!(post(port, "/hooks/abc", &headers, &big, Some(MAX_BODY + 1)).await, 413);
        // Not a POST.
        let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        stream.write_all(b"GET /hooks/abc HTTP/1.1\r\n\r\n").await.unwrap();
        let mut text = String::new();
        let _ = stream.read_to_string(&mut text).await;
        assert!(text.starts_with("HTTP/1.1 405"), "{text}");

        let delivered = router.delivered.lock().unwrap().clone();
        assert_eq!(delivered.len(), 2);
        assert_eq!((delivered[0].delivery.as_str(), delivered[0].event.as_deref()), ("d-1", Some("deploy")));
        assert_eq!(delivered[0].body, body);
        assert_eq!(delivered[1].delivery.len(), 64, "the body's hash");
        task.abort();
    }

    #[tokio::test]
    async fn the_listener_binds_loopback_only() {
        let (listener, port) = bind(0).await.unwrap();
        let address = listener.local_addr().unwrap();
        assert!(address.ip().is_loopback() && port == address.port());
    }
}
