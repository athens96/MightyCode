//! `DelegationLaunch` (C# MightyCode.Core.Tests/DelegationScenarioVerification.cs:13-147): one launch of the app, headless — a real
//! `RunService` on a profile in a temp folder (its state saved and loaded by mighty-core's store), the desktop service's delegation
//! (`app::DelegationApp`) as its hooks, the OS's real listener (`AgentIo` over Windows' named pipe or the Mac's Unix socket) its runs reach the delegation
//! server's calls back through, Claude as `mighty-delegation-fake --fake-claude <root>` and Codex as `mighty-delegation-fake-cli
//! --fake-cli codex`. The switch and the worktree root are injected: nothing here reads the registry or touches
//! `%USERPROFILE%\.mightycode`. It records what the checks judge: each pane's runs in order with how they ended and what they were
//! given, and every permission card. Git, the fake Claude and its MCP server run through the platform's process host (Windows', or
//! M-A's on the Mac). Owner: W7-D.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mighty_core::locale::Locale;
use mighty_core::model::{AppSnapshot, RunSession, RunSettings, Workspace};
use mighty_core::permission::ToolPermissionRequest;
use mighty_core::store::Store;
use mighty_delegation::agent_io::binding::{AgentIoBinding, TOKEN_KEY};
use mighty_delegation::agent_io::server::{AgentIo, DelegationLink, TerminalCall, TerminalHandler, binding_factory};
use mighty_delegation::app::{DelegationApp, DelegationStart, DelegationWindow, MERGE_RETRY};
use mighty_delegation::coordinator::Coordinator;
use mighty_delegation::delivery::DelegationRelease;
use mighty_delegation::git::Git;
use mighty_delegation::git::worktree::Maker;
use mighty_delegation::request::DelegationHandler;
use mighty_delegation::store::{DelegationFile, DelegationFileStore};
use mighty_platform::BoxFuture;
use mighty_platform::fake::FixedDiskSpace;
use mighty_run::ServiceEvent;
use mighty_run::model::{CliCommand, StartRunRequest};
use mighty_run::providers::{Finder, ProviderCatalog};
use mighty_run::service::{RunService, RunServiceOptions};
use serde_json::{Value, json};

/// The OS's own Agent IO listener: Windows' named pipe, the Mac's Unix socket in the profile (M-C).
#[cfg(windows)]
fn listener(_profile: &Path) -> Arc<dyn mighty_platform::AgentIoListener> {
    Arc::new(mighty_platform_windows::NamedPipeListener::new())
}

/// The OS's own Agent IO listener: Windows' named pipe, the Mac's Unix socket in the profile (M-C).
#[cfg(target_os = "macos")]
fn listener(profile: &Path) -> Arc<dyn mighty_platform::AgentIoListener> {
    mighty_platform_macos::agent_io::agent_io_listener(profile)
}

/// No Agent IO transport elsewhere.
#[cfg(not(any(windows, target_os = "macos")))]
fn listener(_profile: &Path) -> Arc<dyn mighty_platform::AgentIoListener> {
    Arc::new(mighty_platform::unsupported::Unsupported)
}

pub const PARENT: &str = "parent";

/// The window's part as a launch has it: delegation acts (a scenario is a launch with `--allow-state-write`).
struct Window;
impl DelegationWindow for Window {}

/// No terminal call is made here.
struct NoTerminal;
impl TerminalHandler for NoTerminal {
    fn handle(&self, _call: TerminalCall) -> BoxFuture<'static, Result<Value, String>> {
        Box::pin(async { Err("No terminal calls are made here.".to_string()) })
    }
}

#[derive(Default)]
struct Record {
    runs: HashMap<String, Vec<String>>,
    ends: HashMap<String, String>,
    inputs: HashMap<String, String>,
    going: HashMap<String, String>,
    cards: Vec<ToolPermissionRequest>,
}

/// The project folder name Claude Code gives a working folder (`SessionHistory.ClaudeProjectFolder`).
pub fn project_folder(path: &str) -> String {
    path.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
}

pub struct DelegationLaunch {
    pub profile: PathBuf,
    pub fake_root: PathBuf,
    pub repo: String,
    pub workspace: Workspace,
    pub service: Arc<RunService>,
    pub app: Arc<DelegationApp>,
    pub switch_on: Arc<AtomicBool>,
    io: Arc<AgentIo>,
    tokens: Arc<Mutex<HashMap<String, String>>>,
    store: Arc<Mutex<Store>>,
    record: Arc<Mutex<Record>>,
    pump: Mutex<Option<tokio::task::JoinHandle<()>>>,
    disposed: AtomicBool,
}

