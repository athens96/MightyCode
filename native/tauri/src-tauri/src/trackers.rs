//! MC-31: issue trackers behind one interface ([`IssueTracker`]): GitHub (`gh`), which `integrations.rs` had on its own, and Linear
//! (GraphQL), Jira Cloud (REST v3) and GitLab (`glab` or REST with a token).
//!
//! - A tracker lists issues for a query, reads one (its body as Markdown), moves it to in progress, in review or done, and comments.
//! - Tokens live only in the OS keychain (`remote::secret_vault`), read when a call needs one; a [`Secret`] never prints, serializes
//!   or goes into an error, a status, a state file or MCP output.
//! - Every HTTP call goes through the app's HTTPS port with a timeout; nothing here runs on a timer.
//! - An issue started in a worktree pane gets the branch `<prefix>/<KEY>-<slug>` ([`issue_session_id`]), the pane's session id being
//!   `<prefix>-<KEY>-<slug>` so the branch follows from the id (`mighty_core::model::issue_pane_branch`).

pub(crate) mod adf;
pub(crate) mod github;
pub(crate) mod gitlab;
pub(crate) mod jira;
pub(crate) mod linear;

use std::sync::Arc;
use std::time::Duration;

use mighty_core::model::{IssueTrackerConfig, LinkedTask, Workspace};
use mighty_platform::http::HttpsRequest;
use serde_json::{Value, json};

use crate::host::Host;

pub(crate) const HTTP_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_BODY: usize = 2 * 1024 * 1024;

/// The trackers a workspace may add beyond GitHub, in the order the issue list shows their tabs.
pub(crate) const EXTERNAL_TRACKERS: [&str; 3] = ["linear", "jira", "gitlab"];

// ── the common shapes ──────────────────────────────────────────────────────────────────────────────────────────────────

/// Where an issue moves when work starts, a pull request is made, or it merges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IssueState {
    InProgress,
    InReview,
    Done,
}

/// A list request: the tracker's filter chip (`mine`, `open` or `all`) and the search text (lowercased).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct IssueQuery {
    pub filter: String,
    pub text: String,
}

/// One row of an issue list.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct IssueSummary {
    pub key: String,
    pub title: String,
    pub state: Option<String>,
    pub priority: Option<String>,
    pub labels: Vec<String>,
    pub url: Option<String>,
    pub updated_at: Option<String>,
}

/// One issue read in full: its body in Markdown, its assignee and its sub-items (a checklist).
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct IssueDetail {
    pub summary: IssueSummary,
    pub body: String,
    pub assignee: Option<String>,
    pub checklist: Vec<(String, bool)>,
}

/// Why a tracker call failed. Its text never carries a token (the HTTPS port never puts a header into an error).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TrackerError {
    /// The tracker is not set up for this workspace (`"github"`, `"linear"`, …).
    NotConfigured(&'static str),
    /// No token, or the tracker refused it.
    AuthRequired,
    Http(u16),
    /// The tracker's own message (GraphQL `errors`, Jira `errorMessages`, a CLI's stderr).
    Api(String),
    Transport(String),
    /// A key that is not one of this tracker's.
    BadKey,
}

impl std::fmt::Display for TrackerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TrackerError::NotConfigured("github") => f.write_str("GitHub repository is not configured."),
            TrackerError::NotConfigured(tracker) => write!(f, "{} is not configured.", tracker_name(tracker)),
            TrackerError::AuthRequired => f.write_str("AUTH_REQUIRED"),
            TrackerError::Http(status) => write!(f, "HTTP {status}"),
            TrackerError::Api(message) | TrackerError::Transport(message) => f.write_str(message),
            TrackerError::BadKey => f.write_str("Unknown issue key."),
        }
    }
}

impl From<TrackerError> for String {
    fn from(error: TrackerError) -> String {
        error.to_string()
    }
}

