//! GitHub (`gh` CLI) workspace integration (`MC-11`):
//! - Per-workspace GitHub `owner/repo` mapping with automatic repository detection
//! - Browsing & searching GitHub issues
//! - Attaching rich context to the composer or launching directly in a new pane / isolated git worktree
//! - MC-31: issues go through `trackers.rs` (GitHub, Linear, Jira, GitLab); Linear, Jira and GitLab issues start on their own
//!   branch and write back as each workspace's switches say

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use mighty_core::model::{IssueTrackerConfig, LinkedTask, PaneWorktree, RunSession, Workspace};
use mighty_platform::SpawnRequest;
use serde_json::{Value, json};

use crate::host::{Host, Registry};
use crate::methods::{flag, text};
use crate::trackers::IssueTracker as _;

pub(crate) async fn detect_github_repo(host: &Host, workspace_path: &Path) -> Option<String> {
    let git = crate::workspace_create::git_executable()?;
    if !workspace_path.is_dir() {
        return None;
    }
    let req = SpawnRequest::new(git, ["remote", "get-url", "origin"], workspace_path.to_path_buf()).env("GIT_TERMINAL_PROMPT", "0");
    let captured = mighty_platform::process::capture(host.processes.as_ref(), req, None, Duration::from_secs(3), 4096).await.ok()?;
    if captured.exit_code != 0 {
        return None;
    }
    let origin = String::from_utf8_lossy(&captured.stdout).trim().to_string();
    mighty_delegation::pane_worktree::github_repository(&origin)
}

pub(crate) fn gh_executable(host: &Host) -> Option<PathBuf> {
    let env = host.runs().map(|r| r.catalog.environment().current().values).unwrap_or_default();
    crate::platform::cli_copies("gh", &env).into_iter().next().map(PathBuf::from)
}

/// Generates a git-worktree-safe session id (`^[a-zA-Z0-9][a-zA-Z0-9._-]{0,127}$`) embedding the task/issue key.
pub fn make_task_session_id(key: &str) -> String {
    let cleaned: String = key
        .trim_start_matches('#')
        .chars()
        .flat_map(char::to_lowercase)
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let prefix = if cleaned.is_empty() { "task".to_string() } else { cleaned.chars().take(24).collect() };
    let suffix: String = uuid::Uuid::new_v4().simple().to_string().chars().take(8).collect();
    format!("{prefix}-{suffix}")
}

/// Formats rich markdown context for composer attachment or initial agent prompt.
pub fn format_item_context(
    source: &str,
    key: &str,
    title: &str,
    stage: Option<&str>,
    url: Option<&str>,
    body: &str,
    checklist: &[(String, bool)],
) -> String {
    let mut lines = Vec::new();
    let header_tag = match source {
        "github_issue" => format!("[GitHub {key}]"),
        "linear_issue" => format!("[Linear {key}]"),
        "jira_issue" => format!("[Jira {key}]"),
        "gitlab_issue" => format!("[GitLab {key}]"),
        _ => format!("[{key}]"),
    };
    lines.push(format!("{header_tag} {title}"));
    if let Some(st) = stage.filter(|s| !s.is_empty()) {
        lines.push(format!("Status: {st}"));
    }
    if let Some(u) = url.filter(|s| !s.is_empty()) {
        lines.push(format!("URL: {u}"));
    }
    if !body.trim().is_empty() {
        lines.push(String::new());
        lines.push(body.trim().to_string());
    }
    if !checklist.is_empty() {
        lines.push(String::new());
        lines.push("Checklist:".into());
        for (item_text, done) in checklist {
            lines.push(format!("- [{}] {}", if *done { "x" } else { " " }, item_text));
        }
    }
    lines.join("\n")
}

