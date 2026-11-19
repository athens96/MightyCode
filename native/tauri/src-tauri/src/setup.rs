//! The setup screen, `openSetup`, the badge (C# MainWindow.Setup.cs:57-551, MainWindow.WebShell.Settings.cs:218-241). Owner: W6-B.
//!
//! The screen's state is mighty-tools' `SetupScreen`, kept here under one lock; the screen (the surface `setup`, place `surface`) draws
//! it with its checklist and bulk install, Settings › 구성 요소 (settings/tools.rs) as a row per item with that item's own install
//! (its confirmation and terminal the screen's), each with mighty-tools' `view` builders.
//!
//! - A list shown (the surface opened, the components box drawn) refreshes the probes (forced: PATH read again) and ticks its first
//!   picks once; while one shows the poll looks every 5 s and probes again when the last probe is 30 s old and something is still
//!   missing. A smoke probes nothing of this PC: only the fixed results its step sets (C# `options.SmokeTest && setupFixedResults is
//!   null`).
//! - The install: the screen's refusals (one runs, elevated, nothing to install), the confirmation (D10), then the script in a
//!   terminal window of its own (the `TerminalLauncher` port), only with `--allow-state-write` and never in a smoke
//!   (`Host::may_change_machine`); its markers watched every 2 s, its items probed every 5 s, everything probed again (forced) when it
//!   ends, and the screen brought back while the app is in front, else the status bar's badge.
//! - At launch (the first frame model) the screen opens when no agent CLI can run or Git is missing, unless never-show was chosen
//!   (C# `PresentSetupIfNeeded`); never in a smoke.
//! - Never-show is stored (HKCU `Software\MightyCode`, through the `PrefsStore`) only with `--allow-state-write` and never in a
//!   smoke.
//!
//! Locks: the screen's lock is taken after the state's (the frame model and the cards read it with the state locked), never before:
//! nothing here calls `host.view()` or a builder while it holds the screen.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use mighty_accounts::accounts::CliLoginOption;
use mighty_core::model::{AppSnapshot, RunSession};
use mighty_native::surface::{DialogResult, Next, SurfaceModel};
use mighty_native::ui::{El, Local, Ui};
use mighty_native::view::{Place, View};
use mighty_platform::{Paths, PrefsStore, SetupFiles, TerminalLaunch, TerminalLauncher};
use mighty_run::model::ProviderRuntime;
use mighty_tools::setup::catalog::SetupPlatform;
use mighty_tools::setup::probe::SetupFirstRun;
use mighty_tools::setup::screen::{Gate, InstallStart, ProbeContext, RUN_PROBE_INTERVAL, SetupScreen, WATCH_INTERVAL, WatchTick};
use mighty_tools::view::components::{self, ComponentActions, ComponentsActions, ComponentsModel, ComponentsView};
use mighty_tools::view::{self, SETUP_SURFACE, SetupActions, SetupExtras};
use mighty_updates::view::{CliUpdateActions, CliUpdateBox};
use tauri::Manager;

use crate::host::Host;

/// What the frame model shows of the setup (C# `setupBadge` and `SetupNeeded`, the shell's `status.setup` and `setupNeeded`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ShellFlags {
    pub badge: bool,
    pub needed: bool,
}

/// How long the components box counts as shown after it was last drawn (Settings draws its boxes on every 250 ms read).
const COMPONENTS_SHOWN_FOR: Duration = Duration::from_secs(2);

/// What a refused install says until W9 lets this build change the machine (W6 defaults: installs only with the flag), the words the
/// accounts and the updates say too.
pub fn verification_mode() -> String {
    mighty_run::strings::get("windows.notifications.statusVerificationMode")
}

