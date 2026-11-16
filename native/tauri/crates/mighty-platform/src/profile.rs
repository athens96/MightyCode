//! The behaviours that differ between the Windows and the Mac app on purpose (.omc/research/tauri/mac-gaps.md §2), as named
//! values the shared logic reads instead of asking which OS it runs on. Each value says what the 0.4.x app of that OS does today;
//! changing one changes what users of that OS see.

/// The OS a build runs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Os {
    Windows,
    Macos,
}

impl Os {
    /// The name the page's `host` push and the logs use.
    pub fn name(self) -> &'static str {
        match self {
            Os::Windows => "windows",
            Os::Macos => "macos",
        }
    }
}

/// How absolute paths look: drive letters and UNC shares, or one root at `/`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathStyle {
    Windows,
    Posix,
}

/// What a request for the plugin list gets while a read of it is already running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginListWhileReading {
    /// The Mac answers `busy` at once.
    Busy,
    /// Windows waits for the running read and answers with its result.
    Join,
}

/// Which package manager updates the agent CLIs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CliUpdater {
    Homebrew,
    Winget,
}

/// Where the app's own settings live (beside the state file's fields).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsStore {
    /// The Mac: UserDefaults `dev.mightycode.native` (`appUpdate.*`, `delegation.enabled`, `setup.neverShow`, ...): the settings the
    /// Swift app keeps there are read from it and written back to it, not to the state file (src-tauri `settings_place`).
    UserDefaults,
    /// Windows: `HKCU\Software\MightyCode` values for some, snapshot fields for others (`AppUpdateAutoCheck`, `AppUpdateLastCheckedAt`).
    RegistryAndSnapshot,
}

/// How the phone pairing identity is kept on disk. Not a profile value: each OS crate's `PairingStore` is the one layout it keeps
/// (Windows `mobile-identity.json` under its protected folder, the Mac `pairing::four_files` with owner-only modes), as the transport is
/// the Agent IO listener's own ([`McpTransport`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairingLayout {
    /// Windows: `mobile-identity.json`, the key, tokens and devices together.
    IdentityFile,
    /// The Mac: `mobile-remote.key`, `relay-keypair.json`, `relay-host-token.json`, `devices.json`.
    FourFiles,
}

/// Where a CLI's interactive sign-in (Antigravity) runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentSignIn {
    /// The Mac: a terminal pane in the window.
    TerminalPane,
    /// Windows: a console window of its own.
    ConsoleWindow,
}

/// What `--agent-io-mcp` listens on: the listener's and the connector's own answer (`AgentIoListener::transport`), so no profile
/// value repeats it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpTransport {
    UnixSocket,
    NamedPipe,
}

impl McpTransport {
    /// The environment variable that hands an agent run's MCP servers the app's listener (C# `AgentIOBinding`, the Mac's
    /// `AgentPaneMCPBinding.swift`): the pipe's name on Windows, the socket's path on the Mac.
    pub fn env_key(self) -> &'static str {
        match self {
            McpTransport::NamedPipe => "MIGHTY_AGENT_IO_PIPE",
            McpTransport::UnixSocket => "MIGHTY_AGENT_IO_SOCKET",
        }
    }
}

