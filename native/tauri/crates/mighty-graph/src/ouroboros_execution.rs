//! Ouroboros executions (C# `Core\OuroborosExecution.cs`; plan §1.8 "The Ouroboros dashboard", W9-E): the pure half.
//!
//! - which tool results name a background execution (`OuroborosExecutionLinks`: [`extract`], [`parse`], [`valid_id`]);
//! - what is ever connected to: plain http on a loopback host with a port named ([`endpoint`], [`DashboardUrl`]), the daemon's own record
//!   ([`endpoint_from_state`]) and the three addresses ([`runs_url`], [`events_url`], [`page_url`]);
//! - the dashboard's two answers read tolerantly ([`summaries`], [`sse_data`], [`board`]) and merged into what a block shows
//!   ([`Snapshot::merge`]);
//! - when one execution is read next ([`Poll`]) and how sparingly the daemon is started ([`DaemonStarts`]);
//! - the daemon's start as data: its arguments, environment and interpreter ([`ensure_arguments`], [`daemon_environment`],
//!   [`tool_pythons`]);
//! - the block's card as the page reads it (C# `WebExecutionCard`, [`Dashboard`] as the projection's [`ExecutionDashboard`]).
//!
//! The loopback client, the pane's monitor and the daemon's start through `ProcessHost` are src-tauri's `ouroboros.rs`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};

use chrono::{DateTime, TimeDelta, Utc};
use mighty_core::graph::MightyGraphRun;
use mighty_core::locale::Locale;
use mighty_core::model::LogEntry;
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use regex::Regex;
use serde_json::{Map, Value, json};

use crate::node;
use crate::view::ExecutionDashboard;

// ── links ─────────────────────────────────────────────────────────────────────────────────────────────────────────────────────

/// `OuroborosExecutionLinks.StartTools`: the style manifest's `job.open` tools.
pub const START_TOOLS: [&str; 4] =
    ["ouroboros_start_execute_seed", "ouroboros_start_auto", "ouroboros_start_evolve_step", "ouroboros_start_ralph"];

/// What a start prints while it has no id yet (`Execution ID: None` from an evolve step whose generation is claimed later).
const PLACEHOLDERS: [&str; 3] = ["pending", "none", "null"];

/// `OuroborosExecutionLink`: a background execution an agent started from a pane. `run_id` is the graph run whose tool result named it;
/// `dashboard_url` is loopback only (anything else in the text is dropped).
#[derive(Debug, Clone, PartialEq)]
pub struct Link {
    pub run_id: String,
    pub tool: String,
    pub job_id: Option<String>,
    pub session_id: Option<String>,
    pub execution_id: Option<String>,
    pub dashboard_url: Option<DashboardUrl>,
    pub timestamp: String,
}

impl Link {
    /// `Key`: one block per execution; a start that named only a job keys on the job.
    pub fn key(&self) -> String {
        match &self.execution_id {
            Some(id) => id.clone(),
            None => format!("job-{}", self.job_id.as_deref().unwrap_or("")),
        }
    }
}

/// `ShortName`: Claude names an MCP tool `mcp__<server>__<tool>`, Codex `<server>.<tool>`.
pub fn short_name(wire: &str) -> &str {
    let mut name = wire;
    if let Some(at) = name.rfind("__") {
        name = &name[at + 2..];
    }
    if let Some(at) = name.rfind('.') {
        name = &name[at + 1..];
    }
    name
}

/// `ValidID`: ids go into URLs (as a query item) and a process argument of their own — letters, digits, `_`, `-` and `:` (an evolve
/// generation is `evolve:<lineage>:generation:<n>`), at most 200, never led by `-` or `:`, so none can read as an option or a path.
pub fn valid_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 200
        && bytes[0].is_ascii_alphanumeric()
        && bytes[1..].iter().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b':' | b'-'))
}

fn placeholder(value: &str) -> bool {
    PLACEHOLDERS.iter().any(|p| p.eq_ignore_ascii_case(value))
}

/// `Parse`: the labelled lines of one start tool's result text. A placeholder and anything that is not a strict id count as absent.
pub fn parse(text: &str, tool: &str, run_id: &str, timestamp: &str) -> Option<Link> {
    let mut values: HashMap<String, String> = HashMap::new();
    for raw in text.split(['\n', '\r']).filter(|r| !r.is_empty()).take(400) {
        let line = raw.trim_matches([' ', '\t']);
        let Some(colon) = line.find(':') else { continue };
        let label = line[..colon].trim_matches([' ', '\t']).to_lowercase();
        let value = line[colon + 1..].trim_matches([' ', '\t']);
        if value.is_empty() || value.contains(' ') || values.contains_key(&label) {
            continue;
        }
        values.insert(label, value.to_string());
    }
    let id = |labels: &[&str]| labels.iter().filter_map(|l| values.get(*l)).find(|v| !placeholder(v) && valid_id(v)).cloned();
    let dashboard = values.get("live dashboard").and_then(|address| dashboard_url(address));
    // A run URL names the execution even when the text still says pending.
    let run_parameter = dashboard.as_ref().and_then(|d| query_item(&d.original, "run"));
    let execution = id(&["execution id"]).or_else(|| run_parameter.filter(|r| valid_id(r) && !placeholder(r)));
    let job = id(&["job id", "job_id"]);
    if execution.is_none() && job.is_none() {
        return None;
    }
    Some(Link {
        run_id: run_id.to_string(),
        tool: tool.to_string(),
        job_id: job,
        session_id: id(&["session id", "auto_session_id"]),
        execution_id: execution,
        dashboard_url: dashboard,
        timestamp: timestamp.to_string(),
    })
}

