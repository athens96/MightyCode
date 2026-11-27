//! MC-48: the verification gate after an agent's turn, and the browser check.
//!
//! - When an agent pane's turn ends well (`completed`; not stopped, failed, waiting for the person, with something queued, an
//!   Antigravity card, a delegated child, a mirror or a compare member) in a workspace whose `.mighty.yaml` has an approved `verify`,
//!   the `verify` commands run in the run's folder (its worktree when it has one) with the pane's variables (`MIGHTY_PORT`, …), on
//!   the SSH machine for an SSH workspace and under the run's sandbox when the workspace has it on (refused, never run outside it,
//!   when it cannot be made). `verify.when: changed` (the default) skips a turn whose request changed no file (its MC-18
//!   checkpoints; when none can tell, it verifies).
//! - A failure is summarized (mighty-delegation `verify::failure_summary`: the end of each failing command, secrets redacted, at most
//!   6,000 characters) and sent to the same pane as its next request; then it verifies again. mighty-delegation `verify::run_loop`
//!   stops it: a pass, `verify.maxRounds` repairs, the same failure twice, the daily budget, the person sending anything or stopping
//!   the run, a repair turn that did not end well. A stop that is not the person's is said by an OS notification and the phone's push.
//! - The receipt ([`VerifyReceipt`]) goes to the page (`verify` push: the result card) and the phone (the pane's detail `verify`).
//!   The person turns the automatic repair off per pane and runs a verification from the card (`verifyRun`).
//! - The browser check ([`browser`]) runs as part of the verification when `verify.browser` is set, and as the agents'
//!   `mighty_browser_check` tool ([`tool`]).

pub mod browser;
pub mod cdp;

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use mighty_core::model::{PaneWorktree, Workspace};
use mighty_core::time;
use mighty_delegation::mighty_yaml::{self, LoadedRepoConfig, VerifyCommandSpec, VerifyWhen};
use mighty_delegation::run_actions;
use mighty_delegation::verify::{
    self, BrowserReport, BrowserScope, CommandResult, LoopSettings, RepairOutcome, RoundResult, StopReason, VerifyHost, VerifyReceipt,
    VerifyStatus,
};
use mighty_platform::SpawnRequest;
use serde_json::{Value, json};
use tokio::sync::oneshot;

use crate::host::{Host, Registry, lock};
use crate::methods::text;

/// The receipts kept per pane (newest first).
const RECEIPTS_KEPT: usize = 10;
/// How long the end of a request's changes is waited for (its checkpoint comes after the turn's end).
const CHANGES_WAIT: Duration = Duration::from_secs(10);

/// A verification running in a pane.
struct Active {
    /// The repair turn being waited for: told how it ended.
    waiting: Option<oneshot::Sender<RepairOutcome>>,
    /// The person pressed 중지 on the card.
    stopped: bool,
}

#[derive(Default)]
struct PaneVerify {
    auto_repair_off: bool,
    receipts: VecDeque<VerifyReceipt>,
    active: Option<Active>,
    /// The config hash an "not approved" note was shown for (once per change).
    warned: Option<String>,
}

static PANES: LazyLock<Mutex<HashMap<String, PaneVerify>>> = LazyLock::new(Mutex::default);

fn panes() -> std::sync::MutexGuard<'static, HashMap<String, PaneVerify>> {
    lock(&PANES)
}

// ── what a verification works on ─────────────────────────────────────────────────────────────────────────────────────────

/// The pane, its workspace and where its run works (MC-118's loops run their checks with it too).
#[derive(Clone)]
pub(crate) struct Context {
    pub(crate) pane: String,
    pub(crate) workspace: Workspace,
    pub(crate) provider: String,
    pub(crate) model: Option<String>,
    pub(crate) record: Option<PaneWorktree>,
    pub(crate) config: LoadedRepoConfig,
    pub(crate) approved: bool,
}

impl Context {
    pub(crate) fn ssh(&self) -> bool {
        self.workspace.ssh.is_some()
    }

    /// The run's folder here (a worktree's when it is on disk, else the workspace's).
    pub(crate) fn local_folder(&self) -> PathBuf {
        self.record
            .as_ref()
            .map(|r| PathBuf::from(r.run_folder()))
            .filter(|dir| dir.is_dir())
            .unwrap_or_else(|| PathBuf::from(self.workspace.path()))
    }

    /// The run's folder on the SSH machine.
    fn remote_folder(&self) -> String {
        match (&self.record, &self.workspace.ssh) {
            (Some(record), _) => record.run_folder().to_string(),
            (None, Some(reference)) => reference.dir.clone(),
            (None, None) => self.workspace.path().to_string(),
        }
    }
}

/// The pane's context with the repository config its run folder follows (an SSH workspace's read over there); None without a
/// `verify` that has something to run.
async fn context(host: &Arc<Host>, pane_id: &str) -> Result<Option<Context>, String> {
    Ok(loop_context(host, pane_id).await?.filter(|ctx| ctx.config.config.verify.has_checks()))
}

