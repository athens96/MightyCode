//! C# RemoteHostVerification (Verification.cs L887-L901): "이 컴퓨터 공유" — the strict listener, the host's status order, the key, the
//! rate limits, the start and stop races and the run count — over loopback port 0 through mighty-platform's `FakeNetwork` (which binds
//! loopback only). The PowerShell runner's real commands run on Windows only.

mod support;

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mighty_platform::fake::FakeNetwork;
use mighty_remote::remote::command_shell;
use mighty_remote::remote::host::{self, RateLimiter, RemoteHost};
use mighty_remote::remote::http_server::{
    self, Failed, HttpListener, HttpRequest, HttpResponse, ListenerOptions, MAXIMUM_CONNECTIONS, counts_as_accept_failure,
};
use mighty_remote::remote::strings;
use mighty_remote::remote::transport::{self, Method, Request};
use mighty_remote::remote::validation;
use mighty_remote::remote::wire::{RemoteWireInfo, protocol};
use mighty_run::model::StartRunRequest;
use mighty_run::strings::{get, get_with, set_language};
use serde_json::json;
use support::fake_host::*;
use tokio::io::AsyncWriteExt;
use tokio::sync::Notify;

fn ok_json() -> HttpResponse {
    HttpResponse::json(200, &json!({}), &[])
}

fn listener(handler: http_server::Handler, options: ListenerOptions) -> HttpListener {
    HttpListener::new(Arc::new(FakeNetwork::default()), IpAddr::V4(Ipv4Addr::LOCALHOST), 0, handler, options)
}

fn answer_with(response: impl Fn(&HttpRequest) -> HttpResponse + Send + Sync + 'static) -> http_server::Handler {
    let response = Arc::new(response);
    Arc::new(move |request: HttpRequest| {
        let answer = response(&request);
        Box::pin(async move { Ok(answer) })
    })
}

/// "remote host: a refused peer is cut off at accept and one peer holds at most its share".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refused_peer_is_cut_off_at_accept_and_one_peer_holds_at_most_its_share() {
    let handled = Arc::new(AtomicUsize::new(0));
    let counter = handled.clone();
    let refused = listener(
        answer_with(move |_| {
            counter.fetch_add(1, Ordering::SeqCst);
            ok_json()
        }),
        ListenerOptions { accept_peer: Some(Arc::new(|_| Ok(false))), ..ListenerOptions::default() },
    );
    let port = refused.start().unwrap();
    assert!(raw_text(port, "GET /v2/info HTTP/1.1\r\n\r\n").await.is_empty() && handled.load(Ordering::SeqCst) == 0);
    refused.dispose().await;
    let asked = Arc::new(Mutex::new(vec![]));
    let record = asked.clone();
    let filtered = listener(
        answer_with(|_| ok_json()),
        ListenerOptions {
            accept_peer: Some(Arc::new(move |peer| {
                record.lock().unwrap().push(peer);
                Ok(true)
            })),
            ..ListenerOptions::default()
        },
    );
    let port = filtered.start().unwrap();
    assert!(raw_text(port, "GET /v2/info HTTP/1.1\r\n\r\n").await.starts_with("HTTP/1.1 200"), "an accepted peer is answered");
    assert_eq!(*asked.lock().unwrap(), [IpAddr::V4(Ipv4Addr::LOCALHOST)], "the filter is asked once with the peer's address");
    filtered.dispose().await;
    let scoped: std::net::SocketAddr = "[fe80::5%7]:1".parse().unwrap();
    assert_eq!(http_server::peer_address(scoped).to_string(), "fe80::5", "the filter sees an IPv6 peer without its scope");
    let mapped: std::net::SocketAddr = "[::ffff:100.101.102.103]:1".parse().unwrap();
    assert_eq!(http_server::peer_address(mapped).to_string(), "100.101.102.103", "and an IPv4-mapped peer as its IPv4");
    let release = Arc::new(Notify::new());
    let released = Arc::new(AtomicBool::new(false));
    let entered = Arc::new(AtomicUsize::new(0));
    let (wait, open_gate, count) = (release.clone(), released.clone(), entered.clone());
    let handler: http_server::Handler = Arc::new(move |_| {
        let (wait, open_gate, count) = (wait.clone(), open_gate.clone(), count.clone());
        Box::pin(async move {
            count.fetch_add(1, Ordering::SeqCst);
            while !open_gate.load(Ordering::SeqCst) {
                let _ = tokio::time::timeout(Duration::from_millis(50), wait.notified()).await;
            }
            Ok(HttpResponse::json(200, &json!({"ok": true}), &[]))
        })
    });
    let capped = listener(handler, ListenerOptions { maximum_connections_per_peer: Some(2), ..ListenerOptions::default() });
    let port = capped.start().unwrap();
    let held =
        [tokio::spawn(raw_text_owned(port, "GET /a HTTP/1.1\r\n\r\n")), tokio::spawn(raw_text_owned(port, "GET /b HTTP/1.1\r\n\r\n"))];
    until(|| entered.load(Ordering::SeqCst) == 2, 10_000, "the two first connections reach the handler").await;
    assert!(raw_text(port, "GET /c HTTP/1.1\r\n\r\n").await.is_empty(), "a third connection from the same peer is closed at accept");
    assert_eq!(entered.load(Ordering::SeqCst), 2, "the third is never handled");
    released.store(true, Ordering::SeqCst);
    release.notify_waiters();
    for answer in held {
        assert!(answer.await.unwrap().starts_with("HTTP/1.1 200"), "the two held ones are answered");
    }
    assert!(raw_text(port, "GET /d HTTP/1.1\r\n\r\n").await.starts_with("HTTP/1.1 200"), "a freed slot takes a new connection");
    capped.dispose().await;
}

async fn raw_text_owned(port: u16, head: &'static str) -> String {
    raw_text(port, head).await
}

