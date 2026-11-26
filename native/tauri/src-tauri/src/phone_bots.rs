//! MC-107: the phone's 봇 tab over the m1 protocol (mighty-remote's router: `GET /m1/bots`, the bot actions, `/m1/state`'s
//! `botsRevision`), built from what the desktop's own 봇 tab and 알림 함 read — the sidebar's bots (`mighty_core::shell::sidebar_bots`
//! with their jobs, next run and last report), the bot reports (bot_report.rs) and the approvals waiting (automation/policy.rs) — and
//! acting through the same calls (`botSleep`'s `set_asleep`, 지금 실행's `run_now`, `botReportRead`, `botReportRerun`,
//! `automationApprovalDecide`).
//!
//! - A phone held to some workspaces (휴대폰에서 볼 워크스페이스) sees only their bots, those bots' reports and their triggers'
//!   approvals; an action on anything else is 404, as a pane it does not see is.
//! - A bot's state is the 봇 tab's (`botTab.ts` `botState`): waiting for the person first (the pane's mark, a waiting permission, a
//!   waiting status), then working, asleep, else idle; `detail` is the words the desktop's line says for the first two.
//! - Report items carry their words and http(s) links only, as the report box shows them; no record, run or path travels.
//! - What starts work (지금 실행, 다시 실행, 실행 on an approval) is refused to a phone paired before device ids (the app update's
//!   rule); an approval is decided only from inside the app (its push has no buttons), so never from a locked phone's shade.

use std::collections::HashSet;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;

use mighty_core::model::{AppSnapshot, PaneBot};
use mighty_core::shell::{Attention, SidebarBot, pane_info, sidebar_bots};
use mighty_core::time::DateTimeOffset;
use mighty_remote::phone::desk::{MobileBotRequest, MobileError};
use serde_json::{Map, Value, json};

use crate::automation::policy::Approval;
use crate::bot_report::Report;
use crate::host::Host;

/// The `/m1/info` capability of the bot routes.
pub const CAPABILITY: &str = "bots";
/// The session id of an approval's push and of a report's push without a pane: the phone opens its 봇 tab on them.
pub const APPROVAL_PUSH: &str = "bot-approval";
pub const REPORT_PUSH: &str = "bot-report";
/// The most reports a phone is sent (the newest).
pub const MAX_REPORTS: usize = 50;

/// What a bot's pane says now, for its state and line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PaneFacts {
    /// The pane's tone (`run`, `wait`, `done`, `err`, `stop`, `idle`).
    pub tone: String,
    pub title: String,
    /// The waiting words (`질문 1`) and what it is on while it runs.
    pub wait: Option<String>,
    pub activity: Option<String>,
    /// The desktop's mark on the pane (MC-26): its words.
    pub attention: Option<String>,
    /// A tool permission, question or plan waits on it.
    pub asks: bool,
}

/// One bot as the phone is told of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhoneBot {
    pub bot: SidebarBot,
    pub facts: Option<PaneFacts>,
    /// Its schedules' and triggers' names (지금 실행 picks one when there are several).
    pub jobs: Vec<String>,
}

/// Where a bot stands (`botTab.ts` `botState`): waiting for the person first, then working, asleep, else idle.
pub fn state_of(asleep: bool, facts: Option<&PaneFacts>) -> &'static str {
    match facts {
        Some(f) if f.attention.is_some() || f.asks || f.tone == "wait" => "needsYou",
        Some(f) if f.tone == "run" => "working",
        _ if asleep => "asleep",
        _ => "idle",
    }
}

/// The desktop line's own words for a bot waiting or working (`statusLine`): its mark's or waiting words, what it is on.
fn detail_of(state: &str, facts: Option<&PaneFacts>) -> Option<String> {
    let facts = facts?;
    let words = match state {
        "needsYou" => facts.attention.clone().filter(|t| !t.trim().is_empty()).or_else(|| facts.wait.clone()),
        "working" => facts.activity.clone(),
        _ => None,
    };
    words.filter(|w| !w.trim().is_empty())
}

/// A link a phone may open: http(s) only.
fn link(url: Option<&str>) -> Option<&str> {
    url.filter(|u| u.starts_with("https://") || u.starts_with("http://"))
}

fn put(map: &mut Map<String, Value>, key: &str, value: Option<impl Into<Value>>) {
    if let Some(value) = value {
        map.insert(key.into(), value.into());
    }
}

