//! MC-33: a Mighty Code server on an SSH machine — installed, updated, started, connected to and removed from this computer over the
//! machine's own ssh (mighty-run's `ssh_server` runs mighty-core's `ssh::server` scripts). Only a remote **Mac** gets one: there is
//! no Linux build, and a Linux or Windows machine keeps the minimal mode (its box says why).
//!
//! - **확인** (detect): `uname -s -m`, the macOS version, who is logged in at its screen, the app in `~/Applications` (else
//!   `/Applications`) and its version, the running server's `mighty server status`.
//! - **설치 / 원격 서버 업데이트**: this app's signed update manifest is read and checked with the build's key (the same check as the
//!   app's own update; a build without a key installs nothing), the Mac package of this app's version — or the latest, said so, when
//!   the manifest names another — downloaded and its size and SHA-256 checked, then sent over ssh and installed (see mighty-run's
//!   notes: the far side's digest, `codesign --verify --deep --strict` for Mighty Code's identifier and this app's team, the running
//!   server stopped with its own CLI, two renames, started detached without a window). The server version differing from this app's
//!   offers the update.
//! - **연결**: the far side's app asked over ssh for a one-time pairing code (`mighty server ssh-pair`, which also starts its loopback
//!   listener), that listener forwarded to a local port from the port allocator (`ssh -N -L 127.0.0.1:<local>:localhost:<port>` over
//!   the shared connection), and the remote client's SSH connection made (`connect_ssh`: the code spent for the key through the
//!   forward): the remote Mac's workspaces and panes come in as any mirrored computer's. The code is never logged or shown.
//! - **끊김**: the forward's ssh ending by itself (the network went) starts a reconnect a few times with growing gaps: the forward
//!   opened again, a new pairing, the mirror connected again. The remote server keeps running meanwhile.
//! - **서버 지우기** (confirmed first): the connection forgotten, the forward closed, the server stopped and `~/Applications/
//!   MightyCode.app` deleted (the far side's data stays; an app in `/Applications` is never touched).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use mighty_core::ssh::server::{AppPlace, Detection, version_core};
use mighty_native::surface::{DefaultButton, DialogButton, DialogResult, DialogSpec};
use mighty_native::ui::Content;
use mighty_platform::profile::UpdatePackage;
use mighty_remote::remote::ssh_tunnel::{SshOpening, ssh_address};
use mighty_remote::remote::wire::RemoteConnectionStatus;
use mighty_run::ssh_server::{InstallPlan, Progress, RemoteServer, ServerError, Step};
use mighty_updates::app_update::service::{Service, ServiceError};
use mighty_updates::app_update::version;

use crate::host::Host;

/// What a machine's server box shows: the last detection, the work going on with its words, the last outcome.
#[derive(Default, Clone)]
struct Entry {
    detection: Option<Detection>,
    working: bool,
    progress: Option<String>,
    /// The last outcome's sentence, and whether it is an error.
    message: Option<(String, bool)>,
    reconnecting: bool,
}

static STATE: Mutex<Option<HashMap<String, Entry>>> = Mutex::new(None);

fn lock() -> MutexGuard<'static, Option<HashMap<String, Entry>>> {
    STATE.lock().unwrap_or_else(|p| p.into_inner())
}

fn with<R>(machine: &str, change: impl FnOnce(&mut Entry) -> R) -> R {
    let mut guard = lock();
    change(guard.get_or_insert_with(HashMap::new).entry(machine.to_string()).or_default())
}

fn entry(machine: &str) -> Entry {
    lock().as_ref().and_then(|m| m.get(machine).cloned()).unwrap_or_default()
}

fn redraw(host: &Host) {
    crate::native::touch(host, crate::settings::SURFACE);
}

fn progress(host: &Host, machine: &str, words: Option<String>) {
    with(machine, |e| e.progress = words);
    redraw(host);
}

