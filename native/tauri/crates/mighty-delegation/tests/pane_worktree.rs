//! Per-pane worktrees (mighty-delegation `pane_worktree`, mighty-core's saved records), C# MightyCode.Core.Tests/PaneWorktreeVerification.cs
//! (the Mac's PaneWorktreeTests and PaneWorktreeToolsTests) check by check: the decision, the PORT, the note, saved records, the copy
//! rules, the list, the pull request URL, and real git for merge, commit-and-merge, undo, status and discard. Git runs only in the
//! temporary repositories of `support::place` (an empty global config, no system config), through the platform's process host
//! (Windows', or M-A's on the Mac). Beyond C#: a 0.4.61-written state with pane worktrees saves back
//! byte for byte (the "data continues" criterion for 0.4.61 users).

mod support;

use std::collections::HashSet;
use std::sync::Arc;

use mighty_core::locale::Locale;
use mighty_core::model::{AppSnapshot, KeptPaneWorktree, PaneWorktree, RemoteWorkspaceReference, RunSession, Workspace};
use mighty_core::shell::{ChildState, DelegationChildRow};
use mighty_delegation::pane_worktree::{
    self as pw, OnDisk, PaneWorktreeDecision, kept_pane_worktrees, normalized, normalized_command, normalized_copy_paths, saved_mode,
};

const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

fn agent(id: &str, workspace: &str, parent: Option<&str>, folder: Option<&str>) -> RunSession {
    RunSession {
        id: Some(id.into()),
        workspace_id: Some(workspace.into()),
        kind: Some("claude".into()),
        provider: Some("claude".into()),
        parent_session_id: parent.map(str::to_string),
        working_folder: folder.map(str::to_string),
        ..RunSession::default()
    }
}

fn pane(worktree: &str, branch: &str, checkout: &str, port: i32) -> PaneWorktree {
    PaneWorktree {
        worktree_path: Some(worktree.into()),
        branch: Some(branch.into()),
        base_branch: Some("main".into()),
        base_commit: Some(COMMIT.into()),
        checkout: Some(checkout.into()),
        port,
        ..PaneWorktree::default()
    }
}

