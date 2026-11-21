//! A human's merge, undo and discard from a child's card, and its close (C# Core/DelegationCards.cs, the coordinator's part of
//! DelegationCleanup.cs and DelegationMerge.cs's `RecordingMerge`/`RecordingUndo`; M/DelegationCards.swift, M/DelegationCleanup.swift:
//! 214-313): the merge queue's steps, the merge a card can undo, the card's answers (`DelegationCardResult`, `DelegationCardNote`, the
//! pending confirmations W7-D's window asks), and the coordinator's card actions. The tools never reach these. The git steps are W7-B's
//! [`crate::git`]. Owner: W7-A.

use std::collections::HashSet;

use crate::coordinator::Coordinator;
use crate::copy::{DelegationCopy, DelegationCopyKind};
use crate::git::cleanup::{self, ChildCleanupHold, ChildCleanupOutcome, ChildDiscardOutcome};
use crate::git::merge::{self, ChildMergeOutcome, ChildUndoOutcome};
use crate::git::paths;
use crate::git::worktree::{report_file_in, task_file_in};
use crate::host::DelegationPaneActivity;
use crate::model::{ChildRecord, ChildState, ChildStateEvent, ChildStates, MergeRecord};
use crate::request::DelegationReasonCode;
use crate::store::{DelegationFile, DelegationFileError, SaveError};
use crate::tools::actions::trim_message;

/// `DelegationCardAction` is mighty-core's (the sidebar's card reads it): merge, undo, discard.
pub use mighty_core::shell::CardAction as DelegationCardAction;

/// `DelegationMergeWait`: what a human's merge from a child's card waits on before it goes through by itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DelegationMergeWait {
    /// The child is still working: it merges once the child reports.
    ChildRunning,
    /// The parent pane is running: it merges once that run ends.
    ParentRunning,
    /// The parent's checkout has uncommitted changes to tracked files: it merges once they are committed or put back.
    ParentChanges,
}

/// `DelegationMergeStep`: what a queued card merge does now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DelegationMergeStep {
    /// Try the merge now.
    Attempt,
    /// Keep it queued, waiting on this.
    Wait(DelegationMergeWait),
    /// Take it out of the queue: the child ended without a new report, was merged, closed, failed or discarded.
    Drop,
}

/// `DelegationMergeQueue.Step`: what a queued merge of a child in `state` does now; `parent_running` is whether its parent pane runs (a
/// closed parent pane does not).
pub fn merge_step(state: ChildState, parent_running: bool) -> DelegationMergeStep {
    match state {
        ChildState::Creating | ChildState::Running | ChildState::Waiting => DelegationMergeStep::Wait(DelegationMergeWait::ChildRunning),
        ChildState::Reported if parent_running => DelegationMergeStep::Wait(DelegationMergeWait::ParentRunning),
        ChildState::Reported => DelegationMergeStep::Attempt,
        _ => DelegationMergeStep::Drop,
    }
}

/// `DelegationMergeQueue.WaitAfter`: what a merge refused for `reason` waits on while it stays queued; None when the refusal is for
/// good (a conflict is the child's to resolve).
pub fn wait_after(reason: DelegationReasonCode) -> Option<DelegationMergeWait> {
    match reason {
        DelegationReasonCode::TrackedChanges => Some(DelegationMergeWait::ParentChanges),
        DelegationReasonCode::ParentBusy => Some(DelegationMergeWait::ParentRunning),
        _ => None,
    }
}

/// `ChildCardDiscardOutcome`: the answer to a human's discard from a child's card, once confirmed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChildCardDiscardOutcome {
    /// The child's worktree, the worktrees nested in it and its branch are gone.
    Discarded,
    /// More worktrees are nested in the child's now than the confirmation named, so nothing was removed: all of them, deepest first,
    /// for the card to ask again.
    NestedChanged(Vec<String>),
    /// Nothing after this step was removed; a later discard goes on from here. In the app's language, git's and the OS's output kept.
    Failed(String),
}

