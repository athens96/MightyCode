//! MC-99: a trigger's `policy` while the app runs (the rules are `mighty_core::automation::policy`).
//!
//! Every event that passed a trigger's `where` comes here from [`super::triggers::deliver`] ([`admit`]): the policy decides, with
//! the trigger's [`PolicyState`] (`state.json`'s `policy`) and its runs going now, whether the action starts, waits for the
//! person's approval, waits for one of its runs to end, or is skipped. Each outcome is noted beside the event in the event inbox
//! with its reason (already handled, cooling down until, daily cap, waits for a run, awaiting approval).
//!
//! - An approval waits in `<data>/automation/pending.json` (it survives a restart) and shows in the 알림 함's 이벤트 tab with the
//!   event, the rule and what will run, and 실행 / 건너뛰기 ([`decide`]); the OS and the phone are told once. One left for
//!   [`APPROVAL_DAYS`] days is noted as expired and dropped.
//! - The events waiting for a run (`concurrency`) are kept there too, [`mighty_core::automation::policy::MAX_WAITING`] a trigger;
//!   when one of its runs ends ([`released`], from fire.rs) or the jobs are read again, the next one starts.
//! - `oncePer: subject` remembers a subject when its action starts or its approval is asked; a start that fails forgets it again,
//!   so a later event may try.
//!
//! A person's own 시험 실행, 다시 실행 and 지금 실행 go straight to the fire: they are the approval.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use chrono::{DateTime, Local, Utc};
use mighty_core::automation::policy::{self as rules, Skip, Verdict};
use mighty_core::automation::spec::{ActionKind, FireMode};
use mighty_core::automation::watch::TriggerEvent;
use mighty_core::locale::Locale;
use serde::{Deserialize, Serialize};

use super::{Item, Job, lock};
use crate::event_inbox::{Outcome, TriggerOutcome};
use crate::host::Host;

/// How long an approval waits before it is dropped as expired.
pub const APPROVAL_DAYS: i64 = 7;
/// A fire whose record has no pane yet still counts as running this long.
const STARTING_MINUTES: i64 = 10;

/// An action waiting for the person's 실행 / 건너뛰기.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Approval {
    pub id: String,
    /// The trigger's job key.
    pub key: String,
    pub name: String,
    #[serde(default)]
    pub workspace: String,
    /// The event inbox record it is noted under.
    pub inbox: String,
    pub event: TriggerEvent,
    /// What will run, in words.
    pub what: String,
    /// RFC 3339.
    pub at: String,
}

/// An event waiting for one of its trigger's runs to end.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Waiting {
    key: String,
    inbox: String,
    event: TriggerEvent,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Pending {
    #[serde(default)]
    approvals: Vec<Approval>,
    #[serde(default)]
    waiting: Vec<Waiting>,
}

static PENDING: LazyLock<Mutex<Option<Pending>>> = LazyLock::new(Default::default);
/// Fires started and not yet back with their record, by job key (so two events at once cannot both pass a concurrency of 1).
static STARTING: LazyLock<Mutex<std::collections::HashMap<String, usize>>> = LazyLock::new(Default::default);
/// The approvals waiting, for the shell's 이벤트 tab badge.
static WAITING_APPROVALS: AtomicUsize = AtomicUsize::new(0);

fn path(host: &Host) -> PathBuf {
    super::store::folder(&host.data_dir).join("pending.json")
}

/// Runs `edit` over what waits (read from disk the first time), saved when `save`.
fn pending_do<T>(host: &Host, save: bool, edit: impl FnOnce(&mut Pending) -> T) -> T {
    let mut slot = lock(&PENDING);
    let pending = slot
        .get_or_insert_with(|| std::fs::read(path(host)).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default());
    let out = edit(pending);
    WAITING_APPROVALS.store(pending.approvals.len(), Ordering::Relaxed);
    if save
        && host.writes()
        && let Ok(bytes) = serde_json::to_vec_pretty(pending)
    {
        let _ = mighty_core::automation::write_atomic(&path(host), &bytes);
    }
    out
}

/// Changes what waits, and saves it.
fn with_pending<T>(host: &Host, edit: impl FnOnce(&mut Pending) -> T) -> T {
    pending_do(host, true, edit)
}