/// "pane worktree: the decision, the PORT, the note and saved records".
#[test]
fn policy_ports_note_and_records() {
    let workspace = Workspace {
        id: Some("w".into()),
        path: Some(std::env::temp_dir().join("w").to_string_lossy().into_owned()),
        ..Workspace::default()
    };
    let none: Vec<RunSession> = vec![];
    let one = vec![agent("a", "w", None, None)];
    assert_eq!(pw::decide(&workspace, &none, None), PaneWorktreeDecision::SameFolder, "the first pane works in the folder");
    assert_eq!(pw::decide(&workspace, &one, None), PaneWorktreeDecision::Ask, "a second pane asks");
    let with = |mode: &str| Workspace { pane_worktree_mode: Some(mode.into()), ..workspace.clone() };
    assert_eq!(pw::decide(&with("further"), &one, None), PaneWorktreeDecision::Worktree, "further opens a worktree");
    assert_eq!(pw::decide(&with("every"), &none, None), PaneWorktreeDecision::Worktree, "every opens the first in a worktree too");
    assert_eq!(pw::decide(&with("odd"), &one, None), PaneWorktreeDecision::Ask, "an unknown mode asks");
    // Children, worktree panes, terminals, other workspaces and closing panes do not count.
    let ignored = vec![
        agent("c", "w", Some("a"), None),
        agent("t", "w", None, Some(r"C:\wt\t")),
        RunSession { id: Some("s".into()), workspace_id: Some("w".into()), kind: Some("shell".into()), ..RunSession::default() },
        agent("o", "other", None, None),
        agent("x", "w", None, None),
    ];
    let closing = HashSet::from(["x".to_string()]);
    assert_eq!(
        pw::decide(&workspace, &ignored, Some(&closing)),
        PaneWorktreeDecision::SameFolder,
        "only open agent panes in the folder count"
    );
    assert_eq!(
        pw::decide_without_asking(&workspace, &one, None),
        PaneWorktreeDecision::SameFolder,
        "the phone cannot be asked: the folder"
    );
    let remote = Workspace {
        remote: Some(RemoteWorkspaceReference {
            connection_id: Some("c".into()),
            workspace_id: Some("w".into()),
            host_name: Some("h".into()),
            version: 2,
            mirror: false,
        }),
        pane_worktree_mode: Some("every".into()),
        ..workspace.clone()
    };
    assert_eq!(pw::decide(&remote, &none, None), PaneWorktreeDecision::SameFolder, "a remote workspace works in its folder");
    // MC-47: an SSH workspace's panes follow its mode (their worktrees are on its machine).
    let ssh = Workspace {
        ssh: Some(mighty_core::model::SshWorkspaceRef {
            machine: "0123456789abcdef0123456789abcdef".into(),
            dir: "/srv/app".into(),
            host: "devbox".into(),
        }),
        pane_worktree_mode: Some("every".into()),
        ..workspace.clone()
    };
    assert_eq!(pw::decide(&ssh, &none, None), PaneWorktreeDecision::Worktree, "an SSH workspace in every mode makes a worktree");
    let ssh_ask = Workspace { pane_worktree_mode: None, ..ssh.clone() };
    assert_eq!(pw::decide(&ssh_ask, &one, None), PaneWorktreeDecision::Ask, "and asks for a second pane");
    let locale = Locale::new("en");
    let children = vec![agent("c", "w", Some("a"), None)];
    assert!(
        pw::discard_blocker("a", &children, &[], &locale).is_some_and(|text| !text.is_empty())
            && pw::discard_blocker("a", &[agent("b", "w", None, None)], &[], &locale).is_none(),
        "children block a discard"
    );
    // Beyond C#'s check: an open delegated child of the pane blocks too, a closed one does not.
    let row = |state: ChildState| DelegationChildRow {
        id: "k".into(),
        parent_session_id: "a".into(),
        state,
        task: None,
        parent_branch: "mighty/a".into(),
        parent_checkout: None,
        can_undo: false,
        held_notices: 0,
        started_by_human: false,
        agent: "claude".into(),
    };
    assert!(pw::discard_blocker("a", &none, &[row(ChildState::Running)], &locale).is_some());
    assert!(pw::discard_blocker("a", &none, &[row(ChildState::Closed)], &locale).is_none());
    // PORT: the Mac's FNV-1a start, then the next free one.
    let empty = HashSet::new();
    assert_eq!(pw::port("pane-a", &empty, |_| true), 4141, "the Mac's port for pane-a");
    assert_eq!(pw::port("11111111-2222-3333-4444-555555555555", &empty, |_| true), 4189, "the Mac's port for a uuid");
    assert_eq!(pw::port("pane-a", &HashSet::from([4141]), |_| true), 4142, "a taken port is skipped");
    assert_eq!(pw::port("pane-a", &empty, |port| port != 4141), 4142, "a bound port is skipped");
    // The note: once, never on a slash command, with the branch, base and PORT.
    let pane = pane(r"C:\wt\p", "mighty/p", r"C:\repo", 4141);
    assert!(
        pane.note_applies("fix it")
            && !pane.note_applies(" /compact")
            && !PaneWorktree { note_given: Some(true), ..pane.clone() }.note_applies("fix it"),
        "the note applies once, never to a slash command"
    );
    let note = pane.with_note("fix it");
    assert!(
        note.starts_with(
            "fix it\n\n[Mighty Code] This pane works in its own git worktree (C:\\wt\\p) on the branch mighty/p, made from main"
        ) && note.contains("PORT=4141")
            && note.contains("Do not merge, push, rebase or switch branches."),
        "the Mac's note"
    );
    assert!(pane.conflict_prompt().contains("`git merge main`"), "the conflict request names the merge");
    let env = pane.environment(r"C:\repo");
    assert!(
        env["PORT"] == "4141"
            && env["MIGHTY_WORKSPACE_FOLDER"] == r"C:\repo"
            && env["MIGHTY_WORKTREE"] == r"C:\wt\p"
            && env.keys().all(|key| mighty_run::model::PANE_ENVIRONMENT_KEYS.contains(&key.as_str())),
        "the run environment"
    );
    // Saved records.
    if cfg!(windows) {
        assert!(
            normalized(&pane, "p", false).is_some()
                && normalized(&pane, "q", false).is_none()
                && normalized(&PaneWorktree { port: 80, ..pane.clone() }, "p", false).is_none()
                && normalized(&PaneWorktree { checkout: Some("relative".into()), ..pane.clone() }, "p", false).is_none(),
            "only the app's own records load"
        );
        let running =
            PaneWorktree { setup_command: Some("npm ci".into()), setup_status: Some(PaneWorktree::RUNNING.into()), ..pane.clone() };
        assert_eq!(
            normalized(&running, "p", true).unwrap().setup_status.as_deref(),
            Some(PaneWorktree::FAILED),
            "a setup cut off by the quit reads failed"
        );
        let bad = normalized(
            &PaneWorktree { merged_commit: Some("x".into()), pre_merge_commit: Some(COMMIT.into()), ..pane.clone() },
            "p",
            false,
        )
        .unwrap();
        assert!(bad.merged_commit.is_none() && bad.pre_merge_commit.is_none(), "a bad merge record is dropped");
        let elsewhere = normalized(&PaneWorktree { working_folder: Some(r"C:\elsewhere".into()), ..pane.clone() }, "p", false).unwrap();
        let inside = normalized(&PaneWorktree { working_folder: Some(r"C:\wt\p\sub".into()), ..pane.clone() }, "p", false).unwrap();
        assert!(
            elsewhere.working_folder.is_none() && inside.working_folder.as_deref() == Some(r"C:\wt\p\sub"),
            "the run folder lies in the worktree"
        );
        let record = |id: &str, title: &str| KeptPaneWorktree {
            session_id: Some(id.into()),
            title: Some(title.into()),
            worktree: Some(pane.clone()),
            ..KeptPaneWorktree::default()
        };
        let kept = kept_pane_worktrees(Some(&vec![record("p", "t"), record("p", ""), record("bad id!", "")]), false);
        assert!(kept.is_some_and(|kept| kept.len() == 1), "one kept record per session, valid ones only");
    }
    assert_eq!(saved_mode(Some("ask")), None, "ask is saved as nothing");
    assert_eq!(normalized_command(Some("  npm ci\r\nnpm test ")).as_deref(), Some("npm ci  npm test"), "one line");
    assert!(normalized_command(Some("   ")).is_none(), "empty is none");
}

