//! The phone's payloads without a router, ported 1:1 by C# name: MobileHostVerification's pure checks (runs, result files, the
//! transcript's runs, the revisions rule, the permission card and the support rules), MobileFlowVerification over
//! native/contracts/fixtures/mobile-roles.json, PhoneAppUpdateVerification's map and steps, the Antigravity card on the phone
//! (AntigravityPermissionVerification over native/contracts/fixtures/antigravity-permissions.json), BackgroundOutputVerification's phone
//! half, and the queued rows (DelegationDeliveryVerification, QueueRunNowVerification).

mod support;

use std::collections::HashMap;

use mighty_core::app_update_view::{Phase, State};
use mighty_core::graph::{GraphAdvisorCall, GraphAgentFigures, MightyGraphAgent, MightyGraphRun};
use mighty_core::json::Wire;
use mighty_core::locale::Locale;
use mighty_core::model::{AgentActivity, LogEntry, RunSession, SessionRateLimit, Workspace};
use mighty_core::permission::ToolPermissionRequest;
use mighty_delegation::delivery::{DelegationDeliveryItem, DelegationDeliveryKind, mobile_items};
use mighty_graph::result_files::StdFiles;
use mighty_remote::phone::app_update::{self, MobileAppUpdate, PhoneAppUpdateStep};
use mighty_remote::phone::desk::MobileQueuedItem;
use mighty_remote::phone::flow::{self, Roles};
use mighty_remote::phone::mighty::{
    self, MobileOut, MobilePermission, MobilePermissionField, MobileResultFile, ResultFilesMemo, RunsInputs,
};
use mighty_remote::phone::revisions::{MobileRevisions, Reading};
use mighty_run::antigravity::permissions::{DeniedAction, PermissionRequest};
use serde_json::{Value, json};
use support::desk::Temp;

/// The checks' language: English for every check of this binary (the Antigravity sample's words are English), set before any reads it,
/// so no check changes it under another running beside it.
fn l() -> Locale {
    mighty_run::strings::set_language("en");
    mighty_run::strings::locale()
}

fn stamp(second: i64) -> String {
    format!("2026-10-12T00:00:{second:02}.0000000+00:00")
}

fn entry(id: &str, kind: &str, text: &str, second: i64) -> LogEntry {
    LogEntry::new(id.into(), kind, text.into(), stamp(second), None)
}

fn block(id: &str, kind: Option<&str>, status: &str, title: &str) -> MightyGraphAgent {
    MightyGraphAgent {
        id: Some(id.into()),
        kind: kind.map(str::to_string),
        status: Some(status.into()),
        title: Some(title.into()),
        input: Some(String::new()),
        entries: Some(vec![]),
        ..MightyGraphAgent::default()
    }
}

fn run(id: &str, input: &str, status: &str) -> MightyGraphRun {
    MightyGraphRun {
        id: Some(id.into()),
        input: Some(input.into()),
        status: Some(status.into()),
        root_entries: Some(vec![]),
        agents: Some(vec![]),
        result_entries: Some(vec![]),
        ..MightyGraphRun::default()
    }
}

fn runs(values: &[MightyGraphRun]) -> Vec<mighty::MobileMightyRun> {
    mighty::runs(values, &RunsInputs::default(), &l())
}

fn files() -> StdFiles {
    StdFiles { ignores_case: cfg!(windows) }
}

/// MobileHostVerification.cs:34 `RunsCarryTheMacsShapeAndCaps` (L470), but the style's request names (W9's styles).
#[test]
fn mobile_host_runs_carry_the_macs_shape_and_caps() {
    let l = l();
    let mut r = run("run-1", "tidy up", "completed");
    r.agents = Some(vec![
        block("a1", None, "running", ""),
        block("a2", Some("task"), "waiting", ""),
        block("a3", Some("steer"), "completed", ""),
        block("a4", Some("compact"), "error", ""),
        block("a5", Some("question"), "stopped", ""),
        block("a6", Some("wormhole"), "idle", ""),
    ]);
    let blocks = mighty::blocks(&r, 3, None, None, None, &[], &l);
    let kinds: Vec<&str> = blocks.iter().map(|b| b.kind.as_str()).collect();
    assert_eq!(
        kinds,
        ["main", "agent", "task", "steer", "compact", "question", "agent"],
        "a kind this build has never heard of is a sub-agent"
    );
    let statuses: Vec<&str> = blocks.iter().map(|b| b.status.as_str()).collect();
    assert_eq!(
        statuses,
        ["completed", "running", "waiting", "completed", "error", "stopped", "running"],
        "every status lands on one of the phone's five words"
    );
    assert!(
        blocks[0].id == "run-1:main"
            && blocks[0].title == l.get_with("remote.block.request", &[("ordinal", "3")])
            && blocks[0].summary.as_deref() == Some("tidy up"),
        "the request block is numbered in the user's language and carries the request as its summary"
    );
    let titles: Vec<String> = blocks[1..].iter().map(|b| b.title.clone()).collect();
    assert_eq!(
        titles,
        [
            l.get("graph.block.agent"),
            l.get("graph.block.task"),
            l.get("graph.block.steer"),
            mighty_graph::labels::compaction_title(&l),
            l.get("graph.block.question"),
            l.get("graph.block.agent")
        ],
        "a block's title is the desktop's own"
    );
    let mut named = run("r", "", "running");
    named.agents = Some(vec![block("a", Some("task"), "running", "run the tests")]);
    assert_eq!(mighty::blocks(&named, 1, None, None, None, &[], &l).last().unwrap().title, "run the tests", "a named block keeps its name");
    for raw in ["idle", "starting", "queued", "running", "anything"] {
        assert_eq!(mighty::status(raw), "running", "{raw} reads as running");
    }
    assert!(
        mighty::status("failed") == "error"
            && mighty::status("cancelled") == "stopped"
            && mighty::status("interrupted") == "stopped"
            && mighty::status("waiting") == "waiting"
    );
    let cut = mighty::output(Some(&"가".repeat(3_000)), mighty::MAXIMUM_BLOCK_OUTPUT).unwrap();
    assert!(
        cut.chars().count() == mighty::MAXIMUM_BLOCK_OUTPUT
            && cut.ends_with('가')
            && mighty::output(None, 2_000).is_none()
            && mighty::output(Some("   "), 2_000).is_none()
            && mighty::output(Some("답\u{7}변"), 2_000).as_deref() == Some("답변"),
        "a block's output is 2,000 characters, cleaned, absent when empty"
    );
    let accented = "e\u{301}";
    assert!(
        mighty::output(Some(&accented.repeat(30)), 10).as_deref() == Some(accented.repeat(10).as_str())
            && mighty::prefix(&format!("a{accented}b"), 2) == format!("a{accented}"),
        "the cut falls between characters, never inside one"
    );
    let many: Vec<MightyGraphRun> = (1..=25).map(|i| run(&format!("run-{i}"), &format!("request {i}"), "completed")).collect();
    let wire = runs(&many);
    assert!(
        wire.len() == mighty::MIGHTY_RUNS && wire[0].id == "run-6" && wire.last().unwrap().id == "run-25",
        "the newest twenty runs travel, oldest first"
    );
    assert!(
        wire[0].blocks[0].title == l.get_with("remote.block.request", &[("ordinal", "6")])
            && wire.last().unwrap().blocks[0].title == l.get_with("remote.block.request", &[("ordinal", "25")]),
        "the ordinal keeps counting from the pane's own history"
    );
    let title = |input: &str| (input == "request 25").then(|| "Seed".to_string());
    let styled = mighty::runs(&many, &RunsInputs { title: Some(&title), ..RunsInputs::default() }, &l);
    assert!(
        styled.last().unwrap().title.as_deref() == Some("Seed") && wire[0].title.is_none() && runs(&[]).is_empty(),
        "a run carries its style's name for the request; a plain pane has none"
    );
    let answer = "가".repeat(25_000);
    let mut two = vec![
        MightyGraphRun { final_output: Some("earlier answer".into()), ..run("run-1", "first", "completed") },
        MightyGraphRun { final_output: Some(answer), ..run("run-2", "second", "completed") },
    ];
    let sent = runs(&two);
    assert!(
        sent[0].result.is_none() && sent[0].blocks[0].output.as_deref() == Some("earlier answer"),
        "an older run leans on its main block's shorter output"
    );
    assert!(
        sent[1].result.as_ref().is_some_and(|r| r.chars().count() == mighty::MAXIMUM_RUN_RESULT)
            && sent[1].blocks[0].output.as_ref().is_some_and(|o| o.chars().count() == mighty::MAXIMUM_BLOCK_OUTPUT),
        "only the newest settled run carries its result, at 20,000 characters"
    );
    two[1].status = Some("running".into());
    assert!(runs(&two)[1].result.is_none(), "a run still in motion has no result");
    two[1].status = Some("failed".into());
    two[1].final_output = Some("stopped on an error".into());
    let failed = &runs(&two)[1];
    assert!(
        failed.status == "error" && failed.result.as_deref() == Some("stopped on an error"),
        "a failed run is an error with what it said"
    );
    assert_eq!(
        runs(&[run("r", &"x".repeat(40_000), "running")])[0].input.len(),
        mighty::MAXIMUM_TEXT,
        "a request's own text is capped too"
    );
    let activity = |id: &str, kind: &str, state: &str, summary: &str, tool: Option<&str>| AgentActivity {
        id: id.into(),
        provider: "claude".into(),
        kind: kind.into(),
        state: state.into(),
        summary: summary.into(),
        tool_name: tool.map(str::to_string),
        output: None,
        duration_ms: None,
    };
    let mut live = run("live", "work", "running");
    live.root_entries = Some(vec![
        LogEntry { activity: Some(activity("a0", "turn", "running", "turn", None)), ..entry("t0", "system", "turn", 0) },
        LogEntry { activity: Some(activity("a1", "read", "completed", "", Some("Read"))), ..entry("t1", "system", "", 1) },
        entry("t2", "assistant", "  first line\nsecond", 2),
        LogEntry { activity: Some(activity("a3", "edit", "running", "edit Models.cs", None)), ..entry("t3", "system", "", 3) },
    ]);
    let steps = &mighty::blocks(&live, 1, None, None, None, &[], &l)[0];
    assert!(
        steps.activity.as_deref() == Some(&["Read".to_string(), "first line".into(), "edit Models.cs".into()][..])
            && steps.duration_ms.is_none(),
        "a running block lists its steps"
    );
    live.status = Some("completed".into());
    let done = &mighty::blocks(&live, 1, None, None, None, &[], &l)[0];
    assert!(done.activity.is_none() && done.duration_ms == Some(3_000.0), "a settled block sends no steps and the span of its records");
    let mut crowded = run("c", "", "running");
    crowded.root_entries = Some((0..12).map(|i| entry(&format!("e{i}"), "assistant", &format!("step {i}"), i)).collect());
    let recent = mighty::blocks(&crowded, 1, None, None, None, &[], &l)[0].activity.clone().unwrap();
    assert!(
        recent.len() == mighty::MAXIMUM_BLOCK_ACTIVITY && recent[0] == "step 4" && recent.last().unwrap() == "step 11",
        "only the newest eight steps travel"
    );
    let plain: Value = serde_json::from_str(&runs(&[run("r", "x", "running")]).to_json()).unwrap();
    let mut names: Vec<&String> = plain[0].as_object().unwrap().keys().collect();
    names.sort();
    let mut block_names: Vec<&String> = plain[0]["blocks"][0].as_object().unwrap().keys().collect();
    block_names.sort();
    assert!(
        names == ["blocks", "id", "input", "status"] && block_names == ["id", "kind", "status", "summary", "title"],
        "a field the host has nothing for is absent from the wire"
    );
}

