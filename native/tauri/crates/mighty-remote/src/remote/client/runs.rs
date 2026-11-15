//! The client's runs (C# `RemoteClientService.Runs`, R\RemoteClientService.Runs.cs; M/Remote/RemoteClientService.swift `start`,
//! `follow`, `stop`, `send`, `dropRuns`, `shutdown`; plan §1.6): the start, the poll loop (100 ms while more waits, else 250 ms + 150
//! ms × quiet, quiet up to 4), passing failures asked again every second for the retry window, the route probe before each request,
//! answers, stops. A run holds the connection entry and revision it started under: whatever it hits later fails that connection only
//! while it is still that one, never a newer one. One poll's events reach the pane under the run's applying lock, and a stop or a lost
//! connection says the run ended under it too, so the pane's last word always comes after the poll being applied. Owner: W8-D.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::ThreadId;
use std::time::{Duration, Instant};

use indexmap::IndexMap;
use mighty_core::model::Workspace;
use mighty_core::permission::ToolPermissionRequest;
use mighty_core::wire;
use mighty_platform::BoxFuture;
use mighty_run::model::{RunEvent, StartRunRequest};
use mighty_run::plan_mode::PlanDecision;
use mighty_run::questionnaire::UserQuestionAnswer;
use mighty_run::service::{RemoteRunner, RemoteSinks};
use mighty_run::strings::{get, get_with};
use mighty_run::{Result as RunResult, RunError, providers};
use serde_json::Value;
use tokio::sync::Notify;

use super::{ClientError, ClientSinks, Gate, RemoteClient, lock, message, unchanged};
use crate::remote::events::{self, Incoming};
use crate::remote::text;
use crate::remote::transport::{self, Method, RemoteTarget, Request};
use crate::remote::validation;
use crate::remote::wire::{
    self as remote_wire, RemoteConnectionStatus, RemoteWireAnswer, RemoteWireJob, RemoteWirePermission, RemoteWirePlan,
    RemoteWireQuestions, RemoteWireStart, protocol, to_bytes,
};

/// The Mac's: 30 s for a start without attachments, 5 s for a stop.
const START_TIMEOUT: Duration = Duration::from_secs(30);
const STOP_TIMEOUT: Duration = Duration::from_secs(5);

/// A lock its own thread may take again (C#'s `Monitor`): a callback that ends its run from inside a poll being applied gets in.
#[derive(Default)]
pub(crate) struct Applying {
    owner: Mutex<(Option<ThreadId>, usize)>,
    free: Condvar,
}

/// Held on one thread only: never across a wait.
pub(crate) struct Applied<'a> {
    lock: &'a Applying,
    _thread: std::marker::PhantomData<*const ()>,
}

impl Applying {
    pub(crate) fn enter(&self) -> Applied<'_> {
        let me = std::thread::current().id();
        let mut owner = lock(&self.owner);
        loop {
            match owner.0 {
                None => {
                    *owner = (Some(me), 1);
                    break;
                }
                Some(holder) if holder == me => {
                    owner.1 += 1;
                    break;
                }
                Some(_) => owner = self.free.wait(owner).unwrap_or_else(std::sync::PoisonError::into_inner),
            }
        }
        Applied { lock: self, _thread: std::marker::PhantomData }
    }

    /// Taken at once when it is free or already this thread's; None while another thread holds it.
    fn try_enter(&self) -> Option<Applied<'_>> {
        let me = std::thread::current().id();
        let mut owner = lock(&self.owner);
        match owner.0 {
            None => *owner = (Some(me), 1),
            Some(holder) if holder == me => owner.1 += 1,
            Some(_) => return None,
        }
        Some(Applied { lock: self, _thread: std::marker::PhantomData })
    }
}

/// `work` under the run's applying lock from async code: at once when the lock is free (or this thread's), else on a blocking thread, so
/// a poll being applied on another thread never parks a runtime worker (nor the timers it drives) while it waits. None when the runtime
/// is shutting down and the work never ran; a panic in it goes on as the caller's.
async fn under_applying<R: Send + 'static>(run: &Arc<Run>, work: impl FnOnce() -> R + Send + 'static) -> Option<R> {
    if let Some(_held) = run.applying.try_enter() {
        return Some(work());
    }
    let run = run.clone();
    match tokio::task::spawn_blocking(move || {
        let _held = run.applying.enter();
        work()
    })
    .await
    {
        Ok(value) => Some(value),
        Err(error) if error.is_panic() => std::panic::resume_unwind(error.into_panic()),
        Err(_) => None,
    }
}

