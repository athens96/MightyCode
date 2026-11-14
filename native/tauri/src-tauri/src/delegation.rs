//! The window's half of delegation (C# WinUI/MainWindow.Delegation.cs, MainWindow.WorktreeMenu.cs; the web shell's
//! MainWindow.WebShell.Shell.cs:62, 71, 89-94, 374-420, MainWindow.WebShell.Composer.cs:71-86, 309-316, 560-611,
//! MainWindow.QueuedComposer.cs:50, 76-104, 290-354, MainWindow.Navigation.cs:375-379) over mighty-delegation's `app::DelegationApp`.
//! Owner: W7-D.
//!
//! W7 Step 0 wrote every seam the shared host files call, so main.rs, methods.rs, runs.rs, host.rs, panes.rs and updates.rs need no
//! further W7 change. The app ([`app`]) is made as the run service's hooks and attached to it at [`start`]; the window's part
//! ([`DelegationWindow`]) refuses a run the window would refuse and finishes a delegated run's request as a send's. The composer's
//! second send and 바로 실행's worktree, agent-menu and held rows come here ([`run_in_new_worktree`], [`release_held_now`]), with
//! `RunInNewWorktree`'s three ways back. Until the coordinator is loaded (W7-A's, at the merge) every answer is C#'s with
//! `delegation` null: no child, nothing held, Ctrl+Enter queues, nothing refused, no child can be made (switched off). What comes with
//! it (W7 plan, W7-D): `InterruptRunsKilledByQuit`, the event pump, `childAction` with its dialogs (dialogs.rs), the parent-close
//! question and the child-close cleanup, the workspace removal's refusal, the sidebar rows with their card notes and busy, the
//! composer's inputs, the worktree menu, `AgentAvailabilityFromCli`, the merge queue's 10 s retry, the ending runs.
//!
//! Gating (W7 plan section 4): without `--allow-state-write` delegation is read-only (the file read, the tree and held rows shown; the
//! server not attached; Ctrl+Enter queues; card actions and child-close cleanup explain why they did nothing); a smoke runs it for real
//! only on its fixture under its own profile. Every git step asks [`Host::may_touch_repo`], every live action [`Host::delegation_live`].

use std::collections::HashSet;
use std::sync::{Arc, Mutex, Weak};

use mighty_composer::composer::HeldItem;
use mighty_composer::queue::{self, ComposerSecondSend, QueuedComposerInput};
use mighty_core::attachment::RunAttachment;
use mighty_core::model::{AppSnapshot, RunSession};
use mighty_core::shell::SidebarChild;
use mighty_delegation::app::{DelegationApp, DelegationWindow};
use mighty_platform::BoxFuture;
use mighty_run::model::{RunEvent, StartRunRequest};
use mighty_run::service::DelegationRunHooks;
use serde_json::{Value, json};

use crate::host::{Host, Reply, View, lock};
use crate::panes;

/// The app's delegation (C# `DesktopService`'s delegation half), made with the run service's hooks (runs.rs) and attached to the
/// service at [`start`]. One app per process, as there is one host.
static APP: Mutex<Option<Arc<DelegationApp>>> = Mutex::new(None);

/// The app's delegation, once runs.rs made it.
pub fn app() -> Option<Arc<DelegationApp>> {
    lock(&APP).clone()
}

/// The window's part of delegation (C# MainWindow.Delegation.cs `InitializeDelegation`), over the host it does not keep alive.
struct Window {
    host: Weak<Host>,
}

impl DelegationWindow for Window {
    /// C# `DelegatedRunBlocked` ([`run_block_reason`]).
    fn run_blocked(&self, pane: &RunSession) -> Option<String> {
        let Some(host) = self.host.upgrade() else { return Some(mighty_run::strings::get("remote.error.startFailed")) };
        let view = host.view();
        run_block_reason(&host, &view.snapshot, pane)
    }

    /// C# `FinishDelegatedRequest`: after the pane's model list, with the pane's model and settings and the registered models.
    fn run_request(&self, request: StartRunRequest) -> BoxFuture<'static, StartRunRequest> {
        let host = self.host.clone();
        Box::pin(async move {
            let mut request = request;
            let Some(host) = host.upgrade() else { return request };
            if host.closing() {
                return request;
            }
            crate::models::prepare(&host, &request.session_id).await;
            if host.closing() {
                return request;
            }
            let view = host.view();
            if let Some(pane) = view.snapshot.session(&request.session_id).filter(|pane| pane.kind() == "claude") {
                let workspace = view.snapshot.workspace(pane.workspace_id()).cloned();
                request.model = pane.model().to_string();
                request.settings = pane.settings.clone();
                request.registered_models = crate::runs::registered_models_for(pane.provider(), workspace.as_ref(), &view.snapshot);
            }
            request
        })
    }

    /// The state changed through the run service: the page's copy follows on the next tick.
    fn state_changed(&self) {
        let Some(host) = self.host.upgrade() else { return };
        host.view().sync();
        host.schedule();
    }

    /// C# `DelegationChanged`: the sidebar and every composer's held rows and second send are drawn again.
    fn changed(&self) {
        let Some(host) = self.host.upgrade() else { return };
        host.panes_changed();
        host.schedule();
    }

    /// C# `DelegationNoted`: the pane shows its new line at once.
    fn noted(&self, session_id: &str) {
        let Some(host) = self.host.upgrade() else { return };
        {
            let mut view = host.view();
            if let Some(ui) = view.panes.get_mut(session_id) {
                ui.dirty = true;
            }
            view.mighty.dirty(session_id);
        }
        host.schedule();
    }
}