fn finish(host: &Host, machine: &str, message: Option<(String, bool)>) {
    with(machine, |e| {
        e.working = false;
        e.progress = None;
        if message.is_some() {
            e.message = message;
        }
    });
    redraw(host);
}

/// Takes the machine's one slot for work; false when other work goes on.
fn begin(host: &Host, machine: &str) -> bool {
    let began = with(machine, |e| {
        if e.working {
            return false;
        }
        e.working = true;
        e.message = None;
        true
    });
    if began {
        redraw(host);
    }
    began
}

/// The version this app compares with a server's: `0.6.0` of `0.6.0+1234`.
fn local_version(host: &Host) -> String {
    version::normalized(&version_core(&crate::settings::about::version_text(host))).unwrap_or_else(|| "0.1.0".into())
}

/// What the box offers for a machine, from its detection and this app's version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    /// Not a Mac, or a Mac whose processor has no build: the minimal mode only (the locale key that says why).
    pub minimal_only: Option<&'static str>,
    pub installed: bool,
    pub running: bool,
    /// The server's (or the installed app's) version differs from this app's: 원격 서버 업데이트.
    pub update: bool,
    /// The app is ours to replace and remove (`~/Applications`).
    pub removable: bool,
}

/// The offer for `detection` against this app's `local` version.
pub fn offer(local: &str, detection: &Detection) -> Offer {
    let minimal_only = if !detection.is_mac() {
        Some("ssh.server.state.notMac")
    } else if !detection.supported_arch() {
        Some("ssh.server.state.arch")
    } else {
        None
    };
    if minimal_only.is_some() {
        return Offer { minimal_only, installed: false, running: false, update: false, removable: false };
    }
    let installed = detection.app.is_some();
    let running = detection.running();
    let remote = detection.server.as_ref().filter(|s| s.running).map(|s| s.version.clone()).or_else(|| detection.version.clone());
    let update = installed && remote.is_some_and(|remote| version_differs(local, &remote));
    Offer { minimal_only, installed, running, update, removable: detection.app == Some(AppPlace::Home) }
}

/// Whether two versions differ as releases (`0.6.0` and `v0.6.0 (abc)` are the same; one that does not read differs).
pub fn version_differs(local: &str, remote: &str) -> bool {
    match (version::normalized(&version_core(local)), version::normalized(&version_core(remote))) {
        (Some(a), Some(b)) => version::compare(&a, &b) != std::cmp::Ordering::Equal,
        _ => true,
    }
}

/// The SSH connection of `machine`'s server in the remote client, if one is listed.
fn connection_of(host: &Arc<Host>, machine: &str) -> Option<mighty_remote::remote::wire::RemoteConnectionInfo> {
    let address = ssh_address(machine);
    crate::remote::app(host).client.connections().into_iter().find(|c| c.address == address)
}

/// The words of a failure of a server step.
fn words(host: &Host, machine: &str, error: &ServerError) -> String {
    let l = &host.locale;
    let target = super::machine(host, machine).map(|m| m.target).unwrap_or_default();
    let detection = entry(machine).detection;
    match error {
        ServerError::Ssh(failure) => {
            let mut text = l.get_with(failure.key(), &[("host", &target)]);
            if let mighty_core::ssh::Failure::Other(line) = failure
                && !line.is_empty()
            {
                text = format!("{text} {line}");
            }
            text
        }
        ServerError::Digest => l.get("ssh.server.error.digest"),
        ServerError::Unpack(refusal) => l.get(match refusal {
            mighty_core::ssh::server::UnpackRefusal::Unpack => "ssh.server.error.unpack",
            mighty_core::ssh::server::UnpackRefusal::NotOneApp => "ssh.server.error.notOneApp",
            mighty_core::ssh::server::UnpackRefusal::NotMightyCode => "ssh.server.error.notMightyCode",
            mighty_core::ssh::server::UnpackRefusal::Version => "ssh.server.error.version",
            mighty_core::ssh::server::UnpackRefusal::Signature => "ssh.server.error.signature",
        }),
        ServerError::Stop(error) => l.get_with("ssh.server.error.stop", &[("error", error)]),
        ServerError::Swap => l.get("ssh.server.error.swap"),
        ServerError::Start(error) if detection.as_ref().and_then(Detection::gui_session) == Some(false) => {
            let _ = error;
            l.get("ssh.server.error.startNoGui")
        }
        ServerError::Start(error) => l.get_with("ssh.server.error.start", &[("error", error)]),
        ServerError::Pair(error) => l.get_with("ssh.server.error.pair", &[("error", error)]),
        ServerError::Remove(error) => l.get_with("ssh.server.error.remove", &[("error", error)]),
        ServerError::Local(error) => error.clone(),
    }
}

