//! The host side of `/v2/memory/*` against a peer that is allowed one shared workspace and nothing else: it cannot name another
//! workspace's slug, overwrite or tombstone a local entry outside its share by id, or push the host's own entries out of a full book;
//! `changes` leaves out the entries of host workspaces that are not shared even when their slug matches, and pages what it returns.

use std::collections::{BTreeMap, HashMap, HashSet};

use mighty_core::shared_memory::{MAX_MEMORY_ENTRIES, MemoryScope, SharedMemoryBook, SharedMemoryEntry, UpsertMemoryInput, normalize_slug};
use mighty_remote::remote::memory::{
    MAX_CHANGES_PAGE_BYTES, MAX_PUSH_BATCH_ENTRIES, MemoryChangesWire, MemoryPushReplyWire, MemoryPushWire, handle_book_changes,
    handle_book_push, parse_limit_query, push_base_hashes, split_push_batches,
};
use mighty_remote::remote::wire::protocol;

const SHARED: &str = "ws-shared";
const PRIVATE: &str = "ws-private";

fn slugs() -> HashMap<String, String> {
    // Two host workspaces with the SAME slug: only one is shared.
    HashMap::from([(SHARED.to_string(), "acme/app".to_string()), (PRIVATE.to_string(), "acme/app".to_string())])
}

fn shared() -> HashSet<String> {
    HashSet::from([SHARED.to_string()])
}

fn project(workspace: &str, slug: &str, key: &str, content: &str) -> UpsertMemoryInput {
    UpsertMemoryInput {
        id: None,
        scope: MemoryScope::Project { workspace_id: workspace.into(), project_slug: slug.into() },
        key: key.into(),
        title: key.into(),
        content: content.into(),
        tags: vec![],
        auto_inject: true,
        pinned: false,
    }
}

fn user(key: &str, content: &str) -> UpsertMemoryInput {
    UpsertMemoryInput { scope: MemoryScope::User, ..project("", "", key, content) }
}

fn push_body(entries: Vec<SharedMemoryEntry>) -> Vec<u8> {
    serde_json::to_vec(&MemoryPushWire {
        protocol: protocol::VERSION,
        device_id: "peer".into(),
        device_name: "Peer".into(),
        entries,
        base_hashes: BTreeMap::new(),
    })
    .unwrap()
}

