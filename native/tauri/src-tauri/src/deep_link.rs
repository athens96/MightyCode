//! M-F: deep links on the Mac (`RunEvent::Opened`, the bundle's `CFBundleURLTypes`): `mightycode://open?path=<abs>&prompt=<≤64 KB>&
//! agent=<provider>`, parsed strictly (Swift `OSIntegration.swift:7-34` `DeepLink.parse`); both `mightycode` and the legacy
//! `mightyclaude` accepted (plan D11, where Swift dropped the legacy one at run time). The handler waits up to 10 s for the page, asks
//! before adding a new folder, and puts the prompt in the draft without sending it (`AppStore+OSIntegration.swift:9-43`
//! `handleDeepLink`); a cold start's address arrives after the window is ready. Nothing a link carries is ever sent: any web page can
//! open one. The log names the count of addresses only (a prompt is the user's text).
//!
//! **A deliberate deviation from Swift** (review of mac-f, item 1): a link's text is untrusted. It is trimmed and its runs of blank
//! lines collapsed; when the pane's draft already holds words, the person is asked first with the link's text shown; and every
//! confirmation a link raises has Cancel as its default (Swift's alert had Open as its default and appended without asking). A path
//! with a control or bidirectional formatting character is refused (it could show the person another folder than the one opened).

use std::sync::Arc;
use std::time::Duration;

use mighty_core::add_pane;
use serde_json::json;
use tauri::Manager;

use crate::host::Host;

pub const SCHEME: &str = "mightycode";
/// The scheme from before the rename: the `mighty` command an older version installed still opens it.
pub const LEGACY_SCHEME: &str = "mightyclaude";
pub const MAXIMUM_PROMPT_BYTES: usize = 64 * 1024;

/// One `open` link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeepLink {
    pub path: String,
    pub prompt: Option<String>,
    pub agent: Option<String>,
}

/// `%XX` decoded as URLComponents decodes a query item (a `+` stays a `+`); None for a bad escape or text that is not UTF-8.
fn decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%' {
            // Two hex digits, nothing else (`from_str_radix` alone would take `%+A`).
            let hex = bytes.get(at + 1..at + 3).filter(|h| h.iter().all(u8::is_ascii_hexdigit))?;
            out.push(u8::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok()?);
            at += 3;
        } else {
            out.push(bytes[at]);
            at += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// `URL(fileURLWithPath:).standardizedFileURL.path` of an absolute path: `.` and empty parts dropped, `..` taking the part before it
/// (never above the root), no trailing slash.
pub fn standardized(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            part => parts.push(part),
        }
    }
    format!("/{}", parts.join("/"))
}

