//! MC-7: the live preview of a worktree's dev server, and picking an element of it for the composer.
//!
//! - 프리뷰 (the pane header's globe): the dev servers listening in the pane's workspace — its main checkout's and each worktree
//!   pane's port block (MC-25), the addresses their running run actions printed, and the ports each pane's processes hold (MC-26)
//!   — found by asking each port once over loopback ([`probe`]: an HTTP answer, or a TLS alert for https). A server opens in the
//!   preview of the pane it belongs to; a port a pane listens on outside its ports is listed but not opened.
//! - Opening follows MC-48's browser rule ([`Allowed`], mighty-delegation `verify::allowed_browser_url`): http(s) on `localhost`,
//!   `127.0.0.1` or `[::1]`, no credentials, a port in the pane's block or a running run action's address. The preview is a window
//!   of an installed Chrome, Edge, Chromium or Brave on a private profile ([`session`]), never a webview of this app: the page has
//!   no way to the app's IPC (the app's capability is the main webview's alone, `host_post` takes only its page with this run's
//!   nonce, and no page can open the browser's DevTools socket). Without such a browser the address opens in the default browser,
//!   with no element picking.
//! - The strip over the pane's composer: 요소 고르기 (DevTools' inspect mode; the element clicked goes to the composer as
//!   `preview-element-<tag>.txt`: [`element::describe`]), 스크린샷 (the element picked last, else the page, as an image attachment),
//!   콘솔 오류 n and 실패한 요청 n (the page's since it last loaded, as text attachments), 닫기. SSH and remote workspaces are not
//!   previewed yet.

pub mod element;
pub mod session;

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use mighty_delegation::verify::{self, BrowserScope};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::sync::oneshot;

use crate::host::{Host, Registry, lock};
use crate::methods::text;

/// How long a port may take to accept, and then to answer.
const PROBE_CONNECT: Duration = Duration::from_millis(250);
const PROBE_ANSWER: Duration = Duration::from_millis(600);
/// The most ports asked at once.
const PROBE_AT_ONCE: usize = 64;

/// Where a pane's preview may go: its port block and the addresses its running run actions printed (each a scope of MC-48's rule).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Allowed {
    scopes: Vec<BrowserScope>,
}

impl Allowed {
    pub fn new(block: Option<(u16, u16)>, actions: Vec<String>) -> Allowed {
        let scopes = if actions.is_empty() {
            vec![BrowserScope { block, detected: None }]
        } else {
            actions.into_iter().map(|url| BrowserScope { block, detected: Some(url) }).collect()
        };
        Allowed { scopes }
    }

    /// `url` checked; Err with the reason of the last scope that refused it.
    pub fn check(&self, url: &str) -> Result<url::Url, String> {
        let mut refused = Err(format!("'{url}' is not an address."));
        for scope in &self.scopes {
            match verify::allowed_browser_url(url, scope) {
                Ok(parsed) => return Ok(parsed),
                Err(reason) => refused = Err(reason),
            }
        }
        refused
    }

    pub fn allows(&self, url: &str) -> bool {
        self.check(url).is_ok()
    }

    /// Every port the pane's preview may open: its block's, then its run actions'.
    fn ports(&self) -> Vec<u16> {
        let mut out = Vec::new();
        if let Some((base, count)) = self.scopes.first().and_then(|s| s.block) {
            out.extend((0..count).filter_map(|i| base.checked_add(i)));
        }
        for scope in &self.scopes {
            if let Some(port) = scope.detected.as_deref().and_then(|u| url::Url::parse(u).ok()).and_then(|u| u.port_or_known_default())
                && !out.contains(&port)
            {
                out.push(port);
            }
        }
        out
    }
}

/// Where a pane's runs work, for its preview.
#[derive(Debug, Clone)]
struct Place {
    pane: String,
    /// The worktree's branch; None for the main checkout.
    branch: Option<String>,
    folder: String,
    allowed: Allowed,
    /// Its running run actions with the address each printed.
    actions: Vec<(String, String)>,
}

