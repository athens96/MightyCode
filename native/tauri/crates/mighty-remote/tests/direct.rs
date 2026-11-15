//! C#'s `MobileDirectVerification` (Tests\MobileDirectVerification.cs; plan W8-C), ported by name: the phone's direct path to this PC.
//! Loopback stands in for Tailscale only through the path's address seam and the fake network (which binds loopback alone); nothing
//! here binds a real Tailscale address or runs netsh.
//!
//! The checks that need the relay host's connection — L492 "a phone connects directly on the relay's handshake", L500 "relay notices
//! carrying a direct- id leave the direct phone alone", L502 "a relay auth frame that is no JSON is answered malformed", and the relay
//! halves of L493, L494 and L501 (pairing, the relay redial, `/m1/info`, the cipher's 28 bytes) — run W8-A's `RelayHost` as the sink and
//! land with it. Here the sink is [`Host`], a stand-in with the relay host's admission (`direct::admit`, one check-and-insert under its
//! lock) and a one-word auth.

use std::collections::VecDeque;
use std::net::{IpAddr, SocketAddr, SocketAddrV6};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use mighty_core::locale::{LANGUAGES, Locale, catalogue};
use mighty_platform::fake::FakeNetwork;
use mighty_platform::{BoxFuture, FirewallChange, FirewallRule, PlatformError};
use mighty_remote::direct::admit::{self, DirectAuthFailures};
use mighty_remote::direct::listener::{
    AddressSource, DirectIncoming, DirectListener, DirectPath, DirectPathOptions, DirectReservation, DirectSettings, DirectSink,
};
use mighty_remote::direct::status::{self, DirectState, MobileDirectStatus};
use mighty_remote::direct::strings;
use mighty_remote::direct::upgrade::{MAXIMUM_HEAD_BYTES, accept_key, accept_web_socket, upgrade_response};
use mighty_remote::direct::wire::{self, CONNECTION_LIMIT};
use mighty_remote::tailscale;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::TcpStream;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;

const REQUEST: &str = "GET /ws?v=1 HTTP/1.1\r\nHost: 100.101.102.103:43139\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n";

fn ko() -> Locale {
    Locale::new("ko")
}

fn log() -> Arc<dyn Fn(&str) + Send + Sync> {
    Arc::new(|_| {})
}

/// Polls `test` every 20 ms for up to 10 s.
async fn until(mut test: impl FnMut() -> bool) -> bool {
    for _ in 0..500 {
        if test() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    test()
}

/// Whether the other side ended the connection (a reset or a close) within `ms`.
async fn reads_end(stream: &mut TcpStream, ms: u64) -> bool {
    let mut byte = [0u8; 1];
    match tokio::time::timeout(Duration::from_millis(ms), stream.read(&mut byte)).await {
        Ok(Ok(0)) | Ok(Err(_)) => true,
        Ok(Ok(_)) | Err(_) => false,
    }
}

/// A connection the listener refuses unread (reset, linger 0, or closed): either its first read ends, or — where the reset arrives
/// before the connect is seen to complete, as macOS reports it (ECONNRESET from the connect itself) — the connect fails with the reset.
/// Any other connect failure is no refusal by this listener.
async fn refused(endpoint: impl tokio::net::ToSocketAddrs, ms: u64) -> bool {
    match TcpStream::connect(endpoint).await {
        Ok(mut stream) => reads_end(&mut stream, ms).await,
        Err(error) => matches!(
            error.kind(),
            std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted | std::io::ErrorKind::BrokenPipe
        ),
    }
}

/// A phone's WebSocket to `endpoint`.
async fn phone(endpoint: &str) -> Result<WebSocketStream<TcpStream>, String> {
    let tcp = TcpStream::connect(endpoint).await.map_err(|e| e.to_string())?;
    let (socket, _) = tokio_tungstenite::client_async(format!("ws://{endpoint}/ws?v=1"), tcp).await.map_err(|e| e.to_string())?;
    Ok(socket)
}

async fn exchange(socket: &mut WebSocketStream<TcpStream>, text: &str) -> Option<String> {
    socket.send(Message::text(text)).await.ok()?;
    match tokio::time::timeout(Duration::from_secs(10), socket.next()).await.ok()?? {
        Ok(Message::Text(answer)) => Some(answer.to_string()),
        _ => None,
    }
}

/// Whether the host ended the phone's connection: the next read is a close, a failure or the end, not a frame.
async fn ended(socket: &mut WebSocketStream<TcpStream>) -> bool {
    loop {
        match tokio::time::timeout(Duration::from_secs(10), socket.next()).await {
            Ok(None) | Ok(Some(Err(_))) | Ok(Some(Ok(Message::Close(_)))) => return true,
            Ok(Some(Ok(_))) => continue,
            Err(_) => return false,
        }
    }
}

#[derive(Default)]
struct Table {
    /// Admitted, not yet upgraded: each reservation's id and peer.
    reserved: Vec<(u64, String)>,
    next: u64,
    /// Every id admitted, accepted and given back, in order.
    admitted: Vec<u64>,
    accepted: Vec<u64>,
    refused: Vec<u64>,
    /// Upgraded, not yet authenticated.
    pending: Vec<String>,
    authenticated: usize,
}

/// The relay host's part as the direct path sees it (C#'s `AcceptDirect` admission and `Connection`'s auth, reduced to one word):
/// "auth-ok" authenticates and is answered "ok", "auth-bad" counts a failure of the peer and is answered `unauthorized` then closed,
/// anything else after auth is echoed; a phone not authenticated by the path's deadline is closed then.
#[derive(Default)]
struct HostInner {
    table: Mutex<Table>,
    failures: DirectAuthFailures,
}

#[derive(Clone, Default)]
struct Host(Arc<HostInner>);

impl Host {
    fn table(&self) -> std::sync::MutexGuard<'_, Table> {
        self.0.table.lock().unwrap()
    }
    fn connections(&self) -> usize {
        let table = self.table();
        table.reserved.len() + table.pending.len() + table.authenticated
    }
}

struct Leaving(Host, String, Arc<AtomicBool>);

impl Drop for Leaving {
    fn drop(&mut self) {
        let mut table = self.0.table();
        if self.2.load(Ordering::SeqCst) {
            table.authenticated -= 1;
        } else if let Some(at) = table.pending.iter().position(|p| *p == self.1) {
            table.pending.remove(at);
        }
    }
}

impl DirectSink for Host {
    fn admit(&self, peer: IpAddr) -> Option<DirectReservation> {
        let peer = tailscale::canonical(peer);
        let mut table = self.table();
        let pending: Vec<String> = table.reserved.iter().map(|(_, p)| p.clone()).chain(table.pending.iter().cloned()).collect();
        let clients = table.reserved.len() + table.pending.len() + table.authenticated;
        if !admit::admit(Some(&peer), true, clients, &pending, self.0.failures.recent(&peer, Instant::now())) {
            return None;
        }
        table.next += 1;
        let id = table.next;
        table.reserved.push((id, peer));
        table.admitted.push(id);
        Some(DirectReservation::new(id))
    }

