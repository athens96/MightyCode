//! MC-90: the 알림 함's general notices: a workspace's agent run that finished (✅ completed or ❌ failed) kept with its pane, its
//! workspace, the request it answered, a one-line summary of what it said and the diagram's run, so the page can list it beside the
//! bots' reports (봇 알림, bot_report.rs) and move to that pane's diagram on a double click or 이동. A bot's scheduled or triggered run
//! is left to its report; a run the person stopped is not kept.

use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::host::{Host, Registry};
use crate::methods::text as arg;

/// The most notices kept.
const MAX_NOTICES: usize = 400;
/// The longest summary and request kept, in characters.
const SUMMARY_CHARS: usize = 200;
const REQUEST_CHARS: usize = 160;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Notice {
    pub id: String,
    pub pane: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default)]
    pub workspace: String,
    /// The pane's title.
    #[serde(default)]
    pub title: String,
    /// `ok` (completed) or `failed`.
    pub status: String,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<String>,
    /// The diagram's run the notice is about (the page aims at its request block).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    /// RFC 3339.
    pub at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seconds: Option<u64>,
    #[serde(default)]
    pub read: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Store {
    notices: Vec<Notice>,
}

static STORE: LazyLock<Mutex<Option<Store>>> = LazyLock::new(Default::default);

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn path(host: &Host) -> PathBuf {
    host.data_dir.join("notices.json")
}

/// Runs `edit` over the notices (read from disk the first time) and saves them when it says so.
fn with_store<T>(host: &Host, edit: impl FnOnce(&mut Store) -> (T, bool)) -> T {
    let mut slot = lock(&STORE);
    let store = slot
        .get_or_insert_with(|| std::fs::read(path(host)).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default());
    let (out, changed) = edit(store);
    if changed && let Ok(bytes) = serde_json::to_vec(store) {
        let _ = mighty_core::automation::write_atomic(&path(host), &bytes);
    }
    out
}

fn cut(text: &str, max: usize) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() <= max {
        return text;
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Keeps `notice`, the newest last, the oldest let go of past [`MAX_NOTICES`].
fn keep(store: &mut Store, notice: Notice) {
    store.notices.push(notice);
    let over = store.notices.len().saturating_sub(MAX_NOTICES);
    store.notices.drain(..over);
}

/// The status a run's end makes a notice of: `completed` is ✅, `error` ❌; a stop the person made is none.
pub fn status_of(ended: &str) -> Option<&'static str> {
    match ended {
        "completed" => Some("ok"),
        "error" => Some("failed"),
        _ => None,
    }
}

/// A pane's run ended (runs.rs, before the automation bus settles its record): a notice of it, unless a schedule or a trigger ran it
/// (its bot's report says it) or the person stopped it.
pub fn on_run_ended(host: &Arc<Host>, pane: &str, ended: &str) {
    let Some(status) = status_of(ended) else { return };
    // A schedule's or a trigger's run is still open here: the bot's report (봇 알림) is its notice.
    if crate::automation::fire::open_record_of(host, pane).is_some() {
        return;
    }
    let (workspace_id, workspace, title, run, seconds) = {
        let view = host.view();
        let Some(session) = view.snapshot.session(pane) else { return };
        let workspace_id = Some(session.workspace_id().to_string()).filter(|w| !w.is_empty());
        let workspace = view.snapshot.workspace(session.workspace_id()).map(|w| w.name().to_string()).unwrap_or_default();
        let run = session.graph_runs().iter().rev().find_map(|r| r.id.clone());
        let seconds = session.run_timing.filter(|t| t.is_valid()).and_then(|t| {
            let end = t.finished_at.unwrap_or(t.last_observed_at);
            u64::try_from((end.utc_ticks() - t.started_at.utc_ticks()) / 10_000_000).ok()
        });
        (workspace_id, workspace, session.title().to_string(), run, seconds)
    };
    let (host, pane) = (host.clone(), pane.to_string());
    tauri::async_runtime::spawn(async move {
        // The pane's last answer (its body loaded when it is on disk), then the request it answered.
        let answer = crate::bot::answer(&host, &pane).await.get("answer").and_then(Value::as_str).unwrap_or_default().to_string();
        let request = {
            let view = host.view();
            view.snapshot
                .session(&pane)
                .and_then(|s| s.logs().iter().rev().find(|e| e.kind() == "user").map(|e| cut(e.text(), REQUEST_CHARS)))
                .filter(|r| !r.is_empty())
        };
        let (_, said, _) = crate::bot_report::from_answer(&answer);
        let summary = if !said.is_empty() {
            cut(&said, SUMMARY_CHARS)
        } else if status == "failed" {
            host.locale.get("notice.failedNoWhy")
        } else {
            host.locale.get("notice.nothing")
        };
        let notice = Notice {
            id: uuid::Uuid::new_v4().simple().to_string(),
            pane,
            workspace_id,
            workspace,
            title,
            status: status.to_string(),
            summary,
            request,
            run,
            at: Utc::now().to_rfc3339(),
            seconds,
            read: false,
        };
        host.log.line(&format!("notice: {} · {} {}", notice.title, notice.status, notice.summary));
        with_store(&host, |store| {
            keep(store, notice);
            ((), true)
        });
        host.schedule();
    });
}

