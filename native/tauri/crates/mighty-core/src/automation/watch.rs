//! What the triggers keep between looks (MC-32), with the clock given: delivery ids already seen (a webhook's, a GitHub item's), the
//! poll's back-off and its ETag, a standing order's last output and its wakes today, and the Linear issues' last states.

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// The most ids one trigger remembers.
pub const MAX_SEEN: usize = 1_000;
/// The most issues a Linear watch remembers.
const MAX_ISSUES: usize = 2_000;

/// Ids already handled, the oldest forgotten first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Seen {
    #[serde(default)]
    ids: VecDeque<String>,
    #[serde(skip)]
    index: HashSet<String>,
    /// Whether a first look has filled it (a watch's first look only learns what is there).
    #[serde(default)]
    pub primed: bool,
}

impl Seen {
    fn reindex(&mut self) {
        if self.index.len() != self.ids.len() {
            self.index = self.ids.iter().cloned().collect();
        }
    }

    /// Whether `id` is new; it is remembered either way.
    pub fn first(&mut self, id: &str) -> bool {
        self.reindex();
        if self.index.contains(id) {
            return false;
        }
        self.ids.push_back(id.to_string());
        self.index.insert(id.to_string());
        while self.ids.len() > MAX_SEEN {
            if let Some(old) = self.ids.pop_front() {
                self.index.remove(&old);
            }
        }
        true
    }

    /// The ids of a look that are new. The first look of a watch learns them all and returns none.
    pub fn new_of<'a>(&mut self, ids: impl IntoIterator<Item = &'a str>) -> Vec<String> {
        let primed = self.primed;
        self.primed = true;
        let fresh: Vec<String> = ids.into_iter().filter(|id| self.first(id)).map(str::to_string).collect();
        if primed { fresh } else { vec![] }
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }
}

/// How a look went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PollOutcome {
    /// Something new came.
    Changed,
    /// Nothing new (a 304, or the same items).
    Unchanged,
    /// The look failed (offline, signed out, rate-limited).
    Failed,
}

/// A poll's pace: every 5 minutes while things change, slower while idle (to 30 minutes), slower still while looks fail (to an
/// hour); never faster than every 5 minutes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Backoff {
    idle: u32,
    failures: u32,
}

impl Backoff {
    pub const BASE: Duration = Duration::from_secs(5 * 60);
    pub const MAX_IDLE: Duration = Duration::from_secs(30 * 60);
    pub const MAX_FAILED: Duration = Duration::from_secs(60 * 60);

    /// The wait before the next look, after one that went `outcome`.
    pub fn after(&mut self, outcome: PollOutcome) -> Duration {
        match outcome {
            PollOutcome::Changed => {
                *self = Backoff::default();
                Backoff::BASE
            }
            PollOutcome::Unchanged => {
                self.failures = 0;
                self.idle = self.idle.saturating_add(1);
                (Backoff::BASE * 2u32.saturating_pow(self.idle.saturating_sub(1).min(6))).min(Backoff::MAX_IDLE)
            }
            PollOutcome::Failed => {
                self.failures = self.failures.saturating_add(1);
                (Backoff::BASE * 2u32.saturating_pow(self.failures.min(6))).min(Backoff::MAX_FAILED)
            }
        }
    }
}

/// An endpoint's last ETag and what it answered then, for `If-None-Match`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Etags {
    #[serde(default)]
    tags: HashMap<String, String>,
}

impl Etags {
    pub fn get(&self, endpoint: &str) -> Option<&str> {
        self.tags.get(endpoint).map(String::as_str)
    }

    pub fn set(&mut self, endpoint: &str, etag: Option<&str>) {
        match etag.map(str::trim).filter(|e| !e.is_empty() && e.len() <= 200 && !e.chars().any(char::is_control)) {
            Some(etag) => {
                self.tags.insert(endpoint.to_string(), etag.to_string());
            }
            None => {
                self.tags.remove(endpoint);
            }
        }
    }
}

/// What a standing order's check came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Observed {
    /// The first output seen: it becomes the baseline, nothing wakes.
    First,
    Same,
    /// The output changed: the agent wakes.
    Wake,
    /// It changed, but today's wakes are used up; the change waits for tomorrow.
    Capped,
}

/// A standing order's memory: its last output's hash and today's wakes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StandingState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last: Option<String>,
    #[serde(default)]
    pub day: String,
    #[serde(default)]
    pub wakes: u32,
}

