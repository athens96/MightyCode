//! The desktop service's delegation host (app.rs) over a real run service: a child's pane made beside its parent and saved, the pane
//! as delegation reads it, the app's note, runs started and steered only where they can be, the run ledger fed by the service's own
//! starts and ends (C# DesktopService.Delegation.cs `IDelegationHost`, `DelegationRunBegan`, `DelegationRunEnded`, `PaneClosed`). No
//! CLI is found, so a start fails without a process.

use std::sync::{Arc, Mutex};

use mighty_core::model::{AppSnapshot, RemoteWorkspaceReference, RunSession, RunSettings, Workspace};
use mighty_delegation::app::{DelegationApp, DelegationWindow};
use mighty_delegation::host::{DelegationAgentAvailability, DelegationChildPane, DelegationHost, DelegationPaneActivity};
use mighty_delegation::model::DeliveryRoute;
use mighty_run::ServiceEvent;
use mighty_run::providers::{Finder, ProviderCatalog};
use mighty_run::service::{DelegationRunHooks, RunService, RunServiceOptions};

/// The window's part as the checks give it: what it was told, and a refusal to switch on.
#[derive(Default)]
struct Window {
    heard: Mutex<Vec<String>>,
    blocked: Mutex<Option<String>>,
}

impl Window {
    fn heard(&self) -> Vec<String> {
        self.heard.lock().unwrap().clone()
    }
}

impl DelegationWindow for Window {
    fn run_blocked(&self, _pane: &RunSession) -> Option<String> {
        self.blocked.lock().unwrap().clone()
    }
    fn agent_availability(&self, provider: &str) -> DelegationAgentAvailability {
        if provider == "codex" { DelegationAgentAvailability::SignedOut } else { DelegationAgentAvailability::Ready }
    }
    fn state_changed(&self) {
        self.heard.lock().unwrap().push("state".into());
    }
    fn changed(&self) {
        self.heard.lock().unwrap().push("changed".into());
    }
    fn noted(&self, session_id: &str) {
        self.heard.lock().unwrap().push(format!("noted {session_id}"));
    }
}

struct Place {
    folder: std::path::PathBuf,
    app: Arc<DelegationApp>,
    window: Arc<Window>,
    service: Arc<RunService>,
    events: tokio::sync::mpsc::UnboundedReceiver<ServiceEvent>,
}

impl Drop for Place {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.folder);
    }
}

fn temp(name: &str) -> std::path::PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let count = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("mighty-delegation-app-{name}-{}-{count}", std::process::id()));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn pane(id: &str, workspace: &str) -> RunSession {
    RunSession { id: Some(id.into()), workspace_id: Some(workspace.into()), ..RunSession::default() }
}

/// A local workspace "w" with the panes "parent" (Claude, plan) and "other", a remote workspace "r" with the pane "far", and the app as
/// the service's hooks.
fn place(name: &str) -> Place {
    let folder = temp(name);
    let workspace = Workspace {
        id: Some("w".into()),
        name: Some("W".into()),
        path: Some(folder.to_string_lossy().into_owned()),
        ..Workspace::default()
    };
    let remote = Workspace {
        id: Some("r".into()),
        name: Some("R".into()),
        path: Some("/far".into()),
        remote: Some(RemoteWorkspaceReference {
            connection_id: Some("h".into()),
            workspace_id: Some("x".into()),
            host_name: Some("Far".into()),
            version: 1,
        }),
        ..Workspace::default()
    };
    let parent =
        RunSession { settings: Some(RunSettings { permission_mode: "plan".into(), ..RunSettings::default() }), ..pane("parent", "w") };
    let snapshot = AppSnapshot {
        workspaces: Some(vec![workspace, remote]),
        sessions: Some(vec![Arc::new(parent), Arc::new(pane("other", "w")), Arc::new(pane("far", "r"))]),
        active_workspace_id: Some("w".into()),
        active_session_id: Some("parent".into()),
        ..AppSnapshot::default()
    };
    let window = Arc::new(Window::default());
    let app = DelegationApp::new(window.clone());
    let finder: Finder = Arc::new(|_: String| Box::pin(async { None }));
    let catalog = Arc::new(ProviderCatalog::new(
        Arc::new(mighty_platform::fake::FakeProcessHost::default()),
        Arc::new(mighty_platform::fake::FakeShellEnvironment::new(std::env::vars().collect())),
        Some(finder),
    ));
    let (service, events) = RunService::new(
        snapshot,
        catalog,
        RunServiceOptions {
            plugin_directory: "unused".into(),
            permission_card: false,
            remote: None,
            configure: None,
            admitting: None,
            binding: None,
            delegation: Some(app.clone()),
        },
    );
    let service = Arc::new(service);
    app.attach(service.clone());
    Place { folder, app, window, service, events }
}

fn child(id: &str, folder: &std::path::Path, by_human: bool) -> DelegationChildPane {
    DelegationChildPane {
        session_id: id.into(),
        parent_session_id: "parent".into(),
        mode: "acceptEdits".into(),
        folder: folder.to_string_lossy().into_owned(),
        started_by_human: by_human,
        provider: "claude".into(),
    }
}

