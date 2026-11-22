//! App-side handler for `mighty-workspace` (`--agent-workspace-mcp`, `MC-14` + `MC-13` shared memory & `remote-cache`
//! integration): strictly read-only, on-demand lookup across local workspaces, live remote/mirrored workspaces, and offline
//! `remote-cache` snapshots (`list_workspaces`, `search_workspace`, `read_workspace_file`, `recent_workspace_activity`).
//!
//! Security boundaries:
//! - Disabled immediately whenever `AppSnapshot::cross_workspace_lookup_on()` is false.
//! - Sensitive files (`.env*` except templates, keys/certs, credential JSONs, `.git/**`, `.ssh/**`, etc.) are refused on both search
//!   and read.
//! - File reads canonicalize both the workspace root and target path, refusing path traversal (`..`) and symlinks escaping the root.

use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Weak};
use std::time::Duration;

#[cfg(test)]
use mighty_core::model::Workspace;
use mighty_core::model::{AppSnapshot, RunSession};
use mighty_core::shared_memory::{
    CachedRemoteSessionDigest, CachedRemoteState, SharedMemoryBook, SharedMemoryEntry, infer_project_slug, infer_remote_project_slug,
    list_cached_remote_states, normalize_slug,
};
use mighty_delegation::mcp::workspace::{DETACHED_MESSAGE, WorkspaceCaller, WorkspaceHandler, WorkspaceRequest, WorkspaceResponse};
use mighty_platform::{BoxFuture, SpawnRequest};
use mighty_remote::remote::wire::RemoteConnectionStatus;

use crate::host::Host;

const MAX_FILE_BYTES: u64 = 512 * 1024;
const MAX_READ_LINES: usize = 250;
const MAX_OUTPUT_BYTES: usize = 32 * 1024;
const MAX_SCANNED_FILES: usize = 2_500;
/// Every folder entry a search may walk, read or not.
const MAX_VISITED_ENTRIES: usize = 50_000;
const MAX_FILE_HITS: usize = 40;
const MAX_CONVERSATION_HITS: usize = 15;
const MAX_MEMORY_HITS: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceOrigin {
    Local,
    Remote,
    RemoteCache,
}

impl WorkspaceOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            WorkspaceOrigin::Local => "local",
            WorkspaceOrigin::Remote => "remote",
            WorkspaceOrigin::RemoteCache => "remote-cache",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedWorkspace {
    pub id: String,
    pub remote_workspace_id: Option<String>,
    pub name: String,
    pub path: String,
    pub project_slug: String,
    pub origin: WorkspaceOrigin,
    pub online: bool,
    pub connection_id: Option<String>,
    pub host_name: Option<String>,
    pub cached_branch: Option<String>,
    pub cached_sessions: Vec<CachedRemoteSessionDigest>,
}

struct AppWorkspaceMcp {
    host: Weak<Host>,
}

/// Returns the `WorkspaceHandler` bound to `host`.
pub fn handler(host: &Arc<Host>) -> Arc<dyn WorkspaceHandler> {
    Arc::new(AppWorkspaceMcp { host: Arc::downgrade(host) })
}

impl WorkspaceHandler for AppWorkspaceMcp {
    fn handle(&self, request: WorkspaceRequest, caller: WorkspaceCaller) -> BoxFuture<'static, WorkspaceResponse> {
        let weak = self.host.clone();
        Box::pin(async move {
            let Some(host) = weak.upgrade() else {
                return WorkspaceResponse::failure("Mighty Code is closing.");
            };
            if host.closing() {
                return WorkspaceResponse::failure("Mighty Code is closing.");
            }
            let snapshot = {
                let view = host.view();
                if !view.snapshot.cross_workspace_lookup_on() {
                    return WorkspaceResponse::failure(DETACHED_MESSAGE);
                }
                view.current()
            };
            match request.tool.as_str() {
                "list_workspaces" => list_workspaces(&host, &snapshot, &caller),
                "search_workspace" => search_workspace(&host, &snapshot, &request).await,
                "read_workspace_file" => read_workspace_file(&host, &snapshot, &request).await,
                "recent_workspace_activity" => recent_workspace_activity(&host, &snapshot, &request).await,
                other => WorkspaceResponse::failure(format!("Unknown mighty-workspace tool: {other}")),
            }
        })
    }
}

/// Normalizes a workspace name into the `@workspace:<token>` slug form (spaces turned into `-`).
pub fn mention_slug(name: &str) -> String {
    name.split_whitespace().collect::<Vec<_>>().join("-")
}

/// Resolves a local workspace by id, exact/case-insensitive name, `@workspace:<slug>`, or folder basename.
#[cfg(test)]
pub fn resolve_local_workspace<'a>(workspaces: &'a [Workspace], raw_query: &str) -> Option<&'a Workspace> {
    let trimmed = raw_query.trim();
    let stripped = strip_workspace_prefix(trimmed);
    if stripped.is_empty() {
        return None;
    }
    let locals: Vec<&Workspace> = workspaces.iter().filter(|w| w.remote.is_none() && !w.path().is_empty()).collect();
    if let Some(exact_id) = locals.iter().copied().find(|w| w.id() == stripped) {
        return Some(exact_id);
    }
    if let Some(exact_name) = locals.iter().copied().find(|w| w.name() == stripped) {
        return Some(exact_name);
    }
    let lower = stripped.to_lowercase();
    if let Some(ci_name) = locals.iter().copied().find(|w| w.name().to_lowercase() == lower) {
        return Some(ci_name);
    }
    if let Some(slug_match) = locals.iter().copied().find(|w| mention_slug(w.name()).to_lowercase() == lower) {
        return Some(slug_match);
    }
    locals
        .into_iter()
        .find(|w| Path::new(w.path()).file_name().and_then(|n| n.to_str()).is_some_and(|base| base.eq_ignore_ascii_case(stripped)))
}

fn strip_workspace_prefix(raw: &str) -> &str {
    raw.trim()
        .strip_prefix("@workspace:")
        .or_else(|| raw.trim().strip_prefix("workspace:"))
        .or_else(|| raw.trim().strip_prefix('@'))
        .unwrap_or(raw.trim())
        .trim()
        .trim_matches('"')
}