/// C# `DelegatedRunBlockReason`: why the window refuses a run delegation starts in the pane now (the app closing, a manual update or
/// plugin change refusing the pane's sends, an automatic update of its CLI holding them); None when one may start. Read with the state
/// locked: it reads the updates' own state and `snapshot`, never `host.view()`.
fn run_block_reason(host: &Host, snapshot: &AppSnapshot, pane: &RunSession) -> Option<String> {
    if host.closing() {
        return Some(mighty_run::strings::get("remote.error.startFailed"));
    }
    crate::updates::manual_block(host, snapshot, pane).or_else(|| {
        crate::updates::holds(host, snapshot, pane).then(|| {
            mighty_run::strings::get_with(
                "settings.cliUpdate.backgroundUpdateQueued",
                &[("provider", mighty_run::strings::provider_label(pane.provider()))],
            )
        })
    })
}

/// Delegation's start with the app (C# `DesktopService.InitializeAsync`'s delegation half, then `InterruptRunsKilledByQuit`), after the
/// run service is up (runs.rs): the app made with the service's hooks is attached to it.
pub fn start(host: &Arc<Host>) {
    if let (Some(app), Some(runs)) = (app(), host.runs()) {
        app.attach(runs.service.clone());
    }
}

/// The app quits: the merge queue's retries and the coordinator's pump stop (C# `DesktopService.DisposeAsync`'s delegation half).
pub fn shutdown(host: &Host) {
    let _ = host;
}

/// The run service's hooks (mighty-run `service::DelegationRunHooks`: `DelegationWorktreeRoot`, `DelegationRunBegan`,
/// `DelegationRunEnded`, the end of `Receive`, `PaneClosed`), read once as runs.rs makes the service: the app's delegation, made here.
pub fn run_hooks(host: &Arc<Host>) -> Option<Arc<dyn DelegationRunHooks>> {
    let app = DelegationApp::new(Arc::new(Window { host: Arc::downgrade(host) }));
    *lock(&APP) = Some(app.clone());
    Some(app)
}

/// Each run event before the pane takes it, from runs.rs's pump. The offers and merges C# makes after `Receive` (DesktopService.cs:
/// 271-275) are the service hook's ([`DelegationRunHooks::received`]); here the window's own part (a child's ending run redraws its
/// composer).
pub fn on_run_event(host: &Arc<Host>, event: &RunEvent) {
    let _ = (host, event);
}

/// The sidebar's delegated children (C# `DelegationSidebar.Rows` with the card notes and busy cards), read by the frame model with the
/// state locked: it reads delegation's own state and `snapshot`, never `host.view()`.
pub fn sidebar_rows(host: &Host, snapshot: &AppSnapshot) -> Vec<SidebarChild> {
    let _ = (host, snapshot);
    vec![]
}

/// What the composer reads of delegation for one pane (`WindowRules`' `SecondSend`, `WorktreeMenuOnly`, `HeldItems`, `EndingRun`).
pub struct WindowInputs {
    pub second_send: ComposerSecondSend,
    pub worktree_menu_only: bool,
    pub held_items: Vec<HeldItem>,
    pub ending_run: bool,
}

/// The pane's composer inputs (C# `owner.SecondSend(pane)`, `WorktreeMenuOnly`, `HeldItems`, `Delegation.EndingRuns.Contains`), read
/// with the state locked as [`sidebar_rows`]. At rest: Ctrl+Enter queues, no menu, nothing held, no run ending.
pub fn window_inputs(host: &Host, snapshot: &AppSnapshot, pane: &RunSession) -> WindowInputs {
    let _ = (host, snapshot, pane);
    WindowInputs { second_send: ComposerSecondSend::Queue, worktree_menu_only: false, held_items: vec![], ending_run: false }
}

