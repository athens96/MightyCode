//! MC-32's triggers while the app is open: a task per GitHub, Linear or standing-order trigger that is runnable (nothing runs for a
//! workspace without one), and the webhook listener while it is on and some webhook trigger listens.
//!
//! - GitHub and Linear are looked at every 5 minutes at most; the wait grows while nothing changes (to 30 minutes) or looks fail
//!   (offline, signed out: to an hour). A watch's first look only learns what is there; at most [`MAX_FIRES_A_LOOK`] new events fire
//!   per look, the rest are remembered as seen.
//! - A standing order's check runs in the workspace folder every `everyMinutes` (60 s limit); its agent wakes only when the output
//!   (with its exit code) changed, at most `dailyCap` times a day.
//! - MC-97: every event, whichever its source, goes through [`deliver`]: it is kept in the event inbox (event_inbox.rs), each trigger
//!   listening to it checks its `where`, and the ones it passes fire; what each did is noted beside the event.
//! - MC-98: a webhook's body is read into its event by its preset and `map` (`mighty_core::automation::mapping`); an MCP trigger
//!   calls its tool every `everyMinutes` (mcp_client.rs), compares the items with the last look's (`mcp_watch`) and delivers what
//!   changed, a few a look. A look that fails waits longer (as GitHub's, to an hour) and says why on the trigger's row and, once
//!   per change of reason, in the event inbox ([`problem_of`]).
//! - MC-99: an event that passed a trigger's `where` goes to its policy (policy.rs): it starts, waits for the person's approval or
//!   for a run to end, or is skipped, each noted in the inbox. An MCP look checks the server's own word on the tool first: one it
//!   marks as changing things is not called without `allowWrite`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use chrono::Local;
use mighty_core::automation::condition;
use mighty_core::automation::mapping::{self, WebhookMap, WebhookPreset};
use mighty_core::automation::mcp_watch::{self, McpItem};
use mighty_core::automation::spec::{GitHubEvent, TriggerSource};
use mighty_core::automation::watch::{Backoff, Observed, PollOutcome, TriggerEvent};

use super::{Item, Job, lock, store, webhook};
use crate::event_inbox::{Outcome, TriggerOutcome};
use crate::host::Host;

/// The most events one look fires.
pub const MAX_FIRES_A_LOOK: usize = 3;
/// How long a standing order's check may run.
const CHECK_TIMEOUT: Duration = Duration::from_secs(60);
/// The wait before a watch's first look (the app's start is busy enough).
const FIRST_LOOK: Duration = Duration::from_secs(20);

#[derive(Default)]
struct Watches {
    /// Trigger key → (what it watches, as JSON; its task).
    tasks: Mutex<HashMap<String, (String, tauri::async_runtime::JoinHandle<()>)>>,
    /// The listener's settings and task, while it serves.
    listener: Mutex<Option<(store::Listener, tauri::async_runtime::JoinHandle<()>)>>,
    listener_error: Mutex<Option<String>>,
    /// Path id → (trigger key, secret), for the listening triggers.
    secrets: Mutex<HashMap<String, (String, String)>>,
    deliveries: Arc<webhook::Deliveries>,
    /// MC-98: trigger key → why its source could not be read at its last look.
    problems: Mutex<HashMap<String, String>>,
}

fn watches() -> &'static Watches {
    static WATCHES: OnceLock<Watches> = OnceLock::new();
    WATCHES.get_or_init(Watches::default)
}

/// The tasks follow the runnable triggers; the listener follows its switch.
pub async fn sync(host: &Arc<Host>) {
    let jobs = super::jobs();
    let polled: Vec<&Job> = jobs.iter().filter(|j| j.runnable() && matches!(&j.item, Item::Trigger(t) if t.on.polled())).collect();
    {
        let mut tasks = lock(&watches().tasks);
        tasks.retain(|key, (shape, task)| {
            let keep = polled.iter().any(|j| &j.key == key && &watch_shape(j) == shape);
            if !keep {
                task.abort();
            }
            keep
        });
        for job in polled {
            if tasks.contains_key(&job.key) {
                continue;
            }
            let task = tauri::async_runtime::spawn(watch(Arc::downgrade(host), job.clone()));
            tasks.insert(job.key.clone(), (watch_shape(job), task));
        }
    }
    let keys: Vec<String> = jobs.iter().filter(|j| matches!(j.item, Item::Trigger(_))).map(|j| j.key.clone()).collect();
    super::with_state(|state| state.keep_triggers(&keys));
    sync_listener(host, &jobs).await;
}

