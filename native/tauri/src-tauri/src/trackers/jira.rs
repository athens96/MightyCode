//! MC-31: Jira Cloud over REST v3 with the account's email and an API token (Basic). `search/jql` lists, `issue/{key}` reads (its
//! ADF description as Markdown), `issue/{key}/transitions` moves an issue (a transition named for the state, else one into the
//! matching status category), and `issue/{key}/comment` comments (Markdown as ADF).

use std::sync::Arc;

use base64::Engine as _;
use mighty_core::model::IssueTrackerConfig;
use serde_json::{Value, json};

use super::{IssueDetail, IssueQuery, IssueState, IssueSummary, IssueTracker, Secret, TrackerError};
use crate::host::Host;

const LIST_FIELDS: [&str; 6] = ["summary", "status", "labels", "updated", "priority", "assignee"];

pub(crate) struct JiraTracker {
    host: Arc<Host>,
    site: String,
    authorization: Secret,
    project: Option<String>,
}

impl JiraTracker {
    pub(crate) async fn new(host: &Arc<Host>, config: &IssueTrackerConfig) -> Result<JiraTracker, TrackerError> {
        let site = config.site.as_deref().and_then(super::normalized_site).ok_or(TrackerError::NotConfigured("jira"))?;
        let email = config.email.as_deref().map(str::trim).filter(|e| !e.is_empty()).ok_or(TrackerError::NotConfigured("jira"))?;
        let token = super::load_token(host, config).await.ok_or(TrackerError::AuthRequired)?;
        let basic = base64::engine::general_purpose::STANDARD.encode(format!("{email}:{}", token.expose()));
        let authorization = Secret::new(&format!("Basic {basic}")).ok_or(TrackerError::AuthRequired)?;
        Ok(JiraTracker { host: host.clone(), site, authorization, project: config.project.clone().filter(|p| !p.trim().is_empty()) })
    }

    async fn call(&self, path: &str, body: Option<&Value>) -> Result<Value, TrackerError> {
        let headers = vec![("Authorization".to_string(), self.authorization.expose().to_string())];
        super::https_json(&self.host, &format!("{}{path}", self.site), headers, body, api_error).await
    }

    /// The signed-in account's display name (the settings' connection test).
    pub(crate) async fn myself(&self) -> Result<String, TrackerError> {
        let me = self.call("/rest/api/3/myself", None).await?;
        Ok(me.get("displayName").and_then(Value::as_str).unwrap_or("").to_string())
    }
}

impl IssueTracker for JiraTracker {
    fn id(&self) -> &'static str {
        "jira"
    }

    async fn list(&self, query: &IssueQuery) -> Result<Vec<IssueSummary>, TrackerError> {
        let body = json!({ "jql": jql(self.project.as_deref(), query), "maxResults": 50, "fields": LIST_FIELDS });
        Ok(parse_list(&self.call("/rest/api/3/search/jql", Some(&body)).await?, &self.site))
    }

    async fn get(&self, key: &str) -> Result<IssueDetail, TrackerError> {
        let key = checked_key(key)?;
        let fields = "summary,status,labels,updated,priority,assignee,description,subtasks";
        let raw = self.call(&format!("/rest/api/3/issue/{key}?fields={fields}"), None).await?;
        parse_detail(&raw, &self.site).ok_or(TrackerError::BadKey)
    }

    async fn set_state(&self, key: &str, state: IssueState) -> Result<(), TrackerError> {
        let key = checked_key(key)?;
        let path = format!("/rest/api/3/issue/{key}/transitions");
        let transitions = self.call(&path, None).await?;
        let Some(id) = pick_transition(&transitions, state) else {
            return Err(TrackerError::Api("No matching Jira transition.".into()));
        };
        self.call(&path, Some(&json!({ "transition": { "id": id } }))).await.map(|_| ())
    }

    async fn comment(&self, key: &str, body: &str) -> Result<(), TrackerError> {
        let key = checked_key(key)?;
        let adf = super::adf::from_markdown(body);
        self.call(&format!("/rest/api/3/issue/{key}/comment"), Some(&json!({ "body": adf }))).await.map(|_| ())
    }
}

