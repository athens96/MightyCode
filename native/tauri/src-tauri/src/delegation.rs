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
//! only on its fixture under its own profile. Every git step asks [`may_touch`], every live action [`Host::delegation_live`].

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};

use mighty_composer::composer::{HeldItem, WorktreeAgent};
use mighty_composer::queue::{self, ComposerSecondSend, QueuedComposerInput};
use mighty_core::attachment::RunAttachment;
use mighty_core::model::{AppSnapshot, RunSession};
use mighty_core::shell::SidebarChild;
use mighty_delegation::app::{DelegationApp, DelegationStart, DelegationWindow};
use mighty_delegation::cards::{DelegationCardAction, DelegationCardResult, PendingChildDiscard};
use mighty_delegation::delivery::DelegationRelease;
use mighty_delegation::git::Git;
use mighty_delegation::git::paths::DelegationPaths;
use mighty_delegation::git::worktree::{self, Maker};
use mighty_delegation::host::DelegationAgentAvailability;
use mighty_delegation::providers::{self, DelegationChildBlock, DelegationChildChoice};
use mighty_delegation::request::{
    DelegationCaller, DelegationHandler, DelegationIoHandler, DelegationName, DelegationRequest, DelegationResponse,
};
use mighty_delegation::store::DelegationFileStore;
use mighty_delegation::text;
use mighty_delegation::tools::delegate::HumanChildStart;
use mighty_platform::BoxFuture;
use mighty_platform::fake::FixedDiskSpace;
use mighty_run::model::{RunEvent, StartRunRequest};
use mighty_run::service::DelegationRunHooks;
use serde_json::{Value, json};

use crate::host::{Host, Reply, View, lock};
use crate::panes;

/// The app's delegation (C# `DesktopService`'s delegation half), made with the run service's hooks (runs.rs) and attached to the
/// service at [`start`]. One app per process, as there is one host.
static APP: Mutex<Option<Arc<DelegationApp>>> = Mutex::new(None);
static HOST: Mutex<Option<Weak<Host>>> = Mutex::new(None);

/// The app's delegation, once runs.rs made it.
pub fn app() -> Option<Arc<DelegationApp>> {
    lock(&APP).clone()
}

/// C# `smokeAgentsReady`: agents the delegation smoke treats as ready whatever the CLI search found (its scripted Claude).
static SMOKE_READY: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// The delegation smoke's word that `provider`'s CLI is ready (smoke_delegation.rs).
#[cfg(feature = "dev-harness")]
pub fn set_smoke_agent_ready(provider: &str, ready: bool) {
    let mut ready_now = lock(&SMOKE_READY);
    ready_now.retain(|agent| agent != provider);
    if ready {
        ready_now.push(provider.to_string());
    }
}

