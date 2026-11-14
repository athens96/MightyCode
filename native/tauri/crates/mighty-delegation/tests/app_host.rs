//! The panes' side of delegation (app_host.rs), C#'s checks under their names: DelegationVerification "delegation: child panes, the
//! run ledger and the pane's snapshot keys" (L648) and the panes' half of DelegationHumanChildVerification "delegation: records and
//! panes carry the human's mark and the parent's agent" (L683).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mighty_core::add_pane;
use mighty_core::json::Wire;
use mighty_core::model::{AppSnapshot, LogEntry, RunSession, RunSettings, Workspace};
use mighty_core::normalize;
use mighty_delegation::app_host::{self, ChildRunEnd, DelegationRunEvent, DelegationRunEventPump, DelegationRunLedger};
use mighty_delegation::host::{DelegationChildPane, DelegationPaneActivity, DelegationPaneState};
use mighty_delegation::request::DelegationReasonCode;
use serde_json::json;

fn child_pane(id: &str, parent: &str, mode: &str, folder: &str, by_human: bool, provider: &str) -> DelegationChildPane {
    DelegationChildPane {
        session_id: id.into(),
        parent_session_id: parent.into(),
        mode: mode.into(),
        folder: folder.into(),
        started_by_human: by_human,
        provider: provider.into(),
    }
}

fn settings(effort: &str, mode: &str) -> Option<RunSettings> {
    Some(RunSettings { effort: effort.into(), permission_mode: mode.into(), ..RunSettings::default() })
}

fn pane(id: &str, workspace: &str) -> RunSession {
    RunSession { id: Some(id.into()), workspace_id: Some(workspace.into()), ..RunSession::default() }
}

fn temp() -> String {
    std::env::temp_dir().to_string_lossy().into_owned()
}

