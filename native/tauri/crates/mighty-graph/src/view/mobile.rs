//! The Mighty graph as a phone reads it (C# `MobileWire`, `MobileMightySupport`, the records of MightyCode.Core/Remote/MobileMighty.cs
//! and the role view of MobileFlow.cs; macOS MobileRemoteSupport.swift:190-384, MobileFlow.swift), and the background strip's rows
//! (C# `PlanCardSupport.BackgroundStripRows`, `MobileBackgroundDone`; M/PlanCardSupport.swift). Pure rules over the saved runs: every
//! status that reaches the wire is one of the contract's five words, every text has its cap, only the newest twenty runs travel, and a
//! background run's output file never does. The m1 router (mighty-remote `phone`, W8-B) writes these records; the desktop's background
//! card reads the strip's rows (src-tauri panes.rs).
//!
//! The records write themselves as `Wire.Json` writes C#'s (declaration order, a null left out) through [`MobileOut::write`].

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Mutex;

use mighty_core::graph::{Awaiting, MightyGraphAgent, MightyGraphRun, shown_agents};
use mighty_core::json::JsonOut;
use mighty_core::locale::Locale;
use mighty_core::model::{BackgroundTask, BackgroundWork, LogEntry, RunSession};
use mighty_core::time::{self, DateTimeOffset};
use unicode_segmentation::UnicodeSegmentation;

use crate::flow::roles::{self, AgentDefinition, FlowMark, FlowRole};
use crate::flow::{card, lines, log, steps};
use crate::node::{self, clean};
use crate::result_files::{self, ResultFileLookup};
use crate::{notice, support, timeline, view_model};

// ── MobileWire ──────────────────────────────────────────────────────────────────────────────────────────────────────────

/// `MobileWire.CliStyle`.
pub const CLI_STYLE: &str = "cli";
/// `MobileWire.MightyStyles`: the old `mightyStyle` field's words.
pub const MIGHTY_STYLES: [&str; 2] = [CLI_STYLE, "ouroboros"];
/// `MobileWire.MaximumEntries`: the newest entries a session detail carries.
pub const MAXIMUM_ENTRIES: usize = 80;
pub const DEFAULT_PAGE_LIMIT: i64 = 50;
pub const MAXIMUM_PAGE_LIMIT: i64 = 100;
/// `MobileWire.MaximumBlockOutput`, `MaximumRunResult`: in characters.
pub const MAXIMUM_BLOCK_OUTPUT: usize = 2_000;
pub const MAXIMUM_RUN_RESULT: usize = 20_000;
/// `MobileWire.MaximumRunResultFiles`.
pub const MAXIMUM_RUN_RESULT_FILES: usize = 50;
/// `MobileWire.MaximumBlockSummary`: a block's own prompt and each activity line, in bytes.
pub const MAXIMUM_BLOCK_SUMMARY: usize = 1_000;
pub const MAXIMUM_BLOCK_ACTIVITY: usize = 8;
/// `MobileWire.MightyRuns`: how many of a pane's newest runs the payload carries.
pub const MIGHTY_RUNS: usize = 20;
pub const MAXIMUM_TEXT: usize = 32_768;
/// `MobileWire.MaximumReplyBytes`: 1 MiB less 32 KiB.
pub const MAXIMUM_REPLY_BYTES: usize = 1_024 * 1_024 - 32 * 1_024;
/// `BackgroundOutputTail.MaxLines`.
pub const OUTPUT_TAIL_LINES: usize = 6;
/// `StyleStateEngine.MaximumTaskListItems`.
pub const MAXIMUM_TASK_LIST_ITEMS: usize = 8;

/// A record as `Wire.Json` writes it.
pub trait MobileOut {
    fn write(&self, out: &mut JsonOut);

    fn to_json(&self) -> String {
        let mut out = JsonOut::new();
        self.write(&mut out);
        out.finish()
    }
}

impl<T: MobileOut> MobileOut for Vec<T> {
    fn write(&self, out: &mut JsonOut) {
        out.begin_array();
        for item in self {
            item.write(out);
        }
        out.end_array();
    }
}

fn prop<T: MobileOut>(out: &mut JsonOut, name: &str, value: Option<&T>) {
    if let Some(value) = value {
        out.name(name);
        value.write(out);
    }
}

fn strings(out: &mut JsonOut, name: &str, value: Option<&Vec<String>>) {
    out.prop_strings(name, value);
}

// ── the records ─────────────────────────────────────────────────────────────────────────────────────────────────────────

/// `MobileBlock`: one block of the graph as a phone draws it, the request itself (`main`) or one of its children.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MobileBlock {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub status: String,
    pub summary: Option<String>,
    pub output: Option<String>,
    pub duration_ms: Option<f64>,
    pub node_model_label: Option<String>,
    pub activity: Option<Vec<String>>,
    pub background: Option<bool>,
    pub output_tail: Option<Vec<String>>,
    pub notice_reply: Option<String>,
    pub role: Option<MobileBlockRole>,
    pub card: Option<MobileBlockCard>,
    pub delegate: Option<MobileDelegateLine>,
}

impl MobileOut for MobileBlock {
    fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.prop_str("id", Some(&self.id));
        out.prop_str("kind", Some(&self.kind));
        out.prop_str("title", Some(&self.title));
        out.prop_str("status", Some(&self.status));
        out.prop_str("summary", self.summary.as_deref());
        out.prop_str("output", self.output.as_deref());
        out.prop_opt_float("durationMs", self.duration_ms);
        out.prop_str("nodeModelLabel", self.node_model_label.as_deref());
        strings(out, "activity", self.activity.as_ref());
        out.prop_opt_bool("background", self.background);
        strings(out, "outputTail", self.output_tail.as_ref());
        out.prop_str("noticeReply", self.notice_reply.as_deref());
        prop(out, "role", self.role.as_ref());
        prop(out, "card", self.card.as_ref());
        prop(out, "delegate", self.delegate.as_ref());
        out.end_object();
    }
}

/// `MobileMightyRun`: one request of the pane.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MobileMightyRun {
    pub id: String,
    pub input: String,
    pub title: Option<String>,
    pub status: String,
    pub blocks: Vec<MobileBlock>,
    pub result: Option<String>,
    pub notice_run: Option<bool>,
    pub background_waiting: Option<i32>,
    pub result_files: Option<Vec<MobileResultFile>>,
    pub awaiting_notice: Option<MobileAwaitingNotice>,
    pub returning: Option<MobileReturn>,
    pub progress: Option<MobileProgress>,
}

impl MobileOut for MobileMightyRun {
    fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.prop_str("id", Some(&self.id));
        out.prop_str("input", Some(&self.input));
        out.prop_str("title", self.title.as_deref());
        out.prop_str("status", Some(&self.status));
        out.name("blocks");
        self.blocks.write(out);
        out.prop_str("result", self.result.as_deref());
        out.prop_opt_bool("noticeRun", self.notice_run);
        out.prop_opt_int("backgroundWaiting", self.background_waiting.map(i64::from));
        prop(out, "resultFiles", self.result_files.as_ref());
        prop(out, "awaitingNotice", self.awaiting_notice.as_ref());
        prop(out, "returning", self.returning.as_ref());
        prop(out, "progress", self.progress.as_ref());
        out.end_object();
    }
}