/// Every task and the listener stopped.
pub fn stop_all() {
    for (_, (_, task)) in lock(&watches().tasks).drain() {
        task.abort();
    }
    let serving = lock(&watches().listener).take();
    if let Some((_, task)) = serving {
        task.abort();
    }
}

fn watch_shape(job: &Job) -> String {
    match &job.item {
        Item::Trigger(t) => format!("{}|{}", serde_json::to_string(t).unwrap_or_default(), job.workspace_path),
        Item::Schedule(_) => String::new(),
    }
}

async fn watch(weak: Weak<Host>, job: Job) {
    let Item::Trigger(trigger) = &job.item else { return };
    tokio::time::sleep(FIRST_LOOK).await;
    let mut backoff = Backoff::default();
    loop {
        let Some(host) = weak.upgrade() else { return };
        if host.closing() {
            return;
        }
        let wait = match &trigger.on {
            TriggerSource::Standing { command, every_minutes, daily_cap } => {
                standing(&host, &job, command, *daily_cap).await;
                Duration::from_secs(u64::from(*every_minutes) * 60)
            }
            TriggerSource::Github { events, repo } => backoff.after(github(&host, &job, events, repo.as_deref()).await),
            TriggerSource::Linear { states } => backoff.after(linear(&host, &job, states).await),
            // MC-98: its own pace; a failing look backs off as GitHub's does, never sooner than its pace.
            TriggerSource::Mcp { server, tool, args, every_minutes, item, allow_write, .. } => {
                let every = Duration::from_secs(u64::from(*every_minutes) * 60);
                match mcp(&host, &job, server, tool, args, item, *allow_write).await {
                    PollOutcome::Failed => backoff.after(PollOutcome::Failed).max(every),
                    outcome => {
                        backoff.after(outcome);
                        every
                    }
                }
            }
            // A webhook listens; MC-66: an event and a bot chain are fired where they happen.
            TriggerSource::Webhook { .. } | TriggerSource::Event { .. } | TriggerSource::Bot { .. } => return,
        };
        drop(host);
        tokio::time::sleep(wait).await;
    }
}

/// MC-97: an event reached the automation layer: kept in the event inbox, then each of `jobs` (the triggers listening to its source
/// and type) checks its `where`. The ones it passes fire (`before` first, a chain's bookkeeping), unless `held` (past a look's few);
/// the others are noted with the condition that failed. MC-99: a passing one goes through its policy ([`super::policy::admit`]).
/// How many passed.
pub(crate) fn deliver(host: &Arc<Host>, jobs: &[Job], event: TriggerEvent, held: bool, before: impl Fn(&Job, &TriggerEvent)) -> usize {
    let inbox = crate::event_inbox::arrived(host, &event);
    let mut passed = 0;
    for job in jobs {
        let Item::Trigger(trigger) = &job.item else { continue };
        if let Some(miss) = condition::first_miss(&trigger.conditions, &event) {
            host.log.line(&format!("automation: '{}' skips {} ({} {} {})", job.name(), event.kind, miss.field, miss.op, miss.expected));
            crate::event_inbox::note(host, &inbox, TriggerOutcome { miss: Some(miss), ..TriggerOutcome::new(job, Outcome::Unmatched) });
            continue;
        }
        passed += 1;
        if held {
            crate::event_inbox::note(host, &inbox, TriggerOutcome::new(job, Outcome::Held));
            continue;
        }
        before(job, &event);
        super::policy::admit(host, job, event.clone(), inbox.clone());
    }
    passed
}

