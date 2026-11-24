//! MC-47: the machine a pane worktree lives on. A local workspace's worktrees are this computer's ([`Local`]: the app's git runner and
//! the file system calls `pane_worktree` always made, unchanged); an SSH workspace's are the SSH machine's ([`Remote`]: the same git
//! steps over the machine's ssh, mighty-core `ssh::worktree`'s scripts for the file system). `pane_worktree`'s rules (create, merge,
//! commit, undo, the discard and its blocks, keep on close, the list, the copies and shared folders) are written once against
//! [`Machine`], so every decision that can lose work reads the machine the work is on, never a copy here.
//!
//! A remote question that cannot be answered (ssh failing) is an `Err`, never a "no": a worktree the machine could not be asked about
//! is never taken for gone or for one without work.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use mighty_core::ssh::worktree::{self as scripts, CopyEntry, Exclude, posix};
use mighty_platform::{DiskSpace, ProcessHost};
use mighty_run::os;
use mighty_run::ssh::SshLaunch;
use mighty_run::ssh_git::SshFolder;

use crate::git::paths::{DelegationPaths, is_inside, same, standardized};
use crate::git::{BoxFuture, Git, GitRun, GitRunner};
use crate::mighty_yaml::CopyRule;
use crate::pane_worktree::{self, CopyResult, ShareResult};

/// A worktree's shared folders' entries in its repository's `info/exclude`: the worktree's top (as its marker names it) and the two
/// git folders that hold them, read before the worktree goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExcludePlace {
    pub top: String,
    pub git_dir: String,
    pub common_dir: String,
}

/// The machine a pane worktree's git and files are on (module docs).
pub trait Machine: GitRunner {
    /// A full path on the machine (`Path.IsPathFullyQualified` here; `/…` or under the home on an SSH machine).
    fn qualified(&self, path: &str) -> bool;
    /// `DelegationPaths.Standardized` with the machine's path rules.
    fn standardized(&self, path: &str) -> String;
    /// `Path.Combine` with the machine's separator.
    fn combine(&self, folder: &str, name: &str) -> String;
    /// `DelegationPaths.Canonical`.
    fn canonical(&self, path: &str) -> String;
    /// `DelegationPaths.CanonicalPlace`.
    fn canonical_place(&self, path: &str) -> String;
    /// `DelegationPaths.Same`.
    fn same(&self, first: &str, second: &str) -> bool;
    /// `DelegationPaths.IsInside`.
    fn is_inside(&self, path: &str, folder: &str) -> bool;
    /// The path as git names a worktree in `worktree list` (links followed).
    fn real_path(&self, path: &str) -> String;
    /// The folder holding `path` and its last name.
    fn split(&self, path: &str) -> Option<(String, String)>;

