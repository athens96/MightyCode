//! MC-3: the headless server — the whole app (the host, the run engine, the phone relay, the control API, the mirror host, MC-32's
//! schedules, the background daemon's link) running without a window: `MightyCode --server [--profile <absolute folder>]`. Not the
//! process daemon (`MightyCode --daemon [--headless]`, daemon/entry.rs), which owns processes and bytes only and runs no host.
//!
//! - Start: main.rs's setup makes no window, no native views and no pet ([`started`]); on the Mac the app is an accessory (no Dock
//!   icon, no menu bar). The host logs `headless: started`. Everything that looks for the main window finds none and does without
//!   (a push to the page goes nowhere, a dialog waits for a page, the ports are not read).
//! - A window later, in the same process ([`show_window`]): the Mac's reopen (the Dock, Finder), a second start of the profile
//!   ([`handoff`], which asks this app through the control API's `app.show` and ends), `mighty server show`, a notification or a
//!   phone pairing that needs the person. It is made as at a normal start (main.rs `open_main_window`): its native views and the pet
//!   attach then. On the Mac the app becomes a regular one while it shows.
//! - Closing that window hides it (the app keeps running, back without a window, an accessory again); ⌘Q from it asks whether the
//!   server stops or only the window closes ([`on_exit_requested`]).
//! - Stop: `mighty server stop` (`server.stop`), SIGTERM or Ctrl+C (a console's close on Windows) quit through the normal quit
//!   ([`stop`]). The daemon's work is kept only when its setting (앱을 닫아도 작업 계속) is on, unless `--stop-all` says otherwise.
//! - Start at login ([`login`]): a LaunchAgent on the Mac, the HKCU Run key on Windows, off unless the person turns it on.

pub mod handoff;
pub mod login;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use mighty_native::surface::{DefaultButton, DialogButton, DialogResult, DialogSpec};
use mighty_native::ui::Content;
use mighty_platform::{DetachedEnvironment, DetachedRequest};
use serde_json::{Value, json};
use tauri::Manager;

use crate::daemon::{Phase, Quit};
use crate::host::{Host, WINDOW};

/// The app's argument that starts it without a window.
pub const ARGUMENT: &str = "--server";

/// This process: whether it started without a window, its `--profile`, when it started.
struct Run {
    headless: bool,
    profile: Option<PathBuf>,
    at: Instant,
}

static RUN: OnceLock<Run> = OnceLock::new();
/// The window of an app started without one was closed: it is hidden, not gone.
static HIDDEN: AtomicBool = AtomicBool::new(false);
/// A window is being made (a second ask waits for it).
static OPENING: AtomicBool = AtomicBool::new(false);
/// ⌘Q's question is up.
static ASKING: AtomicBool = AtomicBool::new(false);

/// main.rs, first thing after the arguments: how this process runs.
pub fn begin(headless: bool, profile: Option<PathBuf>) {
    let _ = RUN.set(Run { headless, profile, at: Instant::now() });
}

/// Whether this process started without a window (`--server`).
pub fn started_headless() -> bool {
    RUN.get().is_some_and(|run| run.headless)
}

/// The app's `--profile`, if any.
pub fn profile() -> Option<PathBuf> {
    RUN.get().and_then(|run| run.profile.clone())
}

pub fn uptime_seconds() -> u64 {
    RUN.get().map_or(0, |run| run.at.elapsed().as_secs())
}

/// The setup of an app started with `--server`, in the window's place: nobody looks at any pane, the OS's end signals quit through
/// the normal quit, and the log says so.
pub fn started(host: &Arc<Host>) {
    crate::attention::set_front(false);
    host.log.line(&format!("headless: started (pid {}, profile {})", std::process::id(), host.data_dir.display()));
    let watcher = host.clone();
    tauri::async_runtime::spawn(async move {
        stopped_by_signal().await;
        stop(&watcher, false, "signal");
    });
}