fn remote_server(host: &Host, machine: &str) -> Result<RemoteServer, String> {
    let launch = super::launch_for_machine(host, machine)?;
    Ok(RemoteServer::new(host.processes.clone(), launch))
}

/// Reads the machine again (no work slot: a step's own read).
async fn read(host: &Arc<Host>, machine: &str) -> Result<Detection, String> {
    let server = remote_server(host, machine)?;
    let detection = server.detect().await.map_err(|e| words(host, machine, &e))?;
    with(machine, |e| e.detection = Some(detection.clone()));
    Ok(detection)
}

/// 확인: the machine read again.
pub async fn detect(host: &Arc<Host>, machine: &str) {
    if !begin(host, machine) {
        return;
    }
    progress(host, machine, Some(host.locale.get("ssh.server.progress.detecting")));
    let outcome = read(host, machine).await;
    finish(host, machine, outcome.err().map(|e| (e, true)));
}

/// The Mac package this app installs: the signed manifest read with the build's key, its `macos` asset, downloaded and checked.
/// Its path, SHA-256 and version, and whether that version is the latest rather than this app's own.
async fn package(host: &Arc<Host>, machine: &str, detection: &Detection) -> Result<(std::path::PathBuf, String, String, bool), String> {
    let l = &host.locale;
    let update = crate::app_update::of(host).ok_or_else(|| l.get("ssh.server.error.noKey"))?;
    let coordinator = update.core.coordinator.clone();
    if !coordinator.has_public_key() {
        return Err(l.get("ssh.server.error.noKey"));
    }
    let user_url = host.view().snapshot.app_update_manifest_url_override.clone();
    let url = coordinator.effective_manifest_url(user_url.as_deref()).ok_or_else(|| l.get("ssh.server.error.noAddress"))?;
    let local = local_version(host);
    progress(host, machine, Some(l.get("ssh.server.progress.checking")));
    let availability = match coordinator.service().check(Some(&url), &local).await {
        Ok(availability) => availability,
        Err(ServiceError::Cancelled) => return Err(l.get("ssh.server.error.noAnswer")),
        Err(ServiceError::Refused(error)) => return Err(error.message(l)),
    };
    let manifest = availability.manifest;
    let asset = manifest.macos.clone().ok_or_else(|| l.get("ssh.server.error.noPackage"))?;
    if let (Some(minimum), Some(macos)) = (&manifest.minimum_system_version, version::normalized(&detection.macos))
        && version::compare(&macos, minimum) == std::cmp::Ordering::Less
    {
        return Err(l.get_with(
            "ssh.server.error.macosTooOld",
            &[("macos", &detection.macos), ("version", &manifest.version), ("minimum", minimum)],
        ));
    }
    let latest_instead = version::compare(&manifest.version, &local) != std::cmp::Ordering::Equal;
    let service = Service::new(update.core.coordinator.service().transport().clone(), &super::folder(&host.data_dir).join("server"), None)
        .with_package(UpdatePackage::MacBundle);
    let (reporter, machine_id) = (host.clone(), machine.to_string());
    let told = Arc::new(Mutex::new(-1i64));
    let report: Arc<dyn Fn(f64) + Send + Sync> = Arc::new(move |fraction: f64| {
        let percent = (fraction * 100.0).round() as i64;
        let mut last = told.lock().unwrap_or_else(|p| p.into_inner());
        if *last != percent {
            *last = percent;
            drop(last);
            let text = reporter.locale.get_with("ssh.server.progress.downloading", &[("percent", &percent.to_string())]);
            progress(&reporter, &machine_id, Some(text));
        }
    });
    let cancel = mighty_updates::runner::CancellationToken::new();
    let path = match service.download(&asset, &manifest.version, report, &cancel).await {
        Ok(path) => path,
        Err(ServiceError::Cancelled) => return Err(l.get("ssh.server.error.noAnswer")),
        Err(ServiceError::Refused(error)) => return Err(l.get_with("ssh.server.error.download", &[("error", &error.message(l))])),
    };
    Ok((path, asset.sha256, manifest.version, latest_instead))
}

