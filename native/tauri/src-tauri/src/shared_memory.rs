//! Host management, `/v2/memory/*` wire bridging, incremental multi-device & custom HTTPS sync,
//! `<data_dir>/remote-cache/<connection_id>/` mirror snapshots & offline fallback, prompt context
//! injection, and Tauri page RPCs for Shared Memory (`MC-13`).
//!
//! Write & smoke invariants:
//! - When `!host.writes() || host.smoke.is_some()`, `SharedMemoryBook` and `remote-cache/` stay strictly
//!   in-memory (never creating or mutating files on disk during `--read-only` or `--smoke` runs).
//! - Endpoint bearer tokens live exclusively in `SecretVault` under [`ENDPOINT_TOKEN_SECRET_ID`], never in JSON.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use mighty_core::model::{AppSnapshot, Workspace};
use mighty_core::shared_memory::{
    self, CachedRemoteSessionDigest, CachedRemoteState, CachedRemoteWorkspace, ConflictResolution, DEFAULT_INJECT_MAX_BYTES, MemoryScope,
    SharedMemoryBook, SharedMemoryEntry, UpsertMemoryInput, infer_project_slug, infer_remote_project_slug, list_cached_remote_states,
    normalize_slug, read_cached_remote_state, render_entries_for_run, write_cached_remote_state,
};
use mighty_platform::HttpsRequest;
use mighty_remote::remote::memory::{
    CHANGES_PATH, DEFAULT_CHANGES_LIMIT, MemoryChangesWire, MemoryHost, MemoryPushReplyWire, MemoryPushWire, PUSH_PATH,
    handle_book_changes, handle_book_push, parse_limit_query, push_base_hashes, split_push_batches,
};
use mighty_remote::remote::mirror::desk::MirrorDesk;
use mighty_remote::remote::wire::{RemoteConnectionStatus, protocol};
use serde_json::{Value, json};
use tauri::Manager as _;

use crate::host::{Host, Registry, lock};
use crate::methods::{flag, text};

pub const ENDPOINT_TOKEN_SECRET_ID: &str = "shared-memory-endpoint-token";
const MAX_ENDPOINT_RESPONSE_BYTES: usize = 1024 * 1024;
/// Pages of a host's changes one sync reads at most (a host with more is caught up on the next sync).
const MAX_PULL_PAGES: usize = 50;

#[derive(Default)]
struct LoadedBook {
    loaded: bool,
    book: SharedMemoryBook,
}

#[derive(Default)]
pub struct SharedMemoryState {
    book: Mutex<LoadedBook>,
    last_synced_at: Mutex<Option<String>>,
    sync_lock: tokio::sync::Mutex<()>,
    epoch: AtomicU64,
    /// One `remote-cache` write in flight, and the newest request that came while it ran.
    cache_gate: Mutex<CacheGate>,
    /// Serializes the `remote-cache` read-modify-write of both writers (the mirror's and the sync's).
    cache_io: Mutex<()>,
}

#[derive(Default)]
struct CacheGate {
    running: bool,
    pending: Option<Option<MirrorDesk>>,
}

fn may_persist(host: &Host) -> bool {
    host.writes() && host.smoke.is_none()
}

fn default_device_name() -> String {
    let raw = crate::mobile::machine_name();
    if raw.trim().is_empty() { "Mighty Code".into() } else { raw.trim().to_string() }
}

fn state(host: &Host) -> Arc<SharedMemoryState> {
    if let Some(existing) = host.app.try_state::<Arc<SharedMemoryState>>() {
        return existing.inner().clone();
    }
    let created = Arc::new(SharedMemoryState::default());
    host.app.manage(created.clone());
    host.app.try_state::<Arc<SharedMemoryState>>().map(|s| s.inner().clone()).unwrap_or(created)
}

fn with_book<T>(host: &Host, run: impl FnOnce(&mut SharedMemoryBook) -> T) -> T {
    let st = state(host);
    let mut guard = lock(&st.book);
    if !guard.loaded {
        let name = default_device_name();
        guard.book = if host.smoke.is_some() {
            SharedMemoryBook::new(mighty_core::wire::new_id(), name)
        } else {
            SharedMemoryBook::load_or_new(&host.data_dir, &name)
        };
        guard.loaded = true;
    }
    run(&mut guard.book)
}

pub fn book_snapshot(host: &Host) -> SharedMemoryBook {
    with_book(host, |book| book.clone())
}

#[allow(dead_code)]
pub fn revision(host: &Host) -> u64 {
    state(host).epoch.load(Ordering::Acquire)
}