/// Why a pane cannot be previewed (an SSH or remote workspace, a pane that is gone).
fn place_of(host: &Host, pane_id: &str) -> Result<Place, String> {
    let (pane, workspace) = {
        let view = host.view();
        let pane = view.snapshot.session(pane_id).cloned().ok_or_else(|| "Session not found.".to_string())?;
        let workspace = view.snapshot.workspace(pane.workspace_id()).cloned().ok_or_else(|| "Workspace not found.".to_string())?;
        (pane, workspace)
    };
    if workspace.remote.is_some() || workspace.ssh.is_some() || workspace.path().is_empty() {
        return Err(host.locale.get("preview.error.localOnly"));
    }
    let record = pane.pane_worktree.clone();
    let (folder, block, branch) = match &record {
        Some(record) => {
            let block = record.port_block().and_then(|(base, count)| Some((u16::try_from(base).ok()?, u16::try_from(count).ok()?)));
            (record.run_folder().to_string(), block, Some(record.branch().to_string()))
        }
        None => {
            let block = crate::worktree_lifecycle::main_block(host, Path::new(workspace.path())).map(|b| (b.base, b.count));
            (workspace.path().to_string(), block, None)
        }
    };
    let key = crate::worktree_lifecycle::path_key(&folder);
    let actions: Vec<(String, String)> = crate::repo_tasks::running_actions_in(host, &key)
        .into_iter()
        .filter_map(|run| Some((run["name"].as_str()?.to_string(), run["url"].as_str()?.to_string())))
        .filter(|(_, url)| url::Url::parse(url).is_ok_and(|u| matches!(u.scheme(), "http" | "https")))
        .collect();
    let allowed = Allowed::new(block, actions.iter().map(|(_, url)| url.clone()).collect());
    Ok(Place { pane: pane_id.to_string(), branch, folder, allowed, actions })
}

/// The places of `pane_id`'s workspace: the pane's own first, then each other agent pane's worktree, and the main checkout once
/// (on the asking pane when it works there, else the first pane that does).
fn places(host: &Host, pane_id: &str) -> Result<Vec<Place>, String> {
    let own = place_of(host, pane_id)?;
    let others: Vec<String> = {
        let view = host.view();
        let workspace = view.snapshot.session(pane_id).map(|s| s.workspace_id().to_string()).unwrap_or_default();
        view.snapshot
            .sessions()
            .iter()
            .filter(|s| s.workspace_id() == workspace && s.kind() == "claude" && s.id() != pane_id)
            .map(|s| s.id().to_string())
            .collect()
    };
    let mut seen: HashSet<String> = HashSet::from([crate::worktree_lifecycle::path_key(&own.folder)]);
    let mut out = vec![own];
    for other in others {
        let Ok(place) = place_of(host, &other) else { continue };
        if seen.insert(crate::worktree_lifecycle::path_key(&place.folder)) {
            out.push(place);
        }
    }
    Ok(out)
}

/// What answers on `port` over loopback: `http` (an HTTP reply to a HEAD), `https` (a TLS alert), or None (nothing, or not a web
/// server: a database answers neither).
pub async fn probe(port: u16) -> Option<&'static str> {
    for host in ["127.0.0.1", "::1"] {
        let Ok(Ok(mut stream)) = tokio::time::timeout(PROBE_CONNECT, tokio::net::TcpStream::connect((host, port))).await else { continue };
        let asked = format!("HEAD / HTTP/1.0\r\nHost: localhost:{port}\r\n\r\n");
        if stream.write_all(asked.as_bytes()).await.is_err() {
            return None;
        }
        let mut answer = [0u8; 8];
        let read = tokio::time::timeout(PROBE_ANSWER, stream.read(&mut answer)).await;
        return match read {
            Ok(Ok(n)) if n >= 5 && &answer[..5] == b"HTTP/" => Some("http"),
            Ok(Ok(n)) if n >= 1 && answer[0] == 0x15 => Some("https"),
            _ => None,
        };
    }
    None
}

