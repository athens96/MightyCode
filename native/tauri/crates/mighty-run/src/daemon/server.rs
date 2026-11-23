//! MC-3: the background daemon's engine. It owns agent runs' children and terminals through the platform's own ports (the same
//! `ProcessHost` and `PtyHost` the app uses, so a child still dies with whoever owns it: the Mac's process groups, which the daemon
//! ends as it exits, and Windows' kill-on-close job objects, which the daemon's handles hold), keeps each one's output in a ring
//! ([`mighty_core::daemon::ring`]) and serves clients over any listener ([`AgentIoListener`]: the profile's 0600 socket on the Mac,
//! an owner-only pipe on Windows, an in-memory one in tests).
//!
//! A connection says `hello` with the token first (within [`Options::hello_within`]); one of another protocol may only ask `status`
//! and `shutdown`. Each connection has one writer: replies, then for each process it attached to, the output after its cursor read
//! from the ring (so a slow client only falls behind, never makes the daemon hold more than the ring), `replayed` once it caught up
//! with what was kept when it attached, and `exit` once it has every byte of an ended process. Nothing polls: output, exits and the
//! idle clock wake what waits on them.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use indexmap::IndexMap;
use mighty_core::daemon::IdleClock;
use mighty_core::daemon::protocol::{
    Command, ControlError, ErrorCode, Event, Kind, LineBuffer, MAX_CHUNK_BYTES, MAX_LINE_BYTES, MAX_PROCESSES, PROTOCOL, PTY_RING_BYTES,
    ProcInfo, PtySpec, RUN_RING_BYTES, SpawnSpec, Welcome, error_line, ok_line, parse_request, token_matches,
};
use mighty_core::daemon::ring::{OutputRing, Stream};
use mighty_platform::process::{ChildReader, ChildWriter};
use mighty_platform::{
    AgentIoConnection, AgentIoListener, ChildControl, ProcessHost, PtyControl, PtyHost, PtyRequest, PtySize, SpawnRequest,
};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::sync::{Notify, Semaphore, mpsc, watch};

/// How the daemon behaves; tests shorten the times and rings.
#[derive(Debug, Clone)]
pub struct Options {
    /// The daemon's version (the app's that started it), told to every client.
    pub app: String,
    pub token: String,
    /// How long it stays with nothing running and nobody attached; None never exits on its own (`--headless`).
    pub idle: Option<Duration>,
    pub run_ring: usize,
    pub pty_ring: usize,
    /// How long a new connection may take to say `hello`.
    pub hello_within: Duration,
    pub max_connections: usize,
}

impl Options {
    pub fn new(app: &str, token: &str) -> Options {
        Options {
            app: app.to_string(),
            token: token.to_string(),
            idle: Some(mighty_core::daemon::IDLE_EXIT),
            run_ring: RUN_RING_BYTES,
            pty_ring: PTY_RING_BYTES,
            hello_within: Duration::from_secs(10),
            max_connections: 16,
        }
    }
}

/// A line for the daemon's log (never a token, an environment value or output).
pub type Log = Arc<dyn Fn(&str) + Send + Sync>;

/// How long an ended child's output readers may take to reach their end before its exit is told anyway.
const DRAIN_AFTER_EXIT: Duration = Duration::from_secs(5);
/// The most bytes one pass of a connection's writer reads per process before replies get their turn.
const WRITE_BATCH: usize = 256 * 1024;
const PASSES_PER_TURN: usize = 16;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|p| p.into_inner())
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn new_proc_id() -> String {
    let mut bytes = [0u8; 12];
    if getrandom::fill(&mut bytes).is_err() {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        return format!("p{:024x}", NEXT.fetch_add(1, Ordering::SeqCst));
    }
    format!("p{}", bytes.iter().map(|b| format!("{b:02x}")).collect::<String>())
}

enum StdinOp {
    Data(Vec<u8>),
    Close,
}

enum Handle {
    Child { control: Arc<dyn ChildControl>, stdin: mpsc::UnboundedSender<StdinOp> },
    Pty(Arc<dyn PtyControl>),
}

