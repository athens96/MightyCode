//! The composer's second send and Mighty Orchestration in Codex and Antigravity panes (C# MightyCode.Core.Tests/
//! DelegationHumanChildVerification.cs, ported from the Mac's ComposerSecondSendTests, DelegationHumanChildTests and
//! DelegationProvidersTests): which pane opens a new worktree, the human's child from the send to its first run with the parent left
//! alone, and the coordinator's half of the app's commit of a Codex or Antigravity child's work (the git half is W7-B's `tests/git.rs`).
//! L683's panes and L687/L693 are W7-D's (the desktop service).

mod support;

use std::sync::{Arc, Mutex};

use mighty_core::model::RunSettings;
use mighty_delegation::delivery::{follow_up_text, report_file_in};
use mighty_delegation::git::auto_commit::{looks_secret, message_for, should_commit};
use mighty_delegation::git::worktree::{ChildWorktree, ChildWorktreeBase, ChildWorktreeCheck};
use mighty_delegation::model::{ChildRecord, ChildRecordOptions, ChildState, DeliveryLane, FollowUp};
use mighty_delegation::providers::{self, ComposerSecondSend};
use mighty_delegation::request::DelegationReasonCode;
use mighty_delegation::store::{self, DelegationFile};
use mighty_delegation::tools::delegate::{HumanChildFirstRun, HumanChildStarter};
use mighty_delegation::tools::delegate::{app_commit_note, first_input, human_first_run, human_note};
use mighty_run::model::StartRunRequest;

const COMMIT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

/// The send's own start of a child's first run, as the window gives it: each run it started, and each child that failed after the
/// answer.
#[derive(Default)]
struct Sends {
    started: Mutex<Vec<(String, HumanChildFirstRun)>>,
    failed: Mutex<Vec<String>>,
    run_fails: std::sync::atomic::AtomicBool,
}

impl Sends {
    fn starter(self: &Arc<Self>) -> HumanChildStarter {
        let (start, fail) = (self.clone(), self.clone());
        HumanChildStarter {
            start: Arc::new(move |child: String, run: HumanChildFirstRun| {
                start.started.lock().unwrap().push((child.clone(), run));
                let answer = (!start.run_fails.load(std::sync::atomic::Ordering::SeqCst)).then(|| format!("send-run-of-{child}"));
                Box::pin(async move { answer })
            }),
            failed: Arc::new(move |child: String| {
                fail.failed.lock().unwrap().push(child);
                Box::pin(async {})
            }),
        }
    }
    fn started(&self) -> Vec<(String, HumanChildFirstRun)> {
        self.started.lock().unwrap().clone()
    }
    fn failed(&self) -> Vec<String> {
        self.failed.lock().unwrap().clone()
    }
}

/// "delegation: the second send opens a worktree only where a child can work"
#[test]
fn the_second_send_opens_a_worktree_only_where_a_child_can_work() {
    let choose = |provider: &str, mode: &str, is_child: bool, on: bool, ready: bool, kind: &str| {
        providers::choose_second_send(kind, provider, mode, is_child, on, ready)
    };
    let default = |provider: &str| choose(provider, "acceptEdits", false, true, true, "claude");
    assert_eq!(default("claude"), ComposerSecondSend::Worktree, "a Claude pane opens a new worktree");
    assert!(
        choose("claude", "acceptEdits", true, true, true, "claude") == ComposerSecondSend::Queue
            && choose("claude", "acceptEdits", false, false, true, "claude") == ComposerSecondSend::Queue
            && choose("claude", "acceptEdits", false, true, false, "claude") == ComposerSecondSend::Queue,
        "otherwise Claude queues, as Enter"
    );
    for provider in ["codex", "gemini"] {
        assert!(
            default(provider) == ComposerSecondSend::Worktree
                && choose(provider, "acceptEdits", true, true, true, "claude") == ComposerSecondSend::Queue
                && choose(provider, "acceptEdits", false, false, true, "claude") == ComposerSecondSend::Queue
                && choose(provider, "acceptEdits", false, true, false, "claude") == ComposerSecondSend::Queue,
            "{provider} opens one too, and otherwise queues"
        );
    }
    assert_eq!(choose("claude", "acceptEdits", false, true, true, "shell"), ComposerSecondSend::Queue, "a shell pane queues");
    for (provider, mode, worktree) in [
        ("codex", "manual", false),
        ("codex", "acceptEdits", true),
        ("codex", "onRequest", true),
        ("codex", "fullAccess", true),
        ("gemini", "manual", false),
        ("gemini", "plan", false),
        ("gemini", "acceptEdits", true),
        ("gemini", "fullAccess", true),
    ] {
        assert_eq!(choose(provider, mode, false, true, true, "claude") == ComposerSecondSend::Worktree, worktree, "{provider} {mode}");
    }
    for mode in providers::starting_modes("claude") {
        assert_eq!(choose("claude", mode, false, true, true, "claude"), ComposerSecondSend::Worktree, "Claude {mode}");
    }
    // A checkout can host one when only the disk is short.
    let base =
        ChildWorktreeBase { repository: "r".into(), prefix: String::new(), parent_branch: "main".into(), base_commit: COMMIT.into() };
    assert!(
        ChildWorktreeCheck::Ready(base).can_host_worktree()
            && ChildWorktreeCheck::Refused(DelegationReasonCode::LowDisk).can_host_worktree()
            && [DelegationReasonCode::NotGit, DelegationReasonCode::DetachedHead, DelegationReasonCode::UnbornBranch]
                .iter()
                .all(|r| !ChildWorktreeCheck::Refused(*r).can_host_worktree()),
        "only a usable checkout can host one"
    );
}