/// A character a path shown to the person must not hold: a control character, or a bidirectional embedding, override or isolate
/// (U+202A–U+202E, U+2066–U+2069), which would make the folder read as another.
fn deceptive(c: char) -> bool {
    c.is_control() || matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

/// Swift `DeepLink.parse`: the scheme and the `open` host, an absolute path (with no control or bidirectional formatting character,
/// review item 6), a prompt kept only when it is not empty and at most 64 KB, an agent only when it is one of the providers.
pub fn parse(url: &str) -> Option<DeepLink> {
    let parsed = tauri::Url::parse(url).ok()?;
    let scheme = parsed.scheme().to_ascii_lowercase();
    if scheme != SCHEME && scheme != LEGACY_SCHEME || !parsed.host_str().is_some_and(|h| h.eq_ignore_ascii_case("open")) {
        return None;
    }
    let mut items: Vec<(String, Option<String>)> = Vec::new();
    for item in parsed.query()?.split('&') {
        let (name, value) = match item.split_once('=') {
            Some((name, value)) => (name, Some(value)),
            None => (item, None),
        };
        items.push((decode(name)?, value.and_then(decode)));
    }
    let value = |name: &str| items.iter().find(|(n, _)| n == name).and_then(|(_, v)| v.clone());
    let raw = value("path").filter(|p| p.starts_with('/') && !p.chars().any(deceptive))?;
    let prompt = value("prompt").filter(|p| !p.is_empty() && p.len() <= MAXIMUM_PROMPT_BYTES);
    let agent = value("agent").filter(|a| mighty_core::wire::PROVIDERS.contains(&a.as_str()));
    Some(DeepLink { path: standardized(&raw), prompt, agent })
}

/// Swift `DeepLink.url`: the link that opens `path`, with a prompt.
#[cfg(any(feature = "dev-harness", test))]
pub fn url(path: &str, prompt: Option<&str>) -> String {
    fn encode(text: &str) -> String {
        // URLComponents' query encoding: everything but the unreserved characters and the query's own safe ones.
        text.bytes()
            .map(|b| match b {
                b'A'..=b'Z'
                | b'a'..=b'z'
                | b'0'..=b'9'
                | b'-'
                | b'.'
                | b'_'
                | b'~'
                | b'/'
                | b'!'
                | b'$'
                | b'\''
                | b'('
                | b')'
                | b'*'
                | b','
                | b';'
                | b':'
                | b'@'
                | b'?' => (b as char).to_string(),
                _ => format!("%{b:02X}"),
            })
            .collect()
    }
    let mut out = format!("{SCHEME}://open?path={}", encode(path));
    if let Some(prompt) = prompt {
        out.push_str(&format!("&prompt={}", encode(prompt)));
    }
    out
}

/// The link's text as it may go into a draft (review item 1): trimmed, its lines' trailing white space dropped, and every run of blank
/// lines one blank line. None when nothing is left.
pub fn cleaned(prompt: &str) -> Option<String> {
    let mut lines: Vec<&str> = Vec::new();
    for line in prompt.trim().lines().map(str::trim_end) {
        if line.is_empty() && lines.last().is_some_and(|last| last.is_empty()) {
            continue;
        }
        lines.push(line);
    }
    let text = lines.join("\n");
    (!text.is_empty()).then_some(text)
}

/// Whether the link's text needs the person's yes before it joins `draft`: when the draft already holds words.
pub fn asks(draft: &str) -> bool {
    !draft.trim().is_empty()
}

/// The words of a link's confirmation that the app has no catalogue key for yet (the locale files are the C# page's too, whose build
/// must not change): the question when the draft already holds words, its message (the pane's title, then the link's text) and its
/// button, in the app's languages.
pub fn draft_words(language: &str) -> (&'static str, &'static str, &'static str) {
    match language {
        "ko" => (
            "링크의 내용을 초안에 추가할까요?",
            "링크가 \"{pane}\"의 초안 뒤에 다음 내용을 추가하려 합니다. 보내지는 않습니다.\n\n{text}",
            "추가",
        ),
        "ja" => (
            "リンクの内容を下書きに追加しますか?",
            "リンクが「{pane}」の下書きの後に次の内容を追加しようとしています。送信はされません。\n\n{text}",
            "追加",
        ),
        "zh" => ("要把链接的内容添加到草稿吗?", "一个链接想把以下内容添加到“{pane}”的草稿之后。不会发送。\n\n{text}", "添加"),
        _ => (
            "Add the link's text to the draft?",
            "A link wants to add this after the draft of \"{pane}\". It is not sent.\n\n{text}",
            "Add",
        ),
    }
}

/// A confirmation a link raises: Cancel is its default (review item 1).
pub fn confirmation(locale: &mighty_core::locale::Locale, title: &str, message: &str, primary: &str) -> serde_json::Value {
    json!({
        "type": "confirm",
        "title": title,
        "message": message,
        "primary": primary,
        "close": locale.get("runChanges.confirm.cancel"),
        "defaultPrimary": false,
    })
}

/// The pane a link's text goes to without an agent named (Swift `newestAgentPane`): the one used last — its newest log entry, else
/// when it was made.
fn last_used(pane: &mighty_core::model::RunSession) -> Option<i64> {
    let ticks = |at: Option<&str>| mighty_core::time::parse(at).map(|t| t.utc_ticks());
    let used = pane.logs().iter().filter_map(|entry| ticks(entry.timestamp.as_deref())).max();
    used.into_iter().chain(ticks(pane.created_at())).max()
}

/// Asks with `dialog` (Cancel the default) unless a sheet is up; true only for the primary button.
async fn ask(host: &Arc<Host>, dialog: serde_json::Value) -> bool {
    let Some(_sheet) = crate::frame::Sheet::begin(host) else { return false };
    let (_, answer) = crate::dialogs::of(host).open(dialog);
    answer.await.ok() == Some(serde_json::Value::Bool(true))
}

/// Swift `ReviewNotes.appending`: `block` after what the composer holds, a blank line between.
pub fn appending(block: &str, draft: &str) -> String {
    if block.is_empty() {
        return draft.to_string();
    }
    let trimmed = draft.trim();
    if trimmed.is_empty() { block.to_string() } else { format!("{trimmed}\n\n{block}") }
}

/// Addresses the OS opened the app with (`RunEvent::Opened`): each `open` link handled in turn once the app has its host.
#[cfg_attr(not(target_os = "macos"), allow(dead_code, reason = "RunEvent::Opened is the Mac's (and iOS's); Windows has no such event"))]
pub fn opened(app: &tauri::AppHandle, urls: &[tauri::Url]) {
    let links: Vec<String> = urls.iter().map(|u| u.to_string()).collect();
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        // A link that launched the app arrives as the app starts: its host is managed a moment later.
        let mut host = None;
        for _ in 0..100 {
            host = app.try_state::<Arc<Host>>().map(|h| h.inner().clone());
            if host.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let Some(host) = host else { return };
        host.log.line(&format!("deep link: {} address(es) received", links.len()));
        for link in links.iter().filter_map(|l| parse(l)) {
            if let Err(error) = open(&host, &link, true).await {
                host.log.line(&format!("deep link: not opened: {error}"));
            }
        }
    });
}

