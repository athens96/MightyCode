//! 대화 검색 (Swift ConversationSearchSheet.swift, WorkspaceView.swift's header button, MightyCodeApp.swift's ⇧⌘F item): the
//! workspace's past sessions whose requests and answers hold every word typed, over `mighty_history::conversation_search`. A hit opens
//! as a new pane that continues the session (the resume list's path: `resumable::apply`), or is saved as Markdown.
//!
//! - `openConversationSearch{workspaceId?}` (the menu's and the shortcut's action, the sidebar's and the header's entries): the sheet
//!   the page draws (`{type:'conversationSearch'}`) for a workspace on this computer, the folder's sessions read once in the
//!   background as it opens (the resume list's query: open panes' sessions and ones older than 60 days left out). Its answer
//!   `{resume: id}` continues that session in a new pane of the workspace.
//! - `conversationSearch{query}`: the sessions' count and, for a query with words, the hits (newest first, at most 50), searched off
//!   the page's thread; a newer query stops the one before it, whose answer is null.
//! - `conversationExport{id}`: the hit's session as Markdown through the OS's save dialog.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use mighty_history::conversation_search::{self, Hit};
use mighty_history::{ResumableSession, ResumableSessionQuery, resumable};
use mighty_platform::SaveRequest;
use serde_json::{Value, json};
use tauri::Manager;
use tokio::sync::OnceCell;

use crate::frame::{Sheet, act, select_workspace};
use crate::host::{Host, Registry, lock};
use crate::methods::text;

/// The sheet open now: its workspace's query, its sessions (read once) and the search running.
struct Open {
    query: ResumableSessionQuery,
    sessions: Arc<OnceCell<Arc<Vec<ResumableSession>>>>,
    running: Option<Arc<AtomicBool>>,
}

#[derive(Default)]
struct Search {
    open: Mutex<Option<Open>>,
}

fn of(host: &Host) -> Arc<Search> {
    if let Some(search) = host.app.try_state::<Arc<Search>>() {
        return search.inner().clone();
    }
    host.app.manage(Arc::new(Search::default()));
    host.app.state::<Arc<Search>>().inner().clone()
}

/// The open sheet's sessions, read off the page's thread the first time they are asked for; none when no sheet is open.
async fn sessions(host: &Host) -> Option<Arc<Vec<ResumableSession>>> {
    let search = of(host);
    let (cell, query) = {
        let open = lock(&search.open);
        let open = open.as_ref()?;
        (open.sessions.clone(), open.query.clone())
    };
    let sessions = cell
        .get_or_init(|| async move {
            let read = tauri::async_runtime::spawn_blocking(move || resumable::listing(&query, &mighty_history::Never)).await;
            let items = read.map(|listing| listing.items).unwrap_or_default();
            Arc::new(items.into_iter().filter(conversation_search::searchable).collect())
        })
        .await;
    Some(sessions.clone())
}

/// The sheet: shown for a workspace on this computer, and what its answer asks for done.
async fn open(host: &Arc<Host>, workspace_id: Option<String>) -> Result<(), String> {
    let locale = host.locale;
    let (workspace, query) = {
        let view = host.view();
        let id = workspace_id.or_else(|| view.snapshot.active_workspace_id.clone());
        let workspace =
            id.and_then(|id| view.snapshot.workspace(&id).cloned()).ok_or_else(|| locale.get("window.error.addWorkspaceFirst"))?;
        let query = crate::history::query(host, &view, &workspace);
        (workspace, query)
    };
    // The sessions are this computer's CLI records; a remote workspace's are on its host.
    let query = query.ok_or_else(|| locale.get("conversationSearch.remote"))?;
    let search = of(host);
    let answer = {
        let Some(_sheet) = Sheet::begin(host) else { return Ok(()) };
        *lock(&search.open) = Some(Open { query, sessions: Arc::new(OnceCell::new()), running: None });
        // The records are read as the sheet opens, not at the first word.
        let reading = host.clone();
        tauri::async_runtime::spawn(async move {
            sessions(&reading).await;
        });
        let (_, answer) = crate::dialogs::of(host).open(json!({
            "type": "conversationSearch", "title": locale.get("conversationSearch.title"), "path": workspace.path(),
        }));
        let answer = answer.await.unwrap_or(Value::Null);
        let picked = match answer.get("resume").and_then(Value::as_str) {
            Some(id) => sessions(host).await.and_then(|all| all.iter().find(|s| s.id() == id).cloned()),
            None => None,
        };
        let open = lock(&search.open).take();
        if let Some(running) = open.and_then(|o| o.running) {
            running.store(true, Ordering::SeqCst);
        }
        picked
    };
    let Some(session) = answer else { return Ok(()) };
    resume(host, workspace.id(), session)
}

