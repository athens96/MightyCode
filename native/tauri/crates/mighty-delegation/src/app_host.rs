//! The panes' side of delegation (C# Core/DelegationAppHost.cs; M/DelegationAppHost.swift): `DelegationPanes` (the child session,
//! inserted beside its parent, the run's workspace, `RunRefusal`, the pane's state), the run ledger (`"run-" + guid`) and the event
//! pump. Pure rules over mighty-core's state, no OS: the desktop service ([`crate::app`]) applies them, the checks test them without
//! the app. Owner: W7-D.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

use mighty_composer::queue::is_agent_pane;
use mighty_core::add_pane;
use mighty_core::layout;
use mighty_core::model::{AppSnapshot, RunSession, RunSettings, Workspace};
use mighty_core::normalize;
use mighty_core::rename;
use mighty_core::wire;
use mighty_platform::BoxFuture;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::host::{DelegationChildPane, DelegationPaneActivity, DelegationPaneState};
pub use crate::reports::ChildRunEnd;
use crate::request::DelegationReasonCode;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// C# `DelegationPanes.ChildSession`: a new agent pane for `pane` in the workspace `workspace_id`, running `pane.provider` (the
/// child's agent). Its mode is the starting mode the parent asked for, written here once and never copied from the last-used pane; it
/// is linked to its parent and works in its worktree folder. Every child of an agent pane `parent` shows its requests the way the
/// parent does (basic or Mighty, and in Mighty the diagram, timeline or pipeline). A child of the parent's own agent that the human
/// started also takes the parent's model and effort, and a Claude one its Mighty style. A child of another agent that the human started
/// runs the model and effort a new pane of that agent would: those of `agent_defaults`, the agent's last-used pane, or its defaults
/// without one. Any child the parent's delegate made runs its agent's defaults.
pub fn child_session(
    pane: &DelegationChildPane,
    workspace_id: &str,
    title: &str,
    parent: Option<&RunSession>,
    agent_defaults: Option<&RunSession>,
) -> RunSession {
    let mut session = RunSession {
        id: Some(pane.session_id.clone()),
        workspace_id: Some(workspace_id.to_string()),
        title: Some(title.to_string()),
        kind: Some("claude".into()),
        provider: Some(pane.provider.clone()),
        settings: Some(RunSettings { permission_mode: pane.mode.clone(), ..RunSettings::default() }),
        parent_session_id: Some(pane.parent_session_id.clone()),
        working_folder: Some(pane.folder.clone()),
        ..RunSession::default()
    };
    let Some(parent) = parent.filter(|parent| is_agent_pane(parent.kind(), parent.provider())) else { return session };
    session.agent_view_mode = parent.agent_view_mode.clone();
    session.graph_view_mode = parent.graph_view_mode.clone();
    if !pane.started_by_human {
        return session;
    }
    // MC-76: a child the person starts takes every setting of the pane it copies (effort, fast mode, web search, turns, budget), as a new
    // pane takes its agent's last-used one; its permission stays the mode its record starts in.
    let settings_of = |from: &RunSession| RunSettings { permission_mode: pane.mode.clone(), ..from.settings.clone().unwrap_or_default() };
    if parent.provider() != pane.provider {
        if let Some(defaults) = agent_defaults.filter(|defaults| defaults.provider() == pane.provider) {
            session.model = defaults.model.clone();
            session.settings = Some(settings_of(defaults));
        }
        return session;
    }
    session.model = parent.model.clone();
    session.settings = Some(settings_of(parent));
    // The same approved bytes; its state starts over at the child's own first request.
    if pane.provider == "claude" {
        session.mighty_style = parent.mighty_style.clone();
        session.mighty_style_hash = parent.mighty_style_hash.clone();
    }
    session
}

