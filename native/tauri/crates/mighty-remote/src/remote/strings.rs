//! The Windows words of remote workspaces (C# `RemoteWindowsStrings`, R\RemoteWindowsStrings.cs; plan §1.6): the `remote.windows.*`
//! keys only the Windows app says ("이 PC", DPAPI, PowerShell, the firewall rule) and the host OS labels both apps show on remote panes
//! (`remote.hostOS.*`). Everything else reuses the Mac's `remote.*` and `settings.remote.*` keys as they are. Owner: W8-D.

use mighty_core::model_label::provider_label;
use mighty_run::strings::{get, get_with};

pub fn local_tailscale_off() -> String {
    get("remote.windows.connect.error.localTailscaleOff")
}
pub fn timeout() -> String {
    get("remote.windows.connect.error.timeout")
}
/// A connect while the saved-connections file is still held open by another program.
pub fn saved_unreadable() -> String {
    get("remote.windows.connect.error.savedUnreadable")
}
pub fn connected_saved() -> String {
    get("remote.windows.connect.detail.connectedSaved")
}
pub fn connected_memory() -> String {
    get("remote.windows.connect.detail.connectedMemory")
}
pub fn host_local_only() -> String {
    get("remote.windows.host.error.localOnly")
}
pub fn host_no_tailscale() -> String {
    get("remote.windows.host.error.noTailscale")
}
pub fn host_command_too_long() -> String {
    get("remote.windows.host.error.commandTooLong")
}
/// A Windows host refuses another computer's Codex or Antigravity run while the full-access switch is off, in any mode.
pub fn host_agent_needs_full_access(provider: &str) -> String {
    get_with("remote.windows.host.error.agentNeedsFullAccess", &[("agent", provider_label(provider))])
}
pub fn not_local() -> String {
    get("remote.windows.workspace.error.notLocal")
}
pub fn pane_unavailable() -> String {
    get("remote.windows.workspace.paneUnavailable")
}
pub fn settings_tailscale_off() -> String {
    get("remote.windows.settings.tailscaleOff")
}
pub fn settings_share_note() -> String {
    get("remote.windows.settings.shareNote")
}
pub fn settings_firewall_note() -> String {
    get("remote.windows.settings.firewallNote")
}
pub fn settings_scope_key() -> String {
    get("remote.windows.settings.scopeKey")
}
pub fn settings_allow_full_access_description() -> String {
    get("remote.windows.settings.allowFullAccessDescription")
}
pub fn key_copied() -> String {
    get("remote.windows.settings.keyCopied")
}
pub fn key_copy_failed() -> String {
    get("remote.windows.settings.keyCopyFailed")
}
pub fn firewall_add() -> String {
    get("remote.windows.firewall.add")
}
pub fn firewall_remove() -> String {
    get("remote.windows.firewall.remove")
}
/// The rule is there: one elevated `set rule` makes it hold exactly the ports in use now.
pub fn firewall_update() -> String {
    get("remote.windows.firewall.update")
}
/// The rule is there but does not open every port in use now, or its ports could not be read.
pub fn firewall_needs_update() -> String {
    get("remote.windows.firewall.needsUpdate")
}
/// Both firewall rows say it: the phone's direct port and the sharing port live in the one rule.
pub fn firewall_remove_closes_both() -> String {
    get("remote.windows.firewall.removeClosesBoth")
}
pub fn session_info_host_os() -> String {
    get("remote.sessionInfo.hostOS")
}

/// `CommandNote(host, platform)`: a command pane's note, naming the host and its OS.
pub fn command_note(host: &str, platform: Option<&str>) -> String {
    get_with("remote.windows.pane.commandNote", &[("host", host), ("os", &host_os(platform))])
}

/// `HostOS(platform)`: the host's OS from `/info` `runtime.platform`.
pub fn host_os(platform: Option<&str>) -> String {
    get(match platform {
        Some("darwin") => "remote.hostOS.darwin",
        Some("win32") => "remote.hostOS.win32",
        Some("linux") => "remote.hostOS.linux",
        _ => "remote.hostOS.unknown",
    })
}

/// `HostWithOS(host, platform)`: "host · OS".
pub fn host_with_os(host: &str, platform: Option<&str>) -> String {
    get_with("remote.hostWithOS", &[("host", host), ("os", &host_os(platform))])
}
