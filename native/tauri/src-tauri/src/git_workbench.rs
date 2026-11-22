//! Git Workbench & automated PR / CI failure fix loop (`MC-5`):
//! - Structured porcelain v2 repository status, local branch listing, and GitHub / GitLab hosting detection
//! - File-level and hunk-level staging / unstaging (`git apply --cached --unidiff-zero`) and safe branch switching / creation
//! - Pre-commit secret & sensitive file scanner (never exposing matched secret values)
//! - One-shot headless Claude Conventional Commit message drafting with deterministic heuristic fallback
//! - Multi-file or staged-only commit, upstream-aware push, PR / MR status & creation (`gh` / `glab`), and 80-line ANSI-stripped failed CI log tail

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use mighty_core::model::{PaneWorktree, Workspace};
use mighty_platform::SpawnRequest;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::host::{Host, Registry};
use crate::methods::{flag, text};

const MAX_GIT_OUTPUT_BYTES: usize = 4 * 1024 * 1024;
const MAX_UNTRACKED_DIFF_BYTES: usize = 512 * 1024;
const MAX_SECRET_SCAN_FILE_BYTES: u64 = 1024 * 1024;
const MAX_RENDERED_DIFF_LINES: usize = 4_000;
const MAX_DRAFT_DIFF_CHARS: usize = 10_000;
const FAILED_LOG_TAIL_LINES: usize = 80;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitWorkbenchFile {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_path: Option<String>,
    pub mark: String,
    pub index_status: String,
    pub worktree_status: String,
    pub staged: bool,
    pub unstaged: bool,
    pub untracked: bool,
    pub conflicted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParsedPorcelainStatus {
    pub head: String,
    pub detached: bool,
    pub upstream: Option<String>,
    pub ahead: i32,
    pub behind: i32,
    pub files: Vec<GitWorkbenchFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitDiffLine {
    pub kind: String,
    pub old_line: Option<usize>,
    pub new_line: Option<usize>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitDiffHunk {
    pub index: usize,
    pub header: String,
    pub old_start: usize,
    pub old_lines: usize,
    pub new_start: usize,
    pub new_lines: usize,
    pub raw_patch: String,
    pub lines: Vec<GitDiffLine>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitStructuredDiff {
    pub path: String,
    pub staged: bool,
    pub hunks: Vec<GitDiffHunk>,
    pub added: usize,
    pub removed: usize,
    pub binary: bool,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretFinding {
    pub path: String,
    pub line: usize,
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitPrInfo {
    pub number: i64,
    pub url: String,
    pub state: String,
    pub title: String,
    pub is_draft: bool,
    pub checks: String,
}

static SECRET_PATTERNS: LazyLock<Vec<(&'static str, Regex)>> = LazyLock::new(|| {
    [
        ("AWS access key", r"\b(AKIA|ASIA)[0-9A-Z]{16}\b"),
        ("GitHub token", r"\b(ghp|gho|ghu|ghs|ghr)_[A-Za-z0-9]{36,}\b|\bgithub_pat_[A-Za-z0-9_]{60,}\b"),
        ("GitLab token", r"\bglpat-[A-Za-z0-9_-]{20,}\b"),
        ("Slack token", r"\bxox[abprs]-[A-Za-z0-9-]{10,}\b"),
        ("Anthropic key", r"\bsk-ant-[A-Za-z0-9_-]{20,}\b"),
        ("OpenAI key", r"\bsk-(proj-)?[A-Za-z0-9_-]{32,}\b"),
        ("Google API key", r"\bAIza[0-9A-Za-z_-]{35}\b"),
        ("Stripe key", r"\b(sk|rk)_live_[0-9A-Za-z]{20,}\b"),
        ("Private key", r"-----BEGIN ([A-Z]+ )?PRIVATE KEY-----"),
    ]
    .into_iter()
    .map(|(kind, pat)| (kind, Regex::new(pat).expect("valid secret regex")))
    .collect()
});

static HUNK_HEADER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@").expect("valid hunk header regex"));

static ANSI_ESCAPE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\x1b\[[0-9;?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)").expect("valid ansi regex"));

fn resolve_local_workspace_root(host: &Host, requested_id: Option<&str>) -> Result<(Workspace, PathBuf), String> {
    let ws = {
        let view = host.view();
        let target_id = requested_id.filter(|id| !id.trim().is_empty()).or(view.snapshot.active_workspace_id.as_deref());
        target_id.and_then(|id| view.snapshot.workspace(id)).cloned()
    };
    let Some(ws) = ws else {
        return Err(host.locale.get("window.error.addWorkspaceFirst"));
    };
    if ws.remote.is_some() {
        return Err("Git Workbench is only available for local workspaces.".into());
    }
    let raw_path = ws.path().trim();
    if raw_path.is_empty() {
        return Err("Workspace folder is not configured.".into());
    }
    let root = PathBuf::from(raw_path);
    if !root.is_dir() {
        return Err("Workspace folder does not exist on disk.".into());
    }
    Ok((ws, root))
}

/// The workspace and the folder a workbench call works in: the run folder of the worktree of the pane named by `paneId` (a worktree
/// pane's `/git` must commit and push its own branch, not the main checkout's), else the workspace folder.
fn resolve_git_target(host: &Host, args: &Value) -> Result<(Workspace, PathBuf), String> {
    let (ws, root) = resolve_local_workspace_root(host, text(args, "workspaceId"))?;
    let Some(pane_id) = text(args, "paneId").map(str::trim).filter(|id| !id.is_empty()) else { return Ok((ws, root)) };
    let record = {
        let view = host.view();
        view.snapshot.session(pane_id).filter(|pane| pane.workspace_id() == ws.id()).and_then(|pane| pane.pane_worktree.clone())
    };
    let Some(record) = record else { return Ok((ws, root)) };
    let folder = PathBuf::from(record.run_folder());
    if !folder.is_dir() {
        return Err("The pane's worktree folder does not exist on disk.".into());
    }
    Ok((ws, folder))
}

/// The base branch of the worktree of the pane named by `paneId`, when it has one.
fn worktree_base_branch(host: &Host, args: &Value) -> Option<String> {
    let pane_id = text(args, "paneId").map(str::trim).filter(|id| !id.is_empty())?;
    let view = host.view();
    view.snapshot.session(pane_id)?.pane_worktree.as_ref()?.base_branch.clone().filter(|base| !base.is_empty())
}

/// Whether an agent pane is running in `folder` (its worktree's run folder, or the workspace folder for a pane without one).
fn agent_running_in(host: &Host, workspace: &Workspace, folder: &Path) -> bool {
    let view = host.view();
    view.snapshot.sessions().iter().any(|pane| {
        if pane.workspace_id() != workspace.id() || pane.kind() != "claude" || pane.status() != "running" {
            return false;
        }
        let pane_folder =
            pane.pane_worktree.as_ref().map_or_else(|| workspace.path().to_string(), |record| record.run_folder().to_string());
        std::fs::canonicalize(&pane_folder).ok() == std::fs::canonicalize(folder).ok()
    })
}

fn ensure_local_repo_write(host: &Host, root: &Path) -> Result<(), String> {
    if !(host.writes() || host.may_touch_repo(root)) {
        return Err(crate::setup::verification_mode());
    }
    Ok(())
}

fn ensure_machine_write(host: &Host) -> Result<(), String> {
    if !host.may_change_machine() {
        return Err(crate::setup::verification_mode());
    }
    Ok(())
}

/// Validates a repository-relative path, rejecting empty strings, NUL bytes, leading slashes, and `..` traversal.
pub fn validate_repo_relative_path(raw: &str) -> Result<String, String> {
    if raw.is_empty() || raw.contains('\0') {
        return Err("Invalid file path.".into());
    }
    if raw.starts_with('/') || raw.starts_with('\\') {
        return Err("File path must be repository-relative.".into());
    }
    let normalized = raw.replace('\\', "/");
    let path = Path::new(&normalized);
    if path.is_absolute() {
        return Err("File path must be repository-relative.".into());
    }
    let mut has_normal = false;
    for comp in path.components() {
        match comp {
            Component::Normal(_) => has_normal = true,
            _ => return Err("Path traversal components are refused.".into()),
        }
    }
    if !has_normal {
        return Err("Invalid file path.".into());
    }
    Ok(normalized)
}

/// Validates a safe local git branch name (`git check-ref-format --branch` subset).
pub fn validate_branch_name(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.len() > 240 {
        return Err("Branch name must be between 1 and 240 characters.".into());
    }
    if trimmed == "@" || trimmed.starts_with('-') || trimmed.starts_with('/') || trimmed.ends_with('/') || trimmed.ends_with('.') {
        return Err("Invalid branch name.".into());
    }
    if trimmed.ends_with(".lock") || trimmed.contains("..") || trimmed.contains("@{") || trimmed.contains("//") {
        return Err("Invalid branch name.".into());
    }
    if trimmed.chars().any(|c| c.is_whitespace() || c.is_control() || matches!(c, '\\' | '~' | '^' | ':' | '?' | '*' | '[')) {
        return Err("Branch name contains unsupported characters.".into());
    }
    if trimmed.split('/').any(|seg| seg.is_empty() || seg.starts_with('.') || seg.ends_with(".lock")) {
        return Err("Invalid branch name segment.".into());
    }
    Ok(trimmed.to_string())
}

fn string_list_arg(args: &Value, key: &str) -> Result<Vec<String>, String> {
    let Some(arr) = args.get(key).and_then(Value::as_array) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::with_capacity(arr.len());
    let mut seen = HashSet::new();
    for item in arr {
        if let Some(raw) = item.as_str() {
            let clean = validate_repo_relative_path(raw)?;
            if seen.insert(clean.clone()) {
                out.push(clean);
            }
        }
    }
    Ok(out)
}

/// The empty folder git is pointed at for hooks (`core.hooksPath`): the work map's own on Windows (git reads a device name there as
/// a relative path), `/dev/null` elsewhere. A repository's hooks never run for the workbench's commits, checkouts and pushes.
fn hooks_path(host: &Host) -> String {
    if cfg!(windows) {
        if let Some(folders) = tauri::Manager::try_state::<crate::workmap::Folders>(&host.app) {
            let _ = std::fs::create_dir_all(&folders.hooks);
            return folders.hooks.to_string_lossy().into_owned();
        }
        let fallback = host.data_dir.join("git-no-hooks");
        let _ = std::fs::create_dir_all(&fallback);
        return fallback.to_string_lossy().into_owned();
    }
    "/dev/null".into()
}

/// The settings every workbench git run takes before its command, the hardened runner's (mighty-run `git`): no fsmonitor, no
/// hooks, no signature program, and only the transports a push or pull needs (`ext::` runs a command and stays off, as does `file`
/// beyond git's own default for what the person typed). Neither textconv nor an external diff program ever runs (the diffs pass
/// `--no-ext-diff --no-textconv`).
fn hardened_settings(git: &Path, hooks: &str) -> Vec<String> {
    let mut settings: Vec<String> =
        ["-c", "core.fsmonitor=false", "-c", "gc.auto=0", "-c", "maintenance.auto=false", "-c", "core.quotepath=false"]
            .into_iter()
            .map(String::from)
            .collect();
    settings.extend(["-c".to_string(), format!("core.hooksPath={hooks}")]);
    settings.extend(mighty_run::git::no_programs(git));
    for protocol in ["https", "http", "ssh", "git"] {
        settings.extend(["-c".to_string(), format!("protocol.{protocol}.allow=always")]);
    }
    settings.extend(["-c".to_string(), "protocol.file.allow=user".to_string()]);
    // A commit the workbench makes is never signed through a program the repository's own config names.
    settings.extend(["-c".to_string(), "commit.gpgsign=false".to_string(), "-c".to_string(), "tag.gpgsign=false".to_string()]);
    settings
}

fn build_git_request(git: PathBuf, root: &Path, args: &[String], read_only: bool, hooks: &str) -> SpawnRequest {
    let mut full_args = Vec::with_capacity(args.len() + 32);
    if read_only {
        full_args.push("--no-optional-locks".into());
    }
    full_args.extend(hardened_settings(&git, hooks));
    full_args.extend(args.iter().cloned());

    let mut request = SpawnRequest::new(git, full_args, root.to_path_buf());
    for (key, _) in std::env::vars_os() {
        let key_str = key.to_string_lossy();
        if key_str.to_ascii_uppercase().starts_with("GIT_")
            && !key_str.eq_ignore_ascii_case("GIT_SSH")
            && !key_str.eq_ignore_ascii_case("GIT_SSH_COMMAND")
        {
            request = request.without(key_str.into_owned());
        }
    }
    request = request
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .env("LC_ALL", "C")
        .env("GIT_ASKPASS", "")
        .env("SSH_ASKPASS_REQUIRE", "never")
        .env("NO_COLOR", "1");
    if read_only {
        request = request.env("GIT_OPTIONAL_LOCKS", "0").env("GIT_NO_LAZY_FETCH", "1");
    }
    request
}

async fn run_git_capture(
    host: &Host,
    root: &Path,
    args: &[&str],
    stdin_bytes: Option<Vec<u8>>,
    timeout_secs: u64,
    read_only: bool,
) -> Result<mighty_platform::process::Captured, String> {
    let owned: Vec<String> = args.iter().map(|s| (*s).to_string()).collect();
    run_git_capture_owned(host, root, &owned, stdin_bytes, timeout_secs, read_only).await
}

async fn run_git_capture_owned(
    host: &Host,
    root: &Path,
    args: &[String],
    stdin_bytes: Option<Vec<u8>>,
    timeout_secs: u64,
    read_only: bool,
) -> Result<mighty_platform::process::Captured, String> {
    let Some(git) = crate::workspace_create::git_executable() else {
        return Err(host.locale.get("workspace.git.error.gitMissing"));
    };
    let req = build_git_request(git, root, args, read_only, &hooks_path(host));
    mighty_platform::process::capture(host.processes.as_ref(), req, stdin_bytes, Duration::from_secs(timeout_secs), MAX_GIT_OUTPUT_BYTES)
        .await
        .map_err(|e| e.to_string())
}

static URL_USERINFO_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b([a-z][a-z0-9+.-]*://)[^/@\s]+@").expect("valid url userinfo regex"));

/// `text` with the user name and password (or token) of every `scheme://user:secret@host` URL taken out: git's stderr and a remote's
/// address carry them, and neither goes to the page or a log.
pub fn strip_url_credentials(text: &str) -> String {
    URL_USERINFO_RE.replace_all(text, "$1").into_owned()
}

fn format_git_failure(captured: &mighty_platform::process::Captured) -> String {
    let stderr = String::from_utf8_lossy(&captured.stderr);
    let trimmed = stderr.trim();
    if !trimmed.is_empty() {
        return strip_url_credentials(trimmed).chars().take(500).collect();
    }
    let stdout = String::from_utf8_lossy(&captured.stdout);
    let out_trimmed = stdout.trim();
    if !out_trimmed.is_empty() {
        return strip_url_credentials(out_trimmed).chars().take(500).collect();
    }
    format!("Git exited with code {}.", captured.exit_code)
}

fn find_hosting_cli(host: &Host, tool: &str) -> Option<PathBuf> {
    let env = host.runs().map(|r| r.catalog.environment().current().values).unwrap_or_default();
    crate::platform::cli_copies(tool, &env).into_iter().next().map(PathBuf::from)
}

async fn run_hosting_cli(
    host: &Host,
    tool: &str,
    root: &Path,
    args: &[String],
    timeout_secs: u64,
) -> Result<mighty_platform::process::Captured, String> {
    let env = host.runs().map(|r| r.catalog.environment().current().values).unwrap_or_default();
    let Some(binary) = crate::platform::cli_copies(tool, &env).into_iter().next().map(PathBuf::from) else {
        let key = if tool == "glab" { "git.pr.error.glabMissing" } else { "git.pr.error.ghMissing" };
        return Err(host.locale.get(key));
    };
    let req = host
        .processes
        .cli_request(&binary, args, root, &env)
        .map_err(|e| e.to_string())?
        .env("GH_PROMPT_DISABLED", "1")
        .env("GLAB_NO_PROMPT", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("NO_COLOR", "1");
    mighty_platform::process::capture(host.processes.as_ref(), req, None, Duration::from_secs(timeout_secs), MAX_GIT_OUTPUT_BYTES)
        .await
        .map_err(|e| e.to_string())
}

/// Detects `"github"`, `"gitlab"`, or `"none"` from a remote origin URL.
pub fn detect_hosting(remote_url: Option<&str>) -> &'static str {
    let Some(url) = remote_url.map(str::trim).filter(|s| !s.is_empty()) else {
        return "none";
    };
    let lower = url.to_ascii_lowercase();
    if lower.contains("gitlab") {
        "gitlab"
    } else if lower.contains("github") {
        "github"
    } else {
        "none"
    }
}

/// Parses `git status --porcelain=v2 --branch -z` output into branch metadata and file entries.
pub fn parse_porcelain_v2_status(bytes: &[u8]) -> ParsedPorcelainStatus {
    let fields: Vec<String> = bytes.split(|b| *b == 0).map(|slice| String::from_utf8_lossy(slice).into_owned()).collect();
    let mut head = String::new();
    let mut detached = false;
    let mut upstream = None;
    let mut ahead = 0i32;
    let mut behind = 0i32;
    let mut files = Vec::new();

    let mut idx = 0usize;
    while idx < fields.len() {
        let raw = fields[idx].trim_end_matches(['\r', '\n']);
        idx += 1;
        if raw.is_empty() {
            continue;
        }
        if let Some(header) = raw.strip_prefix("# ") {
            if let Some(branch_head) = header.strip_prefix("branch.head ") {
                if branch_head == "(detached)" {
                    detached = true;
                    head = "(detached)".into();
                } else {
                    detached = false;
                    head = branch_head.to_string();
                }
            } else if let Some(up) = header.strip_prefix("branch.upstream ") {
                if !up.is_empty() {
                    upstream = Some(up.to_string());
                }
            } else if let Some(ab) = header.strip_prefix("branch.ab ") {
                let mut parts = ab.split_whitespace();
                if let (Some(a_raw), Some(b_raw)) = (parts.next(), parts.next()) {
                    if let Some(a) = a_raw.strip_prefix('+').and_then(|v| v.parse::<i32>().ok()) {
                        ahead = a;
                    }
                    if let Some(b) = b_raw.strip_prefix('-').and_then(|v| v.parse::<i32>().ok()) {
                        behind = b;
                    }
                }
            }
            continue;
        }

        let mut chars = raw.chars();
        let Some(kind) = chars.next() else { continue };
        match kind {
            '1' | '2' | 'u' => {
                let skip = match kind {
                    '1' => 8,
                    '2' => 9,
                    _ => 10,
                };
                let parts: Vec<&str> = raw.splitn(skip + 1, ' ').collect();
                if parts.len() != skip + 1 || parts[1].chars().count() != 2 {
                    continue;
                }
                let mut xy = parts[1].chars();
                let raw_x = xy.next().unwrap_or('.');
                let raw_y = xy.next().unwrap_or('.');
                let index_ch = if raw_x == '.' { ' ' } else { raw_x };
                let worktree_ch = if raw_y == '.' { ' ' } else { raw_y };
                let original_path = if kind == '2' && idx < fields.len() {
                    let orig = fields[idx].clone();
                    idx += 1;
                    (!orig.is_empty()).then_some(orig)
                } else {
                    None
                };
                let conflicted = kind == 'u'
                    || index_ch == 'U'
                    || worktree_ch == 'U'
                    || (index_ch == 'A' && worktree_ch == 'A')
                    || (index_ch == 'D' && worktree_ch == 'D');
                let staged = !conflicted && index_ch != ' ' && index_ch != '?';
                let unstaged = conflicted || (worktree_ch != ' ' && worktree_ch != '?');
                let mark = if conflicted {
                    "!"
                } else if index_ch == 'R' || worktree_ch == 'R' {
                    "R"
                } else if index_ch == 'A' {
                    "A"
                } else if index_ch == 'D' || worktree_ch == 'D' {
                    "D"
                } else {
                    "M"
                };
                files.push(GitWorkbenchFile {
                    path: parts[skip].to_string(),
                    original_path,
                    mark: mark.into(),
                    index_status: index_ch.to_string(),
                    worktree_status: worktree_ch.to_string(),
                    staged,
                    unstaged,
                    untracked: false,
                    conflicted,
                });
            }
            '?' => {
                if let Some(path) = raw.strip_prefix("? ") {
                    files.push(GitWorkbenchFile {
                        path: path.to_string(),
                        original_path: None,
                        mark: "?".into(),
                        index_status: "?".into(),
                        worktree_status: "?".into(),
                        staged: false,
                        unstaged: true,
                        untracked: true,
                        conflicted: false,
                    });
                }
            }
            _ => {}
        }
    }

    ParsedPorcelainStatus { head, detached, upstream, ahead, behind, files }
}

async fn read_origin_url(host: &Host, root: &Path) -> Option<String> {
    let captured = run_git_capture(host, root, &["remote", "get-url", "origin"], None, 5, true).await.ok()?;
    if captured.exit_code != 0 {
        return None;
    }
    let url = String::from_utf8_lossy(&captured.stdout).lines().next().unwrap_or("").trim().to_string();
    (!url.is_empty()).then_some(url)
}

async fn head_exists(host: &Host, root: &Path) -> bool {
    run_git_capture(host, root, &["rev-parse", "--verify", "HEAD"], None, 5, true).await.is_ok_and(|c| c.exit_code == 0)
}

/// The branch a pull request is made against by default: `origin`'s HEAD (`origin/main` is `main`), else `main` or `master` when the
/// repository has one. None when it cannot be told.
async fn default_branch(host: &Host, root: &Path) -> Option<String> {
    if let Ok(cap) = run_git_capture(host, root, &["symbolic-ref", "-q", "--short", "refs/remotes/origin/HEAD"], None, 5, true).await
        && cap.exit_code == 0
    {
        let full = String::from_utf8_lossy(&cap.stdout).trim().to_string();
        if let Some(name) = full.strip_prefix("origin/").filter(|name| !name.is_empty()) {
            return Some(name.to_string());
        }
    }
    for candidate in ["main", "master"] {
        let reference = format!("refs/heads/{candidate}");
        if run_git_capture(host, root, &["show-ref", "--verify", "--quiet", &reference], None, 5, true)
            .await
            .is_ok_and(|c| c.exit_code == 0)
        {
            return Some(candidate.to_string());
        }
    }
    None
}

/// Whether `head` is a branch no pull request is made from: the repository's default branch, or the base of the pane's worktree.
fn is_base_branch(head: &str, default: Option<&str>, worktree_base: Option<&str>) -> bool {
    default == Some(head) || worktree_base == Some(head) || matches!(head, "main" | "master")
}

/// Parses `git diff` unified output into hunks, building a standalone valid `rawPatch` per hunk.
pub fn parse_unified_diff(diff_text: &str, path: &str, staged: bool) -> GitStructuredDiff {
    let normalized_path = path.replace('\\', "/");
    let mut binary = false;
    let mut file_mode_header: Option<String> = None;
    let mut old_file_header = format!("--- a/{normalized_path}");
    let mut new_file_header = format!("+++ b/{normalized_path}");

    let raw_lines: Vec<&str> = diff_text.lines().map(|l| l.trim_end_matches('\r')).collect();
    let mut hunks = Vec::new();
    let mut added = 0usize;
    let mut removed = 0usize;
    let mut rendered_lines = 0usize;
    let mut truncated = false;

    let mut current_header: Option<(String, usize, usize, usize, usize)> = None;
    let mut current_old_line = 0usize;
    let mut current_new_line = 0usize;
    let mut current_lines: Vec<GitDiffLine> = Vec::new();
    let mut current_raw_body: Vec<String> = Vec::new();

    let flush_hunk = |hunks: &mut Vec<GitDiffHunk>,
                      header_info: Option<(String, usize, usize, usize, usize)>,
                      lines: Vec<GitDiffLine>,
                      raw_body: Vec<String>,
                      mode_hdr: &Option<String>,
                      old_hdr: &str,
                      new_hdr: &str| {
        let Some((header, old_start, old_lines, new_start, new_lines)) = header_info else { return };
        let index = hunks.len();
        let mut patch = format!("diff --git a/{normalized_path} b/{normalized_path}\n");
        if let Some(mode) = mode_hdr {
            patch.push_str(mode);
            patch.push('\n');
        }
        patch.push_str(old_hdr);
        patch.push('\n');
        patch.push_str(new_hdr);
        patch.push('\n');
        patch.push_str(&header);
        patch.push('\n');
        for body_line in raw_body {
            patch.push_str(&body_line);
            patch.push('\n');
        }
        hunks.push(GitDiffHunk { index, header, old_start, old_lines, new_start, new_lines, raw_patch: patch, lines });
    };

    for line in raw_lines {
        if current_header.is_none() {
            if line.starts_with("Binary files ") || line == "GIT binary patch" {
                binary = true;
                continue;
            }
            if line.starts_with("new file mode ") || line.starts_with("deleted file mode ") {
                file_mode_header = Some(line.to_string());
                continue;
            }
            if line.starts_with("--- ") {
                old_file_header = line.to_string();
                continue;
            }
            if line.starts_with("+++ ") {
                new_file_header = line.to_string();
                continue;
            }
        }

        if let Some(caps) = HUNK_HEADER_RE.captures(line) {
            let prev_hdr = current_header.take();
            let prev_lines = std::mem::take(&mut current_lines);
            let prev_raw = std::mem::take(&mut current_raw_body);
            flush_hunk(&mut hunks, prev_hdr, prev_lines, prev_raw, &file_mode_header, &old_file_header, &new_file_header);

            if rendered_lines >= MAX_RENDERED_DIFF_LINES {
                truncated = true;
                break;
            }

            let old_start = caps.get(1).and_then(|m| m.as_str().parse::<usize>().ok()).unwrap_or(0);
            let old_count = caps.get(2).map_or(1, |m| m.as_str().parse::<usize>().unwrap_or(1));
            let new_start = caps.get(3).and_then(|m| m.as_str().parse::<usize>().ok()).unwrap_or(0);
            let new_count = caps.get(4).map_or(1, |m| m.as_str().parse::<usize>().unwrap_or(1));

            current_old_line = old_start;
            current_new_line = new_start;
            current_header = Some((line.to_string(), old_start, old_count, new_start, new_count));
            continue;
        }

        if current_header.is_some() {
            if line.starts_with("diff --git ") {
                break;
            }
            if line.starts_with('\\') {
                current_raw_body.push(line.to_string());
                continue;
            }
            if let Some(rest) = line.strip_prefix('+') {
                added += 1;
                current_raw_body.push(line.to_string());
                if rendered_lines < MAX_RENDERED_DIFF_LINES {
                    current_lines.push(GitDiffLine {
                        kind: "add".into(),
                        old_line: None,
                        new_line: Some(current_new_line),
                        text: rest.to_string(),
                    });
                    rendered_lines += 1;
                } else {
                    truncated = true;
                }
                current_new_line += 1;
            } else if let Some(rest) = line.strip_prefix('-') {
                removed += 1;
                current_raw_body.push(line.to_string());
                if rendered_lines < MAX_RENDERED_DIFF_LINES {
                    current_lines.push(GitDiffLine {
                        kind: "del".into(),
                        old_line: Some(current_old_line),
                        new_line: None,
                        text: rest.to_string(),
                    });
                    rendered_lines += 1;
                } else {
                    truncated = true;
                }
                current_old_line += 1;
            } else if let Some(rest) = line.strip_prefix(' ') {
                current_raw_body.push(line.to_string());
                if rendered_lines < MAX_RENDERED_DIFF_LINES {
                    current_lines.push(GitDiffLine {
                        kind: "context".into(),
                        old_line: Some(current_old_line),
                        new_line: Some(current_new_line),
                        text: rest.to_string(),
                    });
                    rendered_lines += 1;
                } else {
                    truncated = true;
                }
                current_old_line += 1;
                current_new_line += 1;
            }
        }
    }

    flush_hunk(&mut hunks, current_header, current_lines, current_raw_body, &file_mode_header, &old_file_header, &new_file_header);

    GitStructuredDiff { path: normalized_path, staged, hunks, added, removed, binary, truncated }
}

/// Builds a synthetic all-added structured diff (including a stageable unified `rawPatch`) for an untracked file.
pub fn build_untracked_file_diff(path: &str, bytes: &[u8]) -> GitStructuredDiff {
    let normalized_path = path.replace('\\', "/");
    let check_len = bytes.len().min(8192);
    if bytes[..check_len].contains(&0) {
        return GitStructuredDiff {
            path: normalized_path,
            staged: false,
            hunks: Vec::new(),
            added: 0,
            removed: 0,
            binary: true,
            truncated: false,
        };
    }

    let mut truncated = bytes.len() > MAX_UNTRACKED_DIFF_BYTES;
    let slice = &bytes[..bytes.len().min(MAX_UNTRACKED_DIFF_BYTES)];
    if slice.is_empty() {
        return GitStructuredDiff {
            path: normalized_path,
            staged: false,
            hunks: Vec::new(),
            added: 0,
            removed: 0,
            binary: false,
            truncated: false,
        };
    }

    let text = String::from_utf8_lossy(slice);
    let ends_with_newline = text.ends_with('\n');
    let mut raw_lines: Vec<&str> = text.split('\n').collect();
    if ends_with_newline {
        raw_lines.pop();
    }
    let total_added = raw_lines.len();
    if raw_lines.len() > MAX_RENDERED_DIFF_LINES {
        raw_lines.truncate(MAX_RENDERED_DIFF_LINES);
        truncated = true;
    }

    let count = raw_lines.len();
    let header = format!("@@ -0,0 +1,{count} @@");
    let mut patch = format!(
        "diff --git a/{normalized_path} b/{normalized_path}\nnew file mode 100644\n--- /dev/null\n+++ b/{normalized_path}\n{header}\n"
    );
    let mut lines = Vec::with_capacity(count);
    for (i, raw_line) in raw_lines.iter().enumerate() {
        let clean = raw_line.trim_end_matches('\r');
        patch.push('+');
        patch.push_str(clean);
        patch.push('\n');
        lines.push(GitDiffLine { kind: "add".into(), old_line: None, new_line: Some(i + 1), text: clean.to_string() });
    }
    if !ends_with_newline && !truncated {
        patch.push_str("\\ No newline at end of file\n");
    }

    GitStructuredDiff {
        path: normalized_path,
        staged: false,
        hunks: vec![GitDiffHunk { index: 0, header, old_start: 0, old_lines: 0, new_start: 1, new_lines: count, raw_patch: patch, lines }],
        added: total_added,
        removed: 0,
        binary: false,
        truncated,
    }
}

/// At most `max` bytes of the file: an untracked file of any size is never read whole to show or sample a piece of it.
fn read_up_to(path: &Path, max: u64) -> std::io::Result<Vec<u8>> {
    use std::io::Read as _;
    let mut bytes = Vec::new();
    std::fs::File::open(path)?.take(max).read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn resolve_file_inside_root(root: &Path, rel_path: &str) -> Option<PathBuf> {
    let clean = validate_repo_relative_path(rel_path).ok()?;
    let canonical_root = std::fs::canonicalize(root).ok()?;
    let target = canonical_root.join(&clean);
    let canonical_target = std::fs::canonicalize(&target).ok()?;
    if !canonical_target.starts_with(&canonical_root) {
        return None;
    }
    canonical_target.is_file().then_some(canonical_target)
}

/// Checks whether a repository-relative path names a secret or sensitive credential file.
pub fn is_secret_file_path(path: &str) -> bool {
    let normalized = path.replace('\\', "/");
    if crate::workspace_mcp::is_sensitive_relative_path(Path::new(&normalized)) {
        return true;
    }
    normalized.to_ascii_lowercase().ends_with(".ppk")
}

/// Scans plain text lines for the 9 secret patterns without recording the matched value.
pub fn scan_text_secrets(text: &str, path: &str, first_line: usize) -> Vec<SecretFinding> {
    let mut findings = Vec::new();
    for (offset, raw_line) in text.split('\n').enumerate() {
        let line = raw_line.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }
        for (kind, regex) in SECRET_PATTERNS.iter() {
            let matched = if *kind == "OpenAI key" {
                regex.find_iter(line).any(|m| !m.as_str().starts_with("sk-ant-"))
            } else {
                regex.is_match(line)
            };
            if matched {
                findings.push(SecretFinding { path: path.to_string(), line: first_line + offset, kind: (*kind).to_string() });
            }
        }
    }
    findings
}

/// Scans only added (`+`) lines of a unified diff (`git diff -U0`) for secret patterns.
/// `diff` with every line that matches a secret pattern replaced by a marker: the excerpt goes to a model, and a token a file holds
/// must not.
pub fn redact_secret_lines(diff: &str) -> String {
    let mut out = String::with_capacity(diff.len());
    for line in diff.lines() {
        match SECRET_PATTERNS.iter().find(|(_, pattern)| pattern.is_match(line)) {
            Some((kind, _)) => {
                out.push(line.chars().next().filter(|c| matches!(c, '+' | '-' | ' ')).unwrap_or('+'));
                out.push_str(&format!("[redacted: {kind}]"));
            }
            None => out.push_str(line),
        }
        out.push('\n');
    }
    out
}

pub fn scan_diff_secrets(diff: &str) -> Vec<SecretFinding> {
    let mut findings = Vec::new();
    let mut current_path = String::new();
    let mut current_line = 0usize;

    for raw in diff.split('\n') {
        let line = raw.trim_end_matches('\r');
        if let Some(rest) = line.strip_prefix("+++ ") {
            if rest == "/dev/null" {
                current_path.clear();
            } else {
                current_path = rest.strip_prefix("b/").unwrap_or(rest).to_string();
            }
        } else if let Some(caps) = HUNK_HEADER_RE.captures(line) {
            current_line = caps.get(3).and_then(|m| m.as_str().parse::<usize>().ok()).unwrap_or(1);
        } else if !current_path.is_empty() {
            if let Some(added_text) = line.strip_prefix('+') {
                findings.extend(scan_text_secrets(added_text, &current_path, current_line));
                current_line += 1;
            } else if !line.starts_with('-') && !line.starts_with('\\') && !line.starts_with("diff --git ") && current_line > 0 {
                current_line += 1;
            }
        }
    }
    findings
}

async fn scan_workspace_secrets(
    host: &Host,
    root: &Path,
    status_files: &[GitWorkbenchFile],
    requested_paths: &[String],
    staged_only: bool,
) -> Vec<SecretFinding> {
    let mut findings = Vec::new();
    let untracked_set: HashSet<&str> = status_files.iter().filter(|f| f.untracked).map(|f| f.path.as_str()).collect();

    let target_paths: Vec<String> = if staged_only {
        status_files
            .iter()
            .filter(|f| f.staged && (requested_paths.is_empty() || requested_paths.iter().any(|p| p == &f.path)))
            .map(|f| f.path.clone())
            .collect()
    } else if !requested_paths.is_empty() {
        requested_paths.to_vec()
    } else {
        status_files.iter().filter(|f| !f.conflicted).map(|f| f.path.clone()).collect()
    };

    if target_paths.is_empty() {
        return findings;
    }

    for path in &target_paths {
        if is_secret_file_path(path) {
            findings.push(SecretFinding { path: path.clone(), line: 0, kind: "secret file".into() });
        }
    }

    if staged_only {
        let mut diff_args: Vec<String> =
            ["diff", "--cached", "--no-color", "--no-ext-diff", "--no-textconv", "-U0", "--"].into_iter().map(String::from).collect();
        diff_args.extend(target_paths.iter().cloned());
        if let Ok(captured) = run_git_capture_owned(host, root, &diff_args, None, 15, true).await
            && captured.exit_code == 0
        {
            findings.extend(scan_diff_secrets(&String::from_utf8_lossy(&captured.stdout)));
        }
    } else {
        let tracked: Vec<String> = target_paths.iter().filter(|p| !untracked_set.contains(p.as_str())).cloned().collect();
        if !tracked.is_empty() {
            if head_exists(host, root).await {
                let mut diff_args: Vec<String> =
                    ["diff", "HEAD", "--no-color", "--no-ext-diff", "--no-textconv", "-U0", "--"].into_iter().map(String::from).collect();
                diff_args.extend(tracked.iter().cloned());
                if let Ok(captured) = run_git_capture_owned(host, root, &diff_args, None, 15, true).await
                    && captured.exit_code == 0
                {
                    findings.extend(scan_diff_secrets(&String::from_utf8_lossy(&captured.stdout)));
                }
            } else {
                for cached_flag in [Some("--cached"), None] {
                    let mut diff_args = vec!["diff".to_string()];
                    if let Some(f) = cached_flag {
                        diff_args.push(f.into());
                    }
                    diff_args.extend(["--no-color", "--no-ext-diff", "--no-textconv", "-U0", "--"].into_iter().map(String::from));
                    diff_args.extend(tracked.iter().cloned());
                    if let Ok(captured) = run_git_capture_owned(host, root, &diff_args, None, 15, true).await
                        && captured.exit_code == 0
                    {
                        findings.extend(scan_diff_secrets(&String::from_utf8_lossy(&captured.stdout)));
                    }
                }
            }
        }

        for path in target_paths.iter().filter(|p| untracked_set.contains(p.as_str())) {
            let Some(resolved) = resolve_file_inside_root(root, path) else { continue };
            let Ok(meta) = std::fs::metadata(&resolved) else { continue };
            if meta.len() > MAX_SECRET_SCAN_FILE_BYTES {
                continue;
            }
            let Ok(bytes) = std::fs::read(&resolved) else { continue };
            if bytes[..bytes.len().min(8192)].contains(&0) {
                continue;
            }
            if let Ok(text) = String::from_utf8(bytes) {
                findings.extend(scan_text_secrets(&text, path, 1));
            }
        }
    }

    let mut deduped = Vec::with_capacity(findings.len());
    let mut seen = HashSet::new();
    for finding in findings {
        if seen.insert(finding.clone()) {
            deduped.push(finding);
        }
    }
    deduped
}

/// Strips surrounding markdown code fences (` ```...``` `) if the model wrapped the commit message.
pub fn strip_markdown_fences(raw: &str) -> String {
    let trimmed = raw.trim();
    if let Some(after_open) = trimmed.strip_prefix("```") {
        let body = match after_open.find('\n') {
            Some(nl) => &after_open[nl + 1..],
            None => after_open,
        };
        let without_close = body.trim_end().strip_suffix("```").unwrap_or(body);
        return without_close.trim().to_string();
    }
    trimmed.to_string()
}

/// Deterministic Conventional Commit message generator when no AI CLI is reachable.
pub fn heuristic_conventional_commit(files: &[GitWorkbenchFile]) -> String {
    if files.is_empty() {
        return "chore: update workspace files".into();
    }

    let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
    let is_doc = |p: &str| {
        let lower = p.to_ascii_lowercase();
        lower.ends_with(".md")
            || lower.ends_with(".mdx")
            || lower.starts_with("docs/")
            || lower.starts_with("doc/")
            || lower == "license"
            || lower.starts_with("changelog")
    };
    let is_test = |p: &str| {
        let lower = p.to_ascii_lowercase();
        lower.contains(".test.")
            || lower.contains(".spec.")
            || lower.contains("_test.")
            || lower.starts_with("tests/")
            || lower.contains("/tests/")
            || lower.contains("/__tests__/")
    };
    let is_config = |p: &str| {
        let lower = p.to_ascii_lowercase();
        let base = lower.rsplit('/').next().unwrap_or(&lower);
        lower.starts_with(".github/")
            || matches!(
                base,
                "cargo.toml"
                    | "cargo.lock"
                    | "package.json"
                    | "package-lock.json"
                    | "pnpm-lock.yaml"
                    | "tsconfig.json"
                    | "makefile"
                    | "dockerfile"
                    | ".gitignore"
            )
    };

    let commit_type = if paths.iter().all(|p| is_doc(p)) {
        "docs"
    } else if paths.iter().all(|p| is_test(p)) {
        "test"
    } else if paths.iter().all(|p| is_config(p)) {
        "chore"
    } else if files.iter().any(|f| f.mark == "A" || f.mark == "?") {
        "feat"
    } else {
        "fix"
    };

    let infer_scope = |p: &str| -> Option<String> {
        let parts: Vec<&str> = p.split('/').filter(|s| !s.is_empty()).collect();
        if parts.len() <= 1 {
            return None;
        }
        let meaningful: Vec<&str> = parts[..parts.len() - 1]
            .iter()
            .copied()
            .filter(|seg| !matches!(*seg, "src" | "lib" | "app" | "native" | "windows" | "web" | "crates" | "components"))
            .collect();
        let raw_scope = meaningful.last().copied().or_else(|| parts.first().copied())?;
        let cleaned: String = raw_scope
            .trim_start_matches("src-")
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
            .collect::<String>()
            .trim_matches('-')
            .to_string();
        (!cleaned.is_empty() && cleaned.len() <= 24).then_some(cleaned)
    };

    let scopes: HashSet<String> = paths.iter().filter_map(|p| infer_scope(p)).collect();
    let scope_part = if scopes.len() == 1 { format!("({})", scopes.into_iter().next().unwrap_or_default()) } else { String::new() };

    let verb = if files.iter().all(|f| f.mark == "A" || f.mark == "?") {
        "add"
    } else if files.iter().all(|f| f.mark == "D") {
        "remove"
    } else {
        "update"
    };

    let subject_target = if files.len() == 1 {
        files[0].path.rsplit('/').next().unwrap_or(&files[0].path).to_string()
    } else if files.len() <= 3 {
        files.iter().map(|f| f.path.rsplit('/').next().unwrap_or(&f.path)).collect::<Vec<_>>().join(", ")
    } else {
        format!("{} files", files.len())
    };

    let mut subject = format!("{commit_type}{scope_part}: {verb} {subject_target}");
    if subject.chars().count() > 72 {
        subject = format!("{commit_type}{scope_part}: {verb} {} files", files.len());
    }

    if files.len() == 1 {
        return subject;
    }

    let mut bullets: Vec<String> = files.iter().take(10).map(|f| format!("- {}", f.path)).collect();
    if files.len() > 10 {
        bullets.push(format!("- ...and {} more", files.len() - 10));
    }
    format!("{subject}\n\n{}", bullets.join("\n"))
}

fn build_draft_commit_prompt(status_short: &str, recent_log: &str, diff_excerpt: &str, touched_paths: &[String]) -> String {
    let files_list = touched_paths.iter().take(40).cloned().collect::<Vec<_>>().join("\n");
    format!(
        "Write ONLY a concise Conventional Commit message (`<type>(<optional-scope>): <imperative summary>`, optionally followed by a blank line and 2-5 short `- ` bullet points when multiple concerns changed) for the following repository changes.\n\
         Rules:\n\
         - Output raw commit message text ONLY. Do NOT wrap in markdown code fences (` ``` `) and include no commentary.\n\
         - Keep the first subject line under 72 characters in English (unless recent commits clearly use another language).\n\n\
         Recent commits:\n{recent_log}\n\n\
         Touched files:\n{files_list}\n\n\
         Git status:\n{status_short}\n\n\
         Diff excerpt:\n{diff_excerpt}"
    )
}

/// Parses GitHub `gh pr view --json number,url,state,title,isDraft,statusCheckRollup` output.
pub fn parse_github_pr(bytes: &[u8]) -> Option<GitPrInfo> {
    let root: Value = serde_json::from_slice(bytes).ok()?;
    let number = root.get("number")?.as_i64()?;
    let url = root.get("url")?.as_str()?.trim().to_string();
    if url.is_empty() {
        return None;
    }
    let raw_state = root.get("state").and_then(Value::as_str).unwrap_or("open").to_ascii_lowercase();
    let state = match raw_state.as_str() {
        "merged" => "merged",
        "closed" => "closed",
        _ => "open",
    }
    .to_string();
    let title = root.get("title").and_then(Value::as_str).unwrap_or("").trim().to_string();
    let is_draft = root.get("isDraft").and_then(Value::as_bool).unwrap_or(false);

    let rollup = root.get("statusCheckRollup").and_then(Value::as_array);
    let checks = match rollup {
        None => "none",
        Some(items) if items.is_empty() => "none",
        Some(items) => {
            let mut has_failed = false;
            let mut has_pending = false;
            for item in items {
                let conclusion = item.get("conclusion").and_then(Value::as_str).unwrap_or("").to_ascii_uppercase();
                let status = item.get("status").and_then(Value::as_str).unwrap_or("").to_ascii_uppercase();
                let item_state = item.get("state").and_then(Value::as_str).unwrap_or("").to_ascii_uppercase();
                if matches!(conclusion.as_str(), "FAILURE" | "TIMED_OUT" | "CANCELLED" | "ACTION_REQUIRED" | "STARTUP_FAILURE")
                    || matches!(item_state.as_str(), "FAILURE" | "ERROR")
                {
                    has_failed = true;
                } else if (!status.is_empty() && status != "COMPLETED") || matches!(item_state.as_str(), "PENDING" | "EXPECTED") {
                    has_pending = true;
                }
            }
            if has_failed {
                "failed"
            } else if has_pending {
                "pending"
            } else {
                "passed"
            }
        }
    }
    .to_string();

    Some(GitPrInfo { number, url, state, title, is_draft, checks })
}

/// Parses GitLab `glab mr view --output json` output.
pub fn parse_gitlab_mr(bytes: &[u8]) -> Option<GitPrInfo> {
    let root: Value = serde_json::from_slice(bytes).ok()?;
    let number = root.get("iid")?.as_i64()?;
    let url = root.get("web_url")?.as_str()?.trim().to_string();
    if url.is_empty() {
        return None;
    }
    let raw_state = root.get("state").and_then(Value::as_str).unwrap_or("opened").to_ascii_lowercase();
    let state = match raw_state.as_str() {
        "opened" | "open" => "open",
        "merged" => "merged",
        "closed" => "closed",
        _ => "open",
    }
    .to_string();
    let title = root.get("title").and_then(Value::as_str).unwrap_or("").trim().to_string();
    let is_draft =
        root.get("draft").and_then(Value::as_bool).or_else(|| root.get("work_in_progress").and_then(Value::as_bool)).unwrap_or(false);

    let pipeline = root.get("head_pipeline").filter(|v| v.is_object()).or_else(|| root.get("pipeline").filter(|v| v.is_object()));
    let pipe_status = pipeline.and_then(|p| p.get("status")).and_then(Value::as_str).unwrap_or("").to_ascii_lowercase();
    let checks = match pipe_status.as_str() {
        "" => "none",
        "success" | "skipped" | "manual" => "passed",
        "failed" | "canceled" => "failed",
        _ => "pending",
    }
    .to_string();

    Some(GitPrInfo { number, url, state, title, is_draft, checks })
}

/// Strips ANSI escape sequences and returns `(joined_tail, line_count)` of the last `max_lines` non-empty lines.
pub fn extract_log_tail(raw: &str, max_lines: usize) -> (String, usize) {
    if max_lines == 0 || raw.is_empty() {
        return (String::new(), 0);
    }
    let stripped = ANSI_ESCAPE_RE.replace_all(raw, "");
    let non_empty: Vec<&str> = stripped.split('\n').map(|l| l.trim_end_matches('\r')).filter(|l| !l.trim().is_empty()).collect();
    if non_empty.is_empty() {
        return (String::new(), 0);
    }
    let skip = non_empty.len().saturating_sub(max_lines);
    let kept = &non_empty[skip..];
    (kept.join("\n"), kept.len())
}

async fn push_workspace_branch(host: &Arc<Host>, root: &Path) -> Result<(), String> {
    let upstream = run_git_capture(host, root, &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"], None, 10, true).await?;
    let push_args: &[&str] = if upstream.exit_code == 0 { &["push"] } else { &["push", "-u", "origin", "HEAD"] };
    let pushed = run_git_capture(host, root, push_args, None, 120, false).await?;
    if pushed.exit_code != 0 {
        return Err(format_git_failure(&pushed));
    }
    host.refresh_git(true);
    Ok(())
}

async fn query_pr_status_value(host: &Host, root: &Path) -> Value {
    let origin = read_origin_url(host, root).await;
    let hosting = detect_hosting(origin.as_deref());
    if hosting == "none" {
        return json!({ "hosting": "none", "pr": Value::Null });
    }
    let cli = if hosting == "gitlab" { "glab" } else { "gh" };
    if find_hosting_cli(host, cli).is_none() {
        return json!({ "hosting": hosting, "pr": Value::Null, "cliMissing": cli });
    }
    if hosting == "github" {
        let args = vec!["pr".into(), "view".into(), "--json".into(), "number,url,state,title,isDraft,statusCheckRollup".into()];
        if let Ok(captured) = run_hosting_cli(host, "gh", root, &args, 20).await
            && captured.exit_code == 0
            && let Some(pr) = parse_github_pr(&captured.stdout)
        {
            return json!({ "hosting": "github", "pr": pr });
        }
        json!({ "hosting": "github", "pr": Value::Null })
    } else {
        let args = vec!["mr".into(), "view".into(), "--output".into(), "json".into()];
        if let Ok(captured) = run_hosting_cli(host, "glab", root, &args, 20).await
            && captured.exit_code == 0
            && let Some(mr) = parse_gitlab_mr(&captured.stdout)
        {
            return json!({ "hosting": "gitlab", "pr": mr });
        }
        json!({ "hosting": "gitlab", "pr": Value::Null })
    }
}

fn extract_first_url_or_line(output: &str) -> Option<String> {
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("https://") || trimmed.starts_with("http://") {
            return Some(trimmed.to_string());
        }
    }
    output.lines().map(str::trim).find(|l| !l.is_empty()).map(str::to_string)
}

/// The worktree pane whose branch is `branch` and which has a linked task: its id and record. A pane that merely has a linked task
/// but works on another branch is not the one this pull request is for.
fn find_linked_task_pane(host: &Host, workspace_id: &str, branch: &str) -> Option<(String, PaneWorktree)> {
    let view = host.view();
    view.snapshot.sessions().iter().filter(|s| s.workspace_id() == workspace_id && s.kind() == "claude").find_map(|session| {
        let record = session.pane_worktree.as_ref().filter(|pw| pw.branch.as_deref() == Some(branch))?;
        let mut record = record.clone();
        if record.linked_task.is_none() {
            record.linked_task = session.linked_task.clone();
        }
        record.linked_task.is_some().then(|| (session.id().to_string(), record))
    })
}

pub fn register(methods: &mut Registry) {
    methods.add("gitWorkbenchStatus", |host: Arc<Host>, args| async move {
        let (ws, root) = resolve_git_target(&host, &args)?;
        let status_cap =
            run_git_capture(&host, &root, &["status", "--porcelain=v2", "--branch", "-z", "--untracked-files=all"], None, 15, true).await?;
        if status_cap.exit_code != 0 {
            return Err(format_git_failure(&status_cap));
        }
        let parsed = parse_porcelain_v2_status(&status_cap.stdout);

        let mut branches: Vec<String> =
            match run_git_capture(&host, &root, &["for-each-ref", "--format=%(refname:short)", "refs/heads/"], None, 10, true).await {
                Ok(cap) if cap.exit_code == 0 => {
                    String::from_utf8_lossy(&cap.stdout).lines().map(str::trim).filter(|b| !b.is_empty()).map(str::to_string).collect()
                }
                _ => Vec::new(),
            };
        if !parsed.detached && !parsed.head.is_empty() && !branches.iter().any(|b| b == &parsed.head) {
            branches.insert(0, parsed.head.clone());
        }

        let remote_url = read_origin_url(&host, &root).await;
        let hosting = detect_hosting(remote_url.as_deref());
        let default = default_branch(&host, &root).await;
        let on_default =
            !parsed.detached && is_base_branch(&parsed.head, default.as_deref(), worktree_base_branch(&host, &args).as_deref());

        Ok(json!({
            "defaultBranch": default,
            "onDefaultBranch": on_default,
            "workspaceId": ws.id(),
            "head": parsed.head,
            "detached": parsed.detached,
            "upstream": parsed.upstream,
            "ahead": parsed.ahead,
            "behind": parsed.behind,
            "branches": branches,
            "hosting": hosting,
            "remoteUrl": remote_url.as_deref().map(strip_url_credentials),
            "files": parsed.files,
        }))
    });

    methods.add("gitWorkbenchDiff", |host: Arc<Host>, args| async move {
        let (_, root) = resolve_git_target(&host, &args)?;
        let raw_path = text(&args, "path").ok_or_else(|| "Missing file path.".to_string())?;
        let clean_path = validate_repo_relative_path(raw_path)?;
        let staged = flag(&args, "staged");

        if staged {
            let diff_args = vec![
                "diff".into(),
                "--cached".into(),
                "--no-color".into(),
                "--no-ext-diff".into(),
                "--no-textconv".into(),
                "-U3".into(),
                "--".into(),
                clean_path.clone(),
            ];
            let captured = run_git_capture_owned(&host, &root, &diff_args, None, 15, true).await?;
            if captured.exit_code != 0 {
                return Err(format_git_failure(&captured));
            }
            let parsed = parse_unified_diff(&String::from_utf8_lossy(&captured.stdout), &clean_path, true);
            return serde_json::to_value(parsed).map_err(|e| e.to_string());
        }

        let diff_args = vec![
            "diff".into(),
            "--no-color".into(),
            "--no-ext-diff".into(),
            "--no-textconv".into(),
            "-U3".into(),
            "--".into(),
            clean_path.clone(),
        ];
        let captured = run_git_capture_owned(&host, &root, &diff_args, None, 15, true).await?;
        if captured.exit_code != 0 {
            return Err(format_git_failure(&captured));
        }
        if !captured.stdout.is_empty() {
            let parsed = parse_unified_diff(&String::from_utf8_lossy(&captured.stdout), &clean_path, false);
            return serde_json::to_value(parsed).map_err(|e| e.to_string());
        }

        let tracked_check = run_git_capture_owned(
            &host,
            &root,
            &["ls-files".into(), "--error-unmatch".into(), "--".into(), clean_path.clone()],
            None,
            5,
            true,
        )
        .await;
        let is_tracked = tracked_check.is_ok_and(|c| c.exit_code == 0);
        if !is_tracked && let Some(resolved) = resolve_file_inside_root(&root, &clean_path) {
            let bytes = read_up_to(&resolved, MAX_UNTRACKED_DIFF_BYTES as u64 + 1).map_err(|e| e.to_string())?;
            let untracked_diff = build_untracked_file_diff(&clean_path, &bytes);
            return serde_json::to_value(untracked_diff).map_err(|e| e.to_string());
        }

        let empty =
            GitStructuredDiff { path: clean_path, staged: false, hunks: Vec::new(), added: 0, removed: 0, binary: false, truncated: false };
        serde_json::to_value(empty).map_err(|e| e.to_string())
    });

    methods.add("gitWorkbenchStage", |host: Arc<Host>, args| async move {
        let (_, root) = resolve_git_target(&host, &args)?;
        ensure_local_repo_write(&host, &root)?;

        if let Some(patch) = text(&args, "patch").filter(|p| !p.trim().is_empty()) {
            let mut input = patch.as_bytes().to_vec();
            if !input.ends_with(b"\n") {
                input.push(b'\n');
            }
            let captured = run_git_capture(&host, &root, &["apply", "--cached", "--unidiff-zero", "-"], Some(input), 20, false).await?;
            if captured.exit_code != 0 {
                return Err(format_git_failure(&captured));
            }
        } else {
            let paths = string_list_arg(&args, "paths")?;
            if paths.is_empty() {
                return Err("No file paths provided to stage.".into());
            }
            let mut cmd: Vec<String> = ["add", "-A", "--"].into_iter().map(String::from).collect();
            cmd.extend(paths);
            let captured = run_git_capture_owned(&host, &root, &cmd, None, 20, false).await?;
            if captured.exit_code != 0 {
                return Err(format_git_failure(&captured));
            }
        }

        host.refresh_git(true);
        Ok(json!({ "ok": true }))
    });

    methods.add("gitWorkbenchUnstage", |host: Arc<Host>, args| async move {
        let (_, root) = resolve_git_target(&host, &args)?;
        ensure_local_repo_write(&host, &root)?;

        if let Some(patch) = text(&args, "patch").filter(|p| !p.trim().is_empty()) {
            let mut input = patch.as_bytes().to_vec();
            if !input.ends_with(b"\n") {
                input.push(b'\n');
            }
            let captured =
                run_git_capture(&host, &root, &["apply", "--cached", "--reverse", "--unidiff-zero", "-"], Some(input), 20, false).await?;
            if captured.exit_code != 0 {
                return Err(format_git_failure(&captured));
            }
        } else {
            let paths = string_list_arg(&args, "paths")?;
            if paths.is_empty() {
                return Err("No file paths provided to unstage.".into());
            }
            let mut cmd: Vec<String> = if head_exists(&host, &root).await {
                ["restore", "--staged", "--"].into_iter().map(String::from).collect()
            } else {
                ["rm", "--cached", "-r", "--"].into_iter().map(String::from).collect()
            };
            cmd.extend(paths);
            let captured = run_git_capture_owned(&host, &root, &cmd, None, 20, false).await?;
            if captured.exit_code != 0 {
                return Err(format_git_failure(&captured));
            }
        }

        host.refresh_git(true);
        Ok(json!({ "ok": true }))
    });

    methods.add("gitWorkbenchSwitchBranch", |host: Arc<Host>, args| async move {
        let (ws, root) = resolve_git_target(&host, &args)?;
        ensure_local_repo_write(&host, &root)?;
        if agent_running_in(&host, &ws, &root) {
            return Err("An agent is running in this folder; stop it before switching branches.".into());
        }
        let raw_branch = text(&args, "branch").unwrap_or("");
        let branch = validate_branch_name(raw_branch)?;
        let create = flag(&args, "create");

        let cmd: Vec<String> =
            if create { vec!["switch".into(), "-c".into(), branch.clone()] } else { vec!["switch".into(), branch.clone()] };
        let captured = run_git_capture_owned(&host, &root, &cmd, None, 20, false).await?;
        if captured.exit_code != 0 {
            return Err(format_git_failure(&captured));
        }

        host.refresh_git(true);
        Ok(json!({ "branch": branch }))
    });

    methods.add("gitWorkbenchScanSecrets", |host: Arc<Host>, args| async move {
        let (_, root) = resolve_git_target(&host, &args)?;
        let requested_paths = string_list_arg(&args, "paths")?;
        let staged_only = flag(&args, "stagedOnly");

        let status_cap =
            run_git_capture(&host, &root, &["status", "--porcelain=v2", "-z", "--untracked-files=all"], None, 15, true).await?;
        if status_cap.exit_code != 0 {
            return Err(format_git_failure(&status_cap));
        }
        let parsed = parse_porcelain_v2_status(&status_cap.stdout);
        let findings = scan_workspace_secrets(&host, &root, &parsed.files, &requested_paths, staged_only).await;
        Ok(json!({ "findings": findings }))
    });

    methods.add("gitWorkbenchDraftMessage", |host: Arc<Host>, args| async move {
        let (_, root) = resolve_git_target(&host, &args)?;
        let requested_paths = string_list_arg(&args, "paths")?;
        let staged_only = flag(&args, "stagedOnly");

        let status_cap =
            run_git_capture(&host, &root, &["status", "--porcelain=v2", "-z", "--untracked-files=all"], None, 15, true).await?;
        if status_cap.exit_code != 0 {
            return Err(format_git_failure(&status_cap));
        }
        let parsed = parse_porcelain_v2_status(&status_cap.stdout);

        let target_files: Vec<GitWorkbenchFile> = if staged_only {
            parsed
                .files
                .into_iter()
                .filter(|f| f.staged && (requested_paths.is_empty() || requested_paths.iter().any(|p| p == &f.path)))
                .collect()
        } else if !requested_paths.is_empty() {
            let set: HashSet<&str> = requested_paths.iter().map(String::as_str).collect();
            parsed.files.into_iter().filter(|f| set.contains(f.path.as_str())).collect()
        } else {
            parsed.files.into_iter().filter(|f| !f.conflicted).collect()
        };

        if target_files.is_empty() {
            return Err(host.locale.get("git.commit.error.empty"));
        }

        let all_target_paths: Vec<String> = target_files.iter().map(|f| f.path.clone()).collect();
        let safe_files: Vec<&GitWorkbenchFile> = target_files.iter().filter(|f| !is_secret_file_path(&f.path)).collect();

        let status_short = match run_git_capture(&host, &root, &["status", "--short"], None, 10, true).await {
            Ok(cap) if cap.exit_code == 0 => String::from_utf8_lossy(&cap.stdout).trim().to_string(),
            _ => String::new(),
        };
        let recent_log = match run_git_capture(&host, &root, &["log", "-n", "5", "--oneline"], None, 10, true).await {
            Ok(cap) if cap.exit_code == 0 => String::from_utf8_lossy(&cap.stdout).trim().to_string(),
            _ => String::new(),
        };

        let mut combined_diff = String::new();
        if staged_only {
            let safe_paths: Vec<String> = safe_files.iter().map(|f| f.path.clone()).collect();
            if !safe_paths.is_empty() {
                let mut diff_args: Vec<String> = ["diff", "--cached", "--no-color", "--no-ext-diff", "--no-textconv", "-U2", "--"]
                    .into_iter()
                    .map(String::from)
                    .collect();
                diff_args.extend(safe_paths);
                if let Ok(cap) = run_git_capture_owned(&host, &root, &diff_args, None, 15, true).await
                    && cap.exit_code == 0
                {
                    combined_diff.push_str(&String::from_utf8_lossy(&cap.stdout));
                }
            }
        } else {
            let safe_tracked: Vec<String> = safe_files.iter().filter(|f| !f.untracked).map(|f| f.path.clone()).collect();
            if !safe_tracked.is_empty() {
                let mut diff_args: Vec<String> = if head_exists(&host, &root).await {
                    ["diff", "HEAD", "--no-color", "--no-ext-diff", "--no-textconv", "-U2", "--"].into_iter().map(String::from).collect()
                } else {
                    ["diff", "--no-color", "--no-ext-diff", "--no-textconv", "-U2", "--"].into_iter().map(String::from).collect()
                };
                diff_args.extend(safe_tracked);
                if let Ok(cap) = run_git_capture_owned(&host, &root, &diff_args, None, 15, true).await
                    && cap.exit_code == 0
                {
                    combined_diff.push_str(&String::from_utf8_lossy(&cap.stdout));
                }
            }
            for uf in safe_files.iter().filter(|f| f.untracked) {
                if combined_diff.len() >= MAX_DRAFT_DIFF_CHARS * 2 {
                    break;
                }
                if let Some(resolved) = resolve_file_inside_root(&root, &uf.path)
                    && let Ok(bytes) = read_up_to(&resolved, 16_384)
                    && !bytes[..bytes.len().min(4096)].contains(&0)
                {
                    let snippet = String::from_utf8_lossy(&bytes[..bytes.len().min(16_384)]);
                    combined_diff.push_str(&format!("\n--- /dev/null\n+++ b/{}\n", uf.path));
                    for line in snippet.lines().take(120) {
                        combined_diff.push('+');
                        combined_diff.push_str(line);
                        combined_diff.push('\n');
                    }
                }
            }
        }

        let truncated_diff: String = redact_secret_lines(&combined_diff).chars().take(MAX_DRAFT_DIFF_CHARS).collect();

        if host.smoke.is_none()
            && let Some(runs) = host.runs().cloned()
            && let Some(command) = runs.catalog.find("claude").await
        {
            let env = mighty_run::providers::claude_headless_environment(runs.catalog.environment().as_ref());
            let prompt = build_draft_commit_prompt(&status_short, &recent_log, &truncated_diff, &all_target_paths);
            if let Ok(answer) =
                mighty_workmap::summary::run(host.processes.as_ref(), &command, &env, &prompt, Duration::from_secs(25)).await
            {
                let cleaned = strip_markdown_fences(&answer);
                if !cleaned.is_empty() {
                    return Ok(json!({ "message": cleaned, "source": "ai" }));
                }
            }
        }

        let fallback = heuristic_conventional_commit(&target_files);
        Ok(json!({ "message": fallback, "source": "heuristic" }))
    });

    methods.add("gitWorkbenchCommit", |host: Arc<Host>, args| async move {
        let (_, root) = resolve_git_target(&host, &args)?;
        ensure_local_repo_write(&host, &root)?;

        let message = text(&args, "message").unwrap_or("").trim().to_string();
        if message.is_empty() {
            return Err(host.locale.get("git.commit.error.empty"));
        }

        let requested_paths = string_list_arg(&args, "paths")?;
        let staged_only = flag(&args, "stagedOnly");
        let allow_secrets = flag(&args, "allowSecrets");

        let status_cap =
            run_git_capture(&host, &root, &["status", "--porcelain=v2", "-z", "--untracked-files=all"], None, 15, true).await?;
        if status_cap.exit_code != 0 {
            return Err(format_git_failure(&status_cap));
        }
        let parsed = parse_porcelain_v2_status(&status_cap.stdout);

        if staged_only {
            if !parsed.files.iter().any(|f| f.staged) {
                return Err(host.locale.get("git.commit.error.empty"));
            }
        } else if requested_paths.is_empty() {
            return Err(host.locale.get("git.commit.error.empty"));
        }

        if !allow_secrets {
            let findings = scan_workspace_secrets(&host, &root, &parsed.files, &requested_paths, staged_only).await;
            if !findings.is_empty() {
                return Ok(json!({
                    "committed": false,
                    "blockedBySecrets": true,
                    "findings": findings,
                }));
            }
        }

        if staged_only {
            let commit_cap = run_git_capture_owned(&host, &root, &["commit".into(), "-m".into(), message], None, 120, false).await?;
            if commit_cap.exit_code != 0 {
                return Err(format_git_failure(&commit_cap));
            }
        } else {
            let mut commit_paths = Vec::with_capacity(requested_paths.len() + 4);
            let mut seen = HashSet::new();
            for req_path in &requested_paths {
                if seen.insert(req_path.clone()) {
                    commit_paths.push(req_path.clone());
                }
                if let Some(orig) = parsed
                    .files
                    .iter()
                    .find(|f| &f.path == req_path)
                    .and_then(|f| f.original_path.as_deref())
                    .and_then(|o| validate_repo_relative_path(o).ok())
                    && seen.insert(orig.clone())
                {
                    commit_paths.push(orig);
                }
            }

            let mut add_cmd: Vec<String> = ["add", "-A", "--"].into_iter().map(String::from).collect();
            add_cmd.extend(commit_paths.iter().cloned());
            let add_cap = run_git_capture_owned(&host, &root, &add_cmd, None, 30, false).await?;
            if add_cap.exit_code != 0 {
                return Err(format_git_failure(&add_cap));
            }

            let mut commit_cmd: Vec<String> = vec!["commit".into(), "--only".into(), "-m".into(), message, "--".into()];
            commit_cmd.extend(commit_paths);
            let commit_cap = run_git_capture_owned(&host, &root, &commit_cmd, None, 120, false).await?;
            if commit_cap.exit_code != 0 {
                return Err(format_git_failure(&commit_cap));
            }
        }

        let sha = match run_git_capture(&host, &root, &["rev-parse", "--short", "HEAD"], None, 10, true).await {
            Ok(cap) if cap.exit_code == 0 => String::from_utf8_lossy(&cap.stdout).lines().next().unwrap_or("").trim().to_string(),
            _ => String::new(),
        };

        host.refresh_git(true);
        Ok(json!({
            "committed": true,
            "blockedBySecrets": false,
            "sha": sha,
            "findings": [],
        }))
    });

    methods.add("gitWorkbenchPush", |host: Arc<Host>, args| async move {
        let (_, root) = resolve_git_target(&host, &args)?;
        ensure_machine_write(&host)?;
        push_workspace_branch(&host, &root).await?;
        Ok(json!({ "pushed": true }))
    });

    methods.add("gitWorkbenchPrStatus", |host: Arc<Host>, args| async move {
        let (_, root) = resolve_git_target(&host, &args)?;
        Ok(query_pr_status_value(&host, &root).await)
    });

    methods.add("gitWorkbenchCreatePr", |host: Arc<Host>, args| async move {
        let (ws, root) = resolve_git_target(&host, &args)?;
        ensure_machine_write(&host)?;

        let origin = read_origin_url(&host, &root).await;
        let hosting = detect_hosting(origin.as_deref());
        if hosting == "none" {
            return Err(host.locale.get("git.pr.noRemote"));
        }

        let cli = if hosting == "gitlab" { "glab" } else { "gh" };
        if find_hosting_cli(&host, cli).is_none() {
            let key = if hosting == "gitlab" { "git.pr.error.glabMissing" } else { "git.pr.error.ghMissing" };
            return Err(host.locale.get(key));
        }

        // A pull request is made from a feature branch: never push the default branch (or a worktree's base) on its behalf.
        let head = match run_git_capture(&host, &root, &["symbolic-ref", "--short", "-q", "HEAD"], None, 5, true).await {
            Ok(cap) if cap.exit_code == 0 => String::from_utf8_lossy(&cap.stdout).trim().to_string(),
            _ => String::new(),
        };
        if head.is_empty() {
            return Err("HEAD is detached; switch to a branch before creating a pull request.".into());
        }
        let default = default_branch(&host, &root).await;
        if is_base_branch(&head, default.as_deref(), worktree_base_branch(&host, &args).as_deref()) {
            return Err(format!("`{head}` is the branch pull requests are made into; switch to a feature branch first."));
        }

        push_workspace_branch(&host, &root).await?;

        let custom_title = text(&args, "title").map(str::trim).filter(|s| !s.is_empty());
        let custom_body = text(&args, "body").map(str::trim).filter(|s| !s.is_empty());

        let create_args: Vec<String> = if hosting == "github" {
            let mut cmd = vec!["pr".to_string(), "create".to_string()];
            if custom_title.is_none() && custom_body.is_none() {
                cmd.push("--fill".into());
            } else {
                if let Some(t) = custom_title {
                    cmd.push("--title".into());
                    cmd.push(t.into());
                } else {
                    cmd.push("--fill".into());
                }
                if let Some(b) = custom_body {
                    cmd.push("--body".into());
                    cmd.push(b.into());
                }
            }
            cmd
        } else {
            let mut cmd = vec!["mr".to_string(), "create".to_string(), "--yes".to_string()];
            if custom_title.is_none() && custom_body.is_none() {
                cmd.push("--fill".into());
            } else {
                if let Some(t) = custom_title {
                    cmd.push("--title".into());
                    cmd.push(t.into());
                } else {
                    cmd.push("--fill".into());
                }
                if let Some(b) = custom_body {
                    cmd.push("--description".into());
                    cmd.push(b.into());
                }
            }
            cmd
        };

        let created = run_hosting_cli(&host, cli, &root, &create_args, 60).await?;
        if created.exit_code != 0 {
            return Err(format_git_failure(&created));
        }

        let printed_url = extract_first_url_or_line(&String::from_utf8_lossy(&created.stdout));
        let pr_status = query_pr_status_value(&host, &root).await;
        let final_url = pr_status.get("pr").and_then(|p| p.get("url")).and_then(Value::as_str).map(str::to_string).or(printed_url);

        let branch_name = match run_git_capture(&host, &root, &["symbolic-ref", "--short", "-q", "HEAD"], None, 5, true).await {
            Ok(cap) if cap.exit_code == 0 => String::from_utf8_lossy(&cap.stdout).trim().to_string(),
            _ => "HEAD".into(),
        };
        if let Some((pane_id, pane_record)) = find_linked_task_pane(&host, ws.id(), &branch_name) {
            crate::integrations::on_worktree_pr(&host, &pane_id, &pane_record, final_url.as_deref());
        }

        host.refresh_git(true);
        Ok(json!({
            "hosting": hosting,
            "url": final_url,
            "pr": pr_status.get("pr").cloned().unwrap_or(Value::Null),
        }))
    });

    methods.add("gitWorkbenchFailedCheckLog", |host: Arc<Host>, args| async move {
        let (_, root) = resolve_git_target(&host, &args)?;
        let branch = match text(&args, "branch").map(str::trim).filter(|s| !s.is_empty()) {
            Some(b) => validate_branch_name(b)?,
            None => match run_git_capture(&host, &root, &["symbolic-ref", "--short", "-q", "HEAD"], None, 5, true).await {
                Ok(cap) if cap.exit_code == 0 => String::from_utf8_lossy(&cap.stdout).trim().to_string(),
                _ => String::new(),
            },
        };
        if branch.is_empty() {
            return Ok(json!({ "log": "", "lines": 0 }));
        }

        let origin = read_origin_url(&host, &root).await;
        let hosting = detect_hosting(origin.as_deref());

        if hosting == "github" {
            let list_args = vec![
                "run".into(),
                "list".into(),
                "--branch".into(),
                branch,
                "--limit".into(),
                "5".into(),
                "--json".into(),
                "databaseId,conclusion".into(),
            ];
            let Ok(list_cap) = run_hosting_cli(&host, "gh", &root, &list_args, 20).await else {
                return Ok(json!({ "log": "", "lines": 0 }));
            };
            if list_cap.exit_code != 0 {
                return Ok(json!({ "log": "", "lines": 0 }));
            }
            let runs: Vec<Value> = serde_json::from_slice(&list_cap.stdout).unwrap_or_default();
            let failed_id = runs.iter().find_map(|r| {
                let conclusion = r.get("conclusion").and_then(Value::as_str).unwrap_or("").to_ascii_lowercase();
                if matches!(conclusion.as_str(), "failure" | "timed_out" | "startup_failure" | "cancelled" | "action_required") {
                    r.get("databaseId").and_then(Value::as_i64)
                } else {
                    None
                }
            });
            let Some(run_id) = failed_id else {
                return Ok(json!({ "log": "", "lines": 0 }));
            };
            let view_args = vec!["run".into(), "view".into(), run_id.to_string(), "--log-failed".into()];
            let Ok(log_cap) = run_hosting_cli(&host, "gh", &root, &view_args, 60).await else {
                return Ok(json!({ "log": "", "lines": 0 }));
            };
            if log_cap.exit_code != 0 {
                return Ok(json!({ "log": "", "lines": 0 }));
            }
            let (log, lines) = extract_log_tail(&String::from_utf8_lossy(&log_cap.stdout), FAILED_LOG_TAIL_LINES);
            return Ok(json!({ "log": log, "lines": lines }));
        }

        if hosting == "gitlab" {
            let mr_args = vec!["mr".into(), "view".into(), "--output".into(), "json".into()];
            let Ok(mr_cap) = run_hosting_cli(&host, "glab", &root, &mr_args, 20).await else {
                return Ok(json!({ "log": "", "lines": 0 }));
            };
            if mr_cap.exit_code != 0 {
                return Ok(json!({ "log": "", "lines": 0 }));
            }
            let Ok(mr_json) = serde_json::from_slice::<Value>(&mr_cap.stdout) else {
                return Ok(json!({ "log": "", "lines": 0 }));
            };
            let project_id = mr_json.get("project_id").and_then(Value::as_i64);
            let pipeline_id =
                mr_json.get("head_pipeline").or_else(|| mr_json.get("pipeline")).and_then(|p| p.get("id")).and_then(Value::as_i64);
            let (Some(proj_id), Some(pipe_id)) = (project_id, pipeline_id) else {
                return Ok(json!({ "log": "", "lines": 0 }));
            };

            let jobs_endpoint = format!("projects/{proj_id}/pipelines/{pipe_id}/jobs");
            let Ok(jobs_cap) = run_hosting_cli(&host, "glab", &root, &["api".into(), jobs_endpoint], 20).await else {
                return Ok(json!({ "log": "", "lines": 0 }));
            };
            if jobs_cap.exit_code != 0 {
                return Ok(json!({ "log": "", "lines": 0 }));
            }
            let jobs: Vec<Value> = serde_json::from_slice(&jobs_cap.stdout).unwrap_or_default();
            let failed_job_id = jobs.iter().find_map(|j| {
                let st = j.get("status").and_then(Value::as_str).unwrap_or("");
                (st.eq_ignore_ascii_case("failed")).then(|| j.get("id").and_then(Value::as_i64)).flatten()
            });
            let Some(job_id) = failed_job_id else {
                return Ok(json!({ "log": "", "lines": 0 }));
            };

            let trace_endpoint = format!("projects/{proj_id}/jobs/{job_id}/trace");
            let Ok(trace_cap) = run_hosting_cli(&host, "glab", &root, &["api".into(), trace_endpoint], 60).await else {
                return Ok(json!({ "log": "", "lines": 0 }));
            };
            if trace_cap.exit_code != 0 {
                return Ok(json!({ "log": "", "lines": 0 }));
            }
            let (log, lines) = extract_log_tail(&String::from_utf8_lossy(&trace_cap.stdout), FAILED_LOG_TAIL_LINES);
            return Ok(json!({ "log": log, "lines": lines }));
        }

        Ok(json!({ "log": "", "lines": 0 }))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_leave_urls_and_git_messages() {
        assert_eq!(strip_url_credentials("https://user:ghp_secret@github.com/a/b.git"), "https://github.com/a/b.git");
        assert_eq!(
            strip_url_credentials("fatal: unable to access 'https://tok@host.example/x/': 403 and ssh://git@host/x"),
            "fatal: unable to access 'https://host.example/x/': 403 and ssh://host/x"
        );
        assert_eq!(strip_url_credentials("git@github.com:a/b.git"), "git@github.com:a/b.git");
    }

    #[test]
    fn secret_lines_are_replaced_before_a_diff_goes_to_a_model() {
        let diff = "+++ b/a.txt\n+token = ghp_0123456789abcdefghijklmnopqrstuvwxyz\n+fine line\n-----BEGIN RSA PRIVATE KEY-----\n";
        let redacted = redact_secret_lines(diff);
        assert!(!redacted.contains("ghp_"), "{redacted}");
        assert!(redacted.contains("+[redacted: GitHub token]"));
        assert!(redacted.contains("+fine line"));
        assert!(!redacted.contains("PRIVATE KEY-----\n") || redacted.contains("[redacted: Private key]"));
    }

    #[test]
    fn a_pull_request_is_never_made_from_the_default_or_base_branch() {
        assert!(is_base_branch("main", None, None));
        assert!(is_base_branch("trunk", Some("trunk"), None));
        assert!(is_base_branch("release/1", None, Some("release/1")));
        assert!(!is_base_branch("mighty/abc", Some("main"), Some("main")));
    }

    #[test]
    fn every_workbench_git_run_carries_the_hardened_settings() {
        let settings = hardened_settings(Path::new("/usr/bin/git"), "/dev/null");
        for needed in [
            "core.hooksPath=/dev/null",
            "core.fsmonitor=false",
            "protocol.allow=never",
            "protocol.https.allow=always",
            "commit.gpgsign=false",
        ] {
            assert!(settings.iter().any(|s| s == needed), "{needed} missing from {settings:?}");
        }
        assert!(!settings.iter().any(|s| s == "protocol.ext.allow=always"));
    }

    #[test]
    fn parses_porcelain_v2_nul_separated_status_entries_and_headers() {
        let raw = concat!(
            "# branch.oid 0123456789abcdef\0",
            "# branch.head feat/git-workbench\0",
            "# branch.upstream origin/feat/git-workbench\0",
            "# branch.ab +3 -1\0",
            "1 .M N... 100644 100644 100644 aaa bbb src/a b.rs\0",
            "1 MM N... 100644 100644 100644 aaa bbb src/both.rs\0",
            "2 R. N... 100644 100644 100644 aaa bbb R100 src/new_name.rs\0src/old_name.rs\0",
            "u UU N... 100644 100644 100644 100644 a b c src/conflict.rs\0",
            "? docs/untracked file.md\0",
            "! target/ignored.o\0",
            "1 D. N... 100644 000000 000000 aaa 000 src/deleted.rs\0"
        );

        let parsed = parse_porcelain_v2_status(raw.as_bytes());
        assert_eq!(parsed.head, "feat/git-workbench");
        assert!(!parsed.detached);
        assert_eq!(parsed.upstream.as_deref(), Some("origin/feat/git-workbench"));
        assert_eq!(parsed.ahead, 3);
        assert_eq!(parsed.behind, 1);
        assert_eq!(parsed.files.len(), 6);

        assert_eq!(parsed.files[0].path, "src/a b.rs");
        assert_eq!(parsed.files[0].mark, "M");
        assert!(!parsed.files[0].staged && parsed.files[0].unstaged);

        assert_eq!(parsed.files[1].path, "src/both.rs");
        assert!(parsed.files[1].staged && parsed.files[1].unstaged);

        assert_eq!(parsed.files[2].path, "src/new_name.rs");
        assert_eq!(parsed.files[2].original_path.as_deref(), Some("src/old_name.rs"));
        assert_eq!(parsed.files[2].mark, "R");
        assert!(parsed.files[2].staged && !parsed.files[2].unstaged);

        assert_eq!(parsed.files[3].path, "src/conflict.rs");
        assert_eq!(parsed.files[3].mark, "!");
        assert!(parsed.files[3].conflicted && !parsed.files[3].staged && parsed.files[3].unstaged);

        assert_eq!(parsed.files[4].path, "docs/untracked file.md");
        assert_eq!(parsed.files[4].mark, "?");
        assert!(parsed.files[4].untracked && !parsed.files[4].staged && parsed.files[4].unstaged);

        assert_eq!(parsed.files[5].path, "src/deleted.rs");
        assert_eq!(parsed.files[5].mark, "D");
        assert!(parsed.files[5].staged && !parsed.files[5].unstaged);

        let detached = parse_porcelain_v2_status(b"# branch.oid deadbeef\0# branch.head (detached)\0");
        assert!(detached.detached);
        assert_eq!(detached.head, "(detached)");
    }

    #[test]
    fn parses_unified_diff_hunks_and_builds_complete_raw_patches() {
        let diff = "\
diff --git a/src/lib.rs b/src/lib.rs
index 1111111..2222222 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -10,3 +10,4 @@ fn alpha() {
     let x = 1;
-    let y = 2;
+    let y = 20;
+    let z = 30;
     println!(\"{x}\");
@@ -40,2 +41,2 @@ fn beta() {
-    old_call();
+    new_call();
 }
\\ No newline at end of file
";
        let structured = parse_unified_diff(diff, "src/lib.rs", false);
        assert!(!structured.binary);
        assert!(!structured.truncated);
        assert_eq!(structured.added, 3);
        assert_eq!(structured.removed, 2);
        assert_eq!(structured.hunks.len(), 2);

        let h0 = &structured.hunks[0];
        assert_eq!(h0.index, 0);
        assert_eq!(h0.old_start, 10);
        assert_eq!(h0.old_lines, 3);
        assert_eq!(h0.new_start, 10);
        assert_eq!(h0.new_lines, 4);
        assert_eq!(h0.lines.len(), 5);
        assert_eq!((h0.lines[0].kind.as_str(), h0.lines[0].old_line, h0.lines[0].new_line), ("context", Some(10), Some(10)));
        assert_eq!((h0.lines[1].kind.as_str(), h0.lines[1].old_line, h0.lines[1].new_line), ("del", Some(11), None));
        assert_eq!((h0.lines[2].kind.as_str(), h0.lines[2].old_line, h0.lines[2].new_line), ("add", None, Some(11)));
        assert_eq!((h0.lines[3].kind.as_str(), h0.lines[3].old_line, h0.lines[3].new_line), ("add", None, Some(12)));
        assert!(h0.raw_patch.starts_with("diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -10,3 +10,4 @@"));

        let h1 = &structured.hunks[1];
        assert_eq!(h1.index, 1);
        assert!(h1.raw_patch.contains("\\ No newline at end of file\n"));

        let untracked = build_untracked_file_diff("notes/todo.txt", b"first\nsecond\n");
        assert_eq!(untracked.added, 2);
        assert_eq!(untracked.hunks.len(), 1);
        assert!(
            untracked.hunks[0]
                .raw_patch
                .contains("new file mode 100644\n--- /dev/null\n+++ b/notes/todo.txt\n@@ -0,0 +1,2 @@\n+first\n+second\n")
        );

        let binary = build_untracked_file_diff("assets/icon.bin", &[0x89, b'P', b'N', b'G', 0x00, 0x01]);
        assert!(binary.binary && binary.hunks.is_empty());
    }

    #[test]
    fn scans_sensitive_paths_and_all_nine_secret_patterns_without_leaking_values() {
        assert!(is_secret_file_path(".env"));
        assert!(is_secret_file_path(".env.production"));
        assert!(is_secret_file_path("deploy/putty.ppk"));
        assert!(is_secret_file_path("certs/server.pem"));
        assert!(!is_secret_file_path(".env.example"));
        assert!(!is_secret_file_path(".env.template"));
        assert!(!is_secret_file_path("src/git_workbench.rs"));

        let aws = "AKIAABCDEFGHIJKLMNOP";
        let gh = format!("ghp_{}", "A".repeat(36));
        let gl = format!("glpat-{}", "b".repeat(22));
        let slack = "xoxb-123456789012-abcdefghijkl";
        let anthropic = format!("sk-ant-api03-{}", "x".repeat(28));
        let openai = format!("sk-proj-{}", "y".repeat(36));
        let google = format!("AIza{}", "z".repeat(35));
        let stripe = format!("sk_live_{}", "9".repeat(24));
        let priv_key = "-----BEGIN OPENSSH PRIVATE KEY-----";

        let diff = format!(
            "diff --git a/src/config.ts b/src/config.ts\n\
             --- a/src/config.ts\n\
             +++ b/src/config.ts\n\
             @@ -4,2 +10,10 @@\n\
              const unchanged = true;\n\
             -const removedOldAws = \"{aws}\";\n\
             +const k1 = \"{aws}\";\n\
             +const k2 = \"{gh}\";\n\
             +const k3 = \"{gl}\";\n\
             +const k4 = \"{slack}\";\n\
             +const k5 = \"{anthropic}\";\n\
             +const k6 = \"{openai}\";\n\
             +const k7 = \"{google}\";\n\
             +const k8 = \"{stripe}\";\n\
             +{priv_key}\n"
        );

        let findings = scan_diff_secrets(&diff);
        let mapped: Vec<(usize, &str)> = findings.iter().map(|f| (f.line, f.kind.as_str())).collect();
        assert_eq!(
            mapped,
            vec![
                (11, "AWS access key"),
                (12, "GitHub token"),
                (13, "GitLab token"),
                (14, "Slack token"),
                (15, "Anthropic key"),
                (16, "OpenAI key"),
                (17, "Google API key"),
                (18, "Stripe key"),
                (19, "Private key"),
            ]
        );

        let serialized = serde_json::to_string(&findings).unwrap();
        for secret in
            [aws, gh.as_str(), gl.as_str(), slack, anthropic.as_str(), openai.as_str(), google.as_str(), stripe.as_str(), priv_key]
        {
            assert!(!serialized.contains(secret), "serialized finding must never leak secret value");
        }
    }

    #[test]
    fn parses_github_and_gitlab_pr_check_rollups() {
        let gh_failed = br#"{
            "number": 42,
            "url": "https://github.com/org/repo/pull/42",
            "state": "OPEN",
            "title": "feat: add git workbench",
            "isDraft": false,
            "statusCheckRollup": [
                {"status": "IN_PROGRESS", "conclusion": ""},
                {"status": "COMPLETED", "conclusion": "FAILURE"}
            ]
        }"#;
        let pr = parse_github_pr(gh_failed).unwrap();
        assert_eq!(pr.number, 42);
        assert_eq!(pr.state, "open");
        assert_eq!(pr.checks, "failed");
        assert_eq!(pr.title, "feat: add git workbench");

        let gh_pending =
            br#"{"number": 8, "url": "https://github.com/o/r/pull/8", "state": "OPEN", "statusCheckRollup": [{"state": "PENDING"}]}"#;
        assert_eq!(parse_github_pr(gh_pending).unwrap().checks, "pending");

        let gh_passed =
            br#"{"number": 9, "url": "https://github.com/o/r/pull/9", "state": "MERGED", "statusCheckRollup": [{"status": "COMPLETED", "conclusion": "SUCCESS"}]}"#;
        let merged = parse_github_pr(gh_passed).unwrap();
        assert_eq!(merged.state, "merged");
        assert_eq!(merged.checks, "passed");

        let gl_json = br#"{
            "iid": 15,
            "web_url": "https://gitlab.example.com/group/proj/-/merge_requests/15",
            "state": "opened",
            "title": "Draft: fix pipeline",
            "draft": true,
            "head_pipeline": {"id": 99, "status": "failed"}
        }"#;
        let mr = parse_gitlab_mr(gl_json).unwrap();
        assert_eq!(mr.number, 15);
        assert_eq!(mr.state, "open");
        assert!(mr.is_draft);
        assert_eq!(mr.checks, "failed");

        assert_eq!(detect_hosting(Some("git@github.com:org/repo.git")), "github");
        assert_eq!(detect_hosting(Some("https://gitlab.company.io/team/app.git")), "gitlab");
        assert_eq!(detect_hosting(None), "none");
    }

    #[test]
    fn strips_ansi_sequences_and_keeps_last_eighty_non_empty_lines() {
        let mut lines = Vec::new();
        for idx in 1..=105 {
            lines.push(format!("\x1b[31merror[{idx:03}]\x1b[0m: check step failed\r"));
            if idx % 3 == 0 {
                lines.push("   \r".to_string());
            }
        }
        let raw = lines.join("\n");
        let (tail, count) = extract_log_tail(&raw, 80);
        assert_eq!(count, 80);
        assert!(!tail.contains("\x1b["));
        assert!(tail.starts_with("error[026]: check step failed\n"));
        assert!(tail.ends_with("error[105]: check step failed"));
    }

    #[test]
    fn validates_paths_branches_fences_and_conventional_commit_fallback() {
        assert!(validate_repo_relative_path("src/git/GitWorkbenchPanel.tsx").is_ok());
        assert!(validate_repo_relative_path("../outside.txt").is_err());
        assert!(validate_repo_relative_path("/etc/passwd").is_err());
        assert!(validate_repo_relative_path("bad\0name").is_err());

        assert!(validate_branch_name("feat/mc-5-workbench").is_ok());
        assert!(validate_branch_name("--upload-pack=evil").is_err());
        assert!(validate_branch_name("bad..branch").is_err());
        assert!(validate_branch_name("has space").is_err());

        assert_eq!(strip_markdown_fences("```text\nfeat(git): add panel\n```"), "feat(git): add panel");

        let fallback = heuristic_conventional_commit(&[
            GitWorkbenchFile {
                path: "native/windows/web/src/git/GitWorkbenchPanel.tsx".into(),
                original_path: None,
                mark: "A".into(),
                index_status: "A".into(),
                worktree_status: " ".into(),
                staged: true,
                unstaged: false,
                untracked: false,
                conflicted: false,
            },
            GitWorkbenchFile {
                path: "native/windows/web/src/git/gitWorkbenchModel.ts".into(),
                original_path: None,
                mark: "?".into(),
                index_status: "?".into(),
                worktree_status: "?".into(),
                staged: false,
                unstaged: true,
                untracked: true,
                conflicted: false,
            },
        ]);
        assert!(fallback.starts_with("feat(git): add "));
        assert!(fallback.contains("- native/windows/web/src/git/GitWorkbenchPanel.tsx"));
    }
}
