//! MC-58: bots — an agent pane with a name (`@handle`) and standing instructions (`RunSession.bot`), called by that name from the
//! command line (`mighty agent send @reviewer …`), a schedule's heartbeat (`@reviewer` as the pane) and other agents (mighty-terminal's
//! `list_bots` and `ask_bot`). A bot stays an ordinary pane: its approvals, queue, phone and mirror work as they do for any pane.
//!
//! - [`open`]: the pane menu's 봇으로 만들기… / 봇 설정… sheet (handle and instructions; 봇 해제 when it is one).
//! - [`ask`]: one request to a bot by its handle, for an agent (the caller is never a bot, the bot is never the caller, the day's
//!   budget is respected); it queues when the bot is busy, as a typed request would.
//! - [`answer`]: where a bot is now and its last answer (redacted and capped), for the caller that waits.

use std::sync::Arc;

use mighty_core::bot_spec::BotSpec;
use mighty_core::model::{BOT_INSTRUCTIONS_LIMIT, PaneBot, RunSession};
use mighty_native::surface::{DialogButton, DialogResult, DialogSpec};
use mighty_native::ui::{Content, El, Local, Wrapping};
use serde_json::{Value, json};

use crate::host::{Host, Registry};
use crate::methods::text as arg;

/// The most of a bot's answer handed back to the agent that asked, in characters.
pub const ANSWER_LIMIT: usize = 12_000;

/// mighty-terminal's `list_bots`, `ask_bot` and `bot_answer` (agent_io.rs), for the agent in pane `caller`; `value` is the
/// arguments as the tool checked them (JSON).
pub async fn tool(host: &Arc<Host>, caller: &str, tool: &str, value: &str) -> Result<Value, String> {
    use mighty_delegation::agent_io::tools::{ASK_BOT, BOT_ANSWER, BOT_REPORT, CREATE_BOT, LIST_BOTS, UPDATE_BOT};
    // MC-73: a bot's report of what it just finished.
    if tool == BOT_REPORT {
        return Ok(crate::bot_report::file(host, caller, value));
    }
    // MC-65: a bot proposed by an agent waits for the person's approval sheet.
    if matches!(tool, CREATE_BOT | UPDATE_BOT) {
        let origin = {
            let view = host.view();
            let pane = view.snapshot.session(caller);
            let named = pane.and_then(|p| p.bot.as_ref()).map(|b| format!("@{}", b.handle));
            named.or_else(|| pane.map(|p| p.title().to_string())).unwrap_or_else(|| host.locale.get("bot.approve.anAgent"))
        };
        return Ok(match propose(host, value, tool == UPDATE_BOT, origin) {
            Ok(()) => json!({ "pending": true, "message": host.locale.get("bot.approve.pending") }),
            Err(words) => json!({ "refused": words }),
        });
    }
    let args: Value = serde_json::from_str(value).map_err(|_| "Unreadable arguments.".to_string())?;
    let bot = args.get("bot").and_then(Value::as_str).unwrap_or_default().to_string();
    match tool {
        LIST_BOTS => Ok(json!({ "bots": list(host, caller) })),
        ASK_BOT => {
            let prompt = args.get("prompt").and_then(Value::as_str).unwrap_or_default();
            // A refusal is an answer with its reason: a tool error reaches the agent only as the socket's one fixed sentence.
            let (pane, receipt) = match ask(host, caller, &bot, prompt).await {
                Ok(sent) => sent,
                Err(refusal) => return Ok(json!({ "bot": shown(&bot), "refused": refusal.words(host) })),
            };
            let title = host.view().snapshot.session(&pane).map(|p| p.title().to_string());
            Ok(json!({ "bot": shown(&bot), "pane": title, "sent": receipt, "next": "bot_answer" }))
        }
        BOT_ANSWER => {
            let pane = host.view().snapshot.bot_pane(&bot).map(|p| p.id().to_string());
            let Some(pane) = pane else { return Ok(json!({ "bot": shown(&bot), "refused": AskRefusal::NoBot(shown(&bot)).words(host) })) };
            if args.get("wait").and_then(Value::as_bool).unwrap_or(false) {
                let until = tokio::time::Instant::now() + ANSWER_WAIT;
                while state(host, &pane) == Some("running") && tokio::time::Instant::now() < until {
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                }
            }
            let mut answer = answer(host, &pane).await;
            answer["bot"] = json!(shown(&bot));
            Ok(answer)
        }
        _ => Err("Unknown agent tool.".into()),
    }
}

/// A name as the agent sees it: `@리뷰어` (as the bot's own name says it when there is one).
fn shown(handle: &str) -> String {
    format!("@{}", PaneBot::handle_of(handle).unwrap_or_else(|| handle.trim().trim_start_matches('@').to_string()))
}