/// MobileHostVerification.cs:85 `RunsCarryTheResultFilesTheDesktopLists` (L471).
#[test]
fn mobile_host_runs_carry_the_result_files_the_desktop_lists() {
    let l = l();
    let temp = Temp::new("mobile-result-files");
    let root = temp.path().join("root");
    std::fs::create_dir_all(root.join("docs")).unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("docs").join("note.md"), "note").unwrap();
    std::fs::write(root.join("src").join("app.ts"), "app").unwrap();
    let outside = temp.path().join("root-outside.txt");
    std::fs::write(&outside, "outside").unwrap();
    let root_text = root.to_string_lossy().into_owned();
    let named = "wrote docs/note.md, see src/app.ts:12 and docs/note.md:3 again; not docs/missing.md or https://example.com/src/app.ts";
    let made = |id: &str, status: &str, output: &str, notice: bool| MightyGraphRun {
        final_output: Some(output.into()),
        notice_run: notice.then_some(true),
        ..run(id, "q", status)
    };
    let lookup = files();
    let with_root = |values: &[MightyGraphRun], memo: Option<&ResultFilesMemo>, folder: &str| {
        mighty::runs(
            values,
            &RunsInputs { workspace_root: Some(folder), lookup: Some(&lookup), result_files: memo, ..RunsInputs::default() },
            &l,
        )
    };
    let mut values = vec![
        made("old", "completed", "older named docs/note.md", false),
        made("new", "completed", &format!("{named} and {}", outside.display()), false),
    ];
    let wire = with_root(&values, None, &root_text);
    assert!(wire[0].result_files.is_none(), "a run that does not carry its result carries no files either");
    let expected =
        vec![MobileResultFile { path: "docs/note.md".into(), line: None }, MobileResultFile { path: "src/app.ts".into(), line: Some(12) }];
    assert_eq!(wire[1].result_files.as_ref(), Some(&expected), "the desktop panel's list: real workspace files only, in order, once each");
    assert!(
        runs(&values)[1].result_files.is_none() && runs(&values)[1].result.is_some(),
        "no folder on this computer: the result travels, its files do not"
    );
    let tail = &with_root(&[made("long", "completed", &format!("{} then src/app.ts", "가".repeat(25_000)), false)], None, &root_text)[0];
    assert!(
        tail.result.as_ref().unwrap().chars().count() == mighty::MAXIMUM_RUN_RESULT
            && tail.result_files.as_deref() == Some(&[MobileResultFile { path: "src/app.ts".into(), line: None }][..]),
        "a file named past the clipped result is still listed"
    );
    for status in ["failed", "stopped"] {
        let one = &with_root(&[made(status, status, named, false)], None, &root_text)[0];
        assert!(one.result.is_some() && one.result_files.is_none(), "a {status} run's result names no files");
    }
    assert!(
        with_root(&[made("running", "running", named, false)], None, &root_text)[0].result_files.is_none(),
        "a run in motion names none"
    );
    let mut open = made("open", "completed", named, false);
    open.agents = Some(vec![block("a", None, "running", "")]);
    assert!(with_root(&[open], None, &root_text)[0].result_files.is_none(), "a run with a block still running is not finished");
    assert!(
        with_root(&[made("none", "completed", "nothing to open here", false)], None, &root_text)[0].result_files.is_none(),
        "a result that names none carries no list"
    );
    let notices = with_root(
        &[made("request", "completed", "docs/note.md", false), made("request:notice:1", "completed", "src/app.ts", true)],
        None,
        &root_text,
    );
    assert!(
        notices[0].result_files.as_ref().unwrap()[0].path == "docs/note.md"
            && notices[1].result_files.as_ref().unwrap()[0].path == "src/app.ts",
        "the newest request and its notice runs each list their own files"
    );
    std::fs::create_dir_all(root.join("many")).unwrap();
    for i in 0..60 {
        std::fs::write(root.join("many").join(format!("f{i:02}.txt")), "").unwrap();
    }
    let crowded_text: Vec<String> = (0..60).map(|i| format!("many/f{i:02}.txt")).collect();
    let crowded =
        with_root(&[made("crowded", "completed", &crowded_text.join(" "), false)], None, &root_text)[0].result_files.clone().unwrap();
    assert!(
        crowded.len() == mighty::MAXIMUM_RUN_RESULT_FILES
            && crowded[0].path == "many/f00.txt"
            && crowded.last().unwrap().path == "many/f49.txt",
        "at most 50 files travel, the first ones"
    );
    let looks = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let memo = {
        let looks = looks.clone();
        ResultFilesMemo::new(move |r, folder| {
            looks.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            mighty::look_through(r, folder, &files())
        })
    };
    let count = || looks.load(std::sync::atomic::Ordering::SeqCst);
    let first = with_root(&values, Some(&memo), &root_text);
    assert!(count() == 1 && first[1].result_files == wire[1].result_files && memo.count() == 1, "the first reply looks the result through");
    let second = with_root(&values, Some(&memo), &root_text);
    assert!(count() == 1 && second[1].result_files == wire[1].result_files, "the next reply for the same result does not look again");
    values[1].final_output = Some(format!("{} and docs/note.md once more", values[1].final_output.clone().unwrap()));
    with_root(&values, Some(&memo), &root_text);
    assert_eq!(count(), 2, "a result that changed is looked through again");
    with_root(&values, Some(&memo), &root.join("docs").to_string_lossy());
    assert_eq!(count(), 3, "another folder is looked through again");
    values[1].status = Some("running".into());
    with_root(&values, Some(&memo), &root_text);
    assert_eq!(memo.count(), 0, "a run that no longer carries a result is dropped from the memo");
    let json: Value = serde_json::from_str(&wire[1].to_json()).unwrap();
    let list = json["resultFiles"].as_array().unwrap();
    assert!(
        list.len() == 2 && list[0].as_object().unwrap().keys().eq(["path"]) && list[1]["path"] == "src/app.ts" && list[1]["line"] == 12,
        "the wire shape is resultFiles: [{{ path, line? }}]"
    );
    assert!(
        serde_json::from_str::<Value>(&wire[0].to_json()).unwrap().get("resultFiles").is_none(),
        "a run without files has no resultFiles on the wire"
    );
}

