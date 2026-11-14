//! Where the run service tells delegation of its runs (C# DesktopService.cs:189, 261-275, 346-351): `DelegationRunBegan` once the
//! route is added, its id what `Launch` answers; `DelegationRunEnded` where a status event ends the route, with the app's closing;
//! what follows `Receive` (the offer of pending items, the queued merges) after the host heard the event; `PaneClosed` for each pane
//! a change took out of the state. No CLI is found, so each start ends in an error without a process.

mod common;

use std::sync::{Arc, Mutex};

use mighty_core::model::{AppSnapshot, RunSession, Workspace};
use mighty_run::ServiceEvent;
use mighty_run::model::{RunEvent, StartRunRequest};
use mighty_run::providers;
use mighty_run::service::{DelegationRunHooks, RunService, RunServiceOptions};

/// What the hooks heard, in order.
#[derive(Default)]
struct Heard {
    lines: Mutex<Vec<String>>,
    began: Mutex<u32>,
    going: Mutex<Vec<String>>,
}

impl Heard {
    fn push(&self, line: String) {
        self.lines.lock().unwrap().push(line);
    }
    fn lines(&self) -> Vec<String> {
        self.lines.lock().unwrap().clone()
    }
}

impl DelegationRunHooks for Heard {
    fn worktree_root(&self, pane: &RunSession) -> Option<String> {
        self.push(format!("root {}", pane.id()));
        None
    }
    fn began(&self, pane: &RunSession) -> String {
        let mut count = self.began.lock().unwrap();
        *count += 1;
        self.going.lock().unwrap().push(pane.id().to_string());
        self.push(format!("began {}", pane.id()));
        format!("run-{count}")
    }
    fn ended(&self, session_id: &str, status: &str, closing: bool) -> bool {
        let mut going = self.going.lock().unwrap();
        let was = going.iter().position(|id| id == session_id).map(|at| going.remove(at)).is_some();
        self.push(format!("ended {session_id} {status} {closing} {was}"));
        was
    }
    fn received(&self, event: &RunEvent, ended: bool, notice_turn_ended: bool) {
        if event.kind == "status" {
            self.push(format!("received {} {} {ended} {notice_turn_ended}", event.session_id, event.status.as_deref().unwrap_or("")));
        }
    }
    fn pane_closed(&self, pane: &RunSession) {
        self.push(format!("closed {}", pane.id()));
    }
}

fn service(hooks: Arc<Heard>) -> (RunService, tokio::sync::mpsc::UnboundedReceiver<ServiceEvent>, Workspace) {
    let folder = common::temp("delegation-hooks");
    let workspace = Workspace { name: Some("w".into()), path: Some(folder.to_string_lossy().into_owned()), ..Workspace::default() };
    let pane = |id: &str| Arc::new(RunSession { id: Some(id.into()), workspace_id: workspace.id.clone(), ..RunSession::default() });
    let snapshot = AppSnapshot {
        workspaces: Some(vec![workspace.clone()]),
        sessions: Some(vec![pane("parent"), pane("other")]),
        active_workspace_id: workspace.id.clone(),
        ..AppSnapshot::default()
    };
    let finder: providers::Finder = Arc::new(|_: String| Box::pin(async { None }));
    let catalog = Arc::new(providers::ProviderCatalog::new(
        Arc::new(mighty_platform::fake::FakeProcessHost::default()),
        common::environment(),
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
            delegation: Some(hooks),
        },
    );
    (service, events, workspace)
}

#[tokio::test]
async fn the_service_tells_delegation_of_each_run_and_each_closed_pane() {
    common::korean();
    let heard = Arc::new(Heard::default());
    let (service, mut events, workspace) = service(heard.clone());
    let workspace_id = workspace.id.clone().unwrap();
    // No CLI: the run fails once admitted, and its end reaches delegation once, after the route was added; the launch answers with
    // the id delegation gave the run.
    let started = service.launch(StartRunRequest::new("parent", &workspace_id, "claude", "hello"), Some(""), false).await;
    assert_eq!(started.ok().flatten().as_deref(), Some("run-1"), "the launch answers delegation's run id");
    common::until(|| heard.lines().iter().any(|l| l.starts_with("received parent error")), 10000, "the run's end").await;
    let lines = heard.lines();
    assert_eq!(lines.first().map(String::as_str), Some("root parent"), "the worktree root is read first");
    assert_eq!(lines.get(1).map(String::as_str), Some("began parent"), "then the run begins");
    let ends: Vec<&String> = lines.iter().filter(|l| l.starts_with("ended")).collect();
    assert_eq!(ends, ["ended parent error false true"], "the error's status ends the run once, not closing");
    assert!(
        lines.iter().any(|l| l == "received parent error true false"),
        "what follows the receive hears that delegation's run ended: {lines:?}"
    );
    let at = |line: &str| lines.iter().position(|l| l == line).unwrap();
    assert!(at("ended parent error false true") < at("received parent error true false"), "the end comes before what follows it");
    // The host heard the end before what follows it was told.
    let mut saw_end = false;
    while let Ok(event) = events.try_recv() {
        if let ServiceEvent::Run { event, .. } = event
            && event.kind == "status"
            && event.status.as_deref() == Some("error")
        {
            saw_end = true;
        }
    }
    assert!(saw_end, "the host got the run's end");

    // A second launch is another run.
    let second = service.launch(StartRunRequest::new("parent", &workspace_id, "claude", "again"), Some(""), false).await;
    assert_eq!(second.ok().flatten().as_deref(), Some("run-2"), "each start begins a run of its own");
    common::until(|| heard.lines().iter().filter(|l| l.starts_with("received parent error")).count() == 2, 10000, "the second end").await;
    // A change that takes a pane out tells delegation which, after the change.
    service
        .update(|s| {
            let mut next = s.clone();
            next.sessions_mut().retain(|p| p.id() != "other");
            next
        })
        .unwrap();
    assert_eq!(heard.lines().last().map(String::as_str), Some("closed other"), "the closed pane reaches delegation");
    service.dispose().await;
    common::remove_when_free(std::path::Path::new(workspace.path())).await;
}