/// Swift `handleDeepLink`: the folder's workspace selected (a folder that is not one yet added only after the user agrees), then the
/// link's text put in an agent pane's composer — never sent. `activate` brings the window forward (not in a smoke). The pane the text
/// went to, if any.
pub async fn open(host: &Arc<Host>, link: &DeepLink, activate: bool) -> Result<Option<String>, String> {
    // A link that launched the app waits for the saved state and the page.
    for _ in 0..100 {
        if host.view().ready {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let (ready, dialog) = {
        let view = host.view();
        (view.ready, view.dialog_open)
    };
    if !ready || dialog || crate::dialogs::of(host).open_count() > 0 {
        return Ok(None);
    }
    if !std::path::Path::new(&link.path).is_dir() {
        host.view().error = Some(host.locale.get_with("deepLink.error.noFolder", &[("path", &link.path)]));
        return Ok(None);
    }
    if activate && let Some(window) = host.app.get_webview_window(crate::host::WINDOW) {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
    let existing = {
        let view = host.view();
        view.snapshot.workspaces().iter().find(|w| w.remote.is_none() && standardized(w.path()) == link.path).map(|w| w.id().to_string())
    };
    match existing {
        Some(id) => {
            let mut view = host.view();
            crate::frame::select_workspace(&mut view, &id)?;
        }
        None => {
            let l = &host.locale;
            let dialog = confirmation(
                l,
                &l.get("deepLink.confirm.title"),
                &l.get_with("deepLink.confirm.message", &[("path", &link.path)]),
                &l.get("deepLink.confirm.open"),
            );
            if !ask(host, dialog).await {
                return Ok(None);
            }
            add_workspace(host, std::path::Path::new(&link.path), link.agent.as_deref()).await?;
        }
    }
    let Some(prompt) = link.prompt.as_deref().and_then(cleaned) else { return Ok(None) };
    let (workspace, found) = {
        let view = host.view();
        let Some(workspace) = view.snapshot.active_workspace_id.clone() else { return Ok(None) };
        let agents = view
            .snapshot
            .sessions()
            .iter()
            .filter(|s| s.workspace_id() == workspace && s.kind() == "claude" && s.parent_session_id.is_none());
        let found = match &link.agent {
            Some(agent) => agents.filter(|s| s.provider() == agent).map(|s| s.id().to_string()).next(),
            // Swift `newestAgentPane`: the one used last.
            None => agents.max_by_key(|s| last_used(s)).map(|s| s.id().to_string()),
        };
        (workspace, found)
    };
    let pane = match found {
        Some(pane) => pane,
        None => {
            let provider = link.agent.as_deref().unwrap_or(add_pane::NEW_PANE_SHORTCUT_PROVIDER);
            let mut made = None;
            crate::frame::add_pane_to(host, Some(&workspace), "claude", provider, None, |pane| {
                made = Some(pane.id().to_string());
                pane
            })?;
            let Some(made) = made else { return Ok(None) };
            made
        }
    };
    let (draft, title) = {
        let mut view = host.view();
        crate::frame::select_session(&mut view, &pane)?;
        let title = view.snapshot.session(&pane).map(|s| s.title().to_string()).unwrap_or_default();
        (view.panes.get(&pane).map(|ui| ui.view.draft.clone()).unwrap_or_default(), title)
    };
    if asks(&draft) {
        let (question, message, add) = draft_words(host.locale.language());
        let message = message.replace("{pane}", &title).replace("{text}", &prompt);
        if !ask(host, confirmation(&host.locale, question, &message, add)).await {
            return Ok(None);
        }
    }
    let mut view = host.view();
    let Ok(ui) = crate::panes::ui(&mut view, &pane) else { return Ok(Some(pane)) };
    ui.view.draft = crate::composer::clamp(&appending(&prompt, &ui.view.draft));
    ui.dirty = true;
    let draft = ui.view.draft.clone();
    if let Some(runs) = host.runs()
        && matches!(runs.service.update_draft(&pane, &draft), Ok(true))
    {
        view.sync();
    }
    Ok(Some(pane))
}

/// C# `OpenWorkspaceFolder` for a link (frame.rs `open_workspace_folder`), with its one new pane the link's agent's: the folder made a
/// workspace and selected; a workspace with no pane gets one agent pane of `agent` (else the default provider), never two (review item
/// 14).
async fn add_workspace(host: &Arc<Host>, path: &std::path::Path, agent: Option<&str>) -> Result<(), String> {
    let workspace = host.approve_local(path).map_err(|error| {
        let text = error.to_string();
        if text.starts_with("store.error.") { host.locale.get(&text) } else { text }
    })?;
    let has_panes = {
        let mut view = host.view();
        let id = workspace.id().to_string();
        view.update(|state| {
            if !state.workspaces().iter().any(|w| w.id() == id) {
                state.workspaces_mut().push(workspace.clone());
            }
            if let Some(expanded) = mighty_core::shell::open_disclosure(state, &id) {
                state.expanded_workspace_ids = Some(expanded);
            }
            state.active_workspace_id = Some(id.clone());
        })?;
        view.shows_dashboard = false;
        view.snapshot.sessions().iter().any(|s| s.workspace_id() == id)
    };
    if !has_panes {
        crate::pane_worktree::add_new_agent_pane(host, agent.unwrap_or(add_pane::NEW_PANE_SHORTCUT_PROVIDER), None).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Swift `OSIntegrationTests.swift:7` "linksAreParsedStrictly".
    #[test]
    fn links_are_parsed_strictly() {
        let link = parse("mightycode://open?path=/Users/me/app/../app&prompt=fix%20the%20tests&agent=codex").unwrap();
        assert_eq!(link, DeepLink { path: "/Users/me/app".into(), prompt: Some("fix the tests".into()), agent: Some("codex".into()) });
        assert_eq!(parse("mightycode://open?path=relative"), None);
        assert_eq!(parse("mightycode://run?path=/x"), None);
        assert_eq!(parse("https://open?path=/x"), None);
        assert_eq!(parse("mightycode://open?path=/x&agent=bash").unwrap().agent, None);
        let round = url("/a b/c", Some("한글 & 기호"));
        assert_eq!(parse(&round), Some(DeepLink { path: "/a b/c".into(), prompt: Some("한글 & 기호".into()), agent: None }));
        // What else a strict reading refuses: a NUL in the path, an empty or too long prompt (the folder still opens).
        assert_eq!(parse("mightycode://open?path=/x%00y"), None);
        assert_eq!(parse("mightycode://open?path=/x&prompt=").unwrap().prompt, None);
        let long = format!("mightycode://open?path=/x&prompt={}", "a".repeat(MAXIMUM_PROMPT_BYTES + 1));
        assert_eq!(parse(&long).unwrap().prompt, None);
        assert_eq!(parse("mightycode://open?path=/x&prompt=a+b").unwrap().prompt.as_deref(), Some("a+b"), "a plus stays a plus");
    }

    /// Swift `RenameMigrationTests.swift:53` "theOldSchemesStillOpen".
    #[test]
    fn the_old_schemes_still_open() {
        assert_eq!(parse("mightyclaude://open?path=/tmp/project").unwrap().path, "/tmp/project");
        assert!(url("/tmp/project", None).starts_with("mightycode://"));
    }

    /// Review items 6 and 7: a path with a control or bidirectional formatting character, and an escape that is not two hex digits.
    #[test]
    fn deceptive_paths_and_loose_escapes_are_refused() {
        assert_eq!(parse("mightycode://open?path=/tmp/a%0Ab"), None);
        assert_eq!(parse("mightycode://open?path=/tmp/%E2%80%AEtxt.app"), None, "U+202E");
        assert_eq!(parse("mightycode://open?path=/tmp/%E2%81%A6x"), None, "U+2066");
        assert_eq!(parse("mightycode://open?path=/tmp/%+Ab"), None);
        assert_eq!(parse("mightycode://open?path=/tmp/%4"), None);
        assert_eq!(parse("mightycode://open?path=/tmp/%41").unwrap().path, "/tmp/A");
        assert_eq!(parse("mightycode://open?path=/tmp/%ED%95%9C").unwrap().path, "/tmp/한");
    }

    /// Review item 1 (a deliberate deviation from Swift): the link's text trimmed with its blank-line runs collapsed; a draft with words
    /// asks first, and the question shows the text; every link's confirmation defaults to Cancel.
    #[test]
    fn a_links_text_is_cleaned_and_asks_before_joining_a_draft() {
        // A line's indentation stays (code in a request); its trailing white space goes.
        assert_eq!(cleaned("  \n\nfix it \n\n\n\n  then test\t\n\n"), Some("fix it\n\n  then test".into()));
        assert_eq!(cleaned(" \n \t "), None);
        assert_eq!(cleaned("one\r\n\r\n\r\ntwo"), Some("one\n\ntwo".into()));
        assert!(!asks("") && !asks("  \n "));
        assert!(asks("half a thought"));
        let locale = mighty_core::locale::Locale::new("en");
        let (question, message, add) = draft_words("en");
        let dialog = confirmation(&locale, question, &message.replace("{pane}", "Claude").replace("{text}", "fix it"), add);
        assert_eq!(dialog["defaultPrimary"], false);
        assert!(dialog["message"].as_str().unwrap().ends_with("\n\nfix it"));
        assert_eq!(dialog["close"], locale.get("runChanges.confirm.cancel"));
        for language in mighty_core::locale::LANGUAGES {
            let (question, message, add) = draft_words(language);
            assert!(!question.is_empty() && message.contains("{pane}") && message.contains("{text}") && !add.is_empty());
        }
    }

    #[test]
    fn a_prompt_follows_the_draft_after_a_blank_line() {
        assert_eq!(appending("fix it", ""), "fix it");
        assert_eq!(appending("fix it", "  draft \n"), "draft\n\nfix it");
        assert_eq!(appending("", "draft"), "draft");
        assert_eq!(standardized("/a//b/./c/../d/"), "/a/b/d");
        assert_eq!(standardized("/../.."), "/");
    }
}