/// The code-signature requirement: this app's team when it is signed by one (an ad hoc or a Windows build: the identifier alone).
fn requirement() -> String {
    let team = crate::platform::update_installer().running_team().ok().flatten();
    mighty_core::ssh::server::requirement(team.as_deref())
}

/// Asks the person; true when they chose `button`.
async fn confirm(host: &Arc<Host>, title: String, body: String, button: String) -> bool {
    let mut spec = DialogSpec::new(title, move |_| Content::Text(body.clone())).styled(None, None);
    spec.primary = Some(DialogButton::new(button));
    spec.close = Some(host.locale.get("ssh.server.confirm.cancel"));
    spec.default_button = DefaultButton::Close;
    crate::native::show_dialog(host, spec).await == DialogResult::Primary
}

/// 설치 / 원격 서버 업데이트, then a connection.
pub async fn install(host: &Arc<Host>, machine: &str) {
    if !begin(host, machine) {
        return;
    }
    let outcome = install_inner(host, machine).await;
    match outcome {
        Ok(Some(version)) => {
            finish(host, machine, Some((host.locale.get_with("ssh.server.done.installed", &[("version", &version)]), false)));
            connect(host, machine).await;
        }
        Ok(None) => finish(host, machine, None),
        Err(error) => finish(host, machine, Some((error, true))),
    }
}

