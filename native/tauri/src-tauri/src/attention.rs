//! MC-26: the panes waiting for the user, and each pane's ports and pull request.
//!
//! - A terminal's notification (OSC 9, 777, 99) or bell (terminal.rs' `HostEvents::signal`, from the page's xterm or the Mac's Ghostty)
//!   marks its pane, at most once a second a pane, unless the pane is the one being looked at (the app in front, the pane active);
//!   with 터미널 알림 받기 on (the `terminal.notifications` preference, on by default). A notification also shows an OS notification;
//!   the bell only marks. An agent pane's approval or question card and a worktree setup that failed mark their pane too.
//! - Looking at a marked pane clears it. The jump key (⇧⌘U, Ctrl+Shift+U; the page's `attentionNext`) goes to the newest marked pane,
//!   the next press to the next. The frame model carries the mark on each pane (the sidebar's dot, the pane's border) and the
//!   dashboard's "나를 기다리는 창 n개"; the phone's pane summary carries it as `attention`.
//! - Every 10 s, while the window shows, the TCP ports the active workspace's panes listen on (their shell, agent and run actions, and
//!   everything under them: [`mighty_platform::listening`]); every 2 minutes a worktree pane's pull request (the Git workbench's
//!   `query_pr_status_value`).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, Weak};
use std::time::{Duration, Instant};

use mighty_core::attention::{AttentionBoard, AttentionKind};
use mighty_core::model::AppSnapshot;
use mighty_core::shell::{PanePr, ShellModel};
use mighty_platform::{PrefValue, PrefsStore};
use mighty_terminal::signals::{Signal, SignalLimiter, sanitize};
use serde_json::{Value, json};
use tauri::Manager;

use crate::host::{Host, Registry};

/// 터미널 알림 받기's preference name (on unless switched off).
pub const SETTING: &str = "terminal.notifications";
/// How often the shown panes' ports are read.
const PORTS_EVERY: Duration = Duration::from_secs(10);
/// How long a worktree's pull request stands before it is asked again.
const PR_FOR: Duration = Duration::from_secs(120);
/// How long a reading of the preference stands.
const SETTING_FOR: Duration = Duration::from_secs(2);

#[derive(Default)]
struct State {
    board: Mutex<AttentionBoard>,
    limiter: Mutex<SignalLimiter>,
    /// The ports each shown pane listens on.
    ports: Mutex<HashMap<String, Vec<u16>>>,
    /// Each worktree's pull request (by its folder) and when it was asked; None while there is none.
    prs: Mutex<HashMap<String, (Instant, Option<PanePr>)>>,
    /// The worktrees whose pull request is being asked now.
    asking: Mutex<HashSet<String>>,
    /// The preference as last read (or switched in this run when it may not be written) and when.
    setting: Mutex<Option<(bool, Option<Instant>)>>,
}

fn state() -> &'static State {
    static STATE: OnceLock<State> = OnceLock::new();
    STATE.get_or_init(State::default)
}

/// Whether the app's window has the keyboard (its focus events; true until one says otherwise).
static FRONT: AtomicBool = AtomicBool::new(true);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

// ── the setting ──────────────────────────────────────────────────────────────────────────────────────────────────────────

/// 터미널 알림 받기: on unless switched off.
pub fn enabled(host: &Host) -> bool {
    let reading = *lock(&state().setting);
    match reading {
        Some((on, None)) => return on,
        Some((on, Some(at))) if at.elapsed() < SETTING_FOR => return on,
        _ => {}
    }
    let saved = host.app.try_state::<Arc<dyn PrefsStore>>().and_then(|prefs| prefs.get(SETTING).ok().flatten());
    let on = saved.is_none_or(|value| value.is_on());
    *lock(&state().setting) = Some((on, Some(Instant::now())));
    on
}