/// `QueryItem`: the decoded value of a query item of an address, none when it has none.
pub fn query_item(url: &str, name: &str) -> Option<String> {
    let query = url.split_once('?').map(|(_, q)| q.split('#').next().unwrap_or(""))?;
    let decode = |s: &str| percent_encoding::percent_decode_str(s).decode_utf8_lossy().into_owned();
    for item in query.split('&').filter(|i| !i.is_empty()) {
        let (key, value) = match item.split_once('=') {
            Some((key, value)) => (key, Some(value)),
            None => (item, None),
        };
        if decode(key) == name {
            return Some(value.map(decode).unwrap_or_default());
        }
    }
    None
}

/// `Extract`: every execution a start tool reported in these runs, once each, on the first run that reported it. A failed start named
/// nothing that runs.
pub fn extract(runs: &[MightyGraphRun]) -> Vec<Link> {
    let mut seen = HashSet::new();
    let mut links = vec![];
    // Called on every projection: the cheap name test runs first.
    let mut scan = |entries: &[LogEntry], run_id: &str| {
        for entry in entries {
            let Some(activity) = &entry.activity else { continue };
            let (Some(tool), Some(output)) = (&activity.tool_name, &activity.output) else { continue };
            if activity.state != "completed" || !tool.contains("ouroboros_start_") {
                continue;
            }
            let name = short_name(tool);
            if START_TOOLS.contains(&name)
                && let Some(link) = parse(output, name, run_id, entry.timestamp.as_deref().unwrap_or(""))
                && seen.insert(link.key())
            {
                links.push(link);
            }
        }
    };
    for run in runs {
        scan(run.root_entries(), run.id());
        for agent in run.agents() {
            scan(agent.entries(), run.id());
        }
    }
    links
}

// ── addresses ─────────────────────────────────────────────────────────────────────────────────────────────────────────────────

/// `OuroborosDashboardEndpoint`: a loopback host and its port.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Endpoint {
    pub host: String,
    pub port: u16,
}

/// `Uri.EscapeDataString`: everything but the unreserved characters.
const DATA: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'_').remove(b'.').remove(b'~');

impl Endpoint {
    pub fn new(host: &str, port: u16) -> Endpoint {
        Endpoint { host: host.to_string(), port }
    }

    /// `Url(path, run)`: `path` starts with "/"; `run` goes in as one encoded query item.
    pub fn url(&self, path: &str, run: Option<&str>) -> DashboardUrl {
        let host = if self.host.contains(':') { format!("[{}]", self.host) } else { self.host.clone() };
        let query = run.map(|r| format!("?run={}", utf8_percent_encode(r, DATA))).unwrap_or_default();
        let rest = format!("{path}{query}");
        DashboardUrl { original: format!("http://{host}:{}{rest}", self.port), endpoint: self.clone(), rest }
    }
}

/// A dashboard address that passed [`endpoint`]: what was written (`OriginalString`), its endpoint and what follows the authority.
/// Two are the same address when their [`DashboardUrl::absolute`] forms are (as .NET compares a `Uri`).
#[derive(Debug, Clone)]
pub struct DashboardUrl {
    pub original: String,
    pub endpoint: Endpoint,
    rest: String,
}

impl PartialEq for DashboardUrl {
    fn eq(&self, other: &Self) -> bool {
        self.absolute() == other.absolute()
    }
}

impl DashboardUrl {
    /// `AbsoluteUri`: the host lowercased (an IPv6 one in brackets), the scheme's own port left out, an empty path as "/".
    pub fn absolute(&self) -> String {
        let host = if self.endpoint.host.contains(':') && !self.endpoint.host.starts_with('[') {
            format!("[{}]", self.endpoint.host)
        } else {
            self.endpoint.host.clone()
        };
        let port = if self.endpoint.port == 80 { String::new() } else { format!(":{}", self.endpoint.port) };
        let rest = if self.rest.starts_with('/') { self.rest.clone() } else { format!("/{}", self.rest) };
        format!("http://{host}{port}{rest}")
    }

    /// The request target sent to the dashboard: the path and the query, never the fragment.
    pub fn target(&self) -> String {
        let rest = self.rest.split('#').next().unwrap_or("");
        if rest.starts_with('/') { rest.to_string() } else { format!("/{rest}") }
    }
}

/// `IsLoopback`.
pub fn is_loopback(host: &str) -> bool {
    matches!(host.to_lowercase().as_str(), "127.0.0.1" | "localhost" | "::1" | "[::1]")
}

