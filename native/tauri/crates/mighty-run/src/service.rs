//! The run half of C# `DesktopService` (DesktopService.cs:97-127, 249-276, 299-416): starting a pane's request (`Launch`: the pane
//! retitled, its user entry and running state applied before the runner takes it), every run event applied to the state
//! (`Receive`: `AppSnapshot.Apply`, which runs `ClaudePlanMode.Record`), the routes that let a stop reach a run not started yet, the
//! answers to a run's requests, and the waiting tool-permission requests per pane (`ToolPermissionChanged`).
//!
//! Host-agnostic: no Tauri type. The service owns the state's copy it applies events to; what happened goes out through one
//! channel ([`ServiceEvent`]) in order — each applied event, each permission change, and when to save (at once at a run's end,
//! else on the host's save schedule, C#'s `QueueSave` and `ScheduleSave`).
//!
//! Antigravity's cards and the rules their continuations hold are [`antigravity`]'s (C# DesktopService.Antigravity.cs): each pane's
//! card ([`ServiceEvent::AntigravityCard`] when it changes), the rules a continuation puts in agy's settings before its run starts and
//! takes out when it ends, the card back with the reason when the continuation cannot start.
//!
//! Delegation (W7) hears every run's start and end and every closed pane through [`DelegationRunHooks`]; its ledger, merges and
//! notices are mighty-delegation's. A remote workspace's runs go through a [`RemoteRunner`] (C# DesktopService.Remote.cs, W8-D); the file
//! originals' pruning (W4), the style's own tool permissions (W6) and the known-session list (W4: [`ServiceEvent::RememberSession`]
//! hands the id to the host) are not here.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use indexmap::IndexMap;
use mighty_core::graph::GRAPH_PROVIDERS;
use mighty_core::model::{AppSnapshot, PlanRecord, RunSession, Workspace};
use mighty_core::normalize;
use mighty_core::time::DateTimeOffset;
use mighty_core::wire;
use mighty_platform::BoxFuture;
use tokio::sync::mpsc;

use crate::antigravity::AntigravityRuleLeases;
use crate::apply;
use crate::attachments;
use crate::manager::{BindingFactory, RunManager, RunManagerOptions};
use crate::model::{RunEvent, StartRunRequest};
use crate::permissions::ToolPermissionRequest;
use crate::phase_models;
use crate::plan_mode::PlanDecision;
use crate::providers::{self, ProviderCatalog};
use crate::questionnaire::UserQuestionAnswer;
use crate::strings;
use crate::timing;
use crate::{Result, RunError, lock};

mod antigravity;

/// What the service tells its host, in order.
#[derive(Debug, Clone)]
pub enum ServiceEvent {
    /// A run event as applied (C# `RunEventReceived`). `notice_turn_ended`: a notice turn ran before it and no longer does.
    Run { event: Box<RunEvent>, notice_turn_ended: bool },
    /// A request is about to start (C# `RequestStarting`).
    RequestStarting(Box<StartRunRequest>),
    /// A tool-permission request waiting in, or settled by, a local pane (C# `ToolPermissionChanged`): never a run event, never saved.
    ToolPermission(Box<ToolPermissionRequest>),
    /// Save the state now (a run's start and end: C# `QueueSave`).
    SaveNow,
    /// Save the state on the host's schedule (a streaming run: C# `ScheduleSave`).
    SaveSoon,
    /// A session id a pane started or resumed (C# `RememberSession`, the resume list's known sessions).
    RememberSession(String),
    /// A pane's Antigravity card came, went, or its answer moved (C# `AntigravityCardChanged`): the pane's id.
    AntigravityCard(String),
}

/// How a pane changed by [`RunService::update_pane`] is saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneSave {
    /// At once (C# `QueueSave`: a shell exited or failed, a shell block's run ended).
    Now,
    /// On the host's schedule (C# `ScheduleSave`: a shell block's run streaming).
    Soon,
    /// With the next save anything else asks for (a shell's idle and running: a restart reads running as idle anyway).
    Later,
}

/// Where a remote runner hands its runs' events and tool requests (C# `AttachRemote`'s `Receive` and `ToolPermissionChanged`): they arrive
/// as a local run's do. Called from the runner's tasks while it applies a poll: each takes the service's lock briefly and never waits.
#[derive(Clone)]
pub struct RemoteSinks {
    pub event: Arc<dyn Fn(RunEvent) + Send + Sync>,
    pub permission: Arc<dyn Fn(ToolPermissionRequest) + Send + Sync>,
}

/// Another computer's runs, for a remote workspace's panes (C# `RemoteClientService`, W8-D's `RemoteClient`).
pub trait RemoteRunner: Send + Sync {
    /// Called once, as the service is made, with where the runs' events and tool requests go.
    fn attach(&self, sinks: RemoteSinks);
    /// C# `EnsureConnectedAsync` and `StartRemoteAsync`'s check: connected now (reconnecting first when it is not; the first send after
    /// launch reads the key from the vault), on a connection that still shares the workspace (else `remote.workspace.error.notShared`).
    fn connect(&self, workspace: Workspace) -> BoxFuture<'_, Result<()>>;
    /// C# `StartAsync(request, workspace)`: the host took the run (Ok), or it was stopped before (`RunError::Cancelled`).
    fn start(&self, request: StartRunRequest, workspace: Workspace) -> BoxFuture<'_, Result<()>>;
    fn stop(&self, session_id: String) -> BoxFuture<'_, Result<()>>;
    fn respond_to_tool_permission(&self, session_id: String, request_id: String, allow: bool) -> BoxFuture<'_, Result<()>>;
    fn answer_questions(
        &self,
        session_id: String,
        request_id: String,
        answers: IndexMap<String, UserQuestionAnswer>,
    ) -> BoxFuture<'_, Result<()>>;
    fn answer_plan(&self, session_id: String, request_id: String, decision: PlanDecision) -> BoxFuture<'_, Result<()>>;
    fn is_running(&self, session_id: &str) -> bool;
    /// C# `DisposeAsync`: the app quits; every run stops, on the host too.
    fn dispose(&self) -> BoxFuture<'_, ()>;
}

