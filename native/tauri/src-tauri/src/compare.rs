//! 비교 실행 in the host (MC-29, after the 0.4 Swift app's `CompareRun.swift` and `AppStore+CompareRun.swift`; the rules are
//! mighty-delegation's `compare`): one request run by several agents — or one agent several times — side by side, each in its own
//! delegated child worktree from the same base commit, compared, and one merged.
//!
//! - Start (`compareOptions`, `compareStart`): the composer's 비교 실행… opens the page's picker with the agents that can start a
//!   child now (installed, signed in, a mode that writes, room under the human children's cap), their models, the recent commits of
//!   the branch (HEAD first), the repository's run actions and `.mighty.yaml` tasks, and the budget reading. The start checks the rows
//!   again, warns once when the daily budget would likely be passed, and starts one human child per row
//!   ([`mighty_delegation::app::DelegationApp::start_human_child_with`]) from the chosen commit, each child's pane given its model and
//!   its own port block before its first run. A row that cannot start says why on its column; the others go on.
//! - The group (`RunSession.compare_runs` on the pane that started it) survives a restart; each child carries its link
//!   (`RunSession.compare_member`) so its runs get its port block. The sidebar draws a card under the pane ([`decorate`]), the phone
//!   payload carries the group (mighty-remote's router).
//! - Compare view (`compareView`, `compareAction`): per child its agent and model, state, `git diff --shortstat <base>...<branch>`,
//!   elapsed time, cost (the usage ledger's records of its pane, [`add_cost`]) and test result; open, diff (the changes sheet), merge
//!   (only a reported child, through the card's merge and 머지 전 점검, then "discard the other n?") and discard (the card's).
//! - The chosen test runs once in each child after it reports ([`delegation_changed`], on delegation's own change, no timer).
//!
//! Gating as delegation's: a start, a merge, a discard and a test need `--allow-state-write` (or a smoke on its fixture) and a folder
//! delegation may touch; reading git is always allowed.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use mighty_core::attachment::RunAttachment;
use mighty_core::model::{
    AppSnapshot, COMPARE_REQUEST_LIMIT, COMPARE_TEST_OUTPUT_LIMIT, CompareChild, CompareMember, CompareRun, RunSession,
};
use mighty_core::shell::{ChildState, ShellModel};
use mighty_core::{time, wire};
use mighty_delegation::app::HumanChildExtras;
use mighty_delegation::cards::DelegationCardResult;
use mighty_delegation::compare::{self as rules, ComparePhase, CompareRow, CompareRowsError};
use mighty_delegation::git::worktree::ChildWorktreeCheck;
use mighty_delegation::tools::delegate::HumanChildStart;
use mighty_platform::BoxFuture;
use serde_json::{Value, json};

use crate::host::{Host, Registry, lock};
use crate::methods::{flag, text};

/// How long a child's test may run before it is stopped.
const TEST_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// How many commits of the branch the picker offers as the base.
const BASES_OFFERED: usize = 15;

/// The tests running now (`<group>/<child>`), so one never starts twice; and whether a check of the groups waits to run.
#[derive(Default)]
struct State {
    testing: HashSet<String>,
}

static STATE: LazyLock<Mutex<State>> = LazyLock::new(Mutex::default);
static CHECK_PENDING: AtomicBool = AtomicBool::new(false);

pub fn register(methods: &mut Registry) {
    methods.add("compareOptions", |host: Arc<Host>, args| async move {
        let Some(pane) = text(&args, "paneId").map(str::to_string) else { return Ok(Value::Null) };
        Ok(options(&host, &pane).await)
    });
    methods.add("compareStart", |host: Arc<Host>, args| async move { Ok(start(&host, &args).await) });
    methods.add("compareView", |host: Arc<Host>, args| async move {
        let (Some(pane), Some(group)) = (text(&args, "paneId"), text(&args, "compareId")) else { return Ok(Value::Null) };
        check_groups(&host);
        Ok(view(&host, pane, group).await)
    });
    methods.add("compareAction", |host: Arc<Host>, args| async move {
        let (Some(pane), Some(group), Some(action)) = (text(&args, "paneId"), text(&args, "compareId"), text(&args, "action")) else {
            return Ok(Value::Null);
        };
        let child = text(&args, "childId").map(str::to_string);
        Ok(action_on(&host, pane, group, child.as_deref(), action).await)
    });
}

// ── reading the state ───────────────────────────────────────────────────────────────────────────────────────────────────

fn group_of(snapshot: &AppSnapshot, pane: &str, group: &str) -> Option<CompareRun> {
    snapshot.session(pane)?.compare_runs.as_ref()?.iter().find(|run| run.id == group).cloned()
}

/// The delegated children's states by id (an empty map while delegation is not loaded).
fn child_states() -> HashMap<String, ChildState> {
    crate::delegation::app().map(|app| app.rows().into_iter().map(|row| (row.id, row.state)).collect()).unwrap_or_default()
}

/// Changes the group `group` of the pane `pane` in the state (saved with it); false when either is gone.
fn update_group(host: &Arc<Host>, pane: &str, group: &str, change: impl FnOnce(&mut CompareRun)) -> bool {
    let mut found = false;
    let _ = host.view().update(|state| {
        let Some(session) = state.sessions_mut().iter_mut().find(|s| s.id() == pane) else { return };
        let mut next = RunSession::clone(session);
        let Some(run) = next.compare_runs.as_mut().and_then(|runs| runs.iter_mut().find(|run| run.id == group)) else { return };
        change(run);
        found = true;
        *session = Arc::new(next);
    });
    if found {
        host.shell_dirty();
        crate::phone::changed(host);
        host.schedule();
    }
    found
}

/// Changes the child `child` of the group in the state.
fn update_child(host: &Arc<Host>, pane: &str, group: &str, child: &str, change: impl FnOnce(&mut CompareChild)) -> bool {
    let mut changed = false;
    update_group(host, pane, group, |run| {
        if let Some(entry) = run.children.iter_mut().find(|c| c.id == child) {
            change(entry);
            changed = true;
        }
    }) && changed
}