/// The event loop runs (`RunEvent::Ready`): on the Mac the windowing layer has just activated the app at launch, which would take the
/// keyboard from the app the person is in (a Terminal that ran `mighty server start`, the login's first app); an app started without
/// a window hands it back by hiding itself (it has nothing to show).
#[cfg(target_os = "macos")]
pub fn launched(app: &tauri::AppHandle) {
    if started_headless() && app.get_webview_window(WINDOW).is_none() {
        let _ = app.hide();
    }
}

/// Whether the main window is there and showing (not the hidden window of an app started without one).
pub fn window_shown(host: &Host) -> bool {
    host.app.get_webview_window(WINDOW).is_some() && !HIDDEN.load(Ordering::SeqCst)
}

/// The window's words for the status: none yet, hidden (closed in an app started without one), or shown.
fn window_word(host: &Host) -> &'static str {
    match (host.app.get_webview_window(WINDOW).is_some(), HIDDEN.load(Ordering::SeqCst)) {
        (false, _) => "none",
        (true, true) => "hidden",
        (true, false) => "shown",
    }
}

#[cfg(target_os = "macos")]
fn set_regular(host: &Host, regular: bool) {
    let policy = if regular { tauri::ActivationPolicy::Regular } else { tauri::ActivationPolicy::Accessory };
    if let Err(error) = host.app.set_activation_policy(policy) {
        host.log.line(&format!("headless: the activation policy was not changed: {error}"));
    }
}

/// The main window shown: brought forward when it is there (hidden or not), made first when this app started without one (on the
/// main thread, as the setup makes it). What happened: `shown` or `opening`.
pub fn show_window(host: &Arc<Host>) -> &'static str {
    #[cfg(target_os = "macos")]
    if started_headless() {
        set_regular(host, true);
        // Unhidden after `launched` hid it.
        let _ = host.app.show();
    }
    if let Some(window) = host.app.get_webview_window(WINDOW) {
        if HIDDEN.swap(false, Ordering::SeqCst) {
            host.log.line("headless: window shown again");
        }
        crate::attention::set_front(true);
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
        return "shown";
    }
    if OPENING.swap(true, Ordering::SeqCst) {
        return "opening";
    }
    // The state's theme read here, never on the main thread (a thread holding the state may be waiting for it).
    let theme = host.view().snapshot.theme().to_string();
    let (app, opened) = (host.app.clone(), host.clone());
    let sent = host.app.run_on_main_thread(move || {
        let made = crate::open_main_window(&app, &theme);
        OPENING.store(false, Ordering::SeqCst);
        match made {
            Ok(()) => {
                opened.log.line("headless: window opened");
                crate::attention::set_front(true);
                if let Some(window) = app.get_webview_window(WINDOW) {
                    let _ = window.set_focus();
                }
            }
            Err(error) => opened.log.line(&format!("headless: the window was not made: {error}")),
        }
    });
    if let Err(error) = sent {
        OPENING.store(false, Ordering::SeqCst);
        host.log.line(&format!("headless: the window was not asked for: {error}"));
    }
    "opening"
}

