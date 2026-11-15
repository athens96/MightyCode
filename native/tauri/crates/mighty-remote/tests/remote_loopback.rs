//! C# RemoteLoopbackVerification (Verification.cs L902-L912): the sharing host and the client in one process over loopback port 0
//! (injected in code, as only checks may). The real command's run needs the Windows process host.

mod support;

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use indexmap::IndexMap;
use mighty_core::model::{PlanRecord, RunSettings, Workspace};
use mighty_platform::BoxFuture;
use mighty_platform::fake::MemoryVault;
use mighty_remote::remote::client::RemoteClient;
use mighty_remote::remote::host::{Cancel, HostRunner, RemoteHost, RunnerSinks};
use mighty_remote::remote::strings;
use mighty_remote::remote::transport::{self, Method, RemoteTarget, Request, TransportError};
use mighty_remote::remote::wire::*;
use mighty_run::model::StartRunRequest;
use mighty_run::plan_mode::PlanDecision;
use mighty_run::questionnaire::UserQuestionAnswer;
use mighty_run::strings::{get, get_with, set_language};
use serde_json::Value;
use support::fake_host::*;

struct Pair {
    fixture: HostFixture,
    client: RemoteClient,
    workspace: Workspace,
    seen: Arc<Collected>,
    folder: std::path::PathBuf,
    state: RemoteHostState,
}

impl Pair {
    fn target(&self) -> RemoteTarget {
        loopback(self.state.port.unwrap())
    }
    fn key(&self) -> String {
        self.state.key.clone().unwrap()
    }
    async fn close(self) {
        self.client.dispose().await;
        self.fixture.dispose().await;
        let _ = std::fs::remove_dir_all(&self.folder);
    }
}

async fn pair(make: MakeHost) -> Pair {
    let fixture = make_host(make).await;
    let state = fixture.host.start(&[fixture.shared.id().into()], 0).await.unwrap();
    let (store, folder) = client_store();
    let seen = Arc::new(Collected::default());
    let client = client_with(&store, &folder, Arc::new(MemoryVault::default()), &seen, loopback_options());
    let info = client.connect("", &format!("127.0.0.1:{}", state.port.unwrap()), state.key.as_deref().unwrap()).await.unwrap();
    let workspace = client.open_workspace(&info.id, fixture.shared.id()).unwrap();
    Pair { fixture, client, workspace, seen, folder, state }
}

fn settings(mode: &str) -> RunSettings {
    RunSettings { permission_mode: mode.into(), ..RunSettings::default() }
}

fn wire_start(workspace: &str, kind: &str, input: &str, provider: &str, mode: &str) -> RemoteWireStart {
    RemoteWireStart {
        request: RemoteWireRequest {
            session_id: "mac-pane".into(),
            workspace_id: workspace.into(),
            kind: kind.into(),
            input: input.into(),
            model: "default".into(),
            provider: provider.into(),
            settings: settings(mode),
            resume_id: None,
            attachments: None,
        },
        registered_models: vec![],
        permission_mode_override: None,
        todo_progress: None,
        phase_models: None,
        antigravity_grant: None,
    }
}

async fn post(target: &RemoteTarget, key: &str, path: &str, body: &Value) -> Result<Vec<u8>, TransportError> {
    let bytes = to_bytes(body);
    transport::request(target, key, Method::Post, path, Request { body: Some(&bytes), ..Request::default() }).await
}

/// A Mac client's start (no graph header), straight to the host; the host's job id.
async fn start_raw(target: &RemoteTarget, key: &str, workspace: &str, kind: &str, input: &str) -> String {
    let body = wire_start(workspace, kind, input, "claude", "manual").to_value();
    RemoteWireJob::decode(&post(target, key, "/v2/runs", &body).await.unwrap()).unwrap().job_id
}

async fn poll_raw(target: &RemoteTarget, key: &str, job: &str, cursor: i64) -> RemoteWirePoll {
    let data =
        transport::request(target, key, Method::Get, &format!("/v2/runs/{job}/events?cursor={cursor}"), Request::default()).await.unwrap();
    RemoteWirePoll::decode(&data).unwrap()
}

fn started(fixture: &HostFixture) -> Vec<StartRunRequest> {
    fixture.runner().map(|r| r.started.lock().unwrap().clone()).unwrap_or_default()
}

