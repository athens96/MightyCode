//! A run event as the wire carries it, both ways (C# `RemoteWireEvents`, `RemoteIncoming`, `RemotePendingPermissions`,
//! R\RemoteWireEvents.cs; plan §1.6): a host writes with `from_*`, a client reads with [`read`]; the Mac is the reference for every
//! field (M/Models.swift RunEvent and LogEntry, M/Remote/RemoteWorkspaceModels.swift `RemoteWorkspaceValidation.event`,
//! M/Remote/RemoteClientService.swift `local`). `graph_run` goes only to a Windows client that asks. Owner: W8-D.

use std::collections::HashMap;
use std::sync::Mutex;

use mighty_core::graph::MightyGraphRun;
use mighty_core::json::Wire;
use mighty_core::model::{AgentActivity, BackgroundWork, LogEntry, PlanRecord, SessionUsage, TodoProgress};
use mighty_core::permission::ToolPermissionRequest;
use mighty_core::wire;
use mighty_graph::node::ExecutionGraphNode;
use mighty_run::activity;
use mighty_run::antigravity::permissions::PermissionRequest;
use mighty_run::model::RunEvent;
use serde_json::{Map, Value};
use unicode_segmentation::UnicodeSegmentation;

use crate::remote::validation::EVENT_TYPES;
use crate::remote::wire::{self as remote_wire, BACKGROUND, GRAPH_RUN, PLAN_RECORD, RUN_EVENT, Shape, TODOS, protocol};

/// C# `RemoteIncoming(Event?, Permission?, HostRunId?, Ignored)`: what one wire event is to a Windows client.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Incoming {
    pub event: Option<RunEvent>,
    pub permission: Option<ToolPermissionRequest>,
    /// The host's run id a tool request's answer carries.
    pub host_run_id: Option<String>,
    /// Read but not drawn (a Mac host's graph node, a usage this client cannot use): left out without a word.
    pub ignored: bool,
}

impl Incoming {
    /// `Usable`: an event, a request or an ignored one; an unusable one is left out alone and said once per poll.
    pub fn usable(&self) -> bool {
        self.event.is_some() || self.permission.is_some() || self.ignored
    }

    fn ignored() -> Incoming {
        Incoming { ignored: true, ..Incoming::default() }
    }
}

/// The members of a wire event that hold a time, and whether the Mac writes that one with milliseconds.
const TIMESTAMP_MEMBERS: [(&str, bool); 8] = [
    ("timestamp", false),
    ("updatedAt", false),
    ("rateLimitsUpdatedAt", false),
    ("resetsAt", false),
    ("receivedAt", true),
    ("decidedAt", true),
    ("startedAt", true),
    ("endedAt", true),
];

/// `FromRunEvent(value, jobId, graphRuns)`: a host's event as the wire carries it; None when it does not travel (`graph_run` to a client
/// that did not ask, a tool request or a tracker node, which travel as [`from_permission`] and [`from_graph_node`], a type the Mac does
/// not read, or a value its client would refuse). The caller leaves a None out of the run's events.
pub fn from_run_event(value: &RunEvent, job_id: &str, graph_runs: bool) -> Option<Map<String, Value>> {
    let travels = if value.kind == "graph_run" {
        graph_runs
    } else {
        value.kind != "permission" && value.kind != "graph" && EVENT_TYPES.contains(&value.kind.as_str())
    };
    if !travels {
        return None;
    }
    let sent = RunEvent { session_id: job_id.into(), permission: None, reason: None, ..value.clone() };
    let mut node = serde_json::from_str::<Value>(&sent.to_json()).ok()?.as_object()?.clone();
    // A lost sign-in is the Mac's "auth"; no other reason is one the Mac reads.
    if value.kind == "status" && value.reason.as_deref() == Some("authentication") {
        node.insert("reason".into(), "auth".into());
    }
    if let Some(Value::Object(entry)) = node.get_mut("entry") {
        pictures(entry, false);
    }
    if let Some(Value::Object(run)) = node.get_mut("graphRun") {
        graph_run_pictures(run, false);
        // A task's output file is a path on this computer: it stays here.
        if let Some(Value::Array(agents)) = run.get_mut("agents") {
            for agent in agents.iter_mut().filter_map(Value::as_object_mut) {
                agent.shift_remove("outputFile");
            }
        }
    }
    normalize_object(&mut node);
    readable(node, job_id, graph_runs)
}

