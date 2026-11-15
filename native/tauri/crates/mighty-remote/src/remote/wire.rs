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
use mighty_core::json::{Wire, same_ignoring_case};
use mighty_core::model::{PhaseModelsSnapshot, RegisteredModelEntry, RunSettings, TodoProgress};
use mighty_run::antigravity::permissions::AllowGrant;
use mighty_run::model::{ModelCatalog, ModelOption, ModsInfo, ProviderCapabilities, ProviderRuntime, RuntimeInfo, StartRunRequest};
use mighty_run::plan_mode::{PlanDecision, outcome};
use mighty_run::questionnaire::UserQuestionAnswer;
use serde_json::{Map, Number, Value};

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

// ── RemoteWire.Json: the strict reader and the writer ───────────────────────────────────────────────────────────────────────

/// A body `RemoteWire.Json` refuses (C#'s `JsonException` while it is read): `remote.error.badBody`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BadBody;

impl std::fmt::Display for BadBody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&mighty_run::strings::get("remote.error.badBody"))
    }
}

impl std::error::Error for BadBody {}

/// A JSON value read with a key repeated in any object refused (`AllowDuplicateProperties = false`, and the event converter's touch of
/// every object inside an event).
struct Unique(Value);

impl<'de> serde::Deserialize<'de> for Unique {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(UniqueVisitor)
    }
}

struct UniqueVisitor;

impl<'de> serde::de::Visitor<'de> for UniqueVisitor {
    type Value = Unique;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("JSON")
    }
    fn visit_bool<E>(self, v: bool) -> Result<Unique, E> {
        Ok(Unique(Value::Bool(v)))
    }
    fn visit_i64<E>(self, v: i64) -> Result<Unique, E> {
        Ok(Unique(Value::Number(v.into())))
    }
    fn visit_u64<E>(self, v: u64) -> Result<Unique, E> {
        Ok(Unique(Value::Number(v.into())))
    }
    fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Unique, E> {
        Number::from_f64(v).map(|n| Unique(Value::Number(n))).ok_or_else(|| E::custom("not a finite number"))
    }
    fn visit_str<E>(self, v: &str) -> Result<Unique, E> {
        Ok(Unique(Value::String(v.to_string())))
    }
    fn visit_string<E>(self, v: String) -> Result<Unique, E> {
        Ok(Unique(Value::String(v)))
    }
    fn visit_unit<E>(self) -> Result<Unique, E> {
        Ok(Unique(Value::Null))
    }
    fn visit_none<E>(self) -> Result<Unique, E> {
        Ok(Unique(Value::Null))
    }
    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Unique, A::Error> {
        let mut items = Vec::new();
        while let Some(Unique(item)) = seq.next_element()? {
            items.push(item);
        }
        Ok(Unique(Value::Array(items)))
    }
    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<Unique, A::Error> {
        let mut object = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if object.contains_key(&key) {
                return Err(serde::de::Error::custom("a repeated member"));
            }
            let Unique(value) = map.next_value()?;
            object.insert(key, value);
        }
        Ok(Unique(Value::Object(object)))
    }
}

/// A body's top-level members as their JSON text, a member repeated at the top refused (what is inside each is read later, on its own).
pub(crate) fn parse_members(body: &[u8]) -> Result<IndexMap<String, Box<serde_json::value::RawValue>>, BadBody> {
    struct Members;
    impl<'de> serde::de::Visitor<'de> for Members {
        type Value = IndexMap<String, Box<serde_json::value::RawValue>>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("an object")
        }
        fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
            let mut members = IndexMap::new();
            while let Some(key) = map.next_key::<String>()? {
                if members.contains_key(&key) {
                    return Err(serde::de::Error::custom("a repeated member"));
                }
                members.insert(key, map.next_value()?);
            }
            Ok(members)
        }
    }
    struct Top(IndexMap<String, Box<serde_json::value::RawValue>>);
    impl<'de> serde::Deserialize<'de> for Top {
        fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            deserializer.deserialize_map(Members).map(Top)
        }
    }
    serde_json::from_slice::<Top>(body).map(|top| top.0).map_err(|_| BadBody)
}

/// The body as JSON, a key repeated anywhere refused.
pub fn parse(body: &[u8]) -> Result<Value, BadBody> {
    serde_json::from_slice::<Unique>(body).map(|u| u.0).map_err(|_| BadBody)
}

/// What a member of a record needs, as C#'s `AsTheMacReads` decides it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Need {
    /// A constructor parameter with no default and a type that is not nullable: present and not null.
    Required,
    /// Defaulted but not nullable: may be missing, never null.
    Defaulted,
    /// Nullable: may be missing or null.
    Nullable,
}

