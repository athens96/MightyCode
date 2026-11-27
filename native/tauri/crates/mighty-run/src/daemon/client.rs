//! MC-3: the app's connection to its background daemon. One long-lived connection carries every request (answered by id, in any
//! order) and every attached process's events; each process's events go to whoever subscribed, through a [`Cursor`] so a byte that
//! replay and live output both carry is handed on once. When the daemon goes, every waiting request fails and every subscriber is
//! told ([`ProcEvent::Gone`]): its readers end and its process counts as stopped.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use mighty_core::daemon::protocol::{
    Command, ControlError, ErrorCode, Event, LineBuffer, MAX_LINE_BYTES, PROTOCOL, ProcInfo, Reply, Welcome, parse_reply,
};
use mighty_core::daemon::ring::{Cursor, Stream};
use mighty_platform::{AgentIoConnection, AgentIoConnector};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::sync::{mpsc, oneshot, watch};

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|p| p.into_inner())
}

/// What a subscriber of one process hears.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcEvent {
    /// Bytes it never had, from `offset` in the process's stream; `lost` before them when the daemon no longer kept what it asked for.
    Output {
        stream: Stream,
        offset: u64,
        data: Vec<u8>,
        lost: u64,
    },
    /// What the daemon kept has all come; live output follows.
    Replayed,
    Exit {
        code: Option<i32>,
        error: Option<String>,
    },
    /// The app let go of the process (it keeps running in the daemon).
    Detached,
    /// The connection to the daemon is gone.
    Gone,
}

struct Sink {
    cursor: Cursor,
    sender: mpsc::UnboundedSender<ProcEvent>,
}

type Pending = HashMap<u64, oneshot::Sender<Result<Value, ControlError>>>;

/// One connection to the daemon.
pub struct DaemonClient {
    writer: mpsc::UnboundedSender<String>,
    pending: Arc<Mutex<Pending>>,
    sinks: Arc<Mutex<HashMap<String, Sink>>>,
    next: AtomicU64,
    closed: watch::Receiver<bool>,
    welcome: Welcome,
}

impl std::fmt::Debug for DaemonClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DaemonClient").field("welcome", &self.welcome).finish_non_exhaustive()
    }
}

impl DaemonClient {
    /// Connects to the daemon at `address` and says `hello`: the client, whether or not the daemon accepted it (see
    /// [`DaemonClient::accepted`]: one of another protocol may only ask `status` and `shutdown`). A wrong token is an error.
    pub async fn connect(
        connector: &dyn AgentIoConnector,
        address: &str,
        token: &str,
        app: &str,
        timeout: Duration,
    ) -> Result<Arc<DaemonClient>, ControlError> {
        let connection =
            connector.connect(address, timeout).await.map_err(|e| ControlError::new(ErrorCode::Failed, format!("not running ({e})")))?;
        DaemonClient::over(connection, token, app, timeout).await
    }

    /// As [`DaemonClient::connect`], over a connection already open.
    pub async fn over(connection: AgentIoConnection, token: &str, app: &str, timeout: Duration) -> Result<Arc<DaemonClient>, ControlError> {
        let (mut reader, mut writer) = tokio::io::split(connection);
        let hello = Command::Hello { protocol: PROTOCOL, app: app.to_string(), token: token.to_string() }.line(1);
        write_line(&mut writer, &hello).await.map_err(|e| ControlError::new(ErrorCode::Failed, e.to_string()))?;
        let mut lines = LineBuffer::new(MAX_LINE_BYTES);
        let mut queued = Vec::new();
        let first = tokio::time::timeout(timeout, read_line(&mut reader, &mut lines, &mut queued))
            .await
            .map_err(|_| ControlError::new(ErrorCode::Timeout, "the daemon did not answer"))?
            .ok_or_else(|| ControlError::new(ErrorCode::Failed, "the daemon closed the connection"))?;
        let welcome = match parse_reply(&first)? {
            Reply::Ok(value) => {
                Welcome::parse(&value).ok_or_else(|| ControlError::new(ErrorCode::Failed, "the daemon's answer is not a welcome"))?
            }
            Reply::Error(error) => return Err(error),
            Reply::Event(_) => return Err(ControlError::new(ErrorCode::Failed, "the daemon's answer was not expected")),
        };
        let (out, out_rx) = mpsc::unbounded_channel::<String>();
        let (closed_tx, closed) = watch::channel(false);
        let pending: Arc<Mutex<Pending>> = Arc::default();
        let sinks: Arc<Mutex<HashMap<String, Sink>>> = Arc::default();
        tokio::spawn(write_loop(writer, out_rx));
        let (read_pending, read_sinks) = (pending.clone(), sinks.clone());
        tokio::spawn(async move {
            while let Some(line) = read_line(&mut reader, &mut lines, &mut queued).await {
                dispatch(&line, &read_pending, &read_sinks);
            }
            // The daemon is gone: every wait fails, every subscriber hears it.
            for (_, waiting) in lock(&read_pending).drain() {
                let _ = waiting.send(Err(ControlError::new(ErrorCode::Failed, "the daemon went away")));
            }
            for (_, sink) in lock(&read_sinks).drain() {
                let _ = sink.sender.send(ProcEvent::Gone);
            }
            closed_tx.send_replace(true);
        });
        Ok(Arc::new(DaemonClient { writer: out, pending, sinks, next: AtomicU64::new(2), closed, welcome }))
    }

    pub fn welcome(&self) -> &Welcome {
        &self.welcome
    }

