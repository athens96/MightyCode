//! The delegation scenarios (C# DelegationScenarioVerification, the Mac's DelegationScenarioS1Tests.swift and S2, S3), headless through
//! the real run path: the run service, the desktop service's delegation, the named pipe, a scripted fake Claude. Each makes its
//! repository and profile in a temp folder and is judged from what is on disk, by the rules scripts/delegation-record.py checks.
//! Windows only while the transport is (W7 plan D4).
#![cfg(windows)]

mod support;

use std::collections::HashSet;
use std::path::Path;

use mighty_delegation::cards::DelegationCardResult;
use mighty_delegation::copy::DelegationCopyKind;
use mighty_delegation::delivery::{DelegationRelease, follow_up_text, notice_text};
use mighty_delegation::git::paths;
use mighty_delegation::git::worktree::report_file_in;
use mighty_delegation::model::{ChildState, DeliveryLane, DeliveryRoute, MergeKind, NoticeKind};
use mighty_delegation::providers;
use mighty_delegation::store::DelegationFileStore;
use mighty_delegation::tools;
use serde_json::Value;
use support::launch::{DelegationLaunch, PARENT, awaited, steps};
use support::{lane, place::DelegationPlace};

const DELEGATE_TOOL: &str = "mcp__mighty-delegation__delegate";
const MERGE_TOOL: &str = "mcp__mighty-delegation__merge";

fn shortened(text: &str) -> String {
    mighty_core::normalize::shortened(text).unwrap()
}

fn events<'a>(log: &'a [Value], name: &str) -> Vec<&'a Value> {
    log.iter().filter(|entry| entry["event"] == name).collect()
}

/// "delegation scenario S1: two children, a human's approval, a fast-forward and a discard": a parent in an asking mode delegates two
/// children, a child's approval only a human answers, both report, the parent fast-forwards child 1 and a human discards child 2 from
/// its card.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s1_two_children_a_humans_approval_a_fast_forward_and_a_discard() {
    Box::pin(s1()).await;
}

