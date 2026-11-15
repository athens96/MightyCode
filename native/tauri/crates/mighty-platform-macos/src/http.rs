//! M-C: HTTPS GET and POST (the account usage's GET, the phone push's POST) over hyper and native-tls (Security.framework's trust),
//! through the system proxy, as mighty-remote's relay already reaches it (plan D17; Swift's URLSession: `ephemeral`, redirects
//! refused by its delegate, no cookies or stored credentials). Contract `mighty_platform::http` (`HttpsSend`), the same rules as
//! Windows' WinHTTP one (mighty-platform-windows http.rs):
//!
//! - a GET goes to an `https://host/path` URL on port 443 only (a host name of ASCII letters, digits, dots and hyphens); a POST to the
//!   push route as the user set it, `https://` or `http://` (a relay on a LAN or loopback), on its own port;
//! - one request, no retry, no cookies, no stored credentials, no redirect followed (a 3xx comes back with its `Location`, `refused`
//!   when it named none), the whole request within its timeout;
//! - the system's proxy for the URL ([`crate::proxy`]): a `CONNECT` tunnel before TLS for `https://`, the absolute form for `http://`;
//! - the body read only for a 2xx GET, refused past `max_body`; a POST's answer is its status and headers (the push reads no body);
//! - no header value, body or URL path ever reaches an error.

use std::sync::Arc;
use std::time::Duration;

use http_body_util::{BodyExt as _, Full};
use hyper::body::Bytes;
use hyper::header::{HeaderName, HeaderValue};
use hyper_util::rt::TokioIo;
use mighty_platform::http::{HttpsRequest, HttpsResponse};
use mighty_platform::{BoxFuture, HttpsSend, PlatformError, Proxy, ProxyResolver, Result};
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};
use tokio::net::TcpStream;

/// A connection hyper speaks over: plain TCP, or TLS on it.
trait Io: AsyncRead + AsyncWrite + Unpin + Send {}

impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

/// How one request goes out.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Target {
    post: bool,
    secure: bool,
    /// A host name, or an IP address without brackets.
    host: String,
    port: u16,
    /// Starts with `/`.
    path: String,
}

impl Target {
    fn default_port(&self) -> u16 {
        if self.secure { 443 } else { 80 }
    }

    /// `host[:port]`, an IPv6 address in brackets, the port left out when it is the scheme's own (the `Host` header).
    fn authority(&self) -> String {
        let host = if self.host.contains(':') { format!("[{}]", self.host) } else { self.host.clone() };
        if self.port == self.default_port() { host } else { format!("{host}:{}", self.port) }
    }

    /// What the proxy lookup is asked about (never the path or query).
    fn origin(&self) -> String {
        format!("{}://{}/", if self.secure { "https" } else { "http" }, self.authority())
    }
}

fn refused_url() -> PlatformError {
    PlatformError::Invalid("only an https://host/path address is asked".into())
}

/// An `https://host[/path?query]` URL as a GET's target; anything else (another scheme or port, user info, a fragment, no host, a host
/// name of anything but ASCII letters, digits, dots and hyphens) refused.
fn split(url: &str) -> Result<Target> {
    let rest = url.strip_prefix("https://").ok_or_else(refused_url)?;
    let (host, path) = match rest.find(['/', '?']) {
        Some(at) if rest[at..].starts_with('/') => (&rest[..at], rest[at..].to_string()),
        Some(at) => (&rest[..at], format!("/{}", &rest[at..])),
        None => (rest, "/".to_string()),
    };
    let host = host.to_ascii_lowercase();
    let named = !host.is_empty() && host.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-');
    if !named || path.contains('#') {
        return Err(refused_url());
    }
    Ok(Target { post: false, secure: true, host, port: 443, path })
}

