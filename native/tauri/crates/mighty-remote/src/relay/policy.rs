//! When the relay link redials (C# R\RelayLink.cs `RelayLinkPolicy`; plan §1.1), pure: a path change settles 1.5 s (5 s at most), a
//! path-triggered redial waits a gap of 5 s doubling to 30 s, a wake redials without pacing, no network goes offline; the control
//! socket's keepalive is a ping every 20 s with a 10 s pong timeout; a reconnect waits 1 s doubling to 30 s, reset after a connect.
//! The path events come from mighty-platform's `Network` port (`on_change`, `on_resume`). Owner: W8-A.
//!
//! At rest (W8 Step 0): the constants and the decision's shape.

use std::time::Duration;

pub const PATH_SETTLE: Duration = Duration::from_millis(1500);
pub const PATH_SETTLE_MAXIMUM: Duration = Duration::from_secs(5);
pub const REDIAL_GAP: Duration = Duration::from_secs(5);
pub const REDIAL_GAP_MAXIMUM: Duration = Duration::from_secs(30);
pub const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(20);
pub const KEEPALIVE_TIMEOUT: Duration = Duration::from_secs(10);
pub const RECONNECT_FIRST: Duration = Duration::from_secs(1);
pub const RECONNECT_MAXIMUM: Duration = Duration::from_secs(30);

/// `RelayLinkPolicy.Action()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkAction {
    None,
    Offline,
    Reconnect,
}
