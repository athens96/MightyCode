//! Per-pane git worktrees (C# `Core\PaneWorktree.cs`, from `main` 0.4.60–0.4.61; the Mac's `PaneWorktree*.swift` as the tie-breaker;
//! docs/devtool-features.md §12 "옮길 규칙"; plan §3 W9-I). Owner: W9-I.
//!
//! A workspace's agent pane that works in its own git worktree instead of the workspace folder, so two agents of one workspace work at
//! once without writing over each other's files. It has no parent: it is made with the same rules and in the same place as a delegated
//! child's worktree ([`make_pane`], branch `mighty/<session id>` at the workspace branch's head), and its run folder is the session's
//! `working_folder` like a child's. Merge, commit and merge, undo and discard are the same git as a child's card merge and discard
//! (W7's [`crate::git`]: `merge`, `cleanup::remove_own_worktree`), every command through mighty-run's runner. The saved records
//! ([`PaneWorktree`], [`KeptPaneWorktree`]) and their normalize rules are mighty-core's (`model.rs`, `normalize.rs`), re-exported here.
//! Writes are the host's to gate (`Host::may_touch_repo`, src-tauri's `pane_worktree.rs`).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use mighty_core::locale::Locale;
pub use mighty_core::model::{KeptPaneWorktree, PaneWorktree};
use mighty_core::model::{RunSession, Workspace};
use mighty_core::normalize::ordinal;
pub use mighty_core::normalize::{
    PANE_WORKTREE_ASK, PANE_WORKTREE_EVERY, PANE_WORKTREE_FURTHER, PANE_WORKTREE_HIGHEST_PORT, PANE_WORKTREE_LOWEST_PORT,
    WORKTREE_COPY_MAXIMUM_ENTRIES, kept_pane_worktrees, pane_worktree as normalized, pane_worktree_mode as mode_of, pane_worktree_port,
    saved_pane_worktree_mode as saved_mode, worktree_copy_paths as normalized_copy_paths, worktree_setup_command as normalized_command,
};
use mighty_core::shell::{ChildState, DelegationChildRow};
use mighty_run::os;

use crate::git::auto_commit::{MAXIMUM_NEW_FILE_BYTES, looks_secret};
use crate::git::cleanup::{ChildDiscardOutcome, remove_own_worktree};
use crate::git::merge::{checked_out_head, has_tracked_changes, head};
use crate::git::paths::{DelegationPaths, is_inside, same, standardized};
use crate::git::worktree::{ChildWorktree, ChildWorktreeBase, ChildWorktreeOutcome, Maker, is_safe_session_id};
use crate::git::{Git, branch_name, is_folder, is_object_name, line, message, run};

/// `PaneWorktreeMode.All`, in the menu's order.
pub const MODES: [&str; 3] = [PANE_WORKTREE_ASK, PANE_WORKTREE_FURTHER, PANE_WORKTREE_EVERY];

/// `PaneWorktree.From`: the record of `made`, a pane worktree in the checkout `checkout`.
pub fn from_made(made: &ChildWorktree, checkout: &str, port: i32, setup_command: Option<String>) -> PaneWorktree {
    PaneWorktree {
        worktree_path: Some(made.worktree_path.clone()),
        branch: Some(made.branch.clone()),
        base_branch: Some(made.parent_branch.clone()),
        base_commit: Some(made.base_commit.clone()),
        checkout: Some(checkout.to_string()),
        port,
        setup_status: setup_command.as_ref().map(|_| PaneWorktree::RUNNING.to_string()),
        setup_command,
        setup_exit_code: None,
        merged_commit: None,
        pre_merge_commit: None,
        working_folder: Some(made.working_folder.clone()),
        note_given: None,
        linked_task: None,
    }
}

/// `ChildWorktreeMaker.MakePane` (C# Core/DelegationWorktree.cs; M/DelegationWorktree.swift makePane): a workspace pane's own worktree,
/// the branch `mighty/<session id>` at the base commit in the same place as a child's, without a child's TASK.md and REPORT.md.
/// `timeout` in seconds (C#'s default is 300).
pub async fn make_pane(maker: &Maker, session_id: &str, start: &ChildWorktreeBase, timeout: f64) -> ChildWorktreeOutcome {
    use ChildWorktreeOutcome::Failed;
    if !is_safe_session_id(session_id) {
        return Failed("The session id cannot name a branch and a folder.".into());
    }
    let worktree = maker.worktree_path(session_id);
    let branch = branch_name(session_id);
    if let Err(error) = std::fs::create_dir_all(maker.root()) {
        return Failed(error.to_string());
    }
    let Some(added) =
        run(maker.git(), &["worktree", "add", "-b", &branch, &worktree, &start.base_commit], &start.repository, timeout).await
    else {
        return Failed("Git did not finish.".into());
    };
    if added.exit_code != 0 {
        return Failed(message(&added));
    }
    let folder = if start.prefix.is_empty() { worktree.clone() } else { standardized(&os::combine(&worktree, &start.prefix)) };
    ChildWorktreeOutcome::Created(ChildWorktree {
        session_id: session_id.to_string(),
        worktree_path: worktree,
        working_folder: folder,
        branch,
        parent_branch: start.parent_branch.clone(),
        base_commit: start.base_commit.clone(),
    })
}

// ── the decision (rule 1) ───────────────────────────────────────────────────────────────────────────────────────────────

/// `PaneWorktreeDecision`: where a new agent pane of a workspace works.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneWorktreeDecision {
    /// In the workspace folder: the first agent pane, a remote workspace, a resumed pane.
    SameFolder,
    /// In a new worktree, without asking: the workspace's mode says so.
    Worktree,
    /// The user chooses: another agent pane already works in the workspace folder.
    Ask,
}

/// `PaneWorktreePolicy.WorksInWorkspaceFolder`: whether `session` is an agent pane of `workspace_id` that works in the workspace folder
/// itself.
pub fn works_in_workspace_folder(session: &RunSession, workspace_id: &str) -> bool {
    session.workspace_id() == workspace_id
        && session.kind() == "claude"
        && session.parent_session_id.is_none()
        && session.working_folder.is_none()
}

/// `PaneWorktreePolicy.Decide`: where a new agent pane of `workspace` works. The rule is "open", not "running"; the panes in `closing`
/// do not count.
pub fn decide<'a>(
    workspace: &Workspace,
    sessions: impl IntoIterator<Item = &'a RunSession>,
    closing: Option<&HashSet<String>>,
) -> PaneWorktreeDecision {
    if workspace.remote.is_some() {
        return PaneWorktreeDecision::SameFolder;
    }
    let mode = mode_of(workspace.pane_worktree_mode.as_deref());
    if mode == PANE_WORKTREE_EVERY {
        return PaneWorktreeDecision::Worktree;
    }
    let taken = sessions
        .into_iter()
        .any(|s| !closing.is_some_and(|closing| closing.contains(s.id())) && works_in_workspace_folder(s, workspace.id()));
    if !taken {
        return PaneWorktreeDecision::SameFolder;
    }
    if mode == PANE_WORKTREE_FURTHER { PaneWorktreeDecision::Worktree } else { PaneWorktreeDecision::Ask }
}

/// `PaneWorktreePolicy.DecideWithoutAsking`: the same decision where nobody can be asked (a pane added from the phone): asking becomes
/// the workspace folder.
pub fn decide_without_asking<'a>(
    workspace: &Workspace,
    sessions: impl IntoIterator<Item = &'a RunSession>,
    closing: Option<&HashSet<String>>,
) -> PaneWorktreeDecision {
    match decide(workspace, sessions, closing) {
        PaneWorktreeDecision::Ask => PaneWorktreeDecision::SameFolder,
        decision => decision,
    }
}

/// `ChildStates.IsOpen` of a sidebar row's state: every state but failed, closed and discarded.
fn open_child(state: ChildState) -> bool {
    !matches!(state, ChildState::Failed | ChildState::Closed | ChildState::Discarded)
}

/// `PaneWorktreePolicy.DiscardBlocker`: why the pane may not be discarded now, None when it may: children still branch from its branch
/// (rule 9).
pub fn discard_blocker<'a>(
    session_id: &str,
    sessions: impl IntoIterator<Item = &'a RunSession>,
    children: &[DelegationChildRow],
    locale: &Locale,
) -> Option<String> {
    let panes = sessions.into_iter().filter(|s| s.parent_session_id.as_deref() == Some(session_id)).count();
    let rows = children.iter().filter(|c| c.parent_session_id == session_id && open_child(c.state)).count();
    let count = panes.max(rows);
    (count > 0).then(|| locale.get_with("paneWorktree.refused.hasChildren", &[("count", &count.to_string())]))
}

// ── the PORT (rule 4) ───────────────────────────────────────────────────────────────────────────────────────────────────

