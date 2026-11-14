//! The smoke's delegation step (C# WinUI/MainWindow.DelegationSmoke.cs; live key `delegation`, `delegationError` on failure): after the
//! ACP step, before the run steps. Owner: W7-D.
//!
//! The fixture is `<profile>\Delegation Smoke\{repo, fake}` (`git init -b main`, a README committed as `Mighty Smoke
//! <smoke@example.invalid>`); Claude is the app's own exe `--fake-claude <fake>`; the switch is in memory; worktrees go under
//! `<profile>\worktrees` with unlimited free space (delegation.rs makes the smoke's maker so). Three children are made the way delegate
//! makes them and reported (B from A's head), C's notice is saved pending, and delegation restarts from the file. Then, under C#'s key
//! names: `childrenListed`, `sidebarTree`, `heldRowPinned`, `windowCloseMergedChild`, `phoneCloseMergedChild`,
//! `phoneSendReleasedHeldNotice`, `heldNoticeReceipt`, `secondSendNewWorktree`, `removeRefusedOpenChild`, `windowParentCloseAsks`,
//! `phoneParentCloseKeepsChild`, `removeRefusedKeptWorktree`, `removedAfterDiscard`, `otherAgentChildStarted`, and the Rust-only
//! `delegateTool`: the fake Claude parent calls `mcp__mighty-delegation__delegate` through the real exe mode and the real pipe, and the
//! child appears. The `phone*` keys call what the phone router will call (W7 plan D8: the close without its question, the send that
//! releases held items). Every git step targets the fixture in the smoke's own folder (its profile, or one of its own in a read-only smoke:
//! `delegation::may_touch`); nothing reaches
//! `%USERPROFILE%\.mightycode`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use mighty_core::model::{RunSession, RunSettings};
use mighty_core::wire;
use mighty_delegation::app::DelegationApp;
use mighty_delegation::cards::DelegationCardResult;
use mighty_delegation::delivery::DelegationRelease;
use mighty_delegation::git::cleanup::{ChildCleanupHold, ChildCleanupOutcome};
use mighty_delegation::git::worktree::{ChildWorktreeBase, ChildWorktreeCheck, ChildWorktreeOutcome, Maker, report_file_in};
use mighty_delegation::git::{Git, branch_name};
use mighty_delegation::host::{DelegationChildPane, DelegationHost};
use mighty_delegation::model::{ChildRecord, ChildRecordOptions, ChildState, DeliveryLane, DeliveryRoute, Notice, NoticeKind, request_key};
use mighty_delegation::request::{DelegationName, DelegationReasonCode};
use mighty_delegation::store::{DelegationFile, DelegationFileStore};
use mighty_platform::fake::FixedDiskSpace;
use mighty_run::model::CliCommand;
use serde_json::{Map, Value, json};

use tauri::Manager;

use crate::host::Host;

/// The keys the step reports, in C#'s order, then the Rust-only one.
const KEYS: [&str; 15] = [
    "childrenListed",
    "sidebarTree",
    "heldRowPinned",
    "windowCloseMergedChild",
    "phoneCloseMergedChild",
    "phoneSendReleasedHeldNotice",
    "heldNoticeReceipt",
    "secondSendNewWorktree",
    "removeRefusedOpenChild",
    "windowParentCloseAsks",
    "phoneParentCloseKeepsChild",
    "removeRefusedKeptWorktree",
    "removedAfterDiscard",
    "otherAgentChildStarted",
    "delegateTool",
];

/// Starts the step; false when it does not run (no delegation, no app binary).
pub fn start(host: &Arc<Host>) -> bool {
    if host.smoke.is_none() || crate::delegation::app().is_none() || std::env::current_exe().is_err() {
        return false;
    }
    host.log.line("smoke: delegation step (a fixture repository under the profile)");
    let host = host.clone();
    tauri::async_runtime::spawn(async move {
        let mut checks = Map::new();
        let outcome = run(&host, &mut checks).await;
        let mut report = Value::Object(checks);
        report["passed"] = json!(outcome.is_ok() && KEYS.iter().all(|key| report.get(*key).is_some_and(|v| !v.is_null() && *v != false)));
        if let Err(error) = outcome {
            host.log.line(&format!("smoke: delegation step failed: {error}"));
            report["error"] = json!(error);
        }
        if let Some(smoke) = &host.smoke {
            smoke.probed(&host, report);
        }
    });
    true
}

