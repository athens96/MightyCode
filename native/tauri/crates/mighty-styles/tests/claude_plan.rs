//! C# `ClaudePlanStyleVerification.cs`, ported by name (L731–L735): the bundled "claude-plan" style and the v6 vocabulary, against the
//! same manifest and golden the macOS StylesClaudePlanTests read.

mod common;

use common::*;
use mighty_core::graph::MightyGraphRun;
use mighty_core::locale::Locale;
use mighty_core::model::{BackgroundTask, BackgroundWork, PlanRecord, RunSession};
use mighty_run::model::StartRunRequest;
use mighty_run::plan_mode::outcome;
use mighty_styles::evaluator::{StyleStateWidget, StyleTaskItem};
use mighty_styles::state::{self, PLAN_STAGES};
use mighty_styles::{decoder, permissions, presentation};
use serde_json::Value;

fn manifest_text(id: &str) -> String {
    String::from_utf8(bundled(id).bytes.to_vec()).expect("utf-8").replace("\r\n", "\n")
}

/// The bundled file's text decoded as the bundled source it is.
fn replaced(text: &str, old: &str, new: &str) -> Option<String> {
    assert!(text.contains(old), "fixture text not found: {old}");
    decoder::decode(text.replace(old, new).as_bytes(), "bundled", None).err().map(|e| e.code)
}

fn record(o: &str, run_id: &str, graph_run_id: Option<&str>, id: &str, launch_override: Option<&str>) -> PlanRecord {
    PlanRecord {
        id: Some(id.into()),
        run_id: Some(run_id.into()),
        plan: Some("# Plan".into()),
        received_at: Some("2026-10-06T01:00:00.000Z".into()),
        decided_at: Some("2026-10-06T01:01:00.000Z".into()),
        outcome: Some(o.into()),
        feedback: None,
        plan_truncated: None,
        graph_run_id: graph_run_id.map(str::to_string),
        launch_override: launch_override.map(str::to_string),
    }
}

fn plain(o: &str) -> PlanRecord {
    record(o, "plan-pane", Some("proc-1"), "req-1", None)
}

fn run(id: &str, source: Option<&str>, status: &str) -> MightyGraphRun {
    MightyGraphRun {
        id: Some(id.into()),
        input: Some("request".into()),
        status: Some(status.into()),
        source_run_id: source.map(str::to_string),
        ..MightyGraphRun::default()
    }
}

fn pane(runs: Option<Vec<MightyGraphRun>>, plans: Option<Vec<PlanRecord>>) -> RunSession {
    RunSession {
        id: Some("plan-pane".into()),
        workspace_id: Some("ws".into()),
        provider: Some("claude".into()),
        agent_view_mode: Some("mighty".into()),
        mighty_style: Some("claude-plan".into()),
        graph_runs: runs,
        plan_history: plans,
        logs: Some(vec![]),
        status: Some("idle".into()),
        ..RunSession::default()
    }
}

fn golden() -> Value {
    serde_json::from_slice(&fixture("golden/claude-plan.panel.json")).expect("golden")
}

