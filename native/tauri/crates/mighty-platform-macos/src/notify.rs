//! M-F: completion notifications through `UNUserNotificationCenter` (objc2; plan D6): `.alert`/`.sound` asked once, sent only when
//! authorized, the pane id in `userInfo`, a click focusing the pane (`Notifier::on_activated`). Swift `AgentCompanion.swift:380-486`.
//! Contract `mighty_platform::notify`. A smoke's notifier is the fake (src-tauri platform.rs `ports`).

use std::sync::Arc;

use mighty_platform::Notifier;
use mighty_platform::unsupported::Unsupported;

/// The Mac's `Notifier` (Unsupported until M-F).
pub fn notifier() -> Arc<dyn Notifier> {
    Arc::new(Unsupported)
}

/// The authorization request (only where the write policy writes, never in a smoke; Unsupported until M-F).
pub fn register_notifications() -> std::result::Result<(), String> {
    Err("unsupported on this platform: Notifier.register".into())
}
