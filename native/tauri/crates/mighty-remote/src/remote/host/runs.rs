//! The sharing host's runs (C# `RemoteHostService.Runs`, `IRemoteRunner`, `RemoteRunnerSinks`, R\RemoteHostService.Runs.cs;
//! M/Remote/RemoteHostService.swift `startRun` … `prune`; plan §1.6): a client's run goes through this PC's runner in a shared folder,
//! its events wait in a bounded buffer the client reads by cursor (4096 events and 4 MiB; a poll at most 256 events and 1.5 MiB;
//! finished runs kept 2 minutes, 32 at most), and a run nobody polls for the heartbeat is stopped. `StartRunAsync`'s order: 415, 400
//! empty, 400 decode, 413 over 512 KiB without attachments, 400 models, 400 `Validate`, 403 `notShared`, 403 full access, 400 a command
//! too long, 429 at 16, 503 admission, the Claude checks, the Antigravity grant, under the lock 503/429, 202. The host's runner is a
//! `RunManager` with PowerShell commands, no pane MCP, no delegation and no picture cache. Owner: W8-D.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use indexmap::IndexMap;
use mighty_core::json::Wire;
use mighty_core::model::{PlanRecord, Workspace};
use mighty_core::permission::ToolPermissionRequest;
use mighty_core::wire;
use mighty_graph::node::ExecutionGraphNode;
use mighty_platform::BoxFuture;
use mighty_run::antigravity::Lease;
use mighty_run::manager::{ResolveWorkspace, RunManager};
use mighty_run::model::{RunEvent, StartRunRequest};
use mighty_run::plan_mode::PlanDecision;
use mighty_run::questionnaire::UserQuestionAnswer;
use mighty_run::strings::{get, get_with};
use mighty_run::{RunError, providers};
use serde_json::{Map, Value};
use tokio::sync::Notify;

use super::{HostFailure, RemoteHost, failure_response, lock, reply};
use crate::remote::command_shell;
use crate::remote::events;
use crate::remote::http_server::{HttpRequest, HttpResponse};
use crate::remote::strings;
use crate::remote::validation;
use crate::remote::wire::{
    RemoteWireAnswer, RemoteWireJob, RemoteWirePermission, RemoteWirePlan, RemoteWireQuestions, RemoteWireStart, RemoteWireSteer, protocol,
    to_bytes,
};

/// C# `IRemoteRunner`: the runner a sharing host's client runs go through.
pub trait HostRunner: Send + Sync {
    fn start(&self, request: StartRunRequest) -> BoxFuture<'_, mighty_run::Result<()>>;
    fn stop(&self, session_id: String) -> BoxFuture<'_, mighty_run::Result<()>>;
    fn respond_to_tool_permission(&self, session_id: &str, request_id: &str, allow: bool) -> mighty_run::Result<()>;
    fn answer_questionnaire(
        &self,
        session_id: &str,
        request_id: &str,
        answers: &IndexMap<String, UserQuestionAnswer>,
    ) -> mighty_run::Result<()>;
    fn answer_plan(&self, session_id: &str, request_id: &str, decision: &PlanDecision) -> mighty_run::Result<PlanRecord>;
    fn dispose(&self) -> BoxFuture<'_, ()>;
}

/// A graph node of a run, by the run's id, as mighty-run's runner gives it (its JSON).
pub type GraphSink = Arc<dyn Fn(&str, Value) + Send + Sync>;

/// C# `RemoteRunnerSinks(Event, Permission, GraphNode, Workspace)`: where a host's runner hands a run's events, tool requests and graph
/// nodes (by session id), and where it looks its workspace up (only a workspace the sharing serves now, at its shared path).
#[derive(Clone)]
pub struct RunnerSinks {
    pub event: Arc<dyn Fn(RunEvent) + Send + Sync>,
    pub permission: Arc<dyn Fn(ToolPermissionRequest) + Send + Sync>,
    pub graph_node: GraphSink,
    pub workspace: ResolveWorkspace,
}

