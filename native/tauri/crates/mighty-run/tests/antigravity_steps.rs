//! An Antigravity request's steps for the progress panel (antigravity::steps): task.md in the conversation's artifact folder, else
//! task boundaries, else one step per tool call and sub-agent; and the checklist instruction the switch adds to what agy is sent, the
//! transcript keeping the request as typed. The activity lines are agy 1.5.13's own (/tmp/agy-probe captures).

mod common;

use std::path::Path;
use std::sync::Arc;

use mighty_core::model::{AppSnapshot, RunSession, TodoItem, TodoProgress, Workspace};
use mighty_run::antigravity::steps::{self, AntigravitySteps, CHECKLIST_INSTRUCTION};
use mighty_run::checklist;
use mighty_run::model::StartRunRequest;
use mighty_run::permissions::ProviderInput;
use mighty_run::plan_mode::{TodoProgressTracker, todos};
use mighty_run::service::{RunService, RunServiceOptions};

const CONVERSATION: &str = "adc2a13e-6bd7-4119-baec-2514162788e3";

fn texts(items: &[TodoItem]) -> Vec<String> {
    items.iter().map(|i| i.content.clone().unwrap_or_default()).collect()
}

fn statuses(items: &[TodoItem]) -> Vec<String> {
    items.iter().map(|i| i.status.clone().unwrap_or_default()).collect()
}

fn shown(progress: &TodoProgress) -> Vec<(String, String)> {
    todos::items(progress).iter().map(|i| (i.content.clone().unwrap_or_default(), i.status.clone().unwrap_or_default())).collect()
}

fn pairs(expected: &[(&str, &str)]) -> Vec<(String, String)> {
    expected.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
}

/// A write of `path` (agy's `write_file`) in `state`.
fn write_line(index: u32, state: &str, path: &Path) -> String {
    let path = serde_json::to_string(&path.to_string_lossy()).unwrap();
    format!(
        r#"{{"event":"step_update","step_update":{{"conversation_id":"{CONVERSATION}","step_index":{index},"state":"{state}","step_type":"tool","tool_name":"write_file","tool_info":{{"name":"write_file","parameters":{{"Path":{path}}}}}}}}}"#
    )
}

/// The probe's lines: the request, the agent's text, a read, a refused command, a refused edit and two writes.
const PROBE: [&str; 12] = [
    r#"{"event":"step_update","step_update":{"conversation_id":"82972e8a-dccb-4cf0-8c66-8c243d5342f0","step_index":0,"state":"DONE","step_type":"user_input"}}"#,
    r#"{"event":"step_update","step_update":{"conversation_id":"82972e8a-dccb-4cf0-8c66-8c243d5342f0","step_index":1,"state":"DONE","step_type":"agent_response","duration_seconds":7.970754}}"#,
    r#"{"event":"step_update","step_update":{"conversation_id":"82972e8a-dccb-4cf0-8c66-8c243d5342f0","step_index":2,"state":"ACTIVE","step_type":"tool","tool_name":"read_file","tool_info":{"name":"read_file","parameters":{"Path":"/tmp/agy-probe/work/calc.py"}}}}"#,
    r#"{"event":"step_update","step_update":{"conversation_id":"82972e8a-dccb-4cf0-8c66-8c243d5342f0","step_index":2,"state":"DONE","step_type":"tool","tool_name":"read_file","duration_seconds":0.547524,"tool_info":{"name":"read_file","parameters":{"Path":"/tmp/agy-probe/work/calc.py"},"output":"2 lines, 32 bytes"}}}"#,
    r#"{"event":"step_update","step_update":{"conversation_id":"82972e8a-dccb-4cf0-8c66-8c243d5342f0","step_index":3,"state":"ACTIVE","step_type":"tool","tool_name":"run_command","tool_info":{"name":"run_command","parameters":{"CommandLine":"ls -la /tmp/agy-probe/work"}}}}"#,
    r#"{"event":"step_update","step_update":{"conversation_id":"82972e8a-dccb-4cf0-8c66-8c243d5342f0","step_index":3,"state":"ERROR","step_type":"tool","tool_name":"run_command","duration_seconds":0.279056,"tool_info":{"name":"run_command","parameters":{"CommandLine":"ls -la /tmp/agy-probe/work"},"error":{"type":"TOOL_ERROR","message":"permission check failed"}}}}"#,
    r#"{"event":"step_update","step_update":{"conversation_id":"adc2a13e-6bd7-4119-baec-2514162788e3","step_index":11,"state":"ACTIVE","step_type":"tool","tool_name":"edit_file","tool_info":{"name":"edit_file","parameters":{"Path":"/tmp/agy-probe/work/calc.py"}}}}"#,
    r#"{"event":"step_update","step_update":{"conversation_id":"adc2a13e-6bd7-4119-baec-2514162788e3","step_index":11,"state":"ERROR","step_type":"tool","tool_name":"edit_file","duration_seconds":6.025485,"tool_info":{"name":"edit_file","parameters":{"Path":"/tmp/agy-probe/work/calc.py"},"error":{"type":"TOOL_ERROR","message":"permission check failed for write_file"}}}}"#,
    r#"{"event":"step_update","step_update":{"conversation_id":"adc2a13e-6bd7-4119-baec-2514162788e3","step_index":12,"state":"ACTIVE","step_type":"tool","tool_name":"write_file","tool_info":{"name":"write_file","parameters":{"Path":"/tmp/agy-probe/work/test_calc.py"}}}}"#,
    r#"{"event":"step_update","step_update":{"conversation_id":"adc2a13e-6bd7-4119-baec-2514162788e3","step_index":12,"state":"DONE","step_type":"tool","tool_name":"write_file","duration_seconds":3.891055,"tool_info":{"name":"write_file","parameters":{"Path":"/tmp/agy-probe/work/test_calc.py"}}}}"#,
    r#"{"event":"step_update","step_update":{"conversation_id":"adc2a13e-6bd7-4119-baec-2514162788e3","step_index":13,"state":"ACTIVE","step_type":"tool","tool_name":"write_file","tool_info":{"name":"write_file","parameters":{"Path":"/tmp/agy-probe/work/README.md"}}}}"#,
    r#"{"event":"result","result":{"conversation_id":"adc2a13e-6bd7-4119-baec-2514162788e3","status":"SUCCESS","response":"","num_turns":1}}"#,
];

