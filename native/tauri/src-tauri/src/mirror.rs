//! The remote mirror in the host (C# `WinUI\MainWindow.RemoteMirror.cs`, `Core\DesktopService.Mirror.cs`, the router's
//! `PendingRequests`/`SubmitInlineAsync`, from `main` 0.4.61, over mighty-remote's `remote::mirror`; plan §3 W9-J). Owner: W9-J.
//!
//! - The client side: one [`MirrorDesk`] over the remote client and the run service, its mirrored workspaces and panes in the state (never
//!   saved: the store leaves them out), the run service's sends, stops and answers of a mirrored pane going to its host
//!   (`RunService::set_mirror`). A connection that comes up starts its mirror; in a smoke the desk follows connections only from the
//!   mirror's own step on (the remote step before it leaves nothing behind).
//! - The host side: this PC's shared panes handed to the sharing host ([`DeskMirrorHost`], read as they are and driven through the phone's
//!   router). The routes answer only while sharing, which starts only where the app may listen (`Host::may_listen`; loopback port 0 in a
//!   smoke).
//! - The window: the servers section (`serverConnect`, `serverDisconnect`, `ShellModel.servers`), the mirror flags of the sidebar's
//!   workspaces, a mirrored pane's strip (`PaneInfo.mirror`), the status bar's words (`Status.mirror`), Settings' 보기, a new pane in a
//!   mirror workspace made on its host, a mirrored pane's close on its host.

use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;
#[cfg(feature = "dev-harness")]
use std::time::Instant;

use mighty_composer::pending::SubmitReceipt;
use mighty_core::model::{AppSnapshot, MirrorReference, RunSession};
use mighty_core::shell::{MirrorStrip, RemoteServer, ShellModel};
use mighty_remote::remote::mirror::desk::{MirrorDesk, MirrorStage, Spawn};
use mighty_remote::remote::mirror::desktop_host::DeskMirrorHost;
use mighty_remote::remote::mirror::{MirrorQueued, protocol, support};
use mighty_remote::remote::wire::RemoteConnectionStatus;
use mighty_run::permissions::ToolPermissionRequest;
use serde_json::Value;
#[cfg(feature = "dev-harness")]
use serde_json::json;

use crate::host::{Host, Registry, lock};

static DESK: Mutex<Option<MirrorDesk>> = Mutex::new(None);

/// The desk once [`start`] made it.
fn desk() -> Option<MirrorDesk> {
    lock(&DESK).clone()
}

/// The app's state as the mirror reads and changes it: the run service's own copy (never the page's, so the desk never waits on the view's
/// lock), the page's copy taken again after each change. Every call comes from the desk's tasks or a page method's, never with the view's
/// lock held.
struct HostStage {
    host: Weak<Host>,
}

impl MirrorStage for HostStage {
    fn snapshot(&self) -> AppSnapshot {
        self.host.upgrade().and_then(|h| h.runs().map(|r| r.service.snapshot())).unwrap_or_default()
    }

    fn change(&self, change: &mut dyn FnMut(&AppSnapshot) -> Option<AppSnapshot>) -> bool {
        let Some(host) = self.host.upgrade().filter(|h| !h.closing()) else { return false };
        let Some(runs) = host.runs().cloned() else { return false };
        if !runs.service.mirror_update(change) {
            return false;
        }
        host.view().sync();
        host.schedule();
        if host.writes() && host.smoke.is_none() {
            crate::shared_memory::cache_mirrored_snapshot(&host, desk());
        }
        true
    }

    fn permission(&self, request: ToolPermissionRequest) {
        if let Some(runs) = self.host.upgrade().and_then(|h| h.runs().cloned()) {
            runs.service.mirror_permission(request);
        }
    }

    fn changed(&self) {
        // Off the caller's task: the composers (a mirrored pane's block), the frame model and Settings' 보기 are read again.
        let Some(host) = self.host.upgrade().filter(|h| !h.closing()) else { return };
        tauri::async_runtime::spawn(async move {
            host.panes_changed();
            host.schedule();
            crate::native::touch(&host, crate::settings::SURFACE);
        });
    }

    fn failed(&self, pane_id: &str, words: &str) {
        let _ = pane_id;
        let Some(host) = self.host.upgrade().filter(|h| !h.closing()) else { return };
        let words = words.to_string();
        tauri::async_runtime::spawn(async move {
            host.view().error = Some(words);
            host.schedule();
        });
    }
}