/// One bot's row: handle, kind and workspace, state and its words, the pane's title, sleep, jobs, next run and last report.
pub fn bot_value(row: &PhoneBot) -> Value {
    let bot = &row.bot;
    let state = state_of(bot.asleep, row.facts.as_ref());
    let mut map = Map::new();
    map.insert("id".into(), json!(bot.id));
    map.insert("handle".into(), json!(bot.handle));
    map.insert("kind".into(), json!(if bot.workspace_id.is_some() { "workspace" } else { "independent" }));
    put(&mut map, "workspace", bot.workspace.clone());
    put(&mut map, "workspaceId", bot.workspace_id.clone());
    map.insert("state".into(), json!(state));
    put(&mut map, "detail", detail_of(state, row.facts.as_ref()));
    put(&mut map, "title", row.facts.as_ref().map(|f| f.title.clone()));
    map.insert("asleep".into(), json!(bot.asleep));
    map.insert("jobs".into(), json!(row.jobs.len()));
    map.insert("jobNames".into(), json!(row.jobs));
    put(&mut map, "nextRun", bot.next_run.clone());
    if let Some(report) = &bot.report {
        map.insert(
            "report".into(),
            json!({ "id": report.id, "status": report.status, "summary": report.summary, "at": report.at, "unread": report.unread }),
        );
    }
    Value::Object(map)
}

/// One report as the phone's 봇 알림 lists it: no record, run or path, only http(s) links.
pub fn report_value(report: &Report) -> Value {
    let items: Vec<Value> = report
        .items
        .iter()
        .map(|item| {
            let mut map = Map::new();
            map.insert("text".into(), json!(item.text));
            put(&mut map, "url", link(item.url.as_deref()));
            Value::Object(map)
        })
        .collect();
    let mut map = Map::new();
    map.insert("id".into(), json!(report.id));
    map.insert("handle".into(), json!(report.handle));
    put(&mut map, "pane", report.pane.clone());
    map.insert("status".into(), json!(report.status));
    map.insert("summary".into(), json!(report.summary));
    map.insert("items".into(), Value::Array(items));
    map.insert("at".into(), json!(report.at));
    put(&mut map, "job", report.job.clone());
    put(&mut map, "reason", report.reason.clone());
    put(&mut map, "eventUrl", link(report.event_url.as_deref()));
    put(&mut map, "costUsd", report.cost_usd);
    put(&mut map, "seconds", report.seconds);
    map.insert("read".into(), json!(report.read));
    map.insert("unread".into(), json!(!report.read && report.status != "ok"));
    map.insert("repeat".into(), json!(report.repeat));
    put(&mut map, "rerun", report.record.as_ref().map(|_| true));
    Value::Object(map)
}

/// One approval waiting: the event (its type, subject, title and link), the rule and its workspace, what will run, when.
pub fn approval_value(approval: &Approval) -> Value {
    let mut map = Map::new();
    map.insert("id".into(), json!(approval.id));
    map.insert("rule".into(), json!(approval.name));
    put(&mut map, "workspace", Some(approval.workspace.clone()).filter(|w| !w.is_empty()));
    map.insert("type".into(), json!(approval.event.kind));
    map.insert("subject".into(), json!(approval.event.subject));
    put(&mut map, "title", Some(approval.event.title.clone()).filter(|t| !t.is_empty()));
    put(&mut map, "url", link(approval.event.url.as_deref()));
    map.insert("what".into(), json!(approval.what));
    map.insert("at".into(), json!(approval.at));
    Value::Object(map)
}

/// `GET /m1/bots`: the bots (the sidebar's order: the independent ones, then each workspace's), the newest reports and the approvals
/// waiting (the oldest first), and how many reports wait unread.
pub fn payload(bots: &[PhoneBot], reports: &[Report], approvals: &[Approval]) -> Value {
    json!({
        "bots": bots.iter().map(bot_value).collect::<Vec<_>>(),
        "reports": reports.iter().map(report_value).collect::<Vec<_>>(),
        "approvals": approvals.iter().map(approval_value).collect::<Vec<_>>(),
        "unread": reports.iter().filter(|r| !r.read && r.status != "ok").count(),
    })
}

