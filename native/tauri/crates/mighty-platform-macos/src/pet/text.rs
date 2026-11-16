//! The pet's words on the Mac: laid out and drawn with CoreText in the system fonts SwiftUI drew them in (Swift
//! `CompanionOverlayView`: SF Pro for the body stack, SF Mono for the code stack, `.monospacedDigit()` where the figures are tabular),
//! measured with the same lines that are drawn ([`CoreTextMeasure`], as DirectWrite measures what it draws on Windows).
//!
//! The Segoe stacks of the scene (`BODY`, `MONO`) name no font installed on a Mac, so they read as the system's faces. A line is the
//! font's own line height (SF Pro's 1.19 of its size: 13 for 11pt words, the Mac's rows `LINE_SCALE` keeps on Windows), the first
//! baseline its ascent below the text's top. One line ends in an ellipsis where it is too wide (`CTLineCreateTruncatedLine`); more
//! lines wrap at words and break a word too long for its line (`CTTypesetterSuggestLineBreak`).

use std::collections::HashMap;
use std::ptr::NonNull;
use std::sync::Mutex;

use mighty_platform::pet::{MONO, PetColor, PetLayout, PetMeasure, PetText, PetTextAlign, WIDE};
use objc2::rc::Retained;
use objc2_app_kit::NSFont;
use objc2_core_foundation::{CFMutableAttributedString, CFRange, CFRetained, CFString, CGAffineTransform, CGFloat};
use objc2_core_graphics::{CGColor, CGColorSpace, CGContext};
use objc2_core_text::{
    CTFont, CTFontSymbolicTraits, CTLine, CTLineTruncationType, CTTypesetter, kCTFontAttributeName, kCTForegroundColorAttributeName,
};

/// A font (CoreText's fonts are immutable and may be used from any thread).
#[derive(Clone)]
pub struct Font(pub CFRetained<CTFont>);
// SAFETY: CTFont is immutable and documented thread-safe.
unsafe impl Send for Font {}
// SAFETY: as above.
unsafe impl Sync for Font {}

/// A colour of the device's RGB (the bitmap's own space: no colour matching, so a fill is the value it was given).
pub struct Color(pub CFRetained<CGColor>);

impl Color {
    pub fn new(color: PetColor) -> Option<Color> {
        let space = CGColorSpace::new_device_rgb()?;
        let parts: [CGFloat; 4] =
            [f64::from(color.r) / 255.0, f64::from(color.g) / 255.0, f64::from(color.b) / 255.0, f64::from(color.a) / 255.0];
        // SAFETY: four components for an RGB space with alpha.
        unsafe { CGColor::new(Some(&space), parts.as_ptr()) }.map(Color)
    }
}

