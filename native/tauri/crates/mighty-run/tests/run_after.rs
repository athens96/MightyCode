//! A send right after a run's last status (delegation's next item, a phone's send): the manager admits it instead of refusing it, but
//! its run starts only once the pane's older run has ended (its task closed: process tree, pumps), so two runs of one pane never
//! overlap, as C#'s `RunManager` keeps a run listed until its `finally`; and the older run's end leaves the newer entry in place. Over the
//! fake process host, so it runs everywhere.

mod common;

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use mighty_core::model::Workspace;
use mighty_platform::fake::{FakeProcessHost, FakeScript};
use mighty_run::manager::ResolveWorkspace;
use mighty_run::model::{CliCommand, RunEvent, StartRunRequest};
use mighty_run::providers::{Finder, ProviderCatalog};
use mighty_run::{RunManager, RunManagerOptions};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_run_admitted_right_after_the_last_status_starts_once_the_older_one_ended() {
    common::korean();
    let folder = common::temp("run-after");
    let workspace = Workspace { path: Some(folder.to_string_lossy().into_owned()), ..Workspace::default() };
    let resolve: ResolveWorkspace = {
        let workspace = workspace.clone();
        Arc::new(move |_| {
            let workspace = workspace.clone();
            Box::pin(async move { Ok(workspace) })
        })
    };
    // The first child ends at once (its run's last status), the second runs until it is stopped.
    let processes = Arc::new(FakeProcessHost::default());
    processes.push(FakeScript::default());
    processes.push(FakeScript { hang: true, ..FakeScript::default() });
    let binary = folder.join("claude-cli").to_string_lossy().into_owned();
    let finder: Finder = Arc::new(move |_: String| {
        let binary = binary.clone();
        Box::pin(async move { Some(CliCommand::new(binary, vec![], Some("2.1.300"))) })
    });
    let catalog = Arc::new(ProviderCatalog::new(processes.clone(), common::environment(), Some(finder)));

    let manager_cell: Arc<OnceLock<Weak<RunManager>>> = Arc::default();
    let statuses: Arc<Mutex<Vec<String>>> = Arc::default();
    let (first_end, spawned_while_closing) = (Arc::new(AtomicBool::new(true)), Arc::new(AtomicUsize::new(usize::MAX)));
    let request = |input: &str| StartRunRequest::new("pane", workspace.id(), "claude", input);
    let second = request("again");
    let emit = {
        let (cell, statuses, first_end, spawned, processes) =
            (manager_cell.clone(), statuses.clone(), first_end.clone(), spawned_while_closing.clone(), processes.clone());
        Arc::new(move |event: RunEvent| {
            if event.kind != "status" {
                return;
            }
            let status = event.status.clone().unwrap_or_default();
            statuses.lock().unwrap().push(status.clone());
            let terminal = matches!(status.as_str(), "completed" | "error" | "stopped");
            // On the first run's last status, from inside it (its task has not closed yet): the pane's next send is admitted, and while
            // the older run is held here nothing of the newer one may start.
            if terminal && first_end.swap(false, Ordering::SeqCst) {
                let manager = cell.get().and_then(Weak::upgrade).expect("the manager");
                let _ = manager.admit(second.clone(), false).expect("a send right after the last status is admitted, not refused");
                std::thread::sleep(Duration::from_millis(400));
                spawned.store(processes.spawned().len(), Ordering::SeqCst);
            }
        })
    };
    let manager = Arc::new(RunManager::new(catalog, RunManagerOptions::new(&common::plugin(&folder), resolve, emit)));
    manager_cell.set(Arc::downgrade(&manager)).unwrap();

    let _ = manager.admit(request("hello"), false).unwrap();
    common::until(|| spawned_while_closing.load(Ordering::SeqCst) != usize::MAX, 10_000, "the first run's last status").await;
    assert_eq!(spawned_while_closing.load(Ordering::SeqCst), 1, "the newer run starts nothing while the older one is still closing");
    common::until(|| processes.spawned().len() == 2, 10_000, "the newer run's child").await;
    assert!(processes.closed(0), "the older run's child was closed before the newer one started");
    // The newer run started only after the older one's end, so that end is behind us: the newer entry is still the pane's run.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(manager.is_running("pane"), "the older run's end left the newer run listed");

    manager.stop("pane").await.unwrap();
    assert!(!manager.is_running("pane"));
    assert_eq!(statuses.lock().unwrap().last().map(String::as_str), Some("stopped"), "the newer run ends when stopped");
    manager.dispose().await;
    common::remove_when_free(&folder).await;
}