/// The common interface every tracker gives (the design's `IssueTracker`).
pub(crate) trait IssueTracker {
    fn id(&self) -> &'static str;
    async fn list(&self, query: &IssueQuery) -> Result<Vec<IssueSummary>, TrackerError>;
    async fn get(&self, key: &str) -> Result<IssueDetail, TrackerError>;
    async fn set_state(&self, key: &str, state: IssueState) -> Result<(), TrackerError>;
    async fn comment(&self, key: &str, body: &str) -> Result<(), TrackerError>;
}

/// One of the trackers, chosen at run time (static dispatch over the trait).
pub(crate) enum AnyTracker {
    GitHub(github::GitHubTracker),
    Linear(linear::LinearTracker),
    Jira(jira::JiraTracker),
    GitLab(gitlab::GitLabTracker),
}

impl IssueTracker for AnyTracker {
    fn id(&self) -> &'static str {
        match self {
            AnyTracker::GitHub(t) => t.id(),
            AnyTracker::Linear(t) => t.id(),
            AnyTracker::Jira(t) => t.id(),
            AnyTracker::GitLab(t) => t.id(),
        }
    }

    async fn list(&self, query: &IssueQuery) -> Result<Vec<IssueSummary>, TrackerError> {
        match self {
            AnyTracker::GitHub(t) => t.list(query).await,
            AnyTracker::Linear(t) => t.list(query).await,
            AnyTracker::Jira(t) => t.list(query).await,
            AnyTracker::GitLab(t) => t.list(query).await,
        }
    }

    async fn get(&self, key: &str) -> Result<IssueDetail, TrackerError> {
        match self {
            AnyTracker::GitHub(t) => t.get(key).await,
            AnyTracker::Linear(t) => t.get(key).await,
            AnyTracker::Jira(t) => t.get(key).await,
            AnyTracker::GitLab(t) => t.get(key).await,
        }
    }

    async fn set_state(&self, key: &str, state: IssueState) -> Result<(), TrackerError> {
        match self {
            AnyTracker::GitHub(t) => t.set_state(key, state).await,
            AnyTracker::Linear(t) => t.set_state(key, state).await,
            AnyTracker::Jira(t) => t.set_state(key, state).await,
            AnyTracker::GitLab(t) => t.set_state(key, state).await,
        }
    }

    async fn comment(&self, key: &str, body: &str) -> Result<(), TrackerError> {
        match self {
            AnyTracker::GitHub(t) => t.comment(key, body).await,
            AnyTracker::Linear(t) => t.comment(key, body).await,
            AnyTracker::Jira(t) => t.comment(key, body).await,
            AnyTracker::GitLab(t) => t.comment(key, body).await,
        }
    }
}

/// The tracker `tracker` of `ws` (`"github"`, `"linear"`, `"jira"`, `"gitlab"`), its token read from the keychain.
pub(crate) async fn tracker_for(host: &Arc<Host>, ws: &Workspace, tracker: &str) -> Result<AnyTracker, TrackerError> {
    match tracker {
        "github" => github::GitHubTracker::for_workspace(host, ws).await.map(AnyTracker::GitHub),
        "linear" | "jira" | "gitlab" => {
            let config = ws
                .issue_tracker(tracker)
                .cloned()
                .unwrap_or_else(|| IssueTrackerConfig { tracker: tracker.to_string(), ..IssueTrackerConfig::default() });
            match tracker {
                "linear" => linear::LinearTracker::new(host, &config).await.map(AnyTracker::Linear),
                "jira" => jira::JiraTracker::new(host, &config).await.map(AnyTracker::Jira),
                _ => gitlab::GitLabTracker::new(host, ws, &config).await.map(AnyTracker::GitLab),
            }
        }
        _ => Err(TrackerError::NotConfigured("issue tracker")),
    }
}

// ── sources, names and the item JSON the page reads ─────────────────────────────────────────────────────────────────────

/// The tracker an issue list's item `source` belongs to.
pub(crate) fn tracker_of_source(source: &str) -> Option<&'static str> {
    match source {
        "github_issue" => Some("github"),
        "linear_issue" => Some("linear"),
        "jira_issue" => Some("jira"),
        "gitlab_issue" => Some("gitlab"),
        _ => None,
    }
}

