//! The app update in the host (C# `WinUI\MainWindow.AppUpdate.cs` whole, `MainWindow.WebShell.Shell.cs:130-131,478-500`,
//! `Core\PhoneAppUpdate.cs`; the Mac's `AppStore+AppUpdate.swift` for the idle facts; plan §1.4, W9-C). Owner: W9-C.
//!
//! mighty-updates' coordinator over its `Service` with the build's key and address (A's `build::public_key` / `manifest_url` over the
//! stamps build.rs wrote), this machine's architecture and the exe's folder as the install; the startup housekeeping (restore, prune,
//! `SyncDisplayVersion`, the renamed shortcuts) where the host may change the machine; the 10 min / 1 s loop, started once the page is
//! up; the idle facts; the countdown banner and the status bar's update words through the shell model ([`frame`]); the page methods
//! `postponeUpdate` and `restartForUpdate`; the helper's launch through `ProcessHost::start_detached` (hidden, in `%TEMP%`, only
//! `SystemRoot`); the install on quit from the quit path ([`quit_hook`], skipped while Windows ends the session); the phone's read and
//! act; `appUpdateLastCheckedAt` and `appUpdateAutoCheck` through the store.
//!
//! **One task.** C# runs every coordinator call on the window's dispatcher. Here a thread of its own (`app-update`) runs a
//! current-thread runtime with a `LocalSet`: every call — the settings button, the switch, the page methods, the phone, the loop's
//! ticks, the coordinator's change callback, the quit — reaches it as a [`Command`] and runs there, interleaving only at awaits. The
//! coordinator's own lock is never what keeps two callers apart.
//!
//! **Gating (plan §4).** The build checks and installs on its own only where it may change the machine (the write policy, never in a
//! smoke), verifies manifests and knows where they are ([`Core::can_run_automatically`]); the press in Settings checks only where it
//! may change the machine. A smoke never starts the loop, the housekeeping or the session watch: its step renders fixture phases
//! (smoke_app_update.rs).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use chrono::{DateTime, Utc};
use mighty_core::locale::Locale;
use mighty_core::shell::AppUpdateFrame;
use mighty_platform::{DetachedEnvironment, DetachedRequest, ProcessHost, SessionEnding, Subscription, UpdateInstaller};
use mighty_remote::phone::app_update::{self as phone_update, PhoneAppUpdateStep};
use mighty_updates::app_update::auto::{self, IdleBlocker, IdleFacts, RestartAction, RestartState};
use mighty_updates::app_update::coordinator::{Coordinator, KeptCheck, Options};
use mighty_updates::app_update::plan::{HELPER_ENVIRONMENT, InstallPlan};
use mighty_updates::app_update::service::Service;
use mighty_updates::app_update::{Phase, State, build, dotnet, housekeeping, version};
use serde_json::Value;
use tauri::Manager;
use tokio::sync::{Notify, mpsc};
use tokio::time::Instant;

use crate::host::{Host, Registry};

/// The stamped key's text (build.rs; empty in a development build), read by A's decoder only.
const PUBLIC_KEY_TEXT: &str = env!("MIGHTY_BUILD_UPDATE_PUBLIC_KEY");
/// The stamped manifest address (build.rs; empty in a development build), read by A's decoder only.
const MANIFEST_URL_TEXT: &str = env!("MIGHTY_BUILD_UPDATE_URL");

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The saved settings a decision reads (C#'s `service.Snapshot` fields).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Settings {
    /// `appUpdateAutoCheck`.
    pub automatic: bool,
    /// `appUpdateLastCheckedAt`, as saved.
    pub last_checked_at: Option<String>,
    /// `appUpdateManifestUrlOverride`.
    pub url: Option<String>,
}

/// What the core reads of the app and asks of it; the host's are [`HostSeams`], a test's its own. Never called with the core's lock
/// held.
pub trait Seams: Send + Sync {
    fn settings(&self) -> Settings;
    /// C# `CollectIdleFacts(now)`.
    fn idle_facts(&self, now: DateTime<Utc>) -> IdleFacts;
    /// `appUpdateLastCheckedAt = at.ToString("O")`.
    fn save_checked_at(&self, at: DateTime<Utc>);
    /// After each change of the update (on the driver): the box drawn again, the phone told.
    fn changed(&self);
}

/// The clock decisions read (C#'s `DateTimeOffset.UtcNow`; a test's follows tokio's paused clock).
pub type Clock = Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>;

/// The loop's timer (C#'s `appUpdateTimer`): made once, stopped and started again at a pace.
#[derive(Debug, Clone, Copy, Default)]
struct Timer {
    created: bool,
    interval: Option<Duration>,
    next: Option<Instant>,
}

#[derive(Default)]
struct CoreState {
    restart: RestartState,
    /// C#'s `appUpdateKeptPending`.
    kept_pending: bool,
    /// C#'s `appUpdateSavedCheck`.
    saved_check: Option<DateTime<Utc>>,
    /// C#'s `phoneRequestedAppUpdate`.
    phone_requested: bool,
    timer: Timer,
    closing: bool,
    /// C#'s `appUpdateSmokeSignedBuild`: the box renders as a signed build with the switch on.
    smoke_signed_build: bool,
}

