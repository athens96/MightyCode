//! The run history (MC-32): one record per fire or trigger — what fired, when, the pane it ran in, its state, what it cost by the
//! usage ledger, a result link (a pull request the pane opened) and an error. Kept newest last in `<data>/automation/history.json`,
//! capped by count and size; a retry is a new record pointing at the one it repeats.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// The most records kept, and the most bytes the file may take.
pub const MAX_RECORDS: usize = 300;
pub const MAX_BYTES: usize = 512 * 1024;
const MAX_REASON_CHARS: usize = 300;
const MAX_ERROR_CHARS: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecordState {
    /// Sent; the pane works on it.
    Running,
    /// The pane waits for an approval or an answer.
    Blocked,
    Done,
    Failed,
    Stopped,
    /// Not run: missed while the app was closed (`catchUp: skip`), over the daily budget, or over a standing order's cap.
    Skipped,
}

impl RecordState {
    pub fn name(self) -> &'static str {
        match self {
            RecordState::Running => "running",
            RecordState::Blocked => "blocked",
            RecordState::Done => "done",
            RecordState::Failed => "failed",
            RecordState::Stopped => "stopped",
            RecordState::Skipped => "skipped",
        }
    }

    /// Whether the record is settled (nothing more will change it).
    pub fn finished(self) -> bool {
        !matches!(self, RecordState::Running | RecordState::Blocked)
    }

    /// The state a run's end (`completed`, `stopped`, `error`) leaves.
    pub fn of_run_status(status: &str) -> RecordState {
        match status {
            "completed" => RecordState::Done,
            "stopped" => RecordState::Stopped,
            _ => RecordState::Failed,
        }
    }
}

/// One fire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunRecord {
    pub id: String,
    pub workspace: String,
    #[serde(default)]
    pub workspace_name: String,
    /// The schedule's or trigger's name.
    pub name: String,
    /// `app` (settings) or `yaml` (`.mighty.yaml`).
    pub origin: String,
    /// `schedule`, `github`, `linear`, `webhook` or `standing`.
    pub kind: String,
    /// What made it fire, in a few words (`예약`, `catch-up`, a PR's title, …).
    #[serde(default)]
    pub reason: String,
    /// RFC 3339.
    pub fired_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheduled_for: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane: Option<String>,
    pub state: RecordState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    /// The record this one repeats.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_of: Option<String>,
    /// The page of the event that fired it (a pull request, a workflow run, an issue).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_url: Option<String>,
}

impl RunRecord {
    /// A record's texts cut to their limits.
    fn clip(&mut self) {
        let cut = |text: &mut String, max: usize| {
            if text.chars().count() > max {
                *text = format!("{}…", text.chars().take(max - 1).collect::<String>());
            }
        };
        cut(&mut self.reason, MAX_REASON_CHARS);
        if let Some(error) = &mut self.error {
            cut(error, MAX_ERROR_CHARS);
        }
    }

