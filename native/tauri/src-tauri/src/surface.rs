//! The host side of the NativeSlot protocol (docs/tauri-architecture.md §3): native views laid over the slots the page reserves.
//!
//! - `host` push: when the page calls `ready`, before the snapshot, the host says who draws what:
//!   `{kind:'host', platform:'windows'|'macos', surfaces:{terminal:'web'|'native'}}`.
//! - `surfaceBounds{paneId, surface, rect:{x,y,width,height} (device px), visible, occluded}`: where a slot is now. The view is shown
//!   at `rect` while `visible && !occluded` and the rectangle is not empty, else hidden. The page sends one per frame at most, and
//!   `visible:false` when the slot goes away.
//! - `surfaceAction{paneId, surface, action:'focus'|'snapshot'}`: focus answers null; snapshot answers the view's picture as a PNG
//!   `data:` URL, or null when the platform has none (the page asks just before it reports the slot occluded).
//!
//! The only native pane view is the Mac's terminal (Ghostty); 0.5 has no browser pane (C#'s `browserBounds`, `browserOpen`, ...: a web
//! page opens in the user's default browser). The manager keeps one slot per (pane, kind). A view opens the first time its slot can
//! show, hides when the slot cannot, hides on a page reload until the page reports again, and closes when its pane is gone from the
//! state.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use mighty_platform::{DeviceRect, Drawing, NativeSurface, Os, SurfaceFactory, SurfaceKind, TerminalSurface};
use serde_json::{Value, json};
use tauri::{Emitter, Manager};
use tokio::sync::Mutex;

use crate::host::{EVENT, Host, Registry, Reply, WINDOW};

/// Where host → page messages of the surfaces go.
pub type Push = Arc<dyn Fn(Value) + Send + Sync>;
/// One `surfaceBounds` report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SlotReport {
    pub rect: DeviceRect,
    pub visible: bool,
    pub occluded: bool,
}

impl SlotReport {
    pub fn shows(&self) -> bool {
        self.visible && !self.occluded && !self.rect.is_empty()
    }

    /// `{rect:{x,y,width,height}, visible, occluded}`, whole device pixels (a missing or broken rect is empty).
    pub fn parse(args: &Value) -> SlotReport {
        let rect = args.get("rect");
        let number = |name: &str| rect.and_then(|r| r.get(name)).and_then(Value::as_f64).filter(|n| n.is_finite()).unwrap_or(0.0).round();
        let size = |name: &str| number(name).clamp(0.0, f64::from(u32::MAX)) as u32;
        let position = |name: &str| number(name).clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32;
        SlotReport {
            rect: DeviceRect { x: position("x"), y: position("y"), width: size("width"), height: size("height") },
            visible: args.get("visible") == Some(&Value::Bool(true)),
            occluded: args.get("occluded") == Some(&Value::Bool(true)),
        }
    }
}

/// A slot's view, shared so that a wait on it (a snapshot, an inspect) happens with the slots' lock let go.
enum View {
    /// The Mac's Ghostty view, opened by its pane's terminal (the Mac stage); Windows draws terminals in the page.
    Terminal(Arc<dyn TerminalSurface>),
}

impl View {
    fn native(&self) -> &dyn NativeSurface {
        match self {
            View::Terminal(view) => view.as_ref(),
        }
    }

    fn shared(&self) -> Arc<dyn NativeSurface> {
        match self {
            View::Terminal(view) => view.clone(),
        }
    }
}

#[derive(Default)]
struct Slot {
    report: Option<SlotReport>,
    view: Option<View>,
    /// What the view was last told: shown, and where.
    shown: bool,
    placed: Option<DeviceRect>,
}

type Key = (String, SurfaceKind);

pub struct SurfaceManager {
    factory: Arc<dyn SurfaceFactory>,
    os: Os,
    push: Push,
    slots: Mutex<HashMap<Key, Slot>>,
}

impl SurfaceManager {
    pub fn new(factory: Arc<dyn SurfaceFactory>, os: Os, push: Push) -> SurfaceManager {
        SurfaceManager { factory, os, push, slots: Mutex::new(HashMap::new()) }
    }

    /// The `host` push.
    pub fn capabilities(&self) -> Value {
        let drawing = |kind| self.factory.drawing(kind).name();
        json!({
            "kind": "host",
            "platform": self.os.name(),
            "surfaces": {"terminal": drawing(SurfaceKind::Terminal)},
        })
    }

    /// Pushes the `host` message (see the module comment).
    pub fn announce(&self) {
        (self.push)(self.capabilities());
    }

