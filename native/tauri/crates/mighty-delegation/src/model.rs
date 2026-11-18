//! The delegation model (C# Core/DelegationModel.cs; M/DelegationModel.swift): the child states and the only moves between them
//! (`ChildStates.After`, `ChildStateEvent`), `ChildRecord`, the lanes, routes and receipts, notices, follow-ups, merges and the request
//! key (`DelegationRequestKey.Make`), and the answer's records made from a `ChildRecord` (`DelegationChildInfo.Of`, …). Owner: W7-A.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::request::{
    DelegationChildInfo, DelegationChildStatus, DelegationChildSummary, DelegationFollowUpInfo, DelegationResponse, by_name,
    delegation_names,
};

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

/// `ChildStateEvent`: what can happen to a delegated child (M/DelegationModel.swift:4-30). [`ChildStates::after`] says which state
/// each event leads to, or refuses it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChildStateEvent {
    /// A run starts in the child's pane: its first run, or any new one (a follow-up or the human's own send).
    StartRun,
    /// The child could not be started.
    FailStart,
    /// The child's run asks for a permission that only a human can answer.
    AskPermission,
    /// That permission request was answered.
    AnswerPermission,
    /// The run ended with a changed REPORT.md.
    EndWithReport,
    /// The run finished, stopped or errored with no new report.
    EndWithoutReport,
    /// A run after a report ended with no new report and the child's branch still at its reported head, not merged since: its report
    /// still stands.
    EndUnchanged,
    /// Quitting the app killed the run.
    QuitApp,
    /// A merge took the child's reported head.
    Merge,
    /// A human undid that merge from the card.
    UndoMerge,
    /// The child's pane closed.
    ClosePane,
    /// A human discarded the child from its card.
    Discard,
}

impl ChildStateEvent {
    /// Every event, in C#'s declaration order (`Enum.GetValues<ChildStateEvent>()`).
    pub const ALL: [ChildStateEvent; 12] = [
        ChildStateEvent::StartRun,
        ChildStateEvent::FailStart,
        ChildStateEvent::AskPermission,
        ChildStateEvent::AnswerPermission,
        ChildStateEvent::EndWithReport,
        ChildStateEvent::EndWithoutReport,
        ChildStateEvent::EndUnchanged,
        ChildStateEvent::QuitApp,
        ChildStateEvent::Merge,
        ChildStateEvent::UndoMerge,
        ChildStateEvent::ClosePane,
        ChildStateEvent::Discard,
    ];
}

/// `ChildStates` (M/DelegationModel.swift:32-74). The events allow exactly these moves: creating → running (its first run starts) |
/// failed (the start failed); running ⇄ waiting (a permission request only a human can answer); running → reported (the run ended with a
/// changed REPORT.md, or with no new report and its branch still at the unmerged reported head) | ended (no new report) | interrupted
/// (quitting the app killed it); reported, ended, interrupted, merged → running (any new run in the open, idle pane, which clears
/// reported); reported → merged; merged → reported (undo while the pane is open; undo on a closed child leaves it closed); every open
/// state → closed; every state but discarded → discarded. Every other event is refused. Discarded is final.
pub trait ChildStates: Copy {
    /// `IsOpen`: open children count toward the cap — every state except failed, closed and discarded.
    fn is_open(self) -> bool;
    /// `After`: the state `change` leads to, or None when it is refused here.
    fn after(self, change: ChildStateEvent) -> Option<ChildState>;
    /// `CanMove`: whether some event moves this state to `next`, a different state.
    fn can_move(self, next: ChildState) -> bool;
}

impl ChildStates for ChildState {
    fn is_open(self) -> bool {
        !matches!(self, ChildState::Failed | ChildState::Closed | ChildState::Discarded)
    }