/// "remote loopback: commands and full access wait for the switch, then run on the host".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn commands_and_full_access_wait_for_the_switch_then_run_on_the_host() {
    set_language("ko");
    let p = pair(MakeHost::default()).await;
    let refused = p.client.start(StartRunRequest::new("pane-1", p.workspace.id(), "shell", "dir"), &p.workspace).await.unwrap_err();
    assert!(refused.status() == Some(403) && refused.message() == get("remote.host.error.commandsOff"), "commands wait for the switch");
    let full = StartRunRequest::new("pane-1", p.workspace.id(), "claude", "hi").with_settings(settings("fullAccess"));
    let refused = p.client.start(full, &p.workspace).await.unwrap_err();
    assert!(
        refused.status() == Some(403) && refused.message() == get("remote.host.error.fullAccessOff"),
        "full access waits for the switch"
    );
    assert!(started(&p.fixture).is_empty(), "nothing ran");
    p.fixture.host.set_allows_full_access(true);
    p.client.start(StartRunRequest::new("pane-1", p.workspace.id(), "shell", "dir"), &p.workspace).await.unwrap();
    until(|| p.seen.events().iter().any(|e| e.session_id == "pane-1" && e.status.as_deref() == Some("completed")), 10_000, "completed")
        .await;
    let shown: Vec<String> = p
        .seen
        .events()
        .iter()
        .filter(|e| e.session_id == "pane-1")
        .map(|e| format!("{}:{}", e.kind, e.status.clone().or_else(|| e.entry.as_ref().and_then(|l| l.text.clone())).unwrap_or_default()))
        .collect();
    assert_eq!(shown, ["status:running", "log:echo:dir", "status:completed"], "the host's events became the pane's in order");
    let ran = started(&p.fixture);
    assert!(ran.len() == 1 && ran[0].workspace_id == p.fixture.shared.id() && ran[0].session_id != "pane-1", "under its own job id");
    assert_eq!(p.fixture.host.state().active_runs, 0, "a finished run is not active");
    p.close().await;
}

/// "remote loopback: a Windows host refuses Codex and Antigravity runs until the switch is on".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_windows_host_refuses_codex_and_antigravity_until_the_switch_is_on() {
    set_language("ko");
    let fixture = make_host(MakeHost { windows_host: Some(true), runtime: Some("info-darwin.json"), ..MakeHost::default() }).await;
    let state = fixture.host.start(&[fixture.shared.id().into()], 0).await.unwrap();
    let (target, key) = (loopback(state.port.unwrap()), state.key.clone().unwrap());
    let start = |provider: &'static str, mode: &'static str| {
        let (target, key, workspace) = (target.clone(), key.clone(), fixture.shared.id().to_string());
        async move { post(&target, &key, "/v2/runs", &wire_start(&workspace, "claude", "hi", provider, mode).to_value()).await }
    };
    for (provider, mode) in [("codex", "acceptEdits"), ("codex", "manual"), ("gemini", "acceptEdits")] {
        let refused = start(provider, mode).await.unwrap_err();
        assert!(refused.status() == Some(403) && refused.message() == strings::host_agent_needs_full_access(provider), "{provider} {mode}");
    }
    assert!(started(&fixture).is_empty(), "nothing ran");
    start("claude", "acceptEdits").await.unwrap();
    until(|| started(&fixture).len() == 1, 10_000, "the Claude run").await;
    assert_eq!(started(&fixture)[0].provider, "claude", "a Claude run in its own mode starts without the switch");
    fixture.host.set_allows_full_access(true);
    start("codex", "acceptEdits").await.unwrap();
    until(|| started(&fixture).len() == 2, 10_000, "the Codex run").await;
    assert_eq!(started(&fixture)[1].provider, "codex", "with the switch on a Codex run starts");
    let refused = start("gemini", "acceptEdits").await.unwrap_err();
    assert!(
        refused.status() == Some(400)
            && refused.message() == get_with("remote.host.error.cliMissing", &[("name", mighty_run::providers::name("gemini"))])
    );
    fixture.dispose().await;
}

