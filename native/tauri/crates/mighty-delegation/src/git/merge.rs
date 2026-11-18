//! Merge and undo (C# Core/DelegationMerge.cs; M/DelegationMerge.swift): the tool's `merge --ff-only`, the card's `merge-tree
//! --write-tree` and at most one `commit-tree` merge commit, the branch follow, undo with `read-tree -m -u` and `update-ref` and its
//! rollback. Every refusal comes before any git write. Owner: W7-B.

use mighty_core::locale::Locale;

use super::{ChildGit, Git, is_folder, is_object_name, line, message, run, usable_folder};
use crate::host::DelegationPaneActivity;
use crate::model::{ChildState, MergeKind, MergeRecord};
use crate::request::DelegationReasonCode;

/// `ChildMerge.FastForwardTimeout`: the longest the one git write may take, well inside the limit of a tool call.
pub const FAST_FORWARD_TIMEOUT: f64 = 30.0;
/// `ChildMerge.CardTimeout`: the longest one git step of a card merge or undo may take. A human waits on these, not a tool call.
pub const CARD_TIMEOUT: f64 = 120.0;
/// C#'s default for a step that names no timeout.
const STEP_TIMEOUT: f64 = 10.0;

/// `ChildMergeOutcome`: the answer to a merge of a delegated child.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChildMergeOutcome {
    /// The recorded parent branch now holds the child's head; the record is what the delegation file writes.
    Merged(MergeRecord),
    /// A card's merge is made and on record in memory, but the delegation file could not be saved (why, in the app's language); the
    /// next save writes it. Made by the coordinator, never here.
    MergedUnsaved(MergeRecord, String),
    /// Refused before any git write; nothing changed.
    Refused(DelegationReasonCode),
    /// Git was missing or failed. A fast-forward git refuses leaves the parent as it was. The words are English for the merge tool's
    /// agent and in the app's language for a card, with git's own output kept as it is.
    Failed(String),
}

/// `ChildUndoOutcome`: the answer to a human's undo of a merge, from the child's card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChildUndoOutcome {
    /// The recorded parent branch, its index and its files are back at the pre-merge commit; `restored_branch` says the child's
    /// deleted branch was made again at its recorded head.
    Undone { record: MergeRecord, restored_branch: bool },
    /// A card's undo is made and its record taken out in memory, but the delegation file could not be saved; the next save writes it.
    /// Made by the coordinator, never here.
    UndoneUnsaved { record: MergeRecord, restored_branch: bool, reason: String },
    /// Refused before any git write; nothing changed.
    Refused(DelegationReasonCode),
    /// The record is no undoable merge of this child, or git was missing or failed. The steps already taken are put back.
    Failed(String),
}

/// `ChildMerge.ConflictPrompt`: what asks a reported child whose card merge was refused with `merge_conflict` to bring its parent
/// branch in, resolve the conflicts on its own branch and report again.
pub fn conflict_prompt(child: &ChildGit<'_>) -> String {
    let (branch, parent) = (child.branch, child.parent_branch);
    let report = super::worktree::report_file_in(child.worktree_path);
    format!(
        "[Mighty Code] Merging your branch {branch} into {parent} from the app was refused: the two conflict. In this worktree, bring {parent} into your branch with `git merge {parent}`, resolve every conflict so that the changes on both sides are kept (ask when a choice is unclear), run the checks that apply, and commit the merge on {branch}, leaving no uncommitted changes to tracked files. Then add a short section to {report} saying what conflicted and how you resolved it. The app merges your branch once you report again. Do not push, rebase or switch branches."
    )
}

