//! The client's strict HTTP/1.1 transport (C# `RemoteTarget`, `RemoteTransport`, R\RemoteTransport.cs; M/Remote/RemoteTransport.swift;
//! plan §1.6, D7): one request per connection over tokio TCP, straight to the resolved Tailscale IP: a fixed header set (the key only in
//! `Authorization: Bearer`, never in a URL, a log or `Debug`), no proxy (`UseProxy = false`), no cookie, no redirect followed, at most
//! 2 MiB read; the answer must carry exactly one `x-mighty-remote-version: 2`. The address is resolved once (a literal, or MagicDNS
//! within 5 s; every result Tailscale's, IPv4 preferred). Owner: W8-D.

use std::net::IpAddr;
use std::time::Duration;

use mighty_run::strings::{get, get_with};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::remote::address::{RemoteAddress, canonical, peer_allowed, unmapped};
use crate::remote::strings;
use crate::remote::text;
use crate::remote::validation;
use crate::remote::wire::{RemoteHttpFailure, protocol};

/// C# `RemoteTarget`: where a connection's requests go, the address checked once and the one IP every request uses, with the loopback
/// flag it was resolved with (so every request re-checks its IP by the same rule).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteTarget {
    pub address: RemoteAddress,
    pub ip: String,
    pub port: u16,
    /// Injected in code by the checks and the smoke only; never from a setting, a connection field or IPC.
    pub allow_loopback: bool,
}

impl RemoteTarget {
    /// A target built by hand (the checks); a request still checks its IP.
    pub fn new(address: RemoteAddress, ip: &str, port: u16, allow_loopback: bool) -> RemoteTarget {
        RemoteTarget { address, ip: ip.into(), port, allow_loopback }
    }
}

/// What a request or a resolution failed with, as C#'s exception types tell them apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportError {
    /// `RemoteHttpFailure`: the host answered with an error status.
    Status(RemoteHttpFailure),
    /// `InvalidDataException`: an answer that is not Mighty's (no or a repeated version header, an unexpected status, too large).
    InvalidData(String),
    /// `TimeoutException` (`remote.windows.connect.error.timeout`).
    Timeout(String),
    /// `HttpRequestException` / `IOException`: the network stumbled.
    Network(String),
    /// `InvalidOperationException`: refused here before anything was sent (a bad request, not Tailscale, unresolved).
    Refused(String),
    /// `ArgumentException`: a header the transport does not allow a caller to set.
    Argument(String),
}

impl TransportError {
    /// What the user reads.
    pub fn message(&self) -> &str {
        match self {
            TransportError::Status(failure) => &failure.message,
            TransportError::InvalidData(m)
            | TransportError::Timeout(m)
            | TransportError::Network(m)
            | TransportError::Refused(m)
            | TransportError::Argument(m) => m,
        }
    }

    /// `RemoteTransport.Transient(error)`: worth asking again for a while (the host busy, or the network stumbled).
    pub fn transient(&self) -> bool {
        match self {
            TransportError::Status(failure) => failure.transient(),
            TransportError::Timeout(_) | TransportError::Network(_) => true,
            _ => false,
        }
    }