/// `Endpoint(Uri)` over an address as written: a `Live Dashboard:` URL is agent-influenced text, so only plain http on a loopback host
/// with a port written in the address itself (the scheme's own default does not count) and no user info is ever connected to.
pub fn dashboard_url(text: &str) -> Option<DashboardUrl> {
    let start = text.find("://")?;
    if !text[..start].eq_ignore_ascii_case("http") {
        return None;
    }
    let after = &text[start + 3..];
    let end = after.find(['/', '?', '#', '\\']).unwrap_or(after.len());
    let (authority, rest) = after.split_at(end);
    if authority.contains('@') {
        return None;
    }
    // `NamesPort`: a colon after any bracket, followed by digits only.
    let colon = authority.rfind(':')?;
    if authority.rfind(']').is_some_and(|b| b > colon) || colon + 1 >= authority.len() {
        return None;
    }
    let digits = &authority[colon + 1..];
    if !digits.bytes().all(|b| b.is_ascii_digit()) || rest.starts_with('\\') {
        return None;
    }
    let port: u32 = digits.parse().ok()?;
    let port = u16::try_from(port).ok().filter(|p| *p >= 1)?;
    let raw = &authority[..colon];
    let host = if let Some(inner) = raw.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
        // .NET writes an IPv6 host in its shortest form: only the loopback one passes.
        (inner.parse::<std::net::Ipv6Addr>().ok()? == std::net::Ipv6Addr::LOCALHOST).then(|| "::1".to_string())?
    } else {
        raw.to_lowercase()
    };
    if host.is_empty() || !is_loopback(&host) {
        return None;
    }
    Some(DashboardUrl { original: text.to_string(), endpoint: Endpoint { host, port }, rest: rest.to_string() })
}

/// `Endpoint(Uri)`: the endpoint of an address that passes [`dashboard_url`].
pub fn endpoint(text: &str) -> Option<Endpoint> {
    dashboard_url(text).map(|u| u.endpoint)
}

/// `Endpoint(stateJson)`: `~/.ouroboros/dashboard.json`. A daemon bound to every interface is still reached on loopback, as its own
/// health probe does.
pub fn endpoint_from_state(state: &[u8]) -> Option<Endpoint> {
    let root: Value = serde_json::from_slice(state).ok()?;
    let root = root.as_object()?;
    let port = root.get("port").filter(|p| p.is_number())?.as_i64().filter(|p| (1..=65_535).contains(p))?;
    let raw = root.get("host").and_then(Value::as_str).unwrap_or("127.0.0.1");
    let host = if raw.is_empty() || raw == "0.0.0.0" { "127.0.0.1" } else { raw };
    is_loopback(host).then(|| Endpoint { host: host.to_string(), port: port as u16 })
}

/// `RunsURL`.
pub fn runs_url(endpoint: &Endpoint) -> DashboardUrl {
    endpoint.url("/api/runs", None)
}

/// `EventsURL`: none for an id that is not strict.
pub fn events_url(endpoint: &Endpoint, execution_id: &str) -> Option<DashboardUrl> {
    valid_id(execution_id).then(|| endpoint.url("/events", Some(execution_id)))
}

/// `PageURL`: the page "open dashboard" shows — the run's board, or the run picker when the start named no execution.
pub fn page_url(endpoint: &Endpoint, execution_id: Option<&str>) -> DashboardUrl {
    endpoint.url("/", execution_id.filter(|id| valid_id(id)))
}

// ── payloads ──────────────────────────────────────────────────────────────────────────────────────────────────────────────────

/// `OuroborosExecutionStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExecutionStatus {
    Running,
    Paused,
    Completed,
    Failed,
    Cancelled,
    Unknown,
}

impl ExecutionStatus {
    pub const ALL: [ExecutionStatus; 6] = [Self::Running, Self::Paused, Self::Completed, Self::Failed, Self::Cancelled, Self::Unknown];

    /// The status' name in lower case (`status.ToString().ToLowerInvariant()`).
    pub fn word(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Unknown => "unknown",
        }
    }

    /// `IsTerminal`.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

/// `OuroborosDashboard.Status`: a wire status by its name in any case; anything else is unknown.
pub fn status(wire: Option<&str>) -> ExecutionStatus {
    let Some(wire) = wire.map(str::trim) else { return ExecutionStatus::Unknown };
    ExecutionStatus::ALL.into_iter().find(|s| s.word().eq_ignore_ascii_case(wire)).unwrap_or(ExecutionStatus::Unknown)
}

/// `OuroborosACStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AcStatus {
    Pending,
    Executing,
    Completed,
    Failed,
}

impl AcStatus {
    pub fn word(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Executing => "executing",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }
}

/// `OuroborosACItem`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcItem {
    pub id: String,
    pub title: String,
    pub status: AcStatus,
    pub depth: i32,
}

impl AcItem {
    pub fn new(id: &str, title: &str, status: AcStatus) -> AcItem {
        AcItem { id: id.into(), title: title.into(), status, depth: 0 }
    }
}

/// `OuroborosRunSummary`: one row of `/api/runs`; every field but the id may be missing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunSummary {
    pub execution_id: String,
    pub status: ExecutionStatus,
    pub goal: Option<String>,
    pub phase: Option<String>,
    pub activity: Option<String>,
    pub completed: Option<i32>,
    pub total: Option<i32>,
    pub pending: Option<i32>,
    pub executing: Option<i32>,
    pub failed: Option<i32>,
}