/// MobileHostVerification.cs:160 `APaneWithoutAGraphIsGroupedFromItsTranscript` (L472).
#[test]
fn mobile_host_a_pane_without_a_graph_is_grouped_from_its_transcript() {
    let pane = RunSession {
        id: Some("p".into()),
        status: Some("running".into()),
        provider: Some("claude".into()),
        logs: Some(vec![
            entry("o1", "assistant", "an old reply", 0),
            entry("u1", "user", "first request", 1),
            entry("r1", "assistant", "first answer", 2),
            entry("u2", "user", "second request", 3),
            entry("r2", "assistant", "working", 4),
        ]),
        ..RunSession::default()
    };
    let saved = mighty::saved_runs(&pane).into_owned();
    let ids: Vec<&str> = saved.iter().map(|r| r.id()).collect();
    let statuses: Vec<&str> = saved.iter().map(|r| r.status()).collect();
    assert!(
        ids == ["history-p", "u1", "u2"] && statuses == ["completed", "completed", "running"],
        "replies before the first request stand under a run of their own"
    );
    assert!(
        saved[1].input() == "first request"
            && saved[1].final_output() == Some("first answer")
            && saved[1].root_entries().len() == 1
            && saved[1].root_entries()[0].id() == "r1"
            && saved[2].final_output().is_none()
            && saved.iter().all(|r| r.agents().is_empty()),
        "a request's replies are its records and its last reply its answer; no sub-agent is invented"
    );
    let wire = runs(&saved);
    assert!(
        wire.len() == 3
            && wire[1].blocks[0].output.as_deref() == Some("first answer")
            && wire[2].status == "running"
            && wire[2].result.is_none(),
        "the phone gets runs for a pane that has no graph"
    );
    assert!(
        mighty::saved_runs(&RunSession { graph_runs: Some(vec![]), ..pane.clone() }).is_empty()
            && mighty::saved_runs(&RunSession { status: Some("idle".into()), ..pane.clone() }).last().unwrap().status() == "completed",
        "a saved graph is used as it stands; an idle pane's last run has completed"
    );
    let with_logs = |logs: Vec<LogEntry>, status: &str| RunSession { logs: Some(logs), status: Some(status.into()), ..pane.clone() };
    let logs = pane.logs().to_vec();
    let digest = mighty::digest(&pane);
    let mut streamed = logs.clone();
    streamed.last_mut().unwrap().text = Some("working on it".into());
    assert_eq!(digest, mighty::digest(&with_logs(streamed.clone(), "running")), "a streamed token does not move the digest");
    let mut more = logs.clone();
    more.push(entry("r3", "assistant", "more", 5));
    assert_ne!(digest, mighty::digest(&with_logs(more, "running")), "a new record moves the digest");
    assert_ne!(digest, mighty::digest(&with_logs(logs.clone(), "completed")), "the last run's status moves the digest");
    let mut whole = logs.clone();
    whole.last_mut().unwrap().text = Some("the whole answer".into());
    assert_ne!(
        mighty::digest(&with_logs(logs.clone(), "completed")),
        mighty::digest(&with_logs(whole, "completed")),
        "the newest settled run's answer length moves the digest"
    );
    let long: Vec<LogEntry> = (0..60)
        .flat_map(|i| {
            [entry(&format!("u{i}"), "user", &format!("q{i}"), i % 60), entry(&format!("a{i}"), "assistant", &format!("a{i}"), i % 60)]
        })
        .collect();
    let mut changed = long.clone();
    changed[0].text = Some("changed".into());
    let long_pane = RunSession { id: Some("l".into()), logs: Some(long), ..RunSession::default() };
    assert_eq!(
        mighty::digest(&long_pane),
        mighty::digest(&RunSession { logs: Some(changed), ..long_pane.clone() }),
        "requests older than the newest twenty are not hashed"
    );
    let mut graph = vec![MightyGraphRun {
        root_entries: Some(vec![entry("m", "assistant", "reading", 0)]),
        agents: Some(vec![block("a", None, "running", "")]),
        ..run("g", "", "running")
    }];
    let before = mighty::digest_runs(&graph);
    graph[0].root_entries.as_mut().unwrap()[0].text = Some("reading the file".into());
    assert_eq!(mighty::digest_runs(&graph), before, "a saved graph's streamed text does not move its digest");
    graph[0].agents.as_mut().unwrap()[0].status = Some("completed".into());
    assert_ne!(mighty::digest_runs(&graph), before, "a block's status moves it");
}

