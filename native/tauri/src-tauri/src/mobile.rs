//! The phone remote's lifecycle in the window (C# WinUI/MainWindow.MobileRemote.cs:18-93: start, the 250 ms tick, wake and network
//! events) over mighty-remote's `relay::host::RelayHost`, `identity::Identity` and `push::hub::PushHub`. Owner: W8-A.
//!
//! - Start (main.rs, after the run service and delegation): nothing in a smoke (C# returns for `SmokeTest`; the smoke's phone step runs
//!   the in-process fake relay itself). Without [`Host::may_reach_relay`] (`--allow-state-write`) nothing is loaded, written or dialled
//!   — the profile's host token would supersede a running C# app's relay socket (4409) — and `host.log` and the Settings box say
//!   verification mode. With it: the identity loaded (made at first use), the relay host made with the phone router as its handler
//!   (`phone::handler`, W8-B; a 404 router until it exists) behind `remote::paired::DesktopRouter` (a computer paired through the relay
//!   reaches the sharing host's mirror, never a phone route), the push hub beside it, the saved `mobileRemote` settings applied (the link
//!   dials only when they are on), and the 250 ms tick started.
//! - The tick (its own task, never the bridge's): `push.observe` over the panes as the phone sees them, then — while a phone is
//!   connected — `phone::take_notifications` → `relay.notify`.
//! - Wake and address changes reach the relay host through the platform's `Network` port (the host follows them while it is on).
//! - Run service events (host.rs's change hook, [`push_note`] / [`noted`]) tell the push hub what moved.
//! - Never a real relay in a test or the smoke; no URL is ever logged.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mighty_core::model::MobileRemoteSettings;
use mighty_platform::Platform;
use mighty_remote::identity::Identity;
use mighty_remote::push::hub::{PushHub, PushSeams, PushSource};
use mighty_remote::push::planner::{self, PushPane};
use mighty_remote::relay::connection::{MobileDevice, MobileReply, RequestHandler};
use mighty_remote::relay::host::{ApprovalRequest, Approver, RelayHost, RelayOptions, StatusSubscription};
use mighty_run::service::ServiceEvent;
use tauri::Manager;

use crate::host::{Host, lock};

/// How often the panes are looked at for pushes and phone notifications (C#'s `mobileNotify` timer).
const TICK: Duration = Duration::from_millis(250);

/// What the Settings box keeps between its builds (C#'s closure state in `BuildMobileRemoteSection`).
#[derive(Default)]
pub struct BoxState {
    /// The relay field as typed (None: as saved).
    pub typed: Option<String>,
    /// The pairing code shown in the clear.
    pub shows_key: bool,
    /// The legacy apps' key shown in the clear (고급, while they are allowed).
    pub shows_legacy_key: bool,
    /// The last QR drawn: its link and its PNG as a data URL with its side in pixels.
    pub qr: Option<(String, String, f64)>,
}

/// The phone remote's state, managed by the app.
#[derive(Default)]
pub struct MobileState {
    relay: Mutex<Option<Arc<RelayHost>>>,
    push: Mutex<Option<Arc<PushHub>>>,
    /// The relay was not started because the app may not reach it (verification mode).
    verification: AtomicBool,
    tick: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
    status: Mutex<Option<StatusSubscription>>,
    /// One settings change at a time (C#'s `writing`).
    writing: tokio::sync::Mutex<()>,
    notifying: AtomicBool,
    pub ui: Mutex<BoxState>,
}

fn state(host: &Host) -> Arc<MobileState> {
    if let Some(state) = host.app.try_state::<Arc<MobileState>>() {
        return state.inner().clone();
    }
    let state = Arc::new(MobileState::default());
    host.app.manage(state.clone());
    host.app.try_state::<Arc<MobileState>>().map(|s| s.inner().clone()).unwrap_or(state)
}

/// The relay host, once started.
pub fn relay(host: &Host) -> Option<Arc<RelayHost>> {
    lock(&state(host).relay).clone()
}

/// The push hub, once started (W8-B's router reaches `/m1/push/register`, `unregister` and `viewing` through it).
pub fn push(host: &Host) -> Option<Arc<PushHub>> {
    lock(&state(host).push).clone()
}