/// Switches 터미널 알림 받기 (Settings › 알림): saved where the app may change the machine, else held for this run.
pub fn set_enabled(host: &Host, on: bool) {
    let saved = host
        .app
        .try_state::<Arc<dyn PrefsStore>>()
        .filter(|_| host.may_change_machine())
        .map(|prefs| prefs.set(SETTING, PrefValue::Int(i64::from(on))));
    match saved {
        Some(Ok(())) => *lock(&state().setting) = Some((on, Some(Instant::now()))),
        Some(Err(error)) => {
            host.log.line(&format!("terminal notifications switch not saved: {error}"));
            *lock(&state().setting) = None;
        }
        None => *lock(&state().setting) = Some((on, None)),
    }
    if !on && !lock(&state().board).is_empty() {
        // Marks already made stay until looked at; only new signals stop.
        host.shell_dirty();
    }
}

// ── the marks ────────────────────────────────────────────────────────────────────────────────────────────────────────────

/// Whether `pane` is the one being looked at: the app in front, the pane active, no dashboard over it.
fn viewed(snapshot: &AppSnapshot, shows_dashboard: bool, pane: &str) -> bool {
    FRONT.load(Ordering::SeqCst) && !shows_dashboard && snapshot.active_session_id.as_deref() == Some(pane)
}

/// A terminal's notification or bell (terminal.rs' `HostEvents::signal`).
pub fn signal(host: &Arc<Host>, pane: &str, signal: Signal) {
    if host.closing() || !enabled(host) {
        return;
    }
    let (title, looked_at) = {
        let view = host.view();
        let Some(session) = view.snapshot.session(pane) else { return };
        (session.title().to_string(), viewed(&view.snapshot, view.shows_dashboard, pane))
    };
    if !lock(&state().limiter).allow(pane, Instant::now()) || looked_at {
        return;
    }
    let (kind, text) = match &signal {
        Signal::Notify { .. } => (AttentionKind::Notify, signal.text()),
        Signal::Bell => (AttentionKind::Bell, String::new()),
    };
    if !lock(&state().board).raise(pane, kind, &text, now_ms()) {
        return;
    }
    host.shell_dirty();
    host.schedule();
    crate::phone::changed(host);
    // MC-9: scripts hear that the pane waits (its kind only, never the terminal's words).
    crate::control::attention(host, pane, kind);
    if kind == AttentionKind::Notify {
        notify(host, pane, &title, &text);
    }
}

/// The OS notification of a terminal's notification: the pane's title over its words; its click focuses the pane.
fn notify(host: &Arc<Host>, pane: &str, title: &str, text: &str) {
    if crate::notifications::permission(host) != crate::notifications::NotificationPermission::Allowed {
        return;
    }
    let Some(ports) = host.app.try_state::<crate::platform::Ports>() else { return };
    let notifier = ports.notifier.clone();
    let body = if text.is_empty() { AttentionKind::Notify.label(&host.locale) } else { text.to_string() };
    let notification = mighty_platform::Notification { title: title.to_string(), body, pane_id: Some(pane.to_string()) };
    let log = host.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(error) = notifier.notify(notification).await {
            log.log.line(&format!("attention: not shown: {error}"));
        }
    });
}

/// A worktree's setup failed (repo_tasks.rs): its pane waits.
pub fn setup_failed(host: &Host, pane: &str, command: &str, exit_code: i32) {
    let text = sanitize(&format!("{command} ({exit_code})"));
    if lock(&state().board).raise(pane, AttentionKind::SetupFailed, &text, now_ms()) {
        host.shell_dirty();
        crate::control::attention(host, pane, AttentionKind::SetupFailed);
    }
}

/// A worktree's setup passed: a failure's mark goes.
pub fn setup_passed(host: &Host, pane: &str) {
    if lock(&state().board).clear_kind(pane, &[AttentionKind::SetupFailed]) {
        host.shell_dirty();
    }
}

