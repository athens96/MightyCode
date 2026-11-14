//! The desktop service's delegation (C# Core/DesktopService.Delegation.cs; DesktopService.cs:129-155, 189, 261-275, 346-351): the app
//! side of parent → child delegation over an `Arc<mighty_run::RunService>` (the state's owner) and the window's part
//! ([`DelegationWindow`], C#'s `DelegatedRunBlocked`, `DelegatedRunRequest`, `EndAgentTerminal`, `AgentAvailabilityNow`,
//! `DelegationChanged`, `DelegationNoted`, `QueuedMergeDone`); no Tauri, so the end-to-end scenarios run headless in `cargo test`. It
//! is the coordinator's [`DelegationHost`] (it makes panes, starts and steers runs, reports what its panes do) and the run service's
//! [`DelegationRunHooks`] (the run ledger, each child's run starts and ends handed to the coordinator in order, a closed child's
//! cleanup). src-tauri delegation.rs drives it. Owner: W7-D.
//!
//! Concurrency: the run service's lock is never taken here while one of this module's own locks is held, and no lock is held across
//! an await (the crate denies `await_holding_lock`). The hooks the service calls under its lock take only this module's own locks.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use mighty_composer::queue::notice_turn_running;
use mighty_core::model::{LogEntry, RunSession};
use mighty_core::{time, wire};
use mighty_platform::BoxFuture;
use mighty_run::RunError;
use mighty_run::model::StartRunRequest;
use mighty_run::service::{DelegationRunHooks, RunService};

use crate::app_host::{self, DelegationRunEvent, DelegationRunEventPump, DelegationRunLedger};
use crate::host::{DelegationAgentAvailability, DelegationChildPane, DelegationHost, DelegationPaneState};
use crate::model::DeliveryRoute;

/// How long a stop waits for the pane's run to end (C# `StopAndWait`).
const STOP_WAIT: Duration = Duration::from_secs(10);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The window's part of delegation (C# MainWindow.Delegation.cs `InitializeDelegation`: the service's `Func`s and events). Each is
/// called from a task of the app, never under one of its locks or the run service's; none may wait on the window's own lock for long.
pub trait DelegationWindow: Send + Sync {
    /// C# `DelegatedRunBlocked`: why the window would refuse a run in the pane now (a CLI update, plugins changing, a sign-in, the app
    /// closing), or None when one may start. Delegation then starts no run there, and what comes for a pane idle after a normal finish
    /// is held as a row of its queued list.
    fn run_blocked(&self, pane: &RunSession) -> Option<String> {
        let _ = pane;
        None
    }
    /// C# `DelegatedRunRequest`: the request of a run delegation starts on its own, finished the way the window finishes its own sends
    /// (the model list read first, the provider's registered models, a style's launch mode). Without it the request is the pane as
    /// saved.
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
    /// The state changed through the run service (a pane made, a note): the window's copy follows.
    fn state_changed(&self) {}
    /// C# `DelegationChanged`: something delegation shows changed (the sidebar's tree, a pane's held rows, a child's card, a child's
    /// close, a child's pane made).
    fn changed(&self) {}
    /// C# `DelegationNoted`: delegation said something in the pane's transcript.
    fn noted(&self, session_id: &str) {
        let _ = session_id;
    }
}

/// The desktop service's delegation (C# `DesktopService`'s `IDelegationHost` half and its run hooks).
pub struct DelegationApp {
    /// The run service, attached once it is made (it is made with this app as its hooks).
    service: OnceLock<Arc<RunService>>,
    window: Arc<dyn DelegationWindow>,
    /// C# `delegationRuns`: the id of each pane's run as delegation names it, and which panes finished a run normally since this launch.
    runs: DelegationRunLedger,
    /// C# `childRuns`: the child panes with a run going, whose end the coordinator hears even when the pane closes first.
    child_runs: Mutex<HashSet<String>>,
    /// C# `delegationRunEvents`: child panes' run starts and ends on their way to the coordinator, in order; None while no coordinator
    /// is loaded.
    run_events: Mutex<Option<Arc<DelegationRunEventPump>>>,
    /// C# `quietChildParents`: panes whose new worktree children the phone's 바로 실행 is starting, one token per start; such a child is
    /// never selected in this window.
    quiet_child_parents: Mutex<HashMap<String, HashSet<String>>>,
}

impl DelegationApp {
    pub fn new(window: Arc<dyn DelegationWindow>) -> Arc<DelegationApp> {
        Arc::new(DelegationApp {
            service: OnceLock::new(),
            window,
            runs: DelegationRunLedger::new(),
            child_runs: Mutex::new(HashSet::new()),
            run_events: Mutex::new(None),
            quiet_child_parents: Mutex::new(HashMap::new()),
        })
    }

    /// The run service this app was made the hooks of; set once.
    pub fn attach(&self, service: Arc<RunService>) {
        let _ = self.service.set(service);
    }

