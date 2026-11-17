//! `--smoke-test`: checks the page and the host together without anyone watching. Once the page has called `getLocale` and `ready`
//! and the host has pushed it the snapshot and the frame model, a probe script runs in the page: it waits for the sidebar's pane
//! titles, reads the DOM (pane titles, the selected pane, the theme, tabs, header, status bar), clicks another pane's sidebar row and
//! waits for the page to show it selected, then hands what it saw to the `smoke_probe` command.
//!
//! Then the page's steps, in this order ([`STEPS`]), each where its platform has what it checks:
//!
//! - the frame (smoke_frame.rs: the sidebar, a row click, the add menu's terminal entry, tab switches, the columns preset);
//! - where the platform draws browser panes natively, the browser surface (the NativeSlot protocol): the host adds a browser pane in a
//!   workspace of its own to the in-memory state and makes it active; a probe types the address of a local fixture page (served on
//!   127.0.0.1 by this process) into the pane's address field and waits until the native view shows it, checks that the view's real
//!   bounds (read back from the OS) are the slot's in device pixels, has the host push a page dialog and checks that the view hides
//!   under it and shows again once it closes, then the history's address, back, a refused popup and a menu over the view;
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
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mighty_core::AppSnapshot;
use mighty_platform::SurfaceKind;
use serde_json::{Value, json};
use tauri::Manager;

use crate::host::{Host, WINDOW};
use crate::surface::SurfaceManager;

pub use crate::smoke_runs::RECORDING;

