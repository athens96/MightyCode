//! How a style reads in words (C# `StylePresentation.cs`): its source and state, a request's prefix, an action's help, the chip
//! glyphs, progress, the phone's widget payload, background task rows, elapsed time, inline text, the approval card's sections and the
//! two-press approval. Owner: W9-D.

use std::fmt::Write as _;

use mighty_core::locale::Locale;
use mighty_core::time::DateTimeOffset;
use serde_json::{Map, Value, json};

use crate::evaluator::StyleStateWidget;
use crate::manifest::{StyleAction, StylePhase, banned, items, ordinal, text};
use crate::registry::RegisteredStyle;
use crate::state::{MAXIMUM_TASK_LIST_ITEMS, PLAN_STAGES};

/// C# `StyleApprovalSection(Id, Title, Lines, Foldable, Monospaced)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyleApprovalSection {
    pub id: String,
    pub title: String,
    pub lines: Vec<String>,
    pub foldable: bool,
    pub monospaced: bool,
}

/// C# `StyleRequestPresentation(Prefix, Icon, Tint)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyleRequestPresentation {
    pub prefix: Option<String>,
    pub icon: Option<String>,
    pub tint: String,
}

/// `Source(source)`.
pub fn source(locale: &Locale, source: &str) -> String {
    locale.get(match source {
        "bundled" => "styles.source.bundled",
        "user" => "styles.source.user",
        _ => "styles.source.workspace",
    })
}

/// `Name(style)`: the name, and its source unless bundled.
pub fn name(locale: &Locale, style: &RegisteredStyle) -> String {
    if style.source == "bundled" {
        style.manifest.name().to_string()
    } else {
        format!("{} · {}", style.manifest.name(), source(locale, &style.source))
    }
}

/// `State(approval)`.
pub fn state(locale: &Locale, approval: &str) -> String {
    locale.get(match approval {
        "preApproved" => "styles.state.preApproved",
        "approved" => "styles.state.approved",
        "revoked" => "styles.state.revoked",
        _ => "styles.state.pending",
    })
}

/// `Header(style, phase, mightyLabel)`.
pub fn header(locale: &Locale, style: Option<&RegisteredStyle>, phase: Option<&StylePhase>, mighty_label: &str) -> String {
    match style {
        None => mighty_label.to_string(),
        Some(style) => match phase {
            None => format!("{mighty_label} · {}", name(locale, style)),
            Some(p) => format!("{mighty_label} · {} · {}", name(locale, style), p.title),
        },
    }
}

/// `Request(styles, prompt)`: the first runnable style (bundled, user, workspace; then by id) that recognises the prompt names it.
pub fn request(styles: &[RegisteredStyle], prompt: &str) -> StyleRequestPresentation {
    let mut ordered: Vec<&RegisteredStyle> = styles.iter().filter(|s| s.runnable()).collect();
    let rank = |s: &str| match s {
        "bundled" => 0,
        "user" => 1,
        _ => 2,
    };
    ordered.sort_by(|a, b| rank(&a.source).cmp(&rank(&b.source)).then_with(|| ordinal(a.id(), b.id())));
    for style in ordered {
        let evaluator = style.evaluator();
        let action = evaluator.recognised_action(prompt);
        let n = crate::evaluator::recognised_name(&style.manifest, prompt);
        let alias = items(&style.manifest.root, "aliases").iter().find(|a| text(a, "name").map(str::to_string) == n);
        let phase_id = action.and_then(|a| a.phase.clone()).or_else(|| alias.and_then(|a| text(a, "phase")).map(str::to_string));
        let phase = style.manifest.phases.iter().find(|p| Some(&p.id) == phase_id.as_ref());
        if action.is_none() && alias.is_none() {
            continue;
        }
        let prefix = match action {
            None => phase.map(|p| p.title.clone()),
            Some(a) => a.request_title.clone().or_else(|| match &a.glyph {
                Some(glyph) => Some(format!("{glyph} {}", a.title)),
                None => Some(phase.map(|p| p.title.clone()).unwrap_or_else(|| a.title.clone())),
            }),
        };
        let presentation = style.manifest.get("presentation");
        return StyleRequestPresentation {
            prefix,
            icon: action.and_then(|a| a.icon.clone()).or_else(|| text(presentation, "icon").map(str::to_string)),
            tint: action
                .and_then(|a| a.tint.clone())
                .or_else(|| text(presentation, "tint").map(str::to_string))
                .unwrap_or_else(|| "accent".into()),
        };
    }
    StyleRequestPresentation { prefix: None, icon: None, tint: "accent".into() }
}

