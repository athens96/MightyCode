//! MC-9: the local control API's server, over any listener (the profile's Unix socket on the Mac, an owner-only named pipe on
//! Windows: mighty-platform's `AgentIoListener`, which already refuses other users and remote clients) and any [`Backend`] (the app's
//! host, a fake in tests).
//!
//! Each connection reads newline-delimited JSON requests (each line at most `MAX_LINE_BYTES`; the first within 10 s, later ones
//! within a minute of the last answer) and answers each in turn. A request without the token's exact value is answered
//! `unauthorized` and the connection closed. Waits and the event stream follow the bus: nothing polls. A token made new closes every
//! stream and wait that began under the old one.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use mighty_core::control::events::{EventFilter, PaneState, WaitUntil};
use mighty_core::control::protocol::{
    Command, ControlError, ErrorCode, LineBuffer, MAX_LINE_BYTES, Request, error_line, event_line, ok_line, parse_request, token_matches,
};
use mighty_platform::{AgentIoConnection, AgentIoListener, BoxFuture};
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::sync::{Semaphore, broadcast, watch};

use super::bus::BusEvent;

/// How long a new connection may take to send its first request, and an open one between requests.
const FIRST_REQUEST: Duration = Duration::from_secs(10);
const BETWEEN_REQUESTS: Duration = Duration::from_secs(60);
/// The connections served at once; one more is closed at once.
const MAX_CONNECTIONS: usize = 32;
/// The longest pattern program `pane.waitOutput` compiles.
const REGEX_SIZE_LIMIT: usize = 1 << 20;

/// What the server asks of the app.
pub trait Backend: Send + Sync + 'static {
    fn status(&self) -> Result<Value, ControlError>;
    fn workspaces(&self) -> Result<Value, ControlError>;
    fn panes(&self, workspace: Option<&str>) -> Result<Value, ControlError>;
    fn read(&self, pane: &str, lines: usize) -> BoxFuture<'_, Result<Value, ControlError>>;
    fn send(&self, pane: &str, text: &str) -> BoxFuture<'_, Result<Value, ControlError>>;
    fn create(&self, workspace: &str, agent: &str, worktree: bool) -> BoxFuture<'_, Result<Value, ControlError>>;
    /// The pane's state now; None when there is no such pane.
    fn state(&self, pane: &str) -> Option<PaneState>;
    fn subscribe(&self) -> broadcast::Receiver<BusEvent>;
    /// MC-58: the pane id `pane` names: a bot's `@handle` becomes its pane's id; anything else is the id as given.
    fn resolve(&self, pane: &str) -> Result<String, ControlError> {
        Ok(pane.to_string())
    }
    /// MC-65/66: `bot.list`, `bot.new`, `bot.sleep`, `bot.run` and `event.emit`.
    fn bot_command(&self, _command: Command) -> BoxFuture<'_, Result<Value, ControlError>> {
        Box::pin(async { Err(ControlError::new(ErrorCode::Failed, "this app has no bots")) })
    }
    /// MC-64: why the bot `handle` takes no request now (asleep, its own budget spent); None when it does.
    fn bot_refusal(&self, _handle: &str) -> Option<String> {
        None
    }
    /// A line of output as it may leave the app (secrets redacted).
    fn redact(&self, text: &str) -> String {
        text.to_string()
    }
    /// MC-3: how the app runs (`server.status`).
    fn server_status(&self) -> Result<Value, ControlError> {
        Err(ControlError::new(ErrorCode::Failed, "this app has no server status"))
    }
    /// MC-3: the app quits through its normal quit, after this answer (`server.stop`).
    fn server_stop(&self, _stop_all: bool) -> Result<Value, ControlError> {
        Err(ControlError::new(ErrorCode::Failed, "this app cannot be stopped from here"))
    }
    /// MC-3: the app's window shown, made first when it has none (`app.show`).
    fn show_window(&self) -> Result<Value, ControlError> {
        Err(ControlError::new(ErrorCode::Failed, "this app has no window to show"))
    }
    /// MC-33: the loopback listener another computer reaches over SSH started, and a one-time code for it (`server.sshPair`).
    fn ssh_pair(&self) -> BoxFuture<'_, Result<Value, ControlError>> {
        Box::pin(async { Err(ControlError::new(ErrorCode::Failed, "this app takes no SSH pairing")) })
    }
}

/// The token every request must carry, and its generation (a new token closes what the old one opened).
pub struct Auth {
    token: Mutex<String>,
    generation: watch::Sender<u64>,
}

impl Auth {
    pub fn new(token: String) -> Auth {
        Auth { token: Mutex::new(token), generation: watch::Sender::new(0) }
    }

