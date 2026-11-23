//! MC-38: every agent's run kept in the background daemon and taken back by a later app, through the run engine, over real children
//! (the fake CLIs through the OS's process host, owned by a daemon served in memory), as `daemon_adopt.rs` does for Claude: Codex's
//! approval app-server (its waiting card back, answered, the run's binding handed on), Codex's `exec --json` of a delegated child
//! (delegation told the run goes on), Antigravity's stream, an ACP agent (opencode: an approval and a question), and pi (its dialog).
//! The first app never answers; it quits keeping its work; the later app writes nothing the agent did not ask for, shows the waiting
//! card again, and its answer reaches the agent, which then ends the run.

mod common;

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use indexmap::IndexMap;
use mighty_core::daemon::protocol::ProcInfo;
use mighty_core::model::{AppSnapshot, RunSession, RunSettings, Workspace};
use mighty_platform::AgentIoListener;
use mighty_platform::fake::{FakeAgentIo, FakePtyHost};
use mighty_run::daemon::client::DaemonClient;
use mighty_run::daemon::owner::{Adoption, KEPT_STATE, RESUME_AT, Route, RoutedProcesses, run_key};
use mighty_run::daemon::server::{Daemon, Options};
use mighty_run::manager::{BindingFactory, Emit, ResolveWorkspace, RunBinding};
use mighty_run::model::{CliCommand, KeptState, RunEvent, StartRunRequest};
use mighty_run::permissions::ToolPermissionRequest;
use mighty_run::providers::ProviderCatalog;
use mighty_run::questionnaire::{UserQuestionAnswer, UserQuestionnaire};
use mighty_run::service::{DelegationRunHooks, RunService, RunServiceOptions, ServiceEvent};
use mighty_run::{RunManager, RunManagerOptions};
use serde_json::{Value, json};

const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// A daemon served in memory over the OS's own process host.
struct Served {
    daemon: Arc<Daemon>,
    io: FakeAgentIo,
    name: String,
    served: tokio::task::JoinHandle<()>,
}

impl Served {
    fn start() -> Served {
        let daemon =
            Daemon::new(common::processes(), Arc::new(FakePtyHost::default()), Options::new("0.6.0", TOKEN), Arc::new(|_: &str| {}));
        let io = FakeAgentIo::default();
        let name = io.bind().unwrap();
        let served = tokio::spawn(daemon.clone().serve(Arc::new(io.clone()) as Arc<dyn AgentIoListener>));
        Served { daemon, io, name, served }
    }

    async fn connect(&self) -> Arc<DaemonClient> {
        DaemonClient::connect(&self.io, &self.name, TOKEN, "test", Duration::from_secs(5)).await.unwrap()
    }

    /// The kept run of `pane`, still running in the daemon.
    async fn kept(&self, pane: &str) -> ProcInfo {
        let listed = self.connect().await.list().await.unwrap();
        let kept = listed.iter().find(|p| p.key.as_deref() == Some(run_key(pane, true).as_str())).expect("the run is kept").clone();
        assert!(kept.running, "the CLI still runs in the daemon");
        kept
    }

    async fn stop(self) {
        self.daemon.stop();
        let _ = self.served.await;
    }
}

fn resolve(workspace: &Workspace) -> ResolveWorkspace {
    let workspace = workspace.clone();
    Arc::new(move |_| {
        let workspace = workspace.clone();
        Box::pin(async move { Ok(workspace) })
    })
}

fn emit(list: &Arc<Mutex<Vec<RunEvent>>>) -> Emit {
    let list = list.clone();
    Arc::new(move |event| list.lock().unwrap().push(event))
}

/// A catalog whose every agent is `command`, its children through `route` (the daemon while one is connected).
fn catalog(route: &Arc<Route>, command: &CliCommand) -> Arc<ProviderCatalog> {
    let command = command.clone();
    let finder: mighty_run::providers::Finder = Arc::new(move |_provider: String| {
        let found = Some(command.clone());
        Box::pin(async move { found })
    });
    let processes = Arc::new(RoutedProcesses::new(common::processes(), route.clone()));
    Arc::new(ProviderCatalog::new(processes, common::environment(), Some(finder)))
}

