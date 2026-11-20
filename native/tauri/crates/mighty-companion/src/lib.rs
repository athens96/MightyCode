//! The companion pet (W8-E, `.omc/research/tauri/w8-plan.md` §1.7, D8-D10; C# Core\Companion.cs, CompanionBubble.cs,
//! CompanionBubbleLayout.cs and the pure half of WinUI\CompanionOverlay.cs; docs/windows-companion.md). No Tauri and no OS code: it
//! lays out a [`scene::PetScene`] each frame and the platform's `PetWindow` draws it (Windows: Direct2D and DirectWrite on a layered
//! window, mighty-platform-windows `pet.rs`). The app's half is src-tauri `companion.rs` (the 1/12 s timer, the followed panes, the
//! card's actions, the popover, the status bar's controls) and `settings/companion.rs`.
//!
//! | Module | C# | Owner |
//! |---|---|---|
//! | [`prefs`] | `CompanionPreferences` (`companion-settings.json`) | W8-E |
//! | [`pet`] | `CompanionPet` (the catalog, the atlas checks, the decode) | W8-E |
//! | [`animation`] | `CompanionAnimation` (rows, `Priority`) | W8-E |
//! | [`carousel`] | `CompanionCarousel` | W8-E |
//! | [`bubble`] | `CompanionBubble` (buttons, questionnaire stepping) | W8-E |
//! | [`plan`] | `CompanionBubble.PlanBlocks` (the plan drawn as blocks) | W8-E |
//! | [`layout`] | `CompanionBubbleLayout`, `CompanionResizeEdges` | W8-E |
//! | [`card`] | `CompanionOverlayCard`, `CompanionOverlayRequest` | W8-E |
//! | [`follow`] | `RefreshCompanion`'s choice of agent, its bubble and its row (MainWindow.Companion.cs) | W8-E |
//! | [`progress`] | the Mac's `CompanionProgress` (the progress card over the bubble) | W8-E |
//! | [`scene`] | `CompanionOverlay`'s drawing as a display list | W8-E |
//! | [`Companion`] | `CompanionOverlay`'s state: its size and place, the card, the press, drag and resize, the menu, the plan's scroll | W8-E |
// A guard kept through a match or if-let body has called out while held: bind first.
#![deny(clippy::significant_drop_in_scrutinee)]

pub mod animation;
pub mod bubble;
pub mod card;
pub mod carousel;
pub mod follow;
pub mod layout;
pub mod pet;
pub mod plan;
pub mod prefs;
pub mod progress;
pub mod scene;

use std::sync::Arc;

use mighty_core::design_tokens::Token;
use mighty_platform::pet::{ApproximateMeasure, PetBitmap, PetItem, PetMeasure, PetPoint, PetRect};
use mighty_platform::surface::PetCursor;
use mighty_platform::{LogicalPoint, LogicalRect, PetEvent, PetScene, PetWindow};

use crate::card::CompanionCard;
use crate::layout::Edges;
use crate::progress::CompanionProgress;
use crate::scene::{Painter, PlanBox, PlanMeasure};

/// What the overlay asks of the app (C# `CompanionOverlay`'s events): `Act` is its `Action(key, action)` — a bubble button, a choice,
/// the pager, `toggle` (the pet), `open` (the bubble or the menu), `hide` — for the card with that key; `Moved` and `Resized` are what to
/// save (`left`, `top` in the desktop's pixels; `bubbleWidth`, `bubbleHeight`, None after a reset); `Failed` ends the pet.
#[derive(Debug, Clone, PartialEq)]
pub enum CompanionAction {
    Act { card: String, action: String },
    Moved { left: i32, top: i32 },
    Resized { width: Option<f64>, height: Option<f64>, left: i32, top: i32 },
    Failed { message: String },
}

/// The window, the bubble and the pet's frame in DIPs, and the drawing's scale (C# `SmokeGeometry`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Geometry {
    pub width: f64,
    pub height: f64,
    pub bubble: PetRect,
    pub pet: PetRect,
    pub scale: f64,
}

/// The bubble's size and where the window's bottom right stands (C# `SmokeLayout`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    pub width: i32,
    pub height: i32,
    pub right: i32,
    pub bottom: i32,
    pub automatic_height: bool,
}

#[derive(Debug, Clone, Default)]
struct Press {
    down: bool,
    dragged: bool,
    swipe: i32,
    wheel: i32,
    walking_row: i32,
    walked_at: u64,
    start: LogicalPoint,
    last: LogicalPoint,
    start_origin: (i32, i32),
    start_frame: (i32, i32, i32, i32),
    action: Option<String>,
    key: Option<String>,
    start_width: i32,
    start_drawn_width: i32,
    start_height: i32,
    start_fixed: Option<f64>,
    edges: Edges,
}