/// The shape a value must have to be read as the Mac's Codable reads it.
#[derive(Clone, Copy)]
pub(crate) enum Shape {
    Text,
    Flag,
    /// A whole number C#'s `int` holds (never from a string, never with a fraction).
    Whole,
    Record(&'static [Member]),
    /// A list without a null entry (`NoNullEntries`).
    List(&'static Shape),
    /// A dictionary without a null value.
    Dictionary(&'static Shape),
    /// Any object (a run event, a type whose own converter reads it).
    Object,
    /// Any value but null (a type whose converter reads it its own way).
    Any,
}

#[derive(Clone, Copy)]
pub(crate) struct Member(pub &'static str, pub Shape, pub Need);

use Need::{Defaulted, Nullable, Required};
use Shape::{Any, Dictionary, Flag, List, Object, Record, Text, Whole};

const TEXTS: Shape = List(&Text);

pub(crate) const MODEL_OPTION: &[Member] = &[
    Member("value", Text, Required),
    Member("displayName", Text, Required),
    Member("description", Text, Required),
    Member("resolvedModel", Text, Nullable),
    Member("supportsEffort", Flag, Nullable),
    Member("supportedEffortLevels", TEXTS, Nullable),
];
pub(crate) const MODEL_CATALOG: &[Member] = &[
    Member("source", Text, Required),
    Member("models", List(&Record(MODEL_OPTION)), Required),
    Member("detail", Text, Required),
    Member("readFailure", Text, Nullable),
    Member("listedAt", Text, Nullable),
];
pub(crate) const CAPABILITIES: &[Member] = &[
    Member("effort", Flag, Required),
    Member("permissionModes", TEXTS, Required),
    Member("maxTurns", Flag, Required),
    Member("maxBudgetUsd", Flag, Required),
    Member("resume", Flag, Required),
    Member("fastMode", Flag, Defaulted),
    Member("webSearch", Flag, Defaulted),
    Member("networkAccess", Flag, Defaulted),
    Member("attachments", Flag, Defaulted),
];
pub(crate) const PROVIDER: &[Member] = &[
    Member("id", Text, Required),
    Member("name", Text, Required),
    Member("available", Flag, Required),
    Member("version", Text, Nullable),
    Member("detail", Text, Required),
    Member("modelCatalog", Record(MODEL_CATALOG), Required),
    Member("capabilities", Record(CAPABILITIES), Required),
];
pub(crate) const MODS: &[Member] =
    &[Member("status", Text, Required), Member("minimumVersion", Text, Required), Member("detail", Text, Required)];
pub(crate) const RUNTIME: &[Member] = &[
    Member("platform", Text, Required),
    Member("appVersion", Text, Required),
    Member("claudeAvailable", Flag, Required),
    Member("claudeVersion", Text, Nullable),
    Member("modelCatalog", Record(MODEL_CATALOG), Nullable),
    // Optional on the Mac, so it may be missing or null here too.
    Member("providers", List(&Record(PROVIDER)), Nullable),
    Member("mods", Record(MODS), Nullable),
];
pub(crate) const SHARED: &[Member] = &[Member("id", Text, Required), Member("name", Text, Required), Member("path", Text, Required)];
pub(crate) const INFO: &[Member] = &[
    Member("protocol", Whole, Required),
    Member("hostId", Text, Required),
    Member("hostName", Text, Required),
    Member("workspaces", List(&Record(SHARED)), Required),
    Member("runtime", Record(RUNTIME), Required),
];
pub(crate) const ATTACHMENT: &[Member] = &[
    Member("id", Text, Required),
    Member("name", Text, Required),
    Member("mediaType", Text, Required),
    Member("dataBase64", Text, Required),
];
pub(crate) const REQUEST: &[Member] = &[
    Member("sessionId", Text, Required),
    Member("workspaceId", Text, Required),
    Member("kind", Text, Required),
    Member("input", Text, Required),
    Member("model", Text, Required),
    Member("provider", Text, Nullable),
    // RunSettingsJsonConverter reads its own members.
    Member("settings", Object, Nullable),
    Member("resumeId", Text, Nullable),
    Member("attachments", List(&Record(ATTACHMENT)), Nullable),
];
pub(crate) const WIRE_MODEL: &[Member] =
    &[Member("name", Text, Required), Member("supportsEffort", Flag, Defaulted), Member("supportedEffortLevels", TEXTS, Nullable)];
pub(crate) const TODO_ITEM: &[Member] = &[
    Member("content", Text, Required),
    Member("status", Text, Required),
    Member("activeForm", Text, Nullable),
    Member("id", Text, Nullable),
];
pub(crate) const TODOS: &[Member] = &[Member("items", List(&Record(TODO_ITEM)), Required), Member("runId", Text, Nullable)];
pub(crate) const PHASE_MODELS: &[Member] = &[
    Member("claudeMain", Text, Required),
    Member("claudeOpusAlias", Text, Required),
    Member("claudeSonnetAlias", Text, Required),
    Member("claudeHaikuAlias", Text, Required),
    Member("claudeSubagentDefault", Text, Required),
    Member("codexReviewModel", Text, Required),
    Member("codexSubagentDefault", Text, Required),
    Member("codexPlanModeReasoningEffort", Text, Required),
    Member("claudeMainEffort", Text, Nullable),
    Member("codexMainEffort", Text, Nullable),
    Member("codexSubagentEffort", Text, Nullable),
];
pub(crate) const GRANT: &[Member] =
    &[Member("requestId", Text, Required), Member("rules", TEXTS, Required), Member("temporary", Flag, Required)];
pub(crate) const START: &[Member] = &[
    Member("request", Record(REQUEST), Required),
    Member("registeredModels", List(&Record(WIRE_MODEL)), Required),
    Member("permissionModeOverride", Text, Nullable),
    Member("todoProgress", Record(TODOS), Nullable),
    Member("phaseModels", Record(PHASE_MODELS), Nullable),
    Member("antigravityGrant", Record(GRANT), Nullable),
];
pub(crate) const JOB: &[Member] = &[Member("protocol", Whole, Required), Member("jobId", Text, Required)];
pub(crate) const WIRE_EVENT: &[Member] = &[Member("cursor", Whole, Required), Member("event", Object, Required)];
pub(crate) const POLL: &[Member] = &[
    Member("protocol", Whole, Required),
    Member("cursor", Whole, Required),
    Member("lastCursor", Whole, Required),
    Member("gap", Flag, Required),
    Member("done", Flag, Required),
    Member("events", List(&Record(WIRE_EVENT)), Required),
];
pub(crate) const STEER: &[Member] = &[Member("text", Text, Required)];
pub(crate) const PERMISSION: &[Member] =
    &[Member("runId", Text, Required), Member("requestId", Text, Required), Member("allow", Flag, Required)];
pub(crate) const QUESTION_ANSWER: &[Member] = &[Member("selectedOptions", TEXTS, Required), Member("customText", Text, Nullable)];
pub(crate) const QUESTIONS: &[Member] = &[
    Member("runId", Text, Required),
    Member("requestId", Text, Required),
    Member("answers", Dictionary(&Record(QUESTION_ANSWER)), Required),
];
pub(crate) const PLAN: &[Member] = &[
    Member("runId", Text, Required),
    Member("requestId", Text, Required),
    Member("outcome", Text, Required),
    Member("feedback", Text, Nullable),
];
pub(crate) const ANSWER: &[Member] = &[Member("protocol", Whole, Required), Member("accepted", Flag, Required)];
pub(crate) const ERROR: &[Member] = &[Member("protocol", Whole, Nullable), Member("error", Text, Required)];

// The run event's records (M/Models.swift RunEvent, LogEntry; Windows' records as RemoteWire.Json reads them).
pub(crate) const LOG_ENTRY: &[Member] = &[
    Member("id", Text, Required),
    Member("kind", Text, Required),
    Member("text", Text, Required),
    Member("timestamp", Text, Required),
    Member("provider", Text, Nullable),
    // AgentActivityConverter loads a value it cannot use as none.
    Member("activity", Any, Nullable),
    Member("images", Any, Nullable),
];
pub(crate) const TOOL_REQUEST: &[Member] = &[
    Member("id", Text, Required),
    Member("runId", Text, Required),
    Member("toolUseId", Text, Required),
    Member("toolName", Text, Required),
    Member("inputJson", Text, Required),
    Member("summary", Text, Required),
    Member("reason", Text, Nullable),
    Member("blockedPath", Text, Nullable),
    Member("state", Text, Defaulted),
    Member("canAllow", Flag, Defaulted),
    Member("canAnswerQuestions", Flag, Defaulted),
    Member("canAnswerPlan", Flag, Defaulted),
    Member("receivedAt", Text, Nullable),
];
pub(crate) const PLAN_RECORD: &[Member] = &[
    Member("id", Text, Required),
    Member("runId", Text, Required),
    Member("plan", Text, Required),
    Member("receivedAt", Text, Required),
    Member("decidedAt", Text, Required),
    Member("outcome", Text, Required),
    Member("feedback", Text, Nullable),
    Member("planTruncated", Flag, Nullable),
    Member("graphRunId", Text, Nullable),
    Member("launchOverride", Text, Nullable),
];
pub(crate) const BACKGROUND_TASK: &[Member] = &[
    Member("id", Text, Required),
    Member("kind", Text, Required),
    Member("description", Text, Required),
    Member("startedAt", Text, Required),
    Member("status", Text, Defaulted),
    Member("toolUseId", Text, Nullable),
    Member("summary", Text, Nullable),
    Member("endedAt", Text, Nullable),
];
pub(crate) const BACKGROUND: &[Member] = &[Member("tasks", List(&Record(BACKGROUND_TASK)), Required), Member("turnEnded", Flag, Defaulted)];
pub(crate) const GRAPH_RUN: &[Member] = &[
    Member("id", Text, Nullable),
    Member("input", Text, Nullable),
    Member("status", Text, Nullable),
    Member("rootEntries", List(&Record(LOG_ENTRY)), Nullable),
    Member("agents", List(&Object), Nullable),
    Member("resultEntries", List(&Record(LOG_ENTRY)), Nullable),
    Member("sourceRunID", Text, Nullable),
    Member("provider", Text, Nullable),
];
pub(crate) const DENIED_ACTION: &[Member] = &[
    Member("kind", Text, Required),
    Member("action", Text, Required),
    Member("displayName", Text, Required),
    Member("target", Text, Nullable),
    Member("rule", Text, Nullable),
];
pub(crate) const CARD: &[Member] = &[
    Member("id", Text, Required),
    Member("runId", Text, Required),
    Member("actions", List(&Record(DENIED_ACTION)), Required),
    Member("receivedAt", Text, Nullable),
];
pub(crate) const RUN_EVENT: &[Member] = &[
    Member("sessionId", Text, Required),
    Member("type", Text, Required),
    Member("entry", Record(LOG_ENTRY), Nullable),
    Member("status", Text, Nullable),
    Member("resumeId", Text, Nullable),
    Member("activity", Any, Nullable),
    Member("usage", Any, Nullable),
    Member("permission", Record(TOOL_REQUEST), Nullable),
    Member("graphRun", Record(GRAPH_RUN), Nullable),
    Member("reason", Text, Nullable),
    Member("plan", Record(PLAN_RECORD), Nullable),
    Member("todos", Record(TODOS), Nullable),
    Member("background", Record(BACKGROUND), Nullable),
    Member("antigravityPermission", Record(CARD), Nullable),
];

/// Whether `value` has `shape` as the Mac's Codable reads it: exact member names (others ignored), numbers never from strings, the
/// required members present, a null only where the type is nullable and never inside a list or a dictionary.
pub(crate) fn fits(value: &Value, shape: Shape) -> bool {
    match shape {
        Text => value.is_string(),
        Flag => value.is_boolean(),
        Whole => value.as_i64().is_some_and(|n| i32::try_from(n).is_ok()),
        Object => value.is_object(),
        Any => !value.is_null(),
        List(item) => value.as_array().is_some_and(|items| items.iter().all(|i| !i.is_null() && fits(i, *item))),
        Dictionary(item) => value.as_object().is_some_and(|o| o.values().all(|i| !i.is_null() && fits(i, *item))),
        Record(members) => {
            let Some(o) = value.as_object() else { return false };
            members.iter().all(|Member(name, shape, need)| match o.get(*name) {
                None => *need != Required,
                Some(Value::Null) => *need == Nullable,
                Some(member) => fits(member, *shape),
            })
        }
    }
}

/// `value` with every key that names one of its record's members in another case left out (recursively): C# reads names exactly and
/// ignores the rest, while mighty-core's readers take the last name in any case.
pub(crate) fn exact(value: &Value, shape: Shape) -> Value {
    match (shape, value) {
        (Record(members), Value::Object(o)) => Value::Object(
            o.iter()
                .filter(|(key, _)| !members.iter().any(|Member(name, _, _)| *name != key.as_str() && same_ignoring_case(name, key)))
                .map(|(key, item)| {
                    let inner = members.iter().find(|Member(name, _, _)| *name == key.as_str());
                    (key.clone(), inner.map_or_else(|| item.clone(), |Member(_, shape, _)| exact(item, *shape)))
                })
                .collect(),
        ),
        (List(item), Value::Array(items)) => Value::Array(items.iter().map(|i| exact(i, *item)).collect()),
        (Dictionary(item), Value::Object(o)) => Value::Object(o.iter().map(|(k, i)| (k.clone(), exact(i, *item))).collect()),
        _ => value.clone(),
    }
}

/// A body parsed and checked against `members`, the case variants of their names left out.
pub(crate) fn checked(body: &[u8], members: &'static [Member]) -> Result<Value, BadBody> {
    checked_value(&parse(body)?, members)
}

pub(crate) fn checked_value(value: &Value, members: &'static [Member]) -> Result<Value, BadBody> {
    if !fits(value, Record(members)) {
        return Err(BadBody);
    }
    Ok(exact(value, Record(members)))
}

fn text(o: &Map<String, Value>, name: &str) -> String {
    o.get(name).and_then(Value::as_str).unwrap_or_default().to_string()
}
fn opt_text(o: &Map<String, Value>, name: &str) -> Option<String> {
    o.get(name).and_then(Value::as_str).map(str::to_string)
}
fn flag(o: &Map<String, Value>, name: &str) -> bool {
    o.get(name).and_then(Value::as_bool).unwrap_or(false)
}
fn whole(o: &Map<String, Value>, name: &str) -> i64 {
    o.get(name).and_then(Value::as_i64).unwrap_or_default()
}
fn texts(value: Option<&Value>) -> Vec<String> {
    value.and_then(Value::as_array).map(|items| items.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default()
}
fn object(value: &Value) -> Map<String, Value> {
    value.as_object().cloned().unwrap_or_default()
}
/// A mighty-core record read from a checked value; a reader that refuses it refuses the body.
fn record<T: Wire>(value: Option<&Value>) -> Result<Option<T>, BadBody> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(v) => T::read(v).map(Some).map_err(|_| BadBody),
    }
}

/// A value as `RemoteWire.Json` writes it: what mighty-core's `Wire.Json` writer writes, as a JSON value.
pub fn written<T: Wire>(value: &T) -> Value {
    serde_json::from_str(&value.to_json()).unwrap_or(Value::Null)
}

/// The JSON text of a value as the Mac's JSONEncoder writes it (non-ASCII and HTML characters as they are, never `\uXXXX`).
pub fn to_bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).unwrap_or_default()
}

fn put(o: &mut Map<String, Value>, name: &str, value: Option<Value>) {
    if let Some(value) = value {
        o.insert(name.into(), value);
    }
}

fn write_texts(values: &[String]) -> Value {
    Value::Array(values.iter().map(|v| Value::String(v.clone())).collect())
}

fn read_model_option(o: &Map<String, Value>) -> ModelOption {
    ModelOption {
        value: text(o, "value"),
        display_name: text(o, "displayName"),
        description: text(o, "description"),
        resolved_model: opt_text(o, "resolvedModel"),
        supports_effort: o.get("supportsEffort").and_then(Value::as_bool),
        supported_effort_levels: o.get("supportedEffortLevels").filter(|v| !v.is_null()).map(|v| texts(Some(v))),
    }
}

fn read_catalog(o: &Map<String, Value>) -> ModelCatalog {
    ModelCatalog {
        source: text(o, "source"),
        models: o
            .get("models")
            .and_then(Value::as_array)
            .map(|m| m.iter().map(|m| read_model_option(&object(m))).collect())
            .unwrap_or_default(),
        detail: text(o, "detail"),
        read_failure: opt_text(o, "readFailure"),
        listed_at: opt_text(o, "listedAt"),
    }
}

fn read_provider(o: &Map<String, Value>) -> ProviderRuntime {
    let caps = o.get("capabilities").map(object).unwrap_or_default();
    ProviderRuntime {
        id: text(o, "id"),
        name: text(o, "name"),
        available: flag(o, "available"),
        version: opt_text(o, "version"),
        detail: text(o, "detail"),
        model_catalog: read_catalog(&o.get("modelCatalog").map(object).unwrap_or_default()),
        capabilities: ProviderCapabilities {
            effort: flag(&caps, "effort"),
            permission_modes: texts(caps.get("permissionModes")),
            max_turns: flag(&caps, "maxTurns"),
            max_budget_usd: flag(&caps, "maxBudgetUsd"),
            resume: flag(&caps, "resume"),
            fast_mode: flag(&caps, "fastMode"),
            web_search: flag(&caps, "webSearch"),
            network_access: flag(&caps, "networkAccess"),
            attachments: flag(&caps, "attachments"),
        },
    }
}

/// A runtime from a value checked against [`RUNTIME`]. A missing or null provider list reads as none.
pub(crate) fn read_runtime(o: &Map<String, Value>) -> RuntimeInfo {
    RuntimeInfo {
        platform: text(o, "platform"),
        app_version: text(o, "appVersion"),
        claude_available: flag(o, "claudeAvailable"),
        claude_version: opt_text(o, "claudeVersion"),
        model_catalog: o.get("modelCatalog").and_then(Value::as_object).map(read_catalog),
        providers: o
            .get("providers")
            .and_then(Value::as_array)
            .map(|p| p.iter().map(|p| read_provider(&object(p))).collect())
            .unwrap_or_default(),
        mods: o.get("mods").and_then(Value::as_object).map(|m| ModsInfo {
            status: text(m, "status"),
            minimum_version: text(m, "minimumVersion"),
            detail: text(m, "detail"),
        }),
    }
}

fn write_catalog(catalog: &ModelCatalog) -> Value {
    let models = catalog
        .models
        .iter()
        .map(|m| {
            let mut model = Map::new();
            model.insert("value".into(), m.value.clone().into());
            model.insert("displayName".into(), m.display_name.clone().into());
            model.insert("description".into(), m.description.clone().into());
            put(&mut model, "resolvedModel", m.resolved_model.clone().map(Value::String));
            put(&mut model, "supportsEffort", m.supports_effort.map(Value::Bool));
            put(&mut model, "supportedEffortLevels", m.supported_effort_levels.as_deref().map(write_texts));
            Value::Object(model)
        })
        .collect();
    let mut o = Map::new();
    o.insert("source".into(), catalog.source.clone().into());
    o.insert("models".into(), Value::Array(models));
    o.insert("detail".into(), catalog.detail.clone().into());
    put(&mut o, "readFailure", catalog.read_failure.clone().map(Value::String));
    put(&mut o, "listedAt", catalog.listed_at.clone().map(Value::String));
    Value::Object(o)
}

fn write_provider(p: &ProviderRuntime) -> Value {
    let c = &p.capabilities;
    let mut caps = Map::new();
    caps.insert("effort".into(), c.effort.into());
    caps.insert("permissionModes".into(), write_texts(&c.permission_modes));
    caps.insert("maxTurns".into(), c.max_turns.into());
    caps.insert("maxBudgetUsd".into(), c.max_budget_usd.into());
    caps.insert("resume".into(), c.resume.into());
    caps.insert("fastMode".into(), c.fast_mode.into());
    caps.insert("webSearch".into(), c.web_search.into());
    caps.insert("networkAccess".into(), c.network_access.into());
    caps.insert("attachments".into(), c.attachments.into());
    let mut provider = Map::new();
    provider.insert("id".into(), p.id.clone().into());
    provider.insert("name".into(), p.name.clone().into());
    provider.insert("available".into(), p.available.into());
    put(&mut provider, "version", p.version.clone().map(Value::String));
    provider.insert("detail".into(), p.detail.clone().into());
    provider.insert("modelCatalog".into(), write_catalog(&p.model_catalog));
    provider.insert("capabilities".into(), Value::Object(caps));
    Value::Object(provider)
}

/// A runtime as `RemoteWire.Json` writes it (C#'s record order, nulls left out).
pub(crate) fn write_runtime(runtime: &RuntimeInfo) -> Value {
    let mut o = Map::new();
    o.insert("platform".into(), runtime.platform.clone().into());
    o.insert("appVersion".into(), runtime.app_version.clone().into());
    o.insert("claudeAvailable".into(), runtime.claude_available.into());
    put(&mut o, "claudeVersion", runtime.claude_version.clone().map(Value::String));
    put(&mut o, "modelCatalog", runtime.model_catalog.as_ref().map(write_catalog));
    o.insert("providers".into(), Value::Array(runtime.providers.iter().map(write_provider).collect()));
    put(
        &mut o,
        "mods",
        runtime.mods.as_ref().map(|m| {
            let mut mods = Map::new();
            mods.insert("status".into(), m.status.clone().into());
            mods.insert("minimumVersion".into(), m.minimum_version.clone().into());
            mods.insert("detail".into(), m.detail.clone().into());
            Value::Object(mods)
        }),
    );
    Value::Object(o)
}

impl RemoteWireInfo {
    pub fn decode(body: &[u8]) -> Result<RemoteWireInfo, BadBody> {
        RemoteWireInfo::read(&parse(body)?)
    }