/// The item `source` of a tracker's issues.
pub(crate) fn source_of_tracker(tracker: &str) -> Option<&'static str> {
    match tracker {
        "github" => Some("github_issue"),
        "linear" => Some("linear_issue"),
        "jira" => Some("jira_issue"),
        "gitlab" => Some("gitlab_issue"),
        _ => None,
    }
}

/// A tracker's product name (not translated).
pub(crate) fn tracker_name(tracker: &str) -> &'static str {
    match tracker {
        "github" => "GitHub",
        "linear" => "Linear",
        "jira" => "Jira",
        "gitlab" => "GitLab",
        _ => "Issue tracker",
    }
}

/// A list row as the page reads it.
pub(crate) fn summary_json(source: &str, s: &IssueSummary) -> Value {
    json!({
        "source": source,
        "key": s.key,
        "title": s.title,
        "stage": s.state,
        "tags": s.labels,
        "url": s.url,
        "updatedAt": s.updated_at,
    })
}

/// An issue read in full as the page reads it, with its first-request context.
pub(crate) fn detail_json(source: &str, d: &IssueDetail, formatted: &str) -> Value {
    let checklist: Vec<Value> = d.checklist.iter().map(|(t, done)| json!({ "text": t, "done": done })).collect();
    let mut out = json!({
        "source": source,
        "key": d.summary.key,
        "title": d.summary.title,
        "stage": d.summary.state,
        "tags": d.summary.labels,
        "url": d.summary.url,
        "body": d.body,
        "checklist": checklist,
        "formattedContext": formatted,
    });
    if d.summary.priority.is_some() {
        out["priority"] = json!(d.summary.priority);
    }
    if let Some(assignee) = d.assignee.as_ref() {
        out["assignee"] = json!(assignee);
    }
    out
}

// ── branches for an issue ───────────────────────────────────────────────────────────────────────────────────────────────

/// The branch prefix of a tracker's issue worktrees (`mighty_core::model::ISSUE_BRANCH_PREFIXES`); GitHub keeps `mighty/<id>`.
pub(crate) fn branch_prefix(tracker: &str) -> Option<&'static str> {
    match tracker {
        "linear" => Some("lin"),
        "jira" => Some("jira"),
        "gitlab" => Some("gl"),
        _ => None,
    }
}

/// `text` as words of ASCII letters and digits joined by `-`, at most `max` characters, cut at a word.
pub(crate) fn slug(text: &str, max: usize, lowercase: bool) -> String {
    let mut out = String::new();
    for word in text.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()) {
        let word = if lowercase { word.to_ascii_lowercase() } else { word.to_string() };
        let extra = if out.is_empty() { word.len() } else { word.len() + 1 };
        if out.len() + extra > max {
            if out.is_empty() {
                out = word.chars().take(max).collect();
            }
            break;
        }
        if !out.is_empty() {
            out.push('-');
        }
        out.push_str(&word);
    }
    out
}

/// The session id of a pane that works on `key` of `tracker` (`lin-ENG-123-fix-login`), its branch then `lin/ENG-123-fix-login`;
/// `n` > 1 adds `-n` (a second pane on the same issue). None for a tracker without a branch prefix or a key with no letter or digit.
pub(crate) fn issue_session_id(tracker: &str, key: &str, title: &str, n: u32) -> Option<String> {
    let prefix = branch_prefix(tracker)?;
    let key = slug(key, 32, false);
    if key.is_empty() {
        return None;
    }
    let title = slug(title, 40, true);
    let mut id = if title.is_empty() { format!("{prefix}-{key}") } else { format!("{prefix}-{key}-{title}") };
    if n > 1 {
        id.push_str(&format!("-{n}"));
    }
    let branch = mighty_core::model::issue_pane_branch(&id)?;
    let safe = mighty_core::wire::identifier(&id)
        && mighty_delegation::git::worktree::is_safe_session_id(&id)
        && crate::git_workbench::validate_branch_name(&branch).is_ok();
    safe.then_some(id)
}

// ── the first request and the pull request ──────────────────────────────────────────────────────────────────────────────