    /// `Directory.Exists` (a link to a folder too).
    fn is_folder<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Result<bool, String>>;
    /// `Directory.Exists || File.Exists`.
    fn exists<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Result<bool, String>>;
    /// The path itself is a link (or a junction).
    fn is_link<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Result<bool, String>>;
    /// Each of `relative` under `folder`: a plain file's length, 0 for a link, a folder or what cannot be read (`Size`).
    fn sizes<'a>(&'a self, folder: &'a str, relative: &'a [String]) -> BoxFuture<'a, Result<Vec<u64>, String>>;
    /// The folder and every one on the way made.
    fn make_folders<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Result<(), String>>;
    /// The free bytes of the volume holding `path`; None when unknown.
    fn free_bytes<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Option<u64>>;
    /// The file `relative` inside `root`: a plain file, no link, reached through no link out of `root`, at most `limit` bytes.
    fn read_inside<'a>(&'a self, root: &'a str, relative: &'a str, limit: u64) -> BoxFuture<'a, Option<Vec<u8>>>;
    /// The link at `path` itself removed (`ChildCleanup.Unlink`).
    fn unlink<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Result<(), String>>;
    /// The directory links inside `folder` removed, never followed; true when the folder itself is a link or one is still inside.
    fn clear_links<'a>(&'a self, folder: &'a str) -> BoxFuture<'a, Result<bool, String>>;
    /// What is at `path` removed with everything in it, a link itself and never followed.
    fn remove<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Result<(), String>>;
    /// `PaneWorktreeCopy.Copy` from `source` into `destination` on the machine.
    fn copy_paths<'a>(&'a self, paths: &'a [String], source: &'a str, destination: &'a str) -> BoxFuture<'a, CopyResult>;
    /// `pane_worktree::copy_mapped_rules` on the machine.
    fn copy_rules<'a>(&'a self, rules: &'a [CopyRule], source: &'a str, destination: &'a str) -> BoxFuture<'a, CopyResult>;
    /// `pane_worktree::share_directories` on the machine.
    fn share<'a>(&'a self, paths: &'a [String], source: &'a str, destination: &'a str) -> BoxFuture<'a, ShareResult>;
    /// Where `worktree`'s shared folders' exclude entries are, read before it goes.
    fn exclude_place<'a>(&'a self, worktree: &'a str) -> BoxFuture<'a, Option<ExcludePlace>>;
    /// Those entries taken out once the worktree went.
    fn remove_excludes<'a>(&'a self, place: &'a ExcludePlace) -> BoxFuture<'a, ()>;
    /// MC-52: a fork's new files (`relative`, as git names them in `source`) copied into `target`: plain files of at most `limit`
    /// bytes only, never over a file `target` has, never through a link, never outside `target`. The paths copied.
    fn carry_files<'a>(&'a self, relative: &'a [String], source: &'a str, target: &'a str, limit: u64) -> BoxFuture<'a, Vec<String>>;

    /// `Path.IsPathFullyQualified(folder) && !folder.Contains('\0') && Directory.Exists(folder)`: a folder git may be pointed at.
    fn usable_folder<'a>(&'a self, folder: &'a str) -> BoxFuture<'a, bool> {
        Box::pin(async move { self.qualified(folder) && !folder.contains('\0') && self.is_folder(folder).await.unwrap_or(false) })
    }
}

// ── this computer ───────────────────────────────────────────────────────────────────────────────────────────────────────

/// This computer: the app's git runner and the file system calls the worktree code always made (`std::fs`, [`DelegationPaths`]).
#[derive(Clone)]
pub struct Local {
    git: Git,
    paths: DelegationPaths,
    disk: Option<Arc<dyn DiskSpace>>,
}

impl Local {
    /// `disk` reads the free space a new worktree's check asks for (None: unknown, as an error is).
    pub fn new(git: Git, paths: DelegationPaths, disk: Option<Arc<dyn DiskSpace>>) -> Local {
        Local { git, paths, disk }
    }

    pub fn git(&self) -> &Git {
        &self.git
    }

    pub fn paths(&self) -> &DelegationPaths {
        &self.paths
    }
}

/// `f` on a blocking thread, as the app ran the copies and the clean-up's walks.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    tokio::task::spawn_blocking(f).await.ok()
}

impl GitRunner for Local {
    fn git_available(&self) -> bool {
        self.git.available()
    }

    fn git_run<'a>(&'a self, arguments: &'a [&'a str], directory: &'a str, timeout: f64, input: Option<&'a str>) -> BoxFuture<'a, GitRun> {
        self.git.git_run(arguments, directory, timeout, input)
    }
}

impl Machine for Local {
    fn qualified(&self, path: &str) -> bool {
        mighty_core::normalize::fully_qualified(path)
    }

    fn standardized(&self, path: &str) -> String {
        standardized(path)
    }

    fn combine(&self, folder: &str, name: &str) -> String {
        os::combine(folder, name)
    }

    fn canonical(&self, path: &str) -> String {
        self.paths.canonical(path)
    }

    fn canonical_place(&self, path: &str) -> String {
        self.paths.canonical_place(path)
    }

    fn same(&self, first: &str, second: &str) -> bool {
        same(first, second)
    }

    fn is_inside(&self, path: &str, folder: &str) -> bool {
        is_inside(path, folder)
    }

