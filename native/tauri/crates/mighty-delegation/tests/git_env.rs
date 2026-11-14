//! The half of "delegation: Windows paths, line endings, long paths and folders held open" (C# DelegationGitVerification.cs:632-644)
//! that sets `GIT_DIR` and `GIT_WORK_TREE` for the whole process: delegation's git drops every inherited `GIT_*` variable and runs no
//! hook of the repository's. A binary of its own with this one check, so no other check ever sees the variables.

mod support;

#[cfg(windows)]
#[tokio::test]
async fn inherited_git_variables_are_dropped_and_no_hook_runs() {
    use std::path::Path;

    use mighty_delegation::git::worktree::ChildWorktreeOutcome;
    use support::place::DelegationPlace;

    let place = DelegationPlace::new();
    place.repository_default().await;
    let hooks = Path::new(&place.repo()).join(".git").join("hooks");
    std::fs::create_dir_all(&hooks).unwrap();
    std::fs::write(hooks.join("post-checkout"), "#!/bin/sh\necho ran > \"$(git rev-parse --git-common-dir)/hook-ran\"\n").unwrap();
    // SAFETY: this binary's only check, so no other thread reads the environment meanwhile.
    unsafe {
        std::env::set_var("GIT_DIR", Path::new(&place.base()).join("nowhere"));
        std::env::set_var("GIT_WORK_TREE", place.base());
    }
    let made = place.maker().create("hooked", &place.repo(), "t").await;
    // SAFETY: as above.
    unsafe {
        std::env::remove_var("GIT_DIR");
        std::env::remove_var("GIT_WORK_TREE");
    }
    assert!(
        matches!(made, ChildWorktreeOutcome::Created(_)) && !Path::new(&place.repo()).join(".git").join("hook-ran").exists(),
        "inherited GIT_* variables are dropped and no hook runs: {made:?}"
    );
    // The same hook does run for a git that is not delegation's, so the check above proves something.
    place.git_in(&place.repo(), &["checkout", "-q", "-b", "other"]).await;
    assert!(Path::new(&place.repo()).join(".git").join("hook-ran").exists(), "the fixture's own git runs the hook");
}