/// The line a pull request's body gets for its linked issue (`Issue: [ENG-123](https://…)`).
pub(crate) fn pr_issue_line(linked: &LinkedTask) -> String {
    let name = tracker_name(&linked.source);
    match linked.url.as_deref().filter(|u| !u.is_empty()) {
        Some(url) => format!("{name} issue: [{}]({url})", linked.key),
        None => format!("{name} issue: {}", linked.key),
    }
}

/// `body` with the linked issue's line added at its end, unless it already names the issue.
pub(crate) fn pr_body_with_issue(body: &str, linked: &LinkedTask) -> String {
    let body = body.trim();
    let named = linked.url.as_deref().is_some_and(|u| !u.is_empty() && body.contains(u));
    if named {
        return body.to_string();
    }
    let line = pr_issue_line(linked);
    if body.is_empty() { line } else { format!("{body}\n\n{line}") }
}

// ── tokens ─────────────────────────────────────────────────────────────────────────────────────────────────────────────

/// A token read from the keychain. It never prints (`Debug` hides it), serializes or reaches an error.
#[derive(Clone)]
pub(crate) struct Secret(String);

impl Secret {
    pub(crate) fn new(value: &str) -> Option<Secret> {
        let value = value.trim();
        (!value.is_empty()).then(|| Secret(value.to_string()))
    }

    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(…)")
    }
}

/// The host of a site address (`https://team.atlassian.net/` → `team.atlassian.net`), lowercased.
pub(crate) fn site_host(site: &str) -> Option<String> {
    let parsed = url::Url::parse(site.trim()).ok()?;
    parsed.host_str().map(str::to_ascii_lowercase).filter(|h| !h.is_empty())
}

/// A site address as `https://host` (no path, no user, no query); None when it is not https.
pub(crate) fn normalized_site(site: &str) -> Option<String> {
    let trimmed = site.trim();
    let with_scheme = if trimmed.contains("://") { trimmed.to_string() } else { format!("https://{trimmed}") };
    let parsed = url::Url::parse(&with_scheme).ok()?;
    if parsed.scheme() != "https" || !parsed.username().is_empty() || parsed.password().is_some() {
        return None;
    }
    let host = parsed.host_str()?.to_ascii_lowercase();
    if host.is_empty() {
        return None;
    }
    Some(match parsed.port() {
        Some(port) => format!("https://{host}:{port}"),
        None => format!("https://{host}"),
    })
}

/// The keychain item of a tracker's token: one per tracker, and for Jira and GitLab one per site.
pub(crate) fn secret_id(config: &IssueTrackerConfig) -> String {
    match config.tracker.as_str() {
        "linear" => "issue-tracker.linear".into(),
        "jira" => format!("issue-tracker.jira.{}", config.site.as_deref().and_then(site_host).unwrap_or_default()),
        _ => format!(
            "issue-tracker.gitlab.{}",
            config.site.as_deref().and_then(site_host).unwrap_or_else(|| gitlab::DEFAULT_HOST.to_string())
        ),
    }
}

/// The tracker's token from the keychain (off the async threads: a Keychain read may ask).
pub(crate) async fn load_token(host: &Arc<Host>, config: &IssueTrackerConfig) -> Option<Secret> {
    let vault = crate::remote::secret_vault(host);
    let id = secret_id(config);
    tauri::async_runtime::spawn_blocking(move || vault.load(&id)).await.ok().flatten().and_then(|t| Secret::new(&t))
}

pub(crate) async fn save_token(host: &Arc<Host>, config: &IssueTrackerConfig, token: &str) -> Result<(), String> {
    let vault = crate::remote::secret_vault(host);
    let (id, token) = (secret_id(config), token.trim().to_string());
    tauri::async_runtime::spawn_blocking(move || vault.save(&id, &token).map_err(|e| e.to_string())).await.map_err(|e| e.to_string())?
}

pub(crate) async fn delete_token(host: &Arc<Host>, config: &IssueTrackerConfig) -> Result<(), String> {
    let vault = crate::remote::secret_vault(host);
    let id = secret_id(config);
    tauri::async_runtime::spawn_blocking(move || vault.delete(&id).map_err(|e| e.to_string())).await.map_err(|e| e.to_string())?
}