/// "remote host: no body is read without leave and the head is bounded".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_body_is_read_without_leave_and_the_head_is_bounded() {
    let seen_body = Arc::new(Mutex::new(-1i64));
    let record = seen_body.clone();
    let server = listener(
        answer_with(move |request| {
            *record.lock().unwrap() = request.body.len() as i64;
            HttpResponse::json(if request.headers.contains_key("authorization") { 200 } else { 401 }, &json!({}), &[])
        }),
        ListenerOptions {
            body_limit: Some(Arc::new(|head: &HttpRequest| Ok(if head.headers.contains_key("authorization") { 1024 } else { 0 }))),
            ..ListenerOptions::default()
        },
    );
    let port = server.start().unwrap();
    let seen = || *seen_body.lock().unwrap();
    let reset = || *seen_body.lock().unwrap() = -1;
    assert!(
        raw_text(port, "POST /v2/runs HTTP/1.1\r\nContent-Type: application/json\r\nContent-Length: 100000\r\n\r\n")
            .await
            .starts_with("HTTP/1.1 401")
            && seen() == 0
    );
    reset();
    assert!(
        raw(port, b"POST /v2/runs HTTP/1.1\r\nContent-Length: 10\r\n\r\n", Some(b"0123456789"), false).await.starts_with("HTTP/1.1 401")
            && seen() == 0
    );
    reset();
    let oversize =
        raw(port, b"POST /v2/x HTTP/1.1\r\nAuthorization: Bearer k\r\nContent-Length: 2048\r\n\r\n", Some(&[0u8; 2048]), false).await;
    assert!(oversize.starts_with("HTTP/1.1 413") && seen() == -1, "over the head's limit is 413, unhandled");
    assert!(!oversize.to_lowercase().contains(protocol::HEADER), "an HTTP-layer refusal carries no version header");
    assert!(
        raw_text(port, "POST /v2/x HTTP/1.1\r\nAuthorization: Bearer k\r\nContent-Length: 999999999\r\n\r\n")
            .await
            .starts_with("HTTP/1.1 413")
    );
    for length in ["+2", "2, 2", "0x2", "", "1234567890"] {
        let head = format!("POST /v2/x HTTP/1.1\r\nAuthorization: Bearer k\r\nContent-Length: {length}\r\n\r\n{{}}");
        assert!(raw_text(port, &head).await.starts_with("HTTP/1.1 413"), "a malformed length is refused: {length}");
    }
    assert!(
        raw_text(port, "POST /v2/x HTTP/1.1\r\nAuthorization: Bearer k\r\nContent-Length: 2\r\n\r\n{}").await.starts_with("HTTP/1.1 200")
            && seen() == 2
    );
    assert!(
        raw_text(port, "POST /v2/x HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n").await.starts_with("HTTP/1.1 400"),
        "chunked is refused"
    );
    assert!(
        raw_text(port, "POST /v2/x HTTP/1.1\r\nAuthorization: Bearer k\r\nContent-Length: 2\r\nTransfer-Encoding: identity\r\n\r\n{}")
            .await
            .starts_with("HTTP/1.1 400")
    );
    let too_big = raw_text(port, &format!("GET /v2/x HTTP/1.1\r\nX-Big: {}\r\n\r\n", "a".repeat(8300))).await;
    assert!(too_big.starts_with("HTTP/1.1 431") && !too_big.to_lowercase().contains(protocol::HEADER), "an oversize head is 431");
    for (head, what) in [
        ("GET /v2/x HTTP/1.1\r\nA: 1\r\na: 2\r\n\r\n", "a repeated header"),
        ("POST /v2/x HTTP/1.1\r\nAuthorization: Bearer k\r\nContent-Length: 2\r\ncontent-length: 2\r\n\r\n{}", "a repeated length"),
        ("GET /v2/x HTTP/1.1\r\nA: 1\r\n folded\r\n\r\n", "a folded line"),
        ("GET /v2/x HTTP/1.1\r\nA: 1\nAuthorization: Bearer k\r\n\r\n", "a bare line feed inside a header"),
        ("GET /v2/x\rAuthorization: k HTTP/1.1\r\n\r\n", "a bare carriage return in the target"),
        ("GET /v2/x HTTP/1.1\r\nBad Name: 1\r\n\r\n", "a header name outside the token set"),
        ("GET /v2/x HTTP/1.1\r\nKéy: 1\r\n\r\n", "a non-ASCII header name"),
        ("DELETE /v2/x HTTP/1.1\r\n\r\n", "only GET and POST"),
        ("GET /v2/x HTTP/1.0\r\n\r\n", "only HTTP/1.1"),
        ("GET http://studio-pc/v2/x HTTP/1.1\r\n\r\n", "only an origin-form target"),
        (
            "POST /v2/x HTTP/1.1\r\nAuthorization: Bearer k\r\nContent-Length: 2\r\n\r\n{}GET / HTTP/1.1\r\n\r\n",
            "one request per connection",
        ),
        ("GET /v2/x HTTP/1.1\r\nAuthorization: Bearer k\r\n\r\nGET / HTTP/1.1\r\n\r\n", "one request per connection, without a body"),
    ] {
        assert!(raw_text(port, head).await.starts_with("HTTP/1.1 400"), "{what} is refused");
    }
    let mut not_utf8 = b"GET /v2/x HTTP/1.1\r\nA: ".to_vec();
    not_utf8.extend_from_slice(&[0xff, 0xfe]);
    not_utf8.extend_from_slice(b"\r\n\r\n");
    assert!(raw(port, &not_utf8, None, false).await.starts_with("HTTP/1.1 400"), "a head that is not UTF-8 is refused");
    let lines: String = (0..34).map(|i| format!("H{i}: 1\r\n")).collect();
    assert!(raw_text(port, &format!("GET /v2/x HTTP/1.1\r\n{lines}\r\n")).await.starts_with("HTTP/1.1 400"), "at most 33 header lines");
    let lines: String = (0..32).map(|i| format!("H{i}: 1\r\n")).collect();
    assert!(
        raw_text(port, &format!("GET /v2/x HTTP/1.1\r\n{lines}Authorization: k\r\n\r\n")).await.starts_with("HTTP/1.1 200"),
        "33 are fine"
    );
    // A body larger than the first buffer, of no power-of-two length, arrives whole and exact.
    let grown = Arc::new(Mutex::new(None));
    let keep = grown.clone();
    let large = listener(
        answer_with(move |request| {
            *keep.lock().unwrap() = Some(request.body.clone());
            ok_json()
        }),
        ListenerOptions::default(),
    );
    let large_port = large.start().unwrap();
    let sent: Vec<u8> = (0..200_003usize).map(|i| (i * 31 + i / 251) as u8).collect();
    let head = format!("POST /v2/x HTTP/1.1\r\nContent-Length: {}\r\n\r\n", sent.len());
    assert!(raw(large_port, head.as_bytes(), Some(&sent), false).await.starts_with("HTTP/1.1 200"), "a 200 KiB body is taken");
    assert_eq!(grown.lock().unwrap().as_deref(), Some(sent.as_slice()), "the handler sees the exact length and bytes");
    large.dispose().await;
    // Bytes that come in a write of their own after the whole body are still a second request.
    let head_seen = Arc::new(AtomicBool::new(false));
    let handled = Arc::new(AtomicBool::new(false));
    let (saw, did) = (head_seen.clone(), handled.clone());
    let pipelined = listener(
        answer_with(move |_| {
            did.store(true, Ordering::SeqCst);
            ok_json()
        }),
        ListenerOptions {
            body_limit: Some(Arc::new(move |_: &HttpRequest| {
                saw.store(true, Ordering::SeqCst);
                Ok(1024)
            })),
            ..ListenerOptions::default()
        },
    );
    let mut client = open(pipelined.start().unwrap(), "POST /v2/x HTTP/1.1\r\nContent-Length: 2\r\n\r\n").await;
    until(|| head_seen.load(Ordering::SeqCst), 10_000, "the head is read before the body is sent").await;
    client.write_all(b"{}GET / HTTP/1.1\r\n\r\n").await.unwrap();
    assert!(
        read_all(&mut client).await.starts_with("HTTP/1.1 400") && !handled.load(Ordering::SeqCst),
        "a request after the body is refused"
    );
    pipelined.dispose().await;
    let answer = raw_text(port, "GET /v2/x HTTP/1.1\r\nAuthorization: Bearer k\r\n\r\n").await;
    assert!(
        answer.contains("Connection: close")
            && answer.contains("Cache-Control: no-store")
            && answer.contains("X-Content-Type-Options: nosniff")
    );
    server.dispose().await;
}