/// Reads what waits.
fn read_pending<T>(host: &Host, read: impl FnOnce(&Pending) -> T) -> T {
    pending_do(host, false, |p| read(p))
}

/// How many approvals wait (the 이벤트 tab's and the bell's badge).
pub fn approvals_waiting() -> usize {
    WAITING_APPROVALS.load(Ordering::Relaxed)
}

fn today() -> String {
    Local::now().format("%Y-%m-%d").to_string()
}

/// The trigger's runs going now: its unsettled records whose pane is still there or that started in the last few minutes (their pane
/// still being made), and its fires on their way.
fn running(host: &Host, job: &Job) -> usize {
    let now = Utc::now();
    let recorded = {
        let view = host.view();
        let history = lock(&super::automation().history);
        history
            .records
            .iter()
            .filter(|r| r.workspace == job.workspace && r.origin == job.origin.name() && r.name == job.name() && !r.state.finished())
            // A pane still there, or one made moments ago (a bot's develop child appears a little after its record names it).
            .filter(|r| {
                let recent = DateTime::parse_from_rfc3339(&r.fired_at)
                    .is_ok_and(|at| (now - at.with_timezone(&Utc)).num_minutes() < STARTING_MINUTES);
                r.pane.as_deref().is_some_and(|pane| view.snapshot.session(pane).is_some()) || recent
            })
            .count()
    };
    recorded + lock(&STARTING).get(&job.key).copied().unwrap_or(0)
}

fn waiting_of(host: &Host, key: &str) -> usize {
    read_pending(host, |p| p.waiting.iter().filter(|w| w.key == key).count())
}

/// A skip's reason in the app's language.
pub fn skip_words(locale: &Locale, skip: &Skip) -> String {
    match skip {
        Skip::Handled(subject) => locale.get_with("automation.policy.handled", &[("subject", subject)]),
        Skip::Cooldown(until) => {
            let time = until.with_timezone(&Local).format("%H:%M").to_string();
            locale.get_with("automation.policy.cooldown", &[("time", &time)])
        }
        Skip::Capped(cap) => locale.get_with("automation.policy.capped", &[("cap", &cap.to_string())]),
        Skip::Full(n) => locale.get_with("automation.policy.full", &[("running", &n.to_string())]),
    }
}

/// What the trigger's action will do for `event`, in words: its kind, then the command, the event or the request's first line, and
/// who does it.
pub fn what_words(locale: &Locale, job: &Job, event: Option<&TriggerEvent>) -> String {
    let action = job.action();
    let kind = locale.get(super::do_key(action.kind));
    let detail = match action.kind {
        ActionKind::Run => mighty_core::automation::action::run_command(&action.with, event)
            .unwrap_or_else(|_| action.with.command.clone().unwrap_or_default()),
        ActionKind::Emit => action.with.event.clone().unwrap_or_default(),
        ActionKind::Develop => {
            let mut words = vec![];
            if action.with.pr() {
                words.push(locale.get("automation.do.withPr"));
            }
            if action.with.merge() {
                words.push(locale.get("automation.do.withMerge"));
            }
            words.join(" · ")
        }
        ActionKind::Ask => action.prompt.lines().find(|l| !l.trim().is_empty()).unwrap_or("").chars().take(80).collect(),
    };
    let who = match (action.kind, action.mode) {
        (ActionKind::Emit, _) => String::new(),
        (_, FireMode::Heartbeat) => action.pane.clone().unwrap_or_default(),
        _ => mighty_run::providers::name(action.agent()).to_string(),
    };
    [kind, detail, who].into_iter().map(|w| w.trim().to_string()).filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" · ")
}

/// MC-99: an event passed `job`'s `where`: its policy decides, and what came of it is noted under the inbox record `inbox`.
pub(crate) fn admit(host: &Arc<Host>, job: &Job, event: TriggerEvent, inbox: String) {
    let Item::Trigger(trigger) = &job.item else { return };
    let state = super::with_state(|s| s.policy.get(&job.key).cloned().unwrap_or_default());
    let verdict = rules::decide(
        &trigger.policy,
        &trigger.action,
        &state,
        &event.subject,
        Utc::now(),
        &today(),
        running(host, job),
        waiting_of(host, &job.key),
        false,
    );
    act(host, job, event, inbox, verdict);
}