/// "remote loopback: tool requests are answered from the client and graph runs go only where asked".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tool_requests_are_answered_from_the_client_and_graph_runs_go_only_where_asked() {
    set_language("ko");
    let p = pair(MakeHost::default()).await;
    let (target, key) = (p.target(), p.key());
    p.client.start(StartRunRequest::new("pane-2", p.workspace.id(), "claude", "ask"), &p.workspace).await.unwrap();
    until(|| p.seen.permissions().iter().any(|r| r.state == "pending"), 10_000, "the request").await;
    let first = p.seen.permissions()[0].clone();
    assert!(
        first.run_id == "pane-2" && first.id == "perm-1" && first.input_json == "{\"command\":\"ls\"}",
        "the host's tool request is the pane's"
    );
    let job = started(&p.fixture)[0].session_id.clone();
    let asked = String::from_utf8(
        transport::request(&target, &key, Method::Get, &format!("/v2/runs/{job}/events?cursor=0"), Request::default()).await.unwrap(),
    )
    .unwrap();
    assert!(asked.contains("\"state\":\"pending\"") && asked.contains("\"canAllow\":true"), "a tool request carries its state: {asked}");
    p.client.respond_to_permission("pane-2", "perm-1", true).await.unwrap();
    until(|| p.seen.events().iter().any(|e| e.session_id == "pane-2" && e.status.as_deref() == Some("completed")), 10_000, "completed")
        .await;
    let answers = p.fixture.runner().unwrap().answers.lock().unwrap().clone();
    assert_eq!(answers, [(job.clone(), "perm-1".to_string(), true)], "the answer reached the host's run");
    assert!(p.seen.permissions().iter().any(|r| r.id == "perm-1" && r.state == "allowed"), "the settled request reached the pane");
    p.client.start(StartRunRequest::new("pane-3", p.workspace.id(), "claude", "graph"), &p.workspace).await.unwrap();
    until(|| p.seen.events().iter().any(|e| e.session_id == "pane-3" && e.status.as_deref() == Some("completed")), 10_000, "completed")
        .await;
    assert!(
        p.seen.events().iter().any(|e| e.session_id == "pane-3"
            && e.kind == "graph_run"
            && e.graph_run.as_ref().and_then(|g| g.id.as_deref()) == Some("gr-1")),
        "a Windows client that asked gets graph runs"
    );
    // A client that did not ask (the Mac's) gets graph nodes and no graph runs.
    let job = start_raw(&target, &key, p.fixture.shared.id(), "claude", "graph").await;
    let mut poll = String::new();
    for _ in 0..500 {
        poll = String::from_utf8(
            transport::request(&target, &key, Method::Get, &format!("/v2/runs/{job}/events?cursor=0"), Request::default()).await.unwrap(),
        )
        .unwrap();
        if poll.contains("\"done\":true") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(poll.contains("\"type\":\"graph\"") && !poll.contains("graph_run"), "a Mac client gets the Mac's graph events: {poll}");
    let value: Value = serde_json::from_str(&poll).unwrap();
    let mut stamps = vec![];
    collect_stamps(&value, &mut stamps);
    let mac = |t: &str| (t.len() == 20 || t.len() == 24) && t.ends_with('Z') && t.as_bytes()[10] == b'T';
    assert!(stamps.len() >= 2 && stamps.iter().all(|t| mac(t)), "every timestamp is one the Mac reads: {poll}");
    p.close().await;
}

/// M-C, `PlatformProfile::remote_graph_runs` false: a Mac host ignores a client's `x-mighty-remote-graph: run` and sends the Mac's
/// graph events (graph nodes, no `graph_run`); the Windows value is the check above.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_mac_host_ignores_the_graph_runs_header() {
    set_language("en");
    let fixture = make_host(MakeHost { graph_runs: Some(false), ..MakeHost::default() }).await;
    let state = fixture.host.start(&[fixture.shared.id().into()], 0).await.unwrap();
    let (target, key) = (loopback(state.port.unwrap()), state.key.clone().unwrap());
    let body = serde_json::to_vec(&wire_start(fixture.shared.id(), "claude", "graph", "claude", "manual").to_value()).unwrap();
    let options = Request { headers: &[(protocol::GRAPH_HEADER, protocol::GRAPH_RUNS)], body: Some(&body), ..Request::default() };
    let job = RemoteWireJob::decode(&transport::request(&target, &key, Method::Post, "/v2/runs", options).await.unwrap()).unwrap().job_id;
    let mut poll = String::new();
    for _ in 0..500 {
        poll = String::from_utf8(
            transport::request(&target, &key, Method::Get, &format!("/v2/runs/{job}/events?cursor=0"), Request::default()).await.unwrap(),
        )
        .unwrap();
        if poll.contains("\"done\":true") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(poll.contains("\"type\":\"graph\"") && !poll.contains("graph_run"), "the header is ignored: {poll}");
    fixture.dispose().await;
}

fn collect_stamps(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Object(o) => {
            for (k, v) in o {
                match (k.as_str(), v) {
                    ("updatedAt" | "timestamp", Value::String(s)) => out.push(s.clone()),
                    _ => collect_stamps(v, out),
                }
            }
        }
        Value::Array(items) => items.iter().for_each(|i| collect_stamps(i, out)),
        _ => {}
    }
}