pub fn register(methods: &mut Registry) {
    methods.add("botSleep", |host: Arc<Host>, args| async move {
        if let Some(id) = arg(&args, "paneId") {
            let on = args.get("asleep").and_then(Value::as_bool).unwrap_or(true);
            set_asleep(&host, id, on).await;
        }
        Ok(Value::Null)
    });
    methods.add("paneBot", |host: Arc<Host>, args| async move {
        if let Some(id) = arg(&args, "paneId") {
            open(&host, id.to_string());
        }
        Ok(Value::Null)
    });
}

/// Why a handle cannot be saved on pane `id`: not a handle, or another pane's.
fn refusal(host: &Host, id: &str, handle: &str, instructions: &str) -> Option<String> {
    let locale = host.locale;
    let Some(handle) = PaneBot::handle_of(handle) else {
        return Some(locale.get("bot.error.handle"));
    };
    if instructions.trim().len() > BOT_INSTRUCTIONS_LIMIT {
        return Some(locale.get("bot.error.instructions"));
    }
    let view = host.view();
    view.snapshot.bot_pane(&handle).filter(|pane| pane.id() != id).map(|_| locale.get_with("bot.error.taken", &[("handle", &handle)]))
}

/// The pane menu's bot sheet: the name and the instructions; MC-64: the permission its runs take, its own daily budget, whether
/// it sleeps, and its schedules and triggers (일정·트리거 추가… saves and opens them in Settings with the form filled for it). An
/// agent pane of this computer only, never a delegated child.
pub fn open(host: &Arc<Host>, id: String) {
    let Some(pane) = host.view().snapshot.session(&id).cloned() else { return };
    if !can_be_bot(host, &pane) {
        return;
    }
    let locale = host.locale;
    let current = pane.bot.clone();
    let provider = pane.provider().to_string();
    let modes: Vec<&'static str> = mighty_core::normalize::permission_modes(&provider).to_vec();
    let mode_now = pane.settings.as_ref().map(|s| s.permission_mode.clone()).unwrap_or_else(|| "manual".into());
    let handle = Local::new(current.as_ref().map(|b| b.handle.clone()).unwrap_or_default());
    let instructions = Local::new(current.as_ref().and_then(|b| b.instructions.clone()).unwrap_or_default());
    let budget = Local::new(current.as_ref().and_then(|b| b.budget_per_day).map(|b| format!("{b:.2}")).unwrap_or_default());
    let asleep = Local::new(current.as_ref().is_some_and(PaneBot::is_asleep));
    let mode = Local::new(modes.iter().position(|m| *m == mode_now).unwrap_or(0));
    let note = Local::new(String::new());
    let jobs_lines = current.as_ref().map(|b| job_lines(host, &b.handle)).unwrap_or_default();
    let (handle_field, instructions_field, note_text) = (handle.clone(), instructions.clone(), note.clone());
    let (budget_field, asleep_field, mode_field) = (budget.clone(), asleep.clone(), mode.clone());
    let mode_words: Vec<String> = modes.iter().map(|m| mighty_composer::runtime::permission_label(&provider, m, &locale)).collect();
    let title = locale.get("bot.dialog.titleEdit");
    let mut spec = DialogSpec::new(title, move |_| {
        let wrapped = |words: String| {
            let mut block = El::text_block(words);
            if let Some(text) = block.text_mut() {
                text.wrapping = Wrapping::Wrap;
            }
            block
        };
        let into_handle = handle_field.clone();
        let into_instructions = instructions_field.clone();
        let into_budget = budget_field.clone();
        let into_asleep = asleep_field.clone();
        let into_mode = mode_field.clone();
        let name = El::text_box(handle_field.get())
            .header(locale.get("bot.dialog.handle"))
            .placeholder(locale.get("bot.dialog.handleHint"))
            .on_text(move |words| into_handle.set(words));
        let words = El::text_box(instructions_field.get())
            .multiline()
            .header(locale.get("bot.dialog.instructions"))
            .placeholder(locale.get("bot.dialog.instructionsHint"))
            .on_text(move |words| into_instructions.set(words));
        let permission = El::combo_box(mode_field.get() as i32, mode_words.iter().map(|w| w.clone().into()).collect())
            .named(locale.get("bot.dialog.permission"))
            .on_select(move |i| into_mode.set(usize::try_from(i).unwrap_or(0)));
        let money = El::text_box(budget_field.get())
            .header(locale.get("bot.dialog.budget"))
            .placeholder(locale.get("bot.dialog.budgetHint"))
            .on_text(move |words| into_budget.set(words));
        let sleep = El::check_box(asleep_field.get(), locale.get("bot.dialog.asleep")).on_toggle(move |on| into_asleep.set(on));
        let mut parts = vec![
            wrapped(locale.get("bot.dialog.message")),
            name,
            words,
            wrapped(locale.get("bot.dialog.permission")),
            permission,
            money,
            sleep,
            wrapped(locale.get("bot.dialog.jobs")),
        ];
        if jobs_lines.is_empty() {
            parts.push(wrapped(locale.get("bot.dialog.noJobs")));
        }
        parts.extend(jobs_lines.iter().map(|line| wrapped(line.clone())));
        parts.push(wrapped(note_text.get()));
        Content::El(Box::new(El::stack(false, mighty_core::design_tokens::metrics::SPACING_SM, parts)))
    })
    .styled(None, None);
    spec.primary = Some(DialogButton::new(locale.get("bot.dialog.save")));
    spec.secondary = Some(DialogButton::new(locale.get("bot.dialog.addJob")));
    spec.close = Some(locale.get("bot.dialog.cancel"));
    let (checked_handle, checked_instructions, checked_budget, checker, pane_id) =
        (handle.clone(), instructions.clone(), budget.clone(), host.clone(), id.clone());
    spec.accept = Some(Box::new(move |answer| {
        if answer == DialogResult::None {
            return true;
        }
        let refused = refusal(&checker, &pane_id, checked_handle.get().trim(), &checked_instructions.get())
            .or_else(|| budget_of(&checked_budget.get()).err().map(|_| checker.locale.get("bot.error.budget")));
        match refused {
            Some(words) => {
                note.set(words);
                false
            }
            None => true,
        }
    }));
    let host = host.clone();
    tauri::async_runtime::spawn(async move {
        let answer = crate::native::show_dialog(&host, spec).await;
        if answer == DialogResult::None {
            return;
        }
        let Some(name) = PaneBot::handle_of(&handle.get()) else { return };
        if refusal(&host, &id, &name, &instructions.get()).is_some() {
            return;
        }
        let Ok(budget_per_day) = budget_of(&budget.get()) else { return };
        let words = instructions.get().trim().to_string();
        let chosen = PaneBot {
            handle: name.clone(),
            instructions: (!words.is_empty()).then_some(words),
            auto: Some(false),
            asleep: asleep.get().then_some(true),
            budget_per_day,
        };
        let permission = modes.get(mode.get()).map(|m| m.to_string());
        let old = current.as_ref().map(|b| b.handle.clone());
        let workspace = pane.workspace_id().to_string();
        let _ = crate::frame::act(&host, async {
            let mut view = host.view();
            // MC-61: a name the user kept stays the app's (it may be named again only if it never was marked); any other is theirs.
            crate::composer::change(&mut view, &id, |pane| {
                let kept = pane.bot.as_ref().filter(|old| PaneBot::key(&old.handle) == PaneBot::key(&chosen.handle));
                let auto = if kept.is_some_and(|old| old.auto == Some(true)) { Some(true) } else { Some(false) };
                let mut settings = pane.settings.clone().unwrap_or_default();
                if let Some(permission) = &permission {
                    settings.permission_mode = permission.clone();
                }
                RunSession { bot: Some(PaneBot { auto, ..chosen.clone() }), settings: Some(settings), ..pane.clone() }
            })
        })
        .await;
        // MC-64: the jobs sent to the bot follow its new name.
        if let Some(old) = old.filter(|old| PaneBot::key(old) != PaneBot::key(&name)) {
            crate::automation::rename_bot(&host, &old, &name);
        }
        if answer == DialogResult::Secondary {
            crate::settings::jobs::preset_for_bot(&host, &workspace, &name, &provider);
        }
    });
}