/// `RequestPrefix(styles, prompt)`.
pub fn request_prefix(styles: &[RegisteredStyle], prompt: &str) -> Option<String> {
    request(styles, prompt).prefix
}

/// `ActionHelp(action)`: help, scope and flags in words, ` · ` between.
pub fn action_help(locale: &Locale, action: &StyleAction) -> String {
    let mut values = Vec::new();
    if !action.help.is_empty() {
        values.push(action.help.clone());
    }
    if let Some(scope) = action.scope.as_deref().filter(|s| !s.is_empty()) {
        values.push(locale.get_with("styles.action.scope", &[("scope", scope)]));
    }
    for flag in &action.flags {
        values.push(locale.get(if flag == "readOnly" { "styles.action.readOnly" } else { "styles.action.userInvoked" }));
    }
    values.join(" · ")
}

/// `Icon(icon)`: the glyph a manifest's icon name is drawn with.
pub fn icon(icon: Option<&str>) -> &'static str {
    match icon.unwrap_or("") {
        "play.fill" => "▷",
        "checkmark.seal" => "✓",
        "arrow.clockwise" | "arrow.triangle.2.circlepath" => "↻",
        "infinity" => "∞",
        "questionmark.bubble" | "questionmark.square.dashed" => "?",
        "arrow.up.message" => "↑",
        "arrow.right" => "→",
        "arrow.triangle.branch" => "⑂",
        "bolt" => "ϟ",
        "book" | "doc.text" => "▤",
        "bookmark" | "flag" => "⚑",
        "calendar" => "▦",
        "chart.bar" | "gauge.with.dots.needle.33percent" => "▥",
        "folder" | "tray" => "▱",
        "list.bullet" => "☷",
        "magnifyingglass" => "⌕",
        "leaf" => "♧",
        "wand.and.stars" | "sparkles" => "✦",
        "lightbulb" => "☀",
        "square.grid.2x2" => "▦",
        "cube" | "puzzlepiece" => "◈",
        "map" => "⌘",
        "paintbrush" | "hammer" => "⚒",
        "point.3.connected.trianglepath.dotted" => "△",
        _ => "",
    }
}

/// `Progress(widget)`: a bare counter shows its count; a total clamps.
pub fn progress(widget: &StyleStateWidget) -> Option<(f64, String)> {
    if widget.value < 0 {
        return None;
    }
    match widget.total {
        Some(total) if total >= 0 => {
            let done = widget.value.min(total);
            Some((if total == 0 { 0.0 } else { done as f64 / total as f64 }, format!("{done}/{total}")))
        }
        _ => Some((0.0, widget.value.to_string())),
    }
}

/// `Payload(widget)`: one widget as the phone payload carries it — only the fields its kind has.
pub fn payload(widget: &StyleStateWidget) -> Value {
    let mut value = Map::new();
    value.insert("kind".into(), json!(widget.kind));
    match widget.kind.as_str() {
        "progressBar" => {
            value.insert("value".into(), json!(widget.value));
            if let Some(total) = widget.total {
                value.insert("total".into(), json!(total));
            }
        }
        "list" => {
            value.insert("items".into(), json!(widget.items.clone().unwrap_or_default()));
        }
        "taskList" => {
            let tasks: Vec<Value> = widget
                .tasks
                .as_deref()
                .unwrap_or(&[])
                .iter()
                .map(|t| {
                    let mut item = Map::new();
                    item.insert("text".into(), json!(t.text));
                    item.insert("kind".into(), json!(t.kind));
                    item.insert("status".into(), json!(t.status));
                    item.insert("startedAt".into(), json!(t.started_at));
                    if let Some(ended) = &t.ended_at {
                        item.insert("endedAt".into(), json!(ended));
                    }
                    Value::Object(item)
                })
                .collect();
            value.insert("items".into(), Value::Array(tasks));
        }
        _ => {
            value.insert("text".into(), json!(widget.text.clone().unwrap_or_default()));
        }
    }
    Value::Object(value)
}

