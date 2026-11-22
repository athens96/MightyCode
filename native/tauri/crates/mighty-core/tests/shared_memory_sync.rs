//! Integration tests for `mighty_core::shared_memory` (`MC-13`):
//! - Secret sanitization across AWS, GitHub, GitLab, Slack, Anthropic, OpenAI, Google, Stripe, PEM, Bearer, and `.env`
//! - Incremental sync (`changes_since`), 3-way fast-forward vs concurrent LWW + preserved side-version conflicts
//! - Conflict resolution (`KeepLocal`, `KeepRemote`, `Merged`)
//! - Prompt injection formatter (`render_for_run`, pinned priority, 4096-byte budget)
//! - Atomic persistence (`0600` on Unix), `remote-cache/` offline state, and `retention::sweep_profile_dir` protection

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

use mighty_core::retention::{RetentionPolicy, sweep_profile_dir};
use mighty_core::shared_memory::{
    CachedRemoteSessionDigest, CachedRemoteState, CachedRemoteWorkspace, CachedSessionLine, ConflictResolution, DEFAULT_INJECT_MAX_BYTES,
    MemoryScope, REDACTED_SECRET, SHARED_MEMORY_BACKUP_FILE, SHARED_MEMORY_FILE, SharedMemoryBook, UpsertMemoryInput, infer_project_slug,
    list_cached_remote_states, read_cached_remote_state, sanitize_secrets, sha256_hex, write_cached_remote_state,
};

fn temp_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mighty-core-mem-{label}-{}", mighty_core::wire::new_id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn sha256_matches_nist_vectors() {
    assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
}

#[test]
fn sanitize_secrets_redacts_all_supported_credential_formats_and_is_idempotent() {
    // Built at run time: a Stripe-shaped literal in the source trips secret scanners on push, though this one is made up.
    let stripe = format!("Stripe: sk_{}_{}", "live", "51H8abcdef1234567890ghijkl");
    let raw = [
        "AWS: AKIAIOSFODNN7EXAMPLE and ASIAQWERASDFZXCV1234",
        "GitHub: ghp_1234567890abcdefghij1234567890 and github_pat_11ABCDEFG0123456789_abcdefghijklmnop",
        "GitLab: glpat-abcdefghijklmnopqrstuvwx",
        "Slack: xoxb-123456789012-abcdef123456",
        "Anthropic: sk-ant-api03-abcdefghijklmnopqrstuvwxyz012345",
        "OpenAI: sk-proj-abcdefghijklmnopqrstuvwxyz012345",
        "Google: AIzaSyDaGmWKa4JsXZ-HjGw7ISLn_3namBGewQe",
        stripe.as_str(),
        "Header: Authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.payload",
        "DATABASE_PASSWORD=super-secret-db-pass",
        "export STRIPE_API_KEY=\"custom_secret_value_999\"",
        "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAABG5vbmU=\n-----END OPENSSH PRIVATE KEY-----",
    ]
    .join("\n");

    let (sanitized, count) = sanitize_secrets(&raw);
    assert_eq!(count, 14, "all 14 secrets must be redacted: {sanitized}");
    assert!(!sanitized.contains("AKIAIOSFODNN7EXAMPLE"));
    assert!(!sanitized.contains("ASIAQWERASDFZXCV1234"));
    assert!(!sanitized.contains("ghp_1234567890"));
    assert!(!sanitized.contains("github_pat_11ABCDEFG"));
    assert!(!sanitized.contains("glpat-abcdefgh"));
    assert!(!sanitized.contains("xoxb-123456"));
    assert!(!sanitized.contains("sk-ant-api03"));
    assert!(!sanitized.contains("sk-proj-abcdef"));
    assert!(!sanitized.contains("AIzaSyDaGmWKa4"));
    assert!(!sanitized.contains("sk_live_51H8"));
    assert!(!sanitized.contains("eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9"));
    assert!(!sanitized.contains("super-secret-db-pass"));
    assert!(!sanitized.contains("custom_secret_value_999"));
    assert!(!sanitized.contains("b3BlbnNzaC1rZXktdjEAAAAABG5vbmU="));
    assert!(sanitized.contains(&format!("Bearer {REDACTED_SECRET}")));
    assert!(sanitized.contains(&format!("DATABASE_PASSWORD={REDACTED_SECRET}")));

    // Re-running on already-sanitized text must report 0 redactions and leave text unchanged.
    let (again, second_count) = sanitize_secrets(&sanitized);
    assert_eq!(second_count, 0);
    assert_eq!(again, sanitized);
}