/// Why a delegated run's request is refused while the app quits.
const CLOSING: &str = "the app is closing";

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
    fn run_request(&self, request: StartRunRequest) -> BoxFuture<'static, Result<StartRunRequest, String>> {
        let host = self.host.clone();
        Box::pin(async move {
            let mut request = request;
            // Quitting (or the window gone): no delegated run starts, as none would start from the composer. Handing the request on
            // unfinished would launch it without its style, so out of a plan mode the style asks for, until the quit's dispose ends it.
            let Some(host) = host.upgrade() else { return Err(CLOSING.into()) };
            if host.closing() {
                return Err(CLOSING.into());
            }
            crate::models::prepare(&host, &request.session_id).await;
            if host.closing() {
                return Err(CLOSING.into());
            }
            let mut workspace_for_memory = None;
            {
                let view = host.view();
                if let Some(pane) = view.snapshot.session(&request.session_id).filter(|pane| pane.kind() == "claude") {
                    let workspace = view.snapshot.workspace(pane.workspace_id()).cloned();
                    request.model = pane.model().to_string();
                    request.settings = pane.settings.clone();
                    request.registered_models = crate::runs::registered_models_for(pane.provider(), workspace.as_ref(), &view.snapshot);
                    workspace_for_memory = workspace;
                }
            }
            if request.kind == "claude" && request.memory_context.is_none() {
                request.memory_context = crate::shared_memory::context_for_pane(&host, workspace_for_memory.as_ref());
            }
            // C# `PrepareStyleRunRequest`: a styled pane's grants and plan-mode launch (styles.rs). When the style needs a yes again it
            // throws and the delegated run does not start (never without its style, so never out of its plan mode).
            let id = request.session_id.clone();
            crate::styles::prepare_run(&host, &id, request).await.inspect_err(|reason| {
                host.log.line(&format!("delegated run in {id} not started: {reason}"));
            })
        })
    }

    /// C# `EndAgentTerminal` (agent_io.rs): the pane's agent terminal and everything it started, ended.
    fn end_agent_terminal(&self, session_id: String) -> BoxFuture<'static, ()> {
        let host = self.host.upgrade();
        Box::pin(async move {
            if let Some(host) = host {
                crate::agent_io::end_agent_terminal(&host, &session_id).await;
            }
        })
    }

    /// C# `AgentAvailabilityFromCli`: found by the CLI search the run engine made, and not signed out by the account status last read;
    /// no CLI is asked.
    fn agent_availability(&self, provider: &str) -> DelegationAgentAvailability {
        let Some(host) = self.host.upgrade() else { return DelegationAgentAvailability::Ready };
        if host.smoke.is_some() && lock(&SMOKE_READY).iter().any(|agent| agent == provider) {
            return DelegationAgentAvailability::Ready;
        }
        let installed = host.runs().and_then(|runs| runs.runtime(provider)).map(|runtime| runtime.available);
        let account = crate::accounts::statuses(&host).into_iter().find(|status| status.provider == provider);
        providers::availability(installed, account.as_ref())
    }

    /// W7 plan D2: delegation acts with `--allow-state-write` or in a smoke.
    fn live(&self) -> bool {
        self.host.upgrade().is_some_and(|host| host.delegation_live())
    }

    /// C# `QueuedMergeDone`: the discard is offered as after a merge the human waited for.
    fn queued_merge_done(&self, child_id: &str) {
        let (Some(host), Some(app)) = (self.host.upgrade(), app()) else { return };
        let id = child_id.to_string();
        tauri::async_runtime::spawn(async move {
            if !host.closing() {
                offer_discard_after_merge(&host, &app, &id).await;
                host.schedule();
            }
        });
    }

    /// The state changed through the run service: the page's copy follows on the next tick.
    fn state_changed(&self) {
        let Some(host) = self.host.upgrade() else { return };
        host.view().sync();
        host.schedule();
    }

    /// C# `DelegationChanged`: the sidebar and every composer's held rows and second send are drawn again; MC-29's compare runs
    /// note their children that finished and start their tests.
    fn changed(&self) {
        let Some(host) = self.host.upgrade() else { return };
        host.panes_changed();
        crate::compare::delegation_changed(&host);
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
pub(crate) fn run_block_reason(host: &Host, snapshot: &AppSnapshot, pane: &RunSession) -> Option<String> {
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

/// The delegation's git hooks folder (C# `DelegationGit.UseProfile(profile)` → `<profile>\git-no-hooks`): the profile's when the app may
/// write it (`--allow-state-write`, or a smoke's own profile), else this machine's own empty folder for the profile, the work map's
/// (`workmap::Folders::hooks`), so a read-only launch writes nothing in the profile. Never under the install folder.
pub fn hooks_folder(data_dir: &Path, machine: Option<&Path>, writes: bool) -> PathBuf {
    match machine {
        Some(machine) if !writes => machine.to_path_buf(),
        _ => data_dir.join("git-no-hooks"),
    }
}

/// The folder a smoke's delegation works in (its fixture repositories, its worktrees, its delegation file and hooks folder): the smoke's
/// own profile when the smoke may write it, else a folder of this process's own, so a read-only smoke of an existing profile
/// (`smoke.ps1 -Profile`) leaves every file of it alone.
pub fn smoke_root(host: &Host) -> PathBuf {
    if host.writes() { host.data_dir.clone() } else { std::env::temp_dir().join(format!("mighty-delegation-smoke-{}", std::process::id())) }
}

/// Whether a git step of delegation that writes may touch the repository or worktree folder at `path` ([`Host::may_touch_repo`]); in a
/// smoke also the smoke's own delegation folder ([`smoke_root`]) as written: an absolute path with no `..` inside it.
pub fn may_touch(host: &Host, path: &Path) -> bool {
    if host.may_touch_repo(path) {
        return true;
    }
    if host.smoke.is_none() || !path.is_absolute() || path.components().any(|part| matches!(part, std::path::Component::ParentDir)) {
        return false;
    }
    // Both sides as the OS finally names them: the coordinator keeps a worktree's path in its long form, while the temporary folder can
    // be written in its 8.3 form (CI's `C:\Users\RUNNER~1\...`).
    let paths = DelegationPaths::new(crate::platform::file_ports().handles);
    let (path, root) = (paths.canonical(&path.to_string_lossy()), paths.canonical(&smoke_root(host).to_string_lossy()));
    mighty_delegation::git::paths::same(&path, &root) || mighty_delegation::git::paths::is_inside(&path, &root)
}

/// Where `delegation-state.json` is kept this launch: the profile when delegation may write it (W7 plan section 4), else a copy made
/// now in a folder of this process's own, so the tree and held rows show and nothing reaches the profile's file.
fn store_folder(host: &Host) -> PathBuf {
    if host.writes() {
        return host.data_dir.clone();
    }
    if host.smoke.is_some() {
        let root = smoke_root(host);
        // A folder of this process's own, never one an earlier process of the same id left (the profile itself is never removed).
        if root != host.data_dir {
            let _ = std::fs::remove_dir_all(&root);
        }
        let _ = std::fs::create_dir_all(&root);
        return root;
    }
    let copy = std::env::temp_dir().join(format!("mighty-delegation-read-only-{}", std::process::id()));
    // Made afresh: never a file an earlier process of the same id left.
    let _ = std::fs::remove_dir_all(&copy);
    let _ = std::fs::create_dir_all(&copy);
    let file = DelegationFileStore::new(&host.data_dir).file_path();
    let _ = std::fs::copy(&file, DelegationFileStore::new(&copy).file_path());
    copy
}

/// Delegation's start with the app (C# `DesktopService.InitializeAsync`'s delegation half, then `InterruptRunsKilledByQuit`), after the
/// run service is up (runs.rs): the app attached to the service, the coordinator made from the delegation file (an unreadable file
/// leaves delegation unavailable this launch), and the runs' delegation calls handed to it (agent_io.rs).
pub fn start(host: &Arc<Host>) {
    let (Some(app), Some(runs)) = (app(), host.runs()) else { return };
    app.attach(runs.service.clone());
    let machine = tauri::Manager::try_state::<crate::workmap::Folders>(&host.app).map(|folders| folders.hooks.clone());
    // The profile's hooks folder only when the app may write the profile; a smoke's in its own folder (review M2: a read-only smoke of
    // an existing profile makes nothing in it).
    let hooks = match (cfg!(windows), host.smoke.is_some()) {
        (false, _) => PathBuf::from("/dev/null"),
        (true, true) => smoke_root(host).join("git-no-hooks"),
        (true, false) => hooks_folder(&host.data_dir, machine.as_deref(), host.writes()),
    };
    let git = Git::new(host.processes.clone(), hooks);
    // A smoke's worktrees are under its own profile, with room for them whatever the disk says.
    let (root, disk): (String, Arc<dyn mighty_platform::DiskSpace>) = match host.smoke {
        Some(_) => (smoke_root(host).join("worktrees").to_string_lossy().into_owned(), Arc::new(FixedDiskSpace(u64::MAX))),
        None => (worktree::default_root(&crate::composer::dirs_home().to_string_lossy()), crate::platform::disk_space()),
    };
    let switch_host = Arc::downgrade(host);
    let switch_on: Arc<dyn Fn() -> bool + Send + Sync> =
        Arc::new(move || switch_host.upgrade().is_some_and(|host| crate::settings::general::delegation_on(&host)));
    let locale = host.locale;
    let mut start = DelegationStart::new(
        store_folder(host),
        Maker::new(&root, git, disk),
        DelegationPaths::new(crate::platform::file_ports().handles),
        switch_on,
    );
    start.locale = Arc::new(move || locale);
    // Loaded now, as C#'s `StartDelegation` is, so no run started after this goes unrecorded; the sweep of what quitting killed follows.
    if !app.begin(start) {
        host.log.line("delegation: the delegation file could not be read; delegation is off for this launch");
        return;
    }
    crate::agent_io::set_delegation_handler(host, Some(Arc::new(CurrentCoordinator)));
    host.log.line("delegation: started");
    let host = host.clone();
    tauri::async_runtime::spawn(async move {
        app.interrupt_runs_killed_by_quit().await;
        host.panes_changed();
        host.schedule();
    });
}

/// C# `SetDelegationOn`'s `RefreshRunningWorktreeReadiness`: the switch changed (settings/general.rs), so every running pane learns again
/// whether its folder can host a child worktree and its Ctrl+Enter follows at once.
pub fn switch_changed(host: &Host) {
    if let Some(app) = app() {
        app.refresh_running_worktree_readiness();
    }
    host.panes_changed();
}

/// Who answers the runs' delegation calls (C# `local.DelegationHandler = coordinator`): the coordinator delegation has now, so a smoke's
/// restart (C# `RestartDelegationAsync`, which sets it again) is followed at once; without one, C#'s handler of no coordinator.
struct CurrentCoordinator;

impl DelegationHandler for CurrentCoordinator {
    fn handle(&self, request: DelegationRequest, caller: DelegationCaller) -> BoxFuture<'static, DelegationResponse> {
        match app().and_then(|app| app.coordinator()) {
            Some(coordinator) => coordinator.handle(request, caller),
            None => DelegationIoHandler::new(|| false).handle(request, caller),
        }
    }
}

/// The app quits, once every run has ended: the coordinator's pump, the children's closes, the merge retry and the watches stop (C#
/// `StopDelegation`), and the runs' delegation calls are answered by no coordinator.
pub fn shutdown(host: &Host) {
    crate::agent_io::set_delegation_handler(host, None);
    if let Some(app) = app() {
        tauri::async_runtime::block_on(async move {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(3), app.stop()).await;
        });
    }
    if !host.writes() {
        let own = match host.smoke {
            Some(_) => smoke_root(host),
            None => std::env::temp_dir().join(format!("mighty-delegation-read-only-{}", std::process::id())),
        };
        let _ = std::fs::remove_dir_all(own);
    }
}

