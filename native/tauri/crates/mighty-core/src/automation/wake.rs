//! MC-39: schedules that fire while the app is closed. With 앱이 꺼져 있어도 예약 실행 on (off by default) the app keeps ONE OS wake-up
//! registered for the earliest next fire across all schedules; the OS then starts the app without a window
//! (`--server --for-schedules`), the scheduler's catch-up fires what is due, and the app quits once nothing has run for
//! [`IDLE_EXIT`]. Pure parts only (the app's `automation::wake` writes the files and runs `launchctl` / `schtasks`):
//!
//! - [`WakeTime`]: the wall-clock minute the OS starts the app at, read from a fire instant in a zone (DST and month ends come from
//!   the recurrence's own instants).
//! - [`launch_agent_plist`]: the Mac's LaunchAgent (`StartCalendarInterval` for that minute; launchd runs a calendar job missed while
//!   the Mac slept once it wakes), no `KeepAlive`, no `RunAtLoad`.
//! - [`task_xml`] and [`schtasks_create`]: Windows' Task Scheduler task (a `TimeTrigger` at that local time, `StartWhenAvailable`,
//!   no time limit, the current user's interactive token, never elevated), registered with `schtasks /Create /XML`: `/SD`'s date
//!   follows the system's locale and `/TR` holds at most 261 characters, so the task is described in XML instead.
//! - [`decide`]: register, remove or leave the registration as it is.
//! - [`IdleClock`]: when a server started for schedules has been idle long enough to quit.

use std::time::{Duration, Instant};

use chrono::{DateTime, Datelike, Timelike, Utc};
use serde::{Deserialize, Serialize};

use super::recurrence::Zone;

/// The app's switch beside `--server`: started by the OS for a schedule.
pub const SWITCH: &str = "--for-schedules";
/// The variable the Mac's LaunchAgent sets: the process launchd started (which leaves at once; see the app's `automation::wake`).
pub const LAUNCHD_MARK: &str = "MIGHTY_CODE_SCHEDULE_WAKE";
/// How long a server started for schedules stays once nothing runs.
pub const IDLE_EXIT: Duration = Duration::from_secs(5 * 60);
/// The least time between two registrations of a later wake while the app runs (an every-minute schedule would otherwise register
/// every minute): an earlier or missing one is always registered at once.
pub const REREGISTER_EVERY: chrono::Duration = chrono::Duration::minutes(10);

/// The wall-clock minute the OS starts the app at (the computer's own time zone).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WakeTime {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
}

impl WakeTime {
    /// The wall clock at `at` in `zone`, to the minute (a fire the recurrence moved past a spring-forward gap is already the gap's
    /// end; a repeated hour's first pass is its first instant).
    pub fn of(at: DateTime<Utc>, zone: &impl Zone) -> WakeTime {
        let wall = zone.wall(at);
        WakeTime { year: wall.year(), month: wall.month(), day: wall.day(), hour: wall.hour(), minute: wall.minute() }
    }

    /// `2026-11-24T09:00:00`: a local time without an offset (the Task Scheduler's `StartBoundary`).
    pub fn local_iso(&self) -> String {
        format!("{:04}-{:02}-{:02}T{:02}:{:02}:00", self.year, self.month, self.day, self.hour, self.minute)
    }
}

/// Text inside an XML element or attribute.
pub fn xml(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;")
}

/// The Mac's LaunchAgent for one wake: `arguments` the program and its arguments, `at` the wall-clock minute. launchd's calendar has
/// no year: the app registers again at each start, so a wake a year early only starts an app that finds nothing due.
pub fn launch_agent_plist(label: &str, arguments: &[String], at: WakeTime) -> String {
    let strings: String = arguments.iter().map(|argument| format!("\t\t<string>{}</string>\n", xml(argument))).collect();
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n\
         <dict>\n\
         \t<key>Label</key>\n\
         \t<string>{label}</string>\n\
         \t<key>ProgramArguments</key>\n\
         \t<array>\n\
         {strings}\
         \t</array>\n\
         \t<key>EnvironmentVariables</key>\n\
         \t<dict>\n\
         \t\t<key>{LAUNCHD_MARK}</key>\n\
         \t\t<string>1</string>\n\
         \t</dict>\n\
         \t<key>StartCalendarInterval</key>\n\
         \t<dict>\n\
         \t\t<key>Month</key>\n\
         \t\t<integer>{}</integer>\n\
         \t\t<key>Day</key>\n\
         \t\t<integer>{}</integer>\n\
         \t\t<key>Hour</key>\n\
         \t\t<integer>{}</integer>\n\
         \t\t<key>Minute</key>\n\
         \t\t<integer>{}</integer>\n\
         \t</dict>\n\
         \t<key>ProcessType</key>\n\
         \t<string>Interactive</string>\n\
         </dict>\n\
         </plist>\n",
        at.month,
        at.day,
        at.hour,
        at.minute,
        label = xml(label),
    )
}