fn act(host: &Arc<Host>, job: &Job, event: TriggerEvent, inbox: String, verdict: Verdict) {
    let l = host.locale;
    match verdict {
        Verdict::Go => start(host, job, event, inbox),
        Verdict::Skip(skip) => {
            let words = skip_words(&l, &skip);
            host.log.line(&format!("automation: '{}' skips {} ({})", job.name(), event.subject, skip.words()));
            crate::event_inbox::note(host, &inbox, TriggerOutcome { reason: Some(words), ..TriggerOutcome::new(job, Outcome::Skipped) });
        }
        Verdict::Hold => {
            let n = running(host, job);
            host.log.line(&format!("automation: '{}' holds {} ({n} running)", job.name(), event.subject));
            let words = l.get_with("automation.policy.waiting", &[("running", &n.to_string())]);
            crate::event_inbox::note(host, &inbox, TriggerOutcome { reason: Some(words), ..TriggerOutcome::new(job, Outcome::Held) });
            remember(host, job, &event.subject);
            with_pending(host, |p| p.waiting.push(Waiting { key: job.key.clone(), inbox, event }));
        }
        Verdict::Approve => ask(host, job, event, inbox),
    }
}

/// `oncePer`'s subject kept for `job`.
fn remember(host: &Host, job: &Job, subject: &str) {
    let Item::Trigger(trigger) = &job.item else { return };
    if trigger.policy.once_per.is_some() {
        super::with_state(|s| s.policy.entry(job.key.clone()).or_default().remember(subject));
        super::save_state(host);
    }
}

/// The action started: counted (its last start and today's), its subject kept, fired on a task and noted.
fn start(host: &Arc<Host>, job: &Job, event: TriggerEvent, inbox: String) {
    super::with_state(|s| s.policy.entry(job.key.clone()).or_default().started(Utc::now(), &today()));
    remember(host, job, &event.subject);
    *lock(&STARTING).entry(job.key.clone()).or_default() += 1;
    let (host, job) = (host.clone(), job.clone());
    tauri::async_runtime::spawn(async move {
        let subject = event.subject.clone();
        let reason = event.title.clone();
        let request =
            super::fire::FireRequest { job: job.clone(), reason, scheduled_for: None, event: Some(event), retry_of: None, asked: false };
        let fired = super::fire::fire(&host, request).await;
        if let Some(n) = lock(&STARTING).get_mut(&job.key) {
            *n = n.saturating_sub(1);
        }
        // A start that failed lets a later event about the subject try again.
        if matches!(fired.outcome, super::fire::FireOutcome::Failed(_)) {
            super::with_state(|s| s.policy.entry(job.key.clone()).or_default().forget(&subject));
            super::save_state(&host);
        }
        let ended = !matches!(fired.outcome, super::fire::FireOutcome::Started);
        crate::event_inbox::note(&host, &inbox, super::triggers::outcome_of(&job, fired));
        // Nothing runs on (skipped, failed, an emit): the next waiting one may go.
        if ended {
            drain(&host, &job.key);
        }
    });
}

/// The approval asked: kept, noted beside the event, and told to the OS and the phone once.
fn ask(host: &Arc<Host>, job: &Job, event: TriggerEvent, inbox: String) {
    let l = host.locale;
    let what = what_words(&l, job, Some(&event));
    remember(host, job, &event.subject);
    let approval = Approval {
        id: uuid::Uuid::new_v4().simple().to_string(),
        key: job.key.clone(),
        name: job.name().to_string(),
        workspace: job.workspace_name.clone(),
        inbox: inbox.clone(),
        event: event.clone(),
        what: what.clone(),
        at: Utc::now().to_rfc3339(),
    };
    with_pending(host, |p| p.approvals.push(approval));
    host.log.line(&format!("automation: '{}' waits for approval of {}", job.name(), event.subject));
    let words = l.get_with("automation.policy.awaiting", &[("what", &what)]);
    crate::event_inbox::note(host, &inbox, TriggerOutcome { reason: Some(words), ..TriggerOutcome::new(job, Outcome::Awaiting) });
    let title = l.get_with("automation.notify.approveTitle", &[("name", job.name())]);
    let body = format!("{} · {}", [event.kind.as_str(), event.subject.as_str()].join(" "), what);
    crate::notifications::notify_custom(host, title.clone(), body.clone());
    if let Some(push) = crate::mobile::push(host) {
        let _ = push.send_custom(mighty_remote::push::message::PERMISSION, &title, &body, "");
    }
    host.schedule();
}