/// `PaneWorktreePorts.Port`: the pane's PORT: from its id (FNV-1a 32-bit over its UTF-8, the same start on every launch and on the
/// Mac), then the next one no other pane holds (`taken`) and `is_free` says nothing listens on; the start when none is.
pub fn port(session_id: &str, taken: &HashSet<i32>, is_free: impl Fn(i32) -> bool) -> i32 {
    let span = (PANE_WORKTREE_HIGHEST_PORT - PANE_WORKTREE_LOWEST_PORT + 1) as u32;
    let mut hash: u32 = 2_166_136_261;
    for byte in session_id.bytes() {
        hash = (hash ^ u32::from(byte)).wrapping_mul(16_777_619);
    }
    let start = hash % span;
    for step in 0..span {
        let port = PANE_WORKTREE_LOWEST_PORT + ((start + step) % span) as i32;
        if !taken.contains(&port) && is_free(port) {
            return port;
        }
    }
    PANE_WORKTREE_LOWEST_PORT + start as i32
}

/// `PaneWorktreePorts.CanBind`: whether a TCP server could listen on `port` on this computer's loopback now.
pub fn can_bind(port: i32) -> bool {
    u16::try_from(port).is_ok_and(|port| std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).is_ok())
}

// ── outcomes ────────────────────────────────────────────────────────────────────────────────────────────────────────────

/// `PaneWorktreeMergeOutcome`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeOutcome {
    /// The base branch is at `commit`; it was at `pre_merge` before.
    Merged {
        commit: String,
        pre_merge: String,
    },
    /// One of [`refusal`]'s codes; nothing changed.
    Refused(&'static str),
    Failed(String),
}

/// `PaneWorktreeUndoOutcome`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UndoOutcome {
    Undone,
    /// `notMerged`, `branchNotCheckedOut`, `baseMoved` or `trackedChanges` (rule 8).
    Refused(&'static str),
    Failed(String),
}

/// `PaneWorktreeCommitOutcome`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommitOutcome {
    /// Committed; `left_out` are new files not taken: they still block the merge.
    Committed {
        left_out: Vec<String>,
    },
    Nothing {
        left_out: Vec<String>,
    },
    Unmerged,
    Failed(String),
}

/// `PaneWorktreeStatus`: how far a pane worktree is from its base.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Status {
    pub ahead: i32,
    pub behind: i32,
    pub changed_files: i32,
}

/// `PaneWorktreeChangedFile`: one file a pane worktree changed against the point it shares with its base; `?` is new and not committed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedFile {
    pub path: String,
    pub status: String,
}

/// `PaneWorktreeOnDisk`: a pane worktree found on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OnDisk {
    pub session_id: String,
    pub worktree_path: String,
    pub branch: String,
    pub head: String,
}

/// `PaneWorktreeRefusal`: the refusal codes of a merge (rule 6), in the order they are checked.
pub mod refusal {
    use mighty_core::locale::Locale;

    pub const WORKTREE_MISSING: &str = "worktreeMissing";
    pub const UNCOMMITTED_CHANGES: &str = "uncommittedChanges";
    pub const NOTHING_TO_MERGE: &str = "nothingToMerge";
    pub const BRANCH_NOT_CHECKED_OUT: &str = "branchNotCheckedOut";
    pub const TRACKED_CHANGES: &str = "trackedChanges";
    pub const MERGE_CONFLICT: &str = "mergeConflict";

    /// `PaneWorktreeRefusal.Text`.
    pub fn text(reason: &str, base_branch: &str, locale: &Locale) -> String {
        locale.get_with(&format!("paneWorktree.refused.{reason}"), &[("base", base_branch)])
    }
}

// ── git (rules 6–10) ────────────────────────────────────────────────────────────────────────────────────────────────────

/// `PaneWorktreeGit.Timeout`: the longest a merge's writing step may take.
const TIMEOUT: f64 = 60.0;
/// C#'s default for a step that names no timeout.
const STEP_TIMEOUT: f64 = 10.0;

/// `PaneWorktreeGit.Merge`: the pane's commits into the branch the workspace folder was on (rule 6). Every refusal comes before any git
/// write: the worktree gone, changes in it, nothing to merge, the base branch not checked out, tracked changes in the workspace folder,
/// a conflict. The base moves by fast-forward when it can, else by exactly one merge commit, through one `merge --ff-only`.
pub async fn merge(git: &Git, pane: &PaneWorktree, locale: &Locale) -> MergeOutcome {
    use MergeOutcome::{Failed, Refused};
    let unfinished = || Failed(locale.get("delegation.card.error.gitUnfinished"));
    if !git.available() {
        return Failed(locale.get("delegation.card.error.gitMissing"));
    }
    let (worktree, checkout) = (pane.worktree_path(), pane.checkout());
    if !is_folder(worktree) {
        return Refused(refusal::WORKTREE_MISSING);
    }
    match has_any_changes(git, worktree).await {
        None => return unfinished(),
        Some(true) => return Refused(refusal::UNCOMMITTED_CHANGES),
        Some(false) => {}
    }
    let Some(branch_head) = branch_head(git, pane).await else { return Refused(refusal::WORKTREE_MISSING) };
    let Some(pre_merge) = checked_out_head(git, pane.base_branch(), checkout).await else {
        return Refused(refusal::BRANCH_NOT_CHECKED_OUT);
    };
    let Some(contained) = run(git, &["merge-base", "--is-ancestor", &branch_head, &pre_merge], checkout, STEP_TIMEOUT).await else {
        return unfinished();
    };
    if contained.exit_code == 0 {
        return Refused(refusal::NOTHING_TO_MERGE);
    }
    match has_tracked_changes(git, checkout).await {
        None => return Failed(locale.get("delegation.card.error.parentChangesUnread")),
        Some(true) => return Refused(refusal::TRACKED_CHANGES),
        Some(false) => {}
    }
    let Some(preview) = run(git, &["merge-tree", "--write-tree", &pre_merge, &branch_head], checkout, TIMEOUT).await else {
        return unfinished();
    };
    if preview.exit_code == 1 {
        return Refused(refusal::MERGE_CONFLICT);
    }
    let tree = line(&preview.stdout).split('\n').next().unwrap_or("").trim().to_string();
    if preview.exit_code != 0 || !is_object_name(&tree) {
        return Failed(message(&preview));
    }
    let Some(ancestor) = run(git, &["merge-base", "--is-ancestor", &pre_merge, &branch_head], checkout, STEP_TIMEOUT).await else {
        return unfinished();
    };
    let target = match ancestor.exit_code {
        0 => branch_head,
        1 => {
            let text = format!("Merge branch '{}' into {}", pane.branch(), pane.base_branch());
            let Some(made) = run(git, &["commit-tree", &tree, "-p", &pre_merge, "-p", &branch_head, "-m", &text], checkout, TIMEOUT).await
            else {
                return unfinished();
            };
            let commit = line(&made.stdout).to_string();
            if made.exit_code != 0 || !is_object_name(&commit) {
                return Failed(message(&made));
            }
            commit
        }
        _ => return Failed(message(&ancestor)),
    };
    let Some(moved) = run(git, &["merge", "--ff-only", "--no-autostash", "--no-stat", "-q", &target], checkout, TIMEOUT).await else {
        return unfinished();
    };
    if moved.exit_code != 0 {
        return Failed(message(&moved));
    }
    if head(git, checkout).await.as_deref() != Some(target.as_str()) {
        return Failed(locale.get("delegation.card.error.parentNotAtMerge"));
    }
    MergeOutcome::Merged { commit: target, pre_merge }
}

/// `BranchHead`: the head of the pane's branch, read in the checkout.
async fn branch_head(git: &Git, pane: &PaneWorktree) -> Option<String> {
    let verify = format!("refs/heads/{}^{{commit}}", pane.branch());
    let found = run(git, &["rev-parse", "-q", "--verify", &verify], pane.checkout(), STEP_TIMEOUT).await.filter(|r| r.exit_code == 0)?;
    let sha = line(&found.stdout);
    is_object_name(sha).then(|| sha.to_string())
}

/// `PaneWorktreeGit.HasAnyChanges`: whether `folder` has changes git would show: tracked or new files, ignored ones aside. None when
/// git could not tell.
pub async fn has_any_changes(git: &Git, folder: &str) -> Option<bool> {
    run(git, &["status", "--porcelain", "--untracked-files=normal"], folder, STEP_TIMEOUT)
        .await
        .filter(|status| status.exit_code == 0)
        .map(|status| !status.stdout.is_empty())
}

/// `PaneWorktreeGit.HasUnmergedWork`: whether the pane holds work the base branch does not; None when git could not tell (counts as
/// work).
pub async fn has_unmerged_work(git: &Git, pane: &PaneWorktree) -> Option<bool> {
    let (worktree, checkout) = (pane.worktree_path(), pane.checkout());
    if !is_folder(worktree) {
        return Some(false);
    }
    let status = run(git, &["status", "--porcelain"], worktree, STEP_TIMEOUT).await.filter(|r| r.exit_code == 0)?;
    if !status.stdout.is_empty() {
        return Some(true);
    }
    let base = format!("refs/heads/{}", pane.base_branch());
    // Beyond C#: the worktree's own head too, so commits made on a detached HEAD (on no branch) count as work and are never discarded.
    if let Some(own) = head(git, worktree).await {
        let contained = run(git, &["merge-base", "--is-ancestor", &own, &base], checkout, STEP_TIMEOUT).await?;
        match contained.exit_code {
            0 => {}
            1 => return Some(true),
            _ => return None,
        }
    }
    let verify = format!("refs/heads/{}^{{commit}}", pane.branch());
    let found = run(git, &["rev-parse", "-q", "--verify", &verify], checkout, STEP_TIMEOUT).await?;
    if found.exit_code != 0 {
        return Some(false);
    }
    let contained = run(git, &["merge-base", "--is-ancestor", line(&found.stdout), &base], checkout, STEP_TIMEOUT).await?;
    match contained.exit_code {
        0 => Some(false),
        1 => Some(true),
        _ => None,
    }
}

