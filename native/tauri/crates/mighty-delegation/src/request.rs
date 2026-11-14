//! Delegation's calls and answers (C# Core/Delegation.cs; M/Delegation.swift): the reason codes, the caller, one forwarded tool call,
//! the app's answer and the records it carries, the names delegation's enums go by, and the handler a call is served by. Written whole
//! in W7 Step 0 so every part compiles against the final types; owner afterwards: W7-A.
//!
//! What stays elsewhere: the switch (`DelegationSwitch`) is src-tauri settings/general.rs's `delegation_on` (W6); the second send's
//! choice (`ComposerSecondSends.Choose`, `MenuOnly`) and the gate (`DelegationIOHandler.Gate`) read [`crate::providers`] and are
//! W7-A's there.

use indexmap::IndexMap;
use mighty_platform::BoxFuture;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use unicode_segmentation::UnicodeSegmentation;

use crate::model::{ChildState, DeliveryLane, MergeRecord};

/// The names delegation's enums carry in the delegation file and in the text an agent reads (C# `DelegationNames`): the Mac's raw
/// values, each case in snake_case (`WidthCap` is `width_cap`).
pub trait DelegationName: Sized + Copy + 'static {
    /// Every case, in declaration order.
    const ALL: &'static [Self];
    /// `DelegationNames.Of`.
    fn name(self) -> &'static str;
    /// `DelegationNames.Parse`: the case of exactly this name (ordinal), else None.
    fn parse(name: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|value| value.name() == name)
    }
}

/// Implements [`DelegationName`] (and, for a type of this crate, serde by that name: C#'s `DelegationNameConverter`).
macro_rules! delegation_names {
    ($ty:ident { $($variant:ident => $name:literal),* $(,)? }) => {
        impl $crate::request::DelegationName for $ty {
            const ALL: &'static [Self] = &[$($ty::$variant),*];
            fn name(self) -> &'static str {
                match self {
                    $($ty::$variant => $name),*
                }
            }
        }
    };
    (serde $ty:ident { $($variant:ident => $name:literal),* $(,)? }) => {
        delegation_names!($ty { $($variant => $name),* });
        impl serde::Serialize for $ty {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                $crate::request::by_name::serialize(self, serializer)
            }
        }
        impl<'de> serde::Deserialize<'de> for $ty {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                $crate::request::by_name::deserialize(deserializer)
            }
        }
    };
}
pub(crate) use delegation_names;

/// serde by [`DelegationName`] (`#[serde(with = "by_name")]` for a type of another crate, mighty-core's [`ChildState`]): a string of
/// exactly a case's name, anything else refused (C# `DelegationNameConverter` throws `JsonException`).
pub mod by_name {
    use super::*;

    pub fn serialize<T: DelegationName, S: Serializer>(value: &T, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(value.name())
    }

    pub fn deserialize<'de, T: DelegationName, D: Deserializer<'de>>(deserializer: D) -> Result<T, D::Error> {
        let name = String::deserialize(deserializer)?;
        T::parse(&name).ok_or_else(|| serde::de::Error::custom(format!("unknown name {name:?}")))
    }
}

/// `DelegationNames.Prefix`: the first `count` text elements of `text` (like a Swift prefix).
pub fn prefix(text: &str, count: usize) -> &str {
    match text.grapheme_indices(true).nth(count) {
        Some((at, _)) => &text[..at],
        None => text,
    }
}

/// `ComposerSecondSend` is mighty-composer's (the composer reads it): Worktree or Queue.
pub use mighty_composer::queue::ComposerSecondSend;

/// `ComposerSecondSends.ReturnsToComposer`: a text whose child could not be made or started goes back to the composer instead of to the
/// pane when the human picked another agent than the pane's from the worktree button's menu.
pub fn returns_to_composer(child_agent: Option<&str>, parent_agent: &str) -> bool {
    child_agent.is_some_and(|agent| agent != parent_agent)
}

