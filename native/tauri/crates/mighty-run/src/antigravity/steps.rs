//! An Antigravity request's steps for the progress panel. agy sends no plan in its stream, so the checklist is read, per request, from
//! the best of three sources, a higher one replacing a lower one:
//!
//! 1. `task.md` in the conversation's artifact folder (`~/.gemini/antigravity-cli/brain/<conversation>/task.md`), read again after
//!    every write the stream reports finished: its `- [ ]` / `- [/]` / `- [x]` lines (an indented line is a step of its own, named
//!    after its top-level line). agy keeps one in planning mode, and when the switch "에이전트에게 진행 체크리스트 요청" is on the
//!    request asks for one ([`CHECKLIST_INSTRUCTION`], `crate::checklist`).
//! 2. `task_boundary` steps (agy's binary knows the step type: TaskName, TaskStatus, TaskSummary): each new task name is the current
//!    step, the one before it done.
//! 3. The main agent's own work: one step per tool call and sub-agent started, in order ("파일 읽기 · calc.py").

use std::io::Read;
use std::path::Path;

use mighty_core::model::TodoItem;
use serde_json::Value;

use super::records;
use crate::checklist::{Activity, ActivityList, components, path_of, with_detail, writes};
use crate::meta;
use crate::plan_mode::{MAXIMUM_TODO_ITEMS, todo_item};
use crate::strings;

pub use crate::checklist::step_text;

/// What a request is told when the switch is on, after what the user typed (only agy reads it; the transcript keeps the request as
/// typed). Checked with agy 1.5.13: it writes `task.md` in the conversation's brain folder first and edits it after each step.
pub const CHECKLIST_INSTRUCTION: &str = concat!(
    crate::checklist::agy_checklist_start!(),
    " in a file named task.md in your artifact directory for this conversation (~/.gemini/antigravity-cli/brain/<this conversation id>/task.md). Write one line per step in the user's language: `- [ ] step` to do, `- [/] step` in progress, `- [x] step` done. Write it before you start the work, then update it as each step starts and finishes. Skip it when the request takes a single step. Never mention the checklist or task.md in your answer."
);

/// The most task boundaries kept for one request (the panel and the saved checklist keep 100).
const MAXIMUM_STEPS: usize = 256;
/// The most of `task.md` read.
const MAXIMUM_TASK_FILE_BYTES: u64 = 256 * 1024;

/// Tools that are no step of the work.
const QUIET_TOOLS: [&str; 7] =
    ["finish", "wait", "wait_5_seconds", "ask_question", "ask_permission", "ask_custom_permission", "list_permissions"];

/// One request's checklist as agy's stream builds it.
#[derive(Debug, Clone, Default)]
pub struct AntigravitySteps {
    /// One per tool call or sub-agent start, keyed by its step (`activity:agy:<step_index>`).
    activity: ActivityList,
    boundaries: Vec<TodoItem>,
    task_file: Option<Vec<TodoItem>>,
}

impl AntigravitySteps {
    /// One line of agy's stream; the request's checklist when this line changed it.
    pub fn consume(&mut self, frame: &Value) -> Option<Vec<TodoItem>> {
        if meta::text(frame, "event") != Some("step_update") {
            return None;
        }
        let payload = meta::prop(frame, "step_update")?;
        let state = meta::text(payload, "state").unwrap_or("").to_uppercase();
        let kind = meta::text(payload, "step_type").unwrap_or("").to_lowercase();
        let changed = if kind.contains("task_boundary") {
            self.boundary(payload)
        } else if kind == "tool" {
            self.tool(payload, &state)
        } else if kind == "subagent" {
            self.subagent(payload, &state)
        } else {
            false
        };
        changed.then(|| self.list())
    }

    /// The checklist: task.md's, else the task boundaries, else the activity.
    pub fn list(&self) -> Vec<TodoItem> {
        if let Some(list) = &self.task_file {
            return list.clone();
        }
        if !self.boundaries.is_empty() {
            return self.boundaries.clone();
        }
        self.activity.items().to_vec()
    }

