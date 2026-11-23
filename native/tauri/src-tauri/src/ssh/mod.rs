//! MC-33: SSH machines — work in a folder of another computer with nothing installed there and no pairing. Authentication is the
//! person's OpenSSH (`~/.ssh/config`, ssh-agent, keys); the app never stores or sees a password or a key. The pure parts (names,
//! ssh_config, argument arrays, quoting, error words) are mighty-core's `ssh`; the run engine's remote owner is mighty-run's `ssh`.
//!
//! - **Machines**: `<data>/ssh/machines.json` (0600 in a 0700 folder): each machine's id, its name (a `Host` alias or
//!   `user@host[:port]`), what the last connection test read (OS, login shell, tmux) and the tmux switch. Read once, written only where
//!   the app writes its profile.
//! - **Connection test** (`연결 테스트`): `ssh -o BatchMode=yes … uname -sm; echo $SHELL; command -v tmux`, with a time limit and no
//!   prompt; not reachable, sign-in refused and an unknown host key are told apart, and a host key is never accepted for the person.
//! - **Workspaces**: a remote folder opened as a workspace whose local folder is an empty one of the app's own
//!   (`<data>/ssh/workspaces/<id>`); terminal panes are `ssh -tt` in a local PTY (terminal.rs), agent panes start the CLI on the far
//!   side (mighty-run's remote owner, [`resolver`]; MC-40: held on the far side, so a dropped connection is reconnected), the files pane reads and (MC-40) saves through `ssh` (`files`).
//! - **Shared connection**: on the Mac every pane of a machine shares one connection (ControlMaster, sockets in a 0700 folder);
//!   Windows' OpenSSH has no ControlMaster, so each pane has its own.
//! - Nothing polls a machine: a lost connection is ssh's own keep-alive ending the pane's ssh.

pub mod files;
pub mod forward;
pub mod review;
pub mod server;

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use mighty_core::model::Workspace;
use mighty_core::ssh::command::{self, Probe, Shell};
use mighty_core::ssh::failure::{self, Failure};
use mighty_core::ssh::{Mux, Target};
use mighty_run::ssh::{SshLaunch, SshResolver};
use serde::{Deserialize, Serialize};

use crate::host::Host;

/// The folder in the profile.
pub const FOLDER: &str = "ssh";
const MACHINES_FILE: &str = "machines.json";
/// The workspaces' empty local folders.
const WORKSPACES: &str = "workspaces";
/// The most machines kept.
pub const MAX_MACHINES: usize = 32;
/// The connection test's whole limit (ssh's own connect timeout is 10 s).
const TEST_TIMEOUT: Duration = Duration::from_secs(20);
/// The largest machines file read.
const MAX_FILE_BYTES: u64 = 256 * 1024;
/// How long the ssh_config's aliases are kept before they are read again.
const ALIASES_FOR: Duration = Duration::from_secs(30);
/// The sidebar id of a machine's header.
pub const SERVER_PREFIX: &str = "ssh-";

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|p| p.into_inner())
}

