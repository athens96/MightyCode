//! C# RemoteDesktopVerification (Verification.cs L913-L921): the run service with the client as its remote runner, against a sharing
//! host on loopback port 0 (injected in code, as only checks may), and the rules the window keeps. C#'s `DesktopService` is spread here
//! over mighty-run's `RunService` (the routing), the client (`OpenRemoteWorkspaceAsync` is `RemoteClient::open_workspace` plus the
//! state's update) and src-tauri (its admission, its phone router, its delegation); each check below names the halves that live
//! elsewhere. L922-L924 scan the WinUI sources: their Rust forms are src-tauri remote.rs's tests under the same names (W8 plan D17).

mod support;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mighty_core::model::{AppSnapshot, RemoteWorkspaceReference, RunSession, RunSettings, Workspace};
use mighty_core::shell::{ChildState, DelegationChildRow, delegation_tree};
use mighty_platform::FirewallRule;
use mighty_platform::fake::MemoryVault;
use mighty_remote::remote::client::{ClientOptions, RemoteClient};
use mighty_remote::remote::features;
use mighty_remote::remote::host::HostRuntime;
use mighty_remote::remote::wire::*;
use mighty_run::model::{RunEvent, StartRunRequest};
use mighty_run::plan_mode::PlanDecision;
use mighty_run::providers::{self, Finder, ProviderCatalog};
use mighty_run::service::{RunService, RunServiceOptions, ServiceEvent};
use mighty_run::strings::{get, get_with, set_language};
use support::fake_host::*;

fn absent() -> Arc<ProviderCatalog> {
    let finder: Finder = Arc::new(|_provider: String| Box::pin(async { None }));
    let environment = Arc::new(mighty_platform::fake::FakeShellEnvironment::new(std::env::vars().collect()));
    Arc::new(ProviderCatalog::new(Arc::new(mighty_platform::unsupported::Unsupported), environment, Some(finder)))
}

fn pane(id: &str, workspace: &str, kind: &str, provider: &str) -> Arc<RunSession> {
    Arc::new(RunSession {
        id: Some(id.into()),
        workspace_id: Some(workspace.into()),
        kind: Some(kind.into()),
        provider: Some(provider.into()),
        ..RunSession::default()
    })
}

/// C#'s `DesktopService` with `AttachRemote`: the run service over a store with one local workspace, the client attached as its
/// remote runner, connected to the fixture's host, its run events and tool requests recorded.
struct Desk {
    fixture: HostFixture,
    client: RemoteClient,
    service: Arc<RunService>,
    events: Arc<Mutex<Vec<RunEvent>>>,
    permissions: Arc<Mutex<Vec<mighty_core::permission::ToolPermissionRequest>>>,
    info: RemoteConnectionInfo,
    near: Workspace,
    directory: PathBuf,
}

impl Desk {
    /// `both`: the host shares its private workspace too.
    async fn new(make: MakeHost, both: bool, options: ClientOptions, catalog: Arc<ProviderCatalog>) -> Desk {
        let fixture = make_host(make).await;
        let mut ids = vec![fixture.shared.id().to_string()];
        if both {
            ids.push(fixture.private.id().to_string());
        }
        let state = fixture.host.start(&ids, 0).await.unwrap();
        let (store, directory) = client_store();
        std::fs::create_dir_all(directory.join("near")).unwrap();
        let near = store.lock().unwrap().approve_local(&directory.join("near")).unwrap();
        let client = RemoteClient::new(approver(&store), directory.clone(), Arc::new(MemoryVault::default()), options);
        let snapshot = AppSnapshot { workspaces: Some(vec![near.clone()]), ..AppSnapshot::default() };
        let (service, mut received) = RunService::new(
            snapshot,
            catalog,
            RunServiceOptions {
                plugin_directory: directory.to_string_lossy().into_owned(),
                permission_card: false,
                remote: Some(Arc::new(client.clone())),
                configure: None,
                admitting: None,
                binding: None,
                delegation: None,
            },
        );
        let (events, permissions) = (Arc::new(Mutex::new(vec![])), Arc::new(Mutex::new(vec![])));
        let (into, asked) = (events.clone(), permissions.clone());
        tokio::spawn(async move {
            while let Some(event) = received.recv().await {
                match event {
                    ServiceEvent::Run { event, .. } => into.lock().unwrap().push(*event),
                    ServiceEvent::ToolPermission(request) => asked.lock().unwrap().push(*request),
                    _ => {}
                }
            }
        });
        let key = state.key.clone().unwrap();
        let info = client.connect("", &format!("127.0.0.1:{}", state.port.unwrap()), &key).await.unwrap();
        Desk { fixture, client, service: Arc::new(service), events, permissions, info, near, directory }
    }