/// The approvals waiting, the oldest first (the ones past [`APPROVAL_DAYS`] noted as expired and dropped first).
pub fn approvals(host: &Arc<Host>) -> Vec<Approval> {
    expire(host);
    read_pending(host, |p| p.approvals.clone())
}

/// Drops the approvals older than [`APPROVAL_DAYS`], each noted as expired beside its event.
fn expire(host: &Arc<Host>) {
    let now = Utc::now();
    let old =
        |a: &Approval| DateTime::parse_from_rfc3339(&a.at).map_or(true, |at| (now - at.with_timezone(&Utc)).num_days() >= APPROVAL_DAYS);
    if !read_pending(host, |p| p.approvals.iter().any(old)) {
        return;
    }
    let gone: Vec<Approval> = with_pending(host, |p| {
        let gone = p.approvals.iter().filter(|a| old(a)).cloned().collect();
        p.approvals.retain(|a| !old(a));
        gone
    });
    for approval in gone {
        note_of(host, &approval, Outcome::Skipped, host.locale.get("automation.policy.expired"));
    }
}

/// An outcome noted for an approval's trigger beside its event (the job may be gone: its name and workspace as kept).
fn note_of(host: &Arc<Host>, approval: &Approval, outcome: Outcome, reason: String) {
    let job = super::job(&approval.key);
    let base = match &job {
        Some(job) => TriggerOutcome::new(job, outcome),
        None => TriggerOutcome {
            key: approval.key.clone(),
            name: approval.name.clone(),
            workspace: approval.workspace.clone(),
            outcome,
            miss: None,
            reason: None,
            run: None,
            at: Utc::now().to_rfc3339(),
            replay: false,
        },
    };
    crate::event_inbox::note(host, &approval.inbox, TriggerOutcome { reason: Some(reason), ..base });
}

/// 실행 (`run`) or 건너뛰기 on the approval `id`: the words of what happened, or why it cannot (gone, its rule gone).
pub fn decide(host: &Arc<Host>, id: &str, run: bool) -> Result<String, String> {
    let l = host.locale;
    let approval = with_pending(host, |p| {
        let found = p.approvals.iter().position(|a| a.id == id)?;
        Some(p.approvals.remove(found))
    })
    .ok_or_else(|| l.get("automation.policy.approvalGone"))?;
    host.schedule();
    if !run {
        host.log.line(&format!("automation: '{}' skipped by the person ({})", approval.name, approval.event.subject));
        note_of(host, &approval, Outcome::Skipped, l.get("automation.policy.declined"));
        return Ok(l.get_with("automation.policy.declinedNote", &[("name", &approval.name)]));
    }
    let Some(job) = super::job(&approval.key).filter(Job::runnable) else {
        note_of(host, &approval, Outcome::Failed, l.get("automation.events.gone"));
        return Err(l.get("automation.events.gone"));
    };
    let Item::Trigger(trigger) = &job.item else { return Err(l.get("automation.events.gone")) };
    let verdict = rules::decide(
        &trigger.policy,
        &trigger.action,
        &Default::default(),
        &approval.event.subject,
        Utc::now(),
        &today(),
        running(host, &job),
        waiting_of(host, &job.key),
        true,
    );
    let words = match &verdict {
        Verdict::Hold => l.get_with("automation.policy.approvedWaits", &[("name", job.name())]),
        Verdict::Skip(skip) => skip_words(&l, skip),
        _ => l.get_with("automation.policy.approved", &[("name", job.name())]),
    };
    act(host, &job, approval.event, approval.inbox, verdict);
    Ok(words)
}