/// `MobileResultFile(Path, Line?)`: one file a run's final result names, relative to the workspace root with "/".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobileResultFile {
    pub path: String,
    pub line: Option<i32>,
}

impl MobileOut for MobileResultFile {
    fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.prop_str("path", Some(&self.path));
        out.prop_opt_int("line", self.line.map(i64::from));
        out.end_object();
    }
}

/// `MobileAwaitingNotice(Delivered, Total, Tasks)`: a request's 알림 대기 block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobileAwaitingNotice {
    pub delivered: usize,
    pub total: usize,
    pub tasks: Vec<MobileAwaitedTask>,
}

impl MobileAwaitingNotice {
    pub const MAXIMUM_TASKS: usize = 16;
    pub const MAXIMUM_TITLE: usize = 200;

    /// `MobileAwaitingNotice.From`.
    pub fn from(awaiting: Option<&Awaiting>) -> Option<MobileAwaitingNotice> {
        let awaiting = awaiting?;
        Some(MobileAwaitingNotice {
            delivered: awaiting.delivered(),
            total: awaiting.tasks.len(),
            tasks: awaiting
                .tasks
                .iter()
                .take(Self::MAXIMUM_TASKS)
                .map(|task| MobileAwaitedTask {
                    title: clean(&task.title, Self::MAXIMUM_TITLE, true),
                    status: timeline::status(&task.status).to_string(),
                })
                .collect(),
        })
    }
}

impl MobileOut for MobileAwaitingNotice {
    fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.prop_int("delivered", self.delivered as i64);
        out.prop_int("total", self.total as i64);
        out.name("tasks");
        self.tasks.write(out);
        out.end_object();
    }
}

/// `MobileAwaitedTask(Title, Status)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobileAwaitedTask {
    pub title: String,
    pub status: String,
}

impl MobileOut for MobileAwaitedTask {
    fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.prop_str("title", Some(&self.title));
        out.prop_str("status", Some(&self.status));
        out.end_object();
    }
}

/// `MobileInkedText(Text, Ink)`: a word in its ink (a design token name).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobileInkedText {
    pub text: String,
    pub ink: String,
}

impl MobileOut for MobileInkedText {
    fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.prop_str("text", Some(&self.text));
        out.prop_str("ink", Some(&self.ink));
        out.end_object();
    }
}

/// `MobileBlockRole(Name, Ink, Identity, Marks, Summary)`: who a block is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobileBlockRole {
    pub name: String,
    pub ink: String,
    pub identity: Option<String>,
    pub marks: Option<Vec<String>>,
    pub summary: String,
}

impl MobileOut for MobileBlockRole {
    fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.prop_str("name", Some(&self.name));
        out.prop_str("ink", Some(&self.ink));
        out.prop_str("identity", self.identity.as_deref());
        strings(out, "marks", self.marks.as_ref());
        out.prop_str("summary", Some(&self.summary));
        out.end_object();
    }
}

/// `MobileCopyRef(RunId, BlockId)`: a notice run's copy of a finished background block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobileCopyRef {
    pub run_id: String,
    pub block_id: String,
}

impl MobileOut for MobileCopyRef {
    fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.prop_str("runId", Some(&self.run_id));
        out.prop_str("blockId", Some(&self.block_id));
        out.end_object();
    }
}

/// `MobileBlockCard`: the compact row of a sub-agent or a background command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobileBlockCard {
    pub activity: Option<String>,
    pub before: Vec<String>,
    pub after: Vec<String>,
    pub started_at: Option<i64>,
    pub ended_at: Option<i64>,
    pub nested: Option<i32>,
    pub copy: Option<MobileCopyRef>,
    pub step: Option<String>,
}

impl MobileOut for MobileBlockCard {
    fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.prop_str("activity", self.activity.as_deref());
        out.name("before");
        out.strings(&self.before);
        out.name("after");
        out.strings(&self.after);
        out.prop_opt_int("startedAt", self.started_at);
        out.prop_opt_int("endedAt", self.ended_at);
        out.prop_opt_int("nested", self.nested.map(i64::from));
        prop(out, "copy", self.copy.as_ref());
        out.prop_str("step", self.step.as_deref());
        out.end_object();
    }
}

/// `MobileProgress`: a request's progress panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobileProgress {
    pub title: String,
    pub heading: String,
    pub request: String,
    pub state_word: String,
    pub status: String,
    pub count: Option<String>,
    pub rows: Vec<MobileProgressRow>,
    pub hidden: usize,
    pub more: Option<String>,
    pub running: bool,
    /// The background work the request started (`steps::background_of`), running first; sent only when there is some.
    pub background: Vec<MobileProgressBackground>,
    pub background_hidden: usize,
}

impl MobileOut for MobileProgress {
    fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.prop_str("title", Some(&self.title));
        out.prop_str("heading", Some(&self.heading));
        out.prop_str("request", Some(&self.request));
        out.prop_str("stateWord", Some(&self.state_word));
        out.prop_str("status", Some(&self.status));
        out.prop_str("count", self.count.as_deref());
        out.name("rows");
        self.rows.write(out);
        out.prop_int("hidden", self.hidden as i64);
        out.prop_str("more", self.more.as_deref());
        out.prop_bool("running", self.running);
        if !self.background.is_empty() {
            out.name("background");
            self.background.write(out);
        }
        out.prop_opt_int("backgroundHidden", (self.background_hidden > 0).then_some(self.background_hidden as i64));
        out.end_object();
    }
}

/// A background line of the progress panel: a command or a sub-agent the request started, and how it is doing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobileProgressBackground {
    pub id: String,
    pub text: String,
    pub agent: bool,
    pub state: String,
    pub started_at: Option<i64>,
    pub ended_at: Option<i64>,
}

impl MobileOut for MobileProgressBackground {
    fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.prop_str("id", Some(&self.id));
        out.prop_str("text", Some(&self.text));
        out.prop_bool("agent", self.agent);
        out.prop_str("state", Some(&self.state));
        out.prop_opt_int("startedAt", self.started_at);
        out.prop_opt_int("endedAt", self.ended_at);
        out.end_object();
    }
}

/// `MobileProgressRow(Index, Text, State, StartedAt, EndedAt)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobileProgressRow {
    pub index: usize,
    pub text: String,
    pub state: String,
    pub started_at: Option<i64>,
    pub ended_at: Option<i64>,
}

impl MobileOut for MobileProgressRow {
    fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.prop_int("index", self.index as i64);
        out.prop_str("text", Some(&self.text));
        out.prop_str("state", Some(&self.state));
        out.prop_opt_int("startedAt", self.started_at);
        out.prop_opt_int("endedAt", self.ended_at);
        out.end_object();
    }
}

/// `MobileDelegateLine(Head, Parts, Model, Progress, Text)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobileDelegateLine {
    pub head: String,
    pub parts: Option<Vec<MobileInkedText>>,
    pub model: Option<String>,
    pub progress: Option<String>,
    pub text: String,
}

impl MobileOut for MobileDelegateLine {
    fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.prop_str("head", Some(&self.head));
        prop(out, "parts", self.parts.as_ref());
        out.prop_str("model", self.model.as_deref());
        out.prop_str("progress", self.progress.as_deref());
        out.prop_str("text", Some(&self.text));
        out.end_object();
    }
}

/// `MobileReturn(Text, Waiting)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobileReturn {
    pub text: String,
    pub waiting: bool,
}