/// MobileHostVerification.cs:254-262, the rule itself (L473): what a pane's revision is read from.
#[test]
fn mobile_host_a_phone_is_told_only_of_what_changed_the_rule() {
    let mut revisions = MobileRevisions::new();
    let pane = RunSession { id: Some("x".into()), ..RunSession::default() };
    fn reading(value: &RunSession, editable: bool, options: i32, mighty: i32) -> Reading<'_> {
        Reading { pane: value, editable, options, mighty, summary: "s".into(), held: String::new() }
    }
    let none: [Workspace; 0] = [];
    revisions.observe(&none, &[reading(&pane, true, 0, 0)], "");
    assert!(revisions.observe(&none, &[reading(&pane, true, 0, 0)], "").is_empty(), "the same reading moves nothing");
    assert_eq!(
        revisions.observe(&none, &[reading(&pane, false, 0, 0)], ""),
        [("session:x".to_string(), 2)],
        "pickers the phone may no longer use move the pane"
    );
    assert_eq!(revisions.observe(&none, &[reading(&pane, false, 7, 0)], ""), [("session:x".to_string(), 3)]);
    assert_eq!(
        revisions.observe(&none, &[reading(&pane, false, 7, 9)], ""),
        [("session:x".to_string(), 4)],
        "the options offered and the Mighty payload's identity move the pane"
    );
    let effort =
        RunSession { settings: Some(mighty_core::model::RunSettings { effort: "high".into(), ..Default::default() }), ..pane.clone() };
    assert_eq!(
        revisions.observe(&none, &[reading(&effort, false, 7, 9)], ""),
        [("session:x".to_string(), 5)],
        "a setting the phone shows moves the pane"
    );
    let space = [Workspace { id: Some("w".into()), path: Some("C:\\w".into()), ..Workspace::default() }];
    assert_eq!(
        revisions.observe(&space, &[reading(&effort, false, 7, 9)], ""),
        [("state".to_string(), 3)],
        "a workspace moves the list alone"
    );
    let held = Reading { held: "n-1".into(), ..reading(&effort, false, 7, 9) };
    assert_eq!(revisions.observe(&space, &[held], ""), [("session:x".to_string(), 6)], "a held row moves the pane");
    assert_eq!(
        revisions.observe(&space, &[Reading { held: "n-1".into(), ..reading(&effort, false, 7, 9) }], "{}"),
        [("state".to_string(), 4)],
        "the app update moves the list"
    );
}