/// `FromPermission(value, jobId)`: a tool request of the host's run, as the Mac's "permission" event (`inputJSON`).
pub fn from_permission(value: &ToolPermissionRequest, job_id: &str) -> Option<Map<String, Value>> {
    let sent = ToolPermissionRequest { run_id: job_id.into(), ..value.clone() };
    let mut permission = serde_json::from_str::<Value>(&sent.to_json()).ok()?.as_object()?.clone();
    rename(&mut permission, "inputJson", "inputJSON");
    let mut node = Map::new();
    node.insert("sessionId".into(), job_id.into());
    node.insert("type".into(), "permission".into());
    node.insert("permission".into(), Value::Object(permission));
    normalize_object(&mut node);
    readable(node, job_id, false)
}

/// `FromGraphNode(value, jobId)`: one execution-graph node of the host's run, as the Mac's "graph" event (for a client that did not ask
/// for `graph_run`); None when the Mac's client would not normalize it.
pub fn from_graph_node(value: &ExecutionGraphNode, job_id: &str) -> Option<Map<String, Value>> {
    let clean = mighty_graph::node::normalized(value, false)?;
    let mut graph = serde_json::from_str::<Value>(&clean.to_json()).ok()?.as_object()?.clone();
    if let Some(Value::Array(entries)) = graph.get_mut("entries") {
        for entry in entries.iter_mut().filter_map(Value::as_object_mut) {
            pictures(entry, false);
        }
    }
    // A task's output file is a path on this computer: it stays here.
    graph.shift_remove("outputFile");
    let mut node = Map::new();
    node.insert("sessionId".into(), job_id.into());
    node.insert("type".into(), "graph".into());
    node.insert("graph".into(), Value::Object(graph));
    normalize_object(&mut node);
    Some(node)
}

/// The written event when a client reads it, else None: a member the Mac requires that the writer left out is caught here.
fn readable(node: Map<String, Value>, job_id: &str, graph_runs: bool) -> Option<Map<String, Value>> {
    read(&node, job_id, job_id, graph_runs).usable().then_some(node)
}

/// `Read(wire, jobId, paneId, graphRuns)`: one event of the host's run, as this client's pane would have it, or unusable. Never panics,
/// whatever the object holds: one event never takes the poll down.
pub fn read(wire: &Map<String, Value>, job_id: &str, pane_id: &str, graph_runs: bool) -> Incoming {
    incoming(wire.clone(), job_id, pane_id, graph_runs).unwrap_or_default()
}

fn text(value: Option<&Value>) -> Option<&str> {
    value.and_then(Value::as_str)
}

/// `Activity(node)`: an activity as the Mac's `AgentActivity` decoder reads it (id, provider, kind, state and summary strings; toolName
/// and output strings when present; durationMs `try?`, none when out of range), before any cleaning; None when that decoder would throw.
fn activity_of(node: &Value) -> Option<AgentActivity> {
    let o = node.as_object()?;
    let optional = |name: &str| match o.get(name) {
        None | Some(Value::Null) => Some(None),
        Some(Value::String(s)) => Some(Some(s.clone())),
        Some(_) => None,
    };
    Some(AgentActivity {
        id: text(o.get("id"))?.into(),
        provider: text(o.get("provider"))?.into(),
        kind: text(o.get("kind"))?.into(),
        state: text(o.get("state"))?.into(),
        summary: text(o.get("summary"))?.into(),
        tool_name: optional("toolName")?,
        output: optional("output")?,
        duration_ms: o.get("durationMs").and_then(Value::as_f64).filter(|ms| activity::valid_duration(*ms)),
    })
}

/// The Mac's `ActivitySupport.valid`: normalizing leaves it as it is.
fn valid_activity(value: &AgentActivity) -> bool {
    activity::normalized(value, false).as_ref() == Some(value)
}