/// L731 "claude plan style: manifest and v6 validation".
#[test]
fn claude_plan_style_manifest_and_v6_validation() {
    let style = bundled("claude-plan");
    let evaluator = style.evaluator();
    let m = &style.manifest;
    assert!(style.source == "bundled" && style.runnable() && m.name() == "클러드 플랜");
    let mut phases: Vec<_> = m.phases.iter().collect();
    phases.sort_by_key(|p| p.order);
    assert_eq!(phases.iter().map(|p| p.id.as_str()).collect::<Vec<_>>().join(","), "plan,approve,execute");
    assert!(evaluator.reads_plan_state() && evaluator.launch_permission_mode() == Some("plan") && evaluator.draws_tasks());
    assert!(
        mighty_styles::evaluator::recognised_name(m, "[계획] 결제").as_deref() == Some("계획]")
            && evaluator.recognised_action("[검증] 다시").map(|a| a.id.as_str()) == Some("verify")
    );
    let text = manifest_text("claude-plan");
    let code = |old: &str, new: &str| replaced(&text, old, new);
    assert_eq!(
        code(
            "\"map\": { \"planning\": \"plan\", \"awaitingApproval\": \"approve\", \"executing\": \"execute\" }",
            "\"map\": { \"planning\": \"plan\", \"awaitingApproval\": \"approve\" }"
        )
        .as_deref(),
        Some("E_RULE_INCOMPLETE")
    );
    assert_eq!(
        code("\"executing\": \"execute\" }", "\"executing\": \"execute\", \"idle\": \"plan\" }").as_deref(),
        Some("E_UNKNOWN_REFERENCE")
    );
    assert_eq!(code("\"executing\": \"execute\" }", "\"executing\": \"run\" }").as_deref(), Some("E_UNKNOWN_REFERENCE"));
    assert_eq!(code("\"kind\": \"planState\"", "\"kind\": \"planStage\"").as_deref(), Some("E_UNKNOWN_RULE"));
    assert_eq!(
        code("\"source\": \"todos\",      \"widget\": \"progressBar\"", "\"source\": \"transcript\", \"widget\": \"progressBar\"")
            .as_deref(),
        Some("E_STATE_RUN_STATE")
    );
    assert_eq!(
        code("\"source\": \"todos\",      \"widget\": \"label\"", "\"source\": \"todos\", \"widget\": \"taskList\"").as_deref(),
        Some("E_STATE_WIDGET")
    );
    assert_eq!(
        code("\"source\": \"background\", \"widget\": \"label\"", "\"source\": \"background\", \"widget\": \"list\"").as_deref(),
        Some("E_STATE_WIDGET")
    );
    assert_eq!(
        code("\"runState\": [", "\"files\": [{\"path\": \"a.md\", \"parser\": \"json\", \"widget\": \"taskList\"}], \"runState\": [")
            .as_deref(),
        Some("E_STATE_WIDGET")
    );
    assert_eq!(code("\"permissionMode\": \"plan\"", "\"permissionMode\": \"fullAccess\"").as_deref(), Some("E_UNKNOWN_RULE"));
    assert_eq!(
        code("\"launch\": { \"permissionMode\": \"plan\" }", "\"launch\": { \"permissionMode\": \"plan\", \"model\": \"opus\" }")
            .as_deref(),
        Some("E_UNKNOWN_FIELD")
    );
    assert_eq!(
        code("\"running\": \"승인한 계획대로 실행 중입니다\"", "\"running\": \"{text} 실행 중\"").as_deref(),
        Some("E_PROMPT_PLACEHOLDER")
    );
    assert_eq!(code("\"running\": \"승인한 계획대로 실행 중입니다\"", "\"busy\": \"실행 중\"").as_deref(), Some("E_UNKNOWN_FIELD"));
    assert_eq!(
        code(
            "\"approve\": {\n        \"running\": \"계획을 검토하고 승인하세요 · 바꿀",
            "\"review\": {\n        \"running\": \"계획을 검토하고 승인하세요 · 바꿀"
        )
        .as_deref(),
        Some("E_UNKNOWN_REFERENCE")
    );
    let sections = presentation::approval(&korean(), &style);
    assert!(sections.iter().any(|s| s.id == "launch" && s.lines[0].contains("plan")), "the approval card shows how requests start");
    let state_lines = &sections.iter().find(|s| s.id == "state").expect("state").lines;
    assert!(
        state_lines.iter().any(|l| l == "실행 상태 background · 위젯 taskList")
            && state_lines.iter().any(|l| l == "단계 approve ← 계획 상태 awaitingApproval"),
        "the approval card lists the run state and the stages: {state_lines:?}"
    );
}