/// "each tool call is a step in the app's words, running until it ends; a refused one ends too; the request, the text and the result
/// are no steps".
#[test]
fn the_agents_tool_calls_are_the_steps_without_a_checklist() {
    common::korean();
    let mut tracker = TodoProgressTracker::new(None);
    let mut seen: Vec<Option<TodoProgress>> = vec![];
    for line in PROBE {
        seen.push(tracker.consume(&common::json(line)));
    }
    assert!(seen[0].is_none() && seen[1].is_none() && seen[11].is_none(), "the request, the agent's text and the result change nothing");
    assert_eq!(shown(seen[2].as_ref().unwrap()), pairs(&[("파일 읽기 · calc.py", "in_progress")]), "a call starts a running step");
    assert_eq!(
        shown(seen[10].as_ref().unwrap()),
        pairs(&[
            ("파일 읽기 · calc.py", "completed"),
            ("명령 · ls -la /tmp/agy-probe/work", "completed"),
            ("파일 수정 · calc.py", "completed"),
            ("파일 쓰기 · test_calc.py", "completed"),
            ("파일 쓰기 · README.md", "in_progress"),
        ]),
        "every call in order, a refused one done (the step model has no failed step)"
    );
    let ids: Vec<Option<String>> = todos::items(seen[10].as_ref().unwrap()).iter().map(|i| i.id.clone()).collect();
    assert_eq!(ids[0].as_deref(), Some("activity:agy:2"), "a step is keyed by agy's step, so two reads of one file stay two steps");
}

/// "a sub-agent started is a step; the artifacts agy writes (implementation_plan.md) are none".
#[test]
fn sub_agents_are_steps_and_artifacts_are_not() {
    common::korean();
    let mut steps = AntigravitySteps::default();
    let plan = r#"{"event":"step_update","step_update":{"conversation_id":"adc2a13e-6bd7-4119-baec-2514162788e3","step_index":6,"state":"DONE","step_type":"tool","tool_name":"write_file","tool_info":{"name":"write_file","parameters":{"Path":"/Users/young/.gemini/antigravity-cli/brain/adc2a13e-6bd7-4119-baec-2514162788e3/implementation_plan.md"}}}}"#;
    assert!(steps.consume(&common::json(plan)).is_none(), "the plan artifact is no step");
    let started = r#"{"event":"step_update","step_update":{"step_index":4,"state":"ACTIVE","step_type":"subagent","tool_name":"invoke_subagent","subagent_info":{"subagents":[{"role":"reviewer","type_name":"general"}]}}}"#;
    let list = steps.consume(&common::json(started)).unwrap();
    assert_eq!(texts(&list), ["하위 에이전트 · reviewer (general)"]);
    assert_eq!(steps::step_text("search_web", Some(&serde_json::json!({"query": "rust 2024"}))), "웹 검색 · rust 2024");
    assert_eq!(steps::step_text("grep_search", Some(&serde_json::json!({"query": "TODO"}))), "내용 검색 · TODO");
    assert_eq!(steps::step_text("list_dir", Some(&serde_json::json!({"directory_path": "/w/src"}))), "폴더 보기 · src");
    assert_eq!(steps::step_text("generate_image", None), "도구 실행 · generate_image");
}

