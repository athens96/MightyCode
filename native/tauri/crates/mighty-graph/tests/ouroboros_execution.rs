//! OuroborosExecutionVerification.cs (`Tests\Verification.cs` "ouroboros execution: …", the pure seven): which tool results name a
//! background execution, what is ever connected to, what the loopback dashboard's payloads become, and when a block is read again. The
//! client, the transport and the pane's monitor are src-tauri's `ouroboros.rs` (its tests hold the other four).

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use chrono::{DateTime, TimeDelta, Utc};
use mighty_core::graph::{MightyGraphAgent, MightyGraphRun};
use mighty_core::model::{AgentActivity, LogEntry};
use mighty_graph::ouroboros_execution::{
    self as ox, AcItem, AcStatus, Board, DAEMON_COOLDOWN, DAEMON_MAXIMUM, DaemonStarts, Endpoint, ExecutionStatus, FAST_INTERVAL, Poll,
    QUIET_AFTER, RunSummary, SLOW_INTERVAL, Snapshot, Source, UNKNOWN_BACKOFF_AFTER, UNKNOWN_INTERVAL, UNKNOWN_STOP_AFTER,
};

const CLAUDE_START: &str = "mcp__plugin_ouroboros_ouroboros__ouroboros_start_execute_seed";
const START_TEXT: &str = "Started background execution.\n\nJob ID: job_6bb930bd8b98\nSession ID: orch_1234\nExecution ID: exec_abc123\n\nRuntime Backend: claude\nLive Dashboard: http://localhost:8123/?run=exec_abc123\nUse ouroboros_ac_tree_hud(session_id, cursor) for live progress.\n";

fn entry(id: &str, tool: &str, output: Option<&str>, state: &str) -> LogEntry {
    LogEntry {
        id: Some(id.into()),
        kind: Some("system".into()),
        text: Some(tool.into()),
        timestamp: Some("2026-09-29T00:00:00Z".into()),
        provider: Some("claude".into()),
        activity: Some(AgentActivity {
            id: format!("activity-{id}"),
            provider: "claude".into(),
            kind: "tool".into(),
            state: state.into(),
            summary: tool.into(),
            tool_name: Some(tool.into()),
            output: output.map(str::to_string),
            duration_ms: None,
        }),
        images: None,
    }
}

fn completed(id: &str, tool: &str, output: &str) -> LogEntry {
    entry(id, tool, Some(output), "completed")
}

fn run(id: &str, entries: Vec<LogEntry>) -> MightyGraphRun {
    MightyGraphRun {
        id: Some(id.into()),
        input: Some("request".into()),
        status: Some("completed".into()),
        root_entries: Some(entries),
        ..MightyGraphRun::default()
    }
}

fn at(text: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(text).unwrap().with_timezone(&Utc)
}

fn snapshot(source: Source, status: ExecutionStatus, completed: Option<i32>, total: Option<i32>) -> Snapshot {
    Snapshot { status, completed, total, ..Snapshot::new(source) }
}

fn keys(links: &[ox::Link]) -> Vec<String> {
    links.iter().map(ox::Link::key).collect()
}

// ── links ─────────────────────────────────────────────────────────────────────────────────────────────────────────────────────