    pub async fn bounds(&self, pane: &str, kind: SurfaceKind, report: SlotReport) -> Result<(), String> {
        if self.factory.drawing(kind) == Drawing::Web {
            return Ok(());
        }
        let mut slots = self.slots.lock().await;
        let slot = slots.entry((pane.to_string(), kind)).or_default();
        slot.report = Some(report);
        self.apply(pane, slot).await
    }

    /// Opens, places, shows or hides the slot's view to match its last report.
    async fn apply(&self, pane: &str, slot: &mut Slot) -> Result<(), String> {
        let Some(report) = slot.report else { return Ok(()) };
        let show = report.shows();
        if slot.view.is_none() {
            // The Mac's Ghostty view, fed by its pane's PTY (surface_terminal.rs, M-D; none before then, so the slot stays empty).
            // Windows never gets here: its page draws terminals (`bounds` returns before).
            if show && let Some(view) = crate::surface_terminal::open(&self.factory, pane, report.rect).await {
                slot.view = Some(View::Terminal(view));
                slot.shown = true;
                slot.placed = Some(report.rect);
            }
            return Ok(());
        }
        let Some(view) = &slot.view else { return Ok(()) };
        let native = view.native();
        if show {
            // Moved before it shows, so it never flashes where it was.
            if slot.placed != Some(report.rect) {
                native.place(report.rect).map_err(|e| e.to_string())?;
                slot.placed = Some(report.rect);
            }
            if !slot.shown {
                native.set_visible(true).map_err(|e| e.to_string())?;
                slot.shown = true;
            }
        } else if slot.shown {
            native.set_visible(false).map_err(|e| e.to_string())?;
            slot.shown = false;
        }
        Ok(())
    }

    /// The slot's view, the slots' lock let go again (for a wait on the view: every other slot's report goes on meanwhile).
    async fn view_of(&self, pane: &str, kind: SurfaceKind) -> Option<Arc<dyn NativeSurface>> {
        self.slots.lock().await.get(&(pane.to_string(), kind)).and_then(|slot| slot.view.as_ref()).map(View::shared)
    }

    /// `focus` → null; `snapshot` → a `data:` URL or null (waited for with the slots' lock let go: up to 2 s). A slot without a view
    /// answers null.
    pub async fn action(&self, pane: &str, kind: SurfaceKind, action: &str) -> Reply {
        let Some(view) = self.view_of(pane, kind).await else { return Ok(Value::Null) };
        match action {
            "focus" => view.focus().map(|()| Value::Null).map_err(|e| e.to_string()),
            "snapshot" => Ok(view.snapshot().await.map_err(|e| e.to_string())?.map(Value::String).unwrap_or(Value::Null)),
            other => Err(format!("unknown surface action: {other}")),
        }
    }

    /// Closes the views of panes no longer in the state.
    pub async fn retain(&self, live: &HashSet<String>) {
        let mut slots = self.slots.lock().await;
        let gone: Vec<Key> = slots.keys().filter(|(pane, _)| !live.contains(pane)).cloned().collect();
        for key in gone {
            if let Some(view) = slots.remove(&key).and_then(|slot| slot.view) {
                let _ = view.native().close();
            }
        }
    }

    /// The page is loading again: every view hides until its slot is reported again.
    pub async fn page_started(&self) {
        let mut slots = self.slots.lock().await;
        for slot in slots.values_mut() {
            slot.report = None;
            if let Some(view) = &slot.view
                && slot.shown
            {
                let _ = view.native().set_visible(false);
                slot.shown = false;
            }
        }
    }

    /// What the OS's view really is (the tests' check); None without a view.
    #[cfg(test)]
    pub async fn inspect(&self, pane: &str, kind: SurfaceKind) -> Option<mighty_platform::SurfaceState> {
        self.view_of(pane, kind).await?.inspect().await.ok()
    }
}

/// The pane ids of the state now.
pub fn live_panes(host: &Host) -> HashSet<String> {
    host.view().snapshot.sessions().iter().map(|session| session.id().to_string()).collect()
}

/// Pushes straight to the page's event (the host's ordered outbox is host.rs's own). Each surface push carries its whole state, so
/// its order relative to call results does not matter; the `host` push is emitted before `ready` is handled, so it reaches the
/// page before the snapshot.
pub fn page_push(app: tauri::AppHandle) -> Push {
    Arc::new(move |message| {
        let _ = app.emit_to(WINDOW, EVENT, &message);
    })
}

fn manager(host: &Host) -> Result<Arc<SurfaceManager>, String> {
    host.app.try_state::<Arc<SurfaceManager>>().map(|state| state.inner().clone()).ok_or_else(|| "no native surfaces".to_string())
}

