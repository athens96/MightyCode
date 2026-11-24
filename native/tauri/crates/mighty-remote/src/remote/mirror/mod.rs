//! The remote mirror (C# `Core\Remote\RemoteMirror.cs`, `RemoteMirrorDesktopHost.cs`, `RemoteHostService.Mirror.cs`,
//! `RemoteClientService.Mirror.cs`, `Core\DesktopService.Mirror.cs`, from `main` 0.4.61; M/MightyCore/Remote/RemoteMirror.swift;
//! docs/remote-workspaces.md "그 컴퓨터의 창 그대로"; shared fixtures `native/contracts/fixtures/remote-mirror/`; plan §3 W9-J). Owner:
//! W9-J.
//!
//! A client connected over Tailscale sees the host's shared workspaces with the agent panes already open there, and keeps using them from
//! here. The panes stay the host's: a send from the client is the host pane's next request, as the phone's is. The wire is versioned on
//! its own ([`protocol::VERSION`]) inside remote protocol 2, and a Mac, a Windows (C#) and this app read each other's answers.
//!
//! - this module: the protocol's bounds, the records, the wire's bodies (read as strictly as the Mac reads them) and the routes;
//! - [`support`]: the rules both sides read (client ids, revisions, the bounded detail, the Mac's session JSON, the client's checks, the
//!   client pane laid over from the host's);
//! - [`host`]: `/v2/mirror` on the sharing host ([`host::MirrorHost`], what the app hands over), behind the /v2/ checks;
//! - [`client`]: the client's requests on a connected connection;
//! - [`desk`]: the client side in the app (C# `DesktopService.Mirror`): one poll per connection, the mirrored workspaces and panes in the
//!   state (never saved), sends, stops and answers going to the host;
//! - [`desktop_host`]: the host side in the app (C# `RemoteMirrorDesktopHost`): this PC's own panes, read as they are and driven through
//!   the phone's m1 handlers.
//!
//! Keys travel only in `Authorization` (the transport's rule); nothing here logs a key, a path's contents or a host's transcript.

pub mod client;
pub mod desk;
pub mod desktop_host;
pub mod host;
pub mod support;

use indexmap::IndexMap;
use mighty_core::attachment::RunAttachment;
use mighty_core::graph::MightyGraphRun;
use mighty_core::json::Wire;
use mighty_core::model::{BackgroundWork, LogEntry, RunSettings, TodoProgress};
use mighty_core::permission::ToolPermissionRequest;
use mighty_core::wire;
use mighty_run::questionnaire::UserQuestionAnswer;
use serde_json::{Map, Value};

use crate::remote::wire::{ATTACHMENT, BadBody, Member, Need, QUESTION_ANSWER, SHARED, Shape, checked};
use crate::remote::wire::{RemoteSharedWorkspace, protocol as remote_protocol};

/// C# `RemoteMirrorProtocol`.
pub mod protocol {
    use std::time::Duration;

    /// Sent as `mirror` in every mirror answer; a client refuses another.
    pub const VERSION: i32 = 1;
    /// The newest transcript entries a pane's detail carries (the phone's page, `MobileWire.MaximumEntries`).
    pub const MAXIMUM_ENTRIES: usize = 80;
    /// The newest graph runs a pane's detail carries (the phone's, `MobileWire.MightyRuns`).
    pub const MAXIMUM_GRAPH_RUNS: usize = 20;
    /// One detail's session JSON stays under this many bytes (a client reads at most 2 MiB).
    pub const MAXIMUM_DETAIL_BYTES: usize = crate::remote::wire::protocol::MAXIMUM_POLL_BYTES;
    /// Panes listed per host.
    pub const MAXIMUM_SESSIONS: usize = 128;
    /// Queued requests listed per pane, each cut to [`MAXIMUM_QUEUED_TEXT`] characters.
    pub const MAXIMUM_QUEUED: usize = 16;
    pub const MAXIMUM_QUEUED_TEXT: usize = 500;
    /// MC-49: what 바로 실행 does on a queued row, as the host decided it (the page's names, `QueueRunNowPath::name`); any other is
    /// read as none (no button).
    pub const RUN_NOW: [&str; 8] = ["worktree", "agentMenu", "here", "steer", "held", "waitsForTurn", "waitsForHeld", "waitsForUpdate"];
    /// MC-49: the actions on a host pane's queued requests a host may take (`queueActionNames`): × (`remove`), 바로 실행 (`runNow`),
    /// the words changed (`edit`) and the order (`move`). A host that sends only `queueActions: true` takes the first two.
    pub const QUEUE_ACTIONS: [&str; 4] = ["remove", "runNow", "edit", "move"];
    /// Pending tool requests a pane's detail carries.
    pub const MAXIMUM_PERMISSIONS: usize = crate::remote::wire::protocol::MAXIMUM_PENDING_PERMISSIONS;
    /// Details a client reads in one pass of its poll.
    pub const MAXIMUM_DETAILS_PER_PASS: usize = 16;
    /// A sent request's text, in UTF-8 bytes.
    pub const MAXIMUM_TEXT: usize = 400_000;
    /// How often a client reads the host's list: while one of its panes runs or waits or the user looks at one, and otherwise.
    pub const LIVE_POLL_INTERVAL: Duration = Duration::from_secs(1);
    pub const IDLE_POLL_INTERVAL: Duration = Duration::from_secs(5);
    /// The status words a pane travels with; any other is sent as `idle`.
    pub const STATUSES: [&str; 5] = ["idle", "running", "completed", "error", "stopped"];
    /// The Mac's 30 s for a send without attachments.
    pub const SEND_TIMEOUT: Duration = Duration::from_secs(30);
}

