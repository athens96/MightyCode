//! MC-71: the new-bot wizard. The page draws it (shell/BotWizard.tsx): a gallery of templates first, then where and who, when and how
//! much, and a last look; ⚡ makes the template's bot at once. The host gives the page what it picks from (`botWizard`) and makes the
//! bot it asked for (`botWizardCreate`) from the same spec the CLI and the agents' `create_bot` use.
//!
//! MC-99: three recipes set up a whole rule (its source, `where`, `do`, `with` and `policy`): CI 고치미 (개발) develops a fix of a
//! failed CI on main and opens a pull request, once per run; MightyWork 릴리스 봇 runs `/release {{task.version}}` when a task
//! titled `vX.Y.Z 릴리스` moves to todo, after the person's approval, once per task; MightyWork 개발 봇 develops a task labelled
//! `bot:dev` when it moves to todo and opens a pull request. The 언제 step shows each trigger's rule and lets the person change
//! its conditions, action and policy (`rules`); a MightyWork recipe asks for the project's key (`project`).

use std::sync::Arc;

use mighty_core::automation::condition::{Condition, Op};
use mighty_core::automation::mcp_watch;
use mighty_core::automation::policy::{OncePer, Policy};
use mighty_core::automation::spec::{ActionKind, BotOutcome, CatchUp, GitHubEvent, TriggerSource, With};
use mighty_core::bot_spec::{BotSchedule, BotSpec, BotTrigger};
use mighty_core::locale::Locale;
use serde_json::{Value, json};

use crate::host::{Host, Registry};
use crate::methods::text as arg;

/// What a template needs from the person beyond a name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Needs {
    Nothing,
    /// A GitHub repository (`owner/name`): a workspace bot may leave it to its workspace's.
    Repo,
    /// A command (its hint's locale key): the check a standing order runs, or the benchmark a schedule runs.
    Command(&'static str),
    /// MC-99: a MightyWork project's key (`MC`), the project its MCP source watches.
    Project,
}

/// MC-99: a recipe's whole rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recipe {
    /// CI failed on main: develop the fix and open a pull request, once per run.
    CiDevelop,
    /// A MightyWork task `vX.Y.Z 릴리스` moved to todo: run `/release {{task.version}}` once approved, once per task.
    Release,
    /// A MightyWork task labelled `bot:dev` moved to todo: develop it and open a pull request, once per task.
    TaskDevelop,
}

/// MC-99: the release recipe's title condition.
pub const RELEASE_TITLE: &str = r"^v?\d+\.\d+\.\d+ 릴리[스즈]";
/// MC-99: the label the develop recipe takes.
pub const DEV_LABEL: &str = "bot:dev";

/// MC-99: a MightyWork project's key as the wizard takes it: 1 to 32 of letters, digits, `-` and `_`.
pub fn valid_project(key: &str) -> bool {
    let key = key.trim();
    !key.is_empty() && key.len() <= 32 && key.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
}

/// MC-99: the MightyWork preset's MCP source for the project `key`.
fn mightywork_source(project: &str) -> TriggerSource {
    let preset = mcp_watch::preset("mightywork").expect("the mightywork preset");
    let mut args = preset.args.clone();
    args["project"] = Value::String(project.trim().to_string());
    TriggerSource::Mcp {
        server: preset.server.to_string(),
        tool: preset.tool.to_string(),
        args,
        every_minutes: mcp_watch::DEFAULT_MCP_MINUTES,
        item: Box::new(preset.item.clone()),
        preset: Some(preset.name.to_string()),
        allow_write: false,
    }
}

impl Recipe {
    /// Its trigger: the source (`repo` a GitHub one's, `project` a MightyWork one's), its conditions, action and policy, asking
    /// `prompt`.
    pub fn trigger(self, repo: Option<String>, project: &str, prompt: String) -> BotTrigger {
        let once = Policy { once_per: Some(OncePer::Subject), ..Policy::default() };
        let stage_to_todo =
            vec![Condition::new("type", Op::Eq, "mightywork.task.stage_changed"), Condition::new("task.to", Op::Eq, "todo")];
        match self {
            Recipe::CiDevelop => BotTrigger {
                conditions: vec![Condition::new("base", Op::Eq, "main").if_present()],
                kind: ActionKind::Develop,
                with: With { pr: Some(true), ..With::default() },
                policy: once,
                ..BotTrigger::ask(TriggerSource::Github { events: vec![GitHubEvent::CiFailed], repo }, prompt)
            },
            Recipe::Release => BotTrigger {
                conditions: [stage_to_todo, vec![Condition::new("task.title", Op::Matches, RELEASE_TITLE)]].concat(),
                kind: ActionKind::Run,
                with: With { command: Some("/release {{task.version}}".into()), ..With::default() },
                policy: Policy { approve: Some(true), ..once },
                ..BotTrigger::ask(mightywork_source(project), prompt)
            },
            Recipe::TaskDevelop => BotTrigger {
                conditions: [stage_to_todo, vec![Condition::new("task.tags", Op::Has, DEV_LABEL)]].concat(),
                kind: ActionKind::Develop,
                with: With { pr: Some(true), ..With::default() },
                policy: once,
                ..BotTrigger::ask(mightywork_source(project), prompt)
            },
        }
    }
}

/// MC-99: a trigger's rule as the 언제 step edits it: its conditions, what it does and its policy.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Rule {
    #[serde(default, rename = "where")]
    pub conditions: Vec<Condition>,
    #[serde(default, rename = "do")]
    pub kind: ActionKind,
    #[serde(default)]
    pub with: With,
    #[serde(default)]
    pub policy: Policy,
}

impl Rule {
    pub fn of(trigger: &BotTrigger) -> Rule {
        Rule { conditions: trigger.conditions.clone(), kind: trigger.kind, with: trigger.with.clone(), policy: trigger.policy.clone() }
    }

    /// `trigger` with this rule.
    fn apply(&self, trigger: &mut BotTrigger) {
        trigger.conditions = self.conditions.clone();
        trigger.kind = self.kind;
        trigger.with = self.with.clone();
        trigger.policy = self.policy.clone();
    }
}