/// Hooked from `pane_worktree.rs` when a worktree's PR push succeeds.
pub fn on_worktree_pr(host: &Arc<Host>, pane_id: &str, pane: &PaneWorktree, compare_url: Option<&str>) {
    // Only a pane that works on a worktree branch of its own moves its task: a record without one is no pull request of that task.
    if pane.branch.as_deref().is_none_or(str::is_empty) {
        return;
    }
    let (ws_id, linked) = {
        let view = host.view();
        let session = view.snapshot.session(pane_id);
        let ws_id = session.map(|s| s.workspace_id().to_string()).unwrap_or_default();
        let linked = pane.linked_task.clone().or_else(|| session.and_then(|s| s.linked_task.clone()));
        (ws_id, linked)
    };
    let Some(linked) = linked else { return };
    let event = crate::trackers::WorkEvent::PullRequest(compare_url.map(str::to_string));
    crate::trackers::spawn_write_back(host, &ws_id, &linked, event, Some(pane_id.to_string()));
}

/// Hooked from `pane_worktree.rs` when an open worktree pane merges into its base branch.
pub fn on_worktree_merged(host: &Arc<Host>, pane_id: &str, pane: &PaneWorktree) {
    let (ws_id, linked) = {
        let view = host.view();
        let session = view.snapshot.session(pane_id);
        let ws_id = session.map(|s| s.workspace_id().to_string()).unwrap_or_default();
        let linked = pane.linked_task.clone().or_else(|| session.and_then(|s| s.linked_task.clone()));
        (ws_id, linked)
    };
    let Some(linked) = linked else { return };
    crate::trackers::spawn_write_back(host, &ws_id, &linked, crate::trackers::WorkEvent::Merged, Some(pane_id.to_string()));
}

/// Hooked from `pane_worktree.rs` when a kept (closed) worktree merges from the Worktree List sheet.
pub fn on_kept_worktree_merged(host: &Arc<Host>, workspace_id: &str, pane: &PaneWorktree) {
    let Some(linked) = pane.linked_task.as_ref() else { return };
    crate::trackers::spawn_write_back(host, workspace_id, linked, crate::trackers::WorkEvent::Merged, None);
}

fn workspace_snapshot(host: &Host, requested_id: Option<&str>) -> Result<Workspace, String> {
    let view = host.view();
    let target = requested_id
        .filter(|id| !id.is_empty())
        .or(view.snapshot.active_workspace_id.as_deref())
        .and_then(|id| view.snapshot.workspace(id))
        .cloned();
    target.ok_or_else(|| host.locale.get("window.error.addWorkspaceFirst"))
}

async fn build_status(host: &Arc<Host>, ws: &Workspace) -> Value {
    let local_path = (ws.remote.is_none() && !ws.path().is_empty()).then(|| PathBuf::from(ws.path()));
    let detected_gh = match local_path.as_deref() {
        Some(p) => detect_github_repo(host, p).await.unwrap_or_default(),
        None => String::new(),
    };
    let is_git = match local_path.as_deref() {
        Some(p) => crate::host::read_git(host.processes.as_ref(), &p.to_string_lossy()).await.is_some(),
        None => false,
    };
    let trackers = tracker_statuses(host, ws).await;

    json!({
        "trackers": trackers,
        "workspaceId": ws.id(),
        "workspaceName": ws.name(),
        "isGitRepo": is_git,
        "githubRepo": ws.github_repo.clone().unwrap_or_else(|| detected_gh.clone()),
        "detectedGithubRepo": detected_gh,
    })
}

/// MC-31: each external tracker's settings for the page, with whether a token is kept (never the token).
async fn tracker_statuses(host: &Arc<Host>, ws: &Workspace) -> Vec<Value> {
    let mut out = Vec::new();
    for tracker in crate::trackers::EXTERNAL_TRACKERS {
        let config = ws
            .issue_tracker(tracker)
            .cloned()
            .unwrap_or_else(|| IssueTrackerConfig { tracker: tracker.to_string(), ..IssueTrackerConfig::default() });
        let has_token = crate::trackers::load_token(host, &config).await.is_some();
        let extra = if tracker == "gitlab" {
            let detected = crate::trackers::gitlab::detect(host, ws).await;
            let glab = ws.remote.is_none() && crate::trackers::gitlab::glab_signed_in(host, ws, &config).await;
            json!({
                "glab": glab,
                "connected": has_token || glab,
                "detectedProject": detected.as_ref().map(|(_, p)| p.clone()).unwrap_or_default(),
                "detectedSite": detected.map(|(h, _)| format!("https://{h}")).unwrap_or_default(),
                "defaultFilter": crate::trackers::default_filter(&config),
            })
        } else {
            json!({ "defaultFilter": crate::trackers::default_filter(&config) })
        };
        out.push(crate::trackers::config_json(&config, has_token, extra));
    }
    out
}