/// Whether the relay was left off because the app may not reach it (the box says verification mode).
pub fn verification(host: &Host) -> bool {
    state(host).verification.load(Ordering::SeqCst)
}

/// The Settings box's own state.
pub fn ui(host: &Host) -> Arc<MobileState> {
    state(host)
}

/// `Environment.MachineName`.
pub fn machine_name() -> String {
    std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).unwrap_or_default()
}

/// The handler until the phone router exists (W8-B's `phone::handler`): every route is not found, as the router at rest answers.
struct NoRouter;

impl RequestHandler for NoRouter {
    fn handle(
        &self,
        _: MobileDevice,
        _: String,
        _: String,
        _: Option<Box<serde_json::value::RawValue>>,
    ) -> mighty_platform::BoxFuture<'static, MobileReply> {
        Box::pin(async { MobileReply::error(404, "Mobile route not found.", None) })
    }
}

/// The phone remote's start with the app (C# `InitializeMobileRemote` from `MainWindow`'s load), after the run service and delegation.
pub fn start(host: &Arc<Host>, platform: &Platform) {
    if host.smoke.is_some() || relay(host).is_some() {
        return;
    }
    let saved = host.view().snapshot.mobile_remote.clone().unwrap_or_default();
    if !host.may_reach_relay() {
        state(host).verification.store(true, Ordering::SeqCst);
        if saved.enabled {
            host.log.line("mobile remote: verification mode (no --allow-state-write): the relay is not reached and no identity is written");
        }
        return;
    }
    let ports = crate::platform::remote_ports(platform, false);
    // Which devices are computers is kept beside the identity (its files have no member for it), so a paired computer keeps its icon.
    let roles = platform.paths.data_dir().join(mighty_platform::pairing::identity_file::FOLDER).join(mighty_remote::identity::ROLES_FILE);
    let identity = match Identity::load_or_create(ports.pairing.clone(), true) {
        Ok(identity) => Arc::new(identity.with_locale(host.locale).with_roles_file(roles)),
        Err(error) => {
            host.log
                .line(&format!("mobile remote: the pairing identity could not be read ({})", mighty_remote::identity::error_kind(&error)));
            return;
        }
    };
    let phones = crate::phone::handler(host).unwrap_or_else(|| Arc::new(NoRouter));
    // A paired computer's requests go to the sharing host (made by remote.rs, maybe later than this): the mirror with its own rules.
    let handler: Arc<dyn RequestHandler> = Arc::new(mighty_remote::remote::paired::DesktopRouter::new(
        phones,
        Arc::new(|| crate::remote::app_if_made().and_then(|app| app.host().cloned())),
    ));
    let name = machine_name();
    let options = RelayOptions {
        host_name: name.clone(),
        app_version: host.version.clone(),
        capabilities: mighty_remote::phone::router::CAPABILITIES.iter().map(|c| c.to_string()).collect(),
        // auth_ok's `direct[]` and the pairing link's `&direct=`: W8-C's direct path.
        direct_endpoints: Some({
            let weak = Arc::downgrade(host);
            Arc::new(move || weak.upgrade().map(|host| crate::direct::endpoints(&host)).unwrap_or_default())
        }),
        // The app's runtime: the network port's callbacks come on Windows' own threads.
        runtime: Some(tauri::async_runtime::handle().inner().clone()),
        // A new phone or computer is let in only once the person here allows it.
        approver: Some(approver(host)),
        ..RelayOptions::default()
    };
    let mut relay_platform = platform.clone();
    relay_platform.network = ports.network.clone();
    relay_platform.proxy = ports.proxy.clone();
    let relay = Arc::new(RelayHost::with_options(identity.clone(), saved.clone(), handler, relay_platform, options));
    let (for_push, for_switch) = (Arc::downgrade(&relay), Arc::downgrade(&relay));
    let seams = PushSeams::new(
        ports.https.clone(),
        Arc::new(move || for_push.upgrade().map(|r| r.relay_for_push()).unwrap_or_default()),
        Arc::new(move || for_switch.upgrade().is_some_and(|r| r.enabled())),
    );
    let push = PushHub::new(identity, &name, seams);
    let mobile = state(host);
    *lock(&mobile.relay) = Some(relay.clone());
    *lock(&mobile.push) = Some(push.clone());
    // The Settings box follows the relay's status (C#'s `StatusChanged += Updated`).
    let app = host.app.clone();
    *lock(&mobile.status) = Some(relay.on_status(move |_| {
        if let Some(host) = app.try_state::<Arc<Host>>() {
            let host = host.inner().clone();
            tauri::async_runtime::spawn(async move {
                if crate::native::is_open(&host, crate::settings::SURFACE) {
                    crate::native::touch(&host, crate::settings::SURFACE);
                }
            });
        }
    }));
    *lock(&mobile.tick) = Some(tauri::async_runtime::spawn(tick(host.clone(), relay.clone(), push)));
    // The direct path runs its admitted sockets through this host and follows the saved settings from now on.
    crate::direct::attach(host, relay.direct_sink());
    let host = host.clone();
    tauri::async_runtime::spawn(async move {
        if relay.apply(saved).await.is_err() {
            host.log.line("mobile remote: the saved settings could not be applied");
        } else {
            host.log.line("mobile remote: started");
        }
    });
}