impl MobileOut for MobileReturn {
    fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.prop_str("text", Some(&self.text));
        out.prop_bool("waiting", self.waiting);
        out.end_object();
    }
}

/// `MobileFlowLog(Status, Rows)`: the session log strip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobileFlowLog {
    pub status: Vec<MobileInkedText>,
    pub rows: Vec<MobileFlowLogRow>,
}

impl MobileOut for MobileFlowLog {
    fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.name("status");
        self.status.write(out);
        out.name("rows");
        self.rows.write(out);
        out.end_object();
    }
}

/// `MobileFlowLogRow(Id, At, Who, Ink, What)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobileFlowLogRow {
    pub id: String,
    pub at: i64,
    pub who: String,
    pub ink: String,
    pub what: String,
}

impl MobileOut for MobileFlowLogRow {
    fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.prop_str("id", Some(&self.id));
        out.prop_int("at", self.at);
        out.prop_str("who", Some(&self.who));
        out.prop_str("ink", Some(&self.ink));
        out.prop_str("what", Some(&self.what));
        out.end_object();
    }
}

// ── MobileFlow: the role view ───────────────────────────────────────────────────────────────────────────────────────────

/// `MobileFlow.Roles(FanOut, Definitions)`: what a payload with the role view is built with.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Roles {
    pub fan_out: bool,
    pub definitions: HashMap<String, AgentDefinition>,
}

impl Roles {
    pub fn new(fan_out: bool) -> Roles {
        Roles { fan_out, definitions: HashMap::new() }
    }

    /// `Definition(agent)`: the custom agent the block's type names.
    pub fn definition(&self, agent: &MightyGraphAgent) -> Option<&AgentDefinition> {
        agent.identity.as_ref().and_then(|i| i.kind.as_deref()).and_then(|kind| self.definitions.get(kind))
    }
}

/// `MobileFlow.Milliseconds`: a time as the wire carries it.
fn milliseconds(time: Option<&DateTimeOffset>) -> Option<i64> {
    time.map(steps::unix_ms)
}

fn mark_names(marks: &[FlowMark]) -> Option<Vec<String>> {
    (!marks.is_empty()).then(|| {
        marks
            .iter()
            .map(|m| match m {
                FlowMark::Background => "background",
                FlowMark::Fork => "fork",
                FlowMark::Worktree => "worktree",
            })
            .map(str::to_string)
            .collect()
    })
}

fn role_word(role: FlowRole) -> &'static str {
    match role {
        FlowRole::Lead => "lead",
        FlowRole::Subagent => "subagent",
        FlowRole::Task => "task",
        FlowRole::Advisor => "advisor",
    }
}

fn token(role: FlowRole) -> String {
    roles::token(roles::ink(role)).to_string()
}

/// `MobileFlow.Lead`: the main session's identity on a run's `main` block (`request`: the request a notice run answers for).
pub fn lead(run: &MightyGraphRun, request: Option<&MightyGraphRun>, locale: &Locale) -> MobileBlockRole {
    let who = roles::lead(run, locale);
    let mut parts = vec![log::lead_name(run, request, locale)];
    if let Some(effort) = &who.effort {
        parts.push(locale.get_with("composer.effort.menu", &[("effort", effort)]));
    }
    MobileBlockRole {
        name: role_word(who.role).into(),
        ink: roles::token(who.ink).into(),
        identity: Some(parts.join(" · ")),
        marks: None,
        summary: who.summary,
    }
}

/// `MobileFlow.Role`: a child block's identity, a sub-agent's or (in the role view) a background command's; None for neither.
pub fn role(
    agent: &MightyGraphAgent,
    run: &MightyGraphRun,
    roles: &Roles,
    output: Option<&Vec<String>>,
    locale: &Locale,
) -> Option<MobileBlockRole> {
    let definition = roles.definition(agent);
    if let Some(who) = roles::subagent(agent, definition, locale) {
        return Some(MobileBlockRole {
            name: role_word(who.role).into(),
            ink: roles::token(who.ink).into(),
            identity: card::identity_line(agent, definition, locale),
            marks: mark_names(&who.marks),
            summary: who.summary,
        });
    }
    if !roles.fan_out {
        return None;
    }
    let task = roles::task(agent, locale)?;
    let output = output.map(Vec::as_slice).unwrap_or_default();
    Some(MobileBlockRole {
        name: role_word(task.role).into(),
        ink: roles::token(task.ink).into(),
        identity: card::of(agent, run, None, output, locale).identity,
        marks: mark_names(&task.marks),
        summary: task.summary,
    })
}

/// `MobileFlow.Card`: the compact row of a request's background sub-agent or command, or of a running sub-agent; None otherwise.
pub fn card_of(
    agent: &MightyGraphAgent,
    run: &MightyGraphRun,
    window: &[MightyGraphRun],
    roles: &Roles,
    output: Option<&Vec<String>>,
    locale: &Locale,
) -> Option<MobileBlockCard> {
    if !roles.fan_out || !card::folds_to_card(agent) {
        return None;
    }
    let lines: &[String] = if agent.is_task() { output.map(Vec::as_slice).unwrap_or_default() } else { &[] };
    let content = card::of(agent, run, roles.definition(agent), lines, locale);
    let (before, after) = card::figure_parts(&content, locale);
    let mut copy = None;
    if agent.background == Some(true) && support::layout_terminal(agent.status()) {
        for candidate in window.iter().rev() {
            if candidate.notice_run == Some(true)
                && let Some(copied) = candidate.agents().iter().rev().find(|a| a.source_block_id.as_deref() == Some(agent.id()))
            {
                copy = Some(MobileCopyRef { run_id: candidate.id().into(), block_id: copied.id().into() });
                break;
            }
        }
    }
    Some(MobileBlockCard {
        activity: content.activity.clone(),
        before,
        after,
        started_at: milliseconds(content.started_at.as_ref()),
        ended_at: milliseconds(content.ended_at.as_ref()),
        nested: (content.nested > 0).then_some(content.nested),
        copy,
        step: steps::step_label(agent, run, locale),
    })
}

/// `MobileFlow.Progress`: the progress of request `runs[index]`, headed by its request number; None for a notice run.
pub fn progress(index: usize, runs: &[MightyGraphRun], ordinal: i32, locale: &Locale) -> Option<MobileProgress> {
    if index >= runs.len() || runs[index].notice_run == Some(true) {
        return None;
    }
    let heading = locale.get_with("graph.timeline.requestOrdinal", &[("n", &ordinal.to_string())]);
    let value = steps::progress_of(index, runs, &heading, locale);
    let rows = value
        .rows
        .iter()
        .map(|row| MobileProgressRow {
            index: row.index,
            text: row.text.clone(),
            state: row.state.name().into(),
            started_at: milliseconds(row.started_at.as_ref()),
            ended_at: milliseconds(row.ended_at.as_ref()),
        })
        .collect();
    let more = (value.hidden > 0).then(|| locale.get_with("graph.progress.more", &[("count", &value.hidden.to_string())]));
    Some(MobileProgress {
        title: value.title,
        heading: value.heading,
        request: value.request,
        state_word: value.state_word,
        status: value.status,
        count: value.count,
        rows,
        hidden: value.hidden,
        more,
        running: value.running,
        background: value
            .background
            .iter()
            .map(|item| MobileProgressBackground {
                id: item.id.clone(),
                text: item.text.clone(),
                agent: item.agent,
                state: item.state.name().into(),
                started_at: milliseconds(item.started_at.as_ref()),
                ended_at: milliseconds(item.ended_at.as_ref()),
            })
            .collect(),
        background_hidden: value.background_hidden,
    })
}

