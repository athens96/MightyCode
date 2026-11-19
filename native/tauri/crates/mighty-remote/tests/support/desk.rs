//! The headless desk (W8-B): `phone::desk::PhoneDesk` over a real `RunService` (the state, normalized as the app keeps it, and which
//! panes run), with C#'s router callbacks as settable closures, so the C# route checks run in `cargo test` as they run over a
//! `DesktopService` in C#: the window's `action` and `extra`, and the optional `Digest`, `ActiveStyle`, `Catalog`, `Structure`,
//! `RoleExtras`, `AppUpdateRead`/`AppUpdateAct`, `ImagePreview`, `QueueCount`, `WorkMapRead`/`WorkMapSummarize`. A check's own
//! folder is a temp folder it deletes; nothing reads or writes the user's profile. Owner: W8-B.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use indexmap::IndexMap;
use mighty_core::app_update_view;
use mighty_core::model::{AppSnapshot, PlanRecord, RunSession, Workspace};
use mighty_core::permission::ToolPermissionRequest;
use mighty_delegation::delivery::DelegationDeliveryItem;
use mighty_files::changes::ChangeSet;
use mighty_graph::structure::mobile::Built;
use mighty_platform::BoxFuture;
use mighty_remote::phone::desk::{
    ActionAnswer, MobileDesktopAction, MobileError, MobileImageResult, MobilePaneDigest, MobilePaneExtras, PhoneDesk,
};
use mighty_remote::phone::router::{Router, RouterOptions};
use mighty_remote::relay::connection::{MobileDevice, MobileReply};
use mighty_run::antigravity::permissions::PermissionRequest;
use mighty_run::model::ModelCatalog;
use mighty_run::plan_mode::PlanDecision;
use mighty_run::providers::{Finder, ProviderCatalog};
use mighty_run::questionnaire::UserQuestionAnswer;
use mighty_run::service::{RunService, RunServiceOptions};
use mighty_workmap::WorkMapModel;
use mighty_workmap::service::WorkMapStart;
use serde_json::Value;

type Of<T> = Option<Arc<dyn Fn(&RunSession) -> T + Send + Sync>>;
type ActionFn = Arc<dyn Fn(&MobileDesktopAction) -> Result<ActionAnswer, MobileError> + Send + Sync>;
type CountFn = Arc<dyn Fn(&str) -> i32 + Send + Sync>;
type ImageFn = Arc<dyn Fn(String, Vec<u8>) -> BoxFuture<'static, MobileImageResult> + Send + Sync>;

/// A temp folder deleted when dropped.
pub struct Temp(pub PathBuf);

impl Temp {
    pub fn new(tag: &str) -> Temp {
        let path = std::env::temp_dir().join(format!("mighty-{tag}-{}", mighty_core::wire::new_id()));
        std::fs::create_dir_all(&path).expect("a temp folder");
        Temp(path)
    }
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The desk's callbacks and recorded calls.
#[derive(Default)]
pub struct Calls {
    pub action: Option<ActionFn>,
    pub extras: Of<MobilePaneExtras>,
    pub role_extras: Of<Option<MobilePaneExtras>>,
    pub digest: Of<Option<MobilePaneDigest>>,
    pub active_style: Of<Option<String>>,
    pub catalog: Of<Option<ModelCatalog>>,
    pub structure: Of<Option<Built>>,
    pub queue_count: Option<CountFn>,
    pub image_preview: Option<ImageFn>,
    pub app_update: Option<Option<app_update_view::State>>,
    pub app_acts: Vec<bool>,
    pub work_map: Option<WorkMapModel>,
    pub summarize: Option<WorkMapStart>,
    pub summarized: Vec<String>,
    pub actions: Vec<MobideskAction>,
    pub permissions: Vec<ToolPermissionRequest>,
    pub answered: Vec<(String, String, String)>,
    pub running: HashSet<String>,
    pub held: HashMap<String, Vec<DelegationDeliveryItem>>,
    pub cards: HashMap<String, PermissionRequest>,
    pub run_folders: HashMap<String, String>,
    pub file_changes: Option<ChangeSet>,
    pub direct: Vec<String>,
    pub extra_capabilities: Vec<String>,
}

pub type MobideskAction = MobileDesktopAction;

/// The headless desk.
pub struct Desk {
    pub service: Arc<RunService>,
    pub calls: Mutex<Calls>,
    pub host_id: String,
    pub host_name: String,
    pub version: String,
}

impl Desk {
    pub fn calls(&self) -> std::sync::MutexGuard<'_, Calls> {
        self.calls.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// The state changed as C#'s `service.UpdateAsync` changes it.
    pub fn update(&self, change: impl FnOnce(&mut AppSnapshot)) {
        self.service
            .update(|current| {
                let mut next = current.clone();
                change(&mut next);
                next
            })
            .expect("the state takes the change");
    }