struct Output {
    ring: OutputRing,
    exit: Option<(Option<i32>, Option<String>)>,
    /// The connections attached, each woken when there is more to send.
    watchers: HashMap<u64, Arc<Notify>>,
}

struct Proc {
    id: String,
    kind: Kind,
    key: Option<String>,
    meta: Mutex<Value>,
    pid: u32,
    started_at: u64,
    handle: Handle,
    out: Mutex<Output>,
}

impl Proc {
    fn wake(watchers: Vec<Arc<Notify>>) {
        for watcher in watchers {
            watcher.notify_one();
        }
    }

    fn push(&self, stream: Stream, bytes: &[u8]) {
        let watchers = {
            let mut out = lock(&self.out);
            out.ring.push(stream, bytes);
            out.watchers.values().cloned().collect()
        };
        Self::wake(watchers);
    }

    fn exited(&self, code: Option<i32>, error: Option<String>) {
        let watchers = {
            let mut out = lock(&self.out);
            out.exit.get_or_insert((code, error));
            out.watchers.values().cloned().collect()
        };
        Self::wake(watchers);
    }

    fn running(&self) -> bool {
        lock(&self.out).exit.is_none()
    }

    fn info(&self) -> ProcInfo {
        let out = lock(&self.out);
        ProcInfo {
            proc: self.id.clone(),
            kind: self.kind,
            key: self.key.clone(),
            meta: lock(&self.meta).clone(),
            pid: self.pid,
            running: out.exit.is_none(),
            code: out.exit.as_ref().and_then(|(code, _)| *code),
            end: out.ring.end(),
            start: out.ring.start(),
            started_at: self.started_at,
            attached: out.watchers.len(),
        }
    }

    /// Ends the process and everything it started, and waits (bounded, by the platform) until it has.
    async fn end(&self) {
        match &self.handle {
            Handle::Child { control, .. } => {
                let _ = control.close().await;
            }
            Handle::Pty(control) => {
                let _ = control.close().await;
            }
        }
    }
}

/// One connection's place in a process's stream.
struct Sub {
    cursor: u64,
    replaying: bool,
    replay_end: u64,
    exit_sent: bool,
}

#[derive(Default)]
struct State {
    procs: IndexMap<String, Arc<Proc>>,
    clients: usize,
}

/// The daemon: its ports, its processes and its clients.
pub struct Daemon {
    processes: Arc<dyn ProcessHost>,
    ptys: Arc<dyn PtyHost>,
    options: Options,
    state: Mutex<State>,
    /// Woken when a process ends or a client comes or goes (the idle clock looks again).
    changed: Notify,
    stopped: watch::Sender<bool>,
    next_connection: AtomicU64,
    log: Log,
}

fn failed(message: impl Into<String>) -> ControlError {
    ControlError::new(ErrorCode::Failed, message)
}

impl Daemon {
    pub fn new(processes: Arc<dyn ProcessHost>, ptys: Arc<dyn PtyHost>, options: Options, log: Log) -> Arc<Daemon> {
        Arc::new(Daemon {
            processes,
            ptys,
            options,
            state: Mutex::new(State::default()),
            changed: Notify::new(),
            stopped: watch::Sender::new(false),
            next_connection: AtomicU64::new(1),
            log,
        })
    }

    /// Asks the daemon to end: [`Daemon::serve`] ends every process and returns.
    pub fn stop(&self) {
        self.stopped.send_replace(true);
    }

    pub fn running(&self) -> usize {
        lock(&self.state).procs.values().filter(|p| p.running()).count()
    }

    pub fn clients(&self) -> usize {
        lock(&self.state).clients
    }

    fn proc(&self, id: &str) -> Option<Arc<Proc>> {
        lock(&self.state).procs.get(id).cloned()
    }

    fn touched(&self) {
        self.changed.notify_waiters();
    }

