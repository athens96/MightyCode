//! Per-pane worktrees in the host (C# `WinUI\MainWindow.PaneWorktree.cs`, from `main` 0.4.60–0.4.61, over mighty-delegation's
//! `pane_worktree`; the page's `web\pane\{WorktreeBar.tsx,worktree.ts}` and `dialogs\Dialogs.tsx`; plan §3 W9-I). Owner: W9-I.
//!
//! A workspace's agent panes that work in their own worktree: made when the pane is added ([`add_new_agent_pane`], where
//! `PaneWorktreePolicy.Decide` says: the workspace folder, a new worktree, or after the person chose in `paneWorktreeChoice`), merged
//! into the workspace branch, committed, undone or discarded from the bar over the pane (`paneWorktreeStatus`, `paneWorktreeAction{merge,
//! commitMerge, resolve, undo, pr, discard, changes, terminal, reveal, port}`, `paneWorktreeDiff`), given their own PORT, the files the
//! workspace copies into them and its setup command, kept on the workspace when their pane closes with work left ([`closed`]), and
//! listed with the ones git still has (`workspaceWorktree{mode, setup, copy, list}`, the sheets `paneWorktreeText`,
//! `paneWorktreeChanges`, `paneWorktreeList`). The phone's create works where the workspace's mode alone says
//! ([`worktree_without_asking`]). `PaneInfo.worktree` (the `paneWorktree.marker` word) is mighty-core's `shell::pane_info`.
//!
//! Gating (plan §4, W7 D2): a git step that writes (a worktree made, a merge, a commit, an undo, a discard) goes only where
//! [`crate::delegation::may_touch`] allows (any repository with the write policy, only the smoke's own fixture in a smoke); otherwise
//! the action says "verification mode" and changes nothing, and a new pane works in the workspace folder, saying why. A push (the pull
//! request) also needs [`Host::may_change_machine`]: never in a smoke. Reading git (the bar's status, the changes, a file's diff, the
//! list) is always allowed.

use std::collections::HashSet;
use std::path::Path;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use mighty_core::model::{KeptPaneWorktree, PaneWorktree, RunSession, Workspace};
use mighty_core::{time, wire};
use mighty_delegation::git::Git;
use mighty_delegation::git::cleanup::ChildDiscardOutcome;
use mighty_delegation::git::paths::DelegationPaths;
use mighty_delegation::git::worktree::{self, ChildWorktreeCheck, ChildWorktreeOutcome, Maker};
use mighty_delegation::pane_worktree::{
    self as core, CommitOutcome, MergeOutcome, PaneWorktreeDecision, Row, Status, UndoOutcome, refusal,
};
use mighty_delegation::request::DelegationReasonCode;
use mighty_delegation::text;
use mighty_platform::SpawnRequest;
use mighty_platform::fake::FixedDiskSpace;
use mighty_run::model::StartRunRequest;
#[cfg(feature = "dev-harness")]
use serde_json::Map;
use serde_json::{Value, json};

use crate::frame::{self, Sheet, act};
use crate::host::{Host, Registry, lock};
use crate::methods::text as arg;

/// The git, the worktree root and the maker the panes' worktrees use: delegation's own (the same root and hooks folder), made once at
/// [`start`].
#[derive(Clone)]
struct Places {
    git: Git,
    root: String,
    paths: DelegationPaths,
    maker: Maker,
}

/// The window's state (C# `paneWorktreeBusy`, `paneWorktreeConflicts`) and the places, one per process as there is one host.
#[derive(Default)]
struct State {
    places: Option<Places>,
    busy: HashSet<String>,
    /// The panes whose last merge was refused for a conflict: their bar offers to ask the agent to resolve it.
    conflicts: HashSet<String>,
}

static STATE: LazyLock<Mutex<State>> = LazyLock::new(Mutex::default);

fn places() -> Option<Places> {
    lock(&STATE).places.clone()
}

/// What answers the step's sheets in the smoke instead of the page (C# `smokeDelegationDialog` for the confirmations): it sees the
/// dialog and gives the page's answer.
type SmokeAnswer = Arc<dyn Fn(&Value) -> Value + Send + Sync>;

static SMOKE_ANSWER: Mutex<Option<SmokeAnswer>> = Mutex::new(None);

// ── the start and the page's methods ────────────────────────────────────────────────────────────────────────────────────

/// After the run engine (main.rs): the git and the root the panes' worktrees use, delegation's own (delegation.rs `start`): a smoke's
/// under its own folder with room for them whatever the disk says, the app's `%USERPROFILE%\.mightycode\worktrees`.
pub fn start(host: &Arc<Host>) {
    let machine = tauri::Manager::try_state::<crate::workmap::Folders>(&host.app).map(|folders| folders.hooks.clone());
    let hooks = match (cfg!(windows), host.smoke.is_some()) {
        (false, _) => std::path::PathBuf::from("/dev/null"),
        (true, true) => crate::delegation::smoke_root(host).join("git-no-hooks"),
        (true, false) => crate::delegation::hooks_folder(&host.data_dir, machine.as_deref(), host.writes()),
    };
    let git = Git::new(host.processes.clone(), hooks);
    let (root, disk): (String, Arc<dyn mighty_platform::DiskSpace>) = match host.smoke {
        Some(_) => {
            (crate::delegation::smoke_root(host).join("worktrees").to_string_lossy().into_owned(), Arc::new(FixedDiskSpace(u64::MAX)))
        }
        None => (worktree::default_root(&crate::composer::dirs_home().to_string_lossy()), crate::platform::disk_space()),
    };
    let maker = Maker::new(&root, git.clone(), disk);
    let paths = DelegationPaths::new(crate::platform::file_ports().handles);
    lock(&STATE).places = Some(Places { git, root: maker.root().to_string(), paths, maker });
}

/// The page's calls for pane worktrees (C# `RegisterPaneWorktreeMethods`).
pub fn register(methods: &mut Registry) {
    methods.add("paneWorktreeStatus", |host: Arc<Host>, args| async move {
        Ok(match arg(&args, "id") {
            Some(id) => status_model(&host, id).await,
            None => Value::Null,
        })
    });
    methods.add("paneWorktreeAction", |host: Arc<Host>, args| async move {
        let (Some(id), Some(action)) = (arg(&args, "id"), arg(&args, "action")) else { return Ok(Value::Null) };
        act(&host, async {
            pane_action(&host, id, action).await;
            Ok(())
        })
        .await
    });
    methods.add("paneWorktreeDiff", |host: Arc<Host>, args| async move {
        let (Some(id), Some(path)) = (arg(&args, "id"), arg(&args, "path")) else { return Ok(Value::Null) };
        let (Some(places), Some(pane)) = (places(), review_record(&host, id)) else { return Ok(Value::Null) };
        Ok(core::file_diff(&places.git, &places.paths, &pane, path).await.map_or(Value::Null, Value::String))
    });
    methods.add("workspaceWorktree", |host: Arc<Host>, args| async move {
        let (Some(workspace), Some(action)) = (arg(&args, "workspaceId"), arg(&args, "action")) else { return Ok(Value::Null) };
        let value = arg(&args, "mode").map(str::to_string);
        act(&host, workspace_action(&host, workspace, action, value)).await
    });
}

// ── the state's helpers ─────────────────────────────────────────────────────────────────────────────────────────────────

/// C# `LocalFolder(workspace)`: the workspace's folder (a local workspace's path).
fn local_folder(workspace: &Workspace) -> String {
    workspace.path().to_string()
}

fn pane_record(host: &Host, id: &str) -> Option<PaneWorktree> {
    host.view().snapshot.session(id).and_then(|s| s.pane_worktree.clone())
}

/// A delegated child's worktree as a pane worktree's record (its branch, the parent's branch as its base, the parent's checkout), so
/// the changes sheet, a file's diff and the review queue read it as they read a pane's.
pub(crate) fn child_record(id: &str) -> Option<PaneWorktree> {
    let coordinator = crate::delegation::app()?.coordinator()?;
    let file = coordinator.file();
    let child = file.children.iter().find(|child| child.id == id)?;
    Some(PaneWorktree {
        worktree_path: Some(child.worktree_path.clone()),
        branch: Some(child.branch.clone()),
        base_branch: Some(child.parent_branch.clone()),
        base_commit: Some(child.base_commit.clone()),
        checkout: Some(child.parent_checkout.clone().unwrap_or_else(|| child.worktree_path.clone())),
        ..PaneWorktree::default()
    })
}

/// The worktree `id` names for reading (the review queue, the changes sheet, 편집기에서 열기): a worktree pane's record, else a
/// delegated child's.
pub(crate) fn review_record(host: &Host, id: &str) -> Option<PaneWorktree> {
    pane_record(host, id).or_else(|| child_record(id))
}

/// What the review queue reads of one worktree (review.rs): how far it is from its base and the size of its work; None when git
/// cannot say (a worktree gone, git failing). Reading git is always allowed.
pub(crate) async fn review_reading(pane: &PaneWorktree) -> Option<(Status, core::DiffStat)> {
    let places = places()?;
    let status = core::status(&places.git, pane).await?;
    let stat = core::diff_stat(&places.git, pane).await?;
    Some((status, stat))
}

/// Whether the pane's last merge was refused for a conflict, and whether an action runs on it now.
pub(crate) fn review_flags(id: &str) -> (bool, bool) {
    let state = lock(&STATE);
    (state.conflicts.contains(id), state.busy.contains(id))
}

/// The review queue's 변경 보기 for a delegated child (a worktree pane's goes through `paneWorktreeAction{changes}`): the same sheet,
/// its files read from the child's worktree.
pub(crate) async fn show_child_changes(host: &Arc<Host>, id: &str) {
    if let Some(record) = child_record(id) {
        show_changes_of(host, id, &record).await;
    }
}

fn system_log(text: String, provider: Option<String>) -> mighty_core::model::LogEntry {
    mighty_core::model::LogEntry::new(wire::new_id(), "system", text, time::now_o(), provider)
}

