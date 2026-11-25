//! C# AgentIOVerification and AgentIORevocationVerification (Verification.cs L513, L521-L525): the pane binding and its secret, the
//! tools' argument rules, the stdio contract of mighty-terminal, the authenticated pipe and its revocation, the agent panes kept out
//! of saved state. (0.5 has no open_url prompt, C#'s AgentWebOpenVerification: a page opens in the default browser.) The pipe checks run over the in-memory transport on
//! every OS, over the real named pipe on Windows and over the real Unix socket on the Mac (M-C, with the Swift socket checks).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mighty_core::model::{AppSnapshot, RunSession, Workspace};
use mighty_delegation::agent_io::binding::{AgentIoBindings, REDACTED, TOKEN_KEY};
use mighty_delegation::agent_io::client;
use mighty_delegation::agent_io::mcp_config;
use mighty_delegation::agent_io::server::{AgentIo, TerminalCall, TerminalHandler};
use mighty_delegation::agent_io::tools::validate;
use mighty_delegation::agent_io::wire::{self, MAXIMUM_BYTES};
use mighty_delegation::mcp::serve::{self, Output};
use mighty_delegation::mcp::terminal::TerminalServer;
use mighty_platform::fake::FakeAgentIo;
use mighty_platform::{AgentIoConnector, AgentIoListener, BoxFuture, McpTransport};
use serde_json::{Value, json};
use tokio::sync::{Notify, mpsc};

fn temp() -> String {
    std::env::temp_dir().to_string_lossy().into_owned()
}

fn executable(name: &str) -> String {
    std::env::temp_dir().join(name).to_string_lossy().into_owned()
}

