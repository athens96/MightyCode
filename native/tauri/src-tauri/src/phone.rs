//! The phone's desk (C# WinUI/MainWindow.MobileRemote.cs:456-835: `MobileRoute`, `MobileAction`, `MobileSubmit` with the fix branch's
//! S-L3 continuation revoke, run-next, run-now, the pane extras; MainWindow.MobileImages.cs) as mighty-remote's `phone::desk::PhoneDesk`
//! over the run service, `DelegationApp`, the composer queue, `ask.rs`'s targets, terminal prompts, `antigravity.rs`'s cards, the app
//! update, `workmap.rs` and `files.rs`; and the router it feeds. Owner: W8-B.
//!
//! - The router is made once ([`router`]) and kept in the app's state; [`changed`] marks it from host.rs's change hook (cheap, no view
//!   lock), [`handler`] hands it to W8-A's relay host, [`take_notifications`] reads it again for the 250 ms tick.
//! - Its uploads are `<profile>/mobile-remote/uploads` (stale `.part` files of an earlier run deleted when it is made).
//! - `MobileAction` runs each mutating route through the pane's own composer: a busy pane queues the text, an idle one starts it through
//!   delegation's send (held items go first, and then the answer carries no ids); `NotStarted` is 409 `remote.error.dropped`; a queued
//!   Antigravity continuation that could not start loses its grant and its card decides again (S-L3); the phone's close never asks, a
//!   parent with open children included.
//! - Styles (styles.rs, W9-D): the style a pane runs, its panel and older phones' Ouroboros field, `guided` and the style in `settings`.
//! - What later stages own answers as C# does without it: the app update (W9) — this build cannot update itself, so the phone gets no
//!   update state and `POST /m1/app-update` answers 409.

use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::{Arc, LazyLock, Mutex};

use indexmap::IndexMap;
use mighty_composer::ask_targets;
use mighty_composer::pending::SubmitReceipt;
use mighty_composer::queue::{self, QueueRunNowPath};
use mighty_core::app_update_view;
use mighty_core::attachment::RunAttachment;
use mighty_core::model::{AppSnapshot, PlanRecord, RunSession, RunSettings, Workspace};
use mighty_core::permission::ToolPermissionRequest;
use mighty_core::{time, wire};
use mighty_delegation::delivery::{DelegationDeliveryItem, DelegationRelease};
use mighty_files::changes::{self, ChangeSet};
use mighty_graph::result_files::StdFiles;
use mighty_graph::structure::mobile::{Built, MobileStructureCache, Naming};
use mighty_platform::BoxFuture;
use mighty_remote::phone::desk::{
    ActionAnswer, MobileAskTarget, MobileBackgroundWork, MobileDesktopAction, MobileError, MobileImageResult, MobilePaneDigest,
    MobilePaneExtras, MobileQueuedItem, PhoneDesk,
};
use mighty_remote::phone::mighty::{self, MobileOut, ResultFilesMemo, RunsInputs};
use mighty_remote::phone::router::{Router, RouterOptions};
use mighty_remote::relay::connection::RequestHandler;
#[cfg(feature = "dev-harness")]
use mighty_remote::relay::connection::{MobileDevice, MobileReply};
use mighty_run::antigravity::permissions::PermissionRequest;
use mighty_run::model::{ModelCatalog, StartRunRequest};
use mighty_run::plan_mode::PlanDecision;
use mighty_run::questionnaire::UserQuestionAnswer;
use mighty_terminal::prompt::TerminalPromptAnswer;
use mighty_workmap::WorkMapModel;
use mighty_workmap::service::WorkMapStart;
use serde_json::{Value, json};
use tauri::Manager;

use crate::host::{Host, lock};
use crate::panes;

/// What the phone's desk keeps beside the router (C#'s window fields: `mobileStructures`, each pane's `mobileResultFiles`).
#[derive(Default)]
struct Kept {
    structures: MobileStructureCache,
    result_files: HashMap<String, Arc<ResultFilesMemo>>,
}

struct PhoneState {
    router: Router,
}

/// The desk: the app's state and window as the router sees them, read through the app handle (the host owns the router's state).
struct Desk {
    app: tauri::AppHandle,
    kept: Arc<Mutex<Kept>>,
}

/// Extra `/m1/info` capabilities offered by the Tauri desktop host beyond `mighty_remote`'s C# baseline.
pub const EXTRA_CAPABILITIES: &[&str] = &["savedPrompts"];

/// Full capability list advertised by `/m1/info` and relay `auth_ok`.
pub fn host_capabilities() -> Vec<String> {
    mighty_remote::phone::router::CAPABILITIES.iter().chain(EXTRA_CAPABILITIES.iter()).map(|c| (*c).to_string()).collect()
}

/// `/m1/info`'s `platform`: this OS's `PlatformProfile::m1_platform` ("win32", "darwin").
fn m1_platform() -> &'static str {
    crate::platform::profile().m1_platform
}

fn state(host: &Host) -> Option<Arc<PhoneState>> {
    host.app.try_state::<Arc<PhoneState>>().map(|s| s.inner().clone())
}

/// Where the phone's uploads are staged: `<profile>/mobile-remote/uploads` when the app may write its profile, else (a read-only launch, any
/// smoke) a folder of this process's own, so nothing is made in the profile. The store makes it only when an upload begins.
fn uploads_folder(host: &Host) -> std::path::PathBuf {
    if host.smoke.is_some() || !host.writes() {
        std::env::temp_dir().join(format!("mighty-phone-uploads-{}", std::process::id()))
    } else {
        host.data_dir.join("mobile-remote").join("uploads")
    }
}

/// What 이 컴퓨터 공유 shares now (none while sharing is off or was never started): what a phone held to the shared workspaces sees,
/// in its routes and its pushes (mobile.rs).
pub fn shared_workspace_ids() -> Vec<String> {
    crate::remote::app_if_made().and_then(|app| app.host().map(|host| host.shared_workspace_ids())).unwrap_or_default()
}

/// The router, made the first time it is asked for.
pub fn router(host: &Arc<Host>) -> Router {
    if let Some(state) = state(host) {
        return state.router.clone();
    }
    let kept = Arc::new(Mutex::new(Kept::default()));
    let desk = Arc::new(Desk { app: host.app.clone(), kept });
    let mut options = RouterOptions::new(uploads_folder(host), m1_platform());
    let app = host.app.clone();
    options.push = Some(Arc::new(move || app.try_state::<Arc<Host>>().and_then(|h| crate::mobile::push(h.inner()))));
    let made = Arc::new(PhoneState { router: Router::new(desk, options) });
    // Another thread may have made one meanwhile: that one is the router.
    host.app.manage(made.clone());
    state(host).map(|s| s.router.clone()).unwrap_or_else(|| made.router.clone())
}

/// Something a phone may show moved (C# `MobileDesktopRouter.Changed` from `RunEventReceived`, `ToolPermissionChanged`,
/// `DelegationChanged`, `AntigravityCardChanged`): called from host.rs's change hook on every run service event. It stays cheap and never
/// takes the view's lock (the caller may hold it): a router not made yet has nothing to mark.
pub fn changed(host: &Host) {
    if let Some(state) = state(host) {
        state.router.changed();
    }
}

/// The phone's uploads still open (the app update's idle facts): the router's, 0 while none was made (never made here).
pub fn open_upload_count(host: &Host) -> usize {
    state(host).map_or(0, |state| state.router.open_upload_count())
}

/// The m1 router as a connection's handler (W8-A's relay host and direct connections).
pub fn handler(host: &Arc<Host>) -> Option<Arc<dyn RequestHandler>> {
    Some(Arc::new(router(host)))
}

/// The router's `Changed`, `Observe` then `TakeNotifications` (W8-A's 250 ms tick, C#'s `mobileNotify.Tick`): the scopes whose revision
/// moved, `state` before the sessions. Never called with the view's lock held.
pub fn take_notifications(host: &Host) -> Vec<(String, i64)> {
    let Some(state) = state(host) else { return vec![] };
    state.router.changed();
    state.router.observe();
    state.router.take_notifications()
}

/// A request routed in-process (the smoke's phone part and the delegation smoke's `phone*` keys, W8 plan D16): what a phone's
/// connection hands the router, from the smoke's own device.
#[cfg(feature = "dev-harness")]
pub async fn route(host: &Arc<Host>, method: &str, path: &str, body: Option<Value>) -> MobileReply {
    let body = body.and_then(|b| serde_json::value::RawValue::from_string(b.to_string()).ok());
    let device = MobileDevice { client_id: "smoke-phone".into(), name: "Smoke".into(), direct: false, desktop: false };
    router(host).route(device, method.into(), path.into(), body).await
}

// ── the pane as the phone reads it ─────────────────────────────────────────────────────────────────────────────────────

fn request(status: u16, message: impl Into<String>) -> MobileError {
    MobileError::request(status, message, None)
}

fn not_found_session() -> MobileError {
    request(404, "Session not found.")
}

/// `OperationCanceledException`: the window closed under the request, or the pane went.
fn cancelled() -> MobileError {
    MobileError::Failed("cancelled".into())
}

impl Desk {
    fn host(&self) -> Result<Arc<Host>, MobileError> {
        self.app.try_state::<Arc<Host>>().map(|h| h.inner().clone()).ok_or_else(cancelled)
    }

    fn snapshot_of(host: &Host) -> AppSnapshot {
        match host.runs() {
            Some(runs) => runs.service.snapshot(),
            None => host.current(),
        }
    }
}

