//! 비교 실행 (MC-29, after the 0.4 Swift app's `CompareRun.swift`): one request handed to several agents — or one agent several times
//! ("3× Claude") — at once, each in its own child worktree from the same base commit, their results set side by side so one is merged
//! and the rest discarded. Built on the children the human starts ([`crate::app::DelegationApp::start_human_child_with`]), so the
//! cards' merge, undo and discard keep working; this only picks who runs, reads what each changed and sums the group up. The host
//! (src-tauri `compare.rs`) keeps the group on the parent pane (`RunSession.compare_runs`), runs the chosen test and draws it.

use std::collections::HashMap;

use mighty_core::model::{COMPARE_MAXIMUM_ROWS, CompareChild, CompareRun};
use mighty_core::shell::{ChildState, PaneCompare};
use mighty_core::usage_ledger::UsageRecord;

use crate::git::worktree::ChildWorktreeBase;
use crate::git::{GitRunner, is_object_name, line, run};
use crate::host::DelegationAgentAvailability;
use crate::model::ChildStates as _;
use crate::providers::DelegationChildChoice;

// ── the rows ─────────────────────────────────────────────────────────────────────────────────────────────────────────────

/// One picked row: the agent and the model its child runs (None: the model a new pane of that agent would run).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompareRow {
    pub agent: String,
    pub model: Option<String>,
}

/// Why the picked rows cannot start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompareRowsError {
    /// Fewer than two rows: nothing to compare.
    TooFew,
    /// More rows than [`max_rows`] allows now (the room left under the compare runs' cap of four).
    TooMany(usize),
    /// An agent that cannot start a child now (not installed, signed out, no mode, or unknown).
    Unavailable(String),
}

/// How many children a compare run may start now: the room left under the compare runs' cap of four
/// ([`crate::model::ChildKind::Compare`]), the parent's compare children still open (`open_compare_children`, of every group of its
/// own) counted. MC-45: counted apart from the three children the human opens from the composer and the three its `delegate` makes,
/// so neither kind takes a compare run's room nor the other way round.
pub fn max_rows(open_compare_children: usize) -> usize {
    COMPARE_MAXIMUM_ROWS.saturating_sub(open_compare_children)
}

/// The agents a row may pick: those that can start a child now, the parent's own agent first.
pub fn ready_agents(choices: &[DelegationChildChoice], parent: &str) -> Vec<String> {
    let ready: Vec<&DelegationChildChoice> = choices.iter().filter(|choice| choice.blocked.is_none()).collect();
    ready.iter().filter(|c| c.agent == parent).chain(ready.iter().filter(|c| c.agent != parent)).map(|c| c.agent.clone()).collect()
}

/// Checks the picked rows against what can start now: two to [`max_rows`] of them, each of an agent [`ready_agents`] offers. The same
/// agent may come more than once, each its own child. A model is kept only when it is a model name (`default` is none).
pub fn checked_rows(
    rows: &[CompareRow],
    choices: &[DelegationChildChoice],
    parent: &str,
    open_compare_children: usize,
) -> Result<Vec<CompareRow>, CompareRowsError> {
    if rows.len() < 2 {
        return Err(CompareRowsError::TooFew);
    }
    let most = max_rows(open_compare_children);
    if rows.len() > most {
        return Err(CompareRowsError::TooMany(most));
    }
    let ready = ready_agents(choices, parent);
    rows.iter()
        .map(|row| {
            if !ready.contains(&row.agent) {
                return Err(CompareRowsError::Unavailable(row.agent.clone()));
            }
            let model = row.model.as_deref().map(str::trim).filter(|m| !m.is_empty() && *m != "default" && mighty_core::wire::model(m));
            Ok(CompareRow { agent: row.agent.clone(), model: model.map(str::to_string) })
        })
        .collect()
}

// ── what a child changed ─────────────────────────────────────────────────────────────────────────────────────────────────

/// What one child changed against the commit it started from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CompareStat {
    pub files: u32,
    pub insertions: u32,
    pub deletions: u32,
}

/// Reads `git diff --shortstat` ("3 files changed, 10 insertions(+), 2 deletions(-)"); an empty output is no change.
pub fn parse_shortstat(text: &str) -> CompareStat {
    let number = |words: &[&str]| -> u32 {
        let tokens: Vec<&str> = text.split(|c: char| c.is_whitespace() || c == ',').filter(|t| !t.is_empty()).collect();
        tokens.windows(2).find(|pair| words.iter().any(|word| pair[1].starts_with(word))).and_then(|pair| pair[0].parse().ok()).unwrap_or(0)
    };
    CompareStat { files: number(&["file"]), insertions: number(&["insertion"]), deletions: number(&["deletion"]) }
}

