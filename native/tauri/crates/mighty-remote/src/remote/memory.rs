//! Incremental shared memory protocol (`/v2/memory/changes` and `/v2/memory/push`) over Tailscale HTTP (`:43137`)
//! and E2EE Paired Computer Relay (`serve_relay`), plus remote-run `memory_context` injection (`MC-13`).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex};

use indexmap::IndexMap;
use mighty_core::model::{PlanRecord, Workspace};
use mighty_core::shared_memory::{MemoryConflict, MemoryScope, SharedMemoryBook, SharedMemoryEntry, infer_project_slug, normalize_slug};
use mighty_platform::BoxFuture;
use mighty_run::model::StartRunRequest;
use mighty_run::plan_mode::PlanDecision;
use mighty_run::questionnaire::UserQuestionAnswer;
use mighty_run::strings::get;
use serde::{Deserialize, Serialize};

use crate::remote::host::{HostFailure, HostRunner, RemoteHost, lock};
use crate::remote::http_server::{HttpRequest, HttpResponse};
use crate::remote::wire::protocol;

pub const CHANGES_PATH: &str = "/v2/memory/changes";
pub const PUSH_PATH: &str = "/v2/memory/push";

/// Entries a `GET /v2/memory/changes` page holds at most when the request names no `limit` (and the most it ever holds).
pub const DEFAULT_CHANGES_LIMIT: usize = 200;
pub const MAX_CHANGES_LIMIT: usize = 500;
/// Serialized bytes a changes page stays under (a relay frame is 1 MiB; a page always holds at least one entry).
pub const MAX_CHANGES_PAGE_BYTES: usize = 256 * 1024;
/// What one `POST /v2/memory/push` body holds at most, so every batch fits the host's request and entry limits.
pub const MAX_PUSH_BATCH_BYTES: usize = 256 * 1024;
pub const MAX_PUSH_BATCH_ENTRIES: usize = 128;

/// Callback resolving formatted `<shared-memory>` text for a shared workspace id during remote run launch (`POST /v2/runs`).
pub type WorkspaceMemoryLookup = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// Host handler for `/v2/memory/changes` and `/v2/memory/push`.
pub trait MemoryHost: Send + Sync {
    /// Returns serialized JSON bytes for `GET /v2/memory/changes?since=<seq>` scoped to `shared_workspaces`.
    fn changes(&self, since: u64, shared_workspaces: &HashSet<String>, query: &str) -> Result<Vec<u8>, (u16, String)>;

    /// Validates, sanitizes, and applies incoming entries from `POST /v2/memory/push` scoped to `shared_workspaces`.
    fn push(&self, shared_workspaces: &HashSet<String>, body: &[u8]) -> Result<Vec<u8>, (u16, String)>;
}

/// Wire response for `GET /v2/memory/changes`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryChangesWire {
    pub protocol: i32,
    pub host_name: String,
    pub device_id: String,
    /// The cursor to pull from next: the last entry's `seq` when `has_more`, else the host's whole sequence.
    pub server_seq: u64,
    #[serde(default)]
    pub entries: Vec<SharedMemoryEntry>,
    /// More entries follow `server_seq` (older peers ignore this and simply catch up on their next sync).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub has_more: bool,
}

/// Wire request body for `POST /v2/memory/push`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryPushWire {
    pub protocol: i32,
    #[serde(default)]
    pub device_id: String,
    #[serde(default)]
    pub device_name: String,
    #[serde(default)]
    pub entries: Vec<SharedMemoryEntry>,
    #[serde(default)]
    pub base_hashes: BTreeMap<String, String>,
}

/// Wire response body for `POST /v2/memory/push`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryPushReplyWire {
    pub protocol: i32,
    pub accepted: bool,
    pub applied: usize,
    pub skipped: usize,
    pub redacted_secrets: usize,
    #[serde(default)]
    pub conflicts: Vec<MemoryConflict>,
    pub server_seq: u64,
}

