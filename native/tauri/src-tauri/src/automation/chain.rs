//! MC-66: triggers fired where things happen on this computer, not by a look: an event `mighty event emit` sends (`release.done`),
//! and another bot's request ending (a bot chain: when @a is done, @b goes on).
//!
//! A chain is one level of hand-offs deep at most twice: a bot woken by a chain may wake one more, never a third, and a bot never
//! wakes itself, so bots cannot call each other round in a loop. Each event's values are the prompt's `{{…}}` (`data.version`,
//! `bot.handle`, `bot.answer`). MC-97: an emitted event is `app.<name>` from `app`, about its `data.subject` (else its name); a
//! chain's is `bot.run.done` or `bot.run.failed` from `bot:@name`, about and by `@name`.
//!
//! MC-99: a trigger's `emit` action sends its event here as `mighty event emit` does ([`emit_action`]), its data filled from the
//! event that fired it, and a `develop`'s end (`bot.develop.done`, `.failed`) reaches the `event` triggers named for it
//! ([`deliver_named`]). Each such event carries how many actions made it (`event.depth`): past [`MAX_EMIT_DEPTH`] it goes nowhere,
//! so rules that emit to each other cannot loop.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, LazyLock, Mutex};

use mighty_core::automation::action::emit_data;
use mighty_core::automation::spec::{Action, BotOutcome, TriggerSource, event_matches, valid_event_name};
use mighty_core::automation::watch::{TriggerEvent, data_fields, prompt_with_event};
use mighty_core::control::events::{ControlEvent, EventBody, PaneState};
use mighty_core::model::PaneBot;
use serde_json::Value;

use super::{Item, Job, lock};
use crate::host::Host;

/// How many hand-offs a chain makes at most (a bot woken by a chain may wake one more).
pub const MAX_CHAIN_DEPTH: u32 = 2;
/// MC-99: how many actions in a row may make events (an emit of an emit of an emit).
pub const MAX_EMIT_DEPTH: u32 = 3;
/// The most of a bot's answer a chained prompt carries.
const ANSWER_CHARS: usize = 4_000;

/// The requests a chain sent, by the pane they went to: how deep in the chain, and the words sent (the pane's request while it works
/// on them).
static CHAINED: LazyLock<Mutex<HashMap<String, (u32, String)>>> = LazyLock::new(Default::default);

/// The triggers of `jobs` an event named `name` fires.
fn listening<'a>(jobs: &'a [Job], name: &str) -> Vec<&'a Job> {
    jobs.iter()
        .filter(|j| j.runnable())
        .filter(|j| matches!(&j.item, Item::Trigger(t) if matches!(&t.on, TriggerSource::Event { name: pattern } if event_matches(pattern.trim(), name))))
        .collect()
}

/// `mighty event emit <name> [--data JSON]`: fires every trigger listening for `name` whose `where` it passes; how many.
pub fn emit(host: &Arc<Host>, name: &str, data: Option<&Value>) -> Result<usize, String> {
    let name = name.trim();
    if !valid_event_name(name, false) {
        return Err(format!("'{name}' is not an event name: lowercase letters, digits, . _ - (starting with a letter)"));
    }
    Ok(deliver_named(host, name, app_event(name, data)))
}

/// An app event `name` with `data` as its `data.*` (MC-97: `app.<name>` from `app`, about its `data.subject`, else its name).
fn app_event(name: &str, data: Option<&Value>) -> TriggerEvent {
    let mut fields = data.map(data_fields).unwrap_or_default();
    fields.insert("event.name".into(), name.to_string());
    let detail: String = data
        .map(|d| serde_json::to_string_pretty(d).unwrap_or_default())
        .unwrap_or_default()
        .chars()
        .take(mighty_core::automation::watch::MAX_EVENT_CHARS)
        .collect();
    let subject = fields.get("data.subject").cloned().unwrap_or_else(|| name.to_string());
    TriggerEvent { id: uuid::Uuid::new_v4().simple().to_string(), title: name.to_string(), detail, fields, ..TriggerEvent::default() }
        .with_envelope("app", &format!("app.{name}"), &subject, None)
}

