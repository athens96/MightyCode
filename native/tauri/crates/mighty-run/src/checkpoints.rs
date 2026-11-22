//! Snapshots of a run's folder (checkpoints): what the folder held as a request started and as each of its steps, turns and the run
//! itself ended, so the result can say which files each part of the work changed, whichever agent made them and however (an edit
//! tool, a shell command, a script).
//!
//! A snapshot is a git tree of the repository's tracked and unignored files, made without touching the repository: git reads the
//! repository's objects but writes its own (the files' new contents) into the app's store, `<data>/snapshots/<hash of the git
//! folder>/objects`, with an index of the store's own (`GIT_INDEX_FILE`; the first one a copy of the repository's, so only files that
//! changed are read again). Two snapshots' trees give the files that changed between them (`git diff-tree`), and a file's change is
//! read when it is opened ([`Snapshots::file_diff`]). A folder outside a git repository has no snapshots.
//!
//! The snapshots run one at a time on a task of their own, in the order they were asked for; a checkpoint goes to its run as a
//! `checkpoint` event ([`crate::apply`]). A store not used for [`KEEP_DAYS`] days is removed.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use mighty_core::graph::{CheckpointFile, MAXIMUM_CHECKPOINT_FILES, MightyGraphRun, RunCheckpoint};
use mighty_core::time;
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;

use crate::git::{Git, is_object_name};
use crate::model::RunEvent;

/// A store not used for this many days is removed.
pub const KEEP_DAYS: u64 = 14;
/// `git add -A` over a large tree the first time.
const ADD_TIMEOUT: f64 = 60.0;
const READ_TIMEOUT: f64 = 20.0;
/// The runs whose checkpoints the worker still holds (sent again with each later one, so one the state did not take yet is not lost).
const HELD_RUNS: usize = 32;

/// Where a run's folder is snapshotted, and what for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    pub session_id: String,
    /// The graph run the checkpoint goes to.
    pub run_id: String,
    /// The folder the run works in.
    pub folder: PathBuf,
    /// `start`, `step`, `turn` or `end`.
    pub kind: String,
    pub label: Option<String>,
    pub step_key: Option<String>,
    /// The newest tree the request's runs already hold, for a request whose earlier snapshots this launch did not take.
    pub base: Option<String>,
}

/// One repository as the snapshots see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repository {
    /// The working tree's top folder.
    pub top: PathBuf,
    /// The repository's own objects (read, never written).
    pub objects: PathBuf,
    /// The repository's index, copied as the store's first one.
    pub index: PathBuf,
    /// The store for this repository.
    pub store: PathBuf,
}

/// The snapshot store of one profile.
#[derive(Clone)]
pub struct Snapshots {
    root: PathBuf,
    git: Git,
}

impl Snapshots {
    /// `root`: `<data>/snapshots`.
    pub fn new(root: PathBuf, git: Git) -> Snapshots {
        Snapshots { root, git }
    }

    /// The repository `folder` is in; None outside one (or without git).
    pub async fn repository(&self, folder: &Path) -> Option<Repository> {
        let result = self
            .git
            .run(
                &["rev-parse", "--path-format=absolute", "--show-toplevel", "--git-common-dir", "--git-path", "index"],
                folder,
                READ_TIMEOUT,
            )
            .await
            .filter(|r| r.exit_code == 0)?;
        let mut lines = result.stdout.lines().map(str::trim).filter(|l| !l.is_empty());
        let (top, common, index) = (lines.next()?, lines.next()?, lines.next()?);
        let common = PathBuf::from(common);
        let hash = Sha256::digest(common.to_string_lossy().as_bytes());
        let name: String = hash.iter().take(16).map(|b| format!("{b:02x}")).collect();
        Some(Repository {
            top: PathBuf::from(top),
            objects: common.join("objects"),
            index: PathBuf::from(index),
            store: self.root.join(name),
        })
    }

