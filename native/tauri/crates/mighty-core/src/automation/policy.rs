//! MC-99: a trigger's `policy`: what keeps its action safe to run on its own.
//!
//! - `approve`: the action waits for the person's 실행 / 건너뛰기 (the 알림 함's 이벤트 tab) before it starts. On by default for
//!   `run` and for a `develop` that merges ([`Action::approval_by_default`]); off otherwise unless set.
//! - `oncePer: subject`: never acts twice on the same subject (`PR#12`, `MC-7`); the last [`MAX_HANDLED`] subjects are kept.
//! - `concurrency`: at most this many of its runs at once (1 by default for `develop` and `run`, no limit otherwise); the events past
//!   it wait their turn ([`MAX_WAITING`] at most), the rest are skipped.
//! - `cooldownMinutes`: no start sooner than this after its last.
//! - `dailyCap`: at most this many starts a (local) day.
//!
//! The app (src-tauri `automation::policy`) keeps each trigger's [`PolicyState`] in `state.json`, counts its runs, and notes every
//! skip, wait and approval in the event inbox with [`Skip::words`]'s reason.

use std::collections::VecDeque;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::spec::{Action, ActionKind};

/// The most subjects `oncePer` remembers.
pub const MAX_HANDLED: usize = 1_000;
/// The most events one trigger keeps waiting for a run to end.
pub const MAX_WAITING: usize = 10;
pub const MAX_CONCURRENCY: u32 = 10;
/// A week.
pub const MAX_COOLDOWN_MINUTES: u32 = 10_080;
pub const MAX_POLICY_DAILY_CAP: u32 = 100;

/// What `oncePer` counts by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OncePer {
    Subject,
}

/// A trigger's policy; every part optional.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Policy {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approve: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub once_per: Option<OncePer>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub concurrency: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cooldown_minutes: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daily_cap: Option<u32>,
}

impl Policy {
    pub fn is_empty(&self) -> bool {
        *self == Policy::default()
    }

    /// Whether `action` waits for the person's approval: as set, else its kind's default.
    pub fn approves(&self, action: &Action) -> bool {
        self.approve.unwrap_or_else(|| action.approval_by_default())
    }

    /// How many of its runs may go at once: as set, else 1 for `develop` and `run`; None for no limit.
    pub fn concurrency(&self, action: &Action) -> Option<u32> {
        self.concurrency.or(matches!(action.kind, ActionKind::Develop | ActionKind::Run).then_some(1))
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.concurrency.is_some_and(|n| !(1..=MAX_CONCURRENCY).contains(&n)) {
            return Err(format!("policy.concurrency runs from 1 to {MAX_CONCURRENCY}"));
        }
        if self.cooldown_minutes.is_some_and(|n| !(1..=MAX_COOLDOWN_MINUTES).contains(&n)) {
            return Err(format!("policy.cooldownMinutes runs from 1 to {MAX_COOLDOWN_MINUTES}"));
        }
        if self.daily_cap.is_some_and(|n| !(1..=MAX_POLICY_DAILY_CAP).contains(&n)) {
            return Err(format!("policy.dailyCap runs from 1 to {MAX_POLICY_DAILY_CAP}"));
        }
        Ok(())
    }

    /// The policy in a few words (`approve · once per subject · 1 at a time`), as the settings list and the wizard show it.
    pub fn words(&self, action: &Action) -> Vec<String> {
        let mut out = vec![];
        if self.approves(action) {
            out.push("approve".to_string());
        }
        if self.once_per.is_some() {
            out.push("oncePer subject".to_string());
        }
        if let Some(n) = self.concurrency(action) {
            out.push(format!("concurrency {n}"));
        }
        if let Some(n) = self.cooldown_minutes {
            out.push(format!("cooldown {n}m"));
        }
        if let Some(n) = self.daily_cap {
            out.push(format!("dailyCap {n}"));
        }
        out
    }
}

/// What a trigger's policy keeps between events: the subjects it acted on, its last start and its starts today.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PolicyState {
    #[serde(default, skip_serializing_if = "VecDeque::is_empty")]
    handled: VecDeque<String>,
    /// RFC 3339.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    day: String,
    #[serde(default)]
    today: u32,
}

impl PolicyState {
    pub fn handled(&self, subject: &str) -> bool {
        !subject.trim().is_empty() && self.handled.iter().any(|s| s == subject.trim())
    }

    /// `subject` acted on (or awaiting approval): kept, the oldest let go past [`MAX_HANDLED`].
    pub fn remember(&mut self, subject: &str) {
        let subject = subject.trim();
        if subject.is_empty() || self.handled(subject) {
            return;
        }
        self.handled.push_back(subject.to_string());
        while self.handled.len() > MAX_HANDLED {
            self.handled.pop_front();
        }
    }

    /// `subject` let go again (its action could not start, so a later event may try).
    pub fn forget(&mut self, subject: &str) {
        self.handled.retain(|s| s != subject.trim());
    }