/// Asks the person at this computer about a device that registers with a pairing code: the window brought forward, an OS notification
/// ("새 기기 '<name>'(휴대폰)가 연결하려 합니다") and a dialog whose default button refuses. The relay host gives it its time (60 s) and
/// drops it after, which closes the dialog; a quitting app refuses.
fn approver(host: &Arc<Host>) -> Approver {
    let weak = Arc::downgrade(host);
    Arc::new(move |request: ApprovalRequest| {
        let weak = weak.clone();
        Box::pin(async move {
            match weak.upgrade() {
                Some(host) if !host.closing() && host.smoke.is_none() => ask(&host, request).await,
                _ => false,
            }
        })
    })
}

/// [`approver`]'s question; true when allowed.
async fn ask(host: &Arc<Host>, request: ApprovalRequest) -> bool {
    use mighty_native::surface::{DefaultButton, DialogButton, DialogResult, DialogSpec};
    use mighty_native::ui::Content;
    let locale = host.locale;
    let kind = locale.get(if request.desktop { "settings.mobileRemote.approval.computer" } else { "settings.mobileRemote.approval.phone" });
    let line = locale.get_with("settings.mobileRemote.approval.body", &[("name", &request.name), ("kind", &kind)]);
    let title = locale.get("settings.mobileRemote.approval.title");
    if let Some(ports) = host.app.try_state::<crate::platform::Ports>() {
        let notifier = ports.notifier.clone();
        let notification = mighty_platform::Notification { title: title.clone(), body: line.clone(), pane_id: None };
        tauri::async_runtime::spawn(async move {
            let _ = notifier.notify(notification).await;
        });
    }
    if let Some(window) = host.app.get_webview_window(crate::host::WINDOW) {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
    let mut text = format!("{line}\n\n{}", locale.get("settings.mobileRemote.approval.explanation"));
    if request.replaces {
        text.push_str("\n\n");
        text.push_str(&locale.get("settings.mobileRemote.approval.replaces"));
    }
    let mut spec = DialogSpec::new(title, move |_| Content::Text(text.clone())).styled(None, None);
    spec.primary = Some(DialogButton::new(locale.get("settings.mobileRemote.approval.allow")));
    spec.close = Some(locale.get("settings.mobileRemote.approval.refuse"));
    spec.default_button = DefaultButton::Close;
    let dialog = crate::native::start_dialog(host, spec);
    /// The dialog closes however the question ends (answered, or dropped when its time ran out).
    struct Closes(Arc<Host>, String);
    impl Drop for Closes {
        fn drop(&mut self) {
            crate::native::close(&self.0, &self.1);
        }
    }
    let _closes = Closes(host.clone(), dialog.id.clone());
    dialog.wait().await == DialogResult::Primary
}

/// The 250 ms tick (C#'s `mobileNotify.Tick`): pushes planned on every tick (nothing while the remote is off), and while a phone is
/// connected the router's notifications sent.
async fn tick(host: Arc<Host>, relay: Arc<RelayHost>, push: Arc<PushHub>) {
    let mut interval = tokio::time::interval(TICK);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        if host.closing() {
            return;
        }
        let source = Panes(host.clone());
        let _ = push.observe(&source);
        let mobile = state(&host);
        if relay.status().connections == 0 || mobile.notifying.swap(true, Ordering::SeqCst) {
            continue;
        }
        for (scope, revision) in crate::phone::take_notifications(&host) {
            relay.notify(&scope, revision).await;
        }
        mobile.notifying.store(false, Ordering::SeqCst);
    }
}