/// C# `RunManager`'s `IRemoteRunner.StartAsync`: a client's run works in the shared folder alone, never in a worktree of its own and
/// never with a local style's tool permissions.
impl HostRunner for RunManager {
    fn start(&self, request: StartRunRequest) -> BoxFuture<'_, mighty_run::Result<()>> {
        Box::pin(async move {
            if request.working_folder.is_some()
                || request.delegation_worktree_root.is_some()
                || request.style_auto_allow.is_some()
                || request.pane_environment.is_some()
            {
                return Err(RunError::Invalid(get("wire.startRun.invalidFormat")));
            }
            RunManager::start(self, request).await
        })
    }
    fn stop(&self, session_id: String) -> BoxFuture<'_, mighty_run::Result<()>> {
        Box::pin(async move { RunManager::stop(self, &session_id).await })
    }
    fn respond_to_tool_permission(&self, session_id: &str, request_id: &str, allow: bool) -> mighty_run::Result<()> {
        RunManager::respond_to_tool_permission(self, session_id, request_id, allow)
    }
    fn answer_questionnaire(
        &self,
        session_id: &str,
        request_id: &str,
        answers: &IndexMap<String, UserQuestionAnswer>,
    ) -> mighty_run::Result<()> {
        RunManager::answer_questionnaire(self, session_id, request_id, answers)
    }
    fn answer_plan(&self, session_id: &str, request_id: &str, decision: &PlanDecision) -> mighty_run::Result<PlanRecord> {
        RunManager::answer_plan(self, session_id, request_id, decision)
    }
    fn dispose(&self) -> BoxFuture<'_, ()> {
        Box::pin(RunManager::dispose(self))
    }
}

/// Cancelled when a run stops or sharing ends, so a launch not begun yet never begins (C#'s `Job.Cancel`, whose token the checks' hold
/// at a launch's start sees).
#[derive(Clone, Default)]
pub struct Cancel(Arc<(AtomicBool, Notify)>);

impl Cancel {
    pub(crate) fn send_replace(&self, cancelled: bool) {
        if cancelled {
            self.0.0.store(true, Ordering::SeqCst);
            self.0.1.notify_waiters();
        }
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.0.load(Ordering::SeqCst)
    }
}

/// One client run. Everything that changes is read and written under the gate.
pub(crate) struct Job {
    /// Which job this is: a later run under the same id is another.
    pub serial: u64,
    /// Each event as it goes out, written once (a run holds its bytes, never a larger object tree), with what it costs a poll.
    pub events: std::collections::VecDeque<(i64, Arc<Vec<u8>>, usize)>,
    pub cursor: i64,
    pub bytes: usize,
    pub last_poll: Duration,
    pub done: bool,
    pub stopping: bool,
    pub finished_at: Option<Duration>,
    /// The run's tool requests still waiting: an answer reaches only one of these.
    pub pending: HashSet<String>,
    /// A Windows client asked for graph_run events instead of graph nodes.
    pub graph_runs: bool,
    pub cancel: Cancel,
    /// The event types left out because the Mac could not read them, each traced once.
    pub dropped: HashSet<String>,
    /// The temporary Antigravity rules this run holds, released when it ends.
    pub lease: Option<Lease>,
}

/// What `{"cursor":N,"event":…}` adds around an event in a poll, at most.
const EVENT_FRAME_BYTES: usize = 32;
/// More waiting requests than any runner keeps; one past this is never answerable from a client.
const MAXIMUM_PENDING_PER_RUN: usize = 64;
const REPORTED_RULES_KEPT: Duration = Duration::from_secs(24 * 60 * 60);
const MAXIMUM_REPORTED_CARDS: usize = 128;

fn content_type(request: &HttpRequest) -> &str {
    request.headers.get("content-type").map_or("", |t| t.split(';').next().unwrap_or_default().trim())
}

fn bad_request() -> HostFailure {
    HostFailure::new(400, get("remote.host.error.badRequest"))
}

fn run_limit() -> HostFailure {
    HostFailure::new(429, get_with("remote.host.error.runLimit", &[("count", &protocol::MAXIMUM_RUNS.to_string())]))
}

impl RemoteHost {
    pub(crate) fn sinks(&self, revision: u64) -> RunnerSinks {
        let (a, b, c, d) = (self.clone(), self.clone(), self.clone(), self.clone());
        RunnerSinks {
            event: Arc::new(move |value| a.receive(value, revision)),
            permission: Arc::new(move |value| b.receive_permission(value, revision)),
            graph_node: Arc::new(move |id, node| c.receive_graph(id, node, revision)),
            workspace: Arc::new(move |id: String| {
                let host = d.clone();
                Box::pin(async move {
                    host.resolve_shared(&id, revision).await.map_err(|failure| match failure {
                        Resolve::Host(f) => RunError::State(f.message),
                        Resolve::Refused(message) => RunError::State(message),
                    })
                })
            }),
        }
    }