/// NSFont's weight for a CSS weight (NSFontWeightRegular … NSFontWeightHeavy).
fn ns_weight(weight: u16) -> CGFloat {
    match weight {
        0..=450 => 0.0,
        451..=550 => 0.23,
        551..=650 => 0.3,
        651..=750 => 0.4,
        _ => 0.56,
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct FontKey {
    mono: bool,
    size: u64,
    weight: u16,
    italic: bool,
    tabular: bool,
}

/// The system faces the pet draws in, made once each.
#[derive(Default)]
pub struct Text {
    fonts: Mutex<HashMap<FontKey, Font>>,
}

/// One text laid out: its lines, the height of each, the first line's ascent, whether one line was cut.
pub struct Laid {
    pub lines: Vec<Line>,
    pub line_height: f64,
    pub ascent: f64,
    pub trimmed: bool,
    pub width: f64,
    pub limit: f64,
}

pub struct Line {
    pub line: CFRetained<CTLine>,
    pub chars: usize,
    /// Its width without the white space it ends in.
    pub width: f64,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Text {
    /// The system face of the stack (`mono`: the code stack), at a size, weight and slant, its figures tabular when asked.
    pub fn font(&self, mono: bool, size: f64, weight: u16, italic: bool, tabular: bool) -> Font {
        let key = FontKey { mono, size: size.to_bits(), weight, italic, tabular };
        if let Some(found) = lock(&self.fonts).get(&key) {
            return found.clone();
        }
        let weight = ns_weight(weight);
        let made: Retained<NSFont> = if mono {
            NSFont::monospacedSystemFontOfSize_weight(size, weight)
        } else if tabular {
            NSFont::monospacedDigitSystemFontOfSize_weight(size, weight)
        } else {
            NSFont::systemFontOfSize_weight(size, weight)
        };
        // SAFETY: NSFont is toll-free bridged with CTFont; the pointer is a live font, retained here as a CTFont.
        let mut font = unsafe { CFRetained::retain(NonNull::new_unchecked(Retained::as_ptr(&made) as *mut CTFont)) };
        if italic {
            // SAFETY: a copy of a live font with the italic trait; no matrix.
            if let Some(slanted) = unsafe {
                font.copy_with_symbolic_traits(0.0, std::ptr::null(), CTFontSymbolicTraits::TraitItalic, CTFontSymbolicTraits::TraitItalic)
            } {
                font = slanted;
            }
        }
        let font = Font(font);
        lock(&self.fonts).insert(key, font.clone());
        font
    }

    /// The text's own face (its stack, size and weight).
    pub fn base(&self, text: &PetText) -> Font {
        self.font(text.family == MONO, text.size, text.weight, false, text.tabular)
    }

    /// The text's words with each run's face and colour, and the UTF-16 they are.
    pub fn attributed(&self, text: &PetText) -> (CFRetained<CFMutableAttributedString>, Vec<u16>) {
        let words: String = text.runs.iter().map(|run| run.text.as_str()).collect();
        let units: Vec<u16> = words.encode_utf16().collect();
        let string = CFMutableAttributedString::new(None, 0).expect("CFAttributedStringCreateMutable");
        let mono_text = text.family == MONO;
        // SAFETY: CFAttributedString calls on the string made here, over ranges inside its words (counted in UTF-16, as CF counts).
        unsafe {
            CFMutableAttributedString::replace_string(Some(&string), CFRange { location: 0, length: 0 }, Some(&CFString::from_str(&words)));
            let mut at = 0isize;
            for run in &text.runs {
                let length = run.text.encode_utf16().count() as isize;
                if length == 0 {
                    continue;
                }
                let range = CFRange { location: at, length };
                let mono = mono_text || run.mono;
                let size = if run.mono && !mono_text { text.size - 1.0 } else { text.size };
                let weight = if run.bold { 700 } else { text.weight };
                let font = self.font(mono, size, weight, run.italic, text.tabular && !run.mono);
                CFMutableAttributedString::set_attribute(Some(&string), range, Some(kCTFontAttributeName), Some(&font.0));
                if let Some(color) = Color::new(run.color) {
                    CFMutableAttributedString::set_attribute(Some(&string), range, Some(kCTForegroundColorAttributeName), Some(&color.0));
                }
                at += length;
            }
        }
        (string, units)
    }

    /// The text laid out at its width (one line when `max_lines` is 1, cut with an ellipsis where it is too wide).
    pub fn lay(&self, text: &PetText) -> Laid {
        let base = self.base(text);
        // SAFETY: plain metric reads of a live font.
        let (ascent, descent, leading) = unsafe { (base.0.ascent(), base.0.descent(), base.0.leading()) };
        let line_height = ascent + descent + leading;
        let limit = if text.rect.width > 0.0 { text.rect.width } else { WIDE };
        let (string, units) = self.attributed(text);
        let mut lines = Vec::new();
        let mut trimmed = false;
        let chars = |from: usize, count: usize| char::decode_utf16(units[from..(from + count).min(units.len())].iter().copied()).count();
        // SAFETY: CoreText calls on the typesetter and lines made from the string above, over ranges inside it.
        unsafe {
            let measure = |line: &CTLine| {
                let width = line.typographic_bounds(std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut());
                (width - line.trailing_whitespace_width()).max(0.0)
            };
            if units.is_empty() {
                let line = CTLine::with_attributed_string(&string);
                lines.push(Line { line, chars: 0, width: 0.0 });
            } else if text.max_lines == 1 {
                let line = CTLine::with_attributed_string(&string);
                let width = measure(&line);
                trimmed = width > limit;
                lines.push(Line { line, chars: chars(0, units.len()), width: width.min(limit) });
            } else {
                let setter = CTTypesetter::with_attributed_string(&string);
                let mut start = 0usize;
                while start < units.len() {
                    let count = setter.suggest_line_break(start as isize, limit).max(1) as usize;
                    let line = setter.line(CFRange { location: start as isize, length: count as isize });
                    let width = measure(&line);
                    lines.push(Line { line, chars: chars(start, count), width });
                    start += count;
                }
            }
        }
        let width = lines.iter().map(|l| l.width).fold(0.0, f64::max);
        Laid { lines, line_height, ascent, trimmed, width, limit }
    }

    /// Draws the text at its place (`text.rect`'s top left, its alignment within `text.rect.width`) on a context whose space runs down
    /// from the top (the pet's DIPs).
    pub fn draw(&self, context: &CGContext, text: &PetText) {
        let laid = self.lay(text);
        let flip = CGAffineTransform { a: 1.0, b: 0.0, c: 0.0, d: -1.0, tx: 0.0, ty: 0.0 };
        CGContext::set_text_matrix(Some(context), flip);
        for (index, line) in laid.lines.iter().enumerate() {
            let drawn = if laid.trimmed { self.truncated(text, &line.line, laid.limit) } else { None };
            let shown = drawn.as_ref().unwrap_or(&line.line);
            let x = match text.align {
                PetTextAlign::Leading => text.rect.x,
                PetTextAlign::Center => text.rect.x + (text.rect.width - line.width) / 2.0,
                PetTextAlign::Trailing => text.rect.x + text.rect.width - line.width,
            };
            let baseline = text.rect.y + index as f64 * laid.line_height + laid.ascent;
            CGContext::set_text_position(Some(context), x, baseline);
            // SAFETY: a live line drawn on a live context.
            unsafe { shown.draw(context) };
        }
    }

    /// The line cut at `width` with an ellipsis in the face and colour of its last run.
    fn truncated(&self, text: &PetText, line: &CTLine, width: f64) -> Option<CFRetained<CTLine>> {
        let last = text.runs.last()?;
        let token = PetText { runs: vec![mighty_platform::pet::PetTextRun { text: "\u{2026}".into(), ..last.clone() }], ..text.clone() };
        let (string, _) = self.attributed(&token);
        // SAFETY: CoreText calls on live lines.
        unsafe {
            let ellipsis = CTLine::with_attributed_string(&string);
            line.truncated_line(width, CTLineTruncationType::End, Some(&ellipsis))
        }
    }
}

/// The words measured with the lines this window draws.
pub struct CoreTextMeasure(pub std::sync::Arc<Text>);

impl PetMeasure for CoreTextMeasure {
    fn lay(&self, text: &PetText) -> PetLayout {
        let laid = self.0.lay(text);
        PetLayout {
            width: laid.width,
            height: laid.line_height * laid.lines.len() as f64,
            line_chars: laid.lines.iter().map(|l| l.chars).collect(),
            trimmed: laid.trimmed,
        }
    }
}

#[cfg(test)]
mod tests {
    use mighty_platform::pet::{BODY, PetRect, PetTextRun};

    use super::*;

    fn words(value: &str, size: f64, width: f64, lines: u32) -> PetText {
        PetText {
            rect: PetRect::new(0.0, 0.0, width, 0.0),
            bounds: PetRect::default(),
            value: value.into(),
            runs: vec![PetTextRun {
                text: value.into(),
                color: PetColor { r: 0, g: 0, b: 0, a: 255 },
                bold: false,
                italic: false,
                mono: false,
            }],
            family: BODY.into(),
            size,
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

    /// CoreText measures as SwiftUI drew the Mac's bubble: an 11pt line of SF Pro is 13 high (the Mac's rows), long words wrap and end
    /// in an ellipsis at their line limit, one line too wide is cut to its width, and every character is on a line.
    #[test]
    fn coretext_measures_the_lines_the_mac_drew() {
        let measure = CoreTextMeasure(std::sync::Arc::new(Text::default()));
        let one = measure.lay(&words("Claude", 11.0, WIDE, 1));
        assert_eq!(one.line_chars, [6]);
        assert!((one.height - 13.0).abs() < 0.5, "an 11pt line is 13 high: {}", one.height);
        assert!(one.width > 20.0 && one.width < 50.0, "{}", one.width);
        let long = "word ".repeat(40);
        let wrapped = measure.lay(&words(&long, 11.0, 120.0, 0));
        assert!(wrapped.line_chars.len() > 3 && wrapped.line_chars.iter().sum::<usize>() == long.chars().count());
        assert!(wrapped.width <= 120.0 + 0.01);
        let two = measure.measure(&words(&long, 11.0, 120.0, 2));
        assert!(two.lines == 2 && two.trimmed && two.shown.is_some_and(|s| s.ends_with('\u{2026}')));
        let cut = measure.lay(&words(&long, 11.0, 100.0, 1));
        assert!(cut.trimmed && cut.width <= 100.0 && cut.line_chars.len() == 1);
        let empty = measure.lay(&words("", 11.0, 100.0, 1));
        assert_eq!((empty.line_chars.as_slice(), empty.width), (&[0usize][..], 0.0));
        assert!(empty.height > 10.0);
    }
}
