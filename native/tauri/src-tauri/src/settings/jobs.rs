//! 예약과 트리거 (MC-32), after 자동화 in Mighty Pet과 알림: today's digest and the daily budget, a workspace's schedules and
//! triggers (the app's own with their switch, edit and removal; `.mighty.yaml`'s read-only, with 승인 while their configuration is not
//! approved), each with its next fire, its last result and 지금 실행; the form that adds or edits the app's own; the webhook listener's
//! switch and port; MC-39's 앱이 꺼져 있어도 예약 실행 with the OS wake-up's state, the last start by the OS and 지금 다시 등록;
//! and the workspace's run history with 다시 실행 and 이어서 하기. Built from the automation module's memory only;
//! every change runs on a task. MC-98: a webhook's preset and map, an MCP polling trigger (MCP 폴링) with its preset, and a `where`
//! row's 값이 있을 때만 (`ifPresent`). MC-99: what a job does (`do`: 묻기, 개발, 실행, 이벤트) with what that takes (`with`), a
//! trigger's policy (승인, 대상마다 한 번, 동시 실행, 쉬는 시간, 하루 상한) and an MCP source's 쓰기 허용 (`allowWrite`).

use std::sync::Arc;

use chrono::{DateTime, Local, Utc};
use mighty_core::automation::condition::{Condition, Op};
use mighty_core::automation::mapping::{WebhookMap, WebhookPreset};
use mighty_core::automation::mcp_watch::{self, McpItem};
use mighty_core::automation::policy::{OncePer, Policy};
use mighty_core::automation::records::RunRecord;
use mighty_core::automation::spec::{
    Action, ActionKind, BotOutcome, CatchUp, DEFAULT_DAILY_CAP, DEFAULT_EVERY_MINUTES, FireMode, GitHubEvent, ScheduleSpec, TriggerSource,
    TriggerSpec, With,
};
use mighty_core::design_tokens::{Token, metrics};
use mighty_core::wire::PROVIDERS;
use mighty_native::kit::{ControlSize, TextStyle};
use mighty_native::ui::{El, Kind, Ui};

use super::{caption, folded, sub_heading};
use crate::automation::{self, Item, Job, Origin, store, triggers};
use crate::host::Host;

const FIELD_WIDTH: f64 = 300.0;
/// The history rows shown.
const HISTORY_ROWS: usize = 12;
/// The kinds the form makes, in its picker's order.
const KINDS: [&str; 8] = ["schedule", "github", "linear", "webhook", "standing", "event", "bot", "mcp"];

/// What a form field's typing changes.
type Setter = Box<dyn Fn(&mut Form, String) + Send + Sync>;

/// MC-97: one row of a trigger's `where` as the form edits it: the field, the op (its place in [`Op::ALL`]) and the value (an `in`
/// list comma-separated); MC-98: whether an event without the field passes (값이 있을 때만).
#[derive(Debug, Clone, Default, PartialEq)]
struct ConditionRow {
    field: String,
    op: usize,
    value: String,
    if_present: bool,
}

impl ConditionRow {
    fn of(condition: &Condition) -> ConditionRow {
        ConditionRow {
            field: condition.field.clone(),
            op: Op::ALL.iter().position(|op| *op == condition.op).unwrap_or(0),
            value: condition.values.join(", "),
            if_present: condition.if_present,
        }
    }

    /// The condition it makes; None for a row left empty.
    fn condition(&self) -> Option<Condition> {
        let (field, value) = (self.field.trim(), self.value.trim());
        if field.is_empty() && value.is_empty() {
            return None;
        }
        let op = Op::ALL.get(self.op).copied().unwrap_or_default();
        let values = if op.takes_list() {
            value.split(',').map(str::trim).filter(|v| !v.is_empty()).map(str::to_string).collect()
        } else {
            vec![value.to_string()]
        };
        Some(Condition { field: field.to_string(), op, values, if_present: self.if_present })
    }
}

/// The form's fields while it is open.
#[derive(Clone, Default)]
struct Form {
    /// The job edited (whether a trigger, its name); None for a new one.
    editing: Option<(bool, String)>,
    kind: usize,
    name: String,
    /// The cron line or RRULE; GitHub's events; Linear's states; a standing order's command.
    when: String,
    prompt: String,
    agent: usize,
    model: String,
    worktree: bool,
    heartbeat: bool,
    pane: String,
    skip_missed: bool,
    every: String,
    cap: String,
    /// MC-64: a GitHub trigger's repository (`owner/name`), empty for the workspace's own.
    repo: String,
    /// MC-97: a trigger's `where` (MC-66's author, base and label of a GitHub trigger are among them).
    conditions: Vec<ConditionRow>,
    /// MC-66: a bot chain's outcomes (`done, failed`).
    outcome: String,
    /// MC-98: a webhook's preset (its place in [`WebhookPreset::ALL`]) and its own map: type, subject, title, link, actor, and more
    /// fields as `name = expression` lines.
    webhook_preset: usize,
    map_type: String,
    map_subject: String,
    map_title: String,
    map_url: String,
    map_actor: String,
    map_fields: String,
    /// MC-98: an MCP source: the preset picked (0 none, else its place in [`mcp_watch::presets`] after 1), the server, the tool, its
    /// arguments (a JSON object), the minutes between looks, and its items: the list, key, field prefix, watched fields and type
    /// prefix.
    mcp_preset: usize,
    server: String,
    tool: String,
    args: String,
    mcp_every: String,
    item_list: String,
    item_key: String,
    item_name: String,
    item_watch: String,
    item_prefix: String,
    /// MC-99: an MCP source may call a tool that does not look read-only.
    allow_write: bool,
    /// MC-99: what it does (its place in [`ActionKind::ALL`]) and what that takes: no pull request (`with.pr: false`), merge, the
    /// command a run runs, the event an emit sends and its data as `name = value` lines.
    act: usize,
    no_pr: bool,
    merge: bool,
    command: String,
    emit_event: String,
    emit_data: String,
    /// MC-99: a trigger's policy: approval (0 the kind's default, 1 always, 2 never), once per subject, and the concurrency,
    /// cooldown minutes and daily cap as typed (empty: unset).
    approve: usize,
    once_per: bool,
    concurrency: String,
    cooldown: String,
    policy_cap: String,
}

/// MC-99: a number field of the policy: empty for none, else what it says (0 when it says no number, refused by the policy's check).
fn optional_number(text: &str) -> Option<u32> {
    let text = text.trim();
    (!text.is_empty()).then(|| text.parse().unwrap_or(0))
}

impl Form {
    fn of(job: &Job) -> Form {
        let action = job.action();
        let mut form = Form {
            editing: Some((matches!(job.item, Item::Trigger(_)), job.name().to_string())),
            name: job.name().to_string(),
            prompt: action.prompt.clone(),
            agent: PROVIDERS.iter().position(|p| *p == action.agent()).unwrap_or(0),
            model: action.model.clone().unwrap_or_default(),
            worktree: action.worktree,
            heartbeat: action.mode == FireMode::Heartbeat,
            pane: action.pane.clone().unwrap_or_default(),
            every: DEFAULT_EVERY_MINUTES.to_string(),
            cap: DEFAULT_DAILY_CAP.to_string(),
            act: ActionKind::ALL.iter().position(|k| *k == action.kind).unwrap_or(0),
            no_pr: !action.with.pr(),
            merge: action.with.merge(),
            command: action.with.command.clone().unwrap_or_default(),
            emit_event: action.with.event.clone().unwrap_or_default(),
            emit_data: action.with.data.iter().map(|(k, v)| format!("{k} = {v}")).collect::<Vec<_>>().join("\n"),
            ..Form::default()
        };
        match &job.item {
            Item::Schedule(s) => {
                form.when = s.when().to_string();
                form.skip_missed = s.catch_up == CatchUp::Skip;
            }
            Item::Trigger(t) => {
                form.kind = KINDS.iter().position(|k| *k == t.on.kind()).unwrap_or(0);
                form.conditions = t.conditions.iter().map(ConditionRow::of).collect();
                form.approve = match t.policy.approve {
                    None => 0,
                    Some(true) => 1,
                    Some(false) => 2,
                };
                form.once_per = t.policy.once_per.is_some();
                let number = |n: Option<u32>| n.map(|n| n.to_string()).unwrap_or_default();
                form.concurrency = number(t.policy.concurrency);
                form.cooldown = number(t.policy.cooldown_minutes);
                form.policy_cap = number(t.policy.daily_cap);
                match &t.on {
                    TriggerSource::Github { events, repo } => {
                        form.when = events.iter().map(|e| e.name()).collect::<Vec<_>>().join(", ");
                        form.repo = repo.clone().unwrap_or_default();
                    }
                    TriggerSource::Event { name } => form.when = name.clone(),
                    TriggerSource::Bot { bot, on } => {
                        form.when = bot.clone();
                        form.outcome = on.iter().map(|o| o.name()).collect::<Vec<_>>().join(", ");
                    }
                    TriggerSource::Linear { states } => form.when = states.join(", "),
                    TriggerSource::Webhook { preset, map } => {
                        form.webhook_preset = WebhookPreset::ALL.iter().position(|p| p == preset).unwrap_or(0);
                        let text = |value: &Option<String>| value.clone().unwrap_or_default();
                        form.map_type = text(&map.kind);
                        form.map_subject = text(&map.subject);
                        form.map_title = text(&map.title);
                        form.map_url = text(&map.url);
                        form.map_actor = text(&map.actor);
                        form.map_fields = map.fields.iter().map(|(k, v)| format!("{k} = {v}")).collect::<Vec<_>>().join("\n");
                    }
                    TriggerSource::Mcp { server, tool, args, every_minutes, item, preset, allow_write } => {
                        form.allow_write = *allow_write;
                        form.mcp_preset = preset
                            .as_deref()
                            .and_then(|p| mcp_watch::presets().iter().position(|known| known.name == p))
                            .map_or(0, |i| i + 1);
                        form.server = server.clone();
                        form.tool = tool.clone();
                        form.args = if args.is_null() { String::new() } else { args.to_string() };
                        form.mcp_every = every_minutes.to_string();
                        form.fill_item(item);
                    }
                    TriggerSource::Standing { command, every_minutes, daily_cap } => {
                        form.when = command.clone();
                        form.every = every_minutes.to_string();
                        form.cap = daily_cap.to_string();
                    }
                }
            }
        }
        form
    }