    /// `OpenRemoteWorkspaceAsync`: the client's approved entry listed (replacing the one it had) and made active.
    fn open(&self, host_workspace: &str) -> Workspace {
        let workspace = self.client.open_workspace(&self.info.id, host_workspace).unwrap();
        let listed = workspace.clone();
        self.service
            .update(move |s| {
                let mut list = s.workspaces().to_vec();
                match list.iter_mut().find(|w| w.id() == listed.id()) {
                    Some(old) => *old = listed.clone(),
                    None => list.push(listed.clone()),
                }
                AppSnapshot { workspaces: Some(list), active_workspace_id: Some(listed.id().into()), ..s.clone() }
            })
            .unwrap();
        workspace
    }

    fn panes(&self, panes: Vec<Arc<RunSession>>) {
        self.service.update(move |s| AppSnapshot { sessions: Some(panes.clone()), ..s.clone() }).unwrap();
    }

    fn status(&self, id: &str) -> String {
        self.service.snapshot().session(id).map(|p| p.status().to_string()).unwrap_or_default()
    }

    fn stops(&self, id: &str) -> usize {
        self.events.lock().unwrap().iter().filter(|e| e.session_id == id && e.status.as_deref() == Some("stopped")).count()
    }

    fn logs(&self, id: &str) -> Vec<(String, String)> {
        let snapshot = self.service.snapshot();
        let Some(pane) = snapshot.session(id) else { return vec![] };
        pane.logs().iter().map(|l| (l.kind.clone().unwrap_or_default(), l.text.clone().unwrap_or_default())).collect()
    }

