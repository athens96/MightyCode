//! The delegation model, file, pruning, gates, tools' words, sidebar rows and merge queue (C# MightyCode.Core.Tests/
//! DelegationVerification.cs, ported from the Mac's DelegationModelTests, DelegationFileStoreTests, DelegationPruningTests and
//! DelegationSidebarTests), each check under its C# name, and the byte round trip of the repository's delegation-record files.

mod support;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mighty_core::model::{AppSnapshot, RunSession, Workspace};
use mighty_core::shell::{CardAction, DelegationChildRow, Tone, delegation_tree};
use mighty_delegation::cards::{DelegationMergeStep, DelegationMergeWait, merge_step, wait_after};
use mighty_delegation::coordinator::{
    self, ANSWER_SECONDS, MAXIMUM_DEPTH, MAXIMUM_FOLLOW_UP_BYTES, MAXIMUM_FOLLOW_UPS, MAXIMUM_OPEN_CHILDREN,
};
use mighty_delegation::copy::{self, DelegationCopy, DelegationCopyKind, MAXIMUM_COPY_BYTES, TRUNCATION_MARKER};
use mighty_delegation::delivery::DELIVERY_OFFERS;
use mighty_delegation::git::cleanup::ChildCleanupHold;
use mighty_delegation::git::worktree::MINIMUM_FREE_BYTES;
use mighty_delegation::model::{
    ChildRecord, ChildRecordOptions, ChildState, ChildStateEvent, ChildStates, DeliveryLane, DeliveryReceipt, DeliveryRoute, FollowUp,
    MergeKind, MergeRecord, Notice, NoticeKind, request_key,
};
use mighty_delegation::pruning::{DelegationAdmission, DelegationPruneContext, element_bytes, new_child_room, report_copy_room};
use mighty_delegation::request::{
    DETACHED_MESSAGE, DelegationChildInfo, DelegationHandler, DelegationIoHandler, DelegationName, DelegationReasonCode, DelegationRequest,
    DelegationResponse,
};
use mighty_delegation::store::{self, DelegationFile, DelegationFileError, DelegationFileStore, MAXIMUM_FILE_BYTES, SaveError};
use mighty_delegation::{providers, sidebar, tools};
use support::host::{caller, request, temp_folder};
use unicode_segmentation::UnicodeSegmentation;

const COMMIT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn record(id: &str, parent: &str, state: ChildState) -> ChildRecord {
    let worktree = std::env::temp_dir().join("worktrees").join(id);
    ChildRecord::make_with(
        id,
        parent,
        &worktree.to_string_lossy(),
        "main",
        "abc123",
        "acceptEdits",
        &format!("key-{id}"),
        ChildRecordOptions { state: Some(state), ..Default::default() },
    )
}

fn copy_of(child: &str, kind: DelegationCopyKind, text: &str, revision: Option<i64>) -> DelegationCopy {
    DelegationCopy::of(child, kind, revision.unwrap_or(if kind == DelegationCopyKind::Report { 1 } else { 0 }), text.as_bytes(), None)
}

/// "delegation: child states, events and the only moves between them"
#[test]
fn child_states_events_and_the_only_moves_between_them() {
    use ChildState as S;
    use ChildStateEvent as E;
    let states = ChildState::ALL;
    assert_eq!(
        states.iter().map(|s| s.name()).collect::<Vec<_>>(),
        ["creating", "running", "waiting", "reported", "ended", "interrupted", "merged", "failed", "closed", "discarded"],
        "the states carry the ontology names"
    );
    // The ontology's moves, written out by hand. Every (state, event) pair not listed here must be refused.
    let after = |state: S| -> Vec<(E, S)> {
        match state {
            S::Creating => {
                vec![(E::StartRun, S::Running), (E::FailStart, S::Failed), (E::ClosePane, S::Closed), (E::Discard, S::Discarded)]
            }
            S::Running => vec![
                (E::AskPermission, S::Waiting),
                (E::EndWithReport, S::Reported),
                (E::EndWithoutReport, S::Ended),
                (E::EndUnchanged, S::Reported),
                (E::QuitApp, S::Interrupted),
                (E::ClosePane, S::Closed),
                (E::Discard, S::Discarded),
            ],
            S::Waiting => vec![(E::AnswerPermission, S::Running), (E::ClosePane, S::Closed), (E::Discard, S::Discarded)],
            S::Reported => vec![(E::StartRun, S::Running), (E::Merge, S::Merged), (E::ClosePane, S::Closed), (E::Discard, S::Discarded)],
            S::Ended | S::Interrupted => vec![(E::StartRun, S::Running), (E::ClosePane, S::Closed), (E::Discard, S::Discarded)],
            S::Merged => {
                vec![(E::StartRun, S::Running), (E::UndoMerge, S::Reported), (E::ClosePane, S::Closed), (E::Discard, S::Discarded)]
            }
            S::Failed => vec![(E::Discard, S::Discarded)],
            S::Closed => vec![(E::UndoMerge, S::Closed), (E::Discard, S::Discarded)],
            S::Discarded => vec![],
        }
    };
    let (mut allowed, mut refused, mut moves) = (0, 0, 0);
    for state in states {
        for change in ChildStateEvent::ALL {
            let expected = after(*state).iter().find(|(e, _)| *e == change).map(|(_, s)| *s);
            assert_eq!(state.after(change), expected, "{state:?} + {change:?}");
            if expected.is_none() {
                refused += 1;
            } else {
                allowed += 1;
            }
        }
        for to in states {
            let expected = to != state && after(*state).iter().any(|(_, s)| s == to);
            assert_eq!(state.can_move(*to), expected, "{state:?} -> {to:?}");
            if expected {
                moves += 1;
            }
        }
    }
    assert!(allowed == 31 && refused == 89 && moves == 29, "exactly the ontology's 31 allowed pairs, 89 refusals and 29 moves");
    assert!(
        !S::Closed.can_move(S::Reported) && !S::Closed.can_move(S::Merged) && !S::Closed.can_move(S::Running),
        "a closed child is never reopened"
    );
    assert!(
        S::Failed.after(E::ClosePane).is_none()
            && S::Failed.after(E::StartRun).is_none()
            && ChildStateEvent::ALL.iter().all(|e| S::Discarded.after(*e).is_none()),
        "failed is only discarded, and discarded is final"
    );
    let filter = |f: &dyn Fn(S) -> bool| states.iter().copied().filter(|s| f(*s)).collect::<Vec<_>>();
    assert_eq!(filter(&|s| s.after(E::Merge).is_some()), [S::Reported], "only a reported child merges");
    assert_eq!(filter(&|s| s.after(E::UndoMerge).is_some()), [S::Merged, S::Closed], "undo is only for a merged or closed child");
    assert_eq!(
        filter(&|s| s.is_open()),
        [S::Creating, S::Running, S::Waiting, S::Reported, S::Ended, S::Interrupted, S::Merged],
        "open is every state except failed, closed and discarded"
    );
    assert_eq!(
        filter(&|s| s.after(E::ClosePane) == Some(S::Closed)),
        filter(&|s| s.is_open()),
        "exactly the open states can have their pane closed"
    );

    // A record moves only by allowed events, and a refused event changes nothing.
    let mut child = record("child-1", "parent-1", S::Creating);
    assert!(child.branch == "mighty/child-1" && child.state == S::Creating && child.report_revision == 0 && child.report_head.is_none());
    let step = |child: &mut ChildRecord, change: E, moved: bool, state: S| {
        let next = child.applying(change);
        assert!(
            next.is_some() == moved
                && next.as_ref().unwrap_or(child).state == state
                && child.applied(change) == *next.as_ref().unwrap_or(child),
            "{change:?} from {:?}",
            child.state
        );
        *child = next.unwrap_or_else(|| child.clone());
    };
    let report = |child: &mut ChildRecord, head: &str, moved: bool| {
        let next = child.recording_report(head);
        let matched = next.is_some() == moved;
        *child = next.unwrap_or_else(|| child.clone());
        matched
    };
    step(&mut child, E::Merge, false, S::Creating);
    assert!(report(&mut child, "def456", false), "no report before a run");
    step(&mut child, E::StartRun, true, S::Running);
    assert!(report(&mut child, "def456", true), "a running child reports");
    assert!(child.state == S::Reported && child.report_revision == 1 && child.report_head.as_deref() == Some("def456"));
    step(&mut child, E::StartRun, true, S::Running);
    assert!(
        report(&mut child, "fed789", true) && child.report_revision == 2 && child.report_head.as_deref() == Some("fed789"),
        "the next report is revision 2"
    );
    step(&mut child, E::Merge, true, S::Merged);
    step(&mut child, E::UndoMerge, true, S::Reported);
    step(&mut child, E::Merge, true, S::Merged);
    step(&mut child, E::ClosePane, true, S::Closed);
    step(&mut child, E::UndoMerge, true, S::Closed);
    step(&mut child, E::StartRun, false, S::Closed);
    assert!(report(&mut child, "000", false) && child.report_revision == 2, "a closed child takes no report");
    step(&mut child, E::Discard, true, S::Discarded);
    step(&mut child, E::Discard, false, S::Discarded);
}

