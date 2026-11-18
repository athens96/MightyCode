//! C# RemoteWorkspaceStateVerification (Verification.cs L844-L846) and "saved remote workspaces are dropped with their panes and layout
//! entries" (L815): `Workspace.remote` version 2 as the Mac writes it, older shapes dropped, a local workspace never read or written as
//! remote, and no runner ever starting in a remote workspace. Over mighty-core's `Store`, mighty-run's `RunManager` and `RunService`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use mighty_core::locale::Locale;
use mighty_core::model::{AppSnapshot, RemoteWorkspaceReference, RunSession, Workspace};
use mighty_core::store::{Store, keeps_workspace};
use mighty_platform::BoxFuture;
use mighty_run::manager::{RunManager, RunManagerOptions};
use mighty_run::model::StartRunRequest;
use mighty_run::providers::{Finder, ProviderCatalog};
use mighty_run::service::{RunService, RunServiceOptions};
use mighty_run::strings::{get, set_language};
use serde_json::Value;

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mighty-remote-{name}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn fixture(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../contracts/fixtures/remote-v2").join(name);
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

fn json(path: &Path) -> String {
    serde_json::to_string(&path.to_string_lossy()).unwrap()
}

fn en() -> Locale {
    Locale::new("en")
}

/// A catalog that finds no CLI; nothing it is asked to start gets that far.
fn absent() -> Arc<ProviderCatalog> {
    let finder: Finder = Arc::new(|_provider: String| Box::pin(async { None }));
    let environment = Arc::new(mighty_platform::fake::FakeShellEnvironment::new(std::env::vars().collect()));
    Arc::new(ProviderCatalog::new(Arc::new(mighty_platform::unsupported::Unsupported), environment, Some(finder)))
}

/// "remote state: the shared reference cases decide what is kept".
#[test]
fn shared_reference_cases_decide_what_is_kept() {
    let file = fixture("state-references.json");
    let cases = file["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 12, "the fixture has 12 cases");
    for case in cases {
        assert_eq!(keeps_workspace(&case["workspace"]), case["kept"].as_bool().unwrap(), "case: {}", case["name"]);
    }
}

/// "remote state: version 2 loads as remote and older references are dropped".
#[test]
fn version_two_loads_as_remote_and_older_references_are_dropped() {
    let (directory, folder) = (temp("state-v2"), temp("state-v2-folder"));
    let local = json(&folder);
    let state = format!(
        "{{\"version\":1,\"workspaces\":[\
         {{\"id\":\"local\",\"name\":\"Local\",\"path\":{local}}},\
         {{\"id\":\"far\",\"name\":\"Far\",\"path\":\"C:\\\\work\\\\project\",\"remote\":{{\"version\":2,\"connectionId\":\"conn-1\",\"workspaceId\":\"ws-project\",\"hostName\":\"studio-pc\"}}}},\
         {{\"id\":\"mac\",\"name\":\"Mac\",\"path\":\"/Users/example/site\",\"remote\":{{\"version\":2,\"connectionId\":\"conn-2\",\"workspaceId\":\"ws-site\",\"hostName\":\"my-mac\"}}}},\
         {{\"id\":\"v20\",\"name\":\"V20\",\"path\":\"C:\\\\work\\\\other\",\"remote\":{{\"version\":2.0,\"connectionId\":\"conn-1\",\"workspaceId\":\"ws-other\",\"hostName\":\"studio-pc\"}}}},\
         {{\"id\":\"old\",\"name\":\"Old\",\"path\":{local},\"remote\":{{\"connectionId\":\"conn-1\",\"workspaceId\":\"w1\",\"hostName\":\"Peer\"}}}},\
         {{\"id\":\"vs\",\"name\":\"VS\",\"path\":{local},\"remote\":{{\"version\":\"2\",\"connectionId\":\"conn-1\",\"workspaceId\":\"w1\",\"hostName\":\"Peer\"}}}},\
         {{\"id\":\"vn\",\"name\":\"VN\",\"path\":{local},\"remote\":{{\"version\":null,\"connectionId\":\"conn-1\",\"workspaceId\":\"w1\",\"hostName\":\"Peer\"}}}},\
         {{\"id\":\"va\",\"name\":\"VA\",\"path\":{local},\"remote\":[]}},\
         {{\"id\":\"bad\",\"name\":\"Bad\",\"path\":\"C:\\\\work\\\\bad\",\"remote\":{{\"version\":2,\"connectionId\":7,\"workspaceId\":\"w1\",\"hostName\":\"Peer\"}}}},\
         {{\"id\":\"dup\",\"name\":\"Dup\",\"path\":\"C:\\\\work\\\\dup\",\"remote\":{{\"version\":2,\"connectionId\":\"conn-1\",\"workspaceId\":\"w1\",\"hostName\":\"Peer\"}},\"remote\":{{\"connectionId\":\"conn-1\",\"workspaceId\":\"w1\",\"hostName\":\"Peer\"}}}},\
         {{\"id\":\"cap\",\"name\":\"Cap\",\"path\":{local},\"Remote\":{{\"version\":2,\"connectionId\":\"conn-1\",\"workspaceId\":\"w1\",\"hostName\":\"Peer\"}}}},\
         {{\"id\":\"vx\",\"name\":\"VX\",\"path\":{local},\"remote\":\"studio-pc\"}}],\
         \"sessions\":[{{\"id\":\"s-local\",\"workspaceId\":\"local\"}},{{\"id\":\"s-far\",\"workspaceId\":\"far\"}},{{\"id\":\"s-far-cmd\",\"workspaceId\":\"far\",\"kind\":\"shell\"}},{{\"id\":\"s-old\",\"workspaceId\":\"old\"}},{{\"id\":\"s-bad\",\"workspaceId\":\"bad\"}},{{\"id\":\"s-vx\",\"workspaceId\":\"vx\"}}]}}"
    );
    std::fs::write(directory.join("workspace-state.json"), state).unwrap();
    let mut store = Store::new(directory.clone(), None, true);
    let loaded = store.load(&en()).unwrap();
    let ids: Vec<&str> = loaded.workspaces().iter().map(Workspace::id).collect();
    assert_eq!(ids, ["local", "far", "mac", "v20", "cap"], "kept");
    let panes: Vec<&str> = loaded.sessions().iter().map(|s| s.id()).collect();
    assert_eq!(panes, ["s-local", "s-far", "s-far-cmd"], "panes of dropped workspaces go with them");
    let far = loaded.workspace("far").unwrap().clone();
    let reference = far.remote.clone().unwrap();
    assert!(
        reference.version == 2
            && reference.connection_id.as_deref() == Some("conn-1")
            && reference.workspace_id.as_deref() == Some("ws-project")
            && reference.host_name.as_deref() == Some("studio-pc")
            && far.path() == "C:\\work\\project",
        "a Windows host path stays the host's"
    );
    assert_eq!(loaded.workspace("mac").unwrap().path(), "/Users/example/site", "a Mac host path is valid on Windows too");
    let v20 = loaded.workspace("v20").unwrap().remote.clone().unwrap();
    assert!(v20.version == 2 && v20.workspace_id.as_deref() == Some("ws-other"), "version 2.0 is the number 2");
    assert!(loaded.workspace("local").unwrap().remote.is_none(), "a local workspace is read as local");
    assert!(loaded.workspace("cap").unwrap().remote.is_none(), "only the exact \"remote\" key is read, as on the Mac");
    store.save(&loaded).unwrap();
    let saved = std::fs::read_to_string(directory.join("workspace-state.json")).unwrap();
    assert!(
        saved.contains("\"remote\":{")
            && saved.contains("\"version\":2")
            && !saved.contains("\"version\":2.0")
            && !saved.contains("\"old\"")
    );
    assert!(!saved.to_lowercase().contains("isremote"), "the computed IsRemote is never written");
    let again = Store::new(directory.clone(), None, true).load(&en()).unwrap();
    assert_eq!(again.workspace("far").unwrap().remote, far.remote, "a reload reads the same reference");
    assert!(again.workspace("local").unwrap().remote.is_none(), "a reload reads the local one as local");
    let _ = std::fs::remove_dir_all(&directory);
    let _ = std::fs::remove_dir_all(&folder);
}

/// "remote state: a local workspace is never written or read as remote". C#'s `ResolveLocalAsync` is the run service's own look-up
/// here: a pane of a remote workspace (even one whose host path is a real folder on this PC) never runs on this PC's runner.
#[tokio::test]
async fn a_local_workspace_is_never_written_or_read_as_remote() {
    set_language("ko");
    let (directory, folder) = (temp("state-local"), temp("state-local-folder"));
    let mut store = Store::new(directory.clone(), None, true);
    store.load(&en()).unwrap();
    let local = store.approve_local(&folder).unwrap();
    let remote = store.approve_remote("conn-1", "ws-project", "project", "C:\\work\\project", "studio-pc").unwrap();
    let reference = remote.remote.clone().unwrap();
    assert!(
        reference.version == 2
            && reference.connection_id.as_deref() == Some("conn-1")
            && reference.workspace_id.as_deref() == Some("ws-project")
            && reference.host_name.as_deref() == Some("studio-pc"),
        "a remote workspace carries version 2"
    );
    assert_eq!(store.approve_remote("conn-1", "ws-project", "project", "C:\\work\\project", "studio-pc").unwrap().id(), remote.id());
    assert!(store.approve_local(&folder).unwrap().id() == local.id() && local.remote.is_none(), "a local folder stays local");
    // A remote workspace whose host path is also a real local folder here: only its "remote" value keeps it off this disk.
    let twin = store.approve_remote("conn-2", "ws-project", "project", &folder.to_string_lossy(), "other").unwrap();
    assert!(twin.id() != local.id() && store.approve_local(&folder).unwrap().id() == local.id(), "never takes a local one's place");
    assert!(store.approve_remote("conn-1", "ws-project", "x", "relative\\path", "studio-pc").is_none(), "a relative host path is refused");
    assert!(store.approve_remote("bad id", "ws-project", "x", "C:\\work", "studio-pc").is_none(), "a bad connection id is refused");
    assert!(store.approve_remote("conn-1", "ws-project", "x", "C:\\work", " ").is_none(), "an empty host name is refused");
    let snapshot = AppSnapshot { workspaces: Some(vec![local.clone(), remote.clone(), twin.clone()]), ..AppSnapshot::default() };
    store.save(&snapshot).unwrap();
    let as_local = AppSnapshot {
        workspaces: Some(vec![local.clone(), remote.clone(), Workspace { remote: None, ..twin.clone() }]),
        ..AppSnapshot::default()
    };
    assert!(store.save(&as_local).is_err(), "a remote workspace cannot be saved as local");
    let forged = RemoteWorkspaceReference {
        connection_id: Some("conn-9".into()),
        workspace_id: Some("ws".into()),
        host_name: Some("x".into()),
        version: 2,
        mirror: false,
    };
    let as_remote = AppSnapshot {
        workspaces: Some(vec![Workspace { remote: Some(forged), ..local.clone() }, remote.clone(), twin.clone()]),
        ..AppSnapshot::default()
    };
    assert!(store.save(&as_remote).is_err(), "a local workspace cannot be saved as remote");
    // A runner never starts in a remote workspace.
    let resolved = twin.clone();
    let resolve: mighty_run::manager::ResolveWorkspace =
        Arc::new(move |_id: String| -> BoxFuture<'static, mighty_run::Result<Workspace>> {
            let workspace = resolved.clone();
            Box::pin(async move { Ok(workspace) })
        });
    let runs = RunManager::new(absent(), RunManagerOptions::new(&directory.to_string_lossy(), resolve, Arc::new(|_| {})));
    let refused = runs.start_reached(StartRunRequest::new("pane-1", twin.id(), "shell", "echo hi")).await.unwrap_err();
    assert_eq!(refused.message(), get("run.error.approvedWorkspaceOnly"), "a runner never starts in a remote workspace");
    runs.dispose().await;
    // Nor does the service resolve one as a local folder: a remote pane's run goes only to its host (here none is attached).
    let pane =
        RunSession { id: Some("pane-2".into()), workspace_id: Some(twin.id().into()), kind: Some("shell".into()), ..RunSession::default() };
    let state = AppSnapshot { workspaces: Some(vec![local, twin.clone()]), sessions: Some(vec![Arc::new(pane)]), ..AppSnapshot::default() };
    let options = RunServiceOptions {
        plugin_directory: directory.to_string_lossy().into_owned(),
        permission_card: false,
        remote: None,
        configure: None,
        admitting: None,
        binding: None,
        delegation: None,
    };
    let (service, _events) = RunService::new(state, absent(), options);
    let refused = service.start(StartRunRequest::new("pane-2", twin.id(), "shell", "echo hi")).await.unwrap_err();
    assert_eq!(refused.message(), get("remote.run.error.notConnected"), "a remote workspace never resolves as a local folder");
    service.dispose().await;
    let _ = std::fs::remove_dir_all(&directory);
    let _ = std::fs::remove_dir_all(&folder);
}

/// "saved remote workspaces are dropped with their panes and layout entries".
#[test]
fn saved_remote_workspaces_are_dropped_with_their_panes_and_layout_entries() {
    let (directory, native) = (temp("legacy"), temp("legacy-native"));
    // Old state: one local workspace and one with a "remote" key (an older shape, no version), panes and layout entries in both. The
    // remote path is fully qualified on every OS, so only the "remote" key can drop it.
    let local = json(&directory);
    let far = json(&directory.join("far"));
    let legacy = format!(
        "{{\"version\":1,\"workspaces\":[{{\"id\":\"local\",\"name\":\"Local\",\"path\":{local}}},{{\"id\":\"far\",\"name\":\"Far\",\"path\":{far},\"remote\":{{\"connectionId\":\"conn1\",\"workspaceId\":\"w1\",\"hostName\":\"Peer\"}}}}],\
         \"sessions\":[{{\"id\":\"s1\",\"workspaceId\":\"local\"}},{{\"id\":\"s2\",\"workspaceId\":\"local\",\"kind\":\"shell\"}},{{\"id\":\"r1\",\"workspaceId\":\"far\"}},{{\"id\":\"r2\",\"workspaceId\":\"far\",\"kind\":\"shell\"}}],\
         \"activeWorkspaceId\":\"far\",\"activeSessionId\":\"r1\",\
         \"paneLayouts\":{{\"local\":{{\"id\":\"l\",\"kind\":\"tabs\",\"sessionIds\":[\"s1\",\"s2\"]}},\"far\":{{\"id\":\"f\",\"kind\":\"tabs\",\"sessionIds\":[\"r1\",\"r2\"]}}}},\
         \"paneLayoutModes\":{{\"local\":\"tabs\",\"far\":\"custom\"}},\"paneLayoutActiveSessionIds\":{{\"local\":\"s2\",\"far\":\"r1\"}}}}"
    );
    // Written as an old profile, so the first-run import path is the one that drops it.
    std::fs::write(directory.join("workspace-state.json"), &legacy).unwrap();
    let mut store = Store::new(native.clone(), Some(directory.clone()), true);
    let state = store.load(&en()).unwrap();
    assert_eq!(state.workspaces().iter().map(Workspace::id).collect::<Vec<_>>(), ["local"], "the remote workspace is dropped");
    assert_eq!(state.sessions().iter().map(|s| s.id()).collect::<Vec<_>>(), ["s1", "s2"], "its panes are dropped");
    assert!(
        state.active_workspace_id.as_deref() == Some("local") && state.active_session_id.as_deref() == Some("s1"),
        "active ids fall back"
    );
    fn keys<V>(map: Option<&indexmap::IndexMap<String, V>>) -> Vec<String> {
        map.map(|m| m.keys().cloned().collect()).unwrap_or_default()
    }
    assert!(
        keys(state.pane_layouts.as_ref()) == ["local"]
            && keys(state.pane_layout_modes.as_ref()) == ["local"]
            && keys(state.pane_layout_active_session_ids.as_ref()) == ["local"],
        "layout entries keyed by the remote workspace are dropped"
    );
    assert!(state.workspace("far").is_none());
    store.save(&state).unwrap();
    let saved = std::fs::read_to_string(native.join("workspace-state.json")).unwrap();
    assert!(!saved.contains("\"far\"") && !saved.contains("\"remote\""), "saved state does not keep the remote workspace");
    assert_eq!(std::fs::read_to_string(directory.join("workspace-state.json")).unwrap(), legacy, "the old profile is left untouched");
    let _ = std::fs::remove_dir_all(&directory);
    let _ = std::fs::remove_dir_all(&native);
}