/// The remote mirror's panes (C# DesktopService.Mirror.cs, W9-J: mighty-remote's `remote::mirror::desk`): a pane that shows another
/// computer's pane (its `mirror` reference set). Its requests, stops and answers go to that computer; nothing of it runs here.
pub trait MirrorRunner: Send + Sync {
    /// C# `MirrorSubmitAsync`: the text and files as the host pane's next request; the host's word for what it did (`started`,
    /// `steered`, `queued`). A pane whose computer is not connected is refused before anything goes.
    fn submit(
        &self,
        session_id: String,
        text: String,
        attachments: Vec<mighty_core::attachment::RunAttachment>,
    ) -> BoxFuture<'_, Result<String>>;
    /// C# `MirrorStopAsync`.
    fn stop(&self, session_id: String) -> BoxFuture<'_, Result<()>>;
    /// C# `AnswerMirror` with `MirrorPermissionAsync`: sent without holding the caller; a failure is said by the mirror itself.
    fn respond_to_tool_permission(&self, session_id: String, request_id: String, allow: bool);
    /// C# `AnswerMirror` with `MirrorAnswersAsync`.
    fn answer_questions(&self, session_id: String, request_id: String, answers: IndexMap<String, UserQuestionAnswer>);
    /// C# `AnswerMirror` with `MirrorPlanAsync`.
    fn answer_plan(&self, session_id: String, request_id: String, decision: PlanDecision);
    /// C# `WakeMirrorFor`: the user looks at another workspace or pane (`UpdateAsync` moved the active ids). Never waits.
    fn looking_at(&self, workspace_id: &str);
}

/// What `Receive` learnt under the lock, for what follows it outside.
#[derive(Debug, Clone, Copy)]
struct Received {
    /// Delegation's ledger had a run of this launch going in the pane, and it ended now (C# `DelegationRunEnded`'s answer).
    delegation_ended: bool,
    /// A notice turn ran before the event and no longer does.
    notice_turn_ended: bool,
}

#[derive(Default)]
struct Route {
    started: bool,
    cancelled: bool,
    /// Which start this route is (C10): the temporary Antigravity rules it holds are released only by its own end.
    holder: u64,
}

struct State {
    snapshot: AppSnapshot,
    routes: HashMap<String, Route>,
    closing: bool,
}

/// What a service the host gave no Antigravity settings has (C5): leases that refuse every grant.
fn refusing_rules() -> AntigravityRuleLeases {
    AntigravityRuleLeases::refusing(&AntigravityRuleLeases::default_settings_path(None))
}

struct Shared {
    state: Mutex<State>,
    events: mpsc::UnboundedSender<ServiceEvent>,
    /// The waiting tool-permission requests of each pane, in the order they came.
    pending: Mutex<HashMap<String, IndexMap<String, ToolPermissionRequest>>>,
    /// Every pane's Antigravity card and the rules its continuation holds.
    antigravity: Mutex<antigravity::Desk>,
    /// agy's settings (`AntigravityRules`), set once the runner's options are made.
    antigravity_rules: OnceLock<AntigravityRuleLeases>,
    /// True once the leftovers a crash left in agy's settings are out (C# `InitializeAsync` awaits `RemoveLeftovers`): no agy run starts
    /// before (C2).
    leftovers_removed: tokio::sync::watch::Receiver<bool>,
    /// Delegation's hooks ([`RunServiceOptions::delegation`]).
    delegation: Option<Arc<dyn DelegationRunHooks>>,
}

impl Shared {
    /// `AntigravityRules`: the ones the host gave (`RunManagerOptions::antigravity_rules`), else leases that refuse every grant (C5): no
    /// rule ever lands in the user's real settings without a ledger.
    fn rules(&self) -> &AntigravityRuleLeases {
        self.antigravity_rules.get_or_init(refusing_rules)
    }

    fn send(&self, event: ServiceEvent) {
        let _ = self.events.send(event);
    }

    /// `Receive`: a valid event applied; a run's end saved at once, anything else on the schedule.
    fn receive(&self, value: RunEvent) {
        if !value.valid() {
            return;
        }
        let received = {
            let mut state = lock(&self.state);
            self.receive_locked(&mut state, &value)
        };
        self.after_receive(value, received);
    }

    /// `Receive` under the lock: the event applied, the route of a run that ended removed and its end told to delegation
    /// (`DelegationRunEnded`), the save asked for.
    fn receive_locked(&self, state: &mut State, value: &RunEvent) -> Received {
        let before = state.snapshot.session(&value.session_id).map(|s| s.graph_runs().to_vec()).unwrap_or_default();
        state.snapshot = apply::apply(&state.snapshot, value, DateTimeOffset::now());
        let after = state.snapshot.session(&value.session_id).map(|s| s.graph_runs().to_vec()).unwrap_or_default();
        let ended = value.kind == "status" && matches!(value.status.as_deref(), Some("stopped" | "completed" | "error"));
        let mut delegation_ended = false;
        if ended {
            state.routes.remove(&value.session_id);
            delegation_ended = self
                .delegation
                .as_ref()
                .is_some_and(|hooks| hooks.ended(&value.session_id, value.status.as_deref().unwrap_or_default(), state.closing));
            self.send(ServiceEvent::SaveNow);
        } else {
            self.send(ServiceEvent::SaveSoon);
        }
        Received { delegation_ended, notice_turn_ended: apply::notice_turn_ended(&before, &after) }
    }

    fn after_receive(&self, value: RunEvent, received: Received) {
        let Received { delegation_ended, notice_turn_ended } = received;
        self.receive_antigravity(&value);
        if value.kind == "resume"
            && let Some(resumed) = &value.resume_id
        {
            self.send(ServiceEvent::RememberSession(resumed.clone()));
        }
        let hooks = self.delegation.clone();
        let event = hooks.is_some().then(|| value.clone());
        self.send(ServiceEvent::Run { event: Box::new(value), notice_turn_ended });
        // After the host heard it (C# `RunEventReceived`, then `OfferDelegationPending` and `KickQueuedMerges`).
        if let (Some(hooks), Some(event)) = (hooks, event) {
            hooks.received(&event, delegation_ended, notice_turn_ended);
        }
    }

    /// `ReceiveToolPermission`: a request waiting or settled, kept per pane and handed to the host.
    fn tool_permission(&self, value: ToolPermissionRequest) {
        {
            let mut pending = lock(&self.pending);
            let pane = pending.entry(value.run_id.clone()).or_default();
            if value.state == "pending" {
                pane.insert(value.id.clone(), value.clone());
            } else {
                pane.shift_remove(&value.id);
            }
            if pane.is_empty() {
                pending.remove(&value.run_id);
            }
        }
        self.send(ServiceEvent::ToolPermission(Box::new(value)));
    }
}