/// Whether `scope` is one the host shares with a peer: `User` when user memory is shared, a project of a shared workspace (by id), or
/// a project this host does not have at all whose slug is a shared workspace's. A project of a host workspace that is NOT shared is
/// never shared, whatever its slug.
fn scope_is_shared(
    scope: &MemoryScope,
    share_user: bool,
    shared_workspaces: &HashSet<String>,
    workspace_slugs: &HashMap<String, String>,
    shared_slugs: &HashSet<String>,
) -> bool {
    match scope {
        MemoryScope::User => share_user,
        MemoryScope::Project { workspace_id, project_slug } => {
            if shared_workspaces.contains(workspace_id) {
                return true;
            }
            if workspace_slugs.contains_key(workspace_id) {
                return false;
            }
            let slug = normalize_slug(project_slug);
            !slug.is_empty() && shared_slugs.contains(&slug)
        }
    }
}

fn shared_slug_set(shared_workspaces: &HashSet<String>, workspace_slugs: &HashMap<String, String>) -> HashSet<String> {
    shared_workspaces.iter().filter_map(|id| workspace_slugs.get(id)).map(|slug| normalize_slug(slug)).filter(|s| !s.is_empty()).collect()
}

/// Parses `limit=<n>` out of a changes query: the page size, clamped to `1..=MAX_CHANGES_LIMIT` (absent or invalid is the default).
pub fn parse_limit_query(query: &str) -> usize {
    let raw = query.strip_prefix('?').unwrap_or(query);
    for pair in raw.split('&') {
        if let Some(("limit", v)) = pair.split_once('=')
            && let Ok(n) = v.parse::<usize>()
        {
            return n.clamp(1, MAX_CHANGES_LIMIT);
        }
    }
    DEFAULT_CHANGES_LIMIT
}

/// Evaluates `GET /v2/memory/changes` against a `SharedMemoryBook` while enforcing workspace & user-scope isolation. One page of at
/// most `limit` entries / [`MAX_CHANGES_PAGE_BYTES`]; a cut page names the last entry's `seq` as `server_seq` and sets `has_more`.
pub fn handle_book_changes(
    book: &SharedMemoryBook,
    host_name: &str,
    since: u64,
    limit: usize,
    shared_workspaces: &HashSet<String>,
    workspace_slugs: &HashMap<String, String>,
) -> Result<Vec<u8>, (u16, String)> {
    let include_user = book.settings.share_user_memory;
    let shared_slugs = shared_slug_set(shared_workspaces, workspace_slugs);
    let mut delta = if book.settings.share_project_memory {
        book.changes_since(since, Some(shared_workspaces), Some(&shared_slugs), include_user)
    } else if include_user {
        let empty = HashSet::new();
        book.changes_since(since, Some(&empty), Some(&empty), true)
    } else {
        mighty_core::shared_memory::MemoryDelta { server_seq: book.server_seq, entries: Vec::new() }
    };
    delta.entries.retain(|e| scope_is_shared(&e.scope, include_user, shared_workspaces, workspace_slugs, &shared_slugs));

    let limit = limit.clamp(1, MAX_CHANGES_LIMIT);
    let mut bytes = 0usize;
    let mut keep = 0usize;
    for entry in &delta.entries {
        let size = serde_json::to_vec(entry).map_or(0, |v| v.len());
        if keep > 0 && (keep >= limit || bytes + size > MAX_CHANGES_PAGE_BYTES) {
            break;
        }
        bytes += size;
        keep += 1;
    }
    let has_more = keep < delta.entries.len();
    delta.entries.truncate(keep);
    let server_seq = if has_more { delta.entries.last().map_or(delta.server_seq, |e| e.seq) } else { delta.server_seq };

    let wire = MemoryChangesWire {
        protocol: protocol::VERSION,
        host_name: if host_name.trim().is_empty() { "Mighty Host".into() } else { host_name.to_string() },
        device_id: book.device_id.clone(),
        server_seq,
        entries: delta.entries,
        has_more,
    };
    serde_json::to_vec(&wire).map_err(|e| (500, e.to_string()))
}