/// "remote host: a slow or stuck request is cut off and stopping closes everything".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_slow_or_stuck_request_is_cut_off_and_stopping_closes_everything() {
    let entered = Arc::new(AtomicBool::new(false));
    let release = Arc::new(Notify::new());
    let (inside, wait) = (entered.clone(), release.clone());
    let handler: http_server::Handler = Arc::new(move |request: HttpRequest| {
        let (inside, wait) = (inside.clone(), wait.clone());
        Box::pin(async move {
            match request.target.as_str() {
                "/stuck" => {
                    inside.store(true, Ordering::SeqCst);
                    wait.notified().await;
                    Ok(ok_json())
                }
                "/hang" => std::future::pending().await,
                "/framing" => Ok(HttpResponse::json(
                    200,
                    &json!({}),
                    &[
                        ("Content-Length", "0"),
                        ("connection", "keep-alive"),
                        ("Content-Type", "text/html"),
                        ("Transfer-Encoding", "chunked"),
                    ],
                )),
                _ => Ok(HttpResponse::json(200, &json!({"ok": true}), &[("X-Ok", "1"), ("X-Evil", "a\r\nSet-Cookie: b")])),
            }
        })
    });
    let server = listener(
        handler,
        ListenerOptions {
            maximum_connections_per_peer: Some(1),
            request_deadline: Duration::from_millis(500),
            handler_deadline: Duration::from_millis(500),
            ..ListenerOptions::default()
        },
    );
    assert!(!server.is_listening(), "not listening before Start");
    let port = server.start().unwrap();
    assert!(server.is_listening(), "listening once started");
    assert!(server.start().is_err(), "a listener starts once");
    // A head trickled one byte at a time is still cut off at the deadline, and its slot is freed.
    let watch = Instant::now();
    let mut slow = open(port, "GET /v2/x HTTP/1.1\r\n").await;
    let (mut reader, mut writer) = slow.split();
    let trickle = async {
        for _ in 0..50 {
            if writer.write_all(b"X").await.is_err() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    };
    let closed = async {
        let mut buffer = [0u8; 64];
        let end = tokio::time::Instant::now() + Duration::from_secs(4);
        loop {
            match tokio::time::timeout_at(end, tokio::io::AsyncReadExt::read(&mut reader, &mut buffer)).await {
                Err(_) => return false,
                Ok(Ok(0)) | Ok(Err(_)) => return true,
                Ok(Ok(_)) => {}
            }
        }
    };
    let closed = tokio::select! { closed = closed => closed, _ = trickle => false };
    assert!(closed && watch.elapsed() < Duration::from_secs(4), "a head still arriving at the deadline is cut off");
    drop(slow);
    let ok = raw_text(port, "GET /v2/x HTTP/1.1\r\n\r\n").await;
    assert!(
        ok.starts_with("HTTP/1.1 200") && ok.contains("X-Ok: 1\r\n") && !ok.contains("Set-Cookie"),
        "a header with a line break is never written"
    );
    let framed = raw_text(port, "GET /framing HTTP/1.1\r\n\r\n").await;
    assert!(framed.starts_with("HTTP/1.1 200") && framed.ends_with("\r\n\r\n{}"), "the answer is framed by its own length");
    assert!(
        framed.split("\r\n").filter(|l| l.to_lowercase().starts_with("content-length:")).count() == 1
            && framed.contains("\r\nContent-Length: 2\r\n")
    );
    assert!(
        framed.contains("\r\nConnection: close\r\n")
            && !framed.to_lowercase().contains("keep-alive")
            && !framed.to_lowercase().contains("chunked")
            && !framed.contains("text/html"),
        "a handler cannot change the framing headers"
    );
    let mut hang = open(port, "GET /hang HTTP/1.1\r\n\r\n").await;
    assert!(closed_within(&mut hang, Duration::from_secs(4)).await, "a handler that never answers is cut off");
    drop(hang);
    assert!(raw_text(port, "GET /v2/x HTTP/1.1\r\n\r\n").await.starts_with("HTTP/1.1 200"), "and its slot is freed");
    // Stopping closes a live connection and takes no new ones.
    let mut stuck = open(port, "GET /stuck HTTP/1.1\r\n\r\n").await;
    until(|| entered.load(Ordering::SeqCst), 10_000, "the stuck request reaches its handler").await;
    let disposing = tokio::spawn({
        let server = server.clone();
        async move { server.dispose().await }
    });
    assert!(closed_within(&mut stuck, Duration::from_secs(4)).await, "stopping closes a live connection");
    assert!(tokio::time::timeout(Duration::from_secs(10), disposing).await.is_ok(), "stopping ends without waiting on the handler");
    release.notify_waiters();
    assert!(raw_text(port, "GET /v2/x HTTP/1.1\r\n\r\n").await.is_empty(), "a stopped listener takes no connection");
    assert!(server.start().is_err() && !server.is_listening(), "a stopped listener never starts again");
}

/// "remote host: a throwing filter or handler never stops the listener".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_throwing_filter_or_handler_never_stops_the_listener() {
    use std::io::ErrorKind;
    assert!(!counts_as_accept_failure(ErrorKind::ConnectionReset) && !counts_as_accept_failure(ErrorKind::ConnectionAborted));
    for kind in
        [ErrorKind::NetworkDown, ErrorKind::AddrNotAvailable, ErrorKind::OutOfMemory, ErrorKind::NetworkUnreachable, ErrorKind::Other]
    {
        assert!(counts_as_accept_failure(kind), "an error of the listener counts: {kind:?}");
    }
    let calls = Arc::new(AtomicUsize::new(0));
    let filter_calls = calls.clone();
    let handler: http_server::Handler = Arc::new(|request: HttpRequest| {
        if request.target == "/throw" {
            panic!("handler failed");
        }
        Box::pin(async move {
            match request.target.as_str() {
                "/fault" => Err(Failed),
                _ => Ok(ok_json()),
            }
        })
    });
    let server = listener(
        handler,
        ListenerOptions {
            body_limit: Some(Arc::new(|head: &HttpRequest| if head.target == "/limit" { Err(Failed) } else { Ok(1024) })),
            accept_peer: Some(Arc::new(move |_| if filter_calls.fetch_add(1, Ordering::SeqCst) == 0 { Err(Failed) } else { Ok(true) })),
            maximum_connections_per_peer: Some(1),
            ..ListenerOptions::default()
        },
    );
    let port = server.start().unwrap();
    assert!(raw_text(port, "GET /v2/x HTTP/1.1\r\n\r\n").await.is_empty(), "a filter that fails refuses the peer");
    for target in ["/throw", "/fault", "/limit"] {
        assert!(raw_text(port, &format!("GET {target} HTTP/1.1\r\n\r\n")).await.is_empty(), "{target} ends its connection unanswered");
        assert!(
            raw_text(port, "GET /v2/x HTTP/1.1\r\n\r\n").await.starts_with("HTTP/1.1 200"),
            "the listener and the slot outlive {target}"
        );
    }
    server.dispose().await;
}