    /// Accepts clients until [`Daemon::stop`], the idle clock or a closed listener; then ends every process (bounded) and returns.
    pub async fn serve(self: Arc<Self>, listener: Arc<dyn AgentIoListener>) {
        let idle = self.options.idle.map(|after| tokio::spawn(self.clone().idle_watch(after)));
        let slots = Arc::new(Semaphore::new(self.options.max_connections.max(1)));
        let mut stopped = self.stopped.subscribe();
        let mut connections = Vec::new();
        loop {
            tokio::select! {
                accepted = listener.accept() => match accepted {
                    Ok(connection) => {
                        let Ok(slot) = slots.clone().try_acquire_owned() else {
                            drop(connection);
                            continue;
                        };
                        let daemon = self.clone();
                        let id = self.next_connection.fetch_add(1, Ordering::SeqCst);
                        connections.retain(|task: &tokio::task::JoinHandle<()>| !task.is_finished());
                        connections.push(tokio::spawn(async move {
                            daemon.connection(connection, id).await;
                            drop(slot);
                        }));
                    }
                    Err(_) => break,
                },
                _ = stopped.wait_for(|stop| *stop) => break,
            }
        }
        listener.close();
        self.end_all().await;
        // A `shutdown`'s answer is on its way: the connections get a moment to write it.
        for task in &connections {
            let _ = tokio::time::timeout(Duration::from_millis(200), async {
                while !task.is_finished() {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await;
        }
        for task in connections {
            task.abort();
        }
        if let Some(idle) = idle {
            idle.abort();
        }
        (self.log)("daemon: stopped");
    }

    /// Every process ended, each close on its own task, all awaited.
    async fn end_all(&self) {
        let procs: Vec<Arc<Proc>> = lock(&self.state).procs.values().filter(|p| p.running()).cloned().collect();
        if !procs.is_empty() {
            (self.log)(&format!("daemon: ending {} processes", procs.len()));
        }
        let tasks: Vec<_> = procs.into_iter().map(|proc| tokio::spawn(async move { proc.end().await })).collect();
        for task in tasks {
            let _ = task.await;
        }
    }

    async fn idle_watch(self: Arc<Self>, after: Duration) {
        let mut clock = IdleClock::new(after, Instant::now());
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let (running, clients, held) = {
                let state = lock(&self.state);
                let held = state
                    .procs
                    .values()
                    .any(|p| !p.running() && p.key.as_deref().and_then(super::owner::parse_run_key).is_some_and(|(_, keep)| keep));
                (state.procs.values().filter(|p| p.running()).count(), state.clients, held)
            };
            let now = Instant::now();
            clock.update(running, clients, now);
            clock.hold(held);
            if clock.expired(now) {
                (self.log)("daemon: idle, exiting");
                self.stop();
                return;
            }
            match clock.deadline() {
                Some(deadline) => {
                    tokio::select! {
                        _ = &mut changed => {}
                        _ = tokio::time::sleep_until(deadline.into()) => {}
                    }
                }
                None => changed.await,
            }
        }
    }

    async fn connection(self: Arc<Self>, connection: AgentIoConnection, id: u64) {
        let (mut reader, writer) = tokio::io::split(connection);
        let (replies, replies_rx) = mpsc::unbounded_channel::<String>();
        let wake = Arc::new(Notify::new());
        let subs: Arc<Mutex<HashMap<String, Sub>>> = Arc::default();
        let writing = tokio::spawn(self.clone().write_loop(writer, replies_rx, wake.clone(), subs.clone()));
        let mut lines = LineBuffer::new(MAX_LINE_BYTES);
        let mut queued = std::collections::VecDeque::new();
        // The first line must be `hello` with the token.
        let accepted = match tokio::time::timeout(self.options.hello_within, next_line(&mut reader, &mut lines, &mut queued)).await {
            Ok(Ok(Some(line))) => self.hello(&line, &replies),
            Ok(Err(error)) => {
                let _ = replies.send(error_line(0, &error));
                None
            }
            _ => None,
        };
        let Some(accepted) = accepted else {
            drop(replies);
            let _ = writing.await;
            return;
        };
        lock(&self.state).clients += 1;
        self.touched();
        loop {
            let line = match next_line(&mut reader, &mut lines, &mut queued).await {
                Ok(Some(line)) => line,
                Ok(None) => break,
                Err(error) => {
                    let _ = replies.send(error_line(0, &error));
                    break;
                }
            };
            let (request, command) = match parse_request(&line) {
                Ok(parsed) => parsed,
                Err(error) => {
                    let request = serde_json::from_slice::<Value>(&line).ok().and_then(|v| v.get("id").and_then(Value::as_u64));
                    let _ = replies.send(error_line(request.unwrap_or(0), &error));
                    continue;
                }
            };
            let result = if !accepted && !command.version_free() {
                Err(ControlError::bad("this daemon speaks another protocol: only status and shutdown"))
            } else {
                self.answer(command, id, &wake, &subs).await
            };
            let line = match result {
                Ok(value) => ok_line(request, value),
                Err(error) => error_line(request, &error),
            };
            if replies.send(line).is_err() {
                break;
            }
        }
        // The connection went: what it attached to forgets it.
        let attached: Vec<String> = lock(&subs).drain().map(|(proc, _)| proc).collect();
        for proc in attached {
            if let Some(proc) = self.proc(&proc) {
                lock(&proc.out).watchers.remove(&id);
            }
        }
        {
            let mut state = lock(&self.state);
            state.clients = state.clients.saturating_sub(1);
        }
        self.touched();
        drop(replies);
        let _ = writing.await;
    }

    /// The first line: `hello` with the token (else `unauthorized`, and the connection ends). Whether the client speaks this protocol.
    fn hello(&self, line: &[u8], replies: &mpsc::UnboundedSender<String>) -> Option<bool> {
        let unauthorized = || ControlError::new(ErrorCode::Unauthorized, "the token is not this daemon's");
        match parse_request(line) {
            Ok((request, Command::Hello { protocol, token, .. })) => {
                if !token_matches(&self.options.token, &token) {
                    let _ = replies.send(error_line(request, &unauthorized()));
                    return None;
                }
                let accepted = protocol == PROTOCOL;
                let welcome = Welcome { protocol: PROTOCOL, app: self.options.app.clone(), pid: std::process::id(), accepted };
                let _ = replies.send(ok_line(request, welcome.to_json()));
                Some(accepted)
            }
            Ok((request, _)) => {
                let _ = replies.send(error_line(request, &unauthorized()));
                None
            }
            Err(error) => {
                let _ = replies.send(error_line(0, &error));
                None
            }
        }
    }

    async fn write_loop(
        self: Arc<Self>,
        mut writer: WriteHalf<AgentIoConnection>,
        mut replies: mpsc::UnboundedReceiver<String>,
        wake: Arc<Notify>,
        subs: Arc<Mutex<HashMap<String, Sub>>>,
    ) {
        loop {
            tokio::select! {
                biased;
                line = replies.recv() => match line {
                    Some(line) => {
                        if write_line(&mut writer, &line).await.is_err() {
                            return;
                        }
                    }
                    None => return,
                },
                () = wake.notified() => {
                    if !self.drain(&mut writer, &subs, &wake).await {
                        return;
                    }
                }
            }
        }
    }

    /// Writes what each attached process has after the connection's cursor; false once the client is gone. After a number of passes
    /// it wakes itself and returns, so replies are never stuck behind a busy process.
    async fn drain(&self, writer: &mut WriteHalf<AgentIoConnection>, subs: &Mutex<HashMap<String, Sub>>, wake: &Notify) -> bool {
        for _ in 0..PASSES_PER_TURN {
            let mut lines = Vec::new();
            {
                let mut subs = lock(subs);
                for (proc_id, sub) in subs.iter_mut() {
                    let Some(proc) = self.proc(proc_id) else { continue };
                    let out = lock(&proc.out);
                    let read = out.ring.read_from(sub.cursor, WRITE_BATCH);
                    for chunk in read.chunks {
                        lines.push(
                            Event::Output { proc: proc_id.clone(), offset: chunk.offset, stream: chunk.stream, data: chunk.data }.line(),
                        );
                    }
                    sub.cursor = read.next;
                    if sub.replaying && sub.cursor >= sub.replay_end {
                        sub.replaying = false;
                        lines.push(Event::Replayed { proc: proc_id.clone(), offset: sub.cursor }.line());
                    }
                    if let Some((code, error)) = &out.exit
                        && !sub.exit_sent
                        && sub.cursor >= out.ring.end()
                    {
                        sub.exit_sent = true;
                        lines.push(Event::Exit { proc: proc_id.clone(), code: *code, error: error.clone() }.line());
                    }
                }
            }
            if lines.is_empty() {
                return true;
            }
            for line in lines {
                if write_line(writer, &line).await.is_err() {
                    return false;
                }
            }
        }
        wake.notify_one();
        true
    }

    async fn answer(
        self: &Arc<Self>,
        command: Command,
        connection: u64,
        wake: &Arc<Notify>,
        subs: &Mutex<HashMap<String, Sub>>,
    ) -> Result<Value, ControlError> {
        match command {
            Command::Hello { .. } => Err(ControlError::bad("hello was already said")),
            Command::Spawn { key, meta, spec } => self.spawn(key, meta, spec).await,
            Command::PtyOpen { key, meta, spec } => self.open_pty(key, meta, spec).await,
            Command::Attach { proc, from } => {
                let found = self.proc(&proc).ok_or_else(|| ControlError::not_found("process"))?;
                let mut subs = lock(subs);
                let mut out = lock(&found.out);
                let end = out.ring.end();
                subs.insert(proc.clone(), Sub { cursor: from, replaying: true, replay_end: end, exit_sent: false });
                out.watchers.insert(connection, wake.clone());
                wake.notify_one();
                Ok(json!({ "proc": proc, "end": end, "start": out.ring.start(), "running": out.exit.is_none() }))
            }
            Command::Detach { proc } => {
                lock(subs).remove(&proc);
                if let Some(found) = self.proc(&proc) {
                    lock(&found.out).watchers.remove(&connection);
                }
                Ok(json!({}))
            }
            Command::Write { proc, data } => {
                let found = self.proc(&proc).ok_or_else(|| ControlError::not_found("process"))?;
                let written = match &found.handle {
                    Handle::Child { stdin, .. } => stdin.send(StdinOp::Data(data)).is_ok(),
                    Handle::Pty(control) => control.write(&data),
                };
                Ok(json!({ "written": written }))
            }
            Command::CloseStdin { proc } => {
                let found = self.proc(&proc).ok_or_else(|| ControlError::not_found("process"))?;
                if let Handle::Child { stdin, .. } = &found.handle {
                    let _ = stdin.send(StdinOp::Close);
                }
                Ok(json!({}))
            }
            Command::Resize { proc, columns, rows } => {
                let found = self.proc(&proc).ok_or_else(|| ControlError::not_found("process"))?;
                match &found.handle {
                    Handle::Pty(control) => control.resize(PtySize { columns, rows }).map_err(|e| failed(e.to_string()))?,
                    Handle::Child { .. } => return Err(ControlError::bad("not a terminal")),
                }
                Ok(json!({}))
            }
            Command::Kill { proc } => {
                let found = self.proc(&proc).ok_or_else(|| ControlError::not_found("process"))?;
                match &found.handle {
                    Handle::Child { control, .. } => control.kill(),
                    Handle::Pty(control) => {
                        let control = control.clone();
                        tokio::spawn(async move {
                            let _ = control.close().await;
                        });
                    }
                }
                Ok(json!({}))
            }
            Command::Busy { proc } => {
                let found = self.proc(&proc).ok_or_else(|| ControlError::not_found("process"))?;
                let busy = match &found.handle {
                    Handle::Pty(control) if found.running() => control.busy(),
                    _ => None,
                };
                Ok(json!({ "busy": busy }))
            }
            Command::Release { proc } => {
                let mut state = lock(&self.state);
                if state.procs.get(&proc).is_some_and(|p| p.running()) {
                    return Err(ControlError::bad("the process still runs"));
                }
                state.procs.shift_remove(&proc);
                Ok(json!({}))
            }
            Command::Meta { proc, meta } => {
                let found = self.proc(&proc).ok_or_else(|| ControlError::not_found("process"))?;
                let mut held = lock(&found.meta);
                if !held.is_object() {
                    *held = json!({});
                }
                if let (Some(held), Some(given)) = (held.as_object_mut(), meta.as_object()) {
                    for (name, value) in given {
                        held.insert(name.clone(), value.clone());
                    }
                }
                if held.to_string().len() > mighty_core::daemon::protocol::MAX_META_BYTES {
                    *held = json!({});
                    return Err(ControlError::new(ErrorCode::TooLarge, "'meta' is too large"));
                }
                Ok(json!({}))
            }
            Command::List => {
                let procs: Vec<Arc<Proc>> = lock(&self.state).procs.values().cloned().collect();
                Ok(json!({ "processes": procs.iter().map(|p| p.info().to_json()).collect::<Vec<_>>() }))
            }
            Command::Status => Ok(self.status()),
            Command::Shutdown => {
                (self.log)("daemon: shutdown asked");
                self.stop();
                Ok(json!({ "stopping": true }))
            }
        }
    }

    /// `status`: the daemon's protocol, version and process, what it holds and the output it keeps.
    pub fn status(&self) -> Value {
        let state = lock(&self.state);
        let running = state.procs.values().filter(|p| p.running()).count();
        let buffered: usize = state.procs.values().map(|p| lock(&p.out).ring.len()).sum();
        json!({
            "protocol": PROTOCOL,
            "app": self.options.app,
            "pid": std::process::id(),
            "running": running,
            "processes": state.procs.len(),
            "clients": state.clients,
            "bufferedBytes": buffered,
        })
    }

    /// Room for one more process: the oldest ended one nobody attached to goes when the daemon is full.
    fn make_room(&self) -> Result<(), ControlError> {
        let mut state = lock(&self.state);
        if state.procs.len() < MAX_PROCESSES {
            return Ok(());
        }
        let oldest = state.procs.iter().find(|(_, p)| !p.running() && lock(&p.out).watchers.is_empty()).map(|(id, _)| id.clone());
        match oldest {
            Some(id) => {
                state.procs.shift_remove(&id);
                Ok(())
            }
            None => Err(ControlError::new(ErrorCode::TooLarge, "the daemon holds too many processes")),
        }
    }

    fn insert(&self, proc: Arc<Proc>) {
        lock(&self.state).procs.insert(proc.id.clone(), proc);
        self.touched();
    }

    async fn spawn(self: &Arc<Self>, key: Option<String>, meta: Value, spec: SpawnSpec) -> Result<Value, ControlError> {
        self.make_room()?;
        let request = SpawnRequest {
            program: PathBuf::from(spec.program),
            args: spec.args,
            cwd: PathBuf::from(spec.cwd),
            env: spec.env,
            env_remove: spec.env_remove,
            shell_command: spec.shell_command,
            restricted: None,
        };
        let mut child = self.processes.spawn(request).await.map_err(|e| failed(e.to_string()))?;
        let (stdin, stdin_rx) = mpsc::unbounded_channel();
        let control = child.control();
        let proc = Arc::new(Proc {
            id: new_proc_id(),
            kind: Kind::Run,
            key,
            meta: Mutex::new(meta),
            pid: child.id,
            started_at: now_ms(),
            handle: Handle::Child { control: control.clone(), stdin },
            out: Mutex::new(Output { ring: OutputRing::new(self.options.run_ring, MAX_CHUNK_BYTES), exit: None, watchers: HashMap::new() }),
        });
        self.insert(proc.clone());
        tokio::spawn(feed_stdin(child.stdin.take(), stdin_rx));
        let (done, done_rx) = watch::channel(0usize);
        let done = Arc::new(done);
        for (reader, stream) in [(child.stdout.take(), Stream::Out), (child.stderr.take(), Stream::Err)] {
            let (proc, done) = (proc.clone(), done.clone());
            tokio::spawn(async move {
                read_into(reader, &proc, stream).await;
                done.send_modify(|n| *n += 1);
            });
        }
        let (daemon, waited) = (self.clone(), proc.clone());
        tokio::spawn(async move {
            let code = control.wait().await;
            let mut done_rx = done_rx;
            let _ = tokio::time::timeout(DRAIN_AFTER_EXIT, done_rx.wait_for(|n| *n >= 2)).await;
            match code {
                Ok(code) => waited.exited(Some(code), None),
                Err(error) => waited.exited(None, Some(error.to_string())),
            }
            daemon.touched();
        });
        (self.log)(&format!("daemon: run {} started (pid {})", proc.id, proc.pid));
        Ok(json!({ "proc": proc.id, "pid": proc.pid }))
    }

    async fn open_pty(self: &Arc<Self>, key: Option<String>, meta: Value, spec: PtySpec) -> Result<Value, ControlError> {
        self.make_room()?;
        let request = PtyRequest {
            program: PathBuf::from(spec.program),
            args: spec.args,
            cwd: PathBuf::from(spec.cwd),
            env: spec.env,
            size: PtySize { columns: spec.columns, rows: spec.rows },
        };
        let session = self.ptys.open(request).await.map_err(|e| failed(e.to_string()))?;
        let control = session.control.clone();
        let proc = Arc::new(Proc {
            id: new_proc_id(),
            kind: Kind::Pty,
            key,
            meta: Mutex::new(meta),
            pid: session.id,
            started_at: now_ms(),
            handle: Handle::Pty(control.clone()),
            out: Mutex::new(Output { ring: OutputRing::new(self.options.pty_ring, MAX_CHUNK_BYTES), exit: None, watchers: HashMap::new() }),
        });
        self.insert(proc.clone());
        let (drained, drained_rx) = watch::channel(false);
        let reading = proc.clone();
        let mut output = session.output;
        tokio::spawn(async move {
            while let Some(bytes) = output.recv().await {
                reading.push(Stream::Pty, &bytes);
            }
            drained.send_replace(true);
        });
        let (daemon, waited) = (self.clone(), proc.clone());
        tokio::spawn(async move {
            let code = control.wait().await;
            let closed = control.close().await;
            let mut drained_rx = drained_rx;
            let _ = tokio::time::timeout(DRAIN_AFTER_EXIT, drained_rx.wait_for(|done| *done)).await;
            match (code, closed) {
                (Ok(code), _) => waited.exited(Some(code), None),
                (Err(error), _) => waited.exited(None, Some(error.to_string())),
            }
            daemon.touched();
        });
        (self.log)(&format!("daemon: terminal {} opened (pid {})", proc.id, proc.pid));
        Ok(json!({ "proc": proc.id, "pid": proc.pid }))
    }
}

async fn read_into(reader: Option<ChildReader>, proc: &Proc, stream: Stream) {
    let Some(mut reader) = reader else { return };
    let mut buffer = vec![0u8; MAX_CHUNK_BYTES];
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) | Err(_) => return,
            Ok(read) => proc.push(stream, &buffer[..read]),
        }
    }
}