/// `DelegationReasonCode`: why a delegation call was refused (M/Delegation.swift:24-52). A refused call carries exactly one and changes
/// nothing; the agent sees the snake_case name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DelegationReasonCode {
    ClaudeOnly,
    ChildCannotDelegate,
    WidthCap,
    LowDisk,
    StoreFull,
    NotGit,
    UnbornBranch,
    DetachedHead,
    WiderMode,
    DiscardHumanOnly,
    NotReported,
    TrackedChanges,
    HeadMoved,
    Diverged,
    BranchNotCheckedOut,
    ChildClosed,
    ParentClosed,
    FollowUpLimit,
    WorktreeMissing,
    MergeConflict,
    UndoParentMoved,
    ParentBusy,
    WorkspaceHasChildren,
    HeldNotRemovable,
    /// The child was started by the human from its parent's composer, not by the parent's delegate call: only the human merges it or
    /// sends it work.
    HumanStarted,
    /// A parent whose agent's merge tool call its CLI does not gate (Codex) merges by tool only in fullAccess; otherwise the human
    /// merges from the child's card.
    MergeHumanOnly,
    /// delegate's `agent` names no agent a child can run.
    AgentUnknown,
    /// The agent delegate named is not installed or not signed in on this PC, as the app last learnt it.
    AgentUnavailable,
}

delegation_names!(serde DelegationReasonCode {
    ClaudeOnly => "claude_only",
    ChildCannotDelegate => "child_cannot_delegate",
    WidthCap => "width_cap",
    LowDisk => "low_disk",
    StoreFull => "store_full",
    NotGit => "not_git",
    UnbornBranch => "unborn_branch",
    DetachedHead => "detached_head",
    WiderMode => "wider_mode",
    DiscardHumanOnly => "discard_human_only",
    NotReported => "not_reported",
    TrackedChanges => "tracked_changes",
    HeadMoved => "head_moved",
    Diverged => "diverged",
    BranchNotCheckedOut => "branch_not_checked_out",
    ChildClosed => "child_closed",
    ParentClosed => "parent_closed",
    FollowUpLimit => "follow_up_limit",
    WorktreeMissing => "worktree_missing",
    MergeConflict => "merge_conflict",
    UndoParentMoved => "undo_parent_moved",
    ParentBusy => "parent_busy",
    WorkspaceHasChildren => "workspace_has_children",
    HeldNotRemovable => "held_not_removable",
    HumanStarted => "human_started",
    MergeHumanOnly => "merge_human_only",
    AgentUnknown => "agent_unknown",
    AgentUnavailable => "agent_unavailable",
});

/// `DelegationCaller`: the pane a delegation call came from, as its run's pane token resolved it (M/AgentPaneMCPBinding.swift:20-41).
/// `delegation`: the run was started with the delegation server attached (a Claude pane's run, or a Codex run in acceptEdits or
/// fullAccess, started while the switch was on). `permission_mode`: the mode the run started in — what a call is judged by, whatever
/// the pane's stored mode becomes meanwhile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelegationCaller {
    pub pane_id: String,
    pub kind: String,
    pub provider: String,
    pub delegation: bool,
    pub permission_mode: String,
}

impl DelegationCaller {
    /// C#'s defaults: kind and provider `claude`, not attached, no mode.
    pub fn new(pane_id: impl Into<String>) -> DelegationCaller {
        DelegationCaller {
            pane_id: pane_id.into(),
            kind: "claude".into(),
            provider: "claude".into(),
            delegation: false,
            permission_mode: String::new(),
        }
    }
}

/// `DelegationRequest`: one delegation tool call forwarded by the delegation MCP server (M/Delegation.swift:54-70). It carries no
/// pane id: the caller is whatever pane the token resolves to. Every argument is a string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DelegationRequest {
    pub tool: String,
    pub arguments: IndexMap<String, String>,
}

impl DelegationRequest {
    /// A call with no arguments.
    pub fn new(tool: impl Into<String>) -> DelegationRequest {
        DelegationRequest { tool: tool.into(), arguments: IndexMap::new() }
    }

    /// `IsToolName`: lowercase letters, digits and underscores, starting with a letter, at most 64 (`^[a-z][a-z0-9_]{0,63}$`).
    pub fn is_tool_name(name: &str) -> bool {
        let bytes = name.as_bytes();
        (1..=64).contains(&bytes.len())
            && bytes[0].is_ascii_lowercase()
            && bytes.iter().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'_')
    }
}

