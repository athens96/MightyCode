//! C# AgentIOVerification, AgentIORevocationVerification and AgentWebOpenVerification (Verification.cs L513, L521-L525, L634): the
//! pane binding and its secret, the tools' argument rules, the stdio contract of mighty-terminal, the authenticated pipe and its
//! revocation, the agent panes kept out of saved state, the open_url prompts. The pipe checks run over the in-memory transport on
//! every OS and over the real named pipe on Windows.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mighty_core::model::{AppSnapshot, RunSession, Workspace};
use mighty_delegation::agent_io::binding::{AgentIoBindings, REDACTED, TOKEN_KEY};
use mighty_delegation::agent_io::client;
use mighty_delegation::agent_io::server::{AgentIo, TerminalCall, TerminalHandler};
use mighty_delegation::agent_io::tools::validate;
use mighty_delegation::agent_io::web_open::{AgentWebOpenPrompts, DEFAULT_TIMEOUT, WebOpenAnswer, WebOpenError};
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

/// "Agent IO BindingSecretsAndArgumentBoundaries".
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
    let mut args = vec![];
    first.append_arguments(&mut args);
    assert!(args[0] == "--mcp-config" && !args.join(" ").contains(&secret), "Claude token never appears in argv/config");
    let config: Value = serde_json::from_str(&args[1]).unwrap();
    assert_eq!(config["mcpServers"]["mighty-terminal"]["command"], json!(executable), "space-containing path is a structured argument");
    let second = registry.bind("pane", "workspace", &temp(), "codex", "pipe", &executable).unwrap();
    assert!(registry.resolve(&secret).is_none(), "new run revokes prior binding");
    args.clear();
    second.append_arguments(&mut args);
    let second_secret = second.environment()[TOKEN_KEY].clone();
    assert!(
        args.iter().any(|s| s.contains("env_vars"))
            && args.iter().all(|s| !s.contains("developer_instructions") && !s.contains(&second_secret)),
        "Codex forwards secret env names and preserves user instructions"
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
    assert_eq!(tools["result"]["tools"].as_array().unwrap().len(), 4, "exactly the Mac terminal/web tool set");
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

fn bind(registry: &AgentIoBindings, pane: &str) -> Arc<mighty_delegation::agent_io::binding::AgentIoBinding> {
    registry.bind(pane, "workspace", &temp(), "codex", "fixture", &executable("app.exe")).unwrap()
}

/// "Agent URL independent choices and thirty-second fallback".
#[tokio::test]
async fn independent_choices_and_fallback() {
    let prompts = Arc::new(AgentWebOpenPrompts::default());
    let registry = AgentIoBindings::default();
    let (first, second) = (bind(&registry, "first"), bind(&registry, "second"));
    let ask = |binding: &Arc<_>, url: &'static str, timeout: Option<Duration>| {
        let (prompts, binding) = (prompts.clone(), Arc::clone(binding));
        tokio::spawn(async move { prompts.request(&binding, url, timeout).await })
    };
    let one = ask(&first, "http://localhost:3000/first", None);
    let two = ask(&second, "https://example.com/second", None);
    let extra = ask(&first, "https://example.com/third", None);
    support_wait(|| prompts.for_pane("first").len() == 2 && prompts.for_pane("second").len() == 1).await;
    let second_id = prompts.for_pane("second")[0].id.clone();
    assert!(prompts.choose(&second_id, "external", true), "second pane can answer before first");
    assert_eq!(two.await.unwrap(), Ok(WebOpenAnswer { destination: "external".into(), remember: true }));
    assert!(!one.is_finished() && !extra.is_finished(), "answer only completes its own request");
    assert!(!prompts.choose(&second_id, "inApp", false), "a duplicate answer is ignored");
    let first_id = prompts.for_pane("first")[0].id.clone();
    assert!(!prompts.choose(&first_id, "javascript", false), "unknown destination cannot resolve a request");
    prompts.choose(&first_id, "inApp", false);
    one.await.unwrap().unwrap();
    let last = prompts.for_pane("first");
    assert_eq!(last.len(), 1);
    prompts.choose(&last[0].id, "external", false);
    extra.await.unwrap().unwrap();
    assert_eq!(DEFAULT_TIMEOUT, Duration::from_secs(30), "production fallback matches Mac's thirty seconds");
    let fallback = ask(&first, "https://example.com/timeout", Some(Duration::from_millis(30)));
    support_wait(|| prompts.for_pane("first").len() == 1 || fallback.is_finished()).await;
    let expired_id = prompts.for_pane("first").first().map(|p| p.id.clone()).unwrap_or_default();
    assert_eq!(
        fallback.await.unwrap(),
        Ok(WebOpenAnswer { destination: "inApp".into(), remember: false }),
        "no response falls back in-app without remembering"
    );
    assert!(
        prompts.for_pane("first").is_empty() && !prompts.choose(&expired_id, "external", true),
        "timeout removes its card and ignores late remembered choices"
    );
}

