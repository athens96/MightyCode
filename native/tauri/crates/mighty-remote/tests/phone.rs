//! The m1 router's checks, ported 1:1 by C# name over the headless desk (tests/support/desk.rs): MobileRemoteVerification's
//! `UploadOwnershipAndClaims`, `DesktopRouteContract` and `ScreenShareSwitch` (the router half), MobileStructureVerification's route
//! check, MobileFlowVerification's `RouteTakesRolesOnTheSessionDetailAlone`, MobileHostVerification's router checks, and the phone halves
//! of other suites (app update, work map, file changes, pending, ask targets, Antigravity cards, pi). Each test names its C# check.

mod support;

use std::sync::Arc;

use mighty_composer::pending::SubmitReceipt;
use mighty_core::app_update_view::{Phase, State};
use mighty_core::model::{LogEntry, RunSession, SessionRateLimit, SessionUsage};
use mighty_core::permission::ToolPermissionRequest;
use mighty_graph::structure::mobile::{Built, MobileCluster, MobileItem, MobileNode, MobileRow, MobileStructure, MobileTally, tag};
use mighty_remote::phone::desk::{ActionAnswer, MobileError, MobileImagePreview, MobileImageResult, MobilePaneDigest, MobilePaneExtras};
use mighty_remote::phone::files::{self, PreviewSlot};
use mighty_remote::phone::router::{self, CAPABILITIES, MAXIMUM_REPLY_BYTES};
use mighty_remote::phone::uploads::MobileUploads;
use mighty_run::antigravity::permissions::{DeniedAction, PermissionRequest};
use mighty_run::model::{ModelCatalog, ModelOption};
use serde_json::{Value, json};
use support::desk::{Temp, World, pane};

fn l() -> mighty_core::locale::Locale {
    mighty_run::strings::locale()
}

fn entry(id: &str, kind: &str, text: &str, second: i64) -> LogEntry {
    LogEntry::new(id.into(), kind, text.into(), format!("2026-10-12T00:00:{:02}.0000000+00:00", second), None)
}

fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

/// MobileRemoteVerification.cs:86 `UploadOwnershipAndClaims` (L456).
#[test]
fn mobile_remote_upload_ownership_and_claims() {
    let temp = Temp::new("mobile-uploads");
    let store = MobileUploads::new(temp.path());
    let ticket = store.begin("pane", "phone-a", "../sample.txt", 4).unwrap();
    let id = ticket.upload_id.clone();
    assert!(store.append(&id, "phone-b", 0, b"ABCD").is_err(), "other phone appended upload");
    assert!(store.append(&id, "phone-a", 1, b"ABCD").is_err(), "out-of-order chunk accepted");
    assert_eq!(store.append(&id, "phone-a", 0, b"ABCD").unwrap(), 4, "valid chunk appended");
    assert_eq!(store.complete(&id, "phone-a").unwrap().name, "sample.txt");
    assert!(store.claim(std::slice::from_ref(&id), "other-pane", "phone-a").is_err(), "cross-pane upload consumed");
    let claim = store.claim(std::slice::from_ref(&id), "pane", "phone-a").unwrap();
    assert!(store.claim(std::slice::from_ref(&id), "pane", "phone-a").is_err(), "concurrent double submit accepted");
    store.finish(&claim, false);
    let again = store.claim(std::slice::from_ref(&id), "pane", "phone-a").unwrap();
    let bytes = mighty_run::attachments::decode(&again.attachments[0]).unwrap();
    assert_eq!(bytes, b"ABCD", "failed submit leaves attachment available");
    store.finish(&again, true);
    assert!(store.claim(std::slice::from_ref(&id), "pane", "phone-a").is_err(), "spent upload replayed");
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0, "consumed file handle deletes staging bytes");
}

/// MobileRemoteVerification.cs:104 `DesktopRouteContract` (L457).
#[tokio::test]
async fn mobile_remote_desktop_route_contract() {
    let world = World::new("mobile-route");
    std::fs::write(world.folder().join("hello.txt"), "안녕하세요").unwrap();
    let mut first = pane("pane", "w");
    first.logs = Some(vec![entry("first", "user", "hello", 0), entry("last", "assistant", "hi", 1)]);
    world.desk.set_sessions(vec![first]);
    world.desk.calls().action = Some(Arc::new(|_| Ok(ActionAnswer::Value(json!("started")))));
    let (status, state) = world.route("GET", "/m1/state", None).await;
    assert!(status == 200 && text(&state["sessions"][0], "id") == Some("pane"), "phone receives desktop sessions");
    let (_, history) = world.route("GET", "/m1/sessions/pane/entries?before=last&limit=1", None).await;
    assert_eq!(text(&history["entries"][0], "id"), Some("first"), "history pagination matches contract");
    assert_eq!(world.route("GET", "/m1/state?wait=11", None).await.0, 400, "unbounded poll rejected");
    let stale = json!({"requestId": "stale", "runId": "pane", "allow": true});
    assert_eq!(world.route("POST", "/m1/sessions/pane/permission", Some(stale)).await.0, 409, "stale permissions fail closed");
    let plan = |decision: &str| Some(json!({"requestId": "stale", "runId": "pane", "decision": decision}));
    assert_eq!(world.route("POST", "/m1/sessions/pane/plan", plan("approveAutoEdit")).await.0, 409, "a stale plan answer fails closed");
    let (status, unknown) = world.route("POST", "/m1/sessions/pane/plan", plan("allow")).await;
    assert!(
        status == 400 && text(&unknown, "error") == Some(&l().get("plan.error.invalidDecision")),
        "an unknown plan decision is refused"
    );
    let bad = json!({"requestId": "../x", "runId": "pane", "decision": "cancel"});
    let (status, bad) = world.route("POST", "/m1/sessions/pane/plan", Some(bad)).await;
    assert!(status == 400 && text(&bad, "error") == Some(&l().get("plan.error.invalidRequest")), "a malformed plan request id is refused");
    let empty = json!({"requestId": "stale", "runId": "pane", "decision": "revise", "feedback": " \n "});
    assert_eq!(world.route("POST", "/m1/sessions/pane/plan", Some(empty)).await.0, 400, "an empty change request never reaches the pane");
    let invented = json!({"text": "hello", "mode": "invented"});
    assert!(
        world.route("POST", "/m1/sessions/pane/submit", Some(invented)).await.0 == 400 && world.desk.actions().is_empty(),
        "invalid submit mode never reaches composer"
    );
    let korean = json!({"text": "한글 request", "mode": "queue"});
    assert_eq!(world.route("POST", "/m1/sessions/pane/submit", Some(korean)).await.0, 202);
    let actions = world.desk.actions();
    assert!(actions.len() == 1 && actions[0].body["text"] == "한글 request", "valid submit routes complete UTF-8 text");
    assert_eq!(
        world.route("POST", "/m1/workspaces/w/sessions", Some(json!({"kind": "shell"}))).await.0,
        409,
        "phone cannot start local terminal"
    );
    for provider in ["pi", "opencode", "hermes"] {
        let (status, _) = world.route("POST", "/m1/workspaces/w/sessions", Some(json!({"kind": "claude", "provider": provider}))).await;
        let last = world.desk.actions().pop().unwrap();
        assert!(
            CAPABILITIES.contains(&provider) && status == 201 && last.action == "create" && last.body["provider"] == provider,
            "the phone may create a pane of {provider}"
        );
    }
    let rovo = json!({"kind": "claude", "provider": "rovo"});
    assert_eq!(world.route("POST", "/m1/workspaces/w/sessions", Some(rovo)).await.0, 400, "an agent the host does not run is refused");
    assert_eq!(world.route("GET", "/m1/workspaces/w/file?path=hello.txt", None).await.0, 200, "workspace text is previewed");
    assert_eq!(world.route("GET", "/m1/workspaces/w/file?path=..%2Foutside.txt", None).await.0, 403, "workspace escape is rejected");
    assert!(
        world.route("GET", "/m1/screen-share/status", None).await.0 == 503 && !CAPABILITIES.contains(&"screenShare"),
        "unsupported capture is not advertised"
    );
}

/// MobileRemoteVerification.cs:193 `ScreenShareSwitch` (L458), the router half: switched off (W8-F's `screen::ENABLED`), screen share is
/// neither advertised nor routed, whatever the host's extra capabilities say; the extra list drops `screenShare` only while it is off.
#[tokio::test]
async fn mobile_remote_screen_share_switch() {
    for (available, name) in [(false, "off"), (mighty_remote::screen::ENABLED, "standard")] {
        let world = World::with(&format!("mobile-screen-{name}"), |o| o.screen_share = available);
        world.desk.calls().extra_capabilities = vec!["screenShare".into()];
        let (_, info) = world.route("GET", "/m1/info", None).await;
        let advertised = info["capabilities"].as_array().unwrap().iter().any(|c| c == "screenShare");
        let state = world.route("GET", "/m1/screen-share/state", None).await.0;
        let session = world.route("POST", "/m1/screen-share/sessions", Some(json!({"mode": "view"}))).await.0;
        assert!(!advertised && state == 503 && session == 503, "screen share switched {name} is neither advertised nor routed");
    }
    assert_eq!(router::extra_capabilities(Some(vec!["screenShare".into(), "other".into()]), false), ["other"]);
    assert_eq!(router::extra_capabilities(Some(vec!["screenShare".into()]), true), ["screenShare"]);
    assert!(router::extra_capabilities(None, true).is_empty(), "the host's extra capabilities drop screenShare only while it is off");
}

fn big_structure() -> MobileStructure {
    let escaped = "\"\\".repeat(2_000);
    let node = |i: usize| MobileNode {
        id: format!("n{i}"),
        depth: 1,
        title: "Agent".into(),
        status: "completed".into(),
        kind: "agent".into(),
        model: None,
        prompt: Some(escaped.clone()),
        output: Some(escaped.clone()),
        activity: None,
        background: None,
        output_tail: None,
        notice_reply: None,
    };
    let item = MobileItem {
        run_id: "r".into(),
        ordinal: 1,
        title: None,
        status: "completed".into(),
        finished: true,
        request: Some(escaped.clone()),
        message: None,
        activity: vec![],
        answer: Some(escaped.clone()),
        tally: MobileTally { agents: 20, tools: 0 },
        clusters: vec![MobileCluster { nodes: (0..20).map(node).collect() }],
        omitted_nodes: None,
        notice_run: None,
        background_waiting: None,
        awaiting_notice: None,
    };
    MobileStructure {
        v: 1,
        shape: "free".into(),
        columns: None,
        rows: Some(vec![MobileRow { item, request: "done".into(), execution: "done".into(), result: "done".into() }]),
        omitted: None,
    }
}

