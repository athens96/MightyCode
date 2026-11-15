//! The cross-references of a structurally sound manifest (C# `StyleManifestDecoder.Validate`): reserved ids and names, built-in
//! features in the Mac validator's order (unknown, undeclared, the map), references between actions, phases and groups, aliases,
//! recognition, the plan-state map, the enter rule and the auto-allowed tools. Owner: W9-D.

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use crate::decoder::{R, RESERVED_IDS, RESERVED_NAMES, fail, known};
use crate::evaluator::recognised_name;
use crate::manifest::{StyleManifest, Vocabulary, boolean, folded, items, ordinal, strings, text};
use crate::state::PLAN_STAGES;

static WIRE_PART: LazyLock<Regex> = LazyLock::new(|| Regex::new("^[A-Za-z0-9_-]{1,64}$").expect("wire part"));

fn sorted(map: &Value) -> Vec<(&String, &Value)> {
    let mut pairs: Vec<(&String, &Value)> = map.as_object().map(|o| o.iter().collect()).unwrap_or_default();
    pairs.sort_by(|a, b| ordinal(a.0, b.0));
    pairs
}

fn reference(s: Option<&str>, ids: &HashSet<String>, path: &str) -> R<()> {
    match s {
        Some(s) if !ids.contains(s) => fail("E_UNKNOWN_REFERENCE", &format!("{path}: {s}")),
        _ => Ok(()),
    }
}