/// The app update's host-free part (C# `MainWindow.AppUpdate.cs`'s fields and methods over the coordinator). Its methods run on the
/// driver ([`drive`]) except the reads ([`Core::frame`], [`Core::phone_read`], the box's).
pub struct Core {
    pub coordinator: Arc<Coordinator>,
    seams: Arc<dyn Seams>,
    clock: Clock,
    /// Where the build may change the machine (the write policy, not a smoke): the loop, the press and the phone need it.
    allowed: bool,
    smoke: bool,
    state: Mutex<CoreState>,
    /// C#'s `appUpdatePoweringOff`.
    powering_off: AtomicBool,
    /// The driver looks at the timer again.
    wake: Notify,
}

impl Core {
    pub fn new(coordinator: Arc<Coordinator>, seams: Arc<dyn Seams>, clock: Clock, allowed: bool, smoke: bool) -> Arc<Core> {
        Arc::new(Core {
            coordinator,
            seams,
            clock,
            allowed: allowed && !smoke,
            smoke,
            state: Mutex::new(CoreState::default()),
            powering_off: AtomicBool::new(false),
            wake: Notify::new(),
        })
    }

    fn now(&self) -> DateTime<Utc> {
        (self.clock)()
    }

    fn state(&self) -> MutexGuard<'_, CoreState> {
        lock(&self.state)
    }

    fn closing(&self) -> bool {
        self.state().closing
    }

    /// C# `AppUpdateCanRunAutomatically`: not a smoke, the build may change the machine, verifies manifests and knows where they are.
    pub fn can_run_automatically(&self) -> bool {
        self.allowed
            && self.coordinator.has_public_key()
            && self.coordinator.effective_manifest_url(self.seams.settings().url.as_deref()).is_some()
    }

    /// C# `AppUpdateAutomaticNow`.
    fn automatic_now(&self) -> bool {
        self.seams.settings().automatic && self.can_run_automatically()
    }

    /// Whether the box's press may reach the coordinator (a check writes under `<data>\updates`).
    pub fn press_allowed(&self) -> bool {
        self.allowed
    }

    /// Windows signs out, restarts or shuts down (`WM_QUERYENDSESSION`), or called it off.
    pub fn session(&self, event: SessionEnding) {
        match event {
            SessionEnding::Query => self.powering_off.store(true, Ordering::SeqCst),
            SessionEnding::End { ending: false } => self.powering_off.store(false, Ordering::SeqCst),
            SessionEnding::End { ending: true } => {}
        }
    }

    // ── the loop's timer ──

    /// The timer's next tick, when it runs.
    fn next_tick(&self) -> Option<Instant> {
        self.state().timer.next
    }

    /// C# `PaceAppUpdateTimer`: once a second while the banner counts down, every ten minutes otherwise; a made timer that is stopped
    /// starts again at that pace.
    fn pace(&self) {
        {
            let mut state = self.state();
            if !state.timer.created {
                return;
            }
            let interval = if matches!(state.restart, RestartState::CountingDown(_)) { auto::COUNTDOWN_TICK } else { auto::TICK };
            if state.timer.next.is_some() && state.timer.interval == Some(interval) {
                return;
            }
            state.timer.interval = Some(interval);
            state.timer.next = Some(Instant::now() + interval);
        }
        self.wake.notify_one();
    }

    /// C# `StartAppUpdateLoop`.
    fn start_loop(&self) {
        if self.closing() || !self.automatic_now() {
            return;
        }
        self.state().timer.created = true;
        self.pace();
    }

    /// C# `StopAppUpdateLoop`: the loop stopped and a running countdown taken away (a "later" stays).
    fn stop_loop(&self) {
        {
            let mut state = self.state();
            state.timer.next = None;
            if matches!(state.restart, RestartState::CountingDown(_)) {
                state.restart = RestartState::Idle;
            }
        }
        self.wake.notify_one();
    }

    /// The timer fired: the next tick scheduled, then C#'s `AppUpdateTick`.
    fn fired(self: &Arc<Self>) {
        {
            let mut state = self.state();
            let interval = state.timer.interval.unwrap_or(auto::TICK);
            state.timer.next = state.timer.next.map(|_| Instant::now() + interval);
        }
        self.tick();
    }

    // ── C#'s methods ──

    /// C# `BeginAutomaticAppUpdateCheck` after its housekeeping: a kept update looked at from the first tick on, the loop, a due check.
    pub fn begin(self: &Arc<Self>) {
        if self.smoke {
            return;
        }
        self.state().kept_pending = true;
        self.start_loop();
        let core = self.clone();
        tokio::task::spawn_local(async move { core.check_if_due().await });
    }

    /// C# `AppUpdateTick`: a check (not while the banner counts down), then the idle restart offer.
    fn tick(self: &Arc<Self>) {
        if self.closing() {
            return;
        }
        if !matches!(self.state().restart, RestartState::CountingDown(_)) {
            let core = self.clone();
            tokio::task::spawn_local(async move { core.check_on_tick().await });
        }
        self.offer_restart();
    }

    /// C# `CheckAppUpdateOnTick`: a kept update first (waiting tick after tick while staging is busy), then a deferred download, then
    /// the due check.
    async fn check_on_tick(self: Arc<Self>) {
        if self.closing() || !self.can_run_automatically() {
            return;
        }
        let settings = self.seams.settings();
        if self.state().kept_pending {
            let kept = self.coordinator.check_kept_update(settings.url.as_deref(), self.now()).await;
            if kept != KeptCheck::Waiting {
                self.state().kept_pending = false;
            }
            if kept != KeptCheck::None {
                return;
            }
        }
        if self.coordinator.staging_deferred() && self.coordinator.state().phase == Phase::Available {
            self.coordinator.retry_deferred_staging().await;
            return;
        }
        self.check_if_due().await;
    }

    /// C# `CheckAppUpdateIfDue`.
    async fn check_if_due(&self) {
        if self.closing() || !self.can_run_automatically() {
            return;
        }
        let settings = self.seams.settings();
        let last = parse_time(settings.last_checked_at.as_deref());
        self.coordinator.check_automatically_if_due(settings.url.as_deref(), settings.automatic, last, self.now()).await;
    }

    /// C# `AppUpdateStateChanged`, on the driver after each change: the phone's request moves on, the box and the phone follow, the
    /// check time is saved once per new check.
    pub fn state_changed(self: &Arc<Self>) {
        if self.closing() {
            return;
        }
        if self.state().phone_requested {
            self.advance_phone();
        }
        self.seams.changed();
        let checked = self.coordinator.state().checked_at;
        if let Some(at) = checked
            && !self.smoke
        {
            let new = {
                let mut state = self.state();
                let new = state.saved_check != Some(at);
                state.saved_check = Some(at);
                new
            };
            if new {
                self.seams.save_checked_at(at);
            }
        }
    }

    /// C# `AppUpdateAutomaticChanged(on)` (the switch was saved): off stops the loop, a download and the countdown; on starts the
    /// loop, checks when due and downloads what a check already found.
    pub fn automatic_changed(self: &Arc<Self>, on: bool) {
        if self.closing() || self.smoke {
            return;
        }
        if !on {
            self.stop_loop();
            if self.coordinator.state().phase == Phase::Downloading {
                self.coordinator.cancel_download();
            }
            self.seams.changed();
            return;
        }
        if !self.can_run_automatically() {
            return;
        }
        self.start_loop();
        let core = self.clone();
        if self.coordinator.state().phase == Phase::Available {
            tokio::task::spawn_local(async move { core.coordinator.download().await });
        } else {
            tokio::task::spawn_local(async move { core.check_if_due().await });
        }
        self.seams.changed();
    }

    /// The box's one button (C#'s `PressAsync(override)`), where the build may change the machine.
    pub async fn press(self: Arc<Self>) {
        if !self.allowed || self.closing() {
            return;
        }
        let url = self.seams.settings().url;
        self.coordinator.press(url.as_deref()).await;
    }

    /// C# `OfferAppUpdateRestart`: starts the countdown when the app is idle, cancels it quietly when it no longer is, restarts to
    /// install when it runs out.
    fn offer_restart(self: &Arc<Self>) {
        if self.closing() {
            return;
        }
        let ready = self.coordinator.state().phase == Phase::Ready;
        let restart = self.state().restart;
        if !ready && restart == RestartState::Idle {
            return;
        }
        let now = self.now();
        let blockers = auto::idle_blockers(&self.seams.idle_facts(now), now);
        let decision = auto::restart_offer(restart, ready, self.automatic_now(), &blockers, now);
        self.state().restart = decision.state;
        self.pace();
        if decision.action == RestartAction::Restart {
            self.restart_automatically();
        }
    }

    /// C# `RestartForAppUpdateAutomatically`: asked again, in this turn, whether the app is idle (input aside), and only then the helper
    /// started; something that started meanwhile leaves the update ready for a fresh countdown.
    fn restart_automatically(&self) {
        if self.closing() {
            return;
        }
        let now = self.now();
        let blockers = auto::idle_blockers(&self.seams.idle_facts(now), now);
        if self.coordinator.state().phase != Phase::Ready
            || !self.automatic_now()
            || blockers.iter().any(|blocker| *blocker != IdleBlocker::UserActive)
        {
            return;
        }
        self.coordinator.install(true);
    }

    /// C# `RestartForAppUpdateNow` (`restartForUpdate`, the banner's and the status bar's "restart now"): a running countdown cleared
    /// first.
    pub fn restart_now(&self) {
        self.state().restart = RestartState::Idle;
        self.pace();
        self.coordinator.install(true);
    }

    /// C# `PostponeAppUpdateRestart` (`postponeUpdate`, the banner's "later"): no banner for `Snooze`; quitting still installs.
    pub fn postpone(&self) {
        self.state().restart = auto::later(self.now());
        self.pace();
    }

    /// The idle blockers now, from the app's idle facts (the smoke's draft check).
    pub fn idle_blockers(&self) -> Vec<IdleBlocker> {
        let now = self.now();
        auto::idle_blockers(&self.seams.idle_facts(now), now)
    }

    /// The restart offer's state (the smoke sets and restores it).
    pub fn restart(&self) -> RestartState {
        self.state().restart
    }

    pub fn set_restart(&self, restart: RestartState) {
        self.state().restart = restart;
    }

    /// C#'s `appUpdateSmokeSignedBuild`.
    pub fn set_smoke_signed_build(&self, on: bool) {
        self.state().smoke_signed_build = on;
    }

    /// How the box describes the state now (C# `RenderAppUpdateSection`'s `Describe` call).
    pub fn describe(&self, locale: &Locale) -> mighty_core::app_update_view::SectionView {
        let signed = self.state().smoke_signed_build;
        mighty_core::app_update_view::describe(
            locale,
            &self.coordinator.state().view(),
            signed || self.coordinator.has_public_key(),
            self.coordinator.built_in_manifest_url(),
            |at| at.with_timezone(&chrono::Local).format("%H:%M").to_string(),
            signed || self.coordinator.automatic(),
        )
    }

    /// C# `RenderAppUpdateCountdown` and `RefreshStatusBar`'s update rows, in `locale`'s words. Takes no lock but its own and the
    /// coordinator's (the shell model reads it with the state locked).
    pub fn frame(&self, locale: &Locale) -> AppUpdateFrame {
        let update = self.coordinator.state();
        let known = update.availability.as_ref().map(|a| a.manifest.version.clone());
        let (restart, closing) = {
            let state = self.state();
            (state.restart, state.closing)
        };
        let countdown = match (restart, known.as_deref()) {
            (RestartState::CountingDown(until), Some(version)) if !closing && !version.is_empty() => {
                let left = (until - self.now()).num_milliseconds();
                let seconds = if left <= 0 { 0 } else { (left + 999) / 1000 };
                Some(locale.get_with("appUpdate.countdown.message", &[("version", version), ("seconds", &seconds.to_string())]))
            }
            _ => None,
        };
        let newer = known.clone().filter(|v| !v.is_empty() && matches!(update.phase, Phase::Available | Phase::Ready));
        let staged = newer.is_some() && update.phase == Phase::Ready && self.coordinator.automatic();
        let with = |key: &str, version: &str| locale.get_with(key, &[("version", version)]);
        AppUpdateFrame {
            countdown,
            update: newer.as_deref().filter(|_| !staged).map(|v| with("window.status.updateBadge", v)),
            staged: newer.as_deref().filter(|_| staged).map(|v| with("window.status.updateStaged", v)),
            staged_version: known,
        }
    }

    // ── the phone (C# `PhoneAppUpdateRead`, `PhoneAppUpdateActAsync`, `AdvancePhoneAppUpdate`) ──

    /// What the phone sees: None while this build cannot update itself.
    pub fn phone_read(&self) -> Option<State> {
        self.can_run_automatically().then(|| self.coordinator.state().view())
    }

    /// The phone's button: `install` false checks; true installs a ready update now, or first checks and downloads, then installs.
    pub async fn phone_act(self: Arc<Self>, install: bool) {
        if !self.can_run_automatically() {
            return;
        }
        let url = self.seams.settings().url;
        if !install {
            self.coordinator.check(url.as_deref()).await;
            return;
        }
        self.state().phone_requested = true;
        if phone_update::begin(&self.coordinator.state().view()) == PhoneAppUpdateStep::Check {
            self.coordinator.check(url.as_deref()).await;
        } else {
            self.advance_phone();
        }
    }

    fn advance_phone(self: &Arc<Self>) {
        match phone_update::next(&self.coordinator.state().view()) {
            PhoneAppUpdateStep::Download => {
                let core = self.clone();
                tokio::task::spawn_local(async move { core.coordinator.download().await });
            }
            PhoneAppUpdateStep::Install => {
                self.state().phone_requested = false;
                // After the change that made it ready has been applied (C#'s `TryEnqueue`).
                let core = self.clone();
                tokio::task::spawn_local(async move { core.restart_now() });
            }
            PhoneAppUpdateStep::Done => self.state().phone_requested = false,
            PhoneAppUpdateStep::Wait | PhoneAppUpdateStep::Check => {}
        }
    }

    // ── the quit ──

    /// C# `InstallAppUpdateOnQuitAsync` then `ShutdownAppUpdateAsync`: the loop stopped, a ready update installed once the app has gone
    /// (not in a smoke, not while an install is under way, not while Windows ends the session), then nothing more starts. True when the
    /// helper was started.
    pub fn quit(&self, powering_off: bool) -> bool {
        {
            let mut state = self.state();
            state.timer.next = None;
        }
        let powering_off = powering_off || self.powering_off.load(Ordering::SeqCst);
        let started = !self.smoke && self.coordinator.state().phase != Phase::Installing && self.coordinator.install_on_quit(powering_off);
        self.state().closing = true;
        self.coordinator.shutdown();
        self.wake.notify_one();
        started
    }
}

