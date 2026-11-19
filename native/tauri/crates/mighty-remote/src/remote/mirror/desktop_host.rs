//! The host side of the remote mirror in the app (C# `RemoteMirrorDesktopHost`, M/MightyCode/AppStore+RemoteMirror.swift
//! `RemoteMirrorBridge`): this PC's own agent panes as a connected client reads and drives them. Reads take the panes as they are now (the
//! state, the phone's waiting requests, the composers' queues); every action goes through the phone's m1 handlers (the router), so what a
//! phone can and cannot do here a mirroring client can and cannot do. The router is told the device `remote-mirror`, never a paired
//! phone's id, so no upload or push is ever another's.

use std::collections::HashSet;
use std::sync::Arc;

use indexmap::IndexMap;
use mighty_core::attachment::RunAttachment;
use mighty_core::model::{AppSnapshot, RunSession};
use mighty_core::permission::ToolPermissionRequest;
use mighty_core::wire;
use mighty_platform::BoxFuture;
use mighty_run::plan_mode::PlanDecision;
use mighty_run::questionnaire::UserQuestionAnswer;
use mighty_run::strings::get;
use serde_json::{Map, Value, json};

use super::host::{MirrorHost, Refusal};
use super::{MirrorPane, MirrorPaneInfo, MirrorQueued, MirrorSummary, support};
use crate::phone::desk::{ActionAnswer, MobileError};
use crate::phone::router::Router;
use crate::relay::connection::MobileDevice;
use crate::remote::text;

/// `Device`: the device the phone router is told for the mirror's actions.
pub const DEVICE: &str = "remote-mirror";

/// The state as it is now.
pub type Snapshot = Arc<dyn Fn() -> AppSnapshot + Send + Sync>;
/// The phone's router; None (not made yet, or the window closing) answers every action as unavailable.
pub type RouterOf = Arc<dyn Fn() -> Option<Router> + Send + Sync>;
/// A pane's queued requests as its composer holds them, in order; empty when it holds none.
pub type Queued = Arc<dyn Fn(&str) -> Vec<MirrorQueued> + Send + Sync>;
/// Whether an Antigravity card waits in the pane (`service.AntigravityCard(id) is not null`).
pub type Card = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// C# `RemoteMirrorDesktopHost(service, router, queued, onWindow)`.
#[derive(Clone)]
pub struct DeskMirrorHost {
    snapshot: Snapshot,
    router: RouterOf,
    queued: Queued,
    card: Card,
}

/// `Mirrorable(state, workspaceIds)`: this PC's own agent panes the mirror may show: of these workspaces, never another computer's.
fn mirrorable<'a>(state: &'a AppSnapshot, workspace_ids: &'a HashSet<String>) -> impl Iterator<Item = &'a Arc<RunSession>> + 'a {
    state.sessions().iter().filter(move |s| {
        s.kind() == "claude"
            && s.mirror.is_none()
            && workspace_ids.contains(s.workspace_id())
            && state.workspaces().iter().any(|w| w.id() == s.workspace_id() && w.remote.is_none())
    })
}

fn start_failed() -> Refusal {
    get("remote.error.startFailed")
}

impl DeskMirrorHost {
    pub fn new(snapshot: Snapshot, router: RouterOf, queued: Queued, card: Card) -> DeskMirrorHost {
        DeskMirrorHost { snapshot, router, queued, card }
    }

    /// `Pending(id)`: the pane's requests waiting for an answer, as the phone sees them.
    fn pending(&self, id: &str) -> Vec<ToolPermissionRequest> {
        (self.router)().map(|r| r.pending_requests(id)).unwrap_or_default().into_iter().filter(|p| p.state == "pending").collect()
    }

    fn router(&self) -> Result<Router, Refusal> {
        (self.router)().ok_or_else(|| get("remote.mirror.error.unavailable"))
    }

    /// The summaries of these workspaces' panes as they are now.
    fn summaries_now(&self, workspace_ids: &HashSet<String>) -> Vec<MirrorSummary> {
        let state = (self.snapshot)();
        mirrorable(&state, workspace_ids)
            .map(|session| {
                let permissions = self.pending(session.id());
                let waiting = (self.queued)(session.id());
                // The pane's own worktree branch (W9-I), none in the workspace's folder.
                let (branch, base) = pane_worktree(session);
                MirrorSummary {
                    id: session.id().into(),
                    workspace_id: session.workspace_id().into(),
                    title: text::cut(session.title(), 200),
                    kind: session.kind().into(),
                    provider: session.provider().into(),
                    model: text::cut(session.model(), 200),
                    status: support::wire_status(session.status()),
                    revision: support::revision(session, &permissions, &waiting, branch.as_deref(), base.as_deref()),
                    pending: permissions.len() as i64 + i64::from((self.card)(session.id())),
                    queued: waiting.len() as i64,
                    branch,
                }
            })
            .collect()
    }