/// The run service's hooks (mighty-run `service::DelegationRunHooks`: `DelegationWorktreeRoot`, `DelegationRunBegan`,
/// `DelegationRunEnded`, the end of `Receive`, `PaneClosed`), read once as runs.rs makes the service: the app's delegation, made here.
pub fn run_hooks(host: &Arc<Host>) -> Option<Arc<dyn DelegationRunHooks>> {
    let app = DelegationApp::new(tauri::async_runtime::handle().inner().clone(), Arc::new(Window { host: Arc::downgrade(host) }));
    *lock(&HOST) = Some(Arc::downgrade(host));
    *lock(&APP) = Some(app.clone());
    Some(app)
}

/// Each run event before the pane takes it, from runs.rs's pump: a running pane learns again whether its folder can host a child
/// worktree (C# `ReceiveQueueRunEvent`'s `RefreshWorktreeReadiness`). The offers and merges C# makes after `Receive`
/// (DesktopService.cs:271-275) are the service hook's ([`DelegationRunHooks::received`]).
pub fn on_run_event(host: &Arc<Host>, event: &RunEvent) {
    if event.kind != "status" || event.status.as_deref() != Some("running") {
        return;
    }
    let Some(app) = app() else { return };
    let pane = app.service().and_then(|service| service.snapshot().session(&event.session_id).cloned());
    if let Some(pane) = pane {
        app.refresh_worktree_readiness(&pane);
    }
    let _ = host;
}

