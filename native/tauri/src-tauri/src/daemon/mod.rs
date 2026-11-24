//! MC-3: work that keeps running after the app closes (설정 › 일반 › 앱을 닫아도 작업 계속, on unless the person turns it off).
//!
//! - The daemon is the app's own exe as `--daemon` ([`entry`]), started detached so it outlives the app, one per profile (its lock),
//!   listening on its own socket or pipe under `<data>/daemon/` with its own token ([`files`]). The app starts it when the setting is on
//!   (or connects to the one already running) and is then its client (mighty-run `daemon::client`).
//! - What it owns: the agent runs' CLIs and the terminal panes' shells. The app's process and PTY hosts are wrapped once ([`wrap`]);
//!   only those two go to the daemon, and only while it is connected (mighty-run `daemon::owner`). The run engine's parsing and state
//!   stay here; only the processes and their bytes move.
//! - Quitting while a daemon runs asks whether to stop the daemon too ([`on_exit_requested`]): quitting only the app lets go of kept
//!   work ([`before_shutdown`]) and leaves the daemon running, stopping the daemon ends its work and exits both; the update's restart
//!   always keeps. At the next start the app attaches again ([`take_back`]): a terminal
//!   pane gets its shell and scrollback back when it opens, an agent pane its run (read on from where the last app stopped,
//!   its waiting approvals asked for again). What no pane can take back is ended.
//! - Safety: a daemon that dies takes its children with it (the Mac's process groups, Windows' job objects) and the app reads them as
//!   ended; a daemon of another protocol (after an update) is never driven: the app offers to stop its work ([`offer_stop_old`]).

pub mod entry;
pub mod files;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use mighty_core::daemon::protocol::{Command, Kind, ProcInfo};
use mighty_native::surface::{DefaultButton, DialogButton, DialogResult, DialogSpec};
use mighty_native::ui::Content;
use mighty_platform::{DetachedEnvironment, DetachedRequest, Platform, PrefValue, PrefsStore};
use mighty_run::daemon::client::DaemonClient;
use mighty_run::daemon::owner::{Adoption, KEPT_STATE, RESUME_AT, Route, RoutedProcesses, RoutedPtys, parse_run_key};
use mighty_run::model::{KeptState, StartRunRequest};
use serde_json::{Value, json};
use tauri::Manager;

use crate::host::Host;

pub use entry::ARGUMENT;

/// The setting (a preference, on unless switched off).
pub const SETTING: &str = "daemon.keepRunning";
/// How long the app waits for a daemon it started to listen.
const START_WAIT: Duration = Duration::from_secs(8);
/// How long a connection to a daemon that may not be there is tried.
const REACH: Duration = Duration::from_millis(600);
/// How old the status shown in Settings may be before it is read again (only while Settings asks).
const STATUS_AGE: Duration = Duration::from_secs(2);

/// What the quit decided for the work the daemon holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quit {
    Keep,
    StopAll,
}

/// Where the app is with its daemon.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Phase {
    #[default]
    Off,
    Starting,
    Connected,
    /// A daemon of another protocol runs (its version).
    Mismatch(String),
    /// The connection was lost (the daemon stopped or died).
    Lost,
    Failed(String),
}

#[derive(Default)]
struct App {
    route: Arc<Route>,
    profile: Mutex<Option<PathBuf>>,
    client: Mutex<Option<Arc<DaemonClient>>>,
    phase: Mutex<Phase>,
    status: Mutex<Option<(Instant, Value)>>,
    quit: Mutex<Option<Quit>>,
    reading: AtomicBool,
    restarted: AtomicBool,
    /// The quit's question is up (a second ⌘Q waits for it).
    asking: AtomicBool,
}

fn app() -> &'static App {
    static APP: OnceLock<App> = OnceLock::new();
    APP.get_or_init(App::default)
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|p| p.into_inner())
}

fn redraw(host: &Host) {
    if crate::native::is_open(host, crate::settings::SURFACE) {
        crate::native::touch(host, crate::settings::SURFACE);
    }
}

/// The platform with its process and PTY hosts wrapped: a run's child and a terminal pane's shell go to the daemon while one is
/// connected; everything else, and everything while none is, as before. `profile` is the app's `--profile`, which a daemon it starts
/// gets too.
pub fn wrap(mut platform: Platform, profile: Option<PathBuf>) -> Platform {
    let state = app();
    *lock(&state.profile) = profile;
    platform.processes = Arc::new(RoutedProcesses::new(platform.processes.clone(), state.route.clone()));
    platform.ptys = Arc::new(RoutedPtys::new(platform.ptys.clone(), state.route.clone()));
    platform
}