    /// An action started at `now` on the local day `day`.
    pub fn started(&mut self, now: DateTime<Utc>, day: &str) {
        self.last = Some(now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
        if self.day != day {
            self.day = day.to_string();
            self.today = 0;
        }
        self.today += 1;
    }

    pub fn starts_on(&self, day: &str) -> u32 {
        if self.day == day { self.today } else { 0 }
    }

    fn last(&self) -> Option<DateTime<Utc>> {
        self.last.as_deref().and_then(|t| DateTime::parse_from_rfc3339(t).ok()).map(|t| t.with_timezone(&Utc))
    }

    pub fn len(&self) -> usize {
        self.handled.len()
    }

    pub fn is_empty(&self) -> bool {
        self.handled.is_empty()
    }
}

/// Why a policy skipped an event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Skip {
    /// `oncePer: subject`: it acted on this subject already.
    Handled(String),
    /// Too soon after its last start: not before this.
    Cooldown(DateTime<Utc>),
    /// Its starts today reached `dailyCap`.
    Capped(u32),
    /// `concurrency` runs going and [`MAX_WAITING`] already waiting.
    Full(u32),
}

impl Skip {
    /// The reason in words (English; the app writes its own language's with the same values).
    pub fn words(&self) -> String {
        match self {
            Skip::Handled(subject) => format!("already handled {subject}"),
            Skip::Cooldown(until) => format!("cooling down until {}", until.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
            Skip::Capped(cap) => format!("daily cap {cap} reached"),
            Skip::Full(n) => format!("{n} running and {MAX_WAITING} waiting"),
        }
    }
}

/// What a policy does with an event that passed the trigger's `where`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Start the action now.
    Go,
    /// Ask the person first.
    Approve,
    /// Wait for one of its runs to end.
    Hold,
    Skip(Skip),
}

