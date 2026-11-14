//! Remote workspaces v2's records (C# R\RemoteWorkspaceModels.cs, the Mac's M/Remote/RemoteWorkspaceModels.swift; pinned by
//! native/contracts/fixtures/remote-v2/; plan §1.6): the protocol's constants, what the host shares and the client saves, and every
//! body of the wire. Written whole in W8 Step 0 so the host, the client and the app compile against final types. Owner: W8-D.
//!
//! What D adds here: the reading and writing of each body with C#'s `RemoteWire.Json` (camelCase, nulls left out, text as the Mac's
//! JSONEncoder writes it — non-ASCII as it is —; read as the Mac's Codable reads: exact member names, numbers never from strings, a
//! member required where the Mac requires it, a null refused where the type is not optional and inside every list and dictionary, a
//! repeated member refused, every object of an event touched once), the conversions `RemoteWireRequest::from`, `RemoteWireStart::from`
//! and `to_request` (the advisor dropped, 64 models at most, phase models only for `claude`, the grant only for `gemini`),
//! `RemoteWireModel::from`/`to_entry`, `RemoteWirePlan::from`/`decision`, and `RemoteWorkspacePhone`'s filters.

use indexmap::IndexMap;
use mighty_core::attachment::RunAttachment;
use mighty_core::model::{PhaseModelsSnapshot, RunSettings, TodoProgress};
use mighty_run::antigravity::permissions::AllowGrant;
use mighty_run::model::RuntimeInfo;
use mighty_run::questionnaire::UserQuestionAnswer;
use serde_json::{Map, Value};

/// C# `RemoteProtocol`.
pub mod protocol {
    use std::time::Duration;

    /// Sent and required as `x-mighty-remote-version` (the removed v1 spoke under `/v1/`).
    pub const VERSION: i32 = 2;
    pub const HEADER: &str = "x-mighty-remote-version";
    pub const PATH_PREFIX: &str = "/v2/";
    pub const DEFAULT_PORT: u16 = 43137;
    /// A run the client has not polled for this long is stopped on the host.
    pub const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(30);
    /// A client asks again this long after a poll failed for a passing reason before it calls the connection lost.
    pub const POLL_RETRY_WINDOW: Duration = Duration::from_secs(10);
    pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
    pub const UPLOAD_TIMEOUT: Duration = Duration::from_secs(60);
    pub const MAXIMUM_SHARED_WORKSPACES: usize = 64;
    pub const MAXIMUM_RUNS: usize = 16;
    pub const MAXIMUM_CONNECTIONS_PER_PEER: usize = 8;
    pub const MAXIMUM_PENDING_PERMISSIONS: usize = 32;
    pub const MAXIMUM_CONNECTIONS: usize = 16;
    pub const MAXIMUM_REQUEST_BYTES: usize = 512 * 1024;
    pub const MAXIMUM_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
    pub const MAXIMUM_POLL_BYTES: usize = 1_536 * 1024;
    pub const MAXIMUM_POLL_EVENTS: usize = 256;
    pub const MAXIMUM_BUFFERED_BYTES: usize = 4 * 1024 * 1024;
    pub const MAXIMUM_BUFFERED_EVENTS: usize = 4096;
    /// A Windows client asks a Windows host for `graph_run` events instead of graph nodes; a Mac host ignores it.
    pub const GRAPH_HEADER: &str = "x-mighty-remote-graph";
    pub const GRAPH_RUNS: &str = "run";
}

/// C# `RemoteSharedWorkspace(Id, Name, Path)`: a workspace the host shares, as the client lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteSharedWorkspace {
    pub id: String,
    pub name: String,
    pub path: String,
}

/// C# `RemoteHostState`: what "이 컴퓨터 공유" shows. Sharing is always off when the app starts.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct RemoteHostState {
    pub enabled: bool,
    /// Where clients reach this computer, `address:port`, one per Tailscale address.
    pub addresses: Vec<String>,
    pub port: Option<u16>,
    /// The connection key while sharing; stopping revokes it for good. Never serialized, logged or in `Debug`.
    pub key: Option<String>,
    pub workspace_ids: Vec<String>,
    pub active_runs: usize,
    /// "원격에서 전체 권한·명령 실행 허용": off every time sharing starts.
    pub allows_full_access: bool,
}

/// C#'s `PrintMembers` without the key.
impl std::fmt::Debug for RemoteHostState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteHostState")
            .field("enabled", &self.enabled)
            .field("addresses", &self.addresses)
            .field("port", &self.port)
            .field("key", &self.key.as_ref().map(|_| "<redacted>"))
            .field("workspace_ids", &self.workspace_ids)
            .field("active_runs", &self.active_runs)
            .field("allows_full_access", &self.allows_full_access)
            .finish()
    }
}

/// C# `RemoteConnectionStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RemoteConnectionStatus {
    #[default]
    Saved,
    Connected,
    Disconnected,
}

