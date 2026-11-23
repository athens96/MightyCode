//! MC-34: the allow-list proxy of one sandboxed run: an HTTP proxy on `127.0.0.1` the CLI reaches through `HTTPS_PROXY` /
//! `HTTP_PROXY`. Under Seatbelt the run may connect to nothing else (the profile opens only this port), so it cannot go around it.
//!
//! It takes `CONNECT host:port` (HTTPS and anything tunnelled) and plain-HTTP requests in absolute form (`GET http://host/…`). Each
//! target is put to the [`Policy`] (the allow list, then the card); a refused one gets `403` and is closed. A listed *name* is
//! resolved here and never connected to an address on this computer, a link-local one (the cloud metadata service) or an unspecified
//! one ([`hosts::reachable`]); an IP literal is only ever let through when the person listed that literal.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Semaphore, watch};

use super::hosts::{self, Target};
use mighty_platform::BoxFuture;

/// The most a request's head may hold.
pub const HEAD_LIMIT: usize = 16 * 1024;
/// How long a client has to send its request head.
pub const HEAD_TIME: Duration = Duration::from_secs(15);
/// How long a connection to the target may take.
pub const CONNECT_TIME: Duration = Duration::from_secs(10);
/// The most connections at once.
pub const CONNECTIONS: usize = 256;

/// Whether a run may reach a target (the allow list answers at once; anything else may wait for the person).
pub trait Policy: Send + Sync + 'static {
    fn decide(&self, target: Target) -> BoxFuture<'static, bool>;
}

/// A running proxy. Dropping it stops it (open tunnels end with their run's process).
pub struct Proxy {
    pub port: u16,
    stop: watch::Sender<bool>,
}

impl Drop for Proxy {
    fn drop(&mut self) {
        self.stop.send_replace(true);
    }
}

impl Proxy {
    /// Listens on `127.0.0.1` (a free port).
    pub async fn start(policy: Arc<dyn Policy>) -> std::io::Result<Proxy> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let port = listener.local_addr()?.port();
        let (stop, stopped) = watch::channel(false);
        let limit = Arc::new(Semaphore::new(CONNECTIONS));
        let mut accepting = stopped.clone();
        tokio::spawn(async move {
            loop {
                let accepted = tokio::select! {
                    accepted = listener.accept() => accepted,
                    _ = accepting.wait_for(|s| *s) => return,
                };
                let Ok((stream, _)) = accepted else { continue };
                let Ok(permit) = limit.clone().try_acquire_owned() else { continue };
                let policy = policy.clone();
                let mut ended = stopped.clone();
                tokio::spawn(async move {
                    tokio::select! {
                        _ = serve(stream, policy) => {}
                        _ = ended.wait_for(|s| *s) => {}
                    }
                    drop(permit);
                });
            }
        });
        Ok(Proxy { port, stop })
    }

    pub fn stop(&self) {
        self.stop.send_replace(true);
    }

    /// The environment a CLI is given to use it (both cases: some tools read only one).
    pub fn environment(&self) -> Vec<(String, String)> {
        let url = format!("http://127.0.0.1:{}", self.port);
        let mut out = Vec::new();
        for name in ["HTTPS_PROXY", "HTTP_PROXY", "https_proxy", "http_proxy", "ALL_PROXY", "all_proxy"] {
            out.push((name.to_string(), url.clone()));
        }
        for name in ["NO_PROXY", "no_proxy"] {
            out.push((name.to_string(), "localhost,127.0.0.1,::1".to_string()));
        }
        out
    }
}

/// What a request head asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Asked {
    /// `CONNECT host:port`.
    Tunnel(Target),
    /// A plain-HTTP request in absolute form, sent on as it came.
    Forward(Target),
}

/// The target of a request head's first line; None for anything the proxy does not take.
pub fn parse_request_line(line: &str) -> Option<Asked> {
    let mut parts = line.split(' ');
    let (method, target, version) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() || !version.starts_with("HTTP/1.") {
        return None;
    }
    if method.eq_ignore_ascii_case("CONNECT") {
        return Target::parse(target, 443).map(Asked::Tunnel);
    }
    let rest = target.strip_prefix("http://").or_else(|| target.strip_prefix("HTTP://"))?;
    let authority = rest.split(['/', '?', '#']).next()?;
    if authority.contains('@') {
        return None;
    }
    Target::parse(authority, 80).map(Asked::Forward)
}