/// "Agent IO BindingSecretsAndArgumentBoundaries"; on the Mac also Swift `AgentIOMCPServerTests.swift:273`
/// "aRunGetsTheTokenInItsEnvironmentNotItsArgvAndClosingRevokesIt": the token is never in a CLI's arguments. A Claude CLI's own
/// environment does not hold it either (its servers read it from a private config file, gone with the run or the binding); a Codex
/// run's does, forwarded to its servers by name and kept from its shell tool's commands (a closed binding resolves no more).
#[test]
fn binding_secrets_and_argument_boundaries() {
    let registry = AgentIoBindings::default();
    let executable = executable("Mighty Code.exe");
    let first = registry.bind("pane", "workspace", &temp(), "claude", "pipe", &executable).unwrap();
    let secret = first.environment()[TOKEN_KEY].clone();
    assert!(
        registry.resolve(&secret).is_some_and(|b| Arc::ptr_eq(&b, &first)) && registry.resolve(&"0".repeat(64)).is_none(),
        "unknown tokens cannot resolve a pane"
    );
    assert!(
        !format!("{first:?}").contains(&secret) && first.redact(&format!("x{secret}y")) == format!("x{REDACTED}y"),
        "binding and output redact the secret"
    );
    let run = first.run_binding();
    assert!(run.environment.is_empty(), "the Claude CLI's environment, which its commands inherit, never holds the token");
    let args = run.arguments.clone();
    assert!(args[0] == "--mcp-config" && !args.join(" ").contains(&secret), "Claude token never appears in argv");
    let file = std::path::PathBuf::from(&args[1]);
    assert_eq!(first.config_file().as_deref(), Some(file.as_path()), "the argument names the binding's config file");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = |path: &std::path::Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert!(mode(&file) == 0o600 && mode(file.parent().unwrap()) == 0o700, "the file and its folder are the user's alone");
    }
    let config: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(config["mcpServers"]["mighty-terminal"]["command"], json!(executable), "space-containing path is a structured argument");
    assert_eq!(
        config["mcpServers"]["mighty-terminal"]["env"],
        json!({"MIGHTY_AGENT_IO_PIPE": "pipe", "MIGHTY_PANE_TOKEN": secret}),
        "the token and the pipe go to the MCP server alone, in its own env"
    );
    (run.ended.expect("the run's end deletes the file"))();
    assert!(!file.exists() && first.config_file().is_none(), "the run's end deleted the config file");
    let mut args = vec![];
    first.append_arguments(&mut args);
    let again = std::path::PathBuf::from(&args[1]);
    assert!(again.exists(), "a new file for a new list");
    let second = registry.bind("pane", "workspace", &temp(), "codex", "pipe", &executable).unwrap();
    assert!(registry.resolve(&secret).is_none() && !again.exists(), "new run revokes prior binding and deletes its file");
    let run = second.run_binding();
    let second_secret = second.environment()[TOKEN_KEY].clone();
    // `MIGHTY_*` joins the user's own `exclude` (none added when their Codex policy uses `filters`).
    let exclude = mcp_config::codex_shell_exclude(mcp_config::codex_home_config().as_deref());
    assert!(exclude.as_deref().is_none_or(|e| e.contains("\"MIGHTY_*\"")));
    assert!(
        run.arguments.iter().any(|s| s == r#"mcp_servers.mighty-terminal.env_vars=["MIGHTY_PANE_TOKEN","MIGHTY_AGENT_IO_PIPE"]"#)
            && exclude.is_none_or(|exclude| run.arguments.iter().any(|s| *s == format!("shell_environment_policy.exclude={exclude}")))
            && run.arguments.iter().all(|s| !s.contains("developer_instructions") && !s.contains(&second_secret))
            && run.environment[TOKEN_KEY] == second_secret,
        "Codex forwards secret env names, keeps them from its shell tool and preserves user instructions"
    );
    registry.revoke("pane");
    assert!(registry.resolve(&second_secret).is_none(), "pane close revokes its token");
    for url in
        ["file:///tmp/private", "javascript:alert(1)", "https://user:password@example.com", "data:text/html,x", "https://example.com\n"]
    {
        assert!(validate("open_url", &json!({ "url": url })).is_err(), "invalid input was accepted: {url}");
    }
    assert_eq!(
        validate("open_url", &json!({"url": "http://localhost:3000/path"})).as_deref(),
        Ok("http://localhost:3000/path"),
        "developer localhost previews supported"
    );
    assert!(validate("run_in_terminal", &json!({"command": "echo safe", "workspace": "other"})).is_err());
    assert!(validate("run_in_terminal", &json!({"command": "가".repeat(30_000)})).is_err());
    // Bind's own refusals: ids, full paths, the two agents.
    for (pane, workspace, path, provider, exe) in [
        ("bad pane", "w", temp(), "claude", executable.clone()),
        ("p", "w", "relative".to_string(), "claude", executable.clone()),
        ("p", "w", temp(), "gemini", executable.clone()),
        ("p", "w", temp(), "claude", "app.exe".to_string()),
    ] {
        assert!(registry.bind(pane, workspace, &path, provider, "pipe", &exe).is_err(), "{pane} {path} {provider} {exe}");
    }
}

/// A transport that answers every call with the C# check's terminal result.
fn counted_transport(calls: Arc<AtomicUsize>) -> mighty_delegation::mcp::terminal::TerminalTransport {
    Arc::new(move |_tool, _args| {
        calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {
            Ok(r#"{"ok":true,"result":{"handle":"handle","status":"done","output":"한글\nline2","exitCode":0}}"#.to_string())
        })
    })
}