/// `ChildMerge.ToolMerge`: the parent's merge tool, fast-forward only. `parent_checkout` is the parent pane's folder. The first rule
/// that applies answers, and every refusal comes before any git write:
/// 1. `not_reported`: the child is not reported.
/// 2. `worktree_missing`: the child's worktree is gone.
/// 3. `head_moved`: `expected_head` is not the reported head, or the child's worktree is no longer on its own branch at that head.
/// 4. `tracked_changes`: the child's worktree has changes to tracked files.
/// 5. `branch_not_checked_out`: the parent checkout does not have the recorded parent branch checked out.
/// 6. `tracked_changes`: the parent checkout has changes to tracked files.
/// 7. `diverged`: the parent branch's head is not an ancestor of the expected head.
///
/// Otherwise its only git write is one `git merge --ff-only` of the expected head in the parent checkout.
pub async fn tool_merge(git: &Git, child: &ChildGit<'_>, expected_head: &str, parent_checkout: &str) -> ChildMergeOutcome {
    use ChildMergeOutcome::{Failed, Refused};
    let (ChildState::Reported, Some(reported)) = (child.state, child.report_head) else {
        return Refused(DelegationReasonCode::NotReported);
    };
    if !is_folder(child.worktree_path) {
        return Refused(DelegationReasonCode::WorktreeMissing);
    }
    if !git.available() {
        return Failed("Git is missing.".into());
    }
    let expected = expected_head.trim().to_lowercase();
    if !is_object_name(&expected) || expected != reported.to_lowercase() {
        return Refused(DelegationReasonCode::HeadMoved);
    }
    let Some(child_branch) = run(git, &["symbolic-ref", "-q", "HEAD"], child.worktree_path, STEP_TIMEOUT).await else {
        return Failed("Git did not finish.".into());
    };
    if !matches!(child_branch.exit_code, 0 | 1) {
        return Refused(DelegationReasonCode::WorktreeMissing);
    }
    if child_branch.exit_code != 0
        || line(&child_branch.stdout) != format!("refs/heads/{}", child.branch)
        || head(git, child.worktree_path).await.as_deref() != Some(expected.as_str())
    {
        return Refused(DelegationReasonCode::HeadMoved);
    }
    match has_tracked_changes(git, child.worktree_path).await {
        None => return Failed("Git could not read the child's changes.".into()),
        Some(true) => return Refused(DelegationReasonCode::TrackedChanges),
        Some(false) => {}
    }
    let Some(pre_merge) = checked_out_head(git, child.parent_branch, parent_checkout).await else {
        return Refused(DelegationReasonCode::BranchNotCheckedOut);
    };
    match has_tracked_changes(git, parent_checkout).await {
        None => return Failed("Git could not read the parent's changes.".into()),
        Some(true) => return Refused(DelegationReasonCode::TrackedChanges),
        Some(false) => {}
    }
    let Some(ancestor) = run(git, &["merge-base", "--is-ancestor", &pre_merge, &expected], parent_checkout, STEP_TIMEOUT).await else {
        return Failed("Git did not finish.".into());
    };
    if ancestor.exit_code == 1 {
        return Refused(DelegationReasonCode::Diverged);
    }
    if ancestor.exit_code != 0 {
        return Failed(message(&ancestor));
    }
    let Some(merged) =
        run(git, &["merge", "--ff-only", "--no-autostash", "--no-stat", "-q", &expected], parent_checkout, FAST_FORWARD_TIMEOUT).await
    else {
        return Failed("Git did not finish.".into());
    };
    if merged.exit_code != 0 {
        return Failed(message(&merged));
    }
    if head(git, parent_checkout).await.as_deref() != Some(expected.as_str()) {
        return Failed("The parent branch is not at the merged head.".into());
    }
    ChildMergeOutcome::Merged(MergeRecord {
        child_id: child.id.to_string(),
        kind: MergeKind::ToolFastForward,
        parent_branch: child.parent_branch.to_string(),
        pre_merge_commit: pre_merge,
        merged_commit: expected.clone(),
        child_head: expected,
    })
}

