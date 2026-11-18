//! `--smoke-test`: checks the page and the host together without anyone watching. Once the page has called `getLocale` and `ready`
//! and the host has pushed it the snapshot and the frame model, a probe script runs in the page: it waits for the sidebar's pane
//! titles, reads the DOM (pane titles, the selected pane, the theme, tabs, header, status bar), clicks another pane's sidebar row and
//! waits for the page to show it selected, then hands what it saw to the `smoke_probe` command.
//!
//! Then the page's steps, in this order ([`STEPS`]), each where its platform has what it checks:
//!
//! - the frame (smoke_frame.rs: the sidebar, a row click, the add menu's terminal entry, tab switches, the columns preset);
//! - where the page draws terminals, the terminal (smoke_terminal.rs: a shell pane's shell started, typed into through the page's xterm
//!   and its marker read back, the footer, the pane's status running and idle again, the pane closed);
//! - the files pane opened from the add menu, an HTML frame, the reference bubble and the picture viewer (smoke_files.rs);
//! - the work map and the resume flow end to end, from the add menu's Claude entry to the resumed pane's older requests (smoke_history.rs);
//! - with git on PATH, a clone through the Git sheet, then a rename sheet that survives a page reload (smoke_frame.rs);
//! - Settings and a native dialog (smoke_settings.rs), then the setup, accounts and updates steps once their parts have them
//!   (smoke_setup.rs, smoke_accounts.rs, smoke_updates.rs: W6-B, W6-C, W6-D).
//!
//! With `--smoke-fake-cli` an opencode run's question card answered from the page (smoke_acp.rs) and the run steps follow (smoke_runs.rs:
//! the composer, the permission and plan cards, the Mighty diagram, the progress panel and a stop, against mighty-run's stand-in CLI);
//! with `--smoke-real-claude` one real request to the installed
//! `claude` CLI follows the first probe instead of the page's steps.
//!
//! W9's steps ([`W9_STEPS`]: styles after Settings, the app update after the updates, the mirror after the remote step, the help
//! capture after the companion, per-pane worktrees after delegation) each start where their part has written them (Step 0: none does).
//! `--help-capture` and `--perf-scenario` runs are smokes whose page probe their part takes over ([`Special`], W9-H).
//!
//! The host adds whether its own state took the click, the calls it saw and the methods it answered "unimplemented" (only those of
//! [`ALLOWED_UNIMPLEMENTED`] pass, `unimplementedAllowed`), and writes `smoke-result.json`; with `--smoke-exit` the app then quits, 0
//! when the smoke passed, else 1. A smoke that has no result in time (495 s, 645 s with the stand-in CLI and 400 s on the Mac, 525 s
//! with the real one) fails.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tauri::Manager;

use crate::host::{Host, WINDOW};
use crate::surface::SurfaceManager;

pub use crate::smoke_runs::RECORDING;

/// Which steps follow the first probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The page's steps.
    Page,
    /// Then the run steps against mighty-run's stand-in CLI.
    FakeClaude,
    /// The page, then one real request to the installed `claude` CLI.
    RealClaude,
}

/// Which probe's report comes next.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Stage {
    #[default]
    Main,
    /// The frame's steps (smoke_frame.rs).
    Frame,
    /// The terminal step (smoke_terminal.rs), where the page draws terminals.
    Terminal,
    /// The files pane, an HTML frame, the reference bubble and the picture viewer (smoke_files.rs).
    Files,
    /// The work map and the resume list (smoke_history.rs).
    History,
    /// The Git sheet's clone of a local bare repository (smoke_frame.rs).
    Create,
    /// A rename sheet open while the page loads again: drawn again and answered (smoke_frame.rs).
    Reload,
    /// Settings and the native dialogs (smoke_settings.rs, W6-A).
    Settings,
    /// The setup screen, components and the status line (smoke_setup.rs, W6-B).
    Setup,
    /// The accounts, the sign-in card and the usage (smoke_accounts.rs, W6-C).
    Accounts,
    /// The plugin browser and the updates (smoke_updates.rs, W6-D).
    Updates,
    /// W8's remote step, in parts ([`REMOTE_PARTS`]): the direct path (smoke_direct.rs, W8-C), remote workspaces (smoke_remote.rs, W8-D)
    /// and the phone's round trip (smoke_phone.rs, W8-B), each on loopback inside this process.
    Remote,
    /// The companion's own test overlay and its page controls (smoke_companion.rs, W8-E).
    Companion,
    /// An opencode run's question card answered from the page (smoke_acp.rs), with the stand-in CLI.
    Acp,
    /// Delegation on its fixture repository under the smoke's profile (smoke_delegation.rs, W7-D).
    Delegation,
    /// W9's steps ([`W9_STEPS`]): styles (smoke_styles.rs, W9-D), the app update box (smoke_app_update.rs, W9-C), the remote mirror
    /// (mirror.rs, W9-J), the help capture (smoke_help.rs, W9-H) and per-pane worktrees (pane_worktree.rs, W9-I).
    Styles,
    AppUpdate,
    Mirror,
    Help,
    PaneWorktree,
    Runs,
}

/// The steps after the first probe, in their order: each one that can start does (`Smoke::start`), the run steps last.
const STEPS: [Stage; 20] = [
    Stage::Frame,
    Stage::Terminal,
    Stage::Files,
    Stage::History,
    Stage::Create,
    Stage::Reload,
    Stage::Settings,
    Stage::Styles,
    Stage::Setup,
    Stage::Accounts,
    Stage::Updates,
    Stage::AppUpdate,
    Stage::Remote,
    Stage::Mirror,
    Stage::Companion,
    Stage::Help,
    Stage::Acp,
    Stage::Delegation,
    Stage::PaneWorktree,
    Stage::Runs,
];