/// The pane's context whatever its `.mighty.yaml` says (an empty, unapproved config when it has none): a loop's own commands run
/// without one. None for a mirror's pane or a workspace without a folder.
pub(crate) async fn loop_context(host: &Arc<Host>, pane_id: &str) -> Result<Option<Context>, String> {
    let (pane, workspace) = {
        let view = host.view();
        let pane = view.snapshot.session(pane_id).cloned().ok_or_else(|| "Session not found.".to_string())?;
        let workspace = view.snapshot.workspace(pane.workspace_id()).cloned().ok_or_else(|| "Workspace not found.".to_string())?;
        (pane, workspace)
    };
    if workspace.remote.is_some() || workspace.path().is_empty() {
        return Ok(None);
    }
    let record = pane.pane_worktree.clone();
    let (config, approved) = if workspace.ssh.is_some() {
        match crate::repo_tasks::load_worktree_config(host, &workspace).await? {
            Some(status) => (status.loaded, status.approved),
            None => (empty_config(), false),
        }
    } else {
        let ws_folder = PathBuf::from(workspace.path());
        let folder = record.as_ref().map(|r| PathBuf::from(r.run_folder())).filter(|dir| dir.is_dir()).unwrap_or_else(|| ws_folder.clone());
        let loaded = tauri::async_runtime::spawn_blocking(move || mighty_yaml::load_from_folder_with_local_fallback(&folder, &ws_folder))
            .await
            .map_err(|e| e.to_string())??;
        match loaded {
            Some(loaded) => {
                let approved = crate::repo_tasks::is_config_approved(host, workspace.path(), &loaded.content_hash);
                (loaded, approved)
            }
            None => (empty_config(), false),
        }
    };
    let model = Some(pane.model().to_string()).filter(|m| !m.trim().is_empty());
    Ok(Some(Context { pane: pane_id.to_string(), workspace, provider: pane.provider().to_string(), model, record, config, approved }))
}

fn empty_config() -> LoadedRepoConfig {
    LoadedRepoConfig { config: Default::default(), files: Vec::new(), content_hash: String::new() }
}

// ── after a turn ─────────────────────────────────────────────────────────────────────────────────────────────────────────

/// A pane's run ended as `ended` (runs.rs's pump, after the pane took the event): a repair turn the loop waits for is told how it
/// ended; otherwise a turn that ended well in a pane that may be verified starts a verification.
pub fn on_run_finished(host: &Arc<Host>, pane_id: &str, ended: &str) {
    let waiting = panes().get_mut(pane_id).and_then(|p| p.active.as_mut()).map(|a| a.waiting.take());
    match waiting {
        Some(Some(sender)) => {
            let _ = sender.send(match ended {
                "completed" => RepairOutcome::Done,
                "stopped" => RepairOutcome::Person,
                _ => RepairOutcome::Ended,
            });
            return;
        }
        // A verification is running its commands: the loop sees the person's turn itself.
        Some(None) => return,
        None => {}
    }
    if ended != "completed" || host.smoke.is_some() || host.closing() {
        return;
    }
    let Some(submission) = eligible(host, pane_id) else { return };
    let (host, pane) = (host.clone(), pane_id.to_string());
    tauri::async_runtime::spawn(async move {
        if let Err(error) = after_turn(&host, &pane, submission).await {
            host.log.line(&format!("verify: {pane}: {error}"));
        }
    });
}

/// Whether `pane_id`'s turn may be verified now: a local or SSH agent pane of its own (no mirror, delegated child or compare member),
/// nothing queued, no Antigravity card. Its send counter when so.
fn eligible(host: &Host, pane_id: &str) -> Option<u64> {
    let mut view = host.view();
    let pane = view.snapshot.session(pane_id)?;
    if pane.kind() != "claude" || pane.mirror.is_some() || pane.parent_session_id.is_some() || pane.compare_member.is_some() {
        return None;
    }
    // MC-118: a pane with a loop has the loop's checks instead.
    if crate::loops::occupies(pane_id) {
        return None;
    }
    let workspace = view.snapshot.workspace(pane.workspace_id())?;
    if workspace.remote.is_some() {
        return None;
    }
    let ui = view.panes.get_mut(pane_id)?;
    if !ui.queue.items().is_empty() {
        return None;
    }
    let submission = ui.submission;
    drop(view);
    crate::antigravity::card_input(host, pane_id).is_none().then_some(submission)
}