/// The run half of C# `DesktopService`.
pub struct RunService {
    shared: Arc<Shared>,
    manager: RunManager,
    remote: Option<Arc<dyn RemoteRunner>>,
    admitting: Option<Admitting>,
    /// The remote mirror's panes (W9-J), set once by the app ([`RunService::set_mirror`]).
    mirror: OnceLock<Arc<dyn MirrorRunner>>,
    /// Reads the body of a pane the load left on disk (the store's `load_pane_body`), set once by the app.
    body_loader: OnceLock<BodyLoader>,
}

/// Reads a pane's body from its file: the pane whole, or None when there is nothing to read (it is whole already).
pub type BodyLoader = Arc<dyn Fn(&RunSession) -> Option<RunSession> + Send + Sync>;

/// How the service is made.
pub struct RunServiceOptions {
    pub plugin_directory: String,
    /// The host can show the approval card (a Claude run then asks it over stdio).
    pub permission_card: bool,
    pub remote: Option<Arc<dyn RemoteRunner>>,
    /// Plugs the graph tracker, the image store, the Agent IO binding and the file originals into the runner.
    pub configure: Option<Configure>,
    /// Called with the pane's id once its route is marked started, just before the runner admits the request, under the service's
    /// lock (a stop asked for meanwhile waits for the admission). The checks' gate on that window; None in the app.
    pub admitting: Option<Admitting>,
    /// A pane's Agent IO and delegation binding (C# `AgentIOBinding`, W7-C: src-tauri agent_io.rs `factory`), put into the runner's
    /// options before [`RunServiceOptions::configure`] runs.
    pub binding: Option<BindingFactory>,
    /// Delegation's run ledger and child worktrees (C# DesktopService.Delegation.cs, W7-D: src-tauri delegation.rs `run_hooks`). None:
    /// no pane is a delegated child's and no run is recorded.
    pub delegation: Option<Arc<dyn DelegationRunHooks>>,
}

/// What delegation hears of the runs the service starts and ends (C# DesktopService.cs:189, 261-275, 346-351;
/// DesktopService.Delegation.cs:196-218, 369-371). `worktree_root`, `began` and `ended` are called under the service's lock, so none may
/// take it again or wait; `received` and `pane_closed` after it is let go.
/// Owner from W7 Step 0: W7-D, which also places the calls of [`DelegationRunHooks::began`] and [`DelegationRunHooks::ended`]
/// (C#: after the route is added, and where a status event ends the route).
pub trait DelegationRunHooks: Send + Sync {
    /// C# `DelegationWorktreeRoot(pane)`: the child's worktree as delegation made it (a Codex or Antigravity child working in a folder
    /// inside it), never what the caller says; None for every other pane.
    fn worktree_root(&self, pane: &RunSession) -> Option<String>;
    /// C# `DelegationRunBegan(pane)`: a run starts in the pane; the run's id in delegation's ledger (`"run-" + guid`).
    fn began(&self, pane: &RunSession) -> String;
    /// C# `DelegationRunEnded(sessionId, status)`: a run of the pane ended with `status` (`closing`: the app quits); false when no run of
    /// this launch was going there.
    fn ended(&self, session_id: &str, status: &str, closing: bool) -> bool;
    /// The end of C# `Receive` (DesktopService.cs:271-275), after the event went to the host and with the service's lock let go: `ended`
    /// is what [`DelegationRunHooks::ended`] answered for this event, `notice_turn_ended` that a notice turn's answer came. C# offers
    /// delegation's pending items again on a run's start, a run's end or a notice turn's end, and moves the queued merges on after an
    /// end. Never waits.
    fn received(&self, event: &RunEvent, ended: bool, notice_turn_ended: bool) {
        let _ = (event, ended, notice_turn_ended);
    }
    /// C# `UpdateAsync`'s `PaneClosed(pane)` (DesktopService.cs:189): the pane left the state through [`RunService::update`], however it
    /// was closed (the window, the phone, its workspace removed). Called with the service's lock let go; never waits.
    fn pane_closed(&self, pane: &RunSession) {
        let _ = pane;
    }
}

pub type Configure = Box<dyn FnOnce(&mut RunManagerOptions) + Send>;
pub type Admitting = Arc<dyn Fn(&str) + Send + Sync>;

impl RunService {
    /// A service over `snapshot` (the loaded state), its events on the returned receiver.
    pub fn new(
        snapshot: AppSnapshot,
        providers: Arc<ProviderCatalog>,
        options: RunServiceOptions,
    ) -> (RunService, mpsc::UnboundedReceiver<ServiceEvent>) {
        let (events, receiver) = mpsc::unbounded_channel();
        let (leftovers_done, leftovers_removed) = tokio::sync::watch::channel(false);
        let shared = Arc::new(Shared {
            state: Mutex::new(State { snapshot, routes: HashMap::new(), closing: false }),
            events,
            pending: Mutex::new(HashMap::new()),
            antigravity: Mutex::new(antigravity::Desk::default()),
            antigravity_rules: OnceLock::new(),
            leftovers_removed,
            delegation: options.delegation,
        });
        let resolver = shared.clone();
        let resolve: crate::manager::ResolveWorkspace = Arc::new(move |id: String| {
            let shared = resolver.clone();
            Box::pin(async move { resolve_local(&shared, &id) })
        });
        let receiver_shared = shared.clone();
        let emit: crate::manager::Emit = Arc::new(move |event| receiver_shared.receive(event));
        let mut manager_options = RunManagerOptions::new(&options.plugin_directory, resolve, emit);
        if options.permission_card {
            let permissions = shared.clone();
            manager_options.permission_requested = Some(Arc::new(move |value| permissions.tool_permission(value)));
        }
        manager_options.binding = options.binding;
        // C# `AttachRemote`: the client's events and tool requests arrive as a local run's do.
        if let Some(remote) = &options.remote {
            let (events, permissions) = (shared.clone(), shared.clone());
            remote.attach(RemoteSinks {
                event: Arc::new(move |value| events.receive(value)),
                permission: Arc::new(move |value| permissions.tool_permission(value)),
            });
        }
        if let Some(configure) = options.configure {
            configure(&mut manager_options);
        }
        // The runner judges a card's file rules with the same leases (C5: refusing ones when the host gave none).
        let rules =
            shared.antigravity_rules.get_or_init(|| manager_options.antigravity_rules.clone().unwrap_or_else(refusing_rules)).clone();
        manager_options.antigravity_rules = Some(rules.clone());
        antigravity::remove_leftovers(rules, leftovers_done);
        let manager = RunManager::new(providers, manager_options);
        (
            RunService {
                shared,
                manager,
                remote: options.remote,
                admitting: options.admitting,
                mirror: OnceLock::new(),
                body_loader: OnceLock::new(),
            },
            receiver,
        )
    }

