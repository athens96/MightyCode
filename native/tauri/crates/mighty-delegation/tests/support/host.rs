//! `DelegationTestHost` (C# MightyCode.Core.Tests/DelegationDeliveryVerification.cs:10-135): the app side as the delegation checks see
//! it — the panes a check sets up, which take an item the way a pane does. A steer reaches a running pane's run; a queued item or a
//! started run starts a new run in a pane that is not running. Every hand-over is recorded with the delegation file as it was on disk at
//! that moment. A check may have the next hand-overs refused, hold a child's pane or a pane's state back behind a gate, and name a
//! child's first run itself. Owner: W7-A.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use mighty_delegation::host::{
    DelegationAgentAvailability, DelegationChildPane, DelegationHost, DelegationPaneActivity, DelegationPaneState,
};
use mighty_delegation::model::DeliveryRoute;
use mighty_delegation::store::{DelegationFile, DelegationFileStore};
use mighty_platform::BoxFuture;
use tokio::sync::watch;

/// One hand-over: by `route`, or a started run when None; `on_disk` the delegation file as it stood then; `run_id` the run that took it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Handed {
    pub session_id: String,
    pub route: Option<DeliveryRoute>,
    pub input: String,
    pub on_disk: Option<DelegationFile>,
    pub run_id: Option<String>,
}

/// A gate a check opens once (C#'s `TaskCompletionSource`).
#[derive(Clone)]
pub struct Gate(Arc<watch::Sender<bool>>);

impl Default for Gate {
    fn default() -> Self {
        Gate::new()
    }
}

impl Gate {
    pub fn new() -> Gate {
        Gate(Arc::new(watch::channel(false).0))
    }

    pub fn open(&self) {
        self.0.send_replace(true);
    }

    pub async fn wait(&self) {
        let mut opened = self.0.subscribe();
        let _ = opened.wait_for(|open| *open).await;
    }
}

type StateGate = Arc<dyn Fn(String) -> BoxFuture<'static, ()> + Send + Sync>;
type OnFirstRun = Arc<dyn Fn(String, String) -> BoxFuture<'static, ()> + Send + Sync>;
type FirstRun = Arc<dyn Fn(&str) -> String + Send + Sync>;

#[derive(Default)]
struct Log {
    panes: HashMap<String, DelegationPaneState>,
    handed: Vec<Handed>,
    created: Vec<DelegationChildPane>,
    first_runs: Vec<(String, String)>,
    stops: Vec<String>,
    calls: Vec<String>,
    notes: Vec<(String, String)>,
    refusals: VecDeque<Option<DelegationPaneActivity>>,
    started: usize,
    agents: HashMap<String, DelegationAgentAvailability>,
    asked: Vec<String>,
    first_run: Option<FirstRun>,
    pane_gate: Option<Gate>,
    state_gate: Option<StateGate>,
    on_first_run: Option<OnFirstRun>,
}

/// `DelegationTestHost(store, runs)`: `store` is the folder of the delegation file each hand-over reads back; started runs are named
/// `<runs>-<n>`.
pub struct TestHost {
    store: Option<PathBuf>,
    runs: String,
    log: Mutex<Log>,
    /// `MakesPanes`: CreatePane makes the child's pane.
    pub makes_panes: AtomicBool,
    pub pane_fails: AtomicBool,
    pub run_fails: AtomicBool,
}

impl TestHost {
    pub fn new(store: Option<PathBuf>, runs: &str) -> Arc<TestHost> {
        Arc::new(TestHost {
            store,
            runs: runs.to_string(),
            log: Mutex::new(Log::default()),
            makes_panes: AtomicBool::new(false),
            pane_fails: AtomicBool::new(false),
            run_fails: AtomicBool::new(false),
        })
    }

    fn log(&self) -> MutexGuard<'_, Log> {
        self.log.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn set_makes_panes(&self, value: bool) {
        self.makes_panes.store(value, Ordering::SeqCst);
    }

    /// `FirstRun`: names a child's first run; None hands a started run over as a pane takes any input.
    pub fn set_first_run(&self, name: Option<impl Fn(&str) -> String + Send + Sync + 'static>) {
        self.log().first_run = name.map(|name| Arc::new(name) as FirstRun);
    }

    /// `PaneGate`: awaited before a created pane answers.
    pub fn set_pane_gate(&self, gate: Option<Gate>) {
        self.log().pane_gate = gate;
    }

