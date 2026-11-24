//! The rules both sides of the remote mirror read (C# `RemoteMirrorSupport`, M/MightyCore/Remote/RemoteMirror.swift
//! `RemoteMirrorSupport`): the client's ids for a host's workspaces and panes, a pane's and the list's revisions, the host pane as the wire
//! carries it (bounded as the phone's payload and fitted under 1.5 MiB), the Mac's session JSON both ways, the client's checks of a list
//! and a detail, and the client pane laid over from the host's summary and detail. Pure: no I/O, no clock but the one passed in.

use mighty_core::graph::MightyGraphRun;
use mighty_core::json::Wire;
use mighty_core::model::{
    AgentActivity, AgentRunTiming, BackgroundWork, LogEntry, MirrorReference, RunSession, RunSettings, STORED_KINDS, TodoProgress,
};
use mighty_core::permission::ToolPermissionRequest;
use mighty_core::time::{self, DateTimeOffset};
use mighty_core::wire;
use serde_json::{Map, Value};
use sha2::{Digest as _, Sha256};

use super::{
    MirrorQueued, MirrorSession, MirrorSummary, QUEUED, REMOTE_VERSION, WireMirrorDetail, WireMirrorState, known_provider, protocol,
};
use crate::remote::events;
use crate::remote::text;
use crate::remote::validation;
use crate::remote::wire::{BACKGROUND, GRAPH_RUN, LOG_ENTRY, Member, Need, RemoteSharedWorkspace, Shape, TODOS, TOOL_REQUEST};
use crate::remote::wire::{exact, fits, protocol as remote_protocol, written};

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `ClientId(connectionId, hostId)`: a stable id on the client for a host's workspace or pane: `mirror-` and 32 hex digits of SHA-256
/// over the connection and the host's id (joined by NUL), so two connections never collide and the same host pane keeps one client id
/// while connected. The Mac's exactly.
pub fn client_id(connection_id: &str, host_id: &str) -> String {
    let hash = Sha256::digest(format!("{connection_id}\0{host_id}").as_bytes());
    format!("mirror-{}", hex(&hash[..16]))
}

/// `Digest(parts)`: SHA-256 of the parts joined by U+0001, its first 12 bytes in hex (24 digits).
pub(crate) fn digest<I: IntoIterator<Item = String>>(parts: I) -> String {
    let joined = parts.into_iter().collect::<Vec<_>>().join("\u{1}");
    hex(&Sha256::digest(joined.as_bytes())[..12])
}

/// `ToUnixTimeMilliseconds()`.
fn unix_ms(at: DateTimeOffset) -> i64 {
    (at.utc_ticks() - 621_355_968_000_000_000).div_euclid(10_000)
}