    fn tool(&mut self, payload: &Value, state: &str) -> bool {
        let info = meta::prop(payload, "tool_info");
        let tool = info.and_then(|i| meta::text(i, "name")).or_else(|| meta::text(payload, "tool_name")).unwrap_or("").to_string();
        let lowered = tool.to_lowercase();
        if lowered == "task_boundary" {
            return self.boundary(payload);
        }
        let parameters = records::antigravity_parameters(info.and_then(|i| meta::prop(i, "parameters")));
        // The artifacts (task.md, implementation_plan.md, …) are not the work; a finished write of task.md is the checklist.
        if let Some(path) = path_of(parameters.as_ref()) {
            let conversation = meta::text(payload, "conversation_id");
            if artifact_conversation(&path).is_some() {
                let failed = !matches!(info.and_then(|i| meta::prop(i, "error")), None | Some(Value::Null));
                if state == "DONE" && !failed && writes(&lowered) && is_task_file(&path, conversation) {
                    return self.read_task_file(Path::new(&path));
                }
                return false;
            }
        }
        if lowered.is_empty() || QUIET_TOOLS.contains(&lowered.as_str()) {
            return false;
        }
        let Some(index) = meta::integer(payload, "step_index") else { return false };
        let text = step_text(&tool, parameters.as_ref());
        self.note_activity(index, &text, state)
    }

    fn subagent(&mut self, payload: &Value, state: &str) -> bool {
        let Some(index) = meta::integer(payload, "step_index") else { return false };
        let specs = meta::prop(payload, "subagent_info").and_then(|i| meta::prop(i, "subagents"));
        let names: Vec<String> = match specs {
            Some(Value::Array(specs)) => {
                specs.iter().take(8).filter_map(|spec| records::title(meta::text(spec, "role"), meta::text(spec, "type_name"))).collect()
            }
            _ => vec![],
        };
        let label = strings::get("graph.block.agent");
        let text = if names.is_empty() { label } else { with_detail(&label, &names.join(", ")) };
        self.note_activity(index, &text, state)
    }

    /// A tool call or sub-agent start seen again: running until it is done (or failed: the step model has no failed step, so done).
    fn note_activity(&mut self, index: i64, text: &str, state: &str) -> bool {
        self.activity.note(&Activity { key: format!("agy:{index}"), text: text.into(), ended: matches!(state, "DONE" | "ERROR") })
    }

    /// A task boundary: its name the current step (the previous one done), its status the words shown while it runs.
    fn boundary(&mut self, payload: &Value) -> bool {
        let info = meta::prop(payload, "tool_info");
        let sources = [
            Some(payload),
            meta::prop(payload, "task_boundary"),
            meta::prop(payload, "task_boundary_info"),
            meta::prop(payload, "parameters"),
            info,
            info.and_then(|i| meta::prop(i, "parameters")),
        ];
        let name = boundary_field(&sources, &["task_name", "TaskName", "taskName"]);
        let status = boundary_field(&sources, &["task_status", "TaskStatus", "taskStatus"]);
        let same = matches!((&name, self.boundaries.last()), (Some(name), Some(last)) if last.content.as_deref() == Some(name.as_str()));
        match name {
            Some(name) if !same => {
                if let Some(last) = self.boundaries.last_mut() {
                    last.status = Some("completed".into());
                }
                if self.boundaries.len() >= MAXIMUM_STEPS {
                    self.boundaries.remove(0);
                }
                self.boundaries.push(todo_item(&name, "in_progress", status.as_deref(), None));
                true
            }
            _ => {
                let (Some(status), Some(last)) = (status, self.boundaries.last_mut()) else { return false };
                if last.active_form.as_deref() == Some(status.as_str()) {
                    return false;
                }
                last.active_form = Some(status);
                true
            }
        }
    }

    fn read_task_file(&mut self, path: &Path) -> bool {
        let Ok(file) = std::fs::File::open(path) else { return false };
        let mut bytes = Vec::new();
        if file.take(MAXIMUM_TASK_FILE_BYTES).read_to_end(&mut bytes).is_err() {
            return false;
        }
        let list = parse_task_file(&String::from_utf8_lossy(&bytes));
        if list.is_empty() || self.task_file.as_ref() == Some(&list) {
            return false;
        }
        self.task_file = Some(list);
        true
    }
}