/// The setup's state (C# MainWindow.Setup.cs's fields), managed by the app.
pub struct SetupState {
    pub screen: Mutex<SetupScreen>,
    /// The script folder, markers and elevation port.
    pub files: Arc<dyn SetupFiles>,
    /// The poll turns (C# `setupPoll`).
    polling: AtomicBool,
    /// The launch's look happened (or was skipped).
    presented: AtomicBool,
    /// When Settings › 구성 요소' list was last drawn.
    components_drawn: Mutex<Option<Instant>>,
    /// 구성 요소' opened rows and filter, kept while the app runs (every row starts closed).
    components_view: Mutex<ComponentsView>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The setup's state, made on first use.
pub fn of(host: &Host) -> Arc<SetupState> {
    if let Some(state) = host.app.try_state::<Arc<SetupState>>() {
        return state.inner().clone();
    }
    host.app.manage(Arc::new(SetupState {
        screen: Mutex::new(SetupScreen::for_platform(platform(host))),
        files: files_port(host),
        polling: AtomicBool::new(false),
        presented: AtomicBool::new(false),
        components_drawn: Mutex::new(None),
        components_view: Mutex::new(ComponentsView::default()),
    }));
    host.app.state::<Arc<SetupState>>().inner().clone()
}

/// The setup files port: a smoke's fake (nothing of this PC is asked or written), else the platform's (platform.rs: Windows'
/// `WindowsSetupFiles`, whose system folder is the probes' working folder, as C# `SetupProber` runs them in `SystemDirectory`).
fn files_port(host: &Host) -> Arc<dyn SetupFiles> {
    if host.smoke.is_some() { Arc::new(mighty_platform::setup::FakeSetupFiles::default()) } else { crate::platform::setup_files() }
}

/// The catalog platform the screen lists and installs: this OS's (the platform profile's), except in a smoke, whose fixed results,
/// fakes and checks are the Windows screen's on every OS.
fn platform(host: &Host) -> SetupPlatform {
    if host.smoke.is_some() { SetupPlatform::Windows } else { SetupPlatform::of(crate::platform::profile().os) }
}

/// Whether this run may change the machine: C#'s `options.SmokeTest` branches, and W6's `--allow-state-write` default.
pub fn gate(host: &Host) -> Gate {
    if host.smoke.is_some() {
        Gate::Smoke
    } else if host.may_change_machine() {
        Gate::Allowed
    } else {
        Gate::ReadOnly
    }
}

/// Each provider's runtime as the run engine last read it (C#'s `runtime.Providers`); empty before the first read.
pub fn runtimes(host: &Host) -> Vec<ProviderRuntime> {
    host.runs().map(|runs| mighty_core::wire::PROVIDERS.iter().filter_map(|id| runs.runtime(id)).collect()).unwrap_or_default()
}

/// How the screen probes this PC: the CLI environment, the platform's finder, `--version` through the host's `ProcessHost` in the
/// system folder (else the temporary folder). None before the run engine is attached.
fn probe_context(host: &Host, files: &dyn SetupFiles) -> Option<ProbeContext> {
    let runs = host.runs()?;
    Some(ProbeContext {
        environment: runs.catalog.environment(),
        finder: Arc::new(crate::platform::cli_copies),
        processes: host.processes.clone(),
        neutral: files.system_folder().map(PathBuf::from).unwrap_or_else(std::env::temp_dir),
    })
}

/// The CLI environment as it stands now (C# `CliEnvironment.Current()`).
fn environment(host: &Host) -> std::collections::BTreeMap<String, String> {
    host.runs().map(|runs| runs.catalog.environment().current().values).unwrap_or_default()
}

/// C# `RedrawSetup`: every list on screen drawn again now (the frame's clock would on its next tick).
pub fn redraw(host: &Host) {
    for surface in [SETUP_SURFACE, crate::settings::SURFACE] {
        if crate::native::is_open(host, surface) {
            crate::native::touch(host, surface);
        }
    }
}

/// C# `RefreshSetup(force, only)`: the probes (or a smoke's fixed results) taken in; an agent CLI that changed reads the runtime and
/// the accounts again; a forced refresh queued meanwhile runs when this one ends.
pub async fn refresh(host: &Arc<Host>, force: bool, only: Option<HashSet<String>>) {
    let state = of(host);
    let (mut force, mut only) = (force, only);
    loop {
        if host.closing() {
            break;
        }
        let Some(context) = probe_context(host, state.files.as_ref()) else { break };
        let runtime = runtimes(host);
        let workspaces_open = !host.view().snapshot.workspaces().is_empty();
        redraw(host);
        // The probes look at the disk (PATH folders, the installers' folders) between their `--version` reads: off the async workers.
        let (task, only, gate) = (state.clone(), only.take(), gate(host));
        let done = tauri::async_runtime::spawn_blocking(move || {
            tauri::async_runtime::block_on(SetupScreen::refresh(
                &task.screen,
                &context,
                force,
                only,
                gate,
                (!runtime.is_empty()).then_some(runtime.as_slice()),
                workspaces_open,
            ))
        })
        .await
        .unwrap_or_default();
        if done.runtime_stale {
            refresh_runtime(host).await;
            crate::accounts::refresh(host);
        }
        if !done.again {
            break;
        }
        force = true;
    }
    redraw(host);
}

/// C# `RefreshRuntime`: every CLI read again (forced), each provider's reading kept, every pane's composer looked at again (a pane
/// blocked for a missing CLI is not any longer).
async fn refresh_runtime(host: &Arc<Host>) {
    let Some(runs) = host.runs().cloned() else { return };
    let info = runs.catalog.get_runtime(true, None).await;
    for provider in info.providers {
        runs.keep(provider);
    }
    host.panes_changed();
}

/// Whether a list shows: the screen is open, or the components box was drawn a moment ago.
fn lists_shown(host: &Host, state: &SetupState) -> bool {
    crate::native::is_open(host, SETUP_SURFACE) || lock(&state.components_drawn).is_some_and(|at| at.elapsed() < COMPONENTS_SHOWN_FOR)
}

/// C# `StartSetupPoll`: while a list shows, every 5 s, everything probed again when the last probe is 30 s old and something is still
/// missing (never while an install runs or a probe does, never in a smoke).
fn start_poll(host: &Arc<Host>) {
    let state = of(host);
    if state.polling.swap(true, Ordering::SeqCst) {
        return;
    }
    let host = host.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(RUN_PROBE_INTERVAL).await;
            if host.closing() || !lists_shown(&host, &state) {
                break;
            }
            let due = lock(&state.screen).poll_due(Instant::now(), gate(&host));
            if due {
                refresh(&host, false, None).await;
            }
        }
        state.polling.store(false, Ordering::SeqCst);
    });
}