/// The newest card of `pane` waiting on the user (a question or an approval) and its words.
pub(crate) fn latest_request(host: &Host, pane: &str) -> Option<(AttentionKind, String, String)> {
    let requests = host.runs()?.service.pending_permissions(pane);
    let request = requests.into_iter().rfind(|r| r.state == "pending" && crate::styles::shows(r))?;
    let kind = if request.can_answer_questions { AttentionKind::Question } else { AttentionKind::Approval };
    let words = if request.summary.trim().is_empty() { &request.tool_name } else { &request.summary };
    Some((kind, request.id.clone(), sanitize(words)))
}

/// The frame model's marks, ports and pull requests (host.rs' `shell_model_with`, the view locked): the cards waiting are read into
/// the board, the pane being looked at is cleared, and the panes that are gone are dropped.
pub fn decorate(host: &Host, snapshot: &AppSnapshot, shows_dashboard: bool, model: &mut ShellModel) {
    let at = now_ms();
    let changed = {
        let mut board = lock(&state().board);
        let mut changed = board.retain(|pane| snapshot.session(pane).is_some());
        for session in snapshot.sessions() {
            if session.kind() != "claude" {
                continue;
            }
            let latest = latest_request(host, session.id());
            changed |= board.sync_request(session.id(), latest.as_ref().map(|(kind, id, text)| (*kind, id.as_str(), text.as_str())), at);
        }
        if let Some(active) = snapshot.active_session_id.as_deref().filter(|pane| viewed(snapshot, shows_dashboard, pane)) {
            changed |= board.clear(active);
        }
        for pane in &mut model.panes {
            pane.attention = board.get(&pane.id).map(|attention| attention.info(&host.locale));
        }
        if let Some(dashboard) = model.dashboard.as_mut() {
            let count = board.len();
            dashboard.attention = (count > 0).then(|| host.locale.get_with("attention.dashboard", &[("count", &count.to_string())]));
        }
        changed
    };
    if changed {
        crate::phone::changed(host);
    }
    let ports = lock(&state().ports);
    let prs = lock(&state().prs);
    for pane in &mut model.panes {
        pane.ports = ports.get(&pane.id).cloned().unwrap_or_default();
        pane.pr = snapshot
            .session(&pane.id)
            .and_then(|session| session.pane_worktree.as_ref())
            .and_then(|worktree| prs.get(worktree.worktree_path()))
            .and_then(|(_, pr)| pr.clone());
    }
}

/// `pane`'s mark for the phone's pane summary: its kind, words and when (RFC 3339).
pub fn phone(pane: &str) -> Option<(String, String, String)> {
    let board = lock(&state().board);
    let attention = board.get(pane)?;
    let at = chrono::DateTime::from_timestamp_millis(attention.at).map(|at| at.to_rfc3339()).unwrap_or_default();
    Some((attention.kind.name().to_string(), attention.text.clone(), at))
}

/// The jump key: the newest marked pane other than the one in view, brought forward and selected (it clears once looked at).
pub fn jump(host: &Arc<Host>) -> Option<String> {
    let current = host.view().snapshot.active_session_id.clone();
    let target = lock(&state().board).next_target(current.as_deref())?;
    crate::notifications::focus(host, &target);
    host.shell_dirty();
    host.schedule();
    Some(target)
}

pub fn register(methods: &mut Registry) {
    methods.add("attentionNext", |host: Arc<Host>, _args| async move { Ok(json!({ "paneId": jump(&host) })) });
}

// ── ports and pull requests ──────────────────────────────────────────────────────────────────────────────────────────────

/// A shown pane as the reading needs it.
struct Shown {
    id: String,
    worktree: Option<String>,
}

/// The active workspace's panes on this computer (none while the dashboard covers them).
fn shown_panes(host: &Host) -> Vec<Shown> {
    let view = host.view();
    if view.shows_dashboard {
        return Vec::new();
    }
    let snapshot = &view.snapshot;
    let Some(workspace) = snapshot.active_workspace_id.as_deref().and_then(|id| snapshot.workspace(id)) else { return Vec::new() };
    if workspace.remote.is_some() {
        return Vec::new();
    }
    snapshot
        .sessions()
        .iter()
        .filter(|s| s.workspace_id() == workspace.id())
        .map(|s| Shown { id: s.id().to_string(), worktree: s.pane_worktree.as_ref().map(|w| w.worktree_path().to_string()) })
        .collect()
}