/// "remote loopback: unshared folders, busy hosts and oversize requests are refused".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unshared_folders_busy_hosts_and_oversize_requests_are_refused() {
    set_language("ko");
    let holding = Arc::new(AtomicBool::new(false));
    let hold = holding.clone();
    let admission: mighty_remote::remote::host::Admission =
        Arc::new(move |_| hold.load(Ordering::SeqCst).then(|| get("run.blocked.changingPlugins")));
    let p = pair(MakeHost { admission: Some(admission), ..MakeHost::default() }).await;
    let (target, key, port) = (p.target(), p.key(), p.state.port.unwrap());
    let unshared = wire_start(p.fixture.private.id(), "claude", "hi", "claude", "manual");
    let refused = post(&target, &key, "/v2/runs", &unshared.to_value()).await.unwrap_err();
    assert!(refused.status() == Some(403) && refused.message() == get("remote.host.error.notShared"), "an unshared workspace is refused");
    let mut remote = unshared.clone();
    remote.request.workspace_id = p.fixture.remote.id().into();
    assert_eq!(
        post(&target, &key, "/v2/runs", &remote.to_value()).await.unwrap_err().status(),
        Some(403),
        "a remote workspace is never a folder here"
    );
    holding.store(true, Ordering::SeqCst);
    let busy = p.client.start(StartRunRequest::new("pane-4", p.workspace.id(), "claude", "hi"), &p.workspace).await.unwrap_err();
    assert!(matches!(&busy, mighty_remote::remote::client::ClientError::Transport(t) if t.status() == Some(503) && t.transient()), "held");
    holding.store(false, Ordering::SeqCst);
    let head = |extra: &str| format!("POST /v2/runs HTTP/1.1\r\nAuthorization: Bearer {key}\r\nx-mighty-remote-version: 2\r\n{extra}\r\n");
    let huge = raw_text(port, &head(&format!("Content-Type: application/json\r\nContent-Length: {}\r\n", 13 * 1024 * 1024))).await;
    assert!(huge.starts_with("HTTP/1.1 413"), "over 12 MiB is refused before reading");
    let bad = raw_text(
        port,
        &format!("GET /v2/runs/nope/events?cursor=0 HTTP/1.1\r\nAuthorization: Bearer {key}\r\nx-mighty-remote-version: 2\r\n\r\n"),
    )
    .await;
    assert!(bad.starts_with("HTTP/1.1 404"), "an unknown run is gone");
    let empty = raw_text(port, &head("Content-Type: application/json\r\nContent-Length: 0\r\n")).await;
    assert!(empty.starts_with("HTTP/1.1 400"), "a start with no body is refused: {empty}");
    // Over 512 KiB is only for a run that brings attachments.
    let padded = format!(
        r#"{{"request":{{"sessionId":"x","workspaceId":"{}","kind":"claude","input":"hi","model":"default"}},"registeredModels":[]{}}}"#,
        p.fixture.shared.id(),
        " ".repeat(protocol::MAXIMUM_REQUEST_BYTES)
    );
    let oversize =
        transport::request(&target, &key, Method::Post, "/v2/runs", Request { body: Some(padded.as_bytes()), ..Request::default() }).await;
    assert_eq!(oversize.unwrap_err().status(), Some(413), "a start over 512 KiB without attachments is refused");
    let mut crowded = unshared.clone();
    crowded.request.workspace_id = p.fixture.shared.id().into();
    crowded.registered_models =
        (0..257).map(|i| RemoteWireModel { name: format!("model-{i}"), supports_effort: false, supported_effort_levels: vec![] }).collect();
    let refused = post(&target, &key, "/v2/runs", &crowded.to_value()).await.unwrap_err();
    assert!(refused.status() == Some(400) && refused.message() == get("wire.startRun.invalidFormat"), "more than 256 registered models");
    p.fixture.host.set_allows_full_access(true);
    let long_command = format!("Write-Output '{}'", "x".repeat(40_000));
    let refused = p.client.start(StartRunRequest::new("pane-8", p.workspace.id(), "shell", &long_command), &p.workspace).await.unwrap_err();
    assert!(refused.status() == Some(400) && refused.message() == strings::host_command_too_long(), "a command too long for Windows");
    assert!(started(&p.fixture).is_empty(), "nothing refused ever ran");
    p.close().await;
}

/// "remote loopback: a run nobody polls is stopped and a stopped host ends the client's runs".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_run_nobody_polls_is_stopped_and_a_stopped_host_ends_the_clients_runs() {
    set_language("ko");
    let unpolled = make_host(MakeHost { heartbeat: Some(Duration::from_secs(1)), ..MakeHost::default() }).await;
    let sharing = unpolled.host.start(&[unpolled.shared.id().into()], 0).await.unwrap();
    let (target, key) = (loopback(sharing.port.unwrap()), sharing.key.clone().unwrap());
    let job = start_raw(&target, &key, unpolled.shared.id(), "claude", "hold").await;
    until(|| unpolled.runner().is_some_and(|r| r.stopped.lock().unwrap().contains(&job)), 6000, "the heartbeat stops it").await;
    let poll = poll_raw(&target, &key, &job, 0).await;
    assert!(poll.done && poll.events.last().unwrap().event["status"] == "stopped", "an unpolled run was stopped after the heartbeat");
    unpolled.dispose().await;
    // The client polls at most every 850 ms while nothing comes: a 3 s heartbeat leaves it room on a slow machine.
    let p = pair(MakeHost { heartbeat: Some(Duration::from_secs(3)), ..MakeHost::default() }).await;
    p.fixture.host.set_allows_full_access(true);
    p.client.start(StartRunRequest::new("pane-5", p.workspace.id(), "shell", "hold"), &p.workspace).await.unwrap();
    until(|| p.seen.events().iter().any(|e| e.session_id == "pane-5" && e.kind == "log"), 10_000, "the echo").await;
    tokio::time::sleep(Duration::from_secs(6)).await;
    assert!(
        !p.seen.events().iter().any(|e| e.session_id == "pane-5" && e.status.as_deref() == Some("stopped")),
        "a polled run outlives the heartbeat"
    );
    p.fixture.host.stop().await;
    until(|| p.seen.events().iter().any(|e| e.session_id == "pane-5" && e.status.as_deref() == Some("error")), 20_000, "the error").await;
    assert_eq!(p.client.connections()[0].status, RemoteConnectionStatus::Disconnected, "a host that stopped sharing ends the connection");
    p.close().await;
}