/// Each of `ports` probed, at most [`PROBE_AT_ONCE`] at a time.
async fn probe_all(ports: Vec<u16>) -> HashMap<u16, &'static str> {
    let mut found = HashMap::new();
    for chunk in ports.chunks(PROBE_AT_ONCE) {
        let answers = futures_util::future::join_all(chunk.iter().map(|&port| async move { (port, probe(port).await) })).await;
        found.extend(answers.into_iter().filter_map(|(port, scheme)| Some((port, scheme?))));
    }
    found
}

/// The servers of the places, in order: each place's run actions' addresses, then the other ports of its block that answer; and the
/// ports its pane's processes listen on outside its ports (`outside`).
async fn servers(places: &[Place], open: &HashSet<String>) -> (Vec<Value>, Vec<Value>) {
    let mut wanted: Vec<u16> = places.iter().flat_map(|p| p.allowed.ports()).collect();
    let pane_ports: HashMap<String, Vec<u16>> = places.iter().map(|p| (p.pane.clone(), crate::attention::pane_ports(&p.pane))).collect();
    wanted.extend(pane_ports.values().flatten().copied());
    wanted.sort_unstable();
    wanted.dedup();
    let answering = probe_all(wanted).await;
    let mut servers = Vec::new();
    let mut outside = Vec::new();
    let mut listed: HashSet<u16> = HashSet::new();
    for place in places {
        let row = |url: String, port: u16, action: Option<&str>| {
            json!({
                "paneId": place.pane,
                "branch": place.branch,
                "url": url,
                "port": port,
                "action": action,
                "open": open.contains(&place.pane),
            })
        };
        for (name, url) in &place.actions {
            let Ok(parsed) = place.allowed.check(url) else { continue };
            let Some(port) = parsed.port_or_known_default() else { continue };
            if listed.insert(port) {
                servers.push(row(parsed.to_string(), port, Some(name)));
            }
        }
        for port in place.allowed.ports() {
            if let Some(scheme) = answering.get(&port)
                && listed.insert(port)
            {
                servers.push(row(format!("{scheme}://localhost:{port}/"), port, None));
            }
        }
        for &port in pane_ports.get(&place.pane).into_iter().flatten() {
            if answering.contains_key(&port) && !listed.contains(&port) && !place.allowed.ports().contains(&port) {
                listed.insert(port);
                outside.push(json!({ "paneId": place.pane, "branch": place.branch, "port": port }));
            }
        }
    }
    (servers, outside)
}

// ── what the page shows ─────────────────────────────────────────────────────────────────────────────────────────────────

/// Each pane's open preview as the strip draws it, and the last thing said on it.
#[derive(Default)]
struct Shown {
    previews: HashMap<String, Value>,
    notes: HashMap<String, String>,
}

static SHOWN: LazyLock<Mutex<Shown>> = LazyLock::new(Mutex::default);

fn message(pane: &str) -> Value {
    let shown = lock(&SHOWN);
    json!({
        "kind": "preview",
        "paneId": pane,
        "preview": shown.previews.get(pane),
        "note": shown.notes.get(pane),
    })
}

fn push(host: &Host, pane: &str) {
    let message = message(pane).to_string();
    host.push(&host.view(), "preview", message);
}

/// The session's word on a pane's preview (None when it closed), pushed.
fn set_shown(host: &Host, pane: &str, preview: Option<Value>) {
    {
        let mut shown = lock(&SHOWN);
        match preview {
            Some(value) => shown.previews.insert(pane.to_string(), value),
            None => shown.previews.remove(pane),
        };
    }
    push(host, pane);
}