fn peer_entry(id: &str, scope: MemoryScope, key: &str, content: &str) -> SharedMemoryEntry {
    let mut entry = SharedMemoryEntry {
        id: id.into(),
        scope,
        key: key.into(),
        title: key.into(),
        content: content.into(),
        tags: vec![],
        auto_inject: true,
        pinned: false,
        revision: 9,
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

fn pushed(book: &mut SharedMemoryBook, entries: Vec<SharedMemoryEntry>) -> MemoryPushReplyWire {
    let reply = handle_book_push(book, &shared(), &slugs(), &push_body(entries)).expect("push accepted");
    serde_json::from_slice(&reply).unwrap()
}

#[test]
fn a_pushed_entrys_slug_is_the_hosts_own_never_another_workspaces() {
    let mut book = SharedMemoryBook::new("host", "Host");
    book.settings.share_project_memory = true;
    // The peer names the shared workspace but claims the slug of ANOTHER host workspace.
    let spoof = peer_entry("e1", MemoryScope::Project { workspace_id: SHARED.into(), project_slug: "other/secret-repo".into() }, "k", "v");
    let reply = pushed(&mut book, vec![spoof]);
    assert_eq!(reply.applied, 1);
    let stored = book.entries.iter().find(|e| e.id == "e1").unwrap();
    let MemoryScope::Project { workspace_id, project_slug } = &stored.scope else { panic!("project scope") };
    assert_eq!(workspace_id, SHARED);
    assert_eq!(normalize_slug(project_slug), "acme/app", "the host's slug for that workspace replaces the claimed one");
}

#[test]
fn a_peer_cannot_overwrite_or_tombstone_a_local_entry_outside_its_share_by_id() {
    let mut book = SharedMemoryBook::new("host", "Host");
    book.settings.share_project_memory = true;
    book.settings.share_user_memory = false;
    let (user_entry, _) = book.upsert(user("my-style", "private user convention")).unwrap();
    let (private_entry, _) = book.upsert(project(PRIVATE, "acme/app", "roadmap", "private project note")).unwrap();

    // Same ids, claiming the shared workspace's scope; one also a tombstone.
    let overwrite_user =
        peer_entry(&user_entry.id, MemoryScope::Project { workspace_id: SHARED.into(), project_slug: String::new() }, "x", "pwn");
    let mut tombstone_private =
        peer_entry(&private_entry.id, MemoryScope::Project { workspace_id: SHARED.into(), project_slug: String::new() }, "y", "");
    tombstone_private.deleted = true;
    let reply = pushed(&mut book, vec![overwrite_user, tombstone_private]);

    assert_eq!(reply.applied, 0);
    assert_eq!(reply.skipped, 2);
    let user_after = book.entries.iter().find(|e| e.id == user_entry.id).unwrap();
    assert_eq!(user_after.content, "private user convention");
    assert!(user_after.scope.is_user() && !user_after.deleted);
    let private_after = book.entries.iter().find(|e| e.id == private_entry.id).unwrap();
    assert_eq!(private_after.content, "private project note");
    assert!(!private_after.deleted);

    // The same push does land on a local entry that IS inside the share (the peer's own workspace).
    let (shared_entry, _) = book.upsert(project(SHARED, "acme/app", "conv", "old")).unwrap();
    let update =
        peer_entry(&shared_entry.id, MemoryScope::Project { workspace_id: SHARED.into(), project_slug: String::new() }, "conv", "new");
    let reply = pushed(&mut book, vec![update]);
    assert_eq!(reply.applied, 1);
    assert_eq!(book.entries.iter().find(|e| e.id == shared_entry.id).unwrap().content, "new");
}

#[test]
fn a_full_book_never_drops_the_hosts_own_entries_for_a_peers() {
    let mut book = SharedMemoryBook::new("host", "Host");
    book.settings.share_project_memory = true;
    let (oldest, _) = book.upsert(project(PRIVATE, "acme/app", "oldest-local", "keep me")).unwrap();
    for i in 1..MAX_MEMORY_ENTRIES {
        book.upsert(project(PRIVATE, "acme/app", &format!("local-{i}"), "filler")).unwrap();
    }
    assert_eq!(book.entries.len(), MAX_MEMORY_ENTRIES);

    let newcomer = peer_entry("fresh", MemoryScope::Project { workspace_id: SHARED.into(), project_slug: String::new() }, "fresh", "v");
    let reply = pushed(&mut book, vec![newcomer]);
    assert_eq!(reply.applied, 0);
    assert_eq!(reply.skipped, 1);
    assert_eq!(book.entries.len(), MAX_MEMORY_ENTRIES);
    assert!(book.entries.iter().any(|e| e.id == oldest.id), "the host's oldest local entry survives");
    assert!(book.entries.iter().all(|e| e.id != "fresh"));
}

fn changes(book: &SharedMemoryBook, since: u64, limit: usize) -> MemoryChangesWire {
    serde_json::from_slice(&handle_book_changes(book, "Host", since, limit, &shared(), &slugs()).unwrap()).unwrap()
}

#[test]
fn changes_leave_out_entries_of_unshared_workspaces_with_a_colliding_slug() {
    let mut book = SharedMemoryBook::new("host", "Host");
    book.settings.share_project_memory = true;
    book.upsert(project(SHARED, "acme/app", "visible", "shared note")).unwrap();
    book.upsert(project(PRIVATE, "acme/app", "hidden", "same slug, not shared")).unwrap();
    // An entry from another device (a workspace this host does not have) with the shared slug is still the shared project's.
    book.upsert(project("someone-elses-ws", "acme/app", "foreign", "from another device")).unwrap();

    let keys: Vec<String> = changes(&book, 0, 100).entries.into_iter().map(|e| e.key).collect();
    assert!(keys.contains(&"visible".to_string()) && keys.contains(&"foreign".to_string()), "{keys:?}");
    assert!(!keys.contains(&"hidden".to_string()), "an unshared host workspace's entry never leaves, whatever its slug: {keys:?}");
}

#[test]
fn changes_come_in_pages_that_resume_from_the_cursor_and_older_peers_still_make_progress() {
    let mut book = SharedMemoryBook::new("host", "Host");
    book.settings.share_project_memory = true;
    for i in 0..25 {
        book.upsert(project(SHARED, "acme/app", &format!("note-{i:02}"), "x")).unwrap();
    }
    let mut cursor = 0;
    let mut seen = Vec::new();
    let mut pages = 0;
    loop {
        let page = changes(&book, cursor, 10);
        pages += 1;
        assert!(page.entries.len() <= 10);
        seen.extend(page.entries.iter().map(|e| e.key.clone()));
        if !page.has_more {
            break;
        }
        assert_eq!(page.server_seq, page.entries.last().unwrap().seq, "a cut page's cursor is its last entry");
        assert!(page.server_seq > cursor);
        cursor = page.server_seq;
    }
    assert_eq!((pages, seen.len()), (3, 25));
    assert_eq!(parse_limit_query("?since=3&limit=7"), 7);
    assert_eq!(parse_limit_query("?since=3"), 200);

    // A byte-heavy set is cut by size too, and a page always holds at least one entry.
    let mut heavy = SharedMemoryBook::new("host", "Host");
    heavy.settings.share_project_memory = true;
    for i in 0..30 {
        heavy.upsert(project(SHARED, "acme/app", &format!("big-{i}"), &"y".repeat(30_000))).unwrap();
    }
    let page = changes(&heavy, 0, 500);
    assert!(page.has_more && !page.entries.is_empty());
    assert!(serde_json::to_vec(&page.entries).unwrap().len() <= MAX_CHANGES_PAGE_BYTES + 32_768);
}

#[test]
fn outgoing_entries_split_into_batches_that_each_fit_one_push() {
    let mut book = SharedMemoryBook::new("peer", "Peer");
    for i in 0..300 {
        book.upsert(project(SHARED, "acme/app", &format!("n-{i}"), "small")).unwrap();
    }
    let batches = split_push_batches(&book.entries);
    assert!(batches.len() >= 3 && batches.iter().all(|b| b.len() <= MAX_PUSH_BATCH_ENTRIES));
    assert_eq!(batches.iter().map(|b| b.len()).sum::<usize>(), 300);

    let mut heavy = SharedMemoryBook::new("peer", "Peer");
    for i in 0..40 {
        heavy.upsert(project(SHARED, "acme/app", &format!("h-{i}"), &"z".repeat(30_000))).unwrap();
    }
    for batch in split_push_batches(&heavy.entries) {
        assert!(
            serde_json::to_vec(&MemoryPushWire {
                protocol: protocol::VERSION,
                device_id: "p".into(),
                device_name: "P".into(),
                entries: batch.to_vec(),
                base_hashes: BTreeMap::new(),
            })
            .unwrap()
            .len()
                < 512 * 1024
        );
    }

    let all = BTreeMap::from([("a".to_string(), "h1".to_string()), ("b".to_string(), "h2".to_string())]);
    let mut one = heavy.entries[0].clone();
    one.id = "a".into();
    assert_eq!(push_base_hashes(&[one], Some(&all)).len(), 1, "only the batch's own hashes are sent");
}