/// C# `DelegationPanes.AddingChild`: `state` with the child's pane as a tab beside its parent. Its title is its task `task`, kept as
/// the pane's name, and its parent's tab group keeps showing what it showed, except that a child the human started shows at once,
/// selected and active, while the human is still on its parent with nothing typed there. The layout is saved as it then is, and the
/// workspace leaves a preset layout mode, as when a human adds a tab; focus stays focus. A `quiet` child (the phone's 바로 실행) is
/// never selected: the phone's gesture does not move this window's view. None, changing nothing, when no pane can be made: the app is
/// at its pane cap, the id is taken, or the parent pane or its workspace is gone.
pub fn adding_child(state: &AppSnapshot, pane: &DelegationChildPane, task: Option<&str>, quiet: bool) -> Option<AppSnapshot> {
    if !add_pane::has_room(state) || state.session(&pane.session_id).is_some() {
        return None;
    }
    let parent = state.session(&pane.parent_session_id)?.clone();
    let workspace = parent.workspace_id();
    state.workspace(workspace)?;
    // A child of another agent takes what a new pane of that agent would: its last-used top-level pane's model and effort.
    let agent_defaults = (parent.provider() != pane.provider)
        .then(|| {
            let target = RunSession {
                id: Some(pane.session_id.clone()),
                workspace_id: Some(workspace.to_string()),
                kind: Some("claude".into()),
                provider: Some(pane.provider.clone()),
                ..RunSession::default()
            };
            add_pane::template(&target, state.sessions()).cloned()
        })
        .flatten();
    let title = task.and_then(normalize::shortened).unwrap_or_else(|| normalize::provider_name(&pane.provider).to_string());
    let mut child = child_session(pane, workspace, &title, Some(&parent), agent_defaults.as_ref());
    child.title_mode = Some(rename::FIXED.into());
    let open: Vec<String> = state.sessions().iter().filter(|s| s.workspace_id() == workspace).map(|s| s.id().to_string()).collect();
    let mode = state
        .pane_layout_modes
        .as_ref()
        .and_then(|modes| modes.get(workspace))
        .cloned()
        .flatten()
        .unwrap_or_else(|| state.layout().to_string());
    let active = if state.active_workspace_id.as_deref() == Some(workspace) {
        state.active_session_id.clone()
    } else {
        state.pane_layout_active_session_ids.as_ref().and_then(|ids| ids.get(workspace)).cloned().flatten()
    };
    let saved = state.pane_layouts.as_ref().and_then(|layouts| layouts.get(workspace)).cloned().flatten();
    let preset = saved.or_else(|| layout::preset(&open, &mode, active.as_deref()));
    let root = layout::normalize(preset.as_ref(), &open, None)?;
    let group = layout::groups(&root).into_iter().find(|tabs| tabs.session_ids().any(|id| id == parent.id()))?.clone();
    let child_id = child.id().to_string();
    let with_child: Vec<String> = open.iter().cloned().chain([child_id.clone()]).collect();
    let index = group.session_ids().position(|id| id == parent.id()).map_or(0, |at| at as i64 + 1);
    // A tab moved into a group is selected there; the group goes back to the tab it showed.
    let next = layout::move_tab(&layout::normalize(Some(&root), &with_child, None)?, &child_id, group.id(), "center", Some(index));
    let shows = pane.started_by_human
        && !quiet
        && state.active_session_id.as_deref() == Some(parent.id())
        && parent.draft.as_deref().unwrap_or("").is_empty();
    let selected = if shows { child_id.clone() } else { group.selected_session_id.clone().unwrap_or_else(|| parent.id().to_string()) };
    let next = layout::select(&next, &selected);
    let mut changed = state.clone();
    if shows {
        changed.active_session_id = Some(child_id);
    }
    changed.sessions_mut().push(Arc::new(child));
    let next_mode = if mode == "focus" {
        mode
    } else if next.kind() == "split" {
        "custom".to_string()
    } else {
        "tabs".to_string()
    };
    changed.pane_layouts.get_or_insert_with(Default::default).insert(workspace.to_string(), Some(next));
    changed.pane_layout_modes.get_or_insert_with(Default::default).insert(workspace.to_string(), Some(next_mode));
    Some(changed)
}

/// C# `DelegationPanes.RunWorkspace`: the workspace a run of `session` starts with. A child's run, its terminal tool and its MCP
/// binding all work in the child's worktree folder; any other pane's in the workspace folder.
pub fn run_workspace(session: &RunSession, workspace: &Workspace) -> Workspace {
    match &session.working_folder {
        Some(folder) => Workspace { path: Some(folder.clone()), ..workspace.clone() },
        None => workspace.clone(),
    }
}