/// `MobilePane(id)`: a local Claude pane, its view made on first use (a pane only ever watched from the phone has none yet).
fn mobile_pane(host: &Host, id: &str) -> Result<Arc<RunSession>, MobileError> {
    let mut view = host.view();
    let pane = view.snapshot.session(id).filter(|p| p.kind() == "claude").cloned().ok_or_else(not_found_session)?;
    if view.snapshot.workspace(pane.workspace_id()).is_none_or(|w| w.remote.is_some()) {
        return Err(not_found_session());
    }
    panes::ensure(&mut view, Some(id)).map_err(|_| not_found_session())?;
    Ok(pane)
}

fn running(host: &Host, id: &str) -> bool {
    host.runs().is_some_and(|r| r.service.is_session_running(id))
}

/// `MobileBusy`: the pane runs, starts or its queue starts one, or an update of its CLI holds it.
fn busy(host: &Host, id: &str) -> bool {
    let view = host.view();
    let Some(pane) = view.snapshot.session(id) else { return false };
    let flags = view.panes.get(id).is_some_and(|u| u.view.starting || u.view.queue_starting);
    flags || pane.status() == "running" || running(host, id) || crate::updates::holds(host, &view.snapshot, pane)
}

fn held(id: &str) -> Vec<DelegationDeliveryItem> {
    crate::delegation::app().map(|app| app.held_items(id)).unwrap_or_default()
}

fn capabilities(host: &Host, provider: &str) -> mighty_run::model::ProviderCapabilities {
    host.runs()
        .and_then(|r| r.runtime(provider))
        .map(|r| r.capabilities)
        .unwrap_or_else(|| mighty_run::providers::capabilities(provider, None))
}

/// The model list as the composer's pickers read it (`PaneRuntime(provider)?.ModelCatalog ?? ProviderCatalog.Fallback(provider)`).
fn picker_catalog(host: &Host, provider: &str) -> mighty_composer::runtime::ModelCatalog {
    host.runs()
        .and_then(|r| r.composer_runtime(provider))
        .map(|r| r.catalog)
        .unwrap_or_else(|| mighty_composer::runtime::ModelCatalog::fallback(provider, &host.locale))
}

fn catalog(host: &Host, provider: &str) -> ModelCatalog {
    host.runs().map(|r| r.catalog_of(provider)).unwrap_or_else(|| mighty_run::providers::fallback(provider))
}

/// `MobileOptions(pane, model, viewMode)`: the pickers the phone shows, from the same sources as the composer's menus; the styles the
/// pane may pick in that view (styles.rs: the registry, none outside the Mighty view or for a remote pane).
fn options(
    host: &Host,
    snapshot: &AppSnapshot,
    pane: &RunSession,
    model: Option<&str>,
    view_mode: Option<&str>,
) -> mighty::MobileSettingsOptions {
    let styles = crate::styles::offered(host, snapshot, pane, view_mode.unwrap_or(pane.agent_view_mode.as_deref().unwrap_or("")));
    let l = host.locale;
    let catalog = catalog(host, pane.provider());
    let caps = capabilities(host, pane.provider());
    let workspace = snapshot.workspace(pane.workspace_id());
    let registered = crate::runs::registered_models_for(pane.provider(), workspace, snapshot);
    let efforts = mighty_run::providers::efforts(pane.provider(), model.unwrap_or(pane.model()), &catalog, Some(&registered));
    mighty::MobileSettingsOptions {
        models: mighty_composer::menus::picker_options(pane, &picker_catalog(host, pane.provider()), &l)
            .into_iter()
            .map(|m| mighty::MobileOption::new(&m.value, &m.display_name))
            .collect(),
        permission_modes: caps
            .permission_modes
            .iter()
            .map(|m| mighty::MobileOption::new(m, &mighty_composer::runtime::permission_label(pane.provider(), m, &l)))
            .collect(),
        efforts: mighty::effort_options(caps.effort, &efforts, &l),
        mighty_styles: mighty_remote::phone::legacy_style::legacy_style_options(&styles),
        styles: mighty_remote::phone::legacy_style::style_options(&styles),
    }
}

/// The 보낼 대상 switch's targets for a pane and the one in use (`AskTargetsFor`, `AskTargets.Effective`).
fn ask_targets_of(host: &Host, snapshot: &AppSnapshot, pane: &RunSession) -> (Vec<ask_targets::AskTarget>, Option<String>) {
    let targets = crate::ask::targets_for(host, snapshot, pane);
    let effective = ask_targets::effective(pane.ask_target.as_deref(), &targets).map(|t| t.id.to_string());
    (targets, effective)
}

impl Desk {
    /// `MobileSnapshot(roles)`: what a pane's detail carries beside its record.
    fn snapshot_extras(&self, pane: &RunSession, roles: bool) -> MobilePaneExtras {
        let Ok(host) = self.host() else { return MobilePaneExtras::default() };
        if pane.kind() != "claude" || mobile_pane(&host, pane.id()).is_err() {
            return MobilePaneExtras::default();
        }
        let snapshot = Desk::snapshot_of(&host);
        let pane = snapshot.session(pane.id()).cloned().unwrap_or_else(|| Arc::new(pane.clone()));
        let l = host.locale;
        let editable = !busy(&host, pane.id());
        let offered = options(&host, &snapshot, &pane, None, None);
        // The style the pane really runs (styles.rs, C# `MobileActiveStyle`): its panel, and the old Ouroboros field for older phones.
        let style_id = crate::styles::active_style(&host, &pane).unwrap_or_else(|| mighty::CLI_STYLE.to_string());
        let styled = crate::styles::phone(&host, &pane, !editable);
        let style_names = crate::styles::source(&host, &pane).map(|s| s.registry);
        let title = |input: &str| style_names.as_ref().and_then(|r| mighty_styles::presentation::request_prefix(&r.styles, input));
        let settings = mighty::MobileSettings {
            editable,
            model: pane.model().into(),
            permission_mode: pane.settings.clone().unwrap_or_default().permission_mode,
            effort: offered.efforts.is_some().then(|| pane.settings.clone().unwrap_or_default().effort),
            agent_view_mode: mighty::view_mode(pane.agent_view_mode.as_deref()).into(),
            mighty_style: mighty::style(Some(&style_id)).into(),
            style_id: style_id.clone(),
            options: offered,
        };
        // A run's resultFiles are looked for in the pane's own folder, once per result (the pane's memo).
        let workspace = snapshot.workspace(pane.workspace_id()).cloned();
        let root = workspace.as_ref().filter(|w| w.remote.is_none()).map(|w| w.path().to_string());
        let memo = {
            let mut kept = lock(&self.kept);
            kept.result_files
                .entry(pane.id().to_string())
                .or_insert_with(|| {
                    Arc::new(ResultFilesMemo::new(|run, folder| {
                        let ignores_case = crate::platform::names_ignore_case(std::path::Path::new(folder));
                        mighty::look_through(run, folder, &StdFiles { ignores_case })
                    }))
                })
                .clone()
        };
        let saved = mighty::saved_runs(&pane);
        // The background runs' last output lines (tails.rs, `BackgroundOutputFor`), and for the role view the custom agents the blocks
        // name (`MobileFlowDefinitions`, read off this thread).
        let tails = crate::tails::output_for(pane.id());
        let flow = roles.then(|| {
            let alive = |id: &str| snapshot.session(id).is_some();
            let definitions =
                crate::mighty::definitions_for(&PHONE_DEFINITIONS, &host, pane.id(), &saved, root.as_deref(), alive, |host, _| {
                    // The pane's digest reads them: the phone is told at the next pass.
                    changed(host);
                });
            mighty::Roles { fan_out: pane.provider() == "claude", definitions: (*definitions).clone() }
        });
        let lookup =
            StdFiles { ignores_case: root.as_deref().is_some_and(|r| crate::platform::names_ignore_case(std::path::Path::new(r))) };
        let runs = mighty::runs(
            &saved,
            &RunsInputs {
                title: style_names.is_some().then_some(&title as &dyn Fn(&str) -> Option<String>),
                output_tails: Some(&tails),
                workspace_root: root.as_deref(),
                lookup: Some(&lookup),
                result_files: Some(&memo),
                roles: flow.as_ref(),
            },
            &l,
        );
        let mut payload = serde_json::Map::new();
        payload.insert("style".into(), json!(mighty::style(Some(&style_id))));
        payload.insert("styleId".into(), json!(style_id));
        payload.insert("runs".into(), serde_json::from_str(&runs.to_json()).unwrap_or(Value::Array(vec![])));
        if let Some(styled) = &styled {
            payload.insert("panel".into(), styled.panel.clone());
            if let Some(legacy) = &styled.ouroboros {
                payload.insert("ouroboros".into(), legacy.clone());
            }
        }
        if let Some(flow) = flow.as_ref().filter(|f| f.fan_out) {
            payload.insert("fanOut".into(), json!(true));
            if let Some(log) = mighty::flow_log(&saved, &flow.definitions, &l) {
                payload.insert("log".into(), serde_json::from_str(&log.to_json()).unwrap_or(Value::Null));
            }
        }
        let status_line = crate::status_line::input(&host, &pane).and_then(|input| input.lines).map(|lines| {
            let lines: Vec<Value> = lines
                .iter()
                .map(|line| {
                    Value::Array(
                        line.iter()
                            .map(|s| {
                                let mut run = serde_json::Map::new();
                                run.insert("text".into(), json!(s.text));
                                if let Some(color) = &s.color {
                                    run.insert("fg".into(), json!(color.to_uppercase()));
                                }
                                run.insert("bold".into(), json!(s.bold));
                                Value::Object(run)
                            })
                            .collect(),
                    )
                })
                .collect();
            json!({"lines": lines})
        });
        // The 보낼 대상 switch needs the pane's catalogue, which a pane only ever read from the phone has not scanned.
        crate::ask::refresh_catalog(&host, &snapshot, &pane, true);
        let (targets, effective) = ask_targets_of(&host, &snapshot, &pane);
        let newest = mighty_graph::support::newest_request_run(pane.graph_runs());
        let background = mighty::mobile_background_work(pane.background_work.as_ref(), newest, Some(&tails), &l)
            .map(|rows| rows.into_iter().map(MobileBackgroundWork::from).collect());
        let queued = queued_rows(&host, pane.id());
        MobilePaneExtras {
            queued,
            settings: serde_json::from_str(&settings.to_json()).ok(),
            mighty: Some(Value::Object(payload)),
            status_line,
            ask_targets: Some(targets.iter().map(|t| MobileAskTarget { id: t.id.into(), label: t.label.into() }).collect()),
            ask_target: effective,
            background_work: background,
            background_done: mighty::mobile_background_done(pane.background_work.as_ref()),
            terminal_prompt: crate::terminal::terminals(&host).ok().and_then(|t| t.prompt(pane.id())),
        }
    }
}