// ── HTTPS ──────────────────────────────────────────────────────────────────────────────────────────────────────────────

/// One HTTPS call through the app's port: GET when `body` is None, else a JSON POST. 401/403 read as [`TrackerError::AuthRequired`];
/// another failure as its status, with the tracker's message when `api_message` finds one in the answer.
pub(crate) async fn https_json(
    host: &Host,
    url: &str,
    headers: Vec<(String, String)>,
    body: Option<&Value>,
    api_message: fn(&Value) -> Option<String>,
) -> Result<Value, TrackerError> {
    let Some(ports) = tauri::Manager::try_state::<crate::platform::Ports>(&host.app) else {
        return Err(TrackerError::Transport("HTTP transport unavailable.".into()));
    };
    let mut headers = headers;
    headers.push(("Accept".into(), "application/json".into()));
    let response = match body {
        None => ports
            .https
            .get(HttpsRequest { url: url.to_string(), headers, timeout: HTTP_TIMEOUT }, MAX_BODY)
            .await
            .map_err(|e| TrackerError::Transport(e.to_string()))?,
        Some(body) => {
            headers.push(("Content-Type".into(), "application/json".into()));
            let bytes = serde_json::to_vec(body).map_err(|e| TrackerError::Transport(e.to_string()))?;
            ports
                .https
                .post_json(HttpsRequest { url: url.to_string(), headers, timeout: HTTP_TIMEOUT }, bytes, MAX_BODY)
                .await
                .map_err(|e| TrackerError::Transport(e.to_string()))?
                .0
        }
    };
    let parsed = if response.body.trim().is_empty() { Value::Null } else { serde_json::from_str(&response.body).unwrap_or(Value::Null) };
    if response.status == 401 || response.status == 403 {
        return Err(TrackerError::AuthRequired);
    }
    if !(200..300).contains(&response.status) {
        return Err(api_message(&parsed).map(TrackerError::Api).unwrap_or(TrackerError::Http(response.status)));
    }
    Ok(parsed)
}

// ── write-back ─────────────────────────────────────────────────────────────────────────────────────────────────────────

/// What happened to a linked issue's work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WorkEvent {
    Started,
    PullRequest(Option<String>),
    Merged,
}

/// What an external tracker's switches say to write back for `event`: the state to move to and the comment to post.
pub(crate) fn write_back_plan(config: &IssueTrackerConfig, event: &WorkEvent) -> (Option<IssueState>, Option<String>) {
    if !config.enabled {
        return (None, None);
    }
    match event {
        WorkEvent::Started => (config.write_start_on().then_some(IssueState::InProgress), None),
        WorkEvent::PullRequest(url) => (
            config.write_pr_on().then_some(IssueState::InReview),
            url.as_deref().filter(|u| !u.is_empty() && config.comment_pr_on()).map(|u| format!("Pull request: {u}")),
        ),
        WorkEvent::Merged => (config.write_merge_on().then_some(IssueState::Done), None),
    }
}