/// DelegationVerification.PanesLedgerAndSnapshotKeys (L648).
#[tokio::test]
async fn child_panes_the_run_ledger_and_the_panes_snapshot_keys() {
    // A child pane is a Claude pane in the requested mode, never the last-used pane's.
    let last_used = RunSession {
        model: Some("claude-opus-5".into()),
        settings: settings("high", "fullAccess"),
        logs: Some(vec![LogEntry::new("l1".into(), "user", "hi".into(), "2026-10-09T10:00:00Z".into(), None)]),
        ..pane("last", "w")
    };
    let worktree = std::env::temp_dir().join("worktrees").join("child").to_string_lossy().into_owned();
    let child = app_host::child_session(&child_pane("child", "parent", "plan", &worktree, false, "claude"), "w", "Child pane", None, None);
    assert!(
        child.id() == "child"
            && child.workspace_id() == "w"
            && child.title.as_deref() == Some("Child pane")
            && child.kind() == "claude"
            && child.provider() == "claude"
            && child.model() == "default"
            && child.parent_session_id.as_deref() == Some("parent")
            && child.working_folder.as_deref() == Some(worktree.as_str())
            && child.settings == settings("default", "plan")
            && last_used.settings.as_ref().unwrap().permission_mode == "fullAccess",
        "exactly the mode asked for"
    );
    // A delegated child is never the template a new pane copies, and what is copied never carries its links.
    let parent = RunSession { created_at: Some("2026-10-01T00:00:00Z".into()), ..pane("parent", "w") };
    let recent = RunSession {
        model: Some("claude-opus-5".into()),
        settings: settings("high", "acceptEdits"),
        logs: Some(vec![LogEntry::new("c1".into(), "user", "hi".into(), "2026-10-09T10:00:00Z".into(), None)]),
        ..child.clone()
    };
    let sessions = vec![Arc::new(parent.clone()), Arc::new(recent.clone())];
    assert_eq!(
        add_pane::template(&pane("fresh", "w"), &sessions).map(|s| s.id()),
        Some("parent"),
        "the child is the most recently used Claude pane, but its parent is the template"
    );
    let fresh = add_pane::inherit(pane("fresh", "w"), &[Arc::new(recent.clone()), Arc::new(parent.clone())]);
    let written = fresh.to_json();
    assert!(
        fresh.parent_session_id.is_none()
            && fresh.working_folder.is_none()
            && !written.contains("parentSessionId")
            && !written.contains("workingFolder"),
        "a new pane never inherits the links"
    );

    // A child runs in its worktree, and refuses with worktree_missing without it; it never falls back to the parent's checkout.
    let workspace = Workspace { id: Some("w".into()), name: Some("W".into()), path: Some(temp()), ..Workspace::default() };
    let runs = app_host::run_workspace(&child, &workspace);
    assert!(
        runs.id() == "w" && runs.path() == worktree && app_host::run_workspace(&parent, &workspace) == workspace,
        "a child's run works in its worktree folder"
    );
    assert!(
        app_host::run_refusal(&parent).is_none()
            && app_host::run_refusal_with(&child, |_| true).is_none()
            && app_host::run_refusal_with(&child, |_| false) == Some(DelegationReasonCode::WorktreeMissing)
            && app_host::run_refusal_with(&RunSession { working_folder: None, ..child.clone() }, |_| true)
                == Some(DelegationReasonCode::WorktreeMissing)
            && app_host::run_refusal(&child) == Some(DelegationReasonCode::WorktreeMissing),
        "a child without its worktree may not run"
    );
    // A pane idle after a normal finish reads as idle while the app would refuse to start a run in it.
    let state = app_host::pane_state(&child, &workspace, Some("run-1"), DelegationPaneActivity::Finished, false);
    assert_eq!(
        state,
        DelegationPaneState {
            session_id: "child".into(),
            permission_mode: "plan".into(),
            folder: worktree.clone(),
            kind: "claude".into(),
            provider: "claude".into(),
            parent_session_id: Some("parent".into()),
            run_id: Some("run-1".into()),
            activity: DelegationPaneActivity::Finished,
        },
        "the pane as the coordinator reads it"
    );
    assert!(
        app_host::pane_state(&child, &workspace, Some("run-1"), DelegationPaneActivity::Finished, true).activity
            == DelegationPaneActivity::Idle
            && app_host::pane_state(&child, &workspace, Some("run-1"), DelegationPaneActivity::Running, true).activity
                == DelegationPaneActivity::Running
            && app_host::pane_state(&child, &workspace, None, DelegationPaneActivity::Idle, true).activity == DelegationPaneActivity::Idle,
        "only a finished pane the app would refuse reads as idle"
    );

    // Run ids and normal finishes since launch are tracked per pane.
    let ledger = DelegationRunLedger::new();
    assert!(
        ledger.run_id("a").is_none()
            && ledger.activity("a", false) == DelegationPaneActivity::Idle
            && ledger.end("a", "completed", false).is_none(),
        "a pane with no run since launch is idle"
    );
    let run = ledger.begin("a");
    assert!(
        run.starts_with("run-")
            && ledger.run_id("a").as_deref() == Some(run.as_str())
            && ledger.activity("a", true) == DelegationPaneActivity::Running,
        "a run gets an id"
    );
    assert!(
        ledger.end("a", "completed", false) == Some((run.clone(), ChildRunEnd::Finished))
            && ledger.end("a", "completed", false).is_none()
            && ledger.activity("a", false) == DelegationPaneActivity::Finished
            && ledger.run_id("a").as_deref() == Some(run.as_str()),
        "a second end of the same run is no end"
    );
    let second = ledger.begin("a");
    assert!(
        second != run
            && ledger.end("a", "stopped", false) == Some((second.clone(), ChildRunEnd::Stopped))
            && ledger.activity("a", false) == DelegationPaneActivity::Idle,
        "a stopped run leaves the pane idle"
    );
    assert!(ledger.end("b", "error", false).is_none(), "no run of b yet");
    let failing = ledger.begin("b");
    assert_eq!(ledger.end("b", "error", false), Some((failing, ChildRunEnd::Errored)), "an error");
    let killed = ledger.begin("b");
    assert!(
        ledger.end("b", "completed", true) == Some((killed, ChildRunEnd::Quit))
            && ledger.activity("b", false) == DelegationPaneActivity::Idle,
        "errors and a quit are told apart"
    );
    ledger.begin("a");
    ledger.end("a", "completed", false);
    ledger.forget("a");
    assert!(ledger.run_id("a").is_none() && ledger.activity("a", false) == DelegationPaneActivity::Idle, "a closed pane is forgotten");

    // Run events reach their handler one at a time, in the order sent.
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let busy = Arc::new(AtomicUsize::new(0));
    let overlapped = Arc::new(AtomicUsize::new(0));
    let (handled, inside, crossed) = (seen.clone(), busy.clone(), overlapped.clone());
    let pump = DelegationRunEventPump::new(
        &tokio::runtime::Handle::current(),
        Arc::new(move |change| {
            let (handled, inside, crossed) = (handled.clone(), inside.clone(), crossed.clone());
            Box::pin(async move {
                if inside.fetch_add(1, Ordering::SeqCst) + 1 > 1 {
                    crossed.fetch_add(1, Ordering::SeqCst);
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
                handled.lock().unwrap().push(match change {
                    DelegationRunEvent::Started { run_id, .. } => format!("start {run_id}"),
                    DelegationRunEvent::Ended { run_id, end, .. } => format!("end {run_id} {end:?}"),
                });
                inside.fetch_sub(1, Ordering::SeqCst);
            })
        }),
    );
    for index in 0..20 {
        pump.send(DelegationRunEvent::Started { child_id: "c".into(), run_id: format!("r{index}") });
        pump.send(DelegationRunEvent::Ended { child_id: "c".into(), run_id: format!("r{index}"), end: ChildRunEnd::Finished });
    }
    pump.finish().await;
    pump.send(DelegationRunEvent::Started { child_id: "c".into(), run_id: "late".into() });
    let expected: Vec<String> = (0..20).flat_map(|index| [format!("start r{index}"), format!("end r{index} Finished")]).collect();
    assert_eq!(*seen.lock().unwrap(), expected, "events are handed on in order, and none after the finish");
    assert_eq!(overlapped.load(Ordering::SeqCst), 0, "one at a time");

    // One event whose handling failed never stops the ones after it.
    let after = Arc::new(Mutex::new(Vec::<String>::new()));
    let kept = after.clone();
    let failing = DelegationRunEventPump::new(
        &tokio::runtime::Handle::current(),
        Arc::new(move |change| {
            let kept = kept.clone();
            Box::pin(async move {
                if let DelegationRunEvent::Started { run_id, .. } = &change
                    && run_id == "bad"
                {
                    panic!("a handler that fails");
                }
                kept.lock().unwrap().push(format!("{change:?}"));
            })
        }),
    );
    failing.send(DelegationRunEvent::Started { child_id: "c".into(), run_id: "bad".into() });
    failing.send(DelegationRunEvent::Ended { child_id: "c".into(), run_id: "good".into(), end: ChildRunEnd::Stopped });
    failing.finish().await;
    assert_eq!(after.lock().unwrap().len(), 1, "the event after a failed one is handed on");

    // The pane's two optional snapshot keys: decoded, a damaged value keeps the conversation, and a state file written before them loads
    // unchanged.
    let folder = std::env::temp_dir().join("w").to_string_lossy().into_owned();
    let child_folder = worktree.clone();
    let saved = json!({
        "version": 1,
        "workspaces": [{"id": "w", "name": "W", "path": folder, "createdAt": "2026-10-01T00:00:00Z"}],
        "sessions": [
            {"id": "parent", "workspaceId": "w", "title": "Claude", "createdAt": "2026-10-09T00:00:00Z"},
            {"id": "child", "workspaceId": "w", "title": "Claude", "createdAt": "2026-10-09T00:00:00Z", "parentSessionId": "parent",
             "workingFolder": child_folder},
            {"id": "damaged", "workspaceId": "w", "title": "Claude", "createdAt": "2026-10-09T00:00:00Z", "parentSessionId": 7,
             "workingFolder": ["x"], "logs": [{"id": "l1", "kind": "user", "text": "keep me", "timestamp": "2026-10-09T00:00:01Z"}]}
        ],
        "activeWorkspaceId": "w"
    });
    let snapshot = normalize::normalize(&AppSnapshot::read(&saved).unwrap(), true).unwrap();
    assert_eq!(snapshot.sessions().iter().map(|s| s.id()).collect::<Vec<_>>(), ["parent", "child", "damaged"], "every pane loads");
    let (loaded, damaged) = (&snapshot.sessions()[1], &snapshot.sessions()[2]);
    assert!(
        loaded.parent_session_id.as_deref() == Some("parent")
            && loaded.working_folder.as_deref() == Some(child_folder.as_str())
            && damaged.parent_session_id.is_none()
            && damaged.working_folder.is_none()
            && damaged.logs().iter().map(|l| l.text()).collect::<Vec<_>>() == ["keep me"],
        "both keys load, and a damaged value keeps the conversation"
    );
    let again = normalize::normalize(&AppSnapshot::read(&serde_json::from_str(&snapshot.to_json()).unwrap()).unwrap(), true).unwrap();
    let plain = again.sessions()[0].to_json();
    assert!(
        again.sessions()[1].parent_session_id.as_deref() == Some("parent")
            && again.sessions()[1].working_folder.as_deref() == Some(child_folder.as_str())
            && !plain.contains("parentSessionId")
            && !plain.contains("workingFolder"),
        "saving writes both keys back for a child and neither for any other pane"
    );

    // Normalize keeps well-formed links, even for a closed parent, and drops the rest.
    let linked = |id: &str, parent: Option<&str>, folder: Option<&str>, kind: &str, provider: &str| RunSession {
        kind: Some(kind.into()),
        provider: Some(provider.into()),
        created_at: Some("2026-10-09T00:00:00Z".into()),
        parent_session_id: parent.map(str::to_string),
        working_folder: folder.map(str::to_string),
        ..pane(id, "w")
    };
    let shell = std::path::Path::new(&child_folder).join("shell").to_string_lossy().into_owned();
    let long = format!("C:\\{}", "a".repeat(4096));
    let bad = format!("{child_folder}\0b");
    let linked = vec![
        linked("child", Some("closed-parent"), Some(&child_folder), "claude", "claude"),
        linked("codex", Some("closed-parent"), Some(&child_folder), "claude", "codex"),
        linked("self", Some("self"), Some("relative/folder"), "claude", "claude"),
        linked("bad", Some("not an id"), Some(&bad), "claude", "claude"),
        linked("long", Some("p"), Some(&long), "claude", "claude"),
        linked("rooted", Some("p"), Some("\\no\\drive"), "claude", "claude"),
        linked("shell", Some("child"), Some(&shell), "shell", "claude"),
        linked("browser", Some("child"), Some(&shell), "browser", "claude"),
        pane("plain", "w"),
    ];
    for restoring in [true, false] {
        let input = AppSnapshot {
            workspaces: Some(vec![workspace.clone()]),
            sessions: Some(linked.iter().cloned().map(Arc::new).collect()),
            active_workspace_id: Some("w".into()),
            ..AppSnapshot::default()
        };
        let output = normalize::normalize(&input, restoring).unwrap();
        let links = |id: &str| {
            let session = output.sessions().iter().find(|s| s.id() == id).unwrap();
            (session.parent_session_id.clone(), session.working_folder.clone())
        };
        let some = |value: &str| Some(value.to_string());
        assert_eq!(
            output.sessions().iter().map(|s| s.id()).collect::<Vec<_>>(),
            linked.iter().map(|s| s.id()).collect::<Vec<_>>(),
            "no pane is dropped"
        );
        assert!(
            links("child") == (some("closed-parent"), some(&child_folder))
                && links("codex") == (some("closed-parent"), some(&child_folder))
                && links("self") == (None, None)
                && links("bad") == (None, None)
                && links("long") == (some("p"), None)
                && links("rooted") == (some("p"), None)
                && links("shell") == (None, None)
                && links("browser") == (None, None)
                && links("plain") == (None, None),
            "well-formed links stay and malformed ones go, restoring {restoring}"
        );
    }
    assert!(
        app_host::is_working_folder(Some(&child_folder))
            && !app_host::is_working_folder(None)
            && !app_host::is_working_folder(Some(""))
            && !app_host::is_working_folder(Some("C:relative")),
        "a working folder is a full path"
    );
}

/// The panes' half of DelegationHumanChildVerification.RecordsAndPanesCarryTheMarkAndTheParentsAgent (L683).
#[test]
fn panes_carry_the_humans_mark_and_the_parents_agent() {
    // A human's child takes its parent's model, effort and view; only a Claude one takes the style.
    for provider in ["claude", "codex", "gemini"] {
        let parent = RunSession {
            provider: Some(provider.into()),
            model: Some("model-x".into()),
            settings: settings("high", "acceptEdits"),
            agent_view_mode: Some("mighty".into()),
            graph_view_mode: Some("structure".into()),
            mighty_style: Some("claude-plan".into()),
            mighty_style_hash: Some("h1".into()),
            mighty_style_since: Some("2026-10-15T00:00:00Z".into()),
            ..pane("parent", "ws")
        };
        let child = child_pane("c1", "parent", "acceptEdits", "/w/c1", true, provider);
        let made = app_host::child_session(&child, "ws", "Fix it", Some(&parent), None);
        let style = if provider == "claude" {
            made.mighty_style.as_deref() == Some("claude-plan") && made.mighty_style_hash.as_deref() == Some("h1")
        } else {
            made.mighty_style.is_none() && made.mighty_style_hash.is_none()
        };
        assert!(
            made.kind() == "claude"
                && made.model() == "model-x"
                && made.agent_view_mode.as_deref() == Some("mighty")
                && made.graph_view_mode.as_deref() == Some("structure")
                && made.parent_session_id.as_deref() == Some("parent")
                && made.working_folder.as_deref() == Some("/w/c1")
                && made.mighty_style_since.is_none()
                && made.provider() == provider
                && made.settings == settings("high", "acceptEdits")
                && style,
            "{provider}: a human's child takes the parent's model and view"
        );
        let agent =
            app_host::child_session(&DelegationChildPane { started_by_human: false, ..child.clone() }, "ws", "Fix it", Some(&parent), None);
        assert!(
            agent.model() == "default"
                && agent.agent_view_mode.as_deref() == Some("mighty")
                && agent.graph_view_mode.as_deref() == Some("structure")
                && agent.mighty_style.is_none()
                && agent.settings.as_ref().unwrap().effort == "default"
                && agent.provider() == provider,
            "{provider}: a delegate call's child shows the view only"
        );
    }
    let orphan = app_host::child_session(&child_pane("c4", "parent", "auto", "/w/c4", false, "claude"), "ws", "t", None, None);
    assert!(
        orphan.agent_view_mode.is_none() && orphan.graph_view_mode.is_none() && orphan.provider() == "claude",
        "no parent leaves the defaults"
    );

    // The human's own child shows at once while the human is still on its parent with nothing typed there.
    let workspace = Workspace { id: Some("ws".into()), path: Some(temp()), ..Workspace::default() };
    let state = AppSnapshot {
        workspaces: Some(vec![workspace]),
        sessions: Some(vec![Arc::new(pane("parent", "ws"))]),
        active_workspace_id: Some("ws".into()),
        active_session_id: Some("parent".into()),
        ..AppSnapshot::default()
    };
    let active = |state: &AppSnapshot, by_human: bool| {
        app_host::adding_child(state, &child_pane("c1", "parent", "plan", &temp(), by_human, "claude"), Some("Fix it"), false)
            .and_then(|next| next.active_session_id)
    };
    assert_eq!(active(&state, true).as_deref(), Some("c1"), "the human's child shows at once");
    assert_eq!(active(&state, false).as_deref(), Some("parent"), "a delegate call's child does not take the focus");
    let typing = AppSnapshot {
        sessions: Some(vec![Arc::new(RunSession { draft: Some("half a thought".into()), ..pane("parent", "ws") })]),
        ..state.clone()
    };
    assert_eq!(active(&typing, true).as_deref(), Some("parent"), "nor does the human's while they type in the parent");
}

/// `DelegationPanes.AddingChild` beyond what the human sees: the tab beside its parent, titled by its task and kept so, the saved layout
/// and mode, a quiet child never selected, and the refusals that change nothing (M/AppStore+Delegation.swift createPane).
#[test]
fn a_child_joins_its_parents_tab_group_beside_it() {
    let workspace = Workspace { id: Some("ws".into()), path: Some(temp()), ..Workspace::default() };
    let state = AppSnapshot {
        workspaces: Some(vec![workspace]),
        sessions: Some(vec![Arc::new(pane("first", "ws")), Arc::new(pane("parent", "ws")), Arc::new(pane("last", "ws"))]),
        active_workspace_id: Some("ws".into()),
        active_session_id: Some("parent".into()),
        layout: Some("tabs".into()),
        ..AppSnapshot::default()
    };
    let human = child_pane("c1", "parent", "plan", &temp(), true, "claude");
    let next = app_host::adding_child(&state, &human, Some("  Fix   the\nbuild  "), false).unwrap();
    let child = next.session("c1").unwrap();
    assert!(
        child.title.as_deref() == Some("Fix the build") && child.title_mode.as_deref() == Some("fixed"),
        "titled by its task, kept as the pane's name"
    );
    let root = next.pane_layouts.as_ref().and_then(|layouts| layouts.get("ws")).cloned().flatten().unwrap();
    let group = mighty_core::layout::groups(&root)[0].clone();
    assert_eq!(group.session_ids().collect::<Vec<_>>(), ["first", "parent", "c1", "last"], "the tab sits right after its parent");
    assert_eq!(group.selected_session_id.as_deref(), Some("c1"), "the human's child is selected");
    assert_eq!(
        next.pane_layout_modes.as_ref().and_then(|modes| modes.get("ws")).cloned().flatten().as_deref(),
        Some("tabs"),
        "the workspace leaves its preset mode"
    );
    // The phone's 바로 실행: never selected, never active.
    let quiet = app_host::adding_child(&state, &human, Some("Fix it"), true).unwrap();
    let root = quiet.pane_layouts.as_ref().and_then(|layouts| layouts.get("ws")).cloned().flatten().unwrap();
    assert!(
        quiet.active_session_id.as_deref() == Some("parent")
            && mighty_core::layout::groups(&root)[0].selected_session_id.as_deref() == Some("parent"),
        "a quiet child is never selected"
    );
    // Without a task the agent names it; focus stays focus.
    let focused = AppSnapshot { layout: Some("focus".into()), ..state.clone() };
    let codex = child_pane("c2", "parent", "acceptEdits", &temp(), false, "codex");
    let named = app_host::adding_child(&focused, &codex, None, false).unwrap();
    assert!(
        named.session("c2").unwrap().title.as_deref() == Some("Codex CLI")
            && named.pane_layout_modes.as_ref().and_then(|m| m.get("ws")).cloned().flatten().as_deref() == Some("focus"),
        "the agent's name, and focus stays focus"
    );
    // No pane is made for a taken id, a missing parent, a missing workspace, or past the cap.
    assert!(app_host::adding_child(&next, &human, Some("again"), false).is_none(), "the id is taken");
    assert!(
        app_host::adding_child(&state, &child_pane("c3", "gone", "plan", &temp(), false, "claude"), None, false).is_none(),
        "the parent is gone"
    );
    let orphaned = AppSnapshot { workspaces: Some(vec![]), ..state.clone() };
    assert!(app_host::adding_child(&orphaned, &human, None, false).is_none(), "the parent's workspace is gone");
    let full = AppSnapshot {
        sessions: Some((0..mighty_core::layout::MAXIMUM_SESSIONS).map(|i| Arc::new(pane(&format!("p{i}"), "ws"))).collect()),
        ..state.clone()
    };
    assert!(
        app_host::adding_child(&full, &child_pane("c4", "p0", "plan", &temp(), false, "claude"), None, false).is_none(),
        "the app is at its pane cap"
    );
}
