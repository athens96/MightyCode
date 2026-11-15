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
    remove_own_worktree(git, paths, session_id, pane.worktree_path(), pane.branch(), pane.checkout(), root, locale).await
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
        let walked: std::io::Result<()> = (|| {
            if folder {
                let mut pending = vec![PathBuf::from(&from)];
                while let Some(current) = pending.pop() {
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
                        let relative = child.strip_prefix(&from).map(Path::to_path_buf).unwrap_or_default();
                        items.push((child.clone(), Path::new(&to).join(relative)));
                        bytes += inner.len();
                        files += 1;
                    }
                }
            } else {
                items.push((PathBuf::from(&from), PathBuf::from(&to)));
                bytes += meta.len();
                files += 1;
            }
            Ok(())
        })();
        if walked.is_err() {
            result.skip(&path, "failed");
            continue;
        }
        if bytes > COPY_MAXIMUM_BYTES || files > COPY_MAXIMUM_FILES {
            result.skip(&path, "limit");
            break;
        }
        let copied: std::io::Result<()> = (|| {
            for (item, target) in &items {
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)?;
                    // Beyond C#: a folder of the worktree on the way may be a link out of it; nothing is written through one.
                    let place = paths.canonical(&parent.to_string_lossy());
                    if !same(&place, &destination_root) && !is_inside(&place, &destination_root) {
                        return Err(std::io::Error::other("a folder on the way leads out of the worktree"));
                    }
                }
                // `File.Copy(overwrite: false)`: never over a file that is there.
                if std::fs::symlink_metadata(target).is_ok() {
                    return Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, target.to_string_lossy().into_owned()));
                }
                std::fs::copy(item, target)?;
            }
            if folder && items.is_empty() {
                std::fs::create_dir_all(&to)?;
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
    fn a_file_read_takes_its_byte_order_mark_off() {
        assert_eq!(read_text(b"\xEF\xBB\xBFhi"), "hi");
        assert_eq!(read_text(&[0xFF, 0xFE, b'h', 0, b'i', 0]), "hi");
        assert_eq!(read_text(b"plain"), "plain");
    }

    #[test]
    fn copy_entries_split_on_commas_and_lines() {
        assert_eq!(copy_entries(".env,,a\r\nb"), vec![".env", "a", "b"]);
    }
}