    pub fn manager(&self) -> &RunManager {
        &self.manager
    }

    /// The remote mirror's panes go through `mirror` from now on (W9-J; set once, a later call changes nothing).
    pub fn set_mirror(&self, mirror: Arc<dyn MirrorRunner>) {
        let _ = self.mirror.set(mirror);
    }

    /// C# `MirrorReference(id) is not null`: None when the pane is no mirrored pane; else the mirror it goes through, or the refusal when
    /// none is attached: a mirrored pane never falls through to a run of this PC or the client's runs.
    fn mirror_of(&self, session_id: &str) -> Option<Result<Arc<dyn MirrorRunner>>> {
        if !lock(&self.shared.state).snapshot.session(session_id).is_some_and(|s| s.mirror.is_some()) {
            return None;
        }
        Some(self.mirror.get().cloned().ok_or_else(|| RunError::State(strings::get("remote.mirror.error.unavailable"))))
    }

    /// C# `ToolPermissionChanged` raised by the remote mirror: a mirrored pane's waiting request (its run id the pane's) shown as a card,
    /// or withdrawn (any state but `pending`).
    pub fn mirror_permission(&self, value: ToolPermissionRequest) {
        self.shared.tool_permission(value);
    }

    /// The remote mirror's own change to the state (C# `snapshot = StateStore.Normalize(next, false)` under the lock, without a save:
    /// the mirror's workspaces and panes are never saved). `change` gives None to leave the state as it is. False when it did not change
    /// (or the service is closing).
    pub fn mirror_update(&self, change: impl FnOnce(&AppSnapshot) -> Option<AppSnapshot>) -> bool {
        let mut state = lock(&self.shared.state);
        if state.closing {
            return false;
        }
        let Some(changed) = change(&state.snapshot) else { return false };
        match normalize::normalize(&changed, false) {
            Ok(next) => {
                state.snapshot = next;
                true
            }
            Err(_) => false,
        }
    }

    /// The app's reader of the panes' bodies the load left on disk (set once; a later call changes nothing).
    pub fn set_body_loader(&self, loader: BodyLoader) {
        let _ = self.body_loader.set(loader);
    }

    /// Reads the bodies of the panes in `ids` the load left on disk, so whatever reads or changes them next has them whole. The files
    /// are read with the lock let go; a pane changed meanwhile keeps its change (read again under the lock). True when a pane changed.
    pub fn ensure_bodies(&self, ids: &[&str]) -> bool {
        let Some(loader) = self.body_loader.get().cloned() else { return false };
        let waiting: Vec<Arc<RunSession>> = {
            let state = lock(&self.shared.state);
            state.snapshot.sessions().iter().filter(|s| s.body_on_disk && ids.contains(&s.id())).cloned().collect()
        };
        if waiting.is_empty() {
            return false;
        }
        let read: Vec<(Arc<RunSession>, RunSession)> =
            waiting.into_iter().filter_map(|pane| loader(&pane).map(|whole| (pane, whole))).collect();
        let mut state = lock(&self.shared.state);
        let mut changed = false;
        for (before, whole) in read {
            let Some(at) = state.snapshot.sessions().iter().position(|s| s.id() == before.id()) else { continue };
            let now = state.snapshot.sessions()[at].clone();
            if !now.body_on_disk {
                continue;
            }
            let next = if Arc::ptr_eq(&now, &before) { whole } else { with_additions(whole, &now) };
            state.snapshot.sessions_mut()[at] = Arc::new(next);
            changed = true;
        }
        changed
    }

    /// The panes of `workspace` whose body is still on disk, read ([`RunService::ensure_bodies`]).
    pub fn ensure_workspace_bodies(&self, workspace: &str) -> bool {
        let ids: Vec<String> = {
            let state = lock(&self.shared.state);
            state
                .snapshot
                .sessions()
                .iter()
                .filter(|s| s.body_on_disk && s.workspace_id() == workspace)
                .map(|s| s.id().to_string())
                .collect()
        };
        self.ensure_bodies(&ids.iter().map(String::as_str).collect::<Vec<_>>())
    }

    /// The state as the service has it now.
    pub fn snapshot(&self) -> AppSnapshot {
        lock(&self.shared.state).snapshot.clone()
    }

    /// The state changed elsewhere (the host's own changes): the service applies its events to this from now on.
    pub fn replace(&self, snapshot: AppSnapshot) {
        lock(&self.shared.state).snapshot = snapshot;
    }

    /// `UpdateAsync`: a change normalized as C# normalizes every change, then saved.
    pub fn update(&self, change: impl FnOnce(&AppSnapshot) -> AppSnapshot) -> Result<()> {
        self.try_update(|current| Some(change(current))).map(|_| ())
    }

    /// [`RunService::update`] for a change that may not happen: `change` gives None to leave the state as it is, unsaved (C#'s
    /// `snapshot = StateStore.Normalize(next, false); QueueSave()` after a check under the lock, as delegation's `CreatePane` and `Note`
    /// do). True when it changed.
    pub fn try_update(&self, change: impl FnOnce(&AppSnapshot) -> Option<AppSnapshot>) -> Result<bool> {
        let (closed, looking): (Vec<Arc<RunSession>>, Option<String>) = {
            let mut state = lock(&self.shared.state);
            if state.closing {
                return Err(RunError::State("the service is closing".into()));
            }
            let Some(changed) = change(&state.snapshot) else { return Ok(false) };
            let next = normalize::normalize(&changed, false).map_err(|e| RunError::Invalid(e.to_string()))?;
            let closed = state.snapshot.sessions().iter().filter(|before| next.session(before.id()).is_none()).cloned().collect();
            let moved = next.active_workspace_id != state.snapshot.active_workspace_id
                || next.active_session_id != state.snapshot.active_session_id;
            let looking = if moved { next.active_workspace_id.clone() } else { None };
            state.snapshot = next;
            (closed, looking)
        };
        // Looking at a mirror workspace or one of its panes reads its host at once (C# `WakeMirrorFor`).
        if let (Some(workspace), Some(mirror)) = (looking, self.mirror.get()) {
            mirror.looking_at(&workspace);
        }
        // A closed pane's Antigravity card and reason go with it (C# `ForgetAntigravity`).
        // A delegated child's close reaches delegation (C# `PaneClosed`).
        for pane in closed {
            self.shared.forget_antigravity(pane.id());
            if let Some(hooks) = &self.shared.delegation {
                hooks.pane_closed(&pane);
            }
        }
        self.shared.send(ServiceEvent::SaveNow);
        Ok(true)
    }