    /// The panes replaced.
    pub fn set_sessions(&self, panes: Vec<RunSession>) {
        self.update(|s| s.sessions = Some(panes.into_iter().map(Arc::new).collect()));
    }

    pub fn actions(&self) -> Vec<MobileDesktopAction> {
        self.calls().actions.clone()
    }
}

/// A workspace over `path` as `AddWorkspaceAsync` makes it.
pub fn workspace(id: &str, path: &Path) -> Workspace {
    Workspace {
        id: Some(id.into()),
        name: Some(path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()),
        path: Some(path.to_string_lossy().into_owned()),
        ..Workspace::default()
    }
}

/// A pane of `workspace` with C#'s `new RunSession { … }` defaults (an idle Claude pane on the default model).
pub fn pane(id: &str, workspace: &str) -> RunSession {
    RunSession {
        id: Some(id.into()),
        workspace_id: Some(workspace.into()),
        title: Some("Claude".into()),
        kind: Some("claude".into()),
        status: Some("idle".into()),
        model: Some("default".into()),
        provider: Some("claude".into()),
        created_at: Some(mighty_core::time::now_o()),
        logs: Some(vec![]),
        ..RunSession::default()
    }
}

fn catalog() -> Arc<ProviderCatalog> {
    let finder: Finder = Arc::new(|_: String| Box::pin(async { None }));
    Arc::new(ProviderCatalog::new(
        Arc::new(mighty_platform::fake::FakeProcessHost::default()),
        Arc::new(mighty_platform::fake::FakeShellEnvironment::new(std::env::vars().collect())),
        Some(finder),
    ))
}

/// A world: a temp folder with `workspace/` in it, a run service whose state has that workspace (id `w`), the desk over it and a router
/// whose uploads are `mobile/uploads`.
pub struct World {
    pub temp: Temp,
    pub desk: Arc<Desk>,
    pub router: Router,
}

impl World {
    pub fn new(tag: &str) -> World {
        World::with(tag, |_| {})
    }

    pub fn with(tag: &str, options: impl FnOnce(&mut RouterOptions)) -> World {
        let temp = Temp::new(tag);
        let folder = temp.path().join("workspace");
        std::fs::create_dir_all(&folder).expect("the workspace folder");
        let snapshot = AppSnapshot { workspaces: Some(vec![workspace("w", &folder)]), sessions: Some(vec![]), ..AppSnapshot::default() };
        let snapshot = mighty_core::normalize::normalize(&snapshot, false).expect("a state");
        World::over(temp, snapshot, "host-fixture", "Fixture", options)
    }

    /// A world over a state as it was loaded (the parity check's profile), with the host's id and name.
    pub fn over(temp: Temp, snapshot: AppSnapshot, host_id: &str, host_name: &str, options: impl FnOnce(&mut RouterOptions)) -> World {
        let (service, _events) = RunService::new(
            snapshot,
            catalog(),
            RunServiceOptions {
                plugin_directory: temp.path().join("plugins").to_string_lossy().into_owned(),
                permission_card: false,
                remote: None,
                configure: None,
                admitting: None,
                binding: None,
                delegation: None,
            },
        );
        let desk = Arc::new(Desk {
            service: Arc::new(service),
            calls: Mutex::new(Calls::default()),
            host_id: host_id.into(),
            host_name: host_name.into(),
            version: "0.0.0".into(),
        });
        let mut router_options = RouterOptions::new(temp.path().join("mobile").join("uploads"), "win32");
        options(&mut router_options);
        let router = Router::new(desk.clone(), router_options);
        World { temp, desk, router }
    }