/// The custom agents read for the phone's role view (C# `mobileFlowDefinitions`), apart from the page's (mighty.rs).
static PHONE_DEFINITIONS: LazyLock<crate::mighty::DefinitionsCache> = LazyLock::new(crate::mighty::DefinitionsCache::new);

/// The phone's queued rows: delegation's held items first, then the queue, each with what 바로 실행 does on it (`RunNowRows`).
fn queued_rows(host: &Host, id: &str) -> Vec<MobileQueuedItem> {
    let held = held(id);
    let mut view = host.view();
    let rows = crate::composer::run_now_rows(host, &mut view, id);
    let queue = view.panes.get(id).map(|u| u.queue.items().to_vec()).unwrap_or_default();
    drop(view);
    let paths: HashMap<String, QueueRunNowPath> = rows.into_iter().collect();
    let decide = |row: &str| paths.get(row).copied().unwrap_or(QueueRunNowPath::Hidden);
    mighty_delegation::delivery::mobile_items(&held, &queue, Some(&decide)).into_iter().map(MobileQueuedItem::from).collect()
}

fn hash_of(write: impl FnOnce(&mut DefaultHasher)) -> i32 {
    let mut hash = DefaultHasher::new();
    write(&mut hash);
    hash.finish() as i32
}

// ── the actions ────────────────────────────────────────────────────────────────────────────────────────────────────────

/// MC-9: `mighty agent send` (the local control API): the phone's own send — a busy pane queues it, the pane's style and "send to"
/// choice apply, an idle pane starts through delegation's send — for a local agent pane. Its outcome (`started`, `queued`) or the
/// refusal's status and words.
pub(crate) async fn control_send(host: &Arc<Host>, id: &str, text: &str) -> Result<String, (u16, String)> {
    let refusal = |error: MobileError| match error {
        MobileError::Request { status, message, .. } => (status, message),
        MobileError::Invalid => (400, "Invalid request.".into()),
        MobileError::Changed => (409, "The pane changed; try again.".into()),
        MobileError::Unavailable => (403, "Unavailable.".into()),
        MobileError::Failed(message) => (500, message),
    };
    mobile_pane(host, id).map_err(refusal)?;
    submit(host, id, text, vec![], false, None).await.map(|receipt| receipt.outcome).map_err(refusal)
}

/// MC-9: `mighty agent new`: a new agent pane of `provider` in the local workspace `workspace_id`, as the phone's `create` makes one
/// (in its own worktree where the workspace's mode says so, or always with `worktree`); its id.
pub(crate) async fn control_create(
    host: Arc<Host>,
    workspace_id: String,
    provider: String,
    worktree: bool,
) -> Result<String, (u16, String)> {
    let created = {
        let view = host.view();
        let template = RunSession {
            id: Some(wire::new_id()),
            workspace_id: Some(workspace_id.clone()),
            provider: Some(provider.clone()),
            kind: Some("claude".into()),
            title: Some(mighty_run::providers::name(&provider).into()),
            created_at: Some(time::now_o()),
            ..RunSession::default()
        };
        mighty_core::add_pane::inherit(template, view.snapshot.sessions())
    };
    // On a task of its own, as the phone's: a dropped request never leaves a worktree without its pane.
    let making = tauri::async_runtime::spawn(crate::pane_worktree::create_without_asking(host, workspace_id, created, worktree));
    making.await.map_err(|error| (500, error.to_string()))?
}

/// `MobileSubmit(text, files, prepared, queueId)`: a phone's send, and a queued request run from the phone. Only the phone's own words
/// are wrapped for the pane's "send to" choice; a busy pane always queues; an idle one starts through delegation's send.
async fn submit(
    host: &Arc<Host>,
    id: &str,
    text: &str,
    files: Vec<RunAttachment>,
    prepared: bool,
    queue_id: Option<&str>,
) -> Result<SubmitReceipt, MobileError> {
    let l = host.locale;
    let own = !prepared && queue_id.is_none();
    if own && host.view().snapshot.session(id).is_some_and(|p| p.ask_target.is_some()) {
        crate::ask::await_targets(host, id).await;
        if !panes::alive(&host.view(), id) {
            return Err(cancelled());
        }
    }
    let (pane, asked) = {
        let view = host.view();
        let pane = view.snapshot.session(id).cloned().ok_or_else(not_found_session)?;
        if let Some(block) = crate::updates::manual_block(host, &view.snapshot, &pane) {
            return Err(request(409, block));
        }
        let asked = crate::ask::send_target(host, &view.snapshot, &pane);
        (pane, asked)
    };
    let mut text = text.to_string();
    if own && asked.is_some() {
        // An older phone may still send files: an ask carries words alone.
        if !files.is_empty() {
            return Err(request(409, l.get("composer.ask.attachmentsOff")));
        }
        text = ask_targets::wrap(&text, asked).ok_or_else(|| request(400, l.get("remote.error.nothingToSend")))?;
    }
    let mut probe = StartRunRequest::new(pane.id(), pane.workspace_id(), pane.kind(), &text);
    probe.provider = pane.provider().to_string();
    probe.model = pane.model().to_string();
    probe.settings = pane.settings.clone();
    probe.attachments = (!files.is_empty()).then(|| files.clone());
    probe.validate().map_err(|_| MobileError::Invalid)?;
    if busy(host, id) {
        // A busy pane always queues the text as its next request, whatever mode the phone sent; a full queue refuses first.
        let drained = {
            let mut view = host.view();
            let ui = panes::ui(&mut view, id).map_err(|_| not_found_session())?;
            if ui.queue.items().len() >= queue::MAXIMUM_ITEMS {
                return Err(request(409, l.get("remote.error.queueFull")));
            }
            let override_mode = queue::background::queued_override(ui.view.launches_in_plan_mode);
            ui.queue.add(&text, files, override_mode, prepared, false, &l).map_err(|e| request(409, e.to_string()))?;
            ui.dirty = true;
            true
        };
        if drained && !busy(host, id) {
            crate::composer::drain_queue(host.clone(), id.to_string());
        }
        return Ok(SubmitReceipt::new("queued"));
    }
    let submission = {
        let mut view = host.view();
        let ui = panes::ui(&mut view, id).map_err(|_| not_found_session())?;
        ui.submission += 1;
        ui.view.starting = true;
        ui.dirty = true;
        ui.submission
    };
    let outcome = start(host, id, &text, files, prepared, queue_id, submission).await;
    if let Some(ui) = host.view().panes.get_mut(id) {
        ui.view.starting = false;
        ui.dirty = true;
    }
    host.schedule();
    outcome
}

/// The start of [`submit`] once the pane is marked starting.
async fn start(
    host: &Arc<Host>,
    id: &str,
    text: &str,
    files: Vec<RunAttachment>,
    prepared: bool,
    queue_id: Option<&str>,
    submission: u64,
) -> Result<SubmitReceipt, MobileError> {
    let l = host.locale;
    // The model list as the CLI has it now comes first; the request takes the pane's model and effort after it.
    crate::models::prepare(host, id).await;
    // C# `MobileSubmit`: a styled pane's own words go as its style's command (`PrepareStyleSubmission`) unless already prepared.
    let queued_prepared =
        queue_id.is_some_and(|item| host.view().panes.get(id).is_some_and(|u| u.queue.items().iter().any(|q| q.id == item && q.prepared)));
    let text = if prepared || queued_prepared {
        text.to_string()
    } else {
        crate::styles::prepare_submission(host, id, text, !files.is_empty()).await.map_err(|e| request(409, e))?
    };
    let text = text.as_str();
    // A queued item keeps the launch decision made when it was queued; an Antigravity card's continuation carries its rules and card.
    let (override_mode, continuation) = {
        let view = host.view();
        let ui = view.panes.get(id);
        if !panes::alive(&view, id) || ui.is_none_or(|u| u.submission != submission) {
            return Err(cancelled());
        }
        let Some(ui) = ui else {
            return Err(cancelled());
        };
        match queue_id {
            Some(item) => {
                let queued = ui.queue.items().iter().find(|q| q.id == item).cloned().ok_or_else(cancelled)?;
                (queued.permission_mode_override.clone(), ui.continuations.get(item).cloned())
            }
            None => (None, None),
        }
    };
    let card = continuation.as_ref().map(|(_, card)| card.clone());
    let started = start_request(host, id, text, files, override_mode, queue_id.is_some(), continuation).await;
    let (answer, revoke) = answer_start(started, card.is_some(), &l);
    // A continuation that could not start leaves the queue with its rules, as the desktop's drain drops it (S-L3).
    if let (Some(reason), Some(card), Some(item)) = (revoke, card, queue_id) {
        if let Some(ui) = host.view().panes.get_mut(id) {
            revoke_queued(ui, item);
        }
        crate::antigravity::continuation_failed(host, id, card, &reason);
    }
    answer
}

