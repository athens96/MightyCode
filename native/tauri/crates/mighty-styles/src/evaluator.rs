//! What a style shows and does from the pane's history (C# `StyleEvaluator.cs`): recognition, the current phase, the visible actions,
//! the Enter rewrite, placeholders, guidance, auto-allowed tools and an open job; and the reading records the state engine fills.
//! Owner: W9-D.

use std::collections::BTreeMap;

use mighty_core::model::RunSession;
use serde_json::Value;

use crate::manifest::{StyleAction, StyleGroup, StyleManifest, StylePhase, boolean, items, strings, text, trim};

/// C# `StyleEvaluator.RecognisedName(m, prompt)`: the word after one of the recognition prefixes, lower-cased when the style says so.
pub fn recognised_name(m: &StyleManifest, prompt: &str) -> Option<String> {
    let value = trim(prompt);
    let rule = m.get("recognition");
    let prefixes = strings(rule, "prefixes");
    let prefix = prefixes.iter().find(|p| value.starts_with(p.as_str()))?;
    let name: String = value[prefix.len()..].chars().take_while(|c| !c.is_whitespace()).collect();
    if name.is_empty() {
        None
    } else if boolean(rule, "lowercase") {
        Some(name.to_lowercase())
    } else {
        Some(name)
    }
}

/// C# `StyleEvaluator(manifest)`.
#[derive(Debug, Clone, Copy)]
pub struct StyleEvaluator<'a> {
    pub manifest: &'a StyleManifest,
}

impl<'a> StyleEvaluator<'a> {
    pub fn new(manifest: &'a StyleManifest) -> Self {
        StyleEvaluator { manifest }
    }