/// C# `NotePane`: a system line in the pane's transcript.
fn note_pane(host: &Arc<Host>, id: &str, text: String) {
    let _ = host.view().update(|state| {
        if let Some(pane) = state.sessions_mut().iter_mut().find(|p| p.id() == id) {
            let mut next = RunSession::clone(pane);
            let entry = system_log(text, next.provider.clone());
            next.logs.get_or_insert_with(Vec::new).push(entry);
            *pane = Arc::new(next);
        }
    });
    host.schedule();
}

/// C# `UpdatePaneWorktree`.
fn update_pane_worktree(host: &Arc<Host>, id: &str, change: impl FnOnce(&mut PaneWorktree)) {
    let _ = host.view().update(|state| {
        if let Some(pane) = state.sessions_mut().iter_mut().find(|p| p.id() == id && p.pane_worktree.is_some()) {
            let mut next = RunSession::clone(pane);
            if let Some(record) = next.pane_worktree.as_mut() {
                change(record);
            }
            *pane = Arc::new(next);
        }
    });
    host.schedule();
}

/// C# `UpdateWorkspace`.
fn update_workspace(host: &Arc<Host>, id: &str, change: impl FnOnce(&mut Workspace)) {
    let _ = host.view().update(|state| {
        if let Some(workspace) = state.workspaces_mut().iter_mut().find(|w| w.id() == id) {
            change(workspace);
        }
    });
    host.schedule();
}

/// C# `error.Text = …`: the banner.
fn banner(host: &Arc<Host>, text: String) {
    host.view().error = Some(text);
    host.schedule();
}

/// The PORTs the open panes hold.
fn taken_ports(host: &Host) -> HashSet<i32> {
    host.view().snapshot.sessions().iter().filter_map(|s| s.pane_worktree.as_ref().map(|p| p.port)).collect()
}

/// `PaneWorktreePorts.CanBind` as the PORT's search asks it: a smoke binds nothing but loopback port 0 (the part brief), so every port
/// reads free there.
fn port_free(host: &Host, port: i32) -> bool {
    host.smoke.is_some() || core::can_bind(port)
}

/// Whether `workspace` is still the active one.
fn still_active(host: &Host, workspace: &Workspace) -> bool {
    host.view().snapshot.active_workspace_id.as_deref() == Some(workspace.id())
}

/// Whether the git steps that write may touch `folders` (the workspace's checkout, a worktree, the root).
fn may_write(host: &Host, folders: &[&str]) -> bool {
    folders.iter().all(|folder| crate::delegation::may_touch(host, Path::new(folder)))
}

/// A sheet in the page (C# `OpenWebDialog` with `dialogOpen`): the page's answer, null when cancelled or while another sheet is up;
/// the smoke's driver answers instead when one is set.
async fn ask(host: &Arc<Host>, dialog: Value) -> Value {
    let dialog = crate::dialogs::without_nulls(dialog);
    if host.smoke.is_some()
        && let Some(answer) = lock(&SMOKE_ANSWER).clone()
    {
        return answer(&dialog);
    }
    let Some(_sheet) = Sheet::begin(host) else { return Value::Null };
    let (_, answer) = crate::dialogs::of(host).open(dialog);
    answer.await.unwrap_or(Value::Null)
}

/// A confirmation (C# `ShowDelegationDialog` over `WebDialog`): true when the person chose its primary button.
async fn confirm(host: &Arc<Host>, title: String, message: String, primary: String) -> bool {
    let close = host.locale.get("resume.cancel");
    let dialog = json!({
        "type": "confirm", "title": title, "message": message, "primary": primary, "close": close,
        "defaultPrimary": false, "destructive": false,
    });
    ask(host, dialog).await == Value::Bool(true)
}

// ── a new agent pane ────────────────────────────────────────────────────────────────────────────────────────────────────

/// C# `AddNewAgentPane`: a new (not resumed) agent pane for the active workspace, where `PaneWorktreePolicy.Decide` says: the workspace
/// folder, a new worktree, or after the person chose. True when a pane was added.
pub async fn add_new_agent_pane(host: &Arc<Host>, provider: &str, group: Option<&str>) -> Result<bool, String> {
    let (workspace, decision) = {
        let view = host.view();
        let workspace = view.snapshot.active_workspace_id.as_deref().and_then(|id| view.snapshot.workspace(id)).cloned();
        let decision = workspace.as_ref().map(|w| core::decide(w, view.snapshot.sessions().iter().map(|s| s.as_ref()), None));
        (workspace, decision)
    };
    let (Some(workspace), Some(decision)) = (workspace, decision) else { return frame::add_pane(host, "claude", provider, group, |p| p) };
    match decision {
        PaneWorktreeDecision::SameFolder => return frame::add_pane(host, "claude", provider, group, |p| p),
        PaneWorktreeDecision::Worktree => return open_in_worktree(host, &workspace, provider, group, true, None).await,
        PaneWorktreeDecision::Ask => {}
    }
    let refusal = match places() {
        Some(places) => match places.maker.check(&local_folder(&workspace)).await {
            ChildWorktreeCheck::Refused(reason) => Some(text::reason(reason, &host.locale)),
            ChildWorktreeCheck::Ready(_) => None,
        },
        None => Some(text::reason(DelegationReasonCode::NotGit, &host.locale)),
    };
    match (ask_choice(host, refusal.as_deref()).await.as_deref(), refusal.is_none()) {
        (Some("worktree"), true) => open_in_worktree(host, &workspace, provider, group, false, None).await,
        (Some("always"), true) => {
            update_workspace(host, workspace.id(), |w| w.pane_worktree_mode = Some(core::PANE_WORKTREE_FURTHER.into()));
            open_in_worktree(host, &workspace, provider, group, false, None).await
        }
        (Some("same"), _) => frame::add_pane(host, "claude", provider, group, |p| p),
        _ => Ok(false),
    }
}

/// C# `AskPaneWorktreeChoice`: where the new pane works: `worktree`, `always`, `same`, or None when the person cancelled.
async fn ask_choice(host: &Arc<Host>, refusal: Option<&str>) -> Option<String> {
    let locale = host.locale;
    let message = match refusal {
        None => locale.get("paneWorktree.choice.detail"),
        Some(reason) => locale.get_with("paneWorktree.choice.notGit", &[("reason", reason)]),
    };
    let dialog = json!({
        "type": "paneWorktreeChoice", "title": locale.get("paneWorktree.choice.title"), "message": message,
        "canWorktree": refusal.is_none(), "worktree": locale.get("paneWorktree.choice.worktree"),
        "always": locale.get("paneWorktree.choice.always"), "sameFolder": locale.get("paneWorktree.choice.sameFolder"),
        "cancel": locale.get("resume.cancel"),
    });
    ask(host, dialog).await.as_str().map(str::to_string)
}

/// A worktree to reopen (a kept or found one): its session id, record and the pane's title.
struct Reuse {
    id: String,
    record: PaneWorktree,
    title: Option<String>,
}

/// C# `OpenAgentPaneInWorktree`: makes the worktree first (branch `mighty/<id>` at the workspace branch's head), then the pane that
/// works in it. A worktree the person chose that cannot be made opens no pane; one the workspace's mode chose (`fall_back`) opens the
/// pane in the folder and says why.
async fn open_in_worktree(
    host: &Arc<Host>,
    workspace: &Workspace,
    provider: &str,
    group: Option<&str>,
    fall_back: bool,
    reuse: Option<Reuse>,
) -> Result<bool, String> {
    let locale = host.locale;
    let id = reuse.as_ref().map_or_else(wire::new_id, |r| r.id.clone());
    let reopened = reuse.is_some();
    let (record, title) = match reuse {
        Some(reuse) => (reuse.record, reuse.title),
        None => {
            let (made, checkout, reason) = make(host, workspace, &id).await;
            let Some(made) = made else {
                let reason = reason.unwrap_or_default();
                if fall_back {
                    let line = locale.get_with("paneWorktree.log.fellBack", &[("reason", &reason)]);
                    let provider_name = provider.to_string();
                    let group = still_active(host, workspace).then_some(group).flatten();
                    return frame::add_pane_to(host, Some(workspace.id()), "claude", provider, group, move |pane| {
                        let mut pane = pane;
                        pane.logs.get_or_insert_with(Vec::new).push(system_log(line, Some(provider_name)));
                        pane
                    });
                }
                banner(host, locale.get_with("paneWorktree.error.cannotMake", &[("reason", &reason)]));
                return Ok(false);
            };
            let port = core::port(&id, &taken_ports(host), |port| port_free(host, port));
            (core::from_made(&made, &checkout, port, workspace.worktree_setup_command.clone()), None)
        }
    };
    // The make can take minutes: the pane goes into its own workspace whichever one is active by then, in the group only when it is.
    let group = still_active(host, workspace).then_some(group).flatten();
    let port = record.port.to_string();
    let words = [("branch", record.branch()), ("base", record.base_branch()), ("port", port.as_str())];
    let line = locale.get_with(if reopened { "paneWorktree.log.reopened" } else { "paneWorktree.log.made" }, &words);
    let (shaped_id, shaped_record, provider_name) = (id.clone(), record.clone(), provider.to_string());
    let added = frame::add_pane_to(host, Some(workspace.id()), "claude", provider, group, move |pane| {
        let mut pane = RunSession {
            id: Some(shaped_id),
            title: title.or(pane.title.clone()),
            working_folder: Some(shaped_record.run_folder().to_string()),
            pane_worktree: Some(shaped_record),
            ..pane
        };
        pane.logs.get_or_insert_with(Vec::new).push(system_log(line, Some(provider_name)));
        pane
    });
    if host.view().snapshot.session(&id).is_none() {
        // The pane could not be added (the pane limit): a worktree made for it goes again; a kept one stays kept.
        if !reopened && let Some(places) = places() {
            core::discard(&places.git, &places.paths, &record, &id, &places.root, &locale).await;
        }
        return added.map(|_| false);
    }
    if reopened {
        update_workspace(host, workspace.id(), |w| {
            if let Some(kept) = w.kept_pane_worktrees.as_mut() {
                kept.retain(|k| k.session_id() != id);
            }
        });
    } else {
        prepare(host, &id, workspace, record);
    }
    added
}