    /// The host's status, for an error status.
    pub fn status(&self) -> Option<u16> {
        match self {
            TransportError::Status(failure) => Some(failure.status),
            _ => None,
        }
    }
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for TransportError {}

/// `RemoteTransport.ResolveAsync(address, allowLoopback)`: a MagicDNS name resolved (or a literal taken), accepted only when every
/// address it names is a Tailscale one; IPv4 first.
pub async fn resolve(address: &RemoteAddress, allow_loopback: bool) -> Result<RemoteTarget, TransportError> {
    let found: Vec<IpAddr> = match address.host.parse::<IpAddr>() {
        Ok(literal) => vec![literal],
        Err(_) => {
            // Any failure of the lookup (no such name, a name the resolver refuses, the 5 s limit) reads as the Mac's getaddrinfo failure.
            let lookup = tokio::time::timeout(Duration::from_secs(5), tokio::net::lookup_host((address.host.as_str(), address.port))).await;
            match lookup {
                Ok(Ok(addresses)) => addresses.map(|a| a.ip()).collect(),
                _ => return Err(TransportError::Refused(get("remote.connect.error.unresolved"))),
            }
        }
    };
    let mut ips: Vec<String> = vec![];
    for ip in found.into_iter().take(32) {
        let spelled = canonical(&unmapped(ip).to_string());
        if !ips.contains(&spelled) {
            ips.push(spelled);
        }
    }
    if ips.is_empty() || !ips.iter().all(|ip| peer_allowed(ip, allow_loopback)) {
        return Err(TransportError::Refused(get("remote.connect.error.notTailscale")));
    }
    ips.sort_by(|a, b| a.contains(':').cmp(&b.contains(':')).then_with(|| a.cmp(b)));
    Ok(RemoteTarget { address: address.clone(), ip: ips[0].clone(), port: address.port, allow_loopback })
}

/// `AllowedExtraHeaders`: the extra request headers a caller may set (the Windows `graph_run` opt-in only).
pub const ALLOWED_EXTRA_HEADERS: [&str; 1] = [protocol::GRAPH_HEADER];

/// GET or POST.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
}

/// One request's options beyond its target, key, method and path.
#[derive(Debug, Clone, Default)]
pub struct Request<'a> {
    pub body: Option<&'a [u8]>,
    /// None is [`protocol::REQUEST_TIMEOUT`].
    pub timeout: Option<Duration>,
    /// None is [`protocol::MAXIMUM_RESPONSE_BYTES`].
    pub maximum_bytes: Option<usize>,
    pub headers: &'a [(&'a str, &'a str)],
}

fn bad_request() -> TransportError {
    TransportError::Refused(get("remote.connect.error.badRequest"))
}

/// Whether the path goes out exactly as written (C# compares `Uri.PathAndQuery`, which resolves dot segments).
fn plain_path(path: &str) -> bool {
    path.starts_with(protocol::PATH_PREFIX)
        && path.bytes().all(|b| b.is_ascii_alphanumeric() || b"/._:?=-".contains(&b))
        && !path.split('?').next().unwrap_or_default().split('/').any(|segment| segment == "." || segment == "..")
}

/// `RemoteTransport.RequestAsync(target, key, method, path, body, timeout, maximumBytes, headers)`: one authenticated request; the
/// answer's body (up to the maximum). Dropping the future cancels it.
pub async fn request(
    target: &RemoteTarget,
    key: &str,
    method: Method,
    path: &str,
    options: Request<'_>,
) -> Result<Vec<u8>, TransportError> {
    if !validation::key(Some(key)) || !plain_path(path) {
        return Err(bad_request());
    }
    for (name, _) in options.headers {
        if !ALLOWED_EXTRA_HEADERS.iter().any(|allowed| allowed.eq_ignore_ascii_case(name)) {
            return Err(TransportError::Argument(format!("remote request header not allowed: {name}")));
        }
    }
    // The key goes only to a Tailscale peer, even for a target built by hand.
    if !peer_allowed(&target.ip, target.allow_loopback) {
        return Err(TransportError::Refused(get("remote.connect.error.notTailscale")));
    }
    let ip: IpAddr = target.ip.trim_matches(['[', ']']).parse().map_err(|_| bad_request())?;
    let limit = options.timeout.unwrap_or(protocol::REQUEST_TIMEOUT);
    let maximum = options.maximum_bytes.unwrap_or(protocol::MAXIMUM_RESPONSE_BYTES);
    let host = if target.ip.contains(':') { format!("[{}]", target.ip) } else { target.ip.clone() };
    let mut head = format!(
        "{} {path} HTTP/1.1\r\nHost: {host}:{}\r\nAuthorization: Bearer {key}\r\n{}: {}\r\nAccept: application/json\r\nConnection: close\r\n",
        if method == Method::Get { "GET" } else { "POST" },
        target.port,
        protocol::HEADER,
        protocol::VERSION
    );
    for (name, value) in options.headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    if let Some(body) = options.body {
        head.push_str(&format!("Content-Type: application/json\r\nContent-Length: {}\r\n", body.len()));
    }
    head.push_str("\r\n");
    let exchange = async {
        let mut stream = TcpStream::connect((ip, target.port)).await.map_err(|e| TransportError::Network(e.kind().to_string()))?;
        stream.write_all(head.as_bytes()).await.map_err(|e| TransportError::Network(e.kind().to_string()))?;
        if let Some(body) = options.body {
            stream.write_all(body).await.map_err(|e| TransportError::Network(e.kind().to_string()))?;
        }
        read_answer(&mut stream, maximum).await
    };
    let (status, data) = match tokio::time::timeout(limit, exchange).await {
        Ok(answer) => answer?,
        Err(_) => return Err(TransportError::Timeout(strings::timeout())),
    };
    if status >= 400 {
        // A route's {protocol, error}, read as the Mac reads it: the error string only; anything unreadable is the status text.
        let message = serde_json::from_slice::<serde_json::Value>(&data)
            .ok()
            .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_string))
            .unwrap_or_else(|| get_with("remote.connect.error.unexpectedStatus", &[("status", &status.to_string())]));
        return Err(TransportError::Status(RemoteHttpFailure { status, message: text::cut(&message, text::DEFAULT_CUT) }));
    }
    Ok(data)
}

