//! M-F: the companion pet as an `NSPanel` `[.borderless, .nonactivatingPanel]`, clear and shadowless, `level=.floating`, a floating
//! panel that `becomesKeyOnlyIfNeeded`, `hidesOnDeactivate=false`, `[.canJoinAllSpaces, .fullScreenAuxiliary]` (Swift
//! `AgentCompanionViews.swift:600-667` `CompanionPanel`), drawing mighty-companion's `PetScene` with CoreGraphics and CoreText
//! ([`draw`], [`text`]). Contract `mighty_platform::PetWindow` (Windows: `mighty-platform-windows/src/pet.rs`, whose behaviour this
//! follows).
//!
//! - **Threads.** AppKit is the main thread's: the panel, its view and their changes run there (`run_on_main_thread`). The picture is
//!   drawn on the window's own render thread (`mighty-pet`), which takes the commands in order (a place never jumps ahead of a draw
//!   queued before it), draws each frame into a bitmap, keeps it for `capture` with its number, and hands the image and its place to
//!   the main thread together. Every call but `focus` returns without waiting for the main thread; `send` (the smoke's input) goes
//!   through the same handlers the view's events do and returns once they ran. The screens, their scale and the motion setting are
//!   read on the main thread when the panel opens and whenever the displays change, so `work_area` and `scale_at`, which the overlay
//!   asks while it holds its own lock, never wait for the main thread.
//! - **Units.** The overlay places the window in the desktop's own units at the window's scale (C#'s physical pixels). On the Mac
//!   these are points times the main display's backing scale, one scale for the whole desktop (`scale_at` answers it everywhere): a
//!   scene's origin and the pointer's screen place are in them, its size is its DIPs in points, and its pixels are drawn at its scale.
//!   The runner's display is at scale 1.0, where they are points (Retina and several displays are the user's checklist, plan §9).
//! - **Focus.** The panel never takes the keyboard or activates the app (a press on a non-activating panel does neither), it shows
//!   with `orderFrontRegardless`, and the frame autosave of Swift is left out: the overlay keeps the place in `companion-settings.json`
//!   (written only where the write policy writes), so the panel never writes the user's defaults.
//! - **Clicks** pass through where the picture is clear: the window server does not deliver a press on a transparent pixel of a
//!   non-opaque window (`ignoresMouseEvents` is never set, which would turn that off). Plan §1.6 has it toggled per hit as on Windows;
//!   the window server's own rule is kept instead, and a click through the pet's clear corners onto the app under it is on the user's
//!   §9 checklist (review item 15: CI's synthetic input reaches the view directly and cannot judge it).

mod draw;
mod text;

use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use mighty_platform::pet::PetMeasure;
use mighty_platform::surface::{PetCapture, PetCursor, PetFocus, PetInput, PetPoll};
use mighty_platform::{LogicalPoint, LogicalRect, PetEvent, PetPointer, PetScene, PetWindow, PlatformError, Result};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObjectProtocol, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSApplication, NSApplicationDidChangeScreenParametersNotification, NSBackingStoreType, NSColor, NSCursor, NSEvent,
    NSFloatingWindowLevel, NSPanel, NSResponder, NSScreen, NSTrackingArea, NSTrackingAreaOptions, NSView, NSWindowCollectionBehavior,
    NSWindowStyleMask, NSWorkspace,
};
use objc2_core_graphics::{CGEventSource, CGEventSourceStateID, CGEventType};
use objc2_foundation::{NSNotification, NSNotificationCenter, NSPoint, NSRect, NSSize};
use tokio::sync::mpsc;

pub use draw::symbol;
pub use text::CoreTextMeasure;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Keeps a Rust panic from unwinding across an Objective-C/AppKit FFI boundary.
fn guarded<R>(fallback: R, f: impl FnOnce() -> R) -> R {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).unwrap_or(fallback)
}

/// One display: its frame and the part not under the menu bar and the Dock, in the desktop's units, down from the top left.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Screen {
    pub frame: LogicalRect,
    pub visible: LogicalRect,
}

/// What the displays are: each one, the main display's height in points (the flip between AppKit's bottom-left space and the
/// desktop's top-left one) and the desktop's units a point.
#[derive(Debug, Clone, PartialEq)]
pub struct Desktop {
    pub screens: Vec<Screen>,
    pub height: f64,
    pub units: f64,
}

impl Default for Desktop {
    fn default() -> Desktop {
        let all = LogicalRect { x: 0.0, y: 0.0, width: 1440.0, height: 900.0 };
        Desktop { screens: vec![Screen { frame: all, visible: all }], height: 900.0, units: 1.0 }
    }
}

impl Desktop {
    /// AppKit's screens (main thread): the first is the main display, whose bottom left is the origin of AppKit's space.
    fn read(mtm: MainThreadMarker) -> Desktop {
        let screens = NSScreen::screens(mtm);
        let Some(main) = screens.iter().next() else { return Desktop::default() };
        let height = main.frame().size.height;
        let units = main.backingScaleFactor().clamp(1.0, 4.0);
        let flip = |r: NSRect| LogicalRect {
            x: r.origin.x * units,
            y: (height - r.origin.y - r.size.height) * units,
            width: r.size.width * units,
            height: r.size.height * units,
        };
        let screens = screens.iter().map(|s| Screen { frame: flip(s.frame()), visible: flip(s.visibleFrame()) }).collect();
        Desktop { screens, height, units }
    }