    pub fn read(value: &Value) -> Result<RemoteWireInfo, BadBody> {
        let o = object(&checked_value(value, INFO)?);
        let workspaces = o.get("workspaces").and_then(Value::as_array).cloned().unwrap_or_default();
        Ok(RemoteWireInfo {
            protocol: whole(&o, "protocol") as i32,
            host_id: text(&o, "hostId"),
            host_name: text(&o, "hostName"),
            workspaces: workspaces
                .iter()
                .map(|w| {
                    let w = object(w);
                    RemoteSharedWorkspace { id: text(&w, "id"), name: text(&w, "name"), path: text(&w, "path") }
                })
                .collect(),
            runtime: read_runtime(&o.get("runtime").map(object).unwrap_or_default()),
        })
    }

    pub fn to_value(&self) -> Value {
        let workspaces = self
            .workspaces
            .iter()
            .map(|w| {
                let mut o = Map::new();
                o.insert("id".into(), w.id.clone().into());
                o.insert("name".into(), w.name.clone().into());
                o.insert("path".into(), w.path.clone().into());
                Value::Object(o)
            })
            .collect();
        let mut o = Map::new();
        o.insert("protocol".into(), self.protocol.into());
        o.insert("hostId".into(), self.host_id.clone().into());
        o.insert("hostName".into(), self.host_name.clone().into());
        o.insert("workspaces".into(), Value::Array(workspaces));
        o.insert("runtime".into(), write_runtime(&self.runtime));
        Value::Object(o)
    }
}

impl RemoteWireRequest {
    /// `RemoteWireRequest.From(request, hostWorkspaceId)`: the advisor is this computer's choice for its own Claude, never sent.
    pub fn from(request: &StartRunRequest, host_workspace_id: &str) -> RemoteWireRequest {
        let mut settings = request.settings();
        settings.advisor = None;
        RemoteWireRequest {
            session_id: request.session_id.clone(),
            workspace_id: host_workspace_id.into(),
            kind: request.kind.clone(),
            input: request.input.clone(),
            model: request.model.clone(),
            provider: request.provider.clone(),
            settings,
            resume_id: request.resume_id.clone(),
            attachments: request.attachment_list().map(<[RunAttachment]>::to_vec),
        }
    }

