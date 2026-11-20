//! A request's checklist for agents that keep no plan unless asked: the switch "에이전트에게 진행 체크리스트 요청" (`AppSnapshot`'s
//! `agent_checklist`, on unless switched off) asks Antigravity, Codex, opencode and Hermes to keep one ([`instruction`]), and while a
//! request has no plan its steps are the main agent's own work, one per tool call ([`ActivityList`], [`codex_activity`],
//! [`acp_activity`], [`pi_activity`]; Antigravity's in `antigravity::steps`). A plan, once one arrives, replaces the activity
//! (`plan_mode::TodoProgressTracker`).
//!
//! Every instruction reaches only the agent: Codex's as `developer_instructions` (or, when the user's own Codex config sets those, after
//! the prompt), the ACP agents' after the prompt they are sent, agy's after its stdin line. The transcript and the request's block keep
//! what the user typed.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use mighty_core::model::TodoItem;
use serde_json::Value;

use crate::activity;
use crate::meta;
use crate::model::StartRunRequest;
use crate::plan_mode::todo_item;
use crate::strings;

// ── what the app adds after a request ───────────────────────────────────────────────────────────────────────────────────

/// How every note the app adds after an ACP agent's prompt starts (Hermes's clarify note, the checklist words).
macro_rules! acp_note_start {
    () => {
        "[Note from the Mighty Code app, not from the user:"
    };
}
/// How agy's checklist words start.
macro_rules! agy_checklist_start {
    () => {
        "[Mighty Code] Keep a checklist of this request"
    };
}
/// How Codex's checklist words start (after its prompt only when its developer instructions cannot carry them).
macro_rules! codex_checklist_start {
    () => {
        "Mighty Code shows your plan as this request's checklist."
    };
}
pub(crate) use {acp_note_start, agy_checklist_start};

pub const ACP_NOTE_START: &str = acp_note_start!();
pub const AGY_CHECKLIST_START: &str = agy_checklist_start!();
pub const CODEX_CHECKLIST_START: &str = codex_checklist_start!();

/// How each note the app may add after a request starts: the words every instruction is built from, so the writer and
/// [`without_app_notes`] cannot drift apart.
pub const APPENDED_NOTE_STARTS: [&str; 3] = [ACP_NOTE_START, AGY_CHECKLIST_START, CODEX_CHECKLIST_START];

/// A stored user message without the notes the app added after it (each its own paragraph at the end, starting with one of
/// [`APPENDED_NOTE_STARTS`]), trimmed; a message with none is returned as it is. The same words inside what the user wrote, or in a
/// paragraph the user wrote after them, are left alone.
pub fn without_app_notes(text: &str) -> String {
    let mut end = text.trim_end().len();
    let mut stripped = false;
    loop {
        let body = &text[..end];
        let start = body.rfind("\n\n").map_or(0, |at| at + 2);
        let paragraph = body[start..].trim_start();
        if !APPENDED_NOTE_STARTS.iter().any(|note| paragraph.starts_with(note)) {
            break;
        }
        stripped = true;
        end = if start == 0 { 0 } else { body[..start - 2].trim_end().len() };
        if end == 0 {
            break;
        }
    }
    if stripped { text[..end].trim().to_string() } else { text.to_string() }
}

/// Codex: its `update_plan` tool is the turn's plan (exec's `todo_list` item, the app-server's `turn/plan/updated`).
pub const CODEX_INSTRUCTION: &str = concat!(
    codex_checklist_start!(),
    " When a request takes more than one step, call update_plan before any other work with one short step per item, in the user's language, then call it again to mark each step in_progress when you start it and completed as soon as it is done, adding or removing steps when the plan changes. Skip the plan for a request that takes a single step, and never mention it in your answer."
);

/// opencode: its `todowrite` tool becomes the ACP `plan` update.
pub const OPENCODE_INSTRUCTION: &str = concat!(
    acp_note_start!(),
    " the app shows your todo list as this request's checklist. When the request takes more than one step, write the todo list with the todowrite tool before any other work, one short todo per step in the user's language, then update it with todowrite to mark each todo in_progress when you start it and completed as soon as it is done. Skip it for a request that takes a single step, and never mention it in your answer.]"
);

