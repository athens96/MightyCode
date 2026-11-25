//! 예약과 트리거 (MC-32), after 자동화 in Mighty Pet과 알림: today's digest and the daily budget, a workspace's schedules and
//! triggers (the app's own with their switch, edit and removal; `.mighty.yaml`'s read-only, with 승인 while their configuration is not
//! approved), each with its next fire, its last result and 지금 실행; the form that adds or edits the app's own; the webhook listener's
//! switch and port; MC-39's 앱이 꺼져 있어도 예약 실행 with the OS wake-up's state, the last start by the OS and 지금 다시 등록;
//! and the workspace's run history with 다시 실행 and 이어서 하기. Built from the automation module's memory only;
//! every change runs on a task.

use std::sync::Arc;

use chrono::{DateTime, Local, Utc};
use mighty_core::automation::condition::{Condition, Op};
use mighty_core::automation::records::RunRecord;
use mighty_core::automation::spec::{
    Action, BotOutcome, CatchUp, DEFAULT_DAILY_CAP, DEFAULT_EVERY_MINUTES, FireMode, GitHubEvent, ScheduleSpec, TriggerSource, TriggerSpec,
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
const KINDS: [&str; 7] = ["schedule", "github", "linear", "webhook", "standing", "event", "bot"];

/// What a form field's typing changes.
type Setter = Box<dyn Fn(&mut Form, String) + Send + Sync>;

/// MC-97: one row of a trigger's `where` as the form edits it: the field, the op (its place in [`Op::ALL`]) and the value (an `in`
/// list comma-separated).
#[derive(Debug, Clone, Default, PartialEq)]
struct ConditionRow {
    field: String,
    op: usize,
    value: String,
}

impl ConditionRow {
    fn of(condition: &Condition) -> ConditionRow {
        ConditionRow {
            field: condition.field.clone(),
            op: Op::ALL.iter().position(|op| *op == condition.op).unwrap_or(0),
            value: condition.values.join(", "),
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
        Some(Condition { field: field.to_string(), op, values })
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
                    TriggerSource::Webhook => {}
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

    fn action(&self) -> Action {
        let text = |t: &str| Some(t.trim().to_string()).filter(|t| !t.is_empty());
        Action {
            prompt: self.prompt.trim().to_string(),
            agent: Some(PROVIDERS.get(self.agent).copied().unwrap_or("claude").to_string()),
            model: text(&self.model),
            worktree: self.worktree,
            mode: if self.heartbeat { FireMode::Heartbeat } else { FireMode::New },
            pane: if self.heartbeat { text(&self.pane) } else { None },
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
                action: self.action(),
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
            "webhook" => TriggerSource::Webhook,
            _ => TriggerSource::Standing {
                command: self.when.trim().to_string(),
                every_minutes: self.every.trim().parse().unwrap_or(0),
                daily_cap: self.cap.trim().parse().unwrap_or(0),
            },
        };
        let conditions = self.conditions.iter().filter_map(ConditionRow::condition).collect();
        let spec = TriggerSpec { name, on, enabled: true, conditions, action: self.action() };
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
    if let Some(last) = automation::last_of(job) {
        facts.push(locale.get_with(
            "automation.jobs.last",
            &[("state", &automation::state_word(&locale, last.state.name())), ("at", &shown_text_time(&last.fired_at))],
        ));
    }
    if !facts.is_empty() {
        lines.push(caption(ui, facts.join(" · ")));
    }
    if let Item::Trigger(TriggerSpec { on: TriggerSource::Webhook, .. }) = &job.item {
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
    if let Item::Trigger(TriggerSpec { on: TriggerSource::Webhook, .. }) = &job.item {
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
    if kind != "webhook" {
        parts.push(field(ui, when_label, current.when.clone(), placeholder, "settings-jobs-form-when", Box::new(|f, t| f.when = t)));
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
    .enabled(host.writes() && !current.name.trim().is_empty() && !current.prompt.trim().is_empty());
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
        rows.push(El::stack(true, metrics::SPACING_SM, vec![field, op, value, remove]));
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
        };
        assert_eq!(row(" labels ", Op::Has, " bot:dev ").condition(), Some(Condition::new("labels", Op::Has, "bot:dev")));
        let listed = row("author", Op::In, "kim, lee,").condition().unwrap();
        assert_eq!(listed.values, vec!["kim".to_string(), "lee".to_string()]);
        assert_eq!(ConditionRow::of(&listed), row("author", Op::In, "kim, lee"));
        assert_eq!(row(" ", Op::Eq, "").condition(), None, "an empty row is left out");
        assert!(row("base", Op::Eq, "").condition().unwrap().validate().is_err(), "a field without a value is refused on save");
    }
}
