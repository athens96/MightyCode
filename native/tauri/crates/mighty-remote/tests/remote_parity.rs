//! The head parser's parity with C# (W8 plan D18): every raw request of native/contracts/fixtures/remote-heads/heads.jsonl sent on its own
//! loopback connection to the strict server, whose handler answers 200 with "len:<body bytes>", answered exactly as C#'s
//! RemoteHttpListener answered it (heads-csharp.txt, written by `tools/parity` `remote heads.jsonl heads-csharp.txt`): the status line,
//! or "closed" / "pending" (nothing within 3 s), and the body's bytes. A case with "limit": "host" goes to a listener with the sharing
//! host's own body limit (none without the key, the larger one for a run).

use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use mighty_platform::fake::FakeNetwork;
use mighty_remote::remote::host::body_limit;
use mighty_remote::remote::http_server::{BodyLimit, Handler, HttpListener, HttpRequest, HttpResponse, ListenerOptions};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// The key the host-limit cases present (tools/parity/Remote.cs's Key).
const KEY: &str = "parity-remote-key-0123456789abcdefghijklmnop";

fn folder() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../contracts/fixtures/remote-heads")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Vec<u8> {
    (0..text.len()).step_by(2).map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap()).collect()
}

/// `Exchange`: the bytes sent, then everything read until the server closes (or 3 s pass with nothing).
async fn exchange(port: u16, raw: &[u8]) -> String {
    let mut stream = tokio::net::TcpStream::connect((Ipv4Addr::LOCALHOST, port)).await.unwrap();
    let _ = stream.write_all(raw).await;
    let mut received = vec![];
    let mut buffer = vec![0u8; 65536];
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        match tokio::time::timeout_at(deadline, stream.read(&mut buffer)).await {
            Ok(Ok(0)) | Ok(Err(_)) => break,
            Ok(Ok(read)) => received.extend_from_slice(&buffer[..read]),
            Err(_) if received.is_empty() => return "pending\t".into(),
            Err(_) => break,
        }
    }
    if received.is_empty() {
        return "closed\t".into();
    }
    let head_end = received.iter().position(|b| *b == b'\r').unwrap_or(received.len());
    let head = String::from_utf8_lossy(&received[..head_end]).into_owned();
    let body = received.windows(4).position(|w| w == b"\r\n\r\n").map(|at| hex(&received[at + 4..])).unwrap_or_default();
    format!("{head}\t{body}")
}

/// "remote parity: the strict server answers every head as C#'s RemoteHttpListener does".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_strict_server_answers_every_head_as_csharps_listener_does() {
    let handler: Handler = Arc::new(|request| {
        Box::pin(async move { Ok(HttpResponse { status: 200, body: format!("len:{}", request.body.len()).into_bytes(), headers: vec![] }) })
    });
    let network = Arc::new(FakeNetwork::default());
    let listener = HttpListener::new(network.clone(), IpAddr::V4(Ipv4Addr::LOCALHOST), 0, handler.clone(), ListenerOptions::default());
    // The host-limit cases: the sharing host's own body limit with KEY, loopback allowed (C#'s RemoteHostService.BodyLimit).
    let limit: BodyLimit = Arc::new(|head: &HttpRequest| Ok(body_limit(head, KEY, true)));
    let hosted = HttpListener::new(
        network,
        IpAddr::V4(Ipv4Addr::LOCALHOST),
        0,
        handler,
        ListenerOptions { body_limit: Some(limit), ..ListenerOptions::default() },
    );
    let port = listener.start().unwrap();
    let host_port = hosted.start().unwrap();
    let corpus = std::fs::read_to_string(folder().join("heads.jsonl")).unwrap();
    let expected = std::fs::read_to_string(folder().join("heads-csharp.txt")).unwrap();
    let mut answers = vec![];
    for line in corpus.lines().filter(|l| !l.is_empty()) {
        let case: Value = serde_json::from_str(line).unwrap();
        let name = case["name"].as_str().unwrap();
        let target = if case["limit"].as_str() == Some("host") { host_port } else { port };
        answers.push(format!("{name}\t{}", exchange(target, &unhex(case["hex"].as_str().unwrap())).await));
    }
    listener.dispose().await;
    hosted.dispose().await;
    let expected: Vec<&str> = expected.lines().collect();
    assert_eq!(answers.len(), expected.len(), "one answer a case");
    let differing: Vec<String> = answers
        .iter()
        .zip(&expected)
        .filter(|(rust, cs)| rust.as_str() != **cs)
        .map(|(rust, cs)| format!("rust {rust}\n  c#  {cs}"))
        .collect();
    assert!(differing.is_empty(), "the answers C# gives:\n{}", differing.join("\n"));
}