/// The install's steps; None when the person said no.
async fn install_inner(host: &Arc<Host>, machine: &str) -> Result<Option<String>, String> {
    let l = host.locale;
    progress(host, machine, Some(l.get("ssh.server.progress.detecting")));
    let detection = read(host, machine).await?;
    let offered = offer(&local_version(host), &detection);
    if let Some(key) = offered.minimal_only {
        return Err(l.get_with(key, &[("system", &detection.os), ("arch", &detection.arch)]));
    }
    let (package, sha256, version, latest_instead) = package(host, machine, &detection).await?;
    let target = super::machine(host, machine).map(|m| m.target).unwrap_or_default();
    let mut body = l.get_with("ssh.server.confirm.installBody", &[("host", &target), ("version", &version)]);
    if latest_instead {
        body.push_str("\n\n");
        body.push_str(&l.get_with("ssh.server.confirm.latestNote", &[("local", &local_version(host)), ("latest", &version)]));
    }
    if detection.server.as_ref().is_some_and(|s| s.running && s.mode == "windowed") {
        body.push_str("\n\n");
        body.push_str(&l.get("ssh.server.confirm.windowedNote"));
    }
    let title = l.get(if offered.installed { "ssh.server.confirm.updateTitle" } else { "ssh.server.confirm.installTitle" });
    progress(host, machine, None);
    if !confirm(host, title, body, l.get("ssh.server.confirm.installButton")).await {
        return Ok(None);
    }
    // A connection to the server being replaced ends first; the new one is made once it runs.
    if let Some(connection) = connection_of(host, machine) {
        crate::remote::app(host).client.disconnect(&connection.id).await;
    }
    super::forward::close_server(host, machine).await;
    let plan = InstallPlan {
        package,
        sha256,
        version: version.clone(),
        requirement: requirement(),
        id: mighty_core::wire::new_id().replace('-', ""),
        stop_first: detection.running(),
    };
    let server = remote_server(host, machine)?;
    let (reporter, machine_id) = (host.clone(), machine.to_string());
    let told = Arc::new(Mutex::new(-1i64));
    let report = move |step: Progress| {
        let l = &reporter.locale;
        let text = match step {
            Progress::Upload(fraction) => {
                let percent = (fraction * 100.0).round() as i64;
                let mut last = told.lock().unwrap_or_else(|p| p.into_inner());
                if *last == percent {
                    return;
                }
                *last = percent;
                l.get_with("ssh.server.progress.uploading", &[("percent", &percent.to_string())])
            }
            Progress::Step(Step::Upload) => l.get_with("ssh.server.progress.uploading", &[("percent", "0")]),
            Progress::Step(Step::Unpack) => l.get("ssh.server.progress.unpacking"),
            Progress::Step(Step::Stop) => l.get("ssh.server.progress.stopping"),
            Progress::Step(Step::Swap) => l.get("ssh.server.progress.swapping"),
            Progress::Step(Step::Start) => l.get("ssh.server.progress.starting"),
        };
        progress(&reporter, &machine_id, Some(text));
    };
    let started = server.install(&plan, &report).await;
    // What the machine has now, whatever happened.
    let _ = read(host, machine).await;
    started.map_err(|e| words(host, machine, &e))?;
    host.log.line(&format!("ssh: a Mighty Code server {version} was installed on an SSH machine"));
    Ok(Some(version))
}

/// 서버 시작: the installed server started without a window.
pub async fn start(host: &Arc<Host>, machine: &str) {
    if !begin(host, machine) {
        return;
    }
    let outcome = start_inner(host, machine).await;
    match outcome {
        Ok(()) => {
            finish(host, machine, None);
            connect(host, machine).await;
        }
        Err(error) => finish(host, machine, Some((error, true))),
    }
}

async fn start_inner(host: &Arc<Host>, machine: &str) -> Result<(), String> {
    progress(host, machine, Some(host.locale.get("ssh.server.progress.starting")));
    let place = entry(machine).detection.and_then(|d| d.app).unwrap_or(AppPlace::Home);
    let server = remote_server(host, machine)?;
    let started = server.start(place).await;
    let _ = read(host, machine).await;
    started.map(|_| ()).map_err(|e| words(host, machine, &e))
}

/// 연결: the server's connection made (or made again), its workspaces mirrored.
pub async fn connect(host: &Arc<Host>, machine: &str) {
    if !begin(host, machine) {
        return;
    }
    progress(host, machine, Some(host.locale.get("ssh.server.progress.connecting")));
    let outcome = connect_inner(host, machine).await;
    finish(
        host,
        machine,
        Some(match outcome {
            Ok(name) => (host.locale.get_with("ssh.server.done.connected", &[("name", &name)]), false),
            Err(error) => (error, true),
        }),
    );
}

async fn connect_inner(host: &Arc<Host>, machine: &str) -> Result<String, String> {
    let name = super::machine(host, machine).map(|m| m.name()).ok_or_else(|| host.locale.get("ssh.error.machineGoneShort"))?;
    let client = crate::remote::app(host).client.clone();
    let info = match connection_of(host, machine) {
        Some(existing) => {
            crate::mirror::connect(&existing.id).await?;
            existing
        }
        None => client.connect_ssh(&name, machine).await.map_err(|e| e.message())?,
    };
    Ok(info.name)
}

/// 연결 끊기: the connection disconnected and the forward closed (the server keeps running).
pub async fn disconnect(host: &Arc<Host>, machine: &str) {
    if let Some(connection) = connection_of(host, machine) {
        crate::remote::app(host).client.disconnect(&connection.id).await;
    }
    super::forward::close_server(host, machine).await;
    with(machine, |e| e.reconnecting = false);
    redraw(host);
}

