//! The Tailscale firewall rule on Windows (C# `WindowsFirewallRuleRunner`, Core\WindowsFirewallRule.cs; plan §1.4, D4): `show rule`
//! through the platform's `ProcessHost` (hidden, its output read; exit 0 present, 1 absent, else unknown), and the change through
//! `ShellExecuteExW` with the elevation verb and `SEE_MASK_NOCLOSEPROCESS`, waited 30 s (error 1223: the user declined; the wait
//! running out: `Failed("timeout")`); an add queries first (present: `set`, absent: `add`, unreadable: `queryFailed`). The command
//! lines are mighty-platform's `firewall::netsh`. This is the only file in the workspace that may name the elevation verb (a CI grep
//! replaces C#'s source scan, L497). It runs only with `--allow-state-write` on the user's click; tests and the smoke use a recording
//! runner. Owner: W8-C.
//!
//! At rest (W8 Step 0): both calls answer `Unsupported`, as the port did before W8.

use std::sync::Arc;

use mighty_platform::{BoxFuture, Firewall, FirewallChange, FirewallRule, PlatformError, ProcessHost, Result};

pub struct WindowsFirewall {
    processes: Arc<dyn ProcessHost>,
}

impl WindowsFirewall {
    pub fn new(processes: Arc<dyn ProcessHost>) -> WindowsFirewall {
        WindowsFirewall { processes }
    }
}

impl Firewall for WindowsFirewall {
    fn read(&self) -> BoxFuture<'_, Result<FirewallRule>> {
        let _ = &self.processes;
        Box::pin(async { Err(PlatformError::unsupported("Firewall", "read")) })
    }
    fn apply(&self, _ports: Vec<u16>) -> BoxFuture<'_, Result<FirewallChange>> {
        Box::pin(async { Err(PlatformError::unsupported("Firewall", "apply")) })
    }
}
