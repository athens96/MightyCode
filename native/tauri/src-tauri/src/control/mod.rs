//! MC-9: scripts drive the app, and hear what its panes do.
//!
//! - A local control API ([`server`]) on the profile's Unix socket (`<data>/control/mighty.sock`, 0600 in a 0700 folder; the Mac) or
//!   an owner-only named pipe that refuses remote clients (Windows), never in a smoke. Every request carries the token in
//!   `<data>/control/token` (0600; [`endpoint`]), which Settings › 자동화 makes new ([`rotate_token`]). The `mighty` CLI ([`client`]) is
//!   the exe's `--cli` mode.
//! - One event bus ([`bus`]): a pane's state moving (started, waiting for an approval or an answer, done, failed, stopped), a pane
//!   marked as waiting (MC-26), and a run's end with its cost. The socket's `events` stream and waits, the status hooks ([`hooks`])
//!   and the webhooks ([`webhooks`]) all read it; MC-32's triggers will too.
//! - Where things happen calls in: the run engine's pump after each event ([`on_run_event`], [`run_finished`]), a shell pane's status
//!   ([`touch`]), the attention marks ([`attention`]) and the terminals' output ([`terminal_output`]).

pub mod backend;
pub mod bus;
pub mod client;
pub mod endpoint;
pub mod hooks;
pub mod server;
#[cfg(test)]
mod tests;
pub mod webhooks;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use base64::Engine as _;
use mighty_core::attention::AttentionKind;
use mighty_core::control::events::{BlockReason, EventBody, PaneRef, PaneState, StateEvent, pane_kind, transition};
use mighty_core::control::text::{OutputRing, strip_ansi};
use mighty_core::model::{AppSnapshot, RunSession};
use mighty_platform::{AgentIoListener, PrefValue, PrefsStore};
use serde_json::{Value, json};
use tauri::Manager;

use crate::host::{Host, View};

/// Settings › 자동화's preferences: the states announced (comma-separated) and the hook's command.
pub const EVENTS_SETTING: &str = "automation.events";
pub const HOOK_SETTING: &str = "automation.hookCommand";
/// The longest hook command Settings keeps.
pub const MAX_HOOK_CHARS: usize = 2_000;
/// A terminal's output kept for `pane read`: lines, and characters a line.
const RING_LINES: usize = 400;
const RING_LINE_CHARS: usize = 2_000;

#[derive(Default)]
struct Settings {
    loaded: bool,
    events: Vec<StateEvent>,
    hook: String,
}

#[derive(Default)]
struct Control {
    auth: Mutex<Option<Arc<server::Auth>>>,
    listener: Mutex<Option<(Arc<dyn AgentIoListener>, String)>>,
    /// Each pane's state as last announced.
    states: Mutex<HashMap<String, PaneState>>,
    /// Each terminal's last lines, and the bytes of a character its last chunk cut.
    rings: Mutex<HashMap<String, (OutputRing, Vec<u8>)>>,
    settings: Mutex<Settings>,
}

fn control() -> &'static Control {
    static CONTROL: OnceLock<Control> = OnceLock::new();
    CONTROL.get_or_init(Control::default)
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|p| p.into_inner())
}

// ── start and stop ───────────────────────────────────────────────────────────────────────────────────────────────────

/// Starts the control API, the hooks and the webhooks; not in a smoke.
pub fn start(host: &Arc<Host>) {
    if host.smoke.is_some() {
        return;
    }
    {
        let view = host.view();
        let mut states = lock(&control().states);
        for session in view.snapshot.sessions() {
            states.insert(session.id().to_string(), PaneState::of(session.status(), false, None));
        }
    }
    load_settings(host);
    let loader = host.clone();
    tauri::async_runtime::spawn(async move { webhooks::load(&loader).await });
    dispatch(host);
    listen(host);
}