fn prefs(host: &Host) -> Option<Arc<dyn PrefsStore>> {
    host.app.try_state::<Arc<dyn PrefsStore>>().map(|p| p.inner().clone())
}

/// Whether the person keeps work running after the app closes (on unless switched off).
pub fn enabled(host: &Host) -> bool {
    enabled_in(prefs(host).as_deref())
}

fn enabled_in(prefs: Option<&dyn PrefsStore>) -> bool {
    prefs.is_none_or(|p| mighty_platform::prefs::switch_on(p, SETTING))
}

/// Where the app is with its daemon now.
pub fn phase() -> Phase {
    lock(&app().phase).clone()
}

fn set_phase(host: &Host, phase: Phase) {
    *lock(&app().phase) = phase;
    redraw(host);
}

/// The daemon at the app's start, when the setting is on (never in a smoke, nor where the app may not write its profile).
pub fn start(host: &Arc<Host>) {
    if host.smoke.is_some() || !host.may_change_machine() {
        return;
    }
    let host = host.clone();
    tauri::async_runtime::spawn(async move {
        // What a crashed daemon left running is ended first, whatever the setting says now (a running one's is left alone).
        let groups = files::folder(&host.data_dir).join(files::GROUPS_FOLDER);
        let _ = tauri::async_runtime::spawn_blocking(move || crate::platform::reap_daemon_groups(&groups)).await;
        if enabled(&host) {
            connect(&host, true).await;
        }
    });
}

/// The setting turned on or off. On: the daemon starts (or the running one is connected) for the work started from now on. Off: new
/// work runs in the app again; what the daemon holds keeps running until it ends (or 데몬 중지).
pub fn set_enabled(host: &Arc<Host>, on: bool) {
    if !host.may_change_machine() {
        return;
    }
    if let Some(prefs) = prefs(host)
        && let Err(error) = prefs.set(SETTING, PrefValue::Bool(on))
    {
        host.log.line(&format!("daemon setting not saved: {error}"));
    }
    if on {
        let host = host.clone();
        tauri::async_runtime::spawn(async move {
            connect(&host, true).await;
        });
    } else {
        app().route.set(None);
        *lock(&app().client) = None;
        set_phase(host, Phase::Off);
    }
}

/// The running daemon, connected; None when none answers.
async fn reach(data_dir: &std::path::Path, timeout: Duration) -> Option<Arc<DaemonClient>> {
    let endpoint = files::read_endpoint(data_dir)?;
    let token = files::read_token(data_dir)?;
    let connector = crate::platform::daemon_connector();
    DaemonClient::connect(connector.as_ref(), &endpoint.address, &token, env!("CARGO_PKG_VERSION"), timeout).await.ok()
}

/// Starts the daemon detached (it outlives the app): the app's own exe as `--daemon`, with the app's `--profile`.
fn launch(host: &Host) -> Result<u32, String> {
    let program = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut args = vec![ARGUMENT.to_string()];
    let profile = lock(&app().profile).clone();
    if let Some(profile) = profile {
        args.push("--profile".into());
        args.push(profile.to_string_lossy().into_owned());
    }
    let cwd = crate::platform::detached_cwd(&program);
    let request = DetachedRequest { program, args, cwd, environment: DetachedEnvironment::Inherit, hidden: true };
    host.processes.start_detached(request).map_err(|e| e.to_string())
}

/// Connects to the profile's daemon, starting one first when none answers and `start` says so; then takes back what an earlier app
/// left and routes new work to it.
async fn connect(host: &Arc<Host>, start: bool) {
    set_phase(host, Phase::Starting);
    let mut client = reach(&host.data_dir, REACH).await;
    if client.is_none() && start {
        match launch(host) {
            Ok(pid) => host.log.line(&format!("daemon: started (pid {pid})")),
            Err(error) => {
                host.log.line(&format!("daemon: not started: {error}"));
                set_phase(host, Phase::Failed(error));
                return;
            }
        }
        // The new daemon writes its endpoint once it listens: looked for a few times a second, for a few seconds at most.
        let deadline = Instant::now() + START_WAIT;
        while client.is_none() && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(150)).await;
            client = reach(&host.data_dir, Duration::from_millis(400)).await;
        }
    }
    let Some(client) = client else {
        set_phase(host, Phase::Failed(host.locale.get("daemon.status.notStarted")));
        return;
    };
    if !client.accepted() {
        let version = client.welcome().app.clone();
        host.log.line(&format!("daemon: version {version} speaks protocol {}: not attached", client.welcome().protocol));
        *lock(&app().client) = Some(client.clone());
        set_phase(host, Phase::Mismatch(version));
        let host = host.clone();
        tauri::async_runtime::spawn(async move { offer_stop_old(&host, client).await });
        return;
    }
    host.log.line(&format!("daemon: connected (pid {})", client.welcome().pid));
    let processes = client.list().await.unwrap_or_default();
    take_back(host, &client, &processes).await;
    app().route.set(Some(client.clone()));
    *lock(&app().client) = Some(client.clone());
    set_phase(host, Phase::Connected);
    watch(host, client);
}

