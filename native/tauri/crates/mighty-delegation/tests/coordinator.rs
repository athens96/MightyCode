//! The coordinator with a fake host on real git repositories in temp folders (C# MightyCode.Core.Tests/
//! DelegationCoordinatorVerification.cs, ported from the Mac's DelegationCoordinatorTests, DelegationDelegateToolTests,
//! DelegationReportTests, DelegationActionToolTests, DelegationStatusToolTests, DelegationCardTests, DelegationStoragePressureTests and
//! the coordinator half of DelegationCleanupTests), each check under its C# name. The MCP server's rendering of an answer is W7-C's
//! (tests of `mcp::delegation`), the run-event pump W7-D's (`app_host`). Git runs through the platform's process host, so these are
//! Windows checks until the Mac stage (W7-B's `place.rs`).

mod support;

#[cfg(windows)]
mod windows {
    use std::collections::HashSet;
    use std::path::Path;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use mighty_core::shell::CardAction;
    use mighty_delegation::cards::{ChildCardDiscardOutcome, unsaved_reason};
    use mighty_delegation::coordinator::{Coordinator, MAXIMUM_FOLLOW_UP_BYTES, not_your_child_message};
    use mighty_delegation::copy::{DelegationCopyKind, MAXIMUM_COPY_BYTES, TRUNCATION_MARKER};
    use mighty_delegation::delivery::notice_text;
    use mighty_delegation::git::cleanup::{ChildCleanupHold, ChildCleanupOutcome, ChildDiscardOutcome};
    use mighty_delegation::git::merge::{self, ChildMergeOutcome, ChildUndoOutcome};
    use mighty_delegation::git::paths;
    use mighty_delegation::git::worktree::{ChildWorktree, MINIMUM_FREE_BYTES, is_safe_session_id, report_file_in, task_file_in};
    use mighty_delegation::host::{DelegationChildPane, DelegationPaneActivity, DelegationPaneState};
    use mighty_delegation::model::{
        ChildRecord, ChildState, ChildStateEvent, DeliveryLane, DeliveryRoute, FollowUp, MergeKind, MergeRecord, Notice, NoticeKind,
        request_key,
    };
    use mighty_delegation::reports::ChildRunEnd;
    use mighty_delegation::request::{DETACHED_MESSAGE, DelegationReasonCode, DelegationResponse};
    use mighty_delegation::store::{self, DelegationFile, DelegationFileError, DelegationFileStore, MAXIMUM_FILE_BYTES, SaveError};
    use mighty_delegation::tools::delegate::first_input;
    use mighty_delegation::{providers, pruning, sidebar, tools};
    use sha2::{Digest, Sha256};

    use super::support::host::{self, Gate, TestHost, caller, copy, eventually, eventually_async, locale, record, record_of, request};
    use super::support::lane;
    use super::support::place::{DelegationPlace, remove_folder};

    use DelegationPaneActivity::{Finished, Idle, Running};

    fn pane(
        id: &str,
        mode: &str,
        folder: &str,
        parent: Option<&str>,
        run: Option<&str>,
        activity: DelegationPaneActivity,
    ) -> DelegationPaneState {
        DelegationPaneState {
            session_id: id.into(),
            permission_mode: mode.into(),
            folder: folder.into(),
            kind: "claude".into(),
            provider: "claude".into(),
            parent_session_id: parent.map(str::to_string),
            run_id: run.map(str::to_string),
            activity,
        }
    }

    fn with_run(record: ChildRecord, run: &str) -> ChildRecord {
        ChildRecord { run_id: Some(run.into()), ..record }
    }

    fn sha256(text: &str) -> String {
        Sha256::digest(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
    }

    /// A place with a coordinator over it. The pane "parent" works in the repository.
    struct Fixture {
        place: DelegationPlace,
        host: Arc<TestHost>,
        coordinator: Coordinator,
        switch: Arc<AtomicBool>,
        attempts: u32,
    }

    impl Fixture {
        fn store(&self) -> DelegationFileStore {
            DelegationFileStore::with_attempts(self.place.profile(), self.attempts)
        }
        fn file(&self) -> DelegationFile {
            self.coordinator.file()
        }
        fn child(&self, id: &str) -> ChildRecord {
            self.file().child(id).cloned().unwrap()
        }
        fn notices(&self, id: &str) -> Vec<Notice> {
            self.file().notices.into_iter().filter(|n| n.child_id == id).collect()
        }
        fn bytes(&self) -> Vec<u8> {
            std::fs::read(self.store().file_path()).unwrap()
        }
        fn loaded(&self) -> DelegationFile {
            self.store().load().unwrap()
        }
        async fn call(&self, tool: &str, arguments: &[(&str, &str)]) -> DelegationResponse {
            self.call_from("parent", tool, arguments).await
        }
        async fn owned_call(&self, tool: &'static str, arguments: Vec<(&'static str, String)>) -> DelegationResponse {
            let arguments: Vec<(&str, &str)> = arguments.iter().map(|(name, value)| (*name, value.as_str())).collect();
            self.call(tool, &arguments).await
        }
        async fn call_from(&self, pane: &str, tool: &str, arguments: &[(&str, &str)]) -> DelegationResponse {
            self.coordinator.handle_call(request(tool, arguments), caller(pane, "claude", "claude", true)).await
        }
        async fn delegate(&self, task: &str, mode: &str) -> DelegationResponse {
            self.call("delegate", &[("task", task), ("mode", mode)]).await
        }
        async fn delegate_from(&self, from: &str, task: &str, mode: &str) -> DelegationResponse {
            self.call_from(from, "delegate", &[("task", task), ("mode", mode)]).await
        }
        /// The child is in `state` and its start is over, the delivery of a failed start's notice included.
        async fn wait_for(&self, id: &str, state: ChildState) -> bool {
            eventually(60_000, || self.file().child(id).map(|c| c.state) == Some(state) && !self.coordinator.is_starting(id)).await
        }
        fn set_parent(&self, mode: &str, run: &str, activity: DelegationPaneActivity) {
            self.host.set(pane("parent", mode, &self.place.repo(), None, Some(run), activity));
        }
        /// A coordinator on the same profile, as the app makes one at launch.
        fn relaunch(&self) -> Coordinator {
            let switch = self.switch.clone();
            host::coordinator(self.store(), self.host.clone(), host::maker(&self.place, 50_000_000_000), move || {
                switch.load(Ordering::SeqCst)
            })
            .unwrap()
        }
        /// A refused call made nothing: the same file, no worktree root, no `mighty/*` branch, no second worktree, no pane or run.
        async fn nothing_made(&self, bytes: &[u8], file: &DelegationFile, has_git: bool) {
            assert!(
                self.bytes() == bytes
                    && self.file() == *file
                    && !Path::new(&self.place.root()).exists()
                    && self.host.created_panes().is_empty()
                    && self.host.started_runs().is_empty(),
                "the refused call changed nothing"
            );
            if has_git {
                assert!(
                    self.place.mighty_branches().await.is_empty() && self.place.worktree_list().await.len() == 1,
                    "no branch and no worktree was made"
                );
            }
        }
    }

    /// `Make`: `commits` None leaves the repository folder without git. The host makes child panes and names their first run
    /// "run-of-<id>".
    async fn make(commits: Option<usize>, file: Option<DelegationFile>, parent_mode: &str, free: u64, attempts: u32) -> Fixture {
        let place = DelegationPlace::new();
        let store = DelegationFileStore::with_attempts(place.profile(), attempts);
        store.save(&file.unwrap_or_default()).unwrap();
        let host = TestHost::new(Some(place.profile().into()), "run");
        host.set_first_run(Some(|id: &str| format!("run-of-{id}")));
        host.set_makes_panes(true);
        std::fs::create_dir_all(place.repo()).unwrap();
        if let Some(count) = commits {
            place.repository(count, "", "false").await;
        }
        let switch = Arc::new(AtomicBool::new(true));
        let on = switch.clone();
        let coordinator = host::coordinator(store, host.clone(), host::maker(&place, free), move || on.load(Ordering::SeqCst)).unwrap();
        let fixture = Fixture { place, host, coordinator, switch, attempts };
        fixture.set_parent(parent_mode, "run-1", Running);
        fixture
    }

    async fn make_default() -> Fixture {
        make(Some(1), None, "auto", 50_000_000_000, 40).await
    }

    /// `Tools`: a coordinator over `children`, saved in the profile before it loads; the pane "parent" runs in the repository.
    fn tools_over(place: DelegationPlace, children: Vec<ChildRecord>, attempts: u32) -> Fixture {
        let store = DelegationFileStore::with_attempts(place.profile(), attempts);
        store.save(&DelegationFile::of(children, vec![], vec![], vec![], vec![])).unwrap();
        let host = TestHost::new(Some(place.profile().into()), "run");
        host.set(pane("parent", "auto", &place.repo(), None, Some("run-1"), Running));
        let switch = Arc::new(AtomicBool::new(true));
        let on = switch.clone();
        let coordinator =
            host::coordinator(store, host.clone(), host::maker(&place, 50_000_000_000), move || on.load(Ordering::SeqCst)).unwrap();
        Fixture { place, host, coordinator, switch, attempts }
    }

    /// `BlockSaves`: every save of the profile's delegation file fails, as a disk that refuses it — a folder stands where the file goes.
    fn block_saves(store: &DelegationFileStore) -> impl FnOnce() + use<> {
        let path = store.file_path();
        let aside = path.with_extension("json.aside");
        std::fs::rename(&path, &aside).unwrap();
        std::fs::create_dir(&path).unwrap();
        move || {
            std::fs::remove_dir(&path).unwrap();
            std::fs::rename(&aside, &path).unwrap();
        }
    }

    /// `MergedChild`: a reported child the tool merged, and its record.
    async fn merged_child(place: &DelegationPlace, id: &str) -> (ChildRecord, MergeRecord) {
        let (child, head) = place.reported_child(id, &[], Some(&place.repo())).await;
        let ChildMergeOutcome::Merged(merge) = merge::tool_merge(&place.git(), &child.view(), &head, &place.repo()).await else {
            panic!("the child was not merged")
        };
        (record_of(&child).applying(ChildStateEvent::Merge).unwrap(), merge)
    }

    /// "delegation: delegate starts a child in its own worktree, once per call, three at most"
    #[tokio::test]
    async fn delegate_starts_a_child_in_its_own_worktree_once_per_call_three_at_most() {
        // Its future is large in a debug build: on the heap, not the test thread's stack.
        Box::pin(delegate_starts_a_child_in_its_own_worktree_once_per_call_three_at_most_body()).await;
    }

    async fn delegate_starts_a_child_in_its_own_worktree_once_per_call_three_at_most_body() {
        let _lane = lane::enter().await;
        Box::pin(async {
            // Delegate answers once the record exists in creating, then opens the child in the asked mode and runs it.
            let fixture = make_default().await;
            let gate = Gate::new();
            fixture.host.set_pane_gate(Some(gate.clone()));
            let head = fixture.place.fixture_git(&["rev-parse", "HEAD"]).await;
            let task = "Write the release notes.\n";
            let answer = fixture.delegate(task, "acceptEdits").await;
            let info = answer.child.clone().expect("a child");
            assert!(
                answer.refused.is_none()
                    && answer.error.is_none()
                    && info.state == ChildState::Creating
                    && info.starting_mode == "acceptEdits"
                    && info.branch == format!("mighty/{}", info.id)
                    && is_safe_session_id(&info.id)
            );
            let saved = fixture.loaded();
            let child = saved.children.first().unwrap().clone();
            assert!(
                saved.children.len() == 1
                    && child.state == ChildState::Creating
                    && child.parent_session_id == "parent"
                    && child.starting_mode == "acceptEdits"
                    && child.parent_branch == "main"
                    && child.id == info.id
                    && child.request_key == request_key("parent", "run-1", task, "acceptEdits", None)
                    && paths::same(&child.worktree_path, &Path::new(&fixture.place.root()).join(&info.id).to_string_lossy())
                    && child.base_commit == head
                    && saved.copy(&info.id, DelegationCopyKind::Task).map(|c| c.text.as_str()) == Some(task)
                    && saved.notices.is_empty(),
                "the record is saved before the answer"
            );
            let canonical = super::support::place::paths();
            assert!(
                child
                    .parent_checkout
                    .as_deref()
                    .is_some_and(|recorded| paths::same(&canonical.canonical(recorded), &canonical.canonical(&fixture.place.repo()))),
                "the record keeps the parent's side"
            );
            assert!(eventually(60_000, || fixture.host.created_panes().len() == 1).await);
            assert_eq!(
                fixture.host.created_panes()[0],
                DelegationChildPane {
                    session_id: info.id.clone(),
                    parent_session_id: "parent".into(),
                    mode: "acceptEdits".into(),
                    folder: child.worktree_path.clone(),
                    started_by_human: false,
                    provider: "claude".into()
                },
                "the host is asked for the child's pane"
            );
            assert!(
                std::fs::read_to_string(task_file_in(&child.worktree_path)).unwrap() == task
                    && fixture.place.fixture_git(&["rev-parse", &format!("refs/heads/{}", info.branch)]).await == head
                    && fixture.file().children[0].state == ChildState::Creating
                    && fixture.host.started_runs().is_empty()
                    && fixture.coordinator.is_starting(&info.id),
                "its worktree is made, and its start is still going"
            );
            // While its start is going, discard is refused and changes nothing.
            let disk = fixture.place.disk().await;
            assert_eq!(
                fixture.coordinator.discard_child(&info.id).await,
                ChildDiscardOutcome::Failed(locale().get("delegation.card.error.stillStarting"))
            );
            assert!(
                fixture.place.disk().await == disk
                    && fixture.host.stops().is_empty()
                    && matches!(fixture.coordinator.discard_from_card(&info.id, Some(&[])).await, ChildCardDiscardOutcome::Failed(_)),
                "a discard waits for the child's start"
            );
            // Once its pane is made, its first run starts and it is running.
            gate.open();
            assert!(
                fixture.wait_for(&info.id, ChildState::Running).await
                    && fixture.loaded().children[0].state == ChildState::Running
                    && fixture.loaded().notices.is_empty(),
                "once its pane is made it runs"
            );
            let run = fixture.host.started_runs().into_iter().next().unwrap();
            let worktree = ChildWorktree {
                session_id: info.id.clone(),
                worktree_path: child.worktree_path.clone(),
                working_folder: child.worktree_path.clone(),
                branch: info.branch.clone(),
                parent_branch: "main".into(),
                base_commit: head.clone(),
            };
            let running = fixture.child(&info.id);
            assert!(
                run.0 == info.id
                    && run.1 == first_input(task, &worktree, "claude", Some("claude"))
                    && run.1.contains(&report_file_in(&child.worktree_path))
                    && running.run_id.as_deref() == Some(format!("run-of-{}", info.id).as_str())
                    && running.awaited_run_id == running.run_id,
                "its first run carries the task, and is the run its parent awaits"
            );
            assert!(
                fixture.place.fixture_git(&["symbolic-ref", "--short", "HEAD"]).await == "main"
                    && fixture.place.fixture_git(&["status", "--porcelain", "--untracked-files=all"]).await.is_empty(),
                "the parent's checkout is untouched"
            );
            drop(fixture);
        })
        .await;

        Box::pin(async {
            // The same call twice in one parent run returns the same child.
            let fixture = make_default().await;
            let first = fixture.delegate("Fix the parser.", "plan").await.child.unwrap();
            assert!(
                fixture.delegate("Fix the parser.", "plan").await.child.map(|c| c.id) == Some(first.id.clone())
                    && fixture.wait_for(&first.id, ChildState::Running).await,
                "the same call is the same child"
            );
            let (a, b) = tokio::join!(fixture.delegate("Fix the parser.", "plan"), fixture.delegate("Fix the parser.", "plan"));
            assert!(
                [a, b]
                    .iter()
                    .all(|repeat| repeat.child.as_ref().is_some_and(|same| same.state == ChildState::Running && same.id == first.id)),
                "repeats return the running child"
            );
            let (a, b) = tokio::join!(fixture.delegate("Write the docs.", "plan"), fixture.delegate("Write the docs.", "plan"));
            let second = a.child.unwrap();
            assert!(b.child.map(|c| c.id) == Some(second.id.clone()) && second.id != first.id, "two calls at once are one child");
            fixture.set_parent("auto", "run-2", Running);
            let third = fixture.delegate("Fix the parser.", "plan").await.child.unwrap();
            assert!(
                [&first.id, &second.id, &third.id].iter().collect::<HashSet<_>>().len() == 3
                    && fixture.wait_for(&second.id, ChildState::Running).await
                    && fixture.wait_for(&third.id, ChildState::Running).await,
                "the parent's next run makes a new child"
            );
            let saved = fixture.loaded();
            let mut panes: Vec<String> = fixture.host.created_panes().into_iter().map(|p| p.session_id).collect();
            panes.sort();
            let mut expected = vec![first.id.clone(), second.id.clone(), third.id.clone()];
            assert!(
                saved.children.iter().map(|c| &c.id).eq(expected.iter())
                    && saved.children.iter().map(|c| &c.request_key).collect::<HashSet<_>>().len() == 3,
                "three children, three keys"
            );
            expected.sort();
            assert_eq!(panes, expected, "three panes");
            let mut branches: Vec<String> = fixture.place.mighty_branches().await.split('\n').map(String::from).collect();
            branches.sort();
            let mut wanted: Vec<String> = [&first, &second, &third].iter().map(|c| format!("refs/heads/{}", c.branch)).collect();
            wanted.sort();
            assert_eq!(branches, wanted, "three branches");
            // Three open children hit the width cap; closing one makes room.
            let (bytes, file) = (fixture.bytes(), fixture.file());
            assert!(
                fixture.delegate("A fourth one.", "plan").await == DelegationResponse::refusal(DelegationReasonCode::WidthCap)
                    && fixture.delegate("A fourth one.", "manual").await == DelegationResponse::refusal(DelegationReasonCode::WidthCap)
                    && fixture.bytes() == bytes
                    && fixture.file() == file
                    && fixture.host.created_panes().len() == 3
                    && std::fs::read_dir(fixture.place.root()).unwrap().count() == 3,
                "a fourth open child is refused with width_cap"
            );
            fixture.host.close(&third.id);
            assert_eq!(fixture.coordinator.close_child(&third.id, false).await, ChildCleanupOutcome::Kept(ChildCleanupHold::NotMerged));
            let fourth = fixture.delegate("A fourth one.", "plan").await.child.expect("closing a child's pane makes room");
            assert!(fixture.wait_for(&fourth.id, ChildState::Running).await);
            drop(fixture);
        })
        .await;

        Box::pin(async {
            // Only open children count toward the cap, and a pane with a parent link cannot delegate.
            let others = vec![
                record("waiting", ChildState::Waiting, "parent"),
                record("merged", ChildState::Merged, "parent"),
                record("failed", ChildState::Failed, "parent"),
                record("closed", ChildState::Closed, "parent"),
                record("discarded", ChildState::Discarded, "parent"),
                record("not-mine", ChildState::Running, "someone-else"),
            ];
            let fixture = make(Some(1), Some(DelegationFile::of(others, vec![], vec![], vec![], vec![])), "auto", 50_000_000_000, 40).await;
            fixture.host.set(pane("waiting", "fullAccess", &fixture.place.repo(), None, Some("run-r"), Running));
            fixture.host.set(pane("linked", "fullAccess", &fixture.place.repo(), Some("parent"), Some("run-l"), Running));
            let (bytes, file) = (fixture.bytes(), fixture.file());
            for from in ["waiting", "linked"] {
                assert_eq!(
                    fixture.delegate_from(from, "Split the work further.", "plan").await,
                    DelegationResponse::refusal(DelegationReasonCode::ChildCannotDelegate),
                    "{from} cannot delegate"
                );
            }
            fixture.nothing_made(&bytes, &file, true).await;
            let third = fixture.delegate("The third one.", "plan").await.child.unwrap();
            assert!(
                fixture.wait_for(&third.id, ChildState::Running).await
                    && fixture.delegate("A fourth one.", "plan").await == DelegationResponse::refusal(DelegationReasonCode::WidthCap)
                    && fixture.host.created_panes().iter().map(|p| p.session_id.clone()).eq([third.id.clone()]),
                "failed, closed and discarded children and another parent's do not count"
            );
        })
        .await;
    }