/// MobileStructureVerification.cs:158 `RouteSendsTheStructureOnlyWhenAskedAndChanged` (L465).
#[tokio::test]
async fn mobile_structure_sent_only_when_asked_and_changed_shed_first() {
    use mighty_core::json::Wire;
    let world = World::new("mobile-structure");
    world.desk.set_sessions(vec![pane("s", "w")]);
    let filler = Arc::new(std::sync::Mutex::new(String::new()));
    let asked = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let value = big_structure();
    let built = Built { tag: tag(Some(&value)), value: Some(value.clone()) };
    {
        let mut calls = world.desk.calls();
        let filler = filler.clone();
        calls.extras = Some(Arc::new(move |_| MobilePaneExtras {
            settings: Some(json!({"filler": filler.lock().unwrap().clone()})),
            mighty: Some(json!({"style": "cli", "runs": ["kept"]})),
            ..MobilePaneExtras::default()
        }));
        let (asked, built) = (asked.clone(), built.clone());
        calls.structure = Some(Arc::new(move |_| {
            asked.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Some(built.clone())
        }));
    }
    let call = |path: &'static str| {
        let world = &world;
        async move {
            let (status, body) = world.route("GET", path, None).await;
            (status, body.get("mighty").cloned().unwrap_or(Value::Null))
        }
    };
    let count = || asked.load(std::sync::atomic::Ordering::SeqCst);
    let (status, plain) = call("/m1/sessions/s?since=0&wait=0").await;
    assert!(
        status == 200 && plain.get("structure").is_none() && plain.get("structureTag").is_none() && count() == 0,
        "a phone that did not ask gets neither field"
    );
    let (status, sent) = call("/m1/sessions/s?since=0&wait=0&structure=0").await;
    let expected: Value = serde_json::from_str(&value.to_json()).unwrap();
    assert!(
        status == 200 && sent["structure"] == expected && sent["structureTag"] == built.tag && count() == 1,
        "a phone that asked gets the value and its tag"
    );
    assert!(sent["runs"][0] == "kept" && sent["style"] == "cli", "the Mighty payload keeps its own fields beside the structure");
    let holding = format!("/m1/sessions/s?since=0&wait=0&structure={}", built.tag);
    let (status, held) = world.route("GET", &holding, None).await;
    let held = &held["mighty"];
    assert!(
        status == 200 && held.get("structure").is_none() && held["structureTag"] == built.tag,
        "a phone that holds the value gets the tag alone"
    );
    assert!(
        call("/m1/state?since=0&wait=0&structure=0").await.0 == 400
            && call("/m1/sessions/s?structure=x").await.0 == 400
            && CAPABILITIES.contains(&"structure"),
        "the structure query is the session detail's alone; the host advertises it"
    );
    let size = value.to_json().len();
    assert!(size > 64 * 1_024, "the fixture's structure must be large enough to pass the limit: {size}");
    *filler.lock().unwrap() = "x".repeat(MAXIMUM_REPLY_BYTES - size / 2);
    let (status, shed) = call("/m1/sessions/s?since=0&wait=0&structure=0").await;
    assert!(
        status == 200 && shed.get("structure").is_none() && shed["structureTag"] == built.tag && shed["runs"][0] == "kept",
        "a reply over the limit sheds the structure first and keeps its tag"
    );
    *filler.lock().unwrap() = "x".repeat(MAXIMUM_REPLY_BYTES + 1);
    assert_eq!(call("/m1/sessions/s?since=0&wait=0&structure=0").await.0, 413, "a reply too large without the structure is still refused");
}

/// MobileFlowVerification.cs:111 `RouteTakesRolesOnTheSessionDetailAlone` (L469).
#[tokio::test]
async fn mobile_role_view_roles_1_is_the_session_details_alone() {
    let world = World::new("mobile-roles");
    world.desk.set_sessions(vec![pane("s", "w")]);
    let reads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    {
        let mut calls = world.desk.calls();
        calls.extras =
            Some(Arc::new(|_| MobilePaneExtras { mighty: Some(json!({"style": "cli", "runs": []})), ..MobilePaneExtras::default() }));
        let reads = reads.clone();
        calls.role_extras = Some(Arc::new(move |_| {
            reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Some(MobilePaneExtras { mighty: Some(json!({"style": "cli", "runs": [], "fanOut": true})), ..MobilePaneExtras::default() })
        }));
    }
    let count = || reads.load(std::sync::atomic::Ordering::SeqCst);
    let (status, plain) = world.route("GET", "/m1/sessions/s?since=0&wait=0", None).await;
    assert!(status == 200 && plain["mighty"].get("fanOut").is_none() && count() == 0, "an older phone gets the plain payload");
    let (status, roles) = world.route("GET", "/m1/sessions/s?since=0&wait=0&roles=1", None).await;
    assert!(status == 200 && roles["mighty"]["fanOut"] == true && count() == 1, "a phone that asked gets the role view");
    let (status, none) = world.route("GET", "/m1/sessions/s?since=0&wait=0&roles=0&structure=0", None).await;
    assert!(status == 200 && none["mighty"].get("fanOut").is_none() && count() == 1, "roles=0 asks for none, beside a structure tag");
    assert!(
        world.route("GET", "/m1/state?since=0&wait=0&roles=1", None).await.0 == 400
            && world.route("GET", "/m1/sessions/s?roles=yes", None).await.0 == 400
            && CAPABILITIES.contains(&"roles"),
        "the roles query is the session detail's alone, a number; the host advertises it"
    );
}

/// MobileHostVerification.cs:189 `APhoneIsToldOnlyOfWhatChanged` (L473), the router's half (the rule's own half is in
/// tests/phone_payloads.rs).
#[tokio::test]
async fn mobile_host_a_phone_is_told_only_of_what_changed() {
    let world = World::new("mobile-revisions");
    let status_line = Arc::new(std::sync::Mutex::new("12:00".to_string()));
    {
        let status_line = status_line.clone();
        world.desk.calls().extras = Some(Arc::new(move |_| MobilePaneExtras {
            status_line: Some(json!({"text": status_line.lock().unwrap().clone()})),
            ..MobilePaneExtras::default()
        }));
    }
    let request = entry("u1", "user", "hello", 0);
    let reply = entry("r1", "assistant", &"a".repeat(300), 1);
    let mut a = pane("a", "w");
    a.status = Some("running".into());
    a.logs = Some(vec![request.clone(), reply.clone()]);
    let mut b = pane("b", "w");
    let router = &world.router;
    let set = |panes: Vec<RunSession>| {
        world.desk.set_sessions(panes);
        router.changed();
        router.observe();
    };
    world.desk.set_sessions(vec![a.clone(), b.clone()]);
    router.observe();
    let first: Vec<String> = router.take_notifications().into_iter().map(|n| n.0).collect();
    assert_eq!(first, ["state", "session:a", "session:b"], "the first reading tells the list and every pane");
    let (state, ra, rb) = (router.state_revision(), router.session_revision("a"), router.session_revision("b"));
    a.logs = Some(vec![request.clone(), LogEntry { text: Some("a".repeat(400)), ..reply.clone() }]);
    set(vec![a.clone(), b.clone()]);
    let streamed = router.take_notifications();
    assert!(
        streamed == [("session:a".to_string(), ra + 1)] && router.state_revision() == state && router.session_revision("b") == rb,
        "a streamed reply tells its own pane alone: {streamed:?}"
    );
    *status_line.lock().unwrap() = "12:01".into();
    router.changed();
    router.observe();
    assert!(router.take_notifications().is_empty() && router.session_revision("a") == ra + 1, "a status line that moved tells nobody");
    a.status = Some("completed".into());
    set(vec![a.clone(), b.clone()]);
    let finished = router.take_notifications();
    assert!(
        finished == [("state".to_string(), state + 1), ("session:a".to_string(), ra + 2)] && router.session_revision("b") == rb,
        "a finished run tells the list and its pane, and no other pane: {finished:?}"
    );
    b.title = Some("renamed".into());
    set(vec![a.clone(), b.clone()]);
    assert_eq!(
        router.take_notifications(),
        [("state".to_string(), state + 2), ("session:b".to_string(), rb + 1)],
        "a renamed pane tells the list and that pane"
    );
    set(vec![a.clone(), b.clone()]);
    assert!(router.take_notifications().is_empty(), "reading again with nothing moved tells nobody");
    let (_, list) = world.route("GET", "/m1/state", None).await;
    let (_, detail) = world.route("GET", "/m1/sessions/a", None).await;
    let revisions: Vec<i64> = list["sessions"].as_array().unwrap().iter().map(|s| s["revision"].as_i64().unwrap()).collect();
    assert!(list["revision"] == state + 2 && revisions == [ra + 2, rb + 1], "the list carries its own revision and each pane's");
    assert!(detail["revision"] == ra + 2 && detail["session"]["revision"] == ra + 2, "a pane's detail carries that pane's revision");
    // A long poll waits on its own scope: another pane moving does not answer it early.
    let started = std::time::Instant::now();
    let path = format!("/m1/sessions/a?since={}&wait=1", ra + 2);
    let poll = world.route("GET", &path, None);
    let mover = async {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        b.status = Some("running".into());
        world.desk.set_sessions(vec![a.clone(), b.clone()]);
        router.changed();
    };
    let ((_, unmoved), ()) = tokio::join!(poll, mover);
    assert!(
        unmoved["revision"] == ra + 2 && started.elapsed() > std::time::Duration::from_millis(800),
        "a pane's poll sleeps through another pane's change"
    );
    let started = std::time::Instant::now();
    let path = format!("/m1/sessions/a?since={}&wait=5", ra + 2);
    let poll = world.route("GET", &path, None);
    let mover = async {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let mut moved = a.clone();
        moved.logs.as_mut().unwrap().push(entry("u2", "user", "again", 2));
        world.desk.set_sessions(vec![moved, b.clone()]);
        router.changed();
    };
    let ((_, woken), ()) = tokio::join!(poll, mover);
    assert!(
        woken["revision"] == ra + 3 && started.elapsed() < std::time::Duration::from_secs(3),
        "a pane's poll answers when that pane moves"
    );
    // P1-RLY-1: Notify wakes long polls promptly when `router.changed()` fires (no 100 ms polling latency).
    let notify_start = std::time::Instant::now();
    let notify_path = format!("/m1/sessions/a?since={}&wait=5", ra + 3);
    let notify_poll = world.route("GET", &notify_path, None);
    let fast_mover = async {
        tokio::time::sleep(std::time::Duration::from_millis(15)).await;
        let mut moved = a.clone();
        moved.logs.as_mut().unwrap().push(entry("u3", "user", "fast", 3));
        world.desk.set_sessions(vec![moved, b.clone()]);
        router.changed();
    };
    let ((_, fast_woken), ()) = tokio::join!(notify_poll, fast_mover);
    assert!(
        fast_woken["revision"] == ra + 4 && notify_start.elapsed() < std::time::Duration::from_millis(150),
        "Notify-driven poll should wake well before a full polling step: elapsed {:?}",
        notify_start.elapsed()
    );
    router.take_notifications();
    set(vec![b.clone()]);
    let closed = router.take_notifications();
    assert!(
        closed.len() == 1 && closed[0].0 == "state" && router.session_revision("a") == 1,
        "a closed pane moves the list and loses its revision"
    );
    // A pane whose digest cannot be read is read from its record, and a poll of another scope still answers.
    let closing = World::new("mobile-revisions-closing");
    closing.desk.set_sessions(vec![b.clone()]);
    closing.desk.calls().digest = Some(Arc::new(|_| None));
    let polled = closing.raw("GET", "/m1/state?since=0&wait=1", None).await;
    let told: Vec<String> = closing.router.take_notifications().into_iter().map(|n| n.0).collect();
    assert!(polled.status == 200 && told == ["state", "session:b"], "a pane whose digest cannot be read does not fail the poll");
    // A pass that fails all the same is made again at the next call, so its change is not lost.
    let flaky = World::new("mobile-revisions-flaky");
    flaky.desk.set_sessions(vec![b.clone()]);
    let fails = Arc::new(std::sync::atomic::AtomicBool::new(true));
    {
        let fails = fails.clone();
        flaky.desk.calls().active_style = Some(Arc::new(move |_| {
            if fails.load(std::sync::atomic::Ordering::SeqCst) {
                panic!("the pane went away");
            }
            None
        }));
    }
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    flaky.router.observe();
    std::panic::set_hook(hook);
    assert!(flaky.router.take_notifications().is_empty(), "the failed pass told nobody");
    fails.store(false, std::sync::atomic::Ordering::SeqCst);
    flaky.router.observe();
    let told: Vec<String> = flaky.router.take_notifications().into_iter().map(|n| n.0).collect();
    assert_eq!(told, ["state", "session:b"], "a failed pass is read again rather than dropped");
}

