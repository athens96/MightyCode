//! C# "delegation: the pipe carries delegation calls and answers as the Mac does" (DelegationAppVerification.cs:98-165, Verification.cs
//! L672), in a binary of its own: it waits out a 31 s answer. On Windows over the named pipe; elsewhere over the in-memory transport
//! until the Mac's socket (its stage).

mod delegation_io;

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use delegation_io::{Canned, Gated, IoHandler, executable, request, temp, until};
use mighty_delegation::agent_io::binding::{AgentIoBinding, TOKEN_KEY};
use mighty_delegation::agent_io::client::{self, response_bytes};
use mighty_delegation::agent_io::server::{
    AgentIo, BUSY_MESSAGE, DELEGATION_CALLS_PER_TOKEN, MALFORMED_MESSAGE, TerminalCall, TerminalHandler, UNKNOWN_TOKEN_MESSAGE,
};
use mighty_delegation::agent_io::wire;
use mighty_delegation::request::{DETACHED_MESSAGE, DelegationCaller, DelegationHandler, DelegationRequest, DelegationResponse};
use mighty_platform::{AgentIoConnector, AgentIoListener, BoxFuture};
use serde_json::{Value, json};

struct Owner(Arc<Mutex<Vec<String>>>);

impl TerminalHandler for Owner {
    fn handle(&self, call: TerminalCall) -> BoxFuture<'static, Result<Value, String>> {
        self.0.lock().unwrap().push(call.pane_id.clone());
        Box::pin(async move { Ok(json!({"owner": call.pane_id})) })
    }
}

fn transport() -> (Arc<dyn AgentIoListener>, Arc<dyn AgentIoConnector>) {
    #[cfg(windows)]
    {
        (Arc::new(mighty_platform_windows::NamedPipeListener::new()), Arc::new(mighty_platform_windows::NamedPipeConnector))
    }
    #[cfg(not(windows))]
    {
        let io = mighty_platform::fake::FakeAgentIo::default();
        (Arc::new(io.clone()), Arc::new(io))
    }
}

