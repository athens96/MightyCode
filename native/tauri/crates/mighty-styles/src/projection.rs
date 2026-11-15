//! The one shape a style's panel and the phone payload are both drawn from (C# `StylePanel` and `StylePanelProjection` in
//! `StylePanelProjection.cs`; M/Styles/StylePanelProjection.swift, §5.6, §7.3), and its canonical bytes — byte-identical to the frozen
//! goldens `styles/golden/*.panel.json`: UTF-8, keys sorted case-blind then by code unit, two-space indent with `" : "`, an empty
//! array or object around one blank line, no `\/`, one trailing newline. Owner: W9-D.

use std::collections::BTreeMap;

use mighty_core::model::RunSession;
use serde_json::{Map, Value, json};

use crate::evaluator::{StyleStateReading, StyleStateWidget};
use crate::manifest::{ordinal, text};
use crate::prerequisites::StylePrerequisiteResult;
use crate::registry::RegisteredStyle;

/// `StylePanel.StyleInfo`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyleInfo {
    pub id: String,
    pub name: String,
    pub source: String,
    pub icon: Option<String>,
    pub tint: Option<String>,
}

/// `StylePanel.PhaseInfo`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhaseInfo {
    pub id: String,
    pub title: String,
    pub index: usize,
    pub count: usize,
}

/// `StylePanel.GroupInfo`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupInfo {
    pub id: String,
    pub title: String,
    pub axis: Option<String>,
    pub question: Option<String>,
    pub selected: bool,
    pub actions: Vec<String>,
}

/// `StylePanel.ActionInfo`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionInfo {
    pub id: String,
    pub title: String,
    pub icon: Option<String>,
    pub glyph: Option<String>,
    pub help: String,
    pub scope: Option<String>,
    pub takes_text: bool,
    pub requires_text: bool,
    pub flags: Vec<String>,
    pub prominent: bool,
}

/// `StylePanel.SetupInfo`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupInfo {
    pub ready: bool,
    pub missing: Vec<String>,
    pub hint: Option<String>,
    pub install_command: Option<String>,
}

/// `StylePanel.PresentationInfo`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresentationInfo {
    pub header_title: String,
    pub source: String,
    pub icon: Option<String>,
    pub tint: Option<String>,
}

/// C# `StylePanel`. `attachments` is always empty since v7 but still sent (a phone that predates v7 reads it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StylePanel {
    pub style: StyleInfo,
    pub phase: Option<PhaseInfo>,
    pub groups: Vec<GroupInfo>,
    pub actions: Vec<ActionInfo>,
    pub next: Vec<String>,
    pub recommended: Option<String>,
    pub setup: SetupInfo,
    pub guidance: Option<String>,
    pub presentation: PresentationInfo,
    pub widgets: Option<Vec<StyleStateWidget>>,
}

/// The flags an action may carry, in the order the panel lists them.
const FLAGS: [&str; 2] = ["userInvoked", "readOnly"];

fn put(o: &mut Map<String, Value>, key: &str, value: Option<&str>) {
    if let Some(v) = value {
        o.insert(key.into(), json!(v));
    }
}

