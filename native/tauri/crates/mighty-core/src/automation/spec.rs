//! What a schedule or a trigger is (MC-32): its name, when or on what it fires, and what a fire does — a prompt sent by an agent
//! (and model) in a fresh pane, in its own worktree or not, or into an existing pane named for it (a heartbeat). The same shapes
//! come from `.mighty.yaml` (`schedules:`, `triggers:`) and from the app's settings, which keep them as JSON.

use serde::{Deserialize, Serialize};

use super::recurrence::Recurrence;

/// The most schedules, and triggers, a workspace keeps (each source).
pub const MAX_SCHEDULES: usize = 32;
pub const MAX_TRIGGERS: usize = 32;
/// The longest prompt, name and shell check.
pub const MAX_PROMPT_CHARS: usize = 8_000;
pub const MAX_NAME_CHARS: usize = 80;
pub const MAX_COMMAND_CHARS: usize = 2_000;
/// A standing order's check runs at most this often, and at least once a day.
pub const MIN_EVERY_MINUTES: u32 = 5;
pub const MAX_EVERY_MINUTES: u32 = 1_440;
pub const DEFAULT_EVERY_MINUTES: u32 = 15;
/// How many times a day a standing order may wake its agent.
pub const DEFAULT_DAILY_CAP: u32 = 5;
pub const MAX_DAILY_CAP: u32 = 50;

fn yes() -> bool {
    true
}

fn is_false(value: &bool) -> bool {
    !*value
}

fn is_true(value: &bool) -> bool {
    *value
}

/// A fresh pane, or the next request of a named one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FireMode {
    #[default]
    New,
    Heartbeat,
}

impl FireMode {
    pub fn name(self) -> &'static str {
        match self {
            FireMode::New => "new",
            FireMode::Heartbeat => "heartbeat",
        }
    }

    pub fn parse(text: &str) -> Option<FireMode> {
        match text.trim() {
            "new" => Some(FireMode::New),
            "heartbeat" => Some(FireMode::Heartbeat),
            _ => None,
        }
    }
}

/// What happens to the fires a closed app missed: none run, or the most recent one runs once.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CatchUp {
    Skip,
    #[default]
    Once,
}

impl CatchUp {
    pub fn name(self) -> &'static str {
        match self {
            CatchUp::Skip => "skip",
            CatchUp::Once => "once",
        }
    }

    pub fn parse(text: &str) -> Option<CatchUp> {
        match text.trim() {
            "skip" => Some(CatchUp::Skip),
            "once" => Some(CatchUp::Once),
            _ => None,
        }
    }
}

/// What a fire does.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Action {
    pub prompt: String,
    /// The provider (`claude`, `codex`, …); `claude` when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub worktree: bool,
    #[serde(default)]
    pub mode: FireMode,
    /// The pane a heartbeat wakes, by its title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane: Option<String>,
}

impl Action {
    pub fn agent(&self) -> &str {
        self.agent.as_deref().filter(|a| !a.is_empty()).unwrap_or("claude")
    }

    pub fn validate(&self, providers: &[&str]) -> Result<(), String> {
        if self.prompt.trim().is_empty() {
            return Err("the prompt is empty".into());
        }
        if self.prompt.chars().count() > MAX_PROMPT_CHARS {
            return Err(format!("the prompt is longer than {MAX_PROMPT_CHARS} characters"));
        }
        if !providers.is_empty() && !providers.contains(&self.agent()) {
            return Err(format!("unknown agent '{}': use {}", self.agent(), providers.join(", ")));
        }
        if let Some(model) = &self.model
            && (model.chars().count() > 120 || model.chars().any(char::is_control))
        {
            return Err("the model name is not valid".into());
        }
        if self.mode == FireMode::Heartbeat && self.pane.as_deref().is_none_or(|p| p.trim().is_empty()) {
            return Err("a heartbeat names the pane it wakes (pane)".into());
        }
        Ok(())
    }
}

/// A schedule: a cron line or an RRULE, and what it does.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleSpec {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cron: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rrule: Option<String>,
    #[serde(default)]
    pub catch_up: CatchUp,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub enabled: bool,
    #[serde(flatten)]
    pub action: Action,
}