/// "remote loopback: a real command runs in the shared folder and stops" (the Windows process host and PowerShell).
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_real_command_runs_in_the_shared_folder_and_stops() {
    use mighty_run::manager::{RunManager, RunManagerOptions};
    use mighty_run::providers::{Finder, ProviderCatalog};
    set_language("ko");
    let plugin = temp("plugin");
    let plugin_text = plugin.to_string_lossy().into_owned();
    let factory: RunnerFactory = Arc::new(move |_host: &RemoteHost, sinks: RunnerSinks| {
        let finder: Finder = Arc::new(|_| Box::pin(async { None }));
        let environment = Arc::new(mighty_platform::fake::FakeShellEnvironment::new(std::env::vars().collect()));
        let catalog =
            Arc::new(ProviderCatalog::new(Arc::new(mighty_platform_windows::WindowsProcessHost::new()), environment, Some(finder)));
        let mut options = RunManagerOptions::new(&plugin_text, sinks.workspace.clone(), sinks.event.clone());
        options.permission_requested = Some(sinks.permission.clone());
        options.graph_nodes = Some(sinks.graph_node.clone());
        options.powershell_commands = Some(format!("{}\\System32", std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into())));
        Arc::new(RunManager::new(catalog, options))
    });
    let p = pair(MakeHost { runner: Some(factory), ..MakeHost::default() }).await;
    p.fixture.host.set_allows_full_access(true);
    p.client.start(StartRunRequest::new("pane-6", p.workspace.id(), "shell", "(Get-Location).Path"), &p.workspace).await.unwrap();
    until(
        || p.seen.events().iter().any(|e| e.session_id == "pane-6" && matches!(e.status.as_deref(), Some("completed" | "error"))),
        20_000,
        "ends",
    )
    .await;
    // Compared as the file system names them: the folder is made under %TEMP%, which can be an 8.3 name (CI's C:\Users\RUNNER~1),
    // while PowerShell prints the long one.
    let shared = std::fs::canonicalize(p.fixture.shared.path()).unwrap();
    let printed = |text: &str| std::fs::canonicalize(text.trim()).is_ok_and(|path| path == shared);
    assert!(
        p.seen.events().iter().any(|e| e.session_id == "pane-6" && e.entry.as_ref().and_then(|l| l.text.as_deref()).is_some_and(printed)),
        "the command ran in the shared folder: {:?}",
        p.seen.words(Some("pane-6"))
    );
    p.client.start(StartRunRequest::new("pane-7", p.workspace.id(), "shell", "Start-Sleep 30"), &p.workspace).await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    p.client.stop("pane-7").await;
    assert_eq!(p.seen.events().iter().rev().find(|e| e.session_id == "pane-7").and_then(|e| e.status.clone()).as_deref(), Some("stopped"));
    until(|| p.fixture.host.state().active_runs == 0, 15_000, "no run is active").await;
    p.close().await;
    let _ = std::fs::remove_dir_all(&plugin);
}

/// A runner whose disposal waits until the check lets it go.
struct SlowDisposal {
    inner: Arc<FakeRunner>,
    entered: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Semaphore>,
}

impl HostRunner for SlowDisposal {
    fn start(&self, request: StartRunRequest) -> BoxFuture<'_, mighty_run::Result<()>> {
        self.inner.start(request)
    }
    fn stop(&self, session_id: String) -> BoxFuture<'_, mighty_run::Result<()>> {
        self.inner.stop(session_id)
    }
    fn respond_to_tool_permission(&self, session_id: &str, request_id: &str, allow: bool) -> mighty_run::Result<()> {
        self.inner.respond_to_tool_permission(session_id, request_id, allow)
    }
    fn answer_questionnaire(
        &self,
        session_id: &str,
        request_id: &str,
        answers: &IndexMap<String, UserQuestionAnswer>,
    ) -> mighty_run::Result<()> {
        self.inner.answer_questionnaire(session_id, request_id, answers)
    }
    fn answer_plan(&self, session_id: &str, request_id: &str, decision: &PlanDecision) -> mighty_run::Result<PlanRecord> {
        self.inner.answer_plan(session_id, request_id, decision)
    }
    fn dispose(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            self.entered.notify_one();
            let _ = self.release.acquire().await;
            self.inner.dispose().await;
        })
    }
}