    async fn close(self) {
        self.service.dispose().await;
        self.fixture.dispose().await;
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

/// "remote desktop: a remote pane runs through the client and the phone never sees it" — the run service's half: the host's workspace
/// listed as remote with the host's path (once, however often it is opened), panes remote by their workspace, a pi pane refused there,
/// the host's output, permissions, stops and plan answers the pane's, and a remote run never holding this PC's CLI updates. The phone's
/// half (the router leaves remote workspaces and panes out) is W8-B's `phone` router over `Workspace::remote`; the delegation half (no
/// child worktree, no child, no pane state) is mighty-delegation's, which reads `RunSession` workspaces the same way (src-tauri
/// delegation.rs `window_inputs`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_remote_pane_runs_through_the_client_and_the_phone_never_sees_it() {
    set_language("en");
    let desk = Desk::new(MakeHost::default(), false, loopback_options(), absent()).await;
    desk.fixture.host.set_allows_full_access(true);
    let far = desk.open(desk.fixture.shared.id());
    assert!(
        far.remote.is_some() && desk.near.remote.is_none() && far.path() == desk.fixture.shared.path(),
        "listed as remote with the host's path"
    );
    let again = desk.open(desk.fixture.shared.id());
    assert_eq!(again.id(), far.id(), "opening it again keeps one entry");
    assert_eq!(desk.service.snapshot().workspaces().iter().filter(|w| w.remote.is_some()).count(), 1);
    desk.panes(vec![
        pane("cmd", far.id(), "shell", "claude"),
        pane("agent", far.id(), "claude", "claude"),
        pane("mine", desk.near.id(), "claude", "claude"),
        pane("pi", far.id(), "claude", "pi"),
    ]);
    // pi's RPC run is local for now: a pi pane in a remote workspace is refused before anything is sent.
    let mut pi = StartRunRequest::new("pi", far.id(), "claude", "hi");
    pi.provider = "pi".into();
    let refused = desk.service.start(pi).await.unwrap_err();
    assert_eq!(refused.message(), get("run.blocked.piRemote"));

    desk.service.start(StartRunRequest::new("cmd", far.id(), "shell", "dir")).await.unwrap();
    until(|| desk.status("cmd") == "completed", 10_000, "the command completes").await;
    assert!(desk.logs("cmd").iter().any(|(_, t)| t == "echo:dir"), "the host's output is the pane's: {:?}", desk.logs("cmd"));

    desk.service.start(StartRunRequest::new("agent", far.id(), "claude", "ask")).await.unwrap();
    until(|| desk.permissions.lock().unwrap().iter().any(|p| p.run_id == "agent" && p.state == "pending"), 10_000, "the request").await;
    desk.service.respond_to_tool_permission("agent", "perm-1", true).await.unwrap();
    let runner = desk.fixture.runner().unwrap();
    until(|| runner.answers.lock().unwrap().iter().any(|a| a.1 == "perm-1" && a.2), 10_000, "the answer reaches the host").await;
    until(|| desk.status("agent") == "completed", 10_000, "the agent completes").await;

    desk.service.start(StartRunRequest::new("agent", far.id(), "claude", "hold")).await.unwrap();
    until(|| desk.service.is_session_running("agent"), 10_000, "running").await;
    assert!(!desk.service.has_active_provider("claude"), "a remote run never holds this PC's CLI updates");
    assert_eq!(
        desk.service.answer_plan("agent", "plan-x", &PlanDecision::cancel()).await.unwrap(),
        None,
        "a remote pane's plan answer goes to the host, its record comes back as an event"
    );
    desk.service.stop("agent").await.unwrap();
    until(|| desk.status("agent") == "stopped", 10_000, "stopped").await;
    assert_eq!(runner.stopped.lock().unwrap().len(), 1, "the stop reached the host");
    until(|| !desk.service.is_session_running("agent"), 10_000, "not running").await;
    desk.close().await;
}

/// "remote desktop: the sharing host runs only in shared folders with no pane tools" — the runner the app's sharing host makes
/// (`host_runner`, C# `CreateRemoteRunner`) finds workspaces only through the host's shared check: a run named for another registered
/// folder fails before it starts, one in the shared folder runs, and once sharing stops it finds nothing; the host tells how many client
/// runs go on. Its options' reading of this PC's CLIs is `HostRuntime` (L921). The runner's lack of pane tools and delegation is
/// `host_runner`'s construction (no binding, no delegation hooks); its PowerShell is remote_host's `commands_run_in_powershell_on_windows`.
/// The commands need the Windows process host.
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_sharing_host_runs_only_in_shared_folders_with_no_pane_tools() {
    use mighty_remote::remote::host::{HostRunner, RemoteHost, RunnerSinks, host_runner};
    set_language("en");
    let plugin = temp("plugin");
    std::fs::create_dir_all(plugin.join(".claude-plugin")).unwrap();
    std::fs::write(plugin.join(".claude-plugin").join("plugin.json"), "{}").unwrap();
    let plugin_text = plugin.to_string_lossy().into_owned();
    let events: Arc<Mutex<Vec<RunEvent>>> = Arc::default();
    type Made = Option<(Arc<dyn HostRunner>, RunnerSinks)>;
    let made: Arc<Mutex<Made>> = Arc::default();
    let counts: Arc<Mutex<Vec<usize>>> = Arc::default();
    let (seen, keep, counted) = (events.clone(), made.clone(), counts.clone());
    let factory: RunnerFactory = Arc::new(move |_host: &RemoteHost, sinks: RunnerSinks| {
        let finder: Finder = Arc::new(|_| Box::pin(async { None }));
        let environment = Arc::new(mighty_platform::fake::FakeShellEnvironment::new(std::env::vars().collect()));
        let catalog =
            Arc::new(ProviderCatalog::new(Arc::new(mighty_platform_windows::WindowsProcessHost::new()), environment, Some(finder)));
        let (into, event) = (seen.clone(), sinks.event.clone());
        let recorded = RunnerSinks {
            event: Arc::new(move |value: RunEvent| {
                into.lock().unwrap().push(value.clone());
                event(value);
            }),
            ..sinks.clone()
        };
        let folder = format!("{}\\System32", std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into()));
        let runner = host_runner(catalog, &plugin_text, recorded, Some(folder));
        *keep.lock().unwrap() = Some((runner.clone(), sinks));
        runner
    });
    let fixture = make_host(MakeHost {
        runner: Some(factory),
        runs_changed: Some(Arc::new(move |count| counted.lock().unwrap().push(count))),
        ..MakeHost::default()
    })
    .await;
    fixture.host.start(&[fixture.shared.id().to_string()], 0).await.unwrap();
    let (runner, sinks) = made.lock().unwrap().clone().expect("the host made its runner");
    assert_eq!(
        (sinks.workspace)(fixture.shared.id().to_string()).await.unwrap().path(),
        fixture.shared.path(),
        "the shared workspace at its path"
    );
    assert!((sinks.workspace)(fixture.private.id().to_string()).await.is_err(), "never a workspace that is not shared");
    runner.start(StartRunRequest::new("job-hidden", fixture.private.id(), "shell", "echo leaked")).await.ok();
    runner.start(StartRunRequest::new("job-shared", fixture.shared.id(), "shell", "echo shared-ok")).await.ok();
    let ended =
        |id: &str| events.lock().unwrap().iter().any(|e| e.session_id == id && matches!(e.status.as_deref(), Some("completed" | "error")));
    until(|| ended("job-hidden") && ended("job-shared"), 20_000, "both runs end").await;
    let all = events.lock().unwrap().clone();
    let text = |e: &RunEvent| e.entry.as_ref().and_then(|l| l.text.clone()).unwrap_or_default();
    assert!(all.iter().any(|e| e.session_id == "job-hidden" && e.status.as_deref() == Some("error")), "an unshared folder's run fails");
    assert!(!all.iter().any(|e| e.session_id == "job-hidden" && text(e).contains("leaked")), "before it starts");
    assert!(all.iter().any(|e| e.session_id == "job-shared" && text(e).contains("shared-ok")), "a run in the shared folder runs");
    fixture.host.stop().await;
    let after = (sinks.workspace)(fixture.shared.id().to_string()).await.unwrap_err();
    assert_eq!(after.message(), get("remote.host.error.stopped"), "once sharing stops its runner finds nothing");
    assert_eq!(counts.lock().unwrap().last().copied().unwrap_or(0), 0, "the host tells the app how many client runs go on");
    runner.dispose().await;
    fixture.dispose().await;
    let _ = std::fs::remove_dir_all(&plugin);
}