/// MC-64: a daily budget as typed: none when empty, else a positive number of dollars (a `$` and spaces aside).
fn budget_of(text: &str) -> Result<Option<f64>, ()> {
    let text = text.trim().trim_start_matches('$').trim();
    if text.is_empty() {
        return Ok(None);
    }
    match text.parse::<f64>() {
        Ok(value) if value.is_finite() && value > 0.0 => Ok(Some(value)),
        _ => Err(()),
    }
}

/// MC-64: the sheet's lines of the bot's schedules and triggers: name, kind, when, the next fire or why it does not run.
fn job_lines(host: &Host, handle: &str) -> Vec<String> {
    let locale = host.locale;
    let next = crate::automation::next_fires();
    crate::automation::jobs_of_bot(handle)
        .iter()
        .map(|job| {
            let what = match &job.item {
                crate::automation::Item::Schedule(s) => s.when().to_string(),
                crate::automation::Item::Trigger(t) => crate::automation::trigger_spec_words(&locale, t),
            };
            let mut line = format!("• {} · {} · {what}", job.name(), crate::automation::kind_word(&locale, job.kind()));
            if !job.runnable() {
                line.push_str(&format!(" · {}", locale.get("automation.jobs.off")));
            } else if let Some(at) = next.get(&job.key) {
                let at = at.with_timezone(&chrono::Local).format("%m-%d %H:%M").to_string();
                line.push_str(&format!(" · {}", locale.get_with("automation.jobs.next", &[("at", &at)])));
            }
            line
        })
        .collect()
}