/// The panes as the push planner sees them (C#'s `MobilePushPane.Project` over `service.Snapshot`): the run service's pending requests
/// and Antigravity cards; a confirmed lost sign-in is the login recovery's remembered request (C# `loginRetries.Requests`).
struct Panes(Arc<Host>);

/// The panes of `service` as the push planner sees them, `sign_in_lost` telling which pane's sign-in was confirmed lost.
fn project(service: &mighty_run::service::RunService, sign_in_lost: &dyn Fn(&str) -> bool) -> Vec<PushPane> {
    let snapshot = service.snapshot();
    let sessions: Vec<&mighty_core::model::RunSession> = snapshot.sessions().iter().map(|s| s.as_ref()).collect();
    let mut workspaces = snapshot.workspaces.clone().unwrap_or_default();
    // A phone held to the shared workspaces (휴대폰에서 볼 워크스페이스) is told of nothing in another.
    if let Some(allowed) =
        mighty_remote::phone::desk::phone_workspace_ids(snapshot.mobile_remote.as_ref(), crate::phone::shared_workspace_ids)
    {
        workspaces.retain(|w| allowed.iter().any(|id| id == w.id()));
    }
    planner::project(
        &sessions,
        &workspaces,
        &|id| service.pending_permissions(id).into_iter().filter(crate::styles::shows).collect(),
        &|_, _| false,
        &|id| service.antigravity_card(id),
        sign_in_lost,
    )
}

impl PushSource for Panes {
    fn panes(&self) -> Vec<PushPane> {
        let Some(runs) = self.0.runs().cloned() else { return vec![] };
        project(&runs.service, &|id| crate::accounts::has_retry(&self.0, id))
    }

    fn sign_in_lost(&self, session_id: &str) -> bool {
        crate::accounts::has_retry(&self.0, session_id)
    }
}

/// What a run service event tells the push hub (C#'s `MobilePushHub.Permission`, `Receive`, `Card`): a run that started, or something
/// that moved. Read before host.rs's change hook consumes the event.
pub enum PushNote {
    Began(String),
    Changed,
}

pub fn push_note(event: &ServiceEvent) -> Option<PushNote> {
    match event {
        ServiceEvent::Run { event, .. } if event.kind == "status" => Some(match event.status.as_deref() {
            Some("running") => PushNote::Began(event.session_id.clone()),
            _ => PushNote::Changed,
        }),
        // A shell's exit or failure is saved at once: its status moved.
        ServiceEvent::ToolPermission(_) | ServiceEvent::AntigravityCard(_) | ServiceEvent::SaveNow => Some(PushNote::Changed),
        _ => None,
    }
}

/// host.rs's change hook: the note handed to the push hub (cheap; no lock of the view).
pub fn noted(host: &Host, note: Option<PushNote>) {
    let (Some(note), Some(push)) = (note, push(host)) else { return };
    match note {
        PushNote::Began(id) => push.began(&id),
        PushNote::Changed => push.changed(),
    }
}

/// C# `Write`: the change saved and applied (the host follows it even when the save fails: access the switch shows as off is off),
/// one change at a time; nothing in a smoke or without the relay host.
pub async fn write(host: &Arc<Host>, change: impl FnOnce(MobileRemoteSettings) -> MobileRemoteSettings) -> Result<(), String> {
    let Some(relay) = relay(host).filter(|_| host.smoke.is_none()) else { return Ok(()) };
    let mobile = state(host);
    let _writing = mobile.writing.lock().await;
    let before = host.view().snapshot.mobile_remote.clone().unwrap_or_default();
    let settings = change(before.clone());
    let saved = host.view().update(|state| state.mobile_remote = Some(settings.clone()));
    let applied = relay.apply(settings.clone()).await.map_err(|error| error.to_string());
    crate::direct::apply(host);
    // The remote turned off: the firewall rule goes when nothing needs it any more, from that same click (C#'s `SwitchedOff`).
    if before.enabled && !settings.enabled {
        crate::direct::switched_off(host).await;
    }
    touch(host);
    saved?;
    applied
}