/// C# `Validate`.
pub(crate) fn validate(m: &StyleManifest, source: &str, vocabulary: &Vocabulary) -> R<()> {
    if source != "bundled" {
        if RESERVED_IDS.contains(&m.id()) {
            return fail("E_RESERVED_ID", m.id());
        }
        if RESERVED_NAMES.iter().any(|n| folded(n) == folded(m.name())) {
            return fail("E_RESERVED_NAME", m.name());
        }
    }
    // A declared name the vocabulary lacks is refused first, so a rule reaches the map check only through a declared, known feature.
    let capabilities = strings(&m.root, "capabilities");
    for c in &capabilities {
        if !vocabulary.contains_key(c) {
            return fail("E_UNKNOWN_CAPABILITY", c);
        }
    }
    for key in ["recommend", "initialGroup"] {
        let rule = m.rule(key);
        if let Some(c) = text(rule, "capability") {
            if !capabilities.iter().any(|d| d == c) {
                return fail("E_CAPABILITY_UNDECLARED", c);
            }
            let states: &[String] = vocabulary.get(c).map(Vec::as_slice).unwrap_or(&[]);
            let map = rule.get("map").unwrap_or(&Value::Null);
            for state in states {
                if map.get(state).is_none() {
                    return fail("E_CAPABILITY_MAP", state);
                }
            }
            let names: Vec<&str> = states.iter().map(String::as_str).collect();
            for (name, _) in sorted(map) {
                known(name, &names, "E_UNKNOWN_REFERENCE", "")?;
            }
        }
    }
    let actions: HashSet<String> = m.actions.iter().map(|a| a.id.clone()).collect();
    let phases: HashSet<String> = m.phases.iter().map(|p| p.id.clone()).collect();
    let groups: HashSet<String> = m.groups.iter().map(|g| g.id.clone()).collect();
    if let Some(job) = m.root.get("job") {
        for a in strings(job, "whileOpen") {
            reference(Some(&a), &actions, "job.whileOpen")?;
        }
    }
    for g in &m.groups {
        for a in &g.actions {
            reference(Some(a), &actions, &format!("groups.{}", g.id))?;
        }
    }
    for a in &m.actions {
        reference(a.phase.as_deref(), &phases, &format!("actions.{}.phase", a.id))?;
    }
    for a in items(&m.root, "aliases") {
        reference(text(a, "phase"), &phases, "aliases.phase")?;
        let name = text(a, "name").unwrap_or_default();
        if actions.contains(name) || m.actions.iter().any(|x| x.r#match.as_deref() == Some(name)) {
            return fail("E_ALIAS_COLLISION", name);
        }
    }
    let mut matches = HashSet::new();
    for a in &m.actions {
        if let Some(found) = &a.r#match
            && ((actions.contains(found) && &a.id != found) || !matches.insert(found.clone()))
        {
            return fail("E_ALIAS_COLLISION", found);
        }
    }
    for a in &m.actions {
        let recognised = recognised_name(m, &a.prompt(""));
        match recognised {
            Some(r) if r == a.id || Some(&r) == a.r#match.as_ref() => {}
            _ => return fail("E_PROMPT_RECOGNITION", &a.id),
        }
    }
    if boolean(m.get("recognition"), "lowercase") {
        for a in items(&m.root, "aliases") {
            let name = text(a, "name").unwrap_or_default();
            if name != name.to_lowercase() {
                return fail("E_PROMPT_RECOGNITION", name);
            }
        }
    }
    let start = m.rule("start");
    if text(start, "kind") == Some("actions") {
        let phase = text(start, "phase").unwrap_or_default();
        if !phases.contains(phase) {
            return fail("E_START_PHASE", phase);
        }
        for a in strings(start, "actions") {
            reference(Some(&a), &actions, "rules.start.actions")?;
        }
    }
    let phase = m.rule("phase");
    match text(phase, "kind") {
        Some("none") => {
            if !phases.is_empty() {
                return fail("E_PHASE_RULE_NONE", "");
            }
        }
        Some("planState") => {
            // §1.17 (v6): every stage, nothing but the stages, each to a phase that exists.
            let map = phase.get("map").unwrap_or(&Value::Null);
            let pairs = sorted(map);
            for (name, _) in &pairs {
                if !PLAN_STAGES.contains(&name.as_str()) {
                    return fail("E_UNKNOWN_REFERENCE", &format!("rules.phase.map: {name}"));
                }
            }
            for stage in PLAN_STAGES {
                if map.get(stage).is_none() {
                    return fail("E_RULE_INCOMPLETE", stage);
                }
            }
            for (name, value) in &pairs {
                reference(value.as_str(), &phases, &format!("rules.phase.map.{name}"))?;
            }
        }
        _ => {
            reference(text(phase, "default"), &phases, "rules.phase.default")?;
            for o in items(phase, "stateOverrides") {
                let n = o.get("sourceIndex").and_then(Value::as_f64).unwrap_or(-1.0);
                let count = m.root.get("stateSources").map(|s| items(s, "files").len()).unwrap_or(0) as f64;
                if n < 0.0 || n >= count {
                    return fail("E_UNKNOWN_REFERENCE", "rules.phase.stateOverrides.sourceIndex");
                }
                reference(text(o, "phase"), &phases, "rules.phase.stateOverrides.phase")?;
            }
        }
    }
    for key in ["placeholders", "guidance"] {
        if let Some(own) = m.get(key).get("phases") {
            for (name, _) in sorted(own) {
                reference(Some(name), &phases, &format!("{key}.phases"))?;
            }
        }
    }
    let next = m.rule("next");
    if text(next, "kind") == Some("byPhase") {
        let map = next.get("map").unwrap_or(&Value::Null);
        for p in &m.phases {
            if map.get(&p.id).is_none() {
                return fail("E_RULE_INCOMPLETE", &p.id);
            }
        }
        if let Some(entries) = map.as_object() {
            for (name, value) in entries {
                reference(Some(name), &phases, "rules.next.map")?;
                for a in value.as_array().map(Vec::as_slice).unwrap_or(&[]) {
                    reference(a.as_str(), &actions, "rules.next.map")?;
                }
            }
        }
    }
    let enter = m.rule("enter");
    if text(enter, "kind") == Some("verbatim") {
        if m.get("placeholders").get("initial").is_some() {
            return fail("E_PLACEHOLDER_INITIAL", "");
        }
    } else {
        let action = text(enter, "action");
        reference(action, &actions, "rules.enter.action")?;
        if !m.action(action.unwrap_or_default()).is_some_and(|a| a.takes_text) {
            return fail("E_ENTER_ACTION_TEXT", action.unwrap_or_default());
        }
        reference(text(enter, "phase"), &phases, "rules.enter.phase")?;
    }
    for name in ["recommend", "initialGroup"] {
        let rule = m.rule(name);
        reference(text(rule, "group"), &groups, &format!("rules.{name}.group"))?;
        if let Some(entries) = rule.get("map").and_then(Value::as_object) {
            for (_, value) in entries {
                reference(value.as_str(), if name == "recommend" { &actions } else { &groups }, &format!("rules.{name}.map"))?;
            }
        }
    }
    let plugins: Vec<String> = items(m.get("prerequisites"), "probes")
        .iter()
        .filter(|p| text(p, "kind") == Some("plugin"))
        .filter_map(|p| text(p, "prefix"))
        .filter_map(|p| p.strip_suffix('@'))
        .map(str::to_string)
        .collect();
    let mut wires = HashSet::new();
    for e in items(&m.root, "autoAllow") {
        let tool = text(e, "tool").unwrap_or_default();
        let server = text(e, "server");
        if tool == "AskUserQuestion" {
            return fail("E_AUTOALLOW_QUESTION", "");
        }
        if !WIRE_PART.is_match(tool) || tool.contains("__") || tool.starts_with('_') {
            return fail("E_AUTOALLOW_SHAPE", tool);
        }
        match server {
            None => {
                if tool != "ToolSearch" {
                    return fail("E_AUTOALLOW_SERVER", tool);
                }
                if source != "bundled" {
                    return fail("E_AUTOALLOW_TOOLSEARCH_BUNDLED", "");
                }
            }
            Some(server) => {
                if !WIRE_PART.is_match(server) || server.contains("__") || server.ends_with('_') {
                    return fail("E_AUTOALLOW_SHAPE", server);
                }
                if !plugins.iter().any(|p| server.starts_with(&format!("plugin_{p}_"))) {
                    return fail("E_AUTOALLOW_FOREIGN_SERVER", server);
                }
            }
        }
        let wire = match server {
            None => tool.to_string(),
            Some(server) => format!("mcp__{server}__{tool}"),
        };
        if !wires.insert(wire.clone()) {
            return fail("E_AUTOALLOW_DUPLICATE", &wire);
        }
    }
    Ok(())
}
