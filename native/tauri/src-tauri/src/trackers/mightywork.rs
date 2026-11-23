//! MC-31: MightyWork tasks behind [`IssueTracker`], over the MCP tools `integrations.rs` always called (`list_tasks`, `get_task`,
//! `update_task`, `add_comment`) with the same arguments, so the task list, the detail and the stage sync behave as before.

use std::path::Path;
use std::sync::Arc;

use mighty_core::model::Workspace;
use serde_json::{Value, json};

use super::{IssueDetail, IssueQuery, IssueState, IssueSummary, IssueTracker, TrackerError};
use crate::host::Host;

pub(crate) struct MightyWorkTracker {
    host: Arc<Host>,
    workspace_path: Option<String>,
    url: Option<String>,
    project: String,
}

impl MightyWorkTracker {
    /// The workspace's project (its setting, else the key `AGENTS.md` / `CLAUDE.md` names).
    pub(crate) fn for_workspace(host: &Arc<Host>, ws: &Workspace) -> Result<MightyWorkTracker, TrackerError> {
        let project = ws
            .mightywork_project
            .clone()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| crate::integrations::detect_mightywork_project(Path::new(ws.path())))
            .unwrap_or_default();
        Self::with_project(host, ws, project)
    }

    /// An explicit project (a linked task's own), with the workspace's MCP address.
    pub(crate) fn with_project(host: &Arc<Host>, ws: &Workspace, project: String) -> Result<MightyWorkTracker, TrackerError> {
        if project.is_empty() {
            return Err(TrackerError::NotConfigured("mightywork"));
        }
        Ok(MightyWorkTracker { host: host.clone(), workspace_path: ws.path.clone(), url: ws.mightywork_url.clone(), project })
    }

    pub(crate) fn project(&self) -> &str {
        &self.project
    }

    async fn tool(&self, name: &str, arguments: Value) -> Result<Value, TrackerError> {
        crate::integrations::mcp_call_tool(&self.host, self.workspace_path.as_deref(), self.url.as_deref(), name, arguments)
            .await
            .map_err(TrackerError::Api)
    }
}

impl IssueTracker for MightyWorkTracker {
    fn id(&self) -> &'static str {
        "mightywork"
    }

    async fn list(&self, query: &IssueQuery) -> Result<Vec<IssueSummary>, TrackerError> {
        let res = self.tool("list_tasks", list_args(&self.project, &query.filter)).await?;
        Ok(parse_list(&res, &query.text))
    }

    async fn get(&self, key: &str) -> Result<IssueDetail, TrackerError> {
        let raw = self.tool("get_task", json!({ "project": self.project, "task": key })).await?;
        Ok(parse_task(&raw, key))
    }

    async fn set_state(&self, key: &str, state: IssueState) -> Result<(), TrackerError> {
        self.tool("update_task", json!({ "project": self.project, "task": key, "stage": stage_of(state) })).await.map(|_| ())
    }

    async fn comment(&self, key: &str, body: &str) -> Result<(), TrackerError> {
        self.tool("add_comment", json!({ "project": self.project, "task": key, "body": body })).await.map(|_| ())
    }
}

/// The pipeline stage a state moves a task to.
pub(crate) fn stage_of(state: IssueState) -> &'static str {
    match state {
        IssueState::InProgress => "in_progress",
        IssueState::InReview => "review",
        IssueState::Done => "done",
    }
}

/// The state of a stage the sync names (`in_progress`, `review`, `done`).
pub(crate) fn state_of_stage(stage: &str) -> Option<IssueState> {
    match stage {
        "in_progress" => Some(IssueState::InProgress),
        "review" => Some(IssueState::InReview),
        "done" => Some(IssueState::Done),
        _ => None,
    }
}

/// `list_tasks`' arguments: the project, 80 at most, and the stage chip when it names one.
pub(crate) fn list_args(project: &str, filter: &str) -> Value {
    let mut call_args = json!({ "project": project, "limit": 80 });
    if matches!(filter, "todo" | "in_progress" | "backlog" | "review" | "qa" | "done") {
        call_args["stage"] = Value::String(filter.into());
    }
    call_args
}

/// `list_tasks`' answer as rows, those matching `query` (lowercased) on key or title.
pub(crate) fn parse_list(res: &Value, query: &str) -> Vec<IssueSummary> {
    let empty = Vec::new();
    let list = res.get("tasks").and_then(Value::as_array).unwrap_or(&empty);
    list.iter()
        .filter_map(|t| {
            let key = t.get("key")?.as_str()?.to_string();
            let title = t.get("title")?.as_str()?.to_string();
            if !query.is_empty() && !key.to_lowercase().contains(query) && !title.to_lowercase().contains(query) {
                return None;
            }
            Some(IssueSummary {
                state: Some(t.get("stage").and_then(Value::as_str).unwrap_or("").to_string()),
                priority: t.get("priority").and_then(Value::as_str).map(str::to_string),
                labels: strings(t.get("tags")),
                key,
                title,
                url: None,
                updated_at: None,
            })
        })
        .collect()
}

