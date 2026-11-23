//! MC-34: the OS sandbox of a workspace's agent runs (opt-in per workspace, off by default).
//!
//! On the Mac a run's CLI is started by `/usr/bin/sandbox-exec -f <profile>`: the profile ([`profile`]) lets it write only in its
//! folder, its own folders, the temporary folders and package caches, keeps git's config and hooks and the instruction files
//! read-only, and closes the secret folders. With the allow list on, it may connect only to its own allow-list proxy ([`proxy`]) on
//! `127.0.0.1`; a host the list does not cover raises a card ([`gate`]). An optional reviewer agent ([`reviewer`]) may settle a
//! request in a safe category on its own; anything else stays the person's.
//!
//! Not sandboxed: shell panes (the person's own commands), SSH workspaces (the CLI runs on the far machine), remote workspaces (on
//! their host) and Windows (no sandbox yet: `docs/sandbox.md` has the options). On a Mac without `sandbox-exec` a sandboxed run is
//! refused (never started unsandboxed by surprise).

pub mod gate;
pub mod hosts;
pub mod profile;
pub mod proxy;
pub mod reviewer;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mighty_core::model::{SandboxConfig, Workspace};
use mighty_platform::{BoxFuture, SpawnRequest};

use crate::model::StartRunRequest;
use gate::{GateHooks, GatePolicy, NetworkGate};
use hosts::AllowList;
use proxy::Proxy;

/// Seatbelt's launcher (deprecated by Apple, still shipped and used by the agents' own sandboxes).
pub const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

/// The sandbox exists on this OS (the Mac).
pub fn supported() -> bool {
    cfg!(target_os = "macos")
}

/// The sandbox can be used now: the Mac, with `sandbox-exec` present.
pub fn available() -> bool {
    supported() && Path::new(SANDBOX_EXEC).is_file()
}

/// What the host gives one sandboxed run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxSetup {
    pub config: SandboxConfig,
    pub home: PathBuf,
    /// The app's data folder (closed to the run but its agent IO folder).
    pub data_dir: Option<PathBuf>,
    /// Where the run's profile is written (made 0700; the profile 0600).
    pub profile_dir: PathBuf,
}

/// One thing the sandbox refused, as the pane and the phone show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    /// `network` or `tool`.
    pub kind: String,
    /// `example.com:443`, or the tool's summary.
    pub what: String,
    pub reason: String,
    pub at: String,
}

/// The app's side of the sandbox (src-tauri's sandbox.rs).
pub trait SandboxHost: Send + Sync {
    /// The sandbox of a run of `workspace`, or None when it is off (or does not apply to this run).
    fn setup(&self, workspace: &Workspace, request: &StartRunRequest) -> Option<SandboxSetup>;
    /// Something was refused.
    fn blocked(&self, session_id: &str, block: &Block);
    /// The reviewer's answer about `review` (None: no reviewer can run now). The run bounds it by [`reviewer::TIME_LIMIT`].
    fn review(&self, config: &SandboxConfig, review: reviewer::Review) -> Option<BoxFuture<'static, Result<reviewer::Opinion, String>>>;
}

/// A run's sandbox while it runs: its profile file, its proxy and gate. [`RunSandbox::close`] at the run's end.
pub struct RunSandbox {
    pub config: SandboxConfig,
    pub folder: PathBuf,
    pub profile: PathBuf,
    pub proxy: Option<Proxy>,
    pub gate: Option<Arc<NetworkGate>>,
}

impl RunSandbox {
    pub fn reviewer_on(&self) -> bool {
        self.config.reviewer_on()
    }

    /// The run ended: waiting cards withdrawn, the proxy stopped, the profile removed.
    pub fn close(&self) {
        if let Some(gate) = &self.gate {
            gate.close();
        }
        if let Some(proxy) = &self.proxy {
            proxy.stop();
        }
        let _ = std::fs::remove_file(&self.profile);
    }
}

fn port_of(url: &str) -> Option<u16> {
    let rest = url.strip_prefix("http://127.0.0.1:").or_else(|| url.strip_prefix("http://localhost:"))?;
    rest.split(['/', '?']).next()?.parse().ok()
}

/// Writes `text` as a new 0600 file in `dir` (made 0700).
fn write_private(dir: &Path, name: &str, text: &str) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        let path = dir.join(name);
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path)?;
        std::io::Write::write_all(&mut file, text.as_bytes())?;
        Ok(path)
    }
    #[cfg(not(unix))]
    {
        let path = dir.join(name);
        std::fs::write(&path, text)?;
        Ok(path)
    }
}