/// What the child's `branch` changed since `base` (`git diff --shortstat <base>...<branch>`), read in `checkout`; None when git
/// cannot tell (a branch gone, git failing, an SSH machine that cannot be reached). MC-52: on the machine the checkout is on.
pub async fn shortstat<G: GitRunner + ?Sized>(git: &G, checkout: &str, base: &str, branch: &str) -> Option<CompareStat> {
    if base.is_empty() || branch.is_empty() || base.starts_with('-') || branch.starts_with('-') {
        return None;
    }
    let range = format!("{base}...{branch}");
    let result = run(git, &["diff", "--shortstat", &range], checkout, 10.0).await?;
    (result.exit_code == 0).then(|| parse_shortstat(line(&result.stdout)))
}

// ── the children's states ────────────────────────────────────────────────────────────────────────────────────────────────

/// A child's state as the compare view and the group card name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComparePhase {
    /// Its worktree, pane and first run are being made.
    Starting,
    Running,
    /// It reported: it can be merged.
    Reported,
    /// Its run ended without a report, it was interrupted or its pane closed.
    Ended,
    Failed,
    Merged,
    Discarded,
}

impl ComparePhase {
    pub fn word(self) -> &'static str {
        match self {
            ComparePhase::Starting => "starting",
            ComparePhase::Running => "running",
            ComparePhase::Reported => "reported",
            ComparePhase::Ended => "ended",
            ComparePhase::Failed => "failed",
            ComparePhase::Merged => "merged",
            ComparePhase::Discarded => "discarded",
        }
    }

    /// Done running: its elapsed time stops here.
    pub fn finished(self) -> bool {
        matches!(self, ComparePhase::Reported | ComparePhase::Ended | ComparePhase::Failed | ComparePhase::Merged | ComparePhase::Discarded)
    }
}

/// The phase of `child`, its delegation record's state (`state`, None when it has no record: one that never started failed, one that
/// started and lost its record was discarded and pruned since).
pub fn phase(child: &CompareChild, state: Option<ChildState>) -> ComparePhase {
    match state {
        _ if child.failure.is_some() && !matches!(state, Some(ChildState::Merged | ChildState::Discarded)) => ComparePhase::Failed,
        None if child.started_at.is_some() => ComparePhase::Discarded,
        None => ComparePhase::Failed,
        Some(ChildState::Creating) => ComparePhase::Starting,
        Some(ChildState::Running | ChildState::Waiting) => ComparePhase::Running,
        Some(ChildState::Reported) => ComparePhase::Reported,
        Some(ChildState::Ended | ChildState::Interrupted | ChildState::Closed) => ComparePhase::Ended,
        Some(ChildState::Failed) => ComparePhase::Failed,
        Some(ChildState::Merged) => ComparePhase::Merged,
        Some(ChildState::Discarded) => ComparePhase::Discarded,
    }
}

/// The group card's sums: its children by phase (starting counts as running).
pub fn summary(run: &CompareRun, states: &HashMap<String, ChildState>) -> PaneCompare {
    let mut card = PaneCompare {
        id: run.id.clone(),
        title: run.request.lines().find(|l| !l.trim().is_empty()).unwrap_or("").chars().take(80).collect(),
        total: run.children.len(),
        ..PaneCompare::default()
    };
    for child in &run.children {
        match phase(child, states.get(&child.id).copied()) {
            ComparePhase::Starting | ComparePhase::Running => card.running += 1,
            ComparePhase::Reported => card.reported += 1,
            ComparePhase::Ended | ComparePhase::Failed => card.failed += 1,
            ComparePhase::Merged => card.merged += 1,
            ComparePhase::Discarded => card.discarded += 1,
        }
    }
    card
}

/// Whether `child` may be merged from the compare view: it reported, by the cards' own rule (a running child's merge waits for its
/// report there; here only a reported one is offered).
pub fn can_merge(child: &CompareChild, state: Option<ChildState>) -> bool {
    phase(child, state) == ComparePhase::Reported
}

