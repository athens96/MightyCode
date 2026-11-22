//! Unified retention caps, TTLs, and safe profile directory garbage collection (`MC-12`, `docs/memory-policy.md`).
//!
//! Safety invariants:
//! - Never follows symlinks and never touches user files outside `<data_dir>` (such as `~/.claude`, `~/.codex`, `~/.gemini`).
//! - Never modifies `saved-prompts.json`, `workspace-state.json`, `workspace-state.json.bak`, `usage-ledger.json`,
//!   `known-sessions.json`, `remote-connections.json`, or `antigravity-temporary-rules.json`.
//! - Temporary `.tmp` files modified within the last 10 minutes are untouched so concurrent atomic writes remain completely safe.
//! - Cached images referenced by live or cold pane bodies (`referenced_images` and `sessions/*.json`) are excluded from eviction.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use crate::model::AppSnapshot;
use crate::store::SESSIONS_DIR;

pub const DEFAULT_IDLE_BODY_TTL_SECS: u64 = 300;
pub const DEFAULT_MAX_HOT_INACTIVE_PANES: usize = 8;
pub const DEFAULT_QUARANTINE_KEEP_DAYS: u64 = 30;
pub const DEFAULT_QUARANTINE_MAX_FILES: usize = 3;
pub const DEFAULT_SNAPSHOT_KEEP_DAYS: u64 = 14;
pub const DEFAULT_SNAPSHOT_MAX_REPO_BYTES: u64 = 256 * 1024 * 1024;
pub const DEFAULT_SNAPSHOT_MAX_TOTAL_BYTES: u64 = 1024 * 1024 * 1024;
pub const DEFAULT_IMAGE_CACHE_KEEP_DAYS: u64 = 30;
pub const DEFAULT_IMAGE_CACHE_MAX_BYTES: u64 = 200 * 1024 * 1024;
pub const DEFAULT_WORKMAP_KEEP_DAYS: u64 = 30;
pub const DEFAULT_WORKMAP_MAX_BYTES: u64 = 32 * 1024 * 1024;
pub const DEFAULT_REMOTE_CACHE_KEEP_DAYS: u64 = 14;
pub const DEFAULT_REMOTE_CACHE_MAX_BYTES: u64 = 64 * 1024 * 1024;
pub const DEFAULT_HOST_LOG_MAX_BYTES: u64 = 10 * 1024 * 1024;

const STALE_TMP_AGE: Duration = Duration::from_secs(10 * 60);
const SECS_PER_DAY: u64 = 24 * 3600;

/// Unified retention thresholds and storage caps for a Mighty Code profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetentionPolicy {
    pub idle_body_ttl_secs: u64,
    pub max_hot_inactive_panes: usize,
    pub quarantine_keep_days: u64,
    pub quarantine_max_files: usize,
    pub snapshot_keep_days: u64,
    pub snapshot_max_repo_bytes: u64,
    pub snapshot_max_total_bytes: u64,
    pub image_cache_keep_days: u64,
    pub image_cache_max_bytes: u64,
    pub workmap_keep_days: u64,
    pub workmap_max_bytes: u64,
    pub remote_cache_keep_days: u64,
    pub remote_cache_max_bytes: u64,
    pub host_log_max_bytes: u64,
}

impl Default for RetentionPolicy {
    fn default() -> Self {
        RetentionPolicy {
            idle_body_ttl_secs: DEFAULT_IDLE_BODY_TTL_SECS,
            max_hot_inactive_panes: DEFAULT_MAX_HOT_INACTIVE_PANES,
            quarantine_keep_days: DEFAULT_QUARANTINE_KEEP_DAYS,
            quarantine_max_files: DEFAULT_QUARANTINE_MAX_FILES,
            snapshot_keep_days: DEFAULT_SNAPSHOT_KEEP_DAYS,
            snapshot_max_repo_bytes: DEFAULT_SNAPSHOT_MAX_REPO_BYTES,
            snapshot_max_total_bytes: DEFAULT_SNAPSHOT_MAX_TOTAL_BYTES,
            image_cache_keep_days: DEFAULT_IMAGE_CACHE_KEEP_DAYS,
            image_cache_max_bytes: DEFAULT_IMAGE_CACHE_MAX_BYTES,
            workmap_keep_days: DEFAULT_WORKMAP_KEEP_DAYS,
            workmap_max_bytes: DEFAULT_WORKMAP_MAX_BYTES,
            remote_cache_keep_days: DEFAULT_REMOTE_CACHE_KEEP_DAYS,
            remote_cache_max_bytes: DEFAULT_REMOTE_CACHE_MAX_BYTES,
            host_log_max_bytes: DEFAULT_HOST_LOG_MAX_BYTES,
        }
    }
}