/// "delegation: each agent has its modes, its words and its way to commit"
#[test]
fn each_agent_has_its_modes_its_words_and_its_way_to_commit() {
    assert_eq!(providers::starting_modes("claude"), ["plan", "manual", "acceptEdits", "auto", "fullAccess"]);
    assert_eq!(providers::starting_modes("codex"), ["manual", "acceptEdits", "onRequest", "fullAccess"]);
    assert!(
        providers::is_no_wider("acceptEdits", "onRequest", "codex")
            && !providers::is_no_wider("fullAccess", "onRequest", "codex")
            && !providers::is_no_wider("plan", "fullAccess", "codex")
            && !providers::is_no_wider("onRequest", "auto", "claude"),
        "a mode is compared within its agent"
    );
    assert!(
        providers::attaches_delegation("claude", "claude", "plan")
            && providers::attaches_delegation("claude", "codex", "acceptEdits")
            && providers::attaches_delegation("claude", "codex", "fullAccess")
            && !providers::attaches_delegation("claude", "codex", "manual")
            && !providers::attaches_delegation("claude", "codex", "onRequest")
            && !providers::attaches_delegation("claude", "gemini", "fullAccess")
            && !providers::attaches_delegation("shell", "claude", "plan"),
        "which runs get the delegation tools"
    );
    assert!(
        providers::starting_modes("claude").iter().all(|m| providers::child_can_write("claude", m))
            && !providers::child_can_write("codex", "manual")
            && providers::child_can_write("codex", "onRequest")
            && !providers::child_can_write("gemini", "plan")
            && providers::child_can_write("gemini", "acceptEdits"),
        "a child can write in every Claude mode and only the editing modes of the others"
    );
    assert!(
        providers::can_commit("claude", "plan")
            && providers::agent_may_merge("claude", "manual")
            && !providers::can_commit("codex", "acceptEdits")
            && providers::can_commit("gemini", "fullAccess")
            && !providers::agent_may_merge("codex", "acceptEdits")
            && providers::agent_may_merge("codex", "fullAccess")
            && providers::can_steer("claude", "claude")
            && !providers::can_steer("claude", "codex"),
        "only Claude commits and merges in any mode"
    );
    assert!(
        !should_commit("claude", true, true, false)
            && ["codex", "gemini"].iter().all(|p| should_commit(p, true, true, false)
                && !should_commit(p, false, true, false)
                && !should_commit(p, true, false, false)
                && !should_commit(p, true, true, true))
            && message_for("c1", 2) == "mighty: c1 report r2",
        "the app commits only for another agent's changed report"
    );
    for name in [
        ".env",
        ".ENV.production",
        "a/b/cert.pem",
        "x.key",
        "id_rsa.pub",
        "credentials",
        r"keys\deploy.key",
        "cert.PFX",
        "store.p12",
        "putty.ppk",
        "id_ed25519",
        "id_ed25519.pub",
        "id_ecdsa",
        "id_dsa.pub",
        ".npmrc",
        "home/.pypirc",
        "secrets.json",
        "Secrets.yaml",
    ] {
        assert!(looks_secret(name), "{name} looks secret");
    }
    for name in [
        "environment.txt",
        "keyboard.swift",
        "a/.envoy/config",
        "my-credentials.txt",
        "npmrc",
        ".npmrc.example",
        "my-secrets.json",
        "secrets",
        "pfx.txt",
    ] {
        assert!(!looks_secret(name), "{name} does not");
    }

    // A Claude child's words are as before; another agent's name no Claude pane and leave the commit to the app.
    let worktree = ChildWorktree {
        session_id: "c1".into(),
        worktree_path: r"C:\w\c1".into(),
        working_folder: r"C:\w\c1".into(),
        branch: "mighty/c1".into(),
        parent_branch: "main".into(),
        base_commit: COMMIT.into(),
    };
    let (task_file, report_file) = (worktree.task_file(), worktree.report_file());
    assert_eq!(
        first_input("Fix the parser.\n", &worktree, "claude", None),
        format!(
            "Another Claude pane in Mighty Code delegated this task to you. You work in your own git worktree on the branch mighty/c1; the task is also saved in {task_file}.\n\nWhen you are done, commit your work on this branch, leaving no uncommitted changes to tracked files, and then write your report to {report_file}: what you did, what is left and what your parent should check. Your parent gets one notice for each of your runs that ends with that file changed, and may merge your branch. Do not merge, push or switch branches.\n\nTask:\nFix the parser.\n"
        ),
        "a Claude child's first run"
    );
    assert_eq!(
        human_note(&worktree, "claude", None),
        format!(
            "[Mighty Code] This request runs in its own git worktree on the branch mighty/c1, opened beside another Claude pane so that pane keeps working undisturbed. When you are done, commit your work on this branch, leaving no uncommitted changes to tracked files, and then write a short report to {report_file}: what you did and what is left. The person who sent this merges your branch from the app. Do not merge, push or switch branches."
        ),
        "a Claude child's note"
    );
    let follow_up = FollowUp::new("f-1", "c1", "Also add tests.", DeliveryLane::Pending);
    assert!(
        follow_up_text(&follow_up, &report_file, "claude")
            .ends_with(&format!("When you are done, commit your work on your branch and write your updated report to {report_file}."))
    );
    for provider in ["codex", "gemini"] {
        let texts = [
            first_input("Fix the parser.", &worktree, provider, None),
            human_note(&worktree, provider, None),
            human_first_run("Fix the parser.", &worktree, provider, None).input,
            follow_up_text(&follow_up, &report_file, provider),
        ];
        for text in &texts {
            assert!(
                !text.contains("Claude pane")
                    && !text.contains("commit your work")
                    && text.contains("Do not commit")
                    && text.contains(&report_file),
                "{provider}: {text}"
            );
        }
        assert!(
            texts[0].starts_with("Another pane in Mighty Code delegated this task to you.")
                && texts[0].ends_with("Task:\nFix the parser.")
                && texts[0].contains(&app_commit_note(&worktree))
                && texts[1].contains(&app_commit_note(&worktree)),
            "{provider}'s first run"
        );
    }
    // A slash command runs alone, the note following it as the next request.
    let command = human_first_run("/review the parser", &worktree, "claude", None);
    let plain = human_first_run("Fix it.", &worktree, "claude", None);
    assert!(
        command.input == "/review the parser"
            && command.following_note == Some(human_note(&worktree, "claude", None))
            && plain.following_note.is_none()
            && plain.input == format!("Fix it.\n\n{}", human_note(&worktree, "claude", None)),
        "a slash command runs alone"
    );
    // A child whose run works in a folder inside its worktree may also write its report there: Codex by a writable root, Antigravity
    // by an added folder.
    let root = std::env::temp_dir().join("w").join("c1").to_string_lossy().into_owned();
    let temp = std::env::temp_dir().to_string_lossy().into_owned();
    let run = |provider: &str, mode: &str| {
        let mut request = StartRunRequest::new("c1", "ws", "claude", "hi")
            .with_provider(provider)
            .with_settings(RunSettings { permission_mode: mode.into(), ..Default::default() });
        request.delegation_worktree_root = Some(root.clone());
        mighty_run::providers::arguments(&request, &temp, None).unwrap()
    };
    let (codex_run, read_only, agy_run) = (run("codex", "acceptEdits"), run("codex", "manual"), run("gemini", "acceptEdits"));
    let writable = format!("sandbox_workspace_write.writable_roots={}", serde_json::to_string(&[&root]).unwrap());
    let at = agy_run.iter().position(|a| a == "--add-dir");
    assert!(
        codex_run.contains(&writable)
            && !read_only.iter().any(|a| a.contains("writable_roots"))
            && at.is_some_and(|at| agy_run[at + 1] == root),
        "the worktree root reaches the run: {codex_run:?} {agy_run:?}"
    );
    // The provider rides with the follow-up's child.
    let child = ChildRecord::make_with(
        "c1",
        "p",
        r"C:\w\c1",
        "main",
        COMMIT,
        "acceptEdits",
        "k",
        ChildRecordOptions { state: Some(ChildState::Reported), provider: Some("codex".into()), ..Default::default() },
    );
    let file = DelegationFile { follow_ups: vec![follow_up.clone()], ..DelegationFile::of(vec![child], vec![], vec![], vec![], vec![]) };
    assert_eq!(
        file.follow_up_item(&follow_up).map(|i| i.text),
        Some(follow_up_text(&follow_up, &report_file_in(r"C:\w\c1"), "codex")),
        "a Codex child's follow-up"
    );

    // A record kept before the marks is an agent's Claude child, and the marks are stored only when they say something (L683's
    // record half).
    let old = store::decode(
        br#"{"version":1,"children":[{"id":"c1","parentSessionId":"p","worktreePath":"/w/c1","branch":"mighty/c1","parentBranch":"main","baseCommit":"abc","startingMode":"plan","requestKey":"k","followUpCount":0,"reportRevision":0,"state":"running"}],"notices":[],"followUps":[],"merges":[],"copies":[]}"#,
    )
    .unwrap()
    .children
    .remove(0);
    let written = String::from_utf8(store::encode(&old)).unwrap();
    assert!(
        old.started_by_human.is_none()
            && old.notifies_parent()
            && old.provider.is_none()
            && old.agent() == "claude"
            && !written.contains("startedByHuman")
            && !written.contains("provider"),
        "an old record is an agent's Claude child"
    );
    let marked = ChildRecord { started_by_human: Some(true), provider: Some("codex".into()), ..old };
    let human =
        store::decode(&store::encode(&DelegationFile::of(vec![marked], vec![], vec![], vec![], vec![]))).unwrap().children.remove(0);
    assert!(
        human.started_by_human == Some(true)
            && human.provider.as_deref() == Some("codex")
            && human.agent() == "codex"
            && !human.notifies_parent(),
        "the marks are stored"
    );
    assert!(
        ChildRecord::make_with(
            "c",
            "p",
            "/w/c",
            "main",
            COMMIT,
            "plan",
            "k",
            ChildRecordOptions { provider: Some("claude".into()), ..Default::default() }
        )
        .provider
        .is_none(),
        "Claude is stored as no provider"
    );
}

