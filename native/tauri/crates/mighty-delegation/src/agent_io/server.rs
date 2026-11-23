//! The app's Agent IO server (C# Core/AgentIO/AgentIOPipe.cs:7-137): one listener per app, 4 waiting and 64 connections, one request
//! and one answer per connection, the token resolved by a fixed-time compare, 4 delegation calls in flight per token, the deadlines
//! (terminal and `open_url` 30 s, delegation 60 s once authenticated), the call dropped when its binding is revoked, the answers'
//! texts. The bytes come from mighty-platform's `AgentIoListener`. Owner: W7-C.
//!
//! The seams (W7 plan, "Seams"): `AgentIo::{start(listener), bindings(), bind(...), revoke(pane), clear()}` for src-tauri agent_io.rs and
//! W7-D (`EndAgentTerminal` revokes a child's binding). The two handlers' signatures are final from W7 Step 0.
//!
//! What a client sends that is not a request never ends a listener: a packet that does not parse is dropped with its connection, a
//! malformed delegation call is answered "malformed", and long delegation calls each hold only their own connection.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use mighty_platform::{AgentIoConnection, AgentIoListener, BoxFuture, McpTransport, PlatformError};
use serde_json::{Map, Value};
use tokio::sync::Semaphore;
use tokio::task::JoinHandle;
use tokio::time::Instant;

use super::binding::{AgentIoBinding, AgentIoBindings, InvalidBinding, Revocation};
use super::client::{DELEGATION_SECONDS, TERMINAL_SECONDS, response_bytes, workspace_response_bytes};
use super::tools::validate_in;
use super::wire::{self, Lenient};
use crate::mcp::workspace::{
    DETACHED_MESSAGE as WORKSPACE_DETACHED_MESSAGE, WorkspaceCaller, WorkspaceHandler, WorkspaceRequest, WorkspaceResponse,
};
use crate::request::{DETACHED_MESSAGE, DelegationCaller, DelegationRequest, DelegationResponse};

/// `IDelegationRequestHandler` (the coordinator implements it): defined beside the request it serves.
pub use crate::request::DelegationHandler;

/// `AgentIOPipe.UnknownTokenMessage`.
pub const UNKNOWN_TOKEN_MESSAGE: &str = "This agent pane is no longer connected to Mighty Code. Reopen the pane and try again.";
/// `AgentIOPipe.BusyMessage`.
pub const BUSY_MESSAGE: &str =
    "This pane already has as many delegation calls going as Mighty Code takes at once. Wait for one to answer and try again.";
/// `AgentIOPipe.MalformedMessage`.
pub const MALFORMED_MESSAGE: &str = "The request to Mighty Code was malformed.";
/// `AgentIOPipe.NoAnswerMessage`.
pub const NO_ANSWER_MESSAGE: &str = "Mighty Code did not answer the request.";
/// A terminal or web call that was refused or failed: no exception text, token, command or private path.
pub const TERMINAL_FAILURE: &str = "The tool request was refused or could not complete. The pane may have closed.";
/// `AgentIOPipe.WaitingListeners`, `Connections`, `DelegationCallsPerToken`.
pub const WAITING_LISTENERS: usize = 4;
pub const CONNECTIONS: usize = 64;
pub const DELEGATION_CALLS_PER_TOKEN: usize = 4;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// One terminal or web tool call once its token resolved to a live binding and its one argument passed `AgentIOTools.Validate` (C#
/// `handle(binding, tool, value, token)`).
#[derive(Clone)]
pub struct TerminalCall {
    /// The agent pane the binding belongs to.
    pub pane_id: String,
    pub workspace_id: String,
    pub workspace_path: String,
    pub provider: String,
    /// `run_in_terminal`, `read_latest_output`, `stop` or `open_url`.
    pub tool: String,
    /// Its one argument (the command, the handle or the URL).
    pub value: String,
    /// The binding the call came on (its revocation, `IsCurrent`).
    pub binding: Arc<AgentIoBinding>,
}

impl fmt::Debug for TerminalCall {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TerminalCall").field("pane_id", &self.pane_id).field("tool", &self.tool).finish_non_exhaustive()
    }
}

/// What serves the terminal and web tools (C# `MainWindow.HandleAgentIO`; src-tauri agent_io.rs over terminal.rs's `Terminals::agent`
/// and open_url's open in the default browser). The result is the tool's JSON (`AgentTerminalResult`, `{destination, url}`); an `Err` is answered with
/// the one refusal text and its detail never reaches the agent. The server drops the future at its deadline or when the binding is
/// revoked, which cancels the call.
pub trait TerminalHandler: Send + Sync {
    fn handle(&self, call: TerminalCall) -> BoxFuture<'static, Result<Value, String>>;
}

