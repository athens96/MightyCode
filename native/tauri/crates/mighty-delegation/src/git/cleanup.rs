//! Cleanup and discard (C# Core/DelegationCleanup.cs:1-289 on `fix/csharp-from-tauri-review`; M/DelegationCleanup.swift): close
//! cleanup of a merged child never forces (`worktree remove`, then `branch -d`); the human's discard ([`remove_own_worktree`], also a
//! pane worktree's) unlinks directory links first, forces nested worktrees deepest first, clears read-only leftovers and `branch -D`.
//! Never `worktree prune`, `reset --hard`, push or fetch. A directory link is unlinked by itself, never followed and never left to a
//! recursive removal. Owner: W7-B.

use std::collections::HashSet;
use std::path::Path;

use mighty_core::locale::Locale;
use mighty_run::os;

use super::merge::{checked_out_head, has_tracked_changes, head};
use super::paths::{DelegationPaths, is_inside, same};
use super::worktree::is_safe_session_id;
use super::{ChildGit, Git, branch_name, exists, is_folder, is_object_name, line, message, run, usable_folder};
use crate::host::DelegationPaneActivity;
use crate::model::{ChildState, MergeRecord};
use crate::request::{DelegationReasonCode, delegation_names};

/// `ChildCleanup.GitTimeout`: the longest one git step may take. A human waits on these, not a tool call.
pub const GIT_TIMEOUT: f64 = 120.0;
/// C#'s default for a step that names no timeout.
const STEP_TIMEOUT: f64 = 10.0;

/// `ChildCleanupHold`: why closing a merged child's pane kept its worktree and branch. Nothing on disk changed: only the pane closed,
/// and a human may still discard the child from its card.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChildCleanupHold {
    /// The child is not merged, or the merge record is another child's.
    NotMerged,
    /// A run is going in the child's pane.
    ChildBusy,
    /// The child's worktree is gone, or is no worktree of the parent's repository.
    WorktreeMissing,
    /// The worktree is not on the child's branch at the head that was merged.
    HeadMoved,
    /// Tracked files in the worktree differ from its head, staged or not.
    TrackedChanges,
    /// The worktree holds files that are neither tracked nor ignored.
    UntrackedFiles,
    /// Another registered worktree lies inside the child's.
    NestedWorktree,
    /// The parent's checkout does not have the recorded parent branch checked out.
    BranchNotCheckedOut,
    /// The recorded parent branch no longer holds the child's head, so `git branch -d` would refuse.
    NotInParent,
    /// A directory junction or directory symbolic link lies inside the worktree, an ignored `node_modules` among others. A git that
    /// descends one while removing the worktree empties the folder it points to, and one that skips it leaves the worktree behind.
    Link,
}

delegation_names!(serde ChildCleanupHold {
    NotMerged => "not_merged",
    ChildBusy => "child_busy",
    WorktreeMissing => "worktree_missing",
    HeadMoved => "head_moved",
    TrackedChanges => "tracked_changes",
    UntrackedFiles => "untracked_files",
    NestedWorktree => "nested_worktree",
    BranchNotCheckedOut => "branch_not_checked_out",
    NotInParent => "not_in_parent",
    Link => "link",
});

/// `ChildCleanupOutcome`: the answer to a human closing a delegated child's pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChildCleanupOutcome {
    /// The worktree went by plain `git worktree remove` and the branch by `git branch -d`.
    Cleaned,
    /// A condition did not hold, so only the pane closed: nothing on disk changed.
    Kept(ChildCleanupHold),
    /// The copies could not be saved, or git was missing or failed. Git refuses before it deletes anything, and the branch goes last,
    /// so the child's work stays on disk or on its branch.
    Failed(String),
}

/// `ChildDiscardOutcome`: the answer to a human's discard of a delegated child from its card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChildDiscardOutcome {
    /// The child's worktree, every worktree nested in it, a leftover folder at its place and its branch are gone, whichever existed.
    Discarded,
    /// Nothing after this step was removed; a later discard goes on from here. In the app's language, with git's and the OS's own
    /// output kept as it is.
    Failed(String),
}