/// One argument as the Microsoft C runtime (and `CommandLineToArgvW`) splits it back (mighty-platform-windows' `quote_windows`, MC-35's
/// rule): as it is when it is not empty and holds no white space or quote; else in quotes, a quote escaped, and a backslash run
/// doubled before a quote or the closing quote (`D:\my work\` keeps its closing quote).
pub fn quote_windows(argument: &str) -> String {
    if !argument.is_empty() && !argument.chars().any(|c| c.is_whitespace() || c == '"') {
        return argument.to_string();
    }
    let mut result = String::from("\"");
    let mut slashes = 0usize;
    for c in argument.chars() {
        if c == '\\' {
            slashes += 1;
            continue;
        }
        let run = if c == '"' { slashes * 2 + 1 } else { slashes };
        result.extend(std::iter::repeat_n('\\', run));
        slashes = 0;
        result.push(c);
    }
    result.extend(std::iter::repeat_n('\\', slashes * 2));
    result.push('"');
    result
}

/// The Task Scheduler's task for one wake: `program` started with `arguments` at the local time `at`, as the current user (its
/// interactive token, the least privilege), started late when the computer was off or asleep then, one instance at a time, on battery
/// too, never stopped for running long.
pub fn task_xml(program: &str, arguments: &[String], at: WakeTime) -> String {
    let line = arguments.iter().map(|a| quote_windows(a)).collect::<Vec<_>>().join(" ");
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-16\"?>\r\n\
         <Task version=\"1.2\" xmlns=\"http://schemas.microsoft.com/windows/2004/02/mit/task\">\r\n\
         \x20 <RegistrationInfo>\r\n\
         \x20   <Description>Mighty Code: starts the app without a window for its next schedule.</Description>\r\n\
         \x20 </RegistrationInfo>\r\n\
         \x20 <Triggers>\r\n\
         \x20   <TimeTrigger>\r\n\
         \x20     <StartBoundary>{}</StartBoundary>\r\n\
         \x20     <Enabled>true</Enabled>\r\n\
         \x20   </TimeTrigger>\r\n\
         \x20 </Triggers>\r\n\
         \x20 <Principals>\r\n\
         \x20   <Principal id=\"Author\">\r\n\
         \x20     <LogonType>InteractiveToken</LogonType>\r\n\
         \x20     <RunLevel>LeastPrivilege</RunLevel>\r\n\
         \x20   </Principal>\r\n\
         \x20 </Principals>\r\n\
         \x20 <Settings>\r\n\
         \x20   <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>\r\n\
         \x20   <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>\r\n\
         \x20   <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>\r\n\
         \x20   <StartWhenAvailable>true</StartWhenAvailable>\r\n\
         \x20   <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>\r\n\
         \x20   <RunOnlyIfIdle>false</RunOnlyIfIdle>\r\n\
         \x20   <WakeToRun>false</WakeToRun>\r\n\
         \x20   <AllowStartOnDemand>true</AllowStartOnDemand>\r\n\
         \x20   <Enabled>true</Enabled>\r\n\
         \x20   <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>\r\n\
         \x20 </Settings>\r\n\
         \x20 <Actions Context=\"Author\">\r\n\
         \x20   <Exec>\r\n\
         \x20     <Command>{}</Command>\r\n\
         \x20     <Arguments>{}</Arguments>\r\n\
         \x20   </Exec>\r\n\
         \x20 </Actions>\r\n\
         </Task>\r\n",
        at.local_iso(),
        xml(program),
        xml(&line),
    )
}

