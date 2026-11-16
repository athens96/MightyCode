//! The pet's picture on the Mac: mighty-platform's `PetScene` drawn with CoreGraphics into a premultiplied BGRA bitmap of the window's
//! pixels (Windows draws the same list with Direct2D onto a DIB), the device's RGB space so a fill is the very colour the scene gives
//! (the smoke reads the card's colour back), antialiased, the words with CoreText ([`super::text`]), the sprite cropped from the atlas
//! and scaled with high-quality interpolation, and the icon font's glyphs as the SF Symbols the Swift bubble showed.
//!
//! Drawn on the window's render thread (a bitmap context is used by one thread at a time); the main thread only shows the picture.

use std::collections::HashMap;

use mighty_platform::pet::{BODY, ICONS, PetBitmap, PetColor, PetGlyphSize, PetItem, PetRect, PetScene, PetSegment, SYMBOLS};
use objc2::rc::Retained;
use objc2_app_kit::{NSImage, NSImageSymbolConfiguration};
use objc2_core_foundation::{CFData, CFRetained, CGFloat, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGBitmapContextCreateImage, CGBitmapContextGetData, CGBitmapInfo, CGColorRenderingIntent, CGColorSpace,
    CGContext, CGDataProvider, CGImage, CGInterpolationQuality, CGLineCap, CGLineJoin, CGPath,
};
use objc2_core_text::CTLine;
use objc2_foundation::{NSRect, NSString};

use super::text::{Color, Text};

/// `kCGImageAlphaPremultipliedFirst | kCGBitmapByteOrder32Little`: B, G, R, A in memory, premultiplied (the scene's atlas and the
/// smoke's capture).
pub const BGRA: u32 = 2 | (2 << 12);

/// One drawn frame: its pixels (rows tightly packed) and the image the window shows.
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
    pub image: CFRetained<CGImage>,
}

/// An SF Symbol's template image and its width over its height.
type Mask = (CFRetained<CGImage>, f64);

/// The renderer of one window (its thread's): the atlas's image and the symbols' masks, each made once.
pub struct Renderer {
    text: std::sync::Arc<Text>,
    space: CFRetained<CGColorSpace>,
    atlas: Option<(String, CFRetained<CGImage>)>,
    symbols: HashMap<(String, u64, u64), Option<Mask>>,
}

fn rect(r: &PetRect) -> CGRect {
    CGRect { origin: CGPoint { x: r.x, y: r.y }, size: CGSize { width: r.width, height: r.height } }
}

/// The SF Symbol the Swift bubble shows where the scene names a Segoe icon (`AgentCompanionViews.swift`), or None for a glyph of a
/// text font.
pub fn symbol(family: &str, glyph: &str) -> Option<&'static str> {
    if family == SYMBOLS {
        return (glyph == "\u{270B}").then_some("hand.raised.fill");
    }
    if family != ICONS {
        return None;
    }
    Some(match glyph {
        "\u{E823}" => "clock",
        "\u{E9D5}" => "list.bullet.clipboard",
        "\u{E73D}" => "checkmark.square.fill",
        "\u{E739}" => "square",
        "\u{EC61}" => "checkmark.circle.fill",
        "\u{F136}" => "exclamationmark.circle.fill",
        "\u{E740}" => "arrow.up.left.and.arrow.down.right",
        "\u{E70F}" => "pencil",
        "\u{E8A7}" => "arrow.up.forward.app",
        _ => "circle",
    })
}

impl Renderer {
    pub fn new(text: std::sync::Arc<Text>) -> Option<Renderer> {
        Some(Renderer { text, space: CGColorSpace::new_device_rgb()?, atlas: None, symbols: HashMap::new() })
    }