/// "task.md's lines: to do, in progress, done; an indented line a step of its own under its top-level line; other lines left out".
#[test]
fn task_md_reads_as_a_checklist() {
    let list = steps::parse_task_file(
        "# Task\n\nSome words.\n- [x] 기존 `calc.py` 확인\n- [/] **테스트** 작성\n  - [x] add\n  - [ ] div\n* [ ] README.md 작성\n1. [X] numbered\n- [?] odd\n-[ ] no space\n",
    );
    assert_eq!(texts(&list), ["기존 calc.py 확인", "테스트 작성", "테스트 작성 › add", "테스트 작성 › div", "README.md 작성", "numbered"]);
    assert_eq!(statuses(&list), ["completed", "in_progress", "completed", "pending", "pending", "completed"]);
    assert!(steps::parse_task_file("no checklist here\n").is_empty());
}

/// "a finished write of task.md in the conversation's artifact folder is the checklist, read again on every later write, and it
/// replaces the activity steps; a write still running, a failed one or another conversation's task.md is not read".
#[test]
fn task_md_replaces_the_activity_and_is_read_again() {
    common::korean();
    let home = common::temp("agy-task-md");
    let folder = home.join(".gemini").join("antigravity-cli").join("brain").join(CONVERSATION);
    std::fs::create_dir_all(&folder).unwrap();
    let task = folder.join("task.md");
    std::fs::write(&task, "- [/] 함수 추가\n- [ ] 테스트 작성\n").unwrap();
    let mut tracker = TodoProgressTracker::new(None);
    let read = tracker.consume(&common::json(PROBE[2])).expect("an activity step first");
    assert_eq!(todos::items(&read).len(), 1);
    assert!(tracker.consume(&common::json(&write_line(5, "ACTIVE", &task))).is_none(), "a write still running is not read");
    let written = tracker.consume(&common::json(&write_line(5, "DONE", &task))).expect("the checklist");
    assert_eq!(shown(&written), pairs(&[("함수 추가", "in_progress"), ("테스트 작성", "pending")]), "task.md replaces the activity");
    // More work: the checklist stands, the activity is not shown again.
    assert!(tracker.consume(&common::json(PROBE[8])).is_none(), "a tool call no longer changes the steps");
    std::fs::write(&task, "- [x] 함수 추가\n- [/] 테스트 작성\n").unwrap();
    let edited = tracker.consume(&common::json(&write_line(9, "DONE", &task).replace("write_file", "edit_file"))).expect("read again");
    assert_eq!(shown(&edited), pairs(&[("함수 추가", "completed"), ("테스트 작성", "in_progress")]));
    // Another conversation's task.md, a task.md elsewhere and a failed write are never read.
    let other = home.join(".gemini").join("antigravity-cli").join("brain").join("11111111-2222-4333-8444-555555555555").join("task.md");
    assert!(!steps::is_task_file(&other.to_string_lossy(), Some(CONVERSATION)));
    assert!(steps::is_task_file(&task.to_string_lossy(), Some(CONVERSATION)) && steps::is_task_file(&task.to_string_lossy(), None));
    assert!(!steps::is_task_file(&home.join("task.md").to_string_lossy(), None));
    assert!(steps::is_task_file(r"C:\Users\me\.gemini\antigravity-cli\brain\adc2a13e-6bd7-4119-baec-2514162788e3\task.md", None));
    std::fs::write(&task, "- [x] 함수 추가\n- [x] 테스트 작성\n").unwrap();
    let failed = write_line(10, "DONE", &task).replace(r#""parameters""#, r#""error":{"message":"no"},"parameters""#);
    assert!(tracker.consume(&common::json(&failed)).is_none(), "a failed write is not read");
    std::fs::remove_dir_all(&home).ok();
}

/// "a task boundary (a synthetic line: agy's binary knows the step, no run has sent one yet) makes its name the current step and the
/// one before it done; it replaces the activity, and task.md replaces it".
#[test]
fn task_boundaries_are_the_steps_unless_there_is_a_task_md() {
    common::korean();
    let mut steps = AntigravitySteps::default();
    steps.consume(&common::json(PROBE[2])).unwrap();
    let first = r#"{"event":"step_update","step_update":{"step_index":3,"state":"DONE","step_type":"task_boundary","task_boundary":{"TaskName":"Reading the code","TaskStatus":"Reading calc.py","TaskSummary":"…"}}}"#;
    let list = steps.consume(&common::json(first)).unwrap();
    assert_eq!(texts(&list), ["Reading the code"], "the boundary replaces the activity");
    assert_eq!(list[0].active_form.as_deref(), Some("Reading calc.py"), "its status is what shows while it runs");
    let status = r#"{"event":"step_update","step_update":{"step_index":4,"state":"DONE","step_type":"TASK_BOUNDARY","task_name":"%SAME%","task_status":"Still reading"}}"#;
    let list = steps.consume(&common::json(status)).unwrap();
    assert_eq!((texts(&list), list[0].active_form.clone()), (vec!["Reading the code".to_string()], Some("Still reading".into())));
    let second = r#"{"event":"step_update","step_update":{"step_index":5,"state":"DONE","step_type":"tool","tool_name":"task_boundary","tool_info":{"name":"task_boundary","parameters":{"taskName":"Writing tests"}}}}"#;
    let list = steps.consume(&common::json(second)).unwrap();
    assert_eq!(texts(&list), ["Reading the code", "Writing tests"]);
    assert_eq!(statuses(&list), ["completed", "in_progress"]);
    // task.md outranks the boundaries.
    let home = common::temp("agy-boundary");
    let folder = home.join(".gemini").join("antigravity-cli").join("brain").join(CONVERSATION);
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("task.md"), "- [ ] 하나\n").unwrap();
    let list = steps.consume(&common::json(&write_line(6, "DONE", &folder.join("task.md")))).unwrap();
    assert_eq!(texts(&list), ["하나"]);
    std::fs::remove_dir_all(&home).ok();
}