/// The pet's window as C#'s `CompanionOverlay` keeps it between frames: its size and place, what it shows, a press in progress (a carry,
/// a resize, a swipe), the menu and the plan's scroll. `Default` has no window: it draws nothing and answers nothing.
pub struct Companion {
    window: Option<Arc<dyn PetWindow>>,
    measure: Arc<dyn PetMeasure>,
    width: i32,
    fixed_height: Option<f64>,
    panel_width: f64,
    height: f64,
    bubble_height: i32,
    bubble_cut: i32,
    scale: f64,
    left: i32,
    top: i32,
    pixel_width: i32,
    pixel_height: i32,
    card: Option<CompanionCard>,
    show_bubble: bool,
    atlas: Option<PetBitmap>,
    row: i32,
    column: u32,
    still: bool,
    ticks: u64,
    card_dirty: bool,
    bubble_items: Vec<PetItem>,
    hits: Vec<(PetRect, String)>,
    spinner: Option<PetPoint>,
    /// Where the progress card's spinner turns, while its request runs.
    progress_spinner: Option<PetPoint>,
    plan_measure: Option<PlanMeasure>,
    plan: PlanBox,
    /// How far the progress card's list was scrolled by hand (None: it stands where it opens, on the step in progress), and its box
    /// as last drawn.
    progress_scroll: Option<f64>,
    progress_box: PlanBox,
    menu_items: Vec<(PetRect, String, String)>,
    menu_area: PetRect,
    menu_open: bool,
    menu_hover: Option<usize>,
    keep_menu: bool,
    press: Press,
    presented: bool,
    fit_pending: bool,
}

impl Default for Companion {
    fn default() -> Self {
        Companion {
            window: None,
            measure: Arc::new(ApproximateMeasure),
            width: layout::DEFAULT_WIDTH as i32,
            fixed_height: None,
            panel_width: layout::DEFAULT_WIDTH + 2.0 * layout::SIDE_PADDING,
            height: layout::BASE_HEIGHT,
            bubble_height: 0,
            bubble_cut: 0,
            scale: 1.0,
            left: 0,
            top: 0,
            pixel_width: 0,
            pixel_height: 0,
            card: None,
            show_bubble: false,
            atlas: None,
            row: 0,
            column: 0,
            still: false,
            ticks: 0,
            card_dirty: true,
            bubble_items: vec![],
            hits: vec![],
            spinner: None,
            progress_spinner: None,
            plan_measure: None,
            plan: PlanBox::default(),
            progress_scroll: None,
            progress_box: PlanBox::default(),
            menu_items: vec![],
            menu_area: PetRect::default(),
            menu_open: false,
            menu_hover: None,
            keep_menu: false,
            press: Press { walking_row: 1, ..Press::default() },
            presented: false,
            fit_pending: false,
        }
    }
}

/// The card without its clocks (the bubble's and the progress card's lines'): a clock that ticks redraws, it does not lay out again.
fn without_clock(card: &CompanionCard) -> CompanionCard {
    CompanionCard { elapsed: None, progress: card.progress.as_ref().map(CompanionProgress::without_clocks), ..card.clone() }
}

fn same_without_clock(a: Option<&CompanionCard>, b: Option<&CompanionCard>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => without_clock(a) == without_clock(b) && a.elapsed.is_none() == b.elapsed.is_none(),
        _ => false,
    }
}

impl Companion {
    /// C# `new CompanionOverlay(preferences)`: the bubble's saved size; the window where it was left, at that display's scale, or first
    /// 300 in from the work area's right edge and 24 above its bottom.
    pub fn new(window: Arc<dyn PetWindow>, preferences: &prefs::CompanionPreferences) -> Companion {
        let width = layout::width(preferences.bubble_width) as i32;
        let fixed_height = layout::height(preferences.bubble_height);
        let (panel_width, height) = layout::panel_size(Some(f64::from(width)), fixed_height, false);
        let at = LogicalPoint { x: f64::from(preferences.left.unwrap_or(0)), y: f64::from(preferences.top.unwrap_or(0)) };
        let scale = window.scale_at(at).unwrap_or(1.0).clamp(1.0, 4.0);
        let mut companion = Companion {
            measure: window.measure(),
            window: Some(window),
            width,
            fixed_height,
            panel_width,
            height,
            scale,
            ..Companion::default()
        };
        companion.pixel_width = (panel_width * scale).ceil() as i32;
        companion.pixel_height = (height * scale).ceil() as i32;
        let work = companion.work_area(at);
        companion.left = preferences.left.unwrap_or(work.right - (300.0 * scale).round() as i32);
        companion.top = preferences.top.unwrap_or(work.bottom - companion.pixel_height - (24.0 * scale).round() as i32);
        companion.clamp_position();
        companion
    }

    /// The window this companion draws in.
    pub fn window(&self) -> Option<&Arc<dyn PetWindow>> {
        self.window.as_ref()
    }

    /// C# `SetAtlas`: the pet's sprite sheet (premultiplied BGRA, 1536 wide), or none, which draws the paw.
    pub fn set_atlas(&mut self, atlas: Option<PetBitmap>) {
        self.atlas = atlas.filter(|a| {
            a.width == pet::ATLAS_WIDTH && a.height >= 9 * pet::CELL_HEIGHT && a.bgra.len() == (a.width * a.height * 4) as usize
        });
    }

