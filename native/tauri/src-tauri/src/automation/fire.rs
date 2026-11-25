//! MC-32: one fire of a schedule or trigger, and its record following the pane.
//!
//! The daily budget is checked first (the usage ledger's spend today plus this agent's average run, `mighty_delegation::compare`'s
//! estimate): a run that would pass it is skipped with a record. Then a fresh pane is made (in its own worktree when asked) or a
//! heartbeat finds its named pane (made once when missing), and the prompt goes through the composer's own path (a busy pane queues
//! it). The record then follows the pane's events: a card waiting (told to the phone and the OS at once), the run's end with its
//! cost, and a pull request link the pane printed.
//!
//! MC-99: by the action's kind (`do`):
//! - `ask`: as above.
//! - `run`: its one command filled from the event (a value missing refuses it) and framed ("run exactly this"), sent the same way.
//! - `develop`: a worktree of its own, through the machinery a person's 새 worktree에서 실행 uses: a bot's job runs as that bot's
//!   child in a new worktree beside its pane (`delegation::start_human_child`, as the composer's second send does), another a fresh
//!   pane made in its own worktree (`pane_worktree::create_without_asking`, as a workspace in worktree mode makes one); never in the
//!   workspace folder itself. The request carries the worktree steps (`mighty_core::automation::action`). When its run ends the
//!   app event `bot.develop.done` or `bot.develop.failed` follows, about the event's subject, with the pull request link and the
//!   branch when known (an `event` trigger on `bot.develop.*` takes it).
//! - `emit`: no pane: another event through chain.rs's own path.
//!
//! Whatever ends a run tells the policy (policy.rs), so what waited for it may start.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use mighty_core::automation::action::{self as acts, Place};
use mighty_core::automation::engine;
use mighty_core::automation::records::{RecordState, RunRecord};
use mighty_core::automation::spec::{ActionKind, FireMode};
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
    /// MC-99: an emit sent its event (the words saying which and to how many).
    Emitted(String),
}

/// MC-99: a `develop` on its way: what its end event says.
#[derive(Debug, Clone)]
struct Develop {
    /// The event's subject (`PR#12`, `MC-7`), its type and source.
    subject: String,
    kind: String,
    /// The bot it works for (`@name`), else none.
    bot: Option<String>,
    job: String,
    workspace: String,
    /// How many actions in a row made the event it works on (its end event goes one deeper).
    depth: u32,
}

/// The develops whose run has not ended, by record id.
static DEVELOPS: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<String, Develop>>> =
    std::sync::LazyLock::new(Default::default);

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
        ended(host, &record);
    }
}

/// MC-99: a record settled: a develop's end event goes out, and the policy hears its run ended.
fn ended(host: &Arc<Host>, record: &RunRecord) {
    let develop = super::lock(&DEVELOPS).remove(&record.id);
    if let Some(develop) = develop {
        develop_ended(host, record, develop);
    }
    super::policy::released(host, record);
}

/// MC-99: `bot.develop.done` (or `.failed`) about the develop's subject, from its bot (`bot:@name`, else `app`), with the run's
/// pull request link and its branch when known.
fn develop_ended(host: &Arc<Host>, record: &RunRecord, develop: Develop) {
    let done = record.state == RecordState::Done;
    let mut fields = std::collections::BTreeMap::new();
    let mut put = |key: &str, value: Option<String>| {
        if let Some(value) = value.filter(|v| !v.trim().is_empty()) {
            fields.insert(key.to_string(), value);
        }
    };
    let pr = record.url.clone().filter(|u| u.contains("/pull/") || u.contains("/merge_requests/"));
    let branch = record.pane.as_deref().and_then(|pane| {
        let view = host.view();
        view.snapshot.session(pane).and_then(|s| s.pane_worktree.as_ref().map(|w| w.branch().to_string()))
    });
    put("develop.pr", pr.clone());
    put("develop.branch", branch);
    put("develop.rule", Some(develop.job.clone()));
    put("develop.workspace", Some(develop.workspace.clone()));
    put("develop.of", Some(develop.kind.clone()));
    put("develop.error", record.error.clone());
    put("bot.handle", develop.bot.as_deref().map(|b| b.trim_start_matches('@').to_string()));
    let outcome = if done { "done" } else { "failed" };
    let name = format!("bot.develop.{outcome}");
    let source = develop.bot.as_deref().map_or_else(|| "app".to_string(), |b| format!("bot:{b}"));
    let mut event = TriggerEvent {
        id: format!("develop:{}", record.id),
        title: format!("{} · {} {outcome}", develop.subject, develop.job),
        url: pr,
        fields,
        ..TriggerEvent::default()
    }
    .with_envelope(&source, &name, &develop.subject, develop.bot.as_deref());
    event.fields.insert("event.depth".into(), (develop.depth + 1).to_string());
    super::chain::deliver_named(host, &name, event);
}