/// `DelegationChildInfo`: one child as its parent's tools show it (M/Delegation.swift). The branch is always `mighty/<id>`; `agent` is
/// the agent the child runs as delegate's `agent` names it (`DelegationProviders.AgentName`: gemini is `antigravity`). W7-A adds
/// `of(&ChildRecord)` beside the record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DelegationChildInfo {
    pub id: String,
    #[serde(with = "by_name")]
    pub state: ChildState,
    pub branch: String,
    pub starting_mode: String,
    pub agent: String,
}

/// `DelegationChildSummary`: one child as list_children shows it (DelegationStatusTools.cs:6). `revision`: the current report
/// revision, 0 until the first report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DelegationChildSummary {
    pub id: String,
    #[serde(with = "by_name")]
    pub state: ChildState,
    pub revision: i32,
    pub agent: String,
}

/// `DelegationChildStatus`: one child as child_status shows it (DelegationStatusTools.cs:17). `head`: the head the current report was
/// made at (merge's `expected_head`), None until the first report. `report`: the stored copy of the current revision's REPORT.md, cut at
/// 64 KiB with a visible marker; None until the first report or once pruning dropped it. `report_truncated`: the report is longer than
/// its copy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DelegationChildStatus {
    pub id: String,
    #[serde(with = "by_name")]
    pub state: ChildState,
    pub revision: i32,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub head: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub report: Option<String>,
    pub report_truncated: bool,
    pub agent: String,
}

/// `DelegationFollowUpInfo`: one follow-up as follow_up answers it (DelegationActionTools.cs:8). `lane`: where delivery has it as the
/// call is answered; `remaining`: how many more follow-ups the child may still get.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DelegationFollowUpInfo {
    pub id: String,
    pub child: String,
    pub lane: DeliveryLane,
    pub remaining: i32,
}

/// `DelegationResponse`: the app's answer to one delegation call (M/Delegation.swift:72-101). One member is set, except that a refusal
/// with a detail sets both `refused` and `error` ([`DelegationResponse::refusal_with`]). On the wire (`Wire.Json`): camelCase, nulls
/// omitted, in this order.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DelegationResponse {
    /// Refused with exactly this reason; nothing changed.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub refused: Option<DelegationReasonCode>,
    /// The call reached no tool, or the tool could not take it as given; with `refused`, what the refusal is about.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub error: Option<String>,
    /// The child delegate just made (or the same one again when that call is repeated in the same parent run).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub child: Option<DelegationChildInfo>,
    /// list_children's answer: the caller's children, possibly none.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub children: Option<Vec<DelegationChildSummary>>,
    /// child_status's answer.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub status: Option<DelegationChildStatus>,
    /// merge's answer: the fast-forward it made, as the MergeRecord it wrote.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub merged: Option<MergeRecord>,
    /// follow_up's answer.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub follow_up: Option<DelegationFollowUpInfo>,
}

impl DelegationResponse {
    /// `Refusal(reason)`.
    pub fn refusal(reason: DelegationReasonCode) -> DelegationResponse {
        DelegationResponse { refused: Some(reason), ..DelegationResponse::default() }
    }

    /// `Refusal(reason, detail)`.
    pub fn refusal_with(reason: DelegationReasonCode, detail: impl Into<String>) -> DelegationResponse {
        DelegationResponse { refused: Some(reason), error: Some(detail.into()), ..DelegationResponse::default() }
    }

    /// `Failure(message)`.
    pub fn failure(message: impl Into<String>) -> DelegationResponse {
        DelegationResponse { error: Some(message.into()), ..DelegationResponse::default() }
    }
}

/// `DelegationIOHandler.DetachedMessage`.
pub const DETACHED_MESSAGE: &str = "The delegation tools are not attached to this pane's run.";

/// `DelegationIOHandler.UnknownToolMessage`.
pub fn unknown_tool_message(tool: &str) -> String {
    format!("Unknown delegation tool {}.", prefix(tool, 80))
}

