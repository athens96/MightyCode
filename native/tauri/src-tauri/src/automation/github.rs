//! MC-32's GitHub trigger: a local app cannot receive GitHub's webhooks, so it looks through `gh api` (the person's own sign-in), at
//! most every 5 minutes and only for workspaces with a GitHub trigger, sending the last ETag as `If-None-Match` (a 304 costs no rate
//! limit) and backing off while nothing changes or the look fails.
//!
//! - `pr_opened`: `repos/<repo>/pulls?state=open&sort=created&direction=desc&per_page=20`, a pull request not seen before.
//! - `review_requested`: `repos/<repo>/notifications?participating=true`, a notification whose reason is `review_requested`.
//! - `ci_failed`: `repos/<repo>/actions/runs?status=failure&per_page=10`, a failed workflow run not seen before.
//!
//! The first look of a watch only learns what is there.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use mighty_core::automation::spec::GitHubEvent;
use mighty_core::automation::watch::TriggerEvent;
use mighty_platform::SpawnRequest;
use serde_json::Value;

use crate::host::Host;

/// What `gh api -i` printed: the status, the headers (lower-case names) and the body.
#[derive(Debug, Clone, PartialEq)]
pub struct Answer {
    pub status: u16,
    pub headers: HashMap<String, String>,
    pub body: Value,
}

impl Answer {
    pub fn etag(&self) -> Option<&str> {
        self.headers.get("etag").map(String::as_str)
    }
}

/// `gh api -i`'s output read: the status line, the headers up to the blank line, then the JSON body (none for a 304).
pub fn parse_include(stdout: &[u8]) -> Option<Answer> {
    let text = String::from_utf8_lossy(stdout);
    let text = text.trim_start();
    let (head, body) = match text.find("\r\n\r\n") {
        Some(at) => (&text[..at], &text[at + 4..]),
        None => match text.find("\n\n") {
            Some(at) => (&text[..at], &text[at + 2..]),
            None => (text, ""),
        },
    };
    let mut lines = head.lines();
    let status: u16 = lines.next()?.split_whitespace().nth(1)?.parse().ok()?;
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_string()))
        .collect();
    let body = if body.trim().is_empty() { Value::Null } else { serde_json::from_str(body.trim()).unwrap_or(Value::Null) };
    Some(Answer { status, headers, body })
}

/// The endpoint one event is looked for at.
pub fn endpoint(event: GitHubEvent, repo: &str) -> String {
    match event {
        GitHubEvent::PrOpened => format!("repos/{repo}/pulls?state=open&sort=created&direction=desc&per_page=20"),
        GitHubEvent::ReviewRequested => format!("repos/{repo}/notifications?participating=true"),
        GitHubEvent::CiFailed => format!("repos/{repo}/actions/runs?status=failure&per_page=10"),
    }
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or("")
}

/// An API address of a pull request or issue as its page (`https://api.github.com/repos/o/r/pulls/5` → `https://github.com/o/r/pull/5`).
fn page_of(api: &str) -> Option<String> {
    let rest = api.strip_prefix("https://api.github.com/repos/")?;
    let parts: Vec<&str> = rest.split('/').collect();
    match parts.as_slice() {
        [owner, repo, "pulls", n] => Some(format!("https://github.com/{owner}/{repo}/pull/{n}")),
        [owner, repo, "issues", n] => Some(format!("https://github.com/{owner}/{repo}/issues/{n}")),
        _ => None,
    }
}

/// The events an answer holds, newest first as GitHub lists them; each with the id duplicates are dropped by.
pub fn events_of(event: GitHubEvent, body: &Value) -> Vec<TriggerEvent> {
    match event {
        GitHubEvent::PrOpened => body
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|pr| {
                let number = pr.get("number")?.as_u64()?;
                let author = pr.get("user").map(|u| text(u, "login")).unwrap_or("");
                Some(TriggerEvent {
                    id: format!("pr:{number}"),
                    title: format!("PR #{number}: {} ({author})", text(pr, "title")),
                    url: Some(text(pr, "html_url").to_string()).filter(|u| !u.is_empty()),
                    detail: String::new(),
                })
            })
            .collect(),
        GitHubEvent::ReviewRequested => body
            .as_array()
            .into_iter()
            .flatten()
            .filter(|n| text(n, "reason") == "review_requested")
            .filter_map(|n| {
                let subject = n.get("subject")?;
                Some(TriggerEvent {
                    id: format!("rr:{}:{}", text(n, "id"), text(n, "updated_at")),
                    title: format!("Review requested: {}", text(subject, "title")),
                    url: page_of(text(subject, "url")),
                    detail: String::new(),
                })
            })
            .collect(),
        GitHubEvent::CiFailed => body
            .get("workflow_runs")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|run| {
                let id = run.get("id")?.as_u64()?;
                Some(TriggerEvent {
                    id: format!("ci:{id}"),
                    title: format!("CI failed: {} on {} ({})", text(run, "name"), text(run, "head_branch"), text(run, "display_title")),
                    url: Some(text(run, "html_url").to_string()).filter(|u| !u.is_empty()),
                    detail: String::new(),
                })
            })
            .collect(),
    }
}