/// `MobileFlow.Delegate`: the line over the rows block `key` delegated to; None when it delegated to none, and outside the role view.
pub fn delegate(key: &str, run: &MightyGraphRun, roles: &Roles, locale: &Locale) -> Option<MobileDelegateLine> {
    if !roles.fan_out {
        return None;
    }
    let children = lines::children(key, run);
    let line = lines::line_of(&children, &roles.definitions, locale)?;
    let text = line.text();
    Some(MobileDelegateLine {
        head: line.head,
        parts: (!line.parts.is_empty())
            .then(|| line.parts.iter().map(|p| MobileInkedText { text: p.text.clone(), ink: token(p.role) }).collect()),
        model: line.model,
        progress: line.progress,
        text,
    })
}

/// `MobileFlow.Returning`: the way back while the request's result is not there.
pub fn returning(run: &MightyGraphRun, roles: &Roles, locale: &Locale) -> Option<MobileReturn> {
    if !roles.fan_out || support::has_result(run) {
        return None;
    }
    let state = lines::return_of(run, locale)?;
    Some(MobileReturn { waiting: state.waiting(), text: state.text })
}

/// `MobileFlow.Log`: the session log strip of a pane's runs; None for a pane without a request.
pub fn flow_log(source: &[MightyGraphRun], definitions: &HashMap<String, AgentDefinition>, locale: &Locale) -> Option<MobileFlowLog> {
    let runs = support::shown_runs(source, locale);
    let status = log::status_of(&runs, locale)?;
    let rows = log::rows(&runs, log::VISIBLE_ROWS, definitions, locale)
        .into_iter()
        .map(|row| MobileFlowLogRow {
            id: row.id,
            at: steps::unix_ms(&row.time),
            who: row.who,
            ink: roles::token(row.ink).into(),
            what: row.what,
        })
        .collect();
    Some(MobileFlowLog { status: status.parts().into_iter().map(|p| MobileInkedText { ink: token(p.role), text: p.text }).collect(), rows })
}

// ── MobileMightySupport ─────────────────────────────────────────────────────────────────────────────────────────────────

/// `MobileMightySupport.Status`: one of the contract's five words.
pub fn status(raw: &str) -> &'static str {
    match raw {
        "completed" => "completed",
        "error" | "failed" => "error",
        "stopped" | "cancelled" | "interrupted" => "stopped",
        "waiting" => "waiting",
        _ => "running",
    }
}

/// `MobileMightySupport.Settled`.
pub fn settled(status: &str) -> bool {
    matches!(status, "completed" | "error" | "stopped")
}

/// `MobileMightySupport.Prefix`: the first `count` characters as a reader counts them (text elements), never half a glyph.
pub fn prefix(text: &str, count: usize) -> &str {
    if text.encode_utf16().count() <= count {
        return text;
    }
    match text.grapheme_indices(true).nth(count) {
        Some((at, _)) => &text[..at],
        None => text,
    }
}

/// `MobileMightySupport.Output`: cleaned, at most `maximum` characters, None when empty.
pub fn output(raw: Option<&str>, maximum: usize) -> Option<String> {
    let clean = node::clean(raw?, 4 * maximum, false);
    (!clean.is_empty()).then(|| prefix(&clean, maximum).to_string())
}

fn summary(raw: &str) -> Option<String> {
    let clean = node::clean(raw, MAXIMUM_BLOCK_SUMMARY, true);
    (!clean.is_empty()).then_some(clean)
}

/// The UTC ticks of a time `DateTimeOffset.TryParse(…, AssumeUniversal)` reads: a time with no zone is UTC.
fn universal_ticks(text: Option<&str>) -> Option<i64> {
    let parsed = time::parse(text)?;
    let text = text?.trim();
    let zoned = text.find(['T', 't', ' ']).is_some_and(|t| text[t..].contains(['Z', 'z', '+', '-']));
    Some(if zoned { parsed.utc_ticks() } else { parsed.local_ticks })
}

/// `ActivitySupport.MaximumDurationMs`: 30 days.
const MAXIMUM_DURATION_MS: f64 = 30.0 * 24.0 * 60.0 * 60.0 * 1000.0;

/// `MobileMightySupport.Duration`: how long a settled block's own records span (rounded away from zero), None while in motion.
pub fn duration(entries: &[LogEntry], status: &str) -> Option<f64> {
    if !settled(status) || entries.len() < 2 {
        return None;
    }
    let first = universal_ticks(entries[0].timestamp.as_deref())?;
    let last = universal_ticks(entries[entries.len() - 1].timestamp.as_deref())?;
    let milliseconds = (last - first) as f64 / 10_000.0;
    (milliseconds.is_finite() && (0.0..=MAXIMUM_DURATION_MS).contains(&milliseconds) && milliseconds > 0.0).then(|| milliseconds.round())
}

/// `MobileMightySupport.Activity`: the recent steps of a block still in motion, oldest first.
pub fn activity(entries: &[LogEntry], status: &str) -> Option<Vec<String>> {
    if !matches!(status, "running" | "waiting") {
        return None;
    }
    let mut lines: Vec<String> = entries.iter().rev().filter_map(step).take(MAXIMUM_BLOCK_ACTIVITY).collect();
    if lines.is_empty() {
        return None;
    }
    lines.reverse();
    Some(lines)
}

/// `MobileMightySupport.Step`: a tool call is its summary (else its tool's name), a reply or an error its first line.
pub fn step(entry: &LogEntry) -> Option<String> {
    if let Some(activity) = &entry.activity {
        if activity.kind == "turn" {
            return None;
        }
        return summary(if activity.summary.is_empty() { activity.tool_name.as_deref().unwrap_or("") } else { &activity.summary });
    }
    if !matches!(entry.kind(), "assistant" | "error") || mighty_run::questionnaire::UserQuestionnaire::parse(entry.text()).is_some() {
        return None;
    }
    let text = entry.text().trim_start();
    let first = text.split(['\r', '\n']).next().unwrap_or("");
    summary(first)
}

fn answer(entries: &[LogEntry]) -> Option<&str> {
    entries.iter().rev().find(|e| e.kind() == "assistant" && !e.text().is_empty()).map(LogEntry::text)
}

/// A background run's last output lines by block id (held in host memory only; the output file's path never reaches the wire).
pub type OutputTails = HashMap<String, Vec<String>>;

