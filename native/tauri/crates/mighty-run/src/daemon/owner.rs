//! MC-3: who owns a run's child and a terminal pane's shell. The app's process and PTY hosts are wrapped once
//! ([`RoutedProcesses`], [`RoutedPtys`]); only an agent run's child (`spawn_keyed`) and a terminal pane's shell (`open_keyed`) are
//! routed, and only while a [`Route`] holds a daemon:
//!
//! - **in-process** (the setting off, or no daemon): the platform's own host starts them, exactly as before;
//! - **daemon**: the daemon starts them; stdin, a resize and a kill go to it, the output streams back. The run engine and the terminal
//!   pane get the same `Child` and `PtySession` they always did, so their parsing and state stay in the app.
//!
//! When the app quits keeping its work, [`Route::detach`] turns every kill and close into letting go: the readers end and the waits
//! answer at once, while the daemon keeps the process and its output. A terminal pane that opens again finds its shell among
//! [`Route::offer`]'s processes and is given it back, its scrollback replayed. When the daemon goes, every remote child and terminal
//! ends (a failed wait), and new ones start in-process.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Context, Poll};
use std::time::Duration;

use mighty_core::daemon::protocol::{Command, ControlError, Kind, MAX_CHUNK_BYTES, ProcInfo, PtySpec, SpawnSpec};
use mighty_core::daemon::ring::Stream;
use mighty_platform::{
    BoxFuture, Child, ChildControl, DetachedChild, DetachedRequest, PlatformError, ProcessHost, PtyControl, PtyHost, PtyRequest,
    PtySession, PtySize, Result, SpawnRequest,
};
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::{mpsc, watch};

use super::client::{DaemonClient, ProcEvent};

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|p| p.into_inner())
}

/// The exit code a child the app let go of reads as (it still runs in the daemon).
pub const DETACHED: i32 = -2;
/// How long a close waits for the daemon to say the process ended.
const CLOSE_WAIT: Duration = Duration::from_secs(6);

fn platform(error: ControlError) -> PlatformError {
    PlatformError::Io(std::io::Error::other(error.message))
}

/// A run's key: its pane, and whether a later app can take it back (only those are kept when the app quits keeping its work).
pub fn run_key(session_id: &str, keep: bool) -> String {
    format!("{}{session_id}", if keep { KEPT_RUN } else { STOPPED_RUN })
}

/// A run's pane and whether it is kept, from its key.
pub fn parse_run_key(key: &str) -> Option<(&str, bool)> {
    key.strip_prefix(KEPT_RUN).map(|pane| (pane, true)).or_else(|| key.strip_prefix(STOPPED_RUN).map(|pane| (pane, false)))
}

const KEPT_RUN: &str = "run:";
const STOPPED_RUN: &str = "run-stop:";
/// The meta a detaching app leaves on a kept run: where in its stream a later app takes it up (just after the last whole line of
/// stdout this app read).
pub const RESUME_AT: &str = "resumeAt";

/// A kept run a later app takes back: the daemon's process and where to read from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Adoption {
    pub proc: String,
    pub pid: u32,
    pub from: u64,
}

/// A remote child the app reads: its key, where its stdout's last whole line ends, and whether its reader has ended.
struct Tracked {
    key: String,
    resume_at: Arc<AtomicU64>,
    ended: watch::Receiver<bool>,
}

/// Where keyed processes go: the daemon while one is connected and the app has not let go, else in-process.
#[derive(Default)]
pub struct Route {
    client: Mutex<Option<Arc<DaemonClient>>>,
    detaching: AtomicBool,
    /// A previous app's terminals the daemon still runs, by pane: given back to the pane that opens again.
    offered: Mutex<HashMap<String, ProcInfo>>,
    /// A previous app's kept runs, by key: attached (not started) by the run that takes each back.
    adoptions: Mutex<HashMap<String, Adoption>>,
    /// The remote children this app reads, by process.
    children: Mutex<HashMap<String, Tracked>>,
}

