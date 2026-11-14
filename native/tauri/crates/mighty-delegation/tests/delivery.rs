//! Delivery and follow-ups (C# MightyCode.Core.Tests/DelegationDeliveryVerification.cs, ported from the Mac's DelegationDeliveryTests
//! and DelegationFollowUpTests): each notice goes to its child's parent pane and each follow-up to its child's pane, exactly once, by
//! steer, by the next run, or held. No git runs here (no child has a REPORT.md), so these run on every OS. The phone's queued rows
//! (`DelegationQueueRows.MobileItems`) are W8's; 바로 실행's decision is mighty-composer's (`tests/queue.rs`).

mod support;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use mighty_delegation::coordinator::{Coordinator, MAXIMUM_FOLLOW_UP_BYTES, RunStarter};
use mighty_delegation::delivery::{
    AutomaticDelivery, DelegationDeliveryItem, DelegationDeliveryKind, DelegationRelease, automatic, follow_up_text, notice_text, now,
    remove_refusal, report_file_in,
};
use mighty_delegation::git::Git;
use mighty_delegation::git::worktree::Maker;
use mighty_delegation::host::{DelegationPaneActivity, DelegationPaneState};
use mighty_delegation::model::{
    ChildRecord, ChildRecordOptions, ChildState, DeliveryLane, DeliveryReceipt, DeliveryRoute, FollowUp, Notice, NoticeKind,
};
use mighty_delegation::reports::ChildRunEnd;
use mighty_delegation::request::{DelegationReasonCode, DelegationResponse};
use mighty_delegation::store::{DelegationFile, DelegationFileStore};
use mighty_platform::fake::FixedDiskSpace;
use support::host::{TestHost, caller, request};
use support::lane;
use support::place::{remove_folder, temp};

use DelegationPaneActivity::{Finished, Idle, Running};

const COMMIT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn released(run_id: &str, ids: &[&str]) -> DelegationRelease {
    DelegationRelease::Released { run_id: run_id.into(), item_ids: ids.iter().map(|id| id.to_string()).collect() }
}

fn texts(notices: &[&Notice], more: &[&str]) -> String {
    notices.iter().map(|n| notice_text(n)).chain(more.iter().map(|s| s.to_string())).collect::<Vec<_>>().join("\n\n")
}

fn held(notice: &Notice) -> Notice {
    Notice { lane: DeliveryLane::Held, ..notice.clone() }
}

/// A profile in a temp folder with the delegation file, and a coordinator over it. The pane "parent" has the running children c1, c2
/// and c3, each on its first run "r-<id>", which delegate started. No child has a REPORT.md, so each run's end sends one
/// ended_without_report notice.
struct Fixture {
    base: PathBuf,
    host: Arc<TestHost>,
    coordinator: Coordinator,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        remove_folder(&self.base);
    }
}

fn child_in(base: &Path, id: &str, state: ChildState) -> ChildRecord {
    let worktree = base.join("worktrees").join(id).to_string_lossy().into_owned();
    let record = ChildRecord::make_with(
        id,
        "parent",
        &worktree,
        "main",
        COMMIT,
        "acceptEdits",
        &format!("key-{id}"),
        ChildRecordOptions { state: Some(state), ..Default::default() },
    );
    ChildRecord { run_id: Some(format!("r-{id}")), awaited_run_id: Some(format!("r-{id}")), ..record }
}

impl Fixture {
    fn new(parent: Option<DelegationPaneActivity>, made: Option<&dyn Fn(&Path) -> DelegationFile>) -> Fixture {
        let base = temp("delivery");
        let store = DelegationFileStore::new(base.join("profile"));
        let file = match made {
            Some(made) => made(&base),
            None => DelegationFile::of(
                ["c1", "c2", "c3"].iter().map(|id| child_in(&base, id, ChildState::Running)).collect(),
                vec![],
                vec![],
                vec![],
                vec![],
            ),
        };
        store.save(&file).unwrap();
        let host = TestHost::new(Some(base.join("profile")), "run");
        if let Some(activity) = parent {
            host.set_claude("parent", activity, (activity != Idle).then_some("p-run"));
        }
        let coordinator = launch(&base, host.clone());
        Fixture { base, host, coordinator }
    }

    fn store(&self) -> DelegationFileStore {
        DelegationFileStore::new(self.base.join("profile"))
    }

    fn loaded(&self) -> DelegationFile {
        self.store().load().unwrap()
    }

    /// The app quits and opens again from the same profile, its runs named after the launch.
    async fn relaunch(&mut self, name: &str) {
        self.host = TestHost::new(Some(self.base.join("profile")), &format!("{name}-run"));
        self.coordinator = launch(&self.base, self.host.clone());
        self.coordinator.interrupt_runs_killed_by_quit().await;
    }

    /// The child's run "r-<id>" finished: its notice, as it was made.
    async fn end(&self, id: &str) -> Notice {
        self.coordinator
            .child_run_ended(id, &format!("r-{id}"), ChildRunEnd::Finished)
            .await
            .unwrap_or_else(|| panic!("no notice for {id}"))
    }

    fn saved(&self, notice: &Notice) -> Option<Notice> {
        self.loaded().notices.into_iter().find(|saved| saved.id == notice.id)
    }

    async fn follow_up(&self, child: &str, text: &str) -> DelegationResponse {
        self.coordinator
            .handle_call(request("follow_up", &[("child", child), ("text", text)]), caller("parent", "claude", "claude", true))
            .await
    }

    fn text_of(&self, follow_up_id: &str) -> String {
        let follow_up = self.coordinator.file().follow_ups.into_iter().find(|f| f.id == follow_up_id).unwrap();
        follow_up_text(&follow_up, &report_file_in(&self.base.join("worktrees").join("c1").to_string_lossy()), "claude")
    }

    /// Every one of `ids` reached a pane in exactly one input.
    fn each_taken_once(&self, ids: &[&str]) {
        for id in ids {
            assert_eq!(self.host.taken().iter().filter(|h| h.input.contains(id)).count(), 1, "{id} reached a pane exactly once");
        }
    }

    fn held_ids(&self, pane: &str) -> Vec<String> {
        self.coordinator.held_items(pane).into_iter().map(|i| i.id).collect()
    }
}