    /// C# `SetCard(value, visible)`: what the bubble shows, and whether it shows.
    pub fn set_card(&mut self, value: Option<CompanionCard>, visible: bool) {
        let shown = visible && value.is_some();
        let key = |card: Option<&CompanionCard>| card.map(|c| c.key.clone());
        if self.press.down && !self.press.edges.is_none() && (!shown || key(self.card.as_ref()) != key(value.as_ref())) {
            self.cancel_resize();
        }
        let changed = self.show_bubble != shown || !same_without_clock(self.card.as_ref(), value.as_ref());
        let ticked = self.card != value;
        if key(self.card.as_ref()) != key(value.as_ref()) {
            self.plan.scroll = 0.0;
        }
        // Another request (or another pane's) opens its list on its own step in progress again.
        let request = |card: Option<&CompanionCard>| card.and_then(|c| c.progress.as_ref()).map(|p| p.heading.clone());
        if key(self.card.as_ref()) != key(value.as_ref()) || request(self.card.as_ref()) != request(value.as_ref()) {
            self.progress_scroll = None;
        }
        self.card = value;
        self.show_bubble = shown;
        if ticked && shown {
            // The clock ticks by itself; a request draws none.
            self.card_dirty = true;
        }
        if !changed {
            return;
        }
        self.card_dirty = true;
        if self.press.down {
            self.fit_pending = true;
        } else {
            self.fit(-1, None);
        }
    }

    /// The frame to draw: the row, its column, and whether the pet stands still (reduced motion).
    pub fn set_frame(&mut self, row: i32, column: u32, still: bool) {
        self.row = row.clamp(0, 8);
        self.column = column;
        self.still = still;
    }

    /// Set by the smoke: the desktop it runs on is not its own, and a click in another window must not put the menu away mid-check.
    pub fn set_keep_menu(&mut self, keep: bool) {
        self.keep_menu = keep;
    }

    /// One 1/12 s frame: the spinner and a carried pet's walk move on.
    pub fn tick(&mut self) {
        self.ticks += 1;
    }

    fn tall(&self) -> bool {
        self.show_bubble && self.card.as_ref().is_some_and(|c| c.approval.is_some())
    }

    /// The progress card over the task bubble and whether it is folded: none over a request or with the bubble away.
    fn progress(&self) -> Option<(&CompanionProgress, bool)> {
        let card = self.card.as_ref().filter(|c| self.show_bubble && c.approval.is_none())?;
        card.progress.as_ref().map(|p| (p, card.progress_collapsed))
    }

    /// How much taller the window stands for the progress card (the gap to the bubble included).
    fn above(&self) -> f64 {
        self.progress().map_or(0.0, |(progress, collapsed)| progress.height(collapsed))
    }

    /// The progress card's place: as wide as the bubble, over it by its gap.
    fn progress_area(&self) -> Option<PetRect> {
        let above = self.above();
        (above > 0.0).then(|| {
            let bubble = self.bubble_area();
            PetRect::new(bubble.x, bubble.y - above, bubble.width, above - progress::GAP)
        })
    }

    /// The approval is never narrower than the default.
    fn drawn_width(&self) -> f64 {
        if self.tall() { layout::approval_width(Some(f64::from(self.width))) } else { f64::from(self.width) }
    }

    /// The bubble stands on the pet: 2 above its frame, 12 in from each side of the window.
    fn bubble_area(&self) -> PetRect {
        PetRect::new(
            layout::SIDE_PADDING,
            self.height - layout::CHROME + layout::PADDING - f64::from(self.bubble_height),
            self.drawn_width(),
            f64::from(self.bubble_height),
        )
    }

    /// The pet's 125 × 135 frame, centred at the bottom above the 8 of padding.
    fn pet_area(&self) -> PetRect {
        PetRect::new(
            (self.panel_width - layout::PET_WIDTH) / 2.0,
            self.height - layout::PADDING - layout::PET_HEIGHT,
            layout::PET_WIDTH,
            layout::PET_HEIGHT,
        )
    }

    /// The saved top is the one of the window around a bubble that follows its content, so the pet returns where it stood.
    fn saved_top(&self) -> i32 {
        self.top + self.pixel_height
            - (layout::panel_size(Some(f64::from(self.width)), self.fixed_height, false).1 * self.scale).ceil() as i32
    }

    fn work_area(&self, near: LogicalPoint) -> Area {
        let fallback = LogicalRect { x: 0.0, y: 0.0, width: 1920.0, height: 1080.0 };
        let rect = self.window.as_ref().and_then(|w| w.work_area(near).ok()).unwrap_or(fallback);
        Area { left: rect.x as i32, top: rect.y as i32, right: (rect.x + rect.width) as i32, bottom: (rect.y + rect.height) as i32 }
    }

    /// C# `Fit(keep, from)`: the window sized around the bubble, growing upward from its bottom edge; sideways it keeps its left edge
    /// (`keep` -1), its right one (1) or its middle (0), measured from `from` while an edge is dragged. The bubble is never taller than
    /// the screen it stands on: what does not fit is cut at its top, so its buttons are still there to press.
    fn fit(&mut self, keep: i32, from: Option<(i32, i32, i32, i32)>) {
        if self.window.is_none() {
            return;
        }
        self.fit_pending = false;
        let origin = from.unwrap_or((self.left, self.top, self.pixel_width, self.pixel_height));
        let screen = self.work_area(LogicalPoint { x: f64::from(origin.0 + origin.2 / 2), y: f64::from(origin.1 + origin.3 - 64) });
        let needed = self.measure_bubble();
        // The progress card stands over the bubble, which leaves it its room on the screen.
        let above = self.above();
        self.bubble_height = needed.min((((f64::from(screen.bottom - screen.top)) / self.scale - layout::CHROME - above) as i32).max(1));
        self.bubble_cut = needed - self.bubble_height;
        let size = layout::panel_size(Some(f64::from(self.width)), self.fixed_height, self.tall());
        self.panel_width = size.0;
        self.height = size.1.max(f64::from(self.bubble_height) + layout::CHROME + above);
        let (w, h) = ((self.panel_width * self.scale).ceil() as i32, (self.height * self.scale).ceil() as i32);
        self.left = origin.0
            + match keep.cmp(&0) {
                std::cmp::Ordering::Less => 0,
                std::cmp::Ordering::Greater => origin.2 - w,
                std::cmp::Ordering::Equal => (origin.2 - w) / 2,
            };
        self.top = origin.1 + origin.3 - h;
        // A menu belongs to the place it opened at; a window of another size puts it away.
        if w != self.pixel_width || h != self.pixel_height {
            self.pixel_width = w;
            self.pixel_height = h;
            self.menu_open = false;
        }
        self.card_dirty = true;
        self.clamp_position();
    }

