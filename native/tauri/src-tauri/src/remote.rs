//! Remote workspaces v2 in the window (C# Core/DesktopService.Remote.cs: attach, the host's options, the runtime cache, the start, the
//! answer; WinUI/MainWindow.RemoteWorkspaces.cs: the client and the host, admission, the key's copy; MainWindow.WebShell.Shell.cs:100,
//! 118, 343, 487-503) over mighty-remote's `remote::host::RemoteHost` and `remote::client::RemoteClient`. Owner: W8-D.
//!
//! The client is made the first time anything asks (runs.rs reads [`runner`] as it makes the run service, before [`start`]); what it
//! needs of the platform (the key vault, this PC's Tailscale addresses) comes at [`start`], which also makes the sharing host. Sharing
//! starts only from Settings' Start button and only where the app may listen ([`Host::may_listen`]), on this PC's Tailscale addresses; a
//! smoke shares on loopback port 0 inside this process (smoke_remote.rs). `remote-connections.json` and the vault's key files are
//! written only with `--allow-state-write` (else the connections and keys live in memory for the launch); every key goes only in
//! `Authorization`, never in a URL, a log or `Debug`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use mighty_core::model::{AppSnapshot, RunSession, Workspace};
use mighty_core::shell::RemoteConnection;
use mighty_platform::fake::MemoryVault;
use mighty_platform::{BoxFuture, Clipboard, Network, Platform, Proxy, ProxyResolver, SecretVault};
use mighty_remote::remote::client::{ClientOptions, LocalAddresses, RemoteClient, RouteCheck};
use mighty_remote::remote::features;
use mighty_remote::remote::host::{Addresses, HostOptions, HostRuntime, RemoteHost, host_runner};
use mighty_remote::remote::strings;
use mighty_remote::remote::wire::{RemoteConnectionInfo, RemoteConnectionStatus, RemoteHostState, protocol};
use mighty_run::model::{RuntimeInfo, StartRunRequest};
use mighty_run::service::RemoteRunner;
use serde_json::Value;

use crate::host::{Host, Reply, lock};

/// How often the open page reads this PC's Tailscale addresses again (C#'s three-second poll).
const TAILSCALE_READ_EVERY: Duration = Duration::from_secs(3);

/// The client's key vault: in memory until [`start`] hands it the platform's (only with `--allow-state-write`, never in a smoke).
#[derive(Default)]
struct KeyVault {
    platform: OnceLock<Arc<dyn SecretVault>>,
    memory: MemoryVault,
}

impl KeyVault {
    fn vault(&self) -> &dyn SecretVault {
        match self.platform.get() {
            Some(vault) => vault.as_ref(),
            None => &self.memory,
        }
    }
}

impl SecretVault for KeyVault {
    fn load(&self, id: &str) -> Option<String> {
        self.vault().load(id)
    }
    fn save(&self, id: &str, secret: &str) -> mighty_platform::Result<()> {
        self.vault().save(id, secret)
    }
    fn delete(&self, id: &str) -> mighty_platform::Result<()> {
        self.vault().delete(id)
    }
}

/// The system's proxy for a paired connection's relay link: the platform's once [`start`] has it, straight before (and in a smoke).
struct AppProxy(Weak<RemoteApp>);

impl ProxyResolver for AppProxy {
    fn resolve(&self, url: &str) -> BoxFuture<'_, mighty_platform::Result<Proxy>> {
        let resolver = self.0.upgrade().and_then(|app| app.proxy.get().cloned());
        let url = url.to_string();
        Box::pin(async move {
            match resolver {
                Some(resolver) => resolver.resolve(&url).await,
                None => Ok(Proxy::Direct),
            }
        })
    }
}

/// What the page keeps between its reads (C# `remoteBusy`, `remoteError`, `remoteSharingShown` and the page's Tailscale reading).
#[derive(Default)]
pub struct Page {
    /// One remote action at a time.
    pub busy: bool,
    pub error: Option<String>,
    /// The page last showed sharing on: a sharing that ended without Stop (a listener that died) is said on the page.
    pub sharing_shown: bool,
    /// This PC's Tailscale addresses as last read.
    pub tailscale: Vec<String>,
    read_at: Option<Instant>,
    reading: bool,
}

/// The window's remote workspaces: the client, the sharing host and the page's state.
pub struct RemoteApp {
    pub client: RemoteClient,
    vault: Arc<KeyVault>,
    /// This PC's network and whether its Tailscale adapter must be named `utun` (the Mac); set at start.
    network: OnceLock<(Arc<dyn Network>, bool)>,
    /// The system's proxy for paired connections' relay links; set at start.
    proxy: OnceLock<Arc<dyn ProxyResolver>>,
    host: OnceLock<RemoteHost>,
    runtime: OnceLock<Arc<HostRuntime>>,
    /// `remoteHostRuns`: another computer's runs going on this PC; while any does, this PC's CLIs and plugins are not updated.
    host_runs: AtomicUsize,
    smoke: bool,
    pub page: Mutex<Page>,
    /// The key last copied while it may still be on the clipboard (emptied at quit if it is). Never logged.
    copied: Arc<Mutex<Option<String>>>,
}

static APP: Mutex<Option<Arc<RemoteApp>>> = Mutex::new(None);

