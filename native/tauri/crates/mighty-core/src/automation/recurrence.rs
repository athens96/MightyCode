//! When a schedule fires (MC-32): a 5-field cron line or a small RRULE subset, both read into one wall-clock matcher, and the next
//! instant it names after a given one in a time zone, DST included.
//!
//! - Cron: `minute hour day-of-month month day-of-week`, each `*`, a number, `a-b`, `*/n`, `a-b/n` or a comma list of them; months
//!   and weekdays may be names (`JAN`, `MON`); 7 is Sunday too. When both day fields are restricted a day matching either fires
//!   (cron's own rule). `@daily`, `@hourly`, `@weekly` are read too.
//! - RRULE: `FREQ=DAILY|WEEKLY|HOURLY` with `BYHOUR`, `BYMINUTE` and `BYDAY` (`MO,TU,…`), optionally behind `RRULE:`. With no
//!   start date to take a time from, BYMINUTE defaults to 0 and a daily or weekly rule's BYHOUR to 0; WEEKLY needs BYDAY.
//! - DST: a wall-clock time the clocks skip (spring forward) fires at the first instant after the gap, once; a time that happens
//!   twice (fall back) fires once, at its first occurrence.

use chrono::{DateTime, Datelike, Duration, Local, MappedLocalTime, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Timelike, Utc};

/// A time zone as the calculator needs it: the instants a wall-clock time names, and the wall clock at an instant.
pub trait Zone {
    fn resolve(&self, wall: NaiveDateTime) -> MappedLocalTime<DateTime<Utc>>;
    fn wall(&self, at: DateTime<Utc>) -> NaiveDateTime;
}

/// The computer's own time zone.
#[derive(Debug, Clone, Copy, Default)]
pub struct LocalZone;

impl Zone for LocalZone {
    fn resolve(&self, wall: NaiveDateTime) -> MappedLocalTime<DateTime<Utc>> {
        Local.from_local_datetime(&wall).map(|t| t.with_timezone(&Utc))
    }

    fn wall(&self, at: DateTime<Utc>) -> NaiveDateTime {
        at.with_timezone(&Local).naive_local()
    }
}

/// The wall-clock fields a schedule fires on, as bit sets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recurrence {
    minutes: u64,
    hours: u32,
    /// Bits 1..=31.
    days: u32,
    /// Bits 1..=12.
    months: u16,
    /// Bits 0..=6, Sunday first.
    weekdays: u8,
    days_any: bool,
    weekdays_any: bool,
}

/// How far ahead [`Recurrence::next_after`] looks before it gives up (a line like `0 0 30 2 *` never fires).
const LOOK_AHEAD_DAYS: i64 = 366 * 5;

const MONTH_NAMES: [&str; 12] = ["JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC"];
const DAY_NAMES: [&str; 7] = ["SUN", "MON", "TUE", "WED", "THU", "FRI", "SAT"];
const RRULE_DAYS: [&str; 7] = ["SU", "MO", "TU", "WE", "TH", "FR", "SA"];

fn bits(values: impl IntoIterator<Item = u32>) -> u64 {
    values.into_iter().fold(0u64, |acc, v| acc | (1u64 << v))
}

/// One cron field read into a bit set; whether it was `*`.
fn field(text: &str, min: u32, max: u32, names: &[&str], name_base: u32, what: &str) -> Result<(u64, bool), String> {
    let text = text.trim();
    if text.is_empty() {
        return Err(format!("the {what} field is empty"));
    }
    let value = |raw: &str| -> Result<u32, String> {
        let upper = raw.to_ascii_uppercase();
        if let Some(at) = names.iter().position(|n| *n == upper) {
            return Ok(at as u32 + name_base);
        }
        raw.parse::<u32>().map_err(|_| format!("'{raw}' is not a {what}"))
    };
    let mut set = 0u64;
    for part in text.split(',') {
        let (range, step) = match part.split_once('/') {
            Some((range, step)) => {
                let step: u32 = step.parse().map_err(|_| format!("'{step}' is not a step"))?;
                if step == 0 {
                    return Err(format!("a step of 0 in the {what} field"));
                }
                (range, step)
            }
            None => (part, 1),
        };
        let (from, to) = if range == "*" {
            (min, max)
        } else if let Some((a, b)) = range.split_once('-') {
            (value(a)?, value(b)?)
        } else {
            let one = value(range)?;
            // `5/15` runs from 5 to the end, as cron reads it.
            (one, if part.contains('/') { max } else { one })
        };
        // 7 is Sunday as well as 0.
        let (from, to) = if what == "day of the week" && to == 7 && from <= 7 { (from, 7) } else { (from, to) };
        let top = if what == "day of the week" { 7 } else { max };
        if from < min || to > top || from > to {
            return Err(format!("'{range}' is out of range for the {what} field ({min}-{max})"));
        }
        let mut v = from;
        while v <= to {
            set |= 1u64 << if what == "day of the week" && v == 7 { 0 } else { v };
            v += step;
        }
    }
    Ok((set, text == "*" || text == "*/1"))
}