    fn read(o: &Map<String, Value>) -> Result<RemoteWireRequest, BadBody> {
        let settings = match o.get("settings") {
            None | Some(Value::Null) => RunSettings::default(),
            Some(v) => RunSettings::read(v).map_err(|_| BadBody)?,
        };
        let attachments = match o.get("attachments") {
            Some(Value::Array(items)) => {
                items.iter().map(|a| RunAttachment::read(a).map_err(|_| BadBody)).collect::<Result<Vec<_>, _>>()?
            }
            _ => vec![],
        };
        Ok(RemoteWireRequest {
            session_id: text(o, "sessionId"),
            workspace_id: text(o, "workspaceId"),
            kind: text(o, "kind"),
            input: text(o, "input"),
            model: text(o, "model"),
            provider: opt_text(o, "provider").unwrap_or_else(|| "claude".into()),
            settings,
            resume_id: opt_text(o, "resumeId"),
            attachments: (!attachments.is_empty()).then_some(attachments),
        })
    }

    fn to_value(&self) -> Value {
        let mut o = Map::new();
        o.insert("sessionId".into(), self.session_id.clone().into());
        o.insert("workspaceId".into(), self.workspace_id.clone().into());
        o.insert("kind".into(), self.kind.clone().into());
        o.insert("input".into(), self.input.clone().into());
        o.insert("model".into(), self.model.clone().into());
        o.insert("provider".into(), self.provider.clone().into());
        o.insert("settings".into(), written(&self.settings));
        put(&mut o, "resumeId", self.resume_id.clone().map(Value::String));
        if let Some(attachments) = self.attachments.as_ref().filter(|a| !a.is_empty()) {
            o.insert("attachments".into(), Value::Array(attachments.iter().map(written).collect()));
        }
        Value::Object(o)
    }
}

impl RemoteWireModel {
    /// `RemoteWireModel.From(entry)`.
    pub fn from(entry: &RegisteredModelEntry) -> RemoteWireModel {
        RemoteWireModel {
            name: entry.name.clone().unwrap_or_default(),
            supports_effort: entry.supports_effort,
            supported_effort_levels: entry.supported_effort_levels.iter().flatten().flatten().cloned().collect(),
        }
    }