/// What the payload is read from, as a short key (no next run: it moves only with a fire, which moves the pane, or a job's change).
pub fn digest(bots: &[PhoneBot], reports: &str, approvals: &[String]) -> String {
    let mut hash = DefaultHasher::new();
    for row in bots {
        let state = state_of(row.bot.asleep, row.facts.as_ref());
        (&row.bot.id, &row.bot.handle, &row.bot.workspace, row.bot.asleep, state).hash(&mut hash);
        (detail_of(state, row.facts.as_ref()), row.facts.as_ref().map(|f| &f.title), &row.jobs).hash(&mut hash);
    }
    reports.hash(&mut hash);
    approvals.hash(&mut hash);
    format!("{:016x}", hash.finish())
}

// ── the actions ────────────────────────────────────────────────────────────────────────────────────────────────────────

/// What a phone asked of the bots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BotAction {
    /// 지금 실행: one of the bot's jobs (by name; the only one when None).
    Run {
        handle: String,
        job: Option<String>,
    },
    /// 재우기 (`true`) / 깨우기.
    Sleep {
        handle: String,
        asleep: bool,
    },
    ReportRead(String),
    ReportRerun(String),
    /// 실행 (`true`) / 건너뛰기 on an approval.
    Decide {
        id: String,
        run: bool,
    },
}

impl BotAction {
    /// Whether it starts work (refused to a phone paired before device ids, as the app update is).
    fn starts_work(&self) -> bool {
        matches!(self, BotAction::Run { .. } | BotAction::ReportRerun(_) | BotAction::Decide { run: true, .. })
    }
}

fn refusal(status: u16, message: impl Into<String>) -> MobileError {
    MobileError::request(status, message, None)
}

fn unknown() -> MobileError {
    refusal(404, "Mobile route not found.")
}

/// The route and body read: `bots/{handle}/run|sleep|wake`, `bot-reports/{id}/read|rerun`, `bot-approvals/{id}/approve|skip`; only
/// `run` takes a body member (`job`, a name), every other member is refused.
pub fn parse(route: &[String], body: &Map<String, Value>) -> Result<BotAction, MobileError> {
    let [scope, id, verb] = route else { return Err(unknown()) };
    let id = id.trim();
    if id.is_empty() {
        return Err(unknown());
    }
    let job = match body.get("job") {
        None | Some(Value::Null) => None,
        Some(Value::String(name)) if !name.trim().is_empty() => Some(name.trim().to_string()),
        Some(_) => return Err(refusal(400, "Invalid request body.")),
    };
    let takes_job = scope == "bots" && verb == "run";
    if body.keys().any(|k| !(takes_job && k == "job")) {
        return Err(refusal(400, "Invalid request body."));
    }
    let id = id.to_string();
    Ok(match (scope.as_str(), verb.as_str()) {
        ("bots", "run") => BotAction::Run { handle: id, job },
        ("bots", "sleep") => BotAction::Sleep { handle: id, asleep: true },
        ("bots", "wake") => BotAction::Sleep { handle: id, asleep: false },
        ("bot-reports", "read") => BotAction::ReportRead(id),
        ("bot-reports", "rerun") => BotAction::ReportRerun(id),
        ("bot-approvals", "approve") => BotAction::Decide { id, run: true },
        ("bot-approvals", "skip") => BotAction::Decide { id, run: false },
        _ => return Err(unknown()),
    })
}

/// What the phone sees, as an action is checked against it.
#[derive(Debug, Clone, Default)]
pub struct Seen {
    /// Each bot's handle, pane and job names.
    pub bots: Vec<(String, String, Vec<String>)>,
    /// Each report's id and whether it can run again.
    pub reports: Vec<(String, bool)>,
    pub approvals: Vec<String>,
}

/// What an allowed action works on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// The bot's pane and, for 지금 실행, the job's name.
    Bot {
        pane: String,
        job: Option<String>,
    },
    Report(String),
    Approval(String),
}