    fn has_job(&self, id: &str) -> bool {
        lock(&self.0.gate).jobs.contains_key(id)
    }

    pub(crate) async fn serve_runs(
        &self,
        request: &HttpRequest,
        route: &[&str],
        query: &str,
        revision: u64,
    ) -> Result<HttpResponse, HostFailure> {
        if request.method == "POST" && route == ["runs"] && query.is_empty() {
            return self.start_run(request, revision).await;
        }
        let (id, action) = match route {
            ["runs", id, action] if wire::identifier(id) && self.has_job(id) => (*id, *action),
            _ => return Err(HostFailure::new(404, get("remote.host.error.runGone"))),
        };
        if request.method == "GET" && action == "events" {
            return self.events(id, query);
        }
        if request.method != "POST" || !query.is_empty() {
            return Err(HostFailure::new(404, get("remote.host.error.badPath")));
        }
        // As on the Mac, a stop reads no body.
        if action == "stop" {
            self.stop_job(id).await;
            return Ok(reply(200, &RemoteWireAnswer { protocol: protocol::VERSION, accepted: true }.to_value()));
        }
        if content_type(request) != "application/json" {
            return Err(HostFailure::new(415, get("remote.host.error.jsonOnly")));
        }
        let active = {
            let gate = lock(&self.0.gate);
            match (&gate.runner, gate.jobs.get(id)) {
                (Some(runner), Some(job)) if !job.done => runner.clone(),
                _ => return Err(HostFailure::new(409, get("remote.host.error.runGone"))),
            }
        };
        if request.body.is_empty() {
            return Err(bad_request());
        }
        let settled = get("permission.error.requestGone");
        let answered = match action {
            "steer" => {
                let steer = RemoteWireSteer::decode(&request.body).map_err(|_| bad_request())?;
                if steer.text.trim().is_empty() || steer.text.len() > 400_000 || steer.text.contains('\0') {
                    return Err(HostFailure::new(400, get("validation.run.badInput")));
                }
                // Since 0.4.35 nothing a person sends joins a running turn: refused, and an older client queues the text itself.
                return Ok(reply(200, &RemoteWireAnswer { protocol: protocol::VERSION, accepted: false }.to_value()));
            }
            "permission" => {
                let permission = RemoteWirePermission::decode(&request.body).map_err(|_| bad_request())?;
                self.answerable(id, &permission.run_id, &permission.request_id, &settled)?;
                active.respond_to_tool_permission(id, &permission.request_id, permission.allow)
            }
            "questions" => {
                let questions = RemoteWireQuestions::decode(&request.body).map_err(|_| bad_request())?;
                self.answerable(id, &questions.run_id, &questions.request_id, &settled)?;
                active.answer_questionnaire(id, &questions.request_id, &questions.answers)
            }
            "plan" => {
                let plan = RemoteWirePlan::decode(&request.body).map_err(|_| bad_request())?;
                let decision = plan.decision().ok_or_else(bad_request)?;
                self.answerable(id, &plan.run_id, &plan.request_id, &get("plan.error.settled"))?;
                active.answer_plan(id, &plan.request_id, &decision).map(|_| ())
            }
            _ => return Err(HostFailure::new(404, get("remote.host.error.badPath"))),
        };
        // What the runner refuses (settled, gone, not its kind) is a conflict, as on the Mac.
        answered.map_err(|error| HostFailure::new(409, error.message()))?;
        Ok(reply(200, &RemoteWireAnswer { protocol: protocol::VERSION, accepted: true }.to_value()))
    }

    /// An answer names this run and one of its requests still waiting; any other request is refused before the runner hears of it.
    fn answerable(&self, id: &str, run_id: &str, request_id: &str, settled: &str) -> Result<(), HostFailure> {
        if request_id.is_empty() || request_id.encode_utf16().count() > 256 {
            return Err(bad_request());
        }
        let gate = lock(&self.0.gate);
        match gate.jobs.get(id) {
            Some(job) if run_id == id && !job.done && job.pending.contains(request_id) => Ok(()),
            _ => Err(HostFailure::new(409, settled)),
        }
    }

