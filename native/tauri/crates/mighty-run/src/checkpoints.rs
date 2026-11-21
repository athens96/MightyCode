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

use std::collections::{HashMap, VecDeque};
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
        std::fs::create_dir_all(repository.store.join("objects")).ok()?;
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
        let variables = Self::variables(&repository);
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
        let Ok(entries) = std::fs::read_dir(&self.root) else { return };
        let limit = Duration::from_secs(KEEP_DAYS * 24 * 3600);
        for entry in entries.flatten() {
            let path = entry.path();
            let used =
                std::fs::metadata(path.join("used")).and_then(|m| m.modified()).or_else(|_| entry.metadata().and_then(|m| m.modified()));
            let old = used.ok().and_then(|at| SystemTime::now().duration_since(at).ok()).is_some_and(|age| age > limit);
            if old && path.is_dir() {
                let _ = std::fs::remove_dir_all(&path);
            }
        }
    }
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
}