/// "remote host: a draining connection holds its peer's slot until the drain ends".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_draining_connection_holds_its_peers_slot_until_the_drain_ends() {
    let handled = Arc::new(AtomicUsize::new(0));
    let released = Arc::new(AtomicUsize::new(0));
    let (count, freed) = (handled.clone(), released.clone());
    let unauthorized = move |count: Arc<AtomicUsize>| {
        answer_with(move |_| {
            count.fetch_add(1, Ordering::SeqCst);
            HttpResponse::json(401, &json!({}), &[])
        })
    };
    let options = |freed: Arc<AtomicUsize>, drain: Duration| ListenerOptions {
        body_limit: Some(Arc::new(|_: &HttpRequest| Ok(0))),
        maximum_connections_per_peer: Some(1),
        drain_deadline: drain,
        slot_released: Some(Arc::new(move || {
            freed.fetch_add(1, Ordering::SeqCst);
        })),
        ..ListenerOptions::default()
    };
    let server = listener(unauthorized(count), options(freed, Duration::from_secs(5)));
    let port = server.start().unwrap();
    let wait_released = |n: usize, millis: u64| {
        let released = released.clone();
        async move { until(|| released.load(Ordering::SeqCst) >= n, millis, "a slot is given back").await }
    };
    // A keyless POST is answered from its head and then drained; while it drains it still holds the peer's one slot.
    {
        let mut first = open(port, "POST /x HTTP/1.1\r\nContent-Length: 5\r\n\r\n").await;
        let mut answer = vec![0u8; 12];
        tokio::io::AsyncReadExt::read_exact(&mut first, &mut answer).await.unwrap();
        assert!(answer.starts_with(b"HTTP/1.1 401"), "the head alone is answered");
        assert!(raw_text(port, "POST /x HTTP/1.1\r\nContent-Length: 5\r\n\r\n").await.is_empty() && handled.load(Ordering::SeqCst) == 1);
    }
    wait_released(1, 10_000).await;
    assert!(raw_text(port, "GET /x HTTP/1.1\r\n\r\n").await.starts_with("HTTP/1.1 401") && handled.load(Ordering::SeqCst) == 2);
    wait_released(2, 10_000).await;
    // One that sends more than 64 KiB at once after its answer is closed once that much is discarded, long before the deadline.
    {
        let mut flooder = open(port, "POST /x HTTP/1.1\r\nContent-Length: 999999\r\n\r\n").await;
        let mut answer = vec![0u8; 12];
        tokio::io::AsyncReadExt::read_exact(&mut flooder, &mut answer).await.unwrap();
        assert!(answer.starts_with(b"HTTP/1.1 401"), "the flooder is answered");
        let _ = flooder.write_all(&vec![0u8; 80 * 1024]).await;
        wait_released(3, 3_000).await;
    }
    assert!(raw_text(port, "GET /x HTTP/1.1\r\n\r\n").await.starts_with("HTTP/1.1 401"), "and the peer is answered again");
    wait_released(4, 10_000).await;
    server.dispose().await;
    // One that keeps sending after its answer is drained for the default deadline only, then closed, and its slot is freed.
    let bounded_released = Arc::new(AtomicUsize::new(0));
    let bounded = listener(unauthorized(Arc::new(AtomicUsize::new(0))), options(bounded_released.clone(), Duration::from_secs(1)));
    let port = bounded.start().unwrap();
    let watch = Instant::now();
    let mut trickler = open(port, "POST /x HTTP/1.1\r\nContent-Length: 999\r\n\r\n").await;
    let mut answer = vec![0u8; 12];
    tokio::io::AsyncReadExt::read_exact(&mut trickler, &mut answer).await.unwrap();
    assert!(answer.starts_with(b"HTTP/1.1 401"), "the trickler is answered");
    let mut cut_off = false;
    while watch.elapsed() < Duration::from_secs(5) {
        if trickler.write_all(b"X").await.is_err() {
            cut_off = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(cut_off && watch.elapsed() < Duration::from_secs(3), "a peer that keeps sending is closed within about 2 s");
    until(|| bounded_released.load(Ordering::SeqCst) >= 1, 10_000, "the trickler's slot is given back").await;
    assert!(raw_text(port, "GET /x HTTP/1.1\r\n\r\n").await.starts_with("HTTP/1.1 401"), "and the peer is answered again");
    bounded.dispose().await;
}

/// "remote host: all peers together hold at most the open-connection cap".
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn all_peers_together_hold_at_most_the_open_connection_cap() {
    let entered = Arc::new(AtomicUsize::new(0));
    let open_gate = Arc::new(AtomicBool::new(false));
    let (count, gate) = (entered.clone(), open_gate.clone());
    let handler: http_server::Handler = Arc::new(move |_| {
        let (count, gate) = (count.clone(), gate.clone());
        Box::pin(async move {
            count.fetch_add(1, Ordering::SeqCst);
            while !gate.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Ok(ok_json())
        })
    });
    let server = listener(handler, ListenerOptions::default());
    let port = server.start().unwrap();
    let held: Vec<_> = (0..MAXIMUM_CONNECTIONS)
        .map(|i| tokio::spawn(async move { raw_text(port, &format!("GET /{i} HTTP/1.1\r\n\r\n")).await }))
        .collect();
    until(|| entered.load(Ordering::SeqCst) == MAXIMUM_CONNECTIONS, 20_000, "64 connections reach the handler").await;
    assert!(
        raw_text(port, "GET /over HTTP/1.1\r\n\r\n").await.is_empty() && entered.load(Ordering::SeqCst) == MAXIMUM_CONNECTIONS,
        "the 65th is closed"
    );
    open_gate.store(true, Ordering::SeqCst);
    for answer in held {
        assert!(answer.await.unwrap().starts_with("HTTP/1.1 200"), "the held ones are answered");
    }
    assert!(raw_text(port, "GET /next HTTP/1.1\r\n\r\n").await.starts_with("HTTP/1.1 200"), "a freed slot takes a new connection");
    server.dispose().await;
}

/// A command line split the way the C runtime powershell.exe starts with splits it (`CommandLineToArgvW` after the program name).
fn argv(line: &str) -> Vec<String> {
    let chars: Vec<char> = line.chars().collect();
    let mut result = vec![];
    let mut at = 0;
    loop {
        while at < chars.len() && matches!(chars[at], ' ' | '\t') {
            at += 1;
        }
        if at >= chars.len() {
            return result;
        }
        let mut value = String::new();
        let mut quoted = false;
        while at < chars.len() && (quoted || !matches!(chars[at], ' ' | '\t')) {
            let mut slashes = 0;
            while at < chars.len() && chars[at] == '\\' {
                slashes += 1;
                at += 1;
            }
            if at < chars.len() && chars[at] == '"' {
                value.extend(std::iter::repeat_n('\\', slashes / 2));
                if slashes % 2 == 1 {
                    value.push('"');
                } else if quoted && at + 1 < chars.len() && chars[at + 1] == '"' {
                    value.push('"');
                    at += 1;
                } else {
                    quoted = !quoted;
                }
                at += 1;
            } else {
                value.extend(std::iter::repeat_n('\\', slashes));
                if at < chars.len() && (quoted || !matches!(chars[at], ' ' | '\t')) {
                    value.push(chars[at]);
                    at += 1;
                }
            }
        }
        result.push(value);
    }
}

/// "remote host: commands run in PowerShell from the system folder and graph nodes reach the host". The runner's real runs (the fake
/// Codex, PowerShell) need the Windows process host: `commands_run_in_powershell_on_windows` below.
#[test]
fn commands_run_in_powershell_from_the_system_folder_and_graph_nodes_reach_the_host() {
    let (binary, arguments) = command_shell::launch("Get-Location; echo \"a b\"", r"C:\Windows\System32\").unwrap();
    assert_eq!(binary, r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe");
    assert_eq!(arguments, ["-NoProfile", "-NonInteractive", "-Command", "Get-Location; echo \"a b\""]);
    let line = command_shell::command_line("echo \"a b\"", r"C:\Windows\System32").unwrap();
    assert!(
        line.contains(" -NoProfile -NonInteractive -Command ") && line.ends_with("\"echo \\\"a b\\\"\""),
        "one quoted argument: {line}"
    );
    // A peer's text never becomes more than the one argument after -Command, whatever quotes and backslashes it holds.
    for text in [
        "a\" -EncodedCommand ZQBjAGgAbwA=",
        "ends in a slash\\",
        "\\\" -File x.ps1",
        "x\\\\\" & calc \"",
        "\"\"",
        " lead and trail ",
        "tab\there",
        "%PATH% & ^ | < > ;",
        "-",
        "'single' `tick` $(sub)",
    ] {
        let split = argv(&command_shell::command_line(text, r"C:\Windows\System32").unwrap());
        assert_eq!(
            split,
            [r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe", "-NoProfile", "-NonInteractive", "-Command", text],
            "one argument for {text}"
        );
    }
    // CreateProcess takes at most 32,767 characters with its NUL: a host can tell which commands fit before it starts one.
    let room = 32766 - command_shell::command_line("", r"C:\Windows\System32").unwrap().len() + 2;
    assert!(command_shell::fits("echo x", r"C:\Windows\System32").unwrap());
    assert_eq!(command_shell::command_line(&"a".repeat(room), r"C:\Windows\System32").unwrap().len(), 32766);
    assert!(
        command_shell::fits(&"a".repeat(room), r"C:\Windows\System32").unwrap()
            && !command_shell::fits(&"a".repeat(room + 1), r"C:\Windows\System32").unwrap()
    );
    for folder in ["", "System32", r"\Windows\System32", "C:Windows", r"..\System32", "C:\\Win\"dows"] {
        assert!(command_shell::launch("echo x", folder).is_err(), "a system folder that is not a full path is refused: {folder}");
        assert!(mighty_run::manager::power_shell_launch("echo x", folder).is_err(), "the runner refuses it too: {folder}");
    }
    assert_eq!(
        mighty_run::manager::power_shell_launch("x", r"C:\Windows\System32").unwrap(),
        command_shell::launch("x", r"C:\Windows\System32").unwrap()
    );
}

#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn commands_run_in_powershell_on_windows() {
    use mighty_core::model::Workspace;
    use mighty_remote::remote::host::HostRunner;
    use mighty_run::manager::{RunManager, RunManagerOptions};
    use mighty_run::model::{CliCommand, RunEvent};
    use mighty_run::providers::{Finder, ProviderCatalog};
    set_language("ko");
    let directory = temp("powershell");
    let workspace = Workspace { id: Some("ws".into()), path: Some(directory.to_string_lossy().into_owned()), ..Workspace::default() };
    let plugin = directory.join("plugin");
    std::fs::create_dir_all(plugin.join(".claude-plugin")).unwrap();
    std::fs::write(plugin.join(".claude-plugin").join("plugin.json"), "{}").unwrap();
    let record = directory.join("codex").to_string_lossy().into_owned();
    let command =
        CliCommand::new(env!("CARGO_BIN_EXE_mighty-remote-fake-cli"), vec!["--fake-cli".into(), "codex".into(), record], Some("2.1.271"));
    let finder: Finder = Arc::new(move |_provider: String| {
        let found = Some(command.clone());
        Box::pin(async move { found })
    });
    let environment = Arc::new(mighty_platform::fake::FakeShellEnvironment::new(std::env::vars().collect()));
    let catalog = Arc::new(ProviderCatalog::new(Arc::new(mighty_platform_windows::WindowsProcessHost::new()), environment, Some(finder)));
    let events: Arc<Mutex<Vec<RunEvent>>> = Arc::default();
    let nodes: Arc<Mutex<Vec<String>>> = Arc::default();
    let (sink, node_sink) = (events.clone(), nodes.clone());
    let resolved = workspace.clone();
    let resolve: mighty_run::manager::ResolveWorkspace = Arc::new(move |_| {
        let w = resolved.clone();
        Box::pin(async move { Ok(w) })
    });
    let mut options = RunManagerOptions::new(&plugin.to_string_lossy(), resolve.clone(), Arc::new(move |e| sink.lock().unwrap().push(e)));
    options.graph = Some(Arc::new(mighty_graph::TrackerFactory::default()));
    options.graph_nodes = Some(Arc::new(move |id: &str, _node| node_sink.lock().unwrap().push(id.to_string())));
    let system = format!("{}\\System32", std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into()));
    options.powershell_commands = Some(system);
    let runner = RunManager::new(catalog.clone(), options);
    let remote: &dyn HostRunner = &runner;
    let ended = |id: &'static str| {
        let events = events.clone();
        move || events.lock().unwrap().iter().any(|e| e.session_id == id && matches!(e.status.as_deref(), Some("completed" | "error")))
    };
    remote.start(StartRunRequest::new("job-a", "ws", "claude", "hello").with_provider("codex")).await.unwrap();
    until(ended("job-a"), 20_000, "job-a ends").await;
    assert!(nodes.lock().unwrap().iter().any(|n| n == "job-a"), "the run's graph nodes reach the host as they change");
    assert!(nodes.lock().unwrap().iter().all(|n| n == "job-a"), "every node names its run");
    remote
        .start(StartRunRequest::new("job-b", "ws", "shell", "echo remote-ok; echo \"nocd=$env:NoDefaultCurrentDirectoryInExePath\""))
        .await
        .unwrap();
    until(ended("job-b"), 20_000, "job-b ends").await;
    let said = |id: &str, text: &str| {
        events
            .lock()
            .unwrap()
            .iter()
            .any(|e| e.session_id == id && e.entry.as_ref().and_then(|l| l.text.as_deref()).is_some_and(|t| t.contains(text)))
    };
    assert!(said("job-b", "remote-ok"), "a command runs in PowerShell");
    assert!(said("job-b", "nocd=1"), "the command runs with NoDefaultCurrentDirectoryInExePath=1");
    assert!(!nodes.lock().unwrap().iter().any(|n| n == "job-b"), "a command pane has no graph");
    // A remote run works in the shared folder alone: a worktree folder is never taken from the host's caller.
    let elsewhere = directory.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let mut worktree = StartRunRequest::new("job-c", "ws", "claude", "hello").with_provider("codex");
    worktree.working_folder = Some(elsewhere.to_string_lossy().into_owned());
    assert!(matches!(remote.start(worktree).await, Err(mighty_run::RunError::Invalid(_))));
    assert!(!runner.is_running("job-c") && !events.lock().unwrap().iter().any(|e| e.session_id == "job-c"), "refused before it starts");
    let mut granted = StartRunRequest::new("job-d", "ws", "claude", "hello");
    granted.style_auto_allow = Some(vec!["ToolSearch".into()]);
    assert!(granted.validate().unwrap().style_auto_allow.is_some_and(|g| g.len() == 1), "the grant itself is one a local style may give");
    assert!(matches!(remote.start(granted.clone()).await, Err(mighty_run::RunError::Invalid(_))));
    assert!(!runner.is_running("job-d") && !events.lock().unwrap().iter().any(|e| e.session_id == "job-d"), "never a local style's tools");
    granted.session_id = "job-f".into();
    granted.style_auto_allow = None;
    remote.start(granted).await.unwrap();
    until(|| events.lock().unwrap().iter().any(|e| e.session_id == "job-f"), 20_000, "job-f starts").await;
    remote.stop("job-f".into()).await.unwrap();
    remote.dispose().await;
    // A host callback that panics loses its node, never the run's last status.
    let thrown = Arc::new(AtomicUsize::new(0));
    let counted = thrown.clone();
    let sink = events.clone();
    let mut options = RunManagerOptions::new(&plugin.to_string_lossy(), resolve, Arc::new(move |e| sink.lock().unwrap().push(e)));
    options.graph = Some(Arc::new(mighty_graph::TrackerFactory::default()));
    options.graph_nodes = Some(Arc::new(move |_: &str, _| {
        counted.fetch_add(1, Ordering::SeqCst);
        panic!("host failed");
    }));
    let throwing = RunManager::new(catalog, options);
    HostRunner::start(&throwing, StartRunRequest::new("job-e", "ws", "claude", "hello").with_provider("codex")).await.unwrap();
    until(ended("job-e"), 20_000, "job-e ends").await;
    let last =
        |id: &str| events.lock().unwrap().iter().rev().find(|e| e.session_id == id && e.status.is_some()).and_then(|e| e.status.clone());
    assert!(thrown.load(Ordering::SeqCst) > 0 && last("job-e") == last("job-a"), "the run ends as it would have");
    throwing.dispose().await;
    let _ = std::fs::remove_dir_all(&directory);
}

/// "remote host: only a keyed run request gets the attachment body".
#[test]
fn only_a_keyed_run_request_gets_the_attachment_body() {
    set_language("ko");
    let key = "k".repeat(43);
    let head = |target: &str, extra: &[(&str, &str)], peer: &str| {
        let mut headers: HashMap<String, String> =
            [("authorization".to_string(), format!("Bearer {key}")), ("x-mighty-remote-version".to_string(), "2".to_string())].into();
        for (name, value) in extra {
            headers.insert(name.to_string(), value.to_string());
        }
        HttpRequest { method: "POST".into(), target: target.into(), headers, body: vec![], remote_address: peer.into() }
    };
    let attachments = mighty_run::attachments::MAXIMUM_REQUEST_BYTES;
    assert_eq!(
        host::body_limit(&head("/v2/runs", &[], "100.90.1.1"), &key, false),
        attachments,
        "a keyed run request may carry attachments"
    );
    let wrong = format!("Bearer {}", "x".repeat(43));
    assert_eq!(
        host::body_limit(&head("/v2/runs", &[("authorization", &wrong)], "100.90.1.1"), &key, false),
        0,
        "a wrong key reads nothing"
    );
    let mut keyless = head("/v2/runs", &[], "100.90.1.1");
    keyless.headers.remove("authorization");
    assert_eq!(host::body_limit(&keyless, &key, false), 0, "no key reads nothing");
    let basic = format!("Basic {key}");
    assert_eq!(host::body_limit(&head("/v2/runs", &[("authorization", &basic)], "100.90.1.1"), &key, false), 0, "only a bearer key counts");
    assert_eq!(host::body_limit(&head("/v2/runs", &[("x-mighty-remote-version", "1")], "100.90.1.1"), &key, false), 0, "an old protocol");
    assert_eq!(host::body_limit(&head("/v2/runs", &[("origin", "https://example.com")], "100.90.1.1"), &key, false), 0, "a browser page");
    assert_eq!(host::body_limit(&head("/v2/runs/x/steer", &[], "100.90.1.1"), &key, false), protocol::MAXIMUM_REQUEST_BYTES, "512 KiB");
    assert_eq!(
        host::body_limit(&head("/v2/runs?x=1", &[], "100.90.1.1"), &key, false),
        protocol::MAXIMUM_REQUEST_BYTES,
        "only the bare route"
    );
    assert!(
        host::body_limit(&head("/v2/runs", &[], "192.168.1.5"), &key, false) == 0
            && host::body_limit(&head("/v2/runs", &[], "127.0.0.1"), &key, false) == 0
    );
    assert_eq!(host::body_limit(&head("/v2/runs", &[], "127.0.0.1"), &key, true), attachments, "loopback only when injected");
    let shell = StartRunRequest::new("j", "w", "shell", "dir");
    let settings = |mode: &str| mighty_core::model::RunSettings { permission_mode: mode.into(), ..Default::default() };
    let full = StartRunRequest::new("j", "w", "claude", "hi").with_settings(settings("fullAccess"));
    let edits = StartRunRequest::new("j", "w", "claude", "hi").with_settings(settings("acceptEdits"));
    let refusal = RemoteHost::full_access_refusal;
    let (commands_off, full_access_off) = (get("remote.host.error.commandsOff"), get("remote.host.error.fullAccessOff"));
    assert!(
        refusal(&shell, false, cfg!(windows)) == Some(commands_off.clone())
            && refusal(&full, false, cfg!(windows)) == Some(full_access_off.clone())
    );
    assert_eq!(
        refusal(&full.clone().with_provider("codex"), false, false),
        Some(full_access_off.clone()),
        "full access waits whatever the agent"
    );
    assert!(
        refusal(&edits, false, false).is_none()
            && refusal(&shell, true, false).is_none()
            && refusal(&full.clone().with_provider("codex"), true, false).is_none()
    );
    // A Windows host cannot confirm it confines Codex or Antigravity as Seatbelt does on a Mac: without the switch they are refused.
    for (provider, mode) in
        [("codex", "manual"), ("codex", "acceptEdits"), ("codex", "onRequest"), ("gemini", "manual"), ("gemini", "acceptEdits")]
    {
        let agent = StartRunRequest::new("j", "w", "claude", "hi").with_provider(provider).with_settings(settings(mode));
        assert_eq!(refusal(&agent, false, true), Some(strings::host_agent_needs_full_access(provider)), "{provider} {mode} on Windows");
        assert!(
            refusal(&agent, true, true).is_none() && refusal(&agent, false, false).is_none(),
            "{provider} {mode}: the switch, or a Mac"
        );
    }
    assert!(
        refusal(&edits, false, true).is_none() && refusal(&StartRunRequest::new("j", "w", "claude", "hi"), false, true).is_none(),
        "Claude keeps its modes"
    );
    assert!(refusal(&full, false, true) == Some(full_access_off) && refusal(&shell, false, true) == Some(commands_off));
    assert!(refusal(&full, true, true).is_none() && refusal(&shell, true, true).is_none());
    assert_eq!(refusal(&full.clone().with_provider("codex"), false, true), Some(strings::host_agent_needs_full_access("codex")));
    let now = Arc::new(Mutex::new(Duration::ZERO));
    let read = now.clone();
    let limiter = RateLimiter::new(Arc::new(move || *read.lock().unwrap()));
    assert!((0..300).all(|_| limiter.allow_peer("100.90.1.1")) && !limiter.allow_peer("100.90.1.1") && limiter.allow_peer("100.90.1.2"));
    assert!((0..600).all(|_| limiter.allow_keyed()) && !limiter.allow_keyed(), "600 keyed requests a second in all");
    *now.lock().unwrap() += Duration::from_secs(1);
    assert!(limiter.allow_peer("100.90.1.1") && limiter.allow_keyed(), "the next second starts afresh");
}

fn get_request(target: &str, key: &str, extra: &str) -> String {
    format!("GET {target} HTTP/1.1\r\nAuthorization: Bearer {key}\r\nx-mighty-remote-version: 2\r\n{extra}\r\n")
}

/// "remote host: sharing starts with a new key and a stop revokes it".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sharing_starts_with_a_new_key_and_a_stop_revokes_it() {
    set_language("ko");
    let fixture = make_host(MakeHost::default()).await;
    let host = fixture.host.clone();
    assert_eq!(host.start(&[], 0).await.unwrap_err(), get("remote.host.error.pickWorkspace"), "nothing picked");
    assert_eq!(
        host.start(&[fixture.remote.id().into()], 0).await.unwrap_err(),
        strings::host_local_only(),
        "a remote workspace is never shared"
    );
    assert_eq!(host.start(&[fixture.shared.id().into()], 80).await.unwrap_err(), get("remote.connect.error.badPort"), "ports below 1024");
    let state = host.start(&[fixture.shared.id().into()], 0).await.unwrap();
    let port = state.port.unwrap();
    assert!(state.enabled && !state.allows_full_access && port > 0 && validation::key(state.key.as_deref()));
    assert_eq!(state.addresses, [format!("127.0.0.1:{port}")], "sharing on loopback (injected)");
    assert_eq!(host.start(&[fixture.shared.id().into()], 0).await.unwrap_err(), get("remote.host.error.busy"), "one sharing at a time");
    let key = state.key.clone().unwrap();
    let target = loopback(port);
    let info =
        RemoteWireInfo::decode(&transport::request(&target, &key, Method::Get, "/v2/info", Request::default()).await.unwrap()).unwrap();
    validation::info(&info).unwrap();
    assert!(
        info.protocol == 2 && info.host_name == "studio-pc" && info.workspaces.len() == 1 && info.workspaces[0].id == fixture.shared.id()
    );
    assert_eq!(info.runtime.platform, "win32", "and the host's OS");
    let plain = raw_text(port, &get_request("/v2/info", &key, "")).await;
    assert!(
        plain.starts_with("HTTP/1.1 200") && plain.contains("x-mighty-remote-version: 2") && !plain.contains(&key),
        "/info never carries the key"
    );
    let wrong = transport::request(&target, &"x".repeat(43), Method::Get, "/v2/info", Request::default()).await.unwrap_err();
    assert_eq!(wrong.status(), Some(401), "a wrong key is 401");
    let keyless = raw_text(port, "GET /v2/info HTTP/1.1\r\nx-mighty-remote-version: 2\r\n\r\n").await;
    assert!(keyless.starts_with("HTTP/1.1 401") && keyless.contains("x-mighty-remote-version: 2"), "no key is 401, with the version");
    assert!(raw_text(port, "GET /v2/nothing HTTP/1.1\r\nx-mighty-remote-version: 2\r\n\r\n").await.starts_with("HTTP/1.1 401"));
    let unread = format!(
        "POST /v2/runs HTTP/1.1\r\nAuthorization: Bearer {}\r\nx-mighty-remote-version: 2\r\nContent-Type: application/json\r\nContent-Length: 400000\r\n\r\n",
        "x".repeat(43)
    );
    assert!(raw_text(port, &unread).await.starts_with("HTTP/1.1 401"), "a wrong key is answered from the head alone");
    let old = format!("GET /v2/info HTTP/1.1\r\nAuthorization: Bearer {key}\r\nx-mighty-remote-version: 1\r\n\r\n");
    assert!(raw_text(port, &old).await.starts_with("HTTP/1.1 426"), "an old protocol is 426");
    let browser = raw_text(port, &get_request("/v2/info", &key, "Origin: https://example.com\r\n")).await;
    assert!(
        browser.starts_with("HTTP/1.1 403") && browser.contains("x-mighty-remote-version: 2"),
        "a browser page is 403, even with the key"
    );
    let answer = raw_text(port, &get_request("/v2/nothing", &key, "")).await;
    assert!(answer.starts_with("HTTP/1.1 404") && answer.contains("x-mighty-remote-version: 2"), "every answer carries the version");
    assert!(raw_text(port, &get_request("/v2/info?x=1", &key, "")).await.starts_with("HTTP/1.1 404"));
    assert!(
        raw_text(port, &get_request("/v2/runs/../info", &key, "")).await.starts_with("HTTP/1.1 404"),
        "a dot segment is never resolved"
    );
    assert!(host.set_allows_full_access(true).allows_full_access, "the switch turns on while sharing");
    let mut pending = open(port, "GET /v2/info HTTP/1.1\r\n").await;
    let stopped = host.stop().await;
    assert!(!stopped.enabled && stopped.key.is_none() && !stopped.allows_full_access, "stopping forgets the key and the switch");
    assert!(closed_within(&mut pending, Duration::from_secs(5)).await, "a connection open when sharing stops is closed");
    assert!(transport::request(&target, &key, Method::Get, "/v2/info", Request::default()).await.is_err(), "nothing answers after a stop");
    let again = host.start(&[fixture.shared.id().into()], 0).await.unwrap();
    assert!(again.key != state.key && !again.allows_full_access, "a new sharing has a new key and the switch off");
    let refused = raw_text(again.port.unwrap(), &get_request("/v2/info", &key, "")).await;
    assert!(refused.starts_with("HTTP/1.1 401"), "the old key is refused by the new sharing");
    host.dispose().await;
    assert_eq!(host.start(&[fixture.shared.id().into()], 0).await.unwrap_err(), get("common.appClosing"), "no sharing after shutdown");
    fixture.dispose().await;
}