impl RunSummary {
    pub fn new(execution_id: &str, status: ExecutionStatus) -> RunSummary {
        RunSummary {
            execution_id: execution_id.into(),
            status,
            goal: None,
            phase: None,
            activity: None,
            completed: None,
            total: None,
            pending: None,
            executing: None,
            failed: None,
        }
    }
}

/// `OuroborosBoard`: the Kanban board one `/events` frame carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Board {
    pub goal: Option<String>,
    pub phase: Option<String>,
    pub activity: Option<String>,
    pub completed: Option<i32>,
    pub total: Option<i32>,
    pub items: Arc<Vec<AcItem>>,
}

impl Board {
    pub fn count(&self, status: AcStatus) -> i32 {
        self.items.iter().filter(|i| i.status == status).count() as i32
    }
}

fn count(owner: &Value, name: &str) -> Option<i32> {
    let number = owner.as_object()?.get(name)?.as_f64()?;
    (number.is_finite() && (0.0..=1_000_000.0).contains(&number)).then_some(number as i32)
}

fn string(owner: &Value, name: &str) -> Option<String> {
    let text = owner.as_object()?.get(name)?.as_str().filter(|t| !t.is_empty())?;
    // The first 2,000 UTF-16 units, as C# cuts it.
    let mut units = 0;
    let cut = text.char_indices().find(|(_, c)| {
        units += c.len_utf16();
        units > 2_000
    });
    Some(cut.map_or(text, |(at, _)| &text[..at]).to_string())
}

/// `Summaries`: `/api/runs` keyed by execution id; none when the body is not that shape.
pub fn summaries(data: &[u8]) -> Option<HashMap<String, RunSummary>> {
    let root: Value = serde_json::from_slice(data).ok()?;
    let runs = root.as_object()?.get("runs")?.as_array()?;
    let mut result = HashMap::new();
    for run in runs.iter().take(500) {
        let Some(id) = run.as_object().and_then(|o| o.get("execution_id")).and_then(Value::as_str) else { continue };
        if !valid_id(id) || result.contains_key(id) {
            continue;
        }
        result.insert(
            id.to_string(),
            RunSummary {
                execution_id: id.to_string(),
                status: status(run.get("status").and_then(Value::as_str)),
                goal: string(run, "goal"),
                phase: string(run, "phase"),
                activity: string(run, "activity"),
                completed: count(run, "completed_count"),
                total: count(run, "total_count"),
                pending: count(run, "pending_count"),
                executing: count(run, "executing_count"),
                failed: count(run, "failed_count"),
            },
        );
    }
    Some(result)
}

/// `SseData`: complete SSE events' `data` in order. Comments and other fields are skipped; a trailing event without its blank line is
/// not complete yet.
pub fn sse_data(text: &str) -> Vec<String> {
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let events: Vec<&str> = text.split("\n\n").collect();
    let mut result = vec![];
    for block in &events[..events.len() - 1] {
        let lines: Vec<&str> =
            block.split('\n').filter_map(|line| line.strip_prefix("data:")).map(|value| value.strip_prefix(' ').unwrap_or(value)).collect();
        if !lines.is_empty() {
            result.push(lines.join("\n"));
        }
    }
    result
}

/// `Board`: one board frame. Cards the reducer put in no known column are kept as executing, as the dashboard itself shows them.
pub fn board(data: &[u8]) -> Option<Board> {
    let root: Value = serde_json::from_slice(data).ok()?;
    let object = root.as_object()?;
    let columns = object.get("columns")?.as_object()?;
    let meta = object.get("meta").cloned().unwrap_or(Value::Null);
    let mut items: Vec<(i32, AcItem)> = vec![];
    let mut ids = HashSet::new();
    for (name, column) in columns {
        let status = match name.as_str() {
            "pending" => AcStatus::Pending,
            "completed" => AcStatus::Completed,
            "failed" => AcStatus::Failed,
            _ => AcStatus::Executing,
        };
        let Some(cards) = column.as_array() else { continue };
        for card in cards.iter().take(1_000) {
            let Some(id) = string(card, "id") else { continue };
            if !ids.insert(id.clone()) {
                continue;
            }
            let title = node::clean(&string(card, "title").unwrap_or_else(|| id.clone()), 400, true);
            let title = if title.is_empty() { id.clone() } else { title };
            let depth = count(card, "depth").unwrap_or(0).min(4);
            items.push((count(card, "ac_index").unwrap_or(i32::MAX), AcItem { id, title, status, depth }));
        }
    }
    items.sort_by(|(a, x), (b, y)| a.cmp(b).then(x.depth.cmp(&y.depth)).then_with(|| x.id.cmp(&y.id)));
    Some(Board {
        goal: string(&meta, "goal"),
        phase: string(&meta, "phase"),
        activity: string(&meta, "activity"),
        completed: count(&meta, "completed"),
        total: count(&meta, "total"),
        items: Arc::new(items.into_iter().map(|(_, item)| item).collect()),
    })
}

/// `OuroborosExecutionSnapshot.Sources`: where what a block shows came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Source {
    /// The dashboard answered.
    Live,
    /// Nothing answered on loopback.
    Unreachable,
    /// The dashboard answered but could not read its runs (a non-200 such as 503). It is up: never started again for this.
    Unreadable,
    /// The start named only a job: there is no execution to look up yet.
    NoExecution,
}

