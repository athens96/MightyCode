//! Worktree port blocks and the worktree's script lifecycle in the host (`MC-25`, over mighty-delegation's `worktree_ports` and
//! `run_actions`).
//!
//! - Port blocks: a pane worktree gets its block when it is made (after its folder exists, before its setup runs), the main checkout
//!   when a task or run action first runs there (or the task panel lists it). They are remembered in `<data>/ports.json` by the
//!   normalized main checkout and worktree paths, released when the worktree is removed, and at the start the blocks of worktrees
//!   that vanished while the app was closed are pruned (and those open panes still hold are recorded again).
//! - Teardown: before a worktree is discarded (the bar, its list, or the cleanup when its pane closes after a merge) the managed
//!   runs in it stop, then the approved `.mighty.yaml`'s `worktree.teardown` runs in its folder with its time limit. A failure does
//!   not stop the removal; the result goes to the pane's transcript and the task panel's runs (and the banner when it failed).
//! - Nothing runs for a remote workspace, nor before the repository config is approved (`repo_tasks::load_workspace_config`).

use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};

use mighty_core::model::{PaneWorktree, Workspace, worktree_variables};
use mighty_delegation::mighty_yaml::{DEFAULT_WORKTREE_PORTS, LoadedRepoConfig, MAXIMUM_WORKTREE_PORTS};
use mighty_delegation::run_actions;
use mighty_delegation::worktree_ports::{self, DEFAULT_START, PortBlock, PortsFile};

use crate::host::{Host, lock};

#[derive(Default)]
struct PortsState {
    loaded: bool,
    file: PortsFile,
}

static PORTS: LazyLock<Mutex<PortsState>> = LazyLock::new(Mutex::default);

/// Runs `change` over `ports.json` (read once), writing it back when it changed and the host writes.
fn with_ports<T>(host: &Host, change: impl FnOnce(&mut PortsFile) -> T) -> T {
    let mut state = lock(&PORTS);
    if !state.loaded {
        state.file = worktree_ports::load(&host.data_dir);
        state.loaded = true;
    }
    let before = state.file.clone();
    let result = change(&mut state.file);
    if state.file != before
        && host.writes()
        && let Err(error) = worktree_ports::save(&host.data_dir, &state.file)
    {
        host.log.line(&format!("ports.json: {error}"));
    }
    result
}

/// Whether `port` is taken now: a smoke binds nothing, so every port reads free there.
fn in_use(host: &Host, port: u16) -> bool {
    host.smoke.is_none() && worktree_ports::port_in_use(port)
}

/// The key a path has in `ports.json` (the trust store's normalized path).
pub fn path_key(path: &str) -> String {
    crate::repo_tasks::canonical_workspace_key(path)
}

/// The top folder of the git checkout `folder` is in (the nearest folder holding `.git`); None outside a repository.
pub fn repo_root(folder: &Path) -> Option<PathBuf> {
    folder.ancestors().find(|dir| dir.join(".git").exists()).map(Path::to_path_buf)
}

/// The size of a block the config asks for (`worktree.ports`, 10 without a config).
pub fn port_count(config: Option<&LoadedRepoConfig>) -> u16 {
    let count = config.map_or(DEFAULT_WORKTREE_PORTS, |c| c.config.worktree.port_count()).min(MAXIMUM_WORKTREE_PORTS);
    u16::try_from(count).unwrap_or(0)
}

/// `record`'s block as mighty-delegation's [`PortBlock`].
pub fn block_of(record: &PaneWorktree) -> Option<PortBlock> {
    let (base, count) = record.port_block()?;
    Some(PortBlock { base: u16::try_from(base).ok()?, count: u16::try_from(count).ok()? })
}

/// Gives the new worktree `record` its block of `count` ports (none with `count` 0), written into the record.
pub fn assign_worktree(host: &Host, record: &mut PaneWorktree, count: u16) {
    let (repo, worktree) = (path_key(record.checkout()), path_key(record.worktree_path()));
    let block = with_ports(host, |file| file.assign(&repo, &worktree, count, DEFAULT_START, |port| in_use(host, port)));
    record.port_base = block.map(|b| i32::from(b.base));
    record.port_count = block.map(|b| i32::from(b.count));
}

