//! App-side handler for `mighty-workspace` (`--agent-workspace-mcp`, `MC-14`): strictly read-only, on-demand lookup across the
//! local workspaces registered in Mighty Code (`list_workspaces`, `search_workspace`, `read_workspace_file`,
//! `recent_workspace_activity`).
//!
//! Security boundaries:
//! - Disabled immediately whenever `AppSnapshot::cross_workspace_lookup_on()` is false.
//! - Only local workspaces present in `AppSnapshot::workspaces()` (`remote.is_none()`) are visible.
//! - Sensitive files (`.env*` except templates, keys/certs, credential JSONs, `.git/**`, `.ssh/**`, etc.) are refused on both search
//!   and read.
//! - File reads canonicalize both the workspace root and target path, refusing path traversal (`..`) and symlinks escaping the root.

use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Weak};
use std::time::Duration;

use mighty_core::model::{AppSnapshot, RunSession, Workspace};
use mighty_delegation::mcp::workspace::{DETACHED_MESSAGE, WorkspaceCaller, WorkspaceHandler, WorkspaceRequest, WorkspaceResponse};
use mighty_platform::{BoxFuture, SpawnRequest};

use crate::host::Host;

const MAX_FILE_BYTES: u64 = 512 * 1024;
const MAX_READ_LINES: usize = 250;
const MAX_OUTPUT_BYTES: usize = 32 * 1024;
const MAX_SCANNED_FILES: usize = 2_500;
const MAX_FILE_HITS: usize = 40;
const MAX_CONVERSATION_HITS: usize = 15;

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
                "list_workspaces" => list_workspaces(&snapshot, &caller),
                "search_workspace" => search_workspace(&snapshot, &request).await,
                "read_workspace_file" => read_workspace_file(&snapshot, &request).await,
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
pub fn resolve_local_workspace<'a>(workspaces: &'a [Workspace], raw_query: &str) -> Option<&'a Workspace> {
    let trimmed = raw_query.trim();
    let stripped = trimmed
        .strip_prefix("@workspace:")
        .or_else(|| trimmed.strip_prefix("workspace:"))
        .or_else(|| trimmed.strip_prefix('@'))
        .unwrap_or(trimmed)
        .trim()
        .trim_matches('"');
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
        if matches!(dir.as_str(), ".git" | ".ssh" | ".aws" | ".gnupg" | ".kube") {
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
    ) {
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
        Some("pem" | "key" | "p12" | "pfx" | "jks" | "keystore" | "kdbx" | "der" | "cer" | "crt")
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
    let all_lines: Vec<&str> = text.lines().collect();
    let total = all_lines.len();
    if total == 0 {
        return Ok(format!("{relative} (empty file, 0 lines)"));
    }
    let start = start_line.max(1).min(total);
    let requested_end = end_line.unwrap_or(start + MAX_READ_LINES - 1).max(start);
    let end = requested_end.min(start + MAX_READ_LINES - 1).min(total);
    let mut out = format!("{relative} (lines {start}-{end} of {total})\n");
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
        Some(prompt) if prompt != title => format!("{} [{}] — latest: {}", title, latest.status(), prompt),
        _ => format!("{} [{}]", title, latest.status()),
    })
}

fn truncate_single_line(text: &str, max_chars: usize) -> String {
    let collapsed: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = collapsed.chars();
    let head: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() { format!("{head}…") } else { head }
}

fn list_workspaces(snapshot: &AppSnapshot, caller: &WorkspaceCaller) -> WorkspaceResponse {
    let locals: Vec<&Workspace> = snapshot.workspaces().iter().filter(|w| w.remote.is_none() && !w.path().is_empty()).collect();
    if locals.is_empty() {
        return WorkspaceResponse::ok("No local workspaces are registered in Mighty Code.");
    }
    let mut lines = Vec::with_capacity(locals.len() + 1);
    lines.push(format!("Registered local workspaces ({}):", locals.len()));
    for ws in locals {
        let marker = if ws.id() == caller.workspace_id { " [current]" } else { "" };
        let path = Path::new(ws.path());
        let folder = path.file_name().and_then(|n| n.to_str()).unwrap_or(ws.path());
        let branch = git_head_branch(path).unwrap_or_else(|| "-".into());
        let pane_count = snapshot.sessions().iter().filter(|s| s.workspace_id() == ws.id() && s.kind() == "claude").count();
        let slug = mention_slug(ws.name());
        let mut line = format!(
            "- {} (id: {}, mention: @workspace:{}, folder: {}, branch: {}, agent panes: {}){}",
            ws.name(),
            ws.id(),
            slug,
            folder,
            branch,
            pane_count,
            marker
        );
        if let Some(summary) = latest_session_summary(snapshot.sessions(), ws.id()) {
            line.push_str(&format!("\n  latest session: {summary}"));
        }
        lines.push(line);
    }
    WorkspaceResponse::ok(lines.join("\n"))
}