impl StylePanel {
    /// The panel as `Wire.Json` writes it (camel-case, a part that is not there left out): the phone's `panel`.
    pub fn to_value(&self) -> Value {
        let mut style = Map::new();
        style.insert("id".into(), json!(self.style.id));
        style.insert("name".into(), json!(self.style.name));
        style.insert("source".into(), json!(self.style.source));
        put(&mut style, "icon", self.style.icon.as_deref());
        put(&mut style, "tint", self.style.tint.as_deref());
        let mut o = Map::new();
        o.insert("style".into(), Value::Object(style));
        if let Some(p) = &self.phase {
            o.insert("phase".into(), json!({ "id": p.id, "title": p.title, "index": p.index, "count": p.count }));
        }
        let groups: Vec<Value> = self
            .groups
            .iter()
            .map(|g| {
                let mut v = Map::new();
                v.insert("id".into(), json!(g.id));
                v.insert("title".into(), json!(g.title));
                put(&mut v, "axis", g.axis.as_deref());
                put(&mut v, "question", g.question.as_deref());
                v.insert("selected".into(), json!(g.selected));
                v.insert("actions".into(), json!(g.actions));
                Value::Object(v)
            })
            .collect();
        o.insert("groups".into(), Value::Array(groups));
        let actions: Vec<Value> = self
            .actions
            .iter()
            .map(|a| {
                let mut v = Map::new();
                v.insert("id".into(), json!(a.id));
                v.insert("title".into(), json!(a.title));
                put(&mut v, "icon", a.icon.as_deref());
                put(&mut v, "glyph", a.glyph.as_deref());
                v.insert("help".into(), json!(a.help));
                put(&mut v, "scope", a.scope.as_deref());
                v.insert("takesText".into(), json!(a.takes_text));
                v.insert("requiresText".into(), json!(a.requires_text));
                v.insert("flags".into(), json!(a.flags));
                v.insert("prominent".into(), json!(a.prominent));
                Value::Object(v)
            })
            .collect();
        o.insert("actions".into(), Value::Array(actions));
        o.insert("next".into(), json!(self.next));
        put(&mut o, "recommended", self.recommended.as_deref());
        o.insert("attachments".into(), json!([]));
        let mut setup = Map::new();
        setup.insert("ready".into(), json!(self.setup.ready));
        setup.insert("missing".into(), json!(self.setup.missing));
        put(&mut setup, "hint", self.setup.hint.as_deref());
        put(&mut setup, "installCommand", self.setup.install_command.as_deref());
        o.insert("setup".into(), Value::Object(setup));
        put(&mut o, "guidance", self.guidance.as_deref());
        let mut presentation = Map::new();
        presentation.insert("headerTitle".into(), json!(self.presentation.header_title));
        presentation.insert("source".into(), json!(self.presentation.source));
        put(&mut presentation, "icon", self.presentation.icon.as_deref());
        put(&mut presentation, "tint", self.presentation.tint.as_deref());
        o.insert("presentation".into(), Value::Object(presentation));
        if let Some(widgets) = &self.widgets {
            o.insert("widgets".into(), Value::Array(widgets.iter().map(crate::presentation::payload).collect()));
        }
        Value::Object(o)
    }
}

/// The prompts a pane's panel reads (C# `Make(style, session, …)`): its request blocks' inputs, or its user entries.
pub fn session_prompts(session: &RunSession) -> Vec<String> {
    match &session.graph_runs {
        Some(runs) if !runs.is_empty() => runs.iter().map(|r| r.input.clone().unwrap_or_default()).collect(),
        _ => session
            .logs
            .as_deref()
            .unwrap_or(&[])
            .iter()
            .filter(|l| l.kind.as_deref() == Some("user"))
            .map(|l| l.text.clone().unwrap_or_default())
            .collect(),
    }
}

/// C# `StylePanelProjection.Make(style, prompts, selectedGroupId, capabilityStates, prerequisites, running, session, state)`.
#[allow(clippy::too_many_arguments)]
pub fn make<S: AsRef<str>>(
    style: &RegisteredStyle,
    prompts: &[S],
    selected_group_id: Option<&str>,
    capability_states: &BTreeMap<String, String>,
    prerequisites: &StylePrerequisiteResult,
    running: bool,
    session: Option<&RunSession>,
    state: Option<&StyleStateReading>,
) -> StylePanel {
    let manifest = &style.manifest;
    let evaluator = style.evaluator();
    // §1.16: the same state the pane's phase bar reads.
    let phase = evaluator.current_phase(prompts, state.map(|s| &s.files), state.and_then(|s| s.plan_stage.as_deref()));
    let job_open = session.is_some_and(|s| evaluator.job_open(s));
    let mut ordered: Vec<_> = manifest.phases.iter().collect();
    ordered.sort_by_key(|p| p.order);
    let selected =
        manifest.groups.iter().find(|g| Some(g.id.as_str()) == selected_group_id).or_else(|| evaluator.initial_group(capability_states));
    let next: Vec<String> = evaluator.visible_actions(phase, selected, running, job_open, false).iter().map(|a| a.id.clone()).collect();
    let presentation = manifest.get("presentation");
    let icon = text(presentation, "icon").map(str::to_string);
    let tint = text(presentation, "tint").map(str::to_string);
    let index = phase.and_then(|p| ordered.iter().position(|o| o.id == p.id));
    StylePanel {
        style: StyleInfo {
            id: manifest.id().into(),
            name: manifest.name().into(),
            source: style.source.clone(),
            icon: icon.clone(),
            tint: tint.clone(),
        },
        phase: index.and_then(|i| phase.map(|p| PhaseInfo { id: p.id.clone(), title: p.title.clone(), index: i, count: ordered.len() })),
        groups: manifest
            .groups
            .iter()
            .map(|g| GroupInfo {
                id: g.id.clone(),
                title: g.title.clone(),
                axis: g.axis.clone(),
                question: g.question.clone(),
                selected: selected.is_some_and(|s| s.id == g.id),
                actions: g.actions.clone(),
            })
            .collect(),
        // The catalogue travels whole; what is drawn is decided by groups and next.
        actions: manifest
            .actions
            .iter()
            .map(|a| ActionInfo {
                id: a.id.clone(),
                title: a.title.clone(),
                icon: a.icon.clone(),
                glyph: a.glyph.clone(),
                help: a.help.clone(),
                scope: a.scope.clone(),
                takes_text: a.takes_text,
                requires_text: a.requires_text,
                flags: FLAGS.iter().filter(|f| a.flags.iter().any(|x| x == *f)).map(|f| f.to_string()).collect(),
                prominent: next.first() == Some(&a.id),
            })
            .collect(),
        recommended: evaluator.recommended_action(capability_states).map(str::to_string),
        setup: SetupInfo {
            ready: prerequisites.ready,
            missing: prerequisites.missing.clone(),
            hint: prerequisites.hint.clone(),
            install_command: manifest.root.get("install").and_then(|i| text(i, "command")).map(str::to_string),
        },
        guidance: evaluator.guidance(phase, running, job_open),
        presentation: PresentationInfo {
            header_title: match phase {
                None => manifest.name().to_string(),
                Some(p) => format!("{} · {}", manifest.name(), p.title),
            },
            source: style.source.clone(),
            icon,
            tint,
        },
        widgets: state.filter(|s| !s.widgets.is_empty()).map(|s| s.widgets.clone()),
        next,
    }
}