/// What the inbox notes of a fire.
pub(crate) fn outcome_of(job: &Job, fired: super::fire::Fired) -> TriggerOutcome {
    use super::fire::FireOutcome;
    let (outcome, reason) = match fired.outcome {
        FireOutcome::Started => (Outcome::Fired, None),
        // MC-99: an emit fired another event (its words).
        FireOutcome::Emitted(words) => (Outcome::Fired, Some(words)),
        FireOutcome::Asleep(words) => (Outcome::Asleep, Some(words)),
        FireOutcome::OverBudget(words) => (Outcome::Budget, Some(words)),
        FireOutcome::Failed(words) => (Outcome::Failed, Some(words)),
    };
    let run = Some(fired.id).filter(|id| !id.is_empty());
    TriggerOutcome { reason: reason.filter(|r| !r.is_empty()), run, ..TriggerOutcome::new(job, outcome) }
}

/// MC-97: 시험 실행: the inbox's event `id` run again against the trigger `key`, one it reached: its `where` checked again, then the
/// usual fire (a person's request, as 다시 실행 is). Refused when the event or the trigger is gone. The words of what happened.
pub async fn replay(host: &Arc<Host>, id: &str, key: &str) -> Result<String, String> {
    let l = host.locale;
    let record = crate::event_inbox::get(host, id).ok_or_else(|| l.get("automation.events.gone"))?;
    if !record.triggers.iter().any(|t| t.key == key) {
        return Err(l.get("automation.events.gone"));
    }
    // MC-98: a source that could not be read made no event to run again.
    if record.triggers.iter().filter(|t| t.key == key).all(|t| t.outcome == Outcome::Unreachable) {
        return Err(l.get("automation.events.unreachableNoReplay"));
    }
    let job = super::job(key).filter(|j| j.runnable()).ok_or_else(|| l.get("automation.events.gone"))?;
    let Item::Trigger(trigger) = &job.item else { return Err(l.get("automation.events.gone")) };
    if let Some(miss) = condition::first_miss(&trigger.conditions, &record.event) {
        let words = l.get_with(
            "automation.events.stillUnmatched",
            &[("condition", &format!("{} {} {}", miss.field, miss.op, miss.expected)), ("seen", miss.seen.as_deref().unwrap_or("-"))],
        );
        crate::event_inbox::note(
            host,
            id,
            TriggerOutcome { miss: Some(miss), replay: true, ..TriggerOutcome::new(&job, Outcome::Unmatched) },
        );
        return Ok(words);
    }
    let request = super::fire::FireRequest {
        job: job.clone(),
        reason: l.get("automation.reason.replay"),
        scheduled_for: None,
        event: Some(record.event.clone()),
        retry_of: None,
        asked: true,
    };
    let fired = super::fire::fire(host, request).await;
    let outcome = TriggerOutcome { replay: true, ..outcome_of(&job, fired) };
    let words = match &outcome.reason {
        Some(reason) => reason.clone(),
        None => l.get_with("automation.events.replayed", &[("name", job.name())]),
    };
    crate::event_inbox::note(host, id, outcome);
    Ok(words)
}

// ── a standing order ─────────────────────────────────────────────────────────────────────────────────────────────────────

async fn standing(host: &Arc<Host>, job: &Job, command: &str, cap: u32) {
    let folder = std::path::Path::new(&job.workspace_path);
    if !folder.is_dir() {
        return;
    }
    let mut request = crate::repo_tasks::shell_request(command, folder);
    for (key, value) in crate::repo_tasks::login_shell_env(host, Some(folder)).await {
        request = request.env(key, value);
    }
    let outcome = mighty_delegation::run_actions::run_bounded(host.processes.as_ref(), request, CHECK_TIMEOUT).await;
    if let Some(error) = &outcome.error {
        host.log.line(&format!("automation: standing order '{}' did not start: {error}", job.name()));
        return;
    }
    let output = format!("{}\n[exit {}]", outcome.output.trim_end(), outcome.exit_code.map_or("-".into(), |c| c.to_string()));
    let today = Local::now().format("%Y-%m-%d").to_string();
    let observed = super::with_state(|state| state.standing.entry(job.key.clone()).or_default().observe(&output, &today, cap));
    super::save_state(host);
    match observed {
        Observed::Wake => {
            let tail: String = {
                let chars: Vec<char> = output.chars().collect();
                chars[chars.len().saturating_sub(2_000)..].iter().collect()
            };
            // MC-97: a check's change, about the trigger's own check. MC-101: with its command, for a request's `{{check.command}}`.
            let fields = [
                ("check.command".to_string(), command.to_string()),
                ("check.output".to_string(), tail.clone()),
                ("check.exit".to_string(), outcome.exit_code.map_or("-".into(), |c| c.to_string())),
            ];
            let event = TriggerEvent {
                id: format!("check:{}", mighty_core::automation::watch::output_hash(&output)),
                title: host.locale.get("automation.reason.changed"),
                detail: tail,
                fields: fields.into_iter().collect(),
                ..TriggerEvent::default()
            }
            .with_envelope("check", "check.output.changed", job.name(), None);
            deliver(host, std::slice::from_ref(job), event, false, |_, _| {});
        }
        Observed::Capped => host.log.line(&format!("automation: standing order '{}' changed, daily cap reached", job.name())),
        Observed::First | Observed::Same => {}
    }
}