/// What a start of the pane's request did, as the phone's answer reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::enum_variant_names, reason = "NotStarted is delegation's word")]
enum Started {
    /// The start failed with this reason.
    Failed(String),
    /// No run started (delegation's `NotStarted`, or stopped before it started).
    NotStarted,
    /// The request went out with the items delegation held, in one run.
    Released,
    /// It started at once: its user entry and graph run.
    Now(Option<String>, Option<String>),
}

/// The phone's answer to a start, and the reason a queued continuation that took it loses its grant (S-L3): only a request that started
/// at once carries its ids (natural submit B5), one that went out with held items answers without them; `NotStarted` is 409
/// `remote.error.dropped`.
fn answer_start(
    started: Started,
    continuation: bool,
    l: &mighty_core::locale::Locale,
) -> (Result<SubmitReceipt, MobileError>, Option<String>) {
    match started {
        Started::Failed(error) => (Err(request(409, error.clone())), continuation.then_some(error)),
        Started::NotStarted => (Err(request(409, l.get("remote.error.dropped"))), continuation.then(|| l.get("remote.error.startFailed"))),
        Started::Released => (Ok(SubmitReceipt::new("started")), None),
        Started::Now(entry, run) => (Ok(SubmitReceipt::with_ids("started", entry.as_deref(), run.as_deref())), None),
    }
}

/// S-L3: the queued continuation `item` leaves the queue with its rules (its grant and card go with it).
fn revoke_queued(ui: &mut panes::PaneUi, item: &str) {
    ui.queue.remove(item);
    ui.continuations.remove(item);
    ui.dirty = true;
}

/// C# `StartFromComposer` for the phone: the pane's request through delegation's send (held items first, in one run), the release it
/// answered telling whether the run was the request's own.
async fn start_request(
    host: &Arc<Host>,
    id: &str,
    text: &str,
    files: Vec<RunAttachment>,
    permission_override: Option<String>,
    queued: bool,
    continuation: Option<(mighty_run::antigravity::AllowGrant, PermissionRequest)>,
) -> Started {
    let Ok(engine) = host.engine() else { return Started::Failed("the run engine is not ready".into()) };
    if let Err(error) = crate::composer::reset_unavailable_agy_model(host, id) {
        return Started::Failed(error);
    }
    let receipt = Arc::new(Mutex::new(mighty_run::model::RunStartReceipt::default()));
    let request = {
        let view = host.view();
        let Some(pane) = view.snapshot.session(id).cloned() else { return Started::Failed("Session not found.".into()) };
        let workspace = view.snapshot.workspace(pane.workspace_id()).cloned();
        let mut request = StartRunRequest::new(pane.id(), pane.workspace_id(), pane.kind(), text);
        request.registered_models = crate::runs::registered_models_for(pane.provider(), workspace.as_ref(), &view.snapshot);
        request.model = pane.model().to_string();
        request.provider = pane.provider().to_string();
        request.settings = pane.settings.clone();
        request.resume_id = pane.resume_id.clone();
        request.attachments = (!files.is_empty()).then_some(files);
        request.permission_mode_override = permission_override.clone();
        request.receipt = Some(receipt.clone());
        if let Some((grant, card)) = continuation {
            request.antigravity_grant = Some(grant);
            request.antigravity_card = Some(card);
        }
        request
    };
    // C# `PrepareStyleRunRequest`: the style's grants and plan-mode launch, its approval read again (styles.rs); a queued item keeps
    // the launch decision made when it was queued.
    let mut request = match crate::styles::prepare_run(host, id, request).await {
        Ok(request) => request,
        Err(error) => return Started::Failed(error),
    };
    if queued {
        request.permission_mode_override = permission_override;
    }
    let ids = || {
        let made = lock(&receipt);
        Started::Now(made.entry_id.clone(), made.graph_run_id.clone())
    };
    match crate::delegation::app() {
        Some(app) => match app.send(request).await {
            Ok(DelegationRelease::NotStarted) => Started::NotStarted,
            Ok(DelegationRelease::Released { .. }) => Started::Released,
            Ok(DelegationRelease::NothingHeld) => ids(),
            Err(error) => Started::Failed(error.message().to_string()),
        },
        None => match engine.service.start(request).await {
            Ok(Some(())) => ids(),
            Ok(None) => Started::NotStarted,
            Err(error) => Started::Failed(error.message().to_string()),
        },
    }
}

/// C# `DelegatedRunBlockReason(pane)`: why a delegated pane may not run now, if it may not.
fn block_reason(host: &Host, id: &str) -> Option<String> {
    let view = host.view();
    let pane = view.snapshot.session(id).cloned()?;
    crate::delegation::run_block_reason(host, &view.snapshot, &pane)
}

/// `SteerQueuedInput(row)`: into a run that only waits on background work, the row (or the first) as a new turn.
fn steer_queued(host: &Host, id: &str, row: Option<&str>) -> bool {
    let next = {
        let view = host.view();
        let Some(pane) = view.snapshot.session(id).cloned() else { return false };
        let local = view.snapshot.workspace(pane.workspace_id()).is_some_and(|w| w.remote.is_none());
        let Some(ui) = view.panes.get(id) else { return false };
        let next = match row {
            Some(row) => ui.queue.items().iter().find(|q| q.id == row).cloned(),
            None => ui.queue.items().first().cloned(),
        };
        let Some(next) = next else { return false };
        let steers = pane.kind() == "claude"
            && next.attachments.is_empty()
            && !queue::is_continuation(&next)
            && queue::background::steers_run_next(
                pane.background_work.as_ref(),
                pane.provider() == "claude" && local,
                ui.view.stopping,
                1,
                false,
            )
            && !mighty_graph::support::notice_turn_running(pane.graph_runs());
        if !steers {
            return false;
        }
        next
    };
    let Some(runs) = host.runs() else { return false };
    if !runs.service.try_steer(id, &next.text) {
        return false;
    }
    if let Some(ui) = host.view().panes.get_mut(id) {
        ui.queue.remove(&next.id);
        ui.forget_unqueued();
        ui.dirty = true;
    }
    true
}

/// `MobileRunNext`: the items delegation holds for the pane go first, all of them in one run; with nothing held, the first queued row
/// runs (steered into a run that only waits on background work).
async fn run_next(host: &Arc<Host>, id: &str) -> Result<ActionAnswer, MobileError> {
    let l = host.locale;
    if !held(id).is_empty()
        && let Some(app) = crate::delegation::app()
    {
        if !host.delegation_live() {
            return Err(request(409, crate::setup::verification_mode()));
        }
        if let Some(blocked) = block_reason(host, id) {
            return Err(request(409, blocked));
        }
        match app.run_next(id).await {
            DelegationRelease::NotStarted => return Err(request(409, l.get("remote.error.startFailed"))),
            DelegationRelease::Released { .. } => {
                if let Some(ui) = host.view().panes.get_mut(id) {
                    ui.dirty = true;
                }
                return Ok(ActionAnswer::Value(Value::Null));
            }
            DelegationRelease::NothingHeld => {}
        }
    }
    if steer_queued(host, id, None) {
        return Ok(ActionAnswer::Value(Value::Null));
    }
    if busy(host, id) {
        return Err(request(409, "Session is still running."));
    }
    let next =
        host.view().panes.get(id).and_then(|u| u.queue.items().first().cloned()).ok_or_else(|| request(409, "The queue is empty."))?;
    submit(host, id, &next.text, next.attachments.clone(), next.prepared, Some(&next.id)).await?;
    if let Some(ui) = host.view().panes.get_mut(id) {
        ui.queue.remove(&next.id);
        ui.forget_unqueued();
        ui.dirty = true;
    }
    Ok(ActionAnswer::Value(Value::Null))
}

