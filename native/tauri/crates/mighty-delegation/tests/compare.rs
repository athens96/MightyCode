//! 비교 실행 (MC-29) over the real run path: every child of one compare run starts from the commit it chose, the same agent may run
//! more than once, one child that cannot start is refused on its own while the others go on, the app hears each child's pane before
//! its first run, and the shortstat reads what a child changed against that commit. Git and the fake Claude run through the
//! platform's process host, as in desktop.rs.

mod support;

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mighty_core::locale::Locale;
use mighty_delegation::app::{DelegationWindow, HumanChildExtras};
use mighty_delegation::compare::{self, CompareStat};
use mighty_delegation::git::worktree::{ChildWorktreeCheck, ChildWorktreeOutcome};
use mighty_delegation::host::DelegationAgentAvailability;
use mighty_delegation::model::ChildKind;
use mighty_delegation::tools::delegate::HumanChildStart;
use support::launch::{DelegationLaunch, PARENT, awaited};
use support::{lane, place::DelegationPlace};

/// The window's part: Antigravity is not installed here.
struct NoAntigravity;

impl DelegationWindow for NoAntigravity {
    fn agent_availability(&self, provider: &str) -> DelegationAgentAvailability {
        if provider == "gemini" { DelegationAgentAvailability::NotInstalled } else { DelegationAgentAvailability::Ready }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_childs_base_moves_only_to_a_commit_on_the_branch_and_its_shortstat_reads_its_work() {
    let _lane = lane::enter().await;
    let place = DelegationPlace::new();
    place.repository(3, "", "false").await;
    let first = place.fixture_git(&["rev-list", "--max-parents=0", "HEAD"]).await;
    let maker = place.maker();
    let ChildWorktreeCheck::Ready(start) = maker.check(&place.repo()).await else { panic!("the repository can host a child") };
    let moved = maker.at_commit(&start, &first).await.expect("the first commit is on the branch");
    assert_eq!(moved.base_commit, first);
    assert_eq!(maker.at_commit(&start, &start.base_commit).await.map(|s| s.base_commit), Some(start.base_commit.clone()));
    // A commit off the branch, a name that is no commit, an option: refused.
    place.fixture_git(&["checkout", "-q", "-b", "side"]).await;
    let side = place.commit(&place.repo(), "side.txt", "side\n").await;
    place.fixture_git(&["checkout", "-q", "main"]).await;
    assert!(maker.at_commit(&start, &side).await.is_none(), "a commit off the branch");
    assert!(maker.at_commit(&start, "0123456789abcdef0123456789abcdef01234567").await.is_none(), "no such commit");
    assert!(maker.at_commit(&start, "--all").await.is_none(), "an option");

    let ChildWorktreeOutcome::Created(child) = maker.make("cmp-a", &moved, "Do the work.\n", 60.0).await else { panic!("made") };
    assert_eq!(compare::shortstat(&place.git(), &place.repo(), &first, &child.branch).await, Some(CompareStat::default()));
    place.commit(&child.worktree_path, "work.txt", "one\ntwo\n").await;
    assert_eq!(
        compare::shortstat(&place.git(), &place.repo(), &first, &child.branch).await,
        Some(CompareStat { files: 1, insertions: 2, deletions: 0 }),
        "only the child's own commit, not the branch's later ones"
    );
    assert_eq!(compare::shortstat(&place.git(), &place.repo(), &first, "mighty/none").await, None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_compare_run_starts_each_row_from_one_commit_and_one_refusal_leaves_the_rest() {
    Box::pin(compare_run()).await;
}

async fn compare_run() {
    let _lane = lane::enter().await;
    let place = DelegationPlace::new();
    place.repository(2, "", "false").await;
    let first = place.fixture_git(&["rev-list", "--max-parents=0", "HEAD"]).await;
    let s = DelegationLaunch::start_with(
        Path::new(&place.profile()),
        &Path::new(&place.base()).join("fake"),
        &place.repo(),
        &place.root(),
        place.git(),
        "auto",
        true,
        mighty_delegation::app::MERGE_RETRY,
        Arc::new(NoAntigravity),
    )
    .await;
    let prepared: Arc<Mutex<Vec<String>>> = Arc::default();
    let extras = || {
        let prepared = prepared.clone();
        HumanChildExtras {
            base_commit: Some(first.clone()),
            prepare: Some(Arc::new(move |id: String| prepared.lock().unwrap().push(id))),
            compare_group: Some("group-1".into()),
        }
    };
    let none: Arc<dyn Fn(String) -> mighty_platform::BoxFuture<'static, ()> + Send + Sync> = Arc::new(|_| Box::pin(async {}));
    // "2× Claude" and an Antigravity row that cannot start.
    let mut started = vec![];
    for agent in ["claude", "claude", "gemini"] {
        started
            .push(s.app.start_human_child_with(PARENT, "Make it faster.", vec![], false, none.clone(), Some(agent), false, extras()).await);
    }
    let ids: Vec<String> = started
        .iter()
        .filter_map(|answer| match answer {
            HumanChildStart::Started(child) => Some(child.id.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(ids.len(), 2, "both Claude rows started: {started:?}");
    assert_ne!(ids[0], ids[1], "each row is its own child");
    assert_eq!(
        started[2],
        HumanChildStart::Failed(Locale::new("en").get_with("delegation.human.error.agentNotInstalledWindows", &[("agent", "Antigravity")])),
        "the row that cannot start says why, on its own"
    );
    let file = s.coordinator().file();
    for id in &ids {
        let record = file.children.iter().find(|child| &child.id == id).expect("its record");
        assert_eq!(record.base_commit, first, "every child starts from the chosen commit");
        assert_eq!(record.started_by_human, Some(true));
        assert_eq!(record.compare_group.as_deref(), Some("group-1"), "MC-45: it counts against the compare runs' cap");
    }
    assert_eq!(s.app.open_children(PARENT, ChildKind::Compare), 2);
    assert_eq!(s.app.open_children(PARENT, ChildKind::Human), 0, "the human children's room is left alone");
    awaited("both panes prepared", || (prepared.lock().unwrap().len() == 2).then_some(())).await;
    let mut heard = prepared.lock().unwrap().clone();
    heard.sort();
    let mut expected = ids.clone();
    expected.sort();
    assert_eq!(heard, expected, "the app hears each child's pane before its first run");
    // A base that is not on the branch makes nothing.
    let refused = s
        .app
        .start_human_child_with(
            PARENT,
            "Make it faster.",
            vec![],
            false,
            none,
            Some("claude"),
            false,
            HumanChildExtras { base_commit: Some("0123456789abcdef0123456789abcdef01234567".into()), ..HumanChildExtras::default() },
        )
        .await;
    assert_eq!(refused, HumanChildStart::Failed(Locale::new("en").get("compare.error.base")));
    let settle = std::time::Instant::now() + Duration::from_secs(30);
    while std::time::Instant::now() < settle && ids.iter().any(|id| s.coordinator().is_starting(id)) {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    s.dispose().await;
}