impl Recurrence {
    /// A 5-field cron line (or `@hourly`, `@daily`, `@weekly`, `@monthly`).
    pub fn cron(line: &str) -> Result<Recurrence, String> {
        let line = match line.trim() {
            "@hourly" => "0 * * * *",
            "@daily" | "@midnight" => "0 0 * * *",
            "@weekly" => "0 0 * * 0",
            "@monthly" => "0 0 1 * *",
            other => other,
        };
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() != 5 {
            return Err(format!("a cron line has 5 fields, not {}", parts.len()));
        }
        let (minutes, _) = field(parts[0], 0, 59, &[], 0, "minute")?;
        let (hours, _) = field(parts[1], 0, 23, &[], 0, "hour")?;
        let (days, days_any) = field(parts[2], 1, 31, &[], 0, "day of the month")?;
        let (months, _) = field(parts[3], 1, 12, &MONTH_NAMES, 1, "month")?;
        let (weekdays, weekdays_any) = field(parts[4], 0, 6, &DAY_NAMES, 0, "day of the week")?;
        Ok(Recurrence {
            minutes,
            hours: hours as u32,
            days: days as u32,
            months: months as u16,
            weekdays: weekdays as u8,
            days_any,
            weekdays_any,
        })
    }

    /// The RRULE subset: `FREQ=DAILY|WEEKLY|HOURLY;BYHOUR=…;BYMINUTE=…;BYDAY=…`.
    pub fn rrule(text: &str) -> Result<Recurrence, String> {
        let text = text.trim();
        let text = text.strip_prefix("RRULE:").unwrap_or(text);
        let (mut freq, mut by_hour, mut by_minute, mut by_day) = (None, None, None, None);
        for part in text.split(';').map(str::trim).filter(|p| !p.is_empty()) {
            let (key, value) = part.split_once('=').ok_or_else(|| format!("'{part}' is not KEY=VALUE"))?;
            let numbers = |max: u32, what: &str| -> Result<Vec<u32>, String> {
                value
                    .split(',')
                    .map(|v| v.trim().parse::<u32>().ok().filter(|n| *n <= max).ok_or_else(|| format!("'{v}' is not a {what}")))
                    .collect()
            };
            match key.trim().to_ascii_uppercase().as_str() {
                "FREQ" => freq = Some(value.trim().to_ascii_uppercase()),
                "BYHOUR" => by_hour = Some(numbers(23, "BYHOUR value")?),
                "BYMINUTE" => by_minute = Some(numbers(59, "BYMINUTE value")?),
                "BYDAY" => {
                    let days: Result<Vec<u32>, String> = value
                        .split(',')
                        .map(|d| {
                            let d = d.trim().to_ascii_uppercase();
                            RRULE_DAYS.iter().position(|n| *n == d).map(|i| i as u32).ok_or_else(|| format!("'{d}' is not a BYDAY value"))
                        })
                        .collect();
                    by_day = Some(days?);
                }
                "INTERVAL" if value.trim() == "1" => {}
                other => return Err(format!("{other} is not supported (FREQ, BYHOUR, BYMINUTE, BYDAY)")),
            }
        }
        let freq = freq.ok_or("FREQ is missing")?;
        let all_hours: Vec<u32> = (0..24).collect();
        let hours = match freq.as_str() {
            "HOURLY" => by_hour.unwrap_or(all_hours),
            "DAILY" | "WEEKLY" => by_hour.unwrap_or_else(|| vec![0]),
            other => return Err(format!("FREQ={other} is not supported (DAILY, WEEKLY, HOURLY)")),
        };
        if freq == "WEEKLY" && by_day.as_ref().is_none_or(Vec::is_empty) {
            return Err("FREQ=WEEKLY needs BYDAY".into());
        }
        let minutes = by_minute.unwrap_or_else(|| vec![0]);
        if hours.is_empty() || minutes.is_empty() {
            return Err("an empty BYHOUR or BYMINUTE".into());
        }
        let weekdays_any = by_day.is_none();
        Ok(Recurrence {
            minutes: bits(minutes),
            hours: bits(hours) as u32,
            days: bits(1..=31) as u32,
            months: bits(1..=12) as u16,
            weekdays: bits(by_day.unwrap_or_else(|| (0..7).collect())) as u8,
            days_any: true,
            weekdays_any,
        })
    }

