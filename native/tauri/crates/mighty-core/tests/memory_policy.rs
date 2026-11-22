//! Integration tests for `MC-12` memory policy (`docs/memory-policy.md`):
//! 1) Incremental `sessions/<stem>.jsonl` append while `"running"`, crash-recovery replay on `Store::load`, delta sync queries
//!    (`read_body_deltas_since`), and compaction back to `<stem>.json` (deleting `.jsonl`) on run completion and pane close.
//! 2) `evict_loaded_body` + `evicted_to_disk` preserving both latest `"user"` and `"assistant"` in `digest.tail`, monotonic
//!    `bodyRevision`, and `graphBlockSizes` across `normalize`, `read_cold_body_logs` non-mutating reads, and lossless
//!    `load_pane_body` restoration.
//! 3) `retention::sweep_profile_dir` + `UsageLedger::prune_closed_sessions` enforcing caps while keeping valid user state intact.

mod common;

use std::collections::{HashSet, btree_map};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use common::temp_dir;
use indexmap::IndexMap;
use mighty_core::graph::{GraphBlockSize, MightyGraphRun, node_id};
use mighty_core::json::{JsonOut, Wire};
use mighty_core::locale::Locale;
use mighty_core::model::{
    AgentRunTiming, AppSnapshot, BackgroundTask, BackgroundWork, LogEntry, PlanRecord, RunSession, SessionUsage, ShellBlockRun, TodoItem,
    TodoProgress, Workspace,
};
use mighty_core::normalize;
use mighty_core::retention::{RetentionPolicy, sweep_profile_dir};
use mighty_core::store::{self, BodyJournalOp, Store};
use mighty_core::time::DateTimeOffset;
use mighty_core::usage_ledger::UsageLedger;
use serde_json::Value;

const P1: &str = "11111111-1111-4111-8111-111111111111";
const P2: &str = "22222222-2222-4222-8222-222222222222";

fn en() -> Locale {
    Locale::new("en")
}

fn root_path(suffix: &str) -> String {
    if cfg!(windows) { format!(r"C:\mighty-test\{suffix}") } else { format!("/mighty-test/{suffix}") }
}

fn log_entry(id: &str, kind: &str, text: &str, minute: u32) -> LogEntry {
    LogEntry {
        id: Some(id.into()),
        kind: Some(kind.into()),
        text: Some(text.into()),
        timestamp: Some(format!("2026-11-22T10:{minute:02}:00Z")),
        provider: Some("claude".into()),
        activity: None,
        images: None,
    }
}

fn graph_run(id: &str, status: &str, input: &str, output: Option<&str>) -> MightyGraphRun {
    MightyGraphRun {
        id: Some(id.into()),
        input: Some(input.into()),
        status: Some(status.into()),
        provider: Some("claude".into()),
        root_entries: Some(vec![log_entry(&format!("{id}-e1"), "assistant", output.unwrap_or("working"), 1)]),
        agents: Some(vec![]),
        final_output: output.map(str::to_string),
        ..MightyGraphRun::default()
    }
}

fn set_mtime_ago(path: &Path, ago: Duration) {
    let target = SystemTime::now().checked_sub(ago).unwrap_or(SystemTime::UNIX_EPOCH);
    let file = fs::File::options().write(true).open(path).unwrap();
    file.set_modified(target).unwrap();
}