/// MobileHostVerification.cs:266 `ASessionReplyReadsAsTheDesktopDoes` (L474), the pure half.
#[test]
fn mobile_host_a_session_reply_reads_as_the_desktop_does_its_parts() {
    let l = l();
    let input = r#"{"command":"npm test","description":"Run the tests","timeout":5000}"#;
    let command = MobilePermission::from(&ToolPermissionRequest::new("q1", "run", "tool", "Bash", input, "npm test"), &l);
    let card = mighty_run::presentation::ToolPermissionPresentation::make("Bash", input);
    assert!(
        command.title == card.title && command.headline.as_deref() == Some("Run the tests"),
        "the phone's card has the desktop card's title and headline"
    );
    assert!(
        command
            .fields
            .iter()
            .map(|f| (f.label.clone(), f.value.clone()))
            .eq(card.fields.iter().map(|f| (f.label.clone(), f.value.clone())))
            && command.plan.is_none()
            && command.received_at.is_none()
            && command.questionnaire.is_none()
            && command.can_allow
            && command.summary == "npm test",
        "its fields are the desktop card's, not the raw input"
    );
    let asked = r#"{"questions":[{"header":"Pick","question":"Which?","multiSelect":false,"options":[{"label":"A","description":"a"},{"label":"B","description":"b"}]}]}"#;
    let mut question = ToolPermissionRequest::new("q2", "run", "tool", "AskUserQuestion", asked, "question");
    question.can_allow = false;
    question.can_answer_questions = true;
    assert!(
        MobilePermission::from(&question, &l).questionnaire.is_some_and(|q| q.questions.len() == 1)
            && MobilePermission::from(&ToolPermissionRequest::new("q3", "run", "tool", "AskUserQuestion", asked, "question"), &l)
                .questionnaire
                .is_none(),
        "a question travels as a questionnaire only when it can be answered"
    );
    let mut plan = ToolPermissionRequest::new("q4", "run", "tool", "ExitPlanMode", r##"{"plan":"# Plan\n1. do it"}"##, "plan");
    plan.can_answer_plan = true;
    plan.received_at = Some(stamp(0));
    let planned = MobilePermission::from(&plan, &l);
    assert!(
        planned.plan.as_deref() == Some("# Plan\n1. do it")
            && planned.title == l.get("plan.card.title")
            && planned.headline.is_none()
            && planned.fields == [MobilePermissionField { label: l.get("plan.card.title"), value: "# Plan\n1. do it".into() }]
            && planned.received_at == Some(stamp(0)),
        "a plan carries its text as the plan, and once more as its one field"
    );
    let limits = mighty::rate_limits(
        Some(&[
            SessionRateLimit { kind: "five_hour".into(), percent_used: Some(140.0), resets_at: None },
            SessionRateLimit { kind: "weekly".into(), percent_used: None, resets_at: None },
            SessionRateLimit { kind: "daily".into(), percent_used: Some(12.5), resets_at: Some(stamp(9)) },
            SessionRateLimit { kind: "monthly".into(), percent_used: Some(f64::NAN), resets_at: None },
        ]),
        &l,
    )
    .unwrap();
    assert!(
        limits.len() == 2
            && limits[0].label == l.get("windows.accountUsage.windowSession")
            && limits[0].used_percent == 100.0
            && limits[1].label == l.get("windows.accountUsage.windowDaily")
            && limits[1].resets_at == Some(stamp(9))
            && mighty::rate_limits(Some(&[SessionRateLimit { kind: "weekly".into(), percent_used: None, resets_at: None }]), &l).is_none()
            && mighty::rate_limits(None, &l).is_none(),
        "the account windows travel under the desktop's names, the unmeasured ones left out"
    );
    let entries: Vec<LogEntry> = (0..10).map(|i| entry(&format!("e{i}"), "assistant", &format!("t{i}"), i)).collect();
    let (page, more) = mighty::page(&entries, "e6", 4);
    assert!(
        page.iter().map(LogEntry::id).eq(["e2", "e3", "e4", "e5"])
            && more
            && mighty::page(&entries, "e2", 4).0.len() == 2
            && !mighty::page(&entries, "e2", 4).1,
        "a page is the entries just before the cursor, newest last"
    );
    assert!(
        mighty::page(&entries, "gone", 4).0.is_empty() && !mighty::page(&entries, "gone", 4).1,
        "a cursor the host no longer holds answers empty rather than restarting"
    );
}

/// MobileHostVerification.cs:319 `APaneTravelsAsItsProviderRuns` (L475), the pure half: an Antigravity pane offers no effort.
#[test]
fn mobile_host_a_pane_travels_as_its_provider_runs_its_options() {
    assert!(mighty::effort_options(false, &["low", "high"], &l()).is_none(), "an Antigravity pane offers no effort");
}

/// The bundled styles as the registry loads them from an empty profile (C# `MobileHostVerification.Bundled`).
fn bundled_styles() -> Vec<mighty_styles::RegisteredStyle> {
    let root = Temp::new("styles");
    let profile = root.path().join("profile");
    mighty_styles::StyleRegistry::load(&profile, &root.path().join("workspace"), std::sync::Arc::new(mighty_styles::PlainAccess), l())
        .styles
}

/// MobileHostVerification.cs:358 `SettingsOfferWhatThePaneCanPick` (L476).
#[test]
fn mobile_host_settings_offer_what_the_pane_can_pick() {
    use mighty_remote::phone::legacy_style::{legacy_style_options, style_options};
    let l = l();
    let styles = bundled_styles();
    let offered = style_options(&styles);
    let runnable: Vec<&str> = styles.iter().filter(|s| s.runnable()).map(|s| s.id()).collect();
    assert!(
        offered[0] == mighty::MobileStyleOption { id: "cli".into(), label: "cli".into(), source: None }
            && offered.iter().skip(1).map(|s| s.id.as_str()).eq(runnable.iter().copied())
            && offered.iter().skip(1).all(|s| s.source.as_deref() == Some("bundled") && !s.label.is_empty()),
        "styles are the CLI, then every style that can run, each with its source"
    );
    assert!(!offered[0].to_json().contains("source"), "cli belongs to no source");
    assert!(
        style_options(&[]).len() == 1 && legacy_style_options(&[]) == [mighty::MobileOption::new("cli", "CLI")],
        "a pane outside the Mighty view is offered the CLI alone"
    );
    assert!(
        legacy_style_options(&styles).iter().map(|s| s.id.as_str()).eq(["cli", "ouroboros"]),
        "the old field offers its fixed two, when they can run"
    );
    let flow = styles.iter().find(|s| s.id() == "ouroboros").expect("ouroboros");
    let mut node: Value = serde_json::from_slice(&flow.bytes).expect("ouroboros json");
    node["id"] = json!("paperthin");
    let reserved = serde_json::to_vec(&node).expect("bytes");
    let stranger = mighty_styles::RegisteredStyle::decoded(
        mighty_styles::decoder::decode(&reserved, "bundled", None).expect("bundled decode"),
        "bundled",
        "paperthin.json",
        None,
        "preApproved",
        &reserved,
    );
    let mut with_stranger = styles.clone();
    with_stranger.push(stranger.clone());
    assert!(stranger.id() == "paperthin" && !legacy_style_options(&with_stranger).iter().any(|s| s.id == "paperthin"));
    assert_eq!(
        mighty_styles::decoder::decode(&reserved, "user", None).map(|_| ()).unwrap_err().code,
        "E_RESERVED_ID",
        "paperthin stays a reserved id"
    );
    assert!(
        mighty::MIGHTY_STYLES == ["cli", "ouroboros"]
            && mighty::style(Some("paperthin")) == "cli"
            && mighty::style(Some("claude-plan")) == "cli",
        "the old field has no paperthin word any more"
    );
    assert!(
        mighty::guided_styles_available("claude", "claude", "mighty")
            && !mighty::guided_styles_available("claude", "claude", "plain")
            && !mighty::guided_styles_available("claude", "codex", "mighty")
            && !mighty::guided_styles_available("shell", "claude", "mighty"),
        "guided styles need a Claude pane in the Mighty view"
    );
    assert!(
        mighty::view_mode(Some("mighty")) == "mighty"
            && mighty::view_mode(None) == "mighty"
            && mighty::view_mode(Some("default")) == "mighty"
            && mighty::style(Some("ouroboros")) == "ouroboros"
            && mighty::style(Some("gstack")) == "cli"
            && mighty::style(None) == "cli",
        "the wire words for the view and the old style field"
    );
    assert!(mighty::effort_options(false, &["low", "high"], &l).is_none(), "a provider with no effort setting sends no effort options");
    let efforts = mighty::effort_options(true, &["low", "xhigh"], &l).unwrap();
    assert_eq!(
        efforts,
        [
            mighty::MobileOption::new("default", "Auto"),
            mighty::MobileOption::new("low", "Low"),
            mighty::MobileOption::new("xhigh", "XHigh")
        ],
        "efforts carry the menu's labels, the default first"
    );
    let settings = mighty::MobileSettings {
        editable: true,
        model: "opus".into(),
        permission_mode: "manual".into(),
        effort: None,
        agent_view_mode: "plain".into(),
        mighty_style: "cli".into(),
        style_id: "cli".into(),
        options: mighty::MobileSettingsOptions::default(),
    };
    let written: Value = serde_json::from_str(&settings.to_json()).unwrap();
    assert!(written.get("effort").is_none() && written["options"].get("efforts").is_none(), "effort is absent together with its options");
    assert!(
        mighty::style_after(None, "cli", Some("ouroboros")).as_deref() == Some("ouroboros")
            && mighty::style_after(Some("cli"), "cli", Some("ouroboros")).as_deref() == Some("ouroboros"),
        "a phone handing back what it was told leaves the stored style alone"
    );
    assert!(
        mighty::style_after(Some("cli"), "ouroboros", Some("ouroboros")).is_none()
            && mighty::style_after(Some("claude-plan"), "ouroboros", Some("ouroboros")).as_deref() == Some("claude-plan")
            && mighty::style_after(Some("ouroboros"), "cli", None).as_deref() == Some("ouroboros"),
        "a style other than the one shown is a change"
    );
    let read = |body: Value| mighty::guided_request(body.as_object().unwrap(), &l);
    let both = read(json!({"styleId": "gstack", "actionId": "ship", "style": "ouroboros", "skill": "seed", "text": "  go\n"})).unwrap();
    assert_eq!(
        both,
        mighty::MobileGuidedRequest { style: "gstack".into(), action: "ship".into(), text: "go".into() },
        "the new shape wins when both arrive, and the text is trimmed"
    );
    assert!(
        read(json!({"style": "ouroboros", "skill": "seed"})).unwrap().text.is_empty()
            && read(json!({"styleId": "ouroboros", "skill": "seed"})).unwrap().action == "seed",
        "the older shape and a half-migrated one are read too"
    );
    for bad in [
        json!({"skill": "seed"}),
        json!({"styleId": "cli", "actionId": "x"}),
        json!({"styleId": "Bad Id", "actionId": "x"}),
        json!({"styleId": "ouroboros"}),
        json!({"styleId": "ouroboros", "actionId": "bad action"}),
        json!({"style": "ouroboros", "actionId": "seed"}),
        json!({"styleId": "ouroboros", "actionId": "seed", "text": "x".repeat(mighty::MAXIMUM_TEXT + 1)}),
    ] {
        assert!(
            matches!(read(bad.clone()), Err(mighty_remote::phone::desk::MobileError::Request { status: 400, .. })),
            "a malformed style request is a 400: {bad}"
        );
    }
    assert_eq!(
        mighty::single_line("  first line \r\n\n second\tline  "),
        "first line second\tline",
        "a phone's text for a style button is folded to one line"
    );
    use mighty_composer::slash::SlashArgument;
    assert!(
        mighty::argument_hint(Some(SlashArgument::Model), &l) == Some(l.get("remote.slashHint.model"))
            && mighty::argument_hint(Some(SlashArgument::Permission), &l) == Some(l.get("remote.slashHint.permission"))
            && mighty::argument_hint(None, &l).is_none(),
        "a command's argument hint is in the user's language"
    );
}

fn fixture(name: &str) -> Value {
    let path = format!("{}/../../../contracts/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).expect("the fixture")).expect("JSON")
}

fn fixture_runs(item: &Value) -> Vec<MightyGraphRun> {
    item["runs"].as_array().unwrap().iter().map(|r| MightyGraphRun::read(r).expect("a run")).collect()
}

fn tails(item: &Value) -> mighty::OutputTails {
    item.get("outputTails")
        .and_then(Value::as_object)
        .map(|t| {
            t.iter()
                .map(|(k, v)| (k.clone(), v.as_array().unwrap().iter().map(|l| l.as_str().unwrap_or("").to_string()).collect()))
                .collect()
        })
        .unwrap_or_default()
}

/// `Projection`: the Mighty payload a host sends for a case, the role view in it, cut to the keys the fixture lists.
fn projection(item: &Value) -> Value {
    let ko = Locale::new("ko");
    let values = fixture_runs(item);
    let roles = Roles::new(item.get("provider").and_then(Value::as_str).unwrap_or("claude") == "claude");
    let tails = tails(item);
    let wire = mighty::runs(&values, &RunsInputs { output_tails: Some(&tails), roles: Some(&roles), ..RunsInputs::default() }, &ko);
    let mut output = serde_json::Map::new();
    if roles.fan_out {
        output.insert("fanOut".into(), json!(true));
        if let Some(log) = flow::log(&values, &HashMap::new(), &ko) {
            output.insert("log".into(), serde_json::from_str(&log.to_json()).unwrap());
        }
    }
    let written: Value = serde_json::from_str(&wire.to_json()).unwrap();
    let projected: Vec<Value> = written
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            let mut kept = serde_json::Map::new();
            for key in ["id", "status", "returning", "progress"] {
                if let Some(v) = r.get(key) {
                    kept.insert(key.into(), v.clone());
                }
            }
            let blocks: Vec<Value> = r["blocks"]
                .as_array()
                .unwrap()
                .iter()
                .map(|b| {
                    let mut shown = serde_json::Map::new();
                    for key in ["id", "kind", "title", "status", "background", "role", "card", "delegate"] {
                        if let Some(v) = b.get(key) {
                            shown.insert(key.into(), v.clone());
                        }
                    }
                    Value::Object(shown)
                })
                .collect();
            kept.insert("blocks".into(), Value::Array(blocks));
            Value::Object(kept)
        })
        .collect();
    output.insert("runs".into(), Value::Array(projected));
    Value::Object(output)
}