fn pane(args: &Value) -> Result<String, String> {
    args.get("paneId")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .ok_or_else(|| "paneId is missing".to_string())
}

fn kind(args: &Value) -> Result<SurfaceKind, String> {
    args.get("surface").and_then(Value::as_str).and_then(SurfaceKind::parse).ok_or_else(|| "unknown surface".to_string())
}

/// The page methods of the NativeSlot protocol.
pub fn register(methods: &mut Registry) {
    methods.add("surfaceBounds", |host: Arc<Host>, args| async move {
        manager(&host)?.bounds(&pane(&args)?, kind(&args)?, SlotReport::parse(&args)).await.map(|()| Value::Null)
    });
    methods.add("surfaceAction", |host: Arc<Host>, args| async move {
        let action = args.get("action").and_then(Value::as_str).unwrap_or("").to_string();
        manager(&host)?.action(&pane(&args)?, kind(&args)?, &action).await
    });
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex as StdMutex;

    use mighty_platform::fake::{FakeSurfaces, SurfaceEvent};

    use super::*;

    struct Harness {
        surfaces: Arc<FakeSurfaces>,
        pushed: Arc<StdMutex<Vec<Value>>>,
        manager: SurfaceManager,
    }

    /// The manager over the fake's views; with `native_terminal` a terminal is a native view (the Mac), its events going nowhere.
    fn harness(native_terminal: bool) -> Harness {
        crate::surface_terminal::natives().connect(Arc::new(|_, _| Box::pin(async {})));
        let surfaces = Arc::new(FakeSurfaces::new(native_terminal));
        let pushed = Arc::new(StdMutex::new(vec![]));
        let sink = pushed.clone();
        let os = if native_terminal { Os::Macos } else { Os::Windows };
        let manager = SurfaceManager::new(surfaces.clone(), os, Arc::new(move |m| sink.lock().unwrap().push(m)));
        Harness { surfaces, pushed, manager }
    }

    const RECT: DeviceRect = DeviceRect { x: 10, y: 20, width: 300, height: 200 };
    const TERMINAL: SurfaceKind = SurfaceKind::Terminal;

    fn shown(rect: DeviceRect) -> SlotReport {
        SlotReport { rect, visible: true, occluded: false }
    }

    #[test]
    fn reports_parse_whole_device_pixels() {
        let report =
            SlotReport::parse(&json!({"rect": {"x": 10.4, "y": -3, "width": 300.6, "height": -5}, "visible": true, "occluded": false}));
        assert_eq!(report, SlotReport { rect: DeviceRect { x: 10, y: -3, width: 301, height: 0 }, visible: true, occluded: false });
        assert!(!report.shows(), "an empty rectangle never shows");
        assert_eq!(SlotReport::parse(&json!({})), SlotReport::default());
    }

    #[test]
    fn the_host_push_says_who_draws_what() {
        let h = harness(false);
        assert_eq!(h.manager.capabilities(), json!({"kind": "host", "platform": "windows", "surfaces": {"terminal": "web"}}));
        h.manager.announce();
        assert_eq!(h.pushed.lock().unwrap()[0]["kind"], "host");
        assert_eq!(harness(true).manager.capabilities()["surfaces"], json!({"terminal": "native"}));
    }

    #[tokio::test]
    async fn a_view_opens_at_its_slot_then_follows_it() {
        let h = harness(true);
        h.manager.bounds("surface-follows", TERMINAL, shown(RECT)).await.unwrap();
        assert_eq!(h.surfaces.events(), [SurfaceEvent::Opened { pane: "surface-follows".into(), kind: TERMINAL, bounds: RECT }]);
        h.surfaces.clear();

        // The same rectangle again does nothing; a moved one is placed.
        h.manager.bounds("surface-follows", TERMINAL, shown(RECT)).await.unwrap();
        let moved = DeviceRect { y: 40, ..RECT };
        h.manager.bounds("surface-follows", TERMINAL, shown(moved)).await.unwrap();
        assert_eq!(h.surfaces.events(), [SurfaceEvent::Placed { pane: "surface-follows".into(), rect: moved }]);
        assert_eq!(h.manager.inspect("surface-follows", TERMINAL).await.unwrap().bounds, moved);
    }

    #[tokio::test]
    async fn an_occluded_or_hidden_slot_hides_its_view_until_it_shows_again() {
        let h = harness(true);
        let pane = "surface-occluded";
        h.manager.bounds(pane, TERMINAL, shown(RECT)).await.unwrap();
        h.surfaces.clear();
        let occluded = SlotReport { occluded: true, ..shown(RECT) };
        h.manager.bounds(pane, TERMINAL, occluded).await.unwrap();
        h.manager.bounds(pane, TERMINAL, occluded).await.unwrap();
        h.manager.bounds(pane, TERMINAL, SlotReport { visible: false, ..shown(RECT) }).await.unwrap();
        h.manager.bounds(pane, TERMINAL, shown(RECT)).await.unwrap();
        assert_eq!(
            h.surfaces.events(),
            [SurfaceEvent::Visible { pane: pane.into(), visible: false }, SurfaceEvent::Visible { pane: pane.into(), visible: true }],
            "hidden once, shown once, never placed again at the same rectangle"
        );
        assert!(h.manager.inspect(pane, TERMINAL).await.unwrap().visible);
    }

    #[tokio::test]
    async fn a_view_waits_for_its_slot_to_show() {
        let h = harness(true);
        h.manager.bounds("surface-waits", TERMINAL, SlotReport { occluded: true, ..shown(RECT) }).await.unwrap();
        assert!(h.surfaces.events().is_empty(), "nothing opens under a page dialog");
        h.manager.bounds("surface-waits", TERMINAL, shown(RECT)).await.unwrap();
        assert!(matches!(h.surfaces.events()[0], SurfaceEvent::Opened { .. }));
    }

    #[tokio::test]
    async fn a_reload_hides_every_view_and_a_closed_pane_closes_its_view() {
        let h = harness(true);
        let pane = "surface-reload";
        h.manager.bounds(pane, TERMINAL, shown(RECT)).await.unwrap();
        h.manager.page_started().await;
        assert_eq!(h.surfaces.events().last(), Some(&SurfaceEvent::Visible { pane: pane.into(), visible: false }));
        h.manager.bounds(pane, TERMINAL, shown(RECT)).await.unwrap();
        assert_eq!(h.surfaces.events().last(), Some(&SurfaceEvent::Visible { pane: pane.into(), visible: true }));
        h.manager.retain(&HashSet::from(["other".to_string()])).await;
        assert_eq!(h.surfaces.events().last(), Some(&SurfaceEvent::Closed { pane: pane.into() }));
        assert!(h.manager.inspect(pane, TERMINAL).await.is_none());
    }

    /// A snapshot is waited for with the slots' lock let go (a view's capture takes its time): every report goes on meanwhile.
    #[tokio::test]
    async fn a_snapshot_never_holds_the_slots() {
        crate::surface_terminal::natives().connect(Arc::new(|_, _| Box::pin(async {})));
        let surfaces = Arc::new(FakeSurfaces::new(true));
        let manager = Arc::new(SurfaceManager::new(surfaces.clone(), Os::Macos, Arc::new(|_| {})));
        let pane = "surface-snapshot";
        manager.bounds(pane, TERMINAL, shown(RECT)).await.unwrap();
        let gate = surfaces.hold_snapshots();
        let snapping = {
            let manager = manager.clone();
            tokio::spawn(async move { manager.action(pane, TERMINAL, "snapshot").await })
        };
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(!snapping.is_finished(), "the snapshot waits on the view");
        let moved = DeviceRect { y: 40, ..RECT };
        tokio::time::timeout(std::time::Duration::from_secs(2), manager.bounds(pane, TERMINAL, shown(moved)))
            .await
            .expect("a report while a snapshot waits")
            .unwrap();
        gate.add_permits(1);
        assert!(snapping.await.unwrap().unwrap().as_str().unwrap().starts_with("data:image/png"));
    }

    #[tokio::test]
    async fn actions_reach_the_view_and_web_kinds_are_left_to_the_page() {
        let h = harness(true);
        let pane = "surface-actions";
        assert_eq!(h.manager.action(pane, TERMINAL, "snapshot").await.unwrap(), Value::Null, "no view, no picture");
        h.manager.bounds(pane, TERMINAL, shown(RECT)).await.unwrap();
        assert!(h.manager.action(pane, TERMINAL, "snapshot").await.unwrap().as_str().unwrap().starts_with("data:image/png"));
        h.manager.action(pane, TERMINAL, "focus").await.unwrap();
        assert!(h.manager.action(pane, TERMINAL, "explode").await.is_err());
        // Windows draws the terminal in the page: its reports change nothing.
        let h = harness(false);
        h.manager.bounds("t", TERMINAL, shown(RECT)).await.unwrap();
        assert!(h.surfaces.events().is_empty());
    }
}