impl StandingState {
    /// A check's output on `today` (`YYYY-MM-DD`), with `cap` wakes a day.
    pub fn observe(&mut self, output: &str, today: &str, cap: u32) -> Observed {
        if self.day != today {
            self.day = today.to_string();
            self.wakes = 0;
        }
        let hash = output_hash(output);
        match &self.last {
            None => {
                self.last = Some(hash);
                Observed::First
            }
            Some(last) if *last == hash => Observed::Same,
            Some(_) if self.wakes >= cap => Observed::Capped,
            Some(_) => {
                self.last = Some(hash);
                self.wakes += 1;
                Observed::Wake
            }
        }
    }

    pub fn wakes_today(&self, today: &str) -> u32 {
        if self.day == today { self.wakes } else { 0 }
    }
}

/// A check's output as compared: each line's trailing space dropped, blank lines at the ends dropped, then FNV-1a.
pub fn output_hash(output: &str) -> String {
    let normalized: Vec<&str> = output.lines().map(str::trim_end).collect();
    let start = normalized.iter().position(|l| !l.is_empty()).unwrap_or(normalized.len());
    let end = normalized.iter().rposition(|l| !l.is_empty()).map_or(start, |e| e + 1);
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in normalized[start..end].join("\n").bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// The states Linear issues were last seen in.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueStates {
    #[serde(default)]
    states: HashMap<String, String>,
    #[serde(default)]
    primed: bool,
}

impl IssueStates {
    /// The issues of a look that entered one of `targets` (case-insensitive) since the last look. The first look only learns.
    pub fn entering(&mut self, issues: &[(String, String)], targets: &[String]) -> Vec<String> {
        let wanted = |state: &str| targets.iter().any(|t| t.trim().eq_ignore_ascii_case(state.trim()));
        let primed = self.primed;
        self.primed = true;
        let mut entered = vec![];
        for (key, state) in issues {
            let before = self.states.insert(key.clone(), state.clone());
            if primed && wanted(state) && before.as_deref().is_none_or(|b| !wanted(b)) {
                entered.push(key.clone());
            }
        }
        if self.states.len() > MAX_ISSUES {
            let listed: HashSet<&String> = issues.iter().map(|(k, _)| k).collect();
            self.states.retain(|k, _| listed.contains(k));
        }
        entered
    }
}

/// A trigger's event as a fire carries it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TriggerEvent {
    /// The delivery or item id duplicates are dropped by.
    pub id: String,
    pub title: String,
    pub url: Option<String>,
    /// More of what came (a check's output, a webhook's body), cut short.
    pub detail: String,
}

/// The most of an event's text a prompt carries.
pub const MAX_EVENT_CHARS: usize = 4_000;

