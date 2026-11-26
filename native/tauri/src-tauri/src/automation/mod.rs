//! MC-32: schedules and event triggers that start agent work on their own, while the app is open.
//!
//! - **Jobs**: each local workspace's schedules and triggers from the app's settings (`store`, always trusted: the person typed
//!   them) and from `.mighty.yaml` / `.mighty.local.yaml` (`schedules:`, `triggers:`; they run only once that configuration is
//!   approved, the same trust as its other keys). They are read at the start, after a change in settings or an approval, and again
//!   at most every [`RELOAD`] when the scheduler wakes.
//! - **The scheduler** ([`scheduler`]): one task that sleeps until the next fire (`mighty_core::automation::plan`), woken early by a
//!   change or the computer's resume, never a polling loop. At the start it finds the fires a closed app missed and applies each
//!   schedule's `catchUp` (default `once`: the most recent missed fire runs once). While the app is closed nothing fires (the
//!   background daemon, MC-3, keeps runs going but cannot start one), unless 앱이 꺼져 있어도 예약 실행 is on ([`wake`], MC-39): then
//!   the OS starts the app without a window at the next fire, and it quits once idle; otherwise a missed fire runs (or is skipped)
//!   at the next start.
//! - **Triggers** ([`triggers`]): GitHub (`gh api` with ETags) and Linear (the MC-31 tracker) looked at every 5 minutes at most, only
//!   for workspaces that have one, backing off while idle or offline; standing orders (a shell check every few minutes that wakes the
//!   agent only when its output changes, at most its daily cap); the signed webhook listener on 127.0.0.1 ([`webhook`]), its body
//!   read by a preset and a map (MC-98); and MC-98's MCP polling, a tool of one of the person's MCP servers called every few minutes
//!   (mcp_client.rs) and its items compared with the last look's.
//! - **A fire** ([`fire`]): the daily budget checked first (skipped with a record when the run would pass it), then a fresh pane (in
//!   its own worktree when asked) or the named pane of a heartbeat, and the prompt sent through the composer's own path. Its record
//!   follows the pane's events on the MC-9 bus: its state, its cost from the ledger, a pull request link it printed. A pane that
//!   waits for an approval or an answer is told to the phone and the OS at once; the rest goes into the day's digest. Every fire,
//!   skip, wait and end is an `automation.*` event on the bus.
//! - MC-99: a trigger's action may `develop` (a worktree of its own and a pull request), `run` one command or skill, or `emit`
//!   another event; its `policy` ([`policy`]) asks the person first, acts once per subject, keeps to its concurrency, cooldown and
//!   daily cap, and says in the event inbox why it skipped or waits.

pub mod chain;
pub mod fire;
pub mod github;
pub mod policy;
pub mod store;
pub mod triggers;
pub mod wake;
pub mod webhook;

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, Weak};
use std::time::{Duration, Instant};

use chrono::{DateTime, Local, Utc};
use mighty_core::automation::plan::{self, Due, Slot};
use mighty_core::automation::records::{History, RunRecord};
use mighty_core::automation::recurrence::{LocalZone, Recurrence};
use mighty_core::automation::spec::{Action, ScheduleSpec, TriggerSource, TriggerSpec};
use mighty_core::control::events::{AutomationNote, EventBody, PaneRef};
use mighty_core::locale::Locale;
use mighty_core::model::{PaneBot, Workspace};

use crate::host::Host;

/// How often the jobs are read again at most, when the scheduler wakes (a `.mighty.yaml` edited outside the app).
pub const RELOAD: Duration = Duration::from_secs(30 * 60);

/// Where a job is defined.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    App,
    Yaml,
}

impl Origin {
    pub fn name(self) -> &'static str {
        match self {
            Origin::App => "app",
            Origin::Yaml => "yaml",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    Schedule(ScheduleSpec),
    Trigger(TriggerSpec),
}

/// A schedule or trigger of one workspace.
#[derive(Debug, Clone, PartialEq)]
pub struct Job {
    pub key: String,
    pub workspace: String,
    pub workspace_name: String,
    pub workspace_path: String,
    pub origin: Origin,
    /// Always for the app's own; a `.mighty.yaml` job once its configuration is approved.
    pub approved: bool,
    pub item: Item,
    /// Why it cannot run (a bad cron line, an unknown agent).
    pub error: Option<String>,
}

impl Job {
    pub fn key_of(workspace: &str, origin: Origin, trigger: bool, name: &str) -> String {
        format!("{workspace}:{}:{}:{name}", origin.name(), if trigger { "t" } else { "s" })
    }

