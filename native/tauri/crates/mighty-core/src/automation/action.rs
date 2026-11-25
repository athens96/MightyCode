//! MC-99: what a `develop` and a `run` send, and what a `run`'s command and an `emit`'s data are once the event's values fill them.
//!
//! - `develop`: the trigger's request (its `{{…}}` filled, the event below it as for `ask`) and a block the agent follows: work in
//!   this worktree, run the project's tests, commit, and, with `pr` (the default), push and open a pull request whose link it prints;
//!   merge it once its checks pass only with `merge`. A bot's standing instructions go with it (its run is a fresh pane).
//! - `run`: exactly one command or skill with a short framing (run this once, nothing else; report the outcome), the request as
//!   context after it. The app never runs it itself: the agent does, under its own permissions.
//! - A `run`'s command and an `emit`'s data are filled strictly: a `{{name}}` the event has no value for refuses the fire (a release
//!   of an empty version never runs).

use std::collections::BTreeMap;

use serde_json::Value;

use super::spec::With;
use super::watch::{TriggerEvent, fill_fields, prompt_with_event};

/// The placeholders (`{{task.version}}`) `text` names, in order.
fn placeholders(text: &str) -> Vec<String> {
    let mut out = vec![];
    let mut rest = text;
    while let Some(open) = rest.find("{{") {
        let after = &rest[open + 2..];
        let Some(close) = after.find("}}") else { break };
        let name = after[..close].trim();
        if !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || matches!(c, '.' | '_' | '-')) {
            out.push(name.to_string());
        }
        rest = &after[close + 2..];
    }
    out
}

/// `text` with its `{{…}}` filled from `fields`; the name of the first one with no value (or an empty one) when any.
pub fn fill_strict(text: &str, fields: &BTreeMap<String, String>) -> Result<String, String> {
    if let Some(missing) = placeholders(text).into_iter().find(|name| fields.get(name).is_none_or(|v| v.trim().is_empty())) {
        return Err(missing);
    }
    Ok(fill_fields(text, fields))
}

/// A `run`'s command for `event` (none: the command as written, a 지금 실행 without an event; a placeholder then refuses it too).
pub fn run_command(with: &With, event: Option<&TriggerEvent>) -> Result<String, String> {
    let command = with.command.as_deref().unwrap_or("").trim();
    let empty = BTreeMap::new();
    fill_strict(command, event.map_or(&empty, |e| &e.fields)).map(|c| c.trim().to_string())
}

/// What a `run` sends: the command framed, the request (filled, with the event) as context when there is one.
pub fn run_prompt(command: &str, request: &str, event: Option<&TriggerEvent>) -> String {
    let mut out = format!(
        "[Mighty Code] Run exactly this, once, and nothing else:\n\n{command}\n\nA slash command or a skill name: run that command or skill. A shell line: run it in this workspace's folder. Do not change it, add steps or retry it in another form. Then report the outcome in a few lines: whether it succeeded, and what it printed or changed that matters."
    );
    let context = match event {
        Some(event) => prompt_with_event(request, event),
        None => request.trim().to_string(),
    };
    if !context.trim().is_empty() {
        out.push_str("\n\nContext:\n");
        out.push_str(context.trim());
    }
    out
}

/// Where a `develop` works: its branch and the branch it started from (None: a folder that is no worktree).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Place {
    pub branch: String,
    pub base: String,
}

/// What a `develop` sends: the request (filled, with the event) and the block of what to do in its worktree; a bot's handle and
/// standing instructions after it when it is a bot's.
pub fn develop_prompt(
    request: &str,
    with: &With,
    event: Option<&TriggerEvent>,
    place: Option<&Place>,
    bot: Option<(&str, &str)>,
) -> String {
    let mut out = match event {
        Some(event) => prompt_with_event(request, event),
        None => request.trim().to_string(),
    };
    let base = place.map_or("the base branch", |p| p.base.as_str());
    let mut steps = vec![
        match place {
            Some(place) => format!(
                "Work only in this pane's own worktree (branch {}, made from {}); never in the main checkout or another branch.",
                place.branch, place.base
            ),
            None => "Work in this folder.".to_string(),
        },
        "Make the change the request asks for, and nothing beyond it.".to_string(),
        "Run the project's tests (and its linters where it has them) and fix what fails until they pass.".to_string(),
        "Commit your work on this branch with a clear message.".to_string(),
    ];
    if with.pr() {
        steps.push(format!("Push the branch and open a pull request against {base} (`gh pr create`); print its URL on a line of its own."));
        steps.push(if with.merge() {
            "Once its checks pass, merge it (`gh pr merge`); if they fail, fix them first, and stop without merging if you cannot."
                .to_string()
        } else {
            "Do not merge it: a person reviews it.".to_string()
        });
    } else {
        steps.push("Do not push and do not open a pull request; leave the commits on this branch.".to_string());
    }
    steps.push("End with a short summary: what changed, the tests you ran and how they went, and the pull request's URL.".to_string());
    out.push_str("\n\n[Mighty Code] Develop this:\n");
    for (n, step) in steps.iter().enumerate() {
        out.push_str(&format!("{}. {step}\n", n + 1));
    }
    if let Some((handle, instructions)) = bot.filter(|(_, text)| !text.trim().is_empty()) {
        out.push_str(&format!("\nYou work for the bot @{handle}; its standing instructions (from the person who set it up):\n"));
        out.push_str(instructions.trim());
    }
    out.trim_end().to_string()
}

