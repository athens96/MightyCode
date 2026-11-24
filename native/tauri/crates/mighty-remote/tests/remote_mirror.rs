//! C# `RemoteMirrorVerification` (Verification.cs, "remote mirror: …", the nine checks by name; docs/remote-workspaces.md "그 컴퓨터의 창
//! 그대로"): `native/contracts/fixtures/remote-mirror/` read as the Mac reads it, the bounds and revisions both sides keep, what is never
//! saved, the host's routes over loopback, and a client mirroring a host end to end. Loopback port 0 only; every folder is a temp folder.
//! Owner: W9-J.

mod support;

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use indexmap::IndexMap;
use mighty_core::attachment::RunAttachment;
use mighty_core::graph::MightyGraphRun;
use mighty_core::json::Wire;
use mighty_core::locale::Locale;
use mighty_core::model::{
    AgentImageRef, AgentRunTiming, AppSnapshot, LogEntry, MirrorReference, RemoteWorkspaceReference, RunSession, TodoItem, TodoProgress,
    Workspace,
};
use mighty_core::permission::ToolPermissionRequest;
use mighty_core::store::Store;
use mighty_platform::BoxFuture;
use mighty_platform::fake::MemoryVault;
use mighty_remote::phone::desk::{ActionAnswer, MobileDesktopAction};
use mighty_remote::remote::host::{self, HostOptions, RemoteHost};
use mighty_remote::remote::http_server::HttpRequest;
use mighty_remote::remote::mirror::desk::{MirrorDesk, MirrorStage, QueueAction, Spawn};
use mighty_remote::remote::mirror::desktop_host::DeskMirrorHost;
use mighty_remote::remote::mirror::host::{MirrorHost, Refusal};
use mighty_remote::remote::mirror::{
    MirrorPane, MirrorPaneInfo, MirrorQueued, MirrorSummary, WireMirrorCreate, WireMirrorCreated, WireMirrorDetail, WireMirrorOutcome,
    WireMirrorPermission, WireMirrorQuestions, WireMirrorState, WireMirrorSubmit, protocol, routes, support as mirror,
};
use mighty_remote::remote::strings;
use mighty_remote::remote::transport::{self, Method, Request, TransportError};
use mighty_remote::remote::wire::{RemoteConnectionStatus, RemoteSharedWorkspace, RemoteWireAnswer, RemoteWirePlan};
use mighty_run::plan_mode::PlanDecision;
use mighty_run::questionnaire::UserQuestionAnswer;
use mighty_run::service::RunService;
use mighty_run::strings::{get, get_with};
use serde_json::{Value, json};
use support::desk::{Temp, World};
use support::fake_host::*;

// ── the fixtures ─────────────────────────────────────────────────────────────────────────────────────────────────────────────

fn folder() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../contracts/fixtures/remote-mirror")
}

fn node(name: &str) -> Value {
    serde_json::from_slice(&std::fs::read(folder().join(name)).unwrap()).unwrap()
}

fn bytes(name: &str) -> Vec<u8> {
    std::fs::read(folder().join(name)).unwrap()
}

fn fixture_session() -> mighty_remote::remote::mirror::MirrorSession {
    let detail = WireMirrorDetail::decode(&bytes("detail.json")).unwrap();
    mirror::read(&detail.session).expect("detail.json's session reads")
}

/// The value with every null member left out (C#'s `RemoteWireFixtureVerification.Same` compares without nulls).
fn plain(value: &Value) -> Value {
    match value {
        Value::Object(o) => Value::Object(o.iter().filter(|(_, v)| !v.is_null()).map(|(k, v)| (k.clone(), plain(v))).collect()),
        Value::Array(items) => Value::Array(items.iter().map(plain).collect()),
        other => other.clone(),
    }
}

fn same(a: &Value, b: &Value) -> bool {
    plain(a) == plain(b)
}

/// The files these checks read (C#'s embedded resources: "the checks read every file of the folder").
const READ: [&str; 13] = [
    "answer.json",
    "create.json",
    "created.json",
    "detail.json",
    "empty.json",
    "endpoints.json",
    "outcome.json",
    "permission.json",
    "plan.json",
    "questions.json",
    "state-unchanged.json",
    "state.json",
    "submit.json",
];

/// "remote mirror: the index names every route and every file is accounted for" (the Mac's
/// theIndexNamesEveryRouteAndEveryFileIsAccountedFor).
#[test]
fn the_index_names_every_route_and_every_file_is_accounted_for() {
    let index = node("endpoints.json");
    let index = index.as_array().unwrap();
    let names: HashSet<&str> = index.iter().map(|e| e["name"].as_str().unwrap()).collect();
    assert_eq!(names, HashSet::from(["list", "detail", "submit", "stop", "permission", "questions", "plan", "close", "create"]));
    let mut named: HashSet<String> = HashSet::from(["endpoints.json".to_string(), "state-unchanged.json".to_string()]);
    for entry in index {
        assert!(entry["path"].as_str().unwrap().starts_with(routes::PREFIX), "every path is under {}", routes::PREFIX);
        for key in ["request", "response"] {
            if let Some(name) = entry[key].as_str() {
                named.insert(name.into());
            }
        }
    }
    let files: HashSet<String> =
        std::fs::read_dir(folder()).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    assert_eq!(files, named, "every file is accounted for");
    assert_eq!(files, READ.iter().map(|n| n.to_string()).collect::<HashSet<_>>(), "the checks read every file of the folder");
}