/// Writes `event` back to a Linear, Jira or GitLab issue linked to a pane, as the workspace's switches say, on a task of its own; a
/// note in the pane says what was written (or why not). GitHub writes nothing.
pub(crate) fn spawn_write_back(host: &Arc<Host>, workspace_id: &str, linked: &LinkedTask, event: WorkEvent, pane_id: Option<String>) {
    if !EXTERNAL_TRACKERS.contains(&linked.source.as_str()) || !host.may_change_machine() {
        return;
    }
    let Some(ws) = host.view().snapshot.workspace(workspace_id).cloned() else { return };
    let Some(config) = ws.issue_tracker(&linked.source).cloned() else { return };
    let (state, comment) = write_back_plan(&config, &event);
    if state.is_none() && comment.is_none() {
        return;
    }
    let (host, linked) = (host.clone(), linked.clone());
    tauri::async_runtime::spawn(async move {
        let words_tracker = tracker_name(&linked.source);
        let note = |text: String| {
            if let Some(pane) = pane_id.as_deref() {
                crate::pane_worktree::append_system_note(&host, pane, text);
            }
        };
        let tracker =
            match tracker_for(&host, &ws, &linked.source).await {
                Ok(tracker) => tracker,
                Err(error) => {
                    let reason = error.to_string();
                    return note(host.locale.get_with(
                        "integrations.sync.issueFailed",
                        &[("tracker", words_tracker), ("key", &linked.key), ("reason", &reason)],
                    ));
                }
            };
        if let Some(state) = state {
            let words = [("tracker", words_tracker), ("key", linked.key.as_str())];
            match tracker.set_state(&linked.key, state).await {
                Ok(()) => note(host.locale.get_with(
                    match state {
                        IssueState::InProgress => "integrations.sync.issueStarted",
                        IssueState::InReview => "integrations.sync.issueReview",
                        IssueState::Done => "integrations.sync.issueDone",
                    },
                    &words,
                )),
                Err(error) => {
                    let reason = error.to_string();
                    note(host.locale.get_with(
                        "integrations.sync.issueFailed",
                        &[("tracker", words_tracker), ("key", &linked.key), ("reason", &reason)],
                    ))
                }
            }
        }
        if let Some(body) = comment {
            match tracker.comment(&linked.key, &body).await {
                Ok(()) => {
                    note(host.locale.get_with("integrations.sync.issueCommented", &[("tracker", words_tracker), ("key", &linked.key)]))
                }
                Err(error) => {
                    let reason = error.to_string();
                    note(host.locale.get_with(
                        "integrations.sync.issueFailed",
                        &[("tracker", words_tracker), ("key", &linked.key), ("reason", &reason)],
                    ))
                }
            }
        }
    });
}

// ── settings and status ────────────────────────────────────────────────────────────────────────────────────────────────

/// A tracker's settings as the page reads them: never its token, only whether one is kept (`connected`).
pub(crate) fn config_json(config: &IssueTrackerConfig, connected: bool, extra: Value) -> Value {
    let mut out = json!({
        "tracker": config.tracker,
        "name": tracker_name(&config.tracker),
        "enabled": config.enabled,
        "connected": connected,
        "project": config.project.clone().unwrap_or_default(),
        "site": config.site.clone().unwrap_or_default(),
        "email": config.email.clone().unwrap_or_default(),
        "filterMine": config.filter_mine_on(),
        "filterOpen": config.filter_open_on(),
        "writeStart": config.write_start_on(),
        "writePr": config.write_pr_on(),
        "writeMerge": config.write_merge_on(),
        "commentPr": config.comment_pr_on(),
    });
    if let (Some(target), Some(more)) = (out.as_object_mut(), extra.as_object()) {
        for (k, v) in more {
            target.insert(k.clone(), v.clone());
        }
    }
    out
}

/// The default filter chip of an external tracker's list.
pub(crate) fn default_filter(config: &IssueTrackerConfig) -> &'static str {
    match (config.filter_mine_on(), config.filter_open_on()) {
        (true, _) => "mine",
        (false, true) => "open",
        (false, false) => "all",
    }
}