/// MC-47: [`assign_worktree`] at the worktree's place: on an SSH machine the block is kept under the machine's own keys and skips
/// the ports something listens on over there (read now), not this computer's.
pub async fn assign_worktree_at(host: &Host, place: &crate::pane_worktree::Place, record: &mut PaneWorktree, count: u16) {
    let Some(ssh) = &place.ssh else { return assign_worktree(host, record, count) };
    let listening = if count > 0 { crate::ssh::worktrees::listening(host, &ssh.launch).await } else { Default::default() };
    let (repo, worktree) = (place.port_key(record.checkout()), place.port_key(record.worktree_path()));
    let block = with_ports(host, |file| file.assign(&repo, &worktree, count, DEFAULT_START, |port| listening.contains(&port)));
    record.port_base = block.map(|b| i32::from(b.base));
    record.port_count = block.map(|b| i32::from(b.count));
}

/// Gives back the block of the removed worktree `record` (MC-47: kept under its place's keys: an SSH machine's own).
pub fn release_worktree_at(host: &Host, place: &crate::pane_worktree::Place, record: &PaneWorktree) {
    let (repo, worktree) = (place.port_key(record.checkout()), place.port_key(record.worktree_path()));
    with_ports(host, |file| file.release(&repo, &worktree));
}

/// The main checkout's place for tasks and run actions: its repository key, its block (assigned on first use) and the variables its
/// runs get. Outside a git checkout: the folder's key, no block, no variables.
pub fn main_place(host: &Host, workspace_folder: &Path, count: u16) -> (String, Option<PortBlock>, Vec<(String, String)>) {
    let Some(root) = repo_root(workspace_folder) else {
        return (path_key(&workspace_folder.to_string_lossy()), None, Vec::new());
    };
    let root_text = root.to_string_lossy().into_owned();
    let repo = path_key(&root_text);
    let block = with_ports(host, |file| file.assign(&repo, &repo, count, DEFAULT_START, |port| in_use(host, port)));
    let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let variables = worktree_variables(block.map(|b| (i32::from(b.base), i32::from(b.count))), &name, &root_text, &root_text);
    (repo, block, variables)
}

/// At the start: prunes the blocks of worktrees that are gone, and records again the blocks open and kept panes still hold.
pub fn start(host: &Arc<Host>) {
    let held: Vec<PaneWorktree> = {
        let view = host.view();
        let open = view.snapshot.sessions().iter().filter_map(|s| s.pane_worktree.clone());
        let kept =
            view.snapshot.workspaces().iter().flat_map(|w| w.kept_pane_worktrees.iter().flatten()).filter_map(|k| k.worktree.clone());
        open.chain(kept).collect()
    };
    let host = host.clone();
    tauri::async_runtime::spawn_blocking(move || {
        with_ports(&host, |file| {
            // An SSH machine's blocks (MC-47) are its own keys, never a folder here: they go when their worktree is removed.
            let released = file.prune(|path| worktree_ports::is_remote_key(path) || Path::new(path).is_dir());
            if !released.is_empty() {
                host.log.line(&format!("ports.json: released {} block(s) of removed worktrees", released.len()));
            }
            for record in &held {
                if let Some(block) = block_of(record)
                    && Path::new(record.worktree_path()).is_dir()
                {
                    file.adopt(&path_key(record.checkout()), &path_key(record.worktree_path()), block);
                }
            }
        });
    });
}