/// Summary of items reclaimed by a retention / garbage-collection pass.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GcReport {
    pub evicted_panes: usize,
    pub quarantine_removed: usize,
    pub temp_files_removed: usize,
    pub image_cache_removed: usize,
    pub workmap_removed: usize,
    pub remote_cache_removed: usize,
    pub snapshot_stores_swept: usize,
    pub usage_sessions_pruned: usize,
    pub upload_parts_removed: usize,
    pub bytes_reclaimed: u64,
}

impl GcReport {
    pub fn total_files_removed(&self) -> usize {
        self.quarantine_removed
            + self.temp_files_removed
            + self.image_cache_removed
            + self.workmap_removed
            + self.remote_cache_removed
            + self.upload_parts_removed
    }
}

#[derive(Debug, Clone)]
struct FileEntry {
    path: PathBuf,
    name: String,
    len: u64,
    modified: SystemTime,
}

fn is_plain_dir(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
}

fn is_protected_root_file(name: &str) -> bool {
    matches!(
        name,
        "saved-prompts.json"
            | "saved-prompts.json.bak"
            | "shared-memory.json"
            | "shared-memory.json.bak"
            | "remote-sharing.json"
            | "remote-sharing.json.bak"
            | "workspace-state.json"
            | "workspace-state.json.bak"
            | "usage-ledger.json"
            | "known-sessions.json"
            | "remote-connections.json"
            | "antigravity-temporary-rules.json"
    )
}

fn age_of(modified: SystemTime, now: SystemTime) -> Duration {
    now.duration_since(modified).unwrap_or(Duration::ZERO)
}

/// A file `Store` moved aside because it could not be read (`.unreadable-*`). The `.before-session-files-*` copy of the whole state
/// from before the split is the one copy of what was there, so it is never swept (docs/memory-policy.md §3.1).
fn is_quarantine_name(name: &str) -> bool {
    name.contains(".unreadable-")
}

