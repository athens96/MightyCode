//! The companion pet's picture as a display list (W8-E, `.omc/research/tauri/w8-plan.md` §0.6, D8): what C#'s `CompanionOverlay`
//! draws with Win2D each frame (WinUI\CompanionOverlay.cs: the sprite cell, the bubble, its texts, buttons, pager, plan blocks and the
//! self-drawn menu), laid out by the shared logic (`mighty_companion::scene` builds it) and drawn by the platform's [`crate::PetWindow`]
//! (Windows: Direct2D and DirectWrite onto a layered window's DIB; the Mac later: CoreGraphics and CoreText in an NSPanel).
//!
//! It lives here, not in mighty-companion, because the platform crates draw it and may depend on nothing but this crate. Every
//! coordinate is in DIPs (device-independent pixels) from the window's top left; the platform draws at the scene's `scale` (pixels a
//! DIP). Colours are already resolved from the design tokens (the scene is drawn as given). Clicks pass through wherever the drawn alpha
//! is 0, so the platform derives its hit-test from the pixels, never from this list.
//!
//! The layout needs the words measured as the platform will draw them (C# measures with the same DirectWrite layouts it draws), so a
//! platform also answers [`PetMeasure`]; [`ApproximateMeasure`] stands in where none can (the tests' golden scenes, an OS without a pet
//! window). Owner: W8-E.

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

impl PetRect {
    pub const fn new(x: f64, y: f64, width: f64, height: f64) -> PetRect {
        PetRect { x, y, width, height }
    }
    pub fn right(&self) -> f64 {
        self.x + self.width
    }
    pub fn bottom(&self) -> f64 {
        self.y + self.height
    }
    /// C# `Inside`: left and top inclusive, right and bottom exclusive.
    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }
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

/// One run of a text: its words and how they look (C#'s `SetFontWeight`/`SetFontStyle`/`SetFontFamily` spans, the plan blocks' bold,
/// italic and code). A `mono` run inside a text of another family is set in [`MONO`] one point smaller (C# `SetFontSize(size - 1)`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PetTextRun {
    pub text: String,
    pub color: PetColor,
    pub bold: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub italic: bool,
    pub mono: bool,
}

/// The body font stack (C# `DesignMetrics.Font.Body`): the platform draws the first family of it that is installed, else Segoe UI.
pub const BODY: &str = "Segoe UI Variable Text, Segoe UI";
/// The code font stack (C# `DesignMetrics.Font.Mono`).
pub const MONO: &str = "Cascadia Mono, Consolas";
/// The icon font a FontIcon uses (C# `Icons`).
pub const ICONS: &str = "Segoe Fluent Icons, Segoe MDL2 Assets";
/// The symbol font that has the raised hand (C# `Symbols`).
pub const SYMBOLS: &str = "Segoe UI Symbol";
/// SF Pro sets a line 1.19 of its size, Segoe UI and the Hangul fallback 1.33: every text not in [`MONO`] is set at nine tenths of its
/// font's line spacing (C# `LineScale`, `CanvasLineSpacingMode.Proportional`), which keeps the Mac's rows (25 for the header, 13 a line of
/// 11pt words).
pub const LINE_SCALE: f64 = 0.9;
/// The width and height a one-line text is laid out in (C# `Wide`).
pub const WIDE: f64 = 4096.0;

