//! The companion step (C# MainWindow.Companion.cs `RunCompanionSmoke`, `RunCompanionDesignSmoke`, `CheckCompanionBubble`; live key
//! `companion`), `Stage::Companion` after the remote step. Owner: W8-E.
//!
//! Only its own test overlay (plan D9): a pet window opened through the main window's native views, never the app's own pet, never the
//! real profile's settings; NOACTIVATE, the front window compared around each step as C#'s `Kept()` does (the smoke shares its
//! desktop: another program may come forward while a picture is written), closed in `finally`. Its presses go through the window's own
//! procedure (C#'s `SendMessage`), the overlay acts on the events the window sends, and every picture is the window's own buffer,
//! never the desktop. The cards are the app's (companion.rs `card_for`), drawn with the window's DirectWrite measure, and measured as C#
//! measures them: the first place, the window and the bubble, each text's size, weight, ink and padding, the card's colour, its
//! corners and hairline, the gap above the pet, the resize strips and their anchors, the requests in the taller window and their
//! buttons, a long plan's scroll, the paw, and the window at 150% and 200%. The status bar's popover is opened in the page and closed
//! with Esc (C#'s dialogs smoke, `checks["companion"]`).
//!
//! On the Mac (M-F) the step also reads back the NSPanel and the front app, and checks the menu bar, a deep link and the completion
//! notification (`MAC_KEYS`, under the report's `mac`); Windows' report and checks do not change.
//!
//! The review, written-answer and plan buttons focus the pane's own card (D10): C#'s two extra windows (`ShowCompanionApproval`,
//! `ShowCompanionPlan`) have no page counterpart, so their checks are not here.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mighty_companion::card::CompanionCard;
use mighty_companion::layout::{self, Edges};
use mighty_companion::pet::{self, CompanionPet};
use mighty_companion::prefs::CompanionPreferences;
use mighty_companion::scene::PetText;
use mighty_companion::{Companion, CompanionAction, bubble};
use mighty_core::design_tokens::{DARK, LIGHT, Token};
use mighty_core::model::RunSession;
use mighty_core::permission::ToolPermissionRequest;
use mighty_core::time::DateTimeOffset;
use mighty_core::wire;
use mighty_platform::surface::{PetCapture, PetFocus, PetInput};
use mighty_platform::{LogicalPoint, PetWindow};
use mighty_run::questionnaire::UserQuestionnaire;
use serde_json::{Map, Value, json};
use tauri::Manager;

use crate::host::Host;

/// The report's keys (C#'s `companion` dictionary), each true when its part held.
const KEYS: [&str; 9] = [
    "bundledAtlasDecoded",
    "allAnimationRowsRendered",
    "nonActivatingWindow",
    "pointerActionBoundToCard",
    "selectedPetPreview",
    "contextMenuNonActivating",
    "contentDrivenHeightAndEdgeResize",
    "planBubbleAndWindow",
    "renderedOverlaySnapshots",
];

/// Mac stage (M-F, plan §3 M-F's smoke keys): the menu bar read back through `NSApp.mainMenu` (every item of the app's own with its
/// words and ⌘ keys, the Workspace menu, the Edit menu's copy and paste), ⌘K, ⌘T, ⌘, and ⌘= pressed through it as AppKit sends a key
/// equivalent no view took (`performKeyEquivalent:`) reaching their items, a `mightycode://open` link delivered in process (the draft
/// set, nothing sent, the window left where it was), the completion notification as the smoke's notifier gets it (its click names the
/// pane), the pet's NSPanel read back as Swift's `CompanionPanel` sets it, and the front app unchanged across the step.
const MAC_KEYS: [&str; 6] =
    ["menuInstalled", "menuKeyEquivalents", "deepLinkOpened", "notificationRequested", "companionPanel", "frontAppUnchanged"];

/// The Mac's keys where the app has a menu bar (the Mac), none elsewhere.
fn mac_keys() -> &'static [&'static str] {
    if crate::platform::has_menu_bar() { &MAC_KEYS } else { &[] }
}

/// The report's `mac` object, made on first use.
fn mac_report(checks: &mut Map<String, Value>) -> &mut Map<String, Value> {
    let entry = checks.entry("mac").or_insert_with(|| json!({}));
    if !entry.is_object() {
        *entry = json!({});
    }
    entry.as_object_mut().expect("made an object above")
}

/// Runs `work` on the main thread (AppKit's) and waits for it, at most 5 s.
async fn on_main<T: Send + 'static>(host: &Host, work: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (answer, answered) = tokio::sync::oneshot::channel();
    host.app
        .run_on_main_thread(move || {
            let _ = answer.send(work());
        })
        .ok()?;
    tokio::time::timeout(Duration::from_secs(5), answered).await.ok()?.ok()
}

/// The menu, the deep link and the notification (the pet's panel and the front app are read in [`run`], while the pet is open).
/// Each part bounded and the whole within [`MAC_LIMIT`] (review item 4: the Mac smoke runs near its own limit).
async fn mac_checks(host: &Arc<Host>, checks: &mut Map<String, Value>) {
    let bounded = async {
        let menu = bound(MENU_LIMIT, "the menu bar", menu_installed(host)).await;
        let keys = bound(MENU_LIMIT, "the menu's keys", menu_keys(host)).await;
        let link = bound(LINK_LIMIT, "the deep link", deep_link(host)).await;
        let notification = notification_requested(host, link.as_ref().ok().and_then(|p| p.as_deref())).await;
        (menu, keys, link, notification)
    };
    let (menu, keys, link, notification) = match tokio::time::timeout(MAC_LIMIT, bounded).await {
        Ok(outcomes) => outcomes,
        Err(_) => {
            let late = || Err::<bool, String>(format!("the Mac checks took over {} s", MAC_LIMIT.as_secs()));
            crate::menu::set_recorder(None);
            (late(), late(), Err(format!("the Mac checks took over {} s", MAC_LIMIT.as_secs())), late())
        }
    };
    let report = mac_report(checks);
    let outcomes = [
        ("menuInstalled", menu),
        ("menuKeyEquivalents", keys),
        ("deepLinkOpened", link.map(|pane| pane.is_some())),
        ("notificationRequested", notification),
    ];
    for (key, outcome) in outcomes {
        match outcome {
            Ok(held) => {
                report.insert(key.into(), json!(held));
            }
            Err(why) => {
                report.insert(key.into(), json!(false));
                report.insert(format!("{key}Error"), json!(why));
            }
        }
    }
}

/// The Mac checks' bounds: the whole, the menu's two, the deep link's (its page wait included).
const MAC_LIMIT: Duration = Duration::from_secs(20);
const MENU_LIMIT: Duration = Duration::from_secs(5);
const LINK_LIMIT: Duration = Duration::from_secs(8);

async fn bound<T>(limit: Duration, what: &str, work: impl std::future::Future<Output = Result<T, String>>) -> Result<T, String> {
    tokio::time::timeout(limit, work).await.unwrap_or_else(|_| Err(format!("{what} took over {} s", limit.as_secs())))
}

/// `menu.rs`'s accelerator as AppKit keeps it: the key and the modifier bits (⇧ 1<<17, ⌃ 1<<18, ⌥ 1<<19, ⌘ 1<<20).
fn equivalent(accelerator: &str) -> (String, u64) {
    let mut modifiers = 0u64;
    let mut key = String::new();
    for part in accelerator.split('+') {
        match part {
            "Cmd" => modifiers |= 1 << 20,
            "Shift" => modifiers |= 1 << 17,
            "Ctrl" => modifiers |= 1 << 18,
            "Alt" => modifiers |= 1 << 19,
            other => key = other.to_lowercase(),
        }
    }
    (key, modifiers)
}

/// An AppKit item's key equivalent the same way (an upper-case letter is its lower case with ⇧).
fn normalized(key: &str, modifiers: u64) -> (String, u64) {
    let shifted = key.chars().count() == 1 && key.chars().all(char::is_uppercase);
    let modifiers = modifiers & ((1 << 17) | (1 << 18) | (1 << 19) | (1 << 20));
    (key.to_lowercase(), if shifted { modifiers | (1 << 17) } else { modifiers })
}