/// `MobileMightySupport.Blocks`: the request itself, then its children in the order the graph recorded them (the fan-out with the role
/// view, else only a notice run's background blocks).
pub fn blocks(
    run: &MightyGraphRun,
    ordinal: i32,
    output_tails: Option<&OutputTails>,
    roles: Option<&Roles>,
    request: Option<&MightyGraphRun>,
    window: &[MightyGraphRun],
    locale: &Locale,
) -> Vec<MobileBlock> {
    let main = status(run.status());
    let title = if run.notice_run == Some(true) {
        notice::heading(run, locale)
    } else {
        locale.get_with("remote.block.request", &[("ordinal", &ordinal.to_string())])
    };
    let mut result = vec![MobileBlock {
        id: format!("{}:main", run.id()),
        kind: "main".into(),
        title,
        status: main.into(),
        summary: summary(run.input()),
        output: output(run.final_output(), MAXIMUM_BLOCK_OUTPUT),
        duration_ms: duration(run.root_entries(), main),
        node_model_label: run.node_model_label.clone(),
        activity: activity(run.root_entries(), main),
        role: roles.map(|_| lead(run, request, locale)),
        delegate: roles.and_then(|r| delegate(lines::MAIN_KEY, run, r, locale)),
        ..MobileBlock::default()
    }];
    let agents: Vec<&MightyGraphAgent> = if roles.is_some_and(|r| r.fan_out) { run.agents().iter().collect() } else { shown_agents(run) };
    for agent in agents {
        let state = status(agent.status());
        let lines = output_tails.and_then(|t| t.get(agent.id()));
        result.push(MobileBlock {
            id: agent.id().into(),
            kind: support::block_kind(agent).into(),
            title: support::block_title(agent, locale),
            status: state.into(),
            summary: summary(agent.input()),
            output: output(answer(agent.entries()), MAXIMUM_BLOCK_OUTPUT),
            duration_ms: duration(agent.entries(), state),
            node_model_label: None,
            activity: activity(agent.entries(), state),
            background: (agent.background == Some(true)).then_some(true),
            output_tail: if agent.is_task() { output_tail(lines) } else { None },
            notice_reply: output(agent.notice_reply.as_deref(), MAXIMUM_BLOCK_OUTPUT),
            role: roles.and_then(|r| role(agent, run, r, lines, locale)),
            card: roles.and_then(|r| card_of(agent, run, window, r, lines, locale)),
            delegate: roles.filter(|_| support::is_subagent(agent)).and_then(|r| delegate(agent.id(), run, r, locale)),
        });
    }
    result
}

/// `MobileMightySupport.OutputTail`: at most six lines, None when there are none.
pub fn output_tail(lines: Option<&Vec<String>>) -> Option<Vec<String>> {
    let lines = lines.filter(|l| !l.is_empty())?;
    Some(lines[lines.len().saturating_sub(OUTPUT_TAIL_LINES)..].to_vec())
}

/// How a memo looks a result through: the run and the workspace folder, the files it names.
pub type LookThrough = dyn Fn(&MightyGraphRun, &str) -> Option<Vec<MobileResultFile>> + Send + Sync;

/// A guided style's own name for a request, from its input.
pub type RequestTitle<'a> = &'a dyn Fn(&str) -> Option<String>;

/// `MobileResultFilesMemo`: one pane's `resultFiles`, kept per run until that run's result or the workspace folder changes.
pub struct ResultFilesMemo {
    look_through: Box<LookThrough>,
    state: Mutex<MemoState>,
}

#[derive(Default)]
struct MemoState {
    by_run: HashMap<String, (u64, Option<Vec<MobileResultFile>>)>,
    used: HashSet<String>,
}

impl std::fmt::Debug for ResultFilesMemo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResultFilesMemo").field("count", &self.count()).finish()
    }
}

impl ResultFilesMemo {
    /// A memo whose look through the workspace is `look_through` (C#'s default: [`look_through`] over the files on disk).
    pub fn new(look_through: impl Fn(&MightyGraphRun, &str) -> Option<Vec<MobileResultFile>> + Send + Sync + 'static) -> ResultFilesMemo {
        ResultFilesMemo { look_through: Box::new(look_through), state: Mutex::new(MemoState::default()) }
    }

    /// `Count`: the runs whose list is kept.
    pub fn count(&self) -> usize {
        self.state.lock().map(|s| s.by_run.len()).unwrap_or(0)
    }

    fn key(run: &MightyGraphRun, root: &str) -> u64 {
        let mut hash = DefaultHasher::new();
        root.hash(&mut hash);
        run.result_entries().len().hash(&mut hash);
        run.result_entries().last().map(LogEntry::id).hash(&mut hash);
        for entry in run.result_entries() {
            entry.text().hash(&mut hash);
        }
        run.final_output().hash(&mut hash);
        hash.finish()
    }

    fn files(&self, run: &MightyGraphRun, root: &str) -> Option<Vec<MobileResultFile>> {
        let key = Self::key(run, root);
        if let Ok(mut state) = self.state.lock() {
            state.used.insert(run.id().to_string());
            if let Some((kept, files)) = state.by_run.get(run.id())
                && *kept == key
            {
                return files.clone();
            }
        }
        let files = (self.look_through)(run, root);
        if let Ok(mut state) = self.state.lock() {
            state.by_run.insert(run.id().to_string(), (key, files.clone()));
        }
        files
    }

    fn prune(&self) {
        if let Ok(mut state) = self.state.lock() {
            let MemoState { by_run, used } = &mut *state;
            by_run.retain(|id, _| used.contains(id));
            used.clear();
        }
    }
}

/// What [`runs`] reads besides the runs (`Runs`' optional parameters).
#[derive(Default, Clone, Copy)]
pub struct RunsInputs<'a> {
    /// The guided style's own name for a request (`title`).
    pub title: Option<RequestTitle<'a>>,
    pub output_tails: Option<&'a OutputTails>,
    /// The pane's workspace folder on this computer (None for another computer's).
    pub workspace_root: Option<&'a str>,
    /// How a result is looked through without a memo (the files on disk).
    pub lookup: Option<&'a dyn ResultFileLookup>,
    pub result_files: Option<&'a ResultFilesMemo>,
    pub roles: Option<&'a Roles>,
}

/// `MobileMightySupport.Runs`: the pane's newest runs, oldest first; only the newest request and the notice runs after it carry their full
/// result, each once settled.
pub fn runs(values: &[MightyGraphRun], inputs: &RunsInputs, locale: &Locale) -> Vec<MobileMightyRun> {
    let values = support::with_background_waiting(values);
    let values: &[MightyGraphRun] = &values;
    let offset = values.len().saturating_sub(MIGHTY_RUNS);
    let ordinals = support::request_ordinals(values);
    let carried = first_carried(values, offset);
    let window = &values[offset..];
    let mut requests: Vec<Option<usize>> = Vec::with_capacity(values.len());
    let mut latest = None;
    for value in values {
        if value.notice_run != Some(true) {
            latest = Some(requests.len());
        }
        requests.push(latest);
    }
    let mut result = Vec::new();
    for index in offset..values.len() {
        let run = &values[index];
        let state = status(run.status());
        let notice = run.notice_run == Some(true);
        let waiting = support::background_waiting(run);
        let result_text =
            if carried.is_some_and(|c| index >= c) && settled(state) { output(run.final_output(), MAXIMUM_RUN_RESULT) } else { None };
        let request = requests[index].map(|at| &values[at]);
        let files =
            if result_text.is_some() { run_result_files(run, inputs.workspace_root, inputs.lookup, inputs.result_files) } else { None };
        let ordinal = ordinals.get(index).copied().unwrap_or(0);
        result.push(MobileMightyRun {
            id: run.id().into(),
            input: clean(run.input(), MAXIMUM_TEXT, false),
            title: if notice { Some(notice::heading(run, locale)) } else { inputs.title.and_then(|t| t(run.input())) },
            status: state.into(),
            blocks: blocks(run, ordinal, inputs.output_tails, inputs.roles, request, window, locale),
            result: result_text,
            notice_run: notice.then_some(true),
            background_waiting: (waiting > 0).then_some(waiting),
            result_files: files,
            awaiting_notice: MobileAwaitingNotice::from(notice::awaiting_notice(run, values, locale).as_ref()),
            returning: inputs.roles.and_then(|r| returning(run, r, locale)),
            progress: inputs.roles.and_then(|_| progress(index, values, ordinal, locale)),
        });
    }
    if let Some(memo) = inputs.result_files {
        memo.prune();
    }
    result
}