/// `AgentIOPipe`: the app's listener and the bindings its calls are checked against.
/// MC-38: a listener for an earlier app's transport name (Windows: its `mighty-agent-io-` pipe, which a new app does not reuse), so a
/// run that app kept running reaches this one; None when the platform cannot listen on it (the Mac's socket path stays the same).
pub type EarlierListener = Arc<dyn Fn(&str) -> Option<Arc<dyn AgentIoListener>> + Send + Sync>;

pub struct AgentIo {
    listener: Arc<dyn AgentIoListener>,
    /// MC-38: the earlier apps' names served as well, with their listeners.
    earlier: Mutex<Vec<(String, Arc<dyn AgentIoListener>)>>,
    earlier_listener: Mutex<Option<EarlierListener>>,
    transport: McpTransport,
    name: String,
    bindings: Arc<AgentIoBindings>,
    terminal: Arc<dyn TerminalHandler>,
    shutdown: Revocation,
    connections: Arc<Semaphore>,
    delegation_calls: Arc<Mutex<HashMap<String, usize>>>,
    listeners: Mutex<Vec<JoinHandle<()>>>,
    serving: Mutex<Vec<JoinHandle<()>>>,
}

impl AgentIo {
    /// Binds the listener and starts its 4 waiting listeners (on the current tokio runtime). Err when the OS refused the name.
    pub fn start(listener: Arc<dyn AgentIoListener>, terminal: Arc<dyn TerminalHandler>) -> Result<Arc<AgentIo>, PlatformError> {
        let name = listener.bind()?;
        let transport = listener.transport();
        let bindings = Arc::new(AgentIoBindings::new(transport));
        // Claude runs' config files go in the agent IO's own folder (the Unix socket's, 0700); what an app that ended without
        // deleting its runs' files left there goes now.
        let folder = super::mcp_config::folder_for(&name, transport == McpTransport::UnixSocket);
        if super::mcp_config::private_folder(&folder).is_ok() {
            super::mcp_config::sweep(&folder);
        }
        bindings.set_config_folder(folder);
        let io = Arc::new(AgentIo {
            listener: listener.clone(),
            earlier: Mutex::new(Vec::new()),
            earlier_listener: Mutex::new(None),
            transport,
            name,
            bindings,
            terminal,
            shutdown: Revocation::default(),
            connections: Arc::new(Semaphore::new(CONNECTIONS)),
            delegation_calls: Arc::new(Mutex::new(HashMap::new())),
            listeners: Mutex::new(vec![]),
            serving: Mutex::new(vec![]),
        });
        let tasks = (0..WAITING_LISTENERS).map(|_| tokio::spawn(io.clone().listen(listener.clone()))).collect();
        *lock(&io.listeners) = tasks;
        Ok(io)
    }

    /// `Name`: what a run's environment carries (`MIGHTY_AGENT_IO_PIPE`).
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn transport(&self) -> McpTransport {
        self.transport
    }

    /// `Bindings`.
    pub fn bindings(&self) -> &Arc<AgentIoBindings> {
        &self.bindings
    }

    /// `Bindings.Bind(pane, workspace, provider, Name, executable)`.
    pub fn bind(
        &self,
        pane: &str,
        workspace_id: &str,
        workspace_path: &str,
        provider: &str,
        executable: &str,
    ) -> Result<Arc<AgentIoBinding>, InvalidBinding> {
        self.bindings.bind(pane, workspace_id, workspace_path, provider, &self.name, executable)
    }

    /// MC-38: how this platform listens on an earlier app's name ([`EarlierListener`]).
    pub fn set_earlier_listener(&self, listener: Option<EarlierListener>) {
        *lock(&self.earlier_listener) = listener;
    }

