//! What the m1 router uses of the desktop (plan §0.5, §1.5): C#'s `MobileDesktopRouter` takes the concrete `DesktopService` and about
//! eighteen callbacks (R\MobileDesktopRouter.cs:1-96); here that surface is one trait, [`PhoneDesk`]. The app implements it over its
//! `RunService`, `DelegationApp`, the composer, the work map and the files pane (src-tauri `phone.rs`, W8-B), the checks headless over
//! a real `RunService` (`tests/support/desk.rs`, W8-B), A's round trip with a stub. Written whole in W8 Step 0, with the records it
//! carries (the top of MobileDesktopRouter.cs, `MobileBackgroundWork` of MobileMighty.cs:115-126). Owner: W8-B.
//!
//! The change signal (C#'s `RunEventReceived`, `ToolPermissionChanged`, `DelegationChanged`, `AntigravityCardChanged` subscriptions) is
//! not on the trait: the host calls `Router::changed` from its own change hook (src-tauri host.rs → `phone::changed`), and the router
//! reads the waiting permissions from [`PhoneDesk::pending_permissions`] instead of keeping C#'s copy of them.
//!
//! Every reply body the router writes goes through `mighty_core::json::JsonOut` (C#'s `Wire.Json`: non-ASCII as `\uXXXX`, which decides
//! the 413 threshold), never serde (plan §5 R2); the `Value`s here are written that way.

use indexmap::IndexMap;
use mighty_composer::pending::SubmitReceipt;
use mighty_core::app_update_view;
use mighty_core::attachment::RunAttachment;
use mighty_core::model::{AppSnapshot, MobileRemoteSettings, PlanRecord, RunSession, Workspace};
use mighty_core::permission::ToolPermissionRequest;
use mighty_core::shell::DelegationChildRow;
use mighty_delegation::delivery::DelegationDeliveryItem;
use mighty_files::changes::ChangeSet;
use mighty_graph::structure::mobile::Built;
use mighty_platform::BoxFuture;
use mighty_run::antigravity::permissions::PermissionRequest;
use mighty_run::model::ModelCatalog;
use mighty_run::plan_mode::PlanDecision;
use mighty_run::questionnaire::UserQuestionAnswer;
use mighty_terminal::prompt::TerminalPrompt;
use mighty_workmap::WorkMapModel;
use mighty_workmap::service::WorkMapStart;
use serde_json::Value;

/// C# `MobileImagePreview(Mime, Data, Width, Height, ThumbnailWidth, ThumbnailHeight)`: a thumbnail as base64 `data`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobileImagePreview {
    pub mime: String,
    pub data: String,
    pub width: u32,
    pub height: u32,
    pub thumbnail_width: u32,
    pub thumbnail_height: u32,
}

/// C# `MobileImageResult(Image, Reason)`: a decoded thumbnail, or why there is none (`tooLarge`, `undecodable`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobileImageResult {
    pub image: Option<MobileImagePreview>,
    pub reason: Option<String>,
}

/// C# `MobilePaneDigest(Editable, Options, Mighty)`: what a pane's revision is read from beside its record (`MobileRevisions.Reading`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MobilePaneDigest {
    pub editable: bool,
    pub options: i32,
    pub mighty: i32,
}

/// C# `MobileQueuedItem(Id, Text, RunNow?, RunNowReason?)`: one row of the phone's `queued` array; `run_now` and `run_now_reason`
/// (`QueueRunNow.Wire`) left out when the row has no button (a held row after the first).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobileQueuedItem {
    pub id: String,
    pub text: String,
    pub run_now: Option<String>,
    pub run_now_reason: Option<String>,
}

/// C# `MobileAskTarget(Id, Label)`: one choice of the phone's "send to" switch, in the desktop menu's order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobileAskTarget {
    pub id: String,
    pub label: String,
}