/// `Usage(o)`: a usage as the Mac's synthesized `SessionUsage` decoder reads it (mighty-core's reader is looser, so these rules first).
fn usage_shape(o: &Map<String, Value>) -> bool {
    let is_text = |v: Option<&Value>| v.is_some_and(Value::is_string);
    let is_number = |v: Option<&Value>| v.is_some_and(Value::is_number);
    let is_whole = |v: &Value| v.as_f64().is_some_and(|n| n.floor() == n && n.abs() <= 9.2e18);
    let absent = |v: Option<&Value>| v.is_none_or(Value::is_null);
    ["provider", "source", "tokenScope", "updatedAt"].iter().all(|k| is_text(o.get(*k)))
        && ["model", "selectedModel", "providerSessionId", "costScope", "rateLimitsUpdatedAt"]
            .iter()
            .all(|k| absent(o.get(*k)) || is_text(o.get(*k)))
        && [
            "inputTokens",
            "outputTokens",
            "cacheReadTokens",
            "cacheWriteTokens",
            "reasoningTokens",
            "totalTokens",
            "contextUsedTokens",
            "contextWindowTokens",
        ]
        .iter()
        .all(|k| absent(o.get(*k)) || o.get(*k).is_some_and(|v| v.is_number() && is_whole(v)))
        && (absent(o.get("costUSD")) || is_number(o.get("costUSD")))
        && match o.get("rateLimits") {
            None | Some(Value::Null) => true,
            Some(Value::Array(windows)) => windows.iter().all(|w| {
                w.as_object().is_some_and(|w| {
                    is_text(w.get("kind"))
                        && (absent(w.get("percentUsed")) || is_number(w.get("percentUsed")))
                        && (absent(w.get("resetsAt")) || is_text(w.get("resetsAt")))
                })
            }),
            Some(_) => false,
        }
}

/// `Lenient<T>(value, key, shape)`: a member the Mac decodes with `try?`: one that does not decode is removed, as if never sent.
fn lenient(value: &mut Map<String, Value>, key: &str, decodes: impl FnOnce(&Value) -> bool) {
    let Some(member) = value.get(key) else { return };
    if member.is_null() {
        return;
    }
    if !decodes(member) {
        value.shift_remove(key);
    }
}

fn shaped<T: Wire>(member: &Value, members: &'static [remote_wire::Member]) -> bool {
    remote_wire::fits(member, Shape::Record(members)) && T::read(&remote_wire::exact(member, Shape::Record(members))).is_ok()
}