    fn service(&self) -> Option<&Arc<RunService>> {
        self.service.get()
    }

    /// C# `closing` (or no service yet).
    fn closing(&self) -> bool {
        self.service().is_none_or(|service| service.closing())
    }

    /// C# `RunId(sessionId)`: the pane's run going now, or its last since this launch, as delegation's receipts name it.
    pub fn run_id(&self, session_id: &str) -> Option<String> {
        self.runs.run_id(session_id)
    }

    /// The pump the child runs' events go through to the coordinator (C# `StartDelegation`'s `delegationRunEvents`); None stops
    /// handing them on.
    pub fn set_run_events(&self, pump: Option<Arc<DelegationRunEventPump>>) {
        *lock(&self.run_events) = pump;
    }

    /// C# `StartDelegatedRun`: a run with `input` in the pane, the way a send starts one: its id once the run's process has it, or None
    /// when the pane cannot run now (a child without its worktree among them) or its start failed or was stopped, so what it carried is
    /// not lost. Its words are delegation's, so they never title the pane.
    pub async fn start_delegated_run(
        &self,
        session_id: &str,
        input: &str,
        attachments: Option<Vec<mighty_core::attachment::RunAttachment>>,
    ) -> Option<String> {
        let service = self.service()?.clone();
        if service.closing() {
            return None;
        }
        let pane = service.snapshot().session(session_id).cloned()?;
        if self.window.run_blocked(&pane).is_some() {
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

    /// C# `StopAndWait`: stops the pane's run, if one is going, and returns once it has ended (its CLI and everything the CLI started
    /// exited). False when it had not ended after ten seconds.
    pub async fn stop_and_wait(&self, session_id: &str) -> bool {
        let Some(service) = self.service().cloned() else { return true };
        if !service.has_route(session_id) {
            return true;
        }
        let deadline = Instant::now() + STOP_WAIT;
        if let Err(RunError::TimedOut(_)) = service.stop(session_id).await {
            return false;
        }
        // A route not started yet ends with its launch's own stopped event.
        while service.has_route(session_id) {
            if Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        true
    }

    /// C# `EndTerminal`: the pane's agent terminal ended by the window.
    async fn end_terminal(&self, session_id: &str) {
        self.window.end_agent_terminal(session_id.to_string()).await;
    }

    /// The quiet start `token` of the pane `parent_id` waits for its child's pane (C# `StartHumanChildAsync`'s token).
    pub fn begin_quiet_child(&self, parent_id: &str, token: &str) {
        lock(&self.quiet_child_parents).entry(parent_id.to_string()).or_default().insert(token.to_string());
    }

    /// C# `EndQuietChild`: the quiet start `token` of the pane `parent_id` no longer waits for its child's pane; a token already
    /// released changes nothing.
    pub fn end_quiet_child(&self, parent_id: &str, token: Option<&str>) {
        let Some(token) = token else { return };
        let mut parents = lock(&self.quiet_child_parents);
        let Some(tokens) = parents.get_mut(parent_id) else { return };
        if !tokens.remove(token) {
            return;
        }
        if tokens.is_empty() {
            parents.remove(parent_id);
        }
    }

    /// The child's pane made beside its parent (C# `IDelegationHost.CreatePane`), its task the pane's title.
    async fn add_child_pane(&self, pane: DelegationChildPane, task: Option<String>) -> bool {
        let Some(service) = self.service().cloned() else { return false };
        // The phone's 바로 실행 child never moves this window's view.
        let quiet = lock(&self.quiet_child_parents).get(&pane.parent_session_id).is_some_and(|tokens| !tokens.is_empty());
        // A save the disk refuses is reported as any other; the pane is open all the same.
        let made = service.try_update(|state| app_host::adding_child(state, &pane, task.as_deref(), quiet));
        if !matches!(made, Ok(true)) {
            return false;
        }
        self.window.state_changed();
        self.window.changed();
        service.snapshot().session(&pane.session_id).is_some()
    }
}

/// C# `DesktopService.Runs`: a run of the pane would validate (every notice and follow-up is a valid run input, so the start is asked
/// about with a stand-in for its words).
fn runs(pane: &RunSession) -> bool {
    let mut request = StartRunRequest::new(pane.id(), pane.workspace_id(), pane.kind(), "notice");
    request.model = pane.model().to_string();
    request.provider = pane.provider().to_string();
    request.settings = pane.settings.clone();
    request.resume_id = pane.resume_id.clone();
    request.validate().is_ok()
}

/// C# `DelegationProviders.CanSteer`: only a Claude run takes a steer.
fn can_steer(kind: &str, provider: &str) -> bool {
    kind == "claude" && provider == "claude"
}

impl DelegationRunHooks for DelegationApp {
    /// C# `DelegationWorktreeRoot`: a delegated child's worktree when its runs work in a folder inside it, read from the coordinator's
    /// maker; None while no coordinator is loaded.
    fn worktree_root(&self, pane: &RunSession) -> Option<String> {
        let _ = pane;
        None
    }

    /// C# `DelegationRunBegan`: a new run starts in the pane: its id. A child's start reaches the coordinator first.
    fn began(&self, pane: &RunSession) -> String {
        let run_id = self.runs.begin(pane.id());
        if pane.parent_session_id.is_some() {
            lock(&self.child_runs).insert(pane.id().to_string());
            if let Some(events) = lock(&self.run_events).as_ref() {
                events.send(DelegationRunEvent::Started { child_id: pane.id().to_string(), run_id: run_id.clone() });
            }
        }
        run_id
    }

    /// C# `DelegationRunEnded`: a run of the pane ended. A child's end reaches the coordinator after its start; while the app quits it
    /// counts as killed by quitting. False when no run of this launch was going there.
    fn ended(&self, session_id: &str, status: &str, closing: bool) -> bool {
        let Some((run_id, end)) = self.runs.end(session_id, status, closing) else { return false };
        if lock(&self.child_runs).remove(session_id)
            && let Some(events) = lock(&self.run_events).as_ref()
        {
            events.send(DelegationRunEvent::Ended { child_id: session_id.to_string(), run_id, end });
        }
        true
    }

    /// C# `PaneClosed` for a pane that is no delegated child (or with no coordinator loaded): its runs are forgotten.
    fn pane_closed(&self, pane: &RunSession) {
        self.runs.forget(pane.id());
    }
}

impl DelegationHost for DelegationApp {
    /// C# `IDelegationHost.CreatePane`: the child's pane of its parent's agent, as a tab beside its parent, in the mode the parent asked
    /// for, linked to its parent and working in its worktree folder; the link saved before its first run. No coordinator is loaded
    /// yet, so the pane is titled by its agent.
    fn create_pane(&self, pane: DelegationChildPane) -> BoxFuture<'_, bool> {
        Box::pin(async move {
            if self.closing() {
                return false;
            }
            self.add_child_pane(pane, None).await
        })
    }

    fn start_run(&self, session_id: String, input: String) -> BoxFuture<'_, Option<String>> {
        Box::pin(async move { self.start_delegated_run(&session_id, &input, None).await })
    }

    /// C# `IDelegationHost.Deliver`: a notice or follow-up steered into the pane's running Claude run and shown in its transcript, or
    /// started as its next run.
    fn deliver(&self, input: String, session_id: String, route: DeliveryRoute) -> BoxFuture<'_, Option<String>> {
        Box::pin(async move {
            if route == DeliveryRoute::Queue {
                return self.start_delegated_run(&session_id, &input, None).await;
            }
            let service = self.service()?.clone();
            // Claude answering finished background work takes no steer: what comes now waits for that turn instead.
            let steerable = !service.closing()
                && service.snapshot().session(&session_id).is_some_and(|pane| {
                    pane.status() == "running" && can_steer(pane.kind(), pane.provider()) && !notice_turn_running(pane.graph_runs())
                })
                && self.runs.run_id(&session_id).is_some();
            if !steerable || !service.try_steer(&session_id, &input) {
                return None;
            }
            // The receipt names the run that took it, read once it has.
            self.runs.run_id(&session_id)
        })
    }

