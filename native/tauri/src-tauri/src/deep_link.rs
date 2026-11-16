//! M-F: deep links on the Mac (`RunEvent::Opened`, the bundle's `CFBundleURLTypes`): `mightycode://open?path=<abs>&prompt=<≤64 KB>&
//! agent=<provider>`, parsed strictly (Swift `OSIntegration.swift:7-34` `DeepLink.parse`); both `mightycode` and the legacy
//! `mightyclaude` accepted (plan D11, where Swift dropped the legacy one at run time). The handler waits up to 10 s for the page, asks
//! before adding a new folder, and puts the prompt in the draft without sending it (`AppStore+OSIntegration.swift:9-43`
//! `handleDeepLink`); a cold start's address arrives after the window is ready. Nothing a link carries is ever sent: any web page can
//! open one. The log names the count of addresses only (a prompt is the user's text).

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
            let hex = std::str::from_utf8(bytes.get(at + 1..at + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
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

/// Swift `DeepLink.parse`: the scheme and the `open` host, an absolute path with no NUL, a prompt kept only when it is not empty and
/// at most 64 KB, an agent only when it is one of the providers.
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
    let raw = value("path").filter(|p| p.starts_with('/') && !p.contains('\0'))?;
    let prompt = value("prompt").filter(|p| !p.is_empty() && p.len() <= MAXIMUM_PROMPT_BYTES);
    let agent = value("agent").filter(|a| mighty_core::wire::PROVIDERS.contains(&a.as_str()));
    Some(DeepLink { path: standardized(&raw), prompt, agent })
}

/// Swift `DeepLink.url`: the link that opens `path`, with a prompt.
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
            let agreed = {
                let Some(_sheet) = crate::frame::Sheet::begin(host) else { return Ok(None) };
                let l = &host.locale;
                let (_, answer) = crate::dialogs::of(host).open(json!({
                    "type": "confirm",
                    "title": l.get("deepLink.confirm.title"),
                    "message": l.get_with("deepLink.confirm.message", &[("path", &link.path)]),
                    "primary": l.get("deepLink.confirm.open"),
                    "close": l.get("runChanges.confirm.cancel"),
                    "defaultPrimary": true,
                }));
                answer.await.ok() == Some(serde_json::Value::Bool(true))
            };
            if !agreed {
                return Ok(None);
            }
            crate::frame::open_workspace_folder(host, std::path::Path::new(&link.path)).await?;
        }
    }
    let Some(prompt) = &link.prompt else { return Ok(None) };
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
            // Swift `newestAgentPane`: the one used last (the newest made, here).
            None => agents.max_by(|a, b| a.created_at().cmp(&b.created_at())).map(|s| s.id().to_string()),
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
    let mut view = host.view();
    crate::frame::select_session(&mut view, &pane)?;
    let Ok(ui) = crate::panes::ui(&mut view, &pane) else { return Ok(Some(pane)) };
    ui.view.draft = crate::composer::clamp(&appending(prompt, &ui.view.draft));
    ui.dirty = true;
    let draft = ui.view.draft.clone();
    if let Some(runs) = host.runs()
        && matches!(runs.service.update_draft(&pane, &draft), Ok(true))
    {
        view.sync();
    }
    Ok(Some(pane))
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

    #[test]
    fn a_prompt_follows_the_draft_after_a_blank_line() {
        assert_eq!(appending("fix it", ""), "fix it");
        assert_eq!(appending("fix it", "  draft \n"), "draft\n\nfix it");
        assert_eq!(appending("", "draft"), "draft");
        assert_eq!(standardized("/a//b/./c/../d/"), "/a/b/d");
        assert_eq!(standardized("/../.."), "/");
    }
}