fn workspace(directory: &Path) -> (Workspace, String) {
    let workspace = Workspace { path: Some(directory.to_string_lossy().into_owned()), ..Workspace::default() };
    let id = workspace.id.clone().unwrap_or_else(|| "w-1".into());
    (workspace, id)
}

fn request(pane: &str, workspace_id: &str, provider: &str, mode: &str) -> StartRunRequest {
    StartRunRequest::new(pane, workspace_id, "claude", "fixture prompt")
        .with_provider(provider)
        .with_settings(RunSettings { permission_mode: mode.into(), ..RunSettings::default() })
}

/// The first app: the run goes to the daemon; every card it shows is recorded and never answered.
struct First {
    route: Arc<Route>,
    manager: Arc<RunManager>,
    cards: Arc<Mutex<Vec<ToolPermissionRequest>>>,
    events: Arc<Mutex<Vec<RunEvent>>>,
}

impl First {
    async fn start(served: &Served, command: &CliCommand, workspace: &Workspace, plugin: &str, request: StartRunRequest) -> First {
        First::start_with(served, command, workspace, plugin, request, None).await
    }

    async fn start_with(
        served: &Served,
        command: &CliCommand,
        workspace: &Workspace,
        plugin: &str,
        request: StartRunRequest,
        binding: Option<BindingFactory>,
    ) -> First {
        let route = Route::new();
        route.set(Some(served.connect().await));
        let cards: Arc<Mutex<Vec<ToolPermissionRequest>>> = Arc::default();
        let events: Arc<Mutex<Vec<RunEvent>>> = Arc::default();
        let mut options = RunManagerOptions::new(plugin, resolve(workspace), emit(&events));
        let sink = cards.clone();
        options.permission_requested = Some(Arc::new(move |value: ToolPermissionRequest| sink.lock().unwrap().push(value)));
        options.binding = binding;
        let manager = Arc::new(RunManager::new(catalog(&route, command), options));
        manager.start(request).await.unwrap();
        First { route, manager, cards, events }
    }

    async fn waiting(&self, what: &str) {
        common::until(|| self.cards.lock().unwrap().iter().any(|r| r.state == "pending"), 30_000, what).await;
    }

    fn logs(&self) -> String {
        let events = self.events.lock().unwrap();
        events.iter().filter_map(|e| e.entry.as_ref().and_then(|l| l.text.clone())).collect::<Vec<_>>().join("\n")
    }

    /// It quits keeping its work: let go first, then the run engine is disposed (its run ends here, the child runs on there).
    async fn keep(self, pane: &str) {
        self.route.detach().await;
        self.manager.dispose().await;
        assert!(!self.manager.is_running(pane));
        // Let go of, the run ends here as stopped, with nothing of it read as a failure.
        let events = self.events.lock().unwrap();
        let last = events.iter().rev().find(|e| e.kind == "status").and_then(|e| e.status.clone());
        assert_eq!(last.as_deref(), Some("stopped"), "the first app's run ends stopped");
        let errors: Vec<String> = events
            .iter()
            .filter_map(|e| e.entry.as_ref().filter(|l| l.kind.as_deref() == Some("error")).and_then(|l| l.text.clone()))
            .collect();
        assert!(errors.is_empty(), "no error is logged: {errors:?}");
    }
}

/// What delegation hears of the later app's runs.
#[derive(Default)]
struct Hooks {
    adopted: Mutex<Vec<String>>,
    began: Mutex<Vec<String>>,
    ended: Mutex<Vec<(String, String)>>,
    root: Option<String>,
}

impl DelegationRunHooks for Hooks {
    fn worktree_root(&self, pane: &RunSession) -> Option<String> {
        pane.parent_session_id.as_ref().and(self.root.clone())
    }
    fn began(&self, pane: &RunSession) -> String {
        self.began.lock().unwrap().push(pane.id().to_string());
        "run-began".into()
    }
    fn adopted(&self, pane: &RunSession) -> String {
        self.adopted.lock().unwrap().push(pane.id().to_string());
        "run-adopted".into()
    }
    fn ended(&self, session_id: &str, status: &str, _closing: bool) -> bool {
        self.ended.lock().unwrap().push((session_id.to_string(), status.to_string()));
        true
    }
}

