//! M-H: the update's HTTPS on the Mac (plan D17; Swift's `URLSession` with `AppUpdateTransportPolicy`), with the shared contract's rules
//! (`mighty_platform::update_transport`): `https://` only (a host name or an IP address, any port: the rehearsal's server is
//! `https://127.0.0.1:<port>`), one GET with no cookies or credentials, a 3xx handed back with its `Location` (the caller follows it, https
//! only, 5 at most), a capped manifest read, a package streamed to its file with progress, its limit and cancel, each within its time; no
//! URL ever in an error or `Debug`.
//!
//! HTTP/1.1 runs here over hyper; the connection — TCP, the system proxy's `CONNECT` tunnel, TLS — is a [`Connect`], M-C's
//! [`crate::http::MacHttps`] in the app (the system proxy through `proxy::MacProxy`, the tunnel, native-tls at TLS 1.2 or later), so the
//! Mac has one HTTPS stack.

use std::io::Write as _;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use http_body_util::{BodyExt as _, Empty};
use hyper::body::Bytes;
use hyper_util::rt::TokioIo;
use mighty_platform::{BoxFuture, DownloadRequest, Downloaded, PlatformError, Result, UpdateResponse, UpdateTransport};
use tokio::io::{AsyncRead, AsyncWrite};

/// A connection hyper speaks HTTP/1.1 over (TLS on TCP, maybe through a proxy's tunnel).
pub trait Stream: AsyncRead + AsyncWrite + Unpin + Send {}

impl<T: AsyncRead + AsyncWrite + Unpin + Send> Stream for T {}

/// Opens a TLS connection to `host:port` (a host name, or an IP address without brackets), checked against the system's trust, through
/// the system's proxy for `https://host:port/` when it names one.
pub trait Connect: Send + Sync {
    fn connect<'a>(&'a self, host: &'a str, port: u16) -> BoxFuture<'a, Result<Box<dyn Stream>>>;
}

/// Where one GET goes.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Target {
    /// A host name (lower case), or an IP address without brackets.
    host: String,
    port: u16,
    /// Starts with `/`.
    path: String,
}

impl Target {
    /// The `Host` header: brackets around an IPv6 address, the port left out when it is 443.
    fn authority(&self) -> String {
        let host = if self.host.contains(':') { format!("[{}]", self.host) } else { self.host.clone() };
        if self.port == 443 { host } else { format!("{host}:{}", self.port) }
    }
}

fn refused() -> PlatformError {
    PlatformError::Invalid("only an https://host[:port]/path address is asked".into())
}

/// An `https://host[:port][/path][?query]` URL; any other scheme, user info, a fragment, a control character or a host of anything but
/// ASCII letters, digits, dots and hyphens (or a bracketed IPv6 address) refused before a connection is made.
fn split(url: &str) -> Result<Target> {
    if url.chars().any(char::is_control) {
        return Err(refused());
    }
    let scheme_end = url.find("://").ok_or_else(refused)?;
    if !url[..scheme_end].eq_ignore_ascii_case("https") {
        return Err(refused());
    }
    let rest = &url[scheme_end + 3..];
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = (&rest[..end], &rest[end..]);
    if authority.contains('@') || tail.contains('#') || authority.is_empty() {
        return Err(refused());
    }
    let path = if tail.is_empty() {
        "/".to_string()
    } else if tail.starts_with('/') {
        tail.to_string()
    } else {
        format!("/{tail}")
    };
    let (host, port) = if let Some(inside) = authority.strip_prefix('[') {
        let (address, after) = inside.split_once(']').ok_or_else(refused)?;
        address.parse::<std::net::Ipv6Addr>().map_err(|_| refused())?;
        (address.to_string(), after.strip_prefix(':'))
    } else {
        match authority.split_once(':') {
            Some((host, port)) => (host.to_ascii_lowercase(), Some(port)),
            None => (authority.to_ascii_lowercase(), None),
        }
    };
    let named = !host.is_empty() && host.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b':');
    if !named || (host.contains(':') && host.parse::<std::net::Ipv6Addr>().is_err()) {
        return Err(refused());
    }
    let port = match port {
        Some(port) => port.parse::<u16>().ok().filter(|port| *port != 0).ok_or_else(refused)?,
        None => 443,
    };
    Ok(Target { host, port, path })
}

type Answer = hyper::Response<hyper::body::Incoming>;