/// "remote loopback: an old stop's run count never lands after a newer one".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_old_stops_run_count_never_lands_after_a_newer_one() {
    let told: Arc<Mutex<Vec<usize>>> = Arc::default();
    let record = told.clone();
    let made = Arc::new(AtomicUsize::new(0));
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let newest: Arc<Mutex<Option<Arc<FakeRunner>>>> = Arc::default();
    let (count, inside, gate, keep) = (made.clone(), entered.clone(), release.clone(), newest.clone());
    let factory: RunnerFactory = Arc::new(move |_host: &RemoteHost, sinks: RunnerSinks| {
        let runner = FakeRunner::new(sinks);
        if count.fetch_add(1, Ordering::SeqCst) == 0 {
            return Arc::new(SlowDisposal { inner: runner, entered: inside.clone(), release: gate.clone() });
        }
        *keep.lock().unwrap() = Some(runner.clone());
        runner
    });
    let runs_changed: Arc<dyn Fn(usize) + Send + Sync> = Arc::new(move |count| record.lock().unwrap().push(count));
    let fixture = make_host(MakeHost { runner: Some(factory), runs_changed: Some(runs_changed), ..MakeHost::default() }).await;
    fixture.host.start(&[fixture.shared.id().into()], 0).await.unwrap();
    // The first sharing's stop waits in its runner's disposal while a new sharing starts and takes a run.
    let host = fixture.host.clone();
    let stopping = tokio::spawn(async move { host.stop().await });
    tokio::time::timeout(Duration::from_secs(10), entered.notified()).await.unwrap();
    let state = fixture.host.start(&[fixture.shared.id().into()], 0).await.unwrap();
    start_raw(&loopback(state.port.unwrap()), state.key.as_deref().unwrap(), fixture.shared.id(), "claude", "hold").await;
    until(|| told.lock().unwrap().last() == Some(&1), 10_000, "the newer count is told").await;
    release.add_permits(1);
    tokio::time::timeout(Duration::from_secs(10), stopping).await.unwrap().unwrap();
    let counts = told.lock().unwrap().clone();
    assert!(
        counts.last() == Some(&1) && fixture.host.state().active_runs == 1,
        "the old stop never tells its count after the newer one: {counts:?}"
    );
    fixture.dispose().await;
}

/// "remote loopback: answers reach only their own job's requests".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn answers_reach_only_their_own_jobs_requests() {
    set_language("ko");
    let fixture = make_host(MakeHost::default()).await;
    let state = fixture.host.start(&[fixture.shared.id().into()], 0).await.unwrap();
    let (target, key, port) = (loopback(state.port.unwrap()), state.key.clone().unwrap(), state.port.unwrap());
    let asking = start_raw(&target, &key, fixture.shared.id(), "claude", "ask").await;
    let holding = start_raw(&target, &key, fixture.shared.id(), "claude", "hold").await;
    until(|| started(&fixture).len() == 2, 10_000, "both started").await;
    for tries in 0.. {
        if poll_raw(&target, &key, &asking, 0).await.events.iter().any(|e| e.event["type"] == "permission") {
            break;
        }
        assert!(tries < 200, "the tool request reached the host");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let conflict = |result: Result<Vec<u8>, TransportError>| result.unwrap_err().status() == Some(409);
    let permission = |run: &str, allow| RemoteWirePermission { run_id: run.into(), request_id: "perm-1".into(), allow }.to_value();
    assert!(
        conflict(post(&target, &key, &format!("/v2/runs/{holding}/permission"), &permission(&holding, true)).await),
        "another job's request"
    );
    assert!(
        conflict(post(&target, &key, &format!("/v2/runs/{asking}/permission"), &permission(&holding, true)).await),
        "naming another run"
    );
    let questions = RemoteWireQuestions { run_id: holding.clone(), request_id: "perm-1".into(), answers: IndexMap::new() }.to_value();
    assert!(conflict(post(&target, &key, &format!("/v2/runs/{holding}/questions"), &questions).await), "questions only for this job's");
    let plan = RemoteWirePlan::from(&holding, "perm-1", &PlanDecision::approve_auto_edit()).to_value();
    assert!(conflict(post(&target, &key, &format!("/v2/runs/{holding}/plan"), &plan).await), "a plan only for this job's");
    let head = |job: &str, action: &str, extra: &str| {
        format!("POST /v2/runs/{job}/{action} HTTP/1.1\r\nAuthorization: Bearer {key}\r\nx-mighty-remote-version: 2\r\n{extra}\r\n")
    };
    assert!(
        raw_text(port, &head(&asking, "permission", "Content-Type: application/json\r\nContent-Length: 0\r\n"))
            .await
            .starts_with("HTTP/1.1 400")
    );
    let text_plain = head(&asking, "permission", "Content-Type: text/plain\r\nContent-Length: 2\r\n");
    assert!(raw(port, text_plain.as_bytes(), Some(b"{}"), false).await.starts_with("HTTP/1.1 415"), "an answer must be JSON");
    let runner = fixture.runner().unwrap();
    assert!(runner.answers.lock().unwrap().is_empty(), "no refused answer reached the runner");
    post(&target, &key, &format!("/v2/runs/{asking}/permission"), &permission(&asking, false)).await.unwrap();
    assert_eq!(*runner.answers.lock().unwrap(), [(asking.clone(), "perm-1".to_string(), false)], "the job's own request is answered");
    let steered = RemoteWireAnswer::decode(
        &post(&target, &key, &format!("/v2/runs/{holding}/steer"), &RemoteWireSteer { text: "more".into() }.to_value()).await.unwrap(),
    )
    .unwrap();
    assert!(!steered.accepted, "a steer to a running turn is refused");
    let blank =
        post(&target, &key, &format!("/v2/runs/{holding}/steer"), &RemoteWireSteer { text: " ".into() }.to_value()).await.unwrap_err();
    assert_eq!(blank.status(), Some(400), "a steer is still checked first");
    assert!(
        raw_text(port, &head(&holding, "stop", "")).await.starts_with("HTTP/1.1 200") && runner.stopped.lock().unwrap().contains(&holding)
    );
    assert!(!runner.stopped.lock().unwrap().contains(&asking), "a stop reaches its own job alone");
    let stopped = poll_raw(&target, &key, &holding, 0).await;
    assert!(stopped.done && stopped.events.last().unwrap().event["status"] == "stopped", "and the run ends stopped");
    fixture.dispose().await;
}