    fn measure_bubble(&mut self) -> i32 {
        let Some(current) = self.card.clone() else { return self.bubble_height };
        let palette = scene::palette(current.light);
        let inner = self.drawn_width() - 2.0 * scene::INSET;
        let measure = self.measure.clone();
        let mut painter = Painter::new(measure.as_ref(), palette, false, &mut self.plan_measure);
        if let Some(request) = &current.approval {
            return (painter.request_content(&current, request, 0.0, 0.0, inner) + 2.0 * scene::INSET).ceil() as i32;
        }
        let natural =
            painter.task_content(&current, 0.0, 0.0, inner, self.fixed_height.map(|h| h - 2.0 * scene::INSET)) + 2.0 * scene::INSET;
        natural.max(self.fixed_height.unwrap_or(0.0)).ceil() as i32
    }

    /// C# `RenderCard`: the bubble, laid out once per change: a card of the app's theme at radius 18 with a hairline of the primary ink
    /// at 10%, or of the amber at 45% around a request. The Mac's material blurs what is behind it; a layered window cannot, so the card
    /// is the opaque `card` token.
    fn render_card(&mut self) {
        self.card_dirty = false;
        self.hits.clear();
        self.spinner = None;
        self.progress_spinner = None;
        self.progress_box = PlanBox::default();
        self.bubble_items.clear();
        let Some(current) = self.card.clone().filter(|_| self.show_bubble) else { return };
        let palette = scene::palette(current.light);
        let bubble = self.bubble_area();
        let measure = self.measure.clone();
        let mut painter = Painter::new(measure.as_ref(), palette, true, &mut self.plan_measure);
        painter.plan.scroll = self.plan.scroll;
        let (x, y, w) = (bubble.x + scene::INSET, bubble.y + scene::INSET - f64::from(self.bubble_cut), bubble.width - 2.0 * scene::INSET);
        if let Some(request) = &current.approval {
            painter.request_content(&current, request, x, y, w);
        } else {
            painter.task_content(&current, x, y, w, self.fixed_height.map(|h| h - 2.0 * scene::INSET));
        }
        let content = std::mem::take(&mut painter.items);
        let edge =
            if current.approval.is_none() { scene::paint(palette[Token::Ink], 0.10) } else { scene::paint(palette[Token::Wait], 0.45) };
        let mut items = vec![
            PetItem::Fill { rect: bubble, radius: scene::CORNER, color: scene::paint(palette[Token::Card], 1.0) },
            PetItem::Stroke {
                rect: PetRect::new(bubble.x + 0.5, bubble.y + 0.5, bubble.width - 1.0, bubble.height - 1.0),
                radius: scene::CORNER - 0.5,
                color: edge,
                width: mighty_core::design_tokens::metrics::STROKE_LINE,
            },
        ];
        // A bubble held to the screen's height shows the end of what it holds: the content starts above the bubble by what was cut and
        // is clipped to it.
        if self.bubble_cut > 0 {
            items.push(PetItem::Clip { rect: bubble, items: content });
        } else {
            items.extend(content);
        }
        self.hits = std::mem::take(&mut painter.hits);
        self.spinner = painter.spinner;
        self.plan = painter.plan;
        self.bubble_items = vec![PetItem::Group { role: "bubble".into(), rect: bubble, opacity: 1.0, items }];
        if let (Some((progress, collapsed)), Some(area)) = (self.progress().map(|(p, c)| (p.clone(), c)), self.progress_area()) {
            let mut plan = None;
            let mut painter = Painter::new(measure.as_ref(), palette, true, &mut plan);
            self.progress_box = painter.progress_card(&progress, collapsed, area, self.progress_scroll);
            self.progress_spinner = painter.spinner;
            self.hits.append(&mut painter.hits);
            self.bubble_items.insert(0, painter.items.remove(0));
        }
    }

