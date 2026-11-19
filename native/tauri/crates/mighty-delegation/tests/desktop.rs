//! The desktop service's delegation over the real run path (app.rs through support/launch.rs), C#'s DelegationAppVerification checks
//! under their names: "the real run path attaches the server and runs a child in its worktree" (L674), "the desktop service hosts
//! delegation and offers the window its entry points" (L675), "a card's merge is queued while the child works or the parent runs, then
//! goes through by itself" (L676), and DelegationCrossAgentVerification "the service offers the worktree menu and starts another agent's
//! child" (L706). Git, the fake Claude and its MCP server run through the platform's process host and transport (Windows' or the
//! Mac's).

mod support;

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mighty_composer::queue::ComposerSecondSend;
use mighty_core::locale::Locale;
use mighty_core::model::{LogEntry, RunSession, RunSettings};
use mighty_delegation::agent_io::binding::{SERVER_NAME as TERMINAL_SERVER, TOKEN_KEY};
use mighty_delegation::app::DelegationWindow;
use mighty_delegation::app_host::{self, ChildRunEnd};
use mighty_delegation::cards::{DelegationCardAction, DelegationCardResult, DelegationMergeWait, PendingParentClose};
use mighty_delegation::git::cleanup::{ChildCleanupHold, ChildCleanupOutcome};
use mighty_delegation::git::paths;
use mighty_delegation::host::{DelegationAgentAvailability, DelegationChildPane};
use mighty_delegation::mcp::delegation::SERVER_NAME as DELEGATION_SERVER;
use mighty_delegation::model::{ChildRecord, ChildRecordOptions, ChildState, DeliveryLane, Notice, NoticeKind};
use mighty_delegation::providers::{DelegationChildBlock, DelegationChildChoice};
use mighty_delegation::request::DelegationReasonCode;
use mighty_delegation::store::{DelegationFile, DelegationFileStore};
use mighty_delegation::tools;
use mighty_delegation::tools::delegate::HumanChildStart;
use mighty_run::model::StartRunRequest;
use serde_json::Value;
use support::launch::{DelegationLaunch, PARENT, awaited, steps};
use support::{host, lane, place::DelegationPlace};

/// The window's part that hears which queued merges went through by themselves (C# `QueuedMergeDone`).
#[derive(Default)]
struct Done(Mutex<Vec<String>>);

impl DelegationWindow for Done {
    fn queued_merge_done(&self, child_id: &str) {
        self.0.lock().unwrap().push(child_id.to_string());
    }
}

fn status(s: &DelegationLaunch, status: &str) {
    s.service
        .update(|state| {
            let mut next = state.clone();
            for pane in next.sessions_mut().iter_mut().filter(|pane| pane.id() == PARENT) {
                *pane = Arc::new(mighty_core::model::RunSession { status: Some(status.into()), ..(**pane).clone() });
            }
            next
        })
        .unwrap();
}

/// L676.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cards_merge_is_queued_while_the_child_works_or_the_parent_runs_then_goes_through_by_itself() {
    Box::pin(queued_merges()).await;
}