/// `DateTimeOffset.TryParse` of the saved check time.
fn parse_time(text: Option<&str>) -> Option<DateTime<Utc>> {
    mighty_core::time::parse(text).and_then(|time| dotnet::from_ticks(time.utc_ticks()))
}

// ── the driver ──

/// Work for the driver, run there in order; it may spawn more on the driver (`tokio::task::spawn_local`).
pub type Command = Box<dyn FnOnce(&Arc<Core>) + Send>;

/// The driver: each command run in the order sent, the loop's ticks, until the channel closes or the core closes. Runs inside a
/// `LocalSet` (the work it spawns runs on the same thread).
pub async fn drive(core: Arc<Core>, mut commands: mpsc::UnboundedReceiver<Command>) {
    loop {
        let next = core.next_tick();
        let tick = async {
            match next {
                Some(at) => tokio::time::sleep_until(at).await,
                None => std::future::pending::<()>().await,
            }
        };
        tokio::select! {
            command = commands.recv() => match command {
                Some(command) => command(&core),
                None => return,
            },
            () = core.wake.notified() => {}
            () = tick => core.fired(),
        }
        if core.closing() {
            return;
        }
    }
}

/// C#'s `StateChanged += TryEnqueue(AppUpdateStateChanged)`: a change, from whatever thread it came (a download's progress arrives on
/// the transport's), handled on the driver.
pub fn wire(coordinator: &Coordinator, commands: &mpsc::UnboundedSender<Command>) {
    let notify = commands.clone();
    coordinator.set_on_change(Arc::new(move || {
        let _ = notify.send(Box::new(|core: &Arc<Core>| core.state_changed()));
    }));
}

