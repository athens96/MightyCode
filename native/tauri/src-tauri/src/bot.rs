//! MC-58: bots — an agent pane with a name (`@handle`) and standing instructions (`RunSession.bot`), called by that name from the
//! command line (`mighty agent send @reviewer …`), a schedule's heartbeat (`@reviewer` as the pane) and other agents (mighty-terminal's
//! `list_bots` and `ask_bot`). A bot stays an ordinary pane: its approvals, queue, phone and mirror work as they do for any pane.
//!
//! - [`open`]: the pane menu's 봇으로 만들기… / 봇 설정… sheet (handle and instructions; 봇 해제 when it is one).
//! - [`ask`]: one request to a bot by its handle, for an agent (the caller is never a bot, the bot is never the caller, the day's
//!   budget is respected); it queues when the bot is busy, as a typed request would.
//! - [`answer`]: where a bot is now and its last answer (redacted and capped), for the caller that waits.

use std::sync::Arc;

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
    use mighty_delegation::agent_io::tools::{ASK_BOT, BOT_ANSWER, LIST_BOTS};
    let args: Value = serde_json::from_str(value).map_err(|_| "Unreadable arguments.".to_string())?;
    let bot = args.get("bot").and_then(Value::as_str).unwrap_or_default().to_string();
    match tool {
        LIST_BOTS => Ok(json!({ "bots": list(host, caller) })),
        ASK_BOT => {
            let prompt = args.get("prompt").and_then(Value::as_str).unwrap_or_default();
            let (pane, receipt) = ask(host, caller, &bot, prompt).await.map_err(|refusal| refusal.words(host))?;
            let title = host.view().snapshot.session(&pane).map(|p| p.title().to_string());
            Ok(json!({ "bot": shown(&bot), "pane": title, "sent": receipt, "next": "bot_answer" }))
        }
        BOT_ANSWER => {
            let pane = host.view().snapshot.bot_pane(&bot).map(|p| p.id().to_string());
            let Some(pane) = pane else { return Err(AskRefusal::NoBot(shown(&bot)).words(host)) };
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
    // MC-59: the sidebar 봇 group's + (an independent bot).
    methods.add("botNew", |host: Arc<Host>, _args| async move {
        open_new(&host);
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

/// The pane menu's bot sheet: the handle and the instructions, saved on the pane (봇 해제 takes the bot off it). An agent pane of
/// this computer only, never a delegated child.
pub fn open(host: &Arc<Host>, id: String) {
    let Some(pane) = host.view().snapshot.session(&id).cloned() else { return };
    if !can_be_bot(host, &pane) {
        return;
    }
    let locale = host.locale;
    let current = pane.bot.clone();
    let handle = Local::new(current.as_ref().map(|b| b.handle.clone()).unwrap_or_default());
    let instructions = Local::new(current.as_ref().and_then(|b| b.instructions.clone()).unwrap_or_default());
    let note = Local::new(String::new());
    let (handle_field, instructions_field, note_text) = (handle.clone(), instructions.clone(), note.clone());
    let title = locale.get("bot.dialog.titleEdit");
    let mut spec = DialogSpec::new(title, move |_| {
        let into_handle = handle_field.clone();
        let into_instructions = instructions_field.clone();
        let mut intro = El::text_block(locale.get("bot.dialog.message"));
        if let Some(text) = intro.text_mut() {
            text.wrapping = Wrapping::Wrap;
        }
        let name = El::text_box(handle_field.get())
            .header(locale.get("bot.dialog.handle"))
            .placeholder(locale.get("bot.dialog.handleHint"))
            .on_text(move |words| into_handle.set(words));
        let words = El::text_box(instructions_field.get())
            .multiline()
            .header(locale.get("bot.dialog.instructions"))
            .placeholder(locale.get("bot.dialog.instructionsHint"))
            .on_text(move |words| into_instructions.set(words));
        let mut validation = El::text_block(note_text.get());
        if let Some(text) = validation.text_mut() {
            text.wrapping = Wrapping::Wrap;
        }
        Content::El(Box::new(El::stack(false, mighty_core::design_tokens::metrics::SPACING_SM, vec![intro, name, words, validation])))
    })
    .styled(None, None);
    spec.primary = Some(DialogButton::new(locale.get("bot.dialog.save")));
    spec.close = Some(locale.get("bot.dialog.cancel"));
    let (checked_handle, checked_instructions, checker, pane_id) = (handle.clone(), instructions.clone(), host.clone(), id.clone());
    spec.accept = Some(Box::new(move |answer| {
        if answer != DialogResult::Primary {
            return true;
        }
        match refusal(&checker, &pane_id, checked_handle.get().trim(), &checked_instructions.get()) {
            Some(words) => {
                note.set(words);
                false
            }
            None => true,
        }
    }));
    let host = host.clone();
    tauri::async_runtime::spawn(async move {
        let bot = match crate::native::show_dialog(&host, spec).await {
            DialogResult::Primary => {
                let typed = handle.get();
                let Some(handle) = PaneBot::handle_of(&typed) else { return };
                if refusal(&host, &id, &handle, &instructions.get()).is_some() {
                    return;
                }
                let words = instructions.get().trim().to_string();
                Some(PaneBot { handle, instructions: (!words.is_empty()).then_some(words), auto: Some(false) })
            }
            DialogResult::Secondary | DialogResult::None => return,
        };
        let _ = crate::frame::act(&host, async {
            let mut view = host.view();
            // MC-61: a name the user kept stays the app's (it may be named again only if it never was marked); any other is theirs.
            let bot = |pane: &RunSession| {
                bot.clone().map(|chosen| {
                    let kept = pane.bot.as_ref().filter(|old| PaneBot::key(&old.handle) == PaneBot::key(&chosen.handle));
                    PaneBot { auto: if kept.is_some_and(|old| old.auto == Some(true)) { Some(true) } else { Some(false) }, ..chosen }
                })
            };
            crate::composer::change(&mut view, &id, |pane| RunSession { bot: bot(pane), ..pane.clone() })
        })
        .await;
    });
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
    let view = host.view();
    view.snapshot
        .bots()
        .filter(|pane| pane.id() != caller)
        .filter_map(|pane| {
            let bot = pane.bot.as_ref()?;
            let workspace = view.snapshot.workspace(pane.workspace_id()).map(|w| w.name().to_string());
            Some(json!({
                "bot": format!("@{}", bot.handle),
                "title": pane.title(),
                "workspace": workspace,
                "status": pane.status(),
                "about": bot.instructions.as_deref().map(|t| first_line(t, 160)),
            }))
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
    let (pane, caller_is_bot) = {
        let view = host.view();
        let pane = view.snapshot.bot_pane(handle).map(|p| p.id().to_string());
        (pane, view.snapshot.session(caller).is_some_and(|p| p.bot.is_some()))
    };
    let Some(pane) = pane else { return Err(AskRefusal::NoBot(shown(handle))) };
    if pane == caller {
        return Err(AskRefusal::ItSelf);
    }
    if caller_is_bot {
        return Err(AskRefusal::FromBot);
    }
    let (_, spent, budget) = crate::usage_report::budget_reading(host);
    if let Some(budget) = budget.filter(|b| spent >= *b) {
        return Err(AskRefusal::OverBudget(
            host.locale.get_with("bot.ask.overBudget", &[("spent", &format!("${spent:.2}")), ("budget", &format!("${budget:.2}"))]),
        ));
    }
    let receipt = crate::phone::control_send(host, &pane, prompt).await.map_err(|(_, words)| AskRefusal::Failed(words))?;
    Ok((pane, receipt))
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

// ── MC-59: independent bots ──────────────────────────────────────────────────────────────────────────────────────────────

/// The sidebar 봇 group's +: a bot of no workspace. Its name, instructions and agent are asked in a sheet; 만들기 makes its folder
/// in the app's data (`bots/<name>`), 폴더 골라 만들기… lets the person pick one. The bot is a pane of the bots' home (a workspace
/// the sidebar never lists as one) working in that folder.
pub fn open_new(host: &Arc<Host>) {
    let locale = host.locale;
    let providers: Vec<&'static str> = mighty_core::wire::PROVIDERS.to_vec();
    let handle = Local::new(String::new());
    let instructions = Local::new(String::new());
    let agent = Local::new(0usize);
    let note = Local::new(String::new());
    let (handle_field, instructions_field, agent_field, note_text) = (handle.clone(), instructions.clone(), agent.clone(), note.clone());
    let names: Vec<String> = providers.iter().map(|p| mighty_run::providers::name(p).to_string()).collect();
    let mut spec = DialogSpec::new(locale.get("bot.new.title"), move |_| {
        let into_handle = handle_field.clone();
        let into_instructions = instructions_field.clone();
        let into_agent = agent_field.clone();
        let mut intro = El::text_block(locale.get("bot.new.message"));
        if let Some(text) = intro.text_mut() {
            text.wrapping = Wrapping::Wrap;
        }
        let name = El::text_box(handle_field.get())
            .header(locale.get("bot.dialog.handle"))
            .placeholder(locale.get("bot.dialog.handleHint"))
            .on_text(move |words| into_handle.set(words));
        let items: Vec<Content> = names.iter().map(|n| n.clone().into()).collect();
        let picker = El::combo_box(agent_field.get() as i32, items)
            .header(locale.get("bot.new.agent"))
            .on_select(move |chosen| into_agent.set(usize::try_from(chosen).unwrap_or(0)));
        let words = El::text_box(instructions_field.get())
            .multiline()
            .header(locale.get("bot.dialog.instructions"))
            .placeholder(locale.get("bot.dialog.instructionsHint"))
            .on_text(move |words| into_instructions.set(words));
        let mut validation = El::text_block(note_text.get());
        if let Some(text) = validation.text_mut() {
            text.wrapping = Wrapping::Wrap;
        }
        Content::El(Box::new(El::stack(
            false,
            mighty_core::design_tokens::metrics::SPACING_SM,
            vec![intro, name, picker, words, validation],
        )))
    })
    .styled(None, None);
    spec.primary = Some(DialogButton::new(locale.get("bot.new.create")));
    spec.secondary = Some(DialogButton::new(locale.get("bot.new.pickFolder")));
    spec.close = Some(locale.get("bot.dialog.cancel"));
    let (checked_handle, checked_instructions, checker) = (handle.clone(), instructions.clone(), host.clone());
    spec.accept = Some(Box::new(move |answer| {
        if answer == DialogResult::None {
            return true;
        }
        match refusal(&checker, "", checked_handle.get().trim(), &checked_instructions.get()) {
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
        let folder = if answer == DialogResult::Secondary {
            match crate::frame::pick_folder_path(&host).await {
                Some(folder) => folder,
                None => return,
            }
        } else {
            host.data_dir.join("bots").join(&name)
        };
        let words = instructions.get().trim().to_string();
        let provider = providers.get(agent.get()).copied().unwrap_or("claude").to_string();
        let bot = PaneBot { handle: name, instructions: (!words.is_empty()).then_some(words), auto: Some(false) };
        if let Err(error) = create_independent(&host, bot, &provider, &folder).await {
            host.log.line(&format!("bot: an independent bot was not made: {error}"));
            let mut view = host.view();
            view.error = Some(error);
            host.schedule();
        }
    });
}

/// Makes the independent bot: its folder (when missing), the bots' home, an agent pane there working in `folder`, named and
/// instructed, then selected.
pub async fn create_independent(host: &Arc<Host>, bot: PaneBot, provider: &str, folder: &std::path::Path) -> Result<String, String> {
    std::fs::create_dir_all(folder).map_err(|error| error.to_string())?;
    let folder = std::path::absolute(folder).map_err(|error| error.to_string())?.to_string_lossy().to_string();
    let home = home(host)?;
    let pane = crate::phone::automation_create(host.clone(), home, provider.to_string(), None, Some(bot.handle.clone()), false)
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

    #[test]
    fn the_about_line_is_the_first_line_with_words_cut_to_its_limit() {
        assert_eq!(first_line("\n  Review diffs.\nMore", 160), "Review diffs.");
        assert_eq!(first_line("abcdef", 3), "abc");
    }
}