/// C# `IDelegationHost.CreatePane` and `Note`: the child's pane beside its parent, linked and saved, the window told; a note in the
/// pane's transcript; nothing for a taken id, a gone parent or a closed pane.
#[tokio::test]
async fn the_host_makes_a_childs_pane_beside_its_parent_and_notes_in_it() {
    let mut place = place("pane");
    let worktree = temp("pane-worktree");
    assert!(place.app.create_pane(child("c1", &worktree, true)).await, "the pane is made");
    let snapshot = place.service.snapshot();
    let made = snapshot.session("c1").unwrap();
    assert!(
        made.parent_session_id.as_deref() == Some("parent")
            && made.working_folder.as_deref() == Some(worktree.to_string_lossy().as_ref())
            && made.settings.as_ref().unwrap().permission_mode == "acceptEdits"
            && made.title.as_deref() == Some("Claude Code"),
        "linked to its parent, in its worktree, in the mode asked for"
    );
    assert_eq!(snapshot.active_session_id.as_deref(), Some("c1"), "the human's child shows at once");
    assert_eq!(place.window.heard(), ["state", "changed"], "the window follows the state and redraws delegation");
    let mut saved = false;
    while let Ok(event) = place.events.try_recv() {
        saved |= matches!(event, ServiceEvent::SaveNow);
    }
    assert!(saved, "the link is saved before its first run");
    assert!(!place.app.create_pane(child("c1", &worktree, false)).await, "a taken id makes no pane");
    let orphan = DelegationChildPane { parent_session_id: "gone".into(), ..child("c2", &worktree, false) };
    assert!(!place.app.create_pane(orphan).await, "a gone parent makes no pane");

    place.app.note("merged into main".into(), "c1".into()).await;
    let logs = place.service.snapshot().session("c1").unwrap().logs().to_vec();
    assert!(logs.last().is_some_and(|l| l.kind() == "system" && l.text() == "merged into main"), "the note is a system line");
    assert_eq!(place.window.heard().last().map(String::as_str), Some("noted c1"));
    let before = place.window.heard().len();
    place.app.note("nobody".into(), "gone".into()).await;
    assert_eq!(place.window.heard().len(), before, "a closed pane takes no note");
    assert_eq!(place.app.agent_availability("codex".into()).await, DelegationAgentAvailability::SignedOut, "the window's word");
    let _ = std::fs::remove_dir_all(&worktree);
}

/// C# `IDelegationHost.PaneState`, `StartRun`, `Deliver` and the run ledger the service's own starts and ends feed.
#[tokio::test]
async fn the_host_reads_panes_and_starts_runs_only_where_they_can_run() {
    let place = place("state");
    let workspace_path = place.folder.to_string_lossy().into_owned();
    let state = place.app.pane_state("parent".into()).await.unwrap();
    assert!(
        state.activity == DelegationPaneActivity::Idle
            && state.permission_mode == "plan"
            && state.folder == workspace_path
            && state.run_id.is_none(),
        "an idle pane with no run since launch"
    );
    assert!(place.app.pane_state("far".into()).await.is_none(), "a remote workspace's pane is unknown to delegation");
    assert!(place.app.pane_state("gone".into()).await.is_none(), "a closed pane is none");

    // No CLI: a start is asked of the service, which begins a run in the ledger and ends it with its error.
    assert!(place.app.start_run("parent".into(), "notice".into()).await.is_none(), "a failed start answers none");
    let first = place.app.run_id("parent").expect("the service began a run in the ledger");
    assert!(first.starts_with("run-"));
    assert!(!place.service.has_route("parent"), "its route ended");
    let state = place.app.pane_state("parent".into()).await.unwrap();
    assert!(
        state.activity == DelegationPaneActivity::Idle && state.run_id.as_deref() == Some(first.as_str()),
        "an errored run leaves it idle"
    );
    assert!(place.app.deliver("notice".into(), "parent".into(), DeliveryRoute::Steer).await.is_none(), "an idle pane takes no steer");

    // A pane idle after a normal finish reads as finished, and as idle while the window would refuse a run there.
    let other = place.service.snapshot().session("other").unwrap().as_ref().clone();
    let run = place.app.began(&other);
    assert!(place.app.ended("other", "completed", false), "a run of this launch ends once");
    assert!(!place.app.ended("other", "completed", false), "a second end is none");
    let state = place.app.pane_state("other".into()).await.unwrap();
    assert!(state.activity == DelegationPaneActivity::Finished && state.run_id.as_deref() == Some(run.as_str()), "finished normally");
    *place.window.blocked.lock().unwrap() = Some("updating".into());
    assert_eq!(place.app.pane_state("other".into()).await.unwrap().activity, DelegationPaneActivity::Idle, "refused: idle");
    assert!(place.app.start_run("other".into(), "notice".into()).await.is_none(), "the window's refusal starts nothing");
    assert_eq!(place.app.run_id("other").as_deref(), Some(run.as_str()), "and asks the service for no run");
    *place.window.blocked.lock().unwrap() = None;

    // A child without its worktree reads as idle after a finish and never runs (worktree_missing).
    let worktree = temp("state-worktree");
    assert!(place.app.create_pane(child("c1", &worktree, false)).await);
    std::fs::remove_dir_all(&worktree).unwrap();
    let c1 = place.service.snapshot().session("c1").unwrap().as_ref().clone();
    place.app.began(&c1);
    place.app.ended("c1", "completed", false);
    assert_eq!(place.app.pane_state("c1".into()).await.unwrap().activity, DelegationPaneActivity::Idle, "no worktree: idle");
    let before = place.app.run_id("c1");
    assert!(place.app.deliver("follow-up".into(), "c1".into(), DeliveryRoute::Queue).await.is_none(), "a queued delivery starts nothing");
    assert_eq!(place.app.run_id("c1"), before, "the service refused it before any run began");

    // A closed pane's runs are forgotten; a stop of an idle pane returns at once.
    place
        .service
        .update(|s| {
            let mut next = s.clone();
            next.sessions_mut().retain(|p| p.id() != "other");
            next
        })
        .unwrap();
    assert!(place.app.run_id("other").is_none(), "the closed pane is forgotten");
    place.app.stop_run("parent".into()).await;
    assert!(place.app.stop_and_wait("parent").await, "nothing to stop");
}