/// "remote mirror: the samples decode, validate and round-trip" (the Mac's theSamplesDecodeValidateAndRoundTrip): every sample reads
/// into this app's types, passes the client's checks and writes back as the Mac wrote it.
#[test]
fn the_samples_decode_validate_and_round_trip() {
    let state = WireMirrorState::decode(&bytes("state.json")).unwrap();
    assert!(mirror::valid_state(&state), "state.json passes the client's checks");
    assert_eq!(state.host_name, "studio-mac");
    assert_eq!(state.workspaces.iter().map(|w| w.id.as_str()).collect::<Vec<_>>(), ["ws-project"]);
    assert_eq!(state.sessions.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["pane-1", "pane-2"]);
    assert!(
        state.sessions[0].pending == 1 && state.sessions[0].queued == 1 && state.sessions[0].branch.as_deref() == Some("mighty/pane-1")
    );
    assert_eq!(state.sessions[1].branch, None, "counts and branch read");
    assert!(same(&node("state.json"), &state.to_value()), "the list writes back as the Mac wrote it: {}", state.to_value());
    let unchanged = WireMirrorState::decode(&bytes("state-unchanged.json")).unwrap();
    assert!(
        mirror::valid_state(&unchanged) && unchanged.unchanged == Some(true) && unchanged.sessions.is_empty(),
        "the unchanged answer reads"
    );
    assert!(same(&node("state-unchanged.json"), &unchanged.to_value()), "the unchanged answer writes back");
    let detail = WireMirrorDetail::decode(&bytes("detail.json")).unwrap();
    let session = fixture_session();
    assert!(mirror::valid_detail(&detail, &session, "pane-1"), "detail.json passes the client's checks");
    assert!(!mirror::valid_detail(&detail, &session, "pane-2"), "a detail answers for the pane asked about only");
    assert!(
        session.logs.len() == 2
            && session.graph_runs.len() == 1
            && session.permissions.len() == 1
            && session.permissions[0].tool_name == "Write"
            && session.permissions[0].input_json.contains("notes.txt"),
        "entries, runs and the request read (inputJSON as input_json)"
    );
    assert_eq!(session.queued, [MirrorQueued { id: "queued-1".into(), text: "Then update CHANGELOG.md".into(), run_now: None }]);
    assert!(session.base.as_deref() == Some("main") && session.branch.as_deref() == Some("mighty/pane-1"), "queue and branch read");
    assert!(
        session.settings.permission_mode == "acceptEdits"
            && session.settings.effort == "high"
            && session.todo_progress.as_ref().and_then(|t| t.items.as_ref()).is_some_and(|i| i.len() == 1)
            && session.started_at.as_deref() == Some("2026-11-13T01:00:01Z"),
        "settings, checklist and clock read"
    );
    // What this app writes for it as a host is the Mac's own JSON (nulls aside).
    let written = mirror::write(&session);
    assert!(
        same(&node("detail.json")["session"], &Value::Object(written.clone())),
        "a host writes the Mac's session: {}",
        Value::Object(written.clone())
    );
    assert!(written["permissions"][0].get("inputJSON").is_some() && written["permissions"][0].get("inputJson").is_none(), "inputJSON");
    assert_eq!(mirror::read(&written).as_ref(), Some(&session), "what this app writes reads back the same");
    let wire = WireMirrorDetail { protocol: 2, mirror: protocol::VERSION, revision: detail.revision.clone(), session: written };
    assert!(same(&node("detail.json"), &wire.to_value()), "the whole answer writes as the Mac's");

    let submit = WireMirrorSubmit::decode(&bytes("submit.json")).unwrap();
    assert!(submit.text == "Then update CHANGELOG.md" && submit.attachments.as_ref().is_some_and(|a| a.len() == 1));
    let checked = mighty_run::attachments::validate(submit.attachments.as_deref()).unwrap();
    assert_eq!(checked.map(|a| a.len()), Some(1), "the send reads with its file");
    assert!(same(&node("submit.json"), &submit.to_value()), "the send writes back");
    assert_eq!(WireMirrorSubmit::decode(br#"{"text":"hi"}"#).unwrap().attachments, None, "a send may leave its files out");
    assert_eq!(WireMirrorOutcome::decode(&bytes("outcome.json")).unwrap().outcome, "queued", "the outcome reads");
    let permission = WireMirrorPermission::decode(&bytes("permission.json")).unwrap();
    assert_eq!(
        permission,
        WireMirrorPermission { request_id: "perm-4".into(), run_id: "job-1".into(), allow: true, always: Some(false) },
        "the permission answer reads"
    );
    assert_eq!(
        WireMirrorPermission::decode(br#"{"requestId":"r","runId":"j","allow":false}"#).unwrap().always,
        None,
        "always may be left out, as the Mac's client does"
    );
    assert_eq!(WireMirrorQuestions::decode(&bytes("questions.json")).unwrap().answers["Which file?"].selected_options, ["README.md"]);
    let plan = RemoteWirePlan::decode(&bytes("plan.json")).unwrap().decision().unwrap();
    assert!(plan.kind == "revise" && plan.feedback.as_deref() == Some("Keep the tests"), "the plan answer reads");
    assert_eq!(WireMirrorCreate::decode(&bytes("create.json")).unwrap().provider, "claude");
    assert_eq!(WireMirrorCreated::decode(&bytes("created.json")).unwrap().session_id, "pane-3", "create and created read");
    assert!(RemoteWireAnswer::decode(&bytes("answer.json")).unwrap().accepted, "the answer reads");
    assert_eq!(node("empty.json"), json!({}), "the empty body reads");
    let round_trips: [(&str, Value); 6] = [
        ("outcome.json", WireMirrorOutcome::decode(&bytes("outcome.json")).unwrap().to_value()),
        ("permission.json", WireMirrorPermission::decode(&bytes("permission.json")).unwrap().to_value()),
        ("questions.json", WireMirrorQuestions::decode(&bytes("questions.json")).unwrap().to_value()),
        ("plan.json", RemoteWirePlan::decode(&bytes("plan.json")).unwrap().to_value()),
        ("create.json", WireMirrorCreate::decode(&bytes("create.json")).unwrap().to_value()),
        ("created.json", WireMirrorCreated::decode(&bytes("created.json")).unwrap().to_value()),
    ];
    for (name, value) in round_trips {
        assert!(same(&node(name), &value), "{name} writes back as the Mac wrote it: {value}");
    }
}

/// "remote mirror: a client refuses lists outside the rules" (the Mac's aClientRefusesListsOutsideTheRules), and the bodies are read
/// as strictly as the Mac reads them.
/// MC-49: what 바로 실행 does on a queued row and whether the host takes the queue's actions travel as members a Mac or an older app
/// leaves out (read as none), and an older reader passes over; a name that is none of the page's reads as no button. The revision moves
/// with a row's 바로 실행, and stays as before for a row without one.
#[test]
fn the_queue_actions_travel_as_additions() {
    let session = fixture_session();
    assert!(!session.queue_actions && session.queued.iter().all(|q| q.run_now.is_none()), "the Mac's detail has neither");
    let rows = vec![
        MirrorQueued { id: "queued-1".into(), text: "Run the tests".into(), run_now: Some("worktree".into()) },
        MirrorQueued { id: "queued-2".into(), text: "Then lint".into(), run_now: None },
    ];
    let hosted = mighty_remote::remote::mirror::MirrorSession { queued: rows.clone(), queue_actions: true, ..session.clone() };
    let written = mirror::write(&hosted);
    assert_eq!(written["queueActions"], json!(true));
    assert_eq!(
        written["queued"],
        json!([{"id": "queued-1", "text": "Run the tests", "runNow": "worktree"}, {"id": "queued-2", "text": "Then lint"}])
    );
    let read = mirror::read(&written).unwrap();
    assert!(read.queue_actions && read.queued == rows, "they read back");
    assert!(!mirror::write(&session).contains_key("queueActions"), "left out when the host does not take them");
    let mut odd = written.clone();
    odd["queued"][0]["runNow"] = json!("runNext");
    assert_eq!(mirror::read(&odd).unwrap().queued[0].run_now, None, "an unknown 바로 실행 is no button");
    odd["queued"][0]["runNow"] = json!(3);
    assert!(mirror::read(&odd).is_none(), "a runNow that is no text refuses the detail, as any member of the wrong kind");
    let pane = host_pane("pane-1", "ws", 2);
    let plain = [MirrorQueued { run_now: None, ..rows[0].clone() }];
    assert_ne!(mirror::revision(&pane, &[], &rows[..1], None, None), mirror::revision(&pane, &[], &plain, None, None));
    let waits = [MirrorQueued { run_now: Some("waitsForTurn".into()), ..rows[0].clone() }];
    assert_ne!(mirror::revision(&pane, &[], &rows[..1], None, None), mirror::revision(&pane, &[], &waits, None, None));
}

#[test]
fn a_client_refuses_lists_outside_the_rules() {
    let state = WireMirrorState::decode(&bytes("state.json")).unwrap();
    assert!(!mirror::valid_state(&WireMirrorState { mirror: 2, ..state.clone() }), "another mirror version is refused");
    assert!(!mirror::valid_state(&WireMirrorState { protocol: 1, ..state.clone() }), "another protocol is refused");
    let pane = |change: &dyn Fn(&mut MirrorSummary)| {
        let mut first = state.sessions[0].clone();
        change(&mut first);
        WireMirrorState { sessions: vec![first, state.sessions[1].clone()], ..state.clone() }
    };
    assert!(!mirror::valid_state(&pane(&|s| s.status = "thinking".into())), "an unknown status is refused");
    assert!(!mirror::valid_state(&pane(&|s| s.workspace_id = "ws-elsewhere".into())), "a pane of an unlisted workspace is refused");
    assert!(!mirror::valid_state(&pane(&|s| s.id = "pane-2".into())), "a pane listed twice is refused");
    assert!(!mirror::valid_state(&pane(&|s| s.kind = "shell".into())), "a command pane is refused");
    assert!(!mirror::valid_state(&pane(&|s| s.id = "../pane".into())), "an id that is no identifier is refused");
    assert!(!mirror::valid_state(&pane(&|s| s.provider = "other".into())), "an unknown agent is refused");
    assert!(!mirror::valid_state(&pane(&|s| s.pending = -1)) && !mirror::valid_state(&pane(&|s| s.queued = 10_001)), "counts are bounded");
    assert!(
        !mirror::valid_state(&pane(&|s| s.title = "t".repeat(201))) && !mirror::valid_state(&pane(&|s| s.revision = "a".repeat(65))),
        "titles and revisions are bounded"
    );
    let many = (0..=protocol::MAXIMUM_SESSIONS)
        .map(|i| MirrorSummary { id: format!("pane-x{i}"), ..state.sessions[1].clone() })
        .collect::<Vec<_>>();
    assert!(!mirror::valid_state(&WireMirrorState { sessions: many, ..state.clone() }), "more than 128 panes are refused");
    assert!(
        !mirror::valid_state(&WireMirrorState { host_name: String::new(), ..state.clone() })
            && !mirror::valid_state(&WireMirrorState { host_name: "h".repeat(121), ..state.clone() }),
        "the host's name is bounded"
    );
    let odd = MirrorSummary { status: "thinking".into(), ..state.sessions[0].clone() };
    assert!(
        mirror::valid_state(&WireMirrorState { unchanged: Some(true), sessions: vec![odd], ..state.clone() }),
        "an unchanged answer's lists mean nothing"
    );
    let detail = WireMirrorDetail::decode(&bytes("detail.json")).unwrap();
    let session = fixture_session();
    let entries = (0..=protocol::MAXIMUM_ENTRIES)
        .map(|i| LogEntry::new(format!("log-{i}"), "user", "x".into(), "2026-11-13T01:00:01Z".into(), None))
        .collect();
    assert!(
        !mirror::valid_detail(&detail, &mighty_remote::remote::mirror::MirrorSession { logs: entries, ..session.clone() }, "pane-1"),
        "more than 80 entries are refused"
    );
    let with_entry = |entry: LogEntry| mighty_remote::remote::mirror::MirrorSession { logs: vec![entry], ..session.clone() };
    assert!(
        !mirror::valid_detail(&detail, &with_entry(LogEntry { kind: Some("secret".into()), ..session.logs[0].clone() }), "pane-1"),
        "an entry kind the transcript never keeps is refused"
    );
    assert!(
        !mirror::valid_detail(&detail, &with_entry(LogEntry { text: Some("x".repeat(131_073)), ..session.logs[0].clone() }), "pane-1"),
        "an entry over 128 KiB is refused"
    );
    let bad_run = ToolPermissionRequest { run_id: "bad id".into(), ..session.permissions[0].clone() };
    assert!(
        !mirror::valid_detail(
            &detail,
            &mighty_remote::remote::mirror::MirrorSession { permissions: vec![bad_run], ..session.clone() },
            "pane-1"
        ),
        "a request whose run id is no identifier is refused"
    );
    assert!(
        !mirror::valid_detail(&WireMirrorDetail { mirror: 2, ..detail.clone() }, &session, "pane-1"),
        "another mirror version is refused"
    );
    // Strict as the Mac's decoder: a required member missing, or a list with a null in it, is a bad answer.
    let mut missing = node("state.json");
    missing["sessions"][0].as_object_mut().unwrap().remove("pending");
    assert!(WireMirrorState::decode(missing.to_string().as_bytes()).is_err(), "a summary without its pending count is refused");
    let mut nulled = node("state.json");
    nulled["sessions"].as_array_mut().unwrap().push(Value::Null);
    assert!(WireMirrorState::decode(nulled.to_string().as_bytes()).is_err(), "a null pane is refused");
    let repeated = br#"{"protocol":2,"protocol":2,"mirror":1,"hostName":"h","revision":"","workspaces":[],"sessions":[]}"#;
    assert!(WireMirrorState::decode(repeated).is_err(), "a repeated member is refused");
    let mut no_logs = node("detail.json")["session"].as_object().unwrap().clone();
    no_logs.remove("logs");
    assert!(mirror::read(&no_logs).is_none(), "a session without its entries does not read");
    // What a pane here can do without is left out rather than the whole pane refused.
    let mut odd = node("detail.json")["session"].as_object().unwrap().clone();
    odd.insert("graphRuns".into(), json!([{"id": 5}]));
    odd.insert("todoProgress".into(), json!({"items": 7}));
    let read = mirror::read(&odd).expect("the pane still reads");
    assert!(read.graph_runs.is_empty() && read.todo_progress.is_none(), "a graph or checklist that does not read here is left out");
}

/// "remote mirror: client ids are the Mac's and stable per connection" (the Mac's clientIdsAreStablePerConnectionAndValidIdentifiers,
/// with the Mac's own digests: SHA-256 of connection, NUL, host id).
#[test]
fn client_ids_are_the_macs_and_stable_per_connection() {
    let id = mirror::client_id("conn-1", "pane-1");
    assert_eq!(id, "mirror-eb9714c66ef76323034182e28c6ad01f", "the ids are the Mac's");
    assert_eq!(mirror::client_id("conn-1", "ws-project"), "mirror-7525e887f2f07c8b09409bdf8509ed76");
    assert!(
        id == mirror::client_id("conn-1", "pane-1")
            && id != mirror::client_id("conn-2", "pane-1")
            && id != mirror::client_id("conn-1", "pane-2"),
        "stable per connection and host id"
    );
    assert_ne!(mirror::client_id("a", "bc"), mirror::client_id("ab", "c"), "the separator keeps two pairs apart");
    assert!(id.starts_with("mirror-") && id.len() == 39 && mighty_core::wire::identifier(&id), "mirror- and 32 hex digits, an identifier");
}

fn at(text: &str) -> mighty_core::time::DateTimeOffset {
    mighty_core::time::parse(Some(text)).unwrap()
}

/// C#'s `Pane(id, workspace, logs)`: a running host pane with `logs` entries and a run that started at 01:00:01.250.
fn host_pane(id: &str, workspace: &str, logs: usize) -> RunSession {
    RunSession {
        id: Some(id.into()),
        workspace_id: Some(workspace.into()),
        title: Some("Host pane".into()),
        status: Some("running".into()),
        provider: Some("claude".into()),
        logs: Some(
            (0..logs)
                .map(|i| {
                    let kind = if i % 2 == 0 { "user" } else { "assistant" };
                    LogEntry::new(
                        format!("log-{i}"),
                        kind,
                        format!("text {i}"),
                        format!("2026-11-13T01:00:0{}.1234567+00:00", i % 10),
                        None,
                    )
                })
                .collect(),
        ),
        run_timing: Some(AgentRunTiming {
            started_at: at("2026-11-13T01:00:01.250Z"),
            last_observed_at: at("2026-11-13T01:00:09Z"),
            finished_at: None,
            is_approximate: false,
        }),
        ..RunSession::default()
    }
}

fn graph_run(id: &str, input: &str) -> MightyGraphRun {
    MightyGraphRun { id: Some(id.into()), input: Some(input.into()), ..MightyGraphRun::default() }
}

/// "remote mirror: details are bounded and revisions move with them": the host's detail is bounded as the phone's (80 entries, 20 runs,
/// 32 requests, 16 queued of 500 characters, 1.5 MiB) and a revision moves whenever the detail does.
#[test]
fn details_are_bounded_and_revisions_move_with_them() {
    let request = ToolPermissionRequest {
        received_at: Some("2026-11-13T01:00:05.1234567+00:00".into()),
        ..ToolPermissionRequest::new("perm-1", "pane-1", "toolu_1", "Bash", "{\"command\":\"ls\"}", "ls")
    };
    let mut logs = host_pane("pane-1", "ws", 99).logs.unwrap();
    logs.push(LogEntry {
        images: Some(vec![Some(AgentImageRef {
            hash: Some("key".into()),
            media_type: Some("image/png".into()),
            width: 1,
            height: 1,
            bytes: 10,
            source: Some("tool".into()),
            path: None,
        })]),
        ..LogEntry::new("log-image".into(), "image", "[picture]".into(), "2026-11-13T01:00:00Z".into(), None)
    });
    let long = RunSession {
        logs: Some(logs),
        graph_runs: Some(
            (0..25)
                .map(|i| MightyGraphRun {
                    status: Some("completed".into()),
                    provider: Some("claude".into()),
                    ..graph_run(&format!("run-{i}"), &format!("ask {i}"))
                })
                .collect(),
        ),
        ..host_pane("pane-1", "ws", 0)
    };
    let queued: Vec<MirrorQueued> =
        (0..20).map(|i| MirrorQueued { id: format!("queued-{i}"), text: "q".repeat(600), run_now: None }).collect();
    let allowed = ToolPermissionRequest { id: "perm-2".into(), state: "allowed".into(), ..request.clone() };
    let session = mirror::session(&long, &[request.clone(), allowed], &queued, Some("mighty/pane-1"), Some("main"));
    assert!(session.logs.len() == 80 && session.has_older, "the newest 80 entries");
    let last = session.logs.last().unwrap();
    assert!(last.kind() == "system" && last.images.is_none(), "a picture as its line without its reference");
    assert!(session.graph_runs.len() == 20 && session.graph_runs[0].id.as_deref() == Some("run-5"), "the newest 20 runs");
    assert_eq!(session.permissions.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), ["perm-1"], "only waiting requests travel");
    assert!(
        session.queued.len() == 16 && session.queued.iter().all(|q| q.text.chars().count() == 500),
        "16 queued requests of 500 characters"
    );
    assert!(
        session.branch.as_deref() == Some("mighty/pane-1")
            && session.base.as_deref() == Some("main")
            && session.started_at.as_deref() == Some("2026-11-13T01:00:01Z")
            && session.finished_at.is_none()
            && session.created_at.ends_with('Z'),
        "the clock in whole seconds as the Mac's formatter writes it"
    );
    let written = mirror::write(&session);
    assert!(written["logs"][0]["timestamp"].as_str().unwrap().ends_with('Z'));
    assert_eq!(written["permissions"][0]["receivedAt"], "2026-11-13T01:00:05.123Z", "times inside travel in the Mac's forms");
    assert_eq!(written["startedAt"], "2026-11-13T01:00:01Z", "the session's own clock stays whole seconds");
    // The budget: runs go first (one stays), then entries (one stays), then texts are cut, last the graph.
    let big = RunSession {
        logs: Some(
            (0..6)
                .map(|i| LogEntry::new(format!("log-{i}"), "assistant", "a".repeat(100_000), "2026-11-13T01:00:01Z".into(), None))
                .collect(),
        ),
        graph_runs: Some(vec![graph_run("run-a", "x"), graph_run("run-b", "y")]),
        ..host_pane("pane-1", "ws", 0)
    };
    let fitted = mirror::fitted(mirror::session(&big, &[], &[], None, None), 250_000);
    assert!(
        fitted.graph_runs.len() == 1 && fitted.graph_runs[0].id.as_deref() == Some("run-b") && fitted.logs.len() == 2 && fitted.has_older,
        "the oldest runs and entries go first: {}/{}",
        fitted.logs.len(),
        fitted.graph_runs.len()
    );
    let cut = mirror::fitted(mirror::session(&big, &[], &[], None, None), 40_000);
    assert!(cut.logs.len() == 1 && cut.logs[0].text().len() == 32 * 1024 && cut.graph_runs.len() == 1, "then the text, the graph kept");
    let tight = mirror::fitted(mirror::session(&big, &[], &[], None, None), 20_000);
    assert!(tight.logs.len() == 1 && tight.graph_runs.is_empty(), "and last the graph emptied");
    assert_eq!(mirror::utf8_prefix("가나다", 7), "가나", "a cut never splits a character");
    // A revision moves whenever the detail does, and stays while nothing changed.
    let pane = host_pane("pane-1", "ws", 3);
    let revision = mirror::revision(&pane, &[], &[], None, None);
    assert!(
        revision.len() == 24 && revision == mirror::revision(&pane.clone(), &[], &[], None, None),
        "24 hex digits, equal for the same pane"
    );
    let mut streamed = pane.clone();
    streamed.logs.as_mut().unwrap()[2].text = Some("text 2!".into());
    let mut settings = pane.settings.clone().unwrap();
    settings.fast_mode = true;
    let finished = RunSession { run_timing: pane.run_timing.map(|t| mighty_run::timing::finish(t, None)), ..pane.clone() };
    let todos = TodoProgress {
        items: Some(vec![TodoItem {
            content: Some("Read".into()),
            status: Some("in_progress".into()),
            active_form: None,
            id: Some("1".into()),
        }]),
        run_id: None,
    };
    let moved: [(&str, String); 8] = [
        ("a streamed answer", mirror::revision(&streamed, &[], &[], None, None)),
        ("the status", mirror::revision(&RunSession { status: Some("completed".into()), ..pane.clone() }, &[], &[], None, None)),
        ("a request waiting", mirror::revision(&pane, std::slice::from_ref(&request), &[], None, None)),
        ("a queued request", mirror::revision(&pane, &[], &queued[..1], None, None)),
        ("the branch", mirror::revision(&pane, &[], &[], Some("mighty/x"), None)),
        ("the settings", mirror::revision(&RunSession { settings: Some(settings), ..pane.clone() }, &[], &[], None, None)),
        ("the clock", mirror::revision(&finished, &[], &[], None, None)),
        ("the checklist", mirror::revision(&RunSession { todo_progress: Some(todos), ..pane.clone() }, &[], &[], None, None)),
    ];
    for (what, other) in moved {
        assert_ne!(other, revision, "{what} moves the revision");
    }
    let workspaces = [RemoteSharedWorkspace { id: "ws".into(), name: "project".into(), path: "C:\\project".into() }];
    let summary = MirrorSummary {
        id: "pane-1".into(),
        workspace_id: "ws".into(),
        title: "Host pane".into(),
        kind: "claude".into(),
        provider: "claude".into(),
        model: "default".into(),
        status: "running".into(),
        revision: revision.clone(),
        pending: 0,
        queued: 0,
        branch: None,
    };
    let list = mirror::state_revision(&workspaces, std::slice::from_ref(&summary));
    assert_ne!(list, mirror::state_revision(&workspaces, &[MirrorSummary { revision: "other".into(), ..summary.clone() }]));
    let renamed = [RemoteSharedWorkspace { name: "renamed".into(), ..workspaces[0].clone() }];
    assert_ne!(list, mirror::state_revision(&renamed, &[summary]), "the list's revision moves with a pane's and a workspace's name");
}

/// "remote mirror: mirrors are never saved and only this PC's panes count": mirrors live in the state while connected and never reach the
/// state file; the 128-pane limit counts only this PC's panes.
#[test]
fn mirrors_are_never_saved_and_only_this_pcs_panes_count() {
    let temp = Temp::new("saved-state");
    let locale = Locale::new("en");
    let mut store = Store::new(temp.path().to_path_buf(), None, true);
    store.load(&locale).unwrap();
    std::fs::create_dir_all(temp.path().join("project")).unwrap();
    let local = store.approve_local(&temp.path().join("project")).unwrap();
    let far = Workspace {
        id: Some(mirror::client_id("conn", "ws-far")),
        name: Some("far".into()),
        path: Some("/Users/example/far".into()),
        remote: Some(RemoteWorkspaceReference {
            connection_id: Some("conn".into()),
            workspace_id: Some("ws-far".into()),
            host_name: Some("studio-mac".into()),
            version: 2,
            mirror: true,
        }),
        ..Workspace::default()
    };
    let reference = MirrorReference {
        connection_id: "conn".into(),
        host_session_id: "pane-far".into(),
        host_workspace_id: "ws-far".into(),
        host_name: "studio-mac".into(),
    };
    let own: Vec<RunSession> = (0..mighty_core::layout::MAXIMUM_SESSIONS - 1)
        .map(|i| RunSession { id: Some(format!("own-{i}")), workspace_id: local.id.clone(), ..RunSession::default() })
        .collect();
    let mirrored: Vec<RunSession> = (0..40)
        .map(|i| RunSession {
            id: Some(mirror::client_id("conn", &format!("pane-{i}"))),
            workspace_id: far.id.clone(),
            title: Some(format!("far {i}")),
            mirror: Some(MirrorReference { host_session_id: format!("pane-{i}"), ..reference.clone() }),
            ..RunSession::default()
        })
        .collect();
    let mut modes = IndexMap::new();
    modes.insert(far.id().to_string(), Some("tabs".to_string()));
    let snapshot = AppSnapshot {
        workspaces: Some(vec![local.clone(), far.clone()]),
        sessions: Some(own.iter().chain(&mirrored).cloned().map(Arc::new).collect()),
        active_workspace_id: far.id.clone(),
        active_session_id: mirrored[0].id.clone(),
        expanded_workspace_ids: Some(vec![local.id().to_string(), far.id().to_string()]),
        pane_layout_modes: Some(modes),
        ..AppSnapshot::default()
    };
    let normalized = mighty_core::normalize::normalize(&snapshot, false).unwrap();
    assert_eq!(normalized.sessions().len(), own.len() + mirrored.len(), "the state holds the mirror's panes beside this PC's");
    assert_eq!(normalized.sessions().iter().filter(|s| s.mirror.is_some()).count(), 40, "references kept");
    assert!(normalized.workspace(far.id()).unwrap().remote.as_ref().unwrap().mirror, "the workspace's flag kept");
    assert!(
        mighty_core::add_pane::has_room(&normalized) && normalized.own_session_count() == 127,
        "the mirror's panes leave this PC's 128th pane free"
    );
    let mut fuller = normalized.clone();
    fuller.sessions_mut().push(Arc::new(RunSession {
        id: Some("own-last".into()),
        workspace_id: local.id.clone(),
        ..RunSession::default()
    }));
    assert!(!mighty_core::add_pane::has_room(&fuller), "this PC's 129th pane is refused");
    let stripped = snapshot.without_mirrors();
    assert_eq!(stripped.workspaces().iter().map(|w| w.id()).collect::<Vec<_>>(), [local.id()]);
    assert!(stripped.sessions().iter().all(|s| s.mirror.is_none()) && stripped.active_workspace_id.as_deref() == Some(local.id()));
    assert!(stripped.active_session_id.is_none() && stripped.expanded_workspace_ids.as_deref() == Some(&[local.id().to_string()][..]));
    assert!(!stripped.pane_layout_modes.as_ref().unwrap().contains_key(far.id()), "without mirrors: workspaces, panes, layouts, ids");
    assert_eq!(stripped.without_mirrors(), stripped, "a snapshot without mirrors is itself");
    store.save(&snapshot).unwrap();
    let text = std::fs::read_to_string(temp.path().join("workspace-state.json")).unwrap();
    assert!(
        !text.contains("mirror-") && !text.contains("studio-mac") && !text.contains("\"mirror\""),
        "the state file holds nothing of it"
    );
    let mut reloaded = Store::new(temp.path().to_path_buf(), None, true);
    let loaded = reloaded.load(&locale).unwrap();
    assert_eq!(loaded.workspaces().iter().map(|w| w.id()).collect::<Vec<_>>(), [local.id()]);
    assert_eq!(loaded.sessions().len(), own.len(), "a restart has no mirror");
    assert!(
        !mirrored[0].to_json().contains("pane-0") && !mirrored[0].to_json().contains("studio-mac"),
        "a pane's reference is never in the JSON"
    );
    assert!(!far.to_json().contains("\"mirror\""), "a workspace's flag is never in the JSON");
}

// ── the host's routes ────────────────────────────────────────────────────────────────────────────────────────────────────

/// C#'s `FakeMirrorHost(shared, hidden)`: the host's panes as a check hands them over: one shared workspace's two panes and one private
/// pane, each recorded when acted on.
struct FakeMirrorHost {
    shared: String,
    hidden: String,
    actions: Mutex<Vec<String>>,
    mode: Mutex<String>,
    provider: Mutex<String>,
    refuse: Mutex<Option<String>>,
}

impl FakeMirrorHost {
    fn new(shared: &str, hidden: &str) -> Arc<FakeMirrorHost> {
        Arc::new(FakeMirrorHost {
            shared: shared.into(),
            hidden: hidden.into(),
            actions: Mutex::new(vec![]),
            mode: Mutex::new("manual".into()),
            provider: Mutex::new("claude".into()),
            refuse: Mutex::new(None),
        })
    }

    fn summary(&self, id: &str, workspace: &str) -> MirrorSummary {
        let provider = if id == "pane-2" { self.provider.lock().unwrap().clone() } else { "claude".into() };
        MirrorSummary {
            id: id.into(),
            workspace_id: workspace.into(),
            title: format!("Pane {id}"),
            kind: "claude".into(),
            provider,
            model: "default".into(),
            status: "idle".into(),
            revision: format!("rev-{id}{}", self.mode.lock().unwrap()),
            pending: 0,
            queued: 0,
            branch: None,
        }
    }

    fn did(&self, what: String) -> Result<(), Refusal> {
        if let Some(refusal) = self.refuse.lock().unwrap().clone() {
            return Err(refusal);
        }
        self.actions.lock().unwrap().push(what);
        Ok(())
    }

    fn actions(&self) -> Vec<String> {
        self.actions.lock().unwrap().clone()
    }
}

impl MirrorHost for FakeMirrorHost {
    fn summaries(&self, workspace_ids: HashSet<String>) -> BoxFuture<'_, Vec<MirrorSummary>> {
        let all = [self.summary("pane-1", &self.shared), self.summary("pane-2", &self.shared), self.summary("pane-hidden", &self.hidden)];
        Box::pin(async move { all.into_iter().filter(|s| workspace_ids.contains(&s.workspace_id)).collect() })
    }
    fn detail(&self, session_id: String, _: HashSet<String>) -> BoxFuture<'_, Option<MirrorPane>> {
        let pane = (session_id != "pane-hidden").then(|| MirrorPane {
            revision: format!("rev-{session_id}"),
            session: mirror::session(&host_pane(&session_id, &self.shared, 3), &[], &[], None, None),
        });
        Box::pin(async move { pane })
    }
    fn pane_info(&self, session_id: String) -> BoxFuture<'_, Option<MirrorPaneInfo>> {
        let info = |workspace: &str, mode: String, provider: String| MirrorPaneInfo {
            workspace_id: workspace.into(),
            permission_mode: mode,
            provider,
        };
        let found = match session_id.as_str() {
            "pane-1" => Some(info(&self.shared, self.mode.lock().unwrap().clone(), "claude".into())),
            "pane-2" => Some(info(&self.shared, "manual".into(), self.provider.lock().unwrap().clone())),
            "pane-hidden" => Some(info(&self.hidden, "manual".into(), "claude".into())),
            _ => None,
        };
        Box::pin(async move { found })
    }
    fn submit(&self, session_id: String, text: String, attachments: Vec<RunAttachment>) -> BoxFuture<'_, Result<String, Refusal>> {
        let done = self.did(format!("submit {session_id} {text} {}", attachments.len())).map(|()| "queued".to_string());
        Box::pin(async move { done })
    }
    fn stop(&self, session_id: String) -> BoxFuture<'_, Result<(), Refusal>> {
        let done = self.did(format!("stop {session_id}"));
        Box::pin(async move { done })
    }
    fn permission(
        &self,
        session_id: String,
        request_id: String,
        run_id: String,
        allow: bool,
        always: bool,
    ) -> BoxFuture<'_, Result<(), Refusal>> {
        let done = self.did(format!("permission {session_id} {request_id} {run_id} {allow} {always}"));
        Box::pin(async move { done })
    }
    fn answers(
        &self,
        session_id: String,
        request_id: String,
        _: String,
        answers: IndexMap<String, UserQuestionAnswer>,
    ) -> BoxFuture<'_, Result<(), Refusal>> {
        let keys = answers.keys().cloned().collect::<Vec<_>>().join(",");
        let done = self.did(format!("answers {session_id} {request_id} {keys}"));
        Box::pin(async move { done })
    }
    fn plan(&self, session_id: String, request_id: String, _: String, decision: PlanDecision) -> BoxFuture<'_, Result<(), Refusal>> {
        let done = self.did(format!("plan {session_id} {request_id} {}", decision.kind));
        Box::pin(async move { done })
    }
    fn create_session(&self, workspace_id: String, provider: String) -> BoxFuture<'_, Result<String, Refusal>> {
        let done = self.did(format!("create {workspace_id} {provider}")).map(|()| "pane-3".to_string());
        Box::pin(async move { done })
    }
    fn close(&self, session_id: String) -> BoxFuture<'_, Result<(), Refusal>> {
        let done = self.did(format!("close {session_id}"));
        Box::pin(async move { done })
    }
}