    /// `ToEntry()`.
    pub fn to_entry(&self) -> RegisteredModelEntry {
        RegisteredModelEntry {
            name: Some(self.name.clone()),
            supports_effort: self.supports_effort,
            supported_effort_levels: Some(self.supported_effort_levels.iter().cloned().map(Some).collect()),
        }
    }

    fn to_value(&self) -> Value {
        let mut o = Map::new();
        o.insert("name".into(), self.name.clone().into());
        o.insert("supportsEffort".into(), self.supports_effort.into());
        o.insert("supportedEffortLevels".into(), write_texts(&self.supported_effort_levels));
        Value::Object(o)
    }
}

/// The most registered models a start carries (the host's own cap).
pub const MAXIMUM_START_MODELS: usize = 64;

impl RemoteWireStart {
    /// `RemoteWireStart.From(request, hostWorkspaceId)`: at most 64 models, phase models only for an agent, the grant only for Antigravity.
    pub fn from(request: &StartRunRequest, host_workspace_id: &str) -> RemoteWireStart {
        RemoteWireStart {
            request: RemoteWireRequest::from(request, host_workspace_id),
            registered_models: request.registered_models.iter().take(MAXIMUM_START_MODELS).map(RemoteWireModel::from).collect(),
            permission_mode_override: request.permission_mode_override.clone(),
            todo_progress: request.todo_progress.clone(),
            phase_models: if request.kind == "claude" { request.phase_models.clone() } else { None },
            antigravity_grant: if request.provider == "gemini" { request.antigravity_grant.clone() } else { None },
        }
    }