/// C# `PaneWorktreeClosed`'s question: whether a closed pane's worktree is kept on its workspace rather than discarded: kept while
/// children branch from it (`children`), or while git says it holds unmerged work or cannot tell (rule 9).
pub async fn kept_on_close(git: &Git, pane: &PaneWorktree, children: bool) -> bool {
    children || has_unmerged_work(git, pane).await != Some(false)
}

/// Beyond C# (W7's place rules for a pane's own worktree): whether `pane`'s worktree is its own place `<root>\<id>` for the id its branch
/// `mighty/<id>` names, and not itself a link or junction. Git that writes in the worktree (a commit, a merge's reads of it) goes only
/// where this holds; the discard checks the same ([`remove_own_worktree`]).
pub fn in_own_place(paths: &DelegationPaths, pane: &PaneWorktree, root: &str) -> bool {
    let Some(id) = pane.branch().strip_prefix("mighty/").filter(|id| is_safe_session_id(id)) else { return false };
    let worktree = pane.worktree_path();
    !crate::git::paths::is_link(worktree) && same(&paths.canonical_place(worktree), &os::combine(&paths.canonical(root), id))
}

/// `PaneWorktreeGit.Discard`: the worktree, every worktree nested in it, a leftover folder at its place and its branch
/// ([`remove_own_worktree`]).
pub async fn discard(
    git: &Git,
    paths: &DelegationPaths,
    pane: &PaneWorktree,
    session_id: &str,
    root: &str,
    locale: &Locale,
) -> ChildDiscardOutcome {
    let shared_top = resolve_worktree_git_dirs(Path::new(pane.worktree_path()))
        .map(|(top, git_dir, common_dir)| (paths.canonical(&top.to_string_lossy()), git_dir, common_dir));
    let outcome = remove_own_worktree(git, paths, session_id, pane.worktree_path(), pane.branch(), pane.checkout(), root, locale).await;
    if matches!(outcome, ChildDiscardOutcome::Discarded)
        && let Some((top, git_dir, common_dir)) = shared_top
    {
        remove_exclude_entries(&common_dir, &top);
        if common_dir != git_dir {
            remove_exclude_entries(&git_dir, &top);
        }
    }
    outcome
}

/// `PaneWorktreeGit.Status`: commits ahead of the base, commits the base moved ahead, and changed files; None when git could not tell.
pub async fn status(git: &Git, pane: &PaneWorktree) -> Option<Status> {
    if !is_folder(pane.worktree_path()) {
        return None;
    }
    let range = format!("refs/heads/{}...refs/heads/{}", pane.base_branch(), pane.branch());
    let counts =
        run(git, &["rev-list", "--left-right", "--count", &range], pane.checkout(), STEP_TIMEOUT).await.filter(|r| r.exit_code == 0)?;
    let changed = run(git, &["status", "--porcelain", "--untracked-files=normal"], pane.worktree_path(), STEP_TIMEOUT)
        .await
        .filter(|r| r.exit_code == 0)?;
    let numbers: Vec<i32> =
        line(&counts.stdout).split(['\t', ' ']).filter(|n| !n.is_empty()).map(|n| n.parse::<i32>().unwrap_or(-1)).collect();
    if numbers.len() != 2 || numbers.iter().any(|n| *n < 0) {
        return None;
    }
    let changed_files = changed.stdout.split('\n').filter(|l| !l.is_empty()).count() as i32;
    Some(Status { ahead: numbers[1], behind: numbers[0], changed_files })
}

/// `MergeBase`: the commit the pane shares with its base.
async fn merge_base(git: &Git, pane: &PaneWorktree) -> Option<String> {
    let base = format!("refs/heads/{}", pane.base_branch());
    let found = run(git, &["merge-base", &base, "HEAD"], pane.worktree_path(), STEP_TIMEOUT).await.filter(|r| r.exit_code == 0)?;
    let commit = line(&found.stdout);
    is_object_name(commit).then(|| commit.to_string())
}

/// `PaneWorktreeGit.ChangedFiles`: the files the pane changed against the commit it shares with its base: its commits and what is not
/// committed yet, in ordinal order of their paths.
pub async fn changed_files(git: &Git, pane: &PaneWorktree) -> Option<Vec<ChangedFile>> {
    let since = merge_base(git, pane).await?;
    let worktree = pane.worktree_path();
    let diff = run(git, &["diff", "--name-status", "-z", "--no-renames", &since], worktree, 30.0).await.filter(|r| r.exit_code == 0)?;
    let others = run(git, &["ls-files", "-z", "--others", "--exclude-standard"], worktree, 30.0).await.filter(|r| r.exit_code == 0)?;
    let mut files = vec![];
    let fields: Vec<&str> = diff.stdout.split('\0').filter(|f| !f.is_empty()).collect();
    let mut i = 0;
    while i + 1 < fields.len() {
        let status: String = fields[i].encode_utf16().take(1).map(|unit| char::from_u32(u32::from(unit)).unwrap_or('?')).collect();
        files.push(ChangedFile { path: fields[i + 1].to_string(), status });
        i += 2;
    }
    for path in others.stdout.split('\0').filter(|p| !p.is_empty()) {
        if !files.iter().any(|f| f.path == path) {
            files.push(ChangedFile { path: path.to_string(), status: "?".into() });
        }
    }
    files.sort_by(|a, b| ordinal(&a.path, &b.path));
    Some(files)
}

/// The size of a worktree's work for the review queue: the files it changed against the commit it shares with its base (its commits,
/// what is not committed yet and its new files) and the lines added and removed in them (a binary file counts as a file, no lines; a
/// new file's lines are not counted).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DiffStat {
    pub files: usize,
    pub added: usize,
    pub removed: usize,
}

/// `git diff --numstat -z` read: `<added>\t<removed>\t<path>\0` per file (`-` for a binary file's counts); with `--no-renames` no entry
/// spans two paths. Paths listed twice count once.
pub fn parse_numstat(text: &str) -> (DiffStat, Vec<String>) {
    let mut stat = DiffStat::default();
    let mut paths: Vec<String> = vec![];
    for entry in text.split('\0').filter(|entry| !entry.trim().is_empty()) {
        let mut fields = entry.trim_start_matches('\n').splitn(3, '\t');
        let (Some(added), Some(removed), Some(path)) = (fields.next(), fields.next(), fields.next()) else { continue };
        if path.is_empty() || paths.iter().any(|p| p == path) {
            continue;
        }
        stat.added += added.parse::<usize>().unwrap_or(0);
        stat.removed += removed.parse::<usize>().unwrap_or(0);
        paths.push(path.to_string());
    }
    stat.files = paths.len();
    (stat, paths)
}

/// The commit a worktree's work is measured from: the one it shares with its base branch, else the commit it was made at (a base
/// branch that is gone, or a detached base).
async fn review_base(git: &Git, pane: &PaneWorktree) -> Option<String> {
    if !pane.base_branch().is_empty()
        && let Some(found) = merge_base(git, pane).await
    {
        return Some(found);
    }
    let made_at = pane.base_commit.as_deref().unwrap_or("");
    is_object_name(made_at).then(|| made_at.to_string())
}

/// [`DiffStat`] of the pane's worktree (two git reads: `diff --numstat` and the untracked files); None when the worktree is gone or
/// git fails.
pub async fn diff_stat(git: &Git, pane: &PaneWorktree) -> Option<DiffStat> {
    if !is_folder(pane.worktree_path()) {
        return None;
    }
    let since = review_base(git, pane).await?;
    let worktree = pane.worktree_path();
    let diff = run(git, &["diff", "--numstat", "-z", "--no-renames", &since], worktree, 30.0).await.filter(|r| r.exit_code == 0)?;
    let others = run(git, &["ls-files", "-z", "--others", "--exclude-standard"], worktree, 30.0).await.filter(|r| r.exit_code == 0)?;
    let (mut stat, paths) = parse_numstat(&diff.stdout);
    stat.files += others.stdout.split('\0').filter(|p| !p.is_empty() && !paths.iter().any(|known| known == p)).count();
    Some(stat)
}

/// `Path.IsPathRooted`.
fn rooted(path: &str) -> bool {
    if !os::windows() {
        return path.starts_with('/');
    }
    let bytes = path.as_bytes();
    path.starts_with(['\\', '/']) || bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic()
}

/// `File.ReadAllText`: the file's text, its byte-order mark taken off (UTF-8, or UTF-16 either way), bytes UTF-8 does not allow read
/// as U+FFFD.
fn read_text(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(rest).into_owned();
    }
    let utf16 = |rest: &[u8], big: bool| {
        let units: Vec<u16> = rest
            .chunks(2)
            .map(|pair| match pair {
                [a, b] if big => u16::from_be_bytes([*a, *b]),
                [a, b] => u16::from_le_bytes([*a, *b]),
                _ => 0xFFFD,
            })
            .collect();
        String::from_utf16_lossy(&units)
    };
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return utf16(rest, false);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return utf16(rest, true);
    }
    String::from_utf8_lossy(bytes).into_owned()
}

