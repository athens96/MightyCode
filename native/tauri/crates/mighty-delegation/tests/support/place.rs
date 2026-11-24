//! `DelegationPlace` (C# MightyCode.Core.Tests/DelegationGitVerification.cs:10-103): a temporary `Base/{repo, worktrees, profile}`,
//! the maker with 50 GB of fake free space (mighty-platform's `FixedDiskSpace`), and the fixture's git — every `GIT_*` removed,
//! `GIT_CONFIG_GLOBAL=<Base>/fixture.gitconfig` (empty), `GIT_CONFIG_NOSYSTEM=1`, `GIT_OPTIONAL_LOCKS=0`, `GIT_TERMINAL_PROMPT=0`,
//! author and committer `Fixture <fixture@example.invalid>`, 120 s per call; `repository(commits, folder, autocrlf)` (`git init -q -b
//! main`, commits adding `file-<i>.txt`), `child`, `reported_child`, `commit`, `snapshot`, `mighty_branches`, `disk`. Delegation's own
//! git ([`DelegationPlace::git`]) reads the same empty global config and no system config, so no check ever reads the user's. It never
//! touches `%USERPROFILE%\.mightycode` or a real repository. Owner: W7-B (published first: W7-A's and W7-D's git checks run in it).
//!
//! Git runs through the platform's process host: `WindowsProcessHost` on Windows, M-A's `MacProcessHost` on the Mac; a folder's final
//! name comes from the platform's file handles (M-G's `MacFileHandles` on the Mac, `/var` read as `/private/var`).
//!
//! A child here is [`PlaceChild`], what C#'s `ChildRecord.Make(id, "parent", …)` holds of it for the git layer; W7-A's checks make their
//! own `ChildRecord` from the [`ChildWorktree`] [`DelegationPlace::made`] returns.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mighty_delegation::git::paths::DelegationPaths;
use mighty_delegation::git::worktree::{ChildWorktree, ChildWorktreeOutcome, Maker, report_file_in};
use mighty_delegation::git::{ChildGit, Git, branch_name};
use mighty_delegation::model::ChildState;
use mighty_platform::fake::FixedDiskSpace;
use mighty_platform::{FileHandles, ProcessHost, SpawnRequest};

/// The platform's process host: Windows' or the Mac's (M-A's), `Unsupported` elsewhere.
pub fn processes() -> Arc<dyn ProcessHost> {
    #[cfg(windows)]
    {
        Arc::new(mighty_platform_windows::WindowsProcessHost::new())
    }
    #[cfg(target_os = "macos")]
    {
        mighty_platform_macos::process_host()
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        Arc::new(mighty_platform::unsupported::Unsupported)
    }
}

/// The platform's file handles, which [`DelegationPaths::canonical`] asks for a folder's final name.
pub fn handles() -> Arc<dyn FileHandles> {
    #[cfg(windows)]
    {
        Arc::new(mighty_platform_windows::files::WindowsFileHandles)
    }
    #[cfg(target_os = "macos")]
    {
        Arc::new(mighty_platform_macos::files::MacFileHandles)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        Arc::new(mighty_platform::unsupported::Unsupported)
    }
}

/// Delegation's path rules over the platform's handles.
pub fn paths() -> DelegationPaths {
    DelegationPaths::new(handles())
}

/// C# `Verification.Temp`: a new empty folder of this check's own, named in full (CI's temp is under `C:\Users\RUNNER~1`: the 8.3
/// name is spelled out, as git spells the worktrees it lists).
pub fn temp(name: &str) -> PathBuf {
    // The count keeps two checks apart when they read the same clock (tests run in parallel).
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let count = NEXT.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("mighty-delegation-{name}-{}-{nanos}-{count}", std::process::id()));
    std::fs::create_dir_all(&path).unwrap();
    PathBuf::from(paths().canonical(&path.to_string_lossy()))
}

/// C# `RemoveFolder`: the folder and everything in it, read-only files made writable first (git's objects are read-only on
/// Windows), links and junctions removed themselves, never followed. Best effort.
pub fn remove_folder(path: &Path) {
    fn remove(path: &Path) -> std::io::Result<()> {
        let meta = std::fs::symlink_metadata(path)?;
        if meta.permissions().readonly() {
            let mut permissions = meta.permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            permissions.set_readonly(false);
            std::fs::set_permissions(path, permissions)?;
        }
        if meta.file_type().is_symlink() {
            return std::fs::remove_dir(path).or_else(|_| std::fs::remove_file(path));
        }
        if !meta.is_dir() {
            return std::fs::remove_file(path);
        }
        for entry in std::fs::read_dir(path)? {
            let _ = remove(&entry?.path());
        }
        std::fs::remove_dir(path)
    }
    let _ = remove(path);
}