/// A push route's URL (`https://` or `http://`, a host name or an IP address, an optional port, a path) as a POST's target; user info,
/// a fragment or any other scheme refused.
fn split_post(url: &str) -> Result<Target> {
    let refused = || PlatformError::Invalid("only an http(s)://host[:port]/path address is posted to".into());
    let (secure, rest) = match (url.strip_prefix("https://"), url.strip_prefix("http://")) {
        (Some(rest), _) => (true, rest),
        (None, Some(rest)) => (false, rest),
        _ => return Err(refused()),
    };
    let end = rest.find(['/', '?']).unwrap_or(rest.len());
    let (authority, path) = (&rest[..end], &rest[end..]);
    let path = if path.is_empty() {
        "/".to_string()
    } else if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    if authority.contains('@') || path.contains('#') {
        return Err(refused());
    }
    let (host, port) = if let Some(inside) = authority.strip_prefix('[') {
        let (address, after) = inside.split_once(']').ok_or_else(refused)?;
        address.parse::<std::net::Ipv6Addr>().map_err(|_| refused())?;
        (address.to_string(), after.strip_prefix(':'))
    } else {
        match authority.rsplit_once(':') {
            Some((host, port)) => (host.to_ascii_lowercase(), Some(port)),
            None => (authority.to_ascii_lowercase(), None),
        }
    };
    let named = !host.is_empty() && host.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b':'));
    if !named || (host.contains(':') && host.parse::<std::net::Ipv6Addr>().is_err()) {
        return Err(refused());
    }
    let port = match port {
        Some(port) => port.parse::<u16>().ok().filter(|p| *p != 0).ok_or_else(refused)?,
        None if secure => 443,
        None => 80,
    };
    Ok(Target { post: true, secure, host, port, path })
}

/// Headers that could carry a second request (a line break) or a bad name are refused before anything is sent.
fn headers(request: &HttpsRequest) -> Result<Vec<(HeaderName, HeaderValue)>> {
    let bad = || PlatformError::Invalid("a header holds a line break".into());
    let target_bad = |text: &str| text.chars().any(|c| c.is_control());
    if target_bad(&request.url) {
        return Err(refused_url());
    }
    request
        .headers
        .iter()
        .map(|(name, value)| {
            if name.is_empty() || target_bad(name) || name.contains(':') || target_bad(value) {
                return Err(bad());
            }
            Ok((HeaderName::from_bytes(name.as_bytes()).map_err(|_| bad())?, HeaderValue::from_str(value).map_err(|_| bad())?))
        })
        .collect()
}

/// `CONNECT host:port HTTP/1.1` through an HTTP proxy, answered 2xx before anything else is sent (mighty-remote's relay tunnel).
async fn tunnel(stream: &mut TcpStream, target: &Target) -> Result<()> {
    let authority =
        if target.host.contains(':') { format!("[{}]:{}", target.host, target.port) } else { format!("{}:{}", target.host, target.port) };
    let request = format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n\r\n");
    stream.write_all(request.as_bytes()).await?;
    let mut head = Vec::with_capacity(256);
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if head.len() > 16 * 1024 || stream.read(&mut byte).await? == 0 {
            return Err(PlatformError::Refused("the proxy did not open a tunnel".into()));
        }
        head.push(byte[0]);
    }
    let line = String::from_utf8_lossy(&head);
    let status = line.split_whitespace().nth(1).and_then(|s| s.parse::<u16>().ok()).unwrap_or(0);
    if (200..300).contains(&status) { Ok(()) } else { Err(PlatformError::Refused(format!("the proxy refused the tunnel ({status})"))) }
}

/// The connection to `target` (straight, or through `route`), TLS on it for `https://`; and whether the request goes in the absolute
/// form (plain HTTP through a proxy).
async fn connect(target: &Target, route: &Proxy) -> Result<(Box<dyn Io>, bool)> {
    let stream = match route {
        Proxy::Direct => TcpStream::connect((target.host.as_str(), target.port)).await?,
        Proxy::Http { host, port } => {
            let mut stream = TcpStream::connect((host.as_str(), *port)).await?;
            if target.secure {
                tunnel(&mut stream, target).await?;
            }
            stream
        }
    };
    let absolute = matches!(route, Proxy::Http { .. }) && !target.secure;
    let _ = stream.set_nodelay(true);
    if !target.secure {
        return Ok((Box::new(stream), absolute));
    }
    let connector = native_tls::TlsConnector::new().map_err(|_| PlatformError::Refused("TLS is not available".into()))?;
    let stream = tokio_native_tls::TlsConnector::from(connector)
        .connect(&target.host, stream)
        .await
        .map_err(|_| PlatformError::Refused("the TLS handshake failed".into()))?;
    Ok((Box::new(stream), false))
}

