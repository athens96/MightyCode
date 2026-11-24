//! The m1 router (C# `MobileDesktopRouter`, R\MobileDesktopRouter.cs:97-553; plan §1.5): `/m1/` paths only (no `#` or `\`), a
//! repeated query key or a POST with a query 400, unknown routes 404 `"Mobile route not found."`, errors `{protocol:1, error, code?}`,
//! C#'s exception mapping ([`crate::phone::desk::MobileError`]), a reply over `MAXIMUM_REPLY_BYTES` 413 (the structure shed first),
//! every route of the table with its statuses and codes, the long polls (`/m1/state`, `/m1/sessions/{id}`: 100 ms re-checks, 10 s at
//! most) on their own tasks, two submit slots, two file reads, the push routes, the screen-share routes 503 while F's switch is off.
//! It implements [`crate::relay::connection::RequestHandler`] for the relay and direct connections. Owner: W8-B.
//!
//! Every body is written with `mighty_core::json::JsonOut` (C#'s `Wire.Json`: non-ASCII as `\uXXXX`, nulls left out), in C#'s property
//! order. The router learns of changes only through [`Router::changed`] (the host's change hook) and reads the waiting permissions from
//! the desk; the panes are read again at the next [`Router::observe`] (the 250 ms tick and every long poll).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use mighty_composer::pending::SubmitReceipt;
use mighty_composer::slash::{self, SlashArgument, SlashCommandAction};
use mighty_core::attachment::RunAttachment;
use mighty_core::json::{JsonOut, Wire};
use mighty_core::locale::Locale;
use mighty_core::model::{AppSnapshot, LogEntry, RunSession, Workspace};
use mighty_core::permission::ToolPermissionRequest;
use mighty_core::shell::DelegationChildRow;
use mighty_core::wire;
use mighty_delegation::request::DelegationName;
use mighty_delegation::sidebar;
use mighty_platform::BoxFuture;
use mighty_run::model::ModelCatalog;
use mighty_run::plan_mode::{self, PlanDecision};
use mighty_terminal::prompt::TerminalPromptAnswer;
use mighty_workmap::WorkMapStart;
use serde_json::value::RawValue;
use serde_json::{Map, Value, json};
use tokio::sync::Semaphore;

use crate::phone::app_update::{self, MobileAppUpdate};
use crate::phone::desk::{ActionAnswer, MobileDesktopAction, MobileError, MobilePaneDigest, MobilePaneExtras, PhoneDesk};
use crate::phone::files::{self, MobileFilePreview, PreviewSlot};
use crate::phone::mighty::{self, MobileOut, MobilePermission};
use crate::phone::revisions::{MobileRevisions, Reading};
use crate::phone::uploads::{CHUNK_SIZE, MobileUploads};
use crate::push::hub::PushHub;
use crate::push::recipients;
use crate::relay::connection::{MobileDevice, MobileReply, RequestHandler};

/// `MobileDesktopRouter.Capabilities` (`:37`).
pub const CAPABILITIES: [&str; 21] = [
    "submit-mode",
    "queue",
    "pane",
    "history",
    "settings",
    "commands",
    "mighty",
    "status",
    "attachments",
    "style",
    "files",
    "structure",
    "direct",
    "push",
    "fileChanges",
    "workMap",
    "appUpdate",
    "pi",
    "opencode",
    "hermes",
    "roles",
];

/// `MobileWire.MaximumReplyBytes`: 1 MiB − 32 KiB.
pub const MAXIMUM_REPLY_BYTES: usize = mighty::MAXIMUM_REPLY_BYTES;
/// `ScreenShareAvailability.Capability`.
pub const SCREEN_SHARE: &str = "screenShare";
/// How often a long poll looks again.
pub const POLL_STEP: Duration = Duration::from_millis(100);
/// A request body's cap, and an upload chunk's.
pub const MAXIMUM_BODY: usize = 65_536;
pub const MAXIMUM_CHUNK_BODY: usize = 300 * 1_024;

/// `work` on a blocking thread, its panic the host's 500.
async fn blocking<T: Send + 'static>(work: impl FnOnce() -> Result<T, MobileError> + Send + 'static) -> Result<T, MobileError> {
    tokio::task::spawn_blocking(work).await.map_err(|_| MobileError::Failed("the desk's reading stopped".into()))?
}

/// `ScreenShareAvailability.Extra`: the extra capabilities a host advertises, `screenShare` left out while screen share is off.
pub fn extra_capabilities(extra: Option<Vec<String>>, available: bool) -> Vec<String> {
    match extra {
        None => vec![],
        Some(values) if available => values,
        Some(values) => values.into_iter().filter(|v| v != SCREEN_SHARE).collect(),
    }
}

/// Where the push routes reach the push hub (W8-A's, made after the router: src-tauri `phone.rs` looks it up at each request).
pub type PushLookup = Arc<dyn Fn() -> Option<Arc<PushHub>> + Send + Sync>;

/// How a router is made (C#'s constructor arguments and init-only properties that are not the desk's).
pub struct RouterOptions {
    /// `<directory>/uploads` (C#: `Path.Combine(directory, "uploads")`).
    pub uploads: PathBuf,
    /// `/m1/info`'s `platform` (`PlatformProfile::m1_platform`).
    pub platform: &'static str,
    /// `ScreenShareAvailable` (`screen::ENABLED` by default).
    pub screen_share: bool,
    /// `Push`: None answers `/m1/push/*` as unknown routes.
    pub push: Option<PushLookup>,
}

impl RouterOptions {
    pub fn new(uploads: PathBuf, platform: &'static str) -> RouterOptions {
        RouterOptions { uploads, platform, screen_share: crate::screen::ENABLED, push: None }
    }
}

#[derive(Default)]
struct Notes {
    revisions: MobileRevisions,
    notifications: Vec<(String, i64)>,
}

struct Inner {
    desk: Arc<dyn PhoneDesk>,
    options: RouterOptions,
    uploads: Arc<MobileUploads>,
    observing: Mutex<()>,
    sync: Mutex<Notes>,
    moved: AtomicBool,
    notify: tokio::sync::Notify,
    submit_slots: Arc<Semaphore>,
    file_slots: Arc<Semaphore>,
    previews: PreviewSlot,
    /// MC-53: the entry windows sent lately, which a detail asked with `bodySince` is built from.
    bodies: crate::body_delta::BodyHistory,
}

/// The m1 router over a desk.
#[derive(Clone)]
pub struct Router {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Router {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Router").field("state", &self.state_revision()).finish()
    }
}

type Routed = Result<MobileReply, MobileError>;

fn locale() -> Locale {
    mighty_run::strings::locale()
}

fn request(status: u16, message: impl Into<String>) -> MobileError {
    MobileError::request(status, message, None)
}

fn not_found() -> MobileError {
    request(404, "Mobile route not found.")
}

/// `Error(status, text, code)` as the reply C#'s catch blocks make.
fn error_reply(error: MobileError) -> MobileReply {
    match error {
        MobileError::Request { status, message, code } => MobileReply::error(status, &message, code.as_deref()),
        MobileError::Invalid => MobileReply::error(400, "Invalid mobile request.", None),
        MobileError::Changed => MobileReply::error(409, "The pane changed; refresh and try again.", None),
        MobileError::Unavailable => MobileReply::error(403, "The requested file is unavailable.", None),
        MobileError::Failed(_) => MobileReply::error(500, "Desktop request failed.", None),
    }
}

/// `Reply(body, status)`: a body over the limit is refused whole.
fn reply(status: u16, body: String) -> MobileReply {
    if body.len() <= MAXIMUM_REPLY_BYTES {
        MobileReply::new(status, body)
    } else {
        MobileReply::error(413, "Response exceeds the mobile display limit.", None)
    }
}

fn object(write: impl FnOnce(&mut JsonOut)) -> String {
    let mut out = JsonOut::new();
    out.begin_object();
    out.prop_int("protocol", 1);
    write(&mut out);
    out.end_object();
    out.finish()
}

/// `OK()`.
fn ok() -> MobileReply {
    reply(200, object(|o| o.prop_bool("ok", true)))
}

/// A value written as `Wire.Json` writes an `object?` member: a null left out.
fn prop_value(out: &mut JsonOut, name: &str, value: Option<&Value>) {
    if let Some(value) = value.filter(|v| !v.is_null()) {
        out.name(name);
        out.value(value);
    }
}

/// `SubmitReply(result)`: a receipt answers with its outcome word in `accepted` and `outcome` and its ids; any other answer as before.
pub fn submit_reply(result: &ActionAnswer) -> String {
    match result {
        ActionAnswer::Receipt(receipt) => mobile_submit_reply(receipt),
        ActionAnswer::Value(value) => object(|o| prop_value(o, "accepted", Some(value))),
    }
}

/// `SubmitReceipt.MobileReply()` (`MobileSubmitReply(Protocol, Accepted, Outcome, EntryId, RunId)`).
pub fn mobile_submit_reply(receipt: &SubmitReceipt) -> String {
    object(|o| {
        o.prop_str("accepted", Some(&receipt.outcome));
        o.prop_str("outcome", Some(&receipt.outcome));
        o.prop_str("entryId", receipt.entry_id.as_deref());
        o.prop_str("runId", receipt.run_id.as_deref());
    })
}

fn answer_value(answer: &ActionAnswer) -> Value {
    match answer {
        ActionAnswer::Value(value) => value.clone(),
        ActionAnswer::Receipt(receipt) => Value::String(receipt.outcome.clone()),
    }
}

/// `Uri.UnescapeDataString`: `%XX` sequences that form UTF-8 decoded, anything else kept as it is.
fn unescape(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%' {
            let hex = |b: u8| (b as char).to_digit(16);
            if let (Some(Some(h)), Some(Some(l))) = (bytes.get(at + 1).map(|b| hex(*b)), bytes.get(at + 2).map(|b| hex(*b))) {
                out.push((h * 16 + l) as u8);
                at += 3;
                continue;
            }
        }
        out.push(bytes[at]);
        at += 1;
    }
    match String::from_utf8(out) {
        Ok(decoded) => decoded,
        Err(_) => text.to_string(),
    }
}

/// `long.TryParse` (`NumberStyles.Integer`: white space around, a leading sign).
fn parse_long(text: &str) -> Option<i64> {
    let trimmed = text.trim_matches([' ', '\t', '\n', '\u{0B}', '\u{0C}', '\r']);
    let digits = trimmed.strip_prefix('+').unwrap_or(trimmed);
    if digits.starts_with('+') || digits.is_empty() {
        return None;
    }
    digits.parse::<i64>().ok()
}

type Query = Vec<(String, String)>;

fn query_get<'a>(query: &'a Query, key: &str) -> Option<&'a str> {
    query.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