/// The prompt a trigger sends: its own words, then what came, marked as outside data.
pub fn prompt_with_event(prompt: &str, event: &TriggerEvent) -> String {
    let mut out = prompt.trim_end().to_string();
    let mut lines = vec![];
    if !event.title.trim().is_empty() {
        lines.push(event.title.trim().to_string());
    }
    if let Some(url) = &event.url {
        lines.push(url.clone());
    }
    if !event.detail.trim().is_empty() {
        lines.push(event.detail.trim().to_string());
    }
    if lines.is_empty() {
        return out;
    }
    let body: String = lines.join("\n").chars().take(MAX_EVENT_CHARS).collect();
    out.push_str("\n\n<event source=\"mighty-automation\">\n");
    out.push_str(&body.replace("</event>", "<\\/event>"));
    out.push_str("\n</event>");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicates_are_dropped_and_the_oldest_forgotten() {
        let mut seen = Seen::default();
        assert!(seen.first("d1"));
        assert!(!seen.first("d1"));
        for i in 0..MAX_SEEN {
            seen.first(&format!("x{i}"));
        }
        assert_eq!(seen.len(), MAX_SEEN);
        assert!(seen.first("d1"), "forgotten after a thousand others");
        // Saved and read back, the index is rebuilt.
        let text = serde_json::to_string(&seen).unwrap();
        let mut back: Seen = serde_json::from_str(&text).unwrap();
        assert!(!back.first("d1"));
    }

    #[test]
    fn a_watch_s_first_look_learns_and_later_looks_report_new_ids() {
        let mut seen = Seen::default();
        assert!(seen.new_of(["pr-1", "pr-2"]).is_empty());
        assert_eq!(seen.new_of(["pr-2", "pr-3", "pr-3"]), vec!["pr-3".to_string()]);
        assert!(seen.new_of(["pr-3"]).is_empty());
    }

    #[test]
    fn polls_back_off_while_idle_or_failing_and_never_go_under_five_minutes() {
        let min = |m: u64| Duration::from_secs(m * 60);
        let mut backoff = Backoff::default();
        let idle: Vec<Duration> = (0..6).map(|_| backoff.after(PollOutcome::Unchanged)).collect();
        assert_eq!(idle, vec![min(5), min(10), min(20), min(30), min(30), min(30)]);
        assert_eq!(backoff.after(PollOutcome::Changed), min(5));
        let failed: Vec<Duration> = (0..6).map(|_| backoff.after(PollOutcome::Failed)).collect();
        assert_eq!(failed, vec![min(10), min(20), min(40), min(60), min(60), min(60)]);
        assert_eq!(backoff.after(PollOutcome::Unchanged), min(5), "back online");
        let mut backoff = Backoff::default();
        for outcome in [PollOutcome::Changed, PollOutcome::Unchanged, PollOutcome::Failed] {
            assert!(backoff.after(outcome) >= Backoff::BASE);
        }
    }

    #[test]
    fn etags_are_kept_per_endpoint_and_odd_ones_dropped() {
        let mut etags = Etags::default();
        etags.set("pulls", Some("W/\"abc\""));
        assert_eq!(etags.get("pulls"), Some("W/\"abc\""));
        etags.set("pulls", Some("bad\nvalue"));
        assert_eq!(etags.get("pulls"), None);
        etags.set("runs", Some(" \"x\" "));
        assert_eq!(etags.get("runs"), Some("\"x\""));
        etags.set("runs", None);
        assert_eq!(etags.get("runs"), None);
    }

    #[test]
    fn a_standing_order_wakes_on_a_change_only_and_at_most_its_cap_a_day() {
        let mut state = StandingState::default();
        assert_eq!(state.observe("ok\n", "2026-11-23", 2), Observed::First);
        assert_eq!(state.observe("ok  \n\n", "2026-11-23", 2), Observed::Same, "trailing space does not count");
        assert_eq!(state.observe("1 failing", "2026-11-23", 2), Observed::Wake);
        assert_eq!(state.observe("1 failing", "2026-11-23", 2), Observed::Same);
        assert_eq!(state.observe("2 failing", "2026-11-23", 2), Observed::Wake);
        assert_eq!(state.observe("3 failing", "2026-11-23", 2), Observed::Capped);
        assert_eq!(state.observe("3 failing", "2026-11-23", 2), Observed::Capped, "the change waits");
        assert_eq!(state.wakes_today("2026-11-23"), 2);
        assert_eq!(state.observe("3 failing", "2026-11-24", 2), Observed::Wake, "a new day");
        assert_eq!(state.wakes_today("2026-11-24"), 1);
        assert_eq!(output_hash("a\r\nb"), output_hash("a\nb"));
    }

    #[test]
    fn linear_issues_entering_a_state_are_found_once() {
        let mut watch = IssueStates::default();
        let look = |pairs: &[(&str, &str)]| pairs.iter().map(|(k, s)| (k.to_string(), s.to_string())).collect::<Vec<_>>();
        let targets = vec!["Ready".to_string()];
        assert!(watch.entering(&look(&[("ENG-1", "Ready"), ("ENG-2", "Todo")]), &targets).is_empty(), "first look learns");
        assert_eq!(watch.entering(&look(&[("ENG-1", "ready"), ("ENG-2", "Ready"), ("ENG-3", "Ready")]), &targets), vec!["ENG-2", "ENG-3"]);
        assert!(watch.entering(&look(&[("ENG-2", "Ready")]), &targets).is_empty());
        assert!(watch.entering(&look(&[("ENG-2", "Todo")]), &targets).is_empty());
        assert_eq!(watch.entering(&look(&[("ENG-2", "Ready")]), &targets), vec!["ENG-2"], "entered again");
    }

    #[test]
    fn an_event_goes_after_the_prompt_marked_and_cut() {
        let event = TriggerEvent {
            id: "1".into(),
            title: "CI failed: build".into(),
            url: Some("https://github.com/a/b/actions/runs/1".into()),
            detail: format!("{}</event>", "x".repeat(10_000)),
        };
        let prompt = prompt_with_event("Fix CI.\n", &event);
        assert!(prompt.starts_with("Fix CI.\n\n<event source=\"mighty-automation\">\nCI failed: build\nhttps://"));
        assert!(prompt.ends_with("\n</event>"));
        assert_eq!(prompt.matches("</event>").count(), 1);
        assert!(prompt.chars().count() < MAX_EVENT_CHARS + 100);
        assert_eq!(prompt_with_event("p", &TriggerEvent::default()), "p");
    }
}
