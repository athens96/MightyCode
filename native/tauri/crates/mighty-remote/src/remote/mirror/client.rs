//! The remote mirror's requests (C# `RemoteClientService.Mirror`, M/Remote/RemoteClientService.swift `mirror…`): a connected host's shared
//! panes read and driven from here. Each request goes only on a connected connection, after the route through this PC's tunnel is checked
//! (as every request of the client); a 401 (the host shares with a new key now) leaves the connection disconnected, as a run's poll does;
//! any other refusal is the caller's to read (its status and the host's words). An answer that is not the mirror's is
//! [`TransportError::InvalidData`] (C#'s `InvalidDataException`, `remote.mirror.error.badAnswer`).

use std::time::Duration;

use indexmap::IndexMap;
use mighty_core::attachment::RunAttachment;
use mighty_core::wire;
use mighty_run::plan_mode::PlanDecision;
use mighty_run::questionnaire::UserQuestionAnswer;
use mighty_run::strings::get;

use super::{
    MirrorPane, MirrorState, WireMirrorCreate, WireMirrorCreated, WireMirrorDetail, WireMirrorOutcome, WireMirrorPermission,
    WireMirrorQuestions, WireMirrorState, WireMirrorSubmit, protocol, routes, support,
};
use crate::remote::client::{ClientError, RemoteClient, lock};
use crate::remote::transport::{self, Method, Request, TransportError};
use crate::remote::wire::{RemoteConnectionStatus, RemoteWirePlan, protocol as remote_protocol, to_bytes};

fn bad_answer() -> ClientError {
    ClientError::Transport(TransportError::InvalidData(get("remote.mirror.error.badAnswer")))
}

/// `RefusalStatus(error)`: the status a host's refusal carries, when it answered with one that is not a passing busy (429, 503); None for
/// any other failure.
pub fn refusal_status(error: &ClientError) -> Option<u16> {
    match error {
        ClientError::Transport(TransportError::Status(failure)) if !failure.transient() => Some(failure.status),
        _ => None,
    }
}

/// `RemoteTransport.Transient(error)`: worth asking again for a while (the host busy, or the network stumbled).
pub fn transient(error: &ClientError) -> bool {
    matches!(error, ClientError::Transport(e) if e.transient())
}

/// An answer that is not the mirror's (C#'s `InvalidDataException`).
pub fn bad_data(error: &ClientError) -> bool {
    matches!(error, ClientError::Transport(TransportError::InvalidData(_)))
}

/// The path of an action on a host pane; a host id that is no identifier is refused before anything is sent.
fn session_path(host_session_id: &str, action: &str) -> Result<String, ClientError> {
    if !wire::identifier(host_session_id) {
        return Err(ClientError::Message(get("remote.error.paneNotFound")));
    }
    Ok(format!("{}/sessions/{host_session_id}/{action}", routes::PREFIX))
}

impl RemoteClient {
    /// `MirrorRequestAsync`: one request on a connected connection, after its route is checked; a 401 fails the connection.
    async fn mirror_request(
        &self,
        connection_id: &str,
        method: Method,
        path: &str,
        body: Option<Vec<u8>>,
        timeout: Option<Duration>,
    ) -> Result<Vec<u8>, ClientError> {
        let (target, key, serial, revision) = {
            let gate = lock(&self.0.gate);
            if gate.closing {
                return Err(ClientError::Message(get("common.appClosing")));
            }
            match gate.connections.get(connection_id) {
                Some(entry) if entry.info.status == RemoteConnectionStatus::Connected => match (&entry.target, &entry.key) {
                    (Some(target), Some(key)) => (target.clone(), key.clone(), entry.serial, entry.revision),
                    _ => return Err(ClientError::Message(get("remote.run.error.notConnected"))),
                },
                _ => return Err(ClientError::Message(get("remote.run.error.notConnected"))),
            }
        };
        self.ensure_route(connection_id, serial, revision, &target).await?;
        let request = Request { body: body.as_deref(), timeout, ..Request::default() };
        match transport::request(&target, &key, method, path, request).await {
            Ok(data) => Ok(data),
            Err(error) => {
                if error.status() == Some(401) {
                    self.fail(connection_id, error.message(), Some((serial, revision))).await;
                }
                Err(ClientError::Transport(error))
            }
        }
    }