fn launch(base: &Path, host: Arc<TestHost>) -> Coordinator {
    let git = Git::new(support::place::processes(), base.join("profile").join("git-no-hooks"));
    let maker = Maker::new(&base.join("worktrees").to_string_lossy(), git, Arc::new(FixedDiskSpace(50_000_000_000)));
    support::host::coordinator(DelegationFileStore::new(base.join("profile")), host, maker, || true).unwrap()
}

/// What `DelegationPanes.PaneState` gives for the parent pane: its stored mode, its folder and run, in `activity` (idle when the app
/// would refuse a run).
fn parent_pane(base: &Path, activity: DelegationPaneActivity) -> DelegationPaneState {
    DelegationPaneState {
        session_id: "parent".into(),
        permission_mode: "auto".into(),
        folder: base.to_string_lossy().into_owned(),
        kind: "claude".into(),
        provider: "claude".into(),
        parent_session_id: None,
        run_id: Some("p-run".into()),
        activity,
    }
}

/// "delegation: notices are steered, start the next run, or are held"
#[tokio::test]
async fn notices_are_steered_start_the_next_run_or_are_held() {
    let _lane = lane::enter().await;
    let pane = |activity, kind: &str, provider: &str| DelegationPaneState {
        session_id: "p".into(),
        permission_mode: "auto".into(),
        folder: std::env::temp_dir().to_string_lossy().into_owned(),
        kind: kind.into(),
        provider: provider.into(),
        parent_session_id: None,
        run_id: Some("p-run".into()),
        activity,
    };
    assert!(
        automatic(Some(&pane(Running, "claude", "claude"))) == AutomaticDelivery::Route(DeliveryRoute::Steer)
            && automatic(Some(&pane(Finished, "claude", "claude"))) == AutomaticDelivery::Route(DeliveryRoute::Queue)
            && automatic(Some(&pane(Idle, "claude", "claude"))) == AutomaticDelivery::Hold
            && automatic(Some(&pane(Running, "claude", "codex"))) == AutomaticDelivery::Later
            && automatic(Some(&pane(Running, "claude", "gemini"))) == AutomaticDelivery::Later
            && automatic(Some(&pane(Running, "terminal", "claude"))) == AutomaticDelivery::Hold
            && automatic(None) == AutomaticDelivery::Hold,
        "the automatic delivery"
    );
    let sample = Notice::new("n-1", "c-1", 2, NoticeKind::Reported, DeliveryLane::Pending);
    assert_eq!(
        notice_text(&sample),
        "[Mighty Code notice n-1] Your child c-1 reported revision 2. Read the report with child_status; merge it with merge when it is right."
    );
    assert_eq!(
        notice_text(&Notice { kind: NoticeKind::EndedWithoutReport, ..sample.clone() }),
        "[Mighty Code notice n-1] Your child c-1's run ended without a new report; its report revision is still 2. Call child_status to see where it stands."
    );
    assert_eq!(
        notice_text(&Notice { kind: NoticeKind::FailedToStart, ..sample }),
        "[Mighty Code notice n-1] Your child c-1 could not be started. Only a human can discard it, from its card."
    );
    assert_eq!(
        follow_up_text(
            &FollowUp::new("f-1", "c-1", "Also add tests.\n", DeliveryLane::Pending),
            r"C:\w\c-1\.mighty-delegation\REPORT.md",
            "claude"
        ),
        "[Mighty Code follow-up f-1] Your parent sent you this follow-up:\n\nAlso add tests.\n\n\nWhen you are done, commit your work on your branch and write your updated report to C:\\w\\c-1\\.mighty-delegation\\REPORT.md."
    );
    let time = now();
    let digits = |range: std::ops::Range<usize>| time[range].bytes().all(|b| b.is_ascii_digit());
    assert!(
        time.len() == 20
            && digits(0..4)
            && &time[4..5] == "-"
            && digits(5..7)
            && &time[7..8] == "-"
            && digits(8..10)
            && &time[10..11] == "T"
            && digits(11..13)
            && digits(14..16)
            && digits(17..19)
            && time.ends_with('Z'),
        "a receipt's time is ISO 8601 to the second: {time}"
    );

    // A notice is steered into the parent's running Claude run once, and saved delivered before the run gets it.
    let fixture = Fixture::new(Some(Running), None);
    let notice = fixture.end("c1").await;
    let handed = fixture.host.all_handed();
    assert!(
        handed.len() == 1
            && handed[0].session_id == "parent"
            && handed[0].route == Some(DeliveryRoute::Steer)
            && handed[0].run_id.as_deref() == Some("p-run")
            && handed[0].input == notice_text(&notice)
            && handed[0].input.contains("child_status"),
        "steered into the running run"
    );
    let written = handed[0].on_disk.as_ref().and_then(|f| f.notices.iter().find(|n| n.id == notice.id).cloned()).unwrap();
    assert!(
        written.lane == DeliveryLane::Delivered
            && written.receipt.as_ref().is_some_and(|r| r.route == DeliveryRoute::Steer && r.run_id == "p-run"),
        "on disk before the run got it"
    );
    let saved = fixture.saved(&notice).unwrap();
    assert!(
        saved.lane == DeliveryLane::Delivered
            && saved.receipt.as_ref().is_some_and(|r| r.route == DeliveryRoute::Steer && r.run_id == "p-run" && !r.time.is_empty())
            && fixture.coordinator.file() == fixture.loaded(),
        "saved delivered with its receipt"
    );
    assert!(fixture.coordinator.child_run_ended("c1", "r-c1", ChildRunEnd::Finished).await.is_none(), "the same end again sends nothing");
    fixture.coordinator.deliver_pending().await;
    assert!(
        fixture.coordinator.send("Go on.", "parent", None).await == DelegationRelease::NothingHeld && fixture.host.all_handed().len() == 1,
        "never twice"
    );
    drop(fixture);

    // A notice starts the next run of a parent idle after a normal finish, and the next is steered into it.
    let fixture = Fixture::new(Some(Finished), None);
    let first = fixture.end("c1").await;
    let second = fixture.end("c2").await;
    let handed = fixture.host.all_handed();
    assert!(
        handed.iter().map(|h| h.route).eq([Some(DeliveryRoute::Queue), Some(DeliveryRoute::Steer)])
            && handed.iter().map(|h| h.run_id.as_deref()).eq([Some("run-1"), Some("run-1")])
            && handed.iter().map(|h| h.input.clone()).eq([notice_text(&first), notice_text(&second)]),
        "the first starts the next run and the second is steered into it"
    );
    assert!(
        handed[0]
            .on_disk
            .as_ref()
            .and_then(|f| f.notices.iter().find(|n| n.id == first.id).cloned())
            .is_some_and(|n| n.lane == DeliveryLane::Delivered && n.receipt.map(|r| r.route) == Some(DeliveryRoute::Queue))
            && fixture
                .saved(&first)
                .is_some_and(|n| n.lane == DeliveryLane::Delivered && n.receipt.map(|r| r.run_id) == Some("run-1".into()))
            && fixture.saved(&second).and_then(|n| n.receipt).is_some_and(|r| r.route == DeliveryRoute::Steer && r.run_id == "run-1"),
        "each receipt names the run that got it"
    );
    fixture.each_taken_once(&[&first.id, &second.id]);
    drop(fixture);

    // A pending notice for a parent that finished normally but cannot start a run now is held, and no run is asked for.
    let fixture = Fixture::new(Some(Running), None);
    fixture.host.refuse_next(&[Some(Running), Some(Running)]);
    let first = fixture.end("c1").await;
    assert_eq!(fixture.saved(&first), Some(first.clone()), "a pane that takes nothing twice leaves the notice pending");
    let asked = fixture.host.all_handed().len();
    fixture.host.set(parent_pane(&fixture.base, Idle));
    fixture.coordinator.deliver_pending().await;
    let second = fixture.end("c2").await;
    assert!(
        fixture.host.all_handed().len() == asked
            && fixture.loaded().notices == [held(&first), held(&second)]
            && fixture.held_ids("parent") == [first.id.clone(), second.id.clone()],
        "both are held, and no run is asked for"
    );
    fixture.host.set(parent_pane(&fixture.base, Finished));
    assert_eq!(
        fixture.coordinator.send("Go on.", "parent", None).await,
        released("run-1", &[&first.id, &second.id]),
        "the next send releases both"
    );
    fixture.each_taken_once(&[&first.id, &second.id]);
    drop(fixture);

    // A new item for a pane with held items is held behind them, whatever the pane is doing.
    let fixture = Fixture::new(Some(Finished), None);
    fixture.host.set(parent_pane(&fixture.base, Idle));
    let first = fixture.end("c1").await;
    assert_eq!(fixture.saved(&first), Some(held(&first)), "held while the app would refuse a run");
    fixture.host.set(parent_pane(&fixture.base, Finished));
    let second = fixture.end("c2").await;
    fixture.host.set_claude("parent", Running, Some("p-run-2"));
    let third = fixture.end("c3").await;
    fixture.coordinator.deliver_pending().await;
    assert!(
        fixture.host.all_handed().is_empty()
            && fixture.loaded().notices == [held(&first), held(&second), held(&third)]
            && fixture.held_ids("parent") == [first.id.clone(), second.id.clone(), third.id.clone()],
        "newer notices are held behind the first"
    );
    fixture.host.set_claude("parent", Finished, Some("p-run-2"));
    assert_eq!(
        fixture.coordinator.send("Go on.", "parent", None).await,
        released("run-1", &[&first.id, &second.id, &third.id]),
        "one send releases all three"
    );
    let run = fixture.host.all_handed();
    assert!(
        run.len() == 1
            && run[0].route.is_none()
            && run[0].session_id == "parent"
            && run[0].input == texts(&[&first, &second, &third], &["Go on."])
            && fixture.loaded().notices.iter().all(|n| n.receipt.as_ref().map(|r| r.run_id.as_str()) == Some("run-1")),
        "oldest first, ahead of the human's text, in one run"
    );
    fixture.each_taken_once(&[&first.id, &second.id, &third.id]);
}