/// `gh`, the folder it runs in and the repository, for a workspace.
pub struct Gh {
    pub gh: PathBuf,
    pub cwd: PathBuf,
    pub repo: String,
}

impl Gh {
    pub async fn for_workspace(host: &Arc<Host>, workspace: &mighty_core::model::Workspace) -> Result<Gh, String> {
        let tracker = crate::trackers::github::GitHubTracker::for_workspace(host, workspace).await.map_err(|e| e.to_string())?;
        let gh = crate::integrations::gh_executable(host).ok_or("GitHub CLI (gh) is not installed.")?;
        let cwd = if workspace.path().is_empty() { std::env::temp_dir() } else { PathBuf::from(workspace.path()) };
        Ok(Gh { gh, cwd, repo: tracker.repo().to_string() })
    }

    /// One look at `endpoint`, with the last ETag.
    pub async fn look(&self, host: &Host, endpoint: &str, etag: Option<&str>) -> Result<Answer, String> {
        let mut args: Vec<String> = vec!["api".into(), "-i".into(), "-H".into(), "Accept: application/vnd.github+json".into()];
        if let Some(etag) = etag {
            args.push("-H".into());
            args.push(format!("If-None-Match: {etag}"));
        }
        args.push(endpoint.to_string());
        let request = SpawnRequest::new(self.gh.clone(), args, self.cwd.clone()).env("GH_PROMPT_DISABLED", "1").env("NO_COLOR", "1");
        let captured = mighty_platform::process::capture(host.processes.as_ref(), request, None, Duration::from_secs(20), 2_000_000)
            .await
            .map_err(|e| e.to_string())?;
        // `gh api` exits 1 for a 304 and for errors alike: the printed status decides.
        match parse_include(&captured.stdout) {
            Some(answer) if answer.status == 304 || (200..300).contains(&answer.status) => Ok(answer),
            Some(answer) => Err(format!("HTTP {}", answer.status)),
            None => Err(String::from_utf8_lossy(&captured.stderr).trim().chars().take(300).collect()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gh_include_output_is_read_with_its_etag_and_a_304_has_no_body() {
        let ok = b"HTTP/2.0 200 OK\r\nEtag: W/\"abc\"\r\nX-Ratelimit-Remaining: 4999\r\n\r\n[{\"number\":7,\"title\":\"Add login\",\"html_url\":\"https://github.com/a/b/pull/7\",\"user\":{\"login\":\"kim\"}}]";
        let answer = parse_include(ok).unwrap();
        assert_eq!((answer.status, answer.etag()), (200, Some("W/\"abc\"")));
        let events = events_of(GitHubEvent::PrOpened, &answer.body);
        assert_eq!(events[0].id, "pr:7");
        assert_eq!(events[0].title, "PR #7: Add login (kim)");
        assert_eq!(events[0].url.as_deref(), Some("https://github.com/a/b/pull/7"));
        let not_modified = parse_include(b"HTTP/2.0 304 Not Modified\r\nEtag: W/\"abc\"\r\n\r\n").unwrap();
        assert_eq!((not_modified.status, not_modified.body.clone()), (304, Value::Null));
        assert!(parse_include(b"gh: not logged in").is_none());
    }

    #[test]
    fn review_requests_and_failed_runs_are_read_from_their_answers() {
        let notifications = serde_json::json!([
            {"id": "11", "reason": "review_requested", "updated_at": "2026-11-23T01:00:00Z",
             "subject": {"title": "Fix cache", "url": "https://api.github.com/repos/a/b/pulls/9"}},
            {"id": "12", "reason": "mention", "updated_at": "2026-11-23T01:00:00Z", "subject": {"title": "x", "url": ""}}
        ]);
        let events = events_of(GitHubEvent::ReviewRequested, &notifications);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].id, "rr:11:2026-11-23T01:00:00Z");
        assert_eq!(events[0].url.as_deref(), Some("https://github.com/a/b/pull/9"));
        let runs = serde_json::json!({"total_count": 1, "workflow_runs": [
            {"id": 555, "name": "CI", "head_branch": "main", "display_title": "Bump deps", "html_url": "https://github.com/a/b/actions/runs/555"}
        ]});
        let events = events_of(GitHubEvent::CiFailed, &runs);
        assert_eq!((events[0].id.as_str(), events[0].title.as_str()), ("ci:555", "CI failed: CI on main (Bump deps)"));
        assert!(events_of(GitHubEvent::CiFailed, &Value::Null).is_empty());
        assert_eq!(endpoint(GitHubEvent::CiFailed, "a/b"), "repos/a/b/actions/runs?status=failure&per_page=10");
    }
}