/// MobileFlowVerification.cs:62 `TheSharedFixtureIsWhatWindowsSends` (L466).
#[test]
fn mobile_role_view_the_shared_fixture_is_what_windows_sends() {
    let cases = fixture("mobile-roles.json")["cases"].as_array().unwrap().clone();
    assert!(cases.len() >= 2, "the fixture has its cases");
    for item in cases {
        let made = projection(&item);
        assert_eq!(made, item["expect"], "{} differs; Windows sends: {}", item["name"], serde_json::to_string_pretty(&made).unwrap());
    }
}

/// MobileFlowVerification.cs:77 `OlderPhonesAndOtherAgents` (L467).
#[test]
fn mobile_role_view_an_older_phone_gets_the_payload_it_always_got() {
    let ko = Locale::new("ko");
    let item = fixture("mobile-roles.json")["cases"][0].clone();
    let values = fixture_runs(&item);
    let tails = tails(&item);
    let plain = mighty::runs(&values, &RunsInputs { output_tails: Some(&tails), ..RunsInputs::default() }, &ko);
    let text = plain.to_json();
    for key in ["role", "card", "delegate", "returning", "progress"] {
        assert!(!text.contains(&format!("\"{key}\"")), "{key} went to an older phone");
    }
    let ids: Vec<Vec<&str>> = plain.iter().map(|r| r.blocks.iter().map(|b| b.id.as_str()).collect()).collect();
    assert_eq!(
        ids,
        [vec!["r1:main"], vec!["r1:notice:1:main", "r1:notice:1:bg"], vec!["r2:main", "scout", "deep"]],
        "a request's background blocks wait for their notice run"
    );
    let claude = mighty::runs(&values, &RunsInputs { roles: Some(&Roles::new(true)), ..RunsInputs::default() }, &ko);
    assert!(
        claude[2].blocks.iter().map(|b| b.id.as_str()).eq(["r2:main", "scout", "ci", "wt", "deep"]),
        "the fan-out is the request's blocks as they were recorded"
    );
    let other = mighty::runs(&values, &RunsInputs { roles: Some(&Roles::new(false)), ..RunsInputs::default() }, &ko);
    assert!(
        other[2].blocks.iter().map(|b| b.id.as_str()).eq(["r2:main", "scout", "deep"])
            && other[2].blocks.iter().all(|b| b.card.is_none() && b.delegate.is_none())
            && other[2].returning.is_none()
            && other[2].blocks.iter().map(|b| b.role.as_ref().map(|r| r.name.as_str())).eq([
                Some("lead"),
                Some("subagent"),
                Some("subagent")
            ]),
        "another agent's pane says who each block is and fans nothing out"
    );
}

/// MobileFlowVerification.cs:97 `DigestMovesWithTheFanOutAndTheAdvisor` (L468).
#[test]
fn mobile_role_view_the_digest_moves_with_the_fan_out_and_the_advisor() {
    let item = fixture("mobile-roles.json")["cases"][0].clone();
    let start = mighty::digest_runs(&fixture_runs(&item));
    let mut stepped = fixture_runs(&item);
    stepped[2].agents.as_mut().unwrap()[2].entries.get_or_insert_with(Vec::new).push(LogEntry::new(
        "wt-step".into(),
        "assistant",
        "초안".into(),
        "2026-11-10T10:06:00Z".into(),
        None,
    ));
    assert_ne!(mighty::digest_runs(&stepped), start, "a background block's step moves the digest");
    let mut counted = fixture_runs(&item);
    counted[2].agents.as_mut().unwrap()[2].figures =
        Some(GraphAgentFigures { started_at: Some("2026-11-10T10:05:00Z".into()), tool_calls: Some(2), ..GraphAgentFigures::default() });
    assert_ne!(mighty::digest_runs(&counted), start, "a background block's figures move the digest");
    let mut consulted = fixture_runs(&item);
    let advisor = consulted[2].advisor.as_mut().expect("the case's advisor");
    advisor
        .calls
        .get_or_insert_with(Vec::new)
        .push(Some(GraphAdvisorCall::read(&json!({"id": "srvtoolu_2", "startedAt": "2026-11-10T10:06:00Z"})).unwrap()));
    assert_ne!(mighty::digest_runs(&consulted), start, "a consultation moves the digest");
}

/// PhoneAppUpdateVerification.cs `TheStateTravelsAsPlainJsonAndKeepsItsNotesShort` (L1255).
#[test]
fn phone_app_update_the_state_travels_as_plain_json_and_keeps_its_notes_short() {
    let state = MobileAppUpdate::new("available", "0.4.55", Some("0.4.56".into()), Some("n".repeat(3_000)), None, None, 2);
    assert_eq!(state.notes.as_ref().unwrap().len(), MobileAppUpdate::MAXIMUM_NOTES, "notes are cut");
    let json: Value = serde_json::from_str(&state.to_json()).unwrap();
    assert!(json["phase"] == "available" && json["running"] == 2 && json["available"] == "0.4.56", "names");
    assert!(json.get("error").is_none() && json.get("progress").is_none(), "absent fields are left out");
    assert!(mighty_remote::phone::router::CAPABILITIES.contains(&"appUpdate"), "the capability");
}

/// PhoneAppUpdateVerification.cs `ThePhasesMapToThePhones` (L1256).
#[test]
fn phone_app_update_the_phases_map_to_the_phones() {
    assert!(app_update::map(None, "1", 0).is_none(), "a build that cannot update has no state");
    let of = |s: State, running: i32| app_update::map(Some(&s), "0.4.55", running).unwrap();
    let at = |phase: Phase| State { phase, ..State::default() };
    let offer = |phase: Phase| State { phase, version: Some("0.4.56".into()), notes: Some("notes".into()), ..State::default() };
    assert!(
        of(State::default(), 0).phase == "idle"
            && of(at(Phase::Checking), 0).phase == "checking"
            && of(at(Phase::UpToDate), 0).phase == "upToDate",
        "idle, checking, upToDate"
    );
    let available = of(offer(Phase::Available), 3);
    assert!(
        available.phase == "available"
            && available.current == "0.4.55"
            && available.available.as_deref() == Some("0.4.56")
            && available.notes.as_deref() == Some("notes")
            && available.running == 3,
        "available carries the version, notes and running count"
    );
    assert!(of(at(Phase::UpToDate), 0).available.is_none(), "an offer that is not newer is no offer");
    let downloading = of(State { download_fraction: 0.456, ..offer(Phase::Downloading) }, 0);
    assert!(downloading.phase == "downloading" && downloading.progress == Some(0.46), "downloading has progress, rounded");
    let staging = of(offer(Phase::Staging), 0);
    assert!(staging.phase == "downloading" && staging.progress == Some(1.0), "staging reads as a full download");
    assert!(of(offer(Phase::Ready), 0).phase == "ready" && of(at(Phase::Installing), 0).phase == "installing", "ready, installing");
    let failed = of(State { error_message: Some("boom".into()), ..at(Phase::Failed) }, 0);
    assert!(failed.phase == "failed" && failed.error.as_deref() == Some("boom"), "failed has the error");
}