/// Hooks and webhooks follow the bus.
fn dispatch(host: &Arc<Host>) {
    let weak = Arc::downgrade(host);
    let mut rx = bus::global().subscribe();
    tauri::async_runtime::spawn(async move {
        loop {
            let event = match rx.recv().await {
                Ok(bus::BusEvent::Control(event)) => event,
                Ok(bus::BusEvent::Output { .. }) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            };
            let Some(host) = weak.upgrade() else { return };
            if host.closing() {
                return;
            }
            let (chosen, hook) = {
                let settings = lock(&control().settings);
                (settings.events.clone(), settings.hook.clone())
            };
            webhooks::on_event(&host, &event, &chosen);
            hooks::on_event(&host, &event, Some(hook), chosen);
        }
    });
}

/// The listener bound, its endpoint and token written, then served on a task of its own.
fn listen(host: &Arc<Host>) {
    // Bound inside the app's runtime (Windows registers the pipe's first instance with it), whatever thread calls.
    let runtime = tauri::async_runtime::handle();
    let _entered = runtime.inner().enter();
    let (listener, address, auth) = match open(&host.data_dir, &host.version) {
        Ok(opened) => opened,
        Err(error) => {
            host.log.line(&format!("control: not listening: {error}"));
            return;
        }
    };
    *lock(&control().auth) = Some(auth.clone());
    *lock(&control().listener) = Some((listener.clone(), address));
    host.log.line("control: listening");
    let backend: Arc<dyn server::Backend> = Arc::new(backend::HostBackend::new(host));
    tauri::async_runtime::spawn(server::serve(listener, backend, auth));
}

/// What [`open`] gives: the listener, its address and the token's check.
pub type Opened = (Arc<dyn AgentIoListener>, String, Arc<server::Auth>);

/// The profile's control listener bound (refused while another instance serves it), then its token (kept, or new) and endpoint
/// written: the listener, its address and the token's check.
pub fn open(data_dir: &std::path::Path, version: &str) -> Result<Opened, String> {
    let listener = crate::platform::control_listener(data_dir);
    let address = listener.bind().map_err(|e| e.to_string())?;
    let opened = endpoint::token_or_new(data_dir).and_then(|token| {
        endpoint::write_endpoint(
            data_dir,
            &endpoint::Endpoint {
                transport: if cfg!(windows) { "pipe" } else { "unix" }.into(),
                address: address.clone(),
                pid: std::process::id(),
                version: version.to_string(),
                elevated: crate::platform::elevated(),
            },
        )
        .map(|()| token)
    });
    match opened {
        Ok(token) => Ok((listener, address, Arc::new(server::Auth::new(token)))),
        Err(error) => {
            listener.close();
            Err(error.to_string())
        }
    }
}

/// The app quits: the listener closes and its endpoint file goes (the token stays for the next run).
pub fn shutdown(host: &Host) {
    let taken = lock(&control().listener).take();
    if let Some((listener, address)) = taken {
        listener.close();
        endpoint::remove_endpoint(&host.data_dir, &address);
    }
}

/// Whether the control API listens.
pub fn listening() -> bool {
    lock(&control().listener).is_some()
}

/// Settings' 토큰 새로 만들기: a new token in the file, and every connection opened under the old one closed.
pub fn rotate_token(host: &Host) -> Result<(), String> {
    let Some(auth) = lock(&control().auth).clone() else { return Err(host.locale.get("automation.api.off")) };
    let token = endpoint::new_token();
    endpoint::write_token(&host.data_dir, &token).map_err(|e| e.to_string())?;
    auth.replace(token);
    host.log.line("control: token made new");
    Ok(())
}

// ── panes as scripts see them ────────────────────────────────────────────────────────────────────────────────────────

/// The ids and names an event or a listing carries for `session`.
pub fn pane_ref(snapshot: &AppSnapshot, session: &RunSession) -> PaneRef {
    let kind = pane_kind(session.kind()).to_string();
    PaneRef {
        pane: session.id().to_string(),
        workspace: session.workspace_id().to_string(),
        title: session.title().to_string(),
        workspace_name: snapshot.workspace(session.workspace_id()).map(|w| w.name().to_string()).unwrap_or_default(),
        agent: if kind == "agent" { session.provider().to_string() } else { String::new() },
        kind,
    }
}