/// The later app: the kept run taken back through the run service (`RunService::adopt`), every waiting card answered (an approval
/// allowed, a question with its first choice). The pane as it ended, and every card it showed.
struct Later {
    session: RunSession,
    cards: Vec<ToolPermissionRequest>,
}

struct Taking<'a> {
    served: &'a Served,
    command: &'a CliCommand,
    workspace: &'a Workspace,
    plugin: &'a str,
    pane: RunSession,
    request: StartRunRequest,
    binding: Option<BindingFactory>,
    hooks: Option<Arc<Hooks>>,
}

impl Taking<'_> {
    async fn take_back(self) -> Later {
        let pane_id = self.pane.id().to_string();
        let kept = self.served.kept(&pane_id).await;
        let from = kept.meta[RESUME_AT].as_u64().expect("where the later app takes it up");
        let route = Route::new();
        route.set(Some(self.served.connect().await));
        route.expect(&run_key(&pane_id, true), Adoption { proc: kept.proc.clone(), pid: kept.pid, from });
        let hooks = self.hooks.clone();
        let (service, mut service_events) = RunService::new(
            AppSnapshot::default(),
            catalog(&route, self.command),
            RunServiceOptions {
                plugin_directory: self.plugin.to_string(),
                permission_card: true,
                remote: None,
                configure: None,
                admitting: None,
                binding: self.binding,
                delegation: hooks.map(|hooks| hooks as Arc<dyn DelegationRunHooks>),
            },
        );
        let service = Arc::new(service);
        let (workspace, pane) = (self.workspace.clone(), self.pane.clone());
        service
            .update(|s| AppSnapshot {
                workspaces: Some(vec![workspace.clone()]),
                sessions: Some(vec![Arc::new(pane.clone())]),
                active_workspace_id: workspace.id.clone(),
                ..s.clone()
            })
            .unwrap();
        let seen: Arc<Mutex<Vec<ToolPermissionRequest>>> = Arc::default();
        let (answering, sink, pane) = (service.clone(), seen.clone(), pane_id.clone());
        let listener = tokio::spawn(async move {
            while let Some(event) = service_events.recv().await {
                let ServiceEvent::ToolPermission(value) = event else { continue };
                sink.lock().unwrap().push(value.as_ref().clone());
                if value.state != "pending" {
                    continue;
                }
                if value.can_answer_questions {
                    let asked = UserQuestionnaire::parse(&value.input_json).unwrap().questions[0].clone();
                    let picks = IndexMap::from([(asked.question.clone(), UserQuestionAnswer::new(&[&asked.options[0].label], None))]);
                    answering.answer_questionnaire(&pane, &value.id, &picks).await.unwrap();
                } else {
                    answering.respond_to_tool_permission(&pane, &value.id, true).await.unwrap();
                }
            }
        });
        let mut request = self.request;
        request.kept = kept.meta.get(KEPT_STATE).cloned().map(KeptState);
        service.adopt(request).await.unwrap();
        let status = || service.snapshot().session(&pane_id).and_then(|s| s.status.clone());
        common::until(
            || !service.is_session_running(&pane_id) && status().as_deref() != Some("running"),
            30_000,
            "the taken-back run ends",
        )
        .await;
        let session = service.snapshot().session(&pane_id).unwrap().as_ref().clone();
        service.dispose().await;
        listener.abort();
        let cards = seen.lock().unwrap().clone();
        Later { session, cards }
    }
}

fn pane(id: &str, workspace: &Workspace, provider: &str, mode: &str) -> RunSession {
    RunSession {
        id: Some(id.into()),
        workspace_id: workspace.id.clone(),
        title: Some("Kept".into()),
        provider: Some(provider.into()),
        settings: Some(RunSettings { permission_mode: mode.into(), ..RunSettings::default() }),
        ..RunSession::default()
    }
}