/// The GET sent and its status line and headers received; the connection's driver, which the caller aborts once the body is read.
async fn get(connector: &dyn Connect, target: &Target, accept: Option<&str>) -> Result<(Answer, tokio::task::JoinHandle<()>)> {
    let stream = connector.connect(&target.host, target.port).await?;
    let (mut sender, connection) = hyper::client::conn::http1::Builder::new()
        .title_case_headers(true)
        .handshake(TokioIo::new(stream))
        .await
        .map_err(|_| PlatformError::Refused("the server did not answer".into()))?;
    let driver = tokio::spawn(async move {
        let _ = connection.await;
    });
    let mut request = hyper::Request::builder().method("GET").uri(target.path.as_str()).header(hyper::header::HOST, target.authority());
    if let Some(accept) = accept {
        request = request.header(hyper::header::ACCEPT, accept);
    }
    let request = request.body(Empty::<Bytes>::new()).map_err(|_| PlatformError::Invalid("the request could not be made".into()))?;
    match sender.send_request(request).await {
        Ok(answer) => Ok((answer, driver)),
        Err(_) => {
            driver.abort();
            Err(PlatformError::Refused("the server did not answer".into()))
        }
    }
}

/// A 3xx's `Location` as sent (none when empty).
fn location(answer: &Answer) -> Option<String> {
    let status = answer.status().as_u16();
    if !(300..400).contains(&status) {
        return None;
    }
    let value = answer.headers().get(hyper::header::LOCATION)?;
    Some(String::from_utf8_lossy(value.as_bytes()).into_owned()).filter(|text| !text.is_empty())
}

fn ended_early() -> PlatformError {
    PlatformError::Refused("the answer ended early".into())
}

/// `work` given up on once `timeout` passes.
async fn within<T>(timeout: Duration, work: impl std::future::Future<Output = Result<T>>) -> Result<T> {
    match tokio::time::timeout(timeout.max(Duration::from_millis(1)), work).await {
        Ok(answer) => answer,
        Err(_) => Err(PlatformError::TimedOut("the update's HTTPS request")),
    }
}

async fn get_capped(connector: &dyn Connect, url: &str, cap: usize) -> Result<UpdateResponse> {
    let target = split(url)?;
    let (answer, driver) = get(connector, &target, Some("application/json")).await?;
    let status = answer.status().as_u16();
    let location = location(&answer);
    let mut body = Vec::new();
    if (200..300).contains(&status) {
        let mut incoming = answer.into_body();
        while let Some(frame) = incoming.frame().await {
            let frame = frame.map_err(|_| ended_early())?;
            if let Some(data) = frame.data_ref() {
                if body.len() + data.len() > cap {
                    driver.abort();
                    return Err(PlatformError::TooLarge(cap));
                }
                body.extend_from_slice(data);
            }
        }
    }
    driver.abort();
    Ok(UpdateResponse { status, location, body })
}

async fn download(connector: &dyn Connect, request: &DownloadRequest) -> Result<Downloaded> {
    let target = split(&request.url)?;
    let cancelled = || PlatformError::Refused("cancelled".into());
    if request.cancel.load(Ordering::SeqCst) {
        return Err(cancelled());
    }
    let (answer, driver) = get(connector, &target, None).await?;
    let status = answer.status().as_u16();
    let location = location(&answer);
    if !(200..300).contains(&status) {
        driver.abort();
        return Ok(Downloaded { status, location, written: 0 });
    }
    let written = async {
        let mut file = std::fs::File::create(&request.destination)?;
        let mut written = 0u64;
        let mut incoming = answer.into_body();
        while let Some(frame) = incoming.frame().await {
            if request.cancel.load(Ordering::SeqCst) {
                return Err(cancelled());
            }
            let frame = frame.map_err(|_| ended_early())?;
            if let Some(data) = frame.data_ref() {
                let size = data.len() as u64;
                if written + size > request.limit {
                    return Err(PlatformError::TooLarge(usize::try_from(request.limit).unwrap_or(usize::MAX)));
                }
                file.write_all(data)?;
                written += size;
                (request.progress)(written);
            }
        }
        file.flush()?;
        Ok(written)
    }
    .await;
    driver.abort();
    Ok(Downloaded { status, location: None, written: written? })
}

/// The Mac's `UpdateTransport` over a [`Connect`].
pub struct MacUpdateTransport {
    connector: Arc<dyn Connect>,
}

impl MacUpdateTransport {
    pub fn new(connector: Arc<dyn Connect>) -> MacUpdateTransport {
        MacUpdateTransport { connector }
    }
}

impl std::fmt::Debug for MacUpdateTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MacUpdateTransport")
    }
}

