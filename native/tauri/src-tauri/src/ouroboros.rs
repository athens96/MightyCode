//! The Ouroboros dashboard (C# `WinUI\MainWindow.GraphOuroboros.cs` and `Core\OuroborosExecution.cs`'s client, transport and monitor;
//! plan §1.8, W9-E). The pure half is mighty-graph's `ouroboros_execution`.
//!
//! - [`LoopbackTransport`]: GETs against the loopback dashboard only — plain HTTP/1.1, no proxy, no cookies, no redirects — so an answer
//!   can never send the app anywhere else; bounded in time and size.
//! - [`Client`]: the daemon's own record first (`~/.ouroboros/dashboard.json`, read, never written), its two GET endpoints, and the daemon
//!   started without a shell (Ouroboros' tool python with `-I`, a fixed argument array and environment, through `ProcessHost`).
//! - [`Monitor`]: one pane's executions — what each block last showed, when it is read next, the "open dashboard" click.
//! - The host's part: a pane's executions are read only while the page shows its Mighty view in the diagram presentation, never in a
//!   smoke run; the reads run off the view's lock and are given to the blocks when they come back ([`watch`], [`hidden`], [`open`]).
//!
//! Locks: the view, then this module's state, then a pane's monitor; none is held across an `.await`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Utc};
use mighty_core::locale::Locale;
use mighty_graph::ouroboros_execution::{
    self as ox, Board, DaemonStarts, Dashboard, DashboardUrl, Endpoint, Link, Poll, RunSummary, Snapshot, Source,
};
use mighty_platform::{BoxFuture, ProcessHost, SpawnRequest};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::sync::Notify;
use tokio::time::Instant;

use crate::host::{Host, lock};

/// How often the host looks for blocks that are due (C#'s timer waits at least 0.2 s).
const POLL: Duration = Duration::from_millis(200);
/// `ConnectTimeout`.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(4);
/// The most a response's head may be.
const MAXIMUM_HEAD_BYTES: usize = 64 * 1024;

// ── cancellation ──────────────────────────────────────────────────────────────────────────────────────────────────────────────

/// A read's cancellation (C#'s `CancellationToken`): the diagram hid or the pane closed.
#[derive(Debug, Default)]
pub struct Cancel {
    flag: AtomicBool,
    notify: Notify,
}

impl Cancel {
    pub fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }

    pub async fn cancelled(&self) {
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.is_cancelled() {
                return;
            }
            notified.await;
        }
    }
}

/// `future`, or none once `cancel` is cancelled.
async fn or_cancel<T>(cancel: &Cancel, future: impl Future<Output = T>) -> Option<T> {
    tokio::select! {
        biased;
        () = cancel.cancelled() => None,
        value = future => Some(value),
    }
}

// ── transport ─────────────────────────────────────────────────────────────────────────────────────────────────────────────────

/// `IOuroborosDashboardTransport`: GETs against the loopback dashboard; checks inject a fake. `url` is an address as written.
pub trait Transport: Send + Sync {
    /// Status and body, or none when nothing answered in time.
    fn get<'a>(&'a self, url: &'a str, timeout: Duration, cancel: &'a Cancel) -> BoxFuture<'a, Option<(u16, Vec<u8>)>>;
    /// An SSE response read until it has been quiet for `quiet` after its first event, no event came within `first_event`, `deadline`
    /// passed or `maximum` bytes arrived; then closed. Only a 200's body.
    fn read_events<'a>(
        &'a self,
        url: &'a str,
        first_event: Duration,
        quiet: Duration,
        deadline: Duration,
        maximum: usize,
        cancel: &'a Cancel,
    ) -> BoxFuture<'a, Option<Vec<u8>>>;
}

/// `LoopbackDashboardTransport`: HTTP against loopback only.
pub struct LoopbackTransport;

impl Transport for LoopbackTransport {
    fn get<'a>(&'a self, url: &'a str, timeout: Duration, cancel: &'a Cancel) -> BoxFuture<'a, Option<(u16, Vec<u8>)>> {
        Box::pin(read(url, timeout, None, ox::MAXIMUM_BYTES, cancel))
    }

    fn read_events<'a>(
        &'a self,
        url: &'a str,
        first_event: Duration,
        quiet: Duration,
        deadline: Duration,
        maximum: usize,
        cancel: &'a Cancel,
    ) -> BoxFuture<'a, Option<Vec<u8>>> {
        Box::pin(async move {
            read(url, deadline, Some((quiet, first_event)), maximum, cancel)
                .await
                .filter(|(status, _)| *status == 200)
                .map(|(_, body)| body)
        })
    }
}

/// `Read`: a plain GET is its whole body within `timeout` and under `maximum`, or nothing. An event stream (`events` = quiet time and
/// first-event deadline) is what arrived when it went quiet, sent no event in time, ran out its time or filled its room.
async fn read(
    url: &str,
    timeout: Duration,
    events: Option<(Duration, Duration)>,
    maximum: usize,
    cancel: &Cancel,
) -> Option<(u16, Vec<u8>)> {
    // Never anything but plain http on loopback, before any connection is made.
    let target = ox::dashboard_url(url)?;
    or_cancel(cancel, exchange(&target, timeout, events, maximum)).await.flatten().filter(|_| !cancel.is_cancelled())
}

async fn exchange(
    target: &DashboardUrl,
    timeout: Duration,
    events: Option<(Duration, Duration)>,
    maximum: usize,
) -> Option<(u16, Vec<u8>)> {
    let started = Instant::now();
    let limit = started + timeout;
    // An event stream that has not even answered by its first-event deadline has nothing to show yet.
    let opening = events.map_or(limit, |(_, first)| (started + first).min(limit));
    let endpoint = &target.endpoint;
    let host = endpoint.host.trim_start_matches('[').trim_end_matches(']');
    let connect = tokio::net::TcpStream::connect((host, endpoint.port));
    let mut stream = tokio::time::timeout_at(opening.min(started + CONNECT_TIMEOUT), connect).await.ok()?.ok()?;
    let authority = if host.contains(':') { format!("[{host}]:{}", endpoint.port) } else { format!("{host}:{}", endpoint.port) };
    let request = format!("GET {} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n\r\n", target.target());
    tokio::time::timeout_at(opening, stream.write_all(request.as_bytes())).await.ok()?.ok()?;
    let mut buffer = vec![0u8; 16 * 1024];
    let mut head = Vec::new();
    let (status, mut framing, rest) = loop {
        if let Some(end) = head.windows(4).position(|w| w == b"\r\n\r\n") {
            let (status, framing) = parse_head(&head[..end])?;
            break (status, framing, head[end + 4..].to_vec());
        }
        if head.len() > MAXIMUM_HEAD_BYTES {
            return None;
        }
        let count = tokio::time::timeout_at(opening, stream.read(&mut buffer)).await.ok()?.ok()?;
        if count == 0 {
            return None;
        }
        head.extend_from_slice(&buffer[..count]);
    };
    let mut body = Vec::new();
    let mut last_event: Option<Instant> = None;
    // Takes fresh bytes; whether the body ended, or none for a body that cannot be read.
    let mut take = |fresh: &[u8], body: &mut Vec<u8>, last_event: &mut Option<Instant>| -> Option<bool> {
        let before = body.len();
        let done = framing.feed(fresh, body)?;
        // An event ends at a blank line, written with LF or CRLF line ends: one may straddle two reads.
        let overlap = before.min(3);
        if events.is_some() && ends_event(&body[before - overlap..]) {
            *last_event = Some(Instant::now());
        }
        Some(done)
    };
    let mut done = match take(&rest, &mut body, &mut last_event) {
        Some(done) => done,
        None if events.is_some() => true,
        None => return None,
    };
    while !done {
        if body.len() >= maximum {
            events?;
            break;
        }
        let step = match events {
            Some((quiet, first)) => last_event.map_or(started + first, |seen| seen + quiet).min(limit),
            None => limit,
        };
        let count = match tokio::time::timeout_at(step, stream.read(&mut buffer)).await {
            Ok(Ok(count)) => count,
            // The stream went quiet, sent nothing in time or broke off: what arrived is the answer.
            _ if events.is_some() => break,
            _ => return None,
        };
        if count == 0 {
            if matches!(framing, Framing::Close) || events.is_some() {
                break;
            }
            return None;
        }
        done = match take(&buffer[..count], &mut body, &mut last_event) {
            Some(done) => done,
            None if events.is_some() => true,
            None => return None,
        };
    }
    if body.len() >= maximum && events.is_none() {
        return None;
    }
    Some((status, body))
}

