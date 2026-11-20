//! The checklist switch for every agent (checklist.rs): Codex, opencode and Hermes are told to keep a plan only while the switch is on
//! (Codex as developer instructions, the ACP agents after the prompt), never for a slash command, the transcript keeping the request
//! as typed; and while a request has no plan its steps are the agent's tool calls — Codex's items, an ACP agent's tool calls (the lines
//! opencode 2 and Hermes sent in /tmp/agent-probe), pi's tool events — until a plan replaces them.

mod common;

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use mighty_core::model::{AppSnapshot, RunSession, RunSettings, TodoItem, TodoProgress, Workspace};
use mighty_run::checklist::{self, CODEX_INSTRUCTION, HERMES_INSTRUCTION, OPENCODE_INSTRUCTION};
use mighty_run::manager::{Emit, ResolveWorkspace};
use mighty_run::model::{CliCommand, RunEvent, StartRunRequest};
use mighty_run::plan_mode::{self, TodoProgressTracker, todos};
use mighty_run::service::{RunService, RunServiceOptions};
use mighty_run::{RunManager, RunManagerOptions};

fn shown(progress: &TodoProgress) -> Vec<(String, String)> {
    todos::items(progress).iter().map(|i| (i.content.clone().unwrap_or_default(), i.status.clone().unwrap_or_default())).collect()
}

fn pairs(expected: &[(&str, &str)]) -> Vec<(String, String)> {
    expected.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
}

/// Every frame through one tracker; the checklist after each.
fn feed(tracker: &mut TodoProgressTracker, lines: &[&str]) -> Vec<Option<TodoProgress>> {
    lines.iter().map(|line| tracker.consume(&common::json(line))).collect()
}

