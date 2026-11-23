//! MC-33: 로컬로 연결 — a dev server on an SSH machine reached from this computer: `ssh -N -L 127.0.0.1:<local>:localhost:<remote>`
//! over the machine's shared connection, the local port picked past every MC-25 port block and every port in use. The ports offered are
//! the ones the pane's output named (a terminal's, or an agent run's last entries); each open forward shows on the pane's SSH bar and
//! is torn down when the pane closes, the machine is removed or the app quits.

use std::sync::{Arc, OnceLock};

use mighty_core::shell::{PaneSsh, PaneSshPort, ShellModel};
use mighty_delegation::worktree_ports;
use mighty_run::ssh::{ForwardInfo, Forwards};
use serde_json::{Value, json};

use crate::host::{Host, Registry};

fn forwards() -> &'static Arc<Forwards> {
    static FORWARDS: OnceLock<Arc<Forwards>> = OnceLock::new();
    FORWARDS.get_or_init(Arc::default)
}

/// The most recent agent log entries read for dev-server addresses.
const LOG_ENTRIES_READ: usize = 12;

/// Opens a forward of `pane`'s machine to its `remote` port; the local port it listens on.
pub async fn open(host: &Arc<Host>, pane: &str, remote: u16) -> Result<u16, String> {
    if remote < 1024 {
        return Err(host.locale.get("ssh.forward.error.port"));
    }
    let workspace = {
        let view = host.view();
        let session = view.snapshot.session(pane).ok_or_else(|| "Session not found.".to_string())?;
        view.snapshot.workspace(session.workspace_id()).filter(|w| w.is_ssh()).cloned().ok_or_else(|| "Session not found.".to_string())?
    };
    if let Some(open) = forwards().list().into_iter().find(|f| f.pane == pane && f.remote == remote) {
        return Ok(open.local);
    }
    let launch = super::launch_for(host, &workspace)?;
    let machine = workspace.ssh.as_ref().map(|r| r.machine.clone()).unwrap_or_default();
    let data_dir = host.data_dir.clone();
    let taken_locals: Vec<u16> = forwards().list().iter().map(|f| f.local).collect();
    let local = tauri::async_runtime::spawn_blocking(move || {
        let mut taken = worktree_ports::load(&data_dir).assigned();
        taken.extend(taken_locals.iter().map(|&base| worktree_ports::PortBlock { base, count: 1 }));
        worktree_ports::find_free_block(&taken, 1, worktree_ports::DEFAULT_START, worktree_ports::port_in_use).map(|b| b.base)
    })
    .await
    .map_err(|e| e.to_string())?
    .ok_or_else(|| host.locale.get("ssh.forward.error.noPort"))?;
    let info = ForwardInfo { pane: pane.to_string(), machine, remote, local };
    let ended = Arc::downgrade(host);
    forwards()
        .open(
            host.processes.as_ref(),
            launch,
            info,
            Box::new(move || {
                if let Some(host) = ended.upgrade() {
                    host.shell_dirty();
                }
            }),
        )
        .await?;
    host.shell_dirty();
    Ok(local)
}

/// Closes `pane`'s forward of `remote` (every one of the pane's with None).
pub async fn close(host: &Host, pane: &str, remote: Option<u16>) {
    let closed = forwards().close_where(host.processes.as_ref(), |f| f.pane == pane && remote.is_none_or(|r| r == f.remote)).await;
    if closed > 0 {
        host.shell_dirty();
    }
}

/// MC-33: the forward of `machine`'s Mighty Code server (its loopback listener's `remote` port on the far side), held under a pane
/// name of its own (no pane shows it); one that is open to that port already is kept, one to another port (the server restarted) is
/// closed first. `ended` runs if its ssh ends by itself (the connection went). The local port it listens on.
pub async fn open_server(
    host: &Arc<Host>,
    machine: &str,
    launch: mighty_run::ssh::SshLaunch,
    remote: u16,
    ended: Box<dyn FnOnce() + Send>,
) -> Result<u16, String> {
    let pane = server_pane(machine);
    if let Some(open) = forwards().list().into_iter().find(|f| f.pane == pane && f.remote == remote) {
        return Ok(open.local);
    }
    forwards().close_where(host.processes.as_ref(), |f| f.pane == pane).await;
    let data_dir = host.data_dir.clone();
    let taken_locals: Vec<u16> = forwards().list().iter().map(|f| f.local).collect();
    let local = tauri::async_runtime::spawn_blocking(move || {
        let mut taken = worktree_ports::load(&data_dir).assigned();
        taken.extend(taken_locals.iter().map(|&base| worktree_ports::PortBlock { base, count: 1 }));
        worktree_ports::find_free_block(&taken, 1, worktree_ports::DEFAULT_START, worktree_ports::port_in_use).map(|b| b.base)
    })
    .await
    .map_err(|e| e.to_string())?
    .ok_or_else(|| host.locale.get("ssh.forward.error.noPort"))?;
    let info = ForwardInfo { pane, machine: machine.to_string(), remote, local };
    forwards().open(host.processes.as_ref(), launch, info, ended).await?;
    Ok(local)
}

