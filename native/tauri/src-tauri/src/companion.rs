//! The companion pet in the window (C# WinUI/MainWindow.Companion.cs, MainWindow.CompanionControls.cs, MainWindow.WebShell.Settings.cs:
//! 138-139, 318-336, 462-470) over mighty-companion and the platform's `PetWindow`. Owner: W8-E.
//!
//! - `companion-settings.json` is read at start (and written only with `--allow-state-write`, `Host::writes`).
//! - The pet window opens through the main window's `SurfaceFactory::open_pet` only where the app may change the machine
//!   (`--allow-state-write`, never in a smoke, plan §4 and D9): elsewhere the settings box says verification mode. A smoke shows only
//!   its own test overlay (smoke_companion.rs).
//! - A 1/12 s clock draws the pet (its row's frame, the turning spinner) and every 12th tick reads the panes again (C#'s timer): only
//!   Claude panes are followed, a waiting request, a waiting activity and a run alive only for the notices it awaits all read "waiting";
//!   the most pressing agent is shown unless one was paged to ([`mighty_companion::follow`]); a request's card is keyed by the SHA-256
//!   of its tool and input, so a press on a card that changed meanwhile is nobody's.
//! - The bubble's answers go through the run service: Allow once and Deny, the question's taps, Back, Next and Send, the plan's Cancel
//!   and its two approvals. Its review, written answer and plan buttons focus the pane, whose own card shows the whole request (C# opens
//!   separate WinUI windows; the page has none, D10).
//! - `companionStatus` opens the `companion` popover (the agents' cards and the pet switch), `companionToggle` flips the pet, and the
//!   frame model's `companion` is the status bar's paw and capsule.
//!
//! Locks: the companion's state lock is never held while the view's is taken (the shell model reads [`frame_model`] with the view
//! locked), and the overlay's lock is taken before the state's.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use mighty_companion::card::{CompanionCard, CompanionOption, CompanionRequest};
use mighty_companion::follow::{self, Agent, Follower};
use mighty_companion::pet::CompanionPet;
use mighty_companion::prefs::{self, CompanionPreferences};
use mighty_companion::progress::CompanionProgress;
use mighty_companion::scene::PROGRESS_FOLD;
use mighty_companion::{Companion, CompanionAction, bubble};
use mighty_core::design_tokens::{Token, metrics};
use mighty_core::model::{AppSnapshot, RunSession};
use mighty_core::permission::ToolPermissionRequest;
use mighty_core::shell::CompanionFrame;
use mighty_native::kit::TextStyle;
use mighty_native::surface::{Next, SurfaceModel};
use mighty_native::ui::{Border, El, Grid, HAlign, Thickness, Track, Ui, VAlign, Wrapping};
use mighty_native::view::{Place, View};
use mighty_platform::pet::PetBitmap;
use mighty_platform::{PetScene, PetWindow, Platform, SurfaceFactory};
use mighty_run::plan_mode::PlanDecision;
use mighty_run::presentation::ToolPermissionPresentation;
use mighty_run::questionnaire::{QuestionnaireProgress, UserQuestionnaire};
use serde_json::Value;
use tauri::Manager;

use crate::host::{Host, Reply};

/// The popover's surface id and kind.
pub const SURFACE: &str = "companion";

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// What the companion keeps between ticks (C#'s `companion*` fields).
#[derive(Default)]
pub struct State {
    pub prefs: CompanionPreferences,
    pub pets: Vec<CompanionPet>,
    /// The pet the window shows (C# `companionLoadedPet`).
    pub loaded: Option<String>,
    pub error: Option<String>,
    follower: Follower,
    /// Where the user is in each question the bubble answers itself, by request key, with the input it was made for.
    drafts: HashMap<String, (String, QuestionnaireProgress)>,
    /// The requests seen waiting, oldest first (C#'s `companionPermissions`).
    waiting: follow::Waiting,
    /// Whether the OS animates (Windows' "Animation effects"): off, the pet stands still.
    system_motion: bool,
    /// What went wrong answering the request the bubble shows, by that request's key.
    request_error: Option<(String, String)>,
    /// The plan whose answer is on its way (its request's key).
    plan_sending: Option<String>,
    card_key: Option<String>,
    presented: Option<ToolPermissionRequest>,
    shown: Option<String>,
    /// The busy Claude panes the status bar counts.
    busy: usize,
    theme: String,
    row: i32,
}

/// The pet's window and its overlay while it stands.
struct Overlay {
    window: Arc<dyn PetWindow>,
    companion: Companion,
    last: Option<PetScene>,
    shown: bool,
    generation: u64,
}

/// The companion of the app (Tauri state).
pub struct CompanionApp {
    state: Mutex<State>,
    overlay: Mutex<Option<Overlay>>,
    surfaces: Mutex<Option<Arc<dyn SurfaceFactory>>>,
    generation: AtomicU64,
    clock: Instant,
    wake: Arc<tokio::sync::Notify>,
    closing: AtomicBool,
}

impl CompanionApp {
    fn now(&self) -> f64 {
        self.clock.elapsed().as_secs_f64()
    }

    /// Wakes the companion clock task after a preference change or shutdown.
    pub fn notify(&self) {
        self.wake.notify_waiters();
    }

    /// The companion's state (never held while the view's lock is taken).
    pub fn state(&self) -> MutexGuard<'_, State> {
        lock(&self.state)
    }

    /// Saves the settings where the app may write.
    pub fn save(&self, host: &Host) {
        save(host, self);
    }

    /// The main window's native views (the smoke opens its own test overlay through them).
    #[cfg(feature = "dev-harness")]
    pub fn surfaces(&self) -> Option<Arc<dyn SurfaceFactory>> {
        lock(&self.surfaces).clone()
    }

    /// The task setting switched: the bubble follows it (C#'s `companionBubble = on`).
    pub fn set_shows_task(&self, on: bool) {
        lock(&self.state).follower.set_bubble(on);
    }
}

/// The app's companion, once started.
pub fn app(host: &Host) -> Option<Arc<CompanionApp>> {
    host.app.try_state::<Arc<CompanionApp>>().map(|state| state.inner().clone())
}

/// The bundled pets (C#'s `Assets\pets`): beside the exe when they were put there, else in the app's resources, else — a development
/// build only — the repository's own.
pub fn bundled_pets(host: &Host) -> PathBuf {
    let beside = std::env::current_exe().ok().and_then(|exe| exe.parent().map(|dir| dir.join("pets")));
    let resources = host.app.path().resource_dir().ok().map(|dir| dir.join("pets"));
    #[cfg(debug_assertions)]
    let repository = Some(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("..").join("assets").join("pets"));
    #[cfg(not(debug_assertions))]
    let repository: Option<PathBuf> = None;
    [beside, resources, repository]
        .into_iter()
        .flatten()
        .find(|dir| dir.join(mighty_companion::pet::DEFAULT_PET).is_dir())
        .unwrap_or_default()
}