impl ScheduleSpec {
    /// The schedule's recurrence, or why it cannot run.
    pub fn validate(&self, providers: &[&str]) -> Result<Recurrence, String> {
        valid_name(&self.name)?;
        let recurrence = Recurrence::parse(self.cron.as_deref(), self.rrule.as_deref())?;
        self.action.validate(providers)?;
        Ok(recurrence)
    }

    /// The cron line or RRULE as written.
    pub fn when(&self) -> &str {
        self.cron.as_deref().filter(|c| !c.trim().is_empty()).or(self.rrule.as_deref()).unwrap_or("")
    }
}

/// A GitHub event a trigger follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GitHubEvent {
    PrOpened,
    ReviewRequested,
    CiFailed,
}

impl GitHubEvent {
    pub const ALL: [GitHubEvent; 3] = [GitHubEvent::PrOpened, GitHubEvent::ReviewRequested, GitHubEvent::CiFailed];

    pub fn name(self) -> &'static str {
        match self {
            GitHubEvent::PrOpened => "pr_opened",
            GitHubEvent::ReviewRequested => "review_requested",
            GitHubEvent::CiFailed => "ci_failed",
        }
    }

    pub fn parse(text: &str) -> Option<GitHubEvent> {
        GitHubEvent::ALL.into_iter().find(|e| e.name() == text.trim())
    }
}

/// What a trigger listens to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum TriggerSource {
    /// The workspace's GitHub repository, watched through `gh api`.
    #[serde(rename_all = "camelCase")]
    Github { events: Vec<GitHubEvent> },
    /// Linear issues (the workspace's MC-31 tracker) entering one of these states.
    #[serde(rename_all = "camelCase")]
    Linear { states: Vec<String> },
    /// A signed POST to the local listener.
    Webhook,
    /// A shell check run every few minutes; its agent wakes when the output changes, at most `daily_cap` times a day.
    #[serde(rename_all = "camelCase")]
    Standing { command: String, every_minutes: u32, daily_cap: u32 },
}

impl TriggerSource {
    pub fn kind(&self) -> &'static str {
        match self {
            TriggerSource::Github { .. } => "github",
            TriggerSource::Linear { .. } => "linear",
            TriggerSource::Webhook => "webhook",
            TriggerSource::Standing { .. } => "standing",
        }
    }

    /// Whether the app has to look for it now and then (GitHub, Linear, a standing order); a webhook only listens.
    pub fn polled(&self) -> bool {
        !matches!(self, TriggerSource::Webhook)
    }
}

/// A trigger: what it listens to, and what it does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TriggerSpec {
    pub name: String,
    pub on: TriggerSource,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub enabled: bool,
    #[serde(flatten)]
    pub action: Action,
}

impl TriggerSpec {
    pub fn validate(&self, providers: &[&str]) -> Result<(), String> {
        valid_name(&self.name)?;
        match &self.on {
            TriggerSource::Github { events } if events.is_empty() => return Err("name at least one GitHub event".into()),
            TriggerSource::Linear { states } if states.iter().all(|s| s.trim().is_empty()) => {
                return Err("name at least one Linear state".into());
            }
            TriggerSource::Standing { command, every_minutes, daily_cap } => {
                if command.trim().is_empty() || command.chars().count() > MAX_COMMAND_CHARS {
                    return Err("the check command is empty or too long".into());
                }
                if !(MIN_EVERY_MINUTES..=MAX_EVERY_MINUTES).contains(every_minutes) {
                    return Err(format!("everyMinutes runs from {MIN_EVERY_MINUTES} to {MAX_EVERY_MINUTES}"));
                }
                if !(1..=MAX_DAILY_CAP).contains(daily_cap) {
                    return Err(format!("dailyCap runs from 1 to {MAX_DAILY_CAP}"));
                }
            }
            _ => {}
        }
        self.action.validate(providers)
    }
}

