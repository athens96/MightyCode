//! Push notifications, held to the Mac's shared fixture (`native/contracts/fixtures/push.json`), ported by C# name (W8-A;
//! MightyCode.Core.Tests `MobilePushVerification`, L438-L454). The hub's checks drive it with a recording transport (no push server is
//! ever reached); the redirect check runs the WinHTTP POST against two loopback listeners on Windows only. Where C# goes through the
//! router's push routes (L446, L447), the check here keeps the hub's and the identity's half: the routes' statuses and words are W8-B's
//! router's.

mod support;

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use mighty_core::model::{AgentActivity, LogEntry, RemoteWorkspaceReference, RunSession, Workspace};
use mighty_core::permission::ToolPermissionRequest;
use mighty_platform::{BoxFuture, PlatformError};
use mighty_remote::identity::{AuthRequest, Identity, PushDevice, PushRegistrationResult, hash};
use mighty_remote::push::hub::{PushHub, PushSeams, PushSource};
use mighty_remote::push::message::{self, CardAction, PushCard, PushMessage, PushPermission, PushRequest};
use mighty_remote::push::planner::{self, Planner, PushPane};
use mighty_remote::push::recipients::{self, Viewing};
use mighty_remote::push::seal;
use mighty_remote::push::transport::{self, PushOutcome, PushSendRequest, PushStatus, PushTransport};
use mighty_run::antigravity::permissions::{DeniedAction, PermissionRequest};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use support::fake_relay::FileStore;

fn fixture() -> Value {
    let text = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../contracts/fixtures/push.json")).unwrap();
    serde_json::from_str(&text).unwrap()
}

fn s<'a>(node: &'a Value, key: &str) -> &'a str {
    node[key].as_str().unwrap()
}