fn persist_snapshot_if_allowed(host: &Host, snapshot: &SharedMemoryBook) -> Result<(), String> {
    state(host).epoch.fetch_add(1, Ordering::Release);
    host.shell_dirty();
    if may_persist(host) {
        snapshot.save(&host.data_dir).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn current_app_snapshot(host: &Host) -> AppSnapshot {
    if let Some(runs) = host.runs() {
        runs.service.snapshot()
    } else {
        let view = host.view();
        view.snapshot.clone()
    }
}

fn local_workspace_slugs(snapshot: &AppSnapshot) -> HashMap<String, String> {
    snapshot
        .workspaces()
        .iter()
        .filter(|w| w.remote.is_none())
        .map(|w| (w.id().to_string(), infer_project_slug(w.path(), w.name())))
        .collect()
}

// ── `RemoteHost` `/v2/memory/*` & Run Injection Hooks ────────────────────────────────────────────────────────────────────

struct AppMemoryHost {
    host: Weak<Host>,
}

impl MemoryHost for AppMemoryHost {
    fn changes(&self, since: u64, shared_workspaces: &HashSet<String>, query: &str) -> Result<Vec<u8>, (u16, String)> {
        let Some(host) = self.host.upgrade().filter(|h| !h.closing()) else {
            return Err((503, "Mighty Code is closing.".into()));
        };
        let snapshot = current_app_snapshot(&host);
        let slugs = local_workspace_slugs(&snapshot);
        let host_name = default_device_name();
        with_book(&host, |book| handle_book_changes(book, &host_name, since, parse_limit_query(query), shared_workspaces, &slugs))
    }

    fn push(&self, shared_workspaces: &HashSet<String>, body: &[u8]) -> Result<Vec<u8>, (u16, String)> {
        let Some(host) = self.host.upgrade().filter(|h| !h.closing()) else {
            return Err((503, "Mighty Code is closing.".into()));
        };
        let snapshot = current_app_snapshot(&host);
        let slugs = local_workspace_slugs(&snapshot);
        let (result, to_save) = with_book(&host, |book| {
            let reply = handle_book_push(book, shared_workspaces, &slugs, body);
            let cloned = reply.is_ok().then(|| book.clone());
            (reply, cloned)
        });
        if let Some(saved) = to_save {
            let _ = persist_snapshot_if_allowed(&host, &saved);
        }
        result
    }
}

/// Returns the `MemoryHost` implementation attached to `RemoteHost`.
pub fn memory_host(host: &Arc<Host>) -> Arc<dyn MemoryHost> {
    Arc::new(AppMemoryHost { host: Arc::downgrade(host) })
}

/// Returns the `RemoteHost` callback that populates `StartRunRequest::memory_context` for remote runs (`POST /v2/runs`).
pub fn remote_workspace_context_hook(host: &Arc<Host>) -> mighty_remote::remote::memory::WorkspaceMemoryLookup {
    let weak = Arc::downgrade(host);
    Arc::new(move |workspace_id: &str| {
        let host = weak.upgrade().filter(|h| !h.closing())?;
        let snapshot = current_app_snapshot(&host);
        let ws = snapshot.workspace(workspace_id);
        context_for_pane(&host, ws)
    })
}

// ── Multi-Provider Prompt Context Lookup ─────────────────────────────────────────────────────────────────────────────────

/// Collects and formats `<shared-memory>` entries (capped to 4096 bytes) for `workspace`, seamlessly merging offline
/// `remote-cache` entries when `workspace` belongs to a remote/mirrored connection.
pub fn context_for_pane(host: &Host, workspace: Option<&Workspace>) -> Option<String> {
    let book = book_snapshot(host);
    if !book.settings.auto_inject {
        return None;
    }
    let Some(ws) = workspace else {
        return render_entries_for_run(&book.entries, "", None, DEFAULT_INJECT_MAX_BYTES);
    };

    let ws_id = ws.id().to_string();
    let remote_ws_id = ws.remote.as_ref().and_then(|r| r.workspace_id.clone()).filter(|s| !s.is_empty());
    let conn_id = ws.remote.as_ref().and_then(|r| r.connection_id.clone()).filter(|s| !s.is_empty());
    let slug = resolve_workspace_slug(host, ws);
    let merged = merged_entries_from_book_and_cache(
        &book,
        (!host.smoke.is_some()).then_some(host.data_dir.as_path()),
        &ws_id,
        remote_ws_id.as_deref(),
        (!slug.is_empty()).then_some(slug.as_str()),
        conn_id.as_deref(),
        true,
    );
    if merged.is_empty() {
        return None;
    }

    // Normalize matching project scopes to `ws_id` so `render_entries_for_run` includes both local `ws_id` and `remote_ws_id`.
    let normalized: Vec<SharedMemoryEntry> = merged
        .into_iter()
        .map(|mut entry| {
            if let MemoryScope::Project { workspace_id, project_slug } = &mut entry.scope
                && remote_ws_id.as_deref().is_some_and(|rw| rw == workspace_id)
            {
                *workspace_id = ws_id.clone();
                if project_slug.is_empty() && !slug.is_empty() {
                    *project_slug = slug.clone();
                }
            }
            entry
        })
        .collect();

    render_entries_for_run(&normalized, &ws_id, (!slug.is_empty()).then_some(slug.as_str()), DEFAULT_INJECT_MAX_BYTES)
}

pub fn resolve_workspace_slug(host: &Host, ws: &Workspace) -> String {
    // A remote workspace's path is a path on the other computer: never read as one on this disk.
    let direct =
        if ws.remote.is_some() { infer_remote_project_slug(ws.path(), ws.name()) } else { infer_project_slug(ws.path(), ws.name()) };
    if !direct.is_empty() && direct.contains('/') {
        return direct;
    }
    if host.smoke.is_none()
        && let Some(rem) = &ws.remote
        && let Some(cid) = rem.connection_id.as_deref()
        && let Some(cached) = read_cached_remote_state(&host.data_dir, cid)
    {
        let target_id = rem.workspace_id.as_deref().unwrap_or(ws.id());
        if let Some(cw) = cached.workspaces.iter().find(|c| c.id == target_id)
            && !cw.project_slug.is_empty()
        {
            return cw.project_slug.clone();
        }
    }
    direct
}

pub fn merged_entries_from_book_and_cache(
    book: &SharedMemoryBook,
    data_dir: Option<&Path>,
    workspace_id: &str,
    remote_workspace_id: Option<&str>,
    project_slug: Option<&str>,
    connection_id: Option<&str>,
    include_user: bool,
) -> Vec<SharedMemoryEntry> {
    let matches_target = |entry: &SharedMemoryEntry| -> bool {
        if entry.deleted {
            return false;
        }
        match &entry.scope {
            MemoryScope::User => include_user,
            MemoryScope::Project { .. } => {
                (!workspace_id.is_empty() && entry.scope.matches_project(workspace_id, project_slug))
                    || remote_workspace_id.is_some_and(|rw| !rw.is_empty() && entry.scope.matches_project(rw, project_slug))
                    || (workspace_id.is_empty()
                        && remote_workspace_id.is_none()
                        && project_slug.is_some_and(|s| !s.is_empty() && entry.scope.matches_project("", Some(s))))
            }
        }
    };

    let mut by_slot: HashMap<String, SharedMemoryEntry> = HashMap::new();
    for entry in book.entries.iter().filter(|e| matches_target(e)) {
        let slot = format!("{}::{}", entry.scope.scope_key(), entry.key);
        by_slot.insert(slot, entry.clone());
    }

    if let (Some(dir), Some(cid)) = (data_dir, connection_id.filter(|s| !s.is_empty()))
        && let Some(cached) = read_cached_remote_state(dir, cid)
    {
        // A remote's user entries are used only while this computer shares user memory.
        for entry in cached.memory_entries.iter().filter(|e| matches_target(e) && (!e.scope.is_user() || book.settings.share_user_memory)) {
            let slot = format!("{}::{}", entry.scope.scope_key(), entry.key);
            let replace = match by_slot.get(&slot) {
                None => true,
                Some(existing) => entry.updated_at > existing.updated_at,
            };
            if replace {
                by_slot.insert(slot, entry.clone());
            }
        }
    }

    let mut list: Vec<SharedMemoryEntry> = by_slot.into_values().collect();
    list.sort_by(|a, b| b.pinned.cmp(&a.pinned).then_with(|| b.updated_at.cmp(&a.updated_at)).then_with(|| a.key.cmp(&b.key)));
    list
}

// ── `remote-cache/<connection_id>/` Mirror & Memory Snapshot Writers ─────────────────────────────────────────────────────

/// How long the mirror's changes are let gather before the cache is written once for all of them.
const CACHE_DEBOUNCE: Duration = Duration::from_secs(2);

/// Triggered by `mirror.rs` when mirrored workspaces/sessions change (many times a second while a host streams). Never runs in
/// `--read-only` or `--smoke`. One write is in flight at a time; a request that comes meanwhile is kept (the newest wins) and written
/// once after it, from the state as it is then.
pub fn cache_mirrored_snapshot(host: &Arc<Host>, desk: Option<MirrorDesk>) {
    if !may_persist(host) || host.closing() {
        return;
    }
    let st = state(host);
    {
        let mut gate = lock(&st.cache_gate);
        if gate.running {
            gate.pending = Some(desk);
            return;
        }
        gate.running = true;
    }
    let host = host.clone();
    tauri::async_runtime::spawn(async move {
        let mut desk = desk;
        loop {
            tokio::time::sleep(CACHE_DEBOUNCE).await;
            if !host.closing() && may_persist(&host) {
                write_mirror_cache(&host, desk.take()).await;
            }
            let mut gate = lock(&st.cache_gate);
            match gate.pending.take() {
                Some(next) if !host.closing() => desk = next,
                _ => {
                    gate.running = false;
                    break;
                }
            }
        }
    });
}

/// Writes `remote-cache/<connection>/state.json` for every connection with mirrored workspaces: its workspaces (name, path, slug,
/// branch), its panes' headings and counts, and the memory entries of its workspaces. A mirrored pane's transcript belongs to the
/// host's computer and is never written here (as `Store::save` writes no mirror pane), so no pane's `tail` is kept.
async fn write_mirror_cache(host: &Arc<Host>, desk: Option<MirrorDesk>) {
    let snapshot = current_app_snapshot(host);
    let connections = crate::remote::app(host).client.connections();
    let host_names: HashMap<String, String> = connections.into_iter().map(|c| (c.id, c.name)).collect();

    let mut ws_by_conn: HashMap<String, Vec<CachedRemoteWorkspace>> = HashMap::new();
    let mut conn_names: HashMap<String, String> = host_names;

    for ws in snapshot.workspaces() {
        let Some(rem) = &ws.remote else { continue };
        let Some(cid) = rem.connection_id.as_deref().filter(|s| !s.is_empty()) else { continue };
        let remote_id = rem.workspace_id.as_deref().filter(|s| !s.is_empty()).unwrap_or(ws.id()).to_string();
        if let Some(hname) = rem.host_name.as_deref().filter(|s| !s.is_empty()) {
            conn_names.entry(cid.to_string()).or_insert_with(|| hname.to_string());
        }
        let list = ws_by_conn.entry(cid.to_string()).or_default();
        if !list.iter().any(|existing| existing.id == remote_id) {
            list.push(CachedRemoteWorkspace {
                id: remote_id,
                name: ws.name().to_string(),
                path: ws.path().to_string(),
                project_slug: infer_remote_project_slug(ws.path(), ws.name()),
                branch: None,
            });
        }
    }

    let mut sessions_by_conn: HashMap<String, Vec<CachedRemoteSessionDigest>> = HashMap::new();
    for session in snapshot.sessions() {
        if session.kind() != "claude" {
            continue;
        }
        let conn_and_ws: Option<(String, String, Option<String>)> = if let Some(m) = &session.mirror {
            let remote_ws = snapshot
                .workspace(session.workspace_id())
                .and_then(|w| w.remote.as_ref())
                .and_then(|r| r.workspace_id.clone())
                .unwrap_or_else(|| session.workspace_id().to_string());
            Some((m.connection_id.clone(), remote_ws, Some(m.host_name.clone())))
        } else if let Some(ws) = snapshot.workspace(session.workspace_id())
            && let Some(rem) = &ws.remote
            && let Some(cid) = rem.connection_id.as_deref().filter(|s| !s.is_empty())
        {
            let remote_ws = rem.workspace_id.clone().unwrap_or_else(|| ws.id().to_string());
            Some((cid.to_string(), remote_ws, rem.host_name.clone()))
        } else {
            None
        };

        let Some((cid, remote_ws_id, maybe_host_name)) = conn_and_ws else { continue };
        if let Some(hname) = maybe_host_name.filter(|s| !s.is_empty()) {
            conn_names.entry(cid.clone()).or_insert(hname);
        }
        let counts = desk.as_ref().and_then(|d| d.counts(session.id())).unwrap_or_default();
        if let Some(branch) = &counts.branch
            && let Some(ws_list) = ws_by_conn.get_mut(&cid)
            && let Some(cw) = ws_list.iter_mut().find(|w| w.id == remote_ws_id && w.branch.is_none())
        {
            cw.branch = Some(branch.clone());
        }

        sessions_by_conn.entry(cid).or_default().push(CachedRemoteSessionDigest {
            id: session.id().to_string(),
            workspace_id: remote_ws_id,
            title: session.title().to_string(),
            provider: session.provider().to_string(),
            model: session.model().to_string(),
            status: session.status().to_string(),
            body_revision: session.body_revision.unwrap_or_default().to_string(),
            branch: counts.branch,
            pending: counts.pending,
            queued: counts.queued,
            tail: Vec::new(),
        });
    }

    if ws_by_conn.is_empty() && sessions_by_conn.is_empty() {
        return;
    }

    let book = book_snapshot(host);
    let data_dir = host.data_dir.clone();
    let all_cids: HashSet<String> = ws_by_conn.keys().chain(sessions_by_conn.keys()).cloned().collect();
    let st = state(host);

    let _ = tauri::async_runtime::spawn_blocking(move || {
        let _io = lock(&st.cache_io);
        for cid in all_cids {
            let prior = read_cached_remote_state(&data_dir, &cid);
            let workspaces = ws_by_conn
                .remove(&cid)
                .filter(|v| !v.is_empty())
                .or_else(|| prior.as_ref().map(|p| p.workspaces.clone()))
                .unwrap_or_default();
            let sessions = sessions_by_conn
                .remove(&cid)
                .filter(|v| !v.is_empty())
                .or_else(|| {
                    prior.as_ref().map(|p| {
                        p.sessions.iter().cloned().map(|session| CachedRemoteSessionDigest { tail: Vec::new(), ..session }).collect()
                    })
                })
                .unwrap_or_default();
            let memory_entries = merge_cache_memory_entries(prior.as_ref(), &book, &workspaces);
            let host_name =
                conn_names.get(&cid).cloned().or_else(|| prior.as_ref().map(|p| p.host_name.clone())).unwrap_or_else(|| cid.clone());
            let cached = CachedRemoteState {
                connection_id: cid,
                host_name,
                cached_at: mighty_core::time::now_o(),
                workspaces,
                sessions,
                memory_entries,
            };
            let _ = write_cached_remote_state(&data_dir, cached);
        }
    })
    .await;
}

fn merge_cache_memory_entries(
    prior: Option<&CachedRemoteState>,
    book: &SharedMemoryBook,
    workspaces: &[CachedRemoteWorkspace],
) -> Vec<SharedMemoryEntry> {
    let ws_ids: HashSet<&str> = workspaces.iter().map(|w| w.id.as_str()).collect();
    let slugs: HashSet<String> = workspaces.iter().map(|w| normalize_slug(&w.project_slug)).filter(|s| !s.is_empty()).collect();

    let mut map: HashMap<String, SharedMemoryEntry> = HashMap::new();
    if let Some(p) = prior {
        for entry in &p.memory_entries {
            if !entry.deleted && (!entry.scope.is_user() || book.settings.share_user_memory) {
                map.insert(entry.id.clone(), entry.clone());
            }
        }
    }
    for entry in &book.entries {
        let relevant = match &entry.scope {
            MemoryScope::User => book.settings.share_user_memory,
            MemoryScope::Project { workspace_id, project_slug } => {
                ws_ids.contains(workspace_id.as_str()) || (!project_slug.is_empty() && slugs.contains(&normalize_slug(project_slug)))
            }
        };
        if relevant {
            if entry.deleted {
                map.remove(&entry.id);
            } else {
                map.insert(entry.id.clone(), entry.clone());
            }
        }
    }
    let mut entries: Vec<SharedMemoryEntry> = map.into_values().collect();
    entries.sort_by(|a, b| b.pinned.cmp(&a.pinned).then_with(|| b.updated_at.cmp(&a.updated_at)));
    entries
}

// ── Incremental Sync Engine (`RemoteClient` + Custom HTTPS Endpoint) ─────────────────────────────────────────────────────

/// Schedules a non-blocking incremental sync when remote/relay connections come online. No-op in `--smoke`.
pub fn on_connections_changed(host: &Arc<Host>) {
    if host.smoke.is_some() || host.closing() {
        return;
    }
    let has_live = crate::remote::app_if_made()
        .is_some_and(|app| app.client.connections().iter().any(|c| c.status == RemoteConnectionStatus::Connected));
    if !has_live {
        return;
    }
    let host = host.clone();
    tauri::async_runtime::spawn(async move {
        let _ = sync_all(&host).await;
    });
}

fn on_local_memory_mutated(host: &Arc<Host>) {
    if host.smoke.is_some() || host.closing() {
        return;
    }
    let has_targets = crate::remote::app_if_made()
        .is_some_and(|app| app.client.connections().iter().any(|c| c.status == RemoteConnectionStatus::Connected))
        || with_book(host, |b| b.settings.custom_endpoint_url.as_deref().is_some_and(|u| !u.trim().is_empty()));
    if !has_targets {
        return;
    }
    let host = host.clone();
    tauri::async_runtime::spawn(async move {
        let _ = sync_all(&host).await;
    });
}

/// Runs an incremental pull (`GET /v2/memory/changes?since=<seq>`) + push (`POST /v2/memory/push`) across all connected
/// `RemoteClient` hosts and optional custom HTTPS endpoint, updating `remote-cache` on success.
pub async fn sync_all(host: &Arc<Host>) -> Result<(), String> {
    if host.closing() {
        return Ok(());
    }
    let st = state(host);
    let _guard = st.sync_lock.lock().await;

    let remote_app = crate::remote::app(host);
    let connections = remote_app.client.connections();
    let app_snapshot = current_app_snapshot(host);

    for conn in &connections {
        if conn.status != RemoteConnectionStatus::Connected {
            continue;
        }
        let _ = sync_single_connection(host, &remote_app.client, &conn.id, &conn.name, &app_snapshot).await;
    }

    if host.smoke.is_none() {
        let endpoint = with_book(host, |b| b.settings.custom_endpoint_url.clone()).filter(|u| !u.trim().is_empty());
        if let Some(url) = endpoint {
            sync_custom_endpoint(host, &url).await?;
        }
    }

    let now = mighty_core::time::now_o();
    {
        let mut synced = lock(&st.last_synced_at);
        *synced = Some(now);
    }
    let latest_book = book_snapshot(host);
    persist_snapshot_if_allowed(host, &latest_book)?;
    Ok(())
}

/// The workspaces one connection's host has, as this computer knows them: their ids (the host's own), and each one's slug, derived
/// from its name and the last folder of its path only, never by reading that path on this disk (it is the other computer's).
struct ConnectionScope {
    ids: HashSet<String>,
    id_to_slug: HashMap<String, String>,
    share_user: bool,
}

impl ConnectionScope {
    fn slugs(&self) -> HashSet<String> {
        self.id_to_slug.values().map(|s| normalize_slug(s)).filter(|s| !s.is_empty()).collect()
    }

    /// An entry as this connection may give it to the book: None for what is not of one of its workspaces (or a user entry while user
    /// memory is not shared); a project entry's slug is the one this computer knows for that workspace unless the host's names the same
    /// project (the host's may be longer, `org/repo` for `repo`), so a host cannot aim an entry at another project by its slug.
    fn admit(&self, mut entry: SharedMemoryEntry) -> Option<SharedMemoryEntry> {
        match &mut entry.scope {
            MemoryScope::User => self.share_user.then_some(entry),
            MemoryScope::Project { workspace_id, project_slug } => {
                if !self.ids.contains(workspace_id.as_str()) {
                    return None;
                }
                let known = self.id_to_slug.get(workspace_id.as_str()).map(|s| normalize_slug(s)).unwrap_or_default();
                let claimed = normalize_slug(project_slug);
                let tail = |slug: &str| slug.rsplit('/').next().unwrap_or_default().to_string();
                if claimed.is_empty() || known.is_empty() || tail(&claimed) != tail(&known) {
                    *project_slug = known;
                }
                Some(entry)
            }
        }
    }

    /// Whether a local entry of `scope` is one this connection may overwrite: a user entry while user memory is shared, a project of
    /// its workspaces or of a project with the same slug.
    fn covers(&self, scope: &MemoryScope, slugs: &HashSet<String>) -> bool {
        match scope {
            MemoryScope::User => self.share_user,
            MemoryScope::Project { workspace_id, project_slug } => {
                self.ids.contains(workspace_id.as_str()) || {
                    let slug = normalize_slug(project_slug);
                    !slug.is_empty() && slugs.contains(&slug)
                }
            }
        }
    }
}

fn connection_scope(host: &Host, snapshot: &AppSnapshot, connection_id: &str, share_user: bool) -> ConnectionScope {
    let mut scope = ConnectionScope { ids: HashSet::new(), id_to_slug: HashMap::new(), share_user };

    for ws in snapshot.workspaces() {
        if let Some(rem) = &ws.remote
            && rem.connection_id.as_deref() == Some(connection_id)
        {
            let remote_id = rem.workspace_id.as_deref().filter(|s| !s.is_empty()).unwrap_or(ws.id()).to_string();
            scope.id_to_slug.insert(remote_id.clone(), infer_remote_project_slug(ws.path(), ws.name()));
            scope.ids.insert(remote_id);
        }
    }

    if host.smoke.is_none()
        && let Some(cached) = read_cached_remote_state(&host.data_dir, connection_id)
    {
        for cw in cached.workspaces {
            scope.id_to_slug.entry(cw.id.clone()).or_insert_with(|| normalize_slug(&cw.project_slug));
            scope.ids.insert(cw.id);
        }
    }

    scope
}

/// `slug -> a workspace id` of one connection, for the outgoing entries that name the project by its slug.
fn slug_to_remote_ws(scope: &ConnectionScope) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for (id, slug) in &scope.id_to_slug {
        let norm = normalize_slug(slug);
        if !norm.is_empty() {
            map.entry(norm).or_insert_with(|| id.clone());
        }
    }
    map
}

fn prepare_outgoing_for_host(
    book: &SharedMemoryBook,
    host_key: &str,
    allowed_remote_ws: Option<&HashSet<String>>,
    slug_to_remote_ws: Option<&HashMap<String, String>>,
) -> (u64, BTreeMap<String, String>, Vec<SharedMemoryEntry>, String, String) {
    let since_seq = book.host_cursors.get(host_key).copied().unwrap_or(0);
    let base_hashes = book.host_base_hashes.get(host_key).cloned().unwrap_or_default();
    let mut outgoing = Vec::new();

    for entry in &book.entries {
        if base_hashes.get(&entry.id).is_some_and(|h| h == &entry.content_hash) {
            continue;
        }
        match &entry.scope {
            MemoryScope::User => {
                if book.settings.share_user_memory {
                    outgoing.push(entry.clone());
                }
            }
            MemoryScope::Project { workspace_id, project_slug } => {
                if !book.settings.share_project_memory {
                    continue;
                }
                if let Some(allowed) = allowed_remote_ws {
                    let norm_slug = normalize_slug(project_slug);
                    let direct = allowed.contains(workspace_id.as_str());
                    let by_slug = (!norm_slug.is_empty()).then(|| slug_to_remote_ws.and_then(|m| m.get(&norm_slug))).flatten();
                    let stripped = workspace_id
                        .strip_prefix("mirror:")
                        .and_then(|rest| rest.split_once(':'))
                        .map(|(_, host_ws)| host_ws)
                        .filter(|host_ws| allowed.contains(*host_ws));
                    if direct {
                        outgoing.push(entry.clone());
                    } else if let Some(mapped_ws) = by_slug.map(String::as_str).or(stripped) {
                        let mut adapted = entry.clone();
                        adapted.scope = MemoryScope::Project { workspace_id: mapped_ws.to_string(), project_slug: project_slug.clone() };
                        outgoing.push(adapted);
                    }
                } else {
                    outgoing.push(entry.clone());
                }
            }
        }
    }

    (since_seq, base_hashes, outgoing, book.device_id.clone(), book.device_name.clone())
}

/// Applies one page of pulled entries to the book (only what `admit` lets through, never over a local entry `covers` does not name)
/// and records the page's cursor; returns the entries that went in.
fn apply_pulled_page(
    host: &Host,
    host_key: &str,
    base_hashes: &BTreeMap<String, String>,
    pulled: MemoryChangesWire,
    admit: &dyn Fn(SharedMemoryEntry) -> Option<SharedMemoryEntry>,
    covers: &dyn Fn(&MemoryScope) -> bool,
) -> Vec<SharedMemoryEntry> {
    let accepted: Vec<SharedMemoryEntry> = pulled.entries.into_iter().filter_map(admit).collect();
    with_book(host, |book| {
        let _ = book.apply_incoming_scoped(accepted.clone(), base_hashes, covers);
        book.record_host_sync(host_key, pulled.server_seq, &accepted);
    });
    accepted
}

/// Pushes `outgoing` to a host in batches under the host's request and entry limits, recording each batch the host took (so the next
/// sync resumes after it). `send` posts one body and gives the reply when the host took it.
async fn push_in_batches<F, Fut>(host: &Host, host_key: &str, outgoing: &[SharedMemoryEntry], device: (String, String), mut send: F)
where
    F: FnMut(Vec<u8>) -> Fut,
    Fut: std::future::Future<Output = Option<MemoryPushReplyWire>>,
{
    for batch in split_push_batches(outgoing) {
        let base_hashes = with_book(host, |book| push_base_hashes(batch, book.host_base_hashes.get(host_key)));
        let wire = MemoryPushWire {
            protocol: protocol::VERSION,
            device_id: device.0.clone(),
            device_name: device.1.clone(),
            entries: batch.to_vec(),
            base_hashes,
        };
        let Ok(payload) = serde_json::to_vec(&wire) else { return };
        let Some(reply) = send(payload).await else { return };
        with_book(host, |book| {
            let cursor = book.host_cursors.get(host_key).copied().unwrap_or(0);
            book.record_host_sync(host_key, cursor, batch);
            merge_remote_conflicts(book, reply.conflicts);
        });
    }
}

async fn sync_single_connection(
    host: &Arc<Host>,
    client: &mighty_remote::remote::client::RemoteClient,
    connection_id: &str,
    connection_name: &str,
    snapshot: &AppSnapshot,
) -> Result<(), String> {
    let host_key = format!("conn:{connection_id}");
    let share_user = with_book(host, |book| book.settings.share_user_memory);
    let scope = connection_scope(host, snapshot, connection_id, share_user);
    let (remote_ws_ids, slug_to_ws) = (scope.ids.clone(), slug_to_remote_ws(&scope));
    let (since_seq, base_hashes, outgoing, device_id, device_name) =
        with_book(host, |book| prepare_outgoing_for_host(book, &host_key, Some(&remote_ws_ids), Some(&slug_to_ws)));
    let slugs = scope.slugs();
    let admit = |entry: SharedMemoryEntry| scope.admit(entry);
    let covers = |memory_scope: &MemoryScope| scope.covers(memory_scope, &slugs);

    let mut cursor = since_seq;
    let mut accepted: Vec<SharedMemoryEntry> = Vec::new();
    let mut host_name = String::new();
    for _ in 0..MAX_PULL_PAGES {
        let changes_path = format!("{CHANGES_PATH}?since={cursor}&limit={DEFAULT_CHANGES_LIMIT}");
        let (status, body) = client.memory_request(connection_id, "GET", &changes_path, None).await?;
        if status != 200 {
            return Err(format!("Remote memory changes returned HTTP {status}."));
        }
        let pulled: MemoryChangesWire = serde_json::from_slice(&body).map_err(|e| format!("Invalid remote memory delta: {e}"))?;
        let (next, more) = (pulled.server_seq, pulled.has_more);
        host_name = pulled.host_name.clone();
        accepted.extend(apply_pulled_page(host, &host_key, &base_hashes, pulled, &admit, &covers));
        if !more || next <= cursor {
            break;
        }
        cursor = next;
    }

    push_in_batches(host, &host_key, &outgoing, (device_id, device_name), |payload| async move {
        let (status, reply) = client.memory_request(connection_id, "POST", PUSH_PATH, Some(payload)).await.ok()?;
        (status == 200).then(|| serde_json::from_slice::<MemoryPushReplyWire>(&reply).ok()).flatten()
    })
    .await;

    if may_persist(host) {
        let book = book_snapshot(host);
        let data_dir = host.data_dir.clone();
        let cid = connection_id.to_string();
        let cname = if host_name.trim().is_empty() { connection_name.to_string() } else { host_name };
        let st = state(host);
        let _ = tauri::async_runtime::spawn_blocking(move || {
            let _io = lock(&st.cache_io);
            let prior = read_cached_remote_state(&data_dir, &cid);
            let workspaces = prior.as_ref().map(|p| p.workspaces.clone()).unwrap_or_default();
            let sessions = prior.as_ref().map(|p| p.sessions.clone()).unwrap_or_default();
            let mut memory_entries = merge_cache_memory_entries(prior.as_ref(), &book, &workspaces);
            for entry in &accepted {
                if entry.deleted {
                    memory_entries.retain(|e| e.id != entry.id);
                } else if let Some(slot) = memory_entries.iter_mut().find(|e| e.id == entry.id) {
                    *slot = entry.clone();
                } else {
                    memory_entries.push(entry.clone());
                }
            }
            let cached = CachedRemoteState {
                connection_id: cid,
                host_name: cname,
                cached_at: mighty_core::time::now_o(),
                workspaces,
                sessions,
                memory_entries,
            };
            let _ = write_cached_remote_state(&data_dir, cached);
        })
        .await;
    }

    Ok(())
}

async fn sync_custom_endpoint(host: &Arc<Host>, raw_url: &str) -> Result<(), String> {
    let base = validate_https_endpoint_url(raw_url)?;
    let Some(ports) = host.app.try_state::<crate::platform::Ports>() else {
        return Ok(());
    };
    let https = ports.https.clone();
    let token = crate::remote::secret_vault(host).load(ENDPOINT_TOKEN_SECRET_ID);

    let host_key = format!("endpoint:{base}");
    let (since_seq, base_hashes, outgoing, device_id, device_name) =
        with_book(host, |book| prepare_outgoing_for_host(book, &host_key, None, None));
    // The endpoint is the person's own: any project, user entries only while user memory is shared.
    let (share_user, share_project) = with_book(host, |book| (book.settings.share_user_memory, book.settings.share_project_memory));
    let covers = |scope: &MemoryScope| if scope.is_user() { share_user } else { share_project };
    let admit = |entry: SharedMemoryEntry| covers(&entry.scope).then_some(entry);

    let mut cursor = since_seq;
    for _ in 0..MAX_PULL_PAGES {
        let mut get_headers = vec![("Accept".to_string(), "application/json".to_string())];
        if let Some(tok) = token.as_deref().filter(|t| !t.trim().is_empty()) {
            get_headers.push(("Authorization".to_string(), format!("Bearer {}", tok.trim())));
        }
        let get_req = HttpsRequest {
            url: format!("{base}{CHANGES_PATH}?since={cursor}&limit={DEFAULT_CHANGES_LIMIT}"),
            headers: get_headers,
            timeout: Duration::from_secs(10),
        };
        let get_res = https.get(get_req, MAX_ENDPOINT_RESPONSE_BYTES).await.map_err(|e| e.to_string())?;
        if !(200..300).contains(&get_res.status) {
            return Err(format!("Endpoint memory sync returned HTTP {}.", get_res.status));
        }
        let pulled: MemoryChangesWire = serde_json::from_str(&get_res.body).map_err(|e| format!("Invalid endpoint memory delta: {e}"))?;
        let (next, more) = (pulled.server_seq, pulled.has_more);
        let _ = apply_pulled_page(host, &host_key, &base_hashes, pulled, &admit, &covers);
        if !more || next <= cursor {
            break;
        }
        cursor = next;
    }

    let mut post_headers =
        vec![("Content-Type".to_string(), "application/json".to_string()), ("Accept".to_string(), "application/json".to_string())];
    if let Some(tok) = token.as_deref().filter(|t| !t.trim().is_empty()) {
        post_headers.push(("Authorization".to_string(), format!("Bearer {}", tok.trim())));
    }
    let post_req = HttpsRequest { url: format!("{base}{PUSH_PATH}"), headers: post_headers, timeout: Duration::from_secs(10) };
    push_in_batches(host, &host_key, &outgoing, (device_id, device_name), |payload| {
        let (https, post_req) = (https.clone(), post_req.clone());
        async move {
            let response = match https.post_json(post_req.clone(), payload.clone(), MAX_ENDPOINT_RESPONSE_BYTES).await {
                Ok((resp, _)) => resp,
                Err(mighty_platform::PlatformError::Unsupported { .. }) => {
                    https.post(post_req, payload, MAX_ENDPOINT_RESPONSE_BYTES).await.ok()?
                }
                Err(_) => return None,
            };
            if !(200..300).contains(&response.status) {
                return None;
            }
            // A reply that is not the host's own shape still means the batch was taken.
            Some(serde_json::from_str::<MemoryPushReplyWire>(&response.body).unwrap_or(MemoryPushReplyWire {
                protocol: protocol::VERSION,
                accepted: true,
                applied: 0,
                skipped: 0,
                redacted_secrets: 0,
                conflicts: Vec::new(),
                server_seq: 0,
            }))
        }
    })
    .await;

    Ok(())
}

fn merge_remote_conflicts(book: &mut SharedMemoryBook, incoming: Vec<shared_memory::MemoryConflict>) {
    for conflict in incoming {
        if !book.conflicts.iter().any(|c| c.entry_id == conflict.entry_id) {
            if book.conflicts.len() >= shared_memory::MAX_CONFLICTS {
                book.conflicts.remove(0);
            }
            book.conflicts.push(conflict);
        }
    }
}

fn validate_https_endpoint_url(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim().trim_end_matches('/');
    if !trimmed.starts_with("https://") || trimmed.len() <= "https://".len() {
        return Err("Custom shared memory endpoint URL must use https://.".into());
    }
    let rest = &trimmed["https://".len()..];
    if rest.chars().any(char::is_whitespace) {
        return Err("Custom shared memory endpoint URL must not contain whitespace.".into());
    }
    Ok(trimmed.to_string())
}

// ── UI State Projection & Tauri IPC Registration ─────────────────────────────────────────────────────────────────────────

fn build_state_response(host: &Arc<Host>, workspace_id_arg: Option<&str>) -> Value {
    let snapshot = current_app_snapshot(host);
    let chosen_ws_id = workspace_id_arg.filter(|s| !s.trim().is_empty()).or(snapshot.active_workspace_id.as_deref()).map(str::to_string);
    let ws = chosen_ws_id.as_deref().and_then(|id| snapshot.workspace(id));
    let project_slug = ws.map(|w| resolve_workspace_slug(host, w)).filter(|s| !s.is_empty());
    let remote_ws_id = ws.and_then(|w| w.remote.as_ref()).and_then(|r| r.workspace_id.clone());
    let conn_id = ws.and_then(|w| w.remote.as_ref()).and_then(|r| r.connection_id.clone());

    let book = book_snapshot(host);
    let cache_dir = host.smoke.is_none().then_some(host.data_dir.as_path());

    let project_entries = if let Some(ws_id) = chosen_ws_id.as_deref() {
        merged_entries_from_book_and_cache(
            &book,
            cache_dir,
            ws_id,
            remote_ws_id.as_deref(),
            project_slug.as_deref(),
            conn_id.as_deref(),
            false,
        )
    } else {
        Vec::new()
    };

    let mut user_entries: Vec<SharedMemoryEntry> = book.entries.iter().filter(|e| !e.deleted && e.scope.is_user()).cloned().collect();
    user_entries.sort_by(|a, b| b.pinned.cmp(&a.pinned).then_with(|| b.updated_at.cmp(&a.updated_at)).then_with(|| a.key.cmp(&b.key)));

    let connections = crate::remote::app(host).client.connections();
    let cached_states: HashMap<String, CachedRemoteState> = if host.smoke.is_none() {
        list_cached_remote_states(&host.data_dir).into_iter().map(|c| (c.connection_id.clone(), c)).collect()
    } else {
        HashMap::new()
    };

    let mut host_statuses = Vec::new();
    let mut seen_cids = HashSet::new();
    for conn in &connections {
        seen_cids.insert(conn.id.clone());
        let cached = cached_states.get(&conn.id);
        let host_key = format!("conn:{}", conn.id);
        let cursor_seq = book.host_cursors.get(&host_key).copied().unwrap_or(0);
        host_statuses.push(json!({
            "connectionId": conn.id,
            "name": conn.name,
            "online": conn.status == RemoteConnectionStatus::Connected,
            "cached": cached.is_some(),
            "cachedAt": cached.map(|c| c.cached_at.clone()),
            "cursorSeq": cursor_seq,
        }));
    }
    for (cid, cached) in &cached_states {
        if seen_cids.insert(cid.clone()) {
            let host_key = format!("conn:{cid}");
            let cursor_seq = book.host_cursors.get(&host_key).copied().unwrap_or(0);
            host_statuses.push(json!({
                "connectionId": cid,
                "name": cached.host_name,
                "online": false,
                "cached": true,
                "cachedAt": cached.cached_at,
                "cursorSeq": cursor_seq,
            }));
        }
    }

    let connected_hosts = connections.iter().filter(|c| c.status == RemoteConnectionStatus::Connected).count();
    let total_hosts = host_statuses.len();
    let cached_hosts = cached_states.len();
    let has_endpoint = book.settings.custom_endpoint_url.as_deref().is_some_and(|u| !u.trim().is_empty());
    let mode = if connected_hosts > 0 || has_endpoint {
        "online"
    } else if total_hosts > 0 {
        "offline"
    } else {
        "localOnly"
    };

    let st = state(host);
    let last_synced_at = lock(&st.last_synced_at).clone();
    let has_token = crate::remote::secret_vault(host).load(ENDPOINT_TOKEN_SECRET_ID).is_some_and(|t| !t.trim().is_empty());

    json!({
        "workspaceId": chosen_ws_id,
        "projectSlug": project_slug,
        "deviceId": book.device_id,
        "deviceName": book.device_name,
        "serverSeq": book.server_seq,
        "projectEntries": project_entries,
        "userEntries": user_entries,
        "conflicts": book.conflicts,
        "settings": {
            "shareProjectMemory": book.settings.share_project_memory,
            "shareUserMemory": book.settings.share_user_memory,
            "autoInject": book.settings.auto_inject,
            "customEndpointUrl": book.settings.custom_endpoint_url,
            "hasEndpointToken": has_token,
        },
        "syncStatus": {
            "mode": mode,
            "connectedHosts": connected_hosts,
            "totalHosts": total_hosts,
            "cachedHosts": cached_hosts,
            "lastSyncedAt": last_synced_at,
            "hosts": host_statuses,
        }
    })
}

fn parse_tags_arg(args: &Value) -> Vec<String> {
    if let Some(arr) = args.get("tags").and_then(Value::as_array) {
        return arr.iter().filter_map(Value::as_str).map(str::to_string).collect();
    }
    if let Some(raw) = text(args, "tags") {
        return raw.split(',').map(str::to_string).collect();
    }
    Vec::new()
}

fn parse_conflict_resolution(args: &Value) -> Result<ConflictResolution, String> {
    if let Some(choice) = text(args, "resolution") {
        return match choice {
            "keepLocal" => Ok(ConflictResolution::KeepLocal),
            "keepRemote" => Ok(ConflictResolution::KeepRemote),
            "merged" => {
                let content = text(args, "content").unwrap_or("").to_string();
                Ok(ConflictResolution::Merged { content })
            }
            other => Err(format!("Unsupported conflict resolution: {other}")),
        };
    }
    if let Some(obj) = args.get("resolution") {
        return serde_json::from_value::<ConflictResolution>(obj.clone()).map_err(|e| format!("Invalid conflict resolution payload: {e}"));
    }
    Err("Missing conflict resolution.".into())
}

pub fn register(methods: &mut Registry) {
    methods.add("sharedMemoryState", |host: Arc<Host>, args| async move {
        let ws_id = text(&args, "workspaceId").map(str::to_string);
        Ok(build_state_response(&host, ws_id.as_deref()))
    });

    methods.add("sharedMemoryUpsert", |host: Arc<Host>, args| async move {
        let ws_id = text(&args, "workspaceId").map(str::to_string);
        let scope_kind = text(&args, "scope").unwrap_or("project");
        let snapshot = current_app_snapshot(&host);
        let scope = if scope_kind.eq_ignore_ascii_case("user") {
            MemoryScope::User
        } else {
            let effective_ws_id = ws_id
                .clone()
                .filter(|s| !s.trim().is_empty())
                .or_else(|| snapshot.active_workspace_id.clone())
                .ok_or_else(|| "Select a workspace before saving project-scoped memory.".to_string())?;
            let slug = snapshot.workspace(&effective_ws_id).map(|w| resolve_workspace_slug(&host, w)).unwrap_or_default();
            MemoryScope::Project { workspace_id: effective_ws_id, project_slug: slug }
        };

        let id = text(&args, "id").map(str::to_string);
        let key = text(&args, "key").unwrap_or("").to_string();
        let title = text(&args, "title").unwrap_or("").to_string();
        let content = text(&args, "content").unwrap_or("").to_string();
        let tags = parse_tags_arg(&args);
        let auto_inject = args.get("autoInject").and_then(Value::as_bool).unwrap_or(true);
        let pinned = flag(&args, "pinned");

        let ((entry, redacted_count), book_after) = with_book(&host, |book| {
            let saved = book.upsert(UpsertMemoryInput { id, scope, key, title, content, tags, auto_inject, pinned })?;
            Ok::<_, String>((saved, book.clone()))
        })?;

        persist_snapshot_if_allowed(&host, &book_after)?;
        on_local_memory_mutated(&host);
        let state_json = build_state_response(&host, ws_id.as_deref());
        Ok(json!({
            "entry": entry,
            "redactedCount": redacted_count,
            "state": state_json,
        }))
    });

    methods.add("sharedMemoryDelete", |host: Arc<Host>, args| async move {
        let ws_id = text(&args, "workspaceId").map(str::to_string);
        let Some(entry_id) = text(&args, "id").map(str::to_string) else {
            return Err("Missing shared memory entry id.".into());
        };
        let (deleted, book_after) = with_book(&host, |book| {
            let removed = book.mark_deleted(&entry_id).is_some();
            (removed, book.clone())
        });
        if deleted {
            persist_snapshot_if_allowed(&host, &book_after)?;
            on_local_memory_mutated(&host);
        }
        let state_json = build_state_response(&host, ws_id.as_deref());
        Ok(json!({
            "ok": deleted,
            "state": state_json,
        }))
    });

    methods.add("sharedMemoryResolveConflict", |host: Arc<Host>, args| async move {
        let ws_id = text(&args, "workspaceId").map(str::to_string);
        let Some(conflict_id) = text(&args, "conflictId").map(str::to_string) else {
            return Err("Missing conflictId.".into());
        };
        let resolution = parse_conflict_resolution(&args)?;
        let (resolved, book_after) = with_book(&host, |book| {
            let entry = book.resolve_conflict(&conflict_id, resolution)?;
            Ok::<_, String>((entry, book.clone()))
        })?;
        persist_snapshot_if_allowed(&host, &book_after)?;
        on_local_memory_mutated(&host);
        let state_json = build_state_response(&host, ws_id.as_deref());
        Ok(json!({
            "entry": resolved,
            "state": state_json,
        }))
    });

    methods.add("sharedMemorySync", |host: Arc<Host>, args| async move {
        let ws_id = text(&args, "workspaceId").map(str::to_string);
        sync_all(&host).await?;
        let state_json = build_state_response(&host, ws_id.as_deref());
        Ok(json!({ "state": state_json }))
    });

    methods.add("sharedMemoryConfigure", |host: Arc<Host>, args| async move {
        let ws_id = text(&args, "workspaceId").map(str::to_string);
        let raw_endpoint = text(&args, "customEndpointUrl").map(str::trim).filter(|s| !s.is_empty());
        let validated_endpoint = match raw_endpoint {
            Some(url) => Some(validate_https_endpoint_url(url)?),
            None => None,
        };

        let vault = crate::remote::secret_vault(&host);
        if let Some(tok) = text(&args, "endpointToken").map(str::trim).filter(|s| !s.is_empty()) {
            vault.save(ENDPOINT_TOKEN_SECRET_ID, tok).map_err(|e| e.to_string())?;
        } else if validated_endpoint.is_none() {
            let _ = vault.delete(ENDPOINT_TOKEN_SECRET_ID);
        }

        let book_after = with_book(&host, |book| {
            if let Some(v) = args.get("shareProjectMemory").and_then(Value::as_bool) {
                book.settings.share_project_memory = v;
            }
            if let Some(v) = args.get("shareUserMemory").and_then(Value::as_bool) {
                book.settings.share_user_memory = v;
            }
            if let Some(v) = args.get("autoInject").and_then(Value::as_bool) {
                book.settings.auto_inject = v;
            }
            if args.get("customEndpointUrl").is_some() {
                book.settings.custom_endpoint_url = validated_endpoint;
            }
            book.clone()
        });

        persist_snapshot_if_allowed(&host, &book_after)?;
        let state_json = build_state_response(&host, ws_id.as_deref());
        Ok(json!({ "state": state_json }))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(scope: MemoryScope) -> SharedMemoryEntry {
        let mut entry = SharedMemoryEntry {
            id: "e".into(),
            scope,
            key: "k".into(),
            title: "k".into(),
            content: "c".into(),
            tags: vec![],
            auto_inject: true,
            pinned: false,
            revision: 1,
            seq: 1,
            content_hash: String::new(),
            updated_at: "2026-11-22T00:00:00Z".into(),
            updated_by_device_id: "d".into(),
            updated_by_device_name: "D".into(),
            deleted: false,
        };
        entry.recompute_hash();
        entry
    }

    fn project(workspace: &str, slug: &str) -> MemoryScope {
        MemoryScope::Project { workspace_id: workspace.into(), project_slug: slug.into() }
    }

    fn scope(share_user: bool) -> ConnectionScope {
        ConnectionScope {
            ids: HashSet::from(["ws-remote".to_string()]),
            id_to_slug: HashMap::from([("ws-remote".to_string(), "app".to_string())]),
            share_user,
        }
    }

    /// What a host (or a custom endpoint) sends is let in only for that connection's workspaces: no user entry while user memory is
    /// off, no local project's, and a slug that names another project is replaced by the one this computer knows.
    #[test]
    fn a_pulled_entry_is_admitted_only_for_the_connections_workspaces() {
        assert!(scope(false).admit(entry(MemoryScope::User)).is_none(), "user entries need share_user_memory");
        assert!(scope(true).admit(entry(MemoryScope::User)).is_some());
        assert!(scope(true).admit(entry(project("a-local-workspace", "app"))).is_none(), "not one of the connection's workspaces");

        let aimed = scope(false).admit(entry(project("ws-remote", "victim/secret-repo"))).expect("a remote workspace's entry");
        assert_eq!(aimed.scope, project("ws-remote", "app"), "the claimed slug of another project is replaced");
        let longer = scope(false).admit(entry(project("ws-remote", "acme/app"))).unwrap();
        assert_eq!(longer.scope, project("ws-remote", "acme/app"), "the host's longer slug for the same project stays");
        let none_claimed = scope(false).admit(entry(project("ws-remote", ""))).unwrap();
        assert_eq!(none_claimed.scope, project("ws-remote", "app"));
    }

    #[test]
    fn a_connection_may_overwrite_only_local_entries_of_its_projects() {
        let connection = scope(false);
        let slugs = connection.slugs();
        assert!(!connection.covers(&MemoryScope::User, &slugs), "a local user entry stays out of reach while user memory is off");
        assert!(connection.covers(&project("ws-remote", ""), &slugs));
        assert!(connection.covers(&project("my-clone", "App"), &slugs), "a local checkout of the same project by slug");
        assert!(!connection.covers(&project("other-project", "other"), &slugs), "an unrelated local project is never overwritten");
        assert!(scope(true).covers(&MemoryScope::User, &slugs));
    }
}