/// Something said on a pane's strip (None clears it), pushed.
fn note(host: &Host, pane: &str, words: Option<String>) {
    {
        let mut shown = lock(&SHOWN);
        match words {
            Some(words) => shown.notes.insert(pane.to_string(), words),
            None => shown.notes.remove(pane),
        };
    }
    push(host, pane);
}

/// A pane closed: its preview closes with it (called with the view locked: nothing here pushes).
pub fn forget(pane: &str) {
    let had = {
        let mut shown = lock(&SHOWN);
        shown.notes.remove(pane);
        shown.previews.remove(pane).is_some()
    };
    if had {
        let pane = pane.to_string();
        tauri::async_runtime::spawn(async move { session::close(pane).await });
    }
}

/// The app quits.
pub fn shutdown() {
    session::shutdown();
}

// ── the page's methods ──────────────────────────────────────────────────────────────────────────────────────────────────

fn pane_arg(args: &Value) -> Result<String, String> {
    text(args, "paneId").filter(|id| !id.is_empty()).map(str::to_string).ok_or_else(|| "Invalid request.".into())
}

/// Waits for the session's answer; a browser that closed meanwhile is said so.
async fn answer<T>(host: &Host, receiver: oneshot::Receiver<Result<T, String>>) -> Result<T, String> {
    receiver.await.map_err(|_| host.locale.get("preview.error.closed"))?
}

/// The result of a strip action: its note cleared when it went well, set when it did not.
fn said<T>(host: &Host, pane: &str, result: Result<T, String>) -> Result<T, String> {
    match &result {
        Ok(_) => {
            if lock(&SHOWN).notes.contains_key(pane) {
                note(host, pane, None);
            }
        }
        Err(error) => note(host, pane, Some(error.clone())),
    }
    result
}

async fn open(host: &Arc<Host>, pane: &str, url: &str, external: bool) -> Result<Value, String> {
    let place = place_of(host, pane)?;
    let url = place.allowed.check(url).map_err(|reason| host.locale.get_with("preview.error.refused", &[("reason", &reason)]))?;
    if external || session::running().await.is_none() && crate::verify::browser::installed().is_none() {
        let address = url.to_string();
        tauri::async_runtime::spawn_blocking(move || crate::platform::open_url(&address)).await.map_err(|e| e.to_string())??;
        return Ok(json!({ "opened": "external" }));
    }
    let mut tries = 0;
    loop {
        let (reply, receiver) = oneshot::channel();
        let command = session::Command::Open { pane: pane.to_string(), url: url.clone(), allowed: place.allowed.clone(), reply };
        session::send(host, command).await?;
        match answer(host, receiver).await {
            // The browser was closing as the open came: once more, in a new one.
            Err(error) if tries == 0 && error == host.locale.get("preview.error.closed") => tries += 1,
            Err(error) => return Err(error),
            Ok(()) => return Ok(json!({ "opened": "preview" })),
        }
    }
}