/// C# `BuildSetupContent`'s `Loaded`: the poll started, the probes read (forced unless fixed), the first picks ticked, the accounts
/// read again (a sign-in row follows them). A smoke without fixed results does nothing.
pub fn list_shown(host: &Arc<Host>) {
    let fixed = lock(&of(host).screen).fixed_results.is_some();
    if host.smoke.is_some() && !fixed {
        return;
    }
    start_poll(host);
    let task = host.clone();
    crate::native::act(host, async move {
        refresh(&task, !fixed, None).await;
        lock(&of(&task).screen).preselect_once();
        if !fixed {
            crate::accounts::refresh(&task);
        }
        redraw(&task);
        Ok(())
    });
}

/// Settings › 구성 요소 drew the list: its first drawing since it was last shown counts as the list shown.
pub fn components_drawn(host: &Arc<Host>) {
    let state = of(host);
    let first = {
        let mut drawn = lock(&state.components_drawn);
        let first = drawn.is_none_or(|at| at.elapsed() >= COMPONENTS_SHOWN_FOR);
        *drawn = Some(Instant::now());
        first
    };
    if first {
        list_shown(host);
    }
}

/// What the rows read besides the screen: the agent CLIs signed out, whether a CLI update or a plugin operation runs. Read before
/// the screen is locked (the parts' own state).
fn extras(host: &Host) -> SetupExtras {
    SetupExtras {
        signed_out: crate::accounts::statuses(host)
            .into_iter()
            .filter(|status| status.logged_in == Some(false))
            .map(|status| status.provider)
            .collect(),
        cli_update_running: crate::updates::any_running(host),
        plugin_operations_running: crate::plugins::operations_running(host),
    }
}

/// The list (C# `BuildSetupContent` with `FillSetupContent`) as the screen's state has it now; no I/O. The catalog is not read here:
/// as in C#, the refresh the list's showing starts reads it (`begin_refresh`), so the first drawing of the components box has no rows.
pub fn list(host: &Arc<Host>, ui: &Ui) -> El {
    let extras = extras(host);
    let state = of(host);
    let screen = lock(&state.screen);
    view::setup_content(ui, &screen, &extras, Arc::new(Actions(host.clone())))
}

/// Settings › 구성 요소 as the screen's, the accounts' and the updates' state have it now (`updates`: the CLI updates' box, the host's
/// unless a check hands its own; `all_open`: every row opened, for a check that reads every row's lines, else the rows the person
/// opened); no I/O. The accounts' and the updates' models are read first (they take the state's lock), then the screen's lock, under
/// which the bundled catalog is read once when nothing read it yet (the box has its rows on its first drawing).
pub fn components_with(host: &Arc<Host>, ui: &mut Ui, updates: CliUpdateBox, update_actions: CliUpdateActions, all_open: bool) -> El {
    let accounts = crate::settings::tools::accounts_model(host);
    let actions = ComponentsActions {
        components: Arc::new(Actions(host.clone())),
        accounts: Arc::new(crate::accounts::TabActions { host: host.clone() }),
        updates: update_actions,
    };
    let state = of(host);
    let view = ComponentsView { all_open, ..lock(&state.components_view).clone() };
    let mut screen = lock(&state.screen);
    screen.load_catalog();
    components::components_tab(ui, &ComponentsModel { screen: &screen, accounts: &accounts, updates: &updates, view: &view }, &actions)
}