    /// MC-98: the item fields filled from `item`.
    fn fill_item(&mut self, item: &McpItem) {
        self.item_list = item.list.clone();
        self.item_key = item.key.clone();
        self.item_name = item.name.clone().unwrap_or_default();
        self.item_watch = item.watch.join(", ");
        self.item_prefix = item.prefix.clone().unwrap_or_default();
    }

    /// MC-98: the MCP preset at `index` in the picker (0 none) picked: its server, tool, arguments and items filled in.
    fn pick_mcp_preset(&mut self, index: usize) {
        self.mcp_preset = index;
        let Some(preset) = index.checked_sub(1).and_then(|i| mcp_watch::presets().into_iter().nth(i)) else { return };
        self.server = preset.server.to_string();
        self.tool = preset.tool.to_string();
        self.args = preset.args.to_string();
        self.mcp_every = mcp_watch::DEFAULT_MCP_MINUTES.to_string();
        self.fill_item(&preset.item);
    }

    /// MC-98: the webhook's own map; a `fields` line without `=` refused.
    fn webhook_map(&self, locale: &mighty_core::locale::Locale) -> Result<WebhookMap, String> {
        let text = |t: &str| Some(t.trim().to_string()).filter(|t| !t.is_empty());
        let mut fields = std::collections::BTreeMap::new();
        for line in self.map_fields.lines().map(str::trim).filter(|l| !l.is_empty()) {
            let (name, expr) = line.split_once('=').ok_or_else(|| locale.get_with("automation.form.badMapLine", &[("line", line)]))?;
            fields.insert(name.trim().to_string(), expr.trim().to_string());
        }
        Ok(WebhookMap {
            kind: text(&self.map_type),
            subject: text(&self.map_subject),
            title: text(&self.map_title),
            url: text(&self.map_url),
            actor: text(&self.map_actor),
            fields,
        })
    }

    /// MC-98: the MCP source the form describes; arguments that are not a JSON object refused.
    fn mcp_source(&self, locale: &mighty_core::locale::Locale) -> Result<TriggerSource, String> {
        let text = |t: &str| Some(t.trim().to_string()).filter(|t| !t.is_empty());
        let args = match self.args.trim() {
            "" => serde_json::Value::Null,
            written => serde_json::from_str(written).map_err(|_| locale.get("automation.form.badArgs"))?,
        };
        let every = match self.mcp_every.trim() {
            "" => mcp_watch::DEFAULT_MCP_MINUTES,
            written => written.parse().unwrap_or(0),
        };
        let item = McpItem {
            list: self.item_list.trim().to_string(),
            key: text(&self.item_key).unwrap_or_else(|| "key".into()),
            name: text(&self.item_name),
            watch: self.item_watch.split(',').map(str::trim).filter(|w| !w.is_empty()).map(str::to_string).collect(),
            prefix: text(&self.item_prefix),
            title: None,
            url: None,
        };
        let preset = self.mcp_preset.checked_sub(1).and_then(|i| mcp_watch::presets().into_iter().nth(i)).map(|p| p.name.to_string());
        Ok(TriggerSource::Mcp {
            server: self.server.trim().to_string(),
            tool: self.tool.trim().to_string(),
            args,
            every_minutes: every,
            item: Box::new(item),
            preset,
            allow_write: self.allow_write,
        })
    }

    /// MC-99: what it does as picked.
    fn act(&self) -> ActionKind {
        ActionKind::ALL.get(self.act).copied().unwrap_or_default()
    }

    /// MC-99: whether the request may be left empty (a run's command and an emit's event say what to do).
    fn prompt_optional(&self) -> bool {
        matches!(self.act(), ActionKind::Run | ActionKind::Emit)
    }

    fn action(&self, locale: &mighty_core::locale::Locale) -> Result<Action, String> {
        let text = |t: &str| Some(t.trim().to_string()).filter(|t| !t.is_empty());
        let kind = self.act();
        let mut with = With::default();
        match kind {
            ActionKind::Develop => {
                with.pr = self.no_pr.then_some(false);
                with.merge = self.merge.then_some(true);
            }
            ActionKind::Run => with.command = text(&self.command),
            ActionKind::Emit => {
                with.event = text(&self.emit_event);
                for line in self.emit_data.lines().map(str::trim).filter(|l| !l.is_empty()) {
                    let (name, value) =
                        line.split_once('=').ok_or_else(|| locale.get_with("automation.form.badMapLine", &[("line", line)]))?;
                    with.data.insert(name.trim().to_string(), value.trim().to_string());
                }
            }
            ActionKind::Ask => {}
        }
        Ok(Action {
            prompt: self.prompt.trim().to_string(),
            agent: Some(PROVIDERS.get(self.agent).copied().unwrap_or("claude").to_string()),
            model: text(&self.model),
            worktree: self.worktree,
            mode: if self.heartbeat { FireMode::Heartbeat } else { FireMode::New },
            pane: if self.heartbeat { text(&self.pane) } else { None },
            kind,
            with,
        })
    }

    /// MC-99: the trigger's policy as filled.
    fn policy(&self) -> Policy {
        Policy {
            approve: match self.approve {
                1 => Some(true),
                2 => Some(false),
                _ => None,
            },
            once_per: self.once_per.then_some(OncePer::Subject),
            concurrency: optional_number(&self.concurrency),
            cooldown_minutes: optional_number(&self.cooldown),
            daily_cap: optional_number(&self.policy_cap),
        }
    }