/// C# `MobileBackgroundWork(Id, Kind, Title, Status, StartedAt, Command?, OutputTail?)` (R\MobileMighty.cs:115-126): one background
/// task still running in a pane, a row of the background strip; its output file's path never travels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobileBackgroundWork {
    pub id: String,
    /// "agent" (a background sub-agent) or "run" (a background command).
    pub kind: String,
    pub title: String,
    /// Always "running".
    pub status: String,
    /// ISO 8601, as the CLI reported the start.
    pub started_at: String,
    pub command: Option<String>,
    /// A run's last output lines (at most 6).
    pub output_tail: Option<Vec<String>>,
}

impl MobileBackgroundWork {
    pub const MAXIMUM_TITLE: usize = 1_000;
    pub const MAXIMUM_COMMAND: usize = 2_000;
}

/// C# `MobilePaneExtras`: what a pane's detail carries beside its record. `settings`, `mighty` and `status_line` are C#'s `object?`
/// members (written through `JsonOut::value`); `ask_targets` only on a pane that offers any; `background_work` absent when none runs;
/// `background_done` sent with the strip, 0 included; `terminal_prompt` absent when none waits.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MobilePaneExtras {
    pub queued: Vec<MobileQueuedItem>,
    pub settings: Option<Value>,
    pub mighty: Option<Value>,
    pub status_line: Option<Value>,
    pub ask_targets: Option<Vec<MobileAskTarget>>,
    pub ask_target: Option<String>,
    pub background_work: Option<Vec<MobileBackgroundWork>>,
    pub background_done: Option<i32>,
    pub terminal_prompt: Option<TerminalPrompt>,
}

/// C# `MobileDesktopAction(SessionId, Action, Body, Attachments?)`: a mutating route handed to the window (WinUI `MobileAction`,
/// MainWindow.MobileRemote.cs:503-545): `create`, `submit`, `stop`, `queue-remove`, `queue-next`, `queue-run-now`, `rename`, `close`,
/// `settings`, `setAskTarget`, `guided`, `terminal-prompt`, `antigravityPermission`, `command`.
#[derive(Debug, Clone, PartialEq)]
pub struct MobileDesktopAction {
    pub session_id: String,
    pub action: String,
    pub body: Value,
    pub attachments: Option<Vec<RunAttachment>>,
}

/// What an action answered (C#'s `object?`): a submit's or guided action's receipt (`SubmitReply`: the outcome word in `accepted` and
/// `outcome`, the ids when it started at once), or any other value, written as `{protocol:1, accepted:<value>}`.
#[derive(Debug, Clone, PartialEq)]
pub enum ActionAnswer {
    Receipt(SubmitReceipt),
    Value(Value),
}

/// Why a route failed, as the router maps C#'s exceptions (plan §1.5): each is written `{protocol:1, error, code?}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MobileError {
    /// `MobileRequestException`: its own status, message and code.
    Request { status: u16, message: String, code: Option<String> },
    /// `ArgumentException`, `JsonException`, `FormatException`: 400 "Invalid mobile request.".
    Invalid,
    /// `InvalidOperationException`: 409 "The pane changed; refresh and try again.".
    Changed,
    /// `IOException`, `UnauthorizedAccessException`: 403 "The requested file is unavailable.".
    Unavailable,
    /// Anything else: the host's 500 (WinUI `MobileRoute`'s "Desktop request failed."). The text is for the log only and never
    /// carries a path's contents, a token or a key.
    Failed(String),
}

impl MobileError {
    /// `new MobileRequestException(status, message, code)`.
    pub fn request(status: u16, message: impl Into<String>, code: Option<&str>) -> MobileError {
        MobileError::Request { status, message: message.into(), code: code.map(str::to_string) }
    }
}

/// 휴대폰에서 볼 워크스페이스 (Settings › 원격 연결 › 휴대폰) from the saved choice: None lets a paired phone see every workspace of this
/// computer (the default); held to the shared ones, Some of what `shared` says is shared now (nothing while sharing is off). Every phone
/// route that lists or reads a workspace, its panes, files, work map or runs is held to it, and so are the pushes.
pub fn phone_workspace_ids(settings: Option<&MobileRemoteSettings>, shared: impl FnOnce() -> Vec<String>) -> Option<Vec<String>> {
    settings.is_some_and(|s| s.phone_shared_only).then(shared)
}

