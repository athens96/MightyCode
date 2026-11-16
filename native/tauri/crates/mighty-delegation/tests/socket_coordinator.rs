//! The coordinator answering over the real transport (M-C; the Swift checks DelegationStatusToolTests, DelegationActionToolTests and
//! DelegationCoordinatorTests name "over the real socket"), each under its Swift name: the delegation MCP server, in process, calls the
//! app's listener on the parent's token, and the coordinator behind it answers. Windows' named pipe and the Mac's Unix socket.
//!
//! The status check needs no git; the two that run git (a child's merge, delegate's worktree) go through the platform's process host
//! (Windows', or M-A's on the Mac).
#![cfg(any(windows, target_os = "macos"))]

mod delegation_io;
mod support;

use std::sync::Arc;

use mighty_delegation::agent_io::binding::TOKEN_KEY;
use mighty_delegation::agent_io::server::{AgentIo, TerminalCall, TerminalHandler};
use mighty_delegation::coordinator::Coordinator;
use mighty_delegation::copy::DelegationCopyKind;
use mighty_delegation::host::{DelegationPaneActivity, DelegationPaneState};
use mighty_delegation::mcp::delegation::DelegationServer;
use mighty_delegation::model::{ChildRecord, ChildState};
use mighty_delegation::modes::delegation_transport;
use mighty_delegation::store::{DelegationFile, DelegationFileStore};
use mighty_platform::BoxFuture;
use serde_json::{Value, json};
use support::host::{self, TestHost, copy, record};
use support::lane;
use support::place::DelegationPlace;

/// Swift's `NoTerminalCalls`: the terminal tools are never asked in these checks.
struct NoTerminalCalls;

impl TerminalHandler for NoTerminalCalls {
    fn handle(&self, _call: TerminalCall) -> BoxFuture<'static, Result<Value, String>> {
        Box::pin(async { Err("no terminal call is expected".into()) })
    }
}

/// A coordinator over `file` in the place's profile, its pane "parent" a Claude pane running in the repository; the listener it
/// answers behind; and the parent's binding with the delegation server attached.
struct Fixture {
    place: DelegationPlace,
    host: Arc<TestHost>,
    coordinator: Arc<Coordinator>,
    io: Arc<AgentIo>,
    server: DelegationServer,
}

impl Fixture {
    fn new(place: DelegationPlace, file: DelegationFile) -> Fixture {
        let store = DelegationFileStore::with_attempts(place.profile(), 40);
        store.save(&file).unwrap();
        let host = TestHost::new(Some(place.profile().into()), "run");
        host.set(DelegationPaneState {
            session_id: "parent".into(),
            permission_mode: "auto".into(),
            folder: place.repo(),
            kind: "claude".into(),
            provider: "claude".into(),
            parent_session_id: None,
            run_id: Some("run-1".into()),
            activity: DelegationPaneActivity::Running,
        });
        std::fs::create_dir_all(place.repo()).unwrap();
        let coordinator = Arc::new(host::coordinator(store, host.clone(), host::maker(&place, 50_000_000_000), || true).unwrap());
        let (listener, connector) = delegation_io::os_transport();
        let io = AgentIo::start(listener, Arc::new(NoTerminalCalls)).unwrap();
        let parent = io.bind("parent", "ws-1", &place.repo(), "claude", &delegation_io::executable()).unwrap();
        let handler: Arc<dyn mighty_delegation::request::DelegationHandler> = coordinator.clone();
        parent.connect_delegation(Some(handler), true, "");
        let transport = delegation_transport(connector, io.name().to_string(), parent.environment()[TOKEN_KEY].clone());
        Fixture { place, host, coordinator, io, server: DelegationServer::new(Some(transport)) }
    }

    /// One `tools/call` through the server, its `result`.
    async fn call(&self, tool: &str, arguments: Value) -> Value {
        let line = json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": {"name": tool, "arguments": arguments}}).to_string();
        let answer: Value = serde_json::from_str(&self.server.handle(&line).await.expect("an answer")).unwrap();
        answer["result"].clone()
    }
}

