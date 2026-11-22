//! `AgentIOBinding` (C# Core/AgentIO/AgentIOBinding.cs:1-118; RunManager.cs:374-380): the pane token (32 random bytes as 64 hex),
//! `MIGHTY_PANE_TOKEN` and the transport's key, Claude's `--mcp-config` and Codex's `-c` lists per mode, `ConnectDelegation`, `Redact`,
//! the bindings and their revocation; the `RunBinding` builder mighty-run's `BindingFactory` returns. Owner: W7-C.
//!
//! The token never travels in a CLI's arguments (every local user can read those) and never in the environment of the commands an agent
//! runs (one of them could reach the agent IO transport on it without the CLI's approval): Claude's servers come from a private config
//! file with the token in each server's own `env`, deleted when the run ends; Codex keeps it in its own environment, forwards it to the
//! servers by name and keeps `MIGHTY_*` from its shell tool ([`super::mcp_config`]). Never logged; [`AgentIoBinding`]'s `Debug` hides
//! it and [`AgentIoBinding::redact`] takes it out of what the run prints.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use mighty_core::json::JsonOut;
use mighty_core::normalize::fully_qualified;
use mighty_core::wire::identifier;
use mighty_platform::McpTransport;
use mighty_run::manager::RunBinding;
use tokio::sync::Notify;

use super::mcp_config;
use crate::mcp::delegation::{HEADLESS_ARGUMENT, SERVER_NAME as DELEGATION_SERVER};
use crate::mcp::workspace::{HEADLESS_ARGUMENT as WORKSPACE_HEADLESS_ARGUMENT, SERVER_NAME as WORKSPACE_SERVER, WorkspaceHandler};
use crate::modes::AGENT_IO_MCP_ARGUMENT;
use crate::request::DelegationHandler;

/// `AgentIOBinding.ServerName`.
pub const SERVER_NAME: &str = "mighty-terminal";
/// `AgentIOBinding.TokenKey`.
pub const TOKEN_KEY: &str = "MIGHTY_PANE_TOKEN";
/// What `Redact` puts in the token's place.
pub const REDACTED: &str = "<pane-token-redacted>";
/// `AgentIOBinding.CodexApprovedInAcceptEdits`: the mighty-terminal tools a Codex acceptEdits run may call without asking (they read
/// what already ran; run_in_terminal, stop and open_url are not among them: a page opened in the user's browser carries what its
/// address says out of the sandbox, network off or not, so it asks like the others).
pub const CODEX_APPROVED_IN_ACCEPT_EDITS: [&str; 1] = ["read_latest_output"];

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A `CancellationTokenSource` that is only ever cancelled: set once, awaited by anyone.
#[derive(Debug, Default)]
pub struct Revocation {
    done: AtomicBool,
    notify: Notify,
}

impl Revocation {
    pub fn cancel(&self) {
        self.done.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    pub fn is_cancelled(&self) -> bool {
        self.done.load(Ordering::SeqCst)
    }

    /// Ends once [`Revocation::cancel`] was called (at once if it already was).
    pub async fn cancelled(&self) {
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.is_cancelled() {
                return;
            }
            notified.await;
        }
    }
}

/// `DelegationProviders.CodexApprovesTools` and `AttachesDelegation`: the delegation core's rules (W7-A's [`crate::providers`]).
pub use crate::providers::{attaches_delegation, codex_approves_tools};

/// What `ConnectDelegation` (and `ConnectWorkspace`) set: who answers the run's delegation/workspace calls, the mode it started in,
/// and whether each optional server was attached.
#[derive(Default)]
struct Link {
    handler: Option<Arc<dyn DelegationHandler>>,
    workspace_handler: Option<Arc<dyn WorkspaceHandler>>,
    permission_mode: String,
    delegation: bool,
    workspace: bool,
}

/// `AgentIOBinding`: one agent run's pane binding. Never part of saved state; its token is never shown.
pub struct AgentIoBinding {
    pub pane_id: String,
    pub workspace_id: String,
    pub workspace_path: String,
    pub provider: String,
    pub pipe_name: String,
    pub executable: String,
    /// The environment key the transport's name travels in (`MIGHTY_AGENT_IO_PIPE` on Windows).
    pub transport_key: &'static str,
    token: String,
    revoked: Revocation,
    link: Mutex<Link>,
    /// Where Claude's config file goes, and the one written for this binding until it is deleted.
    config_folder: PathBuf,
    config_file: Mutex<Option<PathBuf>>,
}