    /// Saves the form as one of the app's own jobs of `workspace`.
    fn save(&self, host: &Arc<Host>, workspace: &str) -> Result<(), String> {
        let name = self.name.trim().to_string();
        let replacing = self.editing.as_ref().map(|(_, name)| name.as_str());
        let kind = KINDS.get(self.kind).copied().unwrap_or("schedule");
        if kind == "schedule" {
            let when = self.when.trim();
            let rule = when.to_ascii_uppercase().contains("FREQ=");
            let spec = ScheduleSpec {
                name,
                cron: (!rule).then(|| when.to_string()),
                rrule: rule.then(|| when.to_string()),
                catch_up: if self.skip_missed { CatchUp::Skip } else { CatchUp::Once },
                enabled: true,
                action: self.action(&host.locale)?,
            };
            return automation::put_schedule(host, workspace, replacing.filter(|_| !self.editing_trigger()), spec);
        }
        let list = |text: &str| text.split(',').map(str::trim).filter(|t| !t.is_empty()).map(str::to_string).collect::<Vec<_>>();
        let on = match kind {
            "github" => {
                let names = list(&self.when);
                let events = names
                    .iter()
                    .map(|n| GitHubEvent::parse(n).ok_or_else(|| host.locale.get_with("automation.form.badEvent", &[("name", n)])))
                    .collect::<Result<Vec<_>, _>>()?;
                let repo = Some(self.repo.trim().to_string()).filter(|t| !t.is_empty());
                TriggerSource::Github { events, repo }
            }
            "event" => TriggerSource::Event { name: self.when.trim().to_string() },
            "bot" => {
                let outcomes = list(&self.outcome);
                let on = if outcomes.is_empty() {
                    vec![BotOutcome::Done]
                } else {
                    outcomes
                        .iter()
                        .map(|n| BotOutcome::parse(n).ok_or_else(|| host.locale.get_with("automation.form.badEvent", &[("name", n)])))
                        .collect::<Result<Vec<_>, _>>()?
                };
                TriggerSource::Bot { bot: self.when.trim().to_string(), on }
            }
            "linear" => TriggerSource::Linear { states: list(&self.when) },
            "webhook" => TriggerSource::Webhook {
                preset: WebhookPreset::ALL.get(self.webhook_preset).copied().unwrap_or_default(),
                map: Box::new(self.webhook_map(&host.locale)?),
            },
            "mcp" => self.mcp_source(&host.locale)?,
            _ => TriggerSource::Standing {
                command: self.when.trim().to_string(),
                every_minutes: self.every.trim().parse().unwrap_or(0),
                daily_cap: self.cap.trim().parse().unwrap_or(0),
            },
        };
        let conditions = self.conditions.iter().filter_map(ConditionRow::condition).collect();
        let spec = TriggerSpec { name, on, enabled: true, conditions, action: self.action(&host.locale)?, policy: self.policy() };
        automation::put_trigger(host, workspace, replacing.filter(|_| self.editing_trigger()), spec)
    }

    fn editing_trigger(&self) -> bool {
        self.editing.as_ref().is_some_and(|(trigger, _)| *trigger)
    }
}

fn redraw(host: &Host) {
    crate::native::touch(host, crate::settings::SURFACE);
}

/// MC-64: a workspace and a form the box takes on its next drawing (a bot sheet's 일정·트리거 추가…).
static PRESET: std::sync::Mutex<Option<(String, Form)>> = std::sync::Mutex::new(None);

/// MC-64: Settings opened on 예약과 트리거 with the bot's workspace chosen and a new schedule's form filled for it: sent to
/// `@handle` (a heartbeat), with its agent.
pub fn preset_for_bot(host: &Arc<Host>, workspace: &str, handle: &str, agent: &str) {
    let form = Form {
        heartbeat: true,
        pane: format!("@{handle}"),
        agent: PROVIDERS.iter().position(|p| *p == agent).unwrap_or(0),
        every: DEFAULT_EVERY_MINUTES.to_string(),
        cap: DEFAULT_DAILY_CAP.to_string(),
        ..Form::default()
    };
    *PRESET.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some((workspace.to_string(), form));
    crate::settings::open(host, Some("companion"));
    redraw(host);
}

/// A time as the box shows it: local, to the minute.
fn shown_time(at: DateTime<Utc>) -> String {
    at.with_timezone(&Local).format("%m-%d %H:%M").to_string()
}

fn shown_text_time(text: &str) -> String {
    DateTime::parse_from_rfc3339(text).map(|t| shown_time(t.with_timezone(&Utc))).unwrap_or_default()
}

fn money(value: f64) -> String {
    format!("${value:.2}")
}

pub fn jobs(host: &Arc<Host>, ui: &mut Ui) -> El {
    let locale = ui.locale();
    let mut rows = ui.settings_rows();
    let small = |ui: &Ui, title: String, press: Box<dyn Fn() + Send + Sync>| {
        ui.settings_push(ui.button(title, press), ControlSize::Small, false, false)
    };
    automation::refresh_if_stale(host);
    rows.add(caption(ui, locale.get("automation.jobs.description")));

    // Today's digest and the budget.
    let digest = automation::digest_today();
    let (_, spent, budget) = crate::usage_report::budget_reading(host);
    let budget_line = match budget {
        Some(budget) => locale.get_with("automation.jobs.budget", &[("spent", &money(spent)), ("budget", &money(budget))]),
        None => locale.get_with("automation.jobs.noBudget", &[("spent", &money(spent))]),
    };
    let digest_line = locale.get_with(
        "automation.jobs.digest",
        &[
            ("done", &digest.done.to_string()),
            ("failed", &digest.failed.to_string()),
            ("skipped", &digest.skipped.to_string()),
            ("running", &digest.running.to_string()),
            ("cost", &money(digest.cost_usd)),
        ],
    );
    rows.add(ui.settings_text(budget_line, TextStyle::size(12.0)).aid("settings-jobs-budget"));
    rows.add(caption(ui, digest_line).aid("settings-jobs-digest"));

    // The workspace.
    let workspaces: Vec<(String, String)> = {
        let view = host.view();
        view.snapshot
            .workspaces()
            .iter()
            .filter(|w| w.remote.is_none() && !w.path().is_empty())
            .map(|w| (w.id().to_string(), w.name().to_string()))
            .collect()
    };
    if workspaces.is_empty() {
        rows.add(caption(ui, locale.get("automation.jobs.noWorkspace")));
        return rows.finish();
    }
    let active = host.view().snapshot.active_workspace_id.clone();
    let chosen = ui.local("automation-jobs-workspace", || active.clone().unwrap_or_default());
    let form = ui.local("automation-jobs-form", || None::<Form>);
    // MC-64: a bot sheet's 일정·트리거 추가…: its workspace and its form.
    let preset = PRESET.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
    if let Some((preset_workspace, preset_form)) = preset {
        chosen.set(preset_workspace);
        form.set(Some(preset_form));
    }
    let index = workspaces.iter().position(|(id, _)| *id == chosen.get()).unwrap_or(0);
    let workspace = workspaces[index].0.clone();
    let ids: Vec<String> = workspaces.iter().map(|(id, _)| id.clone()).collect();
    let (to, picked) = (host.clone(), chosen.clone());
    let picker = ui
        .settings_popup(
            El::combo_box(index as i32, workspaces.iter().map(|(_, name)| name.clone().into()).collect()).on_select(move |i| {
                if let Some(id) = usize::try_from(i).ok().and_then(|i| ids.get(i)) {
                    picked.set(id.clone());
                    redraw(&to);
                }
            }),
            false,
        )
        .named(locale.get("automation.jobs.workspace"))
        .aid("settings-jobs-workspace");
    rows.add(ui.settings_labeled(ui.settings_text(locale.get("automation.jobs.workspace"), TextStyle::default()), picker, false));

    // `.mighty.yaml`'s state.
    if let Some(status) = automation::yaml_status(&workspace) {
        if let Some(error) = &status.error {
            rows.add(caption(ui, locale.get_with("automation.jobs.yamlError", &[("reason", error)])).aid("settings-jobs-yaml-error"));
        } else if !status.approved {
            let (to, id) = (host.clone(), workspace.clone());
            let approve = small(
                ui,
                locale.get("automation.jobs.approve"),
                Box::new(move || {
                    let (host, id) = (to.clone(), id.clone());
                    crate::native::act(&to, async move {
                        let approved = automation::approve(&host, &id).await;
                        automation::say(&host, approved.err());
                        Ok(())
                    });
                }),
            )
            .aid("settings-jobs-approve")
            .enabled(host.writes());
            let words = locale.get_with("automation.jobs.yamlPending", &[("hash", &status.hash[..status.hash.len().min(12)])]);
            rows.add(ui.settings_labeled(ui.settings_text(words, TextStyle::size(12.0)), approve, false));
        }
    }

    // The jobs.
    let all = automation::jobs();
    let mine: Vec<&Job> = all.iter().filter(|j| j.workspace == workspace).collect();
    let next = automation::next_fires();
    rows.add(sub_heading(ui, locale.get("automation.jobs.listTitle")));
    if mine.is_empty() {
        rows.add(caption(ui, locale.get("automation.jobs.empty")));
    }
    let listening = triggers::listening();
    for job in mine {
        rows.add(job_row(host, ui, job, next.get(&job.key).copied(), listening, &form, &workspace));
    }

    // The form.
    let editing = form.get();
    let body = form_body(host, ui, &form, &workspace);
    let title = locale.get(if editing.as_ref().is_some_and(|f| f.editing.is_some()) {
        "automation.form.editTitle"
    } else {
        "automation.form.addTitle"
    });
    rows.add(folded(ui, "settings-jobs-form", editing.is_some(), title, body));
    if let Some(message) = automation::message() {
        rows.add(caption(ui, message).aid("settings-jobs-message"));
    }

    // The webhook listener.
    rows.add(sub_heading(ui, locale.get("automation.listener.title")));
    let settings = automation::listener_settings();
    let to = host.clone();
    let switch = ui
        .settings_switch(locale.get("automation.listener.toggle"), settings.enabled, Some("settings-jobs-listener"), move |on| {
            automation::set_listener(&to, store::Listener { enabled: on, ..automation::listener_settings() });
        })
        .enabled(host.writes());
    rows.add(ui.settings_labeled(ui.settings_text(locale.get("automation.listener.toggle"), TextStyle::default()), switch, false));
    let typed_port = ui.local("automation-jobs-port", || settings.port.to_string());
    let into = typed_port.clone();
    let port_field = ui
        .settings_field(El::text_box(typed_port.get()).placeholder(store::DEFAULT_PORT.to_string()).on_text(move |t| into.set(t)), 11.0)
        .width(120.0)
        .named(locale.get("automation.listener.port"))
        .aid("settings-jobs-port");
    let (to, port_text) = (host.clone(), typed_port.clone());
    let save_port = small(
        ui,
        locale.get("automation.hook.save"),
        Box::new(move || match port_text.get().trim().parse::<u16>() {
            Ok(port) if port >= 1024 => automation::set_listener(&to, store::Listener { port, ..automation::listener_settings() }),
            _ => automation::say(&to, Some(to.locale.get("automation.listener.badPort"))),
        }),
    )
    .aid("settings-jobs-port-save")
    .enabled(typed_port.get().trim() != settings.port.to_string());
    rows.add(El::stack(true, metrics::SPACING_SM, vec![port_field, save_port]));
    let state = match (triggers::listening(), triggers::listener_error()) {
        (Some(port), _) => locale.get_with("automation.listener.on", &[("port", &port.to_string())]),
        (None, Some(error)) => locale.get_with("automation.listener.failed", &[("reason", &error)]),
        (None, None) if settings.enabled => locale.get("automation.listener.idle"),
        (None, None) => locale.get("automation.listener.off"),
    };
    rows.add(caption(ui, state).aid("settings-jobs-listener-state"));
    rows.add(caption(ui, locale.get("automation.listener.note")));

    // MC-39: while the app is closed — the OS starts it without a window at the next fire.
    rows.add(sub_heading(ui, locale.get("automation.wake.title")));
    let changeable = host.may_change_machine() && automation::wake::supported();
    let label = locale.get("automation.wake.toggle");
    let to = host.clone();
    let switch = ui
        .settings_switch(&label, automation::wake::enabled(), Some("settings-jobs-wake"), move |on| automation::wake::set_enabled(&to, on))
        .enabled(changeable);
    let words = ui.settings_titled(&label, locale.get("automation.wake.description"));
    rows.add(ui.settings_labeled(words, switch, true));
    let to = host.clone();
    let again = small(ui, locale.get("automation.wake.again"), Box::new(move || automation::wake::register_again(&to)))
        .aid("settings-jobs-wake-again")
        .enabled(changeable);
    let state = ui.settings_text(automation::wake::state_words(host), TextStyle::size(12.0)).aid("settings-jobs-wake-state");
    rows.add(ui.settings_labeled(state, again, false));
    if let Some(error) = automation::wake::error_words(host) {
        rows.add(caption(ui, error).aid("settings-jobs-wake-error"));
    }
    if let Some(last) = automation::wake::last_words(host) {
        rows.add(caption(ui, last).aid("settings-jobs-wake-last"));
    }
    rows.add(caption(ui, locale.get("automation.jobs.closedNote")));

    // The history.
    rows.add(sub_heading(ui, locale.get("automation.history.title")));
    let records = automation::recent(Some(&workspace), HISTORY_ROWS);
    if records.is_empty() {
        rows.add(caption(ui, locale.get("automation.history.empty")));
    }
    for (index, record) in records.iter().enumerate() {
        rows.add(history_row(host, ui, record, index));
    }
    rows.finish()
}

