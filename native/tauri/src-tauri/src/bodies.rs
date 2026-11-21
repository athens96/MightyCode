//! The panes whose body the split state's load left on disk (docs/superpowers/plans/2026-11-20-session-state-files.md, phase 3): a
//! launch reads the active workspace's panes whole and the others' summaries only. A pane is read whole when something needs it: its
//! workspace becoming the active one (`View::sync` triggers a coalesced background read outside `host.view()`), a run starting on it
//! or a change to it (the run service), the phone's or a paired computer's look at it (`PhoneDesk::ensure_body`). The store never
//! takes what such a pane holds for its whole body.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use mighty_core::store::Store;
use mighty_run::RunService;
use mighty_run::service::BodyLoader;
use tokio::sync::watch;

use crate::host::{FlushWake, Host, View, lock};

/// Tracks in-flight lazy workspace body loads so repeated `View::sync` calls coalesce into one blocking read per workspace.
#[derive(Default)]
pub struct BodyLoads {
    in_flight: Mutex<HashMap<String, watch::Receiver<bool>>>,
}

/// The run service's reader of a pane's body: the store's, under its lock (a save waits, or the read waits for a save).
pub fn loader(store: Arc<Mutex<Store>>) -> BodyLoader {
    Arc::new(move |pane| lock(&store).load_pane_body(pane))
}

/// Returns a non-blocking callback for `View::sync` that loads any lazy pane bodies for a workspace off `host.view()` and off
/// tokio async workers, then syncs the view and schedules a flush.
pub(crate) fn workspace_scheduler(
    loads: Arc<BodyLoads>,
    service: Arc<RunService>,
    view: Arc<Mutex<View>>,
    flush_wake: Arc<FlushWake>,
) -> Arc<dyn Fn(String) + Send + Sync> {
    Arc::new(move |workspace: String| {
        let mut map = lock(&loads.in_flight);
        if map.contains_key(&workspace) {
            return;
        }
        let (tx, rx) = watch::channel(false);
        map.insert(workspace.clone(), rx);
        drop(map);
        let loads = loads.clone();
        let service = service.clone();
        let view = view.clone();
        let flush_wake = flush_wake.clone();
        tauri::async_runtime::spawn_blocking(move || {
            struct Finish {
                loads: Arc<BodyLoads>,
                workspace: String,
                tx: watch::Sender<bool>,
            }
            impl Drop for Finish {
                fn drop(&mut self) {
                    lock(&self.loads.in_flight).remove(&self.workspace);
                    let _ = self.tx.send(true);
                }
            }
            let _finish = Finish { loads, workspace: workspace.clone(), tx };
            if service.ensure_workspace_bodies(&workspace) {
                lock(&view).sync();
                flush_wake.request();
            }
        });
    })
}

/// Ensures the active workspace's lazy pane bodies are loaded outside `host.view()` (on a blocking thread, or awaiting an already
/// in-flight load started by `View::sync`).
pub async fn ensure_active(host: &Arc<Host>) {
    let Some(runs) = host.runs().cloned() else { return };
    let workspace = {
        let view = host.view();
        view.snapshot
            .active_workspace_id
            .as_ref()
            .filter(|active| view.snapshot.sessions().iter().any(|s| s.body_on_disk && s.workspace_id() == active.as_str()))
            .cloned()
    };
    let Some(workspace) = workspace else { return };
    let existing = lock(&host.body_loads().in_flight).get(&workspace).cloned();
    if let Some(mut rx) = existing {
        let _ = rx.wait_for(|done| *done).await;
        return;
    }
    let service = runs.service.clone();
    let target = workspace.clone();
    let loaded = tauri::async_runtime::spawn_blocking(move || service.ensure_workspace_bodies(&target)).await.unwrap_or(false);
    if loaded {
        host.view().sync();
    }
}

