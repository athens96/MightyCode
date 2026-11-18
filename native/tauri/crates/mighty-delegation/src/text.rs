//! Delegation's words in the app's language (C# Core/DelegationText.cs; M/WorkspaceView.swift:875-955), as the sidebar's child rows
//! and cards and the two confirmations say them. Git's and Windows' own output stays as it is, inside the sentence that names it.
//! Owner: W7-A.

use mighty_core::locale::Locale;
use mighty_core::shell::{DelegationChildRow, Tone};

use crate::cards::{
    DelegationCardAction, DelegationCardNote, DelegationCardResult, DelegationMergeWait, PendingChildDiscard, PendingParentClose,
};
use crate::git::cleanup::{ChildCleanupHold, ChildCleanupOutcome};
use crate::model::{ChildRecord, ChildState};
use crate::request::{DelegationName, DelegationReasonCode};

/// `State`: a child's state (`delegation.child.state.*`).
pub fn state(state: ChildState, locale: &Locale) -> String {
    locale.get(&format!("delegation.child.state.{}", state.name()))
}

/// `Action`: a card action's button (`delegation.card.*`).
pub fn action(action: DelegationCardAction, locale: &Locale) -> String {
    locale.get(match action {
        DelegationCardAction::Merge => "delegation.card.merge",
        DelegationCardAction::Undo => "delegation.card.undo",
        DelegationCardAction::Discard => "delegation.card.discard",
    })
}

/// `ActionHelp`: what a card action does to this child, for its tooltip and its accessible name.
pub fn action_help(action: DelegationCardAction, child: &DelegationChildRow, locale: &Locale) -> String {
    match action {
        DelegationCardAction::Merge => locale.get_with(
            if child.state == ChildState::Reported { "delegation.card.mergeHelp" } else { "delegation.card.mergeLaterHelp" },
            &[("branch", &branch(child, locale))],
        ),
        DelegationCardAction::Undo => locale.get_with("delegation.card.undoHelp", &[("branch", &branch(child, locale))]),
        DelegationCardAction::Discard => {
            locale.get_with("delegation.card.discardHelp", &[("branch", &ChildRecord::branch_name(&child.id))])
        }
    }
}

/// The parent's branch as a card names it: the recorded one, or the words for it when none was recorded.
fn branch(child: &DelegationChildRow, locale: &Locale) -> String {
    if child.parent_branch.is_empty() { locale.get("delegation.card.parentBranch") } else { child.parent_branch.clone() }
}

/// `Note`: the card's line for its last answer, in its tone; None when there is nothing to say (a discard that went through closes the
/// pane and leaves the row, and one asked again shows its confirmation instead).
pub fn note(note: &DelegationCardNote, child: &DelegationChildRow, locale: &Locale) -> Option<(String, Tone)> {
    use DelegationCardAction as A;
    use DelegationCardResult as R;
    let branch = branch(child, locale);
    let with = |key: &str, words: &[(&str, &str)]| locale.get_with(key, words);
    match (note.action, &note.result) {
        (A::Merge, R::Done) => Some((with("delegation.card.merged", &[("branch", &branch)]), Tone::Done)),
        (A::Undo, R::Done) => Some((with("delegation.card.undone", &[("branch", &branch)]), Tone::Done)),
        (A::Merge, R::DoneUnsaved(reason)) => {
            Some((with("delegation.card.mergedUnsaved", &[("branch", &branch), ("error", reason)]), Tone::Err))
        }
        (A::Undo, R::DoneUnsaved(reason)) => {
            Some((with("delegation.card.undoneUnsaved", &[("branch", &branch), ("error", reason)]), Tone::Err))
        }
        (A::Discard, R::Done | R::DoneUnsaved(_)) | (_, R::Reconfirm(_)) => None,
        // Said while the child works on it; once it reports again (or ends without a report) its state says the rest.
        (A::Merge, R::ConflictSent) => matches!(child.state, ChildState::Creating | ChildState::Running | ChildState::Waiting)
            .then(|| (with("delegation.card.conflictSent", &[("branch", &branch)]), Tone::Wait)),
        (A::Merge, R::ConflictNotStarted) => Some((with("delegation.card.conflictNotStarted", &[("branch", &branch)]), Tone::Err)),
        (_, R::ConflictSent | R::ConflictNotStarted) => None,
        (A::Merge, R::Queued(on)) => Some((merge_wait(*on, &branch, locale), Tone::Wait)),
        (A::Merge, R::QueueDropped) => Some((locale.get("delegation.card.mergeQueueDropped"), Tone::Wait)),
        (_, R::Queued(_) | R::QueueDropped) => None,
        (A::Merge, R::Refused(reason)) => {
            Some((with("delegation.card.mergeRefused", &[("reason", &self::reason(*reason, locale))]), Tone::Wait))
        }
        (A::Undo, R::Refused(reason)) => {
            Some((with("delegation.card.undoRefused", &[("reason", &self::reason(*reason, locale))]), Tone::Wait))
        }
        (A::Discard, R::Refused(reason)) => {
            Some((with("delegation.card.discardFailed", &[("error", &self::reason(*reason, locale))]), Tone::Err))
        }
        (A::Merge, R::Failed(message)) => Some((with("delegation.card.mergeFailed", &[("error", message)]), Tone::Err)),
        (A::Undo, R::Failed(message)) => Some((with("delegation.card.undoFailed", &[("error", message)]), Tone::Err)),
        (A::Discard, R::Failed(message)) => Some((with("delegation.card.discardFailed", &[("error", message)]), Tone::Err)),
    }
}

