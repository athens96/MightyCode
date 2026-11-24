//! MC-32's triggers while the app is open: a task per GitHub, Linear or standing-order trigger that is runnable (nothing runs for a
//! workspace without one), and the webhook listener while it is on and some webhook trigger listens.
//!
//! - GitHub and Linear are looked at every 5 minutes at most; the wait grows while nothing changes (to 30 minutes) or looks fail
//!   (offline, signed out: to an hour). A watch's first look only learns what is there; at most [`MAX_FIRES_A_LOOK`] new events fire
//!   per look, the rest are remembered as seen.
//! - A standing order's check runs in the workspace folder every `everyMinutes` (60 s limit); its agent wakes only when the output
//!   (with its exit code) changed, at most `dailyCap` times a day.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use chrono::Local;
use mighty_core::automation::spec::{GitHubEvent, TriggerSource};
use mighty_core::automation::watch::{Backoff, Observed, PollOutcome, TriggerEvent};

use super::{Item, Job, lock, store, webhook};
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
            TriggerSource::Webhook => return,
        };
        drop(host);
        tokio::time::sleep(wait).await;
    }
}

/// Fires `job` for an event, unless it was already seen.
fn fire_event(host: &Arc<Host>, job: &Job, event: TriggerEvent) {
    let (host, job) = (host.clone(), job.clone());
    tauri::async_runtime::spawn(async move {
        let reason = event.title.clone();
        super::fire::fire(&host, super::fire::FireRequest { job, reason, scheduled_for: None, event: Some(event), retry_of: None }).await;
    });
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
            let event = TriggerEvent { id: String::new(), title: host.locale.get("automation.reason.changed"), url: None, detail: tail };
            fire_event(host, job, event);
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
        let found = super::github::events_of(*event, &answer.body);
        let fresh = super::with_state(|state| {
            state.etags.entry(key.clone()).or_default().set(&endpoint, answer.etag());
            state.seen.entry(key.clone()).or_default().new_of(found.iter().map(|e| e.id.as_str()))
        });
        super::save_state(host);
        // Oldest first, a few a look.
        for event in found.into_iter().rev().filter(|e| fresh.contains(&e.id)).take(MAX_FIRES_A_LOOK) {
            changed = true;
            fire_event(host, job, event);
        }
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
    let mut changed = false;
    for key in entered.iter().take(MAX_FIRES_A_LOOK) {
        let Some(issue) = issues.iter().find(|i| &i.key == key) else { continue };
        changed = true;
        let event = TriggerEvent {
            id: key.clone(),
            title: format!("{}: {} ({})", issue.key, issue.title, issue.state.clone().unwrap_or_default()),
            url: issue.url.clone(),
            detail: String::new(),
        };
        fire_event(host, job, event);
    }
    if changed { PollOutcome::Changed } else { PollOutcome::Unchanged }
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
        let detail: String =
            String::from_utf8_lossy(&delivery.body).chars().take(mighty_core::automation::watch::MAX_EVENT_CHARS).collect();
        let title = delivery.event.clone().unwrap_or_else(|| "webhook".into());
        fire_event(&host, &job, TriggerEvent { id: delivery.delivery, title, url: None, detail });
    }
}

async fn sync_listener(host: &Arc<Host>, jobs: &[Job]) {
    let settings = super::listener_settings();
    let hooks: Vec<&Job> =
        jobs.iter().filter(|j| j.runnable() && matches!(&j.item, Item::Trigger(t) if t.on == TriggerSource::Webhook)).collect();
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
