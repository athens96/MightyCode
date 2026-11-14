//! The window's half of delegation (C# WinUI/MainWindow.Delegation.cs, MainWindow.WorktreeMenu.cs; the web shell's
//! MainWindow.WebShell.Shell.cs:62, 71, 89-94, 374-420, MainWindow.WebShell.Composer.cs:71-86, 309-316, 560-611,
//! MainWindow.QueuedComposer.cs:50, 76-104, 290-354, MainWindow.Navigation.cs:375-379) over mighty-delegation's `app::DelegationApp`.
//! Owner: W7-D.
//!
//! W7 Step 0 wrote every seam the shared host files call, each at rest (no child, nothing held, Ctrl+Enter queues, nothing refused), so
//! main.rs, methods.rs, runs.rs, host.rs, panes.rs and updates.rs need no further W7 change. What W7-D fills here (W7 plan, W7-D): the
//! start at app start (or delegation disabled for the launch when the file is unreadable), `InterruptRunsKilledByQuit`, the event pump,
//! `childAction` with its dialogs (dialogs.rs), the parent-close question and the child-close cleanup, the workspace removal's refusal,
//! the sidebar rows with their card notes and busy, the composer's inputs, the second send, 바로 실행's worktree and held paths and its
//! menu, `RunInNewWorktree`, `AgentAvailabilityFromCli`, the merge queue's 10 s retry, the ending runs.
//!
//! Gating (W7 plan section 4): without `--allow-state-write` delegation is read-only (the file read, the tree and held rows shown; the
//! server not attached; Ctrl+Enter queues; card actions and child-close cleanup explain why they did nothing); a smoke runs it for real
//! only on its fixture under its own profile. Every git step asks [`Host::may_touch_repo`], every live action [`Host::delegation_live`].

#![allow(dead_code, reason = "W7-D's seams: some are called only once W7-D wires composer.rs's send")]

use std::sync::Arc;

use mighty_composer::composer::HeldItem;
use mighty_composer::queue::{ComposerSecondSend, QueueRunNowPath};
use mighty_core::model::{AppSnapshot, RunSession};
use mighty_core::shell::DelegationChildRow;
use mighty_run::model::RunEvent;
use mighty_run::service::DelegationRunHooks;
use serde_json::{Value, json};

use crate::host::{Host, Reply};

/// Delegation's start with the app (C# `DesktopService.InitializeAsync`'s delegation half, then `InterruptRunsKilledByQuit`), after the
/// run service is up (runs.rs). At rest: nothing.
pub fn start(host: &Arc<Host>) {
    let _ = host;
}

/// The app quits: the merge queue's retries and the coordinator's pump stop (C# `DesktopService.DisposeAsync`'s delegation half).
pub fn shutdown(host: &Host) {
    let _ = host;
}

/// The run service's hooks (mighty-run `service::DelegationRunHooks`: `DelegationWorktreeRoot`, `DelegationRunBegan`,
/// `DelegationRunEnded`), read once as runs.rs makes the service. None: no run is recorded.
pub fn run_hooks(host: &Arc<Host>) -> Option<Arc<dyn DelegationRunHooks>> {
    let _ = host;
    None
}

/// Each run event before the pane takes it (C# DesktopService.cs:261-273: `OfferDelegationPending` on a run's start or end or a
/// notice turn's end, `KickQueuedMerges` on an end), from runs.rs's pump.
pub fn on_run_event(host: &Arc<Host>, event: &RunEvent) {
    let _ = (host, event);
}

/// The sidebar's delegated children (C# `DelegationSidebar.Rows` with the card notes and busy cards), read by the frame model with the
/// state locked: it reads delegation's own state and `snapshot`, never `host.view()`.
pub fn sidebar_rows(host: &Host, snapshot: &AppSnapshot) -> Vec<DelegationChildRow> {
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

/// The composer's second send (`send{second:true}`, C# `SecondSend` → `RunInNewWorktree`): true when the text went to a new child
/// worktree (the answer's outcome `"worktree"`), false when it queues as Enter does.
pub async fn second_send(host: &Arc<Host>, pane: &str, text: &str) -> bool {
    let _ = (host, pane, text);
    false
}

/// 바로 실행's delegation paths (C# MainWindow.QueuedComposer.cs:290-354): a row run in a new worktree, a held item released; the
/// waits do nothing.
pub async fn run_now(host: &Arc<Host>, pane: &str, item: &str, path: QueueRunNowPath) {
    let _ = (host, pane, item, path);
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
