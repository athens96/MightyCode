//! The checks' relaunched exe (C# `Verification.Self`, the test exe started again in a second mode): `--agent-io-mcp`,
//! `--agent-delegation-mcp` and `--fake-claude <absolute root> …` handed to the library exactly as the app's MightyCode.exe hands them
//! (src-tauri main.rs), with the real pipe and process host on Windows and the Unix socket and process host on the Mac.
//! Owner: W7-C. Anything else exits 2.

use std::path::Path;
use std::sync::Arc;

use mighty_delegation::{fake_claude, modes};
use mighty_platform::{AgentIoConnector, ProcessHost};

#[cfg(windows)]
fn ports() -> (Arc<dyn AgentIoConnector>, Arc<dyn ProcessHost>) {
    (Arc::new(mighty_platform_windows::NamedPipeConnector), Arc::new(mighty_platform_windows::WindowsProcessHost::new()))
}

/// The Mac's Unix socket (M-C) and process host (M-A).
#[cfg(target_os = "macos")]
fn ports() -> (Arc<dyn AgentIoConnector>, Arc<dyn ProcessHost>) {
    (mighty_platform_macos::agent_io_connector(), mighty_platform_macos::process_host())
}

#[cfg(not(any(windows, target_os = "macos")))]
fn ports() -> (Arc<dyn AgentIoConnector>, Arc<dyn ProcessHost>) {
    (Arc::new(mighty_platform::unsupported::Unsupported), Arc::new(mighty_platform::unsupported::Unsupported))
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (connector, processes) = ports();
    let code = match args.as_slice() {
        [only] if only == modes::AGENT_IO_MCP_ARGUMENT => modes::agent_io_mcp(connector),
        [only] if only == modes::DELEGATION_MCP_ARGUMENT => modes::delegation_mcp(connector),
        [only] if only == modes::WORKSPACE_MCP_ARGUMENT => modes::workspace_mcp(connector),
        [first, root, rest @ ..] if first == fake_claude::ARGUMENT && Path::new(root).is_absolute() => {
            fake_claude::run(Path::new(root), rest, processes)
        }
        _ => 2,
    };
    std::process::exit(code);
}
