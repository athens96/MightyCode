//! A hostile or broken host against the client (the W8 security review's D-M1, D-L3): chunked answers that try to make the client
//! scan without end, overflow its sums or slice past its buffer are refused as failures, never a panic; a connect that is dropped or
//! fails gives its address back; and a host whose route panics still answers 500 with what it said, as C#'s ServeAsync does.

mod support;

use std::net::Ipv4Addr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use mighty_platform::fake::MemoryVault;
use mighty_remote::remote::transport::{self, Method, Request, TransportError};
use mighty_remote::remote::wire::protocol;
use mighty_run::strings::{get, set_language};
use support::fake_host::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// One request answered with `head` (the version header added) and then `body`, written as it is; the connection is held open
/// afterwards until the client lets go, so an answer that never ends is the client's to refuse.
async fn answering(head: &str, body: Vec<u8>) -> u16 {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let head = format!("{head}\r\n{}: 2\r\n\r\n", protocol::HEADER);
    tokio::spawn(async move {
        let Ok((mut stream, _)) = listener.accept().await else { return };
        let mut request = vec![0u8; 8192];
        let _ = stream.read(&mut request).await;
        let _ = stream.write_all(head.as_bytes()).await;
        let _ = stream.write_all(&body).await;
        let mut rest = [0u8; 64];
        let _ = tokio::time::timeout(Duration::from_secs(10), stream.read(&mut rest)).await;
    });
    port
}

async fn chunked(body: Vec<u8>, maximum: Option<usize>) -> Result<Vec<u8>, TransportError> {
    let port = answering("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked", body).await;
    let options = Request { timeout: Some(Duration::from_secs(5)), maximum_bytes: maximum, ..Request::default() };
    transport::request(&loopback(port), KEY, Method::Get, "/v2/info", options).await
}

/// "remote hostile: a chunked answer is read whole, and every malformed or oversized one is a failure, never a panic".
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hostile_chunked_answers_are_failures_never_a_panic() {
    set_language("en");
    let too_large = TransportError::InvalidData(get("remote.connect.error.responseTooLarge"));
    // A well-formed answer, an extension and a trailer included, arriving byte by byte in two chunks.
    assert_eq!(chunked(b"3;ext=1\r\n{\"a\r\n3\r\n\":1\r\n1\r\n}\r\n0\r\nX-Trailer: y\r\n\r\n".to_vec(), None).await.unwrap(), b"{\"a\":1}");
    // usize::MAX: body.len() + size would overflow.
    assert_eq!(chunked(b"ffffffffffffffff\r\nx".to_vec(), None).await.unwrap_err(), too_large);
    // More than 16 hex digits, a sign, a space inside, nothing: no size.
    for line in ["10000000000000000", "+5", "-5", "1 0", "", "zz", "0x10"] {
        let refused = chunked(format!("{line}\r\nhello\r\n0\r\n\r\n").into_bytes(), None).await.unwrap_err();
        assert!(matches!(refused, TransportError::Network(_)), "{line:?}: {refused:?}");
    }
    // A size line that never ends (no CRLF): refused once it passes the line limit, without waiting for the connection's end.
    let endless = vec![b'1'; 64 * 1024];
    assert!(matches!(chunked(endless, None).await.unwrap_err(), TransportError::Network(_)), "an endless size line");
    // A chunk past the maximum, alone or in sum (one cut short is a_chunked_answer_cut_short_is_refused).
    assert_eq!(chunked(b"20\r\n".to_vec(), Some(16)).await.unwrap_err(), too_large);
    assert_eq!(chunked(b"8\r\naaaaaaaa\r\n9\r\nbbbbbbbbb\r\n0\r\n\r\n".to_vec(), Some(16)).await.unwrap_err(), too_large);
    // A chunk whose data is not followed by CRLF: its size lied.
    assert!(matches!(chunked(b"2\r\nabcdef\r\n0\r\n\r\n".to_vec(), None).await.unwrap_err(), TransportError::Network(_)));
    // A size line in bytes that are not text.
    assert!(matches!(chunked(b"\xff\xfe\r\nab\r\n0\r\n\r\n".to_vec(), None).await.unwrap_err(), TransportError::Network(_)));
}

/// "remote hostile: a chunked answer that stops is refused when the host closes it".
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_chunked_answer_cut_short_is_refused() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let Ok((mut stream, _)) = listener.accept().await else { return };
        let mut request = vec![0u8; 8192];
        let _ = stream.read(&mut request).await;
        let head = format!("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n{}: 2\r\n\r\n10\r\nabc", protocol::HEADER);
        let _ = stream.write_all(head.as_bytes()).await;
    });
    let refused = transport::request(&loopback(port), KEY, Method::Get, "/v2/info", Request::default()).await.unwrap_err();
    assert!(matches!(refused, TransportError::Network(_)), "{refused:?}");
}

/// "remote hostile: a connect that is dropped or fails gives its address back" (the drop guard on `connecting`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_connect_that_is_dropped_or_fails_gives_its_address_back() {
    set_language("en");
    let answering = Arc::new(AtomicBool::new(false));
    let open = answering.clone();
    let host = FakeHost::with(Arc::new(move |_seen: &Seen| {
        let open = open.load(Ordering::SeqCst);
        Box::pin(async move {
            if !open {
                tokio::time::sleep(Duration::from_secs(30)).await;
            }
            Reply::fixture("info.json", 200)
        })
    }))
    .await;
    let (store, directory) = client_store();
    let seen = Arc::new(Collected::default());
    let client = client_with(&store, &directory, Arc::new(MemoryVault::default()), &seen, loopback_options());
    let address = format!("127.0.0.1:{}", host.port);
    // Dropped while the host has not answered.
    assert!(tokio::time::timeout(Duration::from_millis(300), client.connect("", &address, KEY)).await.is_err());
    answering.store(true, Ordering::SeqCst);
    let info = client.connect("", &address, KEY).await.expect("the address is free again after the dropped connect");
    // Failed: a refresh whose host answers nonsense leaves the address free for the next one.
    answering.store(false, Ordering::SeqCst);
    assert!(tokio::time::timeout(Duration::from_millis(300), client.refresh(&info.id)).await.is_err());
    answering.store(true, Ordering::SeqCst);
    client.refresh(&info.id).await.expect("the address is free again after the dropped refresh");
    let _ = std::fs::remove_dir_all(&directory);
}

/// The Mac's start sample (native/contracts/fixtures/remote-v2/start.json).
fn fixture_json_start() -> serde_json::Value {
    fixture("start.json")
}

/// "remote hostile: a host whose route panics answers 500 with what it said" (C# ServeAsync's last catch).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_host_whose_route_panics_answers_500_with_what_it_said() {
    set_language("en");
    let admission: mighty_remote::remote::host::Admission = Arc::new(|_| panic!("admission broke"));
    let fixture = make_host(MakeHost { admission: Some(admission), ..MakeHost::default() }).await;
    let state = fixture.host.start(&[fixture.shared.id().to_string()], 0).await.unwrap();
    let mut start = fixture_json_start();
    start["request"]["workspaceId"] = serde_json::json!(fixture.shared.id());
    let body = serde_json::to_vec(&start).unwrap();
    let options = Request { body: Some(&body), ..Request::default() };
    let refused = transport::request(&loopback(state.port.unwrap()), state.key.as_deref().unwrap(), Method::Post, "/v2/runs", options)
        .await
        .unwrap_err();
    assert_eq!(refused.status(), Some(500), "{refused:?}");
    assert_eq!(refused.message(), "admission broke");
    fixture.dispose().await;
}
