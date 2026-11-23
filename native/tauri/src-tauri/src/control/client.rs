//! MC-9: the `mighty` CLI (`MightyCode --cli …`, which the installed `mighty` script runs for its control commands): reads the
//! profile's endpoint and token, sends one request, prints the answer as JSON (each event of `events` on its own line) and exits 0,
//! 1 on an error, 2 on a timeout. It starts no window, reads no state and writes nothing. MC-3: `server start` is the one command that
//! starts something, the app itself without a window (`--server`, detached), then waits for its control API to answer; `server stop`
//! asks the app to quit and waits until it has.

use std::io::{Read as _, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use mighty_core::control::cli::{self, EXIT_ERROR, EXIT_OK, Invocation, USAGE};
use mighty_core::control::protocol::{Command, ControlError, ErrorCode, LineBuffer, MAX_TEXT_BYTES, Reply, Request, parse_reply};
use mighty_platform::AgentIoConnector;
use serde_json::{Value, json};

use super::endpoint;

/// The exe's argument that starts the CLI.
pub const ARGUMENT: &str = "--cli";
/// How long the CLI waits for the app's socket or pipe.
const CONNECT: Duration = Duration::from_secs(3);
/// The longest answer line the CLI reads (a long transcript).
const MAX_ANSWER_BYTES: usize = 32 * 1024 * 1024;

/// The exe mode: `args` after `--cli`. Never starts the app.
pub fn run(args: &[String]) -> i32 {
    let invocation = match cli::parse(args) {
        Ok(invocation) => invocation,
        Err(error) => {
            let _ = writeln!(std::io::stderr(), "mighty: {}\n\n{USAGE}", error.0);
            return EXIT_ERROR;
        }
    };
    let (mut out, mut err) = (std::io::stdout(), std::io::stderr());
    let profile = match &invocation {
        Invocation::Run { profile, .. } | Invocation::ServerStart { profile } => profile.clone(),
        Invocation::Help => None,
    };
    let data_dir = match data_dir(profile) {
        Ok(dir) => dir,
        Err(error) => return fail(&mut out, &mut err, &ControlError::new(ErrorCode::Failed, error)),
    };
    let connector = crate::platform::control_connector();
    if let Invocation::ServerStart { profile } = invocation {
        let launch = move || crate::server::launch_detached(profile.as_deref());
        return tauri::async_runtime::block_on(server_start(&data_dir, connector, launch, SERVER_START_WAIT, &mut out, &mut err));
    }
    tauri::async_runtime::block_on(run_with(invocation, &data_dir, connector, read_stdin, &mut out, &mut err))
}

/// The profile's data folder: `--profile`, else the app's default one.
fn data_dir(profile: Option<PathBuf>) -> Result<PathBuf, String> {
    if let Some(profile) = &profile
        && !profile.is_absolute()
    {
        return Err("--profile needs an absolute path".into());
    }
    crate::platform::data_dir(profile)
}

fn read_stdin() -> Result<String, String> {
    let mut text = String::new();
    std::io::stdin().take(MAX_TEXT_BYTES as u64 + 1).read_to_string(&mut text).map_err(|e| e.to_string())?;
    if text.len() > MAX_TEXT_BYTES {
        return Err("the text is longer than 64 KB".into());
    }
    Ok(text)
}

fn fail(out: &mut (dyn Write + Send), err: &mut (dyn Write + Send), error: &ControlError) -> i32 {
    let _ = writeln!(out, "{}", json!({ "ok": false, "error": { "code": error.code.name(), "message": error.message } }));
    let _ = writeln!(err, "mighty: {}", error.message);
    cli::exit_code(error.code)
}

fn print(out: &mut (dyn Write + Send), value: &Value) -> bool {
    let text = serde_json::to_string_pretty(value).unwrap_or_default();
    writeln!(out, "{text}").and_then(|()| out.flush()).is_ok()
}

/// One CLI call against the app whose profile is `data_dir`.
pub async fn run_with(
    invocation: Invocation,
    data_dir: &Path,
    connector: Arc<dyn AgentIoConnector>,
    stdin: impl FnOnce() -> Result<String, String> + Send,
    out: &mut (dyn Write + Send),
    err: &mut (dyn Write + Send),
) -> i32 {
    let (mut command, stdin_text) = match invocation {
        Invocation::Help => {
            let _ = writeln!(out, "{USAGE}");
            return EXIT_OK;
        }
        Invocation::Run { command, stdin_text, .. } => (command, stdin_text),
        Invocation::ServerStart { .. } => {
            return fail(out, err, &ControlError::new(ErrorCode::Failed, "server start runs from the CLI's own entry"));
        }
    };
    match command {
        Command::ServerStatus => return server_status(data_dir, connector, out, err).await,
        Command::ServerStop { stop_all } => {
            let exited = |pid: u32, timeout: Duration| process_exited(pid, timeout);
            return server_stop(data_dir, connector, stop_all, SERVER_STOP_WAIT, &exited, out, err).await;
        }
        _ => {}
    }
    if stdin_text && let Command::AgentSend { text, .. } = &mut command {
        match stdin() {
            Ok(read) if !read.trim().is_empty() => *text = read,
            Ok(_) => return fail(out, err, &ControlError::bad("nothing was read from standard input")),
            Err(error) => return fail(out, err, &ControlError::bad(error)),
        }
    }
    let not_running =
        || ControlError::new(ErrorCode::Failed, format!("Mighty Code is not running for this profile ({})", data_dir.display()));
    let Some(endpoint) = endpoint::read_endpoint(data_dir) else { return fail(out, err, &not_running()) };
    let Some(token) = endpoint::read_token(data_dir) else { return fail(out, err, &not_running()) };
    let connection = match connector.connect(&endpoint.address, CONNECT).await {
        Ok(connection) => connection,
        Err(error) => {
            let message = format!("{} ({error})", not_running().message);
            return fail(out, err, &ControlError::new(ErrorCode::Failed, message));
        }
    };
    let streaming = matches!(command, Command::Events { .. });
    let (mut reader, mut writer) = tokio::io::split(connection);
    let request = Request { id: 1, token, command };
    if super::server::write_line(&mut writer, &request.line()).await.is_err() {
        return fail(out, err, &ControlError::new(ErrorCode::Failed, "the app closed the connection"));
    }
    let mut lines = LineBuffer::new(MAX_ANSWER_BYTES);
    let mut queued = Vec::new();
    let mut first = true;
    loop {
        let Some(line) = super::server::read_line(&mut reader, &mut lines, &mut queued).await else {
            return fail(out, err, &ControlError::new(ErrorCode::Failed, "the app closed the connection"));
        };
        match parse_reply(&line) {
            Ok(Reply::Ok(result)) if first => {
                first = false;
                if !streaming {
                    print(out, &result);
                    return EXIT_OK;
                }
            }
            Ok(Reply::Event(event)) if streaming && !first => {
                if writeln!(out, "{event}").and_then(|()| out.flush()).is_err() {
                    // The reader went (`| head`): done.
                    return EXIT_OK;
                }
            }
            Ok(Reply::Error(error)) => return fail(out, err, &error),
            Ok(_) => return fail(out, err, &ControlError::new(ErrorCode::Failed, "the app's answer was not expected")),
            Err(error) => return fail(out, err, &error),
        }
    }
}

// ── MC-3: the headless server ────────────────────────────────────────────────────────────────────────────────────────

/// How long `server start` waits for the app it started to answer (the state's load, the host's start), and `server stop` for the
/// app to have quit (its teardown waits for terminals, runs and the last save).
pub const SERVER_START_WAIT: Duration = Duration::from_secs(60);
pub const SERVER_STOP_WAIT: Duration = Duration::from_secs(60);
/// How often the waits ask again.
const POLL: Duration = Duration::from_millis(250);

/// One request to the profile's running app and its one answer (not `events.subscribe`). `Err` with `failed` and "not running" when
/// no app answers on the profile's endpoint.
pub async fn request(data_dir: &Path, connector: &dyn AgentIoConnector, command: Command, within: Duration) -> Result<Value, ControlError> {
    let not_running =
        || ControlError::new(ErrorCode::Failed, format!("Mighty Code is not running for this profile ({})", data_dir.display()));
    let endpoint = endpoint::read_endpoint(data_dir).ok_or_else(not_running)?;
    let token = endpoint::read_token(data_dir).ok_or_else(not_running)?;
    let exchange = async {
        let connection = connector.connect(&endpoint.address, within).await.map_err(|_| not_running())?;
        let (mut reader, mut writer) = tokio::io::split(connection);
        let request = Request { id: 1, token, command };
        super::server::write_line(&mut writer, &request.line())
            .await
            .map_err(|_| ControlError::new(ErrorCode::Failed, "the app closed the connection"))?;
        let mut lines = LineBuffer::new(MAX_ANSWER_BYTES);
        let mut queued = Vec::new();
        let line = super::server::read_line(&mut reader, &mut lines, &mut queued)
            .await
            .ok_or_else(|| ControlError::new(ErrorCode::Failed, "the app closed the connection"))?;
        match parse_reply(&line)? {
            Reply::Ok(result) => Ok(result),
            Reply::Error(error) => Err(error),
            Reply::Event(_) => Err(ControlError::new(ErrorCode::Failed, "the app's answer was not expected")),
        }
    };
    tokio::time::timeout(within, exchange).await.unwrap_or_else(|_| Err(ControlError::new(ErrorCode::Timeout, "the app did not answer")))
}

/// Whether `error` says nobody answers (rather than an app that answered with an error).
fn is_not_running(error: &ControlError) -> bool {
    error.code == ErrorCode::Failed && error.message.starts_with("Mighty Code is not running")
}

/// `server status`: the running app's mode, version, pid and uptime; `{"running": false}` and exit 1 when none runs.
async fn server_status(
    data_dir: &Path,
    connector: Arc<dyn AgentIoConnector>,
    out: &mut (dyn Write + Send),
    err: &mut (dyn Write + Send),
) -> i32 {
    match request(data_dir, connector.as_ref(), Command::ServerStatus, CONNECT).await {
        Ok(status) => {
            print(out, &status);
            EXIT_OK
        }
        Err(error) if is_not_running(&error) => {
            print(out, &json!({ "running": false, "dataDir": data_dir }));
            let _ = writeln!(err, "mighty: {}", error.message);
            EXIT_ERROR
        }
        Err(error) => fail(out, err, &error),
    }
}

/// `server stop`: the app asked to quit, then waited for until its endpoint answers no more and its process has ended (`exited`, at
/// most `wait` in all), so a `server start` right after never meets the old app's last save. Nothing running is no error:
/// `{"stopped": false, "running": false}`.
pub async fn server_stop(
    data_dir: &Path,
    connector: Arc<dyn AgentIoConnector>,
    stop_all: bool,
    wait: Duration,
    exited: &(dyn Fn(u32, Duration) -> bool + Send + Sync),
    out: &mut (dyn Write + Send),
    err: &mut (dyn Write + Send),
) -> i32 {
    let asked = match request(data_dir, connector.as_ref(), Command::ServerStop { stop_all }, CONNECT).await {
        Ok(asked) => asked,
        Err(error) if is_not_running(&error) => {
            print(out, &json!({ "stopped": false, "running": false }));
            return EXIT_OK;
        }
        Err(error) => return fail(out, err, &error),
    };
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        match request(data_dir, connector.as_ref(), Command::ServerStatus, CONNECT).await {
            Err(error) if is_not_running(&error) => break,
            _ if tokio::time::Instant::now() >= deadline => {
                return fail(out, err, &ControlError::new(ErrorCode::Timeout, "the app has not quit yet"));
            }
            _ => tokio::time::sleep(POLL).await,
        }
    }
    if let Some(pid) = asked.get("pid").and_then(Value::as_u64).and_then(|pid| u32::try_from(pid).ok()) {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        if !exited(pid, left) {
            return fail(out, err, &ControlError::new(ErrorCode::Timeout, format!("the app (pid {pid}) is still quitting")));
        }
    }
    print(out, &json!({ "stopped": true, "pid": asked.get("pid"), "keptDaemonWork": asked.get("keptDaemonWork") }));
    EXIT_OK
}