/// `ChildCleanup.CleanUp`: the cleanup of a merged child whose pane a human closed, after its copies are stored. `record` is the
/// child's merge, `parent_checkout` the parent's side and `child_activity` the child pane's (None once it is closed, which counts as
/// idle). The first rule that applies keeps everything, and every rule is checked before anything is removed:
/// 1. `not_merged`; 2. `child_busy`; 3. `worktree_missing` (gone, or not listed by the parent's repository); 4. `branch_not_checked_out`;
/// 5. `head_moved`; 6. `tracked_changes`; 7. `untracked_files` (ignored files, TASK.md and REPORT.md among them, go with the worktree);
/// 8. `nested_worktree`; 9. `link` (Windows: a directory link inside); 10. `not_in_parent`.
///
/// Otherwise the worktree goes by plain `git worktree remove`, which checks for changes again itself, and then the branch by `git
/// branch -d` from the parent's side. Never `--force`.
pub async fn clean_up(
    git: &Git,
    paths: &DelegationPaths,
    child: &ChildGit<'_>,
    record: &MergeRecord,
    parent_checkout: &str,
    child_activity: Option<DelegationPaneActivity>,
) -> ChildCleanupOutcome {
    use ChildCleanupOutcome::{Failed, Kept};
    let merged_head = record.child_head.to_lowercase();
    if child.state != ChildState::Merged || record.child_id != child.id || !is_object_name(&merged_head) {
        return Kept(ChildCleanupHold::NotMerged);
    }
    if child_activity == Some(DelegationPaneActivity::Running) {
        return Kept(ChildCleanupHold::ChildBusy);
    }
    if !is_folder(child.worktree_path) {
        return Kept(ChildCleanupHold::WorktreeMissing);
    }
    if !git.available() {
        return Failed("Git is missing.".into());
    }
    let Some(parent_head) = checked_out_head(git, &record.parent_branch, parent_checkout).await else {
        return Kept(ChildCleanupHold::BranchNotCheckedOut);
    };
    let Some(listed) = worktrees(git, paths, parent_checkout).await else {
        return Failed("Git could not list the worktrees.".into());
    };
    let own = paths.canonical(child.worktree_path);
    if !listed.iter().any(|path| same(path, &own)) {
        return Kept(ChildCleanupHold::WorktreeMissing);
    }

    let worktree = child.worktree_path;
    let Some(branch) = run(git, &["symbolic-ref", "-q", "HEAD"], worktree, STEP_TIMEOUT).await else {
        return Failed("Git did not finish.".into());
    };
    if branch.exit_code != 0
        || line(&branch.stdout) != format!("refs/heads/{}", child.branch)
        || head(git, worktree).await.as_deref() != Some(merged_head.as_str())
    {
        return Kept(ChildCleanupHold::HeadMoved);
    }
    match has_tracked_changes(git, worktree).await {
        None => return Failed("Git could not read the child's changes.".into()),
        Some(true) => return Kept(ChildCleanupHold::TrackedChanges),
        Some(false) => {}
    }
    // What plain `git worktree remove` checks itself: with tracked files clean, anything listed is a file no ignore rule covers.
    let Some(status) = run(git, &["status", "--porcelain", "--ignore-submodules=none", "--untracked-files=normal"], worktree, STEP_TIMEOUT)
        .await
        .filter(|status| status.exit_code == 0)
    else {
        return Failed("Git could not read the child's files.".into());
    };
    if !status.stdout.is_empty() {
        return Kept(ChildCleanupHold::UntrackedFiles);
    }
    if listed.iter().any(|path| is_inside(path, &own)) {
        return Kept(ChildCleanupHold::NestedWorktree);
    }
    if let Some(held) = link_hold(worktree).await {
        return held;
    }
    let Some(merged) = run(git, &["merge-base", "--is-ancestor", &merged_head, &parent_head], parent_checkout, STEP_TIMEOUT).await else {
        return Failed("Git did not finish.".into());
    };
    if merged.exit_code == 1 {
        return Kept(ChildCleanupHold::NotInParent);
    }
    if merged.exit_code != 0 {
        return Failed(message(&merged));
    }

    // Looked for again right before git: a link made since the first look keeps everything too.
    if let Some(late) = link_hold(worktree).await {
        return late;
    }
    let Some(removed) = run(git, &["worktree", "remove", worktree], parent_checkout, GIT_TIMEOUT).await else {
        return Failed("Git did not finish.".into());
    };
    if removed.exit_code != 0 {
        return Failed(message(&removed));
    }
    // Git may leave the folder and still exit 0 (one it would not descend, a file held open): then the branch stays as well.
    if exists(worktree) {
        return Failed("Git left the child's worktree folder in place, so the branch stays.".into());
    }
    let Some(deleted) = run(git, &["branch", "-d", child.branch], parent_checkout, STEP_TIMEOUT).await else {
        return Failed("Git did not finish; the branch stays.".into());
    };
    if deleted.exit_code != 0 {
        return Failed(message(&deleted));
    }
    ChildCleanupOutcome::Cleaned
}