/// After `merged` went in: the other children still to discard — those with a record that is neither merged nor discarded.
pub fn rest_to_discard(run: &CompareRun, merged: &str, states: &HashMap<String, ChildState>) -> Vec<String> {
    run.children
        .iter()
        .filter(|child| child.id != merged)
        .filter(|child| states.get(&child.id).is_some_and(|state| !matches!(state, ChildState::Merged | ChildState::Discarded)))
        .map(|child| child.id.clone())
        .collect()
}

/// Whether the chosen test should start in `child` now: the run chose one, the child reported and the test has not run.
pub fn test_due(run: &CompareRun, child: &CompareChild, state: Option<ChildState>) -> bool {
    run.test.is_some() && child.test_state.is_none() && phase(child, state) == ComparePhase::Reported
}

// ── an SSH workspace's compare run (MC-52) ───────────────────────────────────────────────────────────────────────────────

/// MC-52: the agents a row may pick in an SSH workspace, with the mode each child runs in: the human children's rules
/// ([`crate::providers::human_child_choices`]: a mode in which the child can change files) for the agents whose CLI runs on an SSH
/// machine (mighty-run `ssh::supported_provider`). Their CLI is the machine's, so this computer's installs and sign-ins are not asked:
/// a CLI missing over there fails its child's first run, as any SSH pane's. MC-45: not the human children's cap of three; the room is
/// the compare runs' cap of four ([`max_rows`] of [`open_members`]), as a local pane's.
pub fn ssh_choices(parent: &str, parent_mode: &str, plan_first: bool) -> Vec<DelegationChildChoice> {
    crate::providers::human_child_choices(parent, parent_mode, plan_first, 0, |_| DelegationAgentAvailability::Ready)
        .into_iter()
        .filter(|choice| mighty_run::ssh::supported_provider(&choice.agent))
        .collect()
}

/// MC-52: where an SSH workspace's children start: the workspace's check (`start`, read on the machine) at the commit the person
/// chose, named by its full hash as that machine's git reads `<base>^{commit}`. Err with git's words when the machine does not have
/// it (or cannot be asked: no child is made then).
pub async fn start_at<G: GitRunner + ?Sized>(git: &G, start: &ChildWorktreeBase, base: &str) -> Result<ChildWorktreeBase, String> {
    let base = base.trim();
    if base.is_empty() || base.starts_with('-') || base.len() > 200 || base.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("The base commit cannot be named.".into());
    }
    let asked = format!("{base}^{{commit}}");
    let read = run(git, &["rev-parse", "--verify", "--quiet", "--end-of-options", &asked], &start.repository, 10.0)
        .await
        .ok_or_else(|| "Git did not finish.".to_string())?;
    let commit = line(&read.stdout).to_string();
    if read.exit_code != 0 || !is_object_name(&commit) {
        return Err(format!("The repository has no commit {base}."));
    }
    Ok(ChildWorktreeBase { base_commit: commit, ..start.clone() })
}

/// MC-52: what the app sees of an SSH workspace's child, a pane worktree on its machine (not a delegated child with a record).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemberSeen<'a> {
    /// Its pane's status (`idle` before its first run, `running`, `completed`, `stopped`, `error`).
    pub status: &'a str,
    /// Its worktree's merge went in (the record's merged commit).
    pub merged: bool,
}

/// MC-52: the state an SSH workspace's child has, as delegation's record would name it, so the compare view, the group card, the
/// merge and the discard of the rest read it as they read a local child's: what became of it ([`CompareChild::outcome`]) first,
/// then its pane (`seen`, None once the pane is gone).
pub fn member_state(child: &CompareChild, seen: Option<MemberSeen>) -> Option<ChildState> {
    match child.outcome.as_deref() {
        Some(CompareChild::MERGED) => return Some(ChildState::Merged),
        Some(CompareChild::DISCARDED) => return Some(ChildState::Discarded),
        _ => {}
    }
    let Some(seen) = seen else {
        // A pane that never started has no state (its column says why); one that started and is gone was closed (its worktree
        // went with it, or was kept on the workspace with its work).
        return child.started_at.as_ref().map(|_| ChildState::Closed);
    };
    if seen.merged {
        return Some(ChildState::Merged);
    }
    Some(match seen.status {
        "running" => ChildState::Running,
        "completed" => ChildState::Reported,
        "stopped" => ChildState::Interrupted,
        "error" | "failed" => ChildState::Failed,
        _ => ChildState::Creating,
    })
}