/// The sidebar's delegated children (C# `DelegationSidebar.Rows` with the card notes and busy cards), read by the frame model with the
/// state locked: it reads delegation's own state, never `host.view()`.
pub fn sidebar_rows(host: &Host, snapshot: &AppSnapshot) -> Vec<SidebarChild> {
    let _ = (host, snapshot);
    app().map(|app| app.sidebar_children()).unwrap_or_default()
}

/// What the composer reads of delegation for one pane (`WindowRules`' `SecondSend`, `WorktreeMenuOnly`, `HeldItems`, `EndingRun`, and
/// the worktree menu's agents 실행 중 보내기 and the queued rows' agent pick offer).
pub struct WindowInputs {
    pub second_send: ComposerSecondSend,
    pub worktree_menu_only: bool,
    pub worktree_agents: Vec<WorktreeAgent>,
    pub held_items: Vec<HeldItem>,
    pub ending_run: bool,
}

/// The pane's composer inputs (C# `owner.SecondSend(pane)`, `WorktreeMenuOnly`, `HeldItems`, `Delegation.EndingRuns.Contains`), read
/// with the state locked as [`sidebar_rows`]. A style's plan-first launch is the pane's `launches_in_plan_mode`, which styles.rs sets from its style.
pub fn window_inputs(host: &Host, snapshot: &AppSnapshot, pane: &RunSession) -> WindowInputs {
    let Some(app) = app() else {
        return WindowInputs {
            second_send: ComposerSecondSend::Queue,
            worktree_menu_only: false,
            worktree_agents: vec![],
            held_items: vec![],
            ending_run: false,
        };
    };
    let second_send = app.second_send(snapshot, pane);
    let worktree_menu_only = app.worktree_menu_only(snapshot, pane, false);
    // C# `OffersWorktreeMenu`: its agents only while a worktree is on offer.
    let offers = second_send == ComposerSecondSend::Worktree || worktree_menu_only;
    WindowInputs {
        second_send,
        worktree_menu_only,
        worktree_agents: if offers { worktree_agents(&host.locale, &app, pane) } else { vec![] },
        held_items: app.held_items(pane.id()).into_iter().map(|item| HeldItem { id: item.id, text: item.text }).collect(),
        ending_run: app.ending_run(pane.id()),
    }
}

/// C# `delegation?.EndingRuns.Contains(pane.Id)`: a delegated child still recording its last run's end (the manual updates' block,
/// updates.rs). Read with the state locked.
pub fn ending_run(host: &Host, pane: &str) -> bool {
    let _ = host;
    app().is_some_and(|app| app.ending_run(pane))
}

/// What closing a pane does first (C# `ClosePane`: the parent-close question `delegation-parent-close-<id>`).
pub enum CloseDecision {
    /// Close it (a delegated child's close cleanup follows through the run service's `PaneClosed`).
    Close,
    /// The user kept it open.
    Keep,
}

/// Asked by frame.rs's `closeSession` before the pane's run stops: a parent pane with open children asks first (C#
/// MainWindow.Navigation.cs:377, `ConfirmParentClose`); confirmed, only the parent closes and its children stay under a 'parent
/// closed' node.
pub async fn before_close(host: &Arc<Host>, pane: &str) -> CloseDecision {
    let Some(question) = app().and_then(|app| app.parent_close_question(pane)) else { return CloseDecision::Close };
    let locale = host.locale;
    let confirmed = confirm(
        host,
        json!({
            "type": "confirm", "title": locale.get("delegation.parentClose.title"),
            "message": text::parent_close_message(&question, &locale),
            "primary": locale.get("delegation.parentClose.confirm"), "close": locale.get("resume.cancel"),
            "defaultPrimary": false, "destructive": false,
        }),
    )
    .await;
    if confirmed && !host.closing() { CloseDecision::Close } else { CloseDecision::Keep }
}