    /// The display a point of the desktop is on, else the nearest one (`MONITOR_DEFAULTTONEAREST`).
    pub fn screen_at(&self, near: LogicalPoint) -> Screen {
        let distance = |r: &LogicalRect| {
            let dx = (r.x - near.x).max(0.0).max(near.x - (r.x + r.width));
            let dy = (r.y - near.y).max(0.0).max(near.y - (r.y + r.height));
            dx * dx + dy * dy
        };
        self.screens
            .iter()
            .copied()
            .min_by(|a, b| distance(&a.frame).total_cmp(&distance(&b.frame)))
            .unwrap_or_else(|| Desktop::default().screens[0])
    }

    /// A point of AppKit's space (points, up from the main display's bottom) in the desktop's units.
    pub fn from_appkit(&self, point: NSPoint) -> LogicalPoint {
        LogicalPoint { x: point.x * self.units, y: (self.height - point.y) * self.units }
    }

    /// The panel's frame in AppKit's space for a window whose top left is `origin` (desktop units) and whose size is `width × height`
    /// points.
    pub fn frame(&self, origin: LogicalPoint, width: f64, height: f64) -> NSRect {
        let (x, top) = (origin.x / self.units, origin.y / self.units);
        NSRect::new(NSPoint::new(x, self.height - top - height), NSSize::new(width, height))
    }
}

/// The window's event channel: the sender its view keeps, the receiver until `events` hands it over.
type Events = (mpsc::UnboundedSender<PetEvent>, Option<mpsc::UnboundedReceiver<PetEvent>>);

/// What the panel's view, the render thread and the port share.
pub struct Shared {
    events: Mutex<Option<Events>>,
    closed: AtomicBool,
    capture: Mutex<PetCapture>,
    capture_wanted: AtomicBool,
    frames: AtomicU64,
    cursor: Mutex<PetCursor>,
    /// The scale of the scene last drawn and its origin (desktop units), as f64 bits.
    scale: AtomicU64,
    left: AtomicU64,
    top: AtomicU64,
    desktop: Mutex<Desktop>,
    reduce_motion: AtomicBool,
    window_number: AtomicIsize,
    /// The pointer is over the panel (its cursor is the overlay's).
    hovering: AtomicBool,
    /// The trackpad's scroll not yet a notch.
    wheel: Mutex<f64>,
    /// The session's press counters at the last `poll`.
    presses: Mutex<u32>,
    text: Arc<text::Text>,
}

impl Shared {
    fn emit(&self, event: PetEvent) {
        if let Some((sender, _)) = lock(&self.events).as_ref() {
            let _ = sender.send(event);
        }
    }

    fn scale(&self) -> f64 {
        f64::from_bits(self.scale.load(Ordering::SeqCst))
    }

    fn origin(&self) -> LogicalPoint {
        LogicalPoint { x: f64::from_bits(self.left.load(Ordering::SeqCst)), y: f64::from_bits(self.top.load(Ordering::SeqCst)) }
    }

    fn open(&self) -> Result<()> {
        if self.closed.load(Ordering::SeqCst) { Err(PlatformError::Refused("the pet window is closed".into())) } else { Ok(()) }
    }

    /// The pointer at a point of the window (DIPs): there, and on the desktop at the window's place and scale (C#'s `pointer`).
    fn pointer_at(&self, window: LogicalPoint) -> PetPointer {
        let (origin, scale) = (self.origin(), self.scale());
        PetPointer { window, screen: LogicalPoint { x: origin.x + window.x * scale, y: origin.y + window.y * scale } }
    }

    /// One input, from the view or from `send`, as the event Windows' window procedure makes of it.
    fn input(&self, input: PetInput) {
        match input {
            PetInput::Click(at) => {
                self.emit(PetEvent::Pressed(self.pointer_at(at)));
                self.emit(PetEvent::Released(self.pointer_at(at)));
            }
            PetInput::Press(at) => self.emit(PetEvent::Pressed(self.pointer_at(at))),
            PetInput::Drag(at) => self.emit(PetEvent::Moved(self.pointer_at(at))),
            PetInput::Release(at) => self.emit(PetEvent::Released(self.pointer_at(at))),
            PetInput::DoubleClick(at) => self.emit(PetEvent::DoubleClicked(self.pointer_at(at))),
            PetInput::RightClick(at) => self.emit(PetEvent::RightClicked(self.pointer_at(at))),
            PetInput::Wheel { at, delta } => self.emit(PetEvent::Wheel { pointer: self.pointer_at(at), delta }),
            PetInput::Dpi(dpi) => self.emit(PetEvent::DpiChanged { scale: (f64::from(dpi) / 96.0).clamp(1.0, 4.0) }),
        }
    }