/// MC-52: an SSH workspace's open children of the pane, counted as a local pane's compare children are (an open state, its pane still
/// there: `alive`), so the room left under the compare runs' cap (MC-45: four, [`max_rows`]) is the same.
pub fn open_members(runs: &[CompareRun], states: &HashMap<String, ChildState>, alive: impl Fn(&str) -> bool) -> usize {
    runs.iter()
        .flat_map(|run| &run.children)
        .filter(|child| alive(&child.id) && states.get(&child.id).is_some_and(|state| state.is_open()))
        .count()
}

// ── the budget ───────────────────────────────────────────────────────────────────────────────────────────────────────────

/// What one child is guessed to cost when the ledger has no run of its agent to go by.
pub const FALLBACK_RUN_COST: f64 = 1.0;

/// A rough guess at what one child of `provider` (running `model`) costs: the average cost of one finished run in `records` (the
/// ledger's recent days) of the same agent and model, else of the same agent, else [`FALLBACK_RUN_COST`]. Each ledger record is one
/// finished run; a child usually takes one.
pub fn estimate_run_cost(records: &[UsageRecord], provider: &str, model: Option<&str>) -> f64 {
    let average = |matching: Vec<&UsageRecord>| -> Option<f64> {
        let costs: Vec<f64> = matching.iter().map(|r| r.cost_usd).filter(|c| c.is_finite() && *c > 0.0).collect();
        (!costs.is_empty()).then(|| costs.iter().sum::<f64>() / costs.len() as f64)
    };
    let of_agent: Vec<&UsageRecord> = records.iter().filter(|r| r.provider == provider).collect();
    model
        .and_then(|model| average(of_agent.iter().copied().filter(|r| r.model == model).collect()))
        .or_else(|| average(of_agent))
        .unwrap_or(FALLBACK_RUN_COST)
}

/// The warning before a start that would likely go over the daily budget: spent today, the run's estimate, and the budget.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BudgetWarning {
    pub spent: f64,
    pub estimate: f64,
    pub budget: f64,
}