/// Collects local, live remote/mirrored, and offline `remote-cache` workspaces into a unified list.
pub fn collect_all_workspaces(
    snapshot: &AppSnapshot,
    cached_states: &[CachedRemoteState],
    online_connections: &HashSet<String>,
) -> Vec<ResolvedWorkspace> {
    let cache_by_conn: HashMap<&str, &CachedRemoteState> = cached_states.iter().map(|c| (c.connection_id.as_str(), c)).collect();
    let mut out = Vec::new();
    let mut seen_remote_pairs: HashSet<(String, String)> = HashSet::new();

    for ws in snapshot.workspaces() {
        if let Some(rem) = &ws.remote {
            let cid = rem.connection_id.clone().unwrap_or_default();
            let rws_id = rem.workspace_id.clone().filter(|s| !s.is_empty()).unwrap_or_else(|| ws.id().to_string());
            if !cid.is_empty() {
                seen_remote_pairs.insert((cid.clone(), rws_id.clone()));
            }
            let online = !cid.is_empty() && online_connections.contains(&cid);
            let cached = cache_by_conn.get(cid.as_str()).copied();
            let cw = cached.and_then(|c| c.workspaces.iter().find(|w| w.id == rws_id));
            let slug = cw
                .map(|w| w.project_slug.clone())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| infer_remote_project_slug(ws.path(), ws.name()));
            let cached_sessions =
                cached.map(|c| c.sessions.iter().filter(|s| s.workspace_id == rws_id).cloned().collect()).unwrap_or_default();
            out.push(ResolvedWorkspace {
                id: ws.id().to_string(),
                remote_workspace_id: Some(rws_id),
                name: ws.name().to_string(),
                path: ws.path().to_string(),
                project_slug: slug,
                origin: if online { WorkspaceOrigin::Remote } else { WorkspaceOrigin::RemoteCache },
                online,
                connection_id: (!cid.is_empty()).then_some(cid),
                host_name: rem.host_name.clone().or_else(|| cached.map(|c| c.host_name.clone())),
                cached_branch: cw.and_then(|w| w.branch.clone()),
                cached_sessions,
            });
        } else if !ws.path().is_empty() {
            out.push(ResolvedWorkspace {
                id: ws.id().to_string(),
                remote_workspace_id: None,
                name: ws.name().to_string(),
                path: ws.path().to_string(),
                project_slug: infer_project_slug(ws.path(), ws.name()),
                origin: WorkspaceOrigin::Local,
                online: true,
                connection_id: None,
                host_name: None,
                cached_branch: None,
                cached_sessions: Vec::new(),
            });
        }
    }

    for cached in cached_states {
        let online = online_connections.contains(&cached.connection_id);
        for cw in &cached.workspaces {
            if !seen_remote_pairs.insert((cached.connection_id.clone(), cw.id.clone())) {
                continue;
            }
            // Another computer's path: its name only, never a lookup on this disk.
            let slug = if cw.project_slug.is_empty() { infer_remote_project_slug(&cw.path, &cw.name) } else { cw.project_slug.clone() };
            let cached_sessions = cached.sessions.iter().filter(|s| s.workspace_id == cw.id).cloned().collect();
            out.push(ResolvedWorkspace {
                id: format!("mirror:{}:{}", cached.connection_id, cw.id),
                remote_workspace_id: Some(cw.id.clone()),
                name: cw.name.clone(),
                path: cw.path.clone(),
                project_slug: slug,
                origin: if online { WorkspaceOrigin::Remote } else { WorkspaceOrigin::RemoteCache },
                online,
                connection_id: Some(cached.connection_id.clone()),
                host_name: Some(cached.host_name.clone()),
                cached_branch: cw.branch.clone(),
                cached_sessions,
            });
        }
    }

    out
}

/// Resolves any workspace (local, remote, or offline `remote-cache`) by id, remote id, name, `@workspace:<slug>`, or folder basename.
pub fn resolve_any_workspace<'a>(workspaces: &'a [ResolvedWorkspace], raw_query: &str) -> Option<&'a ResolvedWorkspace> {
    let stripped = strip_workspace_prefix(raw_query);
    if stripped.is_empty() {
        return None;
    }
    if let Some(exact_id) = workspaces.iter().find(|w| w.id == stripped || w.remote_workspace_id.as_deref() == Some(stripped)) {
        return Some(exact_id);
    }
    if let Some(exact_name) = workspaces.iter().find(|w| w.name == stripped) {
        return Some(exact_name);
    }
    let lower = stripped.to_lowercase();
    if let Some(ci_name) = workspaces.iter().find(|w| w.name.to_lowercase() == lower) {
        return Some(ci_name);
    }
    if let Some(slug_match) = workspaces.iter().find(|w| mention_slug(&w.name).to_lowercase() == lower) {
        return Some(slug_match);
    }
    let norm_slug = normalize_slug(stripped);
    if !norm_slug.is_empty()
        && let Some(proj_match) = workspaces.iter().find(|w| normalize_slug(&w.project_slug) == norm_slug)
    {
        return Some(proj_match);
    }
    workspaces
        .iter()
        .find(|w| w.path.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().is_some_and(|base| base.eq_ignore_ascii_case(stripped)))
}

fn gather_host_lookup_context(host: &Host, snapshot: &AppSnapshot) -> (Vec<ResolvedWorkspace>, SharedMemoryBook, Vec<CachedRemoteState>) {
    let online_connections: HashSet<String> = crate::remote::app_if_made()
        .map(|app| app.client.connections().into_iter().filter(|c| c.status == RemoteConnectionStatus::Connected).map(|c| c.id).collect())
        .unwrap_or_default();
    let cached_states = if host.smoke.is_none() { list_cached_remote_states(&host.data_dir) } else { Vec::new() };
    let resolved = collect_all_workspaces(snapshot, &cached_states, &online_connections);
    let book = crate::shared_memory::book_snapshot(host);
    (resolved, book, cached_states)
}