    fn variables(repository: &Repository) -> Vec<(&'static str, String)> {
        vec![
            ("GIT_INDEX_FILE", repository.store.join("index").to_string_lossy().into_owned()),
            ("GIT_OBJECT_DIRECTORY", repository.store.join("objects").to_string_lossy().into_owned()),
            ("GIT_ALTERNATE_OBJECT_DIRECTORIES", repository.objects.to_string_lossy().into_owned()),
        ]
    }

    /// The tree of the folder's repository as it is now; None outside a repository or when git failed.
    pub async fn snapshot(&self, folder: &Path) -> Option<(Repository, String)> {
        let repository = self.repository(folder).await?;
        // Held to the end: the sweep leaves a store alone while one is being written (and one the sweep is removing is made anew).
        let _lock = lock_store(&repository.store).await?;
        let index = repository.store.join("index");
        // Snapshots run one at a time: a lock left behind is a crash's.
        let _ = std::fs::remove_file(repository.store.join("index.lock"));
        if !index.exists() && repository.index.is_file() {
            let _ = std::fs::copy(&repository.index, &index);
        }
        // Marks the store used, for the pruning.
        let _ = std::fs::write(repository.store.join("used"), time::now_o());
        let variables = Self::variables(&repository);
        let added = self.git.run_with(&["add", "-A", "--", "."], &repository.top, ADD_TIMEOUT, &variables).await;
        if added.is_none_or(|r| r.exit_code != 0) {
            // An index git could not take (another git's format, a damaged copy) is made anew next time.
            let _ = std::fs::remove_file(&index);
            return None;
        }
        let written = self.git.run_with(&["write-tree", "--missing-ok"], &repository.top, READ_TIMEOUT, &variables).await?;
        let tree = written.stdout.trim().to_string();
        (written.exit_code == 0 && is_object_name(&tree)).then_some((repository, tree))
    }

    /// The files that differ between two trees: at most [`MAXIMUM_CHECKPOINT_FILES`], the rest counted; None when git failed.
    pub async fn changes(&self, repository: &Repository, base: &str, tree: &str) -> Option<(Vec<CheckpointFile>, i64)> {
        let variables = Self::variables(repository);
        let read = |format: &'static str| {
            let variables = variables.clone();
            async move {
                self.git
                    .run_with(
                        &["diff-tree", "-r", "-M", "--no-commit-id", format, "-z", base, tree],
                        &repository.top,
                        READ_TIMEOUT,
                        &variables,
                    )
                    .await
                    .filter(|r| r.exit_code == 0)
                    .map(|r| r.stdout)
            }
        };
        let statuses = read("--name-status").await?;
        let counts = read("--numstat").await.unwrap_or_default();
        let mut files = parse_name_status(&statuses);
        let counted = parse_numstat(&counts);
        for file in &mut files {
            if let Some((added, removed)) = counted.get(&file.path) {
                file.added = *added;
                file.removed = *removed;
            }
        }
        let more = files.len().saturating_sub(MAXIMUM_CHECKPOINT_FILES) as i64;
        files.truncate(MAXIMUM_CHECKPOINT_FILES);
        Some((files, more))
    }

    /// One file's change between two trees, as a unified diff (`old_path` for a renamed file); None when git failed.
    pub async fn file_diff(&self, folder: &Path, base: &str, tree: &str, path: &str, old_path: Option<&str>) -> Option<String> {
        if !is_object_name(base) || !is_object_name(tree) || path.is_empty() {
            return None;
        }
        let repository = self.repository(folder).await?;
        if !repository.store.join("objects").is_dir() {
            return None;
        }
        let mut variables = Self::variables(&repository);
        // A path is a path, never a pathspec's magic.
        variables.push(("GIT_LITERAL_PATHSPECS", "1".to_string()));
        let mut arguments = vec!["diff", "--no-color", "--no-ext-diff", "--no-textconv", "-M", base, tree, "--"];
        if let Some(old) = old_path.filter(|old| !old.is_empty() && *old != path) {
            arguments.push(old);
        }
        arguments.push(path);
        let result = self.git.run_with(&arguments, &repository.top, READ_TIMEOUT, &variables).await?;
        (result.exit_code == 0).then_some(result.stdout)
    }

    /// Removes the stores not used for [`KEEP_DAYS`] days.
    pub fn prune(&self) {
        let _ = sweep_with_caps(&self.root, KEEP_DAYS, 0, 0, &HashSet::new());
    }

    /// The names of the stores of the repositories `folders` are in (what [`sweep_with_caps`] is asked to leave alone for the panes
    /// that are open).
    pub async fn store_names(&self, folders: &[PathBuf]) -> HashSet<String> {
        let mut names = HashSet::new();
        for folder in folders {
            if let Some(name) = self.repository(folder).await.and_then(|r| r.store.file_name().map(|n| n.to_string_lossy().into_owned())) {
                names.insert(name);
            }
        }
        names
    }
}