/// `ChildMerge.CardMerge`: a human's merge from the child's card. The reported head goes into the recorded parent branch by
/// fast-forward when the branch's head is an ancestor of it, and otherwise as exactly one merge commit whose parents are the old
/// parent head and the child's head. The first rule that applies answers, and every refusal comes before any git write:
/// 1. `not_reported`: the child is not reported.
/// 2. `branch_not_checked_out`: the parent checkout does not have the recorded parent branch checked out.
/// 3. `tracked_changes`: the parent checkout has changes to tracked files.
/// 4. `merge_conflict`: `git merge-tree` finds conflicts between the parent's head and the child's.
///
/// The merge commit holds exactly the tree merge-tree checked (made with `git commit-tree`). The branch, index and files then move to
/// it, or to the child's head, by one `git merge --ff-only`, which never overwrites an untracked file.
pub async fn card_merge(git: &Git, child: &ChildGit<'_>, parent_checkout: &str, locale: &Locale) -> ChildMergeOutcome {
    use ChildMergeOutcome::{Failed, Refused};
    let reported = child.report_head.map(str::to_lowercase).filter(|head| is_object_name(head));
    let (ChildState::Reported, Some(reported)) = (child.state, reported) else {
        return Refused(DelegationReasonCode::NotReported);
    };
    if !git.available() {
        return Failed(locale.get("delegation.card.error.gitMissing"));
    }
    let Some(pre_merge) = checked_out_head(git, child.parent_branch, parent_checkout).await else {
        return Refused(DelegationReasonCode::BranchNotCheckedOut);
    };
    match has_tracked_changes(git, parent_checkout).await {
        None => return Failed(locale.get("delegation.card.error.parentChangesUnread")),
        Some(true) => return Refused(DelegationReasonCode::TrackedChanges),
        Some(false) => {}
    }
    let unfinished = || Failed(locale.get("delegation.card.error.gitUnfinished"));
    let Some(preview) = run(git, &["merge-tree", "--write-tree", &pre_merge, &reported], parent_checkout, CARD_TIMEOUT).await else {
        return unfinished();
    };
    if preview.exit_code == 1 {
        return Refused(DelegationReasonCode::MergeConflict);
    }
    let tree = line(&preview.stdout).split('\n').next().unwrap_or("").trim_end_matches('\r').to_string();
    if preview.exit_code != 0 || !is_object_name(&tree) {
        return Failed(message(&preview));
    }
    let Some(ancestor) = run(git, &["merge-base", "--is-ancestor", &pre_merge, &reported], parent_checkout, STEP_TIMEOUT).await else {
        return unfinished();
    };
    let (target, kind) = match ancestor.exit_code {
        0 => (reported.clone(), MergeKind::CardFastForward),
        1 => {
            let text = format!("Merge branch '{}' into {}", child.branch, child.parent_branch);
            let Some(made) =
                run(git, &["commit-tree", &tree, "-p", &pre_merge, "-p", &reported, "-m", &text], parent_checkout, CARD_TIMEOUT).await
            else {
                return unfinished();
            };
            let commit = line(&made.stdout).to_string();
            if made.exit_code != 0 || !is_object_name(&commit) {
                return Failed(message(&made));
            }
            (commit, MergeKind::CardMergeCommit)
        }
        _ => return Failed(message(&ancestor)),
    };
    let Some(moved) = run(git, &["merge", "--ff-only", "--no-autostash", "--no-stat", "-q", &target], parent_checkout, CARD_TIMEOUT).await
    else {
        return unfinished();
    };
    if moved.exit_code != 0 {
        return Failed(message(&moved));
    }
    if head(git, parent_checkout).await.as_deref() != Some(target.as_str()) {
        return Failed(locale.get("delegation.card.error.parentNotAtMerge"));
    }
    ChildMergeOutcome::Merged(MergeRecord {
        child_id: child.id.to_string(),
        kind,
        parent_branch: child.parent_branch.to_string(),
        pre_merge_commit: pre_merge,
        merged_commit: target,
        child_head: reported,
    })
}

/// `ChildStates.After(UndoMerge)` is not null: merged (back to reported) or closed (stays closed).
fn undoable(state: ChildState) -> bool {
    matches!(state, ChildState::Merged | ChildState::Closed)
}