/// `PaneWorktreeGit.FileDiff`: the unified diff of `path` against the commit the pane shares with its base (a new file: all added);
/// None when git could not tell, or the path leaves the worktree.
pub async fn file_diff(git: &Git, paths: &DelegationPaths, pane: &PaneWorktree, path: &str) -> Option<String> {
    if path.is_empty() || path.contains('\0') || rooted(path) || path.split(['/', '\\']).any(|part| part == "..") {
        return None;
    }
    let since = merge_base(git, pane).await?;
    let worktree = pane.worktree_path();
    let tracked = run(git, &["--literal-pathspecs", "ls-files", "--error-unmatch", "--", path], worktree, STEP_TIMEOUT)
        .await
        .is_some_and(|r| r.exit_code == 0);
    let known = tracked
        || run(git, &["cat-file", "-e", &format!("{since}:{path}")], worktree, STEP_TIMEOUT).await.is_some_and(|r| r.exit_code == 0);
    if known {
        return run(git, &["--literal-pathspecs", "diff", "--no-color", &since, "--", path], worktree, 30.0)
            .await
            .filter(|r| r.exit_code == 0)
            .map(|shown| shown.stdout);
    }
    // Untracked: shown whole as added lines.
    let full = Path::new(worktree).join(path);
    let meta = std::fs::symlink_metadata(&full).ok()?;
    if !meta.is_file() || meta.file_type().is_symlink() || meta.len() > 1_000_000 {
        return None;
    }
    // Beyond C#: a folder on the way may be a link out of the worktree; the file read must still lie inside it.
    if !is_inside(&paths.canonical(&full.to_string_lossy()), &paths.canonical(worktree)) {
        return None;
    }
    let text = read_text(&std::fs::read(&full).ok()?).replace("\r\n", "\n");
    let lines: Vec<String> = text.split('\n').map(|line| format!("+{line}")).collect();
    Some(format!("+++ b/{path}\n{}", lines.join("\n")))
}

/// `Size`: a plain file's length, 0 for a link, a folder or what cannot be read.
fn size(path: &Path) -> u64 {
    std::fs::symlink_metadata(path).ok().filter(|meta| meta.is_file() && !meta.file_type().is_symlink()).map_or(0, |meta| meta.len())
}

/// `PaneWorktreeGit.CommitAll`: commits everything the worktree changed, new files too, except secret-looking or over-5-MB new files
/// (rule 7), on the pane's branch, never signed, never through hooks.
pub async fn commit_all(git: &Git, pane: &PaneWorktree, locale: &Locale) -> CommitOutcome {
    use CommitOutcome::Failed;
    let folder = pane.worktree_path();
    if !is_folder(folder) {
        return Failed(locale.get("paneWorktree.refused.worktreeMissing"));
    }
    let unfinished = || Failed(locale.get("delegation.card.error.gitUnfinished"));
    let Some(unmerged) = run(git, &["ls-files", "-u"], folder, 60.0).await.filter(|r| r.exit_code == 0) else { return unfinished() };
    let Some(others) = run(git, &["ls-files", "-z", "--others", "--exclude-standard"], folder, 60.0).await.filter(|r| r.exit_code == 0)
    else {
        return unfinished();
    };
    if !unmerged.stdout.is_empty() {
        return CommitOutcome::Unmerged;
    }
    let fresh: Vec<String> = others.stdout.split('\0').filter(|p| !p.is_empty()).map(str::to_string).collect();
    let left_out: Vec<String> = fresh
        .iter()
        .filter(|path| looks_secret(path) || size(&Path::new(folder).join(path.as_str())) > MAXIMUM_NEW_FILE_BYTES)
        .cloned()
        .collect();
    let mut seen = HashSet::new();
    let taken: Vec<&str> = fresh.iter().filter(|path| !left_out.contains(path) && seen.insert(path.as_str())).map(String::as_str).collect();
    let write = |arguments: Vec<String>, input: Option<String>| async move {
        let arguments: Vec<&str> = arguments.iter().map(String::as_str).collect();
        match git.run_detailed(&arguments, Path::new(folder), 300.0, input.as_deref()).await.ok() {
            None => Some(locale.get("delegation.card.error.gitUnfinished")),
            Some(result) if result.exit_code == 0 => None,
            Some(result) => Some(message(&result)),
        }
    };
    let strings = |items: &[&str]| items.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    if let Some(failed) = write(strings(&["add", "-u"]), None).await {
        return Failed(failed);
    }
    if !taken.is_empty()
        && let Some(failed) =
            write(strings(&["--literal-pathspecs", "add", "--pathspec-from-file=-", "--pathspec-file-nul"]), Some(taken.join("\0"))).await
    {
        return Failed(failed);
    }
    let Some(staged) = run(git, &["diff", "--cached", "--quiet"], folder, 60.0).await.filter(|r| matches!(r.exit_code, 0 | 1)) else {
        return unfinished();
    };
    if staged.exit_code == 0 {
        return CommitOutcome::Nothing { left_out };
    }
    let text = format!("mighty: commit {} for its merge", pane.branch());
    if let Some(failed) = write(strings(&["-c", "commit.gpgsign=false", "commit", "-q", "--no-verify", "-m", &text]), None).await {
        return Failed(failed);
    }
    CommitOutcome::Committed { left_out }
}

/// `PaneWorktreeGit.UndoMerge`: puts the base branch back where it was before the pane's last merge (rule 8): `read-tree -m -u`, then
/// compare-and-swap; the files go back when the branch cannot.
pub async fn undo_merge(git: &Git, pane: &PaneWorktree, locale: &Locale) -> UndoOutcome {
    use UndoOutcome::{Failed, Refused};
    let merged = pane.merged_commit.as_deref().map(str::to_lowercase).filter(|m| is_object_name(m));
    let pre_merge = pane.pre_merge_commit.as_deref().map(str::to_lowercase).filter(|p| is_object_name(p));
    let (Some(merged), Some(pre_merge)) = (merged, pre_merge) else { return Refused("notMerged") };
    if !git.available() {
        return Failed(locale.get("delegation.card.error.gitMissing"));
    }
    let checkout = pane.checkout();
    let Some(current) = checked_out_head(git, pane.base_branch(), checkout).await else { return Refused("branchNotCheckedOut") };
    if current != merged {
        return Refused("baseMoved");
    }
    match has_tracked_changes(git, checkout).await {
        None => return Failed(locale.get("delegation.card.error.parentChangesUnread")),
        Some(true) => return Refused("trackedChanges"),
        Some(false) => {}
    }
    run(git, &["update-index", "-q", "--refresh"], checkout, 60.0).await;
    if !run(git, &["read-tree", "-m", "-u", &merged, &pre_merge], checkout, 60.0).await.is_some_and(|r| r.exit_code == 0) {
        return Failed(locale.get("delegation.card.error.filesNotRestored"));
    }
    let reason = format!("mighty: undo the merge of {}", pane.branch());
    let base = format!("refs/heads/{}", pane.base_branch());
    if !run(git, &["update-ref", "-m", &reason, &base, &pre_merge, &merged], checkout, STEP_TIMEOUT).await.is_some_and(|r| r.exit_code == 0)
    {
        run(git, &["read-tree", "-m", "-u", &pre_merge, &merged], checkout, 60.0).await;
        return Failed(locale.get("delegation.card.error.parentMovedDuringUndo"));
    }
    UndoOutcome::Undone
}

/// `PaneWorktreeGit.Worktrees`: the pane worktrees `checkout` has under `root`; delegated children's ids are left out by the caller.
pub async fn worktrees(git: &Git, paths: &DelegationPaths, checkout: &str, root: &str) -> Option<Vec<OnDisk>> {
    let listed = run(git, &["worktree", "list", "--porcelain"], checkout, 30.0).await.filter(|r| r.exit_code == 0)?;
    Some(parse(&listed.stdout, root, paths))
}

// ── the list (rule 10) ──────────────────────────────────────────────────────────────────────────────────────────────────

/// `PaneWorktreeInventory.Open`, `Kept`, `Found`: where a row of the list comes from.
pub const OPEN: &str = "open";
pub const KEPT: &str = "kept";
pub const FOUND: &str = "found";

/// `PaneWorktreeInventory.Row`.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub session_id: String,
    pub title: String,
    pub provider: String,
    pub worktree: PaneWorktree,
    pub origin: &'static str,
}

/// `RealPath`: the path with its links resolved, as git names a worktree: Windows' final path name ([`DelegationPaths::canonical`]),
/// elsewhere every linked folder on the way followed (a temp folder under `/var` is `/private/var` to git on a Mac).
fn real_path(path: &str, paths: &DelegationPaths) -> String {
    if os::windows() {
        return paths.canonical(path);
    }
    let full = standardized(path);
    let mut current = PathBuf::from("/");
    for part in full.split('/').filter(|part| !part.is_empty()) {
        let next = current.join(part);
        current = if std::fs::symlink_metadata(&next).is_ok_and(|meta| meta.file_type().is_symlink()) {
            std::fs::canonicalize(&next).unwrap_or(next)
        } else {
            next
        };
    }
    standardized(&current.to_string_lossy())
}