fn temp(name: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let path = std::env::temp_dir().join(format!("mighty-push-{name}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn permission_of(value: &Value) -> PushPermission {
    PushPermission { request_id: s(value, "requestId").into(), run_id: s(value, "runId").into(), buttons: s(value, "buttons").into() }
}

/// A message as the fixture writes it (the Mac's `JSONDecoder`).
fn message_of(value: &Value) -> PushMessage {
    PushMessage {
        v: value["v"].as_i64().unwrap() as i32,
        id: s(value, "id").into(),
        kind: s(value, "kind").into(),
        host_id: s(value, "hostId").into(),
        host_name: s(value, "hostName").into(),
        workspace: s(value, "workspace").into(),
        title: s(value, "title").into(),
        summary: s(value, "summary").into(),
        session_id: s(value, "sessionId").into(),
        permission: value.get("permission").map(permission_of),
        at: s(value, "at").into(),
    }
}

fn pane_of(value: &Value) -> PushPane {
    PushPane {
        session_id: s(value, "sessionId").into(),
        workspace: s(value, "workspace").into(),
        title: s(value, "title").into(),
        status: s(value, "status").into(),
        preview: value.get("preview").and_then(Value::as_str).map(str::to_string),
        requests: value["requests"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| PushRequest::new(s(r, "id"), s(r, "runId"), s(r, "kind"), s(r, "summary"), s(r, "buttons")))
            .collect(),
        sign_in_lost: value["signInLost"].as_bool().unwrap_or(false),
    }
}

fn pane(session: &str, status: &str, requests: &[(&str, &str)]) -> PushPane {
    PushPane::new(
        session,
        "demo",
        "Claude 1",
        status,
        None,
        requests.iter().map(|(id, run)| PushRequest::new(id, run, message::PERMISSION, "", message::BUTTONS_CLAUDE)).collect(),
    )
}

fn simple_message(kind: &str, session: &str) -> PushMessage {
    PushMessage {
        v: 1,
        id: "x".into(),
        kind: kind.into(),
        host_id: "h".into(),
        host_name: "H".into(),
        workspace: "w".into(),
        title: "t".into(),
        summary: String::new(),
        session_id: session.into(),
        permission: None,
        at: "t".into(),
    }
}

/// L438 "mobile push: the seal follows the shared fixture".
#[test]
fn mobile_push_the_seal_follows_the_shared_fixture() {
    let root = fixture();
    let fx = &root["seal"];
    let b64 = base64::engine::general_purpose::STANDARD;
    let key = seal::key(s(fx, "tokenHash"), s(fx, "clientId")).expect("no key");
    assert_eq!(b64.encode(key.as_ref()), s(fx, "keyB64"), "the key is the Mac's");
    assert_eq!(hash(s(fx, "deviceToken")), s(fx, "tokenHash"), "the token hash is SHA-256 of the token");
    let nonce: [u8; 12] = b64.decode(s(fx, "nonceB64")).unwrap().try_into().unwrap();
    assert_eq!(
        seal::seal(s(fx, "plaintext").as_bytes(), &key, s(fx, "handle"), Some(nonce)).unwrap(),
        s(fx, "envelope"),
        "the envelope is the Mac's"
    );
    assert_eq!(seal::open(s(fx, "envelope"), &key, s(fx, "handle")).unwrap(), s(fx, "plaintext").as_bytes(), "the envelope opens");
    for tampered in fx["tampered"].as_array().unwrap() {
        assert!(seal::open(tampered.as_str().unwrap(), &key, s(fx, "handle")).is_none(), "a tampered envelope does not open");
    }
    assert!(seal::open(s(fx, "envelope"), &key, s(fx, "otherHandle")).is_none(), "another handle does not open it");
    assert!(seal::open(&"A".repeat(3073), &key, s(fx, "handle")).is_none(), "an envelope over 3072 characters does not open");
    assert!(seal::key("not hex", s(fx, "clientId")).is_none(), "a bad hash makes no key");
    assert!(seal::key(&format!("+f{}", &s(fx, "tokenHash")[2..]), s(fx, "clientId")).is_none(), "a signed digit is not hex");
    assert!(
        seal::key(&s(fx, "tokenHash").to_uppercase(), s(fx, "clientId")).is_some_and(|upper| *upper == *key),
        "uppercase hex makes the same key"
    );
    assert_eq!(
        seal::seal(&[0u8; 2049], &key, s(fx, "handle"), None),
        Err(seal::SealError::TooLong),
        "a message over 2048 bytes is not sealed"
    );
    assert!(
        seal::seal(&[0u8; 2048], &key, s(fx, "handle"), None).unwrap().len() <= seal::MAXIMUM_ENVELOPE,
        "the longest message fits the envelope limit"
    );
    // The fixture's plaintext is what the Mac encodes for its message: the same bytes here.
    let message = message_of(&serde_json::from_str(s(fx, "plaintext")).unwrap());
    assert_eq!(message.encoded_for_push().unwrap(), s(fx, "plaintext").as_bytes(), "the plaintext is encoded as the Mac encodes it");
}

/// L439 "mobile push: the planner follows the shared steps".
#[test]
fn mobile_push_the_planner_follows_the_shared_steps() {
    let root = fixture();
    let fx = &root["planner"];
    let mut subject = Planner::new();
    for step in fx["steps"].as_array().unwrap() {
        let note = s(step, "note");
        let panes: Vec<PushPane> = step["panes"].as_array().unwrap().iter().map(pane_of).collect();
        let sent = subject.observe(&panes, s(fx, "hostId"), s(fx, "hostName"), s(fx, "at"));
        let expected = step["expect"].as_array().unwrap();
        assert_eq!(sent.len(), expected.len(), "{note}: count");
        for (message, want) in sent.iter().zip(expected) {
            assert!(
                message.v == 1 && message.host_id == s(fx, "hostId") && message.host_name == s(fx, "hostName") && message.at == s(fx, "at"),
                "{note}: constants"
            );
            assert!(
                message.id == s(want, "id") && message.kind == s(want, "kind") && message.session_id == s(want, "sessionId"),
                "{note}: id {}",
                message.id
            );
            assert!(
                message.workspace == s(want, "workspace") && message.title == s(want, "title") && message.summary == s(want, "summary"),
                "{note}: text"
            );
            assert_eq!(message.permission, want.get("permission").map(permission_of), "{note}: permission");
            let encoded = message.encoded_for_push().unwrap_or_else(|| panic!("{note}: does not encode"));
            assert!(encoded.len() <= seal::MAXIMUM_PLAINTEXT, "{note}: encoded {}", encoded.len());
            let pushed: Value = serde_json::from_slice(&encoded).unwrap();
            let summary = want.get("pushSummary").and_then(Value::as_str).unwrap_or(s(want, "summary"));
            assert!(
                s(&pushed, "summary") == summary && s(&pushed, "id") == s(want, "id") && s(&pushed, "title") == s(want, "title"),
                "{note}: pushed"
            );
        }
    }
}

/// L440 "mobile push: a message is encoded as the Mac's JSONEncoder writes it" (its length checked against Swift's own, 227).
#[test]
fn mobile_push_a_message_is_encoded_as_the_macs_jsonencoder_writes_it() {
    let message = PushMessage {
        v: 1,
        id: "i".into(),
        kind: message::PERMISSION.into(),
        host_id: "h".into(),
        host_name: "H".into(),
        workspace: "a/b".into(),
        title: "\u{1}\n\"\\\t\u{8}\u{c}\r".into(),
        summary: "\u{1F600}\u{7f}\u{2028}가<&".into(),
        session_id: "s".into(),
        permission: Some(PushPermission { request_id: "r".into(), run_id: "u".into(), buttons: message::BUTTONS_CLAUDE.into() }),
        at: "t".into(),
    };
    // `~` stands for a backslash.
    let expected = concat!(
        r#"{"v":1,"id":"i","kind":"permission","hostId":"h","hostName":"H","workspace":"a~/b","title":"~u0001~n~"~~~t~b~f~r","#,
        r#""summary":"SUMMARY","sessionId":"s","permission":{"requestId":"r","runId":"u","buttons":"claude"},"at":"t"}"#
    )
    .replace('~', "\\")
    .replace("SUMMARY", "\u{1F600}\u{7f}\u{2028}가<&");
    let encoded = message.encoded_for_push().expect("does not encode");
    assert_eq!(String::from_utf8(encoded.clone()).unwrap(), expected, "the Mac's escaping");
    assert_eq!(encoded.len(), 227, "Swift's length");
    let plain = PushMessage {
        kind: message::COMPLETED.into(),
        workspace: "w".into(),
        title: "t".into(),
        summary: String::new(),
        permission: None,
        ..message
    };
    assert_eq!(
        String::from_utf8(plain.encoded_for_push().unwrap()).unwrap(),
        r#"{"v":1,"id":"i","kind":"completed","hostId":"h","hostName":"H","workspace":"w","title":"t","summary":"","sessionId":"s","at":"t"}"#,
        "no permission, no key"
    );
    // Over 2048 bytes the summary goes first; still over, the push is skipped.
    let long = PushMessage { summary: "x".repeat(3000), ..plain.clone() };
    assert!(long.encoded_for_push().is_some_and(|data| serde_json::from_slice::<Value>(&data).unwrap()["summary"] == ""));
    assert!(PushMessage { title: "x".repeat(3000), ..plain }.encoded_for_push().is_none());
}

/// L441 "mobile push: requests take their buttons from the card".
#[test]
fn mobile_push_requests_take_their_buttons_from_the_card() {
    let card = |tool: &str, can_allow: bool, plan: Option<&str>, agy: Option<Vec<CardAction>>| PushCard {
        id: "r".into(),
        run_id: "u".into(),
        tool_name: tool.into(),
        summary: "s".into(),
        can_allow,
        questionnaire: false,
        plan: plan.map(str::to_string),
        antigravity: agy,
    };
    assert_eq!(
        PushRequest::from_card(&card("Bash", true, None, None)).buttons,
        message::BUTTONS_CLAUDE,
        "a tool permission gets allow and deny"
    );
    assert_eq!(
        PushRequest::from_card(&card("Bash", false, None, None)).buttons,
        message::BUTTONS_NONE,
        "a deny-only request gets no buttons"
    );
    assert_eq!(PushRequest::from_card(&card("AskUserQuestion", true, None, None)).kind, message::QUESTION, "a question");
    assert_eq!(PushRequest::from_card(&card("ExitPlanMode", true, Some("p"), None)).kind, message::PLAN, "a plan");
    let once = vec![CardAction { rule: Some("command(rm x)".into()), once_only: true }];
    let always = vec![CardAction { rule: Some("command(ls)".into()), once_only: false }];
    let ruleless = vec![CardAction { rule: None, once_only: false }];
    assert_eq!(
        PushRequest::from_card(&card(message::ANTIGRAVITY_TOOL, true, None, Some(once.clone()))).buttons,
        message::BUTTONS_ANTIGRAVITY_ONCE,
        "once only"
    );
    assert_eq!(
        PushRequest::from_card(&card(message::ANTIGRAVITY_TOOL, true, None, Some(always))).buttons,
        message::BUTTONS_ANTIGRAVITY,
        "once or always"
    );
    assert_eq!(
        PushRequest::from_card(&card(message::ANTIGRAVITY_TOOL, false, None, Some(ruleless))).buttons,
        message::BUTTONS_NONE,
        "no rule, no buttons"
    );
    assert_eq!(
        PushRequest::from_card(&card(message::ANTIGRAVITY_TOOL, true, None, Some(once))).kind,
        message::PERMISSION,
        "an Antigravity card is a permission"
    );
    let request = |tool: &str, input: &str, can_answer: bool| ToolPermissionRequest {
        can_answer_questions: can_answer && tool == "AskUserQuestion",
        can_answer_plan: can_answer && tool == "ExitPlanMode",
        ..ToolPermissionRequest::new("r", "u", "t", tool, input, tool)
    };
    let options = r#"[{"label":"A","description":""},{"label":"B","description":""}]"#;
    let two = format!(
        r#"{{"questions":[{{"header":"h","question":"어느 쪽?","multiSelect":false,"options":{options}}},{{"header":"g","question":"언제?","multiSelect":false,"options":{options}}}]}}"#
    );
    assert_eq!(
        PushRequest::from_tool_request(&request("AskUserQuestion", &two, true)).summary,
        "어느 쪽? · 언제?",
        "a question reads as its questions"
    );
    assert_eq!(
        PushRequest::from_tool_request(&request("AskUserQuestion", &two, false)).summary,
        "어느 쪽? · 언제?",
        "even one the phone cannot answer"
    );
    assert_eq!(
        PushRequest::from_tool_request(&request("AskUserQuestion", "{}", true)).summary,
        "",
        "an unreadable question leaves the phone's sentence"
    );
    let plan = json!({"plan": "\n## 로그인 고치기\n- 1단계"}).to_string();
    assert_eq!(
        PushRequest::from_tool_request(&request("ExitPlanMode", &plan, true)).summary,
        "로그인 고치기",
        "a plan reads as its first line"
    );
    assert_eq!(
        PushRequest::from_tool_request(&request("ExitPlanMode", "{}", true)).summary,
        "",
        "an unreadable plan leaves the phone's sentence"
    );
    assert_eq!(
        PushRequest::from_tool_request(&request("Bash", r#"{"command":"ls"}"#, true)).summary,
        "Bash",
        "a tool permission keeps its summary"
    );
}

/// L442 "mobile push: starting over replays nothing and keeps counting".
#[test]
fn mobile_push_starting_over_replays_nothing_and_keeps_counting() {
    let mut planner = Planner::new();
    let running = PushPane::new("p", "w", "t", "running", None, vec![]);
    planner.observe(std::slice::from_ref(&running), "h", "n", "a");
    let ids = |messages: Vec<PushMessage>| messages.into_iter().map(|m| m.id).collect::<Vec<_>>();
    let completed = PushPane { status: "completed".into(), ..running.clone() };
    assert_eq!(ids(planner.observe(std::slice::from_ref(&completed), "h", "n", "a")), vec!["run:p:1:completed"], "the first completion");
    planner.reset();
    let asking = PushPane { requests: vec![PushRequest::new("x", "r", message::PERMISSION, "", message::BUTTONS_CLAUDE)], ..running };
    assert!(planner.observe(&[asking], "h", "n", "a").is_empty(), "the first observation after a reset is the baseline");
    assert_eq!(ids(planner.observe(&[completed], "h", "n", "a")), vec!["run:p:2:completed"], "the count goes on");
}

/// L443 "mobile push: printing shows no handle, token hash or content".
#[test]
fn mobile_push_printing_shows_no_handle_token_hash_or_content() {
    let device = PushDevice {
        device_id: "a".into(),
        handle: "handle-secret".into(),
        token_hash: "f".repeat(64),
        kinds: vec![message::PERMISSION.into()],
    };
    let message = PushMessage {
        id: "request:u:r".into(),
        host_name: "host-secret".into(),
        workspace: "workspace-secret".into(),
        title: "title-secret".into(),
        summary: "summary-secret".into(),
        ..simple_message(message::PERMISSION, "pane-1")
    };
    let send = PushSendRequest { server_id: "server-1".into(), handle: "handle-secret".into(), payload: "envelope-secret".into() };
    for text in [format!("{device:?}"), format!("{:?}", vec![device.clone()])] {
        assert!(!text.contains("handle-secret") && !text.contains(&device.token_hash) && text.contains('a'), "device: {text}");
    }
    let shown = format!("{message:?}");
    assert!(
        !["host-secret", "workspace-secret", "title-secret", "summary-secret"].iter().any(|secret| shown.contains(secret))
            && shown.contains("request:u:r"),
        "message: {shown}"
    );
    let shown = format!("{send:?}");
    assert!(!shown.contains("handle-secret") && !shown.contains("envelope-secret"), "send: {shown}");
}

/// L444 "mobile push: recipients, the relay address and its answers".
#[test]
fn mobile_push_recipients_the_relay_address_and_its_answers() {
    let message = simple_message(message::COMPLETED, "pane-1");
    let all = PushDevice {
        device_id: "a".into(),
        handle: "ha".into(),
        token_hash: "0".repeat(64),
        kinds: message::KINDS.map(String::from).to_vec(),
    };
    let quiet =
        PushDevice { device_id: "b".into(), handle: "hb".into(), token_hash: "0".repeat(64), kinds: vec![message::PERMISSION.into()] };
    let now = 1_000_000i64;
    let view = |session: &str, until: i64| HashMap::from([("a".to_string(), Viewing { session_id: session.into(), until_ms: until })]);
    let ids = |devices: Vec<&PushDevice>| devices.into_iter().map(|d| d.device_id.clone()).collect::<Vec<_>>();
    let both = [all.clone(), quiet];
    assert_eq!(ids(recipients::targets(&message, &both, &HashMap::new(), now)), vec!["a"], "only the kinds a phone turned on");
    let alone = [all];
    assert!(recipients::targets(&message, &alone, &view("pane-1", now + 10_000), now).is_empty(), "not the session it is viewing");
    assert_eq!(
        recipients::targets(&message, &alone, &view("pane-2", now + 10_000), now).len(),
        1,
        "another session it is viewing does not hold"
    );
    assert_eq!(recipients::targets(&message, &alone, &view("pane-1", now - 1000), now).len(), 1, "an expired view does not hold");
    assert!(
        recipients::valid_handle(&"A".repeat(43))
            && !recipients::valid_handle(&"A".repeat(42))
            && !recipients::valid_handle(&format!("{}=", "A".repeat(42))),
        "a handle is 43 base64url characters"
    );
    assert_eq!(recipients::kinds(&["signin", "later", "completed"]), vec!["completed", "signin"], "known kinds in the host's order");
    assert_eq!(
        transport::send_uri("wss://relay.example.invalid").as_deref(),
        Some("https://relay.example.invalid/push/v1/send"),
        "wss → https"
    );
    assert_eq!(transport::send_uri("ws://127.0.0.1:8787").as_deref(), Some("http://127.0.0.1:8787/push/v1/send"), "ws → http");
    assert_eq!(
        transport::send_uri("wss://relay.example.invalid/ws?x=1").as_deref(),
        Some("https://relay.example.invalid/push/v1/send"),
        "the path and query are replaced"
    );
    assert_eq!(transport::send_uri("ftp://relay.example.invalid"), None, "other schemes are refused");
    assert_eq!(PushOutcome::from_status(202, None).status, PushStatus::Sent, "202");
    assert!([403, 404, 410].iter().all(|code| PushOutcome::from_status(*code, None).status == PushStatus::Gone), "gone");
    assert_eq!(
        PushOutcome::from_status(429, Some(Duration::from_secs(5))),
        PushOutcome { status: PushStatus::Retry, retry_after: Some(Duration::from_secs(5)), code: 429 },
        "429 retries"
    );
    assert_eq!(PushOutcome::from_status(502, None).status, PushStatus::Retry, "502 retries");
    assert_eq!(PushOutcome::from_status(503, None).status, PushStatus::Failed, "503 is not retried");
    assert_eq!(PushOutcome::from_status(401, None), PushOutcome { status: PushStatus::Failed, retry_after: None, code: 401 }, "401 fails");
    assert!(
        transport::retry_after(Some("5")) == Some(Duration::from_secs(5))
            && transport::retry_after(Some(" 12 ")) == Some(Duration::from_secs(12)),
        "seconds"
    );
    assert_eq!(transport::retry_after(Some("999999")), Some(Duration::from_secs(3600)), "at most an hour");
    for bad in [None, Some(""), Some("-1"), Some("nan"), Some("inf"), Some("Wed, 21 Oct 2026 07:28:00 GMT")] {
        assert_eq!(transport::retry_after(bad), None, "not seconds: {bad:?}");
    }
    let wide = "가".repeat(80);
    let mut subject = Planner::new();
    subject.observe(&[], "h", "H", "t");
    let request = PushRequest::new("r", "u", message::PERMISSION, &"\u{1F600}".repeat(300), message::BUTTONS_CLAUDE);
    let sent = subject.observe(&[PushPane::new("p", &wide, &wide, "running", None, vec![request])], "h", &wide, "t");
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].summary.chars().count(), 200, "the summary is clipped to 200 scalars");
    let fits = sent[0].encoded_for_push().unwrap();
    assert!(
        fits.len() <= seal::MAXIMUM_PLAINTEXT && serde_json::from_slice::<Value>(&fits).unwrap()["summary"] == sent[0].summary.as_str(),
        "a wide message fits with its summary"
    );
}

/// L445 "mobile push: a relay redirect is a failure and is never followed": the decision, on any OS.
#[test]
fn mobile_push_a_relay_redirect_is_a_failure() {
    for code in [301, 302, 303, 307, 308] {
        assert_eq!(PushOutcome::from_status(code, None), PushOutcome { status: PushStatus::Failed, retry_after: None, code }, "{code}");
    }
}

/// L445 "mobile push: a relay redirect is a failure and is never followed": the platform's POST (WinHTTP; the Mac's hyper over the
/// system proxy, M-C) against a loopback relay answering 307 to a second loopback listener, which is never reached.
#[cfg(any(windows, target_os = "macos"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mobile_push_a_relay_redirect_is_a_failure_and_is_never_followed() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let elsewhere = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let redirecting = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let elsewhere_port = elsewhere.local_addr().unwrap().port();
    let port = redirecting.local_addr().unwrap().port();
    let hits = Arc::new(AtomicUsize::new(0));
    let counted = hits.clone();
    tokio::spawn(async move {
        if elsewhere.accept().await.is_ok() {
            counted.fetch_add(1, Ordering::SeqCst);
        }
    });
    let answer = tokio::spawn(async move {
        let (mut client, _) = redirecting.accept().await.unwrap();
        let mut read = Vec::new();
        let mut buffer = [0u8; 16_384];
        // The whole request, its body too: a socket closed with bytes it never read is reset, and the reset can reach the client before
        // the reply does (it then reads an error, not the 307).
        let complete = |read: &[u8]| {
            let Some(end) = read.windows(4).position(|w| w == b"\r\n\r\n") else { return false };
            let head = String::from_utf8_lossy(&read[..end]).to_ascii_lowercase();
            let length = head.lines().find_map(|line| line.strip_prefix("content-length:")).and_then(|v| v.trim().parse().ok());
            read.len() >= end + 4 + length.unwrap_or(0)
        };
        while !complete(&read) {
            let count = client.read(&mut buffer).await.unwrap();
            if count == 0 {
                break;
            }
            read.extend_from_slice(&buffer[..count]);
        }
        let reply = format!(
            "HTTP/1.1 307 Temporary Redirect\r\nLocation: http://127.0.0.1:{elsewhere_port}/push/v1/send\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        );
        client.write_all(reply.as_bytes()).await.unwrap();
        // Closed gracefully: the client reads the reply to its end before the connection goes.
        let _ = client.shutdown().await;
        let _ =
            tokio::time::timeout(Duration::from_secs(5), async { while client.read(&mut buffer).await.is_ok_and(|count| count > 0) {} })
                .await;
        String::from_utf8_lossy(&read).into_owned()
    });
    #[cfg(windows)]
    let sender = transport::HttpPushTransport::new(Arc::new(mighty_platform_windows::http::WinHttpGet));
    #[cfg(target_os = "macos")]
    let sender = transport::HttpPushTransport::new(Arc::new(mighty_platform_macos::http::MacHttps::new()));
    let request = PushSendRequest { server_id: "s".into(), handle: "h".into(), payload: "p".into() };
    let outcome = sender.send(request, format!("http://127.0.0.1:{port}/push/v1/send"), "t".into()).await.unwrap();
    let head = answer.await.unwrap();
    assert_eq!(outcome, PushOutcome { status: PushStatus::Failed, retry_after: None, code: 307 }, "a redirect fails: {outcome:?}");
    assert!(
        head.starts_with("POST /push/v1/send HTTP/1.1\r\n")
            && head.contains("Authorization: Bearer t\r\n")
            && head.contains("Content-Type: application/json"),
        "{head}"
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(hits.load(Ordering::SeqCst), 0, "the redirect is not followed");
}

/// A host's identity with one phone paired the ordinary way (C#'s `PairedHost`): the identity, the phone's id and its token.
fn paired(directory: &Path) -> (Arc<Identity>, String, String) {
    let identity = Arc::new(Identity::load_or_create(Arc::new(FileStore(directory.to_path_buf())), true).unwrap());
    let client = "AAECAwQFBgcICQoLDA0ODw".to_string();
    let key = identity.pairing_key().to_string();
    let request = AuthRequest::from_json(&json!({"type":"auth","pairingKey":key,"clientId":client,"clientName":"Phone"}));
    let token = identity.authorize(&request, false).unwrap().device_token().expect("no token").to_string();
    (identity, client, token)
}

type Answer = Arc<dyn Fn(&PushSendRequest) -> BoxFuture<'static, mighty_platform::Result<PushOutcome>> + Send + Sync>;

/// The checks' transport (C#'s `RecordingTransport`): each send recorded, then answered by `answer` when set, else the next queued
/// outcome (none: Sent).
#[derive(Default)]
struct Recording {
    sent: Mutex<Vec<(PushSendRequest, String, String)>>,
    outcomes: Mutex<VecDeque<PushOutcome>>,
    answer: Mutex<Option<Answer>>,
}

impl Recording {
    fn count(&self) -> usize {
        self.sent.lock().unwrap().len()
    }
    fn queue(&self, outcome: PushOutcome) {
        self.outcomes.lock().unwrap().push_back(outcome);
    }
    fn answer(&self, answer: Option<Answer>) {
        *self.answer.lock().unwrap() = answer;
    }
}

impl PushTransport for Recording {
    fn send(&self, request: PushSendRequest, url: String, host_token: String) -> BoxFuture<'_, mighty_platform::Result<PushOutcome>> {
        self.sent.lock().unwrap().push((request.clone(), url, host_token));
        let answer = self.answer.lock().unwrap().clone();
        if let Some(answer) = answer {
            return answer(&request);
        }
        let next = self.outcomes.lock().unwrap().pop_front().unwrap_or(PushOutcome::new(PushStatus::Sent));
        Box::pin(async move { Ok(next) })
    }
}

struct Seams {
    transport: Arc<Recording>,
    on: Arc<AtomicBool>,
    waits: Arc<Mutex<Vec<Duration>>>,
    clock: Arc<Mutex<i64>>,
}

impl Seams {
    fn waits(&self) -> Vec<Duration> {
        self.waits.lock().unwrap().clone()
    }
}

/// A hub over `identity` with the checks' seams: the recording transport, `retry` delays, the relay `wss://relay.example.invalid`, a
/// switch, waits recorded (none slept) and a clock at 1000 s.
fn hub(identity: &Arc<Identity>, retry: Vec<Duration>) -> (Arc<PushHub>, Seams) {
    let seams = Seams {
        transport: Arc::new(Recording::default()),
        on: Arc::new(AtomicBool::new(true)),
        waits: Arc::new(Mutex::new(vec![])),
        clock: Arc::new(Mutex::new(1_000_000)),
    };
    let (on, waits, clock) = (seams.on.clone(), seams.waits.clone(), seams.clock.clone());
    let push = PushSeams {
        transport: seams.transport.clone(),
        retry_delays: retry,
        wait: Arc::new(move |wait| {
            waits.lock().unwrap().push(wait);
            Box::pin(async {})
        }),
        clock: Arc::new(move || *clock.lock().unwrap()),
        relay: Arc::new(|| "wss://relay.example.invalid".to_string()),
        remote_on: Arc::new(move || on.load(Ordering::SeqCst)),
    };
    (PushHub::new(identity.clone(), "Test PC", push), seams)
}

async fn settle(handles: Vec<tokio::task::JoinHandle<()>>) {
    for handle in handles {
        handle.await.unwrap();
    }
}

fn key_of(token: &str, client: &str) -> zeroize::Zeroizing<[u8; 32]> {
    seal::key(&hash(token), client).unwrap()
}

/// The pushed messages' ids since `from`, opened as the phone opens them.
fn sent_ids(transport: &Recording, from: usize, token: &str, client: &str) -> Vec<String> {
    let key = key_of(token, client);
    transport.sent.lock().unwrap()[from..]
        .iter()
        .map(|(request, _, _)| {
            let plain = seal::open(&request.payload, &key, &request.handle).unwrap();
            serde_json::from_slice::<Value>(&plain).unwrap()["id"].as_str().unwrap().to_string()
        })
        .collect()
}

/// L446 "mobile push: a phone registers a handle and a view" (the hub's half; the routes' statuses are W8-B's router's).
#[tokio::test]
async fn mobile_push_a_phone_registers_a_handle_and_a_view() {
    let directory = temp("register");
    let (identity, client, token) = paired(&directory);
    let (push, _seams) = hub(&identity, vec![]);
    let handle = "h".repeat(43);
    let kinds = recipients::kinds(&["permission", "completed", "future-kind"]);
    assert_eq!(push.register(&client, &handle, &kinds), PushRegistrationResult::Saved, "a phone with a token registers");
    let devices = identity.push_devices();
    let wanted: HashSet<String> = HashSet::from(["permission".to_string(), "completed".to_string()]);
    assert!(
        devices.len() == 1 && devices[0].handle == handle && devices[0].kinds.iter().cloned().collect::<HashSet<_>>() == wanted,
        "the handle and the known kinds are kept"
    );
    let digest: String = Sha256::digest(token.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(devices[0].token_hash, digest, "the key material is the token hash");
    assert_eq!(push.register("legacy", &handle, &[]), PushRegistrationResult::NeedsToken, "a phone without a token is refused");
    push.viewing(&client, None);
    assert!(push.views().is_empty(), "null clears the view");
    assert!(push.unregister(&client) && identity.push_devices().is_empty(), "unregister forgets the handle");
    assert!(mighty_remote::phone::router::CAPABILITIES.contains(&"push"), "the host advertises push");
    push.dispose();
    let _ = std::fs::remove_dir_all(&directory);
}

struct NoRoutes;

impl mighty_remote::relay::connection::RequestHandler for NoRoutes {
    fn handle(
        &self,
        _: mighty_remote::relay::connection::MobileDevice,
        _: String,
        _: String,
        _: Option<Box<serde_json::value::RawValue>>,
    ) -> BoxFuture<'static, mighty_remote::relay::connection::MobileReply> {
        Box::pin(async { mighty_remote::relay::connection::MobileReply::new(404, "{}".into()) })
    }
}

/// L447 "mobile push: registration follows the Mac's rules" (the hub's and the identity's half).
#[tokio::test]
async fn mobile_push_registration_follows_the_macs_rules() {
    let directory = temp("rules");
    let (identity, client, _) = paired(&directory);
    let (push, seams) = hub(&identity, vec![]);
    *seams.clock.lock().unwrap() = 1_792_713_600_000;
    let (handle, newer) = ("h".repeat(43), "n".repeat(43));
    assert_eq!(push.register("ZZZZZZZZZZZZZZZZZZZZZZ", &handle, &[]), PushRegistrationResult::NeedsToken, "an unknown phone is refused");
    assert!(!recipients::valid_handle(&format!("{handle}\n")), "a trailing newline is not a handle");
    assert_eq!(push.register(&client, &handle, &["completed".into()]), PushRegistrationResult::Saved, "registered");
    assert!(
        identity.clear_push(&client, Some(&newer)) && identity.push_devices()[0].handle == handle,
        "a gone handle that is not the stored one clears nothing"
    );
    push.viewing(&client, Some("pane-1".into()));
    assert_eq!(
        push.views().get(&client),
        Some(&Viewing { session_id: "pane-1".into(), until_ms: 1_792_713_600_000 + 45_000 }),
        "the view holds 45 s from the clock"
    );
    push.viewing(&client, None);
    assert!(push.views().is_empty(), "a body without sessionId clears the view");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        push.viewing(&client, Some("pane-1".into()));
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o500)).unwrap();
        let failed = push.unregister(&client);
        let kept = identity.push_devices().first().map(|d| d.handle.clone()) == Some(handle.clone()) && push.views().contains_key(&client);
        let unsaved = push.register(&client, &newer, &["completed".into()]);
        let old = identity.push_devices().first().map(|d| d.handle.clone()) == Some(handle.clone());
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(!failed && kept, "an unregister that cannot be saved is reported, and the handle and the view are kept");
        assert!(
            unsaved == PushRegistrationResult::Unsaved && old,
            "a register that cannot be saved is reported, and the old handle is kept"
        );
    }
    assert!(
        push.unregister(&client) && identity.push_devices().is_empty() && push.views().is_empty(),
        "unregister forgets the handle and the view"
    );
    assert!(push.unregister(&client), "nothing left to clear is not a failure");
    assert_eq!(push.register(&client, &handle, &["completed".into()]), PushRegistrationResult::Saved, "registered again");
    push.viewing(&client, Some("pane-1".into()));
    assert_eq!(push.views().len(), 1, "a view again");
    let platform = mighty_platform::fake::platform(&directory, mighty_platform::Os::Windows);
    let host = mighty_remote::relay::host::RelayHost::new(identity.clone(), Default::default(), Arc::new(NoRoutes), platform);
    assert!(host.rotate(Some(client.clone())).await.unwrap());
    assert!(push.views().is_empty() && identity.push_devices().is_empty(), "a revoke forgets the device's view and handle");
    host.dispose().await;
    // Reading the file drops a handle the row cannot use and keeps the phone.
    let other = temp("rules-file");
    let (again, phone, _) = paired(&other);
    assert_eq!(again.set_push(&phone, &handle, &["completed".into()]), PushRegistrationResult::Saved, "saved");
    let file = other.join(mighty_platform::pairing::identity_file::FILE);
    let mut saved: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    saved["devices"][0]["push"]["handle"] = Value::from("not a handle");
    std::fs::write(&file, saved.to_string()).unwrap();
    let read = Identity::load_or_create(Arc::new(FileStore(other.clone())), true).unwrap();
    assert!(read.push_devices().is_empty() && read.contains(&phone, false), "a malformed handle is dropped, the phone kept");
    push.dispose();
    let _ = std::fs::remove_dir_all(&directory);
    let _ = std::fs::remove_dir_all(&other);
}