/// `OuroborosExecutionSnapshot`: what a graph block shows for one execution. `items` is none until a board was read.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub source: Source,
    pub status: ExecutionStatus,
    pub goal: Option<String>,
    pub phase: Option<String>,
    pub activity: Option<String>,
    pub completed: Option<i32>,
    pub total: Option<i32>,
    pub pending: Option<i32>,
    pub executing: Option<i32>,
    pub failed: Option<i32>,
    pub items: Option<Arc<Vec<AcItem>>>,
    pub dashboard_url: Option<DashboardUrl>,
}

impl Snapshot {
    pub fn new(source: Source) -> Snapshot {
        Snapshot {
            source,
            status: ExecutionStatus::Unknown,
            goal: None,
            phase: None,
            activity: None,
            completed: None,
            total: None,
            pending: None,
            executing: None,
            failed: None,
            items: None,
            dashboard_url: None,
        }
    }

    /// `Merge`: the summary owns status and counts; a board fills what it lacks and alone supplies the AC list. Neither is a contract,
    /// so either may be missing.
    pub fn merge(
        summary: Option<&RunSummary>,
        board: Option<&Board>,
        previous_items: Option<&Arc<Vec<AcItem>>>,
        dashboard_url: Option<DashboardUrl>,
    ) -> Snapshot {
        let text = |value: Option<&String>| value.map(|v| node::clean(v, 600, true)).filter(|c| !c.is_empty());
        let counted = |status: AcStatus| board.map(|b| b.count(status));
        Snapshot {
            source: Source::Live,
            status: summary.map(|s| s.status).unwrap_or_else(|| board.map_or(ExecutionStatus::Unknown, settled_status)),
            goal: text(summary.and_then(|s| s.goal.as_ref()).or(board.and_then(|b| b.goal.as_ref()))),
            phase: text(summary.and_then(|s| s.phase.as_ref()).or(board.and_then(|b| b.phase.as_ref()))),
            activity: text(summary.and_then(|s| s.activity.as_ref()).or(board.and_then(|b| b.activity.as_ref()))),
            completed: summary.and_then(|s| s.completed).or(board.and_then(|b| b.completed)).or_else(|| counted(AcStatus::Completed)),
            total: summary.and_then(|s| s.total).or_else(|| board.map(|b| b.total.filter(|t| *t > 0).unwrap_or(b.items.len() as i32))),
            pending: summary.and_then(|s| s.pending).or_else(|| counted(AcStatus::Pending)),
            executing: summary.and_then(|s| s.executing).or_else(|| counted(AcStatus::Executing)),
            failed: summary.and_then(|s| s.failed).or_else(|| counted(AcStatus::Failed)),
            items: board.map(|b| b.items.clone()).or_else(|| previous_items.cloned()),
            dashboard_url,
        }
    }
}

/// `SettledStatus`: a run `/api/runs` no longer lists (it keeps only the newest) has no status but its board — with nothing pending or
/// executing and every AC settled, it failed when any AC failed and completed otherwise (the dashboard's own fallback). Anything else
/// stays unknown.
pub fn settled_status(board: &Board) -> ExecutionStatus {
    let (completed, failed) = (board.count(AcStatus::Completed), board.count(AcStatus::Failed));
    if board.count(AcStatus::Pending) != 0
        || board.count(AcStatus::Executing) != 0
        || completed + failed <= 0
        || completed + failed < board.total.unwrap_or(0)
    {
        return ExecutionStatus::Unknown;
    }
    if failed > 0 { ExecutionStatus::Failed } else { ExecutionStatus::Completed }
}

/// `BlockStatus`: the activity mark's own vocabulary for an execution's block — a block nothing was read for yet is starting, one whose
/// status cannot be told is unknown.
pub fn block_status(snapshot: Option<&Snapshot>) -> &'static str {
    match snapshot.map(|s| s.status) {
        None => "starting",
        Some(ExecutionStatus::Running) => "running",
        Some(ExecutionStatus::Paused) => "waiting",
        Some(ExecutionStatus::Completed) => "completed",
        Some(ExecutionStatus::Failed) => "error",
        Some(ExecutionStatus::Cancelled) => "stopped",
        Some(ExecutionStatus::Unknown) => "unknown",
    }
}

// ── polling ───────────────────────────────────────────────────────────────────────────────────────────────────────────────────

/// `FastInterval`, `SlowInterval`.
pub const FAST_INTERVAL: TimeDelta = TimeDelta::seconds(3);
pub const SLOW_INTERVAL: TimeDelta = TimeDelta::seconds(15);
/// `QuietAfter`: no change for this long moves to the slow interval.
pub const QUIET_AFTER: TimeDelta = TimeDelta::seconds(60);
/// `StaleAfter`: a start older than this is history — the daemon is not started for it and, when nothing answers, it is read once and
/// left.
pub const STALE_AFTER: TimeDelta = TimeDelta::hours(12);
/// `UnknownBackoffAfter`, `UnknownStopAfter`: this many reads in a row that could not tell the status move to [`UNKNOWN_INTERVAL`], and
/// stop the block.
pub const UNKNOWN_BACKOFF_AFTER: i32 = 10;
pub const UNKNOWN_STOP_AFTER: i32 = 40;
pub const UNKNOWN_INTERVAL: TimeDelta = TimeDelta::seconds(180);