/// `LinkHold`: `link` when a directory link lies inside `worktree`, looked for off the caller's thread up to the first one; None when
/// none does.
async fn link_hold(worktree: &str) -> Option<ChildCleanupOutcome> {
    let folder = worktree.to_string();
    match blocking(move || has_links(&folder)).await {
        Ok(true) => Some(ChildCleanupOutcome::Kept(ChildCleanupHold::Link)),
        Ok(false) => None,
        Err(error) => Some(ChildCleanupOutcome::Failed(error.to_string())),
    }
}

/// Runs file-system work on tokio's blocking pool.
async fn blocking<T: Send + 'static>(work: impl FnOnce() -> std::io::Result<T> + Send + 'static) -> std::io::Result<T> {
    tokio::task::spawn_blocking(work).await.unwrap_or_else(|error| Err(std::io::Error::other(error.to_string())))
}

/// `ChildCleanup.NestedWorktrees`: the registered worktrees inside the child's worktree, deepest first, for the discard confirmation to
/// name; None when git failed.
pub async fn nested_worktrees(git: &Git, paths: &DelegationPaths, worktree_path: &str, parent_checkout: &str) -> Option<Vec<String>> {
    let listed = worktrees(git, paths, parent_checkout).await?;
    Some(nested(&listed, &paths.canonical(worktree_path)))
}

/// `Nested`: those of `listed` inside `own`, longest first.
fn nested(listed: &[String], own: &str) -> Vec<String> {
    let mut inside: Vec<String> = listed.iter().filter(|path| is_inside(path, own)).cloned().collect();
    inside.sort_by_key(|path| std::cmp::Reverse(path.chars().count()));
    inside
}

/// `ChildCleanup.Discard`: a human's discard from the child's card, once its run is stopped and its report copy kept: the steps of
/// [`remove_own_worktree`] for the child's id, worktree and branch.
pub async fn discard(
    git: &Git,
    paths: &DelegationPaths,
    child: &ChildGit<'_>,
    parent_checkout: &str,
    worktree_root: &str,
    locale: &Locale,
) -> ChildDiscardOutcome {
    remove_own_worktree(git, paths, child.id, child.worktree_path, child.branch, parent_checkout, worktree_root, locale).await
}