async fn queued_merges() {
    let _lane = lane::enter().await;
    let place = DelegationPlace::new();
    let start = place.repository_default().await;
    let repo = place.repo();
    // c1 works and reports later, c2 reported while its parent runs, c3 ends without a report. Human-started, so no notice runs the
    // parent.
    let (reported, _) = place.reported_child("c2", &["c2.txt"], Some(&repo)).await;
    let (working, _) = place.reported_child("c1", &["c1.txt"], Some(&repo)).await;
    let silent = place.child("c3", Some(&repo)).await;
    let human = |record: ChildRecord| ChildRecord { started_by_human: Some(true), ..record };
    let working = human(ChildRecord {
        state: ChildState::Running,
        report_head: None,
        report_revision: 0,
        run_id: Some("r1".into()),
        ..host::record_of(&working)
    });
    let silent = human(ChildRecord { run_id: Some("r3".into()), ..host::record_of(&silent) });
    DelegationFileStore::new(place.profile())
        .save(&DelegationFile::of(vec![working, human(host::record_of(&reported)), silent], vec![], vec![], vec![], vec![]))
        .unwrap();
    let done = Arc::new(Done::default());
    let s = DelegationLaunch::start_with(
        Path::new(&place.profile()),
        &Path::new(&place.base()).join("fake"),
        &repo,
        &place.root(),
        place.git(),
        "acceptEdits",
        true,
        Duration::from_millis(100),
        done.clone(),
    )
    .await;
    // The sweep at launch stopped the running ones; their runs are back.
    s.coordinator().child_run_started("c1", "r1b").await;
    s.coordinator().child_run_started("c3", "r3b").await;
    status(&s, "running");

    // Each says what it waits on, and nothing is busy or merged.
    let result = |note: Option<mighty_delegation::cards::DelegationCardNote>| note.map(|note| note.result);
    assert_eq!(
        result(s.app.merge_child("c1").await),
        Some(DelegationCardResult::Queued(DelegationMergeWait::ChildRunning)),
        "a working child's merge is queued"
    );
    assert_eq!(
        result(s.app.merge_child("c2").await),
        Some(DelegationCardResult::Queued(DelegationMergeWait::ParentRunning)),
        "a running parent's merge is queued"
    );
    assert_eq!(
        result(s.app.merge_child("c3").await),
        Some(DelegationCardResult::Queued(DelegationMergeWait::ChildRunning)),
        "a third is queued too"
    );
    assert!(
        !s.app.is_card_busy("c1") && !s.app.is_card_busy("c2") && place.git_in(&repo, &["rev-parse", "HEAD"]).await == start,
        "a queued merge shows no busy state and merges nothing"
    );

    // The parent's run ends: the slow retry lets c2 through, and the window is told to offer the discard.
    status(&s, "idle");
    awaited("c2's merge", || (result(s.app.card_note("c2")) == Some(DelegationCardResult::Done)).then_some(())).await;
    assert!(
        s.file().children.iter().any(|record| record.id == "c2" && record.state == ChildState::Merged)
            && Path::new(&repo).join("c2.txt").exists(),
        "c2 merged by itself"
    );
    awaited("the discard offer for c2", || (*done.0.lock().unwrap() == ["c2"]).then_some(())).await;

    // c1 reports: its row changes and the merge goes through, with a merge commit over c2's.
    assert!(matches!(result(s.app.card_note("c1")), Some(DelegationCardResult::Queued(_))), "c1 is still queued");
    s.coordinator().child_run_ended("c1", "r1b", ChildRunEnd::Finished).await;
    awaited("c1's merge", || (result(s.app.card_note("c1")) == Some(DelegationCardResult::Done)).then_some(())).await;
    assert!(
        s.file().children.iter().any(|record| record.id == "c1" && record.state == ChildState::Merged)
            && Path::new(&repo).join("c1.txt").exists()
            && Path::new(&repo).join("c2.txt").exists(),
        "c1 merged by itself"
    );
    awaited("the discard offer for c1", || (done.0.lock().unwrap().len() == 2).then_some(())).await;

    // c3 ends with no report: the queued merge is dropped, and the card says so.
    s.coordinator().child_run_ended("c3", "r3b", ChildRunEnd::Finished).await;
    awaited("c3's dropped merge", || (result(s.app.card_note("c3")) == Some(DelegationCardResult::QueueDropped)).then_some(())).await;
    assert!(
        s.file().children.iter().any(|record| record.id == "c3" && record.state == ChildState::Ended)
            && place.git_in(&repo, &["log", "--oneline", "--", "c3.txt"]).await.is_empty(),
        "an ended child is not merged"
    );
    s.dispose().await;
}

/// The window's part a check sets up: agents it knows not installed, and the agent terminals it ended (C# `AgentAvailabilityNow`,
/// `EndAgentTerminal`).
#[derive(Default)]
struct Knows {
    missing: Vec<&'static str>,
    ended: Mutex<Vec<String>>,
}

impl DelegationWindow for Knows {
    fn agent_availability(&self, provider: &str) -> DelegationAgentAvailability {
        if self.missing.contains(&provider) { DelegationAgentAvailability::NotInstalled } else { DelegationAgentAvailability::Ready }
    }
    fn end_agent_terminal(&self, session_id: String) -> mighty_platform::BoxFuture<'static, ()> {
        self.ended.lock().unwrap().push(session_id);
        Box::pin(async {})
    }
}

async fn launch(place: &DelegationPlace, mode: &str, switch_on: bool, window: Arc<dyn DelegationWindow>) -> DelegationLaunch {
    DelegationLaunch::start_with(
        Path::new(&place.profile()),
        &Path::new(&place.base()).join("fake"),
        &place.repo(),
        &place.root(),
        place.git(),
        mode,
        switch_on,
        mighty_delegation::app::MERGE_RETRY,
        window,
    )
    .await
}