/// L732 "claude plan style: stage rules, chips, lines and the plan-mode launch".
#[test]
fn claude_plan_style_stage_rules_chips_lines_and_the_plan_mode_launch() {
    assert_eq!(state::plan_stage(&pane(None, None), false), "planning", "nothing asked is planning");
    assert_eq!(
        state::plan_stage(&pane(Some(vec![run("g1", Some("proc-1"), "running")]), Some(vec![plain(outcome::REVISED)])), true),
        "awaitingApproval"
    );
    for o in [outcome::APPROVED_AUTO, outcome::APPROVED_CONFIRM] {
        assert_eq!(
            state::plan_stage(&pane(Some(vec![run("g1", Some("proc-1"), "completed")]), Some(vec![plain(o)])), false),
            "executing",
            "{o}"
        );
    }
    for o in [outcome::REVISED, outcome::CANCELLED] {
        let plans = vec![
            record(outcome::APPROVED_AUTO, "plan-pane", Some("proc-1"), "old", None),
            record(o, "plan-pane", Some("proc-1"), "new", None),
        ];
        assert_eq!(state::plan_stage(&pane(Some(vec![run("g1", Some("proc-1"), "completed")]), Some(plans)), false), "planning", "{o}");
    }
    let two = vec![run("g1", Some("proc-1"), "completed"), run("g2", Some("proc-2"), "running")];
    assert_eq!(
        state::plan_stage(&pane(Some(two), Some(vec![plain(outcome::APPROVED_AUTO)])), false),
        "planning",
        "the next new request plans again"
    );
    let one = || Some(vec![run("g1", Some("proc-1"), "completed")]);
    assert_eq!(
        state::plan_stage(&pane(one(), Some(vec![record(outcome::APPROVED_AUTO, "plan-pane", Some("g1"), "req-1", None)])), false),
        "executing"
    );
    assert_eq!(
        state::plan_stage(&pane(one(), Some(vec![record(outcome::APPROVED_AUTO, "proc-1", None, "req-1", None)])), false),
        "executing"
    );
    let mut started = pane(one(), Some(vec![plain(outcome::APPROVED_AUTO)]));
    started.status = Some("running".into());
    assert_eq!(state::plan_stage(&started, false), "planning", "a run whose block has not appeared plans");
    started.background_work = Some(BackgroundWork {
        tasks: Some(vec![mighty_run::plan_mode::background_task("b", "agent", "x", "2026-10-06T01:00:00.000Z")]),
        turn_ended: true,
    });
    assert_eq!(
        state::plan_stage(&started, false),
        "executing",
        "a turn that only waits on background work is still the approved request's"
    );
    let mut legacy = pane(None, Some(vec![plain(outcome::APPROVED_AUTO)]));
    legacy.logs = Some(vec![mighty_core::model::LogEntry::new("entry".into(), "user", "x".into(), mighty_core::time::now_o(), None)]);
    assert_eq!(state::plan_stage(&legacy, false), "planning", "the log's blocks own no record");
    legacy.logs = Some(vec![]);
    assert!(
        state::plan_stage(&legacy, false) == "executing"
            && state::plan_stage(&pane(Some(vec![]), Some(vec![plain(outcome::APPROVED_AUTO)])), false) == "executing"
    );
    assert_eq!(
        state::plan_stage(&pane(one(), Some(vec![record(outcome::APPROVED_AUTO, "proc-9", Some("proc-9"), "req-1", None)])), false),
        "planning"
    );

    let style = bundled("claude-plan");
    let evaluator = style.evaluator();
    let m = &style.manifest;
    assert!(
        evaluator.current_phase(&["[검증] x"], None, None).map(|p| p.id.as_str()) == Some("plan")
            && evaluator.current_phase::<&str>(&[], None, Some("awaitingApproval")).map(|p| p.id.as_str()) == Some("approve")
            && evaluator.current_phase(&["[계획] x"], None, Some("executing")).map(|p| p.id.as_str()) == Some("execute"),
        "the stage, not the history, moves the phase"
    );
    let phase = |id: &str| m.phase(id);
    let ids = |actions: Vec<&mighty_styles::StyleAction>| actions.iter().map(|a| a.id.as_str()).collect::<Vec<_>>().join(",");
    assert_eq!(ids(evaluator.visible_actions(phase("plan"), None, false, false, false)), "new-plan");
    assert!(evaluator.visible_actions(phase("approve"), None, true, false, false).is_empty(), "approval shows progress, not chips");
    assert_eq!(ids(evaluator.visible_actions(phase("execute"), None, false, false, false)), "new-plan,verify");
    assert!(
        evaluator.placeholder(phase("plan"), false, false, false) == "무엇을 계획할까요?"
            && evaluator.placeholder(phase("approve"), true, false, false).starts_with("계획을 검토하고 승인하세요")
    );
    assert_eq!(evaluator.placeholder(phase("approve"), true, true, false), "직접 답하려면 여기에 적고 Enter…", "a question still wins");
    assert!(
        evaluator.guidance(phase("execute"), false, false).as_deref() == Some("실행을 마쳤습니다. 새 계획을 세우거나 검증을 요청하세요.")
            && evaluator.guidance(phase("execute"), true, false).as_deref() == Some("승인한 계획대로 실행 중입니다")
    );
    assert_eq!(
        evaluator.guidance(phase("approve"), false, false).as_deref(),
        Some("승인 단계가 끝났습니다. 새 계획을 세우거나 검증을 요청하세요.")
    );
    assert!(evaluator.rewrite_action("결제", phase("plan"), false, false, false, false).is_none(), "Enter is advisory");

    let root = Temp::new();
    let request = StartRunRequest::new("plan-pane", "ws", "claude", "결제");
    let bound = permissions::bind(request.clone(), &pane(None, None), &root.join("profile"), &root.join("workspace"), access(), korean())
        .expect("bind");
    assert!(
        bound.permission_mode_override.as_deref() == Some("plan")
            && bound.settings.as_ref().map(|s| s.permission_mode.as_str()) != Some("plan"),
        "every new request starts in plan mode, the pane's mode untouched"
    );
    let args = mighty_run::providers::arguments(&bound, "/plugin", None).expect("args");
    assert_eq!(
        args.iter().skip_while(|a| *a != "--permission-mode").nth(1).map(String::as_str),
        Some("plan"),
        "plan reaches Claude's argv"
    );
    let mut other_pane = pane(None, None);
    other_pane.mighty_style = Some("ouroboros".into());
    let other = permissions::bind(request, &other_pane, &root.join("profile"), &root.join("workspace"), access(), korean()).expect("bind");
    assert!(other.permission_mode_override.is_none(), "another style leaves the mode alone");
}

