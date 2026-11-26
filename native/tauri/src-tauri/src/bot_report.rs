//! MC-73: a bot's report of the work it just finished — its status (✅ ok · 📝 report · ⚠️ needs · ❌ failed), a one-line summary,
//! up to three items with their links, its cost, time and what fired it — kept for the report box (shell/ReportBox.tsx), its last
//! one on the bot's sidebar row, and appended to `<data>/bots/<handle>/reports/<date>.md`.
//!
//! A report comes three ways: the bot files it with mighty-terminal's `bot_report` tool; a schedule's or a trigger's run that ends
//! with none filed gets one made from its last answer; a run that fails or is skipped gets a ❌ one saying why. A report that says
//! what the last one of the same bot said is counted on that one (`repeat`) instead. ⚠️ and ❌ notify the person (the OS and the
//! phone); 📝 and ✅ wait in the box.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex};

use chrono::{DateTime, Local, Utc};
use mighty_core::automation::records::{RecordState, RunRecord};
use mighty_core::shell::SidebarReport;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::host::{Host, Registry};
use crate::methods::text as arg;

/// The most reports kept.
const MAX_REPORTS: usize = 400;
/// The longest summary and item line kept, in characters.
const SUMMARY_CHARS: usize = 200;
const ITEM_CHARS: usize = 300;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportItem {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub id: String,
    pub handle: String,
    #[serde(default)]
    pub pane: Option<String>,
    /// `ok`, `report`, `needs` or `failed`.
    pub status: String,
    pub summary: String,
    #[serde(default)]
    pub items: Vec<ReportItem>,
    /// RFC 3339.
    pub at: String,
    /// The run it reports (a schedule's or a trigger's), its job's name and what fired it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_url: Option<String>,
    #[serde(default)]
    pub read: bool,
    /// 👍 (true) or 👎 (false).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumb: Option<bool>,
    /// How many times in a row it said the same (1 for once).
    #[serde(default = "one")]
    pub repeat: u32,
    /// MC-90: the diagram's run of its pane the report is about (이동 aims at its request block).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
}

/// The newest run of `pane`'s diagram.
fn newest_run(host: &Host, pane: Option<&str>) -> Option<String> {
    let view = host.view();
    view.snapshot.session(pane?)?.graph_runs().iter().rev().find_map(|r| r.id.clone())
}

fn one() -> u32 {
    1
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Store {
    reports: Vec<Report>,
}

static STORE: LazyLock<Mutex<Option<Store>>> = LazyLock::new(Default::default);

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn path(host: &Host) -> PathBuf {
    host.data_dir.join("bot-reports.json")
}

/// Runs `edit` over the reports (read from disk the first time) and saves them when it says so.
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
    let text = text.trim();
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// The status a word names, `report` when it names none.
pub fn status_of(word: &str) -> &'static str {
    match word.trim() {
        "ok" => "ok",
        "needs" => "needs",
        "failed" => "failed",
        _ => "report",
    }
}

/// A report made from a run's last answer: ✅ when it said nothing (or `NO_REPLY`), else 📝 with its first sentence and the links in
/// it (three at most).
pub fn from_answer(answer: &str) -> (&'static str, String, Vec<ReportItem>) {
    let text = answer.trim();
    let quiet = text.is_empty() || text.lines().last().is_some_and(|l| l.trim().trim_matches(|c: char| c == '*' || c == '`') == "NO_REPLY");
    if quiet {
        return ("ok", String::new(), vec![]);
    }
    let first = text.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with("```")).unwrap_or(text);
    let first = first.trim_start_matches(['-', '*', '>', ' ']);
    // The end of the first sentence, after its stop (a CJK `。` is three bytes, so the cut follows the character's own length).
    let sentence = match first.char_indices().find(|&(_, c)| matches!(c, '.' | '。' | '!' | '?')) {
        Some((at, stop)) if at + stop.len_utf8() < first.len() && at > 10 => &first[..at + stop.len_utf8()],
        _ => first,
    };
    let mut items = Vec::new();
    for word in text.split_whitespace() {
        let url = word.trim_matches(|c: char| matches!(c, '(' | ')' | '<' | '>' | '[' | ']' | ',' | '.' | '`' | '"' | '\''));
        if (url.starts_with("https://") || url.starts_with("http://")) && !items.iter().any(|i: &ReportItem| i.url.as_deref() == Some(url))
        {
            let line = text.lines().find(|l| l.contains(url)).map(|l| cut(&l.replace(url, ""), ITEM_CHARS)).unwrap_or_default();
            let line = line.trim_matches(|c: char| c.is_whitespace() || matches!(c, '-' | '*' | '(' | ')' | ':' | '[' | ']')).to_string();
            items.push(ReportItem { text: if line.is_empty() { url.to_string() } else { line }, url: Some(url.to_string()) });
            if items.len() == 3 {
                break;
            }
        }
    }
    ("report", cut(sentence, SUMMARY_CHARS), items)
}