fn job_row(
    host: &Arc<Host>,
    ui: &mut Ui,
    job: &Job,
    next: Option<DateTime<Utc>>,
    listening: Option<u16>,
    form: &mighty_native::ui::Local<Option<Form>>,
    workspace: &str,
) -> El {
    let locale = ui.locale();
    let trigger = matches!(job.item, Item::Trigger(_));
    let aid = format!("settings-jobs-{}-{}", if trigger { "trigger" } else { "schedule" }, &automation::webhook::path_id(&job.key)[..8]);
    let what = match &job.item {
        Item::Schedule(s) => s.when().to_string(),
        Item::Trigger(t) => automation::trigger_spec_words(&locale, t),
    };
    let origin = if job.origin == Origin::Yaml { " · .mighty.yaml" } else { "" };
    let title = format!("{} · {} · {what}{origin}", job.name(), automation::kind_word(&locale, job.kind()));
    let mut lines = vec![ui.settings_text(title, TextStyle::size(12.0).medium().selectable())];
    let mut facts = vec![];
    if let Some(error) = &job.error {
        facts.push(locale.get_with("automation.jobs.invalid", &[("reason", error)]));
    } else if !job.approved {
        facts.push(locale.get("automation.jobs.needsApproval"));
    } else if !job.enabled() {
        facts.push(locale.get("automation.jobs.off"));
    } else if let Some(at) = next {
        facts.push(locale.get_with("automation.jobs.next", &[("at", &shown_time(at))]));
    }
    if let Item::Trigger(TriggerSpec { on: TriggerSource::Standing { daily_cap, .. }, .. }) = &job.item {
        facts.push(locale.get_with(
            "automation.jobs.wakes",
            &[("count", &triggers::wakes_today(&job.key).to_string()), ("cap", &daily_cap.to_string())],
        ));
    }
    // MC-98: why its source could not be read at its last look.
    if let Some(problem) = triggers::problem_of(&job.key) {
        facts.push(problem);
    }
    if let Some(last) = automation::last_of(job) {
        facts.push(locale.get_with(
            "automation.jobs.last",
            &[("state", &automation::state_word(&locale, last.state.name())), ("at", &shown_text_time(&last.fired_at))],
        ));
    }
    if !facts.is_empty() {
        lines.push(caption(ui, facts.join(" · ")));
    }
    if let Item::Trigger(TriggerSpec { on: TriggerSource::Webhook { .. }, .. }) = &job.item {
        let port = listening.unwrap_or_else(|| automation::listener_settings().port);
        lines.push(ui.settings_text(triggers::address(port, &job.key), TextStyle::size(11.0).selectable()).aid(format!("{aid}-address")));
    }
    let words = El::vstack(lines);

    let small = |ui: &Ui, title: String, press: Box<dyn Fn() + Send + Sync>| {
        ui.settings_push(ui.button(title, press), ControlSize::Small, false, false)
    };
    let mut controls = vec![];
    let (to, key) = (host.clone(), job.key.clone());
    controls.push(
        small(
            ui,
            locale.get("automation.jobs.runNow"),
            Box::new(move || {
                let (host, key) = (to.clone(), key.clone());
                crate::native::act(&to, async move {
                    let said = automation::run_now(&host, &key).await.err();
                    automation::say(&host, said);
                    Ok(())
                });
            }),
        )
        .aid(format!("{aid}-run"))
        .enabled(job.runnable()),
    );
    if let Item::Trigger(TriggerSpec { on: TriggerSource::Webhook { .. }, .. }) = &job.item {
        let (to, key) = (host.clone(), job.key.clone());
        controls.push(
            small(
                ui,
                locale.get("automation.jobs.copySecret"),
                Box::new(move || {
                    let (host, key) = (to.clone(), key.clone());
                    crate::native::act(&to, async move {
                        let said = match triggers::secret_of(&host, &key).await {
                            Ok(secret) => {
                                let clipboard = crate::platform::clipboard();
                                let copied = clipboard.write_secret(&secret).or_else(|_| clipboard.write_text(&secret));
                                match copied {
                                    Ok(()) => host.locale.get("automation.jobs.secretCopied"),
                                    Err(error) => host.locale.get_with("automation.webhooks.failed", &[("reason", &error.to_string())]),
                                }
                            }
                            Err(error) => host.locale.get_with("automation.webhooks.failed", &[("reason", &error)]),
                        };
                        automation::say(&host, Some(said));
                        Ok(())
                    });
                }),
            )
            .aid(format!("{aid}-secret")),
        );
    }
    if job.origin == Origin::App {
        let (to, ws, name) = (host.clone(), workspace.to_string(), job.name().to_string());
        let switch = ui.settings_switch(job.name(), job.enabled(), Some(&format!("{aid}-on")), move |on| {
            automation::set_enabled(&to, &ws, trigger, &name, on);
        });
        let (to, editing, opened) = (host.clone(), form.clone(), Form::of(job));
        controls.push(
            small(
                ui,
                locale.get("automation.jobs.edit"),
                Box::new(move || {
                    editing.set(Some(opened.clone()));
                    redraw(&to);
                }),
            )
            .aid(format!("{aid}-edit")),
        );
        let (to, ws, name) = (host.clone(), workspace.to_string(), job.name().to_string());
        controls.push(
            small(ui, locale.get("automation.webhooks.remove"), Box::new(move || automation::remove_own(&to, &ws, trigger, &name)))
                .aid(format!("{aid}-remove")),
        );
        controls.push(switch);
    } else {
        controls.push(ui.settings_pill(locale.get("automation.jobs.readOnly"), Token::Ink2, Token::Line));
    }
    let buttons = El::new(Kind::Panel { children: controls });
    ui.settings_labeled(words, buttons, true).aid(aid)
}