fn ends_event(fresh: &[u8]) -> bool {
    fresh.windows(2).any(|w| w == b"\n\n") || fresh.windows(4).any(|w| w == b"\r\n\r\n")
}

/// The status line and how the body is framed.
fn parse_head(head: &[u8]) -> Option<(u16, Framing)> {
    let text = std::str::from_utf8(head).ok()?;
    let mut lines = text.split("\r\n");
    let status_line = lines.next()?;
    let mut parts = status_line.splitn(3, ' ');
    if !parts.next()?.starts_with("HTTP/1.") {
        return None;
    }
    let status: u16 = parts.next()?.parse().ok().filter(|s| (100..=999).contains(s))?;
    let mut framing = Framing::Close;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else { continue };
        let (name, value) = (name.trim(), value.trim());
        if name.eq_ignore_ascii_case("transfer-encoding") && value.to_ascii_lowercase().contains("chunked") {
            framing = Framing::Chunked(Chunked::default());
        } else if name.eq_ignore_ascii_case("content-length") && !matches!(framing, Framing::Chunked(_)) {
            framing = Framing::Length(value.parse().ok()?);
        }
    }
    Some((status, framing))
}

/// How a response's body ends: after its length, at its last chunk, or when the connection closes.
enum Framing {
    Length(usize),
    Chunked(Chunked),
    Close,
}

#[derive(Default)]
struct Chunked {
    state: ChunkState,
    line: Vec<u8>,
    remaining: usize,
}

#[derive(Default, PartialEq, Eq)]
enum ChunkState {
    #[default]
    Size,
    Data,
    DataEnd,
    Trailer,
    Done,
}

impl Framing {
    /// The body bytes in `fresh` added to `body`; whether the body ended, or none when it cannot be read.
    fn feed(&mut self, fresh: &[u8], body: &mut Vec<u8>) -> Option<bool> {
        match self {
            Framing::Close => {
                body.extend_from_slice(fresh);
                Some(false)
            }
            Framing::Length(remaining) => {
                let take = fresh.len().min(*remaining);
                body.extend_from_slice(&fresh[..take]);
                *remaining -= take;
                Some(*remaining == 0)
            }
            Framing::Chunked(chunked) => chunked.feed(fresh, body),
        }
    }
}

impl Chunked {
    fn feed(&mut self, mut fresh: &[u8], body: &mut Vec<u8>) -> Option<bool> {
        while !fresh.is_empty() && self.state != ChunkState::Done {
            match self.state {
                ChunkState::Data => {
                    let take = fresh.len().min(self.remaining);
                    body.extend_from_slice(&fresh[..take]);
                    self.remaining -= take;
                    fresh = &fresh[take..];
                    if self.remaining == 0 {
                        self.state = ChunkState::DataEnd;
                    }
                }
                _ => {
                    let byte = fresh[0];
                    fresh = &fresh[1..];
                    if byte != b'\n' {
                        self.line.push(byte);
                        if self.line.len() > 1_024 {
                            return None;
                        }
                        continue;
                    }
                    let line = std::mem::take(&mut self.line);
                    let line = line.strip_suffix(b"\r").unwrap_or(&line);
                    match self.state {
                        ChunkState::Size => {
                            let size = std::str::from_utf8(line).ok()?.split(';').next()?.trim();
                            let size = usize::from_str_radix(size, 16).ok()?;
                            if size == 0 {
                                self.state = ChunkState::Trailer;
                            } else {
                                self.remaining = size;
                                self.state = ChunkState::Data;
                            }
                        }
                        ChunkState::DataEnd => self.state = ChunkState::Size,
                        ChunkState::Trailer if line.is_empty() => self.state = ChunkState::Done,
                        _ => {}
                    }
                }
            }
        }
        Some(self.state == ChunkState::Done)
    }
}

// ── client ────────────────────────────────────────────────────────────────────────────────────────────────────────────────────

/// `OuroborosDashboardClient.Launch`: one daemon start — a fixed executable, argument array, environment and working directory, and no
/// shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub executable: PathBuf,
    pub arguments: Vec<String>,
    pub environment: BTreeMap<String, String>,
    pub working_directory: PathBuf,
}

/// Starts the daemon; whether the start exited well.
pub type Launcher = Arc<dyn Fn(Launch) -> BoxFuture<'static, bool> + Send + Sync>;

/// `OuroborosRunsAnswer`: what `/api/runs` said. Not answered: the daemon is not up. Answered without summaries: a dashboard that is up
/// and cannot read its runs (a non-200 such as 503, or another body) — starting it again cannot help.
#[derive(Debug, Clone, Default)]
pub struct RunsAnswer {
    pub answered: bool,
    #[allow(dead_code, reason = "C#'s Status, which the checks read: the app acts on answered and summaries alone")]
    pub status: u16,
    pub summaries: Option<Arc<HashMap<String, RunSummary>>>,
}

/// `DaemonDirectory`: the daemon runs from a folder no agent can write — `python -m` would otherwise put the working directory first on
/// `sys.path`, and the detached daemon inherits it.
pub fn daemon_directory() -> PathBuf {
    if cfg!(windows)
        && let Some(root) = std::env::var_os("SystemRoot").filter(|r| !r.is_empty())
    {
        return PathBuf::from(root).join("System32");
    }
    std::env::current_dir().ok().and_then(|d| d.ancestors().last().map(Path::to_path_buf)).unwrap_or_else(|| PathBuf::from("/"))
}

/// `OuroborosDashboardClient`.
pub struct Client {
    transport: Arc<dyn Transport>,
    home: PathBuf,
    launcher: Launcher,
    pub state_path: PathBuf,
    pub python_candidates: Vec<PathBuf>,
}

impl Client {
    pub fn new(transport: Arc<dyn Transport>, home: &Path, launcher: Launcher, python_candidates: Option<Vec<PathBuf>>) -> Client {
        let application_data = std::env::var_os("APPDATA").map(PathBuf::from);
        Client {
            transport,
            home: home.to_path_buf(),
            launcher,
            state_path: home.join(".ouroboros").join("dashboard.json"),
            python_candidates: python_candidates.unwrap_or_else(|| ox::tool_pythons(home, cfg!(windows), application_data.as_deref())),
        }
    }

    /// `EndpointAsync`: the running daemon's own record first; a link's address is only a hint.
    pub fn endpoint(&self, hint: Option<&DashboardUrl>) -> Option<Endpoint> {
        let record = std::fs::metadata(&self.state_path)
            .ok()
            .filter(|m| m.is_file() && m.len() < ox::MAXIMUM_STATE_BYTES)
            .and_then(|_| std::fs::read(&self.state_path).ok())
            .and_then(|bytes| ox::endpoint_from_state(&bytes));
        record.or_else(|| hint.map(|h| h.endpoint.clone()))
    }

    /// `SummariesAsync`.
    pub async fn summaries(&self, endpoint: Option<&Endpoint>, cancel: &Cancel) -> RunsAnswer {
        let Some(endpoint) = endpoint else { return RunsAnswer::default() };
        let url = ox::runs_url(endpoint);
        let Some((status, body)) = self.transport.get(&url.original, ox::RUNS_TIMEOUT, cancel).await else { return RunsAnswer::default() };
        RunsAnswer { answered: true, status, summaries: (status == 200).then(|| ox::summaries(&body)).flatten().map(Arc::new) }
    }

    /// `BoardAsync`: the newest complete board the stream sent before it went quiet.
    pub async fn board(&self, endpoint: &Endpoint, execution_id: &str, cancel: &Cancel) -> Option<Board> {
        let url = ox::events_url(endpoint, execution_id)?;
        let data = self
            .transport
            .read_events(&url.original, ox::FIRST_EVENT_DEADLINE, ox::BOARD_QUIET, ox::BOARD_DEADLINE, ox::MAXIMUM_BOARD_BYTES, cancel)
            .await?;
        ox::sse_data(&String::from_utf8_lossy(&data)).last().and_then(|frame| ox::board(frame.as_bytes()))
    }