/// A schedule's or trigger's name: 1 to 80 characters, no control or invisible formatting character, no slash.
pub fn valid_name(name: &str) -> Result<(), String> {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed != name || name.chars().count() > MAX_NAME_CHARS {
        return Err(format!("'{name}' is not a valid name"));
    }
    if name.chars().any(|c| {
        c.is_control()
            || c == '/'
            || c == '\\'
            || matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}')
    }) {
        return Err(format!("'{name}' is not a valid name"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROVIDERS: [&str; 2] = ["claude", "codex"];

    fn schedule() -> ScheduleSpec {
        ScheduleSpec {
            name: "deps".into(),
            cron: Some("0 9 * * *".into()),
            enabled: true,
            action: Action { prompt: "check deps".into(), worktree: true, ..Action::default() },
            ..ScheduleSpec::default()
        }
    }

    #[test]
    fn schedules_check_their_fields() {
        assert!(schedule().validate(&PROVIDERS).is_ok());
        let mut bad = schedule();
        bad.action.prompt = "  ".into();
        assert!(bad.validate(&PROVIDERS).is_err());
        let mut bad = schedule();
        bad.action.agent = Some("nope".into());
        assert!(bad.validate(&PROVIDERS).unwrap_err().contains("unknown agent"));
        let mut bad = schedule();
        bad.action.mode = FireMode::Heartbeat;
        assert!(bad.validate(&PROVIDERS).unwrap_err().contains("heartbeat"));
        bad.action.pane = Some("야간 점검".into());
        assert!(bad.validate(&PROVIDERS).is_ok());
        let mut bad = schedule();
        bad.name = "a/b".into();
        assert!(bad.validate(&PROVIDERS).is_err());
        let mut bad = schedule();
        bad.rrule = Some("FREQ=DAILY".into());
        assert!(bad.validate(&PROVIDERS).is_err(), "cron and rrule both");
    }

    #[test]
    fn specs_keep_their_json_shape() {
        let value = serde_json::to_value(schedule()).unwrap();
        assert_eq!(
            value,
            serde_json::json!({"name": "deps", "cron": "0 9 * * *", "catchUp": "once", "prompt": "check deps", "worktree": true, "mode": "new"})
        );
        let back: ScheduleSpec = serde_json::from_value(value).unwrap();
        assert_eq!(back, schedule());
        let trigger = TriggerSpec {
            name: "ci".into(),
            on: TriggerSource::Standing { command: "make check".into(), every_minutes: 15, daily_cap: 3 },
            enabled: false,
            action: Action { prompt: "fix it".into(), ..Action::default() },
        };
        let value = serde_json::to_value(&trigger).unwrap();
        assert_eq!(value["on"], serde_json::json!({"type": "standing", "command": "make check", "everyMinutes": 15, "dailyCap": 3}));
        assert_eq!(value["enabled"], false);
        assert_eq!(serde_json::from_value::<TriggerSpec>(value).unwrap(), trigger);
        let github: TriggerSpec =
            serde_json::from_str(r#"{"name":"g","on":{"type":"github","events":["ci_failed"]},"prompt":"p"}"#).unwrap();
        assert_eq!(github.on, TriggerSource::Github { events: vec![GitHubEvent::CiFailed] });
        assert!(github.enabled && github.validate(&PROVIDERS).is_ok());
    }

    #[test]
    fn triggers_check_their_sources() {
        let mut trigger = TriggerSpec {
            name: "s".into(),
            on: TriggerSource::Standing { command: "true".into(), every_minutes: 1, daily_cap: 3 },
            enabled: true,
            action: Action { prompt: "p".into(), ..Action::default() },
        };
        assert!(trigger.validate(&PROVIDERS).unwrap_err().contains("everyMinutes"));
        trigger.on = TriggerSource::Standing { command: "true".into(), every_minutes: 5, daily_cap: 0 };
        assert!(trigger.validate(&PROVIDERS).unwrap_err().contains("dailyCap"));
        trigger.on = TriggerSource::Github { events: vec![] };
        assert!(trigger.validate(&PROVIDERS).is_err());
        trigger.on = TriggerSource::Linear { states: vec!["Ready".into()] };
        assert!(trigger.validate(&PROVIDERS).is_ok());
        assert!(TriggerSource::Webhook.kind() == "webhook" && !TriggerSource::Webhook.polled());
    }
}