/// The sharing host's look-up (`RunService::resolve_shared`, C# `ResolveSharedAsync`) by behaviour, for L922's Rust form: a remote
/// workspace is refused even when its host's path is a folder here, an unregistered one and a gone folder are refused, a local folder
/// that is there is found.
#[test]
fn the_shared_check_never_reads_a_remote_workspaces_folder() {
    set_language("en");
    let here = temp("shared-check");
    let local = Workspace { id: Some("near".into()), path: Some(here.to_string_lossy().into_owned()), ..Workspace::default() };
    let gone = Workspace { id: Some("gone".into()), path: Some(here.join("gone").to_string_lossy().into_owned()), ..Workspace::default() };
    let far = Workspace {
        id: Some("far".into()),
        path: Some(here.to_string_lossy().into_owned()),
        remote: Some(RemoteWorkspaceReference {
            connection_id: Some("conn-1".into()),
            workspace_id: Some("ws-1".into()),
            host_name: Some("studio-pc".into()),
            version: 2,
        }),
        ..Workspace::default()
    };
    let snapshot = AppSnapshot { workspaces: Some(vec![local, gone, far]), ..AppSnapshot::default() };
    let options = RunServiceOptions {
        plugin_directory: String::new(),
        permission_card: false,
        remote: None,
        configure: None,
        admitting: None,
        binding: None,
        delegation: None,
    };
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    runtime.block_on(async {
        let (service, _events) = RunService::new(snapshot, absent(), options);
        assert_eq!(
            service.resolve_shared("far").unwrap_err(),
            get("run.error.approvedWorkspaceOnly"),
            "a remote workspace's path is not read"
        );
        assert_eq!(service.resolve_shared("other").unwrap_err(), get("run.error.localWorkspaceNotRegistered"));
        assert_eq!(service.resolve_shared("gone").unwrap_err(), get("store.error.workspaceFolderNotFound"));
        assert_eq!(service.resolve_shared("near").unwrap().id(), "near");
        service.dispose().await;
    });
    let _ = std::fs::remove_dir_all(&here);
}

