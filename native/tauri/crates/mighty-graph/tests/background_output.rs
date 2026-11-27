//! BackgroundOutputVerification.cs (`Tests\Verification.cs` "background output: …"): L1245 whole (L1244 is mighty-remote's
//! tests/phone_payloads.rs, the fixture, cases and read mighty-run's tests/attachments.rs): the kind words, `backgroundWaiting` and the
//! strip's rows follow the block.

use std::collections::HashMap;

use mighty_core::graph::{MightyGraphAgent, MightyGraphRun};
use mighty_core::locale::Locale;
use mighty_core::model::{BackgroundTask, BackgroundWork};
use mighty_core::time::DateTimeOffset;
use mighty_graph::view::mobile::{OutputTails, background_strip_rows};
use mighty_graph::{node, support};

fn l() -> Locale {
    Locale::new("ko")
}

fn agent(id: &str, kind: Option<&str>, status: &str) -> MightyGraphAgent {
    MightyGraphAgent { id: Some(id.into()), kind: kind.map(str::to_string), status: Some(status.into()), ..MightyGraphAgent::default() }
}

fn run_with(agents: Vec<MightyGraphAgent>) -> MightyGraphRun {
    MightyGraphRun {
        id: Some("run-1".into()),
        input: Some("Build it".into()),
        status: Some("completed".into()),
        provider: Some("claude".into()),
        agents: Some(agents),
        ..MightyGraphRun::default()
    }
}

fn task(id: &str, kind: &str, description: &str, started: &str, tool_use_id: Option<&str>) -> BackgroundTask {
    BackgroundTask {
        id: Some(id.into()),
        kind: Some(kind.into()),
        description: Some(description.into()),
        started_at: Some(started.into()),
        // C#'s record defaults the status to running.
        status: Some("running".into()),
        tool_use_id: tool_use_id.map(str::to_string),
        summary: None,
        ended_at: None,
    }
}

/// `TheKindWordsAndTheStripFollowTheBlock` (L1245).
#[test]
fn background_output_the_kind_words_background_waiting_and_the_strips_rows_follow_the_block() {
    let l = l();
    assert_eq!(
        support::background_label_of(true, "running", &l),
        l.get("graph.background.runWaiting"),
        "a running run waits on its notice"
    );
    assert_eq!(
        support::background_label_of(false, "running", &l),
        l.get("graph.background.agentWaiting"),
        "a running sub-agent waits for its reply"
    );
    assert_eq!(
        support::background_label_of(true, "completed", &l),
        l.get_with("graph.background.runEnded", &[("status", &l.get("graph.block.taskNotice.completed"))]),
        "an ended run says how"
    );
    assert_eq!(
        support::background_label_of(false, "failed", &l),
        l.get_with("graph.background.agentEnded", &[("status", &l.get("graph.block.taskNotice.failed"))]),
        "a failed sub-agent says so"
    );
    assert_eq!(
        support::background_label_of(true, "unknown", &l),
        l.get_with("graph.background.runEnded", &[("status", &l.get("styles.state.taskStatus.unknown"))]),
        "an unknown end says unknown"
    );
    assert!(support::background_label(&agent("s", Some("steer"), "completed"), &l).is_none(), "a steer is no background block");
    assert!(support::background_label(&agent("f", None, "running"), &l).is_none(), "a foreground agent is none either");

    // A request whose turn ended waits on a background sub-agent as on a task.
    let waiting = run_with(vec![
        MightyGraphAgent { background: Some(true), ..agent("a", None, "running") },
        agent("t", Some("task"), "running"),
        agent("f", None, "running"),
    ]);
    assert_eq!(support::background_waiting(&waiting), 2, "backgroundWaiting counts background agents and tasks, not a foreground agent");

    // The strip's rows meet the latest request's blocks by tool use id.
    let run_id = "run-1";
    let shell_block = MightyGraphAgent {
        background: Some(true),
        output_file: Some("/tmp/example/x.output".into()),
        ..agent(&node::agent_node_id(run_id, "toolu_shell"), Some("task"), "running")
    };
    let agent_block = MightyGraphAgent { background: Some(true), ..agent(&node::agent_node_id(run_id, "toolu_agent"), None, "running") };
    let shell_id = shell_block.id().to_string();
    let run = MightyGraphRun {
        id: Some(run_id.into()),
        source_run_id: Some(run_id.into()),
        status: Some("completed".into()),
        agents: Some(vec![shell_block, agent_block]),
        ..MightyGraphRun::default()
    };
    let work = BackgroundWork {
        tasks: Some(vec![
            task("b1", "shell", "make all", "2026-10-27T00:00:00Z", Some("toolu_shell")),
            task("b2", "agent", "Survey", "2026-10-27T00:00:01Z", Some("toolu_agent")),
            task("b3", "shell", "npm test", "2026-10-27T00:00:02Z", None),
            task("b4", "other", "watch", "2026-10-27T00:00:03Z", None),
        ]),
        turn_ended: true,
    };
    let output: OutputTails = HashMap::from([(shell_id, vec!["compiling".to_string(), "linking".to_string()])]);
    let rows = background_strip_rows(Some(&work), Some(&run), Some(&output), &l);
    assert_eq!(rows.len(), 4, "one row per task");
    assert!(
        rows[0].kind_text.as_deref() == Some(l.get("graph.background.runWaiting").as_str()) && rows[0].tail.as_deref() == Some("linking"),
        "a run's row takes its block's words and its last line"
    );
    assert!(
        rows[1].kind_text.as_deref() == Some(l.get("graph.background.agentWaiting").as_str()) && rows[1].tail.is_none(),
        "a sub-agent's row takes its block's words, no line"
    );
    assert!(
        rows[2].kind_text.as_deref() == Some(l.get("graph.background.runWaiting").as_str()) && rows[2].tail.is_none(),
        "a shell task without a block reads as a run by its kind"
    );
    assert!(rows[3].kind_text.is_none() && rows[3].tail.is_none(), "another kind keeps the task list's words");
    assert!(rows[3].detail(DateTimeOffset::now(), &l).contains(&rows[3].task.kind_title), "and its detail says them");
    assert!(background_strip_rows(None, Some(&run), None, &l).is_empty(), "no work, no rows");
}