/// The compare link of the pane `child` (a compare run's child): its parent and group.
fn member_of(host: &Host, child: &str) -> Option<CompareMember> {
    host.view().snapshot.session(child)?.compare_member.clone()
}

/// The folder an agent pane's runs work in (its worktree's, or the workspace folder).
fn pane_folder(snapshot: &AppSnapshot, pane: &RunSession) -> Option<String> {
    pane.working_folder.clone().or_else(|| snapshot.workspace(pane.workspace_id()).map(|w| w.path().to_string())).filter(|f| !f.is_empty())
}

// ── the picker ──────────────────────────────────────────────────────────────────────────────────────────────────────────

/// Why a compare run cannot start from the pane now; None when it can be offered.
fn unavailable(host: &Host, snapshot: &AppSnapshot, pane: &RunSession) -> Option<String> {
    let locale = host.locale;
    if pane.kind() != "claude" || !mighty_delegation::providers::is_agent_pane(pane.kind(), pane.provider()) {
        return Some(locale.get("compare.reason.notAgent"));
    }
    if snapshot.workspace(pane.workspace_id()).is_some_and(|w| w.remote.is_some()) {
        return Some(locale.get("compare.reason.remote"));
    }
    if pane.parent_session_id.is_some() {
        return Some(locale.get("compare.reason.child"));
    }
    if crate::delegation::app().and_then(|app| app.coordinator()).is_none() {
        return Some(locale.get("delegation.human.error.switchedOff"));
    }
    if !host.delegation_live() || !pane_folder(snapshot, pane).is_some_and(|f| crate::delegation::may_touch(host, Path::new(&f))) {
        return Some(crate::setup::verification_mode());
    }
    None
}

async fn git_lines(folder: &str, arguments: &[&str]) -> Option<Vec<String>> {
    let places = crate::pane_worktree::places()?;
    let result = mighty_delegation::git::run(&places.git, arguments, folder, 10.0).await?;
    (result.exit_code == 0).then(|| result.stdout.lines().map(|l| l.trim_end_matches('\r').to_string()).filter(|l| !l.is_empty()).collect())
}

/// `compareOptions{paneId}`: what the picker offers (or why it cannot): the agents with their models, the room left, the branch's
/// recent commits, the tests, and the budget reading with each agent's (and model's) estimate.
async fn options(host: &Arc<Host>, pane_id: &str) -> Value {
    let locale = host.locale;
    let (pane, snapshot, plan_first) = {
        let view = host.view();
        let Some(pane) = view.snapshot.session(pane_id).cloned() else { return Value::Null };
        let plan_first = view.panes.get(pane_id).is_some_and(|ui| ui.view.launches_in_plan_mode);
        (pane, view.snapshot.clone(), plan_first)
    };
    let refuse = |reason: String| json!({ "available": false, "reason": reason });
    if let Some(reason) = unavailable(host, &snapshot, &pane) {
        return refuse(reason);
    }
    let Some(app) = crate::delegation::app() else { return refuse(locale.get("delegation.human.error.switchedOff")) };
    let Some(folder) = pane_folder(&snapshot, &pane) else { return refuse(locale.get("compare.reason.notAgent")) };
    if let Some(coordinator) = app.coordinator()
        && let ChildWorktreeCheck::Refused(reason) = coordinator.worktrees().check(&folder).await
        && !matches!(reason, mighty_delegation::request::DelegationReasonCode::LowDisk)
    {
        return refuse(mighty_delegation::text::reason(reason, &locale));
    }
    let open = mighty_delegation::sidebar::open_human_child_count(pane.id(), &app.rows());
    let most = rules::max_rows(open);
    let choices = app.worktree_child_choices(&pane, plan_first);
    let ready = rules::ready_agents(&choices, pane.provider());
    if most < 2 {
        return refuse(
            locale.get_with("compare.reason.room", &[("cap", &mighty_delegation::coordinator::MAXIMUM_OPEN_CHILDREN.to_string())]),
        );
    }
    if ready.is_empty() {
        return refuse(locale.get("compare.reason.agents"));
    }
    let (records, spent, budget) = crate::usage_report::budget_reading(host);
    let mut estimates = vec![];
    let agents: Vec<Value> = ready
        .iter()
        .map(|agent| {
            let catalog = host
                .runs()
                .and_then(|runs| runs.runtime(agent))
                .map(|runtime| runtime.model_catalog)
                .unwrap_or_else(|| mighty_run::providers::fallback(agent));
            let models: Vec<Value> = catalog
                .models
                .iter()
                .filter(|m| m.value != "default" && wire::model(&m.value))
                .map(|m| {
                    estimates.push(
                        json!({ "agent": agent, "model": m.value, "usd": rules::estimate_run_cost(&records, agent, Some(&m.value)) }),
                    );
                    json!({ "id": m.value, "label": m.display_name })
                })
                .collect();
            estimates.push(json!({ "agent": agent, "model": Value::Null, "usd": rules::estimate_run_cost(&records, agent, None) }));
            json!({ "agent": agent, "label": mighty_composer::runtime::provider_label(agent), "models": models })
        })
        .collect();
    let bases: Vec<Value> = git_lines(&folder, &["log", &format!("-{BASES_OFFERED}"), "--format=%H%x09%h%x09%s", "HEAD"])
        .await
        .unwrap_or_default()
        .iter()
        .enumerate()
        .filter_map(|(index, line)| {
            let mut parts = line.splitn(3, '\t');
            let (sha, short, subject) = (parts.next()?, parts.next()?, parts.next().unwrap_or(""));
            Some(json!({ "sha": sha, "short": short, "subject": subject.chars().take(120).collect::<String>(), "head": index == 0 }))
        })
        .collect();
    let branch = git_lines(&folder, &["symbolic-ref", "--short", "-q", "HEAD"]).await.and_then(|l| l.into_iter().next());
    let (tests, approved) = tests_in(host, &snapshot, &pane, &folder).await;
    json!({
        "available": true,
        "max": most,
        "parentAgent": pane.provider(),
        "agents": agents,
        "bases": bases,
        "branch": branch,
        "tests": tests,
        "testsApproved": approved,
        "budget": { "usd": budget, "spent": spent, "estimates": estimates, "fallback": rules::FALLBACK_RUN_COST },
    })
}