/// `StartToolResultsNameTheirExecutions` (L329).
#[test]
fn start_tool_results_name_their_executions() {
    let links = ox::extract(&[run("run-1", vec![completed("a", CLAUDE_START, START_TEXT)])]);
    assert_eq!(links.len(), 1);
    let link = &links[0];
    assert!(
        link.run_id == "run-1"
            && link.tool == "ouroboros_start_execute_seed"
            && link.job_id.as_deref() == Some("job_6bb930bd8b98")
            && link.session_id.as_deref() == Some("orch_1234")
            && link.execution_id.as_deref() == Some("exec_abc123")
            && link.key() == "exec_abc123"
            && link.timestamp == "2026-09-29T00:00:00Z"
            && link.dashboard_url.as_ref().map(|u| u.original.as_str()) == Some("http://localhost:8123/?run=exec_abc123"),
        "a start names its job, session, execution and loopback dashboard: {link:?}"
    );
    // Only the four start tools, and only their settled results.
    let status =
        completed("s", "mcp__plugin_ouroboros_ouroboros__ouroboros_job_status", "**Execution ID**: exec_other\nExecution ID: exec_other");
    let none = run(
        "run-1",
        vec![
            status,
            entry("f", CLAUDE_START, Some(START_TEXT), "error"),
            entry("r", CLAUDE_START, None, "running"),
            completed("l", "mcp__x__not_ouroboros_start_execute_seed", START_TEXT),
        ],
    );
    assert!(ox::extract(&[none]).is_empty(), "another tool, a failed or running start and a lookalike name nothing");
    // Codex names, sub-agent starts and job-only starts, once per execution on the first run that reported it.
    let auto = completed(
        "auto",
        "ouroboros.ouroboros_start_auto",
        "Started background auto session.\n\nStatus: queued\njob_id: job_auto1\nauto_session_id: auto_9\nLive Dashboard: http://127.0.0.1:9000\n",
    );
    let child = MightyGraphAgent {
        id: Some("child".into()),
        status: Some("completed".into()),
        entries: Some(vec![completed("b", CLAUDE_START, START_TEXT)]),
        ..MightyGraphAgent::default()
    };
    let first = MightyGraphRun { input: Some("x".into()), agents: Some(vec![child]), ..run("run-1", vec![auto]) };
    let links = ox::extract(&[first, run("run-2", vec![completed("c", CLAUDE_START, START_TEXT)])]);
    assert!(
        keys(&links) == ["job-job_auto1", "exec_abc123"]
            && links.iter().all(|l| l.run_id == "run-1")
            && links[0].execution_id.is_none()
            && links[0].session_id.as_deref() == Some("auto_9")
            && links[0].dashboard_url.as_ref().map(|u| u.original.as_str()) == Some("http://127.0.0.1:9000"),
        "codex names, sub-agents and job-only starts, once each"
    );
    // A pending execution is taken from a loopback run URL only.
    assert_eq!(
        ox::parse("Job ID: job_1\nExecution ID: pending\nLive Dashboard: http://localhost:8123/?run=exec_from_url\n", "t", "r", "")
            .and_then(|l| l.execution_id)
            .as_deref(),
        Some("exec_from_url"),
        "a pending execution is its run URL's"
    );
    let remote =
        ox::parse("Job ID: job_1\nExecution ID: pending\nLive Dashboard: http://evil.example:8123/?run=exec_from_url\n", "t", "r", "")
            .unwrap();
    assert!(
        remote.execution_id.is_none() && remote.dashboard_url.is_none() && remote.key() == "job-job_1",
        "a dashboard that is not loopback is dropped with its run"
    );
}

