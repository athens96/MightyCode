//! What a style's state sources read (C# `StyleStateEngine` in `StyleStateEngine.cs`): the newest current file of each file source,
//! the pane's own run events, the plan stage with its checklist and background work (§1.17), each as its widget; and which changes
//! can move a source (`Affects`, L721). Owner: W9-D.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use mighty_core::locale::Locale;
use mighty_core::model::{BackgroundWork, RunSession, TodoProgress};
use mighty_core::time::{DateTimeOffset, ticks_of};
use mighty_run::plan_mode::{outcome, todos, work as background};
use regex::Regex;

use crate::evaluator::{StyleEvaluator, StyleFileState, StyleStateReading, StyleStateWidget, StyleTaskItem};
use crate::files;
use crate::manifest::{StyleManifest, items, safe, text, trim};

/// `PlanStages`: the closed plan stages, in order.
pub const PLAN_STAGES: [&str; 3] = ["planning", "awaitingApproval", "executing"];

/// `MaximumTaskListItems`.
pub const MAXIMUM_TASK_LIST_ITEMS: usize = 8;

fn count(locale: &Locale, key: &str, n: usize) -> String {
    locale.get_with(key, &[("count", &n.to_string())])
}

/// `StyleStateEngine.Read(manifest, workspace, since, session)`: each file source's newest file changed since the style was chosen
/// (`since`; none read before it), and the pane's own events since then.
pub fn read(
    manifest: &StyleManifest,
    workspace: &Path,
    since: Option<DateTimeOffset>,
    session: Option<&RunSession>,
    locale: &Locale,
) -> StyleStateReading {
    let mut read_files: Vec<(StyleFileState, StyleStateWidget)> = Vec::new();
    let mut events: Vec<(String, String)> = Vec::new();
    let Some(sources) = manifest.root.get("stateSources") else { return reading(manifest, &read_files, &events, None, None, None, locale) };
    for source in items(sources, "files") {
        let kind = text(source, "widget").unwrap_or_default();
        let mut state = StyleFileState::default();
        let mut widget = StyleStateWidget::new(kind);
        if let Some(since) = since {
            let mut current: Option<(i64, String)> = None;
            for path in files::matches(workspace, text(source, "path").unwrap_or_default()) {
                let time = modified_ticks(&workspace.join(&path));
                if time <= since.utc_ticks() {
                    continue;
                }
                // The newest, then the last by path among equals.
                let newer = match &current {
                    None => true,
                    Some((t, p)) => time > *t || (time == *t && crate::manifest::ordinal(&path, p).is_gt()),
                };
                if newer {
                    current = Some((time, path));
                }
            }
            if let Some((_, path)) = current {
                state = StyleFileState { exists: true, all_checked: false };
                if let Some(bytes) = files::read(workspace, &path, crate::decoder::MAXIMUM_BYTES)
                    && let Ok(text) = String::from_utf8(bytes)
                {
                    (state, widget) = parse(&text, crate::manifest::text(source, "parser").unwrap_or_default(), kind, locale);
                }
            }
        }
        read_files.push((state, widget));
    }
    // A log entry is one activity updated in place: a sub-agent counts once as started and once more when it ended.
    if let (Some(since), Some(session)) = (since, session) {
        for log in session.logs.as_deref().unwrap_or(&[]) {
            let Some(a) = &log.activity else { continue };
            let Some(tool) = a.tool_name.as_deref() else { continue };
            let Some(at) = mighty_core::time::parse(log.timestamp.as_deref()) else { continue };
            if at.utc_ticks() < since.utc_ticks() {
                continue;
            }
            if a.kind == "agent" && matches!(tool.to_lowercase().as_str(), "task" | "agent") {
                events.push(("subagent.start".into(), safe(&a.summary, 400)));
                if matches!(a.state.as_str(), "completed" | "error" | "stopped") {
                    events.push(("subagent.finish".into(), safe(&a.summary, 400)));
                }
            } else if matches!(a.kind.as_str(), "tool" | "command" | "read" | "edit" | "search" | "web") {
                events.push(("tool.call".into(), safe(tool, 400)));
            }
        }
    }
    reading(manifest, &read_files, &events, None, None, None, locale)
}

fn modified_ticks(path: &Path) -> i64 {
    std::fs::metadata(path).and_then(|m| m.modified()).map(|t| ticks_of(chrono::DateTime::<chrono::Utc>::from(t))).unwrap_or(i64::MIN)
}