/// A text laid out in `rect` (C# `CompanionOverlayText`: size, weight, family, lines, trimmed, ink, bounds): `rect.width` is the width
/// it wraps (or, on one line, ends in an ellipsis) at, `rect.height` the height it measured.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PetText {
    pub rect: PetRect,
    /// Where its lines stand: `rect`'s place, the width and height they measured (C# `Bounds`).
    pub bounds: PetRect,
    /// The words it stands for (C#'s `Value`: what the smoke looks for), which `runs` may show shortened.
    pub value: String,
    pub runs: Vec<PetTextRun>,
    /// The font stack ([`BODY`] or [`MONO`]); the platform falls back as DirectWrite does.
    pub family: String,
    pub size: f64,
    /// 400 regular, 600 semibold, 700 bold, 800 extra bold (C# draws 500 as 400; [`PetText::weight`] keeps what the design asked for in
    /// [`PetText::asked`]).
    pub weight: u16,
    /// The weight the design asks for (500 drawn as 400): what the smoke compares with C#'s `CompanionOverlayText.Weight`.
    pub asked: u16,
    /// At most this many lines (1: no wrapping, an ellipsis where it is too wide); 0 is unlimited.
    pub max_lines: u32,
    /// How many lines it measured.
    pub lines: u32,
    /// Its words were shortened to fit (an ellipsis on its last line).
    pub trimmed: bool,
    pub align: PetTextAlign,
    /// Tabular figures (C# `CanvasTypographyFeatureName.TabularFigures`).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub tabular: bool,
    /// The design token its ink is (`ink`, `ink2`, `onAccent`, `errText`): the colour is in the runs; the smoke reads the token.
    pub ink: String,
}

/// How a [`PetItem::Glyph`] is sized.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind", content = "value")]
pub enum PetGlyphSize {
    /// Its ink this tall (C# `Icon`: an SF Symbol draws about as tall as its font size, an icon font's glyph does not).
    Ink(f64),
    /// At this font size (C#'s question mark, 9pt at weight 800).
    Font(f64),
}

/// One straight or curved piece of a [`PetItem::Path`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum PetSegment {
    Line { to: PetPoint },
    Cubic { first: PetPoint, second: PetPoint, to: PetPoint },
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
    /// `FillEllipse`, or `DrawEllipse` with a stroke of that width.
    Ellipse {
        center: PetPoint,
        radius_x: f64,
        radius_y: f64,
        color: PetColor,
        #[serde(skip_serializing_if = "Option::is_none")]
        stroke: Option<f64>,
    },
    /// `DrawLine`, round caps when `round`.
    Line {
        from: PetPoint,
        to: PetPoint,
        color: PetColor,
        width: f64,
        round: bool,
    },
    /// An open arc stroked with round caps (C# `Spinner`): from `start` radians, `sweep` radians clockwise.
    Arc {
        center: PetPoint,
        radius: f64,
        start: f64,
        sweep: f64,
        color: PetColor,
        width: f64,
    },
    /// A closed figure filled (C#'s paw pad and the question bubble's tail).
    Path {
        start: PetPoint,
        segments: Vec<PetSegment>,
        color: PetColor,
    },
    Text(PetText),
    /// A glyph of a font centred on `center` by its ink (C# `Icon`), and `over` a second glyph of the same cell drawn on it in its own
    /// colour (the error mark's exclamation cut in the card's colour).
    Glyph {
        family: String,
        glyph: String,
        center: PetPoint,
        size: PetGlyphSize,
        weight: u16,
        color: PetColor,
        #[serde(skip_serializing_if = "Option::is_none")]
        over: Option<(String, PetColor)>,
    },
    /// A cell of the scene's atlas (`cell` in atlas pixels) drawn into `dest`, high-quality cubic (C#
    /// `CanvasImageInterpolation.HighQualityCubic`), cropped first so the scaling never reads the next frame.
    Sprite {
        cell: PetRect,
        dest: PetRect,
        opacity: f64,
    },
    /// Its items drawn clipped to `rect` (the plan box's scroll area, a bubble held to the screen's height).
    Clip {
        rect: PetRect,
        items: Vec<PetItem>,
    },
    /// Its items, named for what they are (`bubble`, `button:<id>`, `pager`, `menu`, `menu:<id>`, `plan`, `pet`), at `opacity` (a
    /// disabled button's layer): the platform draws the items and ignores the role; the logic's hit areas and the snapshots read it.
    Group {
        role: String,
        rect: PetRect,
        #[serde(skip_serializing_if = "is_one")]
        opacity: f64,
        items: Vec<PetItem>,
    },
}

fn is_one(value: &f64) -> bool {
    *value == 1.0
}