/// "Agent IO StdioMcpContract".
#[tokio::test]
async fn stdio_mcp_contract() {
    let calls = Arc::new(AtomicUsize::new(0));
    let server = TerminalServer::new(counted_transport(calls.clone()));
    let parse = |line: Option<String>| serde_json::from_str::<Value>(&line.expect("an answer")).unwrap();
    let hello = parse(server.handle(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25"}}"#).await);
    assert_eq!(hello["result"]["protocolVersion"], "2025-11-25", "protocol negotiates a supported revision");
    let tools = parse(server.handle(r#"{"jsonrpc":"2.0","id":"list","method":"tools/list"}"#).await);
    let listed = tools["result"]["tools"].as_array().unwrap();
    assert_eq!(listed.len(), 11, "the Mac terminal/web tool set, MC-48's browser check, MC-58's bot tools, MC-65's bot specs and MC-73's report");
    // MC-73: bot_report takes a status (one of four) and a summary, items optional.
    assert_eq!(listed[10]["name"], "bot_report");
    assert_eq!(listed[10]["inputSchema"]["required"], serde_json::json!(["status", "summary"]));
    assert_eq!(listed[10]["inputSchema"]["properties"]["status"]["enum"], serde_json::json!(["ok", "report", "needs", "failed"]));
    let check = &listed[4];
    assert_eq!(check["name"], "mighty_browser_check");
    assert!(check["inputSchema"]["required"].is_null(), "url and paths are optional: {check}");
    assert_eq!(check["inputSchema"]["properties"]["paths"]["type"], "array");
    // MC-58: list_bots takes nothing; ask_bot a bot and a prompt; bot_answer a bot and an optional wait.
    assert_eq!(listed[5]["name"], "list_bots");
    assert!(listed[5]["inputSchema"]["required"].is_null() && listed[5]["inputSchema"]["properties"] == serde_json::json!({}));
    assert_eq!(listed[6]["name"], "ask_bot");
    assert_eq!(listed[6]["inputSchema"]["required"], serde_json::json!(["bot", "prompt"]));
    assert_eq!(listed[7]["name"], "bot_answer");
    assert_eq!(listed[7]["inputSchema"]["properties"]["wait"]["type"], "boolean");
    assert_eq!(listed[7]["inputSchema"]["required"], serde_json::json!(["bot"]));
    // MC-65: create_bot and update_bot take one spec object.
    for (at, name) in [(8, "create_bot"), (9, "update_bot")] {
        assert_eq!(listed[at]["name"], name);
        assert_eq!(listed[at]["inputSchema"]["properties"]["spec"]["type"], "object");
        assert_eq!(listed[at]["inputSchema"]["required"], serde_json::json!(["spec"]));
    }
    assert!(
        server.handle(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).await.is_none(),
        "notifications receive no protocol reply"
    );
    let invalid = parse(
        server
            .handle(r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"open_url","arguments":{"url":"file:///secret"}}}"#)
            .await,
    );
    assert!(invalid["result"]["isError"] == true && calls.load(Ordering::SeqCst) == 0, "invalid tools never reach the host");
    let valid = parse(
        server
            .handle(r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"run_in_terminal","arguments":{"command":"echo hello"}}}"#)
            .await,
    );
    assert!(
        valid["result"]["isError"] == false
            && calls.load(Ordering::SeqCst) == 1
            && valid["result"]["structuredContent"]["output"] == "한글\nline2",
        "UTF-8 structured output remains intact"
    );
    let (sender, receiver) = mpsc::channel(4);
    sender
        .send(Ok(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n{\"jsonrpc\":\"2.0\",\"method\":\"notification\"}\n".to_vec()))
        .await
        .unwrap();
    drop(sender);
    let buffer = Arc::new(Mutex::new(Vec::<u8>::new()));
    let output: Output = buffer.clone();
    serve::serve(receiver, output, server.handler()).await.unwrap();
    let text = String::from_utf8(buffer.lock().unwrap().clone()).unwrap();
    assert_eq!(text.split('\n').filter(|line| !line.trim().is_empty()).count(), 1, "stdio contains only one JSON-RPC reply per request");
}

/// The terminal tools' handler of the pipe checks: answers `{owner}` and records whose call it served.
struct Owner(Arc<Mutex<Vec<String>>>);

impl TerminalHandler for Owner {
    fn handle(&self, call: TerminalCall) -> BoxFuture<'static, Result<Value, String>> {
        self.0.lock().unwrap().push(call.pane_id.clone());
        Box::pin(async move { Ok(json!({"owner": call.pane_id})) })
    }
}

async fn authenticated_pipe_rejects_revocation_over(listener: Arc<dyn AgentIoListener>, connector: Arc<dyn AgentIoConnector>) {
    let received = Arc::new(Mutex::new(vec![]));
    let host = AgentIo::start(listener, Arc::new(Owner(received.clone()))).unwrap();
    let binding = host.bind("mine", "workspace", &temp(), "codex", &executable("app.exe")).unwrap();
    let token = binding.environment()[TOKEN_KEY].clone();
    let arguments = json!({"command": "echo fixture"});
    let first: Value =
        serde_json::from_str(&client::call(connector.as_ref(), host.name(), &token, "run_in_terminal", &arguments).await.unwrap()).unwrap();
    assert!(first["ok"] == true && first["result"]["owner"] == "mine", "authenticated pipe routes only to bound owner");
    host.revoke("mine");
    let next: Value =
        serde_json::from_str(&client::call(connector.as_ref(), host.name(), &token, "run_in_terminal", &arguments).await.unwrap()).unwrap();
    assert!(next["ok"] == false && *received.lock().unwrap() == ["mine"], "revoked credential cannot execute another tool");
    let mut oversized: &[u8] = &[0x7f, 0xff, 0xff, 0xff];
    assert!(wire::read_packet(&mut oversized, McpTransport::NamedPipe).await.is_err(), "oversized packet allocated");
    host.close().await;
}

/// MC-38: a run an earlier app kept running in the background daemon is bound again in the later app with the token its CLI's MCP
/// servers already hold, on the transport name they were started with (served beside the later app's own when it differs, as a
/// Windows pipe does), and its calls are taken and checked as every call is: revoked as any binding is. A kept binding of another
/// workspace or agent, a token another pane holds, one that is not 64 hex digits, or a name the platform cannot listen on is refused.
#[tokio::test]
async fn a_kept_runs_binding_is_taken_back_with_its_token() {
    let received = Arc::new(Mutex::new(vec![]));
    let earlier_io = FakeAgentIo::default();
    let earlier = AgentIo::start(Arc::new(earlier_io.clone()), Arc::new(Owner(received.clone()))).unwrap();
    let binding = earlier.bind("mine", "workspace", &temp(), "codex", &executable("app.exe")).unwrap();
    let kept = binding.kept();
    let token = binding.environment()[TOKEN_KEY].clone();
    let old_name = earlier.name().to_string();
    earlier.close().await;
    drop(binding);

    let later_io = FakeAgentIo::default();
    let later = AgentIo::start(Arc::new(later_io.clone()), Arc::new(Owner(received.clone()))).unwrap();
    assert_ne!(later.name(), old_name, "the later app listens on a name of its own");
    assert!(
        later.adopt("mine", "workspace", &temp(), "codex", &executable("app.exe"), &kept).is_err(),
        "no listener for the earlier name: refused"
    );
    let served: Arc<Mutex<Option<FakeAgentIo>>> = Arc::default();
    let made = served.clone();
    later.set_earlier_listener(Some(Arc::new(move |name: &str| {
        let io = FakeAgentIo::named(name);
        *made.lock().unwrap() = Some(io.clone());
        Some(Arc::new(io) as Arc<dyn AgentIoListener>)
    })));
    for (workspace, provider) in [("other", "codex"), ("workspace", "claude")] {
        assert!(later.adopt("mine", workspace, &temp(), provider, &executable("app.exe"), &kept).is_err(), "{workspace} {provider}");
    }
    let mut short = kept.clone();
    short["token"] = json!("abc");
    assert!(later.adopt("mine", "workspace", &temp(), "codex", &executable("app.exe"), &short).is_err(), "not a token");
    let taken = later.adopt("mine", "workspace", &temp(), "codex", &executable("app.exe"), &kept).unwrap();
    assert_eq!(taken.environment()[TOKEN_KEY], token, "the token the CLI holds");
    assert!(later.adopt("other-pane", "workspace", &temp(), "codex", &executable("app.exe"), &kept).is_err(), "one pane's token only");
    let connector = served.lock().unwrap().clone().expect("the earlier name is served");
    let arguments = json!({"command": "echo fixture"});
    let answer: Value =
        serde_json::from_str(&client::call(&connector, &old_name, &token, "run_in_terminal", &arguments).await.unwrap()).unwrap();
    assert!(answer["ok"] == true && answer["result"]["owner"] == "mine", "the kept run's call is taken: {answer}");
    later.revoke("mine");
    let refused: Value =
        serde_json::from_str(&client::call(&connector, &old_name, &token, "run_in_terminal", &arguments).await.unwrap()).unwrap();
    assert!(refused["ok"] == false, "and revoked as any binding is");
    later.close().await;
    assert!(client::call(&connector, &old_name, &token, "run_in_terminal", &arguments).await.is_err(), "closed with the app");
}

/// "Agent IO AuthenticatedPipeRejectsRevocation", over the in-memory transport.
#[tokio::test]
async fn authenticated_pipe_rejects_revocation() {
    let io = FakeAgentIo::default();
    authenticated_pipe_rejects_revocation_over(Arc::new(io.clone()), Arc::new(io)).await;
}

/// "Agent IO AuthenticatedPipeRejectsRevocation", over the named pipe.
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread")]
async fn authenticated_pipe_rejects_revocation_on_the_named_pipe() {
    authenticated_pipe_rejects_revocation_over(
        Arc::new(mighty_platform_windows::NamedPipeListener::new()),
        Arc::new(mighty_platform_windows::NamedPipeConnector),
    )
    .await;
}

/// "Agent IO AuthenticatedPipeRejectsRevocation", over the Mac's Unix socket (M-C: the socket exists now, so the pipe check runs on it).
#[cfg(target_os = "macos")]
#[tokio::test(flavor = "multi_thread")]
async fn authenticated_pipe_rejects_revocation_on_the_unix_socket() {
    let (listener, connector) = mac::transport("revocation");
    authenticated_pipe_rejects_revocation_over(listener, connector).await;
}

/// The Mac's socket checks (M-C), each over a profile folder of its own under `/tmp` (Swift's `shortTemporaryDirectory`).
#[cfg(target_os = "macos")]
mod mac {
    use std::os::unix::fs::PermissionsExt as _;
    use std::path::PathBuf;

    use mighty_delegation::agent_io::server::{TERMINAL_FAILURE, UNKNOWN_TOKEN_MESSAGE};
    use mighty_delegation::request::DelegationResponse;
    use mighty_platform_macos::agent_io::{UnixSocketConnector, UnixSocketListener, socket_path};
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    use super::*;

    pub fn folder(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos();
        let folder = PathBuf::from(format!("/tmp/mc-io-{name}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        folder
    }

    pub fn transport(name: &str) -> (Arc<dyn AgentIoListener>, Arc<dyn AgentIoConnector>) {
        (Arc::new(UnixSocketListener::new(&folder(name))), Arc::new(UnixSocketConnector))
    }

    fn mode(path: &std::path::Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    async fn read_latest(connector: &dyn AgentIoConnector, socket: &str, token: &str) -> Value {
        match client::call(connector, socket, token, "read_latest_output", &json!({"handle": "h"})).await {
            Ok(text) => serde_json::from_str(&text).unwrap(),
            Err(_) => json!({"failed": true}),
        }
    }

    /// `AgentIOMCPServerTests.swift:327 "socketIsPrivateResolvesTokensToTheirPaneAndIsRemovedOnStop"`: the socket's folder is 0700
    /// and the socket 0600; each pane's token reaches only its own pane, a forged or revoked one none (a terminal call gets C#'s one
    /// refusal text where Swift's says the unknown-token words; a delegation call gets those words on both); once the server stops
    /// the socket is gone and a call fails.
    #[tokio::test(flavor = "multi_thread")]
    async fn socket_is_private_resolves_tokens_to_their_pane_and_is_removed_on_stop() {
        let profile = folder("private");
        let seen = Arc::new(Mutex::new(vec![]));
        let io = AgentIo::start(Arc::new(UnixSocketListener::new(&profile)), Arc::new(Owner(seen.clone()))).unwrap();
        let socket = io.name().to_string();
        assert_eq!(socket, socket_path(&profile, &std::env::temp_dir()).to_string_lossy(), "the profile's socket");
        assert_eq!(io.transport(), McpTransport::UnixSocket);
        let path = std::path::Path::new(&socket);
        assert!(mode(path.parent().unwrap()) == 0o700 && mode(path) == 0o600, "the socket and its folder are the user's alone");
        let a = io.bind("a", "w", &profile.to_string_lossy(), "claude", &executable("app")).unwrap();
        let b = io.bind("b", "w", &profile.to_string_lossy(), "codex", &executable("app")).unwrap();
        let (a_token, b_token) = (a.environment()[TOKEN_KEY].clone(), b.environment()[TOKEN_KEY].clone());
        assert_eq!(a.environment()["MIGHTY_AGENT_IO_SOCKET"], socket, "the run's environment names the socket");
        let connector = UnixSocketConnector;
        assert_eq!(read_latest(&connector, &socket, &a_token).await["result"]["owner"], "a");
        assert_eq!(read_latest(&connector, &socket, &b_token).await["result"]["owner"], "b");
        let forged = read_latest(&connector, &socket, &"0".repeat(64)).await;
        assert!(forged["ok"] == false && forged["error"] == TERMINAL_FAILURE, "a forged token: {forged}");
        let list = mighty_delegation::request::DelegationRequest { tool: "list_children".into(), arguments: Default::default() };
        assert_eq!(
            client::call_delegation(&connector, &socket, "forged", &list).await,
            DelegationResponse::failure(UNKNOWN_TOKEN_MESSAGE),
            "a forged token's delegation call"
        );
        io.revoke("a");
        let revoked = read_latest(&connector, &socket, &a_token).await;
        assert!(revoked["ok"] == false && revoked["error"] == TERMINAL_FAILURE, "a revoked token: {revoked}");
        assert_eq!(*seen.lock().unwrap(), ["a", "b"], "only live tokens reached a pane");
        io.close().await;
        assert!(!path.exists(), "the socket is removed on stop");
        assert!(connector.connect(&socket, Duration::from_millis(200)).await.is_err(), "nothing answers once stopped");
        let _ = std::fs::remove_dir_all(&profile);
    }

    /// `AgentIOMCPServerTests.swift:355 "socketDropsAMessageOverOneMegabyteAndKeepsServing"`: a line over 1 MiB is dropped with its
    /// connection, unanswered and unhandled, and the next call is served.
    #[tokio::test(flavor = "multi_thread")]
    async fn socket_drops_a_message_over_one_megabyte_and_keeps_serving() {
        let profile = folder("large");
        let seen = Arc::new(Mutex::new(vec![]));
        let io = AgentIo::start(Arc::new(UnixSocketListener::new(&profile)), Arc::new(Owner(seen.clone()))).unwrap();
        let socket = io.name().to_string();
        let a = io.bind("a", "w", &profile.to_string_lossy(), "claude", &executable("app")).unwrap();
        let mut raw = tokio::net::UnixStream::connect(&socket).await.unwrap();
        let writing = tokio::spawn(async move {
            let _ = raw.write_all(&vec![b'x'; MAXIMUM_BYTES + 4096]).await;
            let mut answer = Vec::new();
            let _ = tokio::time::timeout(Duration::from_secs(10), raw.read_to_end(&mut answer)).await;
            answer
        });
        assert!(writing.await.unwrap().is_empty(), "no answer to an oversized line");
        assert!(seen.lock().unwrap().is_empty(), "nothing reached a pane");
        let served = read_latest(&UnixSocketConnector, &socket, &a.environment()[TOKEN_KEY]).await;
        assert_eq!(served["result"]["owner"], "a", "the next call is served");
        io.close().await;
        let _ = std::fs::remove_dir_all(&profile);
    }

    /// "Agent IO revocation cancels queued authenticated calls", over the Mac's Unix socket.
    #[tokio::test(flavor = "multi_thread")]
    async fn revocation_cancels_authenticated_queued_request_on_the_unix_socket() {
        let (listener, connector) = transport("queued");
        revocation_cancels_queued_over(listener, connector).await;
    }
}

/// A handler that waits to be released, then does its effect only if its call is still wanted (C#: `ThrowIfCancellationRequested`).
struct Released {
    entered: Arc<Notify>,
    release: Arc<Notify>,
    effects: Arc<AtomicUsize>,
}

impl TerminalHandler for Released {
    fn handle(&self, call: TerminalCall) -> BoxFuture<'static, Result<Value, String>> {
        let (entered, release, effects) = (self.entered.clone(), self.release.clone(), self.effects.clone());
        Box::pin(async move {
            entered.notify_one();
            release.notified().await;
            if call.binding.is_revoked() {
                return Err("revoked".into());
            }
            effects.fetch_add(1, Ordering::SeqCst);
            Ok(json!({"done": true}))
        })
    }
}

async fn revocation_cancels_queued_over(listener: Arc<dyn AgentIoListener>, connector: Arc<dyn AgentIoConnector>) {
    let handler = Released { entered: Arc::new(Notify::new()), release: Arc::new(Notify::new()), effects: Arc::new(AtomicUsize::new(0)) };
    let (entered, release, effects) = (handler.entered.clone(), handler.release.clone(), handler.effects.clone());
    let host = AgentIo::start(listener, Arc::new(handler)).unwrap();
    let previous = host.bind("pane", "workspace", &temp(), "codex", &executable("app.exe")).unwrap();
    let (name, token) = (host.name().to_string(), previous.environment()[TOKEN_KEY].clone());
    let call =
        tokio::spawn(
            async move { client::call(connector.as_ref(), &name, &token, "open_url", &json!({"url": "https://example.com"})).await },
        );
    tokio::time::timeout(Duration::from_secs(5), entered.notified()).await.expect("the call reached the handler");
    let next = host.bind("pane", "workspace", &temp(), "codex", &executable("app.exe")).unwrap();
    release.notify_waiters();
    let response: Value = serde_json::from_str(&call.await.unwrap().unwrap()).unwrap();
    assert!(
        response["ok"] == false
            && effects.load(Ordering::SeqCst) == 0
            && !host.bindings().is_current(&previous)
            && host.bindings().is_current(&next),
        "An authenticated request survived its binding's revocation while queued."
    );
    host.close().await;
}

/// "Agent IO revocation cancels queued authenticated calls", over the in-memory transport.
#[tokio::test]
async fn revocation_cancels_authenticated_queued_request() {
    let io = FakeAgentIo::default();
    revocation_cancels_queued_over(Arc::new(io.clone()), Arc::new(io)).await;
}

/// "Agent IO revocation cancels queued authenticated calls", over the named pipe.
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread")]
async fn revocation_cancels_authenticated_queued_request_on_the_named_pipe() {
    revocation_cancels_queued_over(
        Arc::new(mighty_platform_windows::NamedPipeListener::new()),
        Arc::new(mighty_platform_windows::NamedPipeConnector),
    )
    .await;
}

/// The ownership half of "Agent IO TerminalCleanerAndEphemeralOwnership" (the cleaner half is mighty-terminal's
/// tests/terminal_prompts.rs): an agent's terminal pane is shown live and never saved, goes with its agent pane, and never comes back
/// on a restore (it has no process then).
#[test]
fn terminal_cleaner_and_ephemeral_ownership() {
    use mighty_core::normalize::{normalize, stored};
    let workspace = Workspace { id: Some("w".into()), path: Some(temp()), ..Workspace::default() };
    let parent = RunSession { id: Some("agent".into()), workspace_id: Some("w".into()), ..RunSession::default() };
    let terminal = RunSession {
        id: Some("agent-terminal:agent".into()),
        kind: Some("agent-terminal".into()),
        owner_session_id: Some("agent".into()),
        workspace_id: Some("w".into()),
        ..RunSession::default()
    };
    let mut state = AppSnapshot::default();
    *state.workspaces_mut() = vec![workspace];
    *state.sessions_mut() = vec![Arc::new(parent.clone()), Arc::new(terminal.clone())];
    let snapshot = normalize(&state, false).unwrap();
    let kinds = |s: &AppSnapshot| s.sessions().iter().map(|p| (p.id().to_string(), p.kind().to_string())).collect::<Vec<_>>();
    assert!(
        snapshot.sessions().len() == 2 && kinds(&stored(&snapshot)) == [("agent".to_string(), parent.kind().to_string())],
        "agent IO is shown live and excluded from saved state"
    );
    let mut orphan = snapshot.clone();
    *orphan.sessions_mut() = vec![Arc::new(terminal)];
    assert!(normalize(&orphan, false).unwrap().sessions().is_empty(), "orphan IO panes disappear with parent");
    assert_eq!(normalize(&snapshot, true).unwrap().sessions().len(), 1, "agent IO never restores without native processes");
}

/// The 1 MiB line limit is the packet limit.
#[test]
fn the_line_limit_is_the_packet_limit() {
    assert_eq!(MAXIMUM_BYTES, 1_048_576);
    assert_eq!(serve::overflow_line().len(), MAXIMUM_BYTES + 1);
}