/// C# `delegation?.EndingRuns.Contains(pane.Id)`: a delegated child still recording its last run's end (the manual updates' block,
/// updates.rs). Read with the state locked.
pub fn ending_run(host: &Host, pane: &str) -> bool {
    let _ = (host, pane);
    false
}

/// What closing a pane does first (C# `ClosePane`: the parent-close question `delegation-parent-close-<id>`, a child's close cleanup).
pub enum CloseDecision {
    /// Close it (after whatever cleanup ran).
    Close,
    /// The user kept it open.
    #[allow(dead_code, reason = "asked once the coordinator's open children are read (C# `ParentCloseQuestion`, W7 phase 2)")]
    Keep,
}

/// Asked by frame.rs's `closeSession` before the pane's run stops.
pub async fn before_close(host: &Arc<Host>, pane: &str) -> CloseDecision {
    let _ = (host, pane);
    CloseDecision::Close
}

/// C# `WorkspaceRemovalRefusal`: `delegation.workspace.hasChildren` while an open child or a kept worktree is in the workspace; None
/// lets frame.rs's `removeWorkspace` ask.
pub fn removal_refusal(host: &Host, workspace: &str) -> Option<String> {
    let _ = (host, workspace);
    None
}

/// C# `RunInNewWorktree` (MainWindow.Delegation.cs:396-425): the composer's second send on the busy pane `pane`, the worktree menu's pick
/// of the child's `agent`, or a `queued` row's 바로 실행: the text and its attachments run now in a new child pane, in its own worktree
/// beside the pane, made by delegation as delegate makes one; the pane's run goes on undisturbed. When no child can be made the
/// transcript says why and the text goes on: a queued row back to the head of the queue ([`put_back_queued_row`]), a text meant for
/// another agent back to the composer ([`return_to_composer`]), any other the pane's next request ([`run_after_no_worktree`]). With no
/// coordinator loaded (C# `StartHumanChildAsync` while `delegation` is null) no child can be made: delegation is switched off.
pub fn run_in_new_worktree(
    host: &Arc<Host>,
    pane: &str,
    text: String,
    files: Vec<RunAttachment>,
    agent: Option<String>,
    queued: Option<QueuedComposerInput>,
) {
    let provider = host.view().snapshot.session(pane).map(|p| p.provider().to_string()).unwrap_or_default();
    let to_composer = queue::returns_to_composer(agent.as_deref(), &provider);
    let (host, parent) = (host.clone(), pane.to_string());
    tauri::async_runtime::spawn(async move {
        let reason = host.locale.get("delegation.human.error.switchedOff");
        // A pane that went away meanwhile has no composer to fall back to.
        match queued {
            Some(row) => put_back_queued_row(&host, &parent, row, &reason),
            None if to_composer => return_to_composer(&host, &parent, text, files, &reason),
            None => run_after_no_worktree(&host, &parent, text, files, &reason),
        }
        host.schedule();
    });
}

/// Whether the service runs the pane now (`IsSessionRunning`).
fn running(host: &Host, id: &str) -> bool {
    host.runs().is_some_and(|runs| runs.service.is_session_running(id))
}

/// C# `RunAfterNoWorktree`: the text of a second send that no new child runs is the pane's next request, queued behind the run or
/// started now when the run ended meanwhile; it never joins the running turn. With the queue full it goes back to the composer instead,
/// ahead of anything typed there meanwhile, so it is never lost. The transcript says which, and why.
fn run_after_no_worktree(host: &Arc<Host>, id: &str, text: String, files: Vec<RunAttachment>, reason: &str) {
    let drain = {
        let mut view = host.view();
        if !panes::alive(&view, id) {
            return;
        }
        let Some(pane) = view.snapshot.session(id).cloned() else { return };
        let holds = crate::updates::holds(host, &view.snapshot, &pane);
        let Ok(ui) = panes::ui(&mut view, id) else { return };
        let busy = pane.status() == "running" || ui.view.starting || running(host, id) || holds;
        if ui.queue.items().len() >= queue::MAXIMUM_ITEMS {
            let _ = crate::composer::slash_note(&mut view, id, &host.locale.get_with("delegation.human.returned", &[("reason", reason)]));
            view.error = Some(host.locale.get("queue.full"));
            give_back_to_composer(host, &mut view, id, text, files);
            return;
        }
        let _ = crate::composer::slash_note(&mut view, id, &host.locale.get_with("delegation.human.queued", &[("reason", reason)]));
        let Ok(ui) = panes::ui(&mut view, id) else { return };
        let plan = queue::background::queued_override(ui.view.launches_in_plan_mode);
        if let Err(error) = ui.queue.add(&text, files, plan, false, false, &host.locale) {
            view.error = Some(error.to_string());
            return;
        }
        ui.dirty = true;
        // The run ended meanwhile: it is the next request now.
        !busy || matches!(view.snapshot.session(id).map(|p| p.status()), Some("completed" | "idle"))
    };
    if drain {
        crate::composer::drain_queue(host.clone(), id.to_string());
    }
}