/// `Number(query, key, minimum, maximum, fallback)`.
fn number(query: &Query, key: &str, minimum: i64, maximum: i64, fallback: i64) -> Result<i64, MobileError> {
    match query_get(query, key) {
        None => Ok(fallback),
        Some(text) => parse_long(text).filter(|n| (minimum..=maximum).contains(n)).ok_or_else(|| request(400, format!("Invalid {key}."))),
    }
}

/// `Object(body, maximum)`: the request's body, an object of at most `maximum` UTF-8 bytes as the phone sent it.
fn body_object(body: Option<&RawValue>, maximum: usize) -> Result<Map<String, Value>, MobileError> {
    let invalid = || request(400, "Invalid request body.");
    let raw = body.ok_or_else(invalid)?;
    if raw.get().len() > maximum {
        return Err(invalid());
    }
    match serde_json::from_str::<Value>(raw.get()) {
        Ok(Value::Object(map)) => Ok(map),
        _ => Err(invalid()),
    }
}

fn text<'a>(body: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    body.get(key).and_then(Value::as_str)
}

/// `Required(body, key)`.
fn required<'a>(body: &'a Map<String, Value>, key: &str) -> Result<&'a str, MobileError> {
    text(body, key).ok_or_else(|| request(400, format!("Missing {key}.")))
}

/// `RequestText(body, allowEmpty)`.
fn request_text(body: &Map<String, Value>, allow_empty: bool) -> Result<String, MobileError> {
    let text = required(body, "text")?;
    if (!allow_empty && text.chars().all(char::is_whitespace)) || text.len() > 32_768 || text.contains('\0') {
        return Err(request(400, "Invalid request text."));
    }
    Ok(text.to_string())
}

/// `RemoteWorkspacePhone.Workspaces`: this PC's workspaces only, and of them only `allowed` when the phone is held to some
/// ([`crate::phone::desk::phone_workspace_ids`], 휴대폰에서 볼 워크스페이스).
fn local_workspaces<'a>(snapshot: &'a AppSnapshot, allowed: Option<&[String]>) -> Vec<&'a Workspace> {
    snapshot.workspaces().iter().filter(|w| w.remote.is_none() && allowed.is_none_or(|ids| ids.iter().any(|id| id == w.id()))).collect()
}

/// `RemoteWorkspacePhone.Sessions` without the files and work map panes: the panes of those workspaces the phone is told of.
fn phone_panes(snapshot: &AppSnapshot, allowed: Option<&[String]>) -> Vec<Arc<RunSession>> {
    let local: std::collections::HashSet<&str> = local_workspaces(snapshot, allowed).iter().map(|w| w.id()).collect();
    snapshot.sessions().iter().filter(|s| local.contains(s.workspace_id()) && !matches!(s.kind(), "files" | "workmap")).cloned().collect()
}

/// `ModelLabel.Resolution(model, catalog, hint)`.
fn resolution(model: &str, catalog: &ModelCatalog, hint: Option<&str>) -> Option<String> {
    if let Some(resolved) = catalog.models.iter().find(|m| m.value == model).and_then(|m| m.resolved_model.clone()) {
        return Some(resolved);
    }
    let mut alias = model.trim().to_lowercase();
    if alias.ends_with("[1m]") {
        alias.truncate(alias.len() - 4);
    }
    if ["opus", "sonnet", "haiku", "fable"].contains(&alias.as_str()) { hint.map(str::to_string) } else { None }
}

/// The user's home folder (the slash catalogue's `~/.claude`).
fn home() -> PathBuf {
    std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from).unwrap_or_default()
}

impl Router {
    /// The pane's body read if the load left it on disk (the mirror's detail asks before it reads the pane).
    pub fn ensure_body(&self, id: &str) {
        self.inner.desk.ensure_body(id);
    }

    pub fn new(desk: Arc<dyn PhoneDesk>, options: RouterOptions) -> Router {
        let uploads = Arc::new(MobileUploads::new(&options.uploads));
        Router {
            inner: Arc::new(Inner {
                desk,
                options,
                uploads,
                sync: Mutex::new(Notes::default()),
                moved: AtomicBool::new(true),
                notify: tokio::sync::Notify::new(),
                observing: Mutex::new(()),
                submit_slots: Arc::new(Semaphore::new(2)),
                file_slots: Arc::new(Semaphore::new(2)),
                previews: PreviewSlot::new(),
                bodies: crate::body_delta::BodyHistory::default(),
            }),
        }
    }

    /// `RouteAsync(method, path, body, device)`: the reply of one request (never an error: a failure is its reply).
    pub fn route(
        &self,
        device: MobileDevice,
        method: String,
        path: String,
        body: Option<Box<RawValue>>,
    ) -> BoxFuture<'static, MobileReply> {
        let inner = self.inner.clone();
        // A route that panics answers the host's 500 ("Desktop request failed."), as an exception C# does not map does.
        Box::pin(Unwinding(Box::pin(async move {
            match inner.route(&device.client_id, &method, &path, body.as_deref()).await {
                Ok(reply) => reply,
                Err(error) => error_reply(error),
            }
        })))
    }

    /// `Changed`: something a phone may show moved (a run event, a permission, delegation, an Antigravity card); the panes are read again
    /// at the next [`Router::observe`]. Cheap: the host calls it from its change hook.
    pub fn changed(&self) {
        self.inner.changed();
    }

    /// `Observe`: the panes read again when something moved, the revisions of what changed moved. A pass that fails (a desk that
    /// panicked) is made again at the next call.
    pub fn observe(&self) {
        let _ = self.inner.observe();
    }

    /// `TakeNotifications`: the scopes whose revision moved since the last call, `state` before the sessions, each with its newest
    /// revision.
    pub fn take_notifications(&self) -> Vec<(String, i64)> {
        std::mem::take(&mut self.inner.notes().notifications)
    }

    /// `StateRevision`.
    pub fn state_revision(&self) -> i64 {
        self.inner.notes().revisions.state()
    }

    /// `SessionRevision(id)`.
    pub fn session_revision(&self, id: &str) -> i64 {
        self.inner.notes().revisions.session(id)
    }

    /// `OpenUploadCount`: files the phone is uploading or has uploaded and not yet sent.
    pub fn open_upload_count(&self) -> usize {
        self.inner.uploads.count()
    }

    /// `ResetUploads`.
    pub fn reset_uploads(&self) {
        self.inner.uploads.clear();
    }

    /// `PendingRequests(id)` (W9-J): the pane's tool, question and plan requests waiting for an answer, as the phone sees them, oldest
    /// first (the remote mirror reads them too, `remote::mirror::desktop_host`).
    pub fn pending_requests(&self, id: &str) -> Vec<ToolPermissionRequest> {
        let mut pending = self.inner.pending(id);
        pending.sort_by(|a, b| a.received_at.as_deref().unwrap_or("").cmp(b.received_at.as_deref().unwrap_or("")));
        pending
    }

    /// `SubmitInlineAsync(sessionId, text, attachments)` (W9-J): a send into one of this PC's agent panes with its files already here
    /// (the remote mirror's `submit`): the phone's submit after its uploads, through the same desktop action and slots. The desktop's
    /// answer (a receipt). A pane the phone does not see is 404, a pane that is no agent's 409, as the phone's.
    pub async fn submit_inline(&self, session_id: &str, text: &str, attachments: Vec<RunAttachment>) -> Result<ActionAnswer, MobileError> {
        let pane = self.inner.session(session_id)?;
        if pane.kind() != "claude" {
            return Err(request(409, "Use the desktop for this pane."));
        }
        let mut body = Map::new();
        body.insert("text".into(), Value::String(text.to_string()));
        let permit = self.inner.submit_slots.clone().acquire_owned().await.map_err(|_| MobileError::Failed("closed".into()))?;
        let inner = self.inner.clone();
        let id = pane.id().to_string();
        // On a task of its own, as the phone's submit: a caller that goes away neither frees the slot nor stops the action midway.
        tokio::spawn(async move {
            let _slot = permit;
            let result = inner
                .desk
                .action(MobileDesktopAction {
                    session_id: id,
                    action: "submit".into(),
                    body: Value::Object(body),
                    attachments: (!attachments.is_empty()).then_some(attachments),
                })
                .await;
            inner.changed();
            result
        })
        .await
        .map_err(|_| MobileError::Failed("the submit stopped".into()))?
    }
}

/// A future whose panic becomes [`MobileError::Failed`]'s reply.
struct Unwinding(BoxFuture<'static, MobileReply>);

impl std::future::Future for Unwinding {
    type Output = MobileReply;

    fn poll(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<MobileReply> {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.0.as_mut().poll(cx))) {
            Ok(poll) => poll,
            Err(_) => std::task::Poll::Ready(error_reply(MobileError::Failed("the route panicked".into()))),
        }
    }
}

impl RequestHandler for Router {
    fn handle(&self, device: MobileDevice, method: String, path: String, body: Option<Box<RawValue>>) -> BoxFuture<'static, MobileReply> {
        self.route(device, method, path, body)
    }
}