/// The page methods this part answers: the sidebar server menu's 연결 (Settings' 새로고침, then the mirror) and 연결 끊기.
pub fn register(methods: &mut Registry) {
    methods.add("serverConnect", |host: Arc<Host>, args: Value| async move {
        if let (Some(id), Some(desk)) = (args.get("id").and_then(Value::as_str).map(str::to_string), desk()) {
            crate::remote::action(&host, async move { desk.connect(&id).await });
        }
        Ok(Value::Null)
    });
    methods.add("serverDisconnect", |host: Arc<Host>, args: Value| async move {
        if let Some(id) = args.get("id").and_then(Value::as_str).map(str::to_string) {
            let client = crate::remote::app(&host).client.clone();
            crate::remote::action(&host, async move {
                client.disconnect(&id).await;
                Ok(())
            });
        }
        Ok(Value::Null)
    });
}

/// After remote workspaces (main.rs): the desk over the client and the run service, and this PC's panes handed to the sharing host.
pub fn start(host: &Arc<Host>, platform: &mighty_platform::Platform) {
    let _ = platform;
    let app = crate::remote::app(host);
    let stage = Arc::new(HostStage { host: Arc::downgrade(host) });
    let spawn: Spawn = Arc::new(|future| {
        tauri::async_runtime::spawn(future);
    });
    let made = MirrorDesk::new(app.client.clone(), stage, spawn);
    if host.smoke.is_none() {
        made.attach();
    }
    if let Some(runs) = host.runs() {
        runs.service.set_mirror(Arc::new(made.clone()));
    }
    *lock(&DESK) = Some(made);
    let Some(sharing) = app.host() else { return };
    let (snapshot_of, router_of, queued_of, card_of) =
        (Arc::downgrade(host), Arc::downgrade(host), Arc::downgrade(host), Arc::downgrade(host));
    sharing.set_mirror(Some(Arc::new(DeskMirrorHost::new(
        Arc::new(move || snapshot_of.upgrade().and_then(|h| h.runs().map(|r| r.service.snapshot())).unwrap_or_default()),
        // None while the window closes: every action is answered as unavailable.
        Arc::new(move || router_of.upgrade().filter(|h| !h.closing()).map(|h| crate::phone::router(&h))),
        Arc::new(move |id: &str| {
            let Some(host) = queued_of.upgrade() else { return vec![] };
            let view = host.view();
            view.panes
                .get(id)
                .map(|pane| pane.queue.items().iter().map(|item| MirrorQueued { id: item.id.clone(), text: item.text.clone() }).collect())
                .unwrap_or_default()
        }),
        Arc::new(move |id: &str| {
            card_of.upgrade().and_then(|h| h.runs().map(|r| r.service.antigravity_card(id).is_some())).unwrap_or(false)
        }),
    ))));
}

/// At quit (main.rs), with the other W8 shutdowns: every poll ends, and the sharing host gives this PC's panes back.
pub fn shutdown(host: &Arc<Host>) {
    if let Some(desk) = desk() {
        desk.dispose();
    }
    if let Some(sharing) = crate::remote::app(host).host() {
        sharing.set_mirror(None);
    }
}

/// `IsMirrorWorkspace(workspaceId)` on a state the caller holds.
pub fn is_mirror_workspace(snapshot: &AppSnapshot, workspace_id: &str) -> bool {
    snapshot.workspace(workspace_id).and_then(|w| w.remote.as_ref()).is_some_and(|r| r.mirror)
}

/// Settings' 보기: whether the connection's host may have the mirror (it never answered the mirror's list 404).
pub fn supported(connection_id: &str) -> bool {
    desk().is_some_and(|d| d.link(connection_id).is_none_or(|l| !l.unsupported))
}

/// `MirrorBlockedReason(mirror)`: a mirrored pane waits for its computer's connection.
pub fn blocked_reason(mirror: &MirrorReference) -> Option<String> {
    desk().and_then(|d| d.blocked_reason(mirror))
}