/// L448 "mobile push: a new permission is sealed, sent, and a gone handle forgotten".
#[tokio::test]
async fn mobile_push_a_new_permission_is_sealed_sent_and_a_gone_handle_forgotten() {
    let directory = temp("sealed");
    let (identity, client, token) = paired(&directory);
    let (push, seams) = hub(&identity, vec![Duration::ZERO, Duration::ZERO]);
    let transport = seams.transport.clone();
    let handle = "h".repeat(43);
    assert_eq!(push.register(&client, &handle, &[message::PERMISSION.into()]), PushRegistrationResult::Saved, "registered");
    settle(push.push(&[pane("pane-1", "running", &[])])).await;
    let asking = PushPane {
        requests: vec![PushRequest::new("req-1", "run-1", message::PERMISSION, "git status", message::BUTTONS_CLAUDE)],
        ..pane("pane-1", "running", &[])
    };
    settle(push.push(std::slice::from_ref(&asking))).await;
    assert_eq!(transport.count(), 1, "one push");
    let (request, url, host_token) = transport.sent.lock().unwrap()[0].clone();
    assert_eq!(url, "https://relay.example.invalid/push/v1/send", "to the relay's push route");
    assert_eq!(request.server_id, hash(&host_token), "serverId is SHA-256 of the host token");
    let plain = seal::open(&request.payload, &key_of(&token, &client), &handle).expect("the phone could not open it");
    let pushed: Value = serde_json::from_slice(&plain).unwrap();
    assert!(pushed["id"] == "request:run-1:req-1" && pushed["v"] == 1, "the message");
    assert_eq!(pushed["at"], "1970-01-01T00:16:40Z", "the Mac's timestamp form");
    assert_eq!(pushed["permission"]["buttons"], "claude", "the buttons");
    push.viewing(&client, Some("pane-1".into()));
    let mut more = asking;
    more.requests.push(PushRequest::new("req-2", "run-1", message::PERMISSION, "", message::BUTTONS_CLAUDE));
    settle(push.push(std::slice::from_ref(&more))).await;
    assert_eq!(transport.count(), 1, "nothing for the session the phone is viewing");
    push.viewing(&client, None);
    transport.queue(PushOutcome::new(PushStatus::Retry));
    transport.queue(PushOutcome::new(PushStatus::Retry));
    transport.queue(PushOutcome::from_status(410, None));
    more.requests.push(PushRequest::new("req-3", "run-1", message::PERMISSION, "", message::BUTTONS_CLAUDE));
    settle(push.push(&[more])).await;
    assert!(transport.count() == 4 && identity.push_devices().is_empty(), "two retries, then a gone handle is forgotten");
    push.dispose();
    let _ = std::fs::remove_dir_all(&directory);
}