/// How many actions in a row made `event` (0 for one from outside).
fn depth_of_event(event: &TriggerEvent) -> u32 {
    event.fields.get("event.depth").and_then(|d| d.parse().ok()).unwrap_or(0)
}

/// MC-99: `event` delivered to the `event` triggers listening for `name`; how many passed their `where`. One made past
/// [`MAX_EMIT_DEPTH`] actions in a row goes nowhere.
pub(crate) fn deliver_named(host: &Arc<Host>, name: &str, event: TriggerEvent) -> usize {
    if depth_of_event(&event) > MAX_EMIT_DEPTH {
        host.log.line(&format!("automation: event '{name}' stops after {MAX_EMIT_DEPTH} actions in a row"));
        return 0;
    }
    let jobs: Vec<Job> = listening(&super::jobs(), name).into_iter().cloned().collect();
    let passed = super::triggers::deliver(host, &jobs, event, false, |_, _| {});
    host.log.line(&format!("automation: event '{name}' fired {passed} of {} trigger(s)", jobs.len()));
    passed
}

/// MC-99: the event a trigger's action made goes one action deeper than the one that fired it.
pub(crate) fn deeper(event: &mut TriggerEvent, from: Option<&TriggerEvent>) {
    let depth = from.map_or(0, depth_of_event) + 1;
    event.fields.insert("event.depth".into(), depth.to_string());
}

/// MC-99: a trigger's `emit`: its event with its data filled from `event` (the one that fired it), one action deeper. The words of
/// what it sent, or why not (a value missing, too many in a row).
pub(crate) fn emit_action(host: &Arc<Host>, job: &Job, action: &Action, event: Option<&TriggerEvent>) -> Result<String, String> {
    let l = host.locale;
    let name = action.with.event.as_deref().unwrap_or("").trim().to_string();
    if !valid_event_name(&name, false) {
        return Err(l.get_with("automation.emit.bad", &[("name", &name)]));
    }
    if event.map_or(0, depth_of_event) >= MAX_EMIT_DEPTH {
        return Err(l.get_with("automation.emit.loop", &[("count", &MAX_EMIT_DEPTH.to_string())]));
    }
    let data = emit_data(&action.with, event).map_err(|missing| l.get_with("automation.run.missing", &[("field", &missing)]))?;
    let mut sent = app_event(&name, Some(&data));
    deeper(&mut sent, event);
    sent.fields.insert("event.from".into(), job.name().to_string());
    let passed = deliver_named(host, &name, sent);
    Ok(l.get_with("automation.emit.sent", &[("name", &name), ("count", &passed.to_string())]))
}

/// Whether the chain trigger `on` is woken by the bot `source` ending as `outcome`, for the bot `target` (never itself).
fn wakes(on: &TriggerSource, source: &str, outcome: BotOutcome, target: Option<&str>) -> bool {
    let TriggerSource::Bot { bot, on } = on else { return false };
    let named = bot.trim() == "@*" || PaneBot::key(bot) == PaneBot::key(source);
    let not_itself = target.is_none_or(|t| PaneBot::key(t) != PaneBot::key(source));
    named && not_itself && on.contains(&outcome)
}

/// How deep in a chain the pane's current request is: the depth a chain sent it at, while its latest request is that one; else 0.
fn depth_of(chained: &HashMap<String, (u32, String)>, pane: &str, latest: Option<&str>) -> u32 {
    chained.get(pane).filter(|(_, sent)| latest == Some(sent.as_str())).map_or(0, |(depth, _)| *depth)
}

