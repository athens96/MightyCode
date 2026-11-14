//! The delegation Core's halves of C# MightyCode.Core.Tests/DelegationAppVerification.cs that are W7-A's: the window's words (L678), a
//! host that throws (L679, its run-event pump half is W7-D's), and the delegation file read the Windows way (L680's file half; its git
//! and REPORT.md halves are W7-B's `tests/git.rs`). Also the merge and undo records of DelegationGitVerification's card checks, whose git
//! halves are W7-B's.

mod support;

use mighty_core::locale::Locale;
use mighty_core::shell::{CardAction, ChildState as RowState, DelegationChildRow, Tone};
use mighty_delegation::cards::{DelegationCardNote, DelegationCardResult, DelegationMergeWait, PendingChildDiscard, PendingParentClose};
use mighty_delegation::git::cleanup::{ChildCleanupHold, ChildCleanupOutcome};
use mighty_delegation::model::{ChildRecord, ChildRecordOptions, ChildState, ChildStateEvent, ChildStates, MergeKind, MergeRecord};
use mighty_delegation::request::{DelegationName, DelegationReasonCode};
use mighty_delegation::store::DelegationFile;
use mighty_delegation::text;

const COMMIT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn note(action: CardAction, result: DelegationCardResult) -> DelegationCardNote {
    DelegationCardNote { action, result }
}

/// "delegation: the window says each state, action and answer in the app's language"
#[test]
fn the_window_says_each_state_action_and_answer_in_the_apps_language() {
    for language in mighty_core::locale::LANGUAGES {
        let locale = Locale::new(language);
        for state in ChildState::ALL {
            let words = text::state(*state, &locale);
            assert!(!words.is_empty() && !words.starts_with("delegation."), "{state:?} has its words in {language}");
        }
        for action in [CardAction::Merge, CardAction::Undo, CardAction::Discard] {
            assert!(!text::action(action, &locale).starts_with("delegation."), "{action:?} has its button in {language}");
        }
    }
    let locale = Locale::new("en");
    let row = DelegationChildRow {
        id: "c1".into(),
        parent_session_id: "p".into(),
        state: RowState::Reported,
        task: Some("Task".into()),
        parent_branch: "main".into(),
        parent_checkout: None,
        can_undo: false,
        held_notices: 0,
        started_by_human: false,
        agent: "claude".into(),
    };
    let merged = text::note(&note(CardAction::Merge, DelegationCardResult::Done), &row, &locale).unwrap();
    assert!(merged.1 == Tone::Done && merged.0.contains("main"), "a merge names the branch it went into");
    for wait in [DelegationMergeWait::ChildRunning, DelegationMergeWait::ParentRunning, DelegationMergeWait::ParentChanges] {
        let queued = text::note(&note(CardAction::Merge, DelegationCardResult::Queued(wait)), &row, &locale).unwrap();
        assert!(
            queued.1 == Tone::Wait && queued.0.contains("main") && !queued.0.starts_with("delegation."),
            "{wait:?} is said with the branch"
        );
    }
    assert!(
        text::note(&note(CardAction::Merge, DelegationCardResult::QueueDropped), &row, &locale)
            .is_some_and(|n| n.1 == Tone::Wait && !n.0.is_empty())
            && text::note(&note(CardAction::Undo, DelegationCardResult::Queued(DelegationMergeWait::ChildRunning)), &row, &locale)
                .is_none()
            && text::note(&note(CardAction::Discard, DelegationCardResult::QueueDropped), &row, &locale).is_none(),
        "a dropped queue is said for the merge only"
    );
    let running = DelegationChildRow { state: RowState::Running, ..row.clone() };
    assert!(
        text::action_help(CardAction::Merge, &row, &locale) != text::action_help(CardAction::Merge, &running, &locale)
            && !text::action_help(CardAction::Merge, &running, &locale).starts_with("delegation."),
        "a working child's merge says it waits"
    );
    assert!(
        text::note(&note(CardAction::Discard, DelegationCardResult::Done), &row, &locale).is_none()
            && text::note(&note(CardAction::Discard, DelegationCardResult::Reconfirm(vec!["x".into()])), &row, &locale).is_none(),
        "a discard done or asked again says nothing on the card"
    );
    let failed =
        text::note(&note(CardAction::Merge, DelegationCardResult::Failed("fatal: not a git repository".into())), &row, &locale).unwrap();
    assert!(failed.1 == Tone::Err && failed.0.contains("fatal: not a git repository"), "git's output is the detail of a failure");
    let refused =
        text::note(&note(CardAction::Undo, DelegationCardResult::Refused(DelegationReasonCode::WorkspaceHasChildren)), &row, &locale)
            .unwrap();
    assert!(refused.1 == Tone::Wait && refused.0.contains("workspace_has_children"), "a refusal with no words of its own ends in its code");
    let pending = PendingChildDiscard {
        id: "c1".into(),
        title: "Task".into(),
        branch: "mighty/c1".into(),
        nested: Some(vec![r"C:\wt\c1\inner".into()]),
    };
    assert!(
        text::discard_message(&pending, &locale).contains(r"C:\wt\c1\inner")
            && text::discard_title(&pending, &locale)
                != text::discard_title(&PendingChildDiscard { nested: None, ..pending.clone() }, &locale),
        "the confirmation names the nested worktrees, and says when it cannot"
    );
    assert!(
        text::close_note(Some(&ChildCleanupOutcome::Kept(ChildCleanupHold::Link)), &locale).is_some_and(|n| !n.is_empty())
            && text::close_note(Some(&ChildCleanupOutcome::Kept(ChildCleanupHold::NotMerged)), &locale).is_none()
            && text::close_note(None, &locale).is_none(),
        "only the link hold is said on the card"
    );
    assert!(
        text::parent_close_message(&PendingParentClose { id: "p".into(), title: "Parent pane".into(), open_children: 2 }, &locale)
            .contains("Parent pane"),
        "the parent close names its pane"
    );
    // Every reason code reads as words, those with none of their own ending in the code.
    for reason in DelegationReasonCode::ALL {
        let words = text::reason(*reason, &locale);
        assert!(!words.starts_with("delegation."), "{reason:?} has its words");
    }
    assert!(text::reason(DelegationReasonCode::HeldNotRemovable, &locale).contains(DelegationReasonCode::HeldNotRemovable.name()));
}