/// Whether the window shows (not hidden, not minimized).
fn window_shows(host: &Host) -> bool {
    host.app
        .get_webview_window(crate::host::WINDOW)
        .is_some_and(|window| window.is_visible().unwrap_or(true) && !window.is_minimized().unwrap_or(false))
}

/// One reading of the shown panes' ports; the frame is told when any changed.
fn read_ports(host: &Host, shown: &[Shown]) {
    let mut next: HashMap<String, Vec<u16>> = HashMap::new();
    // The processes of each shown pane first: the system-wide socket reading (an `lsof` on the Mac) only runs when one has any.
    let roots: Vec<(&str, Vec<u32>)> = shown
        .iter()
        .map(|pane| {
            let mut roots = crate::terminal::process_ids(host, &pane.id);
            roots.extend(host.runs().and_then(|runs| runs.service.process_id(&pane.id)));
            roots.extend(crate::repo_tasks::running_pids(host, &pane.id));
            (pane.id.as_str(), roots)
        })
        .filter(|(_, roots)| !roots.is_empty())
        .collect();
    if !roots.is_empty()
        && let Some(net) = crate::platform::process_net()
    {
        for (pane, roots) in roots {
            let ports = net.ports(roots);
            if !ports.is_empty() {
                next.insert(pane.to_string(), ports);
            }
        }
    }
    let mut ports = lock(&state().ports);
    if *ports != next {
        *ports = next;
        host.shell_dirty();
    }
}

/// The pull request of each shown worktree whose reading is older than [`PR_FOR`], asked in the background.
fn read_prs(host: &Arc<Host>, shown: &[Shown]) {
    let due: Vec<String> = {
        let prs = lock(&state().prs);
        let mut asking = lock(&state().asking);
        shown
            .iter()
            .filter_map(|pane| pane.worktree.clone())
            .collect::<HashSet<String>>()
            .into_iter()
            .filter(|folder| prs.get(folder).is_none_or(|(at, _)| at.elapsed() >= PR_FOR) && asking.insert(folder.clone()))
            .collect()
    };
    for folder in due {
        let host = host.clone();
        tauri::async_runtime::spawn(async move {
            let value = crate::git_workbench::query_pr_status_value(&host, &PathBuf::from(&folder)).await;
            let pr = pane_pr(&host, &value);
            lock(&state().asking).remove(&folder);
            let mut prs = lock(&state().prs);
            let changed = prs.get(&folder).is_none_or(|(_, old)| *old != pr);
            prs.insert(folder, (Instant::now(), pr));
            if changed {
                host.shell_dirty();
            }
        });
    }
}

/// The pull request of the worktree at `folder` as the pane chips last read it (the review queue's cards, merge_checks.rs).
pub fn cached_pr(folder: &str) -> Option<PanePr> {
    lock(&state().prs).get(folder).and_then(|(_, pr)| pr.clone())
}

/// The Git workbench's answer (`{hosting, pr}`) as a pane's pull request and its words.
fn pane_pr(host: &Host, value: &Value) -> Option<PanePr> {
    let pr = value.get("pr").filter(|pr| pr.is_object())?;
    let number = pr.get("number")?.as_i64()?;
    let text = |name: &str| pr.get(name).and_then(Value::as_str).unwrap_or("").to_string();
    let draft = pr.get("isDraft").and_then(Value::as_bool).unwrap_or(false);
    let state = match text("state").as_str() {
        "merged" => "merged",
        "closed" => "closed",
        _ if draft => "draft",
        _ => "open",
    };
    let checks = match text("checks").as_str() {
        "passed" => "passed",
        "failed" => "failed",
        "pending" => "pending",
        _ => "none",
    };
    Some(PanePr {
        number,
        state: state.to_string(),
        checks: checks.to_string(),
        url: text("url"),
        help: pr_help(&host.locale, number, state, checks, &text("title")),
    })
}