fn shallow_regular_files(dir: &Path) -> Vec<FileEntry> {
    if !is_plain_dir(dir) {
        return Vec::new();
    }
    let Ok(read) = fs::read_dir(dir) else { return Vec::new() };
    let mut out = Vec::new();
    for entry in read.flatten() {
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_symlink() || !ft.is_file() {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        out.push(FileEntry {
            path: entry.path(),
            name: entry.file_name().to_string_lossy().into_owned(),
            len: meta.len(),
            modified: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
        });
    }
    out
}

fn recursive_regular_files(root: &Path) -> Vec<FileEntry> {
    if !is_plain_dir(root) {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(read) = fs::read_dir(&dir) else { continue };
        for entry in read.flatten() {
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_symlink() {
                continue;
            }
            let path = entry.path();
            if ft.is_dir() {
                stack.push(path);
                continue;
            }
            if !ft.is_file() {
                continue;
            }
            let Ok(meta) = entry.metadata() else { continue };
            out.push(FileEntry {
                path,
                name: entry.file_name().to_string_lossy().into_owned(),
                len: meta.len(),
                modified: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            });
        }
    }
    out
}

fn remove_empty_subdirs(root: &Path) {
    if !is_plain_dir(root) {
        return;
    }
    let Ok(read) = fs::read_dir(root) else { return };
    for entry in read.flatten() {
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_dir() && !ft.is_symlink() {
            let sub = entry.path();
            remove_empty_subdirs(&sub);
            let _ = fs::remove_dir(&sub);
        }
    }
}

/// The hashes of the images the panes of `snapshot` hold in memory (hot panes' logs and graph runs).
fn snapshot_image_hashes(snapshot: &AppSnapshot, hashes: &mut HashSet<String>) {
    for pane in snapshot.sessions() {
        for entry in pane.logs() {
            for img in entry.images.iter().flatten().flatten() {
                if let Some(h) = img.hash.as_deref() {
                    hashes.insert(h.to_ascii_lowercase());
                }
            }
        }
        for run in pane.graph_runs() {
            for entry in run.root_entries().iter().chain(run.result_entries().iter()) {
                for img in entry.images.iter().flatten().flatten() {
                    if let Some(h) = img.hash.as_deref() {
                        hashes.insert(h.to_ascii_lowercase());
                    }
                }
            }
            for agent in run.agents() {
                for entry in agent.entries() {
                    for img in entry.images.iter().flatten().flatten() {
                        if let Some(h) = img.hash.as_deref() {
                            hashes.insert(h.to_ascii_lowercase());
                        }
                    }
                }
            }
        }
    }
}

/// The files of `image_cache_dir` whose hash is in `hashes`.
fn cached_images_of(image_cache_dir: &Path, hashes: &HashSet<String>) -> HashSet<PathBuf> {
    if hashes.is_empty() || !is_plain_dir(image_cache_dir) {
        return HashSet::new();
    }
    let mut paths = HashSet::new();
    for file in shallow_regular_files(image_cache_dir) {
        if let Some((stem, _)) = file.name.split_once('.')
            && hashes.contains(&stem.to_ascii_lowercase())
        {
            paths.insert(file.path);
        }
    }
    paths
}

/// Cached image file paths referenced by the panes of `snapshot` in memory only. [`sweep_profile_dir`] adds what the files in
/// `sessions/` reference itself, so a pass that passes this reads each session file once.
pub fn collect_snapshot_images(image_cache_dir: &Path, snapshot: &AppSnapshot) -> HashSet<PathBuf> {
    let mut hashes: HashSet<String> = HashSet::new();
    snapshot_image_hashes(snapshot, &mut hashes);
    cached_images_of(image_cache_dir, &hashes)
}

/// Collects cached image file paths referenced by `snapshot` (hot panes) and by on-disk session bodies in `<data_dir>/sessions`.
pub fn collect_referenced_images(data_dir: &Path, image_cache_dir: &Path, snapshot: Option<&AppSnapshot>) -> HashSet<PathBuf> {
    let mut hashes: HashSet<String> = HashSet::new();
    if let Some(snapshot) = snapshot {
        snapshot_image_hashes(snapshot, &mut hashes);
    }
    scan_session_files_for_hashes(&data_dir.join(SESSIONS_DIR), &mut hashes);
    cached_images_of(image_cache_dir, &hashes)
}

fn scan_session_files_for_hashes(sessions_dir: &Path, hashes: &mut HashSet<String>) {
    for file in shallow_regular_files(sessions_dir) {
        if !(file.name.ends_with(".json") || file.name.ends_with(".jsonl")) {
            continue;
        }
        let Ok(bytes) = fs::read(&file.path) else { continue };
        let Ok(text) = std::str::from_utf8(&bytes) else { continue };
        let needle = "\"hash\":\"";
        let mut rest = text;
        while let Some(pos) = rest.find(needle) {
            let after = &rest[pos + needle.len()..];
            if after.len() >= 64 && after.as_bytes()[..64].iter().all(u8::is_ascii_hexdigit) {
                hashes.insert(after[..64].to_ascii_lowercase());
            }
            rest = after;
        }
    }
}

fn path_key_set(paths: &HashSet<PathBuf>) -> (HashSet<PathBuf>, HashSet<String>) {
    let mut exact = HashSet::with_capacity(paths.len() * 2);
    let mut names = HashSet::with_capacity(paths.len());
    for p in paths {
        exact.insert(p.clone());
        if let Ok(canon) = fs::canonicalize(p) {
            exact.insert(canon);
        }
        if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
            names.insert(name.to_string());
        }
    }
    (exact, names)
}

fn sweep_bounded_cache(
    cache_dir: &Path,
    keep_days: u64,
    max_bytes: u64,
    now: SystemTime,
    excluded: Option<&HashSet<PathBuf>>,
    temp_removed: &mut usize,
    bytes_reclaimed: &mut u64,
) -> usize {
    if !is_plain_dir(cache_dir) {
        return 0;
    }
    let (excluded_paths, excluded_names) = excluded.map(path_key_set).unwrap_or_default();
    let is_excluded = |entry: &FileEntry| -> bool {
        excluded.is_some()
            && (excluded_paths.contains(&entry.path)
                || excluded_names.contains(&entry.name)
                || fs::canonicalize(&entry.path).is_ok_and(|c| excluded_paths.contains(&c)))
    };

    let all_files = recursive_regular_files(cache_dir);
    let max_age = Duration::from_secs(keep_days.saturating_mul(SECS_PER_DAY));
    let mut kept: Vec<FileEntry> = Vec::new();
    let mut removed = 0usize;

    for file in all_files {
        if file.name.ends_with(".tmp") {
            if age_of(file.modified, now) > STALE_TMP_AGE && fs::remove_file(&file.path).is_ok() {
                *temp_removed += 1;
                *bytes_reclaimed += file.len;
            }
            continue;
        }
        if !is_excluded(&file) && age_of(file.modified, now) > max_age {
            if fs::remove_file(&file.path).is_ok() {
                removed += 1;
                *bytes_reclaimed += file.len;
            }
            continue;
        }
        kept.push(file);
    }

    let mut total: u64 = kept.iter().map(|f| f.len).sum();
    if total > max_bytes {
        kept.sort_by(|a, b| a.modified.cmp(&b.modified).then_with(|| a.name.cmp(&b.name)));
        for file in kept {
            if total <= max_bytes {
                break;
            }
            if is_excluded(&file) {
                continue;
            }
            if fs::remove_file(&file.path).is_ok() {
                total = total.saturating_sub(file.len);
                removed += 1;
                *bytes_reclaimed += file.len;
            }
        }
    }

    remove_empty_subdirs(cache_dir);
    removed
}