/// The whole window's picture for one frame.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct PetScene {
    /// The window's size in DIPs (C#: 258 wide plus padding; 330 tall, 494 with an approval).
    pub width: f64,
    pub height: f64,
    /// The pixels a DIP (the window's display scale, 1 to 4): the window is `ceil(width × scale)` × `ceil(height × scale)` pixels.
    pub scale: f64,
    /// Where the window's top left stands, in the desktop's own units (physical pixels on Windows, points on the Mac): the picture and
    /// its place arrive together (C#'s `UpdateLayeredWindow` carries both).
    pub origin: PetPoint,
    pub items: Vec<PetItem>,
    /// The sprite sheet [`PetItem::Sprite`] crops from.
    pub atlas: Option<PetBitmap>,
}

impl PetScene {
    /// The window's size in pixels at its scale (C# `Math.Ceiling(panelWidth * scale)`).
    pub fn pixel_size(&self) -> (u32, u32) {
        let scale = if self.scale.is_finite() && self.scale > 0.0 { self.scale } else { 1.0 };
        ((self.width * scale).ceil().max(1.0) as u32, (self.height * scale).ceil().max(1.0) as u32)
    }

    /// Every text drawn, in order, through groups and clips (what C#'s `SmokeTexts` collects).
    pub fn texts(&self) -> Vec<&PetText> {
        fn walk<'a>(items: &'a [PetItem], out: &mut Vec<&'a PetText>) {
            for item in items {
                match item {
                    PetItem::Text(text) => out.push(text),
                    PetItem::Clip { items, .. } | PetItem::Group { items, .. } => walk(items, out),
                    _ => {}
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.items, &mut out);
        out
    }

    /// The group of this role, first found.
    pub fn group(&self, role: &str) -> Option<&PetItem> {
        fn walk<'a>(items: &'a [PetItem], role: &str) -> Option<&'a PetItem> {
            for item in items {
                match item {
                    PetItem::Group { role: named, items, .. } => {
                        if named == role {
                            return Some(item);
                        }
                        if let Some(found) = walk(items, role) {
                            return Some(found);
                        }
                    }
                    PetItem::Clip { items, .. } => {
                        if let Some(found) = walk(items, role) {
                            return Some(found);
                        }
                    }
                    _ => {}
                }
            }
            None
        }
        walk(&self.items, role)
    }
}

/// One laid-out text as the platform measured it: the bounds of its lines (C# `LayoutBounds`), how many characters each line holds,
/// and whether its last line ends in the platform's ellipsis.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PetLayout {
    pub width: f64,
    pub height: f64,
    /// Characters (Unicode scalar values) on each line, in order.
    pub line_chars: Vec<usize>,
    pub trimmed: bool,
}

/// A text as it will be drawn: its bounds and lines, and the words it shows when they had to be shortened to its line limit.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PetTextSize {
    pub width: f64,
    pub height: f64,
    pub lines: u32,
    pub trimmed: bool,
    /// The words to draw in place of the text's own (one run), when they were cut.
    pub shown: Option<String>,
}

/// The characters DirectWrite starts a new line at (C# `LineBreaks`).
pub const LINE_BREAKS: [char; 7] = ['\r', '\n', '\u{0B}', '\u{0C}', '\u{85}', '\u{2028}', '\u{2029}'];

/// Measures words as the platform draws them (C# `CanvasTextLayout` over the same format it draws with).
pub trait PetMeasure: Send + Sync {
    /// The text laid out at `text.rect.width` (no wrapping when `max_lines` is 1), its height unbounded.
    fn lay(&self, text: &PetText) -> PetLayout;

