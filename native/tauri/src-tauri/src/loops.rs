//! MC-118: the loop skeleton of 0.7.0 (docs/loops-contract.md): a pane driven toward a goal until its criteria pass.
//!
//! - The loop's model, guard, prompts and driver are mighty-delegation's [`loops`](mighty_delegation::loops); this module keeps the
//!   records (`<data>/loops.json`, the newest 50, with the switch `enabled`; each loop's guard baseline in `<data>/loop-guards/<id>.json`),
//!   answers the page's methods (`loopList`, `loopDraft`, `loopStart`, `loopPause`, `loopResume`, `loopStop`, `loopForget`,
//!   `loopSetEnabled`), pushes `loops` on every change, and implements the driver's [`LoopHost`]: a turn is sent with the phone's
//!   `control_send` (an ordinary request of the pane), its end comes from runs.rs ([`on_run_finished`]), the checks run through the
//!   verification's command runner and browser check (verify/mod.rs) in the pane's run folder.
//! - A pane with a running or paused loop gets no turn-end verification of its own ([`occupies`]).
//! - A loop that was running when the app quit comes back paused.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use mighty_core::time;
use mighty_delegation::loops::{
    self, Changes, CheckKind, CheckResult, Control, Criterion, Guard, GuardBaseline, Limits, LoopHost, LoopRecord, LoopState, LoopStop,
    PromptWords, Start, TurnOutcome,
};
use mighty_delegation::mighty_yaml::VerifyCommandSpec;
use mighty_delegation::verify::{self, CommandResult, RoundResult};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::oneshot;

use crate::host::{Host, Registry, lock};
use crate::methods::text;
use crate::verify::Context;

// ── the records ──────────────────────────────────────────────────────────────────────────────────────────────────────────

#[derive(Default, Serialize, Deserialize)]
struct Store {
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    loops: Vec<LoopRecord>,
}

static STORE: LazyLock<Mutex<Option<Store>>> = LazyLock::new(Default::default);

/// A loop the app is driving now.
struct Live {
    pane: String,
    control: Control,
    /// The turn being waited for: told how it ended.
    waiting: Option<oneshot::Sender<TurnOutcome>>,
}

static LIVE: LazyLock<Mutex<HashMap<String, Live>>> = LazyLock::new(Default::default);

fn path(host: &Host) -> PathBuf {
    host.data_dir.join("loops.json")
}

fn guard_path(host: &Host, id: &str) -> PathBuf {
    host.data_dir.join("loop-guards").join(format!("{id}.json"))
}

/// Runs `edit` over the store (read from disk the first time, a running loop read back paused) and saves it when it says so.
fn with_store<T>(host: &Host, edit: impl FnOnce(&mut Store) -> (T, bool)) -> T {
    let mut slot = lock(&STORE);
    let store = slot.get_or_insert_with(|| {
        let mut store: Store = std::fs::read(path(host)).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default();
        store.loops = std::mem::take(&mut store.loops).into_iter().map(LoopRecord::after_restart).collect();
        store
    });
    let (out, changed) = edit(store);
    if changed && let Ok(bytes) = serde_json::to_vec(store) {
        let _ = mighty_core::automation::write_atomic(&path(host), &bytes);
    }
    out
}

pub fn enabled(host: &Host) -> bool {
    with_store(host, |s| (s.enabled, false))
}

/// Settings › 에이전트 › 루프 (베타).
pub fn set_enabled(host: &Arc<Host>, on: bool) {
    with_store(host, |s| {
        let changed = s.enabled != on;
        s.enabled = on;
        ((), changed)
    });
    push(host);
}

fn find(host: &Host, id: &str) -> Option<LoopRecord> {
    with_store(host, |s| (s.loops.iter().find(|l| l.id == id).cloned(), false))
}

