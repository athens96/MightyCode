//! Cumulative token & cost ledger (`<data>/usage-ledger.json`), 7/30-day breakdown report, and daily budget exceeded alerts across
//! desktop toast and mobile E2EE push (`MC-11`).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use chrono::Local;
use mighty_core::usage_ledger::{UsageLedger, UsageTotal};
use serde_json::{Value, json};
use tauri::Manager as _;

use crate::host::{Host, Registry, lock};

#[derive(Default)]
struct LedgerBook {
    loaded: bool,
    ledger: UsageLedger,
}

#[derive(Default)]
pub struct UsageReportState {
    book: Mutex<LedgerBook>,
}

fn state(host: &Host) -> Arc<UsageReportState> {
    if let Some(existing) = host.app.try_state::<Arc<UsageReportState>>() {
        return existing.inner().clone();
    }
    let created = Arc::new(UsageReportState::default());
    host.app.manage(created.clone());
    host.app.try_state::<Arc<UsageReportState>>().map(|s| s.inner().clone()).unwrap_or(created)
}

fn with_ledger<T>(host: &Host, run: impl FnOnce(&mut UsageLedger) -> T) -> T {
    let st = state(host);
    let mut book = lock(&st.book);
    if !book.loaded {
        book.ledger = UsageLedger::load(&host.data_dir);
        book.loaded = true;
    }
    run(&mut book.ledger)
}

/// Called from `runs.rs` whenever a run reaches `completed`, `stopped`, or `error`; what the run cost, when the ledger recorded it
/// (MC-9's `run.completed` event carries it).
pub fn on_run_finished(host: &Arc<Host>, session_id: &str) -> Option<f64> {
    if host.closing() {
        return None;
    }
    let sample = {
        let view = host.view();
        let pane = view.snapshot.session(session_id)?;
        if pane.kind() != "claude" {
            return None;
        }
        let usage = pane.session_usage.clone()?;
        (pane.id().to_string(), pane.workspace_id().to_string(), pane.provider().to_string(), pane.model().to_string(), usage)
    };

    let today_date = Local::now().date_naive();
    let today_str = today_date.format("%Y-%m-%d").to_string();
    let (recorded, crossed, spent_today, budget, to_save) = with_ledger(host, |ledger| {
        let rec = ledger.record(&sample.0, &sample.1, &sample.2, &sample.3, &sample.4, &today_str);
        let budget = ledger.daily_budget_usd.unwrap_or(0.0);
        let crossed = rec.as_ref().is_some_and(|r| ledger.crossed_budget(budget, r));
        let spent = ledger.cost_on(&today_str);
        let clone_for_save = rec.is_some().then(|| ledger.clone());
        (rec.map(|r| r.cost_usd), crossed, spent, budget, clone_for_save)
    });

    let recorded_cost = recorded?;
    // MC-29: a compare run's child adds what this run cost to its column.
    crate::compare::add_cost(host, session_id, recorded_cost);
    // MC-64: a bot's run counts toward its own daily budget.
    let handle = host.view().snapshot.session(session_id).and_then(|p| p.bot.as_ref().map(|b| b.handle.clone()));
    if let Some(handle) = handle {
        crate::automation::add_bot_spend(host, &handle, &today_str, recorded_cost);
    }

    if host.writes()
        && let Some(snapshot_ledger) = to_save
    {
        let dir = host.data_dir.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let _ = snapshot_ledger.save(&dir, today_date);
        });
    }

    if crossed {
        let spent_label = format!("${spent_today:.2}");
        let budget_label = format!("${budget:.2}");
        let title = host.locale.get("usage.budget.notificationTitle");
        let body = host.locale.get_with("usage.budget.notificationBody", &[("spent", &spent_label), ("budget", &budget_label)]);
        crate::notifications::notify_custom(host, title.clone(), body.clone());
        if let Some(push) = crate::mobile::push(host) {
            let _ = push.send_custom(mighty_remote::push::message::BUDGET, &host.locale.get("usage.title"), &body, "budget");
        }
    }
    Some(recorded_cost)
}

/// MC-28: an `/advisor` opinion's cost and tokens in the ledger, under the workspace of the pane that asked (a one-off run: no
/// per-pane baseline). Saved like a finished run's; a crossed daily budget says so the same way.
pub fn record_advisor(host: &Arc<Host>, workspace_id: &str, provider: &str, model: &str, cost_usd: Option<f64>, tokens: Option<i64>) {
    if host.closing() {
        return;
    }
    let today_date = Local::now().date_naive();
    let today_str = today_date.format("%Y-%m-%d").to_string();
    let (to_save, crossed, spent_today, budget) = with_ledger(host, |ledger| {
        let rec = ledger.record_once(workspace_id, provider, model, cost_usd, tokens, &today_str);
        let budget = ledger.daily_budget_usd.unwrap_or(0.0);
        let crossed = rec.as_ref().is_some_and(|r| ledger.crossed_budget(budget, r));
        (rec.is_some().then(|| ledger.clone()), crossed, ledger.cost_on(&today_str), budget)
    });
    let Some(snapshot_ledger) = to_save else { return };
    if host.writes() {
        let dir = host.data_dir.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let _ = snapshot_ledger.save(&dir, today_date);
        });
    }
    if crossed {
        let body = host
            .locale
            .get_with("usage.budget.notificationBody", &[("spent", &format!("${spent_today:.2}")), ("budget", &format!("${budget:.2}"))]);
        crate::notifications::notify_custom(host, host.locale.get("usage.budget.notificationTitle"), body);
    }
}