/// MC-64: the sidebar's 재우기 / 깨우기 on a bot (and `mighty bot pause|resume`).
pub async fn set_asleep(host: &Arc<Host>, id: &str, on: bool) {
    let _ = crate::frame::act(host, async {
        let mut view = host.view();
        crate::composer::change(&mut view, id, |pane| match &pane.bot {
            Some(bot) => RunSession { bot: Some(PaneBot { asleep: on.then_some(true), ..bot.clone() }), ..pane.clone() },
            None => pane.clone(),
        })
    })
    .await;
}

/// Whether `pane` may be a bot: an agent pane of a workspace on this computer (or reached over SSH), not a delegated child, not a
/// mirror's.
pub fn can_be_bot(host: &Host, pane: &RunSession) -> bool {
    let view = host.view();
    let remote = view.snapshot.workspace(pane.workspace_id()).is_some_and(|w| w.remote.is_some());
    pane.kind() == "claude" && pane.parent_session_id.is_none() && pane.mirror.is_none() && !remote
}

/// What [`ask`] refuses, in the caller's words.
#[derive(Debug, PartialEq)]
pub enum AskRefusal {
    /// No pane is that bot.
    NoBot(String),
    /// A bot asked: bots do not call bots (one level only).
    FromBot,
    /// The bot asked itself.
    ItSelf,
    /// The day's budget is spent.
    OverBudget(String),
    /// The send failed (the queue full, the pane gone).
    Failed(String),
}

impl AskRefusal {
    pub fn words(&self, host: &Host) -> String {
        let locale = host.locale;
        match self {
            AskRefusal::NoBot(handle) => locale.get_with("bot.ask.noBot", &[("handle", handle.trim_start_matches('@'))]),
            AskRefusal::FromBot => locale.get("bot.ask.fromBot"),
            AskRefusal::ItSelf => locale.get("bot.ask.itself"),
            AskRefusal::OverBudget(words) | AskRefusal::Failed(words) => words.clone(),
        }
    }
}

/// The handles of the bots another pane may ask (never the caller itself), with each one's pane title and state.
pub fn list(host: &Host, caller: &str) -> Vec<Value> {
    let rows: Vec<(String, Value)> = {
        let view = host.view();
        view.snapshot
            .bots()
            .filter(|pane| pane.id() != caller)
            .filter_map(|pane| {
                let bot = pane.bot.as_ref()?;
                let workspace = view.snapshot.workspace(pane.workspace_id()).map(|w| w.name().to_string());
                Some((
                    bot.handle.clone(),
                    json!({
                        "bot": format!("@{}", bot.handle),
                        "title": pane.title(),
                        "workspace": workspace,
                        "status": pane.status(),
                        "about": bot.instructions.as_deref().map(|t| first_line(t, 160)),
                        "agent": pane.provider(),
                        "asleep": bot.is_asleep(),
                    }),
                ))
            })
            .collect()
    };
    // MC-64: each bot's schedules and triggers, and its next run.
    rows.into_iter()
        .map(|(handle, mut row)| {
            row["jobs"] = json!(crate::automation::jobs_of_bot(&handle).iter().map(|j| j.name().to_string()).collect::<Vec<_>>());
            row["nextRun"] = json!(crate::automation::next_fire_of_bot(&handle).map(|at| at.to_rfc3339()));
            row
        })
        .collect()
}

fn first_line(text: &str, limit: usize) -> String {
    text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").chars().take(limit).collect()
}

/// Sends `prompt` to the bot `handle` names, for the agent in pane `caller`: refused when the caller is a bot, when it is the bot,
/// when no pane is that bot and when the day's budget is spent; queued when the bot is busy. The bot's own permission mode and
/// settings run the request (the caller widens nothing). The bot's pane id on success.
pub async fn ask(host: &Arc<Host>, caller: &str, handle: &str, prompt: &str) -> Result<(String, String), AskRefusal> {
    let (pane, caller_request) = {
        let view = host.view();
        let pane = view.snapshot.bot_pane(handle).map(|p| p.id().to_string());
        (pane, view.snapshot.session(caller).and_then(|p| latest_request(p.logs())).map(str::to_string))
    };
    let Some(pane) = pane else { return Err(AskRefusal::NoBot(shown(handle))) };
    if pane == caller {
        return Err(AskRefusal::ItSelf);
    }
    // MC-61: every agent pane is a bot (MC-59), so "a bot cannot ask a bot" means a pane working on what a bot asked it.
    if answering_a_bot(&lock(&ASKED), caller, caller_request.as_deref()) {
        return Err(AskRefusal::FromBot);
    }
    // MC-64: a bot asleep or past its own daily budget takes no request.
    if let Some(words) = crate::automation::bot_unavailable(host, handle) {
        return Err(AskRefusal::Failed(words));
    }
    let (_, spent, budget) = crate::usage_report::budget_reading(host);
    if let Some(budget) = budget.filter(|b| spent >= *b) {
        return Err(AskRefusal::OverBudget(
            host.locale.get_with("bot.ask.overBudget", &[("spent", &format!("${spent:.2}")), ("budget", &format!("${budget:.2}"))]),
        ));
    }
    let receipt = crate::phone::control_send(host, &pane, prompt).await.map_err(|(_, words)| AskRefusal::Failed(words))?;
    lock(&ASKED).insert(pane.clone(), prompt.trim().to_string());
    Ok((pane, receipt))
}