/// "remote desktop: remote panes keep to the host's features".
#[test]
fn remote_panes_keep_to_the_hosts_features() {
    set_language("en");
    let local = Workspace { path: Some("C:\\work".into()), ..Workspace::default() };
    let remote = Workspace {
        path: Some("C:\\far".into()),
        remote: Some(RemoteWorkspaceReference {
            connection_id: Some("conn-1".into()),
            workspace_id: Some("ws-1".into()),
            host_name: Some("studio-pc".into()),
            version: 2,
        }),
        ..Workspace::default()
    };
    use mighty_core::add_pane;
    assert_eq!(add_pane::entries_for(Some(&local)), add_pane::entries(), "a local workspace keeps its menu");
    let entries = add_pane::entries_for(Some(&remote));
    let agent = |p: &str| format!("{}{p}", add_pane::AGENT_PREFIX);
    assert!(
        entries.contains(&add_pane::SHELL.to_string())
            && !entries.contains(&add_pane::BROWSER.to_string())
            && !entries.contains(&add_pane::FILES.to_string())
    );
    assert!(mighty_core::wire::PROVIDERS.iter().filter(|p| !providers::is_local_only(p)).all(|p| entries.contains(&agent(p))));
    assert!(["pi", "opencode", "hermes"].iter().all(|p| !entries.contains(&agent(p))), "never the agents that run on this PC only");
    assert_eq!(
        entries[entries.len() - 4..],
        [add_pane::SEPARATOR, add_pane::OPEN_PROJECT, add_pane::NEW_FOLDER, add_pane::FROM_GIT].map(String::from),
        "the three ways to add a (local) workspace end it"
    );
    assert!(features::allows_local_only(Some(&local)) && !features::allows_local_only(Some(&remote)));
    let shell = RunSession { kind: Some("shell".into()), ..RunSession::default() };
    assert!(
        features::uses_local_terminal(&shell, Some(&local)) && !features::uses_local_terminal(&shell, Some(&remote)),
        "never a local ConPTY"
    );
    let unknown = features::provider_runtime("codex", &remote, None);
    assert!(
        unknown.available && unknown.capabilities.permission_modes.iter().any(|m| m == "onRequest") && unknown.detail.contains("studio-pc")
    );
    assert_eq!(
        features::blocked_reason(Some(&remote), &RunSession::default(), None),
        Some(get_with("remote.workspace.connectionGone", &[("host", "studio-pc")])),
        "a removed connection blocks the pane"
    );
    let info = RemoteWireInfo::decode(&serde_json::to_vec(&fixture("info.json")).unwrap()).unwrap();
    let connected = RemoteConnectionInfo {
        id: "conn-1".into(),
        name: "studio".into(),
        address: "100.101.102.103:43137".into(),
        status: RemoteConnectionStatus::Connected,
        host_name: Some("studio-pc".into()),
        workspaces: Some(info.workspaces.clone()),
        runtime: Some(info.runtime.clone()),
        detail: None,
    };
    let claude = info.runtime.providers.iter().find(|p| p.id == "claude").unwrap();
    assert_eq!(&features::provider_runtime("claude", &remote, Some(&connected)), claude, "once the host answered, its CLI is the pane's");
    let with = |provider: &str, mode: Option<&str>, kind: &str| RunSession {
        provider: Some(provider.into()),
        kind: Some(kind.into()),
        settings: mode.map(|m| RunSettings { permission_mode: m.into(), ..RunSettings::default() }),
        ..RunSession::default()
    };
    assert_eq!(
        features::blocked_reason(Some(&remote), &with("codex", None, "claude"), Some(&connected)),
        Some(get_with("remote.host.error.cliMissing", &[("name", providers::name("codex"))]))
    );
    assert_eq!(
        features::blocked_reason(Some(&remote), &with("claude", Some("onRequest"), "claude"), Some(&connected)),
        Some(get_with("remote.workspace.modeUnsupported", &[("host", "studio-pc")]))
    );
    assert_eq!(features::blocked_reason(Some(&remote), &with("claude", None, "claude"), Some(&connected)), None);
    assert_eq!(
        features::blocked_reason(Some(&remote), &with("claude", None, "shell"), Some(&connected)),
        None,
        "commands never wait for a CLI"
    );
    assert_eq!(features::firewall_ports(Some(43139), Some(43137)), [43137, 43139]);
    assert_eq!(features::firewall_ports(None, Some(43137)), [43137]);
    assert!(features::firewall_ports(None, None).is_empty() && features::firewall_ports(Some(80), None).is_empty());
    assert_eq!(features::firewall_ports(Some(43137), Some(43137)), [43137]);
    assert!(
        !features::KEY_COPY.allowed_in_history && !features::KEY_COPY.roamable && features::KEY_COPY.clear_after == Duration::from_secs(60)
    );
    assert!(features::should_clear(Some("k"), "k") && !features::should_clear(Some("other"), "k") && !features::should_clear(None, "k"));
}

/// "remote desktop: a closed parent's child is never placed in a remote workspace" (mighty-core's `delegation_tree`, C#
/// `DelegationSidebar`): a remote workspace's folder is its host's path, never this PC's.
#[test]
fn a_closed_parents_child_is_never_placed_in_a_remote_workspace() {
    let root = std::env::temp_dir().join("mc-placement");
    let (repo, other) = (root.join("repo"), root.join("other"));
    let reference = |ws: &str| RemoteWorkspaceReference {
        connection_id: Some("conn-1".into()),
        workspace_id: Some(ws.into()),
        host_name: Some("studio-pc".into()),
        version: 2,
    };
    let workspace = |id: &str, path: &std::path::Path, remote: Option<RemoteWorkspaceReference>| Workspace {
        id: Some(id.into()),
        name: Some(id.into()),
        path: Some(path.to_string_lossy().into_owned()),
        remote,
        ..Workspace::default()
    };
    let remote = workspace("far", &repo, Some(reference("ws-1")));
    let remote_inside = workspace("far-sub", &other.join("app"), Some(reference("ws-2")));
    let local = workspace("near", &repo, None);
    let row = |checkout: &std::path::Path| DelegationChildRow {
        id: "c1".into(),
        parent_session_id: "gone".into(),
        state: ChildState::Closed,
        task: None,
        parent_branch: "main".into(),
        parent_checkout: Some(checkout.to_string_lossy().into_owned()),
        can_undo: false,
        held_notices: 0,
        started_by_human: false,
        agent: "claude".into(),
    };
    let state = |workspaces: Vec<Workspace>| AppSnapshot { workspaces: Some(workspaces), ..AppSnapshot::default() };
    let children = [row(&repo)];
    let both = state(vec![remote.clone(), local.clone()]);
    assert!(delegation_tree("far", &both, &children).parent_closed.is_empty(), "not the remote one listed first at the same path");
    let near = delegation_tree("near", &both, &children);
    assert_eq!(near.parent_closed.len(), 1, "the local workspace at the checkout");
    assert_eq!(near.parent_closed[0].2.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["c1"]);
    assert!(delegation_tree("far", &state(vec![remote]), &children).parent_closed.is_empty(), "a remote workspace alone places nothing");
    assert!(delegation_tree("far-sub", &state(vec![remote_inside]), &[row(&other)]).parent_closed.is_empty(), "nor one inside it");
}

