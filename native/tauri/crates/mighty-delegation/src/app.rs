//! The desktop service's delegation (C# Core/DesktopService.Delegation.cs; DesktopService.cs:129-155, 189, 261-275, 346-351): the app
//! side of parent → child delegation over an `Arc<mighty_run::RunService>` (the state's owner), the coordinator and the window's part
//! ([`DelegationWindow`], C#'s `DelegatedRunBlocked`, `DelegatedRunRequest`, `EndAgentTerminal`, `AgentAvailabilityNow`,
//! `DelegationChanged`, `DelegationNoted`, `QueuedMergeDone`); no Tauri, so the end-to-end scenarios run headless in `cargo test`. It
//! is the coordinator's [`DelegationHost`] (it makes panes, starts and steers runs, reports what its panes do) and the run service's
//! [`DelegationRunHooks`] (the run ledger, each child's run starts and ends handed to the coordinator in order, a closed child's
//! cleanup), and offers the window its entry points: the second send and the worktree menu, the human's child, the send and run next
//! with held items, the close of a pane, the workspace's removal, the card actions with the merge queue. src-tauri delegation.rs drives
//! it. Owner: W7-D.
//!
//! Concurrency: the run service's lock is never taken while one of this module's own locks is held, and no lock is held across an
//! await (the crate denies `await_holding_lock`). The hooks the service calls under its lock take only this module's own locks.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, Weak};
use std::time::{Duration, Instant};

use mighty_composer::queue::{self, ComposerSecondSend, notice_turn_running};
use mighty_core::attachment::RunAttachment;
use mighty_core::locale::Locale;
use mighty_core::model::{AppSnapshot, LogEntry, RunSession};
use mighty_core::shell::{ChildCardState, DelegationChildRow, SidebarChild};
use mighty_core::{time, wire};
use mighty_platform::BoxFuture;
use mighty_run::RunError;
use mighty_run::model::{RunEvent, StartRunRequest};
use mighty_run::service::{DelegationRunHooks, RunService};
use tokio::sync::watch;

use crate::app_host::{self, DelegationRunEvent, DelegationRunEventPump, DelegationRunLedger};
use crate::cards::{
    ChildCardDiscardOutcome, DelegationCardAction, DelegationCardNote, DelegationCardResult, DelegationMergeStep, DelegationMergeWait,
    PendingChildDiscard, PendingParentClose, merge_step, wait_after,
};
use crate::coordinator::{ANSWER_SECONDS, Coordinator, CoordinatorOptions};
use crate::copy::DelegationCopyKind;
use crate::delivery::{DelegationDeliveryItem, DelegationRelease};
use crate::git::cleanup::ChildCleanupOutcome;
use crate::git::merge::{ChildMergeOutcome, ChildUndoOutcome};
use crate::git::paths::{self, DelegationPaths};
use crate::git::worktree::Maker;
use crate::host::{DelegationAgentAvailability, DelegationChildPane, DelegationHost, DelegationPaneActivity, DelegationPaneState};
use crate::model::{ChildState, DeliveryRoute};
use crate::providers::{self, DelegationChildChoice};
use crate::reports::ChildRunEnd;
use crate::request::DelegationReasonCode;
use crate::store::DelegationFileStore;
use crate::tools::delegate::{HumanChildFirstRun, HumanChildStart, HumanChildStarter};
use crate::{sidebar, text};

/// How long a stop waits for the pane's run to end (C# `StopAndWait`).
const STOP_WAIT: Duration = Duration::from_secs(10);
/// C# `MergeRetryInterval`'s default.
pub const MERGE_RETRY: Duration = Duration::from_secs(10);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The window's part of delegation (C# MainWindow.Delegation.cs `InitializeDelegation`: the service's `Func`s and events). Each is
/// called from a task of the app, never under one of its locks or the run service's.
pub trait DelegationWindow: Send + Sync {
    /// C# `DelegatedRunBlocked`: why the window would refuse a run in the pane now, or None when one may start.
    fn run_blocked(&self, pane: &RunSession) -> Option<String> {
        let _ = pane;
        None
    }
    /// C# `DelegatedRunRequest`: the request of a run delegation starts on its own, finished the way the window finishes its own sends.
    fn run_request(&self, request: StartRunRequest) -> BoxFuture<'static, StartRunRequest> {
        Box::pin(async move { request })
    }
    /// C# `EndAgentTerminal`: ends the pane's agent terminal and everything it started, and returns once they have exited.
    fn end_agent_terminal(&self, session_id: String) -> BoxFuture<'static, ()> {
        let _ = session_id;
        Box::pin(async {})
    }
    /// C# `AgentAvailabilityNow`: whether an agent's CLI can run a child, as the window last learnt it; never by asking a CLI.
    fn agent_availability(&self, provider: &str) -> DelegationAgentAvailability {
        let _ = provider;
        DelegationAgentAvailability::Ready
    }
    /// Whether delegation acts (W7 plan D2: `--allow-state-write`, or a smoke): when not, the tree and held rows are shown and nothing
    /// more — no child's run reaches the coordinator, nothing is delivered, a closed child's cleanup does nothing, Ctrl+Enter queues.
    fn live(&self) -> bool {
        true
    }
    /// The state changed through the run service (a pane made, a note): the window's copy follows.
    fn state_changed(&self) {}
    /// C# `DelegationChanged`: something delegation shows changed.
    fn changed(&self) {}
    /// C# `DelegationNoted`: delegation said something in the pane's transcript.
    fn noted(&self, session_id: &str) {
        let _ = session_id;
    }
    /// C# `QueuedMergeDone`: a merge queued from a child's card went through by itself; the window offers to discard the child.
    fn queued_merge_done(&self, child_id: &str) {
        let _ = child_id;
    }
}

/// What delegation is made of at its start (C# `DesktopService`'s delegation arguments and `StartDelegation`'s store).
#[derive(Clone)]
pub struct DelegationStart {
    /// The folder of `delegation-state.json` (the profile's; a read-only launch's copy).
    pub store: PathBuf,
    pub worktrees: Maker,
    pub paths: DelegationPaths,
    /// The hidden switch, read at each call.
    pub switch_on: Arc<dyn Fn() -> bool + Send + Sync>,
    pub locale: Arc<dyn Fn() -> Locale + Send + Sync>,
    /// The tools' answer time (at most 45 s).
    pub answer_seconds: f64,
    /// C# `MergeRetryInterval`.
    pub merge_retry: Duration,
}

