//! What a fire does to the run history (MC-32), with the clock given and no I/O: a fire's record (running, or skipped over the
//! budget), its pane, a card waiting, the run's end with its cost and result link, and which job a retry fires again. The app's
//! fires (src-tauri `automation::fire`) and the end-to-end check drive the same steps.

use chrono::{DateTime, Utc};

use super::records::{History, RecordState, RunRecord, find_result_url};

/// The job a record belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobRef {
    pub key: String,
    pub workspace: String,
    pub workspace_name: String,
    pub name: String,
    /// `app` or `yaml`.
    pub origin: String,
    /// `schedule`, `github`, `linear`, `webhook` or `standing`.
    pub kind: String,
}

fn text(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// What starts a fire.
#[derive(Debug, Clone, Default)]
pub struct Begin {
    pub reason: String,
    pub scheduled_for: Option<DateTime<Utc>>,
    /// Why it does not run (over the daily budget), if it does not.
    pub over_budget: Option<String>,
    pub retry_of: Option<String>,
    pub event_url: Option<String>,
}

/// A fire's record, added to `history`: running, or skipped with the budget's words.
pub fn begin(history: &mut History, job: &JobRef, start: Begin, id: String, now: DateTime<Utc>) -> RunRecord {
    let skipped = start.over_budget.is_some();
    let record = RunRecord {
        id,
        workspace: job.workspace.clone(),
        workspace_name: job.workspace_name.clone(),
        name: job.name.clone(),
        origin: job.origin.clone(),
        kind: job.kind.clone(),
        reason: start.reason,
        fired_at: text(now),
        scheduled_for: start.scheduled_for.map(text),
        pane: None,
        state: if skipped { RecordState::Skipped } else { RecordState::Running },
        cost_usd: None,
        url: None,
        error: start.over_budget,
        finished_at: skipped.then(|| text(now)),
        retry_of: start.retry_of,
        event_url: start.event_url,
    };
    history.push(record.clone());
    record
}

/// A fire that is not run (missed while the app was closed with `catchUp: skip`).
pub fn skip(
    history: &mut History,
    job: &JobRef,
    reason: String,
    scheduled_for: Option<DateTime<Utc>>,
    id: String,
    now: DateTime<Utc>,
) -> RunRecord {
    let mut record = begin(history, job, Begin { reason, scheduled_for, ..Begin::default() }, id, now);
    record.state = RecordState::Skipped;
    record.finished_at = Some(text(now));
    if let Some(saved) = history.get_mut(&record.id) {
        *saved = record.clone();
    }
    record
}

/// The pane a fire runs in.
pub fn attach(history: &mut History, id: &str, pane: &str) {
    if let Some(record) = history.get_mut(id) {
        record.pane = Some(pane.to_string());
    }
}

/// A fire that could not start (no pane, the send refused).
pub fn fail(history: &mut History, id: &str, error: &str, now: DateTime<Utc>) -> Option<RunRecord> {
    let record = history.get_mut(id)?;
    record.state = RecordState::Failed;
    record.error = Some(error.chars().take(500).collect());
    record.finished_at = Some(text(now));
    Some(record.clone())
}

/// The record running in `pane` waits for the person (an approval or a question).
pub fn blocked(history: &mut History, pane: &str) -> Option<RunRecord> {
    let record = history.open_for_pane(pane)?;
    record.state = RecordState::Blocked;
    Some(record.clone())
}

/// The record's card was answered: it runs again.
pub fn resumed(history: &mut History, pane: &str) -> Option<RunRecord> {
    let record = history.open_for_pane(pane)?;
    record.state = RecordState::Running;
    Some(record.clone())
}

/// The run in `pane` ended (`completed`, `stopped`, `error`) with what it cost; `words` are the pane's last words, read for a pull
/// request link.
pub fn finish(history: &mut History, pane: &str, status: &str, cost: Option<f64>, words: &str, now: DateTime<Utc>) -> Option<RunRecord> {
    let record = history.open_for_pane(pane)?;
    record.add_cost(cost);
    record.state = RecordState::of_run_status(status);
    record.finished_at = Some(text(now));
    if let Some(url) = find_result_url(words) {
        record.url = Some(url);
    }
    Some(record.clone())
}

/// Why a record cannot be fired again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryError {
    /// No such record.
    NoRecord,
    /// Its schedule or trigger is gone, off or not approved.
    NoJob,
}