impl Drop for AgentIoBinding {
    fn drop(&mut self) {
        self.remove_config();
    }
}

impl fmt::Debug for AgentIoBinding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AgentIOBinding({}, token=<redacted>)", self.pane_id)
    }
}

impl AgentIoBinding {
    #[allow(clippy::too_many_arguments)]
    fn new(
        pane_id: &str,
        workspace_id: &str,
        workspace_path: &str,
        provider: &str,
        pipe_name: &str,
        executable: &str,
        transport: McpTransport,
        config_folder: PathBuf,
    ) -> AgentIoBinding {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).expect("the OS's random numbers");
        AgentIoBinding {
            pane_id: pane_id.into(),
            workspace_id: workspace_id.into(),
            workspace_path: workspace_path.into(),
            provider: provider.into(),
            pipe_name: pipe_name.into(),
            executable: executable.into(),
            transport_key: transport.env_key(),
            token: bytes.iter().map(|b| format!("{b:02x}")).collect(),
            revoked: Revocation::default(),
            link: Mutex::new(Link::default()),
            config_folder,
            config_file: Mutex::new(None),
        }
    }

    /// Claude's config file deleted, if one was written (the run ended, the binding was revoked or dropped).
    pub fn remove_config(&self) {
        if let Some(path) = lock(&self.config_file).take() {
            let _ = std::fs::remove_file(path);
        }
    }

    /// The config file written for this binding, while it is there.
    pub fn config_file(&self) -> Option<PathBuf> {
        lock(&self.config_file).clone()
    }

    /// `Kind`: bindings are only made for agent-pane runs.
    pub fn kind(&self) -> &'static str {
        "claude"
    }

    /// `Revoked`: cancelled once a new run rebinds the pane, the pane goes, or the app quits.
    pub fn revoked(&self) -> &Revocation {
        &self.revoked
    }

    pub fn is_revoked(&self) -> bool {
        self.revoked.is_cancelled()
    }

    /// `Delegation`: this run also gets the delegation server on the same token.
    pub fn delegation(&self) -> bool {
        lock(&self.link).delegation
    }

    /// `PermissionMode`: the mode the run started in, what its delegation calls are judged by.
    pub fn permission_mode(&self) -> String {
        lock(&self.link).permission_mode.clone()
    }

    /// `DelegationHandler`: who answers its delegation calls; None answers them as a run without the server.
    pub fn delegation_handler(&self) -> Option<Arc<dyn DelegationHandler>> {
        lock(&self.link).handler.clone()
    }

    /// `ConnectDelegation(handler, attached, permissionMode)`: the run path connects a new binding before its CLI starts.
    pub fn connect_delegation(&self, handler: Option<Arc<dyn DelegationHandler>>, attached: bool, permission_mode: &str) {
        let delegation = attached && attaches_delegation(self.kind(), &self.provider, permission_mode);
        let mut link = lock(&self.link);
        link.handler = handler;
        link.permission_mode = permission_mode.into();
        link.delegation = delegation;
    }

    /// Whether this run also gets the read-only `mighty-workspace` server on the same token.
    pub fn workspace(&self) -> bool {
        lock(&self.link).workspace
    }

    /// Who answers this run's `mighty-workspace` calls; None answers them as detached.
    pub fn workspace_handler(&self) -> Option<Arc<dyn WorkspaceHandler>> {
        lock(&self.link).workspace_handler.clone()
    }

    /// Connects the optional read-only `mighty-workspace` MCP server before the CLI starts.
    pub fn connect_workspace(&self, handler: Option<Arc<dyn WorkspaceHandler>>, enabled: bool) {
        let attached = enabled && handler.is_some() && matches!(self.provider.as_str(), "claude" | "codex");
        let mut link = lock(&self.link);
        link.workspace_handler = handler;
        link.workspace = attached;
    }

    /// `Environment`: the token and the transport's name, what the MCP servers' processes are started with: Claude's config file names
    /// them in each server's `env`; a Codex run's own environment holds them (its shell tool's commands do not get them).
    pub fn environment(&self) -> BTreeMap<String, String> {
        BTreeMap::from([(TOKEN_KEY.to_string(), self.token.clone()), (self.transport_key.to_string(), self.pipe_name.clone())])
    }

    /// `Redact`: the token, wherever it appears, made [`REDACTED`].
    pub fn redact(&self, value: &str) -> String {
        value.replace(&self.token, REDACTED)
    }

    /// `AppendArguments`: Claude's `--mcp-config` naming a private file written now (Wire.Json, a non-ASCII path written `\uXXXX`; each
    /// server with [`AgentIoBinding::environment`] as its `env`; none when the file cannot be written: the run then has no pane tools),
    /// Codex's `-c` settings per mode with `shell_environment_policy.exclude`.
    pub fn append_arguments(&self, arguments: &mut Vec<String>) {
        let delegation = self.delegation();
        let workspace = self.workspace();
        let mode = self.permission_mode();
        let environment = self.environment();
        if self.provider == "claude" {
            let mut out = JsonOut::new();
            out.begin_object();
            out.name("mcpServers");
            out.begin_object();
            let mut servers = vec![(SERVER_NAME, AGENT_IO_MCP_ARGUMENT)];
            // The delegation and workspace servers are their own entries, so mighty-terminal keeps its four tools.
            if delegation {
                servers.push((DELEGATION_SERVER, HEADLESS_ARGUMENT));
            }
            if workspace {
                servers.push((WORKSPACE_SERVER, WORKSPACE_HEADLESS_ARGUMENT));
            }
            for (name, argument) in servers {
                out.name(name);
                out.begin_object();
                out.prop_str("type", Some("stdio"));
                out.prop_str("command", Some(&self.executable));
                out.name("args");
                out.strings(&[argument.to_string()]);
                out.name("env");
                out.begin_object();
                for (key, value) in &environment {
                    out.prop_str(key, Some(value));
                }
                out.end_object();
                out.end_object();
            }
            out.end_object();
            out.end_object();
            self.remove_config();
            if let Ok(path) = mcp_config::write(&self.config_folder, &out.finish()) {
                arguments.extend(["--mcp-config".to_string(), path.to_string_lossy().into_owned()]);
                *lock(&self.config_file) = Some(path);
            }
        } else if self.provider == "codex" {
            // By the run's mode, some tools are approved ahead: codex exec (no asking) otherwise fails every MCP call that needs an
            // approval it cannot ask for. fullAccess approves all servers' tools; acceptEdits approves the delegation server's, the
            // read-only workspace server's, and of mighty-terminal's only reading output; manual approves none, onRequest leaves it to
            // the human. The servers get the token from Codex's environment by name; its shell tool's commands do not
            // (`shell_environment_policy.exclude`).
            let mut servers = vec![(SERVER_NAME, AGENT_IO_MCP_ARGUMENT)];
            if delegation {
                servers.push((DELEGATION_SERVER, HEADLESS_ARGUMENT));
            }
            if workspace {
                servers.push((WORKSPACE_SERVER, WORKSPACE_HEADLESS_ARGUMENT));
            }
            let json = |write: &dyn Fn(&mut JsonOut)| {
                let mut out = JsonOut::new();
                write(&mut out);
                out.finish()
            };
            for (name, argument) in servers {
                let pairs = [
                    ("command", json(&|out| out.str(&self.executable))),
                    ("args", json(&|out| out.strings(&[argument.to_string()]))),
                    ("env_vars", json(&|out| out.strings(&[TOKEN_KEY.to_string(), self.transport_key.to_string()]))),
                ];
                for (key, value) in pairs {
                    arguments.extend(["-c".to_string(), format!("mcp_servers.{name}.{key}={value}")]);
                }
                if mode == "fullAccess" || (name == DELEGATION_SERVER || name == WORKSPACE_SERVER) && codex_approves_tools(&mode) {
                    arguments.extend(["-c".to_string(), format!("mcp_servers.{name}.default_tools_approval_mode=\"approve\"")]);
                } else if name == SERVER_NAME && mode == "acceptEdits" {
                    for tool in CODEX_APPROVED_IN_ACCEPT_EDITS {
                        arguments.extend(["-c".to_string(), format!("mcp_servers.{name}.tools.{tool}.approval_mode=\"approve\"")]);
                    }
                }
            }
            let home = mcp_config::codex_home_config();
            let project = mcp_config::codex_project_config(std::path::Path::new(&self.workspace_path));
            if let Some(exclude) = mcp_config::codex_shell_exclude_with_project(home.as_deref(), project.as_deref()) {
                arguments.extend(["-c".to_string(), format!("shell_environment_policy.exclude={exclude}")]);
            }
        }
    }

    /// What mighty-run lays over the run (C# RunManager.cs:376-379): the environment (a Codex run's only; a Claude CLI's environment
    /// never holds the token), the arguments, the redaction, and the config file's deletion when the run ends.
    pub fn run_binding(self: &Arc<Self>) -> RunBinding {
        let mut arguments = vec![];
        self.append_arguments(&mut arguments);
        let (binding, ending) = (self.clone(), self.clone());
        RunBinding {
            environment: if self.provider == "codex" { self.environment() } else { BTreeMap::new() },
            arguments,
            redact: Arc::new(move |line: &str| binding.redact(line)),
            ended: Some(Box::new(move || ending.remove_config())),
        }
    }
}

