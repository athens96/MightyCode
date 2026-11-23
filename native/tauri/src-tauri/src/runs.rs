//! The run engine in the app: one [`RunService`] (the run half of C# `DesktopService`) for every pane, with mighty-graph's tracker
//! plugged into its runner (`mighty_graph::TrackerFactory`: each request's graph run published every second and at its end, with the
//! notice runs its background work opened, and a Codex run's sub-agents read live from its session files through mighty-history's
//! `CodexSessionWatcher`), the picture cache (`AgentImageCache`) and the CLIs' catalogue (`ProviderCatalog`), whose readings the
//! composer's pills and menus show. pi runs load the app's `ask_user` extension (`agents_directory`) and start a new session when pi
//! has no record of the one asked for (mighty-history's `pi::session_file`).
//!
//! The service owns the state from here on: every change the host makes goes through it (`View::update` → `RunService::update`), and
//! each [`ServiceEvent`] it hands back is applied to the page's copy by [`Host::service_event`] (host.rs), which pushes `event` and
//! `permission` with C#'s rules (WebBridge.cs:75-82) and queues the saves.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use mighty_composer::images::{AgentImageCache, AgentImageError, CacheError};
use mighty_composer::runtime as composer_runtime;
use mighty_core::model::{AgentImageRef, AppSnapshot, RegisteredModelEntry, RunSession, Workspace};
use mighty_graph::TrackerFactory;
use mighty_history::{codex_sessions, history};
use mighty_platform::{ProcessHost, ShellEnvironment};
use mighty_run::antigravity::AntigravityRuleLeases;
#[cfg(feature = "dev-harness")]
use mighty_run::model::CliCommand;
use mighty_run::model::{ModelCatalog, ProviderRuntime};
use mighty_run::parser::{ImageError, ImageStore};
use mighty_run::providers::{self, Finder, ProviderCatalog};
use mighty_run::service::{Configure, RunService, RunServiceOptions, ServiceEvent};

use crate::host::Host;

/// `AgentImageSupport.MaximumCacheBytes`.
const IMAGE_CACHE_BYTES: u64 = 200 * 1_048_576;

/// The smoke's stand-in Claude (mighty-run's `mighty-fake-cli --replay-cli`): asks one Bash permission and one plan, then replays a
/// recording; its stand-in pi (`--fake-cli pi … --pi-fixture pi-rpc-question.jsonl --pi-dialogs`), which asks one question through the
/// app's `ask_user` tool; its Antigravity, the same binary as C#'s fake agy (`--agy-denied`, refused `echo hi` on a new conversation)
/// over the smoke's own agy settings ([`FakeClaude::agy_settings`]); and the ACP step's opencode (`--fake-acp`, smoke_acp.rs). Every
/// other provider is missing.
#[cfg(feature = "dev-harness")]
#[derive(Debug, Clone)]
pub struct FakeClaude {
    pub binary: PathBuf,
    pub recording: String,
    /// Where the fake writes what it was sent (`<record>.args`, `.input`, `.answers`).
    pub record: PathBuf,
}

#[cfg(not(feature = "dev-harness"))]
#[derive(Debug, Clone)]
pub enum FakeClaude {}

#[cfg(feature = "dev-harness")]
impl FakeClaude {
    /// The smoke's own agy settings (never the user's): beside the fake's record.
    pub fn agy_settings(&self) -> PathBuf {
        fake_agy_settings(&self.record)
    }
}

/// The stand-in agy's settings for the fake's record `record`: `<record's folder>\agy-home\.gemini\antigravity-cli\settings.json`.
#[cfg(feature = "dev-harness")]
pub fn fake_agy_settings(record: &Path) -> PathBuf {
    let folder = record.parent().map(Path::to_path_buf).unwrap_or_default();
    folder.join("agy-home").join(".gemini").join("antigravity-cli").join("settings.json")
}

pub struct Runs {
    pub service: Arc<RunService>,
    pub catalog: Arc<ProviderCatalog>,
    pub images: Arc<AgentImageCache>,
    /// Each provider's runtime as last read (C# `service.Runtime` / `PaneRuntime`): none until the first read finished.
    runtime: Mutex<HashMap<String, ProviderRuntime>>,
}