    pub fn name(&self) -> &str {
        match &self.item {
            Item::Schedule(s) => &s.name,
            Item::Trigger(t) => &t.name,
        }
    }

    pub fn action(&self) -> &Action {
        match &self.item {
            Item::Schedule(s) => &s.action,
            Item::Trigger(t) => &t.action,
        }
    }

    pub fn enabled(&self) -> bool {
        match &self.item {
            Item::Schedule(s) => s.enabled,
            Item::Trigger(t) => t.enabled,
        }
    }

    /// `schedule`, or the trigger's kind.
    pub fn kind(&self) -> &'static str {
        match &self.item {
            Item::Schedule(_) => "schedule",
            Item::Trigger(t) => t.on.kind(),
        }
    }

    pub fn runnable(&self) -> bool {
        self.approved && self.error.is_none() && self.enabled()
    }

    /// What the history keeps of the job.
    pub fn reference(&self) -> mighty_core::automation::engine::JobRef {
        mighty_core::automation::engine::JobRef {
            key: self.key.clone(),
            workspace: self.workspace.clone(),
            workspace_name: self.workspace_name.clone(),
            name: self.name().to_string(),
            origin: self.origin.name().to_string(),
            kind: self.kind().to_string(),
        }
    }

    pub fn recurrence(&self) -> Option<Recurrence> {
        match &self.item {
            Item::Schedule(s) => Recurrence::parse(s.cron.as_deref(), s.rrule.as_deref()).ok(),
            Item::Trigger(_) => None,
        }
    }
}

/// A workspace's `.mighty.yaml` as the jobs list shows it: its hash and whether it is approved, or why it could not be read.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct YamlStatus {
    pub hash: String,
    pub approved: bool,
    pub error: Option<String>,
    pub has_jobs: bool,
}

#[derive(Default)]
struct Defs {
    jobs: Vec<Job>,
    yaml: HashMap<String, YamlStatus>,
    loaded: Option<Instant>,
}

#[derive(Default)]
struct Automation {
    started: AtomicBool,
    defs: Mutex<Defs>,
    settings: Mutex<store::Settings>,
    state: Mutex<store::State>,
    history: Mutex<History>,
    wake: tokio::sync::Notify,
    /// Records whose prompt was queued behind a running request: their pane's next run is theirs.
    awaiting_start: Mutex<HashSet<String>>,
    /// What the last action of the jobs box said.
    message: Mutex<Option<String>>,
    resume: Mutex<Option<mighty_platform::net::Subscription>>,
}

fn automation() -> &'static Automation {
    static AUTOMATION: OnceLock<Automation> = OnceLock::new();
    AUTOMATION.get_or_init(Automation::default)
}

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|p| p.into_inner())
}

fn redraw(host: &Host) {
    crate::native::touch(host, crate::settings::SURFACE);
}

/// The record state's word (`완료`, `실패`, `건너뜀`, …).
pub fn state_word(locale: &Locale, state: &str) -> String {
    locale.get(match state {
        "running" => "automation.state.running",
        "blocked" => "automation.state.blocked",
        "done" => "automation.state.done",
        "failed" => "automation.state.failed",
        "stopped" => "automation.state.stopped",
        _ => "automation.state.skipped",
    })
}

// ── start and stop ───────────────────────────────────────────────────────────────────────────────────────────────────────

/// Reads the files, then starts the scheduler, the watches and (when on) the listener; not in a smoke.
pub fn start(host: &Arc<Host>, platform: &mighty_platform::Platform) {
    if host.smoke.is_some() || automation().started.swap(true, Ordering::SeqCst) {
        return;
    }
    *lock(&automation().settings) = store::load_settings(&host.data_dir);
    *lock(&automation().state) = store::load_state(&host.data_dir);
    *lock(&automation().history) = History::load(&store::history_path(&host.data_dir));
    // MC-39's record read here, so Settings draws from memory.
    let _ = wake::record(host);
    // The computer woke: fires due while it slept are looked for now.
    let subscription = platform.network.on_resume(mighty_platform::net::NetworkCallback::new(|| automation().wake.notify_one()));
    if let Ok(subscription) = subscription {
        *lock(&automation().resume) = Some(subscription);
    }
    follow_bus(host);
    let weak = Arc::downgrade(host);
    tauri::async_runtime::spawn(scheduler(weak));
    // MC-39: a server the OS started for schedules quits once idle.
    wake::watch_idle(host);
}