/// The resume list's pick (history.rs `pick`, frame.rs `pick_resumed_session`): never two panes on one session, the id remembered as
/// the app's own, then a new pane of the workspace that continues it.
fn resume(host: &Arc<Host>, workspace_id: &str, session: ResumableSession) -> Result<(), String> {
    {
        let mut view = host.view();
        if resumable::in_use(view.snapshot.sessions().iter().map(|s| s.as_ref())).contains(&session.session_id.to_lowercase()) {
            return Err(host.locale.get("resume.error.inUse"));
        }
        if view.snapshot.active_workspace_id.as_deref() != Some(workspace_id) {
            select_workspace(&mut view, workspace_id)?;
        }
    }
    crate::history::remember_session(host, &session.session_id);
    crate::frame::add_pane_to(host, Some(workspace_id), "claude", &session.provider.clone(), None, |pane| {
        resumable::apply(&session, &pane)
    })?;
    Ok(())
}

/// A hit as the sheet's row.
fn row(host: &Host, hit: &Hit) -> Value {
    let session = &hit.session;
    let modified: std::time::SystemTime = session.modified.to_chrono().into();
    let date = crate::platform::short_date_time(modified)
        .unwrap_or_else(|| resumable::relative_time(session.modified, mighty_core::time::DateTimeOffset::now(), &host.locale));
    json!({
        "id": session.id(),
        "provider": session.provider,
        "agent": mighty_core::model_label::provider_label(&session.provider),
        "title": session.title.clone().unwrap_or_else(|| session.session_id.clone()),
        "date": date,
        "snippet": hit.snippet,
        "matches": host.locale.get_with("conversationSearch.matches", &[("count", &hit.matches.to_string())]),
    })
}

/// `conversationSearch{query}`: `{count, hits}` (no hits for a blank query); null when no sheet is open or a newer query took over.
async fn search(host: Arc<Host>, query: String) -> Result<Value, String> {
    let Some(sessions) = sessions(&host).await else { return Ok(Value::Null) };
    let count = sessions.len();
    if conversation_search::terms(&query).is_empty() {
        return Ok(json!({"count": count, "hits": []}));
    }
    let cancel = Arc::new(AtomicBool::new(false));
    let state = of(&host);
    {
        let mut open = lock(&state.open);
        let Some(open) = open.as_mut() else { return Ok(Value::Null) };
        if let Some(previous) = open.running.replace(cancel.clone()) {
            previous.store(true, Ordering::SeqCst);
        }
    }
    let stop = cancel.clone();
    let hits = tauri::async_runtime::spawn_blocking(move || {
        conversation_search::search(&sessions, &query, conversation_search::LIMIT, stop.as_ref())
    })
    .await
    .map_err(|e| e.to_string())?;
    if cancel.load(Ordering::SeqCst) {
        return Ok(Value::Null);
    }
    let rows: Vec<Value> = hits.iter().map(|hit| row(&host, hit)).collect();
    Ok(json!({"count": count, "hits": rows}))
}

/// `conversationExport{id}`: the session as Markdown, saved where the save dialog says; `{saved}`.
async fn export(host: Arc<Host>, id: String) -> Result<Value, String> {
    let Some(sessions) = sessions(&host).await else { return Ok(Value::Null) };
    let Some(session) = sessions.iter().find(|s| s.id() == id).cloned() else { return Ok(Value::Null) };
    let locale = host.locale;
    let title = session.title.clone().unwrap_or_else(|| session.session_id.clone());
    let (user, agent) = (locale.get("conversationSearch.speaker.user"), mighty_core::model_label::provider_label(&session.provider));
    let (reading, heading) = (session.clone(), title.clone());
    let markdown = tauri::async_runtime::spawn_blocking(move || conversation_search::markdown_of(&reading, &heading, &user, agent))
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| locale.get_with("conversationSearch.exportFailed", &[("error", &session.path.display().to_string())]))?;
    let Some(files) = host.app.try_state::<Arc<crate::files::Files>>().map(|f| f.inner().clone()) else { return Ok(Value::Null) };
    let request =
        SaveRequest { suggested_name: conversation_search::file_name(&title), filter_name: "Markdown".into(), extension: "md".into() };
    let Some(path) = files.save.pick(request).await.map_err(|e| e.to_string())? else { return Ok(json!({"saved": false})) };
    std::fs::write(&path, markdown)
        .map_err(|error| locale.get_with("conversationSearch.exportFailed", &[("error", &error.to_string())]))?;
    Ok(json!({"saved": true}))
}

pub fn register(methods: &mut Registry) {
    methods.add("openConversationSearch", |host: Arc<Host>, args| async move {
        if host.view().dialog_open {
            return Ok(Value::Null);
        }
        let workspace = text(&args, "workspaceId").map(str::to_string);
        act(&host, open(&host, workspace)).await
    });
    methods.add("conversationSearch", |host: Arc<Host>, args| async move {
        let query = text(&args, "query").unwrap_or("").to_string();
        search(host, query).await
    });
    methods.add("conversationExport", |host: Arc<Host>, args| async move {
        let Some(id) = text(&args, "id").map(str::to_string) else { return Ok(Value::Null) };
        export(host, id).await
    });
}