/// "Agent URL revocation, cancellation and shutdown".
#[tokio::test]
async fn revocation_cancellation_and_shutdown() {
    let prompts = Arc::new(AgentWebOpenPrompts::default());
    let registry = AgentIoBindings::default();
    let (first, second) = (bind(&registry, "first"), bind(&registry, "second"));
    let ask = |binding: &Arc<_>, url: &'static str| {
        let (prompts, binding) = (prompts.clone(), Arc::clone(binding));
        tokio::spawn(async move { prompts.request(&binding, url, None).await })
    };
    let revoked = ask(&first, "https://example.com/first");
    support_wait(|| prompts.for_pane("first").len() == 1).await;
    let stale = prompts.for_pane("first")[0].id.clone();
    let replacement = bind(&registry, "first");
    assert_eq!(cancelled(revoked).await, Err(WebOpenError::Cancelled));
    assert!(
        prompts.for_pane("first").is_empty() && !prompts.choose(&stale, "external", true),
        "replaced binding immediately removes card and cannot open or remember"
    );
    let cancelled_request = ask(&replacement, "https://example.com/cancel");
    let independent = ask(&second, "https://example.com/second");
    support_wait(|| prompts.for_pane("first").len() == 1 && prompts.for_pane("second").len() == 1).await;
    // A request is cancelled by dropping it (the server's deadline, the caller gone).
    cancelled_request.abort();
    let _ = cancelled_request.await;
    assert!(prompts.for_pane("first").is_empty() && !independent.is_finished(), "request cancellation leaves other panes active");
    prompts.dispose();
    assert_eq!(cancelled(independent).await, Err(WebOpenError::Cancelled));
    assert!(prompts.for_pane("second").is_empty(), "shutdown cancels and removes every pending card");
    let after = prompts.request(&second, "https://example.com/late", None).await;
    assert_eq!(after, Err(WebOpenError::Cancelled), "nothing is asked once shut down");
}

/// The request's end within 2 s (C#'s `Cancelled`: a revoked URL request must cancel instead of opening a browser).
async fn cancelled(task: tokio::task::JoinHandle<Result<WebOpenAnswer, WebOpenError>>) -> Result<WebOpenAnswer, WebOpenError> {
    tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .expect("Revoked URL request must cancel instead of opening a browser.")
        .unwrap()
}

/// Polls `ready` every 5 ms for up to 5 s.
async fn support_wait(ready: impl Fn() -> bool) {
    for _ in 0..1000 {
        if ready() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("not ready in time");
}

/// The 1 MiB line limit is the packet limit.
#[test]
fn the_line_limit_is_the_packet_limit() {
    assert_eq!(MAXIMUM_BYTES, 1_048_576);
    assert_eq!(serve::overflow_line().len(), MAXIMUM_BYTES + 1);
}
