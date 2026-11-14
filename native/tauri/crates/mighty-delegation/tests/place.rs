//! The fixture itself (tests/support/place.rs, C# `DelegationPlace`): a repository in a temp folder whose git reads no user config,
//! a child made the app's way, and the snapshots every git check compares.

mod support;

#[cfg(windows)]
mod windows {
    use super::support::place::DelegationPlace;

    /// The place makes a repository and a child in it, its git reading only the empty fixture config, and goes when dropped.
    #[tokio::test]
    async fn the_place_makes_a_repository_and_a_child_and_goes_when_dropped() {
        let _lane = super::support::lane::enter().await;
        let place = DelegationPlace::new();
        let base = place.base();
        let head = place.repository(2, "", "false").await;
        assert_eq!(place.fixture_git(&["rev-list", "--count", "HEAD"]).await, "2");
        assert_eq!(place.fixture_git(&["config", "--global", "--list"]).await, "", "the global config is the empty fixture file");
        let (child, reported) = place.reported_child("c1", &[], Some(&place.repo())).await;
        assert!(child.report_head.as_deref() == Some(reported.as_str()) && reported != head && child.base_commit == head);
        assert_eq!(place.mighty_branches().await, "refs/heads/mighty/c1");
        assert_eq!(place.worktree_list().await.len(), 2);
        assert!(place.snapshot(&[&child.worktree_path]).await.contains("refs/heads/mighty/c1"));
        assert!(place.disk().await.contains("REPORT.md: "));
        drop(place);
        assert!(!std::path::Path::new(&base).exists(), "the place is removed");
    }
}