/// "PR #12 · 열림 · 검사 통과" over the title.
fn pr_help(locale: &mighty_core::locale::Locale, number: i64, state: &str, checks: &str, title: &str) -> String {
    let state_word = match state {
        "merged" => locale.get("git.pr.state.merged"),
        "closed" => locale.get("git.pr.state.closed"),
        "draft" => locale.get("panePr.draft"),
        _ => locale.get("git.pr.state.open"),
    };
    let mut help = locale.get_with("git.pr.line", &[("number", &number.to_string()), ("state", &state_word)]);
    let checks_word = match checks {
        "passed" => Some(locale.get("git.pr.checks.passed")),
        "failed" => Some(locale.get("git.pr.checks.failed")),
        "pending" => Some(locale.get("git.pr.checks.pending")),
        _ => None,
    };
    if let Some(word) = checks_word {
        help.push_str(" · ");
        help.push_str(&word);
    }
    let title = sanitize(title);
    if !title.is_empty() {
        help.push('\n');
        help.push_str(&title);
    }
    help
}

/// The window's focus followed (main.rs, as the main window is made: at the start, or later in an app started without one).
pub fn watch_window(host: &Arc<Host>, window: &tauri::WebviewWindow) {
    let weak = Arc::downgrade(host);
    window.on_window_event(move |event| {
        if let tauri::WindowEvent::Focused(front) = event {
            FRONT.store(*front, Ordering::SeqCst);
            // Coming back to the pane that was marked clears it.
            if *front && let Some(host) = weak.upgrade() {
                host.shell_dirty();
            }
        }
    });
}

/// MC-3: whether the app is in front, as the headless server says it (no window, or its window hidden: nobody looks at any pane).
pub fn set_front(front: bool) {
    FRONT.store(front, Ordering::SeqCst);
}

/// The ports and pull requests read while the window shows (main.rs, once the host started; never in a smoke).
pub fn start(host: &Arc<Host>) {
    if host.smoke.is_some() {
        return;
    }
    let weak: Weak<Host> = Arc::downgrade(host);
    let _ = std::thread::Builder::new().name("pane-ports".into()).spawn(move || {
        loop {
            let Some(host) = weak.upgrade() else { return };
            if !host.wait_or_close(PORTS_EVERY) {
                return;
            }
            if !host.is_ready() || !window_shows(&host) {
                continue;
            }
            let shown = shown_panes(&host);
            read_ports(&host, &shown);
            read_prs(&host, &shown);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use mighty_core::locale::Locale;

    #[test]
    fn a_pull_request_reads_its_state_and_checks() {
        let ko = Locale::new("ko");
        let help = pr_help(&ko, 12, "draft", "failed", "Fix\nthe build");
        assert_eq!(help, format!("PR #12 · {} · {}\nFix the build", ko.get("panePr.draft"), ko.get("git.pr.checks.failed")));
        assert_eq!(pr_help(&ko, 3, "merged", "none", ""), format!("PR #3 · {}", ko.get("git.pr.state.merged")));
    }

    #[test]
    fn a_pane_is_viewed_only_when_active_in_front_and_not_under_the_dashboard() {
        let snapshot = AppSnapshot { active_session_id: Some("a".into()), ..AppSnapshot::default() };
        FRONT.store(true, Ordering::SeqCst);
        assert!(viewed(&snapshot, false, "a"));
        assert!(!viewed(&snapshot, false, "b"), "another pane");
        assert!(!viewed(&snapshot, true, "a"), "under the dashboard");
        FRONT.store(false, Ordering::SeqCst);
        assert!(!viewed(&snapshot, false, "a"), "the app behind");
        FRONT.store(true, Ordering::SeqCst);
    }
}