/// The window's remote workspaces, made the first time anything asks (C# `InitializeRemoteWorkspaces`' client half).
pub fn app(host: &Arc<Host>) -> Arc<RemoteApp> {
    let mut app = lock(&APP);
    if let Some(app) = app.as_ref() {
        return app.clone();
    }
    let smoke = host.smoke.is_some();
    let made = Arc::new_cyclic(|me: &Weak<RemoteApp>| {
        let (route, local, proxy) = (me.clone(), me.clone(), me.clone());
        let route_check: RouteCheck = Arc::new(move |ip: &str| {
            let app = route.upgrade().ok_or_else(|| "closing".to_string())?;
            Ok(mighty_remote::remote::address::through_tailscale(ip, &|| app.tailscale()))
        });
        let local_addresses: LocalAddresses = Arc::new(move || local.upgrade().ok_or_else(|| "closing".to_string())?.tailscale());
        let options = ClientOptions {
            // Only a smoke run passes loopback in place of Tailscale (and then asks no adapter).
            allow_loopback_for_tests: smoke,
            route_check: (!smoke).then_some(route_check),
            local_addresses: (!smoke).then_some(local_addresses),
            writes: host.writes() && !smoke,
            // A paired connection's relay link goes through the system's proxy, as the phone remote's does.
            relay_proxy: Some(Arc::new(AppProxy(proxy))),
            client_name: client_name(),
            ..ClientOptions::default()
        };
        let approving = Arc::downgrade(host);
        let vault = Arc::new(KeyVault::default());
        RemoteApp {
            client: RemoteClient::new(
                Arc::new(move |connection, workspace, name, path, host_name| {
                    approving.upgrade()?.approve_remote(connection, workspace, name, path, host_name)
                }),
                host.data_dir.clone(),
                vault.clone(),
                options,
            ),
            vault,
            network: OnceLock::new(),
            proxy: OnceLock::new(),
            host: OnceLock::new(),
            runtime: OnceLock::new(),
            host_runs: AtomicUsize::new(0),
            smoke,
            page: Mutex::new(Page::default()),
            copied: Arc::default(),
        }
    });
    *app = Some(made.clone());
    made
}

/// The app's remote workspaces when they were made (never makes them).
fn existing() -> Option<Arc<RemoteApp>> {
    lock(&APP).clone()
}

/// The app's remote workspaces when they were made (never makes them): the remote mirror's frame parts read the client through this with
/// the view's lock held (mirror.rs, W9-J).
pub fn app_if_made() -> Option<Arc<RemoteApp>> {
    existing()
}

impl RemoteApp {
    /// `TailscaleAddresses.Current()`: this PC's Tailscale addresses now (empty while Tailscale is off), Err when no adapter could be
    /// read; loopback in a smoke.
    pub fn tailscale(&self) -> Result<Vec<String>, String> {
        if self.smoke {
            return Ok(vec!["127.0.0.1".into()]);
        }
        let Some((network, needs_utun)) = self.network.get() else { return Ok(vec![]) };
        let adapters = network.adapters().map_err(|e| e.to_string())?;
        let found = mighty_remote::tailscale::select(&adapters, *needs_utun);
        if found.unreadable {
            return Err(strings::local_tailscale_off());
        }
        Ok(found.addresses.into_iter().map(mighty_remote::tailscale::canonical).collect())
    }

    /// The sharing host, once [`start`] made it.
    pub fn host(&self) -> Option<&RemoteHost> {
        self.host.get()
    }

    /// The page's Tailscale reading, read again off the window's thread when it is older than three seconds (C# `Poll`).
    pub fn page_tailscale(self: &Arc<Self>, host: &Arc<Host>) -> Vec<String> {
        let mut page = lock(&self.page);
        if !page.reading && page.read_at.is_none_or(|at| at.elapsed() >= TAILSCALE_READ_EVERY) {
            page.reading = true;
            let (app, host) = (self.clone(), host.clone());
            tauri::async_runtime::spawn_blocking(move || {
                // An adapter that cannot be read reads as no Tailscale (C# `AdaptersUnreadableException` → []).
                let read = app.tailscale().unwrap_or_default();
                {
                    let mut page = lock(&app.page);
                    page.reading = false;
                    page.read_at = Some(Instant::now());
                    let changed = page.tailscale != read;
                    page.tailscale = read;
                    if !changed {
                        return;
                    }
                }
                if !host.closing() {
                    crate::native::touch(&host, crate::settings::SURFACE);
                }
            });
        }
        page.tailscale.clone()
    }
}

/// `AppVersion.Normalized(value)`: `v` dropped, then `digits(.digits){0,3}(-pre)?(+build)?` of at most 64 characters, else none.
fn normalized_version(value: &str) -> Option<String> {
    let text = value.trim();
    let text = text.strip_prefix(['v', 'V']).unwrap_or(text);
    let tail_ok = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
    let (rest, build) = match text.split_once('+') {
        Some((rest, build)) => (rest, Some(build)),
        None => (text, None),
    };
    let (core, pre) = match rest.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (rest, None),
    };
    let parts: Vec<&str> = core.split('.').collect();
    let fits = text.chars().count() <= 64
        && (1..=4).contains(&parts.len())
        && parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
        && pre.is_none_or(tail_ok)
        && build.is_none_or(|b| tail_ok(b) && !b.contains('+'));
    fits.then(|| text.to_string())
}