    /// `Route(path, body)`: one m1 action as a phone's; a refusal is the router's words for it (else `remote.error.startFailed`).
    async fn route(&self, path: String, body: Value) -> Result<Value, Refusal> {
        let router = self.router()?;
        let raw = serde_json::value::RawValue::from_string(body.to_string()).map_err(|_| start_failed())?;
        let device = MobileDevice { client_id: DEVICE.into(), name: DEVICE.into(), direct: false, desktop: false };
        let reply = router.route(device, "POST".into(), path, Some(raw)).await;
        let answer: Value = serde_json::from_str(&reply.body).unwrap_or(Value::Null);
        if reply.status >= 400 {
            return Err(answer
                .get("error")
                .and_then(Value::as_str)
                .filter(|e| !e.is_empty())
                .map(str::to_string)
                .unwrap_or_else(start_failed));
        }
        Ok(answer)
    }
}

/// The branch and base of the pane's own worktree (C# `session.PaneWorktree?.Branch`, `?.BaseBranch`; W9-I's `RunSession.paneWorktree`),
/// none for a pane in the workspace's folder.
fn pane_worktree(session: &RunSession) -> (Option<String>, Option<String>) {
    match &session.pane_worktree {
        Some(worktree) => (worktree.branch.clone(), worktree.base_branch.clone()),
        None => (None, None),
    }
}