/// Whether `next` says what `last` said (same bot, status and summary): it is counted on `last`.
fn repeats(last: &Report, next: &Report) -> bool {
    last.handle == next.handle && last.status == next.status && last.summary == next.summary && last.items == next.items
}

/// Keeps `report` (or counts it on the bot's last one when it says the same), writes it to the bot's reports file, and notifies the
/// person when it needs them or failed. The report kept.
pub fn keep(host: &Arc<Host>, report: Report) -> Report {
    let kept = with_store(host, |store| {
        if let Some(last) = store.reports.iter_mut().rev().find(|r| r.handle == report.handle)
            && repeats(last, &report)
        {
            last.repeat += 1;
            last.at = report.at.clone();
            last.read = last.read && report.status == "ok";
            last.record = report.record.clone().or(last.record.take());
            last.cost_usd = report.cost_usd.or(last.cost_usd);
            last.seconds = report.seconds.or(last.seconds);
            last.run = report.run.clone().or(last.run.take());
            return (last.clone(), true);
        }
        store.reports.push(report.clone());
        let over = store.reports.len().saturating_sub(MAX_REPORTS);
        store.reports.drain(..over);
        (report.clone(), true)
    });
    append_file(host, &kept);
    if matches!(kept.status.as_str(), "needs" | "failed") && kept.repeat == 1 {
        notify(host, &kept);
    }
    host.log.line(&format!("bot report: @{} {} · {}", kept.handle, kept.status, kept.summary));
    host.schedule();
    kept
}

fn notify(host: &Arc<Host>, report: &Report) {
    let l = host.locale;
    let handle = [("handle", report.handle.as_str())];
    let title = if report.status == "needs" {
        l.get_with("bot.report.notify.needs", &handle)
    } else {
        l.get_with("bot.report.notify.failed", &handle)
    };
    crate::notifications::notify_custom(host, title.clone(), report.summary.clone());
    if let Some(push) = crate::mobile::push(host) {
        let kind = if report.status == "needs" { mighty_remote::push::message::QUESTION } else { mighty_remote::push::message::FAILED };
        // MC-107: a report with no pane (a skipped run) opens the phone's 봇 tab instead (a push without a session is dropped).
        let _ = push.send_custom(kind, &title, &report.summary, report.pane.as_deref().unwrap_or(crate::phone_bots::REPORT_PUSH));
    }
}

/// `<data>/bots/<handle>/reports/<date>.md`: one section a report.
fn append_file(host: &Host, report: &Report) {
    use std::io::Write as _;
    let at = DateTime::parse_from_rfc3339(&report.at).map(|t| t.with_timezone(&Local)).unwrap_or_else(|_| Local::now());
    let folder = host.data_dir.join("bots").join(&report.handle).join("reports");
    if std::fs::create_dir_all(&folder).is_err() {
        return;
    }
    let mark = match report.status.as_str() {
        "ok" => "✅",
        "needs" => "⚠️",
        "failed" => "❌",
        _ => "📝",
    };
    let mut text = format!("\n## {} {mark} {}\n", at.format("%H:%M"), report.summary);
    if let Some(job) = &report.job {
        text.push_str(&format!("\n- {job}{}\n", report.reason.as_deref().map(|r| format!(" · {r}")).unwrap_or_default()));
    }
    for item in &report.items {
        match &item.url {
            Some(url) => text.push_str(&format!("- [{}]({url})\n", item.text)),
            None => text.push_str(&format!("- {}\n", item.text)),
        }
    }
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(folder.join(format!("{}.md", at.format("%Y-%m-%d")))) {
        let _ = file.write_all(text.as_bytes());
    }
}