/// Which steps follow the first probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The page and, where the platform draws browsers natively, the browser surface.
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
    Browser,
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
const STEPS: [Stage; 21] = [
    Stage::Frame,
    Stage::Browser,
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

/// The browser-surface step's probe; `__PANE__` and `__FIXTURE__` are filled in (both plain ASCII, no quotes).
const BROWSER_PROBE: &str = r#"(async () => {
  const until = async (test, ms) => { const end = performance.now() + ms; while (performance.now() < end) { const v = await test(); if (v) return v; await new Promise((r) => setTimeout(r, 50)); } return test(); };
  const invoke = (command, args) => window.__TAURI_INTERNALS__.invoke(command, args);
  const paneId = '__PANE__', fixture = '__FIXTURE__';
  const surface = () => invoke('smoke_surface', { paneId, action: 'inspect' });
  const report = { paneId, fixture };
  try {
    // The Mac's engine of the launch (webkit or cef; none on Windows).
    const engine = (await invoke('smoke_surface', { paneId, action: 'popups' })).engine;
    if (engine) report.browserEngine = engine;
    // This pane's slot, looked up again whenever it is measured (the page may draw it anew).
    const ofPane = (selector) => document.querySelector('.pane-slot[data-pane-id="' + paneId + '"] ' + selector);
    const slotNow = () => ofPane('[data-testid=browser-slot]');
    const slot = await until(slotNow, 15000);
    report.slot = !!slot;
    // Enabled once the host's view opened (status `ready`): a WebView2 in a profile of its own, which a loaded machine can take well over
    // 10 s to start. Waited for that long; without it the step stops with its reason instead of typing into nothing.
    const address = await until(() => { const field = ofPane('[data-testid=browser-address]'); return field && !field.disabled ? field : null; }, 60000);
    report.addressField = !!address;
    if (!address) throw new Error('the browser view never became ready: ' + (ofPane('[data-testid=browser-notice]')?.textContent || 'still starting'));
    // Typed as a person would: the field's value, its input event, then Enter (BrowserView's keyDown sends browserNavigate).
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(address, fixture);
    address.dispatchEvent(new Event('input', { bubbles: true }));
    address.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    const loaded = await until(async () => { const s = await surface(); return s && s.visible && (s.url || '').startsWith(fixture) ? s : null; }, 30000);
    report.loaded = !!loaded;
    report.url = loaded?.url ?? null;
    // The slot in device pixels as the page reports it (src/slot/bounds.ts), against the view's own bounds from the OS: both read again
    // until they agree (the page may still be laying the slot out, the host still placing the view).
    const ratio = window.devicePixelRatio || 1;
    const expected = () => { const box = (slotNow() ?? slot).getBoundingClientRect(); const x = Math.round(box.left * ratio), y = Math.round(box.top * ratio); return { x, y, width: Math.round((box.left + box.width) * ratio) - x, height: Math.round((box.top + box.height) * ratio) - y }; };
    let placed = null;
    report.boundsMatch = !!(await until(async () => { report.expected = expected(); placed = await surface(); return !!placed && ['x', 'y', 'width', 'height'].every((k) => Math.abs(placed.bounds[k] - report.expected[k]) <= 1); }, 10000));
    report.actual = placed?.bounds ?? null;
    report.ratio = ratio;
    await invoke('smoke_surface', { paneId, action: 'dialog' });
    report.dialogShown = !!(await until(() => document.querySelector('[data-testid=dialog]'), 5000));
    report.hiddenUnderDialog = !!(await until(async () => { const s = await surface(); return s && !s.visible; }, 5000));
    await invoke('smoke_surface', { paneId, action: 'dialogClose' });
    report.dialogClosed = !!(await until(() => !document.querySelector('[data-testid=dialog]'), 5000));
    report.shownAfterDialog = !!(await until(async () => { const s = await surface(); return s && s.visible; }, 5000));
    // The address the field shows came back from the host's history as a push.
    const field = () => ofPane('[data-testid=browser-address]')?.value ?? '';
    // ...once the host's history has it (the field alone also shows what was typed).
    report.navigated = !!(await until(async () => field() === fixture && (await surface())?.history === fixture, 10000));
    // A second page, then the page's back button: the host's history and the engine go back to the first.
    const second = fixture.replace('/fixture', '/second'), popup = fixture.replace('/fixture', '/popup');
    // Typed into the field the page shows now (it may have drawn the pane anew since the first address).
    const go = (url) => { const input = ofPane('[data-testid=browser-address]') ?? address; Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(input, url); input.dispatchEvent(new Event('input', { bubbles: true })); input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true })); };
    report.firstFieldKept = address.isConnected;
    go(second);
    report.onSecond = !!(await until(async () => field() === second && (await surface())?.history === second, 10000));
    if (!report.onSecond) report.secondSeen = { field: field(), view: (await surface())?.url ?? null };
    const back = await until(() => { const b = ofPane('button.browser-nav'); return b && !b.disabled ? b : null; }, 10000);
    const loadsBefore = await invoke('smoke_surface', { paneId, action: 'loads' });
    back?.click();
    // CEF's back loads the first page again: the fixture server must see it (its own history alone would say it went back).
    const reloaded = async () => engine !== 'cef' || (await invoke('smoke_surface', { paneId, action: 'loads' })).fixture > loadsBefore.fixture;
    report.wentBack = report.onSecond && !!(await until(async () => { const s = await surface(); return field() === fixture && s && (s.url || '') === fixture && (await reloaded()); }, 10000));
    if (!report.wentBack) report.backSeen = { clicked: !!back, field: field(), view: (await surface())?.url ?? null };
    // window.open from a page: refused and counted, no window opens, and the page stays where it is.
    const before = await invoke('smoke_surface', { paneId, action: 'popups' });
    go(popup);
    report.popupPage = !!(await until(() => field() === popup, 10000));
    // CEF's bridge cancels the popup without a word: its page is given the time to ask, then nothing may have opened.
    const counted = engine !== 'cef';
    if (!counted) await new Promise((r) => setTimeout(r, 1500));
    const after = await until(async () => { const p = await invoke('smoke_surface', { paneId, action: 'popups' }); return !counted || p.refused > before.refused ? p : null; }, 10000);
    const stayed = await surface();
    report.popups = after;
    // Never loaded (the fixture server saw no request for the popup's address) and no window more (Tauri's, and on the Mac AppKit's).
    report.popupBlocked = !!after && after.webviews === before.webviews && (stayed?.url || '') === popup && !after.targetRequested
      && (after.windows === undefined || after.windows === before.windows);
    // A page menu over the slot (the tab's menu at a point inside it): the view steps aside, and comes back once the menu closes.
    const box2 = (slotNow() ?? slot).getBoundingClientRect();
    report.menuAt = { x: Math.round(box2.left + 20), y: Math.round(box2.top + 20) };
    const handle = await until(() => document.querySelector('[data-testid=tab][data-tab-id="' + paneId + '"] .tab-handle'), 5000);
    handle?.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, button: 2, clientX: box2.left + 20, clientY: box2.top + 20 }));
    report.menuOpened = !!handle && !!(await until(() => document.querySelector('[data-testid=menu]'), 5000));
    report.steppedAside = report.menuOpened && !!(await until(async () => { const s = await surface(); return s && !s.visible; }, 5000));
    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
    document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
    report.menuClosed = report.steppedAside && !!(await until(async () => { const s = await surface(); return !document.querySelector('[data-testid=menu]') && s && s.visible; }, 5000));
    // The Mac: the engine's own views in two workspace profiles, a close and reopens (smoke.rs `browser_profiles`).
    if (engine) report.profiles = await invoke('smoke_surface', { paneId, action: 'profiles' });
    // What the fixture page found of the app in the pane (`/globals?…`, each time it loaded).
    if (engine) report.globals = (await invoke('smoke_surface', { paneId, action: 'loads' })).globals;
  } catch (error) {
    report.error = String(error);
  }
  await invoke('smoke_probe', { probe: report });
})();"#;

