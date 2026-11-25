//! MC-32's files under `<data>/automation/`: `settings.json` (the app's own schedules and triggers per workspace, and the webhook
//! listener's switch and port, MC-39's 앱이 꺼져 있어도 예약 실행), `state.json` (each schedule's last fire, the triggers' seen ids, ETags, standing orders, Linear
//! states and MC-98's MCP items) and `history.json` (the run records, `mighty_core::automation::records`). Each is written whole (a temporary file renamed
//! over it), only where the app writes its profile.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use mighty_core::automation::mcp_watch::McpSnapshot;
use mighty_core::automation::plan::SlotState;
use mighty_core::automation::spec::{ScheduleSpec, TriggerSpec};
use mighty_core::automation::watch::{Etags, IssueStates, Seen, StandingState};
use mighty_core::automation::write_atomic;
use serde::{Deserialize, Serialize};

pub const FOLDER: &str = "automation";
/// The listener's port until the person picks one.
pub const DEFAULT_PORT: u16 = 47_823;

pub fn folder(data_dir: &Path) -> PathBuf {
    data_dir.join(FOLDER)
}

pub fn settings_path(data_dir: &Path) -> PathBuf {
    folder(data_dir).join("settings.json")
}

pub fn state_path(data_dir: &Path) -> PathBuf {
    folder(data_dir).join("state.json")
}

pub fn history_path(data_dir: &Path) -> PathBuf {
    folder(data_dir).join("history.json")
}

/// One workspace's own schedules and triggers.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceJobs {
    #[serde(default)]
    pub schedules: Vec<ScheduleSpec>,
    #[serde(default)]
    pub triggers: Vec<TriggerSpec>,
}

/// The webhook listener: off until turned on, on 127.0.0.1 only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Listener {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_port")]
    pub port: u16,
}

fn default_port() -> u16 {
    DEFAULT_PORT
}

impl Default for Listener {
    fn default() -> Listener {
        Listener { enabled: false, port: DEFAULT_PORT }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    /// Workspace id → its jobs.
    #[serde(default)]
    pub workspaces: BTreeMap<String, WorkspaceJobs>,
    #[serde(default)]
    pub listener: Listener,
    /// MC-39: 앱이 꺼져 있어도 예약 실행 (off until turned on): the OS starts the app without a window at the next fire.
    #[serde(default, rename = "wakeWhenClosed")]
    pub wake_when_closed: bool,
}

/// What the scheduler and the watches keep between runs, by job key.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    pub slots: HashMap<String, SlotState>,
    #[serde(default)]
    pub seen: HashMap<String, Seen>,
    #[serde(default)]
    pub etags: HashMap<String, Etags>,
    #[serde(default)]
    pub standing: HashMap<String, StandingState>,
    #[serde(default)]
    pub linear: HashMap<String, IssueStates>,
    /// MC-98: each MCP trigger's items as last seen.
    #[serde(default)]
    pub mcp: HashMap<String, McpSnapshot>,
    /// MC-64: what each bot spent today (by its name's key), for its own daily budget.
    #[serde(default, rename = "botSpend")]
    pub bot_spend: HashMap<String, BotSpend>,
}

/// MC-64: a bot's spend on one day (local), in US dollars.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BotSpend {
    pub day: String,
    pub usd: f64,
}

impl BotSpend {
    /// What was spent on `day` (nothing when the record is of another day).
    pub fn on(&self, day: &str) -> f64 {
        if self.day == day { self.usd } else { 0.0 }
    }

    /// `usd` more on `day` (the count starts again on a new day).
    pub fn add(&mut self, day: &str, usd: f64) {
        if self.day != day {
            *self = BotSpend { day: day.to_string(), usd: 0.0 };
        }
        if usd.is_finite() && usd > 0.0 {
            self.usd += usd;
        }
    }
}

impl State {
    /// Drops what belongs to triggers no longer listed (by key prefix `<trigger key>` or `<trigger key>#…`).
    pub fn keep_triggers(&mut self, keys: &[String]) {
        let listed = |key: &String| keys.iter().any(|k| key == k || key.starts_with(&format!("{k}#")));
        self.seen.retain(|k, _| listed(k));
        self.etags.retain(|k, _| listed(k));
        self.standing.retain(|k, _| listed(k));
        self.linear.retain(|k, _| listed(k));
        self.mcp.retain(|k, _| listed(k));
    }
}

