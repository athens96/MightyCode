//! The window around the bubble (C# Core\CompanionBubbleLayout.cs, the Mac's ResizeEdges.swift; plan §1.7): the bubble's logical-point
//! bounds (258 wide by default, 220-640; a height that follows its content, or one of 100-480), the panel 12 wider on each side and 330
//! tall (the fixed height plus 153), or 494 around an approval; the resize strips (6 at the sides and top, 12 at the two top corners,
//! none at the bottom, which meets the pet), and a resize measured in screen pixels that leaves the opposite edge where it was at every
//! DPI. Owner: W8-E.

use mighty_core::design_tokens::metrics;

pub const DEFAULT_WIDTH: f64 = 258.0;
pub const MINIMUM_WIDTH: f64 = 220.0;
pub const MAXIMUM_WIDTH: f64 = 640.0;
pub const MINIMUM_HEIGHT: f64 = 100.0;
pub const MAXIMUM_HEIGHT: f64 = 480.0;
/// 12 on each side of the bubble, 8 around the column, 2 between the bubble and the pet's 125 × 135 frame.
pub const SIDE_PADDING: f64 = metrics::SPACING_LG;
pub const PADDING: f64 = metrics::SPACING_MD;
pub const GAP: f64 = metrics::SPACING_XXS;
pub const PET_WIDTH: f64 = 125.0;
pub const PET_HEIGHT: f64 = 135.0;
/// Top padding, the gap under the bubble, the pet and the bottom padding.
pub const CHROME: f64 = PADDING + GAP + PET_HEIGHT + PADDING;
/// The window's height around a bubble that follows its content, and around an approval.
pub const BASE_HEIGHT: f64 = 330.0;
pub const TALL_HEIGHT: f64 = 494.0;

/// C# `CompanionResizeEdges` (flags).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct Edges(pub u8);

impl Edges {
    pub const NONE: Edges = Edges(0);
    pub const LEFT: Edges = Edges(1);
    pub const RIGHT: Edges = Edges(2);
    pub const TOP: Edges = Edges(4);
    pub const BOTTOM: Edges = Edges(8);

    pub fn has(self, other: Edges) -> bool {
        self.0 & other.0 == other.0 && other.0 != 0
    }
    pub fn is_none(self) -> bool {
        self.0 == 0
    }
}

impl std::ops::BitOr for Edges {
    type Output = Edges;
    fn bitor(self, other: Edges) -> Edges {
        Edges(self.0 | other.0)
    }
}

/// C# `CompanionBubbleResize(Width, Height, MoveX, MoveY)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BubbleResize {
    pub width: f64,
    pub height: f64,
    pub move_x: f64,
    pub move_y: f64,
}

/// `CompanionBubbleLayout.Width(value)`.
pub fn width(value: Option<f64>) -> f64 {
    value.filter(|w| w.is_finite()).unwrap_or(DEFAULT_WIDTH).clamp(MINIMUM_WIDTH, MAXIMUM_WIDTH)
}

/// `CompanionBubbleLayout.Height(value)`: None keeps the height that follows the content.
pub fn height(value: Option<f64>) -> Option<f64> {
    value.filter(|h| h.is_finite()).map(|h| h.clamp(MINIMUM_HEIGHT, MAXIMUM_HEIGHT))
}

/// `ApprovalWidth`: an approval is never narrower than the default, so its buttons still fit.
pub fn approval_width(value: Option<f64>) -> f64 {
    DEFAULT_WIDTH.max(width(value))
}

/// `PanelSize(width, height, tall)`: the window for a bubble; `tall` is the approval, in its own fixed window.
pub fn panel_size(bubble_width: Option<f64>, bubble_height: Option<f64>, tall: bool) -> (f64, f64) {
    let panel_width = if tall { approval_width(bubble_width) } else { width(bubble_width) } + SIDE_PADDING * 2.0;
    if tall {
        return (panel_width, TALL_HEIGHT);
    }
    (panel_width, BASE_HEIGHT.max(height(bubble_height).map_or(0.0, |fixed| fixed + CHROME)))
}

/// `Vertical(edges)`: the top or bottom is dragged.
pub fn vertical(edges: Edges) -> bool {
    edges.has(Edges::TOP) || edges.has(Edges::BOTTOM)
}

/// `Resize(width, height, dx, dy, scale, edges)`: the pointer's travel in screen pixels as the bubble's new size and how far its
/// left and top move, so the opposite edges stay.
pub fn resize(width_now: f64, height_now: f64, dx: f64, dy: f64, scale: f64, edges: Edges) -> BubbleResize {
    let scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
    let dx = if dx.is_finite() { dx / scale } else { 0.0 };
    let dy = if dy.is_finite() { dy / scale } else { 0.0 };
    let next_width = width(Some(
        width_now
            + if edges.has(Edges::RIGHT) {
                dx
            } else if edges.has(Edges::LEFT) {
                -dx
            } else {
                0.0
            },
    ));
    let next_height = height(Some(
        height_now
            + if edges.has(Edges::BOTTOM) {
                dy
            } else if edges.has(Edges::TOP) {
                -dy
            } else {
                0.0
            },
    ))
    .unwrap_or(MINIMUM_HEIGHT);
    BubbleResize {
        width: next_width,
        height: next_height,
        move_x: if edges.has(Edges::LEFT) { width_now - next_width } else { 0.0 },
        move_y: if edges.has(Edges::TOP) { height_now - next_height } else { 0.0 },
    }
}

/// `Hit(x, y, width, height, vertical)`: thin strips along the bubble's top, left and right edges and its two top corners; the bottom
/// meets the pet, so it has none.
pub fn hit(x: f64, y: f64, width: f64, height: f64, vertical: bool) -> Edges {
    if x < 0.0 || y < 0.0 || x >= width || y >= height {
        return Edges::NONE;
    }
    if vertical && y < 12.0 && x < 12.0 {
        return Edges::LEFT | Edges::TOP;
    }
    if vertical && y < 12.0 && x >= width - 12.0 {
        return Edges::RIGHT | Edges::TOP;
    }
    let mut edges = if x < 6.0 {
        Edges::LEFT
    } else if x >= width - 6.0 {
        Edges::RIGHT
    } else {
        Edges::NONE
    };
    if vertical && y < 6.0 {
        edges = edges | Edges::TOP;
    }
    edges
}