    /// Whether the daemon speaks this app's protocol (and so may be driven).
    pub fn accepted(&self) -> bool {
        self.welcome.accepted && self.welcome.protocol == PROTOCOL
    }

    pub fn is_closed(&self) -> bool {
        *self.closed.borrow()
    }

    /// Ends once the connection to the daemon is gone.
    pub async fn closed(&self) {
        let mut closed = self.closed.clone();
        let _ = closed.wait_for(|closed| *closed).await;
    }

    /// One request and its answer.
    pub async fn request(&self, command: Command) -> Result<Value, ControlError> {
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        let (sender, receiver) = oneshot::channel();
        lock(&self.pending).insert(id, sender);
        if self.is_closed() || self.writer.send(command.line(id)).is_err() {
            lock(&self.pending).remove(&id);
            return Err(ControlError::new(ErrorCode::Failed, "the daemon went away"));
        }
        receiver.await.unwrap_or_else(|_| Err(ControlError::new(ErrorCode::Failed, "the daemon went away")))
    }

    /// A request whose answer nobody waits for (a write, a resize, a kill); false when the connection is gone.
    pub fn send(&self, command: Command) -> bool {
        !self.is_closed() && self.writer.send(command.line(0)).is_ok()
    }

    /// Subscribes to `proc`'s events from `from` on, then asks the daemon to attach: what it kept from there, then live output.
    pub async fn attach(&self, proc: &str, from: u64) -> Result<(mpsc::UnboundedReceiver<ProcEvent>, Value), ControlError> {
        let (sender, receiver) = mpsc::unbounded_channel();
        lock(&self.sinks).insert(proc.to_string(), Sink { cursor: Cursor::at(from), sender });
        match self.request(Command::Attach { proc: proc.to_string(), from }).await {
            Ok(value) => Ok((receiver, value)),
            Err(error) => {
                lock(&self.sinks).remove(proc);
                Err(error)
            }
        }
    }

    /// The app lets go of `proc` (it keeps running in the daemon): its subscriber hears [`ProcEvent::Detached`].
    pub fn detach(&self, proc: &str) {
        if let Some(sink) = lock(&self.sinks).remove(proc) {
            let _ = sink.sender.send(ProcEvent::Detached);
        }
        self.send(Command::Detach { proc: proc.to_string() });
    }

    /// Every process the app attached to let go of (the app quits and keeps its work running).
    pub fn detach_all(&self) {
        let procs: Vec<String> = lock(&self.sinks).keys().cloned().collect();
        for proc in procs {
            self.detach(&proc);
        }
    }

    pub async fn list(&self) -> Result<Vec<ProcInfo>, ControlError> {
        let value = self.request(Command::List).await?;
        Ok(value
            .get("processes")
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(ProcInfo::parse).collect())
            .unwrap_or_default())
    }

    pub async fn status(&self) -> Result<Value, ControlError> {
        self.request(Command::Status).await
    }
}

fn dispatch(line: &[u8], pending: &Mutex<Pending>, sinks: &Mutex<HashMap<String, Sink>>) {
    let Ok(reply) = parse_reply(line) else { return };
    match reply {
        Reply::Event(value) => {
            let Some(event) = Event::parse(&value) else { return };
            let mut sinks = lock(sinks);
            let Some(sink) = sinks.get_mut(event.proc()) else { return };
            let sent = match event {
                Event::Output { offset, stream, data, .. } => {
                    let accepted = sink.cursor.accept(offset, &data);
                    if accepted.data.is_empty() {
                        return;
                    }
                    let from = sink.cursor.next - accepted.data.len() as u64;
                    sink.sender.send(ProcEvent::Output { stream, offset: from, data: accepted.data.to_vec(), lost: accepted.lost })
                }
                Event::Replayed { .. } => sink.sender.send(ProcEvent::Replayed),
                Event::Exit { proc, code, error } => {
                    let sent = sink.sender.send(ProcEvent::Exit { code, error });
                    sinks.remove(&proc);
                    sent
                }
            };
            if sent.is_err() {
                // Nobody reads it any more.
                drop(sinks);
            }
        }
        Reply::Ok(value) => answer(line, pending, Ok(value)),
        Reply::Error(error) => answer(line, pending, Err(error)),
    }
}

fn answer(line: &[u8], pending: &Mutex<Pending>, result: Result<Value, ControlError>) {
    let Some(id) = serde_json::from_slice::<Value>(line).ok().and_then(|v| v.get("id").and_then(Value::as_u64)) else { return };
    if let Some(waiting) = lock(pending).remove(&id) {
        let _ = waiting.send(result);
    }
}

async fn write_loop(mut writer: WriteHalf<AgentIoConnection>, mut lines: mpsc::UnboundedReceiver<String>) {
    while let Some(line) = lines.recv().await {
        if write_line(&mut writer, &line).await.is_err() {
            return;
        }
    }
    let _ = writer.shutdown().await;
}

async fn write_line(writer: &mut WriteHalf<AgentIoConnection>, line: &str) -> std::io::Result<()> {
    let mut bytes = Vec::with_capacity(line.len() + 1);
    bytes.extend_from_slice(line.as_bytes());
    bytes.push(b'\n');
    writer.write_all(&bytes).await?;
    writer.flush().await
}

async fn read_line(reader: &mut ReadHalf<AgentIoConnection>, lines: &mut LineBuffer, queued: &mut Vec<Vec<u8>>) -> Option<Vec<u8>> {
    if !queued.is_empty() {
        return Some(queued.remove(0));
    }
    let mut buffer = vec![0u8; 64 * 1024];
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