/// `Revision(session, permissions, queued, branch, baseBranch)`: a pane's revision, everything its detail carries that can change, read
/// cheaply (the transcript's tail, not all of it; the graph's block identities and states, not their streamed text). Equal in the same
/// process for the same pane; a client only compares it.
pub fn revision(
    session: &RunSession,
    permissions: &[ToolPermissionRequest],
    queued: &[MirrorQueued],
    branch: Option<&str>,
    base_branch: Option<&str>,
) -> String {
    let settings = session.settings.clone().unwrap_or_default();
    let mut parts = vec![
        session.status().to_string(),
        session.title().to_string(),
        session.provider().to_string(),
        session.model().to_string(),
        settings.permission_mode.clone(),
        settings.effort.clone(),
        session.agent_view_mode.clone().unwrap_or_default(),
        session.mighty_style.clone().unwrap_or_default(),
        session.logs().len().to_string(),
        branch.unwrap_or_default().to_string(),
        base_branch.unwrap_or_default().to_string(),
        session.title_mode.clone().unwrap_or_default(),
        settings.to_json(),
    ];
    let logs = session.logs();
    for entry in &logs[logs.len().saturating_sub(12)..] {
        let state = entry.activity.as_ref().map(|a| a.state.as_str()).unwrap_or_default();
        parts.push(format!("{}:{}:{state}", entry.id(), entry.text().len()));
    }
    parts.push(mighty_graph::view::mobile::digest(session).to_string());
    if let Some(todos) = &session.todo_progress {
        for item in todos.items.iter().flatten() {
            let key = item.id.clone().or_else(|| item.content.clone()).unwrap_or_default();
            parts.push(format!("{key}:{}", item.status.as_deref().unwrap_or_default()));
        }
    }
    if let Some(work) = &session.background_work {
        for task in work.tasks.iter().flatten() {
            parts.push(format!("{}:{}", task.id.as_deref().unwrap_or_default(), task.status.as_deref().unwrap_or_default()));
        }
        parts.push(if work.turn_ended { "ended".into() } else { String::new() });
    }
    parts.push(match &session.run_timing {
        Some(timing) => format!("{}:{}", unix_ms(timing.started_at), timing.finished_at.map(unix_ms).unwrap_or(0)),
        None => String::new(),
    });
    parts.extend(permissions.iter().map(|p| format!("{}:{}:{}", p.id, p.run_id, p.state)));
    // MC-49: what 바로 실행 does on a row moves the revision too (a row the host has no button for reads as before).
    parts.extend(queued.iter().map(|q| match &q.run_now {
        Some(run_now) => format!("{}:{}:{run_now}", q.id, q.text.encode_utf16().count()),
        None => format!("{}:{}", q.id, q.text.encode_utf16().count()),
    }));
    digest(parts)
}

/// `StateRevision(workspaces, sessions)`: the list's revision, its workspaces and every pane's id and revision, in order (the Mac's
/// materials).
pub fn state_revision(workspaces: &[RemoteSharedWorkspace], sessions: &[MirrorSummary]) -> String {
    digest(
        workspaces
            .iter()
            .map(|w| format!("{}:{}", w.id, w.name))
            .chain(sessions.iter().map(|s| format!("{}:{}:{}:{}:{}:{}", s.id, s.revision, s.title, s.status, s.pending, s.queued))),
    )
}

/// The status a pane travels with: one of [`protocol::STATUSES`], else `idle`.
pub(crate) fn wire_status(status: &str) -> String {
    if protocol::STATUSES.contains(&status) { status.to_string() } else { "idle".into() }
}

