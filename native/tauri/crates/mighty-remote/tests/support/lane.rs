//! One remote check at a time within a test binary, where a check needs it (fixed ports, long timers, process-wide state): each such
//! check holds the lane for its whole body: `let _lane = support::lane::enter().await;`. A check that failed while holding it lets the
//! next one in.

use std::sync::LazyLock;

use tokio::sync::{Mutex, MutexGuard};

static LANE: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// Waits for the lane (first come, first served) and holds it until the guard drops. A tokio mutex: it works across the separate
/// runtimes `#[tokio::test]` gives each check, and a panicking holder does not poison it.
pub async fn enter() -> MutexGuard<'static, ()> {
    LANE.lock().await
}