    /// The scene drawn at its scale into a bitmap of exactly its pixels; the pixels copied out only when `pixels` (a capture wants them,
    /// review item 10).
    pub fn render(&mut self, scene: &PetScene, pixels: bool) -> Option<Frame> {
        let (width, height) = scene.pixel_size();
        let scale = if scene.scale.is_finite() && scene.scale > 0.0 { scene.scale } else { 1.0 };
        let stride = width as usize * 4;
        // SAFETY: a bitmap context CoreGraphics allocates for `width × height` 4-byte pixels in the device's RGB space.
        let context =
            unsafe { CGBitmapContextCreate(std::ptr::null_mut(), width as usize, height as usize, 8, stride, Some(&self.space), BGRA) }?;
        let all = CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: CGSize { width: f64::from(width), height: f64::from(height) } };
        CGContext::clear_rect(Some(&context), all);
        // The scene's space: DIPs down from the top left.
        CGContext::translate_ctm(Some(&context), 0.0, f64::from(height));
        CGContext::scale_ctm(Some(&context), scale, -scale);
        CGContext::set_should_antialias(Some(&context), true);
        CGContext::set_should_smooth_fonts(Some(&context), false);
        CGContext::set_interpolation_quality(Some(&context), CGInterpolationQuality::High);
        if self.atlas.as_ref().map(|a| a.0.as_str()) != scene.atlas.as_ref().map(|a| a.key.as_str()) {
            self.atlas = scene.atlas.as_ref().and_then(|atlas| self.atlas_image(atlas).map(|image| (atlas.key.clone(), image)));
        }
        self.items(&context, &scene.items, scale);
        let bgra = if pixels {
            let data = CGBitmapContextGetData(Some(&context)) as *const u8;
            if data.is_null() {
                return None;
            }
            // SAFETY: the context's own buffer of `stride × height` bytes, read after its drawing is done.
            unsafe { std::slice::from_raw_parts(data, stride * height as usize) }.to_vec()
        } else {
            Vec::new()
        };
        let image = CGBitmapContextCreateImage(Some(&context))?;
        Some(Frame { width, height, bgra, image })
    }

    fn atlas_image(&self, atlas: &PetBitmap) -> Option<CFRetained<CGImage>> {
        let stride = atlas.width as usize * 4;
        if atlas.bgra.len() != stride.checked_mul(atlas.height as usize)? {
            return None;
        }
        let data = CFData::from_bytes(&atlas.bgra);
        let provider = CGDataProvider::with_cf_data(Some(&data))?;
        // SAFETY: an image over the copied premultiplied BGRA rows, its size and stride checked against the bytes above.
        unsafe {
            CGImage::new(
                atlas.width as usize,
                atlas.height as usize,
                8,
                32,
                stride,
                Some(&self.space),
                CGBitmapInfo(BGRA),
                Some(&provider),
                std::ptr::null(),
                true,
                CGColorRenderingIntent::RenderingIntentDefault,
            )
        }
    }

    fn items(&mut self, context: &CGContext, items: &[PetItem], scale: f64) {
        for item in items {
            self.item(context, item, scale);
        }
    }

    fn fill_color(&self, context: &CGContext, color: PetColor) -> bool {
        match Color::new(color) {
            Some(color) => {
                CGContext::set_fill_color_with_color(Some(context), Some(&color.0));
                true
            }
            None => false,
        }
    }

    fn stroke_color(&self, context: &CGContext, color: PetColor) -> bool {
        match Color::new(color) {
            Some(color) => {
                CGContext::set_stroke_color_with_color(Some(context), Some(&color.0));
                true
            }
            None => false,
        }
    }

    fn rounded(context: &CGContext, r: &PetRect, radius: f64) {
        let radius = radius.clamp(0.0, (r.width / 2.0).min(r.height / 2.0).max(0.0));
        if radius <= 0.0 {
            CGContext::add_rect(Some(context), rect(r));
            return;
        }
        // SAFETY: a path of a rectangle with radii no larger than half its sides (clamped above), no transform.
        let path = unsafe { CGPath::with_rounded_rect(rect(r), radius, radius, std::ptr::null()) };
        CGContext::add_path(Some(context), Some(&path));
    }

    fn item(&mut self, context: &CGContext, item: &PetItem, scale: f64) {
        let c = Some(context);
        match item {
            PetItem::Fill { rect: r, radius, color } => {
                if self.fill_color(context, *color) {
                    CGContext::begin_path(c);
                    Self::rounded(context, r, *radius);
                    CGContext::fill_path(c);
                }
            }
            PetItem::Stroke { rect: r, radius, color, width } => {
                if self.stroke_color(context, *color) {
                    CGContext::set_line_width(c, *width);
                    CGContext::begin_path(c);
                    Self::rounded(context, r, *radius);
                    CGContext::stroke_path(c);
                }
            }
            PetItem::Ellipse { center, radius_x, radius_y, color, stroke } => {
                let r = CGRect {
                    origin: CGPoint { x: center.x - radius_x, y: center.y - radius_y },
                    size: CGSize { width: radius_x * 2.0, height: radius_y * 2.0 },
                };
                match stroke {
                    Some(width) if self.stroke_color(context, *color) => {
                        CGContext::set_line_width(c, *width);
                        CGContext::stroke_ellipse_in_rect(c, r);
                    }
                    None if self.fill_color(context, *color) => CGContext::fill_ellipse_in_rect(c, r),
                    _ => {}
                }
            }
            PetItem::Line { from, to, color, width, round } => {
                if self.stroke_color(context, *color) {
                    CGContext::set_line_width(c, *width);
                    CGContext::set_line_cap(c, if *round { CGLineCap::Round } else { CGLineCap::Butt });
                    CGContext::begin_path(c);
                    CGContext::move_to_point(c, from.x, from.y);
                    CGContext::add_line_to_point(c, to.x, to.y);
                    CGContext::stroke_path(c);
                    CGContext::set_line_cap(c, CGLineCap::Butt);
                }
            }
            PetItem::Arc { center, radius, start, sweep, color, width } => {
                if self.stroke_color(context, *color) {
                    CGContext::set_line_width(c, *width);
                    CGContext::set_line_cap(c, CGLineCap::Round);
                    CGContext::begin_path(c);
                    // The space runs down: a growing angle turns clockwise on screen, as the scene's sweep does.
                    CGContext::add_arc(c, center.x, center.y, *radius, *start, start + sweep, i32::from(*sweep < 0.0));
                    CGContext::stroke_path(c);
                    CGContext::set_line_cap(c, CGLineCap::Butt);
                }
            }
            PetItem::Path { start, segments, color } => {
                if self.fill_color(context, *color) {
                    CGContext::begin_path(c);
                    CGContext::move_to_point(c, start.x, start.y);
                    for segment in segments {
                        match segment {
                            PetSegment::Line { to } => CGContext::add_line_to_point(c, to.x, to.y),
                            PetSegment::Cubic { first, second, to } => {
                                CGContext::add_curve_to_point(c, first.x, first.y, second.x, second.y, to.x, to.y)
                            }
                        }
                    }
                    CGContext::close_path(c);
                    CGContext::fill_path(c);
                }
            }
            PetItem::Text(text) => self.text.draw(context, text),
            PetItem::Glyph { family, glyph, center, size, weight, color, over } => {
                match symbol(family, glyph) {
                    // The SF Symbol is whole (the error mark's exclamation is cut out of it): `over` has nothing left to draw.
                    Some(name) => self.symbol(context, name, *center, *size, *weight, *color, scale),
                    None => {
                        self.glyph(context, family, glyph, *center, *size, *weight, *color);
                        if let Some((second, ink)) = over {
                            self.glyph(context, family, second, *center, *size, *weight, *ink);
                        }
                    }
                }
            }
            PetItem::Sprite { cell, dest, opacity } => {
                let Some((_, atlas)) = &self.atlas else { return };
                let Some(cropped) = CGImage::with_image_in_rect(Some(atlas), rect(cell)) else { return };
                CGContext::save_g_state(c);
                CGContext::set_alpha(c, opacity.clamp(0.0, 1.0));
                // An image draws with its first row at the bottom of its rectangle: drawn unflipped about its own place.
                CGContext::translate_ctm(c, dest.x, dest.y + dest.height);
                CGContext::scale_ctm(c, 1.0, -1.0);
                let place = CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: CGSize { width: dest.width, height: dest.height } };
                CGContext::draw_image(c, place, Some(&cropped));
                CGContext::restore_g_state(c);
            }
            PetItem::Clip { rect: r, items } => {
                CGContext::save_g_state(c);
                CGContext::clip_to_rect(c, rect(r));
                self.items(context, items, scale);
                CGContext::restore_g_state(c);
            }
            PetItem::Group { opacity, items, .. } => {
                if *opacity >= 1.0 {
                    self.items(context, items, scale);
                    return;
                }
                CGContext::save_g_state(c);
                CGContext::set_alpha(c, opacity.clamp(0.0, 1.0));
                // SAFETY: a layer with no auxiliary info, ended below on the same context.
                unsafe { CGContext::begin_transparency_layer(c, None) };
                self.items(context, items, scale);
                CGContext::end_transparency_layer(c);
                CGContext::restore_g_state(c);
            }
        }
        CGContext::set_line_join(c, CGLineJoin::Miter);
    }

    /// A glyph of a text font centred on `center` by its ink (C# `Icon`): at its font size, or at the size that inks it `tall` high.
    #[allow(clippy::too_many_arguments, reason = "the scene's Glyph fields")]
    fn glyph(
        &self,
        context: &CGContext,
        family: &str,
        glyph: &str,
        center: mighty_platform::pet::PetPoint,
        size: PetGlyphSize,
        weight: u16,
        color: PetColor,
    ) {
        let mono = family == mighty_platform::pet::MONO;
        let line_at = |size: f64| {
            let text = mighty_platform::pet::PetText {
                rect: PetRect::default(),
                bounds: PetRect::default(),
                value: glyph.into(),
                runs: vec![mighty_platform::pet::PetTextRun { text: glyph.into(), color, bold: false, italic: false, mono: false }],
                family: if mono { family.into() } else { BODY.into() },
                size,
                weight,
                asked: weight,
                max_lines: 1,
                lines: 1,
                trimmed: false,
                align: mighty_platform::pet::PetTextAlign::Leading,
                tabular: false,
                ink: String::new(),
            };
            let (string, _) = self.text.attributed(&text);
            // SAFETY: a line of the attributed string made above; its ink box read without a context.
            unsafe {
                let line = CTLine::with_attributed_string(&string);
                let ink = line.image_bounds(None);
                (line, ink)
            }
        };
        let (line, ink) = match size {
            PetGlyphSize::Font(size) => line_at(size),
            PetGlyphSize::Ink(tall) => {
                let (_, probe) = line_at(32.0);
                if probe.size.height <= 0.0 {
                    return;
                }
                line_at(32.0 * tall / probe.size.height)
            }
        };
        // The ink box runs up from the baseline; the space here runs down.
        let x = center.x - (ink.origin.x + ink.size.width / 2.0);
        let baseline = center.y + ink.origin.y + ink.size.height / 2.0;
        let flip = objc2_core_foundation::CGAffineTransform { a: 1.0, b: 0.0, c: 0.0, d: -1.0, tx: 0.0, ty: 0.0 };
        CGContext::set_text_matrix(Some(context), flip);
        CGContext::set_text_position(Some(context), x, baseline);
        // SAFETY: a live line drawn on a live context.
        unsafe { line.draw(context) };
    }

    /// An SF Symbol `tall` high (or at a font size), centred on `center`, in one colour: its template image as a mask filled with it.
    #[allow(clippy::too_many_arguments, reason = "the scene's Glyph fields and the scale")]
    fn symbol(
        &mut self,
        context: &CGContext,
        name: &str,
        center: mighty_platform::pet::PetPoint,
        size: PetGlyphSize,
        weight: u16,
        color: PetColor,
        scale: f64,
    ) {
        let tall = match size {
            PetGlyphSize::Ink(tall) | PetGlyphSize::Font(tall) => tall,
        };
        let key = (name.to_string(), tall.to_bits(), (scale * 1000.0) as u64 + u64::from(weight) * 1_000_000);
        let text = self.text.clone();
        let name_owned = name.to_string();
        // NSImage's symbols are made on the main thread (review item 12); the CGImage they give is used here.
        let made = self
            .symbols
            .entry(key)
            .or_insert_with(|| text.on_main(move || mask(&name_owned, tall, weight, scale).map(Sendable)).flatten().map(|m| m.0))
            .as_ref();
        let Some((image, aspect)) = made else { return };
        let (height, width) = (tall, tall * aspect);
        let c = Some(context);
        CGContext::save_g_state(c);
        CGContext::translate_ctm(c, center.x - width / 2.0, center.y + height / 2.0);
        CGContext::scale_ctm(c, 1.0, -1.0);
        let place = CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: CGSize { width, height } };
        CGContext::clip_to_mask(c, place, Some(image));
        if self.fill_color(context, color) {
            CGContext::fill_rect(c, place);
        }
        CGContext::restore_g_state(c);
    }
}