/// `IdsThatCouldBreakOutOfAUrlOrAnArgumentAreRejected` (L330).
#[test]
fn ids_that_could_break_out_of_a_url_or_an_argument_are_rejected() {
    let long_bad = "a".repeat(201);
    for bad in [
        "-rf",
        "../etc",
        "exec/1",
        "exec 1",
        "exec?x=1",
        "exec;rm",
        "",
        &long_bad,
        "exec%2F",
        "pending.1",
        ":evolve",
        "evolve:a/b",
        "exec#1",
        "exec&run=x",
        "exec_1\n",
    ] {
        assert!(!ox::valid_id(bad), "rejected: {bad:?}");
    }
    let long_good = "a".repeat(200);
    for good in ["exec_abc123", "job_6bb930bd8b98", "orch-1", "A", "evolve:lin_seed_x:generation:3", &long_good] {
        assert!(ox::valid_id(good), "accepted: {good}");
    }
    assert!(ox::parse("Job ID: ../x\nExecution ID: -v\n", "t", "r", "").is_none(), "ids that are not strict name nothing");
    assert!(
        ox::ensure_arguments(Some("--serve-daemon")).is_none()
            && ox::ensure_arguments(Some("exec_1")).unwrap() == ["-I", "-m", "ouroboros.dashboard_web", "--run", "exec_1"]
            && ox::ensure_arguments(None).unwrap() == ["-I", "-m", "ouroboros.dashboard_web"],
        "the daemon's arguments"
    );
    // An evolve id is one argument after --run, and a percent-encoded query item.
    assert!(
        ox::ensure_arguments(Some("evolve:lin:generation:2"))
            .unwrap()
            .ends_with(&["--run".to_string(), "evolve:lin:generation:2".to_string()]),
        "an evolve id is one argument"
    );
    let url = ox::events_url(&Endpoint::new("127.0.0.1", 9), "evolve:lin:generation:2").unwrap();
    let absolute = url.absolute();
    let query = absolute.split_once('?').unwrap().1;
    assert!(
        url.target().starts_with("/events?")
            && ox::query_item(&absolute, "run").as_deref() == Some("evolve:lin:generation:2")
            && !query.contains("generation:2"),
        "and one encoded query item: {absolute}"
    );
    // An evolve step without an execution is its job; a planned generation is its own block.
    let evolve = |execution: &str| {
        format!(
            "Started background evolve_step.\n\nJob ID: job_7f3a2b1c9d0e\nLineage ID: lin_seed_mac_resources_regression\nExecution ID: {execution}\n\nUse ouroboros_job_status, ouroboros_job_wait, or ouroboros_job_result to monitor it.\n\n<!-- ouroboros-job-observer-v1 base64\neyJqb2Jfb2JzZXJ2ZXIiOnt9fQ==\n-->\n"
        )
    };
    for placeholder in ["None", "none", "NULL", "Pending"] {
        let link = ox::parse(&evolve(placeholder), "ouroboros_start_evolve_step", "r", "").unwrap();
        assert!(link.execution_id.is_none() && link.key() == "job-job_7f3a2b1c9d0e", "a placeholder is no execution: {placeholder}");
    }
    let tool = "mcp__plugin_ouroboros_ouroboros__ouroboros_start_evolve_step";
    let links = ox::extract(&[
        run("run-1", vec![completed("a", tool, &evolve("None").replace("job_7f3a2b1c9d0e", "job_1"))]),
        run("run-2", vec![completed("b", tool, &evolve("evolve:lin_seed_mac_resources_regression:generation:2"))]),
    ]);
    assert!(
        keys(&links) == ["job-job_1", "evolve:lin_seed_mac_resources_regression:generation:2"] && links[1].dashboard_url.is_none(),
        "two steps of one lineage are two blocks"
    );
    assert!(
        ox::parse("Job ID: job_1\nLive Dashboard: http://localhost:1/?run=None\n", "t", "r", "").unwrap().execution_id.is_none(),
        "a run parameter that is a placeholder names nothing"
    );
}

// ── loopback ──────────────────────────────────────────────────────────────────────────────────────────────────────────────────