/// The run actions and `.mighty.yaml` tasks of the pane's folder (each run in a child's worktree once it reports), and whether the
/// repository's config is approved (they run only then).
async fn tests_in(host: &Host, snapshot: &AppSnapshot, pane: &RunSession, folder: &str) -> (Vec<Value>, bool) {
    let Some(workspace) = snapshot.workspace(pane.workspace_id()).cloned() else { return (vec![], false) };
    let (target, workspace_folder) = (PathBuf::from(folder), PathBuf::from(workspace.path()));
    let Ok((config, tasks)) =
        tauri::async_runtime::spawn_blocking(move || crate::repo_tasks::scan_folder_tasks(&target, &workspace_folder)).await
    else {
        return (vec![], false);
    };
    let approved =
        config.ok().flatten().is_some_and(|loaded| crate::repo_tasks::is_config_approved(host, workspace.path(), &loaded.content_hash));
    let tests = tasks
        .into_iter()
        .filter(|task| task.source == "run" || task.source == "mighty")
        .map(|task| json!({ "id": task.id, "name": task.name, "source": task.source, "command": task.command }))
        .collect();
    (tests, approved)
}

// ── the start ───────────────────────────────────────────────────────────────────────────────────────────────────────────

fn money(value: f64) -> String {
    format!("${value:.2}")
}