/// "delegation: held items are released by a send or run next, once"
#[tokio::test]
async fn held_items_are_released_by_a_send_or_run_next_once() {
    let _lane = lane::enter().await;
    let fixture = Fixture::new(Some(Idle), None);
    let first = fixture.end("c1").await;
    let second = fixture.end("c2").await;
    assert!(fixture.host.all_handed().is_empty() && fixture.loaded().notices == [held(&first), held(&second)], "both are held");
    assert!(
        fixture.held_ids("parent") == [first.id.clone(), second.id.clone()]
            && fixture.coordinator.held_items("parent").iter().all(|i| i.kind == DelegationDeliveryKind::Notice),
        "they are the parent's held rows"
    );
    let words = "이제 합쳐 줘.";
    assert_eq!(
        fixture.coordinator.send(words, "parent", None).await,
        released("run-1", &[&first.id, &second.id]),
        "the send releases both"
    );
    let run = fixture.host.all_handed();
    assert!(
        run.len() == 1
            && run[0].route.is_none()
            && run[0].session_id == "parent"
            && run[0].input == texts(&[&first, &second], &[words])
            && run[0].on_disk.as_ref().unwrap().notices.iter().all(|n| n.lane == DeliveryLane::Delivered),
        "both saved delivered before the run got them"
    );
    assert!(
        fixture.loaded().notices.iter().all(|n| n.lane == DeliveryLane::Delivered
            && n.receipt.as_ref().is_some_and(|r| r.route == DeliveryRoute::Queue && r.run_id == "run-1"))
            && fixture.coordinator.held_items("parent").is_empty(),
        "their receipts name the run"
    );
    assert!(
        fixture.coordinator.send("Thanks.", "parent", None).await == DelegationRelease::NothingHeld
            && fixture.coordinator.run_next("parent").await == DelegationRelease::NothingHeld
            && fixture.host.all_handed().len() == 1,
        "nothing more is held"
    );
    fixture.each_taken_once(&[&first.id, &second.id]);
    drop(fixture);

    // Run next releases only the held items in one run, and the human's queued rows follow in later runs.
    let fixture = Fixture::new(Some(Idle), None);
    let first = fixture.end("c1").await;
    let second = fixture.end("c2").await;
    let mut rows = vec!["Human row 1", "Human row 2"].into_iter();
    for _ in 0..3 {
        if fixture.coordinator.run_next("parent").await == DelegationRelease::NothingHeld {
            use mighty_delegation::host::DelegationHost as _;
            fixture.host.start_run("parent".into(), rows.next().unwrap().into()).await;
        }
        let run = fixture.host.pane("parent").and_then(|p| p.run_id);
        fixture.host.set_claude("parent", Finished, run.as_deref());
    }
    assert_eq!(
        fixture.host.all_handed().iter().map(|h| h.input.clone()).collect::<Vec<_>>(),
        [texts(&[&first, &second], &[]), "Human row 1".into(), "Human row 2".into()],
        "held items first, then the human's rows"
    );
    assert!(
        fixture.host.all_handed().iter().map(|h| h.run_id.as_deref()).eq([Some("run-1"), Some("run-2"), Some("run-3")])
            && fixture.loaded().notices.iter().all(|n| n.receipt.as_ref().map(|r| r.run_id.as_str()) == Some("run-1")),
        "each in its own run"
    );
    fixture.each_taken_once(&[&first.id, &second.id]);
    drop(fixture);

    // Two sends at once release each held item once.
    let fixture = Fixture::new(Some(Idle), None);
    let first = fixture.end("c1").await;
    let second = fixture.end("c2").await;
    let (a, b) = tokio::join!(
        fixture.coordinator.send("From the desktop.", "parent", None),
        fixture.coordinator.send("From the phone.", "parent", None)
    );
    let results = [a, b];
    assert!(
        results.iter().filter(|r| **r == DelegationRelease::NothingHeld).count() == 1
            && results.contains(&released("run-1", &[&first.id, &second.id]))
            && fixture.host.all_handed().len() == 1,
        "one send releases them and the other finds nothing held"
    );
    fixture.each_taken_once(&[&first.id, &second.id]);
    drop(fixture);

    // The app's send starts the release run itself, and run next steers held items into a running Claude run.
    let fixture = Fixture::new(Some(Idle), None);
    let first = fixture.end("c1").await;
    let second = fixture.end("c2").await;
    let started = Arc::new(Mutex::new(vec![]));
    let into = started.clone();
    let starter: RunStarter = Box::new(move |input| {
        into.lock().unwrap().push(input);
        Box::pin(async { Some("app-run-1".to_string()) })
    });
    assert_eq!(
        fixture.coordinator.send("Merge what is ready.", "parent", Some(starter)).await,
        released("app-run-1", &[&first.id, &second.id]),
        "the app's starter starts the release run"
    );
    assert!(
        *started.lock().unwrap() == [texts(&[&first, &second], &["Merge what is ready."])]
            && fixture.host.all_handed().is_empty()
            && fixture.loaded().notices.iter().all(|n| n.lane == DeliveryLane::Delivered
                && n.receipt.as_ref().is_some_and(|r| r.route == DeliveryRoute::Queue && r.run_id == "app-run-1")),
        "the whole input reaches the starter once"
    );
    let third = fixture.end("c3").await;
    let nothing: RunStarter = Box::new(|_| Box::pin(async { None }));
    assert!(
        fixture.coordinator.send("Again.", "parent", Some(nothing)).await == DelegationRelease::NotStarted
            && fixture.saved(&third) == Some(held(&third))
            && fixture.held_ids("parent") == [third.id.clone()],
        "a starter that starts nothing leaves them held"
    );
    fixture.host.set_claude("parent", Running, Some("p-run"));
    assert_eq!(fixture.coordinator.run_next("parent").await, released("p-run", &[&third.id]), "run next steers into the running run");
    let steer = fixture.host.all_handed();
    assert!(
        steer.len() == 1
            && steer[0].route == Some(DeliveryRoute::Steer)
            && steer[0].run_id.as_deref() == Some("p-run")
            && steer[0].input == notice_text(&third)
            && steer[0].on_disk.as_ref().and_then(|f| f.notices.iter().find(|n| n.id == third.id).map(|n| n.lane))
                == Some(DeliveryLane::Delivered)
            && fixture
                .saved(&third)
                .is_some_and(|n| n.lane == DeliveryLane::Delivered
                    && n.receipt.is_some_and(|r| r.route == DeliveryRoute::Steer && r.run_id == "p-run"))
            && fixture.coordinator.run_next("parent").await == DelegationRelease::NothingHeld,
        "steered once, with its receipt"
    );
    fixture.each_taken_once(&[&third.id]);
    drop(fixture);

    // Held rows cannot be removed.
    let rows = [
        DelegationDeliveryItem {
            id: "n-1".into(),
            kind: DelegationDeliveryKind::Notice,
            child_id: "c1".into(),
            session_id: "parent".into(),
            text: "Notice 1".into(),
        },
        DelegationDeliveryItem {
            id: "n-2".into(),
            kind: DelegationDeliveryKind::Notice,
            child_id: "c2".into(),
            session_id: "parent".into(),
            text: "Notice 2".into(),
        },
    ];
    assert!(
        remove_refusal("n-2", &rows) == Some(DelegationReasonCode::HeldNotRemovable)
            && remove_refusal("q-1", &rows).is_none()
            && remove_refusal("n-2", &[]).is_none(),
        "a held row cannot be removed"
    );
}