/// A CGImage handed from the main thread (immutable, and CoreGraphics' images may be used from any thread).
struct Sendable(Mask);
// SAFETY: as above; the f64 is plain data.
unsafe impl Send for Sendable {}

/// The symbol's template image at `tall` points, rendered at `scale` pixels a point, and its width over its height.
fn mask(name: &str, tall: f64, weight: u16, scale: f64) -> Option<Mask> {
    let weight: CGFloat = match weight {
        0..=450 => 0.0,
        451..=550 => 0.23,
        551..=650 => 0.3,
        651..=750 => 0.4,
        _ => 0.56,
    };
    let image: Retained<NSImage> = NSImage::imageWithSystemSymbolName_accessibilityDescription(&NSString::from_str(name), None)?;
    let configured = image.imageWithSymbolConfiguration(&NSImageSymbolConfiguration::configurationWithPointSize_weight(tall, weight))?;
    let size = configured.size();
    if size.height <= 0.0 || size.width <= 0.0 {
        return None;
    }
    let aspect = size.width / size.height;
    let mut proposed =
        NSRect::new(objc2_foundation::NSPoint::new(0.0, 0.0), objc2_foundation::NSSize::new(tall * aspect * scale, tall * scale));
    // SAFETY: a proposed rectangle that lives across the call; no context and no hints (the image renders at the rectangle's size).
    let cg = unsafe { configured.CGImageForProposedRect_context_hints(&mut proposed, None, None) }?;
    // SAFETY: the CGImage NSImage returned, retained here as the CF type it is.
    let image = unsafe { CFRetained::retain(std::ptr::NonNull::new_unchecked(Retained::as_ptr(&cg) as *mut CGImage)) };
    Some((image, aspect))
}

