//! The action tools (C# Core/DelegationActionTools.cs; M/DelegationActionTools.swift:18-136): a parent merges a reported child into its
//! recorded branch, fast-forward only, and sends its children follow-ups. Owner: W7-A.

use crate::coordinator::{Coordinator, MAXIMUM_FOLLOW_UP_BYTES, MAXIMUM_FOLLOW_UPS, not_your_child_message};
use crate::copy::stored_bytes;
use crate::git::merge::{self, ChildMergeOutcome};
use crate::model::{ChildStates, DeliveryLane, FollowUp};
use crate::providers;
use crate::request::{DelegationCaller, DelegationFollowUpInfo, DelegationReasonCode, DelegationResponse};
use crate::store::{DelegationFileError, SaveError};

impl Coordinator {
    /// merge(child, expected_head) from `caller`. A child that is not the caller's is answered with an error, one the human started is
    /// refused with human_started, and a caller whose agent's tool call its CLI does not gate (Codex) and whose run did not start in
    /// fullAccess with merge_human_only. Otherwise the merge waits for its turn and runs the tool's fast-forward in the parent's
    /// checkout recorded when the child was made. Its refusal is answered with its one reason code and changes nothing. A fast-forward
    /// writes its MergeRecord and moves the child to merged; should the child have moved on meanwhile, the record is written all the
    /// same, so the merge can be undone from its card.
    pub(crate) async fn merge_now(&self, id: &str, expected_head: &str, caller: &DelegationCaller) -> DelegationResponse {
        let id = id.trim();
        if !self.is_child(id, caller) {
            return DelegationResponse::failure(not_your_child_message(id));
        }
        if self.is_human_started(id) {
            return DelegationResponse::refusal(DelegationReasonCode::HumanStarted);
        }
        if !providers::agent_may_merge(&caller.provider, &caller.permission_mode) {
            return DelegationResponse::refusal(DelegationReasonCode::MergeHumanOnly);
        }
        let _turn = self.inner.merge_turns.lock().await;
        // Read again after the wait: an earlier merge may have taken this child.
        let Some(child) = self.file().child(id).cloned() else { return DelegationResponse::failure(not_your_child_message(id)) };
        let Some(checkout) = self.parent_checkout(&child).await else { return DelegationResponse::failure("The calling pane is closed.") };
        let Some(followed) = self.follow_parent_branch(id, &checkout).await else {
            return DelegationResponse::failure(not_your_child_message(id));
        };
        match Box::pin(merge::tool_merge(self.inner.worktrees.git(), &followed.git(), expected_head, &checkout)).await {
            ChildMergeOutcome::Refused(reason) => DelegationResponse::refusal(reason),
            ChildMergeOutcome::Failed(message) => DelegationResponse::failure(format!("Git did not merge the child: {message}")),
            ChildMergeOutcome::Merged(record) | ChildMergeOutcome::MergedUnsaved(record, _) => {
                // The merge is made, so it is on record before anything else can run here, and stays so even should the disk refuse it
                // now; the next save writes it, and the parent hears so.
                self.keep_merge(&record);
                let context = self.prune_context().await;
                let mut state = self.lock();
                let file = crate::store::DelegationFile::clone(&state.file);
                if let Err(error) = self.commit(&mut state, file, &context) {
                    return DelegationResponse::failure(format!(
                        "The child is merged: {} now points at {}, and the app keeps the merge on record, but the delegation file could not be saved ({}). Mighty Code saves it again by itself; nothing is needed from you. Do not merge this child again.",
                        record.parent_branch,
                        record.merged_commit,
                        unsaved_reason_for_agent(&error)
                    ));
                }
                DelegationResponse { merged: Some(record), ..DelegationResponse::default() }
            }
        }
    }

