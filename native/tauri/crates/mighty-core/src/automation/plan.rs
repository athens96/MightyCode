//! Which schedules are due (MC-32), with the clock given: one pass finds each schedule's most recent fire since the last one it ran
//! (or since it was first seen), so a fire a closed or sleeping app missed runs once (`catchUp: once`) or is passed over (`skip`).
//! The scheduler then sleeps until [`next_wake`]: one timer, no polling.

use std::collections::HashMap;

use chrono::{DateTime, Duration, TimeZone, Utc};
use serde::{Deserialize, Serialize};

use super::recurrence::{Recurrence, Zone};
use super::spec::CatchUp;

/// A fire found this much after its time still counts as on time (the timer wakes a little late, a busy start).
pub const GRACE: Duration = Duration::minutes(2);
/// How far back a missed fire is looked for.
pub const LOOK_BACK: Duration = Duration::days(35);
/// The most steps one schedule's search takes (an every-minute line over the look-back is about 50,000).
const SEARCH_LIMIT: usize = 60_000;

/// A schedule as the planner sees it.
#[derive(Debug, Clone)]
pub struct Slot {
    pub key: String,
    pub recurrence: Recurrence,
    pub catch_up: CatchUp,
    pub enabled: bool,
}

/// What the planner keeps of a schedule between runs (saved).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlotState {
    /// The last fire time handled (run or passed over), epoch ms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_fire: Option<i64>,
    /// When the schedule was first seen (or last turned on), epoch ms: nothing before it is missed.
    pub first_seen: i64,
}

/// A schedule that is due now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Due {
    /// Run it; `late` when its time passed more than [`GRACE`] ago (a catch-up).
    Fire { key: String, at: DateTime<Utc>, late: bool },
    /// Its time passed while the app was closed and it does not catch up.
    Skip { key: String, at: DateTime<Utc> },
}

impl Due {
    pub fn key(&self) -> &str {
        match self {
            Due::Fire { key, .. } | Due::Skip { key, .. } => key,
        }
    }
}

fn instant(ms: i64) -> DateTime<Utc> {
    Utc.timestamp_millis_opt(ms).single().unwrap_or(DateTime::<Utc>::MIN_UTC)
}

/// The time nothing before is missed for this slot.
fn base(state: &SlotState, now: DateTime<Utc>) -> DateTime<Utc> {
    let since = state.last_fire.map_or(state.first_seen, |last| last.max(state.first_seen));
    instant(since).max(now - LOOK_BACK)
}

/// The schedules due at `now`, each at most once (its most recent fire), their state moved on. A slot seen for the first time starts
/// from `now`; a disabled one keeps starting from `now`, so turning it on misses nothing it was off for. States of slots no longer
/// listed are dropped.
pub fn due(slots: &[Slot], states: &mut HashMap<String, SlotState>, now: DateTime<Utc>, zone: &impl Zone) -> Vec<Due> {
    let now_ms = now.timestamp_millis();
    states.retain(|key, _| slots.iter().any(|s| &s.key == key));
    let mut out = vec![];
    for slot in slots {
        let state = states.entry(slot.key.clone()).or_insert(SlotState { last_fire: None, first_seen: now_ms });
        if !slot.enabled {
            *state = SlotState { last_fire: None, first_seen: now_ms };
            continue;
        }
        let Some(at) = slot.recurrence.last_in(base(state, now), now, zone, SEARCH_LIMIT) else { continue };
        state.last_fire = Some(at.timestamp_millis());
        let late = now - at > GRACE;
        out.push(if late && slot.catch_up == CatchUp::Skip {
            Due::Skip { key: slot.key.clone(), at }
        } else {
            Due::Fire { key: slot.key.clone(), at, late }
        });
    }
    out
}

/// Each enabled slot's next fire after `now`.
pub fn next_fires(
    slots: &[Slot],
    states: &HashMap<String, SlotState>,
    now: DateTime<Utc>,
    zone: &impl Zone,
) -> Vec<(String, DateTime<Utc>)> {
    slots
        .iter()
        .filter(|s| s.enabled)
        .filter_map(|slot| {
            let from = states.get(&slot.key).map_or(now, |state| base(state, now).max(now));
            slot.recurrence.next_after(from, zone).map(|at| (slot.key.clone(), at))
        })
        .collect()
}