/// One raw packet on a connection of its own, and the delegation answer it got.
async fn raw(connector: &dyn AgentIoConnector, name: &str, packet: &str) -> Option<DelegationResponse> {
    let transport = connector.transport();
    let mut stream = connector.connect(name, Duration::from_secs(10)).await.unwrap();
    wire::write_packet(&mut stream, packet.as_bytes(), transport).await.unwrap();
    client::read_response(&wire::read_packet(&mut stream, transport).await.unwrap())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn the_pipe_carries_delegation_calls_and_answers_as_the_mac_does() {
    let terminal = Arc::new(Mutex::new(vec![]));
    let (listener, connector) = transport();
    let pipe = AgentIo::start(listener, Arc::new(Owner(terminal.clone()))).unwrap();
    let bind = |pane: &str, provider: &str, handler: Option<Arc<dyn DelegationHandler>>, attached: bool| -> Arc<AgentIoBinding> {
        let binding = pipe.bind(pane, "ws", &temp(), provider, &executable()).unwrap();
        if let Some(handler) = handler {
            binding.connect_delegation(Some(handler), attached, "");
        }
        binding
    };
    let name = pipe.name().to_string();
    let send = |request: DelegationRequest, binding: &Arc<AgentIoBinding>| {
        let (connector, name, token) = (connector.clone(), name.clone(), binding.environment()[TOKEN_KEY].clone());
        async move { client::call_delegation(connector.as_ref(), &name, &token, &request).await }
    };
    let delegate = || request("delegate", &[("task", "Write the release notes"), ("mode", "plan")]);
    let on: Arc<dyn DelegationHandler> = Arc::new(IoHandler { on: true });
    // Switched on, a Codex run reaches the tools only where it gets the server (acceptEdits, fullAccess); in manual it stays detached.
    let codex = pipe.bind("codex", "ws", &temp(), "codex", &executable()).unwrap();
    codex.connect_delegation(Some(on.clone()), true, "acceptEdits");
    let answer = send(delegate(), &codex).await;
    assert!(
        codex.delegation()
            && codex.permission_mode() == "acceptEdits"
            && answer.refused.is_none()
            && answer.error.as_deref() == Some("Unknown delegation tool delegate."),
        "a Codex acceptEdits run reaches the tools"
    );
    let manual = pipe.bind("codex-manual", "ws", &temp(), "codex", &executable()).unwrap();
    manual.connect_delegation(Some(on.clone()), true, "manual");
    assert_eq!(send(delegate(), &manual).await, DelegationResponse::failure(DETACHED_MESSAGE), "a Codex manual run stays detached");
    assert!(
        send(delegate(), &bind("claude", "claude", Some(on.clone()), true)).await.refused.is_none(),
        "a Claude pane with the server attached is not refused"
    );
    // Switched off, every call reaches nothing; a Claude run started while it was off stays detached; so does a run no service connected.
    let off: Arc<dyn DelegationHandler> = Arc::new(IoHandler { on: false });
    for binding in [
        bind("codex-off", "codex", Some(off.clone()), true),
        bind("claude-off", "claude", Some(off.clone()), true),
        bind("claude-detached", "claude", Some(on.clone()), false),
        bind("claude-alone", "claude", None, false),
    ] {
        assert_eq!(send(delegate(), &binding).await, DelegationResponse::failure(DETACHED_MESSAGE), "{} reaches nothing", binding.pane_id);
    }
    // Forged and revoked tokens reach no pane.
    assert_eq!(
        client::call_delegation(connector.as_ref(), &name, "forged", &delegate()).await,
        DelegationResponse::failure(UNKNOWN_TOKEN_MESSAGE),
        "a forged token"
    );
    let revoked = bind("revoked", "claude", Some(on.clone()), true);
    pipe.revoke("revoked");
    assert_eq!(send(delegate(), &revoked).await, DelegationResponse::failure(UNKNOWN_TOKEN_MESSAGE), "a revoked token");
    assert_eq!(
        client::call_delegation(connector.as_ref(), "not-a-pipe", "x", &delegate()).await,
        DelegationResponse::failure("Mighty Code's agent socket path is invalid."),
        "an invalid pipe name is never opened"
    );
    // A malformed delegation call is answered, and the terminal calls on the shared pipe are served as before.
    let alone = pipe.bind("alone", "ws", &temp(), "claude", &executable()).unwrap();
    let alone_token = alone.environment()[TOKEN_KEY].clone();
    let packet = format!(r#"{{"token":"{alone_token}","delegation":{{"tool":5,"arguments":{{}}}}}}"#);
    assert_eq!(raw(connector.as_ref(), &name, &packet).await, Some(DelegationResponse::failure(MALFORMED_MESSAGE)), "a malformed call");
    let read_output = || {
        let (connector, name, token) = (connector.clone(), name.clone(), alone_token.clone());
        async move {
            let answer = client::call(connector.as_ref(), &name, &token, "read_latest_output", &json!({"handle": "h"})).await.unwrap();
            serde_json::from_str::<Value>(&answer).unwrap()
        }
    };
    let served = read_output().await;
    assert!(
        served["ok"] == true && served["result"]["owner"] == "alone" && *terminal.lock().unwrap() == ["alone"],
        "a terminal call is served, and no delegation call reached the terminal tools"
    );
    // No input ends a listener: a token that is not text, sent more times than there are listeners, and a call with no arguments
    // are answered as malformed, and the terminal tools are served after them.
    let lone = format!(r#"{{"token":"\u{}","delegation":{{}}}}"#, "d800");
    let no_arguments = format!(r#"{{"token":"{alone_token}","delegation":{{"tool":"list_children"}}}}"#);
    for packet in std::iter::repeat_n(lone.as_str(), 17).chain([no_arguments.as_str()]) {
        assert_eq!(
            raw(connector.as_ref(), &name, packet).await,
            Some(DelegationResponse::failure(MALFORMED_MESSAGE)),
            "a malformed call: {}",
            &packet[..packet.len().min(40)]
        );
    }
    assert_eq!(read_output().await["ok"], true, "a terminal call is served after seventeen malformed packets");
    // Long delegation calls never hold the listeners: twenty panes' calls and as many as one pane may have going wait while a
    // terminal call is served, and one call more from that pane is refused.
    let gated = Arc::new(Gated::default());
    let gate: Arc<dyn DelegationHandler> = gated.clone();
    let crowded = bind("crowded", "claude", Some(gate.clone()), true);
    let mut waiting = vec![];
    for i in 0..20 {
        let busy = bind(&format!("busy-{i}"), "claude", Some(gate.clone()), true);
        waiting.push(tokio::spawn(send(request("list_children", &[]), &busy)));
    }
    for _ in 0..DELEGATION_CALLS_PER_TOKEN {
        waiting.push(tokio::spawn(send(request("list_children", &[]), &crowded)));
    }
    let count = waiting.len();
    until(|| gated.entered.load(Ordering::SeqCst) == count, 30_000).await;
    assert_eq!(
        send(request("list_children", &[]), &crowded).await,
        DelegationResponse::failure(BUSY_MESSAGE),
        "one call more than a pane may have going is refused"
    );
    let clock = Instant::now();
    assert!(
        read_output().await["ok"] == true && clock.elapsed() < Duration::from_secs(10),
        "a terminal call is served while twenty-four delegation calls wait"
    );
    gated.release();
    for call in waiting {
        assert_eq!(call.await.unwrap(), DelegationResponse::failure("released"), "each waiting call gets its answer");
    }
    // Every answer crosses the pipe whole, with its caller and string arguments.
    let canned = Arc::new(Canned::new(Duration::ZERO));
    let calls = canned.calls.clone();
    let caller = bind("caller", "claude", Some(canned.clone()), true);
    for (tool, expected) in Canned::answers().into_iter().filter(|(tool, _)| *tool != "slow") {
        let got = send(request(tool, &[("child", "c1"), ("text", "한글 \"quoted\"")]), &caller).await;
        assert_eq!(response_bytes(&got), response_bytes(&expected), "{tool} crosses the pipe");
    }
    let expected_caller = DelegationCaller {
        pane_id: "caller".into(),
        kind: "claude".into(),
        provider: "claude".into(),
        delegation: true,
        permission_mode: String::new(),
    };
    assert!(
        calls.lock().unwrap().iter().all(|(request, who)| *who == expected_caller
            && request.arguments["text"] == "한글 \"quoted\""
            && request.arguments.len() == 2),
        "the caller and the arguments are the pane's"
    );
    // An authenticated delegation call may take past the terminal tools' 30 s: the coordinator answers within 45.
    let slow = bind("slow", "claude", Some(Arc::new(Canned::new(Duration::from_secs(31)))), true);
    assert_eq!(
        send(request("slow", &[]), &slow).await,
        DelegationResponse::failure("slow answer"),
        "a call answered after 31 s still gets its answer"
    );
    pipe.close().await;
}