/// C# `DelegationPanes.RunRefusal`: `worktree_missing` when `session` is a child whose worktree folder is gone (or was never saved),
/// so it may not run; None otherwise. A child never runs in its parent's checkout instead. `folder_exists` asks the disk.
pub fn run_refusal_with(session: &RunSession, folder_exists: impl Fn(&str) -> bool) -> Option<DelegationReasonCode> {
    if session.parent_session_id.is_none() && session.working_folder.is_none() {
        return None;
    }
    match &session.working_folder {
        Some(folder) if folder_exists(folder) => None,
        _ => Some(DelegationReasonCode::WorktreeMissing),
    }
}

/// [`run_refusal_with`] against the disk (`Directory.Exists`).
pub fn run_refusal(session: &RunSession) -> Option<DelegationReasonCode> {
    run_refusal_with(session, |folder| Path::new(folder).is_dir())
}

/// C# `DelegationPanes.IsWorkingFolder`: a working folder a saved pane may keep, a full path of at most 4096 characters with no NUL.
pub fn is_working_folder(path: Option<&str>) -> bool {
    normalize::is_working_folder(path)
}

/// C# `DelegationPanes.PaneState`: the pane `session` of `workspace` as the coordinator reads it. `start_refused`: the app's start
/// would refuse a run in the pane now (a CLI update is going, the pane is blocked, or its request does not validate). A pane idle after
/// a normal finish then reads as idle, so what comes for it is held as a row of its queued list instead of being handed a run the app
/// would refuse. A running pane still reads as running.
pub fn pane_state(
    session: &RunSession,
    workspace: &Workspace,
    run_id: Option<&str>,
    activity: DelegationPaneActivity,
    start_refused: bool,
) -> DelegationPaneState {
    DelegationPaneState {
        session_id: session.id().to_string(),
        permission_mode: session
            .settings
            .as_ref()
            .map(|s| s.permission_mode.clone())
            .unwrap_or_else(|| RunSettings::default().permission_mode),
        folder: run_workspace(session, workspace).path().to_string(),
        kind: session.kind().to_string(),
        provider: session.provider().to_string(),
        parent_session_id: session.parent_session_id.clone(),
        run_id: run_id.map(str::to_string),
        activity: if activity == DelegationPaneActivity::Finished && start_refused { DelegationPaneActivity::Idle } else { activity },
    }
}

#[derive(Default)]
struct Ledger {
    run_ids: HashMap<String, String>,
    going: HashSet<String>,
    finished: HashSet<String>,
}

/// C# `DelegationRunLedger`: the runs of the app's panes as delegation names them. Each run the app starts gets an id, and a pane that
/// finished a run normally since this launch is told apart from one idle any other way.
#[derive(Default)]
pub struct DelegationRunLedger {
    state: Mutex<Ledger>,
}

impl DelegationRunLedger {
    pub fn new() -> DelegationRunLedger {
        DelegationRunLedger::default()
    }

    /// A new run starts in the pane `session_id`: its id.
    pub fn begin(&self, session_id: &str) -> String {
        let run_id = format!("run-{}", wire::new_id());
        let mut state = lock(&self.state);
        state.run_ids.insert(session_id.to_string(), run_id.clone());
        state.going.insert(session_id.to_string());
        state.finished.remove(session_id);
        run_id
    }

    /// The pane's run going now, or its last since this launch.
    pub fn run_id(&self, session_id: &str) -> Option<String> {
        lock(&self.state).run_ids.get(session_id).cloned()
    }

    /// The pane's run ended with the app status `status` ("completed", "stopped" or "error"); `quitting` while the app quits. The run
    /// and how it ended, or None when no run of this launch was going: a second end of the same run is not another end.
    pub fn end(&self, session_id: &str, status: &str, quitting: bool) -> Option<(String, ChildRunEnd)> {
        let mut state = lock(&self.state);
        let run_id = state.run_ids.get(session_id).cloned()?;
        if !state.going.remove(session_id) {
            return None;
        }
        let end = if quitting {
            ChildRunEnd::Quit
        } else {
            match status {
                "completed" => ChildRunEnd::Finished,
                "stopped" => ChildRunEnd::Stopped,
                _ => ChildRunEnd::Errored,
            }
        };
        if end == ChildRunEnd::Finished {
            state.finished.insert(session_id.to_string());
        }
        Some((run_id, end))
    }