/// The worktree made for the session `id` of `workspace` (C# `maker.Check` then `MakePane`): it, the checkout it was made from, or why
/// not. Only where the git that writes may touch the workspace and the root.
async fn make(
    host: &Arc<Host>,
    workspace: &Workspace,
    id: &str,
) -> (Option<mighty_delegation::git::worktree::ChildWorktree>, String, Option<String>) {
    let Some(places) = places() else { return (None, String::new(), Some(text::reason(DelegationReasonCode::NotGit, &host.locale))) };
    let folder = local_folder(workspace);
    match places.maker.check(&folder).await {
        ChildWorktreeCheck::Ready(start) => {
            if !may_write(host, &[&folder, &start.repository, &places.root]) {
                return (None, String::new(), Some(crate::setup::verification_mode()));
            }
            match core::make_pane(&places.maker, id, &start, 300.0).await {
                ChildWorktreeOutcome::Created(made) => (Some(made), start.repository, None),
                ChildWorktreeOutcome::Refused(reason) => (None, String::new(), Some(text::reason(reason, &host.locale))),
                ChildWorktreeOutcome::Failed(message) => (None, String::new(), Some(message)),
            }
        }
        ChildWorktreeCheck::Refused(reason) => (None, String::new(), Some(text::reason(reason, &host.locale))),
    }
}

/// C# `WorktreeWithoutAsking`: the worktree a pane added where nobody can be asked (the phone) works in, when the workspace's mode says
/// so: its record, or why the pane works in the folder instead; (None, None) for the folder by the rule.
pub async fn worktree_without_asking(host: &Arc<Host>, workspace: &Workspace, id: &str) -> (Option<PaneWorktree>, Option<String>) {
    let decision = {
        let view = host.view();
        core::decide_without_asking(workspace, view.snapshot.sessions().iter().map(|s| s.as_ref()), None)
    };
    if decision != PaneWorktreeDecision::Worktree {
        return (None, None);
    }
    let locale = host.locale;
    let fell_back = |reason: &str| Some(locale.get_with("paneWorktree.log.fellBack", &[("reason", reason)]));
    let Some(places) = places() else { return (None, fell_back(&text::reason(DelegationReasonCode::NotGit, &locale))) };
    let folder = local_folder(workspace);
    let ChildWorktreeCheck::Ready(start) = places.maker.check(&folder).await else {
        return (None, fell_back(&text::reason(DelegationReasonCode::NotGit, &locale)));
    };
    if !may_write(host, &[&folder, &start.repository, &places.root]) {
        return (None, fell_back(&crate::setup::verification_mode()));
    }
    match core::make_pane(&places.maker, id, &start, 300.0).await {
        ChildWorktreeOutcome::Created(made) => {
            let port = core::port(id, &taken_ports(host), |port| port_free(host, port));
            (Some(core::from_made(&made, &start.repository, port, workspace.worktree_setup_command.clone())), None)
        }
        ChildWorktreeOutcome::Refused(reason) => (None, fell_back(&text::reason(reason, &locale))),
        ChildWorktreeOutcome::Failed(message) => (None, fell_back(&message)),
    }
}

/// The phone's new pane (C# MainWindow.MobileRemote.cs `create`) with its worktree, or with the line saying why it works in the folder.
pub fn with_worktree(created: RunSession, worktree: Option<&PaneWorktree>, fell_back: Option<&str>, host: &Host) -> RunSession {
    let mut created = created;
    if let Some(worktree) = worktree {
        let port = worktree.port.to_string();
        let line = host
            .locale
            .get_with("paneWorktree.log.made", &[("branch", worktree.branch()), ("base", worktree.base_branch()), ("port", &port)]);
        created.working_folder = Some(worktree.run_folder().to_string());
        created.pane_worktree = Some(worktree.clone());
        let provider = created.provider.clone();
        created.logs.get_or_insert_with(Vec::new).push(system_log(line, provider));
    } else if let Some(fell_back) = fell_back {
        let provider = created.provider.clone();
        created.logs.get_or_insert_with(Vec::new).push(system_log(fell_back.to_string(), provider));
    }
    created
}

/// The phone's `create` (C# MainWindow.MobileRemote.cs:506-517): `created` (made from the phone's provider) added to the local workspace
/// `workspace_id`, in its own worktree where the workspace's mode says so ([`worktree_without_asking`]), its files copied and its setup
/// run after. A missing workspace (404) or a full app (409) refuses before any worktree is made; a worktree made for a pane that then
/// cannot be added goes again. The pane's id, or the phone's status and words. Run on a task of its own (phone.rs).
pub async fn phone_create(host: Arc<Host>, workspace_id: String, created: RunSession) -> Result<String, (u16, String)> {
    let limit = || (409, host.locale.get("workspace.paneLimit"));
    let refusal = |view: &crate::host::View| {
        if !view.snapshot.workspaces().iter().any(|w| w.id() == workspace_id && w.remote.is_none()) {
            return Some((404, "Workspace not found.".to_string()));
        }
        (!mighty_core::add_pane::has_room(&view.snapshot)).then(limit)
    };
    let workspace = {
        let view = host.view();
        if let Some(refused) = refusal(&view) {
            return Err(refused);
        }
        view.snapshot.workspace(&workspace_id).cloned()
    };
    let (worktree, fell_back) = match &workspace {
        Some(workspace) => worktree_without_asking(&host, workspace, created.id()).await,
        None => (None, None),
    };
    let created = with_worktree(created, worktree.as_ref(), fell_back.as_deref(), &host);
    let id = created.id().to_string();
    let added = {
        let mut view = host.view();
        match refusal(&view) {
            Some(refused) => Err(refused),
            None => {
                let pane = Arc::new(created);
                view.update(|state| state.sessions_mut().push(pane)).map_err(|error| (409, error))
            }
        }
    };
    if let Err(refused) = added {
        if let (Some(worktree), Some(places)) = (&worktree, places()) {
            core::discard(&places.git, &places.paths, worktree, &id, &places.root, &host.locale).await;
        }
        return Err(refused);
    }
    host.schedule();
    if let (Some(worktree), Some(workspace)) = (worktree, workspace) {
        prepare(&host, &id, &workspace, worktree);
    }
    Ok(id)
}

/// C# `PreparePaneWorktree`: the workspace's files copied into the new worktree (`PaneWorktreeCopy`), then its setup command once, in
/// the pane's run folder; on a task of its own.
pub fn prepare(host: &Arc<Host>, id: &str, workspace: &Workspace, record: PaneWorktree) {
    let (host, id, workspace) = (host.clone(), id.to_string(), workspace.clone());
    tauri::async_runtime::spawn(async move {
        let locale = host.locale;
        if let Some(paths) = workspace.worktree_copy_paths.clone().filter(|paths| !paths.is_empty())
            && let Some(places) = places()
        {
            let (source, destination) = (local_folder(&workspace), record.run_folder().to_string());
            let handles = places.paths.clone();
            let Ok(copied) = tauri::async_runtime::spawn_blocking(move || core::copy(&paths, &source, &destination, &handles)).await else {
                return;
            };
            if !copied.copied.is_empty() {
                note_pane(&host, &id, locale.get_with("paneWorktree.log.copied", &[("paths", &copied.copied.join(", "))]));
            }
            if !copied.skipped.is_empty() {
                let skipped: Vec<String> = copied
                    .skipped
                    .iter()
                    .map(|(path, why)| format!("{path} ({})", locale.get(&format!("paneWorktree.copy.skip.{why}"))))
                    .collect();
                note_pane(&host, &id, locale.get_with("paneWorktree.log.copySkipped", &[("paths", &skipped.join(", "))]));
            }
        }
        let Some(command) = record.setup_command.clone() else { return };
        note_pane(&host, &id, locale.get_with("paneWorktree.log.setupStarted", &[("command", &command)]));
        let environment = record.environment(&local_folder(&workspace));
        let (code, tail) = run_setup_command(&host, &command, record.run_folder(), &environment).await;
        if host.view().snapshot.session(&id).is_none() {
            return;
        }
        update_pane_worktree(&host, &id, |pane| {
            pane.setup_status = Some(if code == 0 { PaneWorktree::DONE } else { PaneWorktree::FAILED }.into());
            pane.setup_exit_code = Some(code);
        });
        let text = if code == 0 {
            locale.get_with("paneWorktree.log.setupDone", &[("command", &command)])
        } else {
            locale.get_with("paneWorktree.log.setupFailed", &[("command", &command), ("code", &code.to_string())])
        };
        note_pane(&host, &id, if tail.is_empty() { text } else { format!("{text}\n\n{tail}") });
    });
}

/// The last `units` UTF-16 units of `text` (C# `text[^units..]`).
fn utf16_suffix(text: &str, units: usize) -> String {
    let all: Vec<u16> = text.encode_utf16().collect();
    String::from_utf16_lossy(&all[all.len().saturating_sub(units)..])
}