/// The settings `args` carry for `tracker`, on top of `current`.
pub(crate) fn config_from_args(tracker: &str, current: Option<&IssueTrackerConfig>, args: &Value) -> Result<IssueTrackerConfig, String> {
    let mut config = current.cloned().unwrap_or_else(|| IssueTrackerConfig { tracker: tracker.to_string(), ..Default::default() });
    let text = |name: &str| args.get(name).and_then(Value::as_str).map(|s| s.trim().to_string());
    let flag = |name: &str| args.get(name).and_then(Value::as_bool);
    if let Some(enabled) = flag("enabled") {
        config.enabled = enabled;
    }
    if let Some(project) = text("project") {
        config.project = (!project.is_empty()).then_some(project);
    }
    if let Some(site) = text("site") {
        config.site = if site.is_empty() {
            None
        } else {
            Some(normalized_site(&site).ok_or_else(|| format!("{}: the address must start with https://", tracker_name(tracker)))?)
        };
    }
    if let Some(email) = text("email") {
        config.email = (!email.is_empty()).then_some(email);
    }
    config.filter_mine = flag("filterMine").or(config.filter_mine);
    config.filter_open = flag("filterOpen").or(config.filter_open);
    config.write_start = flag("writeStart").or(config.write_start);
    config.write_pr = flag("writePr").or(config.write_pr);
    config.write_merge = flag("writeMerge").or(config.write_merge);
    config.comment_pr = flag("commentPr").or(config.comment_pr);
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(tracker: &str) -> IssueTrackerConfig {
        IssueTrackerConfig { tracker: tracker.into(), enabled: true, ..Default::default() }
    }

    #[test]
    fn issue_branches_are_prefix_key_slug_and_follow_from_the_session_id() {
        let id = issue_session_id("linear", "ENG-123", "Fix login", 1).unwrap();
        assert_eq!(id, "lin-ENG-123-fix-login");
        assert_eq!(mighty_core::model::issue_pane_branch(&id).as_deref(), Some("lin/ENG-123-fix-login"));
        assert!(mighty_core::model::is_pane_branch("lin/ENG-123-fix-login", &id));
        assert_eq!(mighty_core::model::pane_session_of_branch("lin/ENG-123-fix-login").as_deref(), Some(id.as_str()));
        assert_eq!(issue_session_id("jira", "PROJ-7", "Crash on: save / load!", 2).as_deref(), Some("jira-PROJ-7-crash-on-save-load-2"));
        assert_eq!(issue_session_id("gitlab", "#12", "日本語のタイトル", 1).as_deref(), Some("gl-12"));
        // Long titles are cut at a word; the branch stays a valid one.
        let long = issue_session_id("linear", "ENG-1", &"word ".repeat(40), 1).unwrap();
        assert!(long.len() <= 4 + 6 + 41 && !long.ends_with('-'));
        assert!(crate::git_workbench::validate_branch_name(&mighty_core::model::issue_pane_branch(&long).unwrap()).is_ok());
        // GitHub keeps mighty/<id>; a key with nothing usable names no branch.
        assert_eq!(issue_session_id("github", "#1", "x", 1), None);
        assert_eq!(issue_session_id("linear", "--", "x", 1), None);
        // `mighty/…` never reads as an issue branch, and an id without a known prefix names none.
        assert!(!mighty_core::model::is_pane_branch("mighty/lin-x", "lin-x") || mighty_core::model::is_pane_branch("lin/x", "lin-x"));
        assert_eq!(mighty_core::model::issue_pane_branch("mw-12-abcd"), None);
        assert_eq!(mighty_core::model::issue_pane_branch("0123abcd"), None);
        assert!(mighty_core::model::is_pane_branch("mighty/mw-12-abcd", "mw-12-abcd"));
        assert!(!mighty_core::model::is_pane_branch("lin/other", "lin-ENG-1"));
    }

    #[test]
    fn a_token_never_prints_or_reaches_a_status() {
        let secret = Secret::new("  lin_api_SECRET123  ").unwrap();
        assert_eq!(secret.expose(), "lin_api_SECRET123");
        assert!(!format!("{secret:?}").contains("SECRET123"));
        assert!(Secret::new("   ").is_none());
        let config =
            IssueTrackerConfig { site: Some("https://team.atlassian.net".into()), email: Some("me@example.test".into()), ..cfg("jira") };
        let status = config_json(&config, true, json!({ "glab": false })).to_string();
        assert!(!status.contains("SECRET123") && !status.to_ascii_lowercase().contains("token"));
        // The workspace state carries no token field either.
        let mut out = mighty_core::json::JsonOut::new();
        mighty_core::json::Wire::write(&config, &mut out);
        let written = out.finish();
        assert!(!written.to_ascii_lowercase().contains("token") && !written.contains("SECRET123"));
        let read: IssueTrackerConfig = mighty_core::json::Wire::read(&serde_json::from_str(&written).unwrap()).unwrap();
        assert_eq!(read, config);
        // Tokens are kept per tracker and per site.
        assert_eq!(secret_id(&config), "issue-tracker.jira.team.atlassian.net");
        assert_eq!(secret_id(&cfg("linear")), "issue-tracker.linear");
        assert_eq!(secret_id(&cfg("gitlab")), "issue-tracker.gitlab.gitlab.com");
    }

    #[test]
    fn sites_must_be_https_and_lose_paths_and_users() {
        assert_eq!(normalized_site("team.atlassian.net").as_deref(), Some("https://team.atlassian.net"));
        assert_eq!(normalized_site("https://Team.Atlassian.net/jira/your-work").as_deref(), Some("https://team.atlassian.net"));
        assert_eq!(normalized_site("https://gitlab.example.test:8443/").as_deref(), Some("https://gitlab.example.test:8443"));
        assert_eq!(normalized_site("http://gitlab.example.test"), None);
        assert_eq!(normalized_site("https://user:pw@gitlab.example.test"), None);
    }

    #[test]
    fn write_back_follows_each_switch() {
        let mut config = cfg("linear");
        assert_eq!(write_back_plan(&config, &WorkEvent::Started), (None, None));
        assert_eq!(
            write_back_plan(&config, &WorkEvent::PullRequest(Some("https://h/pr/1".into()))),
            (None, Some("Pull request: https://h/pr/1".into()))
        );
        assert_eq!(write_back_plan(&config, &WorkEvent::Merged), (None, None));
        config.write_start = Some(true);
        config.write_pr = Some(true);
        config.write_merge = Some(true);
        config.comment_pr = Some(false);
        assert_eq!(write_back_plan(&config, &WorkEvent::Started), (Some(IssueState::InProgress), None));
        assert_eq!(write_back_plan(&config, &WorkEvent::PullRequest(Some("u".into()))), (Some(IssueState::InReview), None));
        assert_eq!(write_back_plan(&config, &WorkEvent::Merged), (Some(IssueState::Done), None));
        config.enabled = false;
        assert_eq!(write_back_plan(&config, &WorkEvent::Merged), (None, None));
    }

    #[test]
    fn a_pull_request_body_names_its_issue_once() {
        let linked = LinkedTask {
            source: "linear".into(),
            project: Some("ENG".into()),
            key: "ENG-123".into(),
            title: None,
            url: Some("https://linear.app/t/issue/ENG-123".into()),
        };
        assert_eq!(pr_body_with_issue("", &linked), "Linear issue: [ENG-123](https://linear.app/t/issue/ENG-123)");
        let body = pr_body_with_issue("- fix login", &linked);
        assert_eq!(body, "- fix login\n\nLinear issue: [ENG-123](https://linear.app/t/issue/ENG-123)");
        assert_eq!(pr_body_with_issue(&body, &linked), body);
        let bare = LinkedTask { url: None, source: "jira".into(), key: "P-1".into(), ..linked };
        assert_eq!(pr_issue_line(&bare), "Jira issue: P-1");
    }

    #[test]
    fn settings_from_the_page_keep_what_they_do_not_name() {
        let current = IssueTrackerConfig { project: Some("ENG".into()), write_pr: Some(true), ..cfg("linear") };
        let next = config_from_args("linear", Some(&current), &json!({ "enabled": false, "writeStart": true })).unwrap();
        assert_eq!((next.enabled, next.project.as_deref(), next.write_pr, next.write_start), (false, Some("ENG"), Some(true), Some(true)));
        assert!(config_from_args("jira", None, &json!({ "site": "http://x.test" })).is_err());
        assert_eq!(default_filter(&cfg("jira")), "mine");
        assert_eq!(default_filter(&IssueTrackerConfig { filter_mine: Some(false), ..cfg("jira") }), "open");
    }

    #[test]
    fn sources_and_trackers_map_both_ways() {
        for tracker in ["github", "linear", "jira", "gitlab"] {
            assert_eq!(tracker_of_source(source_of_tracker(tracker).unwrap()), Some(tracker));
        }
        // A pane still linked to a MightyWork task or wiki page (saved by an older version) names no tracker.
        assert_eq!(tracker_of_source("mightywork_task"), None);
        assert_eq!(tracker_of_source("mightywork_wiki"), None);
        assert_eq!(source_of_tracker("mightywork"), None);
    }
}
