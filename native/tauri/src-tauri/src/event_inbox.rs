//! MC-97: the 알림 함's 이벤트: the last [`MAX_EVENTS`] events that reached the automation layer (a GitHub or Linear look's new items,
//! a standing order's changed output, a webhook's delivery, `mighty event emit`, a bot's request ending), each with what every
//! trigger listening to it did: fired (with its run), skipped by a `where` condition (which one, what it wanted and the value seen),
//! skipped because its bot is asleep or over budget, held back (past a look's few) or failed to start. Kept in `<data>/events.json`
//! the way notices.rs keeps the general notices.
//!
//! The page lists them (`automationEvents`, the newest first), runs one event again against one of the triggers it reached
//! (`automationEventReplay`: the usual fire, so its approval and permission rules hold; refused when the trigger is gone) and clears
//! them (`automationEventsClear`). The same item seen by two triggers of a repository (each looks on its own) is one event.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use chrono::{DateTime, Utc};
use mighty_core::automation::condition::Miss;
use mighty_core::automation::watch::TriggerEvent;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::host::{Host, Registry};
use crate::methods::text as arg;

/// The most events kept.
pub const MAX_EVENTS: usize = 200;
/// How long after an event the same item (its source, type, subject and id) seen by another trigger still joins it.
const JOIN_SECONDS: i64 = 60 * 60;

/// What one trigger did with an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    /// Its conditions held and it fired (its run in `run`).
    Fired,
    /// A `where` condition did not hold (`miss`).
    Unmatched,
    /// Its bot is asleep.
    Asleep,
    /// Past the daily budget, or its bot's own.
    Budget,
    /// Its conditions held, but a look fires a few at most: this one waited out.
    Held,
    /// It fired but could not start (`reason`).
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TriggerOutcome {
    /// The trigger's job key (a replay finds it by this).
    pub key: String,
    pub name: String,
    #[serde(default)]
    pub workspace: String,
    pub outcome: Outcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub miss: Option<Miss>,
    /// The words of a skip or a failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The run record it started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    /// RFC 3339.
    pub at: String,
    /// A person's 시험 실행, not the event's own arrival.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub replay: bool,
}

impl TriggerOutcome {
    pub fn new(job: &crate::automation::Job, outcome: Outcome) -> TriggerOutcome {
        TriggerOutcome {
            key: job.key.clone(),
            name: job.name().to_string(),
            workspace: job.workspace_name.clone(),
            outcome,
            miss: None,
            reason: None,
            run: None,
            at: Utc::now().to_rfc3339(),
            replay: false,
        }
    }
}

/// One event and what the triggers listening to it did.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventRecord {
    pub id: String,
    pub event: TriggerEvent,
    #[serde(default)]
    pub triggers: Vec<TriggerOutcome>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Store {
    events: Vec<EventRecord>,
}

static STORE: LazyLock<Mutex<Option<Store>>> = LazyLock::new(Default::default);
/// Moves at every change, so the page reads the list again (the frame model's `eventInboxVersion`).
static VERSION: AtomicUsize = AtomicUsize::new(0);

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn path(host: &Host) -> PathBuf {
    host.data_dir.join("events.json")
}

/// Runs `edit` over the events (read from disk the first time) and saves them when it says so.
fn with_store<T>(host: &Host, edit: impl FnOnce(&mut Store) -> (T, bool)) -> T {
    let mut slot = lock(&STORE);
    let store = slot
        .get_or_insert_with(|| std::fs::read(path(host)).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default());
    let (out, changed) = edit(store);
    if changed {
        VERSION.fetch_add(1, Ordering::Relaxed);
        if host.writes()
            && let Ok(bytes) = serde_json::to_vec(store)
        {
            let _ = mighty_core::automation::write_atomic(&path(host), &bytes);
        }
    }
    out
}

/// The record `event` joins: the newest of the same source, type, subject and id within [`JOIN_SECONDS`] (an id-less event joins
/// none).
fn joins(store: &Store, event: &TriggerEvent) -> Option<usize> {
    if event.id.is_empty() {
        return None;
    }
    let at = |text: &str| DateTime::parse_from_rfc3339(text).ok().map(|t| t.timestamp());
    let now = at(&event.time).unwrap_or_else(|| Utc::now().timestamp());
    store.events.iter().rposition(|r| {
        r.event.id == event.id
            && r.event.source == event.source
            && r.event.kind == event.kind
            && r.event.subject == event.subject
            && at(&r.event.time).is_some_and(|t| (now - t).abs() <= JOIN_SECONDS)
    })
}

/// Keeps `event` (or finds the record it joins), the oldest let go of past [`MAX_EVENTS`]: its record's id.
fn keep(store: &mut Store, event: &TriggerEvent) -> String {
    if let Some(found) = joins(store, event) {
        return store.events[found].id.clone();
    }
    let id = uuid::Uuid::new_v4().simple().to_string();
    store.events.push(EventRecord { id: id.clone(), event: event.clone(), triggers: vec![] });
    let over = store.events.len().saturating_sub(MAX_EVENTS);
    store.events.drain(..over);
    id
}