/// The merge and undo records of DelegationGitVerification's card checks (`RecordingMerge`, `RecordingUndo`): a merge goes on record with
/// its child merged, an undo takes it out and the child back to reported (a closed one stays closed), and neither applies twice.
#[test]
fn a_merge_and_its_undo_go_on_record_once() {
    let child = ChildRecord::make_with(
        "c1",
        "parent",
        "/w/c1",
        "main",
        COMMIT,
        "acceptEdits",
        "k",
        ChildRecordOptions { state: Some(ChildState::Running), parent_checkout: Some("/repo".into()), ..Default::default() },
    )
    .recording_report(&"b".repeat(40))
    .unwrap();
    let record = MergeRecord::new("c1", MergeKind::ToolFastForward, "main", COMMIT, &"b".repeat(40), &"b".repeat(40));
    let file = DelegationFile::of(vec![child.clone()], vec![], vec![], vec![], vec![]).recording_merge(&record).unwrap();
    assert!(
        file.children[0].state == ChildState::Merged && file.merges == [record.clone()],
        "the merge goes on record with the child merged"
    );
    assert!(
        file.recording_merge(&record).is_none()
            && file.recording_merge(&MergeRecord { child_id: "missing".into(), ..record.clone() }).is_none(),
        "no second record is written"
    );
    let undone = file.recording_undo(&record).unwrap();
    assert!(
        undone.children[0].state == ChildState::Reported
            && undone.children[0].report_head == child.report_head
            && undone.merges.is_empty()
            && undone.recording_undo(&record).is_none(),
        "the child is reported again"
    );
    let closed = file.setting_child(0, file.children[0].applying(ChildStateEvent::ClosePane).unwrap());
    let card = closed.recording_undo(&record).unwrap();
    assert!(
        card.children[0].state == ChildState::Closed
            && card.merges.is_empty()
            && card.children[0].state.after(ChildStateEvent::Discard) == Some(ChildState::Discarded),
        "the closed child stays closed, for a human to discard"
    );
}

/// "delegation: git ends on its deadline, and reports and the file read the Windows way" — the delegation file's half: a load waits out
/// a program holding the file shut, and one refused past its tries is unreadable.
#[cfg(windows)]
#[test]
fn the_delegation_file_is_read_again_while_another_program_holds_it() {
    use std::os::windows::fs::OpenOptionsExt as _;
    use std::sync::{Arc, Mutex};

    use mighty_delegation::store::{DelegationFileError, DelegationFileStore};

    let folder = support::place::temp("held");
    let open_shut = |path: &std::path::Path| std::fs::OpenOptions::new().read(true).share_mode(0).open(path).unwrap();
    let path = folder.join("profile").join(mighty_delegation::store::FILE_NAME);
    DelegationFileStore::new(folder.join("profile")).save(&DelegationFile::empty()).unwrap();
    // The reader lets go once the load has been refused, so the load has waited it out whatever the machine's pace.
    let held = Arc::new(Mutex::new(Some(open_shut(&path))));
    let refusals = Arc::new(Mutex::new(0));
    let (letting, counting) = (held.clone(), refusals.clone());
    let store = DelegationFileStore::new(folder.join("profile")).with_refused(move || {
        *counting.lock().unwrap() += 1;
        letting.lock().unwrap().take();
    });
    assert!(store.load() == Ok(DelegationFile::empty()) && *refusals.lock().unwrap() >= 1, "a load waits out a reader");
    {
        let _shut = open_shut(&path);
        assert_eq!(
            DelegationFileStore::with_attempts(folder.join("profile"), 3).load(),
            Err(DelegationFileError::Unreadable),
            "a read refused past its tries is unreadable"
        );
    }
    support::place::remove_folder(&folder);
}