/// One SSH machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Machine {
    pub id: String,
    /// As [`Target`] writes it: `devbox`, `me@box:2222`.
    pub target: String,
    /// The person's tmux switch; None is on when the test found tmux.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tmux: Option<bool>,
    /// What the last successful test read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell: Option<String>,
    #[serde(default)]
    pub has_tmux: bool,
    /// The last test: `ok` or a [`Failure::word`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last: Option<String>,
    /// ssh's last line for a failure the app has no word for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl Machine {
    pub fn parsed(&self) -> Option<Target> {
        Target::parse(&self.target).ok()
    }

    /// Whether terminal panes run inside tmux: tmux was found and the person did not turn it off.
    pub fn uses_tmux(&self) -> bool {
        self.has_tmux && self.tmux != Some(false)
    }

    /// The machine's short name (the alias or host).
    pub fn name(&self) -> String {
        self.parsed().map(|t| t.host().to_string()).unwrap_or_else(|| self.target.clone())
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct File {
    #[serde(default)]
    machines: Vec<Machine>,
}

#[derive(Default)]
struct State {
    loaded: bool,
    machines: Vec<Machine>,
    testing: HashSet<String>,
    /// The settings box's last sentence (an add refused, a folder that did not open).
    message: Option<String>,
    aliases: Option<(Instant, Vec<String>)>,
    reading_aliases: bool,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn with<R>(host: &Host, change: impl FnOnce(&mut State) -> R) -> R {
    let mut guard = lock(&STATE);
    let state = guard.get_or_insert_with(State::default);
    if !state.loaded {
        state.loaded = true;
        state.machines = read_machines(&host.data_dir);
    }
    change(state)
}

pub fn folder(data_dir: &Path) -> PathBuf {
    data_dir.join(FOLDER)
}

fn read_machines(data_dir: &Path) -> Vec<Machine> {
    let path = folder(data_dir).join(MACHINES_FILE);
    let Ok(meta) = std::fs::symlink_metadata(&path) else { return vec![] };
    if !meta.is_file() || meta.len() > MAX_FILE_BYTES {
        return vec![];
    }
    let file: File = std::fs::read_to_string(&path).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default();
    let mut seen = HashSet::new();
    file.machines
        .into_iter()
        .filter(|m| mighty_core::wire::identifier(&m.id) && m.parsed().is_some() && seen.insert(m.id.clone()))
        .take(MAX_MACHINES)
        .collect()
}

fn save(host: &Host, machines: &[Machine]) {
    if !host.writes() {
        return;
    }
    let text = serde_json::to_string_pretty(&File { machines: machines.to_vec() }).unwrap_or_default();
    if let Err(error) = crate::control::endpoint::write_private(&folder(&host.data_dir).join(MACHINES_FILE), text.as_bytes()) {
        host.log.line(&format!("ssh: machines not saved: {error}"));
    }
}

fn changed(host: &Host) {
    host.shell_dirty();
    crate::native::touch(host, crate::settings::SURFACE);
}

pub fn machines(host: &Host) -> Vec<Machine> {
    with(host, |s| s.machines.clone())
}

pub fn machine(host: &Host, id: &str) -> Option<Machine> {
    with(host, |s| s.machines.iter().find(|m| m.id == id).cloned())
}

pub fn testing(host: &Host, id: &str) -> bool {
    with(host, |s| s.testing.contains(id))
}

pub fn message(host: &Host) -> Option<String> {
    with(host, |s| s.message.clone())
}

pub fn say(host: &Host, message: Option<String>) {
    with(host, |s| s.message = message);
    crate::native::touch(host, crate::settings::SURFACE);
}

/// Adds a machine by what the person typed or picked; Err with the sentence to show.
pub fn add(host: &Host, text: &str) -> Result<Machine, String> {
    let target = Target::parse(text).map_err(|error| host.locale.get(error.key()))?;
    let written = target.to_string();
    let added = with(host, |s| {
        if let Some(existing) = s.machines.iter().find(|m| m.target == written) {
            return Ok(existing.clone());
        }
        if s.machines.len() >= MAX_MACHINES {
            return Err(host.locale.get("ssh.settings.full"));
        }
        let machine = Machine {
            id: mighty_core::wire::new_id(),
            target: written,
            tmux: None,
            system: None,
            shell: None,
            has_tmux: false,
            last: None,
            detail: None,
        };
        s.machines.push(machine.clone());
        save(host, &s.machines);
        s.message = None;
        Ok(machine)
    })?;
    changed(host);
    Ok(added)
}

/// Removes a machine (its workspaces stay, under a header that says it is gone) and ends its shared connection.
pub async fn remove(host: &Arc<Host>, id: &str) {
    let removed = with(host, |s| {
        let at = s.machines.iter().position(|m| m.id == id)?;
        let machine = s.machines.remove(at);
        save(host, &s.machines);
        Some(machine)
    });
    if let Some(machine) = removed {
        server::forget(host, &machine.id).await;
        forward::close_machine(host, &machine.id).await;
        if let (Some(target), Some(program)) = (machine.parsed(), ssh_program()) {
            let mux = mux(&host.data_dir);
            if let Some(args) = command::exit_master_args(&target, &mux) {
                let request = mighty_platform::SpawnRequest::new(program, args, home_or(&host.data_dir));
                let _ = mighty_platform::process::capture(host.processes.as_ref(), request, None, Duration::from_secs(5), 64 * 1024).await;
            }
        }
    }
    changed(host);
}

/// The person's tmux switch for a machine.
pub fn set_tmux(host: &Host, id: &str, on: bool) {
    with(host, |s| {
        if let Some(machine) = s.machines.iter_mut().find(|m| m.id == id) {
            machine.tmux = Some(on);
            save(host, &s.machines);
        }
    });
    changed(host);
}

/// The local `ssh`: the Mac's `/usr/bin/ssh` (else the first on `PATH`); Windows' `%SystemRoot%\System32\OpenSSH\ssh.exe` (else
/// the first `ssh.exe` on `PATH`). None when there is none.
pub fn ssh_program() -> Option<PathBuf> {
    let windows = mighty_run::os::windows();
    let system = if windows {
        std::env::var_os("SystemRoot").map(|root| PathBuf::from(root).join("System32").join("OpenSSH").join("ssh.exe"))
    } else {
        Some(PathBuf::from("/usr/bin/ssh"))
    };
    if let Some(path) = system.filter(|p| p.is_file()) {
        return Some(path);
    }
    let name = if windows { "ssh.exe" } else { "ssh" };
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).map(|dir| dir.join(name)).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .find(|p| p.is_absolute() && p.is_file())
}

/// The longest ControlPath folder that leaves room for ssh's socket name (`/cm-` + 40 hex digits of `%C` + the 17 characters ssh adds
/// while it makes the socket) in a Unix socket's 104 bytes.
const CONTROL_FOLDER_MAX: usize = 104 - 1 - 4 - 40 - 17;

/// Where the shared connections' sockets live (0700): the profile's `ssh` folder, or — when its path is too long for a socket, as it
/// is under `~/Library/Application Support` — `/tmp/mc-<uid>-ssh`, checked to be this user's own folder and no link. Windows: none
/// (its OpenSSH has no ControlMaster); each pane has its own connection.
pub fn mux(data_dir: &Path) -> Mux {
    if mighty_run::os::windows() {
        return Mux::None;
    }
    match control_folder(data_dir) {
        Some(folder) => Mux::Shared(folder.to_string_lossy().into_owned()),
        None => Mux::None,
    }
}

fn control_folder(data_dir: &Path) -> Option<PathBuf> {
    let preferred = folder(data_dir);
    if preferred.to_string_lossy().len() <= CONTROL_FOLDER_MAX {
        return private_folder(&preferred).then_some(preferred);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        let uid = std::fs::metadata(data_dir).ok()?.uid();
        let short = PathBuf::from(format!("/tmp/mc-{uid}-ssh"));
        let ours = |path: &Path| std::fs::symlink_metadata(path).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink() && m.uid() == uid);
        if private_folder(&short) && ours(&short) {
            return Some(short);
        }
    }
    None
}