    /// An AppKit pointer event in the view: its place in the window, and on the desktop where the pointer is.
    fn pointer(&self, view: &PetView, event: &NSEvent) -> PetPointer {
        // The view is not flipped (its layer then shows the picture upright): its points run up from the bottom.
        let local = view.convertPoint_fromView(event.locationInWindow(), None);
        let screen = lock(&self.desktop).from_appkit(NSEvent::mouseLocation());
        PetPointer { window: LogicalPoint { x: local.x, y: view.bounds().size.height - local.y }, screen }
    }
}

/// A main-thread object carried by the port and its closures, used only on the main thread.
struct MainOnly<T>(T);
// SAFETY: only touched inside closures `run_on_main_thread` runs, and dropped there (`Close`).
unsafe impl<T> Send for MainOnly<T> {}
// SAFETY: as above.
unsafe impl<T> Sync for MainOnly<T> {}

/// The panel, its view and the display observer (main thread).
struct Panel {
    panel: Retained<NSPanel>,
    view: Retained<PetView>,
    observer: Option<Retained<ProtocolObject<dyn NSObjectProtocol>>>,
    shown: bool,
}

pub struct Ivars {
    shared: Arc<Shared>,
    /// The picture shown and the pixels a point it is drawn at (given back to the layer whenever AppKit asks it to redraw).
    image: std::cell::RefCell<Option<objc2_core_foundation::CFRetained<objc2_core_graphics::CGImage>>>,
    scale: std::cell::Cell<f64>,
}

define_class!(
    // SAFETY: NSView has no subclassing requirement this class breaks; it does not implement Drop.
    #[unsafe(super(NSView, NSResponder))]
    #[thread_kind = MainThreadOnly]
    #[name = "MightyPetView"]
    #[ivars = Ivars]
    pub struct PetView;

    impl PetView {
        /// The layer's contents are the render thread's picture: AppKit never draws into it ([`PetView::update_layer`]).
        #[unsafe(method(wantsUpdateLayer))]
        fn wants_update_layer(&self) -> bool {
            guarded(true, || true)
        }

        #[unsafe(method(updateLayer))]
        fn update_layer(&self) {
            guarded((), || self.apply());
        }

        /// A press on the pet acts at once even while another app is in front (the panel never activates this one).
        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            guarded(true, || true)
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            guarded((), || {
                let shared = &self.ivars().shared;
                let pointer = shared.pointer(self, event);
                // Windows' second press of a double click is WM_LBUTTONDBLCLK instead of a press.
                shared.emit(if event.clickCount() % 2 == 0 { PetEvent::DoubleClicked(pointer) } else { PetEvent::Pressed(pointer) });
            });
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &NSEvent) {
            guarded((), || {
                let shared = &self.ivars().shared;
                shared.emit(PetEvent::Released(shared.pointer(self, event)));
            });
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            guarded((), || {
                let shared = &self.ivars().shared;
                shared.emit(PetEvent::Moved(shared.pointer(self, event)));
            });
        }

        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, event: &NSEvent) {
            guarded((), || {
                let shared = &self.ivars().shared;
                shared.emit(PetEvent::Moved(shared.pointer(self, event)));
            });
        }

        #[unsafe(method(rightMouseDown:))]
        fn right_mouse_down(&self, _event: &NSEvent) {}

        #[unsafe(method(rightMouseUp:))]
        fn right_mouse_up(&self, event: &NSEvent) {
            guarded((), || {
                let shared = &self.ivars().shared;
                shared.emit(PetEvent::RightClicked(shared.pointer(self, event)));
            });
        }

        /// A notch of a wheel is Windows' 120; a trackpad's scroll counts as a notch every 30 points of it, its momentum ignored.
        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, event: &NSEvent) {
            guarded((), || {
                let shared = &self.ivars().shared;
                let delta = if event.hasPreciseScrollingDeltas() {
                    if event.momentumPhase().0 != 0 {
                        return;
                    }
                    let mut kept = lock(&shared.wheel);
                    *kept += event.scrollingDeltaY();
                    if kept.abs() < 30.0 {
                        return;
                    }
                    let notch = if *kept > 0.0 { 120 } else { -120 };
                    *kept = 0.0;
                    notch
                } else {
                    (event.scrollingDeltaY() * 120.0).round() as i32
                };
                if delta != 0 {
                    shared.emit(PetEvent::Wheel { pointer: shared.pointer(self, event), delta });
                }
            });
        }

        #[unsafe(method(mouseEntered:))]
        fn mouse_entered(&self, _event: &NSEvent) {
            guarded((), || {
                self.ivars().shared.hovering.store(true, Ordering::SeqCst);
                set_cursor(*lock(&self.ivars().shared.cursor));
            });
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &NSEvent) {
            guarded((), || {
                self.ivars().shared.hovering.store(false, Ordering::SeqCst);
                set_cursor(PetCursor::Arrow);
            });
        }

        #[unsafe(method(updateTrackingAreas))]
        fn update_tracking_areas(&self) {
            guarded((), || {
                // SAFETY: NSView's own handling first.
                let () = unsafe { msg_send![super(self), updateTrackingAreas] };
                for area in self.trackingAreas().iter() {
                    self.removeTrackingArea(&area);
                }
                self.track();
            });
        }
    }
);