mod git {
    use std::path::Path;
    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    use mighty_core::shell::CardAction;
    use mighty_delegation::coordinator::{Coordinator, not_your_child_message};
    use mighty_delegation::copy::{DelegationCopyKind, MAXIMUM_COPY_BYTES};
    use mighty_delegation::git::merge::ChildMergeOutcome;
    use mighty_delegation::git::worktree::report_file_in;
    use mighty_delegation::git::{self, auto_commit};
    use mighty_delegation::host::{DelegationChildPane, DelegationPaneActivity, DelegationPaneState};
    use mighty_delegation::model::{ChildKind, ChildRecord, ChildRecordOptions, ChildState};
    use mighty_delegation::reports::ChildRunEnd;
    use mighty_delegation::request::{DelegationCaller, DelegationReasonCode, DelegationResponse};
    use mighty_delegation::sidebar;
    use mighty_delegation::store::{DelegationFile, DelegationFileStore};
    use mighty_delegation::tools::delegate::{HumanChildAt, HumanChildStart};

    use super::support::host::{self, TestHost, caller, eventually, locale, request};
    use super::support::lane;
    use super::support::place::DelegationPlace;
    use super::{COMMIT, Sends};

    /// A place with a coordinator over it. The pane "parent" works in the repository, running `provider`.
    struct Fixture {
        place: DelegationPlace,
        host: Arc<TestHost>,
        coordinator: Coordinator,
        sends: Arc<Sends>,
    }

    impl Fixture {
        fn store(&self) -> DelegationFileStore {
            DelegationFileStore::new(self.place.profile())
        }
        fn bytes(&self) -> Vec<u8> {
            std::fs::read(self.store().file_path()).unwrap()
        }
        async fn send(&self, text: &str, from: &str, plan_first: bool) -> HumanChildStart {
            self.coordinator.start_human_child(text, from, self.sends.starter(), plan_first, None).await
        }
        async fn call(&self, tool: &str, arguments: &[(&str, &str)]) -> DelegationResponse {
            self.coordinator.handle_call(request(tool, arguments), caller("parent", "claude", "claude", true)).await
        }
        async fn wait_for(&self, id: &str, state: ChildState) -> bool {
            eventually(60_000, || self.coordinator.file().child(id).map(|c| c.state) == Some(state) && !self.coordinator.is_starting(id))
                .await
        }
        fn only(&self) -> ChildRecord {
            let file = self.coordinator.file();
            assert_eq!(file.children.len(), 1);
            file.children[0].clone()
        }
        /// A refused send made nothing: the same file, no worktree root, no `mighty/*` branch, and no pane or run asked for.
        async fn nothing_made(&self, bytes: &[u8], has_git: bool) {
            assert!(
                self.bytes() == bytes
                    && !Path::new(&self.place.root()).exists()
                    && self.host.created_panes().is_empty()
                    && self.sends.started().is_empty()
                    && self.host.started_runs().is_empty(),
                "the refused send changed nothing"
            );
            if has_git {
                assert!(self.place.mighty_branches().await.is_empty(), "no branch was made");
            }
        }
    }