/// The desktop as the m1 router sees it (C#'s `DesktopService` members and the router's constructor callbacks).
///
/// Contract: every method may be called from any task, concurrently with the host's own calls; none blocks on the page or the host's
/// one-at-a-time call path (the long polls run on their own tasks and read [`PhoneDesk::snapshot`]). A method whose C# callback is
/// optional and unset answers its "not there" value as documented (C#: the router's behaviour without that `Func`).
pub trait PhoneDesk: Send + Sync {
    // ── the state ──

    /// `service.Snapshot`: the state as it is now (the run service's own copy).
    fn snapshot(&self) -> AppSnapshot;
    /// The pane's body read from its file if the load left it on disk, before a route reads the pane (its detail, its entries, a
    /// send): the split state reads a pane outside the active workspace only when something asks for it.
    fn ensure_body(&self, id: &str) {
        let _ = id;
    }
    /// `HostId`: the pairing identity's host id (the phone's `/m1/info`).
    fn host_id(&self) -> String;
    /// `HostName`: this computer's name as the phone shows it.
    fn host_name(&self) -> String;
    /// `AppVersion`.
    fn app_version(&self) -> String;
    /// What 이 컴퓨터 공유 shares now (the workspace ids; empty while sharing is off): all a paired phone sees when 휴대폰에서 볼
    /// 워크스페이스 says only the shared ones ([`phone_workspace_ids`]).
    fn shared_workspace_ids(&self) -> Vec<String>;
    /// `service.IsRemoteSession`: a remote workspace's pane (never the phone's).
    fn is_remote_session(&self, session_id: &str) -> bool;
    /// `service.IsSessionRunning`.
    fn is_session_running(&self, session_id: &str) -> bool;
    /// The pane's waiting tool-permission requests (C#'s `permissions` map, kept from `ToolPermissionChanged`; a remote pane's never).
    fn pending_permissions(&self, session_id: &str) -> Vec<ToolPermissionRequest>;
    /// `service.HeldItems`: what delegation holds for the pane, oldest first.
    fn held_items(&self, session_id: &str) -> Vec<DelegationDeliveryItem>;
    /// `service.DelegationChildren`: the delegated children as the sidebar lists them (`DelegationSidebar.rows`), for a child pane's
    /// `parentSessionId` and `childState`; empty without delegation.
    fn child_rows(&self) -> Vec<DelegationChildRow>;
    /// `service.AntigravityCard`: the pane's Antigravity card, if one waits.
    fn antigravity_card(&self, session_id: &str) -> Option<PermissionRequest>;
    /// `service.PendingAntigravityCard`: that card if it is still the one with these ids.
    fn pending_antigravity_card(&self, session_id: &str, request_id: &str, run_id: &str) -> Option<PermissionRequest>;

    // ── answers ──

    /// `service.RespondToToolPermission`.
    fn respond_to_tool_permission(&self, session_id: &str, request_id: &str, allow: bool) -> BoxFuture<'_, Result<(), MobileError>>;
    /// `service.AnswerQuestionnaire`.
    fn answer_questionnaire(
        &self,
        session_id: &str,
        request_id: &str,
        answers: IndexMap<String, UserQuestionAnswer>,
    ) -> BoxFuture<'_, Result<(), MobileError>>;
    /// `service.AnswerPlan`: the plan's record (None for a remote pane).
    fn answer_plan(
        &self,
        session_id: &str,
        request_id: &str,
        decision: PlanDecision,
    ) -> BoxFuture<'_, Result<Option<PlanRecord>, MobileError>>;

    // ── files ──