/// 서버 지우기 (asked first): the connection forgotten, the server stopped and its app deleted.
pub async fn remove(host: &Arc<Host>, machine: &str) {
    let l = host.locale;
    let target = super::machine(host, machine).map(|m| m.target).unwrap_or_default();
    if !confirm(
        host,
        l.get("ssh.server.confirm.removeTitle"),
        l.get_with("ssh.server.confirm.removeBody", &[("host", &target)]),
        l.get("ssh.server.confirm.removeButton"),
    )
    .await
    {
        return;
    }
    if !begin(host, machine) {
        return;
    }
    progress(host, machine, Some(l.get("ssh.server.progress.removing")));
    if let Some(connection) = connection_of(host, machine) {
        crate::remote::app(host).client.remove(&connection.id).await;
    }
    super::forward::close_server(host, machine).await;
    let outcome = async {
        let server = remote_server(host, machine)?;
        let removed = server.remove().await;
        let _ = read(host, machine).await;
        removed.map_err(|e| words(host, machine, &e))
    }
    .await;
    finish(
        host,
        machine,
        Some(match outcome {
            Ok(()) => (l.get("ssh.server.done.removed"), false),
            Err(error) => (error, true),
        }),
    );
}

/// The machine was removed from the list: its server's connection forgotten (the server itself is left as it is).
pub async fn forget(host: &Arc<Host>, machine: &str) {
    if let Some(connection) = connection_of(host, machine) {
        crate::remote::app(host).client.remove(&connection.id).await;
    }
    if let Some(map) = lock().as_mut() {
        map.remove(machine);
    }
}

/// How long the forward may take to listen.
const FORWARD_READY: Duration = Duration::from_secs(10);