fn texts(session: &RunSession) -> String {
    session.logs.clone().unwrap_or_default().iter().filter_map(|l| l.text.clone()).collect::<Vec<_>>().join("\n")
}

fn no_user_entry(session: &RunSession) {
    let logs = session.logs.clone().unwrap_or_default();
    assert!(!logs.iter().any(|entry| entry.kind.as_deref() == Some("user")), "taking a run back adds no request of its own: {logs:?}");
}

fn lines(path: impl AsRef<Path>) -> Vec<String> {
    std::fs::read_to_string(path).unwrap_or_default().lines().filter(|l| !l.is_empty()).map(str::to_string).collect()
}

/// A binding factory that hands each run `token` and records what each adopted request carried of the kept binding.
fn binding(token: &'static str, seen: Arc<Mutex<Vec<(bool, Value)>>>) -> BindingFactory {
    Arc::new(move |request: &StartRunRequest, _workspace: &Workspace| {
        let kept = request.kept.as_ref().map(|kept| kept.part("binding").clone()).unwrap_or(Value::Null);
        seen.lock().unwrap().push((request.adopted, kept.clone()));
        if request.adopted && kept.is_null() {
            return None;
        }
        Some(RunBinding {
            environment: Default::default(),
            arguments: vec![],
            redact: Arc::new(|line: &str| line.to_string()),
            ended: None,
            kept: Some(json!({"token": token, "pipe": "/tmp/agent-io.sock"})),
        })
    })
}

/// Codex's approval app-server: the waiting command card comes back in the later app, which writes no handshake, no thread and no
/// turn (the stand-in fails the run on any frame but the answer), and its allow reaches Codex, which completes the turn. The run's
/// binding is left with the daemon and handed to the later app's factory.
#[tokio::test(flavor = "multi_thread")]
async fn a_kept_codex_approval_turn_is_taken_back_with_its_card_and_binding() {
    common::korean();
    let directory = common::temp("adopt-codex-approval");
    let (workspace, workspace_id) = workspace(&directory);
    let plugin = common::plugin(&directory);
    let command = common::fake_command(&["--codex-approval-fixture"]);
    let served = Served::start();
    let bound: Arc<Mutex<Vec<(bool, Value)>>> = Arc::default();

    let first = First::start_with(
        &served,
        &command,
        &workspace,
        &plugin,
        request("codex", &workspace_id, "codex", "onRequest"),
        Some(binding("first-token", bound.clone())),
    )
    .await;
    first.waiting("the command card waits").await;
    first.keep("codex").await;
    let kept = served.kept("codex").await;
    assert_eq!(kept.meta[KEPT_STATE]["codex"]["threadId"], "thread-1", "the turn's thread is left with the daemon");
    assert_eq!(kept.meta[KEPT_STATE]["codex"]["requests"].as_array().map(Vec::len), Some(1), "the waiting request is left too");

    let later = Taking {
        served: &served,
        command: &command,
        workspace: &workspace,
        plugin: &plugin,
        pane: pane("codex", &workspace, "codex", "onRequest"),
        request: request("codex", &workspace_id, "codex", "onRequest").tap_input("(continue)"),
        binding: Some(binding("later-token", bound.clone())),
        hooks: None,
    }
    .take_back()
    .await;
    assert!(
        later.cards.iter().any(|r| r.state == "pending" && r.tool_name == "command_execution"),
        "the card came back: {:?}",
        later.cards
    );
    assert!(later.cards.iter().any(|r| r.state == "allowed"), "and was answered");
    assert_eq!(later.session.status.as_deref(), Some("completed"), "the turn completed: {}", texts(&later.session));
    no_user_entry(&later.session);
    let bound = bound.lock().unwrap().clone();
    assert_eq!(bound.len(), 2, "each app bound the run once");
    assert!(!bound[0].0 && bound[1].0, "the later app's binding is the adopted run's");
    assert_eq!(bound[1].1["token"], "first-token", "with the binding its CLI was started with");
    served.stop().await;
    common::remove_when_free(&directory).await;
}

