//! MC-65: one bot as a spec — its name, agent, model, permission, where it works, its instructions, its daily budget and the
//! schedules and triggers sent to it — the same JSON whichever way it comes: the 봇 group's + (a template), another agent's
//! `create_bot` / `update_bot` tool, or `mighty bot new --spec`. The app shows it on an approval sheet before anything is made.

use serde::{Deserialize, Serialize};

use crate::automation::spec::{Action, CatchUp, FireMode, ScheduleSpec, TriggerSource, TriggerSpec};
use crate::model::{BOT_INSTRUCTIONS_LIMIT, PaneBot};

/// The most schedules, and triggers, one spec makes.
pub const MAX_JOBS: usize = 16;
/// The largest spec read, in bytes.
pub const MAX_SPEC_BYTES: usize = 64 * 1024;

/// A schedule of the bot: when (a cron line or an RRULE) and what it is asked.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BotSchedule {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cron: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rrule: Option<String>,
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catch_up: Option<CatchUp>,
}

/// A trigger of the bot: what it listens to (as a trigger's `on`) and what it is asked.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BotTrigger {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub on: TriggerSource,
    pub prompt: String,
}

/// One bot as a spec.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BotSpec {
    /// Its name (no `@`); a friendly one in the app's language when none is given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The agent (`claude`, `codex`, …); `claude` when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The permission mode its runs take (one of the agent's); the agent's default when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission: Option<String>,
    /// A workspace (its id or name) it works in as a workspace bot; an independent bot when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    /// An independent bot's folder; `<data>/bots/<name>` when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_per_day: Option<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub schedules: Vec<BotSchedule>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub triggers: Vec<BotTrigger>,
}

impl BotSpec {
    /// Reads a spec from JSON text (at most [`MAX_SPEC_BYTES`]).
    pub fn parse(text: &str) -> Result<BotSpec, String> {
        if text.len() > MAX_SPEC_BYTES {
            return Err(format!("the spec is larger than {} KB", MAX_SPEC_BYTES / 1024));
        }
        serde_json::from_str(text).map_err(|e| format!("the spec is not valid: {e}"))
    }

    pub fn agent(&self) -> &str {
        self.agent.as_deref().map(str::trim).filter(|a| !a.is_empty()).unwrap_or("claude")
    }

    /// The name as given, made a handle (`@` and spaces aside); None when none is given.
    pub fn handle(&self) -> Option<String> {
        self.name.as_deref().map(str::trim).filter(|n| !n.is_empty()).and_then(PaneBot::handle_of)
    }

    /// The schedules and triggers it makes for the bot `handle`: each sent to `@handle` (a heartbeat) with the bot's agent and model,
    /// named as given or `<kind>-<n>`.
    pub fn jobs(&self, handle: &str) -> (Vec<ScheduleSpec>, Vec<TriggerSpec>) {
        let action = |prompt: &str| Action {
            prompt: prompt.trim().to_string(),
            agent: Some(self.agent().to_string()),
            model: self.model.clone().filter(|m| !m.trim().is_empty()),
            worktree: false,
            mode: FireMode::Heartbeat,
            pane: Some(format!("@{handle}")),
        };
        let named = |name: &Option<String>, kind: &str, n: usize| {
            name.as_deref().map(str::trim).filter(|n| !n.is_empty()).map(str::to_string).unwrap_or_else(|| format!("{handle}-{kind}-{n}"))
        };
        let schedules = self
            .schedules
            .iter()
            .enumerate()
            .map(|(i, s)| ScheduleSpec {
                name: named(&s.name, "schedule", i + 1),
                cron: s.cron.clone().filter(|c| !c.trim().is_empty()),
                rrule: s.rrule.clone().filter(|r| !r.trim().is_empty()),
                catch_up: s.catch_up.unwrap_or_default(),
                enabled: true,
                action: action(&s.prompt),
            })
            .collect();
        let triggers = self
            .triggers
            .iter()
            .enumerate()
            .map(|(i, t)| TriggerSpec { name: named(&t.name, "trigger", i + 1), on: t.on.clone(), enabled: true, action: action(&t.prompt) })
            .collect();
        (schedules, triggers)
    }

