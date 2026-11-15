//! The remote mirror in the host (C# `WinUI\MainWindow.RemoteMirror.cs`, `Core\DesktopService.Mirror.cs`, the router's
//! `PendingRequests`/`SubmitInlineAsync`, from `main` 0.4.61, over mighty-remote's `remote::mirror`; plan §3 W9-J). Owner: W9-J.
//!
//! The servers section (`serverConnect`, `serverDisconnect`, `ShellModel.servers`, `SidebarWorkspace.server/mirror`, `StatusModel.mirror`,
//! `PaneInfo.mirror`), the mirrored panes never saved, `CloseSessionAsync` closing a mirrored pane on its host, and the host's
//! `/v2/mirror` through the phone router's handlers; listening and connecting only where `Host::may_listen` allows (loopback in a smoke).

use std::sync::Arc;

use serde_json::Value;

use crate::host::{Host, Registry};

/// The page methods this part answers: `serverConnect`, `serverDisconnect`. Step 0: none registered, so both stay on the unimplemented
/// list (README, the smoke's `ALLOWED_UNIMPLEMENTED`) until W9-J.
pub fn register(_methods: &mut Registry) {}

/// After remote workspaces (main.rs). Step 0 stub (W9-J writes it): nothing connects.
pub fn start(_host: &Arc<Host>, _platform: &mighty_platform::Platform) {}

/// At quit (main.rs), with the other W8 shutdowns. Step 0 stub (W9-J writes it).
pub fn shutdown(_host: &Arc<Host>) {}

/// The smoke's mirror step (smoke.rs `Stage::Mirror`, after the remote step; on loopback inside this process): true when it started; it
/// reports once through `Smoke::probed`. Step 0 stub (W9-J writes it): it does not start.
pub fn smoke_start(_host: &Arc<Host>) -> bool {
    false
}

/// The step's checks from its report. Step 0 stub (W9-J writes it): none.
pub fn smoke_checks(_report: &Value) -> Vec<(&'static str, bool)> {
    Vec::new()
}