    fn rule(&self, name: &str) -> &'a Value {
        self.manifest.rule(name)
    }

    pub fn recognised_action(&self, prompt: &str) -> Option<&'a StyleAction> {
        let n = recognised_name(self.manifest, prompt);
        let n = n.as_deref();
        self.manifest
            .actions
            .iter()
            .find(|a| Some(a.id.as_str()) == n)
            .or_else(|| self.manifest.actions.iter().find(|a| a.r#match.is_some() && a.r#match.as_deref() == n))
    }

    /// §1.17 (v6): the style reads the pane's plan stage — for its phase rule or a run-state source.
    pub fn reads_plan_state(&self) -> bool {
        text(self.rule("phase"), "kind") == Some("planState")
            || self.manifest.root.get("stateSources").is_some_and(|s| !items(s, "runState").is_empty())
    }

    /// §1.17 (v6): `launch.permissionMode`.
    pub fn launch_permission_mode(&self) -> Option<&'a str> {
        self.manifest.root.get("launch").and_then(|l| text(l, "permissionMode"))
    }

    /// §1.17 (v6): the style draws a background task list of its own.
    pub fn draws_tasks(&self) -> bool {
        self.manifest.root.get("stateSources").is_some_and(|s| {
            items(s, "runState").iter().any(|e| text(e, "source") == Some("background") && text(e, "widget") == Some("taskList"))
        })
    }

    /// `CurrentPhase(prompts, states, planStage)`.
    pub fn current_phase<S: AsRef<str>>(
        &self,
        prompts: &[S],
        states: Option<&BTreeMap<usize, StyleFileState>>,
        plan_stage: Option<&str>,
    ) -> Option<&'a StylePhase> {
        let rule = self.rule("phase");
        let kind = text(rule, "kind");
        if kind == Some("none") {
            return None;
        }
        // §1.17: the stage, not the request history, moves a plan-state phase; nothing read is planning.
        if kind == Some("planState") {
            let id = rule.get("map").and_then(|m| text(m, plan_stage.unwrap_or("planning")));
            return self.manifest.phases.iter().find(|p| Some(p.id.as_str()) == id);
        }
        let mut id = text(rule, "default").map(str::to_string);
        for prompt in prompts.iter().rev() {
            let prompt = prompt.as_ref();
            let action = self.recognised_action(prompt);
            let mut phase = action.and_then(|a| a.phase.clone());
            if action.is_none() {
                let n = recognised_name(self.manifest, prompt);
                phase = items(&self.manifest.root, "aliases")
                    .iter()
                    .find(|a| text(a, "name").map(str::to_string) == n)
                    .and_then(|a| text(a, "phase"))
                    .map(str::to_string);
            }
            if phase.is_some() {
                id = phase;
                break;
            }
        }
        let mut current = self.manifest.phases.iter().find(|p| Some(&p.id) == id.as_ref());
        for o in items(rule, "stateOverrides") {
            let Some(candidate) = self.manifest.phases.iter().find(|p| Some(p.id.as_str()) == text(o, "phase")) else { continue };
            let index = o.get("sourceIndex").and_then(Value::as_f64).unwrap_or(-1.0);
            if index < 0.0 {
                continue;
            }
            if let Some(state) = states.and_then(|s| s.get(&(index as usize)))
                && candidate.order > current.map_or(-1, |c| c.order)
                && (if text(o, "condition") == Some("fileExists") { state.exists } else { state.all_checked })
            {
                current = Some(candidate);
            }
        }
        current
    }

    /// `InitialGroup(capabilities)`.
    pub fn initial_group(&self, capabilities: &BTreeMap<String, String>) -> Option<&'a StyleGroup> {
        let rule = self.rule("initialGroup");
        let mut id = text(rule, "group").map(str::to_string);
        if id.is_none()
            && let Some(state) = text(rule, "capability").and_then(|c| capabilities.get(c))
        {
            id = rule.get("map").and_then(|m| text(m, state)).map(str::to_string);
        }
        self.manifest.groups.iter().find(|g| Some(&g.id) == id.as_ref()).or_else(|| self.manifest.groups.first())
    }

    pub fn draws_phase_progress(&self) -> bool {
        text(self.rule("next"), "kind") == Some("byPhase")
    }

    pub fn draws_group_map(&self) -> bool {
        !self.draws_phase_progress() && self.manifest.groups.len() >= 2 && self.manifest.groups.iter().any(|g| g.axis.is_some())
    }

    pub fn reset_title(&self) -> Option<&'a str> {
        text(self.rule("start"), "resetTitle")
    }

    pub fn recommend_group_id(&self) -> Option<&'a str> {
        text(self.rule("recommend"), "group")
    }

    /// `RecommendedAction(capabilities)`.
    pub fn recommended_action(&self, capabilities: &BTreeMap<String, String>) -> Option<&'a str> {
        let rule = self.rule("recommend");
        if text(rule, "kind") != Some("capability") {
            return None;
        }
        let state = capabilities.get(text(rule, "capability")?)?;
        rule.get("map").and_then(|m| text(m, state))
    }

    /// `EffectivePhase(phase, startingNew)`: a reset goes back to the start rule's phase.
    pub fn effective_phase(&self, phase: Option<&'a StylePhase>, starting_new: bool) -> Option<&'a StylePhase> {
        let start = self.rule("start");
        if starting_new && text(start, "kind") == Some("actions") {
            self.manifest.phases.iter().find(|p| Some(p.id.as_str()) == text(start, "phase"))
        } else {
            phase
        }
    }

    pub fn at_start(&self, phase: Option<&StylePhase>) -> bool {
        let start = self.rule("start");
        text(start, "kind") == Some("actions") && phase.map(|p| p.id.as_str()) == text(start, "phase")
    }

    /// `VisibleActions(phase, group, running, jobOpen, startingNew)`.
    pub fn visible_actions(
        &self,
        phase: Option<&StylePhase>,
        group: Option<&StyleGroup>,
        running: bool,
        job_open: bool,
        starting_new: bool,
    ) -> Vec<&'a StyleAction> {
        let start = self.rule("start");
        let next = self.rule("next");
        let ids: Vec<String> = if job_open {
            self.manifest.root.get("job").map(|j| strings(j, "whileOpen")).unwrap_or_default()
        } else if running && text(next, "kind") == Some("byPhase") {
            Vec::new()
        } else if text(start, "kind") == Some("actions") && (phase.map(|p| p.id.as_str()) == text(start, "phase") || starting_new) {
            strings(start, "actions")
        } else if text(next, "kind") == Some("byGroup") {
            group.map(|g| g.actions.clone()).unwrap_or_default()
        } else {
            match phase {
                None => Vec::new(),
                Some(p) => next.get("map").map(|m| strings(m, &p.id)).unwrap_or_default(),
            }
        };
        ids.iter().filter_map(|id| self.manifest.action(id)).collect()
    }

    /// `RewriteAction(draft, phase, attachments, running, hasRequests, startingNew)`: the action a bare first draft is sent as.
    pub fn rewrite_action(
        &self,
        draft: &str,
        phase: Option<&StylePhase>,
        attachments: bool,
        running: bool,
        has_requests: bool,
        starting_new: bool,
    ) -> Option<&'a str> {
        let rule = self.rule("enter");
        (text(rule, "kind") == Some("rewriteBareDraftTo")
            && !attachments
            && !running
            && (!has_requests || starting_new)
            && !draft.trim_start_matches(char::is_whitespace).starts_with('/')
            && recognised_name(self.manifest, draft).is_none()
            && phase.map(|p| p.id.as_str()) == text(rule, "phase"))
        .then(|| text(rule, "action"))
        .flatten()
    }

    /// `Placeholder(phase, running, answering, jobOpen)`.
    pub fn placeholder(&self, phase: Option<&StylePhase>, running: bool, answering: bool, job_open: bool) -> String {
        let p = self.manifest.get("placeholders");
        // §1.17 (v6): a phase's own line for this state wins, after a question and while no job is open.
        if !answering
            && !job_open
            && let Some(phase) = phase
            && let Some(own) = own_line(p, &phase.id, running)
        {
            return own.to_string();
        }
        let key = if answering {
            "answering"
        } else if running || job_open {
            "running"
        } else if text(self.rule("enter"), "phase") == phase.map(|p| p.id.as_str()) && p.get("initial").is_some() {
            "initial"
        } else {
            "idle"
        };
        text(p, key).unwrap_or_default().to_string()
    }

    /// `Guidance(phase, running, jobOpen)`.
    pub fn guidance(&self, phase: Option<&StylePhase>, running: bool, job_open: bool) -> Option<String> {
        let guidance = self.manifest.get("guidance");
        let text_line = if job_open {
            self.manifest.root.get("job").and_then(|j| text(j, "guidance"))
        } else if let Some(own) = phase.and_then(|p| own_line(guidance, &p.id, running)) {
            Some(own)
        } else {
            // The start line belongs to a start rule's own phase.
            text(
                guidance,
                if running {
                    "running"
                } else if self.at_start(phase) {
                    "start"
                } else {
                    "next"
                },
            )
        };
        let line = text_line.filter(|t| !t.is_empty())?;
        Some(match phase {
            Some(p) => line.replace("{phase}", &p.title),
            None => line.replace("{phase} ", "").replace("{phase}", ""),
        })
    }

    /// `AutoAllowed(toolName)`: an exact listed tool, never a question.
    pub fn auto_allowed(&self, tool_name: &str) -> bool {
        tool_name.split("__").last() != Some("AskUserQuestion") && self.manifest.auto_allow().iter().any(|w| w == tool_name)
    }

    /// `JobOpen(session)`: the last opening tool call came after the last closing one.
    pub fn job_open(&self, session: &RunSession) -> bool {
        let Some(job) = self.manifest.root.get("job") else { return false };
        let (mut last_open, mut last_close) = (-1i64, -1i64);
        for (index, log) in session.logs.as_deref().unwrap_or(&[]).iter().enumerate() {
            let Some(a) = &log.activity else { continue };
            let Some(name) = a.tool_name.as_deref() else { continue };
            if a.kind == "turn" {
                continue;
            }
            let output = a.output.as_deref().unwrap_or("");
            let tool = name.split("__").last().unwrap_or(name);
            let matches = |m: &Value| {
                text(m, "tool") == Some(tool)
                    && text(m, "contains").is_none_or(|yes| output.contains(yes))
                    && text(m, "notContains").is_none_or(|no| !output.contains(no))
            };
            if items(job, "open").iter().any(matches) {
                last_open = index as i64;
            }
            if items(job, "close").iter().any(matches) {
                last_close = index as i64;
            }
        }
        last_open >= 0 && last_open > last_close
    }
}