impl Drop for Applied<'_> {
    fn drop(&mut self) {
        let mut owner = lock(&self.lock.owner);
        owner.1 -= 1;
        if owner.1 == 0 {
            owner.0 = None;
            self.lock.free.notify_all();
        }
    }
}

/// Cancelled when the run stops, is dropped or the app closes.
#[derive(Default)]
pub(crate) struct Cancel {
    cancelled: AtomicBool,
    notify: Notify,
}

impl Cancel {
    pub(crate) fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }
    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
    pub(crate) async fn cancelled(&self) {
        loop {
            let notified = self.notify.notified();
            if self.is_cancelled() {
                return;
            }
            notified.await;
        }
    }
}

/// What a run keeps that changes, read and written under the client's gate.
#[derive(Default)]
pub(crate) struct RunState {
    /// Set once, when the host accepted the run.
    pub job_id: Option<String>,
    pub cursor: i64,
    pub stopping: bool,
    /// Waiting tool requests by id, with the host's run id their answer carries.
    pub host_run_ids: HashMap<String, String>,
}

/// One remote pane's run.
pub(crate) struct Run {
    pub connection_id: String,
    pub entry: u64,
    pub revision: u64,
    pub target: RemoteTarget,
    key: String,
    pub cancel: Cancel,
    pub applying: Applying,
    pub state: Mutex<RunState>,
}

/// The key is left out.
impl std::fmt::Debug for Run {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Run").field("connection_id", &self.connection_id).field("target", &self.target).finish()
    }
}

/// What one poll held: the body's counters and each item read for the pane (None: not even an event).
struct Poll {
    cursor: i64,
    last_cursor: i64,
    gap: bool,
    done: bool,
    items: Vec<Option<Incoming>>,
}

/// A callback that failed (C#'s exception), with what it said.
fn panic_text(payload: Box<dyn std::any::Any + Send>) -> String {
    payload.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| payload.downcast_ref::<String>().cloned()).unwrap_or_default()
}

impl RemoteClient {
    fn sinks(&self) -> Option<ClientSinks> {
        lock(&self.0.sinks).clone()
    }