    /// MC-38: the pane's binding again for a run an earlier app kept running in the background daemon ([`AgentIoBindings::adopt`]):
    /// the run's token and its transport's name as that app left them. A name other than this listener's is served as well when the
    /// platform can listen on it ([`AgentIo::set_earlier_listener`]; refused otherwise: the run's MCP servers could not reach this app).
    pub fn adopt(
        self: &Arc<Self>,
        pane: &str,
        workspace_id: &str,
        workspace_path: &str,
        provider: &str,
        executable: &str,
        kept: &Value,
    ) -> Result<Arc<AgentIoBinding>, InvalidBinding> {
        let pipe = kept.get("pipe").and_then(Value::as_str).filter(|pipe| super::client::is_pipe_name(pipe, self.transport));
        let Some(pipe) = pipe else { return Err(InvalidBinding) };
        if pipe != self.name && !self.serve_earlier(pipe) {
            return Err(InvalidBinding);
        }
        self.bindings.adopt(pane, workspace_id, workspace_path, provider, pipe, executable, kept)
    }

    /// `name` served as well (once), on a listener of the platform's; false when it cannot be.
    fn serve_earlier(self: &Arc<Self>, name: &str) -> bool {
        if self.shutdown.is_cancelled() {
            return false;
        }
        if lock(&self.earlier).iter().any(|(served, _)| served == name) {
            return true;
        }
        let Some(make) = lock(&self.earlier_listener).clone() else { return false };
        let Some(listener) = make(name) else { return false };
        if listener.bind().ok().as_deref() != Some(name) || listener.transport() != self.transport {
            listener.close();
            return false;
        }
        lock(&self.earlier).push((name.to_string(), listener.clone()));
        let tasks: Vec<JoinHandle<()>> = (0..WAITING_LISTENERS).map(|_| tokio::spawn(self.clone().listen(listener.clone()))).collect();
        lock(&self.listeners).extend(tasks);
        true
    }

    /// `Bindings.Revoke(pane)`: its calls waiting on the terminal end, no new one is taken.
    pub fn revoke(&self, pane: &str) {
        self.bindings.revoke(pane);
    }

    /// `Bindings.Clear()`.
    pub fn clear(&self) {
        self.bindings.clear();
    }

    /// `DisposeAsync`: every binding revoked, the listeners stopped, the connections being served let finish.
    pub async fn close(&self) {
        self.bindings.clear();
        self.shutdown.cancel();
        self.listener.close();
        for (_, listener) in lock(&self.earlier).drain(..) {
            listener.close();
        }
        let listeners: Vec<_> = lock(&self.listeners).drain(..).collect();
        for task in listeners {
            let _ = task.await;
        }
        let serving: Vec<_> = lock(&self.serving).drain(..).collect();
        for task in serving {
            let _ = task.await;
        }
    }