/// One schedule or trigger a template starts with, and the words saying when.
#[derive(Debug, Clone, Copy)]
pub enum JobDef {
    Github(GitHubEvent, &'static str),
    Cron(&'static str, &'static str),
    /// A cron that skips the runs it missed.
    CronSkip(&'static str, &'static str),
    Event(&'static str, &'static str),
    /// A standing order on the command the person gives.
    Standing(&'static str),
    /// MC-99: a recipe's whole rule.
    Recipe(Recipe, &'static str),
}

impl JobDef {
    /// A schedule's cron line (the wizard lets the person change its time); None for a trigger.
    fn cron(self) -> Option<&'static str> {
        match self {
            JobDef::Cron(cron, _) | JobDef::CronSkip(cron, _) => Some(cron),
            _ => None,
        }
    }

    fn when(self) -> &'static str {
        match self {
            JobDef::Github(_, when)
            | JobDef::Cron(_, when)
            | JobDef::CronSkip(_, when)
            | JobDef::Event(_, when)
            | JobDef::Standing(when)
            | JobDef::Recipe(_, when) => when,
        }
    }
}

/// A template of the wizard's gallery.
#[derive(Debug)]
pub struct TemplateDef {
    pub id: &'static str,
    pub icon: &'static str,
    pub category: &'static str,
    pub label: &'static str,
    pub what: &'static str,
    pub instructions: &'static str,
    pub prompt: &'static str,
    pub needs: Needs,
    pub jobs: &'static [JobDef],
}

/// The gallery's categories, in order.
pub const CATEGORIES: [&str; 6] = [
    "bot.wizard.category.review",
    "bot.wizard.category.release",
    "bot.wizard.category.repo",
    "bot.wizard.category.daily",
    "bot.wizard.category.watch",
    "bot.wizard.category.other",
];

/// The templates, in the gallery's order (빈 봇 last).
pub const TEMPLATES: [TemplateDef; 23] = [
    TemplateDef {
        id: "prReviewer",
        icon: "🔍",
        category: "bot.wizard.category.review",
        label: "bot.template.prReviewer",
        what: "bot.template.prReviewer.what",
        instructions: "bot.template.prReviewer.instructions",
        prompt: "bot.template.prReviewer.prompt",
        needs: Needs::Repo,
        jobs: &[
            JobDef::Github(GitHubEvent::PrOpened, "bot.wizard.when.prOpened"),
            JobDef::Github(GitHubEvent::ReviewRequested, "bot.wizard.when.reviewRequested"),
        ],
    },
    TemplateDef {
        id: "security",
        icon: "🔐",
        category: "bot.wizard.category.review",
        label: "bot.template.security",
        what: "bot.template.security.what",
        instructions: "bot.template.security.instructions",
        prompt: "bot.template.security.prompt",
        needs: Needs::Repo,
        jobs: &[JobDef::Github(GitHubEvent::PrMerged, "bot.wizard.when.prMerged")],
    },
    TemplateDef {
        id: "tests",
        icon: "🧪",
        category: "bot.wizard.category.review",
        label: "bot.template.tests",
        what: "bot.template.tests.what",
        instructions: "bot.template.tests.instructions",
        prompt: "bot.template.tests.prompt",
        needs: Needs::Repo,
        jobs: &[JobDef::Github(GitHubEvent::PrOpened, "bot.wizard.when.prOpened")],
    },
    TemplateDef {
        id: "ciFixer",
        icon: "🛠",
        category: "bot.wizard.category.release",
        label: "bot.template.ciFixer",
        what: "bot.template.ciFixer.what",
        instructions: "bot.template.ciFixer.instructions",
        prompt: "bot.template.ciFixer.prompt",
        needs: Needs::Repo,
        jobs: &[JobDef::Github(GitHubEvent::CiFailed, "bot.wizard.when.ciFailed")],
    },
    // MC-99: the recipes.
    TemplateDef {
        id: "ciDevelop",
        icon: "🧰",
        category: "bot.wizard.category.release",
        label: "bot.template.ciDevelop",
        what: "bot.template.ciDevelop.what",
        instructions: "bot.template.ciDevelop.instructions",
        prompt: "bot.template.ciDevelop.prompt",
        needs: Needs::Repo,
        jobs: &[JobDef::Recipe(Recipe::CiDevelop, "bot.wizard.when.ciFailedMain")],
    },
    TemplateDef {
        id: "mwRelease",
        icon: "🚀",
        category: "bot.wizard.category.release",
        label: "bot.template.mwRelease",
        what: "bot.template.mwRelease.what",
        instructions: "bot.template.mwRelease.instructions",
        prompt: "bot.template.mwRelease.prompt",
        needs: Needs::Project,
        jobs: &[JobDef::Recipe(Recipe::Release, "bot.wizard.when.releaseTodo")],
    },
    TemplateDef {
        id: "mwDevelop",
        icon: "🏗",
        category: "bot.wizard.category.repo",
        label: "bot.template.mwDevelop",
        what: "bot.template.mwDevelop.what",
        instructions: "bot.template.mwDevelop.instructions",
        prompt: "bot.template.mwDevelop.prompt",
        needs: Needs::Project,
        jobs: &[JobDef::Recipe(Recipe::TaskDevelop, "bot.wizard.when.devTodo")],
    },
    TemplateDef {
        id: "releaseNotes",
        icon: "📝",
        category: "bot.wizard.category.release",
        label: "bot.template.releaseNotes",
        what: "bot.template.releaseNotes.what",
        instructions: "bot.template.releaseNotes.instructions",
        prompt: "bot.template.releaseNotes.prompt",
        needs: Needs::Repo,
        jobs: &[JobDef::Github(GitHubEvent::ReleasePublished, "bot.wizard.when.releasePublished")],
    },
    TemplateDef {
        id: "releaseReady",
        icon: "🚦",
        category: "bot.wizard.category.release",
        label: "bot.template.releaseReady",
        what: "bot.template.releaseReady.what",
        instructions: "bot.template.releaseReady.instructions",
        prompt: "bot.template.releaseReady.prompt",
        needs: Needs::Nothing,
        jobs: &[JobDef::Cron("0 17 * * 1-5", "bot.wizard.when.weekdays17")],
    },
    TemplateDef {
        id: "afterRelease",
        icon: "✅",
        category: "bot.wizard.category.release",
        label: "bot.template.afterRelease",
        what: "bot.template.afterRelease.what",
        instructions: "bot.template.afterRelease.instructions",
        prompt: "bot.template.afterRelease.prompt",
        needs: Needs::Nothing,
        jobs: &[JobDef::Event("release.done", "bot.wizard.when.releaseDone")],
    },
    TemplateDef {
        id: "deps",
        icon: "📦",
        category: "bot.wizard.category.repo",
        label: "bot.template.deps",
        what: "bot.template.deps.what",
        instructions: "bot.template.deps.instructions",
        prompt: "bot.template.deps.prompt",
        needs: Needs::Nothing,
        jobs: &[JobDef::Cron("0 10 * * 1", "bot.wizard.when.monday10")],
    },
    TemplateDef {
        id: "issueTriage",
        icon: "🏷",
        category: "bot.wizard.category.repo",
        label: "bot.template.issueTriage",
        what: "bot.template.issueTriage.what",
        instructions: "bot.template.issueTriage.instructions",
        prompt: "bot.template.issueTriage.prompt",
        needs: Needs::Repo,
        jobs: &[JobDef::Github(GitHubEvent::IssueOpened, "bot.wizard.when.issueOpened")],
    },
    TemplateDef {
        id: "docs",
        icon: "📚",
        category: "bot.wizard.category.repo",
        label: "bot.template.docs",
        what: "bot.template.docs.what",
        instructions: "bot.template.docs.instructions",
        prompt: "bot.template.docs.prompt",
        needs: Needs::Repo,
        jobs: &[JobDef::Github(GitHubEvent::PrMerged, "bot.wizard.when.prMerged")],
    },
    TemplateDef {
        id: "cleanup",
        icon: "🧹",
        category: "bot.wizard.category.repo",
        label: "bot.template.cleanup",
        what: "bot.template.cleanup.what",
        instructions: "bot.template.cleanup.instructions",
        prompt: "bot.template.cleanup.prompt",
        needs: Needs::Nothing,
        jobs: &[JobDef::Cron("0 16 * * 5", "bot.wizard.when.friday16")],
    },
    TemplateDef {
        id: "todos",
        icon: "📌",
        category: "bot.wizard.category.repo",
        label: "bot.template.todos",
        what: "bot.template.todos.what",
        instructions: "bot.template.todos.instructions",
        prompt: "bot.template.todos.prompt",
        needs: Needs::Nothing,
        jobs: &[JobDef::Cron("30 9 * * 1", "bot.wizard.when.monday930")],
    },
    TemplateDef {
        id: "briefing",
        icon: "☀️",
        category: "bot.wizard.category.daily",
        label: "bot.template.briefing",
        what: "bot.template.briefing.what",
        instructions: "bot.template.briefing.instructions",
        prompt: "bot.template.briefing.prompt",
        needs: Needs::Nothing,
        jobs: &[JobDef::Cron("0 9 * * 1-5", "bot.wizard.when.weekdays9")],
    },
    TemplateDef {
        id: "wrapUp",
        icon: "🌙",
        category: "bot.wizard.category.daily",
        label: "bot.template.wrapUp",
        what: "bot.template.wrapUp.what",
        instructions: "bot.template.wrapUp.instructions",
        prompt: "bot.template.wrapUp.prompt",
        needs: Needs::Nothing,
        jobs: &[JobDef::Cron("0 18 * * 1-5", "bot.wizard.when.weekdays18")],
    },
    TemplateDef {
        id: "weekly",
        icon: "📊",
        category: "bot.wizard.category.daily",
        label: "bot.template.weekly",
        what: "bot.template.weekly.what",
        instructions: "bot.template.weekly.instructions",
        prompt: "bot.template.weekly.prompt",
        needs: Needs::Nothing,
        jobs: &[JobDef::Cron("0 17 * * 5", "bot.wizard.when.friday17")],
    },
    TemplateDef {
        id: "taskKeeper",
        icon: "📋",
        category: "bot.wizard.category.daily",
        label: "bot.template.taskKeeper",
        what: "bot.template.taskKeeper.what",
        instructions: "bot.template.taskKeeper.instructions",
        prompt: "bot.template.taskKeeper.prompt",
        needs: Needs::Nothing,
        jobs: &[JobDef::CronSkip("*/30 9-19 * * 1-5", "bot.wizard.when.every30")],
    },
    TemplateDef {
        id: "siteWatch",
        icon: "🩺",
        category: "bot.wizard.category.watch",
        label: "bot.template.siteWatch",
        what: "bot.template.siteWatch.what",
        instructions: "bot.template.siteWatch.instructions",
        prompt: "bot.template.siteWatch.prompt",
        needs: Needs::Command("bot.wizard.command.siteHint"),
        jobs: &[JobDef::Standing("bot.wizard.when.standing")],
    },
    TemplateDef {
        id: "perfWatch",
        icon: "⏱",
        category: "bot.wizard.category.watch",
        label: "bot.template.perfWatch",
        what: "bot.template.perfWatch.what",
        instructions: "bot.template.perfWatch.instructions",
        prompt: "bot.template.perfWatch.prompt",
        needs: Needs::Command("bot.wizard.command.benchHint"),
        jobs: &[JobDef::Cron("0 2 * * *", "bot.wizard.when.daily2")],
    },
    TemplateDef {
        id: "guide",
        icon: "💬",
        category: "bot.wizard.category.other",
        label: "bot.template.guide",
        what: "bot.template.guide.what",
        instructions: "bot.template.guide.instructions",
        prompt: "bot.template.guide.instructions",
        needs: Needs::Nothing,
        jobs: &[],
    },
    TemplateDef {
        id: "blank",
        icon: "✳️",
        category: "bot.wizard.category.other",
        label: "bot.template.blank",
        what: "bot.template.blank.what",
        instructions: "bot.template.blank",
        prompt: "bot.template.blank",
        needs: Needs::Nothing,
        jobs: &[],
    },
];

/// The standing order a 사이트 상태 지킴이 keeps: every 10 minutes, at most 12 wake-ups a day.
const STANDING_EVERY_MINUTES: u32 = 10;
const STANDING_DAILY_CAP: u32 = 12;

/// MC-79: a schedule or trigger the person added in the wizard's 언제 step, beside the template's own.
#[derive(Debug, Clone, PartialEq)]
pub enum Extra {
    /// At a time or every N minutes (`cron`).
    Schedule { cron: String, prompt: String },
    /// A GitHub event of `repo` (the workspace's when None); MC-97: only those with `label` (a `where` condition) when given.
    Github { event: GitHubEvent, repo: Option<String>, label: Option<String>, prompt: String },
    /// Another bot's request ending (`@name`, `@*` for any).
    Bot { bot: String, prompt: String },
    /// An event `mighty event emit` sends.
    Event { name: String, prompt: String },
}

impl Extra {
    /// The page's `{ kind, cron | event, repo | bot | name, prompt }`, or the locale key of why it cannot be.
    pub fn read(value: &Value) -> Result<Extra, &'static str> {
        let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::trim).unwrap_or("").to_string();
        let prompt = text("prompt");
        match text("kind").as_str() {
            "schedule" => {
                let cron = text("cron");
                if cron.is_empty() {
                    return Err("bot.wizard.time.noDays");
                }
                Ok(Extra::Schedule { cron, prompt })
            }
            "github" => {
                let event = GitHubEvent::parse(&text("event")).ok_or("bot.wizard.github.needsEvent")?;
                let repo = Some(text("repo")).filter(|r| !r.is_empty());
                if repo.as_deref().is_some_and(|r| !mighty_core::automation::spec::valid_repo(r)) {
                    return Err("bot.new.needsRepo");
                }
                let label = Some(text("label")).filter(|l| !l.is_empty());
                if label.as_deref().is_some_and(|l| Condition::new("labels", Op::Has, l).validate().is_err()) {
                    return Err("bot.wizard.github.badLabel");
                }
                Ok(Extra::Github { event, repo, label, prompt })
            }
            "bot" => {
                let name = text("bot");
                let name = name.trim_start_matches('@').trim();
                if name.is_empty() {
                    return Err("bot.wizard.extra.needsBot");
                }
                Ok(Extra::Bot { bot: format!("@{name}"), prompt })
            }
            "event" => {
                let name = text("name");
                if name.is_empty() {
                    return Err("bot.wizard.extra.needsEvent");
                }
                Ok(Extra::Event { name, prompt })
            }
            _ => Err("bot.wizard.notMade"),
        }
    }

    fn prompt(&self) -> &str {
        match self {
            Extra::Schedule { prompt, .. } | Extra::Github { prompt, .. } | Extra::Bot { prompt, .. } | Extra::Event { prompt, .. } => {
                prompt
            }
        }
    }
}