impl DelegationStart {
    pub fn new(
        store: PathBuf,
        worktrees: Maker,
        paths: DelegationPaths,
        switch_on: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> DelegationStart {
        DelegationStart {
            store,
            worktrees,
            paths,
            switch_on,
            locale: Arc::new(mighty_run::strings::locale),
            answer_seconds: ANSWER_SECONDS,
            merge_retry: MERGE_RETRY,
        }
    }
}

/// The coordinator's host, the app it reaches without keeping it alive.
struct HostLink(Weak<DelegationApp>);

impl DelegationHost for HostLink {
    fn create_pane(&self, pane: DelegationChildPane) -> BoxFuture<'_, bool> {
        let app = self.0.upgrade();
        Box::pin(async move { if let Some(app) = app { app.create_pane_now(pane).await } else { false } })
    }
    fn start_run(&self, session_id: String, input: String) -> BoxFuture<'_, Option<String>> {
        let app = self.0.upgrade();
        Box::pin(async move { app?.start_delegated_run(&session_id, &input, None).await })
    }
    fn deliver(&self, input: String, session_id: String, route: DeliveryRoute) -> BoxFuture<'_, Option<String>> {
        let app = self.0.upgrade();
        Box::pin(async move { app?.deliver_now(input, session_id, route).await })
    }
    fn pane_state(&self, session_id: String) -> BoxFuture<'_, Option<DelegationPaneState>> {
        let app = self.0.upgrade();
        Box::pin(async move { app?.pane_state_now(&session_id) })
    }
    fn stop_run(&self, session_id: String) -> BoxFuture<'_, ()> {
        let app = self.0.upgrade();
        Box::pin(async move {
            if let Some(app) = app {
                app.stop_run(session_id).await;
            }
        })
    }
    fn note(&self, text: String, session_id: String) -> BoxFuture<'_, ()> {
        if let Some(app) = self.0.upgrade() {
            app.note_now(text, session_id);
        }
        Box::pin(async {})
    }
    fn agent_availability(&self, provider: String) -> BoxFuture<'_, DelegationAgentAvailability> {
        let availability = self.0.upgrade().map_or(DelegationAgentAvailability::Ready, |app| app.window.agent_availability(&provider));
        Box::pin(async move { availability })
    }
}

/// What the app keeps of its cards, its merge queue, its closes and its human children; one lock, never held across an await.
#[derive(Default)]
struct Cards {
    busy: HashSet<String>,
    notes: HashMap<String, DelegationCardNote>,
    /// C# `mergeQueue`.
    queue: HashSet<String>,
    /// C# `mergeRetrying`.
    retrying: HashSet<String>,
    /// C# `mergeRetry`: the slow retry's stop, while it runs.
    retry: Option<watch::Sender<bool>>,
    /// C# `childCloses`: what closing each delegated child's pane did, None while it goes.
    closes: HashMap<String, watch::Receiver<Option<ChildCleanupOutcome>>>,
    /// C# `followingNotes`.
    following_notes: HashMap<String, String>,
    /// C# `quietChildParents`.
    quiet_child_parents: HashMap<String, HashSet<String>>,
    /// C# `worktreeReadiness`.
    readiness: HashMap<String, bool>,
    /// C# `worktreeReadinessChecks`.
    readiness_checks: HashSet<String>,
}

/// The coordinator and what runs with it (C# `delegation`, `delegationRunEvents`, `delegationWatch`).
struct Loaded {
    coordinator: Coordinator,
    pump: Arc<DelegationRunEventPump>,
    watch_stop: watch::Sender<bool>,
    watches: Vec<tokio::task::JoinHandle<()>>,
}

/// The desktop service's delegation (C# `DesktopService`'s delegation half).
pub struct DelegationApp {
    me: Weak<DelegationApp>,
    service: OnceLock<Arc<RunService>>,
    runtime: tokio::runtime::Handle,
    window: Arc<dyn DelegationWindow>,
    /// C# `delegationRuns`.
    runs: DelegationRunLedger,
    /// C# `childRuns`.
    child_runs: Mutex<HashSet<String>>,
    loaded: Mutex<Option<Loaded>>,
    start: Mutex<Option<DelegationStart>>,
    cards: Mutex<Cards>,
}

impl DelegationApp {
    /// An app on `runtime` (where its tasks run), with no coordinator until [`DelegationApp::start`].
    pub fn new(runtime: tokio::runtime::Handle, window: Arc<dyn DelegationWindow>) -> Arc<DelegationApp> {
        Arc::new_cyclic(|me| DelegationApp {
            me: me.clone(),
            service: OnceLock::new(),
            runtime,
            window,
            runs: DelegationRunLedger::new(),
            child_runs: Mutex::new(HashSet::new()),
            loaded: Mutex::new(None),
            start: Mutex::new(None),
            cards: Mutex::new(Cards::default()),
        })
    }

    /// The run service this app was made the hooks of; set once.
    pub fn attach(&self, service: Arc<RunService>) {
        let _ = self.service.set(service);
    }

    pub fn service(&self) -> Option<&Arc<RunService>> {
        self.service.get()
    }

    fn snapshot(&self) -> AppSnapshot {
        self.service().map(|service| service.snapshot()).unwrap_or_default()
    }

    /// C# `closing` (or no service yet).
    fn closing(&self) -> bool {
        self.service().is_none_or(|service| service.closing())
    }

    /// C# `Delegation`: the coordinator, None when the delegation file could not be read (delegation is unavailable this launch).
    pub fn coordinator(&self) -> Option<Coordinator> {
        lock(&self.loaded).as_ref().map(|loaded| loaded.coordinator.clone())
    }

    fn locale(&self) -> Locale {
        lock(&self.start).as_ref().map_or_else(|| Locale::new("en"), |start| (start.locale)())
    }

    fn switch_on(&self) -> bool {
        lock(&self.start).as_ref().is_some_and(|start| (start.switch_on)())
    }

    // ── the start, the restart, the stop (C# StartDelegation, RestartDelegationAsync, StopDelegation) ──

    /// C# `StartDelegation`, then `InterruptRunsKilledByQuit` (DesktopService.cs:129-140): the coordinator made from the delegation
    /// file whether or not the switch is on; an unreadable file leaves delegation unavailable this launch (false). Children whose runs
    /// quitting killed come back interrupted, before any pane runs again.
    pub async fn start(&self, start: DelegationStart) -> bool {
        self.begin(start) && {
            self.interrupt_runs_killed_by_quit().await;
            true
        }
    }

    /// C# `StartDelegation` alone, synchronously, so no run the app starts after it goes unrecorded: the coordinator made from the
    /// delegation file; false when it cannot be read.
    pub fn begin(&self, start: DelegationStart) -> bool {
        *lock(&self.start) = Some(start);
        self.load()
    }

    /// C# `InterruptRunsKilledByQuit` (DesktopService.cs:136-138): children whose runs quitting killed come back interrupted, before any
    /// pane runs again. Nothing in a read-only launch (W7 plan D2).
    pub async fn interrupt_runs_killed_by_quit(&self) {
        if self.window.live()
            && let Some(coordinator) = self.coordinator()
        {
            coordinator.interrupt_runs_killed_by_quit().await;
        }
    }

