//! Background memory-policy scheduler and retention garbage collector (`MC-12`, `docs/memory-policy.md`):
//! - Skips completely when `!host.writes() || host.smoke.is_some()`.
//! - Runs a 60s idle-body eviction tick (`bodies::evict_idle`), a delayed startup GC pass (8s after launch), and a 30m periodic GC
//!   across `mighty_core::retention::sweep_profile_dir`, `Checkpoints::sweep_with_caps`, `usage_report` closed-session pruning,
//!   expired `mobile-remote/uploads/*.part` cleanup, and `host.log` size rotation.
//! - Exposes `"memoryPolicyStatus"` and `"runRetentionGc"` IPC methods.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use chrono::Utc;
use mighty_core::retention::{self, GcReport, RetentionPolicy};
use mighty_run::checkpoints::Checkpoints;
use serde_json::json;
use tauri::Manager as _;

use crate::host::{Host, Registry, lock};

const STARTUP_GC_DELAY: Duration = Duration::from_secs(8);
const EVICT_TICK_INTERVAL: Duration = Duration::from_secs(60);
const PERIODIC_GC_TICKS: u64 = 30;
const EXPIRED_UPLOAD_PART_AGE: Duration = Duration::from_secs(15 * 60);

#[derive(Default)]
struct GcStatus {
    last_gc_at: Option<String>,
    last_report: Option<GcReport>,
}

#[derive(Default)]
pub struct RetentionGcState {
    running: Mutex<()>,
    status: Mutex<GcStatus>,
}

fn gc_state(host: &Host) -> Arc<RetentionGcState> {
    if let Some(existing) = host.app.try_state::<Arc<RetentionGcState>>() {
        return existing.inner().clone();
    }
    let created = Arc::new(RetentionGcState::default());
    host.app.manage(created.clone());
    host.app.try_state::<Arc<RetentionGcState>>().map(|s| s.inner().clone()).unwrap_or(created)
}

fn wait_weak(weak: &Weak<Host>, duration: Duration) -> Option<Arc<Host>> {
    let host = weak.upgrade()?;
    let open = host.wait_or_close(duration);
    drop(host);
    if !open {
        return None;
    }
    weak.upgrade().filter(|h| !h.closing())
}

/// The snapshot stores of the repositories the open panes work in: the size caps never remove them (a store is a live pane's
/// checkpoint history, and the next snapshot would have to rebuild it).
fn protected_snapshot_stores(snapshot: &mighty_core::model::AppSnapshot) -> HashSet<String> {
    let Some(snapshots) = crate::runs::SNAPSHOTS.get() else { return HashSet::new() };
    let mut folders: Vec<std::path::PathBuf> = Vec::new();
    for pane in snapshot.sessions() {
        if let Some(folder) = mighty_run::checkpoints::folder_of(snapshot, pane.id())
            && !folders.contains(&folder)
        {
            folders.push(folder);
        }
    }
    tauri::async_runtime::block_on(snapshots.store_names(&folders))
}

/// Runs a single full retention & GC pass on a blocking thread when writing is enabled.
pub fn run_gc_pass(host: &Arc<Host>, policy: &RetentionPolicy) -> GcReport {
    if !host.writes() || host.smoke.is_some() || host.closing() {
        return GcReport::default();
    }
    let state = gc_state(host);
    let _serial = lock(&state.running);
    if host.closing() {
        return GcReport::default();
    }

    let evicted_panes = crate::bodies::evict_idle_with_policy(host, policy);
    let snapshot = host.current();
    let live_session_ids: HashSet<String> = snapshot.sessions().iter().map(|s| s.id().to_string()).collect();
    let image_cache_dir = host.runs().map(|r| r.images.directory().to_path_buf()).unwrap_or_else(|| host.data_dir.join("image-cache"));
    // The sweep reads `sessions/*` for the hashes they reference itself; only the panes in memory are added here (one read, not two).
    let referenced_images = retention::collect_snapshot_images(&image_cache_dir, &snapshot);

    let mut report = retention::sweep_profile_dir(&host.data_dir, policy, &referenced_images);
    report.evicted_panes = evicted_panes;

    let snapshots_dir = host.data_dir.join("snapshots");
    let protected = protected_snapshot_stores(&snapshot);
    let (swept_stores, snap_bytes) = Checkpoints::sweep_with_caps(
        &snapshots_dir,
        policy.snapshot_keep_days,
        policy.snapshot_max_repo_bytes,
        policy.snapshot_max_total_bytes,
        &protected,
    );
    report.snapshot_stores_swept = swept_stores;
    report.bytes_reclaimed += snap_bytes;

    // Prune closed-session deduplication baselines inside the usage ledger's owner (its copy in memory is the one saved).
    report.usage_sessions_pruned = crate::usage_report::prune_closed_panes(host, &live_session_ids);

    // Clean expired `.part` staging files when no mobile upload is open.
    if crate::phone::open_upload_count(host) == 0 {
        let uploads_dir = host.data_dir.join("mobile-remote").join("uploads");
        let (parts_removed, part_bytes) = retention::sweep_expired_upload_parts(&uploads_dir, EXPIRED_UPLOAD_PART_AGE);
        report.upload_parts_removed = parts_removed;
        report.bytes_reclaimed += part_bytes;
    }

    let _ = host.log.rotate_if_oversize(policy.host_log_max_bytes);

    {
        let mut status = lock(&state.status);
        status.last_gc_at = Some(Utc::now().to_rfc3339());
        status.last_report = Some(report.clone());
    }
    report
}