/// `OnlyPlainHttpOnALoopbackHostWithAPortIsEverAnEndpoint` (L331).
#[test]
fn only_plain_http_on_a_loopback_host_with_a_port_is_ever_an_endpoint() {
    assert!(
        ox::endpoint("http://localhost:8123/?run=x") == Some(Endpoint::new("localhost", 8123))
            && ox::endpoint("http://127.0.0.1:1").is_some()
            && ox::endpoint("http://[::1]:8123/").map(|e| e.host).as_deref() == Some("::1")
            && ox::endpoint("http://LOCALHOST:80/") == Some(Endpoint::new("localhost", 80)),
        "loopback http with a port"
    );
    for bad in [
        "https://localhost:8123",
        "http://localhost",
        "http://example.com:8123",
        "http://127.0.0.2:8123",
        "http://user:pw@localhost:8123",
        "file:///tmp/x",
        "http://0.0.0.0:8123",
        "http://localhost.evil.com:80",
        "ftp://localhost:21",
    ] {
        assert!(ox::endpoint(bad).is_none(), "refused: {bad}");
    }
    let state = |json: &str| ox::endpoint_from_state(json.as_bytes());
    assert!(
        state(r#"{"host":"127.0.0.1","port":5000,"pid":1,"db_path":"/x"}"#) == Some(Endpoint::new("127.0.0.1", 5000))
            && state(r#"{"host":"0.0.0.0","port":5000}"#).map(|e| e.host).as_deref() == Some("127.0.0.1")
            && state(r#"{"port":5000}"#).map(|e| e.host).as_deref() == Some("127.0.0.1"),
        "the daemon's own record, reached on loopback"
    );
    for bad in [
        r#"{"host":"10.0.0.5","port":5000}"#,
        r#"{"host":"127.0.0.1","port":"5000"}"#,
        r#"{"host":"127.0.0.1","port":70000}"#,
        "not json",
        "[5000]",
    ] {
        assert!(state(bad).is_none(), "refused record: {bad}");
    }
    let v6 = Endpoint::new("::1", 9);
    assert!(
        ox::events_url(&v6, "exec_1").map(|u| u.absolute()).as_deref() == Some("http://[::1]:9/events?run=exec_1")
            && ox::events_url(&v6, "a/b").is_none()
            && ox::page_url(&v6, None).absolute() == "http://[::1]:9/"
            && ox::page_url(&v6, Some("exec_1")).absolute() == "http://[::1]:9/?run=exec_1"
            && ox::page_url(&v6, Some("a/b")).absolute() == "http://[::1]:9/"
            && ox::runs_url(&v6).absolute() == "http://[::1]:9/api/runs",
        "the three addresses"
    );
}

// ── payloads ──────────────────────────────────────────────────────────────────────────────────────────────────────────────────

/// `DashboardPayloadsAreReadTolerantly` (L332).
#[test]
fn dashboard_payloads_are_read_tolerantly() {
    let runs = ox::summaries(
        br#"{"runs":[
          {"execution_id":"exec_1","session_id":"orch_1","goal":"Build it","status":"running","completed_count":2,"total_count":5,
           "pending_count":1,"executing_count":2,"failed_count":0,"phase":"Execute","activity":"AC 3","extra":{"x":1}},
          {"execution_id":"exec_2","status":"exploded","completed_count":true,"total_count":-3},
          {"execution_id":"../bad","status":"completed"},
          {"status":"completed"},
          "noise"
        ]}"#,
    )
    .unwrap();
    let one = &runs["exec_1"];
    assert!(
        runs.len() == 2
            && one.status == ExecutionStatus::Running
            && one.goal.as_deref() == Some("Build it")
            && one.phase.as_deref() == Some("Execute")
            && one.activity.as_deref() == Some("AC 3")
            && (one.completed, one.total, one.pending, one.executing, one.failed) == (Some(2), Some(5), Some(1), Some(2), Some(0)),
        "a run's summary"
    );
    let two = &runs["exec_2"];
    assert!(
        two.status == ExecutionStatus::Unknown && two.completed.is_none() && two.total.is_none() && two.goal.is_none(),
        "what cannot be read is absent"
    );
    assert!(ox::summaries(br#"{"error":"x"}"#).is_none() && ox::summaries(b"<html>").is_none(), "another body is not runs");
    assert!(
        ox::status(Some("CANCELLED")) == ExecutionStatus::Cancelled
            && ox::status(None) == ExecutionStatus::Unknown
            && ox::status(Some("3")) == ExecutionStatus::Unknown
            && !ExecutionStatus::Paused.is_terminal()
            && ExecutionStatus::Failed.is_terminal(),
        "statuses"
    );
    // What a block's activity mark is told: nothing read yet is starting, a status that cannot be told is unknown.
    let words: Vec<&str> = [
        ExecutionStatus::Running,
        ExecutionStatus::Paused,
        ExecutionStatus::Completed,
        ExecutionStatus::Failed,
        ExecutionStatus::Cancelled,
    ]
    .into_iter()
    .map(|status| ox::block_status(Some(&snapshot(Source::Live, status, None, None))))
    .collect();
    assert!(
        ox::block_status(None) == "starting"
            && ox::block_status(Some(&Snapshot::new(Source::Unreachable))) == "unknown"
            && words == ["running", "waiting", "completed", "error", "stopped"],
        "the block's status words"
    );
    // SSE keeps only complete data events.
    assert!(
        ox::sse_data("data: {\"a\":1}\n\n: keep-alive\n\nevent: x\r\ndata: one\r\ndata: two\r\n\r\ndata: partial")
            == ["{\"a\":1}", "one\ntwo"]
            && ox::sse_data("").is_empty(),
        "complete SSE events only"
    );
    // A board becomes an ordered AC list, unknown columns kept as executing.
    let board = ox::board(
        br#"{"meta":{"goal":"G","phase":"P","activity":"A","completed":1,"total":4},
         "columns":{
          "completed":[{"id":"ac_1","title":"First","status":"completed","ac_index":1}],
          "pending":[{"id":"ac_3","status":"pending","ac_index":3}],
          "executing":[{"id":"ac_2","title":"Second","ac_index":2},{"id":"ac_2_1","title":"Sub","ac_index":2,"depth":1}],
          "failed":[{"title":"no id"}],
          "mystery":[{"id":"ac_4","title":"Fourth","ac_index":4}]},
         "providers":[]}"#,
    )
    .unwrap();
    let ids: Vec<&str> = board.items.iter().map(|i| i.id.as_str()).collect();
    let statuses: Vec<AcStatus> = board.items.iter().map(|i| i.status).collect();
    assert!(
        ids == ["ac_1", "ac_2", "ac_2_1", "ac_3", "ac_4"]
            && statuses == [AcStatus::Completed, AcStatus::Executing, AcStatus::Executing, AcStatus::Pending, AcStatus::Executing]
            && board.items[3].title == "ac_3"
            && board.items[2].depth == 1
            && board.goal.as_deref() == Some("G")
            && board.completed == Some(1)
            && board.total == Some(4)
            && ox::board(br#"{"meta":{}}"#).is_none(),
        "the board's AC list"
    );
}

fn board_of(statuses: &[AcStatus], total: Option<i32>) -> Board {
    Board {
        goal: None,
        phase: None,
        activity: None,
        completed: None,
        total,
        items: Arc::new(statuses.iter().enumerate().map(|(index, status)| AcItem::new(&format!("ac_{index}"), "t", *status)).collect()),
    }
}

/// `TheSummaryOwnsStatusAndCountsAndTheBoardFillsTheRest` (L333).
#[test]
fn the_summary_owns_status_and_counts_and_the_board_fills_the_rest() {
    let board = Board {
        goal: Some("board goal".into()),
        phase: Some("p".into()),
        activity: None,
        completed: None,
        total: None,
        items: Arc::new(vec![AcItem::new("1", "a", AcStatus::Completed), AcItem::new("2", "b", AcStatus::Failed)]),
    };
    let summary = RunSummary {
        activity: Some("act".into()),
        completed: Some(5),
        total: Some(9),
        pending: Some(1),
        executing: Some(3),
        failed: Some(0),
        ..RunSummary::new("exec_1", ExecutionStatus::Running)
    };
    let both = Snapshot::merge(Some(&summary), Some(&board), None, None);
    assert!(
        both.status == ExecutionStatus::Running
            && both.completed == Some(5)
            && both.total == Some(9)
            && both.failed == Some(0)
            && both.goal.as_deref() == Some("board goal")
            && both.phase.as_deref() == Some("p")
            && both.activity.as_deref() == Some("act")
            && both.items.as_ref().map(|i| i.len()) == Some(2)
            && both.source == Source::Live,
        "the summary's counts over the board's words"
    );
    let alone = Snapshot::merge(None, Some(&board), None, None);
    assert!(
        (alone.completed, alone.total, alone.failed, alone.pending) == (Some(1), Some(2), Some(1), Some(0)),
        "the board alone counts its own cards"
    );
    let kept = Arc::new(vec![AcItem::new("x", "x", AcStatus::Pending)]);
    assert!(
        Snapshot::merge(Some(&summary), None, Some(&kept), None).items.is_some_and(|items| Arc::ptr_eq(&items, &kept)),
        "without a board the last list stays"
    );
    // A run the run list dropped takes a settled status from its board.
    let status = |value: &Board| Snapshot::merge(None, Some(value), None, None).status;
    assert!(
        status(&board_of(&[AcStatus::Completed, AcStatus::Completed], None)) == ExecutionStatus::Completed
            && status(&board_of(&[AcStatus::Completed, AcStatus::Failed], None)) == ExecutionStatus::Failed,
        "a settled board tells the status"
    );
    // Work still in flight, nothing settled, or ACs the board has not got yet: not known.
    assert!(
        status(&board_of(&[AcStatus::Completed, AcStatus::Executing], None)) == ExecutionStatus::Unknown
            && status(&board_of(&[AcStatus::Completed, AcStatus::Pending], None)) == ExecutionStatus::Unknown
            && status(&board_of(&[], None)) == ExecutionStatus::Unknown
            && status(&board_of(&[AcStatus::Completed], Some(3))) == ExecutionStatus::Unknown,
        "anything else stays unknown"
    );
    assert_eq!(
        Snapshot::merge(Some(&RunSummary::new("e", ExecutionStatus::Running)), Some(&board_of(&[AcStatus::Completed], None)), None, None)
            .status,
        ExecutionStatus::Running,
        "the summary still owns the status"
    );
    // A settled board ends polling like a terminal summary does.
    let epoch = DateTime::<Utc>::UNIX_EPOCH;
    let mut poll = Poll::new(epoch, false);
    poll.record(&Snapshot::merge(None, Some(&board_of(&[AcStatus::Completed], None)), None, None), epoch);
    assert!(poll.stopped(), "a settled board ends polling");
}

// ── polling ───────────────────────────────────────────────────────────────────────────────────────────────────────────────────

/// `PollingIsFastWhileThingsChangeSlowWhenQuietAndStopsWhenTerminal` (L334).
#[test]
fn polling_is_fast_while_things_change_slow_when_quiet_and_stops_when_terminal() {
    let start = DateTime::<Utc>::from_timestamp(1_000, 0).unwrap();
    let mut poll = Poll::new(start, false);
    assert!(poll.is_due(start), "due at once");
    let mut current = snapshot(Source::Live, ExecutionStatus::Running, Some(1), Some(3));
    poll.record(&current, start);
    assert!(poll.next_at == Some(start + FAST_INTERVAL) && FAST_INTERVAL == TimeDelta::seconds(3), "fast while it changes");
    // No change for a minute: back off. A change speeds it up again.
    let quiet = start + QUIET_AFTER;
    poll.record(&current.clone(), quiet);
    assert!(poll.next_at == Some(quiet + SLOW_INTERVAL) && SLOW_INTERVAL == TimeDelta::seconds(15), "slow when quiet");
    current.completed = Some(2);
    poll.record(&current, quiet + TimeDelta::seconds(15));
    assert_eq!(poll.next_at, Some(quiet + TimeDelta::seconds(15) + FAST_INTERVAL), "a change speeds it up");
    current.status = ExecutionStatus::Completed;
    poll.record(&current, quiet + TimeDelta::seconds(20));
    assert!(poll.stopped() && !poll.is_due(quiet + TimeDelta::seconds(1_000)), "a terminal status ends it");
    // An AC list opened afterwards reads once more, then stops again.
    poll.wake(quiet + TimeDelta::seconds(30));
    assert!(poll.is_due(quiet + TimeDelta::seconds(30)), "woken");
    poll.record(&current, quiet + TimeDelta::seconds(31));
    assert!(poll.stopped(), "and stopped again");
    // Job-only starts and unanswered history are read once.
    let now = Utc::now();
    let mut job = Poll::new(now, false);
    job.record(&Snapshot::new(Source::NoExecution), now);
    let mut history = Poll::new(now, true);
    assert!(job.stopped() && !history.may_ensure_daemon(), "a job-only start is read once, and history never starts the daemon");
    history.record(&Snapshot::new(Source::Unreachable), now);
    let mut live = Poll::new(now, false);
    live.record(&Snapshot::new(Source::Unreachable), now);
    assert!(history.stopped() && !live.stopped(), "unanswered history stops; a live execution keeps being read");
    assert!(
        !Poll::is_stale("2026-09-29T00:00:00Z", at("2026-09-29T11:00:00Z"))
            && Poll::is_stale("2026-09-29T00:00:00.123Z", at("2026-09-30T00:00:00Z"))
            && Poll::is_stale("garbage", now)
            && Poll::is_stale("", now)
            && Poll::is_stale("2026-09-29", at("2026-09-29T01:00:00Z")),
        "a start older than twelve hours, or with no readable time, is history"
    );
}

/// `ReadsThatNeverTellTheStatusBackOffAndTheDaemonIsStartedSparingly` (L335).
#[test]
fn reads_that_never_tell_the_status_back_off_and_the_daemon_is_started_sparingly() {
    let start = DateTime::<Utc>::UNIX_EPOCH;
    let mut poll = Poll::new(start, false);
    let unknown = Snapshot::new(Source::Live);
    let mut now = start;
    for read in 1..=UNKNOWN_STOP_AFTER {
        poll.record(&unknown, now);
        if read < UNKNOWN_BACKOFF_AFTER {
            assert!(poll.next_at.unwrap() - now <= SLOW_INTERVAL, "the first unknown reads keep their pace");
        } else if read < UNKNOWN_STOP_AFTER {
            assert_eq!(poll.next_at, Some(now + UNKNOWN_INTERVAL), "then minutes apart");
        }
        now = poll.next_at.unwrap_or(now);
    }
    assert!(
        poll.stopped() && UNKNOWN_BACKOFF_AFTER == 10 && UNKNOWN_STOP_AFTER == 40 && UNKNOWN_INTERVAL == TimeDelta::seconds(180),
        "and then they stop"
    );
    // A read that tells the status starts the count again.
    let mut known = Poll::new(start, false);
    for _ in 1..UNKNOWN_BACKOFF_AFTER {
        known.record(&unknown, start);
    }
    known.record(&snapshot(Source::Live, ExecutionStatus::Running, None, None), start);
    assert_eq!(known.unknown_reads, 0, "a known status resets the count");
    known.record(&unknown, start);
    assert_eq!(known.next_at, Some(start + FAST_INTERVAL), "and the pace");
    // An answer that cannot be read is not a status either.
    let mut unreadable = Poll::new(start, false);
    for _ in 1..=UNKNOWN_STOP_AFTER {
        unreadable.record(&Snapshot::new(Source::Unreadable), start);
    }
    assert!(unreadable.stopped(), "an unreadable dashboard stops too");
    // The daemon is started at most every few minutes and a few times in all per execution.
    let starts = DaemonStarts::default();
    assert!(
        starts.claim("exec_1", start)
            && !starts.claim("exec_1", start + DAEMON_COOLDOWN - TimeDelta::seconds(1))
            && starts.claim("exec_2", start),
        "one start per cooldown, per execution"
    );
    let mut later = start;
    for _ in 1..DAEMON_MAXIMUM {
        later += DAEMON_COOLDOWN;
        assert!(starts.claim("exec_1", later), "a later start");
    }
    assert!(
        !starts.claim("exec_1", later + DAEMON_COOLDOWN * 10) && DAEMON_MAXIMUM == 3 && DAEMON_COOLDOWN == TimeDelta::seconds(300),
        "never more than three"
    );
    // Whether a block may ask at all: live and still read, never history or a finished run.
    let mut finished = Poll::new(start, false);
    finished.record(&snapshot(Source::Live, ExecutionStatus::Failed, None, None), start);
    assert!(
        Poll::new(start, false).may_ensure_daemon() && !Poll::new(start, true).may_ensure_daemon() && !finished.may_ensure_daemon(),
        "only a live execution asks for the daemon"
    );
}

// ── the daemon's start as data (the pure half of L337) ────────────────────────────────────────────────────────────────────────

/// `WithoutARecordOnlyALoopbackHintIsUsedAndTheDaemonIsEnsuredWithoutAShell` (L337), its environment and interpreter checks (the launch
/// itself is src-tauri's `ouroboros.rs`).
#[test]
fn the_daemon_is_started_with_only_the_environment_it_needs() {
    let home = "/home/u";
    let inherited: HashMap<String, String> = [
        ("PYTHONPATH", "/tmp/evil"),
        ("PYTHONSTARTUP", "/tmp/x.py"),
        ("LANG", "ko_KR.UTF-8"),
        ("TMPDIR", "/var/tmp/"),
        ("OUROBOROS_DASHBOARD", "1"),
        ("HOME", "/elsewhere"),
        ("PATH", "/tmp/bin:/usr/bin"),
        ("DYLD_INSERT_LIBRARIES", "/tmp/x.dylib"),
        ("SystemRoot", r"C:\Windows"),
        ("APPDATA", r"C:\Users\u\AppData\Roaming"),
        ("TEMP", r"C:\Temp"),
        ("PATHEXT", ".EXE"),
        ("ComSpec", r"C:\Windows\system32\cmd.exe"),
        ("windir", r"C:\Windows"),
        ("NUMBER_OF_PROCESSORS", "8"),
        ("PSModulePath", r"C:\x"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    let unix = ox::daemon_environment(home, &inherited, false);
    assert!(
        unix.len() == 6
            && unix["HOME"] == home
            && unix["PATH"] == "/usr/bin:/bin:/usr/sbin:/sbin"
            && unix["LANG"] == "ko_KR.UTF-8"
            && unix["TMPDIR"] == "/var/tmp/"
            && unix["PYTHONSAFEPATH"] == "1"
            && unix["PYTHONNOUSERSITE"] == "1"
            && ox::daemon_environment(home, &HashMap::new(), false)["LANG"] == "en_US.UTF-8",
        "the daemon's environment as the Mac gives it"
    );
    let windows = ox::daemon_environment(home, &inherited, true);
    let names: std::collections::BTreeSet<&str> = windows.keys().map(String::as_str).collect();
    let expected: std::collections::BTreeSet<&str> = [
        "HOME",
        "USERPROFILE",
        "PATH",
        "SystemRoot",
        "windir",
        "ComSpec",
        "PATHEXT",
        "APPDATA",
        "TEMP",
        "NUMBER_OF_PROCESSORS",
        "PYTHONSAFEPATH",
        "PYTHONNOUSERSITE",
    ]
    .into();
    assert!(
        names == expected
            && windows["USERPROFILE"] == home
            && windows["ComSpec"] == r"C:\Windows\system32\cmd.exe"
            && windows["PATHEXT"] == ".EXE"
            && windows["PATH"] == format!(r"{};C:\Windows", Path::new(r"C:\Windows").join("System32").to_string_lossy())
            && windows["SystemRoot"] == r"C:\Windows",
        "on Windows the system's folders, never the app's PATH or an agent's variables: {names:?}"
    );
    // Where the tool's own interpreter is looked for: uv first, then pipx.
    let home = Path::new(home);
    assert_eq!(
        ox::tool_pythons(home, false, None),
        [
            home.join(".local").join("share").join("uv").join("tools").join("ouroboros-ai").join("bin").join("python"),
            home.join(".local").join("pipx").join("venvs").join("ouroboros-ai").join("bin").join("python"),
        ],
        "the tool interpreters (macOS)"
    );
    assert_eq!(
        ox::tool_pythons(home, true, Some(&home.join("Roaming"))),
        [
            home.join("Roaming").join("uv").join("tools").join("ouroboros-ai").join("Scripts").join("python.exe"),
            home.join("pipx").join("venvs").join("ouroboros-ai").join("Scripts").join("python.exe"),
            home.join(".local").join("pipx").join("venvs").join("ouroboros-ai").join("Scripts").join("python.exe"),
        ],
        "the tool interpreters (Windows)"
    );
}