/// Splits `entries` into consecutive batches that each fit one `POST /v2/memory/push`: at most [`MAX_PUSH_BATCH_ENTRIES`] entries and
/// [`MAX_PUSH_BATCH_BYTES`] serialized bytes (an entry larger than that alone is a batch of its own).
pub fn split_push_batches(entries: &[SharedMemoryEntry]) -> Vec<&[SharedMemoryEntry]> {
    let mut batches = Vec::new();
    let (mut start, mut bytes) = (0usize, 0usize);
    for (at, entry) in entries.iter().enumerate() {
        let size = serde_json::to_vec(entry).map_or(0, |v| v.len() + 1);
        if at > start && (at - start >= MAX_PUSH_BATCH_ENTRIES || bytes + size > MAX_PUSH_BATCH_BYTES) {
            batches.push(&entries[start..at]);
            (start, bytes) = (at, 0);
        }
        bytes += size;
    }
    if start < entries.len() {
        batches.push(&entries[start..]);
    }
    batches
}

/// The part of a host's `base_hashes` a batch of entries needs (by id and by `scope::key` slot), so the body stays small.
pub fn push_base_hashes(batch: &[SharedMemoryEntry], all: Option<&BTreeMap<String, String>>) -> BTreeMap<String, String> {
    let Some(all) = all else { return BTreeMap::new() };
    let mut out = BTreeMap::new();
    for entry in batch {
        for key in [entry.id.clone(), format!("{}::{}", entry.scope.scope_key(), entry.key)] {
            if let Some(hash) = all.get(&key) {
                out.insert(key, hash.clone());
            }
        }
    }
    out
}

/// Evaluates `POST /v2/memory/push` into a `SharedMemoryBook`, rejecting unauthorized scopes (`403`),
/// sanitizing secrets on every incoming entry, and recording concurrent conflicts.
pub fn handle_book_push(
    book: &mut SharedMemoryBook,
    shared_workspaces: &HashSet<String>,
    workspace_slugs: &HashMap<String, String>,
    body: &[u8],
) -> Result<Vec<u8>, (u16, String)> {
    let req: MemoryPushWire = serde_json::from_slice(body).map_err(|_| (400, get("remote.host.error.badRequest")))?;
    if req.protocol != protocol::VERSION {
        return Err((400, get("remote.host.error.badRequest")));
    }
    if req.entries.len() > 256 {
        return Err((413, get("remote.run.error.tooLarge")));
    }

    let shared_slugs = shared_slug_set(shared_workspaces, workspace_slugs);
    let mut slug_to_ws: HashMap<String, String> = HashMap::new();
    for ws_id in shared_workspaces {
        if let Some(slug) = workspace_slugs.get(ws_id) {
            let norm = normalize_slug(slug);
            if !norm.is_empty() {
                slug_to_ws.insert(norm, ws_id.clone());
            }
        }
    }

    let mut normalized_entries = Vec::with_capacity(req.entries.len());
    for mut entry in req.entries {
        match &mut entry.scope {
            MemoryScope::User => {
                if !book.settings.share_user_memory {
                    return Err((403, "User-scoped shared memory is disabled on this host.".into()));
                }
            }
            MemoryScope::Project { workspace_id, project_slug } => {
                if !book.settings.share_project_memory {
                    return Err((403, "Project shared memory is disabled on this host.".into()));
                }
                if !shared_workspaces.contains(workspace_id.as_str()) {
                    let norm_slug = normalize_slug(project_slug);
                    let Some(host_ws_id) = (!norm_slug.is_empty()).then(|| slug_to_ws.get(&norm_slug)).flatten() else {
                        return Err((403, get("remote.host.error.notShared")));
                    };
                    *workspace_id = host_ws_id.clone();
                }
                // The slug is the host's own for that workspace, never the peer's: a peer could otherwise name another workspace's slug
                // and have the entry injected into (or synced out of) a project it was not given.
                *project_slug = workspace_slugs.get(workspace_id.as_str()).cloned().unwrap_or_default();
            }
        }
        if entry.updated_by_device_id.trim().is_empty() && !req.device_id.trim().is_empty() {
            entry.updated_by_device_id = req.device_id.clone();
        }
        if entry.updated_by_device_name.trim().is_empty() && !req.device_name.trim().is_empty() {
            entry.updated_by_device_name = req.device_name.clone();
        }
        normalized_entries.push(entry);
    }

    // The peer may only land on local entries it is shown: a local entry of a workspace that is not shared, or a user entry while user
    // memory is off, is skipped even when the peer names its id.
    let share_user = book.settings.share_user_memory;
    let outcome = book.apply_incoming_scoped(normalized_entries, &req.base_hashes, &|scope| {
        scope_is_shared(scope, share_user, shared_workspaces, workspace_slugs, &shared_slugs)
    });
    let reply = MemoryPushReplyWire {
        protocol: protocol::VERSION,
        accepted: true,
        applied: outcome.applied,
        skipped: outcome.skipped,
        redacted_secrets: outcome.redacted_secrets,
        conflicts: outcome.conflicts,
        server_seq: outcome.server_seq,
    };
    serde_json::to_vec(&reply).map_err(|e| (500, e.to_string()))
}