/// A runner that takes each run only when the check lets it (its start waits), and keeps it until it is stopped.
#[derive(Default)]
struct GatedRunner {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
    released: AtomicBool,
    live: Mutex<Vec<String>>,
    taken: AtomicBool,
}

impl HostRunner for GatedRunner {
    fn start(&self, request: StartRunRequest) -> BoxFuture<'_, mighty_run::Result<()>> {
        Box::pin(async move {
            self.entered.notify_one();
            while !self.released.load(Ordering::SeqCst) {
                let _ = tokio::time::timeout(Duration::from_millis(50), self.release.notified()).await;
            }
            self.live.lock().unwrap().push(request.session_id.clone());
            self.taken.store(true, Ordering::SeqCst);
            Ok(())
        })
    }
    fn stop(&self, session_id: String) -> BoxFuture<'_, mighty_run::Result<()>> {
        self.live.lock().unwrap().retain(|s| *s != session_id);
        Box::pin(async { Ok(()) })
    }
    fn respond_to_tool_permission(&self, _: &str, _: &str, _: bool) -> mighty_run::Result<()> {
        Ok(())
    }
    fn answer_questionnaire(&self, _: &str, _: &str, _: &IndexMap<String, UserQuestionAnswer>) -> mighty_run::Result<()> {
        Ok(())
    }
    fn answer_plan(&self, _: &str, _: &str, _: &PlanDecision) -> mighty_run::Result<PlanRecord> {
        Err(mighty_run::RunError::State("no plan".into()))
    }
    fn dispose(&self) -> BoxFuture<'_, ()> {
        self.live.lock().unwrap().clear();
        Box::pin(async {})
    }
}

/// "remote loopback: a stop before the runner takes the run still ends it".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stop_before_the_runner_takes_the_run_still_ends_it() {
    let gated = Arc::new(GatedRunner::default());
    let runner = gated.clone();
    let factory: RunnerFactory = Arc::new(move |_host: &RemoteHost, _sinks| runner.clone());
    let fixture = make_host(MakeHost { runner: Some(factory), ..MakeHost::default() }).await;
    let state = fixture.host.start(&[fixture.shared.id().into()], 0).await.unwrap();
    let (target, key) = (loopback(state.port.unwrap()), state.key.clone().unwrap());
    let job = start_raw(&target, &key, fixture.shared.id(), "claude", "hold").await;
    tokio::time::timeout(Duration::from_secs(10), gated.entered.notified()).await.unwrap();
    // The stop lands while the runner is still taking the run: it has nothing to stop yet.
    post(&target, &key, &format!("/v2/runs/{job}/stop"), &serde_json::json!({})).await.unwrap();
    let poll = poll_raw(&target, &key, &job, 0).await;
    assert!(
        poll.done && poll.events.last().unwrap().event["status"] == "stopped" && fixture.host.state().active_runs == 0,
        "it ends stopped at once"
    );
    gated.released.store(true, Ordering::SeqCst);
    gated.release.notify_waiters();
    until(|| gated.taken.load(Ordering::SeqCst), 10_000, "the runner takes it").await;
    until(|| gated.live.lock().unwrap().is_empty(), 3000, "and the late stop ends it there").await;
    fixture.dispose().await;
}

/// "remote loopback: stopping sharing cancels a launch not begun".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stopping_sharing_cancels_a_launch_not_begun() {
    let fixture = make_host(MakeHost::default()).await;
    let entered: Arc<Mutex<Option<Cancel>>> = Arc::default();
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let (seen, gate) = (entered.clone(), release.clone());
    fixture.host.set_before_launch(Some(Arc::new(move |cancel: Cancel| {
        *seen.lock().unwrap() = Some(cancel);
        let gate = gate.clone();
        Box::pin(async move {
            let _ = gate.acquire().await;
        })
    })));
    let state = fixture.host.start(&[fixture.shared.id().into()], 0).await.unwrap();
    start_raw(&loopback(state.port.unwrap()), state.key.as_deref().unwrap(), fixture.shared.id(), "claude", "hold").await;
    until(|| entered.lock().unwrap().is_some(), 10_000, "the launch begins").await;
    let launch = entered.lock().unwrap().clone().unwrap();
    let runner = fixture.runner().unwrap();
    assert!(!launch.is_cancelled(), "a launch on its way is not cancelled while sharing goes on");
    fixture.host.stop().await;
    assert!(launch.is_cancelled(), "stopping sharing cancels the launch");
    release.add_permits(1);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(runner.started.lock().unwrap().is_empty() && runner.disposed.load(Ordering::SeqCst), "the run never reaches the runner");
    fixture.dispose().await;
}