/// The driver on a thread of its own (C#'s dispatcher).
fn spawn_driver(core: Arc<Core>, commands: mpsc::UnboundedReceiver<Command>) -> std::io::Result<()> {
    std::thread::Builder::new().name("app-update".into()).spawn(move || {
        let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
            Ok(runtime) => runtime,
            Err(error) => {
                eprintln!("the app update's runtime: {error}");
                return;
            }
        };
        tokio::task::LocalSet::new().block_on(&runtime, drive(core, commands));
    })?;
    Ok(())
}

// ── the host ──

/// The host's app update: the core and its driver's channel, kept in the app's state.
pub struct AppUpdate {
    pub core: Arc<Core>,
    commands: mpsc::UnboundedSender<Command>,
    /// The session-ending watch, kept while the app runs.
    session: Mutex<Option<Subscription>>,
}

impl AppUpdate {
    /// `work` run on the driver (dropped once the driver ended).
    pub fn send(&self, work: impl FnOnce(&Arc<Core>) + Send + 'static) {
        let _ = self.commands.send(Box::new(work));
    }

    /// `work` run on the driver and its answer awaited (None once the driver ended).
    pub async fn call<T: Send + 'static, F>(&self, work: impl FnOnce(Arc<Core>) -> F + Send + 'static) -> Option<T>
    where
        F: std::future::Future<Output = T> + 'static,
    {
        let (answer, answered) = tokio::sync::oneshot::channel();
        self.send(move |core| {
            let future = work(core.clone());
            tokio::task::spawn_local(async move {
                let _ = answer.send(future.await);
            });
        });
        answered.await.ok()
    }
}