/// The deliberate differences, one value per behaviour. Every value is read where its behaviour is (the guards job of
/// native-tauri.yml fails on a field nothing reads). Mac stage M-J took out four that described no difference any more: the pairing
/// layout and the Agent IO transport (each the OS port's own), the snapshot's `autoUpdateCLIs`/`autoUpdatePlugins` (C#'s `AppSnapshot`
/// carries both too, Models.cs:387-388) and the files pane's save (neither app saves one: C# `FilePaneKind.Stored`, Swift
/// `SessionKind.stored`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformProfile {
    pub os: Os,
    pub path_style: PathStyle,
    pub plugin_list_while_reading: PluginListWhileReading,
    pub cli_updater: CliUpdater,
    pub settings_store: SettingsStore,
    pub agent_sign_in: AgentSignIn,
    /// The `platform` field of the phone protocol's `/m1/info` ("win32" on Windows, "darwin" on the Mac).
    pub m1_platform: &'static str,
    /// The remote workspace host percent-decodes request paths (the Mac does, Windows does not).
    pub remote_decodes_request_paths: bool,
    /// A Tailscale direct connection needs an inbound firewall rule first (Windows: `netsh` with elevation; the Mac has no step).
    pub direct_connection_firewall: bool,
    /// The app update check is off when the build carries no public key (Windows; the Mac checks and refuses at verification).
    pub update_needs_public_key: bool,
    /// A manifest address stamped into the build wins over one the user set (Windows).
    pub build_update_url_wins: bool,
    /// The staged package's SHA-256 is checked again just before the swap (Windows' `--update-helper`).
    pub update_rechecks_sha256: bool,
    /// W8: an adapter counts as Tailscale's only when it is named `utun…` (the Mac; Windows never reads names: the
    /// tailscale-addresses fixture's `darwin` and `win32` columns).
    pub tailscale_needs_utun: bool,
    /// W8: the direct path also advertises the MagicDNS name the Tailscale CLI gives (the Mac; Windows advertises none).
    pub direct_advertises_magic_dns: bool,
    /// W8: a remote workspace host answers `x-mighty-remote-graph: run` with `graph_run` events (Windows; the Mac ignores it).
    pub remote_graph_runs: bool,
    /// W8: a remote workspace host confines Codex and Antigravity runs (the Mac's Seatbelt); without it (Windows) it refuses them
    /// unless full access is switched on.
    pub remote_confines_agents: bool,
}

impl PlatformProfile {
    /// What the Windows app (0.4.x C#) does.
    pub const fn windows() -> PlatformProfile {
        PlatformProfile {
            os: Os::Windows,
            path_style: PathStyle::Windows,
            plugin_list_while_reading: PluginListWhileReading::Join,
            cli_updater: CliUpdater::Winget,
            settings_store: SettingsStore::RegistryAndSnapshot,
            agent_sign_in: AgentSignIn::ConsoleWindow,
            m1_platform: "win32",
            remote_decodes_request_paths: false,
            direct_connection_firewall: true,
            update_needs_public_key: true,
            build_update_url_wins: true,
            update_rechecks_sha256: true,
            tailscale_needs_utun: false,
            direct_advertises_magic_dns: false,
            remote_graph_runs: true,
            remote_confines_agents: false,
        }
    }

    /// What the Mac app (0.4.x Swift) does.
    pub const fn macos() -> PlatformProfile {
        PlatformProfile {
            os: Os::Macos,
            path_style: PathStyle::Posix,
            plugin_list_while_reading: PluginListWhileReading::Busy,
            cli_updater: CliUpdater::Homebrew,
            settings_store: SettingsStore::UserDefaults,
            agent_sign_in: AgentSignIn::TerminalPane,
            m1_platform: "darwin",
            remote_decodes_request_paths: true,
            direct_connection_firewall: false,
            update_needs_public_key: false,
            build_update_url_wins: false,
            update_rechecks_sha256: false,
            tailscale_needs_utun: true,
            direct_advertises_magic_dns: true,
            remote_graph_runs: false,
            remote_confines_agents: true,
        }
    }

    pub const fn for_os(os: Os) -> PlatformProfile {
        match os {
            Os::Windows => PlatformProfile::windows(),
            Os::Macos => PlatformProfile::macos(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_os_keeps_its_own_behaviour() {
        let windows = PlatformProfile::windows();
        let mac = PlatformProfile::macos();
        assert_eq!(windows.plugin_list_while_reading, PluginListWhileReading::Join);
        assert_eq!(mac.plugin_list_while_reading, PluginListWhileReading::Busy);
        assert_eq!(windows.cli_updater, CliUpdater::Winget);
        assert_eq!(mac.cli_updater, CliUpdater::Homebrew);
        assert_eq!((windows.m1_platform, mac.m1_platform), ("win32", "darwin"));
        assert!(windows.direct_connection_firewall && !mac.direct_connection_firewall);
        assert!(!windows.remote_decodes_request_paths && mac.remote_decodes_request_paths);
        assert!(mac.tailscale_needs_utun && !windows.tailscale_needs_utun);
        assert!(mac.direct_advertises_magic_dns && !windows.direct_advertises_magic_dns);
        assert!(windows.remote_graph_runs && !mac.remote_graph_runs);
        assert!(mac.remote_confines_agents && !windows.remote_confines_agents);
        assert_eq!(PlatformProfile::for_os(Os::Macos), mac);
        assert_eq!(Os::Windows.name(), "windows");
    }
}
