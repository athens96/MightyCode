//! MC-31: Linear over its GraphQL API (`https://api.linear.app/graphql`) with a personal API key (sent as `Authorization: <key>`).
//! `issues` lists, `issue` reads, `issueUpdate` moves an issue to its team's workflow state of type `started` (in progress, or one named
//! for review) or `completed`, and `commentCreate` comments. OAuth is not offered: Linear needs a registered client, which the
//! MightyWork sign-in (dynamic registration) does not fit.

use std::sync::Arc;

use mighty_core::model::IssueTrackerConfig;
use serde_json::{Value, json};

use super::{IssueDetail, IssueQuery, IssueState, IssueSummary, IssueTracker, Secret, TrackerError};
use crate::host::Host;

pub(crate) const ENDPOINT: &str = "https://api.linear.app/graphql";

const ISSUE_FIELDS: &str =
    "id identifier title url updatedAt priorityLabel state { name type } labels { nodes { name } } assignee { name }";

pub(crate) struct LinearTracker {
    host: Arc<Host>,
    token: Secret,
    team: Option<String>,
}

impl LinearTracker {
    pub(crate) async fn new(host: &Arc<Host>, config: &IssueTrackerConfig) -> Result<LinearTracker, TrackerError> {
        let token = super::load_token(host, config).await.ok_or(TrackerError::AuthRequired)?;
        Ok(LinearTracker { host: host.clone(), token, team: config.project.clone().filter(|t| !t.trim().is_empty()) })
    }

    async fn graphql(&self, query: &str, variables: Value) -> Result<Value, TrackerError> {
        let headers = vec![("Authorization".to_string(), self.token.expose().to_string())];
        let answer =
            super::https_json(&self.host, ENDPOINT, headers, Some(&json!({ "query": query, "variables": variables })), graphql_error)
                .await?;
        if let Some(message) = graphql_error(&answer) {
            return Err(TrackerError::Api(message));
        }
        Ok(answer.get("data").cloned().unwrap_or(Value::Null))
    }

    /// The signed-in user's name (the settings' connection test).
    pub(crate) async fn viewer(&self) -> Result<String, TrackerError> {
        let data = self.graphql("query { viewer { id name } }", json!({})).await?;
        Ok(data.pointer("/viewer/name").and_then(Value::as_str).unwrap_or("").to_string())
    }
}

impl IssueTracker for LinearTracker {
    fn id(&self) -> &'static str {
        "linear"
    }

    async fn list(&self, query: &IssueQuery) -> Result<Vec<IssueSummary>, TrackerError> {
        let (text, variables) = list_request(self.team.as_deref(), query);
        Ok(parse_list(&self.graphql(&text, variables).await?))
    }

    async fn get(&self, key: &str) -> Result<IssueDetail, TrackerError> {
        let data = self.graphql(&detail_query(), json!({ "id": key })).await?;
        parse_detail(&data).ok_or(TrackerError::BadKey)
    }

    async fn set_state(&self, key: &str, state: IssueState) -> Result<(), TrackerError> {
        let query = "query($id: String!) { issue(id: $id) { id state { id } team { states { nodes { id name type position } } } } }";
        let data = self.graphql(query, json!({ "id": key })).await?;
        let issue = data.get("issue").filter(|v| !v.is_null()).ok_or(TrackerError::BadKey)?;
        let Some(target) = pick_state(issue.pointer("/team/states/nodes").unwrap_or(&Value::Null), state) else {
            return Err(TrackerError::Api(format!("No {} workflow state in the team.", state_word(state))));
        };
        if issue.pointer("/state/id").and_then(Value::as_str) == Some(target.as_str()) {
            return Ok(());
        }
        let id = issue.get("id").and_then(Value::as_str).unwrap_or(key);
        let mutation = "mutation($id: String!, $stateId: String!) { issueUpdate(id: $id, input: { stateId: $stateId }) { success } }";
        let done = self.graphql(mutation, json!({ "id": id, "stateId": target })).await?;
        success(&done, "issueUpdate")
    }

    async fn comment(&self, key: &str, body: &str) -> Result<(), TrackerError> {
        let data = self.graphql("query($id: String!) { issue(id: $id) { id } }", json!({ "id": key })).await?;
        let id = data.pointer("/issue/id").and_then(Value::as_str).ok_or(TrackerError::BadKey)?;
        let mutation =
            "mutation($issueId: String!, $body: String!) { commentCreate(input: { issueId: $issueId, body: $body }) { success } }";
        let done = self.graphql(mutation, json!({ "issueId": id, "body": body })).await?;
        success(&done, "commentCreate")
    }
}

