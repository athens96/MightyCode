//! The remote mirror on the sharing host (C# `RemoteHostService.Mirror`, `IRemoteMirrorHost`; M/Remote/RemoteHostService.swift
//! `serveMirror`): `GET /v2/mirror[?since=]`, `GET /v2/mirror/sessions/{id}`, `POST /v2/mirror/sessions/{id}/submit|stop|permission|
//! questions|plan|close`, `POST /v2/mirror/sessions/{id}/queue/{item}/remove|run-now` (MC-49) and
//! `POST /v2/mirror/workspaces/{id}/sessions`: the shared workspaces' own agent panes, read and driven as the
//! phone does. A pane of a workspace not shared is unknown here (404), whatever its id. Every request has passed the /v2/ checks (key,
//! version, peer, rate) before it gets here.

use std::collections::HashSet;
use std::sync::Arc;

use indexmap::IndexMap;
use mighty_core::attachment::RunAttachment;
use mighty_core::wire;
use mighty_platform::BoxFuture;
use mighty_run::model::StartRunRequest;
use mighty_run::plan_mode::PlanDecision;
use mighty_run::questionnaire::UserQuestionAnswer;
use mighty_run::strings::get;

use super::{
    MirrorPane, MirrorPaneInfo, MirrorSummary, WireMirrorCreate, WireMirrorCreated, WireMirrorDetail, WireMirrorOutcome,
    WireMirrorPermission, WireMirrorQuestions, WireMirrorState, WireMirrorSubmit, known_provider, protocol, support,
};
use crate::remote::host::{HostFailure, RemoteHost, lock, reply};
use crate::remote::http_server::{HttpRequest, HttpResponse};
use crate::remote::text;
use crate::remote::wire::{RemoteSharedWorkspace, RemoteWireAnswer, RemoteWirePlan, protocol as remote_protocol};

/// A pane's detail through the relay stays under this many bytes of session JSON: one relay frame (1 MiB with its 28-byte seal) less
/// room for the detail's and the reply's envelopes.
pub const RELAY_DETAIL_BYTES: usize = crate::relay::cipher::MAXIMUM_PLAINTEXT - 64 * 1024;

/// What an action on a host pane refused with (C#: the exception's message, answered 409).
pub type Refusal = String;

/// C# `IRemoteMirrorHost`: this app's panes as a connected client reads and drives them. Only panes of shared workspaces are ever asked
/// for; the host checks that before every call. A refusal an action answers is the host's 409, its words the reason.
pub trait MirrorHost: Send + Sync {
    /// `SummariesAsync`: every agent pane of these workspaces, in the order the host lists them.
    fn summaries(&self, workspace_ids: HashSet<String>) -> BoxFuture<'_, Vec<MirrorSummary>>;
    /// `DetailAsync`: one pane in full with its revision; None when it is gone or not in these workspaces.
    fn detail(&self, session_id: String, workspace_ids: HashSet<String>) -> BoxFuture<'_, Option<MirrorPane>>;
    /// `PaneInfoAsync`: which workspace an agent pane of this app is in; None when there is no such agent pane.
    fn pane_info(&self, session_id: String) -> BoxFuture<'_, Option<MirrorPaneInfo>>;
    /// `SubmitAsync`: a request into the host pane, as the phone's send: `started`, `steered` or `queued`.
    fn submit(&self, session_id: String, text: String, attachments: Vec<RunAttachment>) -> BoxFuture<'_, Result<String, Refusal>>;
    fn stop(&self, session_id: String) -> BoxFuture<'_, Result<(), Refusal>>;
    fn permission(
        &self,
        session_id: String,
        request_id: String,
        run_id: String,
        allow: bool,
        always: bool,
    ) -> BoxFuture<'_, Result<(), Refusal>>;
    fn answers(
        &self,
        session_id: String,
        request_id: String,
        run_id: String,
        answers: IndexMap<String, UserQuestionAnswer>,
    ) -> BoxFuture<'_, Result<(), Refusal>>;
    fn plan(&self, session_id: String, request_id: String, run_id: String, decision: PlanDecision) -> BoxFuture<'_, Result<(), Refusal>>;
    /// `CreateSessionAsync`: a new agent pane in a shared workspace (the workspace's worktree mode decides without asking); its id.
    fn create_session(&self, workspace_id: String, provider: String) -> BoxFuture<'_, Result<String, Refusal>>;
    fn close(&self, session_id: String) -> BoxFuture<'_, Result<(), Refusal>>;
    /// MC-49: a queued request taken out of the host pane's queue, as the phone's `queue/{item}/remove`. A host that does not take it
    /// refuses it (and leaves `queueActions` out of its details, so a client never offers it).
    fn queue_remove(&self, session_id: String, item_id: String) -> BoxFuture<'_, Result<(), Refusal>> {
        let _ = (session_id, item_id);
        Box::pin(async { Err(get("remote.mirror.error.unavailable")) })
    }
    /// MC-49: 바로 실행 on a queued request of the host pane, as the phone's `queue/{item}/run-now`: what it did (`here`, `worktree`).
    fn queue_run_now(&self, session_id: String, item_id: String) -> BoxFuture<'_, Result<String, Refusal>> {
        let _ = (session_id, item_id);
        Box::pin(async { Err(get("remote.mirror.error.unavailable")) })
    }
}