/// `ChildMerge.Undo`: a human's undo of `record`, a merge of `child`, from the child's card. `parent_activity` is the parent pane's,
/// None when that pane is closed (a closed parent counts as idle). The child must be merged, or closed after the merge; any other
/// child, or another child's record, fails with nothing changed. The first rule that applies answers, and every refusal comes before
/// any git write:
/// 1. `branch_not_checked_out`: the parent checkout does not have the recorded parent branch checked out.
/// 2. `undo_parent_moved`: that branch's head is not the merged commit.
/// 3. `parent_busy`: the parent pane is running.
/// 4. `tracked_changes`: the parent checkout has changes to tracked files.
///
/// Otherwise a deleted child branch is made again at its recorded head, the index and files go back to the pre-merge commit by `git
/// read-tree -m -u` (never `reset --hard`: untracked files stay, and one in the way fails the undo), and the branch moves back from
/// the merged commit by compare-and-swap. A step that fails puts the earlier ones back.
pub async fn undo(
    git: &Git,
    record: &MergeRecord,
    child: &ChildGit<'_>,
    parent_checkout: &str,
    parent_activity: Option<DelegationPaneActivity>,
    locale: &Locale,
) -> ChildUndoOutcome {
    use ChildUndoOutcome::Refused;
    let failed = |key: &str| ChildUndoOutcome::Failed(locale.get(key));
    if record.child_id != child.id || !undoable(child.state) {
        return failed("delegation.card.error.noMergeToUndo");
    }
    let (merged, pre_merge, child_head) =
        (record.merged_commit.to_lowercase(), record.pre_merge_commit.to_lowercase(), record.child_head.to_lowercase());
    if ![&merged, &pre_merge, &child_head].iter().all(|name| is_object_name(name)) {
        return failed("delegation.card.error.badMergeRecord");
    }
    if !git.available() {
        return failed("delegation.card.error.gitMissing");
    }
    let Some(current) = checked_out_head(git, &record.parent_branch, parent_checkout).await else {
        return Refused(DelegationReasonCode::BranchNotCheckedOut);
    };
    if current != merged {
        return Refused(DelegationReasonCode::UndoParentMoved);
    }
    if parent_activity == Some(DelegationPaneActivity::Running) {
        return Refused(DelegationReasonCode::ParentBusy);
    }
    match has_tracked_changes(git, parent_checkout).await {
        None => return failed("delegation.card.error.parentChangesUnread"),
        Some(true) => return Refused(DelegationReasonCode::TrackedChanges),
        Some(false) => {}
    }

    let reason = format!("mighty: undo the merge of {}", child.branch);
    let child_ref = format!("refs/heads/{}", child.branch);
    let parent_ref = format!("refs/heads/{}", record.parent_branch);
    let Some(found) = run(git, &["rev-parse", "-q", "--verify", &child_ref], parent_checkout, STEP_TIMEOUT)
        .await
        .filter(|found| matches!(found.exit_code, 0 | 1))
    else {
        return failed("delegation.card.error.childBranchUnread");
    };
    let restores_branch = found.exit_code == 1;
    if restores_branch {
        // An empty old value: made only while no such branch exists.
        let Some(made) = run(git, &["update-ref", "-m", &reason, &child_ref, &child_head, ""], parent_checkout, STEP_TIMEOUT).await else {
            return failed("delegation.card.error.gitUnfinished");
        };
        if made.exit_code != 0 {
            return ChildUndoOutcome::Failed(message(&made));
        }
    }
    let put_branch_back = || async {
        if restores_branch {
            run(git, &["update-ref", "-m", &reason, "-d", &child_ref, &child_head], parent_checkout, STEP_TIMEOUT).await;
        }
    };

    // Fresh file stamps, so read-tree compares what the files hold.
    run(git, &["update-index", "-q", "--refresh"], parent_checkout, CARD_TIMEOUT).await;
    if !run(git, &["read-tree", "-m", "-u", &merged, &pre_merge], parent_checkout, CARD_TIMEOUT).await.is_some_and(|r| r.exit_code == 0) {
        put_branch_back().await;
        return failed("delegation.card.error.filesNotRestored");
    }
    if !run(git, &["update-ref", "-m", &reason, &parent_ref, &pre_merge, &merged], parent_checkout, STEP_TIMEOUT)
        .await
        .is_some_and(|r| r.exit_code == 0)
    {
        run(git, &["read-tree", "-m", "-u", &pre_merge, &merged], parent_checkout, CARD_TIMEOUT).await;
        put_branch_back().await;
        return failed("delegation.card.error.parentMovedDuringUndo");
    }
    ChildUndoOutcome::Undone { record: record.clone(), restored_branch: restores_branch }
}