    /// C# `Draw(row, column, reducedMotion)`'s picture: the bubble, the turning spinner, the pet (walking while carried) and the menu, at
    /// the window's place, size and scale. A press elsewhere or Esc puts the menu away first; the row under the pointer takes its wash.
    pub fn scene(&mut self) -> PetScene {
        let Some(window) = self.window.clone() else { return PetScene::default() };
        if self.menu_open && !self.keep_menu {
            let poll = window.poll().unwrap_or_default();
            let (x, y) = ((poll.cursor.x - f64::from(self.left)) / self.scale, (poll.cursor.y - f64::from(self.top)) / self.scale);
            let around =
                PetRect::new(self.menu_area.x - 3.0, self.menu_area.y - 3.0, self.menu_area.width + 6.0, self.menu_area.height + 6.0);
            if poll.escape || poll.pressed && !around.contains(x, y) {
                self.menu_open = false;
            } else {
                self.menu_hover = self.menu_items.iter().position(|item| item.0.contains(x, y));
            }
        }
        let (mut row, mut column) = (self.row, self.column);
        // Carried, the pet walks the way it goes and stands again once the pointer rests.
        if self.press.down
            && self.press.dragged
            && self.press.action.as_deref() == Some("toggle")
            && (self.ticks - self.press.walked_at) * 1000 / 12 < 350
        {
            row = self.press.walking_row;
            column = pet::frame(row, self.ticks as f64 / 12.0, self.still);
        }
        if self.card_dirty {
            self.render_card();
        }
        self.presented = true;
        let palette = scene::palette(self.card.as_ref().is_some_and(|c| c.light));
        let accent = scene::paint(palette[Token::Accent], 1.0);
        let mut items = self.bubble_items.clone();
        let step = if self.still { 0 } else { (self.ticks % 12) as u32 };
        for center in [self.progress_spinner, self.spinner].into_iter().flatten() {
            items.push(scene::spinner(center, step, accent));
        }
        let frame = self.pet_area();
        items.push(PetItem::Group {
            role: "pet".into(),
            rect: frame,
            opacity: 1.0,
            items: scene::pet(frame, self.atlas.is_some(), row, column, accent),
        });
        if self.menu_open {
            let measure = self.measure.clone();
            let mut plan = None;
            let mut painter = Painter::new(measure.as_ref(), palette, true, &mut plan);
            scene::menu(&mut painter, self.menu_area, &self.menu_items, self.menu_hover);
            items.append(&mut painter.items);
        }
        PetScene {
            width: self.panel_width,
            height: self.height,
            scale: self.scale,
            origin: PetPoint { x: f64::from(self.left), y: f64::from(self.top) },
            items,
            atlas: self.atlas.clone(),
        }
    }

    /// C# `LayoutMenu(at)`: the two entries in a card that opens where the pointer is, kept inside the window.
    fn layout_menu(&mut self, at: LogicalPoint) {
        self.menu_items.clear();
        let entries = scene::menu_entries();
        let measure = self.measure.clone();
        let mut plan = None;
        let painter = Painter::new(measure.as_ref(), scene::palette(false), false, &mut plan);
        let widest = entries
            .iter()
            .map(|(_, label)| painter.text(label, scene::MENU_TYPE, 400, scene::WIDE, 1, None, false).width)
            .fold(0.0, f64::max);
        let w = 120f64.max(widest + 2.0 * (scene::MENU_INSET + 10.0)).ceil();
        let h = 2.0 * scene::MENU_INSET + entries.len() as f64 * scene::MENU_ROW;
        let x = (if at.x + w > self.panel_width - 2.0 { at.x - w } else { at.x }).clamp(2.0, 2f64.max(self.panel_width - w - 2.0));
        let y = (if at.y + h > self.height - 2.0 { at.y - h } else { at.y }).clamp(2.0, 2f64.max(self.height - h - 2.0));
        self.menu_area = PetRect::new(x.round(), y.round(), w, h);
        self.menu_hover = None;
        for (index, (id, label)) in entries.into_iter().enumerate() {
            let bounds = PetRect::new(
                self.menu_area.x + scene::MENU_INSET,
                self.menu_area.y + scene::MENU_INSET + index as f64 * scene::MENU_ROW,
                w - 2.0 * scene::MENU_INSET,
                scene::MENU_ROW,
            );
            self.menu_items.push((bounds, id.to_string(), label));
        }
    }

    fn resize_at(&self, at: LogicalPoint) -> Edges {
        let Some(card) = self.card.as_ref().filter(|_| self.show_bubble && !self.menu_open) else { return Edges::NONE };
        let bubble = self.bubble_area();
        layout::hit(at.x - bubble.x, at.y - bubble.y, bubble.width, bubble.height, card.approval.is_none())
    }

    /// C# `Hit(p)`: what a press at a point of the window means. The targets are those of the frame about to be shown.
    fn hit(&mut self, at: LogicalPoint) -> String {
        if self.card_dirty {
            self.render_card();
        }
        if self.menu_open {
            return self
                .menu_items
                .iter()
                .find(|item| item.0.contains(at.x, at.y))
                .map_or_else(|| "menu-close".into(), |item| item.1.clone());
        }
        // The progress card: its heading folds it, the rest opens the pane.
        if let Some(area) = self.progress_area().filter(|area| area.contains(at.x, at.y)) {
            return self
                .hits
                .iter()
                .find(|(bounds, _)| area.contains(bounds.x, bounds.y) && bounds.contains(at.x, at.y))
                .map_or_else(|| "open".into(), |(_, id)| id.clone());
        }
        if self.show_bubble && self.card.is_some() && self.bubble_area().contains(at.x, at.y) {
            if !self.resize_at(at).is_none() {
                return "resize".into();
            }
            return self.hits.iter().find(|(bounds, _)| bounds.contains(at.x, at.y)).map_or_else(|| "open".into(), |(_, id)| id.clone());
        }
        if self.pet_area().contains(at.x, at.y) { "toggle".into() } else { String::new() }
    }