    /// follow_up(child, text) from `caller`. The first rule that applies answers:
    /// 1. an empty text, one over 48 KiB or with a NUL character, or a child that is not the caller's: an error;
    /// 2. human_started; 3. parent_closed (the caller's pane is closed); 4. child_closed (the child is not open);
    /// 5. follow_up_limit (the child has its two); 6. store_full (no room even after pruning).
    ///
    /// Every refusal changes nothing. Otherwise the follow-up is saved pending with its own id and the child's count goes up by one;
    /// delivery then offers it to the child's pane before the answer, which says where it went.
    pub(crate) async fn follow_up_now(&self, id: &str, text: &str, caller: &DelegationCaller) -> DelegationResponse {
        if text.trim().is_empty() {
            return DelegationResponse::failure("follow_up needs a text: the instruction for the child.");
        }
        // Escapes only add bytes, so the UTF-8 size rules out a huge text before it is encoded.
        if text.len() > MAXIMUM_FOLLOW_UP_BYTES || stored_bytes(text) > MAXIMUM_FOLLOW_UP_BYTES {
            return DelegationResponse::failure("A follow-up's text may be at most 48 KiB.");
        }
        if text.contains('\0') {
            return DelegationResponse::failure("A follow-up's text may not contain a NUL character.");
        }
        let id = id.trim();
        if !self.is_child(id, caller) {
            return DelegationResponse::failure(not_your_child_message(id));
        }
        if self.is_human_started(id) {
            return DelegationResponse::refusal(DelegationReasonCode::HumanStarted);
        }
        // Asks the host about every parent with children, the caller among them.
        let context = self.prune_context().await;
        let (follow_up, count) = {
            // Nothing below suspends until the follow-up is saved, so two calls at once cannot both pass the limit.
            let mut state = self.lock();
            if !context.open_pane_ids.contains(&caller.pane_id) {
                return DelegationResponse::refusal(DelegationReasonCode::ParentClosed);
            }
            let Some(index) = state.file.child_index(id) else { return DelegationResponse::failure(not_your_child_message(id)) };
            if !state.file.children[index].state.is_open() {
                return DelegationResponse::refusal(DelegationReasonCode::ChildClosed);
            }
            let count = state.file.children[index].follow_up_count + 1;
            if count > MAXIMUM_FOLLOW_UPS {
                return DelegationResponse::refusal(DelegationReasonCode::FollowUpLimit);
            }
            let follow_up = FollowUp::new(&mighty_core::wire::new_id(), id, text, DeliveryLane::Pending);
            let mut next = crate::store::DelegationFile::clone(&state.file);
            next.children[index].follow_up_count = count;
            next.follow_ups.push(follow_up.clone());
            match self.commit(&mut state, next, &context) {
                Ok(()) => {}
                Err(SaveError::File(DelegationFileError::Full)) => return DelegationResponse::refusal(DelegationReasonCode::StoreFull),
                Err(_) => return DelegationResponse::failure("The delegation file could not be saved, so no follow-up was recorded."),
            }
            (follow_up, count)
        };
        // Offered to the child's pane before the answer, which says where it went.
        self.deliver_pending().await;
        let saved = self.file().follow_ups.iter().find(|saved| saved.id == follow_up.id).cloned().unwrap_or(follow_up);
        DelegationResponse {
            follow_up: Some(DelegationFollowUpInfo::of(&saved, MAXIMUM_FOLLOW_UPS - count)),
            ..DelegationResponse::default()
        }
    }

    /// `IsChild`: `id` names one of `caller`'s children.
    fn is_child(&self, id: &str, caller: &DelegationCaller) -> bool {
        self.lock().file.children.iter().any(|child| child.id == id && child.parent_session_id == caller.pane_id)
    }

    /// `IsHumanStarted`: `id` names a child the human started — the human's to merge and to send work, never the parent's tools'.
    fn is_human_started(&self, id: &str) -> bool {
        self.lock().file.children.iter().any(|child| child.id == id && !child.notifies_parent())
    }
}

/// `UnsavedReasonForAgent`: why the delegation file was not saved, in English for the merge tool's agent, with no closing stop.
pub fn unsaved_reason_for_agent(error: &SaveError) -> String {
    match error {
        SaveError::File(DelegationFileError::Full) => "the delegation file is at its 4 MiB cap".into(),
        SaveError::File(DelegationFileError::Unreadable) => "the delegation file on disk cannot be read".into(),
        SaveError::Io(error) => trim_message(&error.to_string()),
    }
}

/// `error.Message.Trim('.', '。', ' ', '\r', '\n')`.
pub(crate) fn trim_message(message: &str) -> String {
    message.trim_matches(['.', '。', ' ', '\r', '\n']).to_string()
}