    /// `ToRequest(sessionId, workspaceId)`: the host's run, its own job id and workspace id; the extras validated by the caller.
    pub fn to_request(&self, session_id: &str, workspace_id: &str) -> StartRunRequest {
        let r = &self.request;
        let mut request = StartRunRequest::new(session_id, workspace_id, &r.kind, &r.input);
        request.registered_models = self.registered_models.iter().take(MAXIMUM_START_MODELS).map(RemoteWireModel::to_entry).collect();
        request.model = r.model.clone();
        request.provider = r.provider.clone();
        request.settings = Some(r.settings.clone());
        request.resume_id = r.resume_id.clone();
        request.attachments = r.attachments.clone();
        request.permission_mode_override = self.permission_mode_override.clone();
        request.todo_progress = self.todo_progress.clone();
        request.phase_models = self.phase_models.clone();
        request
    }

    pub fn decode(body: &[u8]) -> Result<RemoteWireStart, BadBody> {
        RemoteWireStart::read(&parse(body)?)
    }

    pub fn read(value: &Value) -> Result<RemoteWireStart, BadBody> {
        let o = object(&checked_value(value, START)?);
        let models = o.get("registeredModels").and_then(Value::as_array).cloned().unwrap_or_default();
        Ok(RemoteWireStart {
            request: RemoteWireRequest::read(&o.get("request").map(object).unwrap_or_default())?,
            registered_models: models
                .iter()
                .map(|m| {
                    let m = object(m);
                    RemoteWireModel {
                        name: text(&m, "name"),
                        supports_effort: flag(&m, "supportsEffort"),
                        supported_effort_levels: texts(m.get("supportedEffortLevels")),
                    }
                })
                .collect(),
            permission_mode_override: opt_text(&o, "permissionModeOverride"),
            todo_progress: record(o.get("todoProgress"))?,
            phase_models: record(o.get("phaseModels"))?,
            antigravity_grant: record(o.get("antigravityGrant"))?,
        })
    }