/// Codex's pets: a smoke reads its own folder (C# `smoke-codex`), never the user's.
fn codex_home(host: &Host) -> Option<PathBuf> {
    host.smoke.as_ref().map(|_| host.data_dir.join("smoke-codex"))
}

/// Whether this launch may put the pet on the desktop (plan §4: `--allow-state-write`, never in a smoke).
pub fn may_show(host: &Host) -> bool {
    host.may_change_machine()
}

/// Parks while the companion pet is disabled (`!prefs.enabled`), waking immediately on prefs/toggle change or shutdown.
/// Registers the `Notify` waiter via `enable()` before re-checking to eliminate lost wakeups.
async fn wait_until_enabled(companion: &CompanionApp, host_closing: impl Fn() -> bool) -> bool {
    loop {
        if host_closing() || companion.closing.load(Ordering::SeqCst) {
            return false;
        }
        if lock(&companion.state).prefs.enabled {
            return true;
        }
        let notified = companion.wake.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if host_closing() || companion.closing.load(Ordering::SeqCst) {
            return false;
        }
        if lock(&companion.state).prefs.enabled {
            return true;
        }
        notified.await;
    }
}

/// The companion's start with the app, once the main window's native views exist (its pet window is one): the settings read, the
/// catalog read, the pet shown where it may be, and the 1/12 s clock (parked whenever the pet is disabled).
pub fn start(host: &Arc<Host>, platform: &Platform, surfaces: Arc<dyn SurfaceFactory>) {
    let _ = platform;
    let preferences = prefs::load(&host.data_dir);
    let companion = Arc::new(CompanionApp {
        state: Mutex::new(State {
            follower: Follower::new(preferences.shows_task),
            prefs: preferences,
            system_motion: true,
            ..State::default()
        }),
        overlay: Mutex::new(None),
        surfaces: Mutex::new(Some(surfaces)),
        generation: AtomicU64::new(0),
        clock: Instant::now(),
        wake: Arc::new(tokio::sync::Notify::new()),
        closing: AtomicBool::new(false),
    });
    host.app.manage(companion.clone());
    let host = host.clone();
    tauri::async_runtime::spawn(async move {
        reload(&host).await;
        // The sprite plays at 12 frames a second; what the bubble says is read once a second.
        let mut clock = tokio::time::interval(Duration::from_secs_f64(1.0 / 12.0));
        // A late tick is skipped, not made up for in a burst (review L5).
        clock.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut ticks = 0u64;
        let mut was_parked = false;
        loop {
            if !lock(&companion.state).prefs.enabled {
                was_parked = true;
                if !wait_until_enabled(&companion, || host.closing()).await {
                    break;
                }
            }
            if was_parked {
                was_parked = false;
                clock.reset();
                ticks = 0;
            }
            tokio::select! {
                _ = clock.tick() => {}
                _ = companion.wake.notified() => {
                    if host.closing() || companion.closing.load(Ordering::SeqCst) {
                        break;
                    }
                    continue;
                }
            }
            if host.closing() || companion.closing.load(Ordering::SeqCst) {
                break;
            }
            if !lock(&companion.state).prefs.enabled {
                continue;
            }
            ticks += 1;
            // A tick that fails takes the pet down with what it holds and says why (C#'s catch in the timer, review L4).
            guarded(&companion, || {
                let theme = host.view().snapshot.theme().to_string();
                let themed = std::mem::replace(&mut lock(&companion.state).theme, theme.clone()) != theme;
                if ticks.is_multiple_of(12) || themed {
                    let motion = lock(&companion.overlay).as_ref().is_none_or(|o| o.window.animations());
                    lock(&companion.state).system_motion = motion;
                    refresh(&host);
                }
                let enabled = lock(&companion.state).prefs.enabled;
                if enabled {
                    draw(&host);
                }
            });
        }
    });
}

/// One tick of the clock: a panic in it ends the pet (its window closed) and its words become the box's message.
fn guarded(companion: &CompanionApp, tick: impl FnOnce()) {
    if let Err(panic) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(tick)) {
        lock(&companion.state).error = Some(crate::host::panic_text(panic.as_ref()));
        close_overlay(companion);
    }
}

/// The app quits: the pet window closed and the clock stopped (C# `ShutdownCompanion`).
pub fn shutdown(host: &Host) {
    let Some(companion) = app(host) else { return };
    companion.closing.store(true, Ordering::SeqCst);
    companion.generation.fetch_add(1, Ordering::SeqCst);
    companion.notify();
    close_overlay(&companion);
}

fn close_overlay(companion: &CompanionApp) {
    let taken = lock(&companion.overlay).take();
    if let Some(overlay) = taken {
        let _ = overlay.window.close();
    }
}

/// Saves the settings where the app may write (C# `SaveCompanionPreferences`); what went wrong is the box's message.
fn save(host: &Host, companion: &CompanionApp) {
    if !host.writes() {
        return;
    }
    let preferences = lock(&companion.state).prefs.clone();
    if let Err(error) = prefs::save(&host.data_dir, &preferences) {
        lock(&companion.state).error = Some(error.to_string());
    }
}