#[test]
fn incremental_journal_appends_while_running_replays_on_crash_and_compacts_when_completed() {
    let dir = temp_dir("mem-journal");
    let ws = Workspace {
        id: Some("w1".into()),
        name: Some("Alpha".into()),
        path: Some(root_path("alpha")),
        created_at: Some("2026-11-22T00:00:00Z".into()),
        ..Workspace::default()
    };
    let pane = RunSession {
        id: Some(P1.into()),
        workspace_id: Some("w1".into()),
        title: Some("Build".into()),
        title_mode: Some("fixed".into()),
        kind: Some("claude".into()),
        provider: Some("claude".into()),
        status: Some("idle".into()),
        logs: Some(vec![log_entry("u0", "user", "initial prompt", 0), log_entry("a0", "assistant", "initial answer", 1)]),
        graph_runs: Some(vec![graph_run("run-0", "completed", "initial prompt", Some("initial answer"))]),
        ..RunSession::default()
    };
    let initial = AppSnapshot {
        workspaces: Some(vec![ws.clone()]),
        sessions: Some(vec![Arc::new(pane)]),
        active_workspace_id: Some("w1".into()),
        active_session_id: Some(P1.into()),
        ..AppSnapshot::default()
    };

    // Seed approved workspace via initial state file load + first split save (rev 1).
    fs::write(dir.join(store::STATE_FILE), initial.to_json()).unwrap();
    let mut store = Store::new(dir.clone(), None, true);
    let loaded = store.load(&en()).unwrap();
    store.save(&loaded).unwrap();

    let sessions_dir = dir.join(store::SESSIONS_DIR);
    let body_path = sessions_dir.join(store::body_file_name(P1));
    let journal_path = sessions_dir.join(store::journal_file_name(P1));
    assert!(body_path.exists() && !journal_path.exists(), "idle save writes canonical .json only");

    // Top-level `<data_dir>` parity check: no journal files outside `sessions/`.
    let mut top_names: Vec<String> = fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    top_names.sort();
    assert!(top_names.iter().all(|n| !n.ends_with(".jsonl")), "top-level dir must never hold .jsonl: {top_names:?}");

    let base_bytes_rev1 = fs::read(&body_path).unwrap();
    let started = DateTimeOffset::now();

    // Streaming tick 1 (`status == "running"`, rev 2): append user + streaming assistant + active graphRun.
    let mut tick1 = store.snapshot.clone();
    {
        let mut p = RunSession::clone(tick1.session(P1).unwrap());
        p.status = Some("running".into());
        p.run_timing = Some(AgentRunTiming { started_at: started, last_observed_at: started, finished_at: None, is_approximate: false });
        let mut logs = p.logs().to_vec();
        logs.push(log_entry("u1", "user", "stream incremental changes", 2));
        logs.push(log_entry("a1", "assistant", "part 1", 3));
        p.logs = Some(logs);
        let mut runs = p.graph_runs().to_vec();
        runs.push(graph_run("run-1", "running", "stream incremental changes", None));
        p.graph_runs = Some(runs);
        tick1.sessions = Some(vec![Arc::new(p)]);
    }
    store.save(&tick1).unwrap();
    assert!(journal_path.exists(), "running incremental tick creates sessions/<stem>.jsonl");
    assert_eq!(fs::read(&body_path).unwrap(), base_bytes_rev1, "canonical <stem>.json is NOT rewritten during running append");
    assert_eq!(store.body_revision(P1), 2);

    // Streaming tick 2 (`status == "running"`, rev 3): replace last log (`LogReplaceLast`) + update `todo_progress` (`Meta`).
    let mut tick2 = store.snapshot.clone();
    {
        let mut p = RunSession::clone(tick2.session(P1).unwrap());
        let mut logs = p.logs().to_vec();
        let last = logs.len() - 1;
        logs[last].text = Some("part 1 + part 2 streamed".into());
        logs.push(log_entry("o1", "output", "cargo test ok", 4));
        p.logs = Some(logs);
        p.todo_progress = Some(TodoProgress {
            items: Some(vec![TodoItem {
                id: Some("t1".into()),
                content: Some("Write journal".into()),
                status: Some("in_progress".into()),
                active_form: None,
            }]),
            run_id: Some("run-1".into()),
        });
        tick2.sessions = Some(vec![Arc::new(p)]);
    }
    store.save(&tick2).unwrap();
    assert_eq!(store.body_revision(P1), 3);
    assert_eq!(fs::read(&body_path).unwrap(), base_bytes_rev1, "canonical <stem>.json still untouched at rev 3");

    // Verify `MC-13` `read_body_deltas_since` reads exact contiguous journal records!
    let deltas = store::read_body_deltas_since(&dir, P1, 1).expect("contiguous deltas since rev 1");
    assert_eq!(deltas.len(), 2);
    assert_eq!((deltas[0].rev, deltas[1].rev), (2, 3));
    assert!(deltas[1].ops.iter().any(|op| matches!(op, BodyJournalOp::LogReplaceLast { .. })));
    assert!(deltas[1].ops.iter().any(|op| matches!(op, BodyJournalOp::Meta { .. })));
    assert_eq!(store::read_body_deltas_since(&dir, P1, 3).unwrap().len(), 0);

    // Simulate power-cut torn trailing line at the end of `.jsonl` and verify `Store::load` replays revs 2..=3 cleanly!
    {
        use std::io::Write as _;
        let mut f = fs::OpenOptions::new().append(true).open(&journal_path).unwrap();
        f.write_all(br#"{"rev":99,"sessionId":"truncated"#).unwrap();
    }
    let mut crash_recovered = Store::new(dir.clone(), None, true);
    crash_recovered.lazy = true;
    let recovered_snap = crash_recovered.load(&en()).unwrap();
    let recovered_pane = recovered_snap.session(P1).unwrap();
    assert_eq!(recovered_pane.status(), "stopped", "restored running pane transitions to stopped");
    assert_eq!(crash_recovered.body_revision(P1), 3);
    let recovered_texts: Vec<&str> = recovered_pane.logs().iter().map(LogEntry::text).collect();
    assert_eq!(
        recovered_texts,
        ["initial prompt", "initial answer", "stream incremental changes", "part 1 + part 2 streamed", "cargo test ok"]
    );
    assert_eq!(recovered_pane.graph_runs().len(), 2);
    assert_eq!(recovered_pane.todo_progress.as_ref().unwrap().items.as_ref().unwrap().len(), 1);

    // Remove the simulated torn fragment and finish the run (`status == "completed"`) without changing body fields:
    // `Store::save` must compact `.jsonl` into `<stem>.json` and delete `.jsonl`!
    let clean_lines: String =
        fs::read_to_string(&journal_path).unwrap().lines().filter(|l| !l.contains("\"rev\":99")).map(|l| format!("{l}\n")).collect();
    fs::write(&journal_path, clean_lines).unwrap();

    let mut completed = store.snapshot.clone();
    {
        let mut p = RunSession::clone(completed.session(P1).unwrap());
        p.status = Some("completed".into());
        p.run_timing = p.run_timing.map(|t| AgentRunTiming { finished_at: Some(t.last_observed_at), ..t });
        completed.sessions = Some(vec![Arc::new(p)]);
    }
    store.save(&completed).unwrap();
    assert!(!journal_path.exists(), "completed run compacts and deletes .jsonl");
    let compacted_json: Value = serde_json::from_slice(&fs::read(&body_path).unwrap()).unwrap();
    assert_eq!(compacted_json["bodyRevision"], 3);
    assert_eq!(compacted_json["logs"].as_array().unwrap().len(), 5);
    assert!(
        store::read_body_deltas_since(&dir, P1, 1).is_none(),
        "deltas before compacted base revision return None so caller falls back to full body"
    );
    assert_eq!(store::read_body_deltas_since(&dir, P1, 3).unwrap().len(), 0);

    // Closing a pane with an orphaned `.jsonl` removes `.json`, `.bak`, and `.jsonl`.
    fs::write(&journal_path, "{\"rev\":4,\"sessionId\":\"11111111-1111-4111-8111-111111111111\",\"ops\":[]}\n").unwrap();
    let mut closed = store.snapshot.clone();
    closed.sessions = Some(vec![]);
    store.save(&closed).unwrap();
    assert!(!body_path.exists() && !journal_path.exists());

    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn eviction_preserves_user_and_assistant_digest_block_sizes_and_revisions_and_restores_losslessly() {
    let dir = temp_dir("mem-evict");
    let w1 = Workspace { id: Some("w1".into()), name: Some("Active".into()), path: Some(root_path("w1")), ..Workspace::default() };
    let w2 = Workspace { id: Some("w2".into()), name: Some("Idle".into()), path: Some(root_path("w2")), ..Workspace::default() };

    let active_pane = RunSession {
        id: Some(P1.into()),
        workspace_id: Some("w1".into()),
        title: Some("Active".into()),
        kind: Some("claude".into()),
        provider: Some("claude".into()),
        logs: Some(vec![log_entry("p1-u1", "user", "active task", 0)]),
        ..RunSession::default()
    };

    // Build P2 with an early `"user"` request (`u-early`) and early `"assistant"` summary (`a-early`), followed by 10 `"output"`
    // entries so neither `"user"` nor `"assistant"` sits inside the last `DIGEST_TAIL` (8) entries!
    let long_assistant = format!("assistant architecture summary: {}", "detail ".repeat(300));
    let mut p2_logs =
        vec![log_entry("u-early", "user", "refactor authentication middleware", 0), log_entry("a-early", "assistant", &long_assistant, 1)];
    for idx in 0..10 {
        p2_logs.push(log_entry(&format!("out-{idx}"), "output", &format!("step output {idx}"), 2 + idx));
    }
    let mut block_sizes = IndexMap::new();
    let exec_node = node_id("run-a", "execution");
    let res_node = node_id("run-b", "result");
    block_sizes.insert("pending-input".into(), Some(GraphBlockSize { width: 420.0, height: 180.0 }));
    block_sizes.insert(exec_node.clone(), Some(GraphBlockSize { width: 520.0, height: 260.0 }));
    block_sizes.insert(res_node.clone(), Some(GraphBlockSize { width: 640.0, height: 320.0 }));

    let idle_pane = RunSession {
        id: Some(P2.into()),
        workspace_id: Some("w2".into()),
        title: Some("Auth".into()),
        title_mode: Some("fixed".into()),
        kind: Some("claude".into()),
        provider: Some("claude".into()),
        status: Some("completed".into()),
        logs: Some(p2_logs),
        graph_runs: Some(vec![
            graph_run("run-a", "completed", "step a", Some("done a")),
            graph_run("run-b", "completed", "step b", Some("done b")),
        ]),
        graph_block_sizes: Some(block_sizes),
        plan_history: Some(vec![PlanRecord {
            id: Some("plan-1".into()),
            run_id: Some("run-a".into()),
            plan: Some("# Plan\n1. Ship it".into()),
            received_at: Some("2026-11-22T10:00:00Z".into()),
            decided_at: Some("2026-11-22T10:01:00Z".into()),
            outcome: Some("approvedAuto".into()),
            feedback: None,
            plan_truncated: None,
            graph_run_id: Some("run-a".into()),
            launch_override: None,
        }]),
        todo_progress: Some(TodoProgress {
            items: Some(vec![TodoItem {
                id: Some("todo-1".into()),
                content: Some("Verify auth".into()),
                status: Some("completed".into()),
                active_form: None,
            }]),
            run_id: Some("run-b".into()),
        }),
        background_work: Some(BackgroundWork {
            tasks: Some(vec![BackgroundTask {
                id: Some("bg-1".into()),
                kind: Some("shell".into()),
                description: Some("Watcher".into()),
                started_at: Some("2026-11-22T10:00:00Z".into()),
                status: Some("completed".into()),
                tool_use_id: None,
                summary: Some("Done".into()),
                ended_at: Some("2026-11-22T10:02:00Z".into()),
            }]),
            turn_ended: true,
        }),
        shell_block_runs: Some(vec![ShellBlockRun {
            key: Some("u-early#0".into()),
            command: Some("cargo check".into()),
            state: Some("completed".into()),
            exit_code: Some(0),
            output: Some("ok".into()),
            started_at: Some("2026-11-22T10:00:00Z".into()),
            ended_at: Some("2026-11-22T10:01:00Z".into()),
        }]),
        ..RunSession::default()
    };

    let snapshot = AppSnapshot {
        workspaces: Some(vec![w1, w2]),
        sessions: Some(vec![Arc::new(active_pane), Arc::new(idle_pane)]),
        active_workspace_id: Some("w1".into()),
        active_session_id: Some(P1.into()),
        ..AppSnapshot::default()
    };
    fs::write(dir.join(store::STATE_FILE), snapshot.to_json()).unwrap();
    let (mut store, loaded) = {
        let mut s = Store::new(dir.clone(), None, true);
        let snap = s.load(&en()).unwrap();
        (s, snap)
    };
    store.save(&loaded).unwrap();
    let full_before_evict = store.snapshot.session(P2).unwrap().clone();
    assert_eq!(full_before_evict.logs().len(), 12);
    assert_eq!(store.body_revision(P2), 1);

    // Evict P2 to disk!
    let cold = store.evict_loaded_body(P2).expect("P2 evicted");
    assert!(cold.body_on_disk);
    assert_eq!(cold.body_revision, Some(1));
    assert!(cold.graph_runs.is_none());
    assert!(cold.plan_history.is_none());
    assert!(cold.todo_progress.is_none());
    assert!(cold.background_work.is_none());
    assert!(cold.shell_block_runs.is_none());
    assert_eq!(cold.log_count(), 12);

    // `digest.tail` retains BOTH `u-early` and `a-early` in chronological order ahead of the 8 tail entries (total 10), clamped!
    assert_eq!(cold.logs().len(), 10);
    assert_eq!(cold.logs()[0].id(), "u-early");
    assert_eq!(cold.logs()[0].kind(), "user");
    assert_eq!(cold.logs()[1].id(), "a-early");
    assert_eq!(cold.logs()[1].kind(), "assistant");
    assert!(cold.logs()[1].text().len() <= mighty_core::model::DIGEST_TEXT);

    // Page JSON (`write_page` / `Wire::to_json`) NEVER emits `bodyRevision` or `digest` (C# parity).
    let mut page_out = JsonOut::new();
    cold.write_page(&mut page_out);
    let page_json = page_out.finish();
    assert!(!page_json.contains("bodyRevision") && !page_json.contains("\"digest\""), "{page_json}");

    // Re-normalizing a snapshot with cold P2 MUST preserve `graphBlockSizes` and `body_revision`!
    let renormalized = normalize::normalize(&store.snapshot, false).unwrap();
    let cold_norm = renormalized.session(P2).unwrap();
    let kept_sizes = cold_norm.graph_block_sizes.as_ref().expect("cold graph_block_sizes preserved");
    assert!(kept_sizes.contains_key("pending-input"));
    assert!(kept_sizes.contains_key(&exec_node));
    assert!(kept_sizes.contains_key(&res_node));
    assert_eq!(cold_norm.body_revision, Some(1));

    // `read_cold_body_logs` reads all 12 full unclamped logs without hydrating `store`!
    let cold_full_logs = store::read_cold_body_logs(&dir, P2).expect("cold logs on disk");
    assert_eq!(cold_full_logs.len(), 12);
    assert_eq!(cold_full_logs[1].text(), long_assistant, "on-disk log keeps full unclamped text");
    assert!(store.snapshot.session(P2).unwrap().body_on_disk, "store stays cold after read_cold_body_logs");

    // Saving while P2 is cold keeps the full 12-log body on disk at revision 1.
    store.save(&renormalized).unwrap();
    assert_eq!(store.body_revision(P2), 1);
    assert_eq!(store::read_cold_body_logs(&dir, P2).unwrap().len(), 12);

    // `load_pane_body` restores 100% of `logs`, `graphRuns`, `planHistory`, `todoProgress`, `backgroundWork`, `shellRuns`, `graphBlockSizes`.
    let restored = store.load_pane_body(&cold).expect("restored whole");
    assert!(!restored.body_on_disk);
    assert_eq!(restored.body_revision, Some(1));
    assert_eq!(restored.logs().len(), 12);
    assert_eq!(restored.logs()[1].text(), long_assistant);
    assert_eq!(restored.graph_runs().len(), 2);
    assert_eq!(restored.plan_history.as_ref().unwrap().len(), 1);
    assert_eq!(restored.todo_progress.as_ref().unwrap().items.as_ref().unwrap().len(), 1);
    assert_eq!(restored.background_work.as_ref().unwrap().tasks.as_ref().unwrap().len(), 1);
    assert_eq!(restored.shell_block_runs.as_ref().unwrap().len(), 1);
    assert_eq!(restored.graph_block_sizes.as_ref().unwrap().len(), 3);

    // Subsequent edit after evict -> restore increments `bodyRevision` monotonically to 2!
    let mut edited = store.snapshot.clone();
    {
        let mut p = restored;
        let mut logs = p.logs().to_vec();
        logs.push(log_entry("u-after", "user", "follow-up after restore", 20));
        p.logs = Some(logs);
        edited.sessions = Some(vec![edited.session(P1).unwrap().clone(), Arc::new(p)]);
    }
    store.save(&edited).unwrap();
    assert_eq!(store.body_revision(P2), 2);

    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn retention_sweeper_and_usage_ledger_pruning_enforce_caps_and_preserve_user_assets() {
    let dir = temp_dir("mem-retention");
    fs::create_dir_all(dir.join("sessions")).unwrap();
    fs::create_dir_all(dir.join("image-cache")).unwrap();
    fs::create_dir_all(dir.join("work-map/ws-1")).unwrap();
    fs::create_dir_all(dir.join("remote-cache/conn-1")).unwrap();

    // Protected user/state files must NEVER be touched.
    fs::write(dir.join("workspace-state.json"), r#"{"version":1,"workspaces":[],"sessions":[]}"#).unwrap();
    fs::write(dir.join("saved-prompts.json"), r#"[{"id":"sp1","title":"Keep me"}]"#).unwrap();

    // Quarantine: 4 aged (> 30d) `.unreadable-*` -> the newest 3 of them stay, the oldest goes; 4 recent ones are never removed
    // whatever the cap; the one pre-split copy `.before-session-files-*` is never swept, however old.
    let q_aged = dir.join("workspace-state.json.unreadable-20260101-000000-aaaaaaaa");
    fs::write(&q_aged, vec![b'x'; 128]).unwrap();
    set_mtime_ago(&q_aged, Duration::from_secs(43 * 86_400));
    let q_aged_kept: Vec<PathBuf> = (0..3)
        .map(|i| {
            let q = dir.join(format!("workspace-state.json.unreadable-2026010{i}-000000-cccccccc"));
            fs::write(&q, vec![b'x'; 128]).unwrap();
            set_mtime_ago(&q, Duration::from_secs((40 + i) * 86_400));
            q
        })
        .collect();
    let before_split = dir.join("workspace-state.json.before-session-files-20250101-000000");
    fs::write(&before_split, vec![b's'; 256]).unwrap();
    set_mtime_ago(&before_split, Duration::from_secs(400 * 86_400));
    let mut q_recent: Vec<PathBuf> = Vec::new();
    for i in 0..4 {
        let q = dir.join("sessions").join(format!("p{i}.json.unreadable-20261122-00000{i}-bbbbbbbb"));
        fs::write(&q, vec![b'q'; 64]).unwrap();
        set_mtime_ago(&q, Duration::from_secs((10 - i) * 60));
        q_recent.push(q);
    }

    // `.tmp` files: stale (20m old) removed, fresh (now) preserved!
    let stale_tmp = dir.join("sessions/stale.json.tmp");
    let fresh_tmp = dir.join("sessions/fresh.json.tmp");
    fs::write(&stale_tmp, b"stale").unwrap();
    set_mtime_ago(&stale_tmp, Duration::from_secs(20 * 60));
    fs::write(&fresh_tmp, b"in-flight").unwrap();

    // `image-cache`: cap at 1_000 bytes for test; `referenced` image (800 bytes) stays even if oldest, unreferenced oldest evicted!
    let ref_img = dir.join("image-cache").join(format!("{}.png", "a".repeat(64)));
    let old_img = dir.join("image-cache").join(format!("{}.png", "b".repeat(64)));
    let new_img = dir.join("image-cache").join(format!("{}.png", "c".repeat(64)));
    fs::write(&ref_img, vec![1u8; 800]).unwrap();
    set_mtime_ago(&ref_img, Duration::from_secs(45 * 86_400));
    fs::write(&old_img, vec![2u8; 600]).unwrap();
    set_mtime_ago(&old_img, Duration::from_secs(120));
    fs::write(&new_img, vec![3u8; 150]).unwrap();

    // `work-map` and `remote-cache` exceeding byte caps.
    let wm_old = dir.join("work-map/ws-1/old.json");
    let wm_new = dir.join("work-map/ws-1/new.json");
    fs::write(&wm_old, vec![b'w'; 700]).unwrap();
    set_mtime_ago(&wm_old, Duration::from_secs(120));
    fs::write(&wm_new, vec![b'w'; 400]).unwrap();

    let rc_old = dir.join("remote-cache/conn-1/pane-old.json");
    fs::write(&rc_old, vec![b'r'; 500]).unwrap();
    set_mtime_ago(&rc_old, Duration::from_secs(20 * 86_400));

    let policy = RetentionPolicy {
        quarantine_keep_days: 30,
        quarantine_max_files: 3,
        image_cache_keep_days: 30,
        image_cache_max_bytes: 1_000,
        workmap_keep_days: 30,
        workmap_max_bytes: 500,
        remote_cache_keep_days: 14,
        remote_cache_max_bytes: 1_000,
        ..RetentionPolicy::default()
    };
    let mut referenced: HashSet<PathBuf> = HashSet::new();
    referenced.insert(ref_img.clone());

    let report = sweep_profile_dir(&dir, &policy, &referenced);
    assert_eq!(report.quarantine_removed, 1, "only the oldest of the aged quarantine files is past the cap");
    assert!(!q_aged.exists() && q_aged_kept.iter().all(|q| q.exists()), "the newest 3 aged quarantine files stay");
    assert!(q_recent.iter().all(|q| q.exists()), "a quarantine file within its keep period is never removed, whatever the count");
    assert!(before_split.exists(), "the pre-split copy of the state is never swept");
    assert_eq!(report.temp_files_removed, 1, "stale .tmp removed");
    assert_eq!(report.image_cache_removed, 1, "unreferenced oversize image removed while referenced image kept");
    assert_eq!(report.workmap_removed, 1, "oldest work-map file evicted under cap");
    assert_eq!(report.remote_cache_removed, 1, "aged remote-cache file removed");
    assert!(report.bytes_reclaimed > 0);

    assert!(dir.join("saved-prompts.json").exists(), "saved-prompts.json untouched");
    assert!(dir.join("workspace-state.json").exists(), "workspace-state.json untouched");
    assert!(fresh_tmp.exists() && !stale_tmp.exists(), "in-flight .tmp preserved");
    assert!(ref_img.exists() && new_img.exists() && !old_img.exists(), "referenced image protected even when > keep_days");
    assert!(wm_new.exists() && !wm_old.exists());
    assert!(!rc_old.exists());

    // Verify `UsageLedger::prune_closed_sessions` drops closed panes from `last` while preserving live baselines and `records`.
    let mut ledger = UsageLedger::default();
    let sample = |sid: &str| SessionUsage {
        provider: "claude".into(),
        source: "cli".into(),
        token_scope: "session".into(),
        model: Some("claude-sonnet-4-6".into()),
        selected_model: None,
        provider_session_id: Some(sid.into()),
        input_tokens: Some(100),
        output_tokens: Some(50),
        cache_read_tokens: None,
        cache_write_tokens: None,
        reasoning_tokens: None,
        total_tokens: Some(150),
        context_used_tokens: None,
        context_window_tokens: None,
        cost_usd: Some(0.5),
        cost_scope: Some("session".into()),
        rate_limits: None,
        rate_limits_updated_at: None,
        updated_at: "2026-11-22T00:00:00Z".into(),
    };
    ledger.record("live-pane", "w1", "claude", "default", &sample("s-live"), "2026-11-22");
    ledger.record("closed-1", "w1", "claude", "default", &sample("s-dead-1"), "2026-11-22");
    ledger.record("closed-2", "w2", "claude", "default", &sample("s-dead-2"), "2026-11-22");
    assert_eq!(ledger.last.len(), 3);
    assert_eq!(ledger.records.len(), 3);

    let live_ids: HashSet<String> = ["live-pane".to_string()].into_iter().collect();
    let pruned = ledger.prune_closed_sessions(&live_ids);
    assert_eq!(pruned, 2);
    assert_eq!(ledger.last.keys().cloned().collect::<Vec<_>>(), vec!["live-pane".to_string()]);
    assert!(matches!(ledger.last.entry("live-pane".into()), btree_map::Entry::Occupied(_)));
    assert_eq!(ledger.records.len(), 3, "historical daily usage records remain intact");

    fs::remove_dir_all(dir).unwrap();
}