/// MC-71/79: what the person made of the template's jobs in the 언제 step: which stay on (`on`), the times they set (`crons`), the
/// requests they wrote (`prompts`, the template's where None or empty), each by the template's job, and the jobs they added (`extra`).
#[derive(Debug, Default)]
pub struct JobChoices<'a> {
    pub on: Option<&'a [bool]>,
    pub crons: Option<&'a [Option<String>]>,
    pub prompts: Option<&'a [Option<String>]>,
    pub extra: &'a [Extra],
    /// MC-99: a trigger's rule as the person changed it, by the template's job (its own where None).
    pub rules: Option<&'a [Option<Rule>]>,
    /// MC-99: the MightyWork project a recipe watches.
    pub project: &'a str,
}

pub fn template(id: &str) -> Option<&'static TemplateDef> {
    TEMPLATES.iter().find(|t| t.id == id)
}

impl TemplateDef {
    pub fn blank(&self) -> bool {
        self.id == "blank"
    }

    /// The spec it makes with the jobs `on` keeps (all when None): its instructions and jobs, the prompts in the app's language;
    /// `repo` for the GitHub ones (their workspace's when empty), `command` for the ones that need one; `crons` the times the person
    /// set, by job (a schedule's own cron where None).
    #[cfg(test)]
    pub fn spec(&self, locale: &Locale, repo: &str, command: &str, on: Option<&[bool]>, crons: Option<&[Option<String>]>) -> BotSpec {
        self.spec_with(locale, repo, command, &JobChoices { on, crons, ..JobChoices::default() })
    }