/// Codex's `exec --json` of a delegated child: kept (a child's run was never kept before MC-38), read on from where the first app
/// stopped (its second half only), its prompt written once, and delegation hears that the run goes on and how it ended.
#[tokio::test(flavor = "multi_thread")]
async fn a_kept_codex_exec_run_of_a_delegated_child_goes_on_and_ends_through_delegation() {
    common::korean();
    let directory = common::temp("adopt-codex-exec");
    let (workspace, workspace_id) = workspace(&directory);
    let plugin = common::plugin(&directory);
    let record = directory.join("exec").to_string_lossy().into_owned();
    let command = common::fake_command(&["--kept-stream", "codex", &record]);
    let served = Served::start();
    let mut asked = request("child", &workspace_id, "codex", "acceptEdits");
    asked.delegation_worktree_root = Some(directory.to_string_lossy().into_owned());

    let first = First::start(&served, &command, &workspace, &plugin, asked).await;
    common::until(|| first.logs().contains("FIRST_HALF"), 30_000, "the first half is read").await;
    first.keep("child").await;
    std::fs::write(format!("{record}.go"), "").unwrap();

    let hooks = Arc::new(Hooks { root: Some(directory.to_string_lossy().into_owned()), ..Hooks::default() });
    let mut child = pane("child", &workspace, "codex", "acceptEdits");
    child.parent_session_id = Some("parent".into());
    child.working_folder = Some(directory.to_string_lossy().into_owned());
    let later = Taking {
        served: &served,
        command: &command,
        workspace: &workspace,
        plugin: &plugin,
        pane: child,
        request: request("child", &workspace_id, "codex", "acceptEdits").tap_input("(continue)"),
        binding: None,
        hooks: Some(hooks.clone()),
    }
    .take_back()
    .await;
    let shown = texts(&later.session);
    assert_eq!(later.session.status.as_deref(), Some("completed"), "completed: {shown}");
    assert!(shown.contains("SECOND_HALF") && !shown.contains("FIRST_HALF"), "read on from where the first app stopped: {shown}");
    no_user_entry(&later.session);
    assert_eq!(std::fs::read_to_string(format!("{record}.prompt")).unwrap().matches("\n--\n").count(), 1, "the prompt went once");
    assert_eq!(*hooks.adopted.lock().unwrap(), ["child"], "delegation heard the child's run goes on");
    assert!(hooks.began.lock().unwrap().is_empty(), "as no new run");
    assert_eq!(*hooks.ended.lock().unwrap(), [("child".to_string(), "completed".to_string())], "and how it ended");
    served.stop().await;
    common::remove_when_free(&directory).await;
}

/// Antigravity's headless stream: kept and read on from where the first app stopped.
#[tokio::test(flavor = "multi_thread")]
async fn a_kept_antigravity_run_goes_on() {
    common::korean();
    let directory = common::temp("adopt-agy");
    let (workspace, workspace_id) = workspace(&directory);
    let plugin = common::plugin(&directory);
    let record = directory.join("agy").to_string_lossy().into_owned();
    let command = common::fake_command(&["--kept-stream", "agy", &record]);
    let served = Served::start();

    let first = First::start(&served, &command, &workspace, &plugin, request("agy", &workspace_id, "gemini", "acceptEdits")).await;
    common::until(|| first.logs().contains("FIRST_HALF"), 30_000, "the first half is read").await;
    first.keep("agy").await;
    std::fs::write(format!("{record}.go"), "").unwrap();

    let later = Taking {
        served: &served,
        command: &command,
        workspace: &workspace,
        plugin: &plugin,
        pane: pane("agy", &workspace, "gemini", "acceptEdits"),
        request: request("agy", &workspace_id, "gemini", "acceptEdits").tap_input("(continue)"),
        binding: None,
        hooks: None,
    }
    .take_back()
    .await;
    let shown = texts(&later.session);
    assert_eq!(later.session.status.as_deref(), Some("completed"), "completed: {shown}");
    assert!(shown.contains("SECOND_HALF") && !shown.contains("FIRST_HALF"), "read on from where the first app stopped: {shown}");
    assert_eq!(std::fs::read_to_string(format!("{record}.prompt")).unwrap().matches("\n--\n").count(), 1, "the prompt went once");
    served.stop().await;
    common::remove_when_free(&directory).await;
}