impl PetView {
    fn new(mtm: MainThreadMarker, shared: Arc<Shared>, frame: NSRect) -> Retained<PetView> {
        let this =
            mtm.alloc::<PetView>().set_ivars(Ivars { shared, image: std::cell::RefCell::new(None), scale: std::cell::Cell::new(1.0) });
        // SAFETY: NSView's designated initializer.
        let view: Retained<PetView> = unsafe { msg_send![super(this), initWithFrame: frame] };
        view.setWantsLayer(true);
        view.track();
        view
    }

    /// The picture kept for the layer, set as its contents at its scale.
    fn show(&self, image: objc2_core_foundation::CFRetained<objc2_core_graphics::CGImage>, scale: f64) {
        *self.ivars().image.borrow_mut() = Some(image);
        self.ivars().scale.set(scale);
        self.apply();
    }

    fn apply(&self) {
        let image = self.ivars().image.borrow();
        let Some(image) = image.as_ref() else { return };
        // SAFETY: the view is layer-backed (made so); its layer takes a CGImage as its contents and the scale its pixels are at.
        unsafe {
            let layer: *mut AnyObject = msg_send![self, layer];
            if let Some(layer) = layer.as_ref() {
                let contents = objc2_core_foundation::CFRetained::as_ptr(image).as_ptr() as *mut AnyObject;
                let () = msg_send![layer, setContents: contents];
                let () = msg_send![layer, setContentsScale: self.ivars().scale.get()];
            }
        }
    }

    /// The pointer's moves and its coming and going, wherever the app is (the panel is never key).
    fn track(&self) {
        let options = NSTrackingAreaOptions::MouseMoved
            | NSTrackingAreaOptions::MouseEnteredAndExited
            | NSTrackingAreaOptions::ActiveAlways
            | NSTrackingAreaOptions::InVisibleRect;
        // SAFETY: a tracking area owned by this view (it outlives the area, which it removes on each update).
        let area = unsafe {
            NSTrackingArea::initWithRect_options_owner_userInfo(
                self.mtm().alloc(),
                self.bounds(),
                options,
                Some(&*(self as *const PetView as *const AnyObject)),
                None,
            )
        };
        self.addTrackingArea(&area);
    }
}

#[allow(deprecated, reason = "the frame-resize cursors that replace them are macOS 15's; the app runs from macOS 14")]
fn set_cursor(cursor: PetCursor) {
    let shape = match cursor {
        PetCursor::Arrow | PetCursor::SizeNorthWestSouthEast | PetCursor::SizeNorthEastSouthWest => NSCursor::arrowCursor(),
        PetCursor::SizeWestEast => NSCursor::resizeLeftRightCursor(),
        PetCursor::SizeNorthSouth => NSCursor::resizeUpDownCursor(),
    };
    shape.set();
}

/// What the render thread is told, in order.
enum Command {
    Draw { scene: Box<PetScene>, show: bool },
    Place(LogicalPoint),
    Close,
}

/// A frame's image handed to the main thread (CGImage is immutable and thread-safe).
struct Image(objc2_core_foundation::CFRetained<objc2_core_graphics::CGImage>);
// SAFETY: as above.
unsafe impl Send for Image {}

/// The open panels' window numbers and facts, for the smoke's read-back ([`panel_facts`]).
static LAST_PANEL: Mutex<Option<MainOnly<Retained<NSPanel>>>> = Mutex::new(None);

/// Opens the pet's panel (hidden until its first `show`).
pub async fn open(window: &tauri::Window) -> Result<Box<dyn PetWindow>> {
    let shared = Arc::new(Shared {
        events: Mutex::new(Some({
            let (sender, receiver) = mpsc::unbounded_channel();
            (sender, Some(receiver))
        })),
        closed: AtomicBool::new(false),
        capture: Mutex::default(),
        capture_wanted: AtomicBool::new(false),
        frames: AtomicU64::new(0),
        cursor: Mutex::default(),
        scale: AtomicU64::new(1f64.to_bits()),
        left: AtomicU64::new(0f64.to_bits()),
        top: AtomicU64::new(0f64.to_bits()),
        desktop: Mutex::default(),
        reduce_motion: AtomicBool::new(false),
        window_number: AtomicIsize::new(0),
        hovering: AtomicBool::new(false),
        wheel: Mutex::new(0.0),
        presses: Mutex::new(presses()),
        text: Arc::new(text::Text::on(window.clone())),
    });
    let (made, opened) = tokio::sync::oneshot::channel::<Result<MainOnly<Panel>>>();
    let on_main = shared.clone();
    window
        .run_on_main_thread(move || {
            let made_panel = match MainThreadMarker::new() {
                Some(mtm) => Ok(MainOnly(make_panel(mtm, on_main))),
                None => Err(PlatformError::Refused("the pet's panel: not on the main thread".into())),
            };
            let _ = made.send(made_panel);
        })
        .map_err(|e| PlatformError::Refused(e.to_string()))?;
    let panel = opened.await.map_err(|_| PlatformError::Refused("the pet's panel was not made".into()))??;
    let panel = Arc::new(Mutex::new(Some(panel)));
    let (commands, queue) = std::sync::mpsc::channel::<Command>();
    let (thread_shared, thread_window, thread_panel) = (shared.clone(), window.clone(), panel.clone());
    let thread = std::thread::Builder::new()
        .name("mighty-pet".into())
        .spawn(move || render(thread_shared, thread_window, thread_panel, queue))
        .map_err(PlatformError::Io)?;
    Ok(Box::new(MacPet { shared, window: window.clone(), commands: Mutex::new(commands), thread: Mutex::new(Some(thread)) }))
}