/// `PaneWorktreeInventory.Parse`: `git worktree list --porcelain` read for `<root>\<id>` folders on `mighty/<id>` branches.
pub fn parse(text: &str, root: &str, paths: &DelegationPaths) -> Vec<OnDisk> {
    let root_path = real_path(root, paths);
    let mut found = vec![];
    for block in text.replace("\r\n", "\n").split("\n\n") {
        let (mut path, mut head, mut branch) = (None, None, None);
        for line in block.split('\n') {
            if let Some(rest) = line.strip_prefix("worktree ") {
                path = Some(rest);
            } else if let Some(rest) = line.strip_prefix("HEAD ") {
                head = Some(rest);
            } else if let Some(rest) = line.strip_prefix("branch refs/heads/") {
                branch = Some(rest);
            }
        }
        let (Some(path), Some(head), Some(branch)) = (path, head, branch) else { continue };
        let folder = standardized(path);
        let place = Path::new(&folder);
        let id = place.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        let Some(parent) = place.parent().filter(|parent| !parent.as_os_str().is_empty()) else { continue };
        if !same(&real_path(&parent.to_string_lossy(), paths), &root_path)
            || !mighty_core::wire::identifier(&id)
            || branch != branch_name(&id)
        {
            continue;
        }
        found.push(OnDisk { session_id: id, worktree_path: folder.clone(), branch: branch.to_string(), head: head.to_string() });
    }
    found
}

/// `PaneWorktreeInventory.Rows`: open panes, then kept ones, then those found on disk (only with a base branch, none a delegated
/// child's in `excluded`), each session once.
pub fn rows<'a>(
    open: impl IntoIterator<Item = &'a RunSession>,
    kept: &[KeptPaneWorktree],
    on_disk: &[OnDisk],
    excluded: &HashSet<String>,
    checkout: &str,
    base_branch: Option<&str>,
    port: impl Fn(&str) -> i32,
) -> Vec<Row> {
    let mut rows: Vec<Row> = vec![];
    for session in open {
        if let Some(pane) = &session.pane_worktree {
            rows.push(Row {
                session_id: session.id().into(),
                title: session.title().into(),
                provider: session.provider().into(),
                worktree: pane.clone(),
                origin: OPEN,
            });
        }
    }
    for record in kept {
        if !rows.iter().any(|r| r.session_id == record.session_id()) {
            rows.push(Row {
                session_id: record.session_id().into(),
                title: record.title().into(),
                provider: record.provider().into(),
                worktree: record.worktree.clone().unwrap_or_default(),
                origin: KEPT,
            });
        }
    }
    if let Some(base_branch) = base_branch {
        for disk in on_disk {
            if excluded.contains(&disk.session_id) || rows.iter().any(|r| r.session_id == disk.session_id) {
                continue;
            }
            rows.push(Row {
                session_id: disk.session_id.clone(),
                title: disk.branch.clone(),
                provider: "claude".into(),
                worktree: PaneWorktree {
                    worktree_path: Some(disk.worktree_path.clone()),
                    branch: Some(disk.branch.clone()),
                    base_branch: Some(base_branch.to_string()),
                    base_commit: Some(disk.head.clone()),
                    checkout: Some(checkout.to_string()),
                    port: port(&disk.session_id),
                    working_folder: Some(disk.worktree_path.clone()),
                    ..PaneWorktree::default()
                },
                origin: FOUND,
            });
        }
    }
    rows
}

// ── the copied files (rule 5) ───────────────────────────────────────────────────────────────────────────────────────────

/// `PaneWorktreeCopy.MaximumBytes` and `MaximumFiles`.
pub const COPY_MAXIMUM_BYTES: u64 = 50_000_000;
pub const COPY_MAXIMUM_FILES: usize = 2_000;

/// `PaneWorktreeCopy.Entries`: the settings field (`.env, .env.local` or one per line) as entries.
pub fn copy_entries(text: &str) -> Vec<String> {
    text.split([',', '\n', '\r']).filter(|entry| !entry.is_empty()).map(str::to_string).collect()
}

/// `PaneWorktreeCopy.Result`: the entries copied, and those not copied with why: `missing`, `link`, `outside`, `exists`, `limit`,
/// `failed` (in the order they were met).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CopyResult {
    pub copied: Vec<String>,
    pub skipped: Vec<(String, &'static str)>,
}

impl CopyResult {
    fn skip(&mut self, path: &str, why: &'static str) {
        self.skipped.retain(|(entry, _)| entry != path);
        self.skipped.push((path.to_string(), why));
    }

    /// Why `path` was skipped, if it was.
    pub fn why(&self, path: &str) -> Option<&'static str> {
        self.skipped.iter().find(|(entry, _)| entry == path).map(|(_, why)| *why)
    }
}

/// `IsLink`: a symbolic link, a junction or any other reparse point.
fn is_link(meta: &std::fs::Metadata) -> bool {
    meta.file_type().is_symlink() || reparse_point(meta)
}

/// `FileAttributes.ReparsePoint` (Windows' junctions, cloud placeholders and the rest); a symbolic link elsewhere is all there is.
#[cfg(windows)]
fn reparse_point(meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt as _;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn reparse_point(_meta: &std::fs::Metadata) -> bool {
    false
}

/// `Path.Combine(folder, path)` with the entry's `/` the platform's separator.
fn under(folder: &str, path: &str) -> String {
    let separator = if os::windows() { "\\" } else { "/" };
    os::combine(folder, &path.replace('/', separator))
}

/// `PaneWorktreeCopy.Copy`: copies `paths` from `source` (the workspace folder) into `destination` (the pane's run folder): plain files
/// and folders inside the workspace folder only, never a link (a junction, symbolic link or any reparse point) and nothing one leads
/// to, never over a file the worktree has, at most 50 MB in 2,000 files.
pub fn copy(paths_to_copy: &[String], source: &str, destination: &str, paths: &DelegationPaths) -> CopyResult {
    let mut result = CopyResult::default();
    let source_root = paths.canonical(source);
    let destination_root = paths.canonical(destination);
    let (mut bytes, mut files) = (0u64, 0usize);
    for path in normalized_copy_paths(paths_to_copy.iter().map(|p| Some(p.as_str()))).unwrap_or_default() {
        let from = under(source, &path);
        let Ok(meta) = std::fs::symlink_metadata(&from) else {
            result.skip(&path, "missing");
            continue;
        };
        if is_link(&meta) {
            result.skip(&path, "link");
            continue;
        }
        // A folder on the way may be a link out of the workspace: the real place must still be inside it.
        if !is_inside(&paths.canonical(&from), &source_root) {
            result.skip(&path, "outside");
            continue;
        }
        let to = under(destination, &path);
        if Path::new(&to).exists() || std::fs::symlink_metadata(&to).is_ok() {
            result.skip(&path, "exists");
            continue;
        }
        let mut items: Vec<(PathBuf, PathBuf)> = vec![];
        let folder = meta.is_dir();
        let mut limit_hit = false;
        let walked: std::io::Result<()> = (|| {
            if folder {
                let mut pending = vec![PathBuf::from(&from)];
                'walk: while let Some(current) = pending.pop() {
                    for entry in std::fs::read_dir(&current)? {
                        let child = entry?.path();
                        let inner = std::fs::symlink_metadata(&child)?;
                        if is_link(&inner) {
                            continue;
                        }
                        if inner.is_dir() {
                            pending.push(child);
                            continue;
                        }
                        bytes = bytes.saturating_add(inner.len());
                        files = files.saturating_add(1);
                        if bytes > COPY_MAXIMUM_BYTES || files > COPY_MAXIMUM_FILES {
                            limit_hit = true;
                            break 'walk;
                        }
                        let relative = child.strip_prefix(&from).map(Path::to_path_buf).unwrap_or_default();
                        items.push((child, Path::new(&to).join(relative)));
                    }
                }
            } else {
                bytes = bytes.saturating_add(meta.len());
                files = files.saturating_add(1);
                if bytes > COPY_MAXIMUM_BYTES || files > COPY_MAXIMUM_FILES {
                    limit_hit = true;
                } else {
                    items.push((PathBuf::from(&from), PathBuf::from(&to)));
                }
            }
            Ok(())
        })();
        if limit_hit {
            result.skip(&path, "limit");
            break;
        }
        if walked.is_err() {
            result.skip(&path, "failed");
            continue;
        }
        let destination_path = Path::new(destination);
        let copied: std::io::Result<()> = (|| {
            for (item, target) in &items {
                if let Some(parent) = target.parent() {
                    ensure_safe_destination_dir(parent, destination_path, &destination_root, paths)?;
                }
                copy_file_create_new(item, target)?;
            }
            if folder && items.is_empty() {
                ensure_safe_destination_dir(Path::new(&to), destination_path, &destination_root, paths)?;
            }
            Ok(())
        })();
        match copied {
            Ok(()) => result.copied.push(path),
            Err(_) => result.skip(&path, "failed"),
        }
    }
    result
}