/// The task XML as `schtasks /XML` reads it: UTF-16 little-endian with its byte-order mark.
pub fn utf16_file(text: &str) -> Vec<u8> {
    let mut bytes = vec![0xFF, 0xFE];
    for unit in text.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    bytes
}

/// `schtasks`'s arguments that register (or replace) the task `name` from the XML file `file`.
pub fn schtasks_create(name: &str, file: &str) -> Vec<String> {
    ["/Create", "/TN", name, "/XML", file, "/F"].into_iter().map(String::from).collect()
}

/// `schtasks`'s arguments that remove the task `name`.
pub fn schtasks_delete(name: &str) -> Vec<String> {
    ["/Delete", "/TN", name, "/F"].into_iter().map(String::from).collect()
}

/// `schtasks`'s arguments that ask whether the task `name` is there.
pub fn schtasks_query(name: &str) -> Vec<String> {
    ["/Query", "/TN", name].into_iter().map(String::from).collect()
}

/// What is registered with the OS now (kept in `<data>/automation/wake.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Registration {
    /// The fire instant the wake is for (epoch ms in the file).
    #[serde(with = "epoch_ms")]
    pub at: DateTime<Utc>,
    /// The LaunchAgent's label or the task's name.
    pub name: String,
    /// The program and its arguments.
    pub command: Vec<String>,
    #[serde(with = "epoch_ms")]
    pub registered_at: DateTime<Utc>,
}

/// How the last start by the OS went.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WakeOutcome {
    /// Still running.
    Running,
    /// Quit after [`IDLE_EXIT`] with nothing running.
    Idle,
    /// Kept running (the person showed its window, or it is the app's login server too).
    Stayed,
    /// Quit otherwise (`mighty server stop`, the OS's end signal).
    Stopped,
}

/// The last start by the OS.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LastWake {
    #[serde(with = "epoch_ms")]
    pub started: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "epoch_ms_option")]
    pub ended: Option<DateTime<Utc>>,
    #[serde(default)]
    pub fired: u32,
    #[serde(default)]
    pub skipped: u32,
    pub outcome: WakeOutcome,
}

/// An instant as epoch milliseconds in the file.
mod epoch_ms {
    use chrono::{DateTime, TimeZone, Utc};
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(at: &DateTime<Utc>, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_i64(at.timestamp_millis())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<DateTime<Utc>, D::Error> {
        let ms = i64::deserialize(deserializer)?;
        Utc.timestamp_millis_opt(ms).single().ok_or_else(|| serde::de::Error::custom("an instant out of range"))
    }
}

mod epoch_ms_option {
    use chrono::{DateTime, TimeZone, Utc};
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(at: &Option<DateTime<Utc>>, serializer: S) -> Result<S::Ok, S::Error> {
        match at {
            Some(at) => serializer.serialize_some(&at.timestamp_millis()),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<DateTime<Utc>>, D::Error> {
        Ok(Option::<i64>::deserialize(deserializer)?.and_then(|ms| Utc.timestamp_millis_opt(ms).single()))
    }
}

/// `<data>/automation/wake.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WakeRecord {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registration: Option<Registration>,
    /// Why the last registration or removal failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_wake: Option<LastWake>,
}

/// What the OS should start the app for: the earliest next fire, and the command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wanted {
    pub at: DateTime<Utc>,
    pub command: Vec<String>,
}

/// The wake wanted now: none while the setting is off, the app may not change the machine, or no enabled schedule has a next fire.
pub fn wanted(setting_on: bool, may_change_machine: bool, next_fire: Option<DateTime<Utc>>, command: Vec<String>) -> Option<Wanted> {
    match (setting_on && may_change_machine, next_fire) {
        (true, Some(at)) => Some(Wanted { at, command }),
        _ => None,
    }
}

/// What a sync does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Nothing,
    Register(Wanted),
    Remove,
}

/// The wall-clock minute of an instant (what the OS is given), for comparing registrations.
fn minute(at: DateTime<Utc>) -> i64 {
    at.timestamp().div_euclid(60)
}