/// The policy's verdict on an event about `subject` at `now` (local day `day`), with `running` of the trigger's runs going and
/// `waiting` events waiting. `approved`: the person said 실행 already (only the concurrency counts then).
#[allow(clippy::too_many_arguments, reason = "each is one input of the decision")]
pub fn decide(
    policy: &Policy,
    action: &Action,
    state: &PolicyState,
    subject: &str,
    now: DateTime<Utc>,
    day: &str,
    running: usize,
    waiting: usize,
    approved: bool,
) -> Verdict {
    if !approved {
        if policy.once_per.is_some() && state.handled(subject) {
            return Verdict::Skip(Skip::Handled(subject.trim().to_string()));
        }
        if let (Some(minutes), Some(last)) = (policy.cooldown_minutes, state.last()) {
            let until = last + chrono::Duration::minutes(i64::from(minutes));
            if now < until {
                return Verdict::Skip(Skip::Cooldown(until));
            }
        }
        if let Some(cap) = policy.daily_cap
            && state.starts_on(day) >= cap
        {
            return Verdict::Skip(Skip::Capped(cap));
        }
        if policy.approves(action) {
            return Verdict::Approve;
        }
    }
    match policy.concurrency(action) {
        Some(limit) if running >= limit as usize => {
            if waiting < MAX_WAITING {
                Verdict::Hold
            } else {
                Verdict::Skip(Skip::Full(limit))
            }
        }
        _ => Verdict::Go,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automation::recurrence::tests::utc;
    use crate::automation::spec::With;

    fn action(kind: ActionKind) -> Action {
        Action { prompt: "p".into(), kind, ..Action::default() }
    }

    fn go(policy: &Policy, a: &Action, state: &PolicyState, subject: &str, running: usize) -> Verdict {
        decide(policy, a, state, subject, utc(2026, 11, 26, 10, 0), "2026-11-26", running, 0, false)
    }

    #[test]
    fn approval_is_on_for_run_and_a_merging_develop_unless_set() {
        let none = Policy::default();
        assert!(none.approves(&action(ActionKind::Run)));
        assert!(!none.approves(&action(ActionKind::Develop)));
        let merging = Action { with: With { pr: Some(true), merge: Some(true), ..With::default() }, ..action(ActionKind::Develop) };
        assert!(none.approves(&merging));
        assert!(!none.approves(&action(ActionKind::Ask)) && !none.approves(&action(ActionKind::Emit)));
        let off = Policy { approve: Some(false), ..Policy::default() };
        assert!(!off.approves(&action(ActionKind::Run)), "turned off explicitly");
        let on = Policy { approve: Some(true), ..Policy::default() };
        assert!(on.approves(&action(ActionKind::Ask)));
        assert_eq!(go(&none, &action(ActionKind::Run), &PolicyState::default(), "MC-1", 0), Verdict::Approve);
        assert_eq!(
            decide(&none, &action(ActionKind::Run), &PolicyState::default(), "MC-1", utc(2026, 11, 26, 10, 0), "d", 0, 0, true),
            Verdict::Go,
            "approved: it goes"
        );
    }

    #[test]
    fn once_per_subject_skips_a_subject_handled_and_keeps_the_last_thousand() {
        let policy = Policy { once_per: Some(OncePer::Subject), ..Policy::default() };
        let mut state = PolicyState::default();
        let a = action(ActionKind::Ask);
        assert_eq!(go(&policy, &a, &state, "PR#7", 0), Verdict::Go);
        state.remember("PR#7");
        assert_eq!(go(&policy, &a, &state, "PR#7", 0), Verdict::Skip(Skip::Handled("PR#7".into())));
        assert_eq!(go(&policy, &a, &state, "PR#8", 0), Verdict::Go);
        assert_eq!(go(&Policy::default(), &a, &state, "PR#7", 0), Verdict::Go, "without oncePer the subject is no matter");
        state.forget("PR#7");
        assert_eq!(go(&policy, &a, &state, "PR#7", 0), Verdict::Go, "forgotten after a start that failed");
        for i in 0..MAX_HANDLED + 5 {
            state.remember(&format!("S-{i}"));
        }
        assert_eq!(state.len(), MAX_HANDLED);
        assert!(!state.handled("S-0") && state.handled(&format!("S-{}", MAX_HANDLED + 4)));
        let back: PolicyState = serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
        assert_eq!(back, state);
        assert!(!PolicyState::default().handled(""), "an event without a subject is never handled");
    }

    #[test]
    fn cooldown_and_daily_cap_skip_until_their_time() {
        let a = action(ActionKind::Ask);
        let mut state = PolicyState::default();
        state.started(utc(2026, 11, 26, 9, 50), "2026-11-26");
        let cool = Policy { cooldown_minutes: Some(30), ..Policy::default() };
        assert_eq!(go(&cool, &a, &state, "x", 0), Verdict::Skip(Skip::Cooldown(utc(2026, 11, 26, 10, 20))));
        let later = decide(&cool, &a, &state, "x", utc(2026, 11, 26, 10, 21), "2026-11-26", 0, 0, false);
        assert_eq!(later, Verdict::Go);
        let cap = Policy { daily_cap: Some(2), ..Policy::default() };
        assert_eq!(go(&cap, &a, &state, "x", 0), Verdict::Go);
        state.started(utc(2026, 11, 26, 9, 55), "2026-11-26");
        assert_eq!(go(&cap, &a, &state, "x", 0), Verdict::Skip(Skip::Capped(2)));
        let tomorrow = decide(&cap, &a, &state, "x", utc(2026, 11, 27, 9, 0), "2026-11-27", 0, 0, false);
        assert_eq!(tomorrow, Verdict::Go, "a new day counts again");
        assert!(Skip::Capped(2).words().contains("daily cap 2"));
    }

    #[test]
    fn concurrency_holds_then_skips_and_defaults_to_one_for_develop_and_run() {
        let none = Policy::default();
        let develop = action(ActionKind::Develop);
        assert_eq!(none.concurrency(&develop), Some(1));
        assert_eq!(none.concurrency(&action(ActionKind::Ask)), None);
        assert_eq!(go(&none, &develop, &PolicyState::default(), "a", 0), Verdict::Go);
        assert_eq!(go(&none, &develop, &PolicyState::default(), "a", 1), Verdict::Hold);
        assert_eq!(go(&none, &action(ActionKind::Ask), &PolicyState::default(), "a", 5), Verdict::Go, "no limit");
        let full = decide(&none, &develop, &PolicyState::default(), "a", utc(2026, 11, 26, 10, 0), "d", 1, MAX_WAITING, false);
        assert_eq!(full, Verdict::Skip(Skip::Full(1)));
        let two = Policy { concurrency: Some(2), ..Policy::default() };
        assert_eq!(go(&two, &develop, &PolicyState::default(), "a", 1), Verdict::Go);
        // Approved, it still waits for a free run.
        let run = action(ActionKind::Run);
        assert_eq!(decide(&none, &run, &PolicyState::default(), "a", utc(2026, 11, 26, 10, 0), "d", 1, 0, true), Verdict::Hold);
    }

    #[test]
    fn a_policy_checks_its_ranges_and_keeps_its_json_shape() {
        assert!(Policy::default().validate().is_ok() && Policy::default().is_empty());
        assert!(Policy { concurrency: Some(0), ..Policy::default() }.validate().unwrap_err().contains("concurrency"));
        assert!(Policy { cooldown_minutes: Some(0), ..Policy::default() }.validate().is_err());
        assert!(Policy { daily_cap: Some(101), ..Policy::default() }.validate().is_err());
        let policy: Policy =
            serde_json::from_str(r#"{"approve":true,"oncePer":"subject","concurrency":2,"cooldownMinutes":10,"dailyCap":5}"#).unwrap();
        assert_eq!(policy.once_per, Some(OncePer::Subject));
        assert_eq!(serde_json::from_value::<Policy>(serde_json::to_value(&policy).unwrap()).unwrap(), policy);
        assert!(serde_json::from_str::<Policy>(r#"{"oncePer":"actor"}"#).is_err());
        assert!(serde_json::from_str::<Policy>(r#"{"sneaky":1}"#).is_err(), "unknown keys are refused");
        assert_eq!(
            policy.words(&action(ActionKind::Ask)),
            vec!["approve", "oncePer subject", "concurrency 2", "cooldown 10m", "dailyCap 5"]
        );
    }
}