/// Keeps `record` (replacing the one of its id, else first) and tells the page. Past [`loops::MAXIMUM_LOOPS_KEPT`] the oldest finished
/// ones go.
fn save(host: &Arc<Host>, record: &LoopRecord) {
    let dropped = with_store(host, |s| {
        match s.loops.iter_mut().find(|l| l.id == record.id) {
            Some(slot) => *slot = record.clone(),
            None => s.loops.insert(0, record.clone()),
        }
        let mut dropped = Vec::new();
        while s.loops.len() > loops::MAXIMUM_LOOPS_KEPT {
            let Some(index) = s.loops.iter().rposition(|l| l.state.finished()) else { break };
            dropped.push(s.loops.remove(index).id);
        }
        (dropped, true)
    });
    for id in dropped {
        let _ = std::fs::remove_file(guard_path(host, &id));
    }
    push(host);
}

fn list_json(host: &Host) -> Value {
    with_store(host, |s| (json!({ "enabled": s.enabled, "loops": s.loops }), false))
}

fn push(host: &Host) {
    let mut message = list_json(host);
    message["kind"] = json!("loops");
    host.push(&host.view(), "loops", message.to_string());
}

/// Whether `pane` has a loop running or paused: its turns are the loop's, not the verification gate's.
pub fn occupies(pane: &str) -> bool {
    if lock(&LIVE).values().any(|l| l.pane == pane) {
        return true;
    }
    lock(&STORE)
        .as_ref()
        .is_some_and(|s| s.loops.iter().any(|l| l.pane_id == pane && matches!(l.state, LoopState::Running | LoopState::Paused)))
}

// ── a turn's end ─────────────────────────────────────────────────────────────────────────────────────────────────────────

/// A pane's run ended as `ended` (runs.rs, with the cost the usage ledger gave it): the loop waiting for the turn is told. True when
/// the pane has a loop (its turn-end verification is then skipped).
pub fn on_run_finished(host: &Arc<Host>, pane: &str, ended: &str, cost: Option<f64>) -> bool {
    let sender = {
        let mut live = lock(&LIVE);
        let Some(entry) = live.values_mut().find(|l| l.pane == pane) else { return occupies(pane) };
        entry.waiting.take()
    };
    if let Some(sender) = sender {
        let run_id = newest_run(host, pane);
        let _ = sender.send(match ended {
            "completed" => TurnOutcome::Done { run_id, cost_usd: cost },
            "stopped" => TurnOutcome::Person,
            _ => TurnOutcome::Ended { run_id, cost_usd: cost },
        });
    }
    true
}

fn newest_run(host: &Host, pane: &str) -> Option<String> {
    let view = host.view();
    let session = view.snapshot.session(pane)?;
    mighty_graph::support::newest_request_run(session.graph_runs()).and_then(|r| r.id.clone())
}

fn current_submission(host: &Host, pane: &str) -> Option<u64> {
    host.view().panes.get(pane).map(|ui| ui.submission)
}

// ── the driver's host ────────────────────────────────────────────────────────────────────────────────────────────────────

struct AppLoopHost {
    host: Arc<Host>,
    ctx: Context,
    id: String,
    /// The pane's send counter as the loop last left it (after its own request): anything past it is the person's.
    submission: u64,
    baseline: GuardBaseline,
    started: Instant,
    /// Minutes the loop had run before this drive (a resumed loop's).
    before_minutes: f64,
}

impl AppLoopHost {
    fn control_now(&self) -> Control {
        lock(&LIVE).get(&self.id).map_or(Control::Stop, |l| l.control)
    }
}

impl LoopHost for AppLoopHost {
    async fn turn(&mut self, prompt: String) -> TurnOutcome {
        let (sender, mut receiver) = oneshot::channel();
        if let Some(live) = lock(&LIVE).get_mut(&self.id) {
            live.waiting = Some(sender);
        }
        let host = self.host.clone();
        let pane = self.ctx.pane.clone();
        if crate::phone::control_send(&host, &pane, &prompt).await.is_err() {
            if let Some(live) = lock(&LIVE).get_mut(&self.id) {
                live.waiting = None;
            }
            return TurnOutcome::Ended { run_id: None, cost_usd: None };
        }
        self.submission = current_submission(&host, &pane).unwrap_or(self.submission);
        let outcome = loop {
            tokio::select! {
                outcome = &mut receiver => break outcome.unwrap_or(TurnOutcome::Ended { run_id: None, cost_usd: None }),
                () = tokio::time::sleep(Duration::from_secs(5)) => {
                    if !crate::panes::alive(&host.view(), &pane) {
                        break TurnOutcome::PaneClosed;
                    }
                    if host.closing() {
                        if let Some(live) = lock(&LIVE).get_mut(&self.id) {
                            live.control = Control::Pause;
                        }
                        break TurnOutcome::Interrupted;
                    }
                    if self.control_now() == Control::Stop {
                        break TurnOutcome::Interrupted;
                    }
                }
            }
        };
        if let Some(live) = lock(&LIVE).get_mut(&self.id) {
            live.waiting = None;
        }
        // The person sent something while the turn ran: the loop is theirs to stop.
        if matches!(outcome, TurnOutcome::Done { .. })
            && (current_submission(&host, &pane) != Some(self.submission)
                || host.view().panes.get(&pane).is_some_and(|ui| !ui.queue.items().is_empty()))
        {
            return TurnOutcome::Person;
        }
        outcome
    }