/// C# `RemoteMirrorSummary`: one host pane in the list (`GET /v2/mirror`), enough for the sidebar, and a revision that moves whenever its
/// detail changed. `pending`: tool, question and plan requests waiting for an answer, and an Antigravity card; `branch`: the pane's own
/// worktree branch, when it works in one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirrorSummary {
    pub id: String,
    pub workspace_id: String,
    pub title: String,
    pub kind: String,
    pub provider: String,
    pub model: String,
    pub status: String,
    pub revision: String,
    pub pending: i64,
    pub queued: i64,
    pub branch: Option<String>,
}

/// C# `RemoteMirrorQueued`: a request waiting in a host pane's queue. `run_now` (MC-49, `runNow` on the wire, left out by older hosts and
/// for a row without the button): what 바로 실행 does on it there, one of [`protocol::RUN_NOW`]. `cut` (MC-49, `cut: true`): the text is
/// cut to [`protocol::MAXIMUM_QUEUED_TEXT`], so it is not the whole request and is never edited from a client.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MirrorQueued {
    pub id: String,
    pub text: String,
    pub run_now: Option<String>,
    pub cut: bool,
}

/// C# `RemoteMirrorSession`: one host pane in full (`GET /v2/mirror/sessions/{id}`), in this app's types. On the wire it is the Mac's
/// JSON ([`support::write`], [`support::read`]). `logs`: the newest [`protocol::MAXIMUM_ENTRIES`], pictures left out; `has_older`: the host
/// holds older entries; `started_at`/`finished_at`: the current request's clock, ISO 8601 in whole seconds.
#[derive(Debug, Clone, PartialEq)]
pub struct MirrorSession {
    pub id: String,
    pub workspace_id: String,
    pub title: String,
    pub kind: String,
    pub provider: String,
    pub model: String,
    pub status: String,
    pub created_at: String,
    pub settings: RunSettings,
    pub logs: Vec<LogEntry>,
    pub has_older: bool,
    pub graph_runs: Vec<MightyGraphRun>,
    pub permissions: Vec<ToolPermissionRequest>,
    pub queued: Vec<MirrorQueued>,
    pub title_mode: Option<String>,
    pub agent_view_mode: Option<String>,
    pub mighty_style: Option<String>,
    pub todo_progress: Option<TodoProgress>,
    pub background_work: Option<BackgroundWork>,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub branch: Option<String>,
    pub base: Option<String>,
    /// MC-49: the queue actions the host takes on this pane, of [`protocol::QUEUE_ACTIONS`] (`queueActionNames`; `queueActions: true`
    /// as well when it takes `remove` and `runNow`, which is all an earlier MC-49 host sends). An older host takes none, and its queue
    /// is shown without them.
    pub queue_actions: Vec<String>,
}

/// C# `RemoteMirrorState`: the host's list as the client reads it; `unchanged`: the client's `since` was still current, and the lists
/// mean nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirrorState {
    pub host_name: String,
    pub revision: String,
    pub unchanged: bool,
    pub workspaces: Vec<RemoteSharedWorkspace>,
    pub sessions: Vec<MirrorSummary>,
}