/// C# `ReloadCompanionPets`: the catalog read again, the window taken down and, where the pet may show, made again with the chosen
/// pet's atlas (the paw without one) and shown with its first picture drawn.
pub async fn reload(host: &Arc<Host>) {
    let Some(companion) = app(host) else { return };
    let generation = companion.generation.fetch_add(1, Ordering::SeqCst) + 1;
    let (bundled, profile, codex) = (bundled_pets(host), host.data_dir.clone(), codex_home(host));
    let catalog =
        tauri::async_runtime::spawn_blocking(move || CompanionPet::catalog(&bundled, &profile, codex.as_deref())).await.unwrap_or_default();
    if generation != companion.generation.load(Ordering::SeqCst) || host.closing() {
        return;
    }
    close_overlay(&companion);
    let (enabled, selected) = {
        let mut state = lock(&companion.state);
        state.pets = catalog;
        state.loaded = None;
        let selected = state.pets.iter().find(|p| p.id == state.prefs.selected_pet).or_else(|| state.pets.first()).cloned();
        (state.prefs.enabled, selected)
    };
    companion.notify();
    host.shell_dirty();
    if !enabled || !may_show(host) {
        return;
    }
    // Without a pet image the window still stands, with the paw in its place.
    let mut failure = selected.is_none().then(|| mighty_run::strings::get("companion.error.noPet"));
    let mut atlas: Option<PetBitmap> = None;
    let mut loaded = None;
    if let Some(selected) = selected {
        let decoded = tauri::async_runtime::spawn_blocking(move || {
            CompanionPet::load(&selected.source, &selected.id).and_then(|pet| pet.decode().map(|a| (pet.id, a)))
        })
        .await
        .unwrap_or_else(|e| Err(e.to_string()));
        match decoded {
            Ok((id, bitmap)) => {
                atlas = Some(bitmap);
                loaded = Some(id);
            }
            Err(error) => failure = Some(error),
        }
    }
    let Some(surfaces) = lock(&companion.surfaces).clone() else { return };
    let opened = surfaces.open_pet().await;
    if generation != companion.generation.load(Ordering::SeqCst) || host.closing() || !lock(&companion.state).prefs.enabled {
        if let Ok(window) = opened {
            let _ = window.close();
        }
        return;
    }
    let window: Arc<dyn PetWindow> = match opened {
        Ok(window) => Arc::from(window),
        Err(error) => {
            lock(&companion.state).error = Some(error.to_string());
            return;
        }
    };
    let events = window.events();
    let preferences = lock(&companion.state).prefs.clone();
    let mut overlay = Companion::new(window.clone(), &preferences);
    overlay.set_atlas(atlas);
    *lock(&companion.overlay) = Some(Overlay { window: window.clone(), companion: overlay, last: None, shown: false, generation });
    {
        let mut state = lock(&companion.state);
        state.loaded = loaded;
        state.error = failure;
    }
    refresh(host);
    // Its first picture before it shows: a layered window with none would take every click over its whole rectangle.
    draw(host);
    match events {
        Ok(mut events) => {
            let host = host.clone();
            tauri::async_runtime::spawn(async move {
                while let Some(event) = events.recv().await {
                    let action = {
                        let Some(companion) = app(&host) else { break };
                        let mut overlay = lock(&companion.overlay);
                        let Some(standing) = overlay.as_mut().filter(|o| o.generation == generation) else { break };
                        let action = standing.companion.act(&event);
                        present(standing);
                        action
                    };
                    if let Some(action) = action {
                        let host = host.clone();
                        tauri::async_runtime::spawn(async move { act(&host, action).await });
                    }
                }
            });
        }
        Err(error) => lock(&companion.state).error = Some(error.to_string()),
    }
}

/// The overlay's picture now: shown with its first frame, then drawn whenever it changed.
fn present(overlay: &mut Overlay) {
    let scene = overlay.companion.scene();
    if overlay.last.as_ref() == Some(&scene) && overlay.shown {
        return;
    }
    let result = if overlay.shown { overlay.window.draw(&scene) } else { overlay.window.show(&scene) };
    if result.is_ok() {
        overlay.shown = true;
        overlay.last = Some(scene);
    }
}

/// One frame of the pet as it now stands (C# `DrawCompanion`).
fn draw(host: &Host) {
    let Some(companion) = app(host) else { return };
    let (row, still) = {
        let state = lock(&companion.state);
        (state.row, state.prefs.reduced_motion || !state.system_motion)
    };
    let elapsed = lock(&companion.state).follower.animation_elapsed(companion.now());
    let mut overlay = lock(&companion.overlay);
    let Some(standing) = overlay.as_mut() else { return };
    standing.companion.set_frame(row, mighty_companion::pet::frame(row, elapsed, still), still);
    standing.companion.tick();
    present(standing);
}

/// C# `CompanionStatus`: a pane waits while a request of its waits on the user, while its activity waits, or while it is alive only for
/// the notices its request awaits.
fn status_of(session: &RunSession, requests: &[ToolPermissionRequest]) -> String {
    let waits = !requests.is_empty()
        || session.current_activity.as_ref().is_some_and(|a| a.state == "waiting")
        || mighty_graph::notice::awaits_notice(session);
    if waits { "waiting".into() } else { session.status().into() }
}

fn busy(status: &str) -> bool {
    matches!(status, "running" | "waiting" | "starting" | "queued")
}

/// Each Claude pane's waiting requests (oldest first), as the run service holds them.
fn requests_of(host: &Host, snapshot: &AppSnapshot) -> HashMap<String, Vec<ToolPermissionRequest>> {
    let Some(runs) = host.runs() else { return HashMap::new() };
    snapshot
        .sessions()
        .iter()
        .filter(|s| s.kind() == "claude" && matches!(s.status(), "running" | "waiting" | "starting"))
        .map(|s| {
            (s.id().to_string(), runs.service.pending_permissions(s.id()).into_iter().filter(|r| r.state == "pending").collect::<Vec<_>>())
        })
        .filter(|(_, requests)| !requests.is_empty())
        .collect()
}

