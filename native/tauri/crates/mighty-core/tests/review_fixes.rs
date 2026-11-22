//! Regression tests for the memory-policy review: eviction from the pane as its owner holds it now, a retention scan that survives
//! multi-byte text, a shared-memory book that never lets a peer reach outside its scope or push the local entries out, and a remote
//! workspace's slug that is never read from this disk.

mod common;

use std::fs;
use std::sync::Arc;

use common::temp_dir;
use mighty_core::Wire;
use mighty_core::locale::Locale;
use mighty_core::model::{AppSnapshot, LogEntry, RunSession, Workspace};
use mighty_core::retention::{collect_referenced_images, collect_snapshot_images};
use mighty_core::shared_memory::{
    MAX_MEMORY_ENTRIES, MemoryScope, SharedMemoryBook, SharedMemoryEntry, UpsertMemoryInput, infer_project_slug, infer_remote_project_slug,
};
use mighty_core::store::{self, Store};

const P1: &str = "11111111-1111-4111-8111-111111111111";

fn log(id: &str, kind: &str, text: &str, minute: u32) -> LogEntry {
    LogEntry::new(id.into(), kind, text.into(), format!("2026-11-22T10:{minute:02}:00Z"), Some("claude".into()))
}

/// The pane the service holds after the store's last save: a new log entry, another title, a draft and a status; none of it saved.
#[test]
fn evicting_the_live_pane_keeps_what_the_store_had_not_saved_yet() {
    let dir = temp_dir("evict-live");
    let ws = Workspace {
        id: Some("w1".into()),
        name: Some("Alpha".into()),
        path: Some(if cfg!(windows) { r"C:\mighty-test\alpha".into() } else { "/mighty-test/alpha".into() }),
        created_at: Some("2026-11-22T00:00:00Z".into()),
        ..Workspace::default()
    };
    let pane = RunSession {
        id: Some(P1.into()),
        workspace_id: Some("w1".into()),
        title: Some("Saved title".into()),
        title_mode: Some("fixed".into()),
        kind: Some("claude".into()),
        provider: Some("claude".into()),
        status: Some("idle".into()),
        logs: Some(vec![log("u0", "user", "first", 0), log("a0", "assistant", "answer", 1)]),
        ..RunSession::default()
    };
    let initial = AppSnapshot {
        workspaces: Some(vec![ws]),
        sessions: Some(vec![Arc::new(pane)]),
        active_workspace_id: Some("w1".into()),
        active_session_id: Some(P1.into()),
        ..AppSnapshot::default()
    };
    fs::write(dir.join(store::STATE_FILE), initial.to_json()).unwrap();
    let mut store = Store::new(dir.clone(), None, true);
    let loaded = store.load(&Locale::new("en")).unwrap();
    store.save(&loaded).unwrap();

    let saved = store.snapshot.session(P1).unwrap().clone();
    let mut live = (*saved).clone();
    let mut logs = live.logs().to_vec();
    logs.push(log("u1", "user", "typed after the last save", 5));
    live.logs = Some(logs);
    live.title = Some("Renamed since".into());
    live.draft = Some("half a thought".into());
    live.status = Some("completed".into());

    let cold = store.evict_live_body(&Arc::new(live)).expect("evicted");
    assert!(cold.body_on_disk);
    assert_eq!(cold.title(), "Renamed since", "non-body fields come from the live pane");
    assert_eq!(cold.draft.as_deref(), Some("half a thought"));
    assert_eq!(cold.status(), "completed");
    assert_eq!(cold.log_count(), 3);
    assert!(cold.logs().iter().any(|e| e.id() == "u1"), "the digest holds the unsaved entry");

    let on_disk = store::read_cold_body_logs(&dir, P1).expect("the body file holds it");
    assert_eq!(on_disk.len(), 3);
    assert!(on_disk.iter().any(|e| e.id() == "u1"), "the unsaved log entry reached the body file");
    let restored = store.load_pane_body(&cold).expect("restored");
    assert_eq!(restored.logs().len(), 3);
    assert_eq!(restored.title(), "Renamed since");
    fs::remove_dir_all(dir).unwrap();
}

/// A session file whose `"hash":"` is followed by a multi-byte character used to panic the scan (`&after[1..]` inside a character).
#[test]
fn the_image_hash_scan_survives_multi_byte_text_and_still_finds_the_hash_after_it() {
    let dir = temp_dir("scan-multibyte");
    let sessions = dir.join(store::SESSIONS_DIR);
    let images = dir.join("image-cache");
    fs::create_dir_all(&sessions).unwrap();
    fs::create_dir_all(&images).unwrap();
    let hash = "ab".repeat(32);
    fs::write(
        sessions.join("p.json"),
        format!("{{\"a\":{{\"hash\":\"\u{00e9}\u{00e9}\"}},\"b\":{{\"hash\":\"{hash}\"}},\"c\":{{\"hash\":\"\u{d55c}\"}}}}"),
    )
    .unwrap();
    let image = images.join(format!("{hash}.png"));
    fs::write(&image, b"png").unwrap();
    fs::write(images.join(format!("{}.png", "cd".repeat(32))), b"png").unwrap();

    let found = collect_referenced_images(&dir, &images, None);
    assert_eq!(found, std::iter::once(image.clone()).collect());
    assert!(collect_snapshot_images(&images, &AppSnapshot::default()).is_empty(), "the in-memory pass reads no session file");
    fs::remove_dir_all(dir).unwrap();
}