/// The fixture page the browser pane loads.
/// It tells the fixture server what it finds of the app's IPC in the pane (review M-E 1: on the Mac, nothing).
const FIXTURE: &str = "<!doctype html><html><head><meta charset=utf-8><title>Mighty smoke fixture</title></head>\
<body style=\"margin:0;background:#1f7a52;color:#fff;font:28px system-ui\"><p style=\"padding:24px\">Mighty Code browser surface fixture</p>\
<script>fetch('/globals?tauri=' + typeof window.__TAURI_INTERNALS__ + '&ipc=' + typeof window.ipc + '&handler=' + typeof window.webkit?.messageHandlers?.ipc);</script></body></html>";
/// The second page (`/second`), for back.
const SECOND: &str = "<!doctype html><html><head><meta charset=utf-8><title>Mighty smoke second</title></head>\
<body style=\"margin:0;background:#20508f;color:#fff;font:28px system-ui\"><p style=\"padding:24px\">Second page</p></body></html>";
/// A page that asks for a new window as it loads (`/popup`): the host refuses it, and its address (`/popup-target`) is never asked for.
const POPUP: &str = "<!doctype html><html><head><meta charset=utf-8><title>Mighty smoke popup</title></head>\
<body style=\"margin:0;background:#7a1f52;color:#fff;font:28px system-ui\"><p style=\"padding:24px\">Popup page</p>\
<script>window.open('/popup-target', '_blank');</script></body></html>";