/// C# `RemoteMirrorPane`: a pane's detail and the revision it was read at.
#[derive(Debug, Clone, PartialEq)]
pub struct MirrorPane {
    pub revision: String,
    pub session: MirrorSession,
}

/// C# `RemoteMirrorPaneInfo`: which workspace a host agent pane is in, the agent it runs and its permission mode (full access, and on a
/// Windows host Codex and Antigravity, need the host's switch).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirrorPaneInfo {
    pub workspace_id: String,
    pub permission_mode: String,
    pub provider: String,
}

// ── the wire's bodies (M/MightyCore/Remote/RemoteMirror.swift RemoteWireMirror…) ─────────────────────────────────────────────

use Need::{Nullable, Required};
use Shape::{Dictionary, Flag, List, Object, Record, Text, Whole};

pub(crate) const SUMMARY: &[Member] = &[
    Member("id", Text, Required),
    Member("workspaceId", Text, Required),
    Member("title", Text, Required),
    Member("kind", Text, Required),
    Member("provider", Text, Required),
    Member("model", Text, Required),
    Member("status", Text, Required),
    Member("revision", Text, Required),
    Member("pending", Whole, Required),
    Member("queued", Whole, Required),
    Member("branch", Text, Nullable),
];
pub(crate) const STATE: &[Member] = &[
    Member("protocol", Whole, Required),
    Member("mirror", Whole, Required),
    Member("hostName", Text, Required),
    Member("revision", Text, Required),
    Member("workspaces", List(&Record(SHARED)), Required),
    Member("sessions", List(&Record(SUMMARY)), Required),
    Member("unchanged", Flag, Nullable),
];
pub(crate) const DETAIL: &[Member] = &[
    Member("protocol", Whole, Required),
    Member("mirror", Whole, Required),
    Member("revision", Text, Required),
    Member("session", Object, Required),
];
pub(crate) const SUBMIT: &[Member] = &[Member("text", Text, Required), Member("attachments", List(&Record(ATTACHMENT)), Nullable)];
pub(crate) const OUTCOME: &[Member] = &[Member("protocol", Whole, Required), Member("outcome", Text, Required)];
pub(crate) const PERMISSION: &[Member] = &[
    Member("requestId", Text, Required),
    Member("runId", Text, Required),
    Member("allow", Flag, Required),
    Member("always", Flag, Nullable),
];
pub(crate) const QUESTIONS: &[Member] = &[
    Member("requestId", Text, Required),
    Member("runId", Text, Required),
    Member("answers", Dictionary(&Record(QUESTION_ANSWER)), Required),
];
pub(crate) const CREATE: &[Member] = &[Member("provider", Text, Required)];
pub(crate) const CREATED: &[Member] = &[Member("protocol", Whole, Required), Member("sessionId", Text, Required)];
pub(crate) const QUEUE_EDIT: &[Member] = &[Member("text", Text, Required)];
pub(crate) const QUEUE_MOVE: &[Member] = &[Member("to", Whole, Nullable), Member("before", Text, Nullable)];
pub(crate) const QUEUED: &[Member] =
    &[Member("id", Text, Required), Member("text", Text, Required), Member("runNow", Text, Nullable), Member("cut", Flag, Nullable)];

fn text(o: &Map<String, Value>, name: &str) -> String {
    o.get(name).and_then(Value::as_str).unwrap_or_default().to_string()
}
fn opt_text(o: &Map<String, Value>, name: &str) -> Option<String> {
    o.get(name).and_then(Value::as_str).map(str::to_string)
}
fn whole(o: &Map<String, Value>, name: &str) -> i64 {
    o.get(name).and_then(Value::as_i64).unwrap_or_default()
}
fn items<'a>(o: &'a Map<String, Value>, name: &str) -> impl Iterator<Item = &'a Map<String, Value>> {
    o.get(name).and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_object)
}
fn object(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(o) => o,
        _ => Map::new(),
    }
}
fn put(o: &mut Map<String, Value>, name: &str, value: Option<Value>) {
    if let Some(value) = value {
        o.insert(name.into(), value);
    }
}

/// The shared workspace as the wire writes it.
pub(crate) fn shared_value(w: &RemoteSharedWorkspace) -> Value {
    let mut o = Map::new();
    o.insert("id".into(), w.id.clone().into());
    o.insert("name".into(), w.name.clone().into());
    o.insert("path".into(), w.path.clone().into());
    Value::Object(o)
}