/// The reports filed during a run, by pane: the run's end fills in its cost and time instead of making one.
static FILED: LazyLock<Mutex<HashMap<String, String>>> = LazyLock::new(Default::default);

/// mighty-terminal's `bot_report` from the agent in pane `caller`: `value` is the tool's checked JSON (status, summary, items).
pub fn file(host: &Arc<Host>, caller: &str, value: &str) -> Value {
    let args: Value = serde_json::from_str(value).unwrap_or_default();
    let handle = host.view().snapshot.session(caller).and_then(|p| p.bot.as_ref()).map(|b| b.handle.clone());
    let Some(handle) = handle else { return json!({ "refused": host.locale.get("bot.report.notABot") }) };
    let items: Vec<ReportItem> = args
        .get("items")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|i| {
                    let text = cut(i.get("text").and_then(Value::as_str)?, ITEM_CHARS);
                    Some(ReportItem { text, url: i.get("url").and_then(Value::as_str).map(str::to_string) })
                })
                .take(3)
                .collect()
        })
        .unwrap_or_default();
    let record = crate::automation::fire::open_record_of(host, caller);
    let (job, reason, event_url) =
        record.as_ref().map(|r| (Some(r.name.clone()), Some(r.reason.clone()), r.event_url.clone())).unwrap_or_default();
    let report = Report {
        id: uuid::Uuid::new_v4().simple().to_string(),
        handle,
        pane: Some(caller.to_string()),
        status: status_of(args.get("status").and_then(Value::as_str).unwrap_or("report")).to_string(),
        summary: cut(args.get("summary").and_then(Value::as_str).unwrap_or_default(), SUMMARY_CHARS),
        items,
        at: Utc::now().to_rfc3339(),
        record: record.as_ref().map(|r| r.id.clone()),
        job,
        reason: reason.filter(|r| !r.is_empty()),
        cost_usd: None,
        seconds: None,
        event_url,
        read: false,
        thumb: None,
        repeat: 1,
        run: newest_run(host, Some(caller)),
    };
    let kept = keep(host, report);
    lock(&FILED).insert(caller.to_string(), kept.id.clone());
    json!({ "kept": true, "status": kept.status })
}

/// A schedule's or a trigger's run of a bot settled (finished, failed or skipped): fills in the report it filed, or makes one.
pub fn on_record(host: &Arc<Host>, record: &RunRecord) {
    let Some(handle) = handle_of(host, record) else { return };
    let seconds = record.finished_at.as_deref().and_then(|end| {
        let (a, b) = (DateTime::parse_from_rfc3339(&record.fired_at).ok()?, DateTime::parse_from_rfc3339(end).ok()?);
        u64::try_from((b - a).num_seconds()).ok()
    });
    // The report the bot filed during this run: its cost and time.
    let filed = record.pane.as_deref().and_then(|pane| lock(&FILED).remove(pane));
    if let Some(id) = filed {
        let found = with_store(host, |store| match store.reports.iter_mut().find(|r| r.id == id) {
            Some(report) => {
                report.cost_usd = record.cost_usd;
                report.seconds = seconds;
                report.record = Some(record.id.clone());
                (true, true)
            }
            None => (false, false),
        });
        if found {
            host.schedule();
            return;
        }
    }
    let failed = matches!(record.state, RecordState::Failed | RecordState::Skipped);
    if failed {
        let why = record.error.clone().filter(|e| !e.trim().is_empty()).unwrap_or_else(|| host.locale.get("bot.report.failedNoWhy"));
        keep(host, made(host, record, &handle, seconds, "failed", cut(&why, SUMMARY_CHARS), vec![]));
        return;
    }
    let (host, record) = (host.clone(), record.clone());
    tauri::async_runtime::spawn(async move {
        let answer = match &record.pane {
            Some(pane) => crate::bot::answer(&host, pane).await.get("answer").and_then(Value::as_str).unwrap_or_default().to_string(),
            None => String::new(),
        };
        let (status, summary, items) = from_answer(&answer);
        let summary = if summary.is_empty() { host.locale.get("bot.report.nothing") } else { summary };
        keep(&host, made(&host, &record, &handle, seconds, status, summary, items));
    });
}