async fn refuse(stream: &mut TcpStream, status: &str, why: &str) {
    let body = format!("Mighty Code sandbox: {why}\r\n");
    let reply = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\nX-Mighty-Sandbox: blocked\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(reply.as_bytes()).await;
    let _ = stream.shutdown().await;
}

/// Reads the head (to the blank line); the head and what came after it.
async fn read_head(stream: &mut TcpStream) -> Option<(Vec<u8>, usize)> {
    let mut buffer = Vec::with_capacity(1024);
    let mut chunk = [0u8; 2048];
    loop {
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(at) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
            return Some((buffer, at + 4));
        }
        if buffer.len() > HEAD_LIMIT {
            return None;
        }
    }
}

/// The addresses to try for `target`: the literal itself, or a listed name's addresses that are not on this computer (a name the
/// person listed that is this computer, `localhost:3000`, reaches it).
pub async fn addresses(target: &Target) -> Vec<SocketAddr> {
    if let Some(ip) = target.ip() {
        return vec![SocketAddr::new(ip, target.port)];
    }
    let Ok(found) = tokio::net::lookup_host((target.host.as_str(), target.port)).await else { return vec![] };
    let found: Vec<SocketAddr> = found.collect();
    if target.loopback() {
        return found.into_iter().filter(|a| a.ip().is_loopback()).collect();
    }
    // One address of a name on this computer is enough to refuse the name (a rebinding name may answer either way).
    if found.iter().any(|a| !hosts::reachable(&a.ip())) {
        return vec![];
    }
    found
}