/// "remote desktop: the firewall rows say when the rule lacks a port in use" — the decision (`FirewallNeedsUpdate`). The rule's reading
/// (`WindowsFirewallRule.ShownPorts` and the unelevated runner) is W8-C's `firewall::netsh` and the Windows `Firewall` port, checked
/// there; the rows that say it are settings/mobile_direct.rs and settings/remote.rs, both through W8-C's `FirewallRule::needs_update`.
#[test]
fn the_firewall_rows_say_when_the_rule_lacks_a_port_in_use() {
    const EXE: &str = r"C:\Program Files\Mighty Code\mighty-code.exe";
    let present = |ports: &[u16]| FirewallRule::Present { ports: Some(ports.to_vec()), program: Some(EXE.into()) };
    assert!(present(&[43139]).needs_update(&[43137, 43139], EXE), "a rule without the sharing port needs an update");
    assert!(!present(&[43137, 43139]).needs_update(&[43137, 43139], EXE));
    assert!(!present(&[43137, 43139, 43140]).needs_update(&[43137], EXE), "whatever else it opens");
    let unread = FirewallRule::Present { ports: None, program: Some(EXE.into()) };
    assert!(unread.needs_update(&[43137], EXE), "a rule whose ports cannot be read is never current");
    assert!(!unread.needs_update(&[], EXE), "nothing to open");
    assert!(!FirewallRule::Missing.needs_update(&[43137], EXE), "no rule");
    assert!(!FirewallRule::Unreadable.needs_update(&[43137], EXE), "an unread rule");
}

/// "remote desktop: no client code runs under the service's lock, and a stop while connecting ends the run once": a route check that
/// blocks leaves the service's state free and this PC's other pane runs (its command needs the Windows process host); a stop while
/// connecting ends the run stopped, once, and never reaches the host.
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_client_code_runs_under_the_services_lock() {
    set_language("en");
    let armed = Arc::new(AtomicBool::new(false));
    let (gate_in, gate_out) = (Arc::new(Mutex::new(false)), Arc::new(std::sync::Condvar::new()));
    let reached = Arc::new(AtomicBool::new(false));
    let (arm, flag, held, wake) = (armed.clone(), reached.clone(), gate_in.clone(), gate_out.clone());
    let options = ClientOptions {
        route_check: Some(Arc::new(move |_| {
            if arm.load(Ordering::SeqCst) {
                flag.store(true, Ordering::SeqCst);
                let open = held.lock().unwrap();
                let _ = wake.wait_timeout_while(open, Duration::from_secs(20), |open| !*open).unwrap();
            }
            Ok(true)
        })),
        ..loopback_options()
    };
    let finder: Finder = Arc::new(|_| Box::pin(async { None }));
    let environment = Arc::new(mighty_platform::fake::FakeShellEnvironment::new(std::env::vars().collect()));
    let catalog = Arc::new(ProviderCatalog::new(Arc::new(mighty_platform_windows::WindowsProcessHost::new()), environment, Some(finder)));
    let desk = Arc::new(Desk::new(MakeHost::default(), false, options, catalog).await);
    let far = desk.open(desk.fixture.shared.id());
    desk.panes(vec![pane("far-agent", far.id(), "claude", "claude"), pane("near-cmd", desk.near.id(), "shell", "claude")]);
    armed.store(true, Ordering::SeqCst);
    let starting = {
        let (desk, far) = (desk.clone(), far.id().to_string());
        tokio::spawn(async move { desk.service.start(StartRunRequest::new("far-agent", &far, "claude", "hold")).await })
    };
    until(|| reached.load(Ordering::SeqCst), 10_000, "the remote start reached the client's route check").await;
    let free = {
        let service = desk.service.clone();
        tokio::time::timeout(Duration::from_secs(2), tokio::task::spawn_blocking(move || service.snapshot())).await
    };
    assert!(free.is_ok(), "the service's lock is free while the client checks the route");
    tokio::time::timeout(
        Duration::from_secs(10),
        desk.service.start(StartRunRequest::new("near-cmd", desk.near.id(), "shell", "echo near-ok")),
    )
    .await
    .unwrap()
    .unwrap();
    until(|| matches!(desk.status("near-cmd").as_str(), "completed" | "error"), 20_000, "near-cmd ends").await;
    assert!(
        desk.logs("near-cmd").iter().any(|(_, t)| t.contains("near-ok")),
        "this PC's other pane ran meanwhile: {:?}",
        desk.logs("near-cmd")
    );
    tokio::time::timeout(Duration::from_secs(10), desk.service.stop("far-agent")).await.unwrap().unwrap();
    armed.store(false, Ordering::SeqCst);
    *gate_in.lock().unwrap() = true;
    gate_out.notify_all();
    let _ = tokio::time::timeout(Duration::from_secs(20), starting).await.unwrap();
    assert!(
        desk.status("far-agent") == "stopped" && !desk.service.is_session_running("far-agent"),
        "a stop while connecting ends it stopped"
    );
    assert_eq!(desk.stops("far-agent"), 1, "the pane hears one stop");
    assert!(desk.fixture.runner().is_none_or(|r| r.started.lock().unwrap().is_empty()), "it never reaches the host");
    let desk = Arc::try_unwrap(desk).ok().expect("no other holder");
    desk.close().await;
}