/// The ids of the pane and workspace the step adds (UUID-shaped, as the state's ids are).
const PANE: &str = "5a0e0b0b-0000-4000-8000-00000000b10b";
const WORKSPACE: &str = "5a0e0b0b-0000-4000-8000-00000000a0a0";
/// The id of the page dialog the step opens over the browser pane.
const DIALOG: u64 = 990_001;

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
    browser: Option<Value>,
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
            Stage::Browser => Some(&mut self.browser),
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
            // The frame, browser, gitCreate and reload steps add up to two minutes, the terminal and history steps a minute each.
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

    /// Runs `script` in the page (not `get_webview_window`: once a browser pane has a child webview, the window holds two).
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
            Stage::Browser => page_steps && self.start_browser(host),
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

    /// Adds the browser pane and runs the second probe; false where the platform has no native browser surface.
    fn start_browser(&self, host: &Host) -> bool {
        let Some(surfaces) = host.app.try_state::<Arc<SurfaceManager>>() else { return false };
        if surfaces.capabilities()["surfaces"]["browser"] != "native" {
            return false;
        }
        // The browser engine counts as on for this smoke only (C# `ForceBrowserEngineForSmoke`).
        surfaces.force_engine();
        let fixture = match serve_fixture() {
            Ok(fixture) => fixture,
            Err(error) => {
                host.log.line(&format!("smoke: no fixture server: {error}"));
                return false;
            }
        };
        if let Err(error) = add_browser_pane(host) {
            host.log.line(&format!("smoke: the browser pane was not added: {error}"));
            return false;
        }
        host.flush();
        host.log.line(&format!("smoke: browser surface step ({fixture})"));
        self.eval(host, &BROWSER_PROBE.replace("__PANE__", PANE).replace("__FIXTURE__", &fixture));
        true
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
        let (calls, probe, browser, main_active, terminal, files, history) = {
            let mut progress = self.progress();
            if progress.finished {
                return;
            }
            progress.finished = true;
            let probe = progress.main.clone().unwrap_or_else(|| json!({"error": "no probe report"}));
            (
                progress.calls.clone(),
                probe,
                progress.browser.clone(),
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
        if let Some(browser) = &browser {
            let yes = |key: &str| browser.get(key) == Some(&Value::Bool(true));
            checks["browserSurface"] = json!(
                browser.get("error").is_none()
                    && yes("loaded")
                    && yes("boundsMatch")
                    && yes("dialogShown")
                    && yes("hiddenUnderDialog")
                    && yes("shownAfterDialog")
            );
            checks["browserNavigated"] = json!(yes("navigated"));
            checks["browserBack"] = json!(yes("onSecond") && yes("wentBack"));
            checks["browserPopupBlocked"] = json!(yes("popupPage") && yes("popupBlocked"));
            checks["browserMenuOver"] = json!(yes("menuOpened") && yes("steppedAside") && yes("menuClosed"));
            // The Mac's engine views in two profiles (the Swift browser smoke's keys; `browser_profiles`).
            if crate::platform::browser_engine().is_some() {
                let profiles = browser.get("profiles").cloned().unwrap_or(Value::Null);
                let yes = |key: &str| profiles.get(key) == Some(&Value::Bool(true));
                checks["browserTwoPanesRenderJavaScript"] = json!(yes("twoPanesRenderJavaScript"));
                checks["browserIndependentProfiles"] = json!(yes("independentProfiles"));
                checks["browserClosingOnePaneKeepsOtherAlive"] = json!(yes("closingOnePaneKeepsOtherAlive"));
                checks["browserReopenCycles"] = json!(yes("reopenCycles"));
                // Review M-E 1: a page from the web in a pane finds nothing of the app's IPC (Tauri's scripts and handlers removed).
                let globals: Vec<&str> =
                    browser.get("globals").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
                checks["browserPaneWithoutAppIpc"] = json!(
                    !globals.is_empty() && globals.iter().all(|seen| *seen == "/globals?tauri=undefined&ipc=undefined&handler=undefined")
                );
            }
        }
        attribute(&checks, &mut steps, "browser");
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
            "browser": browser,
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

/// Serves [`FIXTURE`] on 127.0.0.1 for the rest of the run; answers its address.
fn serve_fixture() -> std::io::Result<String> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let address = format!("http://127.0.0.1:{}/fixture", listener.local_addr()?.port());
    std::thread::Builder::new().name("smoke-fixture".into()).spawn(move || {
        // Each connection on a thread of its own: an engine's spare connection that never sends (Chromium preconnects) holds only
        // its own thread for the read's 5 s, never the requests behind it.
        for mut stream in listener.incoming().flatten() {
            let _ = std::thread::Builder::new().name("smoke-fixture-connection".into()).spawn(move || serve_one(&mut stream));
        }
    })?;
    let _ = FIXTURE_ADDRESS.set(address.clone());
    Ok(address)
}

/// One request of the browser fixture.
fn serve_one(stream: &mut std::net::TcpStream) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut request = [0u8; 4096];
    let read = stream.read(&mut request).unwrap_or(0);
    let line = String::from_utf8_lossy(&request[..read]);
    // The Mac's profile check (`browser_profiles`): `/set?pane=<p>` sets its cookie and goes on to `/check?pane=<p>` by script;
    // each such request is recorded with the cookie it sent.
    let path = line.split_whitespace().nth(1).unwrap_or("").to_string();
    let mut set_cookie = String::new();
    let page = if line.starts_with("GET /second") || line.starts_with("GET /popup-target") {
        SECOND.to_string()
    } else if line.starts_with("GET /globals") {
        String::new()
    } else if line.starts_with("GET /popup") {
        POPUP.to_string()
    } else if let Some(pane) = path.strip_prefix("/set?pane=").filter(|p| p.chars().all(|c| c.is_ascii_alphanumeric())) {
        set_cookie = format!("Set-Cookie: {PROFILE_COOKIE}={pane}; Path=/; Max-Age=600\r\n");
        format!(
            "<!doctype html><html><body>set {pane}<script>setTimeout(() => location.href = '/check?pane={pane}', 200)</script></body></html>"
        )
    } else if path.starts_with("/check?pane=") {
        "<!doctype html><html><body>checked</body></html>".to_string()
    } else {
        FIXTURE.to_string()
    };
    let cookie = line
        .lines()
        .find_map(|header| header.strip_prefix("Cookie:").or_else(|| header.strip_prefix("cookie:")))
        .map(|cookie| cookie.trim().to_string());
    FIXTURE_REQUESTS.lock().unwrap_or_else(|p| p.into_inner()).push((path, cookie));
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\n{set_cookie}Connection: close\r\n\r\n{page}",
        page.len()
    );
    let _ = stream.write_all(response.as_bytes());
}