/// The file in a store that says a snapshot or the sweep is working in it.
const STORE_LOCK: &str = "gc.lock";
/// A lock this old is a crash's.
const STALE_LOCK: Duration = Duration::from_secs(10 * 60);
/// How long a snapshot waits for the sweep to finish with its store.
const LOCK_WAIT: Duration = Duration::from_secs(120);

/// A store's lock, released when dropped.
struct StoreLock(PathBuf);

impl Drop for StoreLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Takes the lock of `store` if nothing holds it: Ok(None) when something does, Err when the store's folder is not there.
fn try_lock_store(store: &Path) -> std::io::Result<Option<StoreLock>> {
    let path = store.join(STORE_LOCK);
    match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(_) => Ok(Some(StoreLock(path))),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let age = std::fs::metadata(&path).and_then(|m| m.modified()).ok().and_then(|at| SystemTime::now().duration_since(at).ok());
            if age.is_some_and(|age| age > STALE_LOCK) {
                let _ = std::fs::remove_file(&path);
            }
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

/// The lock of `store` (its folder made first), waiting while the sweep holds it; None when it could not be had.
async fn lock_store(store: &Path) -> Option<StoreLock> {
    let started = std::time::Instant::now();
    loop {
        std::fs::create_dir_all(store.join("objects")).ok()?;
        match try_lock_store(store) {
            Ok(Some(lock)) => return Some(lock),
            // The sweep removed the folder between the two: made again.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return None,
            Ok(None) => {}
        }
        if started.elapsed() > LOCK_WAIT {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

const ACTIVE_STORE_GRACE: Duration = Duration::from_secs(5 * 60);

fn dir_bytes(root: &Path) -> u64 {
    let Ok(meta) = std::fs::symlink_metadata(root) else { return 0 };
    if meta.file_type().is_symlink() {
        return 0;
    }
    if meta.is_file() {
        return meta.len();
    }
    if !meta.is_dir() {
        return 0;
    }
    let mut total = 0u64;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(read) = std::fs::read_dir(&dir) else { continue };
        for entry in read.flatten() {
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_symlink() {
                continue;
            }
            if ft.is_dir() {
                stack.push(entry.path());
            } else if ft.is_file()
                && let Ok(m) = entry.metadata()
            {
                total += m.len();
            }
        }
    }
    total
}

/// When a store was last in use: the newest of `used` (its folder's own time when it has none: the folder changes with every lock
/// taken in it, which is no use), `index` and `index.lock`.
fn store_latest(path: &Path, dir_mtime: SystemTime) -> SystemTime {
    let at = |name: &str| std::fs::metadata(path.join(name)).and_then(|m| m.modified()).ok();
    [at("used").or(Some(dir_mtime)), at("index"), at("index.lock")].into_iter().flatten().max().unwrap_or(dir_mtime)
}

/// Removes `path` (a store) unless a snapshot holds its lock or it was used within the grace period by the time the lock is ours
/// (a snapshot may have started since the sweep looked). True when removed.
fn remove_store(path: &Path, dir_mtime: SystemTime) -> bool {
    let Ok(Some(_lock)) = try_lock_store(path) else { return false };
    let recent = SystemTime::now().duration_since(store_latest(path, dir_mtime)).unwrap_or(Duration::ZERO) < ACTIVE_STORE_GRACE;
    !recent && std::fs::remove_dir_all(path).is_ok()
}

/// Sweeps git checkpoint snapshot stores under `base` (`<data>/snapshots`):
/// - Never touches any store modified within the last 5 minutes (or holding a fresh `index.lock`), nor one a snapshot holds the lock
///   of ([`Snapshots::snapshot`]), so active runs stay safe.
/// - Removes stores unused for more than `keep_days` days.
/// - Resets/removes idle stores exceeding `max_repo_bytes` (when `> 0`) and, oldest-`used` first, idle stores while the total bytes
///   across `base` exceed `max_total_bytes` (when `> 0`): the size caps never remove a store named in `protected` (the repositories
///   of panes that are open), which may stay over a cap.
///
/// Returns `(stores_swept, bytes_reclaimed)`.
pub fn sweep_with_caps(
    base: &Path,
    keep_days: u64,
    max_repo_bytes: u64,
    max_total_bytes: u64,
    protected: &HashSet<String>,
) -> (usize, u64) {
    let Ok(entries) = std::fs::read_dir(base) else { return (0, 0) };
    let now = SystemTime::now();
    let keep_limit = Duration::from_secs(keep_days.saturating_mul(24 * 3600));

    struct StoreEntry {
        path: PathBuf,
        dir_mtime: SystemTime,
        used: SystemTime,
        bytes: u64,
        protected: bool,
    }

    let mut remaining: Vec<StoreEntry> = Vec::new();
    let mut swept = 0usize;
    let mut reclaimed = 0u64;

    for entry in entries.flatten() {
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_symlink() || !ft.is_dir() {
            continue;
        }
        let path = entry.path();
        let named_protected = protected.contains(entry.file_name().to_string_lossy().as_ref());
        let dir_mtime = entry.metadata().and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
        let used_mtime = std::fs::metadata(path.join("used")).and_then(|m| m.modified()).unwrap_or(dir_mtime);

        let latest = store_latest(&path, dir_mtime);
        let recent = now.duration_since(latest).unwrap_or(Duration::ZERO) < ACTIVE_STORE_GRACE;
        let bytes = dir_bytes(&path);
        if recent {
            remaining.push(StoreEntry { path, dir_mtime, used: used_mtime, bytes, protected: true });
            continue;
        }

        let too_old = now.duration_since(used_mtime).unwrap_or(Duration::ZERO) > keep_limit;
        let too_large_repo = max_repo_bytes > 0 && bytes > max_repo_bytes && !named_protected;
        if (too_old || too_large_repo) && remove_store(&path, dir_mtime) {
            swept += 1;
            reclaimed += bytes;
        } else {
            remaining.push(StoreEntry { path, dir_mtime, used: used_mtime, bytes, protected: named_protected });
        }
    }

    if max_total_bytes > 0 {
        let mut total: u64 = remaining.iter().map(|s| s.bytes).sum();
        if total > max_total_bytes {
            remaining.sort_by_key(|s| s.used);
            for store in remaining {
                if total <= max_total_bytes {
                    break;
                }
                if store.protected {
                    continue;
                }
                if remove_store(&store.path, store.dir_mtime) {
                    total = total.saturating_sub(store.bytes);
                    swept += 1;
                    reclaimed += store.bytes;
                }
            }
        }
    }

    (swept, reclaimed)
}

/// The folder a pane's runs work in, for its snapshots: its own working folder (a worktree), else its workspace's; none for a remote
/// workspace or a mirrored pane.
pub fn folder_of(snapshot: &mighty_core::model::AppSnapshot, session_id: &str) -> Option<PathBuf> {
    let pane = snapshot.session(session_id)?;
    if pane.mirror.is_some() {
        return None;
    }
    let folder = match pane.working_folder.as_deref().filter(|f| !f.is_empty()) {
        Some(folder) => folder.to_string(),
        None => {
            let workspace = snapshot.workspace(pane.workspace_id())?;
            if workspace.remote.is_some() {
                return None;
            }
            workspace.path.clone().filter(|p| !p.is_empty())?
        }
    };
    let folder = PathBuf::from(folder);
    folder.is_absolute().then_some(folder)
}

/// `git diff-tree --name-status -z`: `M\0path\0`, `R100\0old\0new\0`.
pub fn parse_name_status(output: &str) -> Vec<CheckpointFile> {
    let mut parts = output.split('\0').filter(|p| !p.is_empty());
    let mut files = vec![];
    while let Some(status) = parts.next() {
        let letter = status.chars().next().unwrap_or('M');
        if matches!(letter, 'R' | 'C') {
            let (Some(old), Some(new)) = (parts.next(), parts.next()) else { break };
            let status = if letter == 'R' { "R" } else { "A" };
            files.push(CheckpointFile { path: new.into(), old_path: Some(old.into()), status: status.into(), ..CheckpointFile::default() });
        } else {
            let Some(path) = parts.next() else { break };
            let status = match letter {
                'A' => "A",
                'D' => "D",
                'T' => "T",
                _ => "M",
            };
            files.push(CheckpointFile { path: path.into(), status: status.into(), ..CheckpointFile::default() });
        }
    }
    files
}

/// `git diff-tree --numstat -z`: `added\tremoved\tpath\0`, a rename `added\tremoved\t\0old\0new\0`, a binary file `-\t-\t…`. By the
/// (new) path: the lines added and removed, None for a binary file.
pub fn parse_numstat(output: &str) -> HashMap<String, (Option<i64>, Option<i64>)> {
    let mut counted = HashMap::new();
    let mut parts = output.split('\0');
    while let Some(head) = parts.next() {
        if head.is_empty() {
            continue;
        }
        let mut fields = head.splitn(3, '\t');
        let (Some(added), Some(removed), Some(path)) = (fields.next(), fields.next(), fields.next()) else { continue };
        let path = if path.is_empty() {
            let _old = parts.next();
            match parts.next() {
                Some(new) => new.to_string(),
                None => break,
            }
        } else {
            path.to_string()
        };
        counted.insert(path, (added.parse().ok(), removed.parse().ok()));
    }
    counted
}

/// The request a run belongs to: a background notice run's id (`<run>:notice:<n>`) stands for the run it came from.
pub fn request_of(run_id: &str) -> &str {
    run_id.split_once(":notice:").map_or(run_id, |(request, _)| request)
}

/// The snapshots of a run's steps, turns and end that `after` shows newly done, compared with `before` (the pane's runs before an
/// event). A step of the main agent's activity (a tool call) is none: it would snapshot the folder at every call.
pub fn due(before: &[MightyGraphRun], after: &[MightyGraphRun]) -> Vec<(String, String, Option<String>, Option<String>)> {
    let mut due = vec![];
    for run in after {
        let previous = before.iter().find(|b| b.id == run.id);
        for step in run.steps.iter().flatten() {
            let key = step.key.as_deref().unwrap_or_default();
            if step.status.as_deref() != Some("completed") || key.starts_with("id:activity:") || key.is_empty() {
                continue;
            }
            let was_done = previous
                .and_then(|p| p.steps.as_ref())
                .and_then(|steps| steps.iter().find(|s| s.key == step.key))
                .is_some_and(|s| s.status.as_deref() == Some("completed"));
            if !was_done {
                due.push((run.id().to_string(), "step".to_string(), step.content.clone(), Some(key.to_string())));
            }
        }
        let ended = mighty_core::graph::terminal(run.status());
        let was_ended = previous.is_some_and(|p| mighty_core::graph::terminal(p.status()));
        if ended && !was_ended {
            let (kind, label) = if run.is_notice() {
                let task = run.notice_title.as_deref().and_then(|t| t.split_once(" · ").map(|(_, task)| task.trim().to_string()));
                ("turn", task)
            } else {
                ("end", None)
            };
            due.push((run.id().to_string(), kind.to_string(), label, None));
        }
    }
    due
}

/// The newest tree the runs of `request` hold (the request's own run, then its notice runs, in order).
pub fn newest_tree(runs: &[MightyGraphRun], request: &str) -> Option<String> {
    runs.iter().filter(|r| request_of(r.id()) == request).flat_map(|r| r.checkpoints()).next_back().map(|c| c.tree.clone())
}

/// Asks for snapshots; they are taken one at a time, in order, by [`Checkpoints::start`]'s task.
#[derive(Clone)]
pub struct Checkpoints {
    jobs: mpsc::UnboundedSender<Job>,
}

impl Checkpoints {
    /// Starts the worker on the current runtime: each checkpoint goes to `deliver` as a `checkpoint` event. The stores not used for
    /// [`KEEP_DAYS`] days are removed first.
    pub fn start(snapshots: Snapshots, deliver: Arc<dyn Fn(RunEvent) + Send + Sync>) -> Checkpoints {
        let (jobs, mut receiver) = mpsc::unbounded_channel::<Job>();
        tokio::spawn(async move {
            let pruned = snapshots.clone();
            let _ = tokio::task::spawn_blocking(move || pruned.prune()).await;
            let mut worker = Worker::default();
            while let Some(job) = receiver.recv().await {
                if let Some(event) = worker.take(&snapshots, job).await {
                    deliver(event);
                }
            }
        });
        Checkpoints { jobs }
    }

    pub fn ask(&self, job: Job) {
        let _ = self.jobs.send(job);
    }

    /// Sweeps snapshot stores under `base` enforcing age (`keep_days`), per-repo byte cap, and total byte cap (the caps spare the
    /// stores in `protected`).
    pub fn sweep_with_caps(
        base: &Path,
        keep_days: u64,
        max_repo_bytes: u64,
        max_total_bytes: u64,
        protected: &HashSet<String>,
    ) -> (usize, u64) {
        sweep_with_caps(base, keep_days, max_repo_bytes, max_total_bytes, protected)
    }
}

/// What the worker remembers between snapshots.
#[derive(Default)]
struct Worker {
    /// Each request's newest tree.
    trees: HashMap<String, String>,
    /// The checkpoints made for each run this launch, newest runs last.
    held: VecDeque<(String, Vec<RunCheckpoint>)>,
}

impl Worker {
    async fn take(&mut self, snapshots: &Snapshots, job: Job) -> Option<RunEvent> {
        let (repository, tree) = snapshots.snapshot(&job.folder).await?;
        let request = request_of(&job.run_id).to_string();
        let base = self.trees.get(&request).cloned().or(job.base.clone());
        self.trees.insert(request, tree.clone());
        let checkpoint = if job.kind == "start" {
            RunCheckpoint { kind: job.kind, at: Some(time::now_o()), tree, ..RunCheckpoint::default() }
        } else {
            let base = base.filter(|base| *base != tree)?;
            let (files, more) = snapshots.changes(&repository, &base, &tree).await?;
            if files.is_empty() {
                return None;
            }
            RunCheckpoint {
                kind: job.kind,
                at: Some(time::now_o()),
                label: job.label,
                step_key: job.step_key,
                tree,
                base: Some(base),
                files,
                more: (more > 0).then_some(more),
            }
        };
        let at = match self.held.iter().position(|(run, _)| *run == job.run_id) {
            Some(at) => at,
            None => {
                if self.held.len() >= HELD_RUNS {
                    self.held.pop_front();
                }
                self.held.push_back((job.run_id.clone(), vec![]));
                self.held.len() - 1
            }
        };
        self.held[at].1.push(checkpoint);
        let run = MightyGraphRun { id: Some(job.run_id), checkpoints: Some(self.held[at].1.clone()), ..MightyGraphRun::default() };
        Some(RunEvent { graph_run: Some(run), ..RunEvent::new(&job.session_id, "checkpoint") })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_status_and_numstat_read_renames_and_binaries() {
        let files = parse_name_status("M\0src/a.rs\0R087\0old.txt\0new.txt\0A\0b c.md\0D\0gone\0");
        let names: Vec<(&str, &str, Option<&str>)> =
            files.iter().map(|f| (f.status.as_str(), f.path.as_str(), f.old_path.as_deref())).collect();
        assert_eq!(names, vec![("M", "src/a.rs", None), ("R", "new.txt", Some("old.txt")), ("A", "b c.md", None), ("D", "gone", None)]);
        let counted = parse_numstat("3\t1\tsrc/a.rs\x002\t2\t\0old.txt\0new.txt\0-\t-\tlogo.png\0");
        assert_eq!(counted["src/a.rs"], (Some(3), Some(1)));
        assert_eq!(counted["new.txt"], (Some(2), Some(2)));
        assert_eq!(counted["logo.png"], (None, None));
    }

    #[test]
    fn a_notice_run_stands_for_its_request() {
        assert_eq!(request_of("abc"), "abc");
        assert_eq!(request_of("abc:notice:2"), "abc");
    }

    fn step(key: &str, status: &str) -> mighty_core::graph::FlowStep {
        mighty_core::graph::FlowStep {
            key: Some(key.into()),
            content: Some(format!("do {key}")),
            active_form: None,
            status: Some(status.into()),
            started_at: None,
            ended_at: None,
        }
    }

    #[test]
    fn steps_done_and_runs_ended_are_due_once_and_activity_never() {
        let run = |status: &str, steps: Vec<mighty_core::graph::FlowStep>| MightyGraphRun {
            id: Some("r".into()),
            status: Some(status.into()),
            steps: Some(steps),
            ..MightyGraphRun::default()
        };
        let before = vec![run("running", vec![step("id:1", "in_progress"), step("id:activity:x", "in_progress")])];
        let after = vec![run("running", vec![step("id:1", "completed"), step("id:activity:x", "completed")])];
        let due_now = due(&before, &after);
        assert_eq!(due_now, vec![("r".into(), "step".into(), Some("do id:1".into()), Some("id:1".into()))]);
        assert!(due(&after, &after).is_empty());
        let ended = vec![run("completed", vec![step("id:1", "completed")])];
        assert_eq!(due(&after, &ended), vec![("r".into(), "end".into(), None, None)]);
        let notice = MightyGraphRun {
            id: Some("r:notice:1".into()),
            status: Some("completed".into()),
            notice_run: Some(true),
            notice_title: Some("백그라운드 작업 완료 · build".into()),
            ..MightyGraphRun::default()
        };
        let running = MightyGraphRun { status: Some("running".into()), ..notice.clone() };
        assert_eq!(due(&[running], std::slice::from_ref(&notice)), vec![("r:notice:1".into(), "turn".into(), Some("build".into()), None)]);
    }

    #[test]
    fn sweep_with_caps_respects_grace_period_and_enforces_age_repo_and_total_caps() {
        let nanos = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_nanos();
        let base = std::env::temp_dir().join(format!("mighty-ckpt-sweep-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();

        let make_store = |name: &str, size: usize, age: Option<Duration>| -> PathBuf {
            let store = base.join(name);
            std::fs::create_dir_all(store.join("objects")).unwrap();
            let blob = store.join("objects/pack.bin");
            std::fs::write(&blob, vec![b'z'; size]).unwrap();
            let used = store.join("used");
            std::fs::write(&used, "2026-11-22T00:00:00Z").unwrap();
            if let Some(ago) = age {
                let target = SystemTime::now().checked_sub(ago).unwrap();
                for p in [&blob, &used] {
                    std::fs::File::options().write(true).open(p).unwrap().set_modified(target).unwrap();
                }
                if let Ok(dir_f) = std::fs::File::open(&store) {
                    let _ = dir_f.set_modified(target);
                }
            }
            store
        };

        // 1) Active store (< 5 min old) exceeding repo cap must NEVER be touched!
        let active = make_store("active-hot", 2_000, None);
        // 2) Aged store (> 14 days) -> swept.
        let aged = make_store("aged-repo", 200, Some(Duration::from_secs(20 * 86_400)));
        // 3) Oversize idle store (> max_repo_bytes = 1_000) -> swept.
        let huge = make_store("huge-repo", 1_500, Some(Duration::from_secs(3600)));
        // 4) Two normal idle stores (600 bytes each); with active (2_000), total = 3_200 > max_total_bytes (2_700),
        //    so the older one (`older-idle`) is evicted while `newer-idle` and `active` stay!
        let older_idle = make_store("older-idle", 600, Some(Duration::from_secs(7200)));
        let newer_idle = make_store("newer-idle", 600, Some(Duration::from_secs(1800)));

        let (swept, reclaimed) = Checkpoints::sweep_with_caps(&base, 14, 1_000, 2_700, &HashSet::new());
        assert_eq!(swept, 3, "aged + huge + older_idle swept");
        assert!(reclaimed >= 2_300);
        assert!(active.exists(), "active store modified within 5m is protected");
        assert!(newer_idle.exists(), "newest idle store within total cap stays");
        assert!(!aged.exists() && !huge.exists() && !older_idle.exists());

        std::fs::remove_dir_all(base).unwrap();
    }

    fn aged_store(base: &Path, name: &str, size: usize, age: Duration) -> PathBuf {
        let store = base.join(name);
        std::fs::create_dir_all(store.join("objects")).unwrap();
        let blob = store.join("objects/pack.bin");
        std::fs::write(&blob, vec![b'z'; size]).unwrap();
        let used = store.join("used");
        std::fs::write(&used, "2026-11-22T00:00:00Z").unwrap();
        let target = SystemTime::now().checked_sub(age).unwrap();
        for p in [&blob, &used] {
            std::fs::File::options().write(true).open(p).unwrap().set_modified(target).unwrap();
        }
        if let Ok(dir) = std::fs::File::open(&store) {
            let _ = dir.set_modified(target);
        }
        store
    }

    fn sweep_base(name: &str) -> PathBuf {
        let nanos = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_nanos();
        let base = std::env::temp_dir().join(format!("mighty-ckpt-{name}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        base
    }

    /// The size caps never remove the store of a repository a live pane works in, even when it is idle and over both caps.
    #[test]
    fn size_caps_spare_the_stores_of_open_panes_but_the_age_rule_still_applies() {
        let base = sweep_base("protected");
        let live = aged_store(&base, "live-repo", 5_000, Duration::from_secs(3600));
        let idle = aged_store(&base, "idle-repo", 900, Duration::from_secs(7200));
        let live_old = aged_store(&base, "live-old", 100, Duration::from_secs(20 * 86_400));
        let protected: HashSet<String> = ["live-repo".to_string(), "live-old".to_string()].into();

        let (swept, _) = sweep_with_caps(&base, 14, 1_000, 1_500, &protected);
        assert!(live.exists(), "an open pane's store survives the per-repo and total caps");
        assert!(!idle.exists(), "an idle store over the total cap is still removed");
        assert!(!live_old.exists(), "the age rule still removes a store unused for more than the keep days");
        assert_eq!(swept, 2);
        std::fs::remove_dir_all(base).unwrap();
    }

    /// A store a snapshot holds the lock of is never removed by the sweep; once the lock is gone it can be.
    #[test]
    fn a_locked_store_is_not_swept_and_a_snapshot_waits_for_the_sweep() {
        let base = sweep_base("locked");
        let store = aged_store(&base, "busy", 100, Duration::from_secs(20 * 86_400));
        let lock = try_lock_store(&store).unwrap().expect("the lock is free");
        assert!(try_lock_store(&store).unwrap().is_none(), "a second taker finds it held");
        assert_eq!(sweep_with_caps(&base, 14, 0, 0, &HashSet::new()).0, 0);
        assert!(store.exists(), "a store with a snapshot in it stays");
        drop(lock);
        assert!(try_lock_store(&store).unwrap().is_some(), "the lock is released on drop");
        assert_eq!(sweep_with_caps(&base, 14, 0, 0, &HashSet::new()).0, 1);
        assert!(!store.exists());

        // A snapshot after the sweep made the folder anew gets its lock.
        let runtime = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
        let again = runtime.block_on(lock_store(&store));
        assert!(again.is_some() && store.join("objects").is_dir());
        std::fs::remove_dir_all(base).unwrap();
    }
}
