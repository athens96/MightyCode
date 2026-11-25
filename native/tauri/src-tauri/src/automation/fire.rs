//! MC-32: one fire of a schedule or trigger, and its record following the pane.
//!
//! The daily budget is checked first (the usage ledger's spend today plus this agent's average run, `mighty_delegation::compare`'s
//! estimate): a run that would pass it is skipped with a record. Then a fresh pane is made (in its own worktree when asked) or a
//! heartbeat finds its named pane (made once when missing), and the prompt goes through the composer's own path (a busy pane queues
//! it). The record then follows the pane's events: a card waiting (told to the phone and the OS at once), the run's end with its
//! cost, and a pull request link the pane printed.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use mighty_core::automation::engine;
use mighty_core::automation::records::{RecordState, RunRecord};
use mighty_core::automation::spec::FireMode;
use mighty_core::automation::watch::{TriggerEvent, prompt_with_event};
use mighty_core::control::events::{BlockReason, ControlEvent, EventBody, PaneState, StateEvent};
use mighty_delegation::compare::{budget_warning, estimate_run_cost};

use super::{Job, awaiting_start, publish, with_history};
use crate::host::Host;

/// What fires.
pub struct FireRequest {
    pub job: Job,
    /// Why, in a few words (`예약`, `놓친 예약`, a PR's title).
    pub reason: String,
    pub scheduled_for: Option<DateTime<Utc>>,
    pub event: Option<TriggerEvent>,
    pub retry_of: Option<String>,
    /// A person asked for it (다시 실행, 지금 실행): a sleeping bot takes it (MC-84).
    pub asked: bool,
}

/// MC-97: what a fire came to, for the event inbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FireOutcome {
    /// Its pane has the prompt (or queued it).
    Started,
    /// Skipped: its bot is asleep (the words saying so).
    Asleep(String),
    /// Skipped: past the daily budget or its bot's own.
    OverBudget(String),
    /// It could not start (no pane, the prompt not sent).
    Failed(String),
}

/// A fire's record id and what it came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fired {
    pub id: String,
    pub outcome: FireOutcome,
}

/// A fire that does not run (missed while the app was closed with `catchUp: skip`): its record and event.
pub fn skip(host: &Host, job: &Job, reason: String, scheduled_for: Option<DateTime<Utc>>) {
    let id = uuid::Uuid::new_v4().simple().to_string();
    let record = with_history(host, |h| engine::skip(h, &job.reference(), reason, scheduled_for, id, Utc::now()));
    publish(host, "skipped", &record);
}

/// Whether the run would pass the daily budget: the words saying so.
fn over_budget(host: &Arc<Host>, job: &Job) -> Option<String> {
    let action = job.action();
    let (records, spent, budget) = crate::usage_report::budget_reading(host);
    let estimate = estimate_run_cost(&records, action.agent(), action.model.as_deref());
    budget_warning(spent, budget, &[estimate]).map(|warning| {
        host.locale.get_with(
            "automation.skip.budget",
            &[("spent", &format!("${:.2}", warning.spent)), ("budget", &format!("${:.2}", warning.budget))],
        )
    })
}

/// The pane a heartbeat wakes: the workspace's agent pane with that title; MC-58: or, for `@handle`, the bot of that name in any
/// workspace.
fn heartbeat_pane(host: &Host, workspace: &str, title: &str) -> Option<String> {
    let view = host.view();
    if title.trim_start().starts_with('@') {
        return view.snapshot.bot_pane(title).map(|s| s.id().to_string());
    }
    view.snapshot
        .sessions()
        .iter()
        .find(|s| s.workspace_id() == workspace && s.kind() == "claude" && s.title().trim() == title.trim())
        .map(|s| s.id().to_string())
}

fn fail(host: &Arc<Host>, id: &str, error: String) {
    if let Some(record) = with_history(host, |h| engine::fail(h, id, &error, Utc::now())) {
        host.log.line(&format!("automation: '{}' failed to start", record.name));
        publish(host, "finished", &record);
        // MC-73: a bot's run that could not start is a ❌ report.
        crate::bot_report::on_record(host, &record);
    }
}

