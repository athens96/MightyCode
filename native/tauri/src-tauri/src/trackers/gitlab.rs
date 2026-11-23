//! MC-31: GitLab issues and notes over REST v4: through `glab api` when glab is installed and signed in to the host (the Git
//! workbench's hosting CLI runner), else HTTPS with a personal access token (`PRIVATE-TOKEN`). States go as quick actions in a note
//! (in progress and in review as the labels `In Progress` / `In Review`, done as `/close`), so every write is one POST.

use std::path::PathBuf;
use std::sync::Arc;

use mighty_core::model::{IssueTrackerConfig, Workspace};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use serde_json::{Value, json};

use super::{IssueDetail, IssueQuery, IssueState, IssueSummary, IssueTracker, Secret, TrackerError};
use crate::host::Host;

pub(crate) const DEFAULT_HOST: &str = "gitlab.com";
pub(crate) const IN_PROGRESS_LABEL: &str = "In Progress";
pub(crate) const IN_REVIEW_LABEL: &str = "In Review";

const COMPONENT: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'_').remove(b'.');

enum Route {
    /// `glab api --hostname <host>`, run in `cwd`.
    Glab { hostname: String, cwd: PathBuf },
    /// HTTPS to `<site>/api/v4` with a token.
    Token { site: String, token: Secret },
}

pub(crate) struct GitLabTracker {
    host: Arc<Host>,
    route: Route,
    project: String,
}

impl GitLabTracker {
    pub(crate) async fn new(host: &Arc<Host>, ws: &Workspace, config: &IssueTrackerConfig) -> Result<GitLabTracker, TrackerError> {
        let (detected_host, detected_project) = detect(host, ws).await.unzip();
        let site = config
            .site
            .as_deref()
            .and_then(super::normalized_site)
            .or_else(|| detected_host.map(|h| format!("https://{h}")))
            .unwrap_or_else(|| format!("https://{DEFAULT_HOST}"));
        let project = config
            .project
            .clone()
            .filter(|p| !p.trim().is_empty())
            .or(detected_project)
            .map(|p| p.trim().trim_matches('/').to_string())
            .ok_or(TrackerError::NotConfigured("gitlab"))?;
        let route = match glab_ready(host, ws, &site).await {
            Some(route) => route,
            None => Route::Token { token: super::load_token(host, config).await.ok_or(TrackerError::AuthRequired)?, site },
        };
        Ok(GitLabTracker { host: host.clone(), route, project })
    }

    /// One REST call: `path` under `/api/v4/`, GET without a body, else a POST of `body`'s string fields.
    async fn call(&self, path: &str, body: Option<&Value>) -> Result<Value, TrackerError> {
        match &self.route {
            Route::Token { site, token } => {
                let headers = vec![("PRIVATE-TOKEN".to_string(), token.expose().to_string())];
                super::https_json(&self.host, &format!("{site}/api/v4/{path}"), headers, body, api_error).await
            }
            Route::Glab { hostname, cwd } => {
                let mut args = vec!["api".to_string(), "--hostname".to_string(), hostname.clone()];
                if let Some(fields) = body.and_then(Value::as_object) {
                    args.extend(["--method".to_string(), "POST".to_string()]);
                    for (name, value) in fields {
                        args.push("--raw-field".into());
                        args.push(format!("{name}={}", value.as_str().unwrap_or_default()));
                    }
                }
                args.push(path.to_string());
                let captured =
                    crate::git_workbench::run_hosting_cli(&self.host, "glab", cwd, &args, 20).await.map_err(TrackerError::Transport)?;
                let stdout = String::from_utf8_lossy(&captured.stdout);
                let parsed: Value = serde_json::from_str(stdout.trim()).unwrap_or(Value::Null);
                if captured.exit_code != 0 {
                    let stderr = String::from_utf8_lossy(&captured.stderr);
                    if stderr.contains("401") || stderr.contains("403") {
                        return Err(TrackerError::AuthRequired);
                    }
                    let message = api_error(&parsed).unwrap_or_else(|| crate::git_workbench::strip_url_credentials(stderr.trim()));
                    return Err(TrackerError::Api(message.lines().next().unwrap_or("glab failed").to_string()));
                }
                Ok(parsed)
            }
        }
    }