/// The unread count, for the frame model (the bell's badge and the 일반 알림 tab).
pub fn decorate(host: &Host, model: &mut mighty_core::shell::ShellModel) {
    model.notices_unread = with_store(host, |store| (store.notices.iter().filter(|n| !n.read).count(), false));
}

pub fn register(methods: &mut Registry) {
    // The 알림 함's 일반 알림: every notice kept, the newest first.
    methods.add("notices", |host: Arc<Host>, _args| async move {
        let notices = with_store(&host, |store| (store.notices.iter().rev().cloned().collect::<Vec<_>>(), false));
        Ok(json!({ "notices": notices }))
    });
    // Read: one notice (`id`), or every one.
    methods.add("noticeRead", |host: Arc<Host>, args| async move {
        let id = arg(&args, "id").map(str::to_string);
        with_store(&host, |store| {
            for notice in store.notices.iter_mut().filter(|n| id.as_deref().is_none_or(|id| n.id == id)) {
                notice.read = true;
            }
            ((), true)
        });
        host.schedule();
        Ok(Value::Null)
    });
    // Cleared: one notice (`id`), or every one already read.
    methods.add("noticeClear", |host: Arc<Host>, args| async move {
        let id = arg(&args, "id").map(str::to_string);
        with_store(&host, |store| {
            store.notices.retain(|n| match id.as_deref() {
                Some(id) => n.id != id,
                None => !n.read,
            });
            ((), true)
        });
        host.schedule();
        Ok(Value::Null)
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notice(id: &str) -> Notice {
        Notice {
            id: id.into(),
            pane: "p1".into(),
            workspace_id: Some("w1".into()),
            workspace: "Shop".into(),
            title: "Claude".into(),
            status: "ok".into(),
            summary: "Done.".into(),
            request: None,
            run: Some("r1".into()),
            at: "2026-11-25T10:00:00Z".into(),
            seconds: Some(12),
            read: false,
        }
    }

    #[test]
    fn a_finished_run_is_a_notice_and_a_stopped_one_is_not() {
        assert_eq!(status_of("completed"), Some("ok"));
        assert_eq!(status_of("error"), Some("failed"));
        assert_eq!(status_of("stopped"), None);
        assert_eq!(status_of("running"), None);
    }

    #[test]
    fn the_store_keeps_the_newest_and_lets_the_oldest_go() {
        let mut store = Store::default();
        for i in 0..MAX_NOTICES + 3 {
            keep(&mut store, notice(&i.to_string()));
        }
        assert_eq!(store.notices.len(), MAX_NOTICES);
        assert_eq!(store.notices[0].id, "3");
        assert_eq!(store.notices.last().map(|n| n.id.as_str()), Some((MAX_NOTICES + 2).to_string().as_str()));
    }

    #[test]
    fn a_notice_reads_back_from_its_json() {
        let kept = notice("a");
        let text = serde_json::to_string(&kept).unwrap();
        assert!(text.contains("\"workspaceId\":\"w1\"") && text.contains("\"run\":\"r1\"") && !text.contains("request"));
        assert_eq!(serde_json::from_str::<Notice>(&text).unwrap(), kept);
    }

    #[test]
    fn a_long_request_is_cut_to_one_line() {
        let long = format!("first line\nsecond {}", "word ".repeat(60));
        let cut_text = cut(&long, REQUEST_CHARS);
        assert!(!cut_text.contains('\n') && cut_text.chars().count() == REQUEST_CHARS && cut_text.ends_with('…'));
    }
}