/// The app update, once [`start`] made it.
pub fn of(host: &Host) -> Option<Arc<AppUpdate>> {
    host.app.try_state::<Arc<AppUpdate>>().map(|state| state.inner().clone())
}

/// The host's [`Seams`]: the saved settings, the idle facts, the store, the box and the phone.
struct HostSeams {
    host: Arc<Host>,
    session: Arc<dyn mighty_platform::SessionWatch>,
    /// `appUpdateAutoCheck` as saved, mirrored for the coordinator's `automatic` (asked with its lock held, so never the state's).
    automatic: Arc<AtomicBool>,
}

impl Seams for HostSeams {
    fn settings(&self) -> Settings {
        let view = self.host.view();
        let snapshot = &view.snapshot;
        Settings {
            automatic: snapshot.app_update_auto_check,
            last_checked_at: snapshot.app_update_last_checked_at.clone(),
            url: snapshot.app_update_manifest_url_override.clone(),
        }
    }
    fn idle_facts(&self, now: DateTime<Utc>) -> IdleFacts {
        collect_idle_facts(&self.host, self.session.as_ref(), now)
    }
    fn save_checked_at(&self, at: DateTime<Utc>) {
        let _ = self.host.view().update(|state| state.app_update_last_checked_at = Some(dotnet::round_trip(at)));
    }
    fn changed(&self) {
        self.automatic.store(self.host.view().snapshot.app_update_auto_check, Ordering::SeqCst);
        if crate::native::is_open(&self.host, crate::settings::SURFACE) {
            crate::native::touch(&self.host, crate::settings::SURFACE);
        }
        crate::phone::changed(&self.host);
    }
}