/// "remote desktop: a stop during the host's accept never ends the next send": a stop while the host accepts ends that run alone; with
/// attachments the start says it was cancelled (so they stay) while the newer send runs on and hears no second stop.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stop_during_the_hosts_accept_never_ends_the_next_send() {
    set_language("en");
    let holding = Arc::new(AtomicBool::new(false));
    let accepting = Arc::new(AtomicUsize::new(0));
    let release = Arc::new((Mutex::new(0usize), std::sync::Condvar::new()));
    let (hold, seen, gate) = (holding.clone(), accepting.clone(), release.clone());
    let admission: mighty_remote::remote::host::Admission = Arc::new(move |_| {
        if hold.swap(false, Ordering::SeqCst) {
            let round = seen.fetch_add(1, Ordering::SeqCst) + 1;
            let (lock, wake) = &*gate;
            let open = lock.lock().unwrap();
            let _ = wake.wait_timeout_while(open, Duration::from_secs(20), |opened| *opened < round).unwrap();
        }
        None
    });
    let desk =
        Arc::new(Desk::new(MakeHost { admission: Some(admission), ..MakeHost::default() }, false, loopback_options(), absent()).await);
    let far = desk.open(desk.fixture.shared.id());
    desk.panes(vec![pane("agent", far.id(), "claude", "claude")]);
    let open_gate = |round: usize| {
        let (lock, wake) = &*release;
        *lock.lock().unwrap() = round;
        wake.notify_all();
    };

    // Without attachments: the stop lands while the host accepts; the start ends quietly.
    holding.store(true, Ordering::SeqCst);
    let first = {
        let (desk, far) = (desk.clone(), far.id().to_string());
        tokio::spawn(async move { desk.service.start(StartRunRequest::new("agent", &far, "claude", "hold")).await })
    };
    until(|| accepting.load(Ordering::SeqCst) == 1, 10_000, "the host is accepting the run").await;
    desk.service.stop("agent").await.unwrap();
    until(|| desk.status("agent") == "stopped", 10_000, "stopped").await;
    open_gate(1);
    let _ = tokio::time::timeout(Duration::from_secs(20), first).await.unwrap();
    assert!(
        desk.status("agent") == "stopped" && !desk.service.is_session_running("agent") && desk.stops("agent") == 1,
        "once: {}",
        desk.stops("agent")
    );
    until(|| desk.fixture.host.state().active_runs == 0, 10_000, "no client run").await;

    // With attachments the start is refused as cancelled; meanwhile the pane was sent again, and that run goes on.
    holding.store(true, Ordering::SeqCst);
    let attachment = mighty_core::attachment::RunAttachment {
        id: "attachment".into(),
        name: "fixture.txt".into(),
        media_type: "text/plain".into(),
        data_base64: "Zml4dHVyZQ==".into(),
    };
    let held = {
        let (desk, far) = (desk.clone(), far.id().to_string());
        tokio::spawn(async move {
            let mut request = StartRunRequest::new("agent", &far, "claude", "hold");
            request.attachments = Some(vec![attachment]);
            desk.service.start(request).await
        })
    };
    until(|| accepting.load(Ordering::SeqCst) == 2, 10_000, "the host is accepting the run with attachments").await;
    desk.service.stop("agent").await.unwrap();
    until(|| desk.stops("agent") == 2, 10_000, "the second stop").await;
    let errors = desk.logs("agent").iter().filter(|(k, _)| k == "error").count();
    desk.service.start(StartRunRequest::new("agent", far.id(), "claude", "hold")).await.unwrap();
    until(|| desk.service.is_session_running("agent") && desk.status("agent") == "running", 10_000, "the newer send runs").await;
    open_gate(2);
    let outcome = tokio::time::timeout(Duration::from_secs(20), held).await.unwrap().unwrap();
    assert!(matches!(outcome, Err(mighty_run::RunError::Cancelled(_))), "the start with attachments says it was cancelled: {outcome:?}");
    assert!(desk.status("agent") == "running" && desk.service.is_session_running("agent"), "the newer send runs on");
    assert_eq!(desk.stops("agent"), 2, "each stop heard once, never one for the newer send");
    assert_eq!(
        desk.logs("agent").iter().filter(|(k, _)| k == "error").count(),
        errors,
        "the older start writes nothing into the newer run"
    );
    desk.service.stop("agent").await.unwrap();
    until(|| desk.status("agent") == "stopped" && desk.stops("agent") == 3, 10_000, "the third stop").await;
    let desk = Arc::try_unwrap(desk).ok().expect("no other holder");
    desk.close().await;
}

