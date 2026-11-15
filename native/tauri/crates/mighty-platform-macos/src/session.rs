//! M-F: the session's end and the user's last input for the app update (with M-H): `NSWorkspaceWillPowerOffNotification` for a
//! logout, restart or shutdown (tao owns `applicationShouldTerminate:`, so Swift's `kAEQuitReason` is not read), the last input from
//! `CGEventSourceSecondsSinceLastEventType(.combinedSessionState, ~0)`, sleep and wake. Swift `AppStore+AppUpdate.swift:161`.
//! Contract `mighty_platform::session` (`SessionWatch`).

use std::sync::Arc;

use mighty_platform::SessionWatch;
use mighty_platform::unsupported::Unsupported;

/// The Mac's `SessionWatch` (Unsupported until M-F).
pub fn session() -> Arc<dyn SessionWatch> {
    Arc::new(Unsupported)
}