    /// `StateGate`: awaited before a pane's state is answered.
    pub fn set_state_gate(&self, gate: Option<impl Fn(String) -> BoxFuture<'static, ()> + Send + Sync + 'static>) {
        self.log().state_gate = gate.map(|gate| Arc::new(gate) as StateGate);
    }

    /// `OnFirstRun`: called with the child's id and its first run's id while the run is being started.
    pub fn set_on_first_run(&self, hook: Option<impl Fn(String, String) -> BoxFuture<'static, ()> + Send + Sync + 'static>) {
        self.log().on_first_run = hook.map(|hook| Arc::new(hook) as OnFirstRun);
    }

    /// `SetAgent`: what the host last learnt of `provider`'s CLI; anything not set is ready.
    pub fn set_agent(&self, provider: &str, availability: DelegationAgentAvailability) {
        self.log().agents.insert(provider.to_string(), availability);
    }

    /// `AvailabilityAsked`: each agent the coordinator asked about, in order.
    pub fn availability_asked(&self) -> Vec<String> {
        self.log().asked.clone()
    }

    /// `AllHanded`.
    pub fn all_handed(&self) -> Vec<Handed> {
        self.log().handed.clone()
    }

    /// `Taken`: the inputs that reached a pane.
    pub fn taken(&self) -> Vec<Handed> {
        self.all_handed().into_iter().filter(|handed| handed.run_id.is_some()).collect()
    }

    pub fn created_panes(&self) -> Vec<DelegationChildPane> {
        self.log().created.clone()
    }

    pub fn started_runs(&self) -> Vec<(String, String)> {
        self.log().first_runs.clone()
    }

    pub fn stops(&self) -> Vec<String> {
        self.log().stops.clone()
    }

    pub fn calls(&self) -> Vec<String> {
        self.log().calls.clone()
    }

    /// `Notes`: the app's notes in panes' transcripts, in order.
    pub fn notes(&self) -> Vec<(String, String)> {
        self.log().notes.clone()
    }

    pub fn set(&self, pane: DelegationPaneState) {
        self.log().panes.insert(pane.session_id.clone(), pane);
    }

    /// `Set(sessionId, activity, runId, provider, folder)`: a pane in "auto" mode, working in `folder` (the temp folder when None).
    pub fn set_pane(&self, session_id: &str, activity: DelegationPaneActivity, run_id: Option<&str>, provider: &str, folder: Option<&str>) {
        self.set(DelegationPaneState {
            session_id: session_id.to_string(),
            permission_mode: "auto".into(),
            folder: folder.map(str::to_string).unwrap_or_else(|| std::env::temp_dir().to_string_lossy().into_owned()),
            kind: "claude".into(),
            provider: provider.to_string(),
            parent_session_id: None,
            run_id: run_id.map(str::to_string),
            activity,
        });
    }

    /// `Set(sessionId, activity, runId)` for a Claude pane.
    pub fn set_claude(&self, session_id: &str, activity: DelegationPaneActivity, run_id: Option<&str>) {
        self.set_pane(session_id, activity, run_id, "claude", None);
    }

    pub fn close(&self, session_id: &str) {
        self.log().panes.remove(session_id);
    }

    pub fn pane(&self, session_id: &str) -> Option<DelegationPaneState> {
        self.log().panes.get(session_id).cloned()
    }

    /// `RefuseNext`: refuses the next hand-overs, one per entry; each leaves its pane in that activity, or as it is for None.
    pub fn refuse_next(&self, after: &[Option<DelegationPaneActivity>]) {
        self.log().refusals.extend(after.iter().copied());
    }

    fn hand(&self, input: String, session_id: String, route: Option<DeliveryRoute>) -> Option<String> {
        let on_disk = self.store.as_ref().and_then(|folder| DelegationFileStore::new(folder.clone()).load().ok());
        let mut log = self.log();
        let mut run_id = None;
        if let Some(after) = log.refusals.pop_front() {
            if let Some(activity) = after
                && let Some(refused) = log.panes.get_mut(&session_id)
            {
                refused.activity = activity;
            }
        } else if let Some(pane) = log.panes.get(&session_id).cloned() {
            if route == Some(DeliveryRoute::Steer) {
                run_id = if pane.activity == DelegationPaneActivity::Running { pane.run_id.clone() } else { None };
            } else if pane.activity != DelegationPaneActivity::Running {
                log.started += 1;
                let id = format!("{}-{}", self.runs, log.started);
                log.panes.insert(
                    session_id.clone(),
                    DelegationPaneState { run_id: Some(id.clone()), activity: DelegationPaneActivity::Running, ..pane },
                );
                run_id = Some(id);
            }
        }
        log.handed.push(Handed { session_id, route, input, on_disk, run_id: run_id.clone() });
        run_id
    }
}