    /// "delegation: delegate refuses before anything is made, and a failed start is kept for a human"
    #[tokio::test]
    async fn delegate_refuses_before_anything_is_made_and_a_failed_start_is_kept_for_a_human() {
        // Its future is large in a debug build: on the heap, not the test thread's stack.
        Box::pin(delegate_refuses_before_anything_is_made_and_a_failed_start_is_kept_for_a_human_body()).await;
    }

    async fn delegate_refuses_before_anything_is_made_and_a_failed_start_is_kept_for_a_human_body() {
        let _lane = lane::enter().await;
        Box::pin(async {
            let modes = providers::starting_modes("claude");
            assert_eq!(modes, ["plan", "manual", "acceptEdits", "auto", "fullAccess"], "the starting modes, narrowest first");
            for (own, stored) in modes.iter().enumerate() {
                for (asked, mode) in modes.iter().enumerate() {
                    assert_eq!(providers::is_no_wider(mode, stored, "claude"), asked <= own, "{mode} under {stored}");
                }
            }
            assert!(
                !providers::is_no_wider("plan", "onRequest", "claude")
                    && !providers::is_no_wider("bypassPermissions", "fullAccess", "claude"),
                "an unknown mode is never no wider"
            );
            let fixture = make(Some(1), None, "acceptEdits", 50_000_000_000, 40).await;
            let (bytes, file) = (fixture.bytes(), fixture.file());
            for mode in ["auto", "fullAccess"] {
                assert_eq!(
                    fixture.delegate("Ship it.", mode).await,
                    DelegationResponse::refusal(DelegationReasonCode::WiderMode),
                    "{mode} is wider than acceptEdits"
                );
            }
            for mode in ["bypassPermissions", "default", "", "PLAN"] {
                assert_eq!(
                    fixture.delegate("Ship it.", mode).await,
                    DelegationResponse::failure("mode must be one of plan, manual, acceptEdits, auto, fullAccess."),
                    "not a starting mode: {mode}"
                );
            }
            assert_eq!(fixture.delegate(" \n\t", "plan").await.error.as_deref(), Some("delegate needs a task: what the child should do."));
            assert_eq!(fixture.delegate("Ship it.\0Now.", "plan").await.error.as_deref(), Some("A task may not contain a NUL character."));
            assert_eq!(
                fixture.delegate(&"x".repeat(MAXIMUM_COPY_BYTES + 1), "plan").await.error.as_deref(),
                Some("A task may be at most 64 KiB.")
            );
            assert_eq!(
                fixture.call("delegate", &[("task", "x")]).await.error.as_deref(),
                Some("delegate takes exactly these string arguments: task, mode, and optionally agent.")
            );
            assert!(
                fixture
                    .call("delegate", &[("task", "x"), ("mode", "plan"), ("branch", "main")])
                    .await
                    .error
                    .unwrap()
                    .contains("task, mode")
            );
            fixture.host.close("parent");
            assert_eq!(fixture.delegate("Ship it.", "plan").await.error.as_deref(), Some("The calling pane is closed."));
            fixture.nothing_made(&bytes, &file, true).await;
            fixture.set_parent("auto", "run-1", Running);
            let child = fixture.delegate("Ship it.", "auto").await.child.unwrap();
            assert!(
                child.starting_mode == "auto"
                    && fixture.wait_for(&child.id, ChildState::Running).await
                    && fixture.loaded().children[0].starting_mode == "auto"
                    && fixture.host.created_panes()[0].mode == "auto",
                "the mode is checked against the parent's stored mode now"
            );
            drop(fixture);
        })
        .await;

        Box::pin(async {
            // A folder that cannot host a child worktree is refused before anything is made.
            let plain = make(None, None, "auto", 50_000_000_000, 40).await;
            let unborn = make(Some(0), None, "auto", 50_000_000_000, 40).await;
            let detached = make(Some(2), None, "auto", 50_000_000_000, 40).await;
            let full = make(Some(1), None, "auto", MINIMUM_FREE_BYTES - 1, 40).await;
            detached.place.fixture_git(&["checkout", "-q", "--detach", "HEAD~1"]).await;
            for (refused, reason) in [
                (&plain, DelegationReasonCode::NotGit),
                (&unborn, DelegationReasonCode::UnbornBranch),
                (&detached, DelegationReasonCode::DetachedHead),
                (&full, DelegationReasonCode::LowDisk),
            ] {
                let (bytes, file) = (refused.bytes(), refused.file());
                assert_eq!(refused.delegate("Do the work.", "plan").await, DelegationResponse::refusal(reason), "the workspace is refused");
                refused.nothing_made(&bytes, &file, reason != DelegationReasonCode::NotGit).await;
            }
            drop((plain, unborn, detached, full));
        })
        .await;

        Box::pin(async {
            // No room for the child and its report, even after pruning, is store_full; pruning a closed child's report copy makes the room.
            fn nearly_full(closed_first: bool) -> DelegationFile {
                let mut file = DelegationFile::empty();
                let full_copy = "r".repeat(MAXIMUM_COPY_BYTES);
                for index in 0.. {
                    let state = if closed_first && index == 0 { ChildState::Closed } else { ChildState::Running };
                    let child = record(&format!("filler-{index}"), state, &format!("other-{index}"));
                    let mut next = file.clone();
                    next.children.push(child.clone());
                    let next = next.setting_copy(copy(&child.id, DelegationCopyKind::Report, &full_copy, None));
                    if store::encode(&next).len() > MAXIMUM_FILE_BYTES {
                        break;
                    }
                    file = next;
                }
                let room = MAXIMUM_FILE_BYTES - store::encode(&file).len();
                file.children.push(record("pad", ChildState::Running, "other-pad"));
                let file = file.setting_copy(copy("pad", DelegationCopyKind::Report, &"p".repeat(room.saturating_sub(3_000)), None));
                let size = store::encode(&file).len();
                assert!(size <= MAXIMUM_FILE_BYTES && MAXIMUM_FILE_BYTES - size < 4_000, "the file is nearly full");
                file
            }
            let packed = make(Some(1), Some(nearly_full(false)), "auto", 50_000_000_000, 40).await;
            let prunable = make(Some(1), Some(nearly_full(true)), "auto", 50_000_000_000, 40).await;
            let (bytes, file) = (packed.bytes(), packed.file());
            assert_eq!(
                packed.delegate("One more child.", "plan").await,
                DelegationResponse::refusal(DelegationReasonCode::StoreFull),
                "no room even after pruning is store_full"
            );
            packed.nothing_made(&bytes, &file, true).await;
            let child = prunable.delegate("One more child.", "plan").await.child.unwrap();
            let saved = prunable.loaded();
            assert!(
                saved.child(&child.id).is_some()
                    && saved.copy("filler-0", DelegationCopyKind::Report).is_none()
                    && saved.copies.iter().filter(|c| c.kind == DelegationCopyKind::Report).count() == nearly_full(true).copies.len() - 1
                    && prunable.bytes().len() <= MAXIMUM_FILE_BYTES
                    && prunable.wait_for(&child.id, ChildState::Running).await,
                "pruning the closed child's report copy makes the room"
            );
            drop((packed, prunable));
        })
        .await;

        Box::pin(async {
            // A start that fails leaves the child failed with one failed_to_start notice, and keeps what it made.
            let fixture = make_default().await;
            fixture.host.pane_fails.store(true, Ordering::SeqCst);
            let no_pane = fixture.delegate("Try it.", "plan").await.child.unwrap();
            assert!(
                no_pane.state == ChildState::Creating
                    && fixture.wait_for(&no_pane.id, ChildState::Failed).await
                    && fixture.host.started_runs().is_empty(),
                "no pane, no run"
            );
            fixture.host.pane_fails.store(false, Ordering::SeqCst);
            fixture.host.run_fails.store(true, Ordering::SeqCst);
            let no_run = fixture.delegate("Try again.", "plan").await.child.unwrap();
            assert!(
                fixture.wait_for(&no_run.id, ChildState::Failed).await
                    && fixture.host.started_runs().iter().map(|r| r.0.clone()).eq([no_run.id.clone()]),
                "a pane, but no run"
            );
            let saved = fixture.loaded();
            assert!(
                saved.children.iter().all(|c| c.state == ChildState::Failed)
                    && saved.notices.iter().map(|n| n.child_id.clone()).eq([no_pane.id.clone(), no_run.id.clone()])
                    && saved.notices.iter().all(|n| n.kind == NoticeKind::FailedToStart && n.report_revision == 0),
                "each failed start has its one notice"
            );
            assert!(
                saved
                    .notices
                    .iter()
                    .all(|n| n.lane == DeliveryLane::Delivered && n.receipt.as_ref().map(|r| r.route) == Some(DeliveryRoute::Steer))
                    && fixture.host.taken().iter().map(|h| h.input.clone()).eq(saved.notices.iter().map(notice_text)),
                "the failed starts' notices reach the parent"
            );
            assert!(saved.children.iter().all(|c| Path::new(&task_file_in(&c.worktree_path)).exists()), "what they made stays");
            let again = fixture.delegate("Try it.", "plan").await.child.unwrap();
            assert!(
                again.state == ChildState::Failed
                    && again.id == no_pane.id
                    && fixture.host.created_panes().len() == 2
                    && fixture.loaded().notices.len() == 2,
                "the same call is that failed child"
            );
            // A host that breaks while starting the run has started nothing either: failed, never left creating.
            fixture.host.run_fails.store(false, Ordering::SeqCst);
            fixture.host.set_on_first_run(Some(|_: String, _: String| -> mighty_platform::BoxFuture<'static, ()> {
                Box::pin(async { panic!("the host broke") })
            }));
            let broke = fixture.delegate("Once more.", "plan").await.child.unwrap();
            assert!(
                fixture.wait_for(&broke.id, ChildState::Failed).await && fixture.loaded().notices.len() == 3,
                "a host that breaks leaves the child failed, with its notice"
            );
            for failed in [&no_pane, &no_run, &broke] {
                assert_eq!(
                    fixture.coordinator.discard_from_card(&failed.id, Some(&[])).await,
                    ChildCardDiscardOutcome::Discarded,
                    "a failed start is discarded"
                );
            }
            assert!(
                fixture.place.mighty_branches().await.is_empty()
                    && fixture.place.only_the_repository_is_listed().await
                    && std::fs::read_dir(fixture.place.root()).unwrap().count() == 0
                    && fixture.loaded().children.iter().all(|c| c.state == ChildState::Discarded),
                "nothing of them is left"
            );
            drop(fixture);
        })
        .await;