/// `DelegationCardResult`: what a child's card action did (M/AppStore+Delegation.swift:6-17).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DelegationCardResult {
    Done,
    /// Done in git, but the delegation file could not be saved (the reason); the next save writes it.
    DoneUnsaved(String),
    /// Refused with this reason; nothing changed.
    Refused(DelegationReasonCode),
    Failed(String),
    /// More worktrees are nested in the child's than its discard's confirmation named: nothing was removed, and it asks again.
    Reconfirm(Vec<String>),
    /// The merge was refused with merge_conflict, and the child's pane was asked to resolve it and report again.
    ConflictSent,
    /// The merge was refused with merge_conflict, and the child's pane took the request but did not start it.
    ConflictNotStarted,
    /// The merge is queued and goes through by itself once this passes.
    Queued(DelegationMergeWait),
    /// The queued merge was dropped: the child ended without a new report.
    QueueDropped,
}

/// `DelegationCardNote`: the last answer a child's card got, shown on the card until its next action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelegationCardNote {
    pub action: DelegationCardAction,
    pub result: DelegationCardResult,
}

/// `PendingChildDiscard`: a discard from a child's card waiting on the human's confirmation. `branch` is `mighty/<id>`; `nested` the
/// worktrees nested in the child's that go with it, deepest first, or None when they could not be listed (the child cannot be discarded
/// now: the confirmation says why and offers no discard).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingChildDiscard {
    pub id: String,
    pub title: String,
    pub branch: String,
    pub nested: Option<Vec<String>>,
}

/// `PendingParentClose`: a close of a parent pane whose children are still open, waiting on the human's confirmation — only the parent
/// closes, and its children stay as cards under a 'parent closed' node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingParentClose {
    pub id: String,
    pub title: String,
    pub open_children: usize,
}

impl DelegationFile {
    /// `UndoableMerge`: the merge a child's card can undo — the child's last one, while the child is merged, or closed with that merge's
    /// head as the head it was closed at. None otherwise, so an older merge never comes back to undo.
    pub fn undoable_merge(&self, id: &str) -> Option<&MergeRecord> {
        let child = self.child(id)?;
        let record = self.merges.iter().rev().find(|merge| merge.child_id == id)?;
        match child.state {
            ChildState::Merged => Some(record),
            ChildState::Closed => child.closed_head.as_deref().filter(|head| head.eq_ignore_ascii_case(&record.child_head)).map(|_| record),
            _ => None,
        }
    }

    /// `RecordingMerge`: a finished merge written — its MergeRecord joins the file and the child moves from reported to merged. None,
    /// changing nothing, when no child here has the record's id or that child cannot move to merged.
    pub fn recording_merge(&self, record: &MergeRecord) -> Option<DelegationFile> {
        let index = self.child_index(&record.child_id)?;
        let merged = self.children[index].applying(ChildStateEvent::Merge)?;
        let mut file = self.setting_child(index, merged);
        file.merges.push(record.clone());
        Some(file)
    }

    /// `RecordingUndo`: a finished undo written — the merge record leaves the file, a merged child goes back to reported and a closed one
    /// stays closed. None, changing nothing, when the record is not here or its child cannot be undone.
    pub fn recording_undo(&self, record: &MergeRecord) -> Option<DelegationFile> {
        let record_index = self.merges.iter().rposition(|merge| merge == record)?;
        let index = self.child_index(&record.child_id)?;
        let undone = self.children[index].applying(ChildStateEvent::UndoMerge)?;
        let mut file = self.setting_child(index, undone);
        file.merges.remove(record_index);
        Some(file)
    }
}

impl ChildRecord {
    /// What W7-B's git layer reads of this child.
    pub fn git(&self) -> crate::git::ChildGit<'_> {
        crate::git::ChildGit {
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
}

/// `UnsavedReason`: why the delegation file was not saved, for a card's note — in the app's language, with no closing stop.
pub fn unsaved_reason(error: &SaveError, locale: &mighty_core::locale::Locale) -> String {
    match error {
        SaveError::File(DelegationFileError::Full) => locale.get("delegation.card.error.fileFull"),
        SaveError::File(DelegationFileError::Unreadable) => locale.get("delegation.card.error.fileUnreadable"),
        SaveError::Io(error) => trim_message(&error.to_string()),
    }
}

impl Coordinator {
    /// `MergeFromCard`: a human's merge of the child from its card, by W7-B's card merge in the parent's checkout recorded when the child
    /// was made, else the parent pane's folder. It takes its turn with the tool's merges, and while it runs delivery starts no run in the
    /// parent's pane or the child's. A refusal carries one reason code and changes nothing; with no parent checkout known it is
    /// branch_not_checked_out. A merge writes its MergeRecord and moves the child to merged.
    pub(crate) async fn merge_from_card_now(&self, id: &str) -> ChildMergeOutcome {
        let outcome = {
            let _turn = self.inner.merge_turns.lock().await;
            let _panes = CardPanes(self);
            Box::pin(self.card_merge(id)).await
        };
        self.deliver_pending().await;
        outcome
    }