/// Fires `request`: its record's id and what it came to.
pub async fn fire(host: &Arc<Host>, request: FireRequest) -> Fired {
    let FireRequest { job, reason, scheduled_for, event, retry_of, asked } = request;
    let action = job.action().clone();
    // MC-99: an emit runs no agent: another event, and nothing to record.
    if action.kind == ActionKind::Emit {
        let outcome = match super::chain::emit_action(host, &job, &action, event.as_ref()) {
            Ok(words) => FireOutcome::Emitted(words),
            Err(words) => FireOutcome::Failed(words),
        };
        return Fired { id: String::new(), outcome };
    }
    // MC-64: a bot asleep or past its own daily budget skips, as a run past the daily budget does; MC-84: a person's own request
    // reaches a sleeping bot.
    let bot = action.bot();
    let asleep = !asked && bot.as_deref().is_some_and(|handle| super::bot_asleep(host, handle));
    let unavailable = bot.clone().and_then(|handle| super::bot_unavailable_for(host, &handle, !asked));
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
    host.log.line(&format!("automation: '{}' fired ({}, {})", job.name(), job.kind(), action.kind.name()));

    // MC-99: a run's command filled from the event; one it lacks a value for never runs.
    let prompt = match action.kind {
        ActionKind::Run => match acts::run_command(&action.with, event.as_ref()) {
            Ok(command) => acts::run_prompt(&command, &action.prompt, event.as_ref()),
            Err(missing) => {
                let message = host.locale.get_with("automation.run.missing", &[("field", &missing)]);
                fail(host, &id, message.clone());
                return Fired { id, outcome: FireOutcome::Failed(message) };
            }
        },
        _ => match &event {
            Some(event) => prompt_with_event(&action.prompt, event),
            None => action.prompt.clone(),
        },
    };
    if action.kind == ActionKind::Develop {
        let outcome = develop(host, &job, &id, event.as_ref()).await;
        return Fired { id, outcome };
    }
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
    let outcome = send(host, &id, &pane, &prompt).await;
    Fired { id, outcome }
}

/// The prompt sent through the composer's own path; a busy pane queues it (its next run is the record's).
async fn send(host: &Arc<Host>, id: &str, pane: &str, prompt: &str) -> FireOutcome {
    match crate::phone::control_send(host, pane, prompt).await {
        Ok(outcome) if outcome == "queued" => {
            awaiting_start().insert(id.to_string());
            FireOutcome::Started
        }
        Ok(_) => FireOutcome::Started,
        Err((_, message)) => {
            fail(host, id, message.clone());
            FireOutcome::Failed(message)
        }
    }
}