impl DelegationHost for TestHost {
    fn create_pane(&self, pane: DelegationChildPane) -> BoxFuture<'_, bool> {
        Box::pin(async move {
            let gate = {
                let mut log = self.log();
                log.created.push(pane.clone());
                log.calls.push(format!("createPane {}", pane.session_id));
                log.pane_gate.clone()
            };
            if let Some(gate) = gate {
                gate.wait().await;
            }
            let mut log = self.log();
            if !self.makes_panes.load(Ordering::SeqCst) || self.pane_fails.load(Ordering::SeqCst) {
                return false;
            }
            log.panes.insert(
                pane.session_id.clone(),
                DelegationPaneState {
                    session_id: pane.session_id.clone(),
                    permission_mode: pane.mode.clone(),
                    folder: pane.folder.clone(),
                    kind: "claude".into(),
                    provider: pane.provider.clone(),
                    parent_session_id: Some(pane.parent_session_id.clone()),
                    run_id: None,
                    activity: DelegationPaneActivity::Idle,
                },
            );
            true
        })
    }

    fn start_run(&self, session_id: String, input: String) -> BoxFuture<'_, Option<String>> {
        Box::pin(async move {
            let (name, hook) = {
                let mut log = self.log();
                log.calls.push(format!("startRun {session_id}"));
                (log.first_run.clone(), log.on_first_run.clone())
            };
            let Some(name) = name else { return self.hand(input, session_id, None) };
            {
                let mut log = self.log();
                log.first_runs.push((session_id.clone(), input));
                if self.run_fails.load(Ordering::SeqCst) {
                    return None;
                }
            }
            let run_id = name(&session_id);
            if let Some(hook) = hook {
                hook(session_id, run_id.clone()).await;
            }
            Some(run_id)
        })
    }

    fn deliver(&self, input: String, session_id: String, route: DeliveryRoute) -> BoxFuture<'_, Option<String>> {
        Box::pin(async move {
            self.log().calls.push(format!("deliver {session_id}"));
            self.hand(input, session_id, Some(route))
        })
    }

    fn pane_state(&self, session_id: String) -> BoxFuture<'_, Option<DelegationPaneState>> {
        Box::pin(async move {
            let gate = {
                let mut log = self.log();
                log.calls.push(format!("paneState {session_id}"));
                log.state_gate.clone()
            };
            if let Some(gate) = gate {
                gate(session_id.clone()).await;
            }
            self.pane(&session_id)
        })
    }

    fn stop_run(&self, session_id: String) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let mut log = self.log();
            log.calls.push(format!("stopRun {session_id}"));
            let pane = log.panes.get(&session_id).cloned();
            let there = pane.as_ref().is_some_and(|pane| std::path::Path::new(&pane.folder).is_dir());
            log.stops.push(format!("{session_id} {} its folder", if there { "with" } else { "without" }));
            if let Some(pane) = pane {
                log.panes.insert(session_id, DelegationPaneState { activity: DelegationPaneActivity::Idle, ..pane });
            }
        })
    }

    fn note(&self, text: String, session_id: String) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            self.log().notes.push((session_id, text));
        })
    }

    fn agent_availability(&self, provider: String) -> BoxFuture<'_, DelegationAgentAvailability> {
        Box::pin(async move {
            let mut log = self.log();
            log.asked.push(provider.clone());
            log.agents.get(&provider).copied().unwrap_or_default()
        })
    }
}

// ── what the delegation checks share (C# `DelegationSupport`) ──────────────────────────────────────────────────────────────

/// `Caller(pane, kind, provider, delegation)`: a call from `pane`, its run started in no mode.
pub fn caller(pane: &str, kind: &str, provider: &str, delegation: bool) -> mighty_delegation::request::DelegationCaller {
    mighty_delegation::request::DelegationCaller {
        pane_id: pane.into(),
        kind: kind.into(),
        provider: provider.into(),
        delegation,
        permission_mode: String::new(),
    }
}

/// `Request(tool, arguments)`.
pub fn request(tool: &str, arguments: &[(&str, &str)]) -> mighty_delegation::request::DelegationRequest {
    mighty_delegation::request::DelegationRequest {
        tool: tool.into(),
        arguments: arguments.iter().map(|(name, value)| (name.to_string(), value.to_string())).collect(),
    }
}

/// `Verification.Temp()`: a new empty folder of this check's own (removed by the check with [`super::place::remove_folder`]).
pub fn temp_folder(name: &str) -> PathBuf {
    super::place::temp(name)
}

