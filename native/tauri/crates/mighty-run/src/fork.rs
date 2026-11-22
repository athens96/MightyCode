//! MC-28: a pane's conversation forked into a new pane (`새 worktree로 갈라내기`, `여기서 갈라내기`). Where the agent's CLI forks a
//! saved session itself ([`crate::providers::cli_fork`]) the new pane's first run does that; elsewhere, or when the fork stops at an
//! earlier request than the last, the first request carries a handoff summary of the source's work: the user's last request, the plan
//! if there was one, the last result and the files the work changed, each cut to its share of [`FORK_HANDOFF_LIMIT`], every line
//! that looks like a secret replaced by the redactor the host gives.

use mighty_core::model::{FORK_HANDOFF_LIMIT, RunSession};

/// The request the fork took the conversation up to, read from the source pane.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ForkPoint {
    /// The user's request at that point.
    pub request: String,
    /// The plan the agent wrote for it, when there was one.
    pub plan: Option<String>,
    /// The agent's answer.
    pub result: String,
    /// Whether that request is the pane's last one (a CLI fork carries the whole session, so only then can it be used).
    pub is_last: bool,
}

const REQUEST_SHARE: usize = 2_000;
const PLAN_SHARE: usize = 3_500;
const RESULT_SHARE: usize = 4_000;
const FILES_SHOWN: usize = 60;

/// The request `request_id` names in `pane` (a graph run id), or the last one; None when the pane has no request yet.
pub fn fork_point(pane: &RunSession, request_id: Option<&str>) -> Option<ForkPoint> {
    let runs = pane.graph_runs();
    // A notice run belongs to the request before it: requests are the runs a person sent.
    let requests: Vec<usize> = (0..runs.len()).filter(|i| runs[*i].notice_run != Some(true)).collect();
    if !requests.is_empty() {
        let at = match request_id {
            Some(id) => *requests.iter().find(|i| runs[**i].id() == id)?,
            None => *requests.last()?,
        };
        let run = &runs[at];
        let result = run
            .final_output
            .clone()
            .filter(|t| !t.trim().is_empty())
            .or_else(|| {
                run.result_entries.as_ref().and_then(|e| e.iter().rev().find(|e| e.kind() == "assistant")).map(|e| e.text().to_string())
            })
            .unwrap_or_default();
        let plan = pane
            .plan_history
            .as_deref()
            .unwrap_or(&[])
            .iter()
            .rev()
            .find(|p| p.graph_run_id.as_deref() == Some(run.id()))
            .and_then(|p| p.plan.clone())
            .filter(|p| !p.trim().is_empty());
        return Some(ForkPoint { request: run.input().to_string(), plan, result, is_last: requests.last() == Some(&at) });
    }
    // A pane without diagrams (a request id cannot name a point in it): its last request and answer.
    if request_id.is_some() {
        return None;
    }
    let logs = pane.logs();
    let user = logs.iter().rposition(|e| e.kind() == "user")?;
    let result = logs[user..].iter().rev().find(|e| e.kind() == "assistant").map(|e| e.text().to_string()).unwrap_or_default();
    let plan = pane.plan_history.as_deref().and_then(|p| p.last()).and_then(|p| p.plan.clone()).filter(|p| !p.trim().is_empty());
    Some(ForkPoint { request: logs[user].text().to_string(), plan, result, is_last: true })
}

/// `text` cut to `limit` characters, saying it was cut.
fn cut(text: &str, limit: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= limit {
        return trimmed.to_string();
    }
    let kept: String = trimmed.chars().take(limit).collect();
    format!("{kept}\n[… cut]")
}

/// The handoff summary a forked pane's first request carries: what the source pane (titled `title`) did up to `point`, with the files
/// its work changed. `redact` replaces each line that looks like a secret (the host's `redact_secret_text`). At most
/// [`FORK_HANDOFF_LIMIT`] characters.
pub fn handoff_summary(title: &str, point: &ForkPoint, changed_files: &[String], redact: &dyn Fn(&str) -> String) -> String {
    let mut out = String::new();
    let title = title.trim();
    out.push_str("[Mighty Code] This pane was forked from another pane");
    if !title.is_empty() {
        out.push_str(&format!(" (\"{}\")", cut(title, 120)));
    }
    out.push_str(". Its conversation does not come along, so here is what was done there so far. Carry on from it.\n");
    if !point.request.trim().is_empty() {
        out.push_str("\n## The user's last request there\n");
        out.push_str(&redact(&cut(&point.request, REQUEST_SHARE)));
        out.push('\n');
    }
    if let Some(plan) = point.plan.as_deref().filter(|p| !p.trim().is_empty()) {
        out.push_str("\n## The plan\n");
        out.push_str(&redact(&cut(plan, PLAN_SHARE)));
        out.push('\n');
    }
    if !point.result.trim().is_empty() {
        out.push_str("\n## The last result\n");
        out.push_str(&redact(&cut(&point.result, RESULT_SHARE)));
        out.push('\n');
    }
    if !changed_files.is_empty() {
        out.push_str("\n## Files changed so far (already in this worktree when they were carried over)\n");
        for file in changed_files.iter().take(FILES_SHOWN) {
            out.push_str("- ");
            out.push_str(&redact(&cut(file, 300)));
            out.push('\n');
        }
        if changed_files.len() > FILES_SHOWN {
            out.push_str(&format!("- … and {} more\n", changed_files.len() - FILES_SHOWN));
        }
    }
    out.push_str("\n## The new request\n");
    if out.chars().count() > FORK_HANDOFF_LIMIT {
        let kept: String = out.chars().take(FORK_HANDOFF_LIMIT - 40).collect();
        out = format!("{kept}\n[… cut]\n\n## The new request\n");
    }
    out
}

/// The request a forked pane's first run sends: the summary, then what was typed.
pub fn with_handoff(handoff: &str, input: &str) -> String {
    format!("{}\n{input}", handoff.trim_end())
}
