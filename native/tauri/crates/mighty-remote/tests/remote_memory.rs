//! Integration tests for `/v2/memory/changes` & `/v2/memory/push` over Tailscale HTTP and E2EE Paired Relay (`MC-13`):
//! - Incremental pull/push roundtrip (`RemoteClient::memory_request` and `RemoteHost::serve_relay`)
//! - Strict scope isolation (unshared workspaces and disabled user memory rejected with `403`, excluded from `changes`)
//! - Incoming secret sanitization on `/v2/memory/push`
//! - Concurrent multi-device conflict detection & side-version preservation
//! - Automatic `StartRunRequest::memory_context` population when starting remote runs on shared workspaces

mod support;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use mighty_core::shared_memory::{
    DEFAULT_INJECT_MAX_BYTES, MemoryScope, REDACTED_SECRET, SharedMemoryBook, SharedMemoryEntry, UpsertMemoryInput, infer_project_slug,
};
use mighty_platform::fake::MemoryVault;
use mighty_remote::remote::memory::{BookMemoryHost, CHANGES_PATH, MemoryChangesWire, MemoryPushReplyWire, MemoryPushWire, PUSH_PATH};
use mighty_remote::remote::wire::protocol;
use mighty_run::model::StartRunRequest;
use mighty_run::service::RemoteRunner;
use support::fake_host::{Collected, MakeHost, client_store, client_with, loopback_options, make_host, until};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn memory_changes_and_push_roundtrip_scope_isolation_sanitization_and_relay() {
    let fixture = make_host(MakeHost::default()).await;
    let shared_ws = fixture.shared.clone();
    let private_ws = fixture.private.clone();
    let shared_slug = infer_project_slug(shared_ws.path(), shared_ws.name());

    let host_book = Arc::new(Mutex::new(SharedMemoryBook::new("host-dev", "Studio Host")));
    {
        let mut b = host_book.lock().unwrap();
        // Shared workspace entry -> must be visible to connected client.
        b.upsert(UpsertMemoryInput {
            id: None,
            scope: MemoryScope::Project { workspace_id: shared_ws.id().to_string(), project_slug: shared_slug.clone() },
            key: "db-pooling".into(),
            title: "DB Pooling".into(),
            content: "Keep max_connections <= 20 in integration tests.".into(),
            tags: vec!["db".into()],
            auto_inject: true,
            pinned: true,
        })
        .unwrap();
        // Private unshared workspace entry -> must NEVER leak in `/v2/memory/changes`!
        b.upsert(UpsertMemoryInput {
            id: None,
            scope: MemoryScope::Project { workspace_id: private_ws.id().to_string(), project_slug: "private-secret-repo".into() },
            key: "private-note".into(),
            title: "Private Note".into(),
            content: "Top secret internal roadmap.".into(),
            tags: vec![],
            auto_inject: true,
            pinned: false,
        })
        .unwrap();
        // User-scoped entry while `share_user_memory` defaults to false -> must NOT be exposed!
        b.upsert(UpsertMemoryInput {
            id: None,
            scope: MemoryScope::User,
            key: "personal-pref".into(),
            title: "Personal".into(),
            content: "Private user preference.".into(),
            tags: vec![],
            auto_inject: true,
            pinned: false,
        })
        .unwrap();
    }

    let store_for_mem = fixture.store.clone();
    let mem_host = Arc::new(BookMemoryHost::new(
        host_book.clone(),
        "Studio Host",
        Arc::new(move || store_for_mem.lock().unwrap().snapshot.workspaces().to_vec()),
    ));
    fixture.host.set_memory(Some(mem_host));

    let mem_book_for_runs = host_book.clone();
    let shared_slug_for_runs = shared_slug.clone();
    fixture.host.set_memory_for_workspace(Some(Arc::new(move |ws_id: &str| {
        mem_book_for_runs.lock().unwrap().render_for_run(ws_id, Some(&shared_slug_for_runs), DEFAULT_INJECT_MAX_BYTES)
    })));

    let state = fixture.host.start(&[shared_ws.id().to_string()], 0).await.unwrap();
    let address = format!("127.0.0.1:{}", state.port.unwrap());
    let key = state.key.clone().unwrap();

    let (c_store, c_dir) = client_store();
    let seen = Arc::new(Collected::default());
    let client = client_with(&c_store, &c_dir, Arc::new(MemoryVault::default()), &seen, loopback_options());
    let conn = client.connect("Studio", &address, &key).await.unwrap();

    // 1. Pull initial changes (`since=0`) -> ONLY the shared workspace entry is returned!
    let (status, bytes) = client.memory_request(&conn.id, "GET", &format!("{CHANGES_PATH}?since=0"), None).await.unwrap();
    assert_eq!(status, 200);
    let changes: MemoryChangesWire = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(changes.entries.len(), 1, "only shared workspace entry may be returned");
    assert_eq!(changes.entries[0].key, "db-pooling");
    let initial_seq = changes.server_seq;

    // Incremental pull at `since=initial_seq` returns 0 entries.
    let (status_inc, bytes_inc) =
        client.memory_request(&conn.id, "GET", &format!("{CHANGES_PATH}?since={initial_seq}"), None).await.unwrap();
    assert_eq!(status_inc, 200);
    let delta_empty: MemoryChangesWire = serde_json::from_slice(&bytes_inc).unwrap();
    assert!(delta_empty.entries.is_empty());

    // 2. Scope isolation on push: pushing to an unshared workspace is rejected with 403!
    let mut unshared_book = SharedMemoryBook::new("laptop-dev", "Laptop");
    let (unshared_entry, _) = unshared_book
        .upsert(UpsertMemoryInput {
            id: None,
            scope: MemoryScope::Project { workspace_id: private_ws.id().to_string(), project_slug: "private-secret-repo".into() },
            key: "intrusion".into(),
            title: "Intrusion".into(),
            content: "Should be blocked.".into(),
            tags: vec![],
            auto_inject: true,
            pinned: false,
        })
        .unwrap();
    let forbidden_push = serde_json::to_vec(&MemoryPushWire {
        protocol: protocol::VERSION,
        device_id: "laptop-dev".into(),
        device_name: "Laptop".into(),
        entries: vec![unshared_entry],
        base_hashes: BTreeMap::new(),
    })
    .unwrap();
    let (forbidden_status, _) = client.memory_request(&conn.id, "POST", PUSH_PATH, Some(forbidden_push)).await.unwrap();
    assert_eq!(forbidden_status, 403, "unshared workspace push must be rejected with 403");

    // 3. Push with embedded raw secret on shared workspace -> sanitized before landing in host book!
    let raw_unsanitized = SharedMemoryEntry {
        id: mighty_core::wire::new_id(),
        scope: MemoryScope::Project { workspace_id: shared_ws.id().to_string(), project_slug: shared_slug.clone() },
        key: "deploy-notes".into(),
        title: "Deploy Notes".into(),
        content: "Never commit sk-ant-api03-abcdefghijklmnopqrstuvwxyz987654 or AKIAIOSFODNN7EXAMPLE.".into(),
        tags: vec!["ops".into()],
        auto_inject: true,
        pinned: false,
        revision: 1,
        seq: 1,
        content_hash: String::new(),
        updated_at: "2026-11-22T12:10:00Z".into(),
        updated_by_device_id: "laptop-dev".into(),
        updated_by_device_name: "Laptop".into(),
        deleted: false,
    };
    let valid_push = serde_json::to_vec(&MemoryPushWire {
        protocol: protocol::VERSION,
        device_id: "laptop-dev".into(),
        device_name: "Laptop".into(),
        entries: vec![raw_unsanitized],
        base_hashes: BTreeMap::new(),
    })
    .unwrap();
    let (push_status, push_bytes) = client.memory_request(&conn.id, "POST", PUSH_PATH, Some(valid_push)).await.unwrap();
    assert_eq!(push_status, 200);
    let push_reply: MemoryPushReplyWire = serde_json::from_slice(&push_bytes).unwrap();
    assert_eq!(push_reply.applied, 1);
    assert_eq!(push_reply.redacted_secrets, 2);

    {
        let b = host_book.lock().unwrap();
        let stored = b.entries.iter().find(|e| e.key == "deploy-notes").unwrap();
        assert!(stored.content.contains(REDACTED_SECRET));
        assert!(!stored.content.contains("sk-ant-api03"));
        assert!(!stored.content.contains("AKIAIOSFODNN7EXAMPLE"));
    }

    // 4. Concurrent edit conflict detection over `/v2/memory/push`!
    let conflicting_entry = SharedMemoryEntry {
        id: changes.entries[0].id.clone(),
        scope: changes.entries[0].scope.clone(),
        key: "db-pooling".into(),
        title: "DB Pooling".into(),
        content: "Concurrent laptop edit: use max_connections = 8.".into(),
        tags: vec!["db".into()],
        auto_inject: true,
        pinned: true,
        revision: 2,
        seq: 2,
        content_hash: String::new(),
        updated_at: "2099-01-01T00:00:00Z".into(),
        updated_by_device_id: "laptop-dev".into(),
        updated_by_device_name: "Laptop".into(),
        deleted: false,
    };
    let conflict_push = serde_json::to_vec(&MemoryPushWire {
        protocol: protocol::VERSION,
        device_id: "laptop-dev".into(),
        device_name: "Laptop".into(),
        entries: vec![conflicting_entry],
        base_hashes: BTreeMap::from([(changes.entries[0].id.clone(), "stale-base-hash".into())]),
    })
    .unwrap();
    let (c_status, c_bytes) = client.memory_request(&conn.id, "POST", PUSH_PATH, Some(conflict_push)).await.unwrap();
    assert_eq!(c_status, 200);
    let c_reply: MemoryPushReplyWire = serde_json::from_slice(&c_bytes).unwrap();
    assert_eq!(c_reply.conflicts.len(), 1, "concurrent push with stale base hash must report conflict");

    // 5. Paired Computer Relay (`serve_relay`) carries `/v2/memory/changes` and `/v2/memory/push` when relay switch is enabled!
    fixture.host.set_relay(true);
    let relay_reply = fixture.host.serve_relay("paired-mac", "GET", &format!("{CHANGES_PATH}?since=0"), None).await;
    assert_eq!(relay_reply.status, 200);
    let relay_changes: MemoryChangesWire = serde_json::from_str(&relay_reply.body).unwrap();
    assert_eq!(relay_changes.entries.len(), 2, "db-pooling + deploy-notes returned over paired relay");

    // 6. Starting a remote run on the shared workspace automatically populates `StartRunRequest::memory_context`!
    let opened_ws = client.open_workspace(&conn.id, shared_ws.id()).unwrap();
    let run_req = StartRunRequest::new("pane-mem-1", opened_ws.id(), "claude", "check pooling");
    RemoteRunner::start(&client, run_req, opened_ws).await.unwrap();
    until(|| fixture.runner().is_some_and(|r| !r.started.lock().unwrap().is_empty()), 3_000, "remote run started").await;
    let started_runs = fixture.runner().unwrap().started.lock().unwrap().clone();
    let mem_ctx = started_runs[0].memory_context.as_deref().expect("memory_context must be injected on remote host");
    assert!(mem_ctx.contains("db-pooling"));
    assert!(mem_ctx.contains("deploy-notes"));

    let _ = std::fs::remove_dir_all(&c_dir);
    fixture.dispose().await;
}