/// "delegation: pending items are offered again and never overtaken"
#[tokio::test]
async fn pending_items_are_offered_again_and_never_overtaken() {
    let _lane = lane::enter().await;
    let fixture = Fixture::new(Some(Running), None);
    fixture.host.refuse_next(&[Some(Finished)]);
    let first = fixture.end("c1").await;
    assert!(
        fixture.host.all_handed().iter().map(|h| h.route).eq([Some(DeliveryRoute::Steer), Some(DeliveryRoute::Queue)])
            && fixture.host.all_handed().iter().map(|h| h.run_id.clone()).eq([None, Some("run-1".to_string())])
            && fixture
                .saved(&first)
                .is_some_and(|n| n.lane == DeliveryLane::Delivered
                    && n.receipt.is_some_and(|r| r.route == DeliveryRoute::Queue && r.run_id == "run-1")),
        "a run that ended gets the notice as its next run"
    );
    fixture.host.refuse_next(&[Some(Running), Some(Running)]);
    let second = fixture.end("c2").await;
    assert!(fixture.host.all_handed().len() == 4 && fixture.saved(&second) == Some(second.clone()), "offered twice, then left pending");
    let third = fixture.end("c3").await;
    let saved = fixture.loaded().notices;
    assert!(
        saved.iter().all(|n| n.lane == DeliveryLane::Delivered)
            && saved.iter().map(|n| n.receipt.as_ref().map(|r| r.route)).eq([
                Some(DeliveryRoute::Queue),
                Some(DeliveryRoute::Steer),
                Some(DeliveryRoute::Steer)
            ]),
        "offered again with the next notice"
    );
    fixture.each_taken_once(&[&first.id, &second.id, &third.id]);
    drop(fixture);

    // A notice a run could not take while starting or ending goes on when the app offers it again.
    let fixture = Fixture::new(Some(Running), None);
    fixture.host.refuse_next(&[Some(Running), Some(Running)]);
    let first = fixture.end("c1").await;
    assert!(fixture.saved(&first) == Some(first.clone()) && fixture.coordinator.held_items("parent").is_empty(), "still pending, not held");
    assert_eq!(fixture.coordinator.pending_work(), 1, "a pending notice counts as pending work");
    fixture.coordinator.deliver_pending().await;
    assert_eq!(fixture.coordinator.pending_work(), 0, "a delivered notice is no pending work");
    let last = fixture.host.taken().pop().unwrap();
    assert!(
        last.route == Some(DeliveryRoute::Steer)
            && last.run_id.as_deref() == Some("p-run")
            && last.input == notice_text(&first)
            && fixture
                .saved(&first)
                .is_some_and(|n| n.lane == DeliveryLane::Delivered && n.receipt.map(|r| r.route) == Some(DeliveryRoute::Steer)),
        "steered once the run is up"
    );
    fixture.host.refuse_next(&[Some(Finished), Some(Finished)]);
    let second = fixture.end("c2").await;
    assert_eq!(fixture.saved(&second), Some(second.clone()), "left pending by a run that was ending");
    fixture.coordinator.deliver_pending().await;
    let next = fixture.host.taken().pop().unwrap();
    assert!(
        next.route == Some(DeliveryRoute::Queue)
            && next.run_id.as_deref() == Some("run-1")
            && next.input == notice_text(&second)
            && fixture.saved(&second).and_then(|n| n.receipt).map(|r| r.run_id) == Some("run-1".into()),
        "it goes on as the next run"
    );
    fixture.each_taken_once(&[&first.id, &second.id]);
    drop(fixture);

    // An item its pane still did not take keeps the newer ones waiting behind it for the pass.
    let fixture = Fixture::new(Some(Running), None);
    fixture.host.refuse_next(&[Some(Running), Some(Running)]);
    let first = fixture.end("c1").await;
    fixture.host.refuse_next(&[Some(Running), Some(Running)]);
    let second = fixture.end("c2").await;
    assert!(
        fixture.saved(&first) == Some(first.clone())
            && fixture.saved(&second) == Some(second.clone())
            && fixture.host.all_handed().len() == 4
            && fixture.host.taken().is_empty(),
        "the newer notice waits behind the first"
    );
    fixture.coordinator.deliver_pending().await;
    assert_eq!(
        fixture.host.taken().iter().map(|h| h.input.clone()).collect::<Vec<_>>(),
        [notice_text(&first), notice_text(&second)],
        "the next pass hands both over, oldest first"
    );
    fixture.each_taken_once(&[&first.id, &second.id]);
    drop(fixture);

    // An item waiting for its pane keeps no other pane's items waiting.
    let fixture = Fixture::new(Some(Running), None);
    fixture.host.set_claude("c1", Running, Some("r-c1"));
    fixture.host.refuse_next(&[Some(Running), Some(Running)]);
    let notice = fixture.end("c2").await;
    fixture.host.refuse_next(&[Some(Running), Some(Running)]);
    let answer = fixture.follow_up("c1", "Go on.").await;
    let taken = fixture.host.taken();
    assert!(
        answer.error.is_none()
            && fixture.saved(&notice) == Some(notice.clone())
            && taken.iter().map(|h| h.session_id.as_str()).eq(["c1"])
            && taken[0].route == Some(DeliveryRoute::Steer),
        "the follow-up goes to its own pane"
    );
    fixture.coordinator.deliver_pending().await;
    assert_eq!(
        fixture.host.taken().iter().map(|h| h.session_id.clone()).collect::<Vec<_>>(),
        ["c1", "parent"],
        "the parent's notice goes on in the next pass"
    );
    fixture.each_taken_once(&[&notice.id]);
}