    async fn card_merge(&self, id: &str) -> ChildMergeOutcome {
        let locale = self.locale();
        let Some(child) = self.file().child(id).cloned() else {
            return ChildMergeOutcome::Failed(locale.get("delegation.card.error.noChild"));
        };
        {
            let mut flags = self.flags();
            flags.card_panes.clear();
            flags.card_panes.insert(child.parent_session_id.clone());
            flags.card_panes.insert(child.id.clone());
        }
        let Some(checkout) = self.parent_checkout(&child).await else {
            return ChildMergeOutcome::Refused(DelegationReasonCode::BranchNotCheckedOut);
        };
        let Some(followed) = self.follow_parent_branch(id, &checkout).await else {
            return ChildMergeOutcome::Failed(locale.get("delegation.card.error.noChild"));
        };
        let outcome = Box::pin(merge::card_merge(self.inner.worktrees.git(), &followed.git(), &checkout, &locale)).await;
        let ChildMergeOutcome::Merged(record) = &outcome else { return outcome };
        self.keep_merge(record);
        let context = self.prune_context().await;
        // The merge stands and stays on record in memory; the human hears that the file could not be saved, and the next save writes it.
        let mut state = self.lock();
        let file = DelegationFile::clone(&state.file);
        if let Err(error) = self.commit(&mut state, file, &context) {
            return ChildMergeOutcome::MergedUnsaved(record.clone(), unsaved_reason(&error, &locale));
        }
        outcome
    }

    /// `ConflictResolution`: the request for the child `id` to resolve its conflict with the parent branch, after its card's merge was
    /// refused with merge_conflict — once per report, so None when the child is not reported or was already asked at this report.
    pub fn conflict_resolution(&self, id: &str) -> Option<String> {
        let child = self.file().child(id).filter(|child| child.state == ChildState::Reported)?.clone();
        let mut flags = self.flags();
        if flags.conflict_asked.get(id) == Some(&child.report_revision) {
            return None;
        }
        flags.conflict_asked.insert(id.to_string(), child.report_revision);
        Some(merge::conflict_prompt(&child.git()))
    }

    /// `ConflictResolutionNotStarted`: the child's pane did not start that request, so the next merge that conflicts at this report asks
    /// again.
    pub fn conflict_resolution_not_started(&self, id: &str) {
        self.flags().conflict_asked.remove(id);
    }

    /// `FollowParentBranch`: the child `id` as it is once its recorded parent branch follows `checkout` — when another branch checked out
    /// there holds the recorded one's work, the record names that branch from now on and is saved (should the disk refuse, the next save
    /// writes it). None when no child has the id.
    pub(crate) async fn follow_parent_branch(&self, id: &str, checkout: &str) -> Option<ChildRecord> {
        let found = self.file().child(id).cloned()?;
        let branch = merge::merge_branch(self.inner.worktrees.git(), &found.git(), checkout).await;
        let Some(branch) = branch.filter(|branch| *branch != found.parent_branch) else { return self.file().child(id).cloned() };
        let context = self.prune_context().await;
        // Nothing below suspends until the new branch is kept, so the record is as it is now.
        let mut state = self.lock();
        let index = state.file.child_index(id)?;
        if state.file.children[index].parent_branch != found.parent_branch {
            return Some(state.file.children[index].clone());
        }
        let child = ChildRecord { parent_branch: branch, ..state.file.children[index].clone() };
        let next = state.file.setting_child(index, child.clone());
        self.save_or_keep(&mut state, next, &context);
        Some(child)
    }

    /// `KeepMerge`: a merge that is made goes on record — with its child merged, or all the same should the child have moved on.
    pub(crate) fn keep_merge(&self, record: &MergeRecord) {
        let mut state = self.lock();
        let next = match state.file.recording_merge(record) {
            Some(next) => next,
            None if state.file.child(&record.child_id).is_some() => {
                let mut next = DelegationFile::clone(&state.file);
                next.merges.push(record.clone());
                next
            }
            None => return,
        };
        self.keep(&mut state, next);
    }

