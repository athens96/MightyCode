//! The `netsh advfirewall` command lines of the one rule the app keeps (C# `WindowsFirewallRule`: the argv builder, its validators and
//! the reader of `show rule`'s ports; Core\WindowsFirewallRule.cs; plan §1.4): shared and pure, so the builder's checks run on both
//! OSes; the Windows crate runs them (`show` unelevated through `ProcessHost`, the change elevated; mighty-platform-windows
//! `firewall.rs`, the only file that may name the elevation verb). Owner: W8-C.
//!
//! At rest (W8 Step 0): the constants only; the builder answers `Unsupported`.

use crate::{PlatformError, Result};

/// `WindowsFirewallRule.RuleName`: shared with remote sharing.
pub const RULE_NAME: &str = "Mighty Code (Tailscale)";
/// The rule's `remoteip=` and `localip=`: Tailscale's ranges.
pub const REMOTE_RANGES: &str = "100.64.0.0/10,fd7a:115c:a1e0::/48";
pub const LOCAL_RANGES: &str = "100.64.0.0/10,fd7a:115c:a1e0::/48";
/// The only exe the rule may name.
pub const PROGRAM_FILE_NAME: &str = "MightyCode.exe";
/// At most this many ports in one rule.
pub const MAX_PORTS: usize = 16;
/// `ERROR_CANCELLED`: the user declined the elevation prompt.
pub const ELEVATION_CANCELLED: i32 = 1223;

/// What one command does to the rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetshCommand {
    /// `show rule name=…` (unelevated; exit 0 present, 1 absent, else unknown).
    Show,
    /// `add rule …` with these ports for this exe.
    Add { ports: Vec<u16>, program: String },
    /// `set rule name=… new … enable=yes`.
    Set { ports: Vec<u16>, program: String },
    /// `delete rule name=…`.
    Delete,
}

/// `%SystemRoot%\System32\netsh.exe` and its arguments for `command`, every value validated (ports 1024-65535, 1-16 of them, sorted
/// and distinct; the program an absolute drive path to `MightyCode.exe`; the system folder a drive path of at most 260 characters).
pub fn command_line(system_folder: &str, command: &NetshCommand) -> Result<(String, Vec<String>)> {
    let _ = (system_folder, command);
    Err(PlatformError::unsupported("netsh", "command_line"))
}