/// An ACP agent (opencode): its waiting approval, and its waiting question, come back in the later app, which writes no
/// `initialize`, no session and no prompt of its own; each answer reaches the agent once and the turn completes.
#[tokio::test(flavor = "multi_thread")]
async fn a_kept_acp_turn_is_taken_back_with_its_waiting_card() {
    common::korean();
    mighty_run::providers::set_agent_files(Some(common::agents()));
    for (recording, question) in [("tools", false), ("question", true)] {
        let directory = common::temp(&format!("adopt-acp-{recording}"));
        let (workspace, workspace_id) = workspace(&directory);
        let plugin = common::plugin(&directory);
        let fixture = common::fixtures().join(format!("opencode-acp-{recording}.jsonl")).to_string_lossy().into_owned();
        let state = directory.to_string_lossy().into_owned();
        let command = CliCommand::new(
            common::fake_text(),
            vec!["--fake-acp".into(), state, fixture, "--fake-await-answers".into()],
            Some("opencode v2.2.7"),
        );
        let served = Served::start();

        let first = First::start(&served, &command, &workspace, &plugin, request("acp", &workspace_id, "opencode", "manual")).await;
        first.waiting("the card waits").await;
        first.keep("acp").await;
        let kept = served.kept("acp").await;
        assert!(kept.meta[KEPT_STATE]["acp"]["sessionId"].is_string(), "the session is left with the daemon");

        let later = Taking {
            served: &served,
            command: &command,
            workspace: &workspace,
            plugin: &plugin,
            pane: pane("acp", &workspace, "opencode", "manual"),
            request: request("acp", &workspace_id, "opencode", "manual").tap_input("(continue)"),
            binding: None,
            hooks: None,
        }
        .take_back()
        .await;
        let states: Vec<&str> = later.cards.iter().map(|c| c.state.as_str()).collect();
        let settled = if question { "answered" } else { "allowed" };
        assert_eq!(states, ["pending", settled], "{recording}: the card came back and was answered once");
        assert_eq!(later.session.status.as_deref(), Some("completed"), "{recording}: {}", texts(&later.session));
        no_user_entry(&later.session);
        let stdin = lines(directory.join("stdin.txt"));
        let methods: Vec<String> = stdin.iter().filter_map(|l| common::json(l)["method"].as_str().map(str::to_string)).collect();
        assert_eq!(methods, ["initialize", "session/new", "session/prompt"], "{recording}: the handshake and prompt went once: {stdin:?}");
        let answers = stdin.iter().filter(|l| common::json(l).get("method").is_none()).count();
        assert_eq!(answers, 1, "{recording}: one answer reached the agent: {stdin:?}");
        served.stop().await;
        common::remove_when_free(&directory).await;
    }
}