/// Brings the main window forward for something the person must see (a notification's pane, a phone's pairing, a folder opened from
/// outside): in an app started without a window it is shown (made first when there is none); otherwise as before, the window that is
/// there shown and focused.
pub fn bring_forward(host: &Arc<Host>) {
    if started_headless() {
        show_window(host);
    } else if let Some(window) = host.app.get_webview_window(WINDOW) {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// The app is back without a window: its window hidden, nobody looking at any pane, on the Mac an accessory again.
fn hide_window(host: &Host) {
    if let Some(window) = host.app.get_webview_window(WINDOW) {
        let _ = window.hide();
    }
    HIDDEN.store(true, Ordering::SeqCst);
    crate::attention::set_front(false);
    #[cfg(target_os = "macos")]
    set_regular(host, false);
    host.log.line("headless: window closed; the server keeps running");
}

/// main.rs, as the main window is made: in an app started without one, closing the window hides it (the app keeps running).
pub fn watch_window(host: &Arc<Host>, window: &tauri::WebviewWindow) {
    if !started_headless() {
        return;
    }
    let weak = Arc::downgrade(host);
    window.on_window_event(move |event| {
        if let tauri::WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            if let Some(host) = weak.upgrade() {
                hide_window(&host);
            }
        }
    });
}

/// The Mac's reopen (the Dock icon, the app opened again from Finder): an app started without a window shows it.
#[cfg(target_os = "macos")]
pub fn reopened(host: &Arc<Host>, has_visible_windows: bool) {
    if started_headless() && (!has_visible_windows || !window_shown(host)) {
        show_window(host);
    }
}

/// ⌘Q in the shown window of an app started without one: the quit waits for the person — close only the window (the server keeps
/// running), stop the server (then the daemon's own question, as any quit), or stay. True when the quit was held. An app started with
/// its window, or one whose window does not show (the OS ending the session), quits as before.
pub fn on_exit_requested(host: &Arc<Host>, hold: impl FnOnce()) -> bool {
    if !started_headless() || !window_shown(host) {
        return false;
    }
    hold();
    if ASKING.swap(true, Ordering::SeqCst) {
        return true;
    }
    let host = host.clone();
    tauri::async_runtime::spawn(async move {
        let locale = host.locale;
        let text = locale.get("server.quit.message");
        let mut spec = DialogSpec::new(locale.get("server.quit.title"), move |_| Content::Text(text.clone())).styled(None, None);
        spec.primary = Some(DialogButton::new(locale.get("server.quit.closeWindow")));
        spec.secondary = Some(DialogButton::new(locale.get("server.quit.stop")));
        spec.close = Some(locale.get("server.quit.cancel"));
        spec.default_button = DefaultButton::Primary;
        let answer = crate::native::show_dialog(&host, spec).await;
        ASKING.store(false, Ordering::SeqCst);
        match answer {
            DialogResult::Primary => hide_window(&host),
            DialogResult::Secondary => {
                host.log.line("headless: stopping (quit from the window)");
                // The daemon's question comes next when it holds running work; it quits itself once answered.
                if !crate::daemon::on_exit_requested(&host, || {}) {
                    host.app.exit(0);
                }
            }
            DialogResult::None => {}
        }
    });
    true
}

/// What a stop keeps of the background daemon's work: everything (`stop_all`) ends it; otherwise it keeps running when the daemon
/// setting is on (the setting's own meaning, as a quit without a window to ask in), and nothing is decided when it is off (nothing runs
/// there for this app).
pub fn stop_choice(stop_all: bool, daemon_on: bool) -> Option<Quit> {
    match (stop_all, daemon_on) {
        (true, _) => Some(Quit::StopAll),
        (false, true) => Some(Quit::Keep),
        (false, false) => None,
    }
}

/// `mighty server stop`, SIGTERM, Ctrl+C: the app quits through its normal quit (RunEvent::Exit's teardown, the last save), the
/// daemon's work as [`stop_choice`] says. Whether the daemon keeps it.
pub fn stop(host: &Arc<Host>, stop_all: bool, why: &str) -> bool {
    let choice = stop_choice(stop_all, crate::daemon::enabled(host));
    if let Some(choice) = choice {
        crate::daemon::quit_as(choice);
    }
    let kept = choice == Some(Quit::Keep) && matches!(crate::daemon::phase(), Phase::Connected);
    host.log.line(&format!("headless: stopping ({why}{})", if kept { ", the daemon's work kept" } else { "" }));
    host.app.exit(0);
    kept
}

/// `server.status`: how this app runs now.
pub fn status(host: &Host) -> Value {
    let shown = window_shown(host);
    json!({
        "running": true,
        "mode": if shown { "windowed" } else { "headless" },
        "startedHeadless": started_headless(),
        "window": window_word(host),
        "version": host.version,
        "pid": std::process::id(),
        "uptimeSeconds": uptime_seconds(),
        "dataDir": host.data_dir,
        "daemon": phase_name(&crate::daemon::phase()),
        "startAtLogin": login::installed(host),
    })
}

/// The daemon's phase as one word.
pub fn phase_name(phase: &Phase) -> &'static str {
    match phase {
        Phase::Off => "off",
        Phase::Starting => "starting",
        Phase::Connected => "connected",
        Phase::Mismatch(_) => "mismatch",
        Phase::Lost => "lost",
        Phase::Failed(_) => "failed",
    }
}

