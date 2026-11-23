//! MC-9: the `mighty` CLI (`MightyCode --cli …`, which the installed `mighty` script runs for its control commands): reads the
//! profile's endpoint and token, sends one request, prints the answer as JSON (each event of `events` on its own line) and exits 0,
//! 1 on an error, 2 on a timeout. It starts no window, reads no state and writes nothing.

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
        Invocation::Run { profile, .. } => profile.clone(),
        Invocation::Help => None,
    };
    let data_dir = match data_dir(profile) {
        Ok(dir) => dir,
        Err(error) => return fail(&mut out, &mut err, &ControlError::new(ErrorCode::Failed, error)),
    };
    let connector = crate::platform::control_connector();
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
    };
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
