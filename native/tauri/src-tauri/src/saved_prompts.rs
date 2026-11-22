//! Host management, placeholder expansion (`{branch}`, `{workspace}`, `{date}`, `{clipboard}`), and page/mobile RPCs for Saved
//! Prompts (`<data>/saved-prompts.json`, `MC-11`).

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use mighty_composer::slash::{SlashCommand, SlashCommandAction, SlashCommandOrigin};
use mighty_core::locale::Locale;
use mighty_core::saved_prompts::{self, SavedPrompt};
use serde_json::json;
use tauri::Manager as _;

use crate::host::{Host, Registry, lock};
use crate::methods::text;

#[derive(Default)]
struct PromptsBook {
    loaded: bool,
    items: Vec<SavedPrompt>,
}

#[derive(Default)]
pub struct PromptsState {
    book: Mutex<PromptsBook>,
    epoch: AtomicU64,
}

fn state(host: &Host) -> Arc<PromptsState> {
    if let Some(existing) = host.app.try_state::<Arc<PromptsState>>() {
        return existing.inner().clone();
    }
    let created = Arc::new(PromptsState::default());
    host.app.manage(created.clone());
    host.app.try_state::<Arc<PromptsState>>().map(|s| s.inner().clone()).unwrap_or(created)
}

fn with_items<T>(host: &Host, run: impl FnOnce(&mut Vec<SavedPrompt>) -> T) -> T {
    let st = state(host);
    let mut book = lock(&st.book);
    if !book.loaded {
        book.items = saved_prompts::load(&host.data_dir);
        book.loaded = true;
    }
    run(&mut book.items)
}

pub fn revision(host: &Host) -> u64 {
    state(host).epoch.load(Ordering::Acquire)
}

pub fn all(host: &Host) -> Vec<SavedPrompt> {
    with_items(host, |items| items.clone())
}

pub fn offered(host: &Host, workspace_id: Option<&str>) -> Vec<SavedPrompt> {
    with_items(host, |items| saved_prompts::offered(items, workspace_id))
}

pub fn by_id(host: &Host, prompt_id: &str) -> Option<SavedPrompt> {
    with_items(host, |items| items.iter().find(|p| p.id == prompt_id).cloned())
}

/// Slash command entries (`/prompt:<name>`) for the active workspace.
pub fn slash_commands(host: &Host, workspace_id: Option<&str>, locale: &Locale) -> Vec<SlashCommand> {
    let source = locale.get("slash.source.savedPrompt");
    offered(host, workspace_id)
        .into_iter()
        .map(|p| {
            let first_line = p.text.lines().next().unwrap_or("").trim();
            let desc: String = first_line.chars().take(80).collect();
            let invocation = format!("{}{}", saved_prompts::INVOCATION_PREFIX, p.name);
            let mut cmd = SlashCommand::new(
                &invocation,
                &desc,
                &source,
                if p.workspace_id.is_none() { SlashCommandOrigin::User } else { SlashCommandOrigin::Project },
            );
            cmd.action = Some(SlashCommandAction::InsertPrompt);
            cmd.action_arg = Some(p.id);
            cmd
        })
        .collect()
}

/// Resolves `{branch}`, `{workspace}`, `{date}`, and `{clipboard}` for a pane and expands `template`.
pub async fn expand_for_pane(
    host: &Arc<Host>,
    pane_id: Option<&str>,
    prompt_id: Option<&str>,
    raw_text: Option<&str>,
) -> Result<String, String> {
    let template = match (prompt_id.filter(|id| !id.is_empty()), raw_text) {
        (Some(id), _) => by_id(host, id).map(|p| p.text).ok_or_else(|| "Saved prompt not found.".to_string())?,
        (None, Some(t)) => t.to_string(),
        (None, None) => String::new(),
    };
    if template.is_empty() {
        return Ok(String::new());
    }

    let (ws_name, local_folder) = {
        let view = host.view();
        let pane = pane_id.and_then(|id| view.snapshot.session(id));
        let ws = pane
            .and_then(|p| view.snapshot.workspace(p.workspace_id()))
            .or_else(|| view.snapshot.active_workspace_id.as_deref().and_then(|id| view.snapshot.workspace(id)));
        let ws_name = ws.map(|w| w.name().to_string()).unwrap_or_default();
        let folder = pane
            .and_then(|p| p.working_folder.clone().filter(|f| !f.is_empty()))
            .or_else(|| ws.filter(|w| w.remote.is_none() && !w.path().is_empty()).map(|w| w.path().to_string()));
        (ws_name, folder)
    };

    let branch = if template.contains("{branch}")
        && let Some(folder) = local_folder
    {
        crate::host::read_git(host.processes.as_ref(), &folder)
            .await
            .map(|info| if info.branch == "(detached)" { String::new() } else { info.branch })
            .unwrap_or_default()
    } else {
        String::new()
    };

    let clipboard = if template.contains("{clipboard}") {
        crate::platform::clipboard().read_text().ok().flatten().unwrap_or_default()
    } else {
        String::new()
    };

    let date = chrono::Local::now().format("%Y-%m-%d").to_string();
    let values = BTreeMap::from([("branch", branch), ("workspace", ws_name), ("date", date), ("clipboard", clipboard)]);
    Ok(saved_prompts::expand(&template, &values))
}

fn persist_if_allowed(host: &Host, items: &[SavedPrompt]) -> Result<(), String> {
    state(host).epoch.fetch_add(1, Ordering::Release);
    host.shell_dirty();
    if host.writes() {
        saved_prompts::save(items, &host.data_dir).map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub fn register(methods: &mut Registry) {
    methods.add("savedPromptsList", |host: Arc<Host>, args| async move {
        let ws_id = text(&args, "workspaceId").map(str::to_string);
        let prompts = all(&host);
        let offered = offered(&host, ws_id.as_deref());
        Ok(json!({ "prompts": prompts, "offered": offered }))
    });

    methods.add("savedPromptsSave", |host: Arc<Host>, args| async move {
        let id = text(&args, "id").map(str::to_string);
        let name = text(&args, "name").unwrap_or("").to_string();
        let body = text(&args, "text").unwrap_or("").to_string();
        let ws_id = text(&args, "workspaceId").map(str::to_string);
        let (saved, snapshot) = with_items(&host, |items| {
            let item = saved_prompts::upsert(items, id.as_deref(), &name, &body, ws_id)?;
            Ok::<_, &'static str>((item, items.clone()))
        })
        .map_err(String::from)?;
        persist_if_allowed(&host, &snapshot)?;
        Ok(json!({ "prompt": saved }))
    });

    methods.add("savedPromptsDelete", |host: Arc<Host>, args| async move {
        let Some(target) = text(&args, "id").map(str::to_string) else {
            return Err("Missing prompt id.".into());
        };
        let (removed, snapshot) = with_items(&host, |items| {
            let before = items.len();
            items.retain(|p| p.id != target);
            (items.len() != before, items.clone())
        });
        if removed {
            persist_if_allowed(&host, &snapshot)?;
        }
        Ok(json!({ "ok": removed }))
    });

    methods.add("savedPromptsExpand", |host: Arc<Host>, args| async move {
        let pane_id = text(&args, "paneId").map(str::to_string);
        let prompt_id = text(&args, "promptId").map(str::to_string);
        let raw_text = text(&args, "text").map(str::to_string);
        let expanded = expand_for_pane(&host, pane_id.as_deref(), prompt_id.as_deref(), raw_text.as_deref()).await?;
        Ok(json!({ "text": expanded }))
    });
}