/// The app quits: the watches and the listener stop (the files are already saved).
pub fn shutdown(host: &Host) {
    triggers::stop_all();
    lock(&automation().resume).take();
    // MC-39: the OS wake-up left right for the next fire (only when on, or when one is still registered).
    if automation().started.load(Ordering::SeqCst) {
        wake::at_quit(host);
    }
}

// ── the jobs ─────────────────────────────────────────────────────────────────────────────────────────────────────────────

fn validated_schedule(spec: &ScheduleSpec) -> Option<String> {
    spec.validate(&mighty_core::wire::PROVIDERS).err()
}

fn validated_trigger(spec: &TriggerSpec) -> Option<String> {
    spec.validate(&mighty_core::wire::PROVIDERS).err()
}

/// Every local workspace's jobs read again; the watches and the listener follow, and the scheduler re-plans.
pub async fn reload(host: &Arc<Host>) {
    let workspaces: Vec<Workspace> = {
        let view = host.view();
        view.snapshot.workspaces().iter().filter(|w| w.remote.is_none() && !w.path().is_empty()).cloned().collect()
    };
    let settings = lock(&automation().settings).clone();
    let mut jobs = vec![];
    let mut yaml = HashMap::new();
    for workspace in &workspaces {
        let job = |origin: Origin, approved: bool, item: Item, error: Option<String>| {
            let trigger = matches!(item, Item::Trigger(_));
            let name = match &item {
                Item::Schedule(s) => s.name.clone(),
                Item::Trigger(t) => t.name.clone(),
            };
            Job {
                key: Job::key_of(workspace.id(), origin, trigger, &name),
                workspace: workspace.id().to_string(),
                workspace_name: workspace.name().to_string(),
                workspace_path: workspace.path().to_string(),
                origin,
                approved,
                item,
                error,
            }
        };
        if let Some(own) = settings.workspaces.get(workspace.id()) {
            for spec in &own.schedules {
                jobs.push(job(Origin::App, true, Item::Schedule(spec.clone()), validated_schedule(spec)));
            }
            for spec in &own.triggers {
                jobs.push(job(Origin::App, true, Item::Trigger(spec.clone()), validated_trigger(spec)));
            }
        }
        match crate::repo_tasks::load_workspace_config(host, workspace).await {
            Ok(Some(status)) => {
                let config = &status.loaded.config;
                let has_jobs = !config.schedules.is_empty() || !config.triggers.is_empty();
                for spec in config.schedules.values() {
                    jobs.push(job(Origin::Yaml, status.approved, Item::Schedule(spec.clone()), None));
                }
                for spec in config.triggers.values() {
                    jobs.push(job(Origin::Yaml, status.approved, Item::Trigger(spec.clone()), None));
                }
                if has_jobs {
                    let entry = YamlStatus { hash: status.loaded.content_hash.clone(), approved: status.approved, error: None, has_jobs };
                    yaml.insert(workspace.id().to_string(), entry);
                }
            }
            Ok(None) => {}
            Err(error) => {
                yaml.insert(workspace.id().to_string(), YamlStatus { error: Some(error), ..YamlStatus::default() });
            }
        }
    }
    {
        let mut defs = lock(&automation().defs);
        *defs = Defs { jobs, yaml, loaded: Some(Instant::now()) };
    }
    triggers::sync(host).await;
    // MC-99: what waited for a run (or across a restart) may start now.
    policy::drain_all(host);
    automation().wake.notify_one();
    redraw(host);
}

/// The jobs box asks: read again on a task when the last read is over two minutes old (a workspace added since, a file edited).
pub fn refresh_if_stale(host: &Arc<Host>) {
    let stale = {
        let mut defs = lock(&automation().defs);
        let stale = defs.loaded.is_some_and(|at| at.elapsed() > Duration::from_secs(120));
        if stale {
            defs.loaded = Some(Instant::now());
        }
        stale
    };
    if stale {
        let host = host.clone();
        tauri::async_runtime::spawn(async move { reload(&host).await });
    }
}

/// The jobs as last read.
pub fn jobs() -> Vec<Job> {
    lock(&automation().defs).jobs.clone()
}

pub fn job(key: &str) -> Option<Job> {
    lock(&automation().defs).jobs.iter().find(|j| j.key == key).cloned()
}

pub fn yaml_status(workspace: &str) -> Option<YamlStatus> {
    lock(&automation().defs).yaml.get(workspace).cloned()
}