/// The checks every bot action passes before it runs, by the desktop's rules: a device paired before device ids may not start work
/// (403 `legacyClient`); a bot, report or approval the phone does not see is 404; 지금 실행 needs a job of the bot's (one named, or its
/// only one: none or several without a name is 409); a report with no run cannot run again (409).
pub fn check(action: &BotAction, seen: &Seen, device: &str, l: &mighty_core::locale::Locale) -> Result<Target, MobileError> {
    if action.starts_work() && device == mighty_platform::pairing::identity_file::LEGACY {
        return Err(MobileError::request(403, l.get("phone.screenShare.refusal.legacyClient"), Some("legacyClient")));
    }
    let bot = |handle: &str| {
        seen.bots
            .iter()
            .find(|(h, _, _)| PaneBot::key(h) == PaneBot::key(handle))
            .ok_or_else(|| refusal(404, l.get_with("remote.error.botNotFound", &[("handle", handle)])))
    };
    match action {
        BotAction::Run { handle, job } => {
            let (shown, pane, jobs) = bot(handle)?;
            let chosen = match job {
                Some(name) => jobs.iter().find(|j| *j == name),
                None if jobs.len() == 1 => jobs.first(),
                None if jobs.is_empty() => None,
                None => {
                    return Err(refusal(409, l.get_with("remote.error.botWhichJob", &[("handle", shown), ("jobs", &jobs.join(", "))])));
                }
            };
            let chosen = chosen.ok_or_else(|| refusal(409, l.get_with("remote.error.botNoJob", &[("handle", shown)])))?;
            Ok(Target::Bot { pane: pane.clone(), job: Some(chosen.clone()) })
        }
        BotAction::Sleep { handle, .. } => bot(handle).map(|(_, pane, _)| Target::Bot { pane: pane.clone(), job: None }),
        BotAction::ReportRead(id) | BotAction::ReportRerun(id) => {
            let (_, reruns) =
                seen.reports.iter().find(|(r, _)| r == id).ok_or_else(|| refusal(404, l.get("remote.error.reportNotFound")))?;
            if matches!(action, BotAction::ReportRerun(_)) && !reruns {
                return Err(refusal(409, l.get("automation.retry.gone")));
            }
            Ok(Target::Report(id.clone()))
        }
        BotAction::Decide { id, .. } => seen
            .approvals
            .iter()
            .find(|a| *a == id)
            .map(|a| Target::Approval(a.clone()))
            .ok_or_else(|| refusal(404, l.get("automation.policy.approvalGone"))),
    }
}

// ── the desk's side (reads the app) ────────────────────────────────────────────────────────────────────────────────────

/// Whether a workspace is one the phone sees.
fn sees(allowed: Option<&[String]>, workspace: &str) -> bool {
    allowed.is_none_or(|ids| ids.iter().any(|id| id == workspace))
}

/// What a bot's pane says now.
fn facts(host: &Host, snapshot: &AppSnapshot, pane: &str, now: DateTimeOffset) -> Option<PaneFacts> {
    let session = snapshot.session(pane)?;
    let info = pane_info(&host.locale, session, Attention::default(), now);
    let asks = host.runs().is_some_and(|r| r.service.pending_permissions(pane).iter().any(|p| p.state == "pending"))
        || host.runs().is_some_and(|r| r.service.antigravity_card(pane).is_some());
    Some(PaneFacts {
        tone: info.tone.to_string(),
        title: info.title,
        wait: info.wait,
        activity: info.activity,
        attention: crate::attention::phone(pane).map(|(_, text, _)| text),
        asks,
    })
}

/// The bots the phone sees, with their pane's facts and job names; `next` fills their next run (left out of the digest).
fn bots(host: &Host, snapshot: &AppSnapshot, allowed: Option<&[String]>, next: bool) -> Vec<PhoneBot> {
    let now = DateTimeOffset::now();
    let jobs = crate::automation::jobs();
    let mut rows: Vec<SidebarBot> = sidebar_bots(snapshot)
        .into_iter()
        .filter(|bot| snapshot.session(&bot.id).is_some_and(|pane| sees(allowed, pane.workspace_id())))
        .collect();
    crate::bot_report::last_reports(host, &mut rows);
    rows.into_iter()
        .map(|mut bot| {
            let key = PaneBot::key(&bot.handle);
            let names: Vec<String> =
                jobs.iter().filter(|j| j.action().bot().is_some_and(|b| PaneBot::key(&b) == key)).map(|j| j.name().to_string()).collect();
            bot.jobs = names.len();
            if next {
                bot.next_run = crate::automation::next_fire_of_bot(&bot.handle).map(|at| at.to_rfc3339());
            }
            let facts = facts(host, snapshot, &bot.id, now);
            PhoneBot { bot, facts, jobs: names }
        })
        .collect()
}

/// The approvals of triggers in the workspaces the phone sees (one whose trigger is gone only when it sees every workspace).
fn approvals(host: &Arc<Host>, allowed: Option<&[String]>) -> Vec<Approval> {
    crate::automation::policy::approvals(host)
        .into_iter()
        .filter(|a| match crate::automation::job(&a.key) {
            Some(job) => sees(allowed, &job.workspace),
            None => allowed.is_none(),
        })
        .collect()
}