    /// [`TemplateDef::spec`] with all the person chose in the 언제 step (MC-79): the requests they wrote and the jobs they added.
    pub fn spec_with(&self, locale: &Locale, repo: &str, command: &str, choices: &JobChoices) -> BotSpec {
        let (on, crons) = (choices.on, choices.crons);
        let repo = Some(repo.trim().to_string()).filter(|r| !r.is_empty());
        let command = command.trim();
        let prompt = || match self.needs {
            Needs::Command(_) => locale.get_with(self.prompt, &[("command", command)]),
            _ => locale.get(self.prompt),
        };
        let mut spec = BotSpec { instructions: (!self.blank()).then(|| locale.get(self.instructions)), ..BotSpec::default() };
        for (index, job) in self.jobs.iter().enumerate() {
            if on.is_some_and(|on| !on.get(index).copied().unwrap_or(true)) {
                continue;
            }
            let written = choices.prompts.and_then(|p| p.get(index)).and_then(|p| p.as_deref()).map(str::trim).filter(|p| !p.is_empty());
            let prompt = || written.map_or_else(prompt, str::to_string);
            let set = crons.and_then(|c| c.get(index)).and_then(|c| c.as_deref()).map(str::trim).filter(|c| !c.is_empty());
            let schedule = |cron: &str, catch_up: Option<CatchUp>| BotSchedule {
                cron: Some(set.unwrap_or(cron).into()),
                prompt: prompt(),
                catch_up,
                ..BotSchedule::default()
            };
            match *job {
                JobDef::Cron(cron, _) => spec.schedules.push(schedule(cron, None)),
                JobDef::CronSkip(cron, _) => spec.schedules.push(schedule(cron, Some(CatchUp::Skip))),
                trigger_job => {
                    let Some(mut trigger) = self.trigger_of(trigger_job, repo.clone(), command, choices.project, prompt()) else {
                        continue;
                    };
                    // MC-99: the rule as the person changed it in the 언제 step.
                    if let Some(rule) = choices.rules.and_then(|r| r.get(index)).and_then(Option::as_ref) {
                        rule.apply(&mut trigger);
                    }
                    spec.triggers.push(trigger);
                }
            }
        }
        // MC-79: the jobs the person added, each with its request (the template's where they wrote none).
        for extra in choices.extra {
            let prompt = Some(extra.prompt().to_string()).filter(|p| !p.is_empty()).unwrap_or_else(prompt);
            let trigger = |on: TriggerSource| BotTrigger::ask(on, prompt.clone());
            match extra {
                Extra::Schedule { cron, .. } => {
                    spec.schedules.push(BotSchedule { cron: Some(cron.clone()), prompt: prompt.clone(), ..BotSchedule::default() })
                }
                // MC-97: its label as a `where` condition.
                Extra::Github { event, repo: own, label, .. } => spec.triggers.push(BotTrigger {
                    conditions: label.iter().map(|l| Condition::new("labels", Op::Has, l)).collect(),
                    ..trigger(TriggerSource::Github { events: vec![*event], repo: own.clone().or_else(|| repo.clone()) })
                }),
                Extra::Bot { bot, .. } => spec.triggers.push(trigger(TriggerSource::Bot { bot: bot.clone(), on: vec![BotOutcome::Done] })),
                Extra::Event { name, .. } => spec.triggers.push(trigger(TriggerSource::Event { name: name.clone() })),
            }
        }
        spec
    }

