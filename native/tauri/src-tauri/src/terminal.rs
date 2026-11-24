//! Terminal panes, the agents' terminals, terminal prompt cards and a transcript's shell block runs (W4-A; C# MainWindow.Terminal.cs,
//! the terminal half of MainWindow.WebShell.Panes.cs, MainWindow.TerminalPrompts.cs, MainWindow.ShellBlockRuns.cs and the
//! terminal half of MainWindow.AgentIO.cs), over mighty-terminal.
//!
//! - Methods: `terminalOpen` (the footer; from then on a `footer` push whenever it changes), `terminalMessage` (the page's xterm:
//!   ready, input, resize, title, directory, copy, paste, pressed), `terminalRestart`, `terminalHistory`, `terminalPromptAnswer`,
//!   `terminalPromptShow`, `shellBlockRun`, `shellBlockStop`.
//! - Pushes: `{kind:'terminal', paneId, message}` — output (each 32 ms, base64 UTF-8), paste, reset, font, theme, footer.
//! - A shell pane's status follows its shell (`RunService::update_pane`, C# `SetShellStatus`: idle, running — read every 500 ms —
//!   completed, error), as a delta like any other change.
//! - A terminal goes with its pane (looked at every quarter second) and every terminal closes when the app quits, each close on its
//!   own thread and the quit waiting at most a few seconds for them.
//!
//! A command the app types into a new shell pane (C# `RunTerminalCommandAsync`) is styles.rs's `run_terminal_command` over
//! frame.rs's `add_pane` and [`TerminalPane::write`].

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, Weak};
use std::time::Duration;

use mighty_composer::cards::TerminalPromptInput;
use mighty_core::add_pane::{save_layout, save_mode};
use mighty_core::layout;
use mighty_core::model::{AppSnapshot, PaneLayoutNode, RunSession};
use mighty_core::shell::effective_layout;
use mighty_core::wire;
use mighty_platform::{Os, Platform};
use mighty_run::service::PaneSave;
use mighty_terminal::history::terminal_history;
use mighty_terminal::pane::{ACTIVITY_INTERVAL, FLUSH_INTERVAL};
use mighty_terminal::prompt::{TerminalPrompt, TerminalPromptAnswer, TerminalPromptMonitor};
use mighty_terminal::shell_blocks::{self, ShellBlockOutput};
use mighty_terminal::{
    AgentTerminal, PaneEvents, PaneKind, PaneOptions, Ports, PromptTerminal, TerminalFont, TerminalMessage, TerminalPane,
};
use serde_json::Value;
#[cfg(feature = "dev-harness")]
use serde_json::json;
use tauri::Manager;

use crate::frame::{activate_pane, select_session};
use crate::host::{Host, Registry};
use crate::methods::text;

/// C# `ShowTerminalHistory` (MainWindow.WorkflowParity.cs:10-23, dialog D14): the pane's saved entries in the panel `terminalHistory`
/// (its help dimmed over the entries, 11pt code, selectable, at most 360 high), closed with its ✕; the window holds its dialog
/// meanwhile, and nothing opens while another sheet is up.
pub async fn show_history(host: &Arc<Host>, pane: &RunSession) {
    let Some(_sheet) = crate::frame::Sheet::begin(host) else { return };
    crate::native::show_dialog(host, history_dialog(pane, &host.locale)).await;
}

/// D14's dialog for `pane` (no I/O): what [`show_history`] shows.
pub fn history_dialog(pane: &RunSession, locale: &mighty_core::locale::Locale) -> mighty_native::surface::DialogSpec {
    use mighty_native::surface::{DialogChrome, DialogSpec};
    use mighty_native::ui::{Content, El, Kind, TextBlock, Wrapping};
    let history = terminal_history(pane, locale);
    let (help, words) = (history.help.clone(), history.text.clone());
    let mut spec = DialogSpec::new(history.title.clone(), move |ui| {
        let mut help = ui.text_block(help.clone(), 12.0).opacity(0.7);
        if let Some(text) = help.text_mut() {
            text.wrapping = Wrapping::Wrap;
        }
        let entries = El::new(Kind::TextBlock(TextBlock {
            text: words.clone(),
            size: Some(11.0),
            weight: 400,
            family: Some(ui.code_family()),
            wrapping: Wrapping::Wrap,
            selectable: true,
            ..TextBlock::default()
        }));
        let scroll = El::new(Kind::ScrollViewer { content: entries.into() }).max_height(360.0);
        Content::El(Box::new(El::stack(false, mighty_core::design_tokens::metrics::SPACING_MD, vec![help, scroll]).width(580.0)))
    })
    .styled(None, None);
    spec.close = Some(history.close.clone());
    spec.chrome = Some(DialogChrome { panel: Some(history.panel.into()), icon: Some(history.icon.into()), ..DialogChrome::default() });
    spec
}

/// How often an agent terminal is looked at for a prompt, and how many looks with no command running end the watch.
const PROMPT_INTERVAL: Duration = Duration::from_millis(400);
const PROMPT_IDLE_POLLS: u32 = 8;
/// A running shell block's record takes its output at most this often.
const BLOCK_FLUSH: Duration = Duration::from_millis(200);
/// How long the quit waits for the terminals to close (their processes die with the app's job handles anyway).
const QUIT_WAIT: Duration = Duration::from_secs(5);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Default)]
struct Prompts {
    monitors: HashMap<String, Arc<TerminalPromptMonitor>>,
    /// The card each agent pane shows now (C# `terminalPrompts`).
    current: HashMap<String, TerminalPrompt>,
    /// The prompt each agent pane last had answered: a poll that found it before the answer never brings its card back.
    answered: HashMap<String, String>,
    watching: bool,
}

impl Prompts {
    /// A poll's findings (read with the lock let go) made the cards: what was answered meanwhile is left out; the panes whose card
    /// changed.
    fn commit(&mut self, mut found: HashMap<String, TerminalPrompt>, agents: &HashSet<String>) -> Vec<String> {
        self.monitors.retain(|id, _| agents.contains(id));
        self.answered.retain(|id, _| agents.contains(id));
        found.retain(|pane, prompt| self.answered.get(pane) != Some(&prompt.id));
        let mut ids: HashSet<String> = found.keys().cloned().collect();
        ids.extend(self.current.keys().cloned());
        let changed: Vec<String> =
            ids.into_iter().filter(|id| found.get(id).map(|p| &p.id) != self.current.get(id).map(|p| &p.id)).collect();
        if !changed.is_empty() {
            self.current = found;
        }
        changed
    }

    /// The card `prompt_id` of `pane` taken for its answer, at most once: it goes from the pane and is marked answered.
    fn take_answer(&mut self, pane: &str, prompt_id: &str) -> Option<TerminalPrompt> {
        let prompt = self.current.get(pane).filter(|p| p.id == prompt_id)?.clone();
        self.current.remove(pane);
        self.answered.insert(pane.to_string(), prompt.id.clone());
        if let Some(monitor) = self.monitors.get(pane) {
            monitor.answered(&prompt);
        }
        Some(prompt)
    }
}

/// A transcript's shell block running in its agent pane's terminal (C# `ShellBlockRunning`).
struct BlockRun {
    pane: String,
    key: String,
    terminal: Arc<AgentTerminal>,
    output: Mutex<ShellBlockOutput>,
    handle: Mutex<Option<String>>,
    stopping: AtomicBool,
    flush_queued: AtomicBool,
}

type Agents = Arc<Mutex<HashMap<String, Arc<AgentTerminal>>>>;