/// `MobileRunQueuedNow(itemId)`: what `RunNowFor` decides now, as the window's click would ("worktree" or "here"); 404 for a row no
/// longer queued, 409 `queue.runNowRefused` for one that cannot run now, `remote.error.startFailed` when a start did not happen.
async fn run_queued_now(host: &Arc<Host>, id: &str, item: &str) -> Result<ActionAnswer, MobileError> {
    let l = host.locale;
    let queued = host.view().panes.get(id).is_some_and(|u| u.queue.items().iter().any(|q| q.id == item));
    if !held(id).iter().any(|h| h.id == item) && !queued {
        return Err(request(404, l.get("remote.error.queueItemNotFound")));
    }
    let path = {
        let mut view = host.view();
        crate::composer::run_now_rows(host, &mut view, id)
            .into_iter()
            .find(|(row, _)| row == item)
            .map_or(QueueRunNowPath::Hidden, |(_, p)| p)
    };
    let refused = || request(409, l.get("queue.runNowRefused"));
    let Some((wire_word, _)) = path.wire().filter(|_| path != QueueRunNowPath::AgentMenu && path.runs()) else { return Err(refused()) };
    // Settling a blocked pane throws the whole queue away with only a log line (C# `DelegatedRunBlockReason`).
    if matches!(path, QueueRunNowPath::Here | QueueRunNowPath::Held)
        && let Some(blocked) = block_reason(host, id)
    {
        return Err(request(409, blocked));
    }
    match path {
        QueueRunNowPath::Here => {
            let row = host.view().panes.get(id).and_then(|u| u.queue.items().iter().find(|q| q.id == item).cloned()).ok_or_else(refused)?;
            if let Some(ui) = host.view().panes.get_mut(id) {
                ui.queue.move_to_front(item);
            }
            submit(host, id, &row.text, row.attachments.clone(), row.prepared, Some(item)).await?;
            if let Some(ui) = host.view().panes.get_mut(id) {
                ui.queue.remove(item);
                ui.forget_unqueued();
                ui.dirty = true;
            }
        }
        QueueRunNowPath::Steer => {
            if !steer_queued(host, id, Some(item)) {
                return Err(request(409, l.get("remote.error.startFailed")));
            }
        }
        QueueRunNowPath::Held => {
            if !host.delegation_live() {
                return Err(request(409, crate::setup::verification_mode()));
            }
            let release = match crate::delegation::app() {
                Some(app) => app.run_next(id).await,
                None => DelegationRelease::NothingHeld,
            };
            if !matches!(release, DelegationRelease::Released { .. }) {
                return Err(request(409, l.get("remote.error.startFailed")));
            }
        }
        QueueRunNowPath::Worktree => {
            let row = {
                let mut view = host.view();
                let ui = panes::ui(&mut view, id).map_err(|_| refused())?;
                let row = ui
                    .queue
                    .items()
                    .iter()
                    .find(|q| q.id == item)
                    .cloned()
                    .filter(|row| !queue::is_continuation(row))
                    .ok_or_else(refused)?;
                ui.queue.remove(item);
                ui.forget_unqueued();
                ui.dirty = true;
                row
            };
            crate::delegation::run_in_new_worktree(host, id, row.text.clone(), row.attachments.clone(), None, Some(row));
        }
        _ => return Err(refused()),
    }
    Ok(ActionAnswer::Value(json!(wire_word)))
}

/// `MobileStopQueue`: the queue cancelled, unless the turn was over and only background work ran.
fn stop_queue(host: &Host, id: &str) {
    let mut view = host.view();
    let work = view.snapshot.session(id).and_then(|p| p.background_work.clone());
    if let Some(ui) = view.panes.get_mut(id) {
        ui.submission += 1;
        ui.queue_kept_on_stop = queue::background::stop_keeps_queue(work.as_ref());
        if !ui.queue_kept_on_stop {
            ui.queue.clear();
            ui.forget_unqueued();
        }
        ui.dirty = true;
    }
}

/// `MobileSettings(body)`: the pane's model, permission mode and effort as the phone picked them, checked against what the composer
/// offers; the view is the Mighty view; the style one the pane may pick (styles.rs: 400 unknown, 409 "Style changed.").
async fn settings(host: &Host, id: &str, body: &serde_json::Map<String, Value>) -> Result<(), MobileError> {
    let l = host.locale;
    if busy(host, id) {
        return Err(request(409, "Stop the session before changing settings."));
    }
    let snapshot = Desk::snapshot_of(host);
    let pane = snapshot.session(id).cloned().ok_or_else(not_found_session)?;
    let current = pane.settings.clone().unwrap_or_default();
    let catalog = catalog(host, pane.provider());
    let caps = capabilities(host, pane.provider());
    let registered = crate::runs::registered_models_for(pane.provider(), snapshot.workspace(pane.workspace_id()), &snapshot);
    let models: Vec<String> =
        mighty_composer::menus::picker_options(&pane, &picker_catalog(host, pane.provider()), &l).into_iter().map(|m| m.value).collect();
    let efforts = |model: &str| mighty_run::providers::efforts(pane.provider(), model, &catalog, Some(&registered));
    let (model, mode, effort) = settings_choice(body, pane.model(), &current, &models, &caps.permission_modes, &efforts, &l)?;
    let (style, hash, changes) =
        crate::styles::phone_choice(host, &snapshot, &pane, body).await.map_err(|(status, message)| request(status, message))?;
    let mut view = host.view();
    let now = view.snapshot.session(id).cloned().ok_or_else(not_found_session)?;
    if busy_locked(host, &view, id)
        || now.provider() != pane.provider()
        || now.model() != pane.model()
        || now.mighty_style != pane.mighty_style
        || now.settings != pane.settings
    {
        return Err(request(409, "The pane changed."));
    }
    crate::composer::change(&mut view, id, |p| {
        let prior = p.settings.clone().unwrap_or_default();
        RunSession {
            model: Some(model.clone()),
            settings: Some(RunSettings {
                network_access: matches!(mode.as_str(), "acceptEdits" | "onRequest") && prior.network_access,
                permission_mode: mode.clone(),
                effort: effort.clone(),
                ..prior
            }),
            agent_view_mode: Some("mighty".into()),
            mighty_style: if changes { style.clone() } else { p.mighty_style.clone() },
            mighty_style_hash: if changes { hash.clone() } else { p.mighty_style_hash.clone() },
            mighty_style_since: if !changes || p.mighty_style == style { p.mighty_style_since.clone() } else { None },
            ..p.clone()
        }
    })
    .map_err(|e| request(409, e))
}

fn busy_locked(host: &Host, view: &crate::host::View, id: &str) -> bool {
    let Some(pane) = view.snapshot.session(id) else { return false };
    view.panes.get(id).is_some_and(|u| u.view.starting || u.view.queue_starting)
        || pane.status() == "running"
        || running(host, id)
        || crate::updates::holds(host, &view.snapshot, pane)
}

/// What a phone's settings request asks for, checked as C#'s `MobileSettings` checks it: only the six keys, each a string; a model the
/// picker offers, a permission mode the provider has, an effort the model takes (or default); the plain or the Mighty view (the style is
/// styles.rs's `phone_choice`). The model, permission mode and effort the pane takes.
fn settings_choice(
    body: &serde_json::Map<String, Value>,
    model_now: &str,
    settings_now: &RunSettings,
    models: &[String],
    modes: &[String],
    efforts: &dyn Fn(&str) -> Vec<String>,
    l: &mighty_core::locale::Locale,
) -> Result<(String, String, String), MobileError> {
    if body.is_empty()
        || body.iter().any(|(k, v)| {
            !matches!(k.as_str(), "model" | "permissionMode" | "effort" | "agentViewMode" | "mightyStyle" | "styleId") || !v.is_string()
        })
    {
        return Err(request(400, "Unknown setting."));
    }
    let text = |k: &str| body.get(k).and_then(Value::as_str);
    let model = text("model").unwrap_or(model_now).to_string();
    let mode = text("permissionMode").unwrap_or(&settings_now.permission_mode).to_string();
    let effort = text("effort").unwrap_or(&settings_now.effort).to_string();
    if !models.contains(&model) || !modes.contains(&mode) || (effort != "default" && !efforts(&model).contains(&effort)) {
        return Err(request(400, "Unsupported model, permission, or effort."));
    }
    if !matches!(text("agentViewMode"), None | Some("plain" | "mighty")) {
        return Err(request(400, "Unsupported view."));
    }
    let _ = l;
    Ok((model, mode, effort))
}

/// The phone's 보낼 대상 choice: a target the pane offers, or None for Claude; anything else is `remote.error.badBody`.
fn ask_target_choice(target: Option<&str>, offered: &[&str], l: &mighty_core::locale::Locale) -> Result<Option<String>, MobileError> {
    match target {
        Some(target) if !offered.contains(&target) => Err(request(400, l.get("remote.error.badBody"))),
        target => Ok(target.map(str::to_string)),
    }
}

/// `MobileUsage`: the session's model, context, tokens, cost and elapsed time, one per line.
fn usage_text(host: &Host, pane: &RunSession) -> String {
    let l = host.locale;
    let usage = pane.session_usage.as_ref();
    let mut lines = vec![format!(
        "{} · {}",
        l.get("composer.label.model"),
        usage.and_then(|u| u.model.clone()).unwrap_or_else(|| pane.model().to_string())
    )];
    if let Some(context) = usage.and_then(|u| u.context_used_tokens) {
        let window = usage.and_then(|u| u.context_window_tokens).map(mighty_run::strings::n0).unwrap_or_default();
        lines.push(format!("{} · {} / {}", l.get("composer.sessionInfo.context"), mighty_run::strings::n0(context), window));
    }
    if let Some(tokens) = usage.and_then(|u| u.total_tokens) {
        lines.push(format!("Tokens · {}", mighty_run::strings::n0(tokens)));
    }
    if let Some(cost) = usage.and_then(|u| u.cost_usd) {
        let text = format!("{cost:.4}");
        let text = text.trim_end_matches('0').trim_end_matches('.');
        lines.push(format!("{} · ${text}", l.get("composer.sessionInfo.cost")));
    }
    if let Some(timing) = pane.run_timing.as_ref() {
        lines.push(format!("{} · {}", l.get("composer.sessionInfo.elapsed"), mighty_run::timing::label(timing, None)));
    }
    lines.join("\n")
}