/// MC-99: a `develop` of record `id`: a bot's as its child in a new worktree beside its pane, another's as a fresh pane in its own
/// worktree; a workspace without one (no git, verification mode) fails it rather than work in the folder itself.
async fn develop(host: &Arc<Host>, job: &super::Job, id: &str, event: Option<&TriggerEvent>) -> FireOutcome {
    let action = job.action().clone();
    let l = host.locale;
    let failed = |message: String| {
        fail(host, id, message.clone());
        FireOutcome::Failed(message)
    };
    // Watched from here on, so a start that fails also ends with `bot.develop.failed`.
    let develop = Develop {
        subject: event.map(|e| e.subject.clone()).filter(|s| !s.is_empty()).unwrap_or_else(|| job.name().to_string()),
        kind: event.map(|e| e.kind.clone()).unwrap_or_default(),
        bot: action.bot().map(|handle| format!("@{handle}")),
        job: job.name().to_string(),
        workspace: job.workspace_name.clone(),
        depth: event.and_then(|e| e.fields.get("event.depth")).and_then(|d| d.parse().ok()).unwrap_or(0),
    };
    super::lock(&DEVELOPS).insert(id.to_string(), develop);
    if let Some(handle) = action.bot() {
        // A bot's: its own child, as the composer's second send makes one (its folder, agent and permission; its card in the bot's
        // pane), its instructions in the request (the child is no bot).
        let found = host.view().snapshot.bot_pane(&handle).map(|p| (p.id().to_string(), p.bot.clone()));
        let Some((parent, bot)) = found else { return failed(l.get_with("bot.ask.noBot", &[("handle", &handle)])) };
        let instructions = bot.and_then(|b| b.instructions).unwrap_or_default();
        let prompt = acts::develop_prompt(&action.prompt, &action.with, event, None, Some((&handle, &instructions)));
        let Some(app) = crate::delegation::app() else { return failed(l.get("delegation.human.error.switchedOff")) };
        let folder = {
            let view = host.view();
            view.snapshot.session(&parent).map(|p| {
                p.working_folder
                    .clone()
                    .unwrap_or_else(|| view.snapshot.workspace(p.workspace_id()).map(|w| w.path().to_string()).unwrap_or_default())
            })
        };
        if !host.delegation_live() || !folder.is_some_and(|f| crate::delegation::may_touch(host, std::path::Path::new(&f))) {
            return failed(crate::setup::verification_mode());
        }
        let child_failed: Arc<dyn Fn(String) -> mighty_platform::BoxFuture<'static, ()> + Send + Sync> = {
            let (host, id) = (host.clone(), id.to_string());
            Arc::new(move |_child: String| {
                let (host, id) = (host.clone(), id.clone());
                Box::pin(async move { fail(&host, &id, mighty_run::strings::get("delegation.human.error.startFailed")) })
            })
        };
        let answer = app.start_human_child(&parent, &prompt, vec![], false, child_failed, None, true).await;
        return match answer {
            mighty_delegation::tools::delegate::HumanChildStart::Started(child) => {
                with_history(host, |h| engine::attach(h, id, &child.id));
                host.log.line(&format!("automation: '{}' develops in @{handle}'s child on {}", job.name(), child.branch));
                FireOutcome::Started
            }
            mighty_delegation::tools::delegate::HumanChildStart::Refused(reason) => failed(mighty_delegation::text::reason(reason, &l)),
            mighty_delegation::tools::delegate::HumanChildStart::Failed(message) => failed(message),
        };
    }
    // Another's: a fresh pane forced into a worktree of its own, as a workspace in worktree mode makes one.
    let made = crate::phone::automation_create(
        host.clone(),
        job.workspace.clone(),
        action.agent().to_string(),
        action.model.clone(),
        Some(job.name().to_string()),
        true,
    )
    .await;
    let pane = match made {
        Ok(pane) => pane,
        Err((_, message)) => return failed(message),
    };
    let place = host
        .view()
        .snapshot
        .session(&pane)
        .and_then(|s| s.pane_worktree.as_ref().map(|w| Place { branch: w.branch().into(), base: w.base_branch().into() }));
    let Some(place) = place else {
        // No worktree was made (not a git repository, verification mode): the pane goes, nothing runs in the folder itself.
        let _ = crate::frame::close_session(host, &pane).await;
        return failed(l.get("automation.develop.noWorktree"));
    };
    with_history(host, |h| engine::attach(h, id, &pane));
    let prompt = acts::develop_prompt(&action.prompt, &action.with, event, Some(&place), None);
    send(host, id, &pane, &prompt).await
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
                    ended(&host, &record);
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