/// Before the worktree `record` of the pane `session_id` (of `workspace_id`) is removed: its managed runs stop, then the approved
/// `worktree.teardown` runs in its folder within its time limit. Never blocks the removal. MC-47: an SSH workspace's teardown runs on
/// its machine, in the worktree there (`place`).
pub async fn before_remove(
    host: &Arc<Host>,
    workspace_id: &str,
    session_id: &str,
    record: &PaneWorktree,
    place: Option<&crate::pane_worktree::Place>,
) {
    let folder = record.run_folder().to_string();
    let ssh = place.and_then(|place| place.ssh.clone());
    if ssh.is_none() {
        crate::repo_tasks::stop_runs_in(host, &path_key(&folder)).await;
    }

    let Some(workspace) = host.view().snapshot.workspace(workspace_id).cloned() else { return };
    if workspace.remote.is_some() || ssh.is_none() && !Path::new(&folder).is_dir() || ssh.is_some() && workspace.ssh.is_none() {
        return;
    }
    let status = match crate::repo_tasks::load_worktree_config(host, &workspace).await {
        Ok(Some(status)) => status,
        _ => return,
    };
    let Some(command) = status.loaded.config.worktree.teardown.clone() else { return };
    let locale = host.locale;
    if !status.approved {
        crate::pane_worktree::append_system_note(host, session_id, locale.get("paneWorktree.log.teardownSkipped"));
        return;
    }
    if ssh.is_some() && !host.may_change_machine()
        || ssh.is_none() && host.smoke.is_some() && !crate::delegation::may_touch(host, Path::new(&folder))
    {
        return;
    }
    let timeout = status.loaded.config.worktree.teardown_timeout();
    crate::pane_worktree::append_system_note(
        host,
        session_id,
        locale.get_with("paneWorktree.log.teardownStarted", &[("command", &command)]),
    );

    let outcome = run_teardown(host, &workspace, record, &status.loaded, &command, timeout, ssh.as_ref()).await;
    crate::repo_tasks::record_teardown(host, workspace.id(), session_id, &command, &outcome, timeout.as_secs());
    let text = if outcome.succeeded() {
        locale.get_with("paneWorktree.log.teardownDone", &[("command", &command)])
    } else if outcome.timed_out {
        locale.get_with("paneWorktree.log.teardownTimedOut", &[("command", &command), ("seconds", &timeout.as_secs().to_string())])
    } else {
        let code = outcome.exit_code.map_or_else(|| "-1".to_string(), |code| code.to_string());
        locale.get_with("paneWorktree.log.teardownFailed", &[("command", &command), ("code", &code)])
    };
    crate::pane_worktree::append_system_note(host, session_id, text.clone());
    if !outcome.succeeded() {
        host.view().error = Some(text);
        host.schedule();
    }
}

async fn run_teardown(
    host: &Arc<Host>,
    workspace: &Workspace,
    record: &PaneWorktree,
    config: &LoadedRepoConfig,
    command: &str,
    timeout: std::time::Duration,
    ssh: Option<&crate::ssh::worktrees::SshPlace>,
) -> run_actions::ScriptOutcome {
    if let (Some(ssh), Some(reference)) = (ssh, &workspace.ssh) {
        // On the machine, in the worktree, with the worktree's values (never this computer's environment).
        let mut values: Vec<(String, String)> = config.config.worktree.env.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        for (key, value) in record.environment(&reference.dir) {
            values.retain(|(k, _)| *k != key);
            values.push((key, value));
        }
        return match crate::ssh::worktrees::command_request(&ssh.launch, record.run_folder(), command, &values) {
            Ok(request) => run_actions::run_bounded(host.processes.as_ref(), request, timeout).await,
            Err(error) => run_actions::ScriptOutcome { exit_code: None, timed_out: false, output: String::new(), error: Some(error) },
        };
    }
    let ws_folder = workspace.path().to_string();
    let mut request = crate::repo_tasks::shell_request(command, record.run_folder());
    for (key, value) in crate::repo_tasks::login_shell_env(host, Some(Path::new(&ws_folder))).await {
        request = request.env(key, value);
    }
    for (key, value) in &config.config.worktree.env {
        request = request.env(key.clone(), value.clone());
    }
    for (key, value) in record.environment(&ws_folder) {
        request = request.env(key, value);
    }
    run_actions::run_bounded(host.processes.as_ref(), request, timeout).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_repository_root_is_the_nearest_folder_with_git() {
        let dir = std::env::temp_dir().join(format!("mighty-lifecycle-root-{}", mighty_core::wire::new_id()));
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::create_dir_all(dir.join("apps/web")).unwrap();
        assert_eq!(repo_root(&dir.join("apps/web")).as_deref(), Some(dir.as_path()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_block_size_follows_the_config() {
        assert_eq!(port_count(None), 10);
        let loaded = |text: &str| LoadedRepoConfig {
            config: mighty_delegation::mighty_yaml::parse_config(text).unwrap(),
            files: vec![],
            content_hash: String::new(),
        };
        assert_eq!(port_count(Some(&loaded("worktree:\n  ports: 0\n"))), 0);
        assert_eq!(port_count(Some(&loaded("worktree:\n  ports: 25\n"))), 25);
        let record = PaneWorktree { port_base: Some(41_010), port_count: Some(10), ..PaneWorktree::default() };
        assert_eq!(block_of(&record), Some(PortBlock { base: 41_010, count: 10 }));
        assert_eq!(block_of(&PaneWorktree::default()), None);
    }
}