#[test]
fn incremental_sync_fast_forwards_and_records_side_versions_on_concurrent_conflict() {
    let mut mac = SharedMemoryBook::new("dev-mac", "MacBook Pro");
    let mut studio = SharedMemoryBook::new("dev-studio", "Studio PC");

    let scope_mac = MemoryScope::Project { workspace_id: "ws-mac-1".into(), project_slug: "acme/mighty-service".into() };
    let scope_studio = MemoryScope::Project { workspace_id: "ws-studio-9".into(), project_slug: "acme/mighty-service".into() };

    // 1. Mac creates initial entry and syncs to Studio.
    let (v1, redacted) = mac
        .upsert(UpsertMemoryInput {
            id: None,
            scope: scope_mac.clone(),
            key: "Architecture Rules".into(),
            title: "Architecture Rules".into(),
            content: "Always use hexagonal ports in mighty-platform.".into(),
            tags: vec!["arch".into(), "rust".into()],
            auto_inject: true,
            pinned: true,
        })
        .unwrap();
    assert_eq!(redacted, 0);
    assert_eq!(v1.key, "architecture-rules");
    assert_eq!(v1.seq, 1);

    let delta_1 = mac.changes_since(0, None, Some(&HashSet::from(["acme/mighty-service".to_string()])), false);
    assert_eq!(delta_1.entries.len(), 1);
    let apply_1 = studio.apply_incoming(delta_1.entries.clone(), &BTreeMap::new());
    assert_eq!(apply_1.applied, 1);
    assert!(apply_1.conflicts.is_empty());
    studio.record_host_sync("mac", delta_1.server_seq, &delta_1.entries);

    // 2. Fast-forward: Mac updates entry from v1 -> v2 while Studio did not touch it.
    let base_hash_v1 = v1.content_hash.clone();
    let (v2, _) = mac
        .upsert(UpsertMemoryInput {
            id: Some(v1.id.clone()),
            scope: scope_mac.clone(),
            key: "architecture-rules".into(),
            title: "Architecture Rules".into(),
            content: "Always use hexagonal ports and never spawn raw subprocesses.".into(),
            tags: vec!["arch".into()],
            auto_inject: true,
            pinned: true,
        })
        .unwrap();
    let ff_bases = BTreeMap::from([(v1.id.clone(), base_hash_v1)]);
    let ff_outcome = studio.apply_incoming(vec![v2.clone()], &ff_bases);
    assert_eq!(ff_outcome.applied, 1);
    assert!(ff_outcome.conflicts.is_empty(), "fast-forward must produce zero conflicts");
    assert_eq!(studio.entries[0].content, v2.content);

    // 3. Concurrent edit on both devices from base v2!
    let shared_base = v2.content_hash.clone();
    let (mut mac_v3, _) = mac
        .upsert(UpsertMemoryInput {
            id: Some(v1.id.clone()),
            scope: scope_mac,
            key: "architecture-rules".into(),
            title: "Architecture Rules".into(),
            content: "Mac edit: prefer BTreeMap for deterministic JSON output.".into(),
            tags: vec!["arch".into()],
            auto_inject: true,
            pinned: true,
        })
        .unwrap();
    mac_v3.updated_at = "2026-11-22T10:00:00Z".into();
    mac.entries[0] = mac_v3.clone();

    let (mut studio_v3, _) = studio
        .upsert(UpsertMemoryInput {
            id: None,
            scope: scope_studio,
            key: "architecture-rules".into(),
            title: "Architecture Rules".into(),
            content: "Studio edit: enforce 0600 file permissions on Unix.".into(),
            tags: vec!["security".into()],
            auto_inject: true,
            pinned: true,
        })
        .unwrap();
    studio_v3.updated_at = "2026-11-22T10:05:00Z".into();
    studio.entries[0] = studio_v3.clone();

    let concurrent_bases = BTreeMap::from([(v1.id.clone(), shared_base)]);
    let conflict_outcome = mac.apply_incoming(vec![studio_v3.clone()], &concurrent_bases);
    assert_eq!(conflict_outcome.applied, 1);
    assert_eq!(conflict_outcome.conflicts.len(), 1, "concurrent edit must record a conflict");
    assert_eq!(mac.conflicts.len(), 1);
    // Studio's timestamp is newer (`10:05` > `10:00`), so LWW selects Studio while preserving Mac's local version.
    assert_eq!(mac.entries[0].content, studio_v3.content);
    assert!(mac.conflicts[0].local_version.content.contains("Mac edit:"));
    assert!(mac.conflicts[0].remote_version.content.contains("Studio edit:"));

    // 4. Resolve conflict via `Merged` (also sanitizing any accidental secret in merged text).
    let conflict_id = mac.conflicts[0].id.clone();
    let seq_before = mac.server_seq;
    let resolved = mac
        .resolve_conflict(
            &conflict_id,
            ConflictResolution::Merged { content: "Use BTreeMap + 0600 permissions. Token: ghp_abcdefghijklmnopqrstuvwxyz1234".into() },
        )
        .unwrap();
    assert!(mac.conflicts.is_empty());
    assert!(resolved.seq > seq_before);
    assert!(resolved.content.contains(REDACTED_SECRET));
    assert!(!resolved.content.contains("ghp_abcdef"));
}

