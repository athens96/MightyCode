//! The app's ordered run-event pump in front of the coordinator (app_host.rs `DelegationRunEventPump::for_coordinator`), W7-D's halves
//! of C#'s checks: DelegationCoordinatorVerification "delegation: a launch interrupts the runs quitting killed" (L657: "a run event
//! reaches it through the app's ordered pump"), "delegation: the file near its cap, and the switch turned off" (L665: a child's report
//! still recorded with the switch off), DelegationAppVerification "delegation: a host that throws is a host that did not take the call"
//! (L679: one event that failed never stops the ones after it).

mod support;

#[cfg(windows)]
use std::path::Path;
use std::sync::{Arc, Mutex};

use mighty_delegation::app_host::{ChildRunEnd, DelegationRunEvent, DelegationRunEventPump};
#[cfg(windows)]
use mighty_delegation::cards::DelegationCardAction;
#[cfg(windows)]
use mighty_delegation::git::worktree::report_file_in;
use mighty_delegation::host::DelegationPaneActivity;
use mighty_delegation::model::{ChildRecord, ChildState};
#[cfg(windows)]
use mighty_delegation::model::{ChildRecordOptions, DeliveryLane, NoticeKind};
#[cfg(windows)]
use mighty_delegation::sidebar;
use mighty_delegation::store::{DelegationFile, DelegationFileStore};
use support::{host, lane, place::DelegationPlace};

fn runtime() -> tokio::runtime::Handle {
    tokio::runtime::Handle::current()
}

/// L657's pump half: a child's run start and end, sent through the app's pump, are recorded in order, and the rows follow.
#[tokio::test]
async fn the_pumps_events_are_recorded_in_order() {
    let _lane = lane::enter().await;
    let folder = host::temp_folder("pump-order");
    let store = DelegationFileStore::new(&folder);
    let running = ChildRecord { run_id: Some("r1".into()), ..host::record("running", ChildState::Running, "parent") };
    store.save(&DelegationFile::of(vec![running], vec![], vec![], vec![], vec![])).unwrap();
    let test_host = host::TestHost::new(Some(store.file_path()), "run");
    test_host.set_claude("parent", DelegationPaneActivity::Idle, Some("p1"));
    let place = DelegationPlace::new();
    let coordinator = host::coordinator(DelegationFileStore::new(&folder), test_host.clone(), host::maker(&place, 50_000_000_000), || true)
        .expect("the file loads");
    let pump = DelegationRunEventPump::for_coordinator(&runtime(), coordinator.clone());
    pump.send(DelegationRunEvent::Started { child_id: "running".into(), run_id: "r2".into() });
    pump.send(DelegationRunEvent::Ended { child_id: "running".into(), run_id: "r2".into(), end: ChildRunEnd::Stopped });
    pump.finish().await;
    let child = coordinator.file().children.clone().into_iter().find(|child| child.id == "running").unwrap();
    assert!(
        child.state == ChildState::Ended && child.run_id.as_deref() == Some("r2"),
        "the pump's events are recorded in order: {child:?}"
    );
    assert_eq!(coordinator.child_rows().borrow()[0].state, ChildState::Ended, "the rows follow");
    let _ = std::fs::remove_dir_all(&folder);
}

/// L665's pump half: with the switch off, a child still runs in its worktree, and its run's end, through the app's ordered events,
/// still records its report for the parent; its card and its cleanup go on.
/// Its git (the child's commit) runs through the process host, Windows' until the Mac stage.
#[cfg(windows)]
#[tokio::test]
async fn with_the_switch_off_a_childs_end_still_records_its_report() {
    let _lane = lane::enter().await;
    let place = DelegationPlace::new();
    place.repository_default().await;
    let made = place.child("c1", Some(&place.repo())).await;
    let base = ChildRecord {
        run_id: Some("run-of-c1".into()),
        ..ChildRecord::make_with(
            &made.id,
            "parent",
            &made.worktree_path,
            &made.parent_branch,
            &made.base_commit,
            "acceptEdits",
            "key-c1",
            ChildRecordOptions { state: Some(ChildState::Running), parent_checkout: Some(place.repo()), ..Default::default() },
        )
    };
    let store = DelegationFileStore::new(place.profile());
    store.save(&DelegationFile::of(vec![base.clone()], vec![], vec![], vec![], vec![])).unwrap();
    let test_host = host::TestHost::new(Some(store.file_path()), "run");
    test_host.set_pane("parent", DelegationPaneActivity::Finished, Some("p1"), "claude", Some(&place.repo()));
    let coordinator =
        host::coordinator(DelegationFileStore::new(place.profile()), test_host.clone(), host::maker(&place, 50_000_000_000), || false)
            .expect("the file loads");
    std::fs::write(Path::new(&made.worktree_path).join("work.txt"), "work\n").unwrap();
    place.git_in(&made.worktree_path, &["add", "-A"]).await;
    place.git_in(&made.worktree_path, &["commit", "-q", "-m", "work"]).await;
    std::fs::write(report_file_in(&made.worktree_path), "Done with the switch off.\n").unwrap();
    let pump = DelegationRunEventPump::for_coordinator(&runtime(), coordinator.clone());
    pump.send(DelegationRunEvent::Ended { child_id: "c1".into(), run_id: "run-of-c1".into(), end: ChildRunEnd::Finished });
    pump.finish().await;
    let file = coordinator.file();
    let child = file.children.iter().find(|child| child.id == "c1").unwrap();
    assert!(child.state == ChildState::Reported && child.report_revision == 1, "the report is recorded: {child:?}");
    let notice = file.notices.first().expect("one notice");
    assert!(file.notices.len() == 1 && notice.kind == NoticeKind::Reported, "one reported notice: {:?}", file.notices);
    assert!(
        host::eventually(5000, || coordinator.file().notices.first().is_some_and(|n| n.lane == DeliveryLane::Delivered)).await,
        "the idle parent takes it"
    );
    assert_eq!(
        sidebar::rows(&coordinator.file())[0].card_actions(),
        [DelegationCardAction::Merge, DelegationCardAction::Discard],
        "its card goes on with the switch off"
    );
}

/// L679's pump half: one event whose handling failed never stops the ones after it.
#[tokio::test]
async fn the_pump_goes_on_after_a_failed_event() {
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let kept = seen.clone();
    let pump = DelegationRunEventPump::new(
        &runtime(),
        Arc::new(move |change| {
            let kept = kept.clone();
            Box::pin(async move {
                let DelegationRunEvent::Started { run_id, .. } = change else { return };
                if run_id == "r1" {
                    panic!("handler");
                }
                kept.lock().unwrap().push(run_id);
            })
        }),
    );
    pump.send(DelegationRunEvent::Started { child_id: "c".into(), run_id: "r1".into() });
    pump.send(DelegationRunEvent::Started { child_id: "c".into(), run_id: "r2".into() });
    pump.finish().await;
    assert_eq!(*seen.lock().unwrap(), ["r2"], "the pump goes on after a failed event");
}