/// [`components_with`] over the host's CLI updates and the rows the person opened.
pub fn components_tab(host: &Arc<Host>, ui: &mut Ui) -> El {
    components_with(host, ui, crate::updates::box_model(host), crate::updates::box_actions(host), false)
}

/// 구성 요소' own state changed (a row opened or closed, the filter): Settings drawn again.
fn change_view(host: &Arc<Host>, change: impl FnOnce(&mut ComponentsView)) {
    change(&mut lock(&of(host).components_view));
    crate::native::touch(host, crate::settings::SURFACE);
}

/// What the list's controls do.
struct Actions(Arc<Host>);

/// Settings › 구성 요소' own buttons: the screen's, with the install of one item.
impl ComponentActions for Actions {
    fn install(&self, id: &str) {
        let (host, id) = (self.0.clone(), id.to_string());
        crate::native::act(&self.0, async move { request_install(&host, Some(&id)).await });
    }

    fn update(&self) {
        SetupActions::update(self);
    }

    fn open_page(&self, url: &str) {
        SetupActions::open_page(self, url);
    }

    fn recheck(&self) {
        SetupActions::recheck(self);
    }

    fn abandon(&self) {
        SetupActions::abandon(self);
    }

    fn toggle(&self, id: &str) {
        change_view(&self.0, |view| view.toggle(id));
    }

    fn show_problems(&self, only: bool) {
        change_view(&self.0, |view| view.problems_only = only);
    }
}

impl SetupActions for Actions {
    /// C# `ToggleSetupSelection`.
    fn toggle(&self, id: &str) {
        lock(&of(&self.0).screen).toggle(id);
    }

    /// The CLI accounts' own sign-in (a terminal window on the CLI's login command, W6-C's); why it cannot start now is the screen's
    /// message (C# `setupMessage = ex.Message`). No screen lock is held while it runs (it redraws).
    fn sign_in(&self, provider: &str) {
        // In verification mode the sign-in only marks 구성 요소' account row: the screen says so itself, as the install does.
        if !self.0.may_change_machine() {
            self.0.log.line("setup: the sign-in was refused (no --allow-state-write)");
            lock(&of(&self.0).screen).message = Some(verification_mode());
            return;
        }
        lock(&of(&self.0).screen).message = None;
        let (host, provider) = (self.0.clone(), provider.to_string());
        crate::native::act(&self.0, async move {
            let refused = crate::accounts::start_cli_sign_in(&host, &provider, CliLoginOption::Account).await.err();
            if let Some(refused) = refused {
                lock(&of(&host).screen).message = Some(refused);
            }
            redraw(&host);
            Ok(())
        });
    }

    /// The CLI update for an old agent CLI (W6-D's), unless one or a plugin operation runs.
    fn update(&self) {
        if !crate::updates::any_running(&self.0) && !crate::plugins::operations_running(&self.0) {
            crate::updates::start_manual(&self.0);
        }
    }

    /// What only a page installs: its page in the user's browser (never in a smoke).
    fn open_page(&self, url: &str) {
        if self.0.smoke.is_some() {
            self.0.log.line(&format!("smoke: the setup's page {url} is not opened"));
            return;
        }
        if let Err(error) = crate::platform::open_url(url) {
            lock(&of(&self.0).screen).message = Some(error);
            redraw(&self.0);
        }
    }

    /// 다시 확인: PATH read again, everything probed.
    fn recheck(&self) {
        let host = self.0.clone();
        crate::native::act(&self.0, async move {
            refresh(&host, true, None).await;
            Ok(())
        });
    }

    fn install(&self) {
        let host = self.0.clone();
        crate::native::act(&self.0, async move { request_install(&host, None).await });
    }

    /// The hint's button: the script never started, so the run is closed by hand (C# `AbandonSetupRun`).
    fn abandon(&self) {
        let host = self.0.clone();
        crate::native::act(&self.0, async move {
            finish_run(&host, None).await;
            Ok(())
        });
    }
}