    fn after(self, change: ChildStateEvent) -> Option<ChildState> {
        use ChildState as S;
        use ChildStateEvent as E;
        match (self, change) {
            (S::Creating | S::Reported | S::Ended | S::Interrupted | S::Merged, E::StartRun) => Some(S::Running),
            (S::Creating, E::FailStart) => Some(S::Failed),
            (S::Running, E::AskPermission) => Some(S::Waiting),
            (S::Waiting, E::AnswerPermission) => Some(S::Running),
            (S::Running, E::EndWithReport) => Some(S::Reported),
            (S::Running, E::EndWithoutReport) => Some(S::Ended),
            (S::Running, E::EndUnchanged) => Some(S::Reported),
            (S::Running, E::QuitApp) => Some(S::Interrupted),
            (S::Reported, E::Merge) => Some(S::Merged),
            (S::Merged, E::UndoMerge) => Some(S::Reported),
            (S::Closed, E::UndoMerge) => Some(S::Closed),
            (_, E::ClosePane) if self.is_open() => Some(S::Closed),
            (_, E::Discard) if self != S::Discarded => Some(S::Discarded),
            _ => None,
        }
    }

    fn can_move(self, next: ChildState) -> bool {
        next != self && ChildStateEvent::ALL.iter().any(|change| self.after(*change) == Some(next))
    }
}

/// `ChildRecord`: one delegated child pane, as the delegation file records it (M/DelegationModel.swift:76-135). In the file with its
/// keys sorted and nulls left out ([`crate::json`]).
///
/// - `id`: the child pane's session id; `branch` always `mighty/<id>`, flat.
/// - `parent_branch`: the parent's branch when the child was made; `base_commit` its head then.
/// - `starting_mode`: the permission mode the parent asked for in delegate, written once.
/// - `report_revision`: 0 until the first report; each report revision is one higher.
/// - `parent_checkout`: the top folder of the parent's checkout when the child was made — where its cleanup and discard run git, even
///   after the parent pane closed. None in a record kept before it was stored.
/// - `closed_head`: the head of the child's branch when a human closed its merged pane, stored with its copies before any cleanup.
/// - `run_id`: the run going in the child's pane, or its last one; None before the first.
/// - `awaited_run_id`: the run that carries the parent's work (the first run, or the run a follow-up went to); its end sends one notice.
/// - `report_digest`: the SHA-256 of the whole REPORT.md at the current revision, so only a changed report makes the next one.
/// - `report_run_id`: the run whose end made the current report.
/// - `started_by_human`: the human started it from its parent's composer (only `Some(true)` is ever written by the app); it sends its
///   parent no notices.
/// - `provider`: the agent CLI the child runs; None is Claude.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChildRecord {
    pub id: String,
    pub parent_session_id: String,
    pub worktree_path: String,
    pub branch: String,
    pub parent_branch: String,
    pub base_commit: String,
    pub starting_mode: String,
    pub request_key: String,
    pub follow_up_count: i32,
    pub report_revision: i32,
    #[serde(with = "by_name")]
    pub state: ChildState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report_head: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_checkout: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub closed_head: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub awaited_run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_by_human: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report_run_id: Option<String>,
}

/// `ChildRecord.Make`'s optional arguments.
#[derive(Debug, Clone, Default)]
pub struct ChildRecordOptions {
    pub follow_up_count: i32,
    pub report_revision: i32,
    pub report_head: Option<String>,
    /// Creating when None.
    pub state: Option<ChildState>,
    pub parent_checkout: Option<String>,
    pub started_by_human: Option<bool>,
    pub provider: Option<String>,
}

impl ChildRecord {
    /// `ChildRecord.Make` with its defaults: creating, no follow-ups, no report, Claude.
    pub fn make(
        id: &str,
        parent_session_id: &str,
        worktree_path: &str,
        parent_branch: &str,
        base_commit: &str,
        starting_mode: &str,
        request_key: &str,
    ) -> ChildRecord {
        ChildRecord::make_with(
            id,
            parent_session_id,
            worktree_path,
            parent_branch,
            base_commit,
            starting_mode,
            request_key,
            ChildRecordOptions::default(),
        )
    }