#[derive(Default)]
struct WakeState {
    panes: usize,
    agents: usize,
    closing: bool,
}

/// Parks `terminal-clock` and `shell-activity` whenever no terminals exist, waking immediately on creation or shutdown.
/// Lock ordering: `panes`/`agents` -> `wake.state` (waiters never acquire `panes`/`agents` while holding `wake.state`).
struct TerminalWake {
    state: Mutex<WakeState>,
    signal: Condvar,
}

impl TerminalWake {
    fn new() -> Arc<TerminalWake> {
        Arc::new(TerminalWake { state: Mutex::new(WakeState::default()), signal: Condvar::new() })
    }

    fn set_panes(&self, count: usize) {
        let mut state = lock(&self.state);
        let grew = count > state.panes;
        state.panes = count;
        if grew {
            self.signal.notify_all();
        }
    }

    fn set_agents(&self, count: usize) {
        let mut state = lock(&self.state);
        let grew = count > state.agents;
        state.agents = count;
        if grew {
            self.signal.notify_all();
        }
    }

    fn close(&self) {
        let mut state = lock(&self.state);
        state.closing = true;
        state.panes = 0;
        state.agents = 0;
        self.signal.notify_all();
    }

    /// Parks until at least one pane (or, if `include_agents`, an agent terminal) exists, returning `None` once closing.
    /// Returns `Some(has_panes)` when awake.
    fn wait_active(&self, include_agents: bool) -> Option<bool> {
        let mut state = lock(&self.state);
        while !state.closing && state.panes == 0 && (!include_agents || state.agents == 0) {
            state = self.signal.wait(state).unwrap_or_else(|p| p.into_inner());
        }
        (!state.closing).then_some(state.panes > 0)
    }

    /// Sleeps up to `duration`, waking immediately when closing or (if `wake_on_pane`) when a pane opens.
    fn sleep_or_close(&self, duration: Duration, wake_on_pane: bool) -> bool {
        let guard = lock(&self.state);
        if guard.closing {
            return false;
        }
        let (guard, _) = self
            .signal
            .wait_timeout_while(guard, duration, |s| !s.closing && !(wake_on_pane && s.panes > 0))
            .unwrap_or_else(|p| p.into_inner());
        !guard.closing
    }
}

/// Every terminal of the window (C# `views`' terminal halves, `agentTerminals`, `terminalPrompts`, `shellBlockRunning`).
pub struct Terminals {
    host: Weak<Host>,
    ports: Ports,
    events: Arc<HostEvents>,
    panes: Mutex<HashMap<String, Arc<TerminalPane>>>,
    agents: Agents,
    prompts: Mutex<Prompts>,
    blocks: Mutex<HashMap<String, Arc<BlockRun>>>,
    /// The smoke's own terminal panes: in a smoke no other pane starts a shell (C# `smokeTerminalEnabled`).
    smoke_allowed: Mutex<HashSet<String>>,
    /// Closes started when a pane or an agent pane went (each on its own task): the quit waits for them too.
    closes: Mutex<Vec<tauri::async_runtime::JoinHandle<()>>>,
    wake: Arc<TerminalWake>,
    last_reconcile_rev: AtomicU64,
    closing: AtomicBool,
}

impl Drop for Terminals {
    fn drop(&mut self) {
        self.wake.close();
    }
}

/// What a pane asks of the window, answered from the host.
struct HostEvents {
    host: Weak<Host>,
    agents: Agents,
}

impl PaneEvents for HostEvents {
    fn post(&self, pane: &str, message: &TerminalMessage) {
        // MC-9: the output kept as plain lines for `mighty pane read` and `pane wait-output`, whoever draws the terminal.
        if let TerminalMessage::Output { data } = message {
            crate::control::terminal_output(pane, data);
        }
        // The Mac feed (M-D): a pane drawn by a native view (Ghostty) gets its output, reset, theme and font there; Windows never
        // opens one, so everything goes to the page's xterm as before.
        if crate::surface_terminal::post(pane, message) {
            return;
        }
        let Some(host) = self.host.upgrade() else { return };
        let json = format!(
            "{{\"kind\":\"terminal\",\"paneId\":{},\"message\":{}}}",
            serde_json::to_string(pane).unwrap_or_default(),
            serde_json::to_string(message).unwrap_or_default()
        );
        let view = host.view();
        host.push(&view, "terminal", json);
    }

    fn status(&self, pane: &str, status: &'static str) {
        if let Some(host) = self.host.upgrade() {
            change_shell_status(&host, pane, |session| mighty_terminal::activity::shell_status_change(session, status));
        }
    }

    fn activity(&self, pane: &str, busy: bool) {
        if let Some(host) = self.host.upgrade() {
            change_shell_status(&host, pane, |session| mighty_terminal::activity::activity_change(session, busy));
        }
    }

    fn error(&self, text: String) {
        if let Some(host) = self.host.upgrade() {
            host.view().error = Some(text);
            host.schedule();
        }
    }

    fn pressed(&self, pane: &str) {
        if let Some(host) = self.host.upgrade() {
            let _ = activate_pane(&mut host.view(), pane);
            host.schedule();
        }
    }

    /// MC-26: the terminal's notification or bell marks the pane (and may notify) unless it is the one being looked at.
    fn signal(&self, pane: &str, signal: mighty_terminal::signals::Signal) {
        if let Some(host) = self.host.upgrade() {
            crate::attention::signal(&host, pane, signal);
        }
    }

    fn ssh_changed(&self, _pane: &str) {
        if let Some(host) = self.host.upgrade() {
            host.shell_dirty();
        }
    }

    fn light(&self) -> bool {
        self.host.upgrade().is_some_and(|host| host.view().snapshot.theme() == "light")
    }

    /// The text setting (frame.rs, C# `AppText.Current`): its terminal size and code font.
    fn font(&self) -> TerminalFont {
        let Some(host) = self.host.upgrade() else { return TerminalFont::default() };
        let text = crate::frame::text_setting(&host);
        TerminalFont { size: text.terminal_size(), family: text.code_family }
    }

    fn agent_terminal(&self, owner: &str) -> Option<Arc<AgentTerminal>> {
        lock(&self.agents).get(owner).cloned()
    }
}

/// C# `SetShellStatus` through the run service (a light update of the pane, `change` deciding from the pane as the update finds it),
/// then the page's copy taken again.
fn change_shell_status(host: &Arc<Host>, pane: &str, change: impl FnOnce(&RunSession) -> Option<(RunSession, bool)>) {
    let Some(runs) = host.runs() else { return };
    let mut status = None;
    let changed = runs.service.update_pane(pane, |session| {
        change(session).map(|(next, now)| {
            status = (next.status() != session.status()).then(|| next.status().to_string());
            (next, if now { PaneSave::Now } else { PaneSave::Later })
        })
    });
    if changed {
        host.view().sync();
        host.schedule();
        // MC-9: a shell's run beginning or ending is a state move scripts hear.
        crate::control::touch(host, pane);
        // C# `ShellStatusChanged` (MainWindow.Terminal.cs:62): the push hub hears a shell's run begin (`Began`) or move (`Changed`), and
        // the phone router that the pane moved; a shell's status never comes through the run service's events.
        crate::mobile::noted(host, Some(shell_push_note(pane, status.as_deref())));
        crate::phone::changed(host);
    }
}

/// What a shell's status change tells the push hub: a run that began, or something that moved.
fn shell_push_note(pane: &str, status: Option<&str>) -> crate::mobile::PushNote {
    match status {
        Some("running") => crate::mobile::PushNote::Began(pane.to_string()),
        _ => crate::mobile::PushNote::Changed,
    }
}