    /// C#'s private `Receive` as the perf scenario reaches it by reflection (MainWindow.PerfScenario.cs `PerfReceive`): one run event
    /// applied through the service's own path, as a run's reader hands it in. Only the timing scenario calls it (src-tauri perf.rs, W9-H);
    /// hidden from the docs, as C#'s is private.
    #[doc(hidden)]
    pub fn receive_for_perf(&self, value: RunEvent) {
        self.shared.receive(value);
    }

    /// C# `closing`: the service began to close ([`RunService::dispose`]).
    pub fn closing(&self) -> bool {
        lock(&self.shared.state).closing
    }

    /// C# `routes.ContainsKey(id)`: a run of the pane is starting or going (its route ends with its status event).
    pub fn has_route(&self, id: &str) -> bool {
        lock(&self.shared.state).routes.contains_key(id)
    }

    /// `UpdateDraft` (DesktopService.cs:198): a pane's draft, cleaned to at most 100,000 characters, replaced on that pane alone without
    /// the whole state's normalize and saved on the schedule (a draft streams as it is typed). False when the pane is gone or its draft
    /// is that already.
    pub fn update_draft(&self, session_id: &str, draft: &str) -> Result<bool> {
        {
            let mut state = lock(&self.shared.state);
            if state.closing {
                return Err(RunError::State("the service is closing".into()));
            }
            let bounded = wire::clean(draft, 100_000);
            let Some(at) = state.snapshot.sessions().iter().position(|s| s.id() == session_id) else { return Ok(false) };
            if state.snapshot.sessions()[at].draft.as_deref().unwrap_or("") == bounded {
                return Ok(false);
            }
            let mut next = (*state.snapshot.sessions()[at]).clone();
            next.draft = Some(bounded);
            state.snapshot.sessions_mut()[at] = Arc::new(next);
        }
        self.shared.send(ServiceEvent::SaveSoon);
        Ok(true)
    }

    /// One pane replaced on its own, without the whole state's normalize: C#'s light updates of one pane (`SetShellStatus`,
    /// DesktopService.cs:165, a shell's status from its terminal; `UpdateShellBlockRun`, :219, a shell block's record while it
    /// streams). `change` gets the pane and gives back its next record and how it is saved, or None to change nothing. False when
    /// the service is closing, the pane is gone or `change` changed nothing.
    pub fn update_pane(&self, session_id: &str, change: impl FnOnce(&RunSession) -> Option<(RunSession, PaneSave)>) -> bool {
        self.ensure_bodies(&[session_id]);
        let save = {
            let mut state = lock(&self.shared.state);
            if state.closing {
                return false;
            }
            let Some(at) = state.snapshot.sessions().iter().position(|s| s.id() == session_id) else { return false };
            let Some((next, save)) = change(&state.snapshot.sessions()[at]) else { return false };
            state.snapshot.sessions_mut()[at] = Arc::new(next);
            save
        };
        match save {
            PaneSave::Now => self.shared.send(ServiceEvent::SaveNow),
            PaneSave::Soon => self.shared.send(ServiceEvent::SaveSoon),
            PaneSave::Later => {}
        }
        true
    }

    /// C# `PaneView.ClearToolPermissions` (MainWindow.cs:88): a pane whose run ended (stopped, completed, error) has no request waiting
    /// any more; the window drops them all, whether or not the channel settled each. False when none was waiting.
    pub fn clear_permissions(&self, session_id: &str) -> bool {
        lock(&self.shared.pending).remove(session_id).is_some()
    }

    /// The waiting tool-permission requests of a pane, oldest first.
    pub fn pending_permissions(&self, session_id: &str) -> Vec<ToolPermissionRequest> {
        lock(&self.shared.pending).get(session_id).map(|p| p.values().cloned().collect()).unwrap_or_default()
    }

    /// C# `RemotePane`: the pane's run goes through the client: it runs there now, or its workspace is a remote one.
    fn remote_pane(&self, session_id: &str) -> bool {
        if self.remote.as_ref().is_some_and(|r| r.is_running(session_id)) {
            return true;
        }
        let state = lock(&self.shared.state);
        let snapshot = &state.snapshot;
        // A mirror workspace's panes are their host's own, driven through the mirror ([`MirrorRunner`]), never the client's runs.
        snapshot
            .session(session_id)
            .and_then(|s| snapshot.workspace(s.workspace_id()))
            .and_then(|w| w.remote.as_ref())
            .is_some_and(|r| !r.mirror)
    }

    /// C# `ResolveSharedAsync` (a sharing host's `ResolveLocal`): a registered local workspace whose folder exists, or why not.
    pub fn resolve_shared(&self, id: &str) -> std::result::Result<Workspace, String> {
        resolve_local(&self.shared, id).map_err(|e| e.to_string())
    }

    /// Whether a pane of this computer runs `provider`'s CLI now (`HasActiveProvider`).
    pub fn has_active_provider(&self, provider: &str) -> bool {
        let state = lock(&self.shared.state);
        let snapshot = &state.snapshot;
        snapshot.sessions().iter().any(|s| {
            s.kind() == "claude"
                && s.provider() == provider
                && !snapshot.workspaces().iter().any(|w| w.id() == s.workspace_id() && w.remote.is_some())
                && (s.status() == "running" || state.routes.contains_key(s.id()))
        })
    }

    pub fn is_session_running(&self, id: &str) -> bool {
        self.manager.is_running(id) || self.remote.as_ref().is_some_and(|r| r.is_running(id))
    }

    /// `StartAsync`.
    pub async fn start(&self, value: StartRunRequest) -> Result<Option<()>> {
        self.launch(value, None, false).await.map(|run| run.map(|_| ()))
    }

    /// `StartReachedAsync`: returns once the run's process is up with its input open.
    pub async fn start_reached(&self, value: StartRunRequest) -> Result<Option<()>> {
        self.launch(value, None, true).await.map(|run| run.map(|_| ()))
    }