/// Stand-alone in-memory / callback-backed `MemoryHost` over a `Mutex<SharedMemoryBook>` and workspace resolver.
pub struct BookMemoryHost {
    book: Arc<Mutex<SharedMemoryBook>>,
    host_name: String,
    workspaces: Arc<dyn Fn() -> Vec<Workspace> + Send + Sync>,
}

impl BookMemoryHost {
    pub fn new(
        book: Arc<Mutex<SharedMemoryBook>>,
        host_name: impl Into<String>,
        workspaces: Arc<dyn Fn() -> Vec<Workspace> + Send + Sync>,
    ) -> Self {
        BookMemoryHost { book, host_name: host_name.into(), workspaces }
    }

    fn slug_map(&self) -> HashMap<String, String> {
        (self.workspaces)()
            .into_iter()
            .filter(|w| w.remote.is_none())
            .map(|w| (w.id().to_string(), infer_project_slug(w.path(), w.name())))
            .collect()
    }
}

impl MemoryHost for BookMemoryHost {
    fn changes(&self, since: u64, shared_workspaces: &HashSet<String>, query: &str) -> Result<Vec<u8>, (u16, String)> {
        let slugs = self.slug_map();
        let book = lock(&self.book);
        handle_book_changes(&book, &self.host_name, since, parse_limit_query(query), shared_workspaces, &slugs)
    }

    fn push(&self, shared_workspaces: &HashSet<String>, body: &[u8]) -> Result<Vec<u8>, (u16, String)> {
        let slugs = self.slug_map();
        let mut book = lock(&self.book);
        handle_book_push(&mut book, shared_workspaces, &slugs, body)
    }
}

/// Parses `?since=<u64>` (allowing empty query or extra `&key=val` parameters).
pub(crate) fn parse_since_query(query: &str) -> Result<u64, HostFailure> {
    if query.is_empty() || query == "?" {
        return Ok(0);
    }
    let raw = query.strip_prefix('?').ok_or_else(|| HostFailure::new(400, get("remote.host.error.badRequest")))?;
    let mut since = 0u64;
    for pair in raw.split('&').filter(|s| !s.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        if k == "since" {
            if v.is_empty() || v.len() > 20 || !v.bytes().all(|b| b.is_ascii_digit()) {
                return Err(HostFailure::new(400, get("remote.host.error.badCursor")));
            }
            since = v.parse::<u64>().map_err(|_| HostFailure::new(400, get("remote.host.error.badCursor")))?;
        }
    }
    Ok(since)
}

fn content_type(request: &HttpRequest) -> &str {
    request.headers.get("content-type").map_or("", |t| t.split(';').next().unwrap_or_default().trim())
}

impl RemoteHost {
    /// Attaches or detaches the `/v2/memory/*` sync handler on this `RemoteHost`.
    pub fn set_memory(&self, value: Option<Arc<dyn MemoryHost>>) {
        lock(&self.0.gate).memory = value;
    }

    /// Builder convenience for attaching `MemoryHost`.
    pub fn with_memory(self, value: Arc<dyn MemoryHost>) -> Self {
        self.set_memory(Some(value));
        self
    }