/// C# `RefreshCompanion`: the agent followed, its card and whether its bubble shows, and the status bar's count.
pub fn refresh(host: &Host) {
    let Some(companion) = app(host) else { return };
    let snapshot = host.view().snapshot.clone();
    let requests = requests_of(host, &snapshot);
    let agents: Vec<&Arc<RunSession>> = snapshot.sessions().iter().filter(|s| s.kind() == "claude").collect();
    let none: Vec<ToolPermissionRequest> = vec![];
    let statuses: Vec<String> = agents.iter().map(|s| status_of(s, requests.get(s.id()).unwrap_or(&none))).collect();
    let now = companion.now();
    let standing = lock(&companion.overlay).is_some();
    let (card, visible, progress_pane) = {
        let mut state = lock(&companion.state);
        let next_busy = statuses.iter().filter(|s| busy(s)).count();
        if state.busy != next_busy {
            state.busy = next_busy;
            host.shell_dirty();
        }
        // The drafts and the error of requests seen waiting that wait no longer are let go (C# `ReceiveCompanionPermission`); a draft of
        // a request the companion never saw waiting (the smoke's own) is the caller's to let go.
        // New ones are taken in the panes' order, each pane's oldest first (review M4).
        let mut listed: Vec<(String, String)> = Vec::new();
        for session in &agents {
            for request in requests.get(session.id()).into_iter().flatten() {
                listed.push((follow::permission_key(&request.run_id, &request.id), request.run_id.clone()));
            }
        }
        for key in state.waiting.observe(&listed) {
            state.drafts.remove(&key);
            if state.request_error.as_ref().is_some_and(|(k, _)| *k == key) {
                state.request_error = None;
            }
            // An answered plan stays marked until it left the panes' requests (review L7: a remote pane's answer takes a round trip).
            if state.plan_sending.as_deref() == Some(key.as_str()) {
                state.plan_sending = None;
            }
        }
        // With no pet on the desktop nothing more is decided (C# MainWindow.Companion.cs:145): the count above is the status bar's.
        if !standing || !state.prefs.enabled {
            return;
        }
        let followed_agents: Vec<Agent> = agents
            .iter()
            .zip(&statuses)
            .map(|(s, status)| Agent {
                id: s.id().to_string(),
                status: status.clone(),
                activity: s.current_activity.clone(),
                started: s.run_timing.map(|t| t.started_at.utc_ticks()),
                last_observed: s.run_timing.map(|t| t.last_observed_at.utc_ticks()),
            })
            .collect();
        let shows_task = state.prefs.shows_task;
        let followed = state.follower.refresh(&followed_agents, shows_task, now);
        state.row = followed.row;
        state.shown = followed.shown.clone();
        let current = followed.shown.as_deref().and_then(|id| snapshot.session(id)).cloned();
        let permission = current.as_ref().and_then(|c| requests.get(c.id())).and_then(|r| r.first()).cloned();
        let key = follow::card_key(
            permission.as_ref().map(|p| (p.run_id.as_str(), p.id.as_str(), p.tool_name.as_str(), p.input_json.as_str())),
            followed.shown.as_deref(),
        );
        state.card_key = Some(key.clone());
        state.presented = permission.clone();
        let pending = requests.keys().any(|run| Some(run.as_str()) != followed.shown.as_deref() && followed.active.contains(run));
        let card = card(
            host,
            &mut state,
            &snapshot,
            key,
            current.as_deref(),
            &followed.status,
            permission.as_ref(),
            followed.position,
            followed.count,
            pending,
        );
        let visible = current.is_some() && (permission.is_some() || followed.bubble && shows_task);
        let progress_pane = current.filter(|_| permission.is_none() && followed.bubble && shows_task);
        (CompanionCard { progress_collapsed: state.prefs.progress_folded(), ..card }, visible, progress_pane)
    };
    // The progress card over the task bubble: the newest request of the pane it shows, read with the state's lock let go.
    let card = CompanionCard { progress: progress_pane.and_then(|pane| progress_of(host, &pane)), ..card };
    let mut overlay = lock(&companion.overlay);
    if let Some(standing) = overlay.as_mut() {
        standing.companion.set_card(Some(card), visible);
    }
}

/// The progress card of a pane (the Mac's `shownProgress`): its newest request's steps and background work as the page's progress
/// panel has them, numbered as the page numbers it (its older history's requests counted); None for a request with neither.
fn progress_of(host: &Host, pane: &Arc<RunSession>) -> Option<CompanionProgress> {
    let earlier = {
        let view = host.view();
        let (older, _) = crate::history::older(host, &view, pane);
        older.iter().filter(|run| !run.is_notice()).count()
    };
    CompanionProgress::newest(pane.graph_runs(), earlier, mighty_core::time::DateTimeOffset::now(), &host.locale)
}

/// C# `CompanionCard`: the workspace over the title, the state word and clock, the request and the work; or the request waiting on the
/// user in their place.
#[allow(clippy::too_many_arguments, reason = "C#'s CompanionCard takes these")]
fn card(
    host: &Host,
    state: &mut State,
    snapshot: &AppSnapshot,
    key: String,
    current: Option<&RunSession>,
    status: &str,
    permission: Option<&ToolPermissionRequest>,
    position: usize,
    count: usize,
    pending: bool,
) -> CompanionCard {
    let locale = &host.locale;
    let workspace = current.and_then(|c| snapshot.workspace(c.workspace_id())).map(|w| w.name().to_string()).unwrap_or_default();
    // The bubble's own word for waiting; every other state takes the pane's.
    let awaiting = status == "waiting"
        && permission.is_none()
        && current
            .is_some_and(|c| c.current_activity.as_ref().is_none_or(|a| a.state != "waiting") && mighty_graph::notice::awaits_notice(c));
    let state_word = if awaiting {
        mighty_graph::notice::state_word(locale)
    } else if status == "waiting" {
        locale.get("graph.state.waiting")
    } else {
        mighty_composer::runtime::state_label(status, locale)
    };
    let work = if awaiting {
        locale.get("companion.status.awaitingNotice")
    } else if let Some(summary) = current.and_then(|c| c.current_activity.as_ref()).map(|a| a.summary.clone()).filter(|s| !s.is_empty()) {
        summary
    } else {
        locale.get(match status {
            "running" => "companion.status.running",
            "waiting" => "companion.status.waiting",
            "completed" => "companion.status.completed",
            "error" => "companion.status.error",
            "stopped" => "companion.status.stopped",
            _ => "companion.status.idle",
        })
    };
    // One line of the last request, as the Mac keeps it.
    let request = current
        .and_then(|c| c.logs().iter().rev().find(|l| l.kind.as_deref() == Some("user")))
        .map(|l| mighty_run::activity::clean(l.text.as_deref(), 2048, true));
    let title = current.map(|c| c.title().to_string()).unwrap_or_default();
    let origin = if workspace.is_empty() { title.clone() } else { format!("{workspace} · {title}") };
    CompanionCard {
        key,
        light: snapshot.theme() == "light",
        status: status.into(),
        workspace,
        title,
        state: state_word,
        elapsed: current.and_then(|c| c.run_timing).map(|t| mighty_run::timing::label(&t, None)),
        request,
        work,
        position,
        count,
        pending,
        approval: permission.map(|p| request_card(host, state, p, origin)),
        progress: None,
        progress_collapsed: false,
    }
}

fn draft<'a>(state: &'a mut State, request: &ToolPermissionRequest) -> &'a mut QuestionnaireProgress {
    let key = follow::permission_key(&request.run_id, &request.id);
    let entry = state.drafts.entry(key.clone()).or_insert_with(|| (request.input_json.clone(), QuestionnaireProgress::new(&key)));
    if entry.0 != request.input_json {
        *entry = (request.input_json.clone(), QuestionnaireProgress::new(&key));
    }
    &mut entry.1
}