/// What to do to the OS's registration: `current` what the record says is registered, `installed` whether the OS's item is there,
/// `last_try` the last registration attempted in this process, `forced` the person's 지금 다시 등록 or the app's quit.
///
/// - Nothing wanted: remove whatever is there.
/// - Missing (no record, or the item gone), another command (the app moved), or registered for later than the next fire (a new or
///   earlier schedule): register now.
/// - Registered for an earlier time still to come (a schedule removed or moved later): register now.
/// - Registered for a time that has passed (the schedule just fired, the app runs on): register again, but not more often than
///   [`REREGISTER_EVERY`] unless forced (the quit always leaves the right one).
pub fn decide(
    wanted: Option<Wanted>,
    current: Option<&Registration>,
    installed: bool,
    now: DateTime<Utc>,
    last_try: Option<DateTime<Utc>>,
    forced: bool,
) -> Step {
    let Some(wanted) = wanted else {
        return if current.is_some() || installed { Step::Remove } else { Step::Nothing };
    };
    let Some(current) = current.filter(|_| installed) else { return Step::Register(wanted) };
    if current.command != wanted.command {
        return Step::Register(wanted);
    }
    if minute(current.at) == minute(wanted.at) {
        return if forced { Step::Register(wanted) } else { Step::Nothing };
    }
    let passed = current.at <= now;
    let throttled = last_try.is_some_and(|at| now - at < REREGISTER_EVERY);
    if passed && throttled && !forced { Step::Nothing } else { Step::Register(wanted) }
}

/// When a server started for schedules quits: once nothing has kept it busy for [`IDLE_EXIT`] (the clock given, so tests drive it).
#[derive(Debug, Clone, Default)]
pub struct IdleClock {
    quiet_since: Option<Instant>,
}

impl IdleClock {
    /// One look at `now`: `busy` when something runs or waits (a run, a card, a terminal at work, a fire under way, the window shown,
    /// a phone's request). True when it is time to quit.
    pub fn tick(&mut self, now: Instant, busy: bool) -> bool {
        if busy {
            self.quiet_since = None;
            return false;
        }
        let since = *self.quiet_since.get_or_insert(now);
        now.saturating_duration_since(since) >= IDLE_EXIT
    }

    /// How long it has been quiet.
    pub fn quiet_for(&self, now: Instant) -> Duration {
        self.quiet_since.map_or(Duration::ZERO, |since| now.saturating_duration_since(since))
    }
}

/// Whether a server started for schedules quits when idle: not while it is also the person's login server (로그인할 때 창 없이 시작
/// keeps a server running) and not in the window's place (an app started with its window never quits by itself).
pub fn quits_when_idle(for_schedules: bool, start_at_login: bool) -> bool {
    for_schedules && !start_at_login
}

#[cfg(test)]
mod tests {
    use chrono::Duration as Span;

    use super::*;
    use crate::automation::plan::{Slot, next_wake};
    use crate::automation::recurrence::Recurrence;
    use crate::automation::recurrence::tests::{NewYork, utc};
    use crate::automation::spec::CatchUp;

    fn args() -> Vec<String> {
        ["/Applications/Mighty Code.app/Contents/MacOS/MightyCode", "--server", "--for-schedules"].map(String::from).to_vec()
    }

    fn wake_for(line: &str, now: DateTime<Utc>) -> WakeTime {
        let slots = vec![Slot { key: "k".into(), recurrence: Recurrence::cron(line).unwrap(), catch_up: CatchUp::Once, enabled: true }];
        let at = next_wake(&slots, &Default::default(), now, &NewYork).unwrap();
        WakeTime::of(at, &NewYork)
    }