    fn issue_path(&self, key: &str) -> Result<String, TrackerError> {
        Ok(format!("projects/{}/issues/{}", encode(&self.project), checked_iid(key)?))
    }

    /// The signed-in user's name (the settings' connection test).
    pub(crate) async fn user(&self) -> Result<String, TrackerError> {
        let me = self.call("user", None).await?;
        Ok(me.get("name").or_else(|| me.get("username")).and_then(Value::as_str).unwrap_or("").to_string())
    }

    pub(crate) fn uses_glab(&self) -> bool {
        matches!(self.route, Route::Glab { .. })
    }
}

impl IssueTracker for GitLabTracker {
    fn id(&self) -> &'static str {
        "gitlab"
    }

    async fn list(&self, query: &IssueQuery) -> Result<Vec<IssueSummary>, TrackerError> {
        Ok(parse_list(&self.call(&list_path(&self.project, query), None).await?))
    }

    async fn get(&self, key: &str) -> Result<IssueDetail, TrackerError> {
        let raw = self.call(&self.issue_path(key)?, None).await?;
        parse_detail(&raw).ok_or(TrackerError::BadKey)
    }

    async fn set_state(&self, key: &str, state: IssueState) -> Result<(), TrackerError> {
        let path = format!("{}/notes", self.issue_path(key)?);
        self.call(&path, Some(&json!({ "body": state_actions(state) }))).await.map(|_| ())
    }

    async fn comment(&self, key: &str, body: &str) -> Result<(), TrackerError> {
        let path = format!("{}/notes", self.issue_path(key)?);
        self.call(&path, Some(&json!({ "body": body }))).await.map(|_| ())
    }
}

/// glab's route when glab is installed and signed in to `site`'s host.
async fn glab_ready(host: &Arc<Host>, ws: &Workspace, site: &str) -> Option<Route> {
    crate::git_workbench::find_hosting_cli(host, "glab")?;
    let parsed = url::Url::parse(site).ok()?;
    let host_name = parsed.host_str()?.to_ascii_lowercase();
    let hostname = match parsed.port() {
        Some(port) => format!("{host_name}:{port}"),
        None => host_name,
    };
    let cwd = if ws.path().is_empty() || ws.remote.is_some() { std::env::temp_dir() } else { PathBuf::from(ws.path()) };
    let args = vec!["auth".to_string(), "status".to_string(), "--hostname".to_string(), hostname.clone()];
    let captured = crate::git_workbench::run_hosting_cli(host, "glab", &cwd, &args, 10).await.ok()?;
    (captured.exit_code == 0).then_some(Route::Glab { hostname, cwd })
}

/// Whether glab is installed and signed in to the workspace's GitLab (the settings show it).
pub(crate) async fn glab_signed_in(host: &Arc<Host>, ws: &Workspace, config: &IssueTrackerConfig) -> bool {
    let detected = detect(host, ws).await.map(|(h, _)| format!("https://{h}"));
    let site = config.site.as_deref().and_then(super::normalized_site).or(detected).unwrap_or_else(|| format!("https://{DEFAULT_HOST}"));
    glab_ready(host, ws, &site).await.is_some()
}

/// The workspace's GitLab host and project path, read from its `origin`.
pub(crate) async fn detect(host: &Host, ws: &Workspace) -> Option<(String, String)> {
    if ws.remote.is_some() || ws.path().is_empty() {
        return None;
    }
    let origin = crate::git_workbench::read_origin_url(host, std::path::Path::new(ws.path())).await?;
    project_from_remote(&origin)
}