/// C# `RunSetupCommand`: the setup command through `cmd /d /s /c` (`%ComSpec%`; `/bin/sh -c` elsewhere), 30 minutes at most: its exit
/// code and last 12 lines (at most 8,000 characters). -1 with why when it could not run or ran out of time.
async fn run_setup_command(host: &Host, command: &str, folder: &str, environment: &indexmap::IndexMap<String, String>) -> (i32, String) {
    let (shell, args): (String, Vec<String>) = if cfg!(windows) {
        let shell = std::env::var("ComSpec").unwrap_or_else(|_| r"C:\Windows\System32\cmd.exe".into());
        (shell, vec!["/d".into(), "/s".into(), "/c".into(), command.into()])
    } else {
        ("/bin/sh".into(), vec!["-c".into(), command.into()])
    };
    let mut request = SpawnRequest::new(shell, args, folder);
    for (key, value) in environment {
        request = request.env(key.clone(), value.clone());
    }
    let mut child = match host.processes.spawn(request).await {
        Ok(child) => child,
        Err(error) => return (-1, error.to_string()),
    };
    let control = child.control();
    // `process.Input.Close()`.
    drop(child.stdin.take());
    let (stdout, stderr) = (child.stdout.take(), child.stderr.take());
    let (mut out, mut err) = (OutputTail::default(), OutputTail::default());
    let waiting = control.clone();
    let finished = {
        let work = async {
            tokio::join!(out.read(stdout), err.read(stderr));
            waiting.wait().await
        };
        tokio::time::timeout(SETUP_TIMEOUT, work).await
    };
    // Past 30 minutes C# kills the process and still says the last lines it wrote, with -1.
    let code = match finished {
        Ok(Ok(code)) => code,
        Ok(Err(_)) => -1,
        Err(_) => {
            control.kill();
            -1
        }
    };
    let _ = control.close().await;
    (code, setup_tail(&out.text(), &err.text()))
}

/// `RunSetupCommand`'s 30 minutes.
const SETUP_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// The end of one of the setup command's outputs: all of it is read (C# lets a verbose setup finish), the last [`OutputTail::KEEP`]
/// bytes kept, more than the 12 lines of at most 8,000 characters the note shows.
#[derive(Default)]
struct OutputTail {
    bytes: std::collections::VecDeque<u8>,
    cut: bool,
}

impl OutputTail {
    const KEEP: usize = 256 * 1024;

    async fn read(&mut self, reader: Option<mighty_platform::process::ChildReader>) {
        use tokio::io::AsyncReadExt as _;
        let Some(mut reader) = reader else { return };
        let mut buffer = [0u8; 8192];
        while let Ok(read) = reader.read(&mut buffer).await {
            if read == 0 {
                return;
            }
            self.bytes.extend(&buffer[..read]);
            let over = self.bytes.len().saturating_sub(Self::KEEP);
            if over > 0 {
                self.bytes.drain(..over);
                self.cut = true;
            }
        }
    }

    /// What was kept as text; a line cut at its start is left out.
    fn text(&self) -> String {
        let bytes: Vec<u8> = self.bytes.iter().copied().collect();
        let start = if self.cut { bytes.iter().position(|b| *b == b'\n').map_or(0, |at| at + 1) } else { 0 };
        String::from_utf8_lossy(&bytes[start..]).into_owned()
    }
}

/// `RunSetupCommand`'s tail: the output, then the errors, as their last 12 lines that are not empty, at most the last 8,000 characters.
fn setup_tail(output: &str, errors: &str) -> String {
    let text = format!("{output}\n{errors}").replace("\r\n", "\n");
    let lines: Vec<&str> = text.split('\n').filter(|line| !line.is_empty()).collect();
    let tail = lines[lines.len().saturating_sub(12)..].join("\n");
    if tail.encode_utf16().count() > 8000 { utf16_suffix(&tail, 8000) } else { tail }
}

// ── the bar ─────────────────────────────────────────────────────────────────────────────────────────────────────────────

/// C# `PaneWorktreeStatusModel`: what the bar over a worktree pane shows beside its record: how far it is from its base, whether an
/// action runs, whether the last merge conflicted. Null for a pane without a worktree.
async fn status_model(host: &Arc<Host>, id: &str) -> Value {
    let Some(pane) = pane_record(host, id) else { return Value::Null };
    let status = match places() {
        Some(places) => own_status(&places, &pane).await,
        None => None,
    };
    let (busy, conflict) = {
        let state = lock(&STATE);
        (state.busy.contains(id), state.conflicts.contains(id))
    };
    json!({
        "ahead": status.map_or(0, |s| s.ahead), "behind": status.map_or(0, |s| s.behind),
        "changed": status.map_or(0, |s| s.changed_files), "known": status.is_some(), "busy": busy, "conflict": conflict,
        "canUndo": pane.merged_commit.is_some() && pane.pre_merge_commit.is_some(),
    })
}

/// C# `PaneWorktreeAction`: one of the bar's actions on the worktree pane `id`.
async fn pane_action(host: &Arc<Host>, id: &str, action: &str) {
    let Some(session) = host.view().snapshot.session(id).cloned() else { return };
    let Some(pane) = session.pane_worktree.clone() else { return };
    let locale = host.locale;
    match action {
        "merge" | "commitMerge" => {
            if writes_refused(host, &pane) {
                return;
            }
            let commit_first = action == "commitMerge";
            busy(host, id, merge_pane(host, id, &pane, commit_first)).await;
        }
        "resolve" => ask_to_resolve(host, &session, &pane).await,
        "undo" => {
            if writes_refused(host, &pane) {
                return;
            }
            busy(host, id, undo_pane_merge(host, id, &pane)).await;
        }
        "pr" => create_pull_request(host, id, &pane).await,
        "discard" => discard_pane_worktree(host, &session, &pane).await,
        "changes" => show_changes(host, &pane).await,
        "terminal" => {
            let (folder, title) =
                (pane.run_folder().to_string(), locale.get_with("paneWorktree.terminal.title", &[("branch", pane.branch())]));
            if let Err(error) = frame::add_pane(host, "shell", "claude", None, move |shell| RunSession {
                working_folder: Some(folder),
                title: Some(title),
                ..shell
            }) {
                banner(host, error);
            }
        }
        "port" => {
            let url = format!("http://localhost:{}", pane.port);
            if let Ok(Err(error)) = tauri::async_runtime::spawn_blocking(move || crate::platform::open_url(&url)).await {
                banner(host, error);
            }
        }
        "reveal" => {
            let folder = std::path::PathBuf::from(pane.run_folder());
            if let Ok(Err(error)) = tauri::async_runtime::spawn_blocking(move || crate::platform::show_in_explorer(&folder)).await {
                banner(host, error);
            }
        }
        // 편집기에서 열기: the folder the pane's runs work in, in the external editor (editor.rs).
        "editor" => {
            if let Err(error) = crate::editor::open_under(host, pane.run_folder(), "", None).await {
                banner(host, error);
            }
        }
        _ => {}
    }
}

/// Without the write policy (or outside the smoke's fixture) a git step that writes does nothing and says why (W7 plan D2).
/// A worktree that is not its own place under the root (or is a link) is never written in either (W7's place rules, beyond C#).
fn writes_refused(host: &Arc<Host>, pane: &PaneWorktree) -> bool {
    if !may_write(host, &[pane.checkout(), pane.worktree_path()]) {
        banner(host, crate::setup::verification_mode());
        return true;
    }
    if !places().is_some_and(|places| core::in_own_place(&places.paths, pane, &places.root)) {
        banner(host, host.locale.get("delegation.card.error.worktreeMisplaced"));
        return true;
    }
    false
}

/// `PaneWorktreeGit.Status` of a worktree in its own place; None (unknown) for one that is not.
async fn own_status(places: &Places, pane: &PaneWorktree) -> Option<Status> {
    if !core::in_own_place(&places.paths, pane, &places.root) {
        return None;
    }
    core::status(&places.git, pane).await
}

/// C# `Busy`: one action at a time on a pane; the bar reads it busy meanwhile.
async fn busy(host: &Arc<Host>, id: &str, work: impl std::future::Future<Output = ()>) {
    /// C#'s `finally`: the pane is no longer busy however the work ends (a dropped call or a panic included).
    struct Busy(String);
    impl Drop for Busy {
        fn drop(&mut self) {
            lock(&STATE).busy.remove(&self.0);
        }
    }
    if !lock(&STATE).busy.insert(id.to_string()) {
        return;
    }
    let _busy = Busy(id.to_string());
    host.schedule();
    work.await;
}

/// C# `MergePane`: the pane's branch into its base, its changes committed first for Commit and merge.
async fn merge_pane(host: &Arc<Host>, id: &str, pane: &PaneWorktree, commit_first: bool) {
    let Some(places) = places() else { return };
    let locale = host.locale;
    if commit_first {
        let left_out = |left: &[String]| locale.get_with("paneWorktree.log.leftOut", &[("paths", &left.join(", "))]);
        match core::commit_all(&places.git, pane, &locale).await {
            CommitOutcome::Unmerged => return note_pane(host, id, locale.get("paneWorktree.refused.unmerged")),
            CommitOutcome::Failed(message) => {
                return note_pane(host, id, locale.get_with("paneWorktree.error.commitFailed", &[("reason", &message)]));
            }
            CommitOutcome::Committed { left_out: left } if left.is_empty() => note_pane(host, id, locale.get("paneWorktree.log.committed")),
            CommitOutcome::Committed { left_out: left } => {
                note_pane(host, id, locale.get("paneWorktree.log.committed"));
                return note_pane(host, id, left_out(&left));
            }
            CommitOutcome::Nothing { left_out: left } if !left.is_empty() => return note_pane(host, id, left_out(&left)),
            CommitOutcome::Nothing { .. } => {}
        }
    }
    match core::merge(&places.git, pane, &locale).await {
        MergeOutcome::Merged { commit, pre_merge } => {
            lock(&STATE).conflicts.remove(id);
            let (merged, before) = (commit.clone(), pre_merge.clone());
            update_pane_worktree(host, id, move |p| {
                p.merged_commit = Some(merged);
                p.pre_merge_commit = Some(before);
            });
            let short: String = commit.chars().take(8).collect();
            let words = [("branch", pane.branch()), ("base", pane.base_branch()), ("commit", short.as_str())];
            note_pane(host, id, locale.get_with("paneWorktree.log.merged", &words));
        }
        MergeOutcome::Refused(reason) => {
            {
                let mut state = lock(&STATE);
                if reason == refusal::MERGE_CONFLICT {
                    state.conflicts.insert(id.to_string());
                } else {
                    state.conflicts.remove(id);
                }
            }
            note_pane(host, id, refusal::text(reason, pane.base_branch(), &locale));
        }
        MergeOutcome::Failed(message) => note_pane(host, id, locale.get_with("paneWorktree.error.mergeFailed", &[("reason", &message)])),
    }
}