async fn feed_stdin(stdin: Option<ChildWriter>, mut ops: mpsc::UnboundedReceiver<StdinOp>) {
    let Some(mut stdin) = stdin else { return };
    while let Some(op) = ops.recv().await {
        match op {
            StdinOp::Data(data) => {
                if stdin.write_all(&data).await.is_err() || stdin.flush().await.is_err() {
                    return;
                }
            }
            StdinOp::Close => {
                let _ = stdin.shutdown().await;
                return;
            }
        }
    }
}

/// The next line from a connection: None when the client went.
async fn next_line(
    reader: &mut ReadHalf<AgentIoConnection>,
    lines: &mut LineBuffer,
    queued: &mut std::collections::VecDeque<Vec<u8>>,
) -> Result<Option<Vec<u8>>, ControlError> {
    if let Some(line) = queued.pop_front() {
        return Ok(Some(line));
    }
    let mut buffer = [0u8; 16 * 1024];
    loop {
        let read = match reader.read(&mut buffer).await {
            Ok(0) | Err(_) => return Ok(None),
            Ok(read) => read,
        };
        queued.extend(lines.push(&buffer[..read])?);
        if let Some(line) = queued.pop_front() {
            return Ok(Some(line));
        }
    }
}

async fn write_line(writer: &mut WriteHalf<AgentIoConnection>, line: &str) -> std::io::Result<()> {
    let mut bytes = Vec::with_capacity(line.len() + 1);
    bytes.extend_from_slice(line.as_bytes());
    bytes.push(b'\n');
    writer.write_all(&bytes).await?;
    writer.flush().await
}