/// "pane worktree: the copy rules, the list and the pull request page".
#[test]
fn copy_list_and_pull_request() {
    let raw = ["./.env", ".env", "../x", "/abs", "~/h", "a/.git/b", "dir/", "C:\\x", "a\\b"];
    assert_eq!(
        normalized_copy_paths(raw.iter().map(|p| Some(*p))).unwrap_or_default(),
        vec![".env", "dir", "a/b"],
        "relative paths inside only"
    );
    let entries: Vec<String> = pw::copy_entries(".env, .env.local\nconfig/").iter().map(|e| e.trim().to_string()).collect();
    assert_eq!(entries, vec![".env", ".env.local", "config/"], "commas and lines");
    let base = support::place::temp("pane-copy");
    {
        let source = base.join("ws");
        let target = base.join("wt");
        std::fs::create_dir_all(source.join("conf").join("deep")).unwrap();
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(source.join(".env"), "A=1").unwrap();
        std::fs::write(source.join("conf").join("deep").join("x.txt"), "x").unwrap();
        std::fs::write(source.join("exists.txt"), "new").unwrap();
        std::fs::write(target.join("exists.txt"), "old").unwrap();
        let wanted: Vec<String> = [".env", "conf", "exists.txt", "missing.txt"].iter().map(|s| s.to_string()).collect();
        let result = pw::copy(&wanted, &source.to_string_lossy(), &target.to_string_lossy(), &support::place::paths());
        assert_eq!(result.copied, vec![".env", "conf"], "files and folders are copied");
        assert!(
            std::fs::read_to_string(target.join("conf").join("deep").join("x.txt")).unwrap() == "x"
                && std::fs::read_to_string(target.join("exists.txt")).unwrap() == "old",
            "nothing is overwritten"
        );
        assert!(result.why("exists.txt") == Some("exists") && result.why("missing.txt") == Some("missing"), "skips say why");
    }
    support::place::remove_folder(&base);
    // The list read from git.
    let root = if cfg!(windows) { r"C:\Users\me\.mightycode\worktrees" } else { "/home/me/.mightycode/worktrees" };
    let repo = if cfg!(windows) { "C:/repo" } else { "/repo" };
    let slashed = root.replace('\\', "/");
    let porcelain = format!(
        "worktree {repo}\nHEAD {COMMIT}\nbranch refs/heads/main\n\nworktree {slashed}/pane-1\nHEAD {COMMIT}\nbranch refs/heads/mighty/pane-1\n\nworktree {slashed}/other\nHEAD {COMMIT}\nbranch refs/heads/feature\n\n"
    );
    let found = pw::parse(&porcelain, root, &support::place::paths());
    assert!(
        found.len() == 1 && found[0].session_id == "pane-1" && found[0].branch == "mighty/pane-1",
        "only <root>/<id> on mighty/<id>: {found:?}"
    );
    let join = |name: &str| mighty_run::os::combine(root, name);
    let open = RunSession {
        id: Some("open-1".into()),
        workspace_id: Some("w".into()),
        kind: Some("claude".into()),
        title: Some("Open".into()),
        pane_worktree: Some(PaneWorktree { base_commit: None, ..pane(&join("open-1"), "mighty/open-1", root, 4100) }),
        ..RunSession::default()
    };
    let kept = KeptPaneWorktree {
        session_id: Some("kept-1".into()),
        title: Some("Kept".into()),
        worktree: Some(pane(&join("kept-1"), "mighty/kept-1", root, 4101)),
        ..KeptPaneWorktree::default()
    };
    let mut on_disk = found.clone();
    on_disk.push(OnDisk {
        session_id: "child-1".into(),
        worktree_path: join("child-1"),
        branch: "mighty/child-1".into(),
        head: COMMIT.into(),
    });
    let rows = pw::rows([&open], &[kept], &on_disk, &HashSet::from(["child-1".to_string()]), root, Some("main"), |_| 4200);
    let listed: Vec<String> = rows.iter().map(|r| format!("{}:{}", r.session_id, r.origin)).collect();
    assert_eq!(listed, vec!["open-1:open", "kept-1:kept", "pane-1:found"], "open, kept, then found; children left out");
    // The pull request page.
    for remote in ["https://github.com/me/app.git", "git@github.com:me/app.git", "ssh://git@github.com/me/app"] {
        assert_eq!(pw::github_repository(remote).as_deref(), Some("me/app"), "a GitHub remote: {remote}");
    }
    assert!(
        pw::github_repository("https://gitlab.com/me/app.git").is_none()
            && pw::github_repository("https://github.com/me/app/extra").is_none(),
        "other hosts and paths are none"
    );
    assert_eq!(
        pw::compare_url("me/app", "main", "mighty/p"),
        "https://github.com/me/app/compare/main...mighty/p?expand=1",
        "the compare page"
    );
}

