//! The delegation model (C# Core/DelegationModel.cs; M/DelegationModel.swift): the child states and the only moves between them
//! (`ChildStates.After`, `ChildStateEvent`), `ChildRecord`, the lanes, routes and receipts, notices, follow-ups, merges and the request
//! key (`DelegationRequestKey.Make`). Owner: W7-A.
//!
//! W7 Step 0 wrote the enums and the merge record [`crate::request`]'s answer carries, with their names; W7-A adds the rest here
//! (`ChildStateEvent`, `ChildStates.After`/`IsOpen`, `ChildRecord`, `DeliveryReceipt`, `Notice`, `FollowUp`, the request key) and the
//! constructors of the answer's records from a `ChildRecord` (`DelegationChildInfo.Of`, …).

use serde::{Deserialize, Serialize};

use crate::request::delegation_names;

/// `ChildState` is mighty-core's (the sidebar's tree and card read it): creating, running, waiting, reported, ended, interrupted,
/// merged, failed, closed, discarded.
pub use mighty_core::shell::ChildState;

delegation_names!(ChildState {
    Creating => "creating",
    Running => "running",
    Waiting => "waiting",
    Reported => "reported",
    Ended => "ended",
    Interrupted => "interrupted",
    Merged => "merged",
    Failed => "failed",
    Closed => "closed",
    Discarded => "discarded",
});

/// `DeliveryLane`: where a notice or follow-up waits — pending (not yet sent), held (sent only by run next or the human's next send)
/// or delivered (exactly once).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeliveryLane {
    Pending,
    Held,
    Delivered,
}

delegation_names!(serde DeliveryLane { Pending => "pending", Held => "held", Delivered => "delivered" });

/// `DeliveryRoute`: how an item reached its pane — steered into a running Claude pane, or queued as the next run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeliveryRoute {
    Steer,
    Queue,
}

delegation_names!(serde DeliveryRoute { Steer => "steer", Queue => "queue" });

/// `NoticeKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NoticeKind {
    Reported,
    EndedWithoutReport,
    FailedToStart,
}

delegation_names!(serde NoticeKind {
    Reported => "reported",
    EndedWithoutReport => "ended_without_report",
    FailedToStart => "failed_to_start",
});

/// `MergeKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MergeKind {
    ToolFastForward,
    CardFastForward,
    CardMergeCommit,
}

delegation_names!(serde MergeKind {
    ToolFastForward => "tool_fast_forward",
    CardFastForward => "card_fast_forward",
    CardMergeCommit => "card_merge_commit",
});

/// `MergeRecord`: written on every merge; undo reads it (M/DelegationModel.swift:189-207). On the pipe in this order (`Wire.Json`); in
/// the delegation file with its keys sorted (`DelegationJson`, [`crate::json`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeRecord {
    pub child_id: String,
    pub kind: MergeKind,
    pub parent_branch: String,
    pub pre_merge_commit: String,
    pub merged_commit: String,
    pub child_head: String,
}