fn fake_exe() -> String {
    env!("CARGO_BIN_EXE_mighty-delegation-fake").to_string()
}

fn fake_cli() -> String {
    env!("CARGO_BIN_EXE_mighty-delegation-fake-cli").to_string()
}

impl DelegationLaunch {
    /// A launch on `profile`. Without a saved workspace it adds `repo` and the parent pane in `parent_mode`.
    pub async fn start(
        profile: &Path,
        fake_root: &Path,
        repo: &str,
        worktrees: &str,
        git: Git,
        parent_mode: &str,
        switch_on: bool,
    ) -> DelegationLaunch {
        Self::start_with(profile, fake_root, repo, worktrees, git, parent_mode, switch_on, MERGE_RETRY, Arc::new(Window)).await
    }

    /// [`DelegationLaunch::start`] with the merge queue's retry interval and the window's part given (C# `MergeRetryInterval`,
    /// `QueuedMergeDone`).
    #[allow(clippy::too_many_arguments, reason = "the launch's parts, one argument each")]
    pub async fn start_with(
        profile: &Path,
        fake_root: &Path,
        repo: &str,
        worktrees: &str,
        git: Git,
        parent_mode: &str,
        switch_on: bool,
        merge_retry: Duration,
        window: Arc<dyn DelegationWindow>,
    ) -> DelegationLaunch {
        std::fs::create_dir_all(profile).unwrap();
        let plugin = fake_root.join("plugin");
        std::fs::create_dir_all(plugin.join(".claude-plugin")).unwrap();
        std::fs::write(plugin.join(".claude-plugin").join("plugin.json"), "{}").unwrap();
        let locale = Locale::new("en");
        let mut store = Store::new(profile.to_path_buf(), None, true);
        let loaded = store.load(&locale).unwrap_or_default();
        let (claude, codex) = (
            CliCommand::new(fake_exe(), vec!["--fake-claude".into(), fake_root.to_string_lossy().into_owned()], Some("2.1.271")),
            CliCommand::new(
                fake_cli(),
                vec!["--fake-cli".into(), "codex".into(), fake_root.join("codex").to_string_lossy().into_owned()],
                Some("0.51.0"),
            ),
        );
        let finder: Finder = Arc::new(move |provider: String| {
            let found = match provider.as_str() {
                "claude" => Some(claude.clone()),
                "codex" => Some(codex.clone()),
                _ => None,
            };
            Box::pin(async move { found })
        });
        let processes = super::place::processes();
        let catalog = Arc::new(ProviderCatalog::new(
            processes.clone(),
            Arc::new(mighty_platform::fake::FakeShellEnvironment::new(std::env::vars().collect())),
            Some(finder),
        ));
        let io = AgentIo::start(listener(profile), Arc::new(NoTerminal)).unwrap();
        let switch = Arc::new(AtomicBool::new(switch_on));
        let app = DelegationApp::new(tokio::runtime::Handle::current(), window);
        let (link_app, link_switch) = (Arc::downgrade(&app), switch.clone());
        let link: DelegationLink = Arc::new(move || {
            let handler = link_app.upgrade().and_then(|app| app.coordinator()).map(|c| Arc::new(c) as Arc<dyn DelegationHandler>);
            (handler, link_switch.load(Ordering::SeqCst))
        });
        let tokens: Arc<Mutex<HashMap<String, String>>> = Arc::default();
        let (service, events) = RunService::new(
            loaded,
            catalog,
            RunServiceOptions {
                plugin_directory: plugin.to_string_lossy().into_owned(),
                permission_card: true,
                remote: None,
                configure: None,
                admitting: None,
                binding: Some({
                    // The token each pane's newest run got, so a check reads that run's binding (C# `Bindings[pane]`).
                    let (inner, tokens) = (binding_factory(io.clone(), fake_exe(), link), tokens.clone());
                    Arc::new(move |request: &StartRunRequest, workspace: &Workspace| {
                        let made = inner(request, workspace);
                        // A Codex run's environment holds it; a Claude run's servers read it from the config file its arguments name.
                        let token = made.as_ref().and_then(|binding| {
                            binding.environment.get(TOKEN_KEY).cloned().or_else(|| {
                                let at = binding.arguments.iter().position(|a| a == "--mcp-config")?;
                                let text = std::fs::read_to_string(binding.arguments.get(at + 1)?).ok()?;
                                let config: serde_json::Value = serde_json::from_str(&text).ok()?;
                                config["mcpServers"]["mighty-terminal"]["env"][TOKEN_KEY].as_str().map(str::to_string)
                            })
                        });
                        if let Some(token) = token {
                            tokens.lock().unwrap().insert(request.session_id.clone(), token);
                        }
                        made
                    })
                }),
                delegation: Some(app.clone()),
            },
        );
        let service = Arc::new(service);
        app.attach(service.clone());
        let started_switch = switch.clone();
        let mut start = DelegationStart::new(
            profile.to_path_buf(),
            Maker::new(worktrees, git, Arc::new(FixedDiskSpace(50_000_000_000))),
            super::place::paths(),
            Arc::new(move || started_switch.load(Ordering::SeqCst)),
        );
        start.locale = Arc::new(move || locale);
        start.merge_retry = merge_retry;
        assert!(app.start(start).await, "the delegation file loads");
        let store = Arc::new(Mutex::new(store));
        let record = Arc::new(Mutex::new(Record::default()));
        let pump = tokio::spawn(Self::pump(events, service.clone(), app.clone(), store.clone(), record.clone()));
        let saved = service.snapshot().workspaces().first().cloned();
        let workspace = match saved {
            Some(saved) => saved,
            None => {
                let workspace = store.lock().unwrap().approve_local(Path::new(repo)).unwrap();
                let parent = RunSession {
                    id: Some(PARENT.into()),
                    workspace_id: workspace.id.clone(),
                    title: Some("Parent".into()),
                    settings: Some(RunSettings { permission_mode: parent_mode.into(), ..RunSettings::default() }),
                    ..RunSession::default()
                };
                let added = workspace.clone();
                service
                    .update(move |state| {
                        let mut next = state.clone();
                        next.workspaces_mut().push(added.clone());
                        next.sessions_mut().push(Arc::new(parent.clone()));
                        next.active_workspace_id = added.id.clone();
                        next
                    })
                    .unwrap();
                workspace
            }
        };
        DelegationLaunch {
            profile: profile.to_path_buf(),
            fake_root: fake_root.to_path_buf(),
            repo: repo.to_string(),
            workspace,
            service,
            app,
            switch_on: switch,
            io,
            tokens,
            store,
            record,
            pump: Mutex::new(Some(pump)),
            disposed: AtomicBool::new(false),
        }
    }