    /// The pane's event, Err with what a callback that failed said.
    fn emit(&self, value: RunEvent) -> Result<(), String> {
        let Some(sinks) = self.sinks() else { return Ok(()) };
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (sinks.event)(value))).map_err(panic_text)
    }

    fn emit_permission(&self, value: ToolPermissionRequest) -> Result<(), String> {
        let Some(sinks) = self.sinks() else { return Ok(()) };
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (sinks.permission)(value))).map_err(panic_text)
    }

    /// `IsRunning(paneId)`.
    pub fn is_running(&self, pane_id: &str) -> bool {
        lock(&self.0.gate).runs.contains_key(pane_id)
    }

    /// `Listed(paneId, run)`: the pane's run is still `run`. Under the gate.
    fn listed(gate: &Gate, pane_id: &str, run: &Arc<Run>) -> bool {
        gate.runs.get(pane_id).is_some_and(|current| Arc::ptr_eq(current, run))
    }

    /// `Current(paneId, run)`: still `run`, not stopping, on the connection it started on.
    fn current(&self, pane_id: &str, run: &Arc<Run>) -> bool {
        let gate = lock(&self.0.gate);
        Self::listed(&gate, pane_id, run) && !lock(&run.state).stopping && unchanged(&gate, &run.connection_id, run.entry, run.revision)
    }

    /// `StartAsync(request, workspace)`: sends one request of a remote workspace's pane to the host and follows its output.
    pub async fn start(&self, request: StartRunRequest, workspace: &Workspace) -> Result<(), ClientError> {
        if lock(&self.0.gate).closing {
            return Err(message(get("common.appClosing")));
        }
        let request = request.validate().map_err(|e| message(e.message().to_string()))?;
        let (run, host_workspace_id) = {
            let mut gate = lock(&self.0.gate);
            if gate.closing {
                return Err(message(get("common.appClosing")));
            }
            if gate.runs.contains_key(&request.session_id) {
                return Err(message(get("run.error.paneAlreadyRunning")));
            }
            if gate.runs.len() >= protocol::MAXIMUM_RUNS {
                return Err(message(get("run.error.concurrentLimit")));
            }
            let not_connected = || message(get("remote.run.error.notConnected"));
            let reference = workspace.remote.clone().filter(|_| workspace.id() == request.workspace_id).ok_or_else(not_connected)?;
            let connection_id = reference.connection_id.clone().unwrap_or_default();
            let host_workspace_id = reference.workspace_id.clone().unwrap_or_default();
            let connection = gate.connections.get(&connection_id).ok_or_else(not_connected)?;
            let (Some(target), Some(key)) = (connection.target.clone(), connection.key.clone()) else { return Err(not_connected()) };
            if connection.info.status != RemoteConnectionStatus::Connected
                || !connection.info.shared().iter().any(|w| w.id == host_workspace_id)
            {
                return Err(not_connected());
            }
            if request.kind == "claude" {
                let provider = connection
                    .info
                    .runtime
                    .as_ref()
                    .and_then(|r| r.providers.iter().find(|p| p.id == request.provider && p.available))
                    .ok_or_else(|| message(get_with("remote.host.error.cliMissing", &[("name", providers::name(&request.provider))])))?;
                if let Some(problem) = validation::capability_problem(&request, &provider.capabilities) {
                    return Err(message(problem));
                }
            }
            let run = Arc::new(Run {
                connection_id,
                entry: connection.serial,
                revision: connection.revision,
                target,
                key,
                cancel: Cancel::default(),
                applying: Applying::default(),
                state: Mutex::new(RunState::default()),
            });
            gate.runs.insert(request.session_id.clone(), run.clone());
            (run, host_workspace_id)
        };
        let result = self.post_start(&request, &run, &host_workspace_id).await;
        if let Err(error) = &result {
            {
                let mut gate = lock(&self.0.gate);
                if Self::listed(&gate, &request.session_id, &run) {
                    gate.runs.remove(&request.session_id);
                }
            }
            // The host no longer takes this key (it restarted sharing): the connection is not connected any more.
            if error.status() == Some(401) {
                self.fail(&run.connection_id, &error.message(), Some((run.entry, run.revision))).await;
            }
        }
        result
    }

    async fn post_start(&self, request: &StartRunRequest, run: &Arc<Run>, host_workspace_id: &str) -> Result<(), ClientError> {
        let body = to_bytes(&RemoteWireStart::from(request, host_workspace_id).to_value());
        if body.len() > mighty_run::attachments::MAXIMUM_REQUEST_BYTES
            || request.attachment_list().is_none() && body.len() > protocol::MAXIMUM_REQUEST_BYTES
        {
            return Err(message(get("remote.run.error.tooLarge")));
        }
        self.ensure_route(&run.connection_id, run.entry, run.revision, &run.target).await?;
        let options = Request {
            body: Some(&body),
            timeout: Some(if request.attachment_list().is_none() { START_TIMEOUT } else { protocol::UPLOAD_TIMEOUT }),
            headers: &[(protocol::GRAPH_HEADER, protocol::GRAPH_RUNS)],
            ..Request::default()
        };
        let data = transport::request(&run.target, &run.key, Method::Post, &format!("{}runs", protocol::PATH_PREFIX), options)
            .await
            .map_err(ClientError::Transport)?;
        let accepted = RemoteWireJob::decode(&data).ok().filter(|job| job.protocol == protocol::VERSION && wire::identifier(&job.job_id));
        let Some(accepted) = accepted else { return Err(message(get("remote.connect.error.notMighty"))) };
        let (cancelled, changed) = {
            let mut gate = lock(&self.0.gate);
            let mut state = lock(&run.state);
            state.job_id = Some(accepted.job_id.clone());
            let cancelled = gate.closing || !Self::listed(&gate, &request.session_id, run) || state.stopping;
            let changed = !unchanged(&gate, &run.connection_id, run.entry, run.revision);
            // Followed from the same moment it was found still current: a disconnect cannot slip in between.
            if !cancelled && !changed {
                drop(state);
                gate.followers.push(run.clone());
                let (client, pane, followed) = (self.clone(), request.session_id.clone(), run.clone());
                tokio::spawn(async move { client.follow(pane, followed).await });
            }
            (cancelled, changed)
        };
        if cancelled || changed {
            // Stopped, dropped, closing, or its connection changed while the host was accepting it: it stops there too.
            self.stop_on_host(run).await;
            {
                let mut gate = lock(&self.0.gate);
                if Self::listed(&gate, &request.session_id, run) {
                    gate.runs.remove(&request.session_id);
                }
            }
            if cancelled {
                return Err(ClientError::Cancelled);
            }
            return Err(message(get("remote.connect.error.changed")));
        }
        Ok(())
    }

    /// `FollowsEnded()`: every follow loop alive now has ended (the checks wait on this instead of sleeping).
    pub async fn follows_ended(&self) {
        while !lock(&self.0.gate).followers.is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    async fn follow(&self, pane_id: String, run: Arc<Run>) {
        // The poll loop in a task of its own: one that panics (a hostile answer this side did not foresee) still ends its run.
        let (client, pane, polled) = (self.clone(), pane_id.clone(), run.clone());
        let polled = tokio::spawn(async move { client.poll(&pane, &polled).await }).await;
        match polled {
            // Polling ends once the run ended, stopped, or its connection changed under it: only the last is still listed, and ends here.
            Ok(Ok(())) => self.end(&pane_id, &run, &get("remote.connect.error.changed")).await,
            // A callback that failed: the run ends, never leaving the pane running with nobody polling.
            Ok(Err(failure)) => self.end(&pane_id, &run, &failure).await,
            Err(error) if error.is_panic() => self.end(&pane_id, &run, &panic_text(error.into_panic())).await,
            Err(_) => self.end(&pane_id, &run, &get("common.appClosing")).await,
        }
        let mut gate = lock(&self.0.gate);
        gate.followers.retain(|r| !Arc::ptr_eq(r, &run));
    }

    /// The poll loop; Err with what a pane callback that failed said.
    async fn poll(&self, pane_id: &str, run: &Arc<Run>) -> Result<(), String> {
        let mut quiet: u64 = 0;
        let mut failing_since: Option<Instant> = None;
        while self.current(pane_id, run) {
            // Outside the tunnel the connection is lost when it is still this run's, and never asked again.
            if let Err(error) = self.ensure_route(&run.connection_id, run.entry, run.revision, &run.target).await {
                self.end(pane_id, run, &error.message()).await;
                return Ok(());
            }
            let (job, cursor) = {
                let state = lock(&run.state);
                (state.job_id.clone().unwrap_or_default(), state.cursor)
            };
            let path = format!("{}runs/{job}/events?cursor={cursor}", protocol::PATH_PREFIX);
            let answer = tokio::select! {
                answer = transport::request(&run.target, &run.key, Method::Get, &path, Request::default()) => answer,
                _ = run.cancel.cancelled() => return Ok(()),
            };
            let data = match answer {
                Ok(data) => {
                    failing_since = None;
                    data
                }
                Err(error) if error.transient() && failing_since.is_none_or(|since| since.elapsed() < self.0.options.poll_retry_window) => {
                    // Asked again for a while, well inside the host's heartbeat.
                    failing_since.get_or_insert_with(Instant::now);
                    if !pause(run, Duration::from_secs(1)).await {
                        return Ok(());
                    }
                    continue;
                }
                Err(error) => {
                    self.lose(pane_id, run, error.message()).await;
                    return Ok(());
                }
            };
            let Some(poll) = read_poll(&data, run, pane_id) else {
                self.lose(pane_id, run, &get("remote.run.error.badOutput")).await;
                return Ok(());
            };
            let count = poll.items.len();
            let more = {
                let _applying = run.applying.enter();
                if !self.current(pane_id, run) {
                    return Ok(());
                }
                if poll.gap {
                    self.emit(RunEvent::log(pane_id, "system", &get("remote.run.notice.gap"), None))?;
                }
                let mut terminal = false;
                let mut skipped = false;
                for incoming in poll.items {
                    // Ended from inside a callback on this thread: nothing more of this run reaches the pane.
                    if !Self::listed(&lock(&self.0.gate), pane_id, run) {
                        return Ok(());
                    }
                    // One event this app cannot use is left out, and said so once; one it reads but does not draw is left out silently.
                    let Some(incoming) = incoming.filter(Incoming::usable) else {
                        skipped = true;
                        continue;
                    };
                    if let Some(permission) = incoming.permission {
                        self.receive(run, permission, incoming.host_run_id.unwrap_or_default())?;
                        continue;
                    }
                    let Some(value) = incoming.event else { continue };
                    let ends = value.kind == "status" && matches!(value.status.as_deref(), Some("completed" | "error" | "stopped"));
                    self.emit(value)?;
                    terminal |= ends;
                }
                if skipped {
                    self.emit(RunEvent::log(pane_id, "system", &get("remote.run.notice.skipped"), None))?;
                }
                lock(&run.state).cursor = poll.cursor;
                if poll.done {
                    {
                        let mut gate = lock(&self.0.gate);
                        if Self::listed(&gate, pane_id, run) {
                            gate.runs.remove(pane_id);
                        }
                    }
                    self.0.pending.forget(pane_id);
                    if !terminal {
                        self.emit(RunEvent::state(pane_id, "error", None))?;
                    }
                    return Ok(());
                }
                poll.last_cursor > poll.cursor
            };
            // More waits on the host: read it soon (at most ten polls a second per run). Otherwise wait longer while nothing comes.
            if more {
                quiet = 0;
                if !pause(run, Duration::from_millis(100)).await {
                    return Ok(());
                }
                continue;
            }
            quiet = if count == 0 { (quiet + 1).min(4) } else { 0 };
            if !pause(run, Duration::from_millis(250 + 150 * quiet)).await {
                return Ok(());
            }
        }
        Ok(())
    }

    /// `Receive(run, permission, hostRunId)`: a tool request reaches the pane, which keeps the newest 32 waiting; older ones are withdrawn.
    /// Under the run's applying lock.
    fn receive(&self, run: &Arc<Run>, permission: ToolPermissionRequest, host_run_id: String) -> Result<(), String> {
        let withdrawn = self.0.pending.admit(&permission);
        {
            let _gate = lock(&self.0.gate);
            let mut state = lock(&run.state);
            for old in &withdrawn {
                state.host_run_ids.remove(&old.id);
            }
            if permission.state == "pending" {
                state.host_run_ids.insert(permission.id.clone(), host_run_id);
            } else {
                state.host_run_ids.remove(&permission.id);
            }
        }
        for old in withdrawn {
            self.emit_permission(old)?;
        }
        self.emit_permission(permission)
    }

    /// `LoseAsync`: a poll failed for good: the connection is lost while it is still this run's (its runs end then), else this run alone.
    async fn lose(&self, pane_id: &str, run: &Arc<Run>, words: &str) {
        // Stopped or dropped meanwhile: a failure that races a stop is not the connection's.
        if run.cancel.is_cancelled() {
            return;
        }
        let listed = Self::listed(&lock(&self.0.gate), pane_id, run);
        if listed && !self.fail(&run.connection_id, words, Some((run.entry, run.revision))).await {
            self.end(pane_id, run, words).await;
        }
    }

    /// `EndAsync`: this run alone ends as lost, unless it already ended or is stopping.
    async fn end(&self, pane_id: &str, run: &Arc<Run>, words: &str) {
        let (client, pane, held, words) = (self.clone(), pane_id.to_string(), run.clone(), words.to_string());
        let ended = under_applying(run, move || {
            {
                let mut gate = lock(&client.0.gate);
                if !Self::listed(&gate, &pane, &held) || lock(&held.state).stopping {
                    return false;
                }
                gate.runs.remove(&pane);
            }
            // A callback that fails here too is only let go: the run is no longer listed either way.
            let _ = client.say_lost(&pane, &words);
            true
        })
        .await;
        if ended == Some(true) {
            self.stop_on_host(run).await;
        }
    }

    /// `SayLost`: the reason, cut as the Mac cuts it, then the error. Under the run's applying lock.
    fn say_lost(&self, pane_id: &str, words: &str) -> Result<(), String> {
        self.0.pending.forget(pane_id);
        let detail = text::cut(words, text::DEFAULT_CUT);
        self.emit(RunEvent::log(pane_id, "error", &get_with("remote.run.error.connectionLost", &[("detail", &detail)]), None))?;
        self.emit(RunEvent::state(pane_id, "error", None))
    }

    /// `StopAsync(paneId)`.
    pub async fn stop(&self, pane_id: &str) {
        let run = {
            let gate = lock(&self.0.gate);
            let Some(run) = gate.runs.get(pane_id).cloned() else { return };
            let mut state = lock(&run.state);
            if state.stopping {
                return;
            }
            state.stopping = true;
            drop(state);
            run
        };
        run.cancel.cancel();
        self.stop_on_host(&run).await;
        let (client, pane, held) = (self.clone(), pane_id.to_string(), run.clone());
        under_applying(&run, move || {
            {
                let mut gate = lock(&client.0.gate);
                if !Self::listed(&gate, &pane, &held) {
                    return;
                }
                gate.runs.remove(&pane);
            }
            client.0.pending.forget(&pane);
            let _ = client.emit(RunEvent::state(&pane, "stopped", None));
        })
        .await;
    }

    /// `StopOnHostAsync`: the route checked, then the host's stop with `{}` (a failure is let go: the host stops a run nobody polls within
    /// 30 s anyway). Boxed: a route outside the tunnel fails the connection, which stops its runs here again.
    fn stop_on_host(&self, run: &Arc<Run>) -> BoxFuture<'static, ()> {
        let (client, run) = (self.clone(), run.clone());
        Box::pin(async move {
            let Some(job) = lock(&run.state).job_id.clone() else { return };
            if client.ensure_route(&run.connection_id, run.entry, run.revision, &run.target).await.is_err() {
                return;
            }
            let path = format!("{}runs/{job}/stop", protocol::PATH_PREFIX);
            let options = Request { body: Some(b"{}"), timeout: Some(STOP_TIMEOUT), ..Request::default() };
            let _ = transport::request(&run.target, &run.key, Method::Post, &path, options).await;
        })
    }

    async fn send(&self, pane_id: &str, action: &str, body: impl FnOnce(&Run, &str) -> Value) -> Result<bool, ClientError> {
        let (run, job) = {
            let gate = lock(&self.0.gate);
            let gone = || message(get("run.error.permissionRunGone"));
            let run = gate.runs.get(pane_id).cloned().ok_or_else(gone)?;
            let state = lock(&run.state);
            let job = state.job_id.clone().filter(|_| !state.stopping).ok_or_else(gone)?;
            drop(state);
            (run, job)
        };
        self.ensure_route(&run.connection_id, run.entry, run.revision, &run.target).await?;
        let bytes = to_bytes(&body(&run, &job));
        let path = format!("{}runs/{job}/{action}", protocol::PATH_PREFIX);
        let data = transport::request(&run.target, &run.key, Method::Post, &path, Request { body: Some(&bytes), ..Request::default() })
            .await
            .map_err(ClientError::Transport)?;
        Ok(RemoteWireAnswer::decode(&data).is_ok_and(|answer| answer.accepted))
    }

    /// `HostRunId(run, requestId)`: the host's run id of a waiting request; the job's for one the pane does not hold.
    fn host_run_id(&self, run: &Run, job: &str, request_id: &str) -> String {
        let _gate = lock(&self.0.gate);
        lock(&run.state).host_run_ids.get(request_id).cloned().unwrap_or_else(|| job.to_string())
    }

    pub async fn respond_to_permission(&self, pane_id: &str, request_id: &str, allow: bool) -> Result<bool, ClientError> {
        self.send(pane_id, "permission", |run, job| {
            RemoteWirePermission { run_id: self.host_run_id(run, job, request_id), request_id: request_id.into(), allow }.to_value()
        })
        .await
    }

    pub async fn answer_questions(
        &self,
        pane_id: &str,
        request_id: &str,
        answers: &IndexMap<String, UserQuestionAnswer>,
    ) -> Result<bool, ClientError> {
        self.send(pane_id, "questions", |run, job| {
            RemoteWireQuestions { run_id: self.host_run_id(run, job, request_id), request_id: request_id.into(), answers: answers.clone() }
                .to_value()
        })
        .await
    }

    pub async fn answer_plan(&self, pane_id: &str, request_id: &str, decision: &PlanDecision) -> Result<bool, ClientError> {
        self.send(pane_id, "plan", |run, job| {
            RemoteWirePlan::from(&self.host_run_id(run, job, request_id), request_id, decision).to_value()
        })
        .await
    }

    /// `TakeRuns(connectionId)`: the connection's runs, taken off the list. Under the gate, in the section that changes its revision.
    pub(crate) fn take_runs(&self, gate: &mut Gate, connection_id: &str) -> Vec<(String, Arc<Run>)> {
        let taken: Vec<(String, Arc<Run>)> =
            gate.runs.iter().filter(|(_, run)| run.connection_id == connection_id).map(|(pane, run)| (pane.clone(), run.clone())).collect();
        for (pane, _) in &taken {
            gate.runs.remove(pane);
        }
        taken
    }

    /// The part of `EndRunsAsync` that happens at once: each taken run is cancelled and its pane gets its last word (stopped, or lost with
    /// `words`); a callback that fails for one pane leaves every other pane its last word. The runs to stop on the host.
    pub(crate) fn say_ended(&self, taken: &[(String, Arc<Run>)], words: Option<&str>) -> Vec<Arc<Run>> {
        for (pane, run) in taken {
            run.cancel.cancel();
            let _applying = run.applying.enter();
            self.say_last(pane, words);
        }
        taken.iter().map(|(_, run)| run.clone()).collect()
    }

    /// A taken run's last word (stopped, or lost with `words`). Under the run's applying lock; a callback that fails is let go.
    fn say_last(&self, pane: &str, words: Option<&str>) {
        let _ = match words {
            Some(words) => self.say_lost(pane, words),
            None => {
                self.0.pending.forget(pane);
                self.emit(RunEvent::state(pane, "stopped", None))
            }
        };
    }

    /// The host stops a run it no longer hears from anyway; this is only sooner.
    pub(crate) async fn stop_all_on_host(&self, runs: Vec<Arc<Run>>) {
        let stops: Vec<_> = runs.iter().map(|run| tokio::spawn(self.stop_on_host(run))).collect();
        for stop in stops {
            let _ = stop.await;
        }
    }

    /// `EndRunsAsync(taken, message)`: taken runs end (replaced or disconnected as stopped, lost with its reason as an error).
    pub(crate) async fn end_runs(&self, taken: Vec<(String, Arc<Run>)>, words: Option<&str>) {
        // As `say_ended`, without parking a runtime worker while a poll of one of them is being applied on another thread.
        for (pane, run) in &taken {
            run.cancel.cancel();
            let (client, pane, words) = (self.clone(), pane.clone(), words.map(str::to_string));
            under_applying(run, move || client.say_last(&pane, words.as_deref())).await;
        }
        self.stop_all_on_host(taken.into_iter().map(|(_, run)| run).collect()).await;
    }

    /// `DisposeAsync`: the app is closing: nothing starts any more, and every run stops, on the host too.
    pub async fn dispose(&self) {
        let panes: Vec<String> = {
            let mut gate = lock(&self.0.gate);
            gate.closing = true;
            gate.runs.keys().cloned().collect()
        };
        for pane in panes {
            self.stop(&pane).await;
        }
        // The loops end once cancelled; a callback still busy is not waited on for long.
        let _ = tokio::time::timeout(Duration::from_secs(2), self.follows_ended()).await;
    }
}