/// The handles of `bots`, by their key.
fn handles(bots: &[PhoneBot]) -> HashSet<String> {
    bots.iter().map(|b| PaneBot::key(&b.bot.handle)).collect()
}

/// The reports of the bots the phone sees (every report when it sees every workspace, a bot since gone included).
fn reports(host: &Host, bots: &[PhoneBot], allowed: Option<&[String]>, max: usize) -> Vec<Report> {
    let shown = handles(bots);
    crate::bot_report::newest(host, max, |handle| allowed.is_none() || shown.contains(&PaneBot::key(handle)))
}

/// [`mighty_remote::phone::desk::PhoneDesk::bots`].
pub fn read(host: &Arc<Host>, snapshot: &AppSnapshot, allowed: Option<&[String]>) -> Value {
    let bots = bots(host, snapshot, allowed, true);
    let reports = reports(host, &bots, allowed, MAX_REPORTS);
    payload(&bots, &reports, &approvals(host, allowed))
}

/// [`mighty_remote::phone::desk::PhoneDesk::bots_digest`].
pub fn read_digest(host: &Arc<Host>, snapshot: &AppSnapshot, allowed: Option<&[String]>) -> String {
    let bots = bots(host, snapshot, allowed, false);
    let approvals: Vec<String> = approvals(host, allowed).into_iter().map(|a| a.id).collect();
    digest(&bots, &crate::bot_report::digest(host), &approvals)
}