    /// Adds a run's cost (a heartbeat pane may run more than once for one record).
    pub fn add_cost(&mut self, cost: Option<f64>) {
        if let Some(cost) = cost.filter(|c| c.is_finite() && *c >= 0.0) {
            self.cost_usd = Some(self.cost_usd.unwrap_or(0.0) + cost);
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct History {
    #[serde(default)]
    pub records: Vec<RunRecord>,
}

impl History {
    /// Reads the file; a missing or damaged one is an empty history.
    pub fn load(path: &Path) -> History {
        std::fs::read(path).ok().and_then(|bytes| serde_json::from_slice::<History>(&bytes).ok()).unwrap_or_default()
    }

    /// Writes the file whole (a temporary file renamed over it).
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let bytes = serde_json::to_vec(self).map_err(std::io::Error::other)?;
        crate::automation::write_atomic(path, &bytes)
    }

    /// Adds a record, newest last, then trims the oldest finished ones to the caps.
    pub fn push(&mut self, mut record: RunRecord) {
        record.clip();
        self.records.push(record);
        self.cap();
    }

    /// Drops the oldest records past [`MAX_RECORDS`] or [`MAX_BYTES`]; a running one goes only when nothing finished is left.
    pub fn cap(&mut self) {
        // `{"records":[` … `]}` and a comma between records.
        let mut sizes: Vec<usize> = self.records.iter().map(|r| serde_json::to_vec(r).map_or(0, |b| b.len()) + 1).collect();
        let mut total: usize = 14 + sizes.iter().sum::<usize>();
        while self.records.len() > MAX_RECORDS || (self.records.len() > 1 && total > MAX_BYTES) {
            let at = self.records.iter().position(|r| r.state.finished()).unwrap_or(0);
            self.records.remove(at);
            total -= sizes.remove(at);
        }
    }

    pub fn get(&self, id: &str) -> Option<&RunRecord> {
        self.records.iter().find(|r| r.id == id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut RunRecord> {
        self.records.iter_mut().find(|r| r.id == id)
    }

    /// The newest unsettled record running in `pane`.
    pub fn open_for_pane(&mut self, pane: &str) -> Option<&mut RunRecord> {
        self.records.iter_mut().rev().find(|r| !r.state.finished() && r.pane.as_deref() == Some(pane))
    }

    /// The newest record of one schedule or trigger.
    pub fn last_of(&self, workspace: &str, origin: &str, name: &str) -> Option<&RunRecord> {
        self.records.iter().rev().find(|r| r.workspace == workspace && r.origin == origin && r.name == name)
    }

    /// The newest `n` records (newest first), of one workspace or all.
    pub fn recent(&self, workspace: Option<&str>, n: usize) -> Vec<&RunRecord> {
        self.records.iter().rev().filter(|r| workspace.is_none_or(|w| r.workspace == w)).take(n).collect()
    }

    /// The digest of the records fired since `since` (RFC 3339 compares as text in one offset; the app writes UTC).
    pub fn digest(&self, since: &str) -> Digest {
        let mut digest = Digest::default();
        for record in self.records.iter().filter(|r| r.fired_at.as_str() >= since) {
            match record.state {
                RecordState::Done => digest.done += 1,
                RecordState::Failed | RecordState::Stopped => digest.failed += 1,
                RecordState::Skipped => digest.skipped += 1,
                RecordState::Running | RecordState::Blocked => digest.running += 1,
            }
            digest.cost_usd += record.cost_usd.unwrap_or(0.0);
        }
        digest
    }
}

/// What a day of automation came to.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Digest {
    pub done: usize,
    pub failed: usize,
    pub skipped: usize,
    pub running: usize,
    pub cost_usd: f64,
}

impl Digest {
    pub fn is_empty(&self) -> bool {
        self.done + self.failed + self.skipped + self.running == 0
    }
}

/// The last pull or merge request link in `text`: `https://github.com/<owner>/<repo>/pull/<n>` or a GitLab
/// `…/-/merge_requests/<n>`.
pub fn find_result_url(text: &str) -> Option<String> {
    let mut found = None;
    let mut rest = text;
    while let Some(at) = rest.find("https://") {
        let tail = &rest[at..];
        let end =
            tail.find(|c: char| c.is_whitespace() || matches!(c, ')' | ']' | '>' | '"' | '\'' | '`' | ',' | '<')).unwrap_or(tail.len());
        let candidate = tail[..end].trim_end_matches(['.', ';', ':']);
        if is_result_url(candidate) {
            found = Some(candidate.to_string());
        }
        rest = &tail[end.max(8)..];
    }
    found
}

fn is_result_url(url: &str) -> bool {
    let Some(path) = url.strip_prefix("https://") else { return false };
    let parts: Vec<&str> = path.split(['?', '#']).next().unwrap_or("").split('/').collect();
    let number = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
    match parts.as_slice() {
        ["github.com", owner, repo, "pull", n, ..] => !owner.is_empty() && !repo.is_empty() && number(n),
        [host, .., "-", "merge_requests", n] | [host, .., "-", "merge_requests", n, _] => !host.is_empty() && number(n),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn record(id: &str, state: RecordState) -> RunRecord {
        RunRecord {
            id: id.into(),
            workspace: "w1".into(),
            workspace_name: "app".into(),
            name: "deps".into(),
            origin: "app".into(),
            kind: "schedule".into(),
            reason: String::new(),
            fired_at: format!("2026-11-23T09:00:{:02}Z", id.len() % 60),
            scheduled_for: None,
            pane: None,
            state,
            cost_usd: None,
            url: None,
            error: None,
            finished_at: None,
            retry_of: None,
            event_url: None,
        }
    }

    #[test]
    fn the_history_keeps_at_most_its_count_dropping_finished_ones_first() {
        let mut history = History::default();
        history.push(record("running-0", RecordState::Running));
        for i in 0..MAX_RECORDS + 20 {
            history.push(record(&format!("r{i}"), RecordState::Done));
        }
        assert_eq!(history.records.len(), MAX_RECORDS);
        assert_eq!(history.records[0].id, "running-0", "an unsettled record outlives finished ones");
        assert_eq!(history.records.last().unwrap().id, format!("r{}", MAX_RECORDS + 19));
    }

    #[test]
    fn the_history_keeps_under_its_size() {
        let mut history = History::default();
        for i in 0..200 {
            let mut big = record(&format!("big{i}"), RecordState::Failed);
            big.error = Some("x".repeat(10_000));
            big.reason = "y".repeat(10_000);
            history.push(big);
        }
        let bytes = serde_json::to_vec(&history).unwrap().len();
        assert!(bytes <= MAX_BYTES, "{bytes}");
        let last = history.records.last().unwrap();
        assert_eq!(last.error.as_ref().unwrap().chars().count(), MAX_ERROR_CHARS);
        assert_eq!(last.reason.chars().count(), MAX_REASON_CHARS);
    }

    #[test]
    fn records_round_trip_through_the_file_and_damage_reads_empty() {
        let dir = std::env::temp_dir().join(format!("mc-history-{}-{}", std::process::id(), line!()));
        let path = dir.join("history.json");
        let mut history = History::default();
        let mut one = record("a", RecordState::Running);
        one.pane = Some("p1".into());
        history.push(one);
        history.save(&path).unwrap();
        let mut back = History::load(&path);
        assert_eq!(back, history);
        let open = back.open_for_pane("p1").unwrap();
        open.add_cost(Some(0.25));
        open.add_cost(Some(0.5));
        open.add_cost(Some(f64::NAN));
        assert_eq!(open.cost_usd, Some(0.75));
        std::fs::write(&path, b"{nope").unwrap();
        assert!(History::load(&path).records.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_digest_counts_since_a_time() {
        let mut history = History::default();
        let mut done = record("a", RecordState::Done);
        done.cost_usd = Some(1.5);
        history.push(done);
        history.push(record("bb", RecordState::Skipped));
        let mut old = record("ccc", RecordState::Failed);
        old.fired_at = "2026-11-20T09:00:00Z".into();
        history.push(old);
        let digest = history.digest("2026-11-23T00:00:00Z");
        assert_eq!((digest.done, digest.failed, digest.skipped, digest.cost_usd), (1, 0, 1, 1.5));
        assert!(History::default().digest("").is_empty());
    }

    #[test]
    fn result_links_are_found_in_a_pane_s_words() {
        let text = "Opened https://github.com/acme/app/pull/42. Also see https://github.com/acme/app/issues/3 and (https://github.com/acme/app/pull/43)";
        assert_eq!(find_result_url(text).as_deref(), Some("https://github.com/acme/app/pull/43"));
        assert_eq!(
            find_result_url("MR: https://gitlab.example.com/g/sub/app/-/merge_requests/7").as_deref(),
            Some("https://gitlab.example.com/g/sub/app/-/merge_requests/7")
        );
        assert_eq!(find_result_url("https://github.com/acme/app/pull/x"), None);
        assert_eq!(find_result_url("http://github.com/acme/app/pull/1"), None);
        assert_eq!(find_result_url("nothing"), None);
        assert_eq!(RecordState::of_run_status("completed"), RecordState::Done);
        assert_eq!(RecordState::of_run_status("error"), RecordState::Failed);
    }
}