    fn load(&self) -> bool {
        if lock(&self.loaded).is_some() {
            return true;
        }
        let Some(start) = lock(&self.start).clone() else { return false };
        let options = CoordinatorOptions {
            worktrees: start.worktrees.clone(),
            paths: start.paths.clone(),
            switch_on: start.switch_on.clone(),
            answer_seconds: start.answer_seconds,
            locale: start.locale.clone(),
        };
        let host: Arc<dyn DelegationHost> = Arc::new(HostLink(self.me.clone()));
        let Ok(coordinator) = Coordinator::new(DelegationFileStore::new(&start.store), host, options) else { return false };
        // A child's end is recorded (the app's commit of its work among it) before its queue or delivery may start its next run.
        let (me, receiving) = (self.me.clone(), coordinator.clone());
        let pump = Arc::new(DelegationRunEventPump::new(
            &self.runtime,
            Arc::new(move |change| {
                let (me, coordinator) = (me.clone(), receiving.clone());
                Box::pin(async move {
                    if let Some(app) = me.upgrade() {
                        app.hand_on(&coordinator, change).await;
                    }
                })
            }),
        ));
        // The sidebar's tree and the held rows of each pane's queued list follow the file, switch on or off.
        let (watch_stop, _) = watch::channel(false);
        let mut watches = vec![];
        let mut rows = coordinator.child_rows();
        let mut held = coordinator.held_rows();
        for which in 0..2 {
            let mut stop = watch_stop.subscribe();
            let me = self.me.clone();
            let (mut rows, mut held) = (rows.clone(), held.clone());
            watches.push(self.runtime.spawn(async move {
                loop {
                    let changed = tokio::select! {
                        _ = stop.changed() => false,
                        changed = async { if which == 0 { rows.changed().await } else { held.changed().await } } => changed.is_ok(),
                    };
                    let Some(app) = me.upgrade().filter(|_| changed) else { return };
                    app.window.changed();
                    app.kick_queued_merges();
                }
            }));
        }
        rows.mark_unchanged();
        held.mark_unchanged();
        *lock(&self.loaded) = Some(Loaded { coordinator, pump, watch_stop, watches });
        self.refresh_running_worktree_readiness();
        true
    }

    /// Hands one run event of a child to the coordinator (C# `ChildRunStarted`, `ChildRunEndRecorded`).
    async fn hand_on(&self, coordinator: &Coordinator, change: DelegationRunEvent) {
        match change {
            DelegationRunEvent::Started { child_id, run_id } => coordinator.child_run_started(&child_id, &run_id).await,
            DelegationRunEvent::Ended { child_id, run_id, end } => {
                coordinator.child_run_ended(&child_id, &run_id, end).await;
                self.window.changed();
                // The note that follows a slash command the human's child ran first, after a normal finish.
                let note = {
                    let mut cards = lock(&self.cards);
                    cards.following_notes.remove(&child_id)
                }
                .filter(|_| end == ChildRunEnd::Finished && !self.closing() && !coordinator.ending_runs().contains(&child_id));
                if let Some(note) = note {
                    self.start_delegated_run(&child_id, &note, None).await;
                }
            }
        }
    }

    /// For the delegation smoke only (C# `RestartDelegationAsync`): drops the coordinator and makes it again from the delegation file
    /// as it is on disk now. No run of a child may be going.
    pub async fn restart(&self) -> bool {
        self.unload().await;
        let loaded = self.load();
        self.window.changed();
        loaded
    }

    async fn unload(&self) {
        let loaded = lock(&self.loaded).take();
        if let Some(loaded) = loaded {
            loaded.pump.finish().await;
            let _ = loaded.watch_stop.send(true);
            for watch in loaded.watches {
                let _ = watch.await;
            }
        }
    }

    /// C# `StopDelegation`: quitting, once every run has ended: the run events handed on, the children's closes over, the merge retry
    /// and the watches stopped.
    pub async fn stop(&self) {
        let pump = lock(&self.loaded).as_ref().map(|loaded| loaded.pump.clone());
        if let Some(pump) = pump {
            pump.finish().await;
        }
        let closes: Vec<_> = lock(&self.cards).closes.values().cloned().collect();
        for mut close in closes {
            let _ = close.wait_for(Option::is_some).await;
        }
        if let Some(retry) = lock(&self.cards).retry.take() {
            let _ = retry.send(true);
        }
        self.unload().await;
    }

    // ── the coordinator's host ──

    /// C# `IDelegationHost.CreatePane`: the child's pane as a tab beside its parent, titled by its task.
    async fn create_pane_now(&self, pane: DelegationChildPane) -> bool {
        let task = self
            .coordinator()
            .and_then(|coordinator| coordinator.file().copy(&pane.session_id, DelegationCopyKind::Task).map(|copy| copy.text.clone()));
        let Some(service) = self.service().cloned() else { return false };
        if service.closing() {
            return false;
        }
        // The phone's 바로 실행 child never moves this window's view.
        let quiet = lock(&self.cards).quiet_child_parents.get(&pane.parent_session_id).is_some_and(|tokens| !tokens.is_empty());
        // A save the disk refuses is reported as any other; the pane is open all the same.
        let made = service.try_update(|state| app_host::adding_child(state, &pane, task.as_deref(), quiet));
        if !matches!(made, Ok(true)) {
            return false;
        }
        self.window.state_changed();
        self.window.changed();
        service.snapshot().session(&pane.session_id).is_some()
    }

    /// C# `StartDelegatedRun`: a run with `input` in the pane, the way a send starts one: its id once the run's process has it, or None
    /// when the pane cannot run now or its start failed or was stopped. Its words are delegation's, so they never title the pane.
    pub async fn start_delegated_run(&self, session_id: &str, input: &str, attachments: Option<Vec<RunAttachment>>) -> Option<String> {
        let service = self.service()?.clone();
        if service.closing() {
            return None;
        }
        let pane = service.snapshot().session(session_id).cloned()?;
        if self.coordinator().is_some_and(|coordinator| coordinator.ending_runs().contains(session_id))
            || self.window.run_blocked(&pane).is_some()
        {
            return None;
        }
        let mut request = StartRunRequest::new(pane.id(), pane.workspace_id(), pane.kind(), input);
        request.model = pane.model().to_string();
        request.provider = pane.provider().to_string();
        request.settings = pane.settings.clone();
        request.resume_id = pane.resume_id.clone();
        request.attachments = attachments.filter(|files| !files.is_empty());
        let request = self.window.run_request(request).await;
        service.launch(request, Some(""), true).await.ok().flatten()
    }

    /// C# `IDelegationHost.Deliver`: steered into the pane's running Claude run and shown in its transcript, or started as its next run.
    async fn deliver_now(&self, input: String, session_id: String, route: DeliveryRoute) -> Option<String> {
        if route == DeliveryRoute::Queue {
            return self.start_delegated_run(&session_id, &input, None).await;
        }
        let service = self.service()?.clone();
        // Claude answering finished background work takes no steer: what comes now waits for that turn instead.
        let steerable = !service.closing()
            && service.snapshot().session(&session_id).is_some_and(|pane| {
                pane.status() == "running" && providers::can_steer(pane.kind(), pane.provider()) && !notice_turn_running(pane.graph_runs())
            })
            && self.runs.run_id(&session_id).is_some();
        if !steerable || !service.try_steer(&session_id, &input) {
            return None;
        }
        // The receipt names the run that took it, read once it has.
        self.runs.run_id(&session_id)
    }

    /// C# `IDelegationHost.PaneState`: one idle after a normal finish whose start would be refused now reads as idle; a remote
    /// workspace's pane is unknown to delegation.
    fn pane_state_now(&self, session_id: &str) -> Option<DelegationPaneState> {
        let service = self.service()?;
        let snapshot = service.snapshot();
        let pane = snapshot.session(session_id)?;
        let workspace = snapshot.workspace(pane.workspace_id()).filter(|workspace| workspace.remote.is_none())?;
        let activity = self.runs.activity(session_id, pane.status() == "running" || service.has_route(session_id));
        let run_id = self.runs.run_id(session_id);
        let refused = activity == DelegationPaneActivity::Finished
            && (service.closing() || app_host::run_refusal(pane).is_some() || self.window.run_blocked(pane).is_some() || !runs(pane));
        Some(app_host::pane_state(pane, workspace, run_id.as_deref(), activity, refused))
    }