/// `MobileAction(request)`: a mutating route through the window's own composer.
async fn action(host: Arc<Host>, request_: MobileDesktopAction) -> Result<ActionAnswer, MobileError> {
    if host.closing() {
        return Err(cancelled());
    }
    let l = host.locale;
    let body = request_.body.as_object().cloned().unwrap_or_default();
    let text = |k: &str| body.get(k).and_then(Value::as_str).map(str::to_string);
    let id = request_.session_id.clone();
    if request_.action == "create" {
        let provider = text("provider").unwrap_or_else(|| "claude".into());
        let created = {
            let view = host.view();
            let template = RunSession {
                id: Some(wire::new_id()),
                workspace_id: Some(id.clone()),
                provider: Some(provider.clone()),
                kind: Some("claude".into()),
                title: Some(mighty_run::providers::name(&provider).into()),
                created_at: Some(time::now_o()),
                ..RunSession::default()
            };
            mighty_core::add_pane::inherit(template, view.snapshot.sessions())
        };
        // The phone cannot be asked: the workspace's mode alone decides whether the pane gets its own worktree (W9-I, C#
        // `PaneWorktreePolicy.DecideWithoutAsking`). On a task of its own, so a dropped request never leaves a worktree without its pane.
        let making = tauri::async_runtime::spawn(crate::pane_worktree::phone_create(host.clone(), id.clone(), created));
        let made = match making.await {
            Ok(Ok(made)) => made,
            Ok(Err((status, message))) => return Err(request(status, message)),
            Err(error) => return Err(MobileError::Failed(error.to_string())),
        };
        return Ok(ActionAnswer::Value(json!(made)));
    }
    let pane = mobile_pane(&host, &id)?;
    let answer = match request_.action.as_str() {
        "submit" => ActionAnswer::Receipt(
            submit(&host, &id, &text("text").unwrap_or_default(), request_.attachments.unwrap_or_default(), false, None).await?,
        ),
        "prompt-send" => {
            let prompt_id = text("promptId").unwrap_or_default();
            let expanded =
                crate::saved_prompts::expand_for_pane(&host, Some(&id), Some(&prompt_id), None).await.map_err(|e| request(404, e))?;
            let receipt = submit(&host, &id, &expanded, vec![], false, None).await?;
            ActionAnswer::Value(json!({
                "outcome": receipt.outcome,
                "text": expanded,
                "entryId": receipt.entry_id,
                "runId": receipt.run_id,
            }))
        }
        "stop" => {
            let was = running(&host, &id);
            stop_queue(&host, &id);
            let runs = host.engine().map_err(MobileError::Failed)?;
            runs.service.stop(&id).await.map_err(|e| MobileError::Failed(e.to_string()))?;
            ActionAnswer::Value(json!(was))
        }
        "queue-remove" => {
            let item = text("id").unwrap_or_default();
            let mut view = host.view();
            let ui = panes::ui(&mut view, &id).map_err(|_| not_found_session())?;
            if !ui.queue.remove(&item) {
                return Err(request(404, "Queued request not found."));
            }
            ui.forget_unqueued();
            ui.dirty = true;
            ActionAnswer::Value(Value::Null)
        }
        "queue-next" => run_next(&host, &id).await?,
        "queue-run-now" => run_queued_now(&host, &id, &text("id").unwrap_or_default()).await?,
        "rename" => {
            let changed = {
                let mut view = host.view();
                let mut refused = None;
                let auto = text("titleMode").as_deref() == Some("auto");
                let title = text("title").unwrap_or_default();
                view.update(|state| {
                    let next = if auto {
                        mighty_core::rename::set_automatic(state, &id, &l)
                    } else {
                        mighty_core::rename::rename_session(state, &id, title.trim(), &l)
                    };
                    match next {
                        Ok(next) => *state = next,
                        Err(error) => refused = Some(error),
                    }
                })
                .map_err(|e| request(409, e))?;
                refused
            };
            if let Some(refused) = changed {
                return Err(request(400, refused));
            }
            ActionAnswer::Value(Value::Null)
        }
        // The phone's close never asks, a parent's with open children included (M/AppStore+Delegation.swift:204-206), and goes through the
        // service like the desktop's: the parent closes alone and its children stay under a 'parent closed' node.
        "close" => {
            if let Some(ui) = host.view().panes.get_mut(&id) {
                ui.submission += 1;
                ui.queue.clear();
                ui.forget_unqueued();
            }
            if let Some(runs) = host.runs()
                && let Err(error) = runs.service.stop(&id).await
                && !matches!(error, mighty_run::RunError::TimedOut(_))
            {
                return Err(MobileError::Failed(error.to_string()));
            }
            let closed = host.view().snapshot.session(&id).cloned();
            host.view().update(|state| state.sessions_mut().retain(|p| p.id() != id)).map_err(|e| request(409, e))?;
            crate::usage_report::forget_pane(&host, &id);
            host.schedule();
            // C# `ClosePane`'s tail, as the window's close (W9-I): a worktree pane's worktree goes with it or is kept on the workspace.
            crate::pane_worktree::after_close(&host, &id, closed).await;
            ActionAnswer::Value(Value::Null)
        }
        "settings" => {
            settings(&host, &id, &body).await?;
            ActionAnswer::Value(Value::Null)
        }
        // The phone's 보낼 대상 switch: a target the pane offers, or null for Claude.
        "setAskTarget" => {
            let snapshot = Desk::snapshot_of(&host);
            let offered: Vec<&str> = crate::ask::targets_for(&host, &snapshot, &pane).iter().map(|t| t.id).collect();
            let target = ask_target_choice(text("target").as_deref(), &offered, &l)?;
            crate::ask::set_target(&host, &id, target).map_err(|e| request(400, e))?;
            ActionAnswer::Value(Value::Null)
        }
        // A style's button (C# `MobileGuided`): the action's prompt sent as the app's own, checked against the pane's style (styles.rs).
        "guided" => {
            let guided = mighty::guided_request(&body, &l)?;
            let prompt = crate::styles::phone_guided(&host, &pane, &guided).await.map_err(|(status, message)| request(status, message))?;
            ActionAnswer::Receipt(submit(&host, &id, &prompt, vec![], true, None).await?)
        }
        // The answer to the pane's waiting terminal command, typed as its card types it; whether it was.
        "terminal-prompt" => {
            let typed = TerminalPromptAnswer::from_phone(text("promptId").as_deref(), text("answer").as_deref(), text("text").as_deref());
            let answered = match (typed, crate::terminal::terminals(&host)) {
                (Some(typed), Ok(terminals)) => terminals.answer_prompt(&id, &text("promptId").unwrap_or_default(), &typed),
                _ => false,
            };
            ActionAnswer::Value(json!(answered))
        }
        // An Antigravity card answered on the phone, as its own buttons answer it; what comes back is why nothing happened.
        "antigravityPermission" => {
            let runs = host.engine().map_err(MobileError::Failed)?;
            let card = runs
                .service
                .antigravity_card(&id)
                .filter(|c| Some(c.id.as_str()) == text("requestId").as_deref() && Some(c.run_id.as_str()) == text("runId").as_deref())
                .ok_or_else(|| request(409, l.get("remote.error.notPendingPermission")))?;
            let decision = crate::antigravity::decision(text("decision").as_deref())
                .unwrap_or(mighty_run::antigravity::permissions::PermissionDecision::Deny);
            match crate::antigravity::answer(&host, &id, &card, decision).await {
                Some(failure) => ActionAnswer::Value(json!(failure)),
                None => ActionAnswer::Value(Value::Null),
            }
        }
        "command" => match text("action").as_deref() {
            Some("help") => ActionAnswer::Value(json!(mighty_composer::slash::help_text(pane.provider(), &l))),
            Some("usage") => ActionAnswer::Value(json!(usage_text(&host, &pane))),
            _ => {
                let reset = {
                    let mut view = host.view();
                    crate::composer::reset_conversation(&host, &mut view, &id).map_err(|e| request(409, e))?
                };
                if !reset {
                    return Err(request(409, "Stop the session before clearing it."));
                }
                ActionAnswer::Value(Value::Null)
            }
        },
        _ => return Err(request(404, "Unknown desktop action.")),
    };
    host.schedule();
    Ok(answer)
}

/// `MakeMobileImagePreview` (MainWindow.MobileImages.cs) over mighty-files' raster: an SVG drawn first, a PDF's first page by the
/// platform's PDF port, then the thumbnail ladder.
fn image_preview(host: &Host, name: &str, bytes: &[u8]) -> MobileImageResult {
    let files = host.app.try_state::<Arc<crate::files::Files>>().map(|f| f.inner().clone());
    let pdf: Arc<dyn mighty_platform::PdfRaster> = match &files {
        Some(files) => files.ports.pdf.clone(),
        None => Arc::new(mighty_platform::unsupported::Unsupported),
    };
    mighty_remote::phone::files::thumbnail(name, bytes, pdf.as_ref())
}