/// The remote client's way to `machine`'s server (its `ssh_tunnel` hook, on every connect and reconnect): a one-time code from the
/// far side's app over ssh, the forward of its loopback listener opened (or kept), and the forward's local port once it listens.
pub async fn open_tunnel(host: &Arc<Host>, machine: &str) -> Result<SshOpening, String> {
    let launch = super::launch_for_machine(host, machine)?;
    // The app's place as last read (after a restart of this app, read again first).
    let known = entry(machine).detection;
    let detection = match known {
        Some(detection) => detection,
        None => read(host, machine).await?,
    };
    let place = detection.app.ok_or_else(|| host.locale.get("ssh.server.error.notInstalled"))?;
    let server = RemoteServer::new(host.processes.clone(), launch.clone());
    let pairing = server.pair(place).await.map_err(|e| words(host, machine, &e))?;
    let ended = {
        let (host, machine) = (Arc::downgrade(host), machine.to_string());
        Box::new(move || {
            if let Some(host) = host.upgrade().filter(|h| !h.closing()) {
                tauri::async_runtime::spawn(async move { forward_ended(&host, &machine).await });
            }
        })
    };
    let local = super::forward::open_server(host, machine, launch, pairing.port, ended).await?;
    // The forward's ssh sets its listener up a moment after it starts.
    let deadline = tokio::time::Instant::now() + FORWARD_READY;
    loop {
        if tokio::net::TcpStream::connect(("127.0.0.1", local)).await.is_ok() {
            break;
        }
        if !super::forward::server_open(machine) || tokio::time::Instant::now() >= deadline {
            return Err(host.locale.get("ssh.server.error.forward"));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Ok(SshOpening::new(local, pairing.code))
}

/// The gaps between reconnects after a lost forward.
const RECONNECT_GAPS: [u64; 6] = [2, 5, 15, 30, 60, 120];

/// The forward's ssh ended by itself: the connection is made again a few times with growing gaps while the machine and its
/// connection are still listed and it is not connected through a forward again; the server on the far side keeps running.
async fn forward_ended(host: &Arc<Host>, machine: &str) {
    if connection_of(host, machine).is_none() || with(machine, |e| std::mem::replace(&mut e.reconnecting, true)) {
        return;
    }
    redraw(host);
    for gap in RECONNECT_GAPS {
        tokio::time::sleep(Duration::from_secs(gap)).await;
        if host.closing() || super::machine(host, machine).is_none() || !entry(machine).reconnecting {
            break;
        }
        let Some(connection) = connection_of(host, machine) else { break };
        if super::forward::server_open(machine) && connection.status == RemoteConnectionStatus::Connected {
            break;
        }
        if crate::mirror::connect(&connection.id).await.is_ok() {
            host.log.line("ssh: the server connection is back after a lost forward");
            break;
        }
    }
    with(machine, |e| e.reconnecting = false);
    redraw(host);
}

/// A connection's address as shown: an SSH machine's server as `ssh · <machine>` (its saved `ssh:<id>` names nobody), any other as it
/// is.
pub fn shown_address(host: &Host, address: &str) -> String {
    match mighty_remote::remote::ssh_tunnel::ssh_machine(address) {
        Some(id) => format!("ssh · {}", super::machine(host, id).map(|m| m.target).unwrap_or_else(|| host.locale.get("ssh.state.removed"))),
        None => address.to_string(),
    }
}

/// What the settings box reads for a machine (its memory only).
pub struct View {
    pub detection: Option<Detection>,
    pub offer: Option<Offer>,
    pub working: bool,
    pub progress: Option<String>,
    pub message: Option<(String, bool)>,
    pub reconnecting: bool,
    pub connected: bool,
    pub connection: bool,
    pub local_version: String,
}

pub fn view(host: &Arc<Host>, machine: &str) -> View {
    let mut e = entry(machine);
    // Not checked here yet, but the connection test already read another OS: the box says at once that it keeps the minimal mode.
    if e.detection.is_none()
        && let Some(system) = super::machine(host, machine).and_then(|m| m.system)
        && !system.starts_with("Darwin")
        && !system.is_empty()
    {
        let mut words = system.split_whitespace();
        let os = words.next().unwrap_or_default().to_string();
        e.detection = Some(Detection { os, arch: words.next().unwrap_or_default().to_string(), ..Detection::default() });
    }
    let local = local_version(host);
    let connection =
        crate::remote::app_if_made().and_then(|app| app.client.connections().into_iter().find(|c| c.address == ssh_address(machine)));
    View {
        offer: e.detection.as_ref().map(|d| offer(&local, d)),
        detection: e.detection,
        working: e.working,
        progress: e.progress,
        message: e.message,
        reconnecting: e.reconnecting,
        connected: connection.as_ref().is_some_and(|c| c.status == RemoteConnectionStatus::Connected),
        connection: connection.is_some(),
        local_version: local,
    }
}

/// The box's sentence of the server's state: not read yet, the minimal mode only (and why), not installed, installed and stopped, or
/// running (headless or with its window), with the connection and the reconnect after it.
pub fn state_words(host: &Host, view: &View) -> String {
    let l = &host.locale;
    let (Some(detection), Some(offer)) = (&view.detection, &view.offer) else { return l.get("ssh.server.state.unknown") };
    if let Some(key) = offer.minimal_only {
        return l.get_with(key, &[("system", &detection.os), ("arch", &detection.arch)]);
    }
    let version = detection.version.clone().unwrap_or_default();
    let mut text = if offer.running {
        let mode = l.get(if detection.server.as_ref().is_some_and(|s| s.mode == "windowed") {
            "ssh.server.mode.windowed"
        } else {
            "ssh.server.mode.headless"
        });
        let running = detection.server.as_ref().map(|s| version_core(&s.version)).filter(|v| !v.is_empty()).unwrap_or(version);
        l.get_with("ssh.server.state.running", &[("version", &running), ("mode", &mode)])
    } else if offer.installed {
        l.get_with("ssh.server.state.stopped", &[("version", &version)])
    } else {
        l.get_with("ssh.server.state.none", &[("macos", &detection.macos), ("arch", &detection.arch)])
    };
    if view.reconnecting {
        text = format!("{text} · {}", l.get("ssh.server.progress.reconnecting"));
    } else if view.connected {
        text = format!("{text} · {}", l.get("ssh.server.state.connected"));
    }
    text
}

/// The box's notes under the state: the update this app's version asks for, and a Mac nobody is logged in at (the server cannot
/// start there).
pub fn notes(host: &Host, view: &View) -> Vec<String> {
    let l = &host.locale;
    let (Some(detection), Some(offer)) = (&view.detection, &view.offer) else { return vec![] };
    let mut notes = vec![];
    if offer.update {
        notes.push(l.get_with("ssh.server.state.versionDiffers", &[("local", &view.local_version)]));
    }
    if offer.minimal_only.is_none() && !offer.running && detection.gui_session() == Some(false) {
        notes.push(l.get_with("ssh.server.state.noGui", &[("user", &detection.user)]));
    }
    if detection.app == Some(AppPlace::System) {
        notes.push(l.get("ssh.server.state.systemApp"));
    }
    notes
}

/// Whether this build can verify a package (it has the update key): one without installs nothing (the settings box says so).
pub fn can_install(host: &Host) -> bool {
    crate::app_update::of(host).is_some_and(|u| u.core.coordinator.has_public_key())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mighty_core::ssh::server::ServerStatus;

    fn mac(app: Option<AppPlace>, version: Option<&str>, server: Option<(&str, &str)>) -> Detection {
        Detection {
            os: "Darwin".into(),
            arch: "arm64".into(),
            macos: "15.1".into(),
            console: "me".into(),
            user: "me".into(),
            app,
            version: version.map(str::to_string),
            server: server.map(|(v, mode)| ServerStatus { running: true, mode: mode.into(), version: v.into(), pid: Some(1) }),
        }
    }

    #[test]
    fn the_offer_follows_the_machine_and_the_versions() {
        let linux = Detection { os: "Linux".into(), arch: "x86_64".into(), ..Detection::default() };
        assert_eq!(offer("0.6.0", &linux).minimal_only, Some("ssh.server.state.notMac"));
        let odd = Detection { arch: "ppc".into(), ..mac(None, None, None) };
        assert_eq!(offer("0.6.0", &odd).minimal_only, Some("ssh.server.state.arch"));

        let none = offer("0.6.0", &mac(None, None, None));
        assert!(!none.installed && !none.running && !none.update && !none.removable && none.minimal_only.is_none());

        let stopped = offer("0.6.0", &mac(Some(AppPlace::Home), Some("0.6.0"), None));
        assert!(stopped.installed && !stopped.running && !stopped.update && stopped.removable);

        let running = offer("0.6.0", &mac(Some(AppPlace::Home), Some("0.6.0"), Some(("0.6.0+812", "headless"))));
        assert!(running.running && !running.update, "a build number is no other version");

        let older = offer("0.6.1", &mac(Some(AppPlace::Home), Some("0.6.0"), Some(("0.6.0", "headless"))));
        assert!(older.update, "the server's version differs: 원격 서버 업데이트");

        let newer_bundle = offer("0.6.0", &mac(Some(AppPlace::System), Some("0.6.2"), None));
        assert!(newer_bundle.update && !newer_bundle.removable, "an app in /Applications is used, never removed");
    }

    #[test]
    fn versions_are_compared_as_releases() {
        assert!(!version_differs("0.6.0", "v0.6.0 (abc123)"));
        assert!(!version_differs("0.6.0+12", "0.6.0"));
        assert!(version_differs("0.6.0", "0.6.1"));
        assert!(version_differs("0.6.0", "garbage"), "a version that does not read is not taken as the same");
    }
}