/// C# `WorkspaceRemovalRefusal`: `delegation.workspace.hasChildren` while an open child or a kept worktree is in the workspace; None
/// lets frame.rs's `removeWorkspace` ask.
pub fn removal_refusal(host: &Host, workspace: &str) -> Option<String> {
    let reason = app()?.workspace_removal_refusal(workspace)?;
    Some(host.locale.get_with("delegation.workspace.hasChildren", &[("code", reason.name())]))
}

/// C# `SendAsync` (the composer's send and the queue's start): with items held for the pane they go first, all of them, in the one run
/// the human's words start; otherwise the request starts as the run service starts it. None when no run started.
pub async fn send(runs: &crate::runs::Runs, mut request: StartRunRequest) -> Result<Option<()>, mighty_run::RunError> {
    if request.kind == "claude" && request.memory_context.is_none() {
        let host = lock(&HOST).as_ref().and_then(Weak::upgrade);
        if let Some(host) = host {
            let snapshot = runs.service.snapshot();
            let workspace = snapshot.workspace(&request.workspace_id);
            request.memory_context = crate::shared_memory::context_for_pane(&host, workspace);
        }
    }
    match app() {
        Some(app) => Ok(match app.send(request).await? {
            DelegationRelease::NotStarted => None,
            DelegationRelease::NothingHeld | DelegationRelease::Released { .. } => Some(()),
        }),
        None => runs.service.start(request).await,
    }
}

/// A confirmation in the page (C# `ShowDelegationDialog` over `WebDialog`): true when the human chose its primary button. None is asked
/// while another sheet is up.
async fn confirm(host: &Arc<Host>, dialog: Value) -> bool {
    let dialog = crate::dialogs::without_nulls(dialog);
    if let Some(answer) = smoke_dialog_answer(host, &dialog) {
        return answer;
    }
    let Some(_sheet) = crate::frame::Sheet::begin(host) else { return false };
    let (_, answer) = crate::dialogs::of(host).open(dialog);
    answer.await.unwrap_or(Value::Null) == Value::Bool(true)
}

/// What answers a confirmation in the delegation smoke instead of the page (C# `smokeDelegationDialog`): it sees the dialog and
/// answers it, true for its primary button.
pub type SmokeDialog = Arc<dyn Fn(&Value) -> bool + Send + Sync>;

static SMOKE_DIALOG: Mutex<Option<SmokeDialog>> = Mutex::new(None);

/// The delegation smoke's dialog driver, or None to let the page answer again (smoke_delegation.rs).
#[cfg(feature = "dev-harness")]
pub fn set_smoke_dialog(driver: Option<SmokeDialog>) {
    *lock(&SMOKE_DIALOG) = driver;
}