/// A standing order's wakes today.
pub fn wakes_today(key: &str) -> u32 {
    let today = Local::now().format("%Y-%m-%d").to_string();
    super::with_state(|state| state.standing.get(key).map_or(0, |s| s.wakes_today(&today)))
}

// ── GitHub ───────────────────────────────────────────────────────────────────────────────────────────────────────────────

fn workspace(host: &Host, id: &str) -> Option<mighty_core::model::Workspace> {
    host.view().snapshot.workspace(id).cloned()
}

async fn github(host: &Arc<Host>, job: &Job, events: &[GitHubEvent], repo: Option<&str>) -> PollOutcome {
    let gh = match repo {
        // MC-64: a named repository (a bot's own), whatever the workspace's folder is.
        Some(repo) => super::github::Gh::for_repo(host, repo),
        None => match workspace(host, &job.workspace) {
            Some(found) => super::github::Gh::for_workspace(host, &found).await,
            None => return PollOutcome::Failed,
        },
    };
    let gh = match gh {
        Ok(gh) => gh,
        Err(error) => {
            host.log.line(&format!("automation: GitHub trigger '{}': {error}", job.name()));
            return PollOutcome::Failed;
        }
    };
    let (mut changed, mut failed) = (false, 0);
    for event in events {
        let key = format!("{}#{}", job.key, event.name());
        let endpoint = super::github::endpoint(*event, &gh.repo);
        let etag = super::with_state(|state| state.etags.get(&key).and_then(|e| e.get(&endpoint)).map(str::to_string));
        let answer = match gh.look(host, &endpoint, etag.as_deref()).await {
            Ok(answer) => answer,
            Err(error) => {
                host.log.line(&format!("automation: GitHub trigger '{}' {}: {error}", job.name(), event.name()));
                failed += 1;
                continue;
            }
        };
        if answer.status == 304 {
            continue;
        }
        let found = super::github::events_of(*event, &gh.repo, &answer.body);
        let fresh = super::with_state(|state| {
            state.etags.entry(key.clone()).or_default().set(&endpoint, answer.etag());
            state.seen.entry(key.clone()).or_default().new_of(found.iter().map(|e| e.id.as_str()))
        });
        super::save_state(host);
        // Oldest first; MC-97: each new one reaches the inbox, the ones the trigger's `where` takes fire, a few a look.
        let mut passed = 0;
        for event in found.into_iter().rev().filter(|e| fresh.contains(&e.id)) {
            passed += deliver(host, std::slice::from_ref(job), event, passed >= MAX_FIRES_A_LOOK, |_, _| {});
        }
        changed |= passed > 0;
    }
    if failed == events.len() && failed > 0 {
        PollOutcome::Failed
    } else if changed {
        PollOutcome::Changed
    } else {
        PollOutcome::Unchanged
    }
}

// ── Linear ───────────────────────────────────────────────────────────────────────────────────────────────────────────────

