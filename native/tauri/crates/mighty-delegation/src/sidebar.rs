//! How the sidebar lists delegated children (C# Core/DelegationSidebar.cs; M/DelegationSidebar.swift:3-141): each child's row from the
//! delegation file (mighty-core's [`DelegationChildRow`], which the shell's tree and card read), the counts a parent's close and the
//! worktree menu ask for, the shown state and its tone. The tree itself (`DelegationSidebar.Tree`) is mighty-core's
//! `shell::delegation_tree`. The delegation file is the authority for each child's link and state; the open panes come from the
//! snapshot. Nothing here depends on the switch, so existing children stay listed with it off. Owner: W7-A.

use mighty_core::model::RunSession;
use mighty_core::shell::{DelegationChildRow, Tone};

use crate::copy::DelegationCopyKind;
use crate::model::{ChildRecord, ChildState, ChildStates};
use crate::store::DelegationFile;

/// `DelegationChildRow.Of`: the row of `record`, its task read from `file`'s stored copy and shortened to one line.
pub fn row(record: &ChildRecord, file: &DelegationFile) -> DelegationChildRow {
    DelegationChildRow {
        id: record.id.clone(),
        parent_session_id: record.parent_session_id.clone(),
        state: record.state,
        task: file.copy(&record.id, DelegationCopyKind::Task).and_then(|task| mighty_core::normalize::shortened(&task.text)),
        parent_branch: record.parent_branch.clone(),
        parent_checkout: record.parent_checkout.clone(),
        can_undo: file.undoable_merge(&record.id).is_some(),
        held_notices: file.held_notices(&record.id).len(),
        started_by_human: !record.notifies_parent(),
        agent: record.agent().to_string(),
    }
}

/// `Rows`: every child the file holds except discarded ones, in the order they were made.
pub fn rows(file: &DelegationFile) -> Vec<DelegationChildRow> {
    file.children.iter().filter(|child| child.state != ChildState::Discarded).map(|child| row(child, file)).collect()
}

/// `OpenChildCount`: the open children of the parent pane `parent_id`, for its close to ask about — the open ones among `children`, and
/// the child panes in `sessions` that `children` does not list yet (a child made a moment ago whose row has not arrived).
pub fn open_child_count(parent_id: &str, children: &[DelegationChildRow], sessions: &[&RunSession]) -> usize {
    let listed: Vec<&DelegationChildRow> = children.iter().filter(|child| child.parent_session_id == parent_id).collect();
    let open = listed.iter().filter(|child| child.state.is_open()).count();
    let unlisted = sessions
        .iter()
        .filter(|session| session.parent_session_id.as_deref() == Some(parent_id) && !listed.iter().any(|child| child.id == session.id()))
        .count();
    open + unlisted
}

/// `OpenHumanChildCount`: how many open children the human started from the parent pane `parent_id`, as the worktree button's menu
/// counts them toward its cap.
pub fn open_human_child_count(parent_id: &str, children: &[DelegationChildRow]) -> usize {
    children.iter().filter(|child| child.parent_session_id == parent_id && child.started_by_human && child.state.is_open()).count()
}

/// `ShownState`: waiting while a running child's pane has a request only a human can answer, otherwise its record's state.
pub fn shown_state(state: ChildState, asks_human: bool) -> ChildState {
    if state == ChildState::Running && asks_human { ChildState::Waiting } else { state }
}

/// `Tone`: the tone of a row's state word and glyph.
pub fn tone(state: ChildState) -> Tone {
    match state {
        ChildState::Creating | ChildState::Running => Tone::Run,
        ChildState::Waiting => Tone::Wait,
        ChildState::Reported | ChildState::Merged => Tone::Done,
        ChildState::Failed => Tone::Err,
        ChildState::Interrupted => Tone::Stop,
        _ => Tone::Idle,
    }
}