/// `session`'s state now: the card it waits on, whether a send of it is starting, its saved status.
pub fn pane_state(host: &Host, view: &View, session: &RunSession) -> (PaneState, Option<BlockReason>) {
    let id = session.id();
    let blocked = if session.kind() == "claude" {
        crate::attention::latest_request(host, id)
            .map(|(kind, _, _)| if kind == AttentionKind::Question { BlockReason::Question } else { BlockReason::Approval })
    } else {
        None
    };
    // The saved status follows the run's own events (`running`, then `completed`, `stopped` or `error`) in the order the window took
    // them; the run engine's own list may still hold a run whose last status already came, so it is not asked. A send that has not
    // started its run yet counts as running.
    let starting = view.panes.get(id).is_some_and(|ui| ui.view.starting || ui.view.queue_starting);
    (PaneState::of(session.status(), starting, blocked), blocked)
}

/// A pane as `status` and `pane list` give it.
pub fn pane_json(host: &Host, view: &View, session: &RunSession) -> Value {
    let snapshot = &view.snapshot;
    let reference = pane_ref(snapshot, session);
    let (state, reason) = pane_state(host, view, session);
    let attention = crate::attention::phone(session.id()).map(|(kind, _, at)| json!({ "kind": kind, "at": at }));
    let worktree = session.pane_worktree.as_ref().map(|w| {
        json!({
            "branch": w.branch(),
            "base": w.base_branch(),
            "path": w.worktree_path(),
            "runFolder": w.run_folder(),
            "port": w.port,
            "portBlock": w.port_block().map(|(base, count)| json!({ "base": base, "count": count })),
        })
    });
    json!({
        "id": reference.pane,
        "workspace": reference.workspace,
        "workspaceName": reference.workspace_name,
        "title": reference.title,
        "kind": reference.kind,
        "agent": if reference.agent.is_empty() { Value::Null } else { json!(reference.agent) },
        "model": if reference.kind == "agent" && !session.model().is_empty() { json!(session.model()) } else { Value::Null },
        "status": session.status(),
        "state": state.name(),
        "reason": reason.map(BlockReason::name),
        "attention": attention,
        "active": snapshot.active_session_id.as_deref() == Some(session.id()),
        "worktree": worktree,
        "parent": session.parent_session_id,
    })
}

// ── what happens, onto the bus ───────────────────────────────────────────────────────────────────────────────────────

/// A pane may have moved: its state read again and, when it changed, announced. A pane that is gone is forgotten.
pub fn touch(host: &Host, pane: &str) {
    if host.closing() {
        return;
    }
    let read = {
        let view = host.view();
        view.snapshot.session(pane).map(|session| {
            let (state, reason) = pane_state(host, &view, session);
            (pane_ref(&view.snapshot, session), state, reason)
        })
    };
    let Some((reference, to, reason)) = read else {
        lock(&control().states).remove(pane);
        lock(&control().rings).remove(pane);
        return;
    };
    let from = lock(&control().states).insert(pane.to_string(), to).unwrap_or(PaneState::Idle);
    if from != to {
        bus::global().publish(reference, EventBody::State { from, to, event: transition(from, to), reason });
    }
}

/// A run-engine event about `pane` went through the window (runs.rs' pump): its state is read again, and a log entry's text is new
/// output for `pane wait-output`.
pub fn on_run_event(host: &Host, pane: &str, entry_text: Option<&str>) {
    if let Some(text) = entry_text {
        bus::global().output(pane, text);
    }
    touch(host, pane);
}

/// A run of `pane` ended (`completed`, `stopped`, `error`), with what it cost when the agent said.
pub fn run_finished(host: &Host, pane: &str, status: &str, cost_usd: Option<f64>) {
    let read = {
        let view = host.view();
        view.snapshot.session(pane).map(|session| {
            let duration_ms = session.run_timing.filter(|t| t.is_valid()).map(|t| {
                let end = t.finished_at.unwrap_or(t.last_observed_at);
                (end.utc_ticks() - t.started_at.utc_ticks()) / 10_000
            });
            (pane_ref(&view.snapshot, session), duration_ms)
        })
    };
    if let Some((reference, duration_ms)) = read {
        bus::global().publish(reference, EventBody::RunCompleted { status: status.to_string(), cost_usd, duration_ms });
    }
}

