//! Pictures read back as RGBA, three ways, because a Metal or IOSurface layer may be missing from some of them:
//!
//! - [`window`]: the window server's picture of one window (`CGWindowListCreateImage`, the window's own number): what is really on
//!   screen, native views over the page included, in their z-order;
//! - [`view_cache`]: AppKit drawing the view into a bitmap (`cacheDisplayInRect:toBitmapImageRep:`, what Swift's smokes use); a
//!   Metal layer's presented frame may be absent;
//! - [`layer_surface`]: the view's layer's `contents` when it is an IOSurface (Ghostty's renderer presents through an
//!   `IOSurfaceLayer`): the renderer's own last frame.

use std::ffi::c_void;
use std::path::Path;

use objc2::msg_send;
use objc2::runtime::AnyObject;
use objc2_foundation::NSRect;

/// An RGBA picture, rows top to bottom.
pub struct Rgba {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u8>,
}

impl Rgba {
    /// How many pixels `test(r, g, b)` holds for.
    pub fn count(&self, test: impl Fn(u8, u8, u8) -> bool) -> usize {
        self.pixels.chunks_exact(4).filter(|p| test(p[0], p[1], p[2])).count()
    }

    pub fn save_png(&self, path: &Path) -> Result<(), String> {
        let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), self.width as u32, self.height as u32);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
        writer.write_image_data(&self.pixels).map_err(|e| e.to_string())
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CgPoint {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CgSize {
    width: f64,
    height: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CgRect {
    origin: CgPoint,
    size: CgSize,
}

type CgImage = *mut c_void;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGImageGetWidth(image: CgImage) -> usize;
    fn CGImageGetHeight(image: CgImage) -> usize;
    fn CGImageRelease(image: CgImage);
    fn CGColorSpaceCreateDeviceRGB() -> *mut c_void;
    fn CGColorSpaceRelease(space: *mut c_void);
    fn CGBitmapContextCreate(data: *mut c_void, width: usize, height: usize, bits: usize, row: usize, space: *mut c_void, info: u32) -> *mut c_void;
    fn CGContextDrawImage(context: *mut c_void, rect: CgRect, image: CgImage);
    fn CGContextRelease(context: *mut c_void);
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFGetTypeID(object: *const c_void) -> usize;
}

#[link(name = "IOSurface", kind = "framework")]
unsafe extern "C" {
    fn IOSurfaceGetTypeID() -> usize;
    fn IOSurfaceLock(surface: *const c_void, options: u32, seed: *mut u32) -> i32;
    fn IOSurfaceUnlock(surface: *const c_void, options: u32, seed: *mut u32) -> i32;
    fn IOSurfaceGetBaseAddress(surface: *const c_void) -> *const u8;
    fn IOSurfaceGetBytesPerRow(surface: *const c_void) -> usize;
    fn IOSurfaceGetWidth(surface: *const c_void) -> usize;
    fn IOSurfaceGetHeight(surface: *const c_void) -> usize;
    fn IOSurfaceGetPixelFormat(surface: *const c_void) -> u32;
}

/// A CGImage drawn into an RGBA bitmap of its own size; releases nothing (the caller owns `image`).
unsafe fn rgba(image: CgImage) -> Option<Rgba> {
    if image.is_null() {
        return None;
    }
    // SAFETY: a live CGImage; the bitmap context draws into `pixels`, which outlives it.
    unsafe {
        let (width, height) = (CGImageGetWidth(image), CGImageGetHeight(image));
        if width == 0 || height == 0 {
            return None;
        }
        let mut pixels = vec![0u8; width * height * 4];
        let space = CGColorSpaceCreateDeviceRGB();
        // kCGImageAlphaPremultipliedLast | kCGBitmapByteOrder32Big: R, G, B, A in memory.
        let context = CGBitmapContextCreate(pixels.as_mut_ptr().cast(), width, height, 8, width * 4, space, 1 | (4 << 12));
        CGColorSpaceRelease(space);
        if context.is_null() {
            return None;
        }
        let rect = CgRect { origin: CgPoint { x: 0.0, y: 0.0 }, size: CgSize { width: width as f64, height: height as f64 } };
        CGContextDrawImage(context, rect, image);
        CGContextRelease(context);
        Some(Rgba { width, height, pixels })
    }
}

/// The window server's picture of window `number` (its `windowNumber`), frame and shadow left out.
pub fn window(number: isize) -> Result<Rgba, String> {
    let null = CgRect { origin: CgPoint { x: f64::INFINITY, y: f64::INFINITY }, size: CgSize { width: 0.0, height: 0.0 } };
    // kCGWindowListOptionIncludingWindow = 1 << 3; kCGWindowImageBoundsIgnoreFraming = 1 << 0.
    // Looked up at run time: the macOS 15 SDK marks it obsoleted (it still exists for older binaries), and a missing symbol must not
    // stop the spike from linking.
    type CreateImage = unsafe extern "C" fn(CgRect, u32, u32, u32) -> CgImage;
    // SAFETY: dlsym of a CoreGraphics function by name; plain CoreGraphics calls; the image is released here.
    unsafe {
        let symbol = libc::dlsym(libc::RTLD_DEFAULT, c"CGWindowListCreateImage".as_ptr());
        if symbol.is_null() {
            return Err("CGWindowListCreateImage is not in this system's CoreGraphics".into());
        }
        let create: CreateImage = std::mem::transmute::<*mut c_void, CreateImage>(symbol);
        let image = create(null, 1 << 3, number as u32, 1);
        if image.is_null() {
            return Err("CGWindowListCreateImage gave no image".into());
        }
        let picture = rgba(image);
        CGImageRelease(image);
        picture.ok_or_else(|| "the window image did not draw".into())
    }
}

/// AppKit's drawing of `view` into a bitmap (`cacheDisplayInRect:`). `view` is an NSView; call on the main thread.
///
/// # Safety
/// `view` must be a live NSView.
pub unsafe fn view_cache(view: *mut AnyObject) -> Result<Rgba, String> {
    // SAFETY: the caller's NSView, on the main thread; the bitmap rep is autoreleased and outlives this call's use of it.
    unsafe {
        let bounds: NSRect = msg_send![view, bounds];
        if bounds.size.width <= 0.0 || bounds.size.height <= 0.0 {
            return Err("the view has no size".into());
        }
        let rep: *mut AnyObject = msg_send![view, bitmapImageRepForCachingDisplayInRect: bounds];
        if rep.is_null() {
            return Err("no bitmap rep".into());
        }
        let _: () = msg_send![view, cacheDisplayInRect: bounds, toBitmapImageRep: rep];
        let image: CgImage = msg_send![rep, CGImage];
        rgba(image).ok_or_else(|| "the cached bitmap did not draw".into())
    }
}

/// The class of `view`'s layer, and its contents read back when they are an IOSurface (BGRA, as Ghostty's renderer writes them).
///
/// # Safety
/// `view` must be a live NSView; call on the main thread.
pub unsafe fn layer_surface(view: *mut AnyObject) -> (String, Result<Rgba, String>) {
    // SAFETY: the caller's NSView; its layer and contents are read, the surface locked read-only around the copy.
    unsafe {
        let layer: *mut AnyObject = msg_send![view, layer];
        if layer.is_null() {
            return ("none".into(), Err("the view has no layer".into()));
        }
        let class = (*layer).class().name().to_string_lossy().into_owned();
        let contents: *mut AnyObject = msg_send![layer, contents];
        if contents.is_null() {
            return (class, Err("the layer has no contents".into()));
        }
        if CFGetTypeID(contents.cast()) != IOSurfaceGetTypeID() {
            let kind = (*contents).class().name().to_string_lossy().into_owned();
            return (class, Err(format!("the layer's contents are a {kind}, not an IOSurface")));
        }
        let surface = contents.cast::<c_void>() as *const c_void;
        let mut seed = 0u32;
        if IOSurfaceLock(surface, 1, &mut seed) != 0 {
            return (class, Err("IOSurfaceLock failed".into()));
        }
        let (width, height, row) = (IOSurfaceGetWidth(surface), IOSurfaceGetHeight(surface), IOSurfaceGetBytesPerRow(surface));
        let format = IOSurfaceGetPixelFormat(surface);
        let base = IOSurfaceGetBaseAddress(surface);
        let mut pixels = vec![0u8; width * height * 4];
        if !base.is_null() {
            for y in 0..height {
                let line = std::slice::from_raw_parts(base.add(y * row), width * 4);
                for x in 0..width {
                    // 'BGRA' (0x42475241) as Ghostty allocates it; anything else is copied as it is and named in the report.
                    let (b, g, r, a) = (line[x * 4], line[x * 4 + 1], line[x * 4 + 2], line[x * 4 + 3]);
                    let at = (y * width + x) * 4;
                    pixels[at..at + 4].copy_from_slice(&[r, g, b, a]);
                }
            }
        }
        IOSurfaceUnlock(surface, 1, &mut seed);
        let name = String::from_utf8_lossy(&format.to_be_bytes()).into_owned();
        (format!("{class} (IOSurface {width}x{height} '{name}')"), Ok(Rgba { width, height, pixels }))
    }
}

/// What a check reports of one picture: whether it was taken, how many pixels of each colour, and where it was saved.
pub fn summary(name: &str, out: &Path, picture: &Result<Rgba, String>, colours: &[(&str, fn(u8, u8, u8) -> bool)]) -> serde_json::Value {
    match picture {
        Ok(picture) => {
            let file = out.join(format!("{name}.png"));
            let saved = picture.save_png(&file);
            let mut counts = serde_json::Map::new();
            for (colour, test) in colours {
                counts.insert((*colour).to_string(), serde_json::json!(picture.count(test)));
            }
            serde_json::json!({"ok": true, "width": picture.width, "height": picture.height, "counts": counts,
                "png": file.file_name().map(|n| n.to_string_lossy().into_owned()), "saveError": saved.err()})
        }
        Err(error) => serde_json::json!({"ok": false, "error": error}),
    }
}