    /// The trigger a template's (non-schedule) job makes, asking `prompt`; None for a schedule.
    fn trigger_of(&self, job: JobDef, repo: Option<String>, command: &str, project: &str, prompt: String) -> Option<BotTrigger> {
        Some(match job {
            JobDef::Github(event, _) => BotTrigger::ask(TriggerSource::Github { events: vec![event], repo }, prompt),
            JobDef::Event(name, _) => BotTrigger::ask(TriggerSource::Event { name: name.into() }, prompt),
            JobDef::Standing(_) => BotTrigger::ask(
                TriggerSource::Standing {
                    command: command.to_string(),
                    every_minutes: STANDING_EVERY_MINUTES,
                    daily_cap: STANDING_DAILY_CAP,
                },
                prompt,
            ),
            JobDef::Recipe(recipe, _) => recipe.trigger(repo, project, prompt),
            JobDef::Cron(..) | JobDef::CronSkip(..) => return None,
        })
    }

    /// MC-99: each job's rule as the 언제 step starts it (None for a schedule).
    fn rules(&self) -> Vec<Option<Rule>> {
        self.jobs.iter().map(|job| self.trigger_of(*job, None, "", "", String::new()).map(|t| Rule::of(&t))).collect()
    }
}

pub fn register(methods: &mut Registry) {
    methods.add("botWizard", |host: Arc<Host>, _args| async move { Ok(model(&host)) });
    methods.add("botWizardCreate", |host: Arc<Host>, args| async move { Ok(create(&host, &args).await) });
}

/// What the wizard picks from: the templates (their words in the app's language), the workspaces of this computer, the agents (and
/// whether each is installed), each agent's permission modes, and a friendly name.
fn model(host: &Arc<Host>) -> Value {
    let locale = host.locale;
    let templates: Vec<Value> = TEMPLATES
        .iter()
        .map(|t| {
            json!({
                "id": t.id,
                "icon": t.icon,
                "category": t.category,
                "label": locale.get(t.label),
                "what": locale.get(t.what),
                "instructions": if t.blank() { String::new() } else { locale.get(t.instructions) },
                "needs": match t.needs {
                    Needs::Nothing => "nothing",
                    Needs::Repo => "repo",
                    Needs::Command(_) => "command",
                    Needs::Project => "project",
                },
                "commandHint": match t.needs { Needs::Command(hint) => locale.get(hint), _ => String::new() },
                // MC-79: the request its jobs send, shown where the person may write another (none for 빈 봇).
                "prompt": if t.blank() { String::new() } else { locale.get(t.prompt) },
                // MC-99: a trigger's rule (its conditions, action and policy) the 언제 step shows and lets the person change.
                "jobs": t.jobs.iter().zip(t.rules()).map(|(j, rule)| json!({ "when": locale.get(j.when()), "cron": j.cron(), "rule": rule })).collect::<Vec<_>>(),
            })
        })
        .collect();
    let categories: Vec<Value> = CATEGORIES.iter().map(|c| json!({ "id": c, "label": locale.get(c) })).collect();
    let (workspaces, active) = {
        let view = host.view();
        let workspaces: Vec<Value> = view
            .snapshot
            .workspaces()
            .iter()
            .filter(|w| w.remote.is_none() && !w.is_bot_home())
            .map(|w| json!({ "id": w.id(), "name": w.name() }))
            .collect();
        (workspaces, view.snapshot.active_workspace_id.clone())
    };
    // MC-79: the bots there are, for a trigger on one's request ending.
    let mut bots: Vec<String> = host.view().snapshot.sessions().iter().filter_map(|s| s.bot.as_ref().map(|b| b.handle.clone())).collect();
    bots.sort();
    bots.dedup();
    let runs = host.runs();
    let agents: Vec<Value> = mighty_core::wire::PROVIDERS
        .iter()
        .map(|p| {
            let available = runs.as_ref().and_then(|r| r.runtime(p)).map(|r| r.available);
            // MC-71: the models it offers, as its model picker lists them (the built-in list until its CLI was read).
            let models: Vec<Value> = runs
                .as_ref()
                .map(|r| r.catalog_of(p).models)
                .unwrap_or_default()
                .into_iter()
                .filter(|m| !m.value.trim().is_empty())
                .map(|m| json!({ "value": m.value, "label": if m.display_name.trim().is_empty() { m.value.clone() } else { m.display_name } }))
                .collect();
            json!({
                "id": p,
                "label": mighty_run::providers::name(p),
                "available": available,
                "models": models,
                "permissions": mighty_core::normalize::permission_modes(p)
                    .iter()
                    .map(|m| json!({ "value": m, "label": mighty_composer::runtime::permission_label(p, m, &locale) }))
                    .collect::<Vec<_>>(),
            })
        })
        .collect();
    json!({
        "categories": categories,
        "templates": templates,
        "workspaces": workspaces,
        "activeWorkspace": active,
        "agents": agents,
        "bots": bots,
        "name": crate::bot::friendly_name(host),
    })
}