/// MC-61: what a bot was last asked by another agent, by its pane id: while that is the pane's latest request, the pane works for
/// a bot and may not ask one (one level deep, so bots never call each other in a loop).
static ASKED: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<String, String>>> = std::sync::LazyLock::new(Default::default);

fn lock<T>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The words of the pane's latest request (its last `user` entry), trimmed.
fn latest_request(logs: &[mighty_core::model::LogEntry]) -> Option<&str> {
    logs.iter().rev().find(|e| e.kind() == "user").map(|e| e.text().trim())
}

/// Whether `caller` is working on what a bot asked it: its latest request is the one [`ask`] sent it.
fn answering_a_bot(asked: &std::collections::HashMap<String, String>, caller: &str, latest: Option<&str>) -> bool {
    asked.get(caller).is_some_and(|sent| latest == Some(sent.as_str()))
}

/// The bot pane's state as the control API names it (`idle`, `running`, `blocked`, `done`, `failed`, `stopped`); None when it is gone.
fn state(host: &Host, pane: &str) -> Option<&'static str> {
    let view = host.view();
    let session = view.snapshot.session(pane)?.clone();
    Some(crate::control::pane_state(host, &view, &session).0.name())
}

/// How long `bot_answer {wait: true}` waits for a working bot (under the terminal tools' 30-second deadline).
const ANSWER_WAIT: std::time::Duration = std::time::Duration::from_secs(20);

/// Where the bot in pane `pane` is and its last answer (redacted, capped): `running` while it works, `blocked` while it waits for
/// the person, else how its last request ended, with the answer to it.
pub async fn answer(host: &Arc<Host>, pane: &str) -> Value {
    let Some(status) = state(host, pane) else { return json!({ "status": "gone" }) };
    let words = crate::git_workbench::redact_secret_text(&last_answer(host, pane).await);
    let cut = words.chars().count() > ANSWER_LIMIT;
    let words: String = words.chars().take(ANSWER_LIMIT).collect();
    json!({ "status": status, "answer": words, "cut": cut })
}

/// What the pane's agent said after the last request (its answer and the app's error lines, not the tools' output), loading the body
/// when it is on disk.
async fn last_answer(host: &Arc<Host>, pane: &str) -> String {
    let loader = host.clone();
    let id = pane.to_string();
    let _ = tauri::async_runtime::spawn_blocking(move || crate::bodies::ensure(&loader, &[id.as_str()])).await;
    let view = host.view();
    let Some(session) = view.snapshot.session(pane) else { return String::new() };
    answer_of(session.logs())
}