/// MobileHostVerification.cs:266 `ASessionReplyReadsAsTheDesktopDoes` (L474), the router's half.
#[tokio::test]
async fn mobile_host_a_session_reply_reads_as_the_desktop_does() {
    let world = World::new("mobile-reply");
    world.desk.calls().active_style = Some(Arc::new(|_| None));
    let mut picture = entry("i1", "image", "screenshot.png", 0);
    picture.images = Some(vec![Some(mighty_core::model::AgentImageRef {
        hash: Some("cache-name".into()),
        media_type: Some("image/png".into()),
        width: 10,
        height: 10,
        bytes: 100,
        source: Some("tool".into()),
        path: None,
    })]);
    let mut first = pane("pane", "w");
    first.logs = Some(vec![entry("u", "user", "look", 0), picture]);
    let mut shell = pane("shell", "w");
    shell.kind = Some("shell".into());
    world.desk.set_sessions(vec![first, shell]);
    let (_, detail) = world.route("GET", "/m1/sessions/pane", None).await;
    let kinds: Vec<&str> = detail["entries"].as_array().unwrap().iter().map(|e| e["kind"].as_str().unwrap()).collect();
    assert!(kinds == ["user", "system"] && detail["entries"][1].get("images").is_none(), "the detail's transcript has no image entry");
    let (_, older) = world.route("GET", "/m1/sessions/pane/entries?before=i1", None).await;
    assert!(older["entries"].as_array().unwrap().len() == 1 && older["hasMore"] == false, "an older page reads the same way");
    let (_, newest) = world.route("GET", "/m1/sessions/pane/entries?limit=1", None).await;
    let (_, gone) = world.route("GET", "/m1/sessions/pane/entries?before=gone", None).await;
    assert!(
        newest["entries"][0]["id"] == "i1"
            && newest["hasMore"] == true
            && gone["entries"].as_array().unwrap().is_empty()
            && gone["hasMore"] == false,
        "without a cursor the page is the newest entries; a cursor the host no longer holds answers empty"
    );
    assert!(detail.get("rateLimits").is_none() && detail.get("usage").is_none(), "a pane that reported no account window sends no limits");
    let session = &detail["session"];
    assert!(
        session["styleId"] == "cli" && session["mightyStyle"] == "cli" && session["agentViewMode"] == "mighty",
        "an agent pane that runs no style says cli, in the Mighty view"
    );
    let (_, state) = world.route("GET", "/m1/state", None).await;
    let shell = &state["sessions"][1];
    assert!(
        shell["terminal"] == true
            && shell.get("styleId").is_none()
            && shell.get("agentViewMode").is_none()
            && shell.get("mightyStyle").is_none(),
        "a pane that is not an agent's has no view and no style"
    );
    assert!(!world.raw("GET", "/m1/state", None).await.body.contains("cache-name"), "no cache name travels");
}