/// pi: its `ask_user` dialog comes back as the later app's card; the answer reaches pi, which ends its turn, and the later app closes
/// its input after the turn as the first one would have. `get_state` and the prompt went once.
#[tokio::test(flavor = "multi_thread")]
async fn a_kept_pi_turn_is_taken_back_with_its_waiting_dialog() {
    common::korean();
    mighty_run::providers::set_agent_files(Some(common::agents()));
    let directory = common::temp("adopt-pi");
    let (workspace, workspace_id) = workspace(&directory);
    let plugin = common::plugin(&directory);
    let record = directory.join("pi").to_string_lossy().into_owned();
    let fixture = common::fixtures().join("pi-rpc-question.jsonl").to_string_lossy().into_owned();
    let command = common::fake_command(&["--fake-cli", "pi", &record, "--pi-fixture", &fixture, "--pi-dialogs"]);
    let served = Served::start();

    let first = First::start(&served, &command, &workspace, &plugin, request("pi", &workspace_id, "pi", "acceptEdits")).await;
    first.waiting("the dialog waits").await;
    first.keep("pi").await;
    let kept = served.kept("pi").await;
    assert_eq!(kept.meta[KEPT_STATE]["pi"]["dialogs"].as_array().map(Vec::len), Some(1), "the dialog is left with the daemon");

    let later = Taking {
        served: &served,
        command: &command,
        workspace: &workspace,
        plugin: &plugin,
        pane: pane("pi", &workspace, "pi", "acceptEdits"),
        request: request("pi", &workspace_id, "pi", "acceptEdits").tap_input("(continue)"),
        binding: None,
        hooks: None,
    }
    .take_back()
    .await;
    let states: Vec<&str> = later.cards.iter().map(|c| c.state.as_str()).collect();
    assert_eq!(states, ["pending", "answered"], "the dialog came back and was answered");
    assert_eq!(later.session.status.as_deref(), Some("completed"), "{}", texts(&later.session));
    let stdin = lines(format!("{record}.stdin"));
    assert_eq!(stdin.iter().filter(|l| l.contains("\"get_state\"")).count(), 1, "get_state went once: {stdin:?}");
    assert_eq!(stdin.iter().filter(|l| l.contains("\"prompt\"")).count(), 1, "the prompt went once: {stdin:?}");
    assert_eq!(stdin.iter().filter(|l| l.contains("extension_ui_response")).count(), 1, "one answer: {stdin:?}");
    assert_eq!(std::fs::read_to_string(format!("{record}.eof")).unwrap(), "after agent_end", "input closed after the turn");
    served.stop().await;
    common::remove_when_free(&directory).await;
}

trait TapInput {
    fn tap_input(self, input: &str) -> Self;
}

impl TapInput for StartRunRequest {
    fn tap_input(mut self, input: &str) -> StartRunRequest {
        self.input = input.to_string();
        self
    }
}

/// A run taken back whose kept child the daemon no longer holds is never started anew: the pane ends in an error saying so.
#[tokio::test(flavor = "multi_thread")]
async fn a_run_whose_kept_child_is_gone_is_not_started_again() {
    common::korean();
    let directory = common::temp("adopt-gone");
    let (workspace, workspace_id) = workspace(&directory);
    let plugin = common::plugin(&directory);
    let record = directory.join("gone").to_string_lossy().into_owned();
    let command = common::fake_command(&["--kept-stream", "codex", &record]);
    let served = Served::start();
    let route = Route::new();
    route.set(Some(served.connect().await));
    let (service, _events) = RunService::new(
        AppSnapshot::default(),
        catalog(&route, &command),
        RunServiceOptions {
            plugin_directory: plugin.clone(),
            permission_card: true,
            remote: None,
            configure: None,
            admitting: None,
            binding: None,
            delegation: None,
        },
    );
    let kept = pane("gone", &workspace, "codex", "acceptEdits");
    service
        .update(|s| AppSnapshot {
            workspaces: Some(vec![workspace.clone()]),
            sessions: Some(vec![Arc::new(kept.clone())]),
            active_workspace_id: workspace.id.clone(),
            ..s.clone()
        })
        .unwrap();
    let _ = service.adopt(request("gone", &workspace_id, "codex", "acceptEdits").tap_input("(continue)")).await;
    let status = || service.snapshot().session("gone").and_then(|s| s.status.clone());
    common::until(|| !service.is_session_running("gone") && status().as_deref() != Some("running"), 30_000, "the run ends").await;
    let session = service.snapshot().session("gone").unwrap().as_ref().clone();
    service.dispose().await;
    assert_eq!(session.status.as_deref(), Some("error"));
    assert!(texts(&session).contains(&mighty_run::strings::get("run.error.keptRunGone")), "{}", texts(&session));
    assert!(!Path::new(&format!("{record}.prompt")).exists(), "nothing was started");
    assert!(served.connect().await.list().await.unwrap().is_empty(), "nothing went to the daemon");
    served.stop().await;
    common::remove_when_free(&directory).await;
}