/// `Eventually`: polls `done` every 10 ms for up to `milliseconds`, then once more.
pub async fn eventually(milliseconds: u64, mut done: impl FnMut() -> bool) -> bool {
    let end = std::time::Instant::now() + std::time::Duration::from_millis(milliseconds);
    while std::time::Instant::now() < end {
        if done() {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    done()
}

/// `EventuallyAsync`: [`eventually`] for a condition that awaits.
pub async fn eventually_async<F: std::future::Future<Output = bool>>(milliseconds: u64, mut done: impl FnMut() -> F) -> bool {
    let end = std::time::Instant::now() + std::time::Duration::from_millis(milliseconds);
    while std::time::Instant::now() < end {
        if done().await {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    done().await
}

static LANGUAGE: Mutex<&str> = Mutex::new("en");

/// The language the checks' coordinators speak (C#'s `Locale.LanguagePreference`); English unless a check sets another.
pub fn set_language(language: &'static str) {
    *LANGUAGE.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = language;
}

/// `Locale.Get` in the checks' language.
pub fn locale() -> mighty_core::locale::Locale {
    mighty_core::locale::Locale::new(&LANGUAGE.lock().unwrap_or_else(|poisoned| poisoned.into_inner()))
}

/// `new DelegationCoordinator(store, host, maker, isSwitchOn)` with C#'s 45 s answer and the checks' language.
pub fn coordinator(
    store: DelegationFileStore,
    host: Arc<TestHost>,
    maker: mighty_delegation::git::worktree::Maker,
    switch_on: impl Fn() -> bool + Send + Sync + 'static,
) -> Result<mighty_delegation::coordinator::Coordinator, mighty_delegation::store::DelegationFileError> {
    let options = mighty_delegation::coordinator::CoordinatorOptions {
        worktrees: maker,
        paths: super::place::paths(),
        switch_on: Arc::new(switch_on),
        answer_seconds: mighty_delegation::coordinator::ANSWER_SECONDS,
        locale: Arc::new(locale),
    };
    mighty_delegation::coordinator::Coordinator::new(store, host, options)
}

/// A maker under `root` with `free` bytes free, its git the place's (no git runs unless a check makes a child).
pub fn maker(place: &super::place::DelegationPlace, free: u64) -> mighty_delegation::git::worktree::Maker {
    mighty_delegation::git::worktree::Maker::new(&place.root(), place.git(), Arc::new(mighty_platform::fake::FixedDiskSpace(free)))
}

/// `Record(id, state, parent, runId, followUps, revision, head)`: a child record that only needs to exist — no worktree behind it.
pub fn record(id: &str, state: mighty_delegation::model::ChildState, parent: &str) -> mighty_delegation::model::ChildRecord {
    let worktree = std::env::temp_dir().join("nonexistent-worktrees").join(id);
    mighty_delegation::model::ChildRecord::make_with(
        id,
        parent,
        &worktree.to_string_lossy(),
        "main",
        &"a".repeat(40),
        "plan",
        &format!("key-{id}"),
        mighty_delegation::model::ChildRecordOptions { state: Some(state), ..Default::default() },
    )
}

/// The record C#'s `ChildRecord.Make(id, "parent", …)` is of a child the place made (`PlaceChild`), with its report head and state.
pub fn record_of(child: &super::place::PlaceChild) -> mighty_delegation::model::ChildRecord {
    mighty_delegation::model::ChildRecord {
        report_head: child.report_head.clone(),
        report_revision: i32::from(child.report_head.is_some()),
        ..mighty_delegation::model::ChildRecord::make_with(
            &child.id,
            &child.parent_session_id,
            &child.worktree_path,
            &child.parent_branch,
            &child.base_commit,
            "acceptEdits",
            &format!("key-{}", child.id),
            mighty_delegation::model::ChildRecordOptions {
                state: Some(child.state),
                parent_checkout: child.parent_checkout.clone(),
                ..Default::default()
            },
        )
    }
}

/// `DelegationCopy.Of` of `text` (a report at revision 1 unless named, a task at 0).
pub fn copy(
    child: &str,
    kind: mighty_delegation::copy::DelegationCopyKind,
    text: &str,
    revision: Option<i64>,
) -> mighty_delegation::copy::DelegationCopy {
    let revision = revision.unwrap_or(if kind == mighty_delegation::copy::DelegationCopyKind::Report { 1 } else { 0 });
    mighty_delegation::copy::DelegationCopy::of(child, kind, revision, text.as_bytes(), None)
}