    pub fn folder(&self) -> PathBuf {
        self.temp.path().join("workspace")
    }

    /// `RouteAsync(method, path, body, "phone")`.
    pub async fn route(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let reply = self.raw(method, path, body).await;
        (reply.status, serde_json::from_str(&reply.body).expect("a JSON reply"))
    }

    pub async fn raw(&self, method: &str, path: &str, body: Option<Value>) -> MobileReply {
        self.from("phone", method, path, body).await
    }

    pub async fn from(&self, device: &str, method: &str, path: &str, body: Option<Value>) -> MobileReply {
        let raw = body.map(|b| serde_json::value::RawValue::from_string(b.to_string()).expect("raw"));
        let device = MobileDevice { client_id: device.into(), name: "Phone".into(), direct: false, desktop: false };
        self.router.route(device, method.into(), path.into(), raw).await
    }
}

impl PhoneDesk for Desk {
    fn snapshot(&self) -> AppSnapshot {
        self.service.snapshot()
    }
    fn host_id(&self) -> String {
        self.host_id.clone()
    }
    fn host_name(&self) -> String {
        self.host_name.clone()
    }
    fn app_version(&self) -> String {
        self.version.clone()
    }
    fn is_remote_session(&self, session_id: &str) -> bool {
        let snapshot = self.service.snapshot();
        snapshot.session(session_id).and_then(|p| snapshot.workspace(p.workspace_id()).cloned()).is_some_and(|w| w.remote.is_some())
    }
    fn is_session_running(&self, session_id: &str) -> bool {
        self.calls().running.contains(session_id) || self.service.is_session_running(session_id)
    }
    fn pending_permissions(&self, session_id: &str) -> Vec<ToolPermissionRequest> {
        let mut found: Vec<ToolPermissionRequest> =
            self.calls().permissions.iter().filter(|p| p.run_id == session_id && p.state == "pending").cloned().collect();
        found.extend(self.service.pending_permissions(session_id));
        found
    }
    fn held_items(&self, session_id: &str) -> Vec<DelegationDeliveryItem> {
        self.calls().held.get(session_id).cloned().unwrap_or_default()
    }
    fn antigravity_card(&self, session_id: &str) -> Option<PermissionRequest> {
        self.calls().cards.get(session_id).cloned()
    }
    fn pending_antigravity_card(&self, session_id: &str, request_id: &str, run_id: &str) -> Option<PermissionRequest> {
        self.antigravity_card(session_id).filter(|c| c.id == request_id && c.run_id == run_id)
    }
    fn respond_to_tool_permission(&self, session_id: &str, request_id: &str, allow: bool) -> BoxFuture<'_, Result<(), MobileError>> {
        self.calls().answered.push((session_id.into(), request_id.into(), if allow { "allow" } else { "deny" }.into()));
        Box::pin(async { Ok(()) })
    }
    fn answer_questionnaire(
        &self,
        session_id: &str,
        request_id: &str,
        answers: IndexMap<String, UserQuestionAnswer>,
    ) -> BoxFuture<'_, Result<(), MobileError>> {
        self.calls().answered.push((session_id.into(), request_id.into(), format!("{} answers", answers.len())));
        Box::pin(async { Ok(()) })
    }
    fn answer_plan(
        &self,
        session_id: &str,
        request_id: &str,
        decision: PlanDecision,
    ) -> BoxFuture<'_, Result<Option<PlanRecord>, MobileError>> {
        self.calls().answered.push((session_id.into(), request_id.into(), decision.kind));
        Box::pin(async { Ok(None) })
    }
    fn run_folder(&self, session_id: &str) -> Option<String> {
        self.calls().run_folders.get(session_id).cloned()
    }
    fn originals_run(&self, _session_id: &str, run_id: Option<&str>) -> Option<String> {
        run_id.map(str::to_string)
    }
    fn file_changes(
        &self,
        _: String,
        _: String,
        _: Option<String>,
        _: Option<String>,
    ) -> BoxFuture<'_, Result<Option<ChangeSet>, MobileError>> {
        let found = self.calls().file_changes.clone();
        Box::pin(async move { Ok(found) })
    }
    fn image_preview(&self, name: String, bytes: Vec<u8>) -> BoxFuture<'_, MobileImageResult> {
        match self.calls().image_preview.clone() {
            Some(preview) => preview(name, bytes),
            None => {
                // Without a decoder only a small PNG or JPEG the phone can draw as it is travels (C#'s router without `ImagePreview`).
                let mime = (bytes.len() <= mighty_remote::phone::files::MAXIMUM_THUMBNAIL_BYTES)
                    .then(|| mighty_composer::attachments::detect_media_type(&bytes))
                    .filter(|m| matches!(*m, "image/png" | "image/jpeg"));
                Box::pin(async move {
                    match mime {
                        Some(mime) => MobileImageResult {
                            image: Some(mighty_remote::phone::desk::MobileImagePreview {
                                mime: mime.into(),
                                data: base64_of(&bytes),
                                width: 0,
                                height: 0,
                                thumbnail_width: 0,
                                thumbnail_height: 0,
                            }),
                            reason: None,
                        },
                        None => MobileImageResult { image: None, reason: Some("tooLarge".into()) },
                    }
                })
            }
        }
    }
    fn action(&self, action: MobileDesktopAction) -> BoxFuture<'_, Result<ActionAnswer, MobileError>> {
        let answer = {
            let mut calls = self.calls();
            calls.actions.push(action.clone());
            calls.action.clone()
        };
        let result = match answer {
            Some(answer) => answer(&action),
            None => Ok(ActionAnswer::Value(Value::Null)),
        };
        Box::pin(async move { result })
    }
    fn extras(&self, pane: &RunSession) -> MobilePaneExtras {
        let extras = self.calls().extras.clone();
        extras.map(|f| f(pane)).unwrap_or_default()
    }
    fn role_extras(&self, pane: &RunSession) -> Option<MobilePaneExtras> {
        let extras = self.calls().role_extras.clone();
        extras.and_then(|f| f(pane))
    }
    fn structure(&self, pane: &RunSession) -> Option<Built> {
        let structure = self.calls().structure.clone();
        structure.and_then(|f| f(pane))
    }
    fn digest(&self, pane: &RunSession) -> Option<MobilePaneDigest> {
        let digest = self.calls().digest.clone();
        digest.and_then(|f| f(pane))
    }
    fn active_style(&self, pane: &RunSession) -> Option<String> {
        let style = self.calls().active_style.clone();
        match style {
            Some(f) => f(pane),
            None => pane.mighty_style.clone(),
        }
    }
    fn app_update_read(&self) -> Option<Option<app_update_view::State>> {
        self.calls().app_update.clone()
    }
    fn app_update_act(&self, install: bool) -> BoxFuture<'_, Result<(), MobileError>> {
        self.calls().app_acts.push(install);
        Box::pin(async { Ok(()) })
    }
    fn queue_count(&self, session_id: &str) -> Option<i32> {
        let count = self.calls().queue_count.clone();
        count.map(|f| f(session_id))
    }
    fn catalog(&self, pane: &RunSession) -> Option<ModelCatalog> {
        let catalog = self.calls().catalog.clone();
        catalog.and_then(|f| f(pane))
    }
    fn work_map_read(&self, _workspace: &Workspace) -> Option<BoxFuture<'_, Result<WorkMapModel, MobileError>>> {
        let model = self.calls().work_map.clone()?;
        Some(Box::pin(async move { Ok(model) }))
    }
    fn work_map_summarize(&self, _workspace: &Workspace, version: &str) -> Option<BoxFuture<'_, Result<WorkMapStart, MobileError>>> {
        let mut calls = self.calls();
        let start = calls.summarize?;
        calls.summarized.push(version.into());
        Some(Box::pin(async move { Ok(start) }))
    }
    fn direct_endpoints(&self) -> Vec<String> {
        self.calls().direct.clone()
    }
    fn extra_capabilities(&self) -> Vec<String> {
        self.calls().extra_capabilities.clone()
    }
}

pub fn base64_of(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}
