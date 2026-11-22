//! MC-27: 머지 전 점검 — everything that matters before a worktree is merged or its pull request is, on one screen (the Git workbench's
//! 점검 tab, `src/git/ChecksTab.tsx`), and the review threads of the pull request handed to the agent.
//!
//! - `gitWorkbenchChecks{workspaceId, paneId}` reads, for a worktree pane (a delegated child's worktree too) or the workspace's
//!   current branch: the uncommitted changes and how far it is from its base (`git status --porcelain=v2 --branch`, `rev-list`);
//!   the pull request (`gh pr view --json number,url,state,title,isDraft,mergeable,reviewDecision,statusCheckRollup`, or the
//!   GitLab merge request, `glab mr view --output json`, and its pipeline's jobs); each CI check; the unresolved review threads
//!   (`gh api graphql` `reviewThreads`, `glab api …/discussions`); and the remaining items of the pane's last request's checklist.
//!   Each part fails on its own ("확인할 수 없음": no remote, gh or glab missing, not logged in, no pull request).
//! - The page asks while the tab is open (when it opens and every 30 s); nothing here runs on a timer.
//! - The warnings ([`warnings`]) are asked once more before a worktree pane's or a child's merge ([`merge_warning_text`]: the
//!   worktree bar, the child card and the review queue go through it); they never block. The review queue's cards carry them too
//!   ([`queue_warnings`]), from what was read last and what the app already knows, never a new `gh` call.
//! - Every `gh`/`glab` call goes through the workbench's `run_hosting_cli`, git through its hardened runner. Comment bodies are
//!   redacted (`redact_secret_text`) and links lose their credentials (`strip_url_credentials`) before they leave the host.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use mighty_core::model::PaneWorktree;
use serde::Serialize;
use serde_json::{Value, json};

use crate::git_workbench::{
    detect_hosting, find_hosting_cli, parse_github_pr, parse_gitlab_mr, parse_porcelain_v2_status, read_origin_url, redact_secret_text,
    run_git_capture, run_hosting_cli, strip_url_credentials, validate_branch_name,
};
use crate::host::{Host, Registry, lock};
use crate::methods::text;

/// How long a reading stands for a merge's warning before it is read again.
const MERGE_READING_FOR: Duration = Duration::from_secs(60);
/// How long a reading stands for the review queue's cards.
const QUEUE_READING_FOR: Duration = Duration::from_secs(600);
/// The longest comment body kept (the page shows its start; the follow-up is capped at 2,000 characters).
const MAX_BODY_CHARS: usize = 4_000;
const HOSTING_TIMEOUT: u64 = 20;

// ── the model ────────────────────────────────────────────────────────────────────────────────────────────────────────────