    /// A schedule's `cron` or `rrule`, exactly one of them.
    pub fn parse(cron: Option<&str>, rrule: Option<&str>) -> Result<Recurrence, String> {
        let cron = cron.map(str::trim).filter(|c| !c.is_empty());
        let rrule = rrule.map(str::trim).filter(|r| !r.is_empty());
        match (cron, rrule) {
            (Some(line), None) => Recurrence::cron(line),
            (None, Some(rule)) => Recurrence::rrule(rule),
            (Some(_), Some(_)) => Err("give either cron or rrule, not both".into()),
            (None, None) => Err("give cron or rrule".into()),
        }
    }

    fn month_matches(&self, month: u32) -> bool {
        self.months & (1 << month) != 0
    }

    fn day_matches(&self, date: NaiveDate) -> bool {
        let dom = self.days & (1 << date.day()) != 0;
        let dow = self.weekdays & (1 << date.weekday().num_days_from_sunday()) != 0;
        match (self.days_any, self.weekdays_any) {
            (true, true) => true,
            (true, false) => dow,
            (false, true) => dom,
            (false, false) => dom || dow,
        }
    }

    /// The first wall-clock minute at or after `from` that matches, within the look-ahead.
    fn next_wall(&self, from: NaiveDateTime) -> Option<NaiveDateTime> {
        let mut at = from.with_second(0)?.with_nanosecond(0)?;
        let end = at + Duration::days(LOOK_AHEAD_DAYS);
        while at < end {
            let date = at.date();
            if !self.month_matches(date.month()) {
                let (y, m) = if date.month() == 12 { (date.year() + 1, 1) } else { (date.year(), date.month() + 1) };
                at = NaiveDate::from_ymd_opt(y, m, 1)?.and_time(NaiveTime::MIN);
                continue;
            }
            if !self.day_matches(date) {
                at = date.succ_opt()?.and_time(NaiveTime::MIN);
                continue;
            }
            if self.hours & (1 << at.hour()) == 0 {
                at = date.and_hms_opt(at.hour(), 0, 0)? + Duration::hours(1);
                continue;
            }
            if self.minutes & (1 << at.minute()) == 0 {
                at += Duration::minutes(1);
                continue;
            }
            return Some(at);
        }
        None
    }

    /// The first instant this fires strictly after `after`, in `zone`. None when nothing matches within five years.
    pub fn next_after(&self, after: DateTime<Utc>, zone: &impl Zone) -> Option<DateTime<Utc>> {
        // The wall clock at `after`, one minute on; an instant at or before `after` (a repeated hour's first pass) is passed over.
        let mut from = zone.wall(after).with_second(0)?.with_nanosecond(0)? + Duration::minutes(1);
        let end = from + Duration::days(LOOK_AHEAD_DAYS);
        while from < end {
            let wall = self.next_wall(from)?;
            let instant = match zone.resolve(wall) {
                MappedLocalTime::Single(t) => Some(t),
                // Fall back: the first of the two.
                MappedLocalTime::Ambiguous(first, _) => Some(first),
                // Spring forward: the first instant after the gap (the wall time an hour on, minutes kept, else the gap's end).
                MappedLocalTime::None => gap_end(wall, zone),
            };
            if let Some(t) = instant
                && t > after
            {
                return Some(t);
            }
            from = wall + Duration::minutes(1);
        }
        None
    }