/// `OuroborosExecutionPoll`: when one execution is read next. Pure: the caller supplies the clock and what the last read saw.
#[derive(Debug, Clone)]
pub struct Poll {
    pub next_at: Option<DateTime<Utc>>,
    pub last_change: DateTime<Utc>,
    pub last: Option<Snapshot>,
    /// Reads in a row whose status was unknown.
    pub unknown_reads: i32,
    pub stale: bool,
}

impl Poll {
    pub fn new(now: DateTime<Utc>, stale: bool) -> Poll {
        Poll { next_at: Some(now), last_change: now, last: None, unknown_reads: 0, stale }
    }

    pub fn stopped(&self) -> bool {
        self.next_at.is_none()
    }

    pub fn is_due(&self, at: DateTime<Utc>) -> bool {
        self.next_at.is_some_and(|next| next <= at)
    }

    /// `Record`: a terminal status ends polling; so does a job-only start (nothing to look up), a stale start that nothing answers for,
    /// and a long run of reads that never told the status.
    pub fn record(&mut self, snapshot: &Snapshot, at: DateTime<Utc>) {
        if self.last.as_ref() != Some(snapshot) {
            self.last_change = at;
            self.last = Some(snapshot.clone());
        }
        self.unknown_reads = if snapshot.status == ExecutionStatus::Unknown { self.unknown_reads + 1 } else { 0 };
        if snapshot.status.is_terminal()
            || snapshot.source == Source::NoExecution
            || self.stale && snapshot.status == ExecutionStatus::Unknown
            || self.unknown_reads >= UNKNOWN_STOP_AFTER
        {
            self.next_at = None;
            return;
        }
        let wait = if self.unknown_reads >= UNKNOWN_BACKOFF_AFTER {
            UNKNOWN_INTERVAL
        } else if at - self.last_change >= QUIET_AFTER {
            SLOW_INTERVAL
        } else {
            FAST_INTERVAL
        };
        self.next_at = Some(at + wait);
    }

    /// `MayEnsureDaemon`: whether this block may ask for the daemon at all — a live execution still being read, never history.
    pub fn may_ensure_daemon(&self) -> bool {
        !self.stale && !self.stopped() && !self.last.as_ref().is_some_and(|l| l.status.is_terminal())
    }

    /// `Wake`: one more read for a stopped or waiting block, such as an AC list opened with nothing read yet; [`Poll::record`] stops it
    /// again if it was over.
    pub fn wake(&mut self, at: DateTime<Utc>) {
        self.next_at = Some(at);
    }

    /// `IsStale`: a start with no readable time (an ISO 8601 date and time with its zone) or older than [`STALE_AFTER`] is history.
    pub fn is_stale(timestamp: &str, now: DateTime<Utc>) -> bool {
        static PATTERN: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"\A\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?(Z|[+-]\d{2}:\d{2})\z").expect("the time pattern"));
        if !PATTERN.is_match(timestamp) {
            return true;
        }
        match DateTime::parse_from_rfc3339(timestamp) {
            Ok(date) => now - date.with_timezone(&Utc) > STALE_AFTER,
            Err(_) => true,
        }
    }
}

/// `OuroborosDaemonStarts`: daemon starts the graph made on its own, process-wide and keyed by execution id, so a graph that leaves the
/// screen and comes back cannot start it again sooner.
#[derive(Debug, Default)]
pub struct DaemonStarts {
    starts: Mutex<HashMap<String, (DateTime<Utc>, i32)>>,
}

/// `Cooldown`, `Maximum`.
pub const DAEMON_COOLDOWN: TimeDelta = TimeDelta::seconds(300);
pub const DAEMON_MAXIMUM: i32 = 3;

static SHARED_STARTS: LazyLock<DaemonStarts> = LazyLock::new(DaemonStarts::default);

impl DaemonStarts {
    /// `Shared`.
    pub fn shared() -> &'static DaemonStarts {
        &SHARED_STARTS
    }

    /// `Claim`: one start for the execution when its last was at least [`DAEMON_COOLDOWN`] ago and it has had fewer than
    /// [`DAEMON_MAXIMUM`]; false otherwise.
    pub fn claim(&self, execution_id: &str, now: DateTime<Utc>) -> bool {
        let mut starts = self.starts.lock().unwrap_or_else(|e| e.into_inner());
        let previous = starts.get(execution_id).copied();
        if let Some((at, count)) = previous
            && (count >= DAEMON_MAXIMUM || now - at < DAEMON_COOLDOWN)
        {
            return false;
        }
        starts.insert(execution_id.to_string(), (now, previous.map_or(0, |(_, c)| c) + 1));
        true
    }
}

// ── the daemon's start ────────────────────────────────────────────────────────────────────────────────────────────────────────