/// `ShowMirrorWorkspace(connectionId, hostWorkspaceId)`, Settings' 보기: the shared workspace as the mirror lists it, chosen once it shows
/// (within five seconds). False when the host turns out not to have the mirror (an older Mighty Code): the caller opens it as this PC's own
/// remote workspace instead, as 열기 did.
pub async fn show_workspace(host: &Arc<Host>, connection_id: &str, host_workspace_id: &str) -> bool {
    let Some(desk) = desk() else { return false };
    let unsupported = |d: &MirrorDesk| d.link(connection_id).is_some_and(|l| l.unsupported);
    if unsupported(&desk) {
        return false;
    }
    let id = support::client_id(connection_id, host_workspace_id);
    desk.start(connection_id);
    crate::settings::close(host);
    host.view().shows_dashboard = false;
    for _ in 0..50 {
        if host.view().snapshot.workspace(&id).is_some() {
            let _ = crate::frame::select_workspace(&mut host.view(), &id);
            return true;
        }
        if unsupported(&desk) {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    true
}

/// A mirror workspace's new pane (C# `AddPane` over `CreateMirrorPaneAsync`): made on its computer, made the active one once the next list
/// shows it; a refusal is said in the error line.
pub fn create_pane(host: &Arc<Host>, workspace_id: String, kind: String, provider: String) {
    let Some(desk) = desk() else { return };
    let host = host.clone();
    tauri::async_runtime::spawn(async move {
        match desk.create_pane(&workspace_id, &kind, &provider).await {
            Ok(Some(made)) => {
                let _ = crate::frame::activate_pane(&mut host.view(), &made);
            }
            Ok(None) => {}
            Err(words) => host.view().error = Some(words),
        }
        host.schedule();
    });
}

/// `PaneView.SubmitMirror` (M/AppStore+RemoteMirror.swift `submitMirror`): a mirrored pane's send, the composer's text and files as the
/// host pane's next request (queued there while it runs, never in this pane's own queue). The text and files leave the composer at once; a
/// pane whose computer is not connected keeps them, and a refusal puts them back with the host's words. None for any other pane (the
/// composer sends it as its own).
pub async fn submit(host: &Arc<Host>, id: &str) -> Option<Result<SubmitReceipt, String>> {
    let mirror = host.view().snapshot.session(id).and_then(|s| s.mirror.clone())?;
    let desk = desk()?;
    let (typed, files) = {
        let mut view = host.view();
        let Ok(ui) = crate::panes::ui(&mut view, id) else { return Some(Ok(SubmitReceipt::new("none"))) };
        let typed = ui.view.draft.clone();
        if typed.trim().is_empty() && ui.view.attachments.is_empty() || ui.view.starting {
            return Some(Ok(SubmitReceipt::new("none")));
        }
        if let Some(reason) = desk.blocked_reason(&mirror) {
            view.error = Some(reason);
            return Some(Ok(SubmitReceipt::new("blocked")));
        }
        let ui = crate::panes::ui(&mut view, id).ok()?;
        ui.view.starting = true;
        ui.submission += 1;
        ui.view.draft.clear();
        let files = std::mem::take(&mut ui.view.attachments);
        ui.dirty = true;
        (typed, files)
    };
    let sent = desk.submit(id, typed.trim(), files.clone()).await;
    let mut view = host.view();
    let alive = crate::panes::alive(&view, id);
    let empty = view.panes.get(id).is_some_and(|ui| ui.view.draft.is_empty());
    if let Some(ui) = view.panes.get_mut(id) {
        ui.view.starting = false;
        ui.dirty = true;
        if sent.is_err() && alive {
            if ui.view.draft.is_empty() {
                ui.view.draft = typed;
            }
            let kept: std::collections::HashSet<String> = ui.view.attachments.iter().map(|a| a.id.clone()).collect();
            let back: Vec<_> = files.into_iter().filter(|f| !kept.contains(&f.id)).collect();
            ui.view.attachments.splice(0..0, back);
        }
    }
    Some(match sent {
        Ok(outcome) => {
            if alive && empty {
                drop(view);
                if let Some(runs) = host.runs() {
                    let _ = runs.service.update_draft(id, "");
                }
                host.view().sync();
            }
            let word = if matches!(outcome.as_str(), "started" | "steered" | "queued") { outcome } else { "started".into() };
            Ok(SubmitReceipt::new(&word))
        }
        Err(words) => {
            view.error = Some(words);
            Ok(SubmitReceipt::new("notStarted"))
        }
    })
}

/// `CloseSessionAsync` of a mirrored pane: closed on its computer, as the phone's close does; it leaves here at once. False for any other
/// pane (the caller closes it as its own).
pub async fn close_pane(host: &Arc<Host>, id: &str) -> Result<bool, String> {
    let mirrored = host.runs().is_some_and(|r| r.service.snapshot().session(id).is_some_and(|s| s.mirror.is_some()));
    if !mirrored {
        return Ok(false);
    }
    let desk = desk().ok_or_else(|| host.locale.get("remote.mirror.error.unavailable"))?;
    desk.close_pane(id).await?;
    Ok(true)
}

/// `MirrorState(connectionId)`: a server's state words and its dot's tone (M/RemoteMirrorViews.swift `RemoteMirrorText.state`).
fn state_words(
    host: &Host,
    desk: Option<&MirrorDesk>,
    connection: Option<RemoteConnectionStatus>,
    connection_id: &str,
) -> (String, &'static str) {
    let l = host.locale;
    let Some(status) = connection else { return (l.get("remote.mirror.status.removed"), "idle") };
    if desk.and_then(|d| d.link(connection_id)).is_some_and(|link| link.phase == "reconnecting") {
        return (l.get("remote.mirror.status.reconnecting"), "wait");
    }
    match status {
        RemoteConnectionStatus::Connected => (l.get("remote.mirror.status.live"), "done"),
        RemoteConnectionStatus::Saved => (l.get("remote.mirror.status.saved"), "idle"),
        RemoteConnectionStatus::Disconnected => (l.get("remote.mirror.status.off"), "idle"),
    }
}

/// One server the sidebar lists, before its words.
struct Server {
    id: String,
    name: String,
    address: Option<String>,
    connected: bool,
    detail: Option<String>,
}

/// The remote mirror's parts of the frame model (C# `WebRemoteServers`, `WebMirrorStrip`, `MirrorStatusText`), laid over the model the
/// state made. Read with the view's lock held: it reads only the client's and the desk's own state.
pub fn frame(host: &Host, snapshot: &AppSnapshot, model: &mut ShellModel) {
    let l = host.locale;
    let desk = desk();
    let client = crate::remote::app_if_made().map(|app| app.client.clone());
    let connections = client.as_ref().map(|c| c.connections()).unwrap_or_default();
    let status_of = |id: &str| connections.iter().find(|c| c.id == id).map(|c| c.status);
    let running_on = |host_name: &str, connection_id: &str| {
        let (text, tone) = state_words(host, desk.as_ref(), status_of(connection_id), connection_id);
        (l.get_with("remote.mirror.runningOn", &[("host", host_name), ("state", &text)]), tone)
    };
    // Every saved connection, then any connection a remote workspace still names after it was removed.
    let mut servers: Vec<Server> = connections
        .iter()
        .map(|c| Server {
            id: c.id.clone(),
            name: c.name.clone(),
            address: Some(c.address.clone()),
            connected: c.status == RemoteConnectionStatus::Connected,
            detail: c.detail.clone(),
        })
        .collect();
    for workspace in snapshot.workspaces() {
        let Some(remote) = &workspace.remote else { continue };
        let id = remote.connection_id.clone().unwrap_or_default();
        if !servers.iter().any(|s| s.id == id) {
            let name = remote.host_name.clone().unwrap_or_default();
            servers.push(Server { id, name, address: None, connected: false, detail: None });
        }
    }
    model.servers = servers
        .into_iter()
        .map(|Server { id, name, address, connected, detail }| {
            let (text, tone) = state_words(host, desk.as_ref(), status_of(&id), &id);
            let line = address.iter().cloned().chain(std::iter::once(text.clone())).collect::<Vec<_>>().join(" · ");
            let error = desk.as_ref().and_then(|d| d.link(&id)).and_then(|link| link.error);
            RemoteServer {
                title: l.get_with("remote.mirror.server", &[("name", &name)]),
                line,
                state: text,
                tone,
                connected,
                known: connections.iter().any(|c| c.id == id),
                help: error.or(detail).unwrap_or_else(|| l.get_with("remote.mirror.serverHelp", &[("name", &name)])),
                id,
            }
        })
        .collect();
    for pane in &mut model.panes {
        let Some(session) = snapshot.session(&pane.id) else { continue };
        pane.mirror = strip(host, desk.as_ref(), session, &running_on);
    }
    let active = snapshot.active_workspace_id.as_deref().and_then(|id| snapshot.workspace(id));
    if let Some(remote) = active.and_then(|w| w.remote.as_ref()).filter(|r| r.mirror) {
        let (text, _) = running_on(remote.host_name.as_deref().unwrap_or(""), remote.connection_id.as_deref().unwrap_or(""));
        let words = l.get_with("remote.mirror.server", &[("name", &text)]);
        model.status.machine = words.clone();
        model.status.mirror = Some(words);
    }
}

/// `WebMirrorStrip(session)`: a mirrored pane's strip (M/RemoteMirrorViews.swift `MirrorPaneBanner`): its words, the host pane's branch and
/// its queue; None for any other pane.
fn strip(
    host: &Host,
    desk: Option<&MirrorDesk>,
    session: &RunSession,
    running_on: &dyn Fn(&str, &str) -> (String, &'static str),
) -> Option<MirrorStrip> {
    let mirror = session.mirror.as_ref()?;
    let l = host.locale;
    let (text, tone) = running_on(&mirror.host_name, &mirror.connection_id);
    let counts = desk.and_then(|d| d.counts(session.id())).unwrap_or_default();
    Some(MirrorStrip {
        badge: l.get("remote.mirror.paneBadge"),
        state: text,
        tone,
        branch: counts.branch.as_ref().map(|branch| match &counts.base {
            Some(base) => format!("{branch} ← {base}"),
            None => branch.clone(),
        }),
        queued: (counts.queued > 0).then(|| l.get_with("remote.mirror.queued", &[("count", &counts.queued.to_string())])),
        queued_help: counts.queued_texts.as_ref().filter(|t| !t.is_empty()).map(|t| t.join("\n")),
        help: l.get_with("remote.mirror.bannerHelp", &[("host", &mirror.host_name), ("count", &protocol::MAXIMUM_ENTRIES.to_string())]),
    })
}

// ── the smoke's step ─────────────────────────────────────────────────────────────────────────────────────────────────────

#[cfg(feature = "dev-harness")]
mod smoke_step {
    use super::*;

    /// The checks of the step's report.
    const KEYS: [(&str, &str); 4] =
        [("listed", "mirrorListed"), ("frame", "mirrorFrame"), ("reconnected", "mirrorReconnected"), ("cleaned", "mirrorCleaned")];

    fn must(holds: bool, why: impl FnOnce() -> String) -> Result<(), String> {
        if holds { Ok(()) } else { Err(format!("mirror: {}", why())) }
    }

    /// Polls `test` every 50 ms for up to `seconds`; Err with what was seen when it never held.
    async fn until(seconds: u64, mut test: impl FnMut() -> bool, seen: impl FnOnce() -> String) -> Result<(), String> {
        let end = Instant::now() + Duration::from_secs(seconds);
        while Instant::now() < end {
            if test() {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        must(test(), seen)
    }

    /// The smoke's mirror step (smoke.rs `Stage::Mirror`, after the remote step; on loopback inside this process): the app's own sharing host
    /// shares the smoke's local workspace on loopback port 0, the app's own client connects to it, and the mirror lists the workspace and its
    /// agent panes, the frame model carries the server, the mirror flags and the strips, a disconnect stops the mirror and a connect starts it
    /// again; everything it adds is taken away again. True when it started; it reports once through `Smoke::probed`.
    pub fn smoke_start(host: &Arc<Host>) -> bool {
        if host.smoke.is_none() {
            return false;
        }
        let Some(desk) = desk() else { return false };
        host.log.line("smoke: mirror step");
        desk.attach();
        let host = host.clone();
        tauri::async_runtime::spawn(async move {
            let report = smoke_run(&host, &desk).await;
            if let Some(smoke) = &host.smoke {
                smoke.probed(&host, report);
            }
        });
        true
    }

    /// The step's checks from its report: each of its keys, and `mirror` (the window left as it was).
    pub fn smoke_checks(report: &Value) -> Vec<(&'static str, bool)> {
        let mut checks: Vec<(&'static str, bool)> =
            KEYS.iter().map(|(key, name)| (*name, report.get(*key) == Some(&Value::Bool(true)))).collect();
        checks.push(("mirror", report.get("passed") == Some(&Value::Bool(true))));
        checks
    }

    async fn smoke_run(host: &Arc<Host>, desk: &MirrorDesk) -> Value {
        let mut checks = serde_json::Map::new();
        let app = crate::remote::app(host);
        let saved = host.view().snapshot.active_workspace_id.clone();
        let mut made: Option<String> = None;
        let outcome = smoke_checked(host, desk, &app, &mut checks, &mut made).await;
        if let Some(sharing) = app.host().cloned()
            && sharing.state().enabled
        {
            sharing.stop().await;
        }
        if let Some(id) = &made {
            app.client.remove(id).await;
        }
        let left = async {
            until(
                10,
                || {
                    let view = host.view();
                    !view.snapshot.workspaces().iter().any(|w| w.remote.is_some())
                        && !view.snapshot.sessions().iter().any(|s| s.mirror.is_some())
                },
                || "the mirror's workspaces and panes must go with the connection".into(),
            )
            .await?;
            if host.view().snapshot.active_workspace_id != saved
                && let Some(front) = &saved
            {
                let _ = crate::frame::select_workspace(&mut host.view(), front);
            }
            let sharing = app.host().is_some_and(|h| h.state().enabled);
            let connections = app.client.connections().len();
            must(!sharing && connections == 0 && host.view().snapshot.active_workspace_id == saved, || {
                format!("the window must be left as it was; sharing {sharing}, connections {connections}")
            })
        };
        let left = left.await;
        if left.is_ok() {
            checks.insert("cleaned".into(), json!(true));
        }
        match (outcome, left) {
            (Ok(()), Ok(())) => {
                checks.insert("passed".into(), json!(true));
            }
            (Err(failure), Ok(())) | (Ok(()), Err(failure)) => {
                checks.insert("error".into(), json!(failure));
            }
            (Err(failure), Err(left)) => {
                checks.insert("error".into(), json!(format!("{failure} | {left}")));
            }
        }
        Value::Object(checks)
    }

    async fn smoke_checked(
        host: &Arc<Host>,
        desk: &MirrorDesk,
        app: &Arc<crate::remote::RemoteApp>,
        checks: &mut serde_json::Map<String, Value>,
        made: &mut Option<String>,
    ) -> Result<(), String> {
        let sharing = app.host().cloned().ok_or("mirror: remote::start made no sharing host")?;
        let client = app.client.clone();
        let (local, agents) = {
            let view = host.view();
            let snapshot = &view.snapshot;
            let usable = snapshot
                .workspaces()
                .iter()
                .find(|w| mighty_remote::remote::features::local_path(Some(w)).is_some_and(|p| std::path::Path::new(p).is_dir()));
            let local = usable.map(|w| w.id().to_string()).ok_or("mirror: the smoke's profile has no local workspace with a folder")?;
            let agents =
                snapshot.sessions().iter().filter(|s| s.workspace_id() == local && s.kind() == "claude" && s.mirror.is_none()).count();
            (local, agents)
        };
        must(!sharing.state().enabled && client.connections().is_empty(), || {
            "the remote step must leave no sharing and no connection".into()
        })?;
        let state = sharing.start(std::slice::from_ref(&local), 0).await.map_err(|e| format!("mirror: sharing did not start: {e}"))?;
        let address = format!("127.0.0.1:{}", state.port.unwrap_or(0));
        let key = state.key.clone().unwrap_or_default();
        let mut connection = None;
        let mut refused = String::new();
        for _ in 0..2 {
            match client.connect("", &address, &key).await {
                Ok(info) => {
                    connection = Some(info);
                    break;
                }
                Err(error) => refused = error.message(),
            }
        }
        let connection = connection.ok_or_else(|| format!("mirror: the client could not reach the host at {address}: {refused}"))?;
        *made = Some(connection.id.clone());
        let id = connection.id.clone();
        let workspace = support::client_id(&id, &local);
        let mirrored = |host: &Host| {
            host.view().snapshot.sessions().iter().filter(|s| s.mirror.as_ref().is_some_and(|m| m.connection_id == id)).count()
        };
        until(
            15,
            || {
                host.view().snapshot.workspace(&workspace).is_some()
                    && mirrored(host) == agents
                    && desk.link(&id).is_some_and(|l| l.phase == "live")
            },
            || {
                format!(
                    "the mirror must list the shared workspace and its {agents} agent panes; panes {}, link {:?}",
                    mirrored(host),
                    desk.link(&id)
                )
            },
        )
        .await?;
        let listed = host.view().snapshot.workspace(&workspace).cloned().unwrap_or_default();
        must(listed.remote.as_ref().is_some_and(|r| r.mirror && r.connection_id.as_deref() == Some(id.as_str())), || {
            "the shared workspace must be listed as a mirror of the connection".into()
        })?;
        checks.insert("listed".into(), json!(true));

        let model = host.shell_model(&mut host.view());
        let server = model.servers.iter().find(|s| s.id == id);
        must(server.is_some_and(|s| s.connected && s.known && s.tone == "done"), || {
            format!("the sidebar must list the server, connected: {server:?}")
        })?;
        let row = model.workspaces.iter().find(|w| w.id == workspace);
        must(row.is_some_and(|w| w.mirror && w.server.as_deref() == Some(id.as_str()) && w.remote_host.is_none() && !w.local), || {
            format!("the mirror workspace must sit under its server: {row:?}")
        })?;
        let strips = model.panes.iter().filter(|p| p.mirror.is_some()).count();
        must(strips == agents, || format!("every mirrored pane must carry its strip: {strips} of {agents}"))?;
        let saved = host.view().current().without_mirrors();
        must(saved.workspace(&workspace).is_none() && !saved.sessions().iter().any(|s| s.mirror.is_some()), || {
            "the mirror must never be saved".into()
        })?;
        checks.insert("frame".into(), json!(true));

        // The server menu's 연결 끊기 stops the mirror (the panes stay), 연결 starts it again.
        client.disconnect(&id).await;
        until(
            10,
            || desk.link(&id).is_some_and(|l| l.phase == "off"),
            || format!("a disconnect must stop the mirror: {:?}", desk.link(&id)),
        )
        .await?;
        must(mirrored(host) == agents, || "the panes stay as last shown".into())?;
        desk.connect(&id).await.map_err(|e| format!("mirror: the connect failed: {e}"))?;
        until(
            10,
            || {
                desk.link(&id).is_some_and(|l| l.phase == "live")
                    && client.connection(&id).is_some_and(|c| c.status == RemoteConnectionStatus::Connected)
            },
            || format!("a connect must start the mirror again: {:?}", desk.link(&id)),
        )
        .await?;
        checks.insert("reconnected".into(), json!(true));
        Ok(())
    }
}

#[cfg(feature = "dev-harness")]
pub use smoke_step::{smoke_checks, smoke_start};

#[cfg(test)]
mod tests {
    use mighty_core::model::{RemoteWorkspaceReference, Workspace};

    use super::*;

    /// A mirror workspace is told apart from this PC's own and from a remote workspace this PC opened (its panes run there through the
    /// client's runs, not the mirror).
    #[test]
    fn only_a_mirror_workspace_is_one() {
        let reference = |mirror: bool| RemoteWorkspaceReference {
            connection_id: Some("conn".into()),
            workspace_id: Some("ws".into()),
            host_name: Some("studio-pc".into()),
            version: 2,
            mirror,
        };
        let state = AppSnapshot {
            workspaces: Some(vec![
                Workspace { id: Some("own".into()), ..Workspace::default() },
                Workspace { id: Some("opened".into()), remote: Some(reference(false)), ..Workspace::default() },
                Workspace { id: Some("mirrored".into()), remote: Some(reference(true)), ..Workspace::default() },
            ]),
            ..AppSnapshot::default()
        };
        assert!(!is_mirror_workspace(&state, "own") && !is_mirror_workspace(&state, "opened") && !is_mirror_workspace(&state, "gone"));
        assert!(is_mirror_workspace(&state, "mirrored"));
    }

    /// The step's checks: each key, and `mirror` for the whole step; a step that did not report fails them all.
    #[cfg(feature = "dev-harness")]
    #[test]
    fn the_smoke_step_reads_its_report() {
        let passed = json!({"listed": true, "frame": true, "reconnected": true, "cleaned": true, "passed": true});
        assert!(smoke_checks(&passed).iter().all(|(_, ok)| *ok));
        assert_eq!(smoke_checks(&passed).len(), 5);
        let failed = json!({"listed": true, "error": "mirror: …"});
        let checks = smoke_checks(&failed);
        assert!(checks.iter().any(|(name, ok)| *name == "mirrorListed" && *ok));
        assert!(checks.iter().any(|(name, ok)| *name == "mirror" && !*ok));
        assert!(smoke_checks(&Value::Null).iter().all(|(_, ok)| !*ok));
    }
}