/// C# `UndoPaneMerge`.
async fn undo_pane_merge(host: &Arc<Host>, id: &str, pane: &PaneWorktree) {
    let Some(places) = places() else { return };
    let locale = host.locale;
    match core::undo_merge(&places.git, pane, &locale).await {
        UndoOutcome::Undone => {
            update_pane_worktree(host, id, |p| {
                p.merged_commit = None;
                p.pre_merge_commit = None;
            });
            note_pane(host, id, locale.get_with("paneWorktree.log.undone", &[("base", pane.base_branch())]));
        }
        UndoOutcome::Refused(reason) => {
            note_pane(host, id, locale.get_with(&format!("paneWorktree.undo.refused.{reason}"), &[("base", pane.base_branch())]));
        }
        UndoOutcome::Failed(message) => note_pane(host, id, locale.get_with("paneWorktree.error.undoFailed", &[("reason", &message)])),
    }
}

/// C# `AskToResolve`: the conflict request as the pane's next run (`PaneWorktreeNote.ConflictPrompt`); a pane that cannot take a run
/// now says why.
async fn ask_to_resolve(host: &Arc<Host>, session: &RunSession, pane: &PaneWorktree) {
    let mut request = StartRunRequest::new(session.id(), session.workspace_id(), "claude", &pane.conflict_prompt());
    request.model = session.model().to_string();
    request.provider = session.provider().to_string();
    request.settings = session.settings.clone();
    request.resume_id = session.resume_id.clone();
    let started = match host.engine() {
        Ok(runs) => runs.service.start(request).await.map_err(|error| error.to_string()),
        Err(error) => Err(error),
    };
    match started {
        Ok(_) => {
            lock(&STATE).conflicts.remove(session.id());
        }
        Err(reason) => note_pane(host, session.id(), host.locale.get_with("paneWorktree.conflict.notSent", &[("reason", &reason)])),
    }
}

/// C# `CreatePullRequest`: asks, then pushes the branch to `origin` and opens GitHub's compare page, or says it was pushed.
async fn create_pull_request(host: &Arc<Host>, id: &str, pane: &PaneWorktree) {
    let Some(places) = places() else { return };
    let locale = host.locale;
    let Some(origin) = core::origin(&places.git, pane).await else { return note_pane(host, id, locale.get("paneWorktree.pr.noOrigin")) };
    let detail = locale.get_with("paneWorktree.pr.detail", &[("branch", pane.branch()), ("base", pane.base_branch())]);
    if !confirm(host, locale.get("paneWorktree.pr.title"), detail, locale.get("paneWorktree.pr.confirm")).await {
        return;
    }
    // A push reaches another computer: never in a smoke, only with the write policy.
    if !host.may_change_machine() {
        return banner(host, crate::setup::verification_mode());
    }
    busy(host, id, async {
        if let Some(failed) = core::push(&places.git, pane, &locale).await {
            return note_pane(host, id, locale.get_with("paneWorktree.pr.failed", &[("reason", &failed)]));
        }
        match core::github_repository(&origin) {
            Some(repository) => {
                note_pane(host, id, locale.get_with("paneWorktree.pr.pushedOpening", &[("branch", pane.branch())]));
                let url = core::compare_url(&repository, pane.base_branch(), pane.branch());
                if let Ok(Err(error)) = tauri::async_runtime::spawn_blocking(move || crate::platform::open_url(&url)).await {
                    banner(host, error);
                }
            }
            None => note_pane(host, id, locale.get_with("paneWorktree.pr.pushed", &[("branch", pane.branch())])),
        }
    })
    .await;
}

/// The delegated children the sidebar lists now (C# `service.DelegationChildren`).
fn delegation_children() -> Vec<mighty_core::shell::DelegationChildRow> {
    crate::delegation::app().map(|app| app.rows()).unwrap_or_default()
}

/// C# `DiscardPaneWorktree`: asks, then removes the worktree and branch and closes the pane; refused while children branch from it.
async fn discard_pane_worktree(host: &Arc<Host>, session: &RunSession, pane: &PaneWorktree) {
    let locale = host.locale;
    let blocked = {
        let view = host.view();
        core::discard_blocker(session.id(), view.snapshot.sessions().iter().map(|s| s.as_ref()), &delegation_children(), &locale)
    };
    if let Some(blocked) = blocked {
        return note_pane(host, session.id(), blocked);
    }
    let detail = locale.get_with("paneWorktree.discard.detail", &[("branch", pane.branch())]);
    if !confirm(host, locale.get("paneWorktree.discard.title"), detail, locale.get("paneWorktree.discard.confirm")).await {
        return;
    }
    if writes_refused(host, pane) {
        return;
    }
    let Some(places) = places() else { return };
    let id = session.id().to_string();
    busy(host, &id, async {
        if session.status() == "running"
            && let Some(runs) = host.runs()
        {
            let _ = runs.service.stop(&id).await;
        }
        match core::discard(&places.git, &places.paths, pane, &id, &places.root, &locale).await {
            ChildDiscardOutcome::Discarded => {
                // Gone already: the close must not keep it.
                let _ = host.view().update(|state| {
                    if let Some(found) = state.sessions_mut().iter_mut().find(|p| p.id() == id) {
                        *found = Arc::new(RunSession { pane_worktree: None, ..RunSession::clone(found) });
                    }
                });
                if let Err(error) = frame::close_session(host, &id).await {
                    banner(host, error);
                }
            }
            ChildDiscardOutcome::Failed(message) => {
                note_pane(host, &id, locale.get_with("paneWorktree.error.discardFailed", &[("reason", &message)]));
            }
        }
    })
    .await;
}

/// The tail every close of a pane shares (C# `ClosePane`: the window's close, frame.rs, and the phone's, phone.rs): `closed` is the pane
/// as it was before it left the state; once it is gone (and the app is not quitting), its worktree goes or is kept ([`closed`]).
pub async fn after_close(host: &Arc<Host>, id: &str, closed: Option<Arc<RunSession>>) {
    if let Some(pane) = closed.filter(|pane| pane.pane_worktree.is_some())
        && !host.closing()
        && host.view().snapshot.session(id).is_none()
    {
        self::closed(host, &pane).await;
    }
}

/// C# `PaneWorktreeClosed`: after a worktree pane closed (rule 9), a worktree with nothing unmerged and no child on its branch goes
/// with it; one with work or children is kept on the workspace's list (`KeptPaneWorktree`), and the app says where. Called by frame.rs's
/// close once the pane is gone.
pub async fn closed(host: &Arc<Host>, closed: &RunSession) {
    let Some(pane) = closed.pane_worktree.clone() else { return };
    let locale = host.locale;
    let children = host.view().snapshot.sessions().iter().any(|s| s.parent_session_id.as_deref() == Some(closed.id()))
        || delegation_children().iter().any(|c| {
            c.parent_session_id == closed.id()
                && !matches!(
                    c.state,
                    mighty_core::shell::ChildState::Failed
                        | mighty_core::shell::ChildState::Closed
                        | mighty_core::shell::ChildState::Discarded
                )
        });
    if let Some(places) = places()
        && may_write(host, &[pane.checkout(), pane.worktree_path()])
        && !core::kept_on_close(&places.git, &pane, children).await
    {
        if let ChildDiscardOutcome::Failed(message) =
            core::discard(&places.git, &places.paths, &pane, closed.id(), &places.root, &locale).await
        {
            banner(host, locale.get_with("paneWorktree.error.discardFailed", &[("reason", &message)]));
        }
        return;
    }
    let kept = KeptPaneWorktree {
        session_id: Some(closed.id().to_string()),
        title: Some(closed.title().to_string()),
        provider: Some(closed.provider().to_string()),
        worktree: Some(pane.clone()),
        ..KeptPaneWorktree::default()
    };
    let id = closed.id().to_string();
    update_workspace(host, closed.workspace_id(), move |w| {
        let list = w.kept_pane_worktrees.get_or_insert_with(Vec::new);
        list.retain(|k| k.session_id() != id);
        list.push(kept);
    });
    banner(host, locale.get_with("paneWorktree.closedWithWork", &[("branch", pane.branch()), ("path", pane.worktree_path())]));
}

/// C# `ShowPaneWorktreeChanges`: the changes sheet: the files the pane changed against its base, each file's diff asked of
/// `paneWorktreeDiff`.
async fn show_changes(host: &Arc<Host>, pane: &PaneWorktree) {
    let pane_id = host
        .view()
        .snapshot
        .sessions()
        .iter()
        .find(|s| s.pane_worktree.as_ref().is_some_and(|p| p.branch == pane.branch))
        .map(|s| s.id().to_string())
        .unwrap_or_default();
    show_changes_of(host, &pane_id, pane).await;
}

/// The changes sheet of the worktree `pane` that `pane_id` names (a worktree pane, or a delegated child for the review queue): each
/// file's diff is asked of `paneWorktreeDiff` with that id.
async fn show_changes_of(host: &Arc<Host>, pane_id: &str, pane: &PaneWorktree) {
    let Some(places) = places() else { return };
    let locale = host.locale;
    let files = core::changed_files(&places.git, pane).await.unwrap_or_default();
    let title = locale.get_with("paneWorktree.changes.title", &[("branch", pane.branch()), ("base", pane.base_branch())]);
    if host.view().dialog_open {
        return;
    }
    let files: Vec<Value> = files.iter().map(|f| json!({"path": f.path, "status": f.status})).collect();
    ask(
        host,
        json!({
            "type": "paneWorktreeChanges", "title": title, "paneId": pane_id, "files": files,
            "none": locale.get("paneWorktree.changes.none"), "pick": locale.get("paneWorktree.changes.pick"),
        }),
    )
    .await;
}