/// A Jira issue key (`PROJ-12`), upper-cased; anything else (that could reach another path) refused.
pub(crate) fn checked_key(key: &str) -> Result<String, TrackerError> {
    let key = key.trim().to_ascii_uppercase();
    let (project, number) = key.split_once('-').ok_or(TrackerError::BadKey)?;
    let project_ok = project.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && project.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && project.len() <= 32;
    let number_ok = !number.is_empty() && number.len() <= 12 && number.chars().all(|c| c.is_ascii_digit());
    if project_ok && number_ok { Ok(key) } else { Err(TrackerError::BadKey) }
}

/// Jira's `errorMessages` / `errors`, as one line.
pub(crate) fn api_error(answer: &Value) -> Option<String> {
    let mut parts: Vec<String> = answer
        .get("errorMessages")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default();
    if let Some(errors) = answer.get("errors").and_then(Value::as_object) {
        parts.extend(errors.iter().filter_map(|(k, v)| v.as_str().map(|v| format!("{k}: {v}"))));
    }
    (!parts.is_empty()).then(|| parts.join("; "))
}

fn quoted(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

/// The JQL of a list: the project, the filter chip (assigned to me, not done) and the search text, newest first. A query with no
/// clause of its own is bounded to the last 90 days (`search/jql` refuses unbounded ones).
pub(crate) fn jql(project: Option<&str>, query: &IssueQuery) -> String {
    let mut clauses = Vec::new();
    if let Some(project) = project.map(str::trim).filter(|p| !p.is_empty()) {
        clauses.push(format!("project = {}", quoted(project)));
    }
    if query.filter == "mine" {
        clauses.push("assignee = currentUser()".to_string());
    }
    if matches!(query.filter.as_str(), "mine" | "open") {
        clauses.push("statusCategory != Done".to_string());
    }
    let text = query.text.trim();
    if !text.is_empty() {
        match checked_key(text) {
            Ok(key) => clauses.push(format!("(key = {} OR text ~ {})", quoted(&key), quoted(text))),
            Err(_) => clauses.push(format!("text ~ {}", quoted(text))),
        }
    }
    if clauses.is_empty() {
        clauses.push("updated >= -90d".to_string());
    }
    format!("{} ORDER BY updated DESC", clauses.join(" AND "))
}

fn summary(issue: &Value, site: &str) -> Option<IssueSummary> {
    let key = issue.get("key")?.as_str()?.to_string();
    let fields = issue.get("fields").unwrap_or(&Value::Null);
    Some(IssueSummary {
        title: fields.get("summary").and_then(Value::as_str).unwrap_or(&key).to_string(),
        state: fields.pointer("/status/name").and_then(Value::as_str).map(str::to_string),
        priority: fields.pointer("/priority/name").and_then(Value::as_str).map(str::to_string),
        labels: fields
            .get("labels")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
            .unwrap_or_default(),
        url: Some(format!("{site}/browse/{key}")),
        updated_at: fields.get("updated").and_then(Value::as_str).map(str::to_string),
        key,
    })
}

/// `search/jql`'s answer as rows.
pub(crate) fn parse_list(answer: &Value, site: &str) -> Vec<IssueSummary> {
    answer.get("issues").and_then(Value::as_array).map(|a| a.iter().filter_map(|i| summary(i, site)).collect()).unwrap_or_default()
}

/// `issue/{key}`'s answer: the description as Markdown and the sub-tasks as a checklist.
pub(crate) fn parse_detail(issue: &Value, site: &str) -> Option<IssueDetail> {
    let fields = issue.get("fields").unwrap_or(&Value::Null);
    let body = match fields.get("description") {
        Some(Value::String(text)) => text.clone(),
        Some(doc) if doc.is_object() => super::adf::to_markdown(doc),
        _ => String::new(),
    };
    Some(IssueDetail {
        summary: summary(issue, site)?,
        body,
        assignee: fields.pointer("/assignee/displayName").and_then(Value::as_str).map(str::to_string),
        checklist: fields
            .get("subtasks")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|s| {
                        let key = s.get("key").and_then(Value::as_str).unwrap_or("");
                        let title = s.pointer("/fields/summary").and_then(Value::as_str)?;
                        let done = s.pointer("/fields/status/statusCategory/key").and_then(Value::as_str) == Some("done");
                        Some((if key.is_empty() { title.to_string() } else { format!("{key} {title}") }, done))
                    })
                    .collect()
            })
            .unwrap_or_default(),
    })
}