    /// `UndoMergeFromCard`: a human's undo from the child's card of the merge [`DelegationFile::undoable_merge`] names. The parent pane's
    /// activity decides parent_busy (a closed parent pane counts as idle); while the undo runs, delivery starts no run in the parent's
    /// pane. A refusal carries one reason code and changes nothing. An undo takes the merge record out of the file: a merged child is
    /// reported again, a closed one stays closed with its branch made again at its recorded head.
    pub(crate) async fn undo_merge_from_card_now(&self, id: &str) -> ChildUndoOutcome {
        let outcome = {
            let _turn = self.inner.merge_turns.lock().await;
            let _panes = CardPanes(self);
            Box::pin(self.card_undo(id)).await
        };
        self.deliver_pending().await;
        outcome
    }

    async fn card_undo(&self, id: &str) -> ChildUndoOutcome {
        let locale = self.locale();
        let no_merge = || ChildUndoOutcome::Failed(locale.get("delegation.card.error.noMergeToUndo"));
        let child = {
            let file = self.file();
            let Some(child) = file.child(id).filter(|_| file.undoable_merge(id).is_some()).cloned() else { return no_merge() };
            let mut flags = self.flags();
            flags.card_panes.clear();
            flags.card_panes.insert(child.parent_session_id.clone());
            child
        };
        let Some(checkout) = self.parent_checkout(&child).await else {
            return ChildUndoOutcome::Refused(DelegationReasonCode::BranchNotCheckedOut);
        };
        let activity = self.host_pane_state(&child.parent_session_id).await.map(|pane| pane.activity);
        // As it is now: the child may have moved on while the host answered.
        let (current, record) = {
            let state = self.lock();
            match (state.file.child(id), state.file.undoable_merge(id)) {
                (Some(current), Some(record)) => (current.clone(), record.clone()),
                _ => return no_merge(),
            }
        };
        let outcome = Box::pin(merge::undo(self.inner.worktrees.git(), &record, &current.git(), &checkout, activity, &locale)).await;
        let ChildUndoOutcome::Undone { record: undone, restored_branch } = &outcome else { return outcome };
        {
            // Undone on disk, so its record goes whatever the child did meanwhile.
            let mut state = self.lock();
            let next = state.file.recording_undo(undone).unwrap_or_else(|| {
                let mut next = DelegationFile::clone(&state.file);
                if let Some(index) = next.merges.iter().rposition(|merge| merge == undone) {
                    next.merges.remove(index);
                }
                next
            });
            self.keep(&mut state, next);
        }
        let context = self.prune_context().await;
        let mut state = self.lock();
        let file = DelegationFile::clone(&state.file);
        if let Err(error) = self.commit(&mut state, file, &context) {
            return ChildUndoOutcome::UndoneUnsaved {
                record: undone.clone(),
                restored_branch: *restored_branch,
                reason: unsaved_reason(&error, &locale),
            };
        }
        outcome
    }

    /// `DiscardFromCard`: a human's discard from the child's card once its confirmation named `confirmed_nested` as the worktrees nested
    /// in the child's (None when it could not list them). The nested worktrees are listed first (when they cannot be, nothing is stopped
    /// or removed); then the child's run is stopped and they are listed again: one there that the confirmation did not name stops the
    /// discard, naming them all to ask again. Otherwise [`Coordinator::discard_child`] removes the child's worktree, those nested in it
    /// and its branch by force, in its turn with the merges and undos.
    pub(crate) async fn discard_from_card_now(&self, id: &str, confirmed_nested: Option<Vec<String>>) -> ChildCardDiscardOutcome {
        let locale = self.locale();
        let failed = |key: &str| ChildCardDiscardOutcome::Failed(locale.get(key));
        {
            let state = self.lock();
            if !state.file.children.iter().any(|child| child.id == id && child.state != ChildState::Discarded) {
                return failed("delegation.card.error.cannotDiscard");
            }
            if self.flags().starting.contains(id) {
                return failed("delegation.card.error.stillStarting");
            }
        }
        if self.nested_worktrees(id).await.is_none() {
            return failed("delegation.card.error.nestedUnlisted");
        }
        self.host_stop_run(id).await;
        let _turn = self.inner.merge_turns.lock().await;
        let Some(nested) = self.nested_worktrees(id).await else { return failed("delegation.card.error.nestedUnlisted") };
        let confirmed: HashSet<String> =
            confirmed_nested.unwrap_or_default().iter().map(|path| comparable(&paths::standardized(path))).collect();
        if !nested.iter().all(|path| confirmed.contains(&comparable(path))) {
            return ChildCardDiscardOutcome::NestedChanged(nested);
        }
        match Box::pin(self.discard_child_now(id)).await {
            ChildDiscardOutcome::Failed(message) => ChildCardDiscardOutcome::Failed(message),
            ChildDiscardOutcome::Discarded => ChildCardDiscardOutcome::Discarded,
        }
    }

