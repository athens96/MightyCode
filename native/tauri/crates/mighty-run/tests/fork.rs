//! MC-28: a pane's conversation forked into a new pane: the CLI's own fork in the first run's arguments (Claude `--fork-session`,
//! Codex `exec fork`), which agents have one, the handoff summary the others get (its cap and its redaction), and the fork link's
//! life through the run's events.

mod common;

use mighty_core::graph::MightyGraphRun;
use mighty_core::model::{FORK_HANDOFF_LIMIT, ForkedFrom, LogEntry, PlanRecord, RunSession, RunSettings};
use mighty_run::fork::{self, ForkPoint};
use mighty_run::model::{RunEvent, StartRunRequest};
use mighty_run::providers;
use mighty_run::resume::MISSING_CONVERSATION;

const SOURCE: &str = "8cc9f04d-1f6e-487a-a773-41586e4f7f12";
const NEW: &str = "11111111-2222-4333-8444-555555555555";

fn request(provider: &str, mode: &str, fork_session: bool) -> StartRunRequest {
    let mut value = StartRunRequest::new("pane", "w", "claude", "hi")
        .with_provider(provider)
        .with_settings(RunSettings { permission_mode: mode.into(), ..RunSettings::default() });
    value.resume_id = Some(SOURCE.into());
    value.fork_session = fork_session;
    value
}

fn arguments(value: &StartRunRequest) -> Vec<String> {
    providers::arguments(value, "", None).unwrap()
}

#[test]
fn claude_forks_the_source_session_with_fork_session() {
    common::korean();
    let args = arguments(&request("claude", "manual", true));
    let resume = args.iter().position(|a| a == "--resume").expect("the source session is named");
    assert_eq!(args[resume + 1], SOURCE);
    assert!(args.iter().any(|a| a == "--fork-session"), "{args:?}");
    let plain = arguments(&request("claude", "manual", false));
    assert!(!plain.iter().any(|a| a == "--fork-session"), "an ordinary resume never forks: {plain:?}");
    let mut fresh = request("claude", "manual", true);
    fresh.resume_id = None;
    assert!(!arguments(&fresh).iter().any(|a| a == "--fork-session"), "nothing to fork without a session");
}

#[test]
fn codex_exec_forks_instead_of_resuming() {
    common::korean();
    let args = arguments(&request("codex", "manual", true));
    let exec = args.iter().position(|a| a == "exec").expect("exec");
    assert_eq!(&args[exec + 1..exec + 3], ["fork", SOURCE], "{args:?}");
    assert!(!args.iter().any(|a| a == "resume"), "{args:?}");
    let resumed = arguments(&request("codex", "manual", false));
    let exec = resumed.iter().position(|a| a == "exec").unwrap();
    assert_eq!(&resumed[exec + 1..exec + 3], ["resume", SOURCE]);
}

#[test]
fn only_claude_and_codex_exec_fork_on_their_own() {
    assert!(providers::cli_fork("claude", "plan"));
    assert!(providers::cli_fork("codex", "acceptEdits"));
    assert!(!providers::cli_fork("codex", "onRequest"), "the approval connection only resumes");
    for provider in ["gemini", "pi", "opencode", "hermes"] {
        assert!(!providers::cli_fork(provider, "manual"), "{provider} gets a handoff summary");
    }
}

fn entry(kind: &str, text: &str) -> LogEntry {
    LogEntry::new(mighty_core::wire::new_id(), kind, text.to_string(), "2026-11-23T00:00:00Z".to_string(), None)
}

fn run(id: &str, input: &str, output: &str) -> MightyGraphRun {
    MightyGraphRun {
        id: Some(id.into()),
        input: Some(input.into()),
        status: Some("completed".into()),
        final_output: Some(output.into()),
        ..MightyGraphRun::default()
    }
}