/// The smoke driver's answer to `dialog`, in a smoke with one set; None lets the page answer (frame.rs's workspace removal asks it too).
pub fn smoke_dialog_answer(host: &Host, dialog: &Value) -> Option<bool> {
    let driver = host.smoke.as_ref().and_then(|_| lock(&SMOKE_DIALOG).clone())?;
    Some(driver(dialog))
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
    let (provider, plan_first) = {
        let view = host.view();
        let provider = view.snapshot.session(pane).map(|p| p.provider().to_string()).unwrap_or_default();
        let launches = view.panes.get(pane).is_some_and(|ui| ui.view.launches_in_plan_mode);
        (provider, queued.as_ref().map_or(launches, |row| row.permission_mode_override.as_deref() == Some("plan")))
    };
    let to_composer = queue::returns_to_composer(agent.as_deref(), &provider);
    let (host, parent) = (host.clone(), pane.to_string());
    let back = Arc::new(Mutex::new(Some((queued, text.clone(), files.clone()))));
    // A pane that went away meanwhile has no composer to fall back to: the text is the child's failed task on its card.
    let fall_back = {
        let (host, parent, back) = (host.clone(), parent.clone(), back.clone());
        move |reason: String| {
            let Some((queued, text, files)) = lock(&back).take() else { return };
            match queued {
                Some(row) => put_back_queued_row(&host, &parent, row, &reason),
                None if to_composer => return_to_composer(&host, &parent, text, files, &reason),
                None => run_after_no_worktree(&host, &parent, text, files, &reason),
            }
            host.schedule();
        }
    };
    let fall_back = Arc::new(fall_back);
    tauri::async_runtime::spawn(async move {
        let Some(app) = app() else {
            fall_back(host.locale.get("delegation.human.error.switchedOff"));
            return;
        };
        // Delegation acts only with `--allow-state-write` or in a smoke, and git only in a repository it may touch (W7 plan section 4).
        let folder = {
            let view = host.view();
            view.snapshot.session(&parent).map(|p| {
                p.working_folder
                    .clone()
                    .unwrap_or_else(|| view.snapshot.workspace(p.workspace_id()).map(|w| w.path().to_string()).unwrap_or_default())
            })
        };
        if !host.delegation_live() || !folder.is_some_and(|folder| may_touch(&host, Path::new(&folder))) {
            fall_back(crate::setup::verification_mode());
            return;
        }
        let failed_back = fall_back.clone();
        let failed: Arc<dyn Fn(String) -> BoxFuture<'static, ()> + Send + Sync> = Arc::new(move |_child: String| {
            let failed_back = failed_back.clone();
            Box::pin(async move { failed_back(mighty_run::strings::get("delegation.human.error.startFailed")) })
        });
        let answer = app.start_human_child(&parent, &text, files, plan_first, failed, agent.as_deref(), false).await;
        let reason = match answer {
            HumanChildStart::Refused(reason) => Some(text::reason(reason, &host.locale)),
            HumanChildStart::Failed(message) => Some(message),
            HumanChildStart::Started(_) => None,
        };
        if let Some(reason) = reason {
            fall_back(reason);
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
    // A read-only launch (W7 plan D2) hands nothing held to a run: it says so, as a card action does.
    if !host.delegation_live() {
        host.view().error = Some(crate::setup::verification_mode());
        host.schedule();
        return;
    }
    let blocked = {
        let view = host.view();
        let Some(pane) = view.snapshot.session(id).cloned() else { return };
        run_block_reason(host, &view.snapshot, &pane)
    };
    if let Some(blocked) = blocked {
        host.view().error = Some(blocked);
        return;
    }
    let (host, id) = (host.clone(), id.to_string());
    tauri::async_runtime::spawn(async move {
        let release = match app() {
            Some(app) => app.run_next(&id).await,
            None => DelegationRelease::NothingHeld,
        };
        match release {
            DelegationRelease::NotStarted => host.view().error = Some(mighty_run::strings::get("remote.error.startFailed")),
            // Taken meanwhile by another run: the first queued row runs now, as run next did.
            DelegationRelease::NothingHeld => crate::composer::drain_queue(host.clone(), id.clone()),
            DelegationRelease::Released { .. } => {}
        }
        if let Some(ui) = host.view().panes.get_mut(&id) {
            ui.dirty = true;
        }
        host.schedule();
    });
}

/// C# `OffersWorktreeMenu`: whether the worktree button, and its menu, shows on the busy pane (its second send opens a worktree, or
/// only another agent's child could). Read with the state locked.
pub fn offers_worktree_menu(host: &Host, snapshot: &AppSnapshot, pane: &str) -> bool {
    let _ = host;
    let (Some(app), Some(pane)) = (app(), snapshot.session(pane)) else { return false };
    app.offers_worktree_menu(snapshot, pane, false)
}

/// The worktree menu's agents (C# `WebRunNowChoices`), for 실행 중 보내기 and the queued rows' agent pick: each agent's name, its row's
/// words (the mode its child starts in, or why it cannot run now) and whether it can run now.
fn worktree_agents(locale: &mighty_core::locale::Locale, app: &DelegationApp, session: &RunSession) -> Vec<WorktreeAgent> {
    app.worktree_child_choices(session, false)
        .into_iter()
        .map(|choice| {
            let title = choice_title(locale, &choice);
            WorktreeAgent {
                label: mighty_composer::runtime::beta_label(&choice.agent, mighty_composer::runtime::provider_label(&choice.agent), locale),
                detail: mighty_composer::runtime::beta_label(&choice.agent, &title, locale),
                enabled: choice.blocked.is_none(),
                agent: choice.agent,
            }
        })
        .collect()
}

/// C# `WorktreeChoiceTitle(choice, parent, marksShortcut: false)`: the agent and the mode its child starts in, or why it cannot run it.
fn choice_title(locale: &mighty_core::locale::Locale, choice: &DelegationChildChoice) -> String {
    let agent = mighty_composer::runtime::provider_label(&choice.agent);
    let Some(blocked) = choice.blocked else {
        let mode = mighty_composer::runtime::permission_label(&choice.agent, choice.mode.as_deref().unwrap_or(""), locale);
        return locale.get_with("composer.worktreeMenu.agent", &[("agent", agent), ("mode", &mode)]);
    };
    let reason = locale.get(match blocked {
        DelegationChildBlock::NotInstalled => "composer.worktreeMenu.notInstalled",
        DelegationChildBlock::SignedOut => "composer.worktreeMenu.signedOut",
        DelegationChildBlock::NoMode => "composer.worktreeMenu.noMode",
        DelegationChildBlock::Cap => "composer.worktreeMenu.cap",
    });
    locale.get_with("composer.worktreeMenu.blocked", &[("agent", agent), ("reason", &reason)])
}

/// `childAction{id, action: "merge"|"undo"|"discard"}` (C# MainWindow.WebShell.Shell.cs:89-94, MainWindow.Delegation.cs:165-225):
/// ignored while the card is busy or a dialog is open; merge with its merged-discard offer, undo, discard with its confirm. Answers null.
/// Without `--allow-state-write` (and outside a smoke) it only says why it did nothing (W7 plan D2); git goes only where it may.
pub async fn child_action(host: Arc<Host>, args: Value) -> Reply {
    let Some(id) = args.get("id").and_then(Value::as_str).map(str::to_string) else { return Ok(Value::Null) };
    let action = match args.get("action").and_then(Value::as_str).map(str::to_ascii_lowercase).as_deref() {
        Some("merge") => DelegationCardAction::Merge,
        Some("undo") => DelegationCardAction::Undo,
        Some("discard") => DelegationCardAction::Discard,
        _ => return Ok(Value::Null),
    };
    let Some(app) = app() else { return Ok(Value::Null) };
    if app.is_card_busy(&id) || host.view().dialog_open {
        return Ok(Value::Null);
    }
    if !host.delegation_live() || !child_may_be_touched(&host, &app, &id) {
        host.view().error = Some(crate::setup::verification_mode());
        host.schedule();
        return Ok(Value::Null);
    }
    match action {
        DelegationCardAction::Merge => {
            if merge_after_checks(&host, &app, &id).await == Some(DelegationCardResult::Done) {
                offer_discard_after_merge(&host, &app, &id).await;
            }
        }
        DelegationCardAction::Undo => {
            app.undo_child_merge(&id).await;
        }
        DelegationCardAction::Discard => discard_child(&host, &app, &id).await,
    }
    host.schedule();
    Ok(Value::Null)
}

/// A card's merge after 머지 전 점검 (merge_checks.rs): what still warns is asked once more (it never blocks), then the merge as the
/// card makes it; its result, None when it was not asked for or the person kept it. The caller checked the card and the gating.
async fn merge_after_checks(host: &Arc<Host>, app: &DelegationApp, id: &str) -> Option<DelegationCardResult> {
    if let Some(record) = crate::pane_worktree::child_record(id)
        && let Some(reasons) = crate::merge_checks::merge_warning_text(host, &crate::merge_checks::target_of(id, &record), false).await
    {
        let locale = host.locale;
        let dialog = json!({
            "type": "confirm", "title": locale.get("git.checks.merge.title"), "message": reasons,
            "primary": locale.get("git.checks.merge.confirm"), "close": locale.get("resume.cancel"),
            "defaultPrimary": false, "destructive": false,
        });
        if !confirm(host, dialog).await || host.closing() {
            return None;
        }
    }
    app.merge_child(id).await.map(|note| note.result)
}

/// MC-29: a card's action the compare view asks for, with the card's own gating: ignored while the card is busy or a dialog is open,
/// refused (saying why) without `--allow-state-write` or where git may not go. A merge goes through 머지 전 점검 and answers whether
/// it went in now (the compare view then asks about the other children, not about this one); a discard confirms as the card's does.
pub(crate) async fn compare_card_action(host: &Arc<Host>, id: &str, merge: bool) -> Option<DelegationCardResult> {
    let app = app()?;
    if app.is_card_busy(id) || host.view().dialog_open {
        return None;
    }
    if !host.delegation_live() || !child_may_be_touched(host, &app, id) {
        host.view().error = Some(crate::setup::verification_mode());
        host.schedule();
        return None;
    }
    if merge {
        return merge_after_checks(host, &app, id).await;
    }
    discard_child(host, &app, id).await;
    None
}

/// MC-29: after one child of a compare run was merged, the others the person agreed to discard, each without a confirmation of its
/// own unless worktrees nested in it changed meanwhile (then it asks as the card does). True when every one went.
pub(crate) async fn discard_confirmed(host: &Arc<Host>, ids: &[String]) -> bool {
    let Some(app) = app() else { return false };
    let mut all = true;
    for id in ids {
        if !child_may_be_touched(host, &app, id) {
            all = false;
            continue;
        }
        let Some(mut pending) = app.merged_discard_offer(id).await else {
            all = false;
            continue;
        };
        loop {
            match app.discard_child(&pending).await.map(|note| note.result) {
                Some(DelegationCardResult::Done) => break,
                Some(DelegationCardResult::Reconfirm(nested)) => {
                    pending.nested = Some(nested);
                    if !confirm_discard(host, &pending).await {
                        all = false;
                        break;
                    }
                }
                _ => {
                    all = false;
                    break;
                }
            }
        }
    }
    all
}

/// Asks the person (a confirmation over the page, the smoke's driver in a smoke): true for its primary button.
pub(crate) async fn ask_confirm(host: &Arc<Host>, dialog: Value) -> bool {
    confirm(host, dialog).await
}

/// Every folder a card's git may change for the child — its worktree and its parent's checkout — is one delegation may touch
/// ([`may_touch`]: any with `--allow-state-write`, only the smoke's own fixture in a smoke).
fn child_may_be_touched(host: &Host, app: &DelegationApp, id: &str) -> bool {
    let Some(coordinator) = app.coordinator() else { return false };
    let file = coordinator.file();
    let Some(child) = file.children.iter().find(|child| child.id == id) else { return false };
    may_touch(host, Path::new(&child.worktree_path))
        && child.parent_checkout.as_deref().is_none_or(|checkout| may_touch(host, Path::new(checkout)))
}

/// C# `DiscardChild`: the confirmation names the worktrees nested in the child's that go with it; when more are nested by the time it
/// runs, nothing is removed and it asks again naming them all.
async fn discard_child(host: &Arc<Host>, app: &DelegationApp, id: &str) {
    if host.view().dialog_open {
        return;
    }
    let Some(mut pending) = app.ask_to_discard_child(id).await else { return };
    while confirm_discard(host, &pending).await {
        match app.discard_child(&pending).await.map(|note| note.result) {
            Some(DelegationCardResult::Reconfirm(nested)) => pending.nested = Some(nested),
            _ => break,
        }
    }
}

/// C# `ConfirmDiscard`: true when the human confirmed a discard it offered (none is offered when the nested worktrees could not be
/// listed).
async fn confirm_discard(host: &Arc<Host>, pending: &PendingChildDiscard) -> bool {
    let locale = host.locale;
    let offered = pending.nested.is_some();
    let dialog = json!({
        "type": "confirm", "title": text::discard_title(pending, &locale), "message": text::discard_message(pending, &locale),
        "primary": offered.then(|| locale.get("delegation.discard.confirm")),
        "close": if offered { locale.get("resume.cancel") } else { locale.get("common.close") },
        "defaultPrimary": false, "destructive": false,
    });
    confirm(host, dialog).await && offered && !host.closing()
}

/// C# `OfferDiscardAfterMerge`: once a merge from the child's card went through, the offer to discard the child right away.
async fn offer_discard_after_merge(host: &Arc<Host>, app: &DelegationApp, id: &str) {
    if host.view().dialog_open {
        return;
    }
    let Some(mut pending) = app.merged_discard_offer(id).await else { return };
    let locale = host.locale;
    let dialog = json!({
        "type": "confirm", "title": locale.get("delegation.mergedDiscard.title"),
        "message": text::merged_discard_message(&pending, &locale),
        "primary": locale.get("delegation.mergedDiscard.confirm"), "close": locale.get("delegation.mergedDiscard.later"),
        "defaultPrimary": false, "destructive": false,
    });
    if !confirm(host, dialog).await || host.closing() {
        return;
    }
    while let Some(DelegationCardResult::Reconfirm(nested)) = app.discard_child(&pending).await.map(|note| note.result) {
        pending.nested = Some(nested);
        if !confirm_discard(host, &pending).await {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// W9 integration review M1: a delegated run whose window is gone or quitting is refused, never handed on without its style.
    #[test]
    fn a_delegated_request_is_refused_once_the_window_is_gone() {
        let window = Window { host: Weak::new() };
        let request = StartRunRequest::new("pane", "ws", "claude", "go on");
        let answer = tauri::async_runtime::block_on(window.run_request(request));
        assert_eq!(answer.map(|r| r.session_id), Err(CLOSING.to_string()));
    }

    /// W7 review M3 (C# DelegationAppVerification.cs:480-490): the delegation's git hooks folder is `<profile>\git-no-hooks` when the
    /// app may write its profile, this machine's own folder for the profile when it may not, and never under the install folder.
    #[test]
    fn the_hooks_folder_is_the_profiles_or_the_machines_never_the_install_folders() {
        use mighty_platform::Paths;
        let paths = mighty_platform::AppPaths::rooted(&std::env::temp_dir().join("mighty-hooks-check"));
        // The machine's folder start() hands over: the work map's, as main.rs configures it.
        let machine = crate::workmap::Folders::new(&paths, false).hooks;
        let writing = hooks_folder(paths.data_dir(), Some(&machine), true);
        let reading = hooks_folder(paths.data_dir(), Some(&machine), false);
        assert_eq!(writing, paths.data_dir().join("git-no-hooks"), "the profile's when it may be written");
        assert!(
            reading == machine && (!cfg!(windows) || reading.starts_with(paths.local_dir())),
            "the machine's own when the profile may not be written"
        );
        assert_eq!(hooks_folder(paths.data_dir(), None, false), paths.data_dir().join("git-no-hooks"), "without it, the profile's");
        let install = std::env::current_exe().unwrap().parent().unwrap().to_path_buf();
        for hooks in [writing, reading] {
            assert!(hooks.is_absolute() && !hooks.starts_with(&install), "{hooks:?} is absolute and not under the install folder");
        }
    }
}
