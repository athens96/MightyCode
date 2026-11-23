//! MC-3: a Claude run kept in the background daemon and taken back by a later app, through the run engine, over a real child (the
//! fake CLI through the OS's process host, owned by a daemon served in memory). The first app's run waits on an approval it never
//! answers; the app quits keeping it; the later app's run attaches (starts nothing), writes no prompt, gets the waiting card back, and
//! its answer reaches the CLI, which then ends the run.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use mighty_core::model::{AppSnapshot, RunSession, Workspace};
use mighty_platform::AgentIoListener;
use mighty_platform::fake::{FakeAgentIo, FakePtyHost};
use mighty_run::daemon::client::DaemonClient;
use mighty_run::daemon::owner::{Adoption, KEPT_STATE, RESUME_AT, Route, RoutedProcesses, run_key};
use mighty_run::daemon::server::{Daemon, Options};
use mighty_run::manager::{Emit, ResolveWorkspace};
use mighty_run::model::{RunEvent, StartRunRequest};
use mighty_run::permissions::ToolPermissionRequest;
use mighty_run::providers::ProviderCatalog;
use mighty_run::service::{RunService, RunServiceOptions, ServiceEvent};
use mighty_run::{RunManager, RunManagerOptions};

const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn resolve(workspace: &Workspace) -> ResolveWorkspace {
    let workspace = workspace.clone();
    Arc::new(move |_| {
        let workspace = workspace.clone();
        Box::pin(async move { Ok(workspace) })
    })
}

fn emit(list: &Arc<Mutex<Vec<RunEvent>>>) -> Emit {
    let list = list.clone();
    Arc::new(move |event| list.lock().unwrap().push(event))
}

/// A catalog whose Claude is the fake with `args`, its children through `route` (the daemon while one is connected).
fn catalog(route: &Arc<Route>, args: &[&str]) -> Arc<ProviderCatalog> {
    let command = common::fake_command(args);
    let finder: mighty_run::providers::Finder = Arc::new(move |_provider: String| {
        let found = Some(command.clone());
        Box::pin(async move { found })
    });
    let processes = Arc::new(RoutedProcesses::new(common::processes(), route.clone()));
    Arc::new(ProviderCatalog::new(processes, common::environment(), Some(finder)))
}