    /// `BoardsAsync`: boards for several executions, at most [`ox::BOARD_CONCURRENCY`] streams open at once, so one quiet run does not hold
    /// up the others.
    pub async fn boards(self: &Arc<Self>, endpoint: &Endpoint, ids: &[String], cancel: &Arc<Cancel>) -> HashMap<String, Board> {
        let gate = Arc::new(tokio::sync::Semaphore::new(ox::BOARD_CONCURRENCY));
        let mut seen = HashSet::new();
        let mut reads = tokio::task::JoinSet::new();
        for id in ids.iter().filter(|id| seen.insert(id.as_str())) {
            let (client, endpoint, id, gate, cancel) = (self.clone(), endpoint.clone(), id.clone(), gate.clone(), cancel.clone());
            reads.spawn(async move {
                let _permit = or_cancel(&cancel, gate.acquire_owned()).await?.ok()?;
                client.board(&endpoint, &id, &cancel).await.map(|board| (id, board))
            });
        }
        let mut result = HashMap::new();
        while let Some(read) = reads.join_next().await {
            if let Ok(Some((id, board))) = read {
                result.insert(id, board);
            }
        }
        result
    }

    /// `EnsureDaemonAsync`: Ouroboros' own singleton election brings the daemon up (or reuses it) — no shell, a fixed argument array and
    /// environment, and a timeout. A cancelled wait stops waiting at once; a start already running is left to its own timeout.
    pub async fn ensure_daemon(&self, execution_id: Option<&str>, cancel: &Cancel) -> bool {
        let Some(arguments) = ox::ensure_arguments(execution_id) else { return false };
        let Some(python) = self.python_candidates.iter().find(|p| p.is_file()) else { return false };
        // Windows names are case-blind, but a process can still carry two spellings: the reader takes the last.
        let inherited: HashMap<String, String> =
            std::env::vars_os().map(|(k, v)| (k.to_string_lossy().into_owned(), v.to_string_lossy().into_owned())).collect();
        let launch = Launch {
            executable: python.clone(),
            arguments,
            environment: ox::daemon_environment(&self.home.to_string_lossy(), &inherited, cfg!(windows)),
            working_directory: daemon_directory(),
        };
        let start = tokio::spawn((self.launcher)(launch));
        matches!(or_cancel(cancel, start).await, Some(Ok(true)))
    }
}

/// The daemon's start through the app's process host: nothing is read from it (its pipes are drained, so a daemon that inherits them
/// never fails a write), its environment is exactly the one given, and a start that has not exited in 20 s is ended.
pub fn process_launcher(processes: Arc<dyn ProcessHost>) -> Launcher {
    Arc::new(move |launch: Launch| {
        let processes = processes.clone();
        Box::pin(async move {
            let request = SpawnRequest {
                program: launch.executable,
                args: launch.arguments,
                cwd: launch.working_directory,
                env: launch.environment,
                // Nothing of the app's own environment but what is given back.
                env_remove: std::env::vars_os().map(|(k, _)| k.to_string_lossy().into_owned()).collect(),
                shell_command: None,
            };
            let Ok(mut child) = processes.spawn(request).await else { return false };
            drop(child.stdin.take());
            for reader in [child.stdout.take(), child.stderr.take()].into_iter().flatten() {
                tokio::spawn(drain(reader));
            }
            let control = child.control();
            match tokio::time::timeout(ox::DAEMON_START_TIMEOUT, control.wait()).await {
                Ok(Ok(code)) => code == 0,
                Ok(Err(_)) => false,
                Err(_) => {
                    control.kill();
                    false
                }
            }
        })
    })
}

async fn drain(mut reader: impl AsyncRead + Unpin) {
    let mut sink = [0u8; 4096];
    while let Ok(count) = reader.read(&mut sink).await {
        if count == 0 {
            break;
        }
    }
}

// ── the pane's executions ─────────────────────────────────────────────────────────────────────────────────────────────────────

/// `OuroborosExecutionMonitor`: live state of the background executions a pane started, keyed by [`Link::key`]. Daemon starts are
/// counted process-wide ([`DaemonStarts::shared`]), so a new monitor for the same pane does not start it again sooner.
pub struct Monitor {
    client: Arc<Client>,
    starts: &'static DaemonStarts,
    snapshots: HashMap<String, Snapshot>,
    polls: HashMap<String, Poll>,
    /// Blocks whose "open dashboard" click is still being handled, and those whose last click showed nothing.
    opening: HashSet<String>,
    open_failed: HashSet<String>,
}

/// `OuroborosExecutionMonitor.Read`: one read of the executions that are due, run away from the monitor ([`ReadJob::run`]) and given to
/// it after ([`Monitor::apply`]). The read touches none of the monitor's state.
pub struct ReadJob {
    client: Arc<Client>,
    starts: &'static DaemonStarts,
    due: Vec<Link>,
    lists: HashSet<String>,
    may_ensure: HashSet<String>,
    endpoint: Option<Endpoint>,
    answer: RunsAnswer,
    boards: HashMap<String, Board>,
}

impl ReadJob {
    /// `RunAsync`.
    pub async fn run(&mut self, clock: &(dyn Fn() -> DateTime<Utc> + Send + Sync), cancel: &Arc<Cancel>) {
        let hint = self.due.iter().find_map(|l| l.dashboard_url.clone());
        self.endpoint = self.client.endpoint(hint.as_ref());
        self.answer = self.client.summaries(self.endpoint.as_ref(), cancel).await;
        // Nothing answers: bring the daemon up for a live execution whose start announced a dashboard (so Ouroboros' dashboard is not
        // turned off), a few times per execution at most and never for history. A dashboard that answered and cannot read its runs is up.
        if !self.answer.answered && !cancel.is_cancelled() {
            let mut candidate: Option<String> = None;
            for link in &self.due {
                if let Some(id) = &link.execution_id
                    && link.dashboard_url.is_some()
                    && self.may_ensure.contains(&link.key())
                    && self.starts.claim(id, clock())
                    && candidate.is_none()
                {
                    candidate = Some(id.clone());
                }
            }
            if let Some(candidate) = candidate
                && !cancel.is_cancelled()
                && self.client.ensure_daemon(Some(&candidate), cancel).await
            {
                self.endpoint = self.client.endpoint(hint.as_ref());
                self.answer = self.client.summaries(self.endpoint.as_ref(), cancel).await;
            }
        }
        // Boards for open AC lists, and for runs /api/runs no longer lists (it holds only the newest): their board is all that is left.
        if let (Some(endpoint), Some(listed)) = (&self.endpoint, &self.answer.summaries) {
            let ids: Vec<String> = self
                .due
                .iter()
                .filter_map(|l| l.execution_id.as_ref().filter(|id| self.lists.contains(&l.key()) || !listed.contains_key(*id)))
                .cloned()
                .collect();
            if !ids.is_empty() && !cancel.is_cancelled() {
                self.boards = self.client.boards(endpoint, &ids, cancel).await;
            }
        }
    }
}

impl Monitor {
    pub fn new(client: Arc<Client>, starts: &'static DaemonStarts) -> Monitor {
        Monitor { client, starts, snapshots: HashMap::new(), polls: HashMap::new(), opening: HashSet::new(), open_failed: HashSet::new() }
    }

    #[cfg(test)]
    pub fn snapshot(&self, key: &str) -> Option<&Snapshot> {
        self.snapshots.get(key)
    }

    #[cfg(test)]
    pub fn opening(&self, key: &str) -> bool {
        self.opening.contains(key)
    }

    #[cfg(test)]
    pub fn open_failed(&self, key: &str) -> bool {
        self.open_failed.contains(key)
    }

    /// `Track`: the pane's executions as drawn, and the blocks whose AC list is open (only these read the board stream). A block that is
    /// gone takes its state with it; an AC list opened with nothing read yet reads now, not at the block's next (possibly slow) turn.
    pub fn track(&mut self, links: &[Link], lists: &HashSet<String>, now: DateTime<Utc>) {
        let keys: HashSet<String> = links.iter().map(Link::key).collect();
        self.snapshots.retain(|k, _| keys.contains(k));
        self.polls.retain(|k, _| keys.contains(k));
        self.opening.retain(|k| keys.contains(k));
        self.open_failed.retain(|k| keys.contains(k));
        for link in links {
            let key = link.key();
            match self.polls.get_mut(&key) {
                None => {
                    self.polls.insert(key, Poll::new(now, Poll::is_stale(&link.timestamp, now)));
                }
                Some(poll) => {
                    if lists.contains(&key) && self.snapshots.get(&key).is_none_or(|s| s.items.is_none()) {
                        poll.wake(now);
                    }
                }
            }
        }
    }

    /// `Due`: the executions to read now.
    pub fn due(&self, links: &[Link], now: DateTime<Utc>) -> Vec<Link> {
        links.iter().filter(|l| self.polls.get(&l.key()).is_some_and(|p| p.is_due(now))).cloned().collect()
    }