    /// `Listen`: waits for clients one at a time, each on a connection slot of its own, and serves each on its own task.
    async fn listen(self: Arc<Self>, listener: Arc<dyn AgentIoListener>) {
        while !self.shutdown.is_cancelled() {
            let permit = tokio::select! {
                permit = self.connections.clone().acquire_owned() => match permit { Ok(permit) => permit, Err(_) => return },
                _ = self.shutdown.cancelled() => return,
            };
            let accepted = tokio::select! {
                accepted = listener.accept() => accepted,
                _ = self.shutdown.cancelled() => return,
            };
            match accepted {
                Ok(connection) => {
                    let io = self.clone();
                    let served = tokio::spawn(async move {
                        let _permit = permit;
                        io.serve(connection).await;
                    });
                    let mut serving = lock(&self.serving);
                    serving.retain(|task| !task.is_finished());
                    serving.push(served);
                }
                Err(_) => {
                    drop(permit);
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_millis(100)) => {}
                        _ = self.shutdown.cancelled() => return,
                    }
                }
            }
        }
    }

    /// `Serve` and `Answer`: one request and its answer within the deadline (30 s, moved on for an authenticated delegation call).
    /// Past the deadline, or once the app quits, nothing is written.
    async fn serve(&self, mut connection: AgentIoConnection) {
        let mut deadline = Instant::now() + Duration::from_secs(TERMINAL_SECONDS);
        let read = tokio::select! {
            read = tokio::time::timeout_at(deadline, wire::read_packet(&mut connection, self.transport)) => read,
            _ = self.shutdown.cancelled() => return,
        };
        let Ok(Ok(packet)) = read else { return };
        let Some(document) = Lenient::parse_bytes(&packet) else { return };
        let answer = match &document.value {
            Value::Object(request) if request.contains_key("delegation") => {
                let call = request.get("delegation").cloned().unwrap_or(Value::Null);
                self.answer_delegation(&document, request, &call, &mut deadline).await.map(|response| response_bytes(&response))
            }
            Value::Object(request) if request.contains_key("workspace") => {
                let call = request.get("workspace").cloned().unwrap_or(Value::Null);
                self.answer_workspace(&document, request, &call, deadline).await.map(|response| workspace_response_bytes(&response))
            }
            request => self.answer_terminal(&document, request, deadline).await,
        };
        let Some(answer) = answer else { return };
        if Instant::now() >= deadline {
            return;
        }
        let _ = tokio::time::timeout_at(deadline, wire::write_packet(&mut connection, &answer, self.transport)).await;
    }

    /// A terminal or web call: `{"ok":true,"result":…}` or the one failure; None once the app quits.
    async fn answer_terminal(&self, document: &Lenient, request: &Value, deadline: Instant) -> Option<Vec<u8>> {
        let empty = Map::new();
        let map = request.as_object().unwrap_or(&empty);
        let outcome: Result<Value, ()> = async {
            let token = document.text(map.get("token")).map_err(|_| ())?.unwrap_or("");
            let binding = self.bindings.resolve(token).ok_or(())?;
            let tool = document.text(map.get("tool")).map_err(|_| ())?.unwrap_or("");
            let arguments = request.as_object().and_then(|map| map.get("arguments")).ok_or(())?;
            let value = validate_in(document, tool, arguments).map_err(|_| ())?;
            if binding.is_revoked() {
                return Err(());
            }
            let call = TerminalCall {
                pane_id: binding.pane_id.clone(),
                workspace_id: binding.workspace_id.clone(),
                workspace_path: binding.workspace_path.clone(),
                provider: binding.provider.clone(),
                tool: tool.to_string(),
                value,
                binding: binding.clone(),
            };
            let handled = self.terminal.handle(call);
            tokio::select! {
                result = tokio::time::timeout_at(deadline, handled) => result.map_err(|_| ())?.map_err(|_| ()),
                _ = binding.revoked().cancelled() => Err(()),
            }
        }
        .await;
        if self.shutdown.is_cancelled() {
            return None;
        }
        let mut out = mighty_core::json::JsonOut::new();
        out.begin_object();
        match outcome {
            Ok(result) => {
                out.prop_bool("ok", true);
                out.name("result");
                out.value(&result);
            }
            Err(()) => {
                out.prop_bool("ok", false);
                out.prop_str("error", Some(TERMINAL_FAILURE));
            }
        }
        out.end_object();
        Some(out.finish().into_bytes())
    }

    /// `AnswerDelegation`: a delegation call, from whatever pane its token belongs to; None once the app quits.
    async fn answer_delegation(
        &self,
        document: &Lenient,
        packet: &Map<String, Value>,
        call: &Value,
        deadline: &mut Instant,
    ) -> Option<DelegationResponse> {
        self.answer_delegation_with_timeout(document, packet, call, deadline, Duration::from_secs(DELEGATION_SECONDS)).await
    }

    async fn answer_delegation_with_timeout(
        &self,
        document: &Lenient,
        packet: &Map<String, Value>,
        call: &Value,
        deadline: &mut Instant,
        timeout: Duration,
    ) -> Option<DelegationResponse> {
        // A token or an argument that is not text (a lone surrogate among them) is a malformed call.
        let Ok(token) = document.text(packet.get("token")).map(|token| token.unwrap_or("").to_string()) else {
            return Some(DelegationResponse::failure(MALFORMED_MESSAGE));
        };
        let Some(request) = read_request(document, call) else { return Some(DelegationResponse::failure(MALFORMED_MESSAGE)) };
        let Some(binding) = self.bindings.resolve(&token) else { return Some(DelegationResponse::failure(UNKNOWN_TOKEN_MESSAGE)) };
        let Some(handler) = binding.delegation_handler() else { return Some(DelegationResponse::failure(DETACHED_MESSAGE)) };
        {
            let mut calls = lock(&self.delegation_calls);
            let count = calls.entry(token.clone()).or_insert(0);
            if *count >= DELEGATION_CALLS_PER_TOKEN {
                return Some(DelegationResponse::failure(BUSY_MESSAGE));
            }
            *count += 1;
        }
        let counted = Counted { calls: self.delegation_calls.clone(), token };
        *deadline = Instant::now() + timeout;
        let caller = DelegationCaller {
            pane_id: binding.pane_id.clone(),
            kind: binding.kind().to_string(),
            provider: binding.provider.clone(),
            delegation: binding.delegation(),
            permission_mode: binding.permission_mode(),
        };
        // On its own task: a handler that panics is one that did not answer, and one past the deadline keeps holding its
        // per-token concurrency slot until the spawned handler finishes instead of releasing it prematurely on timeout.
        let handled = tokio::spawn(async move {
            let _counted = counted;
            handler.handle(request, caller).await
        });
        tokio::select! {
            answered = tokio::time::timeout_at(*deadline, handled) => Some(match answered {
                Ok(Ok(response)) => response,
                _ => DelegationResponse::failure(NO_ANSWER_MESSAGE),
            }),
            _ = self.shutdown.cancelled() => None,
        }
    }

    async fn answer_workspace(
        &self,
        document: &Lenient,
        packet: &Map<String, Value>,
        call: &Value,
        deadline: Instant,
    ) -> Option<WorkspaceResponse> {
        let Ok(token) = document.text(packet.get("token")).map(|token| token.unwrap_or("").to_string()) else {
            return Some(WorkspaceResponse::failure(MALFORMED_MESSAGE));
        };
        let Some(request) = read_workspace_request(document, call) else {
            return Some(WorkspaceResponse::failure(MALFORMED_MESSAGE));
        };
        let Some(binding) = self.bindings.resolve(&token) else {
            return Some(WorkspaceResponse::failure(UNKNOWN_TOKEN_MESSAGE));
        };
        if binding.is_revoked() || !binding.workspace() {
            return Some(WorkspaceResponse::failure(WORKSPACE_DETACHED_MESSAGE));
        }
        let Some(handler) = binding.workspace_handler() else {
            return Some(WorkspaceResponse::failure(WORKSPACE_DETACHED_MESSAGE));
        };
        let caller = WorkspaceCaller {
            pane_id: binding.pane_id.clone(),
            workspace_id: binding.workspace_id.clone(),
            workspace_path: binding.workspace_path.clone(),
            provider: binding.provider.clone(),
        };
        let handled = tokio::spawn(async move { handler.handle(request, caller).await });
        tokio::select! {
            answered = tokio::time::timeout_at(deadline, handled) => Some(match answered {
                Ok(Ok(response)) => response,
                _ => WorkspaceResponse::failure(NO_ANSWER_MESSAGE),
            }),
            _ = binding.revoked().cancelled() => Some(WorkspaceResponse::failure(UNKNOWN_TOKEN_MESSAGE)),
            _ = self.shutdown.cancelled() => None,
        }
    }
}

