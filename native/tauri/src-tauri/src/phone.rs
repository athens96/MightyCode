//! The phone's desk (C# WinUI/MainWindow.MobileRemote.cs:456-835: `MobileRoute`, `MobileAction`, `MobileSubmit` with the fix branch's
//! S-L3 continuation revoke, run-next, run-now, the pane extras; MainWindow.MobileImages.cs) as mighty-remote's
//! `phone::desk::PhoneDesk` over the run service, `DelegationApp`, the composer queue, `ask.rs`'s targets, terminal prompts,
//! `antigravity.rs`'s cards, `updates.rs`'s app update, `workmap.rs` and `files.rs`; and the router it feeds. Owner: W8-B.
//!
//! W8 Step 0 wrote the seams host.rs, mobile.rs (W8-A) and the smoke call, each at rest (no router, nothing to notify), so host.rs needs no
//! further W8 change. What W8-B fills here (W8 plan, W8-B): the `Desk` (C#'s `MobileAction` for `create, submit, stop, queue-remove,
//! queue-next, queue-run-now, rename, close, settings, setAskTarget, guided, terminal-prompt, antigravityPermission, command`; the phone's
//! close never asks; a submit releasing held items answers `started` without ids; `NotStarted` is 409 `remote.error.dropped`; styles None
//! until W9), the router made once and kept, [`changed`] marking it, [`handler`] for the relay host, [`take_notifications`] for the tick.

#![allow(dead_code, reason = "W8-B's seams, at rest until it fills them")]

use std::sync::Arc;

use mighty_remote::relay::connection::RequestHandler;

use crate::host::Host;

/// Something a phone may show moved (C# `MobileDesktopRouter.Changed` from `RunEventReceived`, `ToolPermissionChanged`,
/// `DelegationChanged`, `AntigravityCardChanged`): called from host.rs's change hook on every run service event. It must stay cheap and
/// never take the view's lock (the caller may hold it). At rest: nothing.
pub fn changed(host: &Host) {
    let _ = host;
}

/// The m1 router as a connection's handler (W8-A's relay host and direct connections), once made. At rest: none.
pub fn handler(host: &Arc<Host>) -> Option<Arc<dyn RequestHandler>> {
    let _ = host;
    None
}

/// The router's `Observe` then `TakeNotifications` (W8-A's 250 ms tick): the scopes whose revision moved, `state` before the sessions.
/// At rest: none.
pub fn take_notifications(host: &Host) -> Vec<(String, i64)> {
    let _ = host;
    vec![]
}