    async fn start_run(&self, request: &HttpRequest, revision: u64) -> Result<HttpResponse, HostFailure> {
        let options = &self.0.options;
        if content_type(request) != "application/json" {
            return Err(HostFailure::new(415, get("remote.host.error.jsonOnly")));
        }
        if request.body.is_empty() {
            return Err(bad_request());
        }
        let start = RemoteWireStart::decode(&request.body).map_err(|_| bad_request())?;
        // The listener read up to 12 MiB for this route; past 512 KiB only a run that brings attachments.
        if request.body.len() > protocol::MAXIMUM_REQUEST_BYTES && start.request.attachments.is_none() {
            return Err(HostFailure::new(413, get("remote.run.error.tooLarge")));
        }
        if start.registered_models.len() > 256 {
            return Err(HostFailure::new(400, get("wire.startRun.invalidFormat")));
        }
        if start.registered_models.iter().take(64).any(|m| !wire::model(&m.name))
            || start.phase_models.as_ref().is_some_and(|phase| !validation::phase_models(phase))
        {
            return Err(bad_request());
        }
        let id = wire::new_id();
        let run = start.to_request(&id, &start.request.workspace_id).validate().map_err(|e| HostFailure::new(400, e.message()))?;
        // Only a shared workspace's own folder is ever the working folder; the client sends an id, never a path.
        match self.resolve_shared(&run.workspace_id, revision).await {
            Ok(_) => {}
            Err(Resolve::Host(failure)) => return Err(failure),
            Err(Resolve::Refused(message)) => return Err(HostFailure::new(403, message)),
        }
        let allowed = lock(&self.0.gate).allows_full_access;
        if let Some(refusal) = RemoteHost::full_access_refusal(&run, allowed, options.windows_host) {
            return Err(HostFailure::new(403, refusal));
        }
        // A Windows command line holds 32 767 characters: a longer command is refused here, never cut.
        if run.kind == "shell" && !self.command_fits(&run.input) {
            return Err(HostFailure::new(400, strings::host_command_too_long()));
        }
        // The cheap refusal before the costly checks below; checked again when the run is recorded.
        if lock(&self.0.gate).jobs.values().filter(|j| !j.done).count() >= protocol::MAXIMUM_RUNS {
            return Err(run_limit());
        }
        // This PC's own CLI update, plugin change or model reload holds client runs too.
        if let Some(reason) = (options.admission)(&run) {
            return Err(HostFailure::new(503, reason));
        }
        if run.kind == "claude" {
            // What this PC cannot run is refused here, before anything starts.
            let runtime = self.runtime().await?;
            let provider = runtime.providers.iter().find(|p| p.id == run.provider && p.available).ok_or_else(|| {
                HostFailure::new(400, get_with("remote.host.error.cliMissing", &[("name", providers::name(&run.provider))]))
            })?;
            if let Some(problem) = validation::capability_problem(&run, &provider.capabilities) {
                return Err(HostFailure::new(400, problem));
            }
        }
        // An allowed card's rules go in place before the run starts; refused, nothing runs.
        let mut lease = None;
        if let Some(grant) = start.antigravity_grant.clone().filter(|_| run.provider == "gemini" && run.kind == "claude") {
            let known = {
                let mut gate = lock(&self.0.gate);
                let now = (options.clock)();
                let known = gate.reported_rules.get(&grant.request_id).cloned();
                let valid = known.as_ref().is_some_and(|(rules, at)| {
                    grant.is_valid() && now.saturating_sub(*at) < REPORTED_RULES_KEPT && grant.rules.iter().all(|r| rules.contains(r))
                });
                if !valid {
                    return Err(HostFailure::new(403, get("remote.host.error.antigravityRules")));
                }
                // Kept for good only when this PC lets others run with full access.
                if !grant.temporary && !gate.allows_full_access {
                    return Err(HostFailure::new(403, get("remote.host.error.antigravityAlways")));
                }
                if !grant.keeps_only_allowed_rules() {
                    return Err(HostFailure::new(409, get("remote.error.antigravityOnceOnly")));
                }
                if options.antigravity_rules.is_none() {
                    return Err(HostFailure::new(409, get("remote.host.error.antigravityRules")));
                }
                // A card's rules are granted once: the first accepted grant uses them up.
                gate.reported_rules.remove(&grant.request_id);
                known
            };
            let rules = options.antigravity_rules.clone().unwrap_or_else(|| unreachable!("checked above"));
            let applying = grant.clone();
            match tokio::task::spawn_blocking(move || rules.apply(&applying)).await {
                Ok(Ok(held)) => lease = held,
                failed => {
                    // Nothing was added: the card may be granted again.
                    if let Some(known) = known {
                        lock(&self.0.gate).reported_rules.entry(grant.request_id.clone()).or_insert(known);
                    }
                    let message = match failed {
                        Ok(Err(mighty_run::antigravity::leases::ApplyError::Settings(error))) => error.message,
                        _ => get("remote.host.error.antigravityRules"),
                    };
                    return Err(HostFailure::new(409, message));
                }
            }
        }
        let recorded = {
            let mut gate = lock(&self.0.gate);
            let runner = gate.runner.clone();
            match runner {
                Some(runner) if gate.generation == revision => {
                    if gate.jobs.values().filter(|j| !j.done).count() >= protocol::MAXIMUM_RUNS {
                        Err(run_limit())
                    } else {
                        let now = (options.clock)();
                        prune(&mut gate.jobs, now);
                        let job = Job {
                            serial: self.0.serials.fetch_add(1, Ordering::SeqCst),
                            events: Default::default(),
                            cursor: 0,
                            bytes: 0,
                            last_poll: now,
                            done: false,
                            stopping: false,
                            finished_at: None,
                            pending: HashSet::new(),
                            graph_runs: self.0.options.graph_runs
                                && request.headers.get(protocol::GRAPH_HEADER).map(String::as_str) == Some(protocol::GRAPH_RUNS),
                            cancel: Cancel::default(),
                            dropped: HashSet::new(),
                            lease: lease.take(),
                        };
                        let serial = job.serial;
                        let cancel = job.cancel.clone();
                        gate.jobs.insert(id.clone(), job);
                        Ok((runner, serial, cancel))
                    }
                }
                _ => Err(HostFailure::new(503, get("remote.host.error.stopped"))),
            }
        };
        let (active, serial, cancel) = match recorded {
            Ok(recorded) => recorded,
            Err(failure) => {
                if let (Some(held), Some(rules)) = (lease.take(), &options.antigravity_rules) {
                    rules.release(&held);
                }
                return Err(failure);
            }
        };
        self.announce_runs();
        let host = self.clone();
        tokio::spawn(async move {
            if !cancel.is_cancelled() {
                host.launch(run, serial, cancel, active, revision).await;
            }
        });
        Ok(reply(202, &RemoteWireJob { protocol: protocol::VERSION, job_id: id }.to_value()))
    }