    pub fn to_value(&self) -> Value {
        let mut o = Map::new();
        o.insert("request".into(), self.request.to_value());
        o.insert("registeredModels".into(), Value::Array(self.registered_models.iter().map(RemoteWireModel::to_value).collect()));
        put(&mut o, "permissionModeOverride", self.permission_mode_override.clone().map(Value::String));
        put(&mut o, "todoProgress", self.todo_progress.as_ref().map(written));
        put(&mut o, "phaseModels", self.phase_models.as_ref().map(written));
        put(&mut o, "antigravityGrant", self.antigravity_grant.as_ref().map(written));
        Value::Object(o)
    }
}

impl RemoteWireJob {
    pub fn decode(body: &[u8]) -> Result<RemoteWireJob, BadBody> {
        let o = object(&checked(body, JOB)?);
        Ok(RemoteWireJob { protocol: whole(&o, "protocol") as i32, job_id: text(&o, "jobId") })
    }

    pub fn to_value(&self) -> Value {
        let mut o = Map::new();
        o.insert("protocol".into(), self.protocol.into());
        o.insert("jobId".into(), self.job_id.clone().into());
        Value::Object(o)
    }
}

impl RemoteWirePoll {
    /// The whole poll read strictly (`RemoteWire.Json`): a key repeated inside any event refuses it all. The client reads each event on
    /// its own instead.
    pub fn decode(body: &[u8]) -> Result<RemoteWirePoll, BadBody> {
        let o = object(&checked(body, POLL)?);
        let events = o.get("events").and_then(Value::as_array).cloned().unwrap_or_default();
        Ok(RemoteWirePoll {
            protocol: whole(&o, "protocol") as i32,
            cursor: whole(&o, "cursor"),
            last_cursor: whole(&o, "lastCursor"),
            gap: flag(&o, "gap"),
            done: flag(&o, "done"),
            events: events
                .iter()
                .map(|e| {
                    let e = object(e);
                    RemoteWireEvent { cursor: whole(&e, "cursor"), event: e.get("event").map(object).unwrap_or_default() }
                })
                .collect(),
        })
    }

    pub fn to_value(&self) -> Value {
        let events = self
            .events
            .iter()
            .map(|e| {
                let mut item = Map::new();
                item.insert("cursor".into(), e.cursor.into());
                item.insert("event".into(), Value::Object(e.event.clone()));
                Value::Object(item)
            })
            .collect();
        let mut o = Map::new();
        o.insert("protocol".into(), self.protocol.into());
        o.insert("cursor".into(), self.cursor.into());
        o.insert("lastCursor".into(), self.last_cursor.into());
        o.insert("gap".into(), self.gap.into());
        o.insert("done".into(), self.done.into());
        o.insert("events".into(), Value::Array(events));
        Value::Object(o)
    }
}

impl RemoteWireSteer {
    pub fn decode(body: &[u8]) -> Result<RemoteWireSteer, BadBody> {
        Ok(RemoteWireSteer { text: text(&object(&checked(body, STEER)?), "text") })
    }