/// W9's steps (Step 0): each its stage, the key its report is kept under in the result, its part's start (false: it does not run;
/// every one at rest) and its checks (a step that did not report fails every one of them).
const W9_STEPS: [(Stage, &str, StepStart, StepChecks); 5] = [
    (Stage::Styles, "styles", crate::smoke_styles::start, crate::smoke_styles::checks),
    (Stage::AppUpdate, "appUpdate", crate::smoke_app_update::start, crate::smoke_app_update::checks),
    (Stage::Mirror, "mirror", crate::mirror::smoke_start, crate::mirror::smoke_checks),
    (Stage::Help, "help", crate::smoke_help::start, crate::smoke_help::checks),
    (Stage::PaneWorktree, "paneWorktree", crate::pane_worktree::smoke_start, crate::pane_worktree::smoke_checks),
];

/// The page methods a smoke may meet unanswered (plan D20; README's unimplemented table): none since W9's integration, every method
/// the 0.4.61 page calls is answered.
pub const ALLOWED_UNIMPLEMENTED: [&str; 0] = [];

/// Mac stage (plan §3 0c): the steps whose checks may fail on the Mac while its parts are written, by the keys the result groups them
/// under (`steps`). On the Mac the smoke's exit code says whether every check outside these passed (`passedOutsidePending`); `passed`
/// stays every check's verdict. Step 0 lists every step; each part's merge takes out what passes on the Mac, and the integrator empties
/// it at the end (W9's `ALLOWED_UNIMPLEMENTED` pattern). Windows ignores it. Empty since the Mac stage's integration: every step passes
/// on the Mac.
pub const MAC_PENDING_STEPS: [&str; 0] = [];

/// Which step each check came from, in the order they were added: the checks `checks` gained since the last call are `step`'s.
fn attribute(checks: &Value, steps: &mut Vec<(String, Vec<String>)>, step: &str) {
    let known: std::collections::HashSet<&String> = steps.iter().flat_map(|(_, names)| names).collect();
    let new: Vec<String> = checks.as_object().map(|c| c.keys().filter(|name| !known.contains(name)).cloned().collect()).unwrap_or_default();
    if !new.is_empty() {
        steps.push((step.to_string(), new));
    }
}