/// [`mighty_remote::phone::desk::PhoneDesk::bot_action`]: read, checked against what the phone sees, then done through the desktop's
/// own calls. Its answer: `ok`, and the desktop's words (`note`) when it has some.
pub async fn act(host: &Arc<Host>, snapshot: &AppSnapshot, request: MobileBotRequest) -> Result<Value, MobileError> {
    let l = host.locale;
    let action = parse(&request.route, &request.body)?;
    let allowed = request.allowed.as_deref();
    let shown = bots(host, snapshot, allowed, false);
    let seen = Seen {
        bots: shown.iter().map(|b| (b.bot.handle.clone(), b.bot.id.clone(), b.jobs.clone())).collect(),
        reports: reports(host, &shown, allowed, usize::MAX).into_iter().map(|r| (r.id, r.record.is_some())).collect(),
        approvals: approvals(host, allowed).into_iter().map(|a| a.id).collect(),
    };
    let target = check(&action, &seen, &request.device, &l)?;
    let busy = |words: String| refusal(409, words);
    let note = match (&action, target) {
        (BotAction::Run { handle, .. }, Target::Bot { job: Some(name), .. }) => {
            let key = crate::automation::jobs_of_bot(handle).into_iter().find(|j| j.name() == name).map(|j| j.key);
            let key = key.ok_or_else(|| busy(l.get("automation.retry.gone")))?;
            crate::automation::run_now(host, &key).await.map_err(busy)?;
            host.log.line(&format!("phone: @{handle} ran '{name}' now"));
            None
        }
        (BotAction::Sleep { asleep, .. }, Target::Bot { pane, .. }) => {
            crate::bot::set_asleep(host, &pane, *asleep).await;
            None
        }
        (BotAction::ReportRead(_), Target::Report(id)) => {
            crate::bot_report::mark_read(host, Some(&id));
            None
        }
        (BotAction::ReportRerun(_), Target::Report(id)) => {
            crate::bot_report::rerun(host, &id).await.map_err(busy)?;
            None
        }
        (BotAction::Decide { run, .. }, Target::Approval(id)) => {
            host.log.line(&format!("phone: approval {id} {}", if *run { "run" } else { "skipped" }));
            Some(crate::automation::policy::decide(host, &id, *run).map_err(busy)?)
        }
        _ => return Err(unknown()),
    };
    host.schedule();
    let mut answer = Map::new();
    answer.insert("ok".into(), json!(true));
    put(&mut answer, "note", note);
    Ok(Value::Object(answer))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot_report::ReportItem;
    use mighty_core::automation::watch::TriggerEvent;
    use mighty_core::shell::SidebarReport;

    fn en() -> mighty_core::locale::Locale {
        mighty_core::locale::Locale::new("en")
    }

    fn bot(id: &str, handle: &str, workspace: Option<&str>) -> SidebarBot {
        SidebarBot {
            id: id.into(),
            handle: handle.into(),
            workspace: workspace.map(|_| "web".to_string()),
            workspace_id: workspace.map(str::to_string),
            folder: None,
            asleep: false,
            jobs: 0,
            next_run: None,
            instructed: true,
            report: None,
            watches: Vec::new(),
        }
    }

    fn facts(tone: &str) -> PaneFacts {
        PaneFacts { tone: tone.into(), title: "Reviewer".into(), ..PaneFacts::default() }
    }

    fn report(id: &str, status: &str, read: bool) -> Report {
        Report {
            id: id.into(),
            handle: "리뷰어".into(),
            pane: Some("p1".into()),
            status: status.into(),
            summary: "One risky PR.".into(),
            items: vec![
                ReportItem { text: "#412".into(), url: Some("https://github.com/o/r/pull/412".into()) },
                ReportItem { text: "local".into(), url: Some("file:///Users/me/secret.txt".into()) },
            ],
            at: "2026-11-26T09:00:00Z".into(),
            record: Some("rec-1".into()),
            job: Some("PR review".into()),
            reason: None,
            cost_usd: Some(0.12),
            seconds: Some(40),
            event_url: Some("javascript:alert(1)".into()),
            read,
            thumb: Some(true),
            repeat: 1,
            run: Some("run-9".into()),
        }
    }

    fn approval(id: &str) -> Approval {
        Approval {
            id: id.into(),
            key: "ws:app:t:PR review".into(),
            name: "PR review".into(),
            workspace: "web".into(),
            inbox: "inbox-1".into(),
            event: TriggerEvent {
                kind: "pull_request.opened".into(),
                subject: "o/r#412".into(),
                title: "Fix double charge".into(),
                url: Some("https://github.com/o/r/pull/412".into()),
                ..TriggerEvent::default()
            },
            what: "Send a request to @리뷰어".into(),
            at: "2026-11-26T09:00:00Z".into(),
        }
    }

    /// The 봇 tab's order of states: waiting for the person whatever else is true, then working, asleep, else idle.
    #[test]
    fn a_bots_state_follows_the_desktops_bot_tab() {
        assert_eq!(state_of(false, None), "idle");
        assert_eq!(state_of(true, None), "asleep");
        assert_eq!(state_of(true, Some(&facts("idle"))), "asleep");
        assert_eq!(state_of(true, Some(&facts("run"))), "working", "a sleeping bot still at work shows working");
        assert_eq!(state_of(false, Some(&facts("wait"))), "needsYou");
        let marked = PaneFacts { attention: Some("Approve the deploy?".into()), ..facts("run") };
        assert_eq!(state_of(true, Some(&marked)), "needsYou", "the mark wins over working and sleep");
        assert_eq!(state_of(false, Some(&PaneFacts { asks: true, ..facts("idle") })), "needsYou");
        assert_eq!(detail_of("needsYou", Some(&marked)).as_deref(), Some("Approve the deploy?"));
        let working = PaneFacts { activity: Some("Reading src/".into()), ..facts("run") };
        assert_eq!(detail_of("working", Some(&working)).as_deref(), Some("Reading src/"));
        assert_eq!(detail_of("idle", Some(&working)), None);
    }

    /// The payload: each bot's kind, state, jobs and last report; reports without record or run, only http(s) links; approvals with
    /// the event, rule, what and when; the unread count.
    #[test]
    fn the_payload_carries_bots_reports_and_approvals_without_secrets() {
        let mut independent = bot("p1", "리뷰어", None);
        independent.next_run = Some("2026-11-27T00:00:00Z".into());
        independent.report = Some(SidebarReport {
            id: "r1".into(),
            status: "needs".into(),
            summary: "One risky PR.".into(),
            at: "2026-11-26T09:00:00Z".into(),
            unread: true,
        });
        let mut sleeping = bot("p2", "문서", Some("ws-1"));
        sleeping.asleep = true;
        let rows = vec![
            PhoneBot { bot: independent, facts: Some(facts("run")), jobs: vec!["PR review".into(), "Daily".into()] },
            PhoneBot { bot: sleeping, facts: Some(facts("idle")), jobs: vec![] },
        ];
        let value = payload(&rows, &[report("r1", "needs", false), report("r0", "ok", false)], &[approval("a1")]);
        let bots = value["bots"].as_array().unwrap();
        assert_eq!(bots[0]["kind"], "independent");
        assert_eq!(bots[0]["state"], "working");
        assert_eq!(bots[0]["jobs"], 2);
        assert_eq!(bots[0]["jobNames"], json!(["PR review", "Daily"]));
        assert_eq!(bots[0]["nextRun"], "2026-11-27T00:00:00Z");
        assert_eq!(bots[0]["report"]["unread"], true);
        assert!(bots[0].get("workspace").is_none());
        assert_eq!((&bots[1]["kind"], &bots[1]["workspace"], &bots[1]["state"]), (&json!("workspace"), &json!("web"), &json!("asleep")));
        let first = &value["reports"][0];
        assert_eq!(first["items"][0]["url"], "https://github.com/o/r/pull/412");
        assert!(first["items"][1].get("url").is_none(), "a file link stays home: {first}");
        assert!(first.get("eventUrl").is_none(), "only http(s) links travel");
        for kept in ["record", "run", "thumb"] {
            assert!(first.get(kept).is_none(), "{kept} stays home");
        }
        assert_eq!((&first["unread"], &first["rerun"]), (&json!(true), &json!(true)));
        assert_eq!(value["reports"][1]["unread"], false, "a quiet report never waits");
        assert_eq!(value["unread"], 1);
        let waiting = &value["approvals"][0];
        assert_eq!(waiting["rule"], "PR review");
        assert_eq!(waiting["type"], "pull_request.opened");
        assert_eq!(waiting["title"], "Fix double charge");
        assert_eq!(waiting["what"], "Send a request to @리뷰어");
        assert!(waiting.get("inbox").is_none() && waiting.get("key").is_none());
    }

    /// The digest moves with a bot's state, sleep, jobs, the reports and the approvals, and stays put otherwise.
    #[test]
    fn the_digest_moves_only_with_what_the_phone_shows() {
        let row = |tone: &str, asleep: bool| {
            let mut b = bot("p1", "리뷰어", None);
            b.asleep = asleep;
            PhoneBot { bot: b, facts: Some(facts(tone)), jobs: vec!["Daily".into()] }
        };
        let base = digest(&[row("idle", false)], "1", &[]);
        assert_eq!(base, digest(&[row("idle", false)], "1", &[]));
        assert_ne!(base, digest(&[row("run", false)], "1", &[]));
        assert_ne!(base, digest(&[row("idle", true)], "1", &[]));
        assert_ne!(base, digest(&[row("idle", false)], "2", &[]));
        assert_ne!(base, digest(&[row("idle", false)], "1", &["a1".into()]));
    }

    fn route(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|p| p.to_string()).collect()
    }

    fn body(value: Value) -> Map<String, Value> {
        value.as_object().cloned().unwrap()
    }

    fn status(result: Result<impl std::fmt::Debug, MobileError>) -> u16 {
        match result {
            Err(MobileError::Request { status, .. }) => status,
            other => panic!("not refused: {other:?}"),
        }
    }

    /// Each route's verb, and the refusals: an unknown scope or verb 404, a body member other than `run`'s `job` 400.
    #[test]
    fn a_bot_route_is_read_and_its_body_checked() {
        let none = Map::new();
        assert_eq!(
            parse(&route(&["bots", "리뷰어", "run"]), &body(json!({"job": "Daily"}))).unwrap(),
            BotAction::Run { handle: "리뷰어".into(), job: Some("Daily".into()) }
        );
        assert_eq!(parse(&route(&["bots", "리뷰어", "run"]), &none).unwrap(), BotAction::Run { handle: "리뷰어".into(), job: None });
        assert_eq!(parse(&route(&["bots", "x", "sleep"]), &none).unwrap(), BotAction::Sleep { handle: "x".into(), asleep: true });
        assert_eq!(parse(&route(&["bots", "x", "wake"]), &none).unwrap(), BotAction::Sleep { handle: "x".into(), asleep: false });
        assert_eq!(parse(&route(&["bot-reports", "r1", "read"]), &none).unwrap(), BotAction::ReportRead("r1".into()));
        assert_eq!(parse(&route(&["bot-reports", "r1", "rerun"]), &none).unwrap(), BotAction::ReportRerun("r1".into()));
        assert_eq!(parse(&route(&["bot-approvals", "a1", "approve"]), &none).unwrap(), BotAction::Decide { id: "a1".into(), run: true });
        assert_eq!(parse(&route(&["bot-approvals", "a1", "skip"]), &none).unwrap(), BotAction::Decide { id: "a1".into(), run: false });
        assert_eq!(status(parse(&route(&["bots", "x", "delete"]), &none)), 404);
        assert_eq!(status(parse(&route(&["bot-approvals", "a1", "run"]), &none)), 404);
        assert_eq!(status(parse(&route(&["bots", " ", "run"]), &none)), 404);
        assert_eq!(status(parse(&route(&["bots", "x"]), &none)), 404);
        assert_eq!(status(parse(&route(&["bots", "x", "run"]), &body(json!({"job": 3})))), 400);
        assert_eq!(status(parse(&route(&["bots", "x", "sleep"]), &body(json!({"job": "Daily"})))), 400);
        assert_eq!(status(parse(&route(&["bot-approvals", "a1", "approve"]), &body(json!({"run": true})))), 400);
    }

    /// The checks: what the phone does not see is 404, 지금 실행 picks the named or only job (none or several unnamed 409), a report
    /// with no run cannot run again, and a phone paired before device ids may not start work but may skip, read and sleep.
    #[test]
    fn a_bot_action_is_checked_against_what_the_phone_sees() {
        let l = en();
        let seen = Seen {
            bots: vec![
                ("리뷰어".into(), "p1".into(), vec!["Daily".into()]),
                ("문서".into(), "p2".into(), vec!["A".into(), "B".into()]),
                ("빈".into(), "p3".into(), vec![]),
            ],
            reports: vec![("r1".into(), true), ("r2".into(), false)],
            approvals: vec!["a1".into()],
        };
        let device = "phone-1";
        let run = |handle: &str, job: Option<&str>| BotAction::Run { handle: handle.into(), job: job.map(str::to_string) };
        assert_eq!(check(&run("리뷰어", None), &seen, device, &l).unwrap(), Target::Bot { pane: "p1".into(), job: Some("Daily".into()) });
        assert_eq!(check(&run("@리뷰어", None), &seen, device, &l).unwrap(), Target::Bot { pane: "p1".into(), job: Some("Daily".into()) });
        assert_eq!(check(&run("문서", Some("B")), &seen, device, &l).unwrap(), Target::Bot { pane: "p2".into(), job: Some("B".into()) });
        assert_eq!(status(check(&run("문서", None), &seen, device, &l)), 409, "several jobs need a name");
        assert_eq!(status(check(&run("문서", Some("C")), &seen, device, &l)), 409, "a name the bot has not");
        assert_eq!(status(check(&run("빈", None), &seen, device, &l)), 409, "no job to run");
        assert_eq!(status(check(&run("남의봇", None), &seen, device, &l)), 404, "a bot the phone does not see");
        let sleep = BotAction::Sleep { handle: "빈".into(), asleep: true };
        assert_eq!(check(&sleep, &seen, device, &l).unwrap(), Target::Bot { pane: "p3".into(), job: None });
        assert_eq!(check(&BotAction::ReportRead("r2".into()), &seen, device, &l).unwrap(), Target::Report("r2".into()));
        assert_eq!(status(check(&BotAction::ReportRerun("r2".into()), &seen, device, &l)), 409, "no run to fire again");
        assert_eq!(status(check(&BotAction::ReportRead("r9".into()), &seen, device, &l)), 404);
        let approve = BotAction::Decide { id: "a1".into(), run: true };
        let skip = BotAction::Decide { id: "a1".into(), run: false };
        assert_eq!(check(&approve, &seen, device, &l).unwrap(), Target::Approval("a1".into()));
        assert_eq!(status(check(&BotAction::Decide { id: "a9".into(), run: true }, &seen, device, &l)), 404, "gone or not seen");
        let legacy = mighty_platform::pairing::identity_file::LEGACY;
        for starts in [run("리뷰어", None), BotAction::ReportRerun("r1".into()), approve] {
            match check(&starts, &seen, legacy, &l) {
                Err(MobileError::Request { status: 403, code, .. }) => assert_eq!(code.as_deref(), Some("legacyClient")),
                other => panic!("{starts:?}: {other:?}"),
            }
        }
        for quiet in [skip, sleep, BotAction::ReportRead("r1".into())] {
            assert!(check(&quiet, &seen, legacy, &l).is_ok(), "{quiet:?} starts nothing");
        }
    }
}