    /// `NextAt` (the host polls [`Monitor::due`] instead of a timer per pane): when the next one is due; none once every block stopped (terminal, job-only or unanswered history).
    #[cfg(test)]
    pub fn next_at(&self, links: &[Link]) -> Option<DateTime<Utc>> {
        links.iter().filter_map(|l| self.polls.get(&l.key()).and_then(|p| p.next_at)).min()
    }

    /// `Begin`: a read of the executions that are due, with the blocks whose AC list is open.
    pub fn begin(&self, due: Vec<Link>, lists: &HashSet<String>) -> ReadJob {
        let may_ensure =
            due.iter().map(Link::key).filter(|k| self.polls.get(k).is_some_and(Poll::may_ensure_daemon)).collect::<HashSet<_>>();
        ReadJob {
            client: self.client.clone(),
            starts: self.starts,
            due,
            lists: lists.clone(),
            may_ensure,
            endpoint: None,
            answer: RunsAnswer::default(),
            boards: HashMap::new(),
        }
    }

    /// `Apply`: what a finished read saw, given to its blocks. Whether any block's snapshot changed.
    pub fn apply(&mut self, read: &ReadJob, now: DateTime<Utc>) -> bool {
        let mut changed = false;
        for link in &read.due {
            let key = link.key();
            // A block that left the diagram while this read was on its way has nothing to show.
            if !self.polls.contains_key(&key) {
                continue;
            }
            let page = match &read.endpoint {
                None => link.dashboard_url.clone(),
                Some(endpoint) => Some(ox::page_url(endpoint, link.execution_id.as_deref())),
            };
            let previous = self.snapshots.get(&key);
            let snapshot = match (&link.execution_id, &read.answer.summaries) {
                (Some(id), Some(runs)) => Snapshot::merge(runs.get(id), read.boards.get(id), previous.and_then(|p| p.items.as_ref()), page),
                (None, _) => Snapshot { dashboard_url: page, ..Snapshot::new(Source::NoExecution) },
                // Keep what was last seen, but say the status is not known now.
                (Some(_), None) => Snapshot {
                    source: if read.answer.answered { Source::Unreadable } else { Source::Unreachable },
                    status: ox::ExecutionStatus::Unknown,
                    dashboard_url: page,
                    ..previous.cloned().unwrap_or_else(|| Snapshot::new(Source::Unreachable))
                },
            };
            if previous != Some(&snapshot) {
                self.snapshots.insert(key.clone(), snapshot.clone());
                changed = true;
            }
            if let Some(poll) = self.polls.get_mut(&key) {
                poll.record(&snapshot, now);
            }
        }
        changed
    }

    /// `TickAsync` (the host runs [`ReadJob::run`] off the monitor's lock instead): one read of the executions that are due; whether any block's snapshot changed.
    #[cfg(test)]
    pub async fn tick(
        &mut self,
        due: &[Link],
        lists: &HashSet<String>,
        clock: &(dyn Fn() -> DateTime<Utc> + Send + Sync),
        cancel: &Arc<Cancel>,
    ) -> bool {
        let mut read = self.begin(due.to_vec(), lists);
        read.run(clock, cancel).await;
        !cancel.is_cancelled() && self.apply(&read, clock())
    }

    /// What the projection reads of the pane's executions.
    pub fn dashboard(&self, links: Vec<Link>, locale: Locale) -> Dashboard {
        let keys: HashSet<String> = links.iter().map(Link::key).collect();
        Dashboard {
            snapshots: self.snapshots.iter().filter(|(k, _)| keys.contains(*k)).map(|(k, s)| (k.clone(), s.clone())).collect(),
            opening: self.opening.iter().filter(|k| keys.contains(*k)).cloned().collect(),
            open_failed: self.open_failed.iter().filter(|k| keys.contains(*k)).cloned().collect(),
            links,
            locale,
        }
    }
}

/// `OpenDashboardAsync`: the "open dashboard" click. It may start the daemon even for history (the user asked for it), but never for one
/// that answered. One click at a time per block: false when another is still being handled. `began` runs once the click is taken; `open`
/// says whether anything showed.
pub async fn open_dashboard<F, Fut>(monitor: &Mutex<Monitor>, link: &Link, began: impl FnOnce(), open: F) -> bool
where
    F: FnOnce(DashboardUrl) -> Fut,
    Fut: Future<Output = bool>,
{
    let key = link.key();
    let client = {
        let mut held = lock(monitor);
        if !held.opening.insert(key.clone()) {
            return false;
        }
        held.open_failed.remove(&key);
        held.client.clone()
    };
    began();
    let cancel = Cancel::default();
    let mut endpoint = client.endpoint(link.dashboard_url.as_ref());
    if !client.summaries(endpoint.as_ref(), &cancel).await.answered && client.ensure_daemon(link.execution_id.as_deref(), &cancel).await {
        endpoint = client.endpoint(link.dashboard_url.as_ref());
    }
    let url = match &endpoint {
        None => link.dashboard_url.clone(),
        Some(endpoint) => Some(ox::page_url(endpoint, link.execution_id.as_deref())),
    };
    let opened = match url {
        Some(url) => open(url).await,
        None => false,
    };
    let mut held = lock(monitor);
    held.opening.remove(&key);
    if !opened {
        held.open_failed.insert(key);
    }
    true
}

// ── the host ──────────────────────────────────────────────────────────────────────────────────────────────────────────────────

/// One pane's dashboard reads (C#'s `graphOuroboros`, `graphExecutionLinks`, `graphOuroborosReading`, `graphOuroborosCancel`).
struct Pane {
    monitor: Arc<Mutex<Monitor>>,
    links: Vec<Link>,
    lists: HashSet<String>,
    /// The page shows the pane's Mighty view in the diagram presentation: its blocks are read.
    watching: bool,
    reading: bool,
    cancel: Option<Arc<Cancel>>,
}

impl Pane {
    fn new(client: Arc<Client>) -> Pane {
        Pane {
            monitor: Arc::new(Mutex::new(Monitor::new(client, DaemonStarts::shared()))),
            links: vec![],
            lists: HashSet::new(),
            watching: false,
            reading: false,
            cancel: None,
        }
    }

    /// `StopOuroboros`: no more reads, and the one on its way stops.
    fn stop(&mut self) {
        self.watching = false;
        if let Some(cancel) = self.cancel.take() {
            cancel.cancel();
        }
    }
}

struct State {
    client: Arc<Client>,
    panes: HashMap<String, Pane>,
    stopped: bool,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

/// After the run engine (main.rs): the dashboard's reads, never in a smoke run (a block then says its status is being read).
pub fn start(host: &Arc<Host>) {
    if host.smoke.is_some() {
        return;
    }
    let home = PathBuf::from(crate::accounts::home());
    let client = Arc::new(Client::new(Arc::new(LoopbackTransport), &home, process_launcher(host.processes.clone()), None));
    *lock(&STATE) = Some(State { client, panes: HashMap::new(), stopped: false });
    let host = host.clone();
    let _ = std::thread::Builder::new().name("ouroboros-dashboard".into()).spawn(move || {
        loop {
            std::thread::sleep(POLL);
            if host.closing() || lock(&STATE).as_ref().is_none_or(|s| s.stopped) {
                return;
            }
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| due(&host)));
        }
    });
}

/// At quit (main.rs), before the runs are disposed: the reads stopped, the ones on their way cancelled.
pub fn shutdown(_host: &Arc<Host>) {
    if let Some(state) = lock(&STATE).as_mut() {
        state.stopped = true;
        for pane in state.panes.values_mut() {
            pane.stop();
        }
    }
}

/// `WatchOuroboros` from the projection (mighty.rs, with the view locked): the pane's executions as this projection draws them and the
/// blocks whose AC list is open; read only in the diagram presentation. What the projection shows of them.
pub fn watch(host: &Host, pane: &str, links: Vec<Link>, lists: HashSet<String>, diagram: bool) -> Dashboard {
    let mut guard = lock(&STATE);
    let Some(state) = guard.as_mut().filter(|s| !s.stopped && host.smoke.is_none()) else { return Dashboard::new(links, host.locale) };
    if links.is_empty() && !state.panes.contains_key(pane) {
        return Dashboard::new(links, host.locale);
    }
    let client = state.client.clone();
    let entry = state.panes.entry(pane.to_string()).or_insert_with(|| Pane::new(client));
    entry.links = links.clone();
    entry.lists = lists;
    if diagram {
        entry.watching = true;
        lock(&entry.monitor).track(&entry.links, &entry.lists, Utc::now());
    } else {
        entry.stop();
    }
    lock(&entry.monitor).dashboard(links, host.locale)
}