    #[test]
    fn the_launch_agent_wakes_the_app_at_the_next_fire() {
        let at = wake_for("0 9 * * 1-5", utc(2026, 11, 23, 15, 0));
        assert_eq!(at, WakeTime { year: 2026, month: 11, day: 24, hour: 9, minute: 0 });
        assert_eq!(
            launch_agent_plist("dev.mightycode.native.schedule", &args(), at),
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
             <plist version=\"1.0\">\n\
             <dict>\n\
             \t<key>Label</key>\n\
             \t<string>dev.mightycode.native.schedule</string>\n\
             \t<key>ProgramArguments</key>\n\
             \t<array>\n\
             \t\t<string>/Applications/Mighty Code.app/Contents/MacOS/MightyCode</string>\n\
             \t\t<string>--server</string>\n\
             \t\t<string>--for-schedules</string>\n\
             \t</array>\n\
             \t<key>EnvironmentVariables</key>\n\
             \t<dict>\n\
             \t\t<key>MIGHTY_CODE_SCHEDULE_WAKE</key>\n\
             \t\t<string>1</string>\n\
             \t</dict>\n\
             \t<key>StartCalendarInterval</key>\n\
             \t<dict>\n\
             \t\t<key>Month</key>\n\
             \t\t<integer>11</integer>\n\
             \t\t<key>Day</key>\n\
             \t\t<integer>24</integer>\n\
             \t\t<key>Hour</key>\n\
             \t\t<integer>9</integer>\n\
             \t\t<key>Minute</key>\n\
             \t\t<integer>0</integer>\n\
             \t</dict>\n\
             \t<key>ProcessType</key>\n\
             \t<string>Interactive</string>\n\
             </dict>\n\
             </plist>\n"
        );
        let plist = launch_agent_plist("x", &args(), at);
        assert!(!plist.contains("KeepAlive") && !plist.contains("RunAtLoad"), "started only by the calendar");
        let escaped = launch_agent_plist("x", &["/Users/me/A & <B>/MightyCode".to_string()], at);
        assert!(escaped.contains("<string>/Users/me/A &amp; &lt;B&gt;/MightyCode</string>"));
    }

    #[test]
    fn the_wake_follows_dst_and_month_ends() {
        // 02:30 is skipped on 2026-03-08 in New York: the fire is 03:00 EDT, so is the wake.
        assert_eq!(wake_for("30 2 * * *", utc(2026, 3, 7, 12, 0)), WakeTime { year: 2026, month: 3, day: 8, hour: 3, minute: 0 });
        // 01:30 twice on 2026-11-01: the first pass.
        let first = wake_for("30 1 * * *", utc(2026, 10, 31, 12, 0));
        assert_eq!(first, WakeTime { year: 2026, month: 11, day: 1, hour: 1, minute: 30 });
        // A 09:00 schedule keeps 09:00 across the change (EDT on Oct 31, EST on Nov 2).
        assert_eq!(wake_for("0 9 * * *", utc(2026, 10, 30, 20, 0)).hour, 9);
        assert_eq!(wake_for("0 9 * * *", utc(2026, 11, 1, 20, 0)), WakeTime { year: 2026, month: 11, day: 2, hour: 9, minute: 0 });
        // The 31st: from April 1 the next is May 31 (April has none).
        assert_eq!(wake_for("0 12 31 * *", utc(2026, 4, 1, 12, 0)), WakeTime { year: 2026, month: 5, day: 31, hour: 12, minute: 0 });
        // February 29th: the next leap year.
        assert_eq!(wake_for("0 0 29 2 *", utc(2026, 3, 1, 12, 0)), WakeTime { year: 2028, month: 2, day: 29, hour: 0, minute: 0 });
        // The year's end rolls over.
        let new_year = wake_for("0 0 1 1 *", utc(2026, 12, 31, 12, 0));
        assert_eq!(new_year, WakeTime { year: 2027, month: 1, day: 1, hour: 0, minute: 0 });
        assert_eq!(new_year.local_iso(), "2027-01-01T00:00:00");
        let plist = launch_agent_plist("x", &args(), WakeTime { year: 2026, month: 3, day: 8, hour: 3, minute: 0 });
        assert!(plist.contains("<key>Month</key>\n\t\t<integer>3</integer>\n\t\t<key>Day</key>\n\t\t<integer>8</integer>"));
        assert!(plist.contains("<key>Hour</key>\n\t\t<integer>3</integer>\n\t\t<key>Minute</key>\n\t\t<integer>0</integer>"));
    }