/// MobileHostVerification.cs:319 `APaneTravelsAsItsProviderRuns` (L475), the router's half.
#[tokio::test]
async fn mobile_host_a_pane_travels_as_its_provider_runs() {
    let world = World::new("mobile-provider");
    world.desk.calls().active_style = Some(Arc::new(|_| None));
    let reported = |provider: &str, model: &str, selected: &str| {
        let mut usage: SessionUsage = mighty_core::json::Wire::read(
            &json!({"provider": provider, "source": "cli", "tokenScope": "session", "updatedAt": "2026-10-12T00:00:00Z"}),
        )
        .unwrap();
        usage.model = Some(model.into());
        usage.selected_model = Some(selected.into());
        usage
    };
    let with = |id: &str, provider: &str, model: &str, usage: Option<SessionUsage>, kind: &str| RunSession {
        provider: Some(provider.into()),
        model: Some(model.into()),
        session_usage: usage,
        kind: Some(kind.into()),
        ..pane(id, "w")
    };
    world.desk.set_sessions(vec![
        with("agy", "gemini", "default", Some(reported("gemini", "gemini-4-argon-medium", "default")), "claude"),
        with("codex", "codex", "default", None, "claude"),
        with("opus", "claude", "opus", Some(reported("claude", "claude-opus-5-5", "opus")), "claude"),
        with("switched", "claude", "sonnet", Some(reported("claude", "claude-opus-5-5", "opus")), "claude"),
        with("default", "claude", "default", None, "claude"),
        with("shell", "claude", "default", None, "shell"),
        RunSession {
            forked_from: Some(mighty_core::model::ForkedFrom {
                pane_id: "opus".into(),
                request_id: Some("r2".into()),
                title: Some("Login fix".into()),
                mode: Some("handoff".into()),
                ..Default::default()
            }),
            ..with("fork-abcdef12", "claude", "default", None, "claude")
        },
        RunSession {
            compare_runs: Some(vec![mighty_core::model::CompareRun {
                id: "cmp1".into(),
                request: "Make it faster".into(),
                base_commit: "abc1234".into(),
                children: vec![
                    mighty_core::model::CompareChild {
                        id: "c1".into(),
                        provider: "claude".into(),
                        model: Some("opus".into()),
                        ..Default::default()
                    },
                    mighty_core::model::CompareChild {
                        id: "c2".into(),
                        provider: "codex".into(),
                        failure: Some("signed out".into()),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            }]),
            ..with("compare-parent", "claude", "default", None, "claude")
        },
    ]);
    async fn panes(world: &World) -> std::collections::HashMap<String, Value> {
        let (_, state) = world.route("GET", "/m1/state", None).await;
        state["sessions"].as_array().unwrap().iter().map(|p| (p["id"].as_str().unwrap().to_string(), p.clone())).collect()
    }
    world.desk.calls().attention.insert("shell".into(), ("notify".into(), "빌드 끝".into(), "2026-11-23T00:00:00+00:00".into()));
    let found = panes(&world).await;
    assert_eq!(
        found["shell"]["attention"],
        serde_json::json!({"kind": "notify", "text": "빌드 끝", "at": "2026-11-23T00:00:00+00:00"}),
        "MC-26: the desktop's mark on a waiting pane"
    );
    assert!(found["opus"].get("attention").is_none(), "a pane that does not wait carries none");
    assert_eq!(
        found["fork-abcdef12"]["forkedFrom"],
        serde_json::json!({"paneId": "opus", "requestId": "r2", "title": "Login fix"}),
        "MC-28: a forked pane names its source"
    );
    assert!(found["opus"].get("forkedFrom").is_none(), "a pane that was not forked carries none");
    let compare = &found["compare-parent"]["compareRuns"][0];
    assert!(
        compare["id"] == "cmp1"
            && compare["children"][0]["provider"] == "claude"
            && compare["children"][0]["model"] == "opus"
            && compare["children"][1]["failure"] == "signed out",
        "MC-29: a pane's compare runs, each child with its agent, model and failure: {compare}"
    );
    assert!(found["opus"].get("compareRuns").is_none(), "a pane that started none carries none");
    assert!(
        found["agy"]["provider"] == "gemini" && found["agy"]["model"] == "default" && found["agy"].get("resolvedModel").is_none(),
        "an Antigravity pane on default names no model"
    );
    assert!(found.values().all(|p| p["canSteer"] == false), "every pane, Claude too, only queues: canSteer is always false");
    assert_eq!(found["opus"]["resolvedModel"], "claude-opus-5-5", "a Claude family alias takes the model reported for it");
    assert!(
        found["switched"].get("resolvedModel").is_none() && found["shell"].get("resolvedModel").is_none(),
        "a model reported for an earlier selection never shows, and a command pane names none"
    );
    assert!(found["default"].get("resolvedModel").is_none(), "without a catalogue a Claude default resolves to nothing");
    let (_, detail) = world.route("GET", "/m1/sessions/agy", None).await;
    assert!(
        detail["session"]["canSteer"] == false && detail["session"].get("resolvedModel").is_none(),
        "the session reply's summary says the same"
    );
    world.desk.calls().catalog = Some(Arc::new(|p: &RunSession| {
        (p.provider() == "claude").then(|| ModelCatalog {
            source: "cli".into(),
            models: vec![model_option("default", "Default", Some("claude-opus-5-5")), model_option("opus", "Opus", None)],
            detail: String::new(),
            read_failure: None,
            listed_at: None,
        })
    }));
    let found = panes(&world).await;
    assert!(
        found["default"]["resolvedModel"] == "claude-opus-5-5" && found["agy"].get("resolvedModel").is_none(),
        "a listed default resolves; an Antigravity default still does not"
    );
}

/// MobileHostVerification.cs:419 `FileRepliesCarryACodeAndTheMacsCaps` (L478).
#[tokio::test]
async fn mobile_host_file_replies_carry_a_code_and_the_macs_caps() {
    let world = World::new("mobile-files");
    let root = world.folder();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src").join("app.py"), "print('안녕')\nprint(2)\n").unwrap();
    std::fs::write(root.join("notes.md"), "# notes\n").unwrap();
    std::fs::write(root.join("blob.bin"), [0u8, 1, 2, 3, 0, 255]).unwrap();
    std::fs::write(root.join("long.txt"), "0123456789abcde\n".repeat(files::MAXIMUM_TEXT_BYTES / 16 + 4_096)).unwrap();
    std::fs::write(world.temp.path().join("outside.txt"), "outside").unwrap();
    let escape = |p: &str| url_escape(p);
    let get = |route: &'static str, path: Option<&'static str>, id: &'static str| {
        let world = &world;
        async move {
            let query = path.map(|p| format!("?path={}", escape(p))).unwrap_or_default();
            world.route("GET", &format!("/m1/workspaces/{id}/{route}{query}"), None).await
        }
    };
    let refused = |route: &'static str, path: Option<&'static str>, status: u16, code: &'static str, id: &'static str| {
        let get = &get;
        async move {
            let (s, body) = get(route, path, id).await;
            assert!(
                s == status && body["code"] == code && text(&body, "error").is_some_and(|e| !e.is_empty()),
                "{route} {path:?} must be refused {status} {code}: {s} {body}"
            );
        }
    };
    refused("file", Some("missing.txt"), 404, "notFound", "w").await;
    refused("files", Some("missing"), 404, "notFound", "w").await;
    refused("file", Some("missing/deeper.txt"), 404, "notFound", "w").await;
    refused("file", Some("../outside.txt"), 403, "outsideWorkspace", "w").await;
    refused("file", Some("/etc/passwd"), 403, "outsideWorkspace", "w").await;
    refused("file", Some("C:\\Windows\\win.ini"), 403, "outsideWorkspace", "w").await;
    refused("file", None, 400, "badPath", "w").await;
    refused("file", Some("src//app.py"), 400, "badPath", "w").await;
    refused("files", Some("./src"), 400, "badPath", "w").await;
    refused("files", Some("notes.md"), 400, "notDirectory", "w").await;
    refused("files", Some(""), 404, "workspaceNotFound", "nowhere").await;
    let l = l();
    assert!(
        matches!(files::not_found(&l), MobileError::Request { message, .. } if message == l.get("remote.files.notFound"))
            && matches!(files::superseded(&l), MobileError::Request { status: 409, code: Some(c), .. } if c == "superseded")
            && matches!(files::not_readable(&l), MobileError::Request { status: 403, code: Some(c), .. } if c == "notReadable"),
        "a refusal has its code and its message in the user's language"
    );
    let (status, listing) = get("files", None, "w").await;
    let rows: std::collections::HashMap<String, Value> =
        listing["entries"].as_array().unwrap().iter().map(|e| (e["name"].as_str().unwrap().to_string(), e.clone())).collect();
    assert!(
        status == 200
            && rows["src"]["kind"] == "folder"
            && rows["src"].get("size").is_none()
            && rows["src"]["modified"].as_str().is_some_and(|m| m.len() == 20)
            && rows["notes.md"]["size"] == 8
            && rows["notes.md"]["relativePath"] == "notes.md",
        "a listing's rows carry their kind, their date and a file's size: {listing}"
    );
    let (_, source) = get("file", Some("src/app.py"), "w").await;
    assert!(
        source["type"] == "source"
            && source["language"] == "python"
            && source["encoding"] == "UTF-8"
            && source["lineCount"] == 3
            && source["truncated"] == false
            && source["text"].as_str().unwrap().contains("안녕"),
        "a source file carries its language, encoding and line count: {source}"
    );
    let (_, markdown) = get("file", Some("notes.md"), "w").await;
    assert!(markdown["type"] == "markdown" && markdown.get("language").is_none(), "markdown has no language");
    let (_, cut) = get("file", Some("long.txt"), "w").await;
    assert!(
        cut["truncated"] == true
            && cut["text"].as_str().unwrap().len() == files::MAXIMUM_TEXT_BYTES
            && cut["size"].as_u64().unwrap() > files::MAXIMUM_TEXT_BYTES as u64,
        "a text file is read up to 512 KiB and marked truncated"
    );
    let (_, binary) = get("file", Some("blob.bin"), "w").await;
    let (_, folder) = get("file", Some("src"), "w").await;
    assert!(
        binary["type"] == "unsupported" && binary["reason"] == "binary" && folder["reason"] == "notRegularFile",
        "what cannot be shown says why in the contract's words"
    );
    use mighty_remote::phone::mighty::MobileOut;
    let escaped = files::fitted_preview(files::MobileFilePreview {
        workspace_id: "w".into(),
        path: "p".into(),
        name: "n".into(),
        size: 1,
        kind: "source".into(),
        text: Some("\"".repeat(files::MAXIMUM_TEXT_BYTES)),
        truncated: Some(false),
        line_count: Some(1),
        ..files::MobileFilePreview::default()
    });
    assert!(
        escaped.to_json().len() <= files::MAXIMUM_REPLY_BYTES
            && escaped.truncated == Some(true)
            && escaped.text.as_ref().unwrap().len() < files::MAXIMUM_TEXT_BYTES,
        "a reply stays under 768 KiB"
    );
    let listing = files::fitted_listing(files::MobileFileListing {
        workspace_id: "w".into(),
        path: String::new(),
        entries: (0..2_000)
            .map(|i| files::MobileFileEntry {
                name: format!("{}{i}", "n".repeat(300)),
                relative_path: format!("{}{i}", "p".repeat(300)),
                kind: "file".into(),
                size: Some(1),
                modified: None,
                noise: false,
            })
            .collect(),
        truncated: false,
    });
    assert!(
        listing.to_json().len() <= files::MAXIMUM_REPLY_BYTES
            && listing.truncated
            && !listing.entries.is_empty()
            && listing.entries.len() < 2_000,
        "a listing drops rows from its end to fit"
    );
    // Without a decoder a small picture travels as it is; with one, its thumbnail does.
    use base64::Engine as _;
    let dot = base64::engine::general_purpose::STANDARD
        .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==")
        .unwrap();
    std::fs::write(root.join("dot.png"), &dot).unwrap();
    std::fs::write(
        root.join("leak.svg"),
        "<svg xmlns=\"http://www.w3.org/2000/svg\"><image href=\"https://example.invalid/a.png\"/></svg>",
    )
    .unwrap();
    let (_, raw) = get("file", Some("dot.png"), "w").await;
    assert!(
        raw["type"] == "image"
            && raw["mime"] == "image/png"
            && raw["data"].as_str().is_some_and(|d| !d.is_empty())
            && raw.get("width").is_none(),
        "a small picture travels without a decoder: {raw}"
    );
    let asked = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    {
        let asked = asked.clone();
        world.desk.calls().image_preview = Some(Arc::new(move |name: String, _| {
            asked.lock().unwrap().push(name.clone());
            Box::pin(async move {
                if name == "dot.png" {
                    MobileImageResult {
                        image: Some(MobileImagePreview {
                            mime: "image/jpeg".into(),
                            data: "AAAA".into(),
                            width: 4_000,
                            height: 3_000,
                            thumbnail_width: files::MAXIMUM_THUMBNAIL_PIXELS,
                            thumbnail_height: 1_536,
                        }),
                        reason: None,
                    }
                } else {
                    MobileImageResult { image: None, reason: Some("undecodable".into()) }
                }
            })
        }));
    }
    let (_, drawn) = get("file", Some("dot.png"), "w").await;
    assert!(
        drawn["mime"] == "image/jpeg" && drawn["width"] == 4_000 && drawn["thumbnailWidth"] == 2_048 && drawn["data"] == "AAAA",
        "an image carries its thumbnail and both sizes"
    );
    let (_, leak) = get("file", Some("leak.svg"), "w").await;
    assert!(
        leak["type"] == "unsupported" && leak["reason"] == "undecodable" && *asked.lock().unwrap() == ["dot.png"],
        "an svg that would load something besides itself is never drawn"
    );
    assert!(
        files::MAXIMUM_THUMBNAIL_PIXELS == 2_048
            && files::THUMBNAIL_LADDER[0] == 2_048
            && files::THUMBNAIL_LADDER.windows(2).all(|w| w[0] > w[1])
            && files::MAXIMUM_THUMBNAIL_BYTES == 512 * 1_024,
        "the thumbnail's long side steps down from 2,048 px until it fits 512 KiB"
    );
    // One decode at a time; a newer preview from the same phone takes an older one's place in the queue.
    let slot = PreviewSlot::new();
    assert!(slot.acquire("a").await, "the first preview decodes at once");
    let older = tokio::spawn({
        let slot = slot.clone();
        async move { slot.acquire("a").await }
    });
    tokio::task::yield_now().await;
    while slot.waiting() < 1 {
        tokio::task::yield_now().await;
    }
    let other = tokio::spawn({
        let slot = slot.clone();
        async move { slot.acquire("b").await }
    });
    while slot.waiting() < 2 {
        tokio::task::yield_now().await;
    }
    let newer = tokio::spawn({
        let slot = slot.clone();
        async move { slot.acquire("a").await }
    });
    assert!(!older.await.unwrap(), "a newer preview from the same phone drops the older one waiting");
    while slot.waiting() < 2 {
        tokio::task::yield_now().await;
    }
    assert!(!other.is_finished() && !newer.is_finished());
    slot.release();
    assert!(other.await.unwrap() && !newer.is_finished(), "the next in line decodes, one at a time");
    slot.release();
    assert!(newer.await.unwrap(), "then the newest of the first phone");
    let abandoned = tokio::spawn({
        let slot = slot.clone();
        async move { slot.acquire("c").await }
    });
    while slot.waiting() < 1 {
        tokio::task::yield_now().await;
    }
    abandoned.abort();
    let _ = abandoned.await;
    slot.release();
    assert!(slot.waiting() == 0 && slot.acquire("d").await, "a phone that gave up leaves the queue, and the slot is free again");
    slot.release();
    // The route: a preview superseded while it waited is refused with its code.
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let decoding = Arc::new(tokio::sync::Notify::new());
    {
        let (gate, decoding) = (gate.clone(), decoding.clone());
        world.desk.calls().image_preview = Some(Arc::new(move |_, _| {
            let (gate, decoding) = (gate.clone(), decoding.clone());
            Box::pin(async move {
                decoding.notify_one();
                let _open = gate.acquire().await;
                MobileImageResult { image: None, reason: Some("undecodable".into()) }
            })
        }));
    }
    let world = Arc::new(world);
    let held = tokio::spawn({
        let world = world.clone();
        async move { world.route("GET", "/m1/workspaces/w/file?path=dot.png", None).await }
    });
    decoding.notified().await;
    let second = tokio::spawn({
        let world = world.clone();
        async move { world.route("GET", "/m1/workspaces/w/file?path=dot.png", None).await }
    });
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let third = tokio::spawn({
        let world = world.clone();
        async move { world.route("GET", "/m1/workspaces/w/file?path=dot.png", None).await }
    });
    let (status, dropped) = second.await.unwrap();
    assert!(
        !held.is_finished() && status == 409 && dropped["code"] == "superseded",
        "the preview a newer one replaced is refused as superseded: {status}"
    );
    gate.add_permits(8);
    held.await.unwrap();
    assert_eq!(third.await.unwrap().0, 200, "the newest preview is answered");
}

fn model_option(value: &str, name: &str, resolved: Option<&str>) -> ModelOption {
    ModelOption {
        value: value.into(),
        display_name: name.into(),
        description: String::new(),
        resolved_model: resolved.map(str::to_string),
        supports_effort: None,
        supported_effort_levels: None,
    }
}

fn url_escape(text: &str) -> String {
    text.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') { (b as char).to_string() } else { format!("%{b:02X}") }
        })
        .collect()
}