/// The card the app makes for a pane (C# `CompanionCard` as the smoke calls it: its own overlay shows the app's cards).
#[cfg(feature = "dev-harness")]
#[allow(clippy::too_many_arguments, reason = "C#'s CompanionCard takes these")]
pub fn card_for(
    host: &Host,
    key: &str,
    session: &RunSession,
    status: &str,
    permission: Option<&ToolPermissionRequest>,
    position: usize,
    count: usize,
    pending: bool,
) -> CompanionCard {
    let snapshot = host.view().snapshot.clone();
    let Some(companion) = app(host) else { return CompanionCard::default() };
    let mut state = lock(&companion.state);
    card(host, &mut state, &snapshot, key.to_string(), Some(session), status, permission, position, count, pending)
}

/// A choice tapped in a question's draft, as the bubble taps it (C#'s smoke: `CompanionBubble.Choose(label, form, draft)`).
#[cfg(feature = "dev-harness")]
pub fn choose_for(host: &Host, request: &ToolPermissionRequest, label: &str) {
    let Some(companion) = app(host) else { return };
    let Some(questionnaire) = UserQuestionnaire::parse(&request.input_json) else { return };
    let mut state = lock(&companion.state);
    bubble::choose(label, &questionnaire, draft(&mut state, request));
}

/// A request's draft let go (the smoke's `finally`).
#[cfg(feature = "dev-harness")]
pub fn forget_draft(host: &Host, request: &ToolPermissionRequest) {
    if let Some(companion) = app(host) {
        lock(&companion.state).drafts.remove(&follow::permission_key(&request.run_id, &request.id));
    }
}

/// C# `CompanionRequest`: the approval, the question or the finished plan, with the bubble's buttons for it.
fn request_card(host: &Host, state: &mut State, permission: &ToolPermissionRequest, origin: String) -> CompanionRequest {
    let key = follow::permission_key(&permission.run_id, &permission.id);
    let error = state.request_error.as_ref().filter(|(k, _)| *k == key).map(|(_, m)| m.clone());
    if permission.can_answer_plan
        && let Some(plan) = permission.plan()
    {
        // A finished plan: when it came, the whole plan and its answers; every button off while an answer is on its way.
        let sending = state.plan_sending.as_deref() == Some(key.as_str());
        let offset = chrono::Local::now().offset().local_minus_utc() / 60;
        return CompanionRequest {
            title: host.locale.get("plan.card.title"),
            origin,
            buttons: bubble::plan(sending),
            error,
            plan: Some(plan),
            received: permission
                .received_at
                .as_deref()
                .map(|at| mighty_composer::presentation::plan::received_text(at, offset, &host.locale)),
            busy: sending,
            ..CompanionRequest::default()
        };
    }
    if permission.can_answer_questions
        && let Some(questionnaire) = UserQuestionnaire::parse(&permission.input_json)
    {
        // One question at a time, its choices answered in the bubble.
        let progress = draft(state, permission).clone();
        let question = progress.current(&questionnaire).unwrap_or_else(|| questionnaire.questions.last().expect("a question")).clone();
        return CompanionRequest {
            question: true,
            title: bubble::question_title(&questionnaire, &progress),
            origin,
            headline: Some(question.question.clone()),
            multi_select: question.multi_select,
            options: question
                .options
                .iter()
                .map(|o| CompanionOption {
                    label: o.label.clone(),
                    description: o.description.clone(),
                    picked: bubble::picked(&o.label, &questionnaire, &progress),
                })
                .collect(),
            buttons: bubble::question(&questionnaire, &progress),
            error,
            ..CompanionRequest::default()
        };
    }
    let presentation = ToolPermissionPresentation::make(&permission.tool_name, &permission.input_json);
    let code = presentation.fields.iter().find(|f| f.code).map(|f| f.value.clone());
    CompanionRequest {
        title: host.locale.get("companion.approval.title"),
        origin,
        detail: format!("{} · {}", presentation.title, permission.tool_name),
        code: code.clone().or_else(|| presentation.headline.is_none().then(|| permission.summary.clone())),
        headline: presentation.headline,
        boxed: code.is_some(),
        buttons: bubble::approval(permission.can_allow),
        error,
        ..CompanionRequest::default()
    }
}

/// C# `CompanionAction` and the overlay's `Moved` and `Resized`: what a press asks for.
async fn act(host: &Arc<Host>, action: CompanionAction) {
    let Some(companion) = app(host) else { return };
    match action {
        CompanionAction::Moved { left, top } => {
            {
                let mut state = lock(&companion.state);
                state.prefs.left = Some(left);
                state.prefs.top = Some(top);
            }
            save(host, &companion);
        }
        CompanionAction::Resized { width, height, left, top } => {
            {
                let mut state = lock(&companion.state);
                state.prefs.bubble_width = width;
                state.prefs.bubble_height = height;
                state.prefs.left = Some(left);
                state.prefs.top = Some(top);
            }
            save(host, &companion);
        }
        CompanionAction::Failed { message } => {
            lock(&companion.state).error = Some(message);
            close_overlay(&companion);
        }
        CompanionAction::Act { card, action } => press(host, &companion, &card, &action).await,
    }
}

/// The agent shown, read with the state's lock let go before `then` runs.
fn with_shown(companion: &CompanionApp, then: impl FnOnce(Option<String>)) {
    let shown = lock(&companion.state).shown.clone();
    then(shown);
}