pub fn listener_settings() -> store::Listener {
    lock(&automation().settings).listener
}

fn save_settings(host: &Arc<Host>) {
    if !host.writes() {
        return;
    }
    let settings = lock(&automation().settings).clone();
    let dir = host.data_dir.clone();
    tauri::async_runtime::spawn_blocking(move || store::save_settings(&dir, &settings));
}

/// Settings changed: saved, and the jobs read again.
pub(crate) fn settings_changed(host: &Arc<Host>) {
    save_settings(host);
    let host = host.clone();
    tauri::async_runtime::spawn(async move { reload(&host).await });
}

/// Adds or replaces (by `replacing`, else by name) one of the app's own schedules.
pub fn put_schedule(host: &Arc<Host>, workspace: &str, replacing: Option<&str>, spec: ScheduleSpec) -> Result<(), String> {
    if let Some(error) = validated_schedule(&spec) {
        return Err(error);
    }
    {
        let mut settings = lock(&automation().settings);
        let own = settings.workspaces.entry(workspace.to_string()).or_default();
        let old = replacing.unwrap_or(&spec.name).to_string();
        if old != spec.name && own.schedules.iter().any(|s| s.name == spec.name) {
            return Err(host.locale.get("automation.form.duplicate"));
        }
        let full = own.schedules.len() >= mighty_core::automation::spec::MAX_SCHEDULES;
        match own.schedules.iter_mut().find(|s| s.name == old) {
            Some(existing) => *existing = spec,
            None if full => return Err(host.locale.get("automation.form.full")),
            None => own.schedules.push(spec),
        }
    }
    settings_changed(host);
    Ok(())
}

/// Adds or replaces one of the app's own triggers.
pub fn put_trigger(host: &Arc<Host>, workspace: &str, replacing: Option<&str>, spec: TriggerSpec) -> Result<(), String> {
    if let Some(error) = validated_trigger(&spec) {
        return Err(error);
    }
    {
        let mut settings = lock(&automation().settings);
        let own = settings.workspaces.entry(workspace.to_string()).or_default();
        let old = replacing.unwrap_or(&spec.name).to_string();
        if old != spec.name && own.triggers.iter().any(|t| t.name == spec.name) {
            return Err(host.locale.get("automation.form.duplicate"));
        }
        let full = own.triggers.len() >= mighty_core::automation::spec::MAX_TRIGGERS;
        match own.triggers.iter_mut().find(|t| t.name == old) {
            Some(existing) => *existing = spec,
            None if full => return Err(host.locale.get("automation.form.full")),
            None => own.triggers.push(spec),
        }
    }
    settings_changed(host);
    Ok(())
}

/// Removes one of the app's own jobs.
pub fn remove_own(host: &Arc<Host>, workspace: &str, trigger: bool, name: &str) {
    {
        let mut settings = lock(&automation().settings);
        if let Some(own) = settings.workspaces.get_mut(workspace) {
            if trigger {
                own.triggers.retain(|t| t.name != name);
            } else {
                own.schedules.retain(|s| s.name != name);
            }
        }
    }
    settings_changed(host);
}

/// Turns one of the app's own jobs on or off.
pub fn set_enabled(host: &Arc<Host>, workspace: &str, trigger: bool, name: &str, on: bool) {
    {
        let mut settings = lock(&automation().settings);
        if let Some(own) = settings.workspaces.get_mut(workspace) {
            if trigger {
                own.triggers.iter_mut().filter(|t| t.name == name).for_each(|t| t.enabled = on);
            } else {
                own.schedules.iter_mut().filter(|s| s.name == name).for_each(|s| s.enabled = on);
            }
        }
    }
    settings_changed(host);
}

/// The listener's switch and port.
pub fn set_listener(host: &Arc<Host>, listener: store::Listener) {
    lock(&automation().settings).listener = listener;
    settings_changed(host);
}

/// The jobs box's 승인: the workspace's `.mighty.yaml` approved at the hash shown.
pub async fn approve(host: &Arc<Host>, workspace: &str) -> Result<(), String> {
    let (found, hash) = {
        let view = host.view();
        (view.snapshot.workspace(workspace).cloned(), yaml_status(workspace).map(|s| s.hash))
    };
    let (Some(found), Some(hash)) = (found, hash) else { return Ok(()) };
    crate::repo_tasks::approve_workspace_config(host, &found, &hash).await?;
    reload(host).await;
    Ok(())
}