    /// C# `IDelegationHost.Note`: the app's note in the pane's transcript, as a system line. Nothing for a closed pane.
    fn note_now(&self, text: String, session_id: String) {
        let Some(service) = self.service().cloned() else { return };
        let added = service.try_update(|state| {
            state.session(&session_id)?;
            let mut next = state.clone();
            for pane in next.sessions_mut().iter_mut().filter(|pane| pane.id() == session_id) {
                let mut noted = (**pane).clone();
                let entry = LogEntry::new(wire::new_id(), "system", text.clone(), time::now_o(), pane.provider.clone());
                noted.logs.get_or_insert_with(Vec::new).push(entry);
                *pane = Arc::new(noted);
            }
            Some(next)
        });
        if matches!(added, Ok(true)) {
            self.window.state_changed();
            self.window.noted(&session_id);
        }
    }

    /// C# `StopAndWait`: stops the pane's run, if one is going, and returns once it has ended. False when it had not ended after ten
    /// seconds.
    pub async fn stop_and_wait(&self, session_id: &str) -> bool {
        let Some(service) = self.service().cloned() else { return true };
        if !service.has_route(session_id) {
            return true;
        }
        let deadline = Instant::now() + STOP_WAIT;
        if let Err(RunError::TimedOut(_)) = service.stop(session_id).await {
            return false;
        }
        while service.has_route(session_id) {
            if Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        true
    }

    /// C# `RunId(sessionId)`: the pane's run going now, or its last since this launch.
    pub fn run_id(&self, session_id: &str) -> Option<String> {
        self.runs.run_id(session_id)
    }

    /// C# `OfferDelegationPending`: delegation offers its pending notices and follow-ups to their panes as they are now.
    pub fn offer_pending(&self) {
        if self.closing() || !self.window.live() {
            return;
        }
        if let Some(coordinator) = self.coordinator() {
            self.runtime.spawn(async move { coordinator.deliver_pending().await });
        }
    }

    // ── the composer's new worktree (C# :357-477) ──

    /// C# `WorktreeHostFolder`: the folder a top-level agent pane's runs work in; None for a remote workspace's pane.
    fn worktree_host_folder(&self, snapshot: &AppSnapshot, session: &RunSession) -> Option<String> {
        let workspace = snapshot.workspace(session.workspace_id()).filter(|workspace| workspace.remote.is_none())?;
        Some(app_host::run_workspace(session, workspace).path().to_string())
    }

    fn is_delegated_child(&self, session: &RunSession) -> bool {
        session.parent_session_id.is_some()
            || self.coordinator().is_some_and(|coordinator| coordinator.file().children.iter().any(|child| child.id == session.id()))
    }

    fn can_host_worktree(&self, snapshot: &AppSnapshot, session: &RunSession) -> bool {
        let Some(folder) = self.worktree_host_folder(snapshot, session) else { return false };
        lock(&self.cards).readiness.get(&folder).copied().unwrap_or(false)
    }

    fn remote(snapshot: &AppSnapshot, session: &RunSession) -> bool {
        snapshot.workspace(session.workspace_id()).is_some_and(|workspace| workspace.remote.is_some())
    }

    /// C# `SecondSend`: a new child worktree for a pane that may have children (delegation on, loaded and live, its folder last checked
    /// able to host one), otherwise the queue.
    pub fn second_send(&self, snapshot: &AppSnapshot, session: &RunSession) -> ComposerSecondSend {
        let mode = session.settings.as_ref().map(|s| s.permission_mode.clone()).unwrap_or_default();
        let on = self.coordinator().is_some() && self.window.live() && self.switch_on() && !Self::remote(snapshot, session);
        queue::choose_second_send(
            session.kind(),
            session.provider(),
            &mode,
            self.is_delegated_child(session),
            on,
            self.can_host_worktree(snapshot, session),
        )
    }

    /// C# `WorktreeChildChoices`: Claude, Codex and Antigravity, each with why it cannot run the text now.
    pub fn worktree_child_choices(&self, session: &RunSession, plan_first: bool) -> Vec<DelegationChildChoice> {
        let mode = session.settings.as_ref().map(|s| s.permission_mode.clone()).unwrap_or_default();
        let open = sidebar::open_human_child_count(session.id(), &self.rows());
        providers::human_child_choices(session.provider(), &mode, plan_first, open, |provider| self.window.agent_availability(provider))
    }

    /// C# `WorktreeMenuOnly`: the busy pane shows the worktree button for its menu alone.
    pub fn worktree_menu_only(&self, snapshot: &AppSnapshot, session: &RunSession, plan_first: bool) -> bool {
        let mode = session.settings.as_ref().map(|s| s.permission_mode.clone()).unwrap_or_default();
        self.coordinator().is_some()
            && self.window.live()
            && self.switch_on()
            && !self.is_delegated_child(session)
            && !queue::can_parent_by_human(session.kind(), session.provider(), &mode)
            && queue::menu_only(
                session.kind(),
                session.provider(),
                &mode,
                false,
                true,
                self.can_host_worktree(snapshot, session),
                self.worktree_child_choices(session, plan_first).iter().any(|choice| choice.blocked.is_none()),
            )
    }

    /// C# `OffersWorktreeMenu`.
    pub fn offers_worktree_menu(&self, snapshot: &AppSnapshot, session: &RunSession, plan_first: bool) -> bool {
        self.second_send(snapshot, session) == ComposerSecondSend::Worktree || self.worktree_menu_only(snapshot, session, plan_first)
    }

    /// C# `RefreshWorktreeReadiness`: checks again whether the pane's folder can host a child worktree; one check per folder at a time,
    /// a changed answer redraws.
    pub fn refresh_worktree_readiness(&self, session: &RunSession) {
        if !self.window.live() {
            return;
        }
        let Some(coordinator) = self.coordinator() else { return };
        let mode = session.settings.as_ref().map(|s| s.permission_mode.clone()).unwrap_or_default();
        if session.parent_session_id.is_some()
            || !providers::may_have_human_child(session.kind(), session.provider(), &mode)
            || !self.switch_on()
        {
            return;
        }
        let Some(folder) = self.worktree_host_folder(&self.snapshot(), session) else { return };
        if !lock(&self.cards).readiness_checks.insert(folder.clone()) {
            return;
        }
        let me = self.me.clone();
        let runtime = self.runtime.clone();
        self.runtime.spawn(async move {
            let checking = folder.clone();
            let check = runtime.spawn(async move { coordinator.worktrees().check(&checking).await.can_host_worktree() });
            let ready = check.await.unwrap_or(false);
            let Some(app) = me.upgrade() else { return };
            let changed = {
                let mut cards = lock(&app.cards);
                cards.readiness_checks.remove(&folder);
                cards.readiness.insert(folder, ready) != Some(ready)
            };
            if changed {
                app.window.changed();
            }
        });
    }

    /// C# `RefreshRunningWorktreeReadiness`: for every pane running now.
    pub fn refresh_running_worktree_readiness(&self) {
        let snapshot = self.snapshot();
        for session in snapshot.sessions().iter().filter(|session| session.status() == "running") {
            self.refresh_worktree_readiness(session);
        }
    }

    /// C# `StartHumanChildAsync`: the text and its attachments run now in a new child pane of the busy pane `parent_id`, in its own
    /// worktree, made by delegation as delegate makes one. `failed` hears a child whose worktree, pane or first run could not be made
    /// after the answer. `quiet`: the phone's 바로 실행, never selected here.
    #[allow(clippy::too_many_arguments, reason = "C#'s StartHumanChildAsync, one argument each")]
    pub async fn start_human_child(
        &self,
        parent_id: &str,
        text: &str,
        attachments: Vec<RunAttachment>,
        plan_first: bool,
        failed: Arc<dyn Fn(String) -> BoxFuture<'static, ()> + Send + Sync>,
        agent: Option<&str>,
        quiet: bool,
    ) -> HumanChildStart {
        let Some(coordinator) = self.coordinator() else {
            return HumanChildStart::Failed(self.locale().get("delegation.human.error.switchedOff"));
        };
        // A remote workspace's pane has no children: its folder is on its host.
        let snapshot = self.snapshot();
        let remote = snapshot
            .session(parent_id)
            .and_then(|parent| snapshot.workspace(parent.workspace_id()))
            .and_then(|workspace| workspace.remote.as_ref())
            .map(|reference| reference.host_name.clone().unwrap_or_default());
        if let Some(host) = remote {
            return HumanChildStart::Failed(self.locale().get_with("remote.pane.agentNote", &[("host", &host)]));
        }
        let token = quiet.then(wire::new_id);
        if let Some(token) = &token {
            lock(&self.cards).quiet_child_parents.entry(parent_id.to_string()).or_default().insert(token.clone());
        }
        let (me, started_token, parent) = (self.me.clone(), token.clone(), parent_id.to_string());
        let (failed_me, failed_token, failed_parent) = (self.me.clone(), token.clone(), parent_id.to_string());
        let files = Arc::new(attachments);
        let starter = HumanChildStarter {
            start: Arc::new(move |child_id: String, run: HumanChildFirstRun| {
                let (me, token, parent, files) = (me.clone(), started_token.clone(), parent.clone(), files.clone());
                Box::pin(async move {
                    let app = me.upgrade()?;
                    app.end_quiet_child(&parent, token.as_deref());
                    // Kept before the run starts, so a command that ends at once still finds its note.
                    if let Some(note) = &run.following_note {
                        lock(&app.cards).following_notes.insert(child_id.clone(), note.clone());
                    }
                    let run_id = app.start_delegated_run(&child_id, &run.input, Some(files.as_ref().clone())).await;
                    if run_id.is_none() {
                        lock(&app.cards).following_notes.remove(&child_id);
                    }
                    run_id
                })
            }),
            failed: Arc::new(move |child_id: String| {
                let (me, token, parent, failed) = (failed_me.clone(), failed_token.clone(), failed_parent.clone(), failed.clone());
                Box::pin(async move {
                    if let Some(app) = me.upgrade() {
                        app.end_quiet_child(&parent, token.as_deref());
                    }
                    failed(child_id).await;
                })
            }),
        };
        let answer = coordinator.start_human_child(text, parent_id, starter, plan_first, agent).await;
        // A refused or failed answer made no child: nothing waits for its pane.
        if !matches!(answer, HumanChildStart::Started(_)) {
            self.end_quiet_child(parent_id, token.as_deref());
        }
        answer
    }

    /// C# `EndQuietChild`: the quiet start `token` of the pane `parent_id` no longer waits for its child's pane; a token already
    /// released changes nothing.
    pub fn end_quiet_child(&self, parent_id: &str, token: Option<&str>) {
        let Some(token) = token else { return };
        let mut cards = lock(&self.cards);
        let Some(tokens) = cards.quiet_child_parents.get_mut(parent_id) else { return };
        if tokens.remove(token) && tokens.is_empty() {
            cards.quiet_child_parents.remove(parent_id);
        }
    }

    // ── what the window calls and draws (C# :479-596) ──

    /// C# `DelegationChildren`: the delegated children as the sidebar lists them.
    pub fn rows(&self) -> Vec<DelegationChildRow> {
        self.coordinator().map(|coordinator| coordinator.rows()).unwrap_or_default()
    }

    /// The sidebar's children with their cards' live parts (C# `WebChildCard`'s `IsCardBusy`, `CardNote`, `ChildClose`).
    pub fn sidebar_children(&self) -> Vec<SidebarChild> {
        let locale = self.locale();
        let rows = self.rows();
        let cards = lock(&self.cards);
        rows.into_iter()
            .map(|row| {
                let busy = cards.busy.contains(&row.id);
                let note = cards.notes.get(&row.id).and_then(|note| text::note(note, &row, &locale));
                let closed = cards.closes.get(&row.id).and_then(|close| close.borrow().clone());
                let kept = text::close_note(closed.as_ref(), &locale);
                SidebarChild { card: ChildCardState { busy, note, kept }, row }
            })
            .collect()
    }

    /// C# `HeldItems`: the items held for the pane, oldest first.
    pub fn held_items(&self, session_id: &str) -> Vec<DelegationDeliveryItem> {
        self.coordinator().map(|coordinator| coordinator.held_items(session_id)).unwrap_or_default()
    }

    /// C# `Delegation.EndingRuns.Contains`.
    pub fn ending_run(&self, session_id: &str) -> bool {
        self.coordinator().is_some_and(|coordinator| coordinator.ending_runs().contains(session_id))
    }

    /// C# `SendAsync`: the human's send of `request` in an idle pane; items held for the pane go with it, all of them, oldest first and
    /// ahead of its text, in one run. Nothing held: it starts as the service starts it (`NotStarted` when it was stopped before it
    /// started). A start that is refused or fails gives its error.
    pub async fn send(&self, request: StartRunRequest) -> Result<DelegationRelease, RunError> {
        let service = self.service().cloned().ok_or_else(|| RunError::State("the run engine is not ready".into()))?;
        // A read-only launch (W7 plan D2) hands nothing held to a run: the send starts as the service starts it.
        let coordinator =
            self.coordinator().filter(|coordinator| self.window.live() && !coordinator.held_items(&request.session_id).is_empty());
        let Some(coordinator) = coordinator else {
            return Ok(if service.start(request).await?.is_some() {
                DelegationRelease::NothingHeld
            } else {
                DelegationRelease::NotStarted
            });
        };
        let failure: Arc<Mutex<Option<RunError>>> = Arc::new(Mutex::new(None));
        let (launching, failing, human) = (service.clone(), failure.clone(), request.clone());
        let release = coordinator
            .send(
                &request.input,
                &request.session_id,
                Some(Box::new(move |whole: String| {
                    Box::pin(async move {
                        // The pane's title follows the human's words, not the items ahead of them.
                        let mut request = human.clone();
                        request.input = whole;
                        match launching.launch(request, Some(&human.input), true).await {
                            Ok(run) => run,
                            Err(error) => {
                                *lock(&failing) = Some(error);
                                None
                            }
                        }
                    })
                })),
            )
            .await;
        if let Some(error) = lock(&failure).take() {
            return Err(error);
        }
        if release == DelegationRelease::NothingHeld && service.start(request).await?.is_none() {
            return Ok(DelegationRelease::NotStarted);
        }
        Ok(release)
    }

    /// C# `RunNextAsync`: items held for the pane go first, all of them in one run, steered into its running Claude run when it has one.
    pub async fn run_next(&self, session_id: &str) -> DelegationRelease {
        match self.coordinator().filter(|_| self.window.live()) {
            Some(coordinator) => coordinator.run_next(session_id).await,
            None => DelegationRelease::NothingHeld,
        }
    }

    /// C# `ParentCloseQuestion`: what the confirmation of closing a parent pane with open children shows; None closes without asking.
    pub fn parent_close_question(&self, id: &str) -> Option<PendingParentClose> {
        let snapshot = self.snapshot();
        let sessions: Vec<&RunSession> = snapshot.sessions().iter().map(Arc::as_ref).collect();
        let open = sidebar::open_child_count(id, &self.rows(), &sessions);
        let pane = snapshot.session(id)?;
        (open > 0).then(|| PendingParentClose { id: id.to_string(), title: pane.title().to_string(), open_children: open })
    }

    /// C# `CloseSessionAsync`: the pane's run stopped and ended, then the pane gone; what closing a delegated child's pane did to its
    /// worktree and branch, None for any other pane. `closed` is told once the pane has left the state.
    pub async fn close_session(&self, id: &str, closed: impl FnOnce()) -> Result<Option<ChildCleanupOutcome>, RunError> {
        self.stop_and_wait(id).await;
        let service = self.service().cloned().ok_or_else(|| RunError::State("the run engine is not ready".into()))?;
        service.update(|state| {
            let mut next = state.clone();
            next.sessions_mut().retain(|pane| pane.id() != id);
            next
        })?;
        self.window.state_changed();
        closed();
        let close = lock(&self.cards).closes.get(id).cloned();
        let Some(mut close) = close else { return Ok(None) };
        let outcome = close.wait_for(Option::is_some).await.ok().and_then(|outcome| outcome.clone());
        Ok(outcome)
    }

    /// C# `ChildClose`: the close of a delegated child's pane, None while it goes or when none was closed.
    pub fn child_close(&self, id: &str) -> Option<ChildCleanupOutcome> {
        lock(&self.cards).closes.get(id).and_then(|close| close.borrow().clone())
    }

    /// C# `WorkspaceRemovalRefusal`: `workspace_has_children` while delegation has open children or child worktrees not cleaned up in
    /// the workspace; None when it may be removed.
    pub fn workspace_removal_refusal(&self, workspace_id: &str) -> Option<DelegationReasonCode> {
        let coordinator = self.coordinator()?;
        let snapshot = self.snapshot();
        let workspace = snapshot.workspace(workspace_id).filter(|workspace| workspace.remote.is_none())?;
        let panes: HashSet<String> =
            snapshot.sessions().iter().filter(|s| s.workspace_id() == workspace_id).map(|s| s.id().to_string()).collect();
        coordinator.workspace_removal_refusal(&panes, workspace.path())
    }

    /// C# `CardNote`.
    pub fn card_note(&self, id: &str) -> Option<DelegationCardNote> {
        lock(&self.cards).notes.get(id).cloned()
    }

    /// C# `IsCardBusy`.
    pub fn is_card_busy(&self, id: &str) -> bool {
        lock(&self.cards).busy.contains(id)
    }

    // ── the card's merge and its queue (C# :597-773) ──

    /// C# `MergeChildAsync`: the human's merge from the child's card, queued: it goes through by itself once what it waits on passes.
    /// None when an action of this card is going.
    pub async fn merge_child(&self, id: &str) -> Option<DelegationCardNote> {
        {
            let mut cards = lock(&self.cards);
            if cards.busy.contains(id) {
                return None;
            }
            cards.queue.insert(id.to_string());
        }
        self.advance_queued_merge(id, true).await
    }

    /// C# `AdvanceQueuedMerges`.
    pub async fn advance_queued_merges(&self) {
        let ids: Vec<String> = lock(&self.cards).queue.iter().cloned().collect();
        for id in ids {
            self.advance_queued_merge(&id, false).await;
        }
    }

    /// C# `KickQueuedMerges`: off the caller's task.
    fn kick_queued_merges(&self) {
        if self.closing() || lock(&self.cards).queue.is_empty() {
            return;
        }
        let me = self.me.clone();
        self.runtime.spawn(async move {
            if let Some(app) = me.upgrade() {
                app.advance_queued_merges().await;
            }
        });
    }

    /// C# `AdvanceQueuedMerge`: the queued merge of the child moves on by its step now.
    fn advance_queued_merge<'a>(&'a self, id: &'a str, pressed: bool) -> BoxFuture<'a, Option<DelegationCardNote>> {
        Box::pin(async move {
            let row = {
                let cards = lock(&self.cards);
                if !cards.queue.contains(id) || cards.busy.contains(id) || cards.retrying.contains(id) {
                    return None;
                }
                drop(cards);
                self.rows().into_iter().find(|child| child.id == id)
            };
            let Some(row) = row else {
                lock(&self.cards).queue.remove(id);
                return None;
            };
            let parent_running = {
                let service = self.service()?;
                let snapshot = service.snapshot();
                snapshot
                    .session(&row.parent_session_id)
                    .is_some_and(|parent| parent.status() == "running" || service.has_route(parent.id()))
            };
            let mut note = None;
            let mut changed = false;
            match merge_step(row.state, parent_running) {
                DelegationMergeStep::Wait(wait) => {
                    let mut cards = lock(&self.cards);
                    let conflict =
                        matches!(cards.notes.get(id), Some(DelegationCardNote { result: DelegationCardResult::ConflictSent, .. }));
                    // A child resolving its conflict keeps saying so until it reports.
                    if wait == DelegationMergeWait::ChildRunning && conflict {
                        note = cards.notes.get(id).cloned();
                    } else {
                        let queued = DelegationCardNote { action: DelegationCardAction::Merge, result: DelegationCardResult::Queued(wait) };
                        if cards.notes.get(id) != Some(&queued) {
                            cards.notes.insert(id.to_string(), queued.clone());
                            changed = true;
                        }
                        note = Some(queued);
                    }
                }
                DelegationMergeStep::Drop => {
                    let mut cards = lock(&self.cards);
                    cards.queue.remove(id);
                    if matches!(row.state, ChildState::Ended | ChildState::Interrupted)
                        && cards.notes.get(id).is_some_and(|note| note.action == DelegationCardAction::Merge)
                    {
                        let dropped =
                            DelegationCardNote { action: DelegationCardAction::Merge, result: DelegationCardResult::QueueDropped };
                        cards.notes.insert(id.to_string(), dropped.clone());
                        note = Some(dropped);
                        changed = true;
                    }
                }
                DelegationMergeStep::Attempt => note = self.attempt_queued_merge(id, pressed).await,
            }
            if changed {
                self.window.changed();
            }
            self.schedule_merge_retry();
            note
        })
    }

    /// C# `AttemptQueuedMerge`: tries the queued merge now.
    async fn attempt_queued_merge(&self, id: &str, pressed: bool) -> Option<DelegationCardNote> {
        let Some(coordinator) = self.coordinator() else {
            lock(&self.cards).queue.remove(id);
            return None;
        };
        let work = |merging: Coordinator, id: String| async move {
            match merging.merge_from_card(&id).await {
                ChildMergeOutcome::Merged(_) => DelegationCardResult::Done,
                ChildMergeOutcome::MergedUnsaved(_, reason) => DelegationCardResult::DoneUnsaved(reason),
                ChildMergeOutcome::Refused(reason) => match wait_after(reason) {
                    Some(wait) => DelegationCardResult::Queued(wait),
                    None => DelegationCardResult::Refused(reason),
                },
                ChildMergeOutcome::Failed(message) => DelegationCardResult::Failed(message),
            }
        };
        let mut note = if pressed {
            let merging = coordinator.clone();
            let owned = id.to_string();
            self.card_action(id, DelegationCardAction::Merge, work(merging, owned)).await
        } else {
            {
                let mut cards = lock(&self.cards);
                if cards.busy.contains(id) || !cards.retrying.insert(id.to_string()) {
                    return None;
                }
            }
            let result = work(coordinator.clone(), id.to_string()).await;
            lock(&self.cards).retrying.remove(id);
            let note = DelegationCardNote { action: DelegationCardAction::Merge, result };
            let changed = {
                let mut cards = lock(&self.cards);
                let changed = cards.notes.get(id) != Some(&note);
                cards.notes.insert(id.to_string(), note.clone());
                changed
            };
            if changed {
                self.window.changed();
            }
            Some(note)
        };
        if let Some(refused) = note.clone().filter(|note| note.result == DelegationCardResult::Refused(DelegationReasonCode::MergeConflict))
        {
            note = Some(self.ask_child_to_resolve_conflict(id, refused).await);
        }
        {
            // Queued waits on what passes, and a child asked to resolve its conflict waits for its next report; the rest is over.
            let mut cards = lock(&self.cards);
            if !note
                .as_ref()
                .is_some_and(|note| matches!(note.result, DelegationCardResult::Queued(_) | DelegationCardResult::ConflictSent))
            {
                cards.queue.remove(id);
            }
        }
        if !pressed && note.as_ref().is_some_and(|note| note.result == DelegationCardResult::Done) {
            self.window.queued_merge_done(id);
        }
        note
    }

    /// C# `ScheduleMergeRetry`: a slow retry while any merge waits, stopped once none does.
    fn schedule_merge_retry(&self) {
        let interval = lock(&self.start).as_ref().map_or(MERGE_RETRY, |start| start.merge_retry);
        let mut cards = lock(&self.cards);
        if cards.queue.is_empty() {
            if let Some(retry) = cards.retry.take() {
                let _ = retry.send(true);
            }
            return;
        }
        if cards.retry.is_some() || self.closing() {
            return;
        }
        let (stop, mut stopped) = watch::channel(false);
        cards.retry = Some(stop);
        let me = self.me.clone();
        self.runtime.spawn(async move {
            loop {
                tokio::select! {
                    _ = stopped.changed() => return,
                    _ = tokio::time::sleep(interval) => {}
                }
                let Some(app) = me.upgrade() else { return };
                if app.closing() {
                    return;
                }
                if lock(&app.cards).queue.is_empty() {
                    lock(&app.cards).retry = None;
                    return;
                }
                app.advance_queued_merges().await;
            }
        });
    }

    /// C# `AskChildToResolveConflict`: the child is asked, once per report, to bring the parent branch in, resolve the conflict and
    /// report again, as its pane's next run; a pane that would refuse the start, or a child that cannot commit, is not asked.
    async fn ask_child_to_resolve_conflict(&self, id: &str, refused: DelegationCardNote) -> DelegationCardNote {
        let Some(service) = self.service().cloned() else { return refused };
        let pane = {
            let snapshot = service.snapshot();
            snapshot.session(id).filter(|pane| pane.status() != "running").map(|pane| pane.as_ref().clone())
        };
        {
            let mut cards = lock(&self.cards);
            let Some(pane) = pane.as_ref().filter(|_| !self.closing() && !cards.busy.contains(id) && !service.has_route(id)) else {
                return refused;
            };
            let mode = pane.settings.as_ref().map(|s| s.permission_mode.clone()).unwrap_or_default();
            if !providers::can_commit(pane.provider(), &mode) || app_host::run_refusal(pane).is_some() {
                return refused;
            }
            cards.busy.insert(id.to_string());
        }
        let pane = pane.unwrap_or_default();
        let mut said = None;
        if self.window.run_blocked(&pane).is_none()
            && let Some(coordinator) = self.coordinator()
            && let Some(text) = coordinator.conflict_resolution(id)
        {
            let result = if self.start_delegated_run(id, &text, None).await.is_none() {
                coordinator.conflict_resolution_not_started(id);
                DelegationCardResult::ConflictNotStarted
            } else {
                DelegationCardResult::ConflictSent
            };
            said = Some(DelegationCardNote { action: DelegationCardAction::Merge, result });
        }
        {
            let mut cards = lock(&self.cards);
            cards.busy.remove(id);
            if let Some(said) = &said
                && cards.notes.get(id) == Some(&refused)
            {
                cards.notes.insert(id.to_string(), said.clone());
            }
        }
        self.window.changed();
        said.unwrap_or(refused)
    }

    /// C# `MergedDiscardOffer`: after a merge from the child's card went through, the offer to discard the child now.
    pub async fn merged_discard_offer(&self, id: &str) -> Option<PendingChildDiscard> {
        let coordinator = self.coordinator()?;
        let row = self.rows().into_iter().find(|child| child.id == id)?;
        let title = self.child_title(id, &row);
        let nested = coordinator.nested_worktrees(id).await?;
        Some(PendingChildDiscard { id: id.to_string(), title, branch: crate::git::branch_name(id), nested: Some(nested) })
    }

    fn child_title(&self, id: &str, row: &DelegationChildRow) -> String {
        self.snapshot()
            .session(id)
            .and_then(|pane| pane.title.clone())
            .or_else(|| row.task.clone())
            .unwrap_or_else(|| self.locale().get("delegation.child.untitled"))
    }

    /// C# `UndoChildMergeAsync`.
    pub async fn undo_child_merge(&self, id: &str) -> Option<DelegationCardNote> {
        let coordinator = self.coordinator()?;
        let owned = id.to_string();
        let work = async move {
            match coordinator.undo_merge_from_card(&owned).await {
                ChildUndoOutcome::Undone { .. } => DelegationCardResult::Done,
                ChildUndoOutcome::UndoneUnsaved { reason, .. } => DelegationCardResult::DoneUnsaved(reason),
                ChildUndoOutcome::Refused(reason) => DelegationCardResult::Refused(reason),
                ChildUndoOutcome::Failed(message) => DelegationCardResult::Failed(message),
            }
        };
        self.card_action(id, DelegationCardAction::Undo, work).await
    }

    /// C# `AskToDiscardChildAsync`: what the discard's confirmation shows; None when the child is not listed or its card is busy.
    pub async fn ask_to_discard_child(&self, id: &str) -> Option<PendingChildDiscard> {
        let coordinator = self.coordinator()?;
        let row = self.rows().into_iter().find(|child| child.id == id)?;
        {
            let mut cards = lock(&self.cards);
            if !cards.busy.insert(id.to_string()) {
                return None;
            }
            cards.notes.remove(id);
        }
        self.window.changed();
        let title = self.child_title(id, &row);
        let nested = coordinator.nested_worktrees(id).await;
        lock(&self.cards).busy.remove(id);
        self.window.changed();
        Some(PendingChildDiscard { id: id.to_string(), title, branch: crate::git::branch_name(id), nested })
    }

    /// C# `DiscardChildAsync`: the discard the human confirmed; more nested worktrees now ask again; the child's pane closes once done.
    pub async fn discard_child(&self, confirmed: &PendingChildDiscard) -> Option<DelegationCardNote> {
        let coordinator = self.coordinator()?;
        let (id, nested) = (confirmed.id.clone(), confirmed.nested.clone());
        let work = async move {
            match coordinator.discard_from_card(&id, nested.as_deref()).await {
                ChildCardDiscardOutcome::Discarded => DelegationCardResult::Done,
                ChildCardDiscardOutcome::NestedChanged(nested) => DelegationCardResult::Reconfirm(nested),
                ChildCardDiscardOutcome::Failed(message) => DelegationCardResult::Failed(message),
            }
        };
        let note = self.card_action(&confirmed.id, DelegationCardAction::Discard, work).await?;
        if note.result != DelegationCardResult::Done {
            return Some(note);
        }
        lock(&self.cards).notes.remove(&confirmed.id);
        let _ = self.close_session(&confirmed.id, || {}).await;
        self.window.changed();
        Some(note)
    }

    /// C# `CardAction`: one card action of the child at a time, its answer kept for the card.
    async fn card_action(
        &self,
        id: &str,
        action: DelegationCardAction,
        work: impl std::future::Future<Output = DelegationCardResult>,
    ) -> Option<DelegationCardNote> {
        self.coordinator()?;
        {
            let mut cards = lock(&self.cards);
            if !cards.busy.insert(id.to_string()) {
                return None;
            }
            cards.notes.remove(id);
        }
        self.window.changed();
        let note = DelegationCardNote { action, result: work.await };
        {
            let mut cards = lock(&self.cards);
            cards.busy.remove(id);
            cards.notes.insert(id.to_string(), note.clone());
        }
        self.window.changed();
        Some(note)
    }
}

/// The app as a host of its own (the coordinator reaches it through a link that does not keep it alive).
impl DelegationHost for DelegationApp {
    fn create_pane(&self, pane: DelegationChildPane) -> BoxFuture<'_, bool> {
        Box::pin(self.create_pane_now(pane))
    }
    fn start_run(&self, session_id: String, input: String) -> BoxFuture<'_, Option<String>> {
        Box::pin(async move { self.start_delegated_run(&session_id, &input, None).await })
    }
    fn deliver(&self, input: String, session_id: String, route: DeliveryRoute) -> BoxFuture<'_, Option<String>> {
        Box::pin(self.deliver_now(input, session_id, route))
    }
    fn pane_state(&self, session_id: String) -> BoxFuture<'_, Option<DelegationPaneState>> {
        Box::pin(async move { self.pane_state_now(&session_id) })
    }
    /// C# `IDelegationHost.StopRun`, a discard's stop: the child's run and then its agent terminal.
    fn stop_run(&self, session_id: String) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            self.stop_and_wait(&session_id).await;
            self.window.end_agent_terminal(session_id).await;
        })
    }
    fn note(&self, text: String, session_id: String) -> BoxFuture<'_, ()> {
        self.note_now(text, session_id);
        Box::pin(async {})
    }
    fn agent_availability(&self, provider: String) -> BoxFuture<'_, DelegationAgentAvailability> {
        let availability = self.window.agent_availability(&provider);
        Box::pin(async move { availability })
    }
}