/// "the instruction follows what agy is sent only when the switch is on, never a slash command".
#[test]
fn the_instruction_is_added_only_when_the_switch_is_on() {
    let mut request = StartRunRequest::new("s", "w", "claude", "fix it").with_provider("gemini");
    assert!(!request.checklist, "off unless the service turns it on");
    assert_eq!(checklist::with_instruction("fix it", &request), "fix it");
    request.checklist = true;
    assert_eq!(checklist::with_instruction("fix it", &request), format!("fix it\n\n{CHECKLIST_INSTRUCTION}"));
    let slash = StartRunRequest { checklist: true, ..StartRunRequest::new("s", "w", "claude", "/status").with_provider("gemini") };
    assert_eq!(checklist::with_instruction("/status", &slash), "/status");
    let prepared = ProviderInput::prepare(&request, "", false).unwrap();
    let line = common::json(String::from_utf8(prepared.standard_input).unwrap().trim_end());
    assert_eq!(line["message"]["content"], format!("fix it\n\n{CHECKLIST_INSTRUCTION}"));
}

/// "with the switch on, agy gets the instruction after the request and the transcript keeps the request as typed; a slash command and
/// a request with the switch off go as typed". The stand-in agy records its stdin.
#[tokio::test(flavor = "multi_thread")]
async fn the_service_sends_the_instruction_and_shows_the_request_as_typed() {
    common::korean();
    let directory = common::temp("agy-checklist");
    let folder = directory.join("workspace");
    std::fs::create_dir_all(&folder).unwrap();
    let record = directory.join("agy");
    let catalog = common::fake_catalog(&["--fake-cli", "gemini", &record.to_string_lossy()]);
    let (service, _events) = RunService::new(
        AppSnapshot::default(),
        catalog,
        RunServiceOptions {
            plugin_directory: String::new(),
            permission_card: true,
            remote: None,
            configure: None,
            admitting: None,
            binding: None,
            delegation: None,
        },
    );
    let workspace = Workspace { name: Some("w".into()), path: Some(folder.to_string_lossy().into_owned()), ..Workspace::default() };
    let pane = RunSession {
        id: Some("pane".into()),
        workspace_id: workspace.id.clone(),
        provider: Some("gemini".into()),
        ..RunSession::default()
    };
    service
        .update(|s| AppSnapshot {
            workspaces: Some(vec![workspace.clone()]),
            sessions: Some(vec![Arc::new(pane.clone())]),
            active_workspace_id: workspace.id.clone(),
            agent_checklist: Some(true),
            ..s.clone()
        })
        .unwrap();
    let workspace_id = workspace.id.clone().unwrap();
    let start = |input: &str| StartRunRequest::new("pane", &workspace_id, "claude", input).with_provider("gemini");
    let ended = |service: &RunService| {
        !service.is_session_running("pane")
            && service.snapshot().session("pane").and_then(|s| s.status.clone()).as_deref() != Some("running")
    };
    let prompt = || {
        let text = std::fs::read_to_string(format!("{}.prompt", record.display())).unwrap_or_default();
        common::json(text.trim_end())["message"]["content"].as_str().unwrap_or_default().to_string()
    };
    service.start(start("fix it")).await.unwrap();
    common::until(|| ended(&service), 30000, "the run ends").await;
    assert_eq!(prompt(), format!("fix it\n\n{CHECKLIST_INSTRUCTION}"), "agy gets the instruction");
    service.start(start("/status")).await.unwrap();
    common::until(|| ended(&service), 30000, "the slash command's run ends").await;
    assert_eq!(prompt(), "/status", "a slash command goes as typed");
    service.update(|s| AppSnapshot { agent_checklist: Some(false), ..s.clone() }).unwrap();
    service.start(start("and again")).await.unwrap();
    common::until(|| ended(&service), 30000, "the last run ends").await;
    assert_eq!(prompt(), "and again", "the switch off: as typed");
    let saved = service.snapshot().session("pane").unwrap().as_ref().clone();
    let asked: Vec<String> = saved.logs().iter().filter(|l| l.kind() == "user").filter_map(|l| l.text.clone()).collect();
    assert_eq!(asked, ["fix it", "/status", "and again"], "the transcript keeps the requests as typed");
    assert!(saved.logs().iter().all(|l| !l.text.as_deref().unwrap_or("").contains("task.md")), "nothing shows the instruction");
    service.dispose().await;
    common::remove_when_free(&directory).await;
}