    /// `ChildRecord.Make`: the branch is `mighty/<id>`; a `claude` provider is stored as None.
    #[allow(clippy::too_many_arguments)]
    pub fn make_with(
        id: &str,
        parent_session_id: &str,
        worktree_path: &str,
        parent_branch: &str,
        base_commit: &str,
        starting_mode: &str,
        request_key: &str,
        options: ChildRecordOptions,
    ) -> ChildRecord {
        ChildRecord {
            id: id.into(),
            parent_session_id: parent_session_id.into(),
            worktree_path: worktree_path.into(),
            branch: ChildRecord::branch_name(id),
            parent_branch: parent_branch.into(),
            base_commit: base_commit.into(),
            starting_mode: starting_mode.into(),
            request_key: request_key.into(),
            follow_up_count: options.follow_up_count,
            report_revision: options.report_revision,
            state: options.state.unwrap_or(ChildState::Creating),
            report_head: options.report_head,
            parent_checkout: options.parent_checkout,
            closed_head: None,
            run_id: None,
            awaited_run_id: None,
            report_digest: None,
            started_by_human: options.started_by_human,
            provider: options.provider.filter(|provider| provider != "claude"),
            report_run_id: None,
        }
    }

    /// `BranchName`: `mighty/<session id>`.
    pub fn branch_name(session_id: &str) -> String {
        format!("mighty/{session_id}")
    }

    /// `NotifiesParent`: only a child its parent's delegate call made sends its parent notices.
    pub fn notifies_parent(&self) -> bool {
        self.started_by_human != Some(true)
    }

    /// `Agent`: the agent CLI the child runs.
    pub fn agent(&self) -> &str {
        self.provider.as_deref().unwrap_or("claude")
    }

    /// `Applying`: the child moved by `change`, or None when the event is refused and nothing changes.
    pub fn applying(&self, change: ChildStateEvent) -> Option<ChildRecord> {
        self.state.after(change).map(|state| ChildRecord { state, ..self.clone() })
    }

    /// `Applied`: the child moved by `change`, or as it is when the event is refused.
    pub fn applied(&self, change: ChildStateEvent) -> ChildRecord {
        self.applying(change).unwrap_or_else(|| self.clone())
    }

    /// `RecordingReport`: the run ended with a changed REPORT.md at `head` — reported with the next revision; None, changing nothing,
    /// unless it was running.
    pub fn recording_report(&self, head: &str) -> Option<ChildRecord> {
        self.applying(ChildStateEvent::EndWithReport).map(|reported| ChildRecord {
            report_revision: self.report_revision + 1,
            report_head: Some(head.into()),
            ..reported
        })
    }
}

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

/// `DeliveryReceipt`: `time` in ISO 8601 (`yyyy-MM-ddTHH:mm:ssZ`, UTC); `run_id` the run that received the item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryReceipt {
    pub time: String,
    pub route: DeliveryRoute,
    pub run_id: String,
}

impl DeliveryReceipt {
    pub fn new(time: &str, route: DeliveryRoute, run_id: &str) -> DeliveryReceipt {
        DeliveryReceipt { time: time.into(), route, run_id: run_id.into() }
    }
}

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

/// `Notice`: a pointer to one child event for the parent (M/DelegationModel.swift:155-174). The report body is read through
/// child_status, never carried here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Notice {
    pub id: String,
    pub child_id: String,
    pub report_revision: i32,
    pub kind: NoticeKind,
    pub lane: DeliveryLane,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub receipt: Option<DeliveryReceipt>,
}

impl Notice {
    pub fn new(id: &str, child_id: &str, report_revision: i32, kind: NoticeKind, lane: DeliveryLane) -> Notice {
        Notice { id: id.into(), child_id: child_id.into(), report_revision, kind, lane, receipt: None }
    }

    pub fn with_receipt(self, receipt: DeliveryReceipt) -> Notice {
        Notice { receipt: Some(receipt), ..self }
    }
}

/// `FollowUp`: a parent's instruction to one of its children (M/DelegationModel.swift:176-187).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FollowUp {
    pub id: String,
    pub child_id: String,
    pub text: String,
    pub lane: DeliveryLane,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub receipt: Option<DeliveryReceipt>,
}

