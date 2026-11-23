//! The macOS implementation of the `mighty-platform` ports, written in Rust (docs/tauri-architecture.md §1 rule 4: AppKit through
//! objc2, the terminal through libghostty's C API). On any other OS this crate is empty.
//!
//! Mac stage Step 0 (.omc/research/tauri/mac-plan.md §3 0c, .omc/research/tauri/mac-step0.md): one module per port, each naming the
//! Swift it ports and its owner part, each answering `Unsupported` until its owner fills it in; [`platform`] wires them all.
//!
//! | module | owner | Swift |
//! |---|---|---|
//! | [`process`], [`pty`], [`environment`], [`clipboard`], [`fonts`], [`disk`] | M-A | `ProcessRunner`, `PTYAgentTerminalPane`, `CLIEnvironmentResolver`, … |
//! | [`prefs`], [`secrets`], [`claude_credentials`], [`pairing`] | M-B | UserDefaults sites, `RemoteKeychain`, `AccountUsageService`, `MobileRemoteService` |
//! | [`agent_io`], [`net`], [`proxy`], [`http`] | M-C | `AgentIOSocketServer`, `RelayLink`, `RemoteWorkspace` |
//! | [`terminal_surface`] | M-D | `LocalTerminalSession`, `HostTerminalView`, vendored `GhosttyTerminal` (behaviour reference) |
//! | [`notify`], [`pet`], [`session`], [`input_method`], [`menu`] | M-F | `AgentCompanion`, `AgentCompanionViews`, `InputMethodDiagnostics` |
//! | [`files`], [`setup`], [`page`] | M-G | (Windows' files ports), `HelpCaptureDiagnostics` |
//! | [`update`], [`update_transport`] | M-H | `AppUpdate`, `AppStore+AppUpdate` |
//! | [`formats`] | M-J | `NSLocale` uses |
//!
//! Real already: [`paths`] (`~/Library/Application Support/MightyCode`), [`cli_copies`], [`MacPathProbe`]. No Mac counterpart, by
//! design: the firewall (Tailscale's direct path needs no rule on the Mac), the toast activator (Windows' COM activation), the
//! terminal launcher's console window (the Mac signs in in a terminal pane, `PlatformProfile::agent_sign_in`; its [`terminal`] launcher
//! only hands the setup screen's script to Terminal.app when no workspace holds the pane). Nor is there the Swift app's browser pane
//! (CEF or WKWebView, M-E): 0.5 opens a web page in the default browser ([`open_url`]).
#![cfg(target_os = "macos")]

pub mod agent_io;
/// A crash's leftover children ended at the next launch (the Job Object's kill-on-close on Windows).
pub mod child_groups;
pub mod claude_credentials;
pub mod clipboard;
pub mod disk;
pub mod environment;
pub mod files;
pub mod fonts;
pub mod formats;
pub mod http;
pub mod input_method;
pub mod listening;
/// M-F: the menu bar read back and a key equivalent pressed through it (the smoke).
pub mod menu;
pub mod net;
pub mod notify;
pub mod page;
pub mod pairing;
pub mod pet;
pub mod prefs;
pub mod process;
pub mod proxy;
pub mod pty;
pub mod secrets;
pub mod session;
pub mod setup;
pub mod speech;
/// The setup script in a Terminal.app window (the terminal launcher).
pub mod terminal;
pub mod terminal_surface;
pub mod update;
pub mod update_transport;

use std::path::PathBuf;
use std::sync::Arc;

use mighty_platform::unsupported::Unsupported;
use mighty_platform::{
    AppPaths, BoxFuture, Drawing, PetWindow, Platform, PlatformProfile, Result, SurfaceFactory, SurfaceKind, TerminalOptions,
    TerminalSurface,
};

pub use agent_io::{
    agent_io_connector, agent_io_listener, control_connector, control_listener, daemon_connector, daemon_listener, lock_exclusive,
};
pub use child_groups::{reap_child_groups, record_child_groups};
pub use clipboard::clipboard;
pub use disk::{disk_space, names_ignore_case};
pub use files::{file_ports, open_url, show_in_explorer};
pub use fonts::font_catalog;
pub use formats::{decimal_separator, long_time, number_grouping, short_date_time};
pub use http::https_get;
pub use notify::{notifier, register_notifications};
pub use process::process_host;
pub use speech::speech_recognizer;
pub use update::update_installer;

/// The data folder's name before the rename (Swift `StateRepository`). M-B adds what Swift's `RenameMigration` also moves.
pub const LEGACY_FOLDERS: [&str; 1] = ["mighty-claude"];

/// `~/Library/Application Support/MightyCode` (or `profile`), the pre-rename folder, and
/// `~/Library/Application Support/<identifier>` for this machine's webview data.
pub fn paths(profile: Option<PathBuf>, identifier: &str) -> Result<AppPaths> {
    AppPaths::resolve(profile, dirs::config_dir(), &LEGACY_FOLDERS, dirs::data_local_dir(), identifier)
}

