//! The direct listener's HTTP upgrade (C# `MobileDirect.UpgradeResponse`, `AcceptKey`, `AcceptWebSocketAsync`, R\MobileDirect.cs:57-111;
//! plan §1.4): a head of at most 8192 bytes with nothing after its blank line (a client speaks only after the 101); `GET /ws` (its query
//! is not read) over HTTP/1.1, `Upgrade: websocket`, `Connection` carrying `Upgrade`, `Sec-WebSocket-Version: 13` and a 16-byte base64
//! key; anything else gets no reply. The 101 carries `Sec-WebSocket-Accept` and nothing else is negotiated (no subprotocol, no
//! extension); then the stream is wrapped with `WebSocketStream::from_raw_socket(Role::Server)`, frames held to the relay's 1 MiB and no
//! keepalive of the stack's own (the app's 20 s ping and 60 s idle limit keep the session honest, as on the relay). Owner: W8-C.

use base64::Engine;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::protocol::{Role, WebSocketConfig};

use super::wire::MAXIMUM_FRAME_BYTES;

/// `MobileDirect.MaximumRequestHeadBytes`: the phone's upgrade request, headers included.
pub const MAXIMUM_HEAD_BYTES: usize = 8192;

/// `MobileDirect.AcceptKey`: base64(SHA-1(key + RFC 6455's GUID)).
pub fn accept_key(key: &str) -> String {
    tokio_tungstenite::tungstenite::handshake::derive_accept_key(key.as_bytes())
}

/// `IsKey`: a base64 key of 16 bytes.
fn is_key(key: &str) -> bool {
    base64::engine::general_purpose::STANDARD.decode(key).is_ok_and(|bytes| bytes.len() == 16)
}

/// `MobileDirect.UpgradeResponse`: the 101 for a phone's upgrade request head (up to and with its blank line), or none. Header names
/// and the `Upgrade`/`Connection` tokens are compared without case; the first of a repeated header counts.
pub fn upgrade_response(head: &str) -> Option<String> {
    let lines: Vec<&str> = head.split("\r\n").collect();
    let request: Vec<&str> = lines[0].split(' ').collect();
    if request.len() != 3 || request[0] != "GET" || request[2] != "HTTP/1.1" || request[1].split('?').next() != Some("/ws") {
        return None;
    }
    let mut headers: Vec<(String, String)> = vec![];
    for line in lines.iter().skip(1).filter(|line| !line.is_empty()) {
        let colon = line.find(':').filter(|at| *at > 0)?;
        let name = line[..colon].trim().to_ascii_lowercase();
        if !headers.iter().any(|(known, _)| *known == name) {
            headers.push((name, line[colon + 1..].trim().to_string()));
        }
    }
    let header = |name: &str| headers.iter().find(|(known, _)| known == name).map(|(_, value)| value.as_str());
    if !header("upgrade")?.eq_ignore_ascii_case("websocket") {
        return None;
    }
    if !header("connection")?.split(',').any(|token| token.trim().eq_ignore_ascii_case("Upgrade")) {
        return None;
    }
    if header("sec-websocket-version")? != "13" {
        return None;
    }
    let key = header("sec-websocket-key").filter(|key| is_key(key))?;
    Some(format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n",
        accept_key(key)
    ))
}

/// The configuration of a direct server socket: frames and messages held to 1 MiB, as on the relay.
pub fn config() -> WebSocketConfig {
    WebSocketConfig::default().max_message_size(Some(MAXIMUM_FRAME_BYTES)).max_frame_size(Some(MAXIMUM_FRAME_BYTES))
}

/// `MobileDirect.AcceptWebSocketAsync`: reads the phone's upgrade from `stream` (at most 8 KiB, nothing after its blank line), answers
/// it and returns the server WebSocket; none for anything else (or a connection that ended or failed), and then nothing was written:
/// the caller closes the connection unanswered.
pub async fn accept_web_socket<S: AsyncRead + AsyncWrite + Unpin>(mut stream: S) -> Option<WebSocketStream<S>> {
    let mut buffer = vec![0u8; MAXIMUM_HEAD_BYTES];
    let mut length = 0;
    loop {
        if length == buffer.len() {
            return None;
        }
        let read = stream.read(&mut buffer[length..]).await.ok()?;
        if read == 0 {
            return None;
        }
        length += read;
        let Some(end) = buffer[..length].windows(4).position(|w| w == b"\r\n\r\n") else { continue };
        if end + 4 != length {
            return None;
        }
        // Encoding.ASCII: a byte past 127 reads as '?'.
        let head: String = buffer[..length].iter().map(|b| if b.is_ascii() { *b as char } else { '?' }).collect();
        let response = upgrade_response(&head)?;
        stream.write_all(response.as_bytes()).await.ok()?;
        stream.flush().await.ok()?;
        return Some(WebSocketStream::from_raw_socket(stream, Role::Server, Some(config())).await);
    }
}