/// The version the coordinator compares (C# `AppVersion.Normalized(AppVersionText) ?? "0.1.0"`).
fn current_version(host: &Host) -> String {
    version::normalized(&crate::settings::about::version_text(host)).unwrap_or_else(|| "0.1.0".into())
}

/// C# `LaunchUpdateHelperAsync`'s check: the helper must be inside the staged folder (`Path.GetFullPath`, compared ignoring case).
fn inside_stage(helper: &Path, staged: &Path) -> bool {
    let (Ok(helper), Ok(staged)) = (dotnet::full_path(helper), dotnet::full_path(&dotnet::trim_separator(staged))) else {
        return false;
    };
    let root = format!("{}{}", staged.to_string_lossy(), dotnet::SEPARATOR);
    dotnet::to_upper_invariant(&helper.to_string_lossy()).starts_with(&dotnet::to_upper_invariant(&root))
}

/// C# `LaunchUpdateHelperAsync`: the staged version's exe started detached, hidden, in the temporary folder, with only `SystemRoot`;
/// with a relaunch the app then quits through its normal quit path (`quit`).
pub fn launch_helper(processes: &dyn ProcessHost, locale: &Locale, plan: &InstallPlan, quit: &dyn Fn()) -> Result<(), String> {
    let helper = plan.staged_helper_executable();
    if !inside_stage(&helper, &plan.staged_directory) {
        return Err(locale.get("settings.appUpdate.helperOutsideStage"));
    }
    let environment: BTreeMap<String, String> = HELPER_ENVIRONMENT.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    processes
        .start_detached(DetachedRequest {
            program: helper,
            args: plan.helper_arguments(),
            cwd: std::env::temp_dir(),
            environment: DetachedEnvironment::Exactly(environment),
            hidden: true,
        })
        .map_err(|error| match error {
            // A refusal says why (W9-B: a job that forbids breakaway refuses rather than start a helper the job would kill).
            mighty_platform::PlatformError::Refused(reason) => sentence(&reason),
            other => other.to_string(),
        })?;
    if plan.relaunch {
        quit();
    }
    Ok(())
}

/// A platform's reason as a sentence: its first letter upper case, a full stop at its end.
fn sentence(reason: &str) -> String {
    let mut chars = reason.trim().chars();
    let mut words: String = chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default();
    if !words.ends_with('.') {
        words.push('.');
    }
    words
}

/// The install folder: the exe's own (C# `AppContext.BaseDirectory`).
fn install_directory() -> Option<PathBuf> {
    std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_path_buf))
}

/// The page methods: `postponeUpdate` (C#: snooze 4 h) and `restartForUpdate` (C#: install now and relaunch).
pub fn register(methods: &mut Registry) {
    methods.add("postponeUpdate", |host: Arc<Host>, _| async move {
        if let Some(update) = of(&host) {
            update.send(|core| core.postpone());
        }
        Ok(Value::Null)
    });
    methods.add("restartForUpdate", |host: Arc<Host>, _| async move {
        if let Some(update) = of(&host) {
            update.send(|core| core.restart_now());
        }
        Ok(Value::Null)
    });
}

