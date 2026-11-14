//! What a remote workspace's panes may do (C# `RemoteWorkspaceFeatures`, R\RemoteWorkspaceFeatures.cs; plan §1.6): the composer's
//! block reason (`BlockedReason`, mighty-composer's `WindowRules::remote_block`), the panes a remote workspace offers (`AddPaneEntries`
//! is mighty-core's `add_pane`), `AllowsLocalOnly`, and the firewall rule's ports (the direct port while the remote and direct are on,
//! plus the sharing port while sharing; W8-C's firewall rows). Owner: W8-D.
//!
//! At rest (W8 Step 0): nothing is blocked. `firewall_ports` is written (W8-C's firewall rows need it).

use mighty_core::model::{RunSession, Workspace};

use crate::remote::wire::RemoteConnectionInfo;

/// `RemoteWorkspaceFeatures.BlockedReason(workspace, pane, connection)`: why a remote pane's composer cannot send now, None when it can
/// (or the pane is local).
pub fn blocked_reason(workspace: Option<&Workspace>, pane: &RunSession, connection: Option<&RemoteConnectionInfo>) -> Option<String> {
    let _ = (workspace, pane, connection);
    None
}

/// `RemoteWorkspaceFeatures.FirewallPorts(directPort, sharingPort)`: the ports the Tailscale rule must allow, sorted and distinct.
pub fn firewall_ports(direct_port: Option<u16>, sharing_port: Option<u16>) -> Vec<u16> {
    let mut ports: Vec<u16> = [direct_port, sharing_port].into_iter().flatten().filter(|port| *port >= 1024).collect();
    ports.sort_unstable();
    ports.dedup();
    ports
}
