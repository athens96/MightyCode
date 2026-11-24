//! The Mac's MCP-server checks over the real transport (M-C; Swift's AgentIOMCPServerTests, AgentTerminalToolTests and
//! DelegationSwitchTests named in the Mac plan), each under its Swift name: the exe's two MCP modes (the checks' relaunched exe, as the
//! app relaunches MightyCode) over their stdio, and the app's listener answering them. They run on Windows over the named pipe and on
//! the Mac over the Unix socket: the MCP modes need only the transport, and the check starts the exe itself.
#![cfg(any(windows, target_os = "macos"))]

mod delegation_io;

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use delegation_io::{executable, os_transport, request, temp};
use mighty_delegation::agent_io::binding::{SERVER_NAME, TOKEN_KEY};
use mighty_delegation::agent_io::client;
use mighty_delegation::agent_io::server::{AgentIo, TerminalCall, TerminalHandler};
use mighty_delegation::mcp::workspace::{
    DETACHED_MESSAGE as WORKSPACE_DETACHED_MESSAGE, SERVER_NAME as WORKSPACE_SERVER_NAME, WorkspaceCaller, WorkspaceHandler,
    WorkspaceRequest, WorkspaceResponse,
};
use mighty_delegation::modes::{AGENT_IO_MCP_ARGUMENT, DELEGATION_MCP_ARGUMENT, WORKSPACE_MCP_ARGUMENT};
use mighty_delegation::request::{DETACHED_MESSAGE, DelegationResponse};
use mighty_platform::BoxFuture;
use serde_json::{Value, json};

/// Swift's `PipedMCPServer`: the exe in one MCP mode with `environment`, its stdin written line by line and its stdout read a line at
/// a time.
struct PipedServer {
    child: Child,
    input: Option<ChildStdin>,
    output: Arc<Mutex<BufReader<std::process::ChildStdout>>>,
}

impl PipedServer {
    fn start(mode: &str, environment: &std::collections::BTreeMap<String, String>) -> PipedServer {
        #[allow(clippy::disallowed_methods, reason = "the check starts the checks' own exe, as C#'s Verification.Self does")]
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_mighty-delegation-fake"));
        command.arg(mode).current_dir(std::env::temp_dir()).env_clear().envs(environment);
        if let Ok(path) = std::env::var("PATH") {
            command.env("PATH", path);
        }
        if let Ok(root) = std::env::var("SystemRoot") {
            command.env("SystemRoot", root);
        }
        command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        let mut child = command.spawn().unwrap();
        let input = child.stdin.take();
        let output = Arc::new(Mutex::new(BufReader::new(child.stdout.take().unwrap())));
        PipedServer { child, input, output }
    }

    fn send_raw(&mut self, text: &str) {
        let input = self.input.as_mut().unwrap();
        input.write_all(text.as_bytes()).unwrap();
        input.flush().unwrap();
    }

    fn send(&mut self, message: Value) {
        self.send_raw(&format!("{message}\n"));
    }

    /// The next line it wrote, read within 60 s.
    async fn next(&self) -> Value {
        let output = self.output.clone();
        let read = tokio::task::spawn_blocking(move || {
            let mut line = String::new();
            output.lock().unwrap().read_line(&mut line).unwrap();
            line
        });
        let line = tokio::time::timeout(Duration::from_secs(60), read).await.expect("an answer in time").unwrap();
        serde_json::from_str(&line).unwrap_or_else(|_| panic!("a JSON line: {line:?}"))
    }

    /// Closes its stdin; whether it then left with 0 within 25 s.
    async fn close(mut self) -> bool {
        drop(self.input.take());
        let mut child = self.child;
        let exited = tokio::task::spawn_blocking(move || child.wait().unwrap());
        matches!(tokio::time::timeout(Duration::from_secs(25), exited).await, Ok(Ok(status)) if status.code() == Some(0))
    }
}

/// The terminal tools as the app's would answer them for one finished `echo hi` (Swift's `AgentTerminalIOHandler` ran it for real; the
/// run itself is mighty-terminal's, checked there), recording each call.
struct EchoTerminal(Arc<Mutex<Vec<(String, String, String)>>>);

impl TerminalHandler for EchoTerminal {
    fn handle(&self, call: TerminalCall) -> BoxFuture<'static, Result<Value, String>> {
        self.0.lock().unwrap().push((call.pane_id.clone(), call.tool.clone(), call.value.clone()));
        Box::pin(async move {
            match call.tool.as_str() {
                "run_in_terminal" => Ok(json!({"handle": "h1", "status": "done", "output": "hi\n", "exitCode": 0})),
                "read_latest_output" if call.value == "h1" => {
                    Ok(json!({"handle": "h1", "status": "done", "output": "hi\n", "exitCode": 0}))
                }
                _ => Err("unknown handle".into()),
            }
        })
    }
}

