//! M-B: the user's settings in place, UserDefaults domain `dev.mightycode.native` (the process's standard domain once the bundle id
//! matches; the legacy `dev.mightyclaude.native` copied once, `RenameMigration.swift:47-53`). Swift: every UserDefaults site (the key
//! list is the plan's §2 B row); contract `mighty_platform::prefs` (`PrefsStore`).
//!
//! Tests use `initWithSuiteName:` with a `dev.mightycode.tests.<uuid>` suite, removed at the end: never the runner user's domain.
//! Writes only where the write policy writes (plan §6); a smoke's are in memory.

use std::sync::Arc;

use mighty_platform::PrefsStore;
use mighty_platform::unsupported::Unsupported;

/// The Mac's `PrefsStore` (Unsupported until M-B).
pub fn prefs() -> Arc<dyn PrefsStore> {
    Arc::new(Unsupported)
}