impl Inner {
    fn notes(&self) -> std::sync::MutexGuard<'_, Notes> {
        self.sync.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn changed(&self) {
        self.moved.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    /// `Observe`: Err when the pass failed (it is made again at the next call).
    fn observe(&self) -> Result<(), MobileError> {
        // One pass at a time: a pass asked for while another runs leaves `moved` set for the next call (the running pass may have read
        // the panes before what moved).
        let Ok(_pass) = self.observing.try_lock() else {
            return Ok(());
        };
        if !self.moved.swap(false, Ordering::SeqCst) {
            return Ok(());
        }
        let pass = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let snapshot = self.desk.snapshot();
            let allowed = self.allowed(&snapshot);
            let panes = phone_panes(&snapshot, allowed.as_deref());
            let children = self.desk.child_rows();
            let summaries: Vec<(MobilePaneDigest, String, String)> = panes
                .iter()
                .map(|pane| {
                    let digest = self.desk.digest(pane).unwrap_or_else(|| unread(pane));
                    let held = self.desk.held_items(pane.id()).iter().map(|i| i.id.clone()).collect::<Vec<_>>().join("\n");
                    // MC-49: a queued request edited or moved moves the session's revision too.
                    let held = format!("{held}\u{1}{}", self.desk.queue_digest(pane.id()));
                    (digest, self.summary(pane, 0, &snapshot, &children), held)
                })
                .collect();
            let readings: Vec<Reading> = panes
                .iter()
                .zip(summaries)
                .map(|(pane, (digest, summary, held))| Reading {
                    pane,
                    editable: digest.editable,
                    options: digest.options,
                    mighty: digest.mighty,
                    summary,
                    held,
                })
                .collect();
            let update = self.app_update_phone(&snapshot).map(|u| u.to_json()).unwrap_or_else(|| "null".into());
            let mut notes = self.notes();
            let moved = notes.revisions.observe(local_workspaces(&snapshot, allowed.as_deref()), &readings, &update);
            let any_moved = !moved.is_empty();
            for note in moved {
                notes.notifications.retain(|(scope, _)| *scope != note.0);
                notes.notifications.push(note);
            }
            drop(notes);
            if any_moved {
                self.notify.notify_waiters();
            }
        }));
        if pass.is_err() {
            self.changed();
            return Err(MobileError::Changed);
        }
        Ok(())
    }

    /// The app update as the phone sees it, with the requests running now; None when there is none.
    fn app_update_phone(&self, snapshot: &AppSnapshot) -> Option<MobileAppUpdate> {
        let state = self.desk.app_update_read()?;
        // Every running request of this PC counts, also one in a workspace the phone does not see: an update would end it.
        let running =
            phone_panes(snapshot, None).iter().filter(|s| s.status() == "running" || self.desk.is_session_running(s.id())).count() as i32;
        app_update::map(state.as_ref(), &self.desk.app_version(), running)
    }

    /// The workspaces a paired phone is held to (None: every local one), by the saved choice and what is shared now.
    fn allowed(&self, snapshot: &AppSnapshot) -> Option<Vec<String>> {
        crate::phone::desk::phone_workspace_ids(snapshot.mobile_remote.as_ref(), || self.desk.shared_workspace_ids())
    }

    /// `Pending(id)`: the pane's waiting tool permissions (never a remote pane's).
    fn pending(&self, id: &str) -> Vec<ToolPermissionRequest> {
        if self.desk.is_remote_session(id) {
            return vec![];
        }
        self.desk.pending_permissions(id).into_iter().filter(|p| p.state == "pending" && p.run_id == id).collect()
    }

    /// `Session(id)`: the pane whole (its body read first if the load left it on disk).
    fn session(&self, id: &str) -> Result<Arc<RunSession>, MobileError> {
        self.desk.ensure_body(id);
        let snapshot = self.desk.snapshot();
        phone_panes(&snapshot, self.allowed(&snapshot).as_deref())
            .into_iter()
            .find(|p| p.id() == id)
            .ok_or_else(|| request(404, "Session not found."))
    }

    fn revision(&self, id: &str) -> i64 {
        self.notes().revisions.session(id)
    }

    /// `Summary(pane, revision)`: one pane as the phone's list shows it. `children` is the sidebar's child list
    /// ([`PhoneDesk::child_rows`]), read once for every pane of a pass.
    fn summary(&self, pane: &RunSession, revision: i64, snapshot: &AppSnapshot, children: &[DelegationChildRow]) -> String {
        let l = locale();
        let pending = self.pending(pane.id());
        let tail = pane.logs().iter().rev().find(|e| matches!(e.kind(), "assistant" | "user" | "error" | "output"));
        let agent = pane.kind() == "claude";
        let style = if agent { self.desk.active_style(pane) } else { None };
        let antigravity = self.desk.antigravity_card(pane.id()).is_some() as i64;
        let queued = match self.desk.queue_count(pane.id()) {
            Some(count) => count as i64 + self.desk.held_items(pane.id()).len() as i64,
            None => self.desk.extras(pane).queued.len() as i64,
        };
        let catalog = self.desk.catalog(pane).unwrap_or_else(|| mighty_run::providers::fallback(pane.provider()));
        let resolved =
            (pane.kind() != "shell").then(|| resolution(pane.model(), &catalog, mighty_core::model_label::reported_model(pane))).flatten();
        let _ = l;
        let mut out = JsonOut::new();
        out.begin_object();
        out.prop_str("id", Some(pane.id()));
        out.prop_str("workspaceId", Some(pane.workspace_id()));
        out.prop_str("title", Some(pane.title()));
        out.prop_str("kind", Some(pane.kind()));
        out.prop_str("provider", Some(pane.provider()));
        out.prop_str("model", Some(pane.model()));
        out.prop_str("status", Some(pane.status()));
        out.prop_int("revision", revision);
        let updated = pane.logs().last().and_then(|e| e.timestamp.clone()).or_else(|| pane.created_at.clone());
        out.prop_str("updatedAt", Some(updated.as_deref().unwrap_or("")));
        if let Some(tail) = tail {
            out.name("preview");
            out.begin_object();
            out.prop_str("kind", Some(tail.kind()));
            out.prop_str("text", Some(&wire::clean(tail.text(), 200)));
            out.end_object();
        }
        out.prop_int("pendingPermissions", pending.iter().filter(|p| !p.can_answer_questions).count() as i64 + antigravity);
        out.prop_int("pendingQuestions", pending.iter().filter(|p| p.can_answer_questions).count() as i64);
        out.prop_int("queued", queued);
        out.prop_str("resumeId", pane.resume_id.as_deref());
        out.prop_bool("terminal", !agent);
        if agent {
            out.prop_str("agentViewMode", Some(mighty::view_mode(pane.agent_view_mode.as_deref())));
            out.prop_str("mightyStyle", Some(mighty::style(style.as_deref())));
            out.prop_str("styleId", Some(style.as_deref().unwrap_or(mighty::CLI_STYLE)));
        }
        out.prop_str("titleMode", Some(pane.title_mode.as_deref().unwrap_or("auto")));
        out.prop_str("resolvedModel", resolved.as_deref());
        out.prop_bool("canSteer", false);
        // MC-28: the pane this one was forked from (`forkedFrom { paneId, requestId }`).
        if let Some(link) = &pane.forked_from {
            out.name("forkedFrom");
            out.begin_object();
            out.prop_str("paneId", Some(&link.pane_id));
            out.prop_str("requestId", link.request_id.as_deref());
            out.prop_str("title", link.title.as_deref());
            out.end_object();
        }
        // MC-29: the compare runs this pane started (`compareRuns [{ id, request, baseCommit, test, children [{ id, provider, model,
        // failure, costUsd, testState }] }]`), and a child's group (`compareId`). The phone draws none of it yet.
        if let Some(runs) = pane.compare_runs.as_ref().filter(|runs| !runs.is_empty()) {
            out.name("compareRuns");
            out.begin_array();
            for run in runs {
                out.begin_object();
                out.prop_str("id", Some(&run.id));
                out.prop_str("request", Some(&run.request.chars().take(400).collect::<String>()));
                out.prop_str("baseCommit", Some(&run.base_commit));
                out.prop_str("test", run.test_name.as_deref());
                out.prop_str("createdAt", run.created_at.as_deref());
                out.name("children");
                out.begin_array();
                for child in &run.children {
                    out.begin_object();
                    out.prop_str("id", Some(&child.id));
                    out.prop_str("provider", Some(&child.provider));
                    out.prop_str("model", child.model.as_deref());
                    out.prop_str("failure", child.failure.as_deref());
                    out.prop_opt_float("costUsd", child.cost_usd);
                    out.prop_str("testState", child.test_state.as_deref());
                    out.end_object();
                }
                out.end_array();
                out.end_object();
            }
            out.end_array();
        }
        if let Some(member) = &pane.compare_member {
            out.prop_str("compareId", Some(&member.group));
        }
        // MC-26: the desktop's mark on a pane waiting for the user (a terminal's notification or bell, a card, a failed setup).
        if let Some((kind, text, at)) = self.desk.attention(pane.id()) {
            out.name("attention");
            out.begin_object();
            out.prop_str("kind", Some(&kind));
            out.prop_str("text", Some(&text));
            out.prop_str("at", Some(&at));
            out.end_object();
        }
        // A pane in its own worktree: the bar the phone draws over it (branch, PORT and port block, counts, run actions; MC-25).
        if let Some(bar) = self.desk.pane_worktree(pane, snapshot, children) {
            write_worktree(&mut out, &bar);
        }
        // MC-34: the pane's agent runs in the OS sandbox (the phone's 샌드박스 badge).
        if self.desk.sandboxed(pane, snapshot) {
            out.prop_bool("sandboxed", true);
        }
        // A Mighty Orchestration child: its sidebar row's parent and state, waiting while it asks for a human's answer
        // (`DelegationSidebar.shownState`, the sidebar's own test). A pane the delegation file does not list carries neither.
        if let Some(row) = children.iter().find(|row| row.id == pane.id()) {
            let asks_human = !pending.is_empty() || antigravity > 0;
            out.prop_str("parentSessionId", Some(&row.parent_session_id));
            out.prop_str("childState", Some(sidebar::shown_state(row.state, asks_human).name()));
        }
        out.end_object();
        out.finish()
    }

    /// `State()`: the phone's list.
    fn state(&self) -> String {
        let snapshot = self.desk.snapshot();
        let revision = self.notes().revisions.state();
        let host_name = self.desk.host_name();
        let allowed = self.allowed(&snapshot);
        let mut out = JsonOut::new();
        out.begin_object();
        out.prop_int("protocol", 1);
        out.prop_int("revision", revision);
        out.prop_str("hostName", Some(&host_name));
        out.name("workspaces");
        out.begin_array();
        for w in local_workspaces(&snapshot, allowed.as_deref()) {
            out.begin_object();
            out.prop_str("id", Some(w.id()));
            out.prop_str("name", Some(w.name()));
            out.prop_str("path", Some(w.path()));
            out.end_object();
        }
        out.end_array();
        out.name("sessions");
        out.begin_array();
        let children = self.desk.child_rows();
        for pane in phone_panes(&snapshot, allowed.as_deref()) {
            let summary = self.summary(&pane, self.revision(pane.id()), &snapshot, &children);
            out.raw(&summary);
        }
        out.end_array();
        if let Some(update) = self.app_update_phone(&snapshot) {
            out.name("appUpdate");
            update.write(&mut out);
        }
        out.end_object();
        out.finish()
    }

    /// `SessionReply(pane, asked, roles, body)`: the structure travels only to a phone that asked (`asked`: the tag it holds) and only
    /// when it differs; a detail over the limit sheds the structure first, keeping the tag. `body` (MC-53): the phone takes deltas and
    /// holds the window of this revision (None inside: none yet); without it the entries are the window whole and no mark is sent,
    /// byte for byte the answer C# gives.
    fn session_reply(&self, pane: &RunSession, asked: Option<i64>, roles: bool, body: Option<Option<&str>>) -> MobileReply {
        let detail = if roles { self.desk.role_extras(pane).unwrap_or_else(|| self.desk.extras(pane)) } else { self.desk.extras(pane) };
        let built = if asked.is_none() || detail.mighty.is_none() { None } else { self.desk.structure(pane) };
        let structure = built.as_ref().filter(|b| Some(b.tag) != asked).and_then(|b| b.value.as_ref()).map(|s| s.to_json());
        let tag = built.as_ref().map(|b| b.tag);
        let logs = pane.logs();
        let window = mighty::phone_entries(&logs[logs.len().saturating_sub(mighty::MAXIMUM_ENTRIES)..]);
        let answer = body.map(|since| self.bodies.answer(pane.id(), &window, since));
        let entries = (&window, answer.as_ref());
        let first = reply(200, self.detail(pane, &detail, structure.as_deref(), tag, entries));
        if first.status == 413 && structure.is_some() { reply(200, self.detail(pane, &detail, None, tag, entries)) } else { first }
    }

    /// `Detail(pane, detail, structure, tag)`, with the entries to send (`entries`: the window, and MC-53's answer when the phone asked
    /// for one: only the changed entries for a delta, and the mark after `hasOlder`).
    fn detail(
        &self,
        pane: &RunSession,
        detail: &MobilePaneExtras,
        structure: Option<&str>,
        structure_tag: Option<i64>,
        (window, body): (&Vec<LogEntry>, Option<&crate::body_delta::BodyAnswer>),
    ) -> String {
        let l = locale();
        let snapshot = self.desk.snapshot();
        let revision = self.revision(pane.id());
        let usage = pane.session_usage.as_ref();
        let logs = pane.logs();
        let mut out = JsonOut::new();
        out.begin_object();
        out.prop_int("protocol", 1);
        out.prop_int("revision", revision);
        out.name("session");
        out.raw(&self.summary(pane, revision, &snapshot, &self.desk.child_rows()));
        out.name("entries");
        out.list(body.and_then(|b| b.entries.as_ref()).unwrap_or(window));
        out.prop_bool("hasOlder", logs.len() > mighty::MAXIMUM_ENTRIES);
        if let Some(body) = body {
            out.name("body");
            body.mark.write(&mut out);
        }
        let mut permissions: Vec<MobilePermission> = self.pending(pane.id()).iter().map(|p| MobilePermission::from(p, &l)).collect();
        if let Some(card) = self.desk.antigravity_card(pane.id()) {
            permissions.push(MobilePermission::from_antigravity(&card, &l));
        }
        out.name("permissions");
        permissions.write(&mut out);
        out.name("queued");
        write_queued(&mut out, &detail.queued);
        if let Some(usage) = usage {
            out.name("usage");
            out.begin_object();
            out.prop_str("model", usage.model.as_deref());
            out.prop_opt_int("contextUsedTokens", usage.context_used_tokens);
            out.prop_opt_int("contextWindowTokens", usage.context_window_tokens);
            let percent = match (usage.context_used_tokens, usage.context_window_tokens) {
                (Some(used), Some(window)) if used >= 0 && window > 0 => Some((used as f64 / window as f64 * 100.0).clamp(0.0, 100.0)),
                _ => None,
            };
            out.prop_opt_float("contextPercent", percent);
            out.prop_opt_int("totalTokens", usage.total_tokens);
            out.prop_opt_float("costUSD", usage.cost_usd);
            out.end_object();
        }
        out.prop_opt_float("elapsedSeconds", pane.run_timing.as_ref().map(|t| mighty_run::timing::elapsed(t, None)));
        prop_value(&mut out, "settings", detail.settings.as_ref());
        prop_value(&mut out, "mighty", with_structure(detail.mighty.as_ref(), structure, structure_tag).as_ref());
        prop_value(&mut out, "statusLine", detail.status_line.as_ref());
        if let Some(limits) = mighty::rate_limits(usage.and_then(|u| u.rate_limits.as_deref()), &l) {
            out.name("rateLimits");
            limits.write(&mut out);
        }
        if let Some(targets) = detail.ask_targets.as_ref().filter(|t| !t.is_empty()) {
            out.name("askTargets");
            out.begin_array();
            for t in targets {
                out.begin_object();
                out.prop_str("id", Some(&t.id));
                out.prop_str("label", Some(&t.label));
                out.end_object();
            }
            out.end_array();
            out.prop_str("askTarget", detail.ask_target.as_deref());
        }
        if let Some(work) = detail.background_work.as_ref().filter(|w| !w.is_empty()) {
            out.name("backgroundWork");
            out.begin_array();
            for w in work {
                out.begin_object();
                out.prop_str("id", Some(&w.id));
                out.prop_str("kind", Some(&w.kind));
                out.prop_str("title", Some(&w.title));
                out.prop_str("status", Some(&w.status));
                out.prop_str("startedAt", Some(&w.started_at));
                out.prop_str("command", w.command.as_deref());
                out.prop_strings("outputTail", w.output_tail.as_ref());
                out.end_object();
            }
            out.end_array();
        }
        out.prop_opt_int("backgroundDone", detail.background_done.map(i64::from));
        if let Some(prompt) = &detail.terminal_prompt {
            out.name("terminalPrompt");
            out.begin_object();
            out.prop_str("id", Some(&prompt.id));
            out.prop_str("handle", Some(&prompt.handle));
            out.prop_str("text", Some(&prompt.text));
            out.name("options");
            out.strings(&prompt.options);
            out.prop_str("kind", Some(prompt.kind));
            out.prop_opt_bool("defaultYes", prompt.default_yes);
            out.end_object();
        }
        if let Some(blocked) = detail.sandbox_blocked.as_ref().filter(|b| !b.is_empty()) {
            out.name("sandboxBlocked");
            out.begin_array();
            for block in blocked {
                out.begin_object();
                out.prop_str("kind", Some(&block.kind));
                out.prop_str("what", Some(&block.what));
                out.prop_str("reason", Some(&block.reason));
                out.prop_str("at", Some(&block.at));
                out.end_object();
            }
            out.end_array();
        }
        if let Some(verify) = &detail.verify {
            out.name("verify");
            out.value(verify);
        }
        out.end_object();
        out.finish()
    }

    async fn route(self: &Arc<Self>, device: &str, method: &str, path: &str, body: Option<&RawValue>) -> Routed {
        let l = locale();
        if !path.starts_with("/m1/") || path.contains('#') || path.contains('\\') {
            return Err(not_found());
        }
        let Ok(url) = url::Url::parse(&format!("http://mobile.invalid{path}")) else { return Err(not_found()) };
        if url.host_str() != Some("mobile.invalid") {
            return Err(not_found());
        }
        let route: Vec<String> = url.path().split('/').filter(|s| !s.is_empty()).map(unescape).collect();
        let route: Vec<&str> = route.iter().map(String::as_str).collect();
        let mut query: Query = vec![];
        for item in url.query().unwrap_or("").split('&').filter(|s| !s.is_empty()) {
            let (key, value) = item.split_once('=').unwrap_or((item, ""));
            let (key, value) = (unescape(key), unescape(value));
            if query.iter().any(|(k, _)| *k == key) {
                return Err(request(400, "Repeated query argument."));
            }
            query.push((key, value));
        }
        if method == "POST" && !query.is_empty() {
            return Err(request(400, "Unexpected query argument."));
        }
        // Always the authenticated device, never a body field (M/MobileRemoteService.swift push routes).
        if method == "POST"
            && route.len() == 3
            && route[1] == "push"
            && let Some(push) = self.options.push.as_ref().and_then(|lookup| lookup())
            && let Some(answer) = self.push_route(&push, route[2], device, body, &l)?
        {
            return Ok(answer);
        }
        if method == "GET" && route == ["m1", "info"] {
            return Ok(self.info());
        }
        if method == "GET" && (route == ["m1", "state"] || route.len() == 3 && route[1] == "sessions") {
            return self.poll(&route, &query).await;
        }
        if method == "POST" && route == ["m1", "app-update"] && query.is_empty() {
            return self.app_update(device, body, &l).await;
        }
        if route.len() == 3 && route[1] == "screen-share" {
            return Err(request(503, "Screen sharing is not available on this host."));
        }
        if matches!(route.len(), 4 | 5)
            && route[1] == "workspaces"
            && route[3] == "workmap"
            && let Some(answer) = self.work_map(method, &route, &query, body, &l).await?
        {
            return Ok(answer);
        }
        if route.len() == 4
            && route[1] == "workspaces"
            && let Some(answer) = self.workspace(method, &route, &query, body, device, &l).await?
        {
            return Ok(answer);
        }
        if route.len() >= 4
            && route[1] == "uploads"
            && method == "POST"
            && let Some(answer) = self.upload(&route, body, device).await?
        {
            return Ok(answer);
        }
        if route.len() >= 4 && route[1] == "sessions" {
            return self.pane_route(method, &route, &query, body, device, &l).await;
        }
        Err(not_found())
    }

    fn push_route(
        &self,
        push: &PushHub,
        kind: &str,
        device: &str,
        body: Option<&RawValue>,
        l: &Locale,
    ) -> Result<Option<MobileReply>, MobileError> {
        use crate::identity::PushRegistrationResult;
        match kind {
            "register" => {
                let value = body_object(body, MAXIMUM_BODY)?;
                let handle = required(&value, "handle")?;
                if !recipients::valid_handle(handle) {
                    return Err(request(400, l.get("remote.error.badPushHandle")));
                }
                let Some(Value::Array(list)) = value.get("kinds") else { return Err(request(400, l.get("remote.error.badBody"))) };
                let names: Vec<&str> = list.iter().filter_map(Value::as_str).collect();
                let kinds = recipients::kinds(&names);
                match push.register(device, handle, &kinds) {
                    PushRegistrationResult::Saved => Ok(Some(ok())),
                    PushRegistrationResult::NeedsToken => Err(request(409, l.get("remote.error.pushNeedsToken"))),
                    PushRegistrationResult::Unsaved => Err(request(503, l.get("mobileRemote.device.saveFailed"))),
                }
            }
            "unregister" => {
                if !push.unregister(device) {
                    return Err(request(503, l.get("mobileRemote.device.saveFailed")));
                }
                Ok(Some(ok()))
            }
            "viewing" => {
                // No body, no sessionId and sessionId: null all clear the view.
                let empty = body.is_none_or(|raw| raw.get().trim() == "null");
                if empty {
                    push.viewing(device, None);
                } else {
                    let value = body_object(body, MAXIMUM_BODY)?;
                    match value.get("sessionId") {
                        None | Some(Value::Null) => push.viewing(device, None),
                        Some(Value::String(shown)) if wire::identifier(shown) => push.viewing(device, Some(shown.clone())),
                        _ => return Err(request(404, l.get("remote.error.paneNotFound"))),
                    }
                }
                Ok(Some(ok()))
            }
            _ => Ok(None),
        }
    }

    fn info(&self) -> MobileReply {
        let mut capabilities: Vec<String> = CAPABILITIES.iter().map(|c| c.to_string()).collect();
        for extra in extra_capabilities(Some(self.desk.extra_capabilities()), self.options.screen_share) {
            if !capabilities.contains(&extra) {
                capabilities.push(extra);
            }
        }
        let direct = self.desk.direct_endpoints();
        reply(
            200,
            object(|o| {
                o.prop_str("hostId", Some(&self.desk.host_id()));
                o.prop_str("hostName", Some(&self.desk.host_name()));
                o.prop_str("appVersion", Some(&self.desk.app_version()));
                o.prop_str("platform", Some(self.options.platform));
                o.name("capabilities");
                o.strings(&capabilities);
                o.name("direct");
                o.strings(&direct);
            }),
        )
    }

    /// `GET /m1/state` and `GET /m1/sessions/{id}`: each scope waits on its own revision, 100 ms at a time, at most `wait` seconds.
    async fn poll(self: &Arc<Self>, route: &[&str], query: &Query) -> Routed {
        let detail = route[1] == "sessions";
        if query.iter().any(|(k, _)| {
            !matches!(k.as_str(), "since" | "wait") && !(detail && matches!(k.as_str(), "structure" | "roles" | crate::body_delta::QUERY))
        }) {
            return Err(request(400, "Unexpected poll argument."));
        }
        // MC-53 (`bodyDelta`): the revision of the entry window the phone holds (empty: none yet, but it takes deltas).
        let body: Option<Option<String>> = match query_get(query, crate::body_delta::QUERY) {
            None => None,
            Some(value) => Some(
                crate::body_delta::since_value(value)
                    .ok_or_else(|| request(400, format!("Invalid {}.", crate::body_delta::QUERY)))?
                    .map(str::to_string),
            ),
        };
        let since = number(query, "since", 0, i64::MAX, 0)?;
        let wait = number(query, "wait", 0, 10, 0)?;
        let asked = if query_get(query, "structure").is_some() { Some(number(query, "structure", 0, 999_999_999_999, 0)?) } else { None };
        let roles = query_get(query, "roles").is_some() && number(query, "roles", 0, 999_999_999_999, 0)? > 0;
        let id = detail.then(|| route[2].to_string());
        if let Some(id) = &id {
            self.session(id)?;
        }
        // Each reading of the desk is off the caller's thread (the desk may take the app's locks and read the disk).
        let current = |inner: Arc<Inner>, id: Option<String>| async move {
            blocking(move || {
                inner.observe()?;
                let notes = inner.notes();
                Ok(match &id {
                    None => notes.revisions.state(),
                    Some(id) => notes.revisions.session(id),
                })
            })
            .await
        };
        if current(self.clone(), id.clone()).await? <= since && wait > 0 {
            let until = tokio::time::Instant::now() + Duration::from_secs(wait as u64);
            loop {
                // Register interest before checking the revision so no `changed()` notification between check and await is lost.
                let notified = self.notify.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                if current(self.clone(), id.clone()).await? > since || tokio::time::Instant::now() >= until {
                    break;
                }
                if tokio::time::timeout_at(until, notified).await.is_err() {
                    break;
                }
            }
        }
        let inner = self.clone();
        blocking(move || match id {
            None => Ok(reply(200, inner.state())),
            Some(id) => {
                let pane = inner.session(&id)?;
                Ok(inner.session_reply(&pane, asked, roles, body.as_ref().map(Option::as_deref)))
            }
        })
        .await
    }

    async fn app_update(&self, device: &str, body: Option<&RawValue>, l: &Locale) -> Routed {
        if device == mighty_platform::pairing::identity_file::LEGACY {
            return Err(MobileError::request(403, l.get("phone.screenShare.refusal.legacyClient"), Some("legacyClient")));
        }
        let value = body_object(body, MAXIMUM_BODY)?;
        let install = text(&value, "action") == Some("install");
        if !install && text(&value, "action") != Some("check") {
            return Err(request(400, l.get("appUpdate.mobile.badAction")));
        }
        let snapshot = self.desk.snapshot();
        let Some(state) = self.app_update_phone(&snapshot) else { return Err(request(409, l.get("appUpdate.mobile.unavailable"))) };
        if install && state.running > 0 && value.get("confirm") != Some(&Value::Bool(true)) {
            return Err(request(409, l.get_with("appUpdate.mobile.runningRequests", &[("count", &state.running.to_string())])));
        }
        self.desk.app_update_act(install).await?;
        let after = self.app_update_phone(&self.desk.snapshot()).unwrap_or(state);
        Ok(reply(200, after.to_json()))
    }

    async fn work_map(
        &self,
        method: &str,
        route: &[&str],
        query: &Query,
        body: Option<&RawValue>,
        l: &Locale,
    ) -> Result<Option<MobileReply>, MobileError> {
        // Only this PC's workspaces: a remote one's history is on its host.
        let snapshot = self.desk.snapshot();
        let Some(mapped) =
            local_workspaces(&snapshot, self.allowed(&snapshot).as_deref()).into_iter().find(|w| w.id() == route[2]).cloned()
        else {
            return Err(request(404, "Workspace not found."));
        };
        if method == "GET" && route.len() == 4 && query.is_empty() {
            let Some(read) = self.desk.work_map_read(&mapped) else { return Err(request(404, "Workspace not found.")) };
            let model = read.await?;
            return Ok(Some(reply(200, phone_work_map(&model))));
        }
        if method == "POST" && route.len() == 5 && route[4] == "summarize" {
            let value = body_object(body, MAXIMUM_BODY)?;
            let key = required(&value, "key")?.to_string();
            let Some(start) = self.desk.work_map_summarize(&mapped, &key) else { return Err(request(404, "Workspace not found.")) };
            return match start.await? {
                WorkMapStart::Started => Ok(Some(reply(202, object(|o| o.prop_bool("ok", true))))),
                WorkMapStart::Busy => Err(request(409, l.get("workMap.error.busy"))),
                WorkMapStart::UnknownKey => Err(request(400, "Unknown version.")),
                WorkMapStart::Unavailable => Err(request(404, "Workspace not found.")),
            };
        }
        if self.desk.work_map_read(&mapped).is_none() {
            return Err(request(404, "Workspace not found."));
        }
        Ok(None)
    }

    async fn workspace(
        self: &Arc<Self>,
        method: &str,
        route: &[&str],
        query: &Query,
        body: Option<&RawValue>,
        device: &str,
        l: &Locale,
    ) -> Result<Option<MobileReply>, MobileError> {
        let listing = method == "GET" && route[3] == "files";
        let files = method == "GET" && matches!(route[3], "files" | "file");
        let snapshot = self.desk.snapshot();
        let Some(workspace) =
            local_workspaces(&snapshot, self.allowed(&snapshot).as_deref()).into_iter().find(|w| w.id() == route[2]).cloned()
        else {
            return Err(if files { files::workspace_not_found(l) } else { request(404, "Workspace not found.") });
        };
        if method == "POST" && route[3] == "sessions" {
            let value = body_object(body, MAXIMUM_BODY)?;
            if required(&value, "kind")? != "claude" {
                return Err(request(409, "Create terminal panes on the desktop."));
            }
            let provider = text(&value, "provider").unwrap_or("claude");
            if !wire::PROVIDERS.contains(&provider) {
                return Err(request(400, "Unknown provider."));
            }
            let created = self
                .desk
                .action(MobileDesktopAction {
                    session_id: workspace.id().into(),
                    action: "create".into(),
                    body: Value::Object(value.clone()),
                    attachments: None,
                })
                .await?;
            self.changed();
            let created = answer_value(&created);
            return Ok(Some(reply(201, object(|o| prop_value(o, "sessionId", Some(&created))))));
        }
        if !files {
            return Ok(None);
        }
        // "fileChanges": the preview's changes=1, with the pane and run whose originals count, each optional.
        let unexpected = || request(400, "Unexpected file argument.");
        let changes = match (route[3] == "file", query_get(query, "changes")) {
            (true, Some("1")) => true,
            (true, Some(_)) => return Err(unexpected()),
            _ => false,
        };
        if query.iter().any(|(k, _)| k != "path" && !(changes && matches!(k.as_str(), "changes" | "session" | "run"))) {
            return Err(unexpected());
        }
        let session = query_get(query, "session");
        let run = query_get(query, "run");
        if session.is_some_and(|s| !wire::identifier(s)) || run.is_some_and(|r| !wire::identifier(r)) {
            return Err(unexpected());
        }
        let relative = files::validated_path(query_get(query, "path"), listing, l)?;
        let changes = changes.then(|| (session.map(str::to_string), run.map(str::to_string)));
        self.files(&workspace, relative, listing, device, changes, l).await.map(Some)
    }

    /// The read-only file routes: the folder and the file are read off the caller's thread two at a time; only an image's decode waits
    /// for the preview slot, one at a time, a newer preview from the same phone taking an older one's place. The work runs on a task of
    /// its own holding its slots until it ends: a phone that drops the request frees no slot while its reads or its decode still run.
    async fn files(
        self: &Arc<Self>,
        workspace: &Workspace,
        relative: String,
        listing: bool,
        device: &str,
        changes: Option<(Option<String>, Option<String>)>,
        l: &Locale,
    ) -> Routed {
        let permit = self.file_slots.clone().acquire_owned().await.map_err(|_| MobileError::Failed("closed".into()))?;
        let (inner, workspace, device, locale) = (self.clone(), workspace.clone(), device.to_string(), *l);
        tokio::spawn(async move { inner.file_work(permit, workspace, relative, listing, device, changes, locale).await })
            .await
            .map_err(|_| MobileError::Failed("the file route stopped".into()))?
    }

    #[allow(clippy::too_many_arguments)]
    async fn file_work(
        self: Arc<Self>,
        permit: tokio::sync::OwnedSemaphorePermit,
        workspace: Workspace,
        relative: String,
        listing: bool,
        device: String,
        changes: Option<(Option<String>, Option<String>)>,
        locale: Locale,
    ) -> Routed {
        let root = PathBuf::from(workspace.path());
        let id = workspace.id().to_string();
        if listing {
            let found = blocking(move || {
                let _slot = permit;
                files::listing(&id, &relative, &root, &locale)
            })
            .await?;
            return Ok(reply(200, files::fitted_listing(found).to_json()));
        }
        // A pane of another workspace never lends this one its originals; a delegated child's file and its originals are read in its
        // worktree folder, where they were captured.
        let desk = self.desk.clone();
        let (stage_id, stage_relative, scope) = (id.clone(), relative.clone(), changes.clone());
        let (mut stage, pane, folder) = blocking(move || {
            let snapshot = desk.snapshot();
            let pane = scope
                .as_ref()
                .and_then(|(session, _)| session.clone())
                .filter(|session| snapshot.sessions().iter().any(|s| s.id() == session && s.workspace_id() == stage_id));
            let folder = pane.as_ref().and_then(|p| desk.run_folder(p)).map(PathBuf::from).unwrap_or(root);
            let stage = files::prepare(&stage_id, &stage_relative, &folder, mighty_composer::files::file_handles().as_ref(), &locale)?;
            Ok((stage, pane, folder))
        })
        .await?;
        // Only text has changes, worked out in the same two slots.
        if let Some((_, run)) = &changes
            && stage.image.is_none()
            && matches!(stage.preview.kind.as_str(), "source" | "markdown")
        {
            let originals = pane.as_ref().and_then(|p| self.desk.originals_run(p, run.as_deref()));
            stage.preview.changes =
                self.desk.file_changes(folder.to_string_lossy().into_owned(), relative.clone(), pane.clone(), originals).await?;
        }
        drop(permit);
        let preview = stage.preview;
        let Some(mut file) = stage.image else { return Ok(reply(200, files::fitted_preview(preview).to_json())) };
        if !self.previews.acquire(&device).await {
            return Err(files::superseded(&locale));
        }
        let held = Held(self.previews.clone());
        let bytes = blocking(move || files::image_bytes(&mut file, &locale)).await?;
        let answer = self.image(preview, bytes).await;
        drop(held);
        Ok(answer)
    }

    async fn image(&self, preview: MobileFilePreview, bytes: Option<Vec<u8>>) -> MobileReply {
        let Some(bytes) = bytes else { return reply(200, MobileFilePreview { reason: Some("tooLarge".into()), ..preview }.to_json()) };
        // An svg that would load something besides itself is never drawn.
        if mighty_files::classify::file_extension(&preview.name) == "svg" && mighty_composer::svg::svg_loads_external_content(&bytes) {
            return reply(200, MobileFilePreview { reason: Some("undecodable".into()), ..preview }.to_json());
        }
        let drawn = self.desk.image_preview(preview.name.clone(), bytes).await;
        match drawn.image {
            // A picture sent as it is (no sizes: C#'s router without a decoder) or its thumbnail with both sizes.
            Some(image) => {
                let sized = image.width > 0 || image.height > 0;
                reply(
                    200,
                    MobileFilePreview {
                        kind: "image".into(),
                        mime: Some(image.mime),
                        width: sized.then_some(image.width),
                        height: sized.then_some(image.height),
                        thumbnail_width: sized.then_some(image.thumbnail_width),
                        thumbnail_height: sized.then_some(image.thumbnail_height),
                        data: Some(image.data),
                        ..preview
                    }
                    .to_json(),
                )
            }
            None => {
                reply(200, MobileFilePreview { reason: Some(drawn.reason.unwrap_or_else(|| "undecodable".into())), ..preview }.to_json())
            }
        }
    }

    async fn upload(&self, route: &[&str], body: Option<&RawValue>, device: &str) -> Result<Option<MobileReply>, MobileError> {
        if route.len() == 5
            && route[3] == "chunks"
            && let Some(index) = parse_long(route[4]).filter(|i| i32::try_from(*i).is_ok())
        {
            let value = body_object(body, MAXIMUM_CHUNK_BODY)?;
            let encoded: String = required(&value, "dataBase64")?.chars().filter(|c| !matches!(c, ' ' | '\t' | '\r' | '\n')).collect();
            let data = base64::engine::general_purpose::STANDARD.decode(encoded).map_err(|_| MobileError::Invalid)?;
            if data.len() > CHUNK_SIZE {
                return Err(request(413, "Chunk exceeds limit."));
            }
            let (uploads, upload, owner) = (self.uploads.clone(), route[2].to_string(), device.to_string());
            let received = blocking(move || uploads.append(&upload, &owner, index, &data)).await?;
            return Ok(Some(reply(
                200,
                object(|o| {
                    o.prop_bool("ok", true);
                    o.prop_int("received", received as i64);
                }),
            )));
        }
        if route.len() == 4 && route[3] == "complete" {
            let attachment = self.uploads.complete(route[2], device)?;
            return Ok(Some(reply(
                200,
                object(|o| {
                    o.name("attachment");
                    attachment.write(o);
                }),
            )));
        }
        if route.len() == 4 && route[3] == "cancel" {
            self.uploads.cancel(route[2], device)?;
            return Ok(Some(ok()));
        }
        Ok(None)
    }

    async fn act(&self, session: &str, action: &str, body: Value) -> Result<ActionAnswer, MobileError> {
        self.desk.action(MobileDesktopAction { session_id: session.into(), action: action.into(), body, attachments: None }).await
    }

    async fn pane_route(
        self: &Arc<Self>,
        method: &str,
        route: &[&str],
        query: &Query,
        body: Option<&RawValue>,
        device: &str,
        l: &Locale,
    ) -> Routed {
        let pane = self.session(route[2])?;
        let id = pane.id().to_string();
        if method == "GET" && route.len() == 4 && route[3] == "entries" {
            if query.iter().any(|(k, _)| !matches!(k.as_str(), "before" | "limit")) {
                return Err(request(400, "Unexpected history argument."));
            }
            let limit = number(query, "limit", 1, mighty::MAXIMUM_PAGE_LIMIT, mighty::DEFAULT_PAGE_LIMIT)? as usize;
            let entries = pane.logs();
            let (page, more) = match query_get(query, "before") {
                Some(before) => mighty::page(entries, before, limit),
                None => (entries[entries.len().saturating_sub(limit)..].to_vec(), entries.len() > limit),
            };
            return Ok(reply(
                200,
                object(|o| {
                    o.name("entries");
                    o.list(&mighty::phone_entries(&page));
                    o.prop_bool("hasMore", more);
                }),
            ));
        }
        if pane.kind() != "claude" {
            return Err(request(409, "Use the desktop for this pane."));
        }
        if method == "GET" && route.len() == 4 && route[3] == "commands" && query.is_empty() {
            let (inner, locale) = (self.clone(), *l);
            return blocking(move || inner.commands(&pane, &locale)).await;
        }
        if method == "GET" && route.len() == 4 && route[3] == "prompts" && query.is_empty() {
            let prompts = self.desk.saved_prompts(pane.workspace_id());
            let mut out = JsonOut::new();
            out.begin_object();
            out.prop_int("protocol", 1);
            out.name("prompts");
            out.begin_array();
            for p in &prompts {
                out.begin_object();
                out.prop_str("id", Some(&p.id));
                out.prop_str("name", Some(&p.name));
                out.prop_str("text", Some(&p.text));
                out.prop_str("scope", Some(if p.workspace_id.is_some() { "workspace" } else { "all" }));
                out.end_object();
            }
            out.end_array();
            out.end_object();
            return Ok(reply(200, out.finish()));
        }
        // The worktree bar's actions (the Swift Mac host's route, now the Rust host's too): `{ protocol, done, message?, confirm? }`.
        // A merge that something still warns about answers `confirm { title, message, warnings }` until the phone asks again with
        // `confirmed: true` (MC-27: asked once more, never blocked).
        if method == "POST" && route.len() == 4 && route[3] == "worktree" {
            let value = body_object(body, MAXIMUM_BODY)?;
            let action = required(&value, "action")?;
            if !matches!(action, "refresh" | "merge" | "commitMerge" | "undo" | "resolveConflict" | "discard") {
                return Err(request(400, "Unknown worktree action."));
            }
            let confirmed = matches!(value.get("confirmed"), Some(Value::Bool(true)));
            let answer = self.act(&id, "worktree", json!({"action": action, "confirmed": confirmed})).await?;
            self.changed();
            return Ok(value_reply(200, &answer_value(&answer)));
        }
        // MC-27: the pre-merge checks of the pane's worktree (`mergeChecks`), read now.
        if method == "GET" && route.len() == 4 && route[3] == "checks" {
            if !query.is_empty() {
                return Err(request(400, "Unexpected query argument."));
            }
            let answer = self.act(&id, "checks", json!({})).await?;
            return Ok(value_reply(200, &answer_value(&answer)));
        }
        // MC-29: one compare run of the pane (`compare`): its children read now, and the compare view's actions on them.
        if route.len() == 5 && route[3] == "compare" {
            let group = route[4];
            if !wire::identifier(group) {
                return Err(request(404, "Compare run not found."));
            }
            if method == "GET" {
                if !query.is_empty() {
                    return Err(request(400, "Unexpected query argument."));
                }
                let answer = self.act(&id, "compare-view", json!({"compareId": group})).await?;
                return Ok(value_reply(200, &answer_value(&answer)));
            }
            if method == "POST" {
                let value = body_object(body, MAXIMUM_BODY)?;
                let action = required(&value, "action")?;
                if !matches!(action, "merge" | "discard" | "discardRest") {
                    return Err(request(400, "Unknown compare action."));
                }
                let child = text(&value, "childId");
                if child.is_some_and(|c| !wire::identifier(c)) || (action != "discardRest" && child.is_none()) {
                    return Err(request(400, "Invalid childId."));
                }
                let ids: Vec<&str> = match value.get("ids") {
                    None | Some(Value::Null) => vec![],
                    Some(Value::Array(list)) if list.len() <= 16 => list
                        .iter()
                        .map(|v| v.as_str().filter(|s| wire::identifier(s)))
                        .collect::<Option<_>>()
                        .ok_or_else(|| request(400, "Invalid ids."))?,
                    Some(_) => return Err(request(400, "Invalid ids.")),
                };
                if action == "discardRest" && ids.is_empty() {
                    return Err(request(400, "Invalid ids."));
                }
                let confirmed = matches!(value.get("confirmed"), Some(Value::Bool(true)));
                let forwarded = json!({"compareId": group, "action": action, "childId": child, "ids": ids, "confirmed": confirmed});
                let answer = self.act(&id, "compare-action", forwarded).await?;
                self.changed();
                return Ok(value_reply(200, &answer_value(&answer)));
            }
        }
        if method == "POST" && route.len() == 4 {
            let operation = route[3];
            if operation == "permission" {
                // An Antigravity card is answered here too: allow alone is once, with always always; always never comes without allow.
                let value = body_object(body, MAXIMUM_BODY)?;
                let request_id = required(&value, "requestId")?.to_string();
                let run_id = required(&value, "runId")?.to_string();
                if !wire::identifier(&request_id) || !wire::identifier(&run_id) {
                    return Err(request(400, l.get("remote.error.badPermissionId")));
                }
                let Some(Value::Bool(allow)) = value.get("allow").cloned() else { return Err(request(400, "Missing permission choice.")) };
                let always = match value.get("always") {
                    None | Some(Value::Bool(false)) | Some(Value::Null) => false,
                    Some(Value::Bool(true)) => true,
                    _ => return Err(request(400, "Invalid permission choice.")),
                };
                if always && !allow {
                    return Err(request(400, l.get("remote.error.alwaysNeedsAllow")));
                }
                if self.desk.pending_antigravity_card(&id, &request_id, &run_id).is_some() {
                    let decision = match mighty_run::antigravity::permissions::decision(allow, always) {
                        mighty_run::antigravity::permissions::PermissionDecision::Once => "once",
                        mighty_run::antigravity::permissions::PermissionDecision::Always => "always",
                        mighty_run::antigravity::permissions::PermissionDecision::Deny => "deny",
                    };
                    let answer = json!({"requestId": request_id, "runId": run_id, "decision": decision});
                    if let ActionAnswer::Value(Value::String(failure)) = self.act(&id, "antigravityPermission", answer).await? {
                        return Err(request(409, failure));
                    }
                    self.changed();
                    return Ok(ok());
                }
                if always {
                    return Err(request(409, l.get("remote.error.notAntigravityPermission")));
                }
            }
            if matches!(operation, "permission" | "answers") {
                let value = body_object(body, MAXIMUM_BODY)?;
                let request_id = required(&value, "requestId")?;
                let run_id = required(&value, "runId")?;
                let pending = self.pending(&id).into_iter().find(|p| p.id == request_id && p.run_id == run_id);
                if pending.is_none() || !self.desk.is_session_running(&id) {
                    return Err(request(409, "Request is no longer pending."));
                }
                if operation == "permission" {
                    let Some(Value::Bool(allow)) = value.get("allow") else { return Err(request(400, "Missing permission choice.")) };
                    self.desk.respond_to_tool_permission(&id, request_id, *allow).await?;
                } else {
                    let invalid = || request(400, "Invalid answers.");
                    let Some(Value::Object(answers)) = value.get("answers") else { return Err(invalid()) };
                    if answers.len() > 16 {
                        return Err(invalid());
                    }
                    let parsed = mighty::answers(answers).ok_or_else(invalid)?;
                    self.desk.answer_questionnaire(&id, request_id, parsed).await?;
                }
                self.changed();
                return Ok(ok());
            }
            if operation == "plan" {
                // One of the plan card's four answers (macOS MobilePlanAnswer).
                let value = body_object(body, MAXIMUM_BODY)?;
                let request_id = required(&value, "requestId")?;
                let run_id = required(&value, "runId")?;
                if !wire::identifier(request_id) || !wire::identifier(run_id) {
                    return Err(request(400, l.get("plan.error.invalidRequest")));
                }
                let feedback = text(&value, "feedback");
                let decision = match text(&value, "decision") {
                    Some("approveAutoEdit") => PlanDecision::approve_auto_edit(),
                    Some("approveConfirmEach") => PlanDecision::approve_confirm_each(),
                    Some("revise") => PlanDecision::revise(feedback.unwrap_or("")),
                    Some("cancel") => PlanDecision::cancel(),
                    _ => return Err(request(400, l.get("plan.error.invalidDecision"))),
                };
                if decision.kind == "revise" {
                    plan_mode::validated_feedback(feedback).map_err(|e| request(400, e.message()))?;
                }
                let pending = self.pending(&id).into_iter().find(|p| p.id == request_id && p.run_id == run_id);
                let Some(pending) = pending.filter(|_| self.desk.is_session_running(&id)) else {
                    return Err(request(409, "Request is no longer pending."));
                };
                if !pending.can_answer_plan {
                    return Err(request(409, l.get("plan.error.notPlan")));
                }
                self.desk.answer_plan(&id, request_id, decision).await?;
                self.changed();
                return Ok(ok());
            }
            if operation == "terminal-prompt" {
                // The answer to the card of a command waiting for input; the text of a secret is passed on and never kept.
                let value = body_object(body, MAXIMUM_BODY)?;
                let typed = TerminalPromptAnswer::from_phone(text(&value, "promptId"), text(&value, "answer"), text(&value, "text"))
                    .ok_or_else(|| request(400, l.get("terminalPrompt.error.badAnswer")))?;
                let forwarded = json!({"promptId": text(&value, "promptId"), "answer": typed.name, "text": typed.line});
                if !matches!(self.act(&id, operation, forwarded).await?, ActionAnswer::Value(Value::Bool(true))) {
                    return Err(request(409, l.get("terminalPrompt.error.gone")));
                }
                self.changed();
                return Ok(ok());
            }
            if operation == "uploads" {
                let value = body_object(body, MAXIMUM_BODY)?;
                // C#'s `TryGetInt32`: no size, or a number that is no 32-bit integer, is 400; a size that is no number throws
                // (`InvalidOperationException`: 409, the pane changed).
                let size = match value.get("size") {
                    None => return Err(request(400, "Invalid size.")),
                    Some(Value::Number(n)) => {
                        n.as_i64().filter(|s| i32::try_from(*s).is_ok()).ok_or_else(|| request(400, "Invalid size."))?
                    }
                    Some(_) => return Err(MobileError::Changed),
                };
                let name = required(&value, "name")?.to_string();
                let (uploads, session, device) = (self.uploads.clone(), id.clone(), device.to_string());
                let ticket = blocking(move || uploads.begin(&session, &device, &name, size)).await?;
                return Ok(reply(201, ticket.to_json()));
            }
            if operation == "submit" {
                let value = body_object(body, MAXIMUM_BODY)?;
                let ids: Vec<String> = match value.get("attachments") {
                    None | Some(Value::Null) => vec![],
                    Some(Value::Array(list)) => {
                        list.iter().map(|v| v.as_str().map(str::to_string)).collect::<Option<_>>().ok_or(MobileError::Invalid)?
                    }
                    Some(_) => return Err(MobileError::Invalid),
                };
                request_text(&value, !ids.is_empty())?;
                if let Some(mode) = text(&value, "mode")
                    && !matches!(mode, "queue" | "steer")
                {
                    return Err(request(400, "Unknown submit mode."));
                }
                // The slot, the claim and the action on a task of their own: a phone that drops the request neither frees the slot
                // nor leaves the claim's uploads claimable while the action still runs; the claim is spent only once it was accepted.
                let permit = self.submit_slots.clone().acquire_owned().await.map_err(|_| MobileError::Failed("closed".into()))?;
                let (inner, device) = (self.clone(), device.to_string());
                let result = tokio::spawn(async move {
                    let _slot = permit;
                    let (uploads, session, owner) = (inner.uploads.clone(), id.clone(), device);
                    let claim = blocking(move || uploads.claim(&ids, &session, &owner)).await?;
                    let result = inner
                        .desk
                        .action(MobileDesktopAction {
                            session_id: id,
                            action: "submit".into(),
                            body: Value::Object(value),
                            attachments: Some(claim.attachments.clone()),
                        })
                        .await;
                    inner.uploads.finish(&claim, result.is_ok());
                    if result.is_ok() {
                        inner.changed();
                    }
                    result
                })
                .await
                .map_err(|_| MobileError::Failed("the submit stopped".into()))??;
                return Ok(reply(202, submit_reply(&result)));
            }
            if operation == "prompt-send" {
                let value = body_object(body, MAXIMUM_BODY)?;
                let prompt_id = required(&value, "promptId")?.to_string();
                if !wire::identifier(&prompt_id) {
                    return Err(request(400, "Invalid saved prompt id."));
                }
                if let Some(mode) = text(&value, "mode")
                    && !matches!(mode, "queue" | "steer")
                {
                    return Err(request(400, "Unknown submit mode."));
                }
                let permit = self.submit_slots.clone().acquire_owned().await.map_err(|_| MobileError::Failed("closed".into()))?;
                let inner = self.clone();
                let answer = tokio::spawn(async move {
                    let _slot = permit;
                    let result = inner
                        .desk
                        .action(MobileDesktopAction {
                            session_id: id,
                            action: "prompt-send".into(),
                            body: Value::Object(value),
                            attachments: None,
                        })
                        .await;
                    if result.is_ok() {
                        inner.changed();
                    }
                    result
                })
                .await
                .map_err(|_| MobileError::Failed("the prompt send stopped".into()))??;
                let payload = answer_value(&answer);
                return Ok(reply(
                    202,
                    object(|o| {
                        let outcome = payload.get("outcome").and_then(Value::as_str).unwrap_or("started");
                        o.prop_str("accepted", Some(outcome));
                        o.prop_str("outcome", Some(outcome));
                        o.prop_str("text", payload.get("text").and_then(Value::as_str).or(Some("")));
                        o.prop_str("entryId", payload.get("entryId").and_then(Value::as_str));
                        o.prop_str("runId", payload.get("runId").and_then(Value::as_str));
                    }),
                ));
            }
            if operation == "setAskTarget" {
                // The pane's "send to" choice: a target id it offers, or null for Claude. Which ids it offers is the desktop's.
                let value = body_object(body, MAXIMUM_BODY)?;
                if value.get("target").is_some_and(|t| !(t.is_string() || t.is_null())) {
                    return Err(request(400, l.get("remote.error.badBody")));
                }
                self.act(&id, operation, Value::Object(value)).await?;
                self.changed();
                return Ok(ok());
            }
            if matches!(operation, "stop" | "rename" | "close" | "settings" | "command" | "guided") {
                let value = if matches!(operation, "stop" | "close") { Map::new() } else { body_object(body, MAXIMUM_BODY)? };
                if operation == "rename" && text(&value, "titleMode") != Some("auto") {
                    let title = text(&value, "title").map(|t| t.trim());
                    let ok = title.is_some_and(|t| (1..=80).contains(&t.encode_utf16().count()) && !t.chars().any(char::is_control));
                    if !ok {
                        return Err(request(400, "Invalid pane title."));
                    }
                }
                if operation == "command" && !matches!(text(&value, "action"), Some("clear" | "usage" | "help")) {
                    return Err(request(400, "Unknown command."));
                }
                if operation == "guided" {
                    mighty::guided_request(&value, l)?;
                }
                let result = self.act(&id, operation, Value::Object(value)).await?;
                self.changed();
                if operation == "close" {
                    self.uploads.discard_session(&id);
                }
                let value = answer_value(&result);
                return Ok(match operation {
                    "stop" => reply(200, object(|o| prop_value(o, "stopped", Some(&value)))),
                    "guided" => reply(202, submit_reply(&result)),
                    "command" => reply(
                        200,
                        object(|o| {
                            o.prop_bool("ok", true);
                            prop_value(o, "message", Some(&value));
                        }),
                    ),
                    _ => ok(),
                });
            }
        }
        if method == "POST" && route.len() == 5 && route[3] == "queue" && route[4] == "run-next" {
            self.act(&id, "queue-next", json!({})).await?;
            self.changed();
            return Ok(ok());
        }
        // 바로 실행 on one row: "worktree" or "here" in ran; 404 for a row no longer queued, 409 for one that cannot run now.
        if method == "POST" && route.len() == 6 && route[3] == "queue" && route[5] == "run-now" {
            if !wire::identifier(route[4]) {
                return Err(request(404, l.get("remote.error.queueItemNotFound")));
            }
            let ran = self.act(&id, "queue-run-now", json!({"id": route[4]})).await?;
            self.changed();
            let ran = answer_value(&ran);
            return Ok(reply(
                200,
                object(|o| {
                    o.prop_bool("ok", true);
                    prop_value(o, "ran", Some(&ran));
                }),
            ));
        }
        if method == "POST" && route.len() == 6 && route[3] == "queue" && route[5] == "remove" {
            // A row held for the pane is refused, and nothing changes.
            if let Some(refusal) = mighty_delegation::delivery::remove_refusal(route[4], &self.desk.held_items(&id)) {
                return Err(MobileError::request(409, l.get("queue.heldNotRemovable"), Some(refusal.name())));
            }
            self.act(&id, "queue-remove", json!({"id": route[4]})).await?;
            self.changed();
            return Ok(ok());
        }
        // MC-49 (`queueEdit`): a queued request's words (`{text}`), or its place (`{to}`, 0 the head, or `{before}`, another queued
        // request's id). A row held for the pane is neither edited nor moved; the desktop's own rules refuse the rest (409).
        if method == "POST" && route.len() == 6 && route[3] == "queue" && matches!(route[5], "edit" | "move") {
            if !wire::identifier(route[4]) {
                return Err(request(404, l.get("remote.error.queueItemNotFound")));
            }
            if self.desk.held_items(&id).iter().any(|h| h.id == route[4]) {
                return Err(request(409, l.get("queue.editRefused")));
            }
            let value = body_object(body, MAXIMUM_BODY)?;
            let bad = || request(400, l.get("remote.error.badBody"));
            let action = if route[5] == "edit" {
                let words = value.get("text").and_then(Value::as_str).ok_or_else(bad)?;
                if words.contains('\0') {
                    return Err(bad());
                }
                json!({"id": route[4], "text": words})
            } else {
                match (value.get("to"), value.get("before")) {
                    (Some(to), None) if to.as_u64().is_some_and(|to| to <= 1_000) => json!({"id": route[4], "to": to}),
                    (None, Some(Value::String(before))) if wire::identifier(before) => json!({"id": route[4], "before": before}),
                    _ => return Err(bad()),
                }
            };
            self.act(&id, if route[5] == "edit" { "queue-edit" } else { "queue-move" }, action).await?;
            self.changed();
            return Ok(ok());
        }
        Err(not_found())
    }

    /// `GET /m1/sessions/{id}/commands`: the pane's slash commands but the ones the phone cannot use.
    fn commands(&self, pane: &RunSession, l: &Locale) -> Routed {
        let snapshot = self.desk.snapshot();
        let workspace = snapshot.workspace(pane.workspace_id()).cloned().ok_or(MobileError::Changed)?;
        let found = slash::commands(pane.provider(), Some(std::path::Path::new(workspace.path())), &home(), &slash::DiskFiles, l);
        let mut out = JsonOut::new();
        out.begin_object();
        out.prop_int("protocol", 1);
        out.name("commands");
        out.begin_array();
        for c in found.iter().filter(|c| {
            !matches!(
                c.action,
                Some(
                    SlashCommandAction::OpenPlugins
                        | SlashCommandAction::OpenSettings
                        | SlashCommandAction::SetModel
                        | SlashCommandAction::SetPermission
                        | SlashCommandAction::ManagePrompts
                        | SlashCommandAction::InsertPrompt
                        | SlashCommandAction::OpenIssues
                )
            )
        }) {
            out.begin_object();
            out.prop_str("name", Some(&c.invocation));
            out.prop_str("description", Some(&c.description));
            out.prop_str("source", Some(&c.origin.name().to_lowercase()));
            out.prop_str("argumentHint", mighty::argument_hint(c.argument, l).as_deref());
            let action = match c.action {
                Some(SlashCommandAction::NewConversation) => Some("clear"),
                Some(SlashCommandAction::ShowUsage) => Some("usage"),
                Some(SlashCommandAction::Help) => Some("help"),
                Some(SlashCommandAction::Rename) => Some("rename"),
                _ => c.argument.map(|a| match a {
                    SlashArgument::Model => "model",
                    SlashArgument::Permission => "permission",
                }),
            };
            out.prop_str("action", action);
            out.end_object();
        }
        out.end_array();
        out.end_object();
        Ok(reply(200, out.finish()))
    }
}