/// When the scheduler should wake next: the earliest next fire, if any.
pub fn next_wake(slots: &[Slot], states: &HashMap<String, SlotState>, now: DateTime<Utc>, zone: &impl Zone) -> Option<DateTime<Utc>> {
    next_fires(slots, states, now, zone).into_iter().map(|(_, at)| at).min()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automation::recurrence::tests::{NewYork, utc};

    fn slot(key: &str, line: &str, catch_up: CatchUp) -> Slot {
        Slot { key: key.into(), recurrence: Recurrence::cron(line).unwrap(), catch_up, enabled: true }
    }

    #[test]
    fn a_new_schedule_fires_at_its_next_time_not_for_the_past() {
        let slots = vec![slot("nine", "0 9 * * *", CatchUp::Once)];
        let mut states = HashMap::new();
        // Nov 23 10:00 EST: first seen now, so today's 09:00 is not missed.
        let now = utc(2026, 11, 23, 15, 0);
        assert!(due(&slots, &mut states, now, &NewYork).is_empty());
        assert_eq!(next_wake(&slots, &states, now, &NewYork), Some(utc(2026, 11, 24, 14, 0)));
        // The timer wakes at Nov 24 09:00:05.
        let at = utc(2026, 11, 24, 14, 0) + Duration::seconds(5);
        assert_eq!(
            due(&slots, &mut states, at, &NewYork),
            vec![Due::Fire { key: "nine".into(), at: utc(2026, 11, 24, 14, 0), late: false }]
        );
        assert!(due(&slots, &mut states, at, &NewYork).is_empty(), "once");
        assert_eq!(next_wake(&slots, &states, at, &NewYork), Some(utc(2026, 11, 25, 14, 0)));
    }

    #[test]
    fn missed_fires_catch_up_once_for_the_most_recent_or_are_skipped() {
        let slots = vec![slot("once", "0 9 * * *", CatchUp::Once), slot("skip", "0 9 * * *", CatchUp::Skip)];
        let mut states = HashMap::new();
        due(&slots, &mut states, utc(2026, 11, 20, 15, 0), &NewYork);
        // The app was closed from Nov 20 until Nov 23 12:00: Nov 21, 22 and 23 09:00 were missed; only Nov 23's counts.
        let now = utc(2026, 11, 23, 17, 0);
        let found = due(&slots, &mut states, now, &NewYork);
        assert_eq!(
            found,
            vec![
                Due::Fire { key: "once".into(), at: utc(2026, 11, 23, 14, 0), late: true },
                Due::Skip { key: "skip".into(), at: utc(2026, 11, 23, 14, 0) },
            ]
        );
        assert!(due(&slots, &mut states, now, &NewYork).is_empty());
        // Within the grace a skip-schedule still runs (a timer that woke a minute late).
        let on_time = utc(2026, 11, 24, 14, 1);
        assert!(matches!(
            due(&slots, &mut states, on_time, &NewYork).as_slice(),
            [Due::Fire { late: false, .. }, Due::Fire { late: false, .. }]
        ));
    }

    #[test]
    fn a_disabled_schedule_misses_nothing_it_was_off_for_and_removed_ones_are_forgotten() {
        let mut slots = vec![slot("nine", "0 9 * * *", CatchUp::Once)];
        let mut states = HashMap::new();
        due(&slots, &mut states, utc(2026, 11, 20, 15, 0), &NewYork);
        slots[0].enabled = false;
        assert!(due(&slots, &mut states, utc(2026, 11, 22, 15, 0), &NewYork).is_empty());
        assert_eq!(next_wake(&slots, &states, utc(2026, 11, 22, 15, 0), &NewYork), None);
        slots[0].enabled = true;
        assert!(due(&slots, &mut states, utc(2026, 11, 22, 16, 0), &NewYork).is_empty(), "turned on after 09:00: nothing missed");
        slots.clear();
        due(&slots, &mut states, utc(2026, 11, 22, 16, 0), &NewYork);
        assert!(states.is_empty());
    }

    #[test]
    fn a_very_old_last_fire_looks_back_a_bounded_time() {
        let slots = vec![slot("minute", "* * * * *", CatchUp::Once)];
        let mut states = HashMap::new();
        states.insert("minute".into(), SlotState { last_fire: Some(0), first_seen: 0 });
        let now = utc(2026, 11, 23, 15, 0);
        assert_eq!(due(&slots, &mut states, now, &NewYork), vec![Due::Fire { key: "minute".into(), at: now, late: false }]);
    }
}
