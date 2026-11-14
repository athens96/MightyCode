//! The app's Agent IO server (C# Core/AgentIO/AgentIOPipe.cs:7-137): one listener per app, 4 waiting and 64 connections, one request
//! and one answer per connection, the token resolved by a fixed-time compare, 4 delegation calls in flight per token, the deadlines
//! (terminal 30 s, `open_url` 60 s once authenticated, delegation 60 s), the call dropped when its binding is revoked, the answers'
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
use super::client::{DELEGATION_SECONDS, OPEN_URL_SECONDS, TERMINAL_SECONDS, response_bytes};
use super::tools::validate_in;
use super::wire::{self, Lenient};
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
/// and the open_url prompt). The result is the tool's JSON (`AgentTerminalResult`, `{destination, url, …}`); an `Err` is answered with
/// the one refusal text and its detail never reaches the agent. The server drops the future at its deadline or when the binding is
/// revoked, which cancels the call.
pub trait TerminalHandler: Send + Sync {
    fn handle(&self, call: TerminalCall) -> BoxFuture<'static, Result<Value, String>>;
}

/// `AgentIOPipe`: the app's listener and the bindings its calls are checked against.
pub struct AgentIo {
    listener: Arc<dyn AgentIoListener>,
    transport: McpTransport,
    name: String,
    bindings: Arc<AgentIoBindings>,
    terminal: Arc<dyn TerminalHandler>,
    shutdown: Revocation,
    connections: Arc<Semaphore>,
    delegation_calls: Mutex<HashMap<String, usize>>,
    listeners: Mutex<Vec<JoinHandle<()>>>,
    serving: Mutex<Vec<JoinHandle<()>>>,
}

impl AgentIo {
    /// Binds the listener and starts its 4 waiting listeners (on the current tokio runtime). Err when the OS refused the name.
    pub fn start(listener: Arc<dyn AgentIoListener>, terminal: Arc<dyn TerminalHandler>) -> Result<Arc<AgentIo>, PlatformError> {
        let name = listener.bind()?;
        let transport = listener.transport();
        let io = Arc::new(AgentIo {
            listener,
            transport,
            name,
            bindings: Arc::new(AgentIoBindings::new(transport)),
            terminal,
            shutdown: Revocation::default(),
            connections: Arc::new(Semaphore::new(CONNECTIONS)),
            delegation_calls: Mutex::new(HashMap::new()),
            listeners: Mutex::new(vec![]),
            serving: Mutex::new(vec![]),
        });
        let tasks = (0..WAITING_LISTENERS).map(|_| tokio::spawn(io.clone().listen())).collect();
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
    async fn listen(self: Arc<Self>) {
        while !self.shutdown.is_cancelled() {
            let permit = tokio::select! {
                permit = self.connections.clone().acquire_owned() => match permit { Ok(permit) => permit, Err(_) => return },
                _ = self.shutdown.cancelled() => return,
            };
            let accepted = tokio::select! {
                accepted = self.listener.accept() => accepted,
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

    /// `Serve` and `Answer`: one request and its answer within the deadline (30 s, moved on for an authenticated `open_url` or
    /// delegation call). Past the deadline, or once the app quits, nothing is written.
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
            request => self.answer_terminal(&document, request, &mut deadline).await,
        };
        let Some(answer) = answer else { return };
        if Instant::now() >= deadline {
            return;
        }
        let _ = tokio::time::timeout_at(deadline, wire::write_packet(&mut connection, &answer, self.transport)).await;
    }

    /// A terminal or web call: `{"ok":true,"result":…}` or the one failure; None once the app quits.
    async fn answer_terminal(&self, document: &Lenient, request: &Value, deadline: &mut Instant) -> Option<Vec<u8>> {
        let empty = Map::new();
        let map = request.as_object().unwrap_or(&empty);
        let outcome: Result<Value, ()> = async {
            let token = document.text(map.get("token")).map_err(|_| ())?.unwrap_or("");
            let binding = self.bindings.resolve(token).ok_or(())?;
            let tool = document.text(map.get("tool")).map_err(|_| ())?.unwrap_or("");
            let arguments = request.as_object().and_then(|map| map.get("arguments")).ok_or(())?;
            let value = validate_in(document, tool, arguments).map_err(|_| ())?;
            // The user has 30 seconds to choose a browser: only an authenticated, validated URL request gets the extra time.
            if tool == "open_url" {
                *deadline = Instant::now() + Duration::from_secs(OPEN_URL_SECONDS);
            }
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
                result = tokio::time::timeout_at(*deadline, handled) => result.map_err(|_| ())?.map_err(|_| ()),
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
        let _counted = Counted { calls: &self.delegation_calls, token };
        *deadline = Instant::now() + Duration::from_secs(DELEGATION_SECONDS);
        let caller = DelegationCaller {
            pane_id: binding.pane_id.clone(),
            kind: binding.kind().to_string(),
            provider: binding.provider.clone(),
            delegation: binding.delegation(),
            permission_mode: binding.permission_mode(),
        };
        // On its own task: a handler that panics is one that did not answer, and one past the deadline goes on unwatched.
        let handled = tokio::spawn(handler.handle(request, caller));
        tokio::select! {
            answered = tokio::time::timeout_at(*deadline, handled) => Some(match answered {
                Ok(Ok(response)) => response,
                _ => DelegationResponse::failure(NO_ANSWER_MESSAGE),
            }),
            _ = self.shutdown.cancelled() => None,
        }
    }
}

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
    Arc::new(move |request: &mighty_run::model::StartRunRequest, workspace: &mighty_core::model::Workspace| {
        if workspace.remote.is_some() || workspace.path().is_empty() || !matches!(request.provider.as_str(), "claude" | "codex") {
            return None;
        }
        let binding = io.bind(&request.session_id, workspace.id(), workspace.path(), &request.provider, &executable).ok()?;
        let (handler, enabled) = delegation();
        let settings = request.settings();
        let attached = super::binding::attaches_delegation(&request.kind, &request.provider, &settings.permission_mode) && enabled;
        let mode = request.permission_mode_override.clone().unwrap_or(settings.permission_mode);
        binding.connect_delegation(handler, attached, &mode);
        Some(binding.run_binding())
    })
}

/// One delegation call counted against its token until it is answered.
struct Counted<'a> {
    calls: &'a Mutex<HashMap<String, usize>>,
    token: String,
}

impl Drop for Counted<'_> {
    fn drop(&mut self) {
        let mut calls = lock(self.calls);
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
}