/// An event reached the automation layer: kept, and its record's id (what its triggers' outcomes are noted under).
pub fn arrived(host: &Arc<Host>, event: &TriggerEvent) -> String {
    let id = with_store(host, |store| (keep(store, event), true));
    host.schedule();
    id
}

/// What a trigger did with the event of record `id`.
pub fn note(host: &Arc<Host>, id: &str, outcome: TriggerOutcome) {
    with_store(host, |store| match store.events.iter_mut().find(|r| r.id == id) {
        Some(record) => {
            record.triggers.push(outcome);
            ((), true)
        }
        None => ((), false),
    });
    host.schedule();
}

/// The record `id`, if kept.
pub fn get(host: &Host, id: &str) -> Option<EventRecord> {
    with_store(host, |store| (store.events.iter().find(|r| r.id == id).cloned(), false))
}

/// For the frame model: the page reads the list again when this moves.
pub fn decorate(model: &mut mighty_core::shell::ShellModel) {
    model.event_inbox_version = VERSION.load(Ordering::Relaxed);
}

pub fn register(methods: &mut Registry) {
    // The 이벤트 tab: every event kept, the newest first.
    methods.add("automationEvents", |host: Arc<Host>, _args| async move {
        let events = with_store(&host, |store| (store.events.iter().rev().cloned().collect::<Vec<_>>(), false));
        Ok(json!({ "events": events }))
    });
    // 시험 실행: the event `id` run again against the trigger `trigger` (a job key), through the usual fire.
    methods.add("automationEventReplay", |host: Arc<Host>, args| async move {
        let id = arg(&args, "id").unwrap_or_default().to_string();
        let trigger = arg(&args, "trigger").unwrap_or_default().to_string();
        Ok(match crate::automation::triggers::replay(&host, &id, &trigger).await {
            Ok(words) => json!({ "ok": true, "note": words }),
            Err(error) => json!({ "error": error }),
        })
    });
    methods.add("automationEventsClear", |host: Arc<Host>, _args| async move {
        with_store(&host, |store| {
            store.events.clear();
            ((), true)
        });
        host.schedule();
        Ok(Value::Null)
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(id: &str, time: &str) -> TriggerEvent {
        TriggerEvent { id: id.into(), title: format!("PR {id}"), time: time.into(), ..TriggerEvent::default() }.with_envelope(
            "github:a/b",
            "github.pr.opened",
            id,
            None,
        )
    }

    #[test]
    fn the_inbox_keeps_the_newest_two_hundred() {
        let mut store = Store::default();
        for i in 0..MAX_EVENTS + 5 {
            keep(&mut store, &event(&format!("pr:{i}"), "2026-11-25T10:00:00Z"));
        }
        assert_eq!(store.events.len(), MAX_EVENTS);
        assert_eq!(store.events[0].event.id, "pr:5");
        assert_eq!(store.events.last().map(|r| r.event.id.as_str()), Some(format!("pr:{}", MAX_EVENTS + 4).as_str()));
    }

    #[test]
    fn the_same_item_seen_by_another_trigger_joins_its_event_within_the_hour() {
        let mut store = Store::default();
        let first = keep(&mut store, &event("pr:7", "2026-11-25T10:00:00Z"));
        assert_eq!(keep(&mut store, &event("pr:7", "2026-11-25T10:30:00Z")), first, "another trigger's look");
        assert_ne!(keep(&mut store, &event("pr:7", "2026-11-25T12:00:00Z")), first, "hours later it is another event");
        let other = TriggerEvent { id: "pr:7".into(), time: "2026-11-25T10:01:00Z".into(), ..TriggerEvent::default() }.with_envelope(
            "github:a/b",
            "github.pr.merged",
            "PR#7",
            None,
        );
        assert_ne!(keep(&mut store, &other), first, "another type");
        let id_less = TriggerEvent { time: "2026-11-25T10:00:00Z".into(), ..TriggerEvent::default() };
        assert_ne!(keep(&mut store, &id_less), keep(&mut store, &id_less));
    }

    #[test]
    fn a_record_reads_back_from_its_json() {
        let record = EventRecord {
            id: "e1".into(),
            event: event("pr:7", "2026-11-25T10:00:00Z"),
            triggers: vec![TriggerOutcome {
                key: "w:app:t:review".into(),
                name: "review".into(),
                workspace: "Shop".into(),
                outcome: Outcome::Unmatched,
                miss: Some(Miss { field: "labels".into(), op: "has".into(), expected: "bot:dev".into(), seen: None }),
                reason: None,
                run: None,
                at: "2026-11-25T10:00:01Z".into(),
                replay: false,
            }],
        };
        let value = serde_json::to_value(&record).unwrap();
        assert_eq!(
            (value["event"]["type"].as_str(), value["triggers"][0]["outcome"].as_str()),
            (Some("github.pr.opened"), Some("unmatched"))
        );
        assert!(value["triggers"][0].get("replay").is_none() && value["triggers"][0]["miss"].get("seen").is_none());
        assert_eq!(serde_json::from_value::<EventRecord>(value).unwrap(), record);
    }
}
