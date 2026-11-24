//! MC-7: the preview browser — one Chromium-family browser installed here (MC-48's [`browser::installed`]), started with its window
//! on a private profile folder of this run (`<data>/preview-browser/<id>`, removed when it ends) and DevTools on a loopback port, and
//! one task that owns its DevTools connection: it takes the page's commands ([`Command`]) and reads the browser's events.
//!
//! - Each pane has at most one preview, a window of that browser. Its top-frame navigations are paused (DevTools `Fetch`) and let
//!   through only within the pane's ports ([`Allowed`]); any other is refused and said on the strip. A tab or window the page opens
//!   on an http(s) or file address no preview allows is closed.
//! - Its console errors, uncaught exceptions and failed requests since it last loaded are kept (MC-48's `PageFindings`).
//! - 요소 고르기 is DevTools' own inspect mode: the browser draws the highlight and reports the node clicked; [`element::PICK_SCRIPT`]
//!   reads it. Nothing is injected that the page could call, and no page can reach DevTools ([`browser::private_args`]).
//! - The browser ends when its last preview closes (the person closed the window, or 닫기), when the app quits, or when it goes.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use base64::Engine as _;
use mighty_delegation::verify::PageFindings;
use mighty_platform::{ChildControl, SpawnRequest};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;

use super::{Allowed, element};
use crate::host::Host;
use crate::verify::browser;
use crate::verify::cdp::Cdp;

/// How long the browser may take to open its DevTools port.
const START_LIMIT: Duration = Duration::from_secs(15);
/// How long one command's DevTools calls may take.
const CALL_LIMIT: Duration = Duration::from_secs(10);
/// The least time between two pushes of a pane's preview (a page that logs without end does not flood the page).
const PUSH_GAP: Duration = Duration::from_millis(300);

/// Which of the page's findings go to the composer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Findings {
    Console,
    Requests,
}

pub enum Command {
    Open { pane: String, url: url::Url, allowed: Allowed, reply: oneshot::Sender<Result<(), String>> },
    Pick { pane: String, on: bool, reply: oneshot::Sender<Result<(), String>> },
    Screenshot { pane: String, reply: oneshot::Sender<Result<(String, Vec<u8>), String>> },
    Findings { pane: String, which: Findings, reply: oneshot::Sender<Result<(String, String), String>> },
    Close { pane: String },
}

struct Browser {
    name: String,
    commands: mpsc::UnboundedSender<Command>,
}

static BROWSER: LazyLock<tokio::sync::Mutex<Option<Browser>>> = LazyLock::new(Default::default);
/// A started browser: its process and its profile folder.
type Process = (Arc<dyn ChildControl>, PathBuf);
/// The running browser's, for the app's quit (which cannot wait for the task).
static PROCESS: LazyLock<std::sync::Mutex<Option<Process>>> = LazyLock::new(Default::default);

/// The installed browser's name while the preview browser runs.
pub async fn running() -> Option<String> {
    BROWSER.lock().await.as_ref().filter(|b| !b.commands.is_closed()).map(|b| b.name.clone())
}

/// Hands `command` to the preview browser, starting it first when it does not run.
pub async fn send(host: &Arc<Host>, command: Command) -> Result<(), String> {
    let mut slot = BROWSER.lock().await;
    let command = match slot.as_ref() {
        Some(browser) => match browser.commands.send(command) {
            Ok(()) => return Ok(()),
            Err(mpsc::error::SendError(command)) => command,
        },
        None => command,
    };
    // Only an open starts the browser.
    if !matches!(command, Command::Open { .. }) {
        *slot = None;
        return Err(host.locale.get("preview.error.closed"));
    }
    let browser = launch(host).await?;
    let sent = browser.commands.send(command).map_err(|_| host.locale.get("preview.error.closed"));
    *slot = Some(browser);
    sent
}