    fn resize_from_pointer(&mut self, dx: f64, dy: f64) {
        // An approval is drawn no narrower than the default: its edge is dragged from where it is drawn, and left where it was it keeps
        // the narrower width that was saved.
        let next = layout::resize(
            f64::from(self.press.start_drawn_width),
            f64::from(self.press.start_height),
            dx,
            dy,
            self.scale,
            self.press.edges,
        );
        let mut next_width = next.width as i32;
        let vertical = layout::vertical(self.press.edges);
        if self.card.as_ref().is_some_and(|c| c.approval.is_some()) {
            next_width = next_width.max(layout::DEFAULT_WIDTH as i32);
            if next_width == self.press.start_drawn_width {
                next_width = self.press.start_width;
            }
        }
        // A sideways drag leaves a content-following height as it was.
        if next_width == self.width && (!vertical || next.height as i32 == self.bubble_height) {
            return;
        }
        self.width = next_width;
        if vertical {
            self.fixed_height = Some(f64::from(next.height as i32));
        }
        self.press.dragged = true;
        let keep = if self.press.edges.has(Edges::LEFT) { 1 } else { -1 };
        self.fit(keep, Some(self.press.start_frame));
    }

    /// C# `ResetSize` (a double click on a resize strip): the default width and a height that follows the content again.
    pub fn reset_size(&mut self) -> CompanionAction {
        self.width = layout::DEFAULT_WIDTH as i32;
        self.fixed_height = None;
        self.fit(0, None);
        CompanionAction::Resized { width: None, height: None, left: self.left, top: self.saved_top() }
    }

    fn cancel_resize(&mut self) {
        if self.press.edges.is_none() {
            return;
        }
        self.width = self.press.start_width;
        self.fixed_height = self.press.start_fixed;
        self.press.down = false;
        self.press.dragged = false;
        self.press.edges = Edges::NONE;
        self.press.action = None;
        self.fit(-1, Some(self.press.start_frame));
    }

    /// C# `ClampPosition`: the pet and a bubble that follows its content stay on the screen; the window's own clear padding (12 at the
    /// sides, 8 below) may hang over its edge. The pet's feet are what is held, never the window's top.
    fn clamp_position(&mut self) {
        let work = self
            .work_area(LogicalPoint { x: f64::from(self.left + self.pixel_width / 2), y: f64::from(self.top + self.pixel_height - 64) });
        let side = (layout::SIDE_PADDING * self.scale) as i32;
        let below = (layout::PADDING * self.scale) as i32;
        let room = (layout::panel_size(Some(f64::from(self.width)), self.fixed_height, false).1 * self.scale).ceil() as i32;
        self.left = self.left.clamp(work.left - side, (work.left - side).max(work.right - self.pixel_width + side));
        self.top =
            (self.top + self.pixel_height).clamp((work.top + room).min(work.bottom + below), work.bottom + below) - self.pixel_height;
    }

    fn begin_press(&mut self, pointer: LogicalPoint, screen: LogicalPoint) {
        self.press.start = screen;
        self.press.last = screen;
        self.press.start_origin = (self.left, self.top);
        self.press.start_frame = (self.left, self.top, self.pixel_width, self.pixel_height);
        self.press.down = true;
        self.press.dragged = false;
        self.press.swipe = 0;
        self.press.start_width = self.width;
        self.press.start_drawn_width = self.drawn_width() as i32;
        self.press.start_height = self.bubble_height;
        self.press.start_fixed = self.fixed_height;
        self.press.edges = self.resize_at(pointer);
        self.press.action = Some(self.hit(pointer));
        self.press.key = Some(self.card.as_ref().map(|c| c.key.clone()).unwrap_or_default());
    }

    fn card_key(&self) -> String {
        self.card.as_ref().map(|c| c.key.clone()).unwrap_or_default()
    }

