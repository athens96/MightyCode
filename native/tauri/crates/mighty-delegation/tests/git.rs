//! Delegation's git on real repositories in temp folders (C# MightyCode.Core.Tests/DelegationGitVerification.cs, ported from the
//! Mac's DelegationWorktreeTests, DelegationMergeTests, DelegationCardUndoTests and the git half of DelegationCleanupTests), with the
//! checks Windows needs of its own: paths, line endings, long paths and folders another program holds open; and the git checks of
//! DelegationHumanChildVerification (the app's commit, git's failure kinds) and DelegationAppVerification (junctions, the deadline,
//! REPORT.md read the Windows way). Each check is named by its C# name.
//!
//! Git runs through the platform's process host (Windows', or M-A's on the Mac); the checks of Windows' own paths, junctions and file
//! sharing stay `#[cfg(windows)]`, each with its reason. What the checks
//! assert of W7-A's delegation file (`RecordingMerge`, `RecordingUndo`, the store's round trip, `FirstInput`, the copies, the pane's
//! notes) is W7-A's to check over its `ChildRecord`; here a child is the git layer's view of one ([`support::place::PlaceChild`]). The
//! check that sets `GIT_DIR` process-wide is in its own binary (tests/git_env.rs).

mod support;

mod checks {
    #![cfg_attr(not(windows), allow(unused_imports, dead_code, reason = "what only the Windows checks (junctions, share modes) use"))]

    use std::collections::HashSet;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use mighty_core::locale::Locale;
    use mighty_delegation::git::auto_commit::{self, AutoCommitOutcome, MAXIMUM_NEW_FILE_BYTES};
    use mighty_delegation::git::cleanup::{self, ChildCleanupHold, ChildCleanupOutcome, ChildDiscardOutcome};
    use mighty_delegation::git::merge::{self, ChildMergeOutcome, ChildUndoOutcome};
    use mighty_delegation::git::paths::{is_inside, is_link, same, standardized};
    use mighty_delegation::git::worktree::{
        ChildWorktree, ChildWorktreeCheck, ChildWorktreeOutcome, MINIMUM_FREE_BYTES, Maker, NOTES_FOLDER, is_safe_session_id,
        report_file_in, task_file_in,
    };
    use mighty_delegation::git::{Git, GitFailure, GitRun, is_object_name, line, message, report_file, run};
    use mighty_delegation::host::DelegationPaneActivity;
    use mighty_delegation::model::{ChildState, MergeKind, MergeRecord};
    use mighty_delegation::request::{DelegationName, DelegationReasonCode};
    use mighty_platform::fake::FixedDiskSpace;
    use mighty_platform::{DiskSpace, PlatformError, SpawnRequest};
    use sha2::{Digest, Sha256};

    use super::support::lane;
    use super::support::place::{DelegationPlace, PlaceChild, paths, processes, remove_folder, temp, utf8};

    fn en() -> Locale {
        Locale::new("en")
    }

    fn made(outcome: ChildWorktreeOutcome) -> ChildWorktree {
        match outcome {
            ChildWorktreeOutcome::Created(worktree) => worktree,
            other => panic!("not made: {other:?}"),
        }
    }

    fn merged(outcome: ChildMergeOutcome) -> MergeRecord {
        match outcome {
            ChildMergeOutcome::Merged(record) => record,
            other => panic!("not merged: {other:?}"),
        }
    }

    fn sha256(bytes: &[u8]) -> String {
        Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
    }

    fn combine(folder: &str, name: &str) -> String {
        Path::new(folder).join(name).to_string_lossy().into_owned()
    }