/// The machine's name as the host gives it (C# `Environment.MachineName`), else the OS's name.
fn machine_name(platform: &Platform) -> String {
    let name = std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).unwrap_or_default();
    if name.trim().is_empty() { strings::host_os(Some(platform.profile.m1_platform)) } else { name }
}

/// How a host this computer pairs with names it in its device list: the machine's name, else the app's.
fn client_name() -> String {
    let name = crate::mobile::machine_name();
    if name.trim().is_empty() { "Mighty Code".into() } else { name }
}

/// The system folder a sharing host's commands take Windows PowerShell from (C# `Environment.SpecialFolder.System`); none on the Mac.
fn system_folder(platform: &Platform) -> Option<String> {
    if platform.profile.os != mighty_platform::Os::Windows {
        return None;
    }
    crate::platform::setup_files().system_folder().or_else(|| {
        let root = std::env::var("SystemRoot").ok().filter(|r| !r.trim().is_empty()).unwrap_or_else(|| r"C:\Windows".into());
        Some(format!("{}\\System32", root.trim_end_matches(['\\', '/'])))
    })
}

/// `RemoteHostAdmission(request)`: why this PC cannot take another computer's run now (its CLIs or plugins are changing, or Claude's
/// models are being reloaded); None when it can. Asked on a request's task, so it only reads.
fn admission(host: &Host, request: &StartRunRequest) -> Option<String> {
    if host.closing() {
        return Some(host.locale.get("common.appClosing"));
    }
    if request.kind == "shell" {
        return None;
    }
    let activity = crate::updates::of(host).map(|u| u.activity()).unwrap_or_default();
    if activity.any_cli_update_running() || activity.manual_provider.is_some() {
        return Some(host.locale.get_with("run.blocked.updatingCLI", &[("provider", mighty_run::providers::name(&request.provider))]));
    }
    if activity.plugin_operation_running && matches!(request.provider.as_str(), "claude" | "codex") {
        return Some(host.locale.get("run.blocked.changingPlugins"));
    }
    if request.provider == "claude" && crate::accounts::claude_model_resetting(host) {
        return Some(host.locale.get("composer.model.reloadingClaude"));
    }
    None
}

/// Remote workspaces' start with the app (C# `InitializeRemoteWorkspaces`): the platform's vault (with `--allow-state-write`) and
/// network for the client, the saved connections read off the window's thread, and the sharing host made (sharing is off at start).
pub fn start(host: &Arc<Host>, platform: &Platform) {
    let app = app(host);
    let smoke = host.smoke.is_some();
    let ports = crate::platform::remote_ports(platform, smoke);
    let _ = app.network.set((ports.network.clone(), platform.profile.tailscale_needs_utun));
    let _ = app.proxy.set(ports.proxy.clone());
    if host.writes() && !smoke {
        let _ = app.vault.platform.set(platform.secrets.clone());
    }
    // A connection that comes up or goes down redraws everything that names it: the sidebar, the header's badge, the status bar and the
    // remote panes (the shell model reads `connections`), and the page.
    let changed = Arc::downgrade(host);
    app.client.on_changed(Arc::new(move || {
        if let Some(host) = changed.upgrade().filter(|h| !h.closing()) {
            host.panes_changed();
            host.shell_dirty();
            crate::native::touch(&host, crate::settings::SURFACE);
        }
    }));
    let reading = app.client.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _ = reading.connections();
    });
    let Some(runs) = host.runs().cloned() else { return };
    let version = normalized_version(&host.version).unwrap_or_else(|| "0.1.0".into());
    let reader = runs.catalog.clone();
    let runtime = Arc::new(HostRuntime::new(
        Arc::new(move || {
            let catalog = reader.clone();
            Box::pin(async move { Ok(catalog.get_runtime(false, None).await) })
        }),
        &version,
    ));
    let _ = app.runtime.set(runtime.clone());
    let me = Arc::downgrade(host);
    let (workspaces, resolving, admitting, counting) = (me.clone(), me.clone(), me.clone(), me);
    let (catalog, plugins, system) = (runs.catalog.clone(), crate::runs::plugin_directory(), system_folder(platform));
    let addresses: Option<Addresses> = (!smoke).then(|| {
        let app = Arc::downgrade(&app);
        let read: Addresses = Arc::new(move || app.upgrade().ok_or_else(|| "closing".to_string())?.tailscale());
        read
    });
    let options = HostOptions {
        workspaces: Arc::new(move || workspaces.upgrade().map(|h| h.view().snapshot.workspaces().to_vec()).unwrap_or_default()),
        resolve_local: Arc::new(move |id: String| {
            let runs = resolving.upgrade().as_ref().and_then(|h| h.runs().cloned());
            Box::pin(async move {
                let closing = || mighty_run::strings::get("common.appClosing");
                let Some(runs) = runs else { return Err(closing()) };
                // The check asks the disk whether the folder is there: off the runtime's workers.
                tokio::task::spawn_blocking(move || runs.service.resolve_shared(&id)).await.unwrap_or_else(|_| Err(closing()))
            })
        }),
        runtime: Arc::new(move || {
            let reading = runtime.read();
            Box::pin(async move { reading.wait().await.unwrap_or_else(|_| empty_runtime()) })
        }),
        create_runner: Arc::new(move |sinks| host_runner(catalog.clone(), &plugins, sinks, system.clone())),
        host_name: machine_name(platform),
        admission: Arc::new(move |request| match admitting.upgrade() {
            Some(host) => admission(&host, request),
            None => Some(mighty_run::strings::get("common.appClosing")),
        }),
        runs_changed: Arc::new(move |count| {
            if let Some(app) = existing() {
                app.host_runs.store(count, Ordering::SeqCst);
            }
            if let Some(host) = counting.upgrade().filter(|h| !h.closing()) {
                // Off the host's task: the page and the updates read the view's lock.
                tauri::async_runtime::spawn(async move {
                    crate::native::touch(&host, crate::settings::SURFACE);
                    crate::updates::notify(&host);
                });
            }
        }),
        allow_loopback_for_tests: smoke,
        heartbeat: protocol::HEARTBEAT_TIMEOUT,
        addresses,
        clock: mighty_remote::remote::host::monotonic(),
        windows_host: !platform.profile.remote_confines_codex,
        decodes_paths: platform.profile.remote_decodes_request_paths,
        graph_runs: platform.profile.remote_graph_runs,
        // A client's continuation adds its card's rules to this PC's agy settings, counted with this PC's own panes.
        antigravity_rules: Some(runs.service.antigravity_rules().clone()),
        system_folder: system_folder(platform),
        network: ports.network,
    };
    let _ = app.host.set(RemoteHost::new(options));
    restore_sharing(host);
}