/// `MobileMightySupport.RunResultFiles`: the files a completed, finished run's result names inside the root, the first fifty; None
/// without a root or when nothing was named.
pub fn run_result_files(
    run: &MightyGraphRun,
    workspace_root: Option<&str>,
    lookup: Option<&dyn ResultFileLookup>,
    memo: Option<&ResultFilesMemo>,
) -> Option<Vec<MobileResultFile>> {
    let root = workspace_root?;
    if run.status() != "completed" || !support::finished(run) {
        return None;
    }
    match memo {
        Some(memo) => memo.files(run, root),
        None => look_through(run, root, lookup?),
    }
}

/// `MobileMightySupport.LookThrough`: the desktop panel's `FilesFor`, cut to the wire's cap.
pub fn look_through(run: &MightyGraphRun, workspace_root: &str, lookup: &dyn ResultFileLookup) -> Option<Vec<MobileResultFile>> {
    let files = result_files::files_in(&view_model::result_texts(run), Some(workspace_root), lookup);
    (!files.is_empty())
        .then(|| files.into_iter().take(MAXIMUM_RUN_RESULT_FILES).map(|f| MobileResultFile { path: f.path, line: f.line }).collect())
}

/// `FirstCarried`: the first run from `offset` that carries its result once settled — the newest request — or the last run when the
/// window holds no request.
fn first_carried(values: &[MightyGraphRun], offset: usize) -> Option<usize> {
    if values.is_empty() {
        return None;
    }
    (offset..values.len()).rev().find(|&i| values[i].notice_run != Some(true)).or(Some(values.len() - 1))
}

/// `MobileMightySupport.SavedRuns`: the saved graph, or the requests grouped out of the transcript of a pane that has none.
pub fn saved_runs(session: &RunSession) -> Cow<'_, [MightyGraphRun]> {
    match &session.graph_runs {
        Some(saved) => Cow::Borrowed(saved.as_slice()),
        None => Cow::Owned(legacy_runs(session)),
    }
}

/// `MobileMightySupport.LegacyRuns`: a pane's requests grouped from its transcript, without inventing sub-agents.
pub fn legacy_runs(session: &RunSession) -> Vec<MightyGraphRun> {
    fn finish(run: &mut MightyGraphRun, status: &str) {
        run.status = Some(status.into());
        if status == "completed" {
            run.final_output = run.root_entries().iter().rev().find(|e| e.kind() == "assistant").map(|e| e.text().to_string());
        }
    }
    let mut runs: Vec<MightyGraphRun> = Vec::new();
    let empty = |id: String, input: &str, status: Option<&str>| MightyGraphRun {
        id: Some(id),
        input: Some(input.into()),
        status: Some(status.unwrap_or("running").into()),
        provider: Some(session.provider().into()),
        root_entries: Some(vec![]),
        agents: Some(vec![]),
        result_entries: Some(vec![]),
        ..MightyGraphRun::default()
    };
    for entry in session.logs() {
        if entry.kind() == "user" {
            if let Some(last) = runs.last_mut() {
                finish(last, "completed");
            }
            runs.push(empty(entry.id().into(), entry.text(), None));
        } else {
            if runs.is_empty() {
                runs.push(empty(format!("history-{}", session.id()), "", Some("completed")));
            }
            if let Some(last) = runs.last_mut() {
                last.root_entries.get_or_insert_with(Vec::new).push(entry.clone());
            }
        }
    }
    let status = if session.status() == "idle" { "completed" } else { session.status() };
    if let Some(last) = runs.last_mut() {
        finish(last, status);
    }
    runs
}

fn digest_of(hash: DefaultHasher) -> i32 {
    hash.finish() as i32
}

/// `MobileMightySupport.Digest(values)`: what the phone would see change in the runs — identities, statuses, how many records each block
/// holds with the newest one's id — never the streamed text, but the newest settled run's answer length.
pub fn digest_runs(values: &[MightyGraphRun]) -> i32 {
    let values = support::with_background_waiting(values);
    let values: &[MightyGraphRun] = &values;
    let mut hash = DefaultHasher::new();
    let offset = values.len().saturating_sub(MIGHTY_RUNS);
    if let Some(carried) = first_carried(values, offset) {
        for value in &values[carried..] {
            if settled(status(value.status())) {
                value.final_output().map(|o| o.encode_utf16().count()).hash(&mut hash);
            }
        }
    }
    for run in &values[offset..] {
        run.id().hash(&mut hash);
        run.status().hash(&mut hash);
        run.agents().len().hash(&mut hash);
        run.root_entries().len().hash(&mut hash);
        run.root_entries().last().map(LogEntry::id).hash(&mut hash);
        support::background_waiting(run).hash(&mut hash);
        run.notice_title.hash(&mut hash);
        for agent in run.agents() {
            agent.id().hash(&mut hash);
            agent.status().hash(&mut hash);
            agent.kind.hash(&mut hash);
            agent.entries().len().hash(&mut hash);
            agent.entries().last().map(LogEntry::id).hash(&mut hash);
            agent.notice_reply.as_ref().map(|r| r.encode_utf16().count()).hash(&mut hash);
            agent.figures.as_ref().and_then(|f| f.tool_calls).hash(&mut hash);
            agent.figures.as_ref().and_then(|f| f.ended_at.clone()).hash(&mut hash);
            agent.usage.as_ref().map(|u| u.total()).hash(&mut hash);
            agent.identity.as_ref().and_then(|i| i.resolved_model.clone()).hash(&mut hash);
        }
        for step in run.steps.as_deref().unwrap_or_default() {
            step.key.hash(&mut hash);
            step.status.hash(&mut hash);
            step.started_at.hash(&mut hash);
            step.ended_at.hash(&mut hash);
        }
        let calls: Vec<_> = run.advisor.as_ref().and_then(|a| a.calls.as_ref()).map(|c| c.iter().flatten().collect()).unwrap_or_default();
        run.advisor.as_ref().map(|a| a.total.unwrap_or(0).max(calls.len() as i32)).hash(&mut hash);
        run.advisor.as_ref().and_then(|a| a.usage.as_ref()).map(|u| u.total()).hash(&mut hash);
        for call in calls {
            call.id.hash(&mut hash);
            call.state.hash(&mut hash);
        }
    }
    digest_of(hash)
}

