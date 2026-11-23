//! MC-3: one app per profile around the headless server. Before anything is loaded (main.rs, after the arguments), a start looks
//! for an app already answering on the profile's control API (`server.status`):
//! - a `--server` start ends at once (0) when any app runs for the profile, with or without its window;
//! - a start with a window hands over to an app that started without one: it asks it for its window (`app.show`) and ends (0);
//! - otherwise the start goes on as before (a running app with its own window is left to the Mac's LaunchServices and the write
//!   policy, as before the server existed).
//!
//! Nothing is asked when the profile has no endpoint file, and nothing at all in a smoke. A left-over endpoint of an app that crashed
//! refuses the connection at once (nobody listens), so a start after a crash goes on.

use std::time::Duration;

use mighty_core::control::protocol::{Command, ControlError, ErrorCode};
use serde_json::Value;

use crate::args::Options;

/// How long the start waits for a running app's answer.
const ASK: Duration = Duration::from_secs(2);

/// How this start runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Launch {
    /// With its window (a normal start).
    Window,
    /// `--server`, without one.
    Server,
}

/// An app found running for the profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Running {
    /// It started without a window (`--server`); an app that answered but not as this version does counts as one with a window.
    pub started_headless: bool,
}

impl Running {
    /// What `server.status` answered (Ok), or the error of an app that did answer (an older app that does not know the command).
    pub fn from_answer(answer: &Result<Value, ControlError>) -> Option<Running> {
        match answer {
            Ok(status) => Some(Running { started_headless: status.get("startedHeadless").and_then(Value::as_bool) == Some(true) }),
            Err(error) if not_running(error) => None,
            Err(_) => Some(Running { started_headless: false }),
        }
    }
}

/// What the start does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handoff {
    /// Goes on: nothing runs for the profile (or a windowed app does, and this start has a window too: as before).
    Start,
    /// Ends: an app already runs for the profile and this is a `--server` start.
    AlreadyRunning,
    /// Ends once the running app (started without a window) was asked for its window.
    ShowThere,
}

/// How `options` start: `--server` (also `--server --for-schedules`, MC-39: the OS's start for a schedule, which an app already
/// running for the profile ends at once) or with a window.
pub fn launch_of(options: &Options) -> Launch {
    if options.server { Launch::Server } else { Launch::Window }
}

pub fn decide(launch: Launch, running: Option<Running>) -> Handoff {
    match (launch, running) {
        (_, None) => Handoff::Start,
        (Launch::Server, Some(_)) => Handoff::AlreadyRunning,
        (Launch::Window, Some(Running { started_headless: true })) => Handoff::ShowThere,
        (Launch::Window, Some(Running { started_headless: false })) => Handoff::Start,
    }
}

/// Whether `error` says nobody is there (no endpoint, nobody listening). An app that took the connection but did not answer in time is
/// there (busy, or starting): it counts as running.
fn not_running(error: &ControlError) -> bool {
    error.code == ErrorCode::Failed && error.message.starts_with("Mighty Code is not running")
}

/// main.rs: the exit code when this start ends here, None when it goes on.
pub fn at_start(options: &Options) -> Option<i32> {
    if options.smoke_test {
        return None;
    }
    let data_dir = crate::platform::data_dir(options.profile.clone()).ok()?;
    // Nothing has listened on this profile (or the last app ended cleanly): nothing to ask.
    crate::control::endpoint::read_endpoint(&data_dir)?;
    let connector = crate::platform::control_connector();
    let ask =
        |command: Command| tauri::async_runtime::block_on(crate::control::client::request(&data_dir, connector.as_ref(), command, ASK));
    let launch = launch_of(options);
    let answer = ask(Command::ServerStatus);
    let pid = answer.as_ref().ok().and_then(|status| status.get("pid")).and_then(Value::as_u64).unwrap_or(0);
    match decide(launch, Running::from_answer(&answer)) {
        Handoff::Start => None,
        Handoff::AlreadyRunning => {
            // MC-39: a start for schedules leaves the fire to the app that runs (its own timer fires it).
            eprintln!("Mighty Code is already running for this profile (pid {pid}); this start ends.");
            Some(0)
        }
        // The running app makes or shows its window; when it cannot (it is quitting), this start goes on.
        Handoff::ShowThere => match ask(Command::AppShow) {
            Ok(_) => {
                eprintln!("Mighty Code runs without a window for this profile (pid {pid}): its window is shown there.");
                Some(0)
            }
            Err(_) => None,
        },
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_second_start_never_runs_beside_a_server() {
        let server = Some(Running { started_headless: true });
        let windowed = Some(Running { started_headless: false });
        assert_eq!(decide(Launch::Window, None), Handoff::Start);
        assert_eq!(decide(Launch::Server, None), Handoff::Start);
        assert_eq!(decide(Launch::Window, server), Handoff::ShowThere, "the server shows its window");
        assert_eq!(decide(Launch::Server, server), Handoff::AlreadyRunning);
        assert_eq!(decide(Launch::Server, windowed), Handoff::AlreadyRunning, "a server never starts beside a windowed app");
        assert_eq!(decide(Launch::Window, windowed), Handoff::Start, "two windowed starts as before");
    }

    #[test]
    fn the_running_app_is_read_from_its_answer() {
        let headless = Ok(json!({ "running": true, "startedHeadless": true, "pid": 7 }));
        assert_eq!(Running::from_answer(&headless), Some(Running { started_headless: true }));
        assert_eq!(Running::from_answer(&Ok(json!({ "startedHeadless": false }))), Some(Running { started_headless: false }));
        assert_eq!(Running::from_answer(&Ok(json!({}))), Some(Running { started_headless: false }));
        let older = Err(ControlError::bad("unknown command 'server.status'"));
        assert_eq!(Running::from_answer(&older), Some(Running { started_headless: false }), "an older app answered: it runs");
        let absent = Err(ControlError::new(ErrorCode::Failed, "Mighty Code is not running for this profile (/tmp/p)"));
        assert_eq!(Running::from_answer(&absent), None);
        let busy = Err(ControlError::new(ErrorCode::Timeout, "the app did not answer"));
        assert_eq!(Running::from_answer(&busy), Some(Running { started_headless: false }), "it took the connection: it is there");
        let closing = Err(ControlError::new(ErrorCode::Failed, "the app is closing"));
        assert_eq!(Running::from_answer(&closing), Some(Running { started_headless: false }));
    }
}