/// Whether `machine`'s server forward is open now.
pub fn server_open(machine: &str) -> bool {
    let pane = server_pane(machine);
    forwards().list().iter().any(|f| f.pane == pane)
}

/// The forward of `machine`'s server closed (its server removed).
pub async fn close_server(host: &Host, machine: &str) {
    let pane = server_pane(machine);
    forwards().close_where(host.processes.as_ref(), |f| f.pane == pane).await;
}

/// The pane name the server forward is held under (never a pane id: those are uuids).
fn server_pane(machine: &str) -> String {
    format!("server:{machine}")
}

/// Every forward of a removed machine.
pub async fn close_machine(host: &Host, machine: &str) {
    forwards().close_where(host.processes.as_ref(), |f| f.machine == machine).await;
}

/// Every forward (the app quits).
pub async fn close_all(host: &Host) {
    forwards().close_where(host.processes.as_ref(), |_| true).await;
}

/// The SSH bar of each SSH workspace's pane: its machine, the ports its output named and the forwards open (host.rs' frame, the view
/// locked: this module's memory and the state only).
pub fn decorate(host: &Host, snapshot: &mighty_core::AppSnapshot, model: &mut ShellModel) {
    let open = forwards().list();
    let terminals = crate::terminal::terminals(host).ok();
    for pane in &mut model.panes {
        let Some(session) = snapshot.session(&pane.id) else { continue };
        let Some(reference) = snapshot.workspace(session.workspace_id()).and_then(|w| w.ssh.as_ref()) else { continue };
        let mut offered: Vec<u16> = Vec::new();
        let mut lost = false;
        if session.kind() == "shell" {
            if let Some(terminal) = terminals.as_ref().and_then(|t| t.existing(&pane.id)) {
                offered = terminal.seen_ports();
                lost = terminal.lost();
            }
        } else {
            let logs = session.logs();
            let text: String = logs.iter().rev().take(LOG_ENTRIES_READ).filter_map(|l| l.text.as_deref()).collect::<Vec<_>>().join("\n");
            offered = mighty_core::ssh::dev_server_ports(&text);
        }
        let mut ports: Vec<PaneSshPort> = offered.iter().map(|&remote| PaneSshPort { remote, local: None }).collect();
        for forward in open.iter().filter(|f| f.pane == pane.id) {
            match ports.iter_mut().find(|p| p.remote == forward.remote) {
                Some(port) => port.local = Some(forward.local),
                None => ports.push(PaneSshPort { remote: forward.remote, local: Some(forward.local) }),
            }
        }
        pane.ssh = Some(PaneSsh { host: reference.host.clone(), dir: reference.dir.clone(), ports, lost });
    }
}

pub fn register(methods: &mut Registry) {
    methods.add("sshForward", |host: Arc<Host>, args: Value| async move {
        let pane = crate::methods::text(&args, "paneId").unwrap_or("").to_string();
        let port = crate::methods::number(&args, "port").filter(|p| (1.0..=65535.0).contains(p)).map(|p| p as u16).unwrap_or(0);
        match open(&host, &pane, port).await {
            Ok(local) => Ok(json!({"local": local})),
            Err(message) => {
                host.view().error = Some(message.clone());
                host.shell_dirty();
                Ok(json!({"error": message}))
            }
        }
    });
    methods.add("sshForwardClose", |host: Arc<Host>, args: Value| async move {
        let pane = crate::methods::text(&args, "paneId").unwrap_or("").to_string();
        let port = crate::methods::number(&args, "port").filter(|p| (1.0..=65535.0).contains(p)).map(|p| p as u16);
        close(&host, &pane, port).await;
        Ok(Value::Null)
    });
}