/// `MobileMightySupport.Digest(session)`: the digest of the very runs the payload carries; a pane with no saved graph is read from its
/// transcript by identity alone, walked backwards and stopped at the window the payload sends.
pub fn digest(session: &RunSession) -> i32 {
    if let Some(saved) = &session.graph_runs {
        return digest_runs(saved);
    }
    let logs = session.logs();
    let mut ids: Vec<(String, usize, Option<String>)> = Vec::new();
    let (mut entries, mut last) = (0usize, None::<String>);
    let mut index = logs.len() as isize - 1;
    while index >= 0 && ids.len() < MIGHTY_RUNS {
        let entry = &logs[index as usize];
        if entry.kind() == "user" {
            ids.push((entry.id().into(), entries, last.take()));
            entries = 0;
        } else {
            entries += 1;
            if last.is_none() {
                last = Some(entry.id().into());
            }
        }
        index -= 1;
    }
    if index < 0 && !logs.is_empty() && logs[0].kind() != "user" && ids.len() < MIGHTY_RUNS {
        ids.push((format!("history-{}", session.id()), entries, last));
    }
    let mut hash = DefaultHasher::new();
    if ids.is_empty() {
        return digest_of(hash);
    }
    ids.reverse();
    let status = if session.status() == "idle" { "completed" } else { session.status() };
    if settled(self::status(status)) {
        let mut length = None;
        if status == "completed" {
            for entry in logs.iter().rev().take_while(|e| e.kind() != "user") {
                if entry.kind() == "assistant" {
                    length = Some(entry.text().encode_utf16().count());
                    break;
                }
            }
        }
        length.hash(&mut hash);
    }
    let count = ids.len();
    for (at, (id, entries, last)) in ids.into_iter().enumerate() {
        id.hash(&mut hash);
        (if at == count - 1 { status } else { "completed" }).hash(&mut hash);
        0usize.hash(&mut hash);
        entries.hash(&mut hash);
        last.as_deref().hash(&mut hash);
        0i32.hash(&mut hash);
        None::<String>.hash(&mut hash);
        None::<i32>.hash(&mut hash);
        None::<i64>.hash(&mut hash);
    }
    digest_of(hash)
}

// ── the background strip (PlanCardSupport) ─────────────────────────────────────────────────────────────────────────────

/// `StylePresentation.TaskRow`: one background task as the plan style's task list draws it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRow {
    pub text: String,
    pub kind: String,
    pub kind_title: String,
    pub status: String,
    pub status_title: String,
    pub running: bool,
    pub started_at: Option<DateTimeOffset>,
    pub ended_at: Option<DateTimeOffset>,
}

impl TaskRow {
    /// `Elapsed(now)`: to now while it runs, to its end once it ended.
    pub fn elapsed(&self, now: DateTimeOffset, locale: &Locale) -> String {
        match self.started_at {
            Some(start) => elapsed_words(start, if self.running { now } else { self.ended_at.unwrap_or(now) }, locale),
            None => String::new(),
        }
    }
}

/// `StylePresentation.Elapsed`: seconds, minutes and seconds, or hours and minutes; a clock that went back reads as 0.
pub fn elapsed_words(start: DateTimeOffset, end: DateTimeOffset, locale: &Locale) -> String {
    let seconds = ((end.utc_ticks() - start.utc_ticks()) / 10_000_000).max(0);
    if seconds < 60 {
        return locale.get_with("styles.state.elapsedSeconds", &[("seconds", &seconds.to_string())]);
    }
    if seconds < 3600 {
        return locale.get_with(
            "styles.state.elapsedMinutes",
            &[("minutes", &(seconds / 60).to_string()), ("seconds", &(seconds % 60).to_string())],
        );
    }
    locale
        .get_with("styles.state.elapsedHours", &[("hours", &(seconds / 3600).to_string()), ("minutes", &(seconds % 3600 / 60).to_string())])
}

/// `StyleText.Banned`.
fn banned(c: u32) -> bool {
    c <= 31
        || (127..=159).contains(&c)
        || matches!(c, 0xad | 0x61c | 0x2060 | 0xfeff)
        || (0x200b..=0x200f).contains(&c)
        || (0x2028..=0x202e).contains(&c)
        || (0x2066..=0x2069).contains(&c)
}

/// `StyleText.Safe(value, limit)`: banned characters as U+FFFD, at most `limit` text elements (then "…").
fn style_safe(value: &str, limit: usize) -> String {
    let clean: String = value.chars().map(|c| if banned(c as u32) { '\u{fffd}' } else { c }).collect();
    let elements: Vec<usize> = clean.grapheme_indices(true).map(|(at, _)| at).collect();
    if elements.len() <= limit { clean } else { format!("{}…", &clean[..elements[limit - 1]]) }
}

/// `StylePresentation.Inline`: banned characters dropped (a zero-width joiner kept), trimmed, at most 200 characters.
fn style_inline(text: &str) -> String {
    let kept: String = text.chars().filter(|c| *c as u32 == 0x200d || !banned(*c as u32)).collect();
    kept.trim().chars().take(200).collect()
}

/// A task's kind as the task list reads it: agent, shell or other.
fn task_kind(kind: Option<&str>) -> &'static str {
    match kind {
        Some("agent") => "agent",
        Some("shell") => "shell",
        _ => "other",
    }
}

/// A task's status as the task list reads it: running, completed, failed, stopped or unknown.
fn task_status(status: Option<&str>) -> &'static str {
    match status {
        Some("running") => "running",
        Some("completed") => "completed",
        Some("failed") => "failed",
        Some("stopped") => "stopped",
        _ => "unknown",
    }
}

/// `PlanCardSupport.BackgroundTasks`: the pane's background tasks as the task list draws them, running first, at most eight.
pub fn background_tasks(work: Option<&BackgroundWork>, locale: &Locale) -> Vec<TaskRow> {
    let Some(work) = work else { return vec![] };
    let tasks = work.tasks.as_deref().unwrap_or_default();
    tasks
        .iter()
        .filter(|t| t.status.as_deref() == Some("running"))
        .chain(tasks.iter().filter(|t| t.status.as_deref() != Some("running")))
        .take(MAXIMUM_TASK_LIST_ITEMS)
        .map(|t| {
            let kind = task_kind(t.kind.as_deref());
            let status = task_status(t.status.as_deref());
            let kind_title = locale.get(match kind {
                "agent" => "styles.state.taskKind.agent",
                "shell" => "styles.state.taskKind.shell",
                _ => "styles.state.taskKind.other",
            });
            let text = style_inline(&style_safe(t.description.as_deref().unwrap_or(""), 400));
            TaskRow {
                text: if text.is_empty() { kind_title.clone() } else { text },
                kind: kind.into(),
                kind_title,
                status: status.into(),
                status_title: locale.get(&format!("styles.state.taskStatus.{status}")),
                running: status == "running",
                started_at: time::parse(t.started_at.as_deref()),
                ended_at: time::parse(t.ended_at.as_deref()),
            }
        })
        .collect()
}

/// `PlanCardSupport.BackgroundStripRow`: the task list's row, the kind words the diagram's block uses, a run's last output line, the
/// CLI's task id, whether it is a run, the command or first prompt line, and a run's last output lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackgroundStripRow {
    pub task: TaskRow,
    pub kind_text: Option<String>,
    pub tail: Option<String>,
    pub id: Option<String>,
    pub run: Option<bool>,
    pub command: Option<String>,
    pub tail_lines: Vec<String>,
}

impl BackgroundStripRow {
    fn of(task: TaskRow) -> BackgroundStripRow {
        BackgroundStripRow { task, kind_text: None, tail: None, id: None, run: None, command: None, tail_lines: vec![] }
    }

    /// `Opens`: whether opening the row shows anything more.
    pub fn opens(&self) -> bool {
        self.command.is_some() || !self.tail_lines.is_empty()
    }

