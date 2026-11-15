//! A pane worktree's runs (W9-I; C# DesktopService.cs `Launch`, RunManager.cs `ExecuteAsync`): the run service tells the pane's agent
//! once where it works (the CLI gets the note, the transcript keeps the request as sent, `noteGiven` is saved, a slash command never
//! carries it), and the runner hands the process the pane's PORT, `MIGHTY_WORKSPACE_FOLDER` and `MIGHTY_WORKTREE`, never another key
//! and never a value holding a NUL. The stand-in CLI records its stdin and those variables. Windows: the real process host runs it.
#![cfg(windows)]

mod common;

use std::path::Path;
use std::sync::{Arc, Mutex};

use mighty_core::model::{AppSnapshot, PaneWorktree, RunSession, Workspace};
use mighty_run::manager::{Emit, ResolveWorkspace};
use mighty_run::model::{RunEvent, StartRunRequest};
use mighty_run::service::{RunService, RunServiceOptions};
use mighty_run::{RunManager, RunManagerOptions};
use serde_json::Value;

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

fn suffixed(record: &Path, suffix: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(format!("{}{suffix}", record.display()))
}

fn recorded_env(record: &Path) -> Value {
    common::json(&read(&suffixed(record, ".env")))
}

/// The pane's record: its worktree `worktree` in the checkout `checkout`, PORT 4141.
fn worktree(worktree: &str, checkout: &str) -> PaneWorktree {
    PaneWorktree {
        worktree_path: Some(worktree.into()),
        branch: Some("mighty/pane".into()),
        base_branch: Some("main".into()),
        base_commit: Some("0123456789abcdef0123456789abcdef01234567".into()),
        checkout: Some(checkout.into()),
        port: 4141,
        ..PaneWorktree::default()
    }
}

/// "the run service tells a pane worktree's agent once, the transcript keeping the request as sent, and its run gets the pane's PORT".
#[tokio::test(flavor = "multi_thread")]
async fn a_pane_worktrees_agent_is_told_once_and_its_run_gets_its_port() {
    common::korean();
    let directory = common::temp("pane-worktree-runs");
    let folder = directory.join("workspace");
    let tree = directory.join("worktrees").join("pane");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::create_dir_all(&tree).unwrap();
    let record = directory.join("fake");
    let catalog = common::fake_catalog(&["--fake-cli", "codex", &record.to_string_lossy(), "--record-env"]);
    let (service, _events) = RunService::new(
        AppSnapshot::default(),
        catalog,
        RunServiceOptions {
            plugin_directory: String::new(),
            permission_card: true,
            remote: None,
            configure: None,
            admitting: None,
            binding: None,
            delegation: None,
        },
    );
    let (folder_text, tree_text) = (folder.to_string_lossy().into_owned(), tree.to_string_lossy().into_owned());
    let workspace = Workspace { name: Some("w".into()), path: Some(folder_text.clone()), ..Workspace::default() };
    let record_of_pane = worktree(&tree_text, &folder_text);
    let pane = RunSession {
        id: Some("pane".into()),
        workspace_id: workspace.id.clone(),
        provider: Some("codex".into()),
        working_folder: Some(tree_text.clone()),
        pane_worktree: Some(record_of_pane.clone()),
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
    let workspace_id = workspace.id.clone().unwrap();
    let start = |input: &str| StartRunRequest::new("pane", &workspace_id, "claude", input).with_provider("codex");
    let ended = |service: &RunService| {
        !service.is_session_running("pane")
            && service.snapshot().session("pane").and_then(|s| s.status.clone()).as_deref() != Some("running")
    };
    // A slash command never carries the note, and the note is still to come.
    service.start(start("/status")).await.unwrap();
    common::until(|| ended(&service), 30000, "the slash command's run ends").await;
    assert_eq!(read(&suffixed(&record, ".prompt")), "/status", "a slash command goes as typed");
    assert!(service.snapshot().session("pane").unwrap().pane_worktree.as_ref().unwrap().note_given.is_none());
    // The first request: the CLI gets the note, the transcript the request as sent, the record `noteGiven`.
    service.start(start("fix it")).await.unwrap();
    common::until(|| ended(&service), 30000, "the first run ends").await;
    assert_eq!(read(&suffixed(&record, ".prompt")), record_of_pane.with_note("fix it"), "the CLI got the note once");
    let saved = service.snapshot().session("pane").unwrap().as_ref().clone();
    assert_eq!(saved.pane_worktree.as_ref().and_then(|p| p.note_given), Some(true), "noteGiven is saved");
    let asked: Vec<String> = saved.logs().iter().filter(|l| l.kind() == "user").filter_map(|l| l.text.clone()).collect::<Vec<_>>();
    assert_eq!(asked, vec!["/status".to_string(), "fix it".to_string()], "the transcript keeps the requests as sent");
    let env = recorded_env(&record);
    assert_eq!(env["PORT"], "4141");
    assert_eq!(env["MIGHTY_WORKSPACE_FOLDER"], folder_text.as_str());
    assert_eq!(env["MIGHTY_WORKTREE"], tree_text.as_str());
    // The next request goes as typed.
    service.start(start("and again")).await.unwrap();
    common::until(|| ended(&service), 30000, "the second run ends").await;
    assert_eq!(read(&suffixed(&record, ".prompt")), "and again", "the note is given once");
    service.dispose().await;
    common::remove_when_free(&directory).await;
}

/// "only the pane environment's three keys reach the process, never a value holding a NUL".
#[tokio::test(flavor = "multi_thread")]
async fn only_the_three_keys_reach_the_process_and_never_a_nul() {
    common::korean();
    let directory = common::temp("pane-environment");
    let workspace = Workspace { path: Some(directory.to_string_lossy().into_owned()), ..Workspace::default() };
    let record = directory.join("fake");
    let catalog = common::fake_catalog(&["--fake-cli", "codex", &record.to_string_lossy(), "--record-env"]);
    let events: Arc<Mutex<Vec<RunEvent>>> = Arc::default();
    let sink = events.clone();
    let emit: Emit = Arc::new(move |event| sink.lock().unwrap().push(event));
    let resolved = workspace.clone();
    let resolve: ResolveWorkspace = Arc::new(move |_| {
        let workspace = resolved.clone();
        Box::pin(async move { Ok(workspace) })
    });
    let manager = RunManager::new(catalog, RunManagerOptions::new("", resolve, emit));
    let mut request = StartRunRequest::new("pane", workspace.id.as_deref().unwrap(), "claude", "hello").with_provider("codex");
    request.pane_environment = Some(vec![
        ("PORT".into(), "4142".into()),
        ("MIGHTY_NOT_A_PANE_KEY".into(), "no".into()),
        ("MIGHTY_WORKTREE".into(), "C:\\with\0nul".into()),
    ]);
    manager.start(request).await.unwrap();
    let completed = || events.lock().unwrap().iter().any(|e| e.session_id == "pane" && e.status.as_deref() == Some("completed"));
    common::until(completed, 30000, "the run completes").await;
    let env = recorded_env(&record);
    assert_eq!(env["PORT"], "4142", "an allowed key reaches the process");
    assert!(env["MIGHTY_NOT_A_PANE_KEY"].is_null(), "another key never does");
    assert!(env["MIGHTY_WORKTREE"].is_null(), "nor a value with a NUL in it");
    manager.dispose().await;
    common::remove_when_free(&directory).await;
}