/// Sweeps expired `.part` upload staging files inside `uploads_dir` older than `max_age`.
pub fn sweep_expired_upload_parts(uploads_dir: &Path, max_age: Duration) -> (usize, u64) {
    if !is_plain_dir(uploads_dir) {
        return (0, 0);
    }
    let now = SystemTime::now();
    let mut removed = 0usize;
    let mut reclaimed = 0u64;
    for file in shallow_regular_files(uploads_dir) {
        if file.name.ends_with(".part") && age_of(file.modified, now) > max_age && fs::remove_file(&file.path).is_ok() {
            removed += 1;
            reclaimed += file.len;
        }
    }
    (removed, reclaimed)
}

/// Safely sweeps `<data_dir>` according to `policy`:
/// - Quarantined `.unreadable-*` files in `<data_dir>` and `<data_dir>/sessions` past their keep period (`.before-session-files-*` is never swept)
/// - Stale `.tmp` files (`> 10 min`)
/// - `image-cache/` aged & oversize files (excluding `referenced_images` and hashes referenced by `sessions/*.json`)
/// - `work-map/` aged & oversize files
/// - `remote-cache/` aged & oversize files
pub fn sweep_profile_dir(data_dir: &Path, policy: &RetentionPolicy, referenced_images: &HashSet<PathBuf>) -> GcReport {
    let mut report = GcReport::default();
    if !is_plain_dir(data_dir) {
        return report;
    }
    let now = SystemTime::now();
    let sessions_dir = data_dir.join(SESSIONS_DIR);

    // 1. Stale `.tmp` files and quarantine files in `<data_dir>` and `<data_dir>/sessions`.
    let quarantine_age = Duration::from_secs(policy.quarantine_keep_days.saturating_mul(SECS_PER_DAY));
    let mut aged_quarantine: Vec<FileEntry> = Vec::new();
    for (is_root, folder) in [(true, data_dir), (false, sessions_dir.as_path())] {
        for file in shallow_regular_files(folder) {
            if is_root && is_protected_root_file(&file.name) {
                continue;
            }
            if file.name.ends_with(".tmp") {
                if age_of(file.modified, now) > STALE_TMP_AGE && fs::remove_file(&file.path).is_ok() {
                    report.temp_files_removed += 1;
                    report.bytes_reclaimed += file.len;
                }
                continue;
            }
            if is_quarantine_name(&file.name) && age_of(file.modified, now) > quarantine_age {
                aged_quarantine.push(file);
            }
        }
    }
    // A quarantined file is the person's to look at: one younger than its keep period is never removed, whatever their number. Of
    // those past it, only the newest `quarantine_max_files` stay.
    aged_quarantine.sort_by(|a, b| b.modified.cmp(&a.modified).then_with(|| b.name.cmp(&a.name)));
    for extra in aged_quarantine.into_iter().skip(policy.quarantine_max_files) {
        if fs::remove_file(&extra.path).is_ok() {
            report.quarantine_removed += 1;
            report.bytes_reclaimed += extra.len;
        }
    }

    // 2. `image-cache` (protecting both caller-supplied `referenced_images` and any hashes referenced in `sessions/`).
    let image_cache_dir = data_dir.join("image-cache");
    let mut protected_images = collect_referenced_images(data_dir, &image_cache_dir, None);
    protected_images.extend(referenced_images.iter().cloned());
    report.image_cache_removed = sweep_bounded_cache(
        &image_cache_dir,
        policy.image_cache_keep_days,
        policy.image_cache_max_bytes,
        now,
        Some(&protected_images),
        &mut report.temp_files_removed,
        &mut report.bytes_reclaimed,
    );

    // 3. `work-map`.
    report.workmap_removed = sweep_bounded_cache(
        &data_dir.join("work-map"),
        policy.workmap_keep_days,
        policy.workmap_max_bytes,
        now,
        None,
        &mut report.temp_files_removed,
        &mut report.bytes_reclaimed,
    );

    // 4. `remote-cache`.
    report.remote_cache_removed = sweep_bounded_cache(
        &data_dir.join("remote-cache"),
        policy.remote_cache_keep_days,
        policy.remote_cache_max_bytes,
        now,
        None,
        &mut report.temp_files_removed,
        &mut report.bytes_reclaimed,
    );

    report
}