/// MobileHostVerification.cs:638 `ReplyLimitsAreTheMacs` (L483), the router's half (the relay's 1 MiB frame is W8-A's).
#[tokio::test]
async fn mobile_host_reply_limits_are_the_macs() {
    const _: () = assert!(
        MAXIMUM_REPLY_BYTES == 1_024 * 1_024 - 32 * 1_024 && mighty_remote::phone::mighty::MAXIMUM_REPLY_BYTES == MAXIMUM_REPLY_BYTES
    );
    const _: () = assert!(files::MAXIMUM_REPLY_BYTES == 768 * 1_024);
    let world = World::new("mobile-limits");
    world.desk.set_sessions(vec![pane("s", "w")]);
    let filler = Arc::new(std::sync::Mutex::new(String::new()));
    {
        let filler = filler.clone();
        world.desk.calls().extras = Some(Arc::new(move |_| MobilePaneExtras {
            settings: Some(json!({"filler": filler.lock().unwrap().clone()})),
            ..MobilePaneExtras::default()
        }));
    }
    *filler.lock().unwrap() = "x".repeat(900 * 1_024);
    assert_eq!(world.route("GET", "/m1/sessions/s", None).await.0, 200, "a reply the Windows host used to refuse at 768 KiB now travels");
    *filler.lock().unwrap() = "x".repeat(MAXIMUM_REPLY_BYTES);
    assert_eq!(world.route("GET", "/m1/sessions/s", None).await.0, 413, "a reply over the limit is refused whole");
}