    /// One event of the pet's window (C# `WndProc`); what the app must do about it, if anything.
    pub fn act(&mut self, event: &PetEvent) -> Option<CompanionAction> {
        let window = self.window.clone()?;
        match event {
            PetEvent::Moved(pointer) if !self.press.down => {
                // C#'s WM_SETCURSOR: a resize strip's shape, else the arrow.
                let edges = self.resize_at(pointer.window);
                let cursor = match edges {
                    e if e == Edges::LEFT || e == Edges::RIGHT => PetCursor::SizeWestEast,
                    e if e == Edges::TOP => PetCursor::SizeNorthSouth,
                    e if e == Edges::LEFT | Edges::TOP => PetCursor::SizeNorthWestSouthEast,
                    e if e == Edges::RIGHT | Edges::TOP => PetCursor::SizeNorthEastSouthWest,
                    _ => PetCursor::Arrow,
                };
                let _ = window.set_cursor(cursor);
                None
            }
            PetEvent::Pressed(pointer) => {
                self.begin_press(pointer.window, pointer.screen);
                None
            }
            PetEvent::Moved(pointer) => {
                let (dx, dy) = (pointer.screen.x - self.press.start.x, pointer.screen.y - self.press.start.y);
                let action = self.press.action.clone().unwrap_or_default();
                // Four points of travel make a press a carry, not a click.
                if action == "toggle" && (self.press.dragged || (dx * dx + dy * dy).sqrt() >= 4.0 * self.scale) {
                    self.press.dragged = true;
                    self.left = self.press.start_origin.0 + dx as i32;
                    self.top = self.press.start_origin.1 + dy as i32;
                    let _ = window.place(LogicalPoint { x: f64::from(self.left), y: f64::from(self.top) });
                    // Row 1 walks to the right, row 2 to the left.
                    if pointer.screen.x != self.press.last.x {
                        self.press.walking_row = if pointer.screen.x > self.press.last.x { 1 } else { 2 };
                        self.press.walked_at = self.ticks;
                    }
                } else if action == "open"
                    && !self.menu_open
                    && self.card.as_ref().is_some_and(|c| c.count > 1)
                    && dx.abs() / self.scale > 36.0
                    && dx.abs() > dy.abs()
                {
                    self.press.dragged = true;
                    self.press.swipe = if dx < 0.0 { 1 } else { -1 };
                } else if action == "resize" {
                    self.resize_from_pointer(dx, dy);
                }
                self.press.last = pointer.screen;
                None
            }
            PetEvent::Released(pointer) if self.press.down => {
                let completed = self.press.action.take();
                let key = self.press.key.clone();
                self.press.down = false;
                self.press.edges = Edges::NONE;
                let mut result = None;
                if self.press.dragged && self.press.swipe != 0 {
                    if key.as_deref() == self.card.as_ref().map(|c| c.key.as_str()) {
                        result = Some(CompanionAction::Act {
                            card: key.unwrap_or_default(),
                            action: if self.press.swipe > 0 { "next" } else { "previous" }.into(),
                        });
                    }
                } else if self.press.dragged {
                    if completed.as_deref() == Some("resize")
                        && self.width == self.press.start_width
                        && self.bubble_height == self.press.start_height
                    {
                        self.fixed_height = self.press.start_fixed;
                        self.fit(-1, None);
                    } else {
                        self.clamp_position();
                        result = Some(if completed.as_deref() == Some("resize") {
                            CompanionAction::Resized {
                                width: Some(f64::from(self.width)),
                                height: self.fixed_height,
                                left: self.left,
                                top: self.saved_top(),
                            }
                        } else {
                            CompanionAction::Moved { left: self.left, top: self.saved_top() }
                        });
                    }
                } else if let Some(action) = completed.filter(|a| !a.is_empty())
                    && action == self.hit(pointer.window)
                    && key.as_deref() == Some(self.card_key().as_str())
                {
                    if self.menu_open {
                        self.menu_open = false;
                    }
                    if action != "menu-close" && action != "resize" {
                        result = Some(CompanionAction::Act { card: key.unwrap_or_default(), action });
                    }
                }
                if self.fit_pending {
                    self.fit(-1, None);
                }
                result
            }
            PetEvent::DoubleClicked(pointer) => {
                // A double click on a resize strip restores automatic sizing. On the pager the second of two quick presses is a press
                // like the first; anywhere else it is let go.
                let twice = self.hit(pointer.window);
                if twice == "resize" {
                    return Some(self.reset_size());
                }
                if twice == "previous" || twice == "next" {
                    self.begin_press(pointer.window, pointer.screen);
                }
                None
            }
            PetEvent::RightClicked(pointer) => {
                // The pet's menu opens where the pointer is; a right click elsewhere on the pet moves it there.
                let at = pointer.window;
                if self.menu_open && self.menu_area.contains(at.x, at.y) {
                    return None;
                }
                self.menu_open = false;
                if !self.hit(at).is_empty() {
                    self.layout_menu(at);
                    self.menu_open = true;
                    // The press that opened it is not one that puts it away.
                    let _ = window.poll();
                }
                None
            }
            PetEvent::Cancelled => {
                if self.press.down && !self.press.edges.is_none() {
                    self.cancel_resize();
                }
                let mut result = None;
                if self.press.down {
                    // A carry cut short leaves the pet where it was let go: on the screen, and saved there.
                    let carried = self.press.dragged && self.press.action.as_deref() == Some("toggle");
                    self.press.down = false;
                    if carried {
                        self.clamp_position();
                        result = Some(CompanionAction::Moved { left: self.left, top: self.saved_top() });
                    }
                    if self.fit_pending {
                        self.fit(-1, None);
                    }
                }
                self.press.action = None;
                result
            }
            PetEvent::Wheel { pointer, delta } => {
                // Only over a bubble: a stray scroll on the pet must not turn a page. Over a plan that outgrew its box the wheel moves it.
                let at = pointer.window;
                if self.card_dirty {
                    self.render_card();
                }
                // Over the progress card's list that outgrew its box, the wheel moves the list (as the pane's progress panel scrolls).
                let list = self.progress_box;
                if self.show_bubble
                    && !self.menu_open
                    && self.progress().is_some_and(|(_, collapsed)| !collapsed)
                    && list.extent > list.area.height
                    && list.area.contains(at.x, at.y)
                {
                    let moved = (list.scroll - f64::from(*delta) / 120.0 * scene::PLAN_NOTCH).clamp(0.0, list.extent - list.area.height);
                    if moved != list.scroll {
                        self.progress_scroll = Some(moved);
                        self.card_dirty = true;
                    }
                    return None;
                }
                let over_plan = self.card.as_ref().and_then(|c| c.approval.as_ref()).is_some_and(|r| r.plan.is_some());
                if self.show_bubble
                    && !self.menu_open
                    && over_plan
                    && self.plan.extent > self.plan.area.height
                    && self.plan.area.contains(at.x, at.y)
                {
                    let moved = (self.plan.scroll - f64::from(*delta) / 120.0 * scene::PLAN_NOTCH)
                        .clamp(0.0, self.plan.extent - self.plan.area.height);
                    if moved != self.plan.scroll {
                        self.plan.scroll = moved;
                        self.card_dirty = true;
                    }
                    return None;
                }
                let card = self.card.as_ref().filter(|c| self.show_bubble && !self.menu_open && c.count > 1)?;
                if !self.bubble_area().contains(at.x, at.y) {
                    return None;
                }
                self.press.wheel += delta;
                if self.press.wheel.abs() >= 120 {
                    let action = if self.press.wheel > 0 { "next" } else { "previous" };
                    self.press.wheel = 0;
                    return Some(CompanionAction::Act { card: card.key.clone(), action: action.into() });
                }
                None
            }
            PetEvent::DisplayChanged => {
                self.clamp_position();
                None
            }
            PetEvent::DpiChanged { scale } => {
                if self.press.down && !self.press.edges.is_none() {
                    self.cancel_resize();
                }
                self.scale = scale.clamp(1.0, 4.0);
                self.fit(-1, None);
                None
            }
            PetEvent::Failed { message } => Some(CompanionAction::Failed { message: message.clone() }),
            PetEvent::Released(_) => None,
        }
    }