/// C# `RequestSetupInstall`: the screen's refusals, then the confirmation (D10), then the install when it is answered Run. A run of
/// the app that may not change the machine says so first (the verification mode's words) and shows no confirmation it would refuse.
/// `only`: Settings › 구성 요소' row — that item alone with the prerequisites it is missing, the screen's picks left as they are.
async fn request_install(host: &Arc<Host>, only: Option<&str>) -> Result<(), String> {
    let state = of(host);
    if gate(host) == Gate::ReadOnly {
        host.log.line("setup: the install was refused (no --allow-state-write)");
        lock(&state.screen).message = Some(verification_mode());
        redraw(host);
        return Ok(());
    }
    let environment = environment(host);
    let confirmation = {
        let mut screen = lock(&state.screen);
        match only {
            Some(id) => screen.may_confirm_item(id, state.files.as_ref(), &environment),
            None => screen.may_confirm(state.files.as_ref(), &environment),
        }
    };
    redraw(host);
    let Some(confirmation) = confirmation else { return Ok(()) };
    if crate::native::show_dialog(host, view::setup_confirmation(&confirmation)).await != DialogResult::Primary {
        return Ok(());
    }
    start_install(host, confirmation.steps).await;
    Ok(())
}

/// The script's folder: `--profile`'s `setup` folder (as C#); for the default profile, a release build's is C#'s own,
/// `%LOCALAPPDATA%\MightyCode\setup` (W9-F, plan §1.5: the installed app it replaced is gone), and a development build's this machine's
/// local folder of the Tauri build for the profile — never the C# app's, whose leftovers (a running install's script among them) a
/// start would remove while that app may still run beside it.
fn script_folder(host: &Host) -> Option<PathBuf> {
    let profile = crate::args::parse(std::env::args().skip(1)).ok().and_then(|options| options.profile);
    let paths = host.app.try_state::<Arc<dyn Paths>>();
    folder_for(profile.as_deref(), crate::build_info::RELEASE, paths.as_deref().map(|paths| paths.as_ref()))
}

/// [`script_folder`]'s rule over its inputs.
fn folder_for(profile: Option<&Path>, release: bool, paths: Option<&dyn Paths>) -> Option<PathBuf> {
    if let Some(profile) = profile {
        return Some(mighty_tools::setup::files::folder(Some(profile), Path::new("")));
    }
    let paths = paths?;
    if release {
        // The local folder is `%LOCALAPPDATA%\<identifier>`: its parent is C#'s `SpecialFolder.LocalApplicationData`.
        return paths.local_dir().parent().map(|local| mighty_tools::setup::files::folder(None, local));
    }
    Some(paths.local_dir().join("setup").join(paths.profile_key()))
}

/// C# `StartSetupInstall`: the script written and opened in a terminal window of its own (the folder made and checked, the script
/// written, off the async workers), then watched.
async fn start_install(host: &Arc<Host>, steps: Vec<mighty_tools::setup::script::SetupScriptStep>) {
    let state = of(host);
    let environment = environment(host);
    let Some(folder) = script_folder(host) else { return };
    // Windows' console window (the platform's launcher), a smoke's recording one; the Mac's terminal pane, where the user answers what
    // a step asks (Swift's setup terminal), as the CLI sign-in opens there (`PlatformProfile::agent_sign_in`), else Terminal.app. The
    // pane reads the frame's state and closes this screen, which take the state's lock and the screen's: the screen hands its launch
    // over and the pane opens once the screen's lock is let go.
    let window = host.app.state::<crate::platform::Ports>().terminals.clone();
    let deferred = Arc::new(Deferred::default());
    let pane = deferred.clone();
    let terminals =
        crate::sign_in_pane::sign_in_launcher(crate::platform::profile().agent_sign_in, host.smoke.is_some(), window.clone(), || pane);
    let (task, gate) = (state.clone(), gate(host));
    let started = tauri::async_runtime::spawn_blocking(move || {
        let mut screen = lock(&task.screen);
        let started = screen.start_install(&steps, gate, &environment, task.files.as_ref(), terminals.as_ref(), &folder, Instant::now());
        (started, screen.run.as_ref().map(|run| run.opened))
    })
    .await;
    let handed = lock(&deferred.0).take();
    if let (Ok((InstallStart::Started, Some(opened))), Some(launch)) = (&started, handed)
        && let Err(error) = open_mac_terminal(host, window.as_ref(), launch)
    {
        host.log.line(&format!("setup: the install's terminal did not open: {error}"));
        lock(&state.screen).launch_failed(*opened, error);
        redraw(host);
        return;
    }
    match started {
        Ok((InstallStart::Started, Some(opened))) => watch(host.clone(), opened),
        Ok((InstallStart::NotAllowed, _)) => {
            host.log.line("setup: the install was refused (no --allow-state-write)");
            lock(&state.screen).message = Some(verification_mode());
        }
        _ => {}
    }
    redraw(host);
}