/// C# `DesktopService.Runs`: a run of the pane would validate.
fn runs(pane: &RunSession) -> bool {
    let mut request = StartRunRequest::new(pane.id(), pane.workspace_id(), pane.kind(), "notice");
    request.model = pane.model().to_string();
    request.provider = pane.provider().to_string();
    request.settings = pane.settings.clone();
    request.resume_id = pane.resume_id.clone();
    request.validate().is_ok()
}

impl DelegationRunHooks for DelegationApp {
    /// C# `DelegationWorktreeRoot`: a delegated child's worktree when its runs work in a folder inside it (a Codex or Antigravity run
    /// writes REPORT.md there); None for any other pane.
    fn worktree_root(&self, pane: &RunSession) -> Option<String> {
        if pane.parent_session_id.is_none() || !matches!(pane.provider(), "codex" | "gemini") {
            return None;
        }
        let working = pane.working_folder.as_deref()?;
        let made = self.coordinator()?.worktrees().worktree_path(pane.id());
        let (folder, root) = (paths::standardized(working), paths::standardized(&made));
        (!paths::same(&folder, &root) && paths::is_inside(&folder, &root)).then_some(root)
    }

    /// C# `DelegationRunBegan`: a new run starts in the pane: its id. A child's start reaches the coordinator first.
    fn began(&self, pane: &RunSession) -> String {
        let run_id = self.runs.begin(pane.id());
        if pane.parent_session_id.is_some() && self.window.live() {
            lock(&self.child_runs).insert(pane.id().to_string());
            if let Some(loaded) = lock(&self.loaded).as_ref() {
                loaded.pump.send(DelegationRunEvent::Started { child_id: pane.id().to_string(), run_id: run_id.clone() });
            }
        }
        run_id
    }

