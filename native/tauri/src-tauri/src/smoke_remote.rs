//! The remote step's remote workspaces part (C# WinUI/MainWindow.RemoteWorkspacesSmoke.cs; live key `remoteWorkspaces`), second of
//! `Stage::Remote`: the Settings page's half of remote workspaces with nothing outside this process. The app's own sharing host shares
//! the smoke's local workspace on loopback port 0 (the seam only a smoke sets, in code) and the app's own client connects to it, its key
//! in memory; the page is read before sharing, while sharing and once connected. No clipboard, no firewall, no Tailscale state, no
//! other network: the page's copy and firewall buttons are never pressed, and the firewall buttons must be off. Everything it adds is
//! taken away again. The remote pane's runs over that link are mighty-remote's loopback and desktop checks. Owner: W8-D.

use std::sync::Arc;
use std::time::{Duration, Instant};

use mighty_native::surface::{SurfaceModel, Surfaces};
use mighty_native::ui::Ui;
use mighty_native::view::{Place, Section, View};
use mighty_remote::remote::features;
use mighty_remote::remote::wire::RemoteConnectionStatus;
use serde_json::{Value, json};

use crate::host::{Host, lock};

/// The checks of its report, as C# names them in the step's result.
const KEYS: [&str; 3] = ["settingsPage", "sharing", "connected"];

/// The page's box, read as the page would get it.
struct RemoteBox(Arc<Host>);

impl SurfaceModel for RemoteBox {
    fn view(&mut self, ui: &mut Ui) -> View {
        let body = crate::settings::remote::remote_workspaces(&self.0, ui);
        View::Settings {
            title: String::new(),
            close: String::new(),
            nav: vec![],
            selected: "remote".into(),
            sections: vec![Section { title: None, beta: false, node: Some(ui.section_body(body)) }],
            width: 1.0,
            height: 1.0,
        }
    }
}

/// The page read now, its nodes by automation id.
struct Page {
    surfaces: Surfaces,
    env: mighty_native::ui::Env,
}

impl Page {
    const VIEW: &'static str = "smoke-remote-workspaces";

    fn new(host: &Arc<Host>) -> Page {
        let surfaces = Surfaces::new();
        surfaces.open(Self::VIEW, Place::Surface, Box::new(RemoteBox(host.clone())));
        Page { surfaces, env: crate::native::env(host) }
    }

    fn read(&self) -> Value {
        self.surfaces.forget_sent();
        self.surfaces.render(Self::VIEW, &self.env).and_then(|push| serde_json::from_str(&push).ok()).unwrap_or(Value::Null)
    }

    /// The node shown with automation id `aid` (none when it is not shown).
    fn node(&self, aid: &str) -> Option<Value> {
        let read = self.read();
        let mut all = vec![];
        objects(&read, &mut all);
        all.into_iter().find(|n| n.get("aid").and_then(Value::as_str) == Some(aid)).cloned()
    }

    /// The words shown under `aid`.
    fn words(&self, aid: &str) -> Option<String> {
        let node = self.node(aid)?;
        let mut all = vec![];
        objects(&node, &mut all);
        Some(all.iter().filter_map(|n| n.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join(" "))
    }

    /// Whether the control under `aid` is shown and takes input.
    fn enabled(&self, aid: &str) -> Option<bool> {
        let node = self.node(aid)?;
        let mut all = vec![];
        objects(&node, &mut all);
        Some(!all.iter().any(|n| n.get("disabled") == Some(&Value::Bool(true))))
    }

    /// Every text the page shows.
    fn texts(&self) -> Vec<String> {
        let read = self.read();
        let mut all = vec![];
        objects(&read, &mut all);
        all.iter().filter_map(|n| n.get("text").and_then(Value::as_str).map(str::to_string)).collect()
    }
}

/// Every object in `value`, depth first.
fn objects<'a>(value: &'a Value, out: &mut Vec<&'a Value>) {
    match value {
        Value::Object(map) => {
            out.push(value);
            map.values().for_each(|v| objects(v, out));
        }
        Value::Array(items) => items.iter().for_each(|v| objects(v, out)),
        _ => {}
    }
}