/// `StyleStateEngine.Reading`: a widget per source — files first, then run events, then run state, each in declaration order.
pub fn reading(
    manifest: &StyleManifest,
    read_files: &[(StyleFileState, StyleStateWidget)],
    run_events: &[(String, String)],
    plan_stage: Option<&str>,
    todos: Option<&TodoProgress>,
    work: Option<&BackgroundWork>,
    locale: &Locale,
) -> StyleStateReading {
    let mut result = StyleStateReading { plan_stage: plan_stage.map(str::to_string), ..StyleStateReading::default() };
    let Some(sources) = manifest.root.get("stateSources") else { return result };
    for (index, source) in items(sources, "files").iter().enumerate() {
        let (state, widget) = read_files
            .get(index)
            .cloned()
            .unwrap_or_else(|| (StyleFileState::default(), empty(text(source, "widget").unwrap_or_default())));
        result.files.insert(index, state);
        result.widgets.push(widget);
    }
    for source in items(sources, "runEvents") {
        let kind = text(source, "event").unwrap_or_default();
        let values: Vec<String> = run_events.iter().filter(|(e, _)| e == kind).map(|(_, v)| v.clone()).collect();
        result.widgets.push(aggregate(kind, text(source, "widget").unwrap_or_default(), text(source, "aggregate"), &values, locale));
    }
    if let Some(stage) = plan_stage {
        result.widgets.extend(run_state_widgets(manifest, stage, todos, work, locale));
    }
    result
}

/// `Aggregate`: `count` and `lastValue`, each drawn by the widget the source names; a bar from events has no total.
pub fn aggregate(kind_of_event: &str, kind: &str, aggregate: Option<&str>, values: &[String], locale: &Locale) -> StyleStateWidget {
    match kind {
        "progressBar" => StyleStateWidget::bar(kind, values.len() as i32, None),
        "list" => {
            let take = if aggregate == Some("count") { 8 } else { 1 };
            StyleStateWidget::list(kind, values[values.len().saturating_sub(take)..].to_vec())
        }
        _ => {
            let text = if aggregate == Some("lastValue") {
                values.last().cloned().unwrap_or_default()
            } else {
                let key = match kind_of_event {
                    "subagent.start" => "styles.state.subagentStartCount",
                    "subagent.finish" => "styles.state.subagentFinishCount",
                    _ => "styles.state.toolCallCount",
                };
                count(locale, key, values.len())
            };
            StyleStateWidget::text(kind, Some(text))
        }
    }
}

/// `Affects(manifest, workspace, changedPath)`: the change is under a source's fixed leading folder, or is one of its parents.
pub fn affects(manifest: &StyleManifest, workspace: &Path, changed: &Path) -> bool {
    let Some(sources) = manifest.root.get("stateSources") else { return false };
    items(sources, "files").iter().any(|source| {
        let path = text(source, "path").unwrap_or_default().replace('\\', "/");
        let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
        let mut leading: Vec<&str> = parts.iter().take_while(|p| !p.contains('*') && !p.contains('?')).copied().collect();
        // A literal path's last part is the file; its folder is what changes.
        if leading.len() == parts.len() && !leading.is_empty() {
            leading.pop();
        }
        let folder = leading.iter().fold(PathBuf::from(workspace), |at, part| at.join(part));
        crate::watcher::touches(changed, &folder)
    })
}