/// The job a retry of `id` fires, among the jobs that can run now, and the record it repeats.
pub fn retry_target<'a>(history: &History, id: &str, runnable: &'a [JobRef]) -> Result<(&'a JobRef, RunRecord), RetryError> {
    let record = history.get(id).cloned().ok_or(RetryError::NoRecord)?;
    let job = runnable
        .iter()
        .find(|j| j.workspace == record.workspace && j.origin == record.origin && j.name == record.name)
        .ok_or(RetryError::NoJob)?;
    Ok((job, record))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automation::recurrence::tests::utc;

    fn job() -> JobRef {
        JobRef {
            key: "w1:app:s:deps".into(),
            workspace: "w1".into(),
            workspace_name: "app".into(),
            name: "deps".into(),
            origin: "app".into(),
            kind: "schedule".into(),
        }
    }

    #[test]
    fn a_fire_s_record_follows_its_pane_to_the_end() {
        let mut history = History::default();
        let now = utc(2026, 11, 23, 14, 0);
        let record = begin(
            &mut history,
            &job(),
            Begin { reason: "schedule".into(), scheduled_for: Some(now), ..Begin::default() },
            "r1".into(),
            now,
        );
        assert_eq!(record.state, RecordState::Running);
        attach(&mut history, "r1", "p1");
        assert_eq!(blocked(&mut history, "p1").unwrap().state, RecordState::Blocked);
        assert_eq!(resumed(&mut history, "p1").unwrap().state, RecordState::Running);
        let done = finish(&mut history, "p1", "completed", Some(0.4), "Opened https://github.com/acme/app/pull/12", now).unwrap();
        assert_eq!(
            (done.state, done.cost_usd, done.url.as_deref()),
            (RecordState::Done, Some(0.4), Some("https://github.com/acme/app/pull/12"))
        );
        assert!(finish(&mut history, "p1", "completed", Some(1.0), "", now).is_none(), "settled records do not move");
    }

    #[test]
    fn a_fire_over_the_budget_is_a_skipped_record() {
        let mut history = History::default();
        let now = utc(2026, 11, 23, 14, 0);
        let start = Begin { reason: "schedule".into(), over_budget: Some("over".into()), ..Begin::default() };
        let record = begin(&mut history, &job(), start, "r1".into(), now);
        assert_eq!((record.state, record.error.as_deref()), (RecordState::Skipped, Some("over")));
        assert!(record.finished_at.is_some());
        let missed = skip(&mut history, &job(), "missed".into(), Some(now), "r2".into(), now);
        assert_eq!(history.get("r2").unwrap().state, RecordState::Skipped);
        assert_eq!(missed.state, RecordState::Skipped);
        let failed = begin(&mut history, &job(), Begin::default(), "r3".into(), now);
        assert_eq!(fail(&mut history, &failed.id, "no worktree", now).unwrap().state, RecordState::Failed);
    }

    #[test]
    fn a_retry_finds_its_job_only_while_it_can_run() {
        let mut history = History::default();
        let now = utc(2026, 11, 23, 14, 0);
        begin(&mut history, &job(), Begin::default(), "r1".into(), now);
        let jobs = vec![job()];
        let (found, record) = retry_target(&history, "r1", &jobs).unwrap();
        assert_eq!((found.key.as_str(), record.id.as_str()), ("w1:app:s:deps", "r1"));
        assert_eq!(retry_target(&history, "nope", &jobs).unwrap_err(), RetryError::NoRecord);
        assert_eq!(retry_target(&history, "r1", &[]).unwrap_err(), RetryError::NoJob);
    }
}