/// "remote loopback: the buffer keeps the newest events within its bounds and says the gap".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_buffer_keeps_the_newest_events_within_its_bounds_and_says_the_gap() {
    let fixture = make_host(MakeHost::default()).await;
    let state = fixture.host.start(&[fixture.shared.id().into()], 0).await.unwrap();
    let (target, key, port) = (loopback(state.port.unwrap()), state.key.clone().unwrap(), state.port.unwrap());
    let keys = |o: &serde_json::Map<String, Value>| {
        let mut keys: Vec<&String> = o.keys().collect();
        keys.sort();
        keys.into_iter().cloned().collect::<Vec<_>>().join(",")
    };
    let sample = fixture_json("poll.json");
    for (input, last) in [("flood", 5003i64), ("bulk", 203)] {
        let job = start_raw(&target, &key, fixture.shared.id(), "claude", input).await;
        until(|| fixture.host.state().active_runs == 0, 20_000, "the run ends").await;
        let raw_body =
            transport::request(&target, &key, Method::Get, &format!("/v2/runs/{job}/events?cursor=0"), Request::default()).await.unwrap();
        let raw_value: Value = serde_json::from_slice(&raw_body).unwrap();
        assert_eq!(keys(raw_value.as_object().unwrap()), keys(sample.as_object().unwrap()), "the poll's keys are the Mac's");
        let event_keys = keys(sample["events"][0].as_object().unwrap());
        assert!(raw_value["events"].as_array().unwrap().iter().all(|e| keys(e.as_object().unwrap()) == event_keys), "each event's keys");
        let poll = poll_raw(&target, &key, &job, 0).await;
        assert_eq!(poll.last_cursor, last, "{input}: every event took a cursor");
        assert!(poll.gap && !poll.events.is_empty() && poll.events.len() <= protocol::MAXIMUM_POLL_EVENTS && poll.events[0].cursor > 2);
        assert_eq!(poll.cursor, poll.events.last().unwrap().cursor, "{input}: the oldest were dropped and the poll says so");
        assert!(
            poll.events
                .iter()
                .enumerate()
                .all(|(i, e)| e.cursor == poll.events[0].cursor + i as i64 && e.event["sessionId"] == job.as_str())
        );
        if input == "flood" {
            assert!(
                poll.events[0].cursor == last - protocol::MAXIMUM_BUFFERED_EVENTS as i64 + 1
                    && poll.events.len() == protocol::MAXIMUM_POLL_EVENTS
            );
        } else {
            let bytes: usize = poll
                .events
                .iter()
                .map(|e| to_bytes(&RemoteWirePoll { events: vec![e.clone()], ..poll.clone() }.to_value()["events"][0]).len())
                .sum();
            assert!(bytes <= protocol::MAXIMUM_POLL_BYTES, "a poll stays within its bytes");
        }
        let (mut read, mut cursor, mut done) = (poll.events.len(), poll.cursor, poll.done);
        while !done {
            let next = poll_raw(&target, &key, &job, cursor).await;
            assert!(!next.gap && !next.events.is_empty() && next.events[0].cursor == cursor + 1, "{input}: nothing more is missing");
            read += next.events.len();
            cursor = next.cursor;
            done = next.done;
        }
        assert!(cursor == last && read <= protocol::MAXIMUM_BUFFERED_EVENTS, "{input}: read to the end within the bound");
        let empty = poll_raw(&target, &key, &job, last).await;
        assert!(
            empty.events.is_empty() && !empty.gap && empty.done && empty.cursor == last,
            "an empty poll answers with the request's cursor"
        );
        for query in [
            format!("?cursor={}", last + 1),
            "?cursor=1&x=2".into(),
            "?cursor=-1".into(),
            "?Cursor=1".into(),
            "?cursor=%31".into(),
            String::new(),
        ] {
            let answer = raw_text(
                port,
                &format!("GET /v2/runs/{job}/events{query} HTTP/1.1\r\nAuthorization: Bearer {key}\r\nx-mighty-remote-version: 2\r\n\r\n"),
            )
            .await;
            assert!(answer.starts_with("HTTP/1.1 400"), "a bad cursor is refused: {query}");
        }
    }
    fixture.dispose().await;
}

fn fixture_json(name: &str) -> Value {
    fixture(name)
}