/// The panes in `ids` read whole if the load left their body on disk; the page's copy follows when one was.
pub fn ensure(host: &Arc<Host>, ids: &[&str]) {
    let Some(runs) = host.runs() else { return };
    if runs.service.ensure_bodies(ids) {
        host.view().sync();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use mighty_core::model::{AppSnapshot, LogEntry, RunSession, Workspace};
    use mighty_run::service::{RunService, RunServiceOptions};

    use super::*;

    /// P1-PERF-1: `View::sync()` never holds `view` across synchronous disk I/O (`BodyLoader`): a stalled store lock does not
    /// block `view` acquisition, while the coalesced background loader still completes and syncs the loaded body into `View`.
    #[tokio::test(flavor = "multi_thread")]
    async fn view_sync_loads_bodies_without_holding_view_lock() {
        let environment = Arc::new(mighty_platform::fake::FakeShellEnvironment::new(Default::default()));
        let catalog = Arc::new(mighty_run::providers::ProviderCatalog::new(
            Arc::new(mighty_platform::fake::FakeProcessHost::default()),
            environment,
            None,
        ));
        let options = RunServiceOptions {
            plugin_directory: String::new(),
            permission_card: false,
            remote: None,
            configure: None,
            admitting: None,
            binding: None,
            delegation: None,
        };
        let mut initial = AppSnapshot {
            workspaces: Some(vec![Workspace { id: Some("w1".into()), path: Some("/w1".into()), ..Workspace::default() }]),
            active_workspace_id: Some("w1".into()),
            ..AppSnapshot::default()
        };
        initial.sessions_mut().push(Arc::new(RunSession {
            id: Some("p1".into()),
            workspace_id: Some("w1".into()),
            body_on_disk: true,
            ..RunSession::default()
        }));
        let (service, _events) = RunService::new(initial, catalog, options);
        let service = Arc::new(service);
        let loader_entered = Arc::new(AtomicBool::new(false));
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let release_rx = Arc::new(Mutex::new(release_rx));
        let (entered_flag, rx_for_loader) = (loader_entered.clone(), release_rx.clone());
        service.set_body_loader(Arc::new(move |pane: &RunSession| {
            entered_flag.store(true, Ordering::SeqCst);
            // Simulates `Store::save` retrying under `lock(&store)` while the loader waits.
            let _ = rx_for_loader.lock().unwrap().recv();
            Some(RunSession {
                body_on_disk: false,
                logs: Some(vec![LogEntry::new("e1".into(), "user", "loaded".into(), "t".into(), None)]),
                ..pane.clone()
            })
        }));

        let loads = Arc::new(BodyLoads::default());
        let flush_wake = FlushWake::new();
        let view = Arc::new(Mutex::new(View::for_test(service.clone())));
        let scheduler = workspace_scheduler(loads.clone(), service.clone(), view.clone(), flush_wake.clone());
        lock(&view).set_body_wake(scheduler);

        // `sync()` must return immediately even while `BodyLoader` is blocked!
        let sync_start = std::time::Instant::now();
        lock(&view).sync();
        assert!(sync_start.elapsed() < Duration::from_millis(50), "View::sync blocked on BodyLoader");
        assert!(lock(&view).snapshot.session("p1").unwrap().body_on_disk, "body is still loading in background");

        // Wait until background loader is actively blocked, and prove `view` is still unblocked.
        for _ in 0..100 {
            if loader_entered.load(Ordering::SeqCst) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(loader_entered.load(Ordering::SeqCst), "background loader started");
        assert!(view.try_lock().is_ok(), "view lock is NOT held while BodyLoader waits on Store");

        // Release the blocked loader: background loader finishes, syncs `view`, and marks body loaded.
        release_tx.send(()).unwrap();
        for _ in 0..100 {
            if !lock(&view).snapshot.session("p1").unwrap().body_on_disk {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let pane = lock(&view).snapshot.session("p1").unwrap().clone();
        assert!(!pane.body_on_disk, "body loaded and synced into view");
        assert_eq!(pane.logs().len(), 1);
    }
}