/// "remote host: a flood is refused and the refusal carries the version".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_flood_is_refused_and_the_refusal_carries_the_version() {
    let now = Arc::new(Mutex::new(Duration::ZERO));
    let read = now.clone();
    let fixture = make_host(MakeHost { clock: Some(Arc::new(move || *read.lock().unwrap())), ..MakeHost::default() }).await;
    let state = fixture.host.start(&[fixture.shared.id().into()], 0).await.unwrap();
    let port = state.port.unwrap();
    let keyed = get_request("/v2/info", state.key.as_deref().unwrap(), "");
    for i in 0..300 {
        assert!(
            raw_text(port, "GET /v2/info HTTP/1.1\r\nx-mighty-remote-version: 2\r\n\r\n").await.starts_with("HTTP/1.1 401"),
            "request {i}"
        );
    }
    let refused = raw_text(port, &keyed).await;
    assert!(refused.starts_with("HTTP/1.1 429") && refused.contains("x-mighty-remote-version: 2"), "the 301st request is 429: {refused}");
    *now.lock().unwrap() += Duration::from_secs(1);
    assert!(raw_text(port, &keyed).await.starts_with("HTTP/1.1 200"), "the next second is answered");
    fixture.dispose().await;
}

/// "remote host: a listener that stops accepting turns sharing off".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_listener_that_stops_accepting_turns_sharing_off() {
    let addresses: host::Addresses = Arc::new(|| Ok(vec!["127.0.0.1".into(), "::1".into()]));
    let fixture = make_host(MakeHost { addresses: Some(addresses), ..MakeHost::default() }).await;
    let state = fixture.host.start(&[fixture.shared.id().into()], 0).await.unwrap();
    let port = state.port.unwrap();
    let keyed = get_request("/v2/info", state.key.as_deref().unwrap(), "");
    let listeners = fixture.host.listeners();
    assert!(
        state.addresses == [format!("127.0.0.1:{port}"), format!("[::1]:{port}")]
            && listeners.len() == 2
            && listeners.iter().all(|l| l.is_listening())
    );
    assert!(raw(port, keyed.as_bytes(), None, true).await.starts_with("HTTP/1.1 200"), "both addresses answer");
    // Stands in for accepting that failed for good on one address.
    listeners[0].dispose().await;
    let other = raw(port, keyed.as_bytes(), None, true).await;
    assert!(
        other.starts_with("HTTP/1.1 503") && other.contains("x-mighty-remote-version: 2"),
        "the other address stops answering too: {other}"
    );
    let now = fixture.host.state();
    assert!(!now.enabled && now.key.is_none() && fixture.host.listeners().is_empty(), "sharing shows as off, its listeners closed");
    let again = fixture.host.start(&[fixture.shared.id().into()], 0).await.unwrap();
    assert!(again.key != state.key, "sharing starts again with a new key");
    fixture.host.listeners()[1].dispose().await;
    let now = fixture.host.state();
    assert!(!now.enabled && now.key.is_none(), "Current alone notices a listener that stopped");
    fixture.dispose().await;
}