/// "delegation: reason codes, records on the wire and the request key"
#[test]
fn reason_codes_records_on_the_wire_and_the_request_key() {
    let names = |all: Vec<&'static str>| all;
    assert_eq!(
        names(DelegationReasonCode::ALL.iter().map(|c| c.name()).collect()),
        [
            "claude_only",
            "child_cannot_delegate",
            "width_cap",
            "low_disk",
            "store_full",
            "not_git",
            "unborn_branch",
            "detached_head",
            "wider_mode",
            "discard_human_only",
            "not_reported",
            "tracked_changes",
            "head_moved",
            "diverged",
            "branch_not_checked_out",
            "child_closed",
            "parent_closed",
            "follow_up_limit",
            "worktree_missing",
            "merge_conflict",
            "undo_parent_moved",
            "parent_busy",
            "workspace_has_children",
            "held_not_removable",
            "human_started",
            "merge_human_only",
            "agent_unknown",
            "agent_unavailable"
        ],
        "the reason codes are the ontology list in order"
    );
    assert_eq!(names(NoticeKind::ALL.iter().map(|c| c.name()).collect()), ["reported", "ended_without_report", "failed_to_start"]);
    assert_eq!(names(DeliveryLane::ALL.iter().map(|c| c.name()).collect()), ["pending", "held", "delivered"]);
    assert_eq!(names(DeliveryRoute::ALL.iter().map(|c| c.name()).collect()), ["steer", "queue"]);
    assert_eq!(names(MergeKind::ALL.iter().map(|c| c.name()).collect()), ["tool_fast_forward", "card_fast_forward", "card_merge_commit"]);
    assert_eq!(
        names(ChildCleanupHold::ALL.iter().map(|c| c.name()).collect()),
        [
            "not_merged",
            "child_busy",
            "worktree_missing",
            "head_moved",
            "tracked_changes",
            "untracked_files",
            "nested_worktree",
            "branch_not_checked_out",
            "not_in_parent",
            "link"
        ],
        "the Mac's nine cleanup holds, and Windows' link"
    );
    assert!(
        DelegationReasonCode::parse("width_cap") == Some(DelegationReasonCode::WidthCap)
            && DelegationReasonCode::parse("WidthCap").is_none()
    );

    let receipt = DeliveryReceipt::new("2026-10-09T00:00:00Z", DeliveryRoute::Steer, "run-2");
    let notice = Notice::new("n1", "child-1", 1, NoticeKind::EndedWithoutReport, DeliveryLane::Delivered).with_receipt(receipt);
    let follow_up = FollowUp::new("f1", "child-1", "Also update the tests.", DeliveryLane::Held);
    let merge = MergeRecord::new("child-1", MergeKind::CardMergeCommit, "main", "abc123", "fed789", "def456");
    let child = ChildRecord {
        follow_up_count: 2,
        report_revision: 3,
        report_head: Some("def456".into()),
        ..record("child-1", "parent-1", ChildState::Merged)
    };
    assert_eq!(
        String::from_utf8(store::encode(&notice)).unwrap(),
        r#"{"childId":"child-1","id":"n1","kind":"ended_without_report","lane":"delivered","receipt":{"route":"steer","runId":"run-2","time":"2026-10-09T00:00:00Z"},"reportRevision":1}"#,
        "a notice is written with the Mac's field names, sorted"
    );
    assert!(String::from_utf8(store::encode(&merge)).unwrap().contains("\"card_merge_commit\""));
    let file = DelegationFile::of(vec![child.clone()], vec![notice.clone()], vec![follow_up.clone()], vec![merge.clone()], vec![]);
    assert_eq!(store::decode(&store::encode(&file)), Some(file), "records round-trip");
    // The app's wire options write the same names, so a response crosses the pane pipe as the Mac's does.
    assert_eq!(serde_json::to_string(&DelegationResponse::refusal(DelegationReasonCode::WidthCap)).unwrap(), r#"{"refused":"width_cap"}"#);
    let response = DelegationResponse { merged: Some(merge.clone()), child: Some(DelegationChildInfo::of(&child)), ..Default::default() };
    let back: DelegationResponse = serde_json::from_str(&serde_json::to_string(&response).unwrap()).unwrap();
    assert!(back.merged == Some(merge) && back.child.map(|c| c.state) == Some(ChildState::Merged), "a response round-trips on the wire");
    let read: DelegationRequest = serde_json::from_str(r#"{"tool":"merge","arguments":{"child":"c1","expected_head":"abc"}}"#).unwrap();
    assert!(read.tool == "merge" && read.arguments["expected_head"] == "abc");

    let key = request_key("parent-1", "run-1", "Fix the login bug", "acceptEdits", None);
    // Golden values pin the encoding: each part as an 8-byte big-endian UTF-8 byte count and its bytes, then SHA-256.
    assert_eq!(key, "0767d17eeeaaa20c3f5c0e3a11dc260bd005cc116a76ea02adb2c78fb3a48ef7", "the request key is the Mac's");
    assert_eq!(
        request_key("parent-1", "run-1", "Tidy the caf\u{e9} menu \u{2192} done", "plan", None),
        "7f4b512df24f7e3bc398f2a2ca4f9ac5ce328ce54ec824429a2c55bc3d49e976"
    );
    let variants: HashSet<String> = [
        key.clone(),
        request_key("parent-2", "run-1", "Fix the login bug", "acceptEdits", None),
        request_key("parent-1", "run-2", "Fix the login bug", "acceptEdits", None),
        request_key("parent-1", "run-1", "Fix the login bug.", "acceptEdits", None),
        request_key("parent-1", "run-1", "Fix the login bug", "plan", None),
    ]
    .into();
    assert!(variants.len() == 5 && request_key("parent-1", "run-1", "Fix the login bug", "acceptEdits", None) == key);
    assert!(
        request_key("ab", "c", "t", "plan", None) != request_key("a", "bc", "t", "plan", None)
            && request_key("p", "r", "do it plan", "", None) != request_key("p", "r", "do it", " plan", None),
        "moving text between parts never collides"
    );
}

fn sample() -> DelegationFile {
    let reported = ChildRecord::make_with(
        "c1",
        "p1",
        r"C:\Temp\worktrees\c1",
        "main",
        "a1",
        "acceptEdits",
        "k1",
        ChildRecordOptions { follow_up_count: 1, state: Some(ChildState::Running), ..Default::default() },
    )
    .recording_report("b2")
    .unwrap();
    let closed = ChildRecord::make_with(
        "c2",
        "p1",
        "/tmp/worktrees/c2",
        "main",
        "a1",
        "plan",
        "k2",
        ChildRecordOptions { state: Some(ChildState::Closed), ..Default::default() },
    );
    DelegationFile::of(
        vec![reported, closed],
        vec![
            Notice::new("n1", "c1", 1, NoticeKind::Reported, DeliveryLane::Delivered).with_receipt(DeliveryReceipt::new(
                "2026-10-09T01:02:03Z",
                DeliveryRoute::Steer,
                "run-7",
            )),
            Notice::new("n2", "c2", 0, NoticeKind::EndedWithoutReport, DeliveryLane::Held),
        ],
        vec![FollowUp::new("f1", "c1", "테스트도 추가해 줘", DeliveryLane::Delivered).with_receipt(DeliveryReceipt::new(
            "2026-10-09T01:05:00Z",
            DeliveryRoute::Queue,
            "run-8",
        ))],
        vec![MergeRecord::new("c1", MergeKind::ToolFastForward, "main", "a1", "b2", "b2")],
        vec![],
    )
    .setting_copy(copy_of("c1", DelegationCopyKind::Task, "# 할 일\n버그를 고쳐라", None))
    .setting_copy(copy_of("c1", DelegationCopyKind::Report, "# 보고\n고쳤다", None))
}

fn entries(folder: &Path) -> Vec<String> {
    let mut names: Vec<String> =
        std::fs::read_dir(folder).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    names
}

/// "delegation: the file saves, restores, refuses at 4 MiB and waits out a reader"
#[tokio::test]
async fn the_file_saves_restores_refuses_at_4_mib_and_waits_out_a_reader() {
    let folder = temp_folder("file");
    let missing = DelegationFileStore::new(folder.join("profile"));
    assert!(
        missing.load() == Ok(DelegationFile::empty()) && !missing.file_path().exists() && !missing.folder().exists(),
        "a missing file loads as empty and creates nothing"
    );

    // Beside the workspace state, with the same file name as the Mac.
    std::fs::write(folder.join("workspace-state.json"), "{}").unwrap();
    let store = DelegationFileStore::new(&folder);
    let file = sample();
    store.save(&file).unwrap();
    assert_eq!(store.file_path(), folder.join("delegation-state.json"));
    assert_eq!(entries(&folder), ["delegation-state.json", "workspace-state.json"], "a save leaves no temporary file beside the state");
    let restored = DelegationFileStore::new(&folder).load().unwrap();
    assert!(
        restored == file
            && restored.children[0].report_revision == 1
            && restored.children[0].state == ChildState::Reported
            && restored.notices[0].receipt.as_ref().map(|r| r.run_id.as_str()) == Some("run-7")
            && restored.follow_ups[0].receipt.as_ref().map(|r| r.route) == Some(DeliveryRoute::Queue)
            && restored.merges[0].merged_commit == "b2",
        "a saved file restores exactly"
    );
    assert!(
        restored.copy("c1", DelegationCopyKind::Task).map(|c| c.text.as_str()) == Some("# 할 일\n버그를 고쳐라")
            && !restored.copy("c1", DelegationCopyKind::Report).unwrap().truncated
    );
    let bytes = std::fs::read(store.file_path()).unwrap();
    store.save(&restored).unwrap();
    assert_eq!(std::fs::read(store.file_path()).unwrap(), bytes, "saving the restored file writes the same bytes");
    // The Mac's encoder: version 1, sorted keys, escaped slashes and backslashes, text outside ASCII as it is, absent values left out.
    let text = String::from_utf8(bytes).unwrap();
    assert!(
        text.starts_with(r#"{"children":[{"baseCommit":"a1","branch":"mighty\/c1","followUpCount":1,"id":"c1","parentBranch":"main","parentSessionId":"p1","reportHead":"b2","reportRevision":1,"requestKey":"k1","startingMode":"acceptEdits","state":"reported","worktreePath":"C:\\Temp\\worktrees\\c1"}"#),
        "a child is written as the Mac writes it: {}",
        &text[..300.min(text.len())]
    );
    assert!(text.ends_with(r#","version":1}"#) && text.contains("테스트도") && !text.contains("null") && !text.contains("\\u"));

    let replaced = file.setting_copy(copy_of("c1", DelegationCopyKind::Report, "# 2", Some(2)));
    assert_eq!(
        replaced
            .copies
            .iter()
            .filter(|c| c.child_id == "c1" && c.kind == DelegationCopyKind::Report)
            .map(|c| c.revision)
            .collect::<Vec<_>>(),
        [2]
    );
    assert_eq!(replaced.copy("c1", DelegationCopyKind::Task).map(|c| c.revision), Some(0), "the other kind's copy stays");

    // A save over 4 MiB is refused and the earlier file stays.
    const { assert!(MAXIMUM_FILE_BYTES == 4 * 1024 * 1024 && MAXIMUM_COPY_BYTES == 64 * 1024, "the caps are the Mac's") };
    let before = std::fs::read(store.file_path()).unwrap();
    let full = "r".repeat(MAXIMUM_COPY_BYTES * 2);
    let mut fits = DelegationFile::empty();
    for index in 0..63 {
        fits = fits.setting_copy(copy_of(&format!("c{index}"), DelegationCopyKind::Report, &full, None));
    }
    assert!(store::encode(&fits).len() <= MAXIMUM_FILE_BYTES, "63 full copies fit");
    let over = fits.setting_copy(copy_of("c63", DelegationCopyKind::Report, &full, None)).setting_copy(copy_of(
        "c64",
        DelegationCopyKind::Report,
        &full,
        None,
    ));
    assert!(store::encode(&over).len() > MAXIMUM_FILE_BYTES, "65 do not");
    assert!(matches!(store.save(&over), Err(SaveError::File(DelegationFileError::Full))), "a file over the cap is refused");
    assert!(std::fs::read(store.file_path()).unwrap() == before && store.load() == Ok(sample()), "a refused save leaves the earlier file");

    // An oversized or damaged file, one of another version or one missing a field is refused on load and left alone.
    let damaged: Vec<Vec<u8>> = vec![
        vec![0; MAXIMUM_FILE_BYTES + 1],
        b"{".to_vec(),
        br#"{"version":2,"children":[],"notices":[],"followUps":[],"merges":[],"copies":[]}"#.to_vec(),
        br#"{"version":1,"children":[],"notices":[],"followUps":[],"merges":[]}"#.to_vec(),
        br#"{"version":1,"children":[null],"notices":[],"followUps":[],"merges":[],"copies":[]}"#.to_vec(),
        br#"{"version":1,"children":[],"notices":[{"id":"n","childId":"c","reportRevision":0,"kind":"shouted","lane":"held"}],"followUps":[],"merges":[],"copies":[]}"#.to_vec(),
        br#"{"version":1,"children":[],"notices":[{"id":"n","childId":"c","reportRevision":0,"kind":"reported"}],"followUps":[],"merges":[],"copies":[]}"#.to_vec(),
    ];
    for bytes in damaged {
        std::fs::write(store.file_path(), &bytes).unwrap();
        assert_eq!(
            store.load(),
            Err(DelegationFileError::Unreadable),
            "a damaged file was loaded: {}",
            String::from_utf8_lossy(&bytes[..80.min(bytes.len())])
        );
        assert_eq!(std::fs::read(store.file_path()).unwrap(), bytes, "a refused load leaves the file as it was");
    }
    std::fs::write(
        store.file_path(),
        br#"{"version":1,"children":[],"notices":[],"followUps":[],"merges":[],"copies":[],"later":"ignored"}"#,
    )
    .unwrap();
    assert_eq!(store.load(), Ok(DelegationFile::empty()), "a key this version does not know is passed over");
    std::fs::remove_file(store.file_path()).unwrap();
    std::fs::create_dir(store.file_path()).unwrap();
    assert_eq!(store.load(), Err(DelegationFileError::Unreadable), "a folder is unreadable");
    std::fs::remove_dir(store.file_path()).unwrap();

    #[cfg(windows)]
    held_open(&store, &folder).await;
    support::place::remove_folder(&folder);
}

/// Windows: another program holding the file open for a moment only delays the replace; one that keeps it fails the save honestly.
#[cfg(windows)]
async fn held_open(store: &DelegationFileStore, folder: &Path) {
    use std::os::windows::fs::OpenOptionsExt as _;
    const FILE_SHARE_READ: u32 = 1;
    let open = |path: PathBuf| std::fs::OpenOptions::new().read(true).share_mode(FILE_SHARE_READ).open(path).unwrap();
    store.save(&sample()).unwrap();
    let path = store.file_path();
    let release = tokio::task::spawn_blocking(move || {
        let held = open(path);
        std::thread::sleep(std::time::Duration::from_millis(400));
        drop(held);
    });
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let refused = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = refused.clone();
    let waiting = DelegationFileStore::new(folder).with_refused(move || {
        counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    });
    waiting.save(&sample().setting_copy(copy_of("c2", DelegationCopyKind::Report, "after the wait", None))).unwrap();
    release.await.unwrap();
    assert_eq!(
        store.load().unwrap().copy("c2", DelegationCopyKind::Report).map(|c| c.text.clone()).as_deref(),
        Some("after the wait"),
        "a save that met a brief reader is written once the reader lets go"
    );
    {
        let _held = open(store.file_path());
        let kept = std::fs::read(store.file_path()).unwrap();
        assert!(
            matches!(DelegationFileStore::with_attempts(folder, 3).save(&sample()), Err(SaveError::Io(_))),
            "a save replaced a file another program holds open"
        );
        assert_eq!(std::fs::read(store.file_path()).unwrap(), kept, "a failed replace leaves the earlier file");
    }
    assert_eq!(entries(folder), ["delegation-state.json", "workspace-state.json"], "a failed save leaves no temporary file");
}

/// "delegation: TASK and REPORT copies are cut at 64 KiB as stored"
#[test]
fn task_and_report_copies_are_cut_at_64_kib_as_stored() {
    let cap = MAXIMUM_COPY_BYTES;
    let marker = TRUNCATION_MARKER;
    assert_eq!(marker, "\n\n[truncated at 64 KiB: the full file stays in the child's worktree until cleanup]\n", "the marker is the Mac's");
    let exact = copy_of("c1", DelegationCopyKind::Report, &"a".repeat(cap), None);
    assert!(!exact.truncated && exact.text.len() == cap && !exact.text.ends_with(marker), "a file of exactly 64 KiB is kept whole");
    let text = "a".repeat(cap + 1);
    let cut = copy_of("c1", DelegationCopyKind::Report, &text, None);
    assert!(
        cut.truncated
            && cut.original_bytes == (cap + 1) as i64
            && cut.text.ends_with(marker)
            && cut.text.len() == cap
            && text.starts_with(&cut.text[..cut.text.len() - marker.len()])
    );

    // Three-byte Korean characters are never split, and are stored as they are, not as escapes.
    let korean = "보고서".repeat(30_000);
    let cut_korean = copy_of("c1", DelegationCopyKind::Report, &korean, None);
    let kept = &cut_korean.text[..cut_korean.text.len() - marker.len()];
    assert!(
        cut_korean.truncated
            && cut_korean.text.len() <= cap
            && cap - cut_korean.text.len() < 3
            && korean.starts_with(kept)
            && kept.chars().all(|c| "보고서".contains(c)),
        "Korean text is cut at a character"
    );
    // Inside the file a copy never takes more than the cap, its escapes included; only the marker's own escapes come on top.
    assert_eq!(copy::stored_bytes("a\"b\\c/d\ne\u{1b}f한"), 1 + 2 + 1 + 2 + 1 + 2 + 1 + 2 + 1 + 6 + 1 + 3);
    let bodies = [
        "\"".repeat(cap),
        "line\n".repeat(cap / 5 + 1),
        "\u{1b}[31mred\u{1b}[0m ".repeat(5_000),
        "\u{1F469}\u{200D}\u{1F4BB} ".repeat(8_000),
    ];
    for body in bodies {
        let copy = copy_of("c1", DelegationCopyKind::Report, &body, None);
        let stored = &copy.text[..copy.text.len() - marker.len()];
        assert!(
            copy.truncated
                && copy::stored_bytes(stored) <= cap - marker.len()
                && body.starts_with(stored)
                && element_bytes(&copy) <= report_copy_room("c1"),
            "a copy with escapes stays within its room in the file"
        );
        // Cut by the room it takes in the file: one more text element would not fit.
        let next =
            body.grapheme_indices(true).map(|(at, element)| at + element.len()).find(|end| *end > stored.len()).unwrap_or(body.len());
        assert!(copy::stored_bytes(&body[..next]) > cap - marker.len(), "a character is never split, and one more would not fit");
    }
    assert!(
        copy::start("\u{1F469}\u{200D}\u{1F4BB}\u{1F469}\u{200D}\u{1F4BB}", 12) == "\u{1F469}\u{200D}\u{1F4BB}"
            && copy::start("abc", 0).is_empty(),
        "the start is whole characters within the budget"
    );

    // Reading a huge report stores only the cut copy, and never loads the file whole.
    let folder = temp_folder("copies");
    let report = folder.join("REPORT.md");
    let body = "line of the report\n".repeat(60_000).into_bytes();
    std::fs::write(&report, &body).unwrap();
    let read = DelegationCopy::read("c1", DelegationCopyKind::Report, 3, &report).unwrap();
    assert!(
        read == DelegationCopy::of("c1", DelegationCopyKind::Report, 3, &body, None)
            && read.truncated
            && read.original_bytes == body.len() as i64
            && read.text.len() <= cap
    );
    std::fs::write(&report, "# 짧은 보고").unwrap();
    let short = DelegationCopy::read("c1", DelegationCopyKind::Report, 4, &report).unwrap();
    assert!(
        !short.truncated
            && short.text == "# 짧은 보고"
            && DelegationCopy::try_read("c1", DelegationCopyKind::Report, 4, &folder.join("missing.md")).is_none()
    );
    support::place::remove_folder(&folder);
}

// Pruning: parent p1 is open, p2 is closed; no merge can be undone.
fn open_context() -> DelegationPruneContext {
    DelegationPruneContext::new(["p1".to_string()].into(), |_| false)
}
const CAP: usize = MAXIMUM_FILE_BYTES;
const FULL_COPY: usize = MAXIMUM_COPY_BYTES;

fn pruned_record(id: &str, state: ChildState, parent: &str) -> ChildRecord {
    ChildRecord::make_with(
        id,
        parent,
        &format!("/tmp/worktrees/{id}"),
        "main",
        "a1",
        "acceptEdits",
        &format!("k-{id}"),
        ChildRecordOptions { state: Some(state), ..Default::default() },
    )
}
fn filler(id: &str, kind: DelegationCopyKind, length: usize) -> DelegationCopy {
    copy_of(id, kind, &"r".repeat(length), None)
}
fn sent(id: &str, lane: DeliveryLane, kind: NoticeKind) -> Notice {
    Notice::new(&format!("n-{id}"), id, 1, kind, lane)
}
fn merged(id: &str) -> MergeRecord {
    MergeRecord::new(id, MergeKind::ToolFastForward, "main", "a1", &format!("m-{id}"), &format!("m-{id}"))
}
fn size(file: &DelegationFile) -> usize {
    store::encode(file).len()
}
fn freeing(bytes: usize, file: &DelegationFile, context: &DelegationPruneContext) -> Option<DelegationFile> {
    file.pruned(context, CAP as i64 - size(file) as i64 + bytes as i64)
}
fn without_reports(file: &DelegationFile, ids: &[&str]) -> DelegationFile {
    DelegationFile {
        copies: file
            .copies
            .iter()
            .filter(|c| !(ids.contains(&c.child_id.as_str()) && c.kind == DelegationCopyKind::Report))
            .cloned()
            .collect(),
        ..file.clone()
    }
}
fn without_records(file: &DelegationFile, ids: &[&str]) -> DelegationFile {
    let keep = |id: &String| !ids.contains(&id.as_str());
    DelegationFile {
        version: file.version,
        children: file.children.iter().filter(|c| keep(&c.id)).cloned().collect(),
        notices: file.notices.iter().filter(|n| keep(&n.child_id)).cloned().collect(),
        follow_ups: file.follow_ups.iter().filter(|f| keep(&f.child_id)).cloned().collect(),
        merges: file.merges.iter().filter(|m| keep(&m.child_id)).cloned().collect(),
        copies: file.copies.iter().filter(|c| keep(&c.child_id)).cloned().collect(),
    }
}

/// `start` plus open filler children whose report copies leave exactly `room` bytes under the cap (a negative room passes it).
fn filled(start: DelegationFile, room: i64) -> DelegationFile {
    let mut file = start;
    let mut index = 0;
    let mut add = |file: &mut DelegationFile, length: usize| {
        let id = format!("fill{index:03}");
        index += 1;
        file.children.push(pruned_record(&id, ChildState::Running, "p1"));
        *file = file.setting_copy(filler(&id, DelegationCopyKind::Report, length));
    };
    let whole = (CAP as i64 - room - size(&file) as i64) / (FULL_COPY as i64 + 1_000) - 3;
    for _ in 0..whole {
        add(&mut file, FULL_COPY);
    }
    loop {
        add(&mut file, 10_000);
        let length = 10_000 + CAP as i64 - room - size(&file) as i64;
        assert!(length >= 10_000, "the filler has room to grow");
        let last = file.children.last().unwrap().id.clone();
        if length <= FULL_COPY as i64 {
            return file.setting_copy(filler(&last, DelegationCopyKind::Report, length as usize));
        }
        file = file.setting_copy(filler(&last, DelegationCopyKind::Report, (FULL_COPY as i64).min(length - 30_000) as usize));
    }
}

/// "delegation: pruning goes oldest first and keeps what it must"
#[test]
fn pruning_goes_oldest_first_and_keeps_what_it_must() {
    use ChildState as S;
    use DelegationCopyKind::{Report, Task};
    let open = open_context();
    let small = DelegationFile::of(
        vec![pruned_record("c1", S::Closed, "p1")],
        vec![sent("c1", DeliveryLane::Delivered, NoticeKind::Reported)],
        vec![],
        vec![],
        vec![filler("c1", Report, 1_000)],
    );
    assert!(
        small.pruned(&open, 0).as_ref() == Some(&small) && freeing(0, &small, &open).as_ref() == Some(&small),
        "a file that fits comes back unchanged"
    );

    let file = DelegationFile::of(
        vec![
            pruned_record("c1", S::Closed, "p1"),
            pruned_record("c2", S::Running, "p1"),
            pruned_record("c3", S::Discarded, "p1"),
            pruned_record("c4", S::Closed, "p1"),
        ],
        vec![
            sent("c1", DeliveryLane::Delivered, NoticeKind::Reported),
            sent("c3", DeliveryLane::Delivered, NoticeKind::Reported),
            sent("c4", DeliveryLane::Delivered, NoticeKind::Reported),
        ],
        vec![FollowUp::new("f1", "c1", "테스트도 추가해 줘", DeliveryLane::Delivered)],
        vec![merged("c1")],
        vec![
            filler("c1", Task, 1_000),
            filler("c1", Report, 1_000),
            filler("c2", Report, 1_000),
            filler("c3", Report, 1_000),
            filler("c4", Report, 1_000),
        ],
    );
    let report_bytes: Vec<usize> = ["c1", "c3", "c4"].iter().map(|id| element_bytes(&filler(id, Report, 1_000))).collect();
    assert!(
        freeing(1, &file, &open) == Some(without_reports(&file, &["c1"]))
            && freeing(report_bytes[0] + 1, &file, &open) == Some(without_reports(&file, &["c1", "c3"])),
        "report copies of the oldest closed records go first"
    );
    let oldest_gone = freeing(report_bytes.iter().sum::<usize>() + 1, &file, &open).unwrap();
    assert!(
        oldest_gone == without_records(&without_reports(&file, &["c1", "c3", "c4"]), &["c1"])
            && oldest_gone.copy("c2", Report) == Some(&filler("c2", Report, 1_000)),
        "then the oldest delivered record goes whole"
    );
    assert_eq!(oldest_gone.children.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["c2", "c3", "c4"], "the others stay");
    let first_record_bytes = element_bytes(&file.children[0])
        + element_bytes(&file.notices[0])
        + element_bytes(&file.follow_ups[0])
        + element_bytes(&file.merges[0])
        + element_bytes(&file.copies[0]);
    let next = freeing(report_bytes.iter().sum::<usize>() + first_record_bytes + 1, &file, &open).unwrap();
    assert_eq!(next.children.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["c2", "c4"], "the next oldest record goes next");

    // Open children, failed children, held items of open panes and undoable merges are never pruned.
    let guarded = DelegationFile::of(
        vec![
            pruned_record("open", S::Running, "p1"),
            pruned_record("waiting", S::Waiting, "p1"),
            pruned_record("failed", S::Failed, "p1"),
            pruned_record("heldOpenParent", S::Closed, "p1"),
            pruned_record("heldClosedParent", S::Closed, "p2"),
            pruned_record("pendingNotice", S::Closed, "p1"),
            pruned_record("heldFollowUp", S::Closed, "p1"),
            pruned_record("undoable", S::Closed, "p1"),
            pruned_record("merged", S::Closed, "p1"),
        ],
        vec![
            sent("open", DeliveryLane::Held, NoticeKind::Reported),
            sent("failed", DeliveryLane::Delivered, NoticeKind::FailedToStart),
            sent("heldOpenParent", DeliveryLane::Held, NoticeKind::Reported),
            sent("heldClosedParent", DeliveryLane::Held, NoticeKind::Reported),
            sent("pendingNotice", DeliveryLane::Pending, NoticeKind::Reported),
            sent("undoable", DeliveryLane::Delivered, NoticeKind::Reported),
            sent("merged", DeliveryLane::Delivered, NoticeKind::Reported),
        ],
        vec![
            FollowUp::new("f-open", "open", "one more", DeliveryLane::Held),
            FollowUp::new("f-held", "heldFollowUp", "finish it", DeliveryLane::Held),
        ],
        vec![merged("undoable"), merged("merged")],
        vec![
            filler("open", Task, 1_000),
            filler("open", Report, 1_000),
            filler("waiting", Report, 1_000),
            filler("failed", Task, 1_000),
            filler("heldOpenParent", Report, 1_000),
            filler("heldClosedParent", Report, 1_000),
            filler("pendingNotice", Report, 1_000),
            filler("heldFollowUp", Report, 1_000),
            filler("undoable", Report, 1_000),
            filler("merged", Report, 1_000),
        ],
    );
    let can_undo = DelegationPruneContext::new(["p1", "open", "waiting"].map(String::from).into(), |m| m.merged_commit == "m-undoable");
    let expected = without_records(
        &without_reports(&guarded, &["heldOpenParent", "heldClosedParent", "pendingNotice", "heldFollowUp", "undoable", "merged"]),
        &["heldClosedParent", "merged"],
    );
    assert_eq!(
        guarded.pruned(&can_undo, (CAP - size(&expected)) as i64),
        Some(expected.clone()),
        "everything allowed is pruned, and nothing else"
    );
    assert_eq!(
        expected.children.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
        ["open", "waiting", "failed", "heldOpenParent", "pendingNotice", "heldFollowUp", "undoable"]
    );
    assert!(
        expected.merges == [merged("undoable")]
            && expected.follow_ups.iter().map(|f| f.id.as_str()).collect::<Vec<_>>() == ["f-open", "f-held"]
            && expected.copy("open", Report).is_some()
            && expected.copy("waiting", Report).is_some()
            && expected.copy("failed", Task).is_some(),
        "an undoable merge, held follow-ups and open children's copies stay"
    );
    assert!(guarded.pruned(&can_undo, (CAP - size(&expected) + 1) as i64).is_none(), "needing one byte more than that is not possible");
    // With p1 closed too, the notice held for its closed child counts as delivered.
    let parent_closed = DelegationPruneContext::new(["open", "waiting"].map(String::from).into(), |m| m.merged_commit == "m-undoable");
    let more = without_records(&expected, &["heldOpenParent"]);
    assert_eq!(
        guarded.pruned(&parent_closed, (CAP - size(&more)) as i64),
        Some(more),
        "a notice held for a closed parent's child counts as delivered"
    );
}

/// "delegation: a new child needs room, and saving near the cap prunes"
#[test]
fn a_new_child_needs_room_and_saving_near_the_cap_prunes() {
    use DelegationCopyKind::{Report, Task};
    let open = open_context();
    let new_child = pruned_record("new", ChildState::Creating, "p1");
    let task = filler("new", Task, 200);
    // Only open children: nothing can be pruned, so there is no room for a record and a full report copy.
    let open_only = filled(DelegationFile::empty(), 100);
    assert!(
        size(&open_only) == CAP - 100
            && open_only.admitting(new_child.clone(), &open, Some(task.clone()))
                == DelegationAdmission::Refused(DelegationReasonCode::StoreFull),
        "a full file of open children refuses a new child with store_full"
    );
    // A closed record whose notice is still held for its open parent is protected too.
    let old_copies = vec![filler("old", Task, FULL_COPY), filler("old", Report, FULL_COPY)];
    let held = filled(
        DelegationFile::of(
            vec![pruned_record("old", ChildState::Closed, "p1")],
            vec![sent("old", DeliveryLane::Held, NoticeKind::Reported)],
            vec![],
            vec![],
            old_copies.clone(),
        ),
        100,
    );
    assert_eq!(
        held.admitting(new_child.clone(), &open, Some(task.clone())),
        DelegationAdmission::Refused(DelegationReasonCode::StoreFull),
        "a held notice of an open parent protects its record"
    );
    // Pruning makes room for a new child when it can.
    let roomy = filled(
        DelegationFile::of(
            vec![pruned_record("old", ChildState::Closed, "p1")],
            vec![sent("old", DeliveryLane::Delivered, NoticeKind::Reported)],
            vec![],
            vec![],
            old_copies,
        ),
        100,
    );
    let DelegationAdmission::Admitted(admitted) = roomy.admitting(new_child.clone(), &open, Some(task.clone())) else {
        panic!("the closed, delivered record makes room")
    };
    assert!(
        *admitted.children.last().unwrap() == new_child
            && admitted.copy("new", Task) == Some(&task)
            && admitted.children.iter().all(|c| c.id != "old")
            && admitted.copies.iter().all(|c| c.child_id != "old")
            && admitted.children.len() == roomy.children.len()
            && size(&admitted) + report_copy_room("new") <= CAP,
        "the closed, delivered record makes room"
    );

    let folder = temp_folder("room");
    let store = DelegationFileStore::new(folder.join("profile"));
    // Less than one more child's room left: the closed record's report copy goes.
    let near = filled(
        DelegationFile::of(
            vec![pruned_record("old", ChildState::Closed, "p1")],
            vec![sent("old", DeliveryLane::Delivered, NoticeKind::Reported)],
            vec![],
            vec![],
            vec![filler("old", Task, 1_000), filler("old", Report, FULL_COPY)],
        ),
        10_000,
    );
    let saved = store.save_pruned(&near, &open).unwrap();
    assert!(
        saved == without_reports(&near, &["old"]) && store.load() == Ok(saved.clone()) && size(&saved) + new_child_room() <= CAP,
        "a save near the cap prunes to one more child's room"
    );
    // A file that fits but has nothing to prune is saved as it is; one over the cap with nothing to prune is refused, writing nothing.
    let open_file = filled(DelegationFile::empty(), 10);
    assert_eq!(store.save_pruned(&open_file, &open).unwrap(), open_file, "a file with nothing to prune is saved whole");
    let before = std::fs::read(store.file_path()).unwrap();
    assert!(store.save_pruned(&filled(DelegationFile::empty(), -10), &open).is_err_and(|e| e.is_full()), "a file over the cap is refused");
    assert_eq!(std::fs::read(store.file_path()).unwrap(), before, "a refused save writes nothing");
    support::place::remove_folder(&folder);
}

/// "delegation: the switch, the gates and the six tools" (the switch's registry value is src-tauri's, W6: settings/general.rs)
#[tokio::test]
async fn the_switch_the_gates_and_the_six_tools() {
    assert!(
        providers::is_claude_pane("claude", "claude")
            && !providers::is_claude_pane("claude", "codex")
            && !providers::is_claude_pane("claude", "gemini")
            && !providers::is_claude_pane("shell", "claude")
            && !providers::is_claude_pane("browser", "claude")
    );

    // With no coordinator behind it: off reaches nothing, a pane whose agent has no delegation tools is refused, a run without the
    // server stays detached.
    let delegate = request("delegate", &[("task", "Write the release notes"), ("mode", "plan")]);
    let on = DelegationIoHandler::new(|| true);
    let off = DelegationIoHandler::new(|| false);
    let unknown = DelegationResponse::failure("Unknown delegation tool delegate.");
    assert_eq!(
        on.handle(delegate.clone(), caller("codex", "claude", "codex", true)).await,
        unknown,
        "a Codex pane with the server attached reaches the tools"
    );
    assert_eq!(on.handle(delegate.clone(), caller("codex", "claude", "codex", false)).await, DelegationResponse::failure(DETACHED_MESSAGE));
    for who in [caller("gemini", "claude", "gemini", true), caller("shell", "shell", "claude", true)] {
        for tool in ["delegate", "list_children", "child_status", "merge", "follow_up", "discard", "no_such_tool"] {
            let call = if tool == "delegate" { delegate.clone() } else { request(tool, &[("child", "child-1")]) };
            assert_eq!(
                on.handle(call, who.clone()).await,
                DelegationResponse::refusal(DelegationReasonCode::ClaudeOnly),
                "{} {tool} is claude_only",
                who.pane_id
            );
        }
    }
    assert_eq!(on.handle(delegate.clone(), caller("claude", "claude", "claude", true)).await, unknown);
    for who in [caller("codex", "claude", "codex", true), caller("claude", "claude", "claude", true)] {
        assert_eq!(
            off.handle(delegate.clone(), who).await,
            DelegationResponse::failure(DETACHED_MESSAGE),
            "switched off, every call reaches nothing"
        );
    }
    assert_eq!(
        on.handle(delegate.clone(), caller("claude", "claude", "claude", false)).await,
        DelegationResponse::failure(DETACHED_MESSAGE)
    );
    assert_eq!(DETACHED_MESSAGE, "The delegation tools are not attached to this pane's run.");
    assert!(
        DelegationRequest::is_tool_name("child_status")
            && DelegationRequest::is_tool_name("a1")
            && !DelegationRequest::is_tool_name("Delegate")
            && !DelegationRequest::is_tool_name("1a")
            && !DelegationRequest::is_tool_name("")
            && !DelegationRequest::is_tool_name("a-b")
            && !DelegationRequest::is_tool_name(&"a".repeat(65))
            && DelegationRequest::is_tool_name(&"a".repeat(64))
    );

    // The six tools, their arguments and their words.
    let six = ["delegate", "list_children", "child_status", "merge", "follow_up", "discard"];
    assert_eq!(tools::ALL.iter().map(|t| t.name).collect::<Vec<_>>(), six, "exactly the six tools in order");
    assert!(
        six.iter().all(|name| !["run_in_terminal", "read_latest_output", "stop", "open_url"].contains(name)),
        "none of them is a mighty-terminal tool"
    );
    let arguments: [&[&str]; 6] =
        [&["task", "mode", "agent"], &[], &["child"], &["child", "expected_head"], &["child", "text"], &["child"]];
    for (tool, expected) in tools::ALL.iter().zip(arguments) {
        assert_eq!(tool.arguments.iter().map(|a| a.name).collect::<Vec<_>>(), expected, "{} takes its arguments", tool.name);
    }
    assert!(tools::DELEGATE.description.starts_with("Start a child pane that works on a task in its own git worktree, on the flat branch mighty/<child id> made from your current branch head. It runs your own agent CLI unless agent names another: claude, codex or antigravity, each only if installed and signed in on this PC."));
    assert!(tools::DELEGATE.description.ends_with("At most 3 open children per pane, of all agents together, and a child cannot delegate. The same call twice in one of your runs returns the same child."));
    assert_eq!(
        tools::DISCARD.description,
        "Discarding a child is for the human only, from the child's card in the app. This tool always refuses with discard_human_only and changes nothing."
    );
    assert_eq!(tools::MERGE.arguments[1].description, "The child's reported head commit, as child_status gave it.");
    const {
        assert!(
            MAXIMUM_OPEN_CHILDREN == 3
                && MAXIMUM_DEPTH == 1
                && MAXIMUM_FOLLOW_UPS == 2
                && MAXIMUM_FOLLOW_UP_BYTES == 48 * 1024
                && MINIMUM_FREE_BYTES == 10_000_000_000
                && ANSWER_SECONDS == 45.0
                && DELIVERY_OFFERS == 2,
            "the limits are the Mac's"
        );
    };
    let names = |pairs: &[&str]| pairs.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert_eq!(
        coordinator::argument_problem(names(&["child"]).iter(), &tools::MERGE).as_deref(),
        Some("merge takes exactly these string arguments: child, expected_head.")
    );
    assert_eq!(
        coordinator::argument_problem(names(&["child"]).iter(), &tools::LIST_CHILDREN).as_deref(),
        Some("list_children takes no arguments.")
    );
    assert_eq!(
        coordinator::argument_problem(names(&["text", "child"]).iter(), &tools::FOLLOW_UP),
        None,
        "a call needs exactly its tool's arguments"
    );
    assert_eq!(
        coordinator::argument_problem(names(&["task"]).iter(), &tools::DELEGATE).as_deref(),
        Some("delegate takes exactly these string arguments: task, mode, and optionally agent.")
    );

    // A tool still working at the limit is answered once, and its work goes on.
    let gate = support::host::Gate::new();
    let finished = support::host::Gate::new();
    let late = DelegationResponse::failure("late");
    let (waiting, done) = (gate.clone(), finished.clone());
    let answer = coordinator::answer(0.2, late.clone(), async move {
        waiting.wait().await;
        done.open();
        DelegationResponse::refusal(DelegationReasonCode::StoreFull)
    })
    .await;
    assert_eq!(answer, late, "the late answer comes at the limit");
    gate.open();
    tokio::time::timeout(std::time::Duration::from_secs(30), finished.wait()).await.expect("the work goes on");
    assert_eq!(
        coordinator::answer(30.0, late, async { DelegationResponse::refusal(DelegationReasonCode::NotGit) }).await,
        DelegationResponse::refusal(DelegationReasonCode::NotGit)
    );
    assert_eq!(
        coordinator::late_message("merge", ANSWER_SECONDS),
        "merge did not finish within 45 seconds. Its work goes on in Mighty Code; call list_children to see where your children stand."
    );
    let panicked = coordinator::answer(30.0, DelegationResponse::failure("late"), async { panic!("a tool that fails") }).await;
    assert_eq!(panicked, DelegationResponse::failure(coordinator::NO_ANSWER_MESSAGE), "work that failed is answered once");
}

fn row(id: &str, parent: &str, state: ChildState, checkout: Option<&str>) -> DelegationChildRow {
    DelegationChildRow {
        id: id.into(),
        parent_session_id: parent.into(),
        state,
        task: None,
        parent_branch: "main".into(),
        parent_checkout: checkout.map(str::to_string),
        can_undo: false,
        held_notices: 0,
        started_by_human: false,
        agent: "claude".into(),
    }
}

/// "delegation: a queued merge waits for the report and an idle, clean parent"
#[test]
fn a_queued_merge_waits_for_the_report_and_an_idle_clean_parent() {
    use ChildState as S;
    for working in [S::Running, S::Waiting] {
        assert_eq!(
            row("a", "p", working, None).card_actions(),
            [CardAction::Merge, CardAction::Discard],
            "a {working:?} child's card offers the merge that waits"
        );
    }
    assert_eq!(row("a", "p", S::Ended, None).card_actions(), [CardAction::Discard], "an ended child's card");
    for working in [S::Creating, S::Running, S::Waiting] {
        for parent_running in [false, true] {
            assert_eq!(
                merge_step(working, parent_running),
                DelegationMergeStep::Wait(DelegationMergeWait::ChildRunning),
                "{working:?} waits for its report"
            );
        }
    }
    assert_eq!(merge_step(S::Reported, false), DelegationMergeStep::Attempt, "a reported child is merged");
    assert_eq!(
        merge_step(S::Reported, true),
        DelegationMergeStep::Wait(DelegationMergeWait::ParentRunning),
        "a running parent is waited for"
    );
    for over in [S::Ended, S::Interrupted, S::Merged, S::Failed, S::Closed, S::Discarded] {
        assert_eq!(merge_step(over, false), DelegationMergeStep::Drop, "{over:?} leaves the queue");
    }
    assert_eq!(wait_after(DelegationReasonCode::TrackedChanges), Some(DelegationMergeWait::ParentChanges), "uncommitted changes pass");
    assert_eq!(wait_after(DelegationReasonCode::ParentBusy), Some(DelegationMergeWait::ParentRunning), "a busy parent passes");
    for reason in
        DelegationReasonCode::ALL.iter().filter(|r| !matches!(r, DelegationReasonCode::TrackedChanges | DelegationReasonCode::ParentBusy))
    {
        assert_eq!(wait_after(*reason), None, "{reason:?} is for good");
    }
}

fn pane(id: &str, workspace: &str, parent: Option<&str>) -> RunSession {
    RunSession {
        id: Some(id.into()),
        workspace_id: Some(workspace.into()),
        title: Some(id.into()),
        parent_session_id: parent.map(str::to_string),
        ..Default::default()
    }
}

fn workspace(id: &str, name: &str, path: &str) -> Workspace {
    Workspace { id: Some(id.into()), name: Some(name.into()), path: Some(path.into()), ..Default::default() }
}

fn snapshot(workspaces: &[Workspace], sessions: Vec<RunSession>) -> AppSnapshot {
    AppSnapshot {
        workspaces: Some(workspaces.to_vec()),
        sessions: Some(sessions.into_iter().map(Arc::new).collect()),
        ..Default::default()
    }
}

/// "delegation: the sidebar tree, rows and card actions"
#[test]
fn the_sidebar_tree_rows_and_card_actions() {
    use ChildState as S;
    let windows = mighty_run::os::windows();
    let path = |windows_path: &str, other: &str| if windows { windows_path.to_string() } else { other.to_string() };
    let workspaces = [
        workspace("w1", "repo", &path(r"C:\repo", "/repo")),
        workspace("w2", "sub", &path(r"C:\repo\sub", "/repo/sub")),
        workspace("w3", "app", &path(r"C:\other\app", "/other/app")),
    ];
    // A parent's close counts its open children and a child pane whose row has not arrived.
    let family = [pane("p1", "w1", None), pane("c1", "w1", Some("p1")), pane("c3", "w1", Some("p1")), pane("q1", "w1", Some("q"))];
    let family_refs: Vec<&RunSession> = family.iter().collect();
    let known = [row("c1", "p1", S::Running, None), row("c2", "p1", S::Failed, None), row("x", "q", S::Reported, None)];
    let without_c1: Vec<DelegationChildRow> = known.iter().filter(|r| r.id != "c1").cloned().collect();
    let only_parent = [pane("p1", "w1", None)];
    assert!(
        sidebar::open_child_count("p1", &known, &family_refs) == 2
            && sidebar::open_child_count("p1", &[], &family_refs) == 2
            && sidebar::open_child_count("p1", &without_c1, &only_parent.iter().collect::<Vec<_>>()) == 0,
        "a parent's close counts its open children"
    );

    // Children are listed under their open parent in the order they were made; a pane with no record stays at the top.
    let state = snapshot(
        &workspaces,
        vec![
            pane("p1", "w1", None),
            pane("c1", "w1", Some("p1")),
            pane("x", "w1", None),
            pane("stray", "w1", Some("p1")),
            pane("q", "w2", None),
        ],
    );
    let children = [row("c2", "p1", S::Failed, None), row("c1", "p1", S::Reported, None)];
    let tree = delegation_tree("w1", &state, &children);
    assert_eq!(tree.top.iter().map(|p| p.id()).collect::<Vec<_>>(), ["p1", "x", "stray"], "a child pane is listed once, under its parent");
    assert!(
        tree.children["p1"].iter().map(|r| r.id.as_str()).eq(["c2", "c1"]) && tree.parent_closed.is_empty(),
        "children keep the order they were made in"
    );
    let other = delegation_tree("w2", &state, &children);
    assert!(
        other.top.iter().map(|p| p.id()).eq(["q"]) && other.children.is_empty() && other.parent_closed.is_empty(),
        "another workspace lists none of them"
    );

    // Children of a closed parent are listed under a 'parent closed' node. Folders compare without regard to case or separator.
    if windows {
        let open = snapshot(&workspaces, vec![pane("p1", "w1", None), pane("c3", "w2", Some("gone"))]);
        let orphans = [
            row("c3", "gone", S::Running, Some(r"C:\repo")),
            row("c4", "gone", S::Closed, Some(r"C:\repo")),
            row("c5", "gone2", S::Failed, Some("c:/REPO/")),
            row("c6", "gone3", S::Ended, Some(r"C:\other")),
            row("c7", "gone4", S::Merged, None),
            row("c8", "gone", S::Interrupted, Some(r"C:\repo")),
            row("c9", "gone5", S::Closed, Some(r"C:\rep")),
        ];
        let (w1, w2, w3) =
            (delegation_tree("w1", &open, &orphans), delegation_tree("w2", &open, &orphans), delegation_tree("w3", &open, &orphans));
        let node_ids = |tree: &mighty_core::shell::DelegationTree| tree.parent_closed.iter().map(|n| n.0.clone()).collect::<Vec<_>>();
        assert!(w1.top.iter().map(|p| p.id()).eq(["p1"]) && w1.children.is_empty() && node_ids(&w1) == ["gone", "gone2"]);
        assert!(
            w1.parent_closed[0].2.iter().map(|r| r.id.as_str()).eq(["c4", "c8"]) && w1.parent_closed[0].1 == "main",
            "a closed parent's children are listed at its checkout's workspace"
        );
        assert!(
            w2.top.is_empty()
                && node_ids(&w2) == ["gone"]
                && w2.parent_closed[0].2.iter().map(|r| r.id.as_str()).eq(["c3"])
                && node_ids(&w3) == ["gone3"],
            "a child with its own pane is listed in that pane's workspace"
        );
        let mut placed: Vec<String> =
            [&w1, &w2, &w3].iter().flat_map(|t| t.parent_closed.iter().flat_map(|n| n.2.iter().map(|r| r.id.clone()))).collect();
        placed.sort();
        assert_eq!(placed, ["c3", "c4", "c5", "c6", "c8"], "each placed child is listed once");
    }

    // A row shows its state, with waiting while a running child asks a human.
    assert!(
        sidebar::shown_state(S::Running, true) == S::Waiting
            && sidebar::shown_state(S::Running, false) == S::Running
            && sidebar::shown_state(S::Waiting, false) == S::Waiting
            && sidebar::shown_state(S::Reported, true) == S::Reported
    );
    for (state, tone) in [
        (S::Creating, Tone::Run),
        (S::Running, Tone::Run),
        (S::Waiting, Tone::Wait),
        (S::Reported, Tone::Done),
        (S::Merged, Tone::Done),
        (S::Ended, Tone::Idle),
        (S::Interrupted, Tone::Stop),
        (S::Failed, Tone::Err),
        (S::Closed, Tone::Idle),
    ] {
        assert_eq!(sidebar::tone(state), tone, "{state:?} tone");
    }
    for state in ChildState::ALL {
        let key = format!("delegation.child.state.{}", state.name());
        assert!(
            mighty_core::locale::catalogue("ko").contains_key(&key) && mighty_core::locale::catalogue("en").contains_key(&key),
            "{state:?} has its word in the catalogues"
        );
    }

    // Rows leave out discarded children and carry the task in one line; a row counts only its child's held notices.
    let listed = |id: &str, state: S| {
        ChildRecord::make_with(
            id,
            "p",
            &format!(r"C:\worktrees\{id}"),
            "main",
            "base",
            "auto",
            &format!("key-{id}"),
            ChildRecordOptions { state: Some(state), parent_checkout: Some(r"C:\repo".into()), ..Default::default() },
        )
    };
    let file = DelegationFile::of(
        vec![listed("a", S::Reported), listed("b", S::Discarded), listed("c", S::Failed)],
        vec![],
        vec![],
        vec![],
        vec![copy_of("a", DelegationCopyKind::Task, "Fix the login\n  bug", None), copy_of("a", DelegationCopyKind::Report, "done", None)],
    );
    let rows = sidebar::rows(&file);
    let expected = DelegationChildRow {
        task: Some("Fix the login bug".into()),
        parent_checkout: Some(r"C:\repo".into()),
        ..row("a", "p", S::Reported, None)
    };
    assert!(
        rows.iter().map(|r| r.id.as_str()).eq(["a", "c"]) && rows[0] == expected && rows[1].task.is_none(),
        "rows leave out discarded children and shorten the task"
    );
    let receipt = DeliveryReceipt::new("2026-10-10T01:00:00Z", DeliveryRoute::Queue, "run-1");
    let counted = DelegationFile::of(
        vec![listed("a", S::Ended), listed("b", S::Reported)],
        vec![
            Notice::new("n1", "a", 0, NoticeKind::EndedWithoutReport, DeliveryLane::Held),
            Notice::new("n2", "a", 1, NoticeKind::Reported, DeliveryLane::Delivered).with_receipt(receipt.clone()),
            Notice::new("n3", "a", 1, NoticeKind::Reported, DeliveryLane::Pending),
            Notice::new("n4", "a", 2, NoticeKind::Reported, DeliveryLane::Held),
            Notice::new("n5", "b", 1, NoticeKind::Reported, DeliveryLane::Delivered).with_receipt(receipt),
        ],
        vec![],
        vec![],
        vec![],
    );
    assert_eq!(
        sidebar::rows(&counted).iter().map(|r| r.held_notices).collect::<Vec<_>>(),
        [2, 0],
        "a row counts only its child's held notices"
    );

    // The card's actions: merge while reported, undo while the last merge can be undone, discard until discarded.
    let with_undo = |state: S| DelegationChildRow { can_undo: true, ..row("a", "p", state, None) };
    assert_eq!(row("a", "p", S::Reported, None).card_actions(), [CardAction::Merge, CardAction::Discard]);
    assert_eq!(with_undo(S::Merged).card_actions(), [CardAction::Undo, CardAction::Discard]);
    assert_eq!(with_undo(S::Closed).card_actions(), [CardAction::Undo, CardAction::Discard], "a closed child's card keeps undo");
    assert_eq!(row("a", "p", S::Closed, None).card_actions(), [CardAction::Discard]);
    assert_eq!(row("a", "p", S::Failed, None).card_actions(), [CardAction::Discard]);
    assert!(row("a", "p", S::Discarded, None).card_actions().is_empty(), "a discarded child has no actions");
    // The merge a card can undo: the child's last one while merged, or closed at that merge's head.
    let (b, c) = ("b".repeat(40), "c".repeat(40));
    let first = MergeRecord::new("a", MergeKind::CardFastForward, "main", COMMIT, &b, &b);
    let last = MergeRecord { pre_merge_commit: b.clone(), merged_commit: c.clone(), child_head: "C".repeat(40), ..first.clone() };
    let with = |state: S, closed_head: Option<&str>| {
        DelegationFile::of(
            vec![ChildRecord { closed_head: closed_head.map(str::to_string), ..listed("a", state) }],
            vec![],
            vec![],
            vec![first.clone(), last.clone()],
            vec![],
        )
    };
    assert!(
        with(S::Merged, None).undoable_merge("a") == Some(&last)
            && with(S::Closed, Some(&c)).undoable_merge("a") == Some(&last)
            && with(S::Closed, Some(&b)).undoable_merge("a").is_none()
            && with(S::Closed, None).undoable_merge("a").is_none()
            && with(S::Reported, None).undoable_merge("a").is_none()
            && with(S::Merged, None).undoable_merge("z").is_none(),
        "only the last merge comes back to undo"
    );
    assert!(
        sidebar::rows(&with(S::Merged, None))[0].can_undo && !sidebar::rows(&with(S::Reported, None))[0].can_undo,
        "the row says whether its card can undo"
    );
}

/// The repository's delegation-record profiles (scripts/fixtures/delegation-record/*/profile/delegation-state.json) load, and what a
/// save writes is every member of the file as C#'s encoder writes it (the fixtures are indented and not all key-sorted, so the
/// comparison is with the fixture's own JSON written by `DelegationJson`'s rules); byte parity with C# itself is `tests/parity.rs`.
#[test]
fn the_delegation_record_files_round_trip_byte_for_byte() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../../scripts/fixtures/delegation-record");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&root)
        .unwrap()
        .map(|e| e.unwrap().path().join("profile").join(store::FILE_NAME))
        .filter(|p| p.exists())
        .collect();
    files.sort();
    assert_eq!(files.len(), 9, "the nine delegation-record profiles");
    for path in files {
        let bytes = std::fs::read(&path).unwrap();
        let file = store::decode(&bytes).unwrap_or_else(|| panic!("{} does not load", path.display()));
        let written = store::encode(&file);
        let fixture: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            String::from_utf8(written.clone()).unwrap(),
            String::from_utf8(mighty_delegation::json::encode(&fixture)).unwrap(),
            "{}",
            path.display()
        );
        assert_eq!(store::encode(&store::decode(&written).unwrap()), written, "{}: a second save writes the same bytes", path.display());
    }
}