/// [`connect`] again (starting a daemon when none answers), as a future of its own type (it is awaited inside `connect`'s own tasks).
fn connect_again(host: Arc<Host>) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
    Box::pin(async move { connect(&host, true).await })
}

/// A daemon that goes: new work runs in the app again (what it held reads as ended); one that died is started again once.
fn watch(host: &Arc<Host>, client: Arc<DaemonClient>) {
    let host = host.clone();
    tauri::async_runtime::spawn(async move {
        client.closed().await;
        let current = lock(&app().client).as_ref().is_some_and(|c| Arc::ptr_eq(c, &client));
        if !current || host.closing() {
            return;
        }
        host.log.line("daemon: connection lost");
        app().route.set(None);
        *lock(&app().client) = None;
        set_phase(&host, Phase::Lost);
        if enabled(&host) && !app().restarted.swap(true, Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_secs(2)).await;
            if !host.closing() && enabled(&host) {
                connect_again(host.clone()).await;
            }
        }
    });
}

/// What an earlier app left in the daemon, handed back: each terminal to the shell pane it belongs to (given when the pane opens),
/// each kept agent run to its agent pane (read on from where that app stopped, with what that app left for it, MC-38). A process whose pane is gone, or that no pane here
/// can take back, is ended.
async fn take_back(host: &Arc<Host>, client: &Arc<DaemonClient>, processes: &[ProcInfo]) {
    let route = &app().route;
    let mut offered = Vec::new();
    let mut adopt = Vec::new();
    {
        let view = host.view();
        let snapshot = &view.snapshot;
        for info in processes {
            let pane = |id: &str| snapshot.session(id).cloned();
            let keep = match (info.kind, info.key.as_deref()) {
                (Kind::Pty, Some(key)) => {
                    let shell = pane(key).is_some_and(|p| p.kind() == "shell");
                    if shell && info.running {
                        offered.push(info.clone());
                    }
                    shell && info.running
                }
                (Kind::Run, Some(key)) => match parse_run_key(key) {
                    Some((id, true)) => match pane(id) {
                        // MC-38: an agent pane of any agent takes its run back, a delegated child's too; the run is the agent the
                        // pane names, as the earlier app left it.
                        Some(session)
                            if session.kind() == "claude"
                                && info
                                    .meta
                                    .get(KEPT_STATE)
                                    .and_then(|kept| kept.get("provider"))
                                    .and_then(Value::as_str)
                                    .is_none_or(|provider| provider == session.provider()) =>
                        {
                            let workspace = snapshot.workspace(session.workspace_id()).cloned();
                            let mut request = StartRunRequest::new(session.id(), session.workspace_id(), "claude", "(continue)");
                            request.registered_models =
                                crate::runs::registered_models_for(session.provider(), workspace.as_ref(), snapshot);
                            request.model = session.model().to_string();
                            request.provider = session.provider().to_string();
                            request.settings = session.settings.clone();
                            request.resume_id = session.resume_id.clone();
                            request.kept = info.meta.get(KEPT_STATE).cloned().map(KeptState);
                            let from = info.meta.get(RESUME_AT).and_then(Value::as_u64).unwrap_or(info.start);
                            adopt.push((key.to_string(), Adoption { proc: info.proc.clone(), pid: info.pid, from }, request));
                            true
                        }
                        _ => false,
                    },
                    _ => false,
                },
                _ => false,
            };
            if !keep {
                if info.running {
                    client.send(Command::Kill { proc: info.proc.clone() });
                }
                client.send(Command::Release { proc: info.proc.clone() });
            }
        }
    }
    route.offer(&offered);
    if !offered.is_empty() || !adopt.is_empty() {
        host.log.line(&format!("daemon: taking back {} terminals and {} runs", offered.len(), adopt.len()));
    }
    for (key, adoption, request) in adopt {
        let (host, proc) = (host.clone(), adoption.proc.clone());
        route.expect(&key, adoption);
        let releasing = client.clone();
        tauri::async_runtime::spawn(async move {
            let id = request.session_id.clone();
            // A styled pane's grants as a send would carry them; the run as it is when they cannot be read (it is never stopped for that).
            let request = crate::styles::prepare_run(&host, &id, request.clone()).await.unwrap_or(request);
            let adopted = match host.engine() {
                Ok(engine) => engine.service.adopt(request).await.map_err(|e| e.message().to_string()),
                Err(error) => Err(error),
            };
            if let Err(error) = adopted {
                host.log.line(&format!("daemon: a kept run was not taken back: {error}"));
                releasing.send(Command::Kill { proc: proc.clone() });
                releasing.send(Command::Release { proc });
            }
        });
    }
}

