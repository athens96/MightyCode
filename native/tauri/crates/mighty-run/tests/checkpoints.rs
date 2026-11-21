//! The snapshots of a run's folder against a real repository: the files a step changed (an edit, a new file, a deletion, a rename),
//! ignored files left out, a file's change read back, and nothing written into the repository's own objects or index.

#![cfg(any(windows, target_os = "macos"))]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use mighty_platform::ProcessHost;
use mighty_run::checkpoints::{Checkpoints, Job, Snapshots};
use mighty_run::git::Git;
use mighty_run::model::RunEvent;

fn processes() -> Arc<dyn ProcessHost> {
    #[cfg(windows)]
    {
        Arc::new(mighty_platform_windows::WindowsProcessHost::new())
    }
    #[cfg(target_os = "macos")]
    {
        mighty_platform_macos::process_host()
    }
}

/// A fixture git, through the app's own runner.
async fn git(base: &Path, folder: &Path, arguments: &[&str]) -> String {
    let mut args = vec!["-c", "user.name=t", "-c", "user.email=t@example.com", "-c", "commit.gpgsign=false", "-c", "core.autocrlf=false"];
    args.extend_from_slice(arguments);
    let run = Git::new(processes(), hooks(base)).run_detailed(&args, folder, 30.0, None).await;
    let result = run.result.expect("git runs");
    assert_eq!(result.exit_code, 0, "git {arguments:?}: {}", result.stderr);
    result.stdout
}

/// A repository with one commit: `a.txt`, `old.txt` and `.gitignore` (which leaves out `*.log`).
async fn repository(name: &str) -> (PathBuf, PathBuf) {
    let base = std::env::temp_dir().join(format!("mc-checkpoints-{name}-{}", mighty_core::wire::new_id()));
    let folder = base.join("repo");
    std::fs::create_dir_all(&folder).unwrap();
    git(&base, &folder, &["init", "-q"]).await;
    std::fs::write(folder.join("a.txt"), "one\ntwo\n").unwrap();
    std::fs::write(folder.join("old.txt"), "a file that moves\nwith some lines\nso the rename is seen\n").unwrap();
    std::fs::write(folder.join(".gitignore"), "*.log\n").unwrap();
    git(&base, &folder, &["add", "-A"]).await;
    git(&base, &folder, &["commit", "-q", "-m", "first"]).await;
    (base, folder)
}

fn hooks(base: &Path) -> PathBuf {
    if cfg!(windows) { base.join("git-no-hooks") } else { PathBuf::from("/dev/null") }
}

fn objects(folder: &Path) -> usize {
    walk(&folder.join(".git").join("objects"))
}