impl MirrorSummary {
    fn read(o: &Map<String, Value>) -> MirrorSummary {
        MirrorSummary {
            id: text(o, "id"),
            workspace_id: text(o, "workspaceId"),
            title: text(o, "title"),
            kind: text(o, "kind"),
            provider: text(o, "provider"),
            model: text(o, "model"),
            status: text(o, "status"),
            revision: text(o, "revision"),
            pending: whole(o, "pending"),
            queued: whole(o, "queued"),
            branch: opt_text(o, "branch"),
        }
    }

    pub fn to_value(&self) -> Value {
        let mut o = Map::new();
        o.insert("id".into(), self.id.clone().into());
        o.insert("workspaceId".into(), self.workspace_id.clone().into());
        o.insert("title".into(), self.title.clone().into());
        o.insert("kind".into(), self.kind.clone().into());
        o.insert("provider".into(), self.provider.clone().into());
        o.insert("model".into(), self.model.clone().into());
        o.insert("status".into(), self.status.clone().into());
        o.insert("revision".into(), self.revision.clone().into());
        o.insert("pending".into(), self.pending.into());
        o.insert("queued".into(), self.queued.into());
        put(&mut o, "branch", self.branch.clone().map(Value::String));
        Value::Object(o)
    }
}

/// C# `RemoteWireMirrorState(Protocol, Mirror, HostName, Revision, Workspaces, Sessions, Unchanged?)`. `revision` moves whenever the list
/// (workspaces, panes, any pane's revision) changed; `unchanged`: the client's `since` was this revision, and nothing else is sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireMirrorState {
    pub protocol: i32,
    pub mirror: i32,
    pub host_name: String,
    pub revision: String,
    pub workspaces: Vec<RemoteSharedWorkspace>,
    pub sessions: Vec<MirrorSummary>,
    pub unchanged: Option<bool>,
}

impl WireMirrorState {
    /// Read as the Mac reads it: every required member, no null in a list, no repeated member.
    pub fn decode(body: &[u8]) -> Result<WireMirrorState, BadBody> {
        let o = object(checked(body, STATE)?);
        Ok(WireMirrorState {
            protocol: whole(&o, "protocol") as i32,
            mirror: whole(&o, "mirror") as i32,
            host_name: text(&o, "hostName"),
            revision: text(&o, "revision"),
            workspaces: items(&o, "workspaces")
                .map(|w| RemoteSharedWorkspace { id: text(w, "id"), name: text(w, "name"), path: text(w, "path") })
                .collect(),
            sessions: items(&o, "sessions").map(MirrorSummary::read).collect(),
            unchanged: o.get("unchanged").and_then(Value::as_bool),
        })
    }

    pub fn to_value(&self) -> Value {
        let mut o = Map::new();
        o.insert("protocol".into(), self.protocol.into());
        o.insert("mirror".into(), self.mirror.into());
        o.insert("hostName".into(), self.host_name.clone().into());
        o.insert("revision".into(), self.revision.clone().into());
        o.insert("workspaces".into(), Value::Array(self.workspaces.iter().map(shared_value).collect()));
        o.insert("sessions".into(), Value::Array(self.sessions.iter().map(MirrorSummary::to_value).collect()));
        put(&mut o, "unchanged", self.unchanged.map(Value::Bool));
        Value::Object(o)
    }
}

/// C# `RemoteWireMirrorDetail(Protocol, Mirror, Revision, Session)`: `session` is the Mac's JSON of [`MirrorSession`] ([`support::write`]),
/// each object in it read once (a member repeated anywhere refuses the answer).
#[derive(Debug, Clone, PartialEq)]
pub struct WireMirrorDetail {
    pub protocol: i32,
    pub mirror: i32,
    pub revision: String,
    pub session: Map<String, Value>,
}

impl WireMirrorDetail {
    pub fn decode(body: &[u8]) -> Result<WireMirrorDetail, BadBody> {
        let mut o = object(checked(body, DETAIL)?);
        Ok(WireMirrorDetail {
            protocol: whole(&o, "protocol") as i32,
            mirror: whole(&o, "mirror") as i32,
            revision: text(&o, "revision"),
            session: o.remove("session").map(object).unwrap_or_default(),
        })
    }

    pub fn to_value(&self) -> Value {
        let mut o = Map::new();
        o.insert("protocol".into(), self.protocol.into());
        o.insert("mirror".into(), self.mirror.into());
        o.insert("revision".into(), self.revision.clone().into());
        o.insert("session".into(), Value::Object(self.session.clone()));
        Value::Object(o)
    }
}