async fn fetch_detail(host: &Arc<Host>, ws: &Workspace, source: &str, key: &str) -> Result<(Value, LinkedTask, String), String> {
    let Some(tracker_id) = crate::trackers::tracker_of_source(source) else {
        return Err("Unknown integration item source.".into());
    };
    let tracker = crate::trackers::tracker_for(host, ws, tracker_id).await?;
    let project = match &tracker {
        crate::trackers::AnyTracker::GitHub(t) => Some(t.repo().to_string()),
        _ => linked_project(host, ws, tracker_id).await,
    };
    let detail = tracker.get(key).await?;
    let s = &detail.summary;
    let formatted = format_item_context(source, &s.key, &s.title, s.state.as_deref(), s.url.as_deref(), &detail.body, &detail.checklist);
    let linked = LinkedTask { source: tracker.id().into(), project, key: s.key.clone(), title: Some(s.title.clone()), url: s.url.clone() };
    Ok((crate::trackers::detail_json(source, &detail, &formatted), linked, formatted))
}

/// The team, project key or project path an external tracker's linked issue records.
async fn linked_project(host: &Host, ws: &Workspace, tracker: &str) -> Option<String> {
    let configured = ws.issue_tracker(tracker).and_then(|c| c.project.clone()).filter(|p| !p.trim().is_empty());
    match (configured, tracker) {
        (Some(p), _) => Some(p),
        (None, "gitlab") => crate::trackers::gitlab::detect(host, ws).await.map(|(_, p)| p),
        _ => None,
    }
}