/// PhoneAppUpdateVerification.cs `ThePhonesInstallAdvancesAndNeverLoops` (L1257).
#[test]
fn phone_app_update_the_phones_install_advances_and_never_loops() {
    let at = |phase: Phase| State { phase, ..State::default() };
    let offer = |phase: Phase| State { phase, version: Some("0.4.56".into()), ..State::default() };
    for phase in [Phase::Idle, Phase::UpToDate, Phase::Failed] {
        assert_eq!(app_update::begin(&at(phase)), PhoneAppUpdateStep::Check, "what is known may be old: look again from {phase:?}");
    }
    assert_eq!(app_update::begin(&offer(Phase::Ready)), PhoneAppUpdateStep::Install, "a ready update installs at once");
    assert_eq!(app_update::begin(&offer(Phase::Available)), PhoneAppUpdateStep::Download, "an offer downloads");
    for phase in [Phase::Checking, Phase::Downloading, Phase::Staging, Phase::Installing] {
        assert_eq!(app_update::next(&at(phase)), PhoneAppUpdateStep::Wait, "steps under way wait");
    }
    for phase in [Phase::UpToDate, Phase::Idle, Phase::Failed] {
        assert_eq!(app_update::next(&at(phase)), PhoneAppUpdateStep::Done, "nothing newer, or a failure, ends the request");
    }
    assert_eq!(
        app_update::next(&at(Phase::Available)),
        PhoneAppUpdateStep::Done,
        "an available that is not newer ends the request instead of downloading"
    );
}

fn card() -> PermissionRequest {
    PermissionRequest {
        id: "agy-card".into(),
        run_id: "run-1".into(),
        actions: vec![
            DeniedAction::new(
                "command",
                "command",
                "RunCommand",
                Some("curl -s https://example.com"),
                Some("command(curl -s https://example.com)"),
            ),
            DeniedAction::new("other", "notebook_execution", "NotebookExecution", None, None),
        ],
        received_at: Some("2026-10-22T00:00:00Z".into()),
    }
}

/// AntigravityPermissionVerification.cs:494 `ThePhoneSeesTheCardAsAPermissionAnOlderPhoneCanAnswer` (L626).
#[test]
fn antigravity_cards_the_phone_sees_the_card_as_a_permission_an_older_phone_can_answer() {
    let l = l();
    let node: Value = serde_json::from_str(&MobilePermission::from_antigravity(&card(), &l).to_json()).unwrap();
    assert!(node["id"] == "agy-card" && node["runId"] == "run-1" && node["toolName"] == "AntigravityPermission", "ids and tool name");
    assert!(node["canAllow"] == true && node["title"] == l.get("antigravity.permission.title"), "allowable, with the card's title");
    let fields: Vec<(String, String)> = node["fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| (f["label"].as_str().unwrap().into(), f["value"].as_str().unwrap().into()))
        .collect();
    let kind = |k: &str| mighty_run::strings::get(&format!("antigravity.permission.kind.{k}"));
    assert_eq!(
        fields,
        [(kind("command"), "curl -s https://example.com".to_string()), (kind("other"), "NotebookExecution".to_string())],
        "every action is a field an older phone shows"
    );
    let actions = node["antigravity"]["actions"].as_array().unwrap();
    assert!(
        actions.len() == 2 && actions[0]["kind"] == "command" && actions[0]["rule"] == "command(curl -s https://example.com)",
        "the structured actions"
    );
    assert!(
        actions[1].get("rule").is_none() && actions[1].get("target").is_none() && actions[1]["displayName"] == "NotebookExecution",
        "absent fields stay absent"
    );
    let claude: Value =
        serde_json::from_str(&MobilePermission::from(&ToolPermissionRequest::new("p", "r", "t", "Bash", "{}", "ls"), &l).to_json())
            .unwrap();
    assert!(claude.get("antigravity").is_none(), "a Claude permission carries no card");
    use mighty_run::antigravity::permissions::{PermissionDecision, decision};
    assert!(
        decision(true, false) == PermissionDecision::Once
            && decision(true, true) == PermissionDecision::Always
            && decision(false, false) == PermissionDecision::Deny,
        "the answers"
    );
}

/// AntigravityPermissionVerification.cs:534 `ThePhoneSampleIsWhatTheMacSends` (L628), the phone's samples: the card the shared
/// fixture's permission names is written back as the Mac writes it (the /v2 poll's own reading is W8-D's).
#[test]
fn antigravity_cards_the_phone_sample_is_what_this_host_sends() {
    let _ = l();
    let mobile = fixture("antigravity-permissions.json")["mobile"].clone();
    for sample in [&mobile["permission"], &mobile["permissionPlain"]["permission"]] {
        let actions: Vec<DeniedAction> = sample["antigravity"]["actions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| {
                DeniedAction::new(
                    a["kind"].as_str().unwrap(),
                    a["action"].as_str().unwrap(),
                    a["displayName"].as_str().unwrap(),
                    a["target"].as_str(),
                    a["rule"].as_str(),
                )
            })
            .collect();
        let made = PermissionRequest {
            id: sample["id"].as_str().unwrap().into(),
            run_id: sample["runId"].as_str().unwrap().into(),
            actions,
            received_at: sample["receivedAt"].as_str().map(str::to_string),
        };
        let sent: Value = serde_json::from_str(&MobilePermission::from_antigravity(&made, &Locale::new("en")).to_json()).unwrap();
        assert_eq!(&sent, sample, "the phone sample is what this host sends");
    }
    let interpreter = DeniedAction::new("command", "command", "RunCommand", Some("bash"), Some("command(bash)"));
    assert_eq!(
        mighty::MobileAntigravityCard::from(&PermissionRequest::make("r", vec![interpreter])).actions[0].once_only_reason.as_deref(),
        Some("interpreter"),
        "the phone hears why"
    );
}

fn background_run(agents: Vec<MightyGraphAgent>, notice: bool) -> MightyGraphRun {
    MightyGraphRun { agents: Some(agents), notice_run: notice.then_some(true), ..run("run-1", "go", "completed") }
}

/// BackgroundOutputVerification.cs:79 `ThePhoneGetsTheLinesAndTheReplyButNeverTheFile` (L1244), the blocks (the structure's half went
/// with the 파이프라인, MC-113).
#[test]
fn background_output_the_phones_blocks_carry_the_lines_and_the_reply_never_the_file() {
    let l = l();
    let file = "/tmp/example/tasks/a1b2c3.output";
    let task = MightyGraphAgent {
        input: Some("make all".into()),
        background: Some(true),
        output_file: Some(file.into()),
        notice_reply: Some("The build passed.".into()),
        ..block("graph-task", Some("task"), "completed", "Build")
    };
    let agent = MightyGraphAgent {
        input: Some("Survey the docs".into()),
        background: Some(true),
        ..block("graph-agent", None, "running", "Survey")
    };
    let foreground = MightyGraphAgent { input: Some("Look".into()), ..block("graph-front", None, "completed", "Look") };
    let lines: Vec<String> = (1..=8).map(|i| format!("step {i}")).collect();
    let tails: mighty::OutputTails =
        [("graph-task".to_string(), lines.clone()), ("graph-agent".to_string(), vec!["not a run".to_string()])].into();
    let request =
        mighty::blocks(&background_run(vec![task.clone(), agent.clone(), foreground.clone()], false), 1, Some(&tails), None, None, &[], &l);
    assert!(request.iter().map(|b| b.id.as_str()).eq(["run-1:main", "graph-front"]), "a request's background blocks stay off the phone");
    let blocks =
        mighty::blocks(&background_run(vec![task.clone(), agent.clone(), foreground.clone()], true), 1, Some(&tails), None, None, &[], &l);
    let find = |id: &str| blocks.iter().find(|b| b.id == id).unwrap().clone();
    let r = find("graph-task");
    assert!(
        r.background == Some(true) && r.notice_reply.as_deref() == Some("The build passed."),
        "a task block carries background and its notice reply"
    );
    assert_eq!(r.output_tail.as_deref(), Some(&lines[2..]), "a task block carries at most the last six lines");
    let sub = find("graph-agent");
    assert!(
        sub.background == Some(true) && sub.output_tail.is_none() && sub.notice_reply.is_none(),
        "a background sub-agent says so, and carries no lines"
    );
    let front = find("graph-front");
    assert!(
        front.background.is_none() && front.output_tail.is_none() && front.notice_reply.is_none(),
        "a foreground agent carries none of it"
    );
    assert!(
        mighty::blocks(&background_run(vec![task.clone()], true), 1, None, None, None, &[], &l)
            .iter()
            .find(|b| b.id == "graph-task")
            .unwrap()
            .output_tail
            .is_none(),
        "no lines read: no outputTail"
    );
    let json = mighty::runs(
        &[background_run(vec![task, agent, foreground], true)],
        &RunsInputs { output_tails: Some(&tails), ..RunsInputs::default() },
        &l,
    )
    .to_json();
    assert!(!json.contains("outputFile") && !json.contains(file), "the output file never reaches the phone: {json}");
    assert!(
        json.contains("\"outputTail\"") && json.contains("\"noticeReply\"") && json.contains("\"background\":true"),
        "the wire names the fields"
    );
}