impl PhoneDesk for Desk {
    fn snapshot(&self) -> AppSnapshot {
        self.host().map(|h| Desk::snapshot_of(&h)).unwrap_or_default()
    }
    fn ensure_body(&self, id: &str) {
        if let Ok(host) = self.host() {
            crate::bodies::ensure(&host, &[id]);
        }
    }
    fn shared_workspace_ids(&self) -> Vec<String> {
        shared_workspace_ids()
    }
    fn host_id(&self) -> String {
        self.host().ok().and_then(|h| crate::mobile::relay(&h)).map(|r| r.host_id()).unwrap_or_default()
    }
    fn host_name(&self) -> String {
        crate::mobile::machine_name()
    }
    fn app_version(&self) -> String {
        self.host().map(|h| h.version.clone()).unwrap_or_default()
    }
    fn is_remote_session(&self, session_id: &str) -> bool {
        let snapshot = self.snapshot();
        snapshot.session(session_id).and_then(|p| snapshot.workspace(p.workspace_id())).is_some_and(|w| w.remote.is_some())
    }
    fn is_session_running(&self, session_id: &str) -> bool {
        self.host().is_ok_and(|h| running(&h, session_id))
    }
    fn pending_permissions(&self, session_id: &str) -> Vec<ToolPermissionRequest> {
        self.host()
            .ok()
            .and_then(|h| h.runs().map(|r| r.service.pending_permissions(session_id)))
            .unwrap_or_default()
            .into_iter()
            .filter(crate::styles::shows)
            .collect()
    }
    fn held_items(&self, session_id: &str) -> Vec<DelegationDeliveryItem> {
        held(session_id)
    }
    fn child_rows(&self) -> Vec<mighty_core::shell::DelegationChildRow> {
        crate::delegation::app().map(|app| app.rows()).unwrap_or_default()
    }
    fn antigravity_card(&self, session_id: &str) -> Option<PermissionRequest> {
        self.host().ok()?.runs()?.service.antigravity_card(session_id)
    }
    fn pending_antigravity_card(&self, session_id: &str, request_id: &str, run_id: &str) -> Option<PermissionRequest> {
        self.antigravity_card(session_id).filter(|c| c.id == request_id && c.run_id == run_id)
    }
    fn respond_to_tool_permission(&self, session_id: &str, request_id: &str, allow: bool) -> BoxFuture<'_, Result<(), MobileError>> {
        let (id, request_id) = (session_id.to_string(), request_id.to_string());
        Box::pin(async move {
            let host = self.host()?;
            let runs = host.engine().map_err(MobileError::Failed)?;
            runs.service.respond_to_tool_permission(&id, &request_id, allow).await.map_err(|e| request(409, e.message()))
        })
    }
    fn answer_questionnaire(
        &self,
        session_id: &str,
        request_id: &str,
        answers: IndexMap<String, UserQuestionAnswer>,
    ) -> BoxFuture<'_, Result<(), MobileError>> {
        let (id, request_id) = (session_id.to_string(), request_id.to_string());
        Box::pin(async move {
            let host = self.host()?;
            let runs = host.engine().map_err(MobileError::Failed)?;
            runs.service.answer_questionnaire(&id, &request_id, &answers).await.map_err(|e| request(409, e.message()))
        })
    }
    fn answer_plan(
        &self,
        session_id: &str,
        request_id: &str,
        decision: PlanDecision,
    ) -> BoxFuture<'_, Result<Option<PlanRecord>, MobileError>> {
        let (id, request_id) = (session_id.to_string(), request_id.to_string());
        Box::pin(async move {
            let host = self.host()?;
            let runs = host.engine().map_err(MobileError::Failed)?;
            runs.service.answer_plan(&id, &request_id, &decision).await.map_err(|e| request(409, e.message()))
        })
    }
    fn run_folder(&self, session_id: &str) -> Option<String> {
        let snapshot = self.snapshot();
        let pane = snapshot.session(session_id)?;
        let workspace = snapshot.workspace(pane.workspace_id()).filter(|w| w.remote.is_none())?;
        Some(pane.working_folder.clone().unwrap_or_else(|| workspace.path().to_string()))
    }
    fn originals_run(&self, session_id: &str, run_id: Option<&str>) -> Option<String> {
        let run = run_id?;
        let snapshot = self.snapshot();
        Some(
            snapshot
                .session(session_id)
                .and_then(|p| {
                    p.graph_runs().iter().find(|r| r.id() == run).map(|r| r.source_run_id.clone().unwrap_or_else(|| r.id().to_string()))
                })
                .unwrap_or_else(|| run.to_string()),
        )
    }
    fn file_changes(
        &self,
        root: String,
        relative: String,
        session_id: Option<String>,
        run_id: Option<String>,
    ) -> BoxFuture<'_, Result<Option<ChangeSet>, MobileError>> {
        Box::pin(async move {
            let host = self.host()?;
            let files = host.app.try_state::<Arc<crate::files::Files>>().map(|f| f.inner().clone()).ok_or(MobileError::Unavailable)?;
            Ok(changes::read(
                Some(&files.originals),
                host.processes.as_ref(),
                files.ports.handles.as_ref(),
                &root,
                &relative,
                session_id.as_deref(),
                run_id.as_deref(),
            )
            .await)
        })
    }
    fn image_preview(&self, name: String, bytes: Vec<u8>) -> BoxFuture<'_, MobileImageResult> {
        Box::pin(async move {
            let Ok(host) = self.host() else { return MobileImageResult { image: None, reason: Some("undecodable".into()) } };
            let worker = host.clone();
            tauri::async_runtime::spawn_blocking(move || image_preview(&worker, &name, &bytes))
                .await
                .unwrap_or(MobileImageResult { image: None, reason: Some("undecodable".into()) })
        })
    }
    fn action(&self, action_: MobileDesktopAction) -> BoxFuture<'_, Result<ActionAnswer, MobileError>> {
        Box::pin(async move {
            let host = self.host()?;
            // Spawned and awaited: a phone that went away mid-request never leaves half a submit behind (the desktop's ran to completion).
            tauri::async_runtime::spawn(action(host, action_)).await.map_err(|_| MobileError::Failed("the action stopped".into()))?
        })
    }
    fn extras(&self, pane: &RunSession) -> MobilePaneExtras {
        self.snapshot_extras(pane, false)
    }
    fn role_extras(&self, pane: &RunSession) -> Option<MobilePaneExtras> {
        (pane.kind() == "claude").then(|| self.snapshot_extras(pane, true))
    }
    fn structure(&self, pane: &RunSession) -> Option<Built> {
        if pane.kind() != "claude" {
            return None;
        }
        let host = self.host().ok()?;
        mobile_pane(&host, pane.id()).ok()?;
        let snapshot = Desk::snapshot_of(&host);
        let runs = pane.graph_runs();
        let has_plan =
            host.runs().is_some_and(|r| r.service.pending_permissions(pane.id()).iter().any(|p| p.can_answer_plan && p.state == "pending"));
        let pending = mighty_graph::view::diagram_plan_run_id(has_plan, true, runs);
        let styled = crate::styles::source(&host, pane);
        let inputs = mighty_graph::structure::Inputs {
            style: styled.as_ref().map(|s| &s.style as &dyn mighty_graph::structure::StructureStyle),
            current_phase: styled.as_ref().and_then(|s| s.phase.as_ref()),
            starting_new: styled.as_ref().is_some_and(|s| s.starting_new),
            plan_history: pane.plan_history.as_deref(),
            pending_plan_run_id: pending.as_deref(),
        };
        let options = picker_catalog(&host, pane.provider()).options();
        let key = std::iter::once(host.locale.language().to_string())
            .chain(options.iter().flat_map(|m| [m.value.clone(), m.display_name.clone(), m.resolved_model.clone().unwrap_or_default()]))
            .collect::<Vec<_>>()
            .join("\n");
        let model = |id: &str| mighty_graph::usage_format::short_name(id, Some(&options), true);
        // The background runs' last output lines (tails.rs): the same map until they change, so the cache can tell.
        let tails = crate::tails::output_for(pane.id());
        let naming = Naming { key: &key, title: None, model: Some(&model), tails: Some(&tails) };
        let mut kept = lock(&self.kept);
        let panes: HashSet<String> = snapshot.sessions().iter().map(|p| p.id().to_string()).collect();
        kept.structures.retain(&panes);
        kept.result_files.retain(|id, _| panes.contains(id));
        Some((*kept.structures.structure(pane.id(), runs, &inputs, &naming, &host.locale)).clone())
    }
    fn digest(&self, pane: &RunSession) -> Option<MobilePaneDigest> {
        if pane.kind() != "claude" {
            return None;
        }
        let host = self.host().ok()?;
        if !host.view().panes.contains_key(pane.id()) {
            return None;
        }
        let snapshot = Desk::snapshot_of(&host);
        let offered = options(&host, &snapshot, pane, None, None);
        let styled = crate::styles::phone(&host, pane, busy(&host, pane.id())).map(|s| s.digest);
        let (targets, effective) = ask_targets_of(&host, &snapshot, pane);
        let options = hash_of(|h| {
            for t in &targets {
                format!("ask:{}", t.id).hash(h);
            }
            format!("askTarget:{}", effective.as_deref().unwrap_or("")).hash(h);
            for m in &offered.models {
                (&m.id, &m.label).hash(h);
            }
            for m in &offered.permission_modes {
                m.id.hash(h);
            }
            for m in &offered.mighty_styles {
                m.id.hash(h);
            }
            for m in &offered.styles {
                (&m.id, &m.source).hash(h);
            }
            for m in offered.efforts.iter().flatten() {
                m.id.hash(h);
            }
        });
        let prompt = crate::terminal::terminals(&host).ok().and_then(|t| t.prompt(pane.id())).map(|p| p.id);
        let mighty = hash_of(|h| {
            mighty::digest(pane).hash(h);
            styled.hash(h);
            pane.plan_history.as_ref().map(Vec::len).hash(h);
            pane.plan_history.as_ref().and_then(|p| p.last()).and_then(|r| r.outcome.clone()).hash(h);
            for task in pane.background_work.as_ref().map(mighty::running_tasks).unwrap_or_default() {
                (&task.id, &task.description, &task.tool_use_id, &task.kind).hash(h);
            }
            mighty::mobile_background_done(pane.background_work.as_ref()).hash(h);
            prompt.hash(h);
            // A background run's output lines live in the host's memory, not in the runs: they move the pane through their revision, which
            // moves at most once per 3 s a pane (tails.rs).
            crate::tails::phone_revision_for(pane.id()).hash(h);
            crate::saved_prompts::revision(&host).hash(h);
            // The custom agents read for the phone's role view: their colours and models.
            let definitions = PHONE_DEFINITIONS.peek(pane.id());
            let mut kinds: Vec<&String> = definitions.keys().collect();
            kinds.sort_unstable();
            for kind in kinds {
                (kind, &definitions[kind].color, &definitions[kind].model).hash(h);
            }
        });
        Some(MobilePaneDigest { editable: !busy(&host, pane.id()), options, mighty })
    }
    fn active_style(&self, pane: &RunSession) -> Option<String> {
        crate::styles::active_style(&*self.host().ok()?, pane)
    }
    fn app_update_read(&self) -> Option<Option<app_update_view::State>> {
        // `PhoneAppUpdateRead`: null while this build cannot update itself (app_update.rs: no key, no address, a smoke, or a build that
        // may not change the machine), and the route then answers 409 `appUpdate.mobile.unavailable`.
        Some(self.host().ok().and_then(|host| crate::app_update::phone_read(&host)))
    }
    fn app_update_act(&self, install: bool) -> BoxFuture<'_, Result<(), MobileError>> {
        // `PhoneAppUpdateActAsync`, on the app update's driver (app_update.rs), awaited as C# awaits its check.
        Box::pin(async move {
            if let Ok(host) = self.host() {
                crate::app_update::phone_act(&host, install).await;
            }
            Ok(())
        })
    }
    fn attention(&self, session_id: &str) -> Option<(String, String, String)> {
        crate::attention::phone(session_id)
    }
    fn queue_count(&self, session_id: &str) -> Option<i32> {
        let host = self.host().ok()?;
        let count = host.view().panes.get(session_id).map_or(0, |u| u.queue.items().len() as i32);
        Some(count)
    }
    fn catalog(&self, pane: &RunSession) -> Option<ModelCatalog> {
        self.host().ok()?.runs()?.runtime(pane.provider()).map(|r| r.model_catalog)
    }
    fn work_map_read(&self, workspace: &Workspace) -> Option<BoxFuture<'_, Result<WorkMapModel, MobileError>>> {
        let host = self.host().ok()?;
        let service = crate::workmap::service(&host).ok()?;
        let workspace = workspace.clone();
        Some(Box::pin(async move { Ok(service.model(&workspace).await) }))
    }
    fn work_map_summarize(&self, workspace: &Workspace, version: &str) -> Option<BoxFuture<'_, Result<WorkMapStart, MobileError>>> {
        let host = self.host().ok()?;
        let service = crate::workmap::service(&host).ok()?;
        let (workspace, version) = (workspace.clone(), version.to_string());
        Some(Box::pin(async move { Ok(service.start(&workspace, &version).await) }))
    }
    fn direct_endpoints(&self) -> Vec<String> {
        self.host().map(|h| crate::direct::endpoints(&h)).unwrap_or_default()
    }
    fn extra_capabilities(&self) -> Vec<String> {
        EXTRA_CAPABILITIES.iter().map(|c| (*c).to_string()).collect()
    }
    fn saved_prompts(&self, workspace_id: &str) -> Vec<mighty_core::saved_prompts::SavedPrompt> {
        self.host().map(|h| crate::saved_prompts::offered(&h, Some(workspace_id))).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn en() -> mighty_core::locale::Locale {
        mighty_core::locale::Locale::new("en")
    }

    fn status_of(result: &Result<SubmitReceipt, MobileError>) -> Option<(u16, String)> {
        match result {
            Err(MobileError::Request { status, message, .. }) => Some((*status, message.clone())),
            _ => None,
        }
    }

    /// C# `MobileSubmit`'s answers: a start that went out with held items says started without ids, one that started at once carries
    /// them, `NotStarted` is 409 `remote.error.dropped`, a failure its own reason; a queued continuation that did not start loses its
    /// grant (S-L3) with the start's failure, or `remote.error.startFailed` when no run started.
    #[test]
    fn a_phone_submit_answers_as_its_start_went() {
        let l = en();
        let (answer, revoke) = answer_start(Started::Now(Some("e".into()), Some("r".into())), true, &l);
        assert_eq!(answer.unwrap(), SubmitReceipt::with_ids("started", Some("e"), Some("r")));
        assert!(revoke.is_none(), "a start revokes nothing");
        let (answer, revoke) = answer_start(Started::Released, false, &l);
        assert!(answer.unwrap() == SubmitReceipt::new("started") && revoke.is_none(), "held items went first: no ids");
        let (answer, revoke) = answer_start(Started::NotStarted, false, &l);
        assert_eq!(status_of(&answer), Some((409, l.get("remote.error.dropped"))));
        assert!(revoke.is_none(), "a plain send has no grant to revoke");
        let (answer, revoke) = answer_start(Started::NotStarted, true, &l);
        assert_eq!(status_of(&answer), Some((409, l.get("remote.error.dropped"))));
        assert_eq!(revoke, Some(l.get("remote.error.startFailed")), "a continuation that did not start loses its grant");
        let (answer, revoke) = answer_start(Started::Failed("agy refused".into()), true, &l);
        assert_eq!(status_of(&answer), Some((409, "agy refused".into())));
        assert_eq!(revoke.as_deref(), Some("agy refused"), "with the start's own reason");
    }

    /// S-L3: the revoked continuation leaves the queue with its grant and card; the row behind it stays.
    #[test]
    fn a_revoked_continuation_leaves_the_queue_with_its_rules() {
        let l = en();
        let mut ui = panes::PaneUi::default();
        let first = ui.queue.add("go on", vec![], None, false, true, &l).unwrap().id;
        let second = ui.queue.add("then this", vec![], None, false, false, &l).unwrap().id;
        let grant =
            mighty_run::antigravity::AllowGrant { request_id: "agy-1".into(), rules: vec!["command(echo hi)".into()], temporary: true };
        let card = PermissionRequest { id: "agy-1".into(), run_id: "run-1".into(), actions: vec![], received_at: None };
        ui.continuations.insert(first.clone(), (grant, card));
        ui.dirty = false;
        revoke_queued(&mut ui, &first);
        assert!(ui.continuations.is_empty() && ui.dirty, "the grant and card went");
        assert_eq!(ui.queue.items().iter().map(|q| q.id.as_str()).collect::<Vec<_>>(), [second.as_str()], "the next row stays");
    }

    /// C# `MobileSettings`' checks.
    #[test]
    fn a_settings_request_is_checked_against_what_the_pane_offers() {
        let l = en();
        let models = vec!["default".to_string(), "opus".to_string()];
        let modes = vec!["manual".to_string(), "acceptEdits".to_string()];
        let efforts = |model: &str| if model == "opus" { vec!["low".to_string(), "high".to_string()] } else { vec![] };
        let current = RunSettings::default();
        let check = |body: Value| settings_choice(body.as_object().unwrap(), "default", &current, &models, &modes, &efforts, &l);
        assert_eq!(check(json!({"model": "opus", "effort": "high"})).unwrap(), ("opus".into(), "manual".into(), "high".into()));
        assert_eq!(check(json!({"permissionMode": "acceptEdits"})).unwrap(), ("default".into(), "acceptEdits".into(), "default".into()));
        let refused = |body: Value| match check(body) {
            Err(MobileError::Request { status, message, .. }) => (status, message),
            other => panic!("{other:?}"),
        };
        assert_eq!(refused(json!({})), (400, "Unknown setting.".into()));
        assert_eq!(refused(json!({"model": 3})), (400, "Unknown setting.".into()));
        assert_eq!(refused(json!({"fast": "yes"})), (400, "Unknown setting.".into()));
        assert_eq!(refused(json!({"model": "sonnet"})), (400, "Unsupported model, permission, or effort.".into()));
        assert_eq!(refused(json!({"permissionMode": "fullAccess"})), (400, "Unsupported model, permission, or effort.".into()));
        assert_eq!(
            refused(json!({"effort": "high"})),
            (400, "Unsupported model, permission, or effort.".into()),
            "default takes no effort"
        );
        assert_eq!(refused(json!({"agentViewMode": "graph"})), (400, "Unsupported view.".into()));
        assert!(check(json!({"agentViewMode": "plain", "styleId": "cli"})).is_ok(), "an older phone's plain view and the CLI are heard");
    }

    /// The phone's 보낼 대상: a target the pane offers, or null for Claude.
    #[test]
    fn a_send_to_choice_must_be_offered() {
        let l = en();
        assert_eq!(ask_target_choice(Some("codex"), &["codex"], &l).unwrap().as_deref(), Some("codex"));
        assert_eq!(ask_target_choice(None, &["codex"], &l).unwrap(), None);
        assert!(matches!(ask_target_choice(Some("gemini"), &["codex"], &l), Err(MobileError::Request { status: 400, .. })));
    }

    /// `/m1/info` names this OS as the phone app knows it.
    #[test]
    fn the_phone_hears_this_hosts_platform() {
        assert_eq!(m1_platform(), if cfg!(windows) { "win32" } else { "darwin" });
    }
}