impl Route {
    pub fn new() -> Arc<Route> {
        Arc::new(Route::default())
    }

    /// The daemon keyed processes go to from now on (None: in-process).
    pub fn set(&self, client: Option<Arc<DaemonClient>>) {
        *lock(&self.client) = client;
        self.detaching.store(false, Ordering::SeqCst);
    }

    /// The daemon, while it is connected, speaks this app's protocol, and the app has not let go of its work.
    pub fn client(&self) -> Option<Arc<DaemonClient>> {
        if self.detaching.load(Ordering::SeqCst) {
            return None;
        }
        lock(&self.client).clone().filter(|client| !client.is_closed() && client.accepted())
    }

    /// Whether a daemon is connected (whatever the app is doing with it).
    pub fn connected(&self) -> bool {
        lock(&self.client).as_ref().is_some_and(|client| !client.is_closed())
    }

    /// The app quits keeping its work: from now on a kill or close of a remote process lets go of it instead. Each kept run is let go
    /// of once its reader has taken what came, and told where a later app takes it up ([`RESUME_AT`]); a run that cannot be taken back
    /// is ended; every terminal is let go of. Bounded: a reader that does not end within two seconds is left as it is.
    pub async fn detach(&self) {
        self.detaching.store(true, Ordering::SeqCst);
        let Some(client) = lock(&self.client).clone() else { return };
        let children: Vec<(String, String, Arc<AtomicU64>, watch::Receiver<bool>)> = lock(&self.children)
            .iter()
            .map(|(proc, tracked)| (proc.clone(), tracked.key.clone(), tracked.resume_at.clone(), tracked.ended.clone()))
            .collect();
        for (proc, key, resume_at, mut ended) in children {
            let keep = parse_run_key(&key).is_some_and(|(_, keep)| keep);
            if !keep {
                client.send(Command::Kill { proc: proc.clone() });
                client.detach(&proc);
                continue;
            }
            client.detach(&proc);
            let _ = tokio::time::timeout(Duration::from_secs(2), ended.wait_for(|ended| *ended)).await;
            let meta = json!({ RESUME_AT: resume_at.load(Ordering::SeqCst) });
            let _ = tokio::time::timeout(Duration::from_secs(2), client.request(Command::Meta { proc, meta })).await;
        }
        client.detach_all();
    }

    /// The kept run of `key` that the next keyed spawn of that key attaches to instead of starting.
    pub fn expect(&self, key: &str, adoption: Adoption) {
        lock(&self.adoptions).insert(key.to_string(), adoption);
    }

    fn take_adoption(&self, key: &str) -> Option<Adoption> {
        lock(&self.adoptions).remove(key)
    }

    fn track(&self, proc: &str, key: &str, resume_at: Arc<AtomicU64>, ended: watch::Receiver<bool>) {
        lock(&self.children).insert(proc.to_string(), Tracked { key: key.to_string(), resume_at, ended });
    }

    fn forget(&self, proc: &str) {
        lock(&self.children).remove(proc);
    }

    pub fn detaching(&self) -> bool {
        self.detaching.load(Ordering::SeqCst)
    }

    /// The running terminals of an earlier app the daemon holds, offered to the panes they belong to.
    pub fn offer(&self, processes: &[ProcInfo]) {
        let mut offered = lock(&self.offered);
        offered.clear();
        for info in processes.iter().filter(|p| p.kind == Kind::Pty && p.running) {
            if let Some(key) = &info.key {
                offered.insert(key.clone(), info.clone());
            }
        }
    }

    /// The offered terminal of `key`, taken (each is given back once).
    fn take_offered(&self, key: &str) -> Option<ProcInfo> {
        lock(&self.offered).remove(key)
    }

    /// The panes whose terminal is still offered (none opened it yet).
    pub fn offered_keys(&self) -> Vec<String> {
        lock(&self.offered).keys().cloned().collect()
    }
}

// ── the wrapped hosts ────────────────────────────────────────────────────────────────────────────────────────────────