    /// The last instant this fires in `(after, until]`, stepping forward from `after` (at most `limit` steps).
    pub fn last_in(&self, after: DateTime<Utc>, until: DateTime<Utc>, zone: &impl Zone, limit: usize) -> Option<DateTime<Utc>> {
        let mut last = None;
        let mut at = after;
        for _ in 0..limit {
            match self.next_after(at, zone) {
                Some(next) if next <= until => {
                    last = Some(next);
                    at = next;
                }
                _ => break,
            }
        }
        last
    }
}

/// The first instant after a spring-forward gap that swallowed `wall`.
fn gap_end(wall: NaiveDateTime, zone: &impl Zone) -> Option<DateTime<Utc>> {
    let mut probe = wall;
    // Gaps are at most a few hours; minute steps find the first wall time that exists.
    for _ in 0..(4 * 60) {
        probe += Duration::minutes(1);
        match zone.resolve(probe) {
            MappedLocalTime::Single(t) | MappedLocalTime::Ambiguous(t, _) => return Some(t),
            MappedLocalTime::None => continue,
        }
    }
    None
}

#[cfg(test)]
pub(crate) mod tests {
    use chrono::FixedOffset;

    use super::*;

    /// A zone with US-style DST for tests: UTC−5, and UTC−4 from 2026-03-08 02:00 to 2026-11-01 02:00 local.
    pub(crate) struct NewYork;

    impl NewYork {
        fn spring() -> DateTime<Utc> {
            // 2026-03-08 02:00 EST = 07:00 UTC.
            Utc.with_ymd_and_hms(2026, 3, 8, 7, 0, 0).unwrap()
        }
        fn fall() -> DateTime<Utc> {
            // 2026-11-01 02:00 EDT = 06:00 UTC.
            Utc.with_ymd_and_hms(2026, 11, 1, 6, 0, 0).unwrap()
        }
        fn offset_at(at: DateTime<Utc>) -> FixedOffset {
            if at >= NewYork::spring() && at < NewYork::fall() {
                FixedOffset::west_opt(4 * 3600).unwrap()
            } else {
                FixedOffset::west_opt(5 * 3600).unwrap()
            }
        }
    }

    impl Zone for NewYork {
        fn resolve(&self, wall: NaiveDateTime) -> MappedLocalTime<DateTime<Utc>> {
            let candidates: Vec<DateTime<Utc>> = [4, 5]
                .into_iter()
                .map(|h| Utc.from_utc_datetime(&(wall + Duration::hours(h))))
                .filter(|t| NewYork::offset_at(*t).local_minus_utc() == -(((t.naive_utc() - wall).num_hours() as i32) * 3600))
                .collect();
            match candidates.as_slice() {
                [] => MappedLocalTime::None,
                [one] => MappedLocalTime::Single(*one),
                [a, b] => MappedLocalTime::Ambiguous(*a.min(b), *a.max(b)),
                _ => unreachable!(),
            }
        }
        fn wall(&self, at: DateTime<Utc>) -> NaiveDateTime {
            at.with_timezone(&NewYork::offset_at(at)).naive_local()
        }
    }