/// The time as the Mac's ISO8601DateFormatter writes it (whole seconds, UTC), the only form its `startedAt` reader takes.
fn whole(at: DateTimeOffset) -> String {
    at.to_chrono().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// `Session(session, permissions, queued, branch, baseBranch)`: the host pane as the wire carries it, bounded like the phone's payload and
/// under [`protocol::MAXIMUM_DETAIL_BYTES`].
pub fn session(
    pane: &RunSession,
    permissions: &[ToolPermissionRequest],
    queued: &[MirrorQueued],
    branch: Option<&str>,
    base_branch: Option<&str>,
) -> MirrorSession {
    let all = pane.logs();
    let logs = crate::phone::mighty::phone_entries(&all[all.len().saturating_sub(protocol::MAXIMUM_ENTRIES)..]);
    let runs = pane.graph_runs();
    let runs = runs[runs.len().saturating_sub(protocol::MAXIMUM_GRAPH_RUNS)..].to_vec();
    let waiting: Vec<ToolPermissionRequest> = permissions.iter().filter(|p| p.state == "pending").cloned().collect();
    let created = pane.created_at().unwrap_or_default();
    let value = MirrorSession {
        id: pane.id().into(),
        workspace_id: pane.workspace_id().into(),
        title: text::cut(pane.title(), 200),
        kind: pane.kind().into(),
        provider: pane.provider().into(),
        model: text::cut(pane.model(), 200),
        status: wire_status(pane.status()),
        created_at: events::mac_timestamp(created, false).unwrap_or_else(|| created.to_string()),
        settings: pane.settings.clone().unwrap_or_default(),
        has_older: all.len() > logs.len(),
        logs,
        graph_runs: runs,
        permissions: waiting[waiting.len().saturating_sub(protocol::MAXIMUM_PERMISSIONS)..].to_vec(),
        queued: queued
            .iter()
            .take(protocol::MAXIMUM_QUEUED)
            .map(|q| MirrorQueued {
                id: q.id.clone(),
                text: text::cut(&q.text, protocol::MAXIMUM_QUEUED_TEXT),
                run_now: q.run_now.clone().filter(|r| protocol::RUN_NOW.contains(&r.as_str())),
            })
            .collect(),
        title_mode: pane.title_mode.clone(),
        agent_view_mode: pane.agent_view_mode.clone(),
        mighty_style: pane.mighty_style.clone(),
        todo_progress: pane.todo_progress.clone(),
        background_work: pane.background_work.clone(),
        started_at: pane.run_timing.as_ref().map(|t| whole(t.started_at)),
        finished_at: pane.run_timing.as_ref().and_then(|t| t.finished_at).map(whole),
        branch: branch.map(str::to_string),
        base: base_branch.map(str::to_string),
        queue_actions: false,
    };
    fitted(value, protocol::MAXIMUM_DETAIL_BYTES)
}

/// The JSON size of a session as the wire writes it.
fn size(session: &MirrorSession) -> usize {
    serde_json::to_vec(&Value::Object(write(session))).map_or(usize::MAX, |bytes| bytes.len())
}

/// `Fitted(value, budget)`: `value` cut until its JSON is at most `budget` bytes: the oldest graph runs first (keeping one), then the
/// oldest entries (keeping one, and `has_older` says so), then each remaining text to 32 KiB, and last the graph emptied.
pub fn fitted(value: MirrorSession, budget: usize) -> MirrorSession {
    const CUT: usize = 32 * 1024;
    let mut session = value;
    while size(&session) > budget {
        if session.graph_runs.len() > 1 {
            session.graph_runs.remove(0);
            continue;
        }
        if session.logs.len() > 1 {
            session.logs.remove(0);
            session.has_older = true;
            continue;
        }
        let mut changed = false;
        if session.logs.iter().any(|l| l.text().len() > CUT) {
            for entry in &mut session.logs {
                if entry.text().len() > CUT {
                    entry.text = Some(utf8_prefix(entry.text(), CUT).to_string());
                }
            }
            changed = true;
        }
        if !session.graph_runs.is_empty() && size(&session) > budget {
            session.graph_runs.clear();
            changed = true;
        }
        if !changed {
            break;
        }
    }
    session
}

/// `Utf8Prefix(text, bytes)`: the longest start of `text` of at most `bytes` UTF-8 bytes, never part of a character.
pub fn utf8_prefix(text: &str, bytes: usize) -> &str {
    if text.len() <= bytes {
        return text;
    }
    let mut end = bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// `PhoneEntry(entry)`: an entry without its pictures, an `image` entry as its `system` line.
fn phone_entry(entry: &mut Map<String, Value>) {
    entry.shift_remove("images");
    if entry.get("kind").and_then(Value::as_str) == Some("image") {
        entry.insert("kind".into(), "system".into());
    }
}

fn rename(value: &mut Map<String, Value>, from: &str, to: &str) {
    if let Some(member) = value.shift_remove(from) {
        value.insert(to.into(), member);
    }
}

fn objects(value: Option<&mut Value>) -> impl Iterator<Item = &mut Map<String, Value>> {
    value.and_then(Value::as_array_mut).into_iter().flatten().filter_map(Value::as_object_mut)
}

fn entries_of_run(run: &mut Map<String, Value>, also: impl Fn(&mut Map<String, Value>)) {
    for key in ["rootEntries", "resultEntries"] {
        objects(run.get_mut(key)).for_each(phone_entry);
    }
    for agent in objects(run.get_mut("agents")) {
        also(agent);
        objects(agent.get_mut("entries")).for_each(phone_entry);
    }
}

fn put(o: &mut Map<String, Value>, name: &str, value: Option<Value>) {
    if let Some(value) = value {
        o.insert(name.into(), value);
    }
}

/// A graph run as `RemoteWire.Json` writes it, without its agents' output files (paths on the host).
fn run_value(run: &MightyGraphRun) -> Value {
    let mut out = mighty_core::json::JsonOut::new();
    run.write_with(&mut out, false);
    serde_json::from_str(&out.finish()).unwrap_or(Value::Null)
}

/// `Write(session)`: the session as the Mac's JSON: every entry (the transcript's and the graph's) without pictures, an `image` entry as
/// its `system` line; no task's output file; a request's input as `inputJSON`; the times inside entries, runs, requests and work in the
/// forms the Mac writes. The session's own clock is whole seconds already ([`session`]).
pub fn write(session: &MirrorSession) -> Map<String, Value> {
    let mut o = Map::new();
    o.insert("id".into(), session.id.clone().into());
    o.insert("workspaceId".into(), session.workspace_id.clone().into());
    o.insert("title".into(), session.title.clone().into());
    o.insert("kind".into(), session.kind.clone().into());
    o.insert("provider".into(), session.provider.clone().into());
    o.insert("model".into(), session.model.clone().into());
    o.insert("status".into(), session.status.clone().into());
    o.insert("createdAt".into(), session.created_at.clone().into());
    o.insert("settings".into(), written(&session.settings));
    o.insert("logs".into(), Value::Array(session.logs.iter().map(written).collect()));
    o.insert("hasOlder".into(), session.has_older.into());
    o.insert("graphRuns".into(), Value::Array(session.graph_runs.iter().map(run_value).collect()));
    o.insert(
        "permissions".into(),
        Value::Array(session.permissions.iter().map(|p| serde_json::from_str(&p.to_json()).unwrap_or(Value::Null)).collect()),
    );
    o.insert(
        "queued".into(),
        Value::Array(
            session
                .queued
                .iter()
                .map(|q| {
                    let mut item = Map::new();
                    item.insert("id".into(), q.id.clone().into());
                    item.insert("text".into(), q.text.clone().into());
                    put(&mut item, "runNow", q.run_now.clone().map(Value::String));
                    Value::Object(item)
                })
                .collect(),
        ),
    );
    put(&mut o, "titleMode", session.title_mode.clone().map(Value::String));
    put(&mut o, "agentViewMode", session.agent_view_mode.clone().map(Value::String));
    put(&mut o, "mightyStyle", session.mighty_style.clone().map(Value::String));
    put(&mut o, "todoProgress", session.todo_progress.as_ref().map(written));
    put(&mut o, "backgroundWork", session.background_work.as_ref().map(written));
    put(&mut o, "startedAt", session.started_at.clone().map(Value::String));
    put(&mut o, "finishedAt", session.finished_at.clone().map(Value::String));
    put(&mut o, "branch", session.branch.clone().map(Value::String));
    put(&mut o, "base", session.base.clone().map(Value::String));
    put(&mut o, "queueActions", session.queue_actions.then_some(Value::Bool(true)));
    objects(o.get_mut("logs")).for_each(phone_entry);
    for run in objects(o.get_mut("graphRuns")) {
        entries_of_run(run, |agent| {
            agent.shift_remove("outputFile");
        });
    }
    for permission in objects(o.get_mut("permissions")) {
        rename(permission, "inputJson", "inputJSON");
    }
    for key in ["logs", "graphRuns", "permissions", "todoProgress", "backgroundWork"] {
        if let Some(member) = o.get_mut(key) {
            events::normalize(member);
        }
    }
    o
}

use Need::{Nullable, Required};
use Shape::{Flag, List, Object, Record, Text};

/// The session as the Mac's decoder requires it (after [`read`]'s lenient members are taken out).
const SESSION: &[Member] = &[
    Member("id", Text, Required),
    Member("workspaceId", Text, Required),
    Member("title", Text, Required),
    Member("kind", Text, Required),
    Member("provider", Text, Required),
    Member("model", Text, Required),
    Member("status", Text, Required),
    Member("createdAt", Text, Required),
    Member("settings", Object, Required),
    Member("logs", List(&Record(LOG_ENTRY)), Required),
    Member("hasOlder", Flag, Required),
    Member("graphRuns", List(&Object), Required),
    Member("permissions", List(&Record(TOOL_REQUEST)), Required),
    Member("queued", List(&Record(QUEUED)), Required),
    Member("titleMode", Text, Nullable),
    Member("agentViewMode", Text, Nullable),
    Member("mightyStyle", Text, Nullable),
    Member("todoProgress", Object, Nullable),
    Member("backgroundWork", Object, Nullable),
    Member("startedAt", Text, Nullable),
    Member("finishedAt", Text, Nullable),
    Member("branch", Text, Nullable),
    Member("base", Text, Nullable),
    Member("queueActions", Flag, Nullable),
];

fn decodes<T: Wire>(value: &Value, members: &'static [Member]) -> bool {
    fits(value, Record(members)) && T::read(&exact(value, Record(members))).is_ok()
}

/// `Lenient<T>(value, key)`: a member a Windows pane can do without, removed when it does not read here.
fn lenient(value: &mut Map<String, Value>, key: &str, reads: impl FnOnce(&Value) -> bool) {
    if let Some(member) = value.get(key)
        && !member.is_null()
        && !reads(member)
    {
        value.shift_remove(key);
    }
}

fn graph_runs_read(runs: &Value) -> Option<Vec<MightyGraphRun>> {
    let list = runs.as_array()?;
    list.iter()
        .map(|run| {
            if run.is_null() || !fits(run, Record(GRAPH_RUN)) {
                return None;
            }
            MightyGraphRun::read(run).ok()
        })
        .collect()
}

/// A tool request as the Mac's decoder takes it (`inputJson` here), with Windows' defaults for the members it may leave out.
fn permission_read(p: &Map<String, Value>) -> ToolPermissionRequest {
    let text = |name: &str| p.get(name).and_then(Value::as_str).unwrap_or_default().to_string();
    let opt = |name: &str| p.get(name).and_then(Value::as_str).map(str::to_string);
    let flag = |name: &str, default: bool| p.get(name).and_then(Value::as_bool).unwrap_or(default);
    ToolPermissionRequest {
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
    }
}

/// `Read(wire)`: a host's session JSON (a Mac's or a Windows app's) in this app's types; None when it does not read. As the remote run's
/// reader, a member a Windows pane can do without is left out rather than the whole pane refused: a graph, a checklist or background work
/// that does not read here, and an entry's activity that does not.
pub fn read(wire: &Map<String, Value>) -> Option<MirrorSession> {
    let mut node = wire.clone();
    for permission in objects(node.get_mut("permissions")) {
        permission.shift_remove("inputJson");
        rename(permission, "inputJSON", "inputJson");
    }
    for entry in objects(node.get_mut("logs")) {
        phone_entry(entry);
        lenient(entry, "activity", |a| AgentActivity::from_json(a).is_some());
    }
    if node.get("graphRuns").is_some_and(Value::is_array) {
        for run in objects(node.get_mut("graphRuns")) {
            entries_of_run(run, |_| {});
        }
        if node.get("graphRuns").and_then(graph_runs_read).is_none() {
            node.insert("graphRuns".into(), Value::Array(vec![]));
        }
    }
    lenient(&mut node, "todoProgress", |v| decodes::<TodoProgress>(v, TODOS));
    lenient(&mut node, "backgroundWork", |v| decodes::<BackgroundWork>(v, BACKGROUND));
    let value = Value::Object(node);
    if !fits(&value, Record(SESSION)) {
        return None;
    }
    let o = exact(&value, Record(SESSION));
    let o = o.as_object()?;
    let text = |name: &str| o.get(name).and_then(Value::as_str).unwrap_or_default().to_string();
    let opt = |name: &str| o.get(name).and_then(Value::as_str).map(str::to_string);
    let logs = o.get("logs")?.as_array()?.iter().map(|entry| LogEntry::read(entry).ok()).collect::<Option<Vec<LogEntry>>>()?;
    let permissions = o.get("permissions")?.as_array()?.iter().filter_map(Value::as_object).map(permission_read).collect();
    let queued = o
        .get("queued")?
        .as_array()?
        .iter()
        .filter_map(Value::as_object)
        .map(|q| MirrorQueued {
            id: q.get("id").and_then(Value::as_str).unwrap_or_default().into(),
            text: q.get("text").and_then(Value::as_str).unwrap_or_default().into(),
            run_now: q.get("runNow").and_then(Value::as_str).filter(|r| protocol::RUN_NOW.contains(r)).map(str::to_string),
        })
        .collect();
    let member = |name: &str| o.get(name).filter(|v| !v.is_null());
    Some(MirrorSession {
        id: text("id"),
        workspace_id: text("workspaceId"),
        title: text("title"),
        kind: text("kind"),
        provider: text("provider"),
        model: text("model"),
        status: text("status"),
        created_at: text("createdAt"),
        settings: RunSettings::read(o.get("settings")?).ok()?,
        logs,
        has_older: o.get("hasOlder")?.as_bool()?,
        graph_runs: graph_runs_read(o.get("graphRuns")?)?,
        permissions,
        queued,
        title_mode: opt("titleMode"),
        agent_view_mode: opt("agentViewMode"),
        mighty_style: opt("mightyStyle"),
        todo_progress: member("todoProgress").and_then(|v| TodoProgress::read(&exact(v, Record(TODOS))).ok()),
        background_work: member("backgroundWork").and_then(|v| BackgroundWork::read(&exact(v, Record(BACKGROUND))).ok()),
        started_at: opt("startedAt"),
        finished_at: opt("finishedAt"),
        branch: opt("branch"),
        base: opt("base"),
        queue_actions: o.get("queueActions").and_then(Value::as_bool) == Some(true),
    })
}

/// `RemoteText.Count`: characters as the Mac counts them, counted only as far as `limit` + 1.
fn characters(value: &str, limit: usize) -> usize {
    text::count(value, limit + 1)
}

/// `Valid(state)`: a host's list as the client takes it (the Mac's `valid`): this version, bounded counts and identifiers, panes only of
/// listed workspaces.
pub fn valid_state(state: &WireMirrorState) -> bool {
    if state.protocol != REMOTE_VERSION
        || state.mirror != protocol::VERSION
        || state.host_name.is_empty()
        || characters(&state.host_name, 120) > 120
        || characters(&state.revision, 64) > 64
    {
        return false;
    }
    if state.unchanged == Some(true) {
        return true;
    }
    let workspaces: std::collections::HashSet<&str> = state.workspaces.iter().map(|w| w.id.as_str()).collect();
    let sessions: std::collections::HashSet<&str> = state.sessions.iter().map(|s| s.id.as_str()).collect();
    state.workspaces.len() <= remote_protocol::MAXIMUM_SHARED_WORKSPACES
        && state.workspaces.iter().all(validation::shared_workspace)
        && workspaces.len() == state.workspaces.len()
        && state.sessions.len() <= protocol::MAXIMUM_SESSIONS
        && sessions.len() == state.sessions.len()
        && state.sessions.iter().all(|s| {
            wire::identifier(&s.id)
                && workspaces.contains(s.workspace_id.as_str())
                && s.kind == "claude"
                && known_provider(&s.provider)
                && characters(&s.title, 200) <= 200
                && characters(&s.model, 200) <= 200
                && characters(&s.revision, 64) <= 64
                && protocol::STATUSES.contains(&s.status.as_str())
                && (0..=10_000).contains(&s.pending)
                && (0..=10_000).contains(&s.queued)
                && s.branch.as_deref().is_none_or(|b| characters(b, 200) <= 200)
        })
}

/// `Valid(detail, session, sessionId)`: one pane's detail as the client takes it (the Mac's `valid(_:sessionId:)`).
pub fn valid_detail(detail: &WireMirrorDetail, session: &MirrorSession, session_id: &str) -> bool {
    if detail.protocol != REMOTE_VERSION
        || detail.mirror != protocol::VERSION
        || characters(&detail.revision, 64) > 64
        || session.id != session_id
        || session.kind != "claude"
        || !known_provider(&session.provider)
        || characters(&session.title, 200) > 200
        || !protocol::STATUSES.contains(&session.status.as_str())
        || session.logs.len() > protocol::MAXIMUM_ENTRIES
        || session.graph_runs.len() > protocol::MAXIMUM_GRAPH_RUNS
        || session.permissions.len() > protocol::MAXIMUM_PERMISSIONS
        || session.queued.len() > protocol::MAXIMUM_QUEUED
    {
        return false;
    }
    session.logs.iter().all(|l| wire::identifier(l.id()) && STORED_KINDS.contains(&l.kind()) && l.text().len() <= 131_072)
        && session
            .permissions
            .iter()
            .all(|p| wire::identifier(&p.run_id) && !p.id.is_empty() && characters(&p.id, 256) <= 256 && p.input_json.len() <= 65_536)
}

/// `Pane(summary, existing, clientId, workspaceId, reference)`: the client pane for a host pane: a new one from its summary, or the one
/// it already has with the host's fields laid over it. What the client keeps of its own (block sizes, view choices made here) stays.
pub fn pane(
    summary: &MirrorSummary,
    existing: Option<&RunSession>,
    client_id: &str,
    workspace_id: &str,
    reference: MirrorReference,
) -> RunSession {
    let base = existing.cloned().unwrap_or_else(|| RunSession {
        id: Some(client_id.into()),
        workspace_id: Some(workspace_id.into()),
        title: Some(summary.title.clone()),
        kind: Some("claude".into()),
        provider: Some(summary.provider.clone()),
        ..RunSession::default()
    });
    RunSession {
        workspace_id: Some(workspace_id.into()),
        title: Some(summary.title.clone()),
        provider: Some(summary.provider.clone()),
        model: Some(summary.model.clone()),
        status: Some(summary.status.clone()),
        mirror: Some(reference),
        ..base
    }
}

/// `Apply(detail, existing, now)`: the host pane's detail laid over the client pane (the view mode only when the host sent one).
pub fn apply(detail: &MirrorSession, existing: &RunSession, now: DateTimeOffset) -> RunSession {
    RunSession {
        title: Some(detail.title.clone()),
        title_mode: detail.title_mode.clone(),
        provider: Some(detail.provider.clone()),
        model: Some(detail.model.clone()),
        status: Some(detail.status.clone()),
        settings: Some(detail.settings.clone()),
        agent_view_mode: detail.agent_view_mode.clone().or_else(|| existing.agent_view_mode.clone()),
        mighty_style: detail.mighty_style.clone(),
        logs: Some(detail.logs.clone()),
        graph_runs: if detail.graph_runs.is_empty() { None } else { Some(detail.graph_runs.clone()) },
        todo_progress: detail.todo_progress.clone(),
        background_work: detail.background_work.clone(),
        run_timing: timing(detail.started_at.as_deref(), detail.finished_at.as_deref(), now),
        current_activity: None,
        ..existing.clone()
    }
}

/// `Timing(started, finished, now)`: the host's clock as a run timing here (a finish before the start is none).
fn timing(started: Option<&str>, finished: Option<&str>, now: DateTimeOffset) -> Option<AgentRunTiming> {
    let start = time::parse(started)?;
    let end = time::parse(finished).filter(|stop| stop.utc_ticks() >= start.utc_ticks());
    let observed = end.unwrap_or(if now.utc_ticks() > start.utc_ticks() { now } else { start });
    Some(AgentRunTiming { started_at: start, last_observed_at: observed, finished_at: end, is_approximate: false })
}