    /// What the launch hears of its service, in order: the saves, each run's id as it starts (read when no other run of the pane can
    /// begin) and its end labelled with the id its start carried, the requests' inputs, every permission card.
    async fn pump(
        mut events: tokio::sync::mpsc::UnboundedReceiver<ServiceEvent>,
        service: Arc<RunService>,
        app: Arc<DelegationApp>,
        store: Arc<Mutex<Store>>,
        record: Arc<Mutex<Record>>,
    ) {
        while let Some(event) = events.recv().await {
            match event {
                ServiceEvent::SaveNow | ServiceEvent::SaveSoon => {
                    let _ = store.lock().unwrap().save(&service.snapshot());
                }
                ServiceEvent::ToolPermission(card) => record.lock().unwrap().cards.push(*card),
                ServiceEvent::RequestStarting(request) => {
                    if let Some(run) = app.run_id(&request.session_id) {
                        record.lock().unwrap().inputs.insert(format!("{}/{run}", request.session_id), request.input.clone());
                    }
                }
                ServiceEvent::Run { event, .. } if event.kind == "status" => {
                    let mut record = record.lock().unwrap();
                    let pane = event.session_id.clone();
                    match event.status.as_deref() {
                        Some("running") => {
                            if let Some(run) = app.run_id(&pane) {
                                let list = record.runs.entry(pane.clone()).or_default();
                                if !list.contains(&run) {
                                    list.push(run.clone());
                                }
                                record.going.insert(pane, run);
                            }
                        }
                        Some(status @ ("completed" | "stopped" | "error")) => {
                            if let Some(run) = record.going.remove(&pane) {
                                record.ends.entry(run).or_insert_with(|| status.to_string());
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }

    /// The binding the pane's newest run got (C# `Bindings[pane]`), while it is the pane's current one.
    pub fn binding(&self, pane: &str) -> Option<Arc<AgentIoBinding>> {
        let token = self.tokens.lock().unwrap().get(pane).cloned()?;
        self.io.bindings().resolve(&token)
    }

    /// Whether any run of the pane got a binding.
    pub fn bound(&self, pane: &str) -> bool {
        self.tokens.lock().unwrap().contains_key(pane)
    }

    pub fn coordinator(&self) -> Coordinator {
        self.app.coordinator().expect("delegation is available")
    }

    /// The delegation file as it is on disk.
    pub fn file(&self) -> DelegationFile {
        DelegationFileStore::new(&self.profile).load().unwrap()
    }

    pub fn pane(&self, id: &str) -> Arc<RunSession> {
        self.service.snapshot().session(id).cloned().unwrap_or_else(|| panic!("no pane {id}"))
    }

    pub fn snapshot(&self) -> AppSnapshot {
        self.service.snapshot()
    }

    pub fn runs(&self, pane: &str) -> Vec<String> {
        self.record.lock().unwrap().runs.get(pane).cloned().unwrap_or_default()
    }

    /// What run `run` of the pane was given. The launch records it from the service's `RequestStarting` event, which reaches the
    /// recorder through the service's channel after the start returned (C#'s event is raised inside `Launch`): this waits for it.
    pub async fn input(&self, pane: &str, run: &str) -> Option<String> {
        let (record, key) = (self.record.clone(), format!("{pane}/{run}"));
        Some(awaited(&format!("the input of run {run} in {pane}"), || record.lock().unwrap().inputs.get(&key).cloned()).await)
    }

    pub fn inputs(&self, pane: &str) -> Vec<String> {
        let prefix = format!("{pane}/");
        self.record.lock().unwrap().inputs.iter().filter(|(key, _)| key.starts_with(&prefix)).map(|(_, input)| input.clone()).collect()
    }

    /// What a run in `folder` plays: one turn of steps per prompt, in order.
    pub fn script(&self, folder: &str, turns: Vec<Vec<Value>>) {
        let path = self.fake_root.join("scripts").join(format!("{}.json", project_folder(folder)));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, json!({"turns": turns}).to_string()).unwrap();
    }

    /// What a child's run plays when its first prompt holds `phrase`.
    pub fn child_script(&self, phrase: &str, steps: Vec<Value>) {
        let path = self.fake_root.join("child-scripts.json");
        let mut list: Vec<Value> =
            std::fs::read_to_string(&path).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default();
        list.push(json!({"phrase": phrase, "steps": steps}));
        std::fs::write(path, Value::Array(list).to_string()).unwrap();
    }

    /// The fake Claude's log of runs in `folder`.
    pub fn log(&self, folder: &str) -> Vec<Value> {
        let path = self.fake_root.join("logs").join(format!("{}.jsonl", project_folder(folder)));
        std::fs::read_to_string(path)
            .map(|text| text.lines().filter_map(|line| serde_json::from_str(line).ok()).collect())
            .unwrap_or_default()
    }

    /// The prompts the model received in the one Claude-format transcript of the session run in `folder`.
    pub fn prompts(&self, folder: &str) -> Vec<String> {
        let projects = self.fake_root.join("claude-config").join("projects").join(project_folder(folder));
        let transcripts: Vec<PathBuf> = std::fs::read_dir(&projects)
            .unwrap()
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .filter(|path| path.extension().is_some_and(|e| e == "jsonl"))
            .collect();
        assert_eq!(transcripts.len(), 1, "one session in {folder}");
        std::fs::read_to_string(&transcripts[0])
            .unwrap()
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter(|line| line["type"] == "user")
            .filter_map(|line| line["message"]["content"].as_str().map(str::to_string))
            .collect()
    }

    /// The one session the parent's transcript is (its file's name).
    pub fn session_of(&self, folder: &str) -> String {
        let projects = self.fake_root.join("claude-config").join("projects").join(project_folder(folder));
        let first = std::fs::read_dir(projects).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).next().unwrap();
        first.file_stem().unwrap().to_string_lossy().into_owned()
    }

    /// The card for `tool` still open in the pane, skipping `excluding`.
    pub fn open_card(&self, pane: &str, tool: &str, excluding: &[&str]) -> Option<ToolPermissionRequest> {
        let record = self.record.lock().unwrap();
        record
            .cards
            .iter()
            .find(|card| {
                card.run_id == pane
                    && card.tool_name == tool
                    && card.state == "pending"
                    && !excluding.contains(&card.id.as_str())
                    && !record.cards.iter().any(|other| other.run_id == pane && other.id == card.id && other.state != "pending")
            })
            .cloned()
    }

    /// An argument the card shows (its tool input's member of that name).
    pub fn shown(card: &ToolPermissionRequest, name: &str) -> Option<String> {
        serde_json::from_str::<Value>(&card.input_json).ok().and_then(|input| input[name].as_str().map(str::to_string))
    }

    pub async fn allow(&self, card: &ToolPermissionRequest) {
        self.service.respond_to_tool_permission(&card.run_id, &card.id, true).await.unwrap();
    }

    /// The end of run `run` in `pane`.
    pub async fn ended(&self, pane: &str, run: &str) -> String {
        let record = self.record.clone();
        awaited(&format!("the end of run {run} in {pane}"), || record.lock().unwrap().ends.get(run).cloned()).await
    }

    /// The human's send in the pane with nothing held for it: the run it started.
    pub async fn human_send(&self, text: &str, pane: &str) -> String {
        let before = self.runs(pane).len();
        let release = self.app.send(self.request(text, pane)).await.unwrap();
        assert_eq!(release, DelegationRelease::NothingHeld, "nothing was held for {pane}");
        awaited(&format!("the run of {text}"), || self.runs(pane).get(before).cloned()).await
    }

    pub fn request(&self, text: &str, pane: &str) -> StartRunRequest {
        let session = self.pane(pane);
        let mut request = StartRunRequest::new(pane, session.workspace_id(), "claude", text);
        request.model = session.model().to_string();
        request.provider = session.provider().to_string();
        request.settings = session.settings.clone();
        request.resume_id = session.resume_id.clone();
        request
    }

    /// Quits the app: every run ends, as quitting ends it, delegation records it, and the profile is saved; the pipe closes.
    pub async fn dispose(&self) {
        if self.disposed.swap(true, Ordering::SeqCst) {
            return;
        }
        self.service.dispose().await;
        self.app.stop().await;
        // The recorder stops first (it saves on the service's save events), so the last save below is the last write to the profile
        // and nothing writes it once this returned (C#'s DisposeAsync awaits its saves).
        let pump = self.pump.lock().unwrap().take();
        if let Some(pump) = pump {
            pump.abort();
            let _ = pump.await;
        }
        let _ = self.store.lock().unwrap().save(&self.service.snapshot());
        let _ = tokio::time::timeout(Duration::from_secs(5), self.io.close()).await;
    }
}

/// Polls until `produce` has a value. The bound only ends a scenario that broke; nothing here asserts how long a step took.
pub async fn awaited<T>(what: &str, mut produce: impl FnMut() -> Option<T>) -> T {
    let end = Instant::now() + Duration::from_secs(120);
    loop {
        if let Some(found) = produce() {
            return found;
        }
        assert!(Instant::now() < end, "the scenario never reached: {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Steps of the fake Claude's scripts (C# `Scenario`).
pub mod steps {
    use serde_json::{Map, Value, json};

    pub fn say(text: &str) -> Value {
        json!({"say": text})
    }
    pub fn call(tool: &str, ask: bool, arguments: &[(&str, &str)]) -> Value {
        let arguments: Map<String, Value> = arguments.iter().map(|(name, value)| (name.to_string(), json!(value))).collect();
        json!({"call": tool, "ask": ask, "arguments": arguments})
    }
    pub fn permission(tool: &str, input: &[(&str, &str)]) -> Value {
        let input: Map<String, Value> = input.iter().map(|(name, value)| (name.to_string(), json!(value))).collect();
        json!({"permission": tool, "input": input})
    }
    pub fn run(ask: bool, commands: &[&[&str]]) -> Value {
        json!({"run": commands, "ask": ask})
    }
    pub fn write(path: &str, text: &str) -> Value {
        json!({"write": path, "text": text})
    }
    /// `ChildWorktree.NotesFolder + "/REPORT.md"`.
    pub fn report() -> String {
        format!("{}/REPORT.md", mighty_delegation::git::worktree::NOTES_FOLDER)
    }
    /// A child's steps: a tracked file and its commit, asked for first.
    pub fn commit(file: &str, text: &str) -> Vec<Value> {
        let message = format!("Add {file}");
        vec![write(file, text), run(true, &[&["git", "add", "--", file], &["git", "commit", "-q", "-m", &message]])]
    }
    /// `--name value` in an argv event.
    pub fn option(argv: &Value, name: &str) -> Option<String> {
        let values: Vec<&str> = argv["arguments"].as_array()?.iter().filter_map(Value::as_str).collect();
        let at = values.iter().position(|value| *value == name)?;
        values.get(at + 1).map(|value| value.to_string())
    }
    pub fn structured(call: &Value) -> Value {
        call["result"]["structuredContent"].clone()
    }
    pub fn text(call: &Value) -> String {
        call["result"]["content"]
            .as_array()
            .map(|blocks| blocks.iter().filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("\n"))
            .unwrap_or_default()
    }
}