/// `compareStart{paneId, text, rows: [{agent, model}], base, test, budgetConfirmed}`: the rows checked again, the budget warning
/// once, then one child per row. Answers `{ok, compareId, started, failed}`, `{ok: false, budget}` (asked to confirm) or
/// `{ok: false, error}`.
async fn start(host: &Arc<Host>, args: &Value) -> Value {
    let locale = host.locale;
    let fail = |error: String| json!({ "ok": false, "error": error });
    let Some(pane_id) = text(args, "paneId").map(str::to_string) else { return Value::Null };
    let request = text(args, "text").unwrap_or("").trim().to_string();
    if request.is_empty() {
        return fail(locale.get("compare.picker.noRequest"));
    }
    let rows: Vec<CompareRow> = args
        .get("rows")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(|row| {
                    Some(CompareRow {
                        agent: row.get("agent")?.as_str()?.to_string(),
                        model: row.get("model").and_then(Value::as_str).map(str::to_string),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let (pane, snapshot, plan_first, attachments) = {
        let view = host.view();
        let Some(pane) = view.snapshot.session(&pane_id).cloned() else { return Value::Null };
        let ui = view.panes.get(&pane_id);
        let plan_first = ui.is_some_and(|ui| ui.view.launches_in_plan_mode);
        let attachments: Vec<RunAttachment> = ui.map(|ui| ui.view.attachments.clone()).unwrap_or_default();
        (pane, view.snapshot.clone(), plan_first, attachments)
    };
    if let Some(reason) = unavailable(host, &snapshot, &pane) {
        return fail(reason);
    }
    let Some(app) = crate::delegation::app() else { return fail(locale.get("delegation.human.error.switchedOff")) };
    let Some(folder) = pane_folder(&snapshot, &pane) else { return fail(locale.get("compare.reason.notAgent")) };
    let open = mighty_delegation::sidebar::open_human_child_count(pane.id(), &app.rows());
    let choices = app.worktree_child_choices(&pane, plan_first);
    let rows = match rules::checked_rows(&rows, &choices, pane.provider(), open) {
        Ok(rows) => rows,
        Err(CompareRowsError::TooFew) => return fail(locale.get("compare.error.rows")),
        Err(CompareRowsError::TooMany(most)) => return fail(locale.get_with("compare.error.tooMany", &[("count", &most.to_string())])),
        Err(CompareRowsError::Unavailable(agent)) => {
            return fail(locale.get_with("compare.error.unavailable", &[("agent", mighty_composer::runtime::provider_label(&agent))]));
        }
    };
    // The budget: a rough guess from the ledger, asked about once (the picker showed it too).
    let (records, spent, budget) = crate::usage_report::budget_reading(host);
    let estimates: Vec<f64> = rows.iter().map(|row| rules::estimate_run_cost(&records, &row.agent, row.model.as_deref())).collect();
    if let Some(warning) = rules::budget_warning(spent, budget, &estimates)
        && !flag(args, "budgetConfirmed")
    {
        return json!({ "ok": false, "budget": budget_text(&locale, &warning) });
    }
    // The base: the one chosen, else the head now, so every child starts from the same commit.
    let base = match text(args, "base").map(str::trim).filter(|b| !b.is_empty()) {
        Some(base) => base.to_string(),
        None => match git_lines(&folder, &["rev-parse", "HEAD"]).await.and_then(|l| l.into_iter().next()) {
            Some(head) => head,
            None => return fail(locale.get("delegation.reason.notGit")),
        },
    };
    let branch = git_lines(&folder, &["symbolic-ref", "--short", "-q", "HEAD"]).await.and_then(|l| l.into_iter().next());
    let test = text(args, "test").map(str::to_string).filter(|t| t.starts_with("run:") || t.starts_with("mighty:"));
    let test_name = test.as_deref().and_then(|t| t.split_once(':')).map(|(_, name)| name.to_string());
    let port_count = port_count(host, &snapshot, &pane).await;

    // The group first, so a child's pane that comes before the answer finds it.
    let group = wire::new_id();
    let run = CompareRun {
        id: group.clone(),
        request: request.chars().take(COMPARE_REQUEST_LIMIT).collect(),
        base_commit: base.clone(),
        base_branch: branch,
        test,
        test_name,
        created_at: Some(time::now_o()),
        children: rows
            .iter()
            .map(|row| CompareChild {
                id: wire::new_id(),
                provider: row.agent.clone(),
                model: row.model.clone(),
                ..CompareChild::default()
            })
            .collect(),
    };
    let placeholders: Vec<String> = run.children.iter().map(|c| c.id.clone()).collect();
    let saved = {
        let mut saved = false;
        let _ = host.view().update(|state| {
            if let Some(session) = state.sessions_mut().iter_mut().find(|s| s.id() == pane_id) {
                let mut next = RunSession::clone(session);
                next.compare_runs.get_or_insert_with(Vec::new).push(run);
                *session = Arc::new(next);
                saved = true;
            }
        });
        saved
    };
    if !saved {
        return Value::Null;
    }

    let (mut started, mut failed) = (0usize, 0usize);
    for (index, row) in rows.iter().enumerate() {
        let placeholder = placeholders[index].clone();
        let extras =
            HumanChildExtras { base_commit: Some(base.clone()), prepare: Some(prepare_child(host, &pane_id, &group, row, port_count)) };
        let failed_call = child_failed(host, &pane_id, &group);
        let answer = app
            .start_human_child_with(&pane_id, &request, attachments.clone(), plan_first, failed_call, Some(&row.agent), false, extras)
            .await;
        match answer {
            HumanChildStart::Started(child) => {
                started += 1;
                let id = child.id.clone();
                update_child(host, &pane_id, &group, &placeholder, |entry| {
                    entry.id = id;
                    entry.started_at = Some(time::now_o());
                });
                // A pane prepared before the answer kept its block on its own link: the column reads it from there too.
                if let Some(member) = member_of(host, &child.id) {
                    update_child(host, &pane_id, &group, &child.id, |entry| {
                        entry.port_base = member.port_base;
                        entry.port_count = member.port_count;
                    });
                }
            }
            HumanChildStart::Refused(reason) => {
                failed += 1;
                let why = mighty_delegation::text::reason(reason, &locale);
                update_child(host, &pane_id, &group, &placeholder, |entry| entry.failure = Some(why));
            }
            HumanChildStart::Failed(message) => {
                failed += 1;
                update_child(host, &pane_id, &group, &placeholder, |entry| entry.failure = Some(message));
            }
        }
    }
    if started == 0 {
        return json!({ "ok": false, "error": locale.get("compare.error.noneStarted"), "compareId": group });
    }
    // The words went to the children: the composer is left empty, as after a send.
    {
        let mut view = host.view();
        if let Ok(ui) = crate::panes::ui(&mut view, &pane_id) {
            ui.view.draft.clear();
            ui.view.attachments.clear();
            ui.dirty = true;
        }
    }
    if let Some(runs) = host.runs() {
        let _ = runs.service.update_draft(&pane_id, "");
    }
    let agents: Vec<&str> = rows.iter().map(|row| mighty_composer::runtime::provider_label(&row.agent)).collect();
    let short: String = base.chars().take(7).collect();
    crate::pane_worktree::append_system_note(
        host,
        &pane_id,
        locale.get_with("compare.log.started", &[("agents", &agents.join(", ")), ("base", &short)]),
    );
    host.view().sync();
    host.shell_dirty();
    host.schedule();
    json!({ "ok": true, "compareId": group, "started": started, "failed": failed })
}

/// The budget warning in the app's words.
fn budget_text(locale: &mighty_core::locale::Locale, warning: &rules::BudgetWarning) -> String {
    locale.get_with(
        "compare.budget.warn",
        &[("spent", &money(warning.spent)), ("estimate", &money(warning.estimate)), ("budget", &money(warning.budget))],
    )
}

/// The size of each child's port block: what the approved `.mighty.yaml` asks for, 10 without one.
async fn port_count(host: &Arc<Host>, snapshot: &AppSnapshot, pane: &RunSession) -> u16 {
    let Some(workspace) = snapshot.workspace(pane.workspace_id()).cloned() else { return 0 };
    let config = match crate::repo_tasks::load_workspace_config(host, &workspace).await {
        Ok(Some(status)) if status.approved => Some(status.loaded),
        _ => None,
    };
    crate::worktree_lifecycle::port_count(config.as_ref())
}

/// What happens to a child's pane once it exists, before its first run: its model (the row's), its compare link with its own port
/// block (assigned in `ports.json` as a worktree's).
fn prepare_child(host: &Arc<Host>, parent: &str, group: &str, row: &CompareRow, port_count: u16) -> Arc<dyn Fn(String) + Send + Sync> {
    let (host, parent, group, model) = (host.clone(), parent.to_string(), group.to_string(), row.model.clone());
    Arc::new(move |child: String| {
        let block = crate::pane_worktree::child_record(&child).and_then(|mut record| {
            (port_count > 0).then(|| {
                crate::worktree_lifecycle::assign_worktree(&host, &mut record, port_count);
                record.port_block()
            })?
        });
        let member = CompareMember {
            group: group.clone(),
            parent_id: parent.clone(),
            port_base: block.map(|(base, _)| base),
            port_count: block.map(|(_, count)| count),
        };
        let model = model.clone();
        let _ = host.view().update(|state| {
            if let Some(session) = state.sessions_mut().iter_mut().find(|s| s.id() == child) {
                let mut next = RunSession::clone(session);
                if let Some(model) = model {
                    next.model = Some(model);
                }
                next.compare_member = Some(member);
                *session = Arc::new(next);
            }
        });
        update_child(&host, &parent, &group, &child, |entry| {
            entry.port_base = block.map(|(base, _)| base);
            entry.port_count = block.map(|(_, count)| count);
        });
    })
}

/// A child whose worktree, pane or first run could not be made after its start was answered: its column says so.
fn child_failed(host: &Arc<Host>, parent: &str, group: &str) -> Arc<dyn Fn(String) -> BoxFuture<'static, ()> + Send + Sync> {
    let (host, parent, group) = (host.clone(), parent.to_string(), group.to_string());
    Arc::new(move |child: String| {
        let (host, parent, group) = (host.clone(), parent.clone(), group.clone());
        Box::pin(async move {
            let why = mighty_run::strings::get("delegation.human.error.startFailed");
            update_child(&host, &parent, &group, &child, |entry| entry.failure = Some(why));
        })
    })
}

// ── the compare view ────────────────────────────────────────────────────────────────────────────────────────────────────

/// Seconds between two RFC 3339 times (`to` None: now).
fn elapsed_seconds(from: Option<&str>, to: Option<&str>) -> Option<i64> {
    let parse = |t: &str| chrono::DateTime::parse_from_rfc3339(t).ok().map(|d| d.timestamp());
    let start = parse(from?)?;
    let end = match to {
        Some(to) => parse(to)?,
        None => chrono::Utc::now().timestamp(),
    };
    Some((end - start).max(0))
}

/// `compareView{paneId, compareId}`: the group with one column per child, git read now.
async fn view(host: &Arc<Host>, pane: &str, group: &str) -> Value {
    let (run, panes) = {
        let view = host.view();
        let Some(run) = group_of(&view.snapshot, pane, group) else { return json!({ "gone": true }) };
        let panes: HashMap<String, (bool, Option<CompareMember>)> = run
            .children
            .iter()
            .filter_map(|c| view.snapshot.session(&c.id).map(|s| (c.id.clone(), (true, s.compare_member.clone()))))
            .collect();
        (run, panes)
    };
    let states = child_states();
    let records: HashMap<String, mighty_delegation::model::ChildRecord> = crate::delegation::app()
        .and_then(|app| app.coordinator())
        .map(|coordinator| {
            let file = coordinator.file();
            run.children.iter().filter_map(|c| file.children.iter().find(|r| r.id == c.id).map(|r| (c.id.clone(), r.clone()))).collect()
        })
        .unwrap_or_default();
    let git = crate::pane_worktree::places().map(|places| places.git);
    let mut columns = vec![];
    for child in &run.children {
        let state = states.get(&child.id).copied();
        let phase = rules::phase(child, state);
        let record = records.get(&child.id);
        let stat = match (record, &git) {
            (Some(record), Some(git)) => {
                let checkout = record.parent_checkout.clone().unwrap_or_else(|| record.worktree_path.clone());
                rules::shortstat(git, &checkout, &record.base_commit, &record.branch).await
            }
            _ => None,
        };
        let member = panes.get(&child.id).and_then(|(_, m)| m.clone());
        let (port_base, port_count) = match (child.port_base, child.port_count) {
            (Some(base), Some(count)) => (Some(base), Some(count)),
            _ => (member.as_ref().and_then(|m| m.port_base), member.as_ref().and_then(|m| m.port_count)),
        };
        let open = panes.get(&child.id).is_some_and(|(open, _)| *open);
        columns.push(json!({
            "id": child.id,
            "agent": child.provider,
            "agentLabel": mighty_composer::runtime::provider_label(&child.provider),
            "model": child.model,
            "phase": phase.word(),
            "failure": child.failure,
            "files": stat.map(|s| s.files),
            "added": stat.map(|s| s.insertions),
            "removed": stat.map(|s| s.deletions),
            "elapsed": elapsed_seconds(child.started_at.as_deref(), child.finished_at.as_deref()),
            "running": !phase.finished(),
            "costUsd": child.cost_usd,
            "portBase": port_base,
            "portCount": port_count,
            "test": child.test_state.as_ref().map(|state| json!({ "state": state, "exitCode": child.test_exit_code, "output": child.test_output })),
            "canOpen": open,
            "canDiff": record.is_some(),
            "canMerge": rules::can_merge(child, state),
            "canDiscard": state.is_some_and(|s| s != ChildState::Discarded),
            "branch": record.map(|r| r.branch.clone()),
        }));
    }
    json!({
        "id": run.id,
        "request": run.request,
        "baseCommit": run.base_commit,
        "baseShort": run.base_commit.chars().take(7).collect::<String>(),
        "baseBranch": run.base_branch,
        "test": run.test,
        "testName": run.test_name,
        "createdAt": run.created_at,
        "children": columns,
    })
}

/// `compareAction{paneId, compareId, childId?, action}`: `diff` (the changes sheet), `merge` (then the other children's discard
/// asked), `discard`, `dismiss` (the group's card goes; its children stay as they are). Answers null.
async fn action_on(host: &Arc<Host>, pane: &str, group: &str, child: Option<&str>, action: &str) -> Value {
    let Some(run) = group_of(&host.view().snapshot, pane, group) else { return Value::Null };
    match (action, child) {
        ("dismiss", _) => {
            let _ = host.view().update(|state| {
                if let Some(session) = state.sessions_mut().iter_mut().find(|s| s.id() == pane) {
                    let mut next = RunSession::clone(session);
                    if let Some(runs) = next.compare_runs.as_mut() {
                        runs.retain(|r| r.id != group);
                    }
                    *session = Arc::new(next);
                }
            });
            host.shell_dirty();
            crate::phone::changed(host);
        }
        ("diff", Some(child)) if run.children.iter().any(|c| c.id == child) => crate::pane_worktree::show_child_changes(host, child).await,
        ("discard", Some(child)) if run.children.iter().any(|c| c.id == child) => {
            crate::delegation::compare_card_action(host, child, false).await;
        }
        ("merge", Some(child)) if run.children.iter().any(|c| c.id == child) => merge(host, &run, child).await,
        _ => {}
    }
    host.schedule();
    Value::Null
}

/// A child's merge from the compare view: only a reported one, through the card's merge (and 머지 전 점검); once it went in, the
/// other children still open are offered for discard, all at once.
async fn merge(host: &Arc<Host>, run: &CompareRun, child: &str) {
    let locale = host.locale;
    let states = child_states();
    let Some(entry) = run.children.iter().find(|c| c.id == child) else { return };
    if !rules::can_merge(entry, states.get(child).copied()) {
        crate::pane_worktree::banner(host, locale.get("compare.action.mergeHelp"));
        return;
    }
    if crate::delegation::compare_card_action(host, child, true).await != Some(DelegationCardResult::Done) {
        return;
    }
    let rest = rules::rest_to_discard(run, child, &child_states());
    if rest.is_empty() || host.closing() {
        return;
    }
    let names: Vec<String> = rest
        .iter()
        .filter_map(|id| run.children.iter().find(|c| &c.id == id))
        .map(|c| {
            let label = mighty_composer::runtime::provider_label(&c.provider);
            c.model.as_deref().map_or_else(|| label.to_string(), |m| format!("{label} {m}"))
        })
        .collect();
    let dialog = json!({
        "type": "confirm", "title": locale.get("compare.rest.title"),
        "message": locale.get_with("compare.rest.message", &[
            ("agent", mighty_composer::runtime::provider_label(&entry.provider)),
            ("count", &rest.len().to_string()),
            ("names", &names.join(", ")),
        ]),
        "primary": locale.get("compare.rest.confirm"), "close": locale.get("compare.rest.later"),
        "defaultPrimary": false, "destructive": true,
    });
    if crate::delegation::ask_confirm(host, dialog).await && !host.closing() {
        crate::delegation::discard_confirmed(host, &rest).await;
    }
}

// ── the phone ───────────────────────────────────────────────────────────────────────────────────────────────────────────

/// The longest end of a child's test output the phone gets.
const PHONE_TEST_OUTPUT: usize = 600;

/// The phone's `GET /m1/sessions/{id}/compare/{compareId}`: the compare view's group read now (each child's agent, model, phase,
/// shortstat, time, cost, test result, what may be done), the test output cut to its end. `{ gone: true }` once the group is gone.
pub async fn phone_view(host: &Arc<Host>, pane: &str, group: &str) -> Value {
    check_groups(host);
    let mut value = view(host, pane, group).await;
    if let Some(children) = value.get_mut("children").and_then(Value::as_array_mut) {
        for child in children {
            if let Some(output) = child.pointer_mut("/test/output")
                && let Some(text) = output.as_str()
            {
                let count = text.chars().count();
                let end: String = text.chars().skip(count.saturating_sub(PHONE_TEST_OUTPUT)).collect();
                *output = Value::String(end);
            }
            if let Some(child) = child.as_object_mut() {
                child.remove("branch");
            }
        }
    }
    value
}

/// The phone's `POST /m1/sessions/{id}/compare/{compareId}` (the compare view's actions, without the window's sheets — the phone asked
/// first): `merge` a reported child (what the pre-merge checks still warn about answers `confirm { warnings }` until asked again with
/// `confirmed`; once in, `rest` lists the other children still open, for the phone to offer their discard), `discard` one child, or
/// `discardRest` the children `ids` of that list. `{ done, message?, confirm?, rest? }`. Err: the status and why.
pub async fn phone_action(
    host: &Arc<Host>,
    pane: &str,
    group: &str,
    child: Option<&str>,
    action: &str,
    ids: &[String],
    confirmed: bool,
) -> Result<Value, (u16, String)> {
    let locale = host.locale;
    let Some(run) = group_of(&host.view().snapshot, pane, group) else { return Err((404, locale.get("compare.view.gone"))) };
    let in_group = |id: &str| run.children.iter().any(|c| c.id == id);
    match action {
        "merge" => {
            let Some(child) = child.filter(|c| in_group(c)) else { return Err((404, locale.get("compare.view.gone"))) };
            let states = child_states();
            let Some(entry) = run.children.iter().find(|c| c.id == child) else { return Err((404, locale.get("compare.view.gone"))) };
            if !rules::can_merge(entry, states.get(child).copied()) {
                return Ok(json!({ "done": false, "message": locale.get("compare.action.mergeHelp") }));
            }
            if !confirmed && let Some(record) = crate::pane_worktree::child_record(child) {
                let target = crate::merge_checks::target_of(child, &record);
                let warnings = crate::merge_checks::merge_warnings(host, &target, false).await;
                if !warnings.is_empty() {
                    let lines = crate::merge_checks::warning_lines(&locale, &warnings).join("\n");
                    let message =
                        format!("{}\n\n{}\n\n{lines}", locale.get("git.checks.merge.title"), locale.get("git.checks.merge.message"));
                    return Ok(json!({ "done": false, "message": message, "confirm": { "warnings": warnings } }));
                }
            }
            let words = match crate::delegation::phone_merge_child(host, child).await {
                crate::delegation::PhoneCard::Done(words) => words,
                crate::delegation::PhoneCard::Refused(why) => return Ok(json!({ "done": false, "message": why })),
            };
            let rest: Vec<Value> = rules::rest_to_discard(&run, child, &child_states())
                .iter()
                .filter_map(|id| run.children.iter().find(|c| &c.id == id))
                .map(|c| {
                    let label = mighty_composer::runtime::provider_label(&c.provider);
                    let name = c.model.as_deref().map_or_else(|| label.to_string(), |m| format!("{label} {m}"));
                    json!({ "id": c.id, "label": name })
                })
                .collect();
            host.schedule();
            Ok(json!({ "done": true, "message": words, "rest": rest }))
        }
        "discard" => {
            let Some(child) = child.filter(|c| in_group(c)) else { return Err((404, locale.get("compare.view.gone"))) };
            Ok(discarded(crate::delegation::phone_discard_children(host, &[child.to_string()]).await))
        }
        "discardRest" => {
            // Only children of this group that are still open: the list the merge answered, as it stands now.
            let open = child_states();
            let chosen: Vec<String> = ids
                .iter()
                .filter(|id| in_group(id))
                .filter(|id| open.get(id.as_str()).is_some_and(|s| !matches!(s, ChildState::Merged | ChildState::Discarded)))
                .cloned()
                .collect();
            if chosen.is_empty() {
                return Ok(json!({ "done": true }));
            }
            Ok(discarded(crate::delegation::phone_discard_children(host, &chosen).await))
        }
        _ => Err((400, "Unknown compare action.".into())),
    }
}

fn discarded(card: crate::delegation::PhoneCard) -> Value {
    match card {
        crate::delegation::PhoneCard::Done(words) => json!({ "done": true, "message": words }),
        crate::delegation::PhoneCard::Refused(why) => json!({ "done": false, "message": why }),
    }
}

// ── the sidebar's cards ─────────────────────────────────────────────────────────────────────────────────────────────────

/// The frame model's compare cards (host.rs' `shell_model_with`, the view locked): each pane's groups summed up from delegation's
/// rows.
pub fn decorate(snapshot: &AppSnapshot, model: &mut ShellModel) {
    if !snapshot.sessions().iter().any(|s| s.compare_runs.as_ref().is_some_and(|runs| !runs.is_empty())) {
        return;
    }
    let states = child_states();
    for pane in &mut model.panes {
        let Some(runs) = snapshot.session(&pane.id).and_then(|s| s.compare_runs.as_ref()) else { continue };
        pane.compares = runs.iter().rev().map(|run| rules::summary(run, &states)).collect();
    }
}

// ── costs, finishes and tests ───────────────────────────────────────────────────────────────────────────────────────────

/// A finished run's cost as the usage ledger recorded it (usage_report.rs): a compare child adds it to its column.
pub fn add_cost(host: &Arc<Host>, session_id: &str, cost: f64) {
    if !cost.is_finite() || cost <= 0.0 {
        return;
    }
    let Some(member) = member_of(host, session_id) else { return };
    update_child(host, &member.parent_id, &member.group, session_id, |entry| {
        entry.cost_usd = Some(entry.cost_usd.unwrap_or(0.0) + cost);
    });
}

/// Delegation's state changed (delegation.rs' `changed`): the groups are looked at once, soon, off the caller's stack.
pub fn delegation_changed(host: &Arc<Host>) {
    if CHECK_PENDING.swap(true, Ordering::AcqRel) {
        return;
    }
    let host = host.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        CHECK_PENDING.store(false, Ordering::Release);
        check_groups(&host);
    });
}

/// Every group: a child seen done for the first time gets its finish time; a child that reported with a test chosen and not run gets
/// it started.
fn check_groups(host: &Arc<Host>) {
    if host.closing() {
        return;
    }
    let groups: Vec<(String, CompareRun)> = {
        let view = host.view();
        view.snapshot.sessions().iter().flat_map(|s| s.compare_runs.iter().flatten().map(|run| (s.id().to_string(), run.clone()))).collect()
    };
    if groups.is_empty() {
        return;
    }
    let states = child_states();
    for (pane, run) in groups {
        for child in &run.children {
            let state = states.get(&child.id).copied();
            let phase = rules::phase(child, state);
            if phase.finished() && child.finished_at.is_none() && child.started_at.is_some() {
                update_child(host, &pane, &run.id, &child.id, |entry| entry.finished_at = Some(time::now_o()));
            }
            if rules::test_due(&run, child, state) && phase == ComparePhase::Reported {
                start_test(host, &pane, &run, child);
            }
        }
    }
}

/// The chosen test in the child's worktree, once: its state goes to the column as it starts and ends.
fn start_test(host: &Arc<Host>, pane: &str, run: &CompareRun, child: &CompareChild) {
    let key = format!("{}/{}", run.id, child.id);
    if !lock(&STATE).testing.insert(key.clone()) {
        return;
    }
    let (host, pane, group, child_id, test) = (host.clone(), pane.to_string(), run.id.clone(), child.id.clone(), run.test.clone());
    update_child(&host, &pane, &group, &child_id, |entry| entry.test_state = Some(CompareChild::TEST_RUNNING.into()));
    tauri::async_runtime::spawn(async move {
        let outcome = run_test(&host, &pane, &child_id, test.as_deref().unwrap_or("")).await;
        let (passed, code, output) = outcome;
        update_child(&host, &pane, &group, &child_id, |entry| {
            entry.test_state = Some(if passed { CompareChild::TEST_PASSED } else { CompareChild::TEST_FAILED }.into());
            entry.test_exit_code = code;
            let count = output.chars().count();
            entry.test_output = Some(output.chars().skip(count.saturating_sub(COMPARE_TEST_OUTPUT_LIMIT)).collect());
        });
        lock(&STATE).testing.remove(&format!("{group}/{child_id}"));
    });
}

/// Runs the task `test` (`run:<name>` or `mighty:<name>`) in the child's folder with its port block: passed, exit code, output.
/// Nothing runs before the repository's config is approved, in verification mode, or where delegation may not go.
async fn run_test(host: &Arc<Host>, pane: &str, child: &str, test: &str) -> (bool, Option<i32>, String) {
    let locale = host.locale;
    let found = {
        let view = host.view();
        let child_pane = view.snapshot.session(child).cloned();
        let workspace = view.snapshot.session(pane).and_then(|p| view.snapshot.workspace(p.workspace_id()).cloned());
        child_pane.zip(workspace)
    };
    let Some((child_pane, workspace)) = found else { return (false, None, locale.get("compare.test.gone")) };
    let Some(folder) = child_pane.working_folder.clone().filter(|f| Path::new(f).is_dir()) else {
        return (false, None, locale.get("compare.test.gone"));
    };
    if !host.delegation_live() || !crate::delegation::may_touch(host, Path::new(&folder)) {
        return (false, None, crate::setup::verification_mode());
    }
    let (target, workspace_folder) = (PathBuf::from(&folder), PathBuf::from(workspace.path()));
    let Ok((config, tasks)) =
        tauri::async_runtime::spawn_blocking(move || crate::repo_tasks::scan_folder_tasks(&target, &workspace_folder)).await
    else {
        return (false, None, locale.get("compare.test.gone"));
    };
    let name = test.split_once(':').map_or(test, |(_, name)| name);
    let Some(task) = tasks.into_iter().find(|task| task.id == test) else {
        return (false, None, locale.get_with("compare.test.missing", &[("name", name)]));
    };
    let Some(loaded) = config.ok().flatten().filter(|c| crate::repo_tasks::is_config_approved(host, workspace.path(), &c.content_hash))
    else {
        return (false, None, locale.get("compare.test.notApproved"));
    };
    let cwd = match crate::repo_tasks::resolve_task_cwd(Path::new(&folder), task.cwd.as_deref()) {
        Ok(cwd) => cwd,
        Err(error) => return (false, None, error),
    };
    let mut request = crate::repo_tasks::shell_request(&task.command, cwd.to_string_lossy().into_owned());
    for (key, value) in crate::repo_tasks::login_shell_env(host, Some(Path::new(workspace.path()))).await {
        request = request.env(key, value);
    }
    for (key, value) in &loaded.config.worktree.env {
        request = request.env(key.clone(), value.clone());
    }
    if let Some(values) = child_pane.compare_member.as_ref().and_then(|m| m.environment(&folder, workspace.path())) {
        for (key, value) in values {
            request = request.env(key, value);
        }
    }
    let outcome = mighty_delegation::run_actions::run_bounded(host.processes.as_ref(), request, TEST_TIMEOUT).await;
    let mut output = crate::repo_tasks::strip_ansi_and_controls(&outcome.output);
    if outcome.timed_out {
        output.push_str(&format!("\n{}", locale.get("compare.test.timedOut")));
    }
    if let Some(error) = &outcome.error {
        output.push_str(&format!("\n{error}"));
    }
    (outcome.succeeded(), outcome.exit_code, output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elapsed_time_counts_from_the_start_to_the_finish() {
        assert_eq!(elapsed_seconds(Some("2026-11-23T10:00:00+00:00"), Some("2026-11-23T10:02:05+00:00")), Some(125));
        assert_eq!(elapsed_seconds(Some("2026-11-23T10:00:00+00:00"), Some("2026-11-23T09:00:00+00:00")), Some(0));
        assert_eq!(elapsed_seconds(None, None), None);
        assert!(elapsed_seconds(Some("2020-01-01T00:00:00Z"), None).is_some_and(|s| s > 0));
    }

    #[test]
    fn a_group_is_saved_with_its_pane_and_restored_after_a_restart() {
        use mighty_core::json::Wire;
        let run = CompareRun {
            id: "g1".into(),
            request: "Fix it".into(),
            base_commit: "0123456789abcdef0123456789abcdef01234567".into(),
            base_branch: Some("main".into()),
            test: Some("run:test".into()),
            test_name: Some("test".into()),
            created_at: Some("2026-11-23T10:00:00Z".into()),
            children: vec![
                CompareChild {
                    id: "c1".into(),
                    provider: "claude".into(),
                    started_at: Some("2026-11-23T10:00:00Z".into()),
                    ..CompareChild::default()
                },
                CompareChild {
                    id: "c2".into(),
                    provider: "claude".into(),
                    model: Some("opus".into()),
                    test_state: Some("running".into()),
                    ..CompareChild::default()
                },
                CompareChild {
                    id: "c3".into(),
                    provider: "codex".into(),
                    failure: Some("Codex is signed out.".into()),
                    ..CompareChild::default()
                },
            ],
        };
        let pane = RunSession {
            id: Some("p".into()),
            workspace_id: Some("w".into()),
            kind: Some("claude".into()),
            provider: Some("claude".into()),
            compare_runs: Some(vec![run.clone()]),
            ..RunSession::default()
        };
        let text = pane.to_json();
        let read = RunSession::read(&serde_json::from_str(&text).unwrap()).unwrap();
        assert_eq!(read.compare_runs.as_ref(), Some(&vec![run.clone()]), "written and read back as it was");
        let restored = mighty_core::normalize::compare_runs(read.compare_runs.as_ref(), true).unwrap();
        assert_eq!(restored[0].children.len(), 3, "a repeated agent and a failed row are kept");
        assert_eq!(restored[0].children[1].test_state, None, "a test that was running when the app closed can run again");
        assert_eq!(restored[0].children[2].failure.as_deref(), Some("Codex is signed out."));
        let live = mighty_core::normalize::compare_runs(read.compare_runs.as_ref(), false).unwrap();
        assert_eq!(live[0].children[1].test_state.as_deref(), Some("running"));
    }

    #[test]
    fn a_childs_link_gives_its_runs_its_port_block() {
        let member = CompareMember { group: "g".into(), parent_id: "p".into(), port_base: Some(41_020), port_count: Some(10) };
        let env: HashMap<String, String> = member.environment("/w/.mightycode/worktrees/c1", "/repo").unwrap().into_iter().collect();
        assert_eq!(env["PORT"], "41020");
        assert_eq!(env["MIGHTY_PORT"], "41020");
        assert_eq!(env["MIGHTY_PORT_COUNT"], "10");
        assert_eq!(env["MIGHTY_WORKTREE_NAME"], "c1");
        assert!(CompareMember { port_base: None, ..member.clone() }.environment("/x", "/repo").is_none());
        assert_eq!(mighty_core::normalize::compare_member(Some(&member), Some("p")), Some(member.clone()));
        assert_eq!(mighty_core::normalize::compare_member(Some(&member), Some("other")), None, "only on its parent's child");
        assert_eq!(mighty_core::normalize::compare_member(Some(&member), None), None);
    }
}