/// Where this PC's sharing choice is kept between launches: whether it shared, what and on which port, and whether paired computers
/// were let in through the relay. Without it every launch started with sharing off, so a computer paired through the relay was refused
/// ("상대 컴퓨터가 지금 공유 중이 아닙니다") after this PC restarted until someone pressed Start here again.
const SHARING_FILE: &str = "remote-sharing.json";

/// The sharing choice [`SHARING_FILE`] holds. Full access is never kept: it is off whenever sharing starts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SavedSharing {
    pub sharing: bool,
    pub workspace_ids: Vec<String>,
    pub port: u16,
    pub relay: bool,
}

impl SavedSharing {
    pub fn encode(&self) -> String {
        serde_json::json!({
            "version": 1,
            "sharing": self.sharing,
            "workspaceIds": self.workspace_ids,
            "port": self.port,
            "relay": self.relay,
        })
        .to_string()
    }

    /// The file's choice; none for a file that is not one (it is then as if sharing was off).
    pub fn decode(text: &str) -> Option<SavedSharing> {
        let value: Value = serde_json::from_str(text).ok()?;
        if value.get("version").and_then(Value::as_i64) != Some(1) {
            return None;
        }
        let workspace_ids = value
            .get("workspaceIds")
            .and_then(Value::as_array)
            .map(|ids| {
                ids.iter()
                    .filter_map(Value::as_str)
                    .filter(|id| mighty_core::wire::identifier(id))
                    .take(protocol::MAXIMUM_SHARED_WORKSPACES)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        Some(SavedSharing {
            sharing: value.get("sharing").and_then(Value::as_bool).unwrap_or(false),
            workspace_ids,
            port: value.get("port").and_then(Value::as_u64).and_then(|p| u16::try_from(p).ok()).unwrap_or(protocol::DEFAULT_PORT),
            relay: value.get("relay").and_then(Value::as_bool).unwrap_or(false),
        })
    }
}

/// Keeps the sharing choice as it is now, after the person here started or stopped sharing or flipped the pairing switch (only where
/// the app writes its profile, never in a smoke). Quitting does not touch it: the next launch shares again as before.
pub fn remember_sharing(host: &Arc<Host>, port: Option<u16>) {
    if !host.writes() || host.smoke.is_some() {
        return;
    }
    let Some(sharing) = app(host).host().cloned() else { return };
    let state = sharing.state();
    let path = host.data_dir.join(SHARING_FILE);
    let before = std::fs::read_to_string(&path).ok().and_then(|text| SavedSharing::decode(&text)).unwrap_or_default();
    let saved = SavedSharing {
        sharing: state.enabled,
        workspace_ids: if state.enabled { state.workspace_ids.clone() } else { before.workspace_ids.clone() },
        port: port.or(state.port).unwrap_or(before.port.max(protocol::DEFAULT_PORT)),
        relay: sharing.relay(),
    };
    if saved == before && path.exists() {
        return;
    }
    if mighty_core::store::atomic_write(&path, saved.encode().as_bytes(), None, mighty_core::store::portable_replace).is_err() {
        host.log.line("remote: the sharing choice could not be saved");
    }
}

/// How often, and how far apart, a restored sharing that could not start yet (Tailscale still coming up after a restart) is tried.
const RESTORE_TRIES: u32 = 10;
const RESTORE_GAP: Duration = Duration::from_secs(30);

/// Shares again as the last launch left it ([`SHARING_FILE`]): the pairing switch first, then the workspaces still listed here on the
/// same port, tried a few times while this PC's network comes up. Only where the app may listen, never in a smoke.
fn restore_sharing(host: &Arc<Host>) {
    if !host.may_listen() || host.smoke.is_some() {
        return;
    }
    let Some(saved) = std::fs::read_to_string(host.data_dir.join(SHARING_FILE)).ok().and_then(|text| SavedSharing::decode(&text)) else {
        return;
    };
    let Some(sharing) = app(host).host().cloned() else { return };
    sharing.set_relay(saved.relay);
    if !saved.sharing {
        return;
    }
    let host = host.clone();
    tauri::async_runtime::spawn(async move {
        for attempt in 1..=RESTORE_TRIES {
            if host.closing() || sharing.state().enabled {
                break;
            }
            let ids: Vec<String> = {
                let view = host.view();
                let local: Vec<&str> = view.snapshot.workspaces().iter().filter(|w| w.remote.is_none()).map(|w| w.id()).collect();
                saved.workspace_ids.iter().filter(|id| local.contains(&id.as_str())).cloned().collect()
            };
            if ids.is_empty() {
                break;
            }
            match sharing.start(&ids, saved.port).await {
                Ok(_) => {
                    host.log.line("remote: sharing restored as the last launch left it");
                    break;
                }
                Err(error) if attempt == RESTORE_TRIES => {
                    host.log.line(&format!("remote: the last launch's sharing did not start again: {error}"));
                }
                Err(_) => tokio::time::sleep(RESTORE_GAP).await,
            }
        }
        sync_relay(&host).await;
        crate::native::touch(&host, crate::settings::SURFACE);
    });
}

/// A runtime when this PC's CLIs could not be read (the host still answers /info with no CLI).
fn empty_runtime() -> RuntimeInfo {
    RuntimeInfo {
        platform: String::new(),
        app_version: String::new(),
        claude_available: false,
        claude_version: None,
        model_catalog: None,
        providers: vec![],
        mods: None,
    }
}

/// `ForgetHostRuntime`: the CLIs were looked for again (after an update, a sign-in or a reset): a sharing host reads them again too.
pub fn forget_host_runtime(host: &Host) {
    let _ = host;
    if let Some(runtime) = existing().and_then(|app| app.runtime.get().cloned()) {
        runtime.forget();
    }
}

/// `ShutdownRemoteWorkspacesAsync`: sharing ends with the app (every remote run here stops and the key is gone); the client's runs end
/// with the run service, which disposes it. A copied key still on the clipboard goes with the app too.
pub fn shutdown(host: &Host) {
    let Some(app) = existing() else { return };
    if let Some(sharing) = app.host.get().cloned() {
        tauri::async_runtime::block_on(sharing.dispose());
    }
    let copied = lock(&app.copied).take();
    if let Some(key) = copied
        && host.smoke.is_none()
    {
        clear_if_holding(crate::platform::clipboard().as_ref(), &key);
    }
}

/// `connectRemote` (the sidebar's and the dashboard's connect buttons): C# `OpenRemoteSettings`. Answers null.
pub async fn connect_remote(host: Arc<Host>, args: Value) -> Reply {
    let _ = args;
    open_settings(&host);
    Ok(Value::Null)
}

/// `OpenRemoteSettings`: Settings on its 원격 연결 page, on 다른 컴퓨터 연결 where the connections are (the dashboard's connect, the
/// add-pane menus' Connect to Another Computer…, a remote workspace's 원격 연결 설정…); a Settings already open turns to that page;
/// while another dialog is open nothing happens.
pub fn open_settings(host: &Arc<Host>) {
    crate::settings::remote::show_tab(crate::settings::remote::Tab::Connect);
    if crate::settings::select(host, "remote") || host.view().dialog_open {
        return;
    }
    let _ = host.view().update(|state| state.settings_pane = Some("remote".into()));
    crate::settings::open(host, Some("remote"));
}

/// C# `RemoteConnection(workspace)` for the frame model, by connection id (connected, the host's OS from its last answer). Read with the
/// state locked: it reads the client's own state, never `host.view()`.
pub fn connections(host: &Host) -> HashMap<String, RemoteConnection> {
    let _ = host;
    let Some(app) = existing() else { return HashMap::new() };
    app.client
        .connections()
        .into_iter()
        .map(|c| {
            let shown = RemoteConnection {
                connected: c.status == RemoteConnectionStatus::Connected,
                platform: c.runtime.as_ref().map(|r| r.platform.clone()),
            };
            (c.id, shown)
        })
        .collect()
}

/// `RemoteConnection(workspace)`: the connection a remote workspace runs through as the client knows it now; None for a local workspace
/// or a removed connection.
fn connection_of(workspace: &Workspace) -> Option<RemoteConnectionInfo> {
    let id = workspace.remote.as_ref()?.connection_id.as_deref()?;
    existing()?.client.connection(id)
}

/// C# `RemoteWorkspaceFeatures.BlockedReason` for a pane's composer (`WindowRules::remote_block`), read with the state locked.
pub fn blocked_reason(host: &Host, snapshot: &AppSnapshot, pane: &RunSession) -> Option<String> {
    let _ = host;
    // A mirrored pane waits for its computer's connection (C# `MirrorBlockedReason`, W9-J).
    if let Some(mirror) = &pane.mirror {
        return crate::mirror::blocked_reason(mirror);
    }
    let workspace = snapshot.workspace(pane.workspace_id()).filter(|w| w.remote.is_some())?;
    features::blocked_reason(Some(workspace), pane, connection_of(workspace).as_ref())
}

/// The run service's runner for remote workspaces' panes (`RunServiceOptions.remote`), read once as runs.rs makes the service.
pub fn runner(host: &Arc<Host>) -> Option<Arc<dyn RemoteRunner>> {
    Some(Arc::new(app(host).client.clone()))
}

/// Whether paired computers should keep the relay link up now: this PC shares, with "페어링한 컴퓨터의 연결 받기" on.
pub fn relay_wanted(host: &Arc<Host>) -> bool {
    app(host).host().is_some_and(|sharing| sharing.relay() && sharing.state().enabled)
}

/// The relay host's link follows [`relay_wanted`] (the phone remote's own settings never change): after a start, a stop, the switch,
/// and whenever the page finds them apart (a sharing that ended on its own).
pub async fn sync_relay(host: &Arc<Host>) {
    let Some(relay) = crate::mobile::relay(host) else { return };
    let wanted = relay_wanted(host);
    if relay.desktops() != wanted && relay.set_desktops(wanted).await.is_err() {
        host.log.line("remote: the relay link for paired computers could not be changed");
    }
}

/// The sharing host's port while it shares (the firewall rule's second port, W8-C).
pub fn sharing_port(host: &Host) -> Option<u16> {
    let _ = host;
    let state = existing()?.host.get()?.state();
    if state.enabled { state.port } else { None }
}

/// `remoteHostRuns > 0` reads it: another computer's runs going on this PC (CLI updates and plugin changes wait while any does).
pub fn host_runs(host: &Host) -> usize {
    let _ = host;
    existing().map_or(0, |app| app.host_runs.load(Ordering::SeqCst))
}

/// `remoteHost?.Current()`: what "이 컴퓨터 공유" shows now (a sharing whose listener died ends first); off before the host is made.
pub fn host_state(host: &Arc<Host>) -> RemoteHostState {
    app(host).host().map(RemoteHost::state).unwrap_or_default()
}

/// The page's Open (C# `OpenRemoteWorkspaceAsync`, then Settings closed): the host's shared workspace listed in the sidebar and made
/// active; one with no pane gets a Claude pane.
pub fn open_workspace(host: &Arc<Host>, connection_id: &str, host_workspace_id: &str) -> Result<(), String> {
    let workspace = app(host).client.open_workspace(connection_id, host_workspace_id).map_err(|e| e.message())?;
    crate::settings::close(host);
    crate::frame::open_remote_workspace(host, workspace)
}

/// `RemoteAction(operation)`: one remote action at a time; its error shows on the page, and the page reads both sides again after it.
/// The operation runs in a task of its own, so one that panics still ends the action: the page never stays busy.
pub fn action<F>(host: &Arc<Host>, operation: F)
where
    F: std::future::Future<Output = Result<(), String>> + Send + 'static,
{
    let app = app(host);
    {
        let mut page = lock(&app.page);
        if page.busy {
            return;
        }
        page.busy = true;
        page.error = None;
    }
    crate::native::touch(host, crate::settings::SURFACE);
    let host = host.clone();
    tauri::async_runtime::spawn(async move {
        let outcome = match tokio::spawn(operation).await {
            Ok(outcome) => outcome,
            Err(error) => Err(failure_words(error)),
        };
        {
            let mut page = lock(&app.page);
            if let Err(error) = outcome {
                page.error = Some(error);
            }
            page.busy = false;
        }
        if !host.closing() {
            crate::native::touch(&host, crate::settings::SURFACE);
        }
    });
}

/// What a task that did not finish said: a panic's words, else that it was cancelled.
fn failure_words(error: tokio::task::JoinError) -> String {
    if !error.is_panic() {
        return error.to_string();
    }
    let payload = error.into_panic();
    payload.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| payload.downcast_ref::<String>().cloned()).unwrap_or_default()
}

/// `CopyRemoteKeyAsync(key, copied)`: the key on the clipboard in its secret form (out of clipboard history and the cloud clipboard,
/// skipped by clipboard monitors), cleared after a minute if the clipboard still holds it, and at quit if it still does then.
/// `copied` is told whether it reached the clipboard. A smoke never calls it: it must not touch the real clipboard.
pub fn copy_key(host: &Arc<Host>, key: String, copied: impl Fn(bool) + Send + Sync + 'static) {
    if host.smoke.is_some() {
        return;
    }
    let held = app(host).copied.clone();
    tauri::async_runtime::spawn(copy_secret(crate::platform::clipboard(), key, features::KEY_COPY.clear_after, held, copied));
}

/// The copy over `clipboard`: written in its secret form and kept in `held` (for the clear at quit), `copied` told, then after
/// `clear_after` the clipboard emptied if it still holds exactly the key; `held` forgets it either way (unless a newer copy replaced it).
async fn copy_secret(
    clipboard: Arc<dyn Clipboard>,
    key: String,
    clear_after: Duration,
    held: Arc<Mutex<Option<String>>>,
    copied: impl Fn(bool) + Send + Sync + 'static,
) {
    let (writer, secret) = (clipboard.clone(), key.clone());
    let set = tokio::task::spawn_blocking(move || writer.write_secret(&secret).is_ok()).await.unwrap_or(false);
    if set {
        *lock(&held) = Some(key.clone());
    }
    copied(set);
    if !set {
        return;
    }
    tokio::time::sleep(clear_after).await;
    let checked = key.clone();
    let _ = tokio::task::spawn_blocking(move || clear_if_holding(clipboard.as_ref(), &checked)).await;
    let mut held = lock(&held);
    if held.as_deref() == Some(key.as_str()) {
        *held = None;
    }
}

/// `RemoteSecretCopy.ShouldClear` then `Clipboard.Clear()`: emptied only while it holds exactly `key`. True when it was emptied.
fn clear_if_holding(clipboard: &dyn Clipboard, key: &str) -> bool {
    features::should_clear(clipboard.read_text().ok().flatten().as_deref(), key) && clipboard.clear().is_ok()
}

/// `Copy(value)` (an address): plain text, as the page's other copies.
pub fn copy_text(host: &Arc<Host>, value: String) {
    if host.smoke.is_some() {
        return;
    }
    let clipboard = crate::platform::clipboard();
    tauri::async_runtime::spawn_blocking(move || {
        let _ = clipboard.write_text(&value);
    });
}

#[cfg(test)]
mod tests {
    use mighty_composer::composer::{ComposerInputs, PaneViewState, WindowRules, refresh};
    use mighty_platform::fake::MemoryClipboard;
    use mighty_updates::cli_update::holds::{ProviderUse, UpdateActivity, ready_for_automatic_update};