    fn accept(
        &self,
        reservation: DirectReservation,
        mut socket: WebSocketStream<TcpStream>,
        peer: SocketAddr,
        deadline: tokio::time::Instant,
    ) -> BoxFuture<'static, ()> {
        let host = self.clone();
        let peer = tailscale::canonical(peer.ip());
        {
            let mut table = host.table();
            let at = table.reserved.iter().position(|(id, _)| *id == reservation.id()).expect("its own reservation");
            let (_, reserved) = table.reserved.remove(at);
            assert_eq!(reserved, peer, "the reservation is the peer's");
            table.accepted.push(reservation.id());
            table.pending.push(peer.clone());
        }
        let authenticated = Arc::new(AtomicBool::new(false));
        let leaving = Leaving(host.clone(), peer.clone(), authenticated.clone());
        Box::pin(async move {
            let _leaving = leaving;
            loop {
                // Until auth_ok, the one deadline taken when the socket was accepted (C#'s 10 s from the accept); after, none.
                let next = if authenticated.load(Ordering::SeqCst) {
                    socket.next().await
                } else {
                    match tokio::time::timeout_at(deadline, socket.next()).await {
                        Ok(next) => next,
                        Err(_) => return,
                    }
                };
                let Some(Ok(message)) = next else { return };
                let Message::Text(text) = message else { continue };
                if !authenticated.load(Ordering::SeqCst) {
                    if text.as_str() == "auth-ok" {
                        {
                            let mut table = host.table();
                            if let Some(at) = table.pending.iter().position(|p| *p == peer) {
                                table.pending.remove(at);
                            }
                            table.authenticated += 1;
                            authenticated.store(true, Ordering::SeqCst);
                        }
                        let _ = socket.send(Message::text("ok")).await;
                    } else {
                        host.0.failures.record(&peer, Instant::now());
                        let _ = socket.send(Message::text(wire::GENERIC_REFUSAL)).await;
                        let _ = socket.close(None).await;
                        return;
                    }
                } else {
                    let _ = socket.send(Message::text(text.as_str())).await;
                }
            }
        })
    }

    fn refused(&self, reservation: DirectReservation) {
        let mut table = self.table();
        let at = table.reserved.iter().position(|(id, _)| *id == reservation.id()).expect("its own reservation, given back once");
        table.reserved.remove(at);
        table.refused.push(reservation.id());
    }
}

/// An address source the checks swap whole: `None` stands for adapters that cannot be read.
fn source(addresses: &Arc<Mutex<Option<Vec<String>>>>) -> AddressSource {
    let addresses = addresses.clone();
    Arc::new(move || addresses.lock().unwrap().clone().ok_or_else(|| "adapters unreadable".to_string()))
}

fn on() -> DirectSettings {
    DirectSettings { enabled: true, direct_enabled: true, port: wire::DEFAULT_PORT }
}