/// A run of a trigger ended (fire.rs): its next waiting event may start.
pub(crate) fn released(host: &Arc<Host>, record: &mighty_core::automation::records::RunRecord) {
    let key = super::jobs()
        .into_iter()
        .find(|j| j.workspace == record.workspace && j.origin.name() == record.origin && j.name() == record.name)
        .map(|j| j.key);
    if let Some(key) = key {
        drain(host, &key);
    }
}

/// Starts the waiting events of the trigger `key` while its concurrency lets them (they wait on while it does not); a trigger gone
/// or turned off lets them go, noted.
pub(crate) fn drain(host: &Arc<Host>, key: &str) {
    loop {
        let Some(next) = read_pending(host, |p| p.waiting.iter().find(|w| w.key == key).cloned()) else { return };
        let job = super::job(key).filter(Job::runnable);
        let Some(job) = job else {
            with_pending(host, |p| p.waiting.retain(|w| w.key != key));
            let reason = host.locale.get("automation.events.gone");
            crate::event_inbox::note(
                host,
                &next.inbox,
                TriggerOutcome {
                    key: key.to_string(),
                    name: key.rsplit(':').next().unwrap_or(key).to_string(),
                    workspace: String::new(),
                    outcome: Outcome::Skipped,
                    miss: None,
                    reason: Some(reason),
                    run: None,
                    at: Utc::now().to_rfc3339(),
                    replay: false,
                },
            );
            return;
        };
        let Item::Trigger(trigger) = &job.item else { return };
        let limit = trigger.policy.concurrency(&trigger.action);
        if limit.is_some_and(|n| running(host, &job) >= n as usize) {
            return;
        }
        with_pending(host, |p| {
            if let Some(i) = p.waiting.iter().position(|w| w == &next) {
                p.waiting.remove(i);
            }
        });
        host.log.line(&format!("automation: '{}' starts {} that waited", job.name(), next.event.subject));
        start(host, &job, next.event, next.inbox);
    }
}

/// The jobs were read again: what waits for a trigger that is runnable may start now (after a restart, the runs that went on are
/// counted by their records).
pub(crate) fn drain_all(host: &Arc<Host>) {
    expire(host);
    let mut keys: Vec<String> = read_pending(host, |p| p.waiting.iter().map(|w| w.key.clone()).collect());
    keys.sort();
    keys.dedup();
    for key in keys {
        drain(host, &key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_waits_reads_back_from_its_json() {
        let event = TriggerEvent { id: "e".into(), title: "v0.6.4 릴리스".into(), ..TriggerEvent::default() }.with_envelope(
            "mcp:mightywork",
            "mightywork.task.stage_changed",
            "MC-12",
            None,
        );
        let pending = Pending {
            approvals: vec![Approval {
                id: "a1".into(),
                key: "w:app:t:release".into(),
                name: "release".into(),
                workspace: "Shop".into(),
                inbox: "i1".into(),
                event: event.clone(),
                what: "run · /release v0.6.4 · @릴리스".into(),
                at: "2026-11-26T10:00:00Z".into(),
            }],
            waiting: vec![Waiting { key: "w:app:t:dev".into(), inbox: "i2".into(), event }],
        };
        let text = serde_json::to_string(&pending).unwrap();
        let back: Pending = serde_json::from_str(&text).unwrap();
        assert_eq!((back.approvals, back.waiting), (pending.approvals, pending.waiting));
        let none: Pending = serde_json::from_str("{}").unwrap();
        assert!(none.approvals.is_empty() && none.waiting.is_empty());
    }

    #[test]
    fn a_skip_has_its_words_in_every_language() {
        for language in mighty_core::locale::LANGUAGES {
            let l = Locale::new(language);
            for skip in [Skip::Handled("PR#7".into()), Skip::Capped(3), Skip::Full(1), Skip::Cooldown(Utc::now())] {
                let words = skip_words(&l, &skip);
                assert!(!words.starts_with("automation."), "{language}: {words}");
            }
            for kind in ActionKind::ALL {
                for key in [super::super::do_key(kind), super::super::do_note_key(kind)] {
                    assert!(!l.get(key).starts_with("automation."), "{language} {key}");
                }
            }
        }
    }
}