/// MC-26 marked `pane` as waiting.
pub fn attention(host: &Host, pane: &str, kind: AttentionKind) {
    let reference = {
        let view = host.view();
        view.snapshot.session(pane).map(|session| pane_ref(&view.snapshot, session))
    };
    if let Some(reference) = reference {
        bus::global().publish(reference, EventBody::Attention { kind: kind.name().to_string() });
    }
}

/// A terminal's output (terminal.rs' posts: base64 UTF-8 as the page's xterm gets it): kept as plain lines for `pane read`, and its
/// whole lines sent on for `pane wait-output` while someone waits.
pub fn terminal_output(pane: &str, data: &str) {
    let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(data) else { return };
    let lines = {
        let mut rings = lock(&control().rings);
        let (ring, carry) = rings.entry(pane.to_string()).or_insert_with(|| (OutputRing::new(RING_LINES, RING_LINE_CHARS), Vec::new()));
        carry.extend_from_slice(&bytes);
        // A chunk may end inside a character: its bytes wait for the next one.
        let valid = match std::str::from_utf8(carry) {
            Ok(text) => text.len(),
            Err(error) if error.error_len().is_none() => error.valid_up_to(),
            Err(_) => carry.len(),
        };
        let text = String::from_utf8_lossy(&carry[..valid]).into_owned();
        carry.drain(..valid);
        if carry.len() > 8 {
            carry.clear();
        }
        ring.push(&strip_ansi(&text))
    };
    if !lines.is_empty() && bus::global().heard() {
        bus::global().output(pane, &lines.join("\n"));
    }
}

/// A terminal's last `n` lines, as kept since the app started.
pub fn terminal_tail(pane: &str, n: usize) -> Vec<String> {
    lock(&control().rings).get(pane).map(|(ring, _)| ring.tail(n)).unwrap_or_default()
}

// ── Settings › 자동화 ──────────────────────────────────────────────────────────────────────────────────────────────────

fn prefs(host: &Host) -> Option<Arc<dyn PrefsStore>> {
    host.app.try_state::<Arc<dyn PrefsStore>>().map(|p| p.inner().clone())
}

fn load_settings(host: &Host) {
    let read = |key: &str| prefs(host).and_then(|p| p.get(key).ok().flatten()).and_then(|v| v.text().map(str::to_string));
    let events = read(EVENTS_SETTING).map_or_else(|| StateEvent::DEFAULT.to_vec(), |text| StateEvent::parse_list(&text));
    let hook = read(HOOK_SETTING).unwrap_or_default();
    *lock(&control().settings) = Settings { loaded: true, events, hook };
}

fn settings<T>(host: &Host, read: impl FnOnce(&Settings) -> T) -> T {
    if !lock(&control().settings).loaded {
        load_settings(host);
    }
    read(&lock(&control().settings))
}

/// Saved where the app may change the machine; otherwise held for this run.
fn save_setting(host: &Host, key: &str, text: &str) {
    if !host.may_change_machine() {
        return;
    }
    if let Some(prefs) = prefs(host)
        && let Err(error) = prefs.set(key, PrefValue::Text(text.to_string()))
    {
        host.log.line(&format!("automation setting not saved: {error}"));
    }
}

/// The states hooks and webhooks announce.
pub fn chosen_events(host: &Host) -> Vec<StateEvent> {
    settings(host, |s| s.events.clone())
}

pub fn set_event(host: &Host, event: StateEvent, on: bool) {
    let mut chosen = chosen_events(host);
    chosen.retain(|e| *e != event);
    if on {
        chosen.push(event);
    }
    let text = StateEvent::list_text(&chosen);
    lock(&control().settings).events = StateEvent::parse_list(&text);
    save_setting(host, EVENTS_SETTING, &text);
}

/// Settings' hook command.
pub fn hook_command(host: &Host) -> String {
    settings(host, |s| s.hook.clone())
}

pub fn set_hook_command(host: &Host, command: &str) {
    let command: String = command.trim().chars().take(MAX_HOOK_CHARS).collect();
    lock(&control().settings).hook = command.clone();
    save_setting(host, HOOK_SETTING, &command);
}