/// One background task as both platforms draw it (C# `StylePresentation.TaskRow`).
#[derive(Debug, Clone, PartialEq)]
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
    /// To now while it runs, to its end once it ended.
    pub fn elapsed(&self, locale: &Locale, now: DateTimeOffset) -> String {
        match self.started_at {
            Some(start) => elapsed(locale, start, if self.running { now } else { self.ended_at.unwrap_or(now) }),
            None => String::new(),
        }
    }
}

/// `Tasks(widget)`.
pub fn tasks(locale: &Locale, widget: &StyleStateWidget) -> Vec<TaskRow> {
    widget
        .tasks
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .take(MAXIMUM_TASK_LIST_ITEMS)
        .map(|t| {
            let kind = if matches!(t.kind.as_str(), "agent" | "shell") { t.kind.as_str() } else { "other" };
            let status =
                if matches!(t.status.as_str(), "running" | "completed" | "failed" | "stopped") { t.status.as_str() } else { "unknown" };
            let title = task_kind(locale, kind);
            let text = inline(&t.text);
            TaskRow {
                text: if text.is_empty() { title.clone() } else { text },
                kind: kind.into(),
                kind_title: title,
                status: status.into(),
                status_title: task_status(locale, status),
                running: status == "running",
                started_at: mighty_core::time::parse(Some(&t.started_at)),
                ended_at: mighty_core::time::parse(t.ended_at.as_deref()),
            }
        })
        .collect()
}

pub fn task_kind(locale: &Locale, kind: &str) -> String {
    locale.get(match kind {
        "agent" => "styles.state.taskKind.agent",
        "shell" => "styles.state.taskKind.shell",
        _ => "styles.state.taskKind.other",
    })
}

pub fn task_status(locale: &Locale, status: &str) -> String {
    locale.get(match status {
        "running" => "styles.state.taskStatus.running",
        "completed" => "styles.state.taskStatus.completed",
        "failed" => "styles.state.taskStatus.failed",
        "stopped" => "styles.state.taskStatus.stopped",
        _ => "styles.state.taskStatus.unknown",
    })
}

/// `Elapsed(start, end)`: seconds, minutes and seconds, or hours and minutes; a clock that went back reads as 0.
pub fn elapsed(locale: &Locale, start: DateTimeOffset, end: DateTimeOffset) -> String {
    let seconds = ((end.utc_ticks() - start.utc_ticks()) / 10_000_000).max(0);
    if seconds < 60 {
        locale.get_with("styles.state.elapsedSeconds", &[("seconds", &seconds.to_string())])
    } else if seconds < 3600 {
        locale
            .get_with("styles.state.elapsedMinutes", &[("minutes", &(seconds / 60).to_string()), ("seconds", &(seconds % 60).to_string())])
    } else {
        locale.get_with(
            "styles.state.elapsedHours",
            &[("hours", &(seconds / 3600).to_string()), ("minutes", &(seconds % 3600 / 60).to_string())],
        )
    }
}

/// `Inline(text)`: banned characters out (a zero-width joiner kept), trimmed, at most 200 characters.
pub fn inline(value: &str) -> String {
    let kept: String = value.chars().filter(|c| *c as u32 == 0x200d || !banned(*c as u32)).collect();
    crate::manifest::trim(&kept).chars().take(200).collect()
}