pub fn message() -> Option<String> {
    lock(&automation().message).clone()
}

pub fn say(host: &Host, message: Option<String>) {
    *lock(&automation().message) = message;
    redraw(host);
}

// ── the scheduler ────────────────────────────────────────────────────────────────────────────────────────────────────────

/// The schedules as the planner sees them: runnable ones on, the rest off (their state starts over when they come back).
fn slots(jobs: &[Job]) -> Vec<Slot> {
    jobs.iter()
        .filter_map(|job| {
            let Item::Schedule(spec) = &job.item else { return None };
            let recurrence = job.recurrence()?;
            Some(Slot { key: job.key.clone(), recurrence, catch_up: spec.catch_up, enabled: job.runnable() })
        })
        .collect()
}

/// The earliest next fire across all schedules (MC-39's OS wake-up).
pub fn next_wake() -> Option<DateTime<Utc>> {
    let jobs = jobs();
    let state = lock(&automation().state);
    plan::next_wake(&slots(&jobs), &state.slots, Utc::now(), &LocalZone)
}

/// Each schedule's next fire, by key.
pub fn next_fires() -> HashMap<String, DateTime<Utc>> {
    let jobs = jobs();
    let state = lock(&automation().state);
    plan::next_fires(&slots(&jobs), &state.slots, Utc::now(), &LocalZone).into_iter().collect()
}

pub(crate) fn save_state(host: &Host) {
    if !host.writes() {
        return;
    }
    let state = lock(&automation().state).clone();
    let dir = host.data_dir.clone();
    tauri::async_runtime::spawn_blocking(move || store::save_state(&dir, &state));
}

pub(crate) fn with_state<T>(change: impl FnOnce(&mut store::State) -> T) -> T {
    change(&mut lock(&automation().state))
}

/// The one timer: reads the jobs (first, and when stale), runs what is due, sleeps until the next fire or a change.
async fn scheduler(weak: Weak<Host>) {
    loop {
        let Some(host) = weak.upgrade() else { return };
        if host.closing() {
            return;
        }
        let stale = lock(&automation().defs).loaded.is_none_or(|at| at.elapsed() >= RELOAD);
        if stale {
            reload(&host).await;
        }
        let now = Utc::now();
        let jobs = jobs();
        let slots = slots(&jobs);
        let due = with_state(|state| plan::due(&slots, &mut state.slots, now, &LocalZone));
        if !due.is_empty() {
            save_state(&host);
        }
        for found in due {
            let Some(job) = jobs.iter().find(|j| j.key == found.key()).cloned() else { continue };
            let l = host.locale;
            match found {
                Due::Fire { at, late, .. } => {
                    let reason = l.get(if late { "automation.reason.catchUp" } else { "automation.reason.schedule" });
                    let host = host.clone();
                    // MC-39: a server the OS started waits for the fire to make its pane and send its prompt.
                    let firing = wake::Firing::begin();
                    tauri::async_runtime::spawn(async move {
                        fire::fire(
                            &host,
                            fire::FireRequest { job, reason, scheduled_for: Some(at), event: None, retry_of: None, asked: false },
                        )
                        .await;
                        drop(firing);
                    });
                }
                Due::Skip { at, .. } => fire::skip(&host, &job, l.get("automation.reason.missed"), Some(at)),
            }
        }
        wake::first_pass_done();
        let next = with_state(|state| plan::next_wake(&slots, &state.slots, Utc::now(), &LocalZone));
        // MC-39: the OS wake-up follows the next fire (registered, moved after a fire or a change, or removed).
        wake::sync_later(&host, next, wake::Urgency::Normal);
        let until_next = next.and_then(|at| (at - Utc::now()).to_std().ok()).unwrap_or(RELOAD);
        drop(host);
        // Woken at the next fire, at the reload's time at the latest, or early by a change or the computer's resume.
        tokio::select! {
            () = tokio::time::sleep(until_next.min(RELOAD) + Duration::from_millis(50)) => {}
            () = automation().wake.notified() => {}
        }
    }
}

// ── the records ──────────────────────────────────────────────────────────────────────────────────────────────────────────

pub(crate) fn with_history<T>(host: &Host, change: impl FnOnce(&mut History) -> T) -> T {
    let (out, snapshot) = {
        let mut history = lock(&automation().history);
        let out = change(&mut history);
        (out, history.clone())
    };
    if host.writes() {
        let path = store::history_path(&host.data_dir);
        tauri::async_runtime::spawn_blocking(move || snapshot.save(&path));
    }
    redraw(host);
    out
}