/// `mighty server start`: this exe started detached (it outlives the CLI; the Mac a session of its own, Windows outside the CLI's
/// job) as `--server`, with the profile when one was given. Its pid.
pub fn launch_detached(profile: Option<&Path>) -> Result<u32, String> {
    let program = std::env::current_exe().map_err(|e| e.to_string())?;
    let cwd = program.parent().map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let request = DetachedRequest { program, args: arguments(profile), cwd, environment: DetachedEnvironment::Inherit, hidden: true };
    crate::platform::process_host().start_detached(request).map_err(|e| e.to_string())
}

/// The arguments of an app started without a window for `profile`.
pub fn arguments(profile: Option<&Path>) -> Vec<String> {
    let mut args = vec![ARGUMENT.to_string()];
    if let Some(profile) = profile {
        args.push("--profile".into());
        args.push(profile.to_string_lossy().into_owned());
    }
    args
}

/// Ends when the OS asks the process to end: SIGTERM (launchd at logout, `kill`) or Ctrl+C on the Mac; Ctrl+C, Ctrl+Break or the
/// console's close on Windows (an app started from a console; the release build has none).
async fn stopped_by_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let Ok(mut terminate) = signal(SignalKind::terminate()) else {
            let _ = tokio::signal::ctrl_c().await;
            return;
        };
        tokio::select! {
            _ = terminate.recv() => {}
            _ = tokio::signal::ctrl_c() => {}
        }
    }
    #[cfg(windows)]
    {
        use tokio::signal::windows::{ctrl_break, ctrl_close};
        let (Ok(mut close), Ok(mut brk)) = (ctrl_close(), ctrl_break()) else {
            if tokio::signal::ctrl_c().await.is_err() {
                std::future::pending::<()>().await;
            }
            return;
        };
        tokio::select! {
            _ = close.recv() => {}
            _ = brk.recv() => {}
            _ = tokio::signal::ctrl_c() => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stop keeps the daemon's work only when its setting says so, and `--stop-all` ends it whatever the setting.
    #[test]
    fn a_stop_keeps_the_daemons_work_only_as_its_setting_says() {
        assert_eq!(stop_choice(false, false), None, "the setting off: nothing runs there for this app, nothing is decided");
        assert_eq!(stop_choice(false, true), Some(Quit::Keep));
        assert_eq!(stop_choice(true, true), Some(Quit::StopAll));
        assert_eq!(stop_choice(true, false), Some(Quit::StopAll));
    }

    #[test]
    fn the_server_is_started_with_its_own_switch_and_the_profile() {
        assert_eq!(arguments(None), vec!["--server".to_string()]);
        let folder = if cfg!(windows) { r"C:\Users\me\profile one" } else { "/Users/me/profile one" };
        assert_eq!(arguments(Some(Path::new(folder))), vec!["--server".to_string(), "--profile".into(), folder.into()]);
        assert_ne!(ARGUMENT, crate::daemon::entry::HEADLESS, "never the daemon's switch");
    }

    #[test]
    fn the_daemons_phase_reads_as_one_word() {
        assert_eq!(phase_name(&Phase::Off), "off");
        assert_eq!(phase_name(&Phase::Connected), "connected");
        assert_eq!(phase_name(&Phase::Mismatch("0.5.0".into())), "mismatch");
        assert_eq!(phase_name(&Phase::Failed("x".into())), "failed");
    }
}