#[cfg(test)]
mod tests {
    use mighty_platform::pet::PetPoint;

    use super::*;

    fn at(frame: &Frame, x: u32, y: u32) -> [u8; 4] {
        let i = (y * frame.width + x) as usize * 4;
        [frame.bgra[i], frame.bgra[i + 1], frame.bgra[i + 2], frame.bgra[i + 3]]
    }

    /// The bitmap is the scene's, pixel for pixel where the smoke reads it: a fill is exactly its colour (no colour matching), a
    /// rounded corner is clear, a hairline half a pixel in is the card mixed with the ink, nothing bleeds past a rectangle's edge, and the
    /// scale makes the pixels.
    #[test]
    fn the_scene_draws_its_colours_and_edges() {
        let card = PetColor { r: 0x2a, g: 0x2d, b: 0x33, a: 255 };
        let ink = PetColor { r: 0xff, g: 0xff, b: 0xff, a: 26 };
        let bubble = PetRect::new(12.0, 10.0, 100.0, 60.0);
        let scene = PetScene {
            width: 124.0,
            height: 80.0,
            scale: 2.0,
            items: vec![
                PetItem::Fill { rect: bubble, radius: 18.0, color: card },
                PetItem::Stroke {
                    rect: PetRect::new(bubble.x + 0.5, bubble.y + 0.5, bubble.width - 1.0, bubble.height - 1.0),
                    radius: 17.5,
                    color: ink,
                    width: 1.0,
                },
                PetItem::Glyph {
                    family: ICONS.into(),
                    glyph: "\u{EC61}".into(),
                    center: PetPoint { x: 60.0, y: 40.0 },
                    size: PetGlyphSize::Ink(12.0),
                    weight: 400,
                    color: PetColor { r: 0, g: 200, b: 0, a: 255 },
                    over: None,
                },
            ],
            ..PetScene::default()
        };
        let mut renderer = Renderer::new(std::sync::Arc::new(Text::default())).unwrap();
        let frame = renderer.render(&scene, true).unwrap();
        assert_eq!((frame.width, frame.height), (248, 160));
        assert!(renderer.render(&scene, false).unwrap().bgra.is_empty(), "no pixels copied unless asked");
        assert_eq!(at(&frame, 2 * 17, 2 * 40), [card.b, card.g, card.r, 255]);
        assert_eq!(at(&frame, 2 * 13, 2 * 11)[3], 0, "a corner of radius 18 is clear");
        assert_eq!(at(&frame, 2 * 60, 2 * 72)[3], 0, "nothing below the bubble");
        assert_eq!(at(&frame, 2 * 60, 2 * 10 + 1)[3], 255, "the hairline's row is opaque");
        let green = (0..frame.height).flat_map(|y| (0..frame.width).map(move |x| (x, y))).filter(|&(x, y)| {
            let p = at(&frame, x, y);
            p[1] > 150 && p[2] < 80
        });
        assert!(green.count() > 20, "the SF Symbol drew in its colour");
    }