    async fn check(&mut self, criteria: &[Criterion]) -> Vec<CheckResult> {
        let secrets: Vec<String> = self.ctx.config.config.worktree.env.values().cloned().collect();
        let l = self.host.locale;
        let mut out = Vec::with_capacity(criteria.len());
        for c in criteria {
            if self.control_now() == Control::Stop {
                break;
            }
            let began = Instant::now();
            let name = c.check.name.clone().unwrap_or_else(|| c.id.clone());
            let result = match c.check.kind {
                CheckKind::Checklist => CheckResult {
                    id: c.id.clone(),
                    passed: None,
                    commands: Vec::new(),
                    browser: None,
                    summary: String::new(),
                    duration_ms: 0,
                },
                CheckKind::Command => {
                    let spec = VerifyCommandSpec {
                        command: c.check.command.clone().unwrap_or_default(),
                        cwd: c.check.cwd.clone(),
                        timeout_seconds: None,
                    };
                    let command = crate::verify::run_command(&self.host, &self.ctx, &name, &spec, &secrets).await;
                    command_result(&c.id, command, &l)
                }
                CheckKind::VerifyConfig if name == loops::BROWSER_CHECK => {
                    match self.ctx.config.config.verify.browser.clone().filter(|_| self.ctx.approved) {
                        Some(spec) => {
                            let prefix = format!("loop-{}-{}", self.id, c.id);
                            let report = crate::verify::browser_step(&self.host, &self.ctx, &spec, &prefix).await;
                            browser_result(&c.id, report, began, &l)
                        }
                        None => refused(&c.id, &name, l.get("loop.host.check.notApproved")),
                    }
                }
                CheckKind::VerifyConfig => {
                    let spec = self.ctx.config.config.verify.commands.get(&name).cloned().filter(|_| self.ctx.approved);
                    match spec {
                        Some(spec) => {
                            let command = crate::verify::run_command(&self.host, &self.ctx, &name, &spec, &secrets).await;
                            command_result(&c.id, command, &l)
                        }
                        None => refused(&c.id, &name, l.get("loop.host.check.notApproved")),
                    }
                }
            };
            out.push(result);
        }
        out
    }

    async fn guard(&mut self) -> Guard {
        if self.baseline.files.is_empty() && self.ctx.ssh() {
            return Guard::default();
        }
        let folder = self.ctx.local_folder();
        let now = scan(&folder, self.baseline.files.keys().cloned().collect()).await;
        self.baseline.compare(&now)
    }