async fn pause(run: &Run, delay: Duration) -> bool {
    tokio::select! {
        _ = tokio::time::sleep(delay) => true,
        _ = run.cancel.cancelled() => false,
    }
}

/// `Read(data, run, paneId)`: one poll as the Mac checks it (protocol, cursors, at most 256 events), each event read for the pane on its
/// own (a key repeated inside one refuses that one alone); None when the poll itself is bad, which loses the connection.
fn read_poll(data: &[u8], run: &Run, pane_id: &str) -> Option<Poll> {
    let members = remote_wire::parse_members(data).ok()?;
    let member = |name: &str| members.get(name).map(|raw| remote_wire::parse(raw.get().as_bytes()));
    let whole = |name: &str| match member(name) {
        Some(Ok(Value::Number(n))) => n.as_i64().filter(|n| i32::try_from(*n).is_ok()),
        _ => None,
    };
    let flag = |name: &str| match member(name) {
        Some(Ok(Value::Bool(b))) => Some(b),
        _ => None,
    };
    let (protocol_version, cursor, last_cursor) = (whole("protocol")?, whole("cursor")?, whole("lastCursor")?);
    let (gap, done) = (flag("gap")?, flag("done")?);
    let events: Vec<Box<serde_json::value::RawValue>> = serde_json::from_str(members.get("events")?.get()).ok()?;
    let (job, run_cursor) = {
        let state = lock(&run.state);
        (state.job_id.clone().unwrap_or_default(), state.cursor)
    };
    if protocol_version != i64::from(protocol::VERSION)
        || cursor < run_cursor
        || last_cursor < cursor
        || events.len() > protocol::MAXIMUM_POLL_EVENTS
    {
        return None;
    }
    let mut items = Vec::with_capacity(events.len());
    let mut at = run_cursor;
    for element in &events {
        let item = remote_wire::parse(element.get().as_bytes())
            .ok()
            .and_then(|value| remote_wire::checked_value(&value, remote_wire::WIRE_EVENT).ok());
        let Some(item) = item else {
            items.push(None);
            continue;
        };
        let item_cursor = item.get("cursor").and_then(Value::as_i64).unwrap_or_default();
        let Some(event) = item.get("event").and_then(Value::as_object) else {
            items.push(None);
            continue;
        };
        if item_cursor <= at || item_cursor > cursor {
            return None;
        }
        at = item_cursor;
        items.push(Some(events::read(event, &job, pane_id, true)));
    }
    Some(Poll { cursor, last_cursor, gap, done, items })
}