/// Why a part could not be read: `noRemote`, `cliMissing` (with `cli`), `notLoggedIn`, `noPr`, `failed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Unknown {
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cli: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl Unknown {
    fn new(reason: &str) -> Self {
        Unknown { reason: reason.into(), cli: None, detail: None }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitFacts {
    pub head: String,
    pub base: Option<String>,
    pub uncommitted: usize,
    pub conflicted: usize,
    /// Commits on HEAD the base lacks, and on the base HEAD lacks (against the upstream when there is no base).
    pub ahead: i64,
    pub behind: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrFacts {
    pub hosting: String,
    pub number: i64,
    pub url: String,
    /// `open`, `merged` or `closed`.
    pub state: String,
    pub title: String,
    pub is_draft: bool,
    /// `mergeable`, `conflicting` or `unknown`.
    pub mergeable: String,
    /// `approved`, `changesRequested`, `reviewRequired` or empty.
    pub review_decision: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckItem {
    pub name: String,
    /// `passed`, `failed`, `pending` or `skipped`.
    pub state: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewThread {
    pub id: String,
    pub path: String,
    pub line: Option<i64>,
    /// The code it was left on has changed since.
    pub outdated: bool,
    pub author: String,
    /// The first comment, redacted.
    pub body: String,
    pub url: String,
    pub replies: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChecklistFacts {
    pub total: usize,
    pub remaining: Vec<String>,
}

/// One reading.
#[derive(Debug, Clone, PartialEq)]
pub struct Model {
    pub git: Result<GitFacts, Unknown>,
    pub pr: Result<PrFacts, Unknown>,
    pub checks: Result<Vec<CheckItem>, Unknown>,
    pub threads: Result<Vec<ReviewThread>, Unknown>,
    pub checklist: Result<ChecklistFacts, Unknown>,
}

/// One reason to look again before merging, and how many.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Warning {
    pub code: &'static str,
    pub count: usize,
}

/// The warnings a reading holds: failing or pending checks, unresolved threads, a draft, conflicts and requested changes of an
/// open pull request; remaining checklist items; uncommitted changes (not for 커밋하고 병합, which commits them first). A part that
/// could not be read warns of nothing.
pub fn warnings(model: &Model, commit_first: bool) -> Vec<Warning> {
    let mut out = vec![];
    let mut push = |code: &'static str, count: usize| {
        if count > 0 {
            out.push(Warning { code, count });
        }
    };
    if let Ok(git) = &model.git
        && !commit_first
    {
        push("uncommitted", git.uncommitted);
    }
    let open = model.pr.as_ref().is_ok_and(|pr| pr.state == "open");
    if let Ok(pr) = &model.pr
        && open
    {
        push("draft", usize::from(pr.is_draft));
        push("conflicts", usize::from(pr.mergeable == "conflicting"));
        push("changesRequested", usize::from(pr.review_decision == "changesRequested"));
    }
    if let Ok(checks) = &model.checks
        && open
    {
        push("checksFailed", checks.iter().filter(|c| c.state == "failed").count());
        push("checksPending", checks.iter().filter(|c| c.state == "pending").count());
    }
    if let Ok(threads) = &model.threads
        && open
    {
        push("threads", threads.len());
    }
    if let Ok(checklist) = &model.checklist {
        push("checklist", checklist.remaining.len());
    }
    out
}

fn section<T: Serialize>(part: &Result<T, Unknown>) -> Value {
    match part {
        Ok(value) => json!({ "ok": true, "value": value }),
        Err(unknown) => {
            let mut out = serde_json::to_value(unknown).unwrap_or_else(|_| json!({}));
            out["ok"] = Value::Bool(false);
            out
        }
    }
}

/// The page's model: each part as `{ok: true, value}` or `{ok: false, reason, cli?, detail?}`, and the warnings.
pub fn to_json(model: &Model) -> Value {
    json!({
        "git": section(&model.git),
        "pr": section(&model.pr),
        "checks": section(&model.checks),
        "threads": section(&model.threads),
        "checklist": section(&model.checklist),
        "warnings": warnings(model, false),
    })
}

/// The reasons as the merge's confirmation lists them, one a line.
pub fn warning_lines(locale: &mighty_core::locale::Locale, warnings: &[Warning]) -> Vec<String> {
    warnings
        .iter()
        .map(|w| {
            let count = w.count.to_string();
            let line = match w.code {
                "uncommitted" => locale.get_with("git.checks.warn.uncommitted", &[("count", &count)]),
                "draft" => locale.get("git.checks.warn.draft"),
                "conflicts" => locale.get("git.checks.warn.conflicts"),
                "changesRequested" => locale.get("git.checks.warn.changesRequested"),
                "checksFailed" => locale.get_with("git.checks.warn.checksFailed", &[("count", &count)]),
                "checksPending" => locale.get_with("git.checks.warn.checksPending", &[("count", &count)]),
                "threads" => locale.get_with("git.checks.warn.threads", &[("count", &count)]),
                _ => locale.get_with("git.checks.warn.checklist", &[("count", &count)]),
            };
            format!("• {line}")
        })
        .collect()
}

// ── parsing ──────────────────────────────────────────────────────────────────────────────────────────────────────────────

/// A name, title or checklist item on one line: control characters (a newline included) become spaces.
fn plain(text: &str) -> String {
    text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ")
}

fn str_of<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or("")
}

/// A link the page may open: http(s) only, credentials taken out.
fn clean_url(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.starts_with("https://") || trimmed.starts_with("http://") { strip_url_credentials(trimmed) } else { String::new() }
}

/// A comment body as it may leave the host: secret lines redacted, link credentials stripped, at most [`MAX_BODY_CHARS`].
pub fn clean_body(raw: &str) -> String {
    let cleaned = strip_url_credentials(&redact_secret_text(&raw.replace("\r\n", "\n")));
    let trimmed = cleaned.trim();
    if trimmed.chars().count() > MAX_BODY_CHARS {
        let mut cut: String = trimmed.chars().take(MAX_BODY_CHARS - 1).collect();
        cut.push('…');
        cut
    } else {
        trimmed.to_string()
    }
}

/// A `statusCheckRollup` entry (a check run or a commit status) as a check.
fn github_check(item: &Value) -> CheckItem {
    let upper = |key: &str| str_of(item, key).to_ascii_uppercase();
    let (conclusion, status, state) = (upper("conclusion"), upper("status"), upper("state"));
    let result = if item.get("context").is_some() && item.get("name").is_none() {
        match state.as_str() {
            "SUCCESS" => "passed",
            "FAILURE" | "ERROR" => "failed",
            _ => "pending",
        }
    } else if !status.is_empty() && status != "COMPLETED" {
        "pending"
    } else {
        match conclusion.as_str() {
            "SUCCESS" => "passed",
            "FAILURE" | "TIMED_OUT" | "CANCELLED" | "ACTION_REQUIRED" | "STARTUP_FAILURE" => "failed",
            "NEUTRAL" | "SKIPPED" | "STALE" => "skipped",
            "" => "pending",
            _ => "skipped",
        }
    };
    let name = [str_of(item, "workflowName"), str_of(item, "name"), str_of(item, "context")]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" / ");
    let url = clean_url(if item.get("detailsUrl").is_some() { str_of(item, "detailsUrl") } else { str_of(item, "targetUrl") });
    CheckItem { name: plain(&name), state: result.into(), url }
}

/// `gh pr view --json number,url,state,title,isDraft,mergeable,reviewDecision,statusCheckRollup`: the pull request and its checks.
pub fn parse_github_pr_detail(bytes: &[u8]) -> Option<(PrFacts, Vec<CheckItem>)> {
    let base = parse_github_pr(bytes)?;
    let root: Value = serde_json::from_slice(bytes).ok()?;
    let mergeable = match str_of(&root, "mergeable").to_ascii_uppercase().as_str() {
        "MERGEABLE" => "mergeable",
        "CONFLICTING" => "conflicting",
        _ => "unknown",
    };
    let review_decision = match str_of(&root, "reviewDecision").to_ascii_uppercase().as_str() {
        "APPROVED" => "approved",
        "CHANGES_REQUESTED" => "changesRequested",
        "REVIEW_REQUIRED" => "reviewRequired",
        _ => "",
    };
    let checks =
        root.get("statusCheckRollup").and_then(Value::as_array).map(|items| items.iter().map(github_check).collect()).unwrap_or_default();
    let pr = PrFacts {
        hosting: "github".into(),
        number: base.number,
        url: clean_url(&base.url),
        state: base.state,
        title: plain(&base.title),
        is_draft: base.is_draft,
        mergeable: mergeable.into(),
        review_decision: review_decision.into(),
    };
    Some((pr, checks))
}

/// `glab mr view --output json`: the merge request, its project and its pipeline.
pub fn parse_gitlab_mr_detail(bytes: &[u8]) -> Option<(PrFacts, Option<i64>, Option<i64>)> {
    let base = parse_gitlab_mr(bytes)?;
    let root: Value = serde_json::from_slice(bytes).ok()?;
    let detailed = str_of(&root, "detailed_merge_status").to_ascii_lowercase();
    let conflicting = root.get("has_conflicts").and_then(Value::as_bool).unwrap_or(false) || detailed == "conflict";
    let mergeable = if conflicting {
        "conflicting"
    } else if matches!(detailed.as_str(), "mergeable" | "can_be_merged") || str_of(&root, "merge_status") == "can_be_merged" {
        "mergeable"
    } else {
        "unknown"
    };
    let review_decision = match detailed.as_str() {
        "requested_changes" => "changesRequested",
        "not_approved" => "reviewRequired",
        _ => "",
    };
    let project = root.get("project_id").and_then(Value::as_i64);
    let pipeline = root
        .get("head_pipeline")
        .filter(|p| p.is_object())
        .or_else(|| root.get("pipeline").filter(|p| p.is_object()))
        .and_then(|p| p.get("id"))
        .and_then(Value::as_i64);
    let pr = PrFacts {
        hosting: "gitlab".into(),
        number: base.number,
        url: clean_url(&base.url),
        state: base.state,
        title: plain(&base.title),
        is_draft: base.is_draft,
        mergeable: mergeable.into(),
        review_decision: review_decision.into(),
    };
    Some((pr, project, pipeline))
}

/// `glab api projects/:id/pipelines/:pid/jobs`: each job as a check (a failure allowed to fail is skipped).
pub fn parse_gitlab_jobs(bytes: &[u8]) -> Option<Vec<CheckItem>> {
    let jobs: Vec<Value> = serde_json::from_slice(bytes).ok()?;
    Some(
        jobs.iter()
            .map(|job| {
                let allowed = job.get("allow_failure").and_then(Value::as_bool).unwrap_or(false);
                let state = match str_of(job, "status").to_ascii_lowercase().as_str() {
                    "success" => "passed",
                    "failed" if allowed => "skipped",
                    "failed" | "canceled" => "failed",
                    "skipped" | "manual" => "skipped",
                    _ => "pending",
                };
                let name =
                    [str_of(job, "stage"), str_of(job, "name")].into_iter().filter(|p| !p.is_empty()).collect::<Vec<_>>().join(" / ");
                CheckItem { name: plain(&name), state: state.into(), url: clean_url(str_of(job, "web_url")) }
            })
            .collect(),
    )
}

/// The GraphQL query of a pull request's review threads (`gh api graphql -F owner={owner} -F name={repo} -F number=N`).
pub const REVIEW_THREADS_QUERY: &str = "query($owner:String!,$name:String!,$number:Int!){repository(owner:$owner,name:$name){pullRequest(number:$number){reviewThreads(first:100){nodes{id isResolved isOutdated path line originalLine comments(first:20){totalCount nodes{author{login} body url}}}}}}}";

/// GitHub's answer to [`REVIEW_THREADS_QUERY`]: the unresolved threads. None when it is not an answer (errors, no pull request).
pub fn parse_github_review_threads(bytes: &[u8]) -> Option<Vec<ReviewThread>> {
    let root: Value = serde_json::from_slice(bytes).ok()?;
    let nodes = root.pointer("/data/repository/pullRequest/reviewThreads/nodes")?.as_array()?;
    Some(
        nodes
            .iter()
            .filter(|node| !node.get("isResolved").and_then(Value::as_bool).unwrap_or(false))
            .map(|node| {
                let comments = node.pointer("/comments/nodes").and_then(Value::as_array).cloned().unwrap_or_default();
                let first = comments.first().cloned().unwrap_or(Value::Null);
                let total = node.pointer("/comments/totalCount").and_then(Value::as_u64).map_or(comments.len(), |n| n as usize);
                let author = first.pointer("/author/login").and_then(Value::as_str).unwrap_or("ghost");
                ReviewThread {
                    id: str_of(node, "id").to_string(),
                    path: str_of(node, "path").to_string(),
                    line: node.get("line").and_then(Value::as_i64).or_else(|| node.get("originalLine").and_then(Value::as_i64)),
                    outdated: node.get("isOutdated").and_then(Value::as_bool).unwrap_or(false),
                    author: plain(author),
                    body: clean_body(str_of(&first, "body")),
                    url: clean_url(str_of(&first, "url")),
                    replies: total.saturating_sub(1),
                }
            })
            .collect(),
    )
}

/// `glab api projects/:id/merge_requests/:iid/discussions`: the discussions with a resolvable note not resolved yet, placed by
/// their first note's position; their links are the merge request's (`mr_url`) `#note_<id>`.
pub fn parse_gitlab_discussions(bytes: &[u8], mr_url: &str) -> Option<Vec<ReviewThread>> {
    let discussions: Vec<Value> = serde_json::from_slice(bytes).ok()?;
    let mr_url = clean_url(mr_url);
    Some(
        discussions
            .iter()
            .filter_map(|discussion| {
                let notes: Vec<&Value> = discussion
                    .get("notes")
                    .and_then(Value::as_array)?
                    .iter()
                    .filter(|note| !note.get("system").and_then(Value::as_bool).unwrap_or(false))
                    .collect();
                let first = *notes.first()?;
                let resolvable: Vec<&&Value> =
                    notes.iter().filter(|note| note.get("resolvable").and_then(Value::as_bool).unwrap_or(false)).collect();
                if resolvable.is_empty() || resolvable.iter().all(|note| note.get("resolved").and_then(Value::as_bool).unwrap_or(false)) {
                    return None;
                }
                let position = first.get("position").filter(|p| p.is_object());
                let (path, line) = match position {
                    Some(p) => {
                        let new_path = str_of(p, "new_path");
                        let path = if new_path.is_empty() { str_of(p, "old_path") } else { new_path };
                        (path.to_string(), p.get("new_line").and_then(Value::as_i64).or_else(|| p.get("old_line").and_then(Value::as_i64)))
                    }
                    None => (String::new(), None),
                };
                let note_id = first.get("id").and_then(Value::as_i64);
                Some(ReviewThread {
                    id: str_of(discussion, "id").to_string(),
                    path,
                    line,
                    outdated: false,
                    author: plain(first.pointer("/author/username").and_then(Value::as_str).unwrap_or("")),
                    body: clean_body(str_of(first, "body")),
                    url: match note_id {
                        Some(id) if !mr_url.is_empty() => format!("{mr_url}#note_{id}"),
                        _ => mr_url.clone(),
                    },
                    replies: notes.len().saturating_sub(1),
                })
            })
            .collect(),
    )
}

/// What a failed `gh`/`glab` call means: no pull request for the branch, not logged in, or another failure.
pub fn classify_failure(stderr: &str) -> &'static str {
    let lower = stderr.to_ascii_lowercase();
    if lower.contains("no pull requests found")
        || lower.contains("no open merge request")
        || lower.contains("no merge request")
        || lower.contains("could not find any pull request")
    {
        "noPr"
    } else if lower.contains("auth login")
        || lower.contains("not logged in")
        || lower.contains("authentication")
        || lower.contains("401 unauthorized")
        || lower.contains("glab auth")
    {
        "notLoggedIn"
    } else {
        "failed"
    }
}

fn failure(captured: &mighty_platform::process::Captured, cli: &str) -> Unknown {
    let stderr = String::from_utf8_lossy(&captured.stderr);
    let reason = classify_failure(&stderr);
    let detail = strip_url_credentials(stderr.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or(""));
    let detail: String = redact_secret_text(&detail).chars().take(200).collect();
    Unknown { reason: reason.into(), cli: Some(cli.into()), detail: (!detail.is_empty()).then_some(detail) }
}

// ── the checklist ────────────────────────────────────────────────────────────────────────────────────────────────────────

/// The steps of a request (its checklist) as `(text, done)`.
pub fn checklist_of(steps: &[(String, String)]) -> ChecklistFacts {
    let remaining = steps.iter().filter(|(_, status)| status != "completed").map(|(text, _)| text.clone()).collect();
    ChecklistFacts { total: steps.len(), remaining }
}

/// The checklist of the pane's last request (its steps, else the pane's live checklist); none without a pane.
fn pane_checklist(host: &Host, pane_id: Option<&str>) -> Result<ChecklistFacts, Unknown> {
    let Some(pane_id) = pane_id else { return Ok(ChecklistFacts { total: 0, remaining: vec![] }) };
    let view = host.view();
    let Some(session) = view.snapshot.session(pane_id) else { return Ok(ChecklistFacts { total: 0, remaining: vec![] }) };
    let last = session.graph_runs().iter().rev().find(|run| run.notice_run != Some(true));
    let from_run: Vec<(String, String)> = last
        .and_then(|run| run.steps.as_ref())
        .map(|steps| {
            steps
                .iter()
                .map(|step| (step.content.clone().unwrap_or_default(), step.status.clone().unwrap_or_else(|| "pending".into())))
                .filter(|(text, _)| !text.trim().is_empty())
                .collect()
        })
        .unwrap_or_default();
    let steps = if from_run.is_empty() {
        session
            .todo_progress
            .as_ref()
            .and_then(|progress| progress.items.as_ref())
            .map(|items| {
                items
                    .iter()
                    .map(|item| (item.content.clone().unwrap_or_default(), item.status.clone().unwrap_or_else(|| "pending".into())))
                    .filter(|(text, _)| !text.trim().is_empty())
                    .collect()
            })
            .unwrap_or_default()
    } else {
        from_run
    };
    let steps: Vec<(String, String)> = steps.into_iter().map(|(text, status)| (plain(&text), status)).collect();
    Ok(checklist_of(&steps))
}

// ── reading ──────────────────────────────────────────────────────────────────────────────────────────────────────────────

/// Where a reading works: the folder, the base it is measured against, the pane whose checklist it shows.
#[derive(Debug, Clone)]
pub struct Target {
    pub folder: PathBuf,
    pub base: Option<String>,
    pub pane_id: Option<String>,
}

async fn git_part(host: &Host, folder: &Path, base: Option<&str>) -> Result<GitFacts, Unknown> {
    let failed = |detail: Option<String>| Unknown { reason: "failed".into(), cli: None, detail };
    let status = run_git_capture(host, folder, &["status", "--porcelain=v2", "--branch", "-z", "--untracked-files=all"], None, 15, true)
        .await
        .map_err(|e| failed(Some(e)))?;
    if status.exit_code != 0 {
        return Err(failed(Some(crate::git_workbench::format_git_failure(&status))));
    }
    let parsed = parse_porcelain_v2_status(&status.stdout);
    let mut facts = GitFacts {
        head: parsed.head.clone(),
        base: None,
        uncommitted: parsed.files.len(),
        conflicted: parsed.files.iter().filter(|f| f.conflicted).count(),
        ahead: i64::from(parsed.ahead),
        behind: i64::from(parsed.behind),
    };
    let base = match base {
        Some(base) => Some(base.to_string()),
        None => crate::git_workbench::default_branch(host, folder).await,
    };
    if let Some(base) = base.and_then(|b| validate_branch_name(&b).ok()).filter(|b| *b != parsed.head) {
        let range = format!("{base}...HEAD");
        if let Ok(counted) = run_git_capture(host, folder, &["rev-list", "--left-right", "--count", &range, "--"], None, 10, true).await
            && counted.exit_code == 0
        {
            let words = String::from_utf8_lossy(&counted.stdout);
            let mut parts = words.split_whitespace().map(|n| n.parse::<i64>().unwrap_or(0));
            facts.behind = parts.next().unwrap_or(0);
            facts.ahead = parts.next().unwrap_or(0);
            facts.base = Some(base);
        }
    }
    Ok(facts)
}

type HostingParts = (Result<PrFacts, Unknown>, Result<Vec<CheckItem>, Unknown>, Result<Vec<ReviewThread>, Unknown>);

fn all_unknown(unknown: Unknown) -> HostingParts {
    (Err(unknown.clone()), Err(unknown.clone()), Err(unknown))
}

async fn github_parts(host: &Host, folder: &Path) -> HostingParts {
    let args: Vec<String> =
        ["pr", "view", "--json", "number,url,state,title,isDraft,mergeable,reviewDecision,statusCheckRollup"].map(String::from).into();
    let captured = match run_hosting_cli(host, "gh", folder, &args, HOSTING_TIMEOUT).await {
        Ok(captured) => captured,
        Err(error) => return all_unknown(Unknown { detail: Some(error), ..Unknown::new("failed") }),
    };
    if captured.exit_code != 0 {
        return all_unknown(failure(&captured, "gh"));
    }
    let Some((pr, checks)) = parse_github_pr_detail(&captured.stdout) else { return all_unknown(Unknown::new("failed")) };
    let query = vec![
        "api".to_string(),
        "graphql".into(),
        "-F".into(),
        "owner={owner}".into(),
        "-F".into(),
        "name={repo}".into(),
        "-F".into(),
        format!("number={}", pr.number),
        "-f".into(),
        format!("query={REVIEW_THREADS_QUERY}"),
    ];
    let threads = match run_hosting_cli(host, "gh", folder, &query, HOSTING_TIMEOUT).await {
        Ok(answer) if answer.exit_code == 0 => parse_github_review_threads(&answer.stdout).ok_or_else(|| Unknown::new("failed")),
        Ok(answer) => Err(failure(&answer, "gh")),
        Err(error) => Err(Unknown { detail: Some(error), ..Unknown::new("failed") }),
    };
    (Ok(pr), Ok(checks), threads)
}

async fn gitlab_parts(host: &Host, folder: &Path) -> HostingParts {
    let args: Vec<String> = ["mr", "view", "--output", "json"].map(String::from).into();
    let captured = match run_hosting_cli(host, "glab", folder, &args, HOSTING_TIMEOUT).await {
        Ok(captured) => captured,
        Err(error) => return all_unknown(Unknown { detail: Some(error), ..Unknown::new("failed") }),
    };
    if captured.exit_code != 0 {
        return all_unknown(failure(&captured, "glab"));
    }
    let Some((pr, project, pipeline)) = parse_gitlab_mr_detail(&captured.stdout) else { return all_unknown(Unknown::new("failed")) };
    let Some(project) = project else { return (Ok(pr), Err(Unknown::new("failed")), Err(Unknown::new("failed"))) };
    let checks = match pipeline {
        None => Ok(vec![]),
        Some(pipeline) => {
            let endpoint = vec!["api".to_string(), format!("projects/{project}/pipelines/{pipeline}/jobs?per_page=100")];
            match run_hosting_cli(host, "glab", folder, &endpoint, HOSTING_TIMEOUT).await {
                Ok(answer) if answer.exit_code == 0 => parse_gitlab_jobs(&answer.stdout).ok_or_else(|| Unknown::new("failed")),
                Ok(answer) => Err(failure(&answer, "glab")),
                Err(error) => Err(Unknown { detail: Some(error), ..Unknown::new("failed") }),
            }
        }
    };
    let endpoint = vec!["api".to_string(), format!("projects/{project}/merge_requests/{}/discussions?per_page=100", pr.number)];
    let threads = match run_hosting_cli(host, "glab", folder, &endpoint, HOSTING_TIMEOUT).await {
        Ok(answer) if answer.exit_code == 0 => parse_gitlab_discussions(&answer.stdout, &pr.url).ok_or_else(|| Unknown::new("failed")),
        Ok(answer) => Err(failure(&answer, "glab")),
        Err(error) => Err(Unknown { detail: Some(error), ..Unknown::new("failed") }),
    };
    (Ok(pr), checks, threads)
}

async fn hosting_parts(host: &Host, folder: &Path) -> HostingParts {
    let origin = read_origin_url(host, folder).await;
    let hosting = detect_hosting(origin.as_deref());
    if hosting == "none" {
        return all_unknown(Unknown::new("noRemote"));
    }
    let cli = if hosting == "gitlab" { "glab" } else { "gh" };
    if find_hosting_cli(host, cli).is_none() {
        return all_unknown(Unknown { cli: Some(cli.into()), ..Unknown::new("cliMissing") });
    }
    if hosting == "gitlab" { gitlab_parts(host, folder).await } else { github_parts(host, folder).await }
}

/// The readings kept, by folder: what a merge's warning and the review queue's cards read.
static KEPT: LazyLock<Mutex<HashMap<String, (Instant, Model)>>> = LazyLock::new(Mutex::default);

fn key_of(folder: &Path) -> String {
    crate::worktree_lifecycle::path_key(&folder.to_string_lossy())
}

/// Reads everything for `target` and keeps it; the review queue is read again when its warnings changed.
pub async fn read(host: &Arc<Host>, target: &Target) -> Model {
    let (git, (pr, checks, threads)) =
        tokio::join!(git_part(host, &target.folder, target.base.as_deref()), hosting_parts(host, &target.folder));
    let checklist = pane_checklist(host, target.pane_id.as_deref());
    let model = Model { git, pr, checks, threads, checklist };
    let changed = {
        let mut kept = lock(&KEPT);
        let key = key_of(&target.folder);
        let before = kept.get(&key).map(|(_, old)| warnings(old, false));
        kept.insert(key, (Instant::now(), model.clone()));
        before.is_some_and(|before| before != warnings(&model, false))
    };
    if changed {
        crate::review::changed(host);
    }
    model
}

/// The confirmation's words when a merge of `target` should be asked once more (None when nothing warns, and always in a smoke):
/// a reading of the last minute, else a new one.
pub async fn merge_warning_text(host: &Arc<Host>, target: &Target, commit_first: bool) -> Option<String> {
    if host.smoke.is_some() {
        return None;
    }
    let kept = lock(&KEPT).get(&key_of(&target.folder)).filter(|(at, _)| at.elapsed() < MERGE_READING_FOR).map(|(_, model)| model.clone());
    let model = match kept {
        Some(model) => model,
        None => read(host, target).await,
    };
    let found = warnings(&model, commit_first);
    if found.is_empty() {
        return None;
    }
    Some(format!("{}\n\n{}", host.locale.get("git.checks.merge.message"), warning_lines(&host.locale, &found).join("\n")))
}

/// A review queue card's warnings, never a new `gh` call: the reading of the last ten minutes when there is one (its git and
/// checklist parts as they are now), else what the app knows: the uncommitted files, the checklist, and the worktree's pull
/// request as the pane chips last read it.
pub fn queue_warnings(host: &Host, pane_id: &str, record: &PaneWorktree, uncommitted: usize) -> Vec<Warning> {
    let checklist = pane_checklist(host, Some(pane_id));
    let folder = target_of(pane_id, record).folder;
    let kept = lock(&KEPT).get(&key_of(&folder)).filter(|(at, _)| at.elapsed() < QUEUE_READING_FOR).map(|(_, m)| m.clone());
    if let Some(mut model) = kept {
        if let Ok(git) = model.git.as_mut() {
            git.uncommitted = uncommitted;
        }
        model.checklist = checklist;
        return warnings(&model, false);
    }
    let chip = crate::attention::cached_pr(record.worktree_path());
    let pr = match &chip {
        Some(chip) => Ok(PrFacts {
            hosting: String::new(),
            number: chip.number,
            url: chip.url.clone(),
            state: if chip.state == "draft" { "open".into() } else { chip.state.clone() },
            title: String::new(),
            is_draft: chip.state == "draft",
            mergeable: "unknown".into(),
            review_decision: String::new(),
        }),
        None => Err(Unknown::new("failed")),
    };
    let checks = match chip.as_ref().map(|c| c.checks.as_str()) {
        Some("failed") => Ok(vec![CheckItem { name: String::new(), state: "failed".into(), url: String::new() }]),
        Some("pending") => Ok(vec![CheckItem { name: String::new(), state: "pending".into(), url: String::new() }]),
        Some(_) => Ok(vec![]),
        None => Err(Unknown::new("failed")),
    };
    let git = Ok(GitFacts { head: String::new(), base: None, uncommitted, conflicted: 0, ahead: 0, behind: 0 });
    warnings(&Model { git, pr, checks, threads: Err(Unknown::new("failed")), checklist }, false)
}

/// The reading of a worktree pane's (or a delegated child's, by its pane id) worktree: its run folder against its base.
pub fn target_of(pane_id: &str, record: &PaneWorktree) -> Target {
    let folder = if record.run_folder().is_empty() { record.worktree_path() } else { record.run_folder() };
    Target {
        folder: PathBuf::from(folder),
        base: Some(record.base_branch().to_string()).filter(|b| !b.is_empty()),
        pane_id: Some(pane_id.to_string()),
    }
}

/// The target `paneId` names (a worktree pane's run folder, a delegated child's worktree) or the workspace's folder.
fn resolve_target(host: &Host, args: &Value) -> Result<(String, Target), String> {
    let pane_id = text(args, "paneId").map(str::trim).filter(|id| !id.is_empty()).map(str::to_string);
    if let Some(pane_id) = pane_id.as_deref() {
        let found = {
            let view = host.view();
            view.snapshot.session(pane_id).map(|session| {
                let workspace = view.snapshot.workspace(session.workspace_id()).filter(|w| w.remote.is_none()).cloned();
                (workspace, session.pane_worktree.clone())
            })
        };
        if let Some((Some(workspace), record)) = found {
            let record = record.or_else(|| crate::pane_worktree::child_record(pane_id));
            let target = match record {
                Some(record) => target_of(pane_id, &record),
                None => Target { folder: PathBuf::from(workspace.path()), base: None, pane_id: Some(pane_id.to_string()) },
            };
            if !target.folder.is_dir() {
                return Err("The pane's worktree folder does not exist on disk.".into());
            }
            return Ok((workspace.id().to_string(), target));
        }
    }
    let (workspace, root) = crate::git_workbench::resolve_local_workspace_root(host, text(args, "workspaceId"))?;
    Ok((workspace.id().to_string(), Target { folder: root, base: None, pane_id }))
}

pub fn register(methods: &mut Registry) {
    methods.add("gitWorkbenchChecks", |host: Arc<Host>, args| async move {
        let (workspace_id, target) = resolve_target(&host, &args)?;
        let model = read(&host, &target).await;
        let mut out = to_json(&model);
        out["workspaceId"] = json!(workspace_id);
        out["paneId"] = json!(target.pane_id);
        Ok(out)
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model() -> Model {
        Model {
            git: Ok(GitFacts { head: "mighty/a".into(), base: Some("main".into()), uncommitted: 2, conflicted: 0, ahead: 3, behind: 0 }),
            pr: Ok(PrFacts {
                hosting: "github".into(),
                number: 42,
                url: "https://github.com/o/r/pull/42".into(),
                state: "open".into(),
                title: "Fix".into(),
                is_draft: false,
                mergeable: "mergeable".into(),
                review_decision: String::new(),
            }),
            checks: Ok(vec![
                CheckItem { name: "ci / build".into(), state: "passed".into(), url: String::new() },
                CheckItem { name: "ci / test".into(), state: "failed".into(), url: String::new() },
                CheckItem { name: "ci / lint".into(), state: "pending".into(), url: String::new() },
            ]),
            threads: Ok(vec![]),
            checklist: Ok(ChecklistFacts { total: 3, remaining: vec!["write tests".into()] }),
        }
    }

    fn codes(found: &[Warning]) -> Vec<(&'static str, usize)> {
        found.iter().map(|w| (w.code, w.count)).collect()
    }

    #[test]
    fn the_warnings_name_every_reason_to_look_again_and_never_an_unknown_part() {
        let m = model();
        assert_eq!(codes(&warnings(&m, false)), [("uncommitted", 2), ("checksFailed", 1), ("checksPending", 1), ("checklist", 1)]);
        assert_eq!(
            codes(&warnings(&m, true)),
            [("checksFailed", 1), ("checksPending", 1), ("checklist", 1)],
            "commit and merge commits them"
        );

        let mut pr = m.clone();
        if let Ok(p) = pr.pr.as_mut() {
            p.is_draft = true;
            p.mergeable = "conflicting".into();
            p.review_decision = "changesRequested".into();
        }
        pr.threads = Ok(vec![ReviewThread {
            id: "t".into(),
            path: "a.rs".into(),
            line: Some(1),
            outdated: false,
            author: "r".into(),
            body: "b".into(),
            url: String::new(),
            replies: 0,
        }]);
        let found = codes(&warnings(&pr, true));
        for code in ["draft", "conflicts", "changesRequested", "threads"] {
            assert!(found.iter().any(|(c, _)| *c == code), "{code} in {found:?}");
        }

        // A merged pull request's checks and threads no longer matter.
        let mut merged = pr.clone();
        if let Ok(p) = merged.pr.as_mut() {
            p.state = "merged".into();
        }
        assert_eq!(codes(&warnings(&merged, true)), [("checklist", 1)]);

        // Nothing could be read: nothing warns.
        let unknown = Model {
            git: Err(Unknown::new("failed")),
            pr: Err(Unknown::new("cliMissing")),
            checks: Err(Unknown::new("cliMissing")),
            threads: Err(Unknown::new("cliMissing")),
            checklist: Ok(ChecklistFacts { total: 0, remaining: vec![] }),
        };
        assert!(warnings(&unknown, false).is_empty());
        let json = to_json(&unknown);
        assert_eq!(json["pr"]["ok"], false);
        assert_eq!(json["pr"]["reason"], "cliMissing");
        assert_eq!(to_json(&m)["git"]["value"]["uncommitted"], 2);
    }

    #[test]
    fn the_merge_confirmation_lists_each_reason_on_its_line() {
        let ko = mighty_core::locale::Locale::new("ko");
        let lines = warning_lines(&ko, &warnings(&model(), false));
        assert_eq!(lines.len(), 4);
        assert!(lines.iter().all(|line| line.starts_with("• ")));
        assert_eq!(lines[0], format!("• {}", ko.get_with("git.checks.warn.uncommitted", &[("count", "2")])));
    }

    #[test]
    fn github_pull_request_details_and_each_check_are_read() {
        let raw = br#"{"number":42,"url":"https://github.com/o/r/pull/42","state":"OPEN","title":"Fix login","isDraft":true,
            "mergeable":"CONFLICTING","reviewDecision":"CHANGES_REQUESTED","statusCheckRollup":[
              {"__typename":"CheckRun","name":"build","workflowName":"CI","status":"COMPLETED","conclusion":"SUCCESS","detailsUrl":"https://github.com/o/r/runs/1"},
              {"__typename":"CheckRun","name":"test","workflowName":"CI","status":"COMPLETED","conclusion":"FAILURE","detailsUrl":"https://u:tok@github.com/o/r/runs/2"},
              {"__typename":"CheckRun","name":"lint","workflowName":"CI","status":"IN_PROGRESS","conclusion":"","detailsUrl":"javascript:alert(1)"},
              {"__typename":"CheckRun","name":"docs","status":"COMPLETED","conclusion":"SKIPPED"},
              {"__typename":"StatusContext","context":"vercel","state":"PENDING","targetUrl":"https://vercel.com/x"}
            ]}"#;
        let (pr, checks) = parse_github_pr_detail(raw).expect("parsed");
        assert_eq!((pr.number, pr.state.as_str(), pr.is_draft), (42, "open", true));
        assert_eq!((pr.mergeable.as_str(), pr.review_decision.as_str()), ("conflicting", "changesRequested"));
        let states: Vec<(&str, &str)> = checks.iter().map(|c| (c.name.as_str(), c.state.as_str())).collect();
        assert_eq!(
            states,
            [("CI / build", "passed"), ("CI / test", "failed"), ("CI / lint", "pending"), ("docs", "skipped"), ("vercel", "pending")]
        );
        assert_eq!(checks[1].url, "https://github.com/o/r/runs/2", "credentials leave links");
        assert_eq!(checks[2].url, "", "only http(s) links");
    }

    #[test]
    fn gitlab_merge_requests_and_their_jobs_are_read() {
        let raw = br#"{"iid":7,"web_url":"https://gitlab.com/g/p/-/merge_requests/7","state":"opened","title":"MR","draft":false,
            "has_conflicts":false,"detailed_merge_status":"requested_changes","project_id":99,"head_pipeline":{"id":555,"status":"failed"}}"#;
        let (pr, project, pipeline) = parse_gitlab_mr_detail(raw).expect("parsed");
        assert_eq!((pr.number, pr.state.as_str(), pr.review_decision.as_str()), (7, "open", "changesRequested"));
        assert_eq!((project, pipeline), (Some(99), Some(555)));
        let jobs = br#"[{"name":"build","stage":"build","status":"success","web_url":"https://gitlab.com/j/1"},
            {"name":"test","stage":"test","status":"failed","allow_failure":false},
            {"name":"flaky","stage":"test","status":"failed","allow_failure":true},
            {"name":"deploy","stage":"deploy","status":"manual"},
            {"name":"e2e","stage":"test","status":"running"}]"#;
        let states: Vec<String> = parse_gitlab_jobs(jobs).unwrap().into_iter().map(|c| format!("{}={}", c.name, c.state)).collect();
        assert_eq!(
            states,
            ["build / build=passed", "test / test=failed", "test / flaky=skipped", "deploy / deploy=skipped", "test / e2e=pending"]
        );
    }

    #[test]
    fn github_review_threads_keep_the_unresolved_ones_redacted() {
        let raw = br#"{"data":{"repository":{"pullRequest":{"reviewThreads":{"nodes":[
            {"id":"T1","isResolved":false,"isOutdated":false,"path":"src/a.ts","line":120,"originalLine":118,
             "comments":{"totalCount":3,"nodes":[{"author":{"login":"reviewer"},"body":"Null is not handled here.\ntoken = ghp_0123456789abcdefghijklmnopqrstuvwxyz\nsee https://bob:pw@example.com/x","url":"https://github.com/o/r/pull/42#discussion_r1"}]}},
            {"id":"T2","isResolved":true,"isOutdated":false,"path":"src/b.rs","line":3,"originalLine":3,
             "comments":{"totalCount":1,"nodes":[{"author":{"login":"x"},"body":"done","url":"https://github.com/o/r/pull/42#discussion_r2"}]}},
            {"id":"T3","isResolved":false,"isOutdated":true,"path":"src/c.rs","line":null,"originalLine":33,
             "comments":{"totalCount":1,"nodes":[{"author":null,"body":"Rename this.","url":"https://github.com/o/r/pull/42#discussion_r3"}]}}
        ]}}}}}"#;
        let threads = parse_github_review_threads(raw).expect("parsed");
        assert_eq!(threads.len(), 2, "the resolved thread is left out");
        let first = &threads[0];
        assert_eq!((first.path.as_str(), first.line, first.author.as_str(), first.replies), ("src/a.ts", Some(120), "reviewer", 2));
        assert!(first.body.starts_with("Null is not handled here."));
        assert!(!first.body.contains("ghp_"), "{}", first.body);
        assert!(first.body.contains("[redacted: GitHub token]"));
        assert!(first.body.contains("https://example.com/x") && !first.body.contains("bob:pw"));
        let outdated = &threads[1];
        assert_eq!(
            (outdated.line, outdated.outdated, outdated.author.as_str()),
            (Some(33), true, "ghost"),
            "originalLine when line is gone"
        );
        assert!(parse_github_review_threads(br#"{"errors":[{"message":"Could not resolve"}]}"#).is_none());
    }

    #[test]
    fn gitlab_discussions_keep_the_unresolved_resolvable_ones() {
        let raw = br#"[
            {"id":"d1","notes":[{"id":11,"body":"Handle null","author":{"username":"rev"},"resolvable":true,"resolved":false,
              "position":{"new_path":"src/a.ts","new_line":120,"old_path":"src/a.ts","old_line":118}},
              {"id":12,"body":"ok","author":{"username":"me"},"resolvable":true,"resolved":false}]},
            {"id":"d2","notes":[{"id":21,"body":"fixed","author":{"username":"rev"},"resolvable":true,"resolved":true}]},
            {"id":"d3","notes":[{"id":31,"body":"thanks","author":{"username":"rev"},"resolvable":false}]},
            {"id":"d4","notes":[{"id":41,"body":"added 1 commit","system":true,"resolvable":false}]},
            {"id":"d5","notes":[{"id":51,"body":"removed line","author":{"username":"rev"},"resolvable":true,"resolved":false,
              "position":{"new_path":null,"new_line":null,"old_path":"src/old.rs","old_line":9}}]}
        ]"#;
        let threads = parse_gitlab_discussions(raw, "https://gitlab.com/g/p/-/merge_requests/7").expect("parsed");
        let places: Vec<(&str, Option<i64>, usize)> = threads.iter().map(|t| (t.path.as_str(), t.line, t.replies)).collect();
        assert_eq!(places, [("src/a.ts", Some(120), 1), ("src/old.rs", Some(9), 0)]);
        assert_eq!(threads[0].url, "https://gitlab.com/g/p/-/merge_requests/7#note_11");
        assert_eq!(threads[0].author, "rev");
    }

    #[test]
    fn failures_say_whether_there_is_no_pull_request_or_no_login() {
        assert_eq!(classify_failure("no pull requests found for branch \"mighty/a\""), "noPr");
        assert_eq!(classify_failure("To get started with GitHub CLI, please run:  gh auth login"), "notLoggedIn");
        assert_eq!(classify_failure("no open merge request available for \"x\""), "noPr");
        assert_eq!(classify_failure("HTTP 502"), "failed");
    }

    #[test]
    fn bodies_are_redacted_trimmed_and_capped() {
        assert_eq!(clean_body("  fine\r\nAKIAABCDEFGHIJKLMNOP\n"), "fine\n[redacted: AWS access key]");
        let long = "x".repeat(MAX_BODY_CHARS + 50);
        let capped = clean_body(&long);
        assert_eq!(capped.chars().count(), MAX_BODY_CHARS);
        assert!(capped.ends_with('…'));
    }

    #[test]
    fn a_checklist_keeps_what_is_not_done() {
        let steps = vec![("a".to_string(), "completed".to_string()), ("b".into(), "in_progress".into()), ("c".into(), "pending".into())];
        let facts = checklist_of(&steps);
        assert_eq!((facts.total, facts.remaining), (3, vec!["b".to_string(), "c".to_string()]));
    }
}