/// The Mac's install terminal: the pane in a local workspace first (the user follows the install in the window), else (no local
/// workspace on this Mac yet, as on its first run, or a pane that would not open) the same script in a Terminal.app window of its own
/// (Swift's setup terminal window). Either way the script's EXIT trap makes the end marker the watcher waits for.
fn open_mac_terminal(host: &Host, window: &dyn TerminalLauncher, launch: TerminalLaunch) -> mighty_platform::Result<()> {
    let pane = crate::sign_in_pane::SetupPane { app: host.app.clone() };
    match pane.open(launch.clone()) {
        Ok(_) => return Ok(()),
        Err(error) => host.log.line(&format!("setup: no terminal pane ({error}); the script opens in Terminal.app")),
    }
    let argv = mighty_tools::setup::mac::terminal_app_arguments(&launch.argv)
        .ok_or_else(|| mighty_platform::PlatformError::Invalid("the install launch names no script".into()))?;
    window.open(TerminalLaunch { argv, resolved: None, ..launch }).map(|_| ())
}

/// The Mac's install pane as the screen sees it: the launch is kept, and the pane opened with it once the screen's lock is let go
/// ([`start_install`]).
#[derive(Default)]
struct Deferred(Mutex<Option<TerminalLaunch>>);

impl TerminalLauncher for Deferred {
    fn open(&self, launch: TerminalLaunch) -> mighty_platform::Result<Box<dyn mighty_platform::TerminalWindow>> {
        *lock(&self.0) = Some(launch);
        Ok(Box::new(crate::sign_in_pane::Opened))
    }
}

/// C# `WatchSetupRun`: every 2 s the markers looked at (off the async workers); the not-started hint; the run's items probed every
/// 5 s; the run finished when its script ended (or its window closed). The watcher follows the run that opened at `opened` only: it
/// stops when the run is abandoned or another one starts (C# cancels `setupWatch`, `ReferenceEquals(setupRun, run)`).
fn watch(host: Arc<Host>, opened: Instant) {
    tauri::async_runtime::spawn(async move {
        let state = of(&host);
        loop {
            tokio::time::sleep(WATCH_INTERVAL).await;
            if host.closing() || !lock(&state.screen).runs(opened) {
                return;
            }
            let task = state.clone();
            let tick = tauri::async_runtime::spawn_blocking(move || {
                let mut screen = lock(&task.screen);
                if screen.runs(opened) { screen.watch_tick(Instant::now(), task.files.as_ref()) } else { WatchTick::Idle }
            })
            .await
            .unwrap_or(WatchTick::Idle);
            match tick {
                WatchTick::Idle => return,
                WatchTick::Ended => break,
                WatchTick::Running { probe, redraw: hint } => {
                    if hint {
                        redraw(&host);
                    }
                    if let Some(only) = probe {
                        refresh(&host, false, Some(only)).await;
                    }
                }
            }
        }
        finish_run(&host, Some(opened)).await;
    });
}

/// C# `FinishSetupRun(run)`: the run over (`run`: only the run that opened then; None: the one there is, the hint's button), everything
/// probed again (PATH read anew), then the screen brought back while the app is in front, else the status bar's badge.
async fn finish_run(host: &Arc<Host>, run: Option<Instant>) {
    let finished = {
        let state = of(host);
        let mut screen = lock(&state.screen);
        match run {
            Some(opened) => screen.finish_run_of(opened),
            None => screen.finish_run(),
        }
    };
    if !finished {
        return;
    }
    refresh(host, true, None).await;
    // Settings › 구성 요소 shows the result itself: the screen comes back only when neither list shows.
    let shown = crate::native::is_open(host, SETUP_SURFACE) || crate::native::is_open(host, crate::settings::SURFACE);
    let front = host.app.get_webview(crate::host::WINDOW).and_then(|page| page.window().is_focused().ok()).unwrap_or(false);
    if lock(&of(host).screen).after_finish(shown, front) {
        open(host, None);
    }
}