    pub fn to_value(&self) -> Value {
        let mut o = Map::new();
        o.insert("text".into(), self.text.clone().into());
        Value::Object(o)
    }
}

impl RemoteWirePermission {
    pub fn decode(body: &[u8]) -> Result<RemoteWirePermission, BadBody> {
        let o = object(&checked(body, PERMISSION)?);
        Ok(RemoteWirePermission { run_id: text(&o, "runId"), request_id: text(&o, "requestId"), allow: flag(&o, "allow") })
    }

    pub fn to_value(&self) -> Value {
        let mut o = Map::new();
        o.insert("runId".into(), self.run_id.clone().into());
        o.insert("requestId".into(), self.request_id.clone().into());
        o.insert("allow".into(), self.allow.into());
        Value::Object(o)
    }
}

impl RemoteWireQuestions {
    pub fn decode(body: &[u8]) -> Result<RemoteWireQuestions, BadBody> {
        let o = object(&checked(body, QUESTIONS)?);
        let answers = o
            .get("answers")
            .map(object)
            .unwrap_or_default()
            .iter()
            .map(|(question, answer)| {
                let answer = object(answer);
                let value = UserQuestionAnswer {
                    selected_options: texts(answer.get("selectedOptions")),
                    custom_text: opt_text(&answer, "customText"),
                };
                (question.clone(), value)
            })
            .collect();
        Ok(RemoteWireQuestions { run_id: text(&o, "runId"), request_id: text(&o, "requestId"), answers })
    }

    pub fn to_value(&self) -> Value {
        let mut answers = Map::new();
        for (question, answer) in &self.answers {
            let mut a = Map::new();
            a.insert("selectedOptions".into(), write_texts(&answer.selected_options));
            put(&mut a, "customText", answer.custom_text.clone().map(Value::String));
            answers.insert(question.clone(), Value::Object(a));
        }
        let mut o = Map::new();
        o.insert("runId".into(), self.run_id.clone().into());
        o.insert("requestId".into(), self.request_id.clone().into());
        o.insert("answers".into(), Value::Object(answers));
        Value::Object(o)
    }
}

impl RemoteWirePlan {
    /// `RemoteWirePlan.From(runId, requestId, decision)`: feedback only for a revise.
    pub fn from(run_id: &str, request_id: &str, decision: &PlanDecision) -> RemoteWirePlan {
        RemoteWirePlan {
            run_id: run_id.into(),
            request_id: request_id.into(),
            outcome: decision.outcome().into(),
            feedback: if decision.kind == "revise" { decision.feedback.clone() } else { None },
        }
    }

    /// `Decision()`: the decision this outcome stands for; None for revised without feedback or an unknown outcome.
    pub fn decision(&self) -> Option<PlanDecision> {
        match self.outcome.as_str() {
            outcome::APPROVED_AUTO => Some(PlanDecision::approve_auto_edit()),
            outcome::APPROVED_CONFIRM => Some(PlanDecision::approve_confirm_each()),
            outcome::REVISED => self.feedback.as_ref().map(|f| PlanDecision::revise(f.clone())),
            outcome::CANCELLED => Some(PlanDecision::cancel()),
            _ => None,
        }
    }

    pub fn decode(body: &[u8]) -> Result<RemoteWirePlan, BadBody> {
        let o = object(&checked(body, PLAN)?);
        Ok(RemoteWirePlan {
            run_id: text(&o, "runId"),
            request_id: text(&o, "requestId"),
            outcome: text(&o, "outcome"),
            feedback: opt_text(&o, "feedback"),
        })
    }

    pub fn to_value(&self) -> Value {
        let mut o = Map::new();
        o.insert("runId".into(), self.run_id.clone().into());
        o.insert("requestId".into(), self.request_id.clone().into());
        o.insert("outcome".into(), self.outcome.clone().into());
        put(&mut o, "feedback", self.feedback.clone().map(Value::String));
        Value::Object(o)
    }
}

impl RemoteWireAnswer {
    pub fn decode(body: &[u8]) -> Result<RemoteWireAnswer, BadBody> {
        let o = object(&checked(body, ANSWER)?);
        Ok(RemoteWireAnswer { protocol: whole(&o, "protocol") as i32, accepted: flag(&o, "accepted") })
    }

    pub fn to_value(&self) -> Value {
        let mut o = Map::new();
        o.insert("protocol".into(), self.protocol.into());
        o.insert("accepted".into(), self.accepted.into());
        Value::Object(o)
    }
}

impl RemoteWireError {
    pub fn decode(body: &[u8]) -> Result<RemoteWireError, BadBody> {
        let o = object(&checked(body, ERROR)?);
        Ok(RemoteWireError { protocol: o.get("protocol").and_then(Value::as_i64).map(|p| p as i32), error: text(&o, "error") })
    }

    pub fn to_value(&self) -> Value {
        let mut o = Map::new();
        put(&mut o, "protocol", self.protocol.map(Value::from));
        o.insert("error".into(), self.error.clone().into());
        Value::Object(o)
    }
}