/// The preview slot, released however the decode ended.
struct Held(PreviewSlot);

impl Drop for Held {
    fn drop(&mut self) {
        self.0.release();
    }
}

/// `Unread(pane)`: a pane's digest from its record alone.
fn unread(pane: &RunSession) -> MobilePaneDigest {
    MobilePaneDigest {
        editable: pane.status() != "running",
        options: 0,
        mighty: if pane.kind() == "claude" { mighty::digest(pane) } else { 0 },
    }
}

/// The phone's `queued` rows (`MobileQueuedItem`: `runNow` and `runNowReason` only when the row has a button; `held` and `locked`
/// only when true, MC-49).
fn write_queued(out: &mut JsonOut, rows: &[crate::phone::desk::MobileQueuedItem]) {
    out.begin_array();
    for row in rows {
        out.begin_object();
        out.prop_str("id", Some(&row.id));
        out.prop_str("text", Some(&row.text));
        out.prop_str("runNow", row.run_now.as_deref());
        out.prop_str("runNowReason", row.run_now_reason.as_deref());
        if row.held {
            out.prop_bool("held", true);
        }
        if row.locked {
            out.prop_bool("locked", true);
        }
        out.end_object();
    }
    out.end_array();
}

/// A pane summary's `worktree` (`MobilePaneWorktree`, as the Swift Mac host writes it, with the Rust host's additions): branch and base
/// at most 200 characters, the counts only once read (0…99,999), the flags only when true, the port block and the running run actions
/// only when there are any (`portBase`, `portCount`, `actions [{ name, url? }]`, MC-25).
fn write_worktree(out: &mut JsonOut, bar: &crate::phone::desk::MobilePaneWorktree) {
    let short = |text: &str| text.chars().take(200).collect::<String>();
    let count = |value: Option<i64>| value.map(|v| v.clamp(0, 99_999));
    out.name("worktree");
    out.begin_object();
    out.prop_str("branch", Some(&short(&bar.branch)));
    out.prop_str("base", Some(&short(&bar.base)));
    out.prop_int("port", i64::from(bar.port));
    out.prop_opt_int("ahead", count(bar.ahead));
    out.prop_opt_int("behind", count(bar.behind));
    out.prop_opt_int("changed", count(bar.changed));
    out.prop_str("setup", bar.setup.as_deref());
    if bar.can_undo {
        out.prop_bool("canUndo", true);
    }
    if bar.conflict {
        out.prop_bool("conflict", true);
    }
    if bar.busy {
        out.prop_bool("busy", true);
    }
    out.prop_int("children", bar.children.max(0));
    if let (Some(base), Some(count)) = (bar.port_base, bar.port_count)
        && base > 0
        && count > 0
    {
        out.prop_int("portBase", i64::from(base));
        out.prop_int("portCount", i64::from(count));
    }
    if !bar.actions.is_empty() {
        out.name("actions");
        out.begin_array();
        for action in bar.actions.iter().take(20) {
            out.begin_object();
            out.prop_str("name", Some(&action.name.chars().take(80).collect::<String>()));
            out.prop_str("url", action.url.as_deref().filter(|u| u.starts_with("http://") || u.starts_with("https://")));
            out.end_object();
        }
        out.end_array();
    }
    out.end_object();
}