        Box::pin(async {
            // Its worktree cannot be made (a file stands where the root goes): failed before any pane is asked for.
            let blocked = make_default().await;
            std::fs::write(blocked.place.root(), "not a folder").unwrap();
            let no_worktree = blocked.delegate("Try it.", "plan").await.child.unwrap();
            assert!(
                blocked.wait_for(&no_worktree.id, ChildState::Failed).await
                    && blocked.loaded().notices[0].kind == NoticeKind::FailedToStart
                    && blocked.host.created_panes().is_empty(),
                "no worktree, no pane"
            );
        })
        .await;
    }

    /// The Mac's report fixture: the host names a child's first run "first-<id>".
    async fn reports(file: Option<DelegationFile>) -> Fixture {
        let fixture = make(Some(1), file, "auto", 50_000_000_000, 40).await;
        fixture.host.set_first_run(Some(|id: &str| format!("first-{id}")));
        fixture.set_parent("auto", "parent-run", Running);
        fixture
    }

    async fn running_child(fixture: &Fixture) -> ChildRecord {
        let id = fixture.delegate("Fix the parser.", "acceptEdits").await.child.unwrap().id;
        assert!(fixture.wait_for(&id, ChildState::Running).await, "the child's first run started");
        fixture.child(&id)
    }

    async fn commit_work(fixture: &Fixture, child: &ChildRecord, name: &str) -> String {
        std::fs::write(Path::new(&child.worktree_path).join(format!("{name}.txt")), format!("{name}\n")).unwrap();
        fixture.place.git_in(&child.worktree_path, &["add", "-A"]).await;
        fixture.place.git_in(&child.worktree_path, &["commit", "-q", "-m", name]).await;
        fixture.place.git_in(&child.worktree_path, &["rev-parse", "HEAD"]).await
    }

    fn write_report(child: &ChildRecord, text: &str) {
        std::fs::write(report_file_in(&child.worktree_path), text).unwrap();
    }

    /// "delegation: run ends record report revisions and send one notice each"
    #[tokio::test]
    async fn run_ends_record_report_revisions_and_send_one_notice_each() {
        // Its future is large in a debug build: on the heap, not the test thread's stack.
        Box::pin(run_ends_record_report_revisions_and_send_one_notice_each_body()).await;
    }

    async fn run_ends_record_report_revisions_and_send_one_notice_each_body() {
        let _lane = lane::enter().await;
        Box::pin(async {
            let fixture = reports(None).await;
            let started = running_child(&fixture).await;
            let id = started.id.clone();
            let run = format!("first-{id}");
            assert!(
                started.state == ChildState::Running
                    && started.report_revision == 0
                    && started.report_head.is_none()
                    && started.report_digest.is_none()
                    && started.run_id.as_deref() == Some(run.as_str())
                    && started.awaited_run_id.as_deref() == Some(run.as_str())
            );
            let head = commit_work(&fixture, &started, "parser").await;
            let text = "Fixed the parser.\nLeft: the docs.\n";
            write_report(&started, text);
            let notice = fixture.coordinator.child_run_ended(&id, &run, ChildRunEnd::Finished).await.unwrap();
            assert!(
                notice.kind == NoticeKind::Reported && notice.report_revision == 1 && notice.child_id == id,
                "one reported notice for revision 1"
            );
            let reported = fixture.child(&id);
            assert!(
                reported.state == ChildState::Reported
                    && reported.report_revision == 1
                    && reported.report_head.as_deref() == Some(head.as_str())
                    && reported.report_digest.as_deref() == Some(sha256(text).as_str())
                    && fixture.notices(&id)[0].id == notice.id
                    && fixture
                        .file()
                        .copy(&id, DelegationCopyKind::Report)
                        .is_some_and(|c| c.revision == 1 && !c.truncated && c.text == text)
                    && fixture.loaded() == fixture.file(),
                "the revision is recorded at the child's head, with a copy of the report"
            );
            let taken = fixture.host.taken();
            assert!(
                taken.len() == 1
                    && taken[0].session_id == "parent"
                    && taken[0].route == Some(DeliveryRoute::Steer)
                    && taken[0].run_id.as_deref() == Some("parent-run")
                    && taken[0].input == notice_text(&notice),
                "the notice reaches the parent once"
            );
            let bytes = fixture.bytes();
            assert!(fixture.coordinator.child_run_ended(&id, &run, ChildRunEnd::Finished).await.is_none(), "the same end again is nothing");
            fixture.coordinator.child_run_started(&id, &run).await;
            assert!(fixture.bytes() == bytes && fixture.child(&id) == reported, "nothing changed");
            let status = fixture.call("child_status", &[("child", &id)]).await.status.unwrap();
            assert!(
                status.state == ChildState::Reported
                    && status.revision == 1
                    && status.head.as_deref() == Some(head.as_str())
                    && status.report.as_deref() == Some(text),
                "child_status gives the report"
            );

            // Any new run clears reported, and only a changed report makes the next revision.
            fixture.coordinator.child_run_started(&id, "human-2").await;
            let running = fixture.child(&id);
            assert!(
                running.state == ChildState::Running
                    && running.run_id.as_deref() == Some("human-2")
                    && running.report_revision == 1
                    && running.report_head.as_deref() == Some(head.as_str())
            );
            assert_eq!(
                fixture.call("merge", &[("child", &id), ("expected_head", &head)]).await.refused,
                Some(DelegationReasonCode::NotReported),
                "a run in the child's pane clears reported, so a merge waits"
            );
            assert!(fixture.coordinator.child_run_ended(&id, "human-2", ChildRunEnd::Finished).await.is_none());
            let standing = fixture.child(&id);
            assert!(
                standing.state == ChildState::Reported
                    && standing.report_revision == 1
                    && standing.report_head.as_deref() == Some(head.as_str())
                    && fixture.notices(&id).len() == 1,
                "an unchanged report is no new revision, and it stands"
            );
            fixture.coordinator.child_run_started(&id, "human-2b").await;
            commit_work(&fixture, &started, "unreported").await;
            assert!(
                fixture.coordinator.child_run_ended(&id, "human-2b", ChildRunEnd::Finished).await.is_none()
                    && fixture.child(&id).state == ChildState::Ended
                    && fixture.notices(&id).len() == 1,
                "new work with no new report ends the child"
            );
            fixture.coordinator.child_run_started(&id, "human-3").await;
            let second_head = commit_work(&fixture, &started, "two").await;
            let long_report = "a line of the second report\n".repeat(4_000);
            write_report(&started, &long_report);
            let second_notice = fixture.coordinator.child_run_ended(&id, "human-3", ChildRunEnd::Stopped).await.unwrap();
            assert!(second_notice.kind == NoticeKind::Reported && second_notice.report_revision == 2, "a changed report is revision 2");
            let second = fixture.child(&id);
            let cut = fixture.file().copy(&id, DelegationCopyKind::Report).cloned().unwrap();
            assert!(
                second.state == ChildState::Reported
                    && second.report_revision == 2
                    && second.report_head.as_deref() == Some(second_head.as_str())
                    && second.report_digest.as_deref() == Some(sha256(&long_report).as_str())
                    && fixture.notices(&id).iter().map(|n| n.report_revision).eq([1, 2])
                    && cut.revision == 2
                    && cut.truncated
                    && cut.text.ends_with(TRUNCATION_MARKER)
                    && cut.original_bytes == long_report.len() as i64,
                "a long report is kept as the marked copy"
            );
            assert_eq!(
                fixture.call("merge", &[("child", &id), ("expected_head", &second_head)]).await.merged.map(|m| m.merged_commit),
                Some(second_head.clone()),
                "the reported head merges"
            );
            fixture.coordinator.child_run_started(&id, "human-4").await;
            assert_eq!(fixture.child(&id).state, ChildState::Running, "a merged child runs again");
            write_report(&started, &long_report);
            assert!(
                fixture.coordinator.child_run_ended(&id, "human-4", ChildRunEnd::Errored).await.is_none()
                    && fixture.child(&id).state == ChildState::Ended
                    && fixture.child(&id).report_revision == 2,
                "the same report again is no new revision"
            );
            // A REPORT.md that is a folder is no report, and one with no text is none either.
            fixture.coordinator.child_run_started(&id, "human-5").await;
            let report_path = report_file_in(&started.worktree_path);
            std::fs::remove_file(&report_path).unwrap();
            std::fs::create_dir(&report_path).unwrap();
            assert!(
                mighty_delegation::git::report_file::read(&report_path).is_none()
                    && fixture.coordinator.child_run_ended(&id, "human-5", ChildRunEnd::Finished).await.is_none(),
                "a folder in the report's place is no report"
            );
            std::fs::remove_dir(&report_path).unwrap();
            fixture.coordinator.child_run_started(&id, "human-6").await;
            write_report(&started, " \r\n\t\n");
            assert!(
                fixture.coordinator.child_run_ended(&id, "human-6", ChildRunEnd::Finished).await.is_none()
                    && fixture.child(&id).report_revision == 2
                    && fixture.notices(&id).len() == 2
                    && fixture.loaded() == fixture.file(),
                "a report with no text is no report"
            );
            drop(fixture);
        })
        .await;

        Box::pin(async {
            // A delegate or follow-up run ending without a new report sends one ended_without_report notice.
            let fixture = reports(None).await;
            let started = running_child(&fixture).await;
            let id = started.id.clone();
            let first = fixture.coordinator.child_run_ended(&id, &format!("first-{id}"), ChildRunEnd::Stopped).await.unwrap();
            assert!(
                first.kind == NoticeKind::EndedWithoutReport
                    && first.report_revision == 0
                    && fixture.child(&id).state == ChildState::Ended
                    && fixture.coordinator.child_run_ended(&id, &format!("first-{id}"), ChildRunEnd::Finished).await.is_none()
                    && fixture.notices(&id).len() == 1
                    && fixture.loaded() == fixture.file(),
                "the first run's end without a report sends one notice"
            );
            fixture.host.set_claude(&id, Idle, Some(&format!("first-{id}")));
            assert_eq!(
                fixture.call("follow_up", &[("child", &id), ("text", "Also update the docs.")]).await.follow_up.map(|f| f.lane),
                Some(DeliveryLane::Held),
                "the follow-up is recorded"
            );
            fixture.coordinator.child_run_started(&id, "follow-2").await;
            assert!(
                fixture.coordinator.await_run("follow-2", &id).await.is_none()
                    && fixture.child(&id).awaited_run_id.as_deref() == Some("follow-2"),
                "the follow-up's run is awaited"
            );
            let second = fixture.coordinator.child_run_ended(&id, "follow-2", ChildRunEnd::Errored).await.unwrap();
            assert!(
                second.kind == NoticeKind::EndedWithoutReport
                    && fixture.coordinator.child_run_ended(&id, "follow-2", ChildRunEnd::Errored).await.is_none()
                    && fixture.coordinator.await_run("follow-2", &id).await.is_none()
                    && fixture.notices(&id).len() == 2,
                "its end sends one notice, never two"
            );
            fixture.coordinator.child_run_started(&id, "follow-3").await;
            assert!(fixture.coordinator.child_run_ended(&id, "follow-3", ChildRunEnd::Finished).await.is_none());
            assert_eq!(fixture.coordinator.await_run("follow-3", &id).await.map(|n| n.kind), Some(NoticeKind::EndedWithoutReport));
            assert!(
                fixture.coordinator.await_run("follow-3", &id).await.is_none() && fixture.notices(&id).len() == 3,
                "an end that came first is noticed when the run is awaited"
            );
            fixture.coordinator.child_run_started(&id, "follow-4").await;
            assert!(fixture.coordinator.await_run("follow-4", &id).await.is_none());
            write_report(&started, "Docs updated.\n");
            assert_eq!(
                fixture.coordinator.child_run_ended(&id, "follow-4", ChildRunEnd::Finished).await.map(|n| n.kind),
                Some(NoticeKind::Reported)
            );
            assert!(
                fixture.coordinator.await_run("follow-4", &id).await.is_none() && fixture.notices(&id).len() == 4,
                "a report sends the reported notice only"
            );
            fixture.coordinator.child_run_started(&id, "human-5").await;
            assert!(
                fixture.coordinator.child_run_ended(&id, "human-5", ChildRunEnd::Finished).await.is_none()
                    && fixture.notices(&id).len() == 4
                    && fixture.loaded() == fixture.file(),
                "a human's run sends nothing"
            );
            drop(fixture);
        })
        .await;

        Box::pin(async {
            // The first run's end may come before the host's start answers, and still sends one notice.
            let fixture = reports(None).await;
            let coordinator = fixture.coordinator.clone();
            fixture.host.set_on_first_run(Some(move |child: String, run: String| -> mighty_platform::BoxFuture<'static, ()> {
                let coordinator = coordinator.clone();
                Box::pin(async move {
                    coordinator.child_run_started(&child, &run).await;
                    coordinator.child_run_ended(&child, &run, ChildRunEnd::Errored).await;
                })
            }));
            let id = fixture.delegate("Fix the parser.", "plan").await.child.unwrap().id;
            assert!(eventually(60_000, || !fixture.coordinator.is_starting(&id)).await);
            let ended = fixture.child(&id);
            assert!(
                ended.state == ChildState::Ended
                    && ended.run_id.as_deref() == Some(format!("first-{id}").as_str())
                    && ended.awaited_run_id == ended.run_id
                    && fixture.notices(&id).len() == 1
                    && fixture.notices(&id)[0].kind == NoticeKind::EndedWithoutReport
                    && fixture.loaded() == fixture.file(),
                "an end before the start's answer still sends one notice"
            );
            drop(fixture);
        })
        .await;

        Box::pin(async {
            // Run events take turns, so an end half recorded is never overtaken.
            let fixture = reports(None).await;
            let id = running_child(&fixture).await.id;
            let gate = Gate::new();
            let asked = Arc::new(AtomicUsize::new(0));
            let (waiting, counted) = (gate.clone(), asked.clone());
            fixture.host.set_state_gate(Some(move |_: String| -> mighty_platform::BoxFuture<'static, ()> {
                counted.fetch_add(1, Ordering::SeqCst);
                let waiting = waiting.clone();
                Box::pin(async move { waiting.wait().await })
            }));
            let (c1, c2, c3) = (fixture.coordinator.clone(), fixture.coordinator.clone(), fixture.coordinator.clone());
            let (first_run, end_id, start_id, await_id) = (format!("first-{id}"), id.clone(), id.clone(), id.clone());
            let end = tokio::spawn(async move { c1.child_run_ended(&end_id, &first_run, ChildRunEnd::Finished).await });
            assert!(eventually(60_000, || asked.load(Ordering::SeqCst) > 0).await, "the end is waiting on the host");
            let start = tokio::spawn(async move { c2.child_run_started(&start_id, "follow-2").await });
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            let awaited = tokio::spawn(async move { c3.await_run("follow-2", &await_id).await });
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            assert!(
                fixture.child(&id).run_id.as_deref() == Some(format!("first-{id}").as_str())
                    && !end.is_finished()
                    && !start.is_finished()
                    && !awaited.is_finished(),
                "the start and the await wait their turn"
            );
            gate.open();
            let notice = end.await.unwrap().unwrap();
            assert!(notice.kind == NoticeKind::EndedWithoutReport && notice.report_revision == 0, "the first run's notice is not lost");
            start.await.unwrap();
            assert!(awaited.await.unwrap().is_none());
            let child = fixture.child(&id);
            assert!(
                child.state == ChildState::Running
                    && child.run_id.as_deref() == Some("follow-2")
                    && child.awaited_run_id.as_deref() == Some("follow-2")
            );
            assert_eq!(
                fixture.coordinator.child_run_ended(&id, "follow-2", ChildRunEnd::Finished).await.map(|n| n.kind),
                Some(NoticeKind::EndedWithoutReport)
            );
            assert!(fixture.notices(&id).len() == 2 && fixture.loaded() == fixture.file(), "each is recorded whole, in the order it came");
            drop(fixture);
        })
        .await;

        Box::pin(async {
            // A run killed by quitting the app sends no notice and shows interrupted.
            let fixture = reports(None).await;
            let started = running_child(&fixture).await;
            let id = started.id.clone();
            write_report(&started, "Half done.\n");
            assert!(fixture.coordinator.child_run_ended(&id, &format!("first-{id}"), ChildRunEnd::Quit).await.is_none());
            let child = fixture.child(&id);
            assert!(
                child.state == ChildState::Interrupted
                    && child.report_revision == 0
                    && child.report_head.is_none()
                    && fixture.notices(&id).is_empty()
                    && fixture.call("list_children", &[]).await.children.unwrap()[0].state == ChildState::Interrupted
                    && fixture.loaded() == fixture.file(),
                "a quit interrupts with no notice and no revision"
            );
            fixture.coordinator.child_run_started(&id, "human-2").await;
            assert_eq!(
                fixture.coordinator.child_run_ended(&id, "human-2", ChildRunEnd::Finished).await.map(|n| n.report_revision),
                Some(1)
            );
            assert_eq!(fixture.child(&id).state, ChildState::Reported, "the report it left is the next run's to record");
            drop(fixture);
        })
        .await;

        Box::pin(async {
            // At the next launch, the children whose runs the quit killed come back interrupted, with no notice.
            let file = DelegationFile::of(
                vec![
                    with_run(record("running", ChildState::Running, "parent"), "r"),
                    with_run(record("waiting", ChildState::Waiting, "parent"), "w"),
                    record("reported", ChildState::Reported, "parent"),
                    with_run(record("closed", ChildState::Closed, "parent"), "c"),
                ],
                vec![],
                vec![],
                vec![],
                vec![],
            );
            let fixture = reports(Some(file)).await;
            let mut rows = fixture.coordinator.child_rows();
            assert_eq!(
                fixture.coordinator.interrupt_runs_killed_by_quit().await,
                ["running", "waiting"],
                "the launch's sweep finds the runs the quit killed"
            );
            assert!(
                fixture.file().children.iter().map(|c| c.state).eq([
                    ChildState::Interrupted,
                    ChildState::Interrupted,
                    ChildState::Reported,
                    ChildState::Closed
                ]) && fixture.file().notices.is_empty()
                    && fixture.loaded() == fixture.file(),
                "they are interrupted, with no notice"
            );
            let bytes = fixture.bytes();
            assert!(
                fixture.coordinator.child_run_ended("running", "r", ChildRunEnd::Finished).await.is_none(),
                "a killed run's late end is nothing"
            );
            fixture.coordinator.child_run_started("closed", "c2").await;
            assert!(
                fixture.coordinator.child_run_ended("closed", "c2", ChildRunEnd::Finished).await.is_none()
                    && fixture.coordinator.interrupt_runs_killed_by_quit().await.is_empty()
                    && fixture.bytes() == bytes,
                "a closed child takes no runs"
            );
            assert!(
                rows.has_changed().unwrap()
                    && rows.borrow_and_update().iter().map(|r| r.state).eq([
                        ChildState::Interrupted,
                        ChildState::Interrupted,
                        ChildState::Reported,
                        ChildState::Closed
                    ]),
                "the rows follow the file"
            );
            fixture.coordinator.child_run_started("running", "r2").await;
            fixture.coordinator.child_run_ended("running", "r2", ChildRunEnd::Stopped).await;
            let child = fixture.child("running");
            assert!(
                child.state == ChildState::Ended
                    && child.run_id.as_deref() == Some("r2")
                    && rows.borrow_and_update()[0].state == ChildState::Ended,
                "the events are recorded in order"
            );
        })
        .await;
    }

    /// "delegation: the gates, discard_human_only, the status tools and the server's protocol" (the server's protocol and an answer's
    /// rendering are W7-C's `mcp::delegation` checks)
    #[tokio::test]
    async fn the_gates_discard_human_only_the_status_tools_and_the_servers_protocol() {
        // Its future is large in a debug build: on the heap, not the test thread's stack.
        Box::pin(the_gates_discard_human_only_the_status_tools_and_the_servers_protocol_body()).await;
    }

    async fn the_gates_discard_human_only_the_status_tools_and_the_servers_protocol_body() {
        let _lane = lane::enter().await;
        let reported_head = "b".repeat(40);
        let with = |r: ChildRecord, revision: i32, head: Option<String>| ChildRecord { report_revision: revision, report_head: head, ..r };
        Box::pin(async {
            let report_text = "# 보고\n\nFixed the parser.\n";
            let children = vec![
                record("running", ChildState::Running, "parent"),
                with(record("reported", ChildState::Reported, "parent"), 2, Some(reported_head.clone())),
                with(record("merged", ChildState::Merged, "other"), 1, Some("c".repeat(40))),
                record("failed", ChildState::Failed, "parent"),
                with(record("closed", ChildState::Closed, "parent"), 3, Some("d".repeat(40))),
                record("discarded", ChildState::Discarded, "parent"),
                record("someone-elses", ChildState::Running, "stranger"),
            ];
            let standard = DelegationFile::of(
                children.clone(),
                vec![],
                vec![],
                vec![],
                vec![
                    copy("reported", DelegationCopyKind::Report, report_text, Some(2)),
                    copy("merged", DelegationCopyKind::Report, "Merged work.", Some(1)),
                    copy("someone-elses", DelegationCopyKind::Report, "Not yours.", Some(1)),
                ],
            );
            let off = make(None, Some(standard.clone()), "auto", 50_000_000_000, 40).await;
            off.switch.store(false, Ordering::SeqCst);
            let fixture = make(None, Some(standard), "auto", 50_000_000_000, 40).await;
            let arguments = |tool: &tools::Tool| -> Vec<(&'static str, &'static str)> {
                tool.arguments
                    .iter()
                    .map(|a| {
                        (
                            a.name,
                            match a.name {
                                "mode" => "plan",
                                "child" => "reported",
                                _ => "x",
                            },
                        )
                    })
                    .collect()
            };
            for tool in tools::ALL {
                assert_eq!(
                    off.call(tool.name, &arguments(tool)).await,
                    DelegationResponse::failure(DETACHED_MESSAGE),
                    "{} reaches nothing with the switch off",
                    tool.name
                );
            }
            let (bytes, file) = (fixture.bytes(), fixture.file());
            for who in [caller("gemini", "claude", "gemini", true), caller("shell", "shell", "claude", true)] {
                for tool in tools::ALL {
                    assert_eq!(
                        fixture.coordinator.handle_call(request(tool.name, &arguments(tool)), who.clone()).await,
                        DelegationResponse::refusal(DelegationReasonCode::ClaudeOnly)
                    );
                }
            }
            assert_eq!(
                fixture
                    .coordinator
                    .handle_call(request("discard", &[("child", "reported")]), caller("detached", "claude", "claude", false))
                    .await,
                DelegationResponse::failure(DETACHED_MESSAGE)
            );
            assert_eq!(fixture.call("no_such_tool", &[]).await, DelegationResponse::failure("Unknown delegation tool no_such_tool."));
            for given in [&[("child", "reported")][..], &[("child", "no-such-child")], &[], &[("child", "reported"), ("force", "true")]] {
                assert_eq!(
                    fixture.call("discard", given).await,
                    DelegationResponse::refusal(DelegationReasonCode::DiscardHumanOnly),
                    "discard is for the human only"
                );
            }
            assert_eq!(
                fixture.call("merge", &[("child", "reported")]).await,
                DelegationResponse::failure("merge takes exactly these string arguments: child, expected_head.")
            );
            assert_eq!(
                fixture.call("child_status", &[]).await.error.as_deref(),
                Some("child_status takes exactly these string arguments: child.")
            );
            assert_eq!(
                fixture.call("list_children", &[("child", "reported")]).await.error.as_deref(),
                Some("list_children takes no arguments.")
            );
            assert_eq!(
                fixture.call("follow_up", &[("child", "reported")]).await.error.as_deref(),
                Some("follow_up takes exactly these string arguments: child, text.")
            );
            assert!(
                fixture.bytes() == bytes
                    && fixture.file() == file
                    && fixture.host.calls().is_empty()
                    && off.host.calls().is_empty()
                    && !Path::new(&fixture.place.root()).exists(),
                "the gates and refusals changed nothing and reached no pane"
            );

            let mine = fixture.call("list_children", &[]).await;
            let expected = [
                ("running", ChildState::Running, 0),
                ("reported", ChildState::Reported, 2),
                ("failed", ChildState::Failed, 0),
                ("closed", ChildState::Closed, 3),
                ("discarded", ChildState::Discarded, 0),
            ];
            assert!(mine.refused.is_none() && mine.error.is_none() && mine.child.is_none() && mine.status.is_none());
            assert!(
                mine.children.unwrap().iter().map(|c| (c.id.as_str(), c.state, c.revision)).eq(expected),
                "the caller's children in the order they were made"
            );
            assert_eq!(
                fixture.call_from("other", "list_children", &[]).await.children.unwrap(),
                vec![mighty_delegation::request::DelegationChildSummary::of(&children[2])]
            );
            assert!(fixture.call_from("lonely", "list_children", &[]).await.children.unwrap().is_empty(), "each parent sees only its own");

            let answer = fixture.call("child_status", &[("child", "reported")]).await;
            let status = answer.status.clone().unwrap();
            assert!(answer.refused.is_none() && answer.error.is_none() && answer.child.is_none() && answer.children.is_none());
            assert!(
                status.id == "reported"
                    && status.state == ChildState::Reported
                    && status.revision == 2
                    && !status.report_truncated
                    && status.head.as_deref() == Some(reported_head.as_str())
                    && status.report.as_deref() == Some(report_text),
                "the state, head and report body"
            );
            let running = fixture.call("child_status", &[("child", "running")]).await.status.unwrap();
            assert!(
                running.state == ChildState::Running
                    && running.revision == 0
                    && running.head.is_none()
                    && running.report.is_none()
                    && !running.report_truncated,
                "a child that has not reported"
            );
            let closed = fixture.call("child_status", &[("child", " closed\n")]).await.status.unwrap();
            assert!(
                closed.state == ChildState::Closed
                    && closed.revision == 3
                    && closed.report.is_none()
                    && closed.head.as_deref() == Some("d".repeat(40).as_str()),
                "a closed child whose copy was pruned"
            );
            assert!(
                fixture
                    .call_from("other", "child_status", &[("child", "merged")])
                    .await
                    .status
                    .is_some_and(|s| s.state == ChildState::Merged && s.report.as_deref() == Some("Merged work."))
            );
            assert_eq!(fixture.call("child_status", &[("child", "discarded")]).await.status.map(|s| s.state), Some(ChildState::Discarded));
            assert_eq!(
                fixture.call("child_status", &[("child", "failed")]).await.status.map(|s| s.state),
                Some(ChildState::Failed),
                "every state is read"
            );
            for (id, from) in
                [("someone-elses", "parent"), ("reported", "other"), ("no-such-child", "parent"), ("", "parent"), ("reported", "lonely")]
            {
                assert_eq!(
                    fixture.call_from(from, "child_status", &[("child", id)]).await,
                    DelegationResponse::failure(not_your_child_message(id)),
                    "{from} may not read {id}"
                );
            }
            assert_eq!(not_your_child_message("x"), "No child \"x\" of yours. list_children names your children.");
            assert!(
                fixture.bytes() == bytes && fixture.file() == file && fixture.host.calls().is_empty(),
                "reading changed nothing and reached no pane"
            );
            for tool in tools::ALL {
                let clock = std::time::Instant::now();
                let given = fixture.call(tool.name, &arguments(tool)).await;
                assert!(
                    clock.elapsed().as_secs_f64() < mighty_delegation::coordinator::ANSWER_SECONDS
                        && (given.refused.is_some()
                            || given.error.is_some()
                            || given.children.is_some()
                            || given.status.is_some()
                            || given.merged.is_some()
                            || given.follow_up.is_some())
                        && given != DelegationResponse::failure(DETACHED_MESSAGE)
                        && given.refused != Some(DelegationReasonCode::ClaudeOnly)
                        && given.error.as_deref() != Some(format!("Unknown delegation tool {}.", tool.name).as_str()),
                    "{} is answered by its tool",
                    tool.name
                );
            }
            drop((off, fixture));
        })
        .await;

        Box::pin(async {
            // A long report is the marked copy, and a stale copy is no report.
            let long_report = "line of the report\n".repeat(5_000);
            let fixture = make(
                None,
                Some(DelegationFile::of(
                    vec![
                        with(record("long", ChildState::Reported, "parent"), 1, Some(reported_head.clone())),
                        with(record("stale", ChildState::Reported, "parent"), 2, Some(reported_head.clone())),
                    ],
                    vec![],
                    vec![],
                    vec![],
                    vec![
                        copy("long", DelegationCopyKind::Report, &long_report, Some(1)),
                        copy("stale", DelegationCopyKind::Report, "The first report.", Some(1)),
                    ],
                )),
                "auto",
                50_000_000_000,
                40,
            )
            .await;
            let status = fixture.call("child_status", &[("child", "long")]).await.status.unwrap();
            let body = status.report.clone().unwrap();
            assert!(
                status.report_truncated
                    && body.ends_with(TRUNCATION_MARKER)
                    && long_report.starts_with(&body[..body.len() - TRUNCATION_MARKER.len()])
                    && body.len() <= MAXIMUM_COPY_BYTES,
                "a long report is the marked copy"
            );
            let stale = fixture.call("child_status", &[("child", "stale")]).await.status.unwrap();
            assert!(
                stale.revision == 2
                    && stale.report.is_none()
                    && !stale.report_truncated
                    && stale.head.as_deref() == Some(reported_head.as_str()),
                "a copy of an earlier revision is not the current report"
            );
        })
        .await;
    }

    /// "delegation: the merge tool takes turns, and follow-ups stop at two"
    #[tokio::test]
    async fn the_merge_tool_takes_turns_and_follow_ups_stop_at_two() {
        // Its future is large in a debug build: on the heap, not the test thread's stack.
        Box::pin(the_merge_tool_takes_turns_and_follow_ups_stop_at_two_body()).await;
    }

    async fn the_merge_tool_takes_turns_and_follow_ups_stop_at_two_body() {
        let _lane = lane::enter().await;
        Box::pin(async {
            // merge fast-forwards a reported child and writes its MergeRecord.
            let place = DelegationPlace::new();
            let start = place.repository_default().await;
            let (child, head) = place.reported_child("c1", &["a.txt", "b.txt"], Some(&place.repo())).await;
            let fixture = tools_over(place, vec![record_of(&child)], 40);
            let answer = fixture.call("merge", &[("child", " c1\n"), ("expected_head", &head)]).await;
            let merged = answer.merged.clone().expect("merged");
            assert!(
                answer.refused.is_none()
                    && answer.error.is_none()
                    && merged == MergeRecord::new("c1", MergeKind::ToolFastForward, "main", &start, &head, &head),
                "the fast-forward's record is the answer"
            );
            let place = &fixture.place;
            assert!(
                place.fixture_git(&["symbolic-ref", "HEAD"]).await == "refs/heads/main"
                    && place.fixture_git(&["rev-parse", "HEAD"]).await == head
                    && place.fixture_git(&["rev-list", "--merges", "HEAD"]).await.is_empty()
                    && place.fixture_git(&["status", "--porcelain", "--untracked-files=no"]).await.is_empty()
                    && place.fixture_git(&["rev-parse", "refs/heads/mighty/c1"]).await == head,
                "one fast-forward, and the child's branch is only read"
            );
            let file = fixture.file();
            assert!(
                file.merges == [merged.clone()]
                    && file.children[0].state == ChildState::Merged
                    && fixture.loaded() == file
                    && fixture.relaunch().file().merges == [merged.clone()],
                "the record is saved with the child merged"
            );
            let (bytes, before) = (fixture.bytes(), place.snapshot(&[&child.worktree_path]).await);
            assert!(
                fixture.call("merge", &[("child", "c1"), ("expected_head", &head)]).await
                    == DelegationResponse::refusal(DelegationReasonCode::NotReported)
                    && fixture.bytes() == bytes
                    && place.snapshot(&[&child.worktree_path]).await == before
                    && fixture.file() == file,
                "a second merge is refused"
            );
            drop(fixture);
        })
        .await;

        Box::pin(async {
            // Each merge refusal is answered with its reason code and changes nothing.
            let place = DelegationPlace::new();
            place.repository_default().await;
            let repo = place.repo();
            let running = place.child("running", Some(&repo)).await;
            let (moved, moved_head) = place.reported_child("moved", &[], Some(&repo)).await;
            let (dirty, dirty_head) = place.reported_child("dirty", &[], Some(&repo)).await;
            let (gone, gone_head) = place.reported_child("gone", &[], Some(&repo)).await;
            let (first, first_head) = place.reported_child("first", &[], Some(&repo)).await;
            let (late, late_head) = place.reported_child("late", &[], Some(&repo)).await;
            let fixture = tools_over(place, [&running, &moved, &dirty, &gone, &first, &late].iter().map(|c| record_of(c)).collect(), 40);
            std::fs::write(Path::new(&dirty.worktree_path).join("dirty.txt"), "edited\n").unwrap();
            remove_folder(Path::new(&gone.worktree_path));
            let refuses = |id: &'static str, expected: String, reason: DelegationReasonCode, look: String| {
                let fixture = &fixture;
                async move {
                    let (bytes, file, before) = (fixture.bytes(), fixture.file(), fixture.place.snapshot(&[&look]).await);
                    assert_eq!(
                        fixture.call("merge", &[("child", id), ("expected_head", &expected)]).await,
                        DelegationResponse::refusal(reason),
                        "{id}"
                    );
                    assert!(
                        fixture.bytes() == bytes && fixture.file() == file && fixture.place.snapshot(&[&look]).await == before,
                        "{id}: nothing changed"
                    );
                }
            };
            refuses("running", running.base_commit.clone(), DelegationReasonCode::NotReported, running.worktree_path.clone()).await;
            refuses("moved", running.base_commit.clone(), DelegationReasonCode::HeadMoved, moved.worktree_path.clone()).await;
            refuses("dirty", dirty_head, DelegationReasonCode::TrackedChanges, running.worktree_path.clone()).await;
            refuses("gone", gone_head, DelegationReasonCode::WorktreeMissing, running.worktree_path.clone()).await;
            let later = fixture.place.commit(&moved.worktree_path, "m2.txt", "later\n").await;
            refuses("moved", moved_head.clone(), DelegationReasonCode::HeadMoved, moved.worktree_path.clone()).await;
            refuses("moved", later, DelegationReasonCode::HeadMoved, moved.worktree_path.clone()).await;
            fixture.place.git_in(&moved.worktree_path, &["reset", "-q", "--hard", &moved_head]).await;
            std::fs::write(Path::new(&repo).join("file-0.txt"), "parent edit\n").unwrap();
            refuses("moved", moved_head.clone(), DelegationReasonCode::TrackedChanges, moved.worktree_path.clone()).await;
            for step in [
                &["checkout", "-q", "--", "file-0.txt"][..],
                &["checkout", "-q", "--orphan", "elsewhere"],
                &["rm", "-q", "-r", "--cached", "."],
                &["commit", "-q", "--allow-empty", "-m", "elsewhere"],
            ] {
                fixture.place.fixture_git(step).await;
            }
            refuses("moved", moved_head.clone(), DelegationReasonCode::BranchNotCheckedOut, moved.worktree_path.clone()).await;
            fixture.place.fixture_git(&["clean", "-q", "-f"]).await;
            fixture.place.fixture_git(&["checkout", "-q", "main"]).await;
            assert_eq!(
                fixture.call("merge", &[("child", "first"), ("expected_head", &first_head)]).await.merged.map(|m| m.merged_commit),
                Some(first_head.clone()),
                "the first child merges"
            );
            refuses("late", late_head.clone(), DelegationReasonCode::Diverged, late.worktree_path.clone()).await;
            assert!(
                fixture.place.fixture_git(&["rev-parse", "HEAD"]).await == first_head
                    && fixture.file().merges.iter().map(|m| m.child_id.as_str()).eq(["first"]),
                "only the first is merged"
            );
            let saved = fixture.bytes();
            for (id, from) in [("late", "other"), ("no-such-child", "parent")] {
                assert_eq!(
                    fixture.call_from(from, "merge", &[("child", id), ("expected_head", &late_head)]).await,
                    DelegationResponse::failure(not_your_child_message(id)),
                    "{from} may not merge {id}"
                );
            }
            assert_eq!(fixture.bytes(), saved, "nothing changed");
            assert!(
                matches!(
                    fixture.coordinator.merge_from_card("late").await,
                    ChildMergeOutcome::Merged(MergeRecord { kind: MergeKind::CardMergeCommit, .. })
                ) && fixture.child("late").state == ChildState::Merged,
                "the card merges the diverged sibling"
            );
            drop(fixture);
        })
        .await;

        Box::pin(async {
            // Merges at once take turns, so exactly one fast-forwards.
            let place = DelegationPlace::new();
            place.repository_default().await;
            let (one, one_head) = place.reported_child("one", &[], Some(&place.repo())).await;
            let (two, two_head) = place.reported_child("two", &[], Some(&place.repo())).await;
            let fixture = tools_over(place, vec![record_of(&one), record_of(&two)], 40);
            let one_call = || fixture.owned_call("merge", vec![("child", "one".into()), ("expected_head", one_head.clone())]);
            let (a, b, c, d) = tokio::join!(
                one_call(),
                one_call(),
                fixture.owned_call("merge", vec![("child", "two".into()), ("expected_head", two_head.clone())]),
                one_call()
            );
            let answers = [a, b, c, d];
            assert!(
                answers.iter().filter(|a| a.merged.is_some()).count() == 1
                    && answers
                        .iter()
                        .filter(|a| a.merged.is_none())
                        .all(|a| matches!(a.refused, Some(DelegationReasonCode::NotReported | DelegationReasonCode::Diverged)))
                    && fixture.file().merges.len() == 1
                    && fixture.file().children.iter().filter(|c| c.state == ChildState::Merged).count() == 1
                    && fixture.place.fixture_git(&["rev-parse", "HEAD"]).await == fixture.file().merges[0].merged_commit,
                "exactly one fast-forward: {answers:?}"
            );
            drop(fixture);
        })
        .await;

        Box::pin(async {
            // A tool merge whose save fails tells the parent it stands, and the next save writes it.
            let place = DelegationPlace::new();
            place.repository_default().await;
            let (child, head) = place.reported_child("c1", &[], Some(&place.repo())).await;
            let fixture = tools_over(place, vec![record_of(&child)], 2);
            let unblock = block_saves(&fixture.store());
            let error = fixture.call("merge", &[("child", "c1"), ("expected_head", &head)]).await.error.unwrap();
            assert!(
                error.starts_with(&format!("The child is merged: main now points at {head}, and the app keeps the merge on record, but the delegation file could not be saved ("))
                    && error.ends_with("). Mighty Code saves it again by itself; nothing is needed from you. Do not merge this child again."),
                "the parent is told the merge stands: {error}"
            );
            assert!(fixture.place.fixture_git(&["rev-parse", "HEAD"]).await == head && fixture.file().merges.iter().map(|m| m.child_id.as_str()).eq(["c1"]) && fixture.file().children[0].state == ChildState::Merged, "the merge is on record in memory");
            assert_eq!(fixture.call("merge", &[("child", "c1"), ("expected_head", &head)]).await.refused, Some(DelegationReasonCode::NotReported), "and is not merged again");
            unblock();
            assert!(fixture.loaded().merges.is_empty(), "the disk does not have it yet");
            fixture.coordinator.save().await.unwrap();
            assert!(fixture.loaded() == fixture.file() && fixture.relaunch().file().merges.len() == 1, "the next save writes it");
            drop(fixture);
        })
        .await;

        Box::pin(async {
            // follow_up records at most two per child, and refuses the third with follow_up_limit.
            let fixture = tools_over(
                DelegationPlace::new(),
                vec![record("c1", ChildState::Running, "parent"), record("c2", ChildState::Reported, "parent")],
                40,
            );
            let first = fixture.call("follow_up", &[("child", "c1"), ("text", "Also add tests.")]).await;
            let info = first.follow_up.clone().unwrap();
            assert!(
                first.refused.is_none()
                    && first.error.is_none()
                    && info.child == "c1"
                    && info.lane == DeliveryLane::Held
                    && info.remaining == 1
                    && !info.id.is_empty(),
                "the first follow-up is recorded"
            );
            let file = fixture.file();
            assert!(
                file.follow_ups == [FollowUp::new(&info.id, "c1", "Also add tests.", DeliveryLane::Held)]
                    && file.children.iter().map(|c| c.follow_up_count).eq([1, 0])
                    && fixture.loaded() == file,
                "written to disk before the answer"
            );
            let words = "그리고 문서도 고쳐 줘.\n";
            let second = fixture.call("follow_up", &[("child", " c1\n"), ("text", words)]).await.follow_up.unwrap();
            assert!(
                second.remaining == 0
                    && second.id != info.id
                    && fixture.call("follow_up", &[("child", "c2"), ("text", "Rebase on main.")]).await.follow_up.map(|f| f.remaining)
                        == Some(1)
            );
            let file = fixture.file();
            assert!(
                file.follow_ups.iter().map(|f| f.text.as_str()).eq(["Also add tests.", words, "Rebase on main."])
                    && file.follow_ups.iter().map(|f| f.child_id.as_str()).eq(["c1", "c1", "c2"])
                    && file.follow_ups.iter().all(|f| f.lane == DeliveryLane::Held && f.receipt.is_none())
                    && file.children.iter().map(|c| c.follow_up_count).eq([2, 1]),
                "each is kept exactly as given"
            );
            let bytes = fixture.bytes();
            assert!(
                fixture.call("follow_up", &[("child", "c1"), ("text", "One more thing.")]).await
                    == DelegationResponse::refusal(DelegationReasonCode::FollowUpLimit)
                    && fixture.bytes() == bytes
                    && fixture.file() == file,
                "the third is refused"
            );
            let relaunched = fixture.relaunch();
            assert!(
                relaunched.file() == file
                    && relaunched
                        .handle_call(
                            request("follow_up", &[("child", "c1"), ("text", "Again.")]),
                            caller("parent", "claude", "claude", true)
                        )
                        .await
                        == DelegationResponse::refusal(DelegationReasonCode::FollowUpLimit)
                    && fixture.bytes() == bytes,
                "a relaunch keeps the follow-ups and the limit"
            );
            drop((relaunched, fixture));
        })
        .await;

        Box::pin(async {
            // Follow-ups at once never pass the limit.
            let fixture = tools_over(DelegationPlace::new(), vec![record("c1", ChildState::Running, "parent")], 40);
            let texts: Vec<String> = (0..6).map(|index| format!("Follow-up {index}")).collect();
            let answers = futures_join(
                texts.iter().map(|text| fixture.owned_call("follow_up", vec![("child", "c1".into()), ("text", text.clone())])),
            )
            .await;
            let mut remaining: Vec<i32> = answers.iter().filter_map(|a| a.follow_up.as_ref().map(|f| f.remaining)).collect();
            remaining.sort();
            assert!(
                answers.iter().filter(|a| a.refused == Some(DelegationReasonCode::FollowUpLimit)).count() == 4
                    && fixture.file().follow_ups.len() == 2
                    && fixture.file().children[0].follow_up_count == 2
                    && remaining == [0, 1],
                "six at once record exactly two"
            );
            drop(fixture);
        })
        .await;

        Box::pin(async {
            // follow_up is refused for a closed child or a closed parent, and changes nothing.
            let full = ChildRecord { follow_up_count: 2, ..record("full", ChildState::Running, "parent") };
            let fixture = tools_over(
                DelegationPlace::new(),
                vec![
                    record("closed", ChildState::Closed, "parent"),
                    record("discarded", ChildState::Discarded, "parent"),
                    record("failed", ChildState::Failed, "parent"),
                    record("open", ChildState::Reported, "parent"),
                    full,
                    record("theirs", ChildState::Running, "other"),
                ],
                40,
            );
            let (bytes, file) = (fixture.bytes(), fixture.file());
            for id in ["closed", "discarded", "failed"] {
                assert_eq!(
                    fixture.call("follow_up", &[("child", id), ("text", "Go on.")]).await.refused,
                    Some(DelegationReasonCode::ChildClosed),
                    "{id} is not open"
                );
            }
            assert_eq!(
                fixture.call("follow_up", &[("child", "full"), ("text", "Go on.")]).await,
                DelegationResponse::refusal(DelegationReasonCode::FollowUpLimit)
            );
            assert_eq!(
                fixture.call("follow_up", &[("child", "theirs"), ("text", "Go on.")]).await,
                DelegationResponse::failure(not_your_child_message("theirs"))
            );
            assert_eq!(
                fixture.call_from("other", "follow_up", &[("child", "open"), ("text", "Go on.")]).await,
                DelegationResponse::failure(not_your_child_message("open"))
            );
            assert_eq!(
                fixture.call("follow_up", &[("child", "no-such-child"), ("text", "Go on.")]).await,
                DelegationResponse::failure(not_your_child_message("no-such-child"))
            );
            assert_eq!(
                fixture.call("follow_up", &[("child", "open"), ("text", " \n")]).await.error.as_deref(),
                Some("follow_up needs a text: the instruction for the child.")
            );
            assert_eq!(
                fixture.call("follow_up", &[("child", "open"), ("text", &"x".repeat(MAXIMUM_FOLLOW_UP_BYTES + 1))]).await.error.as_deref(),
                Some("A follow-up's text may be at most 48 KiB.")
            );
            assert_eq!(
                fixture
                    .call("follow_up", &[("child", "open"), ("text", &"\"".repeat(MAXIMUM_FOLLOW_UP_BYTES / 2 + 1))])
                    .await
                    .error
                    .as_deref(),
                Some("A follow-up's text may be at most 48 KiB."),
                "measured as it is stored"
            );
            assert_eq!(
                fixture.call("follow_up", &[("child", "open"), ("text", "Go on.\0Then stop.")]).await.error.as_deref(),
                Some("A follow-up's text may not contain a NUL character.")
            );
            fixture.host.close("parent");
            for id in ["open", "full", "closed"] {
                assert_eq!(
                    fixture.call("follow_up", &[("child", id), ("text", "Go on.")]).await,
                    DelegationResponse::refusal(DelegationReasonCode::ParentClosed),
                    "a closed parent sends nothing more to {id}"
                );
            }
            assert!(
                fixture.bytes() == bytes
                    && fixture.file() == file
                    && !fixture
                        .host
                        .calls()
                        .iter()
                        .any(|c| c.starts_with("deliver") || c.starts_with("startRun") || c.starts_with("createPane")),
                "nothing changed, and no pane was handed anything"
            );
        })
        .await;
    }

    /// Every future at once, their answers in order.
    async fn futures_join<F: std::future::Future>(futures: impl Iterator<Item = F>) -> Vec<F::Output> {
        let pinned: Vec<std::pin::Pin<Box<F>>> = futures.map(Box::pin).collect();
        let mut slots: Vec<Option<F::Output>> = pinned.iter().map(|_| None).collect();
        let mut pinned: Vec<Option<std::pin::Pin<Box<F>>>> = pinned.into_iter().map(Some).collect();
        std::future::poll_fn(|context| {
            let mut pending = false;
            for (slot, future) in slots.iter_mut().zip(pinned.iter_mut()) {
                if let Some(running) = future {
                    match running.as_mut().poll(context) {
                        std::task::Poll::Ready(value) => {
                            *slot = Some(value);
                            *future = None;
                        }
                        std::task::Poll::Pending => pending = true,
                    }
                }
            }
            if pending { std::task::Poll::Pending } else { std::task::Poll::Ready(()) }
        })
        .await;
        slots.into_iter().map(Option::unwrap).collect()
    }

    /// `Card`: a repository on main, and a coordinator over a child c1 made the app's way that committed a.txt and reported it, with
    /// its parent's open pane idle in the repository.
    async fn card(records_checkout: bool, attempts: u32) -> (Fixture, String, String) {
        let place = DelegationPlace::new();
        let start = place.repository_default().await;
        let checkout = place.repo();
        let (child, head) = place.reported_child("c1", &["a.txt"], records_checkout.then_some(checkout.as_str())).await;
        let fixture = tools_over(place, vec![record_of(&child)], attempts);
        fixture.set_parent("auto", "run-1", Finished);
        (fixture, start, head)
    }

    fn actions(fixture: &Fixture) -> Vec<Vec<CardAction>> {
        sidebar::rows(&fixture.file()).iter().map(|r| r.card_actions()).collect()
    }

    /// "delegation: a card merges, undoes and discards, in the app's language"
    #[tokio::test]
    async fn a_card_merges_undoes_and_discards_in_the_apps_language() {
        // Its future is large in a debug build: on the heap, not the test thread's stack.
        Box::pin(a_card_merges_undoes_and_discards_in_the_apps_language_body()).await;
    }

    async fn a_card_merges_undoes_and_discards_in_the_apps_language_body() {
        let _lane = lane::enter().await;
        Box::pin(async {
            let (fixture, start, head) = card(true, 40).await;
            let coordinator = fixture.coordinator.clone();
            assert_eq!(
                actions(&fixture),
                [vec![CardAction::Merge, CardAction::Discard]],
                "a reported child's card offers merge and discard"
            );
            let ChildMergeOutcome::Merged(merged) = coordinator.merge_from_card("c1").await else { panic!("not merged") };
            assert!(
                merged == MergeRecord::new("c1", MergeKind::CardFastForward, "main", &start, &head, &head)
                    && fixture.place.fixture_git(&["rev-parse", "HEAD"]).await == head
                    && fixture.file().children[0].state == ChildState::Merged
                    && fixture.file().merges == [merged.clone()]
                    && fixture.loaded().merges == [merged.clone()]
                    && sidebar::rows(&fixture.file())[0].can_undo
                    && actions(&fixture) == [vec![CardAction::Undo, CardAction::Discard]],
                "the card's merge is on record, and the card offers undo"
            );
            fixture.set_parent("auto", "run-2", Running);
            assert!(
                coordinator.undo_merge_from_card("c1").await == ChildUndoOutcome::Refused(DelegationReasonCode::ParentBusy)
                    && fixture.place.fixture_git(&["rev-parse", "HEAD"]).await == head
                    && fixture.file().merges == [merged.clone()],
                "a running parent refuses the undo"
            );
            fixture.set_parent("auto", "run-2", Finished);
            assert!(
                coordinator.undo_merge_from_card("c1").await == ChildUndoOutcome::Undone { record: merged.clone(), restored_branch: false }
                    && fixture.place.fixture_git(&["rev-parse", "HEAD"]).await == start
                    && fixture.file().merges.is_empty()
                    && fixture.file().children[0].state == ChildState::Reported
                    && fixture.loaded().merges.is_empty()
                    && actions(&fixture) == [vec![CardAction::Merge, CardAction::Discard]]
                    && matches!(coordinator.undo_merge_from_card("c1").await, ChildUndoOutcome::Failed(_)),
                "the undo takes the record out, and there is no second undo"
            );
            let before = fixture.file();
            let repo = fixture.place.repo();
            std::fs::write(Path::new(&repo).join("file-0.txt"), "changed\n").unwrap();
            assert_eq!(
                coordinator.merge_from_card("c1").await,
                ChildMergeOutcome::Refused(DelegationReasonCode::TrackedChanges),
                "tracked changes in the parent's folder"
            );
            for step in [
                &["checkout", "--", "file-0.txt"][..],
                &["checkout", "-q", "--orphan", "elsewhere"],
                &["rm", "-q", "-r", "--cached", "."],
                &["commit", "-q", "--allow-empty", "-m", "elsewhere"],
            ] {
                fixture.place.fixture_git(step).await;
            }
            assert_eq!(
                coordinator.merge_from_card("c1").await,
                ChildMergeOutcome::Refused(DelegationReasonCode::BranchNotCheckedOut),
                "another branch checked out in the parent's folder"
            );
            fixture.place.fixture_git(&["clean", "-q", "-f"]).await;
            fixture.place.fixture_git(&["checkout", "-q", "main"]).await;
            assert!(
                fixture.place.fixture_git(&["rev-parse", "HEAD"]).await == start
                    && fixture.file() == before
                    && matches!(coordinator.merge_from_card("nobody").await, ChildMergeOutcome::Failed(_)),
                "nothing changed, and nobody is not merged"
            );

            // A card's failure reads in the app's language: each language's own words.
            let mut said = HashSet::new();
            for chosen in mighty_core::locale::LANGUAGES {
                host::set_language(chosen);
                let words = locale();
                assert_eq!(
                    coordinator.undo_merge_from_card("c1").await,
                    ChildUndoOutcome::Failed(words.get("delegation.card.error.noMergeToUndo"))
                );
                assert_eq!(
                    coordinator.discard_from_card("nobody", Some(&[])).await,
                    ChildCardDiscardOutcome::Failed(words.get("delegation.card.error.cannotDiscard"))
                );
                let ChildMergeOutcome::Failed(message) = coordinator.merge_from_card("nobody").await else { panic!("merged nobody") };
                assert!(message == words.get("delegation.card.error.noChild") && said.insert(message), "the card's words in {chosen}");
            }
            host::set_language("en");
            assert_eq!(said.len(), 4, "four languages, four texts");
            let en = locale();
            assert!(
                unsaved_reason(&SaveError::File(DelegationFileError::Full), &en) == en.get("delegation.card.error.fileFull")
                    && unsaved_reason(&SaveError::File(DelegationFileError::Unreadable), &en)
                        == en.get("delegation.card.error.fileUnreadable")
                    && mighty_delegation::tools::actions::unsaved_reason_for_agent(&SaveError::File(DelegationFileError::Full))
                        == "the delegation file is at its 4 MiB cap"
                    && mighty_delegation::tools::actions::unsaved_reason_for_agent(&SaveError::File(DelegationFileError::Unreadable))
                        == "the delegation file on disk cannot be read"
                    && unsaved_reason(&SaveError::Io(std::io::Error::other("Access to the path is denied.\r\n")), &en)
                        == "Access to the path is denied",
                "an unsaved file's reason reads as words for the card and the agent, with no closing stop"
            );

            // The discard confirmation gets the worktrees nested in the child's, and asks again when one was nested since.
            let child = fixture.file().children[0].clone();
            assert!(
                coordinator.nested_worktrees("c1").await == Some(vec![]) && coordinator.nested_worktrees("nobody").await.is_none(),
                "no worktree is nested yet"
            );
            let nested = Path::new(&child.worktree_path).join("inner").to_string_lossy().into_owned();
            fixture.place.fixture_git(&["worktree", "add", "-q", "-b", "inner", &nested]).await;
            let canonical = super::support::place::paths();
            let named = coordinator.nested_worktrees("c1").await.unwrap();
            assert!(named.len() == 1 && paths::same(&named[0], &canonical.canonical(&nested)), "the nested worktree is named");
            let ChildCardDiscardOutcome::NestedChanged(all) = coordinator.discard_from_card("c1", Some(&[])).await else {
                panic!("discarded with a nested worktree unnamed")
            };
            assert!(
                all.len() == 1
                    && paths::same(&all[0], &canonical.canonical(&nested))
                    && Path::new(&child.worktree_path).exists()
                    && Path::new(&nested).exists()
                    && fixture.place.fixture_git(&["rev-parse", "--verify", "-q", "refs/heads/mighty/c1"]).await.len() == 40
                    && fixture.file().children[0].state == ChildState::Reported,
                "nested after the confirmation named none: nothing is removed, and the answer names it"
            );
            let written = canonical.canonical(&nested).to_uppercase().replace('\\', "/");
            assert!(
                coordinator.discard_from_card("c1", Some(&[written])).await == ChildCardDiscardOutcome::Discarded
                    && !Path::new(&child.worktree_path).exists()
                    && fixture.place.mighty_branches().await.is_empty()
                    && fixture.file().children[0].state == ChildState::Discarded
                    && sidebar::rows(&fixture.file()).is_empty()
                    && fixture.place.fixture_git(&["rev-parse", "refs/heads/inner"]).await == start,
                "confirmed with it named, they go, and the nested worktree's branch stays"
            );
            drop(fixture);
        })
        .await;

        Box::pin(async {
            // A merge or undo whose save fails stands, stays on record in memory, and the next save writes it.
            let (fixture, start, head) = card(true, 2).await;
            let coordinator = fixture.coordinator.clone();
            let unblock = block_saves(&fixture.store());
            let ChildMergeOutcome::MergedUnsaved(unsaved, reason) = coordinator.merge_from_card("c1").await else {
                panic!("not merged unsaved")
            };
            assert!(
                !reason.is_empty()
                    && !reason.ends_with('.')
                    && fixture.place.fixture_git(&["rev-parse", "HEAD"]).await == head
                    && fixture.file().merges == [unsaved.clone()]
                    && sidebar::rows(&fixture.file())[0].state == ChildState::Merged,
                "the merge stands and is on record in memory"
            );
            unblock();
            assert!(fixture.loaded().merges.is_empty(), "the disk does not have it yet");
            coordinator.save().await.unwrap();
            assert_eq!(fixture.loaded().merges, std::slice::from_ref(&unsaved), "the next save writes the merge");
            let unblock = block_saves(&fixture.store());
            let ChildUndoOutcome::UndoneUnsaved { record: undone, restored_branch: false, reason } =
                coordinator.undo_merge_from_card("c1").await
            else {
                panic!("not undone unsaved")
            };
            assert!(
                !reason.is_empty()
                    && undone == unsaved
                    && fixture.file().merges.is_empty()
                    && fixture.place.fixture_git(&["rev-parse", "HEAD"]).await == start,
                "the undo stands too"
            );
            unblock();
            assert_eq!(fixture.loaded().merges, [unsaved], "the disk still has the merge until the next save");
            drop(fixture);
        })
        .await;

        Box::pin(async {
            // A discard that cannot list the nested worktrees leaves the child's run alone.
            let (fixture, _, _) = card(true, 40).await;
            let coordinator = fixture.coordinator.clone();
            let child = fixture.file().children[0].clone();
            fixture.host.set(pane("c1", "acceptEdits", &child.worktree_path, Some("parent"), Some("c1-run"), Running));
            let moved = Path::new(&fixture.place.base()).join("repo-moved");
            std::fs::rename(fixture.place.repo(), &moved).unwrap();
            assert!(coordinator.nested_worktrees("c1").await.is_none());
            assert_eq!(
                coordinator.discard_from_card("c1", None).await,
                ChildCardDiscardOutcome::Failed(locale().get("delegation.card.error.nestedUnlisted"))
            );
            assert!(
                fixture.host.stops().is_empty()
                    && fixture.host.pane("c1").map(|p| p.activity) == Some(Running)
                    && Path::new(&child.worktree_path).exists()
                    && fixture.file().children[0].state == ChildState::Reported,
                "its run is never stopped, and nothing is removed"
            );
            std::fs::rename(&moved, fixture.place.repo()).unwrap();
            assert_eq!(
                coordinator.discard_from_card("c1", Some(&[])).await,
                ChildCardDiscardOutcome::Discarded,
                "with the folder back, the discard goes on"
            );
            let mut stops = fixture.host.stops();
            stops.dedup();
            assert!(
                stops == ["c1 with its folder"]
                    && fixture.host.pane("c1").map(|p| p.activity) == Some(Idle)
                    && !Path::new(&child.worktree_path).exists()
                    && fixture.file().children[0].state == ChildState::Discarded
            );
            drop(fixture);
        })
        .await;

        Box::pin(async {
            // Delivery starts no run in a pane whose checkout a card is changing; the notice goes on once the card is done.
            let (fixture, _, head) = card(false, 40).await;
            let mut file = fixture.file();
            file.children
                .push(ChildRecord { awaited_run_id: Some("r-c2".into()), ..with_run(record("c2", ChildState::Running, "parent"), "r-c2") });
            fixture.store().save(&file).unwrap();
            let coordinator = fixture.relaunch();
            let gate = Gate::new();
            let asked = Arc::new(AtomicUsize::new(0));
            let (waiting, counted) = (gate.clone(), asked.clone());
            fixture.host.set_state_gate(Some(move |_: String| -> mighty_platform::BoxFuture<'static, ()> {
                let first = counted.fetch_add(1, Ordering::SeqCst) == 0;
                let waiting = waiting.clone();
                Box::pin(async move {
                    if first {
                        waiting.wait().await;
                    }
                })
            }));
            let merging = tokio::spawn({
                let coordinator = coordinator.clone();
                async move { coordinator.merge_from_card("c1").await }
            });
            assert!(eventually(60_000, || asked.load(Ordering::SeqCst) >= 1).await, "the card's merge is under way");
            let notice = coordinator.child_run_ended("c2", "r-c2", ChildRunEnd::Finished).await.expect("a notice");
            assert!(
                fixture.host.all_handed().is_empty()
                    && coordinator.file().notices[0].lane == DeliveryLane::Pending
                    && !merging.is_finished(),
                "no run starts in the parent's pane while the card changes its checkout"
            );
            gate.open();
            assert!(
                matches!(merging.await.unwrap(), ChildMergeOutcome::Merged(_))
                    && fixture.place.fixture_git(&["rev-parse", "HEAD"]).await == head,
                "the card's merge finishes"
            );
            let handed = fixture.host.all_handed();
            assert!(
                handed.len() == 1
                    && handed[0].session_id == "parent"
                    && handed[0].route == Some(DeliveryRoute::Queue)
                    && handed[0].input == notice_text(&notice)
                    && coordinator.file().notices[0].lane == DeliveryLane::Delivered,
                "the notice starts the parent's next run once the card is done"
            );
        })
        .await;
    }

    /// "delegation: closing stores copies before cleanup, and a discard that fails says so"
    #[tokio::test]
    async fn closing_stores_copies_before_cleanup_and_a_discard_that_fails_says_so() {
        // Its future is large in a debug build: on the heap, not the test thread's stack.
        Box::pin(closing_stores_copies_before_cleanup_and_a_discard_that_fails_says_so_body()).await;
    }

    async fn closing_stores_copies_before_cleanup_and_a_discard_that_fails_says_so_body() {
        let _lane = lane::enter().await;
        Box::pin(async {
            let place = DelegationPlace::new();
            let start = place.repository_default().await;
            let (child, merge) = merged_child(&place, "c1").await;
            std::fs::OpenOptions::new()
                .append(true)
                .open(Path::new(&place.repo()).join(".git").join("info").join("exclude"))
                .and_then(|mut f| std::io::Write::write_all(&mut f, b"build/\n"))
                .unwrap();
            std::fs::create_dir_all(Path::new(&child.worktree_path).join("build")).unwrap();
            std::fs::write(Path::new(&child.worktree_path).join("build").join("out.o"), "o\n").unwrap();
            let store = DelegationFileStore::new(place.profile());
            store.save(&DelegationFile::of(vec![child.clone()], vec![], vec![], vec![merge.clone()], vec![])).unwrap();
            let test_host = TestHost::new(Some(place.profile().into()), "run");
            test_host.set(pane("parent", "auto", &place.repo(), None, Some("run-1"), Finished));
            test_host.set(pane("c1", "acceptEdits", &child.worktree_path, Some("parent"), Some("run-c1"), Finished));
            let coordinator = host::coordinator(
                DelegationFileStore::new(place.profile()),
                test_host.clone(),
                host::maker(&place, 50_000_000_000),
                || true,
            )
            .unwrap();
            let parent: HashSet<String> = ["parent".to_string()].into();
            assert_eq!(
                coordinator.workspace_removal_refusal(&parent, &place.repo()),
                Some(DelegationReasonCode::WorkspaceHasChildren),
                "a workspace with an open child cannot be removed"
            );
            assert_eq!(coordinator.close_child("c1", false).await, ChildCleanupOutcome::Cleaned, "the merged child is cleaned up on close");
            assert!(
                !Path::new(&child.worktree_path).exists()
                    && place.only_the_repository_is_listed().await
                    && place.mighty_branches().await.is_empty()
                    && place.fixture_git(&["rev-parse", "refs/heads/main"]).await == merge.merged_commit
                    && place.fixture_git(&["status", "--porcelain", "--untracked-files=all"]).await.is_empty()
                    && test_host.stops().is_empty(),
                "the worktree and branch are gone by plain removal"
            );
            let saved = store.load().unwrap();
            let closed = &saved.children[0];
            assert!(
                coordinator.file() == saved
                    && closed.state == ChildState::Closed
                    && closed.closed_head.as_deref() == Some(merge.child_head.as_str())
                    && saved
                        .copy("c1", DelegationCopyKind::Report)
                        .is_some_and(|c| c.revision == 1 && !c.truncated && c.text == "# Report of c1\nDone.\n")
                    && saved.copy("c1", DelegationCopyKind::Task).map(|c| c.text.as_str()) == Some("Do the work.\n")
                    && saved.merges == [merge.clone()],
                "the copies and the branch head were stored first"
            );
            assert_eq!(
                coordinator.workspace_removal_refusal(&parent, &place.repo()),
                None,
                "with the child cleaned up, the workspace may go"
            );
            assert!(
                sidebar::rows(&saved)[0].card_actions() == [CardAction::Undo, CardAction::Discard]
                    && coordinator.undo_merge_from_card("c1").await
                        == ChildUndoOutcome::Undone { record: merge.clone(), restored_branch: true }
                    && place.fixture_git(&["rev-parse", "refs/heads/mighty/c1"]).await == merge.child_head
                    && place.fixture_git(&["rev-parse", "HEAD"]).await == start
                    && coordinator.file().children[0].state == ChildState::Closed
                    && coordinator.file().merges.is_empty(),
                "the undo after the close makes the branch again"
            );
            assert!(
                coordinator.discard_from_card("c1", Some(&[])).await == ChildCardDiscardOutcome::Discarded
                    && place.mighty_branches().await.is_empty()
                    && coordinator.file().children[0].state == ChildState::Discarded,
                "the closed child's branch goes by discard"
            );
            drop((coordinator, place));
        })
        .await;

        Box::pin(async {
            // Closing keeps the worktree and branch unless the child is merged, clean, and its copies are saved.
            let place = DelegationPlace::new();
            place.repository_default().await;
            let (c1, m1) = merged_child(&place, "c1").await;
            std::fs::write(Path::new(&c1.worktree_path).join("notes.txt"), "note\n").unwrap();
            let c2 = record_of(&place.child("c2", Some(&place.repo())).await);
            place.commit(&c2.worktree_path, "c2.txt", "c2\n").await;
            std::fs::write(Path::new(&c2.worktree_path).join("c2.txt"), "unsaved\n").unwrap();
            let (c3, m3) = merged_child(&place, "c3").await;
            let store = DelegationFileStore::with_attempts(place.profile(), 2);
            store
                .save(&DelegationFile::of(vec![c1.clone(), c2.clone(), c3.clone()], vec![], vec![], vec![m1.clone(), m3.clone()], vec![]))
                .unwrap();
            let test_host = TestHost::new(Some(place.profile().into()), "run");
            test_host.set(pane("parent", "auto", &place.repo(), None, Some("run-1"), Finished));
            test_host.set(pane("c2", "acceptEdits", &c2.worktree_path, Some("parent"), Some("run-c2"), Running));
            let coordinator = host::coordinator(
                DelegationFileStore::with_attempts(place.profile(), 2),
                test_host.clone(),
                host::maker(&place, 50_000_000_000),
                || true,
            )
            .unwrap();
            let before = place.disk().await;
            assert!(
                coordinator.close_child("c2", false).await == ChildCleanupOutcome::Kept(ChildCleanupHold::NotMerged)
                    && place.disk().await == before,
                "an unmerged child's close removes nothing"
            );
            let saved = store.load().unwrap();
            assert!(
                saved.children.iter().map(|c| c.state).eq([ChildState::Merged, ChildState::Closed, ChildState::Merged])
                    && saved.children[1].closed_head.is_none()
                    && saved.copy("c2", DelegationCopyKind::Report).is_none()
                    && sidebar::rows(&saved)[1].card_actions() == [CardAction::Discard],
                "the closed child keeps its card, with only discard"
            );
            assert_eq!(
                coordinator
                    .handle_call(request("follow_up", &[("child", "c2"), ("text", "Go on.")]), caller("parent", "claude", "claude", true))
                    .await
                    .refused,
                Some(DelegationReasonCode::ChildClosed)
            );
            assert!(
                coordinator.discard_child("c2").await == ChildDiscardOutcome::Discarded
                    && test_host.stops() == ["c2 with its folder"]
                    && !Path::new(&c2.worktree_path).exists()
                    && place.mighty_branches().await == "refs/heads/mighty/c1\nrefs/heads/mighty/c3",
                "a human's discard removes the unmerged work"
            );
            let before = place.disk().await;
            assert!(
                coordinator.close_child("c1", false).await == ChildCleanupOutcome::Kept(ChildCleanupHold::UntrackedFiles)
                    && place.disk().await == before,
                "a merged child with an untracked file is kept"
            );
            let saved = store.load().unwrap();
            assert!(
                saved.children[0].state == ChildState::Closed
                    && saved.children[0].closed_head.as_deref() == Some(m1.child_head.as_str())
                    && saved.copy("c1", DelegationCopyKind::Report).map(|c| c.text.as_str()) == Some("# Report of c1\nDone.\n")
                    && saved.copy("c1", DelegationCopyKind::Task).map(|c| c.text.as_str()) == Some("Do the work.\n")
                    && saved.merges == [m1.clone(), m3.clone()],
                "its copies are stored all the same"
            );
            assert!(
                coordinator.close_child("c1", false).await == ChildCleanupOutcome::Kept(ChildCleanupHold::NotMerged)
                    && place.disk().await == before
                    && matches!(coordinator.close_child("nobody", false).await, ChildCleanupOutcome::Failed(_)),
                "a closed child closes no further"
            );
            let _unblock = block_saves(&store);
            let before = place.disk().await;
            assert!(
                coordinator.close_child("c3", false).await
                    == ChildCleanupOutcome::Failed(
                        "The delegation file could not be saved, so the child's worktree and branch were kept.".into()
                    )
                    && place.disk().await == before
                    && Path::new(&c3.worktree_path).exists()
                    && place.fixture_git(&["rev-parse", "refs/heads/mighty/c3"]).await == m3.child_head
                    && coordinator.file().children[2].state == ChildState::Closed,
                "without its copies saved, nothing is removed"
            );
            assert!(
                coordinator.discard_child("c3").await == ChildDiscardOutcome::Failed(locale().get("delegation.card.error.notSaved"))
                    && Path::new(&c3.worktree_path).exists(),
                "a discard whose record cannot be saved removes nothing"
            );
            drop((coordinator, place));
        })
        .await;

        Box::pin(async {
            // Discard stops the child, keeps its report, and force-removes its unmerged work.
            let place = DelegationPlace::new();
            let start = place.repository_default().await;
            let child = record_of(&place.child("c1", Some(&place.repo())).await);
            place.commit(&child.worktree_path, "c1.txt", "c1\n").await;
            std::fs::write(Path::new(&child.worktree_path).join("c1.txt"), "more\n").unwrap();
            std::fs::write(Path::new(&child.worktree_path).join("notes.txt"), "note\n").unwrap();
            let nested = Path::new(&child.worktree_path).join("nested").to_string_lossy().into_owned();
            place.fixture_git(&["worktree", "add", "-q", "-b", "side", &nested, "main"]).await;
            place.fixture_git(&["worktree", "lock", &nested]).await;
            std::fs::write(Path::new(&nested).join("side.txt"), "side work\n").unwrap();
            std::fs::write(report_file_in(&child.worktree_path), "# Report\nHalf done.\n").unwrap();
            let store = DelegationFileStore::new(place.profile());
            store.save(&DelegationFile::of(vec![child.clone()], vec![], vec![], vec![], vec![])).unwrap();
            let test_host = TestHost::new(Some(place.profile().into()), "run");
            test_host.set(pane("parent", "auto", &place.repo(), None, Some("run-1"), Finished));
            test_host.set(pane("c1", "acceptEdits", &child.worktree_path, Some("parent"), Some("run-c1"), Running));
            let coordinator = host::coordinator(
                DelegationFileStore::new(place.profile()),
                test_host.clone(),
                host::maker(&place, 50_000_000_000),
                || true,
            )
            .unwrap();
            assert!(
                coordinator.nested_worktrees("c1").await.is_some_and(|n| n.len() == 1)
                    && coordinator.discard_child("c1").await == ChildDiscardOutcome::Discarded,
                "the child is discarded"
            );
            assert!(
                test_host.stops() == ["c1 with its folder"] && test_host.pane("c1").map(|p| p.activity) == Some(Idle),
                "its run was stopped first"
            );
            assert!(
                !Path::new(&child.worktree_path).exists()
                    && place.only_the_repository_is_listed().await
                    && place.mighty_branches().await.is_empty()
                    && place.fixture_git(&["rev-parse", "refs/heads/main"]).await == start
                    && place.fixture_git(&["rev-parse", "refs/heads/side"]).await == start
                    && place.fixture_git(&["status", "--porcelain", "--untracked-files=all"]).await.is_empty(),
                "its unmerged work is gone, and nothing else"
            );
            let saved = store.load().unwrap();
            assert!(
                coordinator.file() == saved
                    && saved.children[0].state == ChildState::Discarded
                    && saved.copy("c1", DelegationCopyKind::Report).map(|c| c.text.as_str()) == Some("# Report\nHalf done.\n"),
                "a copy of its report is kept"
            );
            let status = coordinator
                .handle_call(request("child_status", &[("child", "c1")]), caller("parent", "claude", "claude", true))
                .await
                .status
                .unwrap();
            assert!(
                matches!(coordinator.discard_child("c1").await, ChildDiscardOutcome::Failed(_))
                    && test_host.stops().len() == 1
                    && store.load().unwrap() == saved
                    && status.state == ChildState::Discarded
                    && status.report.is_none(),
                "discarded is final"
            );
            drop((coordinator, place));
        })
        .await;

        Box::pin(async {
            // Windows: a discard that another program's open file stops is told honestly, the child keeps its state and card, and a second
            // discard finishes.
            let place = DelegationPlace::new();
            place.repository_default().await;
            let (child, head) = place.reported_child("c1", &[], Some(&place.repo())).await;
            let store = DelegationFileStore::new(place.profile());
            store.save(&DelegationFile::of(vec![record_of(&child)], vec![], vec![], vec![], vec![])).unwrap();
            let test_host = TestHost::new(Some(place.profile().into()), "run");
            test_host.set(pane("parent", "auto", &place.repo(), None, Some("run-1"), Finished));
            let coordinator = host::coordinator(DelegationFileStore::new(place.profile()), test_host, host::maker(&place, 50_000_000_000), || true).unwrap();
            {
                use std::os::windows::fs::OpenOptionsExt as _;
                let _held = std::fs::OpenOptions::new().read(true).share_mode(1).open(Path::new(&child.worktree_path).join("c1.txt")).unwrap();
                assert!(matches!(coordinator.discard_from_card("c1", Some(&[])).await, ChildCardDiscardOutcome::Failed(message) if !message.is_empty()), "the card hears that the discard failed");
                assert!(
                    coordinator.file().children[0].state == ChildState::Reported
                        && store.load().unwrap().children[0].state == ChildState::Reported
                        && Path::new(&child.worktree_path).exists()
                        && place.fixture_git(&["rev-parse", "refs/heads/mighty/c1"]).await == head
                        && sidebar::rows(&coordinator.file())[0].card_actions().contains(&CardAction::Discard)
                        && coordinator.workspace_removal_refusal(&HashSet::new(), &place.repo()) == Some(DelegationReasonCode::WorkspaceHasChildren),
                    "the child is not called discarded: it keeps its state, its branch and its card"
                );
            }
            assert!(
                coordinator.discard_from_card("c1", Some(&[])).await == ChildCardDiscardOutcome::Discarded
                    && !Path::new(&child.worktree_path).exists()
                    && place.mighty_branches().await.is_empty()
                    && store.load().unwrap().children[0].state == ChildState::Discarded,
                "tried again once the file is let go, the discard finishes"
            );
        })
        .await;
    }

    /// "delegation: a child whose run would not end is never cleaned up"
    #[tokio::test]
    async fn a_child_whose_run_would_not_end_is_never_cleaned_up() {
        // Its future is large in a debug build: on the heap, not the test thread's stack.
        Box::pin(a_child_whose_run_would_not_end_is_never_cleaned_up_body()).await;
    }

    async fn a_child_whose_run_would_not_end_is_never_cleaned_up_body() {
        let _lane = lane::enter().await;
        let place = DelegationPlace::new();
        place.repository_default().await;
        let (child, merge) = merged_child(&place, "c1").await;
        DelegationFileStore::new(place.profile())
            .save(&DelegationFile::of(vec![child.clone()], vec![], vec![], vec![merge], vec![]))
            .unwrap();
        let test_host = TestHost::new(Some(place.profile().into()), "run");
        test_host.set(pane("parent", "auto", &place.repo(), None, Some("run-1"), Finished));
        let coordinator =
            host::coordinator(DelegationFileStore::new(place.profile()), test_host, host::maker(&place, 50_000_000_000), || true).unwrap();
        assert!(
            coordinator.close_child("c1", true).await == ChildCleanupOutcome::Kept(ChildCleanupHold::ChildBusy)
                && Path::new(&child.worktree_path).exists()
                && place.mighty_branches().await.contains(&child.branch)
                && coordinator.file().children[0].state == ChildState::Closed,
            "a child whose run would not end keeps its worktree and branch"
        );
    }

    /// "delegation: the file near its cap, and the switch turned off"
    #[tokio::test]
    async fn the_file_near_its_cap_and_the_switch_turned_off() {
        // Its future is large in a debug build: on the heap, not the test thread's stack.
        Box::pin(the_file_near_its_cap_and_the_switch_turned_off_body()).await;
    }

    async fn the_file_near_its_cap_and_the_switch_turned_off_body() {
        let _lane = lane::enter().await;
        Box::pin(async {
            let fixture = make_default().await;
            let mine = ChildRecord {
                worktree_path: Path::new(&fixture.place.root()).join("mine").to_string_lossy().into_owned(),
                awaited_run_id: Some("r-mine".into()),
                ..with_run(record("mine", ChildState::Running, "parent"), "r-mine")
            };
            let mut file = DelegationFile::of(vec![mine], vec![], vec![], vec![], vec![]);
            for index in 0.. {
                let child = record(&format!("open{index:03}"), ChildState::Running, &format!("pane{:03}", index / 3));
                let full = copy(&child.id, DelegationCopyKind::Report, &"r".repeat(MAXIMUM_COPY_BYTES), None);
                if store::encode(&file).len() + pruning::element_bytes(&child) + pruning::element_bytes(&full) + pruning::new_child_room()
                    > MAXIMUM_FILE_BYTES
                {
                    break;
                }
                file.children.push(child);
                file = file.setting_copy(full);
            }
            file.children.push(record("pad", ChildState::Running, "pane-pad"));
            file = file.setting_copy(copy("pad", DelegationCopyKind::Report, "p", None));
            let spare = MAXIMUM_FILE_BYTES as i64 - store::encode(&file).len() as i64 - (pruning::new_child_room() as i64 + 2_000);
            if spare > 0 {
                file = file.setting_copy(copy("pad", DelegationCopyKind::Report, &"p".repeat(1 + spare as usize), None));
            }
            let room = MAXIMUM_FILE_BYTES - store::encode(&file).len();
            assert!(
                file.children.len() > 50 && room >= pruning::new_child_room() && room <= pruning::new_child_room() + 2_100,
                "the file is near its cap: {room}"
            );
            fixture.store().save(&file).unwrap();
            let coordinator = fixture.relaunch();
            assert!(coordinator.file() == file && fixture.bytes().len() <= MAXIMUM_FILE_BYTES, "a relaunch reads a nearly full file back");
            host::maker(&fixture.place, 50_000_000_000).create("mine", &fixture.place.repo(), "Task\n").await;
            let report = "보고서의 한 줄 \"quoted\" \\ and / more\n".repeat(4_000);
            write_report(&coordinator.file().child("mine").cloned().unwrap(), &report);
            let notice = coordinator.child_run_ended("mine", "r-mine", ChildRunEnd::Finished).await.unwrap();
            let copy_of_mine = coordinator.file().copy("mine", DelegationCopyKind::Report).cloned().unwrap();
            assert!(
                notice.kind == NoticeKind::Reported
                    && notice.report_revision == 1
                    && copy_of_mine.truncated
                    && copy_of_mine.revision == 1
                    && copy_of_mine.original_bytes == report.len() as i64
                    && fixture.loaded() == coordinator.file()
                    && fixture.bytes().len() <= MAXIMUM_FILE_BYTES,
                "a long report is recorded and saved near the cap"
            );
            let status = coordinator
                .handle_call(request("child_status", &[("child", "mine")]), caller("parent", "claude", "claude", true))
                .await
                .status
                .unwrap();
            let body = status.report.unwrap();
            assert!(
                status.report_truncated
                    && body.ends_with(TRUNCATION_MARKER)
                    && report.starts_with(&body[..body.len() - TRUNCATION_MARKER.len()]),
                "the parent reads the marked copy"
            );
            let (bytes, before) = (fixture.bytes(), coordinator.file());
            let refused = coordinator
                .handle_call(request("delegate", &[("task", "One more."), ("mode", "plan")]), caller("parent", "claude", "claude", true))
                .await;
            assert!(
                refused == DelegationResponse::refusal(DelegationReasonCode::StoreFull)
                    && fixture.bytes() == bytes
                    && coordinator.file() == before
                    && fixture.host.created_panes().is_empty(),
                "a new child is refused with store_full"
            );
            assert!(
                coordinator.file().children.len() == file.children.len() && coordinator.file().copies.len() == file.copies.len() + 1,
                "open children are never pruned"
            );
            drop((coordinator, fixture));
        })
        .await;

        Box::pin(async {
            // Turning the switch off leaves an existing child working, and only detaches the server.
            let place = DelegationPlace::new();
            place.repository_default().await;
            let store = DelegationFileStore::new(place.profile());
            let empty = host::coordinator(
                DelegationFileStore::new(place.profile()),
                TestHost::new(None, "run"),
                host::maker(&place, 50_000_000_000),
                || true,
            )
            .unwrap();
            assert!(
                empty.file() == DelegationFile::empty() && !store.file_path().exists() && !Path::new(&place.profile()).exists(),
                "a launch with no delegation file writes nothing"
            );
            std::fs::create_dir_all(place.profile()).unwrap();
            std::fs::write(store.file_path(), "{").unwrap();
            assert_eq!(
                host::coordinator(
                    DelegationFileStore::new(place.profile()),
                    TestHost::new(None, "run"),
                    host::maker(&place, 50_000_000_000),
                    || true
                )
                .err(),
                Some(DelegationFileError::Unreadable)
            );
            assert_eq!(std::fs::read_to_string(store.file_path()).unwrap(), "{", "a file that cannot be read is left as it is");
            store.save(&DelegationFile::empty()).unwrap();
            let test_host = TestHost::new(Some(place.profile().into()), "run");
            test_host.set_first_run(Some(|id: &str| format!("run-of-{id}")));
            test_host.set_makes_panes(true);
            test_host.set(pane("parent", "auto", &place.repo(), None, Some("run-1"), Running));
            let on = Arc::new(AtomicBool::new(true));
            let switch = on.clone();
            let coordinator =
                host::coordinator(DelegationFileStore::new(place.profile()), test_host, host::maker(&place, 50_000_000_000), move || {
                    switch.load(Ordering::SeqCst)
                })
                .unwrap();
            let parent = caller("parent", "claude", "claude", true);
            let id = coordinator
                .handle_call(request("delegate", &[("task", "Do it."), ("mode", "plan")]), parent.clone())
                .await
                .child
                .unwrap()
                .id;
            assert!(
                eventually_async(60_000, || async {
                    coordinator.file().children[0].state == ChildState::Running && !coordinator.is_starting(&id)
                })
                .await,
                "the child runs"
            );
            on.store(false, Ordering::SeqCst);
            let bytes = std::fs::read(store.file_path()).unwrap();
            assert!(
                coordinator.handle_call(request("list_children", &[]), parent.clone()).await
                    == DelegationResponse::failure(DETACHED_MESSAGE)
                    && coordinator.handle_call(request("child_status", &[("child", &id)]), parent.clone()).await
                        == DelegationResponse::failure(DETACHED_MESSAGE)
                    && std::fs::read(store.file_path()).unwrap() == bytes,
                "with the switch off the tools reach nothing"
            );
            let child = coordinator.file().children[0].clone();
            write_report(&child, "Done with the switch off.\n");
            coordinator.child_run_ended(&id, &format!("run-of-{id}"), ChildRunEnd::Finished).await;
            let after = coordinator.file();
            assert!(
                after.children[0].state == ChildState::Reported
                    && after.children[0].report_revision == 1
                    && after.notices[0].kind == NoticeKind::Reported
                    && after.notices[0].lane == DeliveryLane::Delivered
                    && sidebar::rows(&after)[0].card_actions() == [CardAction::Merge, CardAction::Discard]
                    && matches!(coordinator.merge_from_card(&id).await, ChildMergeOutcome::Merged(_))
                    && coordinator.close_child(&id, false).await == ChildCleanupOutcome::Cleaned,
                "existing children, their cards and their cleanup go on with the switch off"
            );
        })
        .await;
    }

    /// "delegation: a follow-up run that leaves the report standing sends its ended notice once"
    #[tokio::test]
    async fn a_follow_up_run_that_leaves_the_report_standing_sends_its_ended_notice_once() {
        // Its future is large in a debug build: on the heap, not the test thread's stack.
        Box::pin(a_follow_up_run_that_leaves_the_report_standing_sends_its_ended_notice_once_body()).await;
    }

    async fn a_follow_up_run_that_leaves_the_report_standing_sends_its_ended_notice_once_body() {
        let _lane = lane::enter().await;
        let fixture = reports(None).await;
        let started = running_child(&fixture).await;
        let id = started.id.clone();
        let head = commit_work(&fixture, &started, "one").await;
        write_report(&started, "Report.\n");
        assert_eq!(
            fixture.coordinator.child_run_ended(&id, &format!("first-{id}"), ChildRunEnd::Finished).await.map(|n| n.kind),
            Some(NoticeKind::Reported)
        );
        fixture.host.set_claude(&id, Idle, Some(&format!("first-{id}")));
        assert!(fixture.call("follow_up", &[("child", &id), ("text", "Check it once more.")]).await.follow_up.is_some());
        fixture.coordinator.child_run_started(&id, "follow-2").await;
        assert!(fixture.coordinator.child_run_ended(&id, "follow-2", ChildRunEnd::Finished).await.is_none());
        let standing = fixture.child(&id);
        assert!(
            standing.state == ChildState::Reported
                && standing.report_revision == 1
                && standing.report_head.as_deref() == Some(head.as_str()),
            "the report still stands"
        );
        let late = fixture.coordinator.await_run("follow-2", &id).await.unwrap();
        assert!(
            late.kind == NoticeKind::EndedWithoutReport
                && late.report_revision == 1
                && fixture.coordinator.await_run("follow-2", &id).await.is_none(),
            "the late notice goes out once"
        );
        fixture.coordinator.child_run_started(&id, "follow-3").await;
        assert!(fixture.coordinator.await_run("follow-3", &id).await.is_none());
        assert_eq!(
            fixture.coordinator.child_run_ended(&id, "follow-3", ChildRunEnd::Finished).await.map(|n| n.kind),
            Some(NoticeKind::EndedWithoutReport),
            "an awaited run sends its notice at its end"
        );
        assert!(
            fixture.child(&id).state == ChildState::Reported
                && fixture.notices(&id).iter().map(|n| n.kind).eq([
                    NoticeKind::Reported,
                    NoticeKind::EndedWithoutReport,
                    NoticeKind::EndedWithoutReport
                ])
                && fixture.loaded() == fixture.file(),
            "the notices are one each"
        );
    }

    /// `OnFeature`: a repository on main with the parent on `feat`, a child made there and reported, and a coordinator over it.
    async fn on_feature(work_on_feature: bool) -> (Fixture, ChildRecord, String) {
        let place = DelegationPlace::new();
        place.repository_default().await;
        place.fixture_git(&["checkout", "-q", "-b", "feat"]).await;
        let child = place.child("c1", Some(&place.repo())).await;
        assert_eq!(child.parent_branch, "feat", "the child is made while the parent works on feat");
        let head = place.commit(&child.worktree_path, "a.txt", "a\n").await;
        let child = record_of(&child).recording_report(&head).unwrap();
        if work_on_feature {
            place.commit(&place.repo(), "feat.txt", "feat\n").await;
        }
        let fixture = tools_over(place, vec![child.clone()], 40);
        fixture.set_parent("auto", "run-1", Finished);
        (fixture, child, head)
    }

    /// "delegation: a merge follows the recorded branch into the branch that took its work"
    #[tokio::test]
    async fn a_merge_follows_the_recorded_branch_into_the_branch_that_took_its_work() {
        // Its future is large in a debug build: on the heap, not the test thread's stack.
        Box::pin(a_merge_follows_the_recorded_branch_into_the_branch_that_took_its_work_body()).await;
    }

    async fn a_merge_follows_the_recorded_branch_into_the_branch_that_took_its_work_body() {
        let _lane = lane::enter().await;
        Box::pin(async {
            let (fixture, child, head) = on_feature(true).await;
            let place = &fixture.place;
            let git = place.git();
            place.fixture_git(&["checkout", "-q", "main"]).await;
            place.fixture_git(&["merge", "-q", "--no-ff", "-m", "merge feat", "feat"]).await;
            assert_eq!(merge::merge_branch(&git, &child.git(), &place.repo()).await.as_deref(), Some("main"), "main took feat's work");
            place.fixture_git(&["branch", "-q", "-D", "feat"]).await;
            assert_eq!(
                merge::merge_branch(&git, &child.git(), &place.repo()).await.as_deref(),
                Some("main"),
                "main took the work of a branch that is gone"
            );
            let main = place.fixture_git(&["rev-parse", "HEAD"]).await;
            let ChildMergeOutcome::Merged(merged) = fixture.coordinator.merge_from_card("c1").await else { panic!("not merged") };
            assert!(
                merged.parent_branch == "main"
                    && merged.pre_merge_commit == main
                    && merged.kind == MergeKind::CardMergeCommit
                    && place.fixture_git(&["symbolic-ref", "HEAD"]).await == "refs/heads/main"
                    && place.fixture_git(&["rev-parse", "HEAD^2"]).await == head,
                "the card's merge went into main"
            );
            assert!(
                fixture.file().children[0].parent_branch == "main" && fixture.loaded().children[0].parent_branch == "main",
                "the record follows the branch"
            );
            assert!(
                matches!(fixture.coordinator.undo_merge_from_card("c1").await, ChildUndoOutcome::Undone { .. })
                    && place.fixture_git(&["rev-parse", "HEAD"]).await == main,
                "the undo follows it"
            );
            place.fixture_git(&["checkout", "-q", "--detach"]).await;
            assert!(
                merge::merge_branch(&git, &child.git(), &place.repo()).await.is_none()
                    && fixture.coordinator.merge_from_card("c1").await
                        == ChildMergeOutcome::Refused(DelegationReasonCode::BranchNotCheckedOut),
                "a detached HEAD refuses"
            );
            drop(fixture);
        })
        .await;

        Box::pin(async {
            // The merge tool: the recorded branch is gone and main sits where the child began.
            let (fixture, _, head) = on_feature(false).await;
            fixture.place.fixture_git(&["checkout", "-q", "main"]).await;
            fixture.place.fixture_git(&["branch", "-q", "-D", "feat"]).await;
            let start = fixture.place.fixture_git(&["rev-parse", "HEAD"]).await;
            let answer = fixture.call("merge", &[("child", "c1"), ("expected_head", &head)]).await;
            assert!(
                answer.merged == Some(MergeRecord::new("c1", MergeKind::ToolFastForward, "main", &start, &head, &head))
                    && fixture.file().children[0].parent_branch == "main"
                    && fixture.loaded().children[0].parent_branch == "main",
                "the tool's merge went into main"
            );
        })
        .await;
    }
}