/// C# `DesktopService.UpdateShellBlockRun` through the run service.
fn update_block_record(
    host: &Arc<Host>,
    pane: &str,
    key: &str,
    update: impl FnOnce(Option<&mighty_core::model::ShellBlockRun>) -> Option<mighty_core::model::ShellBlockRun>,
) {
    let Some(runs) = host.runs() else { return };
    let changed = runs.service.update_pane(pane, |session| {
        shell_blocks::update_pane(session, key, update).map(|(next, ended)| (next, if ended { PaneSave::Now } else { PaneSave::Soon }))
    });
    if changed {
        host.view().sync();
        host.schedule();
    }
}

/// `AgentIOPaneKind.Terminal` (C#'s `Browser`, `agent-browser`, is gone: 0.5 opens an agent's page in the default browser).
pub(crate) const AGENT_TERMINAL: &str = "agent-terminal";

/// C# `WithAgentIOPane`: the state with the agent pane's terminal pane to its right, titled as on the Mac; unchanged
/// while that pane is already open.
fn with_agent_io_pane(
    state: &mut AppSnapshot,
    defaults: &mut HashMap<String, PaneLayoutNode>,
    agent: &str,
    workspace: &str,
    kind: &str,
    locale: mighty_core::locale::Locale,
) -> Result<(), String> {
    let pane_id = format!("{kind}:{agent}");
    let Some(parent) = state.sessions().iter().find(|p| p.id() == agent && p.workspace_id() == workspace).cloned() else {
        return Err("Agent pane closed.".into());
    };
    if state.session(&pane_id).is_some() {
        return Ok(());
    }
    // The remote mirror's panes are the host's and never count (`PaneLayout.HasRoom`).
    if !mighty_core::add_pane::has_room(state) {
        return Err("Close a pane before opening another.".into());
    }
    let before = effective_layout(state, workspace, defaults);
    let target = before
        .as_ref()
        .and_then(|tree| layout::groups(tree).into_iter().find(|g| g.session_ids().any(|id| id == agent)).map(|g| g.id().to_string()));
    let title = "agentTerminal.terminalPane.title";
    let pane = RunSession {
        id: Some(pane_id.clone()),
        kind: Some(kind.into()),
        owner_session_id: Some(agent.into()),
        workspace_id: Some(workspace.into()),
        workspace_profile_key: Some(workspace.into()),
        provider: parent.provider.clone(),
        title: Some(locale.get(title)),
        ..RunSession::default()
    };
    state.sessions_mut().push(Arc::new(pane));
    let mut tree = effective_layout(state, workspace, defaults);
    if let (Some(node), Some(target)) = (&tree, &target) {
        tree = Some(layout::move_tab(node, &pane_id, target, "right", None));
    }
    save_layout(state, workspace, tree);
    save_mode(state, workspace, "custom");
    Ok(())
}

/// [`with_agent_io_pane`] as a change of the state.
pub(crate) fn show_agent_io_pane(host: &Host, agent: &str, workspace: &str, kind: &str) -> Result<(), String> {
    let mut view = host.view();
    let mut defaults = view.layout_defaults.clone();
    let mut failure = None;
    view.update(|state| {
        if let Err(error) = with_agent_io_pane(state, &mut defaults, agent, workspace, kind, host.locale) {
            failure = Some(error);
        }
    })?;
    view.layout_defaults = defaults;
    failure.map_or(Ok(()), Err)
}

/// C# `LocalPath(workspace)`: the workspace's folder on this computer, None for a remote one.
fn local_folder(state: &AppSnapshot, workspace: &str) -> Option<PathBuf> {
    let workspace = state.workspace(workspace)?;
    (workspace.remote.is_none() && !workspace.path().is_empty()).then(|| PathBuf::from(workspace.path()))
}

/// MC-33: an SSH workspace's terminal: its stand-in folder and the ssh that runs the far side's shell; no terminal (the pane says
/// why) when the machine cannot be reached from here (gone, no local ssh).
fn ssh_shell(
    host: &Host,
    workspace: &mighty_core::model::Workspace,
    pane: &str,
    folder: Option<&str>,
) -> (Option<PathBuf>, Option<mighty_terminal::RemoteShell>) {
    let Ok(mut launch) = crate::ssh::launch_for(host, workspace) else { return (None, None) };
    // MC-47: a terminal opened "in this worktree" of an SSH pane starts in that worktree on the machine.
    if let Some(folder) = folder.filter(|f| mighty_core::ssh::valid_remote_dir(f)) {
        launch.dir = folder.to_string();
    }
    let shell = crate::ssh::shell_for(host, workspace, pane);
    let request = launch.pty_request(&shell, Default::default(), Default::default());
    let host_name = workspace.ssh.as_ref().map(|r| r.host.clone()).unwrap_or_default();
    let remote = mighty_terminal::RemoteShell {
        program: request.program,
        args: request.args,
        title: format!("ssh · {host_name}"),
        directory: format!("{host_name}:{}", launch.dir),
    };
    (Some(launch.local), Some(remote))
}

/// C# `DelegationPanes.IsWorkingFolder`: a delegated child's worktree, a full path of at most 4,096 characters.
fn is_working_folder(path: &str) -> bool {
    wire::utf16_len(path) <= 4096 && !path.contains('\0') && Path::new(path).is_absolute()
}

impl Terminals {
    /// The window's terminals, started with the host: managed by the app, the 32 ms output clock (and a pane's teardown when it
    /// leaves the state) and the 500 ms shell-activity reading on threads of their own.
    pub fn start(host: &Arc<Host>, platform: &Platform) -> Arc<Terminals> {
        let agents: Agents = Arc::default();
        let events = Arc::new(HostEvents { host: Arc::downgrade(host), agents: agents.clone() });
        let ports = Ports {
            ptys: platform.ptys.clone(),
            environment: platform.environment.clone(),
            clipboard: crate::platform::clipboard(),
            locale: host.locale,
            engine: match platform.profile.os {
                Os::Windows => "ConPTY".into(),
                Os::Macos => "Ghostty".into(),
            },
        };
        let wake = TerminalWake::new();
        let terminals = Arc::new(Terminals {
            host: Arc::downgrade(host),
            ports,
            events,
            panes: Mutex::default(),
            agents,
            prompts: Mutex::default(),
            blocks: Mutex::default(),
            smoke_allowed: Mutex::default(),
            closes: Mutex::default(),
            wake: wake.clone(),
            last_reconcile_rev: AtomicU64::new(u64::MAX),
            closing: AtomicBool::new(false),
        });
        host.app.manage(terminals.clone());
        host.log.line(&format!("terminals: {} engine", terminals.ports.engine));
        // A native terminal view's events are its pane's `terminalMessage`s, as the page's xterm sends them (M-D).
        let to_pane = Arc::downgrade(&terminals);
        crate::surface_terminal::natives().connect(Arc::new(move |pane: String, message: Value| {
            let terminals = to_pane.clone();
            Box::pin(async move {
                let Some(terminals) = terminals.upgrade() else { return };
                let Some(host) = terminals.host.upgrade() else { return };
                if let Ok(pane) = terminals.pane(&host, Some(&pane)) {
                    pane.handle_message(&message).await;
                }
            })
        }));
        let clock = Arc::downgrade(&terminals);
        let clock_wake = wake.clone();
        std::thread::Builder::new()
            .name("terminal-clock".into())
            .spawn(move || {
                let mut ticks = 0u64;
                while let Some(has_panes) = clock_wake.wait_active(true) {
                    let Some(terminals) = clock.upgrade() else { return };
                    if terminals.closing.load(Ordering::SeqCst) {
                        return;
                    }
                    if has_panes {
                        for pane in terminals.all() {
                            pane.tick();
                        }
                        ticks += 1;
                        if ticks.is_multiple_of(8) {
                            terminals.reconcile();
                        }
                        drop(terminals);
                        if !clock_wake.sleep_or_close(FLUSH_INTERVAL, false) {
                            return;
                        }
                    } else {
                        terminals.reconcile();
                        drop(terminals);
                        if !clock_wake.sleep_or_close(FLUSH_INTERVAL * 8, true) {
                            return;
                        }
                    }
                }
            })
            .expect("the terminal clock");
        let reading = Arc::downgrade(&terminals);
        let reading_wake = wake;
        std::thread::Builder::new()
            .name("shell-activity".into())
            .spawn(move || {
                while reading_wake.wait_active(false).is_some() {
                    let Some(terminals) = reading.upgrade() else { return };
                    if terminals.closing.load(Ordering::SeqCst) {
                        return;
                    }
                    for pane in terminals.all() {
                        pane.read_activity();
                    }
                    drop(terminals);
                    if !reading_wake.sleep_or_close(ACTIVITY_INTERVAL, false) {
                        return;
                    }
                }
            })
            .expect("the shell-activity reading");
        terminals
    }