fn incoming(mut copy: Map<String, Value>, job_id: &str, pane_id: &str, graph_runs: bool) -> Option<Incoming> {
    // The Mac's RunEvent decoder: sessionId, type, entry, status, resumeId, activity and permission must decode, or the event is refused;
    // usage, graph, reason, plan, todos and background that do not decode are read as absent (try?), which refuses an event of that type.
    lenient(&mut copy, "usage", |v| v.as_object().is_some_and(usage_shape) && SessionUsage::from_json(v).is_some());
    lenient(&mut copy, "reason", Value::is_string);
    lenient(&mut copy, "plan", |v| shaped::<PlanRecord>(v, PLAN_RECORD));
    lenient(&mut copy, "todos", |v| shaped::<TodoProgress>(v, TODOS));
    // BackgroundWork.turnEnded and BackgroundTask.status are defaulted on Windows but required by the Mac's synthesized decoder.
    lenient(&mut copy, "background", |v| {
        shaped::<BackgroundWork>(v, BACKGROUND)
            && v.get("turnEnded").is_some_and(|t| !t.is_null())
            && v.get("tasks").and_then(Value::as_array).is_some_and(|tasks| {
                tasks.iter().all(|t| t.as_object().is_none_or(|task| task.get("status").is_some_and(|s| !s.is_null())))
            })
    });
    lenient(&mut copy, "graphRun", |v| shaped::<MightyGraphRun>(v, GRAPH_RUN));
    let is_log = copy.get("type").and_then(Value::as_str) == Some("log");
    if let Some(Value::Object(entry)) = copy.get_mut("entry") {
        // The Mac reads a damaged activity or picture list of an entry as none (try?). A log whose entry activity is not valid is
        // refused; on any other event that activity is only left out.
        pictures(entry, true);
        if let Some(inner) = entry.get("activity").filter(|a| !a.is_null()) {
            let parsed = activity_of(inner);
            if !parsed.as_ref().is_some_and(valid_activity) {
                if parsed.is_some() && is_log {
                    return None;
                }
                entry.shift_remove("activity");
            }
        }
    }
    if let Some(outer) = copy.get("activity").filter(|a| !a.is_null())
        && !activity_of(outer).as_ref().is_some_and(valid_activity)
    {
        return None;
    }
    if let Some(Value::Object(request)) = copy.get_mut("permission") {
        // Windows names it InputJson; ToolPermissionRequest.state and canAllow are defaulted on Windows but required by the Mac's decoder.
        request.shift_remove("inputJson");
        rename(request, "inputJSON", "inputJson");
        if request.get("state").is_none_or(Value::is_null) || request.get("canAllow").is_none_or(Value::is_null) {
            return None;
        }
    }
    if let Some(Value::Object(run)) = copy.get_mut("graphRun") {
        graph_run_pictures(run, true);
    }
    let checked = remote_wire::checked_value(&Value::Object(copy), RUN_EVENT).ok()?;
    let value = run_event(checked.as_object()?)?;
    if value.session_id != job_id {
        return None;
    }
    let kind = value.kind.clone();
    // A usage the Mac's decoder cannot read, or its client cannot normalize, is left out without a word.
    if kind == "usage" && value.usage.is_none() {
        return Some(Incoming::ignored());
    }
    // A Mac host's tracker node: a Windows pane draws graph_run instead.
    if kind == "graph" {
        return Some(Incoming::ignored());
    }
    if !(EVENT_TYPES.contains(&kind.as_str()) || kind == "graph_run" && graph_runs) {
        return None;
    }
    match kind.as_str() {
        "status" if value.reason.as_deref().is_some_and(|r| r != "auth") => return None,
        "permission" => {
            let permission = value.permission?;
            let characters = |s: &str| s.graphemes(true).count();
            if !wire::identifier(&permission.run_id)
                || permission.id.is_empty()
                || characters(&permission.id) > 256
                || characters(&permission.tool_name) > 256
                || permission.input_json.len() > mighty_run::permissions::MAXIMUM_INPUT_BYTES
                || permission.summary.len() > 16_384
                || permission.state.is_empty()
                || characters(&permission.state) > 32
            {
                return None;
            }
            let host = permission.run_id.clone();
            return Some(Incoming {
                permission: Some(ToolPermissionRequest { run_id: pane_id.into(), ..permission }),
                host_run_id: Some(host),
                ..Incoming::default()
            });
        }
        "log" => {
            let entry = value.entry.as_ref()?;
            if entry.provider.as_deref().is_some_and(|p| !wire::PROVIDERS.contains(&p))
                || entry.text.as_deref().unwrap_or_default().len() > activity::MAXIMUM_MESSAGE_BYTES
            {
                return None;
            }
        }
        _ => {}
    }
    let auth = kind == "status" && value.reason.as_deref() == Some("auth");
    let mut value = RunEvent { session_id: pane_id.into(), permission: None, reason: auth.then(|| "authentication".to_string()), ..value };
    if let Some(entry) = &mut value.entry {
        let limit = if entry.kind() == "assistant" { activity::MAXIMUM_MESSAGE_BYTES } else { 32768 };
        entry.text = Some(activity::clean(entry.text.as_deref(), limit, false));
    }
    value.valid().then(|| Incoming { event: Some(value), ..Incoming::default() })
}