fn add_panes(s: &DelegationLaunch, panes: Vec<RunSession>) {
    s.service
        .update(move |state| {
            let mut next = state.clone();
            next.sessions_mut().extend(panes.iter().cloned().map(Arc::new));
            next
        })
        .unwrap();
}

fn pane(s: &DelegationLaunch, id: &str, kind: &str, provider: &str, mode: &str) -> RunSession {
    RunSession {
        id: Some(id.into()),
        workspace_id: s.workspace.id.clone(),
        kind: Some(kind.into()),
        provider: Some(provider.into()),
        settings: Some(RunSettings { permission_mode: mode.into(), ..RunSettings::default() }),
        ..RunSession::default()
    }
}

/// The servers each run's `--mcp-config` file named, sorted, as the fake read it when it started (the file goes with the run).
fn servers(log: &[Value]) -> Vec<Vec<String>> {
    log.iter()
        .filter(|entry| entry["event"] == "mcpConfig")
        .map(|entry| entry["servers"].as_array().unwrap().iter().filter_map(Value::as_str).map(str::to_string).collect())
        .collect()
}

/// DelegationAppVerification "delegation: the real run path attaches the server and runs a child in its worktree" (L674).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_real_run_path_attaches_the_server_and_runs_a_child_in_its_worktree() {
    Box::pin(real_run_path()).await;
}