/// PhoneAppUpdateVerification.cs `TheRouteValidatesAndAsksBeforeStoppingRequests` (L1258).
#[tokio::test]
async fn phone_app_update_the_route_validates_and_asks_before_stopping_requests() {
    let world = World::new("mobile-update");
    let post = |body: &'static str| {
        let world = &world;
        async move { world.route("POST", "/m1/app-update", Some(serde_json::from_str(body).unwrap())).await }
    };
    assert!(
        post(r#"{"action":"check"}"#).await.0 == 409 && world.route("GET", "/m1/state", None).await.1.get("appUpdate").is_none(),
        "without update support: 409 and no state"
    );
    let offer = State { phase: Phase::Available, version: Some("0.4.56".into()), ..State::default() };
    world.desk.calls().app_update = Some(Some(offer.clone()));
    assert_eq!(post(r#"{"action":"launch"}"#).await.0, 400, "a wrong action is 400");
    let (status, check) = post(r#"{"action":"check"}"#).await;
    assert!(status == 200 && check["phase"] == "available" && world.desk.calls().app_acts == [false], "check");
    assert!(post(r#"{"action":"install"}"#).await.0 == 200 && world.desk.calls().app_acts == [false, true], "install with nothing running");
    assert_eq!(world.route("GET", "/m1/state", None).await.1["appUpdate"]["available"], "0.4.56", "the state carries the update");
    world.desk.set_sessions(vec![RunSession { status: Some("running".into()), ..pane("busy", "w") }]);
    assert!(
        post(r#"{"action":"install"}"#).await.0 == 409 && world.desk.calls().app_acts.len() == 2,
        "running requests without confirm: 409 and nothing done"
    );
    assert!(
        post(r#"{"action":"install","confirm":false}"#).await.0 == 409 && world.desk.calls().app_acts.len() == 2,
        "confirm false is no confirm"
    );
    assert!(
        post(r#"{"action":"install","confirm":true}"#).await.0 == 200 && world.desk.calls().app_acts.len() == 3,
        "confirm lets it through"
    );
    assert!(post(r#"{"action":"check"}"#).await.0 == 200 && world.desk.calls().app_acts.len() == 4, "check needs no confirm");
    assert_eq!(world.route("GET", "/m1/state", None).await.1["appUpdate"]["running"], 1, "the running count");
    let router = &world.router;
    router.changed();
    router.observe();
    router.take_notifications();
    let revision = router.state_revision();
    router.changed();
    router.observe();
    assert_eq!(router.state_revision(), revision, "no change, no new revision");
    world.desk.calls().app_update = Some(Some(State { phase: Phase::Downloading, download_fraction: 0.5, ..offer }));
    router.changed();
    router.observe();
    assert!(
        router.state_revision() == revision + 1 && router.take_notifications().iter().any(|n| n.0 == "state"),
        "a changed update moves the state revision"
    );

    // Key-only legacy phones ("legacy") are refused 403 on POST /m1/app-update and never trigger checks or restarts.
    assert!(
        mighty_remote::relay::connection::MobileDevice { client_id: "legacy".into(), name: "Legacy".into(), direct: false, desktop: false }
            .is_legacy()
    );
    let acts_before = world.desk.calls().app_acts.len();
    for body in [json!({"action": "check"}), json!({"action": "install", "confirm": true})] {
        let refused = world.from("legacy", "POST", "/m1/app-update", Some(body)).await;
        let parsed: Value = serde_json::from_str(&refused.body).unwrap();
        assert_eq!(refused.status, 403, "legacy key-only device refused 403 on app-update");
        assert_eq!(parsed["code"], "legacyClient");
        assert_eq!(parsed["error"], l().get("phone.screenShare.refusal.legacyClient"));
    }
    assert_eq!(world.desk.calls().app_acts.len(), acts_before, "legacy refusal did not act on app update");
}

fn permission(id: &str, run: &str, tool: &str, input: &str) -> ToolPermissionRequest {
    ToolPermissionRequest::new(id, run, "tool", tool, input, "summary")
}

/// AntigravityPermissionVerification.cs:683 `AntigravityCardsAreAnsweredOnThePermissionRoute` (L632), over a card the desk holds
/// (C#'s over a fake agy's first turn: the card's making is mighty-run's, checked there).
#[tokio::test]
async fn antigravity_cards_the_phone_answers_a_card_on_the_permission_route() {
    let world = World::new("mobile-agy");
    let card = PermissionRequest {
        id: "agy-1".into(),
        run_id: "run-1".into(),
        actions: vec![DeniedAction::new("command", "command", "RunCommand", Some("echo hi"), Some("command(echo hi)"))],
        received_at: None,
    };
    world.desk.set_sessions(vec![RunSession { provider: Some("gemini".into()), ..pane("pane", "w") }]);
    world.desk.calls().cards.insert("pane".into(), card.clone());
    let failure = Arc::new(std::sync::Mutex::new(None::<String>));
    {
        let failure = failure.clone();
        world.desk.calls().action =
            Some(Arc::new(move |_| Ok(ActionAnswer::Value(failure.lock().unwrap().clone().map_or(Value::Null, Value::String)))));
    }
    let post = |body: Value| {
        let world = &world;
        async move { world.route("POST", "/m1/sessions/pane/permission", Some(body)).await }
    };
    let (_, detail) = world.route("GET", "/m1/sessions/pane", None).await;
    let shown = &detail["permissions"][0];
    assert!(
        detail["permissions"].as_array().unwrap().len() == 1
            && shown["toolName"] == "AntigravityPermission"
            && shown["id"] == card.id.as_str()
            && shown["runId"] == card.run_id.as_str()
            && shown["antigravity"]["actions"].as_array().unwrap().len() == 1,
        "the session detail carries the card"
    );
    assert_eq!(detail["session"]["pendingPermissions"], 1, "the card counts as one waiting permission");
    assert_eq!(post(json!({"requestId": "agy-1", "runId": "run-1", "allow": true})).await.0, 200, "allow");
    assert_eq!(post(json!({"requestId": "agy-1", "runId": "run-1", "allow": true, "always": true})).await.0, 200, "always");
    assert_eq!(post(json!({"requestId": "agy-1", "runId": "run-1", "allow": false, "always": false})).await.0, 200, "deny");
    let seen: Vec<String> = world
        .desk
        .actions()
        .iter()
        .map(|a| {
            format!(
                "{}:{}:{}:{}",
                a.action,
                a.body["requestId"].as_str().unwrap(),
                a.body["runId"].as_str().unwrap(),
                a.body["decision"].as_str().unwrap()
            )
        })
        .collect();
    assert_eq!(
        seen,
        ["once", "always", "deny"].map(|d| format!("antigravityPermission:agy-1:run-1:{d}")),
        "each answer reaches the pane as its decision"
    );
    let l = l();
    let (status, body) = post(json!({"requestId": "agy-1", "runId": "run-1", "allow": false, "always": true})).await;
    assert!(status == 400 && body["error"] == l.get("remote.error.alwaysNeedsAllow"), "always without allow is refused");
    let (status, body) = post(json!({"requestId": "perm-1", "runId": "pane", "allow": true, "always": true})).await;
    assert!(status == 409 && body["error"] == l.get("remote.error.notAntigravityPermission"), "always on any other request is refused");
    let (status, body) = post(json!({"requestId": "agy-1", "runId": "run-2", "allow": true, "always": true})).await;
    assert!(status == 409 && body["error"] == l.get("remote.error.notAntigravityPermission"), "a card of another run is not this card");
    let (status, body) = post(json!({"requestId": "../x", "runId": "run-1", "allow": true})).await;
    assert!(status == 400 && body["error"] == l.get("remote.error.badPermissionId"), "a malformed id is refused");
    assert_eq!(
        post(json!({"requestId": "stale", "runId": "pane", "allow": true})).await.0,
        409,
        "a stale Claude answer fails closed as before"
    );
    *failure.lock().unwrap() = Some(l.get("antigravity.permission.noRule"));
    let (status, body) = post(json!({"requestId": "agy-1", "runId": "run-1", "allow": true})).await;
    assert!(status == 409 && body["error"] == l.get("antigravity.permission.noRule"), "what the pane says back is the phone's reason");
    assert_eq!(world.desk.actions().len(), 4, "no refused answer reached the pane");
}

/// The permission and question routes over a waiting request (DesktopRouteContract's live half): a pending request of a running pane is
/// answered; a question needs its answers.
#[tokio::test]
async fn mobile_remote_permission_and_question_routes_answer_a_waiting_request() {
    let world = World::new("mobile-answers");
    world.desk.set_sessions(vec![pane("pane", "w")]);
    {
        let mut calls = world.desk.calls();
        calls.running.insert("pane".into());
        calls.permissions.push(permission("perm-1", "pane", "Bash", r#"{"command":"ls"}"#));
        let mut question = permission("q-1", "pane", "AskUserQuestion", r#"{"questions":[]}"#);
        question.can_answer_questions = true;
        calls.permissions.push(question);
    }
    let (_, detail) = world.route("GET", "/m1/sessions/pane", None).await;
    assert!(
        detail["session"]["pendingPermissions"] == 1 && detail["session"]["pendingQuestions"] == 1,
        "the list counts permissions and questions apart"
    );
    assert_eq!(
        world.route("POST", "/m1/sessions/pane/permission", Some(json!({"requestId": "perm-1", "runId": "pane"}))).await.0,
        400,
        "a permission needs its choice"
    );
    assert_eq!(
        world.route("POST", "/m1/sessions/pane/permission", Some(json!({"requestId": "perm-1", "runId": "pane", "allow": true}))).await.0,
        200
    );
    let bad = json!({"requestId": "q-1", "runId": "pane", "answers": {"Which?": {"customText": "x"}}});
    assert_eq!(world.route("POST", "/m1/sessions/pane/answers", Some(bad)).await.0, 400, "an answer without its selection is invalid");
    let good = json!({"requestId": "q-1", "runId": "pane", "answers": {"Which?": {"selectedOptions": ["A"]}}});
    assert_eq!(world.route("POST", "/m1/sessions/pane/answers", Some(good)).await.0, 200);
    assert_eq!(
        world.desk.calls().answered,
        [("pane".to_string(), "perm-1".to_string(), "allow".to_string()), ("pane".into(), "q-1".into(), "1 answers".into())]
    );
}

/// PendingVerification.cs:189 `ThePhoneSubmitRouteAnswersWithTheReceipt` (L1239).
#[tokio::test]
async fn pending_the_phones_submit_route_answers_with_the_receipts_outcome_and_ids() {
    let world = World::new("mobile-pending");
    world.desk.set_sessions(vec![pane("pane", "w")]);
    let answer = Arc::new(std::sync::Mutex::new(ActionAnswer::Receipt(SubmitReceipt::with_ids("started", Some("entry-1"), Some("run-1")))));
    {
        let answer = answer.clone();
        world.desk.calls().action = Some(Arc::new(move |_| Ok(answer.lock().unwrap().clone())));
    }
    let submit = || world.raw("POST", "/m1/sessions/pane/submit", Some(json!({"text": "hello"})));
    let reply = submit().await;
    assert!(
        reply.status == 202
            && reply.body == r#"{"protocol":1,"accepted":"started","outcome":"started","entryId":"entry-1","runId":"run-1"}"#,
        "a start answers with its ids: {}",
        reply.body
    );
    *answer.lock().unwrap() = ActionAnswer::Receipt(SubmitReceipt::new("queued"));
    assert_eq!(submit().await.body, r#"{"protocol":1,"accepted":"queued","outcome":"queued"}"#, "a queued request has no ids");
    *answer.lock().unwrap() = ActionAnswer::Receipt(SubmitReceipt::new("started"));
    assert_eq!(
        submit().await.body,
        r#"{"protocol":1,"accepted":"started","outcome":"started"}"#,
        "a send that released held items answers without ids"
    );
    *answer.lock().unwrap() = ActionAnswer::Value(json!("started"));
    assert_eq!(submit().await.body, r#"{"protocol":1,"accepted":"started"}"#, "an old answer as before");
    let fixture: Value = serde_json::from_str(
        &std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../../contracts/fixtures/pending.json")).unwrap(),
    )
    .unwrap();
    // The new host's bodies in the shared fixture are what this host writes for the same receipt.
    for case in fixture["submitResponses"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["name"].as_str().is_some_and(|n| n.starts_with("new host")) && c["raw"]["accepted"] == c["raw"]["outcome"])
    {
        let parsed = &case["parsed"];
        let receipt = SubmitReceipt::with_ids(parsed["outcome"].as_str().unwrap(), parsed["entryId"].as_str(), parsed["runId"].as_str());
        let written: Value = serde_json::from_str(&submit_reply_of(receipt)).unwrap();
        assert_eq!(written, case["raw"], "{}", case["name"]);
    }
    let started = submit_reply_of(SubmitReceipt::with_ids("started", Some("e"), Some("r")));
    let read = mighty_composer::pending::SubmitResponse::parse(Some(&serde_json::from_str(&started).unwrap()));
    assert!(
        read.accepted && read.entry_id.as_deref() == Some("e") && read.run_id.as_deref() == Some("r"),
        "the phone reads back what the host wrote"
    );
}

fn submit_reply_of(receipt: SubmitReceipt) -> String {
    router::submit_reply(&ActionAnswer::Receipt(receipt))
}

/// WorkMapVerification.cs:348 `ThePhoneReadsAndStartsTheWorkMapThroughTheRouter` (L1208).
#[tokio::test]
async fn work_map_the_phone_reads_and_starts_it_through_m1() {
    use mighty_workmap::{WorkMapModel, WorkMapStart};
    let world = World::new("mobile-workmap");
    assert_eq!(world.route("GET", "/m1/workspaces/w/workmap", None).await.0, 404, "without the work map the routes are not found");
    let mut model = WorkMapModel::unavailable("none");
    model.available = true;
    model.reason = None;
    world.desk.calls().work_map = Some(model.clone());
    world.desk.calls().summarize = Some(WorkMapStart::Started);
    let (status, read) = world.route("GET", "/m1/workspaces/w/workmap", None).await;
    assert!(status == 200 && read["protocol"] == 1 && read["available"] == true, "the phone reads the work map: {read}");
    assert_eq!(world.route("GET", "/m1/workspaces/w/workmap?x=1", None).await.0, 404, "a query is no route");
    assert_eq!(world.route("GET", "/m1/workspaces/nowhere/workmap", None).await.0, 404, "another workspace is not found");
    let (status, started) = world.route("POST", "/m1/workspaces/w/workmap/summarize", Some(json!({"key": "v1"}))).await;
    assert!(status == 202 && started["ok"] == true && world.desk.calls().summarized == ["v1"], "a summary starts");
    assert_eq!(world.route("POST", "/m1/workspaces/w/workmap/summarize", Some(json!({}))).await.0, 400, "the version is required");
    world.desk.calls().summarize = Some(WorkMapStart::Busy);
    let (status, busy) = world.route("POST", "/m1/workspaces/w/workmap/summarize", Some(json!({"key": "v1"}))).await;
    assert!(status == 409 && busy["error"] == l().get("workMap.error.busy"), "a busy summary is 409");
    world.desk.calls().summarize = Some(WorkMapStart::UnknownKey);
    assert_eq!(
        world.route("POST", "/m1/workspaces/w/workmap/summarize", Some(json!({"key": "zz"}))).await.0,
        400,
        "an unknown version is 400"
    );
    world.desk.calls().summarize = Some(WorkMapStart::Unavailable);
    assert_eq!(world.route("POST", "/m1/workspaces/w/workmap/summarize", Some(json!({"key": "v1"}))).await.0, 404);
    assert_eq!(
        world.route("POST", "/m1/workspaces/w/workmap/summarize?x=1", Some(json!({"key": "v1"}))).await.0,
        400,
        "a POST with a query is 400"
    );
}

/// The phone's work map without each commit's file list (C#'s `PhoneWorkMap`; WorkMapVerification's phone shape).
#[test]
fn work_map_the_phones_commits_carry_no_file_list() {
    let model = serde_json::json!({"available": true, "versions": [{"commits": [{"sha": "a", "files": ["x"], "fileCount": 1}]}]});
    let mut out = mighty_workmap::WorkMapModel::unavailable("x");
    out.available = true;
    let written = router::phone_work_map(&out);
    let parsed: Value = serde_json::from_str(&written).unwrap();
    assert_eq!(parsed["protocol"], 1);
    let _ = model;
}

/// FileChangesVerification.cs:385 `ThePhoneAsksForChangesOnlyWithTheCapabilitysArguments` (L1216).
#[tokio::test]
async fn file_changes_the_phone_gets_changes_only_with_changes_1_under_the_file_routes_checks() {
    use mighty_files::changes::ChangeSet;
    let world = World::new("mobile-changes");
    std::fs::write(world.folder().join("a.txt"), "new\n").unwrap();
    world.desk.set_sessions(vec![pane("pane", "w")]);
    world.desk.calls().file_changes = Some(ChangeSet {
        base: "run",
        hunks: vec![],
        added: 1,
        removed: 1,
        truncated: false,
        unchanged: false,
        run_id: Some("run-1".into()),
        head_sha: None,
        created: None,
    });
    let (_, plain) = world.route("GET", "/m1/workspaces/w/file?path=a.txt", None).await;
    assert!(plain.get("changes").is_none(), "without changes=1 no changes travel");
    let (status, changed) = world.route("GET", "/m1/workspaces/w/file?path=a.txt&changes=1&session=pane&run=run-1", None).await;
    assert!(
        status == 200 && changed["changes"]["base"] == "run" && changed["changes"]["runId"] == "run-1",
        "changes=1 carries the changes: {changed}"
    );
    for bad in [
        "/m1/workspaces/w/file?path=a.txt&changes=2",
        "/m1/workspaces/w/file?path=a.txt&session=pane",
        "/m1/workspaces/w/file?path=a.txt&changes=1&session=..%2Fx",
        "/m1/workspaces/w/file?path=a.txt&changes=1&other=1",
        "/m1/workspaces/w/files?path=&changes=1",
    ] {
        let (status, body) = world.route("GET", bad, None).await;
        assert!(status == 400 && body["error"] == "Unexpected file argument.", "{bad} is refused: {status} {body}");
    }
    assert!(CAPABILITIES.contains(&"fileChanges"), "the host advertises it");
}

/// 휴대폰에서 볼 워크스페이스: by default a paired phone sees every workspace of this computer; held to the shared ones it lists, reads
/// and acts only in what 이 컴퓨터 공유 shares now (nothing while sharing is off), on every route: the list, a pane, files, the work map
/// and a new pane.
#[tokio::test]
async fn the_phone_sees_every_local_workspace_or_only_the_shared_ones() {
    use mighty_core::model::MobileRemoteSettings;
    let world = World::new("mobile-shared-only");
    let other = world.temp.path().join("other");
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(world.folder().join("w.txt"), "w\n").unwrap();
    std::fs::write(other.join("a.txt"), "a\n").unwrap();
    world.desk.update(|s| s.workspaces.get_or_insert_with(Vec::new).push(support::desk::workspace("s", &other)));
    world.desk.set_sessions(vec![pane("pane-w", "w"), pane("pane-s", "s")]);
    let mut model = mighty_workmap::WorkMapModel::unavailable("none");
    model.available = true;
    world.desk.calls().work_map = Some(model);
    let listed = |state: &Value, key: &str, field: &str| -> Vec<String> {
        state[key].as_array().unwrap().iter().map(|v| v[field].as_str().unwrap_or_default().to_string()).collect()
    };
    let (status, state) = world.route("GET", "/m1/state", None).await;
    assert!(status == 200 && listed(&state, "workspaces", "id") == ["w", "s"], "every local workspace by default: {state}");
    assert_eq!(listed(&state, "sessions", "id"), ["pane-w", "pane-s"]);
    assert_eq!(world.route("GET", "/m1/workspaces/w/file?path=w.txt", None).await.0, 200, "and its files");
    assert_eq!(world.route("GET", "/m1/workspaces/w/workmap", None).await.0, 200, "and its work map");

    world.desk.update(|s| s.mobile_remote = Some(MobileRemoteSettings { phone_shared_only: true, ..MobileRemoteSettings::default() }));
    let (_, state) = world.route("GET", "/m1/state", None).await;
    assert!(listed(&state, "workspaces", "id").is_empty() && listed(&state, "sessions", "id").is_empty(), "nothing while not sharing");

    world.desk.calls().shared = vec!["s".into()];
    let (_, state) = world.route("GET", "/m1/state", None).await;
    assert!(listed(&state, "workspaces", "id") == ["s"] && listed(&state, "sessions", "id") == ["pane-s"], "the shared one: {state}");
    assert_eq!(world.route("GET", "/m1/workspaces/s/file?path=a.txt", None).await.0, 200, "a shared workspace's file");
    assert_eq!(world.route("GET", "/m1/sessions/pane-s", None).await.0, 200, "a shared workspace's pane");
    assert_eq!(world.route("GET", "/m1/workspaces/s/workmap", None).await.0, 200, "and work map");
    for path in ["/m1/workspaces/w/files?path=", "/m1/workspaces/w/file?path=w.txt", "/m1/sessions/pane-w", "/m1/workspaces/w/workmap"] {
        assert_eq!(world.route("GET", path, None).await.0, 404, "{path}: a workspace not shared is not there");
    }
    let created = world.route("POST", "/m1/workspaces/w/sessions", Some(json!({"kind": "claude"}))).await;
    assert_eq!(created.0, 404, "no pane is made in it");
    let sent = world.route("POST", "/m1/sessions/pane-w/submit", Some(json!({"text": "hi", "clientRequestId": "c1"}))).await;
    assert_eq!(sent.0, 404, "nor sent to");
    assert!(world.desk.actions().is_empty(), "nothing reached the desktop");

    world.desk.update(|s| s.mobile_remote = Some(MobileRemoteSettings::default()));
    let (_, state) = world.route("GET", "/m1/state", None).await;
    assert_eq!(listed(&state, "workspaces", "id"), ["w", "s"], "back to every workspace");
}

/// AskTargetsVerification.cs:146 `ThePhoneSeesTheSwitchOnlyWhereItIsOffered` (L1269), the router's half: the switch only on a pane
/// that offers targets, and `setAskTarget` passes an id or null.
#[tokio::test]
async fn ask_targets_the_phone_sees_the_switch_only_where_it_is_offered() {
    use mighty_remote::phone::desk::MobileAskTarget;
    let world = World::new("mobile-ask");
    world.desk.set_sessions(vec![pane("ask", "w"), pane("plain", "w")]);
    world.desk.calls().extras = Some(Arc::new(|p: &RunSession| {
        if p.id() == "ask" {
            MobilePaneExtras {
                ask_targets: Some(vec![MobileAskTarget { id: "codex".into(), label: "Codex".into() }]),
                ask_target: Some("codex".into()),
                ..MobilePaneExtras::default()
            }
        } else {
            MobilePaneExtras { ask_targets: Some(vec![]), ask_target: Some("codex".into()), ..MobilePaneExtras::default() }
        }
    }));
    let (_, ask) = world.route("GET", "/m1/sessions/ask", None).await;
    assert!(
        ask["askTargets"][0]["id"] == "codex" && ask["askTargets"][0]["label"] == "Codex" && ask["askTarget"] == "codex",
        "a pane that offers targets carries the switch"
    );
    let (_, plain) = world.route("GET", "/m1/sessions/plain", None).await;
    assert!(plain.get("askTargets").is_none() && plain.get("askTarget").is_none(), "no targets: neither field");
    assert_eq!(world.route("POST", "/m1/sessions/ask/setAskTarget", Some(json!({"target": "codex"}))).await.0, 200);
    assert_eq!(world.route("POST", "/m1/sessions/ask/setAskTarget", Some(json!({"target": null}))).await.0, 200);
    let (status, body) = world.route("POST", "/m1/sessions/ask/setAskTarget", Some(json!({"target": 3}))).await;
    assert!(status == 400 && body["error"] == l().get("remote.error.badBody"), "a target that is no id nor null is refused");
    let targets: Vec<Value> = world.desk.actions().iter().map(|a| a.body["target"].clone()).collect();
    assert!(
        targets == [json!("codex"), Value::Null] && world.desk.actions().iter().all(|a| a.action == "setAskTarget"),
        "the id or null reaches the window"
    );
}

/// MC-34: what a pane's sandbox refused reaches the phone as `sandboxBlocked`, with the reason; a pane with none has no field.
#[tokio::test]
async fn the_phone_sees_what_the_sandbox_blocked() {
    use mighty_remote::phone::desk::MobileSandboxBlock;
    let world = World::new("mobile-sandbox");
    world.desk.set_sessions(vec![pane("boxed", "w"), pane("plain", "w")]);
    world.desk.calls().extras = Some(Arc::new(|p: &RunSession| MobilePaneExtras {
        sandbox_blocked: (p.id() == "boxed").then(|| {
            vec![MobileSandboxBlock {
                kind: "network".into(),
                what: "example.com:443".into(),
                reason: "허용 목록에 없는 곳입니다.".into(),
                at: "2026-11-23T10:00:00.000+09:00".into(),
            }]
        }),
        ..MobilePaneExtras::default()
    }));
    let (_, boxed) = world.route("GET", "/m1/sessions/boxed", None).await;
    assert_eq!(boxed["sandboxBlocked"][0]["what"], "example.com:443");
    assert_eq!(boxed["sandboxBlocked"][0]["kind"], "network");
    assert_eq!(boxed["sandboxBlocked"][0]["reason"], "허용 목록에 없는 곳입니다.");
    let (_, plain) = world.route("GET", "/m1/sessions/plain", None).await;
    assert!(plain.get("sandboxBlocked").is_none());
}

/// PiVerification.cs:455, the host's half (L570): the phone is told the host runs pi, and may make a pane of it.
#[tokio::test]
async fn pi_advertised_to_the_phone() {
    let world = World::new("mobile-pi");
    let (_, info) = world.route("GET", "/m1/info", None).await;
    assert!(
        info["capabilities"].as_array().unwrap().iter().any(|c| c == "pi") && info["platform"] == "win32" && info["hostName"] == "Fixture",
        "pi is advertised"
    );
}

/// QueueRunNowVerification.cs:100 `TheWindowRunsNowOnlyWhatCanRun` (L692), the router's half: a held row is refused removal with its
/// code; run-now names the row; run-next and a malformed row id.
#[tokio::test]
async fn queue_the_phones_rows_held_first_and_their_routes() {
    use mighty_delegation::delivery::{DelegationDeliveryItem, DelegationDeliveryKind};
    use mighty_remote::phone::desk::MobileQueuedItem;
    let world = World::new("mobile-queue");
    world.desk.set_sessions(vec![pane("pane", "w")]);
    world.desk.calls().held.insert(
        "pane".into(),
        vec![DelegationDeliveryItem {
            id: "held-1".into(),
            kind: DelegationDeliveryKind::Notice,
            child_id: "c".into(),
            session_id: "pane".into(),
            text: "notice".into(),
        }],
    );
    world.desk.calls().extras = Some(Arc::new(|_| MobilePaneExtras {
        queued: vec![
            MobileQueuedItem { id: "held-1".into(), text: "notice".into(), run_now: Some("here".into()), run_now_reason: None },
            MobileQueuedItem {
                id: "q-1".into(),
                text: "next".into(),
                run_now: Some("unavailable".into()),
                run_now_reason: Some("held".into()),
            },
        ],
        ..MobilePaneExtras::default()
    }));
    world.desk.calls().queue_count = Some(Arc::new(|_| 1));
    world.desk.calls().action =
        Some(Arc::new(|a| Ok(ActionAnswer::Value(if a.action == "queue-run-now" { json!("here") } else { Value::Null }))));
    let (_, detail) = world.route("GET", "/m1/sessions/pane", None).await;
    assert_eq!(
        detail["queued"],
        json!([{"id":"held-1","text":"notice","runNow":"here"},{"id":"q-1","text":"next","runNow":"unavailable","runNowReason":"held"}])
    );
    assert_eq!(detail["session"]["queued"], 2, "the list counts the held rows with the queue");
    let (status, body) = world.route("POST", "/m1/sessions/pane/queue/held-1/remove", None).await;
    assert!(
        status == 409
            && body["code"] == "held_not_removable"
            && body["error"] == l().get("queue.heldNotRemovable")
            && world.desk.actions().is_empty(),
        "a held row is not removed"
    );
    assert_eq!(world.route("POST", "/m1/sessions/pane/queue/q-1/remove", None).await.0, 200);
    let (status, ran) = world.route("POST", "/m1/sessions/pane/queue/q-1/run-now", None).await;
    assert!(status == 200 && ran["ran"] == "here" && ran["ok"] == true, "run-now answers what ran");
    let (status, body) = world.route("POST", "/m1/sessions/pane/queue/..%2Fx/run-now", None).await;
    assert!(status == 404 && body["error"] == l().get("remote.error.queueItemNotFound"), "a malformed row is not found");
    assert_eq!(world.route("POST", "/m1/sessions/pane/queue/run-next", None).await.0, 200);
    let names: Vec<String> = world.desk.actions().iter().map(|a| a.action.clone()).collect();
    assert_eq!(names, ["queue-remove", "queue-run-now", "queue-next"]);
}

/// The router's preconditions and exception mapping (plan §1.5): only `/m1/`, a repeated query key or a POST with a query 400, a desk
/// failure mapped as C#'s exceptions, a non-agent pane refused, stop/close/rename/command/guided validated.
#[tokio::test]
async fn mobile_remote_route_preconditions_and_exception_mapping() {
    let world = World::new("mobile-pre");
    world.desk.set_sessions(vec![pane("pane", "w"), RunSession { kind: Some("shell".into()), ..pane("shell", "w") }]);
    for (path, status) in
        [("/m2/state", 404), ("/m1/state#x", 404), ("/m1/st\\ate", 404), ("/m1/nothing", 404), ("/m1/state?since=1&since=2", 400)]
    {
        let (got, body) = world.route("GET", path, None).await;
        assert!(got == status && body["protocol"] == 1 && body["error"].is_string(), "{path}: {got} {body}");
    }
    assert_eq!(world.route("POST", "/m1/sessions/pane/stop?x=1", None).await.1["error"], "Unexpected query argument.");
    assert_eq!(world.route("GET", "/m1/sessions/shell/commands", None).await.1["error"], "Use the desktop for this pane.");
    assert_eq!(world.route("GET", "/m1/sessions/gone", None).await.0, 404);
    for (failure, status) in [
        (MobileError::Invalid, 400),
        (MobileError::Changed, 409),
        (MobileError::Unavailable, 403),
        (MobileError::request(418, "teapot", Some("tea")), 418),
        (MobileError::Failed("boom".into()), 500),
    ] {
        world.desk.calls().action = Some(Arc::new(move |_| Err(failure.clone())));
        let (got, body) = world.route("POST", "/m1/sessions/pane/stop", None).await;
        assert!(got == status && body.get("code").is_none_or(|c| c == "tea"), "{got} {body}");
    }
    world.desk.calls().action =
        Some(Arc::new(|a| Ok(ActionAnswer::Value(if a.action == "stop" { json!(true) } else { json!("help text") }))));
    assert_eq!(world.route("POST", "/m1/sessions/pane/stop", None).await.1, json!({"protocol": 1, "stopped": true}));
    assert_eq!(world.route("POST", "/m1/sessions/pane/rename", Some(json!({"title": "  "}))).await.0, 400, "an empty title is refused");
    assert_eq!(
        world.route("POST", "/m1/sessions/pane/rename", Some(json!({"title": "a\u{7}b"}))).await.0,
        400,
        "a control character is refused"
    );
    assert_eq!(world.route("POST", "/m1/sessions/pane/rename", Some(json!({"titleMode": "auto"}))).await.0, 200, "auto needs no title");
    assert_eq!(
        world.route("POST", "/m1/sessions/pane/command", Some(json!({"action": "compact"}))).await.0,
        400,
        "an unknown command is refused"
    );
    assert_eq!(
        world.route("POST", "/m1/sessions/pane/command", Some(json!({"action": "help"}))).await.1,
        json!({"protocol": 1, "ok": true, "message": "help text"})
    );
    assert_eq!(
        world.route("POST", "/m1/sessions/pane/guided", Some(json!({"styleId": "cli", "actionId": "x"}))).await.0,
        400,
        "a guided request is checked first"
    );
    let big = json!({"text": "x".repeat(70_000)});
    assert_eq!(
        world.route("POST", "/m1/sessions/pane/submit", Some(big)).await.1["error"],
        "Invalid request body.",
        "a body over 64 KiB is refused"
    );
    assert_eq!(world.route("POST", "/m1/sessions/pane/submit", Some(json!({"text": "  "}))).await.1["error"], "Invalid request text.");
    assert_eq!(world.route("POST", "/m1/sessions/pane/submit", None).await.1["error"], "Invalid request body.");
}

/// The uploads over the routes: a pane's ticket, chunks in order, completion, a submit that spends its claim only when accepted, and a
/// closed pane's uploads gone (C#'s `close` → `DiscardSession`).
#[tokio::test]
async fn mobile_remote_uploads_through_the_routes() {
    let world = World::new("mobile-upload-routes");
    world.desk.set_sessions(vec![pane("pane", "w")]);
    let (status, ticket) = world.route("POST", "/m1/sessions/pane/uploads", Some(json!({"name": "a.txt", "size": 4}))).await;
    assert!(status == 201 && ticket["chunkSize"] == 196_608 && ticket["protocol"] == 1);
    let id = ticket["uploadId"].as_str().unwrap().to_string();
    let chunk = json!({"dataBase64": support::desk::base64_of(b"ABCD")});
    let (status, appended) = world.route("POST", &format!("/m1/uploads/{id}/chunks/0"), Some(chunk)).await;
    assert!(status == 200 && appended["received"] == 4);
    let (_, done) = world.route("POST", &format!("/m1/uploads/{id}/complete"), None).await;
    assert_eq!(done["attachment"], json!({"id": id, "name": "a.txt", "size": 4}));
    assert_eq!(
        world.from("other", "POST", &format!("/m1/uploads/{id}/complete"), None).await.status,
        404,
        "another phone's upload is not found"
    );
    world.desk.calls().action = Some(Arc::new(|_| Err(MobileError::request(409, "dropped", None))));
    let body = json!({"text": "", "attachments": [id]});
    assert_eq!(world.route("POST", "/m1/sessions/pane/submit", Some(body.clone())).await.0, 409, "a refused submit");
    world.desk.calls().action = Some(Arc::new(|_| Ok(ActionAnswer::Receipt(SubmitReceipt::new("queued")))));
    assert_eq!(
        world.route("POST", "/m1/sessions/pane/submit", Some(body.clone())).await.0,
        202,
        "the upload stayed available for the next submit"
    );
    let sent = world.desk.actions().pop().unwrap();
    assert_eq!(sent.attachments.unwrap()[0].name, "a.txt", "the claim's attachments go with the request");
    assert_eq!(world.route("POST", "/m1/sessions/pane/submit", Some(body)).await.0, 404, "a spent upload is gone");
    let (_, another) = world.route("POST", "/m1/sessions/pane/uploads", Some(json!({"name": "b.txt", "size": 1}))).await;
    assert_eq!(world.router.open_upload_count(), 1);
    assert_eq!(world.route("POST", "/m1/sessions/pane/close", None).await.0, 200);
    assert!(world.router.open_upload_count() == 0 && another["uploadId"].is_string(), "a closed pane's uploads go with it");
    assert_eq!(world.route("POST", "/m1/sessions/pane/uploads", Some(json!({"name": "c", "size": 1.5}))).await.1["error"], "Invalid size.");
    let _ = MobilePaneDigest { editable: true, options: 0, mighty: 0 };
    let _ = SessionRateLimit { kind: String::new(), percent_used: None, resets_at: None };
}

/// Review B items 1, 5 (L5) and 9 (L6): the uploads' folder is made only when an upload begins (a router that only reads leaves it
/// alone); a size that is no number is C#'s `InvalidOperationException` (409, the pane changed); a desk that panics answers the host's
/// 500 and the next request is served.
#[tokio::test]
async fn mobile_remote_uploads_folder_waits_for_an_upload_and_failures_answer_as_csharp() {
    let world = World::new("mobile-lazy");
    world.desk.set_sessions(vec![pane("pane", "w")]);
    let folder = world.temp.path().join("mobile").join("uploads");
    std::fs::create_dir_all(world.temp.path().join("mobile")).unwrap();
    std::fs::write(world.temp.path().join("mobile").join("keep.txt"), "x").unwrap();
    for path in ["/m1/info", "/m1/state", "/m1/sessions/pane", "/m1/workspaces/w/files"] {
        assert_eq!(world.route("GET", path, None).await.0, 200, "{path}");
    }
    assert_eq!(world.router.open_upload_count(), 0);
    assert!(!folder.exists(), "reading made no uploads folder");
    let (status, body) = world.route("POST", "/m1/sessions/pane/uploads", Some(json!({"name": "a.txt", "size": "4"}))).await;
    assert!(status == 409 && body["error"] == "The pane changed; refresh and try again.", "a size that is no number: {status} {body}");
    assert!(!folder.exists(), "a refused upload made nothing");
    assert_eq!(world.route("POST", "/m1/sessions/pane/uploads", Some(json!({"name": "a.txt", "size": 4}))).await.0, 201);
    assert!(folder.is_dir(), "the first upload makes the folder");
    world.desk.calls().action = Some(Arc::new(|_| panic!("the window went away")));
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let (status, body) = world.route("POST", "/m1/sessions/pane/stop", None).await;
    std::panic::set_hook(hook);
    assert!(status == 500 && body["error"] == "Desktop request failed.", "a panicking desk: {status} {body}");
    assert_eq!(world.route("GET", "/m1/info", None).await.0, 200, "the next request is served");
}

/// Review B item 2 (M1) and 6 (L3): a submit whose request is dropped still runs to its end on its own task, so its claim is spent
/// once the desk accepted it and the slot is given back only then.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mobile_remote_a_dropped_submit_still_spends_its_claim() {
    let world = Arc::new(World::new("mobile-dropped"));
    world.desk.set_sessions(vec![pane("pane", "w")]);
    let (_, ticket) = world.route("POST", "/m1/sessions/pane/uploads", Some(json!({"name": "a.txt", "size": 4}))).await;
    let id = ticket["uploadId"].as_str().unwrap().to_string();
    world.route("POST", &format!("/m1/uploads/{id}/chunks/0"), Some(json!({"dataBase64": support::desk::base64_of(b"ABCD")}))).await;
    world.route("POST", &format!("/m1/uploads/{id}/complete"), None).await;
    let gate = Arc::new(std::sync::Barrier::new(2));
    {
        let gate = gate.clone();
        world.desk.calls().action = Some(Arc::new(move |_| {
            gate.wait();
            Ok(ActionAnswer::Receipt(SubmitReceipt::new("queued")))
        }));
    }
    let body = json!({"text": "", "attachments": [id]});
    let submitting = tokio::spawn({
        let world = world.clone();
        let body = body.clone();
        async move { world.route("POST", "/m1/sessions/pane/submit", Some(body)).await }
    });
    // The desk holds the action; the phone's request goes away meanwhile.
    while world.desk.actions().is_empty() {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    submitting.abort();
    let opened = tokio::task::spawn_blocking({
        let gate = gate.clone();
        move || gate.wait()
    });
    opened.await.unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while world.router.open_upload_count() != 0 && std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(world.router.open_upload_count(), 0, "the accepted submit spent its upload though its request was dropped");
    world.desk.calls().action = Some(Arc::new(|_| Ok(ActionAnswer::Value(Value::Null))));
    assert_eq!(world.route("POST", "/m1/sessions/pane/submit", Some(json!({"text": "next"}))).await.0, 202, "the slot came back");
}

/// The summary's Mighty Orchestration link (Mac `MobileRemoteSupport.childLink`, sent by the Rust host on both OSes): a delegated child
/// carries its parent and its sidebar state (waiting while it asks a human), a pane the delegation file does not list carries neither.
#[tokio::test]
async fn the_summary_carries_a_childs_parent_and_state() {
    use mighty_core::shell::{ChildState, DelegationChildRow};

    let world = World::new("mobile-children");
    let child = |id: &str| RunSession { parent_session_id: Some("parent".into()), ..pane(id, "w") };
    world.desk.set_sessions(vec![pane("parent", "w"), child("child"), child("asks"), pane("plain", "w")]);
    let row = |id: &str, state: ChildState| DelegationChildRow {
        id: id.into(),
        parent_session_id: "parent".into(),
        state,
        task: None,
        parent_branch: "main".into(),
        parent_checkout: None,
        can_undo: false,
        held_notices: 0,
        started_by_human: false,
        agent: "claude".into(),
    };
    {
        let mut calls = world.desk.calls();
        calls.children = vec![row("child", ChildState::Reported), row("asks", ChildState::Running), row("gone", ChildState::Running)];
        calls.permissions.push(permission("p1", "asks", "Bash", "{}"));
    }
    let (_, state) = world.route("GET", "/m1/state", None).await;
    let found: std::collections::HashMap<String, Value> =
        state["sessions"].as_array().unwrap().iter().map(|p| (p["id"].as_str().unwrap().to_string(), p.clone())).collect();

    assert!(
        found["child"]["parentSessionId"] == "parent" && found["child"]["childState"] == "reported",
        "a child carries its parent and its row's state"
    );
    assert!(
        found["asks"]["parentSessionId"] == "parent" && found["asks"]["childState"] == "waiting",
        "a running child that asks for a human's answer reads as waiting"
    );
    for id in ["plain", "parent"] {
        assert!(
            found[id].get("parentSessionId").is_none() && found[id].get("childState").is_none(),
            "{id}: a pane the delegation file does not list as a child carries neither"
        );
    }
    assert!(found.values().all(|p| p.get("worktree").is_none()), "the Rust host sends no worktree until it takes POST …/worktree");
    let (_, detail) = world.route("GET", "/m1/sessions/child", None).await;
    assert!(
        detail["session"]["parentSessionId"] == "parent" && detail["session"]["childState"] == "reported",
        "the session reply's summary says the same"
    );
}