/// `BoardConcurrency`: board streams one read opens at once.
pub const BOARD_CONCURRENCY: usize = 3;
/// `FirstEventDeadline`: a stream that sent no event by then has nothing to show yet.
pub const FIRST_EVENT_DEADLINE: std::time::Duration = std::time::Duration::from_secs(2);
/// A board stream's quiet time and whole deadline (`BoardAsync`).
pub const BOARD_QUIET: std::time::Duration = std::time::Duration::from_millis(1_200);
pub const BOARD_DEADLINE: std::time::Duration = std::time::Duration::from_secs(8);
/// `/api/runs`' time (`SummariesAsync`).
pub const RUNS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(4);
/// `MaximumStateBytes`, `MaximumBoardBytes`: what is read of the daemon's own record and of one board stream at the most.
pub const MAXIMUM_STATE_BYTES: u64 = 64 * 1024;
pub const MAXIMUM_BOARD_BYTES: usize = 8 * 1024 * 1024;
/// `LoopbackDashboardTransport.MaximumBytes`: a plain answer's body at the most.
pub const MAXIMUM_BYTES: usize = 4 * 1024 * 1024;
/// The daemon's start is given this long to exit (`Capture`).
pub const DAEMON_START_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// `EnsureArguments`: the tool python's arguments; none for an id that is not strict. `-I` keeps Python from reading `PYTHON*`
/// variables, the user site and the working directory; the id is its own argument after `--run`.
pub fn ensure_arguments(execution_id: Option<&str>) -> Option<Vec<String>> {
    let base = ["-I", "-m", "ouroboros.dashboard_web"].map(String::from).to_vec();
    match execution_id {
        None => Some(base),
        Some(id) if valid_id(id) => Some([base, vec!["--run".into(), id.into()]].concat()),
        Some(_) => None,
    }
}

/// `DaemonEnvironment`: only what Python and Ouroboros need, never the app's whole environment (an agent's `PYTHONPATH` and the like).
/// The two `PYTHON*` flags reach the detached daemon, which Ouroboros starts without `-I`. On Windows a Python process also needs the
/// system's own folders and shell (`ComSpec`, `PATHEXT`), the user's folders and the machine's processor and user names. `inherited`
/// is read ignoring case on Windows (as the process' own environment is there).
pub fn daemon_environment(home: &str, inherited: &HashMap<String, String>, windows: bool) -> BTreeMap<String, String> {
    let get = |key: &str| {
        let value =
            if windows { inherited.iter().filter(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v).last() } else { inherited.get(key) };
        value.filter(|v| !v.is_empty()).cloned()
    };
    let mut environment = BTreeMap::new();
    environment.insert("HOME".to_string(), home.to_string());
    environment.insert("PYTHONSAFEPATH".to_string(), "1".to_string());
    environment.insert("PYTHONNOUSERSITE".to_string(), "1".to_string());
    if windows {
        environment.insert("USERPROFILE".to_string(), home.to_string());
        for key in [
            "SystemRoot",
            "SystemDrive",
            "windir",
            "ComSpec",
            "PATHEXT",
            "APPDATA",
            "LOCALAPPDATA",
            "ProgramData",
            "TEMP",
            "TMP",
            "USERNAME",
            "NUMBER_OF_PROCESSORS",
            "PROCESSOR_ARCHITECTURE",
        ] {
            if let Some(value) = get(key) {
                environment.insert(key.to_string(), value);
            }
        }
        let path = get("SystemRoot").map(|root| format!("{};{root}", Path::new(&root).join("System32").to_string_lossy()));
        environment.insert("PATH".to_string(), path.unwrap_or_default());
        return environment;
    }
    environment.insert("PATH".to_string(), "/usr/bin:/bin:/usr/sbin:/sbin".to_string());
    for key in ["LANG", "LC_ALL", "TMPDIR"] {
        if let Some(value) = get(key) {
            environment.insert(key.to_string(), value);
        }
    }
    if !environment.contains_key("LANG") && !environment.contains_key("LC_ALL") {
        environment.insert("LANG".to_string(), "en_US.UTF-8".to_string());
    }
    environment
}

/// `ToolPythons`: the tool environment Ouroboros is installed into (uv, then pipx). On Windows uv keeps its tools under the roaming
/// application data and pipx under the user's folder, each with the interpreter in `Scripts`.
pub fn tool_pythons(home: &Path, windows: bool, application_data: Option<&Path>) -> Vec<PathBuf> {
    if windows {
        let mut tools: Vec<PathBuf> =
            application_data.filter(|a| !a.as_os_str().is_empty()).map(|a| a.join("uv").join("tools")).into_iter().collect();
        tools.push(home.join("pipx").join("venvs"));
        tools.push(home.join(".local").join("pipx").join("venvs"));
        return tools.into_iter().map(|t| t.join("ouroboros-ai").join("Scripts").join("python.exe")).collect();
    }
    vec![
        home.join(".local").join("share").join("uv").join("tools").join("ouroboros-ai").join("bin").join("python"),
        home.join(".local").join("pipx").join("venvs").join("ouroboros-ai").join("bin").join("python"),
    ]
}

// ── the block's card ──────────────────────────────────────────────────────────────────────────────────────────────────────────

