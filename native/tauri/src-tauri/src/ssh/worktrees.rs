//! MC-47: an SSH workspace's pane worktrees and work map, from the app: the machine (its home and whether it has git, asked once per
//! machine; nothing about a worktree is ever kept here), the place its worktrees live (`pane_worktree::Place` over mighty-delegation
//! `machine::Remote`: `~/.mightycode/worktrees` over there), the ports something listens on over there (a port block skips them),
//! its `.mighty.yaml` read over there with the local reader's rules, and a worktree's setup or teardown command run over there.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use mighty_core::model::Workspace;
use mighty_core::ssh::command::sh;
use mighty_core::ssh::worktree::{self as scripts, ConfigFile};
use mighty_delegation::git::GitRunner as _;
use mighty_delegation::machine::Remote;
use mighty_delegation::mighty_yaml::{self, LoadedRepoConfig};
use mighty_delegation::pane_worktree::Worktrees;
use mighty_platform::SpawnRequest;
use mighty_run::ssh::SshLaunch;
use mighty_run::ssh_git::SshFolder;

use crate::host::{Host, lock};

/// An SSH machine's worktrees: which machine, and how it is reached.
#[derive(Clone)]
pub struct SshPlace {
    pub machine: String,
    pub launch: SshLaunch,
}

/// Each machine's home (its real path) once it was asked and had git; a machine without git is asked again next time.
static HOMES: LazyLock<Mutex<HashMap<String, String>>> = LazyLock::new(Mutex::default);

/// The place `workspace`'s pane worktrees live on its machine. Err with the sentence to show when the machine cannot be reached.
pub async fn place(host: &Arc<Host>, workspace: &Workspace) -> Result<crate::pane_worktree::Place, String> {
    let launch = super::launch_for(host, workspace)?;
    let machine = workspace.ssh.as_ref().map(|r| r.machine.clone()).unwrap_or_default();
    let known = lock(&HOMES).get(&machine).cloned();
    let remote = match known {
        Some(home) => Remote::known(host.processes.clone(), launch.clone(), home, true),
        None => {
            let remote = Remote::connect(host.processes.clone(), launch.clone())
                .await
                .map_err(|reason| host.locale.get_with("ssh.worktree.error.unreachable", &[("reason", &reason)]))?;
            if remote.git_available() {
                lock(&HOMES).insert(machine.clone(), remote.home().to_string());
            }
            remote
        }
    };
    let root = remote.root();
    Ok(crate::pane_worktree::Place { worktrees: Worktrees::new(Arc::new(remote), &root), ssh: Some(SshPlace { machine, launch }) })
}

/// The ports something listens on over there; empty when the machine says nothing it knows (a block then skips only the blocks
/// already given).
pub async fn listening(host: &Host, launch: &SshLaunch) -> BTreeSet<u16> {
    let folder = SshFolder::new(host.processes.clone(), launch.clone());
    match folder.exec(&scripts::listening_script(), None, Duration::from_secs(30), 4 << 20).await {
        Ok(ended) if ended.code == 0 => scripts::parse_listening(&String::from_utf8_lossy(&ended.stdout)).unwrap_or_default(),
        _ => BTreeSet::new(),
    }
}

/// A worktree's setup or teardown `command` run over there in `dir` (as the local one: `/bin/sh -c`, the login shell's `PATH`), with
/// `env`'s values; Err when a name or the folder cannot be given.
pub fn command_request(launch: &SshLaunch, dir: &str, command: &str, env: &[(String, String)]) -> Result<SpawnRequest, String> {
    let script = scripts::command_script(dir, command, env).ok_or_else(|| "the command cannot be given to the machine".to_string())?;
    Ok(launch.exec_request(sh(&script), &BTreeMap::new()))
}

/// The first of `names` in `dir` over there, as mighty-delegation `mighty_yaml` reads a local one: a link or anything but a regular
/// file is refused, as is one over the limit.
async fn config_file(folder: &SshFolder, dir: &str, names: &[&str]) -> Result<Option<(String, Vec<u8>)>, String> {
    let limit = mighty_yaml::MAXIMUM_CONFIG_BYTES;
    let script = scripts::config_script(dir, names, limit).ok_or_else(|| "the folder cannot be read".to_string())?;
    let ended = folder.exec(&script, None, Duration::from_secs(30), limit + 4096).await?;
    if ended.code != 0 {
        return Err(format!("the machine answered {}", ended.code));
    }
    match scripts::parse_config(&ended.stdout).ok_or_else(|| "the machine's answer could not be read".to_string())? {
        ConfigFile::Missing => Ok(None),
        ConfigFile::Link(name) => Err(format!("{name} must be a regular file, not a symbolic link.")),
        ConfigFile::NotFile(name) => Err(format!("{name} must be a regular file.")),
        ConfigFile::TooLarge(name) => Err(format!("{name} exceeds maximum size of {limit} bytes.")),
        ConfigFile::File(name, bytes) => Ok(Some((name, bytes))),
    }
}

/// `workspace`'s `.mighty.yaml` and `.mighty.local.yaml`, read in its folder over there.
pub async fn load_config(host: &Host, workspace: &Workspace) -> Result<Option<LoadedRepoConfig>, String> {
    let launch = super::launch_for(host, workspace)?;
    let folder = SshFolder::new(host.processes.clone(), launch.clone());
    let base = config_file(&folder, &launch.dir, &mighty_yaml::BASE_CONFIG_FILES).await?;
    let local = config_file(&folder, &launch.dir, &mighty_yaml::LOCAL_CONFIG_FILES).await?;
    mighty_yaml::load_from_bytes(base, local)
}

/// The work map's repository of an SSH workspace: its folder on its machine.
pub fn repository(host: &Host, workspace: &Workspace) -> Result<mighty_workmap::RemoteRepository, String> {
    let launch = super::launch_for(host, workspace)?;
    let dir = launch.dir.clone();
    Ok(mighty_workmap::RemoteRepository { folder: SshFolder::new(host.processes.clone(), launch), dir })
}