/// The session's presses of any mouse button so far.
fn presses() -> u32 {
    let state = CGEventSourceStateID::CombinedSessionState;
    [CGEventType::LeftMouseDown, CGEventType::RightMouseDown, CGEventType::OtherMouseDown]
        .into_iter()
        .map(|kind| CGEventSource::counter_for_event_type(state, kind))
        .fold(0u32, u32::wrapping_add)
}

fn reduce_motion() -> bool {
    let workspace = NSWorkspace::sharedWorkspace();
    // SAFETY: NSWorkspace's accessibility category answers a BOOL.
    unsafe { msg_send![&*workspace, accessibilityDisplayShouldReduceMotion] }
}

/// The panel and its view (main thread), the desktop read, a display change watched.
fn make_panel(mtm: MainThreadMarker, shared: Arc<Shared>) -> Panel {
    let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(282.0, 330.0));
    let panel = NSPanel::initWithContentRect_styleMask_backing_defer(
        mtm.alloc(),
        frame,
        NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
        NSBackingStoreType::Buffered,
        false,
    );
    panel.setTitle(&objc2_foundation::NSString::from_str("Mighty Code Pet"));
    panel.setOpaque(false);
    panel.setBackgroundColor(Some(&NSColor::clearColor()));
    panel.setHasShadow(false);
    panel.setLevel(NSFloatingWindowLevel);
    panel.setFloatingPanel(true);
    panel.setBecomesKeyOnlyIfNeeded(true);
    panel.setHidesOnDeactivate(false);
    // SAFETY: the panel is kept by its Panel and closed by it; AppKit must not release it on close as well.
    unsafe { panel.setReleasedWhenClosed(false) };
    panel.setMovableByWindowBackground(false);
    panel.setCollectionBehavior(NSWindowCollectionBehavior::CanJoinAllSpaces | NSWindowCollectionBehavior::FullScreenAuxiliary);
    let view = PetView::new(mtm, shared.clone(), frame);
    panel.setContentView(Some(&view));
    *lock(&shared.desktop) = Desktop::read(mtm);
    shared.reduce_motion.store(reduce_motion(), Ordering::SeqCst);
    shared.window_number.store(panel.windowNumber(), Ordering::SeqCst);
    *lock(&LAST_PANEL) = Some(MainOnly(panel.clone()));
    let watched = Arc::downgrade(&shared);
    let block = block2::RcBlock::new(move |_note: std::ptr::NonNull<NSNotification>| {
        guarded((), || {
            let (Some(shared), Some(mtm)) = (watched.upgrade(), MainThreadMarker::new()) else { return };
            let read = Desktop::read(mtm);
            let units = read.units;
            let before = std::mem::replace(&mut *lock(&shared.desktop), read).units;
            shared.reduce_motion.store(reduce_motion(), Ordering::SeqCst);
            if (units - before).abs() > f64::EPSILON {
                shared.emit(PetEvent::DpiChanged { scale: units });
            }
            shared.emit(PetEvent::DisplayChanged);
        });
    });
    // SAFETY: a block that only reads AppKit on the main thread, where AppKit posts the notice (no queue).
    let observer = unsafe {
        NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
            Some(NSApplicationDidChangeScreenParametersNotification),
            None,
            None,
            &block,
        )
    };
    Panel { panel, view, observer: Some(observer), shown: false }
}

impl Panel {
    /// A frame's picture, size and place, together (C#'s `UpdateLayeredWindow`).
    fn show_frame(&mut self, desktop: &Desktop, image: &Image, origin: LogicalPoint, scene: (f64, f64, f64), show: bool) {
        let (width, height, scale) = scene;
        // The picture first, so a redraw the resize asks for shows the new one.
        self.view.show(image.0.clone(), scale);
        self.panel.setFrame_display(desktop.frame(origin, width, height), false);
        if show && !self.shown {
            self.shown = true;
            self.panel.orderFrontRegardless();
        }
    }

    fn close(&mut self) {
        if let Some(observer) = self.observer.take() {
            // SAFETY: the token the default centre returned for the observation.
            unsafe { NSNotificationCenter::defaultCenter().removeObserver(&*(Retained::as_ptr(&observer) as *const AnyObject)) };
        }
        self.panel.orderOut(None);
        self.panel.close();
        let mut last = lock(&LAST_PANEL);
        if last.as_ref().is_some_and(|p| Retained::as_ptr(&p.0) == Retained::as_ptr(&self.panel)) {
            *last = None;
        }
    }
}