/// After the host exists (main.rs): the coordinator and its driver; outside a smoke, once the page is up, the housekeeping (where the
/// host may change the machine), the session watch and the loop (C# `BeginAutomaticAppUpdateCheck`, after the first render).
pub fn start(host: &Arc<Host>, platform: &mighty_platform::Platform) {
    let automatic = Arc::new(AtomicBool::new(host.view().snapshot.app_update_auto_check));
    let service = Arc::new(Service::new(platform.update_transport.clone(), &host.data_dir, build::public_key(Some(PUBLIC_KEY_TEXT))));
    let mut options = Options::new(&current_version(host));
    options.built_in_manifest_url = build::manifest_url(Some(MANIFEST_URL_TEXT));
    options.install_directory = install_directory();
    options.locale = host.locale;
    let reads = automatic.clone();
    options.automatic = Arc::new(move || reads.load(Ordering::SeqCst));
    let installer: Arc<dyn UpdateInstaller> = platform.updates.clone();
    let running = installer.clone();
    options.running_from = Arc::new(move |folder| running.running_from(folder, true).map_err(std::io::Error::other));
    let (processes, locale, app) = (platform.processes.clone(), host.locale, host.app.clone());
    options.launch_helper = Some(Arc::new(move |plan| launch_helper(processes.as_ref(), &locale, plan, &|| app.exit(0))));
    let coordinator = Arc::new(Coordinator::new(service, options));
    let seams = Arc::new(HostSeams { host: host.clone(), session: platform.session.clone(), automatic });
    let core = Core::new(coordinator.clone(), seams, Arc::new(Utc::now), host.may_change_machine(), host.smoke.is_some());
    let (commands, received) = mpsc::unbounded_channel::<Command>();
    if let Err(error) = spawn_driver(core.clone(), received) {
        host.log.line(&format!("app update: no driver: {error}"));
    }
    let update = Arc::new(AppUpdate { core: core.clone(), commands, session: Mutex::new(None) });
    wire(&coordinator, &update.commands);
    host.app.manage(update.clone());
    if host.smoke.is_some() {
        return;
    }
    let (host, session, may_change) = (host.clone(), platform.session.clone(), host.may_change_machine());
    update.clone().send(move |core| {
        let core = core.clone();
        tokio::task::spawn_local(async move {
            // After the first render: the page has said it is ready.
            while !host.view().ready {
                if host.closing() {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            if may_change && let Some(install) = install_directory() {
                let (log, version, installer) =
                    (crate::logfile::LogFile::new(host.log.dir().to_path_buf()), crate::update_helper::version(), installer.clone());
                let _ = tokio::task::spawn_blocking(move || housekeeping_at_start(&install, &version, installer.as_ref(), &log)).await;
            }
            // Without it a quit during a sign-out would still install; the update itself goes on.
            let watched = core.clone();
            match session.on_ending(Arc::new(move |event| watched.session(event))) {
                Ok(subscription) => *lock(&update.session) = Some(subscription),
                Err(error) => host.log.line(&format!("session ending not watched: {error}")),
            }
            core.begin();
        });
    });
}

/// C#'s housekeeping at start, each step best effort: an install an interrupted swap left aside put back, old backups removed, the
/// uninstall entry's version, an install from before the rename refreshed.
fn housekeeping_at_start(install: &Path, version: &str, installer: &dyn UpdateInstaller, log: &crate::logfile::LogFile) {
    housekeeping::restore_interrupted_install(install);
    housekeeping::prune_backups(install, Utc::now());
    if let Err(error) = installer.sync_display_version(install, version) {
        log.line(&format!("app update: the uninstall entry's version not synced: {error}"));
    }
    if let Err(error) = installer.refresh_renamed_install(install) {
        log.line(&format!("app update: the renamed install not refreshed: {error}"));
    }
}

/// main.rs, once the main window exists (on its thread): the session-ending watch on it. Not in a smoke (C#: inside
/// `BeginAutomaticAppUpdateCheck`).
pub fn watch_window(host: &Host, window: &tauri::Window) {
    if host.smoke.is_some() {
        return;
    }
    if let Err(error) = crate::platform::watch_session(window) {
        host.log.line(&format!("session ending not watched: {error}"));
    }
}

/// The quit path's first step (main.rs, C#'s `InstallOnQuitAsync` before the shutdown): on the driver, a ready update installs with
/// `--relaunch 0` unless Windows is ending the session (`powering_off` or the session watch); then the coordinator shuts down. Waits
/// at most 5 s for the driver.
pub fn quit_hook(host: &Arc<Host>, powering_off: bool) {
    let Some(update) = of(host) else { return };
    let (done, finished) = std::sync::mpsc::channel();
    update.send(move |core| {
        let _ = done.send(core.quit(powering_off));
    });
    match finished.recv_timeout(Duration::from_secs(5)) {
        Ok(true) => host.log.line("app update: the update installs once the app has quit"),
        Ok(false) => {}
        Err(_) => host.log.line("app update: the quit's install was not answered"),
    }
}

/// The shell model's update rows (host.rs, with the state locked: reads only the core and the coordinator).
pub fn frame(host: &Host) -> Option<AppUpdateFrame> {
    of(host).map(|update| update.core.frame(&host.locale)).filter(|frame| frame != &AppUpdateFrame::default())
}

/// The phone's read (phone.rs `app_update_read`).
pub fn phone_read(host: &Host) -> Option<State> {
    of(host).and_then(|update| update.core.phone_read())
}

/// The phone's act (phone.rs `app_update_act`): run on the driver and awaited.
pub async fn phone_act(host: &Host, install: bool) {
    if let Some(update) = of(host) {
        update.call(move |core| core.phone_act(install)).await;
    }
}

/// The box's button (settings/app_update.rs).
pub fn press(host: &Host) {
    if let Some(update) = of(host) {
        update.send(|core| {
            tokio::task::spawn_local(core.clone().press());
        });
    }
}

/// The box's switch (settings/app_update.rs): saved, the mirror kept, then C#'s `AppUpdateAutomaticChanged`.
pub fn set_automatic(host: &Arc<Host>, on: bool) -> Result<(), String> {
    host.view().update(|state| state.app_update_auto_check = on)?;
    if let Some(update) = of(host) {
        // The coordinator's `automatic` mirror follows the saved switch at once.
        update.core.seams.changed();
        update.send(move |core| core.automatic_changed(on));
    }
    Ok(())
}

/// The saved switch written as the smoke flips and restores it (C#'s `service.UpdateAsync(AppUpdateAutoCheck)`), the mirror kept; the
/// loop is not told (a smoke has none).
pub fn write_automatic(host: &Host, on: bool) {
    let _ = host.view().update(|state| state.app_update_auto_check = on);
    if let Some(update) = of(host) {
        update.core.seams.changed();
    }
}

/// C# `CollectIdleFacts(now)`: one source per row of the idle table (M/AppStore+AppUpdate.swift appIdleFacts). What the state's lock
/// covers is read under it (every source there takes only its own lock); the rest after.
fn collect_idle_facts(host: &Arc<Host>, session: &dyn mighty_platform::SessionWatch, now: DateTime<Utc>) -> IdleFacts {
    let runs = host.runs().cloned();
    let running = |id: &str| runs.as_ref().is_some_and(|runs| runs.service.is_session_running(id));
    let delegation = crate::delegation::app();
    let mut facts = IdleFacts::default();
    let terminals = crate::terminal::terminals(host).ok();
    {
        let view = host.view();
        let sessions = view.snapshot.sessions();
        // Agent sessions running (remote ones too), panes starting a run, delegation runs ending, its own work under way and children
        // still being made. (C#'s sign-in resends held for a CLI update have no count in accounts.rs.)
        let agent_runs = sessions.iter().filter(|s| s.kind() != "shell" && (s.status() == "running" || running(s.id()))).count()
            + view.panes.values().filter(|ui| ui.view.starting || ui.view.queue_starting).count()
            + sessions.iter().filter(|s| crate::delegation::ending_run(host, s.id())).count()
            + delegation.as_ref().and_then(|app| app.coordinator()).map_or(0, |coordinator| coordinator.pending_work())
            + delegation
                .as_ref()
                .map_or(0, |app| app.rows().iter().filter(|row| row.state == mighty_core::shell::ChildState::Creating).count());
        facts.agent_runs = count(agent_runs);
        // Permission, question and plan cards and Antigravity's card per pane, and requests to open a web page.
        let cards = |id: &str| {
            let card = runs.as_ref().is_some_and(|runs| runs.service.antigravity_card(id).is_some());
            mighty_workmap::dashboard::pending_with_card(&crate::panes::requests(host, id), card).total()
                + crate::agent_io::web_open_cards(host, id).len()
        };
        facts.pending_cards = count(view.panes.keys().map(|id| cards(id)).sum());
        // Only what the person queued: delegation holds live in their own store and survive a restart.
        facts.queued_inputs = count(view.panes.values().map(|ui| ui.queue.items().len()).sum());
        facts.drafts = count(view.panes.values().filter(|ui| ui.has_content() || ui.view.attachments_loading).count());
        // A shell running a command, agent terminals at work, setup, sign-ins (accounts.rs answers per provider).
        let shells = sessions.iter().filter(|s| s.kind() == "shell" && s.status() == "running").count();
        let agents = terminals.as_ref().map_or(0, |terminals| {
            sessions.iter().filter(|s| terminals.existing_agent(s.id()).is_some_and(|agent| agent.is_running())).count()
        });
        let setup = usize::from(lock(&crate::setup::of(host).screen).running());
        let sign_ins = mighty_core::wire::PROVIDERS.iter().filter(|provider| crate::accounts::busy(host, provider)).count();
        facts.terminals = count(shells + agents + setup + sign_ins);
        facts.last_agent_activity = mighty_core::wire::PROVIDERS
            .iter()
            .filter_map(|provider| crate::updates::provider_use_in(host, &view, provider).last_active)
            .max();
        facts.blocking_modal = view.dialog_open;
    }
    let activity = crate::updates::of(host).map(|updates| updates.activity());
    facts.cli_work = activity.as_ref().is_some_and(|a| a.any_cli_update_running() || a.manual_provider.is_some())
        || crate::plugins::operations_running(host)
        || crate::accounts::claude_model_resetting(host)
        || lock(&crate::settings::tools::toolkit_of(host).model).running;
    // Windows has no screen share yet (settings/screen.rs): never sharing.
    facts.screen_sharing = false;
    facts.phone_uploads = count(crate::phone::router(host).open_upload_count());
    let remote = crate::remote::host_state(host);
    facts.remote_host_runs = count(crate::remote::host_runs(host).max(remote.active_runs));
    facts.sharing_workspaces = remote.enabled;
    facts.blocking_modal = facts.blocking_modal
        || crate::dialogs::of(host).open_count() > 0
        || crate::native::is_open(host, crate::settings::SURFACE)
        || crate::native::is_open(host, mighty_tools::view::SETUP_SURFACE);
    // A failed read is input just now: the app never restarts on it.
    facts.last_user_input = Some(match session.since_last_input() {
        Ok(since) => now - chrono::Duration::from_std(since).unwrap_or(chrono::TimeDelta::zero()),
        Err(_) => now,
    });
    facts
}

fn count(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests;