/// The client as the desktop service's runner for remote panes (C# DesktopService.Remote.cs: `StartRemoteAsync`, `AnswerRemote`).
impl RemoteRunner for RemoteClient {
    fn attach(&self, sinks: RemoteSinks) {
        self.set_sinks(ClientSinks { event: sinks.event, permission: sinks.permission });
    }

    fn connect(&self, workspace: Workspace) -> BoxFuture<'_, RunResult<()>> {
        Box::pin(async move {
            let reference = workspace.remote.clone().ok_or_else(|| RunError::State(get("remote.run.error.notConnected")))?;
            let info =
                self.ensure_connected(reference.connection_id.as_deref().unwrap_or_default()).await.map_err(ClientError::run_error)?;
            let host_workspace = reference.workspace_id.as_deref().unwrap_or_default();
            if !info.shared().iter().any(|w| w.id == host_workspace) {
                return Err(RunError::State(get("remote.workspace.error.notShared")));
            }
            Ok(())
        })
    }

    fn start(&self, request: StartRunRequest, workspace: Workspace) -> BoxFuture<'_, RunResult<()>> {
        Box::pin(async move { RemoteClient::start(self, request, &workspace).await.map_err(ClientError::run_error) })
    }

    fn stop(&self, session_id: String) -> BoxFuture<'_, RunResult<()>> {
        Box::pin(async move {
            RemoteClient::stop(self, &session_id).await;
            Ok(())
        })
    }

    fn respond_to_tool_permission(&self, session_id: String, request_id: String, allow: bool) -> BoxFuture<'_, RunResult<()>> {
        Box::pin(
            async move { self.respond_to_permission(&session_id, &request_id, allow).await.map(|_| ()).map_err(ClientError::run_error) },
        )
    }

    fn answer_questions(
        &self,
        session_id: String,
        request_id: String,
        answers: IndexMap<String, UserQuestionAnswer>,
    ) -> BoxFuture<'_, RunResult<()>> {
        Box::pin(async move {
            RemoteClient::answer_questions(self, &session_id, &request_id, &answers).await.map(|_| ()).map_err(ClientError::run_error)
        })
    }

    fn answer_plan(&self, session_id: String, request_id: String, decision: PlanDecision) -> BoxFuture<'_, RunResult<()>> {
        Box::pin(async move {
            RemoteClient::answer_plan(self, &session_id, &request_id, &decision).await.map(|_| ()).map_err(ClientError::run_error)
        })
    }

    fn is_running(&self, session_id: &str) -> bool {
        RemoteClient::is_running(self, session_id)
    }

    fn dispose(&self) -> BoxFuture<'_, ()> {
        Box::pin(RemoteClient::dispose(self))
    }
}
