//! Persistent token & cost ledger (`<data>/usage-ledger.json`, Swift `UsageLedger.swift`): records each finished run's CLI-reported
//! cost and tokens, deduplicating `session`-scoped cumulative totals per pane, aggregating over 7/30-day windows by day, workspace,
//! model, and agent, and detecting daily budget crossings once per day.

use std::collections::BTreeMap;
use std::path::Path;

use chrono::{Duration as ChronoDuration, NaiveDate};
use serde::{Deserialize, Serialize};

use crate::model::SessionUsage;

pub const FILE_NAME: &str = "usage-ledger.json";
pub const KEEP_DAYS: i64 = 120;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageRecord {
    pub day: String,
    #[serde(rename = "workspaceId")]
    pub workspace_id: String,
    pub provider: String,
    pub model: String,
    #[serde(rename = "costUSD")]
    pub cost_usd: f64,
    pub tokens: i64,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct UsageLast {
    #[serde(rename = "providerSessionId", skip_serializing_if = "Option::is_none", default)]
    pub provider_session_id: Option<String>,
    #[serde(rename = "costUSD", skip_serializing_if = "Option::is_none", default)]
    pub cost_usd: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub tokens: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct UsageLedger {
    #[serde(default)]
    pub records: Vec<UsageRecord>,
    #[serde(default)]
    pub last: BTreeMap<String, UsageLast>,
    #[serde(rename = "dailyBudgetUSD", skip_serializing_if = "Option::is_none", default)]
    pub daily_budget_usd: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UsageTotal {
    pub key: String,
    pub cost_usd: f64,
    pub tokens: i64,
    pub runs: usize,
}

impl UsageLedger {
    /// Records the finished run's delta for `pane_id` on `day` (`yyyy-MM-dd`); returns `None` when neither cost nor tokens grew.
    pub fn record(
        &mut self,
        pane_id: &str,
        workspace_id: &str,
        provider: &str,
        model: &str,
        usage: &SessionUsage,
        day: &str,
    ) -> Option<UsageRecord> {
        let previous = self.last.get(pane_id);
        let same_session =
            previous.is_some_and(|prev| prev.provider_session_id.is_some() && prev.provider_session_id == usage.provider_session_id);

        let delta_f64 = |value: Option<f64>, scope: Option<&str>, before: Option<f64>| -> f64 {
            let Some(v) = value.filter(|n| n.is_finite() && *n >= 0.0) else { return 0.0 };
            if scope == Some("session")
                && same_session
                && let Some(prev) = before
            {
                if v >= prev { v - prev } else { v }
            } else {
                v
            }
        };

        let delta_i64 = |value: Option<i64>, scope: Option<&str>, before: Option<i64>| -> i64 {
            let Some(v) = value.filter(|n| *n >= 0) else { return 0 };
            if scope == Some("session")
                && same_session
                && let Some(prev) = before
            {
                if v >= prev { v - prev } else { v }
            } else {
                v
            }
        };

        let cost = delta_f64(usage.cost_usd, usage.cost_scope.as_deref(), previous.and_then(|p| p.cost_usd));
        let tokens = delta_i64(usage.total_tokens, Some(usage.token_scope.as_str()), previous.and_then(|p| p.tokens));

        self.last.insert(
            pane_id.to_string(),
            UsageLast { provider_session_id: usage.provider_session_id.clone(), cost_usd: usage.cost_usd, tokens: usage.total_tokens },
        );

        if cost <= 0.0 && tokens <= 0 {
            return None;
        }

        let chosen_model = usage.model.as_deref().filter(|m| !m.trim().is_empty()).unwrap_or(model).to_string();
        let entry = UsageRecord {
            day: day.to_string(),
            workspace_id: workspace_id.to_string(),
            provider: provider.to_string(),
            model: chosen_model,
            cost_usd: cost,
            tokens,
        };
        self.records.push(entry.clone());
        Some(entry)
    }

    pub fn forget(&mut self, pane_id: &str) {
        self.last.remove(pane_id);
    }

    /// Inclusive `[today - (days - 1) ..= today]` filter.
    pub fn recent(&self, days: i64, today: NaiveDate) -> Vec<UsageRecord> {
        let days = days.max(1);
        let first = (today - ChronoDuration::days(days - 1)).format("%Y-%m-%d").to_string();
        let last_day = today.format("%Y-%m-%d").to_string();
        self.records.iter().filter(|r| r.day >= first && r.day <= last_day).cloned().collect()
    }

    /// Groups `records` by `key_fn`, sorted descending by `(cost_usd, tokens)` then ascending by `key`.
    pub fn totals(records: &[UsageRecord], key_fn: impl Fn(&UsageRecord) -> String) -> Vec<UsageTotal> {
        let mut sums: BTreeMap<String, UsageTotal> = BTreeMap::new();
        for record in records {
            let key = key_fn(record);
            let entry = sums.entry(key.clone()).or_insert(UsageTotal { key, cost_usd: 0.0, tokens: 0, runs: 0 });
            entry.cost_usd += record.cost_usd;
            entry.tokens += record.tokens;
            entry.runs += 1;
        }
        let mut out: Vec<UsageTotal> = sums.into_values().collect();
        out.sort_by(|a, b| {
            b.cost_usd
                .partial_cmp(&a.cost_usd)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.tokens.cmp(&a.tokens))
                .then_with(|| a.key.cmp(&b.key))
        });
        out
    }

    /// One total per day of the last `days` days up to `today`, oldest first, zero-filled for idle days.
    pub fn daily(&self, days: i64, today: NaiveDate) -> Vec<UsageTotal> {
        let days = days.max(1);
        let recent = self.recent(days, today);
        let by_day: BTreeMap<String, UsageTotal> =
            Self::totals(&recent, |r| r.day.clone()).into_iter().map(|t| (t.key.clone(), t)).collect();
        (0..days)
            .rev()
            .map(|back| {
                let d = (today - ChronoDuration::days(back)).format("%Y-%m-%d").to_string();
                by_day.get(&d).cloned().unwrap_or(UsageTotal { key: d, cost_usd: 0.0, tokens: 0, runs: 0 })
            })
            .collect()
    }

    pub fn cost_on(&self, day: &str) -> f64 {
        self.records.iter().filter(|r| r.day == day).map(|r| r.cost_usd).sum()
    }

    /// True once when `record` pushed `record.day`'s cumulative cost from `< budget` to `>= budget`.
    pub fn crossed_budget(&self, budget: f64, record: &UsageRecord) -> bool {
        if !budget.is_finite() || budget <= 0.0 || record.cost_usd <= 0.0 {
            return false;
        }
        let day_total = self.cost_on(&record.day);
        day_total >= budget && (day_total - record.cost_usd) < budget
    }

    pub fn load(directory: &Path) -> UsageLedger {
        let path = directory.join(FILE_NAME);
        let Ok(bytes) = std::fs::read(&path) else { return UsageLedger::default() };
        serde_json::from_slice::<UsageLedger>(&bytes).unwrap_or_default()
    }

    pub fn save(&self, directory: &Path, today: NaiveDate) -> std::io::Result<()> {
        std::fs::create_dir_all(directory)?;
        let kept = UsageLedger {
            records: self.recent(KEEP_DAYS, today),
            last: self.last.clone(),
            daily_budget_usd: self.daily_budget_usd.filter(|b| b.is_finite() && *b > 0.0),
        };
        let mut json = serde_json::to_vec(&kept).map_err(std::io::Error::other)?;
        json.push(b'\n');
        let target = directory.join(FILE_NAME);
        let temp = directory.join(format!("{FILE_NAME}.{}.tmp", std::process::id()));
        std::fs::write(&temp, &json)?;
        if let Err(err) = std::fs::rename(&temp, &target) {
            let _ = std::fs::remove_file(&temp);
            return Err(err);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_usage(session_id: &str, scope: &str, cost: Option<f64>, tokens: Option<i64>, model: &str) -> SessionUsage {
        SessionUsage {
            provider: "claude".into(),
            source: "cli".into(),
            token_scope: scope.into(),
            model: Some(model.into()),
            selected_model: None,
            provider_session_id: Some(session_id.into()),
            input_tokens: None,
            output_tokens: None,
            cache_read_tokens: None,
            cache_write_tokens: None,
            reasoning_tokens: None,
            total_tokens: tokens,
            context_used_tokens: None,
            context_window_tokens: None,
            cost_usd: cost,
            cost_scope: Some(scope.into()),
            rate_limits: None,
            rate_limits_updated_at: None,
            updated_at: "2026-11-22T00:00:00Z".into(),
        }
    }

    #[test]
    fn deduplicates_session_scope_and_crosses_budget_exactly_once() {
        let mut ledger = UsageLedger::default();
        let u1 = sample_usage("s1", "session", Some(1.25), Some(1000), "claude-sonnet-4-6");
        let r1 = ledger.record("pane-1", "ws-1", "claude", "default", &u1, "2026-11-22").unwrap();
        assert!((r1.cost_usd - 1.25).abs() < 1e-9 && r1.tokens == 1000);
        assert!(!ledger.crossed_budget(2.0, &r1));

        // Repeated identical session snapshot produces no new record.
        assert!(ledger.record("pane-1", "ws-1", "claude", "default", &u1, "2026-11-22").is_none());

        // Cumulative session growth records only the delta (2.30 - 1.25 = 1.05) and crosses the $2.00 budget!
        let u2 = sample_usage("s1", "session", Some(2.30), Some(2500), "claude-sonnet-4-6");
        let r2 = ledger.record("pane-1", "ws-1", "claude", "default", &u2, "2026-11-22").unwrap();
        assert!((r2.cost_usd - 1.05).abs() < 1e-9 && r2.tokens == 1500);
        assert!(ledger.crossed_budget(2.0, &r2));

        // Subsequent run on the same day stays above budget and does NOT cross again.
        let u3 = sample_usage("s1", "session", Some(2.80), Some(3000), "claude-sonnet-4-6");
        let r3 = ledger.record("pane-1", "ws-1", "claude", "default", &u3, "2026-11-22").unwrap();
        assert!(!ledger.crossed_budget(2.0, &r3));

        let today = NaiveDate::from_ymd_opt(2026, 11, 22).unwrap();
        let daily = ledger.daily(7, today);
        assert_eq!(daily.len(), 7);
        assert!((daily[6].cost_usd - 2.80).abs() < 1e-9);
        assert_eq!(daily[6].tokens, 3000);
        assert_eq!(daily[6].runs, 3);
    }
}