    /// C# `Text(value, size, weight, wide, lines, family, tabular)`: a text as SwiftUI lays one out. `max_lines` is its lineLimit (0
    /// for none) and what does not fit ends in an ellipsis on its last line, never cut through: one line ends at its first break with
    /// the ellipsis that says more follows; a paragraph too tall keeps the words of the lines that fit, less as many characters as the
    /// ellipsis needs to stay on the last of them.
    fn measure(&self, text: &PetText) -> PetTextSize {
        let single = text.runs.len() == 1;
        let mut working = text.clone();
        let mut shown = None;
        if text.max_lines == 1
            && single
            && let Some(at) = text.runs[0].text.find(LINE_BREAKS)
        {
            let cut = format!("{}\u{2026}", dotnet_trim_end(&text.runs[0].text[..at]));
            working.runs[0].text = cut.clone();
            shown = Some(cut);
        }
        let layout = self.lay(&working);
        let lines = text.max_lines as usize;
        if lines <= 1 || layout.line_chars.len() <= lines || !single {
            let trimmed = shown.is_some() || layout.trimmed;
            return PetTextSize { width: layout.width, height: layout.height, lines: layout.line_chars.len() as u32, trimmed, shown };
        }
        let value: Vec<char> = text.runs[0].text.chars().collect();
        let cut: usize = layout.line_chars.iter().take(lines).sum();
        let mut kept = cut.min(value.len());
        loop {
            let prefix: String = value[..kept].iter().collect();
            let words =
                if kept < value.len() { format!("{}\u{2026}", dotnet_trim_end(&prefix)) } else { dotnet_trim_end(&prefix).to_string() };
            working.runs[0].text = words.clone();
            let laid = self.lay(&working);
            if kept == 0 || laid.line_chars.len() <= lines {
                return PetTextSize {
                    width: laid.width,
                    height: laid.height,
                    lines: laid.line_chars.len() as u32,
                    trimmed: true,
                    shown: Some(words),
                };
            }
            kept -= 1;
        }
    }
}

/// .NET's `TrimEnd()`: white space off the end.
pub fn dotnet_trim_end(value: &str) -> &str {
    value.trim_end_matches(char::is_whitespace)
}

/// A measure that needs no font engine: a fixed advance for each kind of character and the line spacing of Segoe UI (1.33 of the size,
/// at [`LINE_SCALE`]) or of the code face (1.17), wrapped at spaces and broken inside a word too long for its line. The tests' golden
/// scenes and an OS without a pet window lay out with it; the Windows window measures with DirectWrite.
#[derive(Debug, Clone, Copy, Default)]
pub struct ApproximateMeasure;

impl ApproximateMeasure {
    fn advance(c: char, size: f64, bold: bool, mono: bool) -> f64 {
        let wide =
            matches!(c as u32, 0x1100..=0x11FF | 0x2E80..=0x9FFF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF | 0xFF00..=0xFF60 | 0x1F300..=0x1FAFF);
        let em = if mono {
            if wide { 1.2 } else { 0.6 }
        } else if wide {
            1.0
        } else {
            match c {
                ' ' => 0.27,
                'i' | 'j' | 'l' | 'I' | '.' | ',' | ':' | ';' | '\'' | '|' | '!' => 0.25,
                'f' | 't' | 'r' | '(' | ')' | '[' | ']' => 0.34,
                'm' | 'w' | 'M' | 'W' => 0.85,
                '0'..='9' => 0.55,
                'A'..='Z' => 0.64,
                'a'..='z' => 0.52,
                '\u{2026}' => 0.8,
                _ => 0.6,
            }
        };
        em * size * if bold { 1.06 } else { 1.0 }
    }

    fn line_height(text: &PetText) -> f64 {
        if text.family == MONO { text.size * 1.17 } else { text.size * 1.33 * LINE_SCALE }
    }
}