/// The transition that moves an issue to `state`: one whose name (or target status's name) is the state's usual name, else for in
/// progress and done one into the `indeterminate` / `done` status category. In review has no category, so only a name with "review".
pub(crate) fn pick_transition(answer: &Value, state: IssueState) -> Option<String> {
    let transitions = answer.get("transitions")?.as_array()?;
    let names: &[&str] = match state {
        IssueState::InProgress => &["in progress", "start progress"],
        IssueState::InReview => &["in review", "code review", "review"],
        IssueState::Done => &["done", "closed", "resolved"],
    };
    let lower = |v: Option<&Value>| v.and_then(Value::as_str).unwrap_or("").trim().to_lowercase();
    let id = |t: &Value| t.get("id").and_then(|v| v.as_str().map(str::to_string).or_else(|| v.as_i64().map(|n| n.to_string())));
    for wanted in names {
        if let Some(found) = transitions.iter().find(|t| lower(t.get("name")) == *wanted || lower(t.pointer("/to/name")) == *wanted) {
            return id(found);
        }
    }
    if state == IssueState::InReview {
        return transitions
            .iter()
            .find(|t| lower(t.get("name")).contains("review") || lower(t.pointer("/to/name")).contains("review"))
            .and_then(id);
    }
    let category = if state == IssueState::Done { "done" } else { "indeterminate" };
    transitions.iter().find(|t| t.pointer("/to/statusCategory/key").and_then(Value::as_str) == Some(category)).and_then(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SITE: &str = "https://acme.atlassian.net";

    #[test]
    fn parses_search_and_issue_answers() {
        let search: Value = serde_json::from_str(
            r#"{"issues":[
                {"id":"10001","key":"PROJ-12","fields":{"summary":"Crash on save","status":{"name":"To Do","statusCategory":{"key":"new"}},
                 "labels":["bug"],"updated":"2026-11-20T10:00:00.000+0900","priority":{"name":"High"},"assignee":{"displayName":"Kim"}}},
                {"id":"10002","key":"PROJ-13","fields":{"summary":"Plain","status":null,"labels":[],"priority":null}},
                {"id":"x"}
            ],"nextPageToken":"abc"}"#,
        )
        .unwrap();
        let rows = parse_list(&search, SITE);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].key, "PROJ-12");
        assert_eq!(rows[0].title, "Crash on save");
        assert_eq!(rows[0].state.as_deref(), Some("To Do"));
        assert_eq!(rows[0].priority.as_deref(), Some("High"));
        assert_eq!(rows[0].labels, vec!["bug"]);
        assert_eq!(rows[0].url.as_deref(), Some("https://acme.atlassian.net/browse/PROJ-12"));
        assert_eq!(rows[1].state, None);

        let issue: Value = serde_json::from_str(
            r#"{"key":"PROJ-12","fields":{"summary":"Crash on save","status":{"name":"In Progress"},"labels":[],
               "assignee":{"displayName":"Kim"},
               "description":{"type":"doc","version":1,"content":[{"type":"paragraph","content":[{"type":"text","text":"It crashes."}]}]},
               "subtasks":[{"key":"PROJ-14","fields":{"summary":"Repro","status":{"statusCategory":{"key":"done"}}}},
                           {"key":"PROJ-15","fields":{"summary":"Fix","status":{"statusCategory":{"key":"indeterminate"}}}}]}}"#,
        )
        .unwrap();
        let detail = parse_detail(&issue, SITE).unwrap();
        assert_eq!(detail.body, "It crashes.");
        assert_eq!(detail.assignee.as_deref(), Some("Kim"));
        assert_eq!(detail.checklist, vec![("PROJ-14 Repro".to_string(), true), ("PROJ-15 Fix".to_string(), false)]);
        let no_description = json!({ "key": "PROJ-1", "fields": { "summary": "x", "description": null } });
        assert_eq!(parse_detail(&no_description, SITE).unwrap().body, "");
        assert_eq!(
            api_error(&json!({ "errorMessages": ["Issue does not exist"], "errors": { "jql": "bad" } })).as_deref(),
            Some("Issue does not exist; jql: bad")
        );
    }

    #[test]
    fn jql_follows_the_chip_and_quotes_text() {
        assert_eq!(
            jql(Some("PROJ"), &IssueQuery { filter: "mine".into(), text: "say \"hi\"".into() }),
            "project = \"PROJ\" AND assignee = currentUser() AND statusCategory != Done AND text ~ \"say \\\"hi\\\"\" ORDER BY updated DESC"
        );
        assert_eq!(
            jql(None, &IssueQuery { filter: "open".into(), text: "proj-7".into() }),
            "statusCategory != Done AND (key = \"PROJ-7\" OR text ~ \"proj-7\") ORDER BY updated DESC"
        );
        assert_eq!(jql(None, &IssueQuery { filter: "all".into(), text: String::new() }), "updated >= -90d ORDER BY updated DESC");
    }

    #[test]
    fn keys_are_checked_before_they_reach_a_path() {
        assert_eq!(checked_key(" proj-12 ").unwrap(), "PROJ-12");
        for bad in ["PROJ", "PROJ-", "../x-1", "PROJ-1/comment", "1PROJ-2", "PROJ-1a"] {
            assert!(checked_key(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn states_map_to_transitions() {
        let answer = json!({ "transitions": [
            { "id": "11", "name": "To Do", "to": { "name": "To Do", "statusCategory": { "key": "new" } } },
            { "id": "21", "name": "Start", "to": { "name": "Doing", "statusCategory": { "key": "indeterminate" } } },
            { "id": "31", "name": "Send to review", "to": { "name": "Code Review", "statusCategory": { "key": "indeterminate" } } },
            { "id": "41", "name": "Finish", "to": { "name": "Shipped", "statusCategory": { "key": "done" } } }
        ]});
        assert_eq!(pick_transition(&answer, IssueState::InProgress).as_deref(), Some("21"));
        assert_eq!(pick_transition(&answer, IssueState::InReview).as_deref(), Some("31"));
        assert_eq!(pick_transition(&answer, IssueState::Done).as_deref(), Some("41"));
        let named = json!({ "transitions": [
            { "id": 5, "name": "Review", "to": { "name": "Review", "statusCategory": { "key": "indeterminate" } } },
            { "id": 6, "name": "In Progress", "to": { "name": "In Progress", "statusCategory": { "key": "indeterminate" } } },
            { "id": 7, "name": "Done", "to": { "name": "Done", "statusCategory": { "key": "done" } } }
        ]});
        assert_eq!(pick_transition(&named, IssueState::InProgress).as_deref(), Some("6"));
        assert_eq!(pick_transition(&named, IssueState::InReview).as_deref(), Some("5"));
        assert_eq!(pick_transition(&named, IssueState::Done).as_deref(), Some("7"));
        let none = json!({ "transitions": [{ "id": "1", "name": "Start", "to": { "name": "Doing", "statusCategory": { "key": "indeterminate" } } }] });
        assert_eq!(pick_transition(&none, IssueState::InReview), None);
    }
}