fn history_row(host: &Arc<Host>, ui: &mut Ui, record: &RunRecord, index: usize) -> El {
    let locale = ui.locale();
    let mut facts = vec![shown_text_time(&record.fired_at), record.name.clone(), automation::state_word(&locale, record.state.name())];
    if let Some(cost) = record.cost_usd {
        facts.push(money(cost));
    }
    if !record.reason.is_empty() {
        facts.push(record.reason.clone());
    }
    let mut lines = vec![ui.settings_text(facts.join(" · "), TextStyle::size(12.0).selectable())];
    if let Some(error) = &record.error {
        lines.push(caption(ui, error.clone()));
    }
    if let Some(url) = &record.url {
        lines.push(ui.settings_text(url.clone(), TextStyle::size(11.0).selectable()));
    }
    let small = |ui: &Ui, title: String, press: Box<dyn Fn() + Send + Sync>| {
        ui.settings_push(ui.button(title, press), ControlSize::Small, false, false)
    };
    let mut controls = vec![];
    let (to, id) = (host.clone(), record.id.clone());
    controls.push(
        small(
            ui,
            locale.get("automation.history.retry"),
            Box::new(move || {
                let (host, id) = (to.clone(), id.clone());
                crate::native::act(&to, async move {
                    let said = automation::retry(&host, &id).await.err();
                    automation::say(&host, said);
                    Ok(())
                });
            }),
        )
        .aid(format!("settings-jobs-history-retry-{index}")),
    );
    let pane_open = record.pane.as_deref().is_some_and(|p| host.view().snapshot.session(p).is_some());
    let (to, id) = (host.clone(), record.id.clone());
    controls.push(
        small(ui, locale.get("automation.history.continue"), Box::new(move || automation::open(&to, &id)))
            .aid(format!("settings-jobs-history-open-{index}"))
            .enabled(pane_open),
    );
    if let Some(url) = record.url.clone() {
        let to = host.clone();
        controls.push(
            small(
                ui,
                locale.get("automation.history.result"),
                Box::new(move || {
                    let url = url.clone();
                    crate::native::act(&to, async move {
                        tauri::async_runtime::spawn_blocking(move || crate::platform::open_url(&url)).await.map_err(|e| e.to_string())?
                    });
                }),
            )
            .aid(format!("settings-jobs-history-result-{index}")),
        );
    }
    ui.settings_labeled(El::vstack(lines), El::new(Kind::Panel { children: controls }), true)
}

fn form_body(host: &Arc<Host>, ui: &mut Ui, form: &mighty_native::ui::Local<Option<Form>>, workspace: &str) -> El {
    let locale = ui.locale();
    let current = form.get().unwrap_or_default();
    let mut parts = vec![];
    let field = |ui: &Ui, label: String, value: String, placeholder: &str, aid: &str, set: Setter| {
        let form = form.clone();
        let input = ui
            .settings_field(
                El::text_box(value)
                    .placeholder(placeholder)
                    .on_text(move |text| form.update(|f| set(f.get_or_insert_with(Form::default), text))),
                11.0,
            )
            .width(FIELD_WIDTH)
            .named(label.clone())
            .aid(aid.to_string());
        ui.settings_labeled(ui.settings_text(label, TextStyle::size(12.0)), input, false)
    };

    // The kind (fixed while editing).
    let kind_words: Vec<String> = KINDS.iter().map(|k| automation::kind_word(&locale, k)).collect();
    let picked = form.clone();
    let picker = ui
        .settings_popup(
            El::combo_box(current.kind as i32, kind_words.iter().map(|w| w.clone().into()).collect()).on_select(move |i| {
                picked.update(|f| f.get_or_insert_with(Form::default).kind = usize::try_from(i).unwrap_or(0));
            }),
            false,
        )
        .named(locale.get("automation.form.kind"))
        .aid("settings-jobs-form-kind")
        .enabled(current.editing.is_none());
    parts.push(ui.settings_labeled(ui.settings_text(locale.get("automation.form.kind"), TextStyle::size(12.0)), picker, false));
    parts.push(field(
        ui,
        locale.get("automation.form.name"),
        current.name.clone(),
        "",
        "settings-jobs-form-name",
        Box::new(|f, t| f.name = t),
    ));
    let kind = KINDS.get(current.kind).copied().unwrap_or("schedule");
    let (when_label, placeholder) = match kind {
        "schedule" => (locale.get("automation.form.when"), "0 9 * * 1-5"),
        "github" => {
            (locale.get("automation.form.events"), "pr_opened, review_requested, ci_failed, release_published, pr_merged, issue_opened")
        }
        "linear" => (locale.get("automation.form.states"), "Ready, In Review"),
        "standing" => (locale.get("automation.form.command"), "npm test 2>&1 | tail -5"),
        "event" => (locale.get("automation.form.event"), "release.done"),
        "bot" => (locale.get("automation.form.bot"), "@*"),
        _ => (String::new(), ""),
    };
    if kind != "webhook" && kind != "mcp" {
        parts.push(field(ui, when_label, current.when.clone(), placeholder, "settings-jobs-form-when", Box::new(|f, t| f.when = t)));
    }
    // MC-98: a webhook's preset and its own map; an MCP source's preset, call and items.
    if kind == "webhook" {
        parts.extend(webhook_fields(ui, form, &current, &field));
    }
    if kind == "mcp" {
        parts.extend(mcp_fields(host, ui, form, &current, &field));
    }
    if kind == "github" {
        parts.push(field(
            ui,
            locale.get("automation.form.repo"),
            current.repo.clone(),
            "owner/name",
            "settings-jobs-form-repo",
            Box::new(|f, t| f.repo = t),
        ));
        parts.push(caption(ui, locale.get("automation.form.repoNote")));
    }
    if kind == "event" {
        parts.push(caption(ui, locale.get("automation.form.eventNote")));
    }
    if kind == "bot" {
        parts.push(field(
            ui,
            locale.get("automation.form.outcome"),
            current.outcome.clone(),
            "done, failed",
            "settings-jobs-form-outcome",
            Box::new(|f, t| f.outcome = t),
        ));
        parts.push(caption(ui, locale.get("automation.form.botNote")));
    }
    if kind == "schedule" {
        parts.push(caption(ui, locale.get("automation.form.whenNote")));
        let to = form.clone();
        let skip =
            ui.settings_switch(locale.get("automation.form.skipMissed"), current.skip_missed, Some("settings-jobs-form-skip"), move |on| {
                to.update(|f| f.get_or_insert_with(Form::default).skip_missed = on);
            });
        parts.push(ui.settings_labeled(ui.settings_text(locale.get("automation.form.skipMissed"), TextStyle::size(12.0)), skip, false));
    }
    if kind == "standing" {
        parts.push(field(
            ui,
            locale.get("automation.form.every"),
            current.every.clone(),
            "15",
            "settings-jobs-form-every",
            Box::new(|f, t| f.every = t),
        ));
        parts.push(field(
            ui,
            locale.get("automation.form.cap"),
            current.cap.clone(),
            "5",
            "settings-jobs-form-cap",
            Box::new(|f, t| f.cap = t),
        ));
    }

    // MC-97: a trigger's `where`: a row per condition (field, op, value, −), and + for another.
    if kind != "schedule" {
        parts.push(condition_rows(host, ui, form, &current));
    }

    // MC-99: what it does, and what that takes.
    parts.extend(action_fields(host, ui, form, &current, &field));

    // What it does.
    let prompt_form = form.clone();
    let prompt = ui
        .settings_field(
            El::text_box(current.prompt.clone())
                .multiline()
                .placeholder(locale.get("automation.form.promptHint"))
                .on_text(move |text| prompt_form.update(|f| f.get_or_insert_with(Form::default).prompt = text)),
            11.0,
        )
        .width(FIELD_WIDTH)
        .named(locale.get("automation.form.prompt"))
        .aid("settings-jobs-form-prompt");
    parts.push(ui.settings_labeled(ui.settings_text(locale.get("automation.form.prompt"), TextStyle::size(12.0)), prompt, true));
    let agent_form = form.clone();
    let agent = ui
        .settings_popup(
            El::combo_box(current.agent as i32, PROVIDERS.iter().map(|p| mighty_run::providers::name(p).to_string().into()).collect())
                .on_select(move |i| agent_form.update(|f| f.get_or_insert_with(Form::default).agent = usize::try_from(i).unwrap_or(0))),
            false,
        )
        .named(locale.get("automation.form.agent"))
        .aid("settings-jobs-form-agent");
    parts.push(ui.settings_labeled(ui.settings_text(locale.get("automation.form.agent"), TextStyle::size(12.0)), agent, false));
    parts.push(field(
        ui,
        locale.get("automation.form.model"),
        current.model.clone(),
        "sonnet",
        "settings-jobs-form-model",
        Box::new(|f, t| f.model = t),
    ));
    let to = form.clone();
    let worktree =
        ui.settings_switch(locale.get("automation.form.worktree"), current.worktree, Some("settings-jobs-form-worktree"), move |on| {
            to.update(|f| f.get_or_insert_with(Form::default).worktree = on);
        });
    parts.push(ui.settings_labeled(ui.settings_text(locale.get("automation.form.worktree"), TextStyle::size(12.0)), worktree, false));
    let to = form.clone();
    let heartbeat =
        ui.settings_switch(locale.get("automation.form.heartbeat"), current.heartbeat, Some("settings-jobs-form-heartbeat"), move |on| {
            to.update(|f| f.get_or_insert_with(Form::default).heartbeat = on);
        });
    parts.push(ui.settings_labeled(ui.settings_text(locale.get("automation.form.heartbeat"), TextStyle::size(12.0)), heartbeat, false));
    if current.heartbeat {
        parts.push(field(
            ui,
            locale.get("automation.form.pane"),
            current.pane.clone(),
            &locale.get("automation.form.paneHint"),
            "settings-jobs-form-pane",
            Box::new(|f, t| f.pane = t),
        ));
    }
    // MC-99: a trigger's policy.
    if kind != "schedule" {
        parts.extend(policy_fields(ui, form, &current, &field));
    }
    parts.push(caption(ui, locale.get("automation.form.note")));

    // Save and cancel.
    let small = |ui: &Ui, title: String, press: Box<dyn Fn() + Send + Sync>| {
        ui.settings_push(ui.button(title, press), ControlSize::Small, false, false)
    };
    let (to, saving, ws) = (host.clone(), form.clone(), workspace.to_string());
    let save = small(
        ui,
        locale.get("automation.hook.save"),
        Box::new(move || {
            let filled = saving.get().unwrap_or_default();
            match filled.save(&to, &ws) {
                Ok(()) => {
                    saving.set(None);
                    automation::say(&to, Some(to.locale.get("automation.form.saved")));
                }
                Err(error) => automation::say(&to, Some(to.locale.get_with("automation.webhooks.failed", &[("reason", &error)]))),
            }
        }),
    )
    .aid("settings-jobs-form-save")
    .enabled(host.writes() && !current.name.trim().is_empty() && (current.prompt_optional() || !current.prompt.trim().is_empty()));
    let (to, clearing) = (host.clone(), form.clone());
    let cancel = small(
        ui,
        locale.get("daemon.quit.cancel"),
        Box::new(move || {
            clearing.set(None);
            redraw(&to);
        }),
    )
    .aid("settings-jobs-form-cancel");
    parts.push(El::stack(true, metrics::SPACING_SM, vec![save, cancel]));
    El::stack(false, metrics::SPACING_SM, parts)
}