    /// `Detail(now)`: kind · status · elapsed (kind words already say waiting or how it ended).
    pub fn detail(&self, now: DateTimeOffset, locale: &Locale) -> String {
        let elapsed = self.task.elapsed(now, locale);
        let parts: Vec<&str> = match &self.kind_text {
            Some(kind) => vec![kind, &elapsed],
            None => vec![&self.task.kind_title, &self.task.status_title, &elapsed],
        };
        parts.into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ")
    }
}

/// `PlanCardSupport.BackgroundStripRows`: the strip's rows in the task list's order, each meeting its graph block in `run` (the pane's
/// latest request) by tool use id.
pub fn background_strip_rows(
    work: Option<&BackgroundWork>,
    run: Option<&MightyGraphRun>,
    output: Option<&OutputTails>,
    locale: &Locale,
) -> Vec<BackgroundStripRow> {
    let Some(work) = work else { return vec![] };
    let tasks = background_tasks(Some(work), locale);
    let all = work.tasks.as_deref().unwrap_or_default();
    let sources: Vec<&BackgroundTask> = all
        .iter()
        .filter(|t| t.status.as_deref() == Some("running"))
        .chain(all.iter().filter(|t| t.status.as_deref() != Some("running")))
        .collect();
    let matches = tasks.len() == sources.len().min(MAXIMUM_TASK_LIST_ITEMS)
        && tasks.iter().zip(&sources).all(|(task, source)| {
            let kind = task_kind(source.kind.as_deref());
            kind == task.kind && (source.status.as_deref() == Some("running")) == task.running
        });
    if !matches {
        return tasks.into_iter().map(BackgroundStripRow::of).collect();
    }
    let run_id = run.map(|r| r.source_run_id.as_deref().unwrap_or(r.id()));
    let mut blocks: HashMap<&str, &MightyGraphAgent> = HashMap::new();
    for agent in run.map(|r| r.agents()).unwrap_or_default() {
        blocks.entry(agent.id()).or_insert(agent);
    }
    tasks
        .into_iter()
        .zip(sources)
        .map(|(task, source)| {
            let block = match (source.tool_use_id.as_deref(), run_id) {
                (Some(tool), Some(run_id)) => blocks.get(node::agent_node_id(run_id, tool).as_str()).copied(),
                _ => None,
            };
            let is_run = match block {
                Some(block) if block.is_task() || block.is_background_agent() => block.is_task(),
                _ if matches!(task.kind.as_str(), "agent" | "shell") => task.kind == "shell",
                _ => return BackgroundStripRow { id: source.id.clone(), ..BackgroundStripRow::of(task) },
            };
            let lines: Vec<String> = match (is_run, block, output) {
                (true, Some(block), Some(output)) => output
                    .get(block.id())
                    .filter(|all| !all.is_empty())
                    .map(|all| all[all.len().saturating_sub(OUTPUT_TAIL_LINES)..].to_vec())
                    .unwrap_or_default(),
                _ => vec![],
            };
            BackgroundStripRow {
                kind_text: Some(support::background_label_of(is_run, &task.status, locale)),
                tail: lines.last().cloned(),
                id: source.id.clone(),
                run: Some(is_run),
                command: block.and_then(|b| strip_command(b, is_run)),
                tail_lines: lines,
                task,
            }
        })
        .collect()
}

/// `PlanCardSupport.StripCommand`: a run's whole command, a sub-agent's first prompt line.
pub fn strip_command(block: &MightyGraphAgent, run: bool) -> Option<String> {
    let text = if run {
        support::shown_input(block).trim().to_string()
    } else {
        block.input().split('\n').map(|line| line.trim_matches(['\r', ' ', '\t'])).find(|line| !line.is_empty()).unwrap_or("").to_string()
    };
    (!text.is_empty()).then_some(text)
}

/// `BackgroundWork.Running`: the tasks still running.
pub fn running_tasks(work: &BackgroundWork) -> Vec<&BackgroundTask> {
    work.tasks.as_deref().unwrap_or_default().iter().filter(|t| t.status.as_deref() == Some("running")).collect()
}

/// `PlanCardSupport.NoticeDelivered`.
fn notice_delivered(status: Option<&str>) -> bool {
    matches!(status, Some("completed" | "failed" | "stopped"))
}

/// `PlanCardSupport.ShowsBackgroundStrip(work, mighty: true, styleDrawsTasks: false)`.
fn shows_strip_in_mighty(work: &BackgroundWork) -> bool {
    work.tasks
        .as_deref()
        .unwrap_or_default()
        .iter()
        .any(|t| t.status.as_deref() == Some("running") || notice_delivered(t.status.as_deref()))
}

/// `PlanCardSupport.MobileBackgroundDone`: the strip's finished count whenever the Mighty view's strip shows, 0 included; None otherwise.
pub fn mobile_background_done(work: Option<&BackgroundWork>) -> Option<i32> {
    let work = work.filter(|w| shows_strip_in_mighty(w))?;
    Some((work.tasks.as_deref().unwrap_or_default().len() - running_tasks(work).len()) as i32)
}

/// One running background task as the phone's `backgroundWork` carries it (C# `MobileBackgroundWork`; mighty-remote `phone::desk` has
/// the wire record): id, "run" or "agent", title, started at, command and output lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobileBackgroundRow {
    pub id: String,
    pub run: bool,
    pub title: String,
    pub started_at: String,
    pub command: Option<String>,
    pub output_tail: Option<Vec<String>>,
}

/// `MobileBackgroundWork.MaximumTitle`, `MaximumCommand`.
pub const BACKGROUND_TITLE: usize = 1_000;
pub const BACKGROUND_COMMAND: usize = 2_000;

/// `PlanCardSupport.MobileBackgroundWork`: the strip's running rows, each with its command and output lines; never a path. A task that is
/// neither a sub-agent nor a command is left out. None when nothing of them runs.
pub fn mobile_background_work(
    work: Option<&BackgroundWork>,
    run: Option<&MightyGraphRun>,
    output: Option<&OutputTails>,
    locale: &Locale,
) -> Option<Vec<MobileBackgroundRow>> {
    let work = work?;
    let running = running_tasks(work);
    if running.is_empty() {
        return None;
    }
    let mut rows: HashMap<String, BackgroundStripRow> = HashMap::new();
    for row in background_strip_rows(Some(work), run, output, locale) {
        if let Some(id) = row.id.clone() {
            rows.entry(id).or_insert(row);
        }
    }
    let mut items = vec![];
    for task in running {
        let id = task.id.clone().unwrap_or_default();
        let row = rows.get(&id);
        let is_run = match row.and_then(|r| r.run) {
            Some(run) => run,
            None => match task.kind.as_deref() {
                Some("shell") => true,
                Some("agent") => false,
                _ => continue,
            },
        };
        let title = clean(row.map_or(task.description.as_deref().unwrap_or(""), |r| r.task.text.as_str()), BACKGROUND_TITLE, true);
        let command = row.and_then(|r| r.command.as_deref()).map(|text| clean(text, BACKGROUND_COMMAND, false)).filter(|c| !c.is_empty());
        items.push(MobileBackgroundRow {
            id,
            run: is_run,
            title,
            started_at: task.started_at.clone().unwrap_or_default(),
            command,
            output_tail: row.filter(|r| !r.tail_lines.is_empty()).map(|r| r.tail_lines.clone()),
        });
    }
    (!items.is_empty()).then_some(items)
}
