//! What a status hook is told (MC-9): the event in variables, never in its arguments, and never a prompt, an answer or a file.

use super::events::{ControlEvent, EventBody};

/// The longest value a variable carries.
const MAX_VALUE_CHARS: usize = 200;

/// `text` fit for a variable: no control or bidirectional formatting character, white space folded, at most 200 characters.
pub fn clean(text: &str) -> String {
    let folded: String = text
        .chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .filter(|c| !c.is_control() && !matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200E}' | '\u{200F}'))
        .collect();
    let words = folded.split(' ').filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ");
    words.chars().take(MAX_VALUE_CHARS).collect()
}

/// The variables of a hook run for `event`: `MIGHTY_EVENT` (started, blocked, done, failed, stopped), `MIGHTY_STATE`,
/// `MIGHTY_PREVIOUS_STATE`, `MIGHTY_REASON` (approval or question, when blocked), the pane's and workspace's ids and names, the
/// agent and the time. `workspace_path` is the folder the hook runs in.
pub fn env(event: &ControlEvent, workspace_path: &str) -> Vec<(String, String)> {
    let mut values: Vec<(String, String)> = Vec::with_capacity(12);
    let mut put = |key: &str, value: &str| values.push((key.to_string(), clean(value)));
    if let EventBody::State { from, to, event: announced, reason } = &event.body {
        put("MIGHTY_EVENT", announced.map_or("", |e| e.name()));
        put("MIGHTY_STATE", to.name());
        put("MIGHTY_PREVIOUS_STATE", from.name());
        put("MIGHTY_REASON", reason.map_or("", |r| r.name()));
    }
    // MC-32: an automation run's end or skip: `MIGHTY_EVENT` is `automation.finished` or `automation.skipped`.
    if let EventBody::Automation(note) = &event.body {
        put("MIGHTY_EVENT", &format!("automation.{}", note.event));
        put("MIGHTY_STATE", &note.state);
        put("MIGHTY_AUTOMATION_NAME", &note.name);
        put("MIGHTY_AUTOMATION_KIND", &note.kind);
        put("MIGHTY_AUTOMATION_URL", note.url.as_deref().unwrap_or(""));
    }
    put("MIGHTY_PANE_ID", &event.pane.pane);
    put("MIGHTY_PANE_TITLE", &event.pane.title);
    put("MIGHTY_PANE_KIND", &event.pane.kind);
    put("MIGHTY_AGENT", &event.pane.agent);
    put("MIGHTY_WORKSPACE_ID", &event.pane.workspace);
    put("MIGHTY_WORKSPACE_NAME", &event.pane.workspace_name);
    put("MIGHTY_EVENT_AT", &event.at);
    // A path keeps its own characters (it may hold two spaces); only control characters go.
    values.push(("MIGHTY_WORKSPACE_PATH".into(), workspace_path.chars().filter(|c| !c.is_control()).collect()));
    values
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::events::{BlockReason, PaneRef, PaneState, StateEvent};

    #[test]
    fn a_hook_gets_the_event_in_variables_without_any_prompt() {
        let prompt = "deploy with AWS_SECRET=abc123 please";
        let event = ControlEvent {
            seq: 1,
            at: "2026-11-23T10:00:00+00:00".into(),
            pane: PaneRef {
                pane: "p1".into(),
                workspace: "w1".into(),
                title: "Fix\nthe   tests\u{202E}".into(),
                workspace_name: "app".into(),
                agent: "claude".into(),
                kind: "agent".into(),
            },
            body: EventBody::State {
                from: PaneState::Running,
                to: PaneState::Blocked,
                event: Some(StateEvent::Blocked),
                reason: Some(BlockReason::Question),
            },
        };
        let values = env(&event, "/Users/me/my  app");
        let get = |key: &str| values.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());
        assert_eq!(get("MIGHTY_EVENT"), Some("blocked"));
        assert_eq!(get("MIGHTY_STATE"), Some("blocked"));
        assert_eq!(get("MIGHTY_PREVIOUS_STATE"), Some("running"));
        assert_eq!(get("MIGHTY_REASON"), Some("question"));
        assert_eq!(get("MIGHTY_PANE_TITLE"), Some("Fix the tests"), "folded, no formatting characters");
        assert_eq!(get("MIGHTY_WORKSPACE_PATH"), Some("/Users/me/my  app"));
        assert!(values.iter().all(|(k, v)| k.starts_with("MIGHTY_") && !v.contains(prompt) && !v.contains("AWS_SECRET")));
        assert_eq!(clean(&"x".repeat(500)).len(), 200);
        let automation = ControlEvent {
            body: EventBody::Automation(crate::control::events::AutomationNote {
                event: "finished".into(),
                name: "deps check".into(),
                kind: "schedule".into(),
                state: "done".into(),
                url: Some("https://github.com/a/b/pull/9".into()),
                ..Default::default()
            }),
            ..event
        };
        let values = env(&automation, "/w");
        let get = |key: &str| values.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());
        assert_eq!(get("MIGHTY_EVENT"), Some("automation.finished"));
        assert_eq!(get("MIGHTY_AUTOMATION_NAME"), Some("deps check"));
        assert_eq!(get("MIGHTY_AUTOMATION_URL"), Some("https://github.com/a/b/pull/9"));
    }
}