    /// Whether the command fits the command line its PowerShell gets; a system folder the runner cannot use is left for it to report.
    fn command_fits(&self, command: &str) -> bool {
        let system = self.0.options.system_folder.clone().unwrap_or_else(|| r"C:\Windows\System32".into());
        command_shell::fits(command, &system).unwrap_or(true)
    }

    /// `ResolveShared(id, revision)`: shared, local, an absolute path to a folder that exists, the very path shared. The runner looks its
    /// workspace up here again at launch.
    pub(crate) async fn resolve_shared(&self, id: &str, revision: u64) -> Result<Workspace, Resolve> {
        // Sharing stopped meanwhile: unavailable now (503), not a refusal of the workspace.
        if lock(&self.0.gate).generation != revision {
            return Err(Resolve::Host(HostFailure::new(503, get("remote.host.error.stopped"))));
        }
        let not_shared = || Resolve::Refused(get("remote.host.error.notShared"));
        let shared = self.shared_workspaces().into_iter().find(|w| w.id() == id).ok_or_else(not_shared)?;
        let folder = (self.0.options.resolve_local)(id.to_string()).await.map_err(Resolve::Refused)?;
        let path = std::path::Path::new(folder.path());
        if folder.id() != id || folder.remote.is_some() || !path.is_absolute() || folder.path() != shared.path() {
            return Err(not_shared());
        }
        // The disk is asked off the runtime's workers (a slow or gone network folder never parks one).
        let probe = folder.path().to_string();
        if !tokio::task::spawn_blocking(move || std::path::Path::new(&probe).is_dir()).await.unwrap_or(false) {
            return Err(not_shared());
        }
        Ok(folder)
    }