async fn real_run_path() {
    let _lane = lane::enter().await;
    let place = DelegationPlace::new();
    place.repository_default().await;
    let repo = place.repo();
    let s = launch(&place, "acceptEdits", false, Arc::new(Knows::default())).await;
    s.script(&repo, vec![vec![steps::say("Hello.")], vec![steps::say("Again.")]]);
    // Off, the default: the Claude pane's run names mighty-terminal alone, and its binding is detached.
    let first = s.human_send("Say hello.", PARENT).await;
    assert_eq!(s.ended(PARENT, &first).await, "completed", "the switched-off run");
    let log = s.log(&repo);
    let argv: Vec<&Value> = log.iter().filter(|entry| entry["event"] == "argv").collect();
    assert!(
        servers(&log) == [vec![TERMINAL_SERVER.to_string()]]
            && steps::option(argv[0], "--mcp-config").is_some_and(|path| !path.starts_with('{'))
            && s.binding(PARENT).is_some_and(|binding| !binding.delegation())
            && !log.iter().any(|entry| entry["event"] == "mcp"),
        "switched off, one server"
    );
    // On, read at the next run start: the run gets the delegation server, and the fake Claude reaches its six tools.
    s.switch_on.store(true, std::sync::atomic::Ordering::SeqCst);
    let second = s.human_send("Say it again.", PARENT).await;
    assert_eq!(s.ended(PARENT, &second).await, "completed", "the switched-on run");
    let log = s.log(&repo);
    let argv: Vec<&Value> = log.iter().filter(|entry| entry["event"] == "argv").collect();
    let binding = s.binding(PARENT).expect("the parent's binding");
    let token = binding.environment().get(TOKEN_KEY).cloned().unwrap_or_default();
    let words: Vec<&str> = argv[1]["arguments"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert!(
        servers(&log).get(1).is_some_and(|names| names == &[DELEGATION_SERVER, TERMINAL_SERVER])
            && binding.delegation()
            && binding.kind() == "claude"
            && binding.provider == "claude"
            && !token.is_empty()
            && !words.join("\n").contains(&token),
        "switched on, both servers, and the token stays out of argv"
    );
    let mcp = log.iter().find(|entry| entry["event"] == "mcp").expect("the CLI reached the server");
    assert_eq!(
        mcp["tools"].as_array().unwrap().iter().filter_map(Value::as_str).collect::<Vec<_>>(),
        tools::ALL.map(|tool| tool.name),
        "the CLI reached the six tools on the pane's token"
    );
    // A Codex pane's run, and a shell pane's, never get the delegation server: a shell pane gets no binding at all.
    add_panes(&s, vec![pane(&s, "codex", "claude", "codex", "manual"), pane(&s, "shell", "shell", "claude", "manual")]);
    let mut codex = StartRunRequest::new("codex", s.workspace.id(), "claude", "hi");
    codex.provider = "codex".into();
    let _ = s.service.start(codex).await;
    let run = awaited("the codex run", || s.runs("codex").first().cloned()).await;
    s.ended("codex", &run).await;
    let arguments: Vec<String> = serde_json::from_str(&std::fs::read_to_string(s.fake_root.join("codex.args")).unwrap()).unwrap();
    assert!(
        s.binding("codex").is_some_and(|binding| !binding.delegation())
            // Named as a server (the checks' exe is mighty-delegation-fake.exe, so its path holds the word too).
            && arguments.iter().any(|argument| argument.starts_with(&format!("mcp_servers.{TERMINAL_SERVER}.")))
            && !arguments.iter().any(|argument| argument.starts_with(&format!("mcp_servers.{DELEGATION_SERVER}."))),
        "a Codex run names mighty-terminal alone"
    );
    let _ = s.service.start(StartRunRequest::new("shell", s.workspace.id(), "shell", "echo shell")).await;
    let run = awaited("the shell run", || s.runs("shell").first().cloned()).await;
    s.ended("shell", &run).await;
    assert!(!s.bound("shell"), "a shell pane gets no binding");

    // A child's run and its binding work in its worktree folder, never in its parent's checkout.
    let worktree = Path::new(&place.root()).join("child-1");
    std::fs::create_dir_all(&worktree).unwrap();
    let worktree_text = worktree.to_string_lossy().into_owned();
    let child = DelegationChildPane {
        session_id: "child-1".into(),
        parent_session_id: PARENT.into(),
        mode: "plan".into(),
        folder: worktree_text.clone(),
        started_by_human: false,
        provider: "claude".into(),
    };
    add_panes(&s, vec![app_host::child_session(&child, s.workspace.id(), "Child", None, None)]);
    s.script(&worktree_text, vec![vec![steps::say("Working in my worktree.")]]);
    s.service.start(s.request("Write the notes.", "child-1")).await.unwrap();
    let run = awaited("the child's run", || s.runs("child-1").first().cloned()).await;
    assert_eq!(s.ended("child-1", &run).await, "completed", "the child's run");
    let bound = s.binding("child-1").expect("the child's binding");
    assert!(
        bound.delegation() && paths::same(&bound.workspace_path, &worktree_text) && bound.workspace_id == s.workspace.id(),
        "the binding the terminal tool's pane is made from names the worktree"
    );
    let child_log = s.log(&worktree_text);
    assert!(
        s.prompts(&worktree_text) == ["Write the notes."] && child_log.iter().any(|entry| entry["event"] == "argv"),
        "Claude worked in the worktree"
    );
    let child_argv = child_log.iter().find(|entry| entry["event"] == "argv").unwrap();
    assert_eq!(steps::option(child_argv, "--permission-mode").as_deref(), Some("plan"), "in the child's own mode");
    // Its worktree gone, it refuses to run, with worktree_missing, and never falls back to the parent's checkout.
    std::fs::remove_dir_all(&worktree).unwrap();
    let refused = s.service.start(s.request("Again.", "child-1")).await;
    let message = refused.err().map(|error| error.message().to_string()).unwrap_or_default();
    assert!(
        message == mighty_run::strings::get("run.blocked.worktreeMissing")
            && message.contains("worktree_missing")
            && s.runs("child-1").len() == 1
            && s.pane("child-1").status() == "completed",
        "a child without its worktree refuses to run: {message}"
    );
    s.dispose().await;
}

/// DelegationAppVerification "delegation: the desktop service hosts delegation and offers the window its entry points" (L675): the
/// service at launch — the interrupted sweep and held items, a parent close that asks, a workspace that may not go, a send no run took,
/// card actions one at a time, a child's close. Its phone halves (the mobile router's count and refusal of a held row, its revisions)
/// are the phone router's (W8); its child-pane rules are tests/app_host.rs's.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_desktop_service_hosts_delegation_and_offers_the_window_its_entry_points() {
    Box::pin(hosts_delegation()).await;
}