/// `PlanStage(session, pendingPlan)`: a waiting plan is awaitingApproval; otherwise the latest request's plan answers decide.
pub fn plan_stage(session: &RunSession, pending_plan: bool) -> &'static str {
    if pending_plan {
        return "awaitingApproval";
    }
    let (mut last_id, mut last_source, mut last_status): (Option<String>, Option<String>, Option<String>) = (None, None, None);
    let logs = session.logs.as_deref().unwrap_or(&[]);
    if let Some(runs) = &session.graph_runs {
        if let Some(last) = runs.last() {
            last_id = last.id.clone();
            last_source = last.source_run_id.clone();
            last_status = last.status.clone();
        }
    } else if !logs.is_empty() {
        last_id = Some(
            logs.iter()
                .rev()
                .find(|l| l.kind.as_deref() == Some("user"))
                .and_then(|l| l.id.clone())
                .unwrap_or_else(|| format!("history-{}", session.id.as_deref().unwrap_or(""))),
        );
        let status = session.status.as_deref().unwrap_or("");
        last_status = Some(if status == "idle" { "completed".to_string() } else { status.to_string() });
    }
    // A run started but its block is not there yet; a turn that is over and only waits on background work is still its own.
    if session.status.as_deref() == Some("running")
        && last_status.as_deref().is_some_and(mighty_core::graph::terminal)
        && session.background_work.as_ref().map(|w| w.turn_ended) != Some(true)
    {
        return "planning";
    }
    let history = session.plan_history.as_deref().unwrap_or(&[]);
    let latest = match &last_id {
        Some(id) => history.iter().rev().find(|r| {
            r.graph_run_id.as_ref() == Some(id)
                || last_source.as_ref().is_some_and(|s| r.graph_run_id.as_ref() == Some(s) || r.run_id.as_ref() == Some(s))
        }),
        None => history.last(),
    };
    match latest.and_then(|r| r.outcome.as_deref()) {
        Some(o) if o == outcome::APPROVED_AUTO || o == outcome::APPROVED_CONFIRM => "executing",
        _ => "planning",
    }
}

/// `Live(cached, manifest, session, pendingPlan)`: the cached reading with the live plan stage and run-state widgets added last.
pub fn live(
    cached: Option<&StyleStateReading>,
    manifest: &StyleManifest,
    session: &RunSession,
    pending_plan: bool,
    locale: &Locale,
) -> StyleStateReading {
    let mut reading = cached.cloned().unwrap_or_default();
    if !StyleEvaluator::new(manifest).reads_plan_state() {
        return reading;
    }
    let stage = plan_stage(session, pending_plan);
    reading.widgets.extend(run_state_widgets(manifest, stage, session.todo_progress.as_ref(), session.background_work.as_ref(), locale));
    reading.plan_stage = Some(stage.to_string());
    reading
}

/// `RunStateWidgets`: each declared run-state source as its widget.
pub fn run_state_widgets(
    manifest: &StyleManifest,
    stage: &str,
    todos: Option<&TodoProgress>,
    work: Option<&BackgroundWork>,
    locale: &Locale,
) -> Vec<StyleStateWidget> {
    let Some(sources) = manifest.root.get("stateSources") else { return Vec::new() };
    items(sources, "runState")
        .iter()
        .map(|s| run_state(text(s, "source").unwrap_or_default(), text(s, "widget").unwrap_or_default(), stage, todos, work, locale))
        .collect()
}

/// `RunState`: the checklist reads only while executing; background work as a task list (running first), a label or a bar.
pub fn run_state(
    source: &str,
    kind: &str,
    stage: &str,
    progress: Option<&TodoProgress>,
    work: Option<&BackgroundWork>,
    locale: &Locale,
) -> StyleStateWidget {
    if source == "todos" {
        let Some(progress) = progress.filter(|p| stage == "executing" && !todos::items(p).is_empty()) else { return empty(kind) };
        return match kind {
            "progressBar" => StyleStateWidget::bar(kind, todos::completed(progress) as i32, Some(todos::total(progress) as i32)),
            "list" => StyleStateWidget::list(
                kind,
                todos::items(progress)
                    .iter()
                    .filter(|i| i.status.as_deref() != Some("completed"))
                    .map(|i| safe(i.content.as_deref().unwrap_or(""), 400))
                    .take(8)
                    .collect(),
            ),
            _ => StyleStateWidget::text(
                kind,
                Some(match todos::current_text(progress) {
                    Some(current) => locale.get_with("styles.state.todoCurrent", &[("item", &safe(&current, 400))]),
                    None => String::new(),
                }),
            ),
        };
    }
    let tasks = work.map(background::tasks).unwrap_or(&[]);
    let running = tasks.iter().filter(|t| t.status.as_deref() == Some("running")).count();
    match kind {
        "taskList" => StyleStateWidget::tasks(
            kind,
            tasks
                .iter()
                .filter(|t| t.status.as_deref() == Some("running"))
                .chain(tasks.iter().filter(|t| t.status.as_deref() != Some("running")))
                .take(MAXIMUM_TASK_LIST_ITEMS)
                .map(|t| StyleTaskItem {
                    text: safe(t.description.as_deref().unwrap_or(""), 400),
                    kind: t.kind.clone().unwrap_or_default(),
                    status: t.status.clone().unwrap_or_default(),
                    started_at: t.started_at.clone().unwrap_or_default(),
                    ended_at: t.ended_at.clone(),
                })
                .collect(),
        ),
        "label" => StyleStateWidget::text(
            kind,
            Some(if work.is_some_and(background::waiting_on_background) {
                count(locale, "plan.background.status", running)
            } else if running > 0 {
                count(locale, "styles.state.backgroundRunning", running)
            } else {
                String::new()
            }),
        ),
        _ => StyleStateWidget::bar(kind, (tasks.len() - running) as i32, Some(tasks.len() as i32)),
    }
}