async fn search_workspace(snapshot: &AppSnapshot, request: &WorkspaceRequest) -> WorkspaceResponse {
    let target_arg = request.arguments.get("workspace").map(String::as_str).unwrap_or("");
    let query = request.arguments.get("query").map(String::as_str).unwrap_or("").trim().to_string();
    let scope = request.arguments.get("scope").map(|s| s.trim().to_ascii_lowercase()).unwrap_or_else(|| "all".into());
    if query.is_empty() {
        return WorkspaceResponse::failure("Search query must not be empty.");
    }
    if !matches!(scope.as_str(), "all" | "files" | "conversations") {
        return WorkspaceResponse::failure("Invalid scope: expected 'all', 'files', or 'conversations'.");
    }
    let Some(ws) = resolve_local_workspace(snapshot.workspaces(), target_arg) else {
        return WorkspaceResponse::failure(format!(
            "Workspace '{target_arg}' not found. Call list_workspaces first to see available workspace names and ids."
        ));
    };
    let ws_name = ws.name().to_string();
    let ws_id = ws.id().to_string();
    let root = PathBuf::from(ws.path());
    let needle = query.to_lowercase();

    let mut sections = Vec::new();
    if scope != "files" {
        let mut conv_hits = Vec::new();
        for session in snapshot.sessions().iter().filter(|s| s.workspace_id() == ws_id && s.kind() == "claude") {
            if session.title().to_lowercase().contains(&needle) {
                conv_hits.push(format!("- [pane: {} ({})] title matched", truncate_single_line(session.title(), 70), session.provider()));
            }
            for entry in session.logs() {
                let kind = entry.kind.as_deref().unwrap_or("");
                let text = entry.text.as_deref().unwrap_or("");
                if matches!(kind, "user" | "assistant") && text.to_lowercase().contains(&needle) {
                    conv_hits.push(format!(
                        "- [pane: {} · {}] {}",
                        truncate_single_line(session.title(), 48),
                        kind,
                        truncate_single_line(text, 160)
                    ));
                    if conv_hits.len() >= MAX_CONVERSATION_HITS {
                        break;
                    }
                }
            }
            if conv_hits.len() >= MAX_CONVERSATION_HITS {
                break;
            }
        }
        if !conv_hits.is_empty() {
            sections.push(format!("Conversation matches in '{ws_name}' ({}):\n{}", conv_hits.len(), conv_hits.join("\n")));
        }
    }

    if scope != "conversations" {
        let needle_clone = needle.clone();
        let file_hits = tauri::async_runtime::spawn_blocking(move || search_files_in_dir(&root, &needle_clone, MAX_FILE_HITS))
            .await
            .unwrap_or_default();
        if !file_hits.is_empty() {
            sections.push(format!("File matches in '{ws_name}' ({}):\n{}", file_hits.len(), file_hits.join("\n")));
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
    let mut stack = vec![canonical_root.clone()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        let mut sorted: Vec<_> = entries.flatten().collect();
        sorted.sort_by_key(|e| e.file_name());
        for entry in sorted {
            if scanned >= MAX_SCANNED_FILES || hits.len() >= max_hits {
                return hits;
            }
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_symlink() {
                continue;
            }
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
            if ft.is_dir() {
                if !is_skipped_dir_name(name) && !matches!(name, ".ssh" | ".aws" | ".gnupg" | ".kube") {
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

async fn read_workspace_file(snapshot: &AppSnapshot, request: &WorkspaceRequest) -> WorkspaceResponse {
    let target_arg = request.arguments.get("workspace").map(String::as_str).unwrap_or("");
    let path_arg = request.arguments.get("path").map(String::as_str).unwrap_or("").to_string();
    let start_line = request.arguments.get("start_line").and_then(|s| s.trim().parse::<usize>().ok()).unwrap_or(1);
    let end_line = request.arguments.get("end_line").and_then(|s| s.trim().parse::<usize>().ok());
    let Some(ws) = resolve_local_workspace(snapshot.workspaces(), target_arg) else {
        return WorkspaceResponse::failure(format!(
            "Workspace '{target_arg}' not found. Call list_workspaces first to see available workspace names and ids."
        ));
    };
    let root = PathBuf::from(ws.path());
    match tauri::async_runtime::spawn_blocking(move || read_bounded_lines(&root, &path_arg, start_line, end_line)).await {
        Ok(Ok(content)) => WorkspaceResponse::ok(content),
        Ok(Err(error)) => WorkspaceResponse::failure(error),
        Err(error) => WorkspaceResponse::failure(format!("Read task failed: {error}")),
    }
}

async fn recent_workspace_activity(host: &Arc<Host>, snapshot: &AppSnapshot, request: &WorkspaceRequest) -> WorkspaceResponse {
    let target_arg = request.arguments.get("workspace").map(String::as_str).unwrap_or("");
    let Some(ws) = resolve_local_workspace(snapshot.workspaces(), target_arg) else {
        return WorkspaceResponse::failure(format!(
            "Workspace '{target_arg}' not found. Call list_workspaces first to see available workspace names and ids."
        ));
    };
    let ws_name = ws.name().to_string();
    let ws_id = ws.id().to_string();
    let root = PathBuf::from(ws.path());
    let branch = git_head_branch(&root).unwrap_or_else(|| "-".into());

    let mut out = Vec::new();
    out.push(format!("Workspace: {ws_name} (branch: {branch})"));

    let panes: Vec<&Arc<RunSession>> = snapshot.sessions().iter().filter(|s| s.workspace_id() == ws_id && s.kind() == "claude").collect();
    if panes.is_empty() {
        out.push("Agent panes: none open.".into());
    } else {
        out.push(format!("Agent panes ({}):", panes.len()));
        for pane in panes.iter().rev().take(8) {
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
            out.push(format!("- {} ({}, status: {})", truncate_single_line(pane.title(), 70), pane.provider(), pane.status()));
            if let Some(q) = last_user {
                out.push(format!("  request: {q}"));
            }
            if let Some(a) = last_assistant {
                out.push(format!("  summary: {a}"));
            }
        }
    }

    if let Some(git) = crate::workspace_create::git_executable()
        && root.is_dir()
    {
        let log_req =
            SpawnRequest::new(git.clone(), ["log", "-n", "5", "--oneline", "--no-decorate"], root.clone()).env("GIT_TERMINAL_PROMPT", "0");
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

    WorkspaceResponse::ok(out.join("\n"))
}

#[cfg(test)]
mod tests {
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
}