    async fn make(
        commits: Option<usize>,
        file: Option<DelegationFile>,
        parent_mode: &str,
        provider: &str,
        free: u64,
        switch_on: bool,
    ) -> Fixture {
        let place = DelegationPlace::new();
        let store = DelegationFileStore::new(place.profile());
        store.save(&file.unwrap_or_default()).unwrap();
        let test_host = TestHost::new(Some(place.profile().into()), "run");
        test_host.set_first_run(Some(|id: &str| format!("host-run-of-{id}")));
        test_host.set_makes_panes(true);
        std::fs::create_dir_all(place.repo()).unwrap();
        if let Some(count) = commits {
            place.repository(count, "", "false").await;
        }
        test_host.set(DelegationPaneState {
            session_id: "parent".into(),
            permission_mode: parent_mode.into(),
            folder: place.repo(),
            kind: "claude".into(),
            provider: provider.into(),
            parent_session_id: None,
            run_id: Some("run-1".into()),
            activity: DelegationPaneActivity::Running,
        });
        let coordinator = host::coordinator(store, test_host.clone(), host::maker(&place, free), move || switch_on).unwrap();
        Fixture { place, host: test_host, coordinator, sends: Arc::new(Sends::default()) }
    }

    async fn make_with(provider: &str, mode: &str) -> Fixture {
        make(Some(1), None, mode, provider, 50_000_000_000, true).await
    }

    fn open_child(id: &str, by_human: bool) -> ChildRecord {
        let worktree = std::env::temp_dir().join("nonexistent-worktrees").join(id).to_string_lossy().into_owned();
        ChildRecord::make_with(
            id,
            "parent",
            &worktree,
            "main",
            COMMIT,
            "plan",
            &format!("key-{id}"),
            ChildRecordOptions { state: Some(ChildState::Running), started_by_human: by_human.then_some(true), ..Default::default() },
        )
    }

    fn started(answer: HumanChildStart) -> ChildRecord {
        match answer {
            HumanChildStart::Started(child) => *child,
            other => panic!("The send made no child: {other:?}"),
        }
    }

    /// "delegation: the human's send makes a child that never notifies its parent"
    #[tokio::test]
    async fn the_humans_send_makes_a_child_that_never_notifies_its_parent() {
        let _lane = lane::enter().await;
        Box::pin(async {
            let fixture = make_with("claude", "acceptEdits").await;
            let head = fixture.place.fixture_git(&["rev-parse", "HEAD"]).await;
            let text = "Try the other parser in a branch.\n";
            let child = started(fixture.send(text, "parent", false).await);
            assert!(
                child.started_by_human == Some(true)
                    && !child.notifies_parent()
                    && child.state == ChildState::Creating
                    && child.parent_session_id == "parent"
                    && child.starting_mode == "acceptEdits"
                    && child.parent_branch == "main"
                    && child.base_commit == head,
                "the child is the human's, in the parent's mode"
            );
            assert!(fixture.wait_for(&child.id, ChildState::Running).await, "it runs");
            let saved = fixture.store().load().unwrap();
            let run_id = format!("send-run-of-{}", child.id);
            assert!(
                saved.children[0].started_by_human == Some(true)
                    && saved.children[0].run_id.as_deref() == Some(run_id.as_str())
                    && saved.children[0].awaited_run_id.as_deref() == Some(run_id.as_str())
                    && saved.copy(&child.id, DelegationCopyKind::Task).map(|c| c.text.as_str()) == Some(text)
                    && saved.notices.is_empty(),
                "the record is saved with its task and first run"
            );
            assert!(
                fixture.host.created_panes()
                    == [DelegationChildPane {
                        session_id: child.id.clone(),
                        parent_session_id: "parent".into(),
                        mode: "acceptEdits".into(),
                        folder: child.worktree_path.clone(),
                        started_by_human: true,
                        provider: "claude".into()
                    }]
                    && fixture.host.started_runs().is_empty(),
                "the host made its pane and started nothing"
            );
            let runs = fixture.sends.started();
            assert!(
                runs.len() == 1
                    && runs[0].0 == child.id
                    && runs[0].1.following_note.is_none()
                    && runs[0].1.input.starts_with(text)
                    && runs[0].1.input.contains("[Mighty Code]")
                    && runs[0].1.input.contains(&child.branch)
                    && runs[0].1.input.contains(&report_file_in(&child.worktree_path)),
                "the send's own start runs the human's text first"
            );
            assert!(
                fixture.place.fixture_git(&["rev-parse", &format!("refs/heads/{}", child.branch)]).await == head
                    && fixture.place.fixture_git(&["symbolic-ref", "--short", "HEAD"]).await == "main"
                    && fixture.place.fixture_git(&["status", "--porcelain", "--untracked-files=all"]).await.is_empty(),
                "the parent's checkout is untouched"
            );
            fixture.coordinator.child_run_ended(&child.id, &run_id, ChildRunEnd::Finished).await;
            assert!(
                fixture.only().state == ChildState::Ended && fixture.coordinator.file().notices.is_empty(),
                "an end without a report sends no notice"
            );
            fixture.coordinator.child_run_started(&child.id, "run-2").await;
            fixture.place.commit(&child.worktree_path, "note.txt", "Note\n").await;
            std::fs::write(report_file_in(&child.worktree_path), "# Report\n\nWrote the note.\n").unwrap();
            assert!(fixture.coordinator.child_run_ended(&child.id, "run-2", ChildRunEnd::Finished).await.is_none());
            let only = fixture.only();
            assert!(
                only.state == ChildState::Reported
                    && only.report_revision == 1
                    && fixture.coordinator.file().notices.is_empty()
                    && fixture.coordinator.file().held_items_by_pane().is_empty()
                    && sidebar::rows(&fixture.coordinator.file())[0].card_actions() == [CardAction::Merge, CardAction::Discard],
                "a report reaches its card and no notice its parent"
            );
        })
        .await;

        Box::pin(async {
            // A failed start is shown on its card with no notice, and the window hears of it; so does a first run that does not start.
            let fixture = make_with("claude", "acceptEdits").await;
            fixture.host.pane_fails.store(true, Ordering::SeqCst);
            let child = started(fixture.send("Write a note.", "parent", false).await);
            assert!(
                fixture.wait_for(&child.id, ChildState::Failed).await
                    && fixture.sends.started().is_empty()
                    && fixture.sends.failed() == [child.id.clone()]
                    && fixture.coordinator.file().notices.is_empty()
                    && sidebar::rows(&fixture.coordinator.file())[0].card_actions() == [CardAction::Discard],
                "a pane that could not be made fails the child, with no notice"
            );
            let fixture = make_with("claude", "acceptEdits").await;
            fixture.sends.run_fails.store(true, Ordering::SeqCst);
            let child = started(fixture.send("Write a note.", "parent", false).await);
            assert!(
                fixture.wait_for(&child.id, ChildState::Failed).await
                    && fixture.sends.started().iter().map(|r| r.0.clone()).eq([child.id.clone()])
                    && fixture.sends.failed() == [child.id.clone()]
                    && fixture.coordinator.file().notices.is_empty(),
                "a first run that did not start fails the child"
            );
        })
        .await;

        Box::pin(async {
            // A slash command runs alone and the note follows it; a plan-first style starts the child in plan mode where its agent has
            // one.
            let fixture = make_with("claude", "fullAccess").await;
            let command = started(fixture.send("/review the parser", "parent", false).await);
            assert!(fixture.wait_for(&command.id, ChildState::Running).await);
            let first = fixture.sends.started()[0].1.clone();
            assert!(
                first.input == "/review the parser"
                    && first.following_note.as_ref().is_some_and(|n| n.starts_with("[Mighty Code]") && n.contains(&command.branch)),
                "a slash command runs alone"
            );
            let planned = started(fixture.send("Plan the parser.", "parent", true).await);
            assert!(
                planned.starting_mode == "plan"
                    && fixture.wait_for(&planned.id, ChildState::Running).await
                    && fixture.host.created_panes().last().unwrap().mode == "plan",
                "a plan-first style starts the child in plan mode"
            );
            let fixture = make_with("codex", "onRequest").await;
            assert_eq!(
                started(fixture.send("Plan it.", "parent", true).await).starting_mode,
                "onRequest",
                "Codex has no plan mode: the child keeps the parent's mode"
            );
        })
        .await;
    }