/// Validates every ancestor component from `destination` down to `dir` for symlinks/reparse points and containment inside
/// `destination_root` BEFORE creating each missing directory component.
fn ensure_safe_destination_dir(dir: &Path, destination: &Path, destination_root: &str, paths: &DelegationPaths) -> std::io::Result<()> {
    let relative = dir.strip_prefix(destination).map_err(|_| std::io::Error::other("destination target leaves worktree"))?;
    let mut cursor = destination.to_path_buf();
    let root_meta = std::fs::symlink_metadata(&cursor)?;
    if is_link(&root_meta) || !root_meta.is_dir() {
        return Err(std::io::Error::other("destination root is not a real directory"));
    }
    for component in relative.components() {
        match component {
            std::path::Component::Normal(part) => {
                cursor.push(part);
                match std::fs::symlink_metadata(&cursor) {
                    Ok(meta) => {
                        if is_link(&meta) || !meta.is_dir() {
                            return Err(std::io::Error::other("a folder on the way is a link or not a directory"));
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => match std::fs::create_dir(&cursor) {
                        Ok(()) => {}
                        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                            let meta = std::fs::symlink_metadata(&cursor)?;
                            if is_link(&meta) || !meta.is_dir() {
                                return Err(std::io::Error::other("a folder on the way is a link or not a directory"));
                            }
                        }
                        Err(err) => return Err(err),
                    },
                    Err(error) => return Err(error),
                }
                let place = paths.canonical(&cursor.to_string_lossy());
                if !same(&place, destination_root) && !is_inside(&place, destination_root) {
                    return Err(std::io::Error::other("a folder on the way leads out of the worktree"));
                }
            }
            std::path::Component::CurDir => {}
            _ => return Err(std::io::Error::other("invalid destination component")),
        }
    }
    let place = paths.canonical(&dir.to_string_lossy());
    if !same(&place, destination_root) && !is_inside(&place, destination_root) {
        return Err(std::io::Error::other("a folder on the way leads out of the worktree"));
    }
    Ok(())
}

/// Copies a regular non-link file `source` into a brand-new `target` using `OpenOptions::create_new(true)` (never following a
/// symlink or overwriting an existing file).
fn copy_file_create_new(source: &Path, target: &Path) -> std::io::Result<()> {
    use std::io::Write as _;
    let meta = std::fs::symlink_metadata(source)?;
    if is_link(&meta) || !meta.is_file() {
        return Err(std::io::Error::other("source is not a regular file"));
    }
    let mut reader = std::fs::File::open(source)?;
    let mut writer = std::fs::OpenOptions::new().write(true).create_new(true).open(target)?;
    if let Err(error) = std::io::copy(&mut reader, &mut writer).and_then(|_| writer.flush()) {
        drop(writer);
        let _ = std::fs::remove_file(target);
        return Err(error);
    }
    Ok(())
}

/// Copies mapped `{ from, to }` rules from `source` into `destination` with the exact same 50 MB / 2,000 files / no-symlink /
/// inside-root / `create_new(true)` safety invariants as [`copy`], never overwriting an existing file in the worktree.
pub fn copy_mapped_rules(rules: &[crate::mighty_yaml::CopyRule], source: &str, destination: &str, paths: &DelegationPaths) -> CopyResult {
    let mut result = CopyResult::default();
    let source_root = paths.canonical(source);
    let destination_root = paths.canonical(destination);
    let (mut bytes, mut files) = (0u64, 0usize);
    let mut seen_targets = HashSet::new();

    for rule in rules.iter().take(crate::mighty_yaml::MAXIMUM_WORKTREE_RULES) {
        let raw_label = rule.display_label();
        let Some(from_norm) = normalized_copy_paths([Some(rule.from.as_str())]).and_then(|mut v| v.pop()) else {
            result.skip(&raw_label, "outside");
            continue;
        };
        let Some(to_norm) = normalized_copy_paths([Some(rule.to.as_str())]).and_then(|mut v| v.pop()) else {
            result.skip(&raw_label, "outside");
            continue;
        };
        if !seen_targets.insert(to_norm.clone()) {
            continue;
        }
        let label = if from_norm == to_norm { from_norm.clone() } else { format!("{from_norm} -> {to_norm}") };
        let from = under(source, &from_norm);
        let Ok(meta) = std::fs::symlink_metadata(&from) else {
            result.skip(&label, "missing");
            continue;
        };
        if is_link(&meta) {
            result.skip(&label, "link");
            continue;
        }
        if !is_inside(&paths.canonical(&from), &source_root) {
            result.skip(&label, "outside");
            continue;
        }
        let to = under(destination, &to_norm);
        if Path::new(&to).exists() || std::fs::symlink_metadata(&to).is_ok() {
            result.skip(&label, "exists");
            continue;
        }

        let mut items: Vec<(PathBuf, PathBuf)> = vec![];
        let folder = meta.is_dir();
        let mut limit_hit = false;
        let walked: std::io::Result<()> = (|| {
            if folder {
                let mut pending = vec![PathBuf::from(&from)];
                'walk: while let Some(current) = pending.pop() {
                    for entry in std::fs::read_dir(&current)? {
                        let child = entry?.path();
                        let inner = std::fs::symlink_metadata(&child)?;
                        if is_link(&inner) {
                            continue;
                        }
                        if inner.is_dir() {
                            pending.push(child);
                            continue;
                        }
                        bytes = bytes.saturating_add(inner.len());
                        files = files.saturating_add(1);
                        if bytes > COPY_MAXIMUM_BYTES || files > COPY_MAXIMUM_FILES {
                            limit_hit = true;
                            break 'walk;
                        }
                        let relative = child.strip_prefix(&from).map(Path::to_path_buf).unwrap_or_default();
                        items.push((child, Path::new(&to).join(relative)));
                    }
                }
            } else {
                bytes = bytes.saturating_add(meta.len());
                files = files.saturating_add(1);
                if bytes > COPY_MAXIMUM_BYTES || files > COPY_MAXIMUM_FILES {
                    limit_hit = true;
                } else {
                    items.push((PathBuf::from(&from), PathBuf::from(&to)));
                }
            }
            Ok(())
        })();
        if limit_hit {
            result.skip(&label, "limit");
            break;
        }
        if walked.is_err() {
            result.skip(&label, "failed");
            continue;
        }
        let destination_path = Path::new(destination);
        let copied: std::io::Result<()> = (|| {
            for (item, target) in &items {
                if let Some(parent) = target.parent() {
                    ensure_safe_destination_dir(parent, destination_path, &destination_root, paths)?;
                }
                copy_file_create_new(item, target)?;
            }
            if folder && items.is_empty() {
                ensure_safe_destination_dir(Path::new(&to), destination_path, &destination_root, paths)?;
            }
            Ok(())
        })();
        match copied {
            Ok(()) => result.copied.push(label),
            Err(_) => result.skip(&label, "failed"),
        }
    }
    result
}

/// Outcome of sharing cache directories into a new worktree via symlink (Unix) or directory junction (Windows).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShareResult {
    pub shared: Vec<String>,
    pub skipped: Vec<(String, &'static str)>,
}

impl ShareResult {
    fn skip(&mut self, path: &str, why: &'static str) {
        self.skipped.retain(|(entry, _)| entry != path);
        self.skipped.push((path.to_string(), why));
    }

    /// Why `path` was skipped, if it was.
    pub fn why(&self, path: &str) -> Option<&'static str> {
        self.skipped.iter().find(|(entry, _)| entry == path).map(|(_, why)| *why)
    }
}

/// Shares relative cache directories (`node_modules`, `.next`, `target`, `.venv`, etc.) from `source` into `destination` via
/// Unix directory symlink or Windows directory junction (made without a shell), only for folders the source already ignores, and records
/// the shared relative path in `<git-common-dir>/info/exclude` (under the worktree's marker, removed again by [`discard`]) so `git status` never lists a symlinked directory when `.gitignore` uses trailing-slash rules.
pub async fn share_directories(
    paths_to_share: &[String],
    source: &str,
    destination: &str,
    paths: &DelegationPaths,
    git: &Git,
    processes: &dyn mighty_platform::ProcessHost,
) -> ShareResult {
    let mut result = ShareResult::default();
    let source_root = paths.canonical(source);
    let destination_root = paths.canonical(destination);
    let destination_path = Path::new(destination);
    if !std::fs::symlink_metadata(destination_path).is_ok_and(|meta| !is_link(&meta) && meta.is_dir()) {
        for raw in paths_to_share {
            let trimmed = raw.trim();
            if !trimmed.is_empty() {
                result.skip(trimmed, "failed");
            }
        }
        return result;
    }

    let mut seen = HashSet::new();
    for raw in paths_to_share.iter().take(crate::mighty_yaml::MAXIMUM_WORKTREE_RULES) {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Some(path) = normalized_copy_paths([Some(trimmed)]).and_then(|mut v| v.pop()) else {
            result.skip(trimmed, "outside");
            continue;
        };
        if !seen.insert(path.clone()) {
            continue;
        }
        let from = under(source, &path);
        let Ok(meta) = std::fs::symlink_metadata(&from) else {
            result.skip(&path, "missing");
            continue;
        };
        if is_link(&meta) {
            result.skip(&path, "link");
            continue;
        }
        if !meta.is_dir() {
            result.skip(&path, "notDirectory");
            continue;
        }
        let canonical_from = paths.canonical(&from);
        if !is_inside(&canonical_from, &source_root) {
            result.skip(&path, "outside");
            continue;
        }
        // Only a folder the source repository already ignores is shared: the `info/exclude` entry below is the repository's, not the
        // worktree's, and must not hide anything git would list there.
        let ignored = run(git, &["check-ignore", "-q", "--no-index", "--", &path], source, STEP_TIMEOUT).await;
        if !ignored.is_some_and(|r| r.exit_code == 0) {
            result.skip(&path, "notIgnored");
            continue;
        }

        let to = under(destination, &path);
        let to_path = Path::new(&to);
        if to_path.exists() || std::fs::symlink_metadata(to_path).is_ok() {
            result.skip(&path, "exists");
            continue;
        }
        let Some(parent) = to_path.parent() else {
            result.skip(&path, "failed");
            continue;
        };
        if ensure_safe_destination_dir(parent, destination_path, &destination_root, paths).is_err() {
            result.skip(&path, "outside");
            continue;
        }
        if std::fs::symlink_metadata(to_path).is_ok() {
            result.skip(&path, "exists");
            continue;
        }

        if create_directory_link(&canonical_from, to_path, processes).await.is_err() {
            result.skip(&path, "failed");
            continue;
        }
        ensure_git_info_exclude(destination_path, &path, paths);
        result.shared.push(path);
    }
    result
}