/// "the runner reads an Antigravity run's steps from its stream: a refused command is a done step of the request's checklist". The
/// stand-in agy (`--agy-denied`) is refused `echo hi`.
#[tokio::test(flavor = "multi_thread")]
async fn the_runner_sends_an_antigravity_runs_steps() {
    use std::sync::Mutex;

    use mighty_run::manager::{Emit, ResolveWorkspace};
    use mighty_run::model::RunEvent;
    use mighty_run::{RunManager, RunManagerOptions};

    common::korean();
    let directory = common::temp("agy-steps-run");
    let settings = directory.join("settings.json");
    let record = directory.join("agy");
    let catalog = common::fake_catalog(&["--fake-cli", "gemini", &record.to_string_lossy(), "--agy-denied", &settings.to_string_lossy()]);
    let workspace = Workspace { path: Some(directory.to_string_lossy().into_owned()), ..Workspace::default() };
    let events: Arc<Mutex<Vec<RunEvent>>> = Arc::default();
    let sink = events.clone();
    let emit: Emit = Arc::new(move |event| sink.lock().unwrap().push(event));
    let resolved = workspace.clone();
    let resolve: ResolveWorkspace = Arc::new(move |_| {
        let workspace = resolved.clone();
        Box::pin(async move { Ok(workspace) })
    });
    let manager = RunManager::new(catalog, RunManagerOptions::new("", resolve, emit));
    let mut request = StartRunRequest::new("agy", workspace.id.as_deref().unwrap(), "claude", "run it").with_provider("gemini");
    request.graph_run_id = Some("run-1".into());
    manager.start(request).await.unwrap();
    let ended = || {
        events.lock().unwrap().iter().any(|e| e.kind == "status" && matches!(e.status.as_deref(), Some("completed" | "error" | "stopped")))
    };
    common::until(ended, 20000, "the run ends").await;
    manager.dispose().await;
    let checklists: Vec<TodoProgress> =
        events.lock().unwrap().iter().filter(|e| e.kind == "todos").filter_map(|e| e.todos.clone()).collect();
    let last = checklists.last().expect("a todos event");
    assert_eq!(shown(last), pairs(&[("명령 · echo hi", "completed")]), "the refused command is a done step");
    assert_eq!(last.run_id.as_deref(), Some("run-1"), "on the request's run");
    common::remove_when_free(&directory).await;
}