/// L733 "claude plan style: run-state widgets match the shared golden".
#[test]
fn claude_plan_style_run_state_widgets_match_the_shared_golden() {
    let style = bundled("claude-plan");
    let (todos, work) = run_state_fixture();
    let golden = golden();
    let widgets = state::run_state_widgets(&style.manifest, "executing", Some(&todos), Some(&work), &korean());
    let produced: Vec<Value> = widgets.iter().map(presentation::payload).collect();
    assert_eq!(Value::Array(produced), golden["withState"]["widgets"], "run-state widgets match the shared golden");
    for stage in PLAN_STAGES {
        let panel = &golden[format!("planStage{}{}", stage[..1].to_uppercase(), &stage[1..])];
        let phase = style.evaluator().current_phase::<&str>(&[], None, Some(stage));
        assert_eq!(phase.map(|p| p.id.as_str()), panel["phase"]["id"].as_str());
        let running = stage == "awaitingApproval";
        assert_eq!(style.evaluator().guidance(phase, running, false).as_deref(), panel["guidance"].as_str());
        let next: Vec<String> =
            style.evaluator().visible_actions(phase, None, running, false, false).iter().map(|a| a.id.clone()).collect();
        assert_eq!(serde_json::json!(next), panel["next"]);
    }
    let planning = state::run_state_widgets(&style.manifest, "planning", Some(&todos), Some(&work), &korean());
    assert!(
        planning[0].value == 0 && planning[0].total == Some(0) && planning[1].text.as_deref() == Some(""),
        "a new plan does not show the last one's progress"
    );
    let mut live_pane = pane(Some(vec![run("g1", Some("proc-1"), "completed")]), Some(vec![plain(outcome::APPROVED_AUTO)]));
    live_pane.todo_progress = Some(todos);
    live_pane.background_work = Some(work);
    let reading = state::live(None, &style.manifest, &live_pane, false, &korean());
    assert!(
        reading.plan_stage.as_deref() == Some("executing")
            && reading.widgets.len() == 4
            && reading.widgets[3].tasks.as_ref().map(Vec::len) == Some(2)
    );
    assert!(
        state::live(None, &fixture_style("loop").manifest, &pane(None, None), false, &korean()).plan_stage.is_none(),
        "another style never reads the stage"
    );
}