/// A bus event: a bot's request done or failed wakes the chain triggers listening to it.
pub fn on_event(host: &Arc<Host>, event: &ControlEvent) {
    let EventBody::State { to, .. } = &event.body else { return };
    let outcome = match to {
        PaneState::Done => BotOutcome::Done,
        PaneState::Failed => BotOutcome::Failed,
        _ => return,
    };
    let pane = event.pane.pane.clone();
    let (source, latest) = {
        let view = host.view();
        let Some(session) = view.snapshot.session(&pane) else { return };
        let Some(bot) = session.bot.as_ref() else { return };
        let latest = session.logs().iter().rev().find(|e| e.kind() == "user").map(|e| e.text().trim().to_string());
        (bot.handle.clone(), latest)
    };
    let jobs: Vec<Job> = super::jobs()
        .into_iter()
        .filter(|j| j.runnable())
        .filter(|j| matches!(&j.item, Item::Trigger(t) if wakes(&t.on, &source, outcome, j.action().bot().as_deref())))
        .collect();
    if jobs.is_empty() {
        return;
    }
    let depth = depth_of(&lock(&CHAINED), &pane, latest.as_deref());
    if depth >= MAX_CHAIN_DEPTH {
        host.log.line(&format!("automation: @{source}'s chain stops at {MAX_CHAIN_DEPTH} hand-offs"));
        return;
    }
    let host = host.clone();
    tauri::async_runtime::spawn(async move {
        let answer = crate::bot::answer(&host, &pane).await;
        let words: String = answer.get("answer").and_then(Value::as_str).unwrap_or("").chars().take(ANSWER_CHARS).collect();
        let fields: BTreeMap<String, String> = [
            ("bot.handle".to_string(), source.clone()),
            ("bot.outcome".to_string(), outcome.name().to_string()),
            ("bot.answer".to_string(), words.clone()),
        ]
        .into_iter()
        .collect();
        let handle = format!("@{source}");
        let event = TriggerEvent {
            id: uuid::Uuid::new_v4().simple().to_string(),
            title: format!("{handle} {}", outcome.name()),
            detail: words,
            fields,
            ..TriggerEvent::default()
        }
        .with_envelope(&format!("bot:{handle}"), &format!("bot.run.{}", outcome.name()), &handle, Some(&handle));
        // The bot it wakes works on this request one level deeper.
        let deeper = |job: &Job, event: &TriggerEvent| {
            if let Some(target) = job.action().bot() {
                let target_pane = host.view().snapshot.bot_pane(&target).map(|p| p.id().to_string());
                if let Some(target_pane) = target_pane {
                    let sent = prompt_with_event(&job.action().prompt, event).trim().to_string();
                    lock(&CHAINED).insert(target_pane, (depth + 1, sent));
                }
            }
        };
        super::triggers::deliver(&host, &jobs, event, false, deeper);
    });
}

#[cfg(test)]
mod tests {
    use mighty_core::automation::spec::BotOutcome;

    use super::*;

    #[test]
    fn a_chain_wakes_the_named_bot_never_itself_and_stops_after_two_hand_offs() {
        let on = |bot: &str| TriggerSource::Bot { bot: bot.into(), on: vec![BotOutcome::Done] };
        assert!(wakes(&on("@콩이"), "콩이", BotOutcome::Done, Some("보리")));
        assert!(!wakes(&on("@콩이"), "콩이", BotOutcome::Failed, Some("보리")), "only the outcomes named");
        assert!(wakes(&on("@*"), "두부", BotOutcome::Done, Some("보리")), "@* is any bot");
        assert!(!wakes(&on("@*"), "보리", BotOutcome::Done, Some("보리")), "never itself");
        assert!(!wakes(&TriggerSource::webhook(), "콩이", BotOutcome::Done, None));
        let chained: HashMap<String, (u32, String)> = [("p".to_string(), (2, "go on".to_string()))].into();
        assert_eq!(depth_of(&chained, "p", Some("go on")), 2);
        assert_eq!(depth_of(&chained, "p", Some("a request of the person")), 0, "the person asked it since");
        assert_eq!(depth_of(&chained, "q", Some("go on")), 0);
    }
}
