//! The phone remote's lifecycle in the window (C# WinUI/MainWindow.MobileRemote.cs:18-93: start, the 250 ms tick, wake and network
//! events) over mighty-remote's `relay::host::RelayHost`, `identity::Identity` and `push::hub::PushHub`. Owner: W8-A.
//!
//! W8 Step 0 wrote the seams main.rs calls, each at rest (nothing is loaded, connected or pushed), so main.rs, host.rs and smoke.rs need no
//! further W8 change. What W8-A fills here (W8 plan, W8-A): the identity loaded (created only when the app may write), the relay host
//! started from the saved `mobileRemote` settings only when [`Host::may_reach_relay`] (otherwise the box says verification mode and
//! `host.log` says so too), its handler the phone router (`phone::handler`), the 250 ms tick (`push.observe`, then
//! `phone::take_notifications` → `relay.notify`; on its own task, never blocking the bridge), wake and network changes from the platform's
//! `Network` (`crate::platform::remote_ports`), and the relay's status for Settings (`settings/mobile.rs`). Never a real relay in a test
//! or the smoke (the smoke's phone step runs the in-process fake relay on loopback); no URL is ever logged.

#![allow(dead_code, reason = "W8-A's seams, at rest until it fills them")]

use std::sync::Arc;

use mighty_platform::Platform;

use crate::host::Host;

/// The phone remote's start with the app (C# `StartMobileRemoteAsync` from `MainWindow`'s load), after the run service and delegation.
/// At rest: nothing.
pub fn start(host: &Arc<Host>, platform: &Platform) {
    let _ = (host, platform);
}

/// The app quits: the relay host disposed (every connection ended, nothing redials) and the tick stopped. At rest: nothing.
pub fn shutdown(host: &Host) {
    let _ = host;
}