/// The app's process host: everything as the platform's, except an agent run's child while the route holds a daemon.
pub struct RoutedProcesses {
    local: Arc<dyn ProcessHost>,
    route: Arc<Route>,
}

impl RoutedProcesses {
    pub fn new(local: Arc<dyn ProcessHost>, route: Arc<Route>) -> RoutedProcesses {
        RoutedProcesses { local, route }
    }
}

impl ProcessHost for RoutedProcesses {
    fn spawn(&self, request: SpawnRequest) -> BoxFuture<'_, Result<Child>> {
        self.local.spawn(request)
    }

    fn spawn_keyed(&self, request: SpawnRequest, key: &str) -> BoxFuture<'_, Result<Child>> {
        match self.route.client() {
            Some(client) => {
                let (route, key) = (self.route.clone(), key.to_string());
                Box::pin(async move {
                    // A run an earlier app kept: attached where that app stopped reading, never started again.
                    if let Some(adoption) = route.take_adoption(&key) {
                        let (events, _) = client.attach(&adoption.proc, adoption.from).await.map_err(platform)?;
                        return Ok(adopt_child(client, route, adoption.proc, adoption.pid, &key, adoption.from, events));
                    }
                    spawn_remote(client, route, request, &key).await
                })
            }
            None => self.local.spawn_keyed(request, key),
        }
    }

    fn start_detached(&self, request: DetachedRequest) -> Result<u32> {
        self.local.start_detached(request)
    }

    fn start_detached_or_in_job(&self, request: DetachedRequest) -> Result<u32> {
        self.local.start_detached_or_in_job(request)
    }

    fn start_detached_waitable(&self, request: DetachedRequest) -> Result<Box<dyn DetachedChild>> {
        self.local.start_detached_waitable(request)
    }

    fn cli_request(&self, program: &Path, args: &[String], cwd: &Path, env: &BTreeMap<String, String>) -> Result<SpawnRequest> {
        self.local.cli_request(program, args, cwd, env)
    }

    fn shell_request(&self, command: &str, cwd: &Path, env: &BTreeMap<String, String>) -> SpawnRequest {
        self.local.shell_request(command, cwd, env)
    }
}

/// The app's PTY host: everything as the platform's, except a terminal pane's shell while the route holds a daemon.
pub struct RoutedPtys {
    local: Arc<dyn PtyHost>,
    route: Arc<Route>,
}

impl RoutedPtys {
    pub fn new(local: Arc<dyn PtyHost>, route: Arc<Route>) -> RoutedPtys {
        RoutedPtys { local, route }
    }
}

impl PtyHost for RoutedPtys {
    fn default_shell(&self) -> PathBuf {
        self.local.default_shell()
    }

    fn shell_arguments(&self, shell: &Path, command: Option<&str>, no_profile: bool) -> Vec<String> {
        self.local.shell_arguments(shell, command, no_profile)
    }

    fn open(&self, request: PtyRequest) -> BoxFuture<'_, Result<PtySession>> {
        self.local.open(request)
    }

    fn open_keyed(&self, request: PtyRequest, key: &str) -> BoxFuture<'_, Result<PtySession>> {
        match self.route.client() {
            Some(client) => {
                let (route, key) = (self.route.clone(), key.to_string());
                Box::pin(async move {
                    // A shell this pane left running when the app last closed is given back, its scrollback replayed.
                    if let Some(info) = route.take_offered(&key)
                        && let Ok(session) = attach_pty(client.clone(), route.clone(), &info.proc, info.pid, request.size).await
                    {
                        return Ok(session);
                    }
                    open_remote(client, route, request, &key).await
                })
            }
            None => self.local.open_keyed(request, key),
        }
    }
}

// ── a run's child in the daemon ──────────────────────────────────────────────────────────────────────────────────────

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// A process's exit as the app reads it: its code, or why there is none.
type ExitWatch = watch::Receiver<Option<std::result::Result<i32, String>>>;