    /// `NestedWorktrees`: the registered worktrees nested in the child's, deepest first, for the discard confirmation to name; None when
    /// they could not be listed (the parent's checkout is not known, or git failed).
    pub async fn nested_worktrees(&self, id: &str) -> Option<Vec<String>> {
        let child = self.file().child(id).cloned()?;
        let checkout = self.parent_checkout(&child).await?;
        cleanup::nested_worktrees(self.inner.worktrees.git(), &self.inner.paths, &child.worktree_path, &checkout).await
    }

    /// `WorkspaceRemovalRefusal`: workspace_has_children when the workspace at `folder`, with the panes `pane_ids`, still has a child
    /// that is open or whose worktree is still on disk; None when it may go.
    pub fn workspace_removal_refusal(&self, pane_ids: &HashSet<String>, folder: &str) -> Option<DelegationReasonCode> {
        let file = self.file();
        cleanup::workspace_removal_refusal(
            file.children.iter().map(ChildRecord::git),
            pane_ids,
            folder,
            &self.inner.paths,
            &cleanup::worktree_exists,
        )
    }

    /// `CloseChild`: a human closed the child's pane, on the desktop or from the phone. An open child becomes closed and keeps its card.
    /// A merged one first has copies of its REPORT.md and TASK.md and its branch head stored, and only once they are saved is its
    /// worktree and branch cleaned up, when every rule of W7-B's cleanup holds. Otherwise nothing on disk changes. The merge record
    /// stays, so the merge can still be undone. `still_running`: the host could not end the child's run before its pane went, so the
    /// child counts as busy whatever its pane reads.
    pub(crate) async fn close_child_now(&self, id: &str, still_running: bool) -> ChildCleanupOutcome {
        let Some(found) = self.file().child(id).cloned() else { return ChildCleanupOutcome::Failed("No child has this id.".into()) };
        let activity =
            if still_running { Some(DelegationPaneActivity::Running) } else { self.host_pane_state(id).await.map(|pane| pane.activity) };
        let checkout = self.parent_checkout(&found).await;
        let head = if found.state == ChildState::Merged {
            merge::branch_head(self.inner.worktrees.git(), &found.branch, checkout.as_deref(), &found.worktree_path).await
        } else {
            None
        };
        let context = self.prune_context().await;
        let (child, merge_record) = {
            // Nothing below suspends until the close is saved.
            let mut state = self.lock();
            let Some(index) = state.file.child_index(id).filter(|index| state.file.children[*index].state.is_open()) else {
                return ChildCleanupOutcome::Kept(ChildCleanupHold::NotMerged);
            };
            let child = state.file.children[index].clone();
            let merge_record = (child.state == ChildState::Merged)
                .then(|| state.file.merges.iter().rev().find(|merge| merge.child_id == id).cloned())
                .flatten();
            let mut next = DelegationFile::clone(&state.file);
            if merge_record.is_some() {
                let copies = [
                    (DelegationCopyKind::Report, report_file_in(&child.worktree_path), i64::from(child.report_revision)),
                    (DelegationCopyKind::Task, task_file_in(&child.worktree_path), 0),
                ];
                for (kind, path, revision) in copies {
                    if let Some(copy) = DelegationCopy::try_read(id, kind, revision, std::path::Path::new(&path)) {
                        next = next.setting_copy(copy);
                    }
                }
                next.children[index].closed_head = head;
            }
            next.children[index] = next.children[index].applied(ChildStateEvent::ClosePane);
            if self.commit(&mut state, next.clone(), &context).is_err() {
                // Closed all the same; the next save writes it. Without the copies on disk, nothing is removed.
                self.keep(&mut state, next);
                return match merge_record {
                    None => ChildCleanupOutcome::Kept(ChildCleanupHold::NotMerged),
                    Some(_) => ChildCleanupOutcome::Failed(
                        "The delegation file could not be saved, so the child's worktree and branch were kept.".into(),
                    ),
                };
            }
            (child, merge_record)
        };
        let Some(merge_record) = merge_record else { return ChildCleanupOutcome::Kept(ChildCleanupHold::NotMerged) };
        let Some(checkout) = checkout else { return ChildCleanupOutcome::Kept(ChildCleanupHold::BranchNotCheckedOut) };
        Box::pin(cleanup::clean_up(self.inner.worktrees.git(), &self.inner.paths, &child.git(), &merge_record, &checkout, activity)).await
    }