/// `Encoding.UTF8.GetBytes` (no BOM).
pub fn utf8(text: &str) -> Vec<u8> {
    text.as_bytes().to_vec()
}

/// A child as the git layer reads it (C# `ChildRecord.Make(id, "parent", worktree, parentBranch, baseCommit, mode, key, state:,
/// parentCheckout:)` and `RecordingReport(head)`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaceChild {
    pub id: String,
    pub parent_session_id: String,
    pub worktree_path: String,
    pub branch: String,
    pub parent_branch: String,
    pub base_commit: String,
    pub state: ChildState,
    pub report_head: Option<String>,
    pub parent_checkout: Option<String>,
}

impl PlaceChild {
    /// `ChildRecord.Make`: the branch is `mighty/<id>`.
    pub fn make(
        id: &str,
        worktree_path: &str,
        parent_branch: &str,
        base_commit: &str,
        state: ChildState,
        parent_checkout: Option<&str>,
    ) -> PlaceChild {
        PlaceChild {
            id: id.to_string(),
            parent_session_id: "parent".to_string(),
            worktree_path: worktree_path.to_string(),
            branch: branch_name(id),
            parent_branch: parent_branch.to_string(),
            base_commit: base_commit.to_string(),
            state,
            report_head: None,
            parent_checkout: parent_checkout.map(str::to_string),
        }
    }

    /// The git layer's view.
    pub fn view(&self) -> ChildGit<'_> {
        ChildGit {
            id: &self.id,
            parent_session_id: &self.parent_session_id,
            worktree_path: &self.worktree_path,
            branch: &self.branch,
            parent_branch: &self.parent_branch,
            base_commit: &self.base_commit,
            state: self.state,
            report_head: self.report_head.as_deref(),
            parent_checkout: self.parent_checkout.as_deref(),
        }
    }

    /// `RecordingReport(head)` as far as git reads it: reported at `head`.
    pub fn reported(&self, head: &str) -> PlaceChild {
        PlaceChild { state: ChildState::Reported, report_head: Some(head.to_string()), ..self.clone() }
    }

    /// The child in another state, its report head kept.
    pub fn with_state(&self, state: ChildState) -> PlaceChild {
        PlaceChild { state, ..self.clone() }
    }
}

/// A temp folder holding `repo` (the parent's checkout), `worktrees` (the children's root) and `profile` (the delegation file and the
/// hooks folder). Fixture git is kept away from the user's and the system's settings; the repository sets its own identity and line
/// endings, which both the fixture and delegation's own git read, so the two agree whatever the PC is set to. Removed when dropped.
pub struct DelegationPlace {
    base: PathBuf,
}

impl Default for DelegationPlace {
    fn default() -> Self {
        DelegationPlace::new()
    }
}

impl Drop for DelegationPlace {
    fn drop(&mut self) {
        remove_folder(&self.base);
    }
}

impl DelegationPlace {
    pub fn new() -> DelegationPlace {
        let base = temp("place");
        std::fs::write(base.join("fixture.gitconfig"), "").unwrap();
        DelegationPlace { base }
    }

    pub fn base(&self) -> String {
        self.base.to_string_lossy().into_owned()
    }

    pub fn repo(&self) -> String {
        self.join("repo")
    }

    pub fn root(&self) -> String {
        self.join("worktrees")
    }

    pub fn profile(&self) -> String {
        self.join("profile")
    }

    /// `<Base>\<name>`.
    pub fn join(&self, name: &str) -> String {
        self.base.join(name).to_string_lossy().into_owned()
    }

    /// The empty global config every git here reads.
    pub fn global_config(&self) -> PathBuf {
        self.base.join("fixture.gitconfig")
    }

    /// Delegation's own git: hooks in `<profile>\git-no-hooks`, the empty global config and no system config.
    pub fn git(&self) -> Git {
        Git::new(processes(), self.base.join("profile").join("git-no-hooks")).with_global_config(self.global_config())
    }

    pub fn paths(&self) -> DelegationPaths {
        paths()
    }

    /// MC-47: this computer as the pane worktree code reaches it (the fixture's git and paths, 50 GB free).
    pub fn machine(&self) -> mighty_delegation::machine::Local {
        mighty_delegation::machine::Local::new(self.git(), self.paths(), Some(Arc::new(FixedDiskSpace(50_000_000_000))))
    }

    /// `Maker`: under `<Base>\worktrees`, with 50 GB free.
    pub fn maker(&self) -> Maker {
        Maker::new(&self.root(), self.git(), Arc::new(FixedDiskSpace(50_000_000_000)))
    }

    /// C# `Git(arguments)`: a fixture git in the repository that must succeed; its output trimmed, CRLF read as LF.
    pub async fn fixture_git(&self, arguments: &[&str]) -> String {
        self.git_in(&self.repo(), arguments).await
    }