/// `DelegationStatusToolTests.swift:231 "overTheRealSocketTheDelegationServerHandsBackTheListAndTheReport"`: list_children and
/// child_status come back over the real transport as tool data, the report's text and head included, and the host is never asked.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn over_the_real_socket_the_delegation_server_hands_back_the_list_and_the_report() {
    let _lane = lane::enter().await;
    let reported_head = "b".repeat(40);
    let report_text = "# 보고\n\nFixed the parser.\n";
    let with = |r: ChildRecord, revision: i32, head: Option<String>| ChildRecord { report_revision: revision, report_head: head, ..r };
    let children = vec![
        record("running", ChildState::Running, "parent"),
        with(record("reported", ChildState::Reported, "parent"), 2, Some(reported_head.clone())),
        with(record("merged", ChildState::Merged, "parent"), 1, Some("c".repeat(40))),
        record("failed", ChildState::Failed, "parent"),
        with(record("closed", ChildState::Closed, "parent"), 3, Some("d".repeat(40))),
        record("discarded", ChildState::Discarded, "parent"),
    ];
    let file =
        DelegationFile::of(children, vec![], vec![], vec![], vec![copy("reported", DelegationCopyKind::Report, report_text, Some(2))]);
    let fixture = Fixture::new(DelegationPlace::new(), file);
    let list = fixture.call("list_children", json!({})).await;
    assert!(list.get("isError").is_none_or(|e| e == false), "{list}");
    let ids: Vec<&str> = list["structuredContent"]["children"].as_array().unwrap().iter().map(|c| c["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["running", "reported", "merged", "failed", "closed", "discarded"]);
    let status = fixture.call("child_status", json!({"child": "reported"})).await;
    assert!(status.get("isError").is_none_or(|e| e == false), "{status}");
    let data = &status["structuredContent"]["status"];
    assert!(data["state"] == "reported" && data["head"] == reported_head.as_str() && data["report"] == report_text, "{data}");
    assert!(fixture.host.calls().is_empty(), "the host is never asked: {:?}", fixture.host.calls());
    fixture.io.close().await;
}

/// `DelegationActionToolTests.swift:434 "overTheRealSocketMergeAndFollowUpComeBackAsToolData"`: a reported child's merge over the
/// real transport fast-forwards the parent and comes back as tool data; the same merge again is refused `not_reported`; a follow-up is
/// recorded with one more left. Git runs through the platform's process host.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn over_the_real_socket_merge_and_follow_up_come_back_as_tool_data() {
    let _lane = lane::enter().await;
    let place = DelegationPlace::new();
    let base = place.repository_default().await;
    let (child, head) = place.reported_child("c1", &["a.txt"], Some(&place.repo())).await;
    let file = DelegationFile::of(vec![host::record_of(&child)], vec![], vec![], vec![], vec![]);
    let fixture = Fixture::new(place, file);
    let merge = fixture.call("merge", json!({"child": "c1", "expected_head": head})).await;
    assert!(merge.get("isError").is_none_or(|e| e == false), "{merge}");
    let merged = &merge["structuredContent"]["merged"];
    assert!(merged["child"] == "c1" && merged["preMergeCommit"] == base.as_str() && merged["mergedCommit"] == head.as_str(), "{merged}");
    assert_eq!(fixture.place.fixture_git(&["rev-parse", "HEAD"]).await, head);
    let again = fixture.call("merge", json!({"child": "c1", "expected_head": head})).await;
    assert!(again["isError"] == true && again["structuredContent"] == json!({"refused": "not_reported"}), "{again}");
    let follow_up = fixture.call("follow_up", json!({"child": "c1", "text": "Check the docs too."})).await;
    assert!(follow_up.get("isError").is_none_or(|e| e == false), "{follow_up}");
    let recorded = &follow_up["structuredContent"]["followUp"];
    // Swift's lane is `pending`; the shared coordinator follows C#, which holds a merged child's follow-up (`held`): the lane the
    // coordinator chose is what crossed the transport.
    assert!(recorded["child"] == "c1" && recorded["lane"] == "held" && recorded["remaining"] == 1, "{recorded}");
    let file = fixture.coordinator.file();
    assert_eq!(file.follow_ups.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(), [recorded["id"].as_str().unwrap()]);
    assert_eq!(file.merges.iter().map(|m| m.merged_commit.as_str()).collect::<Vec<_>>(), [head.as_str()]);
    fixture.io.close().await;
}

/// `DelegationCoordinatorTests.swift:119 "withTheSwitchOnEachToolAnswersWithinTheSocketLimit"`: the coordinator's answer time is
/// below the transport's 60 s limit, and with the switch on each of the six tools is answered over the real transport within it — by
/// its tool (a refusal, an error or its data), never by the transport giving up, an unknown-tool answer, the detached gate or the
/// Claude-only gate. delegate makes a worktree (git) through the platform's process host.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn with_the_switch_on_each_tool_answers_within_the_socket_limit() {
    use mighty_delegation::agent_io::client::DELEGATION_SECONDS;
    use mighty_delegation::coordinator::ANSWER_SECONDS;
    let _lane = lane::enter().await;
    assert!(ANSWER_SECONDS < DELEGATION_SECONDS as f64);
    let place = DelegationPlace::new();
    place.repository_default().await;
    let fixture = Fixture::new(place, DelegationFile::default());
    for tool in mighty_delegation::tools::ALL {
        let arguments: serde_json::Map<String, Value> = tool
            .arguments
            .iter()
            .map(|argument| {
                let value = match argument.name {
                    "task" => "Write the release notes".to_string(),
                    "mode" => "plan".to_string(),
                    "child" => "child-1".to_string(),
                    "expected_head" => "b".repeat(40),
                    "text" => "Add a summary".to_string(),
                    _ => "x".to_string(),
                };
                (argument.name.to_string(), Value::String(value))
            })
            .collect();
        let started = std::time::Instant::now();
        let answer = fixture.call(tool.name, Value::Object(arguments)).await;
        assert!(started.elapsed() < std::time::Duration::from_secs_f64(ANSWER_SECONDS), "{}", tool.name);
        let text = answer.to_string();
        assert!(
            !text.contains("did not answer the request")
                && !text.contains("Unknown delegation tool")
                && !text.contains(mighty_delegation::request::DETACHED_MESSAGE)
                && answer["structuredContent"]["refused"] != "claude_only",
            "{}: {answer}",
            tool.name
        );
    }
    fixture.io.close().await;
}