/// `ChildMerge.MergeBranch` (the branch follow): the branch a merge of `child` goes into in `parent_checkout`: the recorded parent
/// branch while it is checked out there. When another branch is checked out and holds the recorded one's work (the recorded branch's
/// head, or the child's base commit once that branch is gone, is an ancestor of its head), as when the recorded branch was merged into
/// main, that branch is. None with a detached HEAD, an unrelated branch or a failed git.
pub async fn merge_branch(git: &Git, child: &ChildGit<'_>, parent_checkout: &str) -> Option<String> {
    if !usable_folder(parent_checkout) {
        return None;
    }
    let current = run(git, &["symbolic-ref", "-q", "HEAD"], parent_checkout, STEP_TIMEOUT).await.filter(|r| r.exit_code == 0)?;
    let branch = line(&current.stdout).strip_prefix("refs/heads/")?.to_string();
    if branch == child.parent_branch {
        return Some(branch);
    }
    let verify = format!("refs/heads/{}^{{commit}}", child.parent_branch);
    let recorded = run(git, &["rev-parse", "-q", "--verify", &verify], parent_checkout, STEP_TIMEOUT)
        .await
        .filter(|r| matches!(r.exit_code, 0 | 1))?;
    let anchor = if recorded.exit_code == 0 { line(&recorded.stdout).to_string() } else { child.base_commit.to_lowercase() };
    if !is_object_name(&anchor) {
        return None;
    }
    run(git, &["merge-base", "--is-ancestor", &anchor, "HEAD"], parent_checkout, STEP_TIMEOUT).await.filter(|r| r.exit_code == 0)?;
    Some(branch)
}

/// `ChildMerge.CheckedOutHead`: the head of `branch` when `folder` is a checkout with that branch checked out, else None.
pub async fn checked_out_head(git: &Git, branch: &str, folder: &str) -> Option<String> {
    if !usable_folder(folder) {
        return None;
    }
    let current = run(git, &["symbolic-ref", "-q", "HEAD"], folder, STEP_TIMEOUT).await.filter(|r| r.exit_code == 0)?;
    if line(&current.stdout) != format!("refs/heads/{branch}") {
        return None;
    }
    head(git, folder).await
}

/// `ChildMerge.Head`: the commit checked out in `folder`, or None.
pub async fn head(git: &Git, folder: &str) -> Option<String> {
    let result = run(git, &["rev-parse", "-q", "--verify", "HEAD^{commit}"], folder, STEP_TIMEOUT).await.filter(|r| r.exit_code == 0)?;
    let sha = line(&result.stdout);
    is_object_name(sha).then(|| sha.to_string())
}

/// `ChildMerge.HasTrackedChanges`: whether tracked files in `folder`'s checkout differ from its HEAD, staged or not; untracked and
/// ignored files do not count. None when git failed. Git compares through the checkout's own line-ending settings, so files it wrote
/// with CRLF read as unchanged.
pub async fn has_tracked_changes(git: &Git, folder: &str) -> Option<bool> {
    run(git, &["status", "--porcelain", "--untracked-files=no"], folder, STEP_TIMEOUT)
        .await
        .filter(|status| status.exit_code == 0)
        .map(|status| !status.stdout.is_empty())
}

/// The coordinator's `BranchHead` (C# DelegationCleanup.cs:373-383): the head of the child's branch, read on the parent's side or else
/// in its worktree; None when neither has it.
pub async fn branch_head(git: &Git, branch: &str, parent_checkout: Option<&str>, worktree_path: &str) -> Option<String> {
    let verify = format!("refs/heads/{branch}^{{commit}}");
    for folder in parent_checkout.into_iter().chain([worktree_path]).filter(|folder| is_folder(folder)) {
        let Some(found) = run(git, &["rev-parse", "-q", "--verify", &verify], folder, STEP_TIMEOUT).await.filter(|r| r.exit_code == 0)
        else {
            continue;
        };
        let sha = line(&found.stdout);
        if is_object_name(sha) {
            return Some(sha.to_string());
        }
    }
    None
}