async fn spawn_remote(client: Arc<DaemonClient>, route: Arc<Route>, request: SpawnRequest, key: &str) -> Result<Child> {
    let spec = SpawnSpec {
        program: text(&request.program),
        args: request.args,
        cwd: text(&request.cwd),
        env: request.env,
        env_remove: request.env_remove,
        shell_command: request.shell_command,
    };
    let started = client.request(Command::Spawn { key: Some(key.to_string()), meta: json!({}), spec }).await.map_err(platform)?;
    let proc = started.get("proc").and_then(Value::as_str).unwrap_or_default().to_string();
    let pid = started.get("pid").and_then(Value::as_u64).unwrap_or(0) as u32;
    let (events, _) = client.attach(&proc, 0).await.map_err(platform)?;
    Ok(adopt_child(client, route, proc, pid, key, 0, events))
}

/// A run's child in the daemon as the app's run engine reads it: two output pipes and a control fed by `events` (read from `from`).
fn adopt_child(
    client: Arc<DaemonClient>,
    route: Arc<Route>,
    proc: String,
    pid: u32,
    key: &str,
    from: u64,
    mut events: mpsc::UnboundedReceiver<ProcEvent>,
) -> Child {
    let (out, out_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let (err, err_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let (exit, exit_rx) = watch::channel(None);
    let releasing = (client.clone(), proc.clone());
    let resume_at = Arc::new(AtomicU64::new(from));
    let (ended_tx, ended) = watch::channel(false);
    route.track(&proc, key, resume_at.clone(), ended);
    let forgetting = route.clone();
    tokio::spawn(async move {
        let ended = loop {
            match events.recv().await {
                Some(ProcEvent::Output { stream: Stream::Err, data, .. }) => {
                    let _ = err.send(data);
                }
                Some(ProcEvent::Output { offset, data, .. }) => {
                    if let Some(at) = data.iter().rposition(|b| *b == b'\n') {
                        resume_at.store(offset + at as u64 + 1, Ordering::SeqCst);
                    }
                    let _ = out.send(data);
                }
                Some(ProcEvent::Replayed) => {}
                Some(ProcEvent::Exit { code, error }) => {
                    // Every byte has come: the daemon may forget it.
                    releasing.0.send(Command::Release { proc: releasing.1.clone() });
                    break code.ok_or_else(|| error.unwrap_or_else(|| "the exit code could not be read".into()));
                }
                Some(ProcEvent::Detached) => break Ok(DETACHED),
                Some(ProcEvent::Gone) | None => break Err("the background daemon went away".to_string()),
            }
        };
        drop(out);
        drop(err);
        ended_tx.send_replace(true);
        if !matches!(ended, Ok(DETACHED)) {
            forgetting.forget(&releasing.1);
        }
        exit.send_replace(Some(ended));
    });
    let control = Arc::new(RemoteChild { client: client.clone(), route: route.clone(), proc: proc.clone(), exit: exit_rx });
    Child::new(
        pid,
        Box::new(RemoteStdin { client, route, proc }),
        Box::new(ChannelReader::new(out_rx)),
        Box::new(ChannelReader::new(err_rx)),
        control,
    )
}

struct RemoteChild {
    client: Arc<DaemonClient>,
    route: Arc<Route>,
    proc: String,
    exit: ExitWatch,
}

async fn exited(exit: &ExitWatch) -> Result<i32> {
    let mut exit = exit.clone();
    let ended = exit.wait_for(Option::is_some).await.map_err(|_| PlatformError::Io(std::io::Error::other("the daemon went away")))?;
    match ended.as_ref() {
        Some(Ok(code)) => Ok(*code),
        Some(Err(error)) => Err(PlatformError::Io(std::io::Error::other(error.clone()))),
        None => Err(PlatformError::Io(std::io::Error::other("the daemon went away"))),
    }
}

impl ChildControl for RemoteChild {
    fn kill(&self) {
        if self.route.detaching() {
            self.client.detach(&self.proc);
        } else {
            self.client.send(Command::Kill { proc: self.proc.clone() });
        }
    }

    fn wait(&self) -> BoxFuture<'_, Result<i32>> {
        Box::pin(exited(&self.exit))
    }

    fn close(&self) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            if self.exit.borrow().is_none() {
                self.kill();
            }
            match tokio::time::timeout(CLOSE_WAIT, exited(&self.exit)).await {
                Ok(_) => Ok(()),
                Err(_) => Err(PlatformError::TimedOut("the daemon's process")),
            }
        })
    }
}