    async fn changes(&mut self, run_id: Option<&str>) -> Changes {
        let Some(run_id) = run_id else { return Changes::default() };
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let (ended, changes) = {
                let view = self.host.view();
                let Some(pane) = view.snapshot.session(&self.ctx.pane) else { return Changes::default() };
                let Some(run) = pane.graph_runs().iter().find(|r| r.id.as_deref() == Some(run_id)) else {
                    return Changes::default();
                };
                let checkpoints = run.checkpoints.as_deref().unwrap_or_default();
                let mut paths = std::collections::BTreeSet::new();
                let mut changes = Changes::default();
                for checkpoint in checkpoints.iter().filter(|c| c.kind != "start") {
                    for file in &checkpoint.files {
                        paths.insert(file.path.clone());
                        changes.added += file.added.unwrap_or(0).max(0) as u32;
                        changes.removed += file.removed.unwrap_or(0).max(0) as u32;
                    }
                }
                changes.files = paths.len() as u32;
                (checkpoints.iter().any(|c| c.kind == "end" || c.kind == "turn"), changes)
            };
            if ended || Instant::now() >= deadline {
                return changes;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    fn control(&mut self) -> Control {
        self.control_now()
    }

    fn budget_allows(&mut self) -> bool {
        let (records, spent, budget) = crate::usage_report::budget_reading(&self.host);
        let estimate = mighty_delegation::compare::estimate_run_cost(&records, &self.ctx.provider, self.ctx.model.as_deref());
        mighty_delegation::compare::budget_warning(spent, budget, &[estimate]).is_none()
    }

    fn elapsed_minutes(&mut self) -> f64 {
        self.before_minutes + self.started.elapsed().as_secs_f64() / 60.0
    }

    fn now(&mut self) -> String {
        time::now_o()
    }

    fn summarize(&mut self, round: &RoundResult) -> String {
        verify::failure_summary(round, &crate::verify::redact)
    }

    fn update(&mut self, record: &LoopRecord) {
        save(&self.host, record);
    }
}

fn command_result(id: &str, command: CommandResult, l: &mighty_core::locale::Locale) -> CheckResult {
    let seconds = format!("{:.1}", command.duration_ms as f64 / 1000.0);
    let summary = if command.passed {
        l.get_with("loop.host.check.passed", &[("seconds", &seconds)])
    } else if command.timed_out {
        l.get_with("loop.host.check.timedOut", &[("seconds", &seconds)])
    } else if let Some(error) = &command.error {
        verify::cap_chars(error, 200)
    } else {
        let code = command.exit_code.map_or_else(|| "?".to_string(), |c| c.to_string());
        let last = verify::strip_ansi(&command.output).lines().rev().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").to_string();
        let head = l.get_with("loop.host.check.exit", &[("code", &code)]);
        if last.is_empty() { head } else { format!("{head} · {}", verify::cap_chars(&last, 160)) }
    };
    CheckResult {
        id: id.to_string(),
        passed: Some(command.passed),
        duration_ms: command.duration_ms,
        summary,
        commands: vec![command],
        browser: None,
    }
}

fn browser_result(id: &str, report: verify::BrowserReport, began: Instant, l: &mighty_core::locale::Locale) -> CheckResult {
    let duration_ms = began.elapsed().as_millis() as u64;
    if let Some(reason) = report.unavailable.clone() {
        return refused(id, loops::BROWSER_CHECK, reason);
    }
    let counts = report.counts();
    let problems = counts.console_errors + counts.exceptions + counts.failed_requests + counts.page_errors;
    let summary = l.get_with("loop.host.check.browser", &[("pages", &counts.pages.to_string()), ("problems", &problems.to_string())]);
    CheckResult { id: id.to_string(), passed: Some(!report.failed()), commands: Vec::new(), browser: Some(report), summary, duration_ms }
}

/// A check that could not run: failed, with the reason as its command's error so the repair request says it.
fn refused(id: &str, name: &str, reason: String) -> CheckResult {
    CheckResult {
        id: id.to_string(),
        passed: Some(false),
        commands: vec![CommandResult {
            name: name.to_string(),
            command: name.to_string(),
            passed: false,
            exit_code: None,
            timed_out: false,
            duration_ms: 0,
            output: String::new(),
            error: Some(reason.clone()),
        }],
        browser: None,
        summary: reason,
        duration_ms: 0,
    }
}

// ── the guard's files ────────────────────────────────────────────────────────────────────────────────────────────────────

/// The protected files of `folder` now (git's tracked and unignored files that [`loops::is_protected`] names, plus `also`, the
/// baseline's), each read. Empty when git cannot list the folder.
async fn scan(folder: &Path, also: Vec<String>) -> BTreeMap<String, loops::FileMark> {
    let Some(places) = crate::pane_worktree::places() else { return BTreeMap::new() };
    let dir = folder.to_string_lossy().to_string();
    let listed = mighty_delegation::git::run(&places.git, &["ls-files", "-z", "--cached", "--others", "--exclude-standard"], &dir, 30.0)
        .await
        .filter(|r| r.exit_code == 0);
    let Some(listed) = listed else { return BTreeMap::new() };
    let mut paths: Vec<String> =
        listed.stdout.split('\0').filter(|p| !p.is_empty() && loops::is_protected(p)).map(str::to_string).collect();
    paths.extend(also);
    paths.sort();
    paths.dedup();
    paths.truncate(loops::MAXIMUM_PROTECTED_FILES);
    let folder = folder.to_path_buf();
    tauri::async_runtime::spawn_blocking(move || {
        paths
            .into_iter()
            .filter_map(|p| {
                let bytes = std::fs::read(folder.join(&p)).ok()?;
                Some((p.clone(), loops::mark(&p, &bytes)))
            })
            .collect()
    })
    .await
    .unwrap_or_default()
}

async fn head_commit(folder: &Path) -> Option<String> {
    let places = crate::pane_worktree::places()?;
    let dir = folder.to_string_lossy().to_string();
    let result = mighty_delegation::git::run(&places.git, &["rev-parse", "HEAD"], &dir, 10.0).await?;
    (result.exit_code == 0).then(|| result.stdout.trim().to_string()).filter(|s| !s.is_empty())
}

// ── starting and driving ─────────────────────────────────────────────────────────────────────────────────────────────────

fn words(host: &Host) -> PromptWords {
    let l = host.locale;
    PromptWords {
        criteria_header: l.get("loop.host.prompt.criteria"),
        rules: l.get("loop.host.prompt.rules"),
        repair_header: l.get("loop.host.prompt.repair"),
        guard_touched: l.get("loop.host.prompt.guardTouched"),
        guard_tests_removed: l.get("loop.host.prompt.guardTestsRemoved"),
        guard_skip_added: l.get("loop.host.prompt.guardSkipAdded"),
    }
}

/// The pane's context for a loop: an agent pane of its own on this computer or over SSH (no mirror, delegated child or compare member).
async fn pane_context(host: &Arc<Host>, pane: &str) -> Result<Context, String> {
    let l = host.locale;
    {
        let view = host.view();
        let session = view.snapshot.session(pane).ok_or_else(|| l.get("loop.host.error.noPane"))?;
        if session.kind() != "claude" || session.mirror.is_some() || session.parent_session_id.is_some() || session.compare_member.is_some()
        {
            return Err(l.get("loop.host.error.paneKind"));
        }
    }
    crate::verify::loop_context(host, pane).await?.ok_or_else(|| l.get("loop.host.error.paneKind"))
}

/// The criteria a draft proposes: one per approved `verify` command, and the browser check.
fn proposed(ctx: &Context) -> Vec<Criterion> {
    if !ctx.approved {
        return Vec::new();
    }
    let verify_config = &ctx.config.config.verify;
    let mut out: Vec<Criterion> = verify_config
        .commands
        .iter()
        .map(|(name, spec)| Criterion {
            id: String::new(),
            text: format!("{name}: {}", spec.command),
            check: loops::Check { kind: CheckKind::VerifyConfig, name: Some(name.clone()), command: None, cwd: None },
            level: loops::level_for_name(name),
            status: loops::CriterionStatus::Pending,
            evidence: None,
        })
        .collect();
    if verify_config.browser.is_some() {
        out.push(Criterion {
            id: String::new(),
            text: loops::BROWSER_CHECK.to_string(),
            check: loops::Check { kind: CheckKind::VerifyConfig, name: Some(loops::BROWSER_CHECK.to_string()), command: None, cwd: None },
            level: loops::Level::L4,
            status: loops::CriterionStatus::Pending,
            evidence: None,
        });
    }
    loops::normalize_criteria(out.clone()).unwrap_or(out)
}

/// `loopStart`: the criteria approved and locked, the guard's baseline read, the goal sent.
pub async fn start(host: &Arc<Host>, pane: &str, goal: &str, criteria: Vec<Criterion>, limits: Limits) -> Result<LoopRecord, String> {
    let l = host.locale;
    if !enabled(host) {
        return Err(l.get("loop.host.error.off"));
    }
    let goal = goal.trim();
    if goal.is_empty() {
        return Err(l.get("loop.host.error.noGoal"));
    }
    let goal = verify::cap_chars(goal, loops::MAXIMUM_GOAL_CHARS);
    let ctx = pane_context(host, pane).await?;
    if occupies(pane) {
        return Err(l.get("loop.host.error.running"));
    }
    if crate::phone::busy(host, pane) {
        return Err(l.get("loop.host.error.busy"));
    }
    let criteria = loops::normalize_criteria(criteria).map_err(|e| {
        l.get(match e {
            loops::CriteriaError::NoCommand => "loop.host.error.noCommand",
            loops::CriteriaError::TooMany => "loop.host.error.tooMany",
            loops::CriteriaError::Incomplete => "loop.host.error.incomplete",
        })
    })?;
    if criteria.iter().any(|c| c.check.kind == CheckKind::VerifyConfig) && !ctx.approved {
        return Err(l.get("loop.host.check.notApproved"));
    }
    let folder = ctx.local_folder();
    let (base_commit, baseline) = if ctx.ssh() {
        (None, GuardBaseline::default())
    } else {
        let files = scan(&folder, Vec::new()).await;
        (head_commit(&folder).await, GuardBaseline { files })
    };
    let id = format!("loop-{}", mighty_core::wire::new_id());
    let mut record = LoopRecord::draft(id.clone(), pane, ctx.workspace.id(), &goal, criteria);
    record.criteria_hash = loops::criteria_hash(&record.criteria);
    record.limits = limits.clamped();
    record.state = LoopState::Running;
    record.started_at = Some(time::now_o());
    record.base_commit = base_commit;
    record.guard = baseline.initial();
    if let Ok(bytes) = serde_json::to_vec(&baseline) {
        let file = guard_path(host, &id);
        if let Some(parent) = file.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = mighty_core::automation::write_atomic(&file, &bytes);
    }
    save(host, &record);
    crate::pane_worktree::append_system_note(host, pane, l.get_with("loop.host.note.started", &[("title", &record.title)]));
    let prompt = loops::goal_prompt(&record.goal, &record.criteria, &words(host));
    spawn(host, ctx, record.clone(), baseline, Start::Goal(prompt), 0.0);
    Ok(record)
}

fn spawn(host: &Arc<Host>, ctx: Context, mut record: LoopRecord, baseline: GuardBaseline, start: Start, before_minutes: f64) {
    lock(&LIVE).insert(record.id.clone(), Live { pane: ctx.pane.clone(), control: Control::Go, waiting: None });
    let host = host.clone();
    tauri::async_runtime::spawn(async move {
        let submission = current_submission(&host, &ctx.pane).unwrap_or_default();
        let words = words(&host);
        let mut driver =
            AppLoopHost { host: host.clone(), ctx, id: record.id.clone(), submission, baseline, started: Instant::now(), before_minutes };
        loops::drive(&mut driver, &mut record, start, &words).await;
        lock(&LIVE).remove(&record.id);
        save(&host, &record);
        finished(&host, &record);
    });
}

/// What a loop that ended tells: a line in its pane, and for a failure that is not the person's an OS notification.
fn finished(host: &Arc<Host>, record: &LoopRecord) {
    let l = host.locale;
    let n = record.iterations.len().to_string();
    match record.state {
        LoopState::Passed => {
            crate::pane_worktree::append_system_note(
                host,
                &record.pane_id,
                l.get_with("loop.host.note.passed", &[("title", &record.title), ("n", &n)]),
            );
        }
        LoopState::Failed | LoopState::Stopped => {
            let reason = record.stop_reason.map(|r| l.get(stop_key(r))).unwrap_or_default();
            let text = l.get_with("loop.host.note.ended", &[("title", &record.title), ("n", &n), ("reason", &reason)]);
            crate::pane_worktree::append_system_note(host, &record.pane_id, text);
            if record.state == LoopState::Failed {
                let title = l.get_with("loop.host.notify.title", &[("title", &record.title)]);
                crate::notifications::notify_custom(host, title, reason);
            }
        }
        LoopState::Paused => {
            crate::pane_worktree::append_system_note(
                host,
                &record.pane_id,
                l.get_with("loop.host.note.paused", &[("title", &record.title)]),
            );
        }
        LoopState::Draft | LoopState::Running => {}
    }
}

pub fn stop_key(reason: LoopStop) -> &'static str {
    match reason {
        LoopStop::Passed => "loop.host.stop.passed",
        LoopStop::MaxIterations => "loop.host.stop.maxIterations",
        LoopStop::MaxMinutes => "loop.host.stop.maxMinutes",
        LoopStop::MaxCost => "loop.host.stop.maxCost",
        LoopStop::Budget => "loop.host.stop.budget",
        LoopStop::SameFailure => "loop.host.stop.sameFailure",
        LoopStop::Person => "loop.host.stop.person",
        LoopStop::RepairEnded => "loop.host.stop.repairEnded",
        LoopStop::PaneClosed => "loop.host.stop.paneClosed",
    }
}

/// Minutes between the loop's start and now (a resumed loop counts its paused time too: the limit is wall time).
fn minutes_since(started_at: Option<&str>) -> f64 {
    started_at
        .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
        .map_or(0.0, |at| (chrono::Utc::now() - at.with_timezone(&chrono::Utc)).num_seconds().max(0) as f64 / 60.0)
}

pub async fn pause(host: &Arc<Host>, id: &str) -> Result<LoopRecord, String> {
    let record = find(host, id).ok_or_else(|| host.locale.get("loop.host.error.notFound"))?;
    if let Some(live) = lock(&LIVE).get_mut(id) {
        live.control = Control::Pause;
    }
    Ok(record)
}

pub async fn resume(host: &Arc<Host>, id: &str) -> Result<LoopRecord, String> {
    let l = host.locale;
    let mut record = find(host, id).ok_or_else(|| l.get("loop.host.error.notFound"))?;
    if let Some(live) = lock(&LIVE).get_mut(id) {
        // Paused while its turn ran: it simply goes on.
        live.control = Control::Go;
        return Ok(record);
    }
    if record.state != LoopState::Paused {
        return Err(l.get("loop.host.error.notPaused"));
    }
    if !record.criteria_intact() {
        return Err(l.get("loop.host.error.criteriaChanged"));
    }
    let ctx = pane_context(host, &record.pane_id).await?;
    if crate::phone::busy(host, &record.pane_id) {
        return Err(l.get("loop.host.error.busy"));
    }
    let baseline: GuardBaseline =
        std::fs::read(guard_path(host, id)).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default();
    record.state = LoopState::Running;
    save(host, &record);
    let before = minutes_since(record.started_at.as_deref());
    spawn(host, ctx, record.clone(), baseline, Start::Check, before);
    Ok(record)
}

pub async fn stop(host: &Arc<Host>, id: &str) -> Result<LoopRecord, String> {
    let l = host.locale;
    let mut record = find(host, id).ok_or_else(|| l.get("loop.host.error.notFound"))?;
    let waiting = {
        let mut live = lock(&LIVE);
        match live.get_mut(id) {
            Some(entry) => {
                entry.control = Control::Stop;
                Some(entry.waiting.take())
            }
            None => None,
        }
    };
    match waiting {
        // Driving: it stops at its next step (at once when it waits for a turn; the agent's turn itself goes on).
        Some(Some(sender)) => {
            let _ = sender.send(TurnOutcome::Interrupted);
        }
        Some(None) => {}
        None if record.state == LoopState::Paused => {
            record.state = LoopState::Stopped;
            record.phase = loops::LoopPhase::Done;
            record.stop_reason = Some(LoopStop::Person);
            record.finished_at = Some(time::now_o());
            save(host, &record);
            finished(host, &record);
        }
        None => {}
    }
    Ok(find(host, id).unwrap_or(record))
}

pub fn forget(host: &Arc<Host>, id: &str) -> Result<(), String> {
    let l = host.locale;
    let record = find(host, id).ok_or_else(|| l.get("loop.host.error.notFound"))?;
    if !record.state.finished() {
        return Err(l.get("loop.host.error.notFinished"));
    }
    with_store(host, |s| {
        s.loops.retain(|r| r.id != id);
        ((), true)
    });
    let _ = std::fs::remove_file(guard_path(host, id));
    push(host);
    Ok(())
}

// ── the page's methods ───────────────────────────────────────────────────────────────────────────────────────────────────

fn arg(args: &Value, key: &str) -> Result<String, String> {
    text(args, key).filter(|v| !v.is_empty()).map(str::to_string).ok_or_else(|| "Invalid request.".into())
}

pub fn register(methods: &mut Registry) {
    methods.add("loopList", |host: Arc<Host>, _args| async move { Ok(list_json(&host)) });

    methods.add("loopSetEnabled", |host: Arc<Host>, args| async move {
        let on = args["on"].as_bool().ok_or("Invalid request.")?;
        set_enabled(&host, on);
        Ok(json!({ "enabled": on }))
    });

    // A draft for the sheet: its criteria proposed from the approved `verify`, the default limits. Nothing is kept.
    methods.add("loopDraft", |host: Arc<Host>, args| async move {
        let pane = arg(&args, "paneId")?;
        if !enabled(&host) {
            return Err(host.locale.get("loop.host.error.off"));
        }
        let goal = text(&args, "goal").unwrap_or_default().to_string();
        let ctx = pane_context(&host, &pane).await?;
        let record = LoopRecord::draft(format!("draft-{}", mighty_core::wire::new_id()), &pane, ctx.workspace.id(), &goal, proposed(&ctx));
        serde_json::to_value(record).map_err(|e| e.to_string())
    });

    methods.add("loopStart", |host: Arc<Host>, args| async move {
        let pane = arg(&args, "paneId")?;
        let goal = text(&args, "goal").unwrap_or_default().to_string();
        let criteria: Vec<Criterion> = serde_json::from_value(args["criteria"].clone()).map_err(|_| "Invalid request.".to_string())?;
        let limits: Limits = serde_json::from_value(args["limits"].clone()).unwrap_or_default();
        let record = start(&host, &pane, &goal, criteria, limits).await?;
        serde_json::to_value(record).map_err(|e| e.to_string())
    });

    methods.add("loopPause", |host: Arc<Host>, args| async move {
        let record = pause(&host, &arg(&args, "id")?).await?;
        serde_json::to_value(record).map_err(|e| e.to_string())
    });

    methods.add("loopResume", |host: Arc<Host>, args| async move {
        let record = resume(&host, &arg(&args, "id")?).await?;
        serde_json::to_value(record).map_err(|e| e.to_string())
    });

    methods.add("loopStop", |host: Arc<Host>, args| async move {
        let record = stop(&host, &arg(&args, "id")?).await?;
        serde_json::to_value(record).map_err(|e| e.to_string())
    });

    methods.add("loopForget", |host: Arc<Host>, args| async move {
        forget(&host, &arg(&args, "id")?)?;
        Ok(json!({}))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_stop_has_its_words() {
        let locale = mighty_core::locale::Locale::new("ko");
        for reason in [
            LoopStop::Passed,
            LoopStop::MaxIterations,
            LoopStop::MaxMinutes,
            LoopStop::MaxCost,
            LoopStop::Budget,
            LoopStop::SameFailure,
            LoopStop::Person,
            LoopStop::RepairEnded,
            LoopStop::PaneClosed,
        ] {
            assert_ne!(locale.get(stop_key(reason)), stop_key(reason), "{reason:?}");
        }
    }

    #[test]
    fn the_prompt_words_are_there_in_every_language() {
        for language in ["ko", "en", "ja", "zh"] {
            let locale = mighty_core::locale::Locale::new(language);
            for key in [
                "loop.host.prompt.criteria",
                "loop.host.prompt.rules",
                "loop.host.prompt.repair",
                "loop.host.prompt.guardTouched",
                "loop.host.prompt.guardTestsRemoved",
                "loop.host.prompt.guardSkipAdded",
            ] {
                assert_ne!(locale.get(key), key, "{language} {key}");
            }
            let repair = locale.get("loop.host.prompt.repair");
            assert!(repair.contains("{n}") && repair.contains("{max}"), "{language}: {repair}");
        }
    }

    #[test]
    fn minutes_are_counted_from_the_start() {
        assert_eq!(minutes_since(None), 0.0);
        let an_hour_ago = (chrono::Utc::now() - chrono::Duration::minutes(60)).to_rfc3339();
        let minutes = minutes_since(Some(&an_hour_ago));
        assert!((59.0..61.0).contains(&minutes), "{minutes}");
    }
}