    fn all(&self) -> Vec<Arc<TerminalPane>> {
        lock(&self.panes).values().cloned().collect()
    }

    /// A close run on its own task, kept for the quit to wait on (the ones already ended let go).
    fn close_later(&self, close: impl std::future::Future<Output = ()> + Send + 'static) {
        let handle = tauri::async_runtime::spawn(close);
        let mut closes = lock(&self.closes);
        closes.retain(|close| !close.inner().is_finished());
        closes.push(handle);
    }

    /// C# `RefreshTerminalTheme` for every pane (`Retheme`, `TextAppearanceChanged`): the theme or the text setting changed.
    pub fn restyle(&self) {
        for pane in self.all() {
            pane.restyle();
        }
    }

    /// The terminal of a shell or agent-terminal pane, made the first time (C# `WebTerminalPane`); "Session not found." for any
    /// other id.
    pub fn pane(&self, host: &Host, id: Option<&str>) -> Result<Arc<TerminalPane>, String> {
        let not_found = || "Session not found.".to_string();
        let id = id.ok_or_else(not_found)?;
        let (kind, folder, remote) = {
            let view = host.view();
            let session = view.snapshot.session(id).filter(|s| matches!(s.kind(), "shell" | AGENT_TERMINAL)).ok_or_else(not_found)?;
            let kind =
                if session.kind() == "shell" { PaneKind::Shell } else { PaneKind::Agent { owner: session.owner_session_id.clone() } };
            // MC-33: an SSH workspace's terminal is the local ssh running the far side's shell (or tmux) in the remote folder.
            if kind == PaneKind::Shell
                && let Some(workspace) = view.snapshot.workspace(session.workspace_id()).filter(|w| w.is_ssh()).cloned()
            {
                let working = session.working_folder.clone();
                drop(view);
                let (folder, remote) = ssh_shell(host, &workspace, id, working.as_deref());
                (kind, folder, remote)
            } else {
                // A terminal opened "in this worktree" starts in the pane's folder (W9-I, C# PaneWorktree.cs); one whose folder is gone
                // starts in the workspace.
                let folder = match session.working_folder.as_deref() {
                    Some(path) if kind == PaneKind::Shell && is_working_folder(path) && Path::new(path).is_dir() => {
                        Some(PathBuf::from(path))
                    }
                    _ => local_folder(&view.snapshot, session.workspace_id()),
                };
                (kind, folder, None)
            }
        };
        let mut panes = lock(&self.panes);
        if let Some(pane) = panes.get(id).filter(|pane| *pane.kind() == kind) {
            return Ok(pane.clone());
        }
        let enabled = host.smoke.is_none() || lock(&self.smoke_allowed).contains(id);
        let options = PaneOptions { id: id.to_string(), kind, folder, enabled, no_profile: host.smoke.is_some(), remote };
        let pane = TerminalPane::new(options, self.ports.clone(), self.events.clone());
        if let Some(earlier) = panes.insert(id.to_string(), pane.clone()) {
            self.close_later(async move { earlier.dispose().await });
        }
        self.wake.set_panes(panes.len());
        Ok(pane)
    }

    /// MC-33: a shell pane's terminal when it was made (never makes one).
    pub fn existing(&self, id: &str) -> Option<Arc<TerminalPane>> {
        lock(&self.panes).get(id).cloned()
    }

    /// An agent pane's terminal, made the first time in `directory` (C# `agentTerminals`; Agent IO's `run_in_terminal` makes it too,
    /// W7).
    pub fn agent(&self, pane: &str, directory: &Path) -> Arc<AgentTerminal> {
        let (terminal, inserted) = {
            let mut agents = lock(&self.agents);
            let before = agents.len();
            let terminal = agents
                .entry(pane.to_string())
                .or_insert_with(|| AgentTerminal::new(directory.to_path_buf(), self.ports.ptys.clone(), self.ports.environment.clone()))
                .clone();
            let after = agents.len();
            self.wake.set_agents(after);
            (terminal, after != before)
        };
        if inserted && let Some(host) = self.host.upgrade() {
            host.shell_dirty();
        }
        terminal
    }

    /// The agent pane's terminal, if it has one (C# `agentTerminals.TryGetValue`).
    pub fn existing_agent(&self, pane: &str) -> Option<Arc<AgentTerminal>> {
        lock(&self.agents).get(pane).cloned()
    }

    /// C# `EndAgentTerminal`'s terminal half: the agent pane's terminal and everything it started end, and this returns once they
    /// have (a delegated child's discard and close cleanup look for links in its worktree only then).
    pub async fn end_agent(&self, pane: &str) {
        let terminal = {
            let mut agents = lock(&self.agents);
            let removed = agents.remove(pane);
            self.wake.set_agents(agents.len());
            removed
        };
        if let Some(terminal) = terminal {
            if let Some(host) = self.host.upgrade() {
                host.shell_dirty();
            }
            let _ = tokio::time::timeout(QUIT_WAIT, terminal.dispose()).await;
        }
    }

    /// A pane's terminal goes once the pane leaves the state, an agent's terminal once its agent pane does (C# `CloseTerminal`,
    /// `ReconcileAgentIO`); each close on its own task.
    fn reconcile(&self) {
        let Some(host) = self.host.upgrade() else { return };
        let rev = host.revision();
        if self.last_reconcile_rev.load(Ordering::Acquire) == rev {
            return;
        }
        let (rev, terminal_panes, agent_panes): (u64, HashSet<String>, HashSet<String>) = {
            let view = host.view();
            let sessions = view.snapshot.sessions();
            (
                view.revision(),
                sessions.iter().filter(|s| matches!(s.kind(), "shell" | AGENT_TERMINAL)).map(|s| s.id().to_string()).collect(),
                sessions.iter().filter(|s| s.kind() == "claude").map(|s| s.id().to_string()).collect(),
            )
        };
        self.last_reconcile_rev.store(rev, Ordering::Release);
        let gone: Vec<Arc<TerminalPane>> = {
            let mut panes = lock(&self.panes);
            let ids: Vec<String> = panes.keys().filter(|id| !terminal_panes.contains(*id)).cloned().collect();
            let removed = ids.iter().filter_map(|id| panes.remove(id)).collect();
            self.wake.set_panes(panes.len());
            removed
        };
        for pane in gone {
            crate::surface_terminal::natives().forget(pane.id());
            self.close_later(async move { pane.dispose().await });
        }
        let ended: Vec<Arc<AgentTerminal>> = {
            let mut agents = lock(&self.agents);
            let ids: Vec<String> = agents.keys().filter(|id| !agent_panes.contains(*id)).cloned().collect();
            let removed = ids.iter().filter_map(|id| agents.remove(id)).collect();
            self.wake.set_agents(agents.len());
            removed
        };
        if !ended.is_empty() {
            host.shell_dirty();
        }
        for agent in ended {
            self.close_later(async move { agent.dispose().await });
        }
    }

