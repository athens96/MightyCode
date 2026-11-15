//! C# DelegationAppVerification, W7-C's two (Verification.cs L671, L673): the binding attaches the delegation server only where it
//! should, byte for byte, and the headless `--agent-delegation-mcp` server lists the six tools and forwards over the pipe. The pipe's
//! own check (L672, over 31 s) is tests/pipe_delegation.rs.

mod delegation_io;

use std::sync::Arc;

use delegation_io::{IoHandler, executable, temp};
use mighty_core::json::JsonOut;
use mighty_delegation::agent_io::binding::{AgentIoBindings, SERVER_NAME, TOKEN_KEY};
use mighty_delegation::mcp::delegation::{DelegationServer, HEADLESS_ARGUMENT, MANIFEST, SERVER_NAME as DELEGATION_SERVER};
use mighty_delegation::request::{DelegationHandler, DelegationReasonCode, DelegationResponse};
use mighty_run::model::StartRunRequest;
use serde_json::Value;

fn servers(arguments: &[String]) -> serde_json::Map<String, Value> {
    assert_eq!(arguments.iter().filter(|a| *a == "--mcp-config").count(), 1, "one --mcp-config");
    let at = arguments.iter().position(|a| a == "--mcp-config").unwrap();
    let config: Value = serde_json::from_str(&arguments[at + 1]).unwrap();
    config["mcpServers"].as_object().unwrap().clone()
}

/// `JsonSerializer.Serialize(value, Wire.Json)` of a string.
fn wire(value: &str) -> String {
    let mut out = JsonOut::new();
    out.str(value);
    out.finish()
}