/// The setup surface's model (C# `ShowWebSetup`'s surface): the list, the never-show box at the footer's left; any button (the
/// header's ✕ is 다음에 하기) closes it, never-show applied as it closes.
struct SetupSurface {
    host: Arc<Host>,
    never_show: Local<bool>,
}

impl SurfaceModel for SetupSurface {
    fn view(&mut self, ui: &mut Ui) -> View {
        let ticked = self.never_show.clone();
        let platform = lock(&of(&self.host).screen).platform();
        view::setup_view_on(platform, list(&self.host, ui), self.never_show.get(), move |on| ticked.set(on))
    }

    fn button(&mut self, _which: Option<&str>) -> Next {
        if self.never_show.get() {
            store_never_show(&self.host);
        }
        lock(&of(&self.host).screen).closed();
        Next::Close
    }
}

/// `SetupFirstRun.SetNeverShow()`: written only when this run may change the machine (W6 defaults); otherwise said in the log.
fn store_never_show(host: &Host) {
    if !host.may_change_machine() {
        host.log.line("setup: never-show is not stored (no --allow-state-write, or a smoke)");
        return;
    }
    if let Some(prefs) = host.app.try_state::<Arc<dyn PrefsStore>>()
        && let Err(error) = SetupFirstRun::set_never_show(prefs.as_ref())
    {
        host.log.line(&format!("setup: never-show was not stored: {error}"));
    }
}

/// `setup::open`: C# `OpenSetup(focus)` and `ShowWebSetup` — the setup screen (the surface `setup`, place `surface`), the item `focus`
/// picked when it may be; drawn again when it is open already. `openSetup` and the composer's `installCli` call it.
pub fn open(host: &Arc<Host>, focus: Option<&str>) {
    if !lock(&of(host).screen).open(focus) {
        return;
    }
    if crate::native::is_open(host, SETUP_SURFACE) {
        redraw(host);
        return;
    }
    let model = SetupSurface { host: host.clone(), never_show: Local::new(false) };
    crate::native::open(host, SETUP_SURFACE, Place::Surface, Box::new(model));
    list_shown(host);
}

/// C# `PresentSetupIfNeeded`: at launch the screen opens when no agent CLI can run or Git is missing, unless never-show was chosen
/// (then nothing is probed at all); never in a smoke.
pub async fn present_if_needed(host: &Arc<Host>) {
    let never = host.app.try_state::<Arc<dyn PrefsStore>>().is_some_and(|prefs| SetupFirstRun::never_show(prefs.as_ref()));
    if host.smoke.is_some() || host.closing() || never {
        return;
    }
    refresh(host, false, None).await;
    let present = {
        let state = of(host);
        let screen = lock(&state.screen);
        screen.should_present(never)
    };
    if present {
        open(host, None);
    }
}

/// `setup::shell_flags`: the setup badge and whether setup is needed, for the frame model. Called while the frame model is made, the
/// state locked: it reads the setup's own state, never `host.view()`. The first frame model is the launch: the launch's look
/// ([`present_if_needed`]) starts on a task of its own then.
pub fn shell_flags(host: &Host, _snapshot: &AppSnapshot) -> ShellFlags {
    let state = of(host);
    if !state.presented.load(Ordering::SeqCst)
        && let Some(shared) = host.app.try_state::<Arc<Host>>()
        && !state.presented.swap(true, Ordering::SeqCst)
    {
        let shared = shared.inner().clone();
        tauri::async_runtime::spawn(async move { present_if_needed(&shared).await });
    }
    let (badge, needed) = lock(&state.screen).shell_flags();
    ShellFlags { badge, needed }
}

/// `setup::item_for_missing_cli`: C# `SetupItemForMissingCli` — the setup item that installs an agent pane's missing CLI, for the
/// blocked line's install button (`WindowRules.setup_item_for_missing_cli`). Called with the state locked, as [`shell_flags`].
pub fn item_for_missing_cli(host: &Host, snapshot: &AppSnapshot, pane: &RunSession) -> Option<String> {
    if pane.kind() != "claude" {
        return None;
    }
    let remote = snapshot.workspace(pane.workspace_id()).is_some_and(|workspace| workspace.remote.is_some());
    // Only the pane's own provider's runtime (none before the first read), not every provider's cloned with the state locked.
    let runtime: Vec<ProviderRuntime> = host.runs().and_then(|runs| runs.runtime(pane.provider())).into_iter().collect();
    item_for(host, pane.kind(), pane.provider(), remote, &runtime)
}