    /// Every terminal closed (the app quits): at once, all together, the closes already under way with them, the wait bounded.
    pub async fn close_all(&self) {
        self.closing.store(true, Ordering::SeqCst);
        self.wake.close();
        let panes: Vec<Arc<TerminalPane>> = lock(&self.panes).drain().map(|(_, pane)| pane).collect();
        let agents: Vec<Arc<AgentTerminal>> = lock(&self.agents).drain().map(|(_, agent)| agent).collect();
        let mut closes: Vec<_> = lock(&self.closes).drain(..).collect();
        for pane in panes {
            closes.push(tauri::async_runtime::spawn(async move { pane.dispose().await }));
        }
        for agent in agents {
            closes.push(tauri::async_runtime::spawn(async move { agent.dispose().await }));
        }
        let _ = tokio::time::timeout(QUIT_WAIT, async {
            for close in closes {
                let _ = close.await;
            }
        })
        .await;
    }

    // ── terminal prompts (C# MainWindow.TerminalPrompts.cs) ──────────────────────────────────────────────────────────────────

    /// Starts following the agent terminals for prompts, unless it already does; it ends by itself once no agent terminal runs a
    /// command for eight looks.
    pub(crate) fn watch_prompts(self: &Arc<Self>) {
        {
            let mut prompts = lock(&self.prompts);
            if prompts.watching || self.closing.load(Ordering::SeqCst) {
                return;
            }
            prompts.watching = true;
        }
        let terminals = self.clone();
        tauri::async_runtime::spawn(async move {
            let mut idle = 0;
            while idle < PROMPT_IDLE_POLLS {
                tokio::time::sleep(PROMPT_INTERVAL).await;
                if terminals.closing.load(Ordering::SeqCst) {
                    break;
                }
                idle = if terminals.poll_prompts() { 0 } else { idle + 1 };
            }
            lock(&terminals.prompts).watching = false;
        });
    }

    /// One look at every agent terminal; whether any command still runs.
    fn poll_prompts(&self) -> bool {
        let agents: Vec<(String, Arc<AgentTerminal>)> = lock(&self.agents).iter().map(|(id, t)| (id.clone(), t.clone())).collect();
        let mut running = false;
        let mut found = HashMap::new();
        for (pane, terminal) in &agents {
            if !terminal.running_handles().is_empty() {
                running = true;
            }
            let monitor = lock(&self.prompts).monitors.entry(pane.clone()).or_default().clone();
            if let Some(prompt) = monitor.poll(terminal.as_ref() as &dyn PromptTerminal) {
                found.insert(pane.clone(), prompt);
            }
        }
        let panes: HashSet<String> = agents.iter().map(|(pane, _)| pane.clone()).collect();
        let changed = lock(&self.prompts).commit(found, &panes);
        for id in &changed {
            self.prompt_changed(id);
        }
        running
    }

    /// The page redraws the pane's cards.
    fn prompt_changed(&self, pane: &str) {
        let Some(host) = self.host.upgrade() else { return };
        if let Some(ui) = host.view().panes.get_mut(pane) {
            ui.dirty = true;
        }
        // A card that comes, changes or goes moves the pane on the phone (C# `mobileRouter?.Changed()`), outside the view's lock.
        crate::phone::changed(&host);
    }

    /// The card of the command waiting for input in `pane`'s terminal (C# `terminalPrompts`), as the phone's detail carries it.
    pub fn prompt(&self, pane: &str) -> Option<TerminalPrompt> {
        lock(&self.prompts).current.get(pane).cloned()
    }

    /// C# `AnswerTerminalPrompt`: the answer goes to the waiting command, and the card goes until it asks again. A card is taken for
    /// its answer once (a second answer of it, or a poll that found it before, never types again). Whether it was typed.
    pub fn answer_prompt(&self, pane: &str, prompt_id: &str, answer: &TerminalPromptAnswer) -> bool {
        let Some(terminal) = lock(&self.agents).get(pane).cloned() else { return false };
        let Some(prompt) = lock(&self.prompts).take_answer(pane, prompt_id) else { return false };
        self.prompt_changed(pane);
        terminal.write_to(&prompt.handle, &answer.input())
    }

    /// The card a pane shows now, as the cards model takes it.
    pub fn prompt_input(&self, pane: &str) -> Option<TerminalPromptInput> {
        lock(&self.prompts).current.get(pane).map(|prompt| TerminalPromptInput {
            id: prompt.id.clone(),
            text: prompt.text.clone(),
            options: prompt.options.clone(),
            kind: prompt.kind.to_string(),
            default_yes: prompt.default_yes,
        })
    }

    /// C# `OpenAgentTerminalPane`: the agent's terminal pane put back beside it when it was closed, and selected. The terminal kept
    /// running, so the pane shows its output again.
    fn open_agent_terminal_pane(&self, host: &Host, agent: &str) -> Result<(), String> {
        let workspace = {
            let view = host.view();
            match view.snapshot.session(agent) {
                Some(pane) if lock(&self.agents).contains_key(agent) => pane.workspace_id().to_string(),
                _ => return Ok(()),
            }
        };
        show_agent_io_pane(host, agent, &workspace, AGENT_TERMINAL)?;
        select_session(&mut host.view(), &format!("{AGENT_TERMINAL}:{agent}"))
    }

    // ── shell block runs (C# MainWindow.ShellBlockRuns.cs) ───────────────────────────────────────────────────────────────────