/// MC-29: what the compare run's budget warning reads of the ledger: the records of the last 30 days, what was spent today and the
/// daily budget (None when none is set).
pub fn budget_reading(host: &Host) -> (Vec<mighty_core::usage_ledger::UsageRecord>, f64, Option<f64>) {
    let today = Local::now().date_naive();
    let today_str = today.format("%Y-%m-%d").to_string();
    with_ledger(host, |ledger| {
        (ledger.recent(30, today), ledger.cost_on(&today_str), ledger.daily_budget_usd.filter(|b| b.is_finite() && *b > 0.0))
    })
}

pub fn forget_pane(host: &Host, pane_id: &str) {
    with_ledger(host, |ledger| ledger.forget(pane_id));
}

/// Drops the deduplication baselines of panes that are no longer open from the ledger this module owns, and saves it when any went
/// (under the ledger's lock, so no older copy of it can land over a newer save). Returns how many went.
pub fn prune_closed_panes(host: &Host, live_session_ids: &HashSet<String>) -> usize {
    with_ledger(host, |ledger| {
        let pruned = ledger.prune_closed_sessions(live_session_ids);
        if pruned > 0 && host.writes() {
            let _ = ledger.save(&host.data_dir, Local::now().date_naive());
        }
        pruned
    })
}

fn provider_display_name(provider: &str) -> &str {
    match provider {
        "claude" => "Claude",
        "codex" => "Codex",
        "gemini" => "Antigravity",
        "pi" => "pi",
        "opencode" => "opencode",
        "hermes" => "Hermes",
        other => other,
    }
}

fn row_json(row: &UsageTotal, label: String) -> Value {
    json!({
        "key": row.key,
        "label": label,
        "costUsd": row.cost_usd,
        "tokens": row.tokens,
        "runs": row.runs,
    })
}

pub fn register(methods: &mut Registry) {
    methods.add("usageReport", |host: Arc<Host>, args| async move {
        let days = args.get("days").and_then(Value::as_i64).filter(|d| *d == 30).unwrap_or(7);
        let ws_names: HashMap<String, String> = {
            let view = host.view();
            view.snapshot.workspaces().iter().map(|w| (w.id().to_string(), w.name().to_string())).collect()
        };
        let removed_label = host.locale.get("usage.removedWorkspace");
        let today = Local::now().date_naive();
        let today_str = today.format("%Y-%m-%d").to_string();

        let payload = with_ledger(&host, |ledger| {
            let recent = ledger.recent(days, today);
            let total_cost: f64 = recent.iter().map(|r| r.cost_usd).sum();
            let total_tokens: i64 = recent.iter().map(|r| r.tokens).sum();
            let total_runs = recent.len();
            let today_cost = ledger.cost_on(&today_str);
            let budget = ledger.daily_budget_usd.unwrap_or(0.0);

            let daily: Vec<Value> = ledger.daily(days, today).iter().map(|d| row_json(d, d.key.clone())).collect();
            let by_workspace: Vec<Value> = UsageLedger::totals(&recent, |r| {
                if ws_names.contains_key(&r.workspace_id) { r.workspace_id.clone() } else { "__removed__".into() }
            })
            .into_iter()
            .take(8)
            .map(|row| {
                let label = if row.key == "__removed__" {
                    removed_label.clone()
                } else {
                    ws_names.get(&row.key).cloned().unwrap_or_else(|| row.key.clone())
                };
                row_json(&row, label)
            })
            .collect();

            let by_model: Vec<Value> =
                UsageLedger::totals(&recent, |r| r.model.clone()).into_iter().take(8).map(|row| row_json(&row, row.key.clone())).collect();

            let by_agent: Vec<Value> = UsageLedger::totals(&recent, |r| r.provider.clone())
                .into_iter()
                .take(8)
                .map(|row| row_json(&row, provider_display_name(&row.key).to_string()))
                .collect();

            json!({
                "days": days,
                "totalCostUsd": total_cost,
                "totalTokens": total_tokens,
                "totalRuns": total_runs,
                "todayCostUsd": today_cost,
                "budgetUsd": budget,
                "daily": daily,
                "byWorkspace": by_workspace,
                "byModel": by_model,
                "byAgent": by_agent,
            })
        });

        Ok(payload)
    });

    methods.add("usageSetBudget", |host: Arc<Host>, args| async move {
        let raw = args.get("budgetUsd").and_then(Value::as_f64).unwrap_or(0.0);
        let budget = if raw.is_finite() && raw > 0.0 { (raw * 100.0).round() / 100.0 } else { 0.0 };
        let today = Local::now().date_naive();
        let snapshot_ledger = with_ledger(&host, |ledger| {
            ledger.daily_budget_usd = (budget > 0.0).then_some(budget);
            ledger.clone()
        });
        if host.writes() {
            let dir = host.data_dir.clone();
            let _ = tauri::async_runtime::spawn_blocking(move || snapshot_ledger.save(&dir, today)).await;
        }
        Ok(json!({ "budgetUsd": budget }))
    });
}