/// [`item_for_missing_cli`] over the given runtime (the smoke's step reads it over a runtime of its own, as C#'s does).
pub fn item_for(host: &Host, kind: &str, provider: &str, remote: bool, runtime: &[ProviderRuntime]) -> Option<String> {
    lock(&of(host).screen).item_for_missing_cli(kind, provider, remote, (!runtime.is_empty()).then_some(runtime))
}

#[cfg(test)]
mod tests {
    use mighty_core::locale::Locale;
    use mighty_core::settings_sections as sections;
    use serde_json::json;

    /// W9-F: the install script's folder — `--profile`'s own in any build; for the default profile C#'s `<LocalAppData>/MightyCode/setup`
    /// in a release build, this build's local folder for the profile in a development one.
    #[test]
    fn the_script_folder_is_csharps_in_a_release_build() {
        let root = std::env::temp_dir().join("mighty-setup-folder");
        let paths =
            mighty_platform::AppPaths::resolve(None, Some(root.join("roam")), &[], Some(root.join("local")), "dev.mightycode.native")
                .unwrap();
        let profile = root.join("profile");
        assert_eq!(super::folder_for(Some(&profile), true, Some(&paths)), Some(profile.join("setup")));
        assert_eq!(super::folder_for(Some(&profile), false, None), Some(profile.join("setup")));
        assert_eq!(super::folder_for(None, true, Some(&paths)), Some(root.join("local").join("MightyCode").join("setup")));
        assert_eq!(
            super::folder_for(None, false, Some(&paths)),
            Some(root.join("local").join("dev.mightycode.native").join("setup").join("default"))
        );
        assert_eq!(super::folder_for(None, true, None), None);
    }

    /// C# `SetupVerification.TheScreenIsWiredIntoTheApp` (L1560), as the host's registry and the smoke's checks instead of the WinUI
    /// source: the page's `openSetup` and the composer's `installCli` are methods the host answers; the elevated refusal is said in
    /// words; the smoke drives the screen and reads its checks (`setupTools`, `cancelledWithoutTerminal`, ...).
    #[test]
    fn the_screen_is_wired_into_the_app() {
        let names = crate::methods::registry().names();
        assert!(names.contains("openSetup") && names.contains("installCli"), "{names:?}");
        let elevated = mighty_run::strings::get("setup.windows.error.elevated");
        assert!(!elevated.is_empty() && !elevated.starts_with("setup."), "the elevated refusal is said in words");
        let checks: Vec<&str> = crate::smoke_setup::checks(&json!({})).into_iter().map(|(name, _)| name).collect();
        for name in [
            "setupView",
            "setup",
            "setupToolsFirstRunDecision",
            "setupToolsWelcomeButton",
            "setupToolsRows",
            "setupToolsActions",
            "setupToolsCancelledWithoutTerminal",
            "setupToolsBlockedPaneInstall",
            "setupTools",
            "setupInstallCli",
        ] {
            assert!(checks.contains(&name), "the smoke reads {name}");
        }
    }

    /// C# `ToolkitVerification.ComponentsScreenIsLiveInTheRunningApp` (L1561), over the settings registry: the 구성 요소 tab draws
    /// 구성 요소 (the CLI tab's accounts and updates among its rows), titled from Core, then 내 작업 도구 모음; the smoke builds both over
    /// its fixture (`componentsSection`) and a saved CLI tab opens it.
    #[test]
    fn the_components_screen_is_live_in_settings() {
        let locale = Locale::new("ko");
        let titles: Vec<String> = crate::settings::groups(&locale, "tools").into_iter().map(|group| group.title).collect();
        assert_eq!(titles, [locale.get("settings.components.sectionTitle"), locale.get("settings.toolkit.sectionTitle")]);
        assert_eq!(sections::title_of(&locale, sections::COMPONENTS).as_deref(), Some(titles[0].as_str()));
        assert!(crate::settings::groups(&locale, "cli").is_empty(), "no CLI tab");
        assert_eq!(sections::moved_category("cli"), "tools");
        let checks: Vec<&str> = crate::smoke_setup::checks(&json!({})).into_iter().map(|(name, _)| name).collect();
        assert!(checks.contains(&"componentsSection") && checks.contains(&"statusLine"));
    }
}