/// `WebMightyShown(false)`: the page no longer shows the pane's Mighty view — no more reads, and the one on its way stops.
pub fn hidden(pane: &str) {
    if let Some(pane) = lock(&STATE).as_mut().and_then(|s| s.panes.get_mut(pane)) {
        pane.stop();
    }
}

/// `ReadOuroboros` for every watched pane whose blocks are due: the reads run off this thread and are given to the blocks when they come
/// back; a pane that closed goes with its reads.
fn due(host: &Arc<Host>) {
    let alive: HashSet<String> = host.view().snapshot.sessions().iter().map(|s| s.id().to_string()).collect();
    let mut guard = lock(&STATE);
    let Some(state) = guard.as_mut().filter(|s| !s.stopped) else { return };
    state.panes.retain(|id, pane| {
        let keep = alive.contains(id);
        if !keep {
            pane.stop();
        }
        keep
    });
    let now = Utc::now();
    for (id, pane) in &mut state.panes {
        if !pane.watching || pane.reading {
            continue;
        }
        let job = {
            let monitor = lock(&pane.monitor);
            let due = monitor.due(&pane.links, now);
            if due.is_empty() {
                continue;
            }
            monitor.begin(due, &pane.lists)
        };
        pane.reading = true;
        let cancel = pane.cancel.get_or_insert_with(Default::default).clone();
        let (host, id) = (host.clone(), id.clone());
        tauri::async_runtime::spawn(async move {
            let mut job = job;
            job.run(&Utc::now, &cancel).await;
            let changed = {
                let mut guard = lock(&STATE);
                let Some(pane) = guard.as_mut().and_then(|s| s.panes.get_mut(&id)) else { return };
                pane.reading = false;
                !cancel.is_cancelled() && lock(&pane.monitor).apply(&job, Utc::now())
            };
            if changed {
                host.view().mighty.dirty(&id);
            }
        });
    }
}