async fn menu_installed(host: &Arc<Host>) -> Result<bool, String> {
    // The menu is made once the host is managed: given a moment at most.
    let (settings, help) = (host.locale.get("menu.settings"), host.locale.get("menu.help"));
    let mut entries = Vec::new();
    // One deadline for the whole wait (each read is a main-thread round trip).
    let end = Instant::now() + Duration::from_secs(3);
    while Instant::now() < end {
        entries = on_main(host, crate::platform::menu_entries).await.flatten().unwrap_or_default();
        // Its ⌘? is set just after the menu is in (menu.rs `character_key`).
        if entries.iter().any(|(_, title, _, _)| *title == settings)
            && entries.iter().any(|(_, title, key, _)| *title == help && !key.is_empty())
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let workspace = host.locale.get("menu.workspace");
    require(entries.iter().any(|(menu, _, _, _)| *menu == workspace), "the menu bar has no Workspace menu")?;
    for item in crate::menu::items() {
        let title = crate::menu::item_title(host, item);
        let found = entries.iter().find(|(_, t, _, _)| *t == title).ok_or_else(|| format!("the menu bar has no item {title:?}"))?;
        if let Some(accelerator) = item.accelerator {
            require(
                normalized(&found.2, found.3) == equivalent(accelerator),
                &format!("the item {title:?} has the key {:?} {:#x}, not {accelerator}", found.2, found.3),
            )?;
        }
    }
    for (key, what) in [("c", "copy"), ("v", "paste")] {
        let command = (key.to_string(), 1u64 << 20);
        require(entries.iter().any(|(_, _, k, m)| normalized(k, *m) == command), &format!("the Edit menu has no {what}"))?;
    }
    Ok(true)
}

async fn menu_keys(host: &Arc<Host>) -> Result<bool, String> {
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let recorded = seen.clone();
    crate::menu::set_recorder(Some(Arc::new(move |id: &str| lock(&recorded).push(id.to_string()))));
    let mut taken = Vec::new();
    // ⌘K, ⌘T, ⌘, and ⌘= (their virtual keys on a US layout).
    for (key, code) in [("k", 40u16), ("t", 17), (",", 43), ("=", 24)] {
        taken.push(on_main(host, move || crate::platform::press_menu_key(key, code, 1 << 20)).await.flatten() == Some(true));
    }
    let expected = ["search", "newShell", "settings", "textLarger"];
    let reached = wait("the menu's items chosen", 3, || (lock(&seen).len() >= expected.len()).then_some(())).await;
    crate::menu::set_recorder(None);
    let seen = lock(&seen).clone();
    reached.map_err(|why| format!("{why}: {seen:?}, taken {taken:?}"))?;
    require(taken.iter().all(|t| *t), &format!("the menu bar did not take every key: {taken:?}"))?;
    require(seen == expected, &format!("the keys reached {seen:?}"))?;
    Ok(true)
}

/// A link to a local workspace with a folder and its one agent pane of a provider (so nothing is added), delivered in process: the
/// draft gets the words, nothing runs, and everything is put back after. The pane the words went to.
async fn deep_link(host: &Arc<Host>) -> Result<Option<String>, String> {
    let (folder, pane, provider, active, selected) = {
        let view = host.view();
        let snapshot = &view.snapshot;
        let agents = |workspace: &str| -> Vec<(String, String)> {
            snapshot
                .sessions()
                .iter()
                .filter(|s| s.workspace_id() == workspace && s.kind() == "claude" && s.parent_session_id.is_none())
                .map(|s| (s.id().to_string(), s.provider().to_string()))
                .collect()
        };
        let target = snapshot
            .workspaces()
            .iter()
            .filter(|w| w.remote.is_none() && std::path::Path::new(w.path()).is_dir())
            .find_map(|w| {
                let panes = agents(w.id());
                let only = panes.iter().find(|(_, provider)| panes.iter().filter(|(_, p)| p == provider).count() == 1)?.clone();
                Some((w.path().to_string(), only.0, only.1))
            })
            .ok_or("the smoke's profile has no local workspace with a folder and an agent pane")?;
        (target.0, target.1, target.2, snapshot.active_workspace_id.clone(), snapshot.active_session_id.clone())
    };
    // The pane's draft emptied for the link (a draft with words would ask first, review item 1) and put back after.
    let before = {
        let mut view = host.view();
        view.panes.get_mut(&pane).map(|ui| std::mem::take(&mut ui.view.draft)).unwrap_or_default()
    };
    let words = format!("smoke deep link {}", mighty_core::wire::new_id());
    let link = format!("{}&agent={provider}", crate::deep_link::url(&folder, Some(&words)));
    // Put back whatever happens, a bound cut short included: the draft, then the workspace and the pane that were in front.
    let _restore = Restore { host: host.clone(), pane: pane.clone(), draft: before, active, selected };
    let parsed = crate::deep_link::parse(&link).ok_or("the smoke's link did not parse")?;
    let opened = crate::deep_link::open(host, &parsed, false).await;
    let (draft, running) = {
        let view = host.view();
        (view.panes.get(&pane).map(|ui| ui.view.draft.clone()), host.runs().is_some_and(|r| r.service.is_session_running(&pane)))
    };
    let opened = opened?;
    require(
        opened.as_deref() == Some(pane.as_str()),
        &format!("the link's words went to {opened:?}, not the workspace's {provider} pane"),
    )?;
    require(draft.as_deref().is_some_and(|d| d.ends_with(&words)), "the link's words are not in the pane's draft")?;
    require(!running, "the link started a run")?;
    Ok(opened)
}

/// What the deep link's check changed, put back when it ends (or is cut short).
struct Restore {
    host: Arc<Host>,
    pane: String,
    draft: String,
    active: Option<String>,
    selected: Option<String>,
}

impl Drop for Restore {
    fn drop(&mut self) {
        let mut view = self.host.view();
        if let Some(ui) = view.panes.get_mut(&self.pane) {
            ui.view.draft = self.draft.clone();
            ui.dirty = true;
        }
        if let Some(runs) = self.host.runs() {
            let _ = runs.service.update_draft(&self.pane, &self.draft);
        }
        if let Some(active) = &self.active {
            let _ = crate::frame::select_workspace(&mut view, active);
        }
        if let Some(selected) = &self.selected {
            let _ = crate::frame::select_session(&mut view, selected);
        }
    }
}

/// The completion notification the app makes for the pane (`notifications::completion`, what `on_status` sends), sent and awaited
/// through a recording notifier (a smoke never notifies for real, review item 3): its title, its body naming the pane's title and none
/// of its folder or draft, its pane id, and a click that names the pane.
async fn notification_requested(host: &Arc<Host>, pane: Option<&str>) -> Result<bool, String> {
    use mighty_platform::Notifier as _;
    let pane = pane.ok_or("no pane to notify about (the deep link failed)")?;
    let (title, folder) = {
        let view = host.view();
        let session = view.snapshot.session(pane).ok_or("the pane is gone")?;
        let folder = view.snapshot.workspace(session.workspace_id()).map(|w| w.path().to_string()).unwrap_or_default();
        (session.title().to_string(), folder)
    };
    let notifier = mighty_platform::fake::RecordingNotifier::default();
    let clicked: Arc<Mutex<Option<String>>> = Arc::default();
    let recorded = clicked.clone();
    notifier.on_activated(Arc::new(move |pane| *lock(&recorded) = pane));
    let made = crate::notifications::completion(&host.locale, pane, &title);
    notifier.notify(made.clone()).await.map_err(|e| e.to_string())?;
    notifier.click(0);
    let shown = notifier.shown();
    require(shown == [made.clone()], "the notification was not sent as made")?;
    require(
        made.title == mighty_core::notifications::text(&host.locale, "NotificationTitle")
            && made.body.contains(&title)
            && (folder.is_empty() || !made.body.contains(&folder))
            && made.pane_id.as_deref() == Some(pane),
        &format!("the completion notification is not the pane's: {made:?}"),
    )?;
    require(lock(&clicked).as_deref() == Some(pane), "the notification's click does not name its pane")?;
    Ok(true)
}

/// Starts the step; false when it does not run (no companion, or no native views to open its window with).
pub fn start(host: &Arc<Host>) -> bool {
    if host.smoke.is_none() || crate::companion::app(host).and_then(|c| c.surfaces()).is_none() {
        return false;
    }
    host.log.line("smoke: companion step (its own test overlay)");
    let host = host.clone();
    tauri::async_runtime::spawn(async move {
        let mut checks = Map::new();
        let outcome = run(&host, &mut checks).await;
        // Mac stage (M-F): the menu bar, a deep link and the completion notification, checked whatever the pet did.
        if crate::platform::has_menu_bar() {
            mac_checks(&host, &mut checks).await;
        }
        let mut report = Value::Object(checks);
        report["physicalInputTested"] = json!(false);
        report["passed"] = json!(
            outcome.is_ok()
                && KEYS.iter().all(|key| report.get(*key) == Some(&Value::Bool(true)))
                && report.get("statusPopover") == Some(&Value::Bool(true))
                && mac_keys().iter().all(|key| report["mac"].get(*key) == Some(&Value::Bool(true)))
        );
        if let Err(error) = outcome {
            host.log.line(&format!("smoke: companion step failed: {error}"));
            report["error"] = json!(error);
        }
        if let Some(smoke) = &host.smoke {
            smoke.probed(&host, report);
        }
    });
    true
}

/// The step's checks from its report: each key, the popover's, and the step as a whole.
pub fn checks(report: &Value) -> Vec<(&'static str, bool)> {
    let mut checks: Vec<(&'static str, bool)> = KEYS.iter().map(|key| (*key, report.get(*key) == Some(&Value::Bool(true)))).collect();
    checks.push(("companionStatusPopover", report.get("statusPopover") == Some(&Value::Bool(true))));
    // The Mac's own (none on Windows, whose checks do not change).
    checks.extend(mac_keys().iter().map(|key| (*key, report["mac"].get(*key) == Some(&Value::Bool(true)))));
    checks.push(("companionPassed", report.get("passed") == Some(&Value::Bool(true))));
    checks
}

fn require(holds: bool, why: &str) -> Result<(), String> {
    if holds { Ok(()) } else { Err(why.to_string()) }
}

/// Polls until `produce` has a value, at most `seconds`.
async fn wait<T>(what: &str, seconds: u64, mut produce: impl FnMut() -> Option<T>) -> Result<T, String> {
    let end = Instant::now() + Duration::from_secs(seconds);
    loop {
        if let Some(found) = produce() {
            return Ok(found);
        }
        if Instant::now() >= end {
            return Err(format!("the smoke never reached: {what}"));
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A test overlay: its window, the overlay acting on the window's events, the presses it answered (`card:action`), and how many events
/// it has handled. Closed when dropped (C#'s `using`).
struct Pet {
    window: Arc<dyn PetWindow>,
    overlay: Arc<Mutex<Companion>>,
    clicks: Arc<Mutex<Vec<String>>>,
    handled: Arc<AtomicU64>,
}

impl Drop for Pet {
    fn drop(&mut self) {
        let _ = self.window.close();
    }
}

impl Pet {
    async fn open(host: &Arc<Host>, atlas: Option<mighty_platform::pet::PetBitmap>) -> Result<Pet, String> {
        let surfaces = crate::companion::app(host).and_then(|c| c.surfaces()).ok_or("no native views")?;
        let window: Arc<dyn PetWindow> = Arc::from(surfaces.open_pet().await.map_err(|e| e.to_string())?);
        let mut overlay = Companion::new(window.clone(), &CompanionPreferences::default());
        overlay.set_atlas(atlas);
        overlay.set_keep_menu(true);
        let pet = Pet { window, overlay: Arc::new(Mutex::new(overlay)), clicks: Arc::default(), handled: Arc::default() };
        let mut events = pet.window.events().map_err(|e| e.to_string())?;
        let (overlay, clicks, handled) = (pet.overlay.clone(), pet.clicks.clone(), pet.handled.clone());
        tauri::async_runtime::spawn(async move {
            while let Some(event) = events.recv().await {
                let action = lock(&overlay).act(&event);
                if let Some(CompanionAction::Act { card, action }) = action {
                    lock(&clicks).push(format!("{card}:{action}"));
                }
                // The pointer's own moves (the real cursor over the window, the system's move after a capture) are not counted: a
                // step waits for the events its input makes.
                if !matches!(event, mighty_platform::PetEvent::Moved(_)) {
                    handled.fetch_add(1, Ordering::SeqCst);
                }
            }
        });
        Ok(pet)
    }

    fn set_card(&self, card: CompanionCard, visible: bool) {
        lock(&self.overlay).set_card(Some(card), visible);
    }

    /// C# `overlay.Draw(row, column)`: the frame drawn, waited for until the window made it.
    async fn draw(&self, row: i32, column: u32) -> Result<PetCapture, String> {
        let scene = {
            let mut overlay = lock(&self.overlay);
            overlay.set_frame(row, column, false);
            overlay.scene()
        };
        let before = self.window.capture().map(|c| c.frames).unwrap_or(0);
        self.window.draw(&scene).map_err(|e| e.to_string())?;
        wait("the pet's frame drawn", 10, || self.window.capture().ok().filter(|c| c.frames > before)).await
    }

    /// Input through the window's own procedure, waited for until the overlay handled the events it makes.
    async fn send(&self, input: PetInput, events: u64) -> Result<(), String> {
        let before = self.handled.load(Ordering::SeqCst);
        self.window.send(input).map_err(|e| e.to_string())?;
        wait("the pet's events handled", 10, || (self.handled.load(Ordering::SeqCst) >= before + events).then_some(())).await
    }

    /// A change of display to `dpi` through the window's own procedure, waited for until the overlay takes its scale (not for one more
    /// event: a count is one behind when an earlier input's event came late, and the step would then draw at the old scale).
    async fn scale_to(&self, dpi: u32) -> Result<(), String> {
        let wanted = (f64::from(dpi) / 96.0).clamp(1.0, 4.0);
        let before = self.handled.load(Ordering::SeqCst);
        self.window.send(PetInput::Dpi(dpi)).map_err(|e| e.to_string())?;
        // Its own event handled too, so a later step never meets this change's event late.
        wait("the pet's scale changed", 10, || {
            (lock(&self.overlay).scale() == wanted && self.handled.load(Ordering::SeqCst) > before).then_some(())
        })
        .await
    }

    /// C# `SmokeClick(id)`: a press and release at the middle of the target; false when the frame has no such target.
    async fn click(&self, id: &str) -> Result<bool, String> {
        let Some(at) = lock(&self.overlay).target(id) else { return Ok(false) };
        self.send(PetInput::Click(at), 2).await?;
        Ok(true)
    }

    fn texts(&self) -> Vec<PetText> {
        lock(&self.overlay).scene().texts().into_iter().cloned().collect()
    }

    fn focus(&self) -> PetFocus {
        self.window.focus().unwrap_or_default()
    }

    /// C# `Aside()`: never activated, never the front window, never the keyboard's.
    fn aside(&self) -> bool {
        let focus = self.focus();
        focus.non_activating && focus.foreground != focus.own && !focus.took_thread
    }
}

/// C# `CaptureCompanionSmoke`: the window's own buffer as a PNG in the smoke's profile; its path.
fn capture(host: &Host, frame: &PetCapture, name: &str) -> Result<String, String> {
    require(
        frame.bgra.len() == (frame.width * frame.height * 4) as usize && frame.bgra.iter().any(|b| *b != 0),
        "Companion rendered buffer is empty.",
    )?;
    let png = pet::png(frame.width, frame.height, frame.width * 4, &frame.bgra)?;
    let path = host.log.dir().join(name);
    std::fs::write(&path, png).map_err(|e| e.to_string())?;
    Ok(path.display().to_string())
}

/// A pixel of a frame at a point of the window (C#'s `At`): blue, green, red, alpha.
fn at(frame: &PetCapture, scale: f64, x: f64, y: f64) -> (u8, u8, u8, u8) {
    let index = (((y * scale) as usize) * frame.width as usize + (x * scale) as usize) * 4;
    frame.bgra.get(index..index + 4).map_or((0, 0, 0, 0), |p| (p[0], p[1], p[2], p[3]))
}

fn text<'a>(texts: &'a [PetText], value: &str, what: &str) -> Result<&'a PetText, String> {
    texts.iter().find(|t| t.value == value).ok_or_else(|| format!("the pet's bubble does not draw {what}: \"{value}\""))
}

/// C#'s `Drawn`: a text's size, weight, ink and family, inside the bubble's 12 of padding.
#[allow(clippy::too_many_arguments, reason = "C#'s Drawn checks these")]
fn drawn(
    pet: &Pet,
    texts: &[PetText],
    value: &str,
    size: f64,
    weight: u16,
    ink: Token,
    family: &str,
    what: &str,
) -> Result<PetText, String> {
    let found = text(texts, value, what)?.clone();
    require(
        found.size == size && found.asked == weight && found.ink == ink.camel() && found.family == family,
        &format!(
            "the pet's bubble: {what} must be {size} at weight {weight} in {}; got {} at {} in {}, {}",
            ink.camel(),
            found.size,
            found.asked,
            found.ink,
            found.family
        ),
    )?;
    let bubble = lock(&pet.overlay).geometry().bubble;
    let b = found.bounds;
    require(
        b.x >= bubble.x + 12.0 - 0.01
            && b.right() <= bubble.right() - 12.0 + 0.01
            && b.y >= bubble.y + 12.0 - 0.01
            && b.bottom() <= bubble.bottom() - 12.0 + 0.01,
        &format!("the pet's bubble: {what} must stay inside the 12 of padding; got {b:?} in {bubble:?}"),
    )?;
    Ok(found)
}

/// C#'s smoke agent: the fixture's first Claude pane, named after its provider, with a run that took 42 seconds.
fn agent(host: &Host) -> Result<(RunSession, String), String> {
    let snapshot = host.view().snapshot.clone();
    let first = snapshot.sessions().iter().find(|s| s.kind() == "claude").ok_or("the smoke's fixture has no Claude pane")?;
    let workspace =
        snapshot.workspace(first.workspace_id()).map(|w| w.name().to_string()).ok_or("the smoke's Claude pane has no workspace")?;
    let now = DateTimeOffset::now();
    let started = DateTimeOffset::from_utc_ticks(now.utc_ticks() - 42 * 10_000_000);
    // A run still going would tick its clock between the card and the check: only a finished one is kept.
    let timing = first.run_timing.filter(|t| t.finished_at.is_some()).unwrap_or(mighty_core::model::AgentRunTiming {
        started_at: started,
        last_observed_at: now,
        finished_at: Some(now),
        is_approximate: false,
    });
    // Its last request is the step's own short one: the pane's own depends on what ran before (a `-NoRuns` smoke, the Mac's CEF run,
    // keeps the demo's long request, which the bubble rightly cuts).
    let mut logs = first.logs().to_vec();
    logs.push(mighty_core::model::LogEntry {
        id: Some(wire::new_id()),
        kind: Some("user".into()),
        text: Some("Run the tests and fix what fails.".into()),
        ..Default::default()
    });
    let agent = RunSession {
        title: Some(mighty_run::strings::provider_label(first.provider()).into()),
        run_timing: Some(timing),
        logs: Some(logs),
        ..(**first).clone()
    };
    Ok((agent, workspace))
}

async fn run(host: &Arc<Host>, checks: &mut Map<String, Value>) -> Result<(), String> {
    let bundled = crate::companion::bundled_pets(host);
    let source = bundled.join(pet::DEFAULT_PET);
    let loaded = tauri::async_runtime::spawn_blocking(move || {
        CompanionPet::load(&source, pet::DEFAULT_PET).and_then(|pet| pet.decode().map(|atlas| (pet, atlas)))
    })
    .await
    .map_err(|e| e.to_string())??;
    let (bundled_pet, atlas) = loaded;
    checks.insert("bundledAtlasDecoded".into(), json!(atlas.bgra.len() == (bundled_pet.width * bundled_pet.height * 4) as usize));
    let preview = pet::preview_png(&atlas)?;
    require(pet::png_size(&preview) == Some((192, 208)), "Companion preview must crop exactly the first atlas frame.")?;
    checks.insert("selectedPetPreview".into(), json!(true));
    let (agent, workspace) = agent(host)?;
    let pet = Pet::open(host, Some(atlas.clone())).await?;
    // The app in front when the pet opened (the high half of `foreground` on the Mac): the same at the end.
    let front_app = pet.focus().foreground >> 32;
    // The pet's window is never the one in front, and nothing it is told to do moves the front window: compared around each step.
    let mut foreground: u64;
    let kept = |pet: &Pet, foreground: u64| pet.focus().foreground == foreground && pet.aside();
    let card =
        |key: &str,
         session: &RunSession,
         status: &str,
         permission: Option<&ToolPermissionRequest>,
         position: usize,
         count: usize,
         pending: bool| { crate::companion::card_for(host, key, session, status, permission, position, count, pending) };
    pet.set_card(card("smoke-pet", &agent, "waiting", None, 1, 2, false), true);
    let (placement, scale) = {
        let overlay = lock(&pet.overlay);
        (overlay.placement(), overlay.scale())
    };
    let area = pet.window.work_area(LogicalPoint { x: 0.0, y: 0.0 }).map_err(|e| e.to_string())?;
    let (right, bottom) = ((area.x + area.width) as i32, (area.y + area.height) as i32);
    require(
        placement.right == right - (300.0 * scale).round() as i32 + (282.0 * scale).ceil() as i32
            && placement.bottom == bottom - (24.0 * scale).round() as i32,
        "The pet must first stand 300 in from the screen's right edge and 24 above its bottom (M/AgentCompanionViews.swift:438-439).",
    )?;
    for row in 0..9 {
        pet.draw(row, pet::ROW_FRAMES[row as usize] - 1).await?;
    }
    checks.insert("allAnimationRowsRendered".into(), json!(true));
    foreground = pet.focus().foreground;
    let scene = lock(&pet.overlay).scene();
    pet.window.show(&scene).map_err(|e| e.to_string())?;
    pet.draw(7, 0).await?;
    require(kept(&pet, foreground), "Companion stole foreground focus.")?;
    if crate::platform::has_menu_bar() {
        // The NSPanel as Swift's `CompanionPanel` sets it, read back on the main thread once it is on screen.
        let facts = on_main(host, crate::platform::pet_panel_facts).await.flatten().unwrap_or_default();
        let held = !facts.is_empty() && facts.iter().all(|(_, holds)| *holds);
        let report = mac_report(checks);
        report.insert("companionPanel".into(), json!(held));
        report.insert("companionPanelFacts".into(), json!(facts.iter().map(|(k, v)| (k.to_string(), json!(v))).collect::<Map<_, _>>()));
    }
    tokio::time::sleep(Duration::from_millis(120)).await;
    require(pet.aside(), "Companion stole foreground focus.")?;
    checks.insert("nonActivatingWindow".into(), json!(true));
    foreground = pet.focus().foreground;
    require(
        pet.click("next").await? && pet.click("toggle").await? && *lock(&pet.clicks) == ["smoke-pet:next", "smoke-pet:toggle"],
        "Companion pointer action did not match the displayed card.",
    )?;
    require(kept(&pet, foreground), "Companion click stole foreground focus.")?;
    checks.insert("pointerActionBoundToCard".into(), json!(true));
    // The pet's menu is the Mac's two items, in its order; choosing one acts and puts it away, a click beside it only puts it away.
    foreground = pet.focus().foreground;
    let center = lock(&pet.overlay).target("toggle").ok_or("no pet")?;
    pet.send(PetInput::RightClick(center), 1).await?;
    let frame = pet.draw(0, 0).await?;
    let menu = lock(&pet.overlay).menu();
    let expected =
        [("hide".to_string(), mighty_run::strings::get("menu.hidePet")), ("open".to_string(), mighty_run::strings::get("menu.openAgent"))];
    require(menu == expected, "The pet's menu must offer hide and open, in the Mac's order.")?;
    require(kept(&pet, foreground), "Companion context actions stole focus or could not close.")?;
    let mut screenshots = vec![capture(host, &frame, "smoke-companion-menu-dark.png")?];
    foreground = pet.focus().foreground;
    require(
        pet.click("open").await?
            && lock(&pet.clicks).last().map(String::as_str) == Some("smoke-pet:open")
            && lock(&pet.overlay).menu().is_empty()
            && kept(&pet, foreground),
        "Companion context actions stole focus or could not close.",
    )?;
    pet.send(PetInput::RightClick(center), 1).await?;
    require(
        lock(&pet.overlay).menu().len() == 2
            && pet.click("toggle").await?
            && lock(&pet.overlay).menu().is_empty()
            && lock(&pet.clicks).len() == 3
            && kept(&pet, foreground),
        "A click beside the pet's menu must only put it away.",
    )?;
    checks.insert("contextMenuNonActivating".into(), json!(true));
    // One line of request and of work, then two of each: the height follows the content.
    pet.set_card(card("smoke-compact", &agent, agent.status(), None, 0, 0, false), true);
    let compact = lock(&pet.overlay).placement();
    let wordy = RunSession {
        title: Some(["A long pane title"; 6].join(" ")),
        logs: Some(vec![mighty_core::model::LogEntry {
            id: Some(wire::new_id()),
            kind: Some("user".into()),
            text: Some(["Requested task"; 30].join(" ")),
            timestamp: Some(mighty_core::time::now_o()),
            ..mighty_core::model::LogEntry::default()
        }]),
        current_activity: Some(mighty_core::model::AgentActivity {
            id: "smoke-work".into(),
            provider: agent.provider().into(),
            kind: "command".into(),
            state: "running".into(),
            summary: ["Current work"; 30].join(" "),
            tool_name: None,
            output: None,
            duration_ms: None,
        }),
        ..agent.clone()
    };
    pet.set_card(card("smoke-layout", &wordy, "running", None, 0, 0, false), true);
    let frame = pet.draw(7, 0).await?;
    let automatic = lock(&pet.overlay).placement();
    require(
        automatic.automatic_height && automatic.height != compact.height && automatic.height < 480,
        "Companion default height did not follow measured content.",
    )?;
    let texts = pet.texts();
    let long_title = texts.iter().find(|t| t.size == 11.0 && t.asked == 600).ok_or("no title")?;
    let state = texts.iter().find(|t| t.size == 9.0 && t.asked == 400).ok_or("no state word")?;
    require(
        long_title.lines == 1 && long_title.trimmed && long_title.bounds.right() <= state.bounds.x - 14.0 + 0.01,
        &format!(
            "The bubble's long title must end in an ellipsis before the state word; got {} lines, trimmed {}, {:?}.",
            long_title.lines, long_title.trimmed, long_title.bounds
        ),
    )?;
    let bubble = lock(&pet.overlay).geometry().bubble;
    for words in texts.iter().filter(|t| t.size == 11.0 && t.asked == 400) {
        require(
            words.lines == 2 && words.trimmed && words.bounds.bottom() <= bubble.bottom() - 12.0,
            &format!(
                "The bubble's long words must end in an ellipsis on their second line, inside the padding; got {} lines, trimmed {}.",
                words.lines, words.trimmed
            ),
        )?;
    }
    screenshots.push(capture(host, &frame, "smoke-companion-running-dark.png")?);
    {
        let overlay = lock(&pet.overlay);
        let (w, h) = (f64::from(automatic.width), f64::from(automatic.height));
        require(
            overlay.resize_hit(1.0, h / 2.0) == Edges::LEFT && overlay.resize_hit(w / 2.0, 1.0) == Edges::TOP,
            "Companion top/side resize strips are missing.",
        )?;
        require(
            overlay.resize_hit(w - 2.0, h - 2.0) == Edges::RIGHT && overlay.resize_hit(w / 2.0, h - 2.0) == Edges::NONE,
            "The bubble's bottom meets the pet and must not resize (M/AgentCompanionViews.swift:484-505).",
        )?;
    }
    foreground = pet.focus().foreground;
    lock(&pet.overlay).smoke_resize(Edges::LEFT, -60.0, 0.0, false);
    let wider = lock(&pet.overlay).placement();
    require(
        wider.width == automatic.width + 60 && wider.right == automatic.right && wider.automatic_height,
        "Side resize moved its opposite edge or disabled automatic height.",
    )?;
    lock(&pet.overlay).smoke_resize(Edges::TOP, 0.0, -40.0, false);
    let taller = lock(&pet.overlay).placement();
    require(
        taller.height == wider.height + 40 && taller.bottom == wider.bottom && !taller.automatic_height,
        "Top resize moved the pet instead of pinning the bottom.",
    )?;
    let frame = pet.draw(7, 0).await?;
    require(kept(&pet, foreground), "Companion resize acquired foreground focus.")?;
    screenshots.push(capture(host, &frame, "smoke-companion-taller-dark.png")?);
    let lines: u32 = pet.texts().iter().filter(|t| t.size == 11.0 && t.asked == 400).map(|t| t.lines).sum();
    require(lines > 4, "A taller bubble must show more of the request and the work (M/AgentCompanionViews.swift:198-199).")?;
    foreground = pet.focus().foreground;
    lock(&pet.overlay).smoke_resize(Edges::RIGHT, 40.0, 0.0, true);
    require(lock(&pet.overlay).placement() == taller, "Cancelled resize changed saved geometry.")?;
    lock(&pet.overlay).reset_size();
    let reset = lock(&pet.overlay).placement();
    require(reset.automatic_height && reset.width == layout::DEFAULT_WIDTH as i32, "Double-click reset must restore automatic sizing.")?;
    require(kept(&pet, foreground), "Companion resize acquired foreground focus.")?;
    checks.insert("contentDrivenHeightAndEdgeResize".into(), json!(true));
    design(host, &pet, &agent, &workspace, &mut screenshots).await?;
    require(pet.aside(), "Companion requests acquired foreground focus.")?;
    checks.insert("planBubbleAndWindow".into(), json!(true));
    // On a display at 150% and at 200% the same card is the same points: only the pixels under them grow.
    for dpi in [144u32, 192] {
        // Waited for until the overlay takes the scale, not for one more event: a count is one behind when an earlier input's event
        // came late, and the step would then draw at the old scale.
        pet.scale_to(dpi).await?;
        pet.set_card(CompanionCard { light: false, ..card(&format!("smoke-scale-{dpi}"), &agent, "waiting", None, 0, 0, false) }, true);
        let frame = pet.draw(7, 0).await?;
        require(
            lock(&pet.overlay).scale() == f64::from(dpi) / 96.0 && frame.width == (282.0 * f64::from(dpi) / 96.0).ceil() as u32,
            &format!("The pet's window must follow a display at {dpi} dpi."),
        )?;
        require(pet.aside(), &format!("The pet's window acquired foreground focus as its display changed to {dpi} dpi."))?;
        capture(host, &frame, &format!("smoke-companion-overlay-dark-{}.png", dpi * 100 / 96))?;
        check_bubble(&pet, &frame, &agent, &workspace, false)?;
    }
    pet.scale_to((96.0 * scale).round() as u32).await?;
    let mut captures = vec![];
    for light in [false, true] {
        pet.set_card(CompanionCard { light, ..card(&format!("smoke-design-{light}"), &agent, "waiting", None, 0, 0, false) }, true);
        let frame = pet.draw(7, 0).await?;
        captures.push(capture(host, &frame, &format!("smoke-companion-overlay-{}.png", if light { "light" } else { "dark" }))?);
        check_bubble(&pet, &frame, &agent, &workspace, light)?;
    }
    // To the end of the stage: not through a change of scale or of theme either.
    require(pet.aside(), "The pet's window acquired foreground focus as its scale or theme changed.")?;
    checks.insert("renderedOverlaySnapshots".into(), json!(captures.len() == 2));
    screenshots.extend(captures);
    checks.insert("screenshots".into(), json!(screenshots));
    if crate::platform::has_menu_bar() {
        let unchanged = pet.focus().foreground >> 32 == front_app;
        mac_report(checks).insert("frontAppUnchanged".into(), json!(unchanged));
    }
    drop(pet);
    checks.insert("statusPopover".into(), json!(status_popover(host).await?));
    Ok(())
}

/// C# `CheckCompanionBubble`: the window and the pet's frame, each text's size, weight and ink, and the card with its corner and
/// hairline.
fn check_bubble(pet: &Pet, frame: &PetCapture, agent: &RunSession, workspace: &str, light: bool) -> Result<(), String> {
    let theme = if light { "light" } else { "dark" };
    let palette = if light { &LIGHT } else { &DARK };
    let geometry = lock(&pet.overlay).geometry();
    let (bubble, frame_of_pet, scale) = (geometry.bubble, geometry.pet, geometry.scale);
    require(
        geometry.width == 282.0 && geometry.height == 330.0,
        &format!("the pet's window must be 282 × 330 around a bubble of the default width; got {} × {}", geometry.width, geometry.height),
    )?;
    require(
        bubble.x == 12.0 && bubble.width == 258.0 && bubble.bottom() == 185.0,
        &format!("the bubble must stand 12 in from the window's sides and 2 above the pet; got {bubble:?}"),
    )?;
    require(
        frame_of_pet.x == 78.5 && frame_of_pet.y == 187.0 && frame_of_pet.width == 125.0 && frame_of_pet.height == 135.0,
        &format!("the pet's frame must be 125 × 135, centred, 8 above the window's bottom; got {frame_of_pet:?}"),
    )?;
    let texts = pet.texts();
    let body = mighty_companion::scene::BODY;
    let space = drawn(pet, &texts, workspace, 9.0, 500, Token::Ink2, body, "the workspace")?;
    let title = drawn(pet, &texts, agent.title(), 11.0, 600, Token::Ink, body, "the title")?;
    let state = drawn(pet, &texts, &mighty_run::strings::get("graph.state.waiting"), 9.0, 400, Token::Ink2, body, "the state word")?;
    let clock = drawn(
        pet,
        &texts,
        &mighty_run::timing::label(&agent.run_timing.ok_or("no timing")?, None),
        10.0,
        400,
        Token::Ink2,
        mighty_companion::scene::MONO,
        "the clock",
    )?;
    let asked =
        drawn(pet, &texts, &mighty_run::strings::get("graph.timeline.kind.request"), 9.0, 500, Token::Ink2, body, "the request's label")?;
    let prompt = agent
        .logs()
        .iter()
        .rev()
        .find(|l| l.kind.as_deref() == Some("user"))
        .map(|l| mighty_run::activity::clean(l.text.as_deref(), 2048, true));
    let request = match prompt {
        Some(prompt) => Some(drawn(pet, &texts, &prompt, 11.0, 400, Token::Ink, body, "the request")?),
        None => None,
    };
    let doing = drawn(pet, &texts, &mighty_run::strings::get("transcript.tool.fallback"), 9.0, 500, Token::Ink2, body, "the work's label")?;
    let work = drawn(pet, &texts, &mighty_run::strings::get("companion.status.waiting"), 11.0, 400, Token::Ink, body, "the work")?;
    // The header's columns: 15 for the mark and 7 before the names; the state and the clock end at the padding.
    require(
        (space.bounds.x - (bubble.x + 12.0 + 15.0 + 7.0)).abs() < 0.01
            && (title.bounds.x - space.bounds.x).abs() < 0.01
            && (title.bounds.y - space.bounds.bottom() - 1.0).abs() < 0.01,
        &format!(
            "the pet's bubble ({theme}): the workspace must stand 1 above the title, after the mark; got {:?} and {:?}",
            space.bounds, title.bounds
        ),
    )?;
    require(
        (state.bounds.right() - (bubble.right() - 12.0)).abs() < 0.01
            && (clock.bounds.right() - state.bounds.right()).abs() < 0.01
            && (clock.bounds.y - state.bounds.bottom() - 2.0).abs() < 0.01,
        &format!(
            "the pet's bubble ({theme}): the state word must stand 2 above the clock, both ending at the padding; got {:?} and {:?}",
            state.bounds, clock.bounds
        ),
    )?;
    // Each label starts the row, two below the top of its words, which follow 6 after it; the request is above the work.
    if let Some(request) = &request {
        require(
            (asked.bounds.x - (bubble.x + 12.0)).abs() < 0.01
                && (asked.bounds.y - request.bounds.y - 2.0).abs() < 0.01
                && (request.bounds.x - asked.bounds.right().ceil() - 6.0).abs() < 0.01
                && (doing.bounds.y - work.bounds.y - 2.0).abs() < 0.01
                && request.bounds.bottom() < work.bounds.y
                && request.bounds.y > title.bounds.bottom(),
            &format!(
                "the pet's bubble ({theme}): the request and the work must each follow their label; got {:?} {:?}, {:?} {:?}",
                asked.bounds, request.bounds, doing.bounds, work.bounds
            ),
        )?;
        require(
            request.lines <= 2 && work.lines <= 2 && !request.trimmed,
            &format!(
                "the pet's bubble ({theme}): a short request must be whole, in two lines at most; got {} lines, trimmed {}",
                request.lines, request.trimmed
            ),
        )?;
    }
    let card = palette[Token::Card];
    let middle = at(frame, scale, bubble.x + 5.0, bubble.y + bubble.height / 2.0);
    require(
        middle == (card.b, card.g, card.r, 255),
        &format!("the pet's bubble ({theme}) must be the card colour {}; got {middle:?}", card.hex_text()),
    )?;
    require(
        at(frame, scale, bubble.x + 1.0, bubble.y + 1.0).3 == 0
            && at(frame, scale, bubble.right() - 2.0, bubble.y + 1.0).3 == 0
            && at(frame, scale, bubble.x + 1.0, bubble.bottom() - 2.0).3 == 0
            && at(frame, scale, bubble.x + 7.0, bubble.y + 7.0).3 == 255,
        &format!("the pet's bubble ({theme}) must have corners of radius 18"),
    )?;
    let mut x = bubble.x;
    while x < bubble.right() {
        require(
            at(frame, scale, x, bubble.bottom() + 0.5).3 == 0 && at(frame, scale, x, bubble.bottom() + 1.5).3 == 0,
            &format!("the pet's bubble ({theme}) must leave 2 clear above the pet's frame"),
        )?;
        x += 6.0;
    }
    if scale == scale.floor() {
        // The hairline: the primary ink at 10% on the card.
        let edge = at(frame, scale, bubble.x + bubble.width / 2.0, bubble.y);
        let ink = palette[Token::Ink];
        let mixed = |under: u8, over: u8| (f64::from(under) + (f64::from(over) - f64::from(under)) * 26.0 / 255.0).round() as i32;
        require(
            edge.3 == 255
                && (i32::from(edge.2) - mixed(card.r, ink.r)).abs() <= 2
                && (i32::from(edge.1) - mixed(card.g, ink.g)).abs() <= 2
                && (i32::from(edge.0) - mixed(card.b, ink.b)).abs() <= 2,
            &format!("the pet's bubble ({theme}) must have a hairline of the ink at 10%; got {edge:?}"),
        )?;
    }
    Ok(())
}

/// C# `RunCompanionDesignSmoke`: the pager, an approval, a question and a finished plan in both themes, their presses bound to the card
/// shown, a long plan's scroll, the question stepped through the app's own handler, and the paw.
async fn design(host: &Arc<Host>, pet: &Pet, agent: &RunSession, workspace: &str, screenshots: &mut Vec<String>) -> Result<(), String> {
    let approval = ToolPermissionRequest::new(
        "smoke-approval",
        agent.id(),
        "smoke-tool",
        "Bash",
        r#"{"command":"dotnet test native/windows/MightyCode.Core.Tests -c Release --no-build --logger trx","description":"Run the Windows core tests before the commit"}"#,
        "dotnet test",
    );
    let question = ToolPermissionRequest {
        can_allow: false,
        can_answer_questions: true,
        ..ToolPermissionRequest::new(
            "smoke-question",
            agent.id(),
            "smoke-ask",
            "AskUserQuestion",
            r#"{"questions":[{"header":"Layout","question":"Which layout should the pet's bubble follow?","multiSelect":true,"options":[{"label":"The Mac's","description":"The same order, type and paddings"},{"label":"The earlier Windows card","description":""}]},{"header":"Next","question":"And after that?","multiSelect":false,"options":[{"label":"Ship it","description":""},{"label":"Look again","description":""}]}]}"#,
            "question",
        )
    };
    let plan = ToolPermissionRequest {
        can_allow: false,
        can_answer_plan: true,
        received_at: Some("2027-01-15T08:05:00.000Z".into()),
        ..ToolPermissionRequest::new(
            "smoke-plan",
            agent.id(),
            "smoke-exit-plan",
            mighty_core::permission::PLAN_TOOL,
            r##"{"plan":"# Ship the pet's plan bubble\n\n1. Draw the plan in the bubble\n2. Route its buttons\n3. Open the plan window\n4. Fake an answer"}"##,
            "plan",
        )
    };
    let result = design_steps(host, pet, agent, workspace, screenshots, &approval, &question, &plan).await;
    // Whatever step failed, the app's own pet starts clean: no draft left of the smoke's question.
    crate::companion::forget_draft(host, &question);
    result
}

#[allow(clippy::too_many_arguments, reason = "the design step's fixtures, made by its caller so its finally can clean them")]
async fn design_steps(
    host: &Arc<Host>,
    pet: &Pet,
    agent: &RunSession,
    workspace: &str,
    screenshots: &mut Vec<String>,
    approval: &ToolPermissionRequest,
    question: &ToolPermissionRequest,
    plan: &ToolPermissionRequest,
) -> Result<(), String> {
    let card = |key: &str, status: &str, permission: Option<&ToolPermissionRequest>, position: usize, count: usize, pending: bool| {
        crate::companion::card_for(host, key, agent, status, permission, position, count, pending)
    };
    let form = UserQuestionnaire::parse(&question.input_json).ok_or("the smoke's question did not parse")?;
    crate::companion::choose_for(host, question, "The Mac's");
    let origin = format!("{workspace} · {}", agent.title());
    let body = mighty_companion::scene::BODY;
    for light in [false, true] {
        let theme = if light { "light" } else { "dark" };
        pet.set_card(CompanionCard { light, ..card("smoke-pager", agent.status(), None, 2, 3, true) }, true);
        let frame = pet.draw(0, 0).await?;
        screenshots.push(capture(host, &frame, &format!("smoke-companion-pager-{theme}.png"))?);
        drawn(pet, &pet.texts(), "2 / 3", 9.0, 500, Token::Ink2, body, "the pager")?;
        require(lock(&pet.overlay).geometry().height == layout::BASE_HEIGHT, "the pager must fit the base window")?;

        pet.set_card(CompanionCard { light, ..card("smoke-approval", "waiting", Some(approval), 0, 0, false) }, true);
        let frame = pet.draw(6, 0).await?;
        screenshots.push(capture(host, &frame, &format!("smoke-companion-approval-{theme}.png"))?);
        let geometry = lock(&pet.overlay).geometry();
        require(
            geometry.width == 282.0 && geometry.height == layout::TALL_HEIGHT && geometry.bubble.bottom() == layout::TALL_HEIGHT - 145.0,
            &format!("an approval must stand in the taller window, on the pet; got {} × {}", geometry.width, geometry.height),
        )?;
        let texts = pet.texts();
        drawn(pet, &texts, &mighty_run::strings::get("companion.approval.title"), 11.0, 600, Token::Ink, body, "the approval's title")?;
        drawn(pet, &texts, &origin, 9.0, 400, Token::Ink2, body, "the approval's origin")?;
        drawn(
            pet,
            &texts,
            &format!("{} · Bash", mighty_run::presentation::strings::title_bash()),
            10.0,
            400,
            Token::Ink2,
            body,
            "the approval's tool",
        )?;
        drawn(pet, &texts, "Run the Windows core tests before the commit", 11.0, 500, Token::Ink, body, "the approval's headline")?;
        drawn(
            pet,
            &texts,
            "dotnet test native/windows/MightyCode.Core.Tests -c Release --no-build --logger trx",
            10.0,
            400,
            Token::Ink,
            mighty_companion::scene::MONO,
            "the command",
        )?;
        drawn(pet, &texts, &mighty_run::strings::get("companion.button.open"), 11.0, 400, Token::Ink, body, "open")?;
        drawn(pet, &texts, &mighty_run::presentation::strings::button_deny(), 11.0, 400, Token::Ink, body, "deny")?;
        drawn(pet, &texts, &mighty_run::presentation::strings::button_allow_once(), 11.0, 400, Token::OnAccent, body, "allow once")?;
        require(
            !texts.iter().any(|t| t.value == mighty_run::strings::get("companion.button.review")),
            "the approval's review window must be a symbol, not words",
        )?;
        if geometry.scale == geometry.scale.floor() {
            // Around a request the hairline is the amber at 45%, on the card.
            let palette = if light { &LIGHT } else { &DARK };
            let edge = at(&frame, geometry.scale, geometry.bubble.x + geometry.bubble.width / 2.0, geometry.bubble.y);
            let (under, over) = (palette[Token::Card], palette[Token::Wait]);
            let mixed = |u: u8, o: u8| (f64::from(u) + (f64::from(o) - f64::from(u)) * 115.0 / 255.0).round() as i32;
            require(
                edge.3 == 255
                    && (i32::from(edge.0) - mixed(under.b, over.b)).abs() <= 2
                    && (i32::from(edge.1) - mixed(under.g, over.g)).abs() <= 2
                    && (i32::from(edge.2) - mixed(under.r, over.r)).abs() <= 2,
                &format!("an approval ({theme}) must have a hairline of the amber at 45%; got {edge:?}"),
            )?;
        }

        pet.set_card(CompanionCard { light, ..card("smoke-question", "waiting", Some(question), 0, 0, false) }, true);
        let frame = pet.draw(6, 0).await?;
        screenshots.push(capture(host, &frame, &format!("smoke-companion-question-{theme}.png"))?);
        let texts = pet.texts();
        drawn(
            pet,
            &texts,
            &mighty_run::strings::get_with("companion.question.titleProgress", &[("current", "1"), ("total", "2")]),
            11.0,
            600,
            Token::Ink,
            body,
            "the question's title",
        )?;
        drawn(pet, &texts, "Which layout should the pet's bubble follow?", 11.0, 500, Token::Ink, body, "the question")?;
        drawn(pet, &texts, "The Mac's", 11.0, 500, Token::Ink, body, "a choice")?;
        drawn(pet, &texts, "The same order, type and paddings", 9.0, 400, Token::Ink2, body, "a choice's description")?;
        drawn(pet, &texts, "The earlier Windows card", 11.0, 500, Token::Ink, body, "a choice")?;
        drawn(pet, &texts, &mighty_run::strings::get("companion.question.next"), 11.0, 400, Token::OnAccent, body, "next")?;
        drawn(pet, &texts, &mighty_run::strings::get("phone.questionnaire.cancel"), 11.0, 400, Token::Ink, body, "cancel")?;
        require(
            !texts.iter().any(|t| {
                t.value == mighty_run::strings::get("companion.button.answer")
                    || t.value == mighty_run::strings::get("companion.question.back")
            }),
            "the first question must show no Back, and its answer window as a symbol",
        )?;

        pet.set_card(CompanionCard { light, ..card("smoke-plan", "waiting", Some(plan), 0, 0, false) }, true);
        let frame = pet.draw(6, 0).await?;
        screenshots.push(capture(host, &frame, &format!("smoke-companion-plan-{theme}.png"))?);
        let geometry = lock(&pet.overlay).geometry();
        require(
            geometry.width == 282.0 && geometry.height == layout::TALL_HEIGHT && geometry.bubble.bottom() == layout::TALL_HEIGHT - 145.0,
            &format!("a plan must stand in the taller window, on the pet; got {} × {}", geometry.width, geometry.height),
        )?;
        let texts = pet.texts();
        drawn(pet, &texts, &mighty_run::strings::get("plan.card.title"), 11.0, 600, Token::Ink, body, "the plan's title")?;
        drawn(pet, &texts, &origin, 9.0, 400, Token::Ink2, body, "the plan's origin")?;
        let offset = chrono::Local::now().offset().local_minus_utc() / 60;
        drawn(
            pet,
            &texts,
            &mighty_composer::presentation::plan::received_text("2027-01-15T08:05:00.000Z", offset, &host.locale),
            9.0,
            400,
            Token::Ink2,
            body,
            "when it came",
        )?;
        drawn(pet, &texts, "Ship the pet's plan bubble", 13.0, 600, Token::Ink, body, "the plan's heading")?;
        for step in ["Draw the plan in the bubble", "Route its buttons", "Open the plan window", "Fake an answer"] {
            drawn(pet, &texts, step, 11.0, 400, Token::Ink, body, "a step")?;
        }
        let cancel = drawn(pet, &texts, &mighty_run::strings::get("plan.card.cancel"), 11.0, 400, Token::Ink, body, "cancel")?;
        drawn(pet, &texts, &mighty_run::strings::get("plan.card.revise"), 11.0, 400, Token::Ink, body, "revise")?;
        let confirm = drawn(
            pet,
            &texts,
            &mighty_run::strings::get("plan.card.approveConfirm"),
            11.0,
            400,
            Token::Ink,
            body,
            "approve, confirming each",
        )?;
        let approve = drawn(pet, &texts, &mighty_run::strings::get("plan.card.approveAuto"), 11.0, 400, Token::OnAccent, body, "approve")?;
        require(
            !texts.iter().any(|t| t.value == mighty_run::strings::get("companion.plan.review")),
            "the plan's own window must be a symbol, not words",
        )?;
        require(
            confirm.bounds.y > cancel.bounds.bottom() && approve.bounds.y > confirm.bounds.bottom() && !approve.trimmed && !confirm.trimmed,
            &format!(
                "the plan's approvals ({theme}) must be whole, each on a row under the others; got {:?} and {:?} under {:?}",
                confirm.bounds, approve.bounds, cancel.bounds
            ),
        )?;
        let short = lock(&pet.overlay).plan_box();
        require(
            short.extent <= short.area.height + 0.01 && short.scroll == 0.0,
            &format!("a short plan ({theme}) must show whole, without scrolling; got {} in {}", short.extent, short.area.height),
        )?;
    }
    // A choice, Next, the answer view and the cancel all belong to the question shown; the approval's to the approval. None of them
    // brings a window forward.
    pet.set_card(card("smoke-question", "waiting", Some(question), 0, 0, false), true);
    pet.draw(6, 0).await?;
    let before = lock(&pet.clicks).len();
    let front = pet.focus().foreground;
    let mut pressed = true;
    for id in ["choice-1", "question-commit", "questions", "deny", "open"] {
        pressed &= pet.click(id).await?;
    }
    pressed &= !pet.click("question-back").await? && !pet.click("allow").await?;
    require(
        pressed
            && lock(&pet.clicks)[before..]
                == [
                    "smoke-question:choice-1",
                    "smoke-question:question-commit",
                    "smoke-question:questions",
                    "smoke-question:deny",
                    "smoke-question:open",
                ],
        "The question's choices and buttons did not answer for the question shown.",
    )?;
    pet.set_card(card("smoke-approval", "waiting", Some(approval), 0, 0, false), true);
    pet.draw(6, 0).await?;
    let before = lock(&pet.clicks).len();
    let mut pressed = true;
    for id in ["allow", "review", "deny", "open"] {
        pressed &= pet.click(id).await?;
    }
    require(
        pressed
            && lock(&pet.clicks)[before..]
                == ["smoke-approval:allow", "smoke-approval:review", "smoke-approval:deny", "smoke-approval:open"],
        "The approval's buttons did not answer for the approval shown.",
    )?;
    // A request that cannot be allowed here keeps Allow once, off: no press reaches it, and there is no view to review it in.
    let blocked = ToolPermissionRequest { can_allow: false, ..approval.clone() };
    pet.set_card(card("smoke-approval", "waiting", Some(&blocked), 0, 0, false), true);
    pet.draw(6, 0).await?;
    let before = lock(&pet.clicks).len();
    require(
        !pet.click("allow").await?
            && !pet.click("review").await?
            && pet.click("deny").await?
            && lock(&pet.clicks).len() == before + 1
            && pet.texts().iter().any(|t| t.value == mighty_run::presentation::strings::button_allow_once()),
        "A request that cannot be allowed must show Allow once without taking a press.",
    )?;
    pet.set_card(card("smoke-plan", "waiting", Some(plan), 0, 0, false), true);
    pet.draw(6, 0).await?;
    let before = lock(&pet.clicks).len();
    let mut pressed = true;
    for id in ["plan-approve", "plan-approve-confirm", "plan-revise", "plan-review", "plan-cancel", "open"] {
        pressed &= pet.click(id).await?;
    }
    pressed &= !pet.click("deny").await?;
    require(
        pressed
            && lock(&pet.clicks)[before..]
                == [
                    "smoke-plan:plan-approve",
                    "smoke-plan:plan-approve-confirm",
                    "smoke-plan:plan-revise",
                    "smoke-plan:plan-review",
                    "smoke-plan:plan-cancel",
                    "smoke-plan:open",
                ],
        "The plan's six buttons did not answer for the plan shown.",
    )?;
    let focus = pet.focus();
    require(
        focus.foreground == front && focus.foreground != focus.own && !focus.took_thread,
        "Companion requests acquired foreground focus.",
    )?;
    // A plan longer than the taller window has room for scrolls inside the bubble: the bubble stays within that window, the wheel
    // moves the plan to its last step, and the answers stay under it.
    let steps: Vec<String> = (1..=60).map(|i| format!("Long plan step {i}")).collect();
    let numbered: Vec<String> = steps.iter().enumerate().map(|(i, step)| format!("{}. {step}", i + 1)).collect();
    let long_plan = ToolPermissionRequest {
        id: "smoke-plan-long".into(),
        input_json: json!({ "plan": format!("# A long plan\n\n{}", numbered.join("\n")) }).to_string(),
        ..plan.clone()
    };
    pet.set_card(card("smoke-plan-long", "waiting", Some(&long_plan), 0, 0, false), true);
    pet.draw(6, 0).await?;
    let scrolling = lock(&pet.overlay).plan_box();
    let room = layout::TALL_HEIGHT - layout::CHROME;
    require(
        lock(&pet.overlay).geometry().height == layout::TALL_HEIGHT
            && f64::from(lock(&pet.overlay).placement().height) <= room
            && scrolling.extent > scrolling.area.height
            && scrolling.scroll == 0.0,
        &format!(
            "a long plan must scroll inside a bubble that fits the taller window; got {} of plan in {}",
            scrolling.extent, scrolling.area.height
        ),
    )?;
    let texts = pet.texts();
    require(
        texts.iter().any(|t| t.value == steps[0]) && !texts.iter().any(|t| t.value == steps[59]),
        "a long plan must start at its first step",
    )?;
    let middle = LogicalPoint { x: scrolling.area.x + scrolling.area.width / 2.0, y: scrolling.area.y + scrolling.area.height / 2.0 };
    pet.send(PetInput::Wheel { at: middle, delta: -120 }, 1).await?;
    pet.draw(6, 0).await?;
    let once = lock(&pet.overlay).plan_box().scroll;
    require(
        once > 0.0 && once < scrolling.extent - scrolling.area.height,
        &format!("one notch of the wheel must move the plan a little; got {once}"),
    )?;
    for _ in 0..60 {
        pet.send(PetInput::Wheel { at: middle, delta: -120 }, 1).await?;
    }
    let frame = pet.draw(6, 0).await?;
    let end = lock(&pet.overlay).plan_box();
    let texts = pet.texts();
    let last = texts.iter().find(|t| t.value == steps[59]);
    require(
        (end.scroll - (end.extent - end.area.height)).abs() < 0.01
            && last.is_some_and(|t| t.bounds.bottom() <= end.area.bottom() + 0.01)
            && !texts.iter().any(|t| t.value == steps[0]),
        &format!("the wheel must bring the plan's last step into the bubble; scrolled {} of {}", end.scroll, end.extent - end.area.height),
    )?;
    let before = lock(&pet.clicks).len();
    require(
        pet.click("plan-approve").await?
            && pet.click("plan-cancel").await?
            && lock(&pet.clicks).len() == before + 2
            && pet.focus().foreground == front
            && !pet.focus().took_thread,
        "A scrolled plan must keep its answers, and the wheel must not bring the pet forward.",
    )?;
    screenshots.push(capture(host, &frame, "smoke-companion-plan-long-dark.png")?);
    // The same presses through the app's own handler, short of sending: a multi-choice press toggles, Next moves on, Back shows the
    // earlier picks again, and a press that is no part of the question is left to the request's own buttons.
    let companion = crate::companion::app(host).ok_or("no companion")?;
    let both = mighty_run::strings::get_with("companion.question.titleProgress", &[("current", "2"), ("total", "2")]);
    let request = |host: &Host| {
        crate::companion::card_for(host, "smoke-question", agent, "waiting", Some(question), 0, 0, false).approval.unwrap_or_default()
    };
    require(
        crate::companion::answer_question(host, &companion, question, &form, "choice-1").await
            && request(host).options.iter().all(|o| o.picked),
        "A press on a multi-choice answer must toggle it.",
    )?;
    let second = {
        crate::companion::answer_question(host, &companion, question, &form, bubble::NEXT).await;
        request(host)
    };
    require(
        second.title == both && second.buttons.iter().any(|b| b.id == bubble::BACK) && !second.options.iter().any(|o| o.picked),
        "Next must move the bubble to the second question.",
    )?;
    let first = {
        crate::companion::answer_question(host, &companion, question, &form, bubble::BACK).await;
        request(host)
    };
    require(
        first.options.iter().all(|o| o.picked) && !first.buttons.iter().any(|b| b.id == bubble::BACK),
        "Back must return to the first question with its picks.",
    )?;
    require(
        !crate::companion::answer_question(host, &companion, question, &form, bubble::DENY).await
            && !crate::companion::answer_question(host, &companion, question, &form, bubble::OPEN).await,
        "Cancel and open are not answers to the question.",
    )?;
    // Without a pet image the paw stands in the pet's frame, in the accent with 35 clear around it.
    let bare = Pet::open(host, None).await?;
    bare.set_card(CompanionCard { light: false, ..card("smoke-paw", agent.status(), None, 0, 0, false) }, false);
    let scene = lock(&bare.overlay).scene();
    let front = bare.focus().foreground;
    bare.window.show(&scene).map_err(|e| e.to_string())?;
    let frame = bare.draw(0, 0).await?;
    let focus = bare.focus();
    require(
        focus.foreground == front && focus.foreground != focus.own && !focus.took_thread && focus.non_activating,
        "The pet's window without a pet image acquired foreground focus.",
    )?;
    screenshots.push(capture(host, &frame, "smoke-companion-paw-dark.png")?);
    let geometry = lock(&bare.overlay).geometry();
    let (paw, scale) = (geometry.pet, geometry.scale);
    let accent = DARK[Token::Accent];
    let middle = at(&frame, scale, paw.x + paw.width / 2.0, paw.y + 40.0 + 39.5);
    require(
        middle == (accent.b, accent.g, accent.r, 255)
            && at(&frame, scale, paw.x + 20.0, paw.y + 20.0).3 == 0
            && at(&frame, scale, paw.right() - 20.0, paw.bottom() - 20.0).3 == 0,
        &format!("Without a pet image the paw must stand in the pet's frame, in the accent; got {middle:?}."),
    )?;
    Ok(())
}

/// A check of what the page draws: `body` runs in the page with `until(test, ms)` and fills `r`, handed back through the bridge's `log`
/// method and read from the host's log (as smoke_delegation.rs's probe).
async fn page_probe(host: &Host, body: &str) -> Result<Value, String> {
    let mark = format!("smoke-companion-dom-{}-{}", std::process::id(), wire::new_id());
    let nonce = crate::host_nonce(&host.app);
    let script = format!(
        "(async () => {{ const until = async (test, ms) => {{ const end = performance.now() + ms; while (performance.now() < end) {{ const v = await test(); if (v) return v; await new Promise((r) => setTimeout(r, 100)); }} return test(); }}; const r = {{}}; try {{ {body} }} catch (error) {{ r.error = String(error); }} await window.__TAURI_INTERNALS__.invoke('host_post', {{ nonce: '{nonce}', message: {{ kind: 'call', id: 990000 + Math.floor(Math.random() * 9999), method: 'log', args: {{ level: 'info', message: '{mark} ' + JSON.stringify(r) }} }} }}); }})()"
    );
    let page = host.app.get_webview(crate::host::WINDOW).ok_or("no main window")?;
    page.eval(&script).map_err(|e| e.to_string())?;
    let log = host.log.dir().join("host.log");
    let needle = format!("web info: {mark} ");
    let line = wait("the page probe's answer", 40, || {
        std::fs::read_to_string(&log).ok()?.lines().find_map(|line| line.split_once(&needle).map(|(_, rest)| rest.to_string()))
    })
    .await?;
    serde_json::from_str(&line).map_err(|e| format!("the page probe's answer is no JSON: {e}"))
}

/// The status bar's companion controls (C#'s dialogs smoke, `checks["companion"]`): the paw drawn, the popover opened in the page from a
/// right click on it (MC-92: the status capsule that opened it gave its place to the bell), and Esc closing it.
async fn status_popover(host: &Arc<Host>) -> Result<bool, String> {
    let opened = page_probe(
        host,
        "r.paw = !!(await until(() => document.querySelector('[data-testid=companion-toggle]'), 15000)); \
         document.querySelector('[data-testid=companion-toggle]')?.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true })); \
         r.popover = !!(await until(() => document.querySelector('[data-native-view=companion] [data-native-id]'), 10000));",
    )
    .await?;
    let shown = opened["paw"] == true && opened["popover"] == true && crate::native::is_open(host, crate::companion::SURFACE);
    let closed = page_probe(
        host,
        "document.querySelector('[data-native-view=companion]')?.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true })); \
         r.gone = !!(await until(() => !document.querySelector('[data-native-view=companion]'), 10000));",
    )
    .await?;
    let gone = closed["gone"] == true
        && wait("the popover closed", 10, || (!crate::native::is_open(host, crate::companion::SURFACE)).then_some(())).await.is_ok();
    if !(shown && gone) {
        host.log.line(&format!("smoke: companion popover: {opened} {closed}"));
    }
    Ok(shown && gone)
}