async fn s1() {
    let _lane = lane::enter().await;
    let place = DelegationPlace::new();
    let start = place.repository_default().await;
    let mode = "acceptEdits";
    let s = DelegationLaunch::start(
        Path::new(&place.profile()),
        &Path::new(&place.base()).join("fake"),
        &place.repo(),
        &place.root(),
        place.git(),
        mode,
        true,
    )
    .await;
    let task1 = "Feature one: add feature-one.txt saying one.";
    let task2 = "Feature two: add feature-two.txt saying two.";
    let report = steps::report();
    let child_steps = |file: &str, text: &str, words: &str| {
        let mut list = steps::commit(file, text);
        list.push(steps::write(&report, &format!("# Report\n\n{words}\n")));
        list.push(steps::say("Committed and reported."));
        list
    };
    s.child_script(task1, child_steps("feature-one.txt", "one\n", "Added feature-one.txt."));
    s.child_script(task2, child_steps("feature-two.txt", "two\n", "Added feature-two.txt."));

    // 1. Each delegate call waits on a human approval that shows the starting mode asked for.
    s.script(
        &place.repo(),
        vec![vec![
            steps::call("delegate", true, &[("task", task1), ("mode", mode)]),
            steps::call("delegate", true, &[("task", task2), ("mode", "manual")]),
            steps::say("Two children are on it."),
        ]],
    );
    let first = s.human_send("Delegate feature one at your own mode and feature two at manual.", PARENT).await;
    let approve_one = awaited("the first delegate approval", || s.open_card(PARENT, DELEGATE_TOOL, &[])).await;
    assert!(
        DelegationLaunch::shown(&approve_one, "mode").as_deref() == Some(mode)
            && DelegationLaunch::shown(&approve_one, "task").as_deref() == Some(task1),
        "the first approval shows its task and mode"
    );
    s.allow(&approve_one).await;
    let approve_two = awaited("the second delegate approval", || s.open_card(PARENT, DELEGATE_TOOL, &[&approve_one.id])).await;
    assert!(
        DelegationLaunch::shown(&approve_two, "mode").as_deref() == Some("manual")
            && DelegationLaunch::shown(&approve_two, "task").as_deref() == Some(task2),
        "the second approval shows its task and mode"
    );
    s.allow(&approve_two).await;
    assert_eq!(s.ended(PARENT, &first).await, "completed", "the delegating run");
    let (one, two) = awaited("both children on disk", || {
        let file = s.file();
        let a = file.children.iter().find(|child| child.starting_mode == mode).cloned();
        let b = file.children.iter().find(|child| child.starting_mode == "manual").cloned();
        (file.children.len() == 2).then_some(()).and(a.zip(b))
    })
    .await;

    // 2. Both children ask a human before committing; child 2's request stays open while the parent's run for child 1's notice tries
    //    to answer it, which its tools cannot.
    let ask_two = awaited("child 2's approval request", || s.open_card(&two.id, "Bash", &[])).await;
    let ask_one = awaited("child 1's approval request", || s.open_card(&one.id, "Bash", &[])).await;
    let pane_one = s.pane(&one.id);
    assert!(
        pane_one.parent_session_id.as_deref() == Some(PARENT)
            && pane_one.title_mode.as_deref() == Some("fixed")
            && pane_one.settings.as_ref().unwrap().permission_mode == mode
            && s.pane(&two.id).settings.as_ref().unwrap().permission_mode == "manual"
            && pane_one.working_folder.as_deref() == Some(one.worktree_path.as_str())
            && pane_one.title.as_deref() == Some(shortened(task1).as_str()),
        "each child pane is linked, titled by its task and in the mode its parent asked for"
    );
    s.script(
        &place.repo(),
        vec![vec![
            steps::call("list_children", false, &[]),
            steps::call("answer_permission", false, &[("child", &two.id), ("request", &ask_two.id), ("behavior", "allow")]),
            steps::call("child_status", false, &[("child", &two.id)]),
            steps::say("Child 2 waits for its human."),
        ]],
    );
    s.allow(&ask_one).await;
    let notice_run_one = awaited("child 1's notice starting a parent run", || s.runs(PARENT).get(1).cloned()).await;
    assert_eq!(s.ended(PARENT, &notice_run_one).await, "completed", "child 1's notice run");
    assert!(
        s.open_card(&two.id, "Bash", &[]).map(|card| card.id) == Some(ask_two.id.clone())
            && !s.log(&two.worktree_path).iter().any(|entry| entry["event"] == "permission")
            && s.file().children.iter().find(|child| child.id == two.id).unwrap().report_revision == 0,
        "child 2's request waits for its human"
    );

    // A human answers child 2; it commits and reports too.
    s.script(&place.repo(), vec![vec![steps::call("list_children", false, &[]), steps::say("Both children reported.")]]);
    s.allow(&ask_two).await;
    let notice_run_two = awaited("child 2's notice starting a parent run", || s.runs(PARENT).get(2).cloned()).await;
    assert_eq!(s.ended(PARENT, &notice_run_two).await, "completed", "child 2's notice run");
    let head1 = awaited("child 1's reported head", || {
        let file = s.file();
        file.children.iter().all(|child| child.state == ChildState::Reported).then_some(())?;
        file.children.iter().find(|child| child.id == one.id)?.report_head.clone()
    })
    .await;

    // 3. Asked by the human, the parent fast-forwards child 1 through the merge tool, which asks first in this mode.
    s.script(
        &place.repo(),
        vec![vec![
            steps::call("child_status", false, &[("child", &one.id)]),
            steps::call("merge", true, &[("child", &one.id), ("expected_head", &head1)]),
            steps::say("Merged child 1."),
        ]],
    );
    let merge_run = s.human_send("Merge child 1 if its report is right.", PARENT).await;
    let approve_merge = awaited("the merge approval", || s.open_card(PARENT, MERGE_TOOL, &[])).await;
    assert!(
        DelegationLaunch::shown(&approve_merge, "child").as_deref() == Some(one.id.as_str())
            && DelegationLaunch::shown(&approve_merge, "expected_head").as_deref() == Some(head1.as_str()),
        "the merge approval shows the child and its head"
    );
    s.allow(&approve_merge).await;
    assert_eq!(s.ended(PARENT, &merge_run).await, "completed", "the merge run");

    // 4. A human discards child 2 from its card: its confirmation, then the discard, which closes its pane.
    let pending = s.app.ask_to_discard_child(&two.id).await.expect("a discard confirmation");
    assert!(
        pending.nested.as_ref().is_some_and(Vec::is_empty)
            && pending.branch == format!("mighty/{}", two.id)
            && pending.title == shortened(task2),
        "the confirmation names the child and no nested worktree"
    );
    let discarded = s.app.discard_child(&pending).await;
    assert!(
        discarded.is_some_and(|note| note.result == DelegationCardResult::Done) && s.snapshot().session(&two.id).is_none(),
        "the discard is done and the child's pane closed"
    );
    s.dispose().await;

    // 5. The verdict, from disk alone.
    let saved = s.file();
    let ids: HashSet<&str> = saved.children.iter().map(|child| child.id.as_str()).collect();
    assert!(
        ids == HashSet::from([one.id.as_str(), two.id.as_str()])
            && saved
                .children
                .iter()
                .all(|child| child.parent_session_id == PARENT && child.parent_branch == "main" && child.base_commit == start),
        "two children of the parent, from main's head"
    );
    let child1 = saved.children.iter().find(|child| child.id == one.id).unwrap();
    let child2 = saved.children.iter().find(|child| child.id == two.id).unwrap();
    assert!(
        child1.starting_mode == mode && child2.starting_mode == "manual" && providers::is_no_wider(&child2.starting_mode, mode, "claude"),
        "each stored mode is its request: one the parent's own, one narrower"
    );
    assert!(
        child1.state == ChildState::Merged
            && child1.report_revision == 1
            && child1.report_head.as_deref() == Some(head1.as_str())
            && child2.state == ChildState::Discarded
            && child2.report_revision == 1
            && child2.report_head.as_ref().is_some_and(|head2| *head2 != head1),
        "child 1 merged, child 2 discarded, each reported once"
    );
    assert!(
        saved.copy(&one.id, DelegationCopyKind::Report).is_some_and(|copy| copy.text.contains("Added feature-one.txt."))
            && saved.copy(&two.id, DelegationCopyKind::Report).is_some_and(|copy| copy.text.contains("Added feature-two.txt.")),
        "both reports are kept"
    );
    let notice_ids: HashSet<&str> = saved.notices.iter().map(|notice| notice.id.as_str()).collect();
    assert!(saved.notices.len() == 2 && notice_ids.len() == 2 && saved.follow_ups.is_empty(), "one notice per child");
    for (child, run) in [(&one.id, &notice_run_one), (&two.id, &notice_run_two)] {
        let notice = saved.notices.iter().find(|notice| notice.child_id == *child).unwrap();
        let receipt = notice.receipt.as_ref().unwrap();
        assert!(
            notice.kind == NoticeKind::Reported
                && notice.report_revision == 1
                && notice.lane == DeliveryLane::Delivered
                && receipt.route == DeliveryRoute::Queue
                && receipt.run_id == *run,
            "each notice was the next run of the idle parent, once"
        );
    }
    let merge = &saved.merges;
    assert!(
        merge.len() == 1
            && merge[0].kind == MergeKind::ToolFastForward
            && merge[0].parent_branch == "main"
            && merge[0].child_id == one.id
            && merge[0].pre_merge_commit == start
            && merge[0].merged_commit == head1
            && merge[0].child_head == head1,
        "the tool's one fast-forward of main"
    );
    let repo = place.repo();
    assert_eq!(place.git_in(&repo, &["symbolic-ref", "--short", "HEAD"]).await, "main", "main is checked out");
    assert_eq!(place.git_in(&repo, &["rev-parse", "refs/heads/main"]).await, head1, "main is at child 1's head");
    assert_eq!(
        place.git_in(&repo, &["rev-list", "--parents", "-n", "1", &head1]).await,
        format!("{head1} {start}"),
        "with no merge commit"
    );
    assert!(
        place.git_in(&repo, &["status", "--porcelain", "--untracked-files=no"]).await.is_empty()
            && Path::new(&repo).join("feature-one.txt").exists()
            && !Path::new(&repo).join("feature-two.txt").exists(),
        "the parent's checkout has child 1's work only"
    );
    assert_eq!(place.mighty_branches().await, format!("refs/heads/{}", child1.branch), "child 1's branch stays, child 2's is gone");
    let listed = place.worktree_list().await;
    assert!(
        listed.iter().any(|path| paths::same(path, &place.paths().canonical(&child1.worktree_path)))
            && !Path::new(&child2.worktree_path).exists(),
        "child 1's worktree stays, child 2's is gone"
    );
    let parent_log = s.log(&repo);
    let launches = events(&parent_log, "argv");
    assert!(
        launches.len() == 4
            && launches.iter().all(|argv| {
                steps::option(argv, "--permission-mode").as_deref() == Some(mode)
                    && steps::option(argv, "--permission-prompt-tool").as_deref() == Some("stdio")
            }),
        "the parent ran in its asking mode every time"
    );
    let answers = events(&parent_log, "permission");
    assert_eq!(
        answers.iter().map(|entry| entry["tool"].as_str().unwrap_or("")).collect::<Vec<_>>(),
        [DELEGATE_TOOL, DELEGATE_TOOL, MERGE_TOOL],
        "a human allowed each delegate call and the merge"
    );
    assert!(answers.iter().all(|entry| entry["behavior"] == "allow"), "every answer allowed");
    let calls = events(&parent_log, "call");
    let delegated: Vec<Value> =
        calls.iter().filter(|call| call["tool"] == "delegate").map(|call| steps::structured(call)["child"].clone()).collect();
    assert_eq!(
        delegated.iter().map(|child| child["id"].as_str().unwrap_or("")).collect::<Vec<_>>(),
        [one.id.as_str(), two.id.as_str()],
        "the delegate answers name the children"
    );
    assert_eq!(delegated.iter().map(|child| child["mode"].as_str().unwrap_or("")).collect::<Vec<_>>(), [mode, "manual"], "and their modes");
    let listed_tools: Vec<&str> = events(&parent_log, "mcp")[0]["tools"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert_eq!(listed_tools, tools::ALL.map(|tool| tool.name), "the parent's server lists the six tools");
    let tried = calls.iter().find(|call| call["tool"] == "answer_permission").unwrap();
    assert!(
        tried["result"]["isError"] == true && steps::text(tried).contains("Unknown delegation tool answer_permission."),
        "the parent has no way to answer a child's approval"
    );
    assert!(calls.iter().find(|call| call["tool"] == "merge").unwrap()["result"]["isError"] != true, "the merge call succeeded");
    for child in [child1, child2] {
        let log = s.log(&child.worktree_path);
        let argv = events(&log, "argv");
        assert!(
            argv.len() == 1 && steps::option(argv[0], "--permission-mode").as_deref() == Some(child.starting_mode.as_str()),
            "{} ran once, in its stored mode",
            child.id
        );
        let asked: Vec<(String, String)> = events(&log, "permission")
            .iter()
            .map(|entry| (entry["tool"].as_str().unwrap_or("").to_string(), entry["behavior"].as_str().unwrap_or("").to_string()))
            .collect();
        let codes: Vec<i64> = events(&log, "bash").iter().filter_map(|entry| entry["code"].as_i64()).collect();
        assert!(asked == [("Bash".to_string(), "allow".to_string())] && codes == [0], "{} committed after one human answer", child.id);
    }
    let prompts = s.prompts(&repo);
    for notice in &saved.notices {
        assert_eq!(
            prompts.iter().filter(|prompt| prompt.contains(&notice.id)).count(),
            1,
            "each notice is in exactly one prompt the parent received"
        );
    }
    let resumed = s.session_of(&repo);
    assert!(
        launches.iter().skip(1).all(|argv| steps::option(argv, "--resume").as_deref() == Some(resumed.as_str())),
        "every later parent run resumed its one session"
    );
}

fn copy_folder(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_folder(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap_or_else(|error| panic!("{} was not copied: {error}", entry.path().display()));
        }
    }
}