#[cfg(not(windows))]
async fn create_directory_link(
    canonical_target: &str,
    link_path: &Path,
    _processes: &dyn mighty_platform::ProcessHost,
) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(canonical_target, link_path)
    }
    #[cfg(not(unix))]
    {
        let _ = (canonical_target, link_path);
        Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "directory symlinks unsupported on this target"))
    }
}

/// The `IO_REPARSE_TAG_MOUNT_POINT` buffer of a directory junction to `target`: the `\\??\\` substitute name NT follows, and the plain print
/// name Explorer shows.
#[cfg(any(windows, test))]
fn junction_reparse_buffer(target: &str) -> Vec<u8> {
    let plain = target.replace('/', "\\");
    let (substitute, print) = if let Some(rest) = plain.strip_prefix("\\\\?\\UNC\\") {
        (format!("\\??\\UNC\\{rest}"), format!("\\\\{rest}"))
    } else if let Some(rest) = plain.strip_prefix("\\\\?\\") {
        (format!("\\??\\{rest}"), rest.to_string())
    } else {
        (format!("\\??\\{plain}"), plain.clone())
    };
    let substitute: Vec<u16> = substitute.encode_utf16().collect();
    let print: Vec<u16> = print.encode_utf16().collect();
    let substitute_bytes = (substitute.len() * 2) as u16;
    let print_bytes = (print.len() * 2) as u16;
    let data_length = 8 + substitute_bytes + 2 + print_bytes + 2;
    let mut buffer = Vec::with_capacity(8 + data_length as usize);
    buffer.extend_from_slice(&0xA000_0003u32.to_le_bytes());
    buffer.extend_from_slice(&data_length.to_le_bytes());
    buffer.extend_from_slice(&0u16.to_le_bytes());
    buffer.extend_from_slice(&0u16.to_le_bytes());
    buffer.extend_from_slice(&substitute_bytes.to_le_bytes());
    buffer.extend_from_slice(&(substitute_bytes + 2).to_le_bytes());
    buffer.extend_from_slice(&print_bytes.to_le_bytes());
    for unit in substitute.iter().chain(&[0]).chain(print.iter()).chain(&[0]) {
        buffer.extend_from_slice(&unit.to_le_bytes());
    }
    buffer
}

/// A directory junction made directly with `FSCTL_SET_REPARSE_POINT`; never through `cmd /c mklink`, whose command line a path such
/// as `x&calc` from a cloned repository would escape.
#[cfg(windows)]
async fn create_directory_link(
    canonical_target: &str,
    link_path: &Path,
    _processes: &dyn mighty_platform::ProcessHost,
) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt as _;
    use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_WRITE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        OPEN_EXISTING,
    };
    use windows_sys::Win32::System::IO::DeviceIoControl;
    use windows_sys::Win32::System::Ioctl::FSCTL_SET_REPARSE_POINT;

    let buffer = junction_reparse_buffer(canonical_target);
    std::fs::create_dir(link_path)?;
    let wide: Vec<u16> = link_path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    // SAFETY: `wide` is NUL-terminated and outlives the call; the handle is closed below; `buffer` is the input of the one ioctl.
    let result = unsafe {
        let handle = CreateFileW(
            wide.as_ptr(),
            GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        );
        if handle == INVALID_HANDLE_VALUE {
            Err(std::io::Error::last_os_error())
        } else {
            let mut returned = 0u32;
            let ok = DeviceIoControl(
                handle,
                FSCTL_SET_REPARSE_POINT,
                buffer.as_ptr().cast(),
                buffer.len() as u32,
                std::ptr::null_mut(),
                0,
                &mut returned,
                std::ptr::null_mut(),
            );
            let outcome = if ok == 0 { Err(std::io::Error::last_os_error()) } else { Ok(()) };
            CloseHandle(handle);
            outcome
        }
    };
    if let Err(err) = result {
        let _ = std::fs::remove_dir(link_path);
        return Err(err);
    }
    let meta = std::fs::symlink_metadata(link_path)?;
    if !is_link(&meta) {
        let _ = std::fs::remove_dir(link_path);
        return Err(std::io::Error::other("the directory junction was not made"));
    }
    Ok(())
}

/// Resolves the worktree's git admin directories (`<worktree-git-dir>` and `<git-common-dir>`) and appends an anchored
/// non-trailing-slash exclude entry (e.g. `/node_modules`) to `info/exclude` if not already present.
fn ensure_git_info_exclude(destination: &Path, shared_rel_path: &str, paths: &DelegationPaths) {
    let Some((worktree_top, git_dir, common_dir)) = resolve_worktree_git_dirs(destination) else {
        return;
    };
    let canonical_top = paths.canonical(&worktree_top.to_string_lossy());
    let canonical_dest = paths.canonical(&destination.to_string_lossy());
    let repo_rel = if same(&canonical_dest, &canonical_top) {
        shared_rel_path.trim_matches('/').to_string()
    } else if let Ok(prefix) = Path::new(&canonical_dest).strip_prefix(&canonical_top) {
        let prefix_str = prefix.to_string_lossy().replace('\\', "/");
        format!("{}/{}", prefix_str.trim_matches('/'), shared_rel_path.trim_matches('/'))
    } else {
        shared_rel_path.trim_matches('/').to_string()
    };
    if repo_rel.is_empty() {
        return;
    }
    let pattern = format!("/{}", escape_exclude_glob(&repo_rel));
    let marker = exclude_marker(&canonical_top);
    append_exclude_pattern(&common_dir, &marker, &pattern);
    if common_dir != git_dir {
        append_exclude_pattern(&git_dir, &marker, &pattern);
    }
}

fn resolve_worktree_git_dirs(start: &Path) -> Option<(PathBuf, PathBuf, PathBuf)> {
    let mut cursor = Some(start);
    let mut worktree_top = None;
    let mut dot_git = None;
    while let Some(dir) = cursor {
        let candidate = dir.join(".git");
        if std::fs::symlink_metadata(&candidate).is_ok_and(|meta| !is_link(&meta)) {
            worktree_top = Some(dir.to_path_buf());
            dot_git = Some(candidate);
            break;
        }
        cursor = dir.parent().filter(|p| !p.as_os_str().is_empty());
    }
    let (worktree_top, dot_git) = (worktree_top?, dot_git?);
    let meta = std::fs::symlink_metadata(&dot_git).ok()?;
    if meta.is_dir() {
        return Some((worktree_top, dot_git.clone(), dot_git));
    }
    if !meta.is_file() || meta.len() > 8192 {
        return None;
    }
    let text = std::fs::read_to_string(&dot_git).ok()?;
    let gitdir_line = text.lines().find_map(|line| line.trim().strip_prefix("gitdir:"))?.trim();
    let git_dir_raw = Path::new(gitdir_line);
    let git_dir = if git_dir_raw.is_absolute() { git_dir_raw.to_path_buf() } else { worktree_top.join(git_dir_raw) };
    if !std::fs::symlink_metadata(&git_dir).is_ok_and(|m| !is_link(&m) && m.is_dir()) {
        return None;
    }
    let common_file = git_dir.join("commondir");
    let common_dir = match std::fs::symlink_metadata(&common_file) {
        Ok(cm) if !is_link(&cm) && cm.is_file() && cm.len() <= 4096 => {
            let rel = std::fs::read_to_string(&common_file).ok()?;
            let rel_trimmed = rel.trim();
            let raw_common = Path::new(rel_trimmed);
            let joined = if raw_common.is_absolute() { raw_common.to_path_buf() } else { git_dir.join(raw_common) };
            std::fs::canonicalize(&joined).unwrap_or(joined)
        }
        _ => git_dir.clone(),
    };
    Some((worktree_top, git_dir, common_dir))
}

/// The comment line that owns the entry after it: a discard removes exactly the entries its own worktree added.
fn exclude_marker(worktree_top: &str) -> String {
    format!("# mighty-code shared {}", worktree_top.replace(['\r', '\n'], " "))
}