    /// Swift `PetSpriteFramesTests.swift:46` "slicesEachCellIntoItsOwnFrame": each 192×208 cell of the atlas drawn alone, in its own
    /// colour, its top-left marker at the top left (no vertical flip), nothing of the next cell; drawn at the size it is.
    #[test]
    fn slices_each_cell_into_its_own_frame() {
        let (cell_w, cell_h) = (192u32, 208u32);
        let colours = [[(255u8, 0u8, 0u8), (0, 255, 0), (0, 0, 255)], [(255, 255, 0), (0, 255, 255), (0, 0, 0)]];
        let marker = (255u8, 0u8, 255u8);
        let (width, height) = (cell_w * 3, cell_h * 2);
        let mut bgra = vec![0u8; (width * height * 4) as usize];
        for y in 0..height {
            for x in 0..width {
                let (row, column) = ((y / cell_h) as usize, (x / cell_w) as usize);
                let (lx, ly) = (x % cell_w, y % cell_h);
                let (r, g, b) = if lx < 6 && ly < 6 { marker } else { colours[row][column] };
                let i = ((y * width + x) * 4) as usize;
                bgra[i..i + 4].copy_from_slice(&[b, g, r, 255]);
            }
        }
        let atlas = PetBitmap { key: "test".into(), width, height, bgra: std::sync::Arc::new(bgra) };
        let mut renderer = Renderer::new(std::sync::Arc::new(Text::default())).unwrap();
        for (row, cells) in colours.iter().enumerate() {
            for (column, (r, g, b)) in cells.iter().enumerate() {
                let cell =
                    PetRect::new(f64::from(column as u32 * cell_w), f64::from(row as u32 * cell_h), f64::from(cell_w), f64::from(cell_h));
                let scene = PetScene {
                    width: f64::from(cell_w),
                    height: f64::from(cell_h),
                    scale: 1.0,
                    items: vec![PetItem::Sprite { cell, dest: PetRect::new(0.0, 0.0, f64::from(cell_w), f64::from(cell_h)), opacity: 1.0 }],
                    atlas: Some(atlas.clone()),
                    ..PetScene::default()
                };
                let frame = renderer.render(&scene, true).unwrap();
                assert_eq!((frame.width, frame.height), (cell_w, cell_h));
                assert_eq!(at(&frame, 100, 100), [*b, *g, *r, 255], "row {row} column {column}");
                let mark = at(&frame, 2, 3);
                assert_eq!((mark[2], mark[1], mark[0]), marker, "the marker at the top left in row {row} column {column}");
                let mirrored = at(&frame, 2, cell_h - 1 - 3);
                assert_eq!((mirrored[2], mirrored[1], mirrored[0]), (*r, *g, *b), "no vertical flip in row {row} column {column}");
                let edge = at(&frame, cell_w - 1, cell_h / 2);
                assert_eq!((edge[2], edge[1], edge[0]), (*r, *g, *b), "nothing of the next cell in row {row} column {column}");
            }
        }
    }

    #[test]
    fn the_segoe_icons_read_as_sf_symbols() {
        assert_eq!(symbol(ICONS, "\u{E823}"), Some("clock"));
        assert_eq!(symbol(SYMBOLS, "\u{270B}"), Some("hand.raised.fill"));
        assert_eq!(symbol(BODY, "?"), None);
    }
}