/// `AgentIOMCPServerTests.swift:135 "serverLoopSurvivesGarbageAndExitsWhenStdinCloses"`: the terminal server answers a line that is
/// not JSON with a parse error, skips blank lines, still answers a ping, and leaves with 0 once stdin closes.
#[tokio::test(flavor = "multi_thread")]
async fn server_loop_survives_garbage_and_exits_when_stdin_closes() {
    let mut mcp = PipedServer::start(AGENT_IO_MCP_ARGUMENT, &Default::default());
    mcp.send_raw("this is not json\n");
    assert_eq!(mcp.next().await["error"]["code"], -32700, "a parse error");
    mcp.send_raw("\n\r\n");
    mcp.send(json!({"jsonrpc": "2.0", "id": 9, "method": "ping"}));
    let pong = mcp.next().await;
    assert!(pong["id"] == 9 && !pong["result"].is_null(), "the ping after the garbage is answered: {pong}");
    assert!(mcp.close().await, "the server leaves with 0 once stdin closes");
}

/// `AgentTerminalToolTests.swift:143 "mcpServerOverPipesRunsEchoThroughTheRealSocketServer"`: the terminal server started with a
/// pane's environment initializes, lists the four tools, and its `run_in_terminal` reaches the app's listener on the pane's token and
/// comes back as the tool's result; its handle reads back too.
#[tokio::test(flavor = "multi_thread")]
async fn mcp_server_over_pipes_runs_echo_through_the_real_socket_server() {
    let calls = Arc::new(Mutex::new(vec![]));
    let io = AgentIo::start(os_transport().0, Arc::new(EchoTerminal(calls.clone()))).unwrap();
    let binding = io.bind("pane-a", "ws-1", &temp(), "claude", &executable()).unwrap();
    let mut mcp = PipedServer::start(AGENT_IO_MCP_ARGUMENT, &binding.environment());
    mcp.send(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "0"}}}));
    let initialized = mcp.next().await;
    assert!(
        initialized["id"] == 1
            && initialized["result"]["protocolVersion"] == "2025-06-18"
            && !initialized["result"]["capabilities"]["tools"].is_null()
            && initialized["result"]["serverInfo"]["name"] == SERVER_NAME,
        "{initialized}"
    );
    mcp.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    mcp.send(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}));
    let listed = mcp.next().await;
    let names: Vec<&str> = listed["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(
        listed["id"] == 2
            && names
                == [
                    "run_in_terminal",
                    "read_latest_output",
                    "stop",
                    "open_url",
                    "mighty_browser_check",
                    "list_bots",
                    "ask_bot",
                    "bot_answer",
                    "create_bot",
                    "update_bot"
                ],
        "{names:?}"
    );
    mcp.send(json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "run_in_terminal", "arguments": {"command": "echo hi"}}}));
    let called = mcp.next().await;
    assert!(
        called["id"] == 3
            && called["result"]["isError"] == false
            && called["result"]["structuredContent"]["output"] == "hi\n"
            && called["result"]["structuredContent"]["exitCode"] == 0,
        "{called}"
    );
    mcp.send(
        json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {"name": "read_latest_output", "arguments": {"handle": "h1"}}}),
    );
    let read = mcp.next().await;
    assert!(read["id"] == 4 && read["result"]["isError"] == false && read["result"]["structuredContent"]["status"] == "done", "{read}");
    assert_eq!(
        *calls.lock().unwrap(),
        [
            ("pane-a".to_string(), "run_in_terminal".to_string(), "echo hi".to_string()),
            ("pane-a".to_string(), "read_latest_output".to_string(), "h1".to_string())
        ],
        "both calls reached the app on the pane's token"
    );
    assert!(mcp.close().await);
    io.close().await;
}

/// `DelegationSwitchTests.swift:491 "theServerLoopAnswersOverPipesAndExitsWhenStdinCloses"`: the delegation server's loop answers a
/// ping and leaves with 0 once stdin closes.
#[tokio::test(flavor = "multi_thread")]
async fn the_server_loop_answers_over_pipes_and_exits_when_stdin_closes() {
    let environment = [(TOKEN_KEY.to_string(), "fixture-token".to_string())].into_iter().collect();
    let mut mcp = PipedServer::start(DELEGATION_MCP_ARGUMENT, &environment);
    mcp.send(json!({"jsonrpc": "2.0", "id": 3, "method": "ping"}));
    let pong = mcp.next().await;
    assert!(pong["id"] == 3 && !pong["result"].is_null(), "{pong}");
    assert!(mcp.close().await, "the server leaves with 0 once stdin closes");
}