fn bad_request() -> HostFailure {
    HostFailure::new(400, get("remote.host.error.badRequest"))
}

fn content_type(request: &HttpRequest) -> &str {
    request.headers.get("content-type").map_or("", |t| t.split(';').next().unwrap_or_default().trim())
}

/// `Body<T>(request)`: JSON only (415), read strictly (400).
fn body<T>(request: &HttpRequest, decode: impl FnOnce(&[u8]) -> Result<T, crate::remote::wire::BadBody>) -> Result<T, HostFailure> {
    if content_type(request) != "application/json" {
        return Err(HostFailure::new(415, get("remote.host.error.jsonOnly")));
    }
    decode(&request.body).map_err(|_| bad_request())
}

/// `?since=` with 0 to 64 lower-case hex digits, the list's one query.
fn since_of(query: &str) -> Option<&str> {
    let value = query.strip_prefix("?since=")?;
    (value.len() <= 64 && value.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))).then_some(value)
}

impl RemoteHost {
    /// `SetMirror(value)`: the app's panes a connected client mirrors; None takes them back (the routes answer 404 again).
    pub fn set_mirror(&self, value: Option<Arc<dyn MirrorHost>>) {
        lock(&self.0.gate).mirror = value;
    }

    /// `MirrorRefusal(pane, allowed, windowsHost)`: what a client may not send into, or answer for, a host pane while full access is off: a
    /// pane in full access, and in any mode a pane whose agent nothing confines on this host (`full_access_refusal`'s rules: every agent
    /// but Claude on Windows, and every agent but Claude and Codex on a Mac). None when it may.
    pub fn mirror_refusal(pane: &MirrorPaneInfo, allowed: bool, windows_host: bool) -> Option<String> {
        let request = StartRunRequest {
            settings: Some(mighty_core::model::RunSettings {
                permission_mode: pane.permission_mode.clone(),
                ..mighty_core::model::RunSettings::default()
            }),
            ..StartRunRequest::new(&wire::new_id(), &pane.workspace_id, "claude", "mirror").with_provider(&pane.provider)
        };
        RemoteHost::full_access_refusal(&request, allowed, windows_host)
    }

    /// `StillSharing(revision)`: the sharing this request came in on is still the one on (503 otherwise).
    fn still_sharing(&self, revision: u64) -> Result<(), HostFailure> {
        if lock(&self.0.gate).generation != revision {
            return Err(HostFailure::new(503, get("remote.host.error.stopped")));
        }
        Ok(())
    }