    pub fn check(&self, given: &str) -> bool {
        token_matches(&self.token.lock().unwrap_or_else(|p| p.into_inner()), given)
    }

    /// The token replaced: every stream and wait under the old one ends.
    pub fn replace(&self, token: String) {
        *self.token.lock().unwrap_or_else(|p| p.into_inner()) = token;
        self.generation.send_modify(|generation| *generation += 1);
    }

    fn generation(&self) -> u64 {
        *self.generation.borrow()
    }
}

/// Accepts connections until the listener closes, each served on its own task.
pub async fn serve(listener: Arc<dyn AgentIoListener>, backend: Arc<dyn Backend>, auth: Arc<Auth>) {
    let slots = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    while let Ok(connection) = listener.accept().await {
        let Ok(slot) = slots.clone().try_acquire_owned() else {
            drop(connection);
            continue;
        };
        let (backend, auth) = (backend.clone(), auth.clone());
        tokio::spawn(async move {
            serve_connection(connection, backend, auth).await;
            drop(slot);
        });
    }
}

struct Connection {
    reader: ReadHalf<AgentIoConnection>,
    writer: WriteHalf<AgentIoConnection>,
    lines: LineBuffer,
    queued: std::collections::VecDeque<Vec<u8>>,
}

/// What reading the next request gave.
enum Next {
    Line(Vec<u8>),
    /// The client went, sent too long a line, or waited too long.
    Closed(Option<ControlError>),
}

impl Connection {
    async fn next(&mut self, within: Duration) -> Next {
        if let Some(line) = self.queued.pop_front() {
            return Next::Line(line);
        }
        let mut buffer = [0u8; 8192];
        loop {
            let read = match tokio::time::timeout(within, self.reader.read(&mut buffer)).await {
                Err(_) | Ok(Err(_)) | Ok(Ok(0)) => return Next::Closed(None),
                Ok(Ok(read)) => read,
            };
            match self.lines.push(&buffer[..read]) {
                Ok(lines) => {
                    self.queued.extend(lines);
                    if let Some(line) = self.queued.pop_front() {
                        return Next::Line(line);
                    }
                }
                Err(error) => return Next::Closed(Some(error)),
            }
        }
    }

    async fn write(&mut self, line: &str) -> bool {
        let mut bytes = Vec::with_capacity(line.len() + 1);
        bytes.extend_from_slice(line.as_bytes());
        bytes.push(b'\n');
        self.writer.write_all(&bytes).await.is_ok() && self.writer.flush().await.is_ok()
    }
}

async fn serve_connection(connection: AgentIoConnection, backend: Arc<dyn Backend>, auth: Arc<Auth>) {
    let (reader, writer) = tokio::io::split(connection);
    let mut connection = Connection { reader, writer, lines: LineBuffer::new(MAX_LINE_BYTES), queued: Default::default() };
    let mut within = FIRST_REQUEST;
    loop {
        let line = match connection.next(within).await {
            Next::Line(line) => line,
            Next::Closed(error) => {
                if let Some(error) = error {
                    let _ = connection.write(&error_line(0, &error)).await;
                }
                return;
            }
        };
        within = BETWEEN_REQUESTS;
        let request = match parse_request(&line) {
            Ok(request) => request,
            Err(error) => {
                let id = serde_json::from_slice::<Value>(&line).ok().and_then(|v| v.get("id").and_then(Value::as_u64)).unwrap_or(0);
                if !connection.write(&error_line(id, &error)).await || error.code == ErrorCode::TooLarge {
                    return;
                }
                continue;
            }
        };
        if !auth.check(&request.token) {
            let _ =
                connection.write(&error_line(request.id, &ControlError::new(ErrorCode::Unauthorized, "the token is not this app's"))).await;
            return;
        }
        if !answer(&mut connection, &*backend, &auth, request).await {
            return;
        }
    }
}

/// The command with a bot's `@handle` replaced by its pane's id (MC-58).
fn resolved(backend: &dyn Backend, command: Command) -> Result<Command, ControlError> {
    let pane = |pane: String| if pane.starts_with('@') { backend.resolve(&pane) } else { Ok(pane) };
    Ok(match command {
        Command::PaneRead { pane: p, lines } => Command::PaneRead { pane: pane(p)?, lines },
        // MC-64: a bot asleep or past its own budget takes nothing sent by its name.
        Command::AgentSend { pane: p, text } => {
            if p.starts_with('@')
                && let Some(words) = backend.bot_refusal(&p)
            {
                return Err(ControlError::new(ErrorCode::Failed, words));
            }
            Command::AgentSend { pane: pane(p)?, text }
        }
        Command::AgentWait { pane: p, until, timeout } => Command::AgentWait { pane: pane(p)?, until, timeout },
        Command::PaneWaitOutput { pane: p, regex, timeout } => Command::PaneWaitOutput { pane: pane(p)?, regex, timeout },
        Command::Events { pane: p, types } => Command::Events { pane: p.map(pane).transpose()?, types },
        other => other,
    })
}