    /// "delegation: the parent's tools and caps leave the human's child alone"
    #[tokio::test]
    async fn the_parents_tools_and_caps_leave_the_humans_child_alone() {
        let _lane = lane::enter().await;
        Box::pin(async {
            let file = DelegationFile::of(vec![open_child("agent", false), open_child("human", true)], vec![], vec![], vec![], vec![]);
            let fixture = make(Some(1), Some(file), "acceptEdits", "claude", 50_000_000_000, true).await;
            let bytes = fixture.bytes();
            assert!(
                fixture.call("list_children", &[]).await.children.is_some_and(|c| c.iter().map(|c| c.id.as_str()).eq(["agent"])),
                "list_children leaves it out"
            );
            assert!(
                fixture.call("child_status", &[("child", "human")]).await == DelegationResponse::failure(not_your_child_message("human"))
                    && fixture.call("child_status", &[("child", "agent")]).await.status.map(|s| s.id) == Some("agent".into()),
                "child_status does not know it"
            );
            assert!(
                fixture.call("merge", &[("child", "human"), ("expected_head", &"b".repeat(40))]).await
                    == DelegationResponse::refusal(DelegationReasonCode::HumanStarted)
                    && fixture.call("follow_up", &[("child", "human"), ("text", "Also do this.")]).await
                        == DelegationResponse::refusal(DelegationReasonCode::HumanStarted)
                    && fixture.bytes() == bytes,
                "merge and follow_up are refused with human_started"
            );
            let codex = DelegationCaller {
                provider: "codex".into(),
                delegation: true,
                permission_mode: "acceptEdits".into(),
                ..DelegationCaller::new("parent")
            };
            assert_eq!(
                fixture.coordinator.handle_call(request("merge", &[("child", "agent"), ("expected_head", COMMIT)]), codex).await,
                DelegationResponse::refusal(DelegationReasonCode::MergeHumanOnly),
                "a Codex acceptEdits merge is the human's"
            );
        })
        .await;

        Box::pin(async {
            // Human and agent children each have their own cap of three.
            let agents =
                DelegationFile::of(["a", "b", "c"].iter().map(|id| open_child(id, false)).collect(), vec![], vec![], vec![], vec![]);
            let fixture = make(Some(1), Some(agents), "acceptEdits", "claude", 50_000_000_000, true).await;
            let child = started(fixture.send("One of mine.", "parent", false).await);
            assert!(
                fixture.wait_for(&child.id, ChildState::Running).await
                    && fixture.call("delegate", &[("task", "One more."), ("mode", "plan")]).await
                        == DelegationResponse::refusal(DelegationReasonCode::WidthCap),
                "three agent children leave room for the human's"
            );
            let humans =
                DelegationFile::of(["a", "b", "c"].iter().map(|id| open_child(id, true)).collect(), vec![], vec![], vec![], vec![]);
            let fixture = make(Some(1), Some(humans), "acceptEdits", "claude", 50_000_000_000, true).await;
            let bytes = fixture.bytes();
            assert_eq!(
                fixture.send("One more.", "parent", false).await,
                HumanChildStart::Refused(DelegationReasonCode::WidthCap),
                "a fourth of the human's is refused"
            );
            fixture.nothing_made(&bytes, true).await;
            let delegated = fixture
                .call("delegate", &[("task", "Split this."), ("mode", "plan")])
                .await
                .child
                .expect("three of the human's leave room for the parent's delegate");
            assert!(fixture.wait_for(&delegated.id, ChildState::Running).await);
        })
        .await;

        Box::pin(async {
            // A folder that cannot host a worktree, and delegate's own limits, are refused and nothing is made.
            for (commits, reason) in [(None, DelegationReasonCode::NotGit), (Some(0), DelegationReasonCode::UnbornBranch)] {
                let fixture = make(commits, None, "acceptEdits", "claude", 50_000_000_000, true).await;
                let bytes = fixture.bytes();
                assert_eq!(fixture.send("Try it.", "parent", false).await, HumanChildStart::Refused(reason));
                fixture.nothing_made(&bytes, commits.is_some()).await;
            }
            let fixture = make(Some(1), None, "acceptEdits", "claude", 1_000_000_000, true).await;
            let bytes = fixture.bytes();
            assert_eq!(fixture.send("Try it.", "parent", false).await, HumanChildStart::Refused(DelegationReasonCode::LowDisk), "low_disk");
            fixture.nothing_made(&bytes, true).await;
            let fixture = make_with("claude", "acceptEdits").await;
            let bytes = fixture.bytes();
            let pane = |id: &str, mode: &str, kind: &str, parent: Option<&str>| DelegationPaneState {
                session_id: id.into(),
                permission_mode: mode.into(),
                folder: fixture.place.repo(),
                kind: kind.into(),
                provider: "claude".into(),
                parent_session_id: parent.map(str::to_string),
                run_id: Some("run-x".into()),
                activity: DelegationPaneActivity::Running,
            };
            fixture.host.set(pane("child", "auto", "claude", Some("parent")));
            fixture.host.set(pane("shell", "acceptEdits", "shell", None));
            assert!(
                fixture.send("Split further.", "child", false).await == HumanChildStart::Refused(DelegationReasonCode::ChildCannotDelegate)
                    && fixture.send("Try it.", "shell", false).await == HumanChildStart::Refused(DelegationReasonCode::ClaudeOnly)
                    && fixture.send("Try it.", "gone", false).await == HumanChildStart::Refused(DelegationReasonCode::ParentClosed),
                "a child, a pane that is no agent pane and a closed pane have no new worktree"
            );
            let invalid = HumanChildStart::Failed(locale().get("delegation.human.error.invalidTask"));
            assert!(
                fixture.send("  \n", "parent", false).await == invalid
                    && fixture.send(&"x".repeat(MAXIMUM_COPY_BYTES + 1), "parent", false).await == invalid,
                "no text is no task"
            );
            fixture.nothing_made(&bytes, true).await;
            let fixture = make(Some(1), None, "acceptEdits", "claude", 50_000_000_000, false).await;
            let bytes = fixture.bytes();
            assert_eq!(
                fixture.send("Try it.", "parent", false).await,
                HumanChildStart::Failed(locale().get("delegation.human.error.switchedOff")),
                "switched off"
            );
            fixture.nothing_made(&bytes, true).await;
            for (provider, mode) in [("codex", "manual"), ("gemini", "manual"), ("gemini", "plan")] {
                let fixture = make_with(provider, mode).await;
                let bytes = fixture.bytes();
                assert_eq!(
                    fixture.send("Try it.", "parent", false).await,
                    HumanChildStart::Failed(locale().get("delegation.human.error.readOnlyMode")),
                    "{provider} {mode} makes no child"
                );
                fixture.nothing_made(&bytes, true).await;
            }
        })
        .await;
    }