/// The run event of a value checked against [`RUN_EVENT`], each member read by mighty-core's readers; None when one refuses it.
fn run_event(o: &Map<String, Value>) -> Option<RunEvent> {
    fn member<T: Wire>(o: &Map<String, Value>, name: &str) -> Option<Option<T>> {
        match o.get(name) {
            None | Some(Value::Null) => Some(None),
            Some(v) => T::read(v).ok().map(Some),
        }
    }
    let permission = match o.get("permission") {
        Some(Value::Object(p)) => {
            let text = |name: &str| p.get(name).and_then(Value::as_str).unwrap_or_default().to_string();
            let opt = |name: &str| p.get(name).and_then(Value::as_str).map(str::to_string);
            let flag = |name: &str, default: bool| p.get(name).and_then(Value::as_bool).unwrap_or(default);
            Some(ToolPermissionRequest {
                id: text("id"),
                run_id: text("runId"),
                tool_use_id: text("toolUseId"),
                tool_name: text("toolName"),
                input_json: text("inputJson"),
                summary: text("summary"),
                reason: opt("reason"),
                blocked_path: opt("blockedPath"),
                state: opt("state").unwrap_or_else(|| "pending".into()),
                can_allow: flag("canAllow", true),
                can_answer_questions: flag("canAnswerQuestions", false),
                can_answer_plan: flag("canAnswerPlan", false),
                received_at: opt("receivedAt"),
            })
        }
        _ => None,
    };
    let entry: Option<LogEntry> = member(o, "entry")?;
    Some(RunEvent {
        session_id: o.get("sessionId")?.as_str()?.into(),
        kind: o.get("type")?.as_str()?.into(),
        entry,
        status: o.get("status").and_then(Value::as_str).map(str::to_string),
        resume_id: o.get("resumeId").and_then(Value::as_str).map(str::to_string),
        // AgentActivityConverter normalizes what it reads and loads anything invalid as none.
        activity: o.get("activity").filter(|a| !a.is_null()).and_then(AgentActivity::from_json),
        usage: o.get("usage").filter(|u| !u.is_null()).and_then(SessionUsage::from_json),
        permission,
        graph_run: member(o, "graphRun")?,
        reason: o.get("reason").and_then(Value::as_str).map(str::to_string),
        plan: member(o, "plan")?,
        todos: member(o, "todos")?,
        background: member(o, "background")?,
        antigravity_permission: member::<PermissionRequest>(o, "antigravityPermission")?,
    })
}

/// `MacTimestamp(value, milliseconds)`: the time in the form the Mac writes it, in UTC (`yyyy-MM-ddTHH:mm:ss.fffZ`, or whole seconds);
/// None when it is not an ISO 8601 time. Windows writes "O" (seven digits and an offset), which the Mac's reader does not take.
pub fn mac_timestamp(value: &str, milliseconds: bool) -> Option<String> {
    let b = value.as_bytes();
    let digits = |range: std::ops::Range<usize>| -> Option<i64> {
        let part = value.get(range)?;
        (!part.is_empty() && part.bytes().all(|c| c.is_ascii_digit())).then(|| part.parse().ok()).flatten()
    };
    if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let (year, month, day) = (digits(0..4)?, digits(5..7)?, digits(8..10)?);
    let (hour, minute, second) = (digits(11..13)?, digits(14..16)?, digits(17..19)?);
    let mut rest = &value[19..];
    let mut ticks = 0i64;
    if let Some(after) = rest.strip_prefix('.') {
        let count = after.bytes().take_while(u8::is_ascii_digit).count();
        if count == 0 {
            return None;
        }
        let fraction = &after[..count];
        let mut seven = fraction.chars().take(7).collect::<String>();
        while seven.len() < 7 {
            seven.push('0');
        }
        ticks = seven.parse().ok()?;
        rest = &after[count..];
    }
    let offset_minutes = match rest {
        "Z" => 0,
        _ => {
            let r = rest.as_bytes();
            if r.len() != 6 || !matches!(r[0], b'+' | b'-') || r[3] != b':' {
                return None;
            }
            let hours: i64 = rest.get(1..3).filter(|h| h.bytes().all(|c| c.is_ascii_digit()))?.parse().ok()?;
            let minutes: i64 = rest.get(4..6).filter(|m| m.bytes().all(|c| c.is_ascii_digit()))?.parse().ok()?;
            if hours > 14 || minutes > 59 {
                return None;
            }
            let total = hours * 60 + minutes;
            if r[0] == b'-' { -total } else { total }
        }
    };
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days_in = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    if !(1..=12).contains(&month) || day < 1 || day > days_in[(month - 1) as usize] || hour > 23 || minute > 59 || second > 59 || year < 1 {
        return None;
    }
    let days = days_from_civil(year, month, day);
    let seconds = days * 86_400 + hour * 3600 + minute * 60 + second - offset_minutes * 60;
    let (day_count, in_day) = (seconds.div_euclid(86_400), seconds.rem_euclid(86_400));
    let (y, m, d) = civil_from_days(day_count);
    let (h, mi, s) = (in_day / 3600, in_day / 60 % 60, in_day % 60);
    Some(if milliseconds {
        format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}.{:03}Z", ticks / 10_000)
    } else {
        format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
    })
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// Whether a time is already in one of the two forms the Mac writes (whole seconds or three fraction digits, `Z`).
fn mac_form(value: &str) -> bool {
    let b = value.as_bytes();
    let shape = |len: usize| {
        b.len() == len
            && b.iter().enumerate().all(|(i, c)| match i {
                4 | 7 => *c == b'-',
                10 => *c == b'T',
                13 | 16 => *c == b':',
                19 if len == 24 => *c == b'.',
                _ if i == len - 1 => *c == b'Z',
                _ => c.is_ascii_digit(),
            })
    };
    shape(20) || shape(24)
}