async fn press(host: &Arc<Host>, companion: &Arc<CompanionApp>, key: &str, action: &str) {
    if host.closing() || lock(&companion.state).card_key.as_deref() != Some(key) {
        return;
    }
    match action {
        "hide" => {
            lock(&companion.state).prefs.enabled = false;
            companion.notify();
            host.shell_dirty();
            save(host, companion);
            close_overlay(companion);
            return;
        }
        "toggle" => {
            let turned_on = {
                let mut state = lock(&companion.state);
                let shows_task = state.prefs.shows_task;
                let on = state.follower.toggle(shows_task);
                if on {
                    state.prefs.shows_task = true;
                }
                on
            };
            if turned_on {
                save(host, companion);
            }
            refresh(host);
            return;
        }
        PROGRESS_FOLD => {
            // The progress card folds to its heading or opens again, and stays so between launches.
            {
                let mut state = lock(&companion.state);
                let folded = state.prefs.progress_folded();
                state.prefs.progress_collapsed = Some(!folded);
            }
            save(host, companion);
            refresh(host);
            return;
        }
        "pending" => {
            {
                let mut state = lock(&companion.state);
                let requesting = state.waiting.panes();
                state.follower.pin_pending(&requesting);
            }
            refresh(host);
            return;
        }
        "previous" | "next" => {
            let snapshot = host.view().snapshot.clone();
            let requests = requests_of(host, &snapshot);
            let pages: Vec<String> = snapshot
                .sessions()
                .iter()
                .filter(|s| s.kind() == "claude" && (busy(s.status()) || requests.contains_key(s.id())))
                .map(|s| s.id().to_string())
                .collect();
            lock(&companion.state).follower.page(&pages, if action == "next" { 1 } else { -1 });
            refresh(host);
            return;
        }
        "open" => {
            // With no agent to turn to, the menu's open still brings the window forward. The state's lock is let go before the
            // window is asked (review H2: focusing takes the view's lock, which the shell clock holds while it reads the state's).
            with_shown(companion, |shown| match shown {
                Some(shown) => crate::notifications::focus(host, &shown),
                None => {
                    if let Some(window) = host.app.get_webview_window(crate::host::WINDOW) {
                        let _ = window.show();
                        let _ = window.set_focus();
                    }
                }
            });
            return;
        }
        _ => {}
    }
    // The request the bubble shows, still the one waiting.
    let (presented, shown) = {
        let state = lock(&companion.state);
        (state.presented.clone(), state.shown.clone())
    };
    let Some(presented) = presented else { return };
    let live = host.runs().and_then(|r| r.service.pending_permissions(&presented.run_id).into_iter().find(|r| r.id == presented.id));
    let Some(request) = live.filter(|r| *r == presented && r.state == "pending" && Some(&r.run_id) == shown.as_ref()) else { return };
    // The whole request, an answer written out and the plan's own view are the pane's card (D10): the pane is brought forward.
    if action == bubble::CUSTOM && request.can_answer_questions
        || action == bubble::REVIEW && request.can_allow && !request.can_answer_questions
        || (action == bubble::PLAN_REVIEW || action == bubble::PLAN_REVISE) && request.can_answer_plan
    {
        crate::notifications::focus(host, &request.run_id);
        return;
    }
    if request.can_answer_plan && request.plan().is_some() {
        let decision = match action {
            bubble::PLAN_CANCEL => PlanDecision::cancel(),
            bubble::PLAN_APPROVE_CONFIRM => PlanDecision::approve_confirm_each(),
            bubble::PLAN_APPROVE => PlanDecision::approve_auto_edit(),
            _ => return,
        };
        answer_plan(host, companion, &request, decision).await;
        return;
    }
    if request.can_answer_questions
        && let Some(questionnaire) = UserQuestionnaire::parse(&request.input_json)
        && answer_question(host, companion, &request, &questionnaire, action).await
    {
        return;
    }
    // Allow once and Deny from the bubble itself.
    if action != bubble::DENY && !(action == bubble::ALLOW && request.can_allow && !request.can_answer_questions) {
        return;
    }
    let Ok(runs) = host.engine() else { return };
    if let Err(error) = runs.service.respond_to_tool_permission(&request.run_id, &request.id, action == bubble::ALLOW).await {
        let mut state = lock(&companion.state);
        state.error = Some(error.message().to_string());
        state.request_error = Some((follow::permission_key(&request.run_id, &request.id), error.message().to_string()));
    }
    refresh(host);
}

/// C# `AnswerCompanionQuestion`: a choice answers a single-choice question and toggles in a multi-choice one, Back goes back, Next and
/// Send record what was picked; the answers go once every question has one. False for any other press.
pub async fn answer_question(
    host: &Arc<Host>,
    companion: &CompanionApp,
    request: &ToolPermissionRequest,
    questionnaire: &UserQuestionnaire,
    action: &str,
) -> bool {
    let key = follow::permission_key(&request.run_id, &request.id);
    let answers = {
        let mut state = lock(&companion.state);
        let progress = draft(&mut state, request);
        let answers = if let Some(index) = action.strip_prefix(bubble::CHOICE) {
            let Ok(index) = index.parse::<usize>() else { return true };
            let Some(label) = progress.current(questionnaire).and_then(|q| q.options.get(index)).map(|o| o.label.clone()) else {
                return true;
            };
            bubble::choose(&label, questionnaire, progress)
        } else if action == bubble::BACK {
            progress.back(questionnaire);
            None
        } else if action == bubble::NEXT {
            bubble::commit(questionnaire, progress)
        } else {
            return false;
        };
        if state.request_error.as_ref().is_some_and(|(k, _)| *k == key) {
            state.request_error = None;
        }
        answers
    };
    if let Some(answers) = answers
        && let Ok(runs) = host.engine()
        && let Err(error) = runs.service.answer_questionnaire(&request.run_id, &request.id, &answers).await
    {
        let mut state = lock(&companion.state);
        state.error = Some(error.message().to_string());
        state.request_error = Some((key, error.message().to_string()));
    }
    refresh(host);
    true
}

/// C# `AnswerCompanionPlan`: one answer at a time, every button off while it is on its way; an error stays on the request.
async fn answer_plan(host: &Arc<Host>, companion: &CompanionApp, request: &ToolPermissionRequest, decision: PlanDecision) {
    let key = follow::permission_key(&request.run_id, &request.id);
    {
        let mut state = lock(&companion.state);
        if state.plan_sending.is_some() {
            return;
        }
        state.plan_sending = Some(key.clone());
    }
    refresh(host);
    let answered = match host.engine() {
        Ok(runs) => {
            runs.service.answer_plan(&request.run_id, &request.id, &decision).await.map(|_| ()).map_err(|e| e.message().to_string())
        }
        Err(error) => Err(error),
    };
    if let Err(error) = &answered {
        let mut state = lock(&companion.state);
        state.plan_sending = None;
        state.error = Some(error.clone());
        state.request_error = Some((key, error.clone()));
    }
    host.view().mighty.dirty(&request.run_id);
    refresh(host);
}

/// C# `ToggleCompanionPet`: the pet switched on or off, saved; turned off it goes at once, and the catalog is read again behind it.
pub async fn toggle_pet(host: &Arc<Host>) {
    let Some(companion) = app(host) else { return };
    let enabled = {
        let mut state = lock(&companion.state);
        state.prefs.enabled = !state.prefs.enabled;
        state.prefs.enabled
    };
    companion.notify();
    host.shell_dirty();
    save(host, &companion);
    if !enabled {
        close_overlay(&companion);
    }
    reload(host).await;
    crate::native::touch(host, SURFACE);
}

/// `companionStatus` (the status bar's capsule): the agent status popover in the page; true when it opened.
pub async fn status(host: Arc<Host>, args: Value) -> Reply {
    let _ = args;
    if app(&host).is_none() {
        return Ok(Value::Null);
    }
    refresh(&host);
    crate::native::open(&host, SURFACE, Place::Popover, Box::new(StatusPopover { host: host.clone() }));
    Ok(Value::Bool(true))
}