    use super::*;

    #[test]
    fn the_sharing_choice_reads_back_and_a_foreign_file_is_none() {
        let saved =
            SavedSharing { sharing: true, workspace_ids: vec!["A1B2C3D4-0000-4000-8000-000000000001".into()], port: 43137, relay: true };
        assert_eq!(SavedSharing::decode(&saved.encode()), Some(saved));
        assert_eq!(SavedSharing::decode(r#"{"version":2,"sharing":true}"#), None);
        assert_eq!(SavedSharing::decode("not json"), None);
        let odd = SavedSharing::decode(r#"{"version":1,"sharing":true,"workspaceIds":["../x",7],"port":70000}"#).unwrap();
        assert!(odd.workspace_ids.is_empty());
        assert_eq!(odd.port, protocol::DEFAULT_PORT);
        assert!(!odd.relay);
    }

    #[test]
    fn sharing_comes_back_with_the_app_and_each_choice_of_the_person_here_is_kept() {
        let remote = source("remote.rs");
        assert!(remote.contains("    let _ = app.host.set(RemoteHost::new(options));\n    restore_sharing(host);"));
        let settings = source("settings/remote.rs");
        assert_eq!(settings.matches("crate::remote::remember_sharing(").count(), 3);
    }

    /// A source file's text with its line ends as the repository stores them (a Windows checkout may turn them into CRLF).
    fn source(file: &str) -> String {
        std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join(file)).unwrap().replace("\r\n", "\n")
    }

    fn remote(path: &str) -> Workspace {
        Workspace {
            id: Some("far".into()),
            name: Some("far".into()),
            path: Some(path.into()),
            remote: Some(mighty_core::model::RemoteWorkspaceReference {
                connection_id: Some("conn-1".into()),
                workspace_id: Some("ws-1".into()),
                host_name: Some("studio-pc".into()),
                version: 2,
                mirror: false,
            }),
            ..Workspace::default()
        }
    }

    /// The app's version as the host reports it: C#'s `AppVersion.Normalized`.
    #[test]
    fn the_hosts_app_version_is_normalized_as_csharp_does() {
        assert_eq!(normalized_version("v0.4.59").as_deref(), Some("0.4.59"));
        assert_eq!(normalized_version(" 1.2.3-beta.1+build ").as_deref(), Some("1.2.3-beta.1+build"));
        assert_eq!(normalized_version("1.2.3.4").as_deref(), Some("1.2.3.4"));
        for bad in ["", "1.2.3.4.5", "1..2", "x1", "1.2-", "1.2+", "1.2+a+b"] {
            assert_eq!(normalized_version(bad), None, "{bad}");
        }
    }

    /// "remote desktop: the window reads a workspace's folder only through LocalPath" (C# scans the WinUI sources; W8 plan D17: the Rust
    /// form, by behaviour): `RemoteWorkspaceFeatures.LocalPath` gives a remote workspace no folder, and the remote page lists only local
    /// workspaces, each with its LocalPath. The sharing host's folders come from the run service's shared check, which refuses a remote
    /// workspace before any disk read (mighty-remote's remote_desktop `the_shared_check_never_reads_a_remote_workspaces_folder`).
    #[test]
    fn the_window_reads_a_workspaces_folder_only_through_local_path() {
        let local = Workspace { id: Some("near".into()), name: Some("near".into()), path: Some("C:\\work".into()), ..Workspace::default() };
        let far = remote("\\\\server\\share");
        assert_eq!(features::local_path(Some(&local)), Some("C:\\work"));
        assert_eq!(features::local_path(Some(&far)), None, "a host's path never reaches this PC's disk");
        let listed = crate::settings::remote::locals_of(&[far.clone(), local.clone()]);
        assert_eq!(listed.iter().map(|l| (l.id.as_str(), l.path.as_str())).collect::<Vec<_>>(), [("near", "C:\\work")]);
    }

    /// Whether a copy said it reached the clipboard, once it says.
    async fn told(slot: &Arc<Mutex<Option<bool>>>) -> bool {
        for _ in 0..400 {
            let set = *lock(slot);
            if let Some(set) = set {
                return set;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("the copy never said whether it reached the clipboard")
    }

    /// A copy of "key-1" over `clipboard`, emptied after 300 ms: the task and where it says whether it was copied.
    fn copy(
        clipboard: &Arc<MemoryClipboard>,
        held: &Arc<Mutex<Option<String>>>,
    ) -> (tokio::task::JoinHandle<()>, Arc<Mutex<Option<bool>>>) {
        let said: Arc<Mutex<Option<bool>>> = Arc::default();
        let into = said.clone();
        let task =
            copy_secret(clipboard.clone(), "key-1".into(), Duration::from_millis(300), held.clone(), move |set| *lock(&into) = Some(set));
        (tokio::spawn(task), said)
    }

    /// "remote desktop: the remote page copies the key secretly and host runs hold updates" (W8 plan D17: the Rust form, by behaviour):
    /// the copy writes the clipboard's secret form, empties the clipboard after the wait only while it still holds the key (and at quit),
    /// says when the clipboard refused it; a provider in use (which another computer's runs make every provider, `provider_use_in`)
    /// refuses the manual update and the plugin change and holds the automatic one.
    #[tokio::test]
    async fn the_remote_page_copies_the_key_secretly_and_host_runs_hold_updates() {
        assert_eq!(
            features::KEY_COPY,
            features::SecretCopy { allowed_in_history: false, roamable: false, clear_after: Duration::from_secs(60) }
        );
        // Copied secretly, then emptied after the wait.
        let (clipboard, held) = (Arc::new(MemoryClipboard::default()), Arc::default());
        let (running, said) = copy(&clipboard, &held);
        assert!(told(&said).await, "it reached the clipboard");
        assert!(clipboard.secret() && clipboard.text().as_deref() == Some("key-1") && lock(&held).as_deref() == Some("key-1"));
        running.await.unwrap();
        assert_eq!(clipboard.text(), None, "emptied, not overwritten with empty text");
        assert_eq!(*lock(&held), None);
        // Something else copied meanwhile stays.
        let (running, said) = copy(&clipboard, &held);
        assert!(told(&said).await);
        clipboard.write_text("mine").unwrap();
        running.await.unwrap();
        assert_eq!(clipboard.text().as_deref(), Some("mine"), "only the key itself is cleared");
        // A clipboard another program holds: not copied, nothing kept.
        let busy = Arc::new(MemoryClipboard::default());
        busy.set_busy(true);
        let (running, said) = copy(&busy, &held);
        assert!(!told(&said).await, "the page says it was not copied");
        running.await.unwrap();
        assert_eq!(*lock(&held), None);
        // At quit: emptied only while it holds the key.
        let quitting = MemoryClipboard::with_text("key-1");
        assert!(clear_if_holding(&quitting, "key-1") && quitting.text().is_none());
        let other = MemoryClipboard::with_text("mine");
        assert!(!clear_if_holding(&other, "key-1") && other.text().as_deref() == Some("mine"));
        // A provider in use holds every update.
        let in_use = ProviderUse { running: true, account_change: false, queued: false, last_active: None };
        let activity = UpdateActivity::default();
        assert!(activity.manual_update_refusal("claude", &in_use).is_some(), "a manual update waits");
        assert!(activity.plugin_mutation_block_reason("claude", true, true, &in_use).is_some(), "a plugin change waits");
        assert!(!ready_for_automatic_update(&in_use, chrono::Utc::now()), "an automatic update waits");
        // What makes it in use needs the window (a Host): another computer's runs count as running there.
        let updates = source("updates.rs");
        let provider_use = updates.split("pub fn provider_use_in(").nth(1).unwrap().split("\n}\n").next().unwrap();
        assert!(
            provider_use.contains("crate::remote::host_runs(host) > 0")
                && provider_use.contains("running: active || starting || host_runs")
        );
    }

    /// "remote desktop: the window names a remote host and keeps its panes to it" (W8 plan D17: the Rust form, by behaviour): "host · OS"
    /// from the connection's last answer, the pane notes name the host, a removed connection blocks the pane, and a composer whose
    /// window rules carry the host's block cannot send (its reason the block) while the same pane without it can. The add menu's
    /// limits are mighty-remote's remote_desktop `remote_panes_keep_to_the_hosts_features`.
    #[test]
    fn the_window_names_a_remote_host_and_keeps_its_panes_to_it() {
        assert_eq!(strings::host_with_os("studio-pc", Some("darwin")), format!("studio-pc · {}", strings::host_os(Some("darwin"))));
        let far = remote("C:\\far");
        let shell = RunSession {
            id: Some("cmd".into()),
            workspace_id: Some("far".into()),
            kind: Some("shell".into()),
            provider: Some("claude".into()),
            ..RunSession::default()
        };
        assert!(features::pane_note(&shell, &far, None).contains("studio-pc"), "the pane says where it runs");
        let block = features::blocked_reason(Some(&far), &shell, None);
        assert!(block.as_deref().is_some_and(|b| b.contains("studio-pc")), "a removed connection blocks the pane: {block:?}");
        let view = PaneViewState { draft: "dir".into(), ..PaneViewState::default() };
        let locale = mighty_core::locale::Locale::new("en");
        let composed = |window: &WindowRules| {
            refresh(&ComposerInputs {
                locale,
                now: mighty_core::time::DateTimeOffset::now(),
                pane: &shell,
                workspace: Some(&far),
                runtime: None,
                efforts: &[],
                view: &view,
                window,
            })
        };
        let free = composed(&WindowRules::default());
        let held = composed(&WindowRules { remote_block: block.clone(), ..WindowRules::default() });
        assert!(free.can_send && !held.can_send && Some(held.reason) == block, "the host's block holds the composer");
    }
}