fn own_line<'a>(owner: &'a Value, phase: &str, running: bool) -> Option<&'a str> {
    owner.get("phases").and_then(|p| p.get(phase)).and_then(|lines| text(lines, if running { "running" } else { "idle" }))
}

/// C# `StyleFileState(Exists, AllChecked)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StyleFileState {
    pub exists: bool,
    pub all_checked: bool,
}

/// §1.17 (v6): one background task as a `taskList` widget carries it (C# `StyleTaskItem`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyleTaskItem {
    pub text: String,
    pub kind: String,
    pub status: String,
    pub started_at: String,
    pub ended_at: Option<String>,
}

/// C# `StyleStateWidget(Kind, Text, Value, Total, Items, Tasks)`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StyleStateWidget {
    pub kind: String,
    pub text: Option<String>,
    pub value: i32,
    pub total: Option<i32>,
    pub items: Option<Vec<String>>,
    pub tasks: Option<Vec<StyleTaskItem>>,
}

impl StyleStateWidget {
    pub fn new(kind: &str) -> Self {
        StyleStateWidget { kind: kind.to_string(), ..StyleStateWidget::default() }
    }
    pub fn text(kind: &str, text: Option<String>) -> Self {
        StyleStateWidget { text, ..StyleStateWidget::new(kind) }
    }
    pub fn bar(kind: &str, value: i32, total: Option<i32>) -> Self {
        StyleStateWidget { value, total, ..StyleStateWidget::new(kind) }
    }
    pub fn list(kind: &str, items: Vec<String>) -> Self {
        StyleStateWidget { items: Some(items), ..StyleStateWidget::new(kind) }
    }
    pub fn tasks(kind: &str, tasks: Vec<StyleTaskItem>) -> Self {
        StyleStateWidget { tasks: Some(tasks), ..StyleStateWidget::new(kind) }
    }
}

/// C# `StyleStateReading(Files, Widgets, PlanStage)`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StyleStateReading {
    pub files: BTreeMap<usize, StyleFileState>,
    pub widgets: Vec<StyleStateWidget>,
    pub plan_stage: Option<String>,
}