/// `IDelegationRequestHandler`: serves one delegation call, already resolved to the pane its token belongs to (M/Delegation.swift:
/// 115-118). The coordinator (W7-A) is the app's; the pipe's server (W7-C, `agent_io::server`) calls it within its 60 s. The answer is
/// `'static` so the server can race it against its deadline and drop it on revocation.
pub trait DelegationHandler: Send + Sync {
    fn handle(&self, request: DelegationRequest, caller: DelegationCaller) -> BoxFuture<'static, DelegationResponse>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::MergeKind;

    /// Every reason code by its snake_case name, both ways, and nothing else (C#'s 28).
    #[test]
    fn the_reason_codes_go_by_their_snake_case_names() {
        assert_eq!(DelegationReasonCode::ALL.len(), 28);
        for code in DelegationReasonCode::ALL {
            assert_eq!(DelegationReasonCode::parse(code.name()), Some(*code));
            assert!(code.name().bytes().all(|b| b.is_ascii_lowercase() || b == b'_'), "{}", code.name());
        }
        assert_eq!(DelegationReasonCode::WidthCap.name(), "width_cap");
        assert_eq!(DelegationReasonCode::parse("WidthCap"), None);
    }

    /// `IsToolName`.
    #[test]
    fn a_tool_name_is_lowercase_and_at_most_64() {
        for good in ["delegate", "list_children", "a", "a0_", &"a".repeat(64)] {
            assert!(DelegationRequest::is_tool_name(good), "{good}");
        }
        for bad in ["", "0a", "_a", "Delegate", "a-b", "a b", "é", &"a".repeat(65)] {
            assert!(!DelegationRequest::is_tool_name(bad), "{bad}");
        }
    }

    /// The answer as the wire writes it: camelCase, nulls left out, enums by name, C#'s member order.
    #[test]
    fn an_answer_is_written_as_the_wire_writes_it() {
        let refused = DelegationResponse::refusal_with(DelegationReasonCode::AgentUnavailable, "codex");
        assert_eq!(serde_json::to_string(&refused).unwrap(), r#"{"refused":"agent_unavailable","error":"codex"}"#);
        let status = DelegationResponse {
            status: Some(DelegationChildStatus {
                id: "c".into(),
                state: ChildState::Reported,
                revision: 1,
                head: None,
                report: Some("r".into()),
                report_truncated: false,
                agent: "claude".into(),
            }),
            ..DelegationResponse::default()
        };
        assert_eq!(
            serde_json::to_string(&status).unwrap(),
            r#"{"status":{"id":"c","state":"reported","revision":1,"report":"r","reportTruncated":false,"agent":"claude"}}"#
        );
        let merged = DelegationResponse {
            merged: Some(MergeRecord {
                child_id: "c".into(),
                kind: MergeKind::ToolFastForward,
                parent_branch: "main".into(),
                pre_merge_commit: "a".into(),
                merged_commit: "b".into(),
                child_head: "b".into(),
            }),
            follow_up: None,
            ..DelegationResponse::default()
        };
        assert_eq!(
            serde_json::to_string(&merged).unwrap(),
            r#"{"merged":{"childId":"c","kind":"tool_fast_forward","parentBranch":"main","preMergeCommit":"a","mergedCommit":"b","childHead":"b"}}"#
        );
        let read: DelegationRequest = serde_json::from_str(r#"{"tool":"merge","arguments":{"child":"c","expected_head":"b"}}"#).unwrap();
        assert_eq!(read.arguments.keys().collect::<Vec<_>>(), ["child", "expected_head"]);
    }

    /// `DelegationNames.Prefix` counts text elements, and the unknown tool's message cuts at 80 of them.
    #[test]
    fn prefixes_count_text_elements() {
        assert_eq!(prefix("ab", 5), "ab");
        assert_eq!(prefix("e\u{301}x", 1), "e\u{301}");
        assert_eq!(unknown_tool_message(&"x".repeat(90)), format!("Unknown delegation tool {}.", "x".repeat(80)));
        assert!(returns_to_composer(Some("codex"), "claude") && !returns_to_composer(Some("claude"), "claude"));
        assert!(!returns_to_composer(None, "claude"));
    }
}