/// The position of each needle in the prompts: (prompt, offset), or None.
fn position(prompts: &[String], needle: &str) -> Option<(usize, usize)> {
    prompts.iter().enumerate().find_map(|(at, prompt)| prompt.find(needle).map(|offset| (at, offset)))
}

/// "delegation scenario S2: held notices released together by a send or run next after a relaunch": three children; child 1's notice
/// reaches the idle parent; after the human stops the parent, children 2 and 3 report and their notices are held. The app quits, and
/// from that state each release route (the send, and run next with a queued human row behind it) is played on its own copy of the
/// profile and the Claude config, by a relaunched app: it starts no run by itself, and the route releases both notices together,
/// oldest first, each reaching the parent's transcript once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s2_held_notices_released_together_by_a_send_or_run_next_after_a_relaunch() {
    Box::pin(s2()).await;
}

async fn s2() {
    let _lane = lane::enter().await;
    let place = DelegationPlace::new();
    place.repository_default().await;
    let mode = "acceptEdits";
    let base = Path::new(&place.base()).to_path_buf();
    let first = base.join("launch-1");
    let tasks = ["Feature one: note what one needs.", "Feature two: note what two needs.", "Feature three: note what three needs."];
    let report = steps::report();
    let (children, notices, held) = {
        let s = DelegationLaunch::start(&first.join("profile"), &first, &place.repo(), &place.root(), place.git(), mode, true).await;
        for (index, task) in tasks.iter().enumerate() {
            s.child_script(
                task,
                vec![
                    steps::permission("Write", &[("file_path", &report)]),
                    steps::write(&report, &format!("# Report\n\nChild {} is done.\n", index + 1)),
                    steps::say("Reported."),
                ],
            );
        }
        // 1. The parent delegates three children and finishes normally.
        let mut turn: Vec<Value> = tasks.iter().map(|task| steps::call("delegate", false, &[("task", task), ("mode", mode)])).collect();
        turn.push(steps::say("Three children are on it."));
        s.script(&place.repo(), vec![turn]);
        let delegating = s.human_send("Split the three features.", PARENT).await;
        assert_eq!(s.ended(PARENT, &delegating).await, "completed", "the delegating run");
        let children = awaited("three children on disk", || {
            let file = s.file();
            (file.children.len() == 3).then_some(())?;
            let ordered: Vec<_> = tasks
                .iter()
                .filter_map(|task| {
                    file.children
                        .iter()
                        .find(|child| file.copy(&child.id, DelegationCopyKind::Task).is_some_and(|copy| copy.text.contains(task)))
                        .cloned()
                })
                .collect();
            (ordered.len() == 3).then_some(ordered)
        })
        .await;
        let mut asks = vec![];
        for child in &children {
            asks.push(awaited(&format!("{}'s request", child.id), || s.open_card(&child.id, "Write", &[])).await);
        }

        // 2. Child 1 reports while the parent is idle after its normal finish: its notice is the parent's next run.
        s.script(&place.repo(), vec![vec![steps::say("Child one reported.")]]);
        s.allow(&asks[0]).await;
        let notice_run = awaited("child 1's notice starting a parent run", || s.runs(PARENT).get(1).cloned()).await;
        assert_eq!(s.ended(PARENT, &notice_run).await, "completed", "child 1's notice run");
        let notice1 = awaited("child 1's notice delivered", || {
            s.file().notices.into_iter().find(|notice| notice.child_id == children[0].id && notice.lane == DeliveryLane::Delivered)
        })
        .await;
        let receipt = notice1.receipt.clone().unwrap();
        assert!(
            notice1.kind == NoticeKind::Reported
                && notice1.report_revision == 1
                && receipt.route == DeliveryRoute::Queue
                && receipt.run_id == notice_run
                && s.input(PARENT, &notice_run).await == Some(notice_text(&notice1)),
            "the notice alone was the parent's next run"
        );

        // 3. The human stops the parent's next run while it waits on a request.
        s.script(&place.repo(), vec![vec![steps::permission("Bash", &[("command", "dotnet build")]), steps::say("Built.")]]);
        let stopped = s.human_send("Check the build while they work.", PARENT).await;
        awaited("the parent's request in the run to stop", || s.open_card(PARENT, "Bash", &[])).await;
        s.service.stop(PARENT).await.unwrap();
        assert_eq!(s.ended(PARENT, &stopped).await, "stopped", "the stopped run");

        // 4. Children 2 and 3 report, in that order: both notices are held.
        for (child, ask) in children.iter().zip(&asks).skip(1) {
            s.allow(ask).await;
            awaited(&format!("{}'s notice held", child.id), || {
                s.file().notices.into_iter().find(|notice| notice.child_id == child.id && notice.lane == DeliveryLane::Held)
            })
            .await;
        }
        assert_eq!(s.runs(PARENT), [delegating, notice_run, stopped], "the parent ran nothing more");
        let held = s.file();
        let notices: Vec<_> =
            children.iter().map(|child| held.notices.iter().find(|notice| notice.child_id == child.id).cloned().unwrap()).collect();
        assert!(
            held.notices.len() == 3
                && notices.iter().map(|notice| notice.lane).collect::<Vec<_>>()
                    == [DeliveryLane::Delivered, DeliveryLane::Held, DeliveryLane::Held]
                && notices.iter().all(|notice| notice.kind == NoticeKind::Reported && notice.report_revision == 1)
                && held.children.iter().all(|child| child.state == ChildState::Reported),
            "one notice delivered and two held"
        );
        assert_eq!(
            s.app.held_items(PARENT).into_iter().map(|item| item.id).collect::<Vec<_>>(),
            [notices[1].id.clone(), notices[2].id.clone()],
            "the parent's queued list holds them, oldest first"
        );
        s.dispose().await;
        (children, notices, s.file())
    };
    for (index, route) in ["send", "runNext"].into_iter().enumerate() {
        let folder = base.join(format!("launch-{}", index + 2));
        for name in ["profile", "claude-config"] {
            copy_folder(&first.join(name), &folder.join(name));
        }
        let s = DelegationLaunch::start(&folder.join("profile"), &folder, &place.repo(), &place.root(), place.git(), mode, true).await;
        // The relaunched app restores the panes, none running, and starts no run by itself, not even in a delivery pass.
        s.coordinator().deliver_pending().await;
        assert!(
            s.runs(PARENT).is_empty()
                && children.iter().all(|child| s.runs(&child.id).is_empty())
                && s.file() == held
                && s.snapshot().sessions().iter().all(|pane| pane.status() != "running"),
            "{route}: nothing ran at launch"
        );
        assert!(
            children.iter().all(|child| {
                let pane = s.pane(&child.id);
                pane.parent_session_id.as_deref() == Some(PARENT) && pane.working_folder.as_deref() == Some(child.worktree_path.as_str())
            }),
            "{route}: the child panes came back linked"
        );
        let held_ids = vec![notices[1].id.clone(), notices[2].id.clone()];
        assert_eq!(s.app.held_items(PARENT).into_iter().map(|item| item.id).collect::<Vec<_>>(), held_ids, "{route}: both still held");
        s.script(&place.repo(), vec![vec![steps::say("Read both reports.")], vec![steps::say("Tests pass.")]]);
        let held_texts = [notice_text(&notices[1]), notice_text(&notices[2])];
        let (release_run, after) = if route == "send" {
            let after = "From the desktop: merge what is ready.";
            let DelegationRelease::Released { run_id, item_ids } = s.app.send(s.request(after, PARENT)).await.unwrap() else {
                panic!("the send released nothing")
            };
            assert_eq!(item_ids, held_ids, "the send released both");
            assert_eq!(
                s.input(PARENT, &run_id).await,
                Some(format!("{}\n\n{}\n\n{after}", held_texts[0], held_texts[1])),
                "oldest first, ahead of the human's text, in one run"
            );
            assert_eq!(s.ended(PARENT, &run_id).await, "completed", "the release run");
            assert_eq!(s.runs(PARENT), std::slice::from_ref(&run_id), "one run");
            (run_id, after)
        } else {
            // Run next: what is held first; with nothing held, the pane's next queued human row.
            let after = "Human row: run the tests.";
            let DelegationRelease::Released { run_id, item_ids } = s.app.run_next(PARENT).await else {
                panic!("run next released nothing")
            };
            assert_eq!(item_ids, held_ids, "run next released both");
            assert_eq!(s.input(PARENT, &run_id).await, Some(held_texts.join("\n\n")), "run next carries them alone");
            assert_eq!(s.ended(PARENT, &run_id).await, "completed", "the release run");
            assert_eq!(s.app.run_next(PARENT).await, DelegationRelease::NothingHeld, "then nothing is held");
            s.script(&place.repo(), vec![vec![steps::say("Tests pass.")]]);
            s.service.start(s.request(after, PARENT)).await.unwrap();
            let row_run = awaited("the queued row's run", || s.runs(PARENT).get(1).cloned()).await;
            assert_eq!(s.input(PARENT, &row_run).await.as_deref(), Some(after), "the queued human row runs next");
            assert_eq!(s.ended(PARENT, &row_run).await, "completed", "the row's run");
            (run_id, after)
        };
        let file = s.file();
        assert!(
            file.notices.iter().find(|notice| notice.id == notices[0].id) == Some(&notices[0]),
            "{route}: the first notice is as it was"
        );
        for notice in &notices[1..] {
            let saved = file.notices.iter().find(|saved| saved.id == notice.id).unwrap();
            let receipt = saved.receipt.as_ref().unwrap();
            assert!(
                saved.lane == DeliveryLane::Delivered && receipt.route == DeliveryRoute::Queue && receipt.run_id == release_run,
                "{route}: saved delivered by the release run"
            );
        }
        assert!(
            s.app.held_items(PARENT).is_empty()
                && s.coordinator().send("Again.", PARENT, None).await == DelegationRelease::NothingHeld
                && s.coordinator().run_next(PARENT).await == DelegationRelease::NothingHeld,
            "{route}: nothing more is held"
        );
        s.dispose().await;
        let prompts = s.prompts(&place.repo());
        for notice in &notices {
            assert_eq!(
                prompts.iter().filter(|prompt| prompt.contains(&notice.id)).count(),
                1,
                "{route}: each notice in exactly one prompt"
            );
        }
        assert_eq!(prompts.iter().filter(|prompt| prompt.contains(after)).count(), 1, "{route}: the human's text in one prompt");
        let positions: Vec<Option<(usize, usize)>> =
            [notices[1].id.as_str(), notices[2].id.as_str(), after].iter().map(|needle| position(&prompts, needle)).collect();
        assert!(
            positions.iter().all(Option::is_some) && positions.windows(2).all(|pair| pair[0] < pair[1]),
            "{route}: the released two oldest first and ahead of the human's text"
        );
    }
}

