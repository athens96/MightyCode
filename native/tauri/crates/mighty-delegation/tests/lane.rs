//! The delegation checks' lane (tests/support/lane.rs, W7 Step 0): one check holds it at a time.

mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// Two holders never overlap, and a holder that panicked lets the next one in.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_lane_lets_one_check_in_at_a_time() {
    let inside = Arc::new(AtomicUsize::new(0));
    let most = Arc::new(AtomicUsize::new(0));
    let mut tasks = vec![];
    for _ in 0..8 {
        let (inside, most) = (inside.clone(), most.clone());
        tasks.push(tokio::spawn(async move {
            let _lane = support::lane::enter().await;
            let now = inside.fetch_add(1, Ordering::SeqCst) + 1;
            most.fetch_max(now, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(5)).await;
            inside.fetch_sub(1, Ordering::SeqCst);
        }));
    }
    for task in tasks {
        task.await.unwrap();
    }
    assert_eq!(most.load(Ordering::SeqCst), 1);
    let failed = tokio::spawn(async {
        let _lane = support::lane::enter().await;
        panic!("a check that fails while holding the lane");
    });
    assert!(failed.await.is_err());
    let next = tokio::time::timeout(Duration::from_secs(5), support::lane::enter()).await;
    assert!(next.is_ok(), "the lane is free again");
}