/// C#'s `ReconnectAsync` from the box's button.
pub async fn reconnect(host: &Arc<Host>) -> Result<(), String> {
    let Some(relay) = relay(host).filter(|_| host.smoke.is_none()) else { return Ok(()) };
    if !host.view().snapshot.mobile_remote.as_ref().is_some_and(|m| m.enabled) && !relay.desktops() {
        return Ok(());
    }
    relay.reconnect().await.map_err(|error| error.to_string())?;
    touch(host);
    Ok(())
}

/// C#'s `Revoke(id)` after its confirmation: with no device a new key (every phone and computer unpaired); with one, only that device
/// unpaired while it is still paired (a new key too, the others keep theirs and stay connected).
pub async fn rotate(host: &Arc<Host>, device: Option<String>) -> Result<(), String> {
    let Some(relay) = relay(host).filter(|_| host.smoke.is_none()) else { return Ok(()) };
    relay.rotate(device).await.map_err(|error| error.to_string())?;
    {
        let mobile = state(host);
        let mut ui = lock(&mobile.ui);
        ui.shows_key = false;
        ui.shows_legacy_key = false;
    }
    touch(host);
    Ok(())
}

/// The Settings surface drawn again when it is open.
pub fn touch(host: &Host) {
    if crate::native::is_open(host, crate::settings::SURFACE) {
        crate::native::touch(host, crate::settings::SURFACE);
    }
}

/// The app quits: nothing is planned, sent or tried again; the relay host disposed (every connection ended, nothing redials).
pub fn shutdown(host: &Host) {
    let mobile = state(host);
    let tick = lock(&mobile.tick).take();
    if let Some(tick) = tick {
        tick.abort();
    }
    lock(&mobile.status).take();
    let attached = lock(&mobile.relay).is_some();
    if attached {
        crate::direct::detach(host);
    }
    let push = lock(&mobile.push).take();
    if let Some(push) = push {
        push.dispose();
    }
    let relay = lock(&mobile.relay).take();
    if let Some(relay) = relay {
        tauri::async_runtime::block_on(async move {
            let _ = tokio::time::timeout(Duration::from_secs(3), relay.dispose()).await;
        });
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use mighty_core::model::{AppSnapshot, RunSession, Workspace};
    use mighty_run::service::{RunService, RunServiceOptions};

    /// Review M4: a pane whose sign-in the login recovery holds is pushed as a lost sign-in (C#'s `SignInLost = loginRetries.Requests`).
    #[test]
    fn a_pane_the_login_recovery_holds_reads_as_a_lost_sign_in() {
        let environment = Arc::new(mighty_platform::fake::FakeShellEnvironment::new(Default::default()));
        let catalog = Arc::new(mighty_run::providers::ProviderCatalog::new(
            Arc::new(mighty_platform::fake::FakeProcessHost::default()),
            environment,
            None,
        ));
        let options = RunServiceOptions {
            plugin_directory: String::new(),
            permission_card: false,
            remote: None,
            configure: None,
            admitting: None,
            binding: None,
            delegation: None,
        };
        let pane = |id: &str| Arc::new(RunSession { id: Some(id.into()), workspace_id: Some("ws".into()), ..RunSession::default() });
        let snapshot = AppSnapshot {
            workspaces: Some(vec![Workspace {
                id: Some("ws".into()),
                path: Some(std::env::temp_dir().to_string_lossy().into_owned()),
                ..Workspace::default()
            }]),
            ..AppSnapshot::default()
        };
        let (service, _events) = RunService::new(snapshot, catalog, options);
        service
            .update(|state| {
                let mut next = state.clone();
                next.sessions_mut().extend([pane("held"), pane("free")]);
                next
            })
            .unwrap();
        let panes = super::project(&service, &|id| id == "held");
        let lost: Vec<(&str, bool)> = panes.iter().map(|p| (p.session_id.as_str(), p.sign_in_lost)).collect();
        assert_eq!(lost, vec![("held", true), ("free", false)]);
    }
}