    fn real_path(&self, path: &str) -> String {
        pane_worktree::real_path(path, &self.paths)
    }

    fn split(&self, path: &str) -> Option<(String, String)> {
        let place = Path::new(path);
        let name = place.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        let parent = place.parent().filter(|parent| !parent.as_os_str().is_empty())?;
        Some((parent.to_string_lossy().into_owned(), name))
    }

    fn is_folder<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Result<bool, String>> {
        Box::pin(async move { Ok(Path::new(path).is_dir()) })
    }

    fn exists<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Result<bool, String>> {
        Box::pin(async move { Ok(Path::new(path).exists()) })
    }

    fn is_link<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Result<bool, String>> {
        Box::pin(async move { Ok(crate::git::paths::is_link(path)) })
    }

    fn sizes<'a>(&'a self, folder: &'a str, relative: &'a [String]) -> BoxFuture<'a, Result<Vec<u64>, String>> {
        Box::pin(async move { Ok(relative.iter().map(|path| pane_worktree::size(&Path::new(folder).join(path.as_str()))).collect()) })
    }

    fn make_folders<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move { std::fs::create_dir_all(path).map_err(|error| error.to_string()) })
    }

    fn free_bytes<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Option<u64>> {
        Box::pin(async move { self.disk.as_ref()?.available_bytes(Path::new(path)).ok() })
    }

    fn read_inside<'a>(&'a self, root: &'a str, relative: &'a str, limit: u64) -> BoxFuture<'a, Option<Vec<u8>>> {
        Box::pin(async move {
            let full = Path::new(root).join(relative);
            let meta = std::fs::symlink_metadata(&full).ok()?;
            if !meta.is_file() || meta.file_type().is_symlink() || meta.len() > limit {
                return None;
            }
            // Beyond C#: a folder on the way may be a link out of the worktree; the file read must still lie inside it.
            if !is_inside(&self.paths.canonical(&full.to_string_lossy()), &self.paths.canonical(root)) {
                return None;
            }
            std::fs::read(&full).ok()
        })
    }

    fn unlink<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Result<(), String>> {
        let link = path.to_string();
        Box::pin(async move {
            blocking(move || crate::git::cleanup::unlink_path(&link))
                .await
                .unwrap_or_else(|| Err(std::io::Error::other("stopped")))
                .map_err(|error| error.to_string())
        })
    }

    fn clear_links<'a>(&'a self, folder: &'a str) -> BoxFuture<'a, Result<bool, String>> {
        let folder = folder.to_string();
        Box::pin(async move {
            blocking(move || crate::git::cleanup::clear_links(&folder))
                .await
                .unwrap_or_else(|| Err(std::io::Error::other("stopped")))
                .map_err(|error| error.to_string())
        })
    }

    fn remove<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Result<(), String>> {
        let folder = path.to_string();
        Box::pin(async move {
            blocking(move || crate::git::cleanup::remove_path(&folder))
                .await
                .unwrap_or_else(|| Err(std::io::Error::other("stopped")))
                .map_err(|error| error.to_string())
        })
    }

    fn copy_paths<'a>(&'a self, paths: &'a [String], source: &'a str, destination: &'a str) -> BoxFuture<'a, CopyResult> {
        let (paths, source, destination, handles) = (paths.to_vec(), source.to_string(), destination.to_string(), self.paths.clone());
        Box::pin(async move { blocking(move || pane_worktree::copy(&paths, &source, &destination, &handles)).await.unwrap_or_default() })
    }

    fn copy_rules<'a>(&'a self, rules: &'a [CopyRule], source: &'a str, destination: &'a str) -> BoxFuture<'a, CopyResult> {
        let (rules, source, destination, handles) = (rules.to_vec(), source.to_string(), destination.to_string(), self.paths.clone());
        Box::pin(async move {
            blocking(move || pane_worktree::copy_mapped_rules(&rules, &source, &destination, &handles)).await.unwrap_or_default()
        })
    }

    fn share<'a>(&'a self, paths: &'a [String], source: &'a str, destination: &'a str) -> BoxFuture<'a, ShareResult> {
        Box::pin(pane_worktree::share_directories_in(paths, source, destination, &self.paths, &self.git))
    }

    fn exclude_place<'a>(&'a self, worktree: &'a str) -> BoxFuture<'a, Option<ExcludePlace>> {
        Box::pin(async move {
            let (top, git_dir, common_dir) = pane_worktree::resolve_worktree_git_dirs(Path::new(worktree))?;
            Some(ExcludePlace {
                top: self.paths.canonical(&top.to_string_lossy()),
                git_dir: git_dir.to_string_lossy().into_owned(),
                common_dir: common_dir.to_string_lossy().into_owned(),
            })
        })
    }

    fn remove_excludes<'a>(&'a self, place: &'a ExcludePlace) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let (git_dir, common_dir) = (PathBuf::from(&place.git_dir), PathBuf::from(&place.common_dir));
            pane_worktree::remove_exclude_entries(&common_dir, &place.top);
            if common_dir != git_dir {
                pane_worktree::remove_exclude_entries(&git_dir, &place.top);
            }
        })
    }

    fn carry_files<'a>(&'a self, relative: &'a [String], source: &'a str, target: &'a str, limit: u64) -> BoxFuture<'a, Vec<String>> {
        let (relative, source, target) = (relative.to_vec(), source.to_string(), target.to_string());
        Box::pin(
            async move { blocking(move || pane_worktree::carry_local_files(&relative, &source, &target, limit)).await.unwrap_or_default() },
        )
    }
}