    // ── What the smoke and the tests read back (C#'s `Smoke*` members) ──

    /// The window, the bubble and the pet's frame in DIPs, and the scale (C# `SmokeGeometry`).
    pub fn geometry(&self) -> Geometry {
        Geometry { width: self.panel_width, height: self.height, bubble: self.bubble_area(), pet: self.pet_area(), scale: self.scale }
    }

    /// Where the progress card stands over the bubble, while it shows.
    pub fn progress_rect(&self) -> Option<PetRect> {
        self.progress_area()
    }

    /// The bubble's size and the window's bottom right (C# `SmokeLayout`).
    pub fn placement(&self) -> Placement {
        Placement {
            width: self.width,
            height: self.bubble_height,
            right: self.left + self.pixel_width,
            bottom: self.top + self.pixel_height,
            automatic_height: self.fixed_height.is_none() || self.card.as_ref().is_some_and(|c| c.approval.is_some()),
        }
    }

    /// Where a press on `id` lands (C# `SmokeClick`'s point): `toggle` is the pet, else the open menu's entry or the bubble's button.
    pub fn target(&mut self, id: &str) -> Option<LogicalPoint> {
        if self.card_dirty {
            self.render_card();
        }
        let found = if id == "toggle" {
            Some(self.pet_area())
        } else if self.menu_open {
            self.menu_items.iter().find(|item| item.1 == id).map(|item| item.0)
        } else {
            self.hits.iter().find(|item| item.1 == id).map(|item| item.0)
        }?;
        Some(LogicalPoint { x: found.x + found.width / 2.0, y: found.y + found.height / 2.0 })
    }

    /// The open menu's entries (C# `SmokeMenu`).
    pub fn menu(&self) -> Vec<(String, String)> {
        if self.menu_open { self.menu_items.iter().map(|(_, id, label)| (id.clone(), label.clone())).collect() } else { vec![] }
    }

    /// A point of the bubble (in its own coordinates) as a resize strip or none (C# `SmokeResizeHit`).
    pub fn resize_hit(&self, x: f64, y: f64) -> Edges {
        let bubble = self.bubble_area();
        self.resize_at(LogicalPoint { x: bubble.x + x, y: bubble.y + y })
    }

    /// C# `SmokeResize`: an edge dragged `dx`, `dy` DIPs and let go (or the drag cancelled).
    pub fn smoke_resize(&mut self, edges: Edges, dx: f64, dy: f64, cancel: bool) -> Option<CompanionAction> {
        self.press.start_width = self.width;
        self.press.start_drawn_width = self.drawn_width() as i32;
        self.press.start_height = self.bubble_height;
        self.press.start_fixed = self.fixed_height;
        self.press.start_origin = (self.left, self.top);
        self.press.start_frame = (self.left, self.top, self.pixel_width, self.pixel_height);
        self.press.down = true;
        self.press.dragged = false;
        self.press.swipe = 0;
        self.press.edges = edges;
        self.press.action = Some("resize".into());
        self.press.key = self.card.as_ref().map(|c| c.key.clone());
        self.resize_from_pointer((dx * self.scale).trunc(), (dy * self.scale).trunc());
        if cancel {
            self.cancel_resize();
            return None;
        }
        self.act(&PetEvent::Released(mighty_platform::PetPointer::default()))
    }

    /// The plan's box in the bubble, how tall its whole text is and how far it is scrolled (C# `SmokePlan`).
    pub fn plan_box(&mut self) -> PlanBox {
        if self.card_dirty {
            self.render_card();
        }
        self.plan
    }

    /// The progress card's list as last drawn: its box, how tall the whole list is and how far it is scrolled.
    pub fn progress_box(&self) -> PlanBox {
        self.progress_box
    }

    /// The scale the window draws at.
    pub fn scale(&self) -> f64 {
        self.scale
    }
}

/// A work area in the desktop's own units.
#[derive(Debug, Clone, Copy)]
struct Area {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}