    /// `shellBlockRun{paneId, key, command}`: the command, which must be a line-for-line copy of what that assistant entry shows,
    /// runs in the agent pane's own terminal in the workspace's folder (a delegated child's worktree), and its terminal pane opens
    /// beside the agent. One block runs at a time per pane. The run is the user's own click: it does not pass the agent's tool
    /// permissions.
    async fn run_shell_block(
        self: &Arc<Self>,
        host: &Arc<Host>,
        pane: Option<&str>,
        key: Option<&str>,
        command: Option<&str>,
    ) -> Result<(), String> {
        let command = command.map(|c| c.trim_end_matches(['\r', '\n']));
        let (Some(pane_id), Some(key), Some(command)) = (pane, key, command) else { return Ok(()) };
        if self.closing.load(Ordering::SeqCst)
            || !wire::identifier(pane_id)
            || !shell_blocks::is_key(Some(key))
            || !shell_blocks::is_command(Some(command))
        {
            return Ok(());
        }
        let (workspace, directory) = {
            let view = host.view();
            let Some(pane) = view.snapshot.session(pane_id).filter(|p| p.kind() == "claude") else { return Ok(()) };
            // No pane tools for a remote workspace: its folder is on another computer.
            let Some(folder) = local_folder(&view.snapshot, pane.workspace_id()) else { return Ok(()) };
            if !shell_blocks::shown(pane, key, command) || lock(&self.blocks).contains_key(pane_id) {
                return Ok(());
            }
            // A delegated child works in its worktree, as its runs and its run_in_terminal do.
            let directory = match pane.working_folder.as_deref() {
                Some(path) if is_working_folder(path) && Path::new(path).is_dir() => PathBuf::from(path),
                _ => folder,
            };
            (pane.workspace_id().to_string(), directory)
        };
        let terminal = self.agent(pane_id, &directory);
        show_agent_io_pane(host, pane_id, &workspace, AGENT_TERMINAL)?;
        host.view().shows_dashboard = false;
        let running = {
            let mut blocks = lock(&self.blocks);
            // The change above let another click in: still one block at a time.
            if self.closing.load(Ordering::SeqCst) || blocks.contains_key(pane_id) {
                return Ok(());
            }
            let running = Arc::new(BlockRun {
                pane: pane_id.to_string(),
                key: key.to_string(),
                terminal: terminal.clone(),
                output: Mutex::new(ShellBlockOutput::default()),
                handle: Mutex::new(None),
                stopping: AtomicBool::new(false),
                flush_queued: AtomicBool::new(false),
            });
            blocks.insert(pane_id.to_string(), running.clone());
            running
        };
        let started = shell_blocks::started(key, command, None);
        update_block_record(host, pane_id, key, |_| Some(started));
        let listener = {
            let (running, host) = (running.clone(), Arc::downgrade(host));
            Arc::new(move |raw: &str| receive_block_output(&host, &running, raw))
        };
        let handle = match terminal.start(command, Some(listener)).await {
            Ok(handle) => handle,
            Err(error) => {
                lock(&self.blocks).remove(pane_id);
                update_block_record(host, pane_id, key, |run| {
                    run.filter(|r| r.state.as_deref() == Some("running")).map(|r| shell_blocks::finished(r, None, false, None))
                });
                return Err(error.to_string());
            }
        };
        *lock(&running.handle) = Some(handle.clone());
        self.watch_prompts();
        let (terminals, finishing, host_for_finish) = (self.clone(), running.clone(), host.clone());
        tauri::async_runtime::spawn(async move { terminals.finish_shell_block(&host_for_finish, finishing, handle).await });
        // A Stop pressed while the command was still starting is kept and done now.
        let handle = lock(&running.handle).clone();
        if running.stopping.load(Ordering::SeqCst)
            && let Some(handle) = handle
        {
            let _ = running.terminal.stop(&handle).await;
        }
        Ok(())
    }

    /// C# `FinishShellBlock`: once the command ended, the record takes its last output and its end. Quitting, the record stays
    /// running and the next launch reads it interrupted.
    async fn finish_shell_block(&self, host: &Arc<Host>, running: Arc<BlockRun>, handle: String) {
        let code = running.terminal.exited(&handle).await.unwrap_or(Some(-1));
        {
            let mut blocks = lock(&self.blocks);
            if blocks.get(&running.pane).is_some_and(|current| Arc::ptr_eq(current, &running)) {
                blocks.remove(&running.pane);
            }
        }
        if self.closing.load(Ordering::SeqCst) {
            return;
        }
        let output = lock(&running.output).text();
        let stopped = running.stopping.load(Ordering::SeqCst);
        update_block_record(host, &running.pane, &running.key, |run| {
            run.filter(|r| r.state.as_deref() == Some("running")).map(|r| {
                let with_output = mighty_core::model::ShellBlockRun { output: Some(output), ..r.clone() };
                shell_blocks::finished(&with_output, code, stopped, None)
            })
        });
    }

    /// `shellBlockStop{paneId, key}`: as `stop` stops an agent's command (Ctrl+C, then the process ended after 3 s).
    async fn stop_shell_block(&self, pane: Option<&str>, key: Option<&str>) {
        let Some(running) = pane.and_then(|pane| lock(&self.blocks).get(pane).cloned()) else { return };
        if Some(running.key.as_str()) != key {
            return;
        }
        running.stopping.store(true, Ordering::SeqCst);
        // Still starting: the run stops it once the terminal gave its handle.
        let handle = lock(&running.handle).clone();
        if let Some(handle) = handle {
            let _ = running.terminal.stop(&handle).await;
        }
    }

    // ── the smoke's terminal step ────────────────────────────────────────────────────────────────────────────────────────────

    /// The shell a new terminal runs (the smoke types in its language).
    pub fn default_shell(&self) -> PathBuf {
        self.ports.ptys.default_shell()
    }

    /// The smoke's own pane may start a shell.
    #[cfg(feature = "dev-harness")]
    pub fn smoke_allow(&self, pane: &str) {
        lock(&self.smoke_allowed).insert(pane.to_string());
    }

    /// The pane's terminal as the smoke reads it: its status in the state, its footer and the host's diagnostics; null before the
    /// pane has a terminal.
    #[cfg(feature = "dev-harness")]
    pub fn smoke_state(&self, host: &Host, pane: &str) -> Value {
        let status = host.view().snapshot.session(pane).map(|s| s.status().to_string());
        let Some(terminal) = lock(&self.panes).get(pane).cloned() else { return json!({"status": status}) };
        json!({
            "status": status, "running": terminal.is_running(), "closed": terminal.is_closed(),
            "footer": terminal.footer(), "diagnostics": terminal.diagnostics(),
        })
    }

    /// The smoke's pane closed: its terminal ends at once (as the pane's close would end it); whether it was there.
    #[cfg(feature = "dev-harness")]
    pub async fn smoke_close(&self, pane: &str) -> bool {
        let terminal = {
            let mut panes = lock(&self.panes);
            let removed = panes.remove(pane);
            self.wake.set_panes(panes.len());
            removed
        };
        crate::surface_terminal::natives().forget(pane);
        match terminal {
            Some(terminal) => {
                let _ = tokio::time::timeout(Duration::from_secs(20), terminal.dispose()).await;
                terminal.is_closed() && !terminal.is_running()
            }
            None => false,
        }
    }
}

/// C# `ReceiveShellBlockOutput`: the output is cleaned as it comes and the record takes it at most every 200 ms.
fn receive_block_output(host: &Weak<Host>, running: &Arc<BlockRun>, raw: &str) {
    lock(&running.output).append(raw);
    if running.flush_queued.swap(true, Ordering::SeqCst) {
        return;
    }
    let (host, running) = (host.clone(), running.clone());
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(BLOCK_FLUSH).await;
        running.flush_queued.store(false, Ordering::SeqCst);
        let output = lock(&running.output).text();
        if let Some(host) = host.upgrade() {
            update_block_record(&host, &running.pane, &running.key, |run| {
                run.filter(|r| r.state.as_deref() == Some("running") && r.output.as_deref() != Some(output.as_str()))
                    .map(|r| mighty_core::model::ShellBlockRun { output: Some(output.clone()), ..r.clone() })
            });
        }
    });
}

/// The window's terminals (none before the host started them).
pub fn terminals(host: &Host) -> Result<Arc<Terminals>, String> {
    host.app.try_state::<Arc<Terminals>>().map(|state| state.inner().clone()).ok_or_else(|| "the terminals are not ready".to_string())
}