// ── an SSH machine ──────────────────────────────────────────────────────────────────────────────────────────────────────

/// The longest a file system question takes.
const ASK: Duration = Duration::from_secs(30);
/// The longest a removal or a copy takes (a worktree with its dependencies, 50 MB of copies).
const WORK: Duration = Duration::from_secs(300);

/// An SSH machine (MC-47): git through mighty-run `ssh_git` with delegation's settings (a push alone may use a transport), the file
/// system through mighty-core `ssh::worktree`'s scripts. Its paths are POSIX; its home is its real path (`pwd -P`), so the worktree
/// root under it is named as git names it.
#[derive(Clone)]
pub struct Remote {
    folder: SshFolder,
    home: String,
    git: bool,
}

impl Remote {
    /// The machine of `launch`, asked once for its home and whether it has git. Err with ssh's words when it cannot be reached.
    pub async fn connect(processes: Arc<dyn ProcessHost>, launch: SshLaunch) -> Result<Remote, String> {
        let folder = SshFolder::new(processes, launch);
        let info = folder.info().await?;
        Ok(Remote { folder, home: info.home, git: info.git })
    }

    /// A machine already asked (`home`, whether it has git).
    pub fn known(processes: Arc<dyn ProcessHost>, launch: SshLaunch, home: String, git: bool) -> Remote {
        Remote { folder: SshFolder::new(processes, launch), home, git }
    }

    /// The machine's home (its real path).
    pub fn home(&self) -> &str {
        &self.home
    }

    /// The pane worktrees' root over there: `<home>/.mightycode/worktrees`, as here.
    pub fn root(&self) -> String {
        posix::combine(&posix::combine(&self.home, ".mightycode"), "worktrees")
    }

    pub fn folder(&self) -> &SshFolder {
        &self.folder
    }

    async fn ask(&self, script: Option<String>, limit: Duration) -> Result<(i32, String), String> {
        let script = script.ok_or_else(|| "the path cannot be named on the machine".to_string())?;
        let ended = self.folder.exec(&script, None, limit, 4 << 20).await?;
        Ok((ended.code, String::from_utf8_lossy(&ended.stdout).into_owned()))
    }