    /// `service.RunFolder`: a delegated child's worktree folder, else None (the workspace's folder).
    fn run_folder(&self, session_id: &str) -> Option<String>;
    /// `service.OriginalsRun`: the run whose kept originals a change is read against.
    fn originals_run(&self, session_id: &str, run_id: Option<&str>) -> Option<String>;
    /// `service.FileChangesAsync(root, relative, session, run)`: the file's changes, None when there are none to show.
    fn file_changes(
        &self,
        root: String,
        relative: String,
        session_id: Option<String>,
        run_id: Option<String>,
    ) -> BoxFuture<'_, Result<Option<ChangeSet>, MobileError>>;
    /// `ImagePreview`: an image file's thumbnail from its name and bytes (the thumbnail ladder, 2048 down to 256 px).
    fn image_preview(&self, name: String, bytes: Vec<u8>) -> BoxFuture<'_, MobileImageResult>;

    // ── the window ──

    /// `action`: a mutating route through the window's own composer (WinUI `MobileAction`).
    fn action(&self, action: MobileDesktopAction) -> BoxFuture<'_, Result<ActionAnswer, MobileError>>;
    /// `extra`: the pane's extras (queued rows from `DelegationQueueRows.MobileItems`, settings, Mighty, status line, ask targets,
    /// background work, terminal prompt).
    fn extras(&self, pane: &RunSession) -> MobilePaneExtras;
    /// `RoleExtras`: the extras with the role view in the Mighty payload (`?roles=1`); None: the plain extras.
    fn role_extras(&self, pane: &RunSession) -> Option<MobilePaneExtras>;
    /// `Structure`: the pane's structure view (`?structure=<tag>`); None for a pane with no Mighty payload.
    fn structure(&self, pane: &RunSession) -> Option<Built>;
    /// `Digest`: a pane's settings and Mighty identity for its revision; None reads it from its record alone (`Unread`).
    fn digest(&self, pane: &RunSession) -> Option<MobilePaneDigest>;
    /// `ActiveStyle`: the style a pane really runs, None for none (always None until W9's styles).
    fn active_style(&self, pane: &RunSession) -> Option<String>;
    /// `AppUpdateRead`: the outer None is C#'s unset function (no update state; `POST /m1/app-update` answers 409), the inner None a
    /// build that cannot update itself.
    fn app_update_read(&self) -> Option<Option<app_update_view::State>>;
    /// `AppUpdateAct`: `true` installs (checking and downloading first as needed), `false` only checks.
    fn app_update_act(&self, install: bool) -> BoxFuture<'_, Result<(), MobileError>>;
    /// `QueueCount`: the pane's queued requests; None: C#'s unset function.
    fn queue_count(&self, session_id: &str) -> Option<i32>;
    /// `Catalog`: the model list this PC's CLI gave for the pane's provider (the summary's `resolvedModel`); None reads the fallback.
    fn catalog(&self, pane: &RunSession) -> Option<ModelCatalog>;
    /// `WorkMapRead`: the work map of one of this PC's workspaces; None: C#'s unset function (the work map routes answer 404).
    fn work_map_read(&self, workspace: &Workspace) -> Option<BoxFuture<'_, Result<WorkMapModel, MobileError>>>;
    /// `WorkMapSummarize`: starts the topic summary of one version; None: C#'s unset function.
    fn work_map_summarize(&self, workspace: &Workspace, version: &str) -> Option<BoxFuture<'_, Result<WorkMapStart, MobileError>>>;
    /// `DirectEndpoints`: the direct path's endpoints for `/m1/info` (empty without a listener).
    fn direct_endpoints(&self) -> Vec<String>;
    /// `ExtraCapabilities`: capabilities beside `Capabilities`.
    fn extra_capabilities(&self) -> Vec<String>;
    /// Saved prompts offered in `workspace_id` (`GET /m1/sessions/{id}/prompts`); empty by default.
    fn saved_prompts(&self, workspace_id: &str) -> Vec<mighty_core::saved_prompts::SavedPrompt> {
        let _ = workspace_id;
        vec![]
    }
}