/// Starts the background idle-body eviction ticker (60s) and retention GC scheduler (8s startup delay + 30m periodic).
/// No-op when `!host.writes() || host.smoke.is_some()`.
pub fn start_background_tasks(host: &Arc<Host>) {
    if !host.writes() || host.smoke.is_some() {
        return;
    }
    let _ = gc_state(host);
    let weak = Arc::downgrade(host);
    let _ = std::thread::Builder::new().name("retention-gc".into()).spawn(move || {
        let policy = RetentionPolicy::default();
        let Some(initial_host) = wait_weak(&weak, STARTUP_GC_DELAY) else { return };
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_gc_pass(&initial_host, &policy);
        }));
        drop(initial_host);

        let mut remaining_until_startup_tick = EVICT_TICK_INTERVAL.saturating_sub(STARTUP_GC_DELAY);
        let mut ticks = 0u64;
        loop {
            let sleep_for = if ticks == 0 { remaining_until_startup_tick } else { EVICT_TICK_INTERVAL };
            remaining_until_startup_tick = EVICT_TICK_INTERVAL;
            let Some(tick_host) = wait_weak(&weak, sleep_for) else { return };
            ticks += 1;
            let run_full = ticks.is_multiple_of(PERIODIC_GC_TICKS);
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                if run_full {
                    run_gc_pass(&tick_host, &policy);
                } else {
                    crate::bodies::evict_idle(&tick_host);
                }
            }));
            drop(tick_host);
        }
    });
}

pub fn register(methods: &mut Registry) {
    methods.add("memoryPolicyStatus", |host: Arc<Host>, _| async move {
        let policy = RetentionPolicy::default();
        let snapshot = host.current();
        let mut total_panes = 0usize;
        let mut hot_panes = 0usize;
        let mut cold_panes = 0usize;
        for pane in snapshot.sessions() {
            if !matches!(pane.kind(), "claude" | "shell") || pane.mirror.is_some() {
                continue;
            }
            total_panes += 1;
            if pane.body_on_disk {
                cold_panes += 1;
            } else {
                hot_panes += 1;
            }
        }
        let (last_gc_at, last_report) = {
            let st = gc_state(&host);
            let status = lock(&st.status);
            (status.last_gc_at.clone(), status.last_report.clone())
        };
        Ok(json!({
            "policy": policy,
            "writes": host.writes(),
            "activeWorkspaceId": snapshot.active_workspace_id,
            "totalPanes": total_panes,
            "hotPanes": hot_panes,
            "coldPanes": cold_panes,
            "lastGcAt": last_gc_at,
            "lastReport": last_report,
        }))
    });

    methods.add("runRetentionGc", |host: Arc<Host>, _| async move {
        if !host.writes() || host.smoke.is_some() {
            return serde_json::to_value(GcReport::default()).map_err(|e| e.to_string());
        }
        let report = tauri::async_runtime::spawn_blocking(move || run_gc_pass(&host, &RetentionPolicy::default()))
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_value(report).map_err(|e| e.to_string())
    });
}