async fn serve(mut stream: TcpStream, policy: Arc<dyn Policy>) {
    let Ok(Some((head, end))) = tokio::time::timeout(HEAD_TIME, read_head(&mut stream)).await else {
        return refuse(&mut stream, "400 Bad Request", "no request").await;
    };
    let first = String::from_utf8_lossy(&head[..head.iter().position(|b| *b == b'\r').unwrap_or(0)]).into_owned();
    let Some(asked) = parse_request_line(&first) else {
        return refuse(&mut stream, "400 Bad Request", "only CONNECT and absolute http:// requests are taken").await;
    };
    let target = match &asked {
        Asked::Tunnel(t) | Asked::Forward(t) => t.clone(),
    };
    if !policy.decide(target.clone()).await {
        return refuse(&mut stream, "403 Forbidden", &format!("{} is not on the allow list", target.label())).await;
    }
    let places = addresses(&target).await;
    if places.is_empty() {
        return refuse(&mut stream, "403 Forbidden", &format!("{} does not resolve to an address it may reach", target.label())).await;
    }
    let mut upstream = None;
    for place in places {
        if let Ok(Ok(connected)) = tokio::time::timeout(CONNECT_TIME, TcpStream::connect(place)).await {
            upstream = Some(connected);
            break;
        }
    }
    let Some(mut upstream) = upstream else {
        return refuse(&mut stream, "502 Bad Gateway", &format!("{} could not be reached", target.label())).await;
    };
    let _ = upstream.set_nodelay(true);
    match asked {
        Asked::Tunnel(_) => {
            if stream.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n").await.is_err() {
                return;
            }
            if end < head.len() && upstream.write_all(&head[end..]).await.is_err() {
                return;
            }
        }
        Asked::Forward(_) => {
            if upstream.write_all(&head).await.is_err() {
                return;
            }
        }
    }
    let _ = tokio::io::copy_bidirectional(&mut stream, &mut upstream).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct ListPolicy {
        list: hosts::AllowList,
        asked: Mutex<Vec<Target>>,
    }

    impl Policy for ListPolicy {
        fn decide(&self, target: Target) -> BoxFuture<'static, bool> {
            let allowed = self.list.allows(&target);
            self.asked.lock().unwrap().push(target);
            Box::pin(async move { allowed })
        }
    }

    /// A server that answers every connection with `hello` and its first line.
    async fn echo_server() -> u16 {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut buffer = [0u8; 1024];
                    let read = stream.read(&mut buffer).await.unwrap_or(0);
                    let line = String::from_utf8_lossy(&buffer[..read]).lines().next().unwrap_or("").to_string();
                    let _ = stream.write_all(format!("hello {line}\n").as_bytes()).await;
                });
            }
        });
        port
    }

    async fn ask(port: u16, text: &str) -> String {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        stream.write_all(text.as_bytes()).await.unwrap();
        let mut out = Vec::new();
        let _ = tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut out)).await;
        String::from_utf8_lossy(&out).into_owned()
    }

    #[test]
    fn request_lines_are_read_strictly() {
        assert_eq!(
            parse_request_line("CONNECT github.com:443 HTTP/1.1"),
            Some(Asked::Tunnel(Target { host: "github.com".into(), port: 443 }))
        );
        assert_eq!(
            parse_request_line("GET http://Example.com/x?y HTTP/1.1"),
            Some(Asked::Forward(Target { host: "example.com".into(), port: 80 }))
        );
        assert_eq!(
            parse_request_line("GET http://example.com:8080 HTTP/1.0").map(|a| matches!(a, Asked::Forward(t) if t.port == 8080)),
            Some(true)
        );
        for bad in [
            "GET / HTTP/1.1",
            "GET https://example.com/ HTTP/1.1",
            "GET http://u:p@example.com/ HTTP/1.1",
            "CONNECT a b HTTP/1.1",
            "CONNECT x:1 SPDY/3",
        ] {
            assert_eq!(parse_request_line(bad), None, "{bad}");
        }
    }

    /// CONNECT to a listed target is tunnelled; to anything else, refused with 403: an IP literal for a listed name, the same name on
    /// another port, a name not listed, and a request that is not a proxy request at all.
    #[tokio::test]
    async fn the_proxy_lets_through_only_what_is_listed() {
        let target = echo_server().await;
        let other = echo_server().await;
        let policy = Arc::new(ListPolicy {
            list: hosts::AllowList::new("", &[format!("localhost:{target}"), "github.com".into()]),
            asked: Mutex::new(vec![]),
        });
        let proxy = Proxy::start(policy.clone()).await.unwrap();
        let reply = ask(proxy.port, &format!("CONNECT localhost:{target} HTTP/1.1\r\nHost: x\r\n\r\nPING\r\n")).await;
        assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");
        assert!(reply.contains("hello PING"), "the tunnel carries the bytes: {reply}");

        let reply = ask(proxy.port, &format!("CONNECT 127.0.0.1:{target} HTTP/1.1\r\n\r\n")).await;
        assert!(reply.starts_with("HTTP/1.1 403"), "an IP literal of a listed name is not listed: {reply}");
        let reply = ask(proxy.port, &format!("CONNECT localhost:{other} HTTP/1.1\r\n\r\n")).await;
        assert!(reply.starts_with("HTTP/1.1 403") && reply.contains("X-Mighty-Sandbox: blocked"), "another port: {reply}");
        let reply = ask(proxy.port, "CONNECT example.com:443 HTTP/1.1\r\n\r\n").await;
        assert!(reply.starts_with("HTTP/1.1 403"), "{reply}");
        let reply = ask(proxy.port, "GET / HTTP/1.1\r\nHost: localhost\r\n\r\n").await;
        assert!(reply.starts_with("HTTP/1.1 400"), "{reply}");

        let reply = ask(proxy.port, &format!("GET http://localhost:{target}/path HTTP/1.1\r\nHost: localhost\r\n\r\n")).await;
        assert!(reply.contains("hello GET http://localhost:"), "a plain request is sent on as it came: {reply}");
        let reply = ask(proxy.port, &format!("GET http://localhost:{other}/ HTTP/1.1\r\n\r\n")).await;
        assert!(reply.starts_with("HTTP/1.1 403"), "{reply}");
        let asked = policy.asked.lock().unwrap().clone();
        assert!(asked.contains(&Target { host: "127.0.0.1".into(), port: target }));
        assert!(asked.contains(&Target { host: "example.com".into(), port: 443 }));
        drop(proxy);
    }

    /// A listed name that resolves to this computer is not connected to (it would reach this computer's services).
    #[tokio::test]
    async fn a_listed_name_that_resolves_here_is_refused() {
        // `localhost.` without the loopback exception: a name under a listed wildcard that resolves to 127.0.0.1 is refused. Tested
        // on the address rule itself, since no public name can be relied on in a check.
        let found = addresses(&Target { host: "127.0.0.1".into(), port: 1 }).await;
        assert_eq!(found.len(), 1, "a literal is used as it is (the person listed it)");
        assert!(!hosts::reachable(&"127.0.0.1".parse().unwrap()));
        let local = addresses(&Target { host: "localhost".into(), port: 9 }).await;
        assert!(local.iter().all(|a| a.ip().is_loopback()));
    }
}