/// `Approval(style)`: the approval card's sections, the risk-bearing ones first.
pub fn approval(locale: &Locale, style: &RegisteredStyle) -> Vec<StyleApprovalSection> {
    let m = &style.manifest;
    let section = |id: &str, title: String, lines: Vec<String>, foldable: bool, monospaced: bool| StyleApprovalSection {
        id: id.into(),
        title,
        lines,
        foldable,
        monospaced,
    };
    let mut result = Vec::new();
    let mut origin = vec![source(locale, &style.source), style.path.clone(), format!("SHA-256 {}", style.hash)];
    if let Some(workspace) = &style.workspace_path {
        origin.push(workspace.clone());
    }
    result.push(section("origin", locale.get("styles.approval.origin"), origin, false, false));
    let automatic = m.auto_allow();
    result.push(section(
        "autoAllow",
        locale.get("styles.approval.autoAllow"),
        if automatic.is_empty() { vec![locale.get("styles.approval.noAutoAllow")] } else { automatic },
        false,
        true,
    ));
    if let Some(install) = m.root.get("install") {
        result.push(section(
            "install",
            locale.get("styles.approval.install"),
            vec![text(install, "command").unwrap_or_default().to_string(), locale.get("styles.approval.installNotice")],
            false,
            true,
        ));
    }
    let enter = m.rule("enter");
    let enter_lines = if text(enter, "kind") == Some("verbatim") {
        vec![locale.get("styles.approval.verbatim")]
    } else {
        let phase = m.phase(text(enter, "phase").unwrap_or_default()).map(|p| p.title.clone()).unwrap_or_default();
        let action = m.action(text(enter, "action").unwrap_or_default()).map(|a| a.prompt_template.clone()).unwrap_or_default();
        vec![locale.get_with("styles.approval.rewrite", &[("phase", &phase)]), action]
    };
    result.push(section("enter", locale.get("styles.approval.enter"), enter_lines, false, true));
    let plan_state = text(m.rule("phase"), "kind") == Some("planState");
    let sources = m.root.get("stateSources");
    if sources.is_some() || plan_state {
        let none = Value::Null;
        let sources = sources.unwrap_or(&none);
        let files = items(sources, "files");
        let mut lines: Vec<String> = files
            .iter()
            .map(|f| {
                locale.get_with(
                    "styles.approval.stateFile",
                    &[
                        ("path", text(f, "path").unwrap_or_default()),
                        ("parser", text(f, "parser").unwrap_or_default()),
                        ("widget", text(f, "widget").unwrap_or_default()),
                    ],
                )
            })
            .collect();
        lines.extend(items(sources, "runEvents").iter().map(|f| {
            locale.get_with(
                "styles.approval.stateRunEvent",
                &[
                    ("event", text(f, "event").unwrap_or_default()),
                    ("aggregate", text(f, "aggregate").unwrap_or_default()),
                    ("widget", text(f, "widget").unwrap_or_default()),
                ],
            )
        }));
        lines.extend(items(m.rule("phase"), "stateOverrides").iter().map(|o| {
            let index = o.get("sourceIndex").and_then(Value::as_f64).unwrap_or(0.0) as usize;
            locale.get_with(
                "styles.approval.stateOverride",
                &[
                    ("phase", text(o, "phase").unwrap_or_default()),
                    ("path", files.get(index).and_then(|f| text(f, "path")).unwrap_or_default()),
                    ("condition", text(o, "condition").unwrap_or_default()),
                ],
            )
        }));
        // §1.17 (v6): the pane's plan-mode state, then the stages that move the phase.
        lines.extend(items(sources, "runState").iter().map(|r| {
            locale.get_with(
                "styles.approval.stateRunState",
                &[("source", text(r, "source").unwrap_or_default()), ("widget", text(r, "widget").unwrap_or_default())],
            )
        }));
        if plan_state {
            let map = m.rule("phase").get("map").unwrap_or(&none);
            lines.extend(PLAN_STAGES.iter().map(|stage| {
                locale.get_with("styles.approval.statePlanStage", &[("phase", text(map, stage).unwrap_or("")), ("stage", stage)])
            }));
        }
        result.push(section("state", locale.get("styles.approval.stateTitle"), lines, false, true));
    }
    if let Some(launch) = m.root.get("launch") {
        result.push(section(
            "launch",
            locale.get("styles.approval.launchTitle"),
            vec![locale.get_with("styles.approval.launchPlan", &[("mode", text(launch, "permissionMode").unwrap_or_default())])],
            false,
            false,
        ));
    }
    result.push(section(
        "identity",
        locale.get("styles.approval.identity"),
        vec![m.name().into(), m.id().into(), m.summary().into(), m.subtitle().into()],
        false,
        false,
    ));
    let mut phases: Vec<&StylePhase> = m.phases.iter().collect();
    phases.sort_by_key(|p| p.order);
    let mut structure: Vec<String> = phases.iter().map(|p| format!("{} · {}", p.id, p.title)).collect();
    structure.extend(m.groups.iter().map(|g| {
        let actions = g.actions.join(", ");
        [Some(g.id.as_str()), Some(g.title.as_str()), g.axis.as_deref(), g.question.as_deref(), Some(actions.as_str())]
            .into_iter()
            .flatten()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" · ")
    }));
    structure.extend(
        items(&m.root, "aliases")
            .iter()
            .map(|a| format!("{} → {}", text(a, "name").unwrap_or_default(), text(a, "phase").unwrap_or_default())),
    );
    structure.push(pretty(m.get("recognition")));
    result.push(section("structure", locale.get("styles.approval.structure"), structure, true, false));
    result.push(section("rules", locale.get("styles.approval.rules"), vec![pretty(m.rules())], true, true));
    let actions: Vec<String> = m
        .actions
        .iter()
        .flat_map(|a| {
            let mut head = format!("{} ({})", a.title, a.id);
            if a.takes_text {
                head.push_str(&format!(" · {}", locale.get("styles.action.takesText")));
            }
            if a.requires_text {
                head.push_str(&format!(" · {}", locale.get("styles.action.requiresText")));
            }
            [head, action_help(locale, a), a.prompt_template.clone()]
        })
        .collect();
    result.push(section(
        "actions",
        locale.get_with("styles.approval.actions", &[("count", &m.actions.len().to_string())]),
        actions,
        true,
        true,
    ));
    result.push(section(
        "presentation",
        locale.get("styles.approval.presentation"),
        vec![pretty(m.get("presentation")), pretty(m.get("placeholders")), pretty(m.get("guidance"))],
        true,
        false,
    ));
    result
}