/// Why `Bind` refused (C#'s "Invalid pane binding.").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidBinding;

impl fmt::Display for InvalidBinding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Invalid pane binding.")
    }
}

impl std::error::Error for InvalidBinding {}

/// `AgentIOBindings`: the live binding of each pane.
#[derive(Debug)]
pub struct AgentIoBindings {
    transport: McpTransport,
    bindings: Mutex<HashMap<String, Arc<AgentIoBinding>>>,
    /// Where Claude runs' config files go ([`mcp_config::fallback_folder`] until the agent IO names its own).
    config_folder: Mutex<PathBuf>,
}

impl Default for AgentIoBindings {
    fn default() -> Self {
        AgentIoBindings::new(McpTransport::NamedPipe)
    }
}

impl AgentIoBindings {
    pub fn new(transport: McpTransport) -> AgentIoBindings {
        AgentIoBindings { transport, bindings: Mutex::new(HashMap::new()), config_folder: Mutex::new(mcp_config::fallback_folder()) }
    }

    /// Claude runs' config files from now on go in `folder` (the agent IO's own private folder).
    pub fn set_config_folder(&self, folder: PathBuf) {
        *lock(&self.config_folder) = folder;
    }

    /// Where Claude runs' config files go.
    pub fn config_folder(&self) -> PathBuf {
        lock(&self.config_folder).clone()
    }

