//! MC-66: triggers fired where things happen on this computer, not by a look: an event `mighty event emit` sends (`release.done`),
//! and another bot's request ending (a bot chain: when @a is done, @b goes on).
//!
//! A chain is one level of hand-offs deep at most twice: a bot woken by a chain may wake one more, never a third, and a bot never
//! wakes itself, so bots cannot call each other round in a loop. Each event's values are the prompt's `{{…}}` (`data.version`,
//! `bot.handle`, `bot.answer`).

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, LazyLock, Mutex};

use mighty_core::automation::spec::{BotOutcome, TriggerSource, event_matches, valid_event_name};
use mighty_core::automation::watch::{TriggerEvent, data_fields, prompt_with_event};
use mighty_core::control::events::{ControlEvent, EventBody, PaneState};
use mighty_core::model::PaneBot;
use serde_json::Value;

use super::{Item, Job, lock};
use crate::host::Host;

/// How many hand-offs a chain makes at most (a bot woken by a chain may wake one more).
pub const MAX_CHAIN_DEPTH: u32 = 2;
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

/// `mighty event emit <name> [--data JSON]`: fires every trigger listening for `name`; how many.
pub fn emit(host: &Arc<Host>, name: &str, data: Option<&Value>) -> Result<usize, String> {
    let name = name.trim();
    if !valid_event_name(name, false) {
        return Err(format!("'{name}' is not an event name: lowercase letters, digits, . _ - (starting with a letter)"));
    }
    let jobs = super::jobs();
    let fired = listening(&jobs, name);
    let mut fields = data.map(data_fields).unwrap_or_default();
    fields.insert("event.name".into(), name.to_string());
    let detail: String = data
        .map(|d| serde_json::to_string_pretty(d).unwrap_or_default())
        .unwrap_or_default()
        .chars()
        .take(mighty_core::automation::watch::MAX_EVENT_CHARS)
        .collect();
    for job in &fired {
        let event = TriggerEvent {
            id: uuid::Uuid::new_v4().simple().to_string(),
            title: name.to_string(),
            url: None,
            detail: detail.clone(),
            fields: fields.clone(),
        };
        super::triggers::fire_event(host, job, event);
    }
    host.log.line(&format!("automation: event '{name}' fired {} trigger(s)", fired.len()));
    Ok(fired.len())
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
        for job in &jobs {
            let event = TriggerEvent {
                id: uuid::Uuid::new_v4().simple().to_string(),
                title: format!("@{source} {}", outcome.name()),
                url: None,
                detail: words.clone(),
                fields: fields.clone(),
            };
            // The bot it wakes works on this request one level deeper.
            if let Some(target) = job.action().bot() {
                let target_pane = host.view().snapshot.bot_pane(&target).map(|p| p.id().to_string());
                if let Some(target_pane) = target_pane {
                    let sent = prompt_with_event(&job.action().prompt, &event).trim().to_string();
                    lock(&CHAINED).insert(target_pane, (depth + 1, sent));
                }
            }
            super::triggers::fire_event(&host, job, event);
        }
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
        assert!(!wakes(&TriggerSource::Webhook, "콩이", BotOutcome::Done, None));
        let chained: HashMap<String, (u32, String)> = [("p".to_string(), (2, "go on".to_string()))].into();
        assert_eq!(depth_of(&chained, "p", Some("go on")), 2);
        assert_eq!(depth_of(&chained, "p", Some("a request of the person")), 0, "the person asked it since");
        assert_eq!(depth_of(&chained, "q", Some("go on")), 0);
    }
}