fn answer_of(logs: &[mighty_core::model::LogEntry]) -> String {
    let from = logs.iter().rposition(|e| e.kind() == "user").map_or(0, |at| at + 1);
    logs[from..]
        .iter()
        .filter(|e| matches!(e.kind(), "assistant" | "error"))
        .map(|e| e.text().trim())
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

// ── MC-59, MC-65: a bot from a spec (MC-71: the page's new-bot wizard, bot_wizard.rs, makes one) ─────────────────────────────

/// A workspace of this computer a spec names: by id, else by name (case aside).
fn workspace_named(host: &Host, named: &str) -> Option<String> {
    let view = host.view();
    let local = || view.snapshot.workspaces().iter().filter(|w| w.remote.is_none() && !w.is_bot_home());
    local().find(|w| w.id() == named).or_else(|| local().find(|w| w.name().eq_ignore_ascii_case(named.trim()))).map(|w| w.id().to_string())
}

/// A folder as a spec writes it: `~` for the home folder.
fn folder_path(text: &str) -> std::path::PathBuf {
    match text.trim().strip_prefix("~/") {
        Some(rest) => std::env::home_dir().unwrap_or_default().join(rest),
        None => std::path::PathBuf::from(text.trim()),
    }
}

/// A friendly name in the app's language, not taken.
pub(crate) fn friendly_name(host: &Host) -> String {
    let view = host.view();
    let taken: std::collections::HashSet<String> =
        view.snapshot.bots().filter_map(|p| p.bot.as_ref()).map(|b| PaneBot::key(&b.handle)).collect();
    let language = crate::methods::chosen_language(view.snapshot.language_preference());
    mighty_core::bot_names::pick(&uuid::Uuid::new_v4().simple().to_string(), language, &taken)
}

/// Makes the bot a spec describes: its pane (in its workspace, or an independent one in its folder), its name, instructions, budget
/// and permission, then its schedules and triggers sent to it. Its name.
pub async fn create_from_spec(host: &Arc<Host>, spec: &BotSpec) -> Result<String, String> {
    spec.validate(&mighty_core::wire::PROVIDERS, mighty_core::normalize::permission_modes)?;
    let handle = match spec.handle() {
        Some(handle) => handle,
        None => friendly_name(host),
    };
    if host.view().snapshot.bot_pane(&handle).is_some() {
        return Err(host.locale.get_with("bot.error.taken", &[("handle", &handle)]));
    }
    let bot = PaneBot {
        handle: handle.clone(),
        instructions: spec.instructions.clone().map(|t| t.trim().to_string()).filter(|t| !t.is_empty()),
        auto: Some(spec.handle().is_none()),
        asleep: None,
        budget_per_day: spec.budget_per_day,
    };
    let agent = spec.agent().to_string();
    let model = spec.model.clone().filter(|m| !m.trim().is_empty());
    let (pane, workspace) = match &spec.workspace {
        Some(named) => {
            let workspace =
                workspace_named(host, named).ok_or_else(|| host.locale.get_with("bot.error.noWorkspace", &[("name", named)]))?;
            let pane = crate::phone::automation_create(host.clone(), workspace.clone(), agent.clone(), model, Some(handle.clone()), false)
                .await
                .map_err(|(_, words)| words)?;
            (pane, workspace)
        }
        None => {
            let folder = spec.folder.as_deref().map(folder_path).unwrap_or_else(|| host.data_dir.join("bots").join(&handle));
            let pane = create_independent(host, bot.clone(), &agent, model, &folder).await?;
            (pane, home(host)?)
        }
    };
    let permission = spec.permission.clone();
    let _ = crate::frame::act(host, async {
        let mut view = host.view();
        crate::composer::change(&mut view, &pane, |p| {
            let mut settings = p.settings.clone().unwrap_or_default();
            if let Some(permission) = &permission {
                settings.permission_mode = permission.clone();
            }
            RunSession { bot: Some(bot.clone()), settings: Some(settings), ..p.clone() }
        })
    })
    .await;
    put_jobs(host, &workspace, spec, &handle)?;
    host.log.line(&format!("bot: @{handle} made from a spec"));
    Ok(handle)
}

/// The spec's schedules and triggers put in `workspace`'s jobs, sent to `@handle`.
fn put_jobs(host: &Arc<Host>, workspace: &str, spec: &BotSpec, handle: &str) -> Result<(), String> {
    let (schedules, triggers) = spec.jobs(handle);
    for schedule in schedules {
        crate::automation::put_schedule(host, workspace, None, schedule)?;
    }
    for trigger in triggers {
        crate::automation::put_trigger(host, workspace, None, trigger)?;
    }
    Ok(())
}

/// Changes the bot the spec names: its instructions, budget and permission when given, and its own (the app's) schedules and
/// triggers replaced by the spec's.
pub async fn update_from_spec(host: &Arc<Host>, spec: &BotSpec) -> Result<String, String> {
    spec.validate(&mighty_core::wire::PROVIDERS, mighty_core::normalize::permission_modes)?;
    let handle = spec.handle().ok_or_else(|| host.locale.get("bot.error.updateNeedsName"))?;
    let (pane, workspace) = {
        let view = host.view();
        let pane = view.snapshot.bot_pane(&handle).ok_or_else(|| host.locale.get_with("bot.ask.noBot", &[("handle", &handle)]))?;
        (pane.id().to_string(), pane.workspace_id().to_string())
    };
    let (instructions, budget, permission) = (spec.instructions.clone(), spec.budget_per_day, spec.permission.clone());
    let _ = crate::frame::act(host, async {
        let mut view = host.view();
        crate::composer::change(&mut view, &pane, |p| {
            let mut bot = p.bot.clone().unwrap_or_default();
            if let Some(words) = &instructions {
                bot.instructions = Some(words.trim().to_string()).filter(|t| !t.is_empty());
            }
            if budget.is_some() {
                bot.budget_per_day = budget;
            }
            let mut settings = p.settings.clone().unwrap_or_default();
            if let Some(permission) = &permission {
                settings.permission_mode = permission.clone();
            }
            RunSession { bot: Some(bot), settings: Some(settings), ..p.clone() }
        })
    })
    .await;
    if !spec.schedules.is_empty() || !spec.triggers.is_empty() {
        for job in crate::automation::jobs_of_bot(&handle).iter().filter(|j| j.origin == crate::automation::Origin::App) {
            let trigger = matches!(job.item, crate::automation::Item::Trigger(_));
            crate::automation::remove_own(host, &job.workspace, trigger, job.name());
        }
        put_jobs(host, &workspace, spec, &handle)?;
    }
    Ok(handle)
}

/// The approval sheet's lines: who asks, the bot, its agent, permission, place, budget, instructions' first line and its jobs.
fn summary(host: &Host, spec: &BotSpec, origin: &str) -> Vec<String> {
    let l = host.locale;
    let mut lines = vec![l.get_with("bot.approve.origin", &[("origin", origin)])];
    let name = spec.handle().map(|h| format!("@{h}")).unwrap_or_else(|| l.get("bot.approve.friendlyName"));
    let model = spec.model.as_deref().map(|m| format!(" · {m}")).unwrap_or_default();
    lines.push(format!("{name} · {}{model}", mighty_run::providers::name(spec.agent())));
    if let Some(permission) = &spec.permission {
        lines.push(format!(
            "{}: {}",
            l.get("bot.dialog.permission"),
            mighty_composer::runtime::permission_label(spec.agent(), permission, &l)
        ));
    }
    lines.push(match (&spec.workspace, &spec.folder) {
        (Some(workspace), _) => l.get_with("bot.approve.workspace", &[("name", workspace)]),
        (None, Some(folder)) => l.get_with("bot.approve.folder", &[("folder", folder)]),
        (None, None) => l.get("bot.approve.ownFolder"),
    });
    if let Some(budget) = spec.budget_per_day {
        lines.push(format!("{}: ${budget:.2}", l.get("bot.dialog.budget")));
    }
    if let Some(first) = spec.instructions.as_deref().map(|t| first_line(t, 160)).filter(|t| !t.is_empty()) {
        lines.push(format!("{}: {first}", l.get("bot.dialog.instructions")));
    }
    let (schedules, triggers) = spec.jobs(&spec.handle().unwrap_or_else(|| "bot".into()));
    for s in &schedules {
        lines.push(format!("• {} · {} · {}", s.name, crate::automation::kind_word(&l, "schedule"), s.when()));
    }
    for t in &triggers {
        lines.push(format!(
            "• {} · {} · {}",
            t.name,
            crate::automation::kind_word(&l, t.on.kind()),
            crate::automation::trigger_spec_words(&l, t)
        ));
    }
    lines
}

/// MC-65: a bot made or changed from a spec someone else gave (another agent's `create_bot` / `update_bot`, `mighty bot new`): the
/// spec checked, then the approval sheet shown, and made only if the person allows it. Returns once the sheet is up (or why not).
pub fn propose(host: &Arc<Host>, text: &str, update: bool, origin: String) -> Result<(), String> {
    let spec = BotSpec::parse(text)?;
    spec.validate(&mighty_core::wire::PROVIDERS, mighty_core::normalize::permission_modes)?;
    match (update, spec.handle()) {
        (true, None) => return Err(host.locale.get("bot.error.updateNeedsName")),
        (true, Some(handle)) if host.view().snapshot.bot_pane(&handle).is_none() => {
            return Err(host.locale.get_with("bot.ask.noBot", &[("handle", &handle)]));
        }
        (false, Some(handle)) if host.view().snapshot.bot_pane(&handle).is_some() => {
            return Err(host.locale.get_with("bot.error.taken", &[("handle", &handle)]));
        }
        _ => {}
    }
    if let Some(named) = &spec.workspace
        && workspace_named(host, named).is_none()
    {
        return Err(host.locale.get_with("bot.error.noWorkspace", &[("name", named)]));
    }
    let locale = host.locale;
    let lines = summary(host, &spec, &origin);
    let title = match (update, spec.handle()) {
        (true, Some(handle)) => locale.get_with("bot.approve.updateTitle", &[("handle", &handle)]),
        _ => locale.get("bot.approve.title"),
    };
    let mut sheet = DialogSpec::new(title, move |_| {
        let parts: Vec<El> = std::iter::once(locale.get("bot.approve.message"))
            .chain(lines.iter().cloned())
            .map(|words| {
                let mut block = El::text_block(words);
                if let Some(text) = block.text_mut() {
                    text.wrapping = Wrapping::Wrap;
                }
                block
            })
            .collect();
        Content::El(Box::new(El::stack(false, mighty_core::design_tokens::metrics::SPACING_SM, parts)))
    })
    .styled(None, None);
    sheet.primary = Some(DialogButton::new(locale.get(if update { "bot.approve.update" } else { "bot.approve.create" })));
    sheet.close = Some(locale.get("bot.dialog.cancel"));
    let host = host.clone();
    tauri::async_runtime::spawn(async move {
        if crate::native::show_dialog(&host, sheet).await != DialogResult::Primary {
            host.log.line("bot: a proposed bot was not allowed");
            return;
        }
        let made = if update { update_from_spec(&host, &spec).await } else { create_from_spec(&host, &spec).await };
        if let Err(error) = made {
            host.log.line(&format!("bot: a proposed bot was not made: {error}"));
            let mut view = host.view();
            view.error = Some(error);
            host.schedule();
        }
    });
    Ok(())
}

/// Makes the independent bot: its folder (when missing), the bots' home, an agent pane there working in `folder`, named and
/// instructed, then selected.
pub async fn create_independent(
    host: &Arc<Host>,
    bot: PaneBot,
    provider: &str,
    model: Option<String>,
    folder: &std::path::Path,
) -> Result<String, String> {
    std::fs::create_dir_all(folder).map_err(|error| error.to_string())?;
    let folder = std::path::absolute(folder).map_err(|error| error.to_string())?.to_string_lossy().to_string();
    let home = home(host)?;
    let pane = crate::phone::automation_create(host.clone(), home, provider.to_string(), model, Some(bot.handle.clone()), false)
        .await
        .map_err(|(_, words)| words)?;
    let id = pane.clone();
    let _ = crate::frame::act(host, async {
        let mut view = host.view();
        crate::composer::change(&mut view, &id, |p| RunSession {
            working_folder: Some(folder.clone()),
            bot: Some(bot.clone()),
            ..p.clone()
        })?;
        crate::frame::activate_pane(&mut view, &id)
    })
    .await;
    Ok(pane)
}

/// The bots' home (`<data>/bots`, a workspace marked `botHome`), listed (approved) and made when there is none yet; its id.
fn home(host: &Arc<Host>) -> Result<String, String> {
    if let Some(found) = host.view().snapshot.workspaces().iter().find(|w| w.is_bot_home()) {
        return Ok(found.id().to_string());
    }
    let dir = host.data_dir.join("bots");
    std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    let mut workspace = host.approve_local(&dir).map_err(|error| error.to_string())?;
    workspace.bot_home = Some(true);
    workspace.name = Some(host.locale.get("bot.home.name"));
    let id = workspace.id().to_string();
    let mut view = host.view();
    view.update(|state| match state.workspaces_mut().iter_mut().find(|w| w.id() == id) {
        Some(listed) => *listed = workspace.clone(),
        None => state.workspaces_mut().push(workspace.clone()),
    })?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use mighty_core::model::LogEntry;

    use super::*;

    fn entry(kind: &str, text: &str) -> LogEntry {
        LogEntry::new(mighty_core::wire::new_id(), kind, text.to_string(), String::new(), None)
    }

    #[test]
    fn the_answer_is_what_the_agent_said_after_the_last_request() {
        let logs = vec![
            entry("user", "first"),
            entry("assistant", "old answer"),
            entry("user", "second"),
            entry("output", "$ ls"),
            entry("assistant", "part one"),
            entry("assistant", "  "),
            entry("error", "it stopped"),
        ];
        assert_eq!(answer_of(&logs), "part one\n\nit stopped");
        assert_eq!(answer_of(&[]), "");
    }

    /// MC-61: a pane may ask a bot unless it is working on what a bot asked it (its latest request is the one sent to it).
    #[test]
    fn only_a_pane_answering_a_bot_may_not_ask_one() {
        let asked: std::collections::HashMap<String, String> = [("b".to_string(), "check the build".to_string())].into();
        assert!(answering_a_bot(&asked, "b", Some("check the build")));
        assert!(!answering_a_bot(&asked, "b", Some("a request of the person")), "the person asked it since");
        assert!(!answering_a_bot(&asked, "a", Some("check the build")), "never asked by a bot");
        assert!(!answering_a_bot(&asked, "b", None));
        let logs = vec![entry("user", " first "), entry("assistant", "ok"), entry("user", "  check the build \n"), entry("assistant", "…")];
        assert_eq!(latest_request(&logs), Some("check the build"));
    }

    /// MC-64: a daily budget as typed in the bot sheet.
    #[test]
    fn a_daily_budget_is_empty_or_a_positive_number_of_dollars() {
        assert_eq!(budget_of(""), Ok(None));
        assert_eq!(budget_of(" $2.50 "), Ok(Some(2.5)));
        assert_eq!(budget_of("10"), Ok(Some(10.0)));
        for bad in ["0", "-1", "free", "NaN", "inf"] {
            assert!(budget_of(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_about_line_is_the_first_line_with_words_cut_to_its_limit() {
        assert_eq!(first_line("\n  Review diffs.\nMore", 160), "Review diffs.");
        assert_eq!(first_line("abcdef", 3), "abc");
    }
}