/// A daemon of another protocol (an earlier version's, after an update): never driven. The person is asked whether to stop its work;
/// stopped, a daemon of this version starts in its place.
async fn offer_stop_old(host: &Arc<Host>, client: Arc<DaemonClient>) {
    let running = client.status().await.ok().and_then(|s| s.get("running").and_then(Value::as_u64)).unwrap_or(0);
    if running == 0 {
        let _ = client.request(Command::Shutdown).await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        connect_again(host.clone()).await;
        return;
    }
    let locale = host.locale;
    let version = client.welcome().app.clone();
    let text = locale.get_with("daemon.mismatch.message", &[("version", &version), ("count", &running.to_string())]);
    let mut spec = DialogSpec::new(locale.get("daemon.mismatch.title"), move |_| Content::Text(text.clone())).styled(None, None);
    spec.primary = Some(DialogButton::new(locale.get("daemon.mismatch.stop")));
    spec.close = Some(locale.get("daemon.mismatch.leave"));
    spec.default_button = DefaultButton::Close;
    if crate::native::show_dialog(host, spec).await == DialogResult::Primary {
        stop(host).await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        connect_again(host.clone()).await;
    }
}

/// 데몬 중지: every process the daemon holds ends and it exits (whatever its version). Work the app runs from then on runs in the app.
pub async fn stop(host: &Arc<Host>) {
    let held = lock(&app().client).clone().filter(|client| !client.is_closed());
    let client = match held {
        Some(client) => Some(client),
        None => reach(&host.data_dir, REACH).await,
    };
    app().route.set(None);
    *lock(&app().client) = None;
    if let Some(client) = client {
        let _ = client.request(Command::Shutdown).await;
        host.log.line("daemon: stopped by the person");
    }
    *lock(&app().status) = None;
    set_phase(host, Phase::Off);
}

/// The daemon's status as Settings shows it: read again (on a task) when older than a moment, only while something asks.
pub fn status(host: &Arc<Host>) -> Option<Value> {
    let state = app();
    let cached = lock(&state.status).clone();
    let stale = cached.as_ref().is_none_or(|(at, _)| at.elapsed() > STATUS_AGE);
    if stale && !state.reading.swap(true, Ordering::SeqCst) {
        let host = host.clone();
        tauri::async_runtime::spawn(async move {
            let client = lock(&app().client).clone().filter(|c| !c.is_closed());
            let read = match client {
                Some(client) => client.status().await.ok(),
                None => None,
            };
            let changed = {
                let mut status = lock(&app().status);
                let changed = status.as_ref().map(|(_, v)| v) != read.as_ref();
                *status = read.map(|value| (Instant::now(), value));
                changed
            };
            app().reading.store(false, Ordering::SeqCst);
            if changed {
                redraw(&host);
            }
        });
    }
    cached.map(|(_, value)| value)
}

/// How many processes the daemon runs now (the quit's question; `status` answers across protocol versions).
async fn running(client: &DaemonClient) -> usize {
    if client.accepted() {
        client.list().await.map(|list| list.iter().filter(|p| p.running).count()).unwrap_or(0)
    } else {
        client.status().await.ok().and_then(|s| s.get("running").and_then(Value::as_u64)).unwrap_or(0) as usize
    }
}