/// Hermes: its `todo` tool becomes the ACP `plan` update.
pub const HERMES_INSTRUCTION: &str = concat!(
    acp_note_start!(),
    " the app shows your todo list as this request's checklist. When the request takes more than one step, write the todo list with your todo tool before any other work, one short item per step in the user's language, then update it with the todo tool to mark each item in_progress when you start it and completed as soon as it is done. Skip it for a request that takes a single step, and never mention it in your answer.]"
);

/// Whether the switch asks `provider`'s agent for a checklist (pi has no plan tool: its steps are its tool calls).
pub fn asks(provider: &str) -> bool {
    matches!(provider, "gemini" | "codex" | "opencode" | "hermes")
}

/// Whether this request carries the instruction: the switch on for a local pane of an agent it asks, and no slash command.
pub fn wanted(request: &StartRunRequest) -> bool {
    request.checklist && asks(&request.provider) && !request.input.trim_start().starts_with('/')
}

/// The words `provider` is told after its prompt (Codex's when its developer instructions cannot carry them).
pub fn instruction(provider: &str) -> Option<&'static str> {
    match provider {
        "gemini" => Some(crate::antigravity::steps::CHECKLIST_INSTRUCTION),
        "codex" => Some(CODEX_INSTRUCTION),
        "opencode" => Some(OPENCODE_INSTRUCTION),
        "hermes" => Some(HERMES_INSTRUCTION),
        _ => None,
    }
}

/// `input` with `provider`'s instruction after it when [`wanted`].
pub fn with_instruction(input: &str, request: &StartRunRequest) -> String {
    match instruction(&request.provider).filter(|_| wanted(request)) {
        Some(words) => format!("{input}\n\n{words}"),
        None => input.to_string(),
    }
}

/// Whether the user's own Codex config (`$CODEX_HOME/config.toml`, else `~/.codex/config.toml`, and the project's `.codex/config.toml`)
/// sets `developer_instructions`, which `-c developer_instructions=…` would replace: the instruction then goes after the prompt.
pub fn codex_sets_developer_instructions(environment: &BTreeMap<String, String>, home: Option<&Path>, workspace: &Path) -> bool {
    let codex_home = environment.get("CODEX_HOME").filter(|v| !v.is_empty()).map(PathBuf::from).or_else(|| home.map(|h| h.join(".codex")));
    codex_home
        .into_iter()
        .chain(std::iter::once(workspace.join(".codex")))
        .any(|folder| sets_developer_instructions(&folder.join("config.toml")))
}

fn sets_developer_instructions(config: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(config) else { return false };
    text.lines().take(10_000).map(str::trim).take_while(|line| !line.starts_with('[')).any(|line| {
        line.strip_prefix("developer_instructions").is_some_and(|rest| rest.trim_start().starts_with('='))
            || line.strip_prefix("\"developer_instructions\"").is_some_and(|rest| rest.trim_start().starts_with('='))
    })
}

// ── activity steps ──────────────────────────────────────────────────────────────────────────────────────────────────────

/// The prefix of an activity step's id: such steps are this request's only, never a saved checklist's.
pub const ACTIVITY_ID: &str = "activity:";

/// The most activity steps kept for one request (the panel and the saved checklist keep 100).
const MAXIMUM_STEPS: usize = 256;
/// The most characters of a step's detail (a file name, a command line).
const MAXIMUM_DETAIL_CHARS: usize = 80;

/// Whether `item` is an activity step.
pub fn is_activity(item: &TodoItem) -> bool {
    item.id.as_deref().is_some_and(|id| id.starts_with(ACTIVITY_ID))
}

/// One step of the main agent's work: its key (a tool call's id), its words and whether it ended.
#[derive(Debug, Clone, PartialEq)]
pub struct Activity {
    pub key: String,
    pub text: String,
    pub ended: bool,
}

/// The main agent's tool calls in order, each running until it ended (a failed call ends too: the step model has no failed step).
#[derive(Debug, Clone, Default)]
pub struct ActivityList {
    items: Vec<TodoItem>,
}