#[tokio::test(flavor = "multi_thread")]
async fn a_kept_claude_run_is_taken_back_with_its_waiting_card_and_no_second_prompt() {
    common::korean();
    let directory = common::temp("daemon-adopt");
    let workspace = Workspace { path: Some(directory.to_string_lossy().into_owned()), ..Workspace::default() };
    let workspace_id = workspace.id.clone().unwrap_or_else(|| "w-1".into());
    let plugin = common::plugin(&directory);
    let record = directory.join("kept");
    let args = ["--fake-cli", "claude", &record.to_string_lossy()];
    // The daemon: the OS's own process host, served in memory.
    let daemon = Daemon::new(common::processes(), Arc::new(FakePtyHost::default()), Options::new("0.6.0", TOKEN), Arc::new(|_: &str| {}));
    let io = FakeAgentIo::default();
    let name = io.bind().unwrap();
    let served = tokio::spawn(daemon.clone().serve(Arc::new(io.clone()) as Arc<dyn AgentIoListener>));
    let connect = || async { DaemonClient::connect(&io, &name, TOKEN, "test", Duration::from_secs(5)).await.unwrap() };

    // The first app: the run waits on a tool approval nobody answers.
    let first_route = Route::new();
    first_route.set(Some(connect().await));
    let seen_first: Arc<Mutex<Vec<ToolPermissionRequest>>> = Arc::default();
    let events_first: Arc<Mutex<Vec<RunEvent>>> = Arc::default();
    let mut options = RunManagerOptions::new(&plugin, resolve(&workspace), emit(&events_first));
    let sink = seen_first.clone();
    options.permission_requested = Some(Arc::new(move |value: ToolPermissionRequest| sink.lock().unwrap().push(value)));
    let first = Arc::new(RunManager::new(catalog(&first_route, &args), options));
    first.start(StartRunRequest::new("kept", &workspace_id, "claude", "fixture prompt")).await.unwrap();
    common::until(|| seen_first.lock().unwrap().iter().any(|r| r.id == "ask-1" && r.state == "pending"), 20000, "the card waits").await;
    // It quits keeping its work: let go first, then the run engine is disposed (its run ends here, the child keeps running there).
    first_route.detach().await;
    first.dispose().await;
    assert!(!first.is_running("kept"));
    let later = connect().await;
    let listed = later.list().await.unwrap();
    let kept = listed.iter().find(|p| p.key.as_deref() == Some(run_key("kept", true).as_str())).expect("the run is kept").clone();
    assert!(kept.running, "the CLI still runs in the daemon");
    let from = kept.meta[RESUME_AT].as_u64().expect("where the later app takes it up");
    // MC-38: what the later app needs is left beside it: the CLI's turn watch and its Mods bridge registration.
    let state = &kept.meta[KEPT_STATE];
    assert_eq!(state["provider"], "claude");
    assert!(state["claude"].is_object(), "the turn watch is left");
    assert!(state["bridge"]["url"].as_str().is_some_and(|url| url.starts_with("http://127.0.0.1:")), "the bridge registration is left");

    // The later app takes it back: attached where the first one stopped, the card comes back, its answer reaches the CLI.
    let route = Route::new();
    route.set(Some(later.clone()));
    route.expect(&run_key("kept", true), Adoption { proc: kept.proc.clone(), pid: kept.pid, from });
    // Through the run service, as the app takes a kept run back (`RunService::adopt`): the pane its state says it is.
    let (service, mut service_events) = RunService::new(
        AppSnapshot::default(),
        catalog(&route, &args),
        RunServiceOptions {
            plugin_directory: plugin.clone(),
            permission_card: true,
            remote: None,
            configure: None,
            admitting: None,
            binding: None,
            delegation: None,
        },
    );
    let service = Arc::new(service);
    let pane = RunSession {
        id: Some("kept".into()),
        workspace_id: workspace.id.clone(),
        title: Some("Kept".into()),
        provider: Some("claude".into()),
        ..RunSession::default()
    };
    service
        .update(|s| AppSnapshot {
            workspaces: Some(vec![workspace.clone()]),
            sessions: Some(vec![Arc::new(pane.clone())]),
            active_workspace_id: workspace.id.clone(),
            ..s.clone()
        })
        .unwrap();
    let seen: Arc<Mutex<Vec<ToolPermissionRequest>>> = Arc::default();
    let (answering, sink) = (service.clone(), seen.clone());
    let listener = tokio::spawn(async move {
        while let Some(event) = service_events.recv().await {
            let ServiceEvent::ToolPermission(value) = event else { continue };
            sink.lock().unwrap().push(value.as_ref().clone());
            if value.state == "pending" {
                answering.respond_to_tool_permission(&value.run_id, &value.id, true).await.unwrap();
            }
        }
    });
    let mut request = StartRunRequest::new("kept", &workspace_id, "claude", "(continue)").with_provider("claude");
    request.kept = Some(mighty_run::model::KeptState(state.clone()));
    service.adopt(request).await.unwrap();
    let status = || service.snapshot().session("kept").and_then(|s| s.status.clone());
    common::until(|| !service.is_session_running("kept") && status().as_deref() != Some("running"), 20000, "the taken-back run ends").await;
    let session = service.snapshot().session("kept").unwrap().as_ref().clone();
    service.dispose().await;
    listener.abort();

    let seen = seen.lock().unwrap().clone();
    assert!(seen.iter().any(|r| r.id == "ask-1" && r.state == "pending"), "the waiting card came back: {seen:?}");
    let decision = std::fs::read_to_string(format!("{}.decision", record.display())).expect("the CLI got an answer");
    assert_eq!(common::json(&decision)["response"]["response"]["behavior"], "allow", "the later app's answer reached it");
    let input = std::fs::read_to_string(format!("{}.input", record.display())).unwrap();
    let lines: Vec<serde_json::Value> = input.lines().filter(|l| !l.is_empty()).map(common::json).collect();
    assert_eq!(lines.iter().filter(|l| l["type"] == "user").count(), 1, "the prompt went once, from the first app: {input}");
    assert_eq!(lines.iter().filter(|l| l["request"]["subtype"] == "initialize").count(), 2, "each app said hello to the CLI");
    assert_eq!(session.status.as_deref(), Some("completed"), "the pane ends as the run did");
    let logs = session.logs.clone().unwrap_or_default();
    assert!(!logs.iter().any(|entry| entry.kind.as_deref() == Some("user")), "taking a run back adds no request of its own: {logs:?}");
    daemon.stop();
    let _ = served.await;
    common::remove_when_free(&directory).await;
}