/// `Serialise(panel)`: the golden's bytes.
pub fn serialise(panel: &StylePanel) -> Vec<u8> {
    canonical(&panel.to_value())
}

/// `Canonical(node)`.
pub fn canonical(node: &Value) -> Vec<u8> {
    let mut text = String::new();
    write(&mut text, node, 0);
    text.push('\n');
    text.into_bytes()
}

/// .NET's `StringComparer.OrdinalIgnoreCase`: each code unit upper-cased (simple mapping), then compared.
fn ignore_case(a: &str, b: &str) -> std::cmp::Ordering {
    let upper = |s: &str| -> Vec<u16> {
        s.chars()
            .map(|c| {
                let mut u = c.to_uppercase();
                match (u.next(), u.next()) {
                    (Some(single), None) => single,
                    _ => c,
                }
            })
            .collect::<String>()
            .encode_utf16()
            .collect()
    };
    upper(a).cmp(&upper(b))
}

fn line(text: &mut String, level: usize) {
    text.push('\n');
    text.extend(std::iter::repeat_n(' ', level * 2));
}

fn write(text: &mut String, node: &Value, depth: usize) {
    match node {
        Value::Object(map) => {
            text.push('{');
            let mut pairs: Vec<(&String, &Value)> = map.iter().collect();
            // Case-blind first and by code unit between equals, as the Mac's sorted keys are.
            pairs.sort_by(|a, b| ignore_case(a.0, b.0).then_with(|| ordinal(a.0, b.0)));
            for (index, (key, value)) in pairs.iter().enumerate() {
                if index > 0 {
                    text.push(',');
                }
                line(text, depth + 1);
                quote(text, key);
                text.push_str(" : ");
                write(text, value, depth + 1);
            }
            if pairs.is_empty() {
                text.push('\n');
            }
            line(text, depth);
            text.push('}');
        }
        Value::Array(values) => {
            text.push('[');
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    text.push(',');
                }
                line(text, depth + 1);
                write(text, value, depth + 1);
            }
            if values.is_empty() {
                text.push('\n');
            }
            line(text, depth);
            text.push(']');
        }
        Value::String(s) => quote(text, s),
        Value::Null => text.push_str("null"),
        other => text.push_str(&other.to_string()),
    }
}

fn quote(text: &mut String, value: &str) {
    text.push('"');
    for c in value.chars() {
        match c {
            '"' => text.push_str("\\\""),
            '\\' => text.push_str("\\\\"),
            '\n' => text.push_str("\\n"),
            '\r' => text.push_str("\\r"),
            '\t' => text.push_str("\\t"),
            '\u{8}' => text.push_str("\\b"),
            '\u{c}' => text.push_str("\\f"),
            c if (c as u32) < 0x20 => text.push_str(&format!("\\u{:04x}", c as u32)),
            c => text.push(c),
        }
    }
    text.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_canonical_writer_writes_as_the_mac() {
        let value: Value = serde_json::from_str("{\"b\":[],\"a\":{},\"C\":\"x/\\\"y\\n\"}").expect("json");
        assert_eq!(
            String::from_utf8(canonical(&value)).expect("utf8"),
            "{\n  \"a\" : {\n\n  },\n  \"b\" : [\n\n  ],\n  \"C\" : \"x/\\\"y\\n\"\n}\n"
        );
    }
}