/// L734 "claude plan style: background tasks and the strip outside the style".
#[test]
fn claude_plan_style_background_tasks_and_the_strip_outside_the_style() {
    use mighty_composer::presentation::plan::{background_counts, background_finished, shows_background_strip};
    let (_, work) = run_state_fixture();
    let ko = korean();
    let rows = mighty_graph::view::mobile::background_tasks(Some(&work), &ko);
    assert!(
        rows.len() == 2
            && rows[0].text == "review the diff"
            && rows[0].running
            && rows[0].kind_title == "에이전트"
            && rows[1].status_title == "완료"
    );
    let start = mighty_core::time::parse(Some("2026-10-06T01:00:00.000Z")).expect("start");
    let at = |seconds: i64| mighty_core::time::DateTimeOffset::from_utc_ticks(start.utc_ticks() + seconds * 10_000_000);
    assert!(
        rows[0].elapsed(at(45), &ko) == "45초"
            && rows[0].elapsed(at(192), &ko) == "3분 12초"
            && rows[1].elapsed(at(5 * 3600), &ko) == "1분 5초"
    );
    assert!(presentation::elapsed(&ko, start, at(3725)) == "1시간 2분" && presentation::elapsed(&ko, start, at(-5)) == "0초");
    let odd = presentation::tasks(
        &ko,
        &StyleStateWidget::tasks(
            "taskList",
            vec![StyleTaskItem {
                text: " \n".into(),
                kind: "weird".into(),
                status: "nope".into(),
                started_at: "2026-10-06T01:00:00.000Z".into(),
                ended_at: None,
            }],
        ),
    );
    assert!(odd[0].kind == "other" && odd[0].text == "작업" && odd[0].status_title == "알 수 없음", "closed fallbacks");
    let many = BackgroundWork {
        tasks: Some(
            (0..12)
                .map(|i| {
                    mighty_run::plan_mode::background_task(&format!("t{i}"), "agent", &format!("task {i}"), "2026-10-06T01:00:00.000Z")
                })
                .collect(),
        ),
        turn_ended: false,
    };
    assert_eq!(mighty_graph::view::mobile::background_tasks(Some(&many), &ko).len(), state::MAXIMUM_TASK_LIST_ITEMS, "at most eight tasks");
    let running = BackgroundWork { turn_ended: false, ..work.clone() };
    assert!(shows_background_strip(Some(&work), false, false) && !shows_background_strip(Some(&running), false, false));
    assert!(
        shows_background_strip(Some(&running), true, false)
            && !shows_background_strip(Some(&work), true, true)
            && !shows_background_strip(None, true, false)
    );
    let first = work.tasks.as_ref().expect("tasks")[0].clone();
    let ended =
        |status: &str| BackgroundTask { status: Some(status.into()), ended_at: Some("2026-10-06T01:01:00.000Z".into()), ..first.clone() };
    let told = BackgroundWork { tasks: Some(vec![ended("completed")]), ..work.clone() };
    let lost = BackgroundWork { tasks: Some(vec![ended("unknown")]), ..work.clone() };
    let told_running = BackgroundWork { turn_ended: false, ..told.clone() };
    assert!(
        shows_background_strip(Some(&told), false, false)
            && shows_background_strip(Some(&told_running), true, false)
            && !shows_background_strip(Some(&told_running), false, false)
    );
    assert!(!shows_background_strip(Some(&lost), false, false) && !shows_background_strip(Some(&lost), true, false));
    let mixed = BackgroundWork {
        tasks: Some(vec![
            first.clone(),
            BackgroundTask { id: Some("x".into()), status: Some("completed".into()), ..first.clone() },
            BackgroundTask { id: Some("y".into()), status: Some("unknown".into()), ..first.clone() },
        ]),
        ..work.clone()
    };
    assert_eq!(background_finished(&mixed), 2, "finished is every task that no longer runs");
    assert!(
        mighty_graph::view::mobile::mobile_background_done(Some(&mixed)) == Some(2)
            && mighty_graph::view::mobile::mobile_background_done(Some(&lost)).is_none()
            && mighty_graph::view::mobile::mobile_background_done(None).is_none()
    );
    assert_eq!(background_counts(&told, &ko), "끝남 1개 · 작업 중 0개");
    let payload = serde_json::to_string(&presentation::payload(&StyleStateWidget::tasks(
        "taskList",
        vec![StyleTaskItem {
            text: "x".into(),
            kind: "agent".into(),
            status: "running".into(),
            started_at: "2026-10-06T01:00:00.000Z".into(),
            ended_at: None,
        }],
    )))
    .expect("payload");
    assert!(payload.contains("\"kind\":\"taskList\"") && !payload.contains("endedAt"), "the task list payload carries only what it has");
}