/// "delegation: launch holds what was pending, and a closed parent's notices stay on the cards"
#[tokio::test]
async fn launch_holds_what_was_pending_and_a_closed_parents_notices_stay_on_the_cards() {
    let _lane = lane::enter().await;
    let receipt = DeliveryReceipt::new("2026-10-09T01:00:00Z", DeliveryRoute::Steer, "old-run");
    let old = [
        Notice::new("n-old", "c1", 0, NoticeKind::EndedWithoutReport, DeliveryLane::Pending),
        Notice::new("n-done", "c1", 0, NoticeKind::FailedToStart, DeliveryLane::Delivered).with_receipt(receipt.clone()),
    ];
    let made = {
        let old = old.clone();
        move |base: &Path| {
            DelegationFile::of(
                vec![child_in(base, "c1", ChildState::Ended), child_in(base, "c2", ChildState::Running)],
                old.to_vec(),
                vec![FollowUp::new("f-old", "c1", "테스트도 추가해 줘", DeliveryLane::Pending)],
                vec![],
                vec![],
            )
        }
    };
    let fixture = Fixture::new(None, Some(&made));
    let host = TestHost::new(Some(fixture.base.join("profile")), "run");
    host.set_claude("parent", Finished, Some("p-run"));
    host.set_claude("c1", Finished, Some("r-c1"));
    let coordinator = launch(&fixture.base, host.clone());
    let restored = fixture.loaded();
    assert!(
        host.all_handed().is_empty()
            && restored.notices.iter().map(|n| n.lane).eq([DeliveryLane::Held, DeliveryLane::Delivered])
            && restored.notices[1].receipt == Some(receipt)
            && restored.follow_ups[0].lane == DeliveryLane::Held
            && coordinator.file() == restored,
        "what was pending at launch is held, on disk too"
    );
    assert!(
        coordinator.held_items("parent").iter().map(|i| i.id.as_str()).eq(["n-old"])
            && coordinator.held_items("c1").iter().map(|i| i.id.as_str()).eq(["f-old"]),
        "each pane has its own held rows"
    );
    assert_eq!(coordinator.pending_work(), 0, "held items are no pending work");
    let fresh = coordinator.child_run_ended("c2", "r-c2", ChildRunEnd::Finished).await.unwrap();
    assert!(
        host.all_handed().is_empty()
            && coordinator.held_items("parent").iter().map(|i| i.id.clone()).eq(["n-old".to_string(), fresh.id.clone()]),
        "a new notice is held behind what launch held"
    );
    assert_eq!(coordinator.send("Go on.", "parent", None).await, released("run-1", &["n-old", &fresh.id]));
    assert!(
        host.all_handed().last().unwrap().input == texts(&[&old[0], &fresh], &["Go on."])
            && !host.all_handed().iter().any(|h| h.input.contains("n-done")),
        "the next send releases both, and never what was delivered"
    );
    drop((coordinator, fixture));

    // Held notices for a closed parent's children stay on the child's card.
    let fixture = Fixture::new(Some(Idle), None);
    let first = fixture.end("c1").await;
    fixture.host.close("parent");
    let second = fixture.end("c2").await;
    assert!(
        fixture.host.all_handed().is_empty()
            && fixture.coordinator.held_notices("c1") == [held(&first)]
            && fixture.coordinator.held_notices("c2") == [held(&second)]
            && fixture.coordinator.held_notices("c3").is_empty(),
        "each child's card has its held notices"
    );
    assert!(fixture.coordinator.child_rows().borrow().iter().map(|r| r.held_notices).eq([1, 1, 0]), "the rows carry the count");
    assert!(
        fixture.coordinator.send("Hello?", "parent", None).await == DelegationRelease::NotStarted
            && fixture.coordinator.run_next("parent").await == DelegationRelease::NotStarted
            && fixture.host.all_handed().is_empty()
            && fixture.loaded().notices == [held(&first), held(&second)],
        "nothing releases a closed parent's notices"
    );
    let relaunched = launch(&fixture.base, fixture.host.clone());
    assert!(
        relaunched.held_notices("c1") == [held(&first)]
            && relaunched.held_notices("c2") == [held(&second)]
            && relaunched.child_rows().borrow().iter().map(|r| r.held_notices).eq([1, 1, 0])
            && fixture.host.all_handed().is_empty(),
        "across a relaunch they stay on the cards"
    );
    drop((relaunched, fixture));

    // The held rows the app shows follow the file across a relaunch.
    let fixture = Fixture::new(Some(Idle), None);
    let mut rows = fixture.coordinator.held_rows();
    assert!(rows.borrow_and_update().is_empty(), "no held rows at first");
    let first = fixture.end("c1").await;
    let second = fixture.end("c2").await;
    let expected: Vec<DelegationDeliveryItem> = [&first, &second]
        .iter()
        .map(|n| DelegationDeliveryItem {
            id: n.id.clone(),
            kind: DelegationDeliveryKind::Notice,
            child_id: n.child_id.clone(),
            session_id: "parent".into(),
            text: notice_text(n),
        })
        .collect();
    assert!(rows.has_changed().unwrap(), "a change waits for the app");
    {
        let map = rows.borrow_and_update();
        assert!(map.keys().eq(["parent"]) && map["parent"] == expected, "only the newest map waits for the app");
    }
    assert!(!rows.has_changed().unwrap());
    assert_eq!(fixture.coordinator.held_items("parent"), expected, "the held items are the same rows");
    let relaunched = launch(&fixture.base, fixture.host.clone());
    let mut loaded = relaunched.held_rows();
    assert_eq!(loaded.borrow_and_update()["parent"], expected, "the first map after a relaunch has them");
    assert_eq!(relaunched.send("Go on.", "parent", None).await, released("run-1", &[&first.id, &second.id]));
    assert!(loaded.borrow_and_update().is_empty(), "released, they leave the queued list");
}