    /// MC-76: another agent's child the person starts begins in the mode that agent's last-used pane is in, wider than the parent's or
    /// not, as a new pane of it would; with no such pane, the widest no wider than the parent's.
    #[tokio::test]
    async fn another_agents_child_starts_in_its_last_used_panes_mode() {
        let _lane = lane::enter().await;
        Box::pin(async {
            let fixture = make_with("claude", "acceptEdits").await;
            fixture.host.set_agent("codex", mighty_delegation::host::DelegationAgentAvailability::Ready);
            let at = |latest: Option<&str>| HumanChildAt { latest_mode: latest.map(str::to_string), ..HumanChildAt::default() };
            let start = fixture
                .coordinator
                .start_human_child_at("Try it.", "parent", fixture.sends.starter(), false, Some("codex"), at(Some("fullAccess")))
                .await;
            let child = started(start);
            assert_eq!((child.provider.as_deref(), child.starting_mode.as_str()), (Some("codex"), "fullAccess"), "the Codex pane's mode");
            assert!(fixture.wait_for(&child.id, ChildState::Running).await, "it runs");
            let plain = started(
                fixture.coordinator.start_human_child_at("Again.", "parent", fixture.sends.starter(), false, Some("codex"), at(None)).await,
            );
            assert_ne!(plain.starting_mode, "fullAccess", "with no Codex pane, no wider than the parent's");
        })
        .await;
    }

    /// MC-45: a compare run's children count against their own cap of four, apart from the human's three and the delegate's three.
    #[tokio::test]
    async fn a_compare_runs_children_have_their_own_cap_of_four() {
        let _lane = lane::enter().await;
        let compare = |group: &str| HumanChildAt { base_commit: None, compare_group: Some(group.into()), latest_mode: None };
        let compare_child = |id: &str| ChildRecord { compare_group: Some("g0".into()), ..open_child(id, true) };
        Box::pin(async {
            // Three of the human's and three of the delegate's open: a compare run's children still start, up to four.
            let open: Vec<ChildRecord> = ["h1", "h2", "h3"]
                .iter()
                .map(|id| open_child(id, true))
                .chain(["a1", "a2", "a3"].iter().map(|id| open_child(id, false)))
                .collect();
            let fixture = make(
                Some(1),
                Some(DelegationFile::of(open, vec![], vec![], vec![], vec![])),
                "acceptEdits",
                "claude",
                50_000_000_000,
                true,
            )
            .await;
            let mut ids = vec![];
            for _ in 0..4 {
                let start =
                    fixture.coordinator.start_human_child_at("Try it.", "parent", fixture.sends.starter(), false, None, compare("g")).await;
                let child = started(start);
                assert!(child.compare_group.as_deref() == Some("g") && child.kind() == ChildKind::Compare && !child.notifies_parent());
                assert!(fixture.wait_for(&child.id, ChildState::Running).await, "it runs");
                ids.push(child.id);
            }
            assert_eq!(
                (
                    fixture.coordinator.open_children("parent", ChildKind::Compare),
                    fixture.coordinator.open_children("parent", ChildKind::Human)
                ),
                (4, 3)
            );
            assert_eq!(
                fixture.coordinator.start_human_child_at("Try it.", "parent", fixture.sends.starter(), false, None, compare("g2")).await,
                HumanChildStart::Refused(DelegationReasonCode::WidthCap),
                "a fifth compare child is refused"
            );
            assert_eq!(
                fixture.send("One more.", "parent", false).await,
                HumanChildStart::Refused(DelegationReasonCode::WidthCap),
                "the human's own cap is still three"
            );
            let saved = fixture.store().load().unwrap();
            assert!(
                ids.iter().all(|id| saved.child(id).and_then(|c| c.compare_group.as_deref()) == Some("g")),
                "the group is saved with each record"
            );
            assert!(String::from_utf8(fixture.bytes()).unwrap().contains("\"compareGroup\":\"g\""));
        })
        .await;

        Box::pin(async {
            // Four compare children open: the human's send and the delegate still have their room.
            let open: Vec<ChildRecord> = ["c1", "c2", "c3", "c4"].iter().map(|id| compare_child(id)).collect();
            let fixture = make(
                Some(1),
                Some(DelegationFile::of(open, vec![], vec![], vec![], vec![])),
                "acceptEdits",
                "claude",
                50_000_000_000,
                true,
            )
            .await;
            let bytes = fixture.bytes();
            assert_eq!(
                fixture.coordinator.start_human_child_at("Try it.", "parent", fixture.sends.starter(), false, None, compare("g1")).await,
                HumanChildStart::Refused(DelegationReasonCode::WidthCap)
            );
            fixture.nothing_made(&bytes, true).await;
            let child = started(fixture.send("Mine.", "parent", false).await);
            assert!(child.compare_group.is_none() && child.kind() == ChildKind::Human);
            assert!(fixture.wait_for(&child.id, ChildState::Running).await);
            assert!(fixture.call("delegate", &[("task", "Split this."), ("mode", "plan")]).await.child.is_some(), "the delegate's room");
        })
        .await;
    }