#[cfg(windows)]
mod windows {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use mighty_delegation::host::{DelegationChildPane, DelegationHost, DelegationPaneActivity, DelegationPaneState};
    use mighty_delegation::model::{ChildRecord, ChildState, DeliveryLane, DeliveryRoute};
    use mighty_delegation::reports::ChildRunEnd;
    use mighty_delegation::store::{DelegationFile, DelegationFileStore};
    use mighty_delegation::tools;
    use mighty_platform::BoxFuture;

    use super::support::host::{self, caller, eventually, record_of, request};
    use super::support::lane;
    use super::support::place::DelegationPlace;

    /// A host whose every call fails the way a host can: by panicking (C#'s throwing host), but for the parent's state.
    struct ThrowingHost {
        parent: DelegationPaneState,
        delivers: AtomicUsize,
    }

    impl DelegationHost for ThrowingHost {
        fn create_pane(&self, _: DelegationChildPane) -> BoxFuture<'_, bool> {
            Box::pin(async { panic!("host") })
        }
        fn start_run(&self, _: String, _: String) -> BoxFuture<'_, Option<String>> {
            Box::pin(async { panic!("host") })
        }
        fn deliver(&self, _: String, _: String, _: DeliveryRoute) -> BoxFuture<'_, Option<String>> {
            self.delivers.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { panic!("host") })
        }
        fn pane_state(&self, session_id: String) -> BoxFuture<'_, Option<DelegationPaneState>> {
            let parent = (session_id == self.parent.session_id).then(|| self.parent.clone());
            Box::pin(async move { parent.or_else(|| panic!("host")) })
        }
        fn stop_run(&self, _: String) -> BoxFuture<'_, ()> {
            Box::pin(async { panic!("host") })
        }
    }

    /// "delegation: a host that throws is a host that did not take the call"
    #[tokio::test]
    async fn a_host_that_throws_is_a_host_that_did_not_take_the_call() {
        let _lane = lane::enter().await;
        Box::pin(async {
            let place = DelegationPlace::new();
            place.repository_default().await;
            let child = ChildRecord { run_id: Some("r1".into()), ..record_of(&place.child("c1", None).await) };
            let store = DelegationFileStore::new(place.profile());
            store.save(&DelegationFile::of(vec![child.clone()], vec![], vec![], vec![], vec![])).unwrap();
            let parent = DelegationPaneState {
                session_id: "parent".into(),
                permission_mode: "auto".into(),
                folder: place.repo(),
                kind: "claude".into(),
                provider: "claude".into(),
                parent_session_id: None,
                run_id: Some("run-p".into()),
                activity: DelegationPaneActivity::Finished,
            };
            let throwing = Arc::new(ThrowingHost { parent, delivers: AtomicUsize::new(0) });
            let coordinator =
                host::coordinator(DelegationFileStore::new(place.profile()), throwing.clone(), host::maker(&place, 50_000_000_000), || {
                    true
                })
                .unwrap();
            std::fs::write(mighty_delegation::git::worktree::report_file_in(&child.worktree_path), "# Report\n").unwrap();
            let notice = coordinator.child_run_ended("c1", "r1", ChildRunEnd::Finished).await.unwrap();
            assert!(
                notice.lane == DeliveryLane::Pending
                    && throwing.delivers.load(Ordering::SeqCst) > 0
                    && store.load().unwrap().notices[0].lane == DeliveryLane::Pending,
                "an item the host threw on is not delivered"
            );
            for tool in tools::ALL {
                let arguments: Vec<(&str, &str)> = tool
                    .arguments
                    .iter()
                    .map(|a| {
                        (
                            a.name,
                            match a.name {
                                "mode" => "plan",
                                "child" => "c1",
                                "agent" => "claude",
                                _ => "x",
                            },
                        )
                    })
                    .collect();
                let answer = coordinator.handle_call(request(tool.name, &arguments), caller("parent", "claude", "claude", true)).await;
                assert!(answer != Default::default(), "{} is answered", tool.name);
            }
            assert!(
                eventually(60_000, || coordinator.file().children.iter().any(|c| c.id != "c1" && c.state == ChildState::Failed)).await
                    && !coordinator.file().children.iter().any(|c| c.state == ChildState::Creating),
                "a start the host threw on fails, with no child left creating"
            );
        })
        .await;
    }
}