/// "delegation: the binding attaches the server only to a Claude run with the switch on".
#[test]
fn the_binding_attaches_the_server_only_to_a_claude_run_with_the_switch_on() {
    let registry = AgentIoBindings::default();
    let on: Arc<dyn DelegationHandler> = Arc::new(IoHandler { on: true });
    let exe = executable();
    let bind = |pane: &str, provider: &str| registry.bind(pane, "ws", &temp(), provider, "pipe", &exe).unwrap();
    // Off, the default: byte for byte the config before delegation.
    let off = bind("pane-a", "claude");
    let mut arguments = vec![];
    off.append_arguments(&mut arguments);
    assert_eq!(
        arguments[1],
        format!(r#"{{"mcpServers":{{"mighty-terminal":{{"type":"stdio","command":{},"args":["--agent-io-mcp"]}}}}}}"#, wire(&exe)),
        "switched off, the config is the one before"
    );
    assert!(
        !off.delegation() && off.delegation_handler().is_none() && !arguments.join(" ").contains(DELEGATION_SERVER),
        "a binding no run connected is detached"
    );
    // On: the delegation server beside mighty-terminal, on the same token, which reaches it only through the environment.
    let attached = bind("pane-b", "claude");
    attached.connect_delegation(Some(on.clone()), true, "");
    arguments.clear();
    attached.append_arguments(&mut arguments);
    let listed = servers(&arguments);
    let mut names: Vec<&String> = listed.keys().collect();
    names.sort();
    assert_eq!(names, [DELEGATION_SERVER, SERVER_NAME], "both servers");
    let at = arguments.iter().position(|a| a == "--mcp-config").unwrap();
    assert!(
        arguments[at + 1]
            .contains(&format!(r#""{DELEGATION_SERVER}":{{"type":"stdio","command":{},"args":["--agent-delegation-mcp"]}}"#, wire(&exe))),
        "the delegation server is the app binary's headless mode, with no env of its own"
    );
    assert_eq!(listed[SERVER_NAME]["args"], serde_json::json!(["--agent-io-mcp"]), "mighty-terminal is as it was");
    let token = attached.environment()[TOKEN_KEY].clone();
    assert!(
        attached.delegation()
            && attached.kind() == "claude"
            && attached.provider == "claude"
            && !arguments.join(" ").contains(&token)
            && !arguments.iter().any(|a| a == "--strict-mcp-config"),
        "the token stays in the environment"
    );
    // A Codex run in manual (or with no mode) never names it, even when the run path asks, and approves no tool ahead.
    let codex = bind("pane-c", "codex");
    codex.connect_delegation(Some(on.clone()), true, "manual");
    arguments.clear();
    codex.append_arguments(&mut arguments);
    let joined = arguments.join(" ");
    assert!(
        !codex.delegation()
            && codex.delegation_handler().is_some()
            && !joined.contains(DELEGATION_SERVER)
            && !joined.contains(HEADLESS_ARGUMENT)
            && !joined.contains("approval_mode"),
        "a Codex manual run gets no delegation server"
    );
    // In acceptEdits it gets the server, whose tools are approved ahead, and of mighty-terminal's only reading output and opening a page.
    let edits = bind("pane-d", "codex");
    edits.connect_delegation(Some(on.clone()), true, "acceptEdits");
    arguments.clear();
    edits.append_arguments(&mut arguments);
    let settings: Vec<&String> = arguments.iter().skip(1).step_by(2).collect();
    let has = |setting: &str| settings.iter().any(|s| *s == setting);
    assert!(
        edits.delegation()
            && arguments.iter().step_by(2).all(|flag| flag == "-c")
            && has(r#"mcp_servers.mighty-delegation.args=["--agent-delegation-mcp"]"#)
            && has(r#"mcp_servers.mighty-delegation.env_vars=["MIGHTY_PANE_TOKEN","MIGHTY_AGENT_IO_PIPE"]"#)
            && has(r#"mcp_servers.mighty-delegation.default_tools_approval_mode="approve""#)
            && !has(r#"mcp_servers.mighty-terminal.default_tools_approval_mode="approve""#)
            && has(r#"mcp_servers.mighty-terminal.tools.read_latest_output.approval_mode="approve""#)
            && has(r#"mcp_servers.mighty-terminal.tools.open_url.approval_mode="approve""#)
            && !settings.iter().any(|s| s.contains("run_in_terminal"))
            && !settings.iter().any(|s| s.contains("developer_instructions")),
        "a Codex acceptEdits run gets the server, and approvals that let its tools work"
    );
    // The whole list, in C#'s order.
    let command = wire(&exe);
    let expected: Vec<String> = [
        format!("mcp_servers.mighty-terminal.command={command}"),
        r#"mcp_servers.mighty-terminal.args=["--agent-io-mcp"]"#.to_string(),
        r#"mcp_servers.mighty-terminal.env_vars=["MIGHTY_PANE_TOKEN","MIGHTY_AGENT_IO_PIPE"]"#.to_string(),
        r#"mcp_servers.mighty-terminal.tools.read_latest_output.approval_mode="approve""#.to_string(),
        r#"mcp_servers.mighty-terminal.tools.open_url.approval_mode="approve""#.to_string(),
        format!("mcp_servers.mighty-delegation.command={command}"),
        r#"mcp_servers.mighty-delegation.args=["--agent-delegation-mcp"]"#.to_string(),
        r#"mcp_servers.mighty-delegation.env_vars=["MIGHTY_PANE_TOKEN","MIGHTY_AGENT_IO_PIPE"]"#.to_string(),
        r#"mcp_servers.mighty-delegation.default_tools_approval_mode="approve""#.to_string(),
    ]
    .into_iter()
    .flat_map(|setting| ["-c".to_string(), setting])
    .collect();
    assert_eq!(arguments, expected, "the acceptEdits list byte for byte");
    // fullAccess approves both servers' tools.
    let full = bind("pane-e", "codex");
    full.connect_delegation(Some(on.clone()), true, "fullAccess");
    arguments.clear();
    full.append_arguments(&mut arguments);
    assert!(
        full.delegation()
            && arguments.iter().any(|a| a == r#"mcp_servers.mighty-delegation.default_tools_approval_mode="approve""#)
            && arguments.iter().any(|a| a == r#"mcp_servers.mighty-terminal.default_tools_approval_mode="approve""#),
        "a Codex fullAccess run approves both servers' tools"
    );
    // The switch off detaches it in every mode.
    let detached = bind("pane-f", "codex");
    detached.connect_delegation(Some(on), false, "fullAccess");
    assert!(!detached.delegation(), "a run started with the switch off gets no server");
    // Only the saved record's absolute folder, and only for an agent pane, may be a run's working folder.
    let request = |kind: &str, input: &str, folder: &str| {
        let mut request = StartRunRequest::new("p", "ws", kind, input);
        request.working_folder = Some(folder.to_string());
        request
    };
    for bad in [request("claude", "hi", "relative\\child"), request("shell", "dir", &temp())] {
        assert!(bad.validate().is_err(), "a working folder that is not an agent pane's full path is refused");
    }
    assert_eq!(request("claude", "hi", &temp()).validate().unwrap().working_folder.as_deref(), Some(temp().as_str()));
}

/// The half of "delegation: the headless server lists the six tools and forwards over the pipe" that needs no process: calls that are
/// not well formed never reach the app; one with a lone surrogate is answered all the same.
#[tokio::test]
async fn calls_that_are_not_well_formed_never_reach_the_app() {
    let forwarded = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = forwarded.clone();
    let surface = DelegationServer::new(Some(Arc::new(move |_| {
        counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Box::pin(async { DelegationResponse::refusal(DelegationReasonCode::ClaudeOnly) })
    })));
    let tool_call = |name: &str, arguments: &str| {
        format!(r#"{{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{{"name":"{name}","arguments":{arguments}}}}}"#)
    };
    let parse = |line: Option<String>| serde_json::from_str::<Value>(&line.expect("an answer")).unwrap();
    for name in ["Delegate", "../delegate", "", &"a".repeat(65), "1delegate"] {
        assert_eq!(parse(surface.handle(&tool_call(name, "{}")).await)["error"]["code"], -32602, "the name {name}");
    }
    for arguments in [r#"{"task":42}"#, r#"{"task":["a"]}"#, r#"{"Task":"x"}"#, r#""not an object""#] {
        assert_eq!(parse(surface.handle(&tool_call("delegate", arguments)).await)["result"]["isError"], true, "the arguments {arguments}");
    }
    let lone = format!(r#"{{"task":"\u{}","mode":"plan"}}"#, "d800");
    assert!(
        parse(surface.handle(&tool_call("delegate", &lone)).await).get("error").is_some(),
        "a lone surrogate is answered with an error"
    );
    let alone = DelegationServer::new(None);
    assert_eq!(parse(alone.handle(&tool_call("delegate", r#"{"task":"x","mode":"plan"}"#)).await)["result"]["isError"], true);
    assert_eq!(
        forwarded.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "nothing malformed reached the app, and a server without a pane connection answers alone"
    );
    assert_eq!(MANIFEST.map(|tool| tool.name), ["delegate", "list_children", "child_status", "merge", "follow_up", "discard"]);
}

/// "delegation: the headless server lists the six tools and forwards over the pipe": the exe's `--agent-delegation-mcp` (the checks'
/// relaunched exe) with the binding's environment reaches the app's listener on the pane's token; it leaves with 0 once stdin closes.
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread")]
async fn the_headless_server_lists_the_six_tools_and_forwards_over_the_pipe() {
    use std::io::{BufRead, BufReader, Write};
    use std::time::Duration;

    use delegation_io::Canned;
    use mighty_delegation::agent_io::server::{AgentIo, TerminalCall, TerminalHandler};
    use mighty_delegation::agent_io::tools::ALL;
    use mighty_platform::BoxFuture;

    struct Refuses;
    impl TerminalHandler for Refuses {
        fn handle(&self, _call: TerminalCall) -> BoxFuture<'static, Result<Value, String>> {
            Box::pin(async { Err("no terminal here".to_string()) })
        }
    }

    let canned = Arc::new(Canned::new(Duration::ZERO));
    let calls = canned.calls.clone();
    let pipe = AgentIo::start(Arc::new(mighty_platform_windows::NamedPipeListener::new()), Arc::new(Refuses)).unwrap();
    let binding = pipe.bind("parent", "ws", &temp(), "claude", &executable()).unwrap();
    binding.connect_delegation(Some(canned), true, "");
    #[allow(clippy::disallowed_methods, reason = "the check starts the checks' own exe, as C#'s Verification.Self does")]
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_mighty-delegation-fake"));
    command.arg(HEADLESS_ARGUMENT).current_dir(std::env::temp_dir()).envs(binding.environment());
    command.stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null());
    let mut server = command.spawn().unwrap();
    let mut input = server.stdin.take().unwrap();
    let output = Arc::new(std::sync::Mutex::new(BufReader::new(server.stdout.take().unwrap())));
    let mut ask = |line: &str| {
        writeln!(input, "{line}").unwrap();
        input.flush().unwrap();
        let output = output.clone();
        async move {
            let read = tokio::task::spawn_blocking(move || {
                let mut answer = String::new();
                output.lock().unwrap().read_line(&mut answer).unwrap();
                answer
            });
            let answer = tokio::time::timeout(Duration::from_secs(60), read).await.unwrap().unwrap();
            serde_json::from_str::<Value>(&answer).unwrap()
        }
    };
    let hello = ask(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}}"#).await;
    assert!(
        hello["result"]["serverInfo"]["name"] == DELEGATION_SERVER
            && hello["result"]["protocolVersion"] == "2025-03-26"
            && !hello["result"]["capabilities"]["tools"].is_null(),
        "initialize names the delegation server"
    );
    let tools = ask(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#).await;
    let names: Vec<&str> = tools["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, MANIFEST.map(|tool| tool.name), "exactly the six tools");
    assert!(
        !names.iter().any(|name| ALL.iter().any(|terminal| terminal.name == *name)),
        "the terminal tools stay on mighty-terminal alone"
    );
    let call = ask(
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"delegate","arguments":{"task":"Write the release notes","mode":"plan"}}}"#,
    )
    .await;
    let recorded = calls.lock().unwrap().clone();
    assert!(
        call["result"]["structuredContent"]["child"]["id"] == "c1"
            && recorded.len() == 1
            && recorded[0].0.tool == "delegate"
            && recorded[0].0.arguments["task"] == "Write the release notes"
            && recorded[0].0.arguments["mode"] == "plan"
            && recorded[0].1.pane_id == "parent"
            && recorded[0].1.delegation,
        "the call reached the app with its arguments, on the pane's token"
    );
    drop(input);
    let exited = tokio::task::spawn_blocking(move || server.wait().unwrap());
    let status = tokio::time::timeout(Duration::from_secs(25), exited).await.expect("the server exits when stdin closes").unwrap();
    assert_eq!(status.code(), Some(0), "the server exits when stdin closes");
    pipe.close().await;
}

/// The run service's binding factory (RunManager.cs:374-380 over `ConfigureAgentIO`'s factory): a local Claude run gets the token,
/// the pipe and both servers while the switch is on; a Codex run in manual only mighty-terminal; a remote workspace nothing.
#[tokio::test]
async fn the_binding_factory_binds_local_runs_and_connects_delegation() {
    use mighty_core::model::RunSettings;
    use mighty_core::model::{RemoteWorkspaceReference, Workspace};
    use mighty_delegation::agent_io::server::{AgentIo, TerminalCall, TerminalHandler, binding_factory};
    use mighty_platform::BoxFuture;

    struct Nothing;
    impl TerminalHandler for Nothing {
        fn handle(&self, _call: TerminalCall) -> BoxFuture<'static, Result<Value, String>> {
            Box::pin(async { Err(String::new()) })
        }
    }
    let io = mighty_platform::fake::FakeAgentIo::default();
    let pipe = AgentIo::start(Arc::new(io), Arc::new(Nothing)).unwrap();
    let handler: Arc<dyn DelegationHandler> = Arc::new(IoHandler { on: true });
    let switch = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let (link_handler, link_switch) = (handler.clone(), switch.clone());
    let factory = binding_factory(
        pipe.clone(),
        executable(),
        Arc::new(move || (Some(link_handler.clone()), link_switch.load(std::sync::atomic::Ordering::SeqCst))),
    );
    let workspace = Workspace { id: Some("ws".into()), path: Some(temp()), ..Workspace::default() };
    let claude = StartRunRequest::new("pane", "ws", "claude", "hi");
    let binding = factory(&claude, &workspace).expect("a local Claude run is bound");
    let token = binding.environment["MIGHTY_PANE_TOKEN"].clone();
    assert_eq!(binding.environment["MIGHTY_AGENT_IO_PIPE"], pipe.name());
    assert!(servers(&binding.arguments).contains_key(DELEGATION_SERVER), "the switch on: the delegation server too");
    assert_eq!((binding.redact)(&format!("a{token}b")), "a<pane-token-redacted>b");
    let bound = pipe.bindings().resolve(&token).unwrap();
    assert!(bound.delegation() && bound.delegation_handler().is_some() && bound.permission_mode() == claude.settings().permission_mode);
    switch.store(false, std::sync::atomic::Ordering::SeqCst);
    let off = factory(&claude, &workspace).unwrap();
    assert!(!servers(&off.arguments).contains_key(DELEGATION_SERVER), "the switch off: mighty-terminal alone");
    assert!(pipe.bindings().resolve(&token).is_none(), "the pane's new run revoked the binding before");
    let mut codex = StartRunRequest::new("codex-pane", "ws", "claude", "hi");
    codex.provider = "codex".into();
    codex.settings = Some(RunSettings { permission_mode: "manual".into(), ..RunSettings::default() });
    switch.store(true, std::sync::atomic::Ordering::SeqCst);
    let manual = factory(&codex, &workspace).unwrap();
    assert!(manual.arguments.iter().all(|a| !a.contains(DELEGATION_SERVER)), "a Codex manual run: no delegation server");
    let remote = Workspace {
        remote: Some(RemoteWorkspaceReference {
            connection_id: Some("c".into()),
            workspace_id: Some("w".into()),
            host_name: None,
            version: 1,
            mirror: false,
        }),
        ..workspace.clone()
    };
    assert!(factory(&claude, &remote).is_none(), "a remote workspace's run gets no pane tools");
    let mut pi = claude.clone();
    pi.provider = "pi".into();
    assert!(factory(&pi, &workspace).is_none());
    // A run the registry refuses (a pane id that is not one) gets nothing, and the logged factory says why.
    let said = Arc::new(std::sync::Mutex::new(vec![]));
    let sink = said.clone();
    let logged = mighty_delegation::agent_io::server::logged_binding_factory(
        pipe.clone(),
        executable(),
        Arc::new(|| (None, false)),
        Arc::new(move |line: &str| sink.lock().unwrap().push(line.to_string())),
    );
    let bad = StartRunRequest::new("bad pane", "ws", "claude", "hi");
    assert!(logged(&bad, &workspace).is_none());
    assert_eq!(*said.lock().unwrap(), [r#"agent io: the run of pane "bad pane" gets no pane tools: Invalid pane binding."#]);
    assert!(logged(&claude, &workspace).is_some() && said.lock().unwrap().len() == 1, "a bound run logs nothing");
    pipe.close().await;
}