/// C# `AskWorktreeText`: a one-line setting asked in a sheet: the setup command or the files copied into new worktrees. None when
/// cancelled.
async fn ask_text(host: &Arc<Host>, title: String, detail: String, current: String, placeholder: &str) -> Option<String> {
    let locale = host.locale;
    let dialog = json!({
        "type": "paneWorktreeText", "title": title, "message": detail, "current": current, "placeholder": placeholder,
        "save": locale.get("paneWorktree.setup.save"), "cancel": locale.get("resume.cancel"),
    });
    ask(host, dialog).await.as_str().map(str::to_string)
}

// ── the workspace's items and its list ──────────────────────────────────────────────────────────────────────────────────

/// C# `WorkspaceWorktreeAction`: the workspace row's worktree items: its mode, its setup command, the files it copies, its worktree
/// list. A remote workspace has none.
async fn workspace_action(host: &Arc<Host>, workspace_id: &str, action: &str, value: Option<String>) -> Result<(), String> {
    let Some(workspace) = host.view().snapshot.workspace(workspace_id).filter(|w| w.remote.is_none()).cloned() else { return Ok(()) };
    let locale = host.locale;
    match action {
        "mode" => update_workspace(host, workspace_id, |w| w.pane_worktree_mode = core::saved_mode(value.as_deref())),
        "setup" => {
            let current = workspace.worktree_setup_command.clone().unwrap_or_default();
            if let Some(command) =
                ask_text(host, locale.get("paneWorktree.setup.title"), locale.get("paneWorktree.setup.detail"), current, "npm ci").await
            {
                update_workspace(host, workspace_id, |w| w.worktree_setup_command = core::normalized_command(Some(&command)));
            }
        }
        "copy" => {
            let current = workspace.worktree_copy_paths.clone().unwrap_or_default().join(", ");
            if let Some(list) =
                ask_text(host, locale.get("paneWorktree.copy.title"), locale.get("paneWorktree.copy.detail"), current, ".env, .env.local")
                    .await
            {
                let entries = core::copy_entries(&list);
                update_workspace(host, workspace_id, |w| {
                    w.worktree_copy_paths = core::normalized_copy_paths(entries.iter().map(|e| Some(e.as_str())));
                });
            }
        }
        "list" => show_list(host, workspace).await,
        _ => {}
    }
    Ok(())
}

/// C# `ShowWorktreeList`: the workspace's worktree list (rule 10): each row reopens, merges or deletes; the sheet opens again after an
/// action.
async fn show_list(host: &Arc<Host>, workspace: Workspace) {
    let locale = host.locale;
    let mut workspace = workspace;
    while !host.closing() {
        let rows = worktree_rows(host, &workspace).await;
        if host.view().dialog_open {
            return;
        }
        let n = |value: Option<i32>| value.unwrap_or(0).to_string();
        let listed: Vec<Value> = rows
            .iter()
            .map(|(row, status)| {
                json!({
                    "id": row.session_id, "title": row.title, "branch": row.worktree.branch(), "path": row.worktree.worktree_path(),
                    "origin": row.origin, "originText": locale.get(&format!("paneWorktree.list.origin.{}", row.origin)),
                    "counts": locale.get_with("paneWorktree.list.counts", &[
                        ("ahead", &n(status.map(|s| s.ahead))), ("behind", &n(status.map(|s| s.behind))),
                        ("changed", &n(status.map(|s| s.changed_files))),
                    ]),
                    "actions": row_actions(host, row, *status),
                })
            })
            .collect();
        let answer = ask(
            host,
            json!({
                "type": "paneWorktreeList", "title": locale.get("paneWorktree.list.title"), "empty": locale.get("paneWorktree.list.empty"),
                "close": locale.get("resume.cancel"), "rows": listed,
            }),
        )
        .await;
        let (Some(id), Some(action)) = (answer.get("id").and_then(Value::as_str), answer.get("action").and_then(Value::as_str)) else {
            return;
        };
        let Some((row, _)) = rows.iter().find(|(row, _)| row.session_id == id) else { return };
        if !row_action(host, &workspace, row, action).await {
            return;
        }
        workspace = host.view().snapshot.workspace(workspace.id()).cloned().unwrap_or(workspace);
    }
}

/// C# `RowActions`: an open pane's row shows it; a kept or found one reopens, merges (commits first while files are changed) or
/// deletes.
fn row_actions(host: &Host, row: &Row, status: Option<Status>) -> Value {
    let locale = host.locale;
    if row.origin == core::OPEN {
        return json!([{"action": "show", "label": locale.get("paneWorktree.list.show")}]);
    }
    let merge = if status.is_some_and(|s| s.changed_files > 0) {
        json!({"action": "commitMerge", "label": locale.get("paneWorktree.bar.commitMerge")})
    } else {
        json!({"action": "merge", "label": locale.get("paneWorktree.bar.merge")})
    };
    json!([
        {"action": "reopen", "label": locale.get("paneWorktree.list.reopen")},
        merge,
        {"action": "delete", "label": locale.get("paneWorktree.list.delete")},
    ])
}

/// C# `WorktreeRows`: the rows with their status: open panes, kept ones, and those git has under the root (delegated children's left
/// out).
async fn worktree_rows(host: &Arc<Host>, workspace: &Workspace) -> Vec<(Row, Option<Status>)> {
    let Some(places) = places() else { return vec![] };
    let open: Vec<RunSession> = host
        .view()
        .snapshot
        .sessions()
        .iter()
        .filter(|s| s.workspace_id() == workspace.id() && s.pane_worktree.is_some())
        .map(|s| RunSession::clone(s))
        .collect();
    let check = places.maker.check(&local_folder(workspace)).await;
    let (checkout, base_branch) = match &check {
        ChildWorktreeCheck::Ready(start) => (start.repository.clone(), Some(start.parent_branch.clone())),
        ChildWorktreeCheck::Refused(_) => (
            open.first().and_then(|s| s.pane_worktree.as_ref()).map_or_else(|| local_folder(workspace), |p| p.checkout().to_string()),
            None,
        ),
    };
    let on_disk = core::worktrees(&places.git, &places.paths, &checkout, &places.root).await.unwrap_or_default();
    let (children, taken) = {
        let view = host.view();
        // Delegated children, and every open pane (beyond C#: another workspace's open pane on the same repository is no "found" row
        // to reopen or delete from here).
        let mut children: HashSet<String> = view.snapshot.sessions().iter().map(|s| s.id().to_string()).collect();
        children.extend(delegation_children().into_iter().map(|c| c.id));
        let taken: HashSet<i32> = view.snapshot.sessions().iter().filter_map(|s| s.pane_worktree.as_ref().map(|p| p.port)).collect();
        (children, taken)
    };
    let kept = workspace.kept_pane_worktrees.clone().unwrap_or_default();
    let rows = core::rows(&open, &kept, &on_disk, &children, &checkout, base_branch.as_deref(), |id| {
        core::port(id, &taken, |port| port_free(host, port))
    });
    let mut result = vec![];
    for row in rows {
        let status = own_status(&places, &row.worktree).await;
        result.push((row, status));
    }
    result
}

/// C# `WorktreeRowAction`: a list row's action; true when the list should open again.
async fn row_action(host: &Arc<Host>, workspace: &Workspace, row: &Row, action: &str) -> bool {
    let locale = host.locale;
    let Some(places) = places() else { return false };
    match action {
        "show" => {
            let _ = frame::activate_pane(&mut host.view(), &row.session_id);
            host.schedule();
            false
        }
        "reopen" => {
            if host.view().snapshot.session(&row.session_id).is_some() {
                let _ = frame::activate_pane(&mut host.view(), &row.session_id);
                host.schedule();
                return false;
            }
            if host.view().snapshot.active_workspace_id.as_deref() != Some(workspace.id()) {
                let _ = frame::select_workspace(&mut host.view(), workspace.id());
            }
            let reuse = Reuse {
                id: row.session_id.clone(),
                record: row.worktree.clone(),
                title: (row.origin == core::KEPT).then(|| row.title.clone()),
            };
            if let Err(error) = open_in_worktree(host, workspace, &row.provider, None, false, Some(reuse)).await {
                banner(host, error);
            }
            false
        }
        "merge" | "commitMerge" => {
            if writes_refused(host, &row.worktree) {
                return true;
            }
            if action == "commitMerge" {
                let committed = core::commit_all(&places.git, &row.worktree, &locale).await;
                let blocked = match &committed {
                    CommitOutcome::Committed { left_out } | CommitOutcome::Nothing { left_out } => !left_out.is_empty(),
                    CommitOutcome::Unmerged | CommitOutcome::Failed(_) => true,
                };
                if blocked {
                    banner(host, locale.get("paneWorktree.refused.uncommittedChanges"));
                    return true;
                }
            }
            match core::merge(&places.git, &row.worktree, &locale).await {
                MergeOutcome::Merged { commit, pre_merge } => {
                    let short: String = commit.chars().take(8).collect();
                    let words = [("branch", row.worktree.branch()), ("base", row.worktree.base_branch()), ("commit", short.as_str())];
                    banner(host, locale.get_with("paneWorktree.log.merged", &words));
                    let id = row.session_id.clone();
                    update_workspace(host, workspace.id(), |w| {
                        for kept in w.kept_pane_worktrees.iter_mut().flatten() {
                            if kept.session_id() == id
                                && let Some(record) = kept.worktree.as_mut()
                            {
                                record.merged_commit = Some(commit.clone());
                                record.pre_merge_commit = Some(pre_merge.clone());
                            }
                        }
                    });
                }
                MergeOutcome::Refused(reason) => banner(host, refusal::text(reason, row.worktree.base_branch(), &locale)),
                MergeOutcome::Failed(message) => banner(host, locale.get_with("paneWorktree.error.mergeFailed", &[("reason", &message)])),
            }
            true
        }
        "delete" => {
            let detail = locale.get_with("paneWorktree.list.deleteDetail", &[("branch", row.worktree.branch())]);
            if !confirm(host, locale.get("paneWorktree.list.delete"), detail, locale.get("paneWorktree.list.delete")).await {
                return true;
            }
            if writes_refused(host, &row.worktree) {
                return true;
            }
            match core::discard(&places.git, &places.paths, &row.worktree, &row.session_id, &places.root, &locale).await {
                ChildDiscardOutcome::Failed(message) => {
                    banner(host, locale.get_with("paneWorktree.error.discardFailed", &[("reason", &message)]));
                }
                ChildDiscardOutcome::Discarded => {
                    let id = row.session_id.clone();
                    update_workspace(host, workspace.id(), |w| {
                        if let Some(kept) = w.kept_pane_worktrees.as_mut() {
                            kept.retain(|k| k.session_id() != id);
                        }
                    });
                }
            }
            true
        }
        _ => false,
    }
}