/// The newest records (newest first), of one workspace or all.
pub fn recent(workspace: Option<&str>, n: usize) -> Vec<RunRecord> {
    lock(&automation().history).recent(workspace, n).into_iter().cloned().collect()
}

pub fn last_of(job: &Job) -> Option<RunRecord> {
    lock(&automation().history).last_of(&job.workspace, job.origin.name(), job.name()).cloned()
}

/// Today's digest (since local midnight).
pub fn digest_today() -> mighty_core::automation::records::Digest {
    let midnight = Local::now().date_naive().and_hms_opt(0, 0, 0).and_then(|t| t.and_local_timezone(Local).earliest());
    let since = midnight.map(|t| t.with_timezone(&Utc).to_rfc3339_opts(chrono::SecondsFormat::Secs, true)).unwrap_or_default();
    lock(&automation().history).digest(&since)
}

/// 다시 실행: the record's job fired again (a trigger's with the event it had).
pub async fn retry(host: &Arc<Host>, record_id: &str) -> Result<(), String> {
    let l = host.locale;
    let runnable: Vec<Job> = jobs().into_iter().filter(Job::runnable).collect();
    let references: Vec<mighty_core::automation::engine::JobRef> = runnable.iter().map(Job::reference).collect();
    let found = {
        let history = lock(&automation().history);
        mighty_core::automation::engine::retry_target(&history, record_id, &references).map(|(job, record)| (job.key.clone(), record))
    };
    let (key, record) = found.map_err(|_| l.get("automation.retry.gone"))?;
    let job = runnable.into_iter().find(|j| j.key == key).ok_or_else(|| l.get("automation.retry.gone"))?;
    let event = (record.kind != "schedule").then(|| mighty_core::automation::watch::TriggerEvent {
        title: record.reason.clone(),
        url: record.event_url.clone(),
        ..Default::default()
    });
    let request = fire::FireRequest {
        job,
        reason: l.get("automation.reason.retry"),
        scheduled_for: None,
        event,
        retry_of: Some(record.id.clone()),
        asked: true,
    };
    fire::fire(host, request).await;
    Ok(())
}

/// 지금 실행: a job fired now, out of its schedule (a trigger without an event).
pub async fn run_now(host: &Arc<Host>, key: &str) -> Result<(), String> {
    let job = job(key).filter(Job::runnable).ok_or_else(|| host.locale.get("automation.retry.gone"))?;
    let request = fire::FireRequest {
        job,
        reason: host.locale.get("automation.reason.manual"),
        scheduled_for: None,
        event: None,
        retry_of: None,
        asked: true,
    };
    fire::fire(host, request).await;
    Ok(())
}

/// 이어서 하기: Settings closes and the record's pane comes forward.
pub fn open(host: &Arc<Host>, record_id: &str) {
    let pane = lock(&automation().history).get(record_id).and_then(|r| r.pane.clone());
    if let Some(pane) = pane
        && host.view().snapshot.session(&pane).is_some()
    {
        crate::settings::close(host);
        crate::notifications::focus(host, &pane);
    }
}

/// MC-73: the bot a run record's job sends to (`@handle`), by its workspace and name.
pub fn bot_of_record(record: &RunRecord) -> Option<String> {
    jobs().into_iter().find(|j| j.workspace == record.workspace && j.name() == record.name).and_then(|j| j.action().bot())
}

/// A record's event on the bus (`automation.fired`, `.skipped`, `.blocked`, `.finished`).
pub(crate) fn publish(host: &Host, event: &str, record: &RunRecord) {
    let reference = {
        let view = host.view();
        match record.pane.as_deref().and_then(|p| view.snapshot.session(p)) {
            Some(session) => crate::control::pane_ref(&view.snapshot, session),
            None => PaneRef {
                workspace: record.workspace.clone(),
                workspace_name: record.workspace_name.clone(),
                kind: "automation".into(),
                ..PaneRef::default()
            },
        }
    };
    crate::control::bus::global().publish(
        reference,
        EventBody::Automation(AutomationNote {
            event: event.to_string(),
            name: record.name.clone(),
            kind: record.kind.clone(),
            record: record.id.clone(),
            state: record.state.name().to_string(),
            cost_usd: record.cost_usd,
            url: record.url.clone(),
        }),
    );
}