pub fn register(methods: &mut Registry) {
    methods.add("integrationsStatus", |host: Arc<Host>, args| async move {
        let ws = workspace_snapshot(&host, text(&args, "workspaceId"))?;
        Ok(build_status(&host, &ws).await)
    });

    methods.add("integrationsSaveSettings", |host: Arc<Host>, args| async move {
        let ws_id = workspace_snapshot(&host, text(&args, "workspaceId"))?.id().to_string();
        let gh_repo = text(&args, "githubRepo").map(|s| s.trim().to_string()).filter(|s| !s.is_empty());

        host.view().update(|snapshot| {
            if let Some(ws) = snapshot.workspaces_mut().iter_mut().find(|w| w.id() == ws_id) {
                ws.github_repo = gh_repo;
            }
        })?;
        host.schedule();
        let ws = workspace_snapshot(&host, Some(&ws_id))?;
        Ok(build_status(&host, &ws).await)
    });

    methods.add("integrationsList", |host: Arc<Host>, args| async move {
        let ws = workspace_snapshot(&host, text(&args, "workspaceId"))?;
        let tab = text(&args, "tab").unwrap_or("github");
        let query = text(&args, "query").unwrap_or("").trim().to_lowercase();
        let filter = text(&args, "filter").unwrap_or("all");

        match tab {
            "github" | "linear" | "jira" | "gitlab" => {
                let tracker_id = tab;
                let source = crate::trackers::source_of_tracker(tracker_id).unwrap_or_default();
                if crate::trackers::EXTERNAL_TRACKERS.contains(&tracker_id) && !ws.issue_tracker(tracker_id).is_some_and(|c| c.enabled) {
                    return Ok(json!({ "items": [], "unconfigured": tracker_id }));
                }
                let tracker = match crate::trackers::tracker_for(&host, &ws, tracker_id).await {
                    Ok(t) => t,
                    Err(crate::trackers::TrackerError::NotConfigured(_)) => return Ok(json!({ "items": [], "unconfigured": tracker_id })),
                    // GitHub without gh read as unconfigured before the tracker layer, and still does.
                    Err(_) if tracker_id == "github" => return Ok(json!({ "items": [], "unconfigured": "github" })),
                    Err(err) => return Ok(json!({ "items": [], "error": err.to_string() })),
                };
                let issue_query = crate::trackers::IssueQuery { filter: filter.to_string(), text: query.clone() };
                match tracker.list(&issue_query).await {
                    Ok(rows) => {
                        let items: Vec<Value> = rows.iter().map(|s| crate::trackers::summary_json(source, s)).collect();
                        Ok(json!({ "items": items }))
                    }
                    Err(err) => Ok(json!({ "items": [], "error": err.to_string() })),
                }
            }
            _ => Err("Unknown tab.".into()),
        }
    });

    methods.add("integrationsTrackerSave", |host: Arc<Host>, args| async move {
        let ws = workspace_snapshot(&host, text(&args, "workspaceId"))?;
        let tracker = text(&args, "tracker").unwrap_or("");
        if !crate::trackers::EXTERNAL_TRACKERS.contains(&tracker) {
            return Err("Unknown issue tracker.".into());
        }
        let config = crate::trackers::config_from_args(tracker, ws.issue_tracker(tracker), &args)?;
        if let Some(token) = text(&args, "token").map(str::trim).filter(|t| !t.is_empty()) {
            crate::trackers::save_token(&host, &config, token).await?;
        }
        let ws_id = ws.id().to_string();
        host.view().update(|snapshot| {
            if let Some(ws) = snapshot.workspaces_mut().iter_mut().find(|w| w.id() == ws_id) {
                let list = ws.issue_trackers.get_or_insert_with(Vec::new);
                match list.iter_mut().find(|c| c.tracker == config.tracker) {
                    Some(existing) => *existing = config.clone(),
                    None => list.push(config.clone()),
                }
            }
        })?;
        host.schedule();
        let ws = workspace_snapshot(&host, Some(&ws_id))?;
        Ok(build_status(&host, &ws).await)
    });

    methods.add("integrationsTrackerDisconnect", |host: Arc<Host>, args| async move {
        let ws = workspace_snapshot(&host, text(&args, "workspaceId"))?;
        let tracker = text(&args, "tracker").unwrap_or("");
        if !crate::trackers::EXTERNAL_TRACKERS.contains(&tracker) {
            return Err("Unknown issue tracker.".into());
        }
        let config = ws
            .issue_tracker(tracker)
            .cloned()
            .unwrap_or_else(|| IssueTrackerConfig { tracker: tracker.to_string(), ..IssueTrackerConfig::default() });
        crate::trackers::delete_token(&host, &config).await?;
        Ok(build_status(&host, &ws).await)
    });

    methods.add("integrationsTrackerTest", |host: Arc<Host>, args| async move {
        let ws = workspace_snapshot(&host, text(&args, "workspaceId"))?;
        let tracker = text(&args, "tracker").unwrap_or("");
        if !crate::trackers::EXTERNAL_TRACKERS.contains(&tracker) {
            return Err("Unknown issue tracker.".into());
        }
        let checked = match crate::trackers::tracker_for(&host, &ws, tracker).await {
            Ok(crate::trackers::AnyTracker::Linear(t)) => t.viewer().await.map(|user| (user, "token")),
            Ok(crate::trackers::AnyTracker::Jira(t)) => t.myself().await.map(|user| (user, "token")),
            Ok(crate::trackers::AnyTracker::GitLab(t)) => {
                let via = if t.uses_glab() { "glab" } else { "token" };
                t.user().await.map(|user| (user, via))
            }
            Ok(_) => Err(crate::trackers::TrackerError::NotConfigured("issue tracker")),
            Err(error) => Err(error),
        };
        Ok(match checked {
            Ok((user, via)) => json!({ "ok": true, "user": user, "via": via }),
            Err(error) => json!({ "ok": false, "error": error.to_string() }),
        })
    });

    methods.add("integrationsDetail", |host: Arc<Host>, args| async move {
        let ws = workspace_snapshot(&host, text(&args, "workspaceId"))?;
        let source = text(&args, "source").unwrap_or("");
        let key = text(&args, "key").unwrap_or("");
        let (detail, _, _) = fetch_detail(&host, &ws, source, key).await?;
        Ok(json!({ "item": detail }))
    });

    methods.add("integrationsStart", |host: Arc<Host>, args| async move {
        let ws = workspace_snapshot(&host, text(&args, "workspaceId"))?;
        let mode = text(&args, "mode").unwrap_or("worktree");
        let auto_send = flag(&args, "autoSend");
        let source = text(&args, "source").unwrap_or("");
        let key = text(&args, "key").unwrap_or("");

        let (detail, linked, formatted_context) = fetch_detail(&host, &ws, source, key).await?;
        let item_key = detail.get("key").and_then(Value::as_str).unwrap_or(key);
        let item_title = detail.get("title").and_then(Value::as_str).unwrap_or(item_key);
        let pane_title: String = format!("{item_key} · {item_title}").chars().take(120).collect();
        let prompt = format!("Please implement the following task and verify all requirements:\n\n{formatted_context}");

        let provider = {
            let view = host.view();
            view.snapshot
                .active_session_id
                .as_deref()
                .and_then(|id| view.snapshot.session(id))
                .filter(|s| s.kind() == "claude")
                .map(|s| s.provider().to_string())
                .unwrap_or_else(|| "claude".to_string())
        };

        let external = crate::trackers::EXTERNAL_TRACKERS.contains(&linked.source.as_str());
        // MC-31: an external tracker's issue gets its own branch, `<prefix>/<KEY>-<slug>`; GitHub keeps `mighty/<id>`.
        let issue_session = if mode == "worktree" && external {
            crate::pane_worktree::free_issue_session_id(&host, &ws, &linked.source, item_key, item_title).await
        } else {
            None
        };
        let on_issue_branch = issue_session.is_some();
        let session_id = issue_session.unwrap_or_else(|| make_task_session_id(item_key));
        let persist_linked = matches!(linked.source.as_str(), "github" | "linear" | "jira" | "gitlab").then(|| linked.clone());
        let initial_draft = if auto_send { String::new() } else { prompt.clone() };

        let created = if mode == "worktree" {
            crate::pane_worktree::open_task_in_worktree(
                &host,
                &ws,
                &provider,
                session_id.clone(),
                pane_title,
                initial_draft,
                persist_linked.clone(),
                on_issue_branch,
            )
            .await?
        } else {
            let shaped_id = session_id.clone();
            let shaped_linked = persist_linked.clone();
            crate::frame::add_pane_to(&host, Some(ws.id()), "claude", &provider, None, move |pane| RunSession {
                id: Some(shaped_id),
                title: Some(pane_title),
                title_mode: Some("fixed".into()),
                draft: Some(initial_draft),
                linked_task: shaped_linked,
                ..pane
            })?
        };

        if !created {
            return Ok(json!({ "paneId": session_id, "started": false }));
        }

        {
            let mut view = host.view();
            let _ = crate::panes::ensure(&mut view, Some(&session_id));
            view.sync();
        }
        host.schedule();

        if let Some(ref task_link) = persist_linked
            && external
        {
            crate::trackers::spawn_write_back(&host, ws.id(), task_link, crate::trackers::WorkEvent::Started, Some(session_id.clone()));
        }

        let mut started_run = false;
        if auto_send {
            started_run = crate::composer::start_run(&host, &session_id, &prompt, vec![], None, false, None).await.is_ok();
        }

        Ok(json!({ "paneId": session_id, "started": started_run }))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_safe_worktree_session_ids() {
        let id = make_task_session_id("MC-11");
        assert!(id.starts_with("mc-11-") && mighty_core::wire::identifier(&id));
        let gh_id = make_task_session_id("#42");
        assert!(gh_id.starts_with("42-") && mighty_core::wire::identifier(&gh_id));
    }

    #[test]
    fn formats_context_with_checklist() {
        let ctx = format_item_context(
            "github_issue",
            "#11",
            "Issue picker",
            Some("open"),
            Some("https://example.test/issues/11"),
            "Implement picker.",
            &[("Step 1".into(), true), ("Step 2".into(), false)],
        );
        assert!(ctx.contains("[GitHub #11] Issue picker"));
        assert!(ctx.contains("- [x] Step 1") && ctx.contains("- [ ] Step 2"));
    }
}