/// The frame's agent panes that have a terminal (C# `agentTerminals`), shown or not: their header offers 터미널 창 열기
/// (`terminalPromptShow`, M/SessionPaneView.swift's `agent-terminal-open-` button).
pub fn mark_agent_terminals(host: &Host, model: &mut mighty_core::shell::ShellModel) {
    let Ok(terminals) = terminals(host) else { return };
    let agents = lock(&terminals.agents);
    for pane in &mut model.panes {
        pane.agent_terminal = agents.contains_key(&pane.id);
    }
}

/// The processes of `pane`'s terminal (its shell and what runs under it), for its ports (attention.rs, MC-26).
pub fn process_ids(host: &Host, pane: &str) -> Vec<u32> {
    let Ok(terminals) = terminals(host) else { return Vec::new() };
    let pane = lock(&terminals.panes).get(pane).cloned();
    pane.map(|pane| pane.process_ids()).unwrap_or_default()
}

/// The theme or the text setting changed (methods.rs's `setTheme`, frame.rs's `textSize`): every terminal takes its look again.
pub fn restyle(host: &Host) {
    if let Ok(terminals) = terminals(host) {
        terminals.restyle();
    }
}

/// The terminal prompt card of a pane, for the cards model (panes.rs).
pub fn prompt_input(host: &Host, pane: &str) -> Option<TerminalPromptInput> {
    terminals(host).ok()?.prompt_input(pane)
}

/// Every terminal closed while the app quits (main.rs), the wait bounded.
pub fn close_all(app: &tauri::AppHandle) {
    if let Some(terminals) = app.try_state::<Arc<Terminals>>() {
        let terminals = terminals.inner().clone();
        tauri::async_runtime::block_on(terminals.close_all());
    }
}

/// C# `Act`: the error line cleared, then whatever the action says went wrong shown there.
fn act(host: &Host, result: Result<(), String>) {
    if let Err(error) = result {
        host.view().error = Some(error);
    }
}