    /// "delegation: the app commits a Codex or Antigravity child's work" — the coordinator's half: a Codex or Antigravity child's run
    /// end commits through W7-B's `auto_commit::run`, records the report at that head, and the pane's notes say what stayed out or why
    /// nothing was committed.
    #[tokio::test]
    async fn the_app_commits_a_codex_or_antigravity_childs_work_the_coordinators_half() {
        let _lane = lane::enter().await;
        Box::pin(async {
            for provider in ["codex", "gemini"] {
                let fixture = make_with(provider, "acceptEdits").await;
                let child = started(fixture.send("Try the other parser.", "parent", false).await);
                assert!(
                    child.provider.as_deref() == Some(provider)
                        && child.agent() == provider
                        && fixture.wait_for(&child.id, ChildState::Running).await
                        && fixture.host.created_panes().iter().map(|p| p.provider.clone()).eq([provider.to_string()])
                        && fixture.store().load().unwrap().children[0].provider.as_deref() == Some(provider),
                    "{provider}'s child runs its agent"
                );
                let run = fixture.sends.started()[0].1.input.clone();
                assert!(
                    run.starts_with("Try the other parser.")
                        && !run.contains("Claude pane")
                        && !run.contains("commit your work")
                        && run.contains("Do not commit"),
                    "{provider}'s note leaves the commit to the app"
                );
            }
        })
        .await;

        Box::pin(async {
            let fixture = make_with("codex", "acceptEdits").await;
            let repo = fixture.place.repo();
            std::fs::write(Path::new(&repo).join(".gitignore"), "*.log\n").unwrap();
            fixture.place.fixture_git(&["add", ".gitignore"]).await;
            fixture.place.fixture_git(&["commit", "-q", "-m", "ignore logs"]).await;
            let start = fixture.place.fixture_git(&["rev-parse", "HEAD"]).await;
            let child = started(fixture.send("Add a note.", "parent", false).await);
            assert!(fixture.wait_for(&child.id, ChildState::Running).await, "the child runs");
            let worktree = child.worktree_path.clone();
            let report = report_file_in(&worktree);
            let head = || fixture.place.git_in(&worktree, &["rev-parse", "HEAD"]);
            for (name, text) in [("file-0.txt", "changed\n"), ("note.txt", "Note\n"), ("build.log", "noise\n")] {
                std::fs::write(Path::new(&worktree).join(name), text).unwrap();
            }
            fixture.coordinator.child_run_ended(&child.id, &format!("send-run-of-{}", child.id), ChildRunEnd::Finished).await;
            assert!(head().await == start && fixture.only().state == ChildState::Ended, "no report, no commit");
            fixture.coordinator.child_run_started(&child.id, "run-stopped").await;
            std::fs::write(&report, "# Report\n\nAdded the note.\n").unwrap();
            fixture.coordinator.child_run_ended(&child.id, "run-stopped", ChildRunEnd::Stopped).await;
            assert!(head().await == start && fixture.only().report_revision == 0, "a stopped run commits nothing");
            fixture.coordinator.child_run_started(&child.id, "run-2").await;
            assert!(
                fixture.coordinator.child_run_ended(&child.id, "run-2", ChildRunEnd::Finished).await.is_none(),
                "no notice for the human's child"
            );
            let committed = head().await;
            let changed = fixture.place.git_in(&worktree, &["diff", "--name-only", &start, &committed]).await;
            assert!(
                committed != start
                    && fixture.place.git_in(&worktree, &["log", "-1", "--format=%s"]).await == auto_commit::message_for(&child.id, 1)
                    && fixture.place.git_in(&worktree, &["rev-parse", "HEAD^"]).await == start
                    && changed.split('\n').eq(["file-0.txt", "note.txt"])
                    && fixture.place.git_in(&worktree, &["status", "--porcelain"]).await.is_empty(),
                "the app's one commit holds what .gitignore lets in"
            );
            let only = fixture.only();
            assert!(
                only.state == ChildState::Reported
                    && only.report_revision == 1
                    && only.report_head.as_deref() == Some(committed.as_str())
                    && fixture.host.notes().is_empty(),
                "the report is recorded at that head"
            );
            assert!(
                matches!(fixture.coordinator.merge_from_card(&child.id).await, ChildMergeOutcome::Merged(_))
                    && fixture.place.fixture_git(&["rev-parse", "HEAD"]).await == committed
                    && Path::new(&repo).join("note.txt").exists(),
                "the card merges it"
            );
            fixture.coordinator.child_run_started(&child.id, "run-3").await;
            std::fs::write(Path::new(&worktree).join("note.txt"), "more\n").unwrap();
            fixture.coordinator.child_run_ended(&child.id, "run-3", ChildRunEnd::Finished).await;
            assert_eq!(head().await, committed, "the same report commits nothing");
        })
        .await;

        Box::pin(async {
            // New files that look secret or are large stay uncommitted, and the pane names them.
            let fixture = make_with("gemini", "acceptEdits").await;
            let child = started(fixture.send("Add the config.", "parent", false).await);
            assert!(fixture.wait_for(&child.id, ChildState::Running).await, "the child runs");
            let worktree = child.worktree_path.clone();
            std::fs::create_dir_all(Path::new(&worktree).join("keys")).unwrap();
            let secrets = [".env.local", "keys/server.pem", "keys/deploy.key", "id_rsa", "credentials.json"];
            for name in secrets.iter().chain(["config.json"].iter()) {
                std::fs::write(Path::new(&worktree).join(name), "x\n").unwrap();
            }
            std::fs::write(Path::new(&worktree).join("big.bin"), vec![0u8; auto_commit::MAXIMUM_NEW_FILE_BYTES as usize + 1]).unwrap();
            std::fs::write(Path::new(&worktree).join("file-0.txt"), "changed\n").unwrap();
            std::fs::write(report_file_in(&worktree), "# Report\n").unwrap();
            fixture.coordinator.child_run_ended(&child.id, &format!("send-run-of-{}", child.id), ChildRunEnd::Finished).await;
            let mut committed: Vec<String> = fixture
                .place
                .git_in(&worktree, &["show", "--name-only", "--format=", "HEAD"])
                .await
                .split('\n')
                .map(String::from)
                .collect();
            committed.sort();
            assert_eq!(committed, ["config.json", "file-0.txt"], "the commit");
            let mut out: Vec<String> = fixture
                .place
                .git_in(&worktree, &["ls-files", "--others", "--exclude-standard"])
                .await
                .split('\n')
                .map(String::from)
                .collect();
            out.sort();
            let mut expected: Vec<String> = secrets.iter().map(|s| s.to_string()).chain(["big.bin".to_string()]).collect();
            expected.sort();
            assert_eq!(out, expected, "what stayed out");
            let notes = fixture.host.notes();
            let (session, text) = notes.last().unwrap();
            let lead = locale().get_with("delegation.autoCommit.leftOut", &[("count", "6"), ("paths", "")]);
            assert!(
                *session == child.id
                    && text.starts_with(&lead)
                    && expected.iter().all(|name| text.contains(name.as_str()))
                    && fixture.only().state == ChildState::Reported,
                "the pane names what stayed out: {text}"
            );
        })
        .await;

        Box::pin(async {
            // An unresolved conflict is never committed, and the pane says so; resolved, the next end commits the merge.
            let fixture = make_with("codex", "acceptEdits").await;
            let repo = fixture.place.repo();
            fixture.place.fixture_git(&["checkout", "-q", "-b", "other"]).await;
            std::fs::write(Path::new(&repo).join("file-0.txt"), "theirs\n").unwrap();
            fixture.place.fixture_git(&["commit", "-q", "-am", "theirs"]).await;
            fixture.place.fixture_git(&["checkout", "-q", "main"]).await;
            let child = started(fixture.send("Change file-0.", "parent", false).await);
            assert!(fixture.wait_for(&child.id, ChildState::Running).await, "the child runs");
            let worktree = child.worktree_path.clone();
            std::fs::write(Path::new(&worktree).join("file-0.txt"), "ours\n").unwrap();
            fixture.place.git_in(&worktree, &["commit", "-q", "-am", "ours"]).await;
            let ours = fixture.place.git_in(&worktree, &["rev-parse", "HEAD"]).await;
            assert!(
                git::run(&fixture.place.git(), &["merge", "-q", "other"], &worktree, 10.0).await.is_some_and(|r| r.exit_code != 0),
                "the merge conflicts"
            );
            std::fs::write(report_file_in(&worktree), "# Report\n\nMerged other.\n").unwrap();
            fixture.coordinator.child_run_ended(&child.id, &format!("send-run-of-{}", child.id), ChildRunEnd::Finished).await;
            let only = fixture.only();
            assert!(
                fixture.place.git_in(&worktree, &["rev-parse", "HEAD"]).await == ours
                    && !fixture.place.git_in(&worktree, &["ls-files", "-u"]).await.is_empty()
                    && only.state == ChildState::Ended
                    && only.report_revision == 0
                    && only.report_digest.is_none()
                    && fixture.host.notes() == [(child.id.clone(), locale().get("delegation.autoCommit.unmerged"))],
                "nothing is committed and the pane says why"
            );
            std::fs::write(Path::new(&worktree).join("file-0.txt"), "both\n").unwrap();
            fixture.place.git_in(&worktree, &["add", "file-0.txt"]).await;
            fixture.coordinator.child_run_started(&child.id, "run-2").await;
            fixture.coordinator.child_run_ended(&child.id, "run-2", ChildRunEnd::Finished).await;
            let merged = fixture.place.git_in(&worktree, &["rev-parse", "HEAD"]).await;
            assert!(
                merged != ours
                    && fixture.place.git_in(&worktree, &["rev-list", "--parents", "-n", "1", "HEAD"]).await.split(' ').count() == 3
                    && fixture.only().report_head.as_deref() == Some(merged.as_str()),
                "resolved, the merge is committed"
            );
        })
        .await;

        Box::pin(async {
            // The app never commits for a Claude child.
            let fixture = make_with("claude", "acceptEdits").await;
            let start = fixture.place.fixture_git(&["rev-parse", "HEAD"]).await;
            let child = started(fixture.send("Add a note.", "parent", false).await);
            assert!(fixture.wait_for(&child.id, ChildState::Running).await, "the child runs");
            std::fs::write(
                Path::new(&child.worktree_path).join("note.txt"),
                "Note
",
            )
            .unwrap();
            std::fs::write(
                report_file_in(&child.worktree_path),
                "# Report
",
            )
            .unwrap();
            fixture.coordinator.child_run_ended(&child.id, &format!("send-run-of-{}", child.id), ChildRunEnd::Finished).await;
            assert!(
                fixture.place.git_in(&child.worktree_path, &["rev-parse", "HEAD"]).await == start
                    && fixture.place.git_in(&child.worktree_path, &["status", "--porcelain"]).await == "?? note.txt"
                    && fixture.only().report_head.as_deref() == Some(start.as_str()),
                "a Claude child commits its own work"
            );
        })
        .await;
    }
}