/// `Pretty(value)`: `JsonSerializer.Serialize(value, WriteIndented)` — two-space indent, `"key": value`, the default encoder's escapes
/// (everything past ASCII and the HTML-sensitive characters as `\uXXXX`).
pub fn pretty(value: &Value) -> String {
    let mut out = String::new();
    pretty_into(&mut out, value, 0);
    out
}

fn pretty_into(out: &mut String, value: &Value, depth: usize) {
    let indent = |out: &mut String, level: usize| {
        out.push('\n');
        out.extend(std::iter::repeat_n(' ', level * 2));
    };
    match value {
        Value::Object(map) if map.is_empty() => out.push_str("{}"),
        Value::Array(values) if values.is_empty() => out.push_str("[]"),
        Value::Object(map) => {
            out.push('{');
            for (index, (key, v)) in map.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                indent(out, depth + 1);
                escaped(out, key);
                out.push_str(": ");
                pretty_into(out, v, depth + 1);
            }
            indent(out, depth);
            out.push('}');
        }
        Value::Array(values) => {
            out.push('[');
            for (index, v) in values.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                indent(out, depth + 1);
                pretty_into(out, v, depth + 1);
            }
            indent(out, depth);
            out.push(']');
        }
        Value::String(s) => escaped(out, s),
        other => out.push_str(&other.to_string()),
    }
}

fn escaped(out: &mut String, value: &str) {
    out.push('"');
    for unit in value.encode_utf16() {
        match unit {
            0x5c => out.push_str("\\\\"),
            0x0a => out.push_str("\\n"),
            0x0d => out.push_str("\\r"),
            0x09 => out.push_str("\\t"),
            0x08 => out.push_str("\\b"),
            0x0c => out.push_str("\\f"),
            0x20..=0x7e if !matches!(unit, 0x22 | 0x26 | 0x27 | 0x2b | 0x3c | 0x3e | 0x60) => out.push(unit as u8 as char),
            _ => {
                let _ = write!(out, "\\u{unit:04X}");
            }
        }
    }
    out.push('"');
}

/// C# `StyleApprovalDecision(automaticTools)`: both presses are scoped to one preview and a visible auto-allow block.
#[derive(Debug, Clone, Copy)]
pub struct StyleApprovalDecision {
    automatic_tools: usize,
    pub confirming: bool,
}

impl StyleApprovalDecision {
    pub fn new(automatic_tools: usize) -> Self {
        StyleApprovalDecision { automatic_tools, confirming: false }
    }
    /// `Press(autoAllowVisible)`: true when this press approves.
    pub fn press(&mut self, auto_allow_visible: bool) -> bool {
        if !auto_allow_visible {
            return false;
        }
        if self.automatic_tools > 0 && !self.confirming {
            self.confirming = true;
            return false;
        }
        true
    }
}