/// A folder only this user can open (0700), made where it is new; never a link.
fn private_folder(path: &Path) -> bool {
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        if meta.file_type().is_symlink() || !meta.is_dir() {
            return false;
        }
    } else {
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt as _;
            if std::fs::DirBuilder::new().recursive(true).mode(0o700).create(path).is_err() {
                return false;
            }
        }
        #[cfg(not(unix))]
        if std::fs::create_dir_all(path).is_err() {
            return false;
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).is_ok()
    }
    #[cfg(windows)]
    {
        crate::platform::trust_restrict(path, true).is_ok()
    }
    #[cfg(not(any(unix, windows)))]
    {
        true
    }
}

/// The folder ssh is started in where nothing else fits: the home folder, else the data folder.
fn home_or(data_dir: &Path) -> PathBuf {
    std::env::home_dir().filter(|h| h.is_dir()).unwrap_or_else(|| data_dir.to_path_buf())
}

/// An SSH workspace's empty local folder (made when missing).
pub fn stand_in(data_dir: &Path, workspace_id: &str) -> PathBuf {
    folder(data_dir).join(WORKSPACES).join(workspace_id)
}

/// How to reach `workspace`'s remote folder; Err with the sentence to show.
pub fn launch_for(host: &Host, workspace: &Workspace) -> Result<SshLaunch, String> {
    let reference = workspace.ssh.as_ref().ok_or_else(|| host.locale.get("ssh.run.error.unavailable"))?;
    let machine =
        machine(host, &reference.machine).ok_or_else(|| host.locale.get_with("ssh.error.machineGone", &[("host", &reference.host)]))?;
    let target = machine.parsed().ok_or_else(|| host.locale.get("ssh.target.error.invalid"))?;
    let program = ssh_program().ok_or_else(|| host.locale.get(Failure::NoSsh.key()))?;
    let local = PathBuf::from(workspace.path());
    if !local.is_dir() {
        let _ = std::fs::create_dir_all(&local);
    }
    let local = if local.is_dir() { local } else { home_or(&host.data_dir) };
    Ok(SshLaunch { program, target, mux: mux(&host.data_dir), dir: reference.dir.clone(), local })
}