    async fn probe(&self, paths: &[String]) -> Result<Vec<scripts::Probe>, String> {
        let mut probes = Vec::with_capacity(paths.len());
        for chunk in paths.chunks(scripts::PROBE_CHUNK) {
            let (code, out) = self.ask(scripts::probe_script(chunk), ASK).await?;
            if code != 0 {
                return Err(format!("the machine answered {code}"));
            }
            probes.extend(scripts::parse_probe(&out, chunk.len()).ok_or("the machine's answer could not be read")?);
        }
        Ok(probes)
    }

    async fn probe_one(&self, path: &str) -> Result<scripts::Probe, String> {
        self.probe(&[path.to_string()]).await?.pop().ok_or_else(|| "no answer".into())
    }

    /// Runs `script` (a copy or a share); `labels[i]` names its `i`-th entry. Every entry is `failed` when the machine cannot be asked.
    async fn entries(&self, script: Option<String>, labels: &[String]) -> Vec<(String, Option<String>)> {
        match self.ask(script, WORK).await {
            Ok((0, out)) => scripts::parse_entries(&out)
                .into_iter()
                .filter_map(|(index, why)| labels.get(index).map(|label| (label.clone(), why)))
                .collect(),
            _ => labels.iter().map(|label| (label.clone(), Some("failed".to_string()))).collect(),
        }
    }
}

/// A skip reason as the result keeps it (`&'static str`, the locale's keys); an unknown word reads `failed`.
fn reason(why: &str) -> &'static str {
    match why {
        "missing" => "missing",
        "link" => "link",
        "outside" => "outside",
        "exists" => "exists",
        "limit" => "limit",
        "notDirectory" => "notDirectory",
        "notIgnored" => "notIgnored",
        _ => "failed",
    }
}

impl GitRunner for Remote {
    fn git_available(&self) -> bool {
        self.git
    }

    fn git_run<'a>(&'a self, arguments: &'a [&'a str], directory: &'a str, timeout: f64, input: Option<&'a str>) -> BoxFuture<'a, GitRun> {
        // A pull request's push is the one step that reaches another computer, as here.
        let kind = if arguments.first() == Some(&"push") { scripts::GitKind::Push } else { scripts::GitKind::Delegation };
        Box::pin(self.folder.git(kind, arguments, directory, timeout, input))
    }
}

impl Machine for Remote {
    fn qualified(&self, path: &str) -> bool {
        mighty_core::ssh::valid_remote_dir(path)
    }

    fn standardized(&self, path: &str) -> String {
        posix::standardized(path)
    }

    fn combine(&self, folder: &str, name: &str) -> String {
        posix::combine(folder, name)
    }

    fn canonical(&self, path: &str) -> String {
        posix::standardized(path)
    }

    fn canonical_place(&self, path: &str) -> String {
        posix::standardized(path)
    }

    fn same(&self, first: &str, second: &str) -> bool {
        first == second
    }

    fn is_inside(&self, path: &str, folder: &str) -> bool {
        posix::is_inside(path, folder)
    }

    fn real_path(&self, path: &str) -> String {
        posix::standardized(path)
    }

    fn split(&self, path: &str) -> Option<(String, String)> {
        let standard = posix::standardized(path);
        Some((posix::parent(&standard)?, posix::file_name(&standard)?.to_string()))
    }

    fn is_folder<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Result<bool, String>> {
        Box::pin(async move { Ok(self.probe_one(path).await?.folder) })
    }

    fn exists<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Result<bool, String>> {
        Box::pin(async move { Ok(self.probe_one(path).await?.exists) })
    }

    fn is_link<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Result<bool, String>> {
        Box::pin(async move { Ok(self.probe_one(path).await?.link) })
    }