/// `DelegationSwitchTests.swift:409 "terminalCallsOnTheSharedSocketAreServedAsBefore"`: with no delegation handler connected (the
/// app's switch off), a delegation call on a Claude pane's token reaches nothing, and that pane's terminal calls on the same listener
/// are served as before.
#[tokio::test(flavor = "multi_thread")]
async fn terminal_calls_on_the_shared_socket_are_served_as_before() {
    let calls = Arc::new(Mutex::new(vec![]));
    let (listener, connector) = os_transport();
    let io = AgentIo::start(listener, Arc::new(EchoTerminal(calls.clone()))).unwrap();
    let claude = io.bind("claude", "ws", &temp(), "claude", &executable()).unwrap();
    claude.connect_delegation(None, true, "");
    let token = claude.environment()[TOKEN_KEY].clone();
    let delegate = request("delegate", &[("task", "Write the notes"), ("mode", "plan")]);
    assert_eq!(
        client::call_delegation(connector.as_ref(), io.name(), &token, &delegate).await,
        DelegationResponse::failure(DETACHED_MESSAGE),
        "no delegation server to reach"
    );
    let terminal = client::call(connector.as_ref(), io.name(), &token, "read_latest_output", &json!({"handle": "h1"})).await.unwrap();
    let terminal: Value = serde_json::from_str(&terminal).unwrap();
    assert!(terminal["ok"] == true && terminal["result"]["output"] == "hi\n", "{terminal}");
    assert_eq!(calls.lock().unwrap().iter().map(|c| c.0.as_str()).collect::<Vec<_>>(), ["claude"], "only the terminal call reached it");
    io.close().await;
}

struct StubWorkspace(Arc<Mutex<Vec<(String, String)>>>);

impl WorkspaceHandler for StubWorkspace {
    fn handle(&self, request: WorkspaceRequest, caller: WorkspaceCaller) -> BoxFuture<'static, WorkspaceResponse> {
        self.0.lock().unwrap().push((caller.pane_id, request.tool.clone()));
        Box::pin(async move {
            match request.tool.as_str() {
                "list_workspaces" => WorkspaceResponse::ok("1. api (@workspace:api)"),
                _ => WorkspaceResponse::failure("unknown workspace tool"),
            }
        })
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn workspace_mcp_server_over_pipes_reaches_the_real_socket_server_and_honors_disable() {
    let calls = Arc::new(Mutex::new(vec![]));
    let seen = Arc::new(Mutex::new(vec![]));
    let io = AgentIo::start(os_transport().0, Arc::new(EchoTerminal(calls))).unwrap();
    let binding = io.bind("pane-w", "ws-1", &temp(), "claude", &executable()).unwrap();
    let mut mcp = PipedServer::start(WORKSPACE_MCP_ARGUMENT, &binding.environment());

    mcp.send(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "0"}}}));
    let initialized = mcp.next().await;
    assert!(
        initialized["id"] == 1
            && initialized["result"]["protocolVersion"] == "2025-06-18"
            && initialized["result"]["serverInfo"]["name"] == WORKSPACE_SERVER_NAME,
        "{initialized}"
    );
    mcp.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    mcp.send(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}));
    let listed = mcp.next().await;
    let names: Vec<&str> = listed["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["list_workspaces", "search_workspace", "read_workspace_file", "recent_workspace_activity"]);

    // Disabled by default until `connect_workspace(..., true)` is attached.
    mcp.send(json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "list_workspaces", "arguments": {}}}));
    let disabled = mcp.next().await;
    assert!(
        disabled["id"] == 3
            && disabled["result"]["isError"] == true
            && disabled["result"]["content"][0]["text"] == WORKSPACE_DETACHED_MESSAGE,
        "{disabled}"
    );

    // Enabling cross-workspace lookup on the live binding immediately routes calls over the shared pipe.
    binding.connect_workspace(Some(Arc::new(StubWorkspace(seen.clone()))), true);
    mcp.send(json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {"name": "list_workspaces", "arguments": {}}}));
    let enabled = mcp.next().await;
    assert!(
        enabled["id"] == 4 && enabled["result"]["isError"] != true && enabled["result"]["content"][0]["text"] == "1. api (@workspace:api)",
        "{enabled}"
    );
    assert_eq!(*seen.lock().unwrap(), [("pane-w".to_string(), "list_workspaces".to_string())]);

    assert!(mcp.close().await);
    io.close().await;
}