impl FollowUp {
    pub fn new(id: &str, child_id: &str, text: &str, lane: DeliveryLane) -> FollowUp {
        FollowUp { id: id.into(), child_id: child_id.into(), text: text.into(), lane, receipt: None }
    }

    pub fn with_receipt(self, receipt: DeliveryReceipt) -> FollowUp {
        FollowUp { receipt: Some(receipt), ..self }
    }
}

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

impl MergeRecord {
    pub fn new(
        child_id: &str,
        kind: MergeKind,
        parent_branch: &str,
        pre_merge_commit: &str,
        merged_commit: &str,
        child_head: &str,
    ) -> MergeRecord {
        MergeRecord {
            child_id: child_id.into(),
            kind,
            parent_branch: parent_branch.into(),
            pre_merge_commit: pre_merge_commit.into(),
            merged_commit: merged_commit.into(),
            child_head: child_head.into(),
        }
    }
}

/// `DelegationRequestKey.Make`: the idempotency key of one delegate call — SHA-256 over the calling pane's session id, its current
/// run id, the task text and the starting mode, plus the child's agent when it is not the caller's own, each part written as an 8-byte
/// big-endian UTF-8 length and its bytes; lowercase hex.
pub fn request_key(parent_session_id: &str, parent_run_id: &str, task: &str, starting_mode: &str, other_agent: Option<&str>) -> String {
    let mut hasher = Sha256::new();
    for part in [Some(parent_session_id), Some(parent_run_id), Some(task), Some(starting_mode), other_agent].into_iter().flatten() {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part.as_bytes());
    }
    hasher.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// The name the delegation tools give an agent (`DelegationProviders.AgentName`): antigravity for gemini.
fn agent_name(provider: &str) -> String {
    crate::providers::agent_name(provider).to_string()
}

impl DelegationChildInfo {
    /// `DelegationChildInfo.Of`.
    pub fn of(child: &ChildRecord) -> DelegationChildInfo {
        DelegationChildInfo {
            id: child.id.clone(),
            state: child.state,
            branch: child.branch.clone(),
            starting_mode: child.starting_mode.clone(),
            agent: agent_name(child.agent()),
        }
    }
}

impl DelegationChildSummary {
    /// `DelegationChildSummary.Of`.
    pub fn of(child: &ChildRecord) -> DelegationChildSummary {
        DelegationChildSummary {
            id: child.id.clone(),
            state: child.state,
            revision: child.report_revision,
            agent: agent_name(child.agent()),
        }
    }
}

impl DelegationChildStatus {
    /// `DelegationChildStatus.Of`: `child` with `report`, its stored REPORT.md copy, when that copy holds the current revision.
    pub fn of(child: &ChildRecord, report: Option<&crate::copy::DelegationCopy>) -> DelegationChildStatus {
        let current = report.filter(|copy| {
            copy.child_id == child.id
                && copy.kind == crate::copy::DelegationCopyKind::Report
                && copy.revision == i64::from(child.report_revision)
                && child.report_revision > 0
        });
        DelegationChildStatus {
            id: child.id.clone(),
            state: child.state,
            revision: child.report_revision,
            head: child.report_head.clone(),
            report: current.map(|copy| copy.text.clone()),
            report_truncated: current.is_some_and(|copy| copy.truncated),
            agent: agent_name(child.agent()),
        }
    }
}

impl DelegationFollowUpInfo {
    /// `DelegationFollowUpInfo.Of`.
    pub fn of(follow_up: &FollowUp, remaining: i32) -> DelegationFollowUpInfo {
        DelegationFollowUpInfo { id: follow_up.id.clone(), child: follow_up.child_id.clone(), lane: follow_up.lane, remaining }
    }
}

impl DelegationResponse {
    /// `Delegated(child)`.
    pub fn delegated(child: &ChildRecord) -> DelegationResponse {
        DelegationResponse { child: Some(DelegationChildInfo::of(child)), ..DelegationResponse::default() }
    }
}