    fn sizes<'a>(&'a self, folder: &'a str, relative: &'a [String]) -> BoxFuture<'a, Result<Vec<u64>, String>> {
        Box::pin(async move {
            let full: Vec<String> = relative.iter().map(|path| posix::combine(folder, path)).collect();
            Ok(self.probe(&full).await?.into_iter().map(|probe| probe.size.unwrap_or(0)).collect())
        })
    }

    fn make_folders<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            match self.ask(scripts::mkdir_script(path), ASK).await? {
                (0, _) => Ok(()),
                (code, _) => Err(format!("mkdir answered {code}")),
            }
        })
    }

    fn free_bytes<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Option<u64>> {
        Box::pin(async move {
            self.ask(scripts::free_script(path), ASK)
                .await
                .ok()
                .filter(|(code, _)| *code == 0)
                .and_then(|(_, out)| scripts::parse_free(&out))
        })
    }

    fn read_inside<'a>(&'a self, root: &'a str, relative: &'a str, limit: u64) -> BoxFuture<'a, Option<Vec<u8>>> {
        Box::pin(async move {
            let script = scripts::read_inside_script(root, relative, limit)?;
            let ended = self.folder.exec(&script, None, ASK, usize::try_from(limit).unwrap_or(usize::MAX).saturating_add(1)).await.ok()?;
            (ended.code == 0).then_some(ended.stdout)
        })
    }

    fn unlink<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            match self.ask(scripts::unlink_script(path), ASK).await? {
                (0, _) => Ok(()),
                (code, _) => Err(format!("the link could not be removed ({code})")),
            }
        })
    }

    fn clear_links<'a>(&'a self, folder: &'a str) -> BoxFuture<'a, Result<bool, String>> {
        Box::pin(async move {
            match self.ask(scripts::clear_links_script(folder), WORK).await? {
                (0, out) if out.trim() == "0" => Ok(false),
                (0, out) if out.trim() == "1" => Ok(true),
                (code, _) => Err(format!("a folder in the worktree could not be read ({code})")),
            }
        })
    }

    fn remove<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            match self.ask(scripts::remove_script(path), WORK).await? {
                (0, _) => Ok(()),
                (code, _) => Err(format!("the folder could not be removed ({code})")),
            }
        })
    }

    fn copy_paths<'a>(&'a self, paths: &'a [String], source: &'a str, destination: &'a str) -> BoxFuture<'a, CopyResult> {
        Box::pin(async move {
            let planned = mighty_core::normalize::worktree_copy_paths(paths.iter().map(|p| Some(p.as_str()))).unwrap_or_default();
            let entries: Vec<CopyEntry> = planned.iter().map(|p| CopyEntry { from: p.clone(), to: p.clone() }).collect();
            let mut result = CopyResult::default();
            for (label, why) in self.entries(scripts::copy_script(source, destination, &entries), &planned).await {
                result.record(&label, why.as_deref().map(reason));
            }
            result
        })
    }

    fn copy_rules<'a>(&'a self, rules: &'a [CopyRule], source: &'a str, destination: &'a str) -> BoxFuture<'a, CopyResult> {
        Box::pin(async move {
            let plan = pane_worktree::plan_rules(rules);
            let mut runs: Vec<CopyEntry> = vec![];
            let mut labels: Vec<String> = vec![];
            for step in &plan {
                if let Ok((from, to)) = &step.paths {
                    runs.push(CopyEntry { from: from.clone(), to: to.clone() });
                    labels.push(step.label.clone());
                }
            }
            let answers = self.entries(scripts::copy_script(source, destination, &runs), &labels).await;
            // In the rules' order, as the local copy meets them; the entry that hit the limit is the last one read.
            let mut result = CopyResult::default();
            let mut answers = answers.into_iter();
            for step in &plan {
                match &step.paths {
                    Err(why) => result.record(&step.label, Some(why)),
                    Ok(_) => {
                        let Some((label, why)) = answers.next() else { break };
                        let limit = why.as_deref() == Some("limit");
                        result.record(&label, why.as_deref().map(reason));
                        if limit {
                            break;
                        }
                    }
                }
            }
            result
        })
    }

    fn share<'a>(&'a self, paths: &'a [String], source: &'a str, destination: &'a str) -> BoxFuture<'a, ShareResult> {
        Box::pin(async move {
            let mut result = ShareResult::default();
            let plan = pane_worktree::plan_shares(paths);
            // As here: a destination that is no plain folder fails every entry named, before anything is read.
            let usable = self.probe_one(destination).await.is_ok_and(|probe| probe.folder && !probe.link);
            if !usable {
                for raw in paths {
                    let trimmed = raw.trim();
                    if !trimmed.is_empty() {
                        result.record(trimmed, Some("failed"));
                    }
                }
                return result;
            }
            // The worktree's top and the destination's place in it, as git names them.
            let read = crate::git::run(self, &["rev-parse", "--show-toplevel", "--show-prefix"], destination, 10.0).await;
            let (top, prefix) = match read.filter(|r| r.exit_code == 0) {
                Some(read) => {
                    let mut lines = read.stdout.split('\n');
                    (lines.next().unwrap_or("").trim().to_string(), lines.next().unwrap_or("").trim().to_string())
                }
                None => (String::new(), String::new()),
            };
            let mut names = vec![];
            let mut excludes = vec![];
            for (label, path) in &plan {
                if let Some(path) = path {
                    let relative = format!("{prefix}{path}");
                    let top = posix::standardized(&top);
                    excludes.push(Exclude {
                        marker: pane_worktree::exclude_marker(&top),
                        pattern: format!("/{}", pane_worktree::escape_exclude_glob(relative.trim_matches('/'))),
                    });
                    names.push(label.clone());
                }
            }
            let script = if top.is_empty() {
                None
            } else {
                let only: Vec<String> = plan.iter().filter_map(|(_, path)| path.clone()).collect();
                scripts::share_script(source, destination, &posix::standardized(&top), &only, &excludes)
            };
            let mut answers = self.entries(script, &names).await.into_iter();
            for (label, path) in &plan {
                match path {
                    None => result.record(label, Some("outside")),
                    Some(_) => match answers.next() {
                        Some((label, why)) => result.record(&label, why.as_deref().map(reason)),
                        None => result.record(label, Some("failed")),
                    },
                }
            }
            result
        })
    }

    fn exclude_place<'a>(&'a self, worktree: &'a str) -> BoxFuture<'a, Option<ExcludePlace>> {
        Box::pin(async move {
            let (code, out) = self.ask(scripts::admin_script(worktree), ASK).await.ok()?;
            if code != 0 {
                return None;
            }
            let (git_dir, common_dir, top) = scripts::parse_admin(&out)?;
            Some(ExcludePlace { top: posix::standardized(&top), git_dir, common_dir })
        })
    }

    fn remove_excludes<'a>(&'a self, place: &'a ExcludePlace) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let mut admins = vec![place.common_dir.clone()];
            if place.git_dir != place.common_dir {
                admins.push(place.git_dir.clone());
            }
            let _ = self.ask(scripts::exclude_script(&admins, &pane_worktree::exclude_marker(&place.top)), ASK).await;
        })
    }

    fn carry_files<'a>(&'a self, relative: &'a [String], source: &'a str, target: &'a str, limit: u64) -> BoxFuture<'a, Vec<String>> {
        Box::pin(async move {
            // The local rules: plain files under the limit, read first; then the copy script's (never over a file, never through a
            // link on the way, inside the source only). A machine that cannot be asked copies nothing.
            let fine: Vec<&String> = relative.iter().filter(|path| mighty_core::ssh::files::valid_relative(path)).collect();
            let full: Vec<String> = fine.iter().map(|path| posix::combine(source, path)).collect();
            let Ok(probes) = self.probe(&full).await else { return vec![] };
            let entries: Vec<CopyEntry> = fine
                .iter()
                .zip(&probes)
                .filter(|(_, probe)| probe.exists && !probe.link && !probe.folder && probe.size.is_some_and(|size| size <= limit))
                .map(|(path, _)| CopyEntry { from: path.to_string(), to: path.to_string() })
                .collect();
            if entries.is_empty() {
                return vec![];
            }
            let labels: Vec<String> = entries.iter().map(|entry| entry.from.clone()).collect();
            self.entries(scripts::copy_script(source, target, &entries), &labels)
                .await
                .into_iter()
                .filter(|(_, why)| why.is_none())
                .map(|(label, _)| label)
                .collect()
        })
    }
}