/// C#'s `Ask`: one request to the host; a refusal as its status and `{error}`.
async fn ask(port: u16, key: &str, method: Method, path: &str, body: Option<&str>) -> (u16, Value) {
    let request = Request { body: body.map(str::as_bytes), ..Request::default() };
    match transport::request(&loopback(port), key, method, path, request).await {
        Ok(data) => (200, serde_json::from_slice(&data).unwrap_or(Value::Null)),
        Err(TransportError::Status(failure)) => (failure.status, json!({"error": failure.message})),
        Err(other) => panic!("{path}: {other:?}"),
    }
}

fn error_of(answer: &(u16, Value)) -> &str {
    answer.1["error"].as_str().unwrap_or_default()
}

/// "remote mirror: the host serves only shared agent panes" (the Mac's serveMirror over loopback): only the shared workspaces' agent
/// panes, since and unchanged, 404 for a pane outside them, the switch's 403s (full access, a Windows host's Codex and Antigravity,
/// always), 404 before the app hands its panes over, the bodies read strictly, a send's 12 MiB body.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_host_serves_only_shared_agent_panes() {
    let fixture = make_host(MakeHost { windows_host: Some(true), ..MakeHost::default() }).await;
    let state = fixture.host.start(&[fixture.shared.id().to_string()], 0).await.unwrap();
    let (port, key) = (state.port.unwrap(), state.key.clone().unwrap());
    let before = ask(port, &key, Method::Get, "/v2/mirror", None).await;
    assert!(
        before.0 == 404 && error_of(&before) == get("remote.mirror.error.unavailable"),
        "no mirror before the app hands its panes over"
    );
    let host = FakeMirrorHost::new(fixture.shared.id(), fixture.private.id());
    fixture.host.set_mirror(Some(host.clone()));
    let list = ask(port, &key, Method::Get, "/v2/mirror", None).await;
    let listed = WireMirrorState::decode(list.1.to_string().as_bytes()).unwrap();
    assert!(list.0 == 200 && mirror::valid_state(&listed) && listed.host_name == "studio-pc");
    assert_eq!(listed.workspaces.iter().map(|w| w.id.as_str()).collect::<Vec<_>>(), [fixture.shared.id()]);
    assert_eq!(listed.sessions.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["pane-1", "pane-2"], "only the shared panes are listed");
    let same_list = ask(port, &key, Method::Get, &format!("/v2/mirror?since={}", listed.revision), None).await;
    assert!(
        same_list.0 == 200
            && same_list.1["unchanged"] == json!(true)
            && same_list.1["sessions"].as_array().unwrap().is_empty()
            && same_list.1["revision"] == json!(listed.revision),
        "a current since is answered unchanged with empty lists"
    );
    assert_eq!(ask(port, &key, Method::Get, "/v2/mirror?since=00", None).await.1["sessions"].as_array().unwrap().len(), 2);
    for query in ["?since=XYZ", &format!("?since={}", "a".repeat(65)), "?other=1", "?since=ab?x=1"] {
        assert_eq!(ask(port, &key, Method::Get, &format!("/v2/mirror{query}"), None).await.0, 400, "not one since in hex: {query}");
    }
    let detail = ask(port, &key, Method::Get, "/v2/mirror/sessions/pane-1", None).await;
    let read = WireMirrorDetail::decode(detail.1.to_string().as_bytes()).unwrap();
    assert!(detail.0 == 200 && mirror::valid_detail(&read, &mirror::read(&read.session).unwrap(), "pane-1"), "a shared pane's detail");
    for (method, path) in [
        (Method::Get, "/v2/mirror/sessions/pane-hidden"),
        (Method::Get, "/v2/mirror/sessions/nope"),
        (Method::Post, "/v2/mirror/sessions/pane-hidden/stop"),
    ] {
        assert_eq!(ask(port, &key, method, path, Some("{}")).await.0, 404, "a pane outside the shared workspaces is unknown: {path}");
    }
    assert_eq!(ask(port, &key, Method::Get, "/v2/mirror/sessions/pane-1?x=1", None).await.0, 400, "no query on a pane's route");
    assert_eq!(ask(port, &key, Method::Post, "/v2/mirror/sessions/pane-1/rename", Some("{}")).await.0, 404, "an unknown action");
    // MC-49: an app whose MirrorHost does not take the queue's actions (an older one's) says so in its details and refuses them with its
    // words; an item that is no identifier, another queue action and a pane outside the shared workspaces are unknown.
    assert!(!mirror::read(&read.session).unwrap().queue_actions, "no queueActions from such an app");
    for action in ["remove", "run-now"] {
        let refused = ask(port, &key, Method::Post, &format!("/v2/mirror/sessions/pane-1/queue/queued-1/{action}"), Some("{}")).await;
        assert!(refused.0 == 409 && error_of(&refused) == get("remote.mirror.error.unavailable"), "{action}: {refused:?}");
    }
    for path in [
        "/v2/mirror/sessions/pane-1/queue/-x/remove",
        "/v2/mirror/sessions/pane-1/queue/queued-1/edit",
        "/v2/mirror/sessions/pane-1/queue/queued-1",
        "/v2/mirror/sessions/pane-hidden/queue/queued-1/remove",
    ] {
        assert_eq!(ask(port, &key, Method::Post, path, Some("{}")).await.0, 404, "unknown: {path}");
    }

    let sent = ask(port, &key, Method::Post, "/v2/mirror/sessions/pane-1/submit", Some(r#"{"text":"Then update CHANGELOG.md"}"#)).await;
    assert_eq!(sent.1["outcome"], "queued", "a send answers the host's word");
    let with_file = ask(port, &key, Method::Post, "/v2/mirror/sessions/pane-1/submit", Some(&node("submit.json").to_string())).await;
    assert!(
        with_file.0 == 200 && host.actions().contains(&"submit pane-1 Then update CHANGELOG.md 1".to_string()),
        "a send carries its files"
    );
    let attachment = node("submit.json")["attachments"][0].to_string();
    let long = format!(r#"{{"text":"{}","attachments":[{attachment}]}}"#, "x".repeat(400_001));
    for bad in [r#"{"text":"  "}"#, r#"{"text":"a\u0000b"}"#, "{}", long.as_str()] {
        let answer = ask(port, &key, Method::Post, "/v2/mirror/sessions/pane-1/submit", Some(bad)).await;
        assert_eq!(answer.0, 400, "a send outside the rules is refused: {}", &bad[..bad.len().min(40)]);
    }
    let mut bad_file = node("submit.json");
    bad_file["attachments"][0]["mediaType"] = json!("image/png");
    assert_eq!(
        ask(port, &key, Method::Post, "/v2/mirror/sessions/pane-1/submit", Some(&bad_file.to_string())).await.0,
        400,
        "a mislabelled file"
    );
    *host.mode.lock().unwrap() = "fullAccess".into();
    let full = ask(port, &key, Method::Post, "/v2/mirror/sessions/pane-1/submit", Some(r#"{"text":"hi"}"#)).await;
    assert!(full.0 == 403 && error_of(&full) == get("remote.host.error.fullAccessOff"), "a full-access pane waits for the switch");
    let run_now = ask(port, &key, Method::Post, "/v2/mirror/sessions/pane-1/queue/queued-1/run-now", Some("{}")).await;
    assert!(
        run_now.0 == 403 && error_of(&run_now) == get("remote.host.error.fullAccessOff"),
        "바로 실행 starts a request: the same switch"
    );
    let removal = ask(port, &key, Method::Post, "/v2/mirror/sessions/pane-1/queue/queued-1/remove", Some("{}")).await;
    assert_eq!(removal.0, 409, "a removal runs nothing and is the app's to refuse, not the switch's");
    *host.provider.lock().unwrap() = "codex".into();
    let codex = ask(port, &key, Method::Post, "/v2/mirror/sessions/pane-2/submit", Some(r#"{"text":"hi"}"#)).await;
    assert!(codex.0 == 403 && error_of(&codex) == strings::host_agent_needs_full_access("codex"), "a Windows host's Codex pane waits");
    let info =
        |mode: &str, provider: &str| MirrorPaneInfo { workspace_id: "ws".into(), permission_mode: mode.into(), provider: provider.into() };
    assert_eq!(RemoteHost::mirror_refusal(&info("manual", "gemini"), false, true), Some(strings::host_agent_needs_full_access("gemini")));
    assert_eq!(
        RemoteHost::mirror_refusal(&info("manual", "gemini"), false, false),
        Some(strings::mac_host_agent_needs_full_access("gemini")),
        "nothing confines Antigravity on a Mac either"
    );
    assert!(
        RemoteHost::mirror_refusal(&info("manual", "codex"), false, false).is_none()
            && RemoteHost::mirror_refusal(&info("acceptEdits", "claude"), false, true).is_none(),
        "a Mac host's Codex runs in its sandbox, and Claude keeps its modes"
    );
    // The answers carry a refused pane's run on as a send would: an allow, an answer and a plan's decision wait for the switch too
    // (a decline does not).
    let codex_answers = [
        ("permission", r#"{"requestId":"perm-6","runId":"pane-2","allow":true}"#.to_string()),
        ("questions", node("questions.json").to_string()),
        ("plan", node("plan.json").to_string()),
    ];
    for (route, answer) in &codex_answers {
        let refused = ask(port, &key, Method::Post, &format!("/v2/mirror/sessions/pane-2/{route}"), Some(answer)).await;
        assert!(
            refused.0 == 403 && error_of(&refused) == strings::host_agent_needs_full_access("codex"),
            "a Windows host's Codex pane takes no {route} answer without the switch: {refused:?}"
        );
    }
    let decline = r#"{"requestId":"perm-7","runId":"pane-2","allow":false}"#;
    assert_eq!(ask(port, &key, Method::Post, "/v2/mirror/sessions/pane-2/permission", Some(decline)).await.0, 200, "a decline");
    let always_body = r#"{"requestId":"perm-1","runId":"pane-1","allow":true,"always":true}"#;
    let always = ask(port, &key, Method::Post, "/v2/mirror/sessions/pane-1/permission", Some(always_body)).await;
    assert!(always.0 == 403 && error_of(&always) == get("remote.host.error.antigravityAlways"), "항상 허용 waits for the switch");
    fixture.host.set_allows_full_access(true);
    assert_eq!(ask(port, &key, Method::Post, "/v2/mirror/sessions/pane-1/submit", Some(r#"{"text":"full"}"#)).await.0, 200);
    assert_eq!(ask(port, &key, Method::Post, "/v2/mirror/sessions/pane-2/submit", Some(r#"{"text":"codex"}"#)).await.0, 200, "the switch");
    assert_eq!(
        ask(port, &key, Method::Post, "/v2/mirror/sessions/pane-1/permission", Some(&node("permission.json").to_string())).await.0,
        200
    );
    let always_body = r#"{"requestId":"perm-5","runId":"pane-1","allow":true,"always":true}"#;
    assert_eq!(
        ask(port, &key, Method::Post, "/v2/mirror/sessions/pane-1/permission", Some(always_body)).await.0,
        200,
        "answers reach the app"
    );
    assert_eq!(
        ask(port, &key, Method::Post, "/v2/mirror/sessions/pane-1/questions", Some(&node("questions.json").to_string())).await.0,
        200
    );
    assert_eq!(ask(port, &key, Method::Post, "/v2/mirror/sessions/pane-1/plan", Some(&node("plan.json").to_string())).await.0, 200);
    let revised = r#"{"runId":"j","requestId":"r","outcome":"revised"}"#;
    assert_eq!(ask(port, &key, Method::Post, "/v2/mirror/sessions/pane-1/plan", Some(revised)).await.0, 400, "a revise without words");
    assert_eq!(ask(port, &key, Method::Post, "/v2/mirror/sessions/pane-1/stop", Some("")).await.0, 200, "a stop reads no body");
    assert_eq!(ask(port, &key, Method::Post, "/v2/mirror/sessions/pane-1/close", Some("{}")).await.0, 200, "nor a close");
    let made_path = format!("/v2/mirror/workspaces/{}/sessions", fixture.shared.id());
    let created = ask(port, &key, Method::Post, &made_path, Some(&node("create.json").to_string())).await;
    assert!(created.0 == 200 && created.1["sessionId"] == "pane-3", "a new pane in a shared workspace answers its id");
    let private_path = format!("/v2/mirror/workspaces/{}/sessions", fixture.private.id());
    assert_eq!(ask(port, &key, Method::Post, &private_path, Some(r#"{"provider":"claude"}"#)).await.0, 403, "a private workspace");
    assert_eq!(ask(port, &key, Method::Post, &made_path, Some(r#"{"provider":"other"}"#)).await.0, 400, "an unknown agent is refused");
    let actions = host.actions();
    for expected in [
        "permission pane-1 perm-4 job-1 true false".to_string(),
        "permission pane-1 perm-5 pane-1 true true".to_string(),
        "answers pane-1 perm-2 Which file?".to_string(),
        "plan pane-1 perm-3 revise".to_string(),
        "stop pane-1".to_string(),
        "close pane-1".to_string(),
        format!("create {} claude", fixture.shared.id()),
    ] {
        assert!(actions.contains(&expected), "every action reached the app with the client's ids: {expected} in {actions:?}");
    }
    *host.refuse.lock().unwrap() = Some("Request is no longer pending.".into());
    let refused = ask(port, &key, Method::Post, "/v2/mirror/sessions/pane-1/stop", Some("{}")).await;
    assert!(refused.0 == 409 && error_of(&refused) == "Request is no longer pending.", "what the app refuses is a conflict with its words");
    // The 12 MiB body only for a send, decided from the head.
    let head = |target: &str| HttpRequest {
        method: "POST".into(),
        target: target.into(),
        headers: [("authorization".to_string(), format!("Bearer {key}")), ("x-mighty-remote-version".to_string(), "2".to_string())]
            .into_iter()
            .collect(),
        body: vec![],
        remote_address: "127.0.0.1".into(),
    };
    assert_eq!(host::body_limit(&head("/v2/mirror/sessions/pane-1/submit"), &key, true), mighty_run::attachments::MAXIMUM_REQUEST_BYTES);
    assert_eq!(
        host::body_limit(&head("/v2/mirror/sessions/pane-1/stop"), &key, true),
        mighty_remote::remote::wire::protocol::MAXIMUM_REQUEST_BYTES
    );
    assert_eq!(
        host::body_limit(&head("/v2/mirror/sessions/bad id/submit"), &key, true),
        mighty_remote::remote::wire::protocol::MAXIMUM_REQUEST_BYTES,
        "only a send gets the attachment body"
    );
    assert!(
        routes::is_submit("/v2/mirror/sessions/pane-1/submit")
            && !routes::is_submit("/v2/mirror/sessions/pane-1/submit?x")
            && !routes::is_submit("/v2/mirror/workspaces/a/submit"),
        "the send's route is exactly it"
    );
    // Over 512 KiB without files: refused even though the listener read it.
    let padded = format!(r#"{{"text":"{}","pad":"{}"}}"#, "x".repeat(390_000), " ".repeat(200_000));
    let status = ask(port, &key, Method::Post, "/v2/mirror/sessions/pane-1/submit", Some(&padded)).await.0;
    assert!(status == 400 || status == 413, "a body past 512 KiB without files is refused: {status}");
    fixture.host.set_mirror(None);
    assert_eq!(ask(port, &key, Method::Get, "/v2/mirror", None).await.0, 404, "taken back, the mirror is gone");
    fixture.dispose().await;
}

// ── a client and a host end to end ─────────────────────────────────────────────────────────────────────────────────────────

/// The client's state as the mirror reaches it: the client's run service, with the cards and failures recorded.
struct Stage {
    service: Arc<RunService>,
    permissions: Mutex<Vec<ToolPermissionRequest>>,
    failures: Mutex<Vec<(String, String)>>,
}

impl MirrorStage for Stage {
    fn snapshot(&self) -> AppSnapshot {
        self.service.snapshot()
    }
    fn change(&self, change: &mut dyn FnMut(&AppSnapshot) -> Option<AppSnapshot>) -> bool {
        self.service.mirror_update(change)
    }
    fn permission(&self, request: ToolPermissionRequest) {
        self.permissions.lock().unwrap().push(request.clone());
        self.service.mirror_permission(request);
    }
    fn changed(&self) {}
    fn failed(&self, pane_id: &str, words: &str) {
        self.failures.lock().unwrap().push((pane_id.into(), words.into()));
    }
}

fn spawn() -> Spawn {
    Arc::new(|future| {
        tokio::spawn(future);
    })
}

/// A sharing host over a world's run service (C#'s `DesktopService.HostOptions` over the host's own state), on loopback.
fn host_over(world: &World) -> RemoteHost {
    let (workspaces, resolving) = (world.desk.service.clone(), world.desk.service.clone());
    RemoteHost::new(HostOptions {
        workspaces: Arc::new(move || workspaces.snapshot().workspaces().to_vec()),
        resolve_local: Arc::new(move |id: String| {
            let found = resolving.resolve_shared(&id);
            Box::pin(async move { found })
        }),
        runtime: Arc::new(|| {
            let value = runtime("info.json");
            Box::pin(async move { value })
        }),
        create_runner: Arc::new(|sinks| {
            let runner: Arc<dyn mighty_remote::remote::host::HostRunner> = FakeRunner::new(sinks);
            runner
        }),
        host_name: "studio-pc".into(),
        admission: Arc::new(|_| None),
        runs_changed: Arc::new(|_| {}),
        allow_loopback_for_tests: true,
        heartbeat: mighty_remote::remote::wire::protocol::HEARTBEAT_TIMEOUT,
        addresses: None,
        clock: host::monotonic(),
        windows_host: true,
        decodes_paths: false,
        graph_runs: true,
        antigravity_rules: None,
        system_folder: None,
        network: Arc::new(mighty_platform::fake::FakeNetwork::default()),
    })
}

/// "remote mirror: a Windows client mirrors a Windows host": connecting adds the shared workspace and its agent panes (deterministic ids,
/// never the private workspace's, never a command pane), the detail of the pane looked at comes with its waiting request as a card, sends,
/// answers, stops, a new pane and a close go to the host through the phone's handlers, nothing reaches the state file, and a disconnect
/// stops the mirror while the panes stay.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_windows_client_mirrors_a_windows_host() {
    // The host: this PC's own panes, the phone's router with a desktop action that records what it is asked.
    let host_temp = Temp::new("shared-host");
    std::fs::create_dir_all(host_temp.path().join("project")).unwrap();
    std::fs::create_dir_all(host_temp.path().join("private")).unwrap();
    let shared = support::desk::workspace("shared-ws", &host_temp.path().join("project"));
    let hidden = support::desk::workspace("hidden-ws", &host_temp.path().join("private"));
    let host_state = AppSnapshot {
        workspaces: Some(vec![shared.clone(), hidden.clone()]),
        sessions: Some(
            [
                RunSession { status: Some("idle".into()), run_timing: None, ..host_pane("host-1", "shared-ws", 3) },
                RunSession { kind: Some("shell".into()), ..support::desk::pane("host-cmd", "shared-ws") },
                support::desk::pane("host-hidden", "hidden-ws"),
            ]
            .into_iter()
            .map(Arc::new)
            .collect(),
        ),
        ..AppSnapshot::default()
    };
    let host_state = mighty_core::normalize::normalize(&host_state, false).unwrap();
    let world = World::over(host_temp, host_state, "host-fixture", "Fixture", |_| {});
    let host_service = world.desk.service.clone();
    let acted = host_service.clone();
    world.desk.calls().action = Some(Arc::new(move |action: &MobileDesktopAction| match action.action.as_str() {
        "submit" => Ok(ActionAnswer::Receipt(mighty_composer::pending::SubmitReceipt::new("queued"))),
        "create" => {
            let provider = action.body["provider"].as_str().unwrap_or("claude").to_string();
            let workspace = action.session_id.clone();
            acted
                .update(|s| {
                    let mut next = s.clone();
                    next.sessions_mut()
                        .push(Arc::new(RunSession { provider: Some(provider.clone()), ..support::desk::pane("host-new", &workspace) }));
                    next
                })
                .unwrap();
            Ok(ActionAnswer::Value(json!("host-new")))
        }
        "close" => {
            let id = action.session_id.clone();
            acted
                .update(|s| {
                    let mut next = s.clone();
                    next.sessions_mut().retain(|p| p.id() != id);
                    next
                })
                .unwrap();
            Ok(ActionAnswer::Value(Value::Null))
        }
        _ => Ok(ActionAnswer::Value(Value::Null)),
    }));
    let queued: Arc<Mutex<std::collections::HashMap<String, Vec<MirrorQueued>>>> = Arc::default();
    let (snapshot_of, router, queue) = (host_service.clone(), world.router.clone(), queued.clone());
    let sharing_host = host_over(&world);
    sharing_host.set_mirror(Some(Arc::new(DeskMirrorHost::new(
        Arc::new(move || snapshot_of.snapshot()),
        Arc::new(move || Some(router.clone())),
        Arc::new(move |id| queue.lock().unwrap().get(id).cloned().unwrap_or_default()),
        Arc::new(|_| false),
    ))));
    let sharing = sharing_host.start(&["shared-ws".to_string()], 0).await.unwrap();

    // The client.
    let (store, directory) = client_store();
    std::fs::create_dir_all(directory.join("near")).unwrap();
    let near = store.lock().unwrap().approve_local(&directory.join("near")).unwrap();
    let client_state = AppSnapshot { workspaces: Some(vec![near.clone()]), ..AppSnapshot::default() };
    let client_world =
        World::over(Temp::new("client-pc"), mighty_core::normalize::normalize(&client_state, false).unwrap(), "c", "c", |_| {});
    let service = client_world.desk.service.clone();
    let seen = Arc::new(Collected::default());
    let client = client_with(&store, &directory, Arc::new(MemoryVault::default()), &seen, loopback_options());
    let stage = Arc::new(Stage { service: service.clone(), permissions: Mutex::new(vec![]), failures: Mutex::new(vec![]) });
    let desk = MirrorDesk::new(client.clone(), stage.clone(), spawn());
    desk.set_intervals(Duration::from_millis(100), Duration::from_millis(200));
    desk.attach();
    service.set_mirror(Arc::new(desk.clone()));
    let info = client.connect("", &format!("127.0.0.1:{}", sharing.port.unwrap()), sharing.key.as_deref().unwrap()).await.unwrap();
    let workspace_id = mirror::client_id(&info.id, "shared-ws");
    let pane_id = mirror::client_id(&info.id, "host-1");
    until(|| service.snapshot().session(&pane_id).is_some(), 10_000, "the host's pane shows").await;
    let state = service.snapshot();
    let workspace = state.workspace(&workspace_id).unwrap();
    let remote = workspace.remote.as_ref().unwrap();
    assert!(
        remote.mirror
            && remote.connection_id.as_deref() == Some(info.id.as_str())
            && remote.workspace_id.as_deref() == Some("shared-ws")
            && remote.host_name.as_deref() == Some("studio-pc")
            && workspace.path() == shared.path(),
        "connecting adds the shared workspace as a mirror"
    );
    let mirrored: Vec<&str> = state.sessions().iter().filter(|s| s.mirror.is_some()).map(|s| s.id()).collect();
    assert_eq!(mirrored, [pane_id.as_str()], "only the shared workspace's agent pane, under its deterministic id");
    assert_eq!(
        state.session(&pane_id).unwrap().mirror,
        Some(MirrorReference {
            connection_id: info.id.clone(),
            host_session_id: "host-1".into(),
            host_workspace_id: "shared-ws".into(),
            host_name: "studio-pc".into()
        })
    );
    assert!(desk.link(&info.id).is_some_and(|l| l.phase == "live") && desk.is_mirror_workspace(&workspace_id), "the mirror is live");

    // A waiting request on the host comes with the pane's detail once the user looks at it.
    let request = ToolPermissionRequest {
        received_at: Some("2026-11-13T01:00:05.000Z".into()),
        ..ToolPermissionRequest::new("perm-1", "host-1", "toolu_1", "Bash", "{\"command\":\"ls\"}", "ls")
    };
    world.desk.calls().permissions.push(request.clone());
    queued
        .lock()
        .unwrap()
        .insert("host-1".into(), vec![MirrorQueued { id: "queued-1".into(), text: "Then update CHANGELOG.md".into(), run_now: None }]);
    host_service
        .update(|s| {
            let mut next = s.clone();
            for pane in next.sessions_mut().iter_mut().filter(|p| p.id() == "host-1") {
                let mut changed = (**pane).clone();
                changed.status = Some("running".into());
                changed.logs.get_or_insert_with(Vec::new).push(LogEntry::new(
                    "log-new".into(),
                    "assistant",
                    "Reading the file.".into(),
                    mighty_core::time::now_o(),
                    None,
                ));
                *pane = Arc::new(changed);
            }
            next
        })
        .unwrap();
    service
        .update(|s| AppSnapshot { active_workspace_id: Some(workspace_id.clone()), active_session_id: Some(pane_id.clone()), ..s.clone() })
        .unwrap();
    let card_shown = || stage.permissions.lock().unwrap().iter().any(|p| p.run_id == pane_id && p.id == "perm-1" && p.state == "pending");
    let entry_shown = || service.snapshot().session(&pane_id).is_some_and(|p| p.logs().iter().any(|l| l.id() == "log-new"));
    until(|| card_shown() && entry_shown(), 10_000, "the detail and its waiting request").await;
    let counts = desk.counts(&pane_id).unwrap();
    assert!(
        service.snapshot().session(&pane_id).unwrap().status() == "running"
            && counts.pending == 1
            && counts.queued == 1
            && counts.queued_texts().as_deref() == Some(&["Then update CHANGELOG.md".to_string()][..]),
        "the detail, the waiting request and the queue are the pane's: {counts:?}"
    );

    // Actions go to the host through the phone's handlers. The answer goes to the host's phone handler, which refuses it there (the
    // fixture's pane runs nothing): its words come back.
    service.respond_to_tool_permission(&pane_id, "perm-1", true).await.unwrap();
    until(
        || stage.failures.lock().unwrap().iter().any(|(p, m)| *p == pane_id && m == "Request is no longer pending."),
        5_000,
        "the host's refusal is said",
    )
    .await;
    let host_actions = || {
        world
            .desk
            .actions()
            .iter()
            .map(|a| format!("{} {} {}", a.action, a.session_id, a.body["text"].as_str().or(a.body["provider"].as_str()).unwrap_or("")))
            .collect::<Vec<_>>()
    };
    assert_eq!(desk.submit(&pane_id, "Then update CHANGELOG.md", vec![]).await.unwrap(), "queued");
    assert!(host_actions().contains(&"submit host-1 Then update CHANGELOG.md".to_string()), "a send is the host pane's next request");
    service.start(mighty_run::model::StartRunRequest::new(&pane_id, &workspace_id, "claude", "via start")).await.unwrap();
    assert!(
        host_actions().contains(&"submit host-1 via start".to_string()) && !service.is_session_running(&pane_id),
        "a start of a mirrored pane is a send to its host, nothing runs here"
    );
    service.stop(&pane_id).await.unwrap();
    assert!(host_actions().iter().any(|a| a.starts_with("stop host-1")), "a stop reaches the host");
    let made = desk.create_pane(&workspace_id, "claude", "codex").await.unwrap();
    assert_eq!(made, Some(mirror::client_id(&info.id, "host-new")), "a new pane is made on the host and shows here");
    assert!(host_actions().contains(&"create shared-ws codex".to_string()));
    assert_eq!(
        desk.create_pane(&workspace_id, "shell", "claude").await,
        Err(get("remote.mirror.error.commandPane")),
        "no command pane there"
    );
    let made = made.unwrap();
    desk.close_pane(&made).await.unwrap();
    assert!(
        host_actions().iter().any(|a| a.starts_with("close host-new")) && service.snapshot().session(&made).is_none(),
        "a close closes the host's pane and it leaves here"
    );
    let locale = Locale::new("en");
    service.update(|s| mighty_core::rename::rename_session(s, &pane_id, "mine now", &locale).unwrap()).unwrap();
    assert_ne!(service.snapshot().session(&pane_id).unwrap().title(), "mine now", "a mirrored pane keeps its host's name");

    // Nothing of the mirror reaches the client's state file.
    store.lock().unwrap().save(&service.snapshot()).unwrap();
    let saved = std::fs::read_to_string(directory.join("workspace-state.json")).unwrap();
    assert!(
        saved.contains(near.id()) && !saved.contains(&workspace_id) && !saved.contains(&pane_id) && !saved.contains("Reading the file."),
        "the state file holds this PC's workspace and nothing of the host's"
    );

    // A disconnect stops the mirror; the panes stay as last shown and sends wait for a new connect.
    client.disconnect(&info.id).await;
    until(|| desk.link(&info.id).is_some_and(|l| l.phase == "off"), 10_000, "the mirror stops").await;
    let reference = desk.reference(&pane_id).expect("the pane stays");
    assert_eq!(
        desk.blocked_reason(&reference),
        Some(get_with("remote.mirror.error.disconnected", &[("host", "studio-pc")])),
        "the panes say their computer is not connected"
    );
    assert!(desk.submit(&pane_id, "late", vec![]).await.is_err(), "a send waits for a new connect");
    // Removing the connection takes its mirror away.
    client.remove(&info.id).await;
    until(
        || service.snapshot().workspace(&workspace_id).is_none() && service.snapshot().session(&pane_id).is_none(),
        10_000,
        "the mirror goes with the connection",
    )
    .await;
    assert!(
        stage.permissions.lock().unwrap().iter().any(|p| p.id == "perm-1" && p.run_id == pane_id && p.state != "pending"),
        "its cards are withdrawn"
    );
    assert!(client.connection(&info.id).is_none());
    desk.dispose();
    sharing_host.dispose().await;
}

/// MC-49: a mirrored pane's queue is its host's. The host's composer queue (added to, emptied as a run takes its head, added to again)
/// reaches the client's pane with each row's id, text and what 바로 실행 does there, through the list's and the detail's revisions; ×
/// and 바로 실행 from the client go to the host's phone handlers (`queue-remove`, `queue-run-now`) and the row leaves here at once; before
/// the host said it takes them (no detail read yet, or an older host) they are refused without asking it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_mirrored_panes_queue_is_its_hosts() {
    let host_temp = Temp::new("queue-host");
    std::fs::create_dir_all(host_temp.path().join("project")).unwrap();
    let shared = support::desk::workspace("shared-ws", &host_temp.path().join("project"));
    let host_state = AppSnapshot {
        workspaces: Some(vec![shared.clone()]),
        sessions: Some(vec![Arc::new(RunSession { status: Some("running".into()), ..host_pane("host-1", "shared-ws", 3) })]),
        ..AppSnapshot::default()
    };
    let host_state = mighty_core::normalize::normalize(&host_state, false).unwrap();
    let world = World::over(host_temp, host_state, "host-fixture", "Fixture", |_| {});
    let host_service = world.desk.service.clone();
    // The host pane's composer queue, as the app's `queued` closure reads it; the phone's queue handlers change it.
    let queued: Arc<Mutex<std::collections::HashMap<String, Vec<MirrorQueued>>>> = Arc::default();
    let row =
        |id: &str, text: &str, run_now: Option<&str>| MirrorQueued { id: id.into(), text: text.into(), run_now: run_now.map(Into::into) };
    let handled = queued.clone();
    world.desk.calls().action = Some(Arc::new(move |action: &MobileDesktopAction| {
        let item = action.body["id"].as_str().unwrap_or_default().to_string();
        let mut queue = handled.lock().unwrap();
        let rows = queue.entry(action.session_id.clone()).or_default();
        match action.action.as_str() {
            "queue-remove" | "queue-run-now" if !rows.iter().any(|q| q.id == item) => {
                Err(mighty_remote::phone::desk::MobileError::request(404, "Queued request not found.", None))
            }
            "queue-remove" => {
                rows.retain(|q| q.id != item);
                Ok(ActionAnswer::Value(Value::Null))
            }
            "queue-run-now" => {
                rows.retain(|q| q.id != item);
                Ok(ActionAnswer::Value(json!("here")))
            }
            _ => Ok(ActionAnswer::Value(Value::Null)),
        }
    }));
    let (snapshot_of, router, queue) = (host_service.clone(), world.router.clone(), queued.clone());
    let sharing_host = host_over(&world);
    sharing_host.set_mirror(Some(Arc::new(DeskMirrorHost::new(
        Arc::new(move || snapshot_of.snapshot()),
        Arc::new(move || Some(router.clone())),
        Arc::new(move |id| queue.lock().unwrap().get(id).cloned().unwrap_or_default()),
        Arc::new(|_| false),
    ))));
    let sharing = sharing_host.start(&["shared-ws".to_string()], 0).await.unwrap();

    // The client looks at a workspace of its own: no detail of the host's pane is read until it looks at the mirror's.
    let (store, directory) = client_store();
    std::fs::create_dir_all(directory.join("near")).unwrap();
    let near = store.lock().unwrap().approve_local(&directory.join("near")).unwrap();
    let client_state = AppSnapshot { workspaces: Some(vec![near]), ..AppSnapshot::default() };
    let client_state = mighty_core::normalize::normalize(&client_state, false).unwrap();
    let client_world = World::over(Temp::new("queue-client"), client_state, "c", "c", |_| {});
    let service = client_world.desk.service.clone();
    let seen = Arc::new(Collected::default());
    let client = client_with(&store, &directory, Arc::new(MemoryVault::default()), &seen, loopback_options());
    let stage = Arc::new(Stage { service: service.clone(), permissions: Mutex::new(vec![]), failures: Mutex::new(vec![]) });
    let desk = MirrorDesk::new(client.clone(), stage.clone(), spawn());
    desk.set_intervals(Duration::from_millis(100), Duration::from_millis(200));
    desk.attach();
    service.set_mirror(Arc::new(desk.clone()));
    let info = client.connect("", &format!("127.0.0.1:{}", sharing.port.unwrap()), sharing.key.as_deref().unwrap()).await.unwrap();
    let workspace_id = mirror::client_id(&info.id, "shared-ws");
    let pane_id = mirror::client_id(&info.id, "host-1");
    until(|| service.snapshot().session(&pane_id).is_some(), 10_000, "the host's pane shows").await;

    // Before a detail said the host takes the queue's actions, they are refused here and nothing reaches the host.
    assert_eq!(
        desk.queue_action(&pane_id, "queued-1", QueueAction::Remove).await,
        Err(get("remote.mirror.error.unavailable")),
        "no queue action before the host said it takes them"
    );
    assert!(world.desk.actions().is_empty());

    // The host queues two requests while its pane runs; the user looks at the mirror workspace.
    queued.lock().unwrap().insert(
        "host-1".into(),
        vec![row("queued-1", "Run the tests", Some("worktree")), row("queued-2", "Then lint", Some("waitsForTurn"))],
    );
    service
        .update(|s| AppSnapshot { active_workspace_id: Some(workspace_id.clone()), active_session_id: Some(pane_id.clone()), ..s.clone() })
        .unwrap();
    let items = || desk.counts(&pane_id).and_then(|c| c.queued_items).unwrap_or_default();
    let ids = || items().iter().map(|q| q.id.clone()).collect::<Vec<_>>();
    until(|| ids() == ["queued-1", "queued-2"], 10_000, "the host's queue reaches the client").await;
    let counts = desk.counts(&pane_id).unwrap();
    assert!(counts.queued == 2 && counts.queue_actions, "the count and the host's word that it takes the actions: {counts:?}");
    assert_eq!(items(), [row("queued-1", "Run the tests", Some("worktree")), row("queued-2", "Then lint", Some("waitsForTurn"))]);
    assert_eq!(counts.queued_texts(), Some(vec!["Run the tests".to_string(), "Then lint".to_string()]), "the strip's tooltip");

    // A run takes the head of the queue there: it leaves here; another request joins: it shows.
    queued.lock().unwrap().get_mut("host-1").unwrap().remove(0);
    until(|| ids() == ["queued-2"], 10_000, "a row the host started leaves").await;
    queued.lock().unwrap().get_mut("host-1").unwrap().push(row("queued-3", "And the docs", Some("waitsForTurn")));
    until(|| ids() == ["queued-2", "queued-3"], 10_000, "a row the host queued shows").await;
    assert_eq!(desk.counts(&pane_id).unwrap().queued, 2);

    // × from here: the host's phone handler takes the row out, and it leaves here at once.
    assert_eq!(desk.queue_action(&pane_id, "queued-3", QueueAction::Remove).await, Ok("removed".into()));
    assert!(ids() == ["queued-2"], "the removed row leaves at once: {:?}", ids());
    let acted = || world.desk.actions().iter().map(|a| format!("{} {} {}", a.action, a.session_id, a.body["id"])).collect::<Vec<_>>();
    assert!(acted().contains(&"queue-remove host-1 \"queued-3\"".to_string()), "the removal reached the host: {:?}", acted());
    // 바로 실행 from here: the host's word for what it did.
    assert_eq!(desk.queue_action(&pane_id, "queued-2", QueueAction::RunNow).await, Ok("here".into()));
    assert!(acted().contains(&"queue-run-now host-1 \"queued-2\"".to_string()), "바로 실행 reached the host: {:?}", acted());
    until(|| desk.counts(&pane_id).is_some_and(|c| c.queued == 0 && c.queued_items == Some(vec![])), 10_000, "the queue is empty").await;
    // A row the host no longer holds is the host's refusal; an id that is no identifier never leaves here.
    let gone = desk.queue_action(&pane_id, "queued-2", QueueAction::Remove).await;
    assert!(gone.is_err(), "a row gone on the host is refused: {gone:?}");
    let before = world.desk.actions().len();
    assert_eq!(
        desk.queue_action(&pane_id, "../x", QueueAction::RunNow).await,
        Err(get("remote.error.queueItemNotFound")),
        "a bad item id is refused here"
    );
    assert_eq!(world.desk.actions().len(), before);

    desk.dispose();
    sharing_host.dispose().await;
    let _ = std::fs::remove_dir_all(directory);
}

/// W9-J review L2: a mirrored pane whose mirror is not attached is refused (`remote.mirror.error.unavailable`): its start, stop and
/// answers never fall through to a run of this PC or the client's runs.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_mirrored_pane_never_runs_here_without_its_mirror() {
    let mut far = support::desk::workspace("far", std::path::Path::new("/Users/example/far"));
    far.remote = Some(RemoteWorkspaceReference {
        connection_id: Some("conn".into()),
        workspace_id: Some("ws-far".into()),
        host_name: Some("studio-mac".into()),
        version: 2,
        mirror: true,
    });
    let pane = RunSession {
        mirror: Some(MirrorReference {
            connection_id: "conn".into(),
            host_session_id: "pane-far".into(),
            host_workspace_id: "ws-far".into(),
            host_name: "studio-mac".into(),
        }),
        ..support::desk::pane("mirrored", "far")
    };
    let state = AppSnapshot { workspaces: Some(vec![far]), sessions: Some(vec![Arc::new(pane)]), ..AppSnapshot::default() };
    let world = World::over(Temp::new("unattached"), mighty_core::normalize::normalize(&state, false).unwrap(), "c", "c", |_| {});
    let service = world.desk.service.clone();
    assert!(service.snapshot().session("mirrored").is_some_and(|s| s.mirror.is_some()));
    let unavailable = get("remote.mirror.error.unavailable");
    let started = service.start(mighty_run::model::StartRunRequest::new("mirrored", "far", "claude", "hi")).await;
    assert_eq!(started.map_err(|e| e.message().to_string()), Err(unavailable.clone()), "a start is refused");
    assert!(!service.is_session_running("mirrored"), "nothing runs here");
    assert_eq!(service.stop("mirrored").await.map_err(|e| e.message().to_string()), Err(unavailable.clone()), "a stop is refused");
    let answered = service.respond_to_tool_permission("mirrored", "perm-1", true).await;
    assert_eq!(answered.map_err(|e| e.message().to_string()), Err(unavailable.clone()), "an answer is refused");
    let planned = service.answer_plan("mirrored", "perm-2", &PlanDecision::cancel()).await;
    assert_eq!(planned.map_err(|e| e.message().to_string()), Err(unavailable), "a plan answer is refused");
    assert!(!service.try_steer("mirrored", "note"), "no steer");
}

/// W9-J review L3: a list read by a poll that is no longer the connection's (the connection removed, or disconnected, while the list was
/// read) is never laid over the state: a removal's taking away stays.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_list_read_after_its_connection_left_changes_nothing() {
    let (store, directory) = client_store();
    let world = World::over(Temp::new("stale-poll"), AppSnapshot::default(), "c", "c", |_| {});
    let service = world.desk.service.clone();
    let seen = Arc::new(Collected::default());
    let client = client_with(&store, &directory, Arc::new(MemoryVault::default()), &seen, loopback_options());
    let stage = Arc::new(Stage { service: service.clone(), permissions: Mutex::new(vec![]), failures: Mutex::new(vec![]) });
    let desk = MirrorDesk::new(client, stage, spawn());
    let state = mighty_remote::remote::mirror::MirrorState {
        host_name: "studio-mac".into(),
        revision: "r".into(),
        unchanged: false,
        workspaces: vec![RemoteSharedWorkspace { id: "ws-far".into(), name: "far".into(), path: "/Users/example/far".into() }],
        sessions: vec![],
    };
    for serial in [0, 1, u64::MAX] {
        desk.apply_state("conn", serial, &state);
    }
    assert!(service.snapshot().workspaces().is_empty(), "no poll of the connection: nothing is laid over");
    let _ = std::fs::remove_dir_all(directory);
}

/// "remote mirror: an older host stops the mirror only": a host that answers the mirror's list 404 (an older Mighty Code) leaves the
/// connection up, stops the mirror and says so; the button falls back to 열기.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_older_host_stops_the_mirror_only() {
    let fixture = make_host(MakeHost::default()).await;
    let sharing = fixture.host.start(&[fixture.shared.id().to_string()], 0).await.unwrap();
    let (store, directory) = client_store();
    let client_world = World::over(Temp::new("older-host"), AppSnapshot::default(), "c", "c", |_| {});
    let service = client_world.desk.service.clone();
    let seen = Arc::new(Collected::default());
    let client = client_with(&store, &directory, Arc::new(MemoryVault::default()), &seen, loopback_options());
    let stage = Arc::new(Stage { service: service.clone(), permissions: Mutex::new(vec![]), failures: Mutex::new(vec![]) });
    let desk = MirrorDesk::new(client.clone(), stage, spawn());
    desk.attach();
    service.set_mirror(Arc::new(desk.clone()));
    let info = client.connect("", &format!("127.0.0.1:{}", sharing.port.unwrap()), sharing.key.as_deref().unwrap()).await.unwrap();
    until(|| desk.link(&info.id).is_some_and(|l| l.phase == "off"), 10_000, "the mirror stops").await;
    let link = desk.link(&info.id).unwrap();
    assert!(
        link.unsupported
            && link.error == Some(get_with("remote.mirror.error.olderHost", &[("host", &info.name)]))
            && client.connection(&info.id).unwrap().status == RemoteConnectionStatus::Connected,
        "the mirror stops with the older-host words and the connection stays: {link:?}"
    );
    assert!(!service.snapshot().workspaces().iter().any(|w| w.remote.as_ref().is_some_and(|r| r.mirror)), "no mirror workspace");
    let opened = client.open_workspace(&info.id, fixture.shared.id()).unwrap();
    assert!(opened.remote.as_ref().is_some_and(|r| !r.mirror), "열기 still opens the host's workspace for this PC's own remote panes");
    desk.dispose();
    fixture.dispose().await;
    let _ = std::fs::remove_dir_all(directory);
}