async fn send(proxy: &dyn ProxyResolver, target: Target, request: HttpsRequest, body: Vec<u8>, max_body: usize) -> Result<HttpsResponse> {
    let headers = headers(&request)?;
    // A system proxy that cannot be read is no proxy (URLSession goes straight then).
    let route = proxy.resolve(&target.origin()).await.unwrap_or(Proxy::Direct);
    let (stream, absolute) = connect(&target, &route).await?;
    // Header names as URLSession and WinHTTP write them (`Authorization`, `Content-Type`), not hyper's lower case.
    let (mut sender, connection) = hyper::client::conn::http1::Builder::new()
        .title_case_headers(true)
        .handshake(TokioIo::new(stream))
        .await
        .map_err(|_| PlatformError::Refused("the server did not answer".into()))?;
    let driver = tokio::spawn(async move {
        let _ = connection.await;
    });
    let uri = if absolute { format!("http://{}{}", target.authority(), target.path) } else { target.path.clone() };
    let mut builder = hyper::Request::builder().method(if target.post { "POST" } else { "GET" }).uri(uri);
    builder = builder.header(hyper::header::HOST, target.authority());
    for (name, value) in headers {
        builder = builder.header(name, value);
    }
    if target.post {
        builder = builder.header(hyper::header::CONTENT_LENGTH, body.len());
    }
    let outgoing =
        builder.body(Full::new(Bytes::from(body))).map_err(|_| PlatformError::Invalid("the request could not be made".into()))?;
    let answer = sender.send_request(outgoing).await.map_err(|_| PlatformError::Refused("the server did not answer".into()))?;
    let status = answer.status().as_u16();
    let text = |name: &str| answer.headers().get(name).map(|value| String::from_utf8_lossy(value.as_bytes()).into_owned());
    let retry_after = text("retry-after");
    let redirect_location =
        (300..400).contains(&status).then(|| text("location").filter(|l| !l.is_empty()).unwrap_or_else(|| "refused".into()));
    let mut read = Vec::new();
    if (200..300).contains(&status) && !target.post {
        let mut incoming = answer.into_body();
        while let Some(frame) = incoming.frame().await {
            let frame = frame.map_err(|_| PlatformError::Refused("the answer ended early".into()))?;
            if let Some(data) = frame.data_ref() {
                if read.len() + data.len() > max_body {
                    driver.abort();
                    return Err(PlatformError::TooLarge(max_body));
                }
                read.extend_from_slice(data);
            }
        }
    }
    driver.abort();
    Ok(HttpsResponse { status, body: String::from_utf8_lossy(&read).into_owned(), retry_after, redirect_location })
}

/// The whole request given up on once `timeout` passes (C#'s `HttpClient.Timeout`, Swift's `timeoutIntervalForResource`).
async fn within<T>(timeout: Duration, work: impl std::future::Future<Output = Result<T>>) -> Result<T> {
    match tokio::time::timeout(timeout.max(Duration::from_millis(1)), work).await {
        Ok(answer) => answer,
        Err(_) => Err(PlatformError::TimedOut("the HTTPS request")),
    }
}

/// mighty-platform's `HttpsSend` on the Mac.
pub struct MacHttps {
    proxy: Arc<dyn ProxyResolver>,
}

impl MacHttps {
    /// Through the system's proxy.
    pub fn new() -> MacHttps {
        MacHttps::with_proxy(crate::proxy::proxy())
    }

    /// Through what `proxy` names (the checks' fixed proxy).
    pub fn with_proxy(proxy: Arc<dyn ProxyResolver>) -> MacHttps {
        MacHttps { proxy }
    }
}