    /// C# `GitIn(folder, arguments)`.
    pub async fn git_in(&self, folder: &str, arguments: &[&str]) -> String {
        let (exit, stdout, stderr) = self.try_git_in(folder, arguments).await;
        assert!(exit == 0, "git {}: {stderr}", arguments.join(" "));
        stdout
    }

    /// A fixture git that may fail: its exit code, output (trimmed, CRLF read as LF) and error text.
    pub async fn try_git_in(&self, folder: &str, arguments: &[&str]) -> (i32, String, String) {
        let program = mighty_run::git::executable().expect("git is not on the PATH");
        let mut args = vec!["-C".to_string(), folder.to_string()];
        args.extend(arguments.iter().map(|a| a.to_string()));
        let mut request = SpawnRequest::new(program, args, folder);
        for (key, _) in std::env::vars_os() {
            let key = key.to_string_lossy().into_owned();
            if key.to_ascii_uppercase().starts_with("GIT_") {
                request = request.without(key);
            }
        }
        request = request
            .env("GIT_CONFIG_GLOBAL", self.global_config().to_string_lossy())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            // Never above the place: a folder in it that is no repository never climbs into one around it.
            .env("GIT_CEILING_DIRECTORIES", self.base.parent().unwrap_or(&self.base).to_string_lossy())
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("GIT_TERMINAL_PROMPT", "0");
        for who in ["GIT_AUTHOR", "GIT_COMMITTER"] {
            request = request.env(format!("{who}_NAME"), "Fixture").env(format!("{who}_EMAIL"), "fixture@example.invalid");
        }
        // Generous: the first git launch on a cold CI runner can take seconds. Boxed: a check awaits many of these, and a debug
        // build's future keeps room for each.
        let processes = processes();
        let captured = Box::pin(mighty_platform::capture(processes.as_ref(), request, None, Duration::from_secs(120), 64 << 20))
            .await
            .unwrap_or_else(|e| panic!("git {}: {e}", arguments.join(" ")));
        let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).trim().replace("\r\n", "\n");
        (captured.exit_code, text(&captured.stdout), String::from_utf8_lossy(&captured.stderr).into_owned())
    }

    /// C# `Repository(commits, folder, autocrlf)`: `git init -b main` with `commits` commits, each adding one file in `folder`;
    /// returns the head ("" with no commit).
    pub async fn repository(&self, commits: usize, folder: &str, autocrlf: &str) -> String {
        std::fs::create_dir_all(self.repo()).unwrap();
        self.fixture_git(&["init", "-q", "-b", "main"]).await;
        for (key, value) in
            [("core.autocrlf", autocrlf), ("user.name", "Fixture"), ("user.email", "fixture@example.invalid"), ("commit.gpgsign", "false")]
        {
            self.fixture_git(&["config", key, value]).await;
        }
        let files = if folder.is_empty() { PathBuf::from(self.repo()) } else { Path::new(&self.repo()).join(folder) };
        std::fs::create_dir_all(&files).unwrap();
        for index in 0..commits {
            std::fs::write(files.join(format!("file-{index}.txt")), format!("{index}\n")).unwrap();
            self.fixture_git(&["add", "-A"]).await;
            self.fixture_git(&["commit", "-q", "-m", &format!("commit {index}")]).await;
        }
        if commits > 0 { self.fixture_git(&["rev-parse", "HEAD"]).await } else { String::new() }
    }

    /// `Repository()`: one commit, at the top, `core.autocrlf` false.
    pub async fn repository_default(&self) -> String {
        self.repository(1, "", "false").await
    }

    /// The worktree a child is made with, the app's way: its own `mighty/<id>` branch and worktree at the parent's head, TASK.md
    /// holding `Do the work.\n`.
    pub async fn made(&self, id: &str) -> ChildWorktree {
        match self.maker().create(id, &self.repo(), "Do the work.\n").await {
            ChildWorktreeOutcome::Created(worktree) => worktree,
            other => panic!("the child was not made: {id}: {other:?}"),
        }
    }

    /// C# `Child(id, parentCheckout)`: a running child made the app's way.
    pub async fn child(&self, id: &str, parent_checkout: Option<&str>) -> PlaceChild {
        let made = self.made(id).await;
        PlaceChild::make(id, &made.worktree_path, &made.parent_branch, &made.base_commit, ChildState::Running, parent_checkout)
    }

    /// C# `ReportedChild(id, files, parentCheckout)`: a child that committed one file per name (`<id>.txt` when none are given) and
    /// then reported at its head, its REPORT.md written.
    pub async fn reported_child(&self, id: &str, files: &[&str], parent_checkout: Option<&str>) -> (PlaceChild, String) {
        let child = self.child(id, parent_checkout).await;
        let mut head = child.base_commit.clone();
        let own = format!("{id}.txt");
        let names: Vec<&str> = if files.is_empty() { vec![own.as_str()] } else { files.to_vec() };
        for name in names {
            head = self.commit(&child.worktree_path, name, &format!("{id} {name}\n")).await;
        }
        std::fs::write(report_file_in(&child.worktree_path), format!("# Report of {id}\nDone.\n")).unwrap();
        (child.reported(&head), head)
    }

    /// C# `Commit(folder, name, text)`: writes `name` in `folder` and commits it; returns the new head.
    pub async fn commit(&self, folder: &str, name: &str, text: &str) -> String {
        let path = Path::new(folder).join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, utf8(text)).unwrap();
        self.git_in(folder, &["add", "--", name]).await;
        self.git_in(folder, &["commit", "-q", "-m", &format!("add {name}")]).await;
        self.git_in(folder, &["rev-parse", "HEAD"]).await
    }

    /// C# `Snapshot(checkouts)`: every ref, and each checkout's HEAD, index entries and file status: a refusal leaves all of it as it
    /// was.
    pub async fn snapshot(&self, checkouts: &[&str]) -> String {
        let mut parts = vec![self.fixture_git(&["for-each-ref", "--format=%(refname) %(objectname)"]).await];
        let repo = self.repo();
        for folder in std::iter::once(repo.as_str()).chain(checkouts.iter().copied()) {
            for arguments in [
                &["rev-parse", "--symbolic-full-name", "HEAD"][..],
                &["rev-parse", "HEAD"],
                &["ls-files", "-s"],
                &["status", "--porcelain", "--untracked-files=all"],
            ] {
                parts.push(self.git_in(folder, arguments).await);
            }
        }
        parts.join("\n--\n")
    }

    /// C# `Disk()`: every ref, every registered worktree, the parent's head and file status, and every file under the worktree root with
    /// what it holds (a folder ends with a separator; nothing is followed past a link).
    pub async fn disk(&self) -> String {
        let mut parts = vec![
            self.fixture_git(&["for-each-ref", "--format=%(refname) %(objectname)"]).await,
            self.fixture_git(&["worktree", "list", "--porcelain"]).await,
            self.fixture_git(&["rev-parse", "HEAD"]).await,
            self.fixture_git(&["status", "--porcelain", "--untracked-files=all"]).await,
        ];
        fn walk(folder: &Path, into: &mut Vec<String>) {
            let Ok(entries) = std::fs::read_dir(folder) else { return };
            for entry in entries.flatten() {
                let path = entry.path();
                let Ok(meta) = std::fs::symlink_metadata(&path) else { continue };
                if meta.is_dir() {
                    into.push(format!("{}{}", path.display(), std::path::MAIN_SEPARATOR));
                    walk(&path, into);
                } else if meta.file_type().is_symlink() {
                    into.push(format!("{} -> link", path.display()));
                } else {
                    let bytes = std::fs::read(&path).unwrap_or_default();
                    into.push(format!("{}: {}", path.display(), bytes.iter().map(|b| format!("{b:02X}")).collect::<String>()));
                }
            }
        }
        let mut files = vec![];
        walk(Path::new(&self.root()), &mut files);
        files.sort();
        parts.extend(files);
        parts.join("\n")
    }

    /// C# `WorktreeList()`: the registered worktrees, as delegation compares paths.
    pub async fn worktree_list(&self) -> Vec<String> {
        let paths = self.paths();
        self.fixture_git(&["worktree", "list", "--porcelain"])
            .await
            .split('\n')
            .filter_map(|line| line.strip_prefix("worktree "))
            .map(|path| paths.canonical(path))
            .collect()
    }

    /// C# `OnlyTheRepositoryIsListed()`.
    pub async fn only_the_repository_is_listed(&self) -> bool {
        let listed = self.worktree_list().await;
        listed.len() == 1 && mighty_delegation::git::paths::same(&listed[0], &self.paths().canonical(&self.repo()))
    }

    /// C# `MightyBranches()`: the refs under `refs/heads/mighty/`, one per line.
    pub async fn mighty_branches(&self) -> String {
        self.fixture_git(&["for-each-ref", "--format=%(refname)", "refs/heads/mighty/"]).await
    }
}

/// MC-47: the pane worktrees `maker` makes, on this computer (its root, the fixture's paths, 50 GB free).
pub fn panes(maker: &Maker) -> mighty_delegation::pane_worktree::Worktrees {
    mighty_delegation::pane_worktree::Worktrees::local(maker, &paths(), Arc::new(FixedDiskSpace(50_000_000_000)))
}