/// Answers one request; false when the connection is done.
async fn answer(connection: &mut Connection, backend: &dyn Backend, auth: &Auth, request: Request) -> bool {
    let id = request.id;
    let generation = auth.generation();
    // MC-58: a bot's `@handle` stands for its pane's id in every pane command.
    let command = match resolved(backend, request.command) {
        Ok(command) => command,
        Err(error) => return connection.write(&error_line(id, &error)).await,
    };
    let result = match command {
        Command::Status => backend.status(),
        Command::WorkspaceList => backend.workspaces(),
        Command::PaneList { workspace } => backend.panes(workspace.as_deref()),
        Command::PaneRead { pane, lines } => backend.read(&pane, lines).await,
        Command::AgentSend { pane, text } => backend.send(&pane, &text).await,
        Command::AgentNew { workspace, agent, worktree } => backend.create(&workspace, &agent, worktree).await,
        Command::AgentWait { pane, until, timeout } => {
            let rx = backend.subscribe();
            watch(connection, auth, generation, wait_state(backend, rx, &pane, until, timeout)).await
        }
        Command::PaneWaitOutput { pane, regex, timeout } => {
            let pattern = regex::RegexBuilder::new(&regex).size_limit(REGEX_SIZE_LIMIT).dfa_size_limit(REGEX_SIZE_LIMIT).build();
            match pattern {
                Err(error) => Err(ControlError::bad(format!("the pattern does not read: {}", last_line(&error.to_string())))),
                Ok(pattern) => {
                    let rx = backend.subscribe();
                    watch(connection, auth, generation, wait_output(backend, rx, &pane, &pattern, timeout)).await
                }
            }
        }
        Command::Events { pane, types } => return stream(connection, backend, auth, generation, id, EventFilter { pane, types }).await,
        Command::ServerStatus => backend.server_status(),
        Command::ServerStop { stop_all } => backend.server_stop(stop_all),
        Command::AppShow => backend.show_window(),
        command @ (Command::BotList
        | Command::BotNew { .. }
        | Command::BotSleep { .. }
        | Command::BotRun { .. }
        | Command::EventEmit { .. }) => backend.bot_command(command).await,
        Command::ServerSshPair => backend.ssh_pair().await,
    };
    let line = match &result {
        Ok(value) => ok_line(id, value.clone()),
        Err(error) => error_line(id, error),
    };
    connection.write(&line).await
}

fn last_line(text: &str) -> String {
    text.lines().last().unwrap_or("").trim().chars().take(200).collect()
}

/// Runs a wait while watching the connection: a client that goes, or a new token, ends it.
async fn watch(
    connection: &mut Connection,
    auth: &Auth,
    generation: u64,
    wait: impl std::future::Future<Output = Result<Value, ControlError>>,
) -> Result<Value, ControlError> {
    tokio::pin!(wait);
    let mut probe = [0u8; 1];
    loop {
        tokio::select! {
            done = &mut wait => return done,
            read = connection.reader.read(&mut probe) => match read {
                // Bytes during a wait are not a request: they are dropped and the wait goes on.
                Ok(n) if n > 0 => continue,
                _ => return Err(ControlError::new(ErrorCode::Failed, "the client went away")),
            },
            () = rotated(auth, generation) => return Err(ControlError::new(ErrorCode::Unauthorized, "the token was made new")),
        }
    }
}

/// Ends once the token is not the one of `generation` any more.
async fn rotated(auth: &Auth, generation: u64) {
    let mut changes = auth.generation.subscribe();
    let _ = changes.wait_for(|now| *now != generation).await;
}

/// The next bus event, or a timeout at `deadline`.
async fn next_event(
    rx: &mut broadcast::Receiver<BusEvent>,
    deadline: Option<tokio::time::Instant>,
) -> Result<Option<BusEvent>, ControlError> {
    let received = match deadline {
        Some(deadline) => match tokio::time::timeout_at(deadline, rx.recv()).await {
            Ok(received) => received,
            Err(_) => return Err(ControlError::new(ErrorCode::Timeout, "timed out")),
        },
        None => rx.recv().await,
    };
    match received {
        Ok(event) => Ok(Some(event)),
        // Fell behind: the caller looks at the state again.
        Err(broadcast::error::RecvError::Lagged(_)) => Ok(None),
        Err(broadcast::error::RecvError::Closed) => Err(ControlError::new(ErrorCode::Failed, "the app is closing")),
    }
}