/// `companionToggle` (the status bar's paw): `enabled` flipped, saved, the pet shown or hidden. Answers null.
pub async fn toggle(host: Arc<Host>, args: Value) -> Reply {
    let _ = args;
    toggle_pet(&host).await;
    Ok(Value::Null)
}

/// C# `WebCompanionControls` for the frame model, read with the state locked (never `host.view()`): the paw (on while the pet shows,
/// "Toggle pet" its name, "Hide pet" or "Show pet" its help) and the capsule (the waveform and the count while agents work).
pub fn frame_model(host: &Host, snapshot: &AppSnapshot) -> Option<CompanionFrame> {
    let companion = app(host)?;
    let (pet_on, busy_agents) = {
        let state = lock(&companion.state);
        let pet_on = state.prefs.enabled;
        let busy_agents = if pet_on {
            state.busy
        } else {
            let requests = requests_of(host, snapshot);
            let none: Vec<ToolPermissionRequest> = vec![];
            snapshot
                .sessions()
                .iter()
                .filter(|s| s.kind() == "claude")
                .filter(|s| busy(&status_of(s, requests.get(s.id()).unwrap_or(&none))))
                .count()
        };
        (pet_on, busy_agents)
    };
    let locale = &host.locale;
    let count = busy_agents.to_string();
    Some(CompanionFrame {
        pet_on,
        pet_label: locale.get("companion.status.togglePet"),
        pet_help: Some(locale.get(if pet_on { "menu.hidePet" } else { "companion.status.showPet" })),
        busy: busy_agents > 0,
        count: (busy_agents > 0).then(|| count.clone()),
        status_label: format!(
            "{} · {}",
            locale.get("companion.status.title"),
            locale.get_with("companion.status.busyCount", &[("count", &count)])
        ),
    })
}

/// The agent status popover (C# `BuildCompanionControls`' body, AgentStatusPopover): the title and the working count, the agents' cards,
/// a line, then the pet switch; 350 wide with its padding 18, 14 apart.
struct StatusPopover {
    host: Arc<Host>,
}

/// The popover's width over all, its padding and the height its list scrolls at.
const POPOVER_WIDTH: f64 = 350.0;
const POPOVER_PADDING: f64 = 18.0;
const POPOVER_LIST_HEIGHT: f64 = 330.0;

impl SurfaceModel for StatusPopover {
    fn view(&mut self, ui: &mut Ui) -> View {
        let host = &self.host;
        let locale = ui.locale();
        let snapshot = host.view().snapshot.clone();
        let requests = requests_of(host, &snapshot);
        let none: Vec<ToolPermissionRequest> = vec![];
        let sessions: Vec<&Arc<RunSession>> = snapshot.sessions().iter().filter(|s| s.kind() == "claude").collect();
        let busy_count = sessions.iter().filter(|s| busy(&status_of(s, requests.get(s.id()).unwrap_or(&none)))).count();
        let enabled = app(host).is_some_and(|c| lock(&c.state).prefs.enabled);
        let mut title = ui.text_block(locale.get("companion.status.title"), 14.0);
        if let Some(block) = title.text_mut() {
            block.weight = 600;
        }
        let count = ui.text_block(locale.get_with("companion.status.busyCount", &[("count", &busy_count.to_string())]), metrics::TYPE_PILL);
        let head = El::grid(Grid {
            children: vec![title.cell(0, 0), count.va(VAlign::Center).cell(0, 1)],
            cols: vec![Track::Star(1.0), Track::Auto],
            col_spacing: metrics::SPACING_SM,
            ..Grid::default()
        });
        let mut rows = vec![];
        for session in &sessions {
            let state = status_of(session, requests.get(session.id()).unwrap_or(&none));
            let awaiting = state == "waiting"
                && session.current_activity.as_ref().is_none_or(|a| a.state != "waiting")
                && !requests.contains_key(session.id())
                && mighty_graph::notice::awaits_notice(session);
            let word =
                if awaiting { mighty_graph::notice::state_word(&locale) } else { mighty_composer::runtime::state_label(&state, &locale) };
            let detail = snapshot.workspace(session.workspace_id()).map(|w| w.name().to_string()).unwrap_or_default();
            let elapsed = session.run_timing.map(|t| mighty_run::timing::label(&t, None)).unwrap_or_default();
            let prompt = mighty_core::wire::clean(
                session.logs().iter().rev().find(|l| l.kind.as_deref() == Some("user")).and_then(|l| l.text.as_deref()).unwrap_or(""),
                400,
            );
            let activity = mighty_core::wire::clean(
                &if awaiting {
                    locale.get("companion.status.awaitingNotice")
                } else {
                    session
                        .current_activity
                        .as_ref()
                        .map(|a| a.summary.clone())
                        .unwrap_or_else(|| mighty_composer::runtime::state_label(&state, &locale))
                },
                400,
            );
            let mut heading = ui.text_block(session.title(), 12.0);
            if let Some(block) = heading.text_mut() {
                block.weight = 500;
            }
            let mut words = vec![
                El::grid(Grid {
                    children: vec![heading.cell(0, 0), ui.text_block(word.clone(), 10.0).va(VAlign::Center).cell(0, 1)],
                    cols: vec![Track::Star(1.0), Track::Auto],
                    col_spacing: metrics::SPACING_SM,
                    ..Grid::default()
                }),
                El::grid(Grid {
                    children: vec![
                        ui.text_block(detail.clone(), 10.0).cell(0, 0),
                        ui.text_block(elapsed.clone(), 10.0).va(VAlign::Center).visible(!elapsed.is_empty()).cell(0, 1),
                    ],
                    cols: vec![Track::Star(1.0), Track::Auto],
                    col_spacing: metrics::SPACING_SM,
                    ..Grid::default()
                }),
            ];
            if !prompt.is_empty() {
                let mut text = ui.text_block(prompt.clone(), metrics::TYPE_PILL);
                if let Some(block) = text.text_mut() {
                    block.wrapping = Wrapping::Wrap;
                }
                words.push(text);
            }
            let mut doing = ui.text_block(activity.clone(), metrics::TYPE_PILL);
            if let Some(block) = doing.text_mut() {
                block.wrapping = Wrapping::Wrap;
                block.foreground = Some(ui.brush(Token::Ink2));
            }
            words.push(doing);
            let name = [
                format!("{} · {word}", session.title()),
                [detail, mighty_run::strings::provider_label(session.provider()).to_string(), elapsed]
                    .into_iter()
                    .filter(|t| !t.is_empty())
                    .collect::<Vec<_>>()
                    .join(" · "),
                prompt,
                activity,
            ]
            .into_iter()
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join(". ");
            let id = session.id().to_string();
            let to = host.clone();
            let content = El::stack(false, metrics::SPACING_XS, words);
            let row = El::border(Border {
                child: Some(Box::new(content)),
                background: Some(ui.theme().subtle()),
                padding: Thickness::new(metrics::SPACING_MD, metrics::SPACING_MD, metrics::SPACING_MD, metrics::SPACING_MD),
                corner_radius: 10.0,
                ..Border::default()
            })
            .named(name)
            .on_click(move || {
                let host = to.clone();
                let id = id.clone();
                tauri::async_runtime::spawn(async move {
                    crate::native::close(&host, SURFACE);
                    crate::notifications::focus(&host, &id);
                });
            })
            .ha(HAlign::Stretch);
            rows.push(row);
        }
        let mut body = vec![head];
        if sessions.is_empty() {
            let mut empty = ui.text_block(locale.get("companion.status.empty"), 12.0);
            if let Some(block) = empty.text_mut() {
                block.wrapping = Wrapping::Wrap;
            }
            body.push(empty.margin(Thickness::new(0.0, metrics::SPACING_LG, 0.0, metrics::SPACING_LG)));
        }
        body.push(El::stack(false, metrics::SPACING_SM, rows).max_height(POPOVER_LIST_HEIGHT));
        body.push(El::border(Border { background: Some(ui.brush(Token::Line)), ..Border::default() }).height(metrics::STROKE_LINE));
        let to = host.clone();
        let switch = ui.settings_switch(locale.get("companion.status.petSwitch"), enabled, Some("companion-status-pet"), move |on| {
            let host = to.clone();
            tauri::async_runtime::spawn(async move {
                if app(&host).is_some_and(|c| lock(&c.state).prefs.enabled != on) {
                    toggle_pet(&host).await;
                }
            });
        });
        body.push(El::stack(
            true,
            metrics::SPACING_SM,
            vec![ui.settings_text(locale.get("companion.status.petSwitch"), TextStyle::default()).va(VAlign::Center), switch],
        ));
        let body = El::stack(false, metrics::SPACING_MD, body).width(POPOVER_WIDTH - 2.0 * POPOVER_PADDING);
        View::Companion { title: locale.get("companion.status.title"), body: Some(body) }
    }