fn load<T: Default + for<'de> Deserialize<'de>>(path: &Path) -> T {
    std::fs::read(path).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default()
}

fn save<T: Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    let bytes = serde_json::to_vec_pretty(value).map_err(std::io::Error::other)?;
    write_atomic(path, &bytes)
}

pub fn load_settings(data_dir: &Path) -> Settings {
    load(&settings_path(data_dir))
}

pub fn save_settings(data_dir: &Path, settings: &Settings) -> std::io::Result<()> {
    save(&settings_path(data_dir), settings)
}

pub fn load_state(data_dir: &Path) -> State {
    load(&state_path(data_dir))
}

pub fn save_state(data_dir: &Path, state: &State) -> std::io::Result<()> {
    save(&state_path(data_dir), state)
}

#[cfg(test)]
mod tests {
    use mighty_core::automation::spec::{Action, TriggerSource};

    use super::*;

    #[test]
    fn settings_and_state_round_trip_and_damage_reads_as_defaults() {
        let dir = std::env::temp_dir().join(format!("mc-automation-store-{}", std::process::id()));
        let mut settings = Settings::default();
        settings.workspaces.insert(
            "w1".into(),
            WorkspaceJobs {
                schedules: vec![ScheduleSpec {
                    name: "deps".into(),
                    cron: Some("0 9 * * *".into()),
                    enabled: true,
                    action: Action { prompt: "p".into(), ..Action::default() },
                    ..ScheduleSpec::default()
                }],
                triggers: vec![TriggerSpec {
                    name: "hook".into(),
                    on: TriggerSource::webhook(),
                    enabled: true,
                    conditions: vec![],
                    action: Action { prompt: "q".into(), ..Action::default() },
                }],
            },
        );
        settings.listener = Listener { enabled: true, port: 50_000 };
        assert!(!settings.wake_when_closed, "MC-39's wake is off until turned on");
        settings.wake_when_closed = true;
        save_settings(&dir, &settings).unwrap();
        assert!(std::fs::read_to_string(settings_path(&dir)).unwrap().contains("\"wakeWhenClosed\": true"));
        assert_eq!(load_settings(&dir), settings);
        assert_eq!(load_settings(&dir.join("missing")).listener, Listener::default());
        // MC-97: a GitHub trigger saved with a filter before `where` reads back with its conditions, and is saved with `where`.
        let old = r#"{"workspaces":{"w1":{"triggers":[{"name":"pr","on":{"type":"github","events":["pr_opened"],
            "filter":{"author":"!dependabot[bot]","base":"main"}},"prompt":"p"}]}}}"#;
        std::fs::write(settings_path(&dir), old).unwrap();
        let read = load_settings(&dir);
        let words: Vec<String> = read.workspaces["w1"].triggers[0].conditions.iter().map(|c| c.words()).collect();
        // MC-98: both `ifPresent`, as the old filter passed an event without an author or a base; saved and read back so.
        assert_eq!(words, vec!["author?!=dependabot[bot]", "base?=main"]);
        save_settings(&dir, &read).unwrap();
        let saved = std::fs::read_to_string(settings_path(&dir)).unwrap();
        assert!(saved.contains("\"ifPresent\": true"), "{saved}");
        assert_eq!(load_settings(&dir), read);
        assert!(saved.contains("\"where\"") && !saved.contains("\"filter\""), "{saved}");

        let mut state = State::default();
        state.seen.entry("w1:app:t:hook".into()).or_default().first("d1");
        state.seen.entry("w1:app:t:gone#ci".into()).or_default().first("d2");
        state.keep_triggers(&["w1:app:t:hook".to_string()]);
        assert_eq!(state.seen.len(), 1);
        save_state(&dir, &state).unwrap();
        let mut back = load_state(&dir);
        assert!(!back.seen.get_mut("w1:app:t:hook").unwrap().first("d1"));
        // MC-64: a bot's spend counts on its day only.
        let mut spend = BotSpend::default();
        spend.add("2026-11-25", 1.25);
        spend.add("2026-11-25", 0.5);
        assert_eq!((spend.on("2026-11-25"), spend.on("2026-11-26")), (1.75, 0.0));
        spend.add("2026-11-26", f64::NAN);
        assert_eq!(spend.on("2026-11-26"), 0.0, "a new day starts again, a bad cost adds nothing");
        std::fs::write(state_path(&dir), b"[").unwrap();
        assert_eq!(load_state(&dir), State::default());
        let _ = std::fs::remove_dir_all(dir);
    }
}