/// A report of `record`'s run by the bot `handle`.
fn made(
    host: &Host,
    record: &RunRecord,
    handle: &str,
    seconds: Option<u64>,
    status: &str,
    summary: String,
    items: Vec<ReportItem>,
) -> Report {
    Report {
        id: uuid::Uuid::new_v4().simple().to_string(),
        handle: handle.to_string(),
        pane: record.pane.clone(),
        status: status.to_string(),
        summary,
        items,
        at: Utc::now().to_rfc3339(),
        record: Some(record.id.clone()),
        job: Some(record.name.clone()),
        reason: Some(record.reason.clone()).filter(|r| !r.is_empty()),
        cost_usd: record.cost_usd,
        seconds,
        event_url: record.event_url.clone(),
        read: false,
        thumb: None,
        repeat: 1,
        run: newest_run(host, record.pane.as_deref()),
    }
}

/// The bot a run was for: the bot of its pane, else (a skipped run, which has none) the `@handle` its job sends to.
fn handle_of(host: &Host, record: &RunRecord) -> Option<String> {
    let bot = record.pane.as_ref().and_then(|pane| host.view().snapshot.session(pane).and_then(|p| p.bot.clone()));
    if let Some(bot) = bot {
        return Some(bot.handle);
    }
    crate::automation::bot_of_record(record)
}

/// The bots' rows' last reports and the unread count, for the frame model.
pub fn decorate(host: &Host, model: &mut mighty_core::shell::ShellModel) {
    model.bot_reports_unread = with_store(host, |store| (store.reports.iter().filter(|r| unread(r)).count(), false));
    last_reports(host, &mut model.bots);
}

/// A report that waits for the person: not read, and saying more than "nothing to do".
fn unread(report: &Report) -> bool {
    !report.read && report.status != "ok"
}

/// Each bot's last report, as its row shows it (the frame model's and MC-107's phone's).
pub fn last_reports(host: &Host, bots: &mut [mighty_core::shell::SidebarBot]) {
    with_store(host, |store| {
        for bot in bots.iter_mut() {
            bot.report = store
                .reports
                .iter()
                .rev()
                .find(|r| mighty_core::model::PaneBot::key(&r.handle) == mighty_core::model::PaneBot::key(&bot.handle))
                .map(|r| SidebarReport {
                    id: r.id.clone(),
                    status: r.status.clone(),
                    summary: r.summary.clone(),
                    at: r.at.clone(),
                    unread: unread(r),
                });
        }
        ((), false)
    });
}

/// MC-107: the newest reports first, at most `max`, of the bots `shown` takes (the phone sees only its workspaces' bots).
pub fn newest(host: &Host, max: usize, shown: impl Fn(&str) -> bool) -> Vec<Report> {
    with_store(host, |store| (store.reports.iter().rev().filter(|r| shown(&r.handle)).take(max).cloned().collect(), false))
}

/// MC-107: what the reports are, cheaply: how many, the newest and its time, and how many wait unread (the phone reads them
/// again when it moves).
pub fn digest(host: &Host) -> String {
    with_store(host, |store| {
        let last = store.reports.last().map(|r| format!("{}\u{1f}{}\u{1f}{}", r.id, r.at, r.repeat)).unwrap_or_default();
        let read: usize = store.reports.iter().filter(|r| r.read).count();
        (format!("{}\u{1f}{last}\u{1f}{read}", store.reports.len()), false)
    })
}

/// The report `id`, if it is kept.
pub fn find(host: &Host, id: &str) -> Option<Report> {
    with_store(host, |store| (store.reports.iter().find(|r| r.id == id).cloned(), false))
}

/// Read: one report (`id`), or every one when None.
pub fn mark_read(host: &Arc<Host>, id: Option<&str>) {
    with_store(host, |store| {
        for report in store.reports.iter_mut().filter(|r| id.is_none_or(|id| r.id == id)) {
            report.read = true;
        }
        ((), true)
    });
    host.schedule();
}