    /// Why it cannot be made: an unknown agent or permission, a bad name, budget, folder or job. `providers` are the agents the app
    /// runs; `permissions` gives an agent's modes.
    pub fn validate(&self, providers: &[&str], permissions: impl Fn(&str) -> &'static [&'static str]) -> Result<(), String> {
        let agent = self.agent();
        if !providers.contains(&agent) {
            return Err(format!("unknown agent '{agent}': use {}", providers.join(", ")));
        }
        if let Some(name) = self.name.as_deref().map(str::trim).filter(|n| !n.is_empty())
            && PaneBot::handle_of(name).is_none()
        {
            return Err(format!("'{name}' is not a bot's name: 1 to 32 letters or digits (-, _ and . between them)"));
        }
        if let Some(mode) = self.permission.as_deref()
            && !permissions(agent).contains(&mode)
        {
            return Err(format!("'{mode}' is not a permission of {agent}: use {}", permissions(agent).join(", ")));
        }
        if let Some(model) = &self.model
            && (model.chars().count() > 120 || model.chars().any(char::is_control))
        {
            return Err("the model name is not valid".into());
        }
        if self.instructions.as_deref().is_some_and(|t| t.trim().len() > BOT_INSTRUCTIONS_LIMIT) {
            return Err("the instructions are longer than 16 KB".into());
        }
        if let Some(budget) = self.budget_per_day
            && !(budget.is_finite() && budget > 0.0)
        {
            return Err("budgetPerDay is a number of dollars above 0".into());
        }
        if self.folder.as_deref().is_some_and(|f| f.trim().is_empty() || f.chars().any(char::is_control)) {
            return Err("the folder is not a path".into());
        }
        if self.folder.is_some() && self.workspace.is_some() {
            return Err("a workspace bot works in its workspace: give a folder or a workspace, not both".into());
        }
        if self.schedules.len() > MAX_JOBS || self.triggers.len() > MAX_JOBS {
            return Err(format!("at most {MAX_JOBS} schedules and {MAX_JOBS} triggers"));
        }
        let (schedules, triggers) = self.jobs(&self.handle().unwrap_or_else(|| "bot".into()));
        for spec in &schedules {
            spec.validate(providers).map_err(|e| format!("schedule '{}': {e}", spec.name))?;
        }
        for spec in &triggers {
            spec.validate(providers).map_err(|e| format!("trigger '{}': {e}", spec.name))?;
        }
        let mut names: Vec<&str> = schedules.iter().map(|s| s.name.as_str()).chain(triggers.iter().map(|t| t.name.as_str())).collect();
        names.sort_unstable();
        if names.windows(2).any(|w| w[0] == w[1]) {
            return Err("two jobs share a name".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automation::spec::GitHubEvent;

    const PROVIDERS: [&str; 2] = ["claude", "codex"];

    fn modes(agent: &str) -> &'static [&'static str] {
        crate::normalize::permission_modes(agent)
    }

    #[test]
    fn a_spec_reads_from_json_and_makes_jobs_sent_to_its_bot() {
        let spec = BotSpec::parse(
            r#"{"name":"PR집사","agent":"codex","permission":"acceptEdits","instructions":"Review PRs.","budgetPerDay":3,
                "schedules":[{"cron":"0 9 * * 1-5","prompt":"Summarize open PRs."}],
                "triggers":[{"name":"new-pr","on":{"type":"github","events":["pr_opened"],"repo":"owner/repo"},"prompt":"Review it."}]}"#,
        )
        .unwrap();
        assert!(spec.validate(&PROVIDERS, modes).is_ok());
        assert_eq!(spec.handle().as_deref(), Some("PR집사"));
        let (schedules, triggers) = spec.jobs("PR집사");
        assert_eq!(schedules[0].name, "PR집사-schedule-1");
        assert_eq!(schedules[0].action.bot().as_deref(), Some("PR집사"));
        assert_eq!(schedules[0].action.agent(), "codex");
        assert_eq!(triggers[0].name, "new-pr");
        assert_eq!(triggers[0].on, TriggerSource::Github { events: vec![GitHubEvent::PrOpened], repo: Some("owner/repo".into()), filter: Default::default() });
        let back = BotSpec::parse(&serde_json::to_string(&spec).unwrap()).unwrap();
        assert_eq!(back, spec, "it writes the shape it reads");
    }

    #[test]
    fn a_spec_says_what_is_wrong() {
        let wrong = |json: &str| BotSpec::parse(json).and_then(|s| s.validate(&PROVIDERS, modes)).unwrap_err();
        assert!(wrong(r#"{"agent":"nope"}"#).contains("unknown agent"));
        assert!(wrong(r#"{"name":"two words"}"#).contains("not a bot's name"));
        assert!(wrong(r#"{"agent":"codex","permission":"plan"}"#).contains("not a permission"));
        assert!(wrong(r#"{"budgetPerDay":0}"#).contains("budgetPerDay"));
        assert!(wrong(r#"{"folder":"/tmp/x","workspace":"w1"}"#).contains("not both"));
        assert!(wrong(r#"{"schedules":[{"cron":"every day","prompt":"p"}]}"#).contains("schedule"));
        assert!(wrong(r#"{"schedules":[{"name":"a","cron":"0 9 * * *","prompt":"p"}],"triggers":[{"name":"a","on":{"type":"webhook"},"prompt":"q"}]}"#).contains("share a name"));
        assert!(wrong(r#"{"sneaky":true}"#).contains("not valid"), "unknown fields are refused");
        assert!(BotSpec::parse(&"x".repeat(MAX_SPEC_BYTES + 1)).unwrap_err().contains("larger"));
        assert!(BotSpec::parse("{}").unwrap().validate(&PROVIDERS, modes).is_ok(), "an empty spec is a plain Claude bot");
    }
}