    #[test]
    fn windows_arguments_are_quoted_as_the_c_runtime_splits_them() {
        assert_eq!(quote_windows("--server"), "--server");
        assert_eq!(quote_windows(r"C:\Program Files\Mighty Code\MightyCode.exe"), r#""C:\Program Files\Mighty Code\MightyCode.exe""#);
        assert_eq!(quote_windows(r"D:\my work\"), r#""D:\my work\\""#, "the closing quote is kept");
        assert_eq!(quote_windows(r"D:\work\"), r"D:\work\", "no space: as it is");
        assert_eq!(quote_windows(r#"a "b""#), r#""a \"b\"""#);
        assert_eq!(quote_windows(""), r#""""#);
    }

    #[test]
    fn the_scheduled_task_starts_the_app_at_the_local_time() {
        let at = WakeTime { year: 2026, month: 11, day: 24, hour: 9, minute: 5 };
        let program = r"C:\Users\me\AppData\Local\Programs\Mighty Code\MightyCode.exe";
        let arguments: Vec<String> = ["--server", "--for-schedules", "--profile", r"D:\work & play\"].map(String::from).to_vec();
        let task = task_xml(program, &arguments, at);
        assert!(task.contains("<StartBoundary>2026-11-24T09:05:00</StartBoundary>"), "local time, no offset");
        assert!(task.contains(r"<Command>C:\Users\me\AppData\Local\Programs\Mighty Code\MightyCode.exe</Command>"));
        assert!(task.contains(r#"<Arguments>--server --for-schedules --profile &quot;D:\work &amp; play\\&quot;</Arguments>"#), "{task}");
        for setting in [
            "<StartWhenAvailable>true</StartWhenAvailable>",
            "<ExecutionTimeLimit>PT0S</ExecutionTimeLimit>",
            "<LogonType>InteractiveToken</LogonType>",
            "<RunLevel>LeastPrivilege</RunLevel>",
            "<DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>",
            "<MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>",
        ] {
            assert!(task.contains(setting), "{setting}");
        }
        assert!(!task.contains("HighestAvailable") && !task.contains("<UserId>"), "the current user, never elevated");
        let bytes = utf16_file("<a/>");
        assert_eq!(&bytes[..4], &[0xFF, 0xFE, b'<', 0]);
        assert_eq!(bytes.len(), 2 + 4 * 2);
        assert_eq!(
            schtasks_create("Mighty Code schedule", r"C:\Users\me\AppData\Roaming\MightyCode\automation\wake-task.xml"),
            ["/Create", "/TN", "Mighty Code schedule", "/XML", r"C:\Users\me\AppData\Roaming\MightyCode\automation\wake-task.xml", "/F"]
        );
        assert_eq!(schtasks_delete("Mighty Code schedule 0a"), ["/Delete", "/TN", "Mighty Code schedule 0a", "/F"]);
        assert_eq!(schtasks_query("Mighty Code schedule"), ["/Query", "/TN", "Mighty Code schedule"]);
    }

    fn registration(at: DateTime<Utc>) -> Registration {
        Registration { at, name: "n".into(), command: args(), registered_at: utc(2026, 11, 23, 0, 0) }
    }

    #[test]
    fn the_registration_follows_the_setting_and_the_schedules() {
        let now = utc(2026, 11, 23, 15, 0);
        let nine = utc(2026, 11, 24, 14, 0);
        let wants = wanted(true, true, Some(nine), args());
        assert_eq!(wants, Some(Wanted { at: nine, command: args() }));
        // Off, not allowed to change the machine, or no enabled schedule: nothing wanted, and what is there goes.
        assert_eq!(wanted(false, true, Some(nine), args()), None);
        assert_eq!(wanted(true, false, Some(nine), args()), None);
        assert_eq!(wanted(true, true, None, args()), None);
        assert_eq!(decide(None, Some(&registration(nine)), true, now, None, false), Step::Remove);
        assert_eq!(decide(None, None, true, now, None, false), Step::Remove, "an item the record lost is removed too");
        assert_eq!(decide(None, None, false, now, None, false), Step::Nothing);
        // Nothing registered, or the item gone: registered.
        assert_eq!(decide(wants.clone(), None, false, now, None, false), Step::Register(wants.clone().unwrap()));
        assert_eq!(decide(wants.clone(), Some(&registration(nine)), false, now, None, false), Step::Register(wants.clone().unwrap()));
        // As wanted: left alone, unless the person asks again.
        assert_eq!(decide(wants.clone(), Some(&registration(nine)), true, now, Some(now), false), Step::Nothing);
        assert_eq!(decide(wants.clone(), Some(&registration(nine)), true, now, Some(now), true), Step::Register(wants.clone().unwrap()));
        // The seconds do not matter (the OS is given minutes).
        assert_eq!(decide(wants.clone(), Some(&registration(nine + Span::seconds(30))), true, now, None, false), Step::Nothing);
        // A new earlier schedule, a later one after a removal, or the app moved: registered at once.
        let eight = utc(2026, 11, 24, 13, 0);
        assert!(
            matches!(decide(wanted(true, true, Some(eight), args()), Some(&registration(nine)), true, now, Some(now), false), Step::Register(w) if w.at == eight)
        );
        assert!(matches!(decide(wants.clone(), Some(&registration(eight)), true, now, Some(now), false), Step::Register(_)));
        let moved = wanted(true, true, Some(nine), vec!["/Users/me/Applications/MightyCode".into()]);
        assert!(matches!(decide(moved, Some(&registration(nine)), true, now, Some(now), false), Step::Register(_)));
    }

    #[test]
    fn after_a_fire_the_next_one_is_registered_but_not_every_minute() {
        // The 09:00 fire happened; tomorrow's is wanted.
        let after = utc(2026, 11, 24, 14, 1);
        let tomorrow = wanted(true, true, Some(utc(2026, 11, 25, 14, 0)), args());
        let today = registration(utc(2026, 11, 24, 14, 0));
        assert!(matches!(decide(tomorrow.clone(), Some(&today), true, after, Some(utc(2026, 11, 23, 15, 0)), false), Step::Register(_)));
        // An every-minute schedule: the last try a minute ago holds the next one back, unless forced (the quit).
        let minute_on = wanted(true, true, Some(utc(2026, 11, 24, 14, 2)), args());
        assert_eq!(decide(minute_on.clone(), Some(&today), true, after, Some(after - Span::minutes(1)), false), Step::Nothing);
        assert!(matches!(decide(minute_on.clone(), Some(&today), true, after, Some(after - Span::minutes(1)), true), Step::Register(_)));
        assert!(matches!(decide(minute_on, Some(&today), true, after, Some(after - Span::minutes(11)), false), Step::Register(_)));
    }

    #[test]
    fn a_server_started_for_schedules_quits_after_five_quiet_minutes() {
        let start = Instant::now();
        let at = |seconds: u64| start + Duration::from_secs(seconds);
        let mut clock = IdleClock::default();
        assert!(!clock.tick(at(0), true), "the catch-up's run is under way");
        assert!(!clock.tick(at(60), false));
        assert_eq!(clock.quiet_for(at(120)), Duration::from_secs(60));
        assert!(!clock.tick(at(60 + 299), false));
        // A card waits, a phone asks, the window shows: the quiet starts over.
        assert!(!clock.tick(at(400), true));
        assert_eq!(clock.quiet_for(at(400)), Duration::ZERO);
        assert!(!clock.tick(at(401), false));
        assert!(!clock.tick(at(401 + 299), false));
        assert!(clock.tick(at(401 + 300), false), "five quiet minutes");
        assert!(quits_when_idle(true, false));
        assert!(!quits_when_idle(true, true), "the login server stays");
        assert!(!quits_when_idle(false, false), "an app started with its window, or plain --server, never quits by itself");
    }

    #[test]
    fn the_record_reads_back_and_damage_is_empty() {
        let record = WakeRecord {
            registration: Some(registration(utc(2026, 11, 24, 14, 0))),
            error: None,
            last_wake: Some(LastWake {
                started: utc(2026, 11, 24, 14, 0),
                ended: Some(utc(2026, 11, 24, 14, 9)),
                fired: 1,
                skipped: 0,
                outcome: WakeOutcome::Idle,
            }),
        };
        let text = serde_json::to_string(&record).unwrap();
        assert!(text.contains("\"lastWake\"") && text.contains("\"outcome\":\"idle\""), "{text}");
        assert_eq!(serde_json::from_str::<WakeRecord>(&text).unwrap(), record);
        assert_eq!(serde_json::from_str::<WakeRecord>("{}").unwrap(), WakeRecord::default());
    }
}