    /// The names in a folder, sorted.
    fn entries(folder: &str) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(folder)
            .map(|e| e.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect())
            .unwrap_or_default();
        names.sort();
        names
    }

    /// A relative path that names nothing: a check's current folder is in the app's own checkout, so a relative path there must never
    /// name a real folder git could be pointed at.
    fn relative() -> String {
        let path = format!("mighty-relative-{}/folder", mighty_core::wire::new_id());
        assert!(!Path::new(&path).exists());
        path
    }

    /// How many worktrees `git worktree list --porcelain` names.
    fn listed(list: &str) -> usize {
        list.split("worktree ").count() - 1
    }

    /// A free-space reader that records the paths it was asked about and answers `free` (None: unknown).
    struct Asked {
        paths: Mutex<Vec<PathBuf>>,
        free: Option<u64>,
    }

    impl DiskSpace for Asked {
        fn available_bytes(&self, path: &Path) -> mighty_platform::Result<u64> {
            self.paths.lock().unwrap().push(path.to_path_buf());
            self.free.ok_or_else(|| PlatformError::Invalid("unknown".into()))
        }
    }

    /// `mklink /J link target`, through cmd.exe and the process host.
    #[cfg(windows)]
    async fn junction(link: &str, target: &str) {
        let comspec = std::env::var("ComSpec").unwrap_or_else(|_| r"C:\Windows\System32\cmd.exe".into());
        let folder = Path::new(link).parent().unwrap().to_path_buf();
        let request = SpawnRequest::new(comspec, ["/d", "/c", "mklink", "/J", link, target], folder);
        let made = mighty_platform::capture(processes().as_ref(), request, None, Duration::from_secs(30), 1 << 20).await.unwrap();
        assert!(made.exit_code == 0 && std::fs::read_link(link).is_ok(), "the junction was made: {link}");
    }

    // ---- DelegationGitVerification ----

    /// "delegation: worktrees are made beside the checkout, on a flat branch, with ignored notes"
    #[tokio::test]
    async fn worktrees_are_made_beside_the_checkout_on_a_flat_branch_with_ignored_notes() {
        let _lane = lane::enter().await;
        let place = DelegationPlace::new();
        place.repository(2, "", "false").await;
        let head = place.fixture_git(&["rev-parse", "HEAD"]).await;
        let maker = place.maker();
        let child = made(maker.create("child-1", &place.repo(), "Fix the parser.\n").await);
        assert!(
            child.parent_branch == "main"
                && child.branch == "mighty/child-1"
                && child.base_commit == head
                && child.worktree_path == combine(&place.root(), "child-1")
                && child.working_folder == child.worktree_path,
            "the flat branch at the parent's head, in its own folder under the root: {child:?}"
        );
        assert!(
            place.fixture_git(&["rev-parse", "refs/heads/mighty/child-1"]).await == head
                && place.git_in(&child.worktree_path, &["symbolic-ref", "HEAD"]).await == "refs/heads/mighty/child-1",
            "the worktree is on the child's branch"
        );
        let list = place.fixture_git(&["worktree", "list", "--porcelain"]).await;
        assert!(listed(&list) == 2 && list.contains("branch refs/heads/mighty/child-1"), "one more worktree is registered");
        assert!(
            child.task_file() == combine(&combine(&child.worktree_path, ".mighty-delegation"), "TASK.md")
                && std::fs::read(child.task_file()).unwrap() == utf8("Fix the parser.\n")
                && std::fs::read(child.report_file()).unwrap().is_empty()
                && std::fs::read(combine(&combine(&child.worktree_path, ".mighty-delegation"), ".gitignore")).unwrap() == utf8("*\n"),
            "TASK.md holds the task byte for byte, REPORT.md is empty and the folder ignores itself"
        );
        // Both notes are ignored: no change in the child, and a commit of everything leaves them out.
        assert_eq!(
            place.git_in(&child.worktree_path, &["check-ignore", ".mighty-delegation/TASK.md", ".mighty-delegation/REPORT.md"]).await,
            ".mighty-delegation/TASK.md\n.mighty-delegation/REPORT.md",
            "both notes are ignored"
        );
        assert!(
            place.git_in(&child.worktree_path, &["status", "--porcelain", "--untracked-files=all"]).await.is_empty(),
            "a fresh worktree has no change"
        );
        std::fs::write(child.report_file(), "REPORT: done\n").unwrap();
        std::fs::write(combine(&child.worktree_path, "work.txt"), "work\n").unwrap();
        place.git_in(&child.worktree_path, &["add", "-A"]).await;
        place.git_in(&child.worktree_path, &["commit", "-q", "-m", "child work"]).await;
        let mut tree: Vec<String> =
            place.fixture_git(&["ls-tree", "-r", "--name-only", "mighty/child-1"]).await.split('\n').map(str::to_string).collect();
        tree.sort();
        assert_eq!(tree, ["file-0.txt", "file-1.txt", "work.txt"], "a commit of everything leaves the notes out");
        assert!(
            place.fixture_git(&["symbolic-ref", "--short", "HEAD"]).await == "main"
                && place.fixture_git(&["rev-parse", "HEAD"]).await == head
                && place.fixture_git(&["status", "--porcelain", "--untracked-files=all"]).await.is_empty(),
            "the parent checkout is untouched"
        );

        // Never replaces an existing branch, and never makes an unsafe name.
        place.fixture_git(&["branch", "mighty/taken", "HEAD~1"]).await;
        let old = place.fixture_git(&["rev-parse", "mighty/taken"]).await;
        assert!(
            matches!(maker.create("taken", &place.repo(), "t").await, ChildWorktreeOutcome::Failed(_))
                && place.fixture_git(&["rev-parse", "mighty/taken"]).await == old
                && listed(&place.fixture_git(&["worktree", "list", "--porcelain"]).await) == 2,
            "an existing branch is never reused"
        );
        for unsafe_id in [
            "",
            "../escape",
            "a/b",
            r"a\b",
            ".hidden",
            "-flag",
            "x.lock",
            "a..b",
            "sp ace",
            "\u{D55C}\u{AE00}",
            "nul",
            "CON",
            "com1.txt",
            "LPT9",
            "a:b",
            "trailing.",
        ] {
            // Device names are refused on Windows only (`is_safe_session_id`): a folder named so is plain elsewhere.
            if !cfg!(windows) && ["nul", "CON", "com1.txt", "LPT9"].contains(&unsafe_id) {
                assert!(is_safe_session_id(unsafe_id), "{unsafe_id} is a plain name off Windows");
                continue;
            }
            assert!(
                !is_safe_session_id(unsafe_id)
                    && matches!(maker.create(unsafe_id, &place.repo(), "t").await, ChildWorktreeOutcome::Failed(_)),
                "an unsafe id is never made: {unsafe_id}"
            );
        }
        assert!(
            is_safe_session_id(&mighty_core::wire::new_id())
                && is_safe_session_id(&uuid::Uuid::new_v4().to_string().to_uppercase())
                && is_safe_session_id("console")
                && is_safe_session_id("a.b_c-d"),
            "a session id is safe"
        );
        assert_eq!(place.mighty_branches().await, "refs/heads/mighty/child-1\nrefs/heads/mighty/taken", "no other branch was made");
        assert_eq!(entries(&place.root()), ["child-1"], "no other folder was made");
        drop(place);

        // A subfolder workspace works in the same subfolder of the worktree.
        let nested = DelegationPlace::new();
        nested.repository(1, &combine("app", "src"), "false").await;
        let child = made(nested.maker().create("child-sub", &combine(&combine(&nested.repo(), "app"), "src"), "t").await);
        assert!(
            child.working_folder == combine(&combine(&child.worktree_path, "app"), "src")
                && Path::new(&combine(&child.working_folder, "file-0.txt")).exists()
                && Path::new(&combine(&combine(&child.worktree_path, ".mighty-delegation"), "TASK.md")).exists(),
            "a subfolder workspace works in the same subfolder"
        );
    }

    /// "delegation: a workspace that cannot host a child is refused before anything is made"
    #[tokio::test]
    async fn a_workspace_that_cannot_host_a_child_is_refused_before_anything_is_made() {
        let _lane = lane::enter().await;
        let plenty = || -> Arc<dyn DiskSpace> { Arc::new(FixedDiskSpace(50_000_000_000)) };
        let refused = |reason| ChildWorktreeOutcome::Refused(reason);
        async fn nothing_created(place: &DelegationPlace) {
            assert!(
                !Path::new(&place.root()).exists()
                    && place.mighty_branches().await.is_empty()
                    && listed(&place.fixture_git(&["worktree", "list", "--porcelain"]).await) == 1,
                "nothing was created"
            );
        }
        let (plain, unborn, detached, low) =
            (DelegationPlace::new(), DelegationPlace::new(), DelegationPlace::new(), DelegationPlace::new());
        // Outside git, a missing folder, or a path that is not full.
        std::fs::create_dir_all(plain.repo()).unwrap();
        let maker = Maker::new(&plain.root(), plain.git(), plenty());
        for workspace in [
            plain.repo(),
            combine(&plain.repo(), "missing"),
            relative(),
            r"\rooted\only".into(),
            "C:relative".into(),
            format!("{}\0x", plain.repo()),
        ] {
            assert_eq!(
                maker.create("c", &workspace, "t").await,
                refused(DelegationReasonCode::NotGit),
                "not git: {}",
                workspace.replace('\0', "<NUL>")
            );
        }
        assert!(!Path::new(&plain.root()).exists(), "nothing was created outside git");

        unborn.repository(0, "", "false").await;
        assert_eq!(
            Maker::new(&unborn.root(), unborn.git(), plenty()).create("c", &unborn.repo(), "t").await,
            refused(DelegationReasonCode::UnbornBranch),
            "a branch with no commit is refused"
        );
        nothing_created(&unborn).await;

        detached.repository(2, "", "false").await;
        detached.fixture_git(&["checkout", "-q", "--detach", "HEAD~1"]).await;
        assert_eq!(
            Maker::new(&detached.root(), detached.git(), plenty()).create("c", &detached.repo(), "t").await,
            refused(DelegationReasonCode::DetachedHead),
            "a detached HEAD is refused"
        );
        nothing_created(&detached).await;

        // Under 10 GB free on the root's volume, or unknown.
        low.repository(1, "", "false").await;
        let asked = Arc::new(Asked { paths: Mutex::new(vec![]), free: Some(MINIMUM_FREE_BYTES - 1) });
        assert_eq!(
            Maker::new(&low.root(), low.git(), asked.clone()).create("c", &low.repo(), "t").await,
            refused(DelegationReasonCode::LowDisk),
            "under 10 GB is refused"
        );
        assert_eq!(*asked.paths.lock().unwrap(), [PathBuf::from(low.root())], "the free space asked about is the worktree root's");
        let unknown = Arc::new(Asked { paths: Mutex::new(vec![]), free: None });
        assert_eq!(
            Maker::new(&low.root(), low.git(), unknown).create("c", &low.repo(), "t").await,
            refused(DelegationReasonCode::LowDisk),
            "unknown free space counts as low"
        );
        nothing_created(&low).await;
        let exactly = Arc::new(FixedDiskSpace(MINIMUM_FREE_BYTES));
        assert!(
            matches!(Maker::new(&low.root(), low.git(), exactly).create("c", &low.repo(), "t").await, ChildWorktreeOutcome::Created(_)),
            "exactly 10 GB free is enough"
        );
        // A check refused for its disk alone still counts as a place that can host a child.
        assert!(
            ChildWorktreeCheck::Refused(DelegationReasonCode::LowDisk).can_host_worktree()
                && !ChildWorktreeCheck::Refused(DelegationReasonCode::NotGit).can_host_worktree()
        );
    }

    /// A tool merge that must be refused with `reason` and change nothing.
    async fn tool_refuses(
        place: &DelegationPlace,
        watched: &str,
        subject: &PlaceChild,
        expected: &str,
        checkout: &str,
        reason: DelegationReasonCode,
        why: &str,
    ) {
        let before = place.snapshot(&[watched]).await;
        assert_eq!(merge::tool_merge(&place.git(), &subject.view(), expected, checkout).await, ChildMergeOutcome::Refused(reason), "{why}");
        assert_eq!(place.snapshot(&[watched]).await, before, "{why}: nothing changed");
    }

    /// "delegation: the tool's merge fast-forwards or refuses, changing nothing"
    #[tokio::test]
    async fn the_tools_merge_fast_forwards_or_refuses_changing_nothing() {
        let _lane = lane::enter().await;
        let not_reported = ChildMergeOutcome::Refused(DelegationReasonCode::NotReported);
        {
            let place = DelegationPlace::new();
            let git = place.git();
            let start = place.repository_default().await;
            let child = place.child("c1", None).await;
            place.commit(&child.worktree_path, "a.txt", "a\n").await;
            let head = place.commit(&child.worktree_path, "b.txt", "b\n").await;
            std::fs::write(report_file_in(&child.worktree_path), "# Report\nDone.\n").unwrap();
            // Running with no report yet, and every other state even with a report head on record.
            let before = place.snapshot(&[&child.worktree_path]).await;
            assert_eq!(
                merge::tool_merge(&git, &child.view(), &head, &place.repo()).await,
                not_reported,
                "a running child has not reported"
            );
            for state in ChildState::ALL.iter().copied().filter(|s| *s != ChildState::Reported) {
                let subject = PlaceChild { report_head: Some(head.clone()), ..child.with_state(state) };
                assert_eq!(merge::tool_merge(&git, &subject.view(), &head, &place.repo()).await, not_reported, "{state:?} is not reported");
            }
            assert_eq!(place.snapshot(&[&child.worktree_path]).await, before, "a refusal changes nothing");
            let child = child.reported(&head);
            let record = merged(merge::tool_merge(&git, &child.view(), &head, &place.repo()).await);
            assert_eq!(
                record,
                MergeRecord {
                    child_id: "c1".into(),
                    kind: MergeKind::ToolFastForward,
                    parent_branch: "main".into(),
                    pre_merge_commit: start.clone(),
                    merged_commit: head.clone(),
                    child_head: head.clone()
                },
                "the fast-forward writes its merge record"
            );
            // A fast-forward alone: main is still checked out, now at the child's head, with no merge commit and a clean checkout.
            assert!(
                place.fixture_git(&["symbolic-ref", "HEAD"]).await == "refs/heads/main"
                    && place.fixture_git(&["rev-parse", "HEAD"]).await == head
                    && place.fixture_git(&["rev-list", "--count", &format!("{start}..HEAD")]).await == "2"
                    && place.fixture_git(&["rev-list", "--merges", "HEAD"]).await.is_empty()
                    && place.fixture_git(&["status", "--porcelain", "--untracked-files=all"]).await.is_empty()
                    && std::fs::read_to_string(combine(&place.repo(), "b.txt")).unwrap() == "b\n",
                "main fast-forwarded to the child's head"
            );
            assert!(
                place.fixture_git(&["rev-parse", "refs/heads/mighty/c1"]).await == head
                    && place.git_in(&child.worktree_path, &["symbolic-ref", "HEAD"]).await == "refs/heads/mighty/c1",
                "the child's branch and checkout are only read"
            );
            // A merged child is no longer reported: a second merge is refused.
            let before = place.snapshot(&[&child.worktree_path]).await;
            assert!(
                merge::tool_merge(&git, &child.with_state(ChildState::Merged).view(), &head, &place.repo()).await == not_reported
                    && place.snapshot(&[&child.worktree_path]).await == before,
                "a merged child is not merged again"
            );
        }

        let place = DelegationPlace::new();
        let git = place.git();
        let start = place.repository_default().await;
        let (child, head) = place.reported_child("c1", &["a.txt"], None).await;
        let watched = child.worktree_path.clone();
        // A moved head: the expected head is not the reported one.
        for wrong in [child.base_commit.as_str(), &head[..12], &"0".repeat(40), "HEAD", "mighty/c1", ""] {
            tool_refuses(
                &place,
                &watched,
                &child,
                wrong,
                &place.repo(),
                DelegationReasonCode::HeadMoved,
                &format!("expected head {wrong}"),
            )
            .await;
        }
        // An unstaged, then a staged change to a tracked file in the child.
        std::fs::write(combine(&child.worktree_path, "a.txt"), "changed\n").unwrap();
        tool_refuses(
            &place,
            &watched,
            &child,
            &head,
            &place.repo(),
            DelegationReasonCode::TrackedChanges,
            "an unstaged change in the child",
        )
        .await;
        place.git_in(&child.worktree_path, &["add", "a.txt"]).await;
        tool_refuses(&place, &watched, &child, &head, &place.repo(), DelegationReasonCode::TrackedChanges, "a staged change in the child")
            .await;
        place.git_in(&child.worktree_path, &["reset", "-q", "--hard"]).await;
        // An untracked file and REPORT.md in the child do not count; a tracked change in the parent does.
        std::fs::write(combine(&child.worktree_path, "untracked.txt"), "note\n").unwrap();
        std::fs::write(combine(&place.repo(), "file-0.txt"), "parent edit\n").unwrap();
        tool_refuses(
            &place,
            &watched,
            &child,
            &head,
            &place.repo(),
            DelegationReasonCode::TrackedChanges,
            "a tracked change in the parent",
        )
        .await;
        place.fixture_git(&["checkout", "-q", "--", "file-0.txt"]).await;
        // The parent's side: another branch at the same commit, a detached HEAD, a folder that is no checkout of it.
        place.fixture_git(&["checkout", "-q", "-b", "other"]).await;
        tool_refuses(
            &place,
            &watched,
            &child,
            &head,
            &place.repo(),
            DelegationReasonCode::BranchNotCheckedOut,
            "another branch checked out",
        )
        .await;
        place.fixture_git(&["checkout", "-q", "--detach", "main"]).await;
        tool_refuses(&place, &watched, &child, &head, &place.repo(), DelegationReasonCode::BranchNotCheckedOut, "a detached parent").await;
        for folder in [place.base(), combine(&place.base(), "missing"), relative()] {
            tool_refuses(
                &place,
                &watched,
                &child,
                &head,
                &folder,
                DelegationReasonCode::BranchNotCheckedOut,
                &format!("no checkout at {folder}"),
            )
            .await;
        }
        place.fixture_git(&["checkout", "-q", "main"]).await;
        // A worktree that is gone.
        let gone = PlaceChild { worktree_path: combine(&place.root(), "gone"), ..child.clone() };
        tool_refuses(&place, &watched, &gone, &head, &place.repo(), DelegationReasonCode::WorktreeMissing, "the worktree is gone").await;
        // The upper-case head a human might paste is the same commit, and the parent's folder may be written any way Windows reads it.
        let (sibling, sibling_head) = place.reported_child("c2", &["s.txt"], None).await;
        std::fs::write(combine(&place.repo(), "scratch.txt"), "scratch\n").unwrap();
        // (Windows ignores case and reads either separator; the Mac compares paths as they are, as Swift did.)
        let checkout =
            if cfg!(windows) { format!("{}/", place.repo().to_uppercase().replace('\\', "/")) } else { format!("{}/", place.repo()) };
        let record = merged(merge::tool_merge(&git, &child.view(), &format!(" {}\n", head.to_uppercase()), &checkout).await);
        assert!(record.pre_merge_commit == start && record.merged_commit == head, "untracked files on either side do not block the merge");
        assert!(
            std::fs::read_to_string(combine(&place.repo(), "scratch.txt")).unwrap() == "scratch\n"
                && Path::new(&combine(&child.worktree_path, "untracked.txt")).exists(),
            "untracked files stay"
        );
        // Once one child is merged, a sibling made from the same base has diverged.
        tool_refuses(&place, &watched, &sibling, &sibling_head, &place.repo(), DelegationReasonCode::Diverged, "diverged").await;
        // The child's branch moved past its report, or its checkout left its branch though at the reported head.
        let (moved, moved_head) = place.reported_child("c3", &["m.txt"], None).await;
        let later = place.commit(&moved.worktree_path, "m2.txt", "later\n").await;
        let head_moved = ChildMergeOutcome::Refused(DelegationReasonCode::HeadMoved);
        assert!(
            merge::tool_merge(&git, &moved.view(), &moved_head, &place.repo()).await == head_moved
                && merge::tool_merge(&git, &moved.view(), &later, &place.repo()).await == head_moved,
            "the child's branch moved past its report"
        );
        place.git_in(&moved.worktree_path, &["reset", "-q", "--hard", &moved_head]).await;
        place.git_in(&moved.worktree_path, &["checkout", "-q", "--detach", &moved_head]).await;
        assert_eq!(
            merge::tool_merge(&git, &moved.view(), &moved_head, &place.repo()).await,
            head_moved,
            "the child's checkout left its branch"
        );
    }

    /// A card merge that must be refused with `reason` and change nothing.
    async fn card_refuses(place: &DelegationPlace, subject: &PlaceChild, checkout: &str, reason: DelegationReasonCode, why: &str) {
        let before = place.snapshot(&[]).await;
        assert_eq!(merge::card_merge(&place.git(), &subject.view(), checkout, &en()).await, ChildMergeOutcome::Refused(reason), "{why}");
        assert_eq!(place.snapshot(&[]).await, before, "{why}: nothing changed");
    }

    /// "delegation: the card merges with at most one merge commit, and undoes"
    #[tokio::test]
    async fn the_card_merges_with_at_most_one_merge_commit_and_undoes() {
        let _lane = lane::enter().await;
        let locale = en();
        // Fast-forwards when it can, and undo returns the child to reported.
        {
            let place = DelegationPlace::new();
            let git = place.git();
            let start = place.repository_default().await;
            let (child, head) = place.reported_child("c1", &["a.txt"], None).await;
            let record = merged(merge::card_merge(&git, &child.view(), &place.repo(), &locale).await);
            assert_eq!(
                record,
                MergeRecord {
                    child_id: "c1".into(),
                    kind: MergeKind::CardFastForward,
                    parent_branch: "main".into(),
                    pre_merge_commit: start.clone(),
                    merged_commit: head.clone(),
                    child_head: head.clone()
                },
                "the card fast-forwards when it can"
            );
            assert!(
                place.fixture_git(&["rev-parse", "HEAD"]).await == head
                    && place.fixture_git(&["rev-list", "--merges", "HEAD"]).await.is_empty(),
                "no merge commit for a fast-forward"
            );
            let merged_child = child.with_state(ChildState::Merged);
            // An open, idle parent: the branch, index and files go back; an untracked file stays.
            std::fs::write(combine(&place.repo(), "scratch.txt"), "scratch\n").unwrap();
            assert_eq!(
                merge::undo(&git, &record, &merged_child.view(), &place.repo(), Some(DelegationPaneActivity::Finished), &locale).await,
                ChildUndoOutcome::Undone { record: record.clone(), restored_branch: false },
                "the undo goes through"
            );
            assert!(
                place.fixture_git(&["symbolic-ref", "HEAD"]).await == "refs/heads/main"
                    && place.fixture_git(&["rev-parse", "HEAD"]).await == start
                    && place.fixture_git(&["status", "--porcelain", "--untracked-files=no"]).await.is_empty()
                    && !Path::new(&combine(&place.repo(), "a.txt")).exists()
                    && std::fs::read_to_string(combine(&place.repo(), "scratch.txt")).unwrap() == "scratch\n"
                    && place.fixture_git(&["rev-parse", "refs/heads/mighty/c1"]).await == head,
                "the branch, index and files are back, and an untracked file stays"
            );
            // Reported again at the same head, it can be merged again.
            assert!(
                matches!(merge::card_merge(&git, &child.view(), &place.repo(), &locale).await, ChildMergeOutcome::Merged(_))
                    && place.fixture_git(&["rev-parse", "HEAD"]).await == head,
                "it can be merged again"
            );
        }

        // Leaves exactly one merge commit when the parent moved on, and undoes it.
        {
            let place = DelegationPlace::new();
            let git = place.git();
            place.repository_default().await;
            let (child, head) = place.reported_child("c1", &["a.txt", "b.txt"], None).await;
            let moved = place.commit(&place.repo(), "parent.txt", "p\n").await;
            let record = merged(merge::card_merge(&git, &child.view(), &place.repo(), &locale).await);
            assert!(
                record.kind == MergeKind::CardMergeCommit
                    && record.parent_branch == "main"
                    && record.pre_merge_commit == moved
                    && record.child_head == head,
                "the card makes a merge commit when the parent moved on"
            );
            // One merge commit on top of the old parent head, its parents the old parent head and the child head.
            assert!(
                place.fixture_git(&["rev-parse", "HEAD"]).await == record.merged_commit
                    && place.fixture_git(&["rev-list", "--parents", "-n", "1", "HEAD"]).await
                        == format!("{} {moved} {head}", record.merged_commit)
                    && place.fixture_git(&["rev-list", "--merges", &format!("{moved}..HEAD")]).await == record.merged_commit
                    && place.fixture_git(&["rev-list", "--count", &format!("{moved}..HEAD")]).await == "3"
                    && place.fixture_git(&["status", "--porcelain", "--untracked-files=all"]).await.is_empty(),
                "exactly one merge commit"
            );
            assert!(
                place.fixture_git(&["log", "-1", "--format=%s"]).await == "Merge branch 'mighty/c1' into main"
                    && ["a.txt", "b.txt", "parent.txt"].iter().all(|name| Path::new(&combine(&place.repo(), name)).exists())
                    && place.fixture_git(&["rev-parse", "refs/heads/mighty/c1"]).await == head,
                "the merge holds both sides"
            );
            let merged_child = child.with_state(ChildState::Merged);
            assert!(
                merge::undo(&git, &record, &merged_child.view(), &place.repo(), Some(DelegationPaneActivity::Idle), &locale).await
                    == ChildUndoOutcome::Undone { record: record.clone(), restored_branch: false }
                    && place.fixture_git(&["rev-parse", "HEAD"]).await == moved
                    && place.fixture_git(&["status", "--porcelain", "--untracked-files=all"]).await.is_empty()
                    && place.fixture_git(&["ls-files"]).await == "file-0.txt\nparent.txt",
                "the merge commit is undone"
            );

            // Undo on a closed child makes its branch again and leaves it closed.
            let second = merged(merge::card_merge(&git, &child.view(), &place.repo(), &locale).await);
            place.fixture_git(&["worktree", "remove", &child.worktree_path]).await;
            place.fixture_git(&["branch", "-d", "mighty/c1"]).await;
            let closed = child.with_state(ChildState::Closed);
            assert!(
                merge::undo(&git, &second, &closed.view(), &place.repo(), None, &locale).await
                    == ChildUndoOutcome::Undone { record: second.clone(), restored_branch: true }
                    && place.fixture_git(&["rev-parse", "refs/heads/mighty/c1"]).await == head
                    && place.fixture_git(&["rev-parse", "HEAD"]).await == moved
                    && place.fixture_git(&["status", "--porcelain", "--untracked-files=all"]).await.is_empty(),
                "a closed parent counts as idle, and the deleted branch is made again"
            );
        }

        // Refuses a conflict, tracked changes or another branch, changing nothing.
        let place = DelegationPlace::new();
        let git = place.git();
        let start = place.repository_default().await;
        let clashing = place.child("c1", None).await;
        let clash = place.commit(&clashing.worktree_path, "file-0.txt", "child\n").await;
        let clashing = clashing.reported(&clash);
        let (child, _head) = place.reported_child("c2", &["a.txt"], None).await;
        place.commit(&place.repo(), "file-0.txt", "parent\n").await;
        card_refuses(&place, &clashing, &place.repo(), DelegationReasonCode::MergeConflict, "both sides changed the same line").await;
        assert!(!Path::new(&place.repo()).join(".git").join("MERGE_HEAD").exists(), "no merge was started");
        std::fs::write(combine(&place.repo(), "file-0.txt"), "edit\n").unwrap();
        card_refuses(&place, &child, &place.repo(), DelegationReasonCode::TrackedChanges, "an unstaged change in the parent").await;
        place.fixture_git(&["add", "file-0.txt"]).await;
        card_refuses(&place, &child, &place.repo(), DelegationReasonCode::TrackedChanges, "a staged change in the parent").await;
        place.fixture_git(&["reset", "-q", "--hard"]).await;
        place.fixture_git(&["checkout", "-q", "-b", "other"]).await;
        card_refuses(&place, &child, &place.repo(), DelegationReasonCode::BranchNotCheckedOut, "another branch").await;
        place.fixture_git(&["checkout", "-q", "--detach", "main"]).await;
        card_refuses(&place, &child, &place.repo(), DelegationReasonCode::BranchNotCheckedOut, "a detached HEAD").await;
        for folder in [place.base(), relative()] {
            card_refuses(&place, &child, &folder, DelegationReasonCode::BranchNotCheckedOut, &format!("no checkout at {folder}")).await;
        }
        place.fixture_git(&["checkout", "-q", "main"]).await;
        for state in ChildState::ALL.iter().copied().filter(|s| *s != ChildState::Reported) {
            card_refuses(
                &place,
                &child.with_state(state),
                &place.repo(),
                DelegationReasonCode::NotReported,
                &format!("{state:?} is not reported"),
            )
            .await;
        }
        assert_eq!(place.fixture_git(&["rev-list", "--count", &format!("{start}..main")]).await, "1", "the parent branch did not move");

        // Undo is refused unless the branch is checked out at the merge, the parent idle and tracked files clean.
        let record = merged(merge::card_merge(&git, &child.view(), &place.repo(), &locale).await);
        let merged_child = child.with_state(ChildState::Merged);
        let undo_refuses = async |reason: DelegationReasonCode, activity: Option<DelegationPaneActivity>| {
            let before = place.snapshot(&[]).await;
            assert_eq!(
                merge::undo(&git, &record, &merged_child.view(), &place.repo(), activity, &locale).await,
                ChildUndoOutcome::Refused(reason),
                "undo refuses with {reason:?}"
            );
            assert_eq!(place.snapshot(&[]).await, before, "{reason:?}: nothing changed");
        };
        let idle = Some(DelegationPaneActivity::Idle);
        place.fixture_git(&["checkout", "-q", "-b", "elsewhere"]).await;
        undo_refuses(DelegationReasonCode::BranchNotCheckedOut, idle).await;
        place.fixture_git(&["checkout", "-q", "main"]).await;
        undo_refuses(DelegationReasonCode::ParentBusy, Some(DelegationPaneActivity::Running)).await;
        std::fs::write(combine(&place.repo(), "a.txt"), "edit\n").unwrap();
        undo_refuses(DelegationReasonCode::TrackedChanges, idle).await;
        place.fixture_git(&["checkout", "-q", "--", "a.txt"]).await;
        // A file the undo would put back is in the way as an untracked file: never overwritten.
        place.fixture_git(&["rm", "-q", "file-0.txt"]).await;
        place.fixture_git(&["commit", "-q", "-m", "drop file-0"]).await;
        let dropped = place.fixture_git(&["rev-parse", "HEAD"]).await;
        std::fs::write(combine(&place.repo(), "file-0.txt"), "mine\n").unwrap();
        let in_the_way = place.snapshot(&[]).await;
        let swapped = MergeRecord { pre_merge_commit: record.merged_commit.clone(), merged_commit: dropped, ..record.clone() };
        assert!(
            matches!(merge::undo(&git, &swapped, &merged_child.view(), &place.repo(), idle, &locale).await, ChildUndoOutcome::Failed(_))
                && place.snapshot(&[]).await == in_the_way
                && std::fs::read_to_string(combine(&place.repo(), "file-0.txt")).unwrap() == "mine\n",
            "an untracked file in the way fails the undo and is never overwritten"
        );
        std::fs::remove_file(combine(&place.repo(), "file-0.txt")).unwrap();
        // The branch moved past the merged commit.
        undo_refuses(DelegationReasonCode::UndoParentMoved, idle).await;
        place.fixture_git(&["reset", "-q", "--hard", &record.merged_commit]).await;
        // A record of another child, or a child that ran again since the merge, has nothing to undo.
        let other = PlaceChild { id: "c9".into(), ..merged_child.clone() };
        let bad = MergeRecord { child_head: "not a commit".into(), ..record.clone() };
        assert!(
            merge::undo(&git, &record, &other.view(), &place.repo(), idle, &locale).await
                == ChildUndoOutcome::Failed(locale.get("delegation.card.error.noMergeToUndo"))
                && matches!(
                    merge::undo(&git, &record, &merged_child.with_state(ChildState::Running).view(), &place.repo(), idle, &locale).await,
                    ChildUndoOutcome::Failed(_)
                )
                && merge::undo(&git, &bad, &merged_child.view(), &place.repo(), idle, &locale).await
                    == ChildUndoOutcome::Failed(locale.get("delegation.card.error.badMergeRecord"))
                && place.fixture_git(&["rev-parse", "HEAD"]).await == record.merged_commit,
            "nothing to undo for another child's record or a child that ran again"
        );
        assert!(
            merge::undo(&git, &record, &merged_child.view(), &place.repo(), Some(DelegationPaneActivity::Finished), &locale).await
                == ChildUndoOutcome::Undone { record: record.clone(), restored_branch: false }
                && place.fixture_git(&["rev-parse", "HEAD"]).await == record.pre_merge_commit,
            "with every condition met, the undo goes through"
        );
    }

    /// C# `MergedChild`: a child linked to the parent's checkout, which committed `<id>.txt`, reported and was fast-forward-merged by
    /// the parent's tool.
    async fn merged_child(place: &DelegationPlace, id: &str) -> (PlaceChild, MergeRecord) {
        let (child, head) = place.reported_child(id, &[], Some(&place.repo())).await;
        let record = merged(merge::tool_merge(&place.git(), &child.view(), &head, &place.repo()).await);
        (child.with_state(ChildState::Merged), record)
    }

    /// C# `Ignore`: adds `pattern` to the repository's shared exclude file, which every worktree of it reads.
    fn ignore(place: &DelegationPlace, pattern: &str) {
        use std::io::Write as _;
        let path = Path::new(&place.repo()).join(".git").join("info").join("exclude");
        std::fs::OpenOptions::new().append(true).create(true).open(path).unwrap().write_all(format!("{pattern}\n").as_bytes()).unwrap();
    }

    /// A cleanup that must keep everything for `hold` with nothing on disk changed.
    async fn kept(
        place: &DelegationPlace,
        hold: ChildCleanupHold,
        subject: &PlaceChild,
        record: &MergeRecord,
        activity: Option<DelegationPaneActivity>,
    ) {
        let before = place.disk().await;
        assert_eq!(
            cleanup::clean_up(&place.git(), &place.paths(), &subject.view(), record, &place.repo(), activity).await,
            ChildCleanupOutcome::Kept(hold),
            "cleanup keeps everything for {hold:?}"
        );
        assert_eq!(place.disk().await, before, "{hold:?}: nothing on disk changed");
    }

    /// "delegation: cleanup never forces, and discard removes what is left"
    #[tokio::test]
    async fn cleanup_never_forces_and_discard_removes_what_is_left() {
        let _lane = lane::enter().await;
        let locale = en();
        let finished = Some(DelegationPaneActivity::Finished);
        {
            let place = DelegationPlace::new();
            place.repository_default().await;
            let (child, merge) = merged_child(&place, "c1").await;
            // Not merged: every other state, or another child's merge record. A run going in the child's pane.
            for state in ChildState::ALL.iter().copied().filter(|s| *s != ChildState::Merged) {
                kept(&place, ChildCleanupHold::NotMerged, &child.with_state(state), &merge, finished).await;
            }
            kept(&place, ChildCleanupHold::NotMerged, &child, &MergeRecord { child_id: "c9".into(), ..merge.clone() }, finished).await;
            kept(&place, ChildCleanupHold::ChildBusy, &child, &merge, Some(DelegationPaneActivity::Running)).await;
            // The worktree is gone, or is a folder the parent's repository does not list.
            kept(
                &place,
                ChildCleanupHold::WorktreeMissing,
                &PlaceChild { worktree_path: combine(&place.root(), "gone"), ..child.clone() },
                &merge,
                finished,
            )
            .await;
            let stray = combine(&place.root(), "stray");
            std::fs::create_dir_all(&stray).unwrap();
            kept(
                &place,
                ChildCleanupHold::WorktreeMissing,
                &PlaceChild { worktree_path: stray.clone(), ..child.clone() },
                &merge,
                finished,
            )
            .await;
            std::fs::remove_dir(&stray).unwrap();
            // The parent's checkout left the recorded branch, or that branch no longer holds the child's head.
            place.fixture_git(&["checkout", "-q", "-b", "elsewhere"]).await;
            kept(&place, ChildCleanupHold::BranchNotCheckedOut, &child, &merge, finished).await;
            place.fixture_git(&["checkout", "-q", "main"]).await;
            place.fixture_git(&["branch", "-q", "-D", "elsewhere"]).await;
            place.fixture_git(&["reset", "-q", "--hard", &child.base_commit]).await;
            kept(&place, ChildCleanupHold::NotInParent, &child, &merge, finished).await;
            place.fixture_git(&["reset", "-q", "--hard", &merge.merged_commit]).await;
            // The child's branch moved past the merge, or its worktree left the branch at the merged head.
            place.commit(&child.worktree_path, "later.txt", "later\n").await;
            kept(&place, ChildCleanupHold::HeadMoved, &child, &merge, finished).await;
            place.git_in(&child.worktree_path, &["reset", "-q", "--hard", &merge.child_head]).await;
            place.git_in(&child.worktree_path, &["checkout", "-q", "--detach"]).await;
            kept(&place, ChildCleanupHold::HeadMoved, &child, &merge, finished).await;
            place.git_in(&child.worktree_path, &["checkout", "-q", &child.branch]).await;
            // A tracked change, unstaged and then staged; a file no ignore rule covers.
            std::fs::write(combine(&child.worktree_path, "c1.txt"), "changed\n").unwrap();
            kept(&place, ChildCleanupHold::TrackedChanges, &child, &merge, finished).await;
            place.git_in(&child.worktree_path, &["add", "c1.txt"]).await;
            kept(&place, ChildCleanupHold::TrackedChanges, &child, &merge, finished).await;
            place.git_in(&child.worktree_path, &["reset", "-q", "--hard"]).await;
            std::fs::write(combine(&child.worktree_path, "notes.txt"), "note\n").unwrap();
            kept(&place, ChildCleanupHold::UntrackedFiles, &child, &merge, finished).await;
            std::fs::remove_file(combine(&child.worktree_path, "notes.txt")).unwrap();
            // A registered worktree inside the child's, hidden by an ignore rule and holding work of its own.
            ignore(&place, "nested/");
            let nested = combine(&child.worktree_path, "nested");
            place.fixture_git(&["worktree", "add", "-q", "-b", "side", &nested, "main"]).await;
            std::fs::write(combine(&nested, "side.txt"), "side work\n").unwrap();
            kept(&place, ChildCleanupHold::NestedWorktree, &child, &merge, finished).await;
            let named = cleanup::nested_worktrees(&place.git(), &place.paths(), &child.worktree_path, &place.repo()).await.unwrap();
            assert!(
                named.len() == 1
                    && same(&named[0], &place.paths().canonical(&nested))
                    && std::fs::read_to_string(combine(&nested, "side.txt")).unwrap() == "side work\n",
                "the nested worktree is named, and untouched"
            );
            // The child's folder written another way is the same worktree (in another case only where paths ignore case: Windows).
            let (other_way, repo) = if cfg!(windows) {
                (format!("{}/", child.worktree_path.to_uppercase().replace('\\', "/")), place.repo().to_lowercase())
            } else {
                (format!("{}/", child.worktree_path), place.repo())
            };
            assert_eq!(
                cleanup::nested_worktrees(&place.git(), &place.paths(), &other_way, &repo).await.map(|n| n.len()),
                Some(1),
                "paths compare without regard to case or separator"
            );
            place.fixture_git(&["worktree", "remove", "--force", &nested]).await;
            // Ignored build output beside TASK.md and REPORT.md goes with the worktree.
            ignore(&place, "build/");
            std::fs::create_dir_all(combine(&child.worktree_path, "build")).unwrap();
            std::fs::write(combine(&combine(&child.worktree_path, "build"), "out.o"), "o\n").unwrap();
            // With every condition met, and the pane already closed, the cleanup goes through and leaves the nested worktree's branch alone.
            assert!(
                cleanup::clean_up(&place.git(), &place.paths(), &child.view(), &merge, &place.repo(), None).await
                    == ChildCleanupOutcome::Cleaned
                    && !Path::new(&child.worktree_path).exists()
                    && place.only_the_repository_is_listed().await
                    && place.fixture_git(&["for-each-ref", "--format=%(refname)", "refs/heads/"]).await
                        == "refs/heads/main\nrefs/heads/side"
                    && place.fixture_git(&["rev-parse", "refs/heads/main"]).await == merge.merged_commit,
                "the worktree goes by plain removal and the branch by branch -d"
            );
        }

        // Discard removes what a failed start left, and never a folder that is not the child's place.
        {
            let place = DelegationPlace::new();
            let (git, paths) = (place.git(), place.paths());
            place.repository_default().await;
            let maker = place.maker();
            let ChildWorktreeCheck::Ready(first) = maker.check(&place.repo()).await else { panic!("the workspace cannot host a child") };
            assert!(
                same(&paths.canonical(&first.repository), &paths.canonical(&place.repo()))
                    // Git prints its top folder with `/`; it is read in the OS's own separators (`\` on Windows, `/` on the Mac).
                    && !first.repository.contains(if cfg!(windows) { '/' } else { '\\' })
                    && first.prefix.is_empty()
                    && first.parent_branch == "main",
                "the repository's top folder is read the OS's way"
            );
            let failed = |id: &str| {
                PlaceChild::make(id, &maker.worktree_path(id), "main", &first.base_commit, ChildState::Failed, Some(&place.repo()))
            };
            let discard = async |child: &PlaceChild, checkout: &str, root: &str| {
                cleanup::discard(&git, &paths, &child.view(), checkout, root, &locale).await
            };
            // f1: its place was taken, so `git worktree add` failed.
            let taken = combine(&place.root(), "f1");
            std::fs::create_dir_all(&taken).unwrap();
            std::fs::write(combine(&taken, "junk.txt"), "junk\n").unwrap();
            assert!(
                matches!(maker.make("f1", &first, "Task\n", 300.0).await, ChildWorktreeOutcome::Failed(_)),
                "f1 was not made over a folder in its place"
            );
            // f2: its worktree was made but not its notes, since the workspace has a folder of that name.
            place.commit(&place.repo(), &format!("{NOTES_FOLDER}/keep"), "keep\n").await;
            let ChildWorktreeCheck::Ready(second) = maker.check(&place.repo()).await else { panic!("no second check") };
            assert!(
                matches!(maker.make("f2", &second, "Task\n", 300.0).await, ChildWorktreeOutcome::Failed(_))
                    && Path::new(&maker.worktree_path("f2")).is_dir()
                    && !Path::new(&task_file_in(&maker.worktree_path("f2"))).exists(),
                "f2's notes never replace a folder the repository has"
            );
            // f3: a start killed partway, its worktree still locked while being made and its folder gone.
            place.fixture_git(&["worktree", "add", "-q", "-b", "mighty/f3", &maker.worktree_path("f3"), "main"]).await;
            place.fixture_git(&["worktree", "lock", "--reason", "initializing", &maker.worktree_path("f3")]).await;
            remove_folder(Path::new(&maker.worktree_path("f3")));
            // f4: a start that failed before it made anything. f5: a leftover with a read-only file.
            std::fs::create_dir_all(maker.worktree_path("f5")).unwrap();
            let locked = combine(&maker.worktree_path("f5"), "locked.txt");
            std::fs::write(&locked, "read only\n").unwrap();
            let mut permissions = std::fs::metadata(&locked).unwrap().permissions();
            permissions.set_readonly(true);
            std::fs::set_permissions(&locked, permissions).unwrap();
            let main = place.fixture_git(&["rev-parse", "refs/heads/main"]).await;
            for id in ["f1", "f2", "f3", "f4", "f5"] {
                assert_eq!(
                    discard(&failed(id), &place.repo(), &place.root()).await,
                    ChildDiscardOutcome::Discarded,
                    "discard removes what the failed start of {id} left"
                );
            }
            // Nothing of them is left: no branch, no registration, no folder under the root; the parent's branch is untouched.
            assert!(
                place.mighty_branches().await.is_empty()
                    && place.only_the_repository_is_listed().await
                    && entries(&place.root()).is_empty()
                    && place.fixture_git(&["rev-parse", "refs/heads/main"]).await == main
                    && place.fixture_git(&["status", "--porcelain", "--untracked-files=all"]).await.is_empty(),
                "nothing of them is left"
            );
            // A folder that is not the child's place under the worktree root is never removed.
            let elsewhere = combine(&combine(&place.base(), "elsewhere"), "f6");
            std::fs::create_dir_all(&elsewhere).unwrap();
            std::fs::write(combine(&elsewhere, "keep.txt"), "mine\n").unwrap();
            // Something of the root's own, which a discard naming the root itself would take.
            let sentinel = combine(&combine(&place.root(), "kept"), "sentinel.txt");
            std::fs::create_dir_all(combine(&place.root(), "kept")).unwrap();
            std::fs::write(&sentinel, "root's\n").unwrap();
            let climbing = combine(&combine(&combine(&combine(&combine(&place.root(), "f6"), ".."), ".."), "elsewhere"), "f6");
            let dots =
                PlaceChild::make("..", &combine(&place.root(), ".."), "main", &first.base_commit, ChildState::Failed, Some(&place.repo()));
            for outside in [
                PlaceChild { worktree_path: elsewhere.clone(), ..failed("f6") },
                PlaceChild { worktree_path: place.root(), ..failed("f6") },
                PlaceChild { worktree_path: climbing, ..failed("f6") },
                dots,
            ] {
                assert_eq!(
                    discard(&outside, &place.repo(), &place.root()).await,
                    ChildDiscardOutcome::Failed(locale.get("delegation.card.error.worktreeMisplaced")),
                    "a folder outside the child's place is never removed: {}",
                    outside.worktree_path
                );
            }
            assert!(
                std::fs::read_to_string(combine(&elsewhere, "keep.txt")).unwrap() == "mine\n"
                    && std::fs::read_to_string(&sentinel).unwrap() == "root's\n",
                "the other folder and the root are as they were"
            );
            remove_folder(Path::new(&combine(&place.root(), "kept")));
            // The child's place written another way is still its place.
            std::fs::create_dir_all(maker.worktree_path("f7")).unwrap();
            // (In another case and with `/` only where paths ignore case: Windows.)
            let other_way = |path: String| if cfg!(windows) { path.to_uppercase().replace('\\', "/") } else { format!("{path}/") };
            let f7 = PlaceChild { worktree_path: other_way(maker.worktree_path("f7")), ..failed("f7") };
            let root_written = if cfg!(windows) { format!("{}\\", place.root()) } else { format!("{}/", place.root()) };
            assert!(
                discard(&f7, &place.repo(), &root_written).await == ChildDiscardOutcome::Discarded
                    && !Path::new(&maker.worktree_path("f7")).exists(),
                "the child's place written another way is still its place"
            );
            assert_eq!(
                discard(&failed("f8"), &combine(&place.base(), "missing"), &place.root()).await,
                ChildDiscardOutcome::Failed(locale.get("delegation.card.error.parentCheckoutUnreachable")),
                "a parent's folder that is gone fails the discard"
            );
        }

        // Removing a workspace: refused while it has an open child or a child worktree not cleaned up. Windows' paths as written there;
        // elsewhere the same folders under `/` (only Windows reads a folder in another case or with the other separator as the same).
        let paths = paths();
        let os = |path: &str| if cfg!(windows) { path.to_string() } else { path.trim_start_matches("C:").replace('\\', "/") };
        let removal = |id: &str, parent: &str, state: ChildState, checkout: Option<&str>| PlaceChild {
            parent_session_id: parent.into(),
            ..PlaceChild::make(id, &os(&format!(r"C:\worktrees\{id}")), "main", &"a".repeat(40), state, checkout)
        };
        let panes = |ids: &[&str]| ids.iter().map(|id| id.to_string()).collect::<HashSet<String>>();
        let refusal = |children: &[PlaceChild], pane_ids: &HashSet<String>, folder: &str, exists: &dyn Fn(&str) -> bool| {
            cleanup::workspace_removal_refusal(children.iter().map(PlaceChild::view), pane_ids, folder, &paths, exists)
        };
        let has_children = Some(DelegationReasonCode::WorkspaceHasChildren);
        let (work_app, work_app_closed) = (os(r"C:\work\app"), os(r"C:\work\app\"));
        let sub_written = if cfg!(windows) { "c:/WORK/app/sub".to_string() } else { "/work/app/sub/".to_string() };
        let app = Some(work_app.as_str());
        let never = |_: &str| false;
        for state in
            ChildState::ALL.iter().copied().filter(|s| !matches!(s, ChildState::Failed | ChildState::Closed | ChildState::Discarded))
        {
            // By the parent's pane, the child's own pane, or the parent's checkout alone.
            assert!(
                refusal(&[removal("c1", "p1", state, None)], &panes(&["p1"]), &work_app, &never) == has_children
                    && refusal(&[removal("c1", "gone", state, None)], &panes(&["c1"]), &work_app, &never) == has_children
                    && refusal(&[removal("c1", "gone", state, app)], &panes(&[]), &work_app, &never) == has_children
                    && refusal(&[removal("c1", "gone", state, Some(&sub_written))], &panes(&[]), &work_app_closed, &never) == has_children,
                "a workspace with an open child is refused: {state:?}"
            );
        }
        let worktree_c1 = os(r"C:\worktrees\c1");
        let on_disk = |path: &str| path == worktree_c1;
        for state in [ChildState::Closed, ChildState::Failed] {
            assert_eq!(
                refusal(&[removal("c1", "p1", state, app)], &panes(&[]), &work_app, &on_disk),
                has_children,
                "a child worktree not cleaned up refuses the removal"
            );
        }
        // A discarded child never counts; a cleaned one's worktree is gone.
        assert_eq!(
            refusal(&[removal("c1", "p1", ChildState::Discarded, app)], &panes(&["p1"]), &work_app, &on_disk),
            None,
            "a discarded child never counts"
        );
        assert!(
            refusal(&[removal("c1", "p1", ChildState::Closed, app)], &panes(&["p1"]), &work_app, &never).is_none()
                && refusal(&[removal("c1", "p1", ChildState::Failed, app)], &panes(&["p1"]), &work_app, &never).is_none(),
            "a cleaned child's worktree is gone"
        );
        // Another workspace's children never refuse the removal.
        let (other, apple) = (os(r"C:\work\other"), os(r"C:\work\apple"));
        let others =
            [removal("c1", "p1", ChildState::Running, Some(other.as_str())), removal("c2", "p2", ChildState::Closed, Some(apple.as_str()))];
        let always = |_: &str| true;
        assert!(
            refusal(&others, &panes(&["p3"]), &work_app, &always).is_none()
                && refusal(&others, &panes(&["p2"]), &work_app, &always) == has_children,
            "another workspace's children never refuse the removal"
        );
    }

    /// "delegation: Windows paths, line endings, long paths and folders held open" (its process-wide GIT_DIR half: tests/git_env.rs)
    #[cfg(windows)] // Windows' paths, junctions, 8.3 names, MAX_PATH and share modes.
    #[tokio::test]
    async fn windows_paths_line_endings_long_paths_and_folders_held_open() {
        let _lane = lane::enter().await;
        let locale = en();
        // Paths: full, one kind of separator, no closing one, no regard to case; as far as they exist, their final names.
        {
            let folder = temp("paths");
            let folder_text = folder.to_string_lossy().into_owned();
            let paths = paths();
            let real = combine(&folder_text, "Real Folder With A Long Name");
            std::fs::create_dir_all(&real).unwrap();
            assert!(
                standardized(&format!("{}//", real.replace('\\', "/"))) == real
                    && standardized(&combine(&combine(&combine(&real, ".."), "x"), ".")) == combine(&folder_text, "x")
                    && standardized("\0") == "\0",
                "a path is standardized"
            );
            assert!(
                same(&paths.canonical(&real.to_uppercase()), &paths.canonical(&real))
                    && paths.canonical(&real.to_uppercase().replace('\\', "/")).ends_with("Real Folder With A Long Name"),
                "the final name is the folder's own spelling"
            );
            let missing = paths.canonical(&combine(&combine(&real.to_lowercase(), "Not"), "There"));
            assert!(missing.ends_with(r"Real Folder With A Long Name\Not\There"), "a path is resolved as far as it exists: {missing}");
            assert!(
                is_inside(&combine(&real, "in"), &real.to_uppercase())
                    && !is_inside(&format!("{real}2"), &real)
                    && !is_inside(&real, &real)
                    && is_inside(r"C:\x", r"C:\"),
                "inside means below, not beside"
            );
            // A junction to the folder and its 8.3 short name (where the volume keeps one) are the same folder, as git lists it.
            let link = combine(&folder_text, "link");
            junction(&link, &real).await;
            assert!(
                same(&paths.canonical(&combine(&link, "sub")), &combine(&paths.canonical(&real), "sub")),
                "a junction resolves to the folder it names"
            );
            std::fs::remove_dir(&link).unwrap();
            let short = combine(&folder_text, "REALFO~1");
            if Path::new(&short).is_dir() {
                assert!(same(&paths.canonical(&short), &paths.canonical(&real)), "a short 8.3 name resolves to the long one");
            } else {
                eprintln!("note: the volume of {folder_text} keeps no 8.3 names, so the short-name check was skipped");
            }
            remove_folder(&folder);
        }

        // Line endings: with core.autocrlf on, a fresh worktree reads as unchanged, the notes never count, and merge, cleanup and
        // discard decide as with it off.
        {
            let place = DelegationPlace::new();
            let (git, paths) = (place.git(), place.paths());
            place.repository(1, "", "true").await;
            // Committed with LF, with CRLF, and mixed; git writes each out through the user's setting.
            place.commit(&place.repo(), "lf.txt", "one\ntwo\n").await;
            std::fs::write(combine(&place.repo(), "crlf.txt"), utf8("one\r\ntwo\r\n")).unwrap();
            place.fixture_git(&["-c", "core.autocrlf=false", "add", "crlf.txt"]).await;
            std::fs::write(combine(&place.repo(), "mixed.txt"), utf8("one\r\ntwo\nthree\r\n")).unwrap();
            place.fixture_git(&["-c", "core.autocrlf=false", "add", "mixed.txt"]).await;
            place.fixture_git(&["commit", "-q", "-m", "line endings"]).await;
            place.fixture_git(&["reset", "-q", "--hard"]).await;
            let task = "Fix it.\r\nKeep these endings.\nAnd these.\n";
            let worktree = made(place.maker().create("eol", &place.repo(), task).await);
            let path = worktree.worktree_path.clone();
            assert_eq!(
                std::fs::read(combine(&path, "lf.txt")).unwrap(),
                utf8("one\r\ntwo\r\n"),
                "git wrote the checkout through core.autocrlf"
            );
            assert!(
                std::fs::read(worktree.task_file()).unwrap() == utf8(task)
                    && std::fs::read(combine(&combine(&path, NOTES_FOLDER), ".gitignore")).unwrap() == utf8("*\n"),
                "the app's own files are written byte for byte"
            );
            // Git compares what it wrote a moment ago by content, not by its stamp, once the stamps are stale.
            let stale = std::time::SystemTime::now() - Duration::from_secs(7 * 60);
            for name in ["lf.txt", "crlf.txt", "mixed.txt", "file-0.txt"] {
                std::fs::File::options().write(true).open(combine(&path, name)).unwrap().set_modified(stale).unwrap();
            }
            assert!(
                merge::has_tracked_changes(&git, &path).await == Some(false)
                    && merge::has_tracked_changes(&git, &place.repo()).await == Some(false),
                "a fresh worktree is not changed by its line endings"
            );
            assert_eq!(
                run(&git, &["status", "--porcelain", "--ignore-submodules=none", "--untracked-files=normal"], &path, 10.0)
                    .await
                    .map(|r| r.stdout.len()),
                Some(0),
                "TASK.md, REPORT.md and .gitignore are no changes"
            );
            let child = PlaceChild::make("eol", &path, "main", &worktree.base_commit, ChildState::Running, Some(&place.repo()));
            std::fs::write(combine(&path, "work.txt"), utf8("work\r\nmore\r\n")).unwrap();
            place.git_in(&path, &["add", "--", "work.txt"]).await;
            place.git_in(&path, &["commit", "-q", "-m", "work"]).await;
            let head = place.git_in(&path, &["rev-parse", "HEAD"]).await;
            // The report is hashed byte for byte: the same words with other line endings are another report.
            std::fs::write(worktree.report_file(), utf8("Done.\r\nAll of it.\r\n")).unwrap();
            let windows = report_file::read(&worktree.report_file()).unwrap();
            std::fs::write(worktree.report_file(), utf8("Done.\nAll of it.\n")).unwrap();
            let unix = report_file::read(&worktree.report_file()).unwrap();
            assert!(
                windows.digest == sha256(b"Done.\r\nAll of it.\r\n")
                    && windows.digest != unix.digest
                    && windows.total_bytes == 19
                    && unix.total_bytes == 17,
                "the report's digest is of its bytes"
            );
            let child = child.reported(&head);
            let merge = merged(merge::tool_merge(&git, &child.view(), &head, &place.repo()).await);
            assert!(
                std::fs::read(combine(&place.repo(), "work.txt")).unwrap() == utf8("work\r\nmore\r\n")
                    && merge::has_tracked_changes(&git, &place.repo()).await == Some(false),
                "the merge leaves the parent's checkout unchanged in git's eyes"
            );
            // An undo and a merge again through the card, and then the cleanup on close: every decision rests on an empty status.
            let merged_child = child.with_state(ChildState::Merged);
            assert!(
                matches!(
                    merge::undo(&git, &merge, &merged_child.view(), &place.repo(), None, &locale).await,
                    ChildUndoOutcome::Undone { .. }
                ) && !Path::new(&combine(&place.repo(), "work.txt")).exists()
                    && merge::has_tracked_changes(&git, &place.repo()).await == Some(false),
                "the undo puts the files back"
            );
            let again = merged(merge::card_merge(&git, &child.view(), &place.repo(), &locale).await);
            assert!(
                cleanup::clean_up(&git, &paths, &merged_child.view(), &again, &place.repo(), None).await == ChildCleanupOutcome::Cleaned
                    && !Path::new(&path).exists(),
                "the cleanup removes the worktree with core.autocrlf on"
            );
        }

        // Long paths: a child's worktree sits deeper than its parent's checkout, so a file that fits there may pass 260 characters here.
        {
            let place = DelegationPlace::new();
            let (git, paths) = (place.git(), place.paths());
            place.repository_default().await;
            let id = format!("long-{}", "p".repeat(60));
            let mut parts = vec![];
            let mut budget = 250 - place.repo().chars().count() as i64 - 1 - 4;
            while budget > 60 {
                parts.push("d".repeat(50));
                budget -= 51;
            }
            parts.push(format!("{}.txt", "f".repeat(budget.max(1) as usize)));
            let name = parts.join("/");
            assert!(
                combine(&place.repo(), &name).chars().count() < 260 && combine(&combine(&place.root(), &id), &name).chars().count() > 260,
                "the file fits in the parent's checkout and not in the child's without long paths"
            );
            place.fixture_git(&["config", "core.longpaths", "false"]).await;
            place.commit(&place.repo(), &name, "deep\n").await;
            let (child, head) = place.reported_child(&id, &["work.txt"], Some(&place.repo())).await;
            let deep = format!(r"\\?\{}", combine(&child.worktree_path, &name.replace('/', "\\")));
            assert!(
                std::fs::read_to_string(&deep).unwrap() == "deep\n"
                    && merge::has_tracked_changes(&git, &child.worktree_path).await == Some(false),
                "the long file is checked out in the child's worktree"
            );
            let merge = merged(merge::tool_merge(&git, &child.view(), &head, &place.repo()).await);
            assert!(
                cleanup::clean_up(&git, &paths, &child.with_state(ChildState::Merged).view(), &merge, &place.repo(), None).await
                    == ChildCleanupOutcome::Cleaned
                    && !Path::new(&child.worktree_path).exists(),
                "a worktree with a long path is merged and cleaned up"
            );
            let (second, _) = place.reported_child(&format!("{id}2"), &["more.txt"], Some(&place.repo())).await;
            assert!(
                cleanup::discard(&git, &paths, &second.view(), &place.repo(), &place.root(), &locale).await
                    == ChildDiscardOutcome::Discarded
                    && !Path::new(&second.worktree_path).exists()
                    && place.mighty_branches().await.is_empty(),
                "and discarded"
            );
        }

        // A folder another program holds open: the discard says so, removes nothing after that step, and goes on when tried again.
        {
            use std::os::windows::fs::OpenOptionsExt as _;
            let place = DelegationPlace::new();
            let (git, paths) = (place.git(), place.paths());
            place.repository_default().await;
            let (child, head) = place.reported_child("held", &[], Some(&place.repo())).await;
            std::fs::write(combine(&child.worktree_path, "unsaved.txt"), "not committed\n").unwrap();
            {
                // FileShare.Read: no one may delete it while it is open.
                let _held = std::fs::OpenOptions::new().read(true).share_mode(1).open(combine(&child.worktree_path, "held.txt")).unwrap();
                assert!(
                    matches!(cleanup::discard(&git, &paths, &child.view(), &place.repo(), &place.root(), &locale).await, ChildDiscardOutcome::Failed(why) if !why.is_empty()),
                    "a discard that Windows refuses is a failure, in git's or Windows' words"
                );
                assert!(
                    Path::new(&combine(&child.worktree_path, "held.txt")).exists()
                        && place.fixture_git(&["rev-parse", "refs/heads/mighty/held"]).await == head,
                    "the file held open and the child's branch are still there"
                );
                // Tried again while still held: still an honest failure, and still the branch.
                assert!(
                    matches!(
                        cleanup::discard(&git, &paths, &child.view(), &place.repo(), &place.root(), &locale).await,
                        ChildDiscardOutcome::Failed(_)
                    ) && Path::new(&child.worktree_path).is_dir()
                        && place.fixture_git(&["rev-parse", "refs/heads/mighty/held"]).await == head,
                    "a second try while it is held fails the same way"
                );
            }
            assert!(
                cleanup::discard(&git, &paths, &child.view(), &place.repo(), &place.root(), &locale).await
                    == ChildDiscardOutcome::Discarded
                    && !Path::new(&child.worktree_path).exists()
                    && place.mighty_branches().await.is_empty()
                    && place.only_the_repository_is_listed().await,
                "once the other program lets go, the discard goes on and finishes"
            );
        }

        // Delegation's git: the user's settings, but no prompts, bounded output, and a time limit (no hooks and no inherited GIT_*:
        // tests/git_env.rs).
        let place = DelegationPlace::new();
        let git = place.git();
        place.repository_default().await;
        let inside = run(&git, &["rev-parse", "--is-inside-work-tree"], &place.repo(), 10.0).await.unwrap();
        assert!(inside.exit_code == 0 && line(&inside.stdout) == "true" && inside.stderr.is_empty(), "git runs in the folder it is given");
        let failed = run(&git, &["rev-parse", "--verify", "no-such-ref"], &place.repo(), 10.0).await.unwrap();
        assert!(
            failed.exit_code != 0 && message(&failed).starts_with("fatal:") && failed.stdout.is_empty(),
            "a failure carries git's own words, apart from its output"
        );
        let repo = PathBuf::from(place.repo());
        assert!(
            git.run_detailed(&["status"], &Path::new(&place.base()).join("missing"), 10.0, None).await
                == GitRun::failed(GitFailure::NotStarted)
                && run(&git, &["cat-file", "--batch"], &place.repo(), 60.0).await.is_some_and(|r| r.exit_code == 0),
            "a folder that is gone starts no git, and stdin is closed"
        );
        // A command past its time is stopped with everything it started, and is no answer: what it started never writes afterwards.
        let clock = Instant::now();
        let late = git.run_detailed(&["-c", "alias.wait=!sleep 4 && echo late > late.txt", "wait"], &repo, 1.0, None).await;
        assert!(
            late == GitRun::failed(GitFailure::TimedOut) && clock.elapsed() < Duration::from_secs(15),
            "a command past its time is no answer: {late:?}"
        );
        tokio::time::sleep(Duration::from_secs(6)).await;
        assert!(!repo.join("late.txt").exists(), "what the command started was stopped with it");
        std::fs::write(combine(&place.repo(), "big.txt"), "0123456789abcdef\n".repeat(70_000)).unwrap();
        assert_eq!(
            git.run_detailed(&["--no-pager", "diff", "--no-index", "--", "file-0.txt", "big.txt"], &repo, 10.0, None).await,
            GitRun::failed(GitFailure::TooMuchOutput),
            "more than 1 MiB of output is no answer"
        );
        assert!(
            is_object_name(&"a".repeat(40))
                && is_object_name(&"F".repeat(64))
                && !is_object_name(&"a".repeat(39))
                && !is_object_name(&"g".repeat(40))
                && !is_object_name("HEAD"),
            "an object name is 40 or 64 hex digits"
        );
    }

    // ---- DelegationHumanChildVerification ----

    /// "delegation: the app commits a Codex or Antigravity child's work" (its git: what `ChildAutoCommit.Run` commits and leaves out;
    /// the coordinator's half, a run's end and the pane's notes, is W7-A's)
    #[tokio::test]
    async fn the_app_commits_a_codex_or_antigravity_childs_work() {
        let _lane = lane::enter().await;
        // A Codex child: what .gitignore lets in goes into one commit for the report's revision; the same state commits nothing more.
        {
            let place = DelegationPlace::new();
            let (git, paths) = (place.git(), place.paths());
            place.repository_default().await;
            std::fs::write(combine(&place.repo(), ".gitignore"), "*.log\n").unwrap();
            place.fixture_git(&["add", ".gitignore"]).await;
            place.fixture_git(&["commit", "-q", "-m", "ignore logs"]).await;
            let start = place.fixture_git(&["rev-parse", "HEAD"]).await;
            let child = place.child("c1", Some(&place.repo())).await;
            let worktree = child.worktree_path.clone();
            std::fs::write(combine(&worktree, "note.txt"), "Note\n").unwrap();
            std::fs::write(report_file_in(&worktree), "# Report\n\nAdded the note.\n").unwrap();
            // A Claude child commits its own work: the app never does.
            let claude = auto_commit::run(&git, &paths, &worktree, "c1", 1, "claude").await;
            assert!(
                claude.outcome == AutoCommitOutcome::Nothing
                    && place.git_in(&worktree, &["rev-parse", "HEAD"]).await == start
                    && place.git_in(&worktree, &["status", "--porcelain"]).await == "?? note.txt",
                "a Claude child commits its own work"
            );
            std::fs::write(combine(&worktree, "file-0.txt"), "changed\n").unwrap();
            std::fs::write(combine(&worktree, "build.log"), "noise\n").unwrap();
            let done = auto_commit::run(&git, &paths, &worktree, "c1", 1, "codex").await;
            assert!(
                done.outcome == AutoCommitOutcome::Committed && done.left_out.is_empty() && done.why.is_none(),
                "the app commits: {done:?}"
            );
            let head = place.git_in(&worktree, &["rev-parse", "HEAD"]).await;
            assert!(
                head != start
                    && place.git_in(&worktree, &["log", "-1", "--format=%s"]).await == auto_commit::message_for("c1", 1)
                    && place.git_in(&worktree, &["rev-parse", "HEAD^"]).await == start
                    && place.git_in(&worktree, &["diff", "--name-only", &start, &head]).await == "file-0.txt\nnote.txt"
                    && place.git_in(&worktree, &["status", "--porcelain"]).await.is_empty(),
                "the app's one commit holds what .gitignore lets in"
            );
            assert_eq!(
                auto_commit::run(&git, &paths, &worktree, "c1", 2, "codex").await.outcome,
                AutoCommitOutcome::Nothing,
                "nothing changed: nothing more is committed"
            );
            assert_eq!(place.git_in(&worktree, &["rev-parse", "HEAD"]).await, head);
            // A folder that is not the child's own worktree is never committed.
            let sub = combine(&worktree, "sub");
            std::fs::create_dir_all(&sub).unwrap();
            let not_own = auto_commit::run(&git, &paths, &sub, "c1", 2, "codex").await;
            assert!(
                not_own.outcome == AutoCommitOutcome::Failed
                    && not_own.why.as_deref() == Some("The child's folder is not its own git worktree."),
                "{not_own:?}"
            );
        }

        // New files that look secret or are large stay uncommitted, and are named.
        {
            let place = DelegationPlace::new();
            let (git, paths) = (place.git(), place.paths());
            place.repository_default().await;
            let child = place.child("c1", Some(&place.repo())).await;
            let worktree = child.worktree_path.clone();
            std::fs::create_dir_all(combine(&worktree, "keys")).unwrap();
            let secrets = [".env.local", "keys/server.pem", "keys/deploy.key", "id_rsa", "credentials.json"];
            for name in secrets.iter().chain(&["config.json"]) {
                std::fs::write(combine(&worktree, name), "x\n").unwrap();
            }
            std::fs::write(combine(&worktree, "big.bin"), vec![0u8; MAXIMUM_NEW_FILE_BYTES as usize + 1]).unwrap();
            std::fs::write(combine(&worktree, "file-0.txt"), "changed\n").unwrap();
            std::fs::write(report_file_in(&worktree), "# Report\n").unwrap();
            let done = auto_commit::run(&git, &paths, &worktree, "c1", 1, "gemini").await;
            let mut expected: Vec<String> = secrets.iter().chain(&["big.bin"]).map(|s| s.to_string()).collect();
            expected.sort();
            let mut left_out = done.left_out.clone();
            left_out.sort();
            assert!(done.outcome == AutoCommitOutcome::Committed && left_out == expected, "the left-out files are named: {done:?}");
            let mut committed: Vec<String> =
                place.git_in(&worktree, &["show", "--name-only", "--format=", "HEAD"]).await.split('\n').map(str::to_string).collect();
            committed.sort();
            assert_eq!(committed, ["config.json", "file-0.txt"], "the commit");
            let mut stayed: Vec<String> =
                place.git_in(&worktree, &["ls-files", "--others", "--exclude-standard"]).await.split('\n').map(str::to_string).collect();
            stayed.sort();
            assert_eq!(stayed, expected, "what stayed out");
        }

        // An unresolved conflict is never committed; resolved, the next end commits the merge.
        let place = DelegationPlace::new();
        let (git, paths) = (place.git(), place.paths());
        place.repository_default().await;
        place.fixture_git(&["checkout", "-q", "-b", "other"]).await;
        std::fs::write(combine(&place.repo(), "file-0.txt"), "theirs\n").unwrap();
        place.fixture_git(&["commit", "-q", "-am", "theirs"]).await;
        place.fixture_git(&["checkout", "-q", "main"]).await;
        let child = place.child("c1", Some(&place.repo())).await;
        let worktree = child.worktree_path.clone();
        std::fs::write(combine(&worktree, "file-0.txt"), "ours\n").unwrap();
        place.git_in(&worktree, &["commit", "-q", "-am", "ours"]).await;
        let ours = place.git_in(&worktree, &["rev-parse", "HEAD"]).await;
        assert!(run(&git, &["merge", "-q", "other"], &worktree, 10.0).await.is_some_and(|r| r.exit_code != 0), "the merge conflicts");
        std::fs::write(report_file_in(&worktree), "# Report\n\nMerged other.\n").unwrap();
        assert_eq!(
            auto_commit::run(&git, &paths, &worktree, "c1", 1, "codex").await,
            auto_commit::AutoCommitResult::of(AutoCommitOutcome::Unmerged),
            "nothing is committed"
        );
        assert!(
            place.git_in(&worktree, &["rev-parse", "HEAD"]).await == ours && !place.git_in(&worktree, &["ls-files", "-u"]).await.is_empty(),
            "the conflict stays as it was"
        );
        std::fs::write(combine(&worktree, "file-0.txt"), "both\n").unwrap();
        place.git_in(&worktree, &["add", "file-0.txt"]).await;
        assert_eq!(auto_commit::run(&git, &paths, &worktree, "c1", 1, "codex").await.outcome, AutoCommitOutcome::Committed);
        let head = place.git_in(&worktree, &["rev-parse", "HEAD"]).await;
        assert!(
            head != ours && place.git_in(&worktree, &["rev-list", "--parents", "-n", "1", "HEAD"]).await.split(' ').count() == 3,
            "resolved, the merge is committed"
        );
    }

    /// "delegation: git says why it gave no result, and only its own lock goes"
    #[tokio::test]
    async fn git_says_why_it_gave_no_result_and_only_its_own_lock_goes() {
        let _lane = lane::enter().await;
        let place = DelegationPlace::new();
        let git = place.git();
        let repo = PathBuf::from(place.repo());
        place.repository(1, "", "false").await;
        let head = place.fixture_git(&["rev-parse", "HEAD"]).await;
        // A git that does not read its input closes the pipe: told apart, with its own result.
        let unread = git.run_detailed(&["rev-parse", "HEAD"], &repo, 60.0, Some(&"x".repeat(8_000_000))).await;
        assert!(
            unread.failure == GitFailure::BrokenPipe
                && unread.result.as_ref().is_some_and(|r| r.exit_code == 0 && r.stdout.trim() == head)
                && git.run_detailed(&["rev-parse", "HEAD"], &repo, 60.0, Some(&"x".repeat(8_000_000))).await.ok().is_none(),
            "a broken pipe is told apart: {unread:?}"
        );
        // Input is written while the output is read, within the deadline: one too short ends it as a timeout.
        let big = "x".repeat(20_000_000);
        let late = git.run_detailed(&["hash-object", "--stdin"], &repo, 0.01, Some(&big)).await;
        assert!(late.failure == GitFailure::TimedOut && late.result.is_none(), "the app's own kill on its deadline is a timeout: {late:?}");
        // A run whose caller stops waiting (its future dropped) ends git with everything it started.
        let dropped = tokio::time::timeout(
            Duration::from_secs(1),
            git.run_detailed(&["-c", "alias.wait=!sleep 3 && echo late > dropped.txt", "wait"], &repo, 60.0, None),
        )
        .await;
        assert!(dropped.is_err(), "the run was still going when its future was dropped");
        tokio::time::sleep(Duration::from_secs(5)).await;
        assert!(!repo.join("dropped.txt").exists(), "git and what it started ended with the dropped future");
        let hashed = git.run_detailed(&["hash-object", "--stdin"], &repo, 60.0, Some("hello\n")).await;
        assert!(
            hashed.failure == GitFailure::None
                && hashed.result.as_ref().is_some_and(|r| r.exit_code == 0 && is_object_name(r.stdout.trim())),
            "input reaches git: {hashed:?}"
        );
        // A lock git refuses on is not the app's: it stays. One the app's killed git left is removed.
        let lock_file = combine(&combine(&place.repo(), ".git"), "index.lock");
        std::fs::write(combine(&place.repo(), "file-0.txt"), "changed\n").unwrap();
        std::fs::write(&lock_file, "").unwrap();
        let refused = auto_commit::write_step(&git, &place.repo(), &lock_file, &["add", "-u"], None, 60.0).await;
        assert!(
            refused.as_deref().is_some_and(|why| why.contains("index.lock")) && Path::new(&lock_file).exists(),
            "another git's lock stays: {refused:?}"
        );
        assert!(
            auto_commit::write_step(&git, &place.repo(), &lock_file, &["hash-object", "--stdin"], Some(&big), 0.01).await.as_deref()
                == Some("Git did not finish in time.")
                && !Path::new(&lock_file).exists(),
            "the lock of the app's own killed git goes"
        );
    }

    // ---- DelegationAppVerification ----

    /// "delegation: a junction in a child worktree never reaches what it points to"
    #[cfg(windows)] // Junctions are Windows' (`mklink /J`).
    #[tokio::test]
    async fn a_junction_in_a_child_worktree_never_reaches_what_it_points_to() {
        let _lane = lane::enter().await;
        let locale = en();
        let place = DelegationPlace::new();
        let (git, paths) = (place.git(), place.paths());
        place.repository_default().await;
        std::fs::write(combine(&place.repo(), ".gitignore"), "node_modules/\n").unwrap();
        place.fixture_git(&["add", "-A"]).await;
        place.fixture_git(&["commit", "-q", "-m", "ignore"]).await;
        let outside = combine(&place.base(), "outside");
        std::fs::create_dir_all(&outside).unwrap();
        let sentinel = combine(&outside, "sentinel.txt");
        std::fs::write(&sentinel, "keep me").unwrap();
        // A merged child with an ignored node_modules linked to the outside folder: cleanup keeps everything.
        let (child, head) = place.reported_child("c1", &[], None).await;
        // Only a directory link holds: a folder's files, a file link where Windows lets one be made, and what lies past a junction never do.
        let plain = combine(&place.base(), "plain");
        std::fs::create_dir_all(combine(&plain, "sub")).unwrap();
        std::fs::write(combine(&combine(&plain, "sub"), "a.txt"), "a").unwrap();
        let _ = std::os::windows::fs::symlink_file(combine(&combine(&plain, "sub"), "a.txt"), combine(&plain, "file-link.txt"));
        assert!(
            !cleanup::has_links(&plain).unwrap() && cleanup::links(&plain).unwrap().is_empty(),
            "files and file links are no directory link"
        );
        let j = combine(&combine(&plain, "sub"), "j");
        junction(&j, &outside).await;
        std::fs::create_dir_all(combine(&outside, "inner")).unwrap();
        junction(&combine(&combine(&outside, "inner"), "k"), &plain).await;
        assert!(
            cleanup::has_links(&plain).unwrap() && cleanup::links(&plain).unwrap() == [j.clone()],
            "a junction is one, and the walk never goes past it"
        );
        std::fs::remove_dir(combine(&combine(&outside, "inner"), "k")).unwrap();
        std::fs::remove_dir(combine(&outside, "inner")).unwrap();
        std::fs::create_dir_all(combine(&child.worktree_path, "node_modules")).unwrap();
        junction(&combine(&combine(&child.worktree_path, "node_modules"), "linked"), &outside).await;
        let merge = merged(merge::tool_merge(&git, &child.view(), &head, &place.repo()).await);
        let before = place.disk().await;
        assert_eq!(
            cleanup::clean_up(&git, &paths, &child.with_state(ChildState::Merged).view(), &merge, &place.repo(), None).await,
            ChildCleanupOutcome::Kept(ChildCleanupHold::Link),
            "cleanup holds for the junction"
        );
        assert!(
            place.disk().await == before
                && Path::new(&sentinel).exists()
                && Path::new(&child.worktree_path).is_dir()
                && place.mighty_branches().await.contains(&child.branch),
            "the outside folder, the worktree and the branch stay, and nothing on disk changed"
        );
        // A discard unlinks the junction itself first, then removes everything of the child's.
        assert!(
            cleanup::discard(&git, &paths, &child.view(), &place.repo(), &place.root(), &locale).await == ChildDiscardOutcome::Discarded
                && std::fs::read_to_string(&sentinel).unwrap() == "keep me"
                && !Path::new(&child.worktree_path).exists(),
            "the discard leaves the outside folder whole"
        );
        // Only the child's own branch is ever deleted by force.
        let (second, _) = place.reported_child("c2", &[], None).await;
        let wrong = PlaceChild { branch: "main".into(), ..second.clone() };
        assert!(
            cleanup::discard(&git, &paths, &wrong.view(), &place.repo(), &place.root(), &locale).await
                == ChildDiscardOutcome::Failed(locale.get("delegation.card.error.branchMisplaced"))
                && Path::new(&second.worktree_path).is_dir()
                && !place.fixture_git(&["rev-parse", "--verify", "-q", "refs/heads/main"]).await.is_empty(),
            "a record naming another branch is refused before anything goes"
        );
    }

    /// Beyond C# (W7 review M1): a child's worktree folder replaced by a junction to another registered worktree. The cleanup keeps
    /// everything, the app's commit refuses it, and the discard unlinks the junction alone: the other worktree, its files, its
    /// registration and its branch stay, and the child's own registration and branch go.
    #[cfg(windows)] // Junctions are Windows' (`mklink /J`); the symlink variant below is the Mac's.
    #[tokio::test]
    async fn a_worktree_folder_replaced_by_a_junction_never_reaches_what_it_points_to() {
        worktree_folder_replaced_by_a_link().await;
    }

    /// `DelegationCleanupTests.swift "aWorktreeFolderReplacedByASymlinkNeverReachesWhatItPointsTo"` (71b4aa1a): the same with a
    /// symbolic link. Swift's cleanup keeps it as `worktreeMissing`; this app keeps it as [`ChildCleanupHold::Link`] on both systems.
    #[cfg(unix)] // A symbolic link to a folder; Windows' is the junction variant above (a symlink there needs a privilege).
    #[tokio::test]
    async fn a_worktree_folder_replaced_by_a_symlink_never_reaches_what_it_points_to() {
        worktree_folder_replaced_by_a_link().await;
    }

    /// A folder link at `link` to `target`: a junction on Windows, a symbolic link elsewhere.
    async fn link_folder(link: &str, target: &str) {
        #[cfg(windows)]
        junction(link, target).await;
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, link).unwrap();
    }

    /// Takes away the folder link `link` alone (a junction is removed as a folder, a symbolic link as a file).
    fn unlink_folder(link: &str) {
        std::fs::remove_dir(link).or_else(|_| std::fs::remove_file(link)).unwrap();
    }

    async fn worktree_folder_replaced_by_a_link() {
        let _lane = lane::enter().await;
        let locale = en();
        let place = DelegationPlace::new();
        let (git, paths) = (place.git(), place.paths());
        place.repository_default().await;
        let (child, head) = place.reported_child("c1", &[], Some(&place.repo())).await;
        let (other, other_head) = place.reported_child("c2", &[], Some(&place.repo())).await;
        std::fs::write(combine(&other.worktree_path, "unsaved.txt"), "c2's own work\n").unwrap();
        std::fs::write(combine(&other.worktree_path, "c2.txt"), "c2 changed\n").unwrap();
        let merge = merged(merge::tool_merge(&git, &child.view(), &head, &place.repo()).await);
        // c1's folder goes (its registration stays) and a link to c2's worktree takes its place.
        remove_folder(Path::new(&child.worktree_path));
        link_folder(&child.worktree_path, &other.worktree_path).await;
        let other_state = async || {
            (
                std::fs::read_to_string(combine(&other.worktree_path, "unsaved.txt")).ok(),
                place.git_in(&other.worktree_path, &["status", "--porcelain"]).await,
                place.fixture_git(&["rev-parse", "refs/heads/mighty/c2"]).await,
            )
        };
        let untouched = other_state().await;
        assert!(untouched.0.is_some() && untouched.2 == other_head);

        let before = place.disk().await;
        assert_eq!(
            cleanup::clean_up(&git, &paths, &child.with_state(ChildState::Merged).view(), &merge, &place.repo(), None).await,
            ChildCleanupOutcome::Kept(ChildCleanupHold::Link),
            "cleanup keeps everything for a worktree folder that is a link"
        );
        assert!(place.disk().await == before && other_state().await == untouched, "nothing changed");
        let commit = auto_commit::run(&git, &paths, &child.worktree_path, "c1", 1, "codex").await;
        assert!(
            commit.outcome == AutoCommitOutcome::Failed
                && commit.why.as_deref() == Some("The child's folder is not its own git worktree.")
                && other_state().await == untouched,
            "the app never commits through the link: {commit:?}"
        );
        // Its place is still its place, named as the link and not as what it points to.
        assert!(same(&paths.canonical_place(&child.worktree_path), &combine(&paths.canonical(&place.root()), "c1")));
        assert!(!same(&paths.canonical(&child.worktree_path), &paths.canonical_place(&child.worktree_path)));
        // A record naming a link elsewhere than its own place is refused, and that link stays.
        let elsewhere = combine(&place.root(), "elsewhere");
        link_folder(&elsewhere, &other.worktree_path).await;
        let misplaced = PlaceChild { worktree_path: elsewhere.clone(), ..child.clone() };
        assert_eq!(
            cleanup::discard(&git, &paths, &misplaced.view(), &place.repo(), &place.root(), &locale).await,
            ChildDiscardOutcome::Failed(locale.get("delegation.card.error.worktreeMisplaced")),
            "a link elsewhere than the child's place is refused"
        );
        assert!(is_link(&elsewhere) && other_state().await == untouched, "and stays, with what it points to");
        unlink_folder(&elsewhere);
        assert_eq!(
            cleanup::discard(&git, &paths, &child.view(), &place.repo(), &place.root(), &locale).await,
            ChildDiscardOutcome::Discarded,
            "the discard goes through"
        );
        assert!(
            !Path::new(&child.worktree_path).exists() && Path::new(&other.worktree_path).is_dir() && other_state().await == untouched,
            "only the link went: the other worktree and its work stay"
        );
        let listed = place.worktree_list().await;
        assert!(
            listed.len() == 2
                && listed.iter().any(|path| same(path, &paths.canonical(&other.worktree_path)))
                && place.mighty_branches().await == "refs/heads/mighty/c2",
            "the other worktree stays registered with its branch, and the child's registration and branch are gone: {listed:?}"
        );
    }

    /// "delegation: git ends on its deadline, and reports and the file read the Windows way" (the delegation file's half, read again
    /// while another program holds it, is W7-A's store check)
    #[cfg(windows)] // A junction as the hooks folder and Windows' free-space reader.
    #[tokio::test]
    async fn git_ends_on_its_deadline_and_reports_and_the_file_read_the_windows_way() {
        let _lane = lane::enter().await;
        let folder = temp("deadline");
        let folder_text = folder.to_string_lossy().into_owned();
        let git = Git::new(processes(), folder.join("profile").join("git-no-hooks")).with_global_config(folder.join("fixture.gitconfig"));
        std::fs::write(folder.join("fixture.gitconfig"), "").unwrap();
        // A process git started keeps its output open after git exits: the deadline still ends the call, and that process with it.
        let timer = Instant::now();
        let held = git.run_detailed(&["-c", "alias.hold=!(sleep 5; echo late > late.txt) &", "hold"], &folder, 3.0, None).await;
        assert!(
            held == GitRun::failed(GitFailure::TimedOut) && timer.elapsed() < Duration::from_secs(30),
            "a held pipe ends with the deadline, after {:?}: {held:?}",
            timer.elapsed()
        );
        tokio::time::sleep(Duration::from_secs(5)).await;
        assert!(!folder.join("late.txt").exists(), "the process holding the pipe ended with the deadline");
        assert!(git.hooks().is_absolute() && git.program().is_some_and(Path::is_absolute), "git and its hooks folder are absolute");
        // Every git is pointed at this hooks folder, the profile's and never the install folder, for hooks and signature programs.
        let hooks = git.hooks().to_string_lossy().into_owned();
        for setting in ["core.hooksPath", "gpg.program", "gpg.ssh.program", "gpg.x509.program"] {
            let read = run(&git, &["config", "--get", setting], &folder_text, 10.0).await;
            assert!(read.as_ref().is_some_and(|r| r.exit_code == 0 && line(&r.stdout) == hooks), "{setting} is the hooks folder: {read:?}");
        }
        let install = std::env::current_exe().unwrap().parent().unwrap().to_path_buf();
        assert!(git.hooks().starts_with(&folder) && !git.hooks().starts_with(&install), "the hooks folder is the profile's");
        // Git does not run while anything is in the hooks folder.
        assert!(
            run(&git, &["--version"], &folder_text, 10.0).await.is_some_and(|r| r.exit_code == 0),
            "git runs while the hooks folder is empty"
        );
        std::fs::write(git.hooks().join("pre-commit"), "exit 1").unwrap();
        assert!(run(&git, &["--version"], &folder_text, 10.0).await.is_none(), "git does not run while the hooks folder holds a file");
        assert_eq!(
            git.run_detailed(&["--version"], &folder, 10.0, None).await.failure,
            GitFailure::NotStarted,
            "and says it did not start"
        );
        // Nor while the hooks folder is a junction, even to an empty folder.
        let empty = folder.join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        let linked = folder.join("linked-hooks");
        junction(&linked.to_string_lossy(), &empty.to_string_lossy()).await;
        let through = Git::new(processes(), linked.clone()).with_global_config(folder.join("fixture.gitconfig"));
        assert_eq!(
            through.run_detailed(&["--version"], &folder, 10.0, None).await.failure,
            GitFailure::NotStarted,
            "a hooks folder that is a junction is no plain folder"
        );
        std::fs::remove_dir(&linked).unwrap();
        let disk = mighty_platform_windows::WindowsDiskSpace;
        assert!(
            disk.available_bytes(&folder).is_ok_and(|b| b > 0)
                && disk.available_bytes(&folder.join("not").join("made")).is_ok_and(|b| b > 0),
            "free space is read on the folder's own volume"
        );

        // REPORT.md by its byte-order mark, hashed as its bytes.
        let report = combine(&folder_text, "REPORT.md");
        let text = "# Report\n\nDone, \u{D55C}\u{AE00}.\n";
        let le: Vec<u8> = [0xFF, 0xFE].into_iter().chain(text.encode_utf16().flat_map(u16::to_le_bytes)).collect();
        let be: Vec<u8> = [0xFE, 0xFF].into_iter().chain(text.encode_utf16().flat_map(u16::to_be_bytes)).collect();
        let bom: Vec<u8> = [0xEF, 0xBB, 0xBF].into_iter().chain(text.bytes()).collect();
        for (name, bytes) in [("utf-16", le), ("utf-16BE", be), ("utf-8 BOM", bom), ("utf-8", utf8(text))] {
            std::fs::write(&report, &bytes).unwrap();
            let read = report_file::read(&report).expect("no report");
            assert!(
                read.digest == sha256(&bytes) && read.start == bytes && read.total_bytes == bytes.len() as u64,
                "{name} reads as its bytes"
            );
        }
        let white: Vec<u8> = [0xFF, 0xFE].into_iter().chain(" \n".encode_utf16().flat_map(u16::to_le_bytes)).collect();
        std::fs::write(&report, white).unwrap();
        assert!(report_file::read(&report).is_none(), "a UTF-16 report of whitespace is no report");
        // A report over 64 KiB keeps its start, one byte past the copy's limit, and its whole digest.
        let long = "r".repeat(200_000);
        std::fs::write(&report, &long).unwrap();
        let read = report_file::read(&report).unwrap();
        assert!(
            read.start.len() == 64 * 1024 + 1 && read.total_bytes == 200_000 && read.digest == sha256(long.as_bytes()),
            "a long report keeps its start"
        );
        remove_folder(&folder);
    }
}