fn held() -> Vec<DelegationDeliveryItem> {
    vec![
        DelegationDeliveryItem {
            id: "n-1".into(),
            kind: DelegationDeliveryKind::Notice,
            child_id: "c1".into(),
            session_id: "parent".into(),
            text: "Notice 1".into(),
        },
        DelegationDeliveryItem {
            id: "n-2".into(),
            kind: DelegationDeliveryKind::Notice,
            child_id: "c2".into(),
            session_id: "parent".into(),
            text: "Notice 2".into(),
        },
    ]
}

fn queued() -> Vec<mighty_composer::queue::QueuedComposerInput> {
    vec![mighty_composer::queue::QueuedComposerInput {
        id: "q-1".into(),
        text: "Human row".into(),
        attachments: vec![],
        permission_mode_override: None,
        prepared: false,
        continuation: false,
    }]
}

fn rows(paths: Option<&dyn Fn(&str) -> mighty_composer::queue::QueueRunNowPath>) -> Vec<MobileQueuedItem> {
    mobile_items(&held(), &queued(), paths).into_iter().map(MobileQueuedItem::from).collect()
}

fn item(id: &str, text: &str, run_now: Option<&str>, reason: Option<&str>) -> MobileQueuedItem {
    MobileQueuedItem {
        id: id.into(),
        text: text.into(),
        run_now: run_now.map(str::to_string),
        run_now_reason: reason.map(str::to_string),
        ..MobileQueuedItem::default()
    }
}

/// DelegationDeliveryVerification.cs:368-378 (MobileItems): held rows first in the phone's queued array, in its item shape.
#[test]
fn delegation_delivery_held_rows_come_first_in_the_phones_queue() {
    use mighty_composer::queue::QueueRunNowPath;
    assert_eq!(
        rows(None),
        [item("n-1", "Notice 1", None, None), item("n-2", "Notice 2", None, None), item("q-1", "Human row", None, None)],
        "held rows come first in the phone's queue"
    );
    assert!(mobile_items(&[], &queued(), None).iter().map(|r| r.id.as_str()).eq(["q-1"]), "with nothing held, the human's rows alone");
    let decide = |id: &str| match id {
        "n-1" => QueueRunNowPath::Held,
        "n-2" => QueueRunNowPath::Hidden,
        _ => QueueRunNowPath::Worktree,
    };
    assert_eq!(
        rows(Some(&decide)),
        [
            item("n-1", "Notice 1", Some("here"), None),
            item("n-2", "Notice 2", None, None),
            item("q-1", "Human row", Some("worktree"), None)
        ],
        "the phone's rows carry 바로 실행"
    );
}

/// QueueRunNowVerification.cs:53-56: each phone row carries what 바로 실행 decided, and nothing for a hidden row.
#[test]
fn queue_run_now_each_phone_row_carries_its_run_now() {
    use mighty_composer::queue::QueueRunNowPath;
    let decide = |id: &str| match id {
        "n-1" => QueueRunNowPath::Held,
        "n-2" => QueueRunNowPath::Hidden,
        _ => QueueRunNowPath::WaitsForHeld,
    };
    assert_eq!(
        rows(Some(&decide)),
        [
            item("n-1", "Notice 1", Some("here"), None),
            item("n-2", "Notice 2", None, None),
            item("q-1", "Human row", Some("unavailable"), Some("held"))
        ]
    );
    let world_rows = rows(Some(&decide));
    let mut out = mighty_core::json::JsonOut::new();
    out.begin_object();
    out.prop_str("id", Some(&world_rows[2].id));
    out.prop_str("text", Some(&world_rows[2].text));
    out.prop_str("runNow", world_rows[2].run_now.as_deref());
    out.prop_str("runNowReason", world_rows[2].run_now_reason.as_deref());
    out.end_object();
    assert_eq!(
        out.finish(),
        r#"{"id":"q-1","text":"Human row","runNow":"unavailable","runNowReason":"held"}"#,
        "an unavailable row says why"
    );
}

/// MobileHostVerification.cs:399 `OlderPhonesStillGetTheOuroborosPayload` (L477).
#[test]
fn mobile_host_older_phones_still_get_the_ouroboros_payload() {
    use mighty_remote::phone::legacy_style::payload;
    let styles = bundled_styles();
    let flow = styles.iter().find(|s| s.id() == "ouroboros").expect("ouroboros");
    let phase = |id: &str| flow.manifest.phase(id);
    let seeded = payload(flow, phase("seed"), false).expect("ouroboros payload");
    assert!(seeded.phase == "seed" && !seeded.ready && seeded.next.iter().map(|s| s.skill.as_str()).eq(["run", "evaluate", "status"]));
    assert!(
        seeded.all.iter().map(|s| s.skill.as_str()).eq([
            "interview",
            "auto",
            "seed",
            "run",
            "evaluate",
            "evolve",
            "ralph",
            "status",
            "unstuck",
            "cancel"
        ]) && seeded.takes_text == ["interview", "auto", "unstuck"],
        "the whole catalogue, and which skills take text"
    );
    let seed = seeded.all.iter().find(|s| s.skill == "seed").expect("seed");
    assert!(!seed.title.is_empty() && !seed.help.is_empty(), "a skill carries its title and help");
    let mut phases: Vec<_> = flow.manifest.phases.iter().collect();
    phases.sort_by_key(|p| p.order);
    let ids: Vec<&str> = phases.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(ids, ["goal", "interview", "seed", "run", "evaluate", "evolve"]);
    assert!(ids.iter().all(|id| payload(flow, phase(id), true).expect("payload").phase == *id), "every phase reports itself");
    let start = payload(flow, None, true).expect("payload");
    assert!(start.phase == "goal" && start.next.is_empty() && payload(flow, phase("goal"), true).expect("payload").next.is_empty());
    let deck_bytes =
        std::fs::read(std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../../styles/fixtures/deck.json")).expect("deck");
    let vocabulary = mighty_styles::manifest::Vocabulary::from([(
        "acme.thing".to_string(),
        vec!["absent".to_string(), "open".to_string(), "complete".to_string()],
    )]);
    let deck = mighty_styles::RegisteredStyle::decoded(
        mighty_styles::decoder::decode(&deck_bytes, "user", Some(&vocabulary)).expect("deck"),
        "user",
        "/tmp/deck.json",
        None,
        "approved",
        &deck_bytes,
    );
    assert!(styles.iter().any(|s| s.id() == "claude-plan"));
    assert!(
        styles.iter().filter(|s| s.id() != "ouroboros").chain(std::iter::once(&deck)).all(|s| payload(s, None, true).is_none()),
        "any other style sends none"
    );
    let plan = styles.iter().find(|s| s.id() == "claude-plan").expect("plan");
    let mut wire = serde_json::Map::new();
    if let Some(legacy) = payload(plan, None, true) {
        wire.insert("ouroboros".into(), legacy.to_value());
    }
    assert!(
        !wire.contains_key("ouroboros") && seeded.to_value().get("takesText").is_some(),
        "the payload a style does not have is absent from the wire"
    );
}