    /// `Launch`: the request recorded on its pane and handed to the runner (or the workspace's host). The run's id as delegation names
    /// it (C# `DelegationRunBegan`, [`DelegationRunHooks::began`]; without hooks an id of its own, `"run-" + guid`); None when it was
    /// stopped before it started. `title_from`: what an automatic title follows when it is not the input (`Some("")` for a run
    /// delegation starts on its own: none).
    pub async fn launch(&self, value: StartRunRequest, title_from: Option<&str>, reach: bool) -> Result<Option<String>> {
        // A run adds to its pane's history: the pane's body is read first if the load left it on disk.
        self.ensure_bodies(&[value.session_id.as_str()]);
        // A mirrored pane's request is the host pane's next one, as the phone's send: nothing runs or changes here.
        if let Some(mirror) = self.mirror_of(&value.session_id) {
            let mirror = mirror?;
            let attachments = value.attachment_list().map(|list| list.to_vec()).unwrap_or_default();
            mirror.submit(value.session_id.clone(), value.input.clone(), attachments).await?;
            return Ok(None);
        }
        let mut request = value.validate()?;
        let workspace;
        let remote;
        let run_id;
        let holder;
        {
            let mut state = lock(&self.shared.state);
            if state.closing {
                return Err(RunError::State("the service is closing".into()));
            }
            let found = state.snapshot.workspace(&request.workspace_id).cloned();
            workspace = found.ok_or_else(|| RunError::Invalid(strings::get("run.error.workspaceNotRegistered")))?;
            remote = workspace.remote.is_some();
            if remote && request.kind != "shell" && providers::is_local_only(&request.provider) {
                return Err(RunError::State(strings::get(&format!("run.blocked.{}Remote", request.provider))));
            }
            let pane = state
                .snapshot
                .sessions()
                .iter()
                .find(|s| s.id() == request.session_id && s.workspace_id() == request.workspace_id && s.kind() == request.kind)
                .cloned()
                .ok_or_else(|| RunError::Invalid(strings::get("run.error.sessionNotInWorkspace")))?;
            // A delegated child runs only in its own worktree, never in its parent's checkout.
            if (pane.parent_session_id.is_some() || pane.working_folder.is_some())
                && !pane.working_folder.as_deref().is_some_and(|f| Path::new(f).is_dir())
            {
                return Err(RunError::State(strings::get("run.blocked.worktreeMissing")));
            }
            request.phase_models = phase_models::normalized(state.snapshot.phase_models.as_ref());
            // 에이전트 창's checklist switch, for the agents it asks, on this computer (a remote workspace's host asks by its own switch).
            request.checklist =
                !remote && request.kind == "claude" && crate::checklist::asks(&request.provider) && state.snapshot.agent_checklist_on();
            request.todo_progress = if request.resume_id.is_none() { None } else { pane.todo_progress.clone() };
            // A terminal pane keeps a folder too (a pane worktree's terminal), but only an agent pane's run works in it.
            request.working_folder = if pane.kind() == "claude" { pane.working_folder.clone() } else { None };
            // C# `DelegationWorktreeRoot(pane)`: the child's worktree as delegation made it, never what the caller says.
            request.delegation_worktree_root = self.shared.delegation.as_ref().and_then(|hooks| hooks.worktree_root(&pane));
            // A pane worktree's own PORT and folders (C# `PaneWorktree.Environment`), never for a remote workspace's pane.
            request.pane_environment = (pane.kind() == "claude" && !remote)
                .then(|| pane.pane_worktree.as_ref().map(|worktree| worktree.environment(workspace.path()).into_iter().collect()))
                .flatten();
            if state.routes.contains_key(&request.session_id) {
                return Err(RunError::State(strings::get("run.error.alreadyRunning")));
            }
            holder = antigravity::next_holder();
            state.routes.insert(request.session_id.clone(), Route { holder, ..Route::default() });
            // After the route is added, as C# `DelegationRunBegan(pane)`: a child's run is recorded running before anything else of it.
            run_id = match &self.shared.delegation {
                Some(hooks) => hooks.began(&pane),
                None => format!("run-{}", wire::new_id()),
            };
            let title_input = if request.antigravity_grant.is_some() { "" } else { title_from.unwrap_or(&request.input) };
            let now = DateTimeOffset::now();
            for session in state.snapshot.sessions_mut().iter_mut() {
                if session.id() != request.session_id {
                    continue;
                }
                let mut next: RunSession = requested(session, title_input);
                if request.resume_id.is_none() || session.provider() != request.provider {
                    next.session_usage = None;
                }
                next.provider = Some(request.provider.clone());
                next.current_activity = None;
                next.run_timing = if request.kind == "shell" { None } else { Some(timing::begin(Some(now))) };
                *session = Arc::new(next);
            }
            let mut history = request.input.clone();
            if request.attachment_list().is_some() {
                history.push_str("\n\n");
                history.push_str(&attachments::summary(request.attachment_list()));
            }
            let named = (request.kind == "claude").then_some(request.provider.as_str());
            let asked = RunEvent::log(&request.session_id, "user", &history, named);
            // A pane worktree's agent is told once, with its first request that is no slash command, where it works and that it commits
            // its work (C# `PaneWorktreeNote`). The transcript keeps the request as sent; only the CLI gets the note.
            if !remote
                && request.kind == "claude"
                && let Some(worktree) = pane.pane_worktree.as_ref().filter(|worktree| worktree.note_applies(&request.input))
            {
                request.input = worktree.with_note(&request.input);
                for session in state.snapshot.sessions_mut().iter_mut() {
                    if session.id() == request.session_id && session.pane_worktree.is_some() {
                        let mut next = RunSession::clone(session);
                        if let Some(held) = next.pane_worktree.as_mut() {
                            held.note_given = Some(true);
                        }
                        *session = Arc::new(next);
                    }
                }
            }
            // The run's diagram block is named here, so a send can answer with it before the runner's first event.
            request.graph_run_id =
                (!remote && request.kind != "shell" && GRAPH_PROVIDERS.contains(&request.provider.as_str())).then(wire::new_id);
            state.snapshot = apply::apply(&state.snapshot, &RunEvent::state(&request.session_id, "running", None), now);
            state.snapshot = apply::apply(&state.snapshot, &asked, now);
            if let Some(receipt) = &request.receipt {
                let mut receipt = lock(receipt);
                receipt.entry_id = asked.entry.as_ref().and_then(|e| e.id.clone());
                receipt.graph_run_id = request.graph_run_id.clone();
            }
        }
        self.shared.send(ServiceEvent::SaveNow);
        self.shared.send(ServiceEvent::RequestStarting(Box::new(request.clone())));
        self.shared
            .send(ServiceEvent::Run { event: Box::new(RunEvent::state(&request.session_id, "running", None)), notice_turn_ended: false });
        let session_id = request.session_id.clone();
        // An allowed Antigravity card's rules are in place before its run starts; a remote pane's host puts them in its own settings.
        let held = if remote { Ok(()) } else { self.hold_antigravity_rules(&request).await };
        let started = match held {
            Err(error) => Err(error),
            Ok(()) => {
                let admitted = {
                    let mut state = lock(&self.shared.state);
                    let cancelled = state.closing || state.routes.get(&session_id).is_none_or(|r| r.cancelled);
                    if cancelled {
                        let stopped = RunEvent::state(&session_id, "stopped", None);
                        let received = self.shared.receive_locked(&mut state, &stopped);
                        drop(state);
                        self.shared.after_receive(stopped, received);
                        if request.attachment_list().is_some() {
                            return Err(RunError::Cancelled(strings::get("run.error.attachmentSendCancelled")));
                        }
                        return Ok(None);
                    }
                    if let Some(route) = state.routes.get_mut(&session_id) {
                        route.started = true;
                    }
                    if let Some(admitting) = &self.admitting {
                        admitting(&session_id);
                    }
                    // The runner registers the run under this lock, as C# starts `local.StartAsync` inside it: a stop that finds the
                    // route started always finds the run too.
                    (!remote).then(|| self.manager.admit(request.clone(), reach))
                };
                match admitted {
                    Some(admitted) => match admitted {
                        Ok(admitted) => admitted.reached().await.map(Some),
                        Err(error) => Err(error),
                    },
                    // A remote workspace's run goes to its host through the client, never to this PC's runner, and never under the lock.
                    None => return self.launch_remote(request, workspace, run_id, holder).await,
                }
            }
        };
        match started {
            Ok(Some(())) => Ok(Some(run_id)),
            Ok(None) => {
                self.shared.receive(RunEvent::state(&session_id, "stopped", None));
                Ok(None)
            }
            Err(RunError::Cancelled(message)) => {
                self.shared.receive(RunEvent::state(&session_id, "stopped", None));
                Err(RunError::Cancelled(message))
            }
            Err(error) => {
                self.shared.receive(RunEvent::log(&session_id, "error", error.message(), None));
                self.shared.receive(RunEvent::state(&session_id, "error", None));
                self.shared.continuation_failed(&request, error.message());
                Err(error)
            }
        }
    }

