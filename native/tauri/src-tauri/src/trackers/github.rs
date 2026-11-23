//! MC-31: GitHub issues behind [`IssueTracker`], through `gh` with the arguments `integrations.rs` always used (`issue list`, `issue
//! view`). GitHub issues have no in-progress or review state: those are no-ops, done closes, and a comment is `gh issue comment`.
//! The app writes nothing back to GitHub on its own (as before).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use mighty_core::model::Workspace;
use mighty_platform::SpawnRequest;
use serde_json::Value;

use super::{IssueDetail, IssueQuery, IssueState, IssueSummary, IssueTracker, TrackerError};
use crate::host::Host;

pub(crate) const VIEW_FIELDS: &str = "number,title,body,state,labels,url,updatedAt";
pub(crate) const LIST_FIELDS: &str = "number,title,state,labels,url,updatedAt";

pub(crate) struct GitHubTracker {
    host: Arc<Host>,
    gh: PathBuf,
    repo: String,
    cwd: PathBuf,
}

impl GitHubTracker {
    /// The workspace's repository (its setting, else `origin`) and `gh`.
    pub(crate) async fn for_workspace(host: &Arc<Host>, ws: &Workspace) -> Result<GitHubTracker, TrackerError> {
        let repo = match ws.github_repo.clone().filter(|r| !r.trim().is_empty()) {
            Some(r) => r,
            None => crate::integrations::detect_github_repo(host, Path::new(ws.path())).await.unwrap_or_default(),
        };
        if repo.is_empty() {
            return Err(TrackerError::NotConfigured("github"));
        }
        let Some(gh) = crate::integrations::gh_executable(host) else {
            return Err(TrackerError::Api("GitHub CLI (gh) is not installed.".into()));
        };
        let cwd = if ws.path().is_empty() { std::env::temp_dir() } else { PathBuf::from(ws.path()) };
        Ok(GitHubTracker { host: host.clone(), gh, repo, cwd })
    }

    pub(crate) fn repo(&self) -> &str {
        &self.repo
    }

    async fn gh(&self, args: Vec<String>) -> Result<Vec<u8>, TrackerError> {
        let req = SpawnRequest::new(self.gh.clone(), args, self.cwd.clone()).env("GH_PROMPT_DISABLED", "1").env("NO_COLOR", "1");
        let captured = mighty_platform::process::capture(self.host.processes.as_ref(), req, None, Duration::from_secs(10), 512_000)
            .await
            .map_err(|e| TrackerError::Api(e.to_string()))?;
        if captured.exit_code != 0 {
            return Err(TrackerError::Api(String::from_utf8_lossy(&captured.stderr).trim().to_string()));
        }
        Ok(captured.stdout)
    }
}

impl IssueTracker for GitHubTracker {
    fn id(&self) -> &'static str {
        "github"
    }

    async fn list(&self, query: &IssueQuery) -> Result<Vec<IssueSummary>, TrackerError> {
        let stdout = self.gh(list_args(&self.repo, &query.filter, &query.text)).await?;
        Ok(parse_list(&stdout))
    }

    async fn get(&self, key: &str) -> Result<IssueDetail, TrackerError> {
        let num = key.trim_start_matches('#');
        let stdout = self.gh(view_args(num, &self.repo)).await?;
        let raw: Value = serde_json::from_slice(&stdout).map_err(|e| TrackerError::Api(e.to_string()))?;
        Ok(parse_view(&raw, num))
    }

    async fn set_state(&self, key: &str, state: IssueState) -> Result<(), TrackerError> {
        if state != IssueState::Done {
            return Ok(());
        }
        let num = key.trim_start_matches('#').to_string();
        self.gh(vec!["issue".into(), "close".into(), num, "--repo".into(), self.repo.clone()]).await.map(|_| ())
    }

    async fn comment(&self, key: &str, body: &str) -> Result<(), TrackerError> {
        let num = key.trim_start_matches('#').to_string();
        let args = vec!["issue".into(), "comment".into(), num, "--repo".into(), self.repo.clone(), "--body".into(), body.to_string()];
        self.gh(args).await.map(|_| ())
    }
}

/// `gh issue list`'s arguments: open issues (or all for `all`), 50, the search text.
pub(crate) fn list_args(repo: &str, filter: &str, query: &str) -> Vec<String> {
    let state_arg = if filter == "all" { "all" } else { "open" };
    let mut cmd_args = vec![
        "issue".to_string(),
        "list".to_string(),
        "--repo".to_string(),
        repo.to_string(),
        "--state".to_string(),
        state_arg.to_string(),
        "--limit".to_string(),
        "50".to_string(),
        "--json".to_string(),
        LIST_FIELDS.to_string(),
    ];
    if !query.is_empty() {
        cmd_args.push("--search".to_string());
        cmd_args.push(query.to_string());
    }
    cmd_args
}