    /// `ServeMirrorAsync(request, route, query, revision)`: `route` is the path after `/v2/mirror`; `relay`: a paired computer's request
    /// through the relay, whose answer is one relay frame (a pane's detail is fitted under [`RELAY_DETAIL_BYTES`]).
    pub(crate) async fn serve_mirror(
        &self,
        request: &HttpRequest,
        route: &[&str],
        query: &str,
        revision: u64,
        relay: bool,
    ) -> Result<HttpResponse, HostFailure> {
        let host = lock(&self.0.gate).mirror.clone().ok_or_else(|| HostFailure::new(404, get("remote.mirror.error.unavailable")))?;
        let workspaces = self.shared_workspaces();
        let shared: HashSet<String> = workspaces.iter().map(|w| w.id().to_string()).collect();
        self.still_sharing(revision)?;
        if request.method == "GET" && route.is_empty() {
            let since = if query.is_empty() { None } else { Some(since_of(query).ok_or_else(bad_request)?.to_string()) };
            let sessions: Vec<MirrorSummary> = host
                .summaries(shared.clone())
                .await
                .into_iter()
                .filter(|s| shared.contains(&s.workspace_id))
                .take(protocol::MAXIMUM_SESSIONS)
                .collect();
            let listed: Vec<RemoteSharedWorkspace> = workspaces
                .iter()
                .map(|w| RemoteSharedWorkspace { id: w.id().into(), name: w.name().into(), path: w.path().into() })
                .collect();
            let current = support::state_revision(&listed, &sessions);
            self.still_sharing(revision)?;
            let name = text::cut(&self.0.options.host_name, 120);
            let host_name = if name.is_empty() { "Windows".to_string() } else { name };
            let unchanged = since.as_deref() == Some(current.as_str());
            let state = WireMirrorState {
                protocol: remote_protocol::VERSION,
                mirror: protocol::VERSION,
                host_name,
                revision: current,
                workspaces: if unchanged { vec![] } else { listed },
                sessions: if unchanged { vec![] } else { sessions },
                unchanged: unchanged.then_some(true),
            };
            return Ok(reply(200, &state.to_value()));
        }
        // No query on any mirror route but the list's (a bare "?" is none, as the Mac reads it).
        if !query.is_empty() && query != "?" {
            return Err(HostFailure::new(400, get("remote.host.error.badPath")));
        }
        if request.method == "POST"
            && let ["workspaces", workspace_id, "sessions"] = route
        {
            if !shared.contains(*workspace_id) {
                return Err(HostFailure::new(403, get("remote.host.error.notShared")));
            }
            let create = body(request, WireMirrorCreate::decode)?;
            if !known_provider(&create.provider) {
                return Err(bad_request());
            }
            let id =
                host.create_session(workspace_id.to_string(), create.provider).await.map_err(|refusal| HostFailure::new(409, refusal))?;
            return Ok(reply(201, &WireMirrorCreated { protocol: remote_protocol::VERSION, session_id: id }.to_value()));
        }
        let pane = match route {
            ["sessions", id, ..] if wire::identifier(id) => host.pane_info(id.to_string()).await,
            _ => None,
        };
        let Some(pane) = pane.filter(|p| shared.contains(&p.workspace_id)) else {
            return Err(HostFailure::new(404, get("remote.error.paneNotFound")));
        };
        let id = route[1].to_string();
        if request.method == "GET" && route.len() == 2 {
            let detail = host.detail(id, shared).await.ok_or_else(|| HostFailure::new(404, get("remote.error.paneNotFound")))?;
            self.still_sharing(revision)?;
            // Through the relay the answer is one frame: the oldest entries go first, as the 1.5 MiB fitting does (the revision stays the
            // pane's, so a client polls it as before).
            let session = if relay { support::fitted(detail.session, RELAY_DETAIL_BYTES) } else { detail.session };
            let wire = WireMirrorDetail {
                protocol: remote_protocol::VERSION,
                mirror: protocol::VERSION,
                revision: detail.revision,
                session: support::write(&session),
            };
            return Ok(reply(200, &wire.to_value()));
        }
        let allowed = lock(&self.0.gate).allows_full_access;
        let refused = RemoteHost::mirror_refusal(&pane, allowed, self.0.options.windows_host);
        // What the app refuses (settled, gone, busy, not its kind) is a conflict, its message the reason, as on the Mac.
        let conflict = |refusal: Refusal| HostFailure::new(409, refusal);
        // MC-49: `queue/{item}/remove` and `queue/{item}/run-now`, the phone's queue actions; neither reads a body. 바로 실행 starts the
        // request as a send does, so it waits for the same switch; a removal runs nothing.
        if request.method == "POST"
            && let [_, _, "queue", item, action @ ("remove" | "run-now")] = route
        {
            if !wire::identifier(item) {
                return Err(HostFailure::new(404, get("remote.error.queueItemNotFound")));
            }
            if *action == "remove" {
                host.queue_remove(id, item.to_string()).await.map_err(conflict)?;
                return Ok(reply(200, &RemoteWireAnswer { protocol: remote_protocol::VERSION, accepted: true }.to_value()));
            }
            if let Some(refusal) = refused {
                return Err(HostFailure::new(403, refusal));
            }
            let outcome = host.queue_run_now(id, item.to_string()).await.map_err(conflict)?;
            return Ok(reply(200, &WireMirrorOutcome { protocol: remote_protocol::VERSION, outcome }.to_value()));
        }
        if request.method != "POST" || route.len() != 3 {
            return Err(HostFailure::new(404, get("remote.host.error.badPath")));
        }
        match route[2] {
            "submit" => {
                let submit = body(request, WireMirrorSubmit::decode)?;
                let attachments = submit.attachments.unwrap_or_default();
                if submit.text.len() > protocol::MAXIMUM_TEXT
                    || submit.text.contains('\0')
                    || attachments.len() > mighty_run::attachments::MAXIMUM_COUNT
                    || submit.text.trim().is_empty() && attachments.is_empty()
                {
                    return Err(HostFailure::new(400, get("validation.run.badInput")));
                }
                // The listener read up to 12 MiB for this route; past 512 KiB only a send that brings attachments, as a run's start.
                if request.body.len() > remote_protocol::MAXIMUM_REQUEST_BYTES && attachments.is_empty() {
                    return Err(HostFailure::new(413, get("remote.run.error.tooLarge")));
                }
                let attachments = mighty_run::attachments::validate(Some(&attachments))
                    .map_err(|error| HostFailure::new(400, error.message()))?
                    .unwrap_or_default();
                // A pane in full access (and a pane whose agent nothing confines here) runs only when this computer lets others run
                // with it.
                if let Some(refusal) = refused {
                    return Err(HostFailure::new(403, refusal));
                }
                let outcome = host.submit(id, submit.text, attachments).await.map_err(conflict)?;
                return Ok(reply(200, &WireMirrorOutcome { protocol: remote_protocol::VERSION, outcome }.to_value()));
            }
            // As a run's stop and the Mac's, a stop and a close read no body. Neither `stop` nor `close` is gated behind
            // `mirror_refusal`: `allows_full_access` confines starting/continuing unconfined agent execution, whereas stopping or
            // closing an idle/running pane (such as an unconfined Codex pane newly created via `workspaces/{id}/sessions` in C#
            // `RemoteMirrorVerification` / `tests/remote_mirror.rs::a_windows_client_mirrors_a_windows_host`) executes no agent turn.
            "stop" => host.stop(id).await.map_err(conflict)?,
            "close" => host.close(id).await.map_err(conflict)?,
            "permission" => {
                let permission = body(request, WireMirrorPermission::decode)?;
                let always = permission.always == Some(true);
                // "항상 허용" keeps rules on this computer for good: only with its switch on, as a remote run's grant.
                if always && !allowed {
                    return Err(HostFailure::new(403, get("remote.host.error.antigravityAlways")));
                }
                // An allow lets such a pane's tool go on as a send would, so it waits for the same switch; a decline never widens it.
                if permission.allow
                    && let Some(refusal) = refused
                {
                    return Err(HostFailure::new(403, refusal));
                }
                host.permission(id, permission.request_id, permission.run_id, permission.allow, always).await.map_err(conflict)?
            }
            "questions" => {
                let questions = body(request, WireMirrorQuestions::decode)?;
                // An answer, and a plan's decision below, carry the run on as a send does: the same refusal.
                if let Some(refusal) = refused {
                    return Err(HostFailure::new(403, refusal));
                }
                host.answers(id, questions.request_id, questions.run_id, questions.answers).await.map_err(conflict)?
            }
            "plan" => {
                let plan = body(request, RemoteWirePlan::decode)?;
                let decision = plan.decision().ok_or_else(bad_request)?;
                if let Some(refusal) = refused {
                    return Err(HostFailure::new(403, refusal));
                }
                host.plan(id, plan.request_id, plan.run_id, decision).await.map_err(conflict)?
            }
            _ => return Err(HostFailure::new(404, get("remote.host.error.badPath"))),
        }
        Ok(reply(200, &RemoteWireAnswer { protocol: remote_protocol::VERSION, accepted: true }.to_value()))
    }
}