/// The render thread: each command in order; a run of draws drawn once (the last), a place or a close after them.
fn render(
    shared: Arc<Shared>,
    window: tauri::Window,
    panel: Arc<Mutex<Option<MainOnly<Panel>>>>,
    queue: std::sync::mpsc::Receiver<Command>,
) {
    let mut renderer = draw::Renderer::new(shared.text.clone());
    while let Ok(first) = queue.recv() {
        let mut commands = vec![first];
        commands.extend(queue.try_iter());
        let mut draw: Option<(Box<PetScene>, bool)> = None;
        for command in commands {
            match command {
                Command::Draw { scene, show } => {
                    let show = show || draw.as_ref().is_some_and(|d| d.1);
                    draw = Some((scene, show));
                }
                Command::Place(at) => {
                    if let Some((scene, show)) = draw.take() {
                        frame(&shared, &window, &panel, renderer.as_mut(), &scene, show);
                    }
                    shared.left.store(at.x.to_bits(), Ordering::SeqCst);
                    shared.top.store(at.y.to_bits(), Ordering::SeqCst);
                    let moved = panel.clone();
                    let desktop = lock(&shared.desktop).clone();
                    let _ = window.run_on_main_thread(move || {
                        if let Some(panel) = lock(&moved).as_mut() {
                            let size = panel.0.panel.frame().size;
                            panel.0.panel.setFrame_display(desktop.frame(at, size.width, size.height), false);
                        }
                    });
                }
                Command::Close => {
                    close_on_main(&window, &panel);
                    return;
                }
            }
        }
        if let Some((scene, show)) = draw {
            frame(&shared, &window, &panel, renderer.as_mut(), &scene, show);
        }
    }
    // The port went without a close: the panel still goes on the main thread.
    close_on_main(&window, &panel);
}

/// The panel closed on the main thread. AppKit's objects never end on this thread: if the main thread cannot take the close (the app is
/// quitting and its loop is gone), the panel is let go of without being released (review item 11).
fn close_on_main(window: &tauri::Window, panel: &Arc<Mutex<Option<MainOnly<Panel>>>>) {
    let closing = panel.clone();
    let posted = window.run_on_main_thread(move || {
        let taken = lock(&closing).take();
        if let Some(mut panel) = taken {
            panel.0.close();
        }
    });
    if posted.is_err() {
        let taken = lock(panel).take();
        if let Some(panel) = taken {
            std::mem::forget(panel);
        }
    }
}

/// One frame: drawn here, numbered and kept for `capture` with its pixels, then shown with its place on the main thread.
fn frame(
    shared: &Arc<Shared>,
    window: &tauri::Window,
    panel: &Arc<Mutex<Option<MainOnly<Panel>>>>,
    renderer: Option<&mut draw::Renderer>,
    scene: &PetScene,
    show: bool,
) {
    let scale = if scene.scale.is_finite() && scene.scale > 0.0 { scene.scale } else { 1.0 };
    shared.scale.store(scale.to_bits(), Ordering::SeqCst);
    shared.left.store(scene.origin.x.to_bits(), Ordering::SeqCst);
    shared.top.store(scene.origin.y.to_bits(), Ordering::SeqCst);
    let wanted = shared.capture_wanted.load(Ordering::SeqCst);
    let Some(drawn) = renderer.and_then(|r| r.render(scene, wanted)) else {
        shared.emit(PetEvent::Failed { message: "CoreGraphics could not draw the pet".into() });
        return;
    };
    {
        // The number is kept with the pixels it numbers (a smoke waiting for its frame never reads the last one's).
        let mut kept = lock(&shared.capture);
        let frames = shared.frames.fetch_add(1, Ordering::SeqCst) + 1;
        if wanted {
            *kept = PetCapture { width: drawn.width, height: drawn.height, bgra: drawn.bgra, frames };
        }
    }
    let image = Image(drawn.image);
    let (origin, size) = (LogicalPoint { x: scene.origin.x, y: scene.origin.y }, (scene.width, scene.height, scale));
    let desktop = lock(&shared.desktop).clone();
    let showing = panel.clone();
    let _ = window.run_on_main_thread(move || {
        if let Some(panel) = lock(&showing).as_mut() {
            panel.0.show_frame(&desktop, &image, origin, size, show);
        }
    });
}

struct MacPet {
    shared: Arc<Shared>,
    window: tauri::Window,
    commands: Mutex<std::sync::mpsc::Sender<Command>>,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl MacPet {
    fn post(&self, command: Command) -> Result<()> {
        self.shared.open()?;
        lock(&self.commands).send(command).map_err(|_| PlatformError::Refused("the pet window's thread ended".into()))
    }

    /// Runs `work` on the main thread and waits for it (at most 2 s); inline when called there.
    fn on_main<T: Send + 'static>(&self, work: impl FnOnce(MainThreadMarker) -> T + Send + 'static) -> Result<T> {
        if let Some(mtm) = MainThreadMarker::new() {
            return Ok(work(mtm));
        }
        let (answer, answered) = std::sync::mpsc::channel();
        self.window
            .run_on_main_thread(move || {
                if let Some(mtm) = MainThreadMarker::new() {
                    let _ = answer.send(work(mtm));
                }
            })
            .map_err(|e| PlatformError::Refused(e.to_string()))?;
        answered.recv_timeout(std::time::Duration::from_secs(2)).map_err(|_| PlatformError::TimedOut("the main thread"))
    }
}