/// "remote host: a stop while a start binds closes what it opened".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stop_while_a_start_binds_closes_what_it_opened() {
    set_language("ko");
    let fixture = make_host(MakeHost::default()).await;
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let (inside, wait) = (entered.clone(), release.clone());
    fixture.host.set_before_bind(Some(Arc::new(move |_| {
        let (inside, wait) = (inside.clone(), wait.clone());
        Box::pin(async move {
            inside.notify_one();
            wait.notified().await;
        })
    })));
    let host = fixture.host.clone();
    let shared = fixture.shared.id().to_string();
    let starting = tokio::spawn(async move { host.start(&[shared], 0).await });
    tokio::time::timeout(Duration::from_secs(10), entered.notified()).await.unwrap();
    fixture.host.stop().await;
    release.notify_one();
    assert_eq!(starting.await.unwrap().unwrap_err(), get("remote.host.error.startCancelled"), "a start a stop overtook is cancelled");
    let state = fixture.host.state();
    assert!(
        fixture.host.listeners().is_empty()
            && !state.enabled
            && state.key.is_none()
            && fixture.runner().is_some_and(|r| r.disposed.load(Ordering::SeqCst))
    );
    fixture.dispose().await;
}

/// "remote host: a stop while a start makes its runner disposes that runner".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stop_while_a_start_makes_its_runner_disposes_that_runner() {
    set_language("ko");
    let made: Arc<Mutex<Option<Arc<FakeRunner>>>> = Arc::default();
    let keep = made.clone();
    let factory: RunnerFactory = Arc::new(move |host: &RemoteHost, sinks| {
        let host = host.clone();
        tokio::spawn(async move { host.stop().await });
        let runner = FakeRunner::new(sinks);
        *keep.lock().unwrap() = Some(runner.clone());
        // The stop runs before the start goes on (C#'s `_ = f.Host.StopAsync()` runs to its first wait at once).
        std::thread::sleep(Duration::from_millis(200));
        runner
    });
    let fixture = make_host(MakeHost { runner: Some(factory), ..MakeHost::default() }).await;
    assert_eq!(fixture.host.start(&[fixture.shared.id().into()], 0).await.unwrap_err(), get("remote.host.error.startCancelled"));
    let state = fixture.host.state();
    let runner = made.lock().unwrap().clone().unwrap();
    assert!(runner.disposed.load(Ordering::SeqCst) && fixture.host.listeners().is_empty() && !state.enabled && state.key.is_none());
    fixture.dispose().await;
}