    /// Hands the run to the runner. A runner may refuse it at once (an error) or fail it later through its own events; the first is
    /// turned into those events here. A stop that came while the runner took the run stops it once it is there.
    async fn launch(&self, run: StartRunRequest, serial: u64, cancel: Cancel, active: Arc<dyn HostRunner>, revision: u64) {
        let id = run.session_id.clone();
        let hook = lock(&self.0.before_launch).clone();
        if let Some(hook) = hook {
            hook(cancel).await;
        }
        if !self.open(&id, serial, revision) || lock(&self.0.gate).jobs.get(&id).is_some_and(|j| j.stopping || j.cancel.is_cancelled()) {
            return;
        }
        if let Err(error) = active.start(run).await {
            let stopping = {
                let gate = lock(&self.0.gate);
                match gate.jobs.get(&id) {
                    Some(job) if gate.generation == revision && job.serial == serial && !job.done => job.stopping,
                    _ => return,
                }
            };
            self.receive(RunEvent::log(&id, "error", error.message(), None), revision);
            self.receive(RunEvent::state(&id, if stopping { "stopped" } else { "error" }, None), revision);
            return;
        }
        let late = {
            let gate = lock(&self.0.gate);
            gate.generation != revision || gate.jobs.get(&id).is_none_or(|j| j.serial != serial || j.stopping)
        };
        if late {
            let _ = active.stop(id).await;
        }
    }

    /// Under the gate: `serial` is still the run `id` of this sharing, and not ended.
    fn open(&self, id: &str, serial: u64, revision: u64) -> bool {
        let gate = lock(&self.0.gate);
        gate.generation == revision && gate.jobs.get(id).is_some_and(|j| j.serial == serial && !j.done)
    }

    // The runner's callbacks run on its tasks: each takes the gate briefly, never calls into the runner and never waits.

    fn receive(&self, value: RunEvent, revision: u64) {
        let Some(graph_runs) = self.following(&value.session_id, revision) else { return };
        // A client that did not ask for graph runs (the Mac's) gets graph nodes instead: left out on purpose.
        if value.kind == "graph_run" && !graph_runs {
            return;
        }
        if value.kind == "antigravityPermission"
            && let Some(card) = value.antigravity_permission.as_ref().filter(|card| !card.rules().is_empty())
        {
            self.remember(card.id.clone(), card.rules(), revision);
        }
        let ended = value.kind == "status" && matches!(value.status.as_deref(), Some("completed" | "error" | "stopped"));
        let wire = events::from_run_event(&value, &value.session_id, graph_runs);
        self.append(&value.session_id, &value.kind, wire, revision, ended);
    }

    /// A card a run of this sharing reported: its rules may come back as a client's grant, for a day (the newest 128 cards).
    fn remember(&self, card: String, rules: Vec<String>, revision: u64) {
        let now = (self.0.options.clock)();
        let mut gate = lock(&self.0.gate);
        if gate.generation != revision {
            return;
        }
        gate.reported_rules.retain(|_, (_, at)| now.saturating_sub(*at) < REPORTED_RULES_KEPT);
        while gate.reported_rules.len() >= MAXIMUM_REPORTED_CARDS {
            let Some(oldest) = gate.reported_rules.iter().min_by_key(|(_, (_, at))| *at).map(|(k, _)| k.clone()) else { break };
            gate.reported_rules.remove(&oldest);
        }
        gate.reported_rules.insert(card, (rules.into_iter().collect(), now));
    }

    fn receive_permission(&self, value: ToolPermissionRequest, revision: u64) {
        {
            let mut gate = lock(&self.0.gate);
            if gate.generation != revision {
                return;
            }
            let Some(job) = gate.jobs.get_mut(&value.run_id).filter(|j| !j.done) else { return };
            // Waiting before the client can see it, so its answer is never refused for coming too soon.
            if value.state != "pending" {
                job.pending.remove(&value.id);
            } else if job.pending.len() < MAXIMUM_PENDING_PER_RUN {
                job.pending.insert(value.id.clone());
            }
        }
        let wire = events::from_permission(&value, &value.run_id);
        self.append(&value.run_id, "permission", wire, revision, false);
    }

    fn receive_graph(&self, session_id: &str, node: Value, revision: u64) {
        // A Windows client that asked for graph runs gets those instead of nodes.
        if self.following(session_id, revision) != Some(false) {
            return;
        }
        let wire = ExecutionGraphNode::read(&node).ok().and_then(|node| events::from_graph_node(&node, session_id));
        self.append(session_id, "graph", wire, revision, false);
    }

    /// Whether the job's client asked for graph runs; None when no open job of this sharing has the id.
    fn following(&self, id: &str, revision: u64) -> Option<bool> {
        let gate = lock(&self.0.gate);
        if gate.generation != revision {
            return None;
        }
        gate.jobs.get(id).filter(|j| !j.done).map(|j| j.graph_runs)
    }