pub fn register(methods: &mut Registry) {
    methods.add("terminalOpen", |host: Arc<Host>, args| async move {
        let pane = terminals(&host)?.pane(&host, text(&args, "paneId"))?;
        Ok(serde_json::to_value(pane.open()).unwrap_or(Value::Null))
    });
    methods.add("terminalMessage", |host: Arc<Host>, args| async move {
        let pane = terminals(&host)?.pane(&host, text(&args, "paneId"))?;
        if let Some(message) = args.get("message").filter(|m| m.is_object()) {
            pane.handle_message(message).await;
        }
        Ok(Value::Null)
    });
    methods.add("terminalRestart", |host: Arc<Host>, args| async move {
        let pane = terminals(&host)?.pane(&host, text(&args, "paneId"))?;
        pane.restart().await;
        Ok(Value::Null)
    });
    methods.add("terminalHistory", |host: Arc<Host>, args| async move {
        let pane = text(&args, "paneId").and_then(|id| host.view().snapshot.session(id).filter(|s| s.kind() == "shell").cloned());
        if let Some(pane) = pane {
            show_history(&host, &pane).await;
        }
        Ok(Value::Null)
    });
    methods.add("terminalPromptAnswer", |host: Arc<Host>, args| async move {
        let answer = TerminalPromptAnswer::from_phone(text(&args, "promptId"), text(&args, "answer"), text(&args, "text"))
            .ok_or_else(|| host.locale.get("terminalPrompt.error.badAnswer"))?;
        let terminals = terminals(&host)?;
        if !terminals.answer_prompt(text(&args, "paneId").unwrap_or(""), text(&args, "promptId").unwrap_or(""), &answer) {
            return Err(host.locale.get("terminalPrompt.error.gone"));
        }
        Ok(Value::Null)
    });
    methods.add("terminalPromptShow", |host: Arc<Host>, args| async move {
        host.view().error = None;
        let result = terminals(&host).and_then(|t| t.open_agent_terminal_pane(&host, text(&args, "paneId").unwrap_or("")));
        act(&host, result);
        Ok(Value::Null)
    });
    methods.add("shellBlockRun", |host: Arc<Host>, args| async move {
        host.view().error = None;
        let result = match terminals(&host) {
            Ok(t) => t.run_shell_block(&host, text(&args, "paneId"), text(&args, "key"), text(&args, "command")).await,
            Err(error) => Err(error),
        };
        act(&host, result);
        Ok(Value::Null)
    });
    methods.add("shellBlockStop", |host: Arc<Host>, args| async move {
        host.view().error = None;
        if let Ok(terminals) = terminals(&host) {
            terminals.stop_shell_block(text(&args, "paneId"), text(&args, "key")).await;
        }
        Ok(Value::Null)
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use mighty_core::locale::Locale;
    use mighty_core::model::Workspace;

    fn state() -> AppSnapshot {
        let workspace = Workspace { id: Some("w".into()), path: Some("C:\\work".into()), ..Workspace::default() };
        let agent = RunSession {
            id: Some("agent".into()),
            workspace_id: Some("w".into()),
            provider: Some("codex".into()),
            ..RunSession::default()
        };
        let other = RunSession { id: Some("other".into()), workspace_id: Some("w".into()), ..RunSession::default() };
        AppSnapshot {
            workspaces: Some(vec![workspace]),
            sessions: Some(vec![Arc::new(agent), Arc::new(other)]),
            active_workspace_id: Some("w".into()),
            active_session_id: Some("agent".into()),
            ..AppSnapshot::default()
        }
    }

    /// C# `WithAgentIOPane`: the agent's terminal pane opens to its right, titled "Terminal", owned by the agent, in its provider;
    /// the layout becomes custom; a second open changes nothing; an agent pane that is gone is refused.
    #[test]
    fn the_agents_terminal_pane_opens_beside_it() {
        let mut snapshot = state();
        let mut defaults = HashMap::new();
        with_agent_io_pane(&mut snapshot, &mut defaults, "agent", "w", AGENT_TERMINAL, Locale::new("en")).unwrap();
        let pane = snapshot.session("agent-terminal:agent").expect("the terminal pane");
        assert_eq!(
            (pane.kind(), pane.title(), pane.owner_session_id.as_deref(), pane.provider()),
            (AGENT_TERMINAL, "Terminal", Some("agent"), "codex")
        );
        assert_eq!(pane.workspace_profile_key.as_deref(), Some("w"));
        let modes = snapshot.pane_layout_modes.clone().unwrap();
        assert_eq!(modes.get("w").cloned().flatten().as_deref(), Some("custom"));
        let tree = snapshot.pane_layouts.as_ref().and_then(|l| l.get("w").cloned().flatten()).expect("a saved layout");
        let groups = layout::groups(&tree);
        assert!(groups.len() >= 2, "a group of its own to the right: {tree:?}");
        let again = snapshot.clone();
        with_agent_io_pane(&mut snapshot, &mut defaults, "agent", "w", AGENT_TERMINAL, Locale::new("en")).unwrap();
        assert_eq!(snapshot, again, "already open: unchanged");
        assert_eq!(
            with_agent_io_pane(&mut snapshot, &mut defaults, "gone", "w", AGENT_TERMINAL, Locale::new("en")),
            Err("Agent pane closed.".into())
        );
    }

    /// W9-J review L1: the remote mirror's panes are the host's and never count toward this PC's 128 (`PaneLayout.HasRoom`); this PC's
    /// own 128th pane still refuses the agent's terminal.
    #[test]
    fn mirrored_panes_never_count_toward_the_agents_terminal_room() {
        let mut snapshot = state();
        let mirror = mighty_core::model::MirrorReference {
            connection_id: "conn".into(),
            host_session_id: "host".into(),
            host_workspace_id: "ws".into(),
            host_name: "studio-pc".into(),
        };
        for i in 0..layout::MAXIMUM_SESSIONS {
            snapshot.sessions_mut().push(Arc::new(RunSession {
                id: Some(format!("far-{i}")),
                workspace_id: Some("w".into()),
                mirror: Some(mirror.clone()),
                ..RunSession::default()
            }));
        }
        let mut defaults = HashMap::new();
        with_agent_io_pane(&mut snapshot, &mut defaults, "agent", "w", AGENT_TERMINAL, Locale::new("en")).unwrap();
        assert!(snapshot.session("agent-terminal:agent").is_some(), "the mirrored panes leave room");
        let mut full = state();
        for i in 0..layout::MAXIMUM_SESSIONS - 2 {
            full.sessions_mut().push(Arc::new(RunSession {
                id: Some(format!("own-{i}")),
                workspace_id: Some("w".into()),
                ..RunSession::default()
            }));
        }
        assert_eq!(
            with_agent_io_pane(&mut full, &mut defaults, "agent", "w", AGENT_TERMINAL, Locale::new("en")),
            Err("Close a pane before opening another.".into()),
            "this PC's 128 panes leave none"
        );
    }

    fn prompt(handle: &str, length: usize) -> TerminalPrompt {
        TerminalPrompt {
            id: format!("{handle}:{length}"),
            handle: handle.into(),
            text: "Continue? (y/n)".into(),
            options: vec![],
            kind: "yesNo",
            default_yes: None,
        }
    }

    /// A poll reads the terminals with the prompts' lock let go: a card answered meanwhile never comes back from what that poll
    /// found, and is answered once only; the same command asking again after more output is a new card.
    #[test]
    fn an_answered_prompt_card_never_comes_back_from_an_earlier_poll() {
        let agents = HashSet::from(["agent".to_string()]);
        let mut prompts = Prompts::default();
        let asked = prompt("h", 10);
        assert_eq!(prompts.commit(HashMap::from([("agent".to_string(), asked.clone())]), &agents), ["agent"]);
        // A poll finds the card (lock let go) while the user answers it.
        let found = HashMap::from([("agent".to_string(), asked.clone())]);
        assert_eq!(prompts.take_answer("agent", &asked.id), Some(asked.clone()), "the answer takes the card");
        assert_eq!(prompts.take_answer("agent", &asked.id), None, "a second answer of the same card types nothing");
        assert!(prompts.commit(found, &agents).is_empty() && prompts.current.is_empty(), "the earlier poll does not bring it back");
        let again = prompt("h", 25);
        assert_eq!(prompts.commit(HashMap::from([("agent".to_string(), again.clone())]), &agents), ["agent"]);
        assert_eq!(prompts.current.get("agent"), Some(&again), "asking again after more output is a new card");
        assert!(prompts.commit(HashMap::new(), &HashSet::new()) == ["agent"] && prompts.answered.is_empty(), "a closed pane's go");
    }

    /// C# `ShellStatusChanged` (MainWindow.Terminal.cs:62; "a shell's run between two ticks"): a shell going to running tells the push hub
    /// it began, so a run that starts and ends between two of its reads still pushes; any other change says only that something moved.
    #[test]
    fn a_shells_run_between_two_ticks_reaches_the_push_hub() {
        assert!(matches!(shell_push_note("shell", Some("running")), crate::mobile::PushNote::Began(id) if id == "shell"));
        for status in [Some("completed"), Some("error"), Some("idle"), None] {
            assert!(matches!(shell_push_note("shell", status), crate::mobile::PushNote::Changed), "{status:?}");
        }
    }

    #[test]
    fn a_working_folder_is_a_full_path() {
        assert!(is_working_folder(if cfg!(windows) { "C:\\work\\child" } else { "/work/child" }));
        assert!(!is_working_folder("relative\\child") && !is_working_folder("a\0b") && !is_working_folder(&"x".repeat(4097)));
    }

    /// D14 against C#'s capture (`mighty-native/tests/golden/terminal-history.json`, plan W6 §1.8): C#'s web smoke's shell pane
    /// with its last request and answer, the panel as the page first got it.
    #[test]
    fn the_history_dialog_matches_cs() {
        let pane = RunSession {
            id: Some("smoke-history-pane".into()),
            kind: Some("shell".into()),
            logs: Some(vec![
                mighty_core::model::LogEntry::new("l1".into(), "user", "fixture request 2".into(), "2026-11-14T00:00:00Z".into(), None),
                mighty_core::model::LogEntry::new("l2".into(), "assistant", "fixture answer 2".into(), "2026-11-14T00:00:01Z".into(), None),
            ]),
            ..RunSession::default()
        };
        let golden = include_str!("../../crates/mighty-native/tests/golden/terminal-history.json");
        assert_eq!(crate::native::golden_dialog(history_dialog(&pane, &Locale::new("ko"))), golden.trim_end());
    }

    /// P1-PERF-1: `TerminalWake` parks idle terminal clock threads with zero wakeups while 0 terminals exist,
    /// wakes immediately when a pane/agent is created, and exits cleanly on `close()`.
    #[test]
    fn terminal_wake_parks_when_empty_and_wakes_on_create_or_close() {
        let wake = TerminalWake::new();
        let ticks = Arc::new(AtomicU64::new(0));
        let w_clock = wake.clone();
        let t_clock = ticks.clone();
        let clock = std::thread::spawn(move || {
            while let Some(has_panes) = w_clock.wait_active(true) {
                t_clock.fetch_add(1, Ordering::SeqCst);
                if !w_clock.sleep_or_close(Duration::from_millis(5), !has_panes) {
                    break;
                }
            }
        });
        let act_ticks = Arc::new(AtomicU64::new(0));
        let w_act = wake.clone();
        let t_act = act_ticks.clone();
        let activity = std::thread::spawn(move || {
            while w_act.wait_active(false).is_some() {
                t_act.fetch_add(1, Ordering::SeqCst);
                if !w_act.sleep_or_close(Duration::from_millis(5), false) {
                    break;
                }
            }
        });
        // With 0 panes and 0 agents, both threads remain completely parked.
        std::thread::sleep(Duration::from_millis(40));
        assert_eq!(ticks.load(Ordering::SeqCst), 0, "terminal-clock stayed parked with 0 terminals");
        assert_eq!(act_ticks.load(Ordering::SeqCst), 0, "shell-activity stayed parked with 0 panes");

        // Adding an agent wakes only terminal-clock, NOT shell-activity.
        wake.set_agents(1);
        for _ in 0..50 {
            if ticks.load(Ordering::SeqCst) >= 1 {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(ticks.load(Ordering::SeqCst) >= 1, "terminal-clock woke when agent created");
        assert_eq!(act_ticks.load(Ordering::SeqCst), 0, "shell-activity still parked with 0 panes");

        // Adding a pane wakes shell-activity immediately.
        wake.set_panes(1);
        for _ in 0..50 {
            if act_ticks.load(Ordering::SeqCst) >= 1 {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(act_ticks.load(Ordering::SeqCst) >= 1, "shell-activity woke when pane created");

        // Removing all panes/agents re-parks cleanly; closing wakes and terminates both threads immediately.
        wake.set_panes(0);
        wake.set_agents(0);
        std::thread::sleep(Duration::from_millis(20));
        let started = std::time::Instant::now();
        wake.close();
        clock.join().unwrap();
        activity.join().unwrap();
        assert!(started.elapsed() < Duration::from_millis(200), "both threads exited promptly on close");
    }
}