/// "delegation: follow-ups reach their child exactly once across launches"
#[tokio::test]
async fn follow_ups_reach_their_child_exactly_once_across_launches() {
    let _lane = lane::enter().await;
    let made = |base: &Path| {
        DelegationFile::of(
            vec![
                ChildRecord { awaited_run_id: None, ..child_in(base, "c1", ChildState::Running) },
                child_in(base, "c2", ChildState::Ended),
            ],
            vec![],
            vec![],
            vec![],
            vec![],
        )
    };
    let fixture = Fixture::new(Some(Running), Some(&made));
    fixture.host.set_claude("c1", Running, Some("r-c1"));
    fixture.host.set_claude("c2", Idle, Some("r-c2"));
    let to_running = fixture.follow_up("c1", "Also add tests.").await.follow_up.unwrap();
    let steer = fixture.host.all_handed();
    assert!(
        to_running.lane == DeliveryLane::Delivered
            && steer.len() == 1
            && steer[0].session_id == "c1"
            && steer[0].route == Some(DeliveryRoute::Steer)
            && steer[0].run_id.as_deref() == Some("r-c1")
            && steer[0].input.contains(&to_running.id)
            && steer[0].input.contains("Also add tests.")
            && steer[0].on_disk.as_ref().unwrap().follow_ups[0].lane == DeliveryLane::Delivered,
        "steered into the running child's run"
    );
    assert_eq!(
        fixture.coordinator.file().children[0].awaited_run_id.as_deref(),
        Some("r-c1"),
        "the run that got it is the one c1's parent now awaits"
    );
    let to_stopped = fixture.follow_up("c2", "Rebase on main.").await.follow_up.unwrap();
    assert!(
        to_stopped.lane == DeliveryLane::Held
            && fixture.host.all_handed().len() == 1
            && fixture.coordinator.held_items("c2")[0].kind == DelegationDeliveryKind::FollowUp,
        "held for a stopped child"
    );
    assert_eq!(
        fixture.coordinator.send("Thanks.", "c2", None).await,
        released("run-1", &[&to_stopped.id]),
        "the human's next send in the child releases it"
    );
    let follow_up = fixture.coordinator.file().follow_ups.into_iter().find(|f| f.id == to_stopped.id).unwrap();
    let run = fixture.host.all_handed().pop().unwrap();
    let report = report_file_in(&fixture.base.join("worktrees").join("c2").to_string_lossy());
    assert!(
        run.session_id == "c2"
            && run.input == format!("{}\n\nThanks.", follow_up_text(&follow_up, &report, "claude"))
            && follow_up.lane == DeliveryLane::Delivered
            && follow_up.receipt.as_ref().map(|r| r.run_id.as_str()) == Some("run-1")
            && fixture.coordinator.file().children[1].awaited_run_id.as_deref() == Some("run-1"),
        "ahead of the human's text, and its run is awaited"
    );
    fixture.each_taken_once(&[&to_running.id, &to_stopped.id]);
    drop(fixture);

    // The profile of the Mac's follow-up tests: the parent's child c1 reported at revision 1 on its run "r-c1".
    let reported = |base: &Path| {
        let child = ChildRecord {
            report_revision: 1,
            report_head: Some(COMMIT.into()),
            report_digest: Some("digest".into()),
            ..child_in(base, "c1", ChildState::Reported)
        };
        DelegationFile::of(vec![child], vec![], vec![], vec![], vec![])
    };

    // A follow-up to a reported child idle after a normal finish starts its next run once, and clears reported.
    let mut fixture = Fixture::new(None, Some(&reported));
    fixture.relaunch("launch1").await;
    fixture.host.set_claude("parent", Running, Some("p-run"));
    fixture.host.set_claude("c1", Finished, Some("r-c1"));
    let answer = fixture.follow_up("c1", "Also add tests.").await.follow_up.unwrap();
    let run = fixture.host.all_handed();
    assert!(
        answer.lane == DeliveryLane::Delivered
            && answer.remaining == 1
            && answer.child == "c1"
            && run.len() == 1
            && run[0].session_id == "c1"
            && run[0].route == Some(DeliveryRoute::Queue)
            && run[0].run_id.as_deref() == Some("launch1-run-1")
            && run[0].input == fixture.text_of(&answer.id)
            && run[0].on_disk.as_ref().and_then(|f| f.follow_ups.iter().find(|s| s.id == answer.id).map(|s| s.lane))
                == Some(DeliveryLane::Delivered),
        "it starts the child's next run, saved delivered first"
    );
    let child = fixture.coordinator.file().children[0].clone();
    assert!(
        child.state == ChildState::Running
            && child.run_id.as_deref() == Some("launch1-run-1")
            && child.awaited_run_id.as_deref() == Some("launch1-run-1")
            && child.report_revision == 1
            && child.follow_up_count == 1
            && fixture.coordinator.file() == fixture.loaded()
            && fixture.loaded().follow_ups[0]
                .receipt
                .as_ref()
                .is_some_and(|r| r.route == DeliveryRoute::Queue && r.run_id == "launch1-run-1"),
        "reported is cleared at once"
    );
    let listed = fixture.coordinator.handle_call(request("list_children", &[]), caller("parent", "claude", "claude", true)).await;
    assert_eq!(listed.children.unwrap()[0].state, ChildState::Running, "the parent sees it running");
    fixture.coordinator.deliver_pending().await;
    assert!(
        fixture.coordinator.send("Thanks.", "c1", None).await == DelegationRelease::NothingHeld
            && fixture.coordinator.run_next("c1").await == DelegationRelease::NothingHeld
            && fixture.host.taken().iter().filter(|h| h.input.contains(&answer.id)).count() == 1,
        "never twice"
    );
    let ended = fixture.coordinator.child_run_ended("c1", "launch1-run-1", ChildRunEnd::Finished).await.unwrap();
    assert!(
        ended.kind == NoticeKind::EndedWithoutReport
            && ended.report_revision == 1
            && fixture.coordinator.file().children[0].state == ChildState::Ended,
        "its end without a report sends one notice"
    );
    drop(fixture);

    // Held follow-ups reach the child once after a relaunch from the same profile, and clear reported then.
    let mut fixture = Fixture::new(None, Some(&reported));
    fixture.relaunch("launch1").await;
    let first_host = fixture.host.clone();
    fixture.host.set_claude("parent", Running, Some("p-run"));
    fixture.host.set_claude("c1", Idle, Some("r-c1"));
    let words = "그리고 문서도 고쳐 줘.";
    let first = fixture.follow_up("c1", "Also add tests.").await.follow_up.unwrap();
    let second = fixture.follow_up("c1", words).await.follow_up.unwrap();
    assert!(
        first.lane == DeliveryLane::Held
            && second.lane == DeliveryLane::Held
            && second.remaining == 0
            && fixture.host.all_handed().is_empty()
            && fixture.coordinator.file().children[0].state == ChildState::Reported
            && fixture.held_ids("c1") == [first.id.clone(), second.id.clone()],
        "held, so c1 has not run them: it is still reported"
    );
    let follow_texts = [fixture.text_of(&first.id), fixture.text_of(&second.id)];
    fixture.relaunch("launch2").await;
    let second_host = fixture.host.clone();
    fixture.host.set_claude("parent", Running, Some("p-run-2"));
    assert!(
        fixture.coordinator.file() == fixture.loaded()
            && fixture.coordinator.file().follow_ups.iter().all(|f| f.lane == DeliveryLane::Held)
            && fixture.held_ids("c1") == [first.id.clone(), second.id.clone()]
            && fixture.coordinator.held_items("c1").iter().all(|i| i.kind == DelegationDeliveryKind::FollowUp),
        "they are still held after the relaunch"
    );
    fixture.host.set_claude("c1", Finished, Some("launch2-run-0"));
    fixture.coordinator.deliver_pending().await;
    assert!(
        fixture.host.all_handed().is_empty() && fixture.coordinator.file().children[0].state == ChildState::Reported,
        "nothing reaches c1 on its own"
    );
    let before = fixture.loaded();
    assert!(
        fixture.follow_up("c1", "One more.").await.refused == Some(DelegationReasonCode::FollowUpLimit) && fixture.loaded() == before,
        "a third follow-up is refused"
    );
    fixture.host.set_claude("c1", Idle, Some("launch2-run-0"));
    assert_eq!(
        fixture.coordinator.send("Go on.", "c1", None).await,
        released("launch2-run-1", &[&first.id, &second.id]),
        "the send in c1 releases both"
    );
    let run = fixture.host.all_handed();
    assert!(
        run.len() == 1
            && run[0].session_id == "c1"
            && run[0].route.is_none()
            && run[0].input == format!("{}\n\n{}\n\nGo on.", follow_texts[0], follow_texts[1])
            && run[0].on_disk.as_ref().unwrap().follow_ups.iter().all(|f| f.lane == DeliveryLane::Delivered),
        "oldest first, ahead of the text"
    );
    let saved = fixture.loaded();
    let child = fixture.coordinator.file().children[0].clone();
    assert!(
        child.state == ChildState::Running
            && child.run_id.as_deref() == Some("launch2-run-1")
            && child.awaited_run_id.as_deref() == Some("launch2-run-1")
            && saved
                .follow_ups
                .iter()
                .all(|f| f.lane == DeliveryLane::Delivered && f.receipt.as_ref().map(|r| r.run_id.as_str()) == Some("launch2-run-1")),
        "c1 runs them now"
    );
    fixture.relaunch("launch3").await;
    fixture.host.set_claude("c1", Finished, Some("launch3-run-0"));
    fixture.coordinator.deliver_pending().await;
    assert!(
        fixture.coordinator.held_items("c1").is_empty()
            && fixture.coordinator.send("Hello?", "c1", None).await == DelegationRelease::NothingHeld
            && fixture.coordinator.run_next("c1").await == DelegationRelease::NothingHeld
            && fixture.host.all_handed().is_empty()
            && fixture.loaded().follow_ups == saved.follow_ups
            && fixture.coordinator.file().children[0].state == ChildState::Interrupted,
        "delivered, so nothing goes again"
    );
    for id in [&first.id, &second.id] {
        let count =
            [&first_host, &second_host, &fixture.host].iter().flat_map(|h| h.taken()).filter(|h| h.input.contains(id.as_str())).count();
        assert_eq!(count, 1, "{id} reached c1 exactly once across three launches");
    }
    drop(fixture);

    // A follow-up left pending at quit is held at launch, and run next hands it over once.
    let mut fixture = Fixture::new(None, Some(&reported));
    fixture.relaunch("launch1").await;
    let first_host = fixture.host.clone();
    fixture.host.set_claude("parent", Running, Some("p-run"));
    fixture.host.set_claude("c1", Finished, Some("r-c1"));
    fixture.host.refuse_next(&[None, None]);
    let answer = fixture.follow_up("c1", "Also add tests.").await.follow_up.unwrap();
    assert!(
        answer.lane == DeliveryLane::Pending
            && fixture.host.all_handed().len() == 2
            && fixture.host.taken().is_empty()
            && fixture.loaded().follow_ups[0].lane == DeliveryLane::Pending
            && fixture.coordinator.file().children[0].state == ChildState::Reported,
        "it stays pending"
    );
    fixture.relaunch("launch2").await;
    assert!(
        fixture.loaded().follow_ups[0].lane == DeliveryLane::Held
            && fixture.held_ids("c1") == [answer.id.clone()]
            && fixture.host.all_handed().is_empty(),
        "held at the next launch"
    );
    fixture.host.set_claude("c1", Idle, Some("r-c1"));
    assert_eq!(fixture.coordinator.run_next("c1").await, released("launch2-run-1", &[&answer.id]));
    let child = fixture.coordinator.file().children[0].clone();
    assert!(
        fixture.host.all_handed().iter().map(|h| h.input.clone()).eq([fixture.text_of(&answer.id)])
            && child.state == ChildState::Running
            && child.run_id.as_deref() == Some("launch2-run-1")
            && child.awaited_run_id.as_deref() == Some("launch2-run-1")
            && fixture.coordinator.run_next("c1").await == DelegationRelease::NothingHeld,
        "run next hands it over alone, once"
    );
    assert_eq!(
        first_host.taken().iter().chain(fixture.host.taken().iter()).filter(|h| h.input.contains(&answer.id)).count(),
        1,
        "exactly once across the launches"
    );
    drop(fixture);

    // A follow-up saved delivered just before the app quit is never handed over again.
    let mut fixture = Fixture::new(None, Some(&reported));
    let mut file = fixture.loaded();
    file.children[0].follow_up_count = 1;
    file.follow_ups =
        vec![FollowUp::new("f-written", "c1", "Also add tests.", DeliveryLane::Delivered).with_receipt(DeliveryReceipt::new(
            "2026-10-09T01:00:00Z",
            DeliveryRoute::Queue,
            "",
        ))];
    fixture.store().save(&file).unwrap();
    fixture.relaunch("launch1").await;
    fixture.host.set_claude("c1", Finished, Some("r-c1"));
    fixture.coordinator.deliver_pending().await;
    assert!(
        fixture.coordinator.held_items("c1").is_empty()
            && fixture.coordinator.send("Go on.", "c1", None).await == DelegationRelease::NothingHeld
            && fixture.coordinator.run_next("c1").await == DelegationRelease::NothingHeld
            && fixture.host.all_handed().is_empty()
            && fixture.loaded() == file,
        "saved delivered is never handed over again"
    );
    drop(fixture);

    // Two held follow-ups, each at the most a follow-up may take, fit the one run that run next releases them in.
    let mut fixture = Fixture::new(None, Some(&reported));
    fixture.relaunch("launch1").await;
    fixture.host.set_claude("parent", Running, Some("p-run"));
    fixture.host.set_claude("c1", Idle, Some("r-c1"));
    let longest = "x".repeat(MAXIMUM_FOLLOW_UP_BYTES);
    let first = fixture.follow_up("c1", &longest).await.follow_up.unwrap();
    let second = fixture.follow_up("c1", &longest).await.follow_up.unwrap();
    assert!(
        first.lane == DeliveryLane::Held
            && second.lane == DeliveryLane::Held
            && second.remaining == 0
            && fixture.host.all_handed().is_empty(),
        "both are held"
    );
    assert_eq!(fixture.coordinator.run_next("c1").await, released("launch1-run-1", &[&first.id, &second.id]), "run next releases both");
    let run = fixture.host.all_handed();
    assert!(
        run.len() == 1
            && run[0].session_id == "c1"
            && run[0].route.is_none()
            && run[0].input == format!("{}\n\n{}", fixture.text_of(&first.id), fixture.text_of(&second.id))
            && fixture.coordinator.held_items("c1").is_empty(),
        "oldest first, in one run"
    );
    // The app's start takes that run's input: at most 100,000 characters (`StartRunRequest.Validate`, the queue's buffer).
    assert!(run[0].input.encode_utf16().count() <= 100_000, "the run's input fits the app's start");
}