    /// The rest of `Launch` for a remote pane: its last word is said only while the route is still its own (a stop already ended it, and
    /// a newer send's route is never ended here).
    async fn launch_remote(&self, request: StartRunRequest, workspace: Workspace, run_id: String, holder: u64) -> Result<Option<String>> {
        let session_id = request.session_id.clone();
        match self.start_remote(&request, workspace, holder).await {
            Ok(true) => Ok(Some(run_id)),
            // Stopped before the host took it; the client's stop may already have ended it, and a newer send may have begun.
            Ok(false) => {
                self.say_if_owned(&session_id, holder, vec![RunEvent::state(&session_id, "stopped", None)]);
                Ok(None)
            }
            Err(RunError::Cancelled(message)) => {
                self.say_if_owned(&session_id, holder, vec![RunEvent::state(&session_id, "stopped", None)]);
                Err(RunError::Cancelled(message))
            }
            Err(error) => {
                self.say_if_owned(
                    &session_id,
                    holder,
                    vec![RunEvent::log(&session_id, "error", error.message(), None), RunEvent::state(&session_id, "error", None)],
                );
                self.shared.continuation_failed(&request, error.message());
                Err(error)
            }
        }
    }

    /// C# `StartRemoteAsync`: connected first, on a connection that still shares the workspace, unless stopped meanwhile. False when a
    /// stop came before the host took it; with attachments that is a cancellation instead, as a local start's is, so the caller keeps them.
    async fn start_remote(&self, request: &StartRunRequest, workspace: Workspace, holder: u64) -> Result<bool> {
        let Some(client) = &self.remote else { return Err(RunError::State(strings::get("remote.run.error.notConnected"))) };
        client.connect(workspace.clone()).await?;
        let cancelled =
            |state: &State| state.closing || state.routes.get(&request.session_id).is_none_or(|r| r.holder != holder || r.cancelled);
        let started = if cancelled(&lock(&self.shared.state)) {
            Err(RunError::Cancelled(String::new()))
        } else {
            client.start(request.clone(), workspace).await
        };
        match started {
            Ok(()) => {}
            Err(RunError::Cancelled(_)) => {
                if request.attachment_list().is_some() {
                    return Err(RunError::Cancelled(strings::get("run.error.attachmentSendCancelled")));
                }
                return Ok(false);
            }
            Err(error) => return Err(error),
        }
        // A stop that came after the check above but before the client listed the run found nothing to stop there: it is stopped now.
        if cancelled(&lock(&self.shared.state)) {
            let _ = client.stop(request.session_id.clone()).await;
        }
        Ok(true)
    }

    /// C# `SayIfOwned`: `events` applied only while route `holder` is still the pane's, checked and applied under one lock.
    fn say_if_owned(&self, session_id: &str, holder: u64, events: Vec<RunEvent>) {
        let mut received = vec![];
        {
            let mut state = lock(&self.shared.state);
            if state.routes.get(session_id).is_none_or(|r| r.holder != holder) {
                return;
            }
            for value in events {
                let applied = self.shared.receive_locked(&mut state, &value);
                received.push((value, applied));
            }
        }
        for (value, applied) in received {
            self.shared.after_receive(value, applied);
        }
    }

    /// C# `AnswerRemote`: an answer to a remote pane's request goes out without holding the caller. Only an answer that failed is said,
    /// in the pane, and only while the run it was sent for is still the pane's.
    fn answer_remote(
        &self,
        session_id: &str,
        answer: impl FnOnce(Arc<dyn RemoteRunner>) -> BoxFuture<'static, Result<()>> + Send + 'static,
    ) {
        let Some(client) = self.remote.clone() else { return };
        let holder = lock(&self.shared.state).routes.get(session_id).map(|r| r.holder);
        let shared = self.shared.clone();
        let session_id = session_id.to_string();
        tokio::spawn(async move {
            if let (Err(error), Some(holder)) = (answer(client).await, holder) {
                let event = RunEvent::log(&session_id, "error", error.message(), None);
                let received = {
                    let mut state = lock(&shared.state);
                    if state.routes.get(&session_id).is_none_or(|r| r.holder != holder) {
                        return;
                    }
                    shared.receive_locked(&mut state, &event)
                };
                shared.after_receive(event, received);
            }
        });
    }