/// "remote host: the run count is told outside the host's lock and a failing listener of it changes nothing".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_run_count_is_told_outside_the_hosts_lock_and_a_failing_listener_of_it_changes_nothing() {
    let told: Arc<Mutex<Vec<usize>>> = Arc::default();
    let gate_held = Arc::new(AtomicBool::new(false));
    let holder: Arc<Mutex<Option<RemoteHost>>> = Arc::default();
    let (record, held, asked) = (told.clone(), gate_held.clone(), holder.clone());
    let runs_changed: Arc<dyn Fn(usize) + Send + Sync> = Arc::new(move |count| {
        record.lock().unwrap().push(count);
        // Another thread asks the host: it would wait for ever if this were told under the host's lock.
        if let Some(host) = asked.lock().unwrap().clone() {
            let (done, wait) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let _ = host.state();
                let _ = done.send(());
            });
            if wait.recv_timeout(Duration::from_secs(5)).is_err() {
                held.store(true, Ordering::SeqCst);
            }
        }
        panic!("the app failed to take the count");
    });
    let fixture = make_host(MakeHost { runs_changed: Some(runs_changed), ..MakeHost::default() }).await;
    *holder.lock().unwrap() = Some(fixture.host.clone());
    let state = fixture.host.start(&[fixture.shared.id().into()], 0).await.unwrap();
    let stopped = fixture.host.stop().await;
    assert!(!stopped.enabled && fixture.host.listeners().is_empty() && fixture.runner().is_some_and(|r| r.disposed.load(Ordering::SeqCst)));
    let counts = told.lock().unwrap().clone();
    assert!(
        !counts.is_empty() && counts.iter().all(|c| *c == 0) && !gate_held.load(Ordering::SeqCst),
        "told, outside the lock: {counts:?}"
    );
    assert!(
        transport::request(&loopback(state.port.unwrap()), state.key.as_deref().unwrap(), Method::Get, "/v2/info", Request::default())
            .await
            .is_err()
    );
    assert!(fixture.host.start(&[fixture.shared.id().into()], 0).await.unwrap().enabled, "and sharing starts again");
    *holder.lock().unwrap() = None;
    fixture.dispose().await;
}

