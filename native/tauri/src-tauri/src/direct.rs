//! The Tailscale direct path in the window (C# WinUI/MainWindow.MobileDirect.cs: the firewall rule's state and ports; the direct half
//! of MobileRelayHost: `ApplyDirect`, re-detection every 15 s and on every address change) over mighty-remote's `direct` and
//! `tailscale` and the platform's `Network` and `Firewall`. Owner: W8-C.
//!
//! W8 Step 0 wrote the seams main.rs and mobile.rs (W8-A) call, each at rest (nothing listens, no endpoint), so main.rs needs no further
//! W8 change. What W8-C fills here (W8 plan, W8-C): the direct listeners started only when [`Host::may_listen`] and the remote and direct
//! switches are on (Tailscale addresses only, never a wildcard; the smoke binds loopback port 0 through the fake network:
//! `crate::platform::remote_ports`), handing sockets to W8-A's relay host, the endpoints for `/m1/info` and the pairing URL, the firewall
//! rule read unelevated and changed only on the user's click with `--allow-state-write` (ports: the direct port plus remote sharing's,
//! `remote::sharing_port`), and the rows' state for `settings/mobile_direct.rs`.

#![allow(dead_code, reason = "W8-C's seams, at rest until it fills them")]

use std::sync::Arc;

use mighty_platform::Platform;

use crate::host::Host;

/// The direct path's start with the app. At rest: nothing.
pub fn start(host: &Arc<Host>, platform: &Platform) {
    let _ = (host, platform);
}

/// The app quits: every direct listener closed. At rest: nothing.
pub fn shutdown(host: &Host) {
    let _ = host;
}

/// The endpoints the direct path advertises (`/m1/info`'s `direct`, the pairing URL's `&direct=`): at most 8, IPv6 in brackets; none
/// while nothing listens. At rest: none.
pub fn endpoints(host: &Host) -> Vec<String> {
    let _ = host;
    vec![]
}