/// Whether process `pid` has ended within `timeout` (the platform's wait; a platform that cannot tell says it has).
pub fn process_exited(pid: u32, timeout: Duration) -> bool {
    crate::platform::update_installer().wait_for_exit(pid, timeout).unwrap_or(true)
}

/// `server start`: when an app already runs for the profile its status with `alreadyRunning`; otherwise `launch` starts the app without
/// a window (detached) and its status is printed once its control API answers (at most `wait`; a timeout exits 2).
pub async fn server_start(
    data_dir: &Path,
    connector: Arc<dyn AgentIoConnector>,
    launch: impl FnOnce() -> Result<u32, String> + Send,
    wait: Duration,
    out: &mut (dyn Write + Send),
    err: &mut (dyn Write + Send),
) -> i32 {
    match request(data_dir, connector.as_ref(), Command::ServerStatus, CONNECT).await {
        Ok(mut status) => {
            status["alreadyRunning"] = json!(true);
            print(out, &status);
            return EXIT_OK;
        }
        Err(error) if is_not_running(&error) => {}
        // Something answers on the profile's endpoint but not as this version does (an older app): it is not started over.
        Err(error) => return fail(out, err, &error),
    }
    let pid = match launch() {
        Ok(pid) => pid,
        Err(error) => return fail(out, err, &ControlError::new(ErrorCode::Failed, format!("the app did not start: {error}"))),
    };
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        match request(data_dir, connector.as_ref(), Command::ServerStatus, CONNECT).await {
            Ok(mut status) => {
                status["alreadyRunning"] = json!(false);
                status["started"] = json!(true);
                print(out, &status);
                return EXIT_OK;
            }
            _ if tokio::time::Instant::now() >= deadline => {
                let message = format!("the app (pid {pid}) started but its control API did not answer; see tauri-logs/host.log");
                return fail(out, err, &ControlError::new(ErrorCode::Timeout, message));
            }
            _ => tokio::time::sleep(POLL).await,
        }
    }
}