#[test]
fn render_for_run_prioritizes_pinned_entries_and_caps_at_4096_bytes() {
    let mut book = SharedMemoryBook::new("dev-1", "Laptop");
    book.settings.auto_inject = true;
    let project = MemoryScope::Project { workspace_id: "ws-1".into(), project_slug: "org/app".into() };

    for i in 0..30 {
        book.upsert(UpsertMemoryInput {
            id: None,
            scope: project.clone(),
            key: format!("note-{i:02}"),
            title: format!("Note {i}"),
            content: "x".repeat(260),
            tags: vec![],
            auto_inject: true,
            pinned: false,
        })
        .unwrap();
    }
    book.upsert(UpsertMemoryInput {
        id: None,
        scope: MemoryScope::User,
        key: "global-style".into(),
        title: "Global Style".into(),
        content: "Keep responses concise and test-backed.".into(),
        tags: vec!["style".into()],
        auto_inject: true,
        pinned: true,
    })
    .unwrap();

    let rendered = book.render_for_run("ws-1", Some("org/app"), DEFAULT_INJECT_MAX_BYTES).unwrap();
    assert!(rendered.len() <= DEFAULT_INJECT_MAX_BYTES, "exceeded 4096 bytes: {}", rendered.len());
    assert!(rendered.starts_with("- [user:global-style [pinned]]"), "pinned entry must appear first: {rendered}");

    book.settings.auto_inject = false;
    assert!(book.render_for_run("ws-1", Some("org/app"), DEFAULT_INJECT_MAX_BYTES).is_none());
}