    /// C# `DelegationRunEnded`: a child's end reaches the coordinator after its start, marked ending until recorded; while the app quits
    /// it counts as killed by quitting. False when no run of this launch was going there.
    fn ended(&self, session_id: &str, status: &str, closing: bool) -> bool {
        let Some((run_id, end)) = self.runs.end(session_id, status, closing) else { return false };
        if lock(&self.child_runs).remove(session_id)
            && let Some(loaded) = lock(&self.loaded).as_ref()
        {
            loaded.coordinator.ending_runs().begin(session_id);
            loaded.pump.send(DelegationRunEvent::Ended { child_id: session_id.to_string(), run_id, end });
        }
        true
    }

    /// The end of C# `Receive`: a run that is up takes what its start could not, a pane's end offers again what it could not take while
    /// ending, a notice turn's end offers what waited for it; a parent's end may let its queued merges through.
    fn received(&self, event: &RunEvent, ended: bool, notice_turn_ended: bool) {
        if ended || event.kind == "status" && event.status.as_deref() == Some("running") || notice_turn_ended {
            self.offer_pending();
        }
        if ended {
            self.kick_queued_merges();
        }
    }

    /// C# `PaneClosed`: a delegated child's close reaches the coordinator once its run has ended, and a merged child's worktree and
    /// branch are cleaned up when every rule holds. A parent's close closes only the parent; a read-only launch (D2) cleans up nothing.
    fn pane_closed(&self, pane: &RunSession) {
        let coordinator = self.coordinator();
        let child = coordinator.as_ref().is_some_and(|coordinator| {
            pane.parent_session_id.is_some() || coordinator.file().children.iter().any(|child| child.id == pane.id())
        });
        let (Some(coordinator), true, true) = (coordinator, child, self.window.live()) else {
            self.runs.forget(pane.id());
            return;
        };
        let (close, outcome) = watch::channel(None);
        lock(&self.cards).closes.insert(pane.id().to_string(), outcome);
        let (me, id) = (self.me.clone(), pane.id().to_string());
        self.runtime.spawn(async move {
            let Some(app) = me.upgrade() else { return };
            // A run that did not end keeps the child busy: its worktree is never cleaned up under a live CLI.
            let ended = app.stop_and_wait(&id).await;
            app.window.end_agent_terminal(id.clone()).await;
            app.runs.forget(&id);
            let closing = id.clone();
            let task = app.runtime.spawn(async move { coordinator.close_child(&closing, !ended).await });
            let outcome = task.await.unwrap_or_else(|error| ChildCleanupOutcome::Failed(error.to_string()));
            let _ = close.send(Some(outcome));
            app.window.changed();
        });
    }
}
