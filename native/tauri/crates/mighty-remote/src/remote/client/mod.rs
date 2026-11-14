//! The client (C# `RemoteClientService`, R\RemoteClientService.cs; plan §1.6): saved connections (`saved`) and their keys in the vault,
//! the host's runtime cached 30 s, a remote workspace's panes run through [`RemoteClient`] as mighty-run's `RemoteRunner`; a lost
//! connection ends its runs with `remote.run.error.connectionLost`, a 401 on a start fails the connection. Owner: W8-D.
//!
//! At rest (W8 Step 0): nothing is connected; the app hands the run service no runner (src-tauri `remote::runner`).

pub mod runs;

use indexmap::IndexMap;
use mighty_core::model::Workspace;
use mighty_platform::BoxFuture;
use mighty_run::model::StartRunRequest;
use mighty_run::plan_mode::PlanDecision;
use mighty_run::questionnaire::UserQuestionAnswer;
use mighty_run::service::RemoteRunner;
use mighty_run::{Result, RunError};

use crate::remote::wire::RemoteConnectionInfo;

/// Every saved connection and the runs on them.
#[derive(Default)]
pub struct RemoteClient {}

impl RemoteClient {
    /// The saved connections with their state, for the shell model and Settings › 원격 연결.
    pub fn connections(&self) -> Vec<RemoteConnectionInfo> {
        vec![]
    }
}

fn not_connected() -> RunError {
    RunError::State("remote workspaces are not connected yet".into())
}

impl RemoteRunner for RemoteClient {
    fn start(&self, request: StartRunRequest, workspace: Workspace) -> BoxFuture<'_, Result<bool>> {
        let _ = (request, workspace);
        Box::pin(async { Ok(false) })
    }
    fn stop(&self, session_id: String) -> BoxFuture<'_, Result<()>> {
        let _ = session_id;
        Box::pin(async { Ok(()) })
    }
    fn respond_to_tool_permission(&self, session_id: String, request_id: String, allow: bool) -> BoxFuture<'_, Result<()>> {
        let _ = (session_id, request_id, allow);
        Box::pin(async { Err(not_connected()) })
    }
    fn answer_questions(
        &self,
        session_id: String,
        request_id: String,
        answers: IndexMap<String, UserQuestionAnswer>,
    ) -> BoxFuture<'_, Result<()>> {
        let _ = (session_id, request_id, answers);
        Box::pin(async { Err(not_connected()) })
    }
    fn answer_plan(&self, session_id: String, request_id: String, decision: PlanDecision) -> BoxFuture<'_, Result<()>> {
        let _ = (session_id, request_id, decision);
        Box::pin(async { Err(not_connected()) })
    }
    fn is_running(&self, session_id: &str) -> bool {
        let _ = session_id;
        false
    }
}