/// `agent.wait`: answers once the pane reaches `until` (at once when it already has), or `timeout`. Subscribed before the first look,
/// so no change between them is missed.
pub async fn wait_state(
    backend: &dyn Backend,
    mut rx: broadcast::Receiver<BusEvent>,
    pane: &str,
    until: WaitUntil,
    timeout: Option<Duration>,
) -> Result<Value, ControlError> {
    let deadline = timeout.map(|t| tokio::time::Instant::now() + t);
    let reached = |state: PaneState| json!({ "pane": pane, "until": until.name(), "state": state.name() });
    let state = backend.state(pane).ok_or_else(|| ControlError::not_found("pane"))?;
    if until.reached(state) {
        return Ok(reached(state));
    }
    loop {
        match next_event(&mut rx, deadline).await? {
            Some(BusEvent::Control(event)) if event.pane.pane == pane => {
                if let mighty_core::control::events::EventBody::State { to, .. } = event.body
                    && until.reached(to)
                {
                    return Ok(reached(to));
                }
            }
            Some(_) => {}
            None => {
                let state = backend.state(pane).ok_or_else(|| ControlError::not_found("pane"))?;
                if until.reached(state) {
                    return Ok(reached(state));
                }
            }
        }
    }
}

/// `pane.waitOutput`: answers with the first new line of the pane's output the pattern matches (each line on its own), or `timeout`.
pub async fn wait_output(
    backend: &dyn Backend,
    mut rx: broadcast::Receiver<BusEvent>,
    pane: &str,
    pattern: &regex::Regex,
    timeout: Option<Duration>,
) -> Result<Value, ControlError> {
    let deadline = timeout.map(|t| tokio::time::Instant::now() + t);
    backend.state(pane).ok_or_else(|| ControlError::not_found("pane"))?;
    loop {
        if let Some(BusEvent::Output { pane: from, text }) = next_event(&mut rx, deadline).await?
            && from == pane
        {
            for line in text.lines() {
                if let Some(found) = pattern.find(line) {
                    let line = backend.redact(line);
                    let matched = if line.contains(found.as_str()) { found.as_str().to_string() } else { String::new() };
                    return Ok(json!({ "pane": pane, "line": line, "match": matched }));
                }
            }
        }
    }
}

/// `events.subscribe`: `ok`, then each event the filter admits as its own line, until the client goes or the token is made new. A
/// reader that fell behind is told how many events it missed.
async fn stream(connection: &mut Connection, backend: &dyn Backend, auth: &Auth, generation: u64, id: u64, filter: EventFilter) -> bool {
    let mut rx = backend.subscribe();
    if !connection.write(&ok_line(id, json!({ "subscribed": true, "types": filter.types, "pane": filter.pane }))).await {
        return false;
    }
    let mut probe = [0u8; 1];
    loop {
        let line = tokio::select! {
            received = rx.recv() => match received {
                Ok(BusEvent::Control(event)) if filter.admits(&event) => event_line(id, event.to_json()),
                Ok(_) => continue,
                Err(broadcast::error::RecvError::Lagged(missed)) => event_line(id, json!({ "type": "lagged", "missed": missed })),
                Err(broadcast::error::RecvError::Closed) => return false,
            },
            read = connection.reader.read(&mut probe) => match read {
                Ok(n) if n > 0 => continue,
                _ => return false,
            },
            () = rotated(auth, generation) => {
                let _ = connection.write(&error_line(id, &ControlError::new(ErrorCode::Unauthorized, "the token was made new"))).await;
                return false;
            }
        };
        if !connection.write(&line).await {
            return false;
        }
    }
}

/// Reads one line (at most `max` bytes) from a client's connection: the answer side of [`super::client`].
pub async fn read_line<R: AsyncRead + Unpin>(reader: &mut R, lines: &mut LineBuffer, queued: &mut Vec<Vec<u8>>) -> Option<Vec<u8>> {
    if !queued.is_empty() {
        return Some(queued.remove(0));
    }
    let mut buffer = [0u8; 8192];
    loop {
        let read = reader.read(&mut buffer).await.ok()?;
        if read == 0 {
            return None;
        }
        let mut got = lines.push(&buffer[..read]).ok()?;
        if !got.is_empty() {
            let first = got.remove(0);
            queued.extend(got);
            return Some(first);
        }
    }
}

/// Writes one line to a connection.
pub async fn write_line<W: AsyncWrite + Unpin>(writer: &mut W, line: &str) -> std::io::Result<()> {
    writer.write_all(line.as_bytes()).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await
}