async fn linear(host: &Arc<Host>, job: &Job, states: &[String]) -> PollOutcome {
    use crate::trackers::IssueTracker as _;
    let Some(found) = workspace(host, &job.workspace) else { return PollOutcome::Failed };
    let listed = match crate::trackers::tracker_for(host, &found, "linear").await {
        Ok(tracker) => tracker.list(&crate::trackers::IssueQuery { filter: "all".into(), text: String::new() }).await,
        Err(error) => Err(error),
    };
    let issues = match listed {
        Ok(issues) => issues,
        Err(error) => {
            host.log.line(&format!("automation: Linear trigger '{}': {error}", job.name()));
            return PollOutcome::Failed;
        }
    };
    let pairs: Vec<(String, String)> = issues.iter().map(|i| (i.key.clone(), i.state.clone().unwrap_or_default())).collect();
    let entered = super::with_state(|state| state.linear.entry(job.key.clone()).or_default().entering(&pairs, states));
    super::save_state(host);
    let mut passed = 0;
    for key in &entered {
        let Some(issue) = issues.iter().find(|i| &i.key == key) else { continue };
        let state = issue.state.clone().unwrap_or_default();
        // MC-97: an issue that entered a state, with its values for a `where` and the prompt.
        let fields = [
            ("issue.key", issue.key.clone()),
            ("issue.title", issue.title.clone()),
            ("issue.state", state.clone()),
            ("issue.url", issue.url.clone().unwrap_or_default()),
            ("issue.priority", issue.priority.clone().unwrap_or_default()),
            ("state", state.clone()),
            ("labels", issue.labels.join(",")),
        ];
        let event = TriggerEvent {
            id: key.clone(),
            title: format!("{}: {} ({state})", issue.key, issue.title),
            url: issue.url.clone(),
            fields: fields.into_iter().filter(|(_, v)| !v.is_empty()).map(|(k, v)| (k.to_string(), v)).collect(),
            ..TriggerEvent::default()
        }
        .with_envelope("linear", "linear.issue.state_changed", &issue.key, None);
        passed += deliver(host, std::slice::from_ref(job), event, passed >= MAX_FIRES_A_LOOK, |_, _| {});
    }
    if passed > 0 { PollOutcome::Changed } else { PollOutcome::Unchanged }
}

// ── MC-98: an MCP tool ───────────────────────────────────────────────────────────────────────────────────────────────────────

/// Why the trigger `key`'s source could not be read at its last look, if it could not.
pub fn problem_of(key: &str) -> Option<String> {
    lock(&watches().problems).get(key).cloned()
}

/// The trigger's source read again, or not: its problem kept for its row, and noted in the event inbox when it is new or changed.
fn set_problem(host: &Arc<Host>, job: &Job, source: &str, problem: Option<String>) {
    let changed = {
        let mut problems = lock(&watches().problems);
        match &problem {
            Some(words) => problems.insert(job.key.clone(), words.clone()).as_deref() != Some(words.as_str()),
            None => problems.remove(&job.key).is_some(),
        }
    };
    if !changed {
        return;
    }
    crate::native::touch(host, crate::settings::SURFACE);
    let Some(words) = problem else { return };
    host.log.line(&format!("automation: MCP trigger '{}': {words}", job.name()));
    let event = TriggerEvent {
        id: format!("unreachable:{}", mighty_core::automation::watch::output_hash(&words)),
        title: words.clone(),
        ..TriggerEvent::default()
    }
    .with_envelope(source, "mcp.poll.failed", job.name(), None);
    crate::event_inbox::unreachable(host, job, &event, &words);
}

async fn mcp(
    host: &Arc<Host>,
    job: &Job,
    server: &str,
    tool: &str,
    args: &serde_json::Value,
    item: &McpItem,
    allow_write: bool,
) -> PollOutcome {
    let source = format!("mcp:{server}");
    let folder = Some(std::path::Path::new(&job.workspace_path)).filter(|f| !job.workspace_path.is_empty() && f.is_dir());
    let called = crate::mcp_client::call_tool(host, folder, server, tool, args, allow_write).await;
    let items = called.and_then(|result| {
        mcp_watch::items_of(&result, &item.list).cloned().ok_or_else(|| {
            let list = if item.list.trim().is_empty() { "/".to_string() } else { item.list.clone() };
            crate::mcp_client::McpError::NoList(list)
        })
    });
    let items = match items {
        Ok(items) => items,
        Err(error) => {
            set_problem(host, job, &source, Some(error.words(&host.locale, server, tool)));
            return PollOutcome::Failed;
        }
    };
    set_problem(host, job, &source, None);
    let found = super::with_state(|state| state.mcp.entry(job.key.clone()).or_default().look(&items, item));
    super::save_state(host);
    // In the list's order; each reaches the inbox, the ones the trigger's `where` takes fire, a few a look.
    let mut passed = 0;
    for change in &found {
        let event = mcp_watch::event_of(server, item, change);
        passed += deliver(host, std::slice::from_ref(job), event, passed >= MAX_FIRES_A_LOOK, |_, _| {});
    }
    if found.is_empty() { PollOutcome::Unchanged } else { PollOutcome::Changed }
}