/// A path as one gitignore pattern with no glob in it: `\ * ? [ ]` escaped, and a trailing space kept.
fn escape_exclude_glob(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for c in path.chars() {
        if matches!(c, '\\' | '*' | '?' | '[' | ']') {
            out.push('\\');
        }
        out.push(c);
    }
    if out.ends_with(' ') {
        out.insert(out.len() - 1, '\\');
    }
    out
}

/// `<admin dir>/info/exclude` when it is a plain file below a plain `info` folder (not a link), with its text; None when absent or unsafe.
fn read_exclude(git_admin_dir: &Path) -> Option<(PathBuf, String)> {
    if !std::fs::symlink_metadata(git_admin_dir).is_ok_and(|m| !is_link(&m) && m.is_dir()) {
        return None;
    }
    let info_dir = git_admin_dir.join("info");
    if !std::fs::symlink_metadata(&info_dir).is_ok_and(|m| !is_link(&m) && m.is_dir()) {
        return None;
    }
    let exclude_path = info_dir.join("exclude");
    match std::fs::symlink_metadata(&exclude_path) {
        Ok(meta) if !is_link(&meta) && meta.is_file() && meta.len() <= 1_048_576 => {
            let text = std::fs::read_to_string(&exclude_path).ok()?;
            Some((exclude_path, text))
        }
        _ => None,
    }
}

fn append_exclude_pattern(git_admin_dir: &Path, marker: &str, pattern: &str) {
    use std::io::Write as _;
    if !std::fs::symlink_metadata(git_admin_dir).is_ok_and(|m| !is_link(&m) && m.is_dir()) {
        return;
    }
    let info_dir = git_admin_dir.join("info");
    match std::fs::symlink_metadata(&info_dir) {
        Ok(meta) if is_link(&meta) || !meta.is_dir() => return,
        Ok(_) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            if std::fs::create_dir(&info_dir).is_err() {
                return;
            }
        }
        Err(_) => return,
    }
    let exclude_path = info_dir.join("exclude");
    let existing = match std::fs::symlink_metadata(&exclude_path) {
        Ok(meta) if is_link(&meta) || !meta.is_file() || meta.len() > 1_048_576 => return,
        Ok(_) => std::fs::read_to_string(&exclude_path).unwrap_or_default(),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(_) => return,
    };
    let lines: Vec<&str> = existing.lines().collect();
    if lines.windows(2).any(|pair| pair[0] == marker && pair[1] == pattern) {
        return;
    }
    let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&exclude_path) else {
        return;
    };
    if !existing.is_empty() && !existing.ends_with('\n') {
        let _ = file.write_all(b"\n");
    }
    let _ = writeln!(file, "{marker}\n{pattern}");
}

/// Takes out every entry [`append_exclude_pattern`] made under `worktree_top`'s marker (the marker line and the pattern after it).
fn remove_exclude_entries(git_admin_dir: &Path, worktree_top: &str) {
    let Some((exclude_path, text)) = read_exclude(git_admin_dir) else { return };
    let marker = exclude_marker(worktree_top);
    let mut kept: Vec<&str> = vec![];
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        if line == marker {
            lines.next();
        } else {
            kept.push(line);
        }
    }
    if kept.len() == text.lines().count() {
        return;
    }
    let mut out = kept.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    let _ = std::fs::write(&exclude_path, out);
}

// ── the pull request (rule 11) ──────────────────────────────────────────────────────────────────────────────────────────

/// `PaneWorktreePR.GitHubRepository`: the `owner/repo` of a GitHub remote URL (https, ssh or scp-like), None for any other host.
pub fn github_repository(remote: &str) -> Option<String> {
    let rest = remote.trim();
    for prefix in ["https://github.com/", "http://github.com/", "ssh://git@github.com/", "git@github.com:", "git://github.com/"] {
        let Some(rest) = rest.strip_prefix(prefix) else { continue };
        let rest = rest.strip_suffix(".git").unwrap_or(rest).trim_end_matches('/');
        let parts: Vec<&str> = rest.split('/').collect();
        let fine = |part: &&str| !part.is_empty() && part.chars().all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.'));
        return (parts.len() == 2 && parts.iter().all(fine)).then(|| format!("{}/{}", parts[0], parts[1]));
    }
    None
}

/// `Uri.EscapeDataString`: every byte of the UTF-8 but RFC 3986's unreserved characters as `%XX`.
fn escape_data(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// `PaneWorktreePR.CompareUrl`: GitHub's page for a pull request of `branch` into `base_branch`.
pub fn compare_url(repository: &str, base_branch: &str, branch: &str) -> String {
    let escape = |value: &str| value.split('/').map(escape_data).collect::<Vec<_>>().join("/");
    format!("https://github.com/{repository}/compare/{}...{}?expand=1", escape(base_branch), escape(branch))
}

/// `PaneWorktreePR.Origin`: the `origin` remote of the checkout, None without one.
pub async fn origin(git: &Git, pane: &PaneWorktree) -> Option<String> {
    let found = run(git, &["remote", "get-url", "origin"], pane.checkout(), STEP_TIMEOUT).await.filter(|r| r.exit_code == 0)?;
    let url = line(&found.stdout);
    (!url.is_empty()).then(|| url.to_string())
}

/// `PaneWorktreePR.Push`: pushes the pane's branch to `origin` under the same name, never forced. None when it went; git's words
/// otherwise.
pub async fn push(git: &Git, pane: &PaneWorktree, locale: &Locale) -> Option<String> {
    let reference = format!("refs/heads/{}", pane.branch());
    let spec = format!("{reference}:{reference}");
    let Some(pushed) = run(git, &["push", "--porcelain", "origin", &spec], pane.checkout(), 180.0).await else {
        return Some(locale.get("delegation.card.error.gitUnfinished"));
    };
    (pushed.exit_code != 0).then(|| message(&pushed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_data_string_is_escaped_as_dotnet_escapes_it() {
        assert_eq!(escape_data("a b/ü~"), "a%20b%2F%C3%BC~");
        assert_eq!(
            compare_url("me/app", "release/1.0", "mighty/p q"),
            "https://github.com/me/app/compare/release/1.0...mighty/p%20q?expand=1"
        );
    }

    #[test]
    fn numstat_counts_files_and_lines_once_each() {
        let text = ["3\t1\tsrc/a.rs", "-\t-\tlogo.png", "10\t0\t한글 파일.md", "2\t2\tsrc/a.rs", ""].join("\0");
        let (stat, paths) = parse_numstat(&text);
        assert_eq!(stat, DiffStat { files: 3, added: 13, removed: 1 }, "a binary file counts no lines; a path listed twice once");
        assert_eq!(paths, ["src/a.rs", "logo.png", "한글 파일.md"]);
        assert_eq!(parse_numstat(""), (DiffStat::default(), vec![]));
        assert_eq!(parse_numstat("garbage\0").0, DiffStat::default());
    }

    #[test]
    fn a_file_read_takes_its_byte_order_mark_off() {
        assert_eq!(read_text(b"\xEF\xBB\xBFhi"), "hi");
        assert_eq!(read_text(&[0xFF, 0xFE, b'h', 0, b'i', 0]), "hi");
        assert_eq!(read_text(b"plain"), "plain");
    }

    #[test]
    fn a_junction_buffer_holds_the_target_as_data_and_never_reaches_a_shell() {
        let buffer = junction_reparse_buffer(r"\\?\C:\repo\x&calc");
        assert_eq!(&buffer[..4], &0xA000_0003u32.to_le_bytes());
        assert_eq!(usize::from(u16::from_le_bytes([buffer[4], buffer[5]])) + 8, buffer.len());
        let units: Vec<u16> = buffer[16..].chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        let text = String::from_utf16(&units).unwrap();
        assert!(text.starts_with(r"\??\C:\repo\x&calc"), "{text}");
        assert!(text.ends_with("C:\\repo\\x&calc\0"), "{text}");
    }

    #[test]
    fn exclude_patterns_escape_globs_and_take_out_only_their_own_entries() {
        assert_eq!(escape_exclude_glob("a[1]*?\\b "), "a\\[1\\]\\*\\?\\\\b\\ ");
        let dir = std::env::temp_dir().join(format!("mighty-exclude-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("keep"), "").unwrap();
        append_exclude_pattern(&dir, "# mighty-code shared a", "/x");
        std::fs::create_dir_all(dir.join("info")).unwrap();
        append_exclude_pattern(&dir, "# mighty-code shared a", "/x");
        append_exclude_pattern(&dir, "# mighty-code shared b", "/x");
        append_exclude_pattern(&dir, "# mighty-code shared a", "/x");
        let text = std::fs::read_to_string(dir.join("info").join("exclude")).unwrap();
        assert_eq!(text, "# mighty-code shared a\n/x\n# mighty-code shared b\n/x\n");
        remove_exclude_entries(&dir, "a");
        assert_eq!(std::fs::read_to_string(dir.join("info").join("exclude")).unwrap(), "# mighty-code shared b\n/x\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn copy_entries_split_on_commas_and_lines() {
        assert_eq!(copy_entries(".env,,a\r\nb"), vec![".env", "a", "b"]);
    }
}