/// How to reach a machine itself (no workspace): its ssh in the remote home, started in this computer's home folder (MC-33's server
/// steps); Err with the sentence to show.
pub fn launch_for_machine(host: &Host, machine_id: &str) -> Result<SshLaunch, String> {
    let machine = machine(host, machine_id).ok_or_else(|| host.locale.get("ssh.error.machineGoneShort"))?;
    let target = machine.parsed().ok_or_else(|| host.locale.get("ssh.target.error.invalid"))?;
    let program = ssh_program().ok_or_else(|| host.locale.get(Failure::NoSsh.key()))?;
    Ok(SshLaunch { program, target, mux: mux(&host.data_dir), dir: "~".into(), local: home_or(&host.data_dir) })
}

/// What a terminal pane of `workspace` runs: tmux (named by the pane) where the machine has it and the switch is on, else the login
/// shell.
pub fn shell_for(host: &Host, workspace: &Workspace, pane: &str) -> Shell {
    let tmux = workspace.ssh.as_ref().and_then(|r| machine(host, &r.machine)).is_some_and(|m| m.uses_tmux());
    if tmux { Shell::Tmux(command::tmux_session(pane)) } else { Shell::Login }
}

/// The run engine's way to an SSH workspace (runs.rs).
pub fn resolver(host: &Arc<Host>) -> SshResolver {
    let host = Arc::downgrade(host);
    Arc::new(move |workspace: &Workspace| {
        let host = host.upgrade().ok_or_else(|| "the app is closing".to_string())?;
        launch_for(&host, workspace)
    })
}

/// The connection test: runs once at a time per machine; what it read is saved and the settings box redrawn.
pub async fn test(host: &Arc<Host>, id: &str) {
    let Some(machine) = machine(host, id) else { return };
    if !with(host, |s| s.testing.insert(id.to_string())) {
        return;
    }
    changed(host);
    let outcome = probe(host, &machine).await;
    with(host, |s| {
        s.testing.remove(id);
        if let Some(saved) = s.machines.iter_mut().find(|m| m.id == id) {
            match &outcome {
                Ok(probe) => {
                    saved.last = Some("ok".into());
                    saved.detail = None;
                    saved.system = Some(probe.system.clone());
                    saved.shell = Some(probe.shell.clone());
                    saved.has_tmux = probe.tmux;
                }
                Err(failure) => {
                    saved.last = Some(failure.word().into());
                    saved.detail = match failure {
                        Failure::Other(line) if !line.is_empty() => Some(line.clone()),
                        _ => None,
                    };
                }
            }
            save(host, &s.machines);
        }
    });
    changed(host);
}