/// `ChildCleanup.RemoveOwnWorktree`: the discard of any worktree the app made under `worktree_root` for the session `id` (a delegated
/// child's, or a pane's own). A worktree that is not its place `<root>\<id>`, or a branch that is not `mighty/<id>`, is refused before
/// anything is removed. Otherwise, in this order, each whichever exists:
/// 1. every directory link inside, unlinked by itself (one made meanwhile stops the discard before git);
/// 2. every registered worktree nested in it, deepest first, then its own, each by `git worktree remove --force --force` (twice, so a
///    worktree a killed start left locked goes too; one whose folder is already gone loses its registration);
/// 3. a folder git does not know at its place, as a failed start may leave it (read-only files made writable);
/// 4. its branch, by `git branch -D`: it may hold unmerged work.
///
/// The parent's branch and checkout, and the branches of nested worktrees, are never touched. When the OS refuses a step (a file held
/// open), the step fails with what git or the OS said, nothing after it is removed, and a later discard goes on with what is left.
#[allow(clippy::too_many_arguments)]
pub async fn remove_own_worktree(
    git: &Git,
    paths: &DelegationPaths,
    id: &str,
    worktree_path: &str,
    branch: &str,
    parent_checkout: &str,
    worktree_root: &str,
    locale: &Locale,
) -> ChildDiscardOutcome {
    let failed = |text: String| ChildDiscardOutcome::Failed(text);
    // Only the child's own place is ever forced away, never a folder a record names elsewhere.
    let own = paths.canonical(worktree_path);
    if !is_safe_session_id(id) || !same(&own, &paths.canonical(&os::combine(worktree_root, id))) {
        return failed(locale.get("delegation.card.error.worktreeMisplaced"));
    }
    // Only the child's own branch is ever deleted by force.
    if branch != branch_name(id) {
        return failed(locale.get("delegation.card.error.branchMisplaced"));
    }
    if !git.available() {
        return failed(locale.get("delegation.card.error.gitMissing"));
    }
    let Some(listed) = worktrees(git, paths, parent_checkout).await else {
        return failed(locale.get("delegation.card.error.parentCheckoutUnreachable"));
    };
    // A directory link inside goes first, by itself: git's forced remove must never reach what it points to. Looked for again right
    // before git, a link made meanwhile stops the discard with nothing removed by git.
    let folder = worktree_path.to_string();
    let unlinked = blocking(move || {
        for link in links(&folder)? {
            unlink(&link)?;
        }
        has_links(&folder)
    })
    .await;
    match unlinked {
        Ok(false) => {}
        Ok(true) => return failed(locale.get("delegation.card.error.linkAppeared")),
        Err(error) => return failed(error.to_string()),
    }
    let mut order = nested(&listed, &own);
    order.extend(listed.iter().filter(|path| same(path, &own)).cloned());
    for path in &order {
        let Some(removed) = run(git, &["worktree", "remove", "--force", "--force", path], parent_checkout, GIT_TIMEOUT).await else {
            return failed(locale.get("delegation.card.error.gitUnfinished"));
        };
        if removed.exit_code != 0 {
            return failed(message(&removed));
        }
    }
    let folder = worktree_path.to_string();
    if let Err(error) = blocking(move || remove(&folder)).await {
        return failed(error.to_string());
    }
    // The OS may only have marked the folder for removal while another program still holds it open: it is not gone yet.
    if exists(worktree_path) {
        return failed(os_message(303));
    }
    let reference = format!("refs/heads/{branch}");
    let Some(found) = run(git, &["rev-parse", "-q", "--verify", &reference], parent_checkout, STEP_TIMEOUT)
        .await
        .filter(|found| matches!(found.exit_code, 0 | 1))
    else {
        return failed(locale.get("delegation.card.error.childBranchUnread"));
    };
    if found.exit_code == 0 {
        let Some(deleted) = run(git, &["branch", "-D", branch], parent_checkout, STEP_TIMEOUT).await else {
            return failed(locale.get("delegation.card.error.gitUnfinished"));
        };
        if deleted.exit_code != 0 {
            return failed(message(&deleted));
        }
    }
    ChildDiscardOutcome::Discarded
}

/// `new Win32Exception(code).Message`: the OS's words for an error number, without the number std adds.
fn os_message(code: i32) -> String {
    let text = std::io::Error::from_raw_os_error(code).to_string();
    match text.rfind(" (os error ") {
        Some(at) => text[..at].to_string(),
        None => text,
    }
}

/// `ChildCleanup.Links`: the directory junctions and directory symbolic links inside `folder`, deepest first, found without following
/// any of them; none when the folder is not there. A file link, a cloud placeholder or a deduplicated file is a reparse point git never
/// descends, so it is not one. Fails with what the OS said when a folder in it cannot be read.
pub fn links(folder: &str) -> std::io::Result<Vec<String>> {
    let mut found = vec![];
    directory_links(Path::new(folder), &mut found, false)?;
    let mut found: Vec<String> = found.into_iter().map(|path| path.to_string_lossy().into_owned()).collect();
    found.sort_by_key(|path| std::cmp::Reverse(path.chars().count()));
    Ok(found)
}

/// `ChildCleanup.HasLinks`: whether a directory link lies inside `folder`; the walk stops at the first.
pub fn has_links(folder: &str) -> std::io::Result<bool> {
    let mut found = vec![];
    directory_links(Path::new(folder), &mut found, true)?;
    Ok(!found.is_empty())
}

/// Walks `folder` (hidden and system entries too, never past a directory link), collecting its directory links; `first` stops at one.
fn directory_links(folder: &Path, found: &mut Vec<std::path::PathBuf>, first: bool) -> std::io::Result<()> {
    if !folder.is_dir() {
        return Ok(());
    }
    let mut pending = vec![folder.to_path_buf()];
    while let Some(next) = pending.pop() {
        for entry in std::fs::read_dir(&next)? {
            let path = entry?.path();
            let meta = std::fs::symlink_metadata(&path)?;
            if is_directory_link(&path, &meta) {
                found.push(path);
                if first {
                    return Ok(());
                }
            } else if meta.is_dir() {
                pending.push(path);
            }
        }
    }
    Ok(())
}

