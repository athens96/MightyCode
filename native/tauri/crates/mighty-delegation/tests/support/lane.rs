//! One delegation check at a time within a test binary (the Mac's `DelegationLane.swift`): the checks change process-wide things (the
//! current folder git sees, a fixed listener, long timers), and cargo runs a binary's tests on many threads. Each check holds the lane
//! for its whole body: `let _lane = support::lane::enter().await;`. A check that failed while holding it lets the next one in.

use std::sync::LazyLock;

use tokio::sync::{Mutex, MutexGuard};

static LANE: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// Waits for the lane (first come, first served) and holds it until the guard drops. A tokio mutex: it works across the separate
/// runtimes `#[tokio::test]` gives each check, and a panicking holder does not poison it.
pub async fn enter() -> MutexGuard<'static, ()> {
    LANE.lock().await
}