/// The checks that failed outside the `pending` steps (Mac stage): none means the Mac's gate passes.
fn failed_outside(checks: &Value, steps: &[(String, Vec<String>)], pending: &[&str]) -> Vec<String> {
    let step_of = |name: &String| steps.iter().find(|(_, names)| names.contains(name)).map(|(step, _)| step.as_str());
    checks
        .as_object()
        .map(|c| {
            c.iter()
                .filter(|(name, value)| *value != &Value::Bool(true) && !step_of(name).is_some_and(|step| pending.contains(&step)))
                .map(|(name, _)| name.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// A run that is a smoke but whose page probe another part takes over (W9-H; main.rs from `--help-capture` and `--perf-scenario`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Special {
    #[default]
    None,
    /// `--help-capture` in this language (help_capture.rs).
    HelpCapture { language: String },
    /// `--perf-scenario` (perf.rs).
    Perf,
}

/// The remote step's parts in their order (W8 plan, Step 0): each that can start does, reports its part through `Smoke::probed`, and
/// the next part starts; the step ends after the last. A part's report is kept under its key in the stage's report.
const REMOTE_PARTS: [(&str, StepStart, StepChecks); 3] = [
    ("mobileDirect", crate::smoke_direct::start, crate::smoke_direct::checks),
    ("remoteWorkspaces", crate::smoke_remote::start, crate::smoke_remote::checks),
    ("phone", crate::smoke_phone::start, crate::smoke_phone::checks),
];

const PROBE: &str = r#"(async () => {
  const until = async (test, ms) => { const end = performance.now() + ms; while (performance.now() < end) { const v = test(); if (v) return v; await new Promise((r) => setTimeout(r, 50)); } return test(); };
  const titles = () => [...document.querySelectorAll('.sidebar-pane-title')];
  const report = {};
  try {
    await until(() => titles().length > 0, 15000);
    const rows = [...document.querySelectorAll('[data-testid=sidebar-pane]')];
    const active = document.querySelector('.sidebar-pane.active');
    report.paneTitles = titles().length;
    report.sidebarRows = rows.length;
    report.workspaces = document.querySelectorAll('[data-testid=sidebar-workspace]').length;
    report.selectedTitle = active?.querySelector('.sidebar-pane-title')?.textContent ?? null;
    report.selectedId = active?.dataset.sessionId ?? null;
    report.theme = document.documentElement.dataset.theme ?? null;
    report.language = document.documentElement.lang || null;
    report.tabs = document.querySelectorAll('[data-testid=tab]').length;
    report.groups = document.querySelectorAll('[data-testid=tab-group]').length;
    report.header = document.querySelector('[data-testid=workspace-header]')?.textContent?.trim().slice(0, 200) ?? null;
    report.statusBar = document.querySelector('[data-testid=status-bar]')?.textContent?.trim().slice(0, 200) ?? null;
    const other = rows.find((row) => row.dataset.sessionId !== report.selectedId);
    if (other) {
      const id = other.dataset.sessionId;
      report.clickTarget = id;
      other.click();
      report.clickSelected = !!(await until(() => document.querySelector('.sidebar-pane.active')?.dataset.sessionId === id && document.querySelector('.pane-slot.active:not(.hidden)')?.dataset.paneId === id, 5000));
      report.activeSlot = document.querySelector('.pane-slot.active:not(.hidden)')?.dataset.paneId ?? null;
    }
    // A moment for the calls the newly shown pane makes, so the unimplemented list holds them too.
    await new Promise((r) => setTimeout(r, 1000));
  } catch (error) {
    report.error = String(error);
  }
  await window.__TAURI_INTERNALS__.invoke('smoke_probe', { probe: report });
})();"#;

#[derive(Default)]
struct Progress {
    calls: Vec<String>,
    snapshot_sent: bool,
    shell_sent: bool,
    injected: bool,
    stage: Stage,
    /// The first probe's report, kept while the later steps run.
    main: Option<Value>,
    /// The host's active pane when the first probe reported (its click must have reached the state).
    main_active: Option<String>,
    frame: Option<Value>,
    terminal: Option<Value>,
    files: Option<Value>,
    history: Option<Value>,
    create: Option<Value>,
    /// Where the gitCreate step's clone goes.
    create_location: Option<PathBuf>,
    /// The `gitProgress` pushes the page was sent.
    git_progress: usize,
    reload: Option<Value>,
    /// The W6 steps' reports.
    settings: Option<Value>,
    setup: Option<Value>,
    accounts: Option<Value>,
    updates: Option<Value>,
    acp: Option<Value>,
    /// The delegation step's report (W7-D).
    delegation: Option<Value>,
    /// The remote step's report: each part's under its key (W8), and the part that runs now.
    remote: Option<Value>,
    remote_part: usize,
    /// The companion step's report (W8-E).
    companion: Option<Value>,
    /// W9's steps' reports, each under its key ([`W9_STEPS`]).
    w9: BTreeMap<&'static str, Option<Value>>,
    /// The reload step's page is loading again: its `ready` brings the probe.
    reload_waiting: bool,
    reload_ready: bool,
    reload_injected: bool,
    /// The name the reload step's rename sheet opened with.
    reload_name: Option<String>,
    /// A help capture or perf scenario took the page over (W9-H).
    special_begun: bool,
    finished: bool,
}

impl Progress {
    /// Where `stage`'s report is kept (the run steps' goes to `finish` as it is).
    fn report(&mut self, stage: Stage) -> Option<&mut Option<Value>> {
        match stage {
            Stage::Main => Some(&mut self.main),
            Stage::Frame => Some(&mut self.frame),
            Stage::Terminal => Some(&mut self.terminal),
            Stage::Files => Some(&mut self.files),
            Stage::History => Some(&mut self.history),
            Stage::Create => Some(&mut self.create),
            Stage::Reload => Some(&mut self.reload),
            Stage::Settings => Some(&mut self.settings),
            Stage::Setup => Some(&mut self.setup),
            Stage::Accounts => Some(&mut self.accounts),
            Stage::Updates => Some(&mut self.updates),
            // Each part's report goes under its key (`Smoke::probed`).
            Stage::Remote => None,
            Stage::Companion => Some(&mut self.companion),
            Stage::Acp => Some(&mut self.acp),
            Stage::Delegation => Some(&mut self.delegation),
            Stage::Styles | Stage::AppUpdate | Stage::Mirror | Stage::Help | Stage::PaneWorktree => {
                let key = w9_step(stage).1;
                Some(self.w9.entry(key).or_default())
            }
            Stage::Runs => None,
        }
    }
}

/// `stage`'s row of [`W9_STEPS`].
fn w9_step(stage: Stage) -> (Stage, &'static str, StepStart, StepChecks) {
    W9_STEPS.iter().copied().find(|(each, ..)| *each == stage).expect("a W9 stage has its row")
}

pub struct Smoke {
    out: PathBuf,
    exit: bool,
    started: Instant,
    /// What the load found, for the result.
    load: Value,
    mode: Mode,
    /// Where the stand-in CLI records what it was sent (`<record>.answers` …).
    fake_record: Option<PathBuf>,
    /// A help capture or perf scenario run (W9-H).
    special: Special,
    progress: Mutex<Progress>,
}

impl Smoke {
    pub fn new(out: PathBuf, exit: bool, load: Value, mode: Mode, fake_record: Option<PathBuf>) -> Smoke {
        Smoke {
            out,
            exit,
            started: Instant::now(),
            load,
            mode,
            fake_record,
            special: Special::None,
            progress: Mutex::new(Progress::default()),
        }
    }

    /// This smoke as a help capture or perf scenario run (W9-H).
    pub fn special(mut self, special: Special) -> Smoke {
        self.special = special;
        self
    }

    pub fn fake_record(&self) -> Option<&PathBuf> {
        self.fake_record.as_ref()
    }

    /// A help capture or perf scenario run (W9-H): its part writes its own result and no CLI is found for it (runs.rs).
    pub fn is_special(&self) -> bool {
        self.special != Special::None
    }

    /// Where the smoke's result goes (`--smoke-out`, else `<profile>\smoke-result.json`).
    pub fn out(&self) -> &std::path::Path {
        &self.out
    }

    /// A help capture or perf scenario run ended (C# `FinishSmoke(passed)`, W9-H): its part wrote its own result; nothing more is written
    /// here, and with `--smoke-exit` the app quits, 0 when it passed, else 1.
    pub fn end(&self, host: &Host, passed: bool) {
        {
            let mut progress = self.progress();
            if progress.finished {
                return;
            }
            progress.finished = true;
        }
        if self.exit {
            host.app.exit(if passed { 0 } else { 1 });
        }
    }

    fn timeout(&self) -> Duration {
        Duration::from_secs(match self.mode {
            // The frame, gitCreate and reload steps add up to two minutes, the terminal and history steps a minute each.
            // The W6 steps a minute more, the W8 steps (remote, companion) another, and W9's (plan R13) 45 s.
            Mode::Page => 495,
            Mode::FakeClaude => crate::platform::smoke_fake_claude_seconds(),
            Mode::RealClaude => 525,
        })
    }

    fn progress(&self) -> std::sync::MutexGuard<'_, Progress> {
        self.progress.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn called(&self, method: &str) {
        let mut progress = self.progress();
        if !progress.calls.iter().any(|m| m == method) {
            progress.calls.push(method.to_string());
        }
        if method == "ready" && progress.reload_waiting {
            progress.reload_ready = true;
        }
    }

    pub fn pushed(&self, kind: &str) {
        let mut progress = self.progress();
        match kind {
            "snapshot" => progress.snapshot_sent = true,
            "shell" => progress.shell_sent = true,
            "gitProgress" => progress.git_progress += 1,
            _ => {}
        }
    }

    /// Runs the probe once the page is up (see the module comment).
    pub fn check(&self, host: &Host) {
        let reloaded = {
            let mut progress = self.progress();
            let now = progress.reload_ready && !progress.reload_injected;
            if now {
                progress.reload_injected = true;
            }
            now.then(|| progress.reload_name.clone().unwrap_or_default())
        };
        if let Some(name) = reloaded {
            host.log.line("smoke: the page loaded again; probing its dialogs");
            let name = serde_json::to_string(&name).unwrap_or_default();
            self.eval(host, &crate::smoke_frame::RELOAD_PROBE.replace("__NAME__", &name));
            return;
        }
        {
            let mut progress = self.progress();
            let up = ["getLocale", "ready"].iter().all(|m| progress.calls.iter().any(|c| c == m))
                && progress.snapshot_sent
                && progress.shell_sent;
            if !up || progress.injected {
                return;
            }
            progress.injected = true;
        }
        // A help capture or perf scenario run: its part takes over here once it is written (W9-H); until then the run is a plain smoke.
        let taken = match &self.special {
            Special::None => false,
            Special::HelpCapture { language } => crate::help_capture::begin(host, language),
            Special::Perf => crate::perf::begin(host),
        };
        if taken {
            self.progress().special_begun = true;
            return;
        }
        host.log.line("smoke: the page is up; probing");
        self.eval(host, PROBE);
    }

    /// Runs `script` in the page's own webview.
    pub(crate) fn eval(&self, host: &Host, script: &str) {
        match host.app.get_webview(WINDOW) {
            Some(page) => {
                if let Err(error) = page.eval(script) {
                    self.probed(host, json!({"error": format!("the probe did not run: {error}")}));
                }
            }
            None => self.probed(host, json!({"error": "no main window"})),
        }
    }

    /// A probe's report: each one starts the next step that can start ([`STEPS`]); the last one finishes.
    pub fn probed(&self, host: &Host, probe: Value) {
        let stage = {
            let mut progress = self.progress();
            if progress.finished {
                return;
            }
            let stage = progress.stage;
            if let Some(report) = progress.report(stage) {
                *report = Some(probe.clone());
            }
            if stage == Stage::Remote {
                let key = REMOTE_PARTS[progress.remote_part.min(REMOTE_PARTS.len() - 1)].0;
                let remote = progress.remote.get_or_insert_with(|| json!({}));
                remote[key] = probe.clone();
            }
            stage
        };
        // The remote step's next part, if one starts; else the step ends here.
        let next_part = self.progress().remote_part + 1;
        if stage == Stage::Remote && self.start_remote_part(host, next_part) {
            return;
        }
        if stage == Stage::Main {
            let active = host.view().snapshot.active_session_id.clone();
            self.progress().main_active = active;
        }
        let next = if stage == Stage::Main && probe.get("error").is_some() {
            None
        } else {
            let after = STEPS.iter().position(|step| *step == stage).map_or(0, |at| at + 1);
            STEPS[after..].iter().copied().find(|step| self.start(host, *step))
        };
        match next {
            Some(next) => self.progress().stage = next,
            None => {
                let runs = (stage == Stage::Runs).then_some(probe);
                self.finish(host, runs);
            }
        }
    }

    /// Starts `step` when it can: the page's steps never with the real CLI, each also only where its platform has what it checks.
    fn start(&self, host: &Host, step: Stage) -> bool {
        let page_steps = self.mode != Mode::RealClaude;
        match step {
            Stage::Main => false,
            Stage::Frame => {
                if page_steps {
                    self.eval(host, crate::smoke_frame::FRAME_PROBE);
                }
                page_steps
            }
            Stage::Terminal => page_steps && self.start_terminal(host),
            Stage::Files => page_steps && self.start_files(host),
            Stage::History => page_steps && crate::smoke_history::start(host),
            Stage::Create => page_steps && self.start_create(host),
            Stage::Reload => page_steps && self.start_reload(host),
            // The W6 steps, each its part's (smoke_settings.rs, smoke_setup.rs, smoke_accounts.rs, smoke_updates.rs).
            Stage::Settings => page_steps && shared(host).is_some_and(|host| crate::smoke_settings::start(&host)),
            Stage::Setup => page_steps && shared(host).is_some_and(|host| crate::smoke_setup::start(&host)),
            Stage::Accounts => page_steps && shared(host).is_some_and(|host| crate::smoke_accounts::start(&host)),
            Stage::Updates => page_steps && shared(host).is_some_and(|host| crate::smoke_updates::start(&host)),
            // W8's steps, each its parts' (smoke_direct.rs, smoke_remote.rs, smoke_phone.rs; smoke_companion.rs).
            Stage::Remote => page_steps && self.start_remote_part(host, 0),
            Stage::Companion => page_steps && shared(host).is_some_and(|host| crate::smoke_companion::start(&host)),
            Stage::Acp => self.mode == Mode::FakeClaude && crate::smoke_acp::start(host),
            Stage::Delegation => page_steps && shared(host).is_some_and(|host| crate::smoke_delegation::start(&host)),
            // W9's steps, each its part's ([`W9_STEPS`]).
            Stage::Styles | Stage::AppUpdate | Stage::Mirror | Stage::Help | Stage::PaneWorktree => {
                let start = w9_step(step).2;
                page_steps && shared(host).is_some_and(|host| start(&host))
            }
            Stage::Runs => self.start_runs(host),
        }
    }

    /// Starts the first part of the remote step from `from` on that can start ([`REMOTE_PARTS`]); false when none does.
    fn start_remote_part(&self, host: &Host, from: usize) -> bool {
        let Some(shared) = shared(host) else { return false };
        for (at, (_, start, _)) in REMOTE_PARTS.iter().enumerate().skip(from) {
            if start(&shared) {
                self.progress().remote_part = at;
                return true;
            }
        }
        false
    }

    /// Makes the gitCreate step's bare repository, points the sheets' location at a temporary folder and runs its probe (smoke_frame.rs);
    /// false without git on PATH.
    fn start_create(&self, host: &Host) -> bool {
        if crate::workspace_create::git_executable().is_none() {
            host.log.line("smoke: git is not on PATH; no gitCreate step");
            return false;
        }
        let Some(shared) = host.app.try_state::<Arc<Host>>().map(|h| h.inner().clone()) else { return false };
        host.log.line("smoke: gitCreate step");
        tauri::async_runtime::spawn(async move {
            let Some(smoke) = &shared.smoke else { return };
            match crate::smoke_frame::bare_repository(&shared).await {
                Ok((bare, location)) => {
                    {
                        let mut view = shared.view();
                        let mut next = view.snapshot.clone();
                        next.last_create_location = Some(location.to_string_lossy().into_owned());
                        view.replace(next);
                    }
                    smoke.progress().create_location = Some(location);
                    let workspace = shared.view().snapshot.active_workspace_id.clone().unwrap_or_default();
                    let url = serde_json::to_string(&bare.to_string_lossy()).unwrap_or_default();
                    smoke.eval(&shared, &crate::smoke_frame::CREATE_PROBE.replace("__URL__", &url).replace("__WORKSPACE__", &workspace));
                }
                Err(error) => smoke.probed(&shared, json!({"error": format!("no bare repository: {error}")})),
            }
        });
        true
    }

    /// Starts a rename of the active workspace (its sheet in the page, the flow waiting on its answer), then loads the page again;
    /// the page's `ready` brings the reload probe (`check`). False without an active workspace.
    fn start_reload(&self, host: &Host) -> bool {
        let Some(shared) = host.app.try_state::<Arc<Host>>().map(|h| h.inner().clone()) else { return false };
        let Some((id, name)) = ({
            let view = host.view();
            let state = &view.snapshot;
            state.active_workspace_id.as_deref().and_then(|id| state.workspace(id)).map(|w| (w.id().to_string(), w.name().to_string()))
        }) else {
            return false;
        };
        host.log.line("smoke: reload step");
        self.progress().reload_name = Some(name);
        tauri::async_runtime::spawn(async move {
            let Some(smoke) = &shared.smoke else { return };
            let dialogs = crate::dialogs::of(&shared);
            // The step before may still be ending its own sheet (its answer reaches the host after the page took it away).
            let settled = || dialogs.open_count() == 0 && !shared.view().dialog_open;
            let deadline = Instant::now() + Duration::from_secs(10);
            while !settled() && Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            crate::frame::start_rename_workspace(&shared, id);
            let deadline = Instant::now() + Duration::from_secs(5);
            while dialogs.open_count() == 0 && Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            if dialogs.open_count() == 0 {
                smoke.probed(&shared, json!({"error": "the rename sheet never opened"}));
                return;
            }
            // A moment for the page to draw the sheet before it loads again.
            tokio::time::sleep(Duration::from_millis(500)).await;
            smoke.progress().reload_waiting = true;
            smoke.eval(&shared, "location.reload()");
        });
        true
    }

    /// Adds the run steps' pane and runs their probe; false in the page-only smoke.
    fn start_runs(&self, host: &Host) -> bool {
        let script = match self.mode {
            Mode::Page => return false,
            Mode::FakeClaude => crate::smoke_runs::FAKE_PROBE
                .replace("__TYPED__", crate::smoke_runs::FAKE_REQUEST)
                .replace("__STOP__", crate::smoke_runs::STOP_REQUEST)
                .replace("__PI_PANE__", crate::smoke_runs::PI_PANE)
                .replace("__PI__", crate::smoke_runs::PI_REQUEST)
                .replace("__AGYTYPED__", crate::smoke_runs::AGY_REQUEST)
                .replace("__AGY__", crate::smoke_runs::AGY_PANE),
            Mode::RealClaude => crate::smoke_runs::REAL_PROBE.replace("__TYPED__", crate::smoke_runs::REAL_REQUEST),
        };
        let pane = match crate::smoke_runs::add_pane(host) {
            Ok(pane) => pane,
            Err(error) => {
                host.log.line(&format!("smoke: the run steps' pane was not added: {error}"));
                return false;
            }
        };
        host.flush();
        host.log.line(&format!("smoke: run steps ({:?})", self.mode));
        self.eval(host, &script.replace("__PANE__", &pane));
        true
    }

    /// Adds the files steps' workspace and panes and runs their probe (smoke_files.rs).
    fn start_files(&self, host: &Host) -> bool {
        let Some(host) = host.app.try_state::<Arc<Host>>().map(|h| h.inner().clone()) else { return false };
        match crate::smoke_files::start(&host) {
            Ok(script) => {
                host.flush();
                host.log.line("smoke: files steps");
                self.eval(&host, &script);
                true
            }
            Err(error) => {
                host.log.line(&format!("smoke: the files steps were not set up: {error}"));
                false
            }
        }
    }

    /// Adds the terminal step's shell pane and runs its probe; where the platform draws terminals natively (the Mac's Ghostty), the
    /// native variant (smoke_terminal.rs, M-D), which does not start until it is written.
    fn start_terminal(&self, host: &Host) -> bool {
        let Some(surfaces) = host.app.try_state::<Arc<SurfaceManager>>() else { return false };
        if surfaces.capabilities()["surfaces"]["terminal"] == "native" {
            return crate::smoke_terminal::start_native(host);
        }
        if surfaces.capabilities()["surfaces"]["terminal"] != "web" {
            return false;
        }
        let script = match crate::smoke_terminal::start(host) {
            Ok(script) => script,
            Err(error) => {
                host.log.line(&format!("smoke: the terminal step's pane was not added: {error}"));
                return false;
            }
        };
        host.flush();
        host.log.line("smoke: terminal step");
        self.eval(host, &script);
        true
    }

    /// Writes the result (once) and, with `--smoke-exit`, quits.
    pub fn finish(&self, host: &Host, runs: Option<Value>) {
        let (calls, probe, main_active, terminal, files, history) = {
            let mut progress = self.progress();
            if progress.finished {
                return;
            }
            progress.finished = true;
            let probe = progress.main.clone().unwrap_or_else(|| json!({"error": "no probe report"}));
            (
                progress.calls.clone(),
                probe,
                progress.main_active.clone(),
                progress.terminal.clone(),
                progress.files.clone(),
                progress.history.clone(),
            )
        };
        let (frame, create, create_location, git_progress, reload) = {
            let progress = self.progress();
            (
                progress.frame.clone(),
                progress.create.clone(),
                progress.create_location.clone(),
                progress.git_progress,
                progress.reload.clone(),
            )
        };
        let (snapshot_sent, shell_sent) = {
            let progress = self.progress();
            (progress.snapshot_sent, progress.shell_sent)
        };
        // The W6 steps' reports: settings, setup, accounts, updates.
        let w6 = {
            let progress = self.progress();
            (progress.settings.clone(), progress.setup.clone(), progress.accounts.clone(), progress.updates.clone())
        };
        let (theme, active, workspaces, panes) = {
            let view = host.view();
            let s = &view.snapshot;
            (s.theme().to_string(), s.active_session_id.clone(), s.workspaces().len(), s.sessions().len())
        };
        let count = |key: &str| probe.get(key).and_then(Value::as_u64).unwrap_or(0);
        let text = |key: &str| probe.get(key).and_then(Value::as_str).unwrap_or("");
        let click_target = probe.get("clickTarget").and_then(Value::as_str);
        // The later steps select their own panes: the click's pane must have been the host's active one when the first probe reported.
        let host_selected = click_target.map(|target| main_active.as_deref() == Some(target));
        let up = ["getLocale", "ready"].iter().all(|m| calls.iter().any(|c| c == m));
        let mut checks = json!({
            "getLocaleAndReady": up,
            "snapshotAndShellSent": snapshot_sent && shell_sent,
            "paneTitles": count("paneTitles") > 0,
            "selectedPane": !text("selectedTitle").is_empty(),
            "theme": text("theme") == theme,
            "clickSelected": click_target.is_none() || probe.get("clickSelected") == Some(&Value::Bool(true)),
            "hostSelected": host_selected.unwrap_or(true),
            "noProbeError": probe.get("error").is_none(),
        });
        // Which step each check came from (the Mac's pending steps are judged by it).
        let mut steps: Vec<(String, Vec<String>)> = Vec::new();
        attribute(&checks, &mut steps, "probe");
        let merge = |checks: &mut Value, more: Value| {
            if let (Some(checks), Some(more)) = (checks.as_object_mut(), more.as_object()) {
                checks.extend(more.clone());
            }
        };
        match &frame {
            Some(frame) => merge(&mut checks, crate::smoke_frame::frame_checks(frame)),
            None if self.mode != Mode::RealClaude => merge(&mut checks, json!({"frameSidebar": false})),
            None => {}
        }
        attribute(&checks, &mut steps, "frame");
        if let Some(create) = &create {
            merge(&mut checks, crate::smoke_frame::create_checks(create, host, create_location.as_deref(), git_progress));
        }
        attribute(&checks, &mut steps, "gitCreate");
        match &reload {
            Some(reload) => merge(&mut checks, crate::smoke_frame::reload_checks(reload)),
            None if self.mode != Mode::RealClaude => merge(&mut checks, json!({"dialogResentAfterReload": false})),
            None => {}
        }
        attribute(&checks, &mut steps, "reload");
        if native_terminal(host) {
            // The Mac's Ghostty step (smoke_terminal.rs' native variant, M-D): a step that did not report fails every one of its checks.
            if self.mode != Mode::RealClaude {
                let missing = json!({"error": "the native terminal step did not report"});
                for (name, passed) in crate::smoke_terminal::native_checks(terminal.as_ref().unwrap_or(&missing)) {
                    checks[name] = json!(passed && terminal.is_some());
                }
            }
        } else if let Some(terminal) = &terminal {
            for (name, passed) in crate::smoke_terminal::checks(terminal) {
                checks[name] = json!(passed);
            }
        }
        attribute(&checks, &mut steps, "terminal");
        if self.mode != Mode::RealClaude {
            let empty = json!({"error": "the files steps did not report"});
            for (name, passed) in crate::smoke_files::checks(host, files.as_ref().unwrap_or(&empty)) {
                checks[name] = json!(passed);
            }
            attribute(&checks, &mut steps, "files");
            let missing = json!({"error": "the history step did not report"});
            for (name, passed) in crate::smoke_history::checks(history.as_ref().unwrap_or(&missing)) {
                checks[name] = json!(passed);
            }
            attribute(&checks, &mut steps, "history");
            // The W6 steps: a step that did not start (or did not report) fails every one of its checks (review L7).
            let parts: [(&Option<Value>, StepChecks, &str); 4] = [
                (&w6.0, crate::smoke_settings::checks, "settings"),
                (&w6.1, crate::smoke_setup::checks, "setup"),
                (&w6.2, crate::smoke_accounts::checks, "accounts"),
                (&w6.3, crate::smoke_updates::checks, "updates"),
            ];
            for (report, part, step) in parts {
                let missing = json!({"error": format!("the {step} step did not report")});
                for (name, passed) in part(report.as_ref().unwrap_or(&missing)) {
                    checks[name] = json!(passed && report.is_some());
                }
                attribute(&checks, &mut steps, step);
            }
            // The delegation step (W7-D): its checks, from its report or from the error that it did not report.
            let delegation = self.progress().delegation.clone();
            let missing = json!({"error": "the delegation step did not report"});
            for (name, passed) in crate::smoke_delegation::checks(delegation.as_ref().unwrap_or(&missing)) {
                checks[name] = json!(passed && delegation.is_some());
            }
            attribute(&checks, &mut steps, "delegation");
            // W8's steps: each remote part's checks from its report under its key, then the companion's (a part or step that did not
            // report fails every one of its checks).
            let remote = self.progress().remote.clone();
            for (key, _, part) in REMOTE_PARTS {
                let report = remote.as_ref().and_then(|r| r.get(key)).filter(|r| !r.is_null()).cloned();
                let missing = json!({"error": format!("the remote step's {key} part did not report")});
                for (name, passed) in part(report.as_ref().unwrap_or(&missing)) {
                    checks[name] = json!(passed && report.is_some());
                }
            }
            attribute(&checks, &mut steps, "remote");
            let companion = self.progress().companion.clone();
            let missing = json!({"error": "the companion step did not report"});
            for (name, passed) in crate::smoke_companion::checks(companion.as_ref().unwrap_or(&missing)) {
                checks[name] = json!(passed && companion.is_some());
            }
            attribute(&checks, &mut steps, "companion");
            // W9's steps: each one's checks from its report under its key (a step that did not report fails every one of them).
            let w9 = self.progress().w9.clone();
            for (_, key, _, part) in W9_STEPS {
                let report = w9.get(key).cloned().flatten();
                let missing = json!({"error": format!("the {key} step did not report")});
                for (name, passed) in part(report.as_ref().unwrap_or(&missing)) {
                    checks[name] = json!(passed && report.is_some());
                }
                attribute(&checks, &mut steps, key);
            }
        }
        // While W9 is under way, the page may meet only the methods its parts have not answered yet (plan D20; README's table).
        let unimplemented = host.unimplemented_methods();
        checks["unimplementedAllowed"] = json!(unimplemented.iter().all(|method| ALLOWED_UNIMPLEMENTED.contains(&method.as_str())));
        attribute(&checks, &mut steps, "unimplemented");
        match self.mode {
            Mode::Page => {}
            Mode::FakeClaude => {
                let acp = self.progress().acp.clone().unwrap_or_else(|| json!({"error": "the ACP step did not report"}));
                for (name, passed) in crate::smoke_acp::checks(host, &acp) {
                    checks[name] = json!(passed);
                }
                attribute(&checks, &mut steps, "acp");
                let empty = json!({"error": "the run steps did not report"});
                let r = runs.as_ref().unwrap_or(&empty);
                let yes = |key: &str| r.get(key) == Some(&Value::Bool(true));
                let text = |key: &str| r.get(key).and_then(Value::as_str).unwrap_or("");
                checks["runsNoError"] = json!(r.get("error").is_none());
                checks["composerSend"] = json!(yes("available") && yes("typed") && yes("drafted") && text("outcome") == "started");
                checks["permissionCard"] = json!(yes("permissionShown") && text("permissionTool") == "Bash");
                checks["planCard"] = json!(yes("planShown") && r.get("answers").and_then(Value::as_u64).unwrap_or(0) >= 2);
                checks["mightyDiagram"] = json!(
                    yes("requestBlock")
                        && matches!(text("status"), "completed" | "idle")
                        && !text("resultText").is_empty()
                        && r.get("agentCards").and_then(Value::as_u64).unwrap_or(0) + r.get("blocks").and_then(Value::as_u64).unwrap_or(0)
                            >= 3
                );
                checks["progressSteps"] = json!(
                    yes("stepsDrawn") && text("stepsCount") == "1/3" && text("stepsStates") == "done,current,pending" && yes("stepsHidden")
                );
                checks["stopRun"] = json!(yes("stopPermission") && yes("stopped") && yes("stopCardGone"));
                checks["piQuestionCard"] = json!(
                    yes("piAvailable")
                        && text("piOutcome") == "started"
                        && text("piQuestion") == "What is your favorite color?"
                        && text("piAnswer") == "Red"
                        && matches!(text("piStatus"), "completed" | "idle")
                        && text("piReply") == "Your favorite color is red."
                        && yes("piCardGone")
                );
                checks["antigravityCard"] = json!(
                    yes("agyAvailable")
                        && yes("agyCard")
                        && text("agyHeading").contains("RunCommand")
                        && text("agyTarget") == "echo hi"
                        && text("agyRule").contains("command(echo hi)")
                        && yes("agyAlwaysEnabled")
                );
                checks["antigravityAllowOnce"] = json!(yes("agyContinued") && yes("agyCardGone") && yes("agySettingsRestored"));
            }
            Mode::RealClaude => {
                let empty = json!({"error": "the real request did not report"});
                let r = runs.as_ref().unwrap_or(&empty);
                let state = r.get("state").cloned().unwrap_or(Value::Null);
                let usage = state.get("usage").filter(|u| u.is_object());
                let tokens = usage.is_some_and(|u| {
                    ["outputTokens", "totalTokens", "inputTokens"].iter().any(|k| u.get(*k).is_some_and(Value::is_number))
                });
                checks["realNoError"] = json!(r.get("error").is_none());
                checks["realSent"] =
                    json!(r.get("available") == Some(&Value::Bool(true)) && r.get("outcome").and_then(Value::as_str) == Some("started"));
                checks["realCompleted"] = json!(matches!(state.get("status").and_then(Value::as_str), Some("completed" | "idle")));
                checks["realResultOk"] = json!(r.get("resultBlock").and_then(Value::as_str).is_some());
                checks["realUsage"] = json!(tokens);
            }
        }
        attribute(&checks, &mut steps, "runs");
        // localeKeyLeakScan, last (C# `RunWebLiveSmoke`'s last step): every W6 box's words, none a raw locale key.
        let leak_scan = (self.mode != Mode::RealClaude).then(|| shared(host).map(|host| crate::smoke_settings::leak_scan(&host)));
        if let Some(scan) = &leak_scan {
            checks["localeKeyLeakScan"] = json!(scan.as_ref().is_some_and(crate::smoke_settings::leak_scan_passed));
        }
        attribute(&checks, &mut steps, "localeKeyLeakScan");
        let passed = checks.as_object().is_some_and(|c| c.values().all(|v| v == &Value::Bool(true)));
        // The Mac's gate while its parts are written: every check outside MAC_PENDING_STEPS passed (Windows: `passed` alone).
        let failed_outside_pending = failed_outside(&checks, &steps, &MAC_PENDING_STEPS);
        let mac = crate::platform::smoke_gate_skips_pending();
        let exit_passed = if mac { failed_outside_pending.is_empty() } else { passed };
        let mut result = json!({
            "passed": passed,
            "checks": checks,
            "probe": probe,
            "frame": frame,
            "terminal": terminal,
            "files": files,
            "framesServed": crate::smoke_files::served(host),
            "history": history,
            "gitCreate": create,
            "gitProgressPushes": git_progress,
            "reload": reload,
            "settings": w6.0,
            "setup": w6.1,
            "accounts": w6.2,
            "updates": w6.3,
            "localeKeyLeakScan": leak_scan.flatten(),
            "acp": self.progress().acp.clone(),
            "delegation": self.progress().delegation.clone(),
            "remote": self.progress().remote.clone(),
            "companion": self.progress().companion.clone(),
            "runs": runs,
            "mode": format!("{:?}", self.mode),
            "host": {"workspaces": workspaces, "panes": panes, "theme": theme, "activeSessionId": active, "language": host.locale.language()},
            "load": self.load,
            "calls": calls,
            "unimplemented": unimplemented,
            "elapsedMs": self.started.elapsed().as_millis() as u64,
            "dataDir": host.data_dir,
            "version": host.version,
        });
        // W9's steps' reports, each under its key (null when it did not run).
        let w9 = self.progress().w9.clone();
        for (_, key, _, _) in W9_STEPS {
            result[key] = w9.get(key).cloned().flatten().unwrap_or(Value::Null);
        }
        // Mac stage: each step's checks, the steps still pending on the Mac, and what failed outside them (the exit code's verdict there).
        if mac {
            result["steps"] =
                json!(steps.iter().map(|(step, names)| (step.clone(), json!(names))).collect::<serde_json::Map<String, Value>>());
            result["macPending"] = json!(MAC_PENDING_STEPS);
            result["failedOutsidePending"] = json!(failed_outside_pending);
            result["passedOutsidePending"] = json!(exit_passed);
        }
        if let Some(parent) = self.out.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(error) = std::fs::write(&self.out, serde_json::to_vec_pretty(&result).unwrap_or_default()) {
            host.log.line(&format!("smoke: cannot write {}: {error}", self.out.display()));
        }
        host.log.line(&format!("smoke: {} ({})", if passed { "passed" } else { "FAILED" }, self.out.display()));
        if mac {
            host.log.line(&format!("smoke: outside the Mac's pending steps {}", if exit_passed { "passed" } else { "FAILED" }));
        }
        crate::smoke_history::remove_fixtures();
        if self.exit {
            host.app.exit(if exit_passed { 0 } else { 1 });
        }
    }

    /// Fails the smoke when it has no result in time. A help capture or perf scenario has no limit once it took the page over (C#'s has
    /// none either; its script stops a run that outlasts 30 minutes), but fails when the page never came up for it ([`SPECIAL_PAGE`]).
    pub fn watch(host: Arc<Host>) {
        std::thread::spawn(move || {
            if let Some(smoke) = host.smoke.as_ref().filter(|smoke| smoke.is_special()) {
                std::thread::sleep(SPECIAL_PAGE);
                if !smoke.progress().special_begun {
                    let error = format!("the page did not come up within {} s", SPECIAL_PAGE.as_secs());
                    host.log.line(&format!("smoke: {error}"));
                    let failure = json!({"passed": false, "phase": "startup", "error": error, "aiRequestSent": false});
                    let _ = std::fs::write(&smoke.out, serde_json::to_vec_pretty(&failure).unwrap_or_default());
                    smoke.end(&host, false);
                }
                return;
            }
            let Some(timeout) = host.smoke.as_ref().map(Smoke::timeout) else { return };
            std::thread::sleep(timeout);
            if let Some(smoke) = &host.smoke {
                let error = json!({"error": format!("no result after {} s", timeout.as_secs())});
                let stage = {
                    let mut progress = smoke.progress();
                    let stage = progress.stage;
                    if let Some(report) = progress.report(stage) {
                        *report = Some(error.clone());
                    }
                    stage
                };
                smoke.finish(&host, (stage == Stage::Runs).then_some(error));
            }
        });
    }
}

/// How long a help capture or perf scenario waits for the page to come up before it fails (W9-H).
const SPECIAL_PAGE: Duration = Duration::from_secs(180);

/// Whether the platform draws terminals natively (the Mac's Ghostty): the terminal step is then its native variant.
fn native_terminal(host: &Host) -> bool {
    host.app.try_state::<Arc<SurfaceManager>>().is_some_and(|surfaces| surfaces.capabilities()["surfaces"]["terminal"] == "native")
}

/// A step's checks from its report (W6's, W8's remote parts).
type StepChecks = fn(&Value) -> Vec<(&'static str, bool)>;

/// A step's start (W8's remote parts): false when it does not run.
type StepStart = fn(&Arc<Host>) -> bool;

/// The host as the steps that outlive the call share it.
fn shared(host: &Host) -> Option<Arc<Host>> {
    host.app.try_state::<Arc<Host>>().map(|h| h.inner().clone())
}

/// The page steps' view of the host: `state` (the frame step's, smoke_frame.rs) and `dialogs` (the dialogs whose flows still wait:
/// the reload step's, none once the redrawn sheet was answered). Smoke only.
#[tauri::command]
pub async fn smoke_surface(pane_id: String, action: String, host: tauri::State<'_, Arc<Host>>) -> Result<Value, String> {
    let _ = pane_id;
    if host.smoke.is_none() {
        return Err("smoke only".into());
    }
    match action.as_str() {
        "state" => Ok(crate::smoke_frame::state(&host)),
        "dialogs" => Ok(json!({"open": crate::dialogs::of(&host).open_count(), "dialogOpen": host.view().dialog_open})),
        other => Err(format!("unknown smoke action {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mac stage: the checks are grouped by the step that added them, and the Mac's gate fails only on a check outside the pending
    /// steps (a pending step's failures are allowed; Windows' exit code stays `passed`).
    #[test]
    fn the_mac_gate_fails_only_outside_the_pending_steps() {
        let mut steps = Vec::new();
        let mut checks = json!({"getLocaleAndReady": true, "paneTitles": false});
        attribute(&checks, &mut steps, "probe");
        checks["frameSidebar"] = json!(false);
        attribute(&checks, &mut steps, "frame");
        attribute(&checks, &mut steps, "empty");
        let names = |list: &[&str]| list.iter().map(|name| name.to_string()).collect::<Vec<_>>();
        assert_eq!(
            steps,
            vec![("probe".to_string(), names(&["getLocaleAndReady", "paneTitles"])), ("frame".to_string(), names(&["frameSidebar"]))],
            "a step that added nothing has no group"
        );
        assert!(failed_outside(&checks, &steps, &["probe", "frame"]).is_empty());
        assert_eq!(failed_outside(&checks, &steps, &["probe"]), names(&["frameSidebar"]));
        assert_eq!(failed_outside(&checks, &steps, &[]), names(&["paneTitles", "frameSidebar"]));
        let unique: std::collections::HashSet<&str> = MAC_PENDING_STEPS.iter().copied().collect();
        assert_eq!(unique.len(), MAC_PENDING_STEPS.len());
    }
}