/// A GitLab remote URL's host and project path (`git@gitlab.com:group/sub/app.git` → `gitlab.com`, `group/sub/app`).
pub(crate) fn project_from_remote(remote: &str) -> Option<(String, String)> {
    let remote = remote.trim();
    let (host, path) = if let Some(rest) = remote.split_once("://").map(|(_, r)| r) {
        let rest = rest.rsplit_once('@').map_or(rest, |(_, r)| r);
        let (authority, path) = rest.split_once('/')?;
        (authority.split(':').next()?.to_string(), path.to_string())
    } else {
        let rest = remote.rsplit_once('@').map_or(remote, |(_, r)| r);
        let (host, path) = rest.split_once(':')?;
        (host.to_string(), path.to_string())
    };
    let host = host.to_ascii_lowercase();
    if !host.contains("gitlab") {
        return None;
    }
    let path = path.trim_matches('/').trim_end_matches(".git").to_string();
    (path.contains('/') && !path.contains("..")).then_some((host, path))
}

fn encode(text: &str) -> String {
    utf8_percent_encode(text, COMPONENT).to_string()
}

/// A GitLab issue key (`#12` or `12`) as its iid.
pub(crate) fn checked_iid(key: &str) -> Result<String, TrackerError> {
    let iid = key.trim().rsplit('#').next().unwrap_or("").trim();
    if !iid.is_empty() && iid.len() <= 12 && iid.chars().all(|c| c.is_ascii_digit()) {
        Ok(iid.to_string())
    } else {
        Err(TrackerError::BadKey)
    }
}

/// The issues path of a list: open (or every) issue, assigned to me for `mine`, the search text, newest first.
pub(crate) fn list_path(project: &str, query: &IssueQuery) -> String {
    let mut params = vec![
        format!("state={}", if query.filter == "all" { "all" } else { "opened" }),
        "per_page=50".to_string(),
        "order_by=updated_at".to_string(),
    ];
    if query.filter == "mine" {
        params.push("scope=assigned_to_me".into());
    }
    if !query.text.trim().is_empty() {
        params.push(format!("search={}", encode(query.text.trim())));
    }
    format!("projects/{}/issues?{}", encode(project), params.join("&"))
}

/// The quick actions a note carries to move an issue.
pub(crate) fn state_actions(state: IssueState) -> String {
    match state {
        IssueState::InProgress => format!("/label ~\"{IN_PROGRESS_LABEL}\""),
        IssueState::InReview => format!("/unlabel ~\"{IN_PROGRESS_LABEL}\"\n/label ~\"{IN_REVIEW_LABEL}\""),
        IssueState::Done => "/close".to_string(),
    }
}

/// GitLab's `message` / `error`, as one line.
pub(crate) fn api_error(answer: &Value) -> Option<String> {
    match answer.get("message").or_else(|| answer.get("error"))? {
        Value::String(s) => Some(s.clone()),
        other => Some(other.to_string()),
    }
}

fn summary(issue: &Value) -> Option<IssueSummary> {
    let iid = issue.get("iid")?.as_i64()?;
    let key = format!("#{iid}");
    Some(IssueSummary {
        title: issue.get("title").and_then(Value::as_str).unwrap_or(&key).to_string(),
        state: issue.get("state").and_then(Value::as_str).map(str::to_string),
        priority: None,
        labels: issue
            .get("labels")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
            .unwrap_or_default(),
        url: issue.get("web_url").and_then(Value::as_str).map(str::to_string),
        updated_at: issue.get("updated_at").and_then(Value::as_str).map(str::to_string),
        key,
    })
}

pub(crate) fn parse_list(answer: &Value) -> Vec<IssueSummary> {
    answer.as_array().map(|a| a.iter().filter_map(summary).collect()).unwrap_or_default()
}