/// An `emit`'s data for `event`: each value filled strictly; the name of the first value with no field when any.
pub fn emit_data(with: &With, event: Option<&TriggerEvent>) -> Result<Value, String> {
    let empty = BTreeMap::new();
    let fields = event.map_or(&empty, |e| &e.fields);
    let mut data = serde_json::Map::new();
    for (key, value) in &with.data {
        data.insert(key.clone(), Value::String(fill_strict(value, fields)?));
    }
    Ok(Value::Object(data))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task_event() -> TriggerEvent {
        TriggerEvent {
            id: "e".into(),
            title: "MC-12: v0.6.4 릴리스 (stage: backlog → todo)".into(),
            fields: [("task.version", "v0.6.4"), ("task.key", "MC-12"), ("task.title", "v0.6.4 릴리스")]
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            ..TriggerEvent::default()
        }
        .with_envelope("mcp:mightywork", "mightywork.task.stage_changed", "MC-12", None)
    }

    #[test]
    fn a_run_s_command_is_filled_strictly_and_framed() {
        let with = With { command: Some(" /release {{task.version}} ".into()), ..With::default() };
        assert_eq!(run_command(&with, Some(&task_event())).as_deref(), Ok("/release v0.6.4"));
        let missing = With { command: Some("/release {{task.nope}}".into()), ..With::default() };
        assert_eq!(run_command(&missing, Some(&task_event())), Err("task.nope".into()));
        assert_eq!(run_command(&with, None), Err("task.version".into()), "지금 실행 without an event has no version");
        let plain = With { command: Some("python3 scripts/foo.py".into()), ..With::default() };
        assert_eq!(run_command(&plain, None).as_deref(), Ok("python3 scripts/foo.py"));
        let sent = run_prompt("/release v0.6.4", "Mark {{task.key}} done when it worked.", Some(&task_event()));
        assert!(sent.starts_with("[Mighty Code] Run exactly this, once, and nothing else:\n\n/release v0.6.4\n\n"), "{sent}");
        assert!(sent.contains("Context:\nMark MC-12 done when it worked.") && sent.contains("<event"), "{sent}");
        assert!(!run_prompt("x", "", None).contains("Context:"));
    }

    #[test]
    fn a_develop_s_request_carries_its_worktree_steps() {
        let place = Place { branch: "mc/fix-ci".into(), base: "main".into() };
        let sent =
            develop_prompt("Fix {{event.subject}}.", &With::default(), Some(&task_event()), Some(&place), Some(("dev", "Use MightyWork.")));
        assert!(sent.starts_with("Fix MC-12."), "{sent}");
        assert!(sent.contains("branch mc/fix-ci, made from main") && sent.contains("gh pr create") && sent.contains("Do not merge it"));
        assert!(sent.ends_with("Use MightyWork."), "{sent}");
        let merging = With { pr: Some(true), merge: Some(true), ..With::default() };
        assert!(develop_prompt("x", &merging, None, None, None).contains("gh pr merge"));
        let local = With { pr: Some(false), ..With::default() };
        let sent = develop_prompt("x", &local, None, None, None);
        assert!(sent.contains("Do not push") && !sent.contains("gh pr create") && sent.contains("Work in this folder."));
    }

    #[test]
    fn an_emit_s_data_is_filled_from_the_event() {
        let with = With {
            event: Some("release.asked".into()),
            data: [("version".to_string(), "{{task.version}}".to_string()), ("subject".into(), "{{event.subject}}".into())].into(),
            ..With::default()
        };
        assert_eq!(emit_data(&with, Some(&task_event())).unwrap(), serde_json::json!({"version": "v0.6.4", "subject": "MC-12"}));
        assert_eq!(emit_data(&with, None), Err("event.subject".into()), "the first by name");
        assert_eq!(placeholders("a {{ x.y }} b {{}} {{bad name}} {{z"), vec!["x.y".to_string()]);
    }
}