impl UpdateTransport for MacUpdateTransport {
    fn get_capped(&self, url: &str, cap: usize, timeout: Duration) -> BoxFuture<'_, Result<UpdateResponse>> {
        let url = url.to_string();
        Box::pin(async move { within(timeout, get_capped(self.connector.as_ref(), &url, cap)).await })
    }

    fn download(&self, request: DownloadRequest) -> BoxFuture<'_, Result<Downloaded>> {
        Box::pin(async move { within(request.timeout, download(self.connector.as_ref(), &request)).await })
    }
}

/// The Mac's `UpdateTransport`, over M-C's HTTPS (the system proxy and TLS).
pub fn update_transport() -> Arc<dyn UpdateTransport> {
    Arc::new(MacUpdateTransport::new(Arc::new(crate::http::MacHttps::new())))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::sync::atomic::AtomicBool;

    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio::net::TcpListener;

    use super::*;

    #[test]
    fn only_an_https_address_is_asked() {
        let target = split("https://Updates.Example/mightycode/latest.json?x=1").unwrap();
        assert_eq!(target, Target { host: "updates.example".into(), port: 443, path: "/mightycode/latest.json?x=1".into() });
        assert_eq!(target.authority(), "updates.example");
        let local = split("https://127.0.0.1:8443/latest.json").unwrap();
        assert_eq!((local.host.as_str(), local.port, local.authority()), ("127.0.0.1", 8443, "127.0.0.1:8443".to_string()));
        assert_eq!(split("https://[::1]:9/x").unwrap().authority(), "[::1]:9");
        assert_eq!(split("HTTPS://a.example").unwrap().path, "/");
        assert_eq!(split("https://a.example?q").unwrap().path, "/?q");
        for bad in [
            "http://a.example/x",
            "ftp://a.example/x",
            "https://user@a.example/x",
            "https://a.example/x#frag",
            "https:///x",
            "https://a.example:0/x",
            "https://a.example:99999/x",
            "https://a b/x",
            "https://a.example\\evil/x",
            "https://\u{0430}.example/x",
            "https://a.example/x\r\nHost: evil",
        ] {
            assert!(split(bad).is_err(), "{bad}");
        }
    }

    /// Plain TCP to the test's loopback server: the HTTP over it is what is checked.
    struct PlainTcp;

    impl Connect for PlainTcp {
        fn connect<'a>(&'a self, host: &'a str, port: u16) -> BoxFuture<'a, Result<Box<dyn Stream>>> {
            Box::pin(async move { Ok(Box::new(tokio::net::TcpStream::connect((host, port)).await?) as Box<dyn Stream>) })
        }
    }

    /// A loopback server answering each connection with `reply`; the request heads it read come back.
    async fn server(reply: Vec<u8>) -> (u16, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = Arc::new(Mutex::new(vec![]));
        let record = seen.clone();
        tokio::spawn(async move {
            while let Ok((mut client, _)) = listener.accept().await {
                let (record, reply) = (record.clone(), reply.clone());
                tokio::spawn(async move {
                    let mut head = Vec::new();
                    let mut byte = [0u8; 1];
                    while !head.ends_with(b"\r\n\r\n") && client.read(&mut byte).await.unwrap_or(0) == 1 {
                        head.push(byte[0]);
                    }
                    record.lock().unwrap().push(String::from_utf8_lossy(&head).into_owned());
                    let _ = client.write_all(&reply).await;
                    let _ = client.shutdown().await;
                });
            }
        });
        (port, seen)
    }

    fn transport() -> MacUpdateTransport {
        MacUpdateTransport::new(Arc::new(PlainTcp))
    }

    fn ok(body: &[u8]) -> Vec<u8> {
        let mut reply = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).into_bytes();
        reply.extend_from_slice(body);
        reply
    }

    #[tokio::test]
    async fn a_manifest_is_read_up_to_its_cap_and_a_redirect_comes_back() {
        let (port, seen) = server(ok(b"{\"version\":\"0.5.1\"}")).await;
        let url = format!("https://127.0.0.1:{port}/latest.json");
        let answer = transport().get_capped(&url, 1024, Duration::from_secs(5)).await.unwrap();
        assert_eq!((answer.status, answer.body.as_slice(), answer.location), (200, &b"{\"version\":\"0.5.1\"}"[..], None));
        let head = seen.lock().unwrap()[0].clone();
        assert!(head.starts_with("GET /latest.json HTTP/1.1\r\n"), "{head}");
        assert!(head.contains(&format!("Host: 127.0.0.1:{port}\r\n")) && head.contains("Accept: application/json\r\n"), "{head}");
        assert!(!head.to_ascii_lowercase().contains("cookie") && !head.to_ascii_lowercase().contains("authorization"), "{head}");
        assert!(matches!(transport().get_capped(&url, 4, Duration::from_secs(5)).await, Err(PlatformError::TooLarge(4))));

        let (port, _) =
            server(b"HTTP/1.1 302 Found\r\nLocation: https://cdn.example/next.json\r\nContent-Length: 0\r\n\r\n".to_vec()).await;
        let answer = transport().get_capped(&format!("https://127.0.0.1:{port}/latest.json"), 1024, Duration::from_secs(5)).await.unwrap();
        assert_eq!((answer.status, answer.location.as_deref()), (302, Some("https://cdn.example/next.json")));
        assert!(format!("{answer:?}").contains("location: true") && !format!("{answer:?}").contains("cdn.example"));
        let (port, _) = server(b"HTTP/1.1 404 Not Found\r\nContent-Length: 4\r\n\r\nnope".to_vec()).await;
        let answer = transport().get_capped(&format!("https://127.0.0.1:{port}/x"), 1024, Duration::from_secs(5)).await.unwrap();
        assert_eq!((answer.status, answer.body.len()), (404, 0));
        assert!(matches!(
            transport().get_capped("http://127.0.0.1:1/x", 1024, Duration::from_secs(5)).await,
            Err(PlatformError::Invalid(_))
        ));
    }

    #[tokio::test]
    async fn a_server_that_never_answers_runs_out_of_time() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let mut held = vec![];
            while let Ok((client, _)) = listener.accept().await {
                held.push(client);
            }
        });
        let url = format!("https://127.0.0.1:{port}/latest.json");
        assert!(matches!(transport().get_capped(&url, 1024, Duration::from_millis(300)).await, Err(PlatformError::TimedOut(_))));
    }

    fn temp_file(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        std::env::temp_dir().join(format!("mighty-transport-{name}-{}-{nanos}", std::process::id()))
    }

    fn request(url: String, destination: PathBuf, limit: u64, seen: Arc<Mutex<Vec<u64>>>, cancel: Arc<AtomicBool>) -> DownloadRequest {
        DownloadRequest {
            url,
            destination,
            limit,
            timeout: Duration::from_secs(10),
            progress: Arc::new(move |written| seen.lock().unwrap().push(written)),
            cancel,
        }
    }

    #[tokio::test]
    async fn a_package_is_written_as_it_arrives_within_its_limit() {
        let body: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        let (port, _) = server(ok(&body)).await;
        let url = format!("https://127.0.0.1:{port}/MightyCode-macos.zip");
        let file = temp_file("ok");
        let progress = Arc::new(Mutex::new(vec![]));
        let answer = transport()
            .download(request(url.clone(), file.clone(), body.len() as u64, progress.clone(), Arc::new(AtomicBool::new(false))))
            .await
            .unwrap();
        assert_eq!((answer.status, answer.written), (200, body.len() as u64));
        assert_eq!(std::fs::read(&file).unwrap(), body);
        assert_eq!(progress.lock().unwrap().last().copied(), Some(body.len() as u64));
        let _ = std::fs::remove_file(&file);

        let small = temp_file("limit");
        let refused =
            transport().download(request(url.clone(), small.clone(), 1000, Arc::default(), Arc::new(AtomicBool::new(false)))).await;
        assert!(matches!(refused, Err(PlatformError::TooLarge(1000))), "{refused:?}");
        let _ = std::fs::remove_file(&small);

        let cancelled = temp_file("cancel");
        let refused = transport().download(request(url, cancelled.clone(), 1 << 30, Arc::default(), Arc::new(AtomicBool::new(true)))).await;
        assert!(matches!(&refused, Err(PlatformError::Refused(reason)) if reason == "cancelled"), "{refused:?}");
        let _ = std::fs::remove_file(&cancelled);

        let (port, _) = server(b"HTTP/1.1 307 Temporary Redirect\r\nLocation: /elsewhere.zip\r\nContent-Length: 0\r\n\r\n".to_vec()).await;
        let none = temp_file("redirect");
        let answer = transport()
            .download(request(format!("https://127.0.0.1:{port}/a.zip"), none.clone(), 1 << 20, Arc::default(), Arc::default()))
            .await
            .unwrap();
        assert_eq!((answer.status, answer.location.as_deref(), answer.written), (307, Some("/elsewhere.zip"), 0));
        assert!(!none.exists(), "a non-2xx writes nothing");
    }
}