    /// Attaches or detaches the workspace `memory_context` injector used when remote clients start runs (`POST /v2/runs`).
    pub fn set_memory_for_workspace(&self, value: Option<WorkspaceMemoryLookup>) {
        lock(&self.0.gate).memory_for_workspace = value;
    }

    /// Builder convenience for attaching `memory_for_workspace`.
    pub fn with_memory_for_workspace(self, value: WorkspaceMemoryLookup) -> Self {
        self.set_memory_for_workspace(Some(value));
        self
    }

    pub(crate) fn serve_memory(
        &self,
        request: &HttpRequest,
        route: &[&str],
        query: &str,
        revision: u64,
    ) -> Result<HttpResponse, HostFailure> {
        let memory =
            lock(&self.0.gate).memory.clone().ok_or_else(|| HostFailure::new(404, "Shared memory sync is unavailable on this host."))?;
        let shared: HashSet<String> = self.shared_workspaces().into_iter().map(|w| w.id().to_string()).collect();
        if lock(&self.0.gate).generation != revision {
            return Err(HostFailure::new(503, get("remote.host.error.stopped")));
        }

        let result = match (request.method.as_str(), route) {
            ("GET", ["changes"]) => {
                let since = parse_since_query(query)?;
                memory.changes(since, &shared, query)
            }
            ("POST", ["push"]) => {
                if !query.is_empty() && query != "?" {
                    return Err(HostFailure::new(400, get("remote.host.error.badPath")));
                }
                if content_type(request) != "application/json" {
                    return Err(HostFailure::new(415, get("remote.host.error.jsonOnly")));
                }
                if request.body.is_empty() {
                    return Err(HostFailure::new(400, get("remote.host.error.badRequest")));
                }
                if request.body.len() > protocol::MAXIMUM_REQUEST_BYTES {
                    return Err(HostFailure::new(413, get("remote.run.error.tooLarge")));
                }
                memory.push(&shared, &request.body)
            }
            _ => return Err(HostFailure::new(404, get("remote.host.error.badPath"))),
        };

        if lock(&self.0.gate).generation != revision {
            return Err(HostFailure::new(503, get("remote.host.error.stopped")));
        }

        match result {
            Ok(body) => Ok(HttpResponse { status: 200, body, headers: vec![(protocol::HEADER.into(), "2".into())] }),
            Err((status, message)) => Err(HostFailure::new(status, message)),
        }
    }
}

struct MemoryInjectingRunner {
    inner: Arc<dyn HostRunner>,
    host: RemoteHost,
}

impl HostRunner for MemoryInjectingRunner {
    fn start(&self, mut request: StartRunRequest) -> BoxFuture<'_, mighty_run::Result<()>> {
        if request.kind == "claude" && request.memory_context.is_none() {
            let lookup = lock(&self.host.0.gate).memory_for_workspace.clone();
            if let Some(lookup) = lookup {
                request.memory_context = lookup(&request.workspace_id);
            }
        }
        self.inner.start(request)
    }

    fn stop(&self, session_id: String) -> BoxFuture<'_, mighty_run::Result<()>> {
        self.inner.stop(session_id)
    }

    fn respond_to_tool_permission(&self, session_id: &str, request_id: &str, allow: bool) -> mighty_run::Result<()> {
        self.inner.respond_to_tool_permission(session_id, request_id, allow)
    }

    fn answer_questionnaire(
        &self,
        session_id: &str,
        request_id: &str,
        answers: &IndexMap<String, UserQuestionAnswer>,
    ) -> mighty_run::Result<()> {
        self.inner.answer_questionnaire(session_id, request_id, answers)
    }

    fn answer_plan(&self, session_id: &str, request_id: &str, decision: &PlanDecision) -> mighty_run::Result<PlanRecord> {
        self.inner.answer_plan(session_id, request_id, decision)
    }

    fn dispose(&self) -> BoxFuture<'_, ()> {
        self.inner.dispose()
    }
}

pub(crate) fn wrap_runner(inner: Arc<dyn HostRunner>, host: RemoteHost) -> Arc<dyn HostRunner> {
    Arc::new(MemoryInjectingRunner { inner, host })
}