/// What the run path reads of cross-workspace lookup as it binds a run: who answers `mighty-workspace` calls and whether the switch is on.
pub type WorkspaceLink = Arc<dyn Fn() -> (Option<Arc<dyn WorkspaceHandler>>, bool) + Send + Sync>;

/// What the run path reads of delegation as it binds a run (C# `RunManager.DelegationHandler` and `DelegationEnabled()`): who answers
/// the run's delegation calls (None while delegation is not running), and whether the switch is on and delegation may act.
pub type DelegationLink = Arc<dyn Fn() -> (Option<Arc<dyn DelegationHandler>>, bool) + Send + Sync>;

/// The run service's binding factory (C# `ConfigureAgentIO`'s factory and RunManager.cs:374-380): a Claude or Codex run of a local
/// workspace gets a new binding of its pane on `io` — `MIGHTY_PANE_TOKEN` and the pipe's name in its environment, `--mcp-config` or
/// Codex's `-c` list, the token redacted from every line it reads —, connected to delegation as `delegation` says: attached when the
/// run's kind, agent and mode take the server (`AttachesDelegation`) and the switch is on, judged by the mode it starts in (the plan
/// override, else its settings'). A remote workspace's run gets none (its tools would work in this PC's processes), nor does one the
/// registry refuses (an id that is not one, a path that is not full).
pub fn binding_factory(io: Arc<AgentIo>, executable: String, delegation: DelegationLink) -> mighty_run::manager::BindingFactory {
    logged_binding_factory(io, executable, delegation, Arc::new(|_: &str| {}))
}

/// [`binding_factory`] that says in `log` why a run got no binding when the registry refused it (the pane's id and the reason; never
/// a token or a path).
pub fn logged_binding_factory(
    io: Arc<AgentIo>,
    executable: String,
    delegation: DelegationLink,
    log: Arc<dyn Fn(&str) + Send + Sync>,
) -> mighty_run::manager::BindingFactory {
    logged_binding_factory_with_workspace(io, executable, delegation, Arc::new(|| (None, false)), log)
}