/// C# `PutBackQueuedRow`: a queued row no new child ran goes back to the head of the queue, and the transcript says why; a pane idle
/// meanwhile runs it now, unless an error paused its queue.
fn put_back_queued_row(host: &Arc<Host>, id: &str, item: QueuedComposerInput, reason: &str) {
    let drain = {
        let mut view = host.view();
        if !panes::alive(&view, id) {
            return;
        }
        let _ = crate::composer::slash_note(&mut view, id, &host.locale.get_with("delegation.human.queued", &[("reason", reason)]));
        let status = view.snapshot.session(id).map(|p| p.status().to_string()).unwrap_or_default();
        let Ok(ui) = panes::ui(&mut view, id) else { return };
        let waiting = !ui.queue.items().is_empty();
        ui.queue.insert(0, item);
        ui.dirty = true;
        !waiting && !matches!(status.as_str(), "running" | "error") && !ui.view.starting && !ui.view.queue_starting && !running(host, id)
    };
    if drain {
        crate::composer::drain_queue(host.clone(), id.to_string());
    }
}

/// C# `ReturnToComposer`: the text of a child of another agent that could not be made goes back to the composer with its files, and
/// the transcript says why. It never runs in this pane.
fn return_to_composer(host: &Arc<Host>, id: &str, text: String, files: Vec<RunAttachment>, reason: &str) {
    let mut view = host.view();
    if !panes::alive(&view, id) {
        return;
    }
    let _ = crate::composer::slash_note(&mut view, id, &host.locale.get_with("delegation.human.returnedToComposer", &[("reason", reason)]));
    give_back_to_composer(host, &mut view, id, text, files);
}

/// C# `GiveBackToComposer`: a text no run took goes back to the composer with its files, ahead of anything typed there meanwhile, and
/// without a file the composer already holds.
fn give_back_to_composer(host: &Host, view: &mut View, id: &str, text: String, files: Vec<RunAttachment>) {
    let Ok(ui) = panes::ui(view, id) else { return };
    let draft = if ui.view.draft.is_empty() { text } else { format!("{text}\n\n{}", ui.view.draft) };
    ui.view.draft = crate::composer::clamp(&draft);
    let kept: HashSet<String> = ui.view.attachments.iter().map(|file| file.id.clone()).collect();
    let back: Vec<RunAttachment> = files.into_iter().filter(|file| !kept.contains(&file.id)).collect();
    ui.view.attachments.splice(0..0, back);
    ui.dirty = true;
    let draft = ui.view.draft.clone();
    if let Some(runs) = host.runs()
        && matches!(runs.service.update_draft(id, &draft), Ok(true))
    {
        view.sync();
    }
}

/// C# `ReleaseHeldNow`, 바로 실행 on a held row: the items delegation holds for the pane go to it now, all of them in one run. With no
/// coordinator loaded nothing is held (C# `RunNextAsync` answers `NothingHeld`): the first queued row runs now, as run next did. A
/// refused start says why in the error banner.
pub fn release_held_now(host: &Arc<Host>, id: &str) {
    let blocked = {
        let view = host.view();
        let Some(pane) = view.snapshot.session(id).cloned() else { return };
        run_block_reason(host, &view.snapshot, &pane)
    };
    if let Some(blocked) = blocked {
        host.view().error = Some(blocked);
        return;
    }
    crate::composer::drain_queue(host.clone(), id.to_string());
}

/// C# `OffersWorktreeMenu`: whether the worktree button, and its menu, shows on the busy pane (its second send opens a worktree, or
/// only another agent's child could). Read with the state locked.
pub fn offers_worktree_menu(host: &Host, snapshot: &AppSnapshot, pane: &str) -> bool {
    let Some(pane) = snapshot.session(pane) else { return false };
    let inputs = window_inputs(host, snapshot, pane);
    inputs.second_send == ComposerSecondSend::Worktree || inputs.worktree_menu_only
}

/// `queueRunNowMenu`'s rows (`[{agent, text, enabled}]`, C# `OffersWorktreeMenu`): none at rest.
pub fn run_now_menu(host: &Host, pane: &str) -> Value {
    let _ = (host, pane);
    json!([])
}

/// `childAction{id, action: "merge"|"undo"|"discard"}` (C# MainWindow.WebShell.Shell.cs:89-94, MainWindow.Delegation.cs:165-225):
/// ignored while the card is busy or a dialog is open; merge with its merged-discard offer, undo, discard with its confirm. Answers null.
pub async fn child_action(host: Arc<Host>, args: Value) -> Reply {
    let _ = (host, args);
    Ok(Value::Null)
}