/// "delegation scenario S3: a diverged child's held follow-ups reach it once after a relaunch": children A and B from one base. A is
/// fast-forwarded into main; B's fast-forward is refused with diverged and changes nothing. A human stopped B after it reported, so
/// the parent's two follow-ups to B are held; the app quits, and the relaunched app hands them to B exactly once, with the human's next
/// send in B. B brings in main and reports revision 2, which the merge tool fast-forwards. A third follow-up is refused with
/// follow_up_limit.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s3_a_diverged_childs_held_follow_ups_reach_it_once_after_a_relaunch() {
    Box::pin(s3()).await;
}

async fn s3() {
    let _lane = lane::enter().await;
    let place = DelegationPlace::new();
    let start = place.repository_default().await;
    let (parent_mode, child_mode) = ("auto", "acceptEdits");
    let (task_a, task_b) = ("Feature A: add a.txt saying a.", "Feature B: add b.txt saying b.");
    let bring_in_main = "Bring in main: merge main into your branch, then report again.";
    let keep_b = "Keep b.txt as it is, and say so in REPORT.md.";
    let fake = Path::new(&place.base()).join("app");
    let profile = fake.join("profile");
    let repo = place.repo();
    let report = steps::report();
    let root = place.root();
    let launch = || DelegationLaunch::start(&profile, &fake, &repo, &root, place.git(), parent_mode, true);
    let (a, b, head_a, head_b1, notice_runs, file, follow_ups, run_b1, first_inputs_b) = {
        let s = launch().await;
        let mut steps_a = steps::commit("a.txt", "a\n");
        steps_a.extend([steps::write(&report, "# Report\n\nAdded a.txt.\n"), steps::say("Committed and reported.")]);
        s.child_script(task_a, steps_a);
        let mut steps_b = steps::commit("b.txt", "b\n");
        steps_b.extend([
            steps::write(&report, "# Report\n\nAdded b.txt.\n"),
            steps::permission("Bash", &[("command", "make check")]),
            steps::say("Checked."),
        ]);
        s.child_script(task_b, steps_b);
        // 1. The parent delegates A and B, both from main's one head.
        s.script(
            &repo,
            vec![vec![
                steps::call("delegate", false, &[("task", task_a), ("mode", child_mode)]),
                steps::call("delegate", false, &[("task", task_b), ("mode", child_mode)]),
                steps::say("A and B are on it."),
            ]],
        );
        let delegating = s.human_send("Split features A and B.", PARENT).await;
        assert_eq!(s.ended(PARENT, &delegating).await, "completed", "the delegating run");
        let (a, b) = awaited("both children on disk", || {
            let f = s.file();
            (f.children.len() == 2).then_some(())?;
            let of = |task: &str| {
                f.children
                    .iter()
                    .find(|child| f.copy(&child.id, DelegationCopyKind::Task).is_some_and(|copy| copy.text.contains(task)))
                    .cloned()
            };
            of(task_a).zip(of(task_b))
        })
        .await;
        assert!(
            a.base_commit == start && b.base_commit == start && a.parent_branch == "main" && b.parent_branch == "main",
            "both from main's one head"
        );

        // 2. A commits and reports while the parent is idle after a normal finish: its notice is the parent's next run.
        let ask_a = awaited("A's commit request", || s.open_card(&a.id, "Bash", &[])).await;
        s.script(&repo, vec![vec![steps::say("A reported.")]]);
        s.allow(&ask_a).await;
        let notice_run_a = awaited("A's notice starting a parent run", || s.runs(PARENT).get(1).cloned()).await;
        assert_eq!(s.ended(PARENT, &notice_run_a).await, "completed", "A's notice run");
        let reported = |id: &str, revision: i32| {
            let child = s.file().children.into_iter().find(|child| child.id == id)?;
            (child.state == ChildState::Reported && child.report_revision == revision).then_some(child)
        };
        let head_a = awaited("A's reported head", || reported(&a.id, 1).and_then(|child| child.report_head)).await;

        // 3. B commits and reports, then asks to run its checks, and a human stops it there; its notice is the parent's next run.
        let ask_b = awaited("B's commit request", || s.open_card(&b.id, "Bash", &[])).await;
        s.script(&repo, vec![vec![steps::say("B reported.")]]);
        s.allow(&ask_b).await;
        awaited("B's request to run its checks", || s.open_card(&b.id, "Bash", &[&ask_b.id])).await;
        let run_b1 = s.runs(&b.id).into_iter().next().unwrap();
        assert_eq!(s.runs(&b.id).len(), 1, "B ran once");
        s.service.stop(&b.id).await.unwrap();
        assert_eq!(s.ended(&b.id, &run_b1).await, "stopped", "B's stopped run");
        let notice_run_b = awaited("B's notice starting a parent run", || s.runs(PARENT).get(2).cloned()).await;
        assert_eq!(s.ended(PARENT, &notice_run_b).await, "completed", "B's notice run");
        let head_b1 = awaited("B's reported head", || reported(&b.id, 1).and_then(|child| child.report_head)).await;
        assert!(head_a != head_b1 && head_a != start && head_b1 != start, "A and B each committed");

        // 4. The merge tool fast-forwards main to A's head.
        s.script(&repo, vec![vec![steps::call("merge", false, &[("child", &a.id), ("expected_head", &head_a)]), steps::say("Merged A.")]]);
        let merge_a = s.human_send("Merge A.", PARENT).await;
        assert_eq!(s.ended(PARENT, &merge_a).await, "completed", "the merge run");
        assert_eq!(place.git_in(&repo, &["rev-parse", "refs/heads/main"]).await, head_a, "main is at A's head");

        // 5. B's fast-forward is refused with diverged: main moved on from B's base. Nothing changes.
        let before = s.file();
        let disk = place.snapshot(&[&b.worktree_path]).await;
        s.script(
            &repo,
            vec![vec![steps::call("merge", false, &[("child", &b.id), ("expected_head", &head_b1)]), steps::say("B diverged.")]],
        );
        let merge_b = s.human_send("Merge B.", PARENT).await;
        assert_eq!(s.ended(PARENT, &merge_b).await, "completed", "the refused merge's run");
        assert!(
            s.file() == before
                && place.snapshot(&[&b.worktree_path]).await == disk
                && !Path::new(&repo).join(".git").join("MERGE_HEAD").exists(),
            "the refused merge changed nothing"
        );

        // 6. B's pane is idle after a stop, so both follow-ups are held and B runs nothing.
        s.script(
            &repo,
            vec![vec![
                steps::call("follow_up", false, &[("child", &b.id), ("text", bring_in_main)]),
                steps::call("follow_up", false, &[("child", &b.id), ("text", keep_b)]),
                steps::say("B has its follow-ups."),
            ]],
        );
        let following = s.human_send("B diverged: have it bring in main.", PARENT).await;
        assert_eq!(s.ended(PARENT, &following).await, "completed", "the follow-up run");
        let file = s.file();
        let follow_ups = file.follow_ups.clone();
        assert_eq!(follow_ups.iter().map(|f| f.text.as_str()).collect::<Vec<_>>(), [bring_in_main, keep_b], "two follow-ups");
        let held_b = file.children.iter().find(|child| child.id == b.id).unwrap();
        assert!(
            follow_ups.iter().all(|f| f.child_id == b.id && f.lane == DeliveryLane::Held && f.receipt.is_none())
                && held_b.follow_up_count == 2
                && held_b.state == ChildState::Reported,
            "both held, B still reported"
        );
        assert_eq!(
            s.app.held_items(&b.id).into_iter().map(|item| item.id).collect::<Vec<_>>(),
            follow_ups.iter().map(|f| f.id.clone()).collect::<Vec<_>>(),
            "B's queued list holds them"
        );
        assert_eq!(s.runs(&b.id), std::slice::from_ref(&run_b1), "B ran nothing more");
        assert_eq!(
            s.runs(PARENT),
            [delegating, notice_run_a.clone(), notice_run_b.clone(), merge_a, merge_b, following],
            "the parent's runs"
        );
        let first_inputs_b = s.inputs(&b.id);
        let (a, b) = (
            file.children.iter().find(|child| child.id == a.id).cloned().unwrap(),
            file.children.iter().find(|child| child.id == b.id).cloned().unwrap(),
        );
        s.dispose().await;
        (a, b, head_a, head_b1, vec![notice_run_a, notice_run_b], file, follow_ups, run_b1, first_inputs_b)
    };
    let _ = run_b1;
    assert!(DelegationFileStore::new(&profile).load().unwrap() == file, "quitting changed nothing in the profile");

    // The relaunch: the coordinator finds no run the quit killed, starts no run by itself and still holds both follow-ups.
    let s = launch().await;
    assert!(s.coordinator().interrupt_runs_killed_by_quit().await.is_empty(), "no run was killed by the quit");
    s.coordinator().deliver_pending().await;
    assert!(
        s.runs(PARENT).is_empty() && s.runs(&b.id).is_empty() && s.file() == file && s.pane(&b.id).status() == "stopped",
        "nothing ran at launch; B came back stopped"
    );
    let follow_up_ids: Vec<String> = follow_ups.iter().map(|f| f.id.clone()).collect();
    assert_eq!(s.app.held_items(&b.id).into_iter().map(|item| item.id).collect::<Vec<_>>(), follow_up_ids, "both still held for B");
    assert!(s.app.held_items(PARENT).is_empty(), "nothing held for the parent");

    // 7. The human's next send in B releases both, oldest first, ahead of the text, in one run.
    s.child_script(
        bring_in_main,
        vec![
            steps::run(false, &[&["git", "merge", "--no-edit", "-q", "main"]]),
            steps::write(&report, "# Report\n\nAdded b.txt and brought in main; b.txt is as it was.\n"),
            steps::say("Brought in main and reported again."),
        ],
    );
    let report_file = report_file_in(&b.worktree_path);
    let held_texts: Vec<String> = follow_ups.iter().map(|f| follow_up_text(f, &report_file, "claude")).collect();
    let DelegationRelease::Released { run_id: run_b2, item_ids } = s.app.send(s.request("Go ahead.", &b.id)).await.unwrap() else {
        panic!("B's send released nothing")
    };
    assert_eq!(item_ids, follow_up_ids, "the send released both");
    assert_eq!(s.input(&b.id, &run_b2).await, Some(format!("{}\n\nGo ahead.", held_texts.join("\n\n"))), "oldest first, ahead of the text");
    assert_eq!(s.ended(&b.id, &run_b2).await, "completed", "B's run");
    let revised = awaited("B's revision 2", || {
        let child = s.file().children.into_iter().find(|child| child.id == b.id)?;
        (child.state == ChildState::Reported && child.report_revision == 2).then_some(child)
    })
    .await;
    let head_b2 = revised.report_head.clone().unwrap();
    assert_eq!(place.git_in(&b.worktree_path, &["rev-parse", "HEAD"]).await, head_b2, "B's head is its revision 2");
    assert_eq!(
        place.git_in(&repo, &["rev-list", "--parents", "-n", "1", &head_b2]).await,
        format!("{head_b2} {head_b1} {head_a}"),
        "B's own commit brought main in"
    );
    assert!(
        s.file().follow_ups.iter().all(|f| f.lane == DeliveryLane::Delivered
            && f.receipt.as_ref().is_some_and(|receipt| receipt.route == DeliveryRoute::Queue && receipt.run_id == run_b2)),
        "each follow-up saved delivered by that one run"
    );
    s.coordinator().deliver_pending().await;
    assert!(
        s.coordinator().send("Anything else?", &b.id, None).await == DelegationRelease::NothingHeld
            && s.app.run_next(&b.id).await == DelegationRelease::NothingHeld,
        "nothing more is held for B"
    );
    assert_eq!(s.runs(&b.id), std::slice::from_ref(&run_b2), "B ran once since the relaunch");
    let inputs_b: Vec<String> = first_inputs_b.iter().cloned().chain(s.inputs(&b.id)).collect();
    for f in &follow_ups {
        assert_eq!(inputs_b.iter().filter(|input| input.contains(&f.id)).count(), 1, "each follow-up reached B once");
    }

    // 8. The parent had no run since the relaunch, so B's revision 2 notice is held; the human's send releases it, and the merge tool
    //    fast-forwards main to B's new head.
    let notice2 = awaited("B's revision 2 notice held", || {
        s.file()
            .notices
            .into_iter()
            .find(|notice| notice.child_id == b.id && notice.report_revision == 2 && notice.lane == DeliveryLane::Held)
    })
    .await;
    assert!(notice2.kind == NoticeKind::Reported && s.runs(PARENT).is_empty(), "held, and the parent ran nothing");
    s.script(
        &repo,
        vec![vec![
            steps::call("child_status", false, &[("child", &b.id)]),
            steps::call("merge", false, &[("child", &b.id), ("expected_head", &head_b2)]),
            steps::say("Merged B."),
        ]],
    );
    let text = "B reported again: merge it.";
    let DelegationRelease::Released { run_id: merge_run, item_ids } = s.app.send(s.request(text, PARENT)).await.unwrap() else {
        panic!("the parent's send released nothing")
    };
    assert_eq!(item_ids, std::slice::from_ref(&notice2.id), "the send released the notice");
    assert_eq!(s.input(PARENT, &merge_run).await, Some(format!("{}\n\n{text}", notice_text(&notice2))), "the notice ahead of the text");
    assert_eq!(s.ended(PARENT, &merge_run).await, "completed", "the merge run");
    assert_eq!(place.git_in(&repo, &["rev-parse", "refs/heads/main"]).await, head_b2, "main is at B's revision 2 head");

    // 9. A third follow-up is refused with follow_up_limit and changes nothing.
    let before = s.file();
    s.script(
        &repo,
        vec![vec![
            steps::call("follow_up", false, &[("child", &b.id), ("text", "Also add a changelog line.")]),
            steps::say("B has had its two."),
        ]],
    );
    let third = s.human_send("Ask B for one more change.", PARENT).await;
    assert_eq!(s.ended(PARENT, &third).await, "completed", "the third follow-up's run");
    assert!(s.file() == before && s.runs(&b.id).len() == 1, "the third follow-up changed nothing");
    assert_eq!(s.runs(PARENT), [merge_run.clone(), third], "the parent's runs since the relaunch");
    s.dispose().await;

    // The verdict.
    let saved = s.file();
    let child_a = saved.children.iter().find(|child| child.id == a.id).unwrap();
    let child_b = saved.children.iter().find(|child| child.id == b.id).unwrap();
    assert!(
        saved.children.len() == 2
            && saved.children.iter().all(|child| child.parent_session_id == PARENT
                && child.parent_branch == "main"
                && child.base_commit == start
                && child.starting_mode == child_mode),
        "both from one base, in the asked mode"
    );
    assert!(
        child_a.state == ChildState::Merged
            && child_a.report_revision == 1
            && child_a.follow_up_count == 0
            && child_a.report_head.as_deref() == Some(head_a.as_str())
            && child_b.state == ChildState::Merged
            && child_b.report_revision == 2
            && child_b.follow_up_count == 2
            && child_b.report_head.as_deref() == Some(head_b2.as_str()),
        "A merged at revision 1, B at revision 2"
    );
    assert!(
        saved.copy(&b.id, DelegationCopyKind::Report).is_some_and(|copy| copy.text.contains("brought in main")),
        "B's revision 2 report is kept"
    );
    let merge_of = |id: &str| saved.merges.iter().find(|merge| merge.child_id == id).unwrap();
    assert!(
        saved.merges.len() == 2
            && merge_of(&a.id).kind == MergeKind::ToolFastForward
            && merge_of(&a.id).pre_merge_commit == start
            && merge_of(&a.id).merged_commit == head_a
            && merge_of(&b.id).kind == MergeKind::ToolFastForward
            && merge_of(&b.id).pre_merge_commit == head_a
            && merge_of(&b.id).merged_commit == head_b2
            && merge_of(&b.id).child_head == head_b2,
        "two tool fast-forwards"
    );
    let receipt_of = |notice: &mighty_delegation::model::Notice| notice.receipt.as_ref().map(|receipt| receipt.run_id.clone());
    assert!(
        saved.notices.len() == 3
            && saved.notices.iter().all(|notice| notice.kind == NoticeKind::Reported
                && notice.lane == DeliveryLane::Delivered
                && notice.receipt.as_ref().is_some_and(|receipt| receipt.route == DeliveryRoute::Queue))
            && receipt_of(saved.notices.iter().find(|n| n.child_id == a.id).unwrap()) == Some(notice_runs[0].clone())
            && receipt_of(saved.notices.iter().find(|n| n.child_id == b.id && n.report_revision == 1).unwrap())
                == Some(notice_runs[1].clone())
            && receipt_of(saved.notices.iter().find(|n| n.id == notice2.id).unwrap()) == Some(merge_run.clone()),
        "three notices, each delivered once"
    );
    assert!(
        saved.follow_ups.iter().map(|f| f.id.clone()).collect::<Vec<_>>() == follow_up_ids
            && saved.follow_ups.iter().all(|f| f.receipt.as_ref().is_some_and(|receipt| receipt.run_id == run_b2)),
        "both follow-ups by B's one run"
    );
    assert_eq!(place.git_in(&repo, &["symbolic-ref", "--short", "HEAD"]).await, "main", "main is checked out");
    assert_eq!(place.git_in(&repo, &["rev-list", "--count", "main"]).await, "4", "main reached by fast-forwards only");
    assert!(
        place.git_in(&repo, &["status", "--porcelain", "--untracked-files=no"]).await.is_empty()
            && Path::new(&repo).join("a.txt").exists()
            && Path::new(&repo).join("b.txt").exists(),
        "the parent's checkout has both"
    );
    assert_eq!(place.git_in(&repo, &["rev-parse", &format!("refs/heads/{}", a.branch)]).await, head_a, "A's branch stays");
    assert_eq!(place.git_in(&repo, &["rev-parse", &format!("refs/heads/{}", b.branch)]).await, head_b2, "B's branch stays");
    let parent_log = s.log(&repo);
    let calls = events(&parent_log, "call");
    let merges: Vec<Value> = calls.iter().filter(|call| call["tool"] == "merge").map(|call| steps::structured(call)).collect();
    assert!(
        merges.len() == 3
            && merges[0]["merged"]["mergedCommit"] == head_a.as_str()
            && merges[1]["refused"] == "diverged"
            && merges[1]["merged"].is_null()
            && merges[2]["merged"]["mergedCommit"] == head_b2.as_str()
            && merges[2]["merged"]["preMergeCommit"] == head_a.as_str(),
        "merge A, B refused with diverged, merge B's revision 2"
    );
    let asked: Vec<Value> = calls.iter().filter(|call| call["tool"] == "follow_up").map(|call| steps::structured(call)).collect();
    assert!(
        asked.len() == 3
            && asked[..2].iter().map(|answer| answer["followUp"]["id"].as_str().unwrap_or("").to_string()).collect::<Vec<_>>()
                == follow_up_ids
            && asked[..2].iter().all(|answer| answer["followUp"]["lane"] == "held")
            && asked[..2].iter().map(|answer| answer["followUp"]["remaining"].as_i64()).collect::<Vec<_>>() == [Some(1), Some(0)]
            && asked[2]["refused"] == "follow_up_limit"
            && asked[2]["followUp"].is_null(),
        "two follow-ups held, the third refused"
    );
    let log_b = s.log(&b.worktree_path);
    let argv_b = events(&log_b, "argv");
    assert!(
        argv_b.len() == 2 && argv_b.iter().all(|argv| steps::option(argv, "--permission-mode").as_deref() == Some(child_mode)),
        "B ran twice, in its stored mode"
    );
    assert_eq!(
        events(&log_b, "bash").iter().map(|entry| entry["code"].as_i64()).collect::<Vec<_>>(),
        [Some(0), Some(0)],
        "B's two commands succeeded"
    );
    let parent_prompts = s.prompts(&repo);
    for notice in &saved.notices {
        assert_eq!(
            parent_prompts.iter().filter(|p| p.contains(&notice.id)).count(),
            1,
            "each notice in exactly one prompt the parent received"
        );
    }
    let prompts_b = s.prompts(&b.worktree_path);
    for f in &saved.follow_ups {
        assert_eq!(prompts_b.iter().filter(|p| p.contains(&f.id)).count(), 1, "each follow-up in exactly one prompt B received");
    }
}