#[test]
fn the_fork_point_is_the_named_request_or_the_last() {
    let pane = RunSession {
        graph_runs: Some(vec![run("r1", "first request", "first answer"), run("r2", "second request", "second answer")]),
        plan_history: Some(vec![PlanRecord {
            id: Some("p1".into()),
            run_id: None,
            plan: Some("1. read\n2. change".into()),
            received_at: None,
            decided_at: None,
            outcome: Some("approved".into()),
            feedback: None,
            plan_truncated: None,
            graph_run_id: Some("r1".into()),
            launch_override: None,
        }]),
        ..RunSession::default()
    };
    let last = fork::fork_point(&pane, None).unwrap();
    assert_eq!(
        (last.request.as_str(), last.result.as_str(), last.is_last, last.plan.as_deref()),
        ("second request", "second answer", true, None)
    );
    let earlier = fork::fork_point(&pane, Some("r1")).unwrap();
    assert_eq!(earlier.request, "first request");
    assert_eq!(earlier.plan.as_deref(), Some("1. read\n2. change"), "the plan of that request");
    assert!(!earlier.is_last, "an earlier request cannot use the CLI's fork, which carries the whole session");
    assert!(fork::fork_point(&pane, Some("missing")).is_none());
    let logs_only = RunSession { logs: Some(vec![entry("user", "why?"), entry("assistant", "because")]), ..RunSession::default() };
    let point = fork::fork_point(&logs_only, None).unwrap();
    assert_eq!((point.request.as_str(), point.result.as_str()), ("why?", "because"));
}

fn redact(text: &str) -> String {
    text.lines().map(|l| if l.contains("sk-live-") { "[redacted: key]" } else { l }).collect::<Vec<_>>().join("\n")
}

#[test]
fn the_handoff_summary_is_capped_and_redacted() {
    let point = ForkPoint {
        request: "Fix the login bug".into(),
        plan: Some("1. find it\n2. fix it".into()),
        result: format!("Done. The key was sk-live-abcdef in config.\n{}", "long line of result text ".repeat(2_000)),
        is_last: true,
    };
    let files: Vec<String> = (0..100).map(|i| format!("src/file{i}.rs")).collect();
    let summary = fork::handoff_summary("Login fix", &point, &files, &redact);
    assert!(summary.chars().count() <= FORK_HANDOFF_LIMIT, "{}", summary.chars().count());
    assert!(!summary.contains("sk-live-"), "secrets never leave");
    assert!(summary.contains("[redacted: key]"));
    assert!(summary.contains("Fix the login bug") && summary.contains("1. find it") && summary.contains("src/file0.rs"));
    assert!(summary.contains("and 40 more"), "a long file list is cut");
    assert!(summary.contains("Login fix"));
    assert!(summary.trim_end().ends_with("## The new request"));
    let sent = fork::with_handoff(&summary, "now add a test");
    assert!(sent.starts_with("[Mighty Code]") && sent.ends_with("now add a test"));
}

fn forked(pending: bool) -> RunSession {
    RunSession {
        id: Some("pane".into()),
        kind: Some("claude".into()),
        provider: Some("claude".into()),
        status: Some("running".into()),
        resume_id: Some(SOURCE.into()),
        forked_from: Some(ForkedFrom {
            pane_id: "source".into(),
            request_id: Some("r2".into()),
            mode: Some(ForkedFrom::CLI.into()),
            fork_pending: pending.then_some(true),
            handoff: Some("summary".into()),
            ..ForkedFrom::default()
        }),
        ..RunSession::default()
    }
}

#[test]
fn the_fork_settles_when_the_run_names_its_own_session() {
    let now = mighty_core::time::DateTimeOffset::now();
    let same = RunEvent { resume_id: Some(SOURCE.into()), ..RunEvent::new("pane", "resume") };
    assert!(mighty_run::apply::apply_session(&forked(true), &same, now).forked_from.unwrap().pending(), "the source's id is no fork yet");
    let own = RunEvent { resume_id: Some(NEW.into()), ..RunEvent::new("pane", "resume") };
    let after = mighty_run::apply::apply_session(&forked(true), &own, now);
    let link = after.forked_from.unwrap();
    assert_eq!(after.resume_id.as_deref(), Some(NEW));
    assert!(!link.pending() && link.handoff.is_none(), "the fork took: no summary needed");
    assert_eq!((link.pane_id.as_str(), link.request_id.as_deref()), ("source", Some("r2")), "the link stays");
}

#[test]
fn a_fork_the_cli_cannot_find_falls_back_to_the_summary() {
    let mut pane = forked(true);
    pane.logs = Some(vec![entry("user", "go"), entry("error", &format!("{MISSING_CONVERSATION}{SOURCE}"))]);
    let after = mighty_run::apply::apply_session(&pane, &RunEvent::state("pane", "error", None), mighty_core::time::DateTimeOffset::now());
    let link = after.forked_from.unwrap();
    assert!(after.resume_id.is_none(), "the source's session is forgotten");
    assert!(!link.pending() && link.mode.as_deref() == Some(ForkedFrom::HANDOFF));
    assert_eq!(link.handoff.as_deref(), Some("summary"), "the next request carries it");
}