/// "remote desktop: a send to a workspace the host no longer shares is refused" (the Mac's connectForRemoteRun check).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_send_to_a_workspace_the_host_no_longer_shares_is_refused() {
    set_language("en");
    let desk = Desk::new(MakeHost::default(), true, loopback_options(), absent()).await;
    desk.fixture.host.set_allows_full_access(true);
    let info = desk.info.clone();
    let gone = desk.open(desk.fixture.private.id());
    desk.panes(vec![pane("cmd", gone.id(), "shell", "claude")]);
    // The host forgets the workspace; the client's next reading no longer lists it.
    let kept = vec![desk.fixture.shared.clone(), desk.fixture.remote.clone()];
    desk.fixture.store.lock().unwrap().save(&AppSnapshot { workspaces: Some(kept), ..AppSnapshot::default() }).unwrap();
    desk.client.refresh(&info.id).await.unwrap();
    let refused = desk.service.start(StartRunRequest::new("cmd", gone.id(), "shell", "dir")).await.unwrap_err();
    assert_eq!(refused.message(), get("remote.workspace.error.notShared"), "refused as not shared");
    assert!(
        desk.status("cmd") == "error" && desk.logs("cmd").iter().any(|(k, t)| k == "error" && t == refused.message()),
        "the pane says why"
    );
    assert!(desk.fixture.runner().is_none_or(|r| r.started.lock().unwrap().is_empty()), "nothing reached the host");
    desk.close().await;
}

/// "remote desktop: the host reads this PC's CLIs once, and again when stale, failed or forgotten" (C# `HostRuntimeAsync`,
/// `ForgetHostRuntime`, `HostRuntimeFresh`; mighty-remote's `HostRuntime`, which src-tauri remote.rs hands the host).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_host_reads_this_pcs_clis_once_and_again_when_stale_failed_or_forgotten() {
    let reads = Arc::new(AtomicUsize::new(0));
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let reading = runtime("info.json");
    let (count, wait) = (reads.clone(), gate.clone());
    let runtime = HostRuntime::new(
        Arc::new(move || {
            let (count, wait, reading) = (count.clone(), wait.clone(), reading.clone());
            Box::pin(async move {
                let n = count.fetch_add(1, Ordering::SeqCst) + 1;
                if n == 1 {
                    return Err("the CLI could not be read".to_string());
                }
                if n == 2 {
                    let _ = wait.acquire().await;
                }
                Ok(reading)
            })
        }),
        "1.2.3",
    );
    runtime.set_fresh(Duration::from_secs(300));
    assert!(runtime.read().wait().await.is_err() && reads.load(Ordering::SeqCst) == 1, "the first reading failed");
    let (slow, waiting) = (runtime.read(), runtime.read());
    until(|| reads.load(Ordering::SeqCst) == 2, 5_000, "a failed reading is read again").await;
    assert!(Arc::ptr_eq(&slow, &waiting), "callers share the reading that goes on");
    gate.add_permits(1);
    assert_eq!(slow.wait().await.unwrap().app_version, "1.2.3");
    assert!(Arc::ptr_eq(&runtime.read(), &slow) && reads.load(Ordering::SeqCst) == 2, "a finished reading is kept");
    runtime.forget();
    let fresh = runtime.read();
    fresh.wait().await.unwrap();
    assert!(!Arc::ptr_eq(&fresh, &slow) && reads.load(Ordering::SeqCst) == 3, "a forgotten reading is read again");
    runtime.set_fresh(Duration::from_secs(2));
    assert!(Arc::ptr_eq(&runtime.read(), &fresh), "a reading inside its time is kept");
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let later = runtime.read();
    later.wait().await.unwrap();
    assert!(!Arc::ptr_eq(&later, &fresh) && reads.load(Ordering::SeqCst) == 4, "a reading past its time is read again");
}