/// The browser fixture's address (`…/fixture`), once served.
static FIXTURE_ADDRESS: std::sync::OnceLock<String> = std::sync::OnceLock::new();
/// Every request the browser fixture was asked (its path and query) and the cookie it sent.
static FIXTURE_REQUESTS: Mutex<Vec<(String, Option<String>)>> = Mutex::new(Vec::new());

/// The fixture's requests whose path and query `matches`.
fn fixture_requests(matches: impl Fn(&str) -> bool) -> Vec<String> {
    FIXTURE_REQUESTS.lock().unwrap_or_else(|p| p.into_inner()).iter().map(|(path, _)| path).filter(|path| matches(path)).cloned().collect()
}
/// The cookie the profile check's `/set` pages set.
const PROFILE_COOKIE: &str = "mightyprofile";

/// The Mac's profile check (the Swift browser smoke's `twoPanesRenderJavaScript`, `independentProfiles`,
/// `closingOnePaneKeepsOtherAlive`, `reopenCycles`), on the launch's engine with views of its own over the window's top left: A in
/// workspace profile 1 sets a cookie and goes on to the check page by its page's script; B in profile 2 and C in profile 1 ask for the
/// check page; A and C must send A's cookie and B none. Then A closes and B still loads, and a view opens and closes three times, each
/// loading. Every view is closed at the end.
async fn browser_profiles(surfaces: &SurfaceManager) -> Value {
    let Some(base) = FIXTURE_ADDRESS.get().and_then(|address| address.strip_suffix("/fixture")).map(str::to_string) else {
        return json!({"error": "no fixture"});
    };
    // Every wait of the check together ends within 45 s (each at most 15 s), however many fail.
    let all = Instant::now() + Duration::from_secs(45);
    let seen = |path: String| async move {
        let deadline = (Instant::now() + Duration::from_secs(15)).min(all);
        loop {
            let found = FIXTURE_REQUESTS.lock().unwrap_or_else(|p| p.into_inner()).iter().find(|(p, _)| *p == path).cloned();
            if let Some((_, cookie)) = found {
                return Some(cookie.unwrap_or_default());
            }
            if Instant::now() > deadline {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    };
    let rect = mighty_platform::DeviceRect { x: 0, y: 0, width: 240, height: 160 };
    let open = |pane: &'static str, profile: &'static str, path: &str| {
        let url = format!("{base}{path}");
        async move { surfaces.open_unowned(pane, profile, &url, rect).await }
    };
    let wanted = format!("{PROFILE_COOKIE}=a");
    let has = |cookie: &Option<String>| cookie.as_deref().is_some_and(|c| c.split(';').any(|part| part.trim() == wanted));
    let mut report = json!({});
    let a = open("smoke-profile-a", "smoke-profile-1", "/set?pane=a").await;
    let a_checked = seen("/check?pane=a".into()).await;
    let b = open("smoke-profile-b", "smoke-profile-2", "/check?pane=b").await;
    let b_checked = seen("/check?pane=b".into()).await;
    let c = open("smoke-profile-c", "smoke-profile-1", "/check?pane=c").await;
    let c_checked = seen("/check?pane=c".into()).await;
    report["opened"] = json!([a.is_ok(), b.is_ok(), c.is_ok()]);
    report["cookies"] = json!({"a": a_checked, "b": b_checked, "c": c_checked});
    report["twoPanesRenderJavaScript"] = json!(a_checked.is_some() && b_checked.is_some());
    report["independentProfiles"] =
        json!(has(&a_checked) && has(&c_checked) && b_checked.as_deref().is_some_and(|c| !c.contains(PROFILE_COOKIE)));
    // A closes; B still navigates.
    let closed = a.as_ref().is_ok_and(|view| view.close().is_ok());
    let b_again = match &b {
        Ok(view) => view.navigate(&format!("{base}/check?pane=b2")).is_ok() && seen("/check?pane=b2".into()).await.is_some(),
        Err(_) => false,
    };
    report["closingOnePaneKeepsOtherAlive"] = json!(closed && b_again);
    // A view opened and closed three times, each loading its page.
    let mut cycles = 0;
    for (pane, path) in
        [("smoke-profile-d1", "/check?pane=d1"), ("smoke-profile-d2", "/check?pane=d2"), ("smoke-profile-d3", "/check?pane=d3")]
    {
        if let Ok(view) = open(pane, "smoke-profile-2", path).await
            && seen(path.into()).await.is_some()
            && view.close().is_ok()
        {
            cycles += 1;
        }
    }
    report["reopenCycles"] = json!(cycles == 3);
    for view in [b, c].into_iter().flatten() {
        let _ = view.close();
    }
    report
}

/// A browser pane in a workspace of its own, made the active one (a change of the in-memory state; saved only where the app may
/// write, which a smoke's profile never is unless asked).
fn add_browser_pane(host: &Host) -> Result<(), String> {
    use mighty_core::Wire;
    let mut view = host.view();
    let mut state: Value = serde_json::from_str(&view.snapshot.to_json()).map_err(|e| e.to_string())?;
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let workspace = json!({"id": WORKSPACE, "name": "Smoke browser", "path": host.data_dir, "createdAt": now});
    let pane = json!({
        "id": PANE, "workspaceId": WORKSPACE, "title": "Smoke browser", "kind": "browser", "status": "idle", "model": "default",
        "provider": "claude", "settings": {"effort": "default", "permissionMode": "manual", "maxTurns": null, "maxBudgetUsd": null},
        "logs": [], "draft": "", "createdAt": now,
    });
    let list = |state: &mut Value, name: &str, item: Value| match state.get_mut(name).and_then(Value::as_array_mut) {
        Some(items) => {
            items.push(item);
            Ok(())
        }
        None => Err(format!("the state has no {name}")),
    };
    list(&mut state, "workspaces", workspace)?;
    list(&mut state, "sessions", pane)?;
    let mut expanded: Vec<Value> = state.get("expandedWorkspaceIds").and_then(Value::as_array).cloned().unwrap_or_default();
    expanded.push(json!(WORKSPACE));
    state["expandedWorkspaceIds"] = json!(expanded);
    state["activeWorkspaceId"] = json!(WORKSPACE);
    state["activeSessionId"] = json!(PANE);
    let next = AppSnapshot::read(&state).map_err(|e| e.to_string())?;
    // The workspace is the smoke's own: the store never approved it, so it is not normalized away but stays in memory as made.
    view.replace(next);
    Ok(())
}

/// The browser step's view of the native surface, and its page dialog: `inspect` → `{bounds, visible, url}` read back from the
/// OS (null before the view exists); `dialog` / `dialogClose` push a confirm dialog over the window and take it away. Smoke only.
#[tauri::command]
pub async fn smoke_surface(pane_id: String, action: String, host: tauri::State<'_, Arc<Host>>) -> Result<Value, String> {
    if host.smoke.is_none() {
        return Err("smoke only".into());
    }
    let surfaces = host.app.try_state::<Arc<SurfaceManager>>().map(|s| s.inner().clone()).ok_or("no native surfaces")?;
    let push = crate::surface::page_push(host.app.clone());
    match action.as_str() {
        "inspect" => Ok(surfaces.inspect(&pane_id, SurfaceKind::Browser).await.map_or(Value::Null, |state| {
            let b = state.bounds;
            json!({"bounds": {"x": b.x, "y": b.y, "width": b.width, "height": b.height}, "visible": state.visible, "url": state.url,
                "history": surfaces.browser_url(&pane_id)})
        })),
        "dialog" => {
            push(json!({"kind": "dialog", "id": DIALOG, "dialog": {
                "type": "confirm", "title": "Smoke", "message": "A page dialog over the browser pane", "close": "Close", "defaultPrimary": false
            }}));
            Ok(Value::Null)
        }
        "dialogClose" => {
            push(json!({"kind": "dialogClose", "id": DIALOG}));
            Ok(Value::Null)
        }
        // The popups the pane's page asked for (all refused) and the webviews the app holds (none more for a popup); on the Mac the
        // engine of the launch too (CEF's bridge cancels a popup without saying so: none is counted there).
        "popups" => {
            let mut popups = json!({"refused": surfaces.popups_refused(&pane_id), "webviews": host.app.webviews().len(),
                "targetRequested": !fixture_requests(|path| path.starts_with("/popup-target")).is_empty()});
            if let Some(engine) = crate::platform::browser_engine() {
                popups["engine"] = json!(engine);
                // AppKit's windows, counted on the main thread.
                let (sender, answer) = tokio::sync::oneshot::channel();
                host.app
                    .run_on_main_thread(move || {
                        let _ = sender.send(crate::platform::native_window_count());
                    })
                    .map_err(|e| e.to_string())?;
                popups["windows"] = json!(answer.await.ok().flatten());
            }
            Ok(popups)
        }
        // The fixture's loads (CEF's back) and what its page found of the app's IPC.
        "loads" => Ok(
            json!({"fixture": fixture_requests(|path| path == "/fixture").len(), "globals": fixture_requests(|path| path.starts_with("/globals?"))}),
        ),
        // The Mac's profile check (`browser_profiles`).
        "profiles" if crate::platform::browser_engine().is_some() => Ok(browser_profiles(&surfaces).await),
        // The frame step's view of the host's state (smoke_frame.rs).
        "state" => Ok(crate::smoke_frame::state(&host)),
        // The dialogs whose flows still wait (the reload step's: none once the redrawn sheet was answered).
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
        attribute(&checks, &mut steps, "browser");
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