fn must(holds: bool, why: impl FnOnce() -> String) -> Result<(), String> {
    if holds { Ok(()) } else { Err(format!("remoteWorkspaces: {}", why())) }
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

/// Starts the remote workspaces part; false when it does not run (then the smoke goes on to the phone part). It reports once through
/// `Smoke::probed`.
pub fn start(host: &Arc<Host>) -> bool {
    if host.smoke.is_none() {
        return false;
    }
    host.log.line("smoke: remoteWorkspaces part");
    let host = host.clone();
    tauri::async_runtime::spawn(async move {
        let report = run(&host).await;
        if let Some(smoke) = &host.smoke {
            smoke.probed(&host, report);
        }
    });
    true
}

/// The part's checks from its report: each of C#'s keys, and `passed` (the window left as it was).
pub fn checks(report: &Value) -> Vec<(&'static str, bool)> {
    let names = ["remoteWorkspacesSettingsPage", "remoteWorkspacesSharing", "remoteWorkspacesConnected"];
    let mut checks: Vec<(&'static str, bool)> =
        KEYS.iter().zip(names).map(|(key, name)| (name, report.get(*key) == Some(&Value::Bool(true)))).collect();
    checks.push(("remoteWorkspaces", report.get("passed") == Some(&Value::Bool(true))));
    checks
}

/// `RunRemoteWorkspacesSmoke(local)`: the checks, then the cleanup in order (each step on its own), then the window as it found it.
async fn run(host: &Arc<Host>) -> Value {
    let mut checks = serde_json::Map::new();
    let app = crate::remote::app(host);
    let saved = host.view().snapshot.clone();
    let mut made: Option<String> = None;
    let outcome = checked(host, &app, &mut checks, &mut made).await;
    let mut cleanup = vec![];
    if let Some(sharing) = app.host().cloned()
        && sharing.state().enabled
    {
        sharing.stop().await;
        if sharing.state().enabled {
            cleanup.push("stop sharing: still sharing".to_string());
        }
    }
    if let Some(id) = made {
        app.client.remove(&id).await;
    }
    {
        let mut page = lock(&app.page);
        page.error = None;
        page.sharing_shown = false;
    }
    let after = async {
        must(cleanup.is_empty(), || format!("cleaning up failed: {}", cleanup.join(" | ")))?;
        until(
            10,
            || crate::remote::host_runs(host) == 0,
            || format!("the host's run count never went back to 0: {}", crate::remote::host_runs(host)),
        )
        .await?;
        let now = host.view().snapshot.clone();
        let sharing = app.host().is_some_and(|h| h.state().enabled);
        let connections = app.client.connections().len();
        let remote = now.workspaces().iter().filter(|w| w.remote.is_some()).count();
        must(!sharing && connections == 0 && remote == 0 && now.active_workspace_id == saved.active_workspace_id, || {
            format!(
                "the window must be left as it was; sharing {sharing}, connections {connections}, remote workspaces {remote}, front {:?} (was {:?})",
                now.active_workspace_id, saved.active_workspace_id
            )
        })
    };
    let left = after.await;
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

async fn checked(
    host: &Arc<Host>,
    app: &Arc<crate::remote::RemoteApp>,
    checks: &mut serde_json::Map<String, Value>,
    made: &mut Option<String>,
) -> Result<(), String> {
    let locale = host.locale;
    let sharing = app.host().cloned().ok_or("remoteWorkspaces: remote::start made no sharing host")?;
    let client = app.client.clone();
    let (local, front_is_local) = {
        let view = host.view();
        let snapshot = &view.snapshot;
        let usable = |id: &str| {
            snapshot
                .workspace(id)
                .filter(|w| features::local_path(Some(w)).is_some_and(|p| std::path::Path::new(p).is_dir()))
                .map(|w| w.id().to_string())
        };
        let front = snapshot.active_workspace_id.as_deref().and_then(usable);
        let any = snapshot.workspaces().iter().find_map(|w| usable(w.id()));
        (front.clone().or(any), front.is_some())
    };
    let local = local.ok_or("remoteWorkspaces: the smoke's profile has no local workspace with a folder")?;
    must(!sharing.state().enabled && client.connections().is_empty(), || {
        format!(
            "a fresh profile starts with sharing off and no saved connection; sharing {}, connections {}",
            sharing.state().enabled,
            client.connections().len()
        )
    })?;

    let page = Page::new(host);
    let off = locale.get("settings.remote.sharingOff");
    until(
        10,
        || page.words("settings-remote-share-status").as_deref() == Some(off.as_str()),
        || {
            format!(
                "before sharing the page must say sharing is off (the smoke's fixed Tailscale reading); got {:?}",
                page.words("settings-remote-share-status")
            )
        },
    )
    .await?;
    let start = page.enabled("settings-remote-start-sharing");
    must(start == Some(front_is_local), || {
        format!("the start button must be on with the local workspace in front picked; on {start:?}, front local {front_is_local}")
    })?;
    must(page.enabled("settings-remote-connect") == Some(false), || "connect must wait for an address and a key".into())?;
    let (add, remove) = (page.enabled("settings-remote-firewall-add"), page.enabled("settings-remote-firewall-remove"));
    must(add == Some(false) && remove != Some(true), || {
        format!("the firewall buttons must be off in a smoke run (no netsh, no UAC prompt); add {add:?}, remove {remove:?}")
    })?;
    checks.insert("settingsPage".into(), json!(true));

    // Sharing on the loopback seam, port chosen by the system; commands wait for the full-access switch.
    let state =
        sharing.start(std::slice::from_ref(&local), 0).await.map_err(|e| format!("remoteWorkspaces: sharing did not start: {e}"))?;
    let port = state.port.unwrap_or(0);
    let address = format!("127.0.0.1:{port}");
    let key = state.key.clone().unwrap_or_default();
    must(
        state.enabled
            && !state.allows_full_access
            && port > 0
            && key.len() == 43
            && state.workspace_ids == [local.clone()]
            && state.addresses == [address.clone()],
        || format!("sharing must start on 127.0.0.1 with full access off and a new key: {state:?}"),
    )?;
    must(sharing.set_allows_full_access(true).allows_full_access, || "the full-access switch did not take".into())?;
    let on = locale.get_with("settings.remote.sharingOn", &[("count", "1")]);
    let status = page.words("settings-remote-share-status");
    must(status.as_deref() == Some(on.as_str()), || format!("the page must say one workspace is shared; got {status:?}"))?;
    let shown = page.words("settings-remote-key-text").unwrap_or_default();
    must(shown == "••••••••••••••••" && !shown.contains(&key), || {
        format!("the key must show masked until asked; got {} characters", shown.chars().count())
    })?;
    let switch = page.node("settings-remote-full-access");
    must(switch.as_ref().and_then(|s| s.get("on")) == Some(&Value::Bool(true)), || "the full-access switch must read on".into())?;
    must(page.texts().contains(&address), || format!("the page must list {address}"))?;
    must(page.node("settings-remote-copy-key").is_some() && page.node("settings-remote-copy-address").is_some(), || {
        "the page must offer the key's and the address's copy".into()
    })?;
    must(page.node("settings-remote-copy-key-note").is_none(), || "nothing was copied, so no copy note shows".into())?;
    checks.insert("sharing".into(), json!(true));

    // The client reaches the host (a first /info reads this PC's CLIs, which may outlast one request on a cold runner).
    let mut info = None;
    let mut refused = String::new();
    for _ in 0..2 {
        match client.connect("", &address, &key).await {
            Ok(connected) => {
                info = Some(connected);
                break;
            }
            Err(error) => refused = error.message(),
        }
    }
    let connection = info.ok_or_else(|| format!("remoteWorkspaces: the client could not reach the host at {address}: {refused}"))?;
    *made = Some(connection.id.clone());
    let platform = connection.runtime.as_ref().map(|r| r.platform.clone()).unwrap_or_default();
    let expected = if cfg!(windows) { "win32" } else { "darwin" };
    must(
        connection.status == RemoteConnectionStatus::Connected
            && platform == expected
            && connection.host_name.as_deref().is_some_and(|h| !h.trim().is_empty())
            && connection.shared().iter().any(|s| s.id == local),
        || format!("the client must be connected to a host sharing the workspace; {connection:?}"),
    )?;
    must(page.node(&format!("settings-remote-connection-{}", connection.id)).is_some(), || "the page must list the connection".into())?;
    must(page.node(&format!("settings-remote-open-{local}")).is_some(), || "the page must offer the shared workspace's Open".into())?;
    checks.insert("connected".into(), json!(true));
    Ok(())
}