/// What the projection reads of a pane's executions (C# `graphOuroboros` with `graphExecutionLinks`): each block's last snapshot, its
/// "open dashboard" click being handled or last showing nothing, as the pane's monitor holds them when the projection is made.
#[derive(Debug, Clone)]
pub struct Dashboard {
    pub links: Vec<Link>,
    pub snapshots: HashMap<String, Snapshot>,
    pub opening: HashSet<String>,
    pub open_failed: HashSet<String>,
    pub locale: Locale,
}

impl Dashboard {
    pub fn new(links: Vec<Link>, locale: Locale) -> Dashboard {
        Dashboard { links, snapshots: HashMap::new(), opening: HashSet::new(), open_failed: HashSet::new(), locale }
    }

    /// `WebExecutionCard(key, link, listOpen)`: a background execution's card (`WebMightyExecution`), its words built here.
    pub fn execution_card(&self, key: &str, link: &Link, list_open: bool) -> Value {
        let l = &self.locale;
        let snapshot = self.snapshots.get(key);
        let status = snapshot.map_or(ExecutionStatus::Unknown, |s| s.status);
        let word = status.word();
        let mut card = Map::new();
        card.insert("status".into(), json!(word));
        card.insert("statusWord".into(), json!(l.get(&format!("graph.ouroboros.status.{word}"))));
        if let Some(goal) = snapshot.and_then(|s| s.goal.as_ref()) {
            card.insert("goal".into(), json!(goal));
        }
        let total = snapshot.and_then(|s| s.total).filter(|t| *t > 0);
        let done = total.map(|t| t.min(snapshot.and_then(|s| s.completed).unwrap_or(0)));
        if let (Some(total), Some(done)) = (total, done) {
            let (done, total) = (done.to_string(), total.to_string());
            card.insert("progress".into(), json!(l.get_with("graph.ouroboros.progress", &[("completed", &done), ("total", &total)])));
        }
        let mut counts = vec![];
        if let Some(s) = snapshot.filter(|s| s.source == Source::Live) {
            for (name, value) in [("pending", s.pending), ("executing", s.executing), ("completed", s.completed), ("failed", s.failed)] {
                let n = value.unwrap_or(0).to_string();
                counts.push(l.get_with(&format!("graph.ouroboros.count.{name}"), &[("n", &n)]));
            }
        }
        card.insert("counts".into(), json!(counts));
        let line = [snapshot.and_then(|s| s.phase.as_deref()), snapshot.and_then(|s| s.activity.as_deref())]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · ");
        if !line.is_empty() {
            card.insert("line".into(), json!(line));
        }
        let note = match snapshot.map(|s| s.source) {
            None => Some("graph.ouroboros.waiting"),
            Some(Source::Unreachable) => Some("graph.ouroboros.unreachable"),
            Some(Source::Unreadable) => Some("graph.ouroboros.unreadable"),
            Some(Source::NoExecution) => Some("graph.ouroboros.noExecution"),
            Some(Source::Live) => None,
        };
        if let Some(note) = note {
            card.insert("note".into(), json!(l.get(note)));
        }
        let footer = match &link.execution_id {
            Some(execution) => l.get_with("graph.ouroboros.executionID", &[("id", execution)]),
            None => l.get_with("graph.ouroboros.jobID", &[("id", link.job_id.as_deref().unwrap_or(""))]),
        };
        let failed = self.open_failed.contains(key);
        card.insert("footer".into(), json!(if failed { l.get("graph.ouroboros.openFailed") } else { footer }));
        if let Some(url) = snapshot.and_then(|s| s.dashboard_url.as_ref()).or(link.dashboard_url.as_ref()) {
            card.insert("dashboard".into(), json!(url.absolute()));
        }
        card.insert("key".into(), json!(key));
        card.insert("listOpen".into(), json!(list_open));
        // The AC list while it is open: the first 300, or a line saying it is being read or there is none.
        let items = snapshot.and_then(|s| s.items.as_ref());
        let acs: Option<Vec<Value>> = items.filter(|i| list_open && !i.is_empty()).map(|items| {
            items
                .iter()
                .take(300)
                .map(|item| {
                    let state = item.status.word();
                    json!({"title": item.title, "status": state, "statusWord": l.get(&format!("graph.ouroboros.ac.{state}")), "depth": item.depth})
                })
                .collect()
        });
        if list_open && acs.is_none() {
            let key = if items.is_none() { "graph.ouroboros.loadingACs" } else { "graph.ouroboros.noACs" };
            card.insert("listNote".into(), json!(l.get(key)));
        }
        if let Some(acs) = acs {
            card.insert("acs".into(), Value::Array(acs));
        }
        card.insert("opening".into(), json!(self.opening.contains(key)));
        card.insert("openFailed".into(), json!(failed));
        if let (Some(total), Some(done)) = (total, done) {
            card.insert("done".into(), json!(f64::from(done) / f64::from(total)));
        }
        Value::Object(card)
    }
}

impl ExecutionDashboard for Dashboard {
    fn block_status(&self, key: &str) -> String {
        block_status(self.snapshots.get(key)).to_string()
    }

    fn card(&self, key: &str, list_open: bool) -> Option<Value> {
        let link = self.links.iter().find(|l| l.key() == key)?;
        Some(self.execution_card(key, link, list_open))
    }
}