/// L735 "claude plan style: queue, phone sends, stop and the stored mode": the queue's background rules in every style, the notice,
/// a queued item keeping its launch decision, and saved records keeping only the plan override (the queue and the phone's sends are
/// mighty-composer's and W8's, checked there as well).
#[test]
fn claude_plan_style_queue_phone_sends_stop_and_the_stored_mode() {
    use mighty_composer::queue::background::{queued_override, steers_run_next, stop_keeps_queue, waits_on_background};
    let running = mighty_run::plan_mode::background_task("b", "agent", "x", "2026-10-06T01:00:00.000Z");
    let waiting = BackgroundWork { tasks: Some(vec![running.clone()]), turn_ended: true };
    let busy = BackgroundWork { tasks: Some(vec![running.clone()]), turn_ended: false };
    assert!(
        waits_on_background(Some(&waiting), 1)
            && !waits_on_background(Some(&waiting), 0)
            && !waits_on_background(Some(&busy), 1)
            && !waits_on_background(None, 1)
    );
    assert_eq!(korean().get("queue.waitingOnBackground.windows"), "백그라운드 작업이 끝나야 시작합니다 · 지금 시작하려면 중지");
    assert!(stop_keeps_queue(Some(&waiting)) && !stop_keeps_queue(Some(&busy)) && !stop_keeps_queue(None));
    let completed = BackgroundWork { tasks: Some(vec![BackgroundTask { status: Some("completed".into()), ..running }]), turn_ended: true };
    assert!(
        steers_run_next(Some(&waiting), true, false, 1, false)
            && !steers_run_next(Some(&busy), true, false, 1, false)
            && !steers_run_next(None, true, false, 1, false)
            && !steers_run_next(Some(&waiting), false, false, 1, false)
            && !steers_run_next(Some(&waiting), true, true, 1, false)
            && !steers_run_next(Some(&waiting), true, false, 0, false)
            && !steers_run_next(Some(&waiting), true, false, 1, true)
            && !steers_run_next(Some(&completed), true, false, 1, false)
    );
    let l = Locale::new("ko");
    let mut queue = mighty_composer::queue::QueuedInputBuffer::default();
    let item = queue.add("결제", vec![], queued_override(true), false, false, &l).expect("queued");
    assert!(
        item.permission_mode_override.as_deref() == Some("plan")
            && queue.add("x", vec![], None, false, false, &l).expect("queued").permission_mode_override.is_none()
    );
    assert!(queue.settle("stopped", true) && queue.items().len() == 2, "a kept queue survives the stop and runs next");
    assert!(!queue.settle("stopped", false) && queue.items().is_empty(), "an ordinary stop still cancels the queue");
    let history = vec![
        record(outcome::APPROVED_AUTO, "plan-pane", Some("proc-1"), "req-1", Some("plan")),
        record(outcome::APPROVED_AUTO, "plan-pane", Some("proc-1"), "b", Some("fullAccess")),
    ];
    let normalized = mighty_run::plan_mode::normalized_history(Some(&history)).expect("history");
    assert_eq!(
        normalized.iter().map(|r| r.launch_override.as_deref()).collect::<Vec<_>>(),
        [Some("plan"), None],
        "saved records keep only the plan override"
    );
}