/// Warns when a budget is set and what was spent today plus the estimates of the children would pass it.
pub fn budget_warning(spent_today: f64, budget: Option<f64>, estimates: &[f64]) -> Option<BudgetWarning> {
    let budget = budget.filter(|b| b.is_finite() && *b > 0.0)?;
    let spent = if spent_today.is_finite() { spent_today.max(0.0) } else { 0.0 };
    let estimate: f64 = estimates.iter().filter(|e| e.is_finite() && **e > 0.0).sum();
    (spent + estimate > budget).then_some(BudgetWarning { spent, estimate, budget })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::DelegationChildBlock;

    fn choice(agent: &str, blocked: Option<DelegationChildBlock>) -> DelegationChildChoice {
        DelegationChildChoice { agent: agent.into(), mode: Some("manual".into()), blocked }
    }

    fn row(agent: &str, model: Option<&str>) -> CompareRow {
        CompareRow { agent: agent.into(), model: model.map(str::to_string) }
    }

    fn child(id: &str, provider: &str) -> CompareChild {
        CompareChild { id: id.into(), provider: provider.into(), ..CompareChild::default() }
    }

    #[test]
    fn a_repeated_agent_is_one_child_per_row() {
        let choices = [choice("claude", None), choice("codex", Some(DelegationChildBlock::SignedOut)), choice("gemini", None)];
        assert_eq!(ready_agents(&choices, "gemini"), ["gemini", "claude"]);
        let rows = [row("claude", Some("opus")), row("claude", None), row("claude", Some("default"))];
        let checked = checked_rows(&rows, &choices, "claude", 0).unwrap();
        assert_eq!(checked, [row("claude", Some("opus")), row("claude", None), row("claude", None)]);
        assert_eq!(
            checked_rows(&[row("claude", None), row("codex", None)], &choices, "claude", 0),
            Err(CompareRowsError::Unavailable("codex".into()))
        );
        assert_eq!(checked_rows(&[row("claude", None)], &choices, "claude", 0), Err(CompareRowsError::TooFew));
    }

    /// MC-45: four rows, counted apart from the human children's three.
    #[test]
    fn the_rows_stay_within_four_and_the_room_left() {
        assert_eq!(COMPARE_MAXIMUM_ROWS, 4);
        assert_eq!(max_rows(0), 4, "four, not the human children's three");
        assert_eq!(max_rows(3), 1);
        assert_eq!(max_rows(6), 0);
        let choices = [choice("claude", None)];
        let rows = vec![row("claude", None); 5];
        assert_eq!(checked_rows(&rows[..4], &choices, "claude", 0).map(|r| r.len()), Ok(4), "four rows start");
        assert_eq!(checked_rows(&rows, &choices, "claude", 0), Err(CompareRowsError::TooMany(4)));
        assert_eq!(checked_rows(&rows[..2], &choices, "claude", 3), Err(CompareRowsError::TooMany(1)));
        assert_eq!(checked_rows(&rows[..2], &choices, "claude", 2).map(|r| r.len()), Ok(2), "two still open leave room for two");
    }

    #[test]
    fn shortstat_reads_files_and_lines() {
        assert_eq!(
            parse_shortstat(" 3 files changed, 10 insertions(+), 2 deletions(-)\n"),
            CompareStat { files: 3, insertions: 10, deletions: 2 }
        );
        assert_eq!(parse_shortstat(" 1 file changed, 1 insertion(+)"), CompareStat { files: 1, insertions: 1, deletions: 0 });
        assert_eq!(parse_shortstat(" 2 files changed, 7 deletions(-)"), CompareStat { files: 2, insertions: 0, deletions: 7 });
        assert_eq!(parse_shortstat(""), CompareStat::default());
    }

    #[test]
    fn one_failed_child_is_shown_failed_and_the_rest_go_on() {
        let mut run = CompareRun { id: "g".into(), request: "\nFix the bug\nmore".into(), ..CompareRun::default() };
        run.children = vec![
            child("a", "claude"),
            child("b", "codex"),
            CompareChild { failure: Some("Codex is signed out".into()), ..child("c", "codex") },
        ];
        let states = HashMap::from([("a".to_string(), ChildState::Running), ("b".to_string(), ChildState::Reported)]);
        assert_eq!(phase(&run.children[2], None), ComparePhase::Failed);
        assert_eq!(phase(&run.children[0], states.get("a").copied()), ComparePhase::Running);
        let card = summary(&run, &states);
        assert_eq!((card.title.as_str(), card.total, card.running, card.reported, card.failed), ("Fix the bug", 3, 1, 1, 1));
        assert!(can_merge(&run.children[1], states.get("b").copied()));
        assert!(!can_merge(&run.children[0], states.get("a").copied()));
        assert!(!can_merge(&run.children[2], None));
    }

    #[test]
    fn merging_one_leaves_the_others_to_discard() {
        let mut run = CompareRun { id: "g".into(), ..CompareRun::default() };
        run.children = vec![child("a", "claude"), child("b", "claude"), child("c", "codex"), child("d", "gemini")];
        let states = HashMap::from([
            ("a".to_string(), ChildState::Merged),
            ("b".to_string(), ChildState::Reported),
            ("c".to_string(), ChildState::Discarded),
        ]);
        // d never got a record (it failed to start): nothing of it to discard.
        assert_eq!(rest_to_discard(&run, "a", &states), ["b"]);
    }

    #[test]
    fn the_test_runs_once_after_the_report() {
        let mut run = CompareRun { id: "g".into(), test: Some("run:test".into()), ..CompareRun::default() };
        run.children = vec![child("a", "claude")];
        assert!(test_due(&run, &run.children[0], Some(ChildState::Reported)));
        assert!(!test_due(&run, &run.children[0], Some(ChildState::Running)));
        let ran = CompareChild { test_state: Some(CompareChild::TEST_PASSED.into()), ..child("a", "claude") };
        assert!(!test_due(&run, &ran, Some(ChildState::Reported)));
        run.test = None;
        assert!(!test_due(&run, &run.children[0], Some(ChildState::Reported)));
    }

    #[test]
    fn the_budget_warns_when_the_estimate_would_pass_it() {
        let record = |provider: &str, model: &str, cost: f64| UsageRecord {
            day: "2026-11-23".into(),
            workspace_id: "w".into(),
            provider: provider.into(),
            model: model.into(),
            cost_usd: cost,
            tokens: 1,
        };
        let records =
            [record("claude", "opus", 2.0), record("claude", "opus", 4.0), record("claude", "sonnet", 0.5), record("codex", "gpt", 0.0)];
        assert!((estimate_run_cost(&records, "claude", Some("opus")) - 3.0).abs() < 1e-9);
        assert!((estimate_run_cost(&records, "claude", None) - 6.5 / 3.0).abs() < 1e-9);
        assert!((estimate_run_cost(&records, "claude", Some("haiku")) - 6.5 / 3.0).abs() < 1e-9);
        // A run with no cost says nothing: the fallback.
        assert!((estimate_run_cost(&records, "codex", None) - FALLBACK_RUN_COST).abs() < 1e-9);
        assert_eq!(budget_warning(8.0, Some(10.0), &[1.0, 1.0]), None);
        assert_eq!(budget_warning(8.0, Some(10.0), &[1.5, 1.0]), Some(BudgetWarning { spent: 8.0, estimate: 2.5, budget: 10.0 }));
        assert_eq!(budget_warning(100.0, None, &[1.0]), None);
        assert_eq!(budget_warning(100.0, Some(0.0), &[1.0]), None);
    }

    /// MC-52: an SSH workspace's child reads as a delegated child would: its pane's run, then its merge or discard.
    #[test]
    fn an_ssh_child_reads_as_a_delegated_childs_state() {
        let started = CompareChild { started_at: Some("2026-11-24T10:00:00Z".into()), ..child("a", "claude") };
        let seen = |status| Some(MemberSeen { status, merged: false });
        assert_eq!(member_state(&started, seen("idle")), Some(ChildState::Creating));
        assert_eq!(member_state(&started, seen("running")), Some(ChildState::Running));
        assert_eq!(member_state(&started, seen("completed")), Some(ChildState::Reported));
        assert_eq!(member_state(&started, seen("stopped")), Some(ChildState::Interrupted));
        assert_eq!(member_state(&started, seen("error")), Some(ChildState::Failed));
        assert_eq!(member_state(&started, Some(MemberSeen { status: "completed", merged: true })), Some(ChildState::Merged));
        assert_eq!(member_state(&started, None), Some(ChildState::Closed), "a pane closed without a merge or a discard");
        assert_eq!(member_state(&child("never", "claude"), None), None, "one that never started has no state");
        let merged = CompareChild { id: "m".into(), outcome: Some(CompareChild::MERGED.into()), ..started.clone() };
        assert_eq!(member_state(&merged, None), Some(ChildState::Merged), "kept after its pane closed");
        let discarded = CompareChild { outcome: Some(CompareChild::DISCARDED.into()), ..started.clone() };
        assert_eq!(member_state(&discarded, seen("running")), Some(ChildState::Discarded));
        assert!(can_merge(&started, member_state(&started, seen("completed"))));
        assert!(!can_merge(&started, member_state(&started, seen("running"))));

        let run = CompareRun {
            id: "g".into(),
            children: vec![started.clone(), CompareChild { id: "b".into(), ..started.clone() }, merged.clone()],
            ..CompareRun::default()
        };
        let states: HashMap<String, ChildState> =
            [("a".to_string(), ChildState::Reported), ("b".to_string(), ChildState::Running)].into_iter().collect();
        assert_eq!(open_members(std::slice::from_ref(&run), &states, |_| true), 2);
        assert_eq!(open_members(std::slice::from_ref(&run), &states, |id| id == "a"), 1, "a gone pane leaves room");
        assert_eq!(rest_to_discard(&run, "a", &states), ["b"]);

        let choices = ssh_choices("claude", "acceptEdits", false);
        assert!(choices.iter().all(|c| mighty_run::ssh::supported_provider(&c.agent)), "only agents whose CLI runs over ssh");
        assert!(choices.iter().any(|c| c.agent == "codex" && c.blocked.is_none()), "this computer's installs are not asked");
        // MC-45: the same cap of four as a local pane's: three open members leave room for one row, so no run of two.
        let three = CompareRun {
            id: "g3".into(),
            children: ["x", "y", "z"].iter().map(|id| CompareChild { id: (*id).into(), ..started.clone() }).collect(),
            ..CompareRun::default()
        };
        let running: HashMap<String, ChildState> = ["x", "y", "z"].iter().map(|id| (id.to_string(), ChildState::Running)).collect();
        let open = open_members(std::slice::from_ref(&three), &running, |_| true);
        assert_eq!((open, max_rows(open)), (3, 1));
        assert_eq!(max_rows(open_members(std::slice::from_ref(&three), &running, |id| id == "x")), 3, "closed panes leave room");
    }
}