fn upsert(scope: MemoryScope, key: &str) -> UpsertMemoryInput {
    UpsertMemoryInput {
        id: None,
        scope,
        key: key.into(),
        title: key.into(),
        content: "content".into(),
        tags: vec![],
        auto_inject: true,
        pinned: false,
    }
}

fn incoming(id: &str, scope: MemoryScope, key: &str, content: &str) -> SharedMemoryEntry {
    let mut entry = SharedMemoryEntry {
        id: id.into(),
        scope,
        key: key.into(),
        title: key.into(),
        content: content.into(),
        tags: vec![],
        auto_inject: true,
        pinned: false,
        revision: 3,
        seq: 0,
        content_hash: String::new(),
        updated_at: "2099-01-01T00:00:00Z".into(),
        updated_by_device_id: "peer".into(),
        updated_by_device_name: "Peer".into(),
        deleted: false,
    };
    entry.recompute_hash();
    entry
}

fn project(workspace: &str) -> MemoryScope {
    MemoryScope::Project { workspace_id: workspace.into(), project_slug: String::new() }
}

#[test]
fn a_scoped_apply_skips_what_the_scope_does_not_cover_and_never_evicts_a_live_local_entry() {
    let mut book = SharedMemoryBook::new("dev", "Dev");
    let (mine, _) = book.upsert(upsert(MemoryScope::User, "mine")).unwrap();
    let (theirs, _) = book.upsert(upsert(project("open"), "theirs")).unwrap();
    let only_open = |scope: &MemoryScope| matches!(scope, MemoryScope::Project { workspace_id, .. } if workspace_id == "open");

    let outcome = book.apply_incoming_scoped(
        vec![
            incoming(&mine.id, project("open"), "mine", "overwritten"),
            incoming(&theirs.id, project("open"), "theirs", "updated"),
            incoming("elsewhere", project("closed"), "z", "outside"),
        ],
        &Default::default(),
        &only_open,
    );
    assert_eq!((outcome.applied, outcome.skipped), (1, 2));
    assert_eq!(book.entries.iter().find(|e| e.id == mine.id).unwrap().content, "content", "a local entry outside the scope is untouched");
    assert_eq!(book.entries.iter().find(|e| e.id == theirs.id).unwrap().content, "updated");
    assert!(book.entries.iter().all(|e| e.id != "elsewhere"));

    // Full of live local entries: the unscoped apply skips too instead of removing the first one.
    let mut full = SharedMemoryBook::new("dev", "Dev");
    let (first, _) = full.upsert(upsert(MemoryScope::User, "first")).unwrap();
    for i in 1..MAX_MEMORY_ENTRIES {
        full.upsert(upsert(MemoryScope::User, &format!("k{i}"))).unwrap();
    }
    let outcome = full.apply_incoming(vec![incoming("new", project("open"), "new", "v")], &Default::default());
    assert_eq!((outcome.applied, outcome.skipped), (0, 1));
    assert!(full.entries.iter().any(|e| e.id == first.id) && full.entries.len() == MAX_MEMORY_ENTRIES);

    // A tombstone may make room.
    let first_id = full.entries[5].id.clone();
    full.mark_deleted(&first_id).unwrap();
    let outcome = full.apply_incoming(vec![incoming("new", project("open"), "new", "v")], &Default::default());
    assert_eq!(outcome.applied, 1);
    assert!(full.entries.iter().all(|e| e.id != first_id) && full.entries.iter().any(|e| e.id == "new"));
}

/// A remote workspace's path is the other computer's: its slug is made from the folder name only, never from a `.git/config` that
/// happens to sit at the same path on this disk.
#[test]
fn a_remote_workspaces_slug_never_reads_this_disk() {
    let dir = temp_dir("remote-slug");
    let repo = dir.join("project");
    fs::create_dir_all(repo.join(".git")).unwrap();
    fs::write(repo.join(".git").join("config"), "[remote \"origin\"]\n\turl = git@example.com:local-org/local-repo.git\n").unwrap();
    let path = repo.to_string_lossy().into_owned();
    assert_eq!(infer_project_slug(&path, "Fallback"), "local-org/local-repo");
    assert_eq!(infer_remote_project_slug(&path, "Fallback"), "project");
    assert_eq!(infer_remote_project_slug("", "Some Name"), infer_remote_project_slug("   ", "Some Name"));
    fs::remove_dir_all(dir).unwrap();
}