/// "mobile direct: endpoints, upgrade, admission and status" (L489).
#[test]
fn mobile_direct_endpoints_upgrade_admission_and_status() {
    // Endpoints: host:port, IPv6 in brackets; only Tailscale addresses, each once, at most eight.
    assert!(
        wire::endpoint("100.101.102.103", 43139) == "100.101.102.103:43139"
            && wire::endpoint("fd7a:115c:a1e0::1", 43139) == "[fd7a:115c:a1e0::1]:43139"
    );
    let bound = |items: &[(&str, u16)]| items.iter().map(|(h, p)| (h.to_string(), *p)).collect::<Vec<_>>();
    assert_eq!(
        wire::advertised(
            &bound(&[("100.101.102.103", 43139), ("fd7a:115c:a1e0::1", 43139), ("192.168.0.5", 43139), ("100.101.102.103", 43139)]),
            false
        ),
        vec!["100.101.102.103:43139", "[fd7a:115c:a1e0::1]:43139"],
        "only Tailscale endpoints, each once"
    );
    assert!(
        wire::advertised(&bound(&[("127.0.0.1", 50000)]), true) == vec!["127.0.0.1:50000"]
            && wire::advertised(&bound(&[("127.0.0.1", 50000)]), false).is_empty(),
        "loopback is advertised only through the seam"
    );
    let twelve: Vec<(String, u16)> = (1..=12).map(|i| (format!("100.101.0.{i}"), 43139)).collect();
    assert_eq!(wire::advertised(&twelve, false).len(), wire::MAXIMUM_ENDPOINTS, "at most eight endpoints");
    assert!(
        wire::valid_port(1024) && wire::valid_port(65535) && !wire::valid_port(1023) && !wire::valid_port(65536) && !wire::valid_port(0)
    );
    assert!(wire::is_direct("direct-0f8fad5b-d9cb-469f-a165-70867728950e") && !wire::is_direct("0f8fad5b-d9cb-469f-a165-70867728950e"));
    assert!(
        wire::DEFAULT_PORT == 43139
            && CONNECTION_LIMIT == 36
            && wire::BACKLOG == 36
            && wire::MAXIMUM_FRAME_BYTES == 1024 * 1024
            && wire::HANDSHAKE_DEADLINE == Duration::from_secs(10)
            && admit::FAILURE_WINDOW == Duration::from_secs(60)
            && wire::REDETECT_INTERVAL == Duration::from_secs(15)
            && wire::GENERIC_REFUSAL == "unauthorized",
        "the Mac's limits"
    );

    // The phone's upgrade (RFC 6455 section 1.3's sample key): GET /ws, HTTP/1.1, websocket, version 13, a 16-byte key.
    assert_eq!(accept_key("dGhlIHNhbXBsZSBub25jZQ=="), "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=", "the accept key");
    assert_eq!(
        upgrade_response(REQUEST).as_deref(),
        Some(
            "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n\r\n"
        ),
        "a phone's upgrade is accepted"
    );
    assert!(
        upgrade_response(
            &REQUEST.replace("Connection: Upgrade", "Connection: keep-alive, Upgrade").replace("Upgrade: websocket", "upgrade: WebSocket")
        )
        .is_some(),
        "header names and tokens are case-insensitive"
    );
    for bad in [
        REQUEST.replace("GET /ws?v=1", "GET /other"),
        REQUEST.replace("GET", "POST"),
        REQUEST.replace("HTTP/1.1", "HTTP/1.0"),
        REQUEST.replace("Sec-WebSocket-Version: 13", "Sec-WebSocket-Version: 8"),
        REQUEST.replace("Upgrade: websocket\r\n", ""),
        REQUEST.replace("dGhlIHNhbXBsZSBub25jZQ==", "c2hvcnQ="),
        REQUEST.replace("Connection: Upgrade", "Connection: close"),
    ] {
        assert!(upgrade_response(&bad).is_none(), "a refused upgrade: {}", bad.split("\r\n").next().unwrap());
    }

    // Admission at accept, before anything is read.
    let tailnet = "100.101.102.103";
    let peers = |items: &[&str]| items.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert!(admit::admit(Some(tailnet), false, 0, &[], 0), "a tailnet peer gets a slot");
    assert!(
        !admit::admit(Some("192.168.0.5"), false, 0, &[], 0)
            && !admit::admit(Some("127.0.0.1"), false, 0, &[], 0)
            && !admit::admit(None, false, 0, &[], 0),
        "a peer outside the ranges is dropped"
    );
    assert!(admit::admit(Some("127.0.0.1"), true, 0, &[], 0), "loopback only through the seam");
    assert!(!admit::admit(Some(tailnet), false, wire::MAXIMUM_CLIENTS, &[], 0), "a full phone table admits none");
    assert!(
        !admit::admit(Some(tailnet), false, 4, &peers(&["100.64.0.1", "100.64.0.2", "100.64.0.3", "100.64.0.4"]), 0),
        "four handshakes"
    );
    assert!(
        !admit::admit(Some(tailnet), false, 2, &peers(&[tailnet, tailnet]), 0)
            && admit::admit(Some(tailnet), false, 2, &peers(&[tailnet, "100.64.0.1"]), 0),
        "two handshakes per address"
    );
    assert!(
        !admit::admit(Some(tailnet), false, 0, &[], admit::MAXIMUM_FAILURES)
            && admit::admit(Some(tailnet), false, 0, &[], admit::MAXIMUM_FAILURES - 1),
        "five failed auths shut the address out"
    );

    // Failed auths per address inside 60 s.
    let failures = DirectAuthFailures::default();
    let start = Instant::now();
    let at = |seconds: u64| start + Duration::from_secs(seconds);
    for i in 0..5 {
        failures.record(tailnet, at(i));
    }
    assert!(failures.recent(tailnet, at(10)) == 5 && failures.recent("100.64.0.1", start) == 0, "failures count per address");
    assert_eq!(failures.recent(tailnet, at(62)), 2, "a failure 60 s old no longer counts");
    assert!(failures.recent(tailnet, at(70)) == 0 && failures.peers() == 0, "an address with no recent failure is forgotten");
    for i in 0..256 {
        failures.record(&format!("100.65.{}.{}", i / 256, i % 256), start);
    }
    failures.record("100.66.0.1", at(61));
    assert_eq!(failures.peers(), 1, "a full table drops the addresses whose failures are all old");

    // The status line says what the Mac's says.
    let locale = ko();
    let detail = |status: MobileDirectStatus| strings::status_detail(&locale, &status);
    assert_eq!(detail(MobileDirectStatus::off()), "", "off says nothing");
    assert_eq!(detail(MobileDirectStatus::new(DirectState::Disabled, vec![], None)), locale.get("mobileRemote.direct.status.off"));
    assert_eq!(detail(MobileDirectStatus::new(DirectState::NotFound, vec![], None)), locale.get("mobileRemote.direct.status.notFound"));
    assert_eq!(
        detail(MobileDirectStatus::new(
            DirectState::Listening,
            vec!["100.101.102.103:43139".into(), "[fd7a:115c:a1e0::1]:43139".into()],
            None
        )),
        locale
            .get_with("mobileRemote.direct.status.listeningTemplate", &[("addresses", "100.101.102.103:43139, [fd7a:115c:a1e0::1]:43139")]),
        "the endpoints listened on"
    );
    let failed = |key: &str| locale.get_with("mobileRemote.direct.status.failedTemplate", &[("error", &locale.get(key))]);
    let failing = |code: Option<&str>| detail(MobileDirectStatus::new(DirectState::Failed, vec![], code));
    assert_eq!(failing(Some(status::ADDRESS_IN_USE)), failed("mobileRemote.direct.windows.error.addressInUse"));
    assert_eq!(failing(Some(status::ACCESS_DENIED)), failed("mobileRemote.direct.windows.error.accessDenied"));
    assert_eq!(failing(Some(status::BIND_FAILED)), failed("mobileRemote.direct.windows.error.bindFailed"));
    assert_eq!(failing(Some(status::ADAPTERS_UNREADABLE)), failed("mobileRemote.direct.windows.error.adaptersUnreadable"));
    // Raw operating-system text, or no reason at all, never reaches the line: it says the general bind failure instead.
    for raw in [
        Some("Only one usage of each socket address is normally permitted."),
        Some("Der Zugriff wurde verweigert"),
        Some("in use"),
        Some(""),
        None,
    ] {
        assert_eq!(failing(raw), failed("mobileRemote.direct.windows.error.bindFailed"), "raw text is never shown: {raw:?}");
    }
    // A listener that could not start is named by its socket error, never by the socket's words.
    let io = |kind: std::io::ErrorKind| PlatformError::Io(std::io::Error::from(kind));
    assert!(
        status::error_of(&io(std::io::ErrorKind::AddrInUse)) == status::ADDRESS_IN_USE
            && status::error_of(&io(std::io::ErrorKind::PermissionDenied)) == status::ACCESS_DENIED
            && status::error_of(&io(std::io::ErrorKind::AddrNotAvailable)) == status::BIND_FAILED
            && status::error_of(&io(std::io::ErrorKind::NetworkDown)) == status::BIND_FAILED
            && status::error_of(&PlatformError::Refused("no".into())) == status::BIND_FAILED,
        "socket errors map to codes"
    );
    assert!(MobileDirectStatus::new(DirectState::Listening, vec!["x".into()], None).listening() && !MobileDirectStatus::off().listening());
    assert_eq!(MobileDirectStatus::off().to_json(), r#"{"state":"off","endpoints":[]}"#, "the computed property is never serialized");
    assert_eq!(
        MobileDirectStatus::new(DirectState::Failed, vec![], Some(status::ADDRESS_IN_USE)).to_json(),
        r#"{"state":"failed","endpoints":[],"error":"addressInUse"}"#
    );
}

type Delivered = Arc<Mutex<VecDeque<DirectIncoming>>>;

fn collecting(network: &FakeNetwork) -> (DirectListener, Delivered) {
    let delivered: Delivered = Arc::default();
    let into = delivered.clone();
    let listener = DirectListener::new(
        Arc::new(network.clone()),
        tokio::runtime::Handle::current(),
        Arc::new(move |incoming| into.lock().unwrap().push_back(incoming)),
        log(),
    );
    (listener, delivered)
}

async fn next(delivered: &Delivered) -> DirectIncoming {
    assert!(until(|| !delivered.lock().unwrap().is_empty()).await, "a connection was delivered");
    delivered.lock().unwrap().pop_front().unwrap()
}

/// "mobile direct: the listener holds at most 36 and upgrades a phone" (L490).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mobile_direct_the_listener_holds_at_most_36_and_upgrades_a_phone() {
    let network = FakeNetwork::default();
    let (listener, delivered) = collecting(&network);
    let (bound, error) = listener.update(&["127.0.0.1".into()], 0);
    assert!(error.is_none() && bound.len() == 1 && bound[0].0 == "127.0.0.1" && bound[0].1 > 0, "one listener on the address asked for");
    let port = bound[0].1;
    assert_eq!(network.binds().last().map(|(_, backlog)| *backlog), Some(36), "the backlog is the connection limit");
    assert_eq!(listener.update(&["127.0.0.1".into()], 0).0[0].1, port, "an address already listened on keeps its listener");
    let endpoint = format!("127.0.0.1:{port}");

    // A phone's upgrade, then frames both ways.
    {
        let connecting = tokio::spawn({
            let endpoint = endpoint.clone();
            async move { phone(&endpoint).await }
        });
        let incoming = next(&delivered).await;
        assert_eq!(incoming.peer().as_deref(), Some("127.0.0.1"), "the peer is read from the socket");
        let (stream, slot) = incoming.into_parts();
        let mut server = accept_web_socket(stream).await.expect("the phone's upgrade was accepted");
        let mut client = connecting.await.unwrap().expect("the phone connected");
        client.send(Message::text("hello")).await.unwrap();
        assert!(matches!(server.next().await, Some(Ok(Message::Text(t))) if t.as_str() == "hello"), "frames pass after the upgrade");
        server.send(Message::text("ready")).await.unwrap();
        assert!(matches!(client.next().await, Some(Ok(Message::Text(t))) if t.as_str() == "ready"), "frames pass back to the phone");
        drop(server);
        drop(slot);
    }
    assert!(until(|| listener.live_connections() == 0).await, "a connection that ended gives its slot back once");

    // A request that is no phone's upgrade gets no 101: it is closed unanswered and its slot comes back.
    {
        let mut stranger = TcpStream::connect(&endpoint).await.unwrap();
        stranger.write_all(b"GET /other HTTP/1.1\r\nHost: x\r\n\r\n").await.unwrap();
        let incoming = next(&delivered).await;
        let (stream, slot) = incoming.into_parts();
        assert!(accept_web_socket(stream).await.is_none(), "a refused upgrade gives no socket");
        drop(slot);
        assert!(reads_end(&mut stranger, 3000).await, "a refused upgrade is answered with nothing");
        assert!(until(|| listener.live_connections() == 0).await);
    }

    // A phone that hangs up before anything starts on its connection still gives its slot back once.
    {
        let early = TcpStream::connect(&endpoint).await.unwrap();
        let incoming = next(&delivered).await;
        drop(early);
        let (stream, slot) = incoming.into_parts();
        assert!(accept_web_socket(stream).await.is_none(), "a closed connection gives no socket");
        drop(slot);
        assert!(until(|| listener.live_connections() == 0).await);
    }

    // 36 held at once; the 37th is reset unread and gives its slot straight back.
    {
        let mut held = vec![];
        for _ in 0..CONNECTION_LIMIT {
            held.push(TcpStream::connect(&endpoint).await.unwrap());
        }
        assert!(until(|| listener.live_connections() == CONNECTION_LIMIT).await, "36 held");
        assert!(refused(&endpoint, 3000).await, "the 37th connection is dropped");
        assert_eq!(listener.live_connections(), CONNECTION_LIMIT, "a dropped connection gives its slot back");
        assert_eq!(delivered.lock().unwrap().len(), CONNECTION_LIMIT, "the 37th connection is never delivered");
        drop(held);
        delivered.lock().unwrap().clear();
    }
    assert!(until(|| listener.live_connections() == 0).await);

    // A handler that panics has its connection dropped, and its slot comes back.
    {
        let throwing = DirectListener::new(
            Arc::new(network.clone()),
            tokio::runtime::Handle::current(),
            Arc::new(|_incoming: DirectIncoming| panic!("handler")),
            log(),
        );
        let throwing_port = throwing.update(&["127.0.0.1".into()], 0).0[0].1;
        assert!(refused(("127.0.0.1", throwing_port), 3000).await, "a handler's failure drops the connection");
        assert!(until(|| throwing.live_connections() == 0).await);
        throwing.stop();
    }

    // A port already taken is reported, not thrown; an address that is no IP is the general bind failure.
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let busy = taken.local_addr().unwrap().port();
    let (second, _) = collecting(&network);
    let (bound, error) = second.update(&["127.0.0.1".into()], busy);
    second.stop();
    assert!(
        bound.is_empty() && matches!(error, Some(status::ADDRESS_IN_USE) | Some(status::ACCESS_DENIED)),
        "a taken port is a bind error, as a code and never the socket's words: {error:?}"
    );
    let (unparsable, _) = collecting(&network);
    assert_eq!(unparsable.update(&["not an address".into()], 0), (vec![], Some(status::BIND_FAILED)), "no IP: the general failure");
    // The fake binds loopback alone: a real interface or a wildcard is refused before anything listens.
    let (wildcard, _) = collecting(&network);
    assert!(wildcard.update(&["0.0.0.0".into()], 0).0.is_empty(), "no wildcard");

    // Stop closes every listener.
    listener.stop();
    assert!(listener.update(&["127.0.0.1".into()], 0).0.is_empty(), "a stopped listener binds nothing again");
    tokio::time::sleep(Duration::from_millis(100)).await;
    let refused = match TcpStream::connect(&endpoint).await {
        Ok(mut after) => reads_end(&mut after, 3000).await,
        Err(_) => true,
    };
    assert!(refused, "a stopped listener takes no phone");
}