async fn probe(host: &Host, machine: &Machine) -> Result<Probe, Failure> {
    let target = machine.parsed().ok_or_else(|| Failure::Other(String::new()))?;
    let program = ssh_program().ok_or(Failure::NoSsh)?;
    let request = mighty_platform::SpawnRequest::new(program, command::test_args(&target), home_or(&host.data_dir));
    match mighty_platform::process::capture(host.processes.as_ref(), request, None, TEST_TIMEOUT, 256 * 1024).await {
        Ok(captured) if captured.exit_code == 0 => {
            command::parse_probe(&String::from_utf8_lossy(&captured.stdout)).ok_or_else(|| Failure::Other(String::new()))
        }
        Ok(captured) => Err(failure::classify(&String::from_utf8_lossy(&captured.stderr))),
        Err(mighty_platform::PlatformError::TimedOut(_)) => Err(Failure::TimedOut),
        Err(_) => Err(Failure::NoSsh),
    }
}

/// The sentence of a machine's state: never tested, testing, connected (OS · shell · tmux), or what went wrong.
pub fn state_words(host: &Host, machine: &Machine) -> (String, &'static str) {
    let l = &host.locale;
    if testing(host, &machine.id) {
        return (l.get("ssh.state.testing"), "wait");
    }
    match machine.last.as_deref() {
        None => (l.get("ssh.state.untested"), "idle"),
        Some("ok") => {
            let mut parts: Vec<String> =
                [machine.system.clone(), machine.shell.clone()].into_iter().flatten().filter(|s| !s.is_empty()).collect();
            if machine.has_tmux {
                parts.push("tmux".into());
            }
            (l.get_with("ssh.state.ok", &[("detail", &parts.join(" · "))]), "done")
        }
        Some(word) => {
            let failure = match word {
                "unreachable" => Failure::Unreachable,
                "auth" => Failure::AuthFailed,
                "hostKeyUnknown" => Failure::HostKeyUnknown,
                "hostKeyChanged" => Failure::HostKeyChanged,
                "noSsh" => Failure::NoSsh,
                "timedOut" => Failure::TimedOut,
                _ => Failure::Other(machine.detail.clone().unwrap_or_default()),
            };
            let mut text = l.get_with(failure.key(), &[("host", &machine.target)]);
            if let Failure::Other(line) = &failure
                && !line.is_empty()
            {
                text = format!("{text} {line}");
            }
            (text, "error")
        }
    }
}

/// The `Host` aliases of `~/.ssh/config` (wildcards and `Match` blocks left out), read off the window's thread and kept a while; the
/// settings box is redrawn when a reading lands. The builder calls this (no I/O on its thread).
pub fn aliases(host: &Arc<Host>) -> Vec<String> {
    let (cached, stale) = with(host, |s| {
        let cached = s.aliases.as_ref().map(|(_, list)| list.clone()).unwrap_or_default();
        let stale = s.aliases.as_ref().is_none_or(|(at, _)| at.elapsed() > ALIASES_FOR) && !s.reading_aliases;
        if stale {
            s.reading_aliases = true;
        }
        (cached, stale)
    });
    if stale {
        let host = host.clone();
        tauri::async_runtime::spawn(async move {
            let read = tauri::async_runtime::spawn_blocking(|| {
                let Some(home) = std::env::home_dir() else { return vec![] };
                mighty_core::ssh::config::host_aliases(&home.join(".ssh").join("config"), &home, &mighty_core::ssh::config::Disk)
            })
            .await
            .unwrap_or_default();
            let changed_list = with(&host, |s| {
                s.reading_aliases = false;
                let before = s.aliases.as_ref().map(|(_, l)| l.clone());
                s.aliases = Some((Instant::now(), read.clone()));
                before.as_ref() != Some(&read)
            });
            if changed_list {
                crate::native::touch(&host, crate::settings::SURFACE);
            }
        });
    }
    cached
}