    /// C# `IDelegationHost.PaneState`: the pane as delegation reads it. One idle after a normal finish whose start would be refused now
    /// reads as idle. A remote workspace's pane runs on its host: delegation never knows it.
    fn pane_state(&self, session_id: String) -> BoxFuture<'_, Option<DelegationPaneState>> {
        Box::pin(async move {
            let service = self.service()?;
            let snapshot = service.snapshot();
            let pane = snapshot.session(&session_id)?;
            let workspace = snapshot.workspace(pane.workspace_id()).filter(|workspace| workspace.remote.is_none())?;
            let activity = self.runs.activity(&session_id, pane.status() == "running" || service.has_route(&session_id));
            let run_id = self.runs.run_id(&session_id);
            let refused = activity == crate::host::DelegationPaneActivity::Finished
                && (service.closing() || app_host::run_refusal(pane).is_some() || self.window.run_blocked(pane).is_some() || !runs(pane));
            Some(app_host::pane_state(pane, workspace, run_id.as_deref(), activity, refused))
        })
    }

    /// C# `IDelegationHost.StopRun`, a discard's stop: the child's run and then its agent terminal, each ended before the discard looks
    /// at its worktree.
    fn stop_run(&self, session_id: String) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            self.stop_and_wait(&session_id).await;
            self.end_terminal(&session_id).await;
        })
    }

    /// C# `IDelegationHost.Note`: the app's note in the pane's transcript, as a system line. Does nothing for a closed pane.
    fn note(&self, text: String, session_id: String) -> BoxFuture<'_, ()> {
        Box::pin(async move {
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
        })
    }

    fn agent_availability(&self, provider: String) -> BoxFuture<'_, DelegationAgentAvailability> {
        Box::pin(async move { self.window.agent_availability(&provider) })
    }
}