fn network(error: std::io::Error) -> TransportError {
    TransportError::Network(error.kind().to_string())
}

/// The answer's status and body: the head (at most 64 KiB, as .NET reads it), the version header exactly once, a status in 2xx or
/// 4xx-5xx, and a body framed by its length, chunks, or the end of the connection, never over `maximum`.
async fn read_answer(stream: &mut TcpStream, maximum: usize) -> Result<(u16, Vec<u8>), TransportError> {
    let not_mighty = || TransportError::InvalidData(get("remote.connect.error.notMighty"));
    let too_large = || TransportError::InvalidData(get("remote.connect.error.responseTooLarge"));
    let mut buffer = Vec::with_capacity(16 * 1024);
    let mut chunk = vec![0u8; 16 * 1024];
    let end = loop {
        if let Some(at) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
            break at;
        }
        if buffer.len() > 64 * 1024 {
            return Err(TransportError::Network("the answer's head is too large".into()));
        }
        let read = stream.read(&mut chunk).await.map_err(network)?;
        if read == 0 {
            return Err(TransportError::Network("the connection closed before the answer".into()));
        }
        buffer.extend_from_slice(&chunk[..read]);
    };
    let head = String::from_utf8_lossy(&buffer[..end]).into_owned();
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let mut parts = status_line.splitn(3, ' ');
    let (version, code) = (parts.next().unwrap_or_default(), parts.next().unwrap_or_default());
    if !version.starts_with("HTTP/1.") {
        return Err(TransportError::Network("not an HTTP answer".into()));
    }
    let status: u16 = code.parse().map_err(|_| TransportError::Network("not an HTTP status".into()))?;
    let mut versions = vec![];
    let mut length: Option<usize> = None;
    let mut chunked = false;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else { continue };
        let (name, value) = (name.trim(), value.trim());
        if name.eq_ignore_ascii_case(protocol::HEADER) {
            versions.extend(value.split(',').map(|v| v.trim().to_string()));
        } else if name.eq_ignore_ascii_case("content-length") {
            length = Some(value.parse().map_err(|_| TransportError::Network("a bad length".into()))?);
        } else if name.eq_ignore_ascii_case("transfer-encoding") {
            chunked = value.to_ascii_lowercase().contains("chunked");
        }
    }
    // Exactly one header, exactly "2": a repeated one is no Mighty answer either.
    if versions.len() != 1 || versions[0] != protocol::VERSION.to_string() {
        return Err(not_mighty());
    }
    if !(200..=299).contains(&status) && !(400..=599).contains(&status) {
        return Err(TransportError::InvalidData(get_with("remote.connect.error.unexpectedStatus", &[("status", &status.to_string())])));
    }
    if length.is_some_and(|l| l > maximum) {
        return Err(too_large());
    }
    let mut body = buffer[end + 4..].to_vec();
    if chunked {
        return read_chunked(stream, body, maximum).await.map(|b| (status, b));
    }
    loop {
        if body.len() > maximum {
            return Err(too_large());
        }
        if length.is_some_and(|l| body.len() >= l) {
            body.truncate(length.unwrap_or_default());
            return Ok((status, body));
        }
        let read = stream.read(&mut chunk).await.map_err(network)?;
        if read == 0 {
            if length.is_some() {
                return Err(TransportError::Network("the answer ended early".into()));
            }
            return Ok((status, body));
        }
        if body.len() + read > maximum {
            return Err(too_large());
        }
        body.extend_from_slice(&chunk[..read]);
    }
}