fn state_word(state: IssueState) -> &'static str {
    match state {
        IssueState::InProgress => "in-progress",
        IssueState::InReview => "review",
        IssueState::Done => "completed",
    }
}

fn success(data: &Value, field: &str) -> Result<(), TrackerError> {
    if data.get(field).and_then(|f| f.get("success")).and_then(Value::as_bool) == Some(true) {
        Ok(())
    } else {
        Err(TrackerError::Api(format!("Linear {field} did not succeed.")))
    }
}

/// GraphQL's first error message.
pub(crate) fn graphql_error(answer: &Value) -> Option<String> {
    let first = answer.get("errors")?.as_array()?.first()?;
    Some(first.get("message").and_then(Value::as_str).unwrap_or("Linear error").to_string())
}

/// The `issues` query and its variables for a team (or every team) and a filter chip.
pub(crate) fn list_request(team: Option<&str>, query: &IssueQuery) -> (String, Value) {
    let mut filter = serde_json::Map::new();
    if let Some(team) = team.map(str::trim).filter(|t| !t.is_empty()) {
        filter.insert("team".into(), json!({ "key": { "eq": team } }));
    }
    if query.filter == "mine" {
        filter.insert("assignee".into(), json!({ "isMe": { "eq": true } }));
    }
    if matches!(query.filter.as_str(), "mine" | "open") {
        filter.insert("state".into(), json!({ "type": { "nin": ["completed", "canceled"] } }));
    }
    if !query.text.trim().is_empty() {
        filter.insert("title".into(), json!({ "containsIgnoreCase": query.text.trim() }));
    }
    let text = format!(
        "query($filter: IssueFilter, $first: Int) {{ issues(filter: $filter, first: $first, orderBy: updatedAt) {{ nodes {{ {ISSUE_FIELDS} }} }} }}"
    );
    (text, json!({ "filter": Value::Object(filter), "first": 50 }))
}

pub(crate) fn detail_query() -> String {
    format!(
        "query($id: String!) {{ issue(id: $id) {{ {ISSUE_FIELDS} description team {{ key }} children {{ nodes {{ identifier title state {{ type }} }} }} }} }}"
    )
}

fn summary(node: &Value) -> Option<IssueSummary> {
    let key = node.get("identifier")?.as_str()?.to_string();
    Some(IssueSummary {
        title: node.get("title").and_then(Value::as_str).unwrap_or(&key).to_string(),
        state: node.pointer("/state/name").and_then(Value::as_str).map(str::to_string),
        priority: node.get("priorityLabel").and_then(Value::as_str).filter(|p| !p.is_empty() && *p != "No priority").map(str::to_string),
        labels: node
            .pointer("/labels/nodes")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|l| l.get("name").and_then(Value::as_str)).map(str::to_string).collect())
            .unwrap_or_default(),
        url: node.get("url").and_then(Value::as_str).map(str::to_string),
        updated_at: node.get("updatedAt").and_then(Value::as_str).map(str::to_string),
        key,
    })
}

/// `issues`' answer (its `data`) as rows.
pub(crate) fn parse_list(data: &Value) -> Vec<IssueSummary> {
    data.pointer("/issues/nodes").and_then(Value::as_array).map(|a| a.iter().filter_map(summary).collect()).unwrap_or_default()
}