/// The wizard's request, made into a bot: its template's spec with the jobs left on, its name, agent, model, where it works, its
/// permission and daily budget, awake (MC-85: put to sleep later from its row); then its first words asked for (a one-sentence greeting) when
/// wanted. `{ paneId, handle }`, `{ cancelled: true }` when the folder picker was closed, or `{ error }`.
async fn create(host: &Arc<Host>, args: &Value) -> Value {
    let locale = host.locale;
    let Some(template) = arg(args, "template").and_then(template) else { return json!({ "error": "unknown template" }) };
    let text = |key: &str| arg(args, key).map(str::trim).unwrap_or("").to_string();
    let jobs: Option<Vec<bool>> =
        args.get("jobs").and_then(Value::as_array).map(|a| a.iter().map(|v| v.as_bool().unwrap_or(true)).collect());
    let workspace = Some(text("workspace")).filter(|w| !w.is_empty());
    let (repo, command) = (text("repo"), text("command"));
    if template.needs == Needs::Repo && workspace.is_none() && jobs.as_ref().is_none_or(|on| on.iter().any(|b| *b)) && repo.is_empty() {
        return json!({ "error": locale.get("bot.new.needsRepo") });
    }
    if !repo.is_empty() && !mighty_core::automation::spec::valid_repo(&repo) {
        return json!({ "error": locale.get("bot.new.needsRepo") });
    }
    if matches!(template.needs, Needs::Command(_)) && command.is_empty() {
        return json!({ "error": locale.get("bot.wizard.needsCommand") });
    }
    // MC-99: a MightyWork recipe's project.
    let project = text("project");
    if template.needs == Needs::Project && jobs.as_ref().is_none_or(|on| on.iter().any(|b| *b)) && !valid_project(&project) {
        return json!({ "error": locale.get("bot.wizard.project.needed") });
    }
    // MC-99: each trigger's rule as the person changed it.
    let rules: Option<Vec<Option<Rule>>> = match args.get("rules").and_then(Value::as_array) {
        None => None,
        Some(items) => {
            match items.iter().map(|v| if v.is_null() { Ok(None) } else { serde_json::from_value::<Rule>(v.clone()).map(Some) }).collect() {
                Ok(rules) => Some(rules),
                Err(error) => return json!({ "error": locale.get_with("bot.wizard.rule.bad", &[("reason", &error.to_string())]) }),
            }
        }
    };
    let crons: Option<Vec<Option<String>>> =
        args.get("crons").and_then(Value::as_array).map(|a| a.iter().map(|v| v.as_str().map(str::to_string)).collect());
    let prompts: Option<Vec<Option<String>>> =
        args.get("prompts").and_then(Value::as_array).map(|a| a.iter().map(|v| v.as_str().map(str::to_string)).collect());
    // MC-79: the jobs the person added in the 언제 step.
    let extra = match args.get("extra").and_then(Value::as_array).map(|a| a.iter().map(Extra::read).collect::<Result<Vec<_>, _>>()) {
        Some(Err(key)) => return json!({ "error": locale.get(key) }),
        Some(Ok(extra)) => extra,
        None => Vec::new(),
    };
    if workspace.is_none() && repo.is_empty() && extra.iter().any(|e| matches!(e, Extra::Github { repo: None, .. })) {
        return json!({ "error": locale.get("bot.wizard.repo.needed") });
    }
    if template.blank() && extra.iter().any(|e| e.prompt().is_empty()) {
        return json!({ "error": locale.get("bot.wizard.job.needsPrompt") });
    }
    let choices = JobChoices {
        on: jobs.as_deref(),
        crons: crons.as_deref(),
        prompts: prompts.as_deref(),
        extra: &extra,
        rules: rules.as_deref(),
        project: &project,
    };
    let mut spec = template.spec_with(&locale, &repo, &command, &choices);
    spec.name = Some(text("name")).filter(|n| !n.is_empty());
    spec.agent = Some(text("agent")).filter(|a| !a.is_empty());
    spec.model = Some(text("model")).filter(|m| !m.is_empty());
    spec.permission = Some(text("permission")).filter(|p| !p.is_empty());
    spec.budget_per_day = args.get("budget").and_then(Value::as_f64).filter(|b| b.is_finite() && *b > 0.0);
    let instructions = text("instructions");
    if !instructions.is_empty() {
        spec.instructions = Some(instructions);
    }
    spec.workspace = workspace;
    if spec.workspace.is_none() && args.get("pickFolder").and_then(Value::as_bool) == Some(true) {
        match crate::frame::pick_folder_path(host).await {
            Some(folder) => spec.folder = Some(folder.to_string_lossy().to_string()),
            None => return json!({ "cancelled": true }),
        }
    }
    let handle = match crate::bot::create_from_spec(host, &spec).await {
        Ok(handle) => handle,
        Err(error) => return json!({ "error": error }),
    };
    let pane = host.view().snapshot.bot_pane(&handle).map(|p| p.id().to_string());
    let Some(pane) = pane else { return json!({ "error": "the bot was made but its pane is gone" }) };
    if args.get("greet").and_then(Value::as_bool) == Some(true) {
        let greeting = locale.get("bot.wizard.greetPrompt");
        if let Err((_, words)) = crate::phone::control_send(host, &pane, &greeting).await {
            host.log.line(&format!("bot wizard: @{handle}'s greeting was not sent: {words}"));
        }
    }
    host.log.line(&format!("bot wizard: @{handle} made from the {} template", template.id));
    json!({ "paneId": pane, "handle": handle })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every template makes a spec the app takes in every language, its jobs sent to the bot, its words written, its jobs kept or
    /// left as asked.
    #[test]
    fn every_template_makes_a_valid_spec_in_every_language() {
        for language in mighty_core::locale::LANGUAGES {
            let locale = Locale::new(language);
            for t in &TEMPLATES {
                let spec = t.spec(&locale, "owner/repo", "curl -sS https://example.com", None, None);
                spec.validate(&mighty_core::wire::PROVIDERS, mighty_core::normalize::permission_modes)
                    .unwrap_or_else(|e| panic!("{} in {language}: {e}", t.id));
                let (schedules, triggers) = spec.jobs("콩이");
                assert_eq!(schedules.len() + triggers.len(), t.jobs.len(), "{}", t.id);
                assert!(
                    schedules
                        .iter()
                        .map(|s| &s.action)
                        .chain(triggers.iter().map(|t| &t.action))
                        .all(|a| a.bot().as_deref() == Some("콩이"))
                );
                for key in [t.label, t.what, t.category].into_iter().chain(t.jobs.iter().map(|j| j.when())) {
                    assert!(!locale.get(key).starts_with("bot."), "{key} has its words in {language}");
                }
                if !t.blank() {
                    assert!(
                        !locale.get(t.instructions).starts_with("bot.") && !locale.get(t.prompt).starts_with("bot."),
                        "{} in {language}",
                        t.id
                    );
                }
                let none_on = vec![false; t.jobs.len()];
                let (s, tr) = t.spec(&locale, "", "x", Some(&none_on), None).jobs("콩이");
                assert!(s.is_empty() && tr.is_empty(), "{}: jobs left off", t.id);
            }
        }
        assert_eq!(TEMPLATES.iter().map(|t| t.id).collect::<std::collections::HashSet<_>>().len(), TEMPLATES.len(), "ids are unique");
        assert!(TEMPLATES.iter().all(|t| CATEGORIES.contains(&t.category)));
    }

    /// A workspace bot's GitHub trigger may leave the repository to its workspace; a command reaches the prompt or the standing order.
    #[test]
    fn a_repository_left_empty_is_the_workspaces_and_a_command_is_used() {
        let locale = Locale::new("en");
        let spec = template("prReviewer").unwrap().spec(&locale, " ", "", None, None);
        assert!(spec.triggers.iter().all(|t| matches!(&t.on, TriggerSource::Github { repo: None, .. })));
        let site = template("siteWatch").unwrap().spec(&locale, "", "curl -s https://x.dev", None, None);
        assert!(matches!(&site.triggers[0].on, TriggerSource::Standing { command, .. } if command == "curl -s https://x.dev"));
        let perf = template("perfWatch").unwrap().spec(&locale, "", "cargo bench", None, None);
        assert!(perf.schedules[0].prompt.contains("cargo bench"));
        // The time the person set replaces the template's; an empty one keeps it.
        let briefing = template("briefing").unwrap();
        let set = briefing.spec(&locale, "", "", None, Some(&[Some("30 8 * * 1,3,5".into())]));
        assert_eq!(set.schedules[0].cron.as_deref(), Some("30 8 * * 1,3,5"));
        assert!(set.validate(&mighty_core::wire::PROVIDERS, mighty_core::normalize::permission_modes).is_ok());
        let kept = briefing.spec(&locale, "", "", None, Some(&[Some(" ".into())]));
        assert_eq!(kept.schedules[0].cron.as_deref(), Some("0 9 * * 1-5"));
    }

    /// MC-79: the jobs the person added go to the bot beside the template's, each with its request (the template's where none was
    /// written); a template job's request may be rewritten; what the page sends is read or refused with its reason.
    #[test]
    fn the_jobs_added_in_the_when_step_join_the_templates() {
        let locale = Locale::new("ko");
        let read = |v: Value| Extra::read(&v);
        let extra = vec![
            read(json!({ "kind": "schedule", "cron": "0 7 * * 1-5", "prompt": "" })).unwrap(),
            read(json!({ "kind": "github", "event": "issue_opened", "repo": "", "label": "bot:dev", "prompt": "이슈 봐 줘" })).unwrap(),
            read(json!({ "kind": "bot", "bot": "@콩이", "prompt": "" })).unwrap(),
            read(json!({ "kind": "event", "name": "release.done", "prompt": "" })).unwrap(),
        ];
        let briefing = template("briefing").unwrap();
        let prompts = [Some("오늘 할 일만".to_string())];
        let choices = JobChoices { prompts: Some(&prompts), extra: &extra, ..JobChoices::default() };
        let spec = briefing.spec_with(&locale, "owner/repo", "", &choices);
        assert!(spec.validate(&mighty_core::wire::PROVIDERS, mighty_core::normalize::permission_modes).is_ok());
        assert_eq!(spec.schedules.len(), 2);
        assert_eq!(spec.schedules[0].prompt, "오늘 할 일만", "the template job's request as written");
        assert_eq!(spec.schedules[1].cron.as_deref(), Some("0 7 * * 1-5"));
        assert_eq!(spec.schedules[1].prompt, locale.get(briefing.prompt), "an added job with no request sends the template's");
        assert!(
            matches!(&spec.triggers[0].on, TriggerSource::Github { events, repo: Some(r), .. } if events == &[GitHubEvent::IssueOpened] && r == "owner/repo")
        );
        assert_eq!(spec.triggers[0].prompt, "이슈 봐 줘");
        // MC-97: its label is a `where` condition; the template's own triggers take every event.
        assert_eq!(spec.triggers[0].conditions, vec![Condition::new("labels", Op::Has, "bot:dev")]);
        assert!(spec.triggers[1].conditions.is_empty());
        assert!(matches!(&spec.triggers[1].on, TriggerSource::Bot { bot, on } if bot == "@콩이" && on == &[BotOutcome::Done]));
        assert!(matches!(&spec.triggers[2].on, TriggerSource::Event { name } if name == "release.done"));
        let (schedules, triggers) = spec.jobs("하루");
        assert_eq!((schedules.len(), triggers.len()), (2, 3));
        // Turned off, the template's job goes; the added ones stay.
        let off = briefing.spec_with(&locale, "", "", &JobChoices { on: Some(&[false]), extra: &extra[..1], ..JobChoices::default() });
        assert_eq!(off.schedules.len(), 1);
        assert_eq!(read(json!({ "kind": "github", "event": "nope" })), Err("bot.wizard.github.needsEvent"));
        assert_eq!(read(json!({ "kind": "github", "event": "pr_opened", "repo": "not a repo" })), Err("bot.new.needsRepo"));
        assert_eq!(read(json!({ "kind": "github", "event": "pr_opened", "label": "a\u{7}" })), Err("bot.wizard.github.badLabel"));
        assert_eq!(read(json!({ "kind": "bot", "bot": " @ " })), Err("bot.wizard.extra.needsBot"));
        assert_eq!(read(json!({ "kind": "event", "name": "" })), Err("bot.wizard.extra.needsEvent"));
    }

    /// MC-99: the three recipes make their whole rule (source, where, do, with, policy) and validate in every language; a MightyWork
    /// one watches the project given; a rule the person changed in the 언제 step replaces the recipe's.
    #[test]
    fn the_recipes_make_their_rules() {
        use mighty_core::automation::spec::ActionKind;
        for language in mighty_core::locale::LANGUAGES {
            let locale = Locale::new(language);
            for id in ["ciDevelop", "mwRelease", "mwDevelop"] {
                let choices = JobChoices { project: "MC", ..JobChoices::default() };
                let spec = template(id).unwrap().spec_with(&locale, "owner/repo", "", &choices);
                spec.validate(&mighty_core::wire::PROVIDERS, mighty_core::normalize::permission_modes)
                    .unwrap_or_else(|e| panic!("{id} in {language}: {e}"));
                assert_eq!(spec.triggers.len(), 1, "{id}");
            }
        }
        let locale = Locale::new("ko");
        let made = |id: &str, project: &str| {
            let choices = JobChoices { project, ..JobChoices::default() };
            template(id).unwrap().spec_with(&locale, "", "", &choices).triggers.remove(0)
        };
        let ci = made("ciDevelop", "");
        assert!(matches!(&ci.on, TriggerSource::Github { events, repo: None } if events == &[GitHubEvent::CiFailed]));
        assert_eq!(ci.conditions, vec![Condition::new("base", Op::Eq, "main").if_present()]);
        assert_eq!((ci.kind, ci.with.pr, ci.policy.once_per), (ActionKind::Develop, Some(true), Some(OncePer::Subject)));
        assert!(!ci.policy.approves(&mighty_core::automation::spec::Action { kind: ci.kind, with: ci.with.clone(), ..Default::default() }));

        let release = made("mwRelease", "MC");
        let TriggerSource::Mcp { server, tool, args, item, preset, allow_write, .. } = &release.on else { panic!("{:?}", release.on) };
        assert_eq!(
            (server.as_str(), tool.as_str(), preset.as_deref(), *allow_write),
            ("mightywork", "list_tasks", Some("mightywork"), false)
        );
        assert_eq!((args["project"].as_str(), item.prefix()), (Some("MC"), "mightywork.task"));
        assert_eq!(
            release.conditions.iter().map(Condition::words).collect::<Vec<_>>(),
            vec!["type=mightywork.task.stage_changed", "task.to=todo", r"task.title matches ^v?\d+\.\d+\.\d+ 릴리[스즈]"]
        );
        assert_eq!((release.kind, release.with.command.as_deref()), (ActionKind::Run, Some("/release {{task.version}}")));
        assert_eq!((release.policy.approve, release.policy.once_per), (Some(true), Some(OncePer::Subject)));
        let title = mighty_core::automation::watch::TriggerEvent {
            fields: [("task.title".to_string(), "v0.6.4 릴리스".to_string()), ("task.to".into(), "todo".into())].into(),
            ..Default::default()
        }
        .with_envelope("mcp:mightywork", "mightywork.task.stage_changed", "MC-120", None);
        assert!(mighty_core::automation::condition::first_miss(&release.conditions, &title).is_none(), "v0.6.4 릴리스 to todo passes");
        // The title spelled 릴리즈, or without the v, passes too; another task's title does not.
        for (title, passes) in [("0.6.4 릴리즈", true), ("v0.6.4 릴리즈 준비", true), ("릴리스 노트 정리", false)] {
            let event = mighty_core::automation::watch::TriggerEvent {
                fields: [("task.title".to_string(), title.to_string()), ("task.to".into(), "todo".into())].into(),
                ..Default::default()
            }
            .with_envelope("mcp:mightywork", "mightywork.task.stage_changed", "MC-121", None);
            assert_eq!(mighty_core::automation::condition::first_miss(&release.conditions, &event).is_none(), passes, "{title}");
        }

        let develop = made("mwDevelop", "MC");
        assert_eq!(develop.conditions.last(), Some(&Condition::new("task.tags", Op::Has, DEV_LABEL)));
        assert_eq!((develop.kind, develop.with.pr), (ActionKind::Develop, Some(true)));

        // The 언제 step: the rule the person changed replaces the recipe's.
        let rule = Rule {
            conditions: vec![],
            kind: ActionKind::Develop,
            with: With { pr: Some(false), ..With::default() },
            policy: Policy::default(),
        };
        let rules = [Some(rule.clone())];
        let choices = JobChoices { project: "MC", rules: Some(&rules), ..JobChoices::default() };
        let changed = template("mwDevelop").unwrap().spec_with(&locale, "", "", &choices).triggers.remove(0);
        assert_eq!(Rule::of(&changed), rule);
        assert_eq!(template("mwRelease").unwrap().rules()[0].as_ref().map(|r| r.kind), Some(ActionKind::Run));
        assert_eq!(template("briefing").unwrap().rules(), vec![None], "a schedule has no rule");
        assert!(valid_project("MC") && valid_project("my-proj_2") && !valid_project("") && !valid_project("a b"));
        let rule: Rule = serde_json::from_value(json!({"where": [{"field": "task.to", "op": "eq", "value": "todo"}], "do": "run",
            "with": {"command": "/x"}, "policy": {"approve": false}}))
        .unwrap();
        assert_eq!((rule.kind, rule.policy.approve), (ActionKind::Run, Some(false)));
    }
}