/// `TaskName` and its kind from the first place that has one; agy's `%SAME%` keeps the one before.
fn boundary_field(sources: &[Option<&Value>], keys: &[&str]) -> Option<String> {
    sources.iter().flatten().find_map(|source| {
        keys.iter().find_map(|key| meta::text(source, key)).map(str::trim).filter(|v| !v.is_empty() && *v != "%SAME%").map(str::to_string)
    })
}

/// The conversation whose artifact folder (`…/antigravity-cli/brain/<conversation>/…`) holds `path`, if any.
pub fn artifact_conversation(path: &str) -> Option<String> {
    let parts = components(path);
    (2..parts.len().saturating_sub(1)).find_map(|at| {
        let folder = parts[at - 2].to_lowercase();
        (matches!(folder.as_str(), "antigravity-cli" | "antigravity") && parts[at - 1].eq_ignore_ascii_case("brain"))
            .then(|| records::conversation_id(Some(parts[at])).map(str::to_string))
            .flatten()
    })
}

/// Whether `path` is `task.md` straight in a conversation's artifact folder (`conversation`'s, when the step names one).
pub fn is_task_file(path: &str, conversation: Option<&str>) -> bool {
    let parts = components(path);
    let Some(name) = parts.last() else { return false };
    if !name.eq_ignore_ascii_case("task.md") || parts.len() < 4 {
        return false;
    }
    let folder = parts[parts.len() - 2];
    artifact_conversation(path).as_deref() == Some(folder)
        && conversation.filter(|c| !c.is_empty()).is_none_or(|c| c.eq_ignore_ascii_case(folder))
}

/// `task.md`'s checklist: each `- [ ]` (to do), `- [/]` (in progress) or `- [x]` (done) line, `*`, `+` and `1.` bullets too. An
/// indented line is a step of its own, named after the top-level line above it ("구현 › 테스트").
pub fn parse_task_file(text: &str) -> Vec<TodoItem> {
    let lines: Vec<(usize, &str, &str)> = text.lines().take(4096).filter_map(checkbox).collect();
    let base = lines.iter().map(|(indent, _, _)| *indent).min().unwrap_or(0);
    let mut top: Option<String> = None;
    let mut items = Vec::new();
    for (indent, status, content) in lines {
        let content: String = content.replace("**", "").replace('`', "").trim().to_string();
        if content.is_empty() {
            continue;
        }
        let shown = if indent <= base {
            top = Some(content.clone());
            content
        } else {
            match &top {
                Some(parent) => format!("{parent} › {content}"),
                None => content,
            }
        };
        items.push(todo_item(&shown, status, None, None));
        if items.len() >= MAXIMUM_TODO_ITEMS {
            break;
        }
    }
    items
}

/// One checklist line: its indent (a tab counts four), its status and its words.
fn checkbox(line: &str) -> Option<(usize, &'static str, &str)> {
    let rest = line.trim_start();
    let indent: usize = line[..line.len() - rest.len()].chars().map(|c| if c == '\t' { 4 } else { 1 }).sum();
    let rest = if let Some(after) = rest.strip_prefix(['-', '*', '+']) {
        after
    } else {
        let digits = rest.chars().take_while(char::is_ascii_digit).count();
        if digits == 0 {
            return None;
        }
        rest[digits..].strip_prefix(['.', ')'])?
    };
    let rest = rest.strip_prefix([' ', '\t'])?.trim_start();
    let mut chars = rest.chars();
    if chars.next() != Some('[') {
        return None;
    }
    let status = match chars.next()? {
        ' ' => "pending",
        '/' => "in_progress",
        'x' | 'X' => "completed",
        _ => return None,
    };
    let words = chars.as_str().strip_prefix(']')?;
    if !words.is_empty() && !words.starts_with([' ', '\t']) {
        return None;
    }
    Some((indent, status, words.trim()))
}