/// mighty-composer's picture cache as the run engine's image store (`AgentImageStore.Prepare`).
struct CacheStore(Arc<AgentImageCache>);

fn image_error(error: CacheError) -> ImageError {
    match error {
        CacheError::Image(AgentImageError::Empty) => ImageError::Empty,
        CacheError::Image(AgentImageError::TooLarge) => ImageError::TooLarge,
        CacheError::Image(AgentImageError::UnsupportedType) => ImageError::UnsupportedType,
        CacheError::Image(AgentImageError::Undecodable) | CacheError::Io(_) => ImageError::Undecodable,
        CacheError::Image(AgentImageError::ExternalSvg) => ImageError::ExternalSvg,
        CacheError::Image(AgentImageError::TooManyPixels) => ImageError::TooManyPixels,
        CacheError::Image(AgentImageError::InvalidEncoding) => ImageError::InvalidEncoding,
    }
}

impl ImageStore for CacheStore {
    fn prepare(&self, base64: &str, media_type: &str, source: &str, read_path: Option<&str>) -> Result<AgentImageRef, ImageError> {
        Ok(self.0.prepare_base64(base64, media_type).map_err(image_error)?.reference(source, read_path))
    }
}

/// What Hermes's `state.db` says of a session a run would continue (C# `HermesSessions.MessageCount`, `StartedWithFullAccess` of
/// `StateDatabase(environment)`), read through mighty-history; read-only.
fn hermes_session(environment: &std::collections::BTreeMap<String, String>, session: &str) -> (Option<i32>, Option<bool>) {
    let environment: HashMap<String, String> = environment.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    let database = mighty_history::acp::hermes_state_database(&environment, None);
    (
        mighty_history::acp::hermes_message_count(&database, session),
        mighty_history::acp::hermes_started_with_full_access(&database, session),
    )
}

/// A Hermes pane's run ended (C# `RefreshModelsAfterHermesRun`): the pane whose model list is read again, so the models its session
/// offered are in the menu. None for anything else.
fn hermes_run_ended(host: &Host, event: &ServiceEvent) -> Option<String> {
    let ServiceEvent::Run { event, .. } = event else { return None };
    if event.kind != "status" || !matches!(event.status.as_deref(), Some("completed" | "error" | "stopped")) {
        return None;
    }
    let view = host.view();
    view.snapshot.session(&event.session_id).filter(|s| s.kind() == "claude" && s.provider() == "hermes")?;
    Some(event.session_id.clone())
}

/// The mighty-bridge plugin the Claude runs load (C#: `mods/mighty-bridge` copied next to the app as `claude-mods`): beside the exe
/// when it was put there, else in the Mac bundle's `Contents/Resources/mods/mighty-bridge` (the Swift app's place,
/// scripts/build-tauri-macos.sh), else — a development build only — the repository's own. A release build never reads the build
/// machine's paths: empty when neither place has it.
pub fn plugin_directory() -> String {
    let exe = std::env::current_exe().ok();
    let beside = exe.as_ref().and_then(|exe| exe.parent().map(|dir| dir.join("claude-mods")));
    // Only an exe inside an `.app` (`<bundle>.app/Contents/MacOS/<exe>`, review M-I 6): elsewhere no folder two levels up is read.
    let bundled = exe
        .as_deref()
        .and_then(mighty_updates::app_update::stage_mac::bundle_of)
        .map(|bundle| bundle.join("Contents").join("Resources").join("mods").join("mighty-bridge"));
    #[cfg(debug_assertions)]
    let repository = Some(Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("..").join("mods").join("mighty-bridge"));
    #[cfg(not(debug_assertions))]
    let repository = None;
    plugin_directory_among(beside, bundled, repository).map(|dir| dir.to_string_lossy().into_owned()).unwrap_or_default()
}

/// The first of the folders that holds `.claude-plugin`: beside the exe, in the Mac bundle's resources, or (development only) in the
/// repository.
fn plugin_directory_among(beside: Option<PathBuf>, bundled: Option<PathBuf>, repository: Option<PathBuf>) -> Option<PathBuf> {
    [beside, bundled, repository].into_iter().flatten().find(|dir| dir.join(".claude-plugin").is_dir())
}