/// `gh issue view`'s arguments.
pub(crate) fn view_args(num: &str, repo: &str) -> Vec<String> {
    ["issue", "view", num, "--repo", repo, "--json", VIEW_FIELDS].iter().map(|s| s.to_string()).collect()
}

fn labels(value: &Value) -> Vec<String> {
    value
        .get("labels")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|l| l.get("name").and_then(Value::as_str)).map(str::to_string).collect())
        .unwrap_or_default()
}

/// `gh issue list --json`'s output as rows (nothing for output that does not parse).
pub(crate) fn parse_list(stdout: &[u8]) -> Vec<IssueSummary> {
    let list: Vec<Value> = serde_json::from_slice(stdout).unwrap_or_default();
    list.iter()
        .filter_map(|i| {
            let num = i.get("number")?.as_i64()?;
            let title = i.get("title")?.as_str()?.to_string();
            Some(IssueSummary {
                key: format!("#{num}"),
                title,
                state: i.get("state").and_then(Value::as_str).map(|s| s.to_ascii_lowercase()),
                priority: None,
                labels: labels(i),
                url: i.get("url").and_then(Value::as_str).map(str::to_string),
                updated_at: i.get("updatedAt").and_then(Value::as_str).map(str::to_string),
            })
        })
        .collect()
}

/// `gh issue view --json`'s output.
pub(crate) fn parse_view(raw: &Value, num: &str) -> IssueDetail {
    let issue_key = format!("#{}", raw.get("number").and_then(Value::as_i64).map(|n| n.to_string()).unwrap_or_else(|| num.to_string()));
    IssueDetail {
        summary: IssueSummary {
            title: raw.get("title").and_then(Value::as_str).unwrap_or(&issue_key).to_string(),
            state: raw.get("state").and_then(Value::as_str).map(|s| s.to_ascii_lowercase()),
            priority: None,
            labels: labels(raw),
            url: raw.get("url").and_then(Value::as_str).map(str::to_string),
            updated_at: None,
            key: issue_key,
        },
        body: raw.get("body").and_then(Value::as_str).unwrap_or("").to_string(),
        assignee: None,
        checklist: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::trackers::{detail_json, summary_json};

    /// The regression the move must keep: the same `gh` arguments and the same rows as before MC-31.
    #[test]
    fn issue_lists_keep_their_arguments_and_rows() {
        assert_eq!(
            list_args("o/r", "open", ""),
            ["issue", "list", "--repo", "o/r", "--state", "open", "--limit", "50", "--json", "number,title,state,labels,url,updatedAt"]
        );
        assert_eq!(list_args("o/r", "all", "crash").last().map(String::as_str), Some("crash"));
        assert_eq!(list_args("o/r", "all", "crash")[5], "all");
        assert_eq!(
            view_args("42", "o/r"),
            ["issue", "view", "42", "--repo", "o/r", "--json", "number,title,body,state,labels,url,updatedAt"]
        );
        let stdout = br#"[{"number":42,"title":"Crash","state":"OPEN","labels":[{"name":"bug"}],"url":"https://github.com/o/r/issues/42","updatedAt":"2026-11-20T00:00:00Z"},{"title":"no number"}]"#;
        let rows: Vec<Value> = parse_list(stdout).iter().map(|s| summary_json("github_issue", s)).collect();
        assert_eq!(
            rows,
            vec![json!({
                "source": "github_issue", "key": "#42", "title": "Crash", "stage": "open", "tags": ["bug"],
                "url": "https://github.com/o/r/issues/42", "updatedAt": "2026-11-20T00:00:00Z"
            })]
        );
        assert!(parse_list(b"not json").is_empty());
    }

    #[test]
    fn issue_details_keep_their_json() {
        let raw = json!({ "number": 42, "title": "Crash", "body": "Steps", "state": "CLOSED", "labels": [{ "name": "bug" }], "url": "https://github.com/o/r/issues/42" });
        let detail = parse_view(&raw, "42");
        let formatted = crate::integrations::format_item_context(
            "github_issue",
            &detail.summary.key,
            &detail.summary.title,
            detail.summary.state.as_deref(),
            detail.summary.url.as_deref(),
            &detail.body,
            &[],
        );
        assert_eq!(formatted, "[GitHub #42] Crash\nStatus: closed\nURL: https://github.com/o/r/issues/42\n\nSteps");
        assert_eq!(
            detail_json("github_issue", &detail, &formatted),
            json!({
                "source": "github_issue", "key": "#42", "title": "Crash", "stage": "closed", "tags": ["bug"],
                "url": "https://github.com/o/r/issues/42", "body": "Steps", "checklist": [], "formattedContext": formatted,
            })
        );
        assert_eq!(parse_view(&json!({}), "7").summary.key, "#7");
    }
}