/// C# `RemoteWireMirrorSubmit(Text, Attachments?)`: `attachments` may be left out (no files).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireMirrorSubmit {
    pub text: String,
    pub attachments: Option<Vec<RunAttachment>>,
}

impl WireMirrorSubmit {
    pub fn decode(body: &[u8]) -> Result<WireMirrorSubmit, BadBody> {
        let o = object(checked(body, SUBMIT)?);
        let attachments = match o.get("attachments") {
            Some(Value::Array(list)) => Some(list.iter().map(RunAttachment::read).collect::<Result<Vec<_>, _>>().map_err(|_| BadBody)?),
            _ => None,
        };
        Ok(WireMirrorSubmit { text: text(&o, "text"), attachments })
    }

    pub fn to_value(&self) -> Value {
        let mut o = Map::new();
        o.insert("text".into(), self.text.clone().into());
        put(
            &mut o,
            "attachments",
            self.attachments.as_ref().map(|list| Value::Array(list.iter().map(crate::remote::wire::written).collect())),
        );
        Value::Object(o)
    }
}

/// C# `RemoteWireMirrorOutcome(Protocol, Outcome)`: the host's word for what a send did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireMirrorOutcome {
    pub protocol: i32,
    pub outcome: String,
}

impl WireMirrorOutcome {
    pub fn decode(body: &[u8]) -> Result<WireMirrorOutcome, BadBody> {
        let o = object(checked(body, OUTCOME)?);
        Ok(WireMirrorOutcome { protocol: whole(&o, "protocol") as i32, outcome: text(&o, "outcome") })
    }

    pub fn to_value(&self) -> Value {
        let mut o = Map::new();
        o.insert("protocol".into(), self.protocol.into());
        o.insert("outcome".into(), self.outcome.clone().into());
        Value::Object(o)
    }
}

/// C# `RemoteWireMirrorPermission(RequestId, RunId, Allow, Always?)`: `always` (an Antigravity card's 항상 허용) only when set, as the
/// Mac's client sends it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireMirrorPermission {
    pub request_id: String,
    pub run_id: String,
    pub allow: bool,
    pub always: Option<bool>,
}

impl WireMirrorPermission {
    pub fn decode(body: &[u8]) -> Result<WireMirrorPermission, BadBody> {
        let o = object(checked(body, PERMISSION)?);
        Ok(WireMirrorPermission {
            request_id: text(&o, "requestId"),
            run_id: text(&o, "runId"),
            allow: o.get("allow").and_then(Value::as_bool).unwrap_or(false),
            always: o.get("always").and_then(Value::as_bool),
        })
    }

    pub fn to_value(&self) -> Value {
        let mut o = Map::new();
        o.insert("requestId".into(), self.request_id.clone().into());
        o.insert("runId".into(), self.run_id.clone().into());
        o.insert("allow".into(), self.allow.into());
        put(&mut o, "always", self.always.map(Value::Bool));
        Value::Object(o)
    }
}

/// C# `RemoteWireMirrorQuestions(RequestId, RunId, Answers)`.
#[derive(Debug, Clone, PartialEq)]
pub struct WireMirrorQuestions {
    pub request_id: String,
    pub run_id: String,
    pub answers: IndexMap<String, UserQuestionAnswer>,
}

impl WireMirrorQuestions {
    pub fn decode(body: &[u8]) -> Result<WireMirrorQuestions, BadBody> {
        let o = object(checked(body, QUESTIONS)?);
        let answers = o
            .get("answers")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
            .map(|(question, answer)| {
                let answer = answer.as_object().cloned().unwrap_or_default();
                let selected = answer
                    .get("selectedOptions")
                    .and_then(Value::as_array)
                    .map(|list| list.iter().filter_map(Value::as_str).map(str::to_string).collect())
                    .unwrap_or_default();
                (question.clone(), UserQuestionAnswer { selected_options: selected, custom_text: opt_text(&answer, "customText") })
            })
            .collect();
        Ok(WireMirrorQuestions { request_id: text(&o, "requestId"), run_id: text(&o, "runId"), answers })
    }