/// The step's checks from its report: each key passed, and the step as a whole.
pub fn checks(report: &Value) -> Vec<(&'static str, bool)> {
    let mut checks: Vec<(&'static str, bool)> =
        KEYS.iter().map(|key| (*key, report.get(*key).is_some_and(|value| !value.is_null() && *value != false))).collect();
    checks.push(("delegationPassed", report.get("passed") == Some(&Value::Bool(true))));
    checks
}

fn require(holds: bool, why: &str) -> Result<(), String> {
    if holds { Ok(()) } else { Err(why.to_string()) }
}

/// Polls until `produce` has a value, at most `seconds`.
async fn wait<T>(what: &str, seconds: u64, mut produce: impl FnMut() -> Option<T>) -> Result<T, String> {
    let end = Instant::now() + Duration::from_secs(seconds);
    loop {
        if let Some(found) = produce() {
            return Ok(found);
        }
        if Instant::now() >= end {
            return Err(format!("the smoke never reached: {what}"));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The smoke's git (C# `DelegationSmokeGit`, `DelegationSmokeCommit`): the delegation runner, committing as Mighty Smoke, unsigned.
async fn git(runner: &Git, folder: &Path, arguments: &[&str]) -> Result<String, String> {
    let mut all = vec!["-c", "user.name=Mighty Smoke", "-c", "user.email=smoke@example.invalid", "-c", "commit.gpgsign=false"];
    all.extend_from_slice(arguments);
    match runner.run(&all, folder, 30.0).await {
        Some(result) if result.exit_code == 0 => Ok(result.stdout.trim().to_string()),
        Some(result) => Err(format!("git {arguments:?} failed: {}", result.stderr.trim())),
        None => Err(format!("git {arguments:?} did not run")),
    }
}

/// A repository with one commit (the README) at `folder`.
async fn repository(runner: &Git, folder: &Path, title: &str) -> Result<(), String> {
    std::fs::create_dir_all(folder).map_err(|e| e.to_string())?;
    git(runner, folder, &["init", "-q", "-b", "main"]).await?;
    std::fs::write(folder.join("README.md"), format!("# {title}\n")).map_err(|e| e.to_string())?;
    git(runner, folder, &["add", "-A"]).await?;
    git(runner, folder, &["commit", "-q", "-m", "Start"]).await?;
    Ok(())
}

/// The folder listed as a workspace and a pane of `provider` in `mode` added to it, active: the pane's id and the workspace's.
fn add_parent(host: &Arc<Host>, folder: &Path, provider: &str, mode: &str, title: &str) -> Result<(String, String), String> {
    let workspace = host.approve_local(folder).map_err(|e| e.to_string())?;
    let (pane, workspace_id) = (wire::new_id(), workspace.id().to_string());
    let parent = RunSession {
        id: Some(pane.clone()),
        workspace_id: Some(workspace_id.clone()),
        kind: Some("claude".into()),
        provider: Some(provider.into()),
        title: Some(title.into()),
        settings: Some(RunSettings { permission_mode: mode.into(), ..RunSettings::default() }),
        ..RunSession::default()
    };
    host.view().update(|state| {
        if !state.workspaces().iter().any(|w| w.id() == workspace_id) {
            state.workspaces_mut().push(workspace.clone());
        }
        state.sessions_mut().push(Arc::new(parent.clone()));
        state.active_workspace_id = Some(workspace_id.clone());
        state.active_session_id = Some(pane.clone());
        // Open in the sidebar, as a folder the user opens is (frame.rs `open_workspace_folder`): its children are drawn there.
        if let Some(expanded) = mighty_core::shell::open_disclosure(state, &workspace_id) {
            state.expanded_workspace_ids = Some(expanded);
        }
    })?;
    host.schedule();
    Ok((pane, workspace_id))
}

/// A merged child closed: its worktree and branch went, its merge record stayed (C# `DelegationSmokeCleaned`).
async fn cleaned(app: &DelegationApp, runner: &Git, repo: &Path, id: &str) -> Result<bool, String> {
    let outcome = wait("the merged child's close", 30, || app.child_close(id)).await?;
    let file = app.coordinator().map(|c| c.file()).unwrap_or_default();
    let child = file.children.iter().find(|child| child.id == id).cloned().ok_or("the child is not listed")?;
    let branches = git(runner, repo, &["for-each-ref", "--format=%(refname)", &format!("refs/heads/{}", branch_name(id))]).await?;
    Ok(outcome == ChildCleanupOutcome::Cleaned
        && !Path::new(&child.worktree_path).exists()
        && branches.is_empty()
        && file.merges.iter().any(|merge| merge.child_id == id))
}

/// A check of what the page draws (C# `DelegationSmokeFind` over the WinUI tree): `body` runs in the page with `until(test, ms)` and
/// fills `r`; the page hands `r` back through the bridge's `log` method, which this step reads from the host's log (the step goes on,
/// where the smoke's own probe would end it).
async fn page_probe(host: &Host, body: &str) -> Result<Value, String> {
    // This process's own and new, so a line an earlier smoke left in the log is never read for it.
    let mark = format!("smoke-delegation-dom-{}-{}", std::process::id(), wire::new_id());
    let script = format!(
        "(async () => {{ const until = async (test, ms) => {{ const end = performance.now() + ms; while (performance.now() < end) {{ const v = await test(); if (v) return v; await new Promise((r) => setTimeout(r, 100)); }} return test(); }}; const r = {{}}; try {{ {body} }} catch (error) {{ r.error = String(error); }} await window.__TAURI_INTERNALS__.invoke('host_post', {{ message: {{ kind: 'call', id: 990000 + Math.floor(Math.random() * 9999), method: 'log', args: {{ level: 'info', message: '{mark} ' + JSON.stringify(r) }} }} }}); }})()"
    );
    let page = host.app.get_webview(crate::host::WINDOW).ok_or("no main window")?;
    page.eval(&script).map_err(|e| e.to_string())?;
    let log = host.log.dir().join("host.log");
    let needle = format!("web info: {mark} ");
    let line = wait("the page probe's answer", 40, || {
        std::fs::read_to_string(&log).ok()?.lines().find_map(|line| line.split_once(&needle).map(|(_, rest)| rest.to_string()))
    })
    .await?;
    serde_json::from_str(&line).map_err(|e| format!("the page probe's answer is no JSON: {e}"))
}

/// The pane's status in the state as the switch's check reads it (a busy pane's).
fn set_status(host: &Host, pane: &str, status: &str) -> Result<(), String> {
    host.view().update(|state| {
        if let Some(found) = state.sessions_mut().iter_mut().find(|p| p.id() == pane) {
            *found = Arc::new(RunSession { status: Some(status.into()), ..(**found).clone() });
        }
    })
}

/// The dialogs the smoke's driver was shown, and its answer to each (C# `smokeDelegationDialog`).
fn record_dialogs(answer: bool) -> Arc<std::sync::Mutex<Vec<Value>>> {
    let shown: Arc<std::sync::Mutex<Vec<Value>>> = Arc::default();
    let keep = shown.clone();
    crate::delegation::set_smoke_dialog(Some(Arc::new(move |dialog: &Value| {
        keep.lock().unwrap().push(dialog.clone());
        answer
    })));
    shown
}

/// A card's merge from the human (C# `DelegationSmokeMerge`).
async fn merge(app: &DelegationApp, id: &str) -> Result<(), String> {
    let note = app.merge_child(id).await.ok_or("the card was busy")?;
    require(note.result == DelegationCardResult::Done, &format!("the merge of {id} did not go through: {:?}", note.result))
}

async fn run(host: &Arc<Host>, checks: &mut Map<String, Value>) -> Result<(), String> {
    let app = crate::delegation::app().ok_or("no delegation")?;
    let runs = host.engine()?;
    let executable = std::env::current_exe().map_err(|e| e.to_string())?.to_string_lossy().into_owned();
    let root = crate::delegation::smoke_root(host).join("Delegation Smoke");
    let (repo, fake) = (root.join("repo"), root.join("fake"));
    std::fs::create_dir_all(&fake).map_err(|e| e.to_string())?;
    require(
        crate::delegation::may_touch(host, &repo) && crate::delegation::may_touch(host, &root.join("cross")),
        "the fixture is not under the smoke's folder",
    )?;
    runs.catalog.set_override(
        "claude",
        Some(CliCommand::new(
            executable,
            vec![mighty_delegation::fake_claude::ARGUMENT.into(), fake.to_string_lossy().into_owned()],
            Some("2.1.271"),
        )),
    );
    let runner = Git::new(host.processes.clone(), crate::delegation::smoke_root(host).join("git-no-hooks"));
    let outcome = steps(host, &app, &runner, &repo, &fake, &root, checks).await;
    // What the step added goes, whatever happened: the switch, the dialog driver, the override, the Agent IO listener its delegateTool
    // step started (Agent IO is off in a smoke, as C#'s), its panes and workspaces.
    crate::settings::general::set_delegation_on(host, false);
    crate::delegation::set_smoke_dialog(None);
    let closing = host.clone();
    let _ = tauri::async_runtime::spawn_blocking(move || crate::agent_io::shutdown(&closing)).await;
    runs.catalog.set_override("claude", None);
    let added: Vec<String> =
        host.view().snapshot.workspaces().iter().filter(|w| Path::new(w.path()).starts_with(&root)).map(|w| w.id().to_string()).collect();
    for workspace in &added {
        let panes: Vec<String> =
            host.view().snapshot.sessions().iter().filter(|s| s.workspace_id() == workspace).map(|s| s.id().to_string()).collect();
        for pane in panes {
            let _ = app.stop_and_wait(&pane).await;
        }
    }
    let _ = host.view().update(|state| {
        state.sessions_mut().retain(|s| !added.iter().any(|w| w == s.workspace_id()));
        state.workspaces_mut().retain(|w| !added.iter().any(|id| id == w.id()));
        if state.active_workspace_id.as_ref().is_some_and(|id| added.contains(id)) {
            state.active_workspace_id = state.workspaces().first().map(|w| w.id().to_string());
            state.active_session_id = None;
        }
    });
    host.schedule();
    outcome
}

#[allow(clippy::too_many_lines, reason = "the smoke's steps, in C#'s order, one block each")]
async fn steps(
    host: &Arc<Host>,
    app: &Arc<DelegationApp>,
    runner: &Git,
    repo: &Path,
    fake: &Path,
    root: &Path,
    checks: &mut Map<String, Value>,
) -> Result<(), String> {
    repository(runner, repo, "Delegation smoke").await?;
    let mode = "acceptEdits";
    let (parent, workspace) = add_parent(host, repo, "claude", mode, "Delegation parent")?;
    let repo_text = repo.to_string_lossy().into_owned();

    host.log.line("smoke: delegation: Three reported children in worktrees made the way delegate m");
    // Three reported children in worktrees made the way delegate makes them. B starts at A's head, so both merge by fast-forward.
    let maker = Maker::new(
        &crate::delegation::smoke_root(host).join("worktrees").to_string_lossy(),
        runner.clone(),
        Arc::new(FixedDiskSpace(u64::MAX)),
    );
    let ChildWorktreeCheck::Ready(start) = maker.check(&repo_text).await else {
        return Err("the smoke repository is not ready for worktrees".into());
    };
    let ids: Vec<String> = (0..3).map(|_| wire::new_id()).collect();
    let (mut records, mut folders, mut heads): (Vec<ChildRecord>, Vec<String>, Vec<String>) = (vec![], vec![], vec![]);
    for (index, id) in ids.iter().enumerate() {
        let task = format!("Smoke child {}: write note {}.", index + 1, index + 1);
        let from: ChildWorktreeBase =
            if index == 1 { ChildWorktreeBase { base_commit: heads[0].clone(), ..start.clone() } } else { start.clone() };
        let ChildWorktreeOutcome::Created(made) = maker.make(id, &from, &task, 30.0).await else {
            return Err(format!("child {}'s worktree was not made", index + 1));
        };
        let folder = PathBuf::from(&made.worktree_path);
        std::fs::write(folder.join(format!("note-{}.txt", index + 1)), format!("Note {}\n", index + 1)).map_err(|e| e.to_string())?;
        git(runner, &folder, &["add", "-A"]).await?;
        git(runner, &folder, &["commit", "-q", "-m", &format!("Note {}", index + 1)]).await?;
        std::fs::write(report_file_in(&made.worktree_path), format!("# Report\n\nChild {} wrote its note.\n", index + 1))
            .map_err(|e| e.to_string())?;
        let head = git(runner, &folder, &["rev-parse", "HEAD"]).await?;
        records.push(ChildRecord::make_with(
            id,
            &parent,
            &made.worktree_path,
            &made.parent_branch,
            &made.base_commit,
            mode,
            &request_key(&parent, "smoke", &task, mode, None),
            ChildRecordOptions {
                report_revision: 1,
                report_head: Some(head.clone()),
                state: Some(ChildState::Reported),
                parent_checkout: Some(repo_text.clone()),
                ..Default::default()
            },
        ));
        heads.push(head);
        folders.push(made.working_folder.clone());
    }
    let (a, b, c) = (ids[0].clone(), ids[1].clone(), ids[2].clone());
    // C's notice was pending when the app last quit, so this launch holds it.
    let notice = Notice::new(&wire::new_id(), &c, 1, NoticeKind::Reported, DeliveryLane::Pending);
    DelegationFileStore::new(crate::delegation::smoke_root(host))
        .save(&DelegationFile::of(records.clone(), vec![notice.clone()], vec![], vec![], vec![]))
        .map_err(|e| format!("{e:?}"))?;
    require(app.restart().await, "delegation did not load the smoke's file")?;
    for (id, folder) in ids.iter().zip(&folders) {
        let pane = DelegationChildPane {
            session_id: id.clone(),
            parent_session_id: parent.clone(),
            mode: mode.into(),
            folder: folder.clone(),
            started_by_human: false,
            provider: "claude".into(),
        };
        require(app.create_pane(pane).await, "a child pane was not made")?;
    }
    wait("the children and the held notice", 10, || {
        (app.rows().len() == 3 && app.held_items(&parent).iter().map(|item| item.id.clone()).collect::<Vec<_>>() == [notice.id.clone()])
            .then_some(())
    })
    .await?;
    checks.insert("childrenListed".into(), json!(true));
    // The sidebar lists each child one step in under its parent with its state and a card that merges and discards.
    let snapshot = host.view().snapshot.clone();
    let rows = app.rows();
    let tree = mighty_core::shell::delegation_tree(&workspace, &snapshot, &rows);
    let under: Vec<&str> =
        tree.children.get(&parent).map(|children| children.iter().map(|child| child.id.as_str()).collect()).unwrap_or_default();
    let cards = app.sidebar_children();
    let listed = ids.iter().all(|id| {
        under.contains(&id.as_str())
            && cards.iter().any(|child| {
                child.row.id == *id
                    && child.row.state == ChildState::Reported
                    && child.row.card_actions() == [mighty_core::shell::CardAction::Merge, mighty_core::shell::CardAction::Discard]
            })
    });
    require(listed, "the children are not listed one step in under their parent with merge and discard")?;
    let ids_json = serde_json::to_string(&ids).unwrap_or_default();
    let tree_probe = page_probe(
        host,
        &format!(
            "const ids = {ids_json}; r.cards = !!(await until(() => ids.every((id) => document.querySelector('[data-testid=\"child-card-' + id + '\"]')), 15000)); r.rows = ids.every((id) => {{ const row = document.querySelector('.sidebar-child [data-session-id=\"' + id + '\"]')?.closest('.sidebar-child'); return !!row && parseFloat(row.style.paddingLeft) === 38; }}); const slot = () => document.querySelector('.pane-slot:not(.hidden)[data-pane-id=\"{parent}\"]'); const first = await until(() => slot()?.querySelector('.composer-queue-item'), 15000); r.held = first?.dataset.queueHeld ?? null;"
        ),
    )
    .await?;
    require(
        tree_probe["cards"] == true && tree_probe["rows"] == true,
        &format!("the page does not list the children one step in under their parent with their cards: {tree_probe}"),
    )?;
    checks.insert("sidebarTree".into(), json!(true));
    require(tree_probe["held"] == notice.id.as_str(), &format!("the page's first queue row is not the held notice: {tree_probe}"))?;
    let held = {
        let mut view = host.view();
        crate::panes::composer_state(host, &mut view, &parent).map(|state| state.queue.held.first().map(|row| row.id.clone()))
    };
    require(held == Some(Some(notice.id.clone())), "the held notice is not the first row of the parent's queue")?;
    checks.insert("heldRowPinned".into(), json!(true));

    host.log.line("smoke: delegation: 1. A merged child closed in the window is cleaned up: its wo");
    // 1. A merged child closed in the window is cleaned up: its worktree and branch go, its merge record stays.
    merge(app, &a).await?;
    let merged_card = app.sidebar_children().into_iter().find(|child| child.row.id == a);
    require(
        merged_card
            .is_some_and(|child| child.row.card_actions().contains(&mighty_core::shell::CardAction::Undo) && child.card.note.is_some()),
        "the merged child's card does not offer undo or say it merged",
    )?;
    crate::frame::close_session(host, &a).await?;
    checks.insert("windowCloseMergedChild".into(), json!(cleaned(app, runner, repo, &a).await?));

    host.log.line("smoke: delegation: 2. So is one closed from the phone (its close never asks).");
    // 2. So is one closed from the phone (its close never asks).
    merge(app, &b).await?;
    app.close_session(&b, || {}).await.map_err(|e| e.to_string())?;
    checks.insert("phoneCloseMergedChild".into(), json!(cleaned(app, runner, repo, &b).await?));
    require(git(runner, repo, &["rev-parse", "main"]).await? == heads[1], "main is not at child B's head after both merges")?;

    host.log.line("smoke: delegation: 3. A phone send to the idle parent releases C's held notice ");
    // 3. A phone send to the idle parent releases C's held notice ahead of its text, in one run.
    let text = "Carry on with what the third child reported.";
    let mut request = mighty_run::model::StartRunRequest::new(&parent, &workspace, "claude", text);
    request.provider = "claude".into();
    request.settings = Some(RunSettings { permission_mode: mode.into(), ..RunSettings::default() });
    let release = app.send(request).await.map_err(|e| e.to_string())?;
    require(matches!(release, DelegationRelease::Released { .. }), "the phone send released nothing")?;
    let delivered = wait("C's notice delivered", 60, || {
        app.coordinator()?
            .file()
            .notices
            .clone()
            .into_iter()
            .find(|n| n.id == notice.id && n.lane == DeliveryLane::Delivered && n.receipt.is_some())
    })
    .await?;
    let runs = host.engine()?;
    wait("the parent's run to end", 60, || {
        (!runs.service.is_session_running(&parent) && runs.service.snapshot().session(&parent).is_some_and(|p| p.status() != "running"))
            .then_some(())
    })
    .await?;
    let sent = runs
        .service
        .snapshot()
        .session(&parent)
        .and_then(|p| p.logs().iter().rev().find(|e| e.kind() == "user").map(|e| e.text().to_string()));
    let sent = sent.unwrap_or_default();
    let pointer = sent.find(&format!("[Mighty Code notice {}]", notice.id));
    let human = sent.find(text);
    let receipt = delivered.receipt.clone().ok_or("the delivered notice has no receipt")?;
    require(
        receipt.route == DeliveryRoute::Queue
            && pointer.zip(human).is_some_and(|(p, h)| h > p)
            && sent.contains(&c)
            && app.held_items(&parent).is_empty(),
        "the held notice did not go ahead of the phone's text in one run",
    )?;
    checks.insert("phoneSendReleasedHeldNotice".into(), json!(true));
    checks.insert("heldNoticeReceipt".into(), json!({"route": receipt.route.name(), "runId": receipt.run_id}));

    host.log.line("smoke: delegation: 3b. With Mighty Orchestration on, the second send runs a tex");
    // 3b. With Mighty Orchestration on, the second send runs a text in a new child worktree beside the parent: the child is the human's,
    //     shows at once, runs the text, and reports to its card with no notice for its parent.
    host.view().update(|state| {
        state.active_session_id = Some(parent.clone());
        if let Some(pane) = state.sessions_mut().iter_mut().find(|p| p.id() == parent) {
            *pane = Arc::new(RunSession { draft: Some(String::new()), ..(**pane).clone() });
        }
    })?;
    let running = |host: &Host| {
        let snapshot = host.view().snapshot.clone();
        snapshot.session(&parent).map(|p| (snapshot.clone(), RunSession { status: Some("running".into()), ..(**p).clone() }))
    };
    let (snapshot, parent_pane) = running(host).ok_or("the parent is gone")?;
    require(
        app.second_send(&snapshot, &parent_pane) != mighty_composer::queue::ComposerSecondSend::Worktree,
        "the second send offered a worktree with the switch off",
    )?;
    // The switch itself has every running pane checked again (C# `SetDelegationOn`); the parent reads as running, as a busy pane's does.
    set_status(host, &parent, "running")?;
    crate::settings::general::set_delegation_on(host, true);
    wait("the parent's folder to be ready for worktrees", 30, || {
        let (snapshot, pane) = running(host)?;
        (app.second_send(&snapshot, &pane) == mighty_composer::queue::ComposerSecondSend::Worktree).then_some(())
    })
    .await?;
    let phrase = "Smoke human child";
    let script = json!([{
        "phrase": phrase,
        "steps": [
            {"write": "note-h.txt", "text": "Note h\n"},
            {"run": [["git", "add", "note-h.txt"], ["git", "commit", "-q", "-m", "Note h"]], "ask": false},
            {"write": ".mighty-delegation/REPORT.md", "text": "# Report\n\nWrote note h.\n"},
        ],
    }]);
    std::fs::write(fake.join("child-scripts.json"), script.to_string()).map_err(|e| e.to_string())?;
    crate::delegation::run_in_new_worktree(host, &parent, format!("{phrase}: write note h."), vec![], None, None);
    let human_child = wait("the human's child to report", 90, || {
        app.coordinator()?
            .file()
            .children
            .clone()
            .into_iter()
            .find(|child| child.started_by_human == Some(true) && child.state == ChildState::Reported)
    })
    .await?;
    let snapshot = host.view().snapshot.clone();
    let linked =
        snapshot.session(&human_child.id).is_some_and(|p| p.parent_session_id.as_deref() == Some(&parent) && p.provider() == "claude");
    let notices = app.coordinator().map(|c| c.file().notices.clone()).unwrap_or_default();
    require(
        linked
            && snapshot.active_session_id.as_deref() == Some(human_child.id.as_str())
            && human_child.starting_mode == mode
            && notices.iter().all(|n| n.child_id != human_child.id)
            && app.held_items(&parent).is_empty(),
        "the human's child did not open beside its parent, run its text and report with no notice",
    )?;
    checks.insert("secondSendNewWorktree".into(), json!(true));
    host.log.line("smoke: delegation: Its card's discard, through its confirmation, removes it aga");
    // Its card's discard, through its confirmation, removes it again.
    let shown = record_dialogs(true);
    crate::delegation::child_action(host.clone(), json!({"id": human_child.id, "action": "discard"})).await?;
    crate::delegation::set_smoke_dialog(None);
    require(!shown.lock().unwrap().is_empty(), "the human's child's discard asked nothing")?;
    let gone = app
        .coordinator()
        .is_some_and(|c| c.file().children.iter().any(|child| child.id == human_child.id && child.state == ChildState::Discarded));
    require(gone && !Path::new(&human_child.worktree_path).exists(), "the human's child was not discarded")?;
    crate::settings::general::set_delegation_on(host, false);
    set_status(host, &parent, "idle")?;

    host.log.line("smoke: delegation: 4. A workspace with an open child is not removed.");
    // 4. A workspace with an open child is not removed.
    require(
        crate::delegation::removal_refusal(host, &workspace).is_some() && host.view().snapshot.session(&c).is_some(),
        "a workspace with an open child was removed",
    )?;
    checks.insert("removeRefusedOpenChild".into(), json!(DelegationReasonCode::WorkspaceHasChildren.name()));

    host.log.line("smoke: delegation: 5. Closing the parent in the window asks first, naming its o");
    // 5. Closing the parent in the window asks first, naming its open child, and closes nothing yet.
    let parent_title = host.view().snapshot.session(&parent).map(|p| p.title().to_string()).unwrap_or_default();
    let asked = record_dialogs(false);
    crate::frame::close_session(host, &parent).await?;
    crate::delegation::set_smoke_dialog(None);
    let expected = mighty_delegation::text::parent_close_message(
        &mighty_delegation::cards::PendingParentClose { id: parent.clone(), title: parent_title.clone(), open_children: 1 },
        &host.locale,
    );
    let dialogs = asked.lock().unwrap().clone();
    require(
        dialogs.len() == 1
            && dialogs[0]["message"] == expected.as_str()
            && !parent_title.is_empty()
            && host.view().snapshot.session(&parent).is_some(),
        &format!("the window's close of a parent with an open child did not ask first, or closed it: {dialogs:?}"),
    )?;
    checks.insert("windowParentCloseAsks".into(), json!(true));

    host.log.line("smoke: delegation: 6. From the phone it closes only the parent at once: C stays");
    // 6. From the phone it closes only the parent at once: C stays open, listed under a 'parent closed' node.
    app.close_session(&parent, || {}).await.map_err(|e| e.to_string())?;
    let snapshot = host.view().snapshot.clone();
    let rows = app.rows();
    let tree = mighty_core::shell::delegation_tree(&workspace, &snapshot, &rows);
    require(
        snapshot.session(&c).is_some()
            && rows.iter().any(|row| row.id == c && row.state == ChildState::Reported)
            && tree.parent_closed.iter().any(|(id, _, children)| *id == parent && children.iter().any(|child| child.id == c)),
        "closing the parent did not keep its open child under 'parent closed'",
    )?;
    let closed_probe = page_probe(
        host,
        &format!(
            "r.node = !!(await until(() => document.querySelector('[data-testid=\"sidebar-parent-closed-{parent}\"]'), 15000)); r.child = !!document.querySelector('.sidebar-child [data-session-id=\"{c}\"]');"
        ),
    )
    .await?;
    require(
        closed_probe["node"] == true && closed_probe["child"] == true,
        &format!("the page shows no 'parent closed' node with C: {closed_probe}"),
    )?;
    checks.insert("phoneParentCloseKeepsChild".into(), json!(true));

    host.log.line("smoke: delegation: 7. Closing the unmerged child keeps its worktree, and the wo");
    // 7. Closing the unmerged child keeps its worktree, and the workspace stays refused while it is on disk.
    app.close_session(&c, || {}).await.map_err(|e| e.to_string())?;
    let kept = wait("C's close", 30, || app.child_close(&c)).await?;
    require(
        kept == ChildCleanupOutcome::Kept(ChildCleanupHold::NotMerged)
            && Path::new(&records[2].worktree_path).exists()
            && crate::delegation::removal_refusal(host, &workspace).is_some(),
        "a workspace with a child worktree not cleaned up was removed, or C's close removed its work",
    )?;
    let refused = crate::frame::remove_workspace(host, &workspace).await;
    require(
        refused.as_ref().err().is_some_and(|reason| reason.contains(DelegationReasonCode::WorkspaceHasChildren.name()))
            && host.view().snapshot.workspace(&workspace).is_some(),
        &format!("the window removed a workspace with a kept child worktree: {refused:?}"),
    )?;
    checks.insert("removeRefusedKeptWorktree".into(), json!(true));

    host.log.line("smoke: delegation: 8. Once the human discards C from its card, through its conf");
    // 8. Once the human discards C from its card, through its confirmation, the workspace goes.
    let confirmation = record_dialogs(true);
    crate::delegation::child_action(host.clone(), json!({"id": c, "action": "discard"})).await?;
    crate::delegation::set_smoke_dialog(None);
    let dialogs = confirmation.lock().unwrap().clone();
    require(
        dialogs.first().is_some_and(|dialog| {
            dialog["message"].as_str().is_some_and(|m| m.contains(&host.locale.get("delegation.discard.noNested")))
                && dialog["primary"] == host.locale.get("delegation.discard.confirm").as_str()
        }),
        &format!("the discard confirmation did not name what goes: {dialogs:?}"),
    )?;
    let discarded =
        app.coordinator().is_some_and(|co| co.file().children.iter().any(|child| child.id == c && child.state == ChildState::Discarded));
    require(discarded && !Path::new(&records[2].worktree_path).exists(), "C was not discarded")?;
    require(crate::delegation::removal_refusal(host, &workspace).is_none(), "the workspace is still refused")?;
    let removal = record_dialogs(true);
    crate::frame::remove_workspace(host, &workspace).await?;
    crate::delegation::set_smoke_dialog(None);
    require(removal.lock().unwrap().len() == 1, "the workspace's removal asked nothing")?;
    checks.insert("removedAfterDiscard".into(), json!(host.view().snapshot.workspace(&workspace).is_none()));

    host.log.line("smoke: delegation: 9. A child of another agent: on a Codex pane in manual mode,");
    // 9. A child of another agent: on a Codex pane in manual mode, whose own child could not change files, the worktree menu offers
    //    only a Claude child in plan mode, and its pick starts one, run by the scripted Claude (treated as ready).
    crate::settings::general::set_delegation_on(host, true);
    crate::delegation::set_smoke_agent_ready("claude", true);
    let cross = root.join("cross");
    repository(runner, &cross, "Cross-agent smoke").await?;
    let (codex_pane, _) = add_parent(host, &cross, "codex", "manual", "Codex parent")?;
    let codex = host.view().snapshot.session(&codex_pane).map(|p| (**p).clone()).ok_or("no Codex pane")?;
    // The switch's own check reaches the pane as it reads running, as a busy pane's does.
    set_status(host, &codex_pane, "running")?;
    crate::settings::general::set_delegation_on(host, false);
    crate::settings::general::set_delegation_on(host, true);
    wait("the Codex pane's worktree menu", 30, || {
        let snapshot = host.view().snapshot.clone();
        app.worktree_menu_only(&snapshot, &codex, false).then_some(())
    })
    .await?;
    let menu = app.worktree_child_choices(&codex, false);
    require(
        menu.len() == 3
            && menu[0].agent == "claude"
            && menu[0].mode.as_deref() == Some("plan")
            && menu[0].blocked.is_none()
            && menu[1..].iter().all(|choice| choice.mode.is_none() && choice.blocked.is_some()),
        "the read-only Codex pane's menu does not offer only a Claude child in plan mode",
    )?;
    crate::delegation::run_in_new_worktree(
        host,
        &codex_pane,
        "Smoke cross-agent child: write a note.".into(),
        vec![],
        Some("claude".into()),
        None,
    );
    let cross_child = wait("the Codex pane's Claude child", 60, || {
        app.coordinator()?.file().children.clone().into_iter().find(|child| {
            child.parent_session_id == codex_pane && child.started_by_human == Some(true) && child.state != ChildState::Creating
        })
    })
    .await?;
    let snapshot = host.view().snapshot.clone();
    require(
        cross_child.state != ChildState::Failed
            && cross_child.provider.is_none()
            && cross_child.starting_mode == "plan"
            && snapshot
                .session(&cross_child.id)
                .is_some_and(|p| p.provider() == "claude" && p.parent_session_id.as_deref() == Some(&codex_pane)),
        &format!("the Codex pane's pick of Claude did not start a Claude child ({})", cross_child.state.name()),
    )?;
    checks.insert(
        "otherAgentChildStarted".into(),
        json!({"parent": "codex", "child": "claude", "mode": cross_child.starting_mode, "state": cross_child.state.name()}),
    );
    host.log.line("smoke: delegation: The child settles before anything under it goes.");
    // The child settles before anything under it goes.
    let settle = Instant::now() + Duration::from_secs(30);
    while Instant::now() < settle && app.coordinator().is_some_and(|c| c.is_starting(&cross_child.id)) {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    host.log.line("smoke: delegation: Rust only: the fake Claude parent calls delegate through the");
    // Rust only: the fake Claude parent calls delegate through the real exe mode and the real pipe, and the child appears.
    host.log.line("smoke: delegation: listening");
    crate::agent_io::listen(host).ok_or("the Agent IO listener did not start")?;
    host.log.line("smoke: delegation: listener up");
    let tool_repo = root.join("tool");
    repository(runner, &tool_repo, "Delegate tool smoke").await?;
    let (tool_parent, tool_workspace) = add_parent(host, &tool_repo, "claude", "acceptEdits", "Delegating parent")?;
    let task = "Smoke delegate tool: write tool.txt.";
    let key: String = tool_repo.to_string_lossy().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    std::fs::create_dir_all(fake.join("scripts")).map_err(|e| e.to_string())?;
    std::fs::write(
        fake.join("scripts").join(format!("{key}.json")),
        json!({"turns": [[{"call": "delegate", "ask": false, "arguments": {"task": task, "mode": "acceptEdits"}}, {"say": "Delegated."}]]})
            .to_string(),
    )
    .map_err(|e| e.to_string())?;
    let mut request = mighty_run::model::StartRunRequest::new(&tool_parent, &tool_workspace, "claude", "Delegate the tool note.");
    request.provider = "claude".into();
    request.settings = Some(RunSettings { permission_mode: "acceptEdits".into(), ..RunSettings::default() });
    host.log.line("smoke: delegation: delegating parent asked");
    app.send(request).await.map_err(|e| e.to_string())?;
    host.log.line("smoke: delegation: delegating parent sent");
    let delegated = wait("the delegate tool's child", 60, || {
        let coordinator = app.coordinator()?;
        let file = coordinator.file();
        file.children
            .iter()
            .find(|child| {
                child.parent_session_id == tool_parent
                    && file.copy(&child.id, mighty_delegation::copy::DelegationCopyKind::Task).is_some_and(|copy| copy.text.contains(task))
                    && child.state != ChildState::Creating
            })
            .cloned()
    })
    .await?;
    let settle = Instant::now() + Duration::from_secs(30);
    while Instant::now() < settle && app.coordinator().is_some_and(|c| c.is_starting(&delegated.id)) {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let delegated = app
        .coordinator()
        .and_then(|coordinator| coordinator.file().children.iter().find(|child| child.id == delegated.id).cloned())
        .ok_or("the delegate tool's child is gone")?;
    require(delegated.state != ChildState::Failed, "the delegate tool's child failed to start")?;
    checks.insert("delegateTool".into(), json!({"child": delegated.id, "state": delegated.state.name()}));
    crate::delegation::set_smoke_agent_ready("claude", false);
    Ok(())
}
