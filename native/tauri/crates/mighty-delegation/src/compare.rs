//! 비교 실행 (MC-29, after the 0.4 Swift app's `CompareRun.swift`): one request handed to several agents — or one agent several times
//! ("3× Claude") — at once, each in its own child worktree from the same base commit, their results set side by side so one is merged
//! and the rest discarded. Built on the children the human starts ([`crate::app::DelegationApp::start_human_child_with`]), so the
//! cards' merge, undo and discard keep working; this only picks who runs, reads what each changed and sums the group up. The host
//! (src-tauri `compare.rs`) keeps the group on the parent pane (`RunSession.compare_runs`), runs the chosen test and draws it.

use std::collections::HashMap;

use mighty_core::model::{COMPARE_MAXIMUM_ROWS, CompareChild, CompareRun};
use mighty_core::shell::{ChildState, PaneCompare};
use mighty_core::usage_ledger::UsageRecord;

use crate::coordinator::MAXIMUM_OPEN_CHILDREN;
use crate::git::{Git, line, run};
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
    /// More rows than [`max_rows`] allows now (the human children's cap, or four).
    TooMany(usize),
    /// An agent that cannot start a child now (not installed, signed out, no mode, or unknown).
    Unavailable(String),
}

/// How many children a compare run may start now: four at most, and no more than the room left under the human children's cap
/// of the parent (`open_human_children` of them open already).
pub fn max_rows(open_human_children: usize) -> usize {
    COMPARE_MAXIMUM_ROWS.min(MAXIMUM_OPEN_CHILDREN.saturating_sub(open_human_children))
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
    open_human_children: usize,
) -> Result<Vec<CompareRow>, CompareRowsError> {
    if rows.len() < 2 {
        return Err(CompareRowsError::TooFew);
    }
    let most = max_rows(open_human_children);
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
/// cannot tell (a branch gone, git failing).
pub async fn shortstat(git: &Git, checkout: &str, base: &str, branch: &str) -> Option<CompareStat> {
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

    #[test]
    fn the_rows_stay_within_four_and_the_room_left() {
        assert_eq!(max_rows(0), COMPARE_MAXIMUM_ROWS.min(MAXIMUM_OPEN_CHILDREN));
        assert_eq!(max_rows(MAXIMUM_OPEN_CHILDREN - 1), 1);
        assert_eq!(max_rows(MAXIMUM_OPEN_CHILDREN + 2), 0);
        let choices = [choice("claude", None)];
        let rows = vec![row("claude", None); 5];
        assert_eq!(checked_rows(&rows, &choices, "claude", 0), Err(CompareRowsError::TooMany(max_rows(0))));
        assert_eq!(checked_rows(&rows[..2], &choices, "claude", MAXIMUM_OPEN_CHILDREN - 1), Err(CompareRowsError::TooMany(1)));
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
}