/// `WebOpenDashboard(key)`: the run's page on the loopback dashboard, started first when nothing answers, shown in the system's browser.
/// Never in a smoke run.
pub async fn open(host: &Arc<Host>, pane: &str, key: &str) {
    if host.smoke.is_some() {
        return;
    }
    let (monitor, link) = {
        let mut guard = lock(&STATE);
        let Some(state) = guard.as_mut().filter(|s| !s.stopped) else { return };
        let client = state.client.clone();
        let entry = state.panes.entry(pane.to_string()).or_insert_with(|| Pane::new(client));
        let Some(link) = entry.links.iter().find(|l| l.key() == key).cloned() else { return };
        (entry.monitor.clone(), link)
    };
    let refresh = || host.view().mighty.dirty(pane);
    open_dashboard(&monitor, &link, refresh, |url| async move {
        if ox::dashboard_url(&url.original).is_none() {
            return false;
        }
        let address = url.absolute();
        matches!(tauri::async_runtime::spawn_blocking(move || crate::platform::open_url(&address)).await, Ok(Ok(())))
    })
    .await;
    refresh();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;
    use std::sync::atomic::AtomicUsize;

    fn utf8(text: &str) -> Vec<u8> {
        text.as_bytes().to_vec()
    }

    fn at(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text).unwrap().with_timezone(&Utc)
    }

    /// A home of the test's own (never the user's).
    fn home(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("mighty-ouroboros-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join(".ouroboros")).unwrap();
        root
    }

    fn leak() -> &'static DaemonStarts {
        Box::leak(Box::new(DaemonStarts::default()))
    }

    fn nothing() -> Launcher {
        Arc::new(|_| Box::pin(async { panic!("nothing to start") }))
    }

    fn recording(launches: &Arc<StdMutex<Vec<Launch>>>) -> Launcher {
        let launches = launches.clone();
        Arc::new(move |launch| {
            launches.lock().unwrap().push(launch);
            Box::pin(async { true })
        })
    }

    /// `FakeTransport`: answers `/api/runs` with `runs` (or `answer`'s value) and a board stream with `events`; counts the streams open.
    #[derive(Default)]
    struct FakeTransport {
        runs: Option<Vec<u8>>,
        events: Option<Vec<u8>>,
        status: u16,
        answer: Option<Arc<StdMutex<Option<Vec<u8>>>>>,
        delay: Duration,
        calls: StdMutex<Vec<String>>,
        open: AtomicUsize,
        most_open: AtomicUsize,
    }

    impl FakeTransport {
        fn new(runs: Option<&str>, events: Option<&str>) -> FakeTransport {
            FakeTransport { runs: runs.map(utf8), events: events.map(utf8), status: 200, ..FakeTransport::default() }
        }

        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    fn absolute(url: &str) -> String {
        ox::dashboard_url(url).map(|u| u.absolute()).unwrap_or_else(|| url.to_string())
    }

    impl Transport for FakeTransport {
        fn get<'a>(&'a self, url: &'a str, _timeout: Duration, cancel: &'a Cancel) -> BoxFuture<'a, Option<(u16, Vec<u8>)>> {
            self.calls.lock().unwrap().push(format!("GET {}", absolute(url)));
            Box::pin(async move {
                if !self.delay.is_zero() {
                    or_cancel(cancel, tokio::time::sleep(self.delay)).await?;
                }
                let body = match &self.answer {
                    Some(answer) => answer.lock().unwrap().clone(),
                    None => self.runs.clone(),
                };
                body.map(|b| (self.status, b))
            })
        }

        fn read_events<'a>(
            &'a self,
            url: &'a str,
            first_event: Duration,
            _quiet: Duration,
            deadline: Duration,
            maximum: usize,
            cancel: &'a Cancel,
        ) -> BoxFuture<'a, Option<Vec<u8>>> {
            assert!(
                first_event == ox::FIRST_EVENT_DEADLINE && first_event < deadline && maximum == ox::MAXIMUM_BOARD_BYTES,
                "the board stream's limits"
            );
            self.calls.lock().unwrap().push(format!("SSE {}", absolute(url)));
            Box::pin(async move {
                let open = self.open.fetch_add(1, Ordering::SeqCst) + 1;
                self.most_open.fetch_max(open, Ordering::SeqCst);
                if !self.delay.is_zero() {
                    let _ = or_cancel(cancel, tokio::time::sleep(self.delay)).await;
                }
                self.open.fetch_sub(1, Ordering::SeqCst);
                if let Some(events) = &self.events {
                    return Some(events.clone());
                }
                (!self.delay.is_zero()).then(|| {
                    let run = ox::query_item(url, "run").unwrap_or_default();
                    utf8(&format!("data: {{\"meta\":{{}},\"columns\":{{\"completed\":[{{\"id\":\"{run}\"}}]}}}}\n\n"))
                })
            })
        }
    }

    fn fake_client(transport: FakeTransport, home: &Path, launcher: Launcher) -> (Arc<Client>, Arc<FakeTransport>) {
        let transport = Arc::new(transport);
        (Arc::new(Client::new(transport.clone(), home, launcher, Some(vec![]))), transport)
    }

    fn write_record(home: &Path, json: &str) {
        std::fs::write(home.join(".ouroboros").join("dashboard.json"), json).unwrap();
    }

    fn page(url: &str) -> DashboardUrl {
        ox::dashboard_url(url).unwrap()
    }

    /// `TheClientReadsTheDaemonsOwnRecordAndOnlyGetsTheTwoEndpoints` (L336).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_client_reads_the_daemons_own_record_and_only_gets_the_two_endpoints() {
        let home = home("record");
        let cancel = Arc::new(Cancel::default());
        write_record(&home, r#"{"host":"127.0.0.1","port":4321,"pid":7,"db_path":"/db"}"#);
        let frame = r#"{"meta":{"total":1},"columns":{"completed":[{"id":"ac_1","title":"One"}]}}"#;
        let (client, transport) = fake_client(
            FakeTransport::new(
                Some(r#"{"runs":[{"execution_id":"exec_1","status":"completed"}]}"#),
                Some(&format!("data: {{}}\n\ndata: {frame}\n\n")),
            ),
            &home,
            nothing(),
        );
        // The record wins over a hint from tool output.
        let endpoint = client.endpoint(Some(&page("http://localhost:9999/")));
        assert_eq!(endpoint, Some(Endpoint::new("127.0.0.1", 4321)), "the daemon's own record wins");
        let endpoint = endpoint.unwrap();
        let answer = client.summaries(Some(&endpoint), &cancel).await;
        assert_eq!(
            answer.summaries.as_ref().and_then(|s| s.get("exec_1")).map(|s| s.status),
            Some(ox::ExecutionStatus::Completed),
            "the run's summary"
        );
        let board = client.board(&endpoint, "exec_1", &cancel).await;
        assert!(
            board.is_some_and(|b| b.items.iter().map(|i| i.title.as_str()).eq(["One"]))
                && client.board(&endpoint, "exec/1", &cancel).await.is_none(),
            "the newest complete board, and none for an id that is not strict"
        );
        assert_eq!(
            transport.calls(),
            ["GET http://127.0.0.1:4321/api/runs", "SSE http://127.0.0.1:4321/events?run=exec_1"],
            "only the two endpoints"
        );
        // A record too large to be one is not read.
        write_record(&home, &format!(r#"{{"host":"127.0.0.1","port":4321,"pad":" {}"}}"#, "x".repeat(ox::MAXIMUM_STATE_BYTES as usize)));
        assert!(client.endpoint(None).is_none(), "an oversized record is not read");
        // A dashboard that answers without runs is up but unreadable.
        let answer = |status: u16, body: &str| {
            let (client, _) = fake_client(FakeTransport { status, ..FakeTransport::new(Some(body), None) }, &home, nothing());
            let cancel = cancel.clone();
            async move { client.summaries(Some(&Endpoint::new("127.0.0.1", 4321)), &cancel).await }
        };
        let unavailable = answer(503, r#"{"runs":[],"error":"picker_index_contract_unavailable"}"#).await;
        let html = answer(200, "<html>").await;
        let empty = answer(200, r#"{"runs":[]}"#).await;
        assert!(
            unavailable.answered
                && unavailable.status == 503
                && unavailable.summaries.is_none()
                && html.answered
                && html.summaries.is_none()
                && empty.answered
                && empty.summaries.is_some_and(|s| s.is_empty()),
            "an answer without runs is unreadable, not absent"
        );
        // Boards are read concurrently but bounded.
        let (slow, transport) =
            fake_client(FakeTransport { delay: Duration::from_millis(80), ..FakeTransport::new(None, None) }, &home, nothing());
        let mut ids: Vec<String> = (0..7).map(|n| format!("exec_{n}")).collect();
        ids.push("exec_0".into());
        let boards = slow.boards(&Endpoint::new("127.0.0.1", 1), &ids, &cancel).await;
        let most = transport.most_open.load(Ordering::SeqCst);
        assert!(
            boards.len() == 7
                && boards["exec_3"].items.len() == 1
                && boards["exec_3"].items[0].id == "exec_3"
                && most > 1
                && most <= ox::BOARD_CONCURRENCY
                && transport.calls().len() == 7,
            "seven boards, three streams at once: {most}"
        );
        std::fs::remove_dir_all(home).unwrap();
    }

    /// `WithoutARecordOnlyALoopbackHintIsUsedAndTheDaemonIsEnsuredWithoutAShell` (L337); its environment and interpreter checks are
    /// mighty-graph's tests/ouroboros_execution.rs `the_daemon_is_started_with_only_the_environment_it_needs`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn without_a_record_only_a_loopback_hint_is_used_and_the_daemon_is_ensured_without_a_shell() {
        let home = home("hint");
        let cancel = Cancel::default();
        let launches = Arc::new(StdMutex::new(vec![]));
        let python = home.join("tools").join("ouroboros-ai").join("python");
        let transport = Arc::new(FakeTransport::new(None, None));
        let client = Client::new(transport, &home, recording(&launches), Some(vec![home.join("absent").join("python"), python.clone()]));
        assert!(
            client.endpoint(None).is_none()
                && ox::dashboard_url("http://example.com:80/").is_none()
                && client.endpoint(Some(&page("http://localhost:8123/?run=x"))) == Some(Endpoint::new("localhost", 8123)),
            "only a loopback hint"
        );
        assert!(
            !client.summaries(Some(&Endpoint::new("localhost", 8123)), &cancel).await.answered
                && !client.summaries(None, &cancel).await.answered,
            "nothing answered"
        );
        // No tool python installed: nothing is launched.
        assert!(!client.ensure_daemon(Some("exec_1"), &cancel).await && launches.lock().unwrap().is_empty(), "no python, no start");
        std::fs::create_dir_all(python.parent().unwrap()).unwrap();
        std::fs::write(&python, "").unwrap();
        assert!(
            client.ensure_daemon(Some("exec_1"), &cancel).await
                && !client.ensure_daemon(Some("-x"), &cancel).await
                && launches.lock().unwrap().len() == 1,
            "one start, and none for an id that reads as an option"
        );
        // Isolated python, from a folder an agent cannot write, with only the environment it needs.
        let launch = launches.lock().unwrap()[0].clone();
        assert!(
            launch.executable == python
                && launch.arguments == ["-I", "-m", "ouroboros.dashboard_web", "--run", "exec_1"]
                && launch.working_directory == daemon_directory()
                && launch.environment["HOME"] == home.to_string_lossy()
                && launch.environment["PYTHONSAFEPATH"] == "1"
                && launch.environment["PYTHONNOUSERSITE"] == "1"
                && !launch.environment.contains_key("PYTHONPATH"),
            "the start as launched: {launch:?}"
        );
        std::fs::remove_dir_all(home).unwrap();
    }

    /// A dashboard on loopback answering each request with what `respond` gives: a delay, a status and a body, then closing.
    #[cfg(windows)]
    async fn serve(respond: fn(&str) -> (Duration, u16, String)) -> (u16, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let served = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut request = Vec::new();
                    let mut buffer = [0u8; 4096];
                    while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                        match stream.read(&mut buffer).await {
                            Ok(count) if count > 0 => request.extend_from_slice(&buffer[..count]),
                            _ => return,
                        }
                    }
                    let text = String::from_utf8_lossy(&request).into_owned();
                    let target = text.split(' ').nth(1).unwrap_or("").to_string();
                    let (delay, status, body) = respond(&target);
                    tokio::time::sleep(delay).await;
                    let answer = format!("HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                    let _ = stream.write_all(answer.as_bytes()).await;
                });
            }
        });
        (port, served)
    }

    /// `TheLoopbackTransportReadsOnlyLoopbackWithinItsLimits` (L338). Real sockets: Windows only (CI's Mac runs the rest).
    #[cfg(windows)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_loopback_transport_reads_only_loopback_within_its_limits() {
        let home = home("loopback");
        let cancel = Arc::new(Cancel::default());
        let frame = r#"{"meta":{},"columns":{"executing":[{"id":"ac_1","title":"Doing"}]}}"#;
        let (port, served) = serve(|target| {
            let frame = r#"{"meta":{},"columns":{"executing":[{"id":"ac_1","title":"Doing"}]}}"#;
            if target.starts_with("/api/runs") {
                (Duration::ZERO, 200, r#"{"runs":[{"execution_id":"exec_1","status":"running"}]}"#.into())
            } else if target == "/events?run=exec_1" {
                (Duration::ZERO, 200, format!(": hello\n\ndata: {frame}\n\n"))
            } else if target == "/events?run=exec_slow" {
                (Duration::from_secs(5), 200, "data: {}\n\n".into())
            } else if target == "/large" {
                (Duration::ZERO, 200, "x".repeat(2_000))
            } else {
                (Duration::ZERO, 404, "{}".into())
            }
        })
        .await;
        let _ = frame;
        let transport = LoopbackTransport;
        let client = Client::new(Arc::new(LoopbackTransport), &home, nothing(), Some(vec![]));
        let endpoint = Endpoint::new("127.0.0.1", port);
        let answer = client.summaries(Some(&endpoint), &cancel).await;
        assert_eq!(
            answer.summaries.and_then(|s| s.get("exec_1").map(|r| r.status)),
            Some(ox::ExecutionStatus::Running),
            "runs from a real loopback dashboard"
        );
        assert_eq!(
            client.board(&endpoint, "exec_1", &cancel).await.map(|b| b.items[0].title.clone()).as_deref(),
            Some("Doing"),
            "a closed event stream's board"
        );
        // Not an event stream it asked for: nothing.
        assert!(
            client.board(&endpoint, "exec_2", &cancel).await.is_none()
                && transport.get(&endpoint.url("/missing", None).original, Duration::from_secs(4), &cancel).await.map(|a| a.0) == Some(404),
            "another status is not a board"
        );
        // Never anything but loopback, before any connection is made.
        assert!(
            transport.get("http://example.com:80/api/runs", Duration::from_secs(1), &cancel).await.is_none()
                && transport
                    .read_events(
                        &format!("https://127.0.0.1:{port}/events?run=exec_1"),
                        Duration::from_secs(1),
                        Duration::from_secs(1),
                        Duration::from_secs(2),
                        1024,
                        &cancel
                    )
                    .await
                    .is_none(),
            "never anything but plain http on loopback"
        );
        // An event stream that sends nothing is left after its first-event deadline, not the whole one.
        let started = std::time::Instant::now();
        let slow = endpoint.url("/events", Some("exec_slow")).original;
        let silent = transport
            .read_events(&slow, Duration::from_millis(500), Duration::from_millis(1_200), Duration::from_secs(8), 1024, &cancel)
            .await;
        assert!(silent.is_none() && started.elapsed() < Duration::from_secs(4), "a silent stream is left early: {:?}", started.elapsed());
        // A plain answer is read whole in its time or not at all; a cancelled read ends at once.
        assert!(transport.get(&slow, Duration::from_millis(300), &cancel).await.is_none(), "a slow answer is nothing");
        let cancelled = Arc::new(Cancel::default());
        let stopper = cancelled.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            stopper.cancel();
        });
        let started = std::time::Instant::now();
        assert!(
            transport
                .read_events(&slow, Duration::from_secs(6), Duration::from_millis(1_200), Duration::from_secs(8), 1024, &cancelled)
                .await
                .is_none()
                && started.elapsed() < Duration::from_secs(3),
            "a cancelled read ends at once"
        );
        assert!(
            ox::MAXIMUM_BYTES == 4 * 1024 * 1024
                && transport.get(&endpoint.url("/large", None).original, Duration::from_secs(4), &cancel).await.map(|a| a.1.len())
                    == Some(2_000),
            "a body under the cap is read whole"
        );
        // A stream that stays open and ends its events with CRLF blank lines is read until it goes quiet, not cut at its first-event
        // deadline; this one is chunked, as a streaming server writes it.
        let open = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let open_port = open.local_addr().unwrap().port();
        let writer = tokio::spawn(async move {
            let (mut stream, _) = open.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0u8; 4096];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                let count = stream.read(&mut buffer).await.unwrap();
                request.extend_from_slice(&buffer[..count]);
            }
            let chunk = |text: &str| format!("{:x}\r\n{text}\r\n", text.len());
            let first = "data: {\"meta\":{},\"columns\":{\"pending\":[{\"id\":\"first\"}]}}\r\n\r\n";
            let second = "data: {\"meta\":{},\"columns\":{\"completed\":[{\"id\":\"second\"}]}}\r\n\r\n";
            let head = "HTTP/1.1 200 X\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n";
            stream.write_all(format!("{head}{}", chunk(first)).as_bytes()).await.unwrap();
            tokio::time::sleep(Duration::from_millis(900)).await;
            stream.write_all(chunk(second).as_bytes()).await.unwrap();
            tokio::time::sleep(Duration::from_secs(4)).await;
        });
        let url = Endpoint::new("127.0.0.1", open_port).url("/events", Some("exec_1")).original;
        let events = transport
            .read_events(&url, Duration::from_millis(600), Duration::from_millis(1_500), Duration::from_secs(4), 1024 * 1024, &cancel)
            .await;
        let frames = events.map(|e| ox::sse_data(&String::from_utf8_lossy(&e))).unwrap_or_default();
        assert!(
            frames.len() == 2 && ox::board(frames[1].as_bytes()).is_some_and(|b| b.items.len() == 1 && b.items[0].id == "second"),
            "CRLF events keep an open stream read until it goes quiet: {}",
            frames.len()
        );
        writer.abort();
        served.abort();
        std::fs::remove_dir_all(home).unwrap();
    }

    /// `ThePanesMonitorReadsItsExecutionsAndStartsTheDaemonSparingly` (L339).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_panes_monitor_reads_its_executions_and_starts_the_daemon_sparingly() {
        let home = home("monitor");
        let cancel = Arc::new(Cancel::default());
        write_record(&home, r#"{"host":"127.0.0.1","port":4321}"#);
        let now = at("2026-09-29T01:00:00Z");
        let exec = Link {
            run_id: "run-1".into(),
            tool: "t".into(),
            job_id: Some("job_1".into()),
            session_id: None,
            execution_id: Some("exec_1".into()),
            dashboard_url: Some(page("http://localhost:8123/?run=exec_1")),
            timestamp: "2026-09-29T00:59:00Z".into(),
        };
        let dropped = Link { execution_id: Some("exec_old".into()), job_id: Some("job_0".into()), ..exec.clone() };
        let job_only = Link {
            run_id: "run-1".into(),
            tool: "t".into(),
            job_id: Some("job_2".into()),
            session_id: None,
            execution_id: None,
            dashboard_url: None,
            timestamp: "2026-09-29T00:59:00Z".into(),
        };
        let links = vec![exec.clone(), dropped.clone(), job_only.clone()];
        let frame = r#"{"meta":{"total":1},"columns":{"completed":[{"id":"ac_1","title":"One"}]}}"#;
        let (client, transport) = fake_client(
            FakeTransport::new(
                Some(r#"{"runs":[{"execution_id":"exec_1","status":"running","goal":"Build it","completed_count":1,"total_count":3}]}"#),
                Some(&format!("data: {frame}\n\n")),
            ),
            &home,
            nothing(),
        );
        let none = HashSet::new();
        let mut monitor = Monitor::new(client, leak());
        monitor.track(&links, &none, now);
        assert!(
            monitor.due(&links, now).len() == 3 && monitor.next_at(&links) == Some(now) && monitor.snapshot("exec_1").is_none(),
            "every new block is due at once"
        );
        assert!(monitor.tick(&links, &none, &move || now, &cancel).await, "the first read changes the blocks");
        // The listed run is its summary; the one the list dropped is read from its board; a job has nothing to look up.
        let listed = monitor.snapshot("exec_1").unwrap();
        assert!(
            listed.source == Source::Live
                && listed.status == ox::ExecutionStatus::Running
                && listed.goal.as_deref() == Some("Build it")
                && listed.completed == Some(1)
                && listed.total == Some(3)
                && listed.items.is_none()
                && listed.dashboard_url.as_ref().map(DashboardUrl::absolute).as_deref() == Some("http://127.0.0.1:4321/?run=exec_1"),
            "the listed run: {listed:?}"
        );
        let old = monitor.snapshot("exec_old").unwrap();
        let job = monitor.snapshot("job-job_2").unwrap();
        assert!(
            old.status == ox::ExecutionStatus::Completed
                && old.items.as_ref().map(|i| i.len()) == Some(1)
                && job.source == Source::NoExecution
                && job.dashboard_url.as_ref().map(DashboardUrl::absolute).as_deref() == Some("http://127.0.0.1:4321/"),
            "the dropped run's board and the job-only start"
        );
        assert_eq!(
            transport.calls(),
            ["GET http://127.0.0.1:4321/api/runs", "SSE http://127.0.0.1:4321/events?run=exec_old"],
            "the board is read only for the run the list dropped"
        );
        // The settled and the job-only blocks stop; the running one is read again in three seconds.
        let later = now + chrono::TimeDelta::seconds(3);
        assert!(
            monitor.due(&links, now).is_empty()
                && monitor.next_at(&links) == Some(later)
                && monitor.due(&links, later).iter().map(Link::key).eq(["exec_1".to_string()]),
            "only the running block is read again"
        );
        // Its AC list opened: read now, with its board.
        let open: HashSet<String> = ["exec_1".to_string()].into();
        let second = now + chrono::TimeDelta::seconds(1);
        monitor.track(&links, &open, second);
        assert!(monitor.due(&links, second).iter().map(Link::key).eq(["exec_1".to_string()]), "an opened AC list is due at once");
        assert!(
            monitor.tick(std::slice::from_ref(&exec), &open, &move || second, &cancel).await
                && monitor.snapshot("exec_1").unwrap().items.as_ref().is_some_and(|i| i.len() == 1 && i[0].title == "One"),
            "an opened AC list is read at once"
        );
        let fourth = now + chrono::TimeDelta::seconds(4);
        assert!(!monitor.tick(std::slice::from_ref(&exec), &open, &move || fourth, &cancel).await, "the same answer changes nothing");
        // A block that is gone takes its state with it.
        monitor.track(std::slice::from_ref(&exec), &open, now + chrono::TimeDelta::seconds(5));
        assert!(
            monitor.snapshot("exec_old").is_none() && monitor.snapshot("job-job_2").is_none() && monitor.snapshot("exec_1").is_some(),
            "a block that is gone takes its state with it"
        );

        // Nothing answers: the daemon is brought up once for a live execution that announced a dashboard, never for history.
        std::fs::remove_file(home.join(".ouroboros").join("dashboard.json")).unwrap();
        let launches = Arc::new(StdMutex::new(vec![]));
        let python = home.join("python");
        std::fs::write(&python, "").unwrap();
        let answer: Arc<StdMutex<Option<Vec<u8>>>> = Arc::new(StdMutex::new(None));
        let silent = Arc::new(FakeTransport { answer: Some(answer.clone()), status: 200, ..FakeTransport::default() });
        let history = Link { execution_id: Some("exec_history".into()), timestamp: "2026-09-01T00:00:00Z".into(), ..exec.clone() };
        let quiet_client = Arc::new(Client::new(silent, &home, recording(&launches), Some(vec![python])));
        let mut quiet = Monitor::new(quiet_client, leak());
        let waiting = vec![exec.clone(), history];
        quiet.track(&waiting, &none, now);
        quiet.tick(&waiting, &none, &move || now, &cancel).await;
        let unreachable = quiet.snapshot("exec_1").unwrap();
        assert!(
            launches.lock().unwrap().len() == 1
                && launches.lock().unwrap()[0].arguments.last().map(String::as_str) == Some("exec_1")
                && unreachable.source == Source::Unreachable
                && unreachable.status == ox::ExecutionStatus::Unknown
                && unreachable.dashboard_url.as_ref().map(DashboardUrl::absolute).as_deref() == Some("http://localhost:8123/?run=exec_1"),
            "one start for the live execution; unreachable keeps the link's own page"
        );
        assert!(
            quiet.due(&waiting, now + chrono::TimeDelta::hours(1)).iter().map(Link::key).eq(["exec_1".to_string()]),
            "unanswered history is read once and left"
        );
        let third = now + chrono::TimeDelta::seconds(3);
        quiet.tick(std::slice::from_ref(&exec), &none, &move || third, &cancel).await;
        assert_eq!(launches.lock().unwrap().len(), 1, "not started again within the cooldown");
        // It answers but cannot read its runs: up, never started again, and the last counts stay under an unknown status.
        *answer.lock().unwrap() = Some(utf8("<html>"));
        let tenth = now + chrono::TimeDelta::minutes(10);
        quiet.tick(std::slice::from_ref(&exec), &none, &move || tenth, &cancel).await;
        let unreadable = quiet.snapshot("exec_1").unwrap();
        assert!(
            launches.lock().unwrap().len() == 1
                && unreadable.source == Source::Unreadable
                && unreadable.status == ox::ExecutionStatus::Unknown,
            "an unreadable dashboard is up"
        );

        // The "open dashboard" click: the page of the run, once per click, and a failure is remembered.
        let shared = Arc::new(Mutex::new(monitor));
        let shown = Arc::new(StdMutex::new(vec![]));
        let (watched, seen) = (shared.clone(), shown.clone());
        let taken = open_dashboard(
            &shared,
            &exec,
            || {},
            |url| async move {
                assert!(lock(&watched).opening("exec_1"), "the click is being handled");
                seen.lock().unwrap().push(url.absolute());
                true
            },
        )
        .await;
        assert!(
            taken
                && *shown.lock().unwrap() == ["http://localhost:8123/?run=exec_1"]
                && !lock(&shared).opening("exec_1")
                && !lock(&shared).open_failed("exec_1"),
            "the dashboard page opens: {:?}",
            shown.lock().unwrap()
        );
        open_dashboard(&shared, &exec, || {}, |_| async { false }).await;
        assert!(lock(&shared).open_failed("exec_1"), "a click that showed nothing says so");
        assert!(
            open_dashboard(&shared, &job_only, || {}, |_| async { true }).await
                && lock(&shared).open_failed("job-job_2")
                && lock(&shared).open_failed("exec_1"),
            "with no address there is nothing to open"
        );

        // The read runs away from the monitor and changes no block until it is given to them.
        write_record(&home, r#"{"host":"127.0.0.1","port":4321}"#);
        let one = vec![exec.clone()];
        let (whole_client, _) =
            fake_client(FakeTransport::new(Some(r#"{"runs":[{"execution_id":"exec_1","status":"completed"}]}"#), None), &home, nothing());
        let mut whole = Monitor::new(whole_client, leak());
        whole.track(&one, &none, now);
        let mut read = whole.begin(whole.due(&one, now), &none);
        let job_cancel = cancel.clone();
        let read = tokio::spawn(async move {
            read.run(&move || now, &job_cancel).await;
            read
        })
        .await
        .unwrap();
        assert!(whole.snapshot("exec_1").is_none(), "nothing changes before the read is applied");
        assert!(
            whole.apply(&read, now)
                && whole.snapshot("exec_1").is_some_and(|s| s.status == ox::ExecutionStatus::Completed)
                && whole.next_at(&one).is_none(),
            "a read is given to its blocks only when applied, then settles them"
        );
        // A read cancelled while it waits (the diagram hid, the pane closed) ends at once and leaves its block due.
        let (slow_client, _) = fake_client(
            FakeTransport { delay: Duration::from_secs(5), ..FakeTransport::new(Some(r#"{"runs":[]}"#), None) },
            &home,
            nothing(),
        );
        let mut slow = Monitor::new(slow_client, leak());
        slow.track(&one, &none, now);
        let mut waited = slow.begin(slow.due(&one, now), &none);
        let stop = Arc::new(Cancel::default());
        let stopper = stop.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            stopper.cancel();
        });
        let started = std::time::Instant::now();
        waited.run(&move || now, &stop).await;
        assert!(
            started.elapsed() < Duration::from_secs(3) && slow.snapshot("exec_1").is_none() && slow.due(&one, now).len() == 1,
            "a cancelled read ends at once and leaves its block due: {:?}",
            started.elapsed()
        );
        std::fs::remove_dir_all(home).unwrap();
    }

    /// The block's card as the page reads it (C# `WebExecutionCard`): the words for each state, the AC list while open, the open click.
    #[test]
    fn the_execution_card_says_what_the_dashboard_last_said() {
        let l = Locale::new("en");
        let link = Link {
            run_id: "run-1".into(),
            tool: "t".into(),
            job_id: Some("job_1".into()),
            session_id: None,
            execution_id: Some("exec_1".into()),
            dashboard_url: Some(page("http://localhost:8123/?run=exec_1")),
            timestamp: String::new(),
        };
        let mut dashboard = Dashboard::new(vec![link.clone()], l);
        let waiting = mighty_graph::view::ExecutionDashboard::card(&dashboard, "exec_1", true).unwrap();
        assert!(
            waiting["status"] == "unknown"
                && waiting["note"] == l.get("graph.ouroboros.waiting")
                && waiting["listNote"] == l.get("graph.ouroboros.loadingACs")
                && waiting["counts"].as_array().is_some_and(Vec::is_empty)
                && waiting["dashboard"] == "http://localhost:8123/?run=exec_1"
                && waiting["footer"] == l.get_with("graph.ouroboros.executionID", &[("id", "exec_1")])
                && waiting.get("progress").is_none()
                && waiting.get("done").is_none(),
            "nothing read yet: {waiting}"
        );
        assert_eq!(mighty_graph::view::ExecutionDashboard::block_status(&dashboard, "exec_1"), "starting", "and its block is starting");
        let items = Arc::new(vec![ox::AcItem::new("ac_1", "One", ox::AcStatus::Completed)]);
        dashboard.snapshots.insert(
            "exec_1".into(),
            Snapshot {
                status: ox::ExecutionStatus::Running,
                completed: Some(1),
                total: Some(4),
                pending: Some(3),
                phase: Some("Execute".into()),
                activity: Some("AC 2".into()),
                items: Some(items),
                ..Snapshot::new(Source::Live)
            },
        );
        dashboard.open_failed.insert("exec_1".into());
        let live = dashboard.execution_card("exec_1", &link, true);
        assert!(
            live["status"] == "running"
                && live["progress"] == l.get_with("graph.ouroboros.progress", &[("completed", "1"), ("total", "4")])
                && live["counts"].as_array().is_some_and(|c| c.len() == 4)
                && live["line"] == "Execute · AC 2"
                && live.get("note").is_none()
                && live["acs"][0]["statusWord"] == l.get("graph.ouroboros.ac.completed")
                && live.get("listNote").is_none()
                && live["openFailed"] == true
                && live["footer"] == l.get("graph.ouroboros.openFailed")
                && live["done"] == 0.25,
            "a live read: {live}"
        );
        assert!(dashboard.execution_card("exec_1", &link, false).get("acs").is_none(), "a closed list carries no ACs");
        assert_eq!(mighty_graph::view::ExecutionDashboard::block_status(&dashboard, "exec_1"), "running", "the block's status");
    }
}