/// A run's stdin in the daemon: each write goes as one or more `write` requests, the shutdown as `stdin.close`. Once the app let go
/// of its work (quitting, keeping it), nothing more is written and the stdin stays open: the run engine's own teardown must not end a
/// run the daemon keeps.
struct RemoteStdin {
    client: Arc<DaemonClient>,
    route: Arc<Route>,
    proc: String,
}

impl AsyncWrite for RemoteStdin {
    fn poll_write(self: Pin<&mut Self>, _cx: &mut Context<'_>, buf: &[u8]) -> Poll<std::io::Result<usize>> {
        if self.route.detaching() {
            return Poll::Ready(Ok(buf.len()));
        }
        let take = buf.len().min(MAX_CHUNK_BYTES);
        if self.client.send(Command::Write { proc: self.proc.clone(), data: buf[..take].to_vec() }) {
            Poll::Ready(Ok(take))
        } else {
            Poll::Ready(Err(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "the daemon went away")))
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        if !self.route.detaching() {
            self.client.send(Command::CloseStdin { proc: self.proc.clone() });
        }
        Poll::Ready(Ok(()))
    }
}

/// Bytes from a channel as a reader; it ends when the channel does.
pub struct ChannelReader {
    receiver: mpsc::UnboundedReceiver<Vec<u8>>,
    left: Vec<u8>,
    at: usize,
}

impl ChannelReader {
    pub fn new(receiver: mpsc::UnboundedReceiver<Vec<u8>>) -> ChannelReader {
        ChannelReader { receiver, left: Vec::new(), at: 0 }
    }
}