    pub fn to_value(&self) -> Value {
        let mut answers = Map::new();
        for (question, answer) in &self.answers {
            let mut a = Map::new();
            a.insert("selectedOptions".into(), Value::Array(answer.selected_options.iter().cloned().map(Value::String).collect()));
            put(&mut a, "customText", answer.custom_text.clone().map(Value::String));
            answers.insert(question.clone(), Value::Object(a));
        }
        let mut o = Map::new();
        o.insert("requestId".into(), self.request_id.clone().into());
        o.insert("runId".into(), self.run_id.clone().into());
        o.insert("answers".into(), Value::Object(answers));
        Value::Object(o)
    }
}

/// MC-49: `POST …/queue/{item}/edit`'s body: the queued request's new words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireMirrorQueueEdit {
    pub text: String,
}

impl WireMirrorQueueEdit {
    pub fn decode(body: &[u8]) -> Result<WireMirrorQueueEdit, BadBody> {
        Ok(WireMirrorQueueEdit { text: text(&object(checked(body, QUEUE_EDIT)?), "text") })
    }

    pub fn to_value(&self) -> Value {
        let mut o = Map::new();
        o.insert("text".into(), self.text.clone().into());
        Value::Object(o)
    }
}

/// MC-49: where `POST …/queue/{item}/move` puts a queued request: at a place (`to`, 0 the head, past the end the end) or just before
/// another queued request (`before`); exactly one of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueuePlace {
    To(usize),
    Before(String),
}

impl QueuePlace {
    /// The body, read strictly: one of `to` (0…1,000) and `before` (an identifier).
    pub fn decode(body: &[u8]) -> Result<QueuePlace, BadBody> {
        let o = object(checked(body, QUEUE_MOVE)?);
        let to = o.get("to").and_then(Value::as_i64);
        let before = o.get("before").and_then(Value::as_str);
        match (to, before) {
            (Some(to), None) if (0..=1_000).contains(&to) => Ok(QueuePlace::To(to as usize)),
            (None, Some(before)) if wire::identifier(before) => Ok(QueuePlace::Before(before.to_string())),
            _ => Err(BadBody),
        }
    }

    pub fn to_value(&self) -> Value {
        let mut o = Map::new();
        match self {
            QueuePlace::To(to) => o.insert("to".into(), (*to as i64).into()),
            QueuePlace::Before(before) => o.insert("before".into(), before.clone().into()),
        };
        Value::Object(o)
    }
}

/// C# `RemoteWireMirrorCreate(Provider)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireMirrorCreate {
    pub provider: String,
}

impl WireMirrorCreate {
    pub fn decode(body: &[u8]) -> Result<WireMirrorCreate, BadBody> {
        Ok(WireMirrorCreate { provider: text(&object(checked(body, CREATE)?), "provider") })
    }

    pub fn to_value(&self) -> Value {
        let mut o = Map::new();
        o.insert("provider".into(), self.provider.clone().into());
        Value::Object(o)
    }
}

/// C# `RemoteWireMirrorCreated(Protocol, SessionId)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireMirrorCreated {
    pub protocol: i32,
    pub session_id: String,
}

impl WireMirrorCreated {
    pub fn decode(body: &[u8]) -> Result<WireMirrorCreated, BadBody> {
        let o = object(checked(body, CREATED)?);
        Ok(WireMirrorCreated { protocol: whole(&o, "protocol") as i32, session_id: text(&o, "sessionId") })
    }

    pub fn to_value(&self) -> Value {
        let mut o = Map::new();
        o.insert("protocol".into(), self.protocol.into());
        o.insert("sessionId".into(), self.session_id.clone().into());
        Value::Object(o)
    }
}

/// C# `RemoteMirrorRoutes`: the mirror's paths under `/v2/`.
pub mod routes {
    /// `RemoteMirrorRoutes.Prefix`.
    pub const PREFIX: &str = "/v2/mirror";

    /// `IsSubmit(target)`: `POST /v2/mirror/sessions/{id}/submit`, the one mirror request that may carry attachments.
    pub fn is_submit(target: &str) -> bool {
        let parts: Vec<&str> = target.split('/').collect();
        parts.len() == 6
            && parts[0].is_empty()
            && parts[1] == "v2"
            && parts[2] == "mirror"
            && parts[3] == "sessions"
            && parts[5] == "submit"
            && mighty_core::wire::identifier(parts[4])
    }
}

/// Whether `provider` is one of the agents the wire carries (C# `Wire.Providers`).
pub(crate) fn known_provider(provider: &str) -> bool {
    wire::PROVIDERS.contains(&provider)
}

/// The remote protocol's version every mirror answer also carries.
pub(crate) const REMOTE_VERSION: i32 = remote_protocol::VERSION;