    fn button(&mut self, which: Option<&str>) -> Next {
        let _ = which;
        Next::Close
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use mighty_platform::fake::{FakePet, PetCall};

    use super::*;

    fn companion(pet: Option<FakePet>) -> CompanionApp {
        let overlay = pet.map(|pet| {
            let window: Arc<dyn PetWindow> = Arc::new(pet);
            Overlay {
                companion: Companion::new(window.clone(), &CompanionPreferences::default()),
                window,
                last: None,
                shown: false,
                generation: 1,
            }
        });
        CompanionApp {
            state: Mutex::new(State { shown: Some("pane".into()), system_motion: true, ..State::default() }),
            overlay: Mutex::new(overlay),
            surfaces: Mutex::new(None),
            generation: AtomicU64::new(1),
            clock: Instant::now(),
            wake: Arc::new(tokio::sync::Notify::new()),
            closing: AtomicBool::new(false),
        }
    }

    /// Review H2: what the open press does with the agent shown runs with the state's lock let go. The other thread is the shell
    /// clock: it holds the view's lock while it reads the state's (`frame_model`); the press's focus takes the view's.
    #[test]
    fn the_open_press_lets_the_state_go_before_it_focuses() {
        let companion = Arc::new(companion(None));
        let view = Arc::new(Mutex::new(()));
        let (done, finished) = mpsc::channel();
        let (held, holding) = mpsc::channel();
        let (clock_companion, clock_view, clock_done) = (companion.clone(), view.clone(), done.clone());
        let clock = std::thread::spawn(move || {
            let guard = lock(&clock_view);
            held.send(()).unwrap();
            std::thread::sleep(Duration::from_millis(100));
            let _state = lock(&clock_companion.state);
            drop(guard);
            clock_done.send("clock").unwrap();
        });
        holding.recv().unwrap();
        let (press_companion, press_view) = (companion.clone(), view.clone());
        let press = std::thread::spawn(move || {
            with_shown(&press_companion, |shown| {
                assert_eq!(shown.as_deref(), Some("pane"));
                let _view = lock(&press_view);
            });
            done.send("press").unwrap();
        });
        for _ in 0..2 {
            finished.recv_timeout(Duration::from_secs(5)).expect("the press and the clock deadlocked");
        }
        clock.join().unwrap();
        press.join().unwrap();
    }

    /// Review L4: a tick that panics closes the pet's window and says why; the next tick goes on.
    #[test]
    fn a_panicking_tick_takes_the_pet_down_and_says_why() {
        let pet = FakePet::default();
        let companion = companion(Some(pet.clone()));
        guarded(&companion, || panic!("the tick broke"));
        assert!(lock(&companion.overlay).is_none(), "the window is let go");
        assert_eq!(pet.calls().last(), Some(&PetCall::Closed));
        assert_eq!(lock(&companion.state).error.as_deref(), Some("the tick broke"));
        let mut ran = false;
        guarded(&companion, || ran = true);
        assert!(ran);
    }

    /// P1-PERF-1: when `!prefs.enabled`, the companion clock parks on `Notify` instead of polling at 12 Hz,
    /// waking immediately when enabled or when closing.
    #[tokio::test]
    async fn disabled_companion_parks_until_enabled_or_shutdown() {
        let companion = Arc::new(companion(None));
        lock(&companion.state).prefs.enabled = false;
        let waiter = companion.clone();
        let task = tokio::spawn(async move { wait_until_enabled(&waiter, || false).await });
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(!task.is_finished(), "disabled companion must remain parked");

        lock(&companion.state).prefs.enabled = true;
        companion.notify();
        assert!(tokio::time::timeout(Duration::from_millis(200), task).await.unwrap().unwrap(), "woke when enabled");

        lock(&companion.state).prefs.enabled = false;
        let waiter = companion.clone();
        let task = tokio::spawn(async move { wait_until_enabled(&waiter, || false).await });
        tokio::time::sleep(Duration::from_millis(20)).await;
        companion.closing.store(true, Ordering::SeqCst);
        companion.notify();
        assert!(!tokio::time::timeout(Duration::from_millis(200), task).await.unwrap().unwrap(), "exited on shutdown");
    }
}