    /// One event into the job's buffer: the newest 4096 within 4 MiB are kept, one too large for any poll is never kept (its cursor shows
    /// as a gap), and nothing after the run's last status. None is an event the Mac could not read, left out.
    fn append(&self, job_id: &str, kind: &str, wire: Option<Map<String, Value>>, revision: u64, ended: bool) {
        let data = wire.map(|w| Arc::new(to_bytes(&Value::Object(w))));
        let mut lease = None;
        {
            let mut gate = lock(&self.0.gate);
            if gate.generation != revision {
                return;
            }
            let now = (self.0.options.clock)();
            let Some(job) = gate.jobs.get_mut(job_id).filter(|j| !j.done) else { return };
            match data {
                None => {
                    job.dropped.insert(kind.to_string());
                }
                Some(data) => {
                    job.cursor += 1;
                    let size = data.len() + EVENT_FRAME_BYTES;
                    if size <= protocol::MAXIMUM_POLL_BYTES - 4096 {
                        job.events.push_back((job.cursor, data, size));
                        job.bytes += size;
                        while job.events.len() > protocol::MAXIMUM_BUFFERED_EVENTS || job.bytes > protocol::MAXIMUM_BUFFERED_BYTES {
                            if let Some((_, _, dropped)) = job.events.pop_front() {
                                job.bytes -= dropped;
                            }
                        }
                    }
                }
            }
            if ended {
                job.done = true;
                job.finished_at = Some(now);
                job.pending.clear();
                lease = job.lease.take();
            }
        }
        // The run's temporary Antigravity rules leave agy's settings, and the count is told, from elsewhere: nothing may wait here.
        if let (Some(held), Some(rules)) = (lease, self.0.options.antigravity_rules.clone()) {
            spawn_blocking(move || rules.release(&held));
        }
        if ended {
            let host = self.clone();
            spawn_blocking(move || host.announce_runs());
        }
    }

    fn events(&self, id: &str, query: &str) -> Result<HttpResponse, HostFailure> {
        let bad_cursor = || HostFailure::new(400, get("remote.host.error.badCursor"));
        let requested: i64 = query
            .strip_prefix("?cursor=")
            .filter(|digits| (1..=12).contains(&digits.len()) && digits.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|digits| digits.parse().ok())
            .ok_or_else(bad_cursor)?;
        let (selected, last, newest, gap, done) = {
            let mut gate = lock(&self.0.gate);
            let now = (self.0.options.clock)();
            let job = gate.jobs.get_mut(id).ok_or_else(bad_cursor)?;
            if requested > job.cursor {
                return Err(bad_cursor());
            }
            job.last_poll = now;
            let mut selected = vec![];
            let mut bytes = 0;
            for (cursor, data, size) in &job.events {
                if *cursor <= requested {
                    continue;
                }
                if selected.len() >= protocol::MAXIMUM_POLL_EVENTS || bytes + size > protocol::MAXIMUM_POLL_BYTES {
                    break;
                }
                selected.push((*cursor, data.clone()));
                bytes += size;
            }
            // A cursor skipped between the request's and what follows: the buffer dropped it.
            let mut expected = requested + 1;
            let mut gap = false;
            for (cursor, _) in &selected {
                if *cursor != expected {
                    gap = true;
                }
                expected = cursor + 1;
            }
            // Nothing kept after the cursor although the run went on: all of it was dropped.
            if selected.is_empty() && job.cursor > requested {
                gap = true;
            }
            let last = selected.last().map(|(c, _)| *c).unwrap_or(if gap { job.cursor } else { requested });
            let done = job.done && last >= job.cursor;
            (selected, last, job.cursor, gap, done)
        };
        // Written outside the lock: an event's bytes never change once kept.
        let mut body = format!(
            "{{\"protocol\":{},\"cursor\":{last},\"lastCursor\":{newest},\"gap\":{gap},\"done\":{done},\"events\":[",
            protocol::VERSION
        )
        .into_bytes();
        for (i, (cursor, data)) in selected.iter().enumerate() {
            if i > 0 {
                body.push(b',');
            }
            body.extend_from_slice(format!("{{\"cursor\":{cursor},\"event\":").as_bytes());
            body.extend_from_slice(data);
            body.push(b'}');
        }
        body.extend_from_slice(b"]}");
        Ok(HttpResponse { status: 200, body, headers: vec![(protocol::HEADER.into(), "2".into())] })
    }

