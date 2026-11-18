//! What a remote workspace's panes may do (C# `RemoteWorkspaceFeatures`, `RemoteSecretCopy`, R\RemoteWorkspaceFeatures.cs; plan §1.6):
//! delegation, the files pane, the browser pane, styles that read files, the local terminal, agent I/O tools, the status line and the
//! git badge all read this PC's disk, network or processes, so a remote workspace has none of them. The add-pane menu is mighty-core's
//! `add_pane::entries_for`. Owner: W8-D.

use std::time::Duration;

use mighty_core::model::{RunSession, Workspace};
use mighty_run::model::ProviderRuntime;
use mighty_run::providers;
use mighty_run::strings::get_with;

use crate::remote::strings;
use crate::remote::wire::{RemoteConnectionInfo, RemoteConnectionStatus};

/// `LocalPath(workspace)`: the workspace's folder on this PC, None for a remote workspace (whose path is its host's): the one way the
/// window reads a workspace's folder, so nothing local ever opens, reads, runs in or authenticates to a host's path.
pub fn local_path(workspace: Option<&Workspace>) -> Option<&str> {
    workspace.filter(|w| w.remote.is_none()).and_then(|w| w.path.as_deref())
}

/// `AllowsLocalOnly(workspace)`: delegation, files, browser, file-reading styles, agent I/O, the status line and the git badge.
pub fn allows_local_only(workspace: Option<&Workspace>) -> bool {
    workspace.is_none_or(|w| w.remote.is_none())
}

/// `UsesLocalTerminal(pane, workspace)`: a command pane is this PC's terminal only in a local workspace.
pub fn uses_local_terminal(pane: &RunSession, workspace: Option<&Workspace>) -> bool {
    pane.kind() == "shell" && workspace.is_none_or(|w| w.remote.is_none())
}

fn host_name(workspace: &Workspace) -> &str {
    workspace.remote.as_ref().and_then(|r| r.host_name.as_deref()).unwrap_or("")
}

/// `ProviderRuntime(provider, reference, connection)`: the host's CLI as its last answer described it; before this launch heard from the
/// host, a stand-in that blocks nothing.
pub fn provider_runtime(provider: &str, workspace: &Workspace, connection: Option<&RemoteConnectionInfo>) -> ProviderRuntime {
    if let Some(known) = connection.and_then(|c| c.runtime.as_ref()).and_then(|r| r.providers.iter().find(|p| p.id == provider)) {
        return known.clone();
    }
    let mut capabilities = providers::capabilities(provider, None);
    capabilities.permission_modes = providers::permission_modes(provider, true).into_iter().map(str::to_string).collect();
    ProviderRuntime {
        id: provider.into(),
        name: providers::name(provider).into(),
        available: true,
        version: None,
        detail: get_with("remote.workspace.runtimeUnknown", &[("host", host_name(workspace))]),
        model_catalog: providers::fallback(provider),
        capabilities,
    }
}

/// `BlockedReason(pane, reference, connection)`: why a remote pane cannot send now (its connection was removed, or the host said its
/// CLI is missing or lacks the mode); None when it can, or when the pane is local.
pub fn blocked_reason(workspace: Option<&Workspace>, pane: &RunSession, connection: Option<&RemoteConnectionInfo>) -> Option<String> {
    let workspace = workspace.filter(|w| w.remote.is_some())?;
    let host = host_name(workspace);
    let Some(connection) = connection else {
        return Some(get_with("remote.workspace.connectionGone", &[("host", host)]));
    };
    if pane.kind() == "shell" || connection.status != RemoteConnectionStatus::Connected {
        return None;
    }
    let provider = connection.runtime.as_ref()?.providers.iter().find(|p| p.id == pane.provider())?;
    if !provider.available {
        return Some(get_with("remote.host.error.cliMissing", &[("name", providers::name(pane.provider()))]));
    }
    let mode = pane.settings.clone().unwrap_or_default().permission_mode;
    if !provider.capabilities.permission_modes.contains(&mode) {
        return Some(get_with("remote.workspace.modeUnsupported", &[("host", host)]));
    }
    None
}

/// `StatusLine(reference, connection)`: where a remote workspace runs, "host · OS", and whether its connection is up.
pub fn status_line(workspace: &Workspace, connection: Option<&RemoteConnectionInfo>) -> String {
    let connected = connection.is_some_and(|c| c.status == RemoteConnectionStatus::Connected);
    let platform = connection.and_then(|c| c.runtime.as_ref()).map(|r| r.platform.as_str());
    let host = strings::host_with_os(host_name(workspace), platform);
    get_with(if connected { "remote.status.connected" } else { "remote.status.notConnected" }, &[("host", &host)])
}

/// `PaneNote(pane, reference, connection)`: the line under a remote pane's empty state.
pub fn pane_note(pane: &RunSession, workspace: &Workspace, connection: Option<&RemoteConnectionInfo>) -> String {
    if pane.kind() == "shell" {
        strings::command_note(host_name(workspace), connection.and_then(|c| c.runtime.as_ref()).map(|r| r.platform.as_str()))
    } else {
        get_with("remote.pane.agentNote", &[("host", host_name(workspace))])
    }
}

/// `FirewallPorts(phoneDirectPort, sharingPort)`: the ports the Tailscale rule must allow (1024-65535), sorted and distinct (W8-C's
/// rows read it through direct.rs; whether the rule there is current is `mighty_platform::FirewallRule::needs_update`).
pub fn firewall_ports(direct_port: Option<u16>, sharing_port: Option<u16>) -> Vec<u16> {
    let mut ports: Vec<u16> = [direct_port, sharing_port].into_iter().flatten().filter(|p| *p >= 1024).collect();
    ports.sort_unstable();
    ports.dedup();
    ports
}

/// C# `RemoteSecretCopy`: how the connection key is copied (never in clipboard history or the cloud clipboard, cleared after a minute
/// if still there).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SecretCopy {
    pub allowed_in_history: bool,
    pub roamable: bool,
    pub clear_after: Duration,
}

/// `RemoteSecretCopy.Key`.
pub const KEY_COPY: SecretCopy = SecretCopy { allowed_in_history: false, roamable: false, clear_after: Duration::from_secs(60) };

/// `RemoteSecretCopy.ShouldClear(clipboardText, copied)`: the clipboard still holds exactly what was copied.
pub fn should_clear(clipboard_text: Option<&str>, copied: &str) -> bool {
    clipboard_text == Some(copied)
}

/// Remote workspaces are not mirrored to the phone (C# `RemoteWorkspacePhone.Workspaces`): this computer's own workspaces only.
pub fn phone_workspaces(all: &[Workspace]) -> Vec<Workspace> {
    all.iter().filter(|w| w.remote.is_none()).cloned().collect()
}

/// `RemoteWorkspacePhone.Sessions`: the panes of this computer's own workspaces only.
pub fn phone_sessions<'a>(sessions: impl IntoIterator<Item = &'a RunSession>, workspaces: &[Workspace]) -> Vec<RunSession> {
    let local: std::collections::HashSet<&str> = workspaces.iter().filter(|w| w.remote.is_none()).map(Workspace::id).collect();
    sessions.into_iter().filter(|s| local.contains(s.workspace_id())).cloned().collect()
}
