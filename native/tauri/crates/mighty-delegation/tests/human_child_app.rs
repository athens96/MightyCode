//! W7-D's DelegationHumanChildVerification checks under their C# names: "delegation: a card merge that conflicts asks the child once per
//! report" (L687: what the desktop service's merge queue asks a conflicting child, `ConflictResolution`) and "delegation: a human's child
//! whose start throws gives its text back once" (L693, read from the delegate tool's source as C# reads its own). The fixture is
//! C#'s `Make(parentMode: "fullAccess")`: a place with one commit, a parent pane running Claude, a coordinator over it.

mod support;

use std::path::Path;
#[cfg(windows)]
use std::sync::Arc;

#[cfg(windows)]
use mighty_delegation::app_host::ChildRunEnd;
#[cfg(windows)]
use mighty_delegation::git::merge::conflict_prompt;
#[cfg(windows)]
use mighty_delegation::git::worktree::report_file_in;
#[cfg(windows)]
use mighty_delegation::host::{DelegationPaneActivity, DelegationPaneState};
#[cfg(windows)]
use mighty_delegation::model::ChildState;
#[cfg(windows)]
use mighty_delegation::store::{DelegationFile, DelegationFileStore};
#[cfg(windows)]
use mighty_delegation::tools::delegate::{HumanChildFirstRun, HumanChildStart, HumanChildStarter};
#[cfg(windows)]
use support::host::{self, TestHost, eventually};
#[cfg(windows)]
use support::{lane, place::DelegationPlace};

/// The git half: the process host that runs git is Windows' until the Mac stage.
#[cfg(windows)]
mod git {
    use super::*;

    /// L687.
    #[tokio::test]
    async fn a_card_merge_that_conflicts_asks_the_child_once_per_report() {
        Box::pin(conflict_once()).await;
    }

    async fn conflict_once() {
        let _lane = lane::enter().await;
        let place = DelegationPlace::new();
        let store = DelegationFileStore::new(place.profile());
        store.save(&DelegationFile::default()).unwrap();
        let test_host = TestHost::new(Some(place.profile().into()), "run");
        test_host.set_makes_panes(true);
        place.repository(1, "", "false").await;
        test_host.set(DelegationPaneState {
            session_id: "parent".into(),
            permission_mode: "fullAccess".into(),
            folder: place.repo(),
            kind: "claude".into(),
            provider: "claude".into(),
            parent_session_id: None,
            run_id: Some("run-1".into()),
            activity: DelegationPaneActivity::Running,
        });
        let coordinator =
            host::coordinator(DelegationFileStore::new(place.profile()), test_host.clone(), host::maker(&place, 50_000_000_000), || true)
                .expect("the file loads");
        let starter = HumanChildStarter {
            start: Arc::new(|id: String, _run: HumanChildFirstRun| Box::pin(async move { Some(format!("send-run-of-{id}")) })),
            failed: Arc::new(|_id: String| Box::pin(async {})),
        };
        let HumanChildStart::Started(child) = coordinator.start_human_child("Change file-0.", "parent", starter, false, None).await else {
            panic!("the send made no child")
        };
        let running = eventually(60_000, || {
            coordinator.file().children.iter().any(|c| c.id == child.id && c.state == ChildState::Running)
                && !coordinator.is_starting(&child.id)
        })
        .await;
        assert!(running, "the child runs");
        assert!(coordinator.conflict_resolution(&child.id).is_none(), "a child that has not reported is not asked");
        std::fs::write(Path::new(&child.worktree_path).join("file-0.txt"), "ours\n").unwrap();
        place.git_in(&child.worktree_path, &["commit", "-q", "-am", "ours"]).await;
        std::fs::write(report_file_in(&child.worktree_path), "# Report\n").unwrap();
        coordinator.child_run_ended(&child.id, &format!("send-run-of-{}", child.id), ChildRunEnd::Finished).await;
        let reported = coordinator.file().children.iter().find(|c| c.id == child.id).cloned().unwrap();
        let prompt = conflict_prompt(&reported.git());
        assert!(
            reported.state == ChildState::Reported
                && coordinator.conflict_resolution(&child.id).as_deref() == Some(prompt.as_str())
                && coordinator.conflict_resolution(&child.id).is_none(),
            "asked once per report"
        );
        assert!(
            prompt.starts_with(&format!(
                "[Mighty Code] Merging your branch {} into main from the app was refused: the two conflict.",
                reported.branch
            )) && prompt.contains(&report_file_in(&reported.worktree_path)),
            "the request names the branches and the report"
        );
        coordinator.conflict_resolution_not_started(&child.id);
        assert!(coordinator.conflict_resolution(&child.id).is_some(), "a request that did not start is asked again");
    }
}

/// L693: a human's child whose start fails outside its pane's start (its worktree, the file's save, a notice's delivery) still gives
/// its text back, and only once. Nothing here can make those fail, so it is read from the delegate tool's source, as C# reads its own:
/// every failure call goes through the one guarded call, and every way the start ends without a run makes it.
#[test]
fn a_humans_child_whose_start_throws_gives_its_text_back_once() {
    let source = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join("tools").join("delegate.rs"))
        .unwrap()
        .replace("\r\n", "\n");
    let at = source.find("async fn start_child(").expect("start_child is there");
    let end = source[at..].find("\n    }\n").map_or(source.len(), |end| at + end);
    let start = &source[at..end];
    assert!(
        start.matches("(human.failed)(").count() == 1 && start.contains("human.filter(|_| !*failed_called)"),
        "the failure call is made in one place, at most once"
    );
    assert!(
        start.matches("fail_once(&mut failed_called).await").count() >= 2,
        "each way the start ends with no run (no worktree, no pane or run) gives the text back"
    );
}