impl ActivityList {
    pub fn items(&self) -> &[TodoItem] {
        &self.items
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// A call seen (again); whether the list changed.
    pub fn note(&mut self, activity: &Activity) -> bool {
        let id = format!("{ACTIVITY_ID}{}", activity.key);
        let status = if activity.ended { "completed" } else { "in_progress" };
        if let Some(item) = self.items.iter_mut().find(|i| i.id.as_deref() == Some(id.as_str())) {
            // A later update may name what the call works on (opencode announces a call before its input); one that names less
            // does not take the name back.
            let named = item.status.as_deref() != Some("completed")
                && !activity.text.is_empty()
                && item.content.as_deref() != Some(activity.text.as_str())
                && (activity.text.contains(" · ") || !item.content.as_deref().unwrap_or("").contains(" · "));
            if named {
                item.content = Some(activity.text.clone());
            }
            if item.status.as_deref() == Some("completed") || item.status.as_deref() == Some(status) {
                return named;
            }
            item.status = Some(status.into());
            return true;
        }
        if activity.text.is_empty() {
            return false;
        }
        if self.items.len() >= MAXIMUM_STEPS {
            let oldest = self.items.iter().position(|i| i.status.as_deref() == Some("completed")).unwrap_or(0);
            self.items.remove(oldest);
        }
        self.items.push(todo_item(&activity.text, status, None, Some(&id)));
        true
    }
}

/// The plan tools themselves are no step of the work.
fn plan_tool(name: &str) -> bool {
    matches!(name, "todowrite" | "todoread" | "todo" | "todo_list" | "update_plan" | "write_todos" | "todo_write")
}

/// A Codex `item.started` / `item.completed` frame's step (exec's shapes; an approval run's items are mapped to them): a command, a
/// file change (its files), an MCP tool call or a web search.
pub fn codex_activity(frame: &Value) -> Option<Activity> {
    let kind = meta::text(frame, "type")?;
    if !matches!(kind, "item.started" | "item.updated" | "item.completed") {
        return None;
    }
    let item = meta::prop(frame, "item")?;
    let id = meta::text(item, "id").filter(|i| !i.is_empty())?;
    let status = meta::text(item, "status").unwrap_or("");
    let ended = kind == "item.completed" || matches!(status, "completed" | "failed" | "declined");
    let text = match meta::text(item, "type")? {
        "command_execution" => with_detail(&strings::get("antigravity.permission.kind.command"), &shell_line(meta::text(item, "command")?)),
        "file_change" => {
            let changes = match meta::prop(item, "changes") {
                Some(Value::Array(changes)) => changes.as_slice(),
                _ => &[],
            };
            let all_added = !changes.is_empty()
                && changes.iter().all(|c| meta::text(c, "kind").or_else(|| meta::text_of(meta::prop(c, "kind"), "type")) == Some("add"));
            let names: Vec<&str> = changes.iter().take(8).filter_map(|c| meta::text(c, "path")).map(file_name).collect();
            let label = strings::get(if all_added { "permission.tool.write" } else { "permission.tool.edit" });
            with_detail(&label, &names.join(", "))
        }
        "mcp_tool_call" => {
            let server = meta::text(item, "server").unwrap_or("");
            let label = strings::get_with("permission.tool.mcp", &[("server", server)]);
            with_detail(&label, meta::text(item, "tool").unwrap_or(""))
        }
        "web_search" => with_detail(&strings::get("permission.tool.webSearch"), meta::text(item, "query").unwrap_or("")),
        _ => return None,
    };
    Some(Activity { key: format!("codex:{id}"), text, ended })
}

/// A shell wrapper's line (`/bin/zsh -lc 'ls -la'`, Codex's form) as the command it runs.
fn shell_line(command: &str) -> String {
    let trimmed = command.trim();
    for shell in ["bash -lc ", "zsh -lc ", "sh -lc ", "bash -c ", "zsh -c ", "sh -c "] {
        if let Some(at) = trimmed.find(shell) {
            let before = &trimmed[..at];
            if before.is_empty() || before.ends_with('/') {
                let rest = trimmed[at + shell.len()..].trim();
                let unquoted = rest
                    .strip_prefix('\'')
                    .and_then(|r| r.strip_suffix('\''))
                    .or_else(|| rest.strip_prefix('"').and_then(|r| r.strip_suffix('"')));
                return unquoted.unwrap_or(rest).to_string();
            }
        }
    }
    trimmed.to_string()
}

/// An ACP `tool_call` / `tool_call_update`'s step (opencode, Hermes): what its kind says it does, on what its input, locations or title
/// name. A call still pending (asking) is running; a completed or failed one ended.
pub fn acp_activity(frame: &Value) -> Option<Activity> {
    if meta::text(frame, "type") != Some("acp.update") {
        return None;
    }
    let update = meta::prop(frame, "update")?;
    if !matches!(meta::text(update, "sessionUpdate"), Some("tool_call" | "tool_call_update")) {
        return None;
    }
    let id = meta::text(update, "toolCallId").filter(|i| !i.is_empty())?;
    let ended = matches!(meta::text(update, "status"), Some("completed" | "failed"));
    let (kind, title) = (meta::text(update, "kind"), meta::text(update, "title"));
    let named = crate::acp::titled_tool(title).map(|t| t.to_lowercase());
    if named.as_deref().is_some_and(plan_tool) || meta::text(update, "name").is_some_and(|n| plan_tool(&n.to_lowercase())) {
        return None;
    }
    // An update that says nothing of itself only moves a step already seen.
    if kind.is_none() && title.is_none() && meta::prop(update, "rawInput").is_none() {
        return Some(Activity { key: format!("acp:{id}"), text: String::new(), ended });
    }
    if kind == Some("think") {
        return None;
    }
    // An edit whose tool writes a whole file (opencode's `write`, Hermes's `write_file`) is a write.
    let tool = match named.as_deref() {
        Some(name) if kind == Some("edit") && name.contains("write") => "write".to_string(),
        _ => crate::acp::tool_name(kind, named.as_deref().or(title)),
    };
    let mut input = crate::acp::display_input(meta::prop(update, "rawInput"));
    // The call's location is its file, as the agent resolved it (opencode's input may say only ".").
    if let (Value::Object(fields), Some(Value::Array(locations))) = (&mut input, meta::prop(update, "locations"))
        && let Some(path) = locations.iter().find_map(|l| meta::text(l, "path"))
    {
        fields.insert("path".into(), Value::String(path.into()));
    }
    let mut text = step_text(&tool, Some(&input));
    // No input to name what it works on: the title does, without its tool's name ("terminal: npm test" → "npm test").
    if !text.contains(" · ")
        && let Some(title) = title.filter(|t| !t.trim().is_empty() && !t.eq_ignore_ascii_case(&tool))
    {
        let detail = match (title.split_once(": "), crate::acp::titled_tool(Some(title))) {
            (Some((name, rest)), Some(titled)) if titled == name => rest,
            _ => title,
        };
        text = with_detail(&text, detail);
    }
    Some(Activity { key: format!("acp:{id}"), text, ended })
}

/// pi's `tool_execution_start` / `tool_execution_end` (read, bash, edit, write); its question tool is no step.
pub fn pi_activity(frame: &Value) -> Option<Activity> {
    let kind = meta::text(frame, "type")?;
    if !matches!(kind, "tool_execution_start" | "tool_execution_update" | "tool_execution_end") {
        return None;
    }
    let id = meta::text(frame, "toolCallId").filter(|i| !i.is_empty())?;
    let tool = meta::text(frame, "toolName").unwrap_or("Tool");
    if tool == crate::providers::PI_QUESTION_TOOL {
        return None;
    }
    let input = crate::acp::display_input(meta::prop(frame, "args"));
    Some(Activity { key: format!("pi:{id}"), text: step_text(tool, Some(&input)), ended: kind == "tool_execution_end" })
}

// ── a step's words ──────────────────────────────────────────────────────────────────────────────────────────────────────

pub(crate) fn with_detail(label: &str, detail: &str) -> String {
    let detail = activity::clean(Some(detail), 4096, true);
    let detail = detail.trim();
    if detail.is_empty() {
        return label.to_string();
    }
    let shown: String = if detail.chars().count() > MAXIMUM_DETAIL_CHARS {
        format!("{}…", detail.chars().take(MAXIMUM_DETAIL_CHARS - 1).collect::<String>())
    } else {
        detail.to_string()
    };
    format!("{label} · {shown}")
}

pub(crate) fn components(path: &str) -> Vec<&str> {
    path.split(['/', '\\']).filter(|c| !c.is_empty()).collect()
}

/// A path's last part (a file or folder name).
pub(crate) fn file_name(path: &str) -> &str {
    components(path).last().copied().unwrap_or(path)
}

fn first_text(parameters: Option<&Value>, keys: &[&str]) -> Option<String> {
    let parameters = parameters?;
    keys.iter().find_map(|key| meta::text(parameters, key)).filter(|t| !t.trim().is_empty()).map(str::to_string)
}

/// A file tool's path among its (snake-cased) parameters.
pub(crate) fn path_of(parameters: Option<&Value>) -> Option<String> {
    let parameters = parameters?;
    ["path", "file_path", "target_file", "absolute_path", "filepath", "file", "directory_path", "search_path"]
        .iter()
        .find_map(|key| meta::text(parameters, key))
        .filter(|p| !p.is_empty())
        .map(str::to_string)
}

/// A tool that writes a file (agy's stream names `write_file` and `edit_file`; its tool list `write_to_file`, `replace_file_content`, …).
pub(crate) fn writes(tool: &str) -> bool {
    tool.contains("write") || tool.contains("edit") || tool.contains("replace") || matches!(tool, "sed_file" | "apply_patch" | "patch")
}

/// A tool call's step, in the app's language: what it does and on what ("파일 읽기 · calc.py", "명령 · ls -la"). `parameters` are the
/// call's, snake-cased as the transcript's rows read them.
pub fn step_text(tool: &str, parameters: Option<&Value>) -> String {
    let name = tool.to_lowercase();
    let kind = activity::kind(&name);
    let file = || path_of(parameters).map(|p| file_name(&p).to_string());
    let (label, detail) = if kind == "agent" || name.contains("subagent") {
        (strings::get("graph.block.agent"), first_text(parameters, &["name", "role", "type_name", "description"]))
    } else if matches!(name.as_str(), "list_dir" | "list_directory" | "ls" | "list_resources") {
        (strings::get("run.activityStep.list"), file())
    } else if kind == "read" || name.starts_with("read_file") || name.starts_with("view_") {
        (strings::get("permission.tool.read"), file())
    } else if name.contains("write") {
        (strings::get("permission.tool.write"), file())
    } else if kind == "edit" || writes(&name) {
        (strings::get("permission.tool.edit"), file())
    } else if kind == "command" {
        let command = first_text(parameters, &["command_line", "command", "cmd", "input"]);
        (strings::get("antigravity.permission.kind.command"), command.map(|c| c.lines().next().unwrap_or("").to_string()))
    } else if matches!(name.as_str(), "find_by_name" | "glob" | "find" | "glob_search") {
        (strings::get("permission.tool.glob"), first_text(parameters, &["pattern", "query", "search_pattern"]))
    } else if kind == "search" {
        (strings::get("permission.tool.grep"), first_text(parameters, &["query", "pattern", "search_pattern"]))
    } else if name.contains("search") && kind == "web" {
        (strings::get("permission.tool.webSearch"), first_text(parameters, &["query", "url"]))
    } else if matches!(name.as_str(), "read_url_content" | "web_fetch" | "webfetch") {
        (strings::get("permission.tool.webFetch"), first_text(parameters, &["url", "query"]))
    } else if kind == "web" {
        (strings::get("run.activityStep.browser"), first_text(parameters, &["url", "query"]))
    } else {
        (strings::get("permission.tool.tool"), Some(tool.to_string()))
    };
    with_detail(&label, detail.as_deref().unwrap_or(""))
}