impl AsyncRead for ChannelReader {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        loop {
            if self.at < self.left.len() {
                let take = (self.left.len() - self.at).min(buf.remaining());
                let at = self.at;
                buf.put_slice(&self.left[at..at + take]);
                self.at += take;
                return Poll::Ready(Ok(()));
            }
            match self.receiver.poll_recv(cx) {
                Poll::Ready(Some(bytes)) => {
                    self.left = bytes;
                    self.at = 0;
                }
                Poll::Ready(None) => return Poll::Ready(Ok(())),
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

// ── a terminal in the daemon ─────────────────────────────────────────────────────────────────────────────────────────

async fn open_remote(client: Arc<DaemonClient>, route: Arc<Route>, request: PtyRequest, key: &str) -> Result<PtySession> {
    let size = request.size.clamped();
    let spec = PtySpec {
        program: text(&request.program),
        args: request.args,
        cwd: text(&request.cwd),
        env: request.env,
        columns: size.columns,
        rows: size.rows,
    };
    let opened = client.request(Command::PtyOpen { key: Some(key.to_string()), meta: json!({}), spec }).await.map_err(platform)?;
    let proc = opened.get("proc").and_then(Value::as_str).unwrap_or_default().to_string();
    let pid = opened.get("pid").and_then(Value::as_u64).unwrap_or(0) as u32;
    attach_pty(client, route, &proc, pid, size).await
}

/// A terminal in the daemon as the app's terminal pane reads it, from its first kept byte (its scrollback, for a terminal an earlier
/// app left running), at `size`.
async fn attach_pty(client: Arc<DaemonClient>, route: Arc<Route>, proc: &str, pid: u32, size: PtySize) -> Result<PtySession> {
    let (mut events, attached) = client.attach(proc, 0).await.map_err(platform)?;
    if !attached.get("running").and_then(Value::as_bool).unwrap_or(false) {
        client.detach(proc);
        return Err(PlatformError::Invalid("the terminal has ended".into()));
    }
    let size = size.clamped();
    client.send(Command::Resize { proc: proc.to_string(), columns: size.columns, rows: size.rows });
    let (output, output_rx) = mpsc::channel::<Vec<u8>>(256);
    let (exit, exit_rx) = watch::channel(None);
    let releasing = (client.clone(), proc.to_string());
    tokio::spawn(async move {
        let ended = loop {
            match events.recv().await {
                Some(ProcEvent::Output { data, .. }) => {
                    if output.send(data).await.is_err() {
                        // Nobody reads it: the pane went. The terminal stays the daemon's until it ends.
                    }
                }
                Some(ProcEvent::Replayed) => {}
                Some(ProcEvent::Exit { code, error }) => {
                    releasing.0.send(Command::Release { proc: releasing.1.clone() });
                    break code.ok_or_else(|| error.unwrap_or_else(|| "the exit code could not be read".into()));
                }
                Some(ProcEvent::Detached) => break Ok(DETACHED),
                Some(ProcEvent::Gone) | None => break Err("the background daemon went away".to_string()),
            }
        };
        drop(output);
        exit.send_replace(Some(ended));
    });
    let control = Arc::new(RemotePty {
        client,
        route,
        proc: proc.to_string(),
        pid,
        exit: exit_rx,
        busy: Arc::new(AtomicU8::new(0)),
        runtime: tokio::runtime::Handle::try_current().ok(),
    });
    Ok(PtySession { id: pid, output: output_rx, control })
}

struct RemotePty {
    client: Arc<DaemonClient>,
    route: Arc<Route>,
    proc: String,
    pid: u32,
    exit: ExitWatch,
    /// The last reading of whether a command runs: 0 not known, 1 idle, 2 busy.
    busy: Arc<AtomicU8>,
    runtime: Option<tokio::runtime::Handle>,
}

impl RemotePty {
    fn ended(&self) -> bool {
        self.exit.borrow().is_some()
    }
}

impl PtyControl for RemotePty {
    fn write(&self, bytes: &[u8]) -> bool {
        if self.ended() || self.route.detaching() || bytes.len() > 4 * MAX_CHUNK_BYTES {
            return false;
        }
        bytes.chunks(MAX_CHUNK_BYTES).all(|chunk| self.client.send(Command::Write { proc: self.proc.clone(), data: chunk.to_vec() }))
    }

    fn resize(&self, size: PtySize) -> Result<()> {
        if self.ended() {
            return Err(PlatformError::Invalid("the terminal has closed".into()));
        }
        let size = size.clamped();
        self.client.send(Command::Resize { proc: self.proc.clone(), columns: size.columns, rows: size.rows });
        Ok(())
    }

    fn process_ids(&self) -> Option<Vec<u32>> {
        (!self.ended()).then(|| vec![self.pid])
    }

    /// The last reading, a fresh one asked for on the way (the window reads every half second, so it trails by one reading).
    fn busy(&self) -> Option<bool> {
        if self.ended() {
            return None;
        }
        if let Some(runtime) = &self.runtime {
            let (client, proc, busy) = (self.client.clone(), self.proc.clone(), self.busy.clone());
            runtime.spawn(async move {
                let read = client.request(Command::Busy { proc }).await.ok().and_then(|v| v.get("busy").and_then(Value::as_bool));
                busy.store(
                    match read {
                        Some(true) => 2,
                        Some(false) => 1,
                        None => 0,
                    },
                    Ordering::SeqCst,
                );
            });
        }
        match self.busy.load(Ordering::SeqCst) {
            2 => Some(true),
            1 => Some(false),
            _ => None,
        }
    }

    fn wait(&self) -> BoxFuture<'_, Result<i32>> {
        Box::pin(exited(&self.exit))
    }

    fn close(&self) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            if !self.ended() {
                if self.route.detaching() {
                    self.client.detach(&self.proc);
                } else {
                    self.client.send(Command::Kill { proc: self.proc.clone() });
                }
            }
            let _ = tokio::time::timeout(CLOSE_WAIT, exited(&self.exit)).await;
            Ok(())
        })
    }
}