/// Opens `dir` of a machine as a workspace (the one listed for that machine and folder, if any), selects it and gives a new one a
/// terminal pane. Err with the sentence to show.
pub async fn open_folder(host: &Arc<Host>, machine_id: &str, dir: &str) -> Result<(), String> {
    let dir = dir.trim();
    let dir = if dir.len() > 1 { dir.trim_end_matches('/') } else { dir };
    let machine = machine(host, machine_id).ok_or_else(|| host.locale.get("ssh.error.machineGoneShort"))?;
    if !mighty_core::ssh::valid_remote_dir(dir) {
        return Err(host.locale.get("ssh.open.error.dir"));
    }
    let data_dir = host.data_dir.clone();
    let workspace = host
        .approve_ssh(&machine.id, &machine.target, dir, |id| {
            let path = stand_in(&data_dir, id);
            let _ = std::fs::create_dir_all(&path);
            path.to_string_lossy().into_owned()
        })
        .ok_or_else(|| host.locale.get("ssh.open.error.dir"))?;
    let has_panes = {
        let mut view = host.view();
        let id = workspace.id().to_string();
        view.update(|state| {
            if !state.workspaces().iter().any(|w| w.id() == id) {
                state.workspaces_mut().push(workspace.clone());
            }
            if let Some(expanded) = mighty_core::shell::open_disclosure(state, &id) {
                state.expanded_workspace_ids = Some(expanded);
            }
            state.active_workspace_id = Some(id.clone());
        })?;
        view.shows_dashboard = false;
        view.snapshot.sessions().iter().any(|s| s.workspace_id() == id)
    };
    if !has_panes {
        crate::frame::add_pane(host, "shell", "claude", None, |pane| pane)?;
    }
    crate::settings::close(host);
    Ok(())
}

