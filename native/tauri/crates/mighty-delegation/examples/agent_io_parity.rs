//! The Rust side of the Agent IO parity check (tools/parity/AgentIo.cs is C#'s; tools/parity/mcp.ps1 runs both and compares bytes):
//! `agent_io_parity <in folder> <out folder>` answers `bindings.jsonl`, `terminal.jsonl` and `delegation.jsonl` line by line into
//! `bindings.txt`, `terminal.txt` and `delegation.txt` (see AgentIo.cs for the formats). W7-C.

use std::path::Path;
use std::sync::Arc;

use mighty_core::json::JsonOut;
use mighty_delegation::agent_io::binding::AgentIoBindings;
use mighty_delegation::agent_io::wire::MAXIMUM_BYTES;
use mighty_delegation::mcp::delegation::DelegationServer;
use mighty_delegation::mcp::terminal::TerminalServer;
use mighty_delegation::modes::terminal_transport;
use mighty_delegation::request::{DelegationCaller, DelegationHandler, DelegationRequest, DelegationResponse};
use mighty_platform::BoxFuture;
use mighty_platform::unsupported::Unsupported;
use serde_json::Value;

/// A handler the bindings connect to (their arguments never ask it anything).
struct Answers;

impl DelegationHandler for Answers {
    fn handle(&self, _request: DelegationRequest, _caller: DelegationCaller) -> BoxFuture<'static, DelegationResponse> {
        Box::pin(async { DelegationResponse::default() })
    }
}

fn oversize() -> String {
    format!(r#"{{"jsonrpc":"2.0","id":1,"method":"ping","x":"{}"}}"#, "x".repeat(MAXIMUM_BYTES))
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [input, output] = args.as_slice() else {
        eprintln!("agent_io_parity <in folder> <out folder>");
        std::process::exit(2);
    };
    let (input, output) = (Path::new(input), Path::new(output));
    std::fs::create_dir_all(output).unwrap();
    let mut bindings = vec![];
    for line in read(&input.join("bindings.jsonl")).lines().filter(|line| !line.is_empty()) {
        let vector: Value = serde_json::from_str(line).unwrap();
        let text = |name: &str| vector[name].as_str().unwrap().to_string();
        let registry = AgentIoBindings::default();
        let temp = std::env::temp_dir().to_string_lossy().into_owned();
        let binding = registry.bind("pane", "ws", &temp, &text("provider"), "pipe", &text("executable")).unwrap();
        binding.connect_delegation(Some(Arc::new(Answers)), vector["attached"] == true, &text("mode"));
        let mut arguments = vec![];
        binding.append_arguments(&mut arguments);
        let mut out = JsonOut::new();
        out.strings(&arguments);
        bindings.push(out.finish());
    }
    std::fs::write(output.join("bindings.txt"), bindings.join("\n") + "\n").unwrap();
    let terminal = TerminalServer::new(terminal_transport(Arc::new(Unsupported), String::new(), String::new()));
    let mut answers = vec![];
    for line in read(&input.join("terminal.jsonl")).split('\n').filter(|line| !line.is_empty()) {
        let line = if line == "@oversize" { oversize() } else { line.to_string() };
        answers.push(terminal.handle(&line).await.unwrap_or_else(|| "<none>".into()));
    }
    std::fs::write(output.join("terminal.txt"), answers.join("\n") + "\n").unwrap();
    let delegation = DelegationServer::new(None);
    let mut answers = vec![];
    for line in read(&input.join("delegation.jsonl")).split('\n').filter(|line| !line.is_empty()) {
        let line = if line == "@oversize" { oversize() } else { line.to_string() };
        answers.push(delegation.handle(&line).await.unwrap_or_else(|| "<none>".into()));
    }
    std::fs::write(output.join("delegation.txt"), answers.join("\n") + "\n").unwrap();
}