fn walk(folder: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(folder) else { return 0 };
    entries.flatten().map(|e| if e.path().is_dir() { walk(&e.path()) } else { 1 }).sum()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_step_lists_the_files_it_changed_and_the_repository_is_untouched() {
    let (base, folder) = repository("files").await;
    let snapshots = Snapshots::new(base.join("snapshots"), Git::new(processes(), hooks(&base)));
    let index_before = std::fs::read(folder.join(".git").join("index")).unwrap();
    let objects_before = objects(&folder);

    let (repository, start) = snapshots.snapshot(&folder).await.expect("a snapshot of a repository");
    std::fs::write(folder.join("a.txt"), "one\nTWO\nthree\n").unwrap();
    std::fs::write(folder.join("b.md"), "# new\n").unwrap();
    std::fs::rename(folder.join("old.txt"), folder.join("new.txt")).unwrap();
    std::fs::write(folder.join("build.log"), "ignored\n").unwrap();
    let (_, step) = snapshots.snapshot(&folder).await.expect("a second snapshot");
    assert_ne!(start, step);

    let (files, more) = snapshots.changes(&repository, &start, &step).await.expect("the changes");
    assert_eq!(more, 0);
    let mut seen: Vec<String> =
        files.iter().map(|f| format!("{} {} {:?} {:?} {:?}", f.status, f.path, f.old_path, f.added, f.removed)).collect();
    seen.sort();
    assert_eq!(seen, vec!["A b.md None Some(1) Some(0)", "M a.txt None Some(2) Some(1)", "R new.txt Some(\"old.txt\") Some(0) Some(0)",]);

    let diff = snapshots.file_diff(&folder, &start, &step, "a.txt", None).await.expect("the file's change");
    assert!(diff.contains("-two") && diff.contains("+TWO") && diff.contains("+three"), "{diff}");

    // The repository's index and objects are as they were: the snapshots' own went to the store.
    assert_eq!(std::fs::read(folder.join(".git").join("index")).unwrap(), index_before);
    assert_eq!(objects(&folder), objects_before);
    assert!(walk(&repository.store.join("objects")) > 0);
    let _ = std::fs::remove_dir_all(&base);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_worker_sends_each_runs_checkpoints_and_skips_a_step_that_changed_nothing() {
    let (base, folder) = repository("worker").await;
    let snapshots = Snapshots::new(base.join("snapshots"), Git::new(processes(), hooks(&base)));
    let sent: Arc<Mutex<Vec<RunEvent>>> = Arc::default();
    let into = sent.clone();
    let checkpoints = Checkpoints::start(snapshots, Arc::new(move |event| into.lock().unwrap().push(event)));
    let job = |run: &str, kind: &str, key: Option<&str>| Job {
        session_id: "pane".into(),
        run_id: run.into(),
        folder: folder.clone(),
        kind: kind.into(),
        label: key.map(|k| format!("step {k}")),
        step_key: key.map(str::to_string),
        base: None,
    };
    checkpoints.ask(job("run", "start", None));
    // Waits for each snapshot before changing the folder, as a run's steps come one after another.
    let wait = |count: usize| {
        let sent = sent.clone();
        async move {
            for _ in 0..200 {
                if sent.lock().unwrap().len() >= count {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            panic!("no checkpoint");
        }
    };
    wait(1).await;
    std::fs::write(folder.join("a.txt"), "changed\n").unwrap();
    checkpoints.ask(job("run", "step", Some("id:1")));
    wait(2).await;
    // Nothing changed: no checkpoint.
    checkpoints.ask(job("run", "step", Some("id:2")));
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;
    assert_eq!(sent.lock().unwrap().len(), 2);
    std::fs::write(folder.join("c.txt"), "from the background task\n").unwrap();
    checkpoints.ask(job("run:notice:1", "turn", None));
    wait(3).await;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let sent = sent.lock().unwrap();
    assert_eq!(sent.len(), 3, "{sent:?}");
    assert!(sent.iter().all(|e| e.kind == "checkpoint" && e.session_id == "pane"));
    let step = sent[1].graph_run.as_ref().unwrap();
    assert_eq!(step.id(), "run");
    let kept = step.checkpoints();
    assert_eq!(kept.iter().map(|c| c.kind.as_str()).collect::<Vec<_>>(), vec!["start", "step"]);
    assert_eq!(kept[1].base.as_deref(), Some(kept[0].tree.as_str()));
    assert_eq!(kept[1].files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), vec!["a.txt"]);
    // The notice turn is measured from the request's last tree, the step's.
    let turn = sent[2].graph_run.as_ref().unwrap();
    assert_eq!(turn.id(), "run:notice:1");
    let turn = &turn.checkpoints()[0];
    assert_eq!(turn.base.as_deref(), Some(kept[1].tree.as_str()));
    assert_eq!(turn.files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), vec!["c.txt"]);
    drop(sent);
    let _ = std::fs::remove_dir_all(&base);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_folder_outside_a_repository_has_no_snapshot() {
    let base = std::env::temp_dir().join(format!("mc-checkpoints-none-{}", mighty_core::wire::new_id()));
    std::fs::create_dir_all(base.join("plain")).unwrap();
    // Keeps git from finding a repository around the temporary folder.
    let snapshots = Snapshots::new(base.join("snapshots"), Git::new(processes(), hooks(&base)));
    let found = snapshots.repository(&base.join("plain")).await;
    if let Some(found) = found {
        // A temporary folder inside a repository (a checkout's own temp): nothing to check here.
        assert!(!found.top.starts_with(&base));
    }
    let _ = std::fs::remove_dir_all(&base);
}

/// A checkpoint event lands on its run, once however often it is sent again, and the tracker's next record of the run keeps it.
#[test]
fn a_checkpoint_lands_on_its_run_and_the_trackers_next_record_keeps_it() {
    use mighty_core::graph::{CheckpointFile, MightyGraphRun, RunCheckpoint};
    use mighty_core::model::{AppSnapshot, RunSession};
    use mighty_core::time::DateTimeOffset;
    let run = MightyGraphRun { id: Some("r1".into()), status: Some("running".into()), ..MightyGraphRun::default() };
    let pane = RunSession {
        id: Some("s1".into()),
        kind: Some("claude".into()),
        provider: Some("codex".into()),
        graph_runs: Some(vec![run.clone()]),
        ..RunSession::default()
    };
    let state = AppSnapshot { sessions: Some(vec![Arc::new(pane)]), ..AppSnapshot::default() };
    let tree = |c: char| c.to_string().repeat(40);
    let checkpoint = RunCheckpoint {
        kind: "step".into(),
        label: Some("write the parser".into()),
        step_key: Some("id:1".into()),
        tree: tree('b'),
        base: Some(tree('a')),
        files: vec![CheckpointFile {
            path: "src/parser.rs".into(),
            status: "M".into(),
            added: Some(3),
            removed: Some(1),
            ..CheckpointFile::default()
        }],
        ..RunCheckpoint::default()
    };
    let event = RunEvent {
        graph_run: Some(MightyGraphRun { id: Some("r1".into()), checkpoints: Some(vec![checkpoint.clone()]), ..MightyGraphRun::default() }),
        ..RunEvent::new("s1", "checkpoint")
    };
    assert!(event.valid());
    let now = DateTimeOffset::now();
    let once = mighty_run::apply::apply(&state, &event, now);
    let twice = mighty_run::apply::apply(&once, &event, now);
    assert_eq!(twice.sessions()[0].graph_runs()[0].checkpoints(), std::slice::from_ref(&checkpoint));
    // The tracker's next record of the run has no checkpoints of its own.
    let tracked =
        RunEvent { graph_run: Some(MightyGraphRun { status: Some("completed".into()), ..run }), ..RunEvent::new("s1", "graph_run") };
    let after = mighty_run::apply::apply(&twice, &tracked, now);
    let kept = &after.sessions()[0].graph_runs()[0];
    assert_eq!(kept.status(), "completed");
    assert_eq!(kept.checkpoints(), std::slice::from_ref(&checkpoint));
    // Written and read back whole.
    use mighty_core::json::Wire;
    let json = kept.to_json();
    let read = MightyGraphRun::read(&serde_json::from_str(&json).unwrap()).unwrap();
    assert_eq!(read.checkpoints(), std::slice::from_ref(&checkpoint));
}

/// Every agent the diagram follows, not Claude alone: a step of its checklist done and its run's end are due for a snapshot, and an
/// activity step (a tool call) is not.
#[test]
fn every_agents_steps_and_end_are_due_for_a_snapshot() {
    use mighty_core::graph::MightyGraphRun;
    use mighty_core::model::{AppSnapshot, RunSession, TodoProgress};
    use mighty_core::time::DateTimeOffset;
    use mighty_run::checkpoints::due;
    use mighty_run::plan_mode;
    for provider in ["codex", "gemini", "opencode", "hermes", "pi"] {
        let run = MightyGraphRun {
            id: Some("r".into()),
            status: Some("running".into()),
            source_run_id: Some("r".into()),
            provider: Some(provider.into()),
            ..MightyGraphRun::default()
        };
        let pane = RunSession {
            id: Some("s".into()),
            kind: Some("claude".into()),
            provider: Some(provider.into()),
            graph_runs: Some(vec![run.clone()]),
            ..RunSession::default()
        };
        let mut state = AppSnapshot { sessions: Some(vec![Arc::new(pane)]), ..AppSnapshot::default() };
        let now = DateTimeOffset::now();
        let todos = |status: &str| {
            let mut event = RunEvent::new("s", "todos");
            event.todos = Some(TodoProgress {
                items: Some(vec![
                    plan_mode::todo_item("Write the parser", status, None, Some("1")),
                    plan_mode::todo_item("Read src/a.rs", status, None, Some("activity:call-1")),
                ]),
                run_id: Some("r".into()),
            });
            event
        };
        let runs = |state: &AppSnapshot| state.sessions()[0].graph_runs().to_vec();
        state = mighty_run::apply::apply(&state, &todos("in_progress"), now);
        let before = runs(&state);
        state = mighty_run::apply::apply(&state, &todos("completed"), now);
        let steps = due(&before, &runs(&state));
        assert_eq!(steps.len(), 1, "{provider}: one step, not the activity: {steps:?}");
        assert_eq!((steps[0].1.as_str(), steps[0].2.as_deref()), ("step", Some("Write the parser")), "{provider}");
        let before = runs(&state);
        let ended =
            RunEvent { graph_run: Some(MightyGraphRun { status: Some("completed".into()), ..run }), ..RunEvent::new("s", "graph_run") };
        state = mighty_run::apply::apply(&state, &ended, now);
        let end = due(&before, &runs(&state));
        assert_eq!(end.iter().map(|d| d.1.as_str()).collect::<Vec<_>>(), vec!["end"], "{provider}");
    }
}