/// The profile input of a run whose launch is `launch` (its environment names the bridge, the agent IO socket, the CLI's own home).
pub fn profile_input(
    setup: &SandboxSetup,
    provider: &str,
    folder: &Path,
    env: &BTreeMap<String, String>,
    ports: Vec<u16>,
) -> profile::ProfileInput {
    let read = |name: &str| env.get(name).cloned().or_else(|| std::env::var(name).ok()).filter(|v| !v.is_empty());
    let mut ports = ports;
    ports.extend(read("MIGHTY_CLAUDE_BRIDGE_URL").as_deref().and_then(port_of));
    for key in ["PORT", "MIGHTY_PORT"] {
        ports.extend(read(key).and_then(|p| p.parse::<u16>().ok()));
    }
    let state_overrides =
        ["CODEX_HOME", "HERMES_HOME"].iter().filter_map(|k| read(k)).map(PathBuf::from).filter(|p| p.is_absolute()).collect();
    let sockets = read("MIGHTY_AGENT_IO_SOCKET").map(PathBuf::from).into_iter().collect();
    profile::ProfileInput {
        provider: provider.to_string(),
        home: setup.home.clone(),
        folder: folder.to_path_buf(),
        temp: read("TMPDIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir),
        data_dir: setup.data_dir.clone(),
        state_overrides,
        sockets,
        ports,
        limit_network: setup.config.limit_network_on(),
        protect_instructions: setup.config.protect_instructions_on(),
        read_only: setup.config.read_only_paths.clone(),
    }
}

/// `launch` started by `sandbox-exec -f <profile>`, with no ssh agent to talk to.
pub fn wrap(launch: &mut SpawnRequest, profile: &Path) {
    let mut args = vec!["-f".to_string(), profile.to_string_lossy().into_owned(), launch.program.to_string_lossy().into_owned()];
    args.append(&mut launch.args);
    launch.program = PathBuf::from(SANDBOX_EXEC);
    launch.args = args;
    launch.env.remove("SSH_AUTH_SOCK");
    launch.env_remove.push("SSH_AUTH_SOCK".into());
    launch.env.insert("MIGHTY_SANDBOX".into(), "1".into());
}

/// Starts a run's sandbox: its proxy and gate (with the allow list on), its profile, and `launch` wrapped. Err with the reason
/// (the run is then refused: a person who turned the sandbox on never gets an unsandboxed run by surprise).
pub async fn prepare(
    setup: &SandboxSetup,
    provider: &str,
    session_id: &str,
    folder: &Path,
    launch: &mut SpawnRequest,
    hooks: GateHooks,
) -> Result<RunSandbox, String> {
    let folder = profile::canonical(folder).ok_or_else(|| "the run folder is missing".to_string())?;
    let list = AllowList::new(provider, &setup.config.allowed_hosts);
    let (gate, proxy) = if setup.config.limit_network_on() {
        let gate = NetworkGate::new(session_id, list.clone(), hooks);
        let proxy = Proxy::start(Arc::new(GatePolicy(gate.clone()))).await.map_err(|e| e.to_string())?;
        for (name, value) in proxy.environment() {
            launch.env.insert(name, value);
        }
        (Some(gate), Some(proxy))
    } else {
        (None, None)
    };
    let mut ports: Vec<u16> = proxy.iter().map(|p| p.port).collect();
    ports.extend(list.loopback_ports());
    let input = profile_input(setup, provider, &folder, &launch.env, ports);
    // The CLI's own folders exist before it starts: inside the sandbox it may write in them but not make their parents.
    for place in profile::state_folders(provider).iter().map(|f| setup.home.join(f)).chain(input.state_overrides.iter().cloned()) {
        let _ = std::fs::create_dir_all(place);
    }
    let text = profile::generate(&input).map_err(|e| e.to_string())?;
    let name = format!(
        "{}-{}.sb",
        session_id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').take(64).collect::<String>(),
        mighty_core::wire::new_id()
    );
    let path = write_private(&setup.profile_dir, &name, &text).map_err(|e| e.to_string())?;
    wrap(launch, &path);
    Ok(RunSandbox { config: setup.config.clone(), folder, profile: path, proxy, gate })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_launch_is_wrapped_by_sandbox_exec() {
        let mut launch = SpawnRequest::new("/usr/local/bin/claude", ["--print", "hi"], "/tmp");
        launch.env.insert("SSH_AUTH_SOCK".into(), "/tmp/agent".into());
        wrap(&mut launch, Path::new("/data/sandbox/p.sb"));
        assert_eq!(launch.program, PathBuf::from(SANDBOX_EXEC));
        assert_eq!(launch.args, ["-f", "/data/sandbox/p.sb", "/usr/local/bin/claude", "--print", "hi"]);
        assert!(!launch.env.contains_key("SSH_AUTH_SOCK") && launch.env_remove.contains(&"SSH_AUTH_SOCK".to_string()));
        assert_eq!(launch.env.get("MIGHTY_SANDBOX").map(String::as_str), Some("1"));
    }

    #[test]
    #[cfg(unix)]
    fn the_runs_own_ports_and_sockets_are_found_in_its_environment() {
        let setup = SandboxSetup {
            config: SandboxConfig { enabled: true, ..SandboxConfig::default() },
            home: "/Users/me".into(),
            data_dir: None,
            profile_dir: "/tmp/x".into(),
        };
        let mut env = BTreeMap::new();
        env.insert("MIGHTY_CLAUDE_BRIDGE_URL".to_string(), "http://127.0.0.1:41234/events".to_string());
        env.insert("PORT".to_string(), "3100".to_string());
        env.insert("MIGHTY_AGENT_IO_SOCKET".to_string(), "/data/agent-io/io.sock".to_string());
        env.insert("CODEX_HOME".to_string(), "/Users/me/.codex-work".to_string());
        env.insert("HERMES_HOME".to_string(), "relative".to_string());
        let input = profile_input(&setup, "codex", Path::new("/w"), &env, vec![5000]);
        assert_eq!(input.ports, [5000, 41234, 3100]);
        assert_eq!(input.sockets, [PathBuf::from("/data/agent-io/io.sock")]);
        assert_eq!(input.state_overrides, [PathBuf::from("/Users/me/.codex-work")]);
        assert!(input.limit_network && input.protect_instructions);
        assert_eq!(port_of("http://localhost:8/x"), Some(8));
        assert_eq!(port_of("https://127.0.0.1:8"), None);
    }
}