fn strings(value: Option<&Value>) -> Vec<String> {
    value.and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default()
}

/// `get_task`'s answer.
pub(crate) fn parse_task(raw: &Value, key: &str) -> IssueDetail {
    let item_key = raw.get("key").and_then(Value::as_str).unwrap_or(key).to_string();
    IssueDetail {
        summary: IssueSummary {
            title: raw.get("title").and_then(Value::as_str).unwrap_or(&item_key).to_string(),
            state: raw.get("stage").and_then(Value::as_str).map(str::to_string),
            priority: raw.get("priority").and_then(Value::as_str).map(str::to_string),
            labels: strings(raw.get("tags")),
            url: raw.get("url").and_then(Value::as_str).map(str::to_string),
            updated_at: None,
            key: item_key,
        },
        body: raw.get("description").and_then(Value::as_str).unwrap_or("").to_string(),
        assignee: None,
        checklist: raw
            .get("checklist")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|c| Some((c.get("text")?.as_str()?.to_string(), c.get("done").and_then(Value::as_bool).unwrap_or(false))))
                    .collect()
            })
            .unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trackers::{detail_json, summary_json};

    /// The regression the move must keep: the same `list_tasks` arguments, the same rows (keys and values) as before MC-31.
    #[test]
    fn task_lists_keep_their_calls_and_rows() {
        assert_eq!(list_args("MC", "all"), json!({ "project": "MC", "limit": 80 }));
        assert_eq!(list_args("MC", "in_progress"), json!({ "project": "MC", "limit": 80, "stage": "in_progress" }));
        assert_eq!(list_args("MC", "open"), json!({ "project": "MC", "limit": 80 }));
        let res = json!({ "tasks": [
            { "key": "MC-31", "title": "Issue trackers", "stage": "in_progress", "priority": "high", "tags": ["v0.6.0"] },
            { "key": "MC-30", "title": "Hunks", "tags": [] },
            { "key": "MC-1" }
        ]});
        let rows: Vec<Value> = parse_list(&res, "").iter().map(|s| summary_json("mightywork_task", s)).collect();
        assert_eq!(
            rows,
            vec![
                json!({ "source": "mightywork_task", "key": "MC-31", "title": "Issue trackers", "stage": "in_progress", "priority": "high", "tags": ["v0.6.0"] }),
                json!({ "source": "mightywork_task", "key": "MC-30", "title": "Hunks", "stage": "", "priority": null, "tags": [] }),
            ]
        );
        assert_eq!(parse_list(&res, "hunk").len(), 1);
        assert_eq!(parse_list(&res, "mc-31").len(), 1);
        assert!(parse_list(&json!({}), "").is_empty());
    }

    #[test]
    fn task_details_keep_their_json_and_context() {
        let raw = json!({
            "key": "MC-11", "title": "MightyWork Integration", "stage": "in_progress", "priority": "high",
            "url": "https://example.test/MC-11", "description": "Implement picker.", "tags": ["v0.5.0"],
            "checklist": [{ "text": "Step 1", "done": true }, { "text": "Step 2" }]
        });
        let detail = parse_task(&raw, "MC-11");
        let formatted = crate::integrations::format_item_context(
            "mightywork_task",
            &detail.summary.key,
            &detail.summary.title,
            detail.summary.state.as_deref(),
            detail.summary.url.as_deref(),
            &detail.body,
            &detail.checklist,
        );
        assert_eq!(
            formatted,
            "[MightyWork MC-11] MightyWork Integration\nStatus: in_progress\nURL: https://example.test/MC-11\n\nImplement picker.\n\nChecklist:\n- [x] Step 1\n- [ ] Step 2"
        );
        assert_eq!(
            detail_json("mightywork_task", &detail, &formatted),
            json!({
                "source": "mightywork_task", "key": "MC-11", "title": "MightyWork Integration", "stage": "in_progress",
                "priority": "high", "tags": ["v0.5.0"], "url": "https://example.test/MC-11", "body": "Implement picker.",
                "checklist": [{ "text": "Step 1", "done": true }, { "text": "Step 2", "done": false }],
                "formattedContext": formatted,
            })
        );
        let bare = parse_task(&json!({}), "MC-9");
        assert_eq!((bare.summary.key.as_str(), bare.summary.title.as_str(), bare.summary.priority.clone()), ("MC-9", "MC-9", None));
    }

    #[test]
    fn stages_keep_their_names() {
        for (state, stage) in [(IssueState::InProgress, "in_progress"), (IssueState::InReview, "review"), (IssueState::Done, "done")] {
            assert_eq!(stage_of(state), stage);
            assert_eq!(state_of_stage(stage), Some(state));
        }
        assert_eq!(state_of_stage("qa"), None);
    }
}
