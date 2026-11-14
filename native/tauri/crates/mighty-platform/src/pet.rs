//! The companion pet's picture as a display list (W8-E, `.omc/research/tauri/w8-plan.md` §0.6, D8): what C#'s `CompanionOverlay`
//! draws with Win2D each frame (WinUI\CompanionOverlay.cs: the sprite cell, the bubble, its texts, buttons, pager, plan blocks and the
//! self-drawn menu), laid out by the shared logic (`mighty_companion::scene` builds it) and drawn by the platform's [`crate::PetWindow`]
//! (Windows: Direct2D and DirectWrite onto a layered window's DIB; the Mac later: CoreGraphics and CoreText in an NSPanel).
//!
//! It lives here, not in mighty-companion, because the platform crates draw it and may depend on nothing but this crate. Every
//! coordinate is in DIPs (device-independent pixels) from the window's top left; the platform scales by its DPI. Colours are already
//! resolved from the design tokens (the scene is drawn as given). Clicks pass through wherever the drawn alpha is 0, so the platform
//! derives its hit-test from the pixels, never from this list. Owner: W8-E.

use std::sync::Arc;

use serde::Serialize;

/// A straight (not premultiplied) sRGB colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct PetColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize)]
pub struct PetRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize)]
pub struct PetPoint {
    pub x: f64,
    pub y: f64,
}

/// The pet's sprite sheet (an atlas of 192×208 cells, C# `Companion.Atlas`), decoded once: premultiplied BGRA, rows tightly packed.
/// `key` names it (the pet's id and the file's stamp), so a platform keeps its copy until the key changes.
#[derive(Clone, PartialEq, Eq)]
pub struct PetBitmap {
    pub key: String,
    pub width: u32,
    pub height: u32,
    pub bgra: Arc<Vec<u8>>,
}

impl std::fmt::Debug for PetBitmap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PetBitmap")
            .field("key", &self.key)
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

/// The pixels never reach a snapshot: the key and the size do.
impl Serialize for PetBitmap {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut out = serializer.serialize_struct("PetBitmap", 3)?;
        out.serialize_field("key", &self.key)?;
        out.serialize_field("width", &self.width)?;
        out.serialize_field("height", &self.height)?;
        out.end()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PetTextAlign {
    #[default]
    Leading,
    Center,
    Trailing,
}

/// One run of a text: its words and how they look (C#'s `SetFontWeight` spans, the plan blocks' bold and code).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PetTextRun {
    pub text: String,
    pub color: PetColor,
    pub bold: bool,
    pub mono: bool,
}

/// A text laid out in `rect` (C# `CompanionOverlayText`: size, weight, family, lines, trimmed, ink, bounds).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PetText {
    pub rect: PetRect,
    pub runs: Vec<PetTextRun>,
    /// The font family (C#'s `Body` stack or the code font); the platform falls back as DirectWrite does.
    pub family: String,
    pub size: f64,
    /// 400 regular, 600 semibold, 700 bold (C# draws 500 as 400).
    pub weight: u16,
    /// At most this many lines; 0 is unlimited.
    pub max_lines: u32,
    /// The last line ends in an ellipsis when cut.
    pub trimmed: bool,
    pub align: PetTextAlign,
}

/// One thing drawn, in order (later items over earlier ones).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum PetItem {
    /// `FillRoundedRectangle` (radius 0: `FillRectangle`).
    Fill {
        rect: PetRect,
        radius: f64,
        color: PetColor,
    },
    /// `DrawRoundedRectangle` with a stroke of `width`, centred on the rectangle's edge.
    Stroke {
        rect: PetRect,
        radius: f64,
        color: PetColor,
        width: f64,
    },
    /// `FillEllipse`.
    Ellipse {
        center: PetPoint,
        radius_x: f64,
        radius_y: f64,
        color: PetColor,
    },
    /// `DrawLine`, round caps when `round`.
    Line {
        from: PetPoint,
        to: PetPoint,
        color: PetColor,
        width: f64,
        round: bool,
    },
    Text(PetText),
    /// A cell of the scene's atlas (`cell` in atlas pixels) drawn into `dest`, high-quality cubic (C#
    /// `CanvasImageInterpolation.HighQualityCubic`).
    Sprite {
        cell: PetRect,
        dest: PetRect,
        opacity: f64,
    },
    /// Its items drawn clipped to `rect` (the plan box's scroll area).
    Clip {
        rect: PetRect,
        items: Vec<PetItem>,
    },
    /// Its items, named for what they are (`bubble`, `button:<id>`, `pager`, `menu`, `menu:<id>`, `plan`): the platform draws the
    /// items and ignores the role; the logic's hit areas and the snapshots read it.
    Group {
        role: String,
        rect: PetRect,
        items: Vec<PetItem>,
    },
}

/// The whole window's picture for one frame.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct PetScene {
    /// The window's size in DIPs (C#: 258 wide plus padding; 330 tall, 494 with an approval).
    pub width: f64,
    pub height: f64,
    pub items: Vec<PetItem>,
    /// The sprite sheet [`PetItem::Sprite`] crops from.
    pub atlas: Option<PetBitmap>,
}