// ── the webhook listener ─────────────────────────────────────────────────────────────────────────────────────────────────

/// The keychain item of a webhook trigger's secret.
fn secret_item(path_id: &str) -> String {
    format!("automation.webhook.{path_id}")
}

/// A webhook trigger's secret, made and kept in the keychain the first time.
pub async fn secret_of(host: &Arc<Host>, key: &str) -> Result<String, String> {
    let id = webhook::path_id(key);
    if let Some((_, secret)) = lock(&watches().secrets).get(&id) {
        return Ok(secret.clone());
    }
    let vault = crate::remote::secret_vault(host);
    let item = secret_item(&id);
    let made = tauri::async_runtime::spawn_blocking(move || match vault.load(&item) {
        Some(secret) if !secret.trim().is_empty() => Ok(secret),
        _ => {
            let secret = webhook::new_secret();
            vault.save(&item, &secret).map(|()| secret).map_err(|e| e.to_string())
        }
    })
    .await
    .map_err(|e| e.to_string())??;
    lock(&watches().secrets).insert(id, (key.to_string(), made.clone()));
    Ok(made)
}

/// The listener's address of a trigger.
pub fn address(port: u16, key: &str) -> String {
    format!("http://127.0.0.1:{port}/hooks/{}", webhook::path_id(key))
}

/// Why the listener is not listening, when it should be.
pub fn listener_error() -> Option<String> {
    lock(&watches().listener_error).clone()
}

pub fn listening() -> Option<u16> {
    lock(&watches().listener).as_ref().map(|(settings, _)| settings.port)
}

struct AppRouter {
    host: Weak<Host>,
}

impl webhook::Router for AppRouter {
    fn secret(&self, id: &str) -> Option<String> {
        let (key, secret) = lock(&watches().secrets).get(id).cloned()?;
        super::job(&key).filter(Job::runnable).map(|_| secret)
    }

    fn deliver(&self, delivery: webhook::Delivery) {
        let Some(host) = self.host.upgrade() else { return };
        let Some((key, _)) = lock(&watches().secrets).get(&delivery.id).cloned() else { return };
        let Some(job) = super::job(&key) else { return };
        let Item::Trigger(trigger) = &job.item else { return };
        let TriggerSource::Webhook { preset, map } = &trigger.on else { return };
        let event = webhook_event(job.name(), *preset, map, delivery);
        deliver(&host, std::slice::from_ref(&job), event, false, |_, _| {});
    }
}

/// A delivery to the webhook trigger `name` as its event. MC-66: a JSON body's values are the prompt's `{{data.…}}`; MC-97: the
/// sender's event name is `webhook.event`; MC-98: its preset and `map` read the type, subject, title, link, actor and more fields
/// from the body (`webhook.received`, about the delivery's id, when they give none).
fn webhook_event(name: &str, preset: WebhookPreset, map: &WebhookMap, delivery: webhook::Delivery) -> TriggerEvent {
    let detail: String = String::from_utf8_lossy(&delivery.body).chars().take(mighty_core::automation::watch::MAX_EVENT_CHARS).collect();
    let body = serde_json::from_slice::<serde_json::Value>(&delivery.body).unwrap_or(serde_json::Value::Null);
    let mut fields = mighty_core::automation::watch::data_fields(&body);
    let header = delivery.event.as_deref().map(str::trim).filter(|n| !n.is_empty()).map(|n| n.chars().take(200).collect::<String>());
    if let Some(header) = &header {
        fields.insert("webhook.event".into(), header.clone());
    }
    let mapped = mapping::apply(preset, map, &body, header.as_deref());
    fields.extend(mapped.fields);
    let title = mapped.title.or(header).unwrap_or_else(|| "webhook".into());
    let subject = mapped.subject.unwrap_or_else(|| delivery.delivery.clone());
    TriggerEvent { id: delivery.delivery, title, url: mapped.url, detail, fields, ..TriggerEvent::default() }.with_envelope(
        &format!("webhook:{name}"),
        &mapped.kind,
        &subject,
        mapped.actor.as_deref(),
    )
}