fn empty(kind: &str) -> StyleStateWidget {
    match kind {
        "progressBar" => StyleStateWidget::bar(kind, 0, Some(0)),
        "list" => StyleStateWidget::list(kind, Vec::new()),
        "taskList" => StyleStateWidget::tasks(kind, Vec::new()),
        _ => StyleStateWidget::text(kind, Some(String::new())),
    }
}

static CHECK_ITEM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[-*+] +\[([ xX])\](?: +(.*))?$").expect("checklist"));

/// `Parse(text, parser, kind)`: a markdown checklist (fenced blocks skipped) or a JSON file, as the source's widget.
pub fn parse(text_value: &str, parser: &str, kind: &str, locale: &Locale) -> (StyleFileState, StyleStateWidget) {
    let state = StyleFileState { exists: true, all_checked: false };
    if parser == "markdownChecklist" {
        let mut fence = false;
        let mut found: Vec<(String, bool)> = Vec::new();
        for raw in text_value.split('\n') {
            let line = trim(raw);
            if line.starts_with("```") || line.starts_with("~~~") {
                fence = !fence;
                continue;
            }
            if fence {
                continue;
            }
            if let Some(m) = CHECK_ITEM.captures(line) {
                let item = m.get(2).map(|g| trim(g.as_str())).unwrap_or("");
                found.push((safe(item, 400), &m[1] != " "));
            }
        }
        let checked = found.iter().filter(|(_, c)| *c).count();
        let state = StyleFileState { exists: true, all_checked: !found.is_empty() && checked == found.len() };
        let widget = match kind {
            "progressBar" => StyleStateWidget::bar(kind, checked as i32, Some(found.len() as i32)),
            "list" => StyleStateWidget::list(kind, found.iter().filter(|(_, c)| !c).take(8).map(|(t, _)| t.clone()).collect()),
            _ => StyleStateWidget::text(
                kind,
                Some(
                    locale
                        .get_with("styles.state.checklistLabel", &[("checked", &checked.to_string()), ("total", &found.len().to_string())]),
                ),
            ),
        };
        return (state, widget);
    }
    let Ok(root) = serde_json::from_str::<serde_json::Value>(text_value) else { return (state, StyleStateWidget::new(kind)) };
    let scalar = |e: &serde_json::Value| -> Option<String> {
        match e {
            serde_json::Value::String(s) => Some(safe(s, 400)),
            serde_json::Value::Number(n) => Some(safe(&n.to_string(), 400)),
            serde_json::Value::Bool(b) => Some(if *b { "True" } else { "False" }.to_string()),
            _ => None,
        }
    };
    match kind {
        "list" => {
            let list = if root.is_array() { Some(&root) } else { root.get("items").filter(|_| root.is_object()) };
            let values = list.and_then(|l| l.as_array()).map(|a| a.iter().filter_map(scalar).take(8).collect()).unwrap_or_default();
            (state, StyleStateWidget::list(kind, values))
        }
        "label" => {
            let value = root.get("text").filter(|_| root.is_object()).unwrap_or(&root);
            (state, StyleStateWidget::text(kind, scalar(value)))
        }
        _ => {
            let number = |key: &str| -> Option<i32> {
                root.get(key).and_then(|v| v.as_i64()).filter(|n| (0..=i32::MAX as i64).contains(n)).map(|n| n as i32)
            };
            let done = number("value");
            let total = number("total");
            let value = match (done, total) {
                (Some(d), Some(bound)) => d.min(bound),
                (Some(d), None) => d,
                _ => 0,
            };
            (state, StyleStateWidget::bar(kind, value, total))
        }
    }
}