pub fn register(methods: &mut Registry) {
    // The dev servers of the pane's workspace, each with the pane its preview belongs to.
    methods.add("previewTargets", |host: Arc<Host>, args| async move {
        let pane = pane_arg(&args)?;
        let places = match places(&host, &pane) {
            Ok(places) => places,
            Err(reason) => return Ok(json!({ "available": false, "reason": reason })),
        };
        let open: HashSet<String> = lock(&SHOWN).previews.keys().cloned().collect();
        let (servers, outside) = servers(&places, &open).await;
        let browser = match session::running().await {
            Some(name) => Some(name),
            None => tauri::async_runtime::spawn_blocking(|| crate::verify::browser::installed().map(|(name, _)| name)).await.ok().flatten(),
        };
        Ok(json!({ "available": true, "browser": browser, "servers": servers, "outside": outside }))
    });

    // Opens `url` in the pane's preview (or the default browser: `external`, or no browser to drive), within the pane's ports.
    methods.add("previewOpen", |host: Arc<Host>, args| async move {
        let pane = pane_arg(&args)?;
        let url = text(&args, "url").unwrap_or_default().to_string();
        let result = open(&host, &pane, &url, args["external"] == true).await;
        said(&host, &pane, result)
    });

    methods.add("previewState", |_host: Arc<Host>, args| async move { Ok(message(&pane_arg(&args)?)) });

    methods.add("previewDismissNote", |host: Arc<Host>, args| async move {
        note(&host, &pane_arg(&args)?, None);
        Ok(Value::Null)
    });

    // 요소 고르기 on or off.
    methods.add("previewPick", |host: Arc<Host>, args| async move {
        let pane = pane_arg(&args)?;
        let (reply, receiver) = oneshot::channel();
        let result = async {
            session::send(&host, session::Command::Pick { pane: pane.clone(), on: args["on"] != false, reply }).await?;
            answer(&host, receiver).await
        };
        let result = result.await;
        said(&host, &pane, result).map(|()| Value::Null)
    });

    // 스크린샷: the element picked last (else the page) to the composer.
    methods.add("previewScreenshot", |host: Arc<Host>, args| async move {
        let pane = pane_arg(&args)?;
        let (reply, receiver) = oneshot::channel();
        let result = async {
            session::send(&host, session::Command::Screenshot { pane: pane.clone(), reply }).await?;
            let (name, bytes) = answer(&host, receiver).await?;
            crate::composer::add_files(&host, &pane, vec![(name, bytes)]).await
        };
        let result = result.await;
        said(&host, &pane, result).map(|()| Value::Null)
    });

    // 콘솔 오류 / 실패한 요청: the page's since it last loaded, to the composer.
    methods.add("previewFindings", |host: Arc<Host>, args| async move {
        let pane = pane_arg(&args)?;
        let which = match text(&args, "which") {
            Some("console") => session::Findings::Console,
            Some("requests") => session::Findings::Requests,
            _ => return Err("Invalid request.".into()),
        };
        let (reply, receiver) = oneshot::channel();
        let result = async {
            session::send(&host, session::Command::Findings { pane: pane.clone(), which, reply }).await?;
            let (name, words) = answer(&host, receiver).await?;
            crate::composer::add_files(&host, &pane, vec![(name, words.into_bytes())]).await
        };
        let result = result.await;
        said(&host, &pane, result).map(|()| Value::Null)
    });

    methods.add("previewClose", |host: Arc<Host>, args| async move {
        let pane = pane_arg(&args)?;
        let _ = session::send(&host, session::Command::Close { pane: pane.clone() }).await;
        {
            let mut shown = lock(&SHOWN);
            shown.notes.remove(&pane);
            shown.previews.remove(&pane);
        }
        push(&host, &pane);
        Ok(Value::Null)
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_preview_opens_only_on_this_computer_within_the_panes_ports() {
        let allowed = Allowed::new(Some((41000, 10)), vec!["http://localhost:5173/".into()]);
        assert!(allowed.check("http://localhost:41000/").is_ok());
        assert!(allowed.check("http://127.0.0.1:41009/app").is_ok());
        assert!(allowed.check("http://[::1]:41004/").is_ok());
        assert!(allowed.check("http://localhost:5173/settings").is_ok(), "the run action's address");
        for refused in [
            "http://localhost:41010/",
            "http://localhost:3000/",
            "http://0.0.0.0:41000/",
            "http://192.168.0.2:41000/",
            "http://example.com:41000/",
            "http://localhost.example.com:41000/",
            "http://user:secret@localhost:41000/",
            "file:///etc/hosts",
            "javascript:alert(1)",
            "tauri://localhost/index.html",
            "ws://localhost:41000/",
            "",
        ] {
            assert!(allowed.check(refused).is_err(), "{refused}");
        }
    }

    #[test]
    fn every_running_run_action_opens_not_only_the_first() {
        let allowed = Allowed::new(None, vec!["http://localhost:5173/".into(), "http://127.0.0.1:6006/".into()]);
        assert!(allowed.allows("http://localhost:5173/") && allowed.allows("http://localhost:6006/"));
        assert!(!allowed.allows("http://localhost:41000/"), "no block, no other port");
        assert_eq!(allowed.ports(), vec![5173, 6006]);
        assert_eq!(Allowed::new(Some((41000, 3)), vec!["http://localhost:41001/".into()]).ports(), vec![41000, 41001, 41002]);
        assert_eq!(Allowed::new(None, Vec::new()).ports(), Vec::<u16>::new(), "nothing to open: a main checkout without a block");
        assert_eq!(Allowed::new(Some((65534, 10)), Vec::new()).ports(), vec![65534, 65535], "never past the last port");
    }

    #[tokio::test]
    async fn a_dev_server_is_told_from_a_port_that_is_not_a_web_server() {
        use tokio::net::TcpListener;
        let web = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let web_port = web.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = web.accept().await {
                let mut buffer = [0u8; 256];
                let _ = stream.read(&mut buffer).await;
                let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n").await;
            }
        });
        let quiet = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let quiet_port = quiet.local_addr().unwrap().port();
        tokio::spawn(async move {
            let mut kept = Vec::new();
            while let Ok((stream, _)) = quiet.accept().await {
                kept.push(stream);
            }
        });
        let closed = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let closed_port = closed.local_addr().unwrap().port();
        drop(closed);
        assert_eq!(probe(web_port).await, Some("http"));
        assert_eq!(probe(quiet_port).await, None, "a port that never answers HTTP (a database) is no dev server");
        assert_eq!(probe(closed_port).await, None);
        let found = probe_all(vec![web_port, quiet_port, closed_port]).await;
        assert_eq!(found, HashMap::from([(web_port, "http")]));
    }

    /// MC-7 requirement 4: the previewed page cannot reach the app's Tauri IPC. It is never loaded in one of the app's webviews (it
    /// is a window of another program, `session::launch_request`); and even there, nothing would let it in: the app's only capability
    /// is the main webview's, no capability names a remote address, no global `__TAURI__` is injected, and `host_post` trusts only
    /// the app's own origin (main.rs `preview_addresses_are_never_the_apps_origin`).
    #[test]
    fn the_previewed_page_has_no_way_to_the_apps_ipc() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let config: Value = serde_json::from_str(&std::fs::read_to_string(root.join("tauri.conf.json")).unwrap()).unwrap();
        assert_eq!(config["app"]["withGlobalTauri"], false);
        let csp = config["app"]["security"]["csp"].as_str().unwrap();
        assert!(!csp.contains("localhost:") && !csp.contains("127.0.0.1"), "the app's page frames no dev server: {csp}");
        let mut capabilities = 0;
        for entry in std::fs::read_dir(root.join("capabilities")).unwrap() {
            let path = entry.unwrap().path();
            let text = std::fs::read_to_string(&path).unwrap();
            let capability: Value = serde_json::from_str(&text).unwrap();
            capabilities += 1;
            assert_eq!(capability["webviews"], json!(["main"]), "{}: only the app's own page", path.display());
            assert!(capability.get("remote").is_none(), "{}: no remote address gets IPC", path.display());
            assert!(capability.get("windows").is_none_or(|w| w.as_array().is_some_and(|w| w.iter().all(|w| w == "main"))));
        }
        assert!(capabilities >= 1);
        // The preview is opened by a browser this app starts, not by Tauri: no webview is ever made for it.
        let request = session::launch_request(Path::new("/opt/chrome"), Path::new("/tmp/p"));
        assert_eq!(request.program, Path::new("/opt/chrome"));
        let sources = [include_str!("mod.rs"), include_str!("session.rs")];
        for source in sources {
            for made in [concat!("WebviewWindowBuilder", "::new"), concat!("WebviewBuilder", "::new"), concat!("add_", "child(")] {
                assert!(!source.contains(made), "{made}");
            }
        }
    }
}