    /// What the pane is doing: `running` when a run is going or starting.
    pub fn activity(&self, session_id: &str, running: bool) -> DelegationPaneActivity {
        if running {
            DelegationPaneActivity::Running
        } else if lock(&self.state).finished.contains(session_id) {
            DelegationPaneActivity::Finished
        } else {
            DelegationPaneActivity::Idle
        }
    }

    /// The pane closed.
    pub fn forget(&self, session_id: &str) {
        let mut state = lock(&self.state);
        state.run_ids.remove(session_id);
        state.going.remove(session_id);
        state.finished.remove(session_id);
    }
}

/// C# `DelegationRunEvent`: a run event of a child's pane, as the host reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DelegationRunEvent {
    Started {
        child_id: String,
        run_id: String,
    },
    /// MC-38: the run an earlier app kept running goes on as `run_id`.
    Adopted {
        child_id: String,
        run_id: String,
    },
    Ended {
        child_id: String,
        run_id: String,
        end: ChildRunEnd,
    },
}

/// What the pump hands each event to, one at a time.
pub type DeliverRunEvent = Arc<dyn Fn(DelegationRunEvent) -> BoxFuture<'static, ()> + Send + Sync>;

/// C# `DelegationRunEventPump`: hands a host's child run events on one at a time, in the order they happened, so a run's end never
/// overtakes its start. One event whose handling failed (panicked) never stops the ones after it.
pub struct DelegationRunEventPump {
    events: Mutex<Option<mpsc::UnboundedSender<DelegationRunEvent>>>,
    pump: Mutex<Option<JoinHandle<()>>>,
}

impl DelegationRunEventPump {
    /// A pump on `runtime`, handing each event to `deliver` once the one before it was handled.
    pub fn new(runtime: &tokio::runtime::Handle, deliver: DeliverRunEvent) -> DelegationRunEventPump {
        let (events, mut receiver) = mpsc::unbounded_channel::<DelegationRunEvent>();
        let handing = runtime.clone();
        let pump = runtime.spawn(async move {
            while let Some(change) = receiver.recv().await {
                // Its own task, so a panic in it is that task's alone.
                let _ = handing.spawn(deliver(change)).await;
            }
        });
        DelegationRunEventPump { events: Mutex::new(Some(events)), pump: Mutex::new(Some(pump)) }
    }

    /// C# `new DelegationRunEventPump(coordinator)`: each event reaches the coordinator's record of its child's runs.
    pub fn for_coordinator(runtime: &tokio::runtime::Handle, coordinator: crate::coordinator::Coordinator) -> DelegationRunEventPump {
        DelegationRunEventPump::new(
            runtime,
            Arc::new(move |change| {
                let coordinator = coordinator.clone();
                Box::pin(async move {
                    match change {
                        DelegationRunEvent::Started { child_id, run_id } => coordinator.child_run_started(&child_id, &run_id).await,
                        DelegationRunEvent::Adopted { child_id, run_id } => coordinator.child_run_adopted(&child_id, &run_id).await,
                        DelegationRunEvent::Ended { child_id, run_id, end } => {
                            coordinator.child_run_ended(&child_id, &run_id, end).await;
                        }
                    }
                })
            }),
        )
    }

    /// An event to hand on; none is taken after [`DelegationRunEventPump::finish`].
    pub fn send(&self, change: DelegationRunEvent) {
        if let Some(events) = lock(&self.events).as_ref() {
            let _ = events.send(change);
        }
    }

    /// Takes no more events and returns once every one sent is handed on.
    pub async fn finish(&self) {
        lock(&self.events).take();
        let pump = lock(&self.pump).take();
        if let Some(pump) = pump {
            let _ = pump.await;
        }
    }
}