/// A `session/update`'s update as the ACP channel hands it on (`acp.update`).
fn acp(update: &str) -> String {
    format!(r#"{{"type":"acp.update","sessionId":"s","update":{update}}}"#)
}

fn request(provider: &str, input: &str, on: bool) -> StartRunRequest {
    StartRunRequest { checklist: on, ..StartRunRequest::new("s", "w", "claude", input).with_provider(provider) }
}

/// "the switch asks Gemini, Codex, opencode and Hermes, never pi; never a slash command; each agent its own words".
#[test]
fn the_instruction_goes_only_where_the_switch_asks() {
    for provider in ["gemini", "codex", "opencode", "hermes"] {
        assert!(checklist::wanted(&request(provider, "fix it", true)), "{provider} is asked");
        assert!(!checklist::wanted(&request(provider, "fix it", false)), "{provider}: the switch off");
        assert!(!checklist::wanted(&request(provider, "  /status", true)), "{provider}: a slash command");
    }
    assert!(!checklist::asks("pi") && !checklist::asks("claude") && checklist::instruction("pi").is_none());
    assert_eq!(checklist::with_instruction("fix it", &request("hermes", "fix it", true)), format!("fix it\n\n{HERMES_INSTRUCTION}"));
    assert_eq!(checklist::with_instruction("fix it", &request("opencode", "fix it", true)), format!("fix it\n\n{OPENCODE_INSTRUCTION}"));
    assert!(
        CODEX_INSTRUCTION.contains("update_plan") && OPENCODE_INSTRUCTION.contains("todowrite") && HERMES_INSTRUCTION.contains("todo tool")
    );
}

/// "Codex is told in its developer instructions (a TOML string `-c` reads), only while the switch asks".
#[test]
fn codex_gets_its_instruction_as_developer_instructions() {
    let arguments = |request: &StartRunRequest| mighty_run::providers::arguments(request, "", None).unwrap();
    let on = arguments(&request("codex", "fix it", true));
    let at = on.iter().position(|a| a.starts_with("developer_instructions=")).expect("the instruction is a config value");
    assert_eq!(on[at - 1], "-c");
    let value: serde_json::Value = serde_json::from_str(on[at].trim_start_matches("developer_instructions=")).unwrap();
    assert_eq!(value, CODEX_INSTRUCTION, "a basic string TOML reads as the words");
    assert!(at < on.iter().position(|a| a == "exec").unwrap(), "before the subcommand");
    let approvals = StartRunRequest {
        settings: Some(RunSettings { permission_mode: "onRequest".into(), ..RunSettings::default() }),
        ..request("codex", "fix it", true)
    };
    assert!(arguments(&approvals).iter().any(|a| a.starts_with("developer_instructions=")), "the approval run's app-server too");
    for off in [request("codex", "fix it", false), request("codex", "/review", true)] {
        assert!(!arguments(&off).iter().any(|a| a.contains("developer_instructions")), "off, or a slash command: none");
    }
}

/// "a Codex config that sets its own developer instructions keeps them: the words then follow the prompt".
#[test]
fn a_users_own_codex_developer_instructions_are_never_replaced() {
    let home = common::temp("codex-home");
    let workspace = common::temp("codex-workspace");
    let none = BTreeMap::new();
    assert!(!checklist::codex_sets_developer_instructions(&none, Some(&home), &workspace), "no config");
    std::fs::create_dir_all(home.join(".codex")).unwrap();
    std::fs::write(home.join(".codex").join("config.toml"), "model = \"x\"\n[profiles.p]\ndeveloper_instructions = \"theirs\"\n").unwrap();
    assert!(!checklist::codex_sets_developer_instructions(&none, Some(&home), &workspace), "a profile's own is not the top level");
    std::fs::write(home.join(".codex").join("config.toml"), "model = \"x\"\ndeveloper_instructions = \"\"\"\nmine\n\"\"\"\n").unwrap();
    assert!(checklist::codex_sets_developer_instructions(&none, Some(&home), &workspace));
    let elsewhere = common::temp("codex-elsewhere");
    let environment = BTreeMap::from([("CODEX_HOME".to_string(), elsewhere.to_string_lossy().into_owned())]);
    assert!(!checklist::codex_sets_developer_instructions(&environment, Some(&home), &workspace), "CODEX_HOME is the home");
    std::fs::create_dir_all(workspace.join(".codex")).unwrap();
    std::fs::write(workspace.join(".codex").join("config.toml"), "developer_instructions='project'\n").unwrap();
    assert!(checklist::codex_sets_developer_instructions(&environment, Some(&home), &workspace), "the project's config too");
    for folder in [home, workspace, elsewhere] {
        std::fs::remove_dir_all(folder).ok();
    }
}

/// "Codex's items are steps until its plan (update_plan's todo_list) replaces them".
#[test]
fn codex_items_are_the_steps_until_a_plan_replaces_them() {
    common::korean();
    let mut tracker = TodoProgressTracker::new(None);
    let seen = feed(
        &mut tracker,
        &[
            r#"{"type":"thread.started","thread_id":"t"}"#,
            r#"{"type":"item.completed","item":{"id":"item_0","type":"reasoning","text":"thinking"}}"#,
            r#"{"type":"item.started","item":{"id":"item_1","type":"command_execution","command":"/bin/zsh -lc 'git status'","aggregated_output":"","exit_code":null,"status":"in_progress"}}"#,
            r#"{"type":"item.completed","item":{"id":"item_1","type":"command_execution","command":"/bin/zsh -lc 'git status'","aggregated_output":"clean","exit_code":0,"status":"completed"}}"#,
            r#"{"type":"item.completed","item":{"id":"item_2","type":"file_change","changes":[{"path":"/w/src/calc.py","kind":"update"},{"path":"/w/README.md","kind":"add"}],"status":"completed"}}"#,
            r#"{"type":"item.completed","item":{"id":"item_3","type":"file_change","changes":[{"path":"/w/NOTES.md","kind":"add"}],"status":"failed"}}"#,
            r#"{"type":"item.started","item":{"id":"item_4","type":"mcp_tool_call","server":"github","tool":"search_issues","status":"in_progress"}}"#,
            r#"{"type":"item.started","item":{"id":"item_5","type":"web_search","query":"rust 2024 edition"}}"#,
        ],
    );
    assert!(seen[0].is_none() && seen[1].is_none(), "the thread and the reasoning are no steps");
    assert_eq!(shown(seen[2].as_ref().unwrap()), pairs(&[("명령 · git status", "in_progress")]));
    assert_eq!(
        shown(seen[7].as_ref().unwrap()),
        pairs(&[
            ("명령 · git status", "completed"),
            ("파일 수정 · calc.py, README.md", "completed"),
            ("파일 쓰기 · NOTES.md", "completed"),
            ("MCP 도구 · github · search_issues", "in_progress"),
            ("웹 검색 · rust 2024 edition", "in_progress"),
        ])
    );
    let plan = tracker
        .consume(&common::json(
            r#"{"type":"item.updated","item":{"id":"turn-plan","type":"todo_list","items":[{"text":"상태 확인","completed":true},{"text":"고치기","completed":false}]}}"#,
        ))
        .expect("the plan");
    assert_eq!(shown(&plan), pairs(&[("상태 확인", "completed"), ("고치기", "in_progress")]), "the plan replaces the activity");
    let more = r#"{"type":"item.started","item":{"id":"item_6","type":"command_execution","command":"cargo test","status":"in_progress"}}"#;
    assert!(tracker.consume(&common::json(more)).is_none(), "a tool call no longer changes the steps");
}

/// opencode 2's own lines (the probe and the recorded fixture): its tool calls are steps, a call's later update names its file, a
/// refused edit ends; its todowrite call is no step, and its plan replaces the activity.
#[test]
fn opencodes_tool_calls_are_the_steps_until_its_plan() {
    common::korean();
    let mut tracker = TodoProgressTracker::new(None);
    let lines = [
        acp(
            r#"{"sessionUpdate":"tool_call","toolCallId":"chatcmpl-tool-9e5f","title":"read","kind":"read","status":"pending","locations":[],"rawInput":{}}"#,
        ),
        acp(
            r#"{"sessionUpdate":"tool_call_update","toolCallId":"chatcmpl-tool-9e5f","title":"read","kind":"read","status":"in_progress","rawInput":{"path":"."},"locations":[{"path":"/tmp/agent-probe/opencode2"}]}"#,
        ),
        acp(
            r#"{"sessionUpdate":"tool_call_update","toolCallId":"chatcmpl-tool-9e5f","status":"completed","locations":[{"path":"/tmp/agent-probe/opencode2"}]}"#,
        ),
        acp(
            r#"{"sessionUpdate":"tool_call","toolCallId":"chatcmpl-tool-8ddc","title":"write","kind":"edit","status":"pending","rawInput":{},"locations":[]}"#,
        ),
        acp(
            r#"{"sessionUpdate":"tool_call_update","toolCallId":"chatcmpl-tool-8ddc","title":"write","kind":"edit","status":"in_progress","rawInput":{"path":"README.md","content":"hello"}}"#,
        ),
        acp(
            r#"{"sessionUpdate":"tool_call_update","toolCallId":"call_02ShellB","status":"in_progress","kind":"execute","title":"npm test","locations":[],"rawInput":{"command":"npm test","description":"Run the test suite","cwd":"/tmp/project"}}"#,
        ),
        acp(
            r#"{"sessionUpdate":"tool_call_update","toolCallId":"call_03EditC","status":"failed","kind":"edit","title":"src/main.ts","locations":[],"rawInput":{"path":"src/main.ts","oldString":"return 1","newString":"return 0"}}"#,
        ),
        acp(
            r#"{"sessionUpdate":"tool_call","toolCallId":"todo-1","name":"todowrite","title":"todowrite","kind":"other","status":"pending","rawInput":{"todos":[]}}"#,
        ),
        acp(r#"{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"done"}}"#),
    ];
    let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
    let seen = feed(&mut tracker, &lines);
    assert_eq!(shown(seen[0].as_ref().unwrap()), pairs(&[("파일 읽기", "in_progress")]), "announced before its input");
    assert_eq!(shown(seen[1].as_ref().unwrap()), pairs(&[("파일 읽기 · opencode2", "in_progress")]), "its update names it");
    assert!(seen[7].is_none() && seen[8].is_none(), "the todo tool and the reply are no steps");
    assert_eq!(
        shown(seen[6].as_ref().unwrap()),
        pairs(&[
            ("파일 읽기 · opencode2", "completed"),
            ("파일 쓰기 · README.md", "in_progress"),
            ("명령 · npm test", "in_progress"),
            ("파일 수정 · main.ts", "completed"),
        ]),
        "opencode's write is a write, a refused edit ends"
    );
    let plan = acp(r#"{"sessionUpdate":"plan","entries":[{"content":"README.md 작성","status":"in_progress","priority":"medium"}]}"#);
    assert_eq!(
        shown(&tracker.consume(&common::json(&plan)).unwrap()),
        pairs(&[("README.md 작성", "in_progress")]),
        "the plan replaces them"
    );
}

/// Hermes's own lines (the probe): a search, a command named only by its title, its todo_list tool (no step) and the plan it then
/// sent; a bare status for a call never announced is no step.
#[test]
fn hermes_tool_calls_are_the_steps_until_its_plan() {
    common::korean();
    let mut tracker = TodoProgressTracker::new(None);
    let lines = [
        acp(
            r#"{"sessionUpdate":"tool_call","toolCallId":"call_05d0","title":"search_files: *","kind":"search","locations":[{"path":"/tmp/agent-probe/hermes2"}]}"#,
        ),
        acp(r#"{"sessionUpdate":"tool_call_update","toolCallId":"call_05d0","kind":"search","status":"completed"}"#),
        acp(r#"{"status":"failed","toolCallId":"edit-approval-1","sessionUpdate":"tool_call_update"}"#),
        acp(
            r#"{"sessionUpdate":"tool_call","toolCallId":"call_b344","title":"terminal: pwd + 1 command","kind":"execute","locations":[]}"#,
        ),
        acp(
            r#"{"kind":"read","locations":[{"path":"/etc/shells"}],"title":"read_file: shells","toolCallId":"call_fa7e","sessionUpdate":"tool_call"}"#,
        ),
        acp(
            r#"{"sessionUpdate":"tool_call","toolCallId":"call_f897","title":"todo_list: planning 4 task(s)","kind":"other","rawInput":{"todos":[{"id":"1","content":"README.md 작성","status":"in_progress"}]},"locations":[]}"#,
        ),
    ];
    let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
    let seen = feed(&mut tracker, &lines);
    assert!(seen[2].is_none() && seen[5].is_none(), "a bare status of an unknown call and the todo tool are no steps");
    assert_eq!(
        shown(seen[4].as_ref().unwrap()),
        pairs(&[("내용 검색 · *", "completed"), ("명령 · pwd + 1 command", "in_progress"), ("파일 읽기 · shells", "in_progress")])
    );
    let plan = acp(
        r#"{"entries":[{"content":"README.md 작성","priority":"medium","status":"in_progress"},{"content":"LICENSE 작성","priority":"medium","status":"pending"}],"sessionUpdate":"plan"}"#,
    );
    assert_eq!(
        shown(&tracker.consume(&common::json(&plan)).unwrap()),
        pairs(&[("README.md 작성", "in_progress"), ("LICENSE 작성", "pending")]),
        "Hermes's plan replaces the activity"
    );
}

/// "pi keeps no plan: its tool events are the steps; its question tool is none".
#[test]
fn pis_tool_events_are_the_steps() {
    common::korean();
    let mut tracker = TodoProgressTracker::new(None);
    let seen = feed(
        &mut tracker,
        &[
            r#"{"type":"agent_start"}"#,
            r#"{"type":"tool_execution_start","toolCallId":"call-1","toolName":"read","args":{"path":"/w/notes.txt"}}"#,
            r#"{"type":"tool_execution_end","toolCallId":"call-1","toolName":"read","result":{"content":[{"type":"text","text":"x"}]},"isError":false}"#,
            r#"{"type":"tool_execution_start","toolCallId":"call-2","toolName":"bash","args":{"command":"ls -la"}}"#,
            r#"{"type":"tool_execution_end","toolCallId":"call-2","toolName":"bash","isError":true}"#,
            r#"{"type":"tool_execution_start","toolCallId":"call-3","toolName":"ask_user","args":{"question":"Which?","options":["a","b"]}}"#,
            r#"{"type":"tool_execution_start","toolCallId":"call-4","toolName":"write","args":{"path":"/w/out.md","content":"y"}}"#,
        ],
    );
    assert!(seen[0].is_none() && seen[5].is_none(), "the agent's start and its question are no steps");
    assert_eq!(
        shown(seen[6].as_ref().unwrap()),
        pairs(&[("파일 읽기 · notes.txt", "completed"), ("명령 · ls -la", "completed"), ("파일 쓰기 · out.md", "in_progress")])
    );
}

/// "a request's tool calls are not the pane's checklist: a Codex pane does not keep them, and a resumed run never continues them".
#[test]
fn activity_steps_are_never_a_saved_checklist() {
    let activity = TodoProgress {
        items: Some(vec![plan_mode::todo_item("명령 · ls", "completed", None, Some("activity:codex:item_1"))]),
        run_id: Some("r".into()),
    };
    let pane = RunSession { id: Some("s".into()), kind: Some("claude".into()), provider: Some("codex".into()), ..RunSession::default() };
    let mut event = RunEvent::new("s", "todos");
    event.todos = Some(activity.clone());
    assert!(plan_mode::apply(pane.clone(), &event).todo_progress.is_none(), "the pane keeps no tool calls");
    let plan = TodoProgress { items: Some(vec![plan_mode::todo_item("고치기", "pending", None, None)]), run_id: Some("r".into()) };
    event.todos = Some(plan);
    assert!(plan_mode::apply(pane, &event).todo_progress.is_some(), "a plan is still kept");
    let mut resumed = TodoProgressTracker::new(Some(&activity));
    let call = r#"{"type":"item.started","item":{"id":"item_1","type":"command_execution","command":"ls","status":"in_progress"}}"#;
    let items: Vec<TodoItem> = todos::items(&resumed.consume(&common::json(call)).unwrap()).to_vec();
    assert_eq!(items.len(), 1, "the earlier request's calls are not carried: {items:?}");
    assert_eq!(items[0].status.as_deref(), Some("in_progress"));
}

fn workspace(directory: &Path) -> Workspace {
    Workspace { name: Some("w".into()), path: Some(directory.to_string_lossy().into_owned()), ..Workspace::default() }
}

/// "an opencode run's prompt carries the words only when the switch asks" (the stand-in agent records its stdin).
#[tokio::test(flavor = "multi_thread")]
async fn an_acp_prompt_carries_the_instruction_only_when_asked() {
    common::korean();
    for on in [true, false] {
        let directory = common::temp(if on { "acp-checklist-on" } else { "acp-checklist-off" });
        let workspace = workspace(&directory);
        let fixture = common::fixtures().join("opencode-acp-answer.jsonl").to_string_lossy().into_owned();
        let command = CliCommand::new(
            common::fake_text(),
            vec!["--fake-acp".into(), directory.to_string_lossy().into_owned(), fixture],
            Some("opencode v2.2.7"),
        );
        let catalog = common::catalog(move |asked| (asked == "opencode").then(|| command.clone()));
        let events: Arc<Mutex<Vec<RunEvent>>> = Arc::default();
        let sink = events.clone();
        let emit: Emit = Arc::new(move |event| sink.lock().unwrap().push(event));
        let resolved = workspace.clone();
        let resolve: ResolveWorkspace = Arc::new(move |_| {
            let workspace = resolved.clone();
            Box::pin(async move { Ok(workspace) })
        });
        let manager = RunManager::new(catalog, RunManagerOptions::new("", resolve, emit));
        let mut start = StartRunRequest::new("pane", workspace.id.as_deref().unwrap(), "claude", "hi there").with_provider("opencode");
        start.checklist = on;
        manager.start(start).await.unwrap();
        let ended = || {
            events
                .lock()
                .unwrap()
                .iter()
                .any(|e| e.kind == "status" && matches!(e.status.as_deref(), Some("completed" | "error" | "stopped")))
        };
        common::until(ended, 30_000, "the run ends").await;
        manager.dispose().await;
        let stdin = std::fs::read_to_string(directory.join("stdin.txt")).unwrap_or_default();
        let prompt = stdin.lines().find(|l| l.contains("\"session/prompt\"")).map(common::json).expect("the prompt");
        let text = prompt["params"]["prompt"][0]["text"].as_str().unwrap_or_default().to_string();
        let expected = if on { format!("hi there\n\n{OPENCODE_INSTRUCTION}") } else { "hi there".to_string() };
        assert_eq!(text, expected, "the prompt, the switch {}", if on { "on" } else { "off" });
        common::remove_when_free(&directory).await;
    }
}

/// "with the switch on, Codex gets the words as developer instructions and its prompt and the transcript are the request as typed".
#[tokio::test(flavor = "multi_thread")]
async fn a_codex_panes_transcript_keeps_the_request_as_typed() {
    common::korean();
    let directory = common::temp("codex-checklist");
    let folder = directory.join("workspace");
    std::fs::create_dir_all(&folder).unwrap();
    let record = directory.join("codex");
    let catalog = common::fake_catalog(&["--fake-cli", "codex", &record.to_string_lossy()]);
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
    let workspace = workspace(&folder);
    let pane =
        RunSession { id: Some("pane".into()), workspace_id: workspace.id.clone(), provider: Some("codex".into()), ..RunSession::default() };
    service
        .update(|s| AppSnapshot {
            workspaces: Some(vec![workspace.clone()]),
            sessions: Some(vec![Arc::new(pane.clone())]),
            active_workspace_id: workspace.id.clone(),
            ..s.clone()
        })
        .unwrap();
    assert!(service.snapshot().agent_checklist_on(), "on by default");
    let start = StartRunRequest::new("pane", workspace.id.as_deref().unwrap(), "claude", "fix it").with_provider("codex");
    service.start(start).await.unwrap();
    let ended = || {
        !service.is_session_running("pane")
            && service.snapshot().session("pane").and_then(|s| s.status.clone()).as_deref() != Some("running")
    };
    common::until(ended, 30_000, "the run ends").await;
    let prompt = std::fs::read_to_string(format!("{}.prompt", record.display())).unwrap_or_default();
    let args = common::recorded_args(&record);
    let saved = service.snapshot().session("pane").unwrap().as_ref().clone();
    service.dispose().await;
    // The words go after the prompt only when the user's own Codex config sets developer instructions.
    if args.iter().any(|a| a.starts_with("developer_instructions=")) {
        assert_eq!(prompt, "fix it", "the prompt is the request as typed");
    } else {
        assert_eq!(prompt, format!("fix it\n\n{CODEX_INSTRUCTION}"), "the user's own instructions stand; the words follow the prompt");
    }
    let asked: Vec<String> = saved.logs().iter().filter(|l| l.kind() == "user").filter_map(|l| l.text.clone()).collect();
    assert_eq!(asked, ["fix it"], "the transcript keeps the request as typed");
    common::remove_when_free(&directory).await;
}

/// "every note the app puts after a request starts with one of the shared marks, and only trailing notes are taken off".
#[test]
fn the_apps_notes_come_off_a_stored_request_and_nothing_else() {
    use mighty_run::antigravity::steps::CHECKLIST_INSTRUCTION;
    use mighty_run::checklist::{APPENDED_NOTE_STARTS, without_app_notes};
    let notes =
        [CHECKLIST_INSTRUCTION, CODEX_INSTRUCTION, OPENCODE_INSTRUCTION, HERMES_INSTRUCTION, mighty_run::acp::agents::HERMES_PROMPT_NOTE];
    for note in notes {
        assert!(APPENDED_NOTE_STARTS.iter().any(|start| note.starts_with(start)), "a shared mark starts: {note}");
        assert_eq!(without_app_notes(&format!("fix it\n\n{note}\n")), "fix it");
        assert_eq!(without_app_notes(&format!("say \"{note}\"")), format!("say \"{note}\""), "mid-text stays");
    }
    let attached = format!(
        "fix it\n\nAttached file \"a.txt\": \"/tmp/a.txt\" (read this copy as needed).\n\n{}\n\n{HERMES_INSTRUCTION}",
        mighty_run::acp::agents::HERMES_PROMPT_NOTE
    );
    assert_eq!(
        without_app_notes(&attached),
        "fix it\n\nAttached file \"a.txt\": \"/tmp/a.txt\" (read this copy as needed).",
        "every trailing note"
    );
    let after = format!("fix it\n\n{CODEX_INSTRUCTION}\n\nP.S. thanks");
    assert_eq!(without_app_notes(&after), after, "a paragraph the user wrote after them keeps them");
    assert_eq!(without_app_notes("  plain  "), "  plain  ", "a request without notes is as it was");
    assert_eq!(without_app_notes(CODEX_INSTRUCTION), "");
}