impl PetWindow for MacPet {
    fn show(&self, scene: &PetScene) -> Result<()> {
        self.post(Command::Draw { scene: Box::new(scene.clone()), show: true })
    }
    fn draw(&self, scene: &PetScene) -> Result<()> {
        self.post(Command::Draw { scene: Box::new(scene.clone()), show: false })
    }
    fn place(&self, at: LogicalPoint) -> Result<()> {
        self.post(Command::Place(at))
    }
    fn work_area(&self, near: LogicalPoint) -> Result<LogicalRect> {
        self.shared.open()?;
        Ok(lock(&self.shared.desktop).screen_at(near).visible)
    }
    fn events(&self) -> Result<mpsc::UnboundedReceiver<PetEvent>> {
        self.shared.open()?;
        let mut events = lock(&self.shared.events);
        let (_, receiver) = events.as_mut().ok_or_else(|| PlatformError::Refused("the pet window is closed".into()))?;
        receiver.take().ok_or_else(|| PlatformError::Invalid("the pet window's events were taken already".into()))
    }
    fn close(&self) -> Result<()> {
        self.post(Command::Close)?;
        self.shared.closed.store(true, Ordering::SeqCst);
        *lock(&self.shared.events) = None;
        Ok(())
    }
    fn scale_at(&self, _near: LogicalPoint) -> Result<f64> {
        self.shared.open()?;
        Ok(lock(&self.shared.desktop).units)
    }
    fn animations(&self) -> bool {
        !self.shared.reduce_motion.load(Ordering::SeqCst)
    }
    fn measure(&self) -> Arc<dyn PetMeasure> {
        Arc::new(CoreTextMeasure(self.shared.text.clone()))
    }
    fn poll(&self) -> Result<PetPoll> {
        self.shared.open()?;
        let cursor = lock(&self.shared.desktop).from_appkit(NSEvent::mouseLocation());
        // Down now, or pressed and let go since the last look: a tap between two frames counts too.
        let now = presses();
        let before = std::mem::replace(&mut *lock(&self.shared.presses), now);
        let pressed = NSEvent::pressedMouseButtons() != 0 || now != before;
        let escape = CGEventSource::key_state(CGEventSourceStateID::CombinedSessionState, 53);
        Ok(PetPoll { cursor, pressed, escape })
    }
    fn set_cursor(&self, cursor: PetCursor) -> Result<()> {
        self.shared.open()?;
        let changed = std::mem::replace(&mut *lock(&self.shared.cursor), cursor) != cursor;
        if changed && self.shared.hovering.load(Ordering::SeqCst) {
            let _ = self.window.run_on_main_thread(move || set_cursor(cursor));
        }
        Ok(())
    }
    fn capture(&self) -> Result<PetCapture> {
        self.shared.open()?;
        self.shared.capture_wanted.store(true, Ordering::SeqCst);
        Ok(lock(&self.shared.capture).clone())
    }
    fn focus(&self) -> Result<PetFocus> {
        self.shared.open()?;
        let own_number = self.shared.window_number.load(Ordering::SeqCst);
        self.on_main(move |mtm| {
            let app = NSApplication::sharedApplication(mtm);
            let pid = std::process::id() as i32;
            let front = NSWorkspace::sharedWorkspace().frontmostApplication().map_or(0, |a| a.processIdentifier());
            let key = app.keyWindow();
            let key_number = if front == pid { key.as_ref().map_or(0, |w| w.windowNumber()) } else { 0 };
            let own = app.windowWithWindowNumber(own_number);
            let took = own.as_ref().is_some_and(|w| w.isKeyWindow() || w.isMainWindow());
            let non_activating = own.as_ref().is_some_and(|w| w.styleMask().contains(NSWindowStyleMask::NonactivatingPanel));
            PetFocus {
                non_activating,
                foreground: (u64::from(front as u32) << 32) | u64::from(key_number as u32),
                own: (u64::from(pid as u32) << 32) | u64::from(own_number as u32),
                took_thread: took,
            }
        })
    }
    fn send(&self, input: PetInput) -> Result<()> {
        self.shared.open()?;
        self.shared.input(input);
        Ok(())
    }
}