/// A folder that is a junction or a symbolic link: Windows names a target for those two kinds of reparse point only.
fn is_directory_link(path: &Path, meta: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileTypeExt as _;
        meta.file_type().is_symlink_dir() && std::fs::read_link(path).is_ok()
    }
    #[cfg(not(windows))]
    {
        meta.file_type().is_symlink() && std::fs::metadata(path).is_ok_and(|target| target.is_dir())
    }
}

/// `Unlink`: removes the junction or link at `path` itself, never what it points to.
fn unlink(path: &str) -> std::io::Result<()> {
    let meta = std::fs::symlink_metadata(path)?;
    if meta.permissions().readonly() {
        let mut permissions = meta.permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
        std::fs::set_permissions(path, permissions)?;
    }
    if is_directory_link(Path::new(path), &meta) { std::fs::remove_dir(path) } else { std::fs::remove_file(path) }
}

/// `Remove`: the file or folder at `path` with everything in it, if it is there. A read-only file is made writable first (Windows
/// refuses one as it is); a link or junction inside is removed itself, never followed.
fn remove(path: &str) -> std::io::Result<()> {
    let Ok(meta) = std::fs::symlink_metadata(path) else { return Ok(()) };
    remove_entry(Path::new(path), &meta)
}

fn remove_entry(path: &Path, meta: &std::fs::Metadata) -> std::io::Result<()> {
    if meta.permissions().readonly() {
        let mut permissions = meta.permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
        std::fs::set_permissions(path, permissions)?;
    }
    if meta.file_type().is_symlink() {
        // A link is removed itself: a folder link as a folder, a file link as a file.
        return std::fs::remove_dir(path).or_else(|_| std::fs::remove_file(path));
    }
    if !meta.is_dir() {
        return std::fs::remove_file(path);
    }
    for entry in std::fs::read_dir(path)? {
        let entry = entry?.path();
        let inner = std::fs::symlink_metadata(&entry)?;
        remove_entry(&entry, &inner)?;
    }
    std::fs::remove_dir(path)
}

/// `ChildCleanup.Worktrees`: the canonical paths of the worktrees registered in `folder`'s repository (`worktree list --porcelain
/// -z`), or None when git failed.
pub async fn worktrees(git: &Git, paths: &DelegationPaths, folder: &str) -> Option<Vec<String>> {
    if !usable_folder(folder) {
        return None;
    }
    let list = run(git, &["worktree", "list", "--porcelain", "-z"], folder, STEP_TIMEOUT).await.filter(|list| list.exit_code == 0)?;
    Some(list.stdout.split('\0').filter_map(|entry| entry.strip_prefix("worktree ")).map(|path| paths.canonical(path)).collect())
}

/// `DelegationWorkspaceRemoval.Refusal`: `workspace_has_children` when the workspace at `folder`, with the panes `pane_ids`, still has
/// a child that is open or whose worktree is still on disk (a closed child kept, or what a failed start left); None when it may go. A
/// child is the workspace's when its pane or its parent's is one of `pane_ids`, or its parent's checkout is `folder` or inside it. A
/// discarded child's worktree is gone, so it never counts. `worktree_exists` is the disk's say ([`exists`] in the app).
pub fn workspace_removal_refusal<'a>(
    children: impl IntoIterator<Item = ChildGit<'a>>,
    pane_ids: &HashSet<String>,
    folder: &str,
    paths: &DelegationPaths,
    worktree_exists: &dyn Fn(&str) -> bool,
) -> Option<DelegationReasonCode> {
    let root = paths.canonical(folder);
    for child in children.into_iter().filter(|child| child.state != ChildState::Discarded) {
        let checkout = child.parent_checkout.map(|recorded| paths.canonical(recorded));
        let ours = pane_ids.contains(child.id)
            || pane_ids.contains(child.parent_session_id)
            || checkout.is_some_and(|checkout| same(&checkout, &root) || is_inside(&checkout, &root));
        let open = !matches!(child.state, ChildState::Failed | ChildState::Closed | ChildState::Discarded);
        if ours && (open || worktree_exists(child.worktree_path)) {
            return Some(DelegationReasonCode::WorkspaceHasChildren);
        }
    }
    None
}

/// The app's `worktreeExists` for [`workspace_removal_refusal`]: `Directory.Exists(path) || File.Exists(path)`.
pub fn worktree_exists(path: &str) -> bool {
    exists(path)
}