    /// Stops one run (the client's stop, or the heartbeat): the runner ends its process, and the run ends stopped if it said nothing.
    pub(crate) async fn stop_job(&self, id: &str) {
        let (active, revision, serial, cancel) = {
            let mut gate = lock(&self.0.gate);
            let runner = gate.runner.clone();
            let generation = gate.generation;
            let Some(job) = gate.jobs.get_mut(id).filter(|j| !j.done && !j.stopping) else { return };
            job.stopping = true;
            (runner, generation, job.serial, job.cancel.clone())
        };
        cancel.send_replace(true);
        if let Some(active) = active {
            let _ = active.stop(id.to_string()).await;
        }
        if self.open(id, serial, revision) {
            self.receive(RunEvent::state(id, "stopped", None), revision);
        }
    }

    /// The heartbeat, once a second: a listener that stopped for good ends the sharing, and a run whose client stopped polling is stopped.
    pub(crate) async fn lease_loop(&self, revision: u64, mut stopping: tokio::sync::watch::Receiver<bool>) {
        loop {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(1)) => {}
                _ = stopping.changed() => return,
            }
            if *stopping.borrow() || self.stop_if_listener_failed() {
                return;
            }
            let orphaned: Vec<String> = {
                let gate = lock(&self.0.gate);
                if gate.generation != revision {
                    return;
                }
                let now = (self.0.options.clock)();
                gate.jobs
                    .iter()
                    .filter(|(_, j)| !j.done && !j.stopping && now.saturating_sub(j.last_poll) >= self.0.options.heartbeat)
                    .map(|(id, _)| id.clone())
                    .collect()
            };
            for id in orphaned {
                self.stop_job(&id).await;
            }
            let mut gate = lock(&self.0.gate);
            if gate.generation == revision {
                let now = (self.0.options.clock)();
                prune(&mut gate.jobs, now);
            }
        }
    }

    /// A refusal as the route writes it (for the checks).
    pub fn refusal(status: u16, message: &str) -> HttpResponse {
        failure_response(status, message)
    }
}

/// Why `resolve_shared` refused: the host's own failure (sharing stopped, 503), or the workspace (the caller's status).
pub(crate) enum Resolve {
    Host(HostFailure),
    Refused(String),
}

/// Under the gate: finished runs older than two minutes go, and the oldest finished ones while there are 32 or more.
fn prune(jobs: &mut HashMap<String, Job>, now: Duration) {
    jobs.retain(|_, j| !(j.done && now.saturating_sub(j.finished_at.unwrap_or(now)) > Duration::from_secs(120)));
    while jobs.len() >= 2 * protocol::MAXIMUM_RUNS {
        let Some(oldest) = jobs.iter().filter(|(_, j)| j.done).min_by_key(|(_, j)| j.finished_at).map(|(id, _)| id.clone()) else { break };
        jobs.remove(&oldest);
    }
}

/// Work handed off the runner's task (C#'s thread pool): on the runtime when there is one, else at once.
fn spawn_blocking(work: impl FnOnce() + Send + 'static) {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            handle.spawn_blocking(work);
        }
        Err(_) => work(),
    }
}

/// C# `DesktopService.CreateRemoteRunner(sinks)`: the runner a sharing host's client runs go through: this PC's CLIs, looking workspaces
/// up only through the host's shared check, its tool requests to the host, Windows PowerShell for commands (from `system_folder`), its
/// graph nodes to the host, and no pane MCP (no agent I/O binding), no delegation and no picture cache.
pub fn host_runner(
    catalog: Arc<mighty_run::providers::ProviderCatalog>,
    plugin_directory: &str,
    sinks: RunnerSinks,
    system_folder: Option<String>,
) -> Arc<dyn HostRunner> {
    let event = sinks.event.clone();
    let mut options =
        mighty_run::manager::RunManagerOptions::new(plugin_directory, sinks.workspace.clone(), Arc::new(move |value| event(value)));
    let permission = sinks.permission.clone();
    options.permission_requested = Some(Arc::new(move |value| permission(value)));
    options.graph = Some(Arc::new(mighty_graph::TrackerFactory::default()));
    let graph = sinks.graph_node.clone();
    options.graph_nodes = Some(Arc::new(move |id: &str, node| graph(id, node)));
    options.powershell_commands = system_folder;
    Arc::new(RunManager::new(catalog, options))
}