/// Reads the given bytes, then the end; records what is written (C#'s `ReadOnlyThenRecording`).
struct ReadOnlyThenRecording {
    input: std::io::Cursor<Vec<u8>>,
    written: Arc<Mutex<Vec<u8>>>,
}

impl AsyncRead for ReadOnlyThenRecording {
    fn poll_read(mut self: Pin<&mut Self>, _cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        let at = self.input.position() as usize;
        let rest = &self.input.get_ref()[at..];
        let take = rest.len().min(buf.remaining());
        buf.put_slice(&rest[..take]);
        self.input.set_position((at + take) as u64);
        Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for ReadOnlyThenRecording {
    fn poll_write(self: Pin<&mut Self>, _cx: &mut Context<'_>, data: &[u8]) -> Poll<std::io::Result<usize>> {
        self.written.lock().unwrap().extend_from_slice(data);
        Poll::Ready(Ok(data.len()))
    }
    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

fn recording(bytes: Vec<u8>) -> (ReadOnlyThenRecording, Arc<Mutex<Vec<u8>>>) {
    let written = Arc::new(Mutex::new(vec![]));
    (ReadOnlyThenRecording { input: std::io::Cursor::new(bytes), written: written.clone() }, written)
}

/// "mobile direct: the upgrade and the peer are read strictly" (L491).
#[tokio::test]
async fn mobile_direct_the_upgrade_and_the_peer_are_read_strictly() {
    // Nothing is written back to a refused request.
    let long = format!("GET /ws?v=1 HTTP/1.1\r\nX: {}\r\n\r\n", "a".repeat(MAXIMUM_HEAD_BYTES));
    for (bytes, reason) in [
        (REQUEST.replace("GET /ws?v=1", "GET /other").into_bytes(), "another path"),
        (format!("{REQUEST}x").into_bytes(), "bytes after the blank line"),
        (REQUEST.as_bytes()[..REQUEST.len() - 2].to_vec(), "a head that never ends"),
        (long.into_bytes(), "a head over 8 KiB"),
        (vec![], "nothing at all"),
    ] {
        let (stream, written) = recording(bytes);
        assert!(accept_web_socket(stream).await.is_none() && written.lock().unwrap().is_empty(), "a refused upgrade: {reason}");
    }
    let (good, written) = recording(REQUEST.as_bytes().to_vec());
    assert!(accept_web_socket(good).await.is_some(), "a good upgrade gives a socket");
    assert!(
        String::from_utf8_lossy(&written.lock().unwrap()).contains("Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n\r\n"),
        "a good upgrade is answered with a 101"
    );

    // One spelling per peer, so the per-address limits never split across forms of one address.
    let peer = |address: &str| tailscale::canonical(tailscale::peer(address.parse().unwrap()));
    assert_eq!(peer("::ffff:100.101.102.103"), "100.101.102.103", "an IPv4-mapped peer is its IPv4");
    assert_eq!(peer("FD7A:115C:A1E0::1"), "fd7a:115c:a1e0::1", "an IPv6 peer is lowercase");
    let zoned = SocketAddrV6::new("fd7a:115c:a1e0:0:0:0:0:1".parse().unwrap(), 1, 0, 7);
    assert_eq!(tailscale::canonical(tailscale::peer(IpAddr::V6(*zoned.ip()))), "fd7a:115c:a1e0::1", "no zone, compressed");
    assert_eq!(peer("100.101.102.103"), "100.101.102.103", "an IPv4 peer stays");
}

fn path(network: &FakeNetwork, options: DirectPathOptions) -> DirectPath {
    DirectPath::new(Arc::new(network.clone()), tokio::runtime::Handle::current(), options, log())
}

/// "mobile direct: only who may gets in, and relay redials leave them alone" (L493), the direct path's half: stalled handshakes from one
/// address hold their two slots and a third is reset at once, the deadline cuts them; a paired phone stays through a re-read that
/// finds the same address; the watch re-reads on its own (none found, unreadable, back), publishing only a pass that changed the
/// status; the switch off ends the direct phones; five refused auths shut an address out. (The relay redial is W8-A's half.)
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mobile_direct_only_who_may_gets_in_and_relay_redials_leave_them_alone() {
    let network = FakeNetwork::default();
    let addresses = Arc::new(Mutex::new(Some(vec!["127.0.0.1".to_string()])));
    let options = DirectPathOptions {
        handshake_deadline: Duration::from_secs(2),
        redetect_interval: Duration::from_millis(200),
        ..DirectPathOptions::default()
    };
    let path = path(&network, options);
    let host = Host::default();
    path.set_sink(Some(Arc::new(host.clone())));
    let published = Arc::new(AtomicUsize::new(0));
    path.on_published({
        let published = published.clone();
        move || {
            published.fetch_add(1, Ordering::SeqCst);
        }
    });
    path.set_address_source(Some(source(&addresses)), 0).await;
    assert_eq!(path.status(), MobileDirectStatus::off(), "nothing listens while the remote is off");
    path.apply(on()).await;
    let mut endpoint = path.endpoints().pop().expect("an endpoint");

    // Two stalled handshakes from one address hold its two slots; a third is reset unread at once, long before the 2 s deadline and
    // while the first two are still held; the deadline then frees them.
    {
        let mut first = TcpStream::connect(&endpoint).await.unwrap();
        let mut second = TcpStream::connect(&endpoint).await.unwrap();
        assert!(until(|| host.connections() == 2).await, "two admitted");
        let started = Instant::now();
        assert!(refused(&endpoint, 500).await, "a third handshake from the same address is dropped at once, not at the deadline");
        assert!(started.elapsed() < Duration::from_millis(1500) && host.connections() == 2, "the first two are still held");
        assert!(reads_end(&mut first, 5000).await && reads_end(&mut second, 5000).await, "a stalled handshake is cut at the deadline");
    }
    assert!(until(|| host.connections() == 0 && path.live_connections() == 0).await, "their slots came back");

    // A paired phone stays through a re-read that finds the same address; the watch notices it go away.
    {
        let mut phone = phone(&endpoint).await.unwrap();
        assert_eq!(exchange(&mut phone, "auth-ok").await.as_deref(), Some("ok"), "the phone is in");
        path.refresh(false).await;
        assert_eq!(exchange(&mut phone, "after-refresh").await.as_deref(), Some("after-refresh"), "a re-read finding the same address");
        *addresses.lock().unwrap() = Some(vec![]);
        assert!(until(|| path.status().state == DirectState::NotFound).await, "the watch re-reads the addresses on its own");
        assert!(path.endpoints().is_empty());
        assert_eq!(strings::status_detail(&ko(), &path.status()), ko().get("mobileRemote.direct.status.notFound"));
        assert_eq!(exchange(&mut phone, "still").await.as_deref(), Some("still"), "the phone already in stays");
    }
    // Adapters that cannot be read: the watch says so and keeps going, and listens again once they can be.
    *addresses.lock().unwrap() = None;
    assert!(until(|| path.status().state == DirectState::Failed).await);
    assert!(path.status().error.as_deref() == Some(status::ADAPTERS_UNREADABLE) && path.endpoints().is_empty(), "reported, not thrown");
    let words = strings::status_detail(&ko(), &path.status());
    assert!(
        words
            == ko().get_with(
                "mobileRemote.direct.status.failedTemplate",
                &[("error", &ko().get("mobileRemote.direct.windows.error.adaptersUnreadable"))]
            )
            && !words.contains("adapters unreadable"),
        "it says so in words, never the fault's own"
    );
    // Only a pass that changes the status publishes: several ticks that fail the same way publish nothing.
    let (before, passes) = (published.load(Ordering::SeqCst), path.passes());
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert!(path.passes() > passes && published.load(Ordering::SeqCst) == before, "a pass that ends where it began publishes nothing");
    *addresses.lock().unwrap() = Some(vec!["127.0.0.1".into()]);
    assert!(until(|| path.status().listening()).await, "listening again");
    endpoint = path.endpoints().pop().unwrap();

    // The direct switch off ends the direct phones and says so.
    {
        let mut phone = phone(&endpoint).await.unwrap();
        assert_eq!(exchange(&mut phone, "auth-ok").await.as_deref(), Some("ok"), "a second phone is in");
        path.apply(DirectSettings { direct_enabled: false, ..on() }).await;
        assert!(path.status().state == DirectState::Disabled && path.endpoints().is_empty());
        assert_eq!(strings::status_detail(&ko(), &path.status()), ko().get("mobileRemote.direct.status.off"), "the switch off says so");
        assert!(ended(&mut phone).await, "the direct phone was ended");
        assert!(until(|| host.connections() == 0).await, "and left the table");
    }

    // Five refused keys from one address within a minute: its sixth socket is reset unread.
    path.apply(on()).await;
    endpoint = path.endpoints().pop().unwrap();
    for _ in 0..admit::MAXIMUM_FAILURES {
        let mut stranger = phone(&endpoint).await.unwrap();
        assert_eq!(exchange(&mut stranger, "auth-bad").await.as_deref(), Some(wire::GENERIC_REFUSAL), "every refusal is the same word");
        drop(stranger);
        assert!(until(|| host.connections() == 0).await);
    }
    assert!(phone(&endpoint).await.is_err(), "an address that keeps failing is shut out");

    // The remote off: off, nothing listens; the path disposed publishes nothing more.
    path.apply(DirectSettings { enabled: false, ..on() }).await;
    assert_eq!(path.status(), MobileDirectStatus::off());
    path.dispose();
    assert!(path.endpoints().is_empty());
}

/// "mobile direct: the host advertises its endpoints" (L494), the direct path's half: nothing while nothing listens, the bound
/// address with its port once listening, Tailscale's addresses only (loopback through the seam alone), the fake network asked for
/// exactly those addresses with the backlog. The QR's `&direct=`, auth_ok's `direct[]` and `/m1/info` read these (W8-A, W8-B).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mobile_direct_the_host_advertises_its_endpoints() {
    let network = FakeNetwork::default();
    let path = path(&network, DirectPathOptions::default());
    path.set_sink(Some(Arc::new(Host::default())));
    assert!(path.endpoints().is_empty(), "nothing advertised while nothing listens");
    let addresses = Arc::new(Mutex::new(Some(vec!["127.0.0.1".to_string()])));
    path.set_address_source(Some(source(&addresses)), 0).await;
    path.apply(on()).await;
    let endpoints = path.endpoints();
    assert!(endpoints.len() == 1 && endpoints[0].starts_with("127.0.0.1:"), "the seam's address is listened on and advertised");
    assert_eq!(path.status(), MobileDirectStatus::new(DirectState::Listening, endpoints.clone(), None));
    assert!(network.binds().iter().all(|(address, backlog)| address.ip().is_loopback() && *backlog == 36), "only what was asked for");
    // Without the seam the path reads the adapters: only Tailscale's are offered (the fake refuses to bind them: a code, no endpoint).
    path.set_address_source(None, 0).await;
    network.set_adapters(mighty_platform::Adapters {
        adapters: vec![mighty_platform::Adapter {
            name: "Tailscale".into(),
            addresses: vec!["100.101.102.103".parse().unwrap(), "fd7a:115c:a1e0::1".parse().unwrap(), "192.168.0.5".parse().unwrap()],
        }],
        unreadable: 0,
    });
    path.refresh(true).await;
    let asked: Vec<IpAddr> = network.binds().iter().rev().take(2).map(|(address, _)| address.ip()).collect();
    assert!(
        asked.contains(&"100.101.102.103".parse().unwrap()) && asked.contains(&"fd7a:115c:a1e0::1".parse().unwrap()),
        "the Tailscale addresses are asked for: {asked:?}"
    );
    assert!(!network.binds().iter().any(|(address, _)| address.ip() == "192.168.0.5".parse::<IpAddr>().unwrap()), "never the LAN");
    assert!(
        network.binds().iter().all(|(address, _)| address.port() == wire::DEFAULT_PORT || address.port() == 0),
        "the saved port without the seam"
    );
    assert!(path.status().state == DirectState::Failed && path.endpoints().is_empty(), "a refused bind advertises nothing");
    path.dispose();
}

/// "mobile direct: the Windows words are in every language" (L498).
#[test]
fn mobile_direct_the_windows_words_are_in_every_language() {
    for language in LANGUAGES {
        let words = catalogue(language);
        for key in strings::KEYS {
            assert!(words.get(key).is_some_and(|text| !text.trim().is_empty()), "{language} lacks {key}");
        }
        assert!(
            words["mobileRemote.direct.windows.firewall.failedTemplate"].contains("{error}"),
            "{language}: the failure names its error"
        );
        assert!(!words["mobileRemote.direct.windows.description"].contains("Mac"), "{language}: the Windows description names no Mac");
        assert!(words.get("setup.button.recheck").is_some_and(|t| !t.trim().is_empty()), "{language} has the check-again words");
    }
    assert!(strings::KEYS.len() == 21 && strings::KEYS.iter().all(|key| key.starts_with("mobileRemote.direct.windows.")));
    // Why the path could not listen: each code its own words, in every language, the four distinct.
    let errors = [
        (status::ADDRESS_IN_USE, "mobileRemote.direct.windows.error.addressInUse"),
        (status::ACCESS_DENIED, "mobileRemote.direct.windows.error.accessDenied"),
        (status::BIND_FAILED, "mobileRemote.direct.windows.error.bindFailed"),
        (status::ADAPTERS_UNREADABLE, "mobileRemote.direct.windows.error.adaptersUnreadable"),
    ];
    for language in LANGUAGES {
        let locale = Locale::new(language);
        for (code, key) in errors {
            assert!(strings::KEYS.contains(&key) && strings::error(&locale, Some(code)) == catalogue(language)[key], "{language}: {code}");
        }
        let distinct: std::collections::HashSet<String> = errors.iter().map(|(code, _)| strings::error(&locale, Some(code))).collect();
        assert_eq!(distinct.len(), errors.len(), "{language}: each reason says its own words");
    }
    let locale = ko();
    let present = FirewallRule::Present { ports: None, program: None };
    assert!(
        strings::firewall_state(&locale, &present) == locale.get("mobileRemote.direct.windows.firewall.present")
            && strings::firewall_state(&locale, &FirewallRule::Missing) == locale.get("mobileRemote.direct.windows.firewall.absent")
            && strings::firewall_state(&locale, &FirewallRule::Unreadable) == locale.get("mobileRemote.direct.windows.firewall.unknown"),
        "the rule's three states"
    );
    assert!(
        strings::firewall_button(&locale, &present) == locale.get("mobileRemote.direct.windows.firewall.removeButton")
            && strings::firewall_button(&locale, &FirewallRule::Missing) == locale.get("mobileRemote.direct.windows.firewall.addButton"),
        "the button adds when absent and removes when present"
    );
    let outcome = |change: FirewallChange| strings::firewall_outcome(&locale, &change);
    let refused = |detail: &str| FirewallChange::Refused(detail.into());
    let failed = |detail: &str| FirewallChange::Failed(detail.into());
    assert!(
        outcome(FirewallChange::Applied(None)).is_none()
            && outcome(FirewallChange::Declined) == Some(locale.get("mobileRemote.direct.windows.firewall.cancelled"))
            && outcome(refused("noPorts")) == Some(locale.get("mobileRemote.direct.windows.firewall.noPorts"))
            && outcome(refused("badProgram")) == Some(locale.get("mobileRemote.direct.windows.firewall.badProgram"))
            && outcome(failed("1")) == Some(locale.get_with("mobileRemote.direct.windows.firewall.failedTemplate", &[("error", "1")])),
        "every outcome has its words"
    );
    // Every detail code the runner can return has its own words; no raw code or OS text reaches the screen.
    let refusals = [
        ("noPorts", "mobileRemote.direct.windows.firewall.noPorts"),
        ("badPorts", "mobileRemote.direct.windows.firewall.badPorts"),
        ("badProgram", "mobileRemote.direct.windows.firewall.badProgram"),
        ("noSystemDirectory", "mobileRemote.direct.windows.firewall.noSystemDirectory"),
    ];
    for (code, key) in refusals {
        assert_eq!(outcome(refused(code)), Some(locale.get(key)), "refused {code} has its words");
    }
    assert_eq!(outcome(failed("timeout")), Some(locale.get("mobileRemote.direct.windows.firewall.timeout")));
    assert_eq!(outcome(failed("queryFailed")), Some(locale.get("mobileRemote.direct.windows.firewall.queryFailed")));
    let unknown = locale.get("mobileRemote.direct.windows.firewall.failedUnknown");
    for odd in
        ["Der Zugriff wurde verweigert", "The system cannot find the file specified.", "timeout ", "-", "-1", "+1", "1 ", "2147483648", ""]
    {
        assert_eq!(outcome(failed(odd)), Some(unknown.clone()), "failed {odd:?} says the unknown words");
        assert_eq!(outcome(refused(odd)), Some(unknown.clone()), "refused {odd:?} says the unknown words");
    }
    // The same mapping in every language.
    for language in LANGUAGES {
        let (locale, words) = (Locale::new(language), catalogue(language));
        let outcome = |change: FirewallChange| strings::firewall_outcome(&locale, &change);
        assert_eq!(outcome(FirewallChange::Declined).as_deref(), Some(words["mobileRemote.direct.windows.firewall.cancelled"].as_str()));
        assert_eq!(outcome(refused("badPorts")).as_deref(), Some(words["mobileRemote.direct.windows.firewall.badPorts"].as_str()));
        assert_eq!(outcome(failed("timeout")).as_deref(), Some(words["mobileRemote.direct.windows.firewall.timeout"].as_str()));
        assert_eq!(outcome(failed("5")), Some(words["mobileRemote.direct.windows.firewall.failedTemplate"].replace("{error}", "5")));
        assert_eq!(outcome(failed("-1")).as_deref(), Some(words["mobileRemote.direct.windows.firewall.failedUnknown"].as_str()));
        assert_eq!(outcome(failed("2147483648")).as_deref(), Some(words["mobileRemote.direct.windows.firewall.failedUnknown"].as_str()));
    }
    let mut all: Vec<String> = refusals.iter().map(|(code, _)| outcome(refused(code)).unwrap()).collect();
    all.extend([outcome(failed("timeout")).unwrap(), outcome(failed("queryFailed")).unwrap(), unknown]);
    let distinct: std::collections::HashSet<&String> = all.iter().collect();
    assert!(distinct.len() == all.len() && all.iter().all(|t| !t.is_empty()), "each outcome says its own words");
}

/// "mobile direct: a direct frame over 1 MiB is refused" (L501), the direct socket's half: a frame of exactly 1 MiB is read and the
/// connection goes on; one byte more ends it, read no further than the limit. (The cipher's 28 bytes and the relay host's reply after
/// the full frame are W8-A's half.)
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mobile_direct_a_direct_frame_over_1_mib_is_refused() {
    let network = FakeNetwork::default();
    let (listener, delivered) = collecting(&network);
    let port = listener.update(&["127.0.0.1".into()], 0).0[0].1;
    let endpoint = format!("127.0.0.1:{port}");
    let connecting = tokio::spawn(async move { phone(&endpoint).await });
    let (stream, _slot) = next(&delivered).await.into_parts();
    let mut server = accept_web_socket(stream).await.unwrap();
    let mut client = connecting.await.unwrap().unwrap();
    client.send(Message::binary(vec![b'a'; wire::MAXIMUM_FRAME_BYTES])).await.unwrap();
    assert!(
        matches!(server.next().await, Some(Ok(Message::Binary(b))) if b.len() == wire::MAXIMUM_FRAME_BYTES),
        "a frame of exactly 1 MiB is read"
    );
    client.send(Message::text("after-full-frame")).await.unwrap();
    assert!(matches!(server.next().await, Some(Ok(Message::Text(t))) if t.as_str() == "after-full-frame"), "the connection goes on");
    let _ = client.send(Message::binary(vec![0u8; wire::MAXIMUM_FRAME_BYTES + 1])).await;
    assert!(matches!(server.next().await, Some(Err(_)) | None), "a frame one byte over 1 MiB ends the connection");
    listener.stop();
}

/// "mobile direct: Settings read the adapters off the caller and outside the lock" (L503): applying reads the adapters on a thread of
/// their own, never the caller's; while a slow read waits, the caller's thread is free and a phone still gets in (the accepts never
/// wait on the read); the read then binds as before, keeping the listener the phone came in on.
#[tokio::test(flavor = "current_thread")]
async fn mobile_direct_settings_read_the_adapters_off_the_caller_and_outside_the_lock() {
    let caller = std::thread::current().id();
    let read_on = Arc::new(Mutex::new(vec![]));
    let hold = Arc::new(AtomicBool::new(false));
    let held = Arc::new(AtomicUsize::new(0));
    let gate = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
    let source: AddressSource = {
        let (read_on, hold, held, gate) = (read_on.clone(), hold.clone(), held.clone(), gate.clone());
        Arc::new(move || {
            read_on.lock().unwrap().push(std::thread::current().id());
            if hold.load(Ordering::SeqCst) {
                held.fetch_add(1, Ordering::SeqCst);
                let (open, opened) = &*gate;
                let guard = open.lock().unwrap();
                let _ = opened.wait_timeout_while(guard, Duration::from_secs(10), |open| !*open).unwrap();
            }
            Ok(vec!["127.0.0.1".to_string()])
        })
    };
    let network = FakeNetwork::default();
    let path = path(&network, DirectPathOptions::default());
    path.set_sink(Some(Arc::new(Host::default())));
    path.set_address_source(Some(source), 0).await;
    path.apply(on()).await;
    let endpoint = path.endpoints().pop().expect("listening");
    assert!(!read_on.lock().unwrap().is_empty() && !read_on.lock().unwrap().contains(&caller), "the read runs off the caller's thread");
    hold.store(true, Ordering::SeqCst);
    let applying = tokio::spawn({
        let path = path.clone();
        async move { path.apply(DirectSettings { port: wire::DEFAULT_PORT + 1, ..on() }).await }
    });
    assert!(until(|| held.load(Ordering::SeqCst) > 0).await, "the read is under way");
    // The caller's thread is free while the read waits: a phone gets in on it now.
    let mut phone = phone(&endpoint).await.expect("a phone connects while the read is under way");
    assert_eq!(exchange(&mut phone, "auth-ok").await.as_deref(), Some("ok"), "and is admitted: the lock is not held by the read");
    assert!(!applying.is_finished(), "the read was still under way");
    hold.store(false, Ordering::SeqCst);
    {
        let (open, opened) = &*gate;
        *open.lock().unwrap() = true;
        opened.notify_all();
    }
    applying.await.unwrap();
    assert!(!read_on.lock().unwrap().contains(&caller), "nor is any later read on the caller's thread");
    assert!(path.status().listening() && path.endpoints() == vec![endpoint], "the same listener is kept");
    assert_eq!(exchange(&mut phone, "after-apply").await.as_deref(), Some("after-apply"), "and the phone on it stays");
    path.dispose();
}

/// Review M1: one deadline from the accept to auth_ok (C#'s 10 s, here 2 s), shared by the upgrade and the sink's hello and auth: a phone
/// that takes most of it for its upgrade and then stalls is cut when the deadline taken at the accept runs out, not a fresh one after
/// the upgrade; a phone that never upgrades gives its reservation back by its own id; every reservation is handed over exactly once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mobile_direct_one_deadline_covers_the_upgrade_and_the_auth() {
    let network = FakeNetwork::default();
    let options = DirectPathOptions { handshake_deadline: Duration::from_secs(2), ..DirectPathOptions::default() };
    let path = path(&network, options);
    let host = Host::default();
    path.set_sink(Some(Arc::new(host.clone())));
    let addresses = Arc::new(Mutex::new(Some(vec!["127.0.0.1".to_string()])));
    path.set_address_source(Some(source(&addresses)), 0).await;
    path.apply(on()).await;
    let endpoint = path.endpoints().pop().expect("listening");

    let connected = Instant::now();
    let mut slow = TcpStream::connect(&endpoint).await.unwrap();
    tokio::time::sleep(Duration::from_millis(1200)).await;
    slow.write_all(REQUEST.as_bytes()).await.unwrap();
    let mut answer = [0u8; 256];
    let read = slow.read(&mut answer).await.unwrap();
    assert!(String::from_utf8_lossy(&answer[..read]).starts_with("HTTP/1.1 101"), "upgraded late in its deadline");
    // Now it stalls: no hello, no auth.
    assert!(reads_end(&mut slow, 5000).await, "the stalled phone is cut");
    let lasted = connected.elapsed();
    assert!(
        lasted >= Duration::from_millis(1700) && lasted < Duration::from_millis(2900),
        "cut at the deadline taken at the accept (2 s), not 2 s after its upgrade: {lasted:?}"
    );

    let mut silent = TcpStream::connect(&endpoint).await.unwrap();
    assert!(reads_end(&mut silent, 5000).await, "a phone that never upgrades is cut at the deadline");
    assert!(until(|| host.connections() == 0).await);
    let table = host.table();
    let mut handed: Vec<u64> = table.accepted.iter().chain(table.refused.iter()).copied().collect();
    handed.sort_unstable();
    assert!(table.admitted.len() == 2 && handed == table.admitted, "each reservation handed over once, by its own id");
    assert!(table.accepted.len() == 1 && table.refused.len() == 1 && table.accepted[0] != table.refused[0]);
    drop(table);
    path.dispose();
}

/// Review M2: without a relay host to run what it admits the path never listens — no bind, no endpoint, status off, even with both
/// switches on; a sink then makes it listen, and taking it away stops it and ends its phones.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mobile_direct_the_path_listens_only_with_a_sink() {
    let network = FakeNetwork::default();
    let path = path(&network, DirectPathOptions::default());
    let addresses = Arc::new(Mutex::new(Some(vec!["127.0.0.1".to_string()])));
    path.set_address_source(Some(source(&addresses)), 0).await;
    path.apply(on()).await;
    assert!(
        path.status() == MobileDirectStatus::off() && path.endpoints().is_empty() && network.binds().is_empty(),
        "no sink, no listener"
    );
    path.apply(DirectSettings { direct_enabled: false, ..on() }).await;
    assert_eq!(path.status(), MobileDirectStatus::off(), "nor a switch state of its own");
    path.apply(on()).await;
    let host = Host::default();
    path.set_sink(Some(Arc::new(host.clone())));
    path.refresh(true).await;
    let endpoint = path.endpoints().pop().expect("a sink makes it listen");
    assert!(path.status().listening());
    let mut phone = phone(&endpoint).await.unwrap();
    assert_eq!(exchange(&mut phone, "auth-ok").await.as_deref(), Some("ok"));
    path.set_sink(None);
    path.refresh(true).await;
    assert!(path.status() == MobileDirectStatus::off() && path.endpoints().is_empty(), "the sink gone: off");
    assert!(ended(&mut phone).await, "its phones end");
    assert!(TcpStream::connect(&endpoint).await.is_err() || until(|| host.connections() == 0).await);
    // The seam takes loopback alone: anything else it names is dropped before a bind is asked for.
    path.set_sink(Some(Arc::new(Host::default())));
    *addresses.lock().unwrap() = Some(vec!["100.101.102.103".into(), "0.0.0.0".into()]);
    let binds = network.binds().len();
    path.refresh(true).await;
    assert!(network.binds().len() == binds && path.status().state == DirectState::NotFound, "no non-loopback bind through the seam");
    path.dispose();
}

/// Review L1: settings are kept by the path when they are stored, so passes that finish in any order apply the last one stored.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mobile_direct_settings_land_in_the_order_they_were_stored() {
    let network = FakeNetwork::default();
    let path = path(&network, DirectPathOptions::default());
    path.set_sink(Some(Arc::new(Host::default())));
    let addresses = Arc::new(Mutex::new(Some(vec!["127.0.0.1".to_string()])));
    path.set_address_source(Some(source(&addresses)), 0).await;
    for round in 0..20 {
        let first = path.store_settings(on());
        let one = tokio::spawn({
            let path = path.clone();
            async move { path.refresh(first).await }
        });
        let second = path.store_settings(DirectSettings { direct_enabled: false, ..on() });
        let two = tokio::spawn({
            let path = path.clone();
            async move { path.refresh(second).await }
        });
        let _ = tokio::join!(two, one);
        assert_eq!(path.status().state, DirectState::Disabled, "round {round}: the last stored settings won");
        assert!(path.endpoints().is_empty());
    }
    path.dispose();
}