impl MirrorHost for DeskMirrorHost {
    fn summaries(&self, workspace_ids: HashSet<String>) -> BoxFuture<'_, Vec<MirrorSummary>> {
        Box::pin(async move { self.summaries_now(&workspace_ids) })
    }

    fn detail(&self, session_id: String, workspace_ids: HashSet<String>) -> BoxFuture<'_, Option<MirrorPane>> {
        Box::pin(async move {
            let state = (self.snapshot)();
            let pane = mirrorable(&state, &workspace_ids).find(|s| s.id() == session_id)?.clone();
            let (permissions, queued) = (self.pending(&session_id), (self.queued)(&session_id));
            // The detail (bounded and fitted under 1.5 MiB, which writes its JSON) is built off the runtime's workers.
            tokio::task::spawn_blocking(move || {
                let (branch, base) = pane_worktree(&pane);
                MirrorPane {
                    revision: support::revision(&pane, &permissions, &queued, branch.as_deref(), base.as_deref()),
                    session: support::session(&pane, &permissions, &queued, branch.as_deref(), base.as_deref()),
                }
            })
            .await
            .ok()
        })
    }

    fn pane_info(&self, session_id: String) -> BoxFuture<'_, Option<MirrorPaneInfo>> {
        Box::pin(async move {
            let state = (self.snapshot)();
            let session = state.sessions().iter().find(|s| s.id() == session_id && s.kind() == "claude" && s.mirror.is_none())?;
            state.workspaces().iter().any(|w| w.id() == session.workspace_id() && w.remote.is_none()).then(|| MirrorPaneInfo {
                workspace_id: session.workspace_id().into(),
                permission_mode: session.settings.as_ref().map(|s| s.permission_mode.clone()).unwrap_or_else(|| "manual".into()),
                provider: session.provider().into(),
            })
        })
    }

    fn submit(&self, session_id: String, text: String, attachments: Vec<RunAttachment>) -> BoxFuture<'_, Result<String, Refusal>> {
        Box::pin(async move {
            let router = self.router()?;
            let answer = router.submit_inline(&session_id, &text, attachments).await.map_err(|error| match error {
                MobileError::Request { message, .. } => message,
                _ => start_failed(),
            })?;
            // The phone's words for what happened; anything else the composer answered (blocked, not started) is a refusal.
            match answer {
                ActionAnswer::Receipt(receipt) if matches!(receipt.outcome.as_str(), "started" | "steered" | "queued") => {
                    Ok(receipt.outcome)
                }
                _ => Err(start_failed()),
            }
        })
    }

    fn stop(&self, session_id: String) -> BoxFuture<'_, Result<(), Refusal>> {
        Box::pin(async move { self.route(format!("/m1/sessions/{session_id}/stop"), json!({})).await.map(|_| ()) })
    }

    fn close(&self, session_id: String) -> BoxFuture<'_, Result<(), Refusal>> {
        Box::pin(async move { self.route(format!("/m1/sessions/{session_id}/close"), json!({})).await.map(|_| ()) })
    }

    fn permission(
        &self,
        session_id: String,
        request_id: String,
        run_id: String,
        allow: bool,
        always: bool,
    ) -> BoxFuture<'_, Result<(), Refusal>> {
        Box::pin(async move {
            let body = json!({"requestId": request_id, "runId": run_id, "allow": allow, "always": always});
            self.route(format!("/m1/sessions/{session_id}/permission"), body).await.map(|_| ())
        })
    }

    fn answers(
        &self,
        session_id: String,
        request_id: String,
        run_id: String,
        answers: IndexMap<String, UserQuestionAnswer>,
    ) -> BoxFuture<'_, Result<(), Refusal>> {
        Box::pin(async move {
            let mut written = Map::new();
            for (question, answer) in answers {
                let mut a = Map::new();
                a.insert("selectedOptions".into(), Value::Array(answer.selected_options.into_iter().map(Value::String).collect()));
                if let Some(custom) = answer.custom_text {
                    a.insert("customText".into(), Value::String(custom));
                }
                written.insert(question, Value::Object(a));
            }
            let body = json!({"requestId": request_id, "runId": run_id, "answers": Value::Object(written)});
            self.route(format!("/m1/sessions/{session_id}/answers"), body).await.map(|_| ())
        })
    }

    fn plan(&self, session_id: String, request_id: String, run_id: String, decision: PlanDecision) -> BoxFuture<'_, Result<(), Refusal>> {
        Box::pin(async move {
            let mut body = json!({"requestId": request_id, "runId": run_id, "decision": decision.kind});
            if let Some(feedback) = decision.feedback {
                body["feedback"] = Value::String(feedback);
            }
            self.route(format!("/m1/sessions/{session_id}/plan"), body).await.map(|_| ())
        })
    }

    fn create_session(&self, workspace_id: String, provider: String) -> BoxFuture<'_, Result<String, Refusal>> {
        Box::pin(async move {
            let answer =
                self.route(format!("/m1/workspaces/{workspace_id}/sessions"), json!({"kind": "claude", "provider": provider})).await?;
            match answer.get("sessionId").and_then(Value::as_str) {
                Some(id) if wire::identifier(id) => Ok(id.to_string()),
                _ => Err(start_failed()),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mighty_core::model::{PaneWorktree, Workspace};

    /// J L4 (W9 integration): a host pane in its own worktree hands its branch to the summary and moves the revision with its branch and
    /// base (C# `session.PaneWorktree?.Branch`, `?.BaseBranch`); a pane in the workspace's folder hands none.
    #[test]
    fn a_pane_in_its_own_worktree_hands_its_branch_to_the_mirror() {
        let plain = RunSession {
            id: Some("pane-1".into()),
            workspace_id: Some("ws".into()),
            kind: Some("claude".into()),
            provider: Some("claude".into()),
            ..RunSession::default()
        };
        let worktree = RunSession {
            id: Some("pane-2".into()),
            pane_worktree: Some(PaneWorktree {
                branch: Some("mighty/pane-2".into()),
                base_branch: Some("main".into()),
                ..PaneWorktree::default()
            }),
            ..plain.clone()
        };
        let state = AppSnapshot {
            workspaces: Some(vec![Workspace { id: Some("ws".into()), path: Some(r"C:\work".into()), ..Workspace::default() }]),
            sessions: Some(vec![Arc::new(plain.clone()), Arc::new(worktree.clone())]),
            ..AppSnapshot::default()
        };
        assert_eq!(pane_worktree(&plain), (None, None));
        assert_eq!(pane_worktree(&worktree), (Some("mighty/pane-2".into()), Some("main".into())));
        let host = DeskMirrorHost::new(Arc::new(move || state.clone()), Arc::new(|| None), Arc::new(|_| Vec::new()), Arc::new(|_| false));
        let summaries = host.summaries_now(&HashSet::from(["ws".to_string()]));
        let branches: Vec<_> = summaries.iter().map(|s| (s.id.as_str(), s.branch.as_deref())).collect();
        assert_eq!(branches, [("pane-1", None), ("pane-2", Some("mighty/pane-2"))]);
        let rebased = RunSession {
            pane_worktree: Some(PaneWorktree { base_branch: Some("develop".into()), ..worktree.pane_worktree.clone().unwrap() }),
            ..worktree.clone()
        };
        let (branch, base) = pane_worktree(&rebased);
        assert_ne!(
            support::revision(&worktree, &[], &[], Some("mighty/pane-2"), Some("main")),
            support::revision(&rebased, &[], &[], branch.as_deref(), base.as_deref()),
            "the revision moves with the base branch"
        );
    }
}