impl Default for MacHttps {
    fn default() -> Self {
        MacHttps::new()
    }
}

impl std::fmt::Debug for MacHttps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MacHttps")
    }
}

impl HttpsSend for MacHttps {
    fn get(&self, request: HttpsRequest, max_body: usize) -> BoxFuture<'_, Result<HttpsResponse>> {
        Box::pin(async move {
            let target = split(&request.url)?;
            let timeout = request.timeout;
            within(timeout, send(self.proxy.as_ref(), target, request, vec![], max_body)).await
        })
    }

    fn post(&self, request: HttpsRequest, body: Vec<u8>, max_body: usize) -> BoxFuture<'_, Result<HttpsResponse>> {
        Box::pin(async move {
            let target = split_post(&request.url)?;
            let timeout = request.timeout;
            within(timeout, send(self.proxy.as_ref(), target, request, body, max_body)).await
        })
    }
}

/// The Platform's HTTPS (the push's POST, and a GET under the same rules).
pub fn https() -> Arc<dyn HttpsSend> {
    Arc::new(MacHttps::new())
}

/// The account usage's GET (src-tauri platform.rs `Ports`; which URLs may be asked is mighty-accounts' `usage::guard`).
pub fn https_get() -> Arc<dyn HttpsSend> {
    Arc::new(MacHttps::new())
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use mighty_platform::fake::FakeProxy;
    use tokio::net::TcpListener;

    use super::*;

    fn request(url: &str, headers: &[(&str, &str)]) -> HttpsRequest {
        HttpsRequest {
            url: url.into(),
            headers: headers.iter().map(|(n, v)| (n.to_string(), v.to_string())).collect(),
            timeout: Duration::from_secs(5),
        }
    }

    /// A GET's URL: `https://host/path?query` only, on HTTPS's own port (the same table as Windows').
    #[test]
    fn only_an_https_address_on_its_own_port_is_asked() {
        let target = split("https://API.anthropic.com/api/oauth/usage?at_wall=1&skip_spend=1").unwrap();
        assert_eq!(
            (target.host.as_str(), target.port, target.path.as_str()),
            ("api.anthropic.com", 443, "/api/oauth/usage?at_wall=1&skip_spend=1")
        );
        assert_eq!(split("https://api.anthropic.com").unwrap().path, "/");
        for bad in [
            "http://api.anthropic.com/x",
            "https://api.anthropic.com:8443/x",
            "https://user@api.anthropic.com/x",
            "https://api.anthropic.com/x#frag",
            "https:///x",
            "ftp://x/y",
            "https://api.anthropic.com\\evil.test/x",
            "https://api.anthropic.com%2Fevil.test/x",
            "https://api anthropic.com/x",
            "https://[::1]/x",
            "https://\u{0430}pi.anthropic.com/x",
            "https://api.anthropic.com\u{0}/x",
        ] {
            assert!(split(bad).is_err(), "{bad}");
        }
    }

    /// The push's route: `https://` or `http://`, any port, a name or an address; nothing that could carry credentials or a fragment.
    #[test]
    fn a_push_route_is_split_as_posted() {
        let split = |url: &str| {
            let t = split_post(url).unwrap();
            (t.secure, t.host, t.port, t.path)
        };
        assert_eq!(
            split("https://relay.example.invalid/push/v1/send"),
            (true, "relay.example.invalid".into(), 443, "/push/v1/send".into())
        );
        assert_eq!(split("http://127.0.0.1:8787/push/v1/send"), (false, "127.0.0.1".into(), 8787, "/push/v1/send".into()));
        assert_eq!(split("http://[::1]:9/x"), (false, "::1".into(), 9, "/x".into()));
        for bad in [
            "ftp://x/y",
            "https://user@x/y",
            "https://x/y#z",
            "https://x:0/y",
            "https://x:99999/y",
            "https:///y",
            "https://a b/y",
            "http://x:1:2/y",
        ] {
            assert!(split_post(bad).is_err(), "{bad}");
        }
        let v6 = split_post("http://[::1]:9/x").unwrap();
        assert_eq!((v6.authority(), v6.origin()), ("[::1]:9".to_string(), "http://[::1]:9/".to_string()));
        assert_eq!(split_post("https://relay.example/x").unwrap().authority(), "relay.example", "the scheme's own port is left out");
    }

    /// A request that could carry a second request is refused before anything is sent, and Debug names the headers only.
    #[test]
    fn a_request_that_could_go_elsewhere_is_refused_before_anything_is_sent() {
        assert!(headers(&request("https://a.example/x", &[("Accept", "application/json")])).is_ok());
        for bad in [
            request("https://a.example/x", &[("Authorization", "Bearer a\r\nX-Evil: 1")]),
            request("https://a.example/x", &[("Bad:Name", "v")]),
            request("https://a.example/x", &[("", "v")]),
            request("https://a.example/x\r\nHost: evil.test", &[]),
        ] {
            assert!(headers(&bad).is_err(), "{bad:?}");
        }
        let shown = format!("{:?}", request("https://a.example/x", &[("Authorization", "Bearer sk-FIXTURE")]));
        assert!(shown.contains("Authorization") && !shown.contains("sk-FIXTURE"), "{shown}");
    }

    /// One loopback HTTP server answering each connection with `reply`; the requests it read come back.
    async fn server(reply: &'static str) -> (u16, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = Arc::new(Mutex::new(vec![]));
        let record = seen.clone();
        tokio::spawn(async move {
            while let Ok((mut client, _)) = listener.accept().await {
                let record = record.clone();
                tokio::spawn(async move {
                    let mut read = Vec::new();
                    let mut buffer = [0u8; 16_384];
                    let complete = |read: &[u8]| {
                        let Some(end) = read.windows(4).position(|w| w == b"\r\n\r\n") else { return false };
                        let head = String::from_utf8_lossy(&read[..end]).to_ascii_lowercase();
                        let length = head.lines().find_map(|line| line.strip_prefix("content-length:")).and_then(|v| v.trim().parse().ok());
                        read.len() >= end + 4 + length.unwrap_or(0)
                    };
                    while !complete(&read) {
                        match client.read(&mut buffer).await {
                            Ok(0) | Err(_) => break,
                            Ok(count) => read.extend_from_slice(&buffer[..count]),
                        }
                    }
                    record.lock().unwrap().push(String::from_utf8_lossy(&read).into_owned());
                    let _ = client.write_all(reply.as_bytes()).await;
                    let _ = client.shutdown().await;
                });
            }
        });
        (port, seen)
    }

    /// The push's POST to a loopback relay: the body sent whole with its length and headers, the status and `Retry-After` read, the
    /// body of the answer not read; a 3xx is never followed and comes back with its `Location`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_post_sends_its_body_and_never_follows_a_redirect() {
        let https = MacHttps::with_proxy(Arc::new(FakeProxy::default()));
        let (port, seen) =
            server("HTTP/1.1 429 Too Many Requests\r\nRetry-After: 7\r\nContent-Length: 2\r\nConnection: close\r\n\r\nno").await;
        let url = format!("http://127.0.0.1:{port}/push/v1/send");
        let answer = https
            .post(request(&url, &[("Authorization", "Bearer t"), ("Content-Type", "application/json")]), b"{\"a\":1}".to_vec(), 64)
            .await
            .unwrap();
        assert_eq!((answer.status, answer.retry_after.as_deref(), answer.body.as_str()), (429, Some("7"), ""));
        let head = seen.lock().unwrap()[0].clone();
        assert!(head.starts_with("POST /push/v1/send HTTP/1.1\r\n"), "{head}");
        let lower = head.to_ascii_lowercase();
        assert!(
            lower.contains("authorization: bearer t\r\n") && lower.contains("content-length: 7\r\n") && head.ends_with("{\"a\":1}"),
            "{head}"
        );
        assert!(lower.contains(&format!("host: 127.0.0.1:{port}\r\n")), "{head}");

        let (elsewhere, reached) = server("HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n").await;
        let redirect: &'static str = Box::leak(
            format!("HTTP/1.1 307 Temporary Redirect\r\nLocation: http://127.0.0.1:{elsewhere}/x\r\nContent-Length: 0\r\n\r\n")
                .into_boxed_str(),
        );
        let (port, _) = server(redirect).await;
        let answer = https.post(request(&format!("http://127.0.0.1:{port}/push/v1/send"), &[]), vec![], 64).await.unwrap();
        assert_eq!((answer.status, answer.redirect_location), (307, Some(format!("http://127.0.0.1:{elsewhere}/x"))));
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(reached.lock().unwrap().is_empty(), "the redirect is not followed");
        let (port, _) = server("HTTP/1.1 302 Found\r\nContent-Length: 0\r\n\r\n").await;
        let answer = https.post(request(&format!("http://127.0.0.1:{port}/"), &[]), vec![], 64).await.unwrap();
        assert_eq!(answer.redirect_location.as_deref(), Some("refused"), "a 3xx with no Location");
    }

    /// Plain HTTP through a proxy goes in the absolute form to the proxy; HTTPS opens a `CONNECT` tunnel first, and a proxy that
    /// refuses it fails the request without saying more than its status.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_proxy_gets_the_absolute_form_or_a_tunnel() {
        let (proxy_port, seen) = server("HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n").await;
        let proxy = FakeProxy::default();
        proxy.set(Proxy::Http { host: "127.0.0.1".into(), port: proxy_port });
        let https = MacHttps::with_proxy(Arc::new(proxy));
        let answer = https.post(request("http://relay.example.invalid:8787/push/v1/send", &[]), b"x".to_vec(), 64).await.unwrap();
        assert_eq!(answer.status, 204);
        let head = seen.lock().unwrap()[0].clone();
        assert!(head.starts_with("POST http://relay.example.invalid:8787/push/v1/send HTTP/1.1\r\n"), "{head}");

        let (refusing, asked) = server("HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n").await;
        let proxy = FakeProxy::default();
        proxy.set(Proxy::Http { host: "127.0.0.1".into(), port: refusing });
        let https = MacHttps::with_proxy(Arc::new(proxy));
        let error =
            https.get(request("https://api.example.invalid/x?token=SECRET", &[("Authorization", "Bearer SECRET")]), 64).await.unwrap_err();
        assert!(matches!(error, PlatformError::Refused(_)) && !error.to_string().contains("SECRET"), "{error}");
        let head = asked.lock().unwrap()[0].clone();
        assert!(head.starts_with("CONNECT api.example.invalid:443 HTTP/1.1\r\n") && !head.contains("SECRET"), "{head}");
    }

    /// A 2xx GET body past the cap is refused; the whole request ends within its timeout.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_large_body_is_refused_and_a_slow_server_times_out() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((client, _)) = listener.accept().await {
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    drop(client);
                });
            }
        });
        let https = MacHttps::with_proxy(Arc::new(FakeProxy::default()));
        let started = std::time::Instant::now();
        let mut slow = request(&format!("http://127.0.0.1:{port}/push/v1/send"), &[]);
        slow.timeout = Duration::from_millis(200);
        assert!(matches!(https.post(slow, vec![], 64).await, Err(PlatformError::TimedOut(_))));
        assert!(started.elapsed() < Duration::from_secs(2), "given up on at the timeout");
        // The body cap applies to a GET; a GET is HTTPS only, so the cap is checked on the reading code directly.
        let (port, _) = server("HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\n0123456789").await;
        let target = Target { post: false, secure: false, host: "127.0.0.1".into(), port, path: "/".into() };
        let read = send(&FakeProxy::default(), target.clone(), request("https://unused.example/", &[]), vec![], 4).await;
        assert!(matches!(read, Err(PlatformError::TooLarge(4))), "{read:?}");
        let read = send(&FakeProxy::default(), target, request("https://unused.example/", &[]), vec![], 64).await.unwrap();
        assert_eq!((read.status, read.body.as_str()), (200, "0123456789"));
    }
}