fn memory_entries_for_resolved(
    ws: &ResolvedWorkspace,
    book: &SharedMemoryBook,
    cached_states: &[CachedRemoteState],
    include_user: bool,
) -> Vec<SharedMemoryEntry> {
    let slug = (!ws.project_slug.is_empty()).then_some(ws.project_slug.as_str());
    let matches_ws = |entry: &SharedMemoryEntry| -> bool {
        if entry.deleted {
            return false;
        }
        match &entry.scope {
            mighty_core::shared_memory::MemoryScope::User => include_user,
            mighty_core::shared_memory::MemoryScope::Project { .. } => {
                entry.scope.matches_project(&ws.id, slug)
                    || ws.remote_workspace_id.as_deref().is_some_and(|rw| entry.scope.matches_project(rw, slug))
            }
        }
    };

    let mut by_slot: HashMap<String, SharedMemoryEntry> = HashMap::new();
    for entry in book.entries.iter().filter(|e| matches_ws(e)) {
        let slot = format!("{}::{}", entry.scope.scope_key(), entry.key);
        by_slot.insert(slot, entry.clone());
    }
    if let Some(cid) = ws.connection_id.as_deref()
        && let Some(cached) = cached_states.iter().find(|c| c.connection_id == cid)
    {
        for entry in cached.memory_entries.iter().filter(|e| matches_ws(e)) {
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
    let mut out: Vec<SharedMemoryEntry> = by_slot.into_values().collect();
    out.sort_by(|a, b| b.pinned.cmp(&a.pinned).then_with(|| b.updated_at.cmp(&a.updated_at)).then_with(|| a.key.cmp(&b.key)));
    out
}

/// Whether a workspace-relative path names a secret, key, credential, or VCS internal file that must never be exposed.
pub fn is_sensitive_relative_path(path: &Path) -> bool {
    let mut parts = Vec::new();
    for component in path.components() {
        let Component::Normal(os) = component else { return true };
        let Some(text) = os.to_str() else { return true };
        parts.push(text.to_ascii_lowercase());
    }
    if parts.is_empty() {
        return true;
    }
    for dir in &parts[..parts.len() - 1] {
        if matches!(dir.as_str(), ".git" | ".ssh" | ".aws" | ".gnupg" | ".kube" | ".docker" | ".azure" | ".gcloud" | ".terraform.d") {
            return true;
        }
    }
    let file = parts.last().map(String::as_str).unwrap_or("");
    if file == ".env" {
        return true;
    }
    if let Some(suffix) = file.strip_prefix(".env.") {
        return !matches!(suffix, "example" | "sample" | "template" | "schema" | "defaults");
    }
    if matches!(
        file,
        ".npmrc"
            | ".netrc"
            | ".pypirc"
            | ".dockerconfigjson"
            | "google-services.json"
            | "googleservice-info.plist"
            | "secrets.json"
            | "secrets.yaml"
            | "secrets.yml"
            | "master.key"
            | ".dev.vars"
            | ".envrc"
            | ".git-credentials"
            | "credentials"
            | "auth.json"
            | "hosts.yml"
            | ".pgpass"
            | ".vault-token"
            | ".htpasswd"
            | "terraform.tfstate"
            | "terraform.tfstate.backup"
            | ".boto"
            | ".s3cfg"
    ) {
        return true;
    }
    if file.starts_with("authkey_") && file.ends_with(".p8") || file.ends_with(".tfvars") || file.ends_with(".tfvars.json") {
        return true;
    }
    if file.starts_with("id_rsa") || file.starts_with("id_ed25519") || file.starts_with("id_ecdsa") || file.starts_with("id_dsa") {
        return true;
    }
    if (file.contains("credentials") || file.contains("service-account") || file.contains("client_secret"))
        && (file.ends_with(".json") || file.ends_with(".yaml") || file.ends_with(".yml") || file.ends_with(".toml"))
    {
        return true;
    }
    matches!(
        Path::new(file).extension().and_then(|e| e.to_str()),
        Some("pem" | "key" | "p12" | "pfx" | "jks" | "keystore" | "kdbx" | "der" | "cer" | "crt" | "p8" | "ppk" | "asc" | "gpg")
    )
}

fn is_skipped_dir_name(name: &str) -> bool {
    matches!(
        name,
        ".git"
            | ".hg"
            | ".svn"
            | "node_modules"
            | "target"
            | "dist"
            | "build"
            | "out"
            | ".next"
            | ".nuxt"
            | ".turbo"
            | ".venv"
            | "venv"
            | "__pycache__"
            | "Pods"
            | "DerivedData"
            | ".gradle"
            | "coverage"
            | ".wrangler"
    )
}

/// Validates and resolves `relative` inside `root`, refusing traversal, sensitive names, and symlinks escaping `root`.
pub fn resolve_safe_file(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let trimmed = relative.trim().trim_start_matches("./");
    if trimmed.is_empty() {
        return Err("File path must not be empty.".into());
    }
    let rel = Path::new(trimmed);
    if rel.is_absolute() {
        return Err("File path must be workspace-relative, not absolute.".into());
    }
    for component in rel.components() {
        if !matches!(component, Component::Normal(_)) {
            return Err("Path traversal ('..') and prefix components are refused.".into());
        }
    }
    if is_sensitive_relative_path(rel) {
        return Err(format!("Access to sensitive or secret file '{trimmed}' is refused by Mighty Code policy."));
    }
    let canonical_root = std::fs::canonicalize(root).map_err(|_| "Workspace folder does not exist on disk.".to_string())?;
    let joined = canonical_root.join(rel);
    let canonical_target = std::fs::canonicalize(&joined).map_err(|_| format!("File not found: {trimmed}"))?;
    if !canonical_target.starts_with(&canonical_root) {
        return Err("Refused symlink escaping the workspace root.".into());
    }
    let rel_inside = canonical_target.strip_prefix(&canonical_root).unwrap_or(rel);
    if is_sensitive_relative_path(rel_inside) {
        return Err(format!("Access to sensitive target '{trimmed}' is refused by Mighty Code policy."));
    }
    let meta = std::fs::metadata(&canonical_target).map_err(|e| format!("Cannot read metadata for {trimmed}: {e}"))?;
    if !meta.is_file() {
        return Err(format!("Not a regular file: {trimmed}"));
    }
    if meta.len() > MAX_FILE_BYTES {
        return Err(format!("File '{trimmed}' is {} bytes (exceeds 512 KiB read limit).", meta.len()));
    }
    Ok(canonical_target)
}

/// Reads `[start_line..=end_line]` (1-based, bounded to 250 lines / 32 KiB) of a validated text file.
pub fn read_bounded_lines(root: &Path, relative: &str, start_line: usize, end_line: Option<usize>) -> Result<String, String> {
    let target = resolve_safe_file(root, relative)?;
    let bytes = std::fs::read(&target).map_err(|e| format!("Cannot read '{relative}': {e}"))?;
    if bytes[..bytes.len().min(8192)].contains(&0) {
        return Err(format!("Binary file '{relative}' cannot be read as text."));
    }
    let text = String::from_utf8_lossy(&bytes);
    slice_numbered_text(relative, &text, start_line, end_line)
}

fn slice_numbered_text(label: &str, text: &str, start_line: usize, end_line: Option<usize>) -> Result<String, String> {
    let all_lines: Vec<&str> = text.lines().collect();
    let total = all_lines.len();
    if total == 0 {
        return Ok(format!("{label} (empty file, 0 lines)"));
    }
    let start = start_line.max(1).min(total);
    let requested_end = end_line.unwrap_or(start + MAX_READ_LINES - 1).max(start);
    let end = requested_end.min(start + MAX_READ_LINES - 1).min(total);
    let mut out = format!("{label} (lines {start}-{end} of {total})\n");
    for (idx, line) in all_lines[start - 1..end].iter().enumerate() {
        let numbered = format!("{:>4}: {}\n", start + idx, line);
        if out.len() + numbered.len() > MAX_OUTPUT_BYTES {
            out.push_str("... (truncated at 32 KiB output budget)\n");
            break;
        }
        out.push_str(&numbered);
    }
    Ok(out)
}

/// Fast `.git/HEAD` reader (supports both standard `.git/HEAD` and git worktree `.git` gitdir files).
pub fn git_head_branch(root: &Path) -> Option<String> {
    let dot_git = root.join(".git");
    let head_path = if dot_git.is_dir() {
        dot_git.join("HEAD")
    } else if dot_git.is_file() {
        let pointer = std::fs::read_to_string(&dot_git).ok()?;
        let gitdir = pointer.trim().strip_prefix("gitdir:")?.trim();
        let resolved = Path::new(gitdir);
        let full = if resolved.is_absolute() { resolved.to_path_buf() } else { root.join(resolved) };
        full.join("HEAD")
    } else {
        return None;
    };
    let head = std::fs::read_to_string(head_path).ok()?;
    let trimmed = head.trim();
    if let Some(branch) = trimmed.strip_prefix("ref: refs/heads/") {
        Some(branch.to_string())
    } else {
        Some(trimmed.chars().take(8).collect())
    }
}

fn latest_session_summary(sessions: &[Arc<RunSession>], workspace_id: &str) -> Option<String> {
    let panes: Vec<&Arc<RunSession>> = sessions.iter().filter(|s| s.workspace_id() == workspace_id && s.kind() == "claude").collect();
    let latest = panes.last()?;
    let last_user = latest
        .logs()
        .iter()
        .rev()
        .find(|e| e.kind.as_deref() == Some("user") && !e.text.as_deref().unwrap_or("").trim().is_empty())
        .map(|e| truncate_single_line(e.text.as_deref().unwrap_or(""), 100));
    let title = truncate_single_line(latest.title(), 60);
    Some(match last_user {
        Some(prompt) if prompt != title => format!("{} [{}] - latest: {}", title, latest.status(), prompt),
        _ => format!("{} [{}]", title, latest.status()),
    })
}

fn latest_cached_session_summary(sessions: &[CachedRemoteSessionDigest]) -> Option<String> {
    let latest = sessions.last()?;
    let last_user =
        latest.tail.iter().rev().find(|l| l.kind == "user" && !l.text.trim().is_empty()).map(|l| truncate_single_line(&l.text, 100));
    let title = truncate_single_line(&latest.title, 60);
    Some(match last_user {
        Some(prompt) if prompt != title => format!("{} [{}] - latest: {}", title, latest.status, prompt),
        _ => format!("{} [{}]", title, latest.status),
    })
}

fn truncate_single_line(text: &str, max_chars: usize) -> String {
    let collapsed: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = collapsed.chars();
    let head: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() { format!("{head}...") } else { head }
}

pub fn format_workspace_list(
    workspaces: &[ResolvedWorkspace],
    sessions: &[Arc<RunSession>],
    book: &SharedMemoryBook,
    cached_states: &[CachedRemoteState],
    current_workspace_id: &str,
) -> String {
    if workspaces.is_empty() {
        return "No workspaces are registered in Mighty Code.".into();
    }
    let mut lines = Vec::with_capacity(workspaces.len() + 1);
    lines.push(format!("Registered workspaces ({}):", workspaces.len()));
    for ws in workspaces {
        let marker = if ws.id == current_workspace_id { " [current]" } else { "" };
        let folder = ws.path.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().filter(|s| !s.is_empty()).unwrap_or(&ws.path);
        let branch = match ws.origin {
            WorkspaceOrigin::Local => git_head_branch(Path::new(&ws.path)).unwrap_or_else(|| "-".into()),
            WorkspaceOrigin::Remote | WorkspaceOrigin::RemoteCache => ws.cached_branch.clone().unwrap_or_else(|| "-".into()),
        };
        let live_pane_count = sessions.iter().filter(|s| s.workspace_id() == ws.id && s.kind() == "claude").count();
        let pane_count = if live_pane_count > 0 { live_pane_count } else { ws.cached_sessions.len() };
        let mem_count = memory_entries_for_resolved(ws, book, cached_states, true).len();
        let slug = mention_slug(&ws.name);
        let host_note = ws.host_name.as_deref().map(|h| format!(", host: {h}")).unwrap_or_default();
        let mut line = format!(
            "- {} (id: {}, mention: @workspace:{}, origin: {}, online: {}, sharedMemoryCount: {}{}, folder: {}, branch: {}, agent panes: {}){}",
            ws.name,
            ws.id,
            slug,
            ws.origin.as_str(),
            ws.online,
            mem_count,
            host_note,
            folder,
            branch,
            pane_count,
            marker
        );
        if let Some(summary) = latest_session_summary(sessions, &ws.id).or_else(|| latest_cached_session_summary(&ws.cached_sessions)) {
            line.push_str(&format!("\n  latest session: {summary}"));
        }
        lines.push(line);
    }
    lines.join("\n")
}

fn list_workspaces(host: &Host, snapshot: &AppSnapshot, caller: &WorkspaceCaller) -> WorkspaceResponse {
    let (workspaces, book, cached_states) = gather_host_lookup_context(host, snapshot);
    WorkspaceResponse::ok(format_workspace_list(&workspaces, snapshot.sessions(), &book, &cached_states, &caller.workspace_id))
}

fn search_memory_entries(entries: &[SharedMemoryEntry], needle_lower: &str, max_hits: usize) -> Vec<String> {
    let mut hits = Vec::new();
    for entry in entries {
        if entry.deleted {
            continue;
        }
        let tags_joined = entry.tags.join(", ");
        let matched = entry.key.to_lowercase().contains(needle_lower)
            || entry.title.to_lowercase().contains(needle_lower)
            || entry.content.to_lowercase().contains(needle_lower)
            || tags_joined.to_lowercase().contains(needle_lower);
        if matched {
            let scope_tag = if entry.scope.is_user() { "user" } else { "project" };
            let pin_tag = if entry.pinned { " [pinned]" } else { "" };
            hits.push(format!(
                "- [{}:{}{}] {}: {}",
                scope_tag,
                entry.key,
                pin_tag,
                truncate_single_line(&entry.title, 48),
                truncate_single_line(&entry.content, 160)
            ));
            if hits.len() >= max_hits {
                break;
            }
        }
    }
    hits
}

fn search_cached_conversations(sessions: &[CachedRemoteSessionDigest], needle_lower: &str, max_hits: usize) -> Vec<String> {
    let mut hits = Vec::new();
    for session in sessions {
        if session.title.to_lowercase().contains(needle_lower) {
            hits.push(format!(
                "- [cached pane: {} ({}, rev: {})] title matched",
                truncate_single_line(&session.title, 70),
                session.provider,
                session.body_revision
            ));
        }
        for line in &session.tail {
            if line.text.to_lowercase().contains(needle_lower) {
                hits.push(format!(
                    "- [cached pane: {} · {}] {}",
                    truncate_single_line(&session.title, 48),
                    line.kind,
                    truncate_single_line(&line.text, 160)
                ));
                if hits.len() >= max_hits {
                    return hits;
                }
            }
        }
        if hits.len() >= max_hits {
            return hits;
        }
    }
    hits
}

async fn search_workspace(host: &Host, snapshot: &AppSnapshot, request: &WorkspaceRequest) -> WorkspaceResponse {
    let target_arg = request.arguments.get("workspace").map(String::as_str).unwrap_or("");
    let query = request.arguments.get("query").map(String::as_str).unwrap_or("").trim().to_string();
    let scope = request.arguments.get("scope").map(|s| s.trim().to_ascii_lowercase()).unwrap_or_else(|| "all".into());
    if query.is_empty() {
        return WorkspaceResponse::failure("Search query must not be empty.");
    }
    if !matches!(scope.as_str(), "all" | "files" | "conversations" | "memory") {
        return WorkspaceResponse::failure("Invalid scope: expected 'all', 'files', 'conversations', or 'memory'.");
    }

    let (workspaces, book, cached_states) = gather_host_lookup_context(host, snapshot);
    let Some(ws) = resolve_any_workspace(&workspaces, target_arg).cloned() else {
        return WorkspaceResponse::failure(format!(
            "Workspace '{target_arg}' not found. Call list_workspaces first to see available workspace names and ids."
        ));
    };

    let ws_name = ws.name.clone();
    let ws_id = ws.id.clone();
    let needle = query.to_lowercase();
    let mut sections = Vec::new();

    if matches!(scope.as_str(), "all" | "memory") {
        let mem_entries = memory_entries_for_resolved(&ws, &book, &cached_states, true);
        let mem_hits = search_memory_entries(&mem_entries, &needle, MAX_MEMORY_HITS);
        if !mem_hits.is_empty() {
            sections.push(format!("Shared memory matches in '{ws_name}' ({}):\n{}", mem_hits.len(), mem_hits.join("\n")));
        }
    }

    if matches!(scope.as_str(), "all" | "conversations") {
        let panes: Vec<Arc<RunSession>> =
            snapshot.sessions().iter().filter(|s| s.workspace_id() == ws_id && s.kind() == "claude").cloned().collect();
        let data_dir_owned = host.data_dir.clone();
        let needle_conv = needle.clone();
        let cached_sessions = ws.cached_sessions.clone();
        let conv_hits = tauri::async_runtime::spawn_blocking(move || {
            let mut hits = Vec::new();
            let mut seen_ids = HashSet::new();
            for session in &panes {
                seen_ids.insert(session.id().to_string());
                if session.title().to_lowercase().contains(&needle_conv) {
                    hits.push(format!("- [pane: {} ({})] title matched", truncate_single_line(session.title(), 70), session.provider()));
                }
                let cold_logs =
                    session.body_on_disk.then(|| mighty_core::store::read_cold_body_logs(&data_dir_owned, session.id())).flatten();
                let logs = cold_logs.as_deref().unwrap_or_else(|| session.logs());
                for entry in logs {
                    let kind = entry.kind.as_deref().unwrap_or("");
                    let text = entry.text.as_deref().unwrap_or("");
                    if matches!(kind, "user" | "assistant") && text.to_lowercase().contains(&needle_conv) {
                        hits.push(format!(
                            "- [pane: {} · {}] {}",
                            truncate_single_line(session.title(), 48),
                            kind,
                            truncate_single_line(text, 160)
                        ));
                        if hits.len() >= MAX_CONVERSATION_HITS {
                            break;
                        }
                    }
                }
                if hits.len() >= MAX_CONVERSATION_HITS {
                    break;
                }
            }
            if hits.len() < MAX_CONVERSATION_HITS {
                let remaining_cached: Vec<CachedRemoteSessionDigest> =
                    cached_sessions.into_iter().filter(|s| !seen_ids.contains(&s.id)).collect();
                hits.extend(search_cached_conversations(&remaining_cached, &needle_conv, MAX_CONVERSATION_HITS - hits.len()));
            }
            hits
        })
        .await
        .unwrap_or_default();
        if !conv_hits.is_empty() {
            sections.push(format!("Conversation matches in '{ws_name}' ({}):\n{}", conv_hits.len(), conv_hits.join("\n")));
        }
    }

    if matches!(scope.as_str(), "all" | "files") {
        if ws.origin == WorkspaceOrigin::Local {
            let root = PathBuf::from(&ws.path);
            let needle_clone = needle.clone();
            let file_hits = tauri::async_runtime::spawn_blocking(move || search_files_in_dir(&root, &needle_clone, MAX_FILE_HITS))
                .await
                .unwrap_or_default();
            if !file_hits.is_empty() {
                sections.push(format!("File matches in '{ws_name}' ({}):\n{}", file_hits.len(), file_hits.join("\n")));
            }
        } else if scope == "files" {
            sections.push(format!(
                "Workspace '{ws_name}' is {} (online: {}); remote source files are not mounted on this machine. Use scope 'memory' or 'conversations', or read '@memory'.",
                ws.origin.as_str(),
                ws.online
            ));
        }
    }

    if sections.is_empty() {
        WorkspaceResponse::ok(format!("No matches for '{query}' in workspace '{ws_name}' (scope: {scope})."))
    } else {
        WorkspaceResponse::ok(sections.join("\n\n"))
    }
}

/// Bounded recursive text search inside `root`, skipping ignored directories, symlinks, binary files, and sensitive files.
pub fn search_files_in_dir(root: &Path, needle_lower: &str, max_hits: usize) -> Vec<String> {
    let Ok(canonical_root) = std::fs::canonicalize(root) else { return Vec::new() };
    let mut hits = Vec::new();
    let mut scanned = 0usize;
    let mut visited = 0usize;
    let mut stack = vec![canonical_root.clone()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        let mut sorted: Vec<_> = entries.flatten().collect();
        sorted.sort_by_key(|e| e.file_name());
        for entry in sorted {
            // Every entry walked counts, folders and skipped files too, so a huge tree (a home folder) still ends soon.
            visited += 1;
            if scanned >= MAX_SCANNED_FILES || visited >= MAX_VISITED_ENTRIES || hits.len() >= max_hits {
                return hits;
            }
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_symlink() {
                continue;
            }
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
            if ft.is_dir() {
                if !is_skipped_dir_name(name) && !matches!(name, ".ssh" | ".aws" | ".gnupg" | ".kube" | ".docker" | ".azure" | ".gcloud") {
                    stack.push(path);
                }
                continue;
            }
            if !ft.is_file() {
                continue;
            }
            let Ok(rel) = path.strip_prefix(&canonical_root) else { continue };
            if is_sensitive_relative_path(rel) {
                continue;
            }
            let Ok(meta) = entry.metadata() else { continue };
            if meta.len() > MAX_FILE_BYTES {
                continue;
            }
            scanned += 1;
            let rel_display = rel.to_string_lossy().replace('\\', "/");
            if rel_display.to_lowercase().contains(needle_lower) {
                hits.push(format!("{rel_display}: (file path match)"));
                if hits.len() >= max_hits {
                    return hits;
                }
            }
            let Ok(bytes) = std::fs::read(&path) else { continue };
            if bytes[..bytes.len().min(8192)].contains(&0) {
                continue;
            }
            let Ok(text) = std::str::from_utf8(&bytes) else { continue };
            for (idx, line) in text.lines().enumerate() {
                if line.to_lowercase().contains(needle_lower) {
                    hits.push(format!("{rel_display}:{}: {}", idx + 1, truncate_single_line(line, 180)));
                    if hits.len() >= max_hits {
                        return hits;
                    }
                }
            }
        }
    }
    hits
}

/// Formats shared memory entries (project + user global) as a virtual markdown document (`@memory` / `MEMORY.md`).
pub fn render_virtual_memory_document(ws_name: &str, entries: &[SharedMemoryEntry]) -> String {
    if entries.is_empty() {
        return format!("# Shared Memory ({ws_name})\n\nNo shared memory entries recorded for workspace '{ws_name}'.\n");
    }
    let mut out = format!("# Shared Memory ({ws_name})\n\n");
    for entry in entries {
        if entry.deleted {
            continue;
        }
        let scope_label = if entry.scope.is_user() { "user" } else { "project" };
        let pinned_note = if entry.pinned { " · pinned" } else { "" };
        let tags_note = if entry.tags.is_empty() { String::new() } else { format!(" · tags: {}", entry.tags.join(", ")) };
        out.push_str(&format!(
            "## [{}:{}] {}{}{}\n_rev {} · updated {} by {}_\n\n{}\n\n",
            scope_label,
            entry.key,
            entry.title,
            pinned_note,
            tags_note,
            entry.revision,
            entry.updated_at,
            if entry.updated_by_device_name.is_empty() { "Mighty Code" } else { &entry.updated_by_device_name },
            entry.content.trim()
        ));
    }
    out
}

fn is_virtual_memory_path(ws: &ResolvedWorkspace, raw_path: &str) -> bool {
    let trimmed = raw_path.trim().trim_start_matches("./");
    if trimmed.eq_ignore_ascii_case("@memory") {
        return true;
    }
    if trimmed.eq_ignore_ascii_case("memory.md") {
        if ws.origin != WorkspaceOrigin::Local {
            return true;
        }
        return !Path::new(&ws.path).join("MEMORY.md").is_file();
    }
    false
}

async fn read_workspace_file(host: &Host, snapshot: &AppSnapshot, request: &WorkspaceRequest) -> WorkspaceResponse {
    let target_arg = request.arguments.get("workspace").map(String::as_str).unwrap_or("");
    let path_arg = request.arguments.get("path").map(String::as_str).unwrap_or("").to_string();
    let start_line = request.arguments.get("start_line").and_then(|s| s.trim().parse::<usize>().ok()).unwrap_or(1);
    let end_line = request.arguments.get("end_line").and_then(|s| s.trim().parse::<usize>().ok());

    let (workspaces, book, cached_states) = gather_host_lookup_context(host, snapshot);
    let Some(ws) = resolve_any_workspace(&workspaces, target_arg).cloned() else {
        return WorkspaceResponse::failure(format!(
            "Workspace '{target_arg}' not found. Call list_workspaces first to see available workspace names and ids."
        ));
    };

    if is_virtual_memory_path(&ws, &path_arg) {
        let entries = memory_entries_for_resolved(&ws, &book, &cached_states, true);
        let doc = render_virtual_memory_document(&ws.name, &entries);
        return match slice_numbered_text("@memory", &doc, start_line, end_line) {
            Ok(sliced) => WorkspaceResponse::ok(sliced),
            Err(err) => WorkspaceResponse::failure(err),
        };
    }

    if ws.origin != WorkspaceOrigin::Local {
        return WorkspaceResponse::failure(format!(
            "Workspace '{}' is {} (online: {}); remote disk files cannot be read directly here. Pass path='@memory' (or 'MEMORY.md') to read shared memory, or use search_workspace / recent_workspace_activity.",
            ws.name,
            ws.origin.as_str(),
            ws.online
        ));
    }

    let root = PathBuf::from(&ws.path);
    match tauri::async_runtime::spawn_blocking(move || read_bounded_lines(&root, &path_arg, start_line, end_line)).await {
        Ok(Ok(content)) => WorkspaceResponse::ok(content),
        Ok(Err(error)) => WorkspaceResponse::failure(error),
        Err(error) => WorkspaceResponse::failure(format!("Read task failed: {error}")),
    }
}

pub fn format_recent_activity_base(
    ws: &ResolvedWorkspace,
    live_panes: &[&Arc<RunSession>],
    memory_entries: &[SharedMemoryEntry],
) -> Vec<String> {
    let branch = match ws.origin {
        WorkspaceOrigin::Local => git_head_branch(Path::new(&ws.path)).unwrap_or_else(|| "-".into()),
        WorkspaceOrigin::Remote | WorkspaceOrigin::RemoteCache => ws.cached_branch.clone().unwrap_or_else(|| "-".into()),
    };
    let mut out = Vec::new();
    out.push(format!("Workspace: {} (origin: {}, online: {}, branch: {})", ws.name, ws.origin.as_str(), ws.online, branch));

    if !live_panes.is_empty() {
        out.push(format!("Agent panes ({}):", live_panes.len()));
        for pane in live_panes.iter().rev().take(8) {
            let last_user = pane
                .logs()
                .iter()
                .rev()
                .find(|e| e.kind.as_deref() == Some("user") && !e.text.as_deref().unwrap_or("").trim().is_empty())
                .map(|e| truncate_single_line(e.text.as_deref().unwrap_or(""), 140));
            let last_assistant = pane
                .logs()
                .iter()
                .rev()
                .find(|e| e.kind.as_deref() == Some("assistant") && !e.text.as_deref().unwrap_or("").trim().is_empty())
                .map(|e| truncate_single_line(e.text.as_deref().unwrap_or(""), 200));
            out.push(format!(
                "- {} ({}, status: {}, bodyRevision: {})",
                truncate_single_line(pane.title(), 70),
                pane.provider(),
                pane.status(),
                pane.body_revision.unwrap_or_default()
            ));
            if let Some(q) = last_user {
                out.push(format!("  request: {q}"));
            }
            if let Some(a) = last_assistant {
                out.push(format!("  summary: {a}"));
            }
        }
    } else if !ws.cached_sessions.is_empty() {
        out.push(format!("Cached remote agent panes ({}):", ws.cached_sessions.len()));
        for session in ws.cached_sessions.iter().rev().take(8) {
            let last_user = session
                .tail
                .iter()
                .rev()
                .find(|l| l.kind == "user" && !l.text.trim().is_empty())
                .map(|l| truncate_single_line(&l.text, 140));
            let last_assistant = session
                .tail
                .iter()
                .rev()
                .find(|l| l.kind == "assistant" && !l.text.trim().is_empty())
                .map(|l| truncate_single_line(&l.text, 200));
            out.push(format!(
                "- {} ({}, status: {}, bodyRevision: {})",
                truncate_single_line(&session.title, 70),
                session.provider,
                session.status,
                session.body_revision
            ));
            if let Some(q) = last_user {
                out.push(format!("  request: {q}"));
            }
            if let Some(a) = last_assistant {
                out.push(format!("  summary: {a}"));
            }
        }
    } else {
        out.push("Agent panes: none open.".into());
    }

    if !memory_entries.is_empty() {
        out.push(format!("Shared memory ({}):", memory_entries.len()));
        for entry in memory_entries.iter().take(8) {
            let scope_tag = if entry.scope.is_user() { "user" } else { "project" };
            out.push(format!(
                "- [{}:{}] {} (rev {}, updated {}): {}",
                scope_tag,
                entry.key,
                truncate_single_line(&entry.title, 50),
                entry.revision,
                entry.updated_at,
                truncate_single_line(&entry.content, 160)
            ));
        }
    }

    out
}

async fn recent_workspace_activity(host: &Arc<Host>, snapshot: &AppSnapshot, request: &WorkspaceRequest) -> WorkspaceResponse {
    let target_arg = request.arguments.get("workspace").map(String::as_str).unwrap_or("");
    let (workspaces, book, cached_states) = gather_host_lookup_context(host, snapshot);
    let Some(ws) = resolve_any_workspace(&workspaces, target_arg).cloned() else {
        return WorkspaceResponse::failure(format!(
            "Workspace '{target_arg}' not found. Call list_workspaces first to see available workspace names and ids."
        ));
    };

    let panes: Vec<&Arc<RunSession>> = snapshot.sessions().iter().filter(|s| s.workspace_id() == ws.id && s.kind() == "claude").collect();
    let memory_entries = memory_entries_for_resolved(&ws, &book, &cached_states, true);
    let mut out = format_recent_activity_base(&ws, &panes, &memory_entries);

    if ws.origin == WorkspaceOrigin::Local {
        let root = PathBuf::from(&ws.path);
        if let Some(git) = crate::workspace_create::git_executable()
            && root.is_dir()
        {
            let log_req = SpawnRequest::new(git.clone(), ["log", "-n", "5", "--oneline", "--no-decorate"], root.clone())
                .env("GIT_TERMINAL_PROMPT", "0");
            if let Ok(captured) =
                mighty_platform::process::capture(host.processes.as_ref(), log_req, None, Duration::from_secs(3), 16_384).await
                && captured.exit_code == 0
            {
                let commits = String::from_utf8_lossy(&captured.stdout).trim().to_string();
                if !commits.is_empty() {
                    out.push(format!("Recent git commits:\n{commits}"));
                }
            }
            let status_req = SpawnRequest::new(git, ["status", "--short"], root).env("GIT_TERMINAL_PROMPT", "0");
            if let Ok(captured) =
                mighty_platform::process::capture(host.processes.as_ref(), status_req, None, Duration::from_secs(3), 16_384).await
                && captured.exit_code == 0
            {
                let stdout = String::from_utf8_lossy(&captured.stdout);
                let dirty: Vec<&str> = stdout
                    .lines()
                    .filter(|line| {
                        let path_part = line.get(3..).unwrap_or("").trim();
                        !is_sensitive_relative_path(Path::new(path_part))
                    })
                    .take(20)
                    .collect();
                if !dirty.is_empty() {
                    out.push(format!("Uncommitted changes:\n{}", dirty.join("\n")));
                }
            }
        }
    }

    WorkspaceResponse::ok(out.join("\n"))
}

#[cfg(test)]
mod tests {
    use mighty_core::shared_memory::{CachedRemoteWorkspace, CachedSessionLine, MemoryScope, UpsertMemoryInput};

    use super::*;

    fn temp_workspace(label: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("mighty-ws-mcp-{label}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn sensitive_paths_are_blocked_while_templates_and_source_files_pass() {
        for blocked in [
            ".env",
            ".env.local",
            ".env.production",
            "config/.env.staging",
            "certs/server.pem",
            "certs/private.key",
            "keys/id_ed25519",
            "firebase/google-services.json",
            "ios/GoogleService-Info.plist",
            "gcp/service-account-prod.json",
            "oauth/client_secret_123.json",
            ".git/config",
            ".ssh/config",
            ".npmrc",
            "relay/cloudflare/.dev.vars",
            ".envrc",
            ".git-credentials",
            "keys/AuthKey_ABC123.p8",
            "home/credentials",
            ".codex/auth.json",
            "gh/hosts.yml",
            ".pgpass",
            ".vault-token",
            "infra/prod.tfvars",
            "infra/terraform.tfstate",
            ".docker/config.json",
        ] {
            assert!(is_sensitive_relative_path(Path::new(blocked)), "must block {blocked}");
        }
        for allowed in [".env.example", ".env.sample", ".env.template", "src/env.ts", "docs/security.md", "package.json", "CLAUDE.md"] {
            assert!(!is_sensitive_relative_path(Path::new(allowed)), "must allow {allowed}");
        }
    }

    #[test]
    fn read_and_search_enforce_root_boundary_and_secret_filtering() {
        let root = temp_workspace("boundary");
        let outside = temp_workspace("outside");
        std::fs::write(outside.join("secret.txt"), "outside-secret").unwrap();
        std::fs::write(root.join(".env"), "API_KEY=top-secret").unwrap();
        std::fs::write(root.join(".env.example"), "API_KEY=placeholder").unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/schema.ts"), "export interface UserDto {\n  id: string;\n  email: string;\n}\n").unwrap();

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(outside.join("secret.txt"), root.join("src/escaped.txt")).unwrap();
            assert!(read_bounded_lines(&root, "src/escaped.txt", 1, Some(10)).is_err(), "escaping symlink must be refused");
        }

        assert!(read_bounded_lines(&root, "../outside/secret.txt", 1, Some(10)).is_err(), "traversal must be refused");
        assert!(read_bounded_lines(&root, ".env", 1, Some(10)).is_err(), ".env must be refused");
        let template = read_bounded_lines(&root, ".env.example", 1, Some(10)).unwrap();
        assert!(template.contains("API_KEY=placeholder"));

        let dto = read_bounded_lines(&root, "src/schema.ts", 2, Some(3)).unwrap();
        assert!(dto.contains("(lines 2-3 of 4)") && dto.contains("id: string;") && !dto.contains("export interface"));

        let secret_hits = search_files_in_dir(&root, "top-secret", 20);
        assert!(secret_hits.is_empty(), "search must skip .env secrets");
        let code_hits = search_files_in_dir(&root, "userdto", 20);
        assert_eq!(code_hits.len(), 1);
        assert!(code_hits[0].starts_with("src/schema.ts:1:"));

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[test]
    fn resolve_workspace_accepts_id_name_mention_slug_and_ignores_remote() {
        let local = Workspace {
            id: Some("ws-1".into()),
            name: Some("Mighty Server".into()),
            path: Some("/Users/dev/mighty-server".into()),
            ..Workspace::default()
        };
        let remote = Workspace {
            id: Some("ws-remote".into()),
            name: Some("Remote Box".into()),
            path: Some("/srv/box".into()),
            remote: Some(mighty_core::model::RemoteWorkspaceReference {
                connection_id: Some("c1".into()),
                workspace_id: Some("w1".into()),
                host_name: Some("box".into()),
                version: 2,
                mirror: false,
            }),
            ..Workspace::default()
        };
        let list = [local, remote];
        assert_eq!(resolve_local_workspace(&list, "ws-1").unwrap().id(), "ws-1");
        assert_eq!(resolve_local_workspace(&list, "mighty server").unwrap().id(), "ws-1");
        assert_eq!(resolve_local_workspace(&list, "@workspace:Mighty-Server").unwrap().id(), "ws-1");
        assert_eq!(resolve_local_workspace(&list, "mighty-server").unwrap().id(), "ws-1");
        assert!(resolve_local_workspace(&list, "Remote Box").is_none(), "remote workspaces are not local lookup targets");
    }

    #[test]
    fn offline_remote_cache_workspaces_and_virtual_memory_work_seamlessly() {
        let mut book = SharedMemoryBook::new("dev-a", "Studio Mac");
        let (user_entry, _) = book
            .upsert(UpsertMemoryInput {
                id: None,
                scope: MemoryScope::User,
                key: "style-guide".into(),
                title: "Style Guide".into(),
                content: "Prefer concise Rust modules and explicit error types.".into(),
                tags: vec!["rust".into()],
                auto_inject: true,
                pinned: true,
            })
            .unwrap();
        let (proj_entry, _) = book
            .upsert(UpsertMemoryInput {
                id: None,
                scope: MemoryScope::Project { workspace_id: "host-ws-9".into(), project_slug: "acme/payments".into() },
                key: "db-port".into(),
                title: "Database Port".into(),
                content: "Integration Postgres runs on port 55432.".into(),
                tags: vec!["db".into()],
                auto_inject: true,
                pinned: false,
            })
            .unwrap();

        let cached_state = CachedRemoteState {
            connection_id: "conn-offline".into(),
            host_name: "Linux Build Box".into(),
            cached_at: "2026-11-22T12:00:00Z".into(),
            workspaces: vec![CachedRemoteWorkspace {
                id: "host-ws-9".into(),
                name: "Payments API".into(),
                path: "/srv/acme/payments".into(),
                project_slug: "acme/payments".into(),
                branch: Some("feat/ledger".into()),
            }],
            sessions: vec![CachedRemoteSessionDigest {
                id: "pane-77".into(),
                workspace_id: "host-ws-9".into(),
                title: "Fix settlement rounding".into(),
                provider: "claude".into(),
                model: "claude-sonnet-4-6".into(),
                status: "completed".into(),
                body_revision: "rev-42".into(),
                branch: Some("feat/ledger".into()),
                pending: 0,
                queued: 0,
                tail: vec![
                    CachedSessionLine { kind: "user".into(), text: "Audit settlement rounding edge cases".into() },
                    CachedSessionLine { kind: "assistant".into(), text: "Updated half-even rounding in ledger.rs".into() },
                ],
            }],
            memory_entries: vec![proj_entry],
        };

        let snapshot = AppSnapshot::default();
        let online = HashSet::new();
        let resolved = collect_all_workspaces(&snapshot, std::slice::from_ref(&cached_state), &online);
        assert_eq!(resolved.len(), 1);
        let ws = resolve_any_workspace(&resolved, "@workspace:Payments-API").expect("resolves offline cached workspace");
        assert_eq!(ws.origin, WorkspaceOrigin::RemoteCache);
        assert!(!ws.online);

        let listed = format_workspace_list(&resolved, &[], &book, std::slice::from_ref(&cached_state), "");
        assert!(listed.contains("origin: remote-cache, online: false, sharedMemoryCount: 2"));
        assert!(listed.contains("branch: feat/ledger"));

        let entries = memory_entries_for_resolved(ws, &book, std::slice::from_ref(&cached_state), true);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].id, user_entry.id, "pinned user entry sorts first");

        let mem_hits = search_memory_entries(&entries, "55432", 10);
        assert_eq!(mem_hits.len(), 1);
        assert!(mem_hits[0].contains("[project:db-port]"));

        let conv_hits = search_cached_conversations(&ws.cached_sessions, "half-even", 10);
        assert_eq!(conv_hits.len(), 1);

        let virtual_doc = render_virtual_memory_document(&ws.name, &entries);
        assert!(virtual_doc.contains("[user:style-guide]"));
        assert!(virtual_doc.contains("[project:db-port]"));

        let activity = format_recent_activity_base(ws, &[], &entries).join("\n");
        assert!(activity.contains("origin: remote-cache, online: false, branch: feat/ledger"));
        assert!(activity.contains("bodyRevision: rev-42"));
        assert!(activity.contains("Shared memory (2):"));
    }
}