#[test]
fn persistence_remote_cache_git_slug_and_retention_gc_protection_work_together() {
    let dir = temp_dir("persist");

    // Verify git remote slug inference without spawning git.
    let repo = dir.join("sample-repo");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    std::fs::write(
        repo.join(".git/config"),
        "[core]\n\trepositoryformatversion = 0\n[remote \"origin\"]\n\turl = git@github.com:MightyOrg/MightyCode.git\n",
    )
    .unwrap();
    assert_eq!(infer_project_slug(&repo.to_string_lossy(), "Fallback"), "mightyorg/mightycode");

    // Save shared-memory.json twice so `.bak` is also written, plus `remote-sharing.json`.
    let mut book = SharedMemoryBook::new("dev-a", "Host A");
    book.upsert(UpsertMemoryInput {
        id: None,
        scope: MemoryScope::User,
        key: "pref".into(),
        title: "Pref".into(),
        content: "Use strict TypeScript.".into(),
        tags: vec![],
        auto_inject: true,
        pinned: false,
    })
    .unwrap();
    book.save(&dir).unwrap();
    book.upsert(UpsertMemoryInput {
        id: None,
        scope: MemoryScope::User,
        key: "pref-2".into(),
        title: "Pref 2".into(),
        content: "Prefer CSS variables.".into(),
        tags: vec![],
        auto_inject: true,
        pinned: false,
    })
    .unwrap();
    book.save(&dir).unwrap();
    std::fs::write(dir.join("remote-sharing.json"), "{\"version\":1,\"sharing\":true}").unwrap();
    std::fs::write(dir.join("remote-sharing.json.bak"), "{\"version\":1,\"sharing\":false}").unwrap();

    // Write offline remote-cache state.
    write_cached_remote_state(
        &dir,
        CachedRemoteState {
            connection_id: "conn-studio".into(),
            host_name: "studio-box".into(),
            cached_at: "2026-11-22T12:00:00Z".into(),
            workspaces: vec![CachedRemoteWorkspace {
                id: "ws-remote-1".into(),
                name: "MightyCode".into(),
                path: "/srv/mightycode".into(),
                project_slug: "mightyorg/mightycode".into(),
                branch: Some("main".into()),
            }],
            sessions: vec![CachedRemoteSessionDigest {
                id: "pane-1".into(),
                workspace_id: "ws-remote-1".into(),
                title: "Refactor auth".into(),
                provider: "claude".into(),
                model: "sonnet".into(),
                status: "completed".into(),
                body_revision: "rev-42".into(),
                branch: Some("feat/auth".into()),
                pending: 0,
                queued: 0,
                tail: vec![CachedSessionLine { kind: "assistant".into(), text: "Auth refactor complete.".into() }],
            }],
            memory_entries: book.entries.clone(),
        },
    )
    .unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mem_mode = std::fs::metadata(dir.join(SHARED_MEMORY_FILE)).unwrap().permissions().mode() & 0o777;
        assert_eq!(mem_mode, 0o600, "shared-memory.json must be 0600 on Unix");
        let cache_file = mighty_core::shared_memory::remote_cache_state_path(&dir, "conn-studio");
        let cache_mode = std::fs::metadata(cache_file).unwrap().permissions().mode() & 0o777;
        assert_eq!(cache_mode, 0o600, "remote-cache state.json must be 0600 on Unix");
    }

    // Run profile GC sweep: protected root files and fresh remote-cache must survive intact.
    let _report = sweep_profile_dir(&dir, &RetentionPolicy::default(), &HashSet::new());
    assert!(dir.join(SHARED_MEMORY_FILE).is_file());
    assert!(dir.join(SHARED_MEMORY_BACKUP_FILE).is_file());
    assert!(dir.join("remote-sharing.json").is_file());
    assert!(dir.join("remote-sharing.json.bak").is_file());

    let loaded = SharedMemoryBook::load_or_new(&dir, "Host A");
    assert_eq!(loaded.entries.len(), 2);

    let cached = read_cached_remote_state(&dir, "conn-studio").unwrap();
    assert_eq!(cached.host_name, "studio-box");
    assert_eq!(cached.sessions[0].body_revision, "rev-42");
    assert_eq!(list_cached_remote_states(&dir).len(), 1);

    let _ = std::fs::remove_dir_all(&dir);
}