impl Drop for MacPet {
    fn drop(&mut self) {
        if !self.shared.closed.load(Ordering::SeqCst) {
            let _ = self.close();
        }
        let Some(thread) = lock(&self.thread).take() else { return };
        // The render thread ends at its close; waited for off any async worker, at most 5 s (as Windows' window thread).
        let join = move || {
            let end = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while !thread.is_finished() && std::time::Instant::now() < end {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            if thread.is_finished() {
                let _ = thread.join();
            }
        };
        if tokio::runtime::Handle::try_current().is_ok() {
            let _ = std::thread::Builder::new().name("mighty-pet-join".into()).spawn(join);
        } else {
            join();
        }
    }
}

/// The smoke's read-back of the last pet panel opened (main thread): what Swift's `CompanionPanel` sets — non-activating and
/// borderless, floating level, a floating panel that becomes key only if needed, kept on deactivation, on every Space and over full
/// screen apps, clear and shadowless — each true when it holds, and whether it is on screen and not key.
pub fn panel_facts() -> Option<Vec<(&'static str, bool)>> {
    let mtm = MainThreadMarker::new()?;
    let last = lock(&LAST_PANEL);
    let panel = &last.as_ref()?.0;
    let _ = mtm;
    let style = panel.styleMask();
    let behavior = panel.collectionBehavior();
    Some(vec![
        ("nonActivatingPanel", style.contains(NSWindowStyleMask::NonactivatingPanel)),
        ("borderless", !style.contains(NSWindowStyleMask::Titled)),
        ("floatingLevel", panel.level() == NSFloatingWindowLevel),
        ("floatingPanel", panel.isFloatingPanel()),
        ("becomesKeyOnlyIfNeeded", panel.becomesKeyOnlyIfNeeded()),
        ("keptOnDeactivate", !panel.hidesOnDeactivate()),
        ("allSpaces", behavior.contains(NSWindowCollectionBehavior::CanJoinAllSpaces)),
        ("fullScreenAuxiliary", behavior.contains(NSWindowCollectionBehavior::FullScreenAuxiliary)),
        ("clear", !panel.isOpaque() && !panel.hasShadow()),
        ("notKey", !panel.isKeyWindow()),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The panel's frame in AppKit's space is the scene's size in points at the overlay's place, flipped from the desktop's top-left
    /// units, at scale 1 and 2 (the size itself is the shared layout's: mighty-companion's `the_pet_window_fits_its_bubble`).
    #[test]
    fn the_panel_frame_follows_the_scene() {
        let one = Desktop { screens: Desktop::default().screens, height: 900.0, units: 1.0 };
        let frame = one.frame(LogicalPoint { x: 1140.0, y: 546.0 }, 282.0, 330.0);
        assert_eq!((frame.origin.x, frame.origin.y, frame.size.width, frame.size.height), (1140.0, 24.0, 282.0, 330.0));
        let retina = Desktop { units: 2.0, ..one.clone() };
        let frame = retina.frame(LogicalPoint { x: 2280.0, y: 1092.0 }, 282.0, 330.0);
        assert_eq!((frame.origin.x, frame.origin.y, frame.size.width, frame.size.height), (1140.0, 24.0, 282.0, 330.0));
        assert_eq!(retina.from_appkit(NSPoint::new(10.0, 890.0)), LogicalPoint { x: 20.0, y: 20.0 });
    }

    #[test]
    fn a_point_off_every_display_finds_the_nearest() {
        let left = LogicalRect { x: 0.0, y: 0.0, width: 100.0, height: 100.0 };
        let right = LogicalRect { x: 100.0, y: 0.0, width: 100.0, height: 100.0 };
        let desktop = Desktop {
            screens: vec![Screen { frame: left, visible: left }, Screen { frame: right, visible: right }],
            height: 100.0,
            units: 1.0,
        };
        assert_eq!(desktop.screen_at(LogicalPoint { x: 150.0, y: 50.0 }).frame, right);
        assert_eq!(desktop.screen_at(LogicalPoint { x: 500.0, y: -40.0 }).frame, right);
        assert_eq!(desktop.screen_at(LogicalPoint { x: -5.0, y: 10.0 }).frame, left);
    }

    /// Input sent to the window becomes the events Windows' window procedure makes of it, at the window's place and scale.
    #[test]
    fn input_becomes_the_window_events() {
        let shared = Shared {
            events: Mutex::new(Some({
                let (sender, receiver) = mpsc::unbounded_channel();
                (sender, Some(receiver))
            })),
            closed: AtomicBool::new(false),
            capture: Mutex::default(),
            capture_wanted: AtomicBool::new(false),
            frames: AtomicU64::new(0),
            cursor: Mutex::default(),
            scale: AtomicU64::new(1.5f64.to_bits()),
            left: AtomicU64::new(100f64.to_bits()),
            top: AtomicU64::new(200f64.to_bits()),
            desktop: Mutex::default(),
            reduce_motion: AtomicBool::new(false),
            window_number: AtomicIsize::new(0),
            hovering: AtomicBool::new(false),
            wheel: Mutex::new(0.0),
            presses: Mutex::new(0),
            text: Arc::default(),
        };
        let mut events = lock(&shared.events).as_mut().unwrap().1.take().unwrap();
        let at = LogicalPoint { x: 10.0, y: 20.0 };
        shared.input(PetInput::Click(at));
        shared.input(PetInput::Dpi(144));
        let pointer = PetPointer { window: at, screen: LogicalPoint { x: 115.0, y: 230.0 } };
        assert_eq!(events.try_recv().unwrap(), PetEvent::Pressed(pointer));
        assert_eq!(events.try_recv().unwrap(), PetEvent::Released(pointer));
        assert_eq!(events.try_recv().unwrap(), PetEvent::DpiChanged { scale: 1.5 });
        assert!(events.try_recv().is_err());
    }
}