/// A desk's answer written as a reply object: `protocol` first, then its own members (an answer that is no object is `ok: true`).
fn value_reply(status: u16, value: &Value) -> MobileReply {
    reply(
        status,
        object(|o| match value {
            Value::Object(map) => {
                for (key, member) in map {
                    if key != "protocol" {
                        prop_value(o, key, Some(member));
                    }
                }
            }
            _ => o.prop_bool("ok", true),
        }),
    )
}

/// `Mighty(mighty, structure, tag)`: the payload with the structure fields a phone asked for — the tag always, the value when it travels.
fn with_structure(mighty: Option<&Value>, structure: Option<&str>, tag: Option<i64>) -> Option<Value> {
    let mighty = mighty?;
    let Some(tag) = tag else { return Some(mighty.clone()) };
    let mut value = match mighty {
        Value::Object(map) => map.clone(),
        other => return Some(other.clone()),
    };
    if let Some(structure) = structure.and_then(|s| serde_json::from_str::<Value>(s).ok()) {
        value.insert("structure".into(), structure);
    }
    value.insert("structureTag".into(), json!(tag));
    Some(Value::Object(value))
}

/// `PhoneWorkMap`: the model without each commit's file list (its count stays), with `protocol`.
pub fn phone_work_map(model: &mighty_workmap::WorkMapModel) -> String {
    let mut node = model.to_value();
    if let Some(versions) = node.get_mut("versions").and_then(Value::as_array_mut) {
        for version in versions {
            if let Some(commits) = version.get_mut("commits").and_then(Value::as_array_mut) {
                for commit in commits {
                    if let Some(commit) = commit.as_object_mut() {
                        commit.shift_remove("files");
                    }
                }
            }
        }
    }
    if let Some(map) = node.as_object_mut() {
        map.insert("protocol".into(), json!(1));
    }
    let mut out = JsonOut::new();
    out.value(&node);
    out.finish()
}

/// The phone's queued rows from delegation's (`DelegationQueueRows.MobileItems`).
impl From<mighty_delegation::delivery::MobileQueuedRow> for crate::phone::desk::MobileQueuedItem {
    fn from(row: mighty_delegation::delivery::MobileQueuedRow) -> Self {
        crate::phone::desk::MobileQueuedItem {
            id: row.id,
            text: row.text,
            run_now: row.run_now.map(str::to_string),
            run_now_reason: row.run_now_reason.map(str::to_string),
            held: false,
            locked: false,
        }
    }
}

/// The phone's background row from the strip's (`PlanCardSupport.MobileBackgroundWork`).
impl From<mighty_graph::view::mobile::MobileBackgroundRow> for crate::phone::desk::MobileBackgroundWork {
    fn from(row: mighty_graph::view::mobile::MobileBackgroundRow) -> Self {
        crate::phone::desk::MobileBackgroundWork {
            id: row.id,
            kind: if row.run { "run" } else { "agent" }.into(),
            title: row.title,
            status: "running".into(),
            started_at: row.started_at,
            command: row.command,
            output_tail: row.output_tail,
        }
    }
}