async fn hosts_delegation() {
    let _lane = lane::enter().await;
    let place = DelegationPlace::new();
    place.repository_default().await;
    let commit = "a".repeat(40);
    let record = |id: &str, state: ChildState, options: ChildRecordOptions| {
        ChildRecord::make_with(
            id,
            PARENT,
            &Path::new(&place.root()).join(id).to_string_lossy(),
            "main",
            &commit,
            "plan",
            &format!("key-{id}"),
            ChildRecordOptions { state: Some(state), ..options },
        )
    };
    let running = ChildRecord { run_id: Some("run-old".into()), ..record("c1", ChildState::Running, ChildRecordOptions::default()) };
    let gone = Path::new(&place.base()).join("gone").to_string_lossy().into_owned();
    let reported = record(
        "c2",
        ChildState::Reported,
        ChildRecordOptions { report_revision: 1, report_head: Some(commit.clone()), parent_checkout: Some(gone), ..Default::default() },
    );
    let notice = Notice::new("n1", "c2", 1, NoticeKind::Reported, DeliveryLane::Pending);
    DelegationFileStore::new(place.profile())
        .save(&DelegationFile::of(vec![running, reported], vec![notice], vec![], vec![], vec![]))
        .unwrap();
    let window = Arc::new(Knows::default());
    let s = launch(&place, "acceptEdits", true, window.clone()).await;
    assert!(
        s.file().children.iter().any(|record| record.id == "c1" && record.state == ChildState::Interrupted)
            && s.app.held_items(PARENT).iter().map(|item| item.id.as_str()).collect::<Vec<_>>() == ["n1"],
        "launch interrupts the runs the quit killed and holds what was pending"
    );
    assert!(
        s.app.parent_close_question(PARENT) == Some(PendingParentClose { id: PARENT.into(), title: "Parent".into(), open_children: 2 })
            && s.app.parent_close_question("nobody").is_none(),
        "a parent with open children asks first"
    );
    assert_eq!(
        s.app.workspace_removal_refusal(s.workspace.id()),
        Some(DelegationReasonCode::WorkspaceHasChildren),
        "the workspace has children"
    );
    let snapshot = s.snapshot();
    let rows = s.app.rows();
    let tree = mighty_core::shell::delegation_tree(s.workspace.id(), &snapshot, &rows);
    assert_eq!(
        tree.children.get(PARENT).map(|children| children.iter().map(|row| row.id.as_str()).collect::<Vec<_>>()),
        Some(vec!["c1", "c2"]),
        "the sidebar lists both under the parent"
    );
    // A send whose start is refused takes nothing: the held notice stays held, and the refusal is the caller's to show.
    let refused = s.app.send(s.request(&"x".repeat(100_000), PARENT)).await;
    assert!(
        refused.is_err()
            && s.app.held_items(PARENT).iter().map(|item| item.id.as_str()).collect::<Vec<_>>() == ["n1"]
            && s.file().notices.first().is_some_and(|notice| notice.lane == DeliveryLane::Held)
            && s.runs(PARENT).is_empty(),
        "a send no run took leaves the held items held"
    );
    // Card actions: a discard confirmation that cannot list nested worktrees, and the answer kept for the card.
    let pending = s.app.ask_to_discard_child("c2").await.expect("a confirmation");
    assert!(
        pending.nested.is_none() && pending.branch == "mighty/c2" && !s.app.is_card_busy("c2"),
        "with the parent's folder gone, the confirmation cannot list nested worktrees and offers no discard"
    );
    let note = s.app.merge_child("c2").await.expect("a note");
    assert!(
        note.action == DelegationCardAction::Merge
            && matches!(note.result, DelegationCardResult::Refused(_) | DelegationCardResult::Failed(_))
            && s.app.card_note("c2") == Some(note.clone()),
        "a card's answer is kept for it: {note:?}"
    );
    // Closing a child pane reaches the coordinator once the pane is gone, its agent terminal ended first.
    let child = DelegationChildPane {
        session_id: "c1".into(),
        parent_session_id: PARENT.into(),
        mode: "plan".into(),
        folder: Path::new(&place.root()).join("c1").to_string_lossy().into_owned(),
        started_by_human: false,
        provider: "claude".into(),
    };
    add_panes(&s, vec![app_host::child_session(&child, s.workspace.id(), "Child", None, None)]);
    let closed = s.app.close_session("c1", || {}).await.unwrap();
    assert!(
        closed == Some(ChildCleanupOutcome::Kept(ChildCleanupHold::NotMerged))
            && s.file().children.iter().any(|record| record.id == "c1" && record.state == ChildState::Closed)
            && s.snapshot().session("c1").is_none(),
        "a child's close closes its record: {closed:?}"
    );
    assert_eq!(*window.ended.lock().unwrap(), ["c1"], "the child's agent terminal is ended before its close reaches the coordinator");
    s.dispose().await;
}

/// DelegationCrossAgentVerification "delegation: the service offers the worktree menu and starts another agent's child" (L706).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_service_offers_the_worktree_menu_and_starts_another_agents_child() {
    Box::pin(other_agents_child()).await;
}