async fn after_turn(host: &Arc<Host>, pane_id: &str, submission: u64) -> Result<(), String> {
    let Some(ctx) = context(host, pane_id).await? else { return Ok(()) };
    if !ctx.approved {
        let first = {
            let mut all = panes();
            let entry = all.entry(pane_id.to_string()).or_default();
            let first = entry.warned.as_deref() != Some(ctx.config.content_hash.as_str());
            entry.warned = Some(ctx.config.content_hash.clone());
            first
        };
        if first {
            crate::pane_worktree::append_system_note(host, pane_id, host.locale.get("verify.note.notApproved"));
        }
        return Ok(());
    }
    let run_id = {
        let view = host.view();
        let pane = view.snapshot.session(pane_id).ok_or("Session not found.")?;
        mighty_graph::support::newest_request_run(pane.graph_runs()).and_then(|r| r.id.clone())
    };
    if ctx.config.config.verify.when() == VerifyWhen::Changed
        && let Some(run) = run_id.as_deref()
        && request_changed(host, pane_id, run).await == Some(false)
    {
        return Ok(());
    }
    // The person may have sent something while the changes were waited for.
    if current_submission(host, pane_id) != Some(submission) || crate::phone::busy(host, pane_id) {
        return Ok(());
    }
    run(host, ctx, run_id, submission, false).await
}