/// The longest chunk-size line taken (its size, extensions and CRLF): a line past it is no answer of a Mighty host.
const MAXIMUM_CHUNK_LINE: usize = 1024;

fn bad_chunk() -> TransportError {
    TransportError::Network("a bad chunk".into())
}

/// More of the answer into `raw`: the connection's end before the answer's is a failure, and `raw` never holds more than the largest
/// body, one chunk line and what one read brings.
async fn more(stream: &mut TcpStream, raw: &mut Vec<u8>, chunk: &mut [u8], maximum: usize) -> Result<(), TransportError> {
    let read = stream.read(chunk).await.map_err(network)?;
    if read == 0 {
        return Err(TransportError::Network("the answer ended early".into()));
    }
    raw.extend_from_slice(&chunk[..read]);
    if raw.len() > maximum.saturating_add(MAXIMUM_CHUNK_LINE + 4).saturating_add(chunk.len()) {
        return Err(TransportError::InvalidData(get("remote.connect.error.responseTooLarge")));
    }
    Ok(())
}

/// A chunk-size line's size: hex digits only (at most 16, an extension after `;` let go), else a bad chunk.
fn chunk_size(line: &[u8]) -> Result<usize, TransportError> {
    let text = std::str::from_utf8(line).map_err(|_| bad_chunk())?;
    let digits = text.split(';').next().unwrap_or_default().trim_matches([' ', '\t']);
    if digits.is_empty() || digits.len() > 16 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(bad_chunk());
    }
    usize::from_str_radix(digits, 16).map_err(|_| bad_chunk())
}

/// A chunked body: each size line bounded, each chunk's data and its CRLF checked, every sum checked, the body at most `maximum`.
async fn read_chunked(stream: &mut TcpStream, mut raw: Vec<u8>, maximum: usize) -> Result<Vec<u8>, TransportError> {
    let mut body = vec![];
    let mut chunk = vec![0u8; 16 * 1024];
    loop {
        // The size line, looked for only in what came since the last look (a long line is never scanned again and again).
        let mut scanned: usize = 0;
        let line_end = loop {
            let from = scanned.saturating_sub(1);
            if let Some(at) = raw[from..].windows(2).position(|w| w == b"\r\n") {
                break from + at;
            }
            if raw.len() > MAXIMUM_CHUNK_LINE {
                return Err(bad_chunk());
            }
            scanned = raw.len();
            more(stream, &mut raw, &mut chunk, maximum).await?;
        };
        if line_end + 2 > MAXIMUM_CHUNK_LINE {
            return Err(bad_chunk());
        }
        let size = chunk_size(&raw[..line_end])?;
        if size == 0 {
            return Ok(body);
        }
        if body.len().checked_add(size).is_none_or(|total| total > maximum) {
            return Err(TransportError::InvalidData(get("remote.connect.error.responseTooLarge")));
        }
        // size <= maximum here, so the sum cannot overflow; checked anyway.
        let data_end = (line_end + 2).checked_add(size).ok_or_else(bad_chunk)?;
        let next = data_end.checked_add(2).ok_or_else(bad_chunk)?;
        while raw.len() < next {
            more(stream, &mut raw, &mut chunk, maximum).await?;
        }
        if &raw[data_end..next] != b"\r\n" {
            return Err(bad_chunk());
        }
        body.extend_from_slice(&raw[line_end + 2..data_end]);
        raw.drain(..next);
    }
}
