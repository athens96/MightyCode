//! The companion pet (W8-E, `.omc/research/tauri/w8-plan.md` §1.7, D8-D10; C# Core\Companion.cs, CompanionBubble.cs,
//! CompanionBubbleLayout.cs and the pure half of WinUI\CompanionOverlay.cs; docs/windows-companion.md). No Tauri and no OS code: it
//! lays out a [`scene::PetScene`] each frame and the platform's `PetWindow` draws it (Windows: Direct2D and DirectWrite on a layered
//! window, mighty-platform-windows `pet.rs`). The app's half is src-tauri `companion.rs` (the 1/12 s timer, the followed panes, the
//! card's actions, the popover, the status bar's controls) and `settings/companion.rs`.
//!
//! | Module | C# | Owner |
//! |---|---|---|
//! | [`prefs`] | `CompanionPreferences` (`companion-settings.json`) | W8-E |
//! | [`pet`] | `CompanionPetCatalog`, `CompanionAtlas` (the catalog and the atlas checks) | W8-E |
//! | [`animation`] | `Companion.cs:137-153` (rows, `Priority`, timing) | W8-E |
//! | [`carousel`] | `CompanionCarousel` | W8-E |
//! | [`bubble`] | `CompanionBubble` (buttons, questionnaire stepping) | W8-E |
//! | [`plan`] | `CompanionPlanBlock` (the plan drawn as blocks) | W8-E |
//! | [`layout`] | `CompanionBubbleLayout`, `ResizeEdges` (layout, hit areas, resize strips, DPI anchors) | W8-E |
//! | [`scene`] | `CompanionOverlay`'s drawing as a display list | W8-E |
//!
//! At rest (W8 Step 0): every module is empty and [`Companion`] shows nothing.

pub mod animation;
pub mod bubble;
pub mod carousel;
pub mod layout;
pub mod pet;
pub mod plan;
pub mod prefs;
pub mod scene;

use mighty_platform::{PetEvent, PetScene};

/// What a press on the pet or its bubble asks the app to do (C# `CompanionOverlay`'s callbacks to `MainWindow.Companion`): answer the
/// card shown, open its pane, hide the pet, and so on. The app answers through its run service (src-tauri `companion.rs`).
#[derive(Debug, Clone, PartialEq)]
pub enum CompanionAction {
    /// A bubble button (`allow`, `deny`, `approve`, `revise`, …) for the card with this key.
    Button { card: String, button: String },
    /// Open the pane the bubble follows (C# opens a separate WinUI window for review, questions and plan; here the page's own card,
    /// D10).
    OpenPane { pane: String },
    /// The self-drawn menu's "hide".
    Hide,
    /// The window moved: its new top left, to be saved (`left`, `top`).
    Moved { left: f64, top: f64 },
    /// The bubble was resized: its new size, to be saved (`bubbleWidth`, `bubbleHeight`).
    Resized { width: f64, height: f64 },
}

/// The pet's state between frames: what it follows, its animation, its bubble and its window's size and place.
#[derive(Debug, Default)]
pub struct Companion {}

impl Companion {
    /// One 1/12 s frame: the animation moves on (the followed panes' state is read every 12th tick by the caller).
    pub fn tick(&mut self) {}

    /// The frame's picture.
    pub fn scene(&self) -> PetScene {
        PetScene::default()
    }

    /// One event of the pet's window; what the app must do about it, if anything.
    pub fn act(&mut self, event: &PetEvent) -> Option<CompanionAction> {
        let _ = event;
        None
    }
}