/// The agents' files the app ships (C# `AgentFiles`: the Mac bundle's Resources/Agents, which the C# build copies next to its exe as
/// `Agents`; pi's `ask_user` extension and opencode's question instructions among them): beside the exe when they were put there, else
/// in the app's resources (`resources`: Tauri's resource folder, the Mac bundle's Contents/Resources), else — a development build only
/// — the repository's own. A release build never reads the build machine's paths: None when neither place has them.
pub fn agents_directory(resources: Option<PathBuf>) -> Option<PathBuf> {
    let beside = std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_path_buf));
    #[cfg(debug_assertions)]
    let repository = Some(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("macos").join("Sources").join("MightyCore").join("Resources"),
    );
    #[cfg(not(debug_assertions))]
    let repository = None;
    agents_directory_among(beside, resources, repository)
}

/// The first of the folders' `Agents` that is there: beside the exe, in the resources, in the repository's resources.
fn agents_directory_among(beside: Option<PathBuf>, resources: Option<PathBuf>, repository: Option<PathBuf>) -> Option<PathBuf> {
    [beside, resources, repository].into_iter().flatten().map(|folder| folder.join("Agents")).find(|dir| dir.is_dir())
}

/// The snapshot store, once [`start_checkpoints`] made it: a result's list reads a file's change from it (mighty.rs).
pub static SNAPSHOTS: std::sync::OnceLock<mighty_run::checkpoints::Snapshots> = std::sync::OnceLock::new();

/// Starts the snapshots of the runs' folders (mighty-run checkpoints.rs) in `folder`, git pointed at `hooks` for hooks.
pub fn start_checkpoints(host: &Arc<Host>, folder: PathBuf, hooks: PathBuf) {
    let Some(runs) = host.runs() else { return };
    let git = mighty_run::git::Git::new(host.processes.clone(), hooks);
    let snapshots = mighty_run::checkpoints::Snapshots::new(folder, git);
    let _ = SNAPSHOTS.set(snapshots.clone());
    let runtime = tauri::async_runtime::handle();
    runs.service.set_checkpoints(snapshots, runtime.inner());
}