/// The records follow their panes on the MC-9 bus: a card waiting, a run's end with its cost.
fn follow_bus(host: &Arc<Host>) {
    let weak = Arc::downgrade(host);
    let mut rx = crate::control::bus::global().subscribe();
    tauri::async_runtime::spawn(async move {
        loop {
            let event = match rx.recv().await {
                Ok(crate::control::bus::BusEvent::Control(event)) => event,
                Ok(crate::control::bus::BusEvent::Output { .. }) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            };
            let Some(host) = weak.upgrade() else { return };
            if host.closing() {
                return;
            }
            fire::on_event(&host, &event);
            // MC-66: a bot's request ending wakes the chains listening to it.
            chain::on_event(&host, &event);
        }
    });
}

pub(crate) fn awaiting_start() -> MutexGuard<'static, HashSet<String>> {
    lock(&automation().awaiting_start)
}

/// MC-99: the locale key of an action kind's word (묻기, 개발, 실행, 이벤트 보내기).
pub fn do_key(kind: mighty_core::automation::spec::ActionKind) -> &'static str {
    use mighty_core::automation::spec::ActionKind;
    match kind {
        ActionKind::Ask => "automation.do.ask",
        ActionKind::Develop => "automation.do.develop",
        ActionKind::Run => "automation.do.run",
        ActionKind::Emit => "automation.do.emit",
    }
}

/// MC-99: the locale key of the settings form's line on what an action kind does.
pub fn do_note_key(kind: mighty_core::automation::spec::ActionKind) -> &'static str {
    use mighty_core::automation::spec::ActionKind;
    match kind {
        ActionKind::Ask => "automation.form.doNote.ask",
        ActionKind::Develop => "automation.form.doNote.develop",
        ActionKind::Run => "automation.form.doNote.run",
        ActionKind::Emit => "automation.form.doNote.emit",
    }
}

/// A source's kind word (`예약`, `GitHub`, …).
pub fn kind_word(locale: &Locale, kind: &str) -> String {
    match kind {
        "schedule" => locale.get("automation.kind.schedule"),
        "github" => "GitHub".into(),
        "linear" => "Linear".into(),
        "webhook" => locale.get("automation.kind.webhook"),
        "event" => locale.get("automation.kind.event"),
        "bot" => locale.get("automation.kind.bot"),
        "mcp" => locale.get("automation.kind.mcp"),
        _ => locale.get("automation.kind.standing"),
    }
}

// ── MC-64: a bot's own jobs ──────────────────────────────────────────────────────────────────────────────────────────

/// The jobs sent to the bot `handle` (heartbeats to `@handle`, of any workspace), in the list's order.
pub fn jobs_of_bot(handle: &str) -> Vec<Job> {
    let key = PaneBot::key(handle);
    jobs().into_iter().filter(|j| j.action().bot().is_some_and(|b| PaneBot::key(&b) == key)).collect()
}

/// MC-108: what an independent bot watches through its jobs: the workspace each lives in (not the bots' home) and the GitHub
/// repository a trigger names.
pub fn note_watches(snapshot: &mighty_core::model::AppSnapshot, bot: &mut mighty_core::shell::SidebarBot, jobs: &[Job]) {
    for job in jobs {
        if snapshot.workspace(&job.workspace).is_some_and(|w| !w.is_bot_home()) {
            bot.watch_workspace(&job.workspace, &job.workspace_name);
        }
    }
    for job in jobs {
        if let Item::Trigger(trigger) = &job.item
            && let TriggerSource::Github { repo: Some(repo), .. } = &trigger.on
        {
            bot.watch_repo(repo);
        }
    }
}

/// The soonest next fire of the bot's runnable schedules.
pub fn next_fire_of_bot(handle: &str) -> Option<DateTime<Utc>> {
    let next = next_fires();
    jobs_of_bot(handle).iter().filter(|j| j.runnable()).filter_map(|j| next.get(&j.key).copied()).min()
}

/// The bot's name changed: the app's own jobs sent to it follow (a `.mighty.yaml` job keeps what its file says).
pub fn rename_bot(host: &Arc<Host>, old: &str, new: &str) {
    let (old_key, to) = (PaneBot::key(old), format!("@{new}"));
    let changed = {
        let mut settings = lock(&automation().settings);
        let mut changed = false;
        for jobs in settings.workspaces.values_mut() {
            let actions = jobs.schedules.iter_mut().map(|s| &mut s.action).chain(jobs.triggers.iter_mut().map(|t| &mut t.action));
            for action in actions {
                if action.bot().is_some_and(|b| PaneBot::key(&b) == old_key) {
                    action.pane = Some(to.clone());
                    changed = true;
                }
            }
        }
        changed
    };
    if changed {
        settings_changed(host);
    }
}