pub(crate) fn parse_detail(issue: &Value) -> Option<IssueDetail> {
    Some(IssueDetail {
        summary: summary(issue)?,
        body: issue.get("description").and_then(Value::as_str).unwrap_or("").to_string(),
        assignee: issue
            .get("assignees")
            .and_then(Value::as_array)
            .and_then(|a| a.first())
            .or_else(|| issue.get("assignee").filter(|a| !a.is_null()))
            .and_then(|a| a.get("name").or_else(|| a.get("username")))
            .and_then(Value::as_str)
            .map(str::to_string),
        checklist: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_issue_lists_and_details() {
        let list: Value = serde_json::from_str(
            r#"[{"id":1,"iid":12,"title":"Fix login","state":"opened","labels":["bug","In Progress"],
                 "web_url":"https://gitlab.com/acme/app/-/issues/12","updated_at":"2026-11-20T10:00:00.000Z",
                 "assignees":[{"name":"Kim","username":"kim"}]},
                {"id":2,"iid":13,"title":"Closed one","state":"closed","labels":[]},
                {"id":3,"title":"no iid"}]"#,
        )
        .unwrap();
        let rows = parse_list(&list);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].key, "#12");
        assert_eq!(rows[0].state.as_deref(), Some("opened"));
        assert_eq!(rows[0].labels, vec!["bug", "In Progress"]);
        assert_eq!(rows[0].url.as_deref(), Some("https://gitlab.com/acme/app/-/issues/12"));
        assert_eq!(rows[1].state.as_deref(), Some("closed"));

        let issue: Value = serde_json::from_str(
            r#"{"iid":12,"title":"Fix login","state":"opened","labels":[],"description":"- [ ] repro\n- [x] fix",
                "web_url":"https://gitlab.com/acme/app/-/issues/12","assignees":[],"assignee":{"username":"lee"}}"#,
        )
        .unwrap();
        let detail = parse_detail(&issue).unwrap();
        assert_eq!(detail.body, "- [ ] repro\n- [x] fix");
        assert_eq!(detail.assignee.as_deref(), Some("lee"));
        assert_eq!(api_error(&json!({ "message": "404 Project Not Found" })).as_deref(), Some("404 Project Not Found"));
        assert_eq!(api_error(&json!({ "message": { "title": ["is missing"] } })).as_deref(), Some(r#"{"title":["is missing"]}"#));
    }

    #[test]
    fn paths_encode_the_project_and_follow_the_chip() {
        assert_eq!(
            list_path("acme/sub/app", &IssueQuery { filter: "mine".into(), text: "log in".into() }),
            "projects/acme%2Fsub%2Fapp/issues?state=opened&per_page=50&order_by=updated_at&scope=assigned_to_me&search=log%20in"
        );
        assert_eq!(
            list_path("acme/app", &IssueQuery { filter: "all".into(), text: String::new() }),
            "projects/acme%2Fapp/issues?state=all&per_page=50&order_by=updated_at"
        );
        assert_eq!(checked_iid("#12").unwrap(), "12");
        assert_eq!(checked_iid("acme/app#7").unwrap(), "7");
        assert!(checked_iid("12/notes").is_err() && checked_iid("#").is_err());
    }

    #[test]
    fn states_go_as_quick_actions() {
        assert_eq!(state_actions(IssueState::InProgress), "/label ~\"In Progress\"");
        assert_eq!(state_actions(IssueState::InReview), "/unlabel ~\"In Progress\"\n/label ~\"In Review\"");
        assert_eq!(state_actions(IssueState::Done), "/close");
    }

    #[test]
    fn reads_the_project_from_a_remote() {
        let read = |r: &str| project_from_remote(r).map(|(h, p)| format!("{h} {p}"));
        assert_eq!(read("git@gitlab.com:acme/sub/app.git").as_deref(), Some("gitlab.com acme/sub/app"));
        assert_eq!(read("https://oauth2:secret@gitlab.example.com/acme/app.git").as_deref(), Some("gitlab.example.com acme/app"));
        assert_eq!(read("ssh://git@gitlab.example.com:2222/acme/app.git").as_deref(), Some("gitlab.example.com acme/app"));
        assert_eq!(read("git@github.com:acme/app.git"), None);
        assert_eq!(read("https://gitlab.com/app"), None);
    }
}