// ── the smoke's step ────────────────────────────────────────────────────────────────────────────────────────────────────

#[cfg(feature = "dev-harness")]
pub use smoke_step::{smoke_checks, smoke_start};

#[cfg(feature = "dev-harness")]
mod smoke_step {
    use super::*;

    /// The keys the step reports (Rust-only: C#'s smoke has no pane worktree step), in the order they are checked.
    const KEYS: [&str; 11] = [
        "paneWorktreeMade",
        "paneWorktreeMarker",
        "paneWorktreeSetup",
        "paneWorktreeStatus",
        "paneWorktreeDiff",
        "paneWorktreeCommitMerge",
        "paneWorktreeUndo",
        "paneWorktreeList",
        "paneWorktreeDiscard",
        "paneWorktreeClosedClean",
        "paneWorktreeRefusedOutside",
    ];

    /// The smoke's pane-worktree step (smoke.rs `Stage::PaneWorktree`, after delegation): in a fixture repository under the smoke's own
    /// folder (`<folder>\Pane Worktree Smoke\repo`, a workspace in `every` mode copying `.env` and running a setup command), a pane is made
    /// in its own worktree the way Ctrl+N makes one, shows its marker, copies and sets up, reads its status and a file's diff, commits and
    /// merges into main, undoes, is listed, and is discarded through the bar's confirmation; a second pane with nothing of its own closes and
    /// takes its worktree with it. Every git step stays in the smoke's folder ([`crate::delegation::may_touch`]). The setup command is
    /// `echo setup-ok` through `%ComSpec% /d /s /c` (`/bin/sh -c` elsewhere), run in the fixture's worktree: deterministic, it changes
    /// nothing outside it. The PORT search binds nothing in a smoke ([`port_free`]). Afterwards the fixture folder goes; the store keeps its
    /// in-memory approval of the folder (it has no forget, and the folder is gone). True when it started; it reports once through
    /// `Smoke::probed`.
    pub fn smoke_start(host: &Arc<Host>) -> bool {
        if host.smoke.is_none() || places().is_none_or(|places| !places.git.available()) {
            return false;
        }
        host.log.line("smoke: pane worktree step (a fixture repository under the smoke's folder)");
        let host = host.clone();
        tauri::async_runtime::spawn(async move {
            let mut checks = Map::new();
            let before = {
                let view = host.view();
                (view.snapshot.active_workspace_id.clone(), view.snapshot.active_session_id.clone())
            };
            let root = crate::delegation::smoke_root(&host).join("Pane Worktree Smoke");
            let outcome = smoke_steps(&host, &root, &mut checks).await;
            smoke_clean_up(&host, &root, before).await;
            let mut report = Value::Object(checks);
            report["passed"] = json!(outcome.is_ok() && KEYS.iter().all(|key| report.get(*key) == Some(&Value::Bool(true))));
            if let Err(error) = outcome {
                host.log.line(&format!("smoke: pane worktree step failed: {error}"));
                report["error"] = json!(error);
            }
            if let Some(smoke) = &host.smoke {
                smoke.probed(&host, report);
            }
        });
        true
    }