    pub(crate) fn wall(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, mo, d).unwrap().and_hms_opt(h, mi, 0).unwrap()
    }

    pub(crate) fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
    }

    /// The next `n` fires after the wall time `from`, as wall times in New York.
    fn fires(rule: &Recurrence, from: DateTime<Utc>, n: usize) -> Vec<DateTime<Utc>> {
        let mut out = vec![];
        let mut at = from;
        for _ in 0..n {
            let Some(next) = rule.next_after(at, &NewYork) else { break };
            out.push(next);
            at = next;
        }
        out
    }

    #[test]
    fn cron_lines_read_every_field_form() {
        let every_15 = Recurrence::cron("*/15 9-17 * * MON-FRI").unwrap();
        // 2026-11-23 is a Monday; 08:59 EST = 13:59 UTC.
        let got = fires(&every_15, utc(2026, 11, 23, 13, 59), 3);
        assert_eq!(got, vec![utc(2026, 11, 23, 14, 0), utc(2026, 11, 23, 14, 15), utc(2026, 11, 23, 14, 30)]);
        // Friday 17:45 EST, then Monday 09:00.
        let got = fires(&every_15, utc(2026, 11, 27, 22, 45), 1);
        assert_eq!(got, vec![utc(2026, 11, 30, 14, 0)]);
        assert!(Recurrence::cron("0 9 * * 7").unwrap() == Recurrence::cron("0 9 * * 0").unwrap(), "7 is Sunday");
        assert!(Recurrence::cron("0 9 * jan,Feb SUN").is_ok());
        assert!(Recurrence::cron("5/20 * * * *").is_ok());
        for bad in ["* * * *", "60 * * * *", "* 24 * * *", "* * 0 * *", "* * * 13 *", "*/0 * * * *", "a * * * *", "5-1 * * * *", ""] {
            assert!(Recurrence::cron(bad).is_err(), "{bad}");
        }
        assert_eq!(Recurrence::cron("@daily").unwrap(), Recurrence::cron("0 0 * * *").unwrap());
    }

    #[test]
    fn both_day_fields_restricted_fire_on_either() {
        // The 1st, or any Monday.
        let rule = Recurrence::cron("0 9 1 * 1").unwrap();
        // From 2026-11-28 (Sat): Mon Nov 30, then Tue Dec 1, then Mon Dec 7.
        let got = fires(&rule, utc(2026, 11, 28, 15, 0), 3);
        assert_eq!(got, vec![utc(2026, 11, 30, 14, 0), utc(2026, 12, 1, 14, 0), utc(2026, 12, 7, 14, 0)]);
    }

    #[test]
    fn month_ends_fire_only_in_months_that_have_the_day() {
        let rule = Recurrence::cron("0 12 31 * *").unwrap();
        let got = fires(&rule, utc(2026, 1, 1, 0, 0), 4);
        let days: Vec<(u32, u32)> = got.iter().map(|t| (t.month(), t.day())).collect();
        assert_eq!(days, vec![(1, 31), (3, 31), (5, 31), (7, 31)]);
        // February 29th only in a leap year.
        let leap = Recurrence::cron("0 0 29 2 *").unwrap();
        let got = leap.next_after(utc(2026, 3, 1, 0, 0), &NewYork).unwrap();
        assert_eq!((got.year(), NewYork.wall(got).month(), NewYork.wall(got).day()), (2028, 2, 29));
        assert_eq!(Recurrence::cron("0 0 30 2 *").unwrap().next_after(utc(2026, 1, 1, 0, 0), &NewYork), None, "never");
        // The year's end.
        let got = Recurrence::cron("30 23 31 12 *").unwrap().next_after(utc(2026, 6, 1, 0, 0), &NewYork).unwrap();
        assert_eq!(NewYork.wall(got), wall(2026, 12, 31, 23, 30));
    }

    #[test]
    fn a_skipped_hour_fires_once_right_after_the_gap() {
        // 02:30 does not exist on 2026-03-08 in New York: it fires at 03:00 EDT (07:00 UTC), once, then 02:30 the next day.
        let rule = Recurrence::cron("30 2 * * *").unwrap();
        let got = fires(&rule, utc(2026, 3, 7, 12, 0), 3);
        assert_eq!(got[0], utc(2026, 3, 8, 7, 0));
        assert_eq!(NewYork.wall(got[0]), wall(2026, 3, 8, 3, 0));
        assert_eq!(NewYork.wall(got[1]), wall(2026, 3, 9, 2, 30));
        // Every 15 minutes through the gap: 01:45 EST, then 03:00 EDT (the gap's minutes fold into one fire), 03:15.
        let rule = Recurrence::cron("*/15 * * * *").unwrap();
        let got = fires(&rule, utc(2026, 3, 8, 6, 40), 3);
        let walls: Vec<NaiveDateTime> = got.iter().map(|t| NewYork.wall(*t)).collect();
        assert_eq!(walls, vec![wall(2026, 3, 8, 1, 45), wall(2026, 3, 8, 3, 0), wall(2026, 3, 8, 3, 15)]);
    }

    #[test]
    fn a_repeated_hour_fires_once() {
        // 01:30 happens twice on 2026-11-01: it fires at the first (EDT, 05:30 UTC) only.
        let rule = Recurrence::cron("30 1 * * *").unwrap();
        let got = fires(&rule, utc(2026, 10, 31, 12, 0), 2);
        assert_eq!(got[0], utc(2026, 11, 1, 5, 30));
        assert_eq!(NewYork.wall(got[1]), wall(2026, 11, 2, 1, 30));
        // Started during the second pass (01:10 EST = 06:10 UTC): the day's 01:30 already fired, the next is tomorrow's.
        let got = rule.next_after(utc(2026, 11, 1, 6, 10), &NewYork).unwrap();
        assert_eq!(NewYork.wall(got), wall(2026, 11, 2, 1, 30));
        // A daily 09:00 keeps its wall time across both changes.
        let nine = Recurrence::cron("0 9 * * *").unwrap();
        let walls: Vec<NaiveDateTime> = fires(&nine, utc(2026, 10, 30, 20, 0), 4).iter().map(|t| NewYork.wall(*t)).collect();
        assert_eq!(walls, vec![wall(2026, 10, 31, 9, 0), wall(2026, 11, 1, 9, 0), wall(2026, 11, 2, 9, 0), wall(2026, 11, 3, 9, 0)]);
    }

    #[test]
    fn rrules_read_their_subset() {
        let daily = Recurrence::rrule("RRULE:FREQ=DAILY;BYHOUR=9;BYMINUTE=0").unwrap();
        assert_eq!(daily, Recurrence::cron("0 9 * * *").unwrap());
        let weekly = Recurrence::rrule("FREQ=WEEKLY;BYDAY=MO,FR;BYHOUR=8,17;BYMINUTE=30").unwrap();
        assert_eq!(weekly, Recurrence::cron("30 8,17 * * 1,5").unwrap());
        let hourly = Recurrence::rrule("FREQ=HOURLY;BYMINUTE=15").unwrap();
        assert_eq!(hourly, Recurrence::cron("15 * * * *").unwrap());
        let weekdays = Recurrence::rrule("FREQ=DAILY;BYDAY=MO,TU,WE,TH,FR;BYHOUR=9").unwrap();
        assert_eq!(weekdays, Recurrence::cron("0 9 * * 1-5").unwrap());
        for bad in ["FREQ=WEEKLY;BYHOUR=9", "FREQ=MONTHLY", "BYHOUR=9", "FREQ=DAILY;BYHOUR=24", "FREQ=DAILY;BYDAY=XX", "FREQ=DAILY;COUNT=3"]
        {
            assert!(Recurrence::rrule(bad).is_err(), "{bad}");
        }
        assert!(Recurrence::parse(Some("0 9 * * *"), Some("FREQ=DAILY")).is_err());
        assert!(Recurrence::parse(None, None).is_err());
        assert!(Recurrence::parse(Some(" "), Some("FREQ=HOURLY")).is_ok());
    }

    #[test]
    fn the_last_fire_in_a_window_is_found() {
        let nine = Recurrence::cron("0 9 * * *").unwrap();
        // From Nov 20 noon to Nov 23 08:00 (EST): Nov 21, 22 fired, the last is Nov 22 09:00.
        let last = nine.last_in(utc(2026, 11, 20, 17, 0), utc(2026, 11, 23, 13, 0), &NewYork, 1000).unwrap();
        assert_eq!(NewYork.wall(last), wall(2026, 11, 22, 9, 0));
        assert_eq!(nine.last_in(utc(2026, 11, 23, 14, 0), utc(2026, 11, 23, 15, 0), &NewYork, 1000), None);
    }

    #[test]
    fn the_local_zone_resolves_today() {
        let rule = Recurrence::cron("* * * * *").unwrap();
        let now = Utc::now();
        let next = rule.next_after(now, &LocalZone).unwrap();
        assert!(next > now && next - now <= Duration::minutes(2));
    }
}