/// [`logged_binding_factory`] that also connects the optional read-only `mighty-workspace` MCP server when `workspace` says so.
pub fn logged_binding_factory_with_workspace(
    io: Arc<AgentIo>,
    executable: String,
    delegation: DelegationLink,
    workspace_link: WorkspaceLink,
    log: Arc<dyn Fn(&str) + Send + Sync>,
) -> mighty_run::manager::BindingFactory {
    Arc::new(move |request: &mighty_run::model::StartRunRequest, workspace: &mighty_core::model::Workspace| {
        if workspace.remote.is_some() || workspace.path().is_empty() || !matches!(request.provider.as_str(), "claude" | "codex") {
            return None;
        }
        // MC-38: a run taken back from the background daemon keeps the binding its CLI was started with, or has none.
        let kept = request.kept.as_ref().map(|kept| kept.part("binding")).filter(|kept| !kept.is_null());
        if request.adopted && kept.is_none() {
            return None;
        }
        let bound = match (request.adopted, kept) {
            (true, Some(kept)) => io.adopt(&request.session_id, workspace.id(), workspace.path(), &request.provider, &executable, kept),
            _ => io.bind(&request.session_id, workspace.id(), workspace.path(), &request.provider, &executable),
        };
        let binding = match bound {
            Ok(binding) => binding,
            Err(refused) => {
                let pane: String = request.session_id.chars().take(128).collect();
                log(&format!("agent io: the run of pane {pane:?} gets no pane tools: {refused}"));
                return None;
            }
        };
        let (handler, enabled) = delegation();
        let settings = request.settings();
        let mode = request.permission_mode_override.clone().unwrap_or(settings.permission_mode);
        let attached = super::binding::attaches_delegation(&request.kind, &request.provider, &mode) && enabled;
        binding.connect_delegation(handler, attached, &mode);
        let (ws_handler, ws_enabled) = workspace_link();
        binding.connect_workspace(ws_handler, ws_enabled && request.kind == "claude");
        Some(if request.adopted { binding.adopted_run_binding() } else { binding.run_binding() })
    })
}

/// One delegation call counted against its token until its spawned handler task finishes.
struct Counted {
    calls: Arc<Mutex<HashMap<String, usize>>>,
    token: String,
}

impl Drop for Counted {
    fn drop(&mut self) {
        let mut calls = lock(&self.calls);
        if let Some(count) = calls.get_mut(&self.token) {
            *count -= 1;
            if *count == 0 {
                calls.remove(&self.token);
            }
        }
    }
}

/// `call.Deserialize<DelegationRequest>(DelegationJson.Options)` and the null-argument check: an object whose `tool` (any case) is a
/// string and whose `arguments` is an object of strings, both present; None for anything else (a name or text it cannot read
/// included).
fn read_request(document: &Lenient, call: &Value) -> Option<DelegationRequest> {
    let Value::Object(map) = call else { return None };
    document.keys(map).ok()?;
    let member = |name: &str| map.iter().filter(|(key, _)| key.eq_ignore_ascii_case(name)).map(|(_, value)| value).next_back();
    let tool = match member("tool")? {
        Value::String(tool) => document.read(tool).ok()?.to_string(),
        _ => return None,
    };
    let Value::Object(arguments) = member("arguments")? else { return None };
    document.keys(arguments).ok()?;
    let mut values = indexmap::IndexMap::new();
    for (key, value) in arguments {
        let Value::String(text) = value else { return None };
        values.insert(key.clone(), document.read(text).ok()?.to_string());
    }
    Some(DelegationRequest { tool, arguments: values })
}