    /// `DiscardChild`: a human's discard of the child from its card, after a confirmation naming any nested worktrees. It is the only
    /// path that removes unmerged work: the child's run is stopped, a copy of its REPORT.md is kept, and then W7-B's discard removes its
    /// worktree, the worktrees nested in it and its branch by force, with whatever a failed start left. While the child's start is still
    /// going the discard is refused with nothing changed. The child is discarded once its worktree and branch are gone; until then it
    /// keeps its state and card, and a discard may be tried again.
    pub async fn discard_child(&self, id: &str) -> ChildDiscardOutcome {
        let id = id.to_string();
        self.isolated(ChildDiscardOutcome::Failed, move |coordinator| async move { coordinator.discard_child_now(&id).await }).await
    }

    async fn discard_child_now(&self, id: &str) -> ChildDiscardOutcome {
        let locale = self.locale();
        let failed = |key: &str| ChildDiscardOutcome::Failed(locale.get(key));
        {
            let state = self.lock();
            match state.file.child(id) {
                Some(found) if found.state != ChildState::Discarded => {}
                _ => return failed("delegation.card.error.cannotDiscard"),
            }
            if self.flags().starting.contains(id) {
                return failed("delegation.card.error.stillStarting");
            }
        }
        self.host_stop_run(id).await;
        let Some(found) = self.file().child(id).cloned() else { return failed("delegation.card.error.cannotDiscard") };
        let Some(checkout) = self.parent_checkout(&found).await else { return failed("delegation.card.error.parentCheckoutUnknown") };
        let context = self.prune_context().await;
        let child = {
            let mut state = self.lock();
            let Some(index) = state.file.child_index(id).filter(|index| state.file.children[*index].state != ChildState::Discarded) else {
                return failed("delegation.card.error.cannotDiscard");
            };
            let child = state.file.children[index].clone();
            let mut next = DelegationFile::clone(&state.file);
            let report = report_file_in(&child.worktree_path);
            if let Some(copy) =
                DelegationCopy::try_read(id, DelegationCopyKind::Report, i64::from(child.report_revision), std::path::Path::new(&report))
                    .filter(|copy| copy.original_bytes > 0)
            {
                next = next.setting_copy(copy);
            }
            if self.commit(&mut state, next, &context).is_err() {
                return failed("delegation.card.error.notSaved");
            }
            child
        };
        let outcome = Box::pin(cleanup::discard(
            self.inner.worktrees.git(),
            &self.inner.paths,
            &child.git(),
            &checkout,
            self.inner.worktrees.root(),
            &locale,
        ))
        .await;
        if outcome != ChildDiscardOutcome::Discarded {
            return outcome;
        }
        let after = self.prune_context().await;
        let mut state = self.lock();
        if let Some(index) = state.file.child_index(id)
            && let Some(discarded) = state.file.children[index].applying(ChildStateEvent::Discard)
        {
            let next = state.file.setting_child(index, discarded);
            self.save_or_keep(&mut state, next, &after);
        }
        outcome
    }

    /// `ParentCheckout`: where git runs on the parent's side — the checkout recorded when the child was made, else the parent pane's
    /// folder while it is open.
    pub(crate) async fn parent_checkout(&self, child: &ChildRecord) -> Option<String> {
        match &child.parent_checkout {
            Some(recorded) => Some(recorded.clone()),
            None => self.host_pane_state(&child.parent_session_id).await.map(|pane| pane.folder),
        }
    }
}

/// `DelegationPaths.Comparer`: paths compare without regard to case on Windows, exactly elsewhere.
fn comparable(path: &str) -> String {
    if mighty_run::os::windows() { path.to_lowercase() } else { path.to_string() }
}

/// Clears the card panes once a card's merge or undo is over, however it ends (C#'s `finally`).
struct CardPanes<'a>(&'a Coordinator);

impl Drop for CardPanes<'_> {
    fn drop(&mut self) {
        self.0.flags().card_panes.clear();
    }
}