    /// `Bind`: a new binding for the pane's run, revoking the one before. The pane and workspace ids are identifiers, the workspace's
    /// path and the executable full paths, the provider claude or codex.
    pub fn bind(
        &self,
        pane: &str,
        workspace_id: &str,
        workspace_path: &str,
        provider: &str,
        pipe: &str,
        executable: &str,
    ) -> Result<Arc<AgentIoBinding>, InvalidBinding> {
        if !identifier(pane)
            || !identifier(workspace_id)
            || !fully_qualified(workspace_path)
            || !fully_qualified(executable)
            || !matches!(provider, "claude" | "codex")
        {
            return Err(InvalidBinding);
        }
        let folder = self.config_folder();
        let value = Arc::new(AgentIoBinding::new(pane, workspace_id, workspace_path, provider, pipe, executable, self.transport, folder));
        let previous = lock(&self.bindings).insert(pane.to_string(), value.clone());
        if let Some(previous) = previous {
            previous.revoked.cancel();
            previous.remove_config();
        }
        Ok(value)
    }

    /// `Resolve`: the binding of this token, compared in fixed time; None for any token that is not 64 characters.
    pub fn resolve(&self, token: &str) -> Option<Arc<AgentIoBinding>> {
        if token.len() != 64 {
            return None;
        }
        lock(&self.bindings).values().find(|value| fixed_time_equals(value.token.as_bytes(), token.as_bytes())).cloned()
    }

    /// `IsCurrent`: not revoked and still the pane's binding.
    pub fn is_current(&self, value: &Arc<AgentIoBinding>) -> bool {
        !value.is_revoked() && lock(&self.bindings).get(&value.pane_id).is_some_and(|current| Arc::ptr_eq(current, value))
    }

    /// The panes bound now.
    pub fn panes(&self) -> Vec<String> {
        lock(&self.bindings).keys().cloned().collect()
    }

    /// `Revoke`: the pane's binding goes (its pane closed, or its child's terminal ended).
    pub fn revoke(&self, pane: &str) {
        let previous = lock(&self.bindings).remove(pane);
        if let Some(previous) = previous {
            previous.revoked.cancel();
            previous.remove_config();
        }
    }

    /// `Clear`: every binding goes (the app quits).
    pub fn clear(&self) {
        let previous: Vec<_> = lock(&self.bindings).drain().map(|(_, value)| value).collect();
        for value in previous {
            value.revoked.cancel();
            value.remove_config();
        }
    }
}

/// `CryptographicOperations.FixedTimeEquals`: the time taken depends only on the lengths.
fn fixed_time_equals(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}