async fn other_agents_child() {
    let _lane = lane::enter().await;
    let place = DelegationPlace::new();
    place.repository_default().await;
    let s = launch(&place, "auto", true, Arc::new(Knows { missing: vec!["gemini"], ..Knows::default() })).await;
    // The Claude parent's model, the Codex pane used last, and a read-only Codex pane.
    s.service
        .update(|state| {
            let mut next = state.clone();
            for pane in next.sessions_mut().iter_mut().filter(|pane| pane.id() == PARENT) {
                let mut changed = (**pane).clone();
                changed.model = Some("opus".into());
                if let Some(settings) = changed.settings.as_mut() {
                    settings.effort = "high".into();
                }
                *pane = Arc::new(changed);
            }
            next
        })
        .unwrap();
    let template = RunSession {
        model: Some("gpt-5.6-sol".into()),
        settings: Some(RunSettings { effort: "low".into(), permission_mode: "fullAccess".into(), ..RunSettings::default() }),
        logs: Some(vec![LogEntry::new("l".into(), "user", "hi".into(), "2099-01-01T00:00:00Z".into(), None)]),
        ..pane(&s, "codex-template", "claude", "codex", "fullAccess")
    };
    add_panes(&s, vec![pane(&s, "codex-manual", "claude", "codex", "manual"), template]);
    let parent = s.pane(PARENT);
    let choice = |agent: &str, mode: &str, blocked: Option<DelegationChildBlock>| DelegationChildChoice {
        agent: agent.into(),
        mode: Some(mode.into()),
        blocked,
    };
    assert_eq!(
        s.app.worktree_child_choices(&parent, false),
        [
            choice("claude", "auto", None),
            choice("codex", "acceptEdits", None),
            choice("gemini", "acceptEdits", Some(DelegationChildBlock::NotInstalled))
        ],
        "a Claude pane in auto offers its menu from what the window knows"
    );
    // The read-only Codex pane: its own child cannot write, a Claude plan child can, so the button shows for its menu alone.
    let manual = s.pane("codex-manual");
    s.app.refresh_worktree_readiness(&manual);
    awaited("the read-only Codex pane's menu", || s.app.worktree_menu_only(&s.snapshot(), &manual, false).then_some(())).await;
    let snapshot = s.snapshot();
    assert!(
        s.app.second_send(&snapshot, &manual) == ComposerSecondSend::Queue && s.app.offers_worktree_menu(&snapshot, &manual, false),
        "a read-only Codex pane offers the menu alone"
    );
    assert!(!s.app.worktree_menu_only(&snapshot, &parent, false), "a Claude pane's own child can write: not the menu alone");
    // Another agent's child the human starts takes that agent's last-used pane's model and effort, in its widest mode.
    let none: Arc<dyn Fn(String) -> mighty_platform::BoxFuture<'static, ()> + Send + Sync> = Arc::new(|_| Box::pin(async {}));
    let HumanChildStart::Started(child) =
        s.app.start_human_child(PARENT, "Try it in Codex.", vec![], false, none.clone(), Some("codex"), false).await
    else {
        panic!("no Codex child")
    };
    assert!(
        child.provider.as_deref() == Some("codex") && child.starting_mode == "acceptEdits" && child.started_by_human == Some(true),
        "a Codex child in acceptEdits"
    );
    awaited("its pane", || s.snapshot().session(&child.id).map(|_| ())).await;
    let made = s.pane(&child.id);
    let settings = made.settings.clone().unwrap();
    assert!(
        made.provider() == "codex"
            && made.model() == "gpt-5.6-sol"
            && made.parent_session_id.as_deref() == Some(PARENT)
            && settings.effort == "low"
            && settings.permission_mode == "acceptEdits",
        "it takes the Codex template's model and effort"
    );
    assert!(s.app.rows().iter().any(|row| row.id == child.id && row.agent == "codex" && row.started_by_human), "its row names its agent");
    let refused = s.app.start_human_child(PARENT, "Try it in Antigravity.", vec![], false, none, Some("gemini"), false).await;
    assert_eq!(
        refused,
        HumanChildStart::Failed(Locale::new("en").get_with("delegation.human.error.agentNotInstalledWindows", &[("agent", "Antigravity")])),
        "an agent the window found not installed is refused in the app's words"
    );
    let settle = std::time::Instant::now() + Duration::from_secs(30);
    while std::time::Instant::now() < settle && s.coordinator().is_starting(&child.id) {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    s.dispose().await;
}
