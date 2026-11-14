//! Remote workspaces v2 in the window (C# Core/DesktopService.Remote.cs: attach, the host's options, the runtime cache, the start, the
//! answer; WinUI/MainWindow.RemoteWorkspaces.cs: the host badge, admission, the key's copy; MainWindow.WebShell.Shell.cs:100, 118, 343,
//! 487-503) over mighty-remote's `remote::host::RemoteHost` and `remote::client::RemoteClient`. Owner: W8-D.
//!
//! W8 Step 0 wrote the seams main.rs, methods.rs, host.rs, panes.rs and runs.rs call, each at rest (no connection, nothing blocked, no
//! runner, nothing shared), so none of them needs a further W8 change. What W8-D fills here (W8 plan, W8-D): `connectRemote` opening
//! Settings › 원격 연결 (and the add menu's `connect`, frame.rs), the saved connections for the frame model (`connections`), the
//! composer's block reason (`blocked_reason`), the client as the run service's `RemoteRunner` (`runner`), the sharing host started only
//! from the Start button when [`Host::may_listen`] (its port for W8-C's firewall rows: `sharing_port`), `RunsChanged` holding CLI updates,
//! the key copied secretly (history and roaming off, cleared after 60 s). `remote-connections.json` and the vault's key files are
//! written only with `--allow-state-write` (else in memory); every key only in `Authorization`, never in a URL, a log or `Debug`.

#![allow(dead_code, reason = "W8-D's seams, at rest until it fills them")]

use std::collections::HashMap;
use std::sync::Arc;

use mighty_core::model::{AppSnapshot, RunSession};
use mighty_core::shell::RemoteConnection;
use mighty_platform::Platform;
use mighty_run::service::RemoteRunner;
use serde_json::Value;

use crate::host::{Host, Reply};

/// Remote workspaces' start with the app (the saved connections read; sharing is always off at start). At rest: nothing.
pub fn start(host: &Arc<Host>, platform: &Platform) {
    let _ = (host, platform);
}

/// The app quits: sharing stopped (its key revoked) and every connection's runs ended. At rest: nothing.
pub fn shutdown(host: &Host) {
    let _ = host;
}

/// `connectRemote` (C# MainWindow.WebShell.Shell.cs: the sidebar's and the dashboard's connect buttons → Settings › 원격 연결). Answers
/// null. At rest: nothing opens.
pub async fn connect_remote(host: Arc<Host>, args: Value) -> Reply {
    let _ = (host, args);
    Ok(Value::Null)
}

/// C# `RemoteConnection(workspace)` for the frame model, by connection id (connected, the host's OS from its last answer). Read with the
/// state locked: it reads the client's own state, never `host.view()`. At rest: none (a remote workspace reads "not connected").
pub fn connections(host: &Host) -> HashMap<String, RemoteConnection> {
    let _ = host;
    HashMap::new()
}

/// C# `RemoteWorkspaceFeatures.BlockedReason` for a pane's composer (`WindowRules::remote_block`), read with the state locked. At rest:
/// none.
pub fn blocked_reason(host: &Host, snapshot: &AppSnapshot, pane: &RunSession) -> Option<String> {
    let _ = (host, snapshot, pane);
    None
}

/// The run service's runner for remote workspaces' panes (`RunServiceOptions.remote`), read once as runs.rs makes the service. At rest:
/// none (a remote pane's run cannot start, as before W8).
pub fn runner(host: &Arc<Host>) -> Option<Arc<dyn RemoteRunner>> {
    let _ = host;
    None
}

/// The sharing host's port while it shares (the firewall rule's second port, W8-C). At rest: none.
pub fn sharing_port(host: &Host) -> Option<u16> {
    let _ = host;
    None
}