/// What the bot `handle` spent today (local day).
pub fn bot_spent_today(handle: &str) -> f64 {
    let day = Local::now().date_naive().format("%Y-%m-%d").to_string();
    with_state(|state| state.bot_spend.get(&PaneBot::key(handle)).map(|s| s.on(&day)).unwrap_or(0.0))
}

/// A bot's finished run: its cost on `day` added to the bot's count.
pub fn add_bot_spend(host: &Host, handle: &str, day: &str, usd: f64) {
    with_state(|state| state.bot_spend.entry(PaneBot::key(handle)).or_default().add(day, usd));
    save_state(host);
}

/// Why the bot `handle` may not take work now: asleep, or its own daily budget spent. The words saying so.
pub fn bot_unavailable(host: &Host, handle: &str) -> Option<String> {
    bot_unavailable_for(host, handle, true)
}

/// Whether the bot `handle` is asleep (MC-84: its scheduled fires skip without a report).
pub fn bot_asleep(host: &Host, handle: &str) -> bool {
    host.view().snapshot.bot_pane(handle).and_then(|p| p.bot.clone()).is_some_and(|bot| bot.is_asleep())
}

/// [`bot_unavailable`], sleep counted only when `asleep_counts`: a person's own 다시 실행 or 지금 실행 reaches a sleeping bot, as
/// what they type into its pane does (MC-84); its daily budget still holds.
pub fn bot_unavailable_for(host: &Host, handle: &str, asleep_counts: bool) -> Option<String> {
    let bot = host.view().snapshot.bot_pane(handle).and_then(|p| p.bot.clone())?;
    if asleep_counts && bot.is_asleep() {
        return Some(host.locale.get_with("bot.asleep.refused", &[("handle", &bot.handle)]));
    }
    let budget = bot.budget_per_day?;
    let spent = bot_spent_today(&bot.handle);
    (spent >= budget).then(|| {
        host.locale.get_with(
            "bot.budget.spent",
            &[("handle", &bot.handle), ("spent", &format!("${spent:.2}")), ("budget", &format!("${budget:.2}"))],
        )
    })
}

/// MC-97: what a trigger listens to and which of its events it takes (its `where`), in a few words.
/// MC-99: and what it does when it is not `ask`, and its policy.
pub fn trigger_spec_words(locale: &Locale, trigger: &TriggerSpec) -> String {
    let mut words = vec![trigger_words(locale, &trigger.on)];
    if !trigger.conditions.is_empty() {
        words.push(mighty_core::automation::condition::words(&trigger.conditions));
    }
    if !trigger.action.kind.is_ask() {
        words.push(trigger.action.words());
    }
    let policy = trigger.policy.words(&trigger.action);
    if !policy.is_empty() {
        words.push(policy.join(", "));
    }
    words.join(" · ")
}

/// What a trigger listens to, in a few words.
fn trigger_words(locale: &Locale, source: &TriggerSource) -> String {
    match source {
        TriggerSource::Github { events, repo } => {
            let mut words = events.iter().map(|e| e.name()).collect::<Vec<_>>().join(", ");
            if let Some(repo) = repo {
                words.push_str(&format!(" · {repo}"));
            }
            words
        }
        TriggerSource::Event { name } => name.clone(),
        TriggerSource::Bot { bot, on } => format!("{bot} · {}", on.iter().map(|o| o.name()).collect::<Vec<_>>().join(", ")),
        TriggerSource::Linear { states } => states.join(", "),
        // MC-98: its preset when not the generic one, and the type its own map makes.
        TriggerSource::Webhook { preset, map } => {
            let mut words = vec![locale.get("automation.kind.webhook")];
            if !preset.is_generic() {
                words.push(preset.name().to_string());
            }
            words.extend(map.kind.clone());
            words.join(" · ")
        }
        TriggerSource::Mcp { server, tool, every_minutes, .. } => {
            locale.get_with("automation.mcp.summary", &[("server", server), ("tool", tool), ("minutes", &every_minutes.to_string())])
        }
        TriggerSource::Standing { command, every_minutes, daily_cap } => {
            let short: String = command.chars().take(40).collect();
            locale.get_with(
                "automation.standing.summary",
                &[("command", &short), ("minutes", &every_minutes.to_string()), ("cap", &daily_cap.to_string())],
            )
        }
    }
}