    /// `MirrorStateAsync(connectionId, since)`: the host's shared workspaces and their agent panes (`GET /v2/mirror`); `unchanged` when
    /// `since` is still current. A `since` that is not 0–64 lower-case hex digits is left out.
    pub async fn mirror_state(&self, connection_id: &str, since: Option<&str>) -> Result<MirrorState, ClientError> {
        let since = since.filter(|s| s.len() <= 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
        let path = match since {
            Some(since) => format!("{}?since={since}", routes::PREFIX),
            None => routes::PREFIX.to_string(),
        };
        let data = self.mirror_request(connection_id, Method::Get, &path, None, None).await?;
        let state = WireMirrorState::decode(&data).map_err(|_| bad_answer())?;
        if !support::valid_state(&state) {
            return Err(bad_answer());
        }
        Ok(MirrorState {
            host_name: state.host_name,
            revision: state.revision,
            unchanged: state.unchanged == Some(true),
            workspaces: state.workspaces,
            sessions: state.sessions,
        })
    }

    /// `MirrorDetailAsync(connectionId, hostSessionId)`: one host pane in full (`GET /v2/mirror/sessions/{id}`), in this app's types.
    pub async fn mirror_detail(&self, connection_id: &str, host_session_id: &str) -> Result<MirrorPane, ClientError> {
        if !wire::identifier(host_session_id) {
            return Err(bad_answer());
        }
        let path = format!("{}/sessions/{host_session_id}", routes::PREFIX);
        let data = self.mirror_request(connection_id, Method::Get, &path, None, None).await?;
        let detail = WireMirrorDetail::decode(&data).map_err(|_| bad_answer())?;
        let session = support::read(&detail.session).ok_or_else(bad_answer)?;
        if !support::valid_detail(&detail, &session, host_session_id) {
            return Err(bad_answer());
        }
        Ok(MirrorPane { revision: detail.revision, session })
    }

    /// `MirrorSubmitAsync`: a request into the host pane; the host's word for what it did (`started`, `queued`, …; `accepted` when its
    /// answer says none).
    pub async fn mirror_submit(
        &self,
        connection_id: &str,
        host_session_id: &str,
        text: &str,
        attachments: Vec<RunAttachment>,
    ) -> Result<String, ClientError> {
        let files = !attachments.is_empty();
        let body = to_bytes(&WireMirrorSubmit { text: text.into(), attachments: files.then_some(attachments) }.to_value());
        if body.len() > mighty_run::attachments::MAXIMUM_REQUEST_BYTES || !files && body.len() > remote_protocol::MAXIMUM_REQUEST_BYTES {
            return Err(ClientError::Message(get("remote.run.error.tooLarge")));
        }
        let timeout = if files { remote_protocol::UPLOAD_TIMEOUT } else { protocol::SEND_TIMEOUT };
        let path = session_path(host_session_id, "submit")?;
        let data = self.mirror_request(connection_id, Method::Post, &path, Some(body), Some(timeout)).await?;
        Ok(WireMirrorOutcome::decode(&data).map(|o| o.outcome).unwrap_or_else(|_| "accepted".into()))
    }

    /// `MirrorStopAsync`.
    pub async fn mirror_stop(&self, connection_id: &str, host_session_id: &str) -> Result<(), ClientError> {
        let path = session_path(host_session_id, "stop")?;
        self.mirror_request(connection_id, Method::Post, &path, Some(b"{}".to_vec()), None).await.map(|_| ())
    }

    /// `MirrorCloseAsync`.
    pub async fn mirror_close(&self, connection_id: &str, host_session_id: &str) -> Result<(), ClientError> {
        let path = session_path(host_session_id, "close")?;
        self.mirror_request(connection_id, Method::Post, &path, Some(b"{}".to_vec()), None).await.map(|_| ())
    }

    /// `MirrorPermissionAsync`: an answer to a host pane's tool request; `always` (an Antigravity card's 항상 허용) only when set, as the
    /// Mac sends it.
    pub async fn mirror_permission(
        &self,
        connection_id: &str,
        host_session_id: &str,
        request_id: &str,
        run_id: &str,
        allow: bool,
        always: bool,
    ) -> Result<(), ClientError> {
        let path = session_path(host_session_id, "permission")?;
        let body = WireMirrorPermission { request_id: request_id.into(), run_id: run_id.into(), allow, always: always.then_some(true) };
        self.mirror_request(connection_id, Method::Post, &path, Some(to_bytes(&body.to_value())), None).await.map(|_| ())
    }

    /// `MirrorAnswersAsync`.
    pub async fn mirror_answers(
        &self,
        connection_id: &str,
        host_session_id: &str,
        request_id: &str,
        run_id: &str,
        answers: IndexMap<String, UserQuestionAnswer>,
    ) -> Result<(), ClientError> {
        let path = session_path(host_session_id, "questions")?;
        let body = WireMirrorQuestions { request_id: request_id.into(), run_id: run_id.into(), answers };
        self.mirror_request(connection_id, Method::Post, &path, Some(to_bytes(&body.to_value())), None).await.map(|_| ())
    }

    /// `MirrorPlanAsync`: the remote run's plan body.
    pub async fn mirror_plan(
        &self,
        connection_id: &str,
        host_session_id: &str,
        request_id: &str,
        run_id: &str,
        decision: &PlanDecision,
    ) -> Result<(), ClientError> {
        let path = session_path(host_session_id, "plan")?;
        let body = RemoteWirePlan::from(run_id, request_id, decision);
        self.mirror_request(connection_id, Method::Post, &path, Some(to_bytes(&body.to_value())), None).await.map(|_| ())
    }

    /// `MirrorCreateSessionAsync`: a new agent pane in the host's shared workspace; the host's id for it.
    pub async fn mirror_create_session(&self, connection_id: &str, host_workspace_id: &str, provider: &str) -> Result<String, ClientError> {
        if !wire::identifier(host_workspace_id) {
            return Err(ClientError::Message(get("remote.workspace.error.notShared")));
        }
        let path = format!("{}/workspaces/{host_workspace_id}/sessions", routes::PREFIX);
        let body = to_bytes(&WireMirrorCreate { provider: provider.into() }.to_value());
        let data = self.mirror_request(connection_id, Method::Post, &path, Some(body), None).await?;
        match WireMirrorCreated::decode(&data) {
            Ok(created) if wire::identifier(&created.session_id) => Ok(created.session_id),
            _ => Err(bad_answer()),
        }
    }

    /// `MirrorLostAsync`: marks the connection lost from the mirror's poll (it failed past its retries); its runs end as a failed poll's.
    pub async fn mirror_lost(&self, connection_id: &str, message: &str) {
        let connected =
            lock(&self.0.gate).connections.get(connection_id).is_some_and(|e| e.info.status == RemoteConnectionStatus::Connected);
        if connected {
            self.fail(connection_id, message, None).await;
        }
    }
}