impl PetMeasure for ApproximateMeasure {
    fn lay(&self, text: &PetText) -> PetLayout {
        let mono_text = text.family == MONO;
        let bold_text = text.weight >= 600;
        // Every character with its advance, a break where the words break the line.
        let mut glyphs: Vec<(char, f64)> = Vec::new();
        for run in &text.runs {
            let mono = mono_text || run.mono;
            let size = if run.mono && !mono_text { text.size - 1.0 } else { text.size };
            for c in run.text.chars() {
                glyphs.push((c, Self::advance(c, size, bold_text || run.bold, mono)));
            }
        }
        let limit = if text.rect.width > 0.0 { text.rect.width } else { WIDE };
        let one_line = text.max_lines == 1;
        let mut lines: Vec<(usize, f64)> = Vec::new();
        let (mut count, mut width, mut ink) = (0usize, 0.0f64, 0.0f64);
        let mut trimmed = false;
        let mut at = 0;
        while at < glyphs.len() {
            let (c, advance) = glyphs[at];
            if LINE_BREAKS.contains(&c) {
                if !one_line {
                    lines.push((count + 1, ink));
                    (count, width, ink) = (0, 0.0, 0.0);
                    at += 1;
                    continue;
                }
                count += 1;
                at += 1;
                continue;
            }
            if one_line {
                count += 1;
                width += advance;
                if c != ' ' {
                    ink = width;
                }
                at += 1;
                continue;
            }
            if c == ' ' {
                count += 1;
                width += advance;
                at += 1;
                continue;
            }
            // A word: its characters up to the next space or break.
            let end = (at..glyphs.len()).find(|&i| glyphs[i].0 == ' ' || LINE_BREAKS.contains(&glyphs[i].0)).unwrap_or(glyphs.len());
            let word: f64 = glyphs[at..end].iter().map(|g| g.1).sum();
            if width + word > limit && count > 0 && ink > 0.0 {
                lines.push((count, ink));
                (count, width, ink) = (0, 0.0, 0.0);
            }
            if word > limit {
                // Too wide for any line: broken where it reaches the edge (EmergencyBreak).
                for glyph in &glyphs[at..end] {
                    if width + glyph.1 > limit && count > 0 {
                        lines.push((count, ink));
                        (count, width) = (0, 0.0);
                    }
                    count += 1;
                    width += glyph.1;
                    ink = width;
                }
            } else {
                count += end - at;
                width += word;
                ink = width;
            }
            at = end;
        }
        if count > 0 || lines.is_empty() {
            lines.push((count, ink));
        }
        if one_line && lines[0].1 > limit {
            trimmed = true;
            lines[0].1 = limit;
        }
        let height = Self::line_height(text) * lines.len() as f64;
        PetLayout {
            width: lines.iter().map(|l| l.1).fold(0.0, f64::max),
            height: if glyphs.is_empty() { Self::line_height(text) } else { height },
            line_chars: lines.iter().map(|l| l.0).collect(),
            trimmed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(words: &str, width: f64, lines: u32) -> PetText {
        PetText {
            rect: PetRect::new(0.0, 0.0, width, 0.0),
            bounds: PetRect::default(),
            value: words.into(),
            runs: vec![PetTextRun { text: words.into(), color: PetColor::default(), bold: false, italic: false, mono: false }],
            family: BODY.into(),
            size: 11.0,
            weight: 400,
            asked: 400,
            max_lines: lines,
            lines: 0,
            trimmed: false,
            align: PetTextAlign::Leading,
            tabular: false,
            ink: "ink".into(),
        }
    }

    #[test]
    fn one_line_ends_at_its_first_break_and_a_paragraph_at_its_limit() {
        let measure = ApproximateMeasure;
        let one = measure.measure(&text("first line\nsecond", WIDE, 1));
        assert_eq!((one.shown.as_deref(), one.lines, one.trimmed), (Some("first line\u{2026}"), 1, true));
        let long = "word ".repeat(80);
        let two = measure.measure(&text(&long, 120.0, 2));
        assert_eq!(two.lines, 2);
        assert!(two.trimmed && two.shown.as_deref().is_some_and(|s| s.ends_with('\u{2026}')));
        let whole = measure.measure(&text("short", 120.0, 2));
        assert_eq!((whole.lines, whole.trimmed, whole.shown), (1, false, None));
        let wide = measure.measure(&text(&long, 100.0, 1));
        assert!(wide.trimmed && wide.width <= 100.0 && wide.lines == 1);
    }

    #[test]
    fn the_scene_finds_its_texts_and_groups() {
        let scene = PetScene {
            items: vec![PetItem::Group {
                role: "bubble".into(),
                rect: PetRect::default(),
                opacity: 1.0,
                items: vec![PetItem::Clip { rect: PetRect::default(), items: vec![PetItem::Text(text("a", 10.0, 1))] }],
            }],
            ..PetScene::default()
        };
        assert_eq!(scene.texts().len(), 1);
        assert!(scene.group("bubble").is_some() && scene.group("menu").is_none());
        assert_eq!(PetScene { width: 282.0, height: 330.0, scale: 1.5, ..PetScene::default() }.pixel_size(), (423, 495));
    }
}