/// Fires `request`: its record's id and what it came to.
pub async fn fire(host: &Arc<Host>, request: FireRequest) -> Fired {
    let FireRequest { job, reason, scheduled_for, event, retry_of, asked } = request;
    // MC-64: a bot asleep or past its own daily budget skips, as a run past the daily budget does; MC-84: a person's own request
    // reaches a sleeping bot.
    let bot = job.action().bot();
    let asleep = !asked && bot.as_deref().is_some_and(|handle| super::bot_asleep(host, handle));
    let unavailable = bot.and_then(|handle| super::bot_unavailable_for(host, &handle, !asked));
    let start = engine::Begin {
        reason,
        scheduled_for,
        over_budget: unavailable.or_else(|| over_budget(host, &job)),
        retry_of,
        event_url: event.as_ref().and_then(|e| e.url.clone()),
    };
    let id = uuid::Uuid::new_v4().simple().to_string();
    let record = with_history(host, |h| engine::begin(h, &job.reference(), start, id.clone(), Utc::now()));
    if record.state == RecordState::Skipped {
        publish(host, "skipped", &record);
        // MC-73: a bot's run skipped over its budget is a ❌ report saying why. MC-84: one skipped because the person put the bot
        // to sleep is kept in the history only: a report at every fire said nothing new.
        if !asleep {
            crate::bot_report::on_record(host, &record);
        }
        let words = record.error.clone().unwrap_or_default();
        let outcome = if asleep { FireOutcome::Asleep(words) } else { FireOutcome::OverBudget(words) };
        return Fired { id, outcome };
    }
    publish(host, "fired", &record);
    host.log.line(&format!("automation: '{}' fired ({})", job.name(), job.kind()));

    let action = job.action().clone();
    let prompt = match &event {
        Some(event) => prompt_with_event(&action.prompt, event),
        None => action.prompt.clone(),
    };
    let found = match action.mode {
        FireMode::Heartbeat => action.pane.as_deref().and_then(|title| heartbeat_pane(host, &job.workspace, title)),
        FireMode::New => None,
    };
    // MC-58: a bot is never made by a schedule: a heartbeat to a bot no pane is fails.
    let bot_gone = action.mode == FireMode::Heartbeat && action.pane.as_deref().is_some_and(|t| t.trim_start().starts_with('@'));
    if found.is_none() && bot_gone {
        let handle = action.pane.as_deref().unwrap_or_default().trim().trim_start_matches('@').to_string();
        let message = host.locale.get_with("bot.ask.noBot", &[("handle", &handle)]);
        fail(host, &id, message.clone());
        return Fired { id, outcome: FireOutcome::Failed(message) };
    }
    let pane = match found {
        Some(pane) => pane,
        None => {
            let title = match action.mode {
                FireMode::Heartbeat => action.pane.clone(),
                FireMode::New => Some(job.name().to_string()),
            };
            let made = crate::phone::automation_create(
                host.clone(),
                job.workspace.clone(),
                action.agent().to_string(),
                action.model.clone(),
                title,
                action.worktree,
            )
            .await;
            match made {
                Ok(pane) => pane,
                Err((_, message)) => {
                    fail(host, &id, message.clone());
                    return Fired { id, outcome: FireOutcome::Failed(message) };
                }
            }
        }
    };
    with_history(host, |h| engine::attach(h, &id, &pane));
    let outcome = match crate::phone::control_send(host, &pane, &prompt).await {
        Ok(outcome) if outcome == "queued" => {
            awaiting_start().insert(id.clone());
            FireOutcome::Started
        }
        Ok(_) => FireOutcome::Started,
        Err((_, message)) => {
            fail(host, &id, message.clone());
            FireOutcome::Failed(message)
        }
    };
    Fired { id, outcome }
}

/// The OS notification and the phone push of a fire that needs the person.
fn needs_you(host: &Arc<Host>, record: &RunRecord, reason: Option<BlockReason>) {
    let l = host.locale;
    let title = l.get_with("automation.notify.title", &[("name", &record.name)]);
    let question = reason == Some(BlockReason::Question);
    let body = l.get(if question { "automation.notify.question" } else { "automation.notify.approval" });
    crate::notifications::notify_custom(host, title.clone(), body.clone());
    if let Some(push) = crate::mobile::push(host) {
        let kind = if question { mighty_remote::push::message::QUESTION } else { mighty_remote::push::message::PERMISSION };
        let _ = push.send_custom(kind, &title, &body, record.pane.as_deref().unwrap_or(""));
    }
}

/// A bus event: the record running in its pane follows it.
pub fn on_event(host: &Arc<Host>, event: &ControlEvent) {
    let pane = event.pane.pane.as_str();
    if pane.is_empty() || event.is_automation() {
        return;
    }
    let Some(record_id) = open_record(pane) else { return };
    match &event.body {
        EventBody::State { from, to, event: announced, reason } => {
            if *announced == Some(StateEvent::Started) {
                awaiting_start().remove(&record_id);
            }
            if awaiting_start().contains(&record_id) {
                return;
            }
            if *to == PaneState::Blocked {
                if let Some(record) = with_history(host, |h| engine::blocked(h, pane)) {
                    needs_you(host, &record, *reason);
                    publish(host, "blocked", &record);
                }
            } else if *from == PaneState::Blocked && *to == PaneState::Running {
                with_history(host, |h| engine::resumed(h, pane));
            }
        }
        EventBody::RunCompleted { status, cost_usd, .. } => {
            if awaiting_start().contains(&record_id) {
                return;
            }
            let (host, pane, status, cost) = (host.clone(), pane.to_string(), status.clone(), *cost_usd);
            tauri::async_runtime::spawn(async move {
                let words = last_words(&host, &pane).await;
                if let Some(record) = with_history(&host, |h| engine::finish(h, &pane, &status, cost, &words, Utc::now())) {
                    publish(&host, "finished", &record);
                    // MC-73: the bot's report of the run: the one it filed, or one made from its answer.
                    crate::bot_report::on_record(&host, &record);
                }
            });
        }
        _ => {}
    }
}

/// MC-73: the unsettled record running in `pane`, whole.
pub(crate) fn open_record_of(host: &Host, pane: &str) -> Option<RunRecord> {
    let id = open_record(pane)?;
    with_history(host, |h| h.records.iter().find(|r| r.id == id).cloned())
}

/// The id of the unsettled record running in `pane`.
fn open_record(pane: &str) -> Option<String> {
    let history = super::lock(&super::automation().history);
    history.records.iter().rev().find(|r| !r.state.finished() && r.pane.as_deref() == Some(pane)).map(|r| r.id.clone())
}

/// The pane's last words (a pull request link is looked for in them).
async fn last_words(host: &Arc<Host>, pane: &str) -> String {
    let loader = host.clone();
    let id = pane.to_string();
    let _ = tauri::async_runtime::spawn_blocking(move || crate::bodies::ensure(&loader, &[id.as_str()])).await;
    let view = host.view();
    let Some(session) = view.snapshot.session(pane) else { return String::new() };
    let logs = session.logs();
    let from = logs.len().saturating_sub(40);
    logs[from..].iter().map(|e| e.text()).collect::<Vec<_>>().join("\n")
}
