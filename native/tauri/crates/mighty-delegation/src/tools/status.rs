//! The status tools (C# Core/DelegationStatusTools.cs; M/DelegationStatusTools.swift:39-58): a parent reads where its children stand.
//! Both only read the delegation file, so neither changes anything. Neither lists or reads a child the human started. Owner: W7-A.

use crate::coordinator::{Coordinator, not_your_child_message};
use crate::copy::DelegationCopyKind;
use crate::request::{DelegationCaller, DelegationChildStatus, DelegationChildSummary, DelegationResponse};

impl Coordinator {
    /// list_children from `caller`: its children in the order they were made, each with its id, agent, state and current report
    /// revision. None is an empty list.
    pub(crate) fn list_children(&self, caller: &DelegationCaller) -> DelegationResponse {
        let state = self.lock();
        let children = state
            .file
            .children
            .iter()
            .filter(|child| child.parent_session_id == caller.pane_id && child.notifies_parent())
            .map(DelegationChildSummary::of)
            .collect();
        DelegationResponse { children: Some(children), ..DelegationResponse::default() }
    }

    /// child_status(child) from `caller`: that child's state, the head of its current report and that report's body. An id that is not
    /// one of the caller's children is answered with an error.
    pub(crate) fn child_status(&self, id: &str, caller: &DelegationCaller) -> DelegationResponse {
        let id = id.trim();
        let state = self.lock();
        match state.file.children.iter().find(|c| c.id == id && c.parent_session_id == caller.pane_id && c.notifies_parent()) {
            Some(child) => DelegationResponse {
                status: Some(DelegationChildStatus::of(child, state.file.copy(&child.id, DelegationCopyKind::Report))),
                ..DelegationResponse::default()
            },
            None => DelegationResponse::failure(not_your_child_message(id)),
        }
    }
}