/// `issue`'s answer (its `data`): the issue with its Markdown description, and its sub-issues as a checklist.
pub(crate) fn parse_detail(data: &Value) -> Option<IssueDetail> {
    let node = data.get("issue").filter(|v| !v.is_null())?;
    Some(IssueDetail {
        summary: summary(node)?,
        body: node.get("description").and_then(Value::as_str).unwrap_or("").to_string(),
        assignee: node.pointer("/assignee/name").and_then(Value::as_str).map(str::to_string),
        checklist: node
            .pointer("/children/nodes")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|c| {
                        let id = c.get("identifier").and_then(Value::as_str).unwrap_or("");
                        let title = c.get("title").and_then(Value::as_str)?;
                        let done = matches!(c.pointer("/state/type").and_then(Value::as_str), Some("completed" | "canceled"));
                        Some((if id.is_empty() { title.to_string() } else { format!("{id} {title}") }, done))
                    })
                    .collect()
            })
            .unwrap_or_default(),
    })
}

/// The workflow state an issue moves to: for in progress a `started` state not named for review (`In Progress` first, else the
/// first by position); for in review a `started` state whose name has "review"; for done a `completed` state (`Done` first).
pub(crate) fn pick_state(states: &Value, state: IssueState) -> Option<String> {
    let mut nodes: Vec<&Value> = states.as_array()?.iter().collect();
    nodes.sort_by(|a, b| {
        let pa = a.get("position").and_then(Value::as_f64).unwrap_or(0.0);
        let pb = b.get("position").and_then(Value::as_f64).unwrap_or(0.0);
        pa.partial_cmp(&pb).unwrap_or(std::cmp::Ordering::Equal)
    });
    let kind = |n: &Value| n.get("type").and_then(Value::as_str).unwrap_or("").to_string();
    let name = |n: &Value| n.get("name").and_then(Value::as_str).unwrap_or("").to_ascii_lowercase();
    let id = |n: &&Value| n.get("id").and_then(Value::as_str).map(str::to_string);
    let (wanted_type, preferred): (&str, &str) = match state {
        IssueState::InProgress => ("started", "in progress"),
        IssueState::InReview => ("started", ""),
        IssueState::Done => ("completed", "done"),
    };
    let candidates: Vec<&Value> = nodes
        .into_iter()
        .filter(|n| kind(n) == wanted_type)
        .filter(|n| match state {
            IssueState::InProgress => !name(n).contains("review"),
            IssueState::InReview => name(n).contains("review"),
            IssueState::Done => true,
        })
        .collect();
    candidates.iter().find(|n| !preferred.is_empty() && name(n) == preferred).or_else(|| candidates.first()).and_then(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &str = r#"{"data":{"issues":{"nodes":[
        {"id":"u1","identifier":"ENG-123","title":"Fix login","url":"https://linear.app/acme/issue/ENG-123/fix-login",
         "updatedAt":"2026-11-20T10:00:00.000Z","priorityLabel":"High","state":{"name":"Todo","type":"unstarted"},
         "labels":{"nodes":[{"name":"bug"},{"name":"auth"}]},"assignee":{"name":"Kim"}},
        {"id":"u2","identifier":"ENG-7","title":"No priority","url":null,"updatedAt":null,"priorityLabel":"No priority",
         "state":null,"labels":{"nodes":[]},"assignee":null},
        {"id":"u3","title":"no identifier"}
    ]}}}"#;

    const DETAIL: &str = r#"{"data":{"issue":{"id":"u1","identifier":"ENG-123","title":"Fix login",
        "url":"https://linear.app/acme/issue/ENG-123/fix-login","updatedAt":"2026-11-20T10:00:00.000Z","priorityLabel":"Urgent",
        "state":{"name":"In Progress","type":"started"},"labels":{"nodes":[{"name":"bug"}]},"assignee":{"name":"Kim"},
        "description":"Steps:\n\n- open /login","team":{"key":"ENG"},
        "children":{"nodes":[{"identifier":"ENG-124","title":"Add test","state":{"type":"completed"}},
                             {"identifier":"ENG-125","title":"Docs","state":{"type":"backlog"}}]}}}}"#;

    #[test]
    fn parses_issue_lists_and_details() {
        let answer: Value = serde_json::from_str(LIST).unwrap();
        let rows = parse_list(&answer["data"]);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].key, "ENG-123");
        assert_eq!(rows[0].title, "Fix login");
        assert_eq!(rows[0].state.as_deref(), Some("Todo"));
        assert_eq!(rows[0].priority.as_deref(), Some("High"));
        assert_eq!(rows[0].labels, vec!["bug", "auth"]);
        assert_eq!(rows[0].url.as_deref(), Some("https://linear.app/acme/issue/ENG-123/fix-login"));
        assert_eq!((rows[1].priority.clone(), rows[1].state.clone(), rows[1].url.clone()), (None, None, None));

        let answer: Value = serde_json::from_str(DETAIL).unwrap();
        let detail = parse_detail(&answer["data"]).unwrap();
        assert_eq!(detail.summary.key, "ENG-123");
        assert_eq!(detail.body, "Steps:\n\n- open /login");
        assert_eq!(detail.assignee.as_deref(), Some("Kim"));
        assert_eq!(detail.checklist, vec![("ENG-124 Add test".to_string(), true), ("ENG-125 Docs".to_string(), false)]);
        assert!(parse_detail(&json!({ "issue": null })).is_none());
        assert_eq!(graphql_error(&json!({ "errors": [{ "message": "Entity not found" }] })).as_deref(), Some("Entity not found"));
    }

    #[test]
    fn list_filters_follow_the_chip_and_team() {
        let (text, vars) = list_request(Some("ENG"), &IssueQuery { filter: "mine".into(), text: "login".into() });
        assert!(text.contains("issues(filter: $filter, first: $first, orderBy: updatedAt)"));
        assert_eq!(
            vars,
            json!({ "first": 50, "filter": {
                "team": { "key": { "eq": "ENG" } },
                "assignee": { "isMe": { "eq": true } },
                "state": { "type": { "nin": ["completed", "canceled"] } },
                "title": { "containsIgnoreCase": "login" }
            }})
        );
        let (_, open) = list_request(None, &IssueQuery { filter: "open".into(), text: String::new() });
        assert_eq!(open["filter"], json!({ "state": { "type": { "nin": ["completed", "canceled"] } } }));
        let (_, all) = list_request(Some(" "), &IssueQuery { filter: "all".into(), text: String::new() });
        assert_eq!(all["filter"], json!({}));
    }

    #[test]
    fn states_map_to_started_and_completed_workflow_states() {
        let states = json!([
            { "id": "s-done", "name": "Done", "type": "completed", "position": 5 },
            { "id": "s-review", "name": "In Review", "type": "started", "position": 3 },
            { "id": "s-prog", "name": "In Progress", "type": "started", "position": 2 },
            { "id": "s-todo", "name": "Todo", "type": "unstarted", "position": 1 },
            { "id": "s-merged", "name": "Merged", "type": "completed", "position": 4 },
            { "id": "s-cancel", "name": "Canceled", "type": "canceled", "position": 6 }
        ]);
        assert_eq!(pick_state(&states, IssueState::InProgress).as_deref(), Some("s-prog"));
        assert_eq!(pick_state(&states, IssueState::InReview).as_deref(), Some("s-review"));
        assert_eq!(pick_state(&states, IssueState::Done).as_deref(), Some("s-done"));
        let plain = json!([
            { "id": "a", "name": "Doing", "type": "started", "position": 2 },
            { "id": "b", "name": "Shipped", "type": "completed", "position": 3 }
        ]);
        assert_eq!(pick_state(&plain, IssueState::InProgress).as_deref(), Some("a"));
        assert_eq!(pick_state(&plain, IssueState::InReview), None);
        assert_eq!(pick_state(&plain, IssueState::Done).as_deref(), Some("b"));
    }
}