/// Beyond C# (the "data continues" criterion): a state 0.4.61 wrote with a pane worktree, a kept one and the workspace's settings
/// loads, normalizes and saves back byte for byte; a remote workspace's settings and a child's or terminal's record are dropped.
#[test]
fn a_state_0_4_61_wrote_with_pane_worktrees_saves_back_byte_for_byte() {
    use mighty_core::json::{JsonOut, Wire};
    let root = if cfg!(windows) { r"C:\\Users\\me\\.mightycode\\worktrees" } else { "/Users/me/.mightycode/worktrees" };
    let repo = if cfg!(windows) { r"C:\\repo" } else { "/repo" };
    let sep = if cfg!(windows) { r"\\" } else { "/" };
    let worktree = |id: &str, port: i32, extra: &str| {
        format!(
            r#"{{"worktreePath":"{root}{sep}{id}","branch":"mighty/{id}","baseBranch":"main","baseCommit":"{COMMIT}","checkout":"{repo}","port":{port}{extra}}}"#
        )
    };
    let open = worktree(
        "p1",
        4141,
        &format!(
            r#","setupCommand":"npm ci","setupStatus":"done","setupExitCode":0,"mergedCommit":"{COMMIT}","preMergeCommit":"{COMMIT}","workingFolder":"{root}{sep}p1","noteGiven":true"#
        ),
    );
    let kept = worktree("p2", 4142, "");
    // Times as Wire.Json writes them (its encoder writes + as +).
    let plus = format!("{}u002B", '\\');
    let (made, at) = (format!("2026-11-01T00:00:00.0000000{plus}09:00"), format!("2026-11-02T00:00:00.0000000{plus}09:00"));
    let text = format!(
        r#"{{"workspaces":[{{"id":"w","name":"App","path":"{repo}","createdAt":"{made}","paneWorktreeMode":"further","worktreeSetupCommand":"npm ci","worktreeCopyPaths":[".env","config/local.json"],"keptPaneWorktrees":[{{"sessionId":"p2","title":"Kept","provider":"codex","worktree":{kept},"keptAt":"{at}"}}]}}],"sessions":[{{"id":"p1","workspaceId":"w","title":"Claude","kind":"claude","status":"idle","model":"default","provider":"claude","logs":[],"createdAt":"{made}","draft":"","workingFolder":"{root}{sep}p1","paneWorktree":{open}}}]}}"#
    );
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    let read = AppSnapshot::read(&value).unwrap();
    let saved = mighty_core::normalize::normalize(&read, true).unwrap();
    let session = saved.session("p1").unwrap();
    let mut out = JsonOut::new();
    session.pane_worktree.as_ref().unwrap().write(&mut out);
    assert_eq!(out.finish(), open, "the pane's record saves back as 0.4.61 wrote it");
    let mut out = JsonOut::new();
    saved.workspace("w").unwrap().write(&mut out);
    let workspace = out.finish();
    assert!(
        workspace.ends_with(&format!(
            r#""paneWorktreeMode":"further","worktreeSetupCommand":"npm ci","worktreeCopyPaths":[".env","config/local.json"],"keptPaneWorktrees":[{{"sessionId":"p2","title":"Kept","provider":"codex","worktree":{kept},"keptAt":"{at}"}}]}}"#
        )),
        "the workspace's settings save back as written: {workspace}"
    );
    // A remote workspace drops them; a terminal keeps its folder but never a record; a child never has one.
    let mut changed = saved.clone();
    let remote = RemoteWorkspaceReference {
        connection_id: Some("c".into()),
        workspace_id: Some("h".into()),
        host_name: Some("Host".into()),
        version: 2,
        mirror: false,
    };
    changed.workspaces_mut()[0].remote = Some(remote);
    let record = session.pane_worktree.clone();
    changed.sessions_mut().push(Arc::new(RunSession {
        id: Some("t1".into()),
        workspace_id: Some("w".into()),
        kind: Some("shell".into()),
        working_folder: Some(format!("{}{}p1", root.replace(r"\\", r"\"), sep.replace(r"\\", r"\"))),
        pane_worktree: record.clone(),
        ..RunSession::default()
    }));
    let again = mighty_core::normalize::normalize(&changed, false).unwrap();
    let w = again.workspace("w").unwrap();
    assert!(w.pane_worktree_mode.is_none() && w.worktree_copy_paths.is_none() && w.kept_pane_worktrees.is_none());
    assert!(again.session("p1").unwrap().pane_worktree.is_none(), "a remote workspace's pane has no worktree");
    let terminal = again.session("t1").unwrap();
    assert!(terminal.working_folder.is_some() && terminal.pane_worktree.is_none(), "a terminal keeps its folder, never a record");
}

mod git {
    use mighty_delegation::git::cleanup::ChildDiscardOutcome;
    use mighty_delegation::git::worktree::{ChildWorktreeCheck, ChildWorktreeOutcome, NOTES_FOLDER};
    use mighty_delegation::pane_worktree::{self as pw, CommitOutcome, MergeOutcome, UndoOutcome, refusal};

    use super::*;
    use crate::support::lane;
    use crate::support::place::DelegationPlace;

    /// C# `Pane(id)`: a repository with one commit and the pane worktree `id` made in it the app's way.
    async fn pane_in(place: &DelegationPlace, id: &str) -> PaneWorktree {
        place.repository_default().await;
        let maker = place.maker();
        let ChildWorktreeCheck::Ready(start) = maker.check(&place.repo()).await else { panic!("the repository cannot host a worktree") };
        let ChildWorktreeOutcome::Created(made) = pw::make_pane(&crate::support::place::panes(&maker), id, &start, 300.0).await else {
            panic!("no pane worktree")
        };
        assert!(!std::path::Path::new(&made.worktree_path).join(NOTES_FOLDER).exists(), "a pane worktree has no TASK and REPORT");
        pw::from_made(&made, &start.repository, 4141, None)
    }

    /// "pane worktree: merge, commit and merge, undo, status and discard with real git".
    #[tokio::test]
    async fn git_merge_commit_undo_status_and_discard() {
        let _lane = lane::enter().await;
        let place = DelegationPlace::new();
        let (git, locale) = (place.git(), Locale::new("en"));
        let pane = pane_in(&place, "pane-a").await;
        let worktree = std::path::PathBuf::from(pane.worktree_path());
        let repo = std::path::PathBuf::from(place.repo());
        assert_eq!(
            pw::merge(&place.machine(), &pane, &locale).await,
            MergeOutcome::Refused(refusal::NOTHING_TO_MERGE),
            "nothing to merge yet"
        );
        // A new file blocks the merge; commit-and-merge commits it, leaving a secret-looking one out.
        std::fs::write(worktree.join("feature.txt"), "feature\n").unwrap();
        std::fs::write(worktree.join("id_rsa"), "secret\n").unwrap();
        assert_eq!(
            pw::merge(&place.machine(), &pane, &locale).await,
            MergeOutcome::Refused(refusal::UNCOMMITTED_CHANGES),
            "an untracked file blocks the merge"
        );
        assert_eq!(
            pw::status(&place.machine(), &pane).await,
            Some(pw::Status { ahead: 0, behind: 0, changed_files: 2 }),
            "two changed files"
        );
        let committed = pw::commit_all(&place.machine(), &pane, &locale).await;
        assert_eq!(committed, CommitOutcome::Committed { left_out: vec!["id_rsa".into()] }, "the secret-looking file is left out");
        assert_eq!(
            pw::merge(&place.machine(), &pane, &locale).await,
            MergeOutcome::Refused(refusal::UNCOMMITTED_CHANGES),
            "the left-out file still blocks"
        );
        std::fs::remove_file(worktree.join("id_rsa")).unwrap();
        let files = pw::changed_files(&git, &pane).await.unwrap();
        assert!(
            files.len() == 1 && files[0].path == "feature.txt" && files[0].status == "A",
            "the changed files against the base: {files:?}"
        );
        assert!(
            pw::file_diff(&place.machine(), &pane, "feature.txt").await.is_some_and(|diff| diff.contains("+feature")),
            "the file's diff"
        );
        let before = place.fixture_git(&["rev-parse", "HEAD"]).await;
        let MergeOutcome::Merged { commit, pre_merge } = pw::merge(&place.machine(), &pane, &locale).await else { panic!("not merged") };
        assert!(
            pre_merge == before && place.fixture_git(&["rev-parse", "HEAD"]).await == commit && repo.join("feature.txt").exists(),
            "a fast-forward into main"
        );
        // Undo puts main back and keeps the pane's commits.
        let recorded = PaneWorktree { merged_commit: Some(commit.clone()), pre_merge_commit: Some(pre_merge.clone()), ..pane.clone() };
        assert!(
            pw::undo_merge(&place.machine(), &recorded, &locale).await == UndoOutcome::Undone
                && place.fixture_git(&["rev-parse", "HEAD"]).await == before
                && !repo.join("feature.txt").exists(),
            "the merge is undone"
        );
        assert_eq!(
            pw::undo_merge(&place.machine(), &recorded, &locale).await,
            UndoOutcome::Refused("baseMoved"),
            "a second undo finds the base moved"
        );
        assert_eq!(pw::undo_merge(&place.machine(), &pane, &locale).await, UndoOutcome::Refused("notMerged"), "no merge on record");
        // The base moves: one merge commit; a conflict is refused before any write.
        std::fs::write(repo.join("base.txt"), "base\n").unwrap();
        place.fixture_git(&["add", "-A"]).await;
        place.fixture_git(&["commit", "-q", "-m", "base"]).await;
        let status = pw::status(&place.machine(), &pane).await.unwrap();
        assert!(status.ahead == 1 && status.behind == 1, "ahead and behind: {status:?}");
        let MergeOutcome::Merged { commit: second, .. } = pw::merge(&place.machine(), &pane, &locale).await else {
            panic!("no merge commit")
        };
        assert_eq!(
            place.fixture_git(&["rev-list", "--parents", "-n", "1", &second]).await.split(' ').count(),
            3,
            "a merge commit with two parents"
        );
        assert_eq!(
            place.fixture_git(&["log", "-1", "--format=%s", &second]).await,
            "Merge branch 'mighty/pane-a' into main",
            "the merge commit's message is C#'s"
        );
        std::fs::write(repo.join("file-0.txt"), "main side\n").unwrap();
        place.fixture_git(&["commit", "-q", "-am", "main side"]).await;
        std::fs::write(worktree.join("file-0.txt"), "pane side\n").unwrap();
        place.git_in(pane.worktree_path(), &["commit", "-q", "-am", "pane side"]).await;
        let head = place.fixture_git(&["rev-parse", "HEAD"]).await;
        assert!(
            pw::merge(&place.machine(), &pane, &locale).await == MergeOutcome::Refused(refusal::MERGE_CONFLICT)
                && place.fixture_git(&["rev-parse", "HEAD"]).await == head,
            "a conflict changes nothing"
        );
        // Tracked changes in the workspace folder refuse; so does another branch checked out.
        std::fs::write(repo.join("file-0.txt"), "dirty\n").unwrap();
        assert!(
            matches!(
                pw::merge(&place.machine(), &pane, &locale).await,
                MergeOutcome::Refused(refusal::TRACKED_CHANGES | refusal::MERGE_CONFLICT)
            ),
            "a dirty workspace folder refuses"
        );
        place.fixture_git(&["checkout", "-q", "--", "file-0.txt"]).await;
        place.fixture_git(&["checkout", "-q", "-b", "other"]).await;
        assert_eq!(
            pw::merge(&place.machine(), &pane, &locale).await,
            MergeOutcome::Refused(refusal::BRANCH_NOT_CHECKED_OUT),
            "another branch checked out refuses"
        );
        place.fixture_git(&["checkout", "-q", "main"]).await;
        // The list finds the worktree; unmerged work keeps it; discard removes it and its branch.
        let listed = pw::worktrees(&place.machine(), &place.repo(), &place.root()).await.unwrap();
        assert!(listed.len() == 1 && listed[0].session_id == "pane-a", "git lists the pane worktree: {listed:?}");
        assert_eq!(pw::has_unmerged_work(&place.machine(), &pane).await, Some(true), "unmerged commits are work");
        let discarded = pw::discard(&place.machine(), &pane, "pane-a", &place.root(), &locale).await;
        assert!(
            discarded == ChildDiscardOutcome::Discarded && !worktree.exists() && place.mighty_branches().await.is_empty(),
            "discard removes the worktree and its branch: {discarded:?}"
        );
    }

    /// Beyond C#: the commit-and-merge commit is C#'s (its message, unsigned, no hooks), the untracked file's diff is all added lines,
    /// a path out of the worktree has no diff, and a pane with nothing of its own has no unmerged work.
    #[tokio::test]
    async fn the_commit_the_diffs_and_the_work_left_are_csharps() {
        let _lane = lane::enter().await;
        let place = DelegationPlace::new();
        let (git, locale) = (place.git(), Locale::new("en"));
        let pane = pane_in(&place, "pane-b").await;
        let worktree = std::path::PathBuf::from(pane.worktree_path());
        assert_eq!(pw::has_unmerged_work(&place.machine(), &pane).await, Some(false), "a fresh pane has no work");
        std::fs::write(worktree.join("new.txt"), "one\r\ntwo").unwrap();
        assert_eq!(
            pw::file_diff(&place.machine(), &pane, "new.txt").await.as_deref(),
            Some("+++ b/new.txt\n+one\n+two"),
            "an untracked file is all added"
        );
        for outside in ["../x", "", "/abs", "a/../../b"] {
            assert!(pw::file_diff(&place.machine(), &pane, outside).await.is_none(), "no diff of {outside:?}");
        }
        assert_eq!(pw::has_unmerged_work(&place.machine(), &pane).await, Some(true), "a new file is work");
        assert_eq!(pw::commit_all(&place.machine(), &pane, &locale).await, CommitOutcome::Committed { left_out: vec![] });
        assert_eq!(place.git_in(pane.worktree_path(), &["log", "-1", "--format=%s"]).await, "mighty: commit mighty/pane-b for its merge");
        assert_eq!(
            pw::commit_all(&place.machine(), &pane, &locale).await,
            CommitOutcome::Nothing { left_out: vec![] },
            "nothing more to commit"
        );
        let files = pw::changed_files(&git, &pane).await.unwrap();
        assert!(files.len() == 1 && files[0].path == "new.txt" && files[0].status == "A");
        assert!(pw::origin(&git, &pane).await.is_none(), "no origin");
    }

    /// Removes a directory link itself: a junction is a folder to Windows, a symbolic link a file elsewhere.
    fn unlink(link: &std::path::Path) {
        if cfg!(windows) { std::fs::remove_dir(link) } else { std::fs::remove_file(link) }.unwrap();
    }

    /// A directory link elsewhere than Windows: a symbolic link.
    #[cfg(not(windows))]
    async fn junction(link: &std::path::Path, target: &std::path::Path) {
        std::os::unix::fs::symlink(target, link).unwrap();
    }

    /// `mklink /J link target`, through cmd.exe and the process host.
    #[cfg(windows)]
    async fn junction(link: &std::path::Path, target: &std::path::Path) {
        let comspec = std::env::var("ComSpec").unwrap_or_else(|_| r"C:\Windows\System32\cmd.exe".into());
        let (link, target) = (link.to_string_lossy().into_owned(), target.to_string_lossy().into_owned());
        let folder = std::path::Path::new(&link).parent().unwrap().to_path_buf();
        let request = mighty_platform::SpawnRequest::new(comspec, ["/d", "/c", "mklink", "/J", &link, &target], folder);
        let processes = crate::support::place::processes();
        let made = mighty_platform::capture(processes.as_ref(), request, None, std::time::Duration::from_secs(30), 1 << 20).await.unwrap();
        assert!(made.exit_code == 0 && std::fs::read_link(&link).is_ok(), "the junction was made: {link}");
    }

    /// Beyond C# (review M3, L11): a closed pane's worktree is kept while it holds work, children branch from it, git cannot tell, or
    /// its own HEAD holds commits on no branch; only a clean one goes.
    #[tokio::test]
    async fn a_closed_pane_keeps_its_worktree_with_work_children_or_an_unknown_answer() {
        let _lane = lane::enter().await;
        let place = DelegationPlace::new();
        let pane = pane_in(&place, "pane-c").await;
        let worktree = std::path::PathBuf::from(pane.worktree_path());
        assert!(!pw::kept_on_close(&place.machine(), &pane, false).await, "a clean pane's worktree goes");
        assert!(pw::kept_on_close(&place.machine(), &pane, true).await, "children keep it");
        std::fs::write(worktree.join("new.txt"), "work\n").unwrap();
        assert!(pw::kept_on_close(&place.machine(), &pane, false).await, "a new file is work");
        std::fs::remove_file(worktree.join("new.txt")).unwrap();
        let gone = PaneWorktree { checkout: Some(place.join("no-such-checkout")), ..pane.clone() };
        assert!(pw::kept_on_close(&place.machine(), &gone, false).await, "an answer git cannot give keeps it");
        // A commit on a detached HEAD is on no branch: the branch alone says nothing is unmerged, the worktree's HEAD says otherwise.
        place.git_in(pane.worktree_path(), &["checkout", "-q", "--detach"]).await;
        place.commit(pane.worktree_path(), "detached.txt", "detached\n").await;
        assert!(pw::kept_on_close(&place.machine(), &pane, false).await, "a commit on a detached HEAD is work");
    }

    /// Beyond C# (review M3, L1-L3): the copy never takes a link or what one leads to, nor writes through a link in the worktree; a
    /// file's diff never reads through one; a worktree that is not its own place, or is a link, is not one the app writes in.
    #[tokio::test]
    async fn links_never_lead_a_copy_a_diff_or_a_write_out_of_place() {
        let _lane = lane::enter().await;
        let place = DelegationPlace::new();
        let paths = place.paths();
        let pane = pane_in(&place, "pane-d").await;
        let base = std::path::PathBuf::from(place.base());
        let (source, outside) = (base.join("ws"), base.join("outside"));
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.env"), "S=1\n").unwrap();
        junction(&source.join("linked"), &outside).await;
        let wanted: Vec<String> = ["linked", "linked/secret.env"].iter().map(|s| s.to_string()).collect();
        let destination = base.join("dest");
        std::fs::create_dir_all(&destination).unwrap();
        let copied = pw::copy(&wanted, &source.to_string_lossy(), &destination.to_string_lossy(), &paths);
        assert!(copied.copied.is_empty(), "nothing a link leads to is copied: {copied:?}");
        assert_eq!(copied.why("linked"), Some("link"), "the link itself");
        assert_eq!(copied.why("linked/secret.env"), Some("outside"), "a file through a linked folder");
        // A folder of the worktree that is a link out of it is never written through — neither nested file subdirs nor empty dirs.
        std::fs::create_dir_all(source.join("conf").join("nested")).unwrap();
        std::fs::write(source.join("conf").join("nested").join("x.txt"), "x").unwrap();
        std::fs::create_dir_all(source.join("conf").join("empty_sub")).unwrap();
        let elsewhere = base.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        junction(&destination.join("conf"), &elsewhere).await;
        let through = pw::copy(
            &["conf/nested/x.txt".to_string(), "conf/empty_sub".to_string()],
            &source.to_string_lossy(),
            &destination.to_string_lossy(),
            &paths,
        );
        assert!(
            through.copied.is_empty() && !elsewhere.join("nested").exists() && !elsewhere.join("empty_sub").exists(),
            "neither nested directories nor empty directories may be created through a symlinked ancestor: {through:?}"
        );
        // A file's diff never reads through a link in the worktree.
        let worktree = std::path::PathBuf::from(pane.worktree_path());
        junction(&worktree.join("lnk"), &outside).await;
        assert!(pw::file_diff(&place.machine(), &pane, "lnk/secret.env").await.is_none(), "no diff through a link");
        // Its own place only, never a link.
        assert!(pw::in_own_place(&place.machine(), &pane, &place.root()).await, "the pane's worktree is its own place");
        let misplaced = PaneWorktree { worktree_path: Some(place.repo()), ..pane.clone() };
        assert!(!pw::in_own_place(&place.machine(), &misplaced, &place.root()).await, "a worktree elsewhere is not");
        let linked_place = std::path::PathBuf::from(place.root()).join("pane-e");
        junction(&linked_place, &outside).await;
        let linked = PaneWorktree {
            worktree_path: Some(linked_place.to_string_lossy().into_owned()),
            branch: Some("mighty/pane-e".into()),
            ..pane.clone()
        };
        assert!(!pw::in_own_place(&place.machine(), &linked, &place.root()).await, "a link at the place is not");
        unlink(&linked_place);
        unlink(&worktree.join("lnk"));
    }

    /// MC-28: a fork's worktree starts at the source's HEAD and its uncommitted changes come along (tracked ones through `git stash
    /// create` and `stash apply`, new ones copied, gitignored ones never), the source left exactly as it was.
    #[tokio::test]
    async fn a_fork_carries_the_sources_uncommitted_changes_without_touching_it() {
        let _lane = lane::enter().await;
        let place = DelegationPlace::new();
        let git = place.git();
        let source = pane_in(&place, "pane-src").await;
        let src = std::path::PathBuf::from(source.worktree_path());
        // A commit of the source's own, then work not committed: a change, a new file, an ignored file.
        place.commit(source.worktree_path(), "committed.txt", "committed\n").await;
        std::fs::write(src.join(".gitignore"), "secret.env\n").unwrap();
        place.git_in(source.worktree_path(), &["add", ".gitignore"]).await;
        place.git_in(source.worktree_path(), &["commit", "-q", "-m", "ignore"]).await;
        std::fs::write(src.join("file-0.txt"), "changed in the source\n").unwrap();
        std::fs::create_dir_all(src.join("new")).unwrap();
        std::fs::write(src.join("new/added.txt"), "new file\n").unwrap();
        std::fs::write(src.join("secret.env"), "TOKEN=1\n").unwrap();
        let status_before = place.git_in(source.worktree_path(), &["status", "--porcelain"]).await;
        let stash_before = place.git_in(source.worktree_path(), &["stash", "list"]).await;

        let maker = place.maker();
        let ChildWorktreeCheck::Ready(read) = maker.check(source.worktree_path()).await else { panic!("the source cannot be read") };
        let start = pw::fork_start(&read, Some(&source));
        assert_eq!(start.repository, source.checkout(), "the fork works in the source's checkout");
        assert_eq!(start.parent_branch, "main", "and merges where the source does");
        assert_eq!(start.base_commit, place.git_in(source.worktree_path(), &["rev-parse", "HEAD"]).await, "at the source's HEAD");
        let id = pw::fork_session_id("ABCDEF12-3456-4789-8abc-def012345678");
        assert_eq!(id, "fork-abcdef12");
        let ChildWorktreeOutcome::Created(made) = pw::make_pane(&crate::support::place::panes(&maker), &id, &start, 300.0).await else {
            panic!("no fork worktree")
        };
        assert_eq!(made.branch, "mighty/fork-abcdef12");
        let fork = std::path::PathBuf::from(&made.worktree_path);
        assert!(fork.join("committed.txt").exists(), "the source's commits are there");

        let carried = pw::carry_changes(&git, source.worktree_path(), &made.worktree_path).await;
        assert_eq!(carried, pw::CarryOutcome::Carried { tracked: true, copied: vec!["new/added.txt".into()] });
        assert_eq!(std::fs::read_to_string(fork.join("file-0.txt")).unwrap(), "changed in the source\n");
        assert_eq!(std::fs::read_to_string(fork.join("new/added.txt")).unwrap(), "new file\n");
        assert!(!fork.join("secret.env").exists(), "a gitignored file never comes along");
        assert_eq!(place.git_in(source.worktree_path(), &["status", "--porcelain"]).await, status_before, "the source is untouched");
        assert_eq!(place.git_in(source.worktree_path(), &["stash", "list"]).await, stash_before, "and so is its stash list");
        let mut paths = pw::uncommitted_paths(&git, source.worktree_path()).await;
        paths.sort();
        assert_eq!(paths, ["file-0.txt", "new/added.txt"], "the summary's list leaves the ignored file out");
        let clean = pane_in_other(&place, &maker, &start, "fork-0000clean").await;
        assert_eq!(pw::carry_changes(&git, &clean, &clean).await, pw::CarryOutcome::Nothing, "nothing to carry from a clean checkout");
    }

    /// MC-28: a fork made at another commit than the changes were made on refuses them, says why and leaves the fork as it was made.
    #[tokio::test]
    async fn a_carry_that_conflicts_fails_and_puts_the_fork_back() {
        let _lane = lane::enter().await;
        let place = DelegationPlace::new();
        let git = place.git();
        let source = pane_in(&place, "pane-con").await;
        let maker = place.maker();
        let ChildWorktreeCheck::Ready(read) = maker.check(source.worktree_path()).await else { panic!("the source cannot be read") };
        let start = pw::fork_start(&read, Some(&source));
        let ChildWorktreeOutcome::Created(made) =
            pw::make_pane(&crate::support::place::panes(&maker), "fork-c0ffee00", &start, 300.0).await
        else {
            panic!("no fork")
        };
        // The fork moves on with a change of its own to the same line; the source changes it differently without committing.
        std::fs::write(std::path::Path::new(&made.worktree_path).join("file-0.txt"), "the fork's line\n").unwrap();
        place.git_in(&made.worktree_path, &["commit", "-q", "-am", "fork side"]).await;
        let fork_head = place.git_in(&made.worktree_path, &["rev-parse", "HEAD"]).await;
        std::fs::write(std::path::Path::new(source.worktree_path()).join("file-0.txt"), "the source's line\n").unwrap();
        let before = place.git_in(source.worktree_path(), &["status", "--porcelain"]).await;
        let carried = pw::carry_changes(&git, source.worktree_path(), &made.worktree_path).await;
        assert!(matches!(&carried, pw::CarryOutcome::Failed(why) if !why.is_empty()), "{carried:?}");
        assert_eq!(place.git_in(&made.worktree_path, &["status", "--porcelain"]).await, "", "the fork is clean again");
        assert_eq!(place.git_in(&made.worktree_path, &["rev-parse", "HEAD"]).await, fork_head);
        assert_eq!(place.git_in(source.worktree_path(), &["status", "--porcelain"]).await, before, "the source is untouched");
    }

    async fn pane_in_other(
        place: &DelegationPlace,
        maker: &mighty_delegation::git::worktree::Maker,
        start: &mighty_delegation::git::worktree::ChildWorktreeBase,
        id: &str,
    ) -> String {
        let _ = place;
        let ChildWorktreeOutcome::Created(made) = pw::make_pane(&crate::support::place::panes(maker), id, start, 300.0).await else {
            panic!("no worktree {id}")
        };
        made.worktree_path
    }
}