    /// `StopAsync`: a route not started yet is marked, so its start stops; a started one is stopped by its runner.
    pub async fn stop(&self, id: &str) -> Result<()> {
        // A mirrored pane runs on its host: the stop goes there.
        if let Some(mirror) = self.mirror_of(id) {
            let mirror = mirror?;
            return mirror.stop(id.to_string()).await;
        }
        {
            let mut state = lock(&self.shared.state);
            let Some(route) = state.routes.get_mut(id) else { return Ok(()) };
            route.cancelled = true;
            if !route.started {
                return Ok(());
            }
        }
        if self.remote_pane(id)
            && let Some(remote) = &self.remote
        {
            return remote.stop(id.to_string()).await;
        }
        self.manager.stop(id).await
    }

    /// `RespondToToolPermission`: 이번만 허용 / 거부, the only way a request is ever answered.
    pub async fn respond_to_tool_permission(&self, session_id: &str, request_id: &str, allow: bool) -> Result<()> {
        // A mirrored pane's request is its host's: the answer goes there.
        if let Some(mirror) = self.mirror_of(session_id) {
            mirror?.respond_to_tool_permission(session_id.into(), request_id.into(), allow);
            return Ok(());
        }
        if self.remote_pane(session_id) && self.remote.is_some() {
            let (session, request) = (session_id.to_string(), request_id.to_string());
            self.answer_remote(session_id, move |client| {
                Box::pin(async move { client.respond_to_tool_permission(session, request, allow).await })
            });
            return Ok(());
        }
        self.manager.respond_to_tool_permission(session_id, request_id, allow)
    }

    /// `AnswerQuestionnaire`.
    pub async fn answer_questionnaire(
        &self,
        session_id: &str,
        request_id: &str,
        answers: &IndexMap<String, UserQuestionAnswer>,
    ) -> Result<()> {
        if let Some(mirror) = self.mirror_of(session_id) {
            mirror?.answer_questions(session_id.into(), request_id.into(), answers.clone());
            return Ok(());
        }
        if self.remote_pane(session_id) && self.remote.is_some() {
            let (session, request, answers) = (session_id.to_string(), request_id.to_string(), answers.clone());
            self.answer_remote(session_id, move |client| Box::pin(async move { client.answer_questions(session, request, answers).await }));
            return Ok(());
        }
        self.manager.answer_questionnaire(session_id, request_id, answers)
    }

    /// `AnswerPlan`: None for a remote pane (its record comes back as the host's event).
    pub async fn answer_plan(&self, session_id: &str, request_id: &str, decision: &PlanDecision) -> Result<Option<PlanRecord>> {
        if let Some(mirror) = self.mirror_of(session_id) {
            mirror?.answer_plan(session_id.into(), request_id.into(), decision.clone());
            return Ok(None);
        }
        if self.remote_pane(session_id) && self.remote.is_some() {
            let (session, request, decision) = (session_id.to_string(), request_id.to_string(), decision.clone());
            self.answer_remote(session_id, move |client| Box::pin(async move { client.answer_plan(session, request, decision).await }));
            return Ok(None);
        }
        self.manager.answer_plan(session_id, request_id, decision).map(Some)
    }

    /// `TrySteerAsync`: a notice handed to the pane's running local Claude turn, recorded as the user's line.
    pub fn try_steer(&self, session_id: &str, text: &str) -> bool {
        if self.mirror_of(session_id).is_some() || self.remote_pane(session_id) || !self.manager.try_steer(session_id, text) {
            return false;
        }
        self.shared.receive(RunEvent::log(session_id, "user", text, Some("claude")));
        true
    }

    /// `DisposeAsync`'s run part: every run stopped, then one more save.
    pub async fn dispose(&self) {
        lock(&self.shared.state).closing = true;
        // The client's runs stop first (C#: and whatever that does never keeps this PC's runs from shutting down).
        if let Some(remote) = &self.remote {
            remote.dispose().await;
        }
        self.manager.dispose().await;
        self.release_antigravity_rules().await;
        self.shared.send(ServiceEvent::SaveNow);
    }
}

/// `PaneTitle.Requested`: a pane whose title follows its requests takes the new one.
fn requested(session: &RunSession, input: &str) -> RunSession {
    let mut next = session.clone();
    if normalize::follows_requests(session)
        && let Some(title) = normalize::shortened(normalize::ask_question(input))
    {
        next.title = Some(title);
    }
    next
}

/// `ResolveLocal` and `StateStore.ResolveLocalAsync`: a local workspace the state has, its folder there.
fn resolve_local(shared: &Shared, id: &str) -> Result<Workspace> {
    let state = lock(&shared.state);
    let workspace =
        state.snapshot.workspace(id).cloned().ok_or_else(|| RunError::Invalid(strings::get("run.error.localWorkspaceNotRegistered")))?;
    if workspace.remote.is_some() {
        return Err(RunError::State(strings::get("run.error.approvedWorkspaceOnly")));
    }
    if !Path::new(workspace.path()).is_dir() {
        return Err(RunError::Io(strings::get("store.error.workspaceFolderNotFound")));
    }
    Ok(workspace)
}

/// A pane read whole from its file, with what was added to it meanwhile (while its body was being read) laid over by id.
fn with_additions(mut whole: RunSession, now: &RunSession) -> RunSession {
    let mut logs = whole.logs().to_vec();
    for entry in now.logs() {
        if !logs.iter().any(|e| e.id == entry.id) {
            logs.push(entry.clone());
        }
    }
    whole.logs = Some(logs);
    let mut runs = whole.graph_runs().to_vec();
    for run in now.graph_runs() {
        match runs.iter().position(|r| r.id == run.id) {
            Some(at) => runs[at] = run.clone(),
            None => runs.push(run.clone()),
        }
    }
    whole.graph_runs = Some(runs);
    RunSession {
        plan_history: now.plan_history.clone().or(whole.plan_history.take()),
        todo_progress: now.todo_progress.clone().or(whole.todo_progress.take()),
        background_work: now.background_work.clone().or(whole.background_work.take()),
        shell_block_runs: now.shell_block_runs.clone().or(whole.shell_block_runs.take()),
        // The pane's own fields as they are now; its body as read and added to.
        logs: whole.logs.take(),
        graph_runs: whole.graph_runs.take(),
        body_on_disk: false,
        stored_digest: None,
        ..now.clone()
    }
}
