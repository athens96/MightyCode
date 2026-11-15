//! The change watcher (C# `StyleChangeWatcher.cs`): tells its owner that something changed under a folder, so styles and what they
//! read are read again on a file change. Changes are gathered for [`StyleChangeWatcher::latency`] (500 ms unless told otherwise; the
//! host gives a workspace's folders 2 s) and handed over together on the watcher's thread. A folder that is not there yet is waited
//! for, and one taken away is reported. C# listens to `FileSystemWatcher`; this crate has no OS code, so the folder is looked at
//! (plain std::fs, bounded) a few times per latency instead — the host watches only the folders a style can be moved by
//! (`.claude\mighty-styles` and the active style's source folders), never a whole workspace. More than 256 paths in one report is an
//! empty report: read everything again. Off in a smoke (the host's rule). Owner: W9-D.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

use crate::manifest::StyleManifest;

/// More changed paths than this in one report are a re-scan.
const MAXIMUM_PATHS: usize = 256;
/// The most entries one look reads, and how deep.
const MAXIMUM_ENTRIES: usize = 4096;
const MAXIMUM_DEPTH: usize = 8;

/// What one look saw: each entry's kind, size and time.
type Snapshot = BTreeMap<PathBuf, (bool, u64, Option<SystemTime>)>;

fn look(root: &Path) -> Option<Snapshot> {
    if !root.is_dir() {
        return None;
    }
    let mut seen = Snapshot::new();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((folder, depth)) = stack.pop() {
        let Ok(listing) = std::fs::read_dir(&folder) else { continue };
        for entry in listing.filter_map(|e| e.ok()) {
            if seen.len() >= MAXIMUM_ENTRIES {
                return Some(seen);
            }
            let path = entry.path();
            let Ok(meta) = std::fs::symlink_metadata(&path) else { continue };
            // A link or a junction is not followed: its own change is what counts.
            let link =
                entry.file_type().is_ok_and(|t| t.is_symlink()) || matches!(mighty_core::workspace_files::link_target(&path), Ok(Some(_)));
            if meta.is_dir() && !link && depth < MAXIMUM_DEPTH {
                stack.push((path.clone(), depth + 1));
            }
            seen.insert(path, (meta.is_dir(), meta.len(), meta.modified().ok()));
        }
    }
    Some(seen)
}

fn differences(root: &Path, before: &Option<Snapshot>, after: &Option<Snapshot>, out: &mut BTreeSet<PathBuf>) {
    match (before, after) {
        (None, None) => {}
        // The folder appeared, or was taken away.
        (None, Some(_)) | (Some(_), None) => {
            out.insert(root.to_path_buf());
            for snapshot in [before, after].into_iter().flatten() {
                out.extend(snapshot.keys().cloned());
            }
        }
        (Some(a), Some(b)) => {
            for (path, state) in b {
                if a.get(path) != Some(state) {
                    out.insert(path.clone());
                }
            }
            out.extend(a.keys().filter(|p| !b.contains_key(*p)).cloned());
        }
    }
}

/// C# `StyleChangeWatcher(root, changed, latency)`; stops when dropped.
pub struct StyleChangeWatcher {
    latency: Duration,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for StyleChangeWatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StyleChangeWatcher").field("latency", &self.latency).finish_non_exhaustive()
    }
}

impl StyleChangeWatcher {
    pub fn new(root: &Path, changed: impl Fn(Vec<PathBuf>) + Send + 'static, latency: Option<Duration>) -> Self {
        let latency = latency.unwrap_or(Duration::from_millis(500));
        let stop = Arc::new(AtomicBool::new(false));
        let root = mighty_core::workspace_files::full_path(root).unwrap_or_else(|| root.to_path_buf());
        let poll = (latency / 4).clamp(Duration::from_millis(10), Duration::from_millis(500));
        let flag = stop.clone();
        let thread = std::thread::Builder::new()
            .name("style-watcher".into())
            .spawn(move || {
                let mut last = look(&root);
                let mut pending: BTreeSet<PathBuf> = BTreeSet::new();
                let mut since: Option<Instant> = None;
                while !flag.load(Ordering::Acquire) {
                    std::thread::sleep(poll);
                    if flag.load(Ordering::Acquire) {
                        return;
                    }
                    let now = look(&root);
                    let before = pending.len();
                    differences(&root, &last, &now, &mut pending);
                    last = now;
                    if pending.len() > before && since.is_none() {
                        since = Some(Instant::now());
                    }
                    if since.is_some_and(|s| s.elapsed() >= latency) {
                        let report = if pending.len() > MAXIMUM_PATHS { Vec::new() } else { pending.iter().cloned().collect() };
                        pending.clear();
                        since = None;
                        changed(report);
                    }
                }
            })
            .ok();
        StyleChangeWatcher { latency, stop, thread }
    }

    /// How long changes are gathered before they are reported.
    pub fn latency(&self) -> Duration {
        self.latency
    }
}

impl Drop for StyleChangeWatcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take()
            && thread.thread().id() != std::thread::current().id()
        {
            let _ = thread.join();
        }
    }
}

/// C# `Concerns(changed, workspace, manifest)`: the workspace's own style files, or a file source of the pane's style; an empty list
/// is the watcher's "read everything again".
pub fn concerns(changed: &[PathBuf], workspace: &Path, manifest: Option<&StyleManifest>) -> bool {
    if changed.is_empty() {
        return true;
    }
    let styles = workspace.join(".claude").join("mighty-styles");
    changed.iter().any(|path| touches(path, &styles) || manifest.is_some_and(|m| crate::state::affects(m, workspace, path)))
}

/// C# `Touches(changed, folder)`: one path is the other, or lies under it (case-blind, as C#).
pub fn touches(changed: &Path, folder: &Path) -> bool {
    fn normal(path: &Path) -> String {
        let full = mighty_core::workspace_files::full_path(path).unwrap_or_else(|| path.to_path_buf());
        let text = full.to_string_lossy().into_owned();
        let trimmed = text.trim_end_matches(['/', '\\']);
        if trimmed.is_empty() || trimmed.ends_with(':') { text } else { trimmed.to_string() }
    }
    fn under(path: &str, parent: &str) -> bool {
        path.len() > parent.len()
            && path.is_char_boundary(parent.len())
            && path[..parent.len()].to_uppercase() == parent.to_uppercase()
            && matches!(path.as_bytes()[parent.len()], b'\\' | b'/')
    }
    let changed = normal(changed);
    let folder = normal(folder);
    changed.to_uppercase() == folder.to_uppercase() || under(&changed, &folder) || under(&folder, &changed)
}