impl Runs {
    /// The service over the loaded state, its events pumped into the host, and the CLIs read once in the background.
    pub fn start(
        host: &Arc<Host>,
        processes: Arc<dyn ProcessHost>,
        environment: Arc<dyn ShellEnvironment>,
        fake: Option<FakeClaude>,
        image_dir: PathBuf,
        catalog_dir: PathBuf,
        antigravity: AntigravityRuleLeases,
    ) -> Arc<Runs> {
        mighty_run::strings::set_language(host.locale.language());
        // The Mac reads its own words where the crates' copy has Windows-only ones (this PC, PowerShell: settings_words.rs).
        mighty_run::strings::use_words(crate::platform::words());
        // `{value:N0}` (the attachments' sizes) in the user's regional format, as C#'s current culture writes it.
        if let Some((separator, sizes)) = crate::platform::number_grouping() {
            mighty_run::strings::set_number_grouping(&separator, &sizes);
        }
        #[cfg(feature = "dev-harness")]
        let finder: Option<Finder> = fake.map(|fake| {
            let command = CliCommand::new(
                fake.binary.to_string_lossy().into_owned(),
                vec!["--replay-cli".into(), fake.recording.clone(), fake.record.to_string_lossy().into_owned()],
                Some("2.1.322"),
            );
            let pi = CliCommand::new(
                fake.binary.to_string_lossy().into_owned(),
                vec![
                    "--fake-cli".into(),
                    "pi".into(),
                    format!("{}-pi", fake.record.to_string_lossy()),
                    "--pi-fixture".into(),
                    crate::smoke_runs::pi_fixture().to_string_lossy().into_owned(),
                    "--pi-dialogs".into(),
                ],
                Some("0.70.2"),
            );
            let agy = CliCommand::new(
                fake.binary.to_string_lossy().into_owned(),
                vec![
                    "--fake-cli".into(),
                    "gemini".into(),
                    format!("{}-agy", fake.record.display()),
                    "--agy-denied".into(),
                    fake.agy_settings().to_string_lossy().into_owned(),
                ],
                Some("1.4.4"),
            );
            // The ACP step's opencode: the stand-in replaying a recorded session that asks one question (smoke_acp.rs).
            let state = crate::smoke_acp::state(&fake.record);
            let _ = std::fs::create_dir_all(&state);
            let opencode = CliCommand::new(
                fake.binary.to_string_lossy().into_owned(),
                vec![
                    "--fake-acp".into(),
                    state.to_string_lossy().into_owned(),
                    crate::smoke_acp::fixture().to_string_lossy().into_owned(),
                    // The question waits for the page's answer, as a real agent's does.
                    "--fake-await-answers".into(),
                ],
                Some("opencode v2.2.7"),
            );
            let finder: Finder = Arc::new(move |provider: String| {
                let found = match provider.as_str() {
                    "claude" => Some(command.clone()),
                    "pi" => Some(pi.clone()),
                    "gemini" => Some(agy.clone()),
                    "opencode" => Some(opencode.clone()),
                    _ => None,
                };
                Box::pin(async move { found })
            });
            finder
        });
        #[cfg(not(feature = "dev-harness"))]
        let finder: Option<Finder> = {
            let _ = fake;
            None
        };
        // A help capture or perf scenario (W9-H) starts no CLI: none is found, so the runtime read runs none; the help capture's demo CLIs
        // are overrides (`ProviderCatalog::set_override`), which come before the finder.
        #[cfg(feature = "dev-harness")]
        let special = host.smoke.as_ref().is_some_and(crate::smoke::Smoke::is_special);
        #[cfg(not(feature = "dev-harness"))]
        let special = false;
        let finder = finder.or_else(|| {
            special.then(|| {
                let none: Finder = Arc::new(|_provider: String| Box::pin(async { None }));
                none
            })
        });
        // The last list each CLI gave is kept in `catalog_dir` (C#'s storageDirectory, KeptModelLists), so a restart still has it.
        let catalog = Arc::new(ProviderCatalog::new(processes, environment, finder).with_storage(Some(catalog_dir)));
        let images = Arc::new(AgentImageCache::new(image_dir, IMAGE_CACHE_BYTES));
        let store: Arc<dyn ImageStore> = Arc::new(CacheStore(images.clone()));
        // A Claude run's Edit, Write and MultiEdit results keep the files' originals (files.rs, C# FileOriginalStore).
        let originals = crate::files::originals_hook(&host.app);
        providers::set_agent_files(agents_directory(tauri::Manager::path(&host.app).resource_dir().ok()));
        // The CLIs' own records are read, never written: a Codex run's session files, pi's session list.
        let home = history::user_home();
        let pi_home = home.clone();
        let memory_host = Arc::downgrade(host);
        // Temporary Antigravity rules a crash left in agy's settings go first (C# `InitializeAsync`, M/AppStore.swift:433-434): the run
        // service takes them out as it starts and no agy run starts before (C2).
        let configure: Configure = Box::new(move |options| {
            let lookup_host = memory_host.clone();
            options.graph =
                Some(Arc::new(TrackerFactory { codex_sessions: Some(codex_sessions::watcher_factory(home)), ..TrackerFactory::default() }));
            options.pi_sessions = Some(Arc::new(move |id, workspace, environment| {
                let environment: HashMap<String, String> = environment.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                mighty_history::pi::session_file(id, workspace, &pi_home, &environment).is_some()
            }));
            options.images = Some(store);
            options.file_originals = originals;
            options.antigravity_rules = Some(antigravity);
            options.hermes_session = Some(Arc::new(hermes_session));
            options.memory_for_workspace = Some(Arc::new(move |workspace| {
                let host = lookup_host.upgrade()?;
                crate::shared_memory::context_for_pane(&host, Some(workspace))
            }));
        });
        let snapshot = host.view().snapshot.clone();
        let (service, mut events) = RunService::new(
            snapshot,
            catalog.clone(),
            RunServiceOptions {
                plugin_directory: plugin_directory(),
                permission_card: true,
                // A remote workspace's panes run on their host (remote.rs, W8-D's client).
                remote: crate::remote::runner(host),
                configure: Some(configure),
                admitting: None,
                // A Claude or Codex run's MCP servers (agent_io.rs, W7-C) and delegation's run ledger and worktrees (delegation.rs, W7-D).
                binding: crate::agent_io::factory(host),
                delegation: crate::delegation::run_hooks(host),
            },
        );
        service.set_body_loader(crate::bodies::loader(host.store_handle()));
        service.set_body_evictor(crate::bodies::evictor(host.store_handle()));
        let runs = Arc::new(Runs { service: Arc::new(service), catalog, images, runtime: Mutex::new(HashMap::new()) });
        host.attach(runs.clone());
        let pump = host.clone();
        tauri::async_runtime::spawn(async move {
            while let Some(event) = events.recv().await {
                let hermes = hermes_run_ended(&pump, &event);
                // C# MainWindow.cs:88's order: `RecordProviderActivity` (updates.rs, W6-D) and `ReceiveLoginSignal` (accounts.rs, W6-C)
                // before the pane takes the event, `HandleRunEventForNotification` (notifications.rs, W6-A) after it; a request about to
                // start goes to the login recovery (C# `service.RequestStarting`, MainWindow.LoginRecovery.cs:49).
                // Only what the notification needs is kept past the pane's taking the event (never the whole event: its logs and
                // usage are not copied for every event).
                let mut status = None;
                // MC-9: the pane the event is about (its state is read again once the window took it) and a log entry's text (new
                // output for `mighty pane wait-output`).
                let touched: Option<(String, Option<String>)> = match &event {
                    ServiceEvent::Run { event: run, .. } => {
                        let text = (run.kind == "log").then(|| run.entry.as_ref().map(|e| e.text().to_string())).flatten();
                        Some((run.session_id.clone(), text))
                    }
                    ServiceEvent::ToolPermission(request) => Some((request.run_id.clone(), None)),
                    _ => None,
                };
                if let ServiceEvent::Run { event: run, .. } = &event {
                    crate::updates::on_run_event(&pump, run);
                    crate::accounts::on_run_event(&pump, run);
                    // Delegation's offers and queued merges on a run's start or end (C# DesktopService.cs:261-273; delegation.rs, W7-D).
                    crate::delegation::on_run_event(&pump, run);
                    if run.kind == "status" {
                        status = Some((run.session_id.clone(), run.status.clone()));
                    }
                }
                if let ServiceEvent::RequestStarting(request) = &event {
                    crate::accounts::on_request_starting(&pump, request);
                }
                pump.service_event(event);
                if let Some((pane, text)) = &touched {
                    crate::control::on_run_event(&pump, pane, text.as_deref());
                }
                if let Some((session, status)) = &status {
                    crate::notifications::on_status(&pump, session, status.as_deref());
                    if let Some(ended @ ("completed" | "stopped" | "error")) = status.as_deref() {
                        let cost = crate::usage_report::on_run_finished(&pump, session);
                        crate::control::run_finished(&pump, session, ended, cost);
                    }
                }
                // C# `RefreshModels(sessionId, invalidate: true)` through the model lists' own refresh (models.rs), which reconciles the
                // pane with the new list and asks no CLI in a smoke.
                if let Some(id) = hermes {
                    crate::models::refresh(&pump, &pump.view().snapshot, &id, true);
                }
            }
        });
        // The CLIs read once in the background; never in a help capture or perf scenario (C#'s smoke skips `RefreshRuntime`: the help
        // capture keeps no runtime, the perf scenario takes the fixture one).
        if special {
            return runs;
        }
        let reader = host.clone();
        let reading = runs.clone();
        tauri::async_runtime::spawn(async move {
            let info = reading.catalog.get_runtime(false, None).await;
            {
                let mut runtime = crate::host::lock(&reading.runtime);
                for provider in info.providers {
                    runtime.insert(provider.id.clone(), provider);
                }
            }
            reader.log.line(&format!(
                "runtime read: {}",
                crate::host::lock(&reading.runtime)
                    .values()
                    .map(|p| format!("{} {}", p.id, if p.available { "available" } else { "missing" }))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
            reader.panes_changed();
        });
        runs
    }

    /// `PaneRuntime(provider)` as the run engine read it.
    pub fn runtime(&self, provider: &str) -> Option<ProviderRuntime> {
        crate::host::lock(&self.runtime).get(provider).cloned()
    }

    /// C#'s smoke runtime (`new ProviderRuntime(p, Name(p), true, "fixture", "isolated fixture", Fallback(p), Capabilities(p))` for every
    /// provider): each one available with its built-in list, read from no CLI (the help capture and the perf scenario, W9-H).
    #[cfg(feature = "dev-harness")]
    pub fn use_fixture_runtime(&self) {
        let mut runtime = crate::host::lock(&self.runtime);
        for provider in mighty_core::wire::PROVIDERS {
            runtime.insert(
                provider.to_string(),
                ProviderRuntime {
                    id: provider.to_string(),
                    name: providers::name(provider).to_string(),
                    available: true,
                    version: Some("fixture".into()),
                    detail: "isolated fixture".into(),
                    model_catalog: providers::fallback(provider),
                    capabilities: providers::capabilities(provider, None),
                },
            );
        }
    }

    /// A provider's newer reading taken as its runtime (C# `runtime = ProviderCatalog.With(runtime, refreshed)`, models.rs).
    pub fn keep(&self, value: ProviderRuntime) {
        crate::host::lock(&self.runtime).insert(value.id.clone(), value);
    }

    /// The runtime as the composer reads it (mighty-composer's mirror of C# `ProviderRuntime`).
    pub fn composer_runtime(&self, provider: &str) -> Option<composer_runtime::ProviderRuntime> {
        self.runtime(provider).map(|r| composer_runtime(&r))
    }

    /// `PaneRuntime(provider)?.ModelCatalog ?? ProviderCatalog.Fallback(provider)`.
    pub fn catalog_of(&self, provider: &str) -> ModelCatalog {
        self.runtime(provider).map(|r| r.model_catalog).unwrap_or_else(|| providers::fallback(provider))
    }

    /// `ProviderCatalog.Efforts(pane.Provider, pane.Model, catalog, RegisteredModelsFor(...))`.
    pub fn efforts(&self, pane: &RunSession, workspace: Option<&Workspace>, snapshot: &AppSnapshot) -> Vec<String> {
        let registered = registered_models_for(pane.provider(), workspace, snapshot);
        providers::efforts(pane.provider(), pane.model(), &self.catalog_of(pane.provider()), Some(&registered))
    }
}

/// mighty-run's `ProviderRuntime` as mighty-composer reads it (the one place the two meet).
pub fn composer_runtime(value: &ProviderRuntime) -> composer_runtime::ProviderRuntime {
    let caps = &value.capabilities;
    composer_runtime::ProviderRuntime {
        name: value.name.clone(),
        available: value.available,
        detail: value.detail.clone(),
        catalog: composer_runtime::ModelCatalog {
            source: value.model_catalog.source.clone(),
            models: value
                .model_catalog
                .models
                .iter()
                .map(|m| composer_runtime::CatalogModel {
                    value: m.value.clone(),
                    display_name: m.display_name.clone(),
                    description: m.description.clone(),
                    resolved_model: m.resolved_model.clone(),
                })
                .collect(),
            // `ModelListNotices.Notice`: the built-in list after a read that failed, or the last list the CLI gave, kept.
            notice: mighty_run::kept_lists::notice(&value.model_catalog, None)
                .map(|n| composer_runtime::ModelListNotice { line: n.line, help: n.help }),
        },
        capabilities: composer_runtime::ProviderCapabilities {
            effort: caps.effort,
            permission_modes: caps.permission_modes.clone(),
            max_turns: caps.max_turns,
            max_budget_usd: caps.max_budget_usd,
            resume: caps.resume,
            fast_mode: caps.fast_mode,
            web_search: caps.web_search,
            network_access: caps.network_access,
            attachments: caps.attachments,
        },
    }
}

/// C# `RegisteredModelsFor` (MainWindow.cs:781): the workspace's registered models for the provider, then the app's it does not name.
pub fn registered_models_for(provider: &str, workspace: Option<&Workspace>, snapshot: &AppSnapshot) -> Vec<RegisteredModelEntry> {
    let entries = |config: Option<&mighty_core::model::ModelDefaultsConfig>| -> Vec<RegisteredModelEntry> {
        let side = config.and_then(|c| if provider == "codex" { c.codex.as_ref() } else { c.claude.as_ref() });
        side.and_then(|s| s.registered_models.as_ref()).map(|list| list.iter().flatten().cloned().collect()).unwrap_or_default()
    };
    let from_workspace = entries(workspace.and_then(|w| w.model_defaults.as_ref()));
    let from_app = entries(snapshot.model_defaults.as_ref());
    if from_app.is_empty() {
        return from_workspace;
    }
    if from_workspace.is_empty() {
        return from_app;
    }
    let names: Vec<Option<String>> = from_workspace.iter().map(|e| e.name.clone()).collect();
    from_workspace.iter().cloned().chain(from_app.into_iter().filter(|e| !names.contains(&e.name))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The agents' files: beside the exe first, then the app's resources (the Mac bundle's Contents/Resources), then the repository's —
    /// which a release build does not pass, so without the other two it finds none rather than the build machine's.
    #[test]
    fn the_agents_files_are_beside_the_exe_then_in_the_resources_and_the_repository_only_in_development() {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let root = std::env::temp_dir().join(format!("mighty-agents-{}-{nanos}", std::process::id()));
        let (exe, resources, repository) = (root.join("exe"), root.join("Resources"), root.join("repository"));
        for folder in [&exe, &resources, &repository] {
            std::fs::create_dir_all(folder).unwrap();
        }
        let among = |beside: &Path, repository: Option<&Path>| {
            agents_directory_among(Some(beside.to_path_buf()), Some(resources.clone()), repository.map(Path::to_path_buf))
        };
        assert_eq!(among(&exe, None), None, "a release build without the files anywhere finds none");
        std::fs::create_dir_all(repository.join("Agents")).unwrap();
        assert_eq!(among(&exe, Some(&repository)), Some(repository.join("Agents")), "a development build falls back to the repository");
        assert_eq!(among(&exe, None), None, "a release build never reads the repository");
        std::fs::create_dir_all(resources.join("Agents")).unwrap();
        assert_eq!(among(&exe, Some(&repository)), Some(resources.join("Agents")), "the resources come before the repository");
        std::fs::create_dir_all(exe.join("Agents")).unwrap();
        assert_eq!(among(&exe, Some(&repository)), Some(exe.join("Agents")), "beside the exe comes first");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// P0-SEC-3: the mighty-bridge plugin follows the same rule as `agents_directory`: beside the exe, then bundled in the Mac app,
    /// and the repository only in development builds (never leaking `CARGO_MANIFEST_DIR` in release).
    #[test]
    fn the_plugin_directory_is_beside_the_exe_then_bundled_and_the_repository_only_in_development() {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let root = std::env::temp_dir().join(format!("mighty-plugin-{}-{nanos}", std::process::id()));
        let (beside, bundled, repository) = (root.join("claude-mods"), root.join("bundled"), root.join("repository"));
        for folder in [&beside, &bundled, &repository] {
            std::fs::create_dir_all(folder).unwrap();
        }
        let among = |repo: Option<&Path>| plugin_directory_among(Some(beside.clone()), Some(bundled.clone()), repo.map(Path::to_path_buf));
        assert_eq!(among(None), None, "a release build without the plugin anywhere finds none");
        std::fs::create_dir_all(repository.join(".claude-plugin")).unwrap();
        assert_eq!(among(Some(&repository)), Some(repository.clone()), "a development build falls back to the repository");
        assert_eq!(among(None), None, "a release build never reads the repository");
        std::fs::create_dir_all(bundled.join(".claude-plugin")).unwrap();
        assert_eq!(among(Some(&repository)), Some(bundled.clone()), "the bundle comes before the repository");
        std::fs::create_dir_all(beside.join(".claude-plugin")).unwrap();
        assert_eq!(among(Some(&repository)), Some(beside.clone()), "beside the exe comes first");
        let _ = std::fs::remove_dir_all(&root);
    }
}
