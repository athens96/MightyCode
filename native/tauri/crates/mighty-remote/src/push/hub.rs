//! The push hub (C# `MobilePushHub`, R\MobilePushHub.cs; plan §1.3): driven by the app's 250 ms tick, it reads the panes only when
//! something moved, bridges a run that started and ended between two reads as `running`, plans (`planner`), picks the recipients
//! (`recipients`), seals (`seal`) and sends (`transport`); nothing goes out while the remote is off. The m1 routes `/m1/push/register`,
//! `unregister` and `viewing` (W8-B's router) reach it. Owner: W8-A.
//!
//! At rest (W8 Step 0): a hub that never pushes.

use std::time::Duration;

use mighty_core::model::AppSnapshot;

use crate::identity::PushRegistrationResult;

/// `MobilePushHub.MaximumRetryWait`.
pub const MAXIMUM_RETRY_WAIT: Duration = Duration::from_secs(30);
/// How long a phone's viewing record lasts.
pub const VIEWING: Duration = Duration::from_secs(45);

#[derive(Default)]
pub struct PushHub {}

impl PushHub {
    /// `Changed`: something a push may follow moved; the panes are read at the next [`PushHub::observe`].
    pub fn changed(&self) {}

    /// `Observe` (the 250 ms tick): the panes read when something moved, and what to push sent.
    pub fn observe(&self, snapshot: &AppSnapshot) {
        let _ = snapshot;
    }

    /// `Register(device, handle, kinds)` (`POST /m1/push/register`).
    pub fn register(&self, device: &str, handle: String, kinds: Vec<String>) -> PushRegistrationResult {
        let _ = (device, handle, kinds);
        PushRegistrationResult::Unsaved
    }

    /// `Unregister(device)` (`POST /m1/push/unregister`).
    pub fn unregister(&self, device: &str) -> bool {
        let _ = device;
        false
    }

    /// `View(device, sessionId)` (`POST /m1/push/viewing`): the phone shows that session now (None: none).
    pub fn viewing(&self, device: &str, session_id: Option<String>) {
        let _ = (device, session_id);
    }

    /// A rotation or revoke: every viewing record dropped.
    pub fn forget_views(&self) {}
}
