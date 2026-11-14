//! The desktop service's delegation over the real run path (app.rs through support/launch.rs), C#'s DelegationAppVerification checks
//! under their names: "delegation: a card's merge is queued while the child works or the parent runs, then goes through by itself"
//! (L676). Windows only while the transport is (W7 plan D4).
#![cfg(windows)]

mod support;

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mighty_delegation::app::DelegationWindow;
use mighty_delegation::app_host::ChildRunEnd;
use mighty_delegation::cards::{DelegationCardResult, DelegationMergeWait};
use mighty_delegation::model::{ChildRecord, ChildState};
use mighty_delegation::store::{DelegationFile, DelegationFileStore};
use support::launch::{DelegationLaunch, PARENT, awaited};
use support::{host, lane, place::DelegationPlace};

/// The window's part that hears which queued merges went through by themselves (C# `QueuedMergeDone`).
#[derive(Default)]
struct Done(Mutex<Vec<String>>);

impl DelegationWindow for Done {
    fn queued_merge_done(&self, child_id: &str) {
        self.0.lock().unwrap().push(child_id.to_string());
    }
}

fn status(s: &DelegationLaunch, status: &str) {
    s.service
        .update(|state| {
            let mut next = state.clone();
            for pane in next.sessions_mut().iter_mut().filter(|pane| pane.id() == PARENT) {
                *pane = Arc::new(mighty_core::model::RunSession { status: Some(status.into()), ..(**pane).clone() });
            }
            next
        })
        .unwrap();
}

/// L676.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cards_merge_is_queued_while_the_child_works_or_the_parent_runs_then_goes_through_by_itself() {
    Box::pin(queued_merges()).await;
}

async fn queued_merges() {
    let _lane = lane::enter().await;
    let place = DelegationPlace::new();
    let start = place.repository_default().await;
    let repo = place.repo();
    // c1 works and reports later, c2 reported while its parent runs, c3 ends without a report. Human-started, so no notice runs the
    // parent.
    let (reported, _) = place.reported_child("c2", &["c2.txt"], Some(&repo)).await;
    let (working, _) = place.reported_child("c1", &["c1.txt"], Some(&repo)).await;
    let silent = place.child("c3", Some(&repo)).await;
    let human = |record: ChildRecord| ChildRecord { started_by_human: Some(true), ..record };
    let working = human(ChildRecord {
        state: ChildState::Running,
        report_head: None,
        report_revision: 0,
        run_id: Some("r1".into()),
        ..host::record_of(&working)
    });
    let silent = human(ChildRecord { run_id: Some("r3".into()), ..host::record_of(&silent) });
    DelegationFileStore::new(place.profile())
        .save(&DelegationFile::of(vec![working, human(host::record_of(&reported)), silent], vec![], vec![], vec![], vec![]))
        .unwrap();
    let done = Arc::new(Done::default());
    let s = DelegationLaunch::start_with(
        Path::new(&place.profile()),
        &Path::new(&place.base()).join("fake"),
        &repo,
        &place.root(),
        place.git(),
        "acceptEdits",
        true,
        Duration::from_millis(100),
        done.clone(),
    )
    .await;
    // The sweep at launch stopped the running ones; their runs are back.
    s.coordinator().child_run_started("c1", "r1b").await;
    s.coordinator().child_run_started("c3", "r3b").await;
    status(&s, "running");

    // Each says what it waits on, and nothing is busy or merged.
    let result = |note: Option<mighty_delegation::cards::DelegationCardNote>| note.map(|note| note.result);
    assert_eq!(
        result(s.app.merge_child("c1").await),
        Some(DelegationCardResult::Queued(DelegationMergeWait::ChildRunning)),
        "a working child's merge is queued"
    );
    assert_eq!(
        result(s.app.merge_child("c2").await),
        Some(DelegationCardResult::Queued(DelegationMergeWait::ParentRunning)),
        "a running parent's merge is queued"
    );
    assert_eq!(
        result(s.app.merge_child("c3").await),
        Some(DelegationCardResult::Queued(DelegationMergeWait::ChildRunning)),
        "a third is queued too"
    );
    assert!(
        !s.app.is_card_busy("c1") && !s.app.is_card_busy("c2") && place.git_in(&repo, &["rev-parse", "HEAD"]).await == start,
        "a queued merge shows no busy state and merges nothing"
    );

    // The parent's run ends: the slow retry lets c2 through, and the window is told to offer the discard.
    status(&s, "idle");
    awaited("c2's merge", || (result(s.app.card_note("c2")) == Some(DelegationCardResult::Done)).then_some(())).await;
    assert!(
        s.file().children.iter().any(|record| record.id == "c2" && record.state == ChildState::Merged)
            && Path::new(&repo).join("c2.txt").exists(),
        "c2 merged by itself"
    );
    awaited("the discard offer for c2", || (*done.0.lock().unwrap() == ["c2"]).then_some(())).await;

    // c1 reports: its row changes and the merge goes through, with a merge commit over c2's.
    assert!(matches!(result(s.app.card_note("c1")), Some(DelegationCardResult::Queued(_))), "c1 is still queued");
    s.coordinator().child_run_ended("c1", "r1b", ChildRunEnd::Finished).await;
    awaited("c1's merge", || (result(s.app.card_note("c1")) == Some(DelegationCardResult::Done)).then_some(())).await;
    assert!(
        s.file().children.iter().any(|record| record.id == "c1" && record.state == ChildState::Merged)
            && Path::new(&repo).join("c1.txt").exists()
            && Path::new(&repo).join("c2.txt").exists(),
        "c1 merged by itself"
    );
    awaited("the discard offer for c1", || (done.0.lock().unwrap().len() == 2).then_some(())).await;

    // c3 ends with no report: the queued merge is dropped, and the card says so.
    s.coordinator().child_run_ended("c3", "r3b", ChildRunEnd::Finished).await;
    awaited("c3's dropped merge", || (result(s.app.card_note("c3")) == Some(DelegationCardResult::QueueDropped)).then_some(())).await;
    assert!(
        s.file().children.iter().any(|record| record.id == "c3" && record.state == ChildState::Ended)
            && place.git_in(&repo, &["log", "--oneline", "--", "c3.txt"]).await.is_empty(),
        "an ended child is not merged"
    );
    s.dispose().await;
}