async fn sync_listener(host: &Arc<Host>, jobs: &[Job]) {
    let settings = super::listener_settings();
    let hooks: Vec<&Job> = jobs.iter().filter(|j| j.runnable() && matches!(&j.item, Item::Trigger(t) if t.on.is_webhook())).collect();
    for job in &hooks {
        if let Err(error) = secret_of(host, &job.key).await {
            host.log.line(&format!("automation: webhook secret for '{}' not kept: {error}", job.name()));
        }
    }
    let wanted = settings.enabled && !hooks.is_empty() && host.may_listen();
    let current = lock(&watches().listener).as_ref().map(|(s, _)| *s);
    if wanted && current == Some(settings) {
        return;
    }
    let serving = lock(&watches().listener).take();
    if let Some((_, task)) = serving {
        task.abort();
    }
    *lock(&watches().listener_error) = None;
    if !wanted {
        return;
    }
    // Bound inside the app's runtime, whatever thread calls.
    match webhook::bind(settings.port).await {
        Ok((listener, _)) => {
            let router: Arc<dyn webhook::Router> = Arc::new(AppRouter { host: Arc::downgrade(host) });
            let task = tauri::async_runtime::spawn(webhook::serve(listener, router, watches().deliveries.clone()));
            *lock(&watches().listener) = Some((settings, task));
            host.log.line(&format!("automation: webhook listener on 127.0.0.1:{}", settings.port));
        }
        Err(error) => {
            *lock(&watches().listener_error) = Some(error.to_string());
            host.log.line(&format!("automation: webhook listener not started: {error}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn delivery(event: Option<&str>, body: &str) -> webhook::Delivery {
        webhook::Delivery { id: "abc".into(), delivery: "d-1".into(), event: event.map(str::to_string), body: body.as_bytes().to_vec() }
    }

    /// MC-98: a webhook's event: the generic one as before, a preset's read from the body, the body's values as `data.*` either way.
    #[test]
    fn a_webhook_s_body_is_read_into_its_event() {
        let plain = webhook_event("deploy", WebhookPreset::Generic, &WebhookMap::default(), delivery(Some("deploy"), r#"{"env":"prod"}"#));
        assert_eq!((plain.kind.as_str(), plain.subject.as_str(), plain.title.as_str()), ("webhook.received", "d-1", "deploy"));
        assert_eq!(plain.source, "webhook:deploy");
        assert_eq!((plain.value("data.env"), plain.value("webhook.event")), (Some("prod"), Some("deploy")));
        let body = r#"{"event":"task.stage_changed","actor":"kim","task":{"key":"MC-12","title":"Fix","stage":"review",
            "previous_stage":"in_progress","labels":["bug"],"url":"https://w.dev/MC-12"}}"#;
        let task = webhook_event("board", WebhookPreset::Mightywork, &WebhookMap::default(), delivery(None, body));
        assert_eq!(
            (task.kind.as_str(), task.subject.as_str(), task.actor.as_deref()),
            ("mightywork.task.stage_changed", "MC-12", Some("kim"))
        );
        assert_eq!(
            (task.value("task.from"), task.value("task.to"), task.value("data.event")),
            (Some("in_progress"), Some("review"), Some("task.stage_changed"))
        );
        assert_eq!((task.title.as_str(), task.url.as_deref()), ("MC-12 Fix", Some("https://w.dev/MC-12")));
        let not_json = webhook_event("x", WebhookPreset::Github, &WebhookMap::default(), delivery(Some("ping"), "zen"));
        assert_eq!((not_json.kind.as_str(), not_json.subject.as_str()), ("github.ping", "d-1"), "the header alone");
    }
}