/// 다시 실행: the run the report `id` came from, fired again (its words when it cannot be).
pub async fn rerun(host: &Arc<Host>, id: &str) -> Result<(), String> {
    let record = find(host, id).and_then(|r| r.record);
    let Some(record) = record else { return Err(host.locale.get("automation.retry.gone")) };
    crate::automation::retry(host, &record).await
}

pub fn register(methods: &mut Registry) {
    // The report box: every report kept, the newest first.
    methods.add("botReports", |host: Arc<Host>, _args| async move {
        let reports = with_store(&host, |store| (store.reports.iter().rev().cloned().collect::<Vec<_>>(), false));
        Ok(json!({ "reports": reports }))
    });
    // Read: one report (`id`), or every one.
    methods.add("botReportRead", |host: Arc<Host>, args| async move {
        mark_read(&host, arg(&args, "id"));
        Ok(Value::Null)
    });
    // 👍 / 👎 (`up`: true, false, or null to take it back).
    methods.add("botReportThumb", |host: Arc<Host>, args| async move {
        let (id, up) = (arg(&args, "id").map(str::to_string), args.get("up").and_then(Value::as_bool));
        with_store(&host, |store| {
            let found = store.reports.iter_mut().find(|r| Some(&r.id) == id.as_ref());
            if let Some(report) = found {
                report.thumb = up;
            }
            ((), true)
        });
        Ok(Value::Null)
    });
    // 다시 실행: the run the report came from, fired again.
    methods.add("botReportRerun", |host: Arc<Host>, args| async move {
        Ok(match rerun(&host, arg(&args, "id").unwrap_or_default()).await {
            Ok(()) => json!({ "ok": true }),
            Err(words) => json!({ "error": words }),
        })
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_answer_makes_a_quiet_report_or_one_with_its_first_sentence_and_links() {
        assert_eq!(from_answer("  ").0, "ok");
        assert_eq!(from_answer("Nothing new today.\n\n**NO_REPLY**").0, "ok");
        let (status, summary, items) = from_answer(
            "## Review\nOne of the three open pull requests has a risky change. More below.\n- #412 double charge on retry https://github.com/o/r/pull/412\n- see (https://github.com/o/r/pull/418).\n- again https://github.com/o/r/pull/412",
        );
        assert_eq!(status, "report");
        assert_eq!(summary, "One of the three open pull requests has a risky change.");
        assert_eq!(items.len(), 2, "each link once: {items:?}");
        assert_eq!(items[0].url.as_deref(), Some("https://github.com/o/r/pull/412"));
        assert_eq!(items[0].text, "#412 double charge on retry");
        assert_eq!(items[1].url.as_deref(), Some("https://github.com/o/r/pull/418"));
    }

    #[test]
    fn a_first_sentence_ending_in_a_cjk_stop_is_cut_after_it() {
        let (_, summary, _) = from_answer("今天检查了三个打开的拉取请求并且没有发现问题。下面是详细内容。");
        assert_eq!(summary, "今天检查了三个打开的拉取请求并且没有发现问题。");
        let (_, summary, _) = from_answer("本日のレビューでは三つのプルリクエストを確認しました。詳細は以下です。");
        assert_eq!(summary, "本日のレビューでは三つのプルリクエストを確認しました。");
    }

    #[test]
    fn the_same_report_again_is_counted_and_a_status_word_is_read() {
        let report = |summary: &str| Report {
            id: "x".into(),
            handle: "콩이".into(),
            pane: None,
            status: "ok".into(),
            summary: summary.into(),
            items: vec![],
            at: String::new(),
            record: None,
            job: None,
            reason: None,
            cost_usd: None,
            seconds: None,
            event_url: None,
            read: false,
            thumb: None,
            repeat: 1,
            run: None,
        };
        assert!(repeats(&report("nothing"), &report("nothing")));
        assert!(!repeats(&report("nothing"), &report("something")));
        assert_eq!((status_of("needs"), status_of("ok"), status_of("whatever")), ("needs", "ok", "report"));
        assert_eq!(cut(&"가".repeat(10), 5), "가가가가…");
    }
}
