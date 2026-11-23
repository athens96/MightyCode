//! MC-32's files under `<data>/automation/`: `settings.json` (the app's own schedules and triggers per workspace, and the webhook
//! listener's switch and port, MC-39's 앱이 꺼져 있어도 예약 실행), `state.json` (each schedule's last fire, the triggers' seen ids, ETags, standing orders and Linear
//! states) and `history.json` (the run records, `mighty_core::automation::records`). Each is written whole (a temporary file renamed
//! over it), only where the app writes its profile.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

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
}

impl State {
    /// Drops what belongs to triggers no longer listed (by key prefix `<trigger key>` or `<trigger key>#…`).
    pub fn keep_triggers(&mut self, keys: &[String]) {
        let listed = |key: &String| keys.iter().any(|k| key == k || key.starts_with(&format!("{k}#")));
        self.seen.retain(|k, _| listed(k));
        self.etags.retain(|k, _| listed(k));
        self.standing.retain(|k, _| listed(k));
        self.linear.retain(|k, _| listed(k));
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
                    on: TriggerSource::Webhook,
                    enabled: true,
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

        let mut state = State::default();
        state.seen.entry("w1:app:t:hook".into()).or_default().first("d1");
        state.seen.entry("w1:app:t:gone#ci".into()).or_default().first("d2");
        state.keep_triggers(&["w1:app:t:hook".to_string()]);
        assert_eq!(state.seen.len(), 1);
        save_state(&dir, &state).unwrap();
        let mut back = load_state(&dir);
        assert!(!back.seen.get_mut("w1:app:t:hook").unwrap().first("d1"));
        std::fs::write(state_path(&dir), b"[").unwrap();
        assert_eq!(load_state(&dir), State::default());
        let _ = std::fs::remove_dir_all(dir);
    }
}