/// Closes `pane`'s preview when the browser runs (never starts it).
pub async fn close(pane: String) {
    if let Some(browser) = BROWSER.lock().await.as_ref() {
        let _ = browser.commands.send(Command::Close { pane });
    }
}

/// The app quits: the browser and its profile go.
pub fn shutdown() {
    drop(BROWSER.try_lock().ok().and_then(|mut slot| slot.take()));
    let process = crate::host::lock(&PROCESS).take();
    if let Some((control, profile)) = process {
        control.kill();
        // The browser lets go of its folder a moment after it ends.
        for _ in 0..5 {
            if std::fs::remove_dir_all(&profile).is_ok() || !profile.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

/// The preview browser's profile folders.
pub fn profile_root(host: &Host) -> PathBuf {
    host.data_dir.join("preview-browser")
}

/// The browser's start: its window, its own profile, DevTools on loopback only (no `--headless`: the person sees and uses it).
pub fn launch_request(program: &Path, profile: &Path) -> SpawnRequest {
    let mut args = browser::private_args(profile);
    args.extend(["--window-size=1280,860".to_string(), "about:blank".into()]);
    SpawnRequest::new(program, args, profile)
}

async fn launch(host: &Arc<Host>) -> Result<Browser, String> {
    let l = host.locale;
    let (name, program) = browser::installed().ok_or_else(|| l.get("preview.error.noBrowser"))?;
    let root = profile_root(host);
    // A profile left behind by a run that did not end well.
    if let Ok(entries) = std::fs::read_dir(&root) {
        for entry in entries.flatten() {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
    let profile = root.join(mighty_core::wire::new_id());
    std::fs::create_dir_all(&profile).map_err(|e| e.to_string())?;
    let failed = |reason: &str| l.get_with("preview.error.startFailed", &[("reason", reason)]);
    let mut child = match host.processes.spawn(launch_request(&program, &profile)).await {
        Ok(child) => child,
        Err(error) => {
            let _ = std::fs::remove_dir_all(&profile);
            return Err(failed(&error.to_string()));
        }
    };
    let control = child.control();
    browser::drop_output(&mut child);
    let connected = async {
        let address = browser::devtools_address(&profile, START_LIMIT).await.ok_or("no DevTools port")?;
        let mut cdp = Cdp::connect(&address, START_LIMIT).await?;
        let deadline = Instant::now() + CALL_LIMIT;
        cdp.call(None, "Target.setDiscoverTargets", json!({ "discover": true }), deadline).await?;
        let targets = cdp.call(None, "Target.getTargets", json!({}), deadline).await?;
        let spare = targets["targetInfos"]
            .as_array()
            .and_then(|list| list.iter().find(|t| t["type"] == "page"))
            .and_then(|t| t["targetId"].as_str())
            .map(str::to_string);
        Ok::<_, String>((cdp, spare))
    };
    let (cdp, spare) = match connected.await {
        Ok(found) => found,
        Err(error) => {
            control.kill();
            let _ = control.close().await;
            browser::remove_profile(&profile).await;
            return Err(failed(&error));
        }
    };
    *crate::host::lock(&PROCESS) = Some((control.clone(), profile.clone()));
    let (commands, receiver) = mpsc::unbounded_channel();
    let pump = Pump {
        host: host.clone(),
        cdp,
        browser: name.clone(),
        previews: HashMap::new(),
        spare,
        dirty: HashSet::new(),
        pushed: HashMap::new(),
        ended: false,
    };
    tauri::async_runtime::spawn(pump.run(receiver, control, profile));
    Ok(Browser { name, commands })
}

/// The element picked last: its node (as long as the page keeps it) and its tag and selector.
struct Picked {
    node: i64,
    tag: String,
    selector: String,
}

struct Preview {
    target: String,
    session: String,
    main_frame: String,
    url: String,
    title: String,
    allowed: Allowed,
    findings: PageFindings,
    requests: HashMap<String, (String, String)>,
    picking: bool,
    picked: Option<Picked>,
}

struct Pump {
    host: Arc<Host>,
    cdp: Cdp,
    browser: String,
    /// By pane.
    previews: HashMap<String, Preview>,
    /// The window the browser opened with, for the first preview.
    spare: Option<String>,
    dirty: HashSet<String>,
    pushed: HashMap<String, Instant>,
    ended: bool,
}

/// Whether a navigation of a preview's tab goes on: a sub-frame's always (the page's own embeds), the top frame's only within the
/// pane's ports.
pub fn navigation_allowed(allowed: &Allowed, url: &str, top_frame: bool) -> bool {
    !top_frame || allowed.allows(url)
}

/// Whether another tab of the preview browser is closed: one on an http(s) or file address that no preview allows.
pub fn closes_other_tab(url: &str, previews: &[&Allowed]) -> bool {
    let policed = url::Url::parse(url).is_ok_and(|u| matches!(u.scheme(), "http" | "https" | "file"));
    policed && !previews.iter().any(|allowed| allowed.allows(url))
}

impl Pump {
    async fn run(mut self, mut commands: mpsc::UnboundedReceiver<Command>, control: Arc<dyn ChildControl>, profile: PathBuf) {
        loop {
            let tick = Instant::now() + PUSH_GAP;
            tokio::select! {
                command = commands.recv() => match command {
                    Some(command) => self.command(command).await,
                    None => break,
                },
                event = self.cdp.next_event(tick) => match event {
                    Some((session, method, params)) => self.event(session, &method, params).await,
                    None if self.cdp.closed() => break,
                    None => {}
                },
            }
            self.flush(false);
            if self.ended || self.cdp.closed() {
                break;
            }
        }
        // New commands go to a new browser; the ones already queued are told this one closed.
        commands.close();
        while let Ok(command) = commands.try_recv() {
            self.refuse(command);
        }
        let _ = self.cdp.call(None, "Browser.close", json!({}), Instant::now() + Duration::from_secs(2)).await;
        self.cdp.close().await;
        control.kill();
        let _ = control.close().await;
        {
            let mut process = crate::host::lock(&PROCESS);
            if process.as_ref().is_some_and(|(_, p)| *p == profile) {
                *process = None;
            }
        }
        browser::remove_profile(&profile).await;
        let panes: Vec<String> = self.previews.drain().map(|(pane, _)| pane).collect();
        for pane in panes {
            super::set_shown(&self.host, &pane, None);
        }
    }

    fn refuse(&self, command: Command) {
        let closed = || self.host.locale.get("preview.error.closed");
        match command {
            Command::Open { reply, .. } | Command::Pick { reply, .. } => drop(reply.send(Err(closed()))),
            Command::Screenshot { reply, .. } => drop(reply.send(Err(closed()))),
            Command::Findings { reply, .. } => drop(reply.send(Err(closed()))),
            Command::Close { .. } => {}
        }
    }

    // ── the page's commands ──────────────────────────────────────────────────────────────────────────────────────────────

    async fn command(&mut self, command: Command) {
        match command {
            Command::Open { pane, url, allowed, reply } => {
                let result = self.open(&pane, &url, allowed).await;
                let _ = reply.send(result);
            }
            Command::Pick { pane, on, reply } => {
                let result = self.pick(&pane, on).await;
                let _ = reply.send(result);
            }
            Command::Screenshot { pane, reply } => {
                let result = self.screenshot(&pane).await;
                let _ = reply.send(result);
            }
            Command::Findings { pane, which, reply } => {
                let _ = reply.send(self.findings(&pane, which));
            }
            Command::Close { pane } => {
                if let Some(preview) = self.previews.remove(&pane) {
                    let _ = self.cdp.send(None, "Target.closeTarget", json!({ "targetId": preview.target })).await;
                    super::set_shown(&self.host, &pane, None);
                }
                self.ended = self.previews.is_empty();
            }
        }
    }

    async fn open(&mut self, pane: &str, url: &url::Url, allowed: Allowed) -> Result<(), String> {
        let deadline = Instant::now() + CALL_LIMIT;
        if let Some(preview) = self.previews.get_mut(pane) {
            preview.allowed = allowed;
            let (session, target) = (preview.session.clone(), preview.target.clone());
            self.cdp.send(Some(&session), "Page.navigate", json!({ "url": url.as_str() })).await?;
            let _ = self.cdp.call(None, "Target.activateTarget", json!({ "targetId": target }), deadline).await;
            self.dirty.insert(pane.to_string());
            return Ok(());
        }
        let (target, session) = match self.spare.take() {
            Some(spare) => match self.attach(&spare, deadline).await {
                Ok(session) => (spare, session),
                Err(_) => self.new_window(deadline).await?,
            },
            None => self.new_window(deadline).await?,
        };
        let s = Some(session.as_str());
        let prepared = async {
            for method in ["Runtime.enable", "Log.enable", "Network.enable", "Page.enable", "DOM.enable", "Overlay.enable"] {
                self.cdp.call(s, method, json!({}), deadline).await?;
            }
            let patterns = json!({ "patterns": [{ "urlPattern": "*", "resourceType": "Document", "requestStage": "Request" }] });
            self.cdp.call(s, "Fetch.enable", patterns, deadline).await?;
            let tree = self.cdp.call(s, "Page.getFrameTree", json!({}), deadline).await?;
            Ok::<_, String>(tree["frameTree"]["frame"]["id"].as_str().unwrap_or(&target).to_string())
        };
        let main_frame = match prepared.await {
            Ok(frame) => frame,
            Err(error) => {
                let _ = self.cdp.send(None, "Target.closeTarget", json!({ "targetId": target })).await;
                return Err(error);
            }
        };
        self.previews.insert(
            pane.to_string(),
            Preview {
                target: target.clone(),
                session: session.clone(),
                main_frame,
                url: url.to_string(),
                title: String::new(),
                allowed,
                findings: PageFindings { url: url.to_string(), ..PageFindings::default() },
                requests: HashMap::new(),
                picking: false,
                picked: None,
            },
        );
        self.cdp.send(Some(&session), "Page.navigate", json!({ "url": url.as_str() })).await?;
        let _ = self.cdp.call(None, "Target.activateTarget", json!({ "targetId": target }), deadline).await;
        self.dirty.insert(pane.to_string());
        Ok(())
    }

    async fn attach(&mut self, target: &str, deadline: Instant) -> Result<String, String> {
        let attached = self.cdp.call(None, "Target.attachToTarget", json!({ "targetId": target, "flatten": true }), deadline).await?;
        attached["sessionId"].as_str().map(str::to_string).ok_or_else(|| "no session".to_string())
    }

    async fn new_window(&mut self, deadline: Instant) -> Result<(String, String), String> {
        let created = self.cdp.call(None, "Target.createTarget", json!({ "url": "about:blank", "newWindow": true }), deadline).await?;
        let target = created["targetId"].as_str().ok_or("no window")?.to_string();
        let session = self.attach(&target, deadline).await?;
        Ok((target, session))
    }

    fn preview(&self, pane: &str) -> Result<&Preview, String> {
        self.previews.get(pane).ok_or_else(|| self.host.locale.get("preview.error.notOpen"))
    }

    async fn pick(&mut self, pane: &str, on: bool) -> Result<(), String> {
        let deadline = Instant::now() + CALL_LIMIT;
        let (session, target) = {
            let preview = self.preview(pane)?;
            (preview.session.clone(), preview.target.clone())
        };
        let params = if on {
            json!({ "mode": "searchForNode", "highlightConfig": highlight() })
        } else {
            json!({ "mode": "none", "highlightConfig": {} })
        };
        self.cdp.call(Some(&session), "Overlay.setInspectMode", params, deadline).await?;
        if on {
            let _ = self.cdp.call(None, "Target.activateTarget", json!({ "targetId": target }), deadline).await;
            let _ = self.cdp.call(Some(&session), "Page.bringToFront", json!({}), deadline).await;
        }
        if let Some(preview) = self.previews.get_mut(pane) {
            preview.picking = on;
        }
        self.dirty.insert(pane.to_string());
        Ok(())
    }

    /// The node clicked in inspect mode: read, its description to the pane's composer, the app forward on that pane.
    async fn picked(&mut self, pane: &str, node: i64) {
        let deadline = Instant::now() + CALL_LIMIT;
        let Some(session) = self.previews.get(pane).map(|p| p.session.clone()) else { return };
        let s = Some(session.as_str());
        let _ = self.cdp.call(s, "Overlay.setInspectMode", json!({ "mode": "none", "highlightConfig": {} }), deadline).await;
        let read = async {
            let object = self.cdp.call(s, "DOM.resolveNode", json!({ "backendNodeId": node }), deadline).await?;
            let object = object["object"]["objectId"].as_str().ok_or("no node")?.to_string();
            let answer = self
                .cdp
                .call(
                    s,
                    "Runtime.callFunctionOn",
                    json!({ "objectId": object, "functionDeclaration": element::PICK_SCRIPT, "returnByValue": true }),
                    deadline,
                )
                .await;
            let _ = self.cdp.send(s, "Runtime.releaseObject", json!({ "objectId": object })).await;
            let value = answer?;
            let text = value["result"]["value"].as_str().ok_or("the element could not be read")?;
            serde_json::from_str::<element::ElementPick>(text).map_err(|e| e.to_string())
        };
        let read = read.await;
        let Some(preview) = self.previews.get_mut(pane) else { return };
        preview.picking = false;
        self.dirty.insert(pane.to_string());
        match read {
            Ok(pick) if pick.error.is_none() => {
                preview.picked = Some(Picked { node, tag: pick.tag.clone(), selector: pick.selector.clone() });
                let files = vec![(element::file_name(&pick), element::describe(&pick).into_bytes())];
                let (host, pane) = (self.host.clone(), pane.to_string());
                tauri::async_runtime::spawn(async move {
                    match crate::composer::add_files(&host, &pane, files).await {
                        Ok(()) => crate::notifications::focus(&host, &pane),
                        Err(error) => super::note(&host, &pane, Some(error)),
                    }
                });
            }
            Ok(pick) => super::note(&self.host, pane, pick.error),
            Err(error) => super::note(&self.host, pane, Some(error)),
        }
    }

    async fn screenshot(&mut self, pane: &str) -> Result<(String, Vec<u8>), String> {
        let deadline = Instant::now() + CALL_LIMIT;
        let (session, picked) = {
            let preview = self.preview(pane)?;
            (preview.session.clone(), preview.picked.as_ref().map(|p| (p.node, p.tag.clone())))
        };
        let s = Some(session.as_str());
        let mut params = json!({ "format": "png", "captureBeyondViewport": false });
        if let Some((node, _)) = &picked {
            let locale = self.host.locale;
            let gone = || locale.get("preview.error.elementGone");
            let object = self.cdp.call(s, "DOM.resolveNode", json!({ "backendNodeId": node }), deadline).await.map_err(|_| gone())?;
            let object = object["object"]["objectId"].as_str().ok_or_else(gone)?.to_string();
            let answer = self
                .cdp
                .call(
                    s,
                    "Runtime.callFunctionOn",
                    json!({ "objectId": object, "functionDeclaration": element::RECT_SCRIPT, "returnByValue": true }),
                    deadline,
                )
                .await;
            let _ = self.cdp.send(s, "Runtime.releaseObject", json!({ "objectId": object })).await;
            let rect = answer?["result"]["value"].as_array().cloned().unwrap_or_default();
            let n = |i: usize| rect.get(i).and_then(Value::as_f64).unwrap_or(f64::NAN);
            let Some((x, y, width, height)) = element::clip(n(0), n(1), n(2), n(3)) else {
                if let Some(preview) = self.previews.get_mut(pane) {
                    preview.picked = None;
                    self.dirty.insert(pane.to_string());
                }
                return Err(gone());
            };
            params = json!({
                "format": "png",
                "captureBeyondViewport": true,
                "clip": { "x": x, "y": y, "width": width, "height": height, "scale": 1 },
            });
        }
        let tag = picked.as_ref().map(|(_, tag)| tag.as_str());
        let shot = self.cdp.call(s, "Page.captureScreenshot", params.clone(), deadline).await?;
        let bytes = decode(&shot)?;
        if bytes.len() <= mighty_composer::attachments::MAXIMUM_FILE_BYTES {
            return Ok((element::shot_name(tag, "png"), bytes));
        }
        // Too large for an attachment as a PNG: a JPEG.
        params["format"] = json!("jpeg");
        params["quality"] = json!(70);
        let shot = self.cdp.call(s, "Page.captureScreenshot", params, deadline).await?;
        Ok((element::shot_name(tag, "jpg"), decode(&shot)?))
    }

    fn findings(&self, pane: &str, which: Findings) -> Result<(String, String), String> {
        let preview = self.preview(pane)?;
        let (name, text) = match which {
            Findings::Console => ("preview-console.txt", element::console_text(&preview.findings)),
            Findings::Requests => ("preview-requests.txt", element::requests_text(&preview.findings)),
        };
        text.map(|text| (name.to_string(), text)).ok_or_else(|| self.host.locale.get("preview.error.nothing"))
    }

    // ── the browser's events ─────────────────────────────────────────────────────────────────────────────────────────────

    fn pane_of_session(&self, session: &str) -> Option<String> {
        self.previews.iter().find(|(_, p)| p.session == session).map(|(pane, _)| pane.clone())
    }

    fn pane_of_target(&self, target: &str) -> Option<String> {
        self.previews.iter().find(|(_, p)| p.target == target).map(|(pane, _)| pane.clone())
    }

    async fn event(&mut self, session: Option<String>, method: &str, params: Value) {
        match method {
            "Target.targetDestroyed" | "Target.targetCrashed" => {
                let target = params["targetId"].as_str().unwrap_or_default();
                if self.spare.as_deref() == Some(target) {
                    self.spare = None;
                }
                if let Some(pane) = self.pane_of_target(target) {
                    self.previews.remove(&pane);
                    super::set_shown(&self.host, &pane, None);
                    self.ended = self.previews.is_empty();
                }
                return;
            }
            "Target.detachedFromTarget" => {
                if let Some(pane) = params["sessionId"].as_str().and_then(|s| self.pane_of_session(s)) {
                    self.previews.remove(&pane);
                    super::set_shown(&self.host, &pane, None);
                    self.ended = self.previews.is_empty();
                }
                return;
            }
            "Target.targetCreated" | "Target.targetInfoChanged" => {
                self.target_info(&params["targetInfo"]).await;
                return;
            }
            _ => {}
        }
        let Some(pane) = session.as_deref().and_then(|s| self.pane_of_session(s)) else { return };
        match method {
            "Fetch.requestPaused" => {
                let Some(preview) = self.previews.get(&pane) else { return };
                let url = params["request"]["url"].as_str().unwrap_or_default().to_string();
                let top = params["frameId"].as_str() == Some(preview.main_frame.as_str());
                let request = json!({ "requestId": params["requestId"] });
                let s = Some(preview.session.clone());
                if navigation_allowed(&preview.allowed, &url, top) {
                    let _ = self.cdp.send(s.as_deref(), "Fetch.continueRequest", request).await;
                } else {
                    let mut failed = request;
                    failed["errorReason"] = json!("BlockedByClient");
                    let _ = self.cdp.send(s.as_deref(), "Fetch.failRequest", failed).await;
                    let words = self.host.locale.get_with("preview.note.blocked", &[("url", &element_url(&url))]);
                    super::note(&self.host, &pane, Some(words));
                }
            }
            "Page.frameNavigated" if params["frame"]["parentId"].is_null() => {
                let Some(preview) = self.previews.get_mut(&pane) else { return };
                let url = params["frame"]["url"].as_str().unwrap_or_default().to_string();
                preview.findings = PageFindings { url: url.clone(), ..PageFindings::default() };
                preview.requests.clear();
                preview.picked = None;
                preview.url = url;
                self.dirty.insert(pane);
            }
            "Page.navigatedWithinDocument" => {
                let Some(preview) = self.previews.get_mut(&pane) else { return };
                if params["frameId"].as_str() == Some(preview.main_frame.as_str()) {
                    preview.url = params["url"].as_str().unwrap_or_default().to_string();
                    self.dirty.insert(pane);
                }
            }
            "Overlay.inspectNodeRequested" => {
                if let Some(node) = params["backendNodeId"].as_i64() {
                    self.picked(&pane, node).await;
                }
            }
            "Overlay.inspectModeCanceled" => {
                if let Some(preview) = self.previews.get_mut(&pane) {
                    preview.picking = false;
                    self.dirty.insert(pane);
                }
            }
            _ => {
                let Some(preview) = self.previews.get_mut(&pane) else { return };
                let before = counts(&preview.findings);
                preview.findings.observe(method, &params, &mut preview.requests);
                if counts(&preview.findings) != before {
                    self.dirty.insert(pane);
                }
            }
        }
    }

    /// A tab's address or title changed: a preview's is shown; another one off every preview's ports is closed.
    async fn target_info(&mut self, info: &Value) {
        if info["type"] != "page" {
            return;
        }
        let target = info["targetId"].as_str().unwrap_or_default();
        let url = info["url"].as_str().unwrap_or_default();
        if let Some(pane) = self.pane_of_target(target) {
            if let Some(preview) = self.previews.get_mut(&pane) {
                preview.title = info["title"].as_str().unwrap_or_default().chars().take(200).collect();
                self.dirty.insert(pane);
            }
            return;
        }
        if self.spare.as_deref() == Some(target) {
            return;
        }
        let allowed: Vec<&Allowed> = self.previews.values().map(|p| &p.allowed).collect();
        if closes_other_tab(url, &allowed) {
            let _ = self.cdp.send(None, "Target.closeTarget", json!({ "targetId": target })).await;
            let opener = info["openerId"].as_str().and_then(|o| self.pane_of_target(o));
            if let Some(pane) = opener {
                let words = self.host.locale.get_with("preview.note.blocked", &[("url", &element_url(url))]);
                super::note(&self.host, &pane, Some(words));
            }
        }
    }

    /// The panes whose preview changed, pushed (at most every [`PUSH_GAP`] each unless `now`).
    fn flush(&mut self, now: bool) {
        let due: Vec<String> =
            self.dirty.iter().filter(|pane| now || self.pushed.get(*pane).is_none_or(|at| at.elapsed() >= PUSH_GAP)).cloned().collect();
        for pane in due {
            self.dirty.remove(&pane);
            self.pushed.insert(pane.clone(), Instant::now());
            let Some(preview) = self.previews.get(&pane) else { continue };
            let shown = json!({
                "url": preview.url,
                "title": preview.title,
                "browser": self.browser,
                "picking": preview.picking,
                "picked": preview.picked.as_ref().map(|p| json!({ "tag": p.tag, "selector": p.selector })),
                "consoleErrors": preview.findings.console_errors.len() + preview.findings.exceptions.len(),
                "failedRequests": preview.findings.failed_requests.len(),
            });
            super::set_shown(&self.host, &pane, Some(shown));
        }
    }
}

fn counts(findings: &PageFindings) -> (usize, usize, usize) {
    (findings.console_errors.len(), findings.exceptions.len(), findings.failed_requests.len())
}

/// An address as the strip says it: at most 200 characters, no control character.
fn element_url(url: &str) -> String {
    url.chars().filter(|c| !c.is_control()).take(200).collect()
}

fn decode(shot: &Value) -> Result<Vec<u8>, String> {
    let data = shot["data"].as_str().ok_or("no screenshot")?;
    base64::engine::general_purpose::STANDARD.decode(data).map_err(|e| e.to_string())
}

/// The inspect mode's highlight (DevTools' own colours: content, padding, border, margin).
fn highlight() -> Value {
    json!({
        "showInfo": true,
        "showStyles": true,
        "showExtensionLines": false,
        "contentColor": { "r": 111, "g": 168, "b": 220, "a": 0.66 },
        "paddingColor": { "r": 147, "g": 196, "b": 125, "a": 0.55 },
        "borderColor": { "r": 255, "g": 229, "b": 153, "a": 0.66 },
        "marginColor": { "r": 246, "g": 178, "b": 107, "a": 0.66 },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn worktree() -> Allowed {
        Allowed::new(Some((41000, 10)), Vec::new())
    }

    #[test]
    fn the_preview_browser_is_another_program_on_a_private_profile_no_page_can_drive() {
        let request =
            launch_request(Path::new("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"), Path::new("/data/preview-browser/x"));
        assert_eq!(request.program, PathBuf::from("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"));
        assert_ne!(Some(request.program.clone()), std::env::current_exe().ok(), "never the app's own webview");
        assert!(request.args.iter().any(|a| a == "--user-data-dir=/data/preview-browser/x"), "its own profile, not the person's");
        assert!(request.args.contains(&"--remote-debugging-address=127.0.0.1".to_string()));
        assert!(request.args.contains(&"--remote-debugging-port=0".to_string()));
        assert!(request.args.contains(&"--disable-extensions".to_string()));
        assert!(!request.args.iter().any(|a| a.starts_with("--headless")), "the person sees the window");
        for refused in ["--remote-allow-origins", "--load-extension", "--disable-web-security", "--allow-file-access-from-files"] {
            assert!(!request.args.iter().any(|a| a.starts_with(refused)), "{refused}: a page could reach DevTools or the disk");
        }
        assert_eq!(request.args.last().map(String::as_str), Some("about:blank"), "it opens on nothing; the preview navigates");
    }

    #[test]
    fn a_previews_top_frame_stays_within_the_panes_ports() {
        let allowed = worktree();
        assert!(navigation_allowed(&allowed, "http://localhost:41003/login", true));
        assert!(navigation_allowed(&allowed, "http://127.0.0.1:41009/", true));
        for refused in [
            "https://example.com/",
            "http://localhost:5173/",
            "http://localhost:41010/",
            "file:///etc/passwd",
            "http://user:pw@localhost:41000/",
        ] {
            assert!(!navigation_allowed(&allowed, refused, true), "{refused}");
        }
        assert!(navigation_allowed(&allowed, "https://www.youtube.com/embed/x", false), "a sub-frame is the page's own");
    }

    #[test]
    fn another_tab_off_every_previews_ports_is_closed() {
        let a = worktree();
        let b = Allowed::new(None, vec!["http://localhost:5173/".into()]);
        assert!(!closes_other_tab("http://localhost:41001/popup", &[&a, &b]));
        assert!(!closes_other_tab("http://localhost:5173/x", &[&a, &b]));
        assert!(closes_other_tab("https://example.com/", &[&a, &b]));
        assert!(closes_other_tab("file:///Users/me/.ssh/id_rsa", &[&a]));
        assert!(!closes_other_tab("about:blank", &[&a]), "a new tab is left alone until it goes somewhere");
        assert!(!closes_other_tab("chrome://newtab/", &[]));
        assert!(!closes_other_tab("devtools://devtools/bundled/inspector.html", &[]));
    }
}