/// `Normalize(node)`: every time member of a wire event in a form the Mac reads; one already in either of the Mac's forms is kept exactly,
/// anything else that is a time is rewritten by [`mac_timestamp`]. Only those members are touched: a time inside text never is.
pub fn normalize(node: &mut Value) {
    match node {
        Value::Object(o) => normalize_object(o),
        Value::Array(items) => items.iter_mut().for_each(normalize),
        _ => {}
    }
}

fn normalize_object(o: &mut Map<String, Value>) {
    for (key, value) in o.iter_mut() {
        if let (Some((_, milliseconds)), Value::String(s)) = (TIMESTAMP_MEMBERS.iter().find(|(name, _)| name == key), &*value) {
            if !mac_form(s)
                && let Some(t) = mac_timestamp(s, *milliseconds)
            {
                *value = Value::String(t);
            }
        } else {
            normalize(value);
        }
    }
}

/// `Pictures(entry, reading)`: a picture's cache key names a file on the host, so pictures never cross; a client reads a picture entry
/// as its text alone.
fn pictures(entry: &mut Map<String, Value>, reading: bool) {
    entry.shift_remove("images");
    if reading && entry.get("kind").and_then(Value::as_str) == Some("image") {
        entry.insert("kind".into(), "system".into());
    }
}

fn graph_run_pictures(run: &mut Map<String, Value>, reading: bool) {
    for key in ["rootEntries", "resultEntries"] {
        if let Some(Value::Array(entries)) = run.get_mut(key) {
            entries.iter_mut().filter_map(Value::as_object_mut).for_each(|e| pictures(e, reading));
        }
    }
    if let Some(Value::Array(agents)) = run.get_mut("agents") {
        for agent in agents.iter_mut().filter_map(Value::as_object_mut) {
            if let Some(Value::Array(entries)) = agent.get_mut("entries") {
                entries.iter_mut().filter_map(Value::as_object_mut).for_each(|e| pictures(e, reading));
            }
        }
    }
}

fn rename(value: &mut Map<String, Value>, from: &str, to: &str) {
    if let Some(member) = value.shift_remove(from) {
        value.insert(to.into(), member);
    }
}

/// C# `RemotePendingPermissions`: a remote pane keeps only the newest 32 waiting tool requests (M/RemotePendingPermissions); older ones
/// are withdrawn as "cancelled" changes, so the pane's cards go too.
#[derive(Default)]
pub struct PendingPermissions {
    pending: Mutex<HashMap<String, Vec<ToolPermissionRequest>>>,
}

impl PendingPermissions {
    /// `Admit(request)`: the requests this one pushed out, each already marked cancelled.
    pub fn admit(&self, request: &ToolPermissionRequest) -> Vec<ToolPermissionRequest> {
        let mut pending = self.pending.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let list = pending.entry(request.run_id.clone()).or_default();
        list.retain(|r| r.id != request.id);
        if request.state != "pending" {
            return vec![];
        }
        list.push(request.clone());
        let mut evicted = vec![];
        while list.len() > protocol::MAXIMUM_PENDING_PERMISSIONS {
            let mut old = list.remove(0);
            old.state = "cancelled".into();
            evicted.push(old);
        }
        evicted
    }

    pub fn count(&self, pane_id: &str) -> usize {
        self.pending.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(pane_id).map_or(0, Vec::len)
    }

    pub fn forget(&self, pane_id: &str) {
        self.pending.lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(pane_id);
    }
}