/// "remote host: sharing refuses what it cannot serve".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sharing_refuses_what_it_cannot_serve() {
    set_language("ko");
    let no_tailscale = strings::host_no_tailscale();
    // The LAN and the wildcard address, never bound (the wildcard spelled from the type, never written out).
    let lan: host::Addresses = Arc::new(|| Ok(vec!["192.168.1.5".into(), Ipv4Addr::UNSPECIFIED.to_string()]));
    let fixture = make_host(MakeHost { addresses: Some(lan), ..MakeHost::default() }).await;
    assert_eq!(
        fixture.host.start(&[fixture.shared.id().into()], 0).await.unwrap_err(),
        no_tailscale,
        "no address but Tailscale's is ever bound"
    );
    assert!(fixture.network.binds().is_empty(), "nothing was bound");
    fixture.dispose().await;
    let unreadable: host::Addresses = Arc::new(|| Err("adapters".into()));
    let fixture = make_host(MakeHost { addresses: Some(unreadable), ..MakeHost::default() }).await;
    assert_eq!(fixture.host.start(&[fixture.shared.id().into()], 0).await.unwrap_err(), no_tailscale, "adapters that cannot be read");
    fixture.dispose().await;
    let fixture = make_host(MakeHost::default()).await;
    let taken = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let busy_port = taken.local_addr().unwrap().port();
    let prefix = get_with("remote.host.error.listenFailed", &[("error", "\u{1}")]).split('\u{1}').next().unwrap().to_string();
    let failed = fixture.host.start(&[fixture.shared.id().into()], busy_port).await.unwrap_err();
    assert!(failed.starts_with(&prefix) && failed.len() > prefix.len(), "a port in use is a listen failure: {failed}");
    let state = fixture.host.state();
    assert!(
        fixture.host.listeners().is_empty()
            && !state.enabled
            && state.key.is_none()
            && fixture.runner().is_some_and(|r| r.disposed.load(Ordering::SeqCst))
    );
    drop(taken);
    assert!(fixture.host.set_allows_full_access(true).allows_full_access, "the switch can be set while sharing is off");
    let state = fixture.host.start(&[fixture.shared.id().into()], 0).await.unwrap();
    assert!(state.enabled && !state.allows_full_access, "sharing always starts with the switch off");
    let port = state.port.unwrap();
    let key = state.key.clone().unwrap();
    for target in ["/v2\\info", "/v2/info#x"] {
        let answer = raw_text(port, &get_request(target, &key, "")).await;
        assert!(
            answer.starts_with("HTTP/1.1 400") && answer.contains("x-mighty-remote-version: 2"),
            "a target the Mac cannot read: {target}"
        );
    }
    let accented = raw(port, get_request("/v2/infö", &key, "").as_bytes(), None, false).await;
    assert!(accented.starts_with("HTTP/1.1 400") && accented.contains("x-mighty-remote-version: 2"), "a non-ASCII target is 400");
    assert!(raw_text(port, &get_request("http://x/v2/info", &key, "")).await.starts_with("HTTP/1.1 400"), "an absolute-form target");
    assert!(raw_text(port, &get_request("/v2/info", &key, "")).await.starts_with("HTTP/1.1 200"), "and the plain target is answered");
    fixture.dispose().await;
}