/// The quit's dialog when a daemon is running: asks whether to stop the daemon too (`Secondary` → [`Quit::StopAll`]) or quit only the
/// app (`Primary` → [`Quit::Keep`]).
fn quit_dialog(locale: mighty_core::locale::Locale, count: usize) -> DialogSpec {
    let text = if count > 0 {
        locale.get_with("daemon.quit.message", &[("count", &count.to_string())])
    } else {
        locale.get("daemon.quit.idleMessage")
    };
    let mut spec = DialogSpec::new(locale.get("daemon.quit.title"), move |_| Content::Text(text.clone())).styled(None, None);
    spec.primary = Some(DialogButton::new(locale.get("daemon.quit.keep")));
    spec.secondary = Some(DialogButton::new(locale.get("daemon.quit.stopAll")));
    spec.close = Some(locale.get("daemon.quit.cancel"));
    spec.default_button = DefaultButton::Primary;
    spec
}

fn quit_choice(answer: DialogResult) -> Option<Quit> {
    match answer {
        DialogResult::Primary => Some(Quit::Keep),
        DialogResult::Secondary => Some(Quit::StopAll),
        DialogResult::None => None,
    }
}

/// The update's restart keeps the daemon's work whatever the person chose before.
pub fn keep_for_update() {
    quit_as(Quit::Keep);
}

/// The quit's choice made without asking (the update's restart; MC-3's headless server stopping, server.rs `stop_choice`).
pub fn quit_as(choice: Quit) {
    *lock(&app().quit) = Some(choice);
}

/// ⌘Q (or the window's close) while a daemon runs: the quit waits for the person's answer — quit only the app (keep the daemon
/// running), stop the daemon too, or stay. True when the quit was held.
pub fn on_exit_requested(host: &Arc<Host>, hold: impl FnOnce()) -> bool {
    if lock(&app().quit).is_some() || host.smoke.is_some() || !host.may_change_machine() {
        return false;
    }
    if app().asking.load(Ordering::SeqCst) {
        hold();
        return true;
    }
    let connected = lock(&app().client).clone().filter(|c| !c.is_closed());
    if connected.is_none() && files::read_endpoint(&host.data_dir).is_none() {
        return false;
    }
    // A window that shows (MC-3: the hidden window of an app started without one does not).
    let alive = crate::server::window_shown(host);
    if !alive {
        // No window left to ask in (Windows' last window closed, the headless server): the setting's own meaning, keep.
        if enabled(host) {
            *lock(&app().quit) = Some(Quit::Keep);
        }
        return false;
    }
    hold();
    app().asking.store(true, Ordering::SeqCst);
    let host = host.clone();
    tauri::async_runtime::spawn(async move {
        let client = match connected {
            Some(client) => Some(client),
            None => reach(&host.data_dir, REACH).await,
        };
        let Some(client) = client else {
            app().asking.store(false, Ordering::SeqCst);
            *lock(&app().quit) = Some(Quit::Keep);
            host.app.exit(0);
            return;
        };
        let count = running(&client).await;
        let spec = quit_dialog(host.locale, count);
        let choice = quit_choice(crate::native::show_dialog(&host, spec).await);
        app().asking.store(false, Ordering::SeqCst);
        if let Some(choice) = choice {
            *lock(&app().quit) = Some(choice);
            host.app.exit(0);
        }
    });
    true
}

/// The app quits (before the terminals close and the runs are disposed): kept work is let go of, so closing the panes and stopping
/// the runs leaves it running in the daemon.
pub fn before_shutdown(host: &Host) {
    let keep = *lock(&app().quit) == Some(Quit::Keep);
    if keep && app().route.connected() {
        host.log.line("daemon: the app quits, its work keeps running");
        // MC-38: the runs let go of end here as the quit's: delegation records a kept child's run as killed by quitting (the next app
        // carries it on), never as stopped by a human.
        if let Some(runs) = host.runs() {
            runs.service.letting_go();
        }
        tauri::async_runtime::block_on(async {
            let _ = tokio::time::timeout(Duration::from_secs(5), app().route.detach()).await;
        });
    }
}

/// The app quits (after the runs are disposed): when it stopped the daemon too, the daemon exits.
pub fn after_shutdown(host: &Host) {
    let stop_all = *lock(&app().quit) == Some(Quit::StopAll);
    if !stop_all {
        return;
    }
    let held = lock(&app().client).clone().filter(|c| !c.is_closed());
    tauri::async_runtime::block_on(async {
        let client = match held {
            Some(client) => Some(client),
            None => reach(&host.data_dir, REACH).await,
        };
        if let Some(client) = client {
            let _ = tokio::time::timeout(Duration::from_secs(3), client.request(Command::Shutdown)).await;
            host.log.line("daemon: stopped with the app");
        }
    });
}