/// `MergeWait`: what a queued card merge waits on before it goes through by itself.
pub fn merge_wait(wait: DelegationMergeWait, branch: &str, locale: &Locale) -> String {
    let key = match wait {
        DelegationMergeWait::ChildRunning => "delegation.card.mergeQueuedChild",
        DelegationMergeWait::ParentRunning => "delegation.card.mergeQueuedParent",
        DelegationMergeWait::ParentChanges => "delegation.card.mergeQueuedChanges",
    };
    locale.get_with(key, &[("branch", branch)])
}

/// `CloseNote`: what closing the child's pane left, when the card says so — the Windows-only `link` hold, which keeps a merged child's
/// worktree that a junction or directory link lies in; None for any other close, as on the Mac.
pub fn close_note(outcome: Option<&ChildCleanupOutcome>, locale: &Locale) -> Option<String> {
    matches!(outcome, Some(ChildCleanupOutcome::Kept(ChildCleanupHold::Link))).then(|| locale.get("delegation.card.keptLink"))
}

/// `Reason`: why a card's merge or undo, or the human's run in a new child worktree, was refused, ending in its reason code.
pub fn reason(reason: DelegationReasonCode, locale: &Locale) -> String {
    use DelegationReasonCode as R;
    let key = match reason {
        R::NotGit => "delegation.reason.notGit",
        R::DetachedHead => "delegation.reason.detachedHead",
        R::UnbornBranch => "delegation.reason.unbornBranch",
        R::WidthCap => "delegation.reason.widthCap",
        R::LowDisk => "delegation.reason.lowDisk",
        R::StoreFull => "delegation.reason.storeFull",
        R::NotReported => "delegation.reason.notReported",
        R::BranchNotCheckedOut => "delegation.reason.branchNotCheckedOut",
        R::TrackedChanges => "delegation.reason.trackedChanges",
        R::MergeConflict => "delegation.reason.mergeConflict",
        R::UndoParentMoved => "delegation.reason.undoParentMoved",
        R::ParentBusy => "delegation.reason.parentBusy",
        R::MergeHumanOnly => "delegation.reason.mergeHumanOnly",
        _ => return locale.get_with("delegation.reason.other", &[("code", reason.name())]),
    };
    locale.get(key)
}

/// `DiscardTitle`: the discard confirmation's title — its question, or that the child cannot be discarded now when the worktrees nested
/// in it could not be listed.
pub fn discard_title(pending: &PendingChildDiscard, locale: &Locale) -> String {
    locale.get(if pending.nested.is_none() { "delegation.discard.unavailableTitle" } else { "delegation.discard.title" })
}

/// `DiscardMessage`: what goes, and the nested worktrees that go with it; or, when those could not be listed, why nothing can go now.
pub fn discard_message(pending: &PendingChildDiscard, locale: &Locale) -> String {
    let Some(paths) = &pending.nested else { return locale.get_with("delegation.discard.unavailable", &[("title", &pending.title)]) };
    let nested = if paths.is_empty() { locale.get("delegation.discard.noNested") } else { nested_words(paths, locale) };
    format!("{}\n\n{nested}", locale.get_with("delegation.discard.message", &[("title", &pending.title), ("branch", &pending.branch)]))
}

/// `MergedDiscardMessage`: the offer after a merge from the card — the merged work stays in the parent's branch; what the discard
/// removes, and the nested worktrees that go with it.
pub fn merged_discard_message(pending: &PendingChildDiscard, locale: &Locale) -> String {
    let nested = match &pending.nested {
        Some(paths) if !paths.is_empty() => format!("\n\n{}", nested_words(paths, locale)),
        _ => String::new(),
    };
    format!("{}{nested}", locale.get_with("delegation.mergedDiscard.message", &[("title", &pending.title), ("branch", &pending.branch)]))
}

fn nested_words(paths: &[String], locale: &Locale) -> String {
    locale.get_with("delegation.discard.nested", &[("count", &paths.len().to_string()), ("paths", &paths.join("\n"))])
}

/// `ParentCloseMessage`: the parent-close confirmation's text, with the count of its open children.
pub fn parent_close_message(pending: &PendingParentClose, locale: &Locale) -> String {
    locale.get_with("delegation.parentClose.message", &[("title", &pending.title), ("count", &pending.open_children.to_string())])
}