pub fn platform(profile: Option<PathBuf>, identifier: &str) -> Result<Platform> {
    let paths = Arc::new(paths(profile, identifier)?);
    Ok(Platform {
        profile: PlatformProfile::macos(),
        pairing: pairing::pairing_store(paths.as_ref()),
        paths,
        processes: process::process_host(),
        ptys: pty::pty_host(),
        environment: environment::shell_environment(),
        secrets: secrets::secrets(),
        prefs: prefs::prefs(),
        notifier: notify::notifier(),
        // Tailscale's direct path needs no firewall rule on the Mac (PlatformProfile::direct_connection_firewall).
        firewall: Arc::new(Unsupported),
        updates: update::update_installer(),
        network: net::network(),
        proxy: proxy::proxy(),
        https: http::https(),
        update_transport: update_transport::update_transport(),
        session: session::session(),
        // Windows' toast activation (COM); a Mac notification's click comes back through `Notifier::on_activated`.
        activator: Arc::new(Unsupported),
    })
}

/// The Mac's native views of one window: the terminal (Ghostty) is native, and the pet is an NSPanel. Each opens through its owner's
/// module.
pub struct MacSurfaces {
    window: tauri::Window,
}

impl SurfaceFactory for MacSurfaces {
    fn drawing(&self, _kind: SurfaceKind) -> Drawing {
        Drawing::Native
    }
    fn open_terminal(&self, options: TerminalOptions) -> BoxFuture<'_, Result<Box<dyn TerminalSurface>>> {
        Box::pin(terminal_surface::open(&self.window, options))
    }
    fn open_pet(&self) -> BoxFuture<'_, Result<Box<dyn PetWindow>>> {
        Box::pin(pet::open(&self.window))
    }
}

/// The native views over `window`'s page (its NSView hosts Ghostty's).
pub fn surfaces(window: tauri::Window) -> Arc<dyn SurfaceFactory> {
    Arc::new(MacSurfaces { window })
}

/// What the Antigravity rule safety asks the disk (mighty-platform `PathProbe`, Swift `AntigravityRuleSafety`): a link read
/// without following it, what the file system calls a path (`realpath`); the Mac has no drive letters.
pub struct MacPathProbe;

impl mighty_platform::PathProbe for MacPathProbe {
    fn checks_paths(&self) -> bool {
        true
    }
    fn fixed_drive(&self, _letter: char) -> bool {
        false
    }
    fn link_target(&self, path: &std::path::Path) -> Option<PathBuf> {
        let meta = std::fs::symlink_metadata(path).ok()?;
        if !meta.file_type().is_symlink() {
            return None;
        }
        std::fs::read_link(path).ok()
    }
    fn final_path(&self, path: &std::path::Path) -> Option<PathBuf> {
        std::fs::canonicalize(path).ok()
    }
}

/// The Antigravity rule safety's disk questions on the Mac.
pub fn path_probe() -> Arc<dyn mighty_platform::PathProbe> {
    Arc::new(MacPathProbe)
}

/// A script in a terminal window of its own: Terminal.app ([`terminal`]). The Mac signs in in its own terminal pane (W6;
/// `PlatformProfile::agent_sign_in`); the setup screen's install falls back to this window when no local workspace holds its pane.
pub fn terminal_launcher() -> Arc<dyn mighty_platform::TerminalLauncher> {
    terminal::terminal_launcher()
}

/// Each copy of the command `name` along the environment's PATH (M/AppStore+AskTargets.swift reads the login shell's PATH): an
/// executable file of that name in each folder, once.
pub fn cli_copies(name: &str, environment: &std::collections::BTreeMap<String, String>) -> Vec<String> {
    use std::os::unix::fs::PermissionsExt as _;
    let mut copies: Vec<String> = Vec::new();
    for folder in environment.get("PATH").map(String::as_str).unwrap_or("").split(':').take(64) {
        if folder.is_empty() || !folder.starts_with('/') {
            continue;
        }
        let path = std::path::Path::new(folder).join(name);
        let runnable = std::fs::metadata(&path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0);
        let text = path.to_string_lossy().into_owned();
        if runnable && !copies.contains(&text) {
            copies.push(text);
        }
    }
    copies
}

#[cfg(test)]
mod tests {
    use super::*;
    use mighty_platform::Paths;

    #[test]
    fn the_default_profile_is_application_support_mightycode() {
        let paths = paths(None, "dev.mightycode.native").unwrap();
        assert!(paths.data_dir().ends_with("Library/Application Support/MightyCode"));
        assert!(paths.legacy_data_dirs()[0].ends_with("Library/Application Support/mighty-claude"));
    }

    /// Step 0: every port is wired, the Mac profile is the Mac's, and what has no Mac counterpart says so.
    #[test]
    fn the_platform_wires_every_port_with_the_mac_profile() {
        let root = std::env::temp_dir().join(format!("mighty-macos-platform-{}", std::process::id()));
        let platform = platform(Some(root.clone()), "dev.mightycode.native").unwrap();
        assert_eq!(platform.profile, PlatformProfile::macos());
        assert!(platform.paths.data_dir().starts_with(&root));
    }
}