/// Settings' status line words for the phase and the status read.
pub fn status_words(host: &Arc<Host>) -> String {
    let locale = host.locale;
    match phase() {
        Phase::Off => locale.get("daemon.status.off"),
        Phase::Starting => locale.get("daemon.status.starting"),
        Phase::Lost => locale.get("daemon.status.lost"),
        Phase::Failed(reason) => locale.get_with("daemon.status.failed", &[("reason", &reason)]),
        Phase::Mismatch(version) => locale.get_with("daemon.status.mismatch", &[("version", &version)]),
        Phase::Connected => match status(host) {
            Some(status) => {
                let number = |key: &str| status.get(key).and_then(Value::as_u64).unwrap_or(0);
                let memory = format!("{:.1}", number("bufferedBytes") as f64 / 1_048_576.0);
                locale.get_with(
                    "daemon.status.running",
                    &[
                        ("pid", &number("pid").to_string()),
                        ("running", &number("running").to_string()),
                        ("processes", &number("processes").to_string()),
                        ("memory", &memory),
                    ],
                )
            }
            None => locale.get("daemon.status.connected"),
        },
    }
}

/// Whether there is a daemon to stop (Settings' 데몬 중지).
pub fn stoppable() -> bool {
    matches!(phase(), Phase::Connected | Phase::Mismatch(_))
}

/// The phone's and the CLI's view of it (for `status`): the phase and the last status read.
#[allow(dead_code, reason = "a seam the phone's settings and MC-32's triggers read")]
pub fn summary() -> Value {
    let status = lock(&app().status).clone().map(|(_, v)| v);
    json!({ "phase": format!("{:?}", phase()), "status": status })
}

#[cfg(test)]
mod tests {
    use mighty_core::locale::Locale;
    use mighty_platform::fake::MemoryPrefs;

    use super::*;

    #[test]
    fn the_daemon_is_on_by_default_until_switched_off() {
        assert!(enabled_in(None), "no store: on by default");
        let prefs = MemoryPrefs::default();
        assert!(enabled_in(Some(&prefs)), "unset key: on by default");
        prefs.set(SETTING, PrefValue::Bool(false)).unwrap();
        assert!(!enabled_in(Some(&prefs)), "switched off");
        prefs.set(SETTING, PrefValue::Bool(true)).unwrap();
        assert!(enabled_in(Some(&prefs)), "switched back on");
        prefs.set(SETTING, PrefValue::Int(0)).unwrap();
        assert!(!enabled_in(Some(&prefs)), "DWORD 0 is off");
    }

    #[test]
    fn the_quit_dialog_asks_for_both_idle_and_busy_daemons_and_maps_each_answer() {
        for language in ["ko", "en", "zh", "ja"] {
            let locale = Locale::new(language);
            let idle: Value = serde_json::from_str(&crate::native::golden_dialog(quit_dialog(locale, 0))).unwrap();
            let busy: Value = serde_json::from_str(&crate::native::golden_dialog(quit_dialog(locale, 3))).unwrap();
            assert_eq!(idle["view"]["title"].as_str(), Some(locale.get("daemon.quit.title").as_str()), "{language}");
            assert_eq!(
                idle["view"]["sections"][0]["node"]["text"].as_str(),
                Some(locale.get("daemon.quit.idleMessage").as_str()),
                "{language}"
            );
            let buttons = idle["view"]["buttons"].as_array().unwrap();
            let label_of = |id: &str| buttons.iter().find(|b| b["id"] == id).and_then(|b| b["label"].as_str());
            assert_eq!(label_of("primary"), Some(locale.get("daemon.quit.keep").as_str()), "{language}");
            assert_eq!(label_of("secondary"), Some(locale.get("daemon.quit.stopAll").as_str()), "{language}");
            assert_eq!(label_of("close"), Some(locale.get("daemon.quit.cancel").as_str()), "{language}");
            let busy_text = busy["view"]["sections"][0]["node"]["text"].as_str().unwrap();
            assert!(busy_text.contains('3') && !busy_text.contains("{count}"), "{language}: {busy_text}");
        }
        assert_eq!(quit_choice(DialogResult::Primary), Some(Quit::Keep), "UI only: keep daemon running");
        assert_eq!(quit_choice(DialogResult::Secondary), Some(Quit::StopAll), "stop daemon and quit UI");
        assert_eq!(quit_choice(DialogResult::None), None, "cancel stays in the app");
    }
}