/// The sidebar's headers for the SSH machines (after the remote mirror's servers), and the SSH workspaces' rows under them: their
/// path shown as `host:dir`. Read with the view locked: only this module's memory.
pub fn frame(host: &Host, snapshot: &mighty_core::AppSnapshot, model: &mut mighty_core::shell::ShellModel) {
    let l = &host.locale;
    let list = machines(host);
    let mut headers: Vec<(String, String, Option<&Machine>)> =
        list.iter().map(|m| (format!("{SERVER_PREFIX}{}", m.id), m.name(), Some(m))).collect();
    for workspace in snapshot.workspaces() {
        let Some(reference) = &workspace.ssh else { continue };
        let id = format!("{SERVER_PREFIX}{}", reference.machine);
        if !headers.iter().any(|(h, _, _)| *h == id) {
            headers.push((id, reference.host.clone(), None));
        }
    }
    for (id, name, machine) in headers {
        let (state, tone) = match machine {
            Some(machine) => state_words(host, machine),
            None => (l.get("ssh.state.removed"), "idle"),
        };
        let address = machine.map(|m| m.target.clone()).unwrap_or_default();
        let line = [address.clone(), state.clone()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
        model.servers.push(mighty_core::shell::RemoteServer {
            title: l.get_with("ssh.sidebar.server", &[("name", &name)]),
            line,
            state: state.clone(),
            tone,
            connected: tone == "done",
            known: machine.is_some(),
            help: state,
            ssh: true,
            id,
        });
    }
    for row in &mut model.workspaces {
        let Some(reference) = snapshot.workspace(&row.id).and_then(|w| w.ssh.as_ref()) else { continue };
        row.local = false;
        row.server = Some(format!("{SERVER_PREFIX}{}", reference.machine));
        row.path = format!("{}:{}", reference.host, reference.dir);
        row.ssh = true;
    }
    if let Some(header) = &mut model.header
        && let Some(reference) = snapshot.workspace(&header.id).and_then(|w| w.ssh.as_ref())
    {
        header.path = format!("{}:{}", reference.host, reference.dir);
        header.git = None;
        header.work_map_enabled = false;
        header.work_map_help = l.get("ssh.workMap.unavailable");
        header.remote_host = Some(l.get_with("ssh.sidebar.server", &[("name", &reference.host)]));
        model.status.machine = l.get_with("ssh.status.machine", &[("host", &reference.host)]);
    }
}

/// The page's SSH actions: the sidebar header's menu (test, the settings) and a forward from a pane's header.
pub fn register(methods: &mut crate::host::Registry) {
    methods.add("sshMachineTest", |host: Arc<Host>, args: serde_json::Value| async move {
        let id = crate::methods::text(&args, "id").unwrap_or("").trim_start_matches(SERVER_PREFIX).to_string();
        let task = host.clone();
        tauri::async_runtime::spawn(async move { test(&task, &id).await });
        Ok(serde_json::Value::Null)
    });
    methods.add("sshSettings", |host: Arc<Host>, _args: serde_json::Value| async move {
        crate::settings::open(&host, Some("remote"));
        Ok(serde_json::Value::Null)
    });
    forward::register(methods);
}

/// The text of a short remote command's output, capped; Err with the failure.
pub async fn exec(host: &Host, launch: &SshLaunch, remote: String, cap: usize) -> Result<(i32, Vec<u8>), Failure> {
    exec_input(host, launch, remote, None, cap, Duration::from_secs(30)).await
}

/// [`exec`] with bytes for the remote command's standard input (a file being written, MC-40) and a limit of its own.
pub async fn exec_input(
    host: &Host,
    launch: &SshLaunch,
    remote: String,
    input: Option<Vec<u8>>,
    cap: usize,
    limit: Duration,
) -> Result<(i32, Vec<u8>), Failure> {
    let request = launch.exec_request(remote, &BTreeMap::new());
    match mighty_platform::process::capture(host.processes.as_ref(), request, input, limit, cap + 64 * 1024).await {
        Ok(captured) if captured.exit_code == command::SSH_FAILED => Err(failure::classify(&String::from_utf8_lossy(&captured.stderr))),
        Ok(captured) => Ok((captured.exit_code, captured.stdout)),
        Err(mighty_platform::PlatformError::TimedOut(_)) => Err(Failure::TimedOut),
        Err(_) => Err(Failure::NoSsh),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_machines_file_keeps_only_valid_machines() {
        let dir = std::env::temp_dir().join(format!("mc-ssh-machines-{}", std::process::id()));
        std::fs::create_dir_all(folder(&dir)).unwrap();
        std::fs::write(
            folder(&dir).join(MACHINES_FILE),
            r#"{"machines":[
                {"id":"0123456789abcdef0123456789abcdef","target":"devbox","hasTmux":true},
                {"id":"0123456789abcdef0123456789abcdef","target":"again"},
                {"id":"fedcba9876543210fedcba9876543210","target":"-oProxyCommand=x"},
                {"id":"not an id","target":"box"}
            ]}"#,
        )
        .unwrap();
        let machines = read_machines(&dir);
        assert_eq!(machines.len(), 1);
        assert!(machines[0].uses_tmux());
        let off = Machine { tmux: Some(false), ..machines[0].clone() };
        assert!(!off.uses_tmux());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_control_folder_fits_a_socket_name() {
        let short = Path::new("/tmp/mc-ssh-short-data");
        assert!(folder(short).to_string_lossy().len() <= CONTROL_FOLDER_MAX);
        // `<folder>/cm-` + `%C` + ssh's own suffix stays inside sun_path.
        const { assert!(CONTROL_FOLDER_MAX + 4 + 40 + 17 < 104) };
    }
}