/// Whether request `run_id` of `pane_id` changed any file: a checkpoint after its start that names files. None when no checkpoint
/// can tell (no snapshot of the folder): it is verified then.
async fn request_changed(host: &Host, pane_id: &str, run_id: &str) -> Option<bool> {
    let deadline = Instant::now() + CHANGES_WAIT;
    loop {
        let (started, changed) = {
            let view = host.view();
            let pane = view.snapshot.session(pane_id)?;
            let run = pane.graph_runs().iter().find(|r| r.id.as_deref() == Some(run_id))?;
            let checkpoints = run.checkpoints.as_deref().unwrap_or_default();
            (
                checkpoints.iter().any(|c| c.kind == "start"),
                checkpoints.iter().any(|c| c.kind != "start" && (!c.files.is_empty() || c.more.unwrap_or(0) > 0)),
            )
        };
        if changed {
            return Some(true);
        }
        if Instant::now() >= deadline {
            return started.then_some(false);
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

fn current_submission(host: &Host, pane_id: &str) -> Option<u64> {
    host.view().panes.get(pane_id).map(|ui| ui.submission)
}

// ── the loop ─────────────────────────────────────────────────────────────────────────────────────────────────────────────

/// Runs the verification loop of `ctx` (`manual`: once, from the card, never asking for a repair).
async fn run(host: &Arc<Host>, ctx: Context, run_id: Option<String>, submission: u64, manual: bool) -> Result<(), String> {
    {
        let mut all = panes();
        let entry = all.entry(ctx.pane.clone()).or_default();
        if entry.active.is_some() {
            return Err(host.locale.get("verify.error.running"));
        }
        entry.active = Some(Active { waiting: None, stopped: false });
    }
    let auto_repair = !manual && !panes().get(&ctx.pane).is_some_and(|p| p.auto_repair_off);
    let verify_config = &ctx.config.config.verify;
    let id = mighty_core::wire::new_id();
    let mut receipt = VerifyReceipt::new(id.clone(), ctx.config.short_hash(), verify_config.rounds(), time::now_o());
    receipt.manual = manual;
    receipt.run_ids.extend(run_id);
    let settings = LoopSettings { max_rounds: verify_config.rounds(), auto_repair, repair_header: host.locale.get("verify.repair.header") };
    let mut loop_host = AppVerifyHost { host: host.clone(), ctx: ctx.clone(), submission, round: 0, receipt_id: id };
    verify::run_loop(&mut loop_host, &mut receipt, &settings, &redact).await;
    receipt.finished_at = Some(time::now_o());
    if let Some(entry) = panes().get_mut(&ctx.pane) {
        entry.active = None;
    }
    store(host, &ctx.pane, &receipt);
    finished(host, &ctx, &receipt);
    Ok(())
}

/// What a finished verification tells: a line in the pane, and for a failure the loop stopped on (not the person's, not a manual
/// one) an OS notification and the phone's push.
fn finished(host: &Arc<Host>, ctx: &Context, receipt: &VerifyReceipt) {
    let l = host.locale;
    let rounds = receipt.rounds.to_string();
    if receipt.status == VerifyStatus::Passed {
        let count = receipt.commands.len().to_string();
        crate::pane_worktree::append_system_note(
            host,
            &ctx.pane,
            l.get_with("verify.note.passed", &[("count", &count), ("rounds", &rounds)]),
        );
        return;
    }
    let reason = receipt.stop_reason.map(|r| l.get(reason_key(r))).unwrap_or_default();
    let text = l.get_with("verify.note.failed", &[("rounds", &rounds), ("reason", &reason)]);
    crate::pane_worktree::append_system_note(host, &ctx.pane, text);
    if receipt.manual || receipt.stop_reason == Some(StopReason::Person) {
        return;
    }
    let title = l.get_with("verify.notify.title", &[("rounds", &rounds)]);
    let pane_title = host.view().snapshot.session(&ctx.pane).map(|p| p.title().to_string()).unwrap_or_default();
    let body = l.get_with("verify.notify.body", &[("pane", &pane_title), ("reason", &reason)]);
    crate::notifications::notify_custom(host, title.clone(), body.clone());
    if let Some(push) = crate::mobile::push(host) {
        let _ = push.send_custom(mighty_remote::push::message::FAILED, &title, &body, &ctx.pane);
    }
}

pub fn reason_key(reason: StopReason) -> &'static str {
    match reason {
        StopReason::MaxRounds => "verify.stop.maxRounds",
        StopReason::SameFailure => "verify.stop.sameFailure",
        StopReason::Budget => "verify.stop.budget",
        StopReason::Person => "verify.stop.person",
        StopReason::RepairOff => "verify.stop.repairOff",
        StopReason::RepairEnded => "verify.stop.repairEnded",
    }
}

/// The host's secret redactor, and the values of `worktree.env` never repeated to the agent.
pub(crate) fn redact(text: &str) -> String {
    crate::git_workbench::redact_secret_text(text)
}

fn redact_with(text: &str, values: &[String]) -> String {
    let mut out = redact(text);
    for value in values.iter().filter(|v| v.chars().count() >= 6) {
        out = out.replace(value.as_str(), "[redacted]");
    }
    out
}

struct AppVerifyHost {
    host: Arc<Host>,
    ctx: Context,
    /// The pane's send counter as the loop last left it (after its own repair request).
    submission: u64,
    round: u32,
    receipt_id: String,
}

impl AppVerifyHost {
    fn stopped(&self) -> bool {
        panes().get(&self.ctx.pane).and_then(|p| p.active.as_ref()).is_some_and(|a| a.stopped)
    }
}

impl VerifyHost for AppVerifyHost {
    async fn verify(&mut self) -> RoundResult {
        self.round += 1;
        run_round(&self.host, &self.ctx, &format!("{}-{}", self.receipt_id, self.round), || self.stopped()).await
    }

    fn person_stepped_in(&mut self) -> bool {
        let host = &self.host;
        let pane = &self.ctx.pane;
        self.stopped()
            || !crate::panes::alive(&host.view(), pane)
            || current_submission(host, pane) != Some(self.submission)
            || host.view().panes.get(pane).is_some_and(|ui| !ui.queue.items().is_empty())
    }

    fn budget_allows(&mut self) -> bool {
        let (records, spent, budget) = crate::usage_report::budget_reading(&self.host);
        let estimate = mighty_delegation::compare::estimate_run_cost(&records, &self.ctx.provider, self.ctx.model.as_deref());
        mighty_delegation::compare::budget_warning(spent, budget, &[estimate]).is_none()
    }

    async fn repair(&mut self, prompt: String) -> RepairOutcome {
        let (sender, mut receiver) = oneshot::channel();
        if let Some(active) = panes().get_mut(&self.ctx.pane).and_then(|p| p.active.as_mut()) {
            active.waiting = Some(sender);
        }
        let host = self.host.clone();
        if crate::phone::control_send(&host, &self.ctx.pane, &prompt).await.is_err() {
            if let Some(active) = panes().get_mut(&self.ctx.pane).and_then(|p| p.active.as_mut()) {
                active.waiting = None;
            }
            return RepairOutcome::Ended;
        }
        // Its own send moved the counter: anything after it is the person's.
        self.submission = current_submission(&host, &self.ctx.pane).unwrap_or(self.submission);
        loop {
            tokio::select! {
                outcome = &mut receiver => return outcome.unwrap_or(RepairOutcome::Ended),
                () = tokio::time::sleep(Duration::from_secs(5)) => {
                    if !crate::panes::alive(&host.view(), &self.ctx.pane) || host.closing() {
                        return RepairOutcome::Ended;
                    }
                    if self.stopped() {
                        return RepairOutcome::Person;
                    }
                }
            }
        }
    }

    fn update(&mut self, receipt: &VerifyReceipt) {
        let mut receipt = receipt.clone();
        // The repair's request joins the receipt as its turn starts.
        if receipt.status != VerifyStatus::Repairing
            && let Some(newest) = self
                .host
                .view()
                .snapshot
                .session(&self.ctx.pane)
                .and_then(|p| mighty_graph::support::newest_request_run(p.graph_runs()).and_then(|r| r.id.clone()))
            && !receipt.run_ids.contains(&newest)
        {
            receipt.run_ids.push(newest);
        }
        store(&self.host, &self.ctx.pane, &receipt);
    }
}

/// Keeps `receipt` for its pane (replacing the one of the same id) and tells the page and the phone.
fn store(host: &Arc<Host>, pane: &str, receipt: &VerifyReceipt) {
    {
        let mut all = panes();
        let entry = all.entry(pane.to_string()).or_default();
        // The run ids it gathered stay with it, in the order the requests came.
        let mut receipt = receipt.clone();
        if let Some(old) = entry.receipts.iter().find(|r| r.id == receipt.id) {
            receipt.run_ids = merge_run_ids(&old.run_ids, &receipt.run_ids);
        }
        entry.receipts.retain(|r| r.id != receipt.id);
        entry.receipts.push_front(receipt);
        entry.receipts.truncate(RECEIPTS_KEPT);
    }
    push(host, pane);
    crate::phone::changed(host);
}

/// `kept` then each of `new` it does not hold yet.
fn merge_run_ids(kept: &[String], new: &[String]) -> Vec<String> {
    let mut merged = kept.to_vec();
    for id in new {
        if !merged.contains(id) {
            merged.push(id.clone());
        }
    }
    merged
}

fn pane_json(pane: &str) -> Value {
    let all = panes();
    let entry = all.get(pane);
    json!({
        "kind": "verify",
        "paneId": pane,
        "autoRepair": !entry.is_some_and(|p| p.auto_repair_off),
        "running": entry.is_some_and(|p| p.active.is_some()),
        "receipts": entry.map(|p| p.receipts.iter().collect::<Vec<_>>()).unwrap_or_default(),
    })
}

fn push(host: &Host, pane: &str) {
    let message = pane_json(pane).to_string();
    host.push(&host.view(), "verify", message);
}

/// The pane's newest receipt for the phone (`verify` in its detail): without the commands' output.
pub fn phone_value(pane: &str) -> Option<Value> {
    let all = panes();
    let receipt = all.get(pane)?.receipts.front()?;
    let mut value = serde_json::to_value(receipt).ok()?;
    if let Some(commands) = value["commands"].as_array_mut() {
        for command in commands {
            if let Some(object) = command.as_object_mut() {
                object.remove("output");
            }
        }
    }
    if let Some(object) = value.as_object_mut() {
        object.remove("browser");
    }
    Some(value)
}

/// A pane closed: its receipts go.
pub fn forget(pane: &str) {
    panes().remove(pane);
}

// ── one round ────────────────────────────────────────────────────────────────────────────────────────────────────────────

/// Runs each `verify` command in order (the rest are marked stopped once the person pressed 중지), then the browser check.
async fn run_round(host: &Arc<Host>, ctx: &Context, shot_prefix: &str, stopped: impl Fn() -> bool) -> RoundResult {
    let mut round = RoundResult::default();
    let secrets: Vec<String> = ctx.config.config.worktree.env.values().cloned().collect();
    for (name, spec) in &ctx.config.config.verify.commands {
        if stopped() {
            round.commands.push(CommandResult {
                name: name.clone(),
                command: spec.command.clone(),
                passed: false,
                exit_code: None,
                timed_out: false,
                duration_ms: 0,
                output: String::new(),
                error: Some(host.locale.get("verify.stopped")),
            });
            continue;
        }
        round.commands.push(run_command(host, ctx, name, spec, &secrets).await);
    }
    if let Some(browser) = ctx.config.config.verify.browser.clone()
        && !stopped()
    {
        round.browser = Some(browser_step(host, ctx, &browser, shot_prefix).await);
    }
    round
}

pub(crate) async fn run_command(
    host: &Arc<Host>,
    ctx: &Context,
    name: &str,
    spec: &VerifyCommandSpec,
    secrets: &[String],
) -> CommandResult {
    let began = Instant::now();
    let failed = |error: String| CommandResult {
        name: name.to_string(),
        command: spec.command.clone(),
        passed: false,
        exit_code: None,
        timed_out: false,
        duration_ms: began.elapsed().as_millis() as u64,
        output: String::new(),
        error: Some(error),
    };
    let (request, sandbox) = match command_request(host, ctx, spec).await {
        Ok(made) => made,
        Err(error) => return failed(error),
    };
    let outcome = run_actions::run_bounded(host.processes.as_ref(), request, spec.timeout()).await;
    if let Some(sandbox) = sandbox {
        sandbox.close();
    }
    let output = verify::cap_chars_tail(&redact_with(&verify::strip_ansi(&outcome.output), secrets), verify::RECEIPT_OUTPUT_CHARS);
    CommandResult {
        name: name.to_string(),
        command: spec.command.clone(),
        passed: outcome.succeeded(),
        exit_code: outcome.exit_code,
        timed_out: outcome.timed_out,
        duration_ms: began.elapsed().as_millis() as u64,
        output,
        error: outcome.error,
    }
}

/// The command's request: on the SSH machine in the run's folder there, or here with the login shell's `PATH`, `worktree.env` and
/// the pane's variables, under the run's sandbox when the workspace has it on.
async fn command_request(
    host: &Arc<Host>,
    ctx: &Context,
    spec: &VerifyCommandSpec,
) -> Result<(SpawnRequest, Option<mighty_run::sandbox::RunSandbox>), String> {
    let config = &ctx.config.config;
    if ctx.ssh() {
        if !host.may_change_machine() {
            return Err(host.locale.get("verify.error.unavailable"));
        }
        let reference = ctx.workspace.ssh.as_ref().ok_or("no SSH machine")?;
        let launch = crate::ssh::launch_for(host, &ctx.workspace)?;
        let mut values: Vec<(String, String)> = config.worktree.env.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        if let Some(record) = &ctx.record {
            for (key, value) in record.environment(&reference.dir) {
                values.retain(|(k, _)| *k != key);
                values.push((key, value));
            }
        }
        let folder = ctx.remote_folder();
        let dir = match spec.cwd.as_deref() {
            Some(cwd) => format!("{}/{cwd}", folder.trim_end_matches('/')),
            None => folder,
        };
        return Ok((crate::ssh::worktrees::command_request(&launch, &dir, &spec.command, &values)?, None));
    }
    let folder = ctx.local_folder();
    let cwd = crate::repo_tasks::resolve_task_cwd(&folder, spec.cwd.as_deref())?;
    let ws_folder = ctx.workspace.path().to_string();
    let mut request = crate::repo_tasks::shell_request(&spec.command, cwd);
    for (key, value) in crate::repo_tasks::login_shell_env(host, Some(Path::new(&ws_folder))).await {
        request = request.env(key, value);
    }
    for (key, value) in &config.worktree.env {
        request = request.env(key.clone(), value.clone());
    }
    match &ctx.record {
        Some(record) => {
            for (key, value) in record.environment(&ws_folder) {
                request = request.env(key, value);
            }
        }
        None => {
            let count = crate::worktree_lifecycle::port_count(Some(&ctx.config));
            let (_, _, variables) = crate::worktree_lifecycle::main_place(host, Path::new(&ws_folder), count);
            for (key, value) in variables {
                request = request.env(key, value);
            }
        }
    }
    let sandbox = sandboxed(host, ctx, &folder, &mut request).await?;
    Ok((request, sandbox))
}

/// The run's sandbox laid on `request` when the workspace has it on (as the agent's run gets it: the same profile, the network
/// allow list with nothing asked); Err when it cannot be made — the command is then not run.
async fn sandboxed(
    host: &Arc<Host>,
    ctx: &Context,
    folder: &Path,
    request: &mut SpawnRequest,
) -> Result<Option<mighty_run::sandbox::RunSandbox>, String> {
    use mighty_run::sandbox::{self, gate};
    let hooks_host = crate::sandbox::verify_hooks(host);
    let probe = mighty_run::model::StartRunRequest::new(&ctx.pane, ctx.workspace.id(), "claude", "verify");
    let Some(setup) = hooks_host.setup(&ctx.workspace, &probe) else { return Ok(None) };
    let refused = |reason: String| host.locale.get_with("verify.error.sandbox", &[("reason", &reason)]);
    if !sandbox::supported() {
        return Err(refused(mighty_run::strings::get("sandbox.warning.unsupported")));
    }
    if !sandbox::available() {
        return Err(refused(mighty_run::strings::get("sandbox.warning.unavailable")));
    }
    let logger = host.clone();
    let hooks = gate::GateHooks {
        card: Arc::new(|_| {}),
        blocked: Arc::new(move |target, reason| logger.log.line(&format!("verify sandbox: blocked {} ({reason})", target.label()))),
    };
    let session = format!("verify-{}", ctx.pane);
    let prepared = if cfg!(windows) {
        sandbox::prepare_windows(&setup, hooks_host.clone(), &ctx.provider, &session, folder, request, hooks).await
    } else {
        sandbox::prepare(&setup, &ctx.provider, &session, folder, request, hooks).await
    };
    let prepared = prepared.map_err(refused)?;
    if let Some(gate) = &prepared.gate {
        // Nobody is asked during a verification: a host off the list is refused at once.
        gate.refuse_unlisted();
    }
    Ok(Some(prepared))
}

// ── the browser ──────────────────────────────────────────────────────────────────────────────────────────────────────────

/// The addresses this pane's browser check may visit: its port block (a worktree's, else the main checkout's) and the address its
/// run action printed.
fn scope_of(host: &Arc<Host>, ctx: &Context, detected: Option<String>) -> BrowserScope {
    let block = match &ctx.record {
        Some(record) => record.port_block().and_then(|(base, count)| Some((u16::try_from(base).ok()?, u16::try_from(count).ok()?))),
        None if !ctx.ssh() => {
            let count = crate::worktree_lifecycle::port_count(Some(&ctx.config));
            crate::worktree_lifecycle::main_place(host, Path::new(ctx.workspace.path()), count).1.map(|b| (b.base, b.count))
        }
        None => None,
    };
    BrowserScope { block, detected }
}

/// The address a run action running in the pane's folder printed: the one named, else the first.
fn running_action_url(ctx: &Context, host: &Host, name: Option<&str>) -> Option<String> {
    if ctx.ssh() {
        return None;
    }
    let key = crate::worktree_lifecycle::path_key(&ctx.local_folder().to_string_lossy());
    crate::repo_tasks::running_actions_in(host, &key)
        .into_iter()
        .filter(|run| name.is_none_or(|n| run["name"] == n))
        .find_map(|run| run["url"].as_str().map(str::to_string))
}

pub(crate) async fn browser_step(
    host: &Arc<Host>,
    ctx: &Context,
    spec: &mighty_delegation::mighty_yaml::VerifyBrowserSpec,
    prefix: &str,
) -> BrowserReport {
    let base = spec.url.clone().or_else(|| running_action_url(ctx, host, spec.run_action.as_deref()));
    let Some(base) = base else {
        let action = spec.run_action.clone().unwrap_or_default();
        return BrowserReport::unavailable(host.locale.get_with("verify.browser.noAddress", &[("action", &action)]));
    };
    let scope = scope_of(host, ctx, Some(base.clone()));
    let targets = match browser::targets(&base, &spec.visit_paths(), &scope) {
        Ok(targets) => targets,
        Err(reason) => return BrowserReport::unavailable(host.locale.get_with("verify.browser.refused", &[("reason", &reason)])),
    };
    let targets = match forwarded(host, ctx, targets).await {
        Ok(targets) => targets,
        Err(reason) => return BrowserReport::unavailable(reason),
    };
    browser::check(host, &targets, browser::CheckLimits::VERIFY, prefix).await
}

/// An SSH workspace's addresses through a forward of each port to this computer (MC-33's 로컬로 연결); a local one's as they are.
async fn forwarded(host: &Arc<Host>, ctx: &Context, targets: Vec<url::Url>) -> Result<Vec<url::Url>, String> {
    if !ctx.ssh() {
        return Ok(targets);
    }
    let mut out = Vec::with_capacity(targets.len());
    for mut target in targets {
        let remote = target.port_or_known_default().ok_or("no port")?;
        let local = crate::ssh::forward::open(host, &ctx.pane, remote).await?;
        target.set_host(Some("127.0.0.1")).map_err(|e| e.to_string())?;
        target.set_port(Some(local)).map_err(|()| "port".to_string())?;
        out.push(target);
    }
    Ok(out)
}

/// The agents' `mighty_browser_check {url?, paths?}` (agent_io.rs): the pane's dev server checked as the verification does, the
/// address only within the pane's ports or its run action's; the findings as JSON.
pub async fn tool(host: &Arc<Host>, pane_id: &str, arguments: &str) -> Result<Value, String> {
    let args: Value = serde_json::from_str(arguments).map_err(|_| "Invalid arguments.".to_string())?;
    let (workspace, record) = {
        let view = host.view();
        let pane = view.snapshot.session(pane_id).ok_or("Agent pane closed.")?;
        let workspace = view.snapshot.workspace(pane.workspace_id()).cloned().ok_or("Workspace not found.")?;
        (workspace, pane.pane_worktree.clone())
    };
    let ctx = Context {
        pane: pane_id.to_string(),
        provider: String::new(),
        model: None,
        record,
        config: empty_config(),
        approved: false,
        workspace,
    };
    if ctx.ssh() || ctx.workspace.remote.is_some() {
        return Err(host.locale.get("verify.tool.localOnly"));
    }
    let detected = running_action_url(&ctx, host, None);
    let base =
        args["url"].as_str().map(str::to_string).or_else(|| detected.clone()).ok_or_else(|| host.locale.get("verify.tool.noAddress"))?;
    let paths: Vec<String> =
        args["paths"].as_array().map(|p| p.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default();
    // An address the agent names gives no port of its own: only the pane's block and its running action's.
    let scope = scope_of(host, &ctx, detected);
    let targets = browser::targets(&base, &paths, &scope)?;
    let prefix = format!("tool-{}", mighty_core::wire::new_id());
    let report = browser::check(host, &targets, browser::CheckLimits::TOOL, &prefix).await;
    if let Some(reason) = &report.unavailable {
        return Ok(json!({ "available": false, "reason": reason }));
    }
    let counts = report.counts();
    Ok(json!({
        "available": true,
        "browser": report.browser,
        "passed": !report.failed(),
        "counts": counts,
        "pages": report.pages,
        "screenshotFolder": browser::screenshot_dir(host).to_string_lossy(),
    }))
}

// ── the page's methods ───────────────────────────────────────────────────────────────────────────────────────────────────

fn pane_arg(args: &Value) -> Result<String, String> {
    text(args, "paneId").filter(|id| !id.is_empty()).map(str::to_string).ok_or_else(|| "Invalid request.".into())
}

pub fn register(methods: &mut Registry) {
    // The pane's receipts and switch, and whether its workspace has an approved `verify` (the card's 검증 실행).
    methods.add("verifyState", |host: Arc<Host>, args| async move {
        let pane = pane_arg(&args)?;
        let ctx = context(&host, &pane).await.ok().flatten();
        let mut value = pane_json(&pane);
        value["configured"] = json!(ctx.is_some());
        value["approved"] = json!(ctx.as_ref().is_some_and(|c| c.approved));
        value["commands"] =
            json!(ctx.as_ref().map(|c| c.config.config.verify.commands.keys().cloned().collect::<Vec<_>>()).unwrap_or_default());
        value["browser"] = json!(ctx.as_ref().is_some_and(|c| c.config.config.verify.browser.is_some()));
        Ok(value)
    });

    methods.add("verifySetAutoRepair", |host: Arc<Host>, args| async move {
        let pane = pane_arg(&args)?;
        let on = args["on"].as_bool().ok_or("Invalid request.")?;
        panes().entry(pane.clone()).or_default().auto_repair_off = !on;
        push(&host, &pane);
        Ok(json!({ "autoRepair": on }))
    });

    // 검증 실행: once, now, never asking for a repair.
    methods.add("verifyRun", |host: Arc<Host>, args| async move {
        let pane = pane_arg(&args)?;
        let l = host.locale;
        if crate::phone::busy(&host, &pane) {
            return Err(l.get("verify.error.busy"));
        }
        let ctx = context(&host, &pane).await?.ok_or_else(|| l.get("verify.error.notConfigured"))?;
        if !ctx.approved {
            return Err(l.get("verify.note.notApproved"));
        }
        if panes().get(&pane).is_some_and(|p| p.active.is_some()) {
            return Err(l.get("verify.error.running"));
        }
        let run_id = host
            .view()
            .snapshot
            .session(&pane)
            .and_then(|p| mighty_graph::support::newest_request_run(p.graph_runs()).and_then(|r| r.id.clone()));
        let submission = current_submission(&host, &pane).unwrap_or_default();
        let task_host = host.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(error) = run(&task_host, ctx, run_id, submission, true).await {
                task_host.log.line(&format!("verify: {error}"));
            }
        });
        Ok(json!({ "started": true }))
    });

    // 중지: the loop stops after the command running now, and asks for no repair.
    methods.add("verifyStop", |host: Arc<Host>, args| async move {
        let pane = pane_arg(&args)?;
        let stopped = match panes().get_mut(&pane).and_then(|p| p.active.as_mut()) {
            Some(active) => {
                active.stopped = true;
                true
            }
            None => false,
        };
        push(&host, &pane);
        Ok(json!({ "stopped": stopped }))
    });

    // A screenshot of a browser check, as a data URL.
    methods.add("verifyScreenshot", |host: Arc<Host>, args| async move {
        let name = text(&args, "file").unwrap_or_default().to_string();
        if !browser::screenshot_name_ok(&name) {
            return Err("Invalid request.".into());
        }
        let path = browser::screenshot_dir(&host).join(&name);
        let bytes = tauri::async_runtime::spawn_blocking(move || std::fs::read(path).ok()).await.map_err(|e| e.to_string())?;
        let url = crate::files::web_image_url(bytes, "image/png").ok_or_else(|| host.locale.get("verify.error.screenshotGone"))?;
        Ok(json!({ "url": url }))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_values_and_secret_lines_never_reach_the_agent() {
        let text = "connecting with hunter2-password\nAWS_SECRET_ACCESS_KEY=abcdEFGHijklMNOPqrstUVWXyz0123456789abcd\nok";
        let out = redact_with(text, &["hunter2-password".to_string(), "abc".to_string()]);
        assert!(!out.contains("hunter2-password") && out.contains("[redacted]"), "{out}");
        assert!(out.contains("ok"));
        assert!(out.contains("abc") || !text.contains("abc"), "a short value is not hunted for");
    }

    #[test]
    fn a_receipts_requests_stay_in_the_order_they_came() {
        let ids = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(merge_run_ids(&ids(&["a", "b"]), &ids(&["a"])), ids(&["a", "b"]), "the loop's own copy knows only the first");
        assert_eq!(merge_run_ids(&ids(&["a"]), &ids(&["a", "b"])), ids(&["a", "b"]));
        assert_eq!(merge_run_ids(&[], &ids(&["a"])), ids(&["a"]));
    }

    #[test]
    fn every_stop_has_its_words() {
        let locale = mighty_core::locale::Locale::new("ko");
        for reason in [
            StopReason::MaxRounds,
            StopReason::SameFailure,
            StopReason::Budget,
            StopReason::Person,
            StopReason::RepairOff,
            StopReason::RepairEnded,
        ] {
            assert_ne!(locale.get(reason_key(reason)), reason_key(reason), "{reason:?}");
        }
    }
}