/// MC-98: a text row of the webhook's or MCP source's fields: its label's key, its aid's part, its value, placeholder and setter.
type TextRow = (&'static str, &'static str, String, &'static str, fn(&mut Form, String));

/// A labeled text field of the form: its label, value, placeholder, aid and what typing changes.
type FieldOf<'a> = dyn Fn(&Ui, String, String, &str, &str, Setter) -> El + 'a;

/// MC-98: a webhook's preset picker and its map's fields.
fn webhook_fields(ui: &mut Ui, form: &mighty_native::ui::Local<Option<Form>>, current: &Form, field: &FieldOf) -> Vec<El> {
    let locale = ui.locale();
    let mut parts = vec![];
    let names: Vec<String> = WebhookPreset::ALL
        .iter()
        .map(|p| if p.is_generic() { locale.get("automation.form.webhookGeneric") } else { p.label().to_string() })
        .collect();
    let picked = form.clone();
    let picker = ui
        .settings_popup(
            El::combo_box(current.webhook_preset as i32, names.into_iter().map(Into::into).collect()).on_select(move |i| {
                picked.update(|f| f.get_or_insert_with(Form::default).webhook_preset = usize::try_from(i).unwrap_or(0));
            }),
            false,
        )
        .named(locale.get("automation.form.preset"))
        .aid("settings-jobs-form-webhook-preset");
    parts.push(ui.settings_labeled(ui.settings_text(locale.get("automation.form.preset"), TextStyle::size(12.0)), picker, false));
    let rows: [TextRow; 5] = [
        ("automation.form.mapType", "type", current.map_type.clone(), "deploy.{{env}}", |f, t| f.map_type = t),
        ("automation.form.mapSubject", "subject", current.map_subject.clone(), "/task/key", |f, t| f.map_subject = t),
        ("automation.form.mapTitle", "title", current.map_title.clone(), "pull_request.title || issue.title", |f, t| f.map_title = t),
        ("automation.form.mapUrl", "url", current.map_url.clone(), "task.url", |f, t| f.map_url = t),
        ("automation.form.mapActor", "actor", current.map_actor.clone(), "sender.login", |f, t| f.map_actor = t),
    ];
    for (key, part, value, placeholder, set) in rows {
        parts.push(field(ui, locale.get(key), value, placeholder, &format!("settings-jobs-form-map-{part}"), Box::new(set)));
    }
    let fields_form = form.clone();
    let fields = ui
        .settings_field(
            El::text_box(current.map_fields.clone())
                .multiline()
                .placeholder("task.to = task.stage")
                .on_text(move |text| fields_form.update(|f| f.get_or_insert_with(Form::default).map_fields = text)),
            11.0,
        )
        .width(FIELD_WIDTH)
        .named(locale.get("automation.form.mapFields"))
        .aid("settings-jobs-form-map-fields");
    parts.push(ui.settings_labeled(ui.settings_text(locale.get("automation.form.mapFields"), TextStyle::size(12.0)), fields, true));
    parts.push(caption(ui, locale.get("automation.form.mapNote")));
    parts
}

/// MC-98: an MCP source's preset picker, call (server, tool, arguments, minutes) and items (list, key, prefix, watched fields, type).
fn mcp_fields(host: &Arc<Host>, ui: &mut Ui, form: &mighty_native::ui::Local<Option<Form>>, current: &Form, field: &FieldOf) -> Vec<El> {
    let locale = ui.locale();
    let mut parts = vec![];
    let mut names = vec![locale.get("automation.form.mcpCustom")];
    names.extend(mcp_watch::presets().iter().map(|p| p.label.to_string()));
    let (to, picked) = (host.clone(), form.clone());
    let picker = ui
        .settings_popup(
            El::combo_box(current.mcp_preset as i32, names.into_iter().map(Into::into).collect()).on_select(move |i| {
                picked.update(|f| f.get_or_insert_with(Form::default).pick_mcp_preset(usize::try_from(i).unwrap_or(0)));
                redraw(&to);
            }),
            false,
        )
        .named(locale.get("automation.form.preset"))
        .aid("settings-jobs-form-mcp-preset");
    parts.push(ui.settings_labeled(ui.settings_text(locale.get("automation.form.preset"), TextStyle::size(12.0)), picker, false));
    let rows: [TextRow; 9] = [
        ("automation.form.mcpServer", "server", current.server.clone(), "mightywork", |f, t| f.server = t),
        ("automation.form.mcpTool", "tool", current.tool.clone(), "list_tasks", |f, t| f.tool = t),
        ("automation.form.mcpArgs", "args", current.args.clone(), "{\"project\": \"MC\", \"limit\": 100}", |f, t| f.args = t),
        ("automation.form.every", "every", current.mcp_every.clone(), "5", |f, t| f.mcp_every = t),
        ("automation.form.itemList", "list", current.item_list.clone(), "/tasks", |f, t| f.item_list = t),
        ("automation.form.itemKey", "key", current.item_key.clone(), "key", |f, t| f.item_key = t),
        ("automation.form.itemName", "name", current.item_name.clone(), "task", |f, t| f.item_name = t),
        ("automation.form.itemWatch", "watch", current.item_watch.clone(), "stage, tags", |f, t| f.item_watch = t),
        ("automation.form.itemPrefix", "prefix", current.item_prefix.clone(), "mcp.item", |f, t| f.item_prefix = t),
    ];
    for (key, part, value, placeholder, set) in rows {
        parts.push(field(ui, locale.get(key), value, placeholder, &format!("settings-jobs-form-mcp-{part}"), Box::new(set)));
    }
    // MC-99: a tool that does not look read-only, called on every look anyway.
    let to = form.clone();
    let allow = ui.settings_switch(
        locale.get("automation.form.allowWrite"),
        current.allow_write,
        Some("settings-jobs-form-mcp-allow-write"),
        move |on| {
            to.update(|f| f.get_or_insert_with(Form::default).allow_write = on);
        },
    );
    parts.push(ui.settings_labeled(ui.settings_text(locale.get("automation.form.allowWrite"), TextStyle::size(12.0)), allow, false));
    parts.push(caption(ui, locale.get("automation.form.mcpNote")));
    parts
}

/// MC-99: what the job does (묻기, 개발, 실행, 이벤트) and what that takes: a develop's pull request and merge, a run's command, an
/// emit's event and data.
fn action_fields(host: &Arc<Host>, ui: &mut Ui, form: &mighty_native::ui::Local<Option<Form>>, current: &Form, field: &FieldOf) -> Vec<El> {
    let locale = ui.locale();
    let mut parts = vec![];
    let names: Vec<String> = ActionKind::ALL.iter().map(|k| locale.get(automation::do_key(*k))).collect();
    let (to, picked) = (host.clone(), form.clone());
    let picker = ui
        .settings_popup(
            El::combo_box(current.act as i32, names.into_iter().map(Into::into).collect()).on_select(move |i| {
                picked.update(|f| f.get_or_insert_with(Form::default).act = usize::try_from(i).unwrap_or(0));
                redraw(&to);
            }),
            false,
        )
        .named(locale.get("automation.form.do"))
        .aid("settings-jobs-form-do");
    parts.push(ui.settings_labeled(ui.settings_text(locale.get("automation.form.do"), TextStyle::size(12.0)), picker, false));
    match current.act() {
        ActionKind::Develop => {
            let to = form.clone();
            let pr =
                ui.settings_switch(locale.get("automation.form.withPr"), !current.no_pr, Some("settings-jobs-form-with-pr"), move |on| {
                    to.update(|f| f.get_or_insert_with(Form::default).no_pr = !on);
                });
            parts.push(ui.settings_labeled(ui.settings_text(locale.get("automation.form.withPr"), TextStyle::size(12.0)), pr, false));
            let to = form.clone();
            let merge = ui.settings_switch(
                locale.get("automation.form.withMerge"),
                current.merge,
                Some("settings-jobs-form-with-merge"),
                move |on| {
                    to.update(|f| f.get_or_insert_with(Form::default).merge = on);
                },
            );
            parts.push(ui.settings_labeled(ui.settings_text(locale.get("automation.form.withMerge"), TextStyle::size(12.0)), merge, false));
        }
        ActionKind::Run => parts.push(field(
            ui,
            locale.get("automation.form.withCommand"),
            current.command.clone(),
            "/release {{task.version}}",
            "settings-jobs-form-with-command",
            Box::new(|f, t| f.command = t),
        )),
        ActionKind::Emit => {
            parts.push(field(
                ui,
                locale.get("automation.form.withEvent"),
                current.emit_event.clone(),
                "release.asked",
                "settings-jobs-form-with-event",
                Box::new(|f, t| f.emit_event = t),
            ));
            let data_form = form.clone();
            let data = ui
                .settings_field(
                    El::text_box(current.emit_data.clone())
                        .multiline()
                        .placeholder("version = {{task.version}}")
                        .on_text(move |text| data_form.update(|f| f.get_or_insert_with(Form::default).emit_data = text)),
                    11.0,
                )
                .width(FIELD_WIDTH)
                .named(locale.get("automation.form.withData"))
                .aid("settings-jobs-form-with-data");
            parts.push(ui.settings_labeled(ui.settings_text(locale.get("automation.form.withData"), TextStyle::size(12.0)), data, true));
        }
        ActionKind::Ask => {}
    }
    parts.push(caption(ui, locale.get(automation::do_note_key(current.act()))));
    parts
}

/// MC-99: a trigger's policy: approval (its kind's default, always, never), once per subject, concurrency, cooldown and daily cap.
fn policy_fields(ui: &mut Ui, form: &mighty_native::ui::Local<Option<Form>>, current: &Form, field: &FieldOf) -> Vec<El> {
    let locale = ui.locale();
    let mut parts = vec![ui.settings_text(locale.get("automation.form.policy"), TextStyle::size(12.0).medium())];
    let names =
        [locale.get("automation.form.approveDefault"), locale.get("automation.form.approveOn"), locale.get("automation.form.approveOff")];
    let picked = form.clone();
    let picker = ui
        .settings_popup(
            El::combo_box(current.approve as i32, names.into_iter().map(Into::into).collect()).on_select(move |i| {
                picked.update(|f| f.get_or_insert_with(Form::default).approve = usize::try_from(i).unwrap_or(0));
            }),
            false,
        )
        .named(locale.get("automation.form.approve"))
        .aid("settings-jobs-form-approve");
    parts.push(ui.settings_labeled(ui.settings_text(locale.get("automation.form.approve"), TextStyle::size(12.0)), picker, false));
    let to = form.clone();
    let once =
        ui.settings_switch(locale.get("automation.form.oncePer"), current.once_per, Some("settings-jobs-form-once-per"), move |on| {
            to.update(|f| f.get_or_insert_with(Form::default).once_per = on);
        });
    parts.push(ui.settings_labeled(ui.settings_text(locale.get("automation.form.oncePer"), TextStyle::size(12.0)), once, false));
    let rows: [TextRow; 3] = [
        ("automation.form.concurrency", "concurrency", current.concurrency.clone(), "1", |f, t| f.concurrency = t),
        ("automation.form.cooldown", "cooldown", current.cooldown.clone(), "30", |f, t| f.cooldown = t),
        ("automation.form.policyCap", "cap", current.policy_cap.clone(), "5", |f, t| f.policy_cap = t),
    ];
    for (key, part, value, placeholder, set) in rows {
        parts.push(field(ui, locale.get(key), value, placeholder, &format!("settings-jobs-form-policy-{part}"), Box::new(set)));
    }
    parts.push(caption(ui, locale.get("automation.form.policyNote")));
    parts
}

/// MC-97: the form's `where` rows: field, op, value and − each, then + for another, and what the fields are.
fn condition_rows(host: &Arc<Host>, ui: &mut Ui, form: &mighty_native::ui::Local<Option<Form>>, current: &Form) -> El {
    let locale = ui.locale();
    let mut rows = vec![ui.settings_text(locale.get("automation.form.where"), TextStyle::size(12.0).medium())];
    let ops: Vec<String> = Op::ALL.iter().map(|op| op.name().to_string()).collect();
    for (index, row) in current.conditions.iter().enumerate() {
        let aid = format!("settings-jobs-form-where-{index}");
        let text = |value: String, placeholder: &str, label: String, width: f64, part: &str, set: fn(&mut ConditionRow, String)| {
            let form = form.clone();
            ui.settings_field(
                El::text_box(value).placeholder(placeholder).on_text(move |typed| {
                    form.update(|f| {
                        if let Some(row) = f.get_or_insert_with(Form::default).conditions.get_mut(index) {
                            set(row, typed);
                        }
                    })
                }),
                11.0,
            )
            .width(width)
            .named(label)
            .aid(format!("{aid}-{part}"))
        };
        let field = text(row.field.clone(), "labels", locale.get("automation.form.whereField"), 110.0, "field", |r, t| r.field = t);
        let picked = form.clone();
        let op = ui
            .settings_popup(
                El::combo_box(row.op as i32, ops.iter().map(|o| o.clone().into()).collect()).on_select(move |i| {
                    picked.update(|f| {
                        if let Some(row) = f.get_or_insert_with(Form::default).conditions.get_mut(index) {
                            row.op = usize::try_from(i).unwrap_or(0);
                        }
                    })
                }),
                false,
            )
            .named(locale.get("automation.form.whereOp"))
            .aid(format!("{aid}-op"));
        let value = text(row.value.clone(), "bot:dev", locale.get("automation.form.whereValue"), 140.0, "value", |r, t| r.value = t);
        // MC-98: an event without the field passes this one.
        let toggled = form.clone();
        let if_present = El::check_box(row.if_present, locale.get("automation.form.whereIfPresent"))
            .named(locale.get("automation.form.whereIfPresent"))
            .aid(format!("{aid}-if-present"))
            .on_toggle(move |on| {
                toggled.update(|f| {
                    if let Some(row) = f.get_or_insert_with(Form::default).conditions.get_mut(index) {
                        row.if_present = on;
                    }
                })
            });
        let (to, removing) = (host.clone(), form.clone());
        let remove = ui
            .settings_push(
                ui.button("−", move || {
                    removing.update(|f| {
                        let conditions = &mut f.get_or_insert_with(Form::default).conditions;
                        if index < conditions.len() {
                            conditions.remove(index);
                        }
                    });
                    redraw(&to);
                }),
                ControlSize::Small,
                false,
                false,
            )
            .named(locale.get("automation.form.whereRemove"))
            .aid(format!("{aid}-remove"));
        rows.push(El::stack(true, metrics::SPACING_SM, vec![field, op, value, if_present, remove]));
    }
    let (to, adding) = (host.clone(), form.clone());
    let add = ui
        .settings_push(
            ui.button(locale.get("automation.form.whereAdd"), move || {
                adding.update(|f| f.get_or_insert_with(Form::default).conditions.push(ConditionRow::default()));
                redraw(&to);
            }),
            ControlSize::Small,
            false,
            false,
        )
        .aid("settings-jobs-form-where-add")
        .enabled(current.conditions.len() < mighty_core::automation::condition::MAX_CONDITIONS);
    rows.push(add);
    rows.push(caption(ui, locale.get("automation.form.whereNote")));
    El::stack(false, metrics::SPACING_SM, rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// MC-97: a `where` row makes its condition (an `in` list split at commas, an empty row none) and reads back from it.
    #[test]
    fn a_where_row_makes_its_condition_and_reads_back() {
        let row = |field: &str, op: Op, value: &str| ConditionRow {
            field: field.into(),
            op: Op::ALL.iter().position(|o| *o == op).unwrap(),
            value: value.into(),
            if_present: false,
        };
        assert_eq!(row(" labels ", Op::Has, " bot:dev ").condition(), Some(Condition::new("labels", Op::Has, "bot:dev")));
        let listed = row("author", Op::In, "kim, lee,").condition().unwrap();
        assert_eq!(listed.values, vec!["kim".to_string(), "lee".to_string()]);
        assert_eq!(ConditionRow::of(&listed), row("author", Op::In, "kim, lee"));
        assert_eq!(row(" ", Op::Eq, "").condition(), None, "an empty row is left out");
        assert!(row("base", Op::Eq, "").condition().unwrap().validate().is_err(), "a field without a value is refused on save");
        // MC-98: 값이 있을 때만 goes both ways.
        let optional = Condition::new("base", Op::Eq, "main").if_present();
        assert_eq!(ConditionRow::of(&optional).condition(), Some(optional));
    }

    /// MC-98: a webhook's map and an MCP source read from the form and back; a preset fills the MCP fields; bad input is refused.
    #[test]
    fn the_form_makes_a_webhook_map_and_an_mcp_source() {
        let locale = mighty_core::locale::Locale::new("en");
        let form = Form {
            map_type: "deploy.{{env}}".into(),
            map_subject: " ".into(),
            map_fields: "env = env\n\n task.to = task.stage ".into(),
            ..Form::default()
        };
        let map = form.webhook_map(&locale).unwrap();
        assert_eq!((map.kind.as_deref(), map.subject.as_deref()), (Some("deploy.{{env}}"), None));
        assert_eq!(map.fields.get("task.to").map(String::as_str), Some("task.stage"));
        assert!(Form { map_fields: "no equals".into(), ..Form::default() }.webhook_map(&locale).is_err());

        let mut form = Form::default();
        form.pick_mcp_preset(1);
        assert_eq!((form.server.as_str(), form.tool.as_str(), form.item_list.as_str()), ("mightywork", "list_tasks", "/tasks"));
        assert_eq!((form.item_watch.as_str(), form.item_prefix.as_str(), form.mcp_every.as_str()), ("stage, tags", "mightywork.task", "5"));
        form.args = r#"{"project": "MC", "limit": 100}"#.into();
        let source = form.mcp_source(&locale).unwrap();
        let TriggerSource::Mcp { args, item, preset, every_minutes, .. } = &source else { panic!("{source:?}") };
        assert_eq!((args["project"].as_str(), item.watch.len(), preset.as_deref(), *every_minutes), (Some("MC"), 2, Some("mightywork"), 5));
        let job = Job {
            key: "w:app:t:board".into(),
            workspace: "w".into(),
            workspace_name: "W".into(),
            workspace_path: String::new(),
            origin: Origin::App,
            approved: true,
            item: Item::Trigger(TriggerSpec {
                name: "board".into(),
                on: source.clone(),
                enabled: true,
                conditions: vec![],
                action: Action { prompt: "p".into(), ..Action::default() },
                policy: Policy::default(),
            }),
            error: None,
        };
        let back = Form::of(&job);
        assert_eq!((back.kind, back.mcp_preset), (KINDS.iter().position(|k| *k == "mcp").unwrap(), 1));
        assert_eq!(back.mcp_source(&locale).unwrap(), source, "it reads back what it made");
        assert!(Form { args: "[1".into(), ..form.clone() }.mcp_source(&locale).is_err());
    }

    /// MC-99: what a job does, what that takes and a trigger's policy read from the form and back.
    #[test]
    fn the_form_makes_an_action_and_a_policy_and_reads_them_back() {
        let locale = mighty_core::locale::Locale::new("en");
        let form = Form {
            prompt: "Fix it".into(),
            act: ActionKind::ALL.iter().position(|k| *k == ActionKind::Develop).unwrap(),
            merge: true,
            command: "ignored for develop".into(),
            approve: 2,
            once_per: true,
            concurrency: "2".into(),
            cooldown: " ".into(),
            policy_cap: "x".into(),
            ..Form::default()
        };
        let action = form.action(&locale).unwrap();
        assert_eq!(
            (action.kind, action.with.pr, action.with.merge, action.with.command.as_deref()),
            (ActionKind::Develop, None, Some(true), None)
        );
        let policy = form.policy();
        assert_eq!(
            (policy.approve, policy.once_per, policy.concurrency, policy.cooldown_minutes),
            (Some(false), Some(OncePer::Subject), Some(2), None)
        );
        assert!(policy.validate().unwrap_err().contains("dailyCap"), "a cap that is no number is refused");
        let emit = Form {
            act: ActionKind::ALL.iter().position(|k| *k == ActionKind::Emit).unwrap(),
            emit_event: "release.asked".into(),
            emit_data: "version = {{task.version}}\n\n".into(),
            ..Form::default()
        };
        assert!(emit.prompt_optional());
        let made = emit.action(&locale).unwrap();
        assert_eq!(made.with.data.get("version").map(String::as_str), Some("{{task.version}}"));
        assert!(Form { emit_data: "no equals".into(), ..emit }.action(&locale).is_err());
        let job = Job {
            key: "w:app:t:ci".into(),
            workspace: "w".into(),
            workspace_name: "W".into(),
            workspace_path: String::new(),
            origin: Origin::App,
            approved: true,
            item: Item::Trigger(TriggerSpec {
                name: "ci".into(),
                on: TriggerSource::Github { events: vec![GitHubEvent::CiFailed], repo: None },
                enabled: true,
                conditions: vec![],
                action: action.clone(),
                policy: Policy { daily_cap: Some(3), ..policy.clone() },
            }),
            error: None,
        };
        let back = Form::of(&job);
        assert_eq!(back.action(&locale).unwrap(), action, "it reads back what it made");
        assert_eq!(back.policy(), Policy { daily_cap: Some(3), ..policy });
    }
}