fn read_workspace_request(document: &Lenient, call: &Value) -> Option<WorkspaceRequest> {
    let parsed = read_request(document, call)?;
    Some(WorkspaceRequest { tool: parsed.tool, arguments: parsed.arguments })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_delegation_call_reads_as_csharp_reads_it() {
        let read = |text: &str| {
            let document = Lenient::parse(text).unwrap();
            read_request(&document, &document.value)
        };
        let request = read(r#"{"Tool":"merge","ARGUMENTS":{"child":"c","expected_head":"h"},"extra":1}"#).unwrap();
        assert_eq!(request.tool, "merge");
        assert_eq!(request.arguments.keys().collect::<Vec<_>>(), ["child", "expected_head"]);
        for malformed in [
            r#"{"tool":5,"arguments":{}}"#,
            r#"{"tool":"x"}"#,
            r#"{"arguments":{}}"#,
            r#"{"tool":null,"arguments":{}}"#,
            r#"{"tool":"x","arguments":null}"#,
            r#"{"tool":"x","arguments":{"a":null}}"#,
            r#"{"tool":"x","arguments":{"a":1}}"#,
            r#"{"tool":"\ud800","arguments":{}}"#,
            r#"{"tool":"x","arguments":{"a":"\udc00"}}"#,
            r#"[]"#,
            r#"null"#,
        ] {
            assert_eq!(read(malformed), None, "{malformed}");
        }
    }

    /// P0-CORE-2: `permission_mode_override` governs whether delegation is attached (not the un-overridden `settings.permission_mode`).
    #[tokio::test]
    async fn permission_mode_override_governs_delegation_attachment() {
        use mighty_core::model::{RunSettings, Workspace};
        use mighty_run::model::StartRunRequest;

        struct NoTerminal;
        impl TerminalHandler for NoTerminal {
            fn handle(&self, _call: TerminalCall) -> BoxFuture<'static, Result<Value, String>> {
                Box::pin(async { Err(String::new()) })
            }
        }
        struct OkDelegation;
        impl DelegationHandler for OkDelegation {
            fn handle(&self, _request: DelegationRequest, _caller: DelegationCaller) -> BoxFuture<'static, DelegationResponse> {
                Box::pin(async { DelegationResponse::failure("unused") })
            }
        }

        let pipe = AgentIo::start(Arc::new(mighty_platform::fake::FakeAgentIo::default()), Arc::new(NoTerminal)).unwrap();
        let handler: Arc<dyn DelegationHandler> = Arc::new(OkDelegation);
        let exe = if cfg!(windows) { r"C:\mighty\app.exe" } else { "/mighty/app" }.to_string();
        let workspace =
            Workspace { id: Some("ws".into()), path: Some(std::env::temp_dir().to_string_lossy().into_owned()), ..Workspace::default() };
        let factory = binding_factory(pipe.clone(), exe, Arc::new(move || (Some(handler.clone()), true)));
        let delegation_key = format!("mcp_servers.{}.", crate::mcp::delegation::SERVER_NAME);

        // Widening override: Codex pane stored as `manual` (no delegation), overridden to `acceptEdits` -> delegation IS attached.
        let mut widened = StartRunRequest::new("codex-widened", "ws", "claude", "hi");
        widened.provider = "codex".into();
        widened.settings = Some(RunSettings { permission_mode: "manual".into(), ..RunSettings::default() });
        widened.permission_mode_override = Some("acceptEdits".into());
        let widened_binding = factory(&widened, &workspace).expect("bound");
        let token = widened_binding.environment.get(super::super::binding::TOKEN_KEY).unwrap();
        let bound = pipe.bindings().resolve(token).unwrap();
        assert!(bound.delegation(), "override to acceptEdits must attach delegation");
        assert_eq!(bound.permission_mode(), "acceptEdits");
        assert!(
            widened_binding.arguments.iter().any(|arg| arg.contains(&delegation_key)),
            "delegation server must be passed to Codex CLI when override attaches delegation"
        );

        // Narrowing override: Codex pane stored as `fullAccess`, overridden to `manual` -> delegation is NOT attached.
        let mut narrowed = StartRunRequest::new("codex-narrowed", "ws", "claude", "hi");
        narrowed.provider = "codex".into();
        narrowed.settings = Some(RunSettings { permission_mode: "fullAccess".into(), ..RunSettings::default() });
        narrowed.permission_mode_override = Some("manual".into());
        let narrowed_binding = factory(&narrowed, &workspace).expect("bound");
        let token = narrowed_binding.environment.get(super::super::binding::TOKEN_KEY).unwrap();
        let bound = pipe.bindings().resolve(token).unwrap();
        assert!(!bound.delegation(), "override to manual must detach delegation");
        assert_eq!(bound.permission_mode(), "manual");
        assert!(
            narrowed_binding.arguments.iter().all(|arg| !arg.contains(&delegation_key)),
            "delegation server must not be passed when override detaches delegation"
        );

        pipe.close().await;
    }

    /// P1-SEC-1: timed-out delegation calls must keep holding their `DELEGATION_CALLS_PER_TOKEN` slot until the spawned handler
    /// task actually finishes, rather than releasing the slot prematurely on timeout.
    #[tokio::test]
    async fn timed_out_delegation_calls_hold_slots_until_handler_finishes() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::sync::Notify;

        struct NoTerminal;
        impl TerminalHandler for NoTerminal {
            fn handle(&self, _call: TerminalCall) -> BoxFuture<'static, Result<Value, String>> {
                Box::pin(async { Err("unused".into()) })
            }
        }
        struct StuckDelegation {
            entered: AtomicUsize,
            release: Arc<Notify>,
        }
        impl DelegationHandler for StuckDelegation {
            fn handle(&self, _request: DelegationRequest, _caller: DelegationCaller) -> BoxFuture<'static, DelegationResponse> {
                self.entered.fetch_add(1, Ordering::SeqCst);
                let release = self.release.clone();
                Box::pin(async move {
                    release.notified().await;
                    DelegationResponse::failure("finished")
                })
            }
        }

        let pipe = AgentIo::start(Arc::new(mighty_platform::fake::FakeAgentIo::default()), Arc::new(NoTerminal)).unwrap();
        let release = Arc::new(Notify::new());
        let handler = Arc::new(StuckDelegation { entered: AtomicUsize::new(0), release: release.clone() });
        let exe = if cfg!(windows) { r"C:\mighty\app.exe" } else { "/mighty/app" }.to_string();
        let binding = pipe.bind("pane-timeout", "ws", &std::env::temp_dir().to_string_lossy(), "claude", &exe).unwrap();
        binding.connect_delegation(Some(handler.clone()), true, "fullAccess");
        let token = binding.environment()[super::super::binding::TOKEN_KEY].clone();

        let make_packet = |token: &str| {
            let bytes = format!(r#"{{"token":"{token}","delegation":{{"tool":"list_children","arguments":{{}}}}}}"#);
            Lenient::parse_bytes(bytes.as_bytes()).unwrap()
        };

        // Start DELEGATION_CALLS_PER_TOKEN calls with a short timeout while their spawned tasks remain blocked.
        let mut futures = Vec::new();
        for _ in 0..DELEGATION_CALLS_PER_TOKEN {
            let pipe = pipe.clone();
            let token = token.clone();
            futures.push(tokio::spawn(async move {
                let document = make_packet(&token);
                let packet = document.value.as_object().unwrap();
                let call = packet.get("delegation").unwrap();
                let mut deadline = Instant::now() + Duration::from_secs(TERMINAL_SECONDS);
                pipe.answer_delegation_with_timeout(&document, packet, call, &mut deadline, Duration::from_millis(40)).await
            }));
        }
        for fut in futures {
            assert_eq!(fut.await.unwrap(), Some(DelegationResponse::failure(NO_ANSWER_MESSAGE)));
        }
        assert_eq!(handler.entered.load(Ordering::SeqCst), DELEGATION_CALLS_PER_TOKEN);

        // Even after timeout, all slots are still held by the unfinished tasks, so another call must get BUSY_MESSAGE.
        let document = make_packet(&token);
        let packet = document.value.as_object().unwrap();
        let call = packet.get("delegation").unwrap();
        let mut deadline = Instant::now() + Duration::from_secs(TERMINAL_SECONDS);
        assert_eq!(
            pipe.answer_delegation_with_timeout(&document, packet, call, &mut deadline, Duration::from_millis(40)).await,
            Some(DelegationResponse::failure(BUSY_MESSAGE)),
            "timed-out tasks must continue holding their concurrency slots until finished"
        );

        // Once the blocked tasks finish, the slots are released and a subsequent call proceeds normally.
        release.notify_waiters();
        let wait_limit = Instant::now() + Duration::from_secs(2);
        while !lock(&pipe.delegation_calls).is_empty() && Instant::now() < wait_limit {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(lock(&pipe.delegation_calls).is_empty(), "slots must be released once spawned handlers finish");

        let pipe2 = pipe.clone();
        let token2 = token.clone();
        let next = tokio::spawn(async move {
            let document = make_packet(&token2);
            let packet = document.value.as_object().unwrap();
            let call = packet.get("delegation").unwrap();
            let mut deadline = Instant::now() + Duration::from_secs(TERMINAL_SECONDS);
            pipe2.answer_delegation_with_timeout(&document, packet, call, &mut deadline, Duration::from_secs(2)).await
        });
        while handler.entered.load(Ordering::SeqCst) < DELEGATION_CALLS_PER_TOKEN + 1 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        release.notify_waiters();
        assert_eq!(next.await.unwrap(), Some(DelegationResponse::failure("finished")));

        pipe.close().await;
    }
}