/// C# `RemoteConnectionInfo(Id, Name, Address, Status, HostName?, Workspaces?, Runtime?, Detail?)`: one saved "다른 컴퓨터 연결" entry.
/// Its key is in the vault (`SecretVault`), never here.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RemoteConnectionInfo {
    pub id: String,
    pub name: String,
    pub address: String,
    pub status: RemoteConnectionStatus,
    pub host_name: Option<String>,
    pub workspaces: Option<Vec<RemoteSharedWorkspace>>,
    pub runtime: Option<RuntimeInfo>,
    pub detail: Option<String>,
}

impl RemoteConnectionInfo {
    /// C# `Shared`: the host's workspaces, none before its first answer.
    pub fn shared(&self) -> &[RemoteSharedWorkspace] {
        self.workspaces.as_deref().unwrap_or(&[])
    }
}

/// C# `RemoteWireInfo(Protocol, HostId, HostName, Workspaces, Runtime)`: `GET /v2/info`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteWireInfo {
    pub protocol: i32,
    pub host_id: String,
    pub host_name: String,
    pub workspaces: Vec<RemoteSharedWorkspace>,
    pub runtime: RuntimeInfo,
}

/// C# `RemoteWireRequest`: a pane's request as the Mac's `StartRunRequest` encodes it, never `registeredModels` or anything local. As
/// the Mac's decoder, a missing or null provider is `claude`, missing settings are the defaults, and an empty attachment list is none
/// (and is not written).
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteWireRequest {
    pub session_id: String,
    pub workspace_id: String,
    pub kind: String,
    pub input: String,
    pub model: String,
    pub provider: String,
    pub settings: RunSettings,
    pub resume_id: Option<String>,
    pub attachments: Option<Vec<RunAttachment>>,
}

/// C# `RemoteWireModel(Name, SupportsEffort, SupportedEffortLevels)`: `RegisteredModelEntry` as the Mac writes it
/// (`supportedEffortLevels` always present) and reads it (both flags optional).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteWireModel {
    pub name: String,
    pub supports_effort: bool,
    pub supported_effort_levels: Vec<String>,
}

/// C# `RemoteWireStart(Request, RegisteredModels, PermissionModeOverride?, TodoProgress?, PhaseModels?, AntigravityGrant?)`: `POST
/// /v2/runs`. `antigravity_grant`: an Antigravity continuation's rules of a card the host reported (the host refuses any other).
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteWireStart {
    pub request: RemoteWireRequest,
    pub registered_models: Vec<RemoteWireModel>,
    pub permission_mode_override: Option<String>,
    pub todo_progress: Option<TodoProgress>,
    pub phase_models: Option<PhaseModelsSnapshot>,
    pub antigravity_grant: Option<AllowGrant>,
}

/// C# `RemoteWireJob(Protocol, JobId)`: the 202 of a start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteWireJob {
    pub protocol: i32,
    pub job_id: String,
}

/// C# `RemoteWireEvent(Cursor, Event)`: one run event as one JSON object (`RemoteWire.EventConverter`: keys matched exactly, every
/// object inside read once so a repeated key anywhere is refused while the poll is read).
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteWireEvent {
    pub cursor: i64,
    pub event: Map<String, Value>,
}

/// C# `RemoteWirePoll(Protocol, Cursor, LastCursor, Gap, Done, Events)`: `GET /v2/runs/{id}/events?cursor=N`. `cursor`: the last
/// cursor in `events` (the request's when none); `last_cursor`: the run's newest (more waits while it is past `cursor`); `gap`: events
/// after the request's cursor were dropped from the host's buffer.
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteWirePoll {
    pub protocol: i32,
    pub cursor: i64,
    pub last_cursor: i64,
    pub gap: bool,
    pub done: bool,
    pub events: Vec<RemoteWireEvent>,
}

/// C# `RemoteWireSteer(Text)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteWireSteer {
    pub text: String,
}

/// C# `RemoteWirePermission(RunId, RequestId, Allow)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteWirePermission {
    pub run_id: String,
    pub request_id: String,
    pub allow: bool,
}

/// C# `RemoteWireQuestions(RunId, RequestId, Answers)`.
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteWireQuestions {
    pub run_id: String,
    pub request_id: String,
    pub answers: IndexMap<String, UserQuestionAnswer>,
}

/// C# `RemoteWirePlan(RunId, RequestId, Outcome, Feedback?)`: `feedback` only for a revise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteWirePlan {
    pub run_id: String,
    pub request_id: String,
    pub outcome: String,
    pub feedback: Option<String>,
}

/// C# `RemoteWireAnswer(Protocol, Accepted)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteWireAnswer {
    pub protocol: i32,
    pub accepted: bool,
}

/// C# `RemoteWireError(Protocol?, Error)`: every route's error carries `protocol`; the HTTP layer under the routes (a malformed head
/// 400, a body too large 413, headers too large 431) answers `{error}` alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteWireError {
    pub protocol: Option<i32>,
    pub error: String,
}

/// C# `RemoteHttpFailure(Status, Message)`: the host answered with an error status; its message is what the user reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteHttpFailure {
    pub status: u16,
    pub message: String,
}

impl RemoteHttpFailure {
    /// Busy for a moment (a rate limit, a local update holding runs): worth asking again.
    pub fn transient(&self) -> bool {
        matches!(self.status, 429 | 503)
    }
}

impl std::fmt::Display for RemoteHttpFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for RemoteHttpFailure {}