/// L449 "mobile push: the panes are projected as the Mac projects them".
#[test]
fn mobile_push_the_panes_are_projected_as_the_mac_projects_them() {
    let workspace = |id: &str, name: &str, remote: bool| Workspace {
        id: Some(id.into()),
        name: Some(name.into()),
        remote: remote.then(|| RemoteWorkspaceReference {
            connection_id: Some("c".into()),
            workspace_id: Some("w".into()),
            host_name: Some("other PC".into()),
            version: 2,
            mirror: false,
        }),
        ..Workspace::default()
    };
    let workspaces = vec![workspace("ws-1", "demo", false), workspace("ws-1", "second", false), workspace("ws-r", "away", true)];
    let wide = "\u{1F600}".repeat(450);
    let entry = |id: &str, text: &str| LogEntry::new(id.into(), "assistant", text.into(), "t".into(), None);
    let session = |id: &str, workspace: &str, title: &str, status: &str, kind: &str| RunSession {
        id: Some(id.into()),
        workspace_id: Some(workspace.into()),
        title: Some(title.into()),
        status: Some(status.into()),
        kind: Some(kind.into()),
        logs: Some(vec![]),
        ..RunSession::default()
    };
    let mut tool = entry("3", "ran a tool");
    tool.activity = Some(AgentActivity {
        id: "a".into(),
        provider: "claude".into(),
        kind: "tool".into(),
        state: "completed".into(),
        summary: "s".into(),
        tool_name: None,
        output: None,
        duration_ms: None,
    });
    let asking = RunSession {
        logs: Some(vec![entry("1", "older"), entry("2", &wide), tool, entry("4", "")]),
        ..session("pane-a", "ws-1", "Claude 1", "running", "claude")
    };
    let signed_out = session("pane-b", "ws-1", "Claude 2", "error", "claude");
    let files = session("pane-f", "ws-1", "Files", "idle", "files");
    let away = session("pane-r", "ws-r", "Claude", "idle", "claude");
    let request = |id: &str, state: &str| ToolPermissionRequest {
        state: state.into(),
        ..ToolPermissionRequest::new(id, "pane-a", &format!("tu-{id}"), "Bash", r#"{"command":"ls"}"#, &format!("ls {id}"))
    };
    let permissions = Mutex::new(HashMap::from([
        ("pane-a".to_string(), vec![request("styled", "pending"), request("asked", "pending"), request("done", "allowed")]),
        ("pane-r".to_string(), vec![request("far", "pending")]),
    ]));
    let card = PermissionRequest {
        id: "agy-1".into(),
        run_id: "run-agy".into(),
        actions: vec![DeniedAction {
            kind: "command".into(),
            action: "RunCommand".into(),
            display_name: "RunCommand".into(),
            target: Some("ls".into()),
            rule: Some("command(ls)".into()),
            verdict: None,
        }],
        received_at: None,
    };
    let sessions = [&asking, &signed_out, &files, &away];
    let project = || {
        let permissions = permissions.lock().unwrap().clone();
        planner::project(
            &sessions,
            &workspaces,
            &|id| permissions.get(id).cloned().unwrap_or_default(),
            &|session, request| session == "pane-a" && request.id == "styled",
            &|id| (id == "pane-a").then(|| card.clone()),
            &|id| id == "pane-b",
        )
    };
    let panes = project();
    assert_eq!(panes.iter().map(|p| p.session_id.as_str()).collect::<Vec<_>>(), vec!["pane-a", "pane-b"], "only the panes a phone can see");
    let a = &panes[0];
    assert!(
        a.workspace == "demo" && a.title == "Claude 1" && a.status == "running",
        "the first workspace of an id names it, and a pane waiting on a request is still running"
    );
    assert!(
        a.requests.iter().map(|r| r.id.as_str()).collect::<Vec<_>>() == vec!["asked", "agy-1"]
            && a.requests[0].buttons == message::BUTTONS_CLAUDE
            && a.requests[1].kind == message::PERMISSION
            && a.requests[1].buttons == message::BUTTONS_ANTIGRAVITY,
        "the pending requests minus the style's own, then the Antigravity card: {:?}",
        a.requests
    );
    let preview = a.preview.clone().unwrap();
    assert!(
        preview == "\u{1F600}".repeat(400) && preview.chars().count() == planner::PREVIEW_LIMIT,
        "the newest line that is not a tool activity, cut to 400 scalars"
    );
    assert!(
        !a.sign_in_lost && panes[1].sign_in_lost && panes[1].preview.is_none() && panes[1].workspace == "demo",
        "a confirmed lost sign-in; no line, no preview"
    );
    assert_eq!(project(), panes, "the same panes again are equal");
    permissions
        .lock()
        .unwrap()
        .insert("pane-a".into(), vec![request("styled", "pending"), request("asked", "pending"), request("next", "pending")]);
    assert_ne!(project(), panes, "a new request makes them differ");
}

struct Nothing;

impl PushSource for Nothing {
    fn panes(&self) -> Vec<PushPane> {
        vec![]
    }
    fn sign_in_lost(&self, _: &str) -> bool {
        false
    }
}

async fn hand(push: &Arc<PushHub>, transport: &Recording, pane: PushPane) -> usize {
    let before = transport.count();
    settle(push.push(&[pane])).await;
    transport.count() - before
}

/// L450 "mobile push: nothing goes out while off, and switching on pushes nothing old".
#[tokio::test]
async fn mobile_push_nothing_goes_out_while_off_and_switching_on_pushes_nothing_old() {
    let directory = temp("off");
    let (identity, client, _) = paired(&directory);
    let (push, seams) = hub(&identity, vec![]);
    let transport = seams.transport.clone();
    seams.on.store(false, Ordering::SeqCst);
    let registered = push.register(&client, &"h".repeat(43), &[message::PERMISSION.into(), message::COMPLETED.into()]);
    assert_eq!(registered, PushRegistrationResult::Saved, "registered");
    let asking = |ids: &[&str]| pane("pane-1", "running", &ids.iter().map(|id| (*id, "run-1")).collect::<Vec<_>>());
    assert!(
        push.observe(&Nothing).is_empty()
            && hand(&push, &transport, asking(&[])).await == 0
            && hand(&push, &transport, asking(&["r1"])).await == 0,
        "nothing while off"
    );
    seams.on.store(true, Ordering::SeqCst);
    assert_eq!(hand(&push, &transport, asking(&["r1", "r2"])).await, 0, "the first hand-off after switching on only sets the baseline");
    assert_eq!(hand(&push, &transport, asking(&["r1", "r2", "r3"])).await, 1, "then a new request is pushed");
    assert_eq!(hand(&push, &transport, asking(&["r1", "r2", "r3"])).await, 0, "the same panes again decide nothing");
    seams.on.store(false, Ordering::SeqCst);
    assert_eq!(hand(&push, &transport, asking(&["r1", "r2", "r3", "r4"])).await, 0, "off again: nothing");
    seams.on.store(true, Ordering::SeqCst);
    assert_eq!(
        hand(&push, &transport, asking(&["r1", "r2", "r3", "r4", "r5"])).await,
        0,
        "on again: a new baseline, r4 and r5 came while off"
    );
    assert!(
        hand(&push, &transport, asking(&["r3", "r6"])).await == 1
            && hand(&push, &transport, asking(&["r3"])).await == 0
            && hand(&push, &transport, asking(&["r3", "r6"])).await == 0,
        "an id already sent is never sent again"
    );
    push.dispose();
    assert!(
        push.observe(&Nothing).is_empty()
            && hand(&push, &transport, asking(&["r7"])).await == 0
            && hand(&push, &transport, asking(&["r7", "r8"])).await == 0,
        "nothing after shutdown"
    );
    let _ = std::fs::remove_dir_all(&directory);
}

/// L451 "mobile push: deliveries retry and stop as the Mac's do".
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mobile_push_deliveries_retry_and_stop_as_the_macs_do() {
    let directory = temp("retry");
    let (identity, client, _) = paired(&directory);
    let (push, seams) = hub(&identity, mighty_remote::push::hub::RETRY_DELAYS.to_vec());
    let transport = seams.transport.clone();
    let (handle, newer) = ("h".repeat(43), "n".repeat(43));
    assert_eq!(push.register(&client, &handle, &[message::PERMISSION.into()]), PushRegistrationResult::Saved, "registered");
    let serial = AtomicUsize::new(0);
    let ask = || {
        let n = serial.fetch_add(1, Ordering::SeqCst) + 1;
        pane("pane-1", "running", &[(&format!("r{n}"), "run-1")])
    };
    settle(push.push(&[pane("pane-1", "running", &[])])).await;
    transport.queue(PushOutcome::retry(Some(Duration::from_secs(5))));
    transport.queue(PushOutcome::retry(Some(transport::MAXIMUM_RETRY_AFTER)));
    settle(push.push(&[ask()])).await;
    assert!(
        transport.count() == 3 && seams.waits() == vec![Duration::from_secs(5), Duration::from_secs(30)],
        "a longer Retry-After wins, at most 30 s: {:?}",
        seams.waits()
    );
    seams.waits.lock().unwrap().clear();
    transport.queue(PushOutcome::retry(Some(Duration::from_secs(1))));
    transport.queue(PushOutcome::new(PushStatus::Retry));
    transport.queue(PushOutcome::new(PushStatus::Retry));
    settle(push.push(&[ask()])).await;
    assert!(
        transport.count() == 6 && seams.waits() == vec![Duration::from_secs(2), Duration::from_secs(8)],
        "2 s, 8 s, then it gives up: {:?}",
        seams.waits()
    );
    transport.queue(PushOutcome::from_status(400, None));
    settle(push.push(&[ask()])).await;
    assert!(transport.count() == 7 && identity.push_devices().len() == 1, "a refusal is not retried and forgets nothing");
    let thrown = Arc::new(AtomicUsize::new(0));
    let counter = thrown.clone();
    transport.answer(Some(Arc::new(move |_| {
        let first = counter.fetch_add(1, Ordering::SeqCst) == 0;
        Box::pin(async move { if first { Err(PlatformError::Refused("boom".into())) } else { Ok(PushOutcome::new(PushStatus::Sent)) } })
    })));
    settle(push.push(&[ask()])).await;
    assert_eq!(transport.count(), 9, "an exception is tried again");
    // The phone registered a newer handle while the old one was on its way: "gone" for the old one leaves the new one.
    let (again, newer_handle, phone) = (push.clone(), newer.clone(), client.clone());
    transport.answer(Some(Arc::new(move |_| {
        again.register(&phone, &newer_handle, &[message::PERMISSION.into()]);
        Box::pin(async { Ok(PushOutcome::from_status(404, None)) })
    })));
    settle(push.push(&[ask()])).await;
    assert_eq!(identity.push_devices()[0].handle, newer, "only the handle that was sent to is forgotten");
    // Switched off between two attempts: no more.
    seams.waits.lock().unwrap().clear();
    let on = seams.on.clone();
    transport.answer(Some(Arc::new(move |_| {
        on.store(false, Ordering::SeqCst);
        Box::pin(async { Ok(PushOutcome::new(PushStatus::Retry)) })
    })));
    let before = transport.count();
    settle(push.push(&[ask()])).await;
    assert!(transport.count() == before + 1 && seams.waits().is_empty(), "switched off meanwhile, nothing is tried again");
    seams.on.store(true, Ordering::SeqCst);
    // Closing while a send is under way: it is cancelled, and no retry is planned.
    let entered = Arc::new(tokio::sync::Notify::new());
    let signal = entered.clone();
    transport.answer(Some(Arc::new(move |_| {
        signal.notify_one();
        Box::pin(async {
            std::future::pending::<()>().await;
            Ok(PushOutcome::new(PushStatus::Sent))
        })
    })));
    let before = transport.count();
    let sending = push.push(&[ask()]);
    tokio::time::timeout(Duration::from_secs(10), entered.notified()).await.unwrap();
    push.dispose();
    tokio::time::timeout(Duration::from_secs(10), settle(sending)).await.expect("closing ends the send");
    assert!(transport.count() == before + 1 && seams.waits().is_empty(), "closing ends the send without a retry");
    let _ = std::fs::remove_dir_all(&directory);
}

/// L452 "mobile push: the caller's cancellation is not a retry": the app closing while a POST is under way ends the send at once (the
/// transport's future dropped), and nothing is tried again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mobile_push_the_callers_cancellation_is_not_a_retry() {
    struct Hanging(AtomicUsize);
    impl mighty_platform::HttpsSend for Hanging {
        fn get(
            &self,
            _: mighty_platform::HttpsRequest,
            _: usize,
        ) -> BoxFuture<'_, mighty_platform::Result<mighty_platform::HttpsResponse>> {
            Box::pin(async { Err(PlatformError::unsupported("Hanging", "get")) })
        }
        fn post(
            &self,
            _: mighty_platform::HttpsRequest,
            _: Vec<u8>,
            _: usize,
        ) -> BoxFuture<'_, mighty_platform::Result<mighty_platform::HttpsResponse>> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Box::pin(std::future::pending())
        }
    }
    let directory = temp("cancel");
    let (identity, client, _) = paired(&directory);
    let https = Arc::new(Hanging(AtomicUsize::new(0)));
    let seams = PushSeams::new(https.clone(), Arc::new(|| "wss://relay.example.invalid".to_string()), Arc::new(|| true));
    let push = PushHub::new(identity.clone(), "Test PC", seams);
    push.register(&client, &"h".repeat(43), &[message::PERMISSION.into()]);
    settle(push.push(&[pane("pane-1", "running", &[])])).await;
    let sending = push.push(&[pane("pane-1", "running", &[("r1", "run-1")])]);
    tokio::time::timeout(Duration::from_secs(10), async {
        while https.0.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    push.dispose();
    tokio::time::timeout(Duration::from_secs(10), settle(sending)).await.expect("a cancelled send ends");
    assert_eq!(https.0.load(Ordering::SeqCst), 1, "not tried again");
    let _ = std::fs::remove_dir_all(&directory);
}

/// The checks' panes (C#'s `DesktopService` snapshot and the login recovery's lost sign-ins).
#[derive(Default)]
struct Panes {
    panes: Mutex<Vec<PushPane>>,
    lost: Mutex<HashSet<String>>,
}

impl PushSource for Panes {
    fn panes(&self) -> Vec<PushPane> {
        let lost = self.lost.lock().unwrap().clone();
        self.panes.lock().unwrap().iter().map(|p| PushPane { sign_in_lost: lost.contains(&p.session_id), ..p.clone() }).collect()
    }
    fn sign_in_lost(&self, session_id: &str) -> bool {
        self.lost.lock().unwrap().contains(session_id)
    }
}

impl Panes {
    fn set(&self, panes: Vec<PushPane>) {
        *self.panes.lock().unwrap() = panes;
    }
}

async fn observe(push: &Arc<PushHub>, source: &Panes, transport: &Recording, token: &str, client: &str) -> Vec<String> {
    let count = transport.count();
    settle(push.observe(source)).await;
    sent_ids(transport, count, token, client)
}

/// L453 "mobile push: observing reads the panes, and disposing lets go".
#[tokio::test]
async fn mobile_push_observing_reads_the_panes_and_disposing_lets_go() {
    let directory = temp("observe");
    let (identity, client, token) = paired(&directory);
    let before = identity.rotation_listeners();
    let (push, seams) = hub(&identity, vec![]);
    assert_eq!(identity.rotation_listeners(), before + 1, "subscribed to the identity's rotation");
    let transport = seams.transport.clone();
    let registered = push.register(&client, &"h".repeat(43), &[message::SIGNIN.into(), message::COMPLETED.into()]);
    assert_eq!(registered, PushRegistrationResult::Saved, "registered");
    let source = Panes::default();
    source.set(vec![PushPane::new("pane-1", "demo", "Claude 1", "running", Some("all done"), vec![])]);
    assert!(observe(&push, &source, &transport, &token, &client).await.is_empty(), "the first observation is the baseline");
    source.lost.lock().unwrap().insert("pane-1".into());
    assert_eq!(
        observe(&push, &source, &transport, &token, &client).await,
        vec!["signin:pane-1:1"],
        "a lost sign-in is seen without any other news"
    );
    source.set(vec![PushPane::new("pane-1", "demo", "Claude 1", "completed", Some("all done"), vec![])]);
    assert!(observe(&push, &source, &transport, &token, &client).await.is_empty(), "nothing reported, nothing read");
    push.changed();
    assert_eq!(
        observe(&push, &source, &transport, &token, &client).await,
        vec!["run:pane-1:1:completed"],
        "a finished run once something moved"
    );
    push.dispose();
    assert_eq!(identity.rotation_listeners(), before, "disposing lets go of the identity");
    let _ = std::fs::remove_dir_all(&directory);
}

/// L454 "mobile push: a run between two ticks is still pushed": a run that starts and ends between two ticks still gets its push, a
/// shell's command too; only what the app reports makes the panes be read again; a request of a pane that is gone is not projected.
#[tokio::test]
async fn mobile_push_a_run_between_two_ticks_is_still_pushed() {
    let directory = temp("ticks");
    let (identity, client, token) = paired(&directory);
    let (push, seams) = hub(&identity, vec![]);
    let transport = seams.transport.clone();
    let registered = push.register(&client, &"h".repeat(43), &[message::COMPLETED.into(), message::FAILED.into()]);
    assert_eq!(registered, PushRegistrationResult::Saved, "registered");
    let source = Panes::default();
    let both = |status: &str, shell: &str| {
        vec![
            PushPane::new("pane-1", "demo", "Claude 1", status, Some("done"), vec![]),
            PushPane::new("shell-1", "demo", "Terminal", shell, None, vec![]),
        ]
    };
    source.set(both("idle", "idle"));
    assert!(observe(&push, &source, &transport, &token, &client).await.is_empty(), "the baseline");
    push.began("pane-1");
    source.set(both("completed", "idle"));
    push.changed();
    assert_eq!(
        observe(&push, &source, &transport, &token, &client).await,
        vec!["run:pane-1:1:completed"],
        "running then completed between two ticks"
    );
    push.began("pane-1");
    source.set(both("error", "idle"));
    push.changed();
    assert_eq!(
        observe(&push, &source, &transport, &token, &client).await,
        vec!["run:pane-1:2:failed"],
        "running then error between two ticks"
    );
    push.began("pane-1");
    source.set(both("running", "idle"));
    assert!(observe(&push, &source, &transport, &token, &client).await.is_empty(), "a run seen running");
    source.set(both("completed", "idle"));
    push.changed();
    assert_eq!(
        observe(&push, &source, &transport, &token, &client).await,
        vec!["run:pane-1:3:completed"],
        "and seen ending: one push, not two"
    );
    push.began("shell-1");
    source.set(both("completed", "completed"));
    push.changed();
    assert_eq!(
        observe(&push, &source, &transport, &token, &client).await,
        vec!["run:shell-1:1:completed"],
        "a shell's run between two ticks"
    );
    // Output alone does not make the panes be read again: the change below is seen only once something is reported.
    source.set(both("running", "completed"));
    assert!(observe(&push, &source, &transport, &token, &client).await.is_empty(), "output alone sends nothing");
    source.set(both("completed", "completed"));
    push.changed();
    assert!(
        observe(&push, &source, &transport, &token, &client).await.is_empty(),
        "output did not make the panes be read while the pane was running"
    );
    // A pending request whose pane is no longer there is not projected.
    let gone = ToolPermissionRequest::new("req-gone", "pane-gone", "tu", "Bash", "{}", "ls");
    let here = RunSession { id: Some("pane-1".into()), workspace_id: Some("ws".into()), ..RunSession::default() };
    let workspaces = vec![Workspace { id: Some("ws".into()), ..Workspace::default() }];
    let projected = planner::project(
        &[&here],
        &workspaces,
        &|id| if id == "pane-gone" { vec![gone.clone()] } else { vec![] },
        &|_, _| false,
        &|_| None,
        &|_| false,
    );
    assert!(projected.len() == 1 && projected[0].requests.is_empty(), "only the requests of panes still there");
    push.dispose();
    let _ = std::fs::remove_dir_all(&directory);
}