    /// The step's checks from its report: each key, and the step as a whole.
    pub fn smoke_checks(report: &Value) -> Vec<(&'static str, bool)> {
        let mut checks: Vec<(&'static str, bool)> = KEYS.iter().map(|key| (*key, report.get(*key) == Some(&Value::Bool(true)))).collect();
        checks.push(("paneWorktreePassed", report.get("passed") == Some(&Value::Bool(true))));
        checks
    }

    fn require(holds: bool, why: &str) -> Result<(), String> {
        if holds { Ok(()) } else { Err(why.to_string()) }
    }

    /// The fixture's git: the smoke's runner, committing as Mighty Smoke, unsigned.
    async fn smoke_git(git: &Git, folder: &Path, arguments: &[&str]) -> Result<String, String> {
        let mut all = vec!["-c", "user.name=Mighty Smoke", "-c", "user.email=smoke@example.invalid", "-c", "commit.gpgsign=false"];
        all.extend_from_slice(arguments);
        match git.run(&all, folder, 30.0).await {
            Some(result) if result.exit_code == 0 => Ok(result.stdout.trim().to_string()),
            Some(result) => Err(format!("git {arguments:?} failed: {}", result.stderr.trim())),
            None => Err(format!("git {arguments:?} did not run")),
        }
    }

    /// Polls until `holds`, at most `seconds`.
    async fn wait_until(what: &str, seconds: u64, mut holds: impl FnMut() -> bool) -> Result<(), String> {
        let end = std::time::Instant::now() + Duration::from_secs(seconds);
        while !holds() {
            if std::time::Instant::now() >= end {
                return Err(format!("the smoke never reached: {what}"));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Ok(())
    }

    /// The pane of `workspace` made last with a worktree.
    fn worktree_pane(host: &Host, workspace: &str, except: &[String]) -> Option<RunSession> {
        host.view()
            .snapshot
            .sessions()
            .iter()
            .rev()
            .find(|s| s.workspace_id() == workspace && s.pane_worktree.is_some() && !except.iter().any(|id| id == s.id()))
            .map(|s| RunSession::clone(s))
    }

    #[allow(clippy::too_many_lines, reason = "the smoke's steps in order, one block each")]
    async fn smoke_steps(host: &Arc<Host>, root: &Path, checks: &mut Map<String, Value>) -> Result<(), String> {
        let places = places().ok_or("no places")?;
        let repo = root.join("repo");
        require(may_write(host, &[&repo.to_string_lossy(), &places.root]), "the fixture is not under the smoke's folder")?;
        let _ = std::fs::remove_dir_all(root);
        std::fs::create_dir_all(&repo).map_err(|e| e.to_string())?;
        let git = &places.git;
        smoke_git(git, &repo, &["init", "-q", "-b", "main"]).await?;
        // The worktrees' commits read the repository's own identity, whatever the machine's git says.
        smoke_git(git, &repo, &["config", "user.name", "Mighty Smoke"]).await?;
        smoke_git(git, &repo, &["config", "user.email", "smoke@example.invalid"]).await?;
        std::fs::write(repo.join("README.md"), "# Pane Worktree Smoke\n").map_err(|e| e.to_string())?;
        std::fs::write(repo.join(".gitignore"), ".env\n").map_err(|e| e.to_string())?;
        smoke_git(git, &repo, &["add", "-A"]).await?;
        smoke_git(git, &repo, &["commit", "-q", "-m", "Start"]).await?;
        std::fs::write(repo.join(".env"), "SMOKE=1\n").map_err(|e| e.to_string())?;
        // The workspace: every agent pane in its own worktree, `.env` copied, a setup command that says it ran.
        let workspace = host.approve_local(&repo).map_err(|e| e.to_string())?;
        let workspace_id = workspace.id().to_string();
        host.view().update(|state| {
            if !state.workspaces().iter().any(|w| w.id() == workspace_id) {
                state.workspaces_mut().push(workspace.clone());
            }
            if let Some(listed) = state.workspaces_mut().iter_mut().find(|w| w.id() == workspace_id) {
                listed.pane_worktree_mode = Some(core::PANE_WORKTREE_EVERY.into());
                listed.worktree_copy_paths = Some(vec![".env".into()]);
                listed.worktree_setup_command = Some("echo setup-ok".into());
            }
            state.active_workspace_id = Some(workspace_id.clone());
        })?;
        // The confirmations are answered yes; the list sheet closes.
        *lock(&SMOKE_ANSWER) = Some(Arc::new(|dialog: &Value| match dialog.get("type").and_then(Value::as_str) {
            Some("confirm") => Value::Bool(true),
            _ => Value::Null,
        }));

        // Made the way Ctrl+N makes one: in its own worktree on mighty/<id>.
        let added = add_new_agent_pane(host, "claude", None).await?;
        let pane = worktree_pane(host, &workspace_id, &[]).ok_or("no pane in its own worktree")?;
        let record = pane.pane_worktree.clone().ok_or("no record")?;
        let id = pane.id().to_string();
        checks.insert(
            "paneWorktreeMade".into(),
            json!(
                added
                    && record.branch() == format!("mighty/{id}")
                    && record.base_branch() == "main"
                    && Path::new(record.worktree_path()).is_dir()
                    && pane.working_folder.as_deref() == Some(record.run_folder())
                    && core::pane_worktree_port(record.port)
            ),
        );
        let marker = {
            let mut view = host.view();
            host.shell_model(&mut view).panes.iter().find(|info| info.id == id).and_then(|info| info.worktree.clone())
        };
        checks.insert("paneWorktreeMarker".into(), json!(marker.is_some_and(|marker| marker.contains(record.branch()))));
        let worktree = std::path::PathBuf::from(record.worktree_path());
        wait_until("the setup command's end", 120, || {
            pane_record(host, &id).is_some_and(|p| p.setup_status.as_deref() != Some(PaneWorktree::RUNNING))
        })
        .await?;
        let set_up = pane_record(host, &id).ok_or("the pane is gone")?;
        checks.insert(
            "paneWorktreeSetup".into(),
            json!(
                set_up.setup_status.as_deref() == Some(PaneWorktree::DONE)
                    && set_up.setup_exit_code == Some(0)
                    && worktree.join(".env").is_file()
            ),
        );
        // The bar's status, then a new file's diff.
        let status = status_model(host, &id).await;
        checks.insert("paneWorktreeStatus".into(), json!(status["known"] == json!(true) && status["changed"] == json!(0)));
        std::fs::write(worktree.join("feature.txt"), "feature\n").map_err(|e| e.to_string())?;
        let diff = core::file_diff(git, &places.paths, &set_up, "feature.txt").await;
        let changed = status_model(host, &id).await;
        checks
            .insert("paneWorktreeDiff".into(), json!(diff.is_some_and(|diff| diff.contains("+feature")) && changed["changed"] == json!(1)));
        // Commit and merge: main holds the file; then undo puts main back.
        let before = smoke_git(git, &repo, &["rev-parse", "HEAD"]).await?;
        pane_action(host, &id, "commitMerge").await;
        let merged = pane_record(host, &id).ok_or("the pane is gone")?;
        let head = smoke_git(git, &repo, &["rev-parse", "HEAD"]).await?;
        checks.insert(
            "paneWorktreeCommitMerge".into(),
            json!(
                repo.join("feature.txt").is_file()
                    && head != before
                    && merged.merged_commit.as_deref() == Some(head.as_str())
                    && merged.pre_merge_commit.as_deref() == Some(before.as_str())
            ),
        );
        pane_action(host, &id, "undo").await;
        let undone = pane_record(host, &id).ok_or("the pane is gone")?;
        checks.insert(
            "paneWorktreeUndo".into(),
            json!(
                !repo.join("feature.txt").exists()
                    && smoke_git(git, &repo, &["rev-parse", "HEAD"]).await? == before
                    && undone.merged_commit.is_none()
            ),
        );
        // The list names the open pane.
        let listed_workspace = host.view().snapshot.workspace(&workspace_id).cloned().ok_or("the workspace is gone")?;
        let rows = worktree_rows(host, &listed_workspace).await;
        checks.insert(
            "paneWorktreeList".into(),
            json!(rows.iter().any(|(row, status)| row.session_id == id && row.origin == core::OPEN && status.is_some())),
        );
        // Discarded through the bar's confirmation: the worktree, the branch and the pane go.
        pane_action(host, &id, "discard").await;
        let branches = smoke_git(git, &repo, &["for-each-ref", "--format=%(refname)", &format!("refs/heads/mighty/{id}")]).await?;
        checks.insert(
            "paneWorktreeDiscard".into(),
            json!(!worktree.exists() && branches.is_empty() && host.view().snapshot.session(&id).is_none()),
        );
        // A second pane with nothing of its own closes and takes its worktree with it.
        add_new_agent_pane(host, "claude", None).await?;
        let second = worktree_pane(host, &workspace_id, &[]).ok_or("no second pane")?;
        let second_worktree = second.pane_worktree.as_ref().map(|p| p.worktree_path().to_string()).unwrap_or_default();
        wait_until("the second setup's end", 120, || {
            pane_record(host, second.id()).is_some_and(|p| p.setup_status.as_deref() != Some(PaneWorktree::RUNNING))
        })
        .await?;
        frame::close_session(host, second.id()).await?;
        let kept = host.view().snapshot.workspace(&workspace_id).and_then(|w| w.kept_pane_worktrees.clone()).unwrap_or_default();
        checks.insert(
            "paneWorktreeClosedClean".into(),
            json!(!Path::new(&second_worktree).exists() && kept.is_empty() && host.view().snapshot.session(second.id()).is_none()),
        );
        // A pane whose worktree lies outside what the smoke may touch: every action that writes says "verification mode" and git is left as
        // it was (plan §4, W7 D2).
        let outside = std::env::temp_dir().join(format!("mighty-pane-worktree-outside-{}", std::process::id()));
        let refused = refused_outside(host, &outside, &workspace_id).await;
        let _ = host.view().update(|state| state.sessions_mut().retain(|s| s.id() != "pane-outside"));
        let removing = outside.clone();
        let _ = tauri::async_runtime::spawn_blocking(move || std::fs::remove_dir_all(removing)).await;
        checks.insert("paneWorktreeRefusedOutside".into(), json!(refused?));
        Ok(())
    }

    /// The refusals' check: a repository and a pane worktree in `outside` (a temporary folder of this process's own, never under the smoke's
    /// folder), the pane in the smoke's workspace with a new file in its worktree; commit-and-merge, merge, undo and discard each refuse
    /// with "verification mode" and leave every ref, the new file and the worktree as they were.
    async fn refused_outside(host: &Arc<Host>, outside: &Path, workspace_id: &str) -> Result<bool, String> {
        let places = places().ok_or("no places")?;
        let git = &places.git;
        let (repo, tree) = (outside.join("repo"), outside.join("pane-outside"));
        require(!may_write(host, &[&repo.to_string_lossy(), &tree.to_string_lossy()]), "the outside folder is one the smoke may touch")?;
        let _ = std::fs::remove_dir_all(outside);
        std::fs::create_dir_all(&repo).map_err(|e| e.to_string())?;
        smoke_git(git, &repo, &["init", "-q", "-b", "main"]).await?;
        std::fs::write(repo.join("README.md"), "# outside\n").map_err(|e| e.to_string())?;
        smoke_git(git, &repo, &["add", "-A"]).await?;
        smoke_git(git, &repo, &["commit", "-q", "-m", "Start"]).await?;
        let tree_text = tree.to_string_lossy().into_owned();
        smoke_git(git, &repo, &["worktree", "add", "-q", "-b", "mighty/pane-outside", &tree_text, "HEAD"]).await?;
        std::fs::write(tree.join("work.txt"), "work\n").map_err(|e| e.to_string())?;
        let refs = || async { smoke_git(git, &repo, &["for-each-ref", "--format=%(refname) %(objectname)"]).await };
        let before = refs().await?;
        let record = PaneWorktree {
            worktree_path: Some(tree_text.clone()),
            branch: Some("mighty/pane-outside".into()),
            base_branch: Some("main".into()),
            base_commit: Some(smoke_git(git, &repo, &["rev-parse", "HEAD"]).await?),
            checkout: Some(repo.to_string_lossy().into_owned()),
            port: core::PANE_WORKTREE_LOWEST_PORT,
            merged_commit: Some(smoke_git(git, &repo, &["rev-parse", "HEAD"]).await?),
            pre_merge_commit: Some(smoke_git(git, &repo, &["rev-parse", "HEAD"]).await?),
            working_folder: Some(tree_text.clone()),
            ..PaneWorktree::default()
        };
        let pane = RunSession {
            id: Some("pane-outside".into()),
            workspace_id: Some(workspace_id.to_string()),
            working_folder: Some(tree_text),
            pane_worktree: Some(record),
            ..RunSession::default()
        };
        host.view().update(|state| state.sessions_mut().push(Arc::new(pane)))?;
        require(pane_record(host, "pane-outside").is_some(), "the outside pane has no record")?;
        let verification = crate::setup::verification_mode();
        let mut all = true;
        for action in ["commitMerge", "merge", "undo", "discard"] {
            host.view().error = None;
            pane_action(host, "pane-outside", action).await;
            let said = host.view().error.clone();
            all &= said.as_deref() == Some(verification.as_str());
        }
        host.view().error = None;
        let status = smoke_git(git, &tree, &["status", "--porcelain"]).await?;
        Ok(all
            && refs().await? == before
            && status.contains("work.txt")
            && tree.is_dir()
            && host.view().snapshot.session("pane-outside").is_some())
    }

    /// What the step added goes, whatever happened: the dialog driver, its panes and workspace (their worktrees with the folder), and the
    /// workspace and pane that were active come back.
    async fn smoke_clean_up(host: &Arc<Host>, root: &Path, before: (Option<String>, Option<String>)) {
        *lock(&SMOKE_ANSWER) = None;
        let added: Vec<String> = host
            .view()
            .snapshot
            .workspaces()
            .iter()
            .filter(|w| Path::new(w.path()).starts_with(root))
            .map(|w| w.id().to_string())
            .collect();
        let panes: Vec<(String, Option<PaneWorktree>)> = host
            .view()
            .snapshot
            .sessions()
            .iter()
            .filter(|s| added.iter().any(|w| w == s.workspace_id()))
            .map(|s| (s.id().to_string(), s.pane_worktree.clone()))
            .collect();
        if let Some(places) = places() {
            for (id, record) in &panes {
                if let Some(runs) = host.runs() {
                    let _ = runs.service.stop(id).await;
                }
                if let Some(record) = record {
                    core::discard(&places.git, &places.paths, record, id, &places.root, &host.locale).await;
                }
            }
        }
        let _ = host.view().update(|state| {
            state.sessions_mut().retain(|s| !added.iter().any(|w| w == s.workspace_id()));
            state.workspaces_mut().retain(|w| !added.iter().any(|id| id == w.id()));
            let (workspace, session) = before.clone();
            if workspace.as_ref().is_none_or(|id| state.workspaces().iter().any(|w| w.id() == id)) {
                state.active_workspace_id = workspace;
                state.active_session_id = session;
            } else {
                state.active_workspace_id = state.workspaces().first().map(|w| w.id().to_string());
                state.active_session_id = None;
            }
        });
        host.schedule();
        let folder = root.to_path_buf();
        let _ = tauri::async_runtime::spawn_blocking(move || std::fs::remove_dir_all(folder)).await;
    }
}
