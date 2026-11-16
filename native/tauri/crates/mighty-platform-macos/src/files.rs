//! M-G: the files pane's ports on the Mac (plan §1.7; contract `mighty_platform::files`), where Windows has
//! `GetFinalPathNameByHandleW`, `Windows.Data.Pdf` and the shell's file actions:
//! - [`MacFileHandles`]: an open file's path from `fcntl(F_GETPATH)` (what the file system calls it: `/var/…` is `/private/var/…`),
//!   a folder's through an `O_EVTONLY` handle, `single_link` when `fstat` gives `st_nlink == 1 && S_IFREG`; `asks_handles` true.
//! - [`MacPdf`]: the first page through `CGPDFDocument`, drawn into a bitmap at the size asked for and sized in DIPs (96 to the
//!   inch) as `Windows.Data.Pdf` reports it, so the shared sizing is the same on both.
//! - [`MacDesktop`]: show in Finder (`activateFileViewerSelectingURLs`, Swift `FilePaneView`), open with the default app (`NSWorkspace
//!   openURL`) refusing an app bundle, an executable file and the types that run what they hold, as Windows refuses programs; a
//!   picture on the pasteboard as PNG and TIFF.
//! - [`open_url`] and [`show_in_explorer`] (Swift `selectFile(nil, inFileViewerRootedAtPath:)`).

use std::ffi::{OsStr, c_void};
use std::fs::File;
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::PermissionsExt as _;
use std::os::unix::io::AsRawFd as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mighty_platform::{ClipboardImage, Desktop, FileHandles, FilePorts, PdfPage, PdfRaster, PlatformError, Result};
use objc2::AllocAnyThread as _;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSBitmapImageFileType, NSBitmapImageRep, NSDeviceRGBColorSpace, NSImage, NSPasteboard, NSPasteboardTypePNG, NSPasteboardTypeTIFF,
    NSWorkspace,
};
use objc2_foundation::{NSArray, NSData, NSDictionary, NSString, NSURL};

/// The files pane's ports on the Mac.
pub fn file_ports() -> FilePorts {
    FilePorts { handles: Arc::new(MacFileHandles), pdf: Arc::new(MacPdf), desktop: Arc::new(MacDesktop) }
}

/// What `fcntl(F_GETPATH)` says the descriptor is: the path the file system has for it (links resolved, `/var` → `/private/var`).
fn descriptor_path(fd: i32) -> Option<PathBuf> {
    let mut buffer = vec![0u8; libc::PATH_MAX as usize + 1];
    // SAFETY: an open descriptor and a buffer of at least MAXPATHLEN bytes, as F_GETPATH requires.
    if unsafe { libc::fcntl(fd, libc::F_GETPATH, buffer.as_mut_ptr()) } == -1 {
        return None;
    }
    let length = buffer.iter().position(|&b| b == 0)?;
    (length > 0).then(|| PathBuf::from(OsStr::from_bytes(&buffer[..length])))
}

fn file_status(file: &File) -> Option<libc::stat> {
    // SAFETY: an all-zero stat is a valid out-structure; the file's own open descriptor.
    let mut status: libc::stat = unsafe { std::mem::zeroed() };
    (unsafe { libc::fstat(file.as_raw_fd(), &mut status) } == 0).then_some(status)
}

/// The handle checks of `WorkspaceFiles.OpenFile`, the Mac's way.
#[derive(Debug, Clone, Copy, Default)]
pub struct MacFileHandles;

impl FileHandles for MacFileHandles {
    fn asks_handles(&self) -> bool {
        true
    }

    /// A regular file or a folder on a volume, not a device, pipe or socket a name opened.
    fn is_disk(&self, file: &File) -> bool {
        file_status(file).is_some_and(|s| matches!(s.st_mode & libc::S_IFMT, libc::S_IFREG | libc::S_IFDIR))
    }

    fn final_path(&self, file: &File) -> Option<PathBuf> {
        descriptor_path(file.as_raw_fd())
    }

    /// The folder (or file) opened for events only (`O_EVTONLY`: no read access asked, as Windows opens it with none), never blocking
    /// on a pipe, and its descriptor asked.
    fn final_directory_path(&self, path: &Path) -> Option<PathBuf> {
        let name = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
        // SAFETY: a NUL-terminated path; the descriptor is closed below.
        let fd = unsafe { libc::open(name.as_ptr(), libc::O_EVTONLY | libc::O_NONBLOCK | libc::O_CLOEXEC) };
        if fd < 0 {
            return None;
        }
        let found = descriptor_path(fd);
        // SAFETY: the descriptor opened above, closed once.
        unsafe { libc::close(fd) };
        found
    }

    /// C# `StyleFiles.SingleLink` on the Mac: `st_nlink == 1` and `S_IFREG`.
    fn single_link(&self, file: &File) -> bool {
        file_status(file).is_some_and(|s| s.st_nlink == 1 && s.st_mode & libc::S_IFMT == libc::S_IFREG)
    }
}

// ---- the PDF's first page ------------------------------------------------------------------------------------------------------

/// CoreGraphics' C functions the page drawing needs (the framework is linked by core-graphics).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct Rect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct Transform {
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    tx: f64,
    ty: f64,
}

type Ref = *mut c_void;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGDataProviderCreateWithData(info: *mut c_void, data: *const c_void, size: usize, release: Option<extern "C" fn()>) -> Ref;
    fn CGDataProviderRelease(provider: Ref);
    fn CGPDFDocumentCreateWithProvider(provider: Ref) -> Ref;
    fn CGPDFDocumentRelease(document: Ref);
    fn CGPDFDocumentGetNumberOfPages(document: Ref) -> usize;
    fn CGPDFDocumentGetPage(document: Ref, number: usize) -> Ref;
    fn CGPDFPageGetBoxRect(page: Ref, kind: i32) -> Rect;
    fn CGPDFPageGetRotationAngle(page: Ref) -> i32;
    fn CGColorSpaceCreateDeviceRGB() -> Ref;
    fn CGColorSpaceRelease(space: Ref);
    fn CGBitmapContextCreate(
        data: *mut c_void,
        width: usize,
        height: usize,
        bits_per_component: usize,
        bytes_per_row: usize,
        space: Ref,
        info: u32,
    ) -> Ref;
    fn CGContextRelease(context: Ref);
    fn CGContextSetRGBFillColor(context: Ref, red: f64, green: f64, blue: f64, alpha: f64);
    fn CGContextFillRect(context: Ref, rect: Rect);
    fn CGContextScaleCTM(context: Ref, sx: f64, sy: f64);
    fn CGContextConcatCTM(context: Ref, transform: Transform);
    fn CGContextTranslateCTM(context: Ref, tx: f64, ty: f64);
    fn CGContextDrawPDFPage(context: Ref, page: Ref);
}

/// `kCGPDFCropBox`: the page as a viewer shows it.
const CROP_BOX: i32 = 1;
/// `kCGImageAlphaPremultipliedLast | kCGBitmapByteOrder32Big`: RGBA bytes in memory order.
const RGBA: u32 = 1 | (4 << 12);
/// Windows.Data.Pdf measures a page in DIPs (96 to the inch); a PDF's are points (72 to the inch).
const DIPS_PER_POINT: f64 = 96.0 / 72.0;

/// A page's crop box and its rotation as shown: the size in points (turned when the page is) and the drawing's turn, a multiple of 90.
fn page_turn(rotation: i32, box_width: f64, box_height: f64) -> (i32, f64, f64) {
    let turn = rotation.rem_euclid(360) / 90 * 90;
    if turn % 180 == 0 { (turn, box_width, box_height) } else { (turn, box_height, box_width) }
}

/// The transform that turns a page drawn at the origin (`width` × `height` points, y up) clockwise by `turn` degrees, as a viewer
/// shows a page with `/Rotate`, keeping it at the origin.
fn turned(turn: i32, width: f64, height: f64) -> Transform {
    match turn {
        90 => Transform { a: 0.0, b: -1.0, c: 1.0, d: 0.0, tx: 0.0, ty: width },
        180 => Transform { a: -1.0, b: 0.0, c: 0.0, d: -1.0, tx: width, ty: height },
        270 => Transform { a: 0.0, b: 1.0, c: -1.0, d: 0.0, tx: height, ty: 0.0 },
        _ => Transform { a: 1.0, b: 0.0, c: 0.0, d: 1.0, tx: 0.0, ty: 0.0 },
    }
}

/// RGBA pixels (rows from the top, alpha 255) as PNG, through `NSBitmapImageRep`.
fn png_of(rgba: &[u8], width: usize, height: usize) -> Option<Vec<u8>> {
    // SAFETY: planes null makes the rep allocate its own buffer of the size described; it is filled below within its length.
    let rep = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            std::ptr::null_mut(),
            width as isize,
            height as isize,
            8,
            4,
            true,
            false,
            NSDeviceRGBColorSpace,
            (width * 4) as isize,
            32,
        )
    }?;
    let target = rep.bitmapData();
    if target.is_null() {
        return None;
    }
    // SAFETY: the rep's own buffer holds width × height × 4 bytes (its row length as given), as does `rgba`.
    unsafe { std::ptr::copy_nonoverlapping(rgba.as_ptr(), target, width * height * 4) };
    let properties = NSDictionary::new();
    // SAFETY: an empty property dictionary of the right key type.
    let data = unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &properties) }?;
    Some(data.to_vec())
}

/// A PDF's first page drawn by CoreGraphics.
#[derive(Debug, Clone, Copy, Default)]
pub struct MacPdf;

impl PdfRaster for MacPdf {
    fn first_page(&self, data: &[u8], size: &(dyn Fn(f64, f64) -> Option<(u32, u32)> + Sync)) -> Result<Option<PdfPage>> {
        let refused = || PlatformError::Refused("CGPDFDocument: the document does not load".into());
        // SAFETY: the provider reads `data`, which outlives the document and the provider (both released below).
        let provider = unsafe { CGDataProviderCreateWithData(std::ptr::null_mut(), data.as_ptr().cast(), data.len(), None) };
        if provider.is_null() {
            return Err(refused());
        }
        let document = unsafe { CGPDFDocumentCreateWithProvider(provider) };
        let release = || unsafe {
            if !document.is_null() {
                CGPDFDocumentRelease(document);
            }
            CGDataProviderRelease(provider);
        };
        if document.is_null() {
            release();
            return Err(refused());
        }
        // SAFETY: a document CoreGraphics opened; its pages belong to it.
        let page = if unsafe { CGPDFDocumentGetNumberOfPages(document) } == 0 {
            std::ptr::null_mut()
        } else {
            unsafe { CGPDFDocumentGetPage(document, 1) }
        };
        if page.is_null() {
            release();
            return Ok(None);
        }
        let crop = unsafe { CGPDFPageGetBoxRect(page, CROP_BOX) };
        let (turn, points_wide, points_high) = page_turn(unsafe { CGPDFPageGetRotationAngle(page) }, crop.width, crop.height);
        let (width, height) = (points_wide * DIPS_PER_POINT, points_high * DIPS_PER_POINT);
        let png = match size(width, height) {
            Some((pixels_wide, pixels_high)) if pixels_wide > 0 && pixels_high > 0 && points_wide > 0.0 && points_high > 0.0 => {
                let (w, h) = (pixels_wide as usize, pixels_high as usize);
                let mut pixels = vec![0u8; w * h * 4];
                // SAFETY: a bitmap context over `pixels` (w × h RGBA, its row length as given), released before `pixels` is read.
                unsafe {
                    let space = CGColorSpaceCreateDeviceRGB();
                    let context = CGBitmapContextCreate(pixels.as_mut_ptr().cast(), w, h, 8, w * 4, space, RGBA);
                    CGColorSpaceRelease(space);
                    if context.is_null() {
                        release();
                        return Err(PlatformError::Refused("CGBitmapContextCreate failed".into()));
                    }
                    // White under the page, as Windows.Data.Pdf draws it.
                    CGContextSetRGBFillColor(context, 1.0, 1.0, 1.0, 1.0);
                    CGContextFillRect(context, Rect { x: 0.0, y: 0.0, width: w as f64, height: h as f64 });
                    CGContextScaleCTM(context, w as f64 / points_wide, h as f64 / points_high);
                    CGContextConcatCTM(context, turned(turn, crop.width, crop.height));
                    CGContextTranslateCTM(context, -crop.x, -crop.y);
                    CGContextDrawPDFPage(context, page);
                    CGContextRelease(context);
                }
                png_of(&pixels, w, h)
            }
            _ => None,
        };
        release();
        Ok(Some(PdfPage { width, height, png }))
    }
}

// ---- the desktop ----------------------------------------------------------------------------------------------------------------

/// File types that run what they hold when opened, whatever app opens them: scripts Terminal runs, installers, Automator actions and
/// workflows, Java archives, system plug-ins that install themselves, and the location files that open whatever they name (an app
/// among them). An app bundle is a folder, refused before this.
const CODE_TYPES: [&str; 31] = [
    "app",
    "command",
    "tool",
    "terminal",
    "sh",
    "bash",
    "zsh",
    "csh",
    "ksh",
    "tcsh",
    "fish",
    "pkg",
    "mpkg",
    "workflow",
    "action",
    "jar",
    "jnlp",
    "prefpane",
    "saver",
    "qlgenerator",
    "mdimporter",
    "kext",
    "plugin",
    "bundle",
    "osax",
    "dylib",
    "webloc",
    "inetloc",
    "fileloc",
    "afploc",
    "url",
];

/// The apps that run what they are handed (a shell, an installer, a launcher), by their bundle's name without `.app`, in lower case:
/// a file whose default app is one of these is never opened, whatever its type.
const LAUNCHERS: [&str; 12] = [
    "terminal",
    "iterm",
    "iterm2",
    "installer",
    "jar launcher",
    "python launcher",
    "automator installer",
    "automator application stub",
    "script menu",
    "system preferences",
    "system settings",
    "wish",
];

/// Whether opening a file of type `extension` (no dot) with the app `handler` (its bundle's file name, `Terminal.app`) would run code.
pub fn opens_as_code(extension: &str, handler: Option<&str>) -> bool {
    CODE_TYPES.iter().any(|t| t.eq_ignore_ascii_case(extension))
        || handler.is_some_and(|name| {
            let name = name.to_lowercase();
            LAUNCHERS.contains(&name.strip_suffix(".app").unwrap_or(&name))
        })
}

fn file_url(path: &Path) -> Retained<NSURL> {
    NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()))
}

/// Whether opening the regular file at `path` with its default app could run code: an executable file (any execute bit; Terminal
/// runs it), a type in [`CODE_TYPES`], or one whose default app is in [`LAUNCHERS`].
pub fn runs_code(path: &Path, mode: u32) -> bool {
    if mode & 0o111 != 0 {
        return true;
    }
    let extension = path.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
    let handler = NSWorkspace::sharedWorkspace().URLForApplicationToOpenURL(&file_url(path));
    let name = handler.and_then(|url| url.lastPathComponent()).map(|name| name.to_string());
    opens_as_code(&extension, name.as_deref())
}

/// The pasteboard gets the picture as PNG and as TIFF (what Mac apps paste), replacing what it held.
fn put_image(pasteboard: &NSPasteboard, image: &ClipboardImage) -> Result<()> {
    let png = NSData::with_bytes(&image.png);
    let tiff = NSImage::initWithData(NSImage::alloc(), &png).and_then(|picture| picture.TIFFRepresentation());
    let Some(tiff) = tiff else { return Err(PlatformError::Refused("the picture does not read as PNG".into())) };
    pasteboard.clearContents();
    // SAFETY: the pasteboard types are AppKit's own constants.
    let (put_png, put_tiff) = unsafe {
        (pasteboard.setData_forType(Some(&png), NSPasteboardTypePNG), pasteboard.setData_forType(Some(&tiff), NSPasteboardTypeTIFF))
    };
    if put_png && put_tiff { Ok(()) } else { Err(PlatformError::Refused("the pasteboard refused the picture".into())) }
}

/// Finder's and LaunchServices' file actions.
#[derive(Debug, Clone, Copy, Default)]
pub struct MacDesktop;

impl Desktop for MacDesktop {
    /// The file selected in a Finder window (Swift `activateFileViewerSelecting([url])`).
    fn reveal(&self, path: &Path) -> Result<()> {
        std::fs::symlink_metadata(path)?;
        NSWorkspace::sharedWorkspace().activateFileViewerSelectingURLs(&NSArray::from_retained_slice(&[file_url(path)]));
        Ok(())
    }

    /// The default app through `NSWorkspace openURL`; a folder (an app bundle among them) or a file that can run code is refused first.
    fn open(&self, path: &Path) -> Result<bool> {
        let Ok(meta) = std::fs::metadata(path) else { return Ok(false) };
        if !meta.is_file() || runs_code(path, meta.permissions().mode()) {
            return Ok(false);
        }
        Ok(NSWorkspace::sharedWorkspace().openURL(&file_url(path)))
    }

    /// The general pasteboard gets the picture as PNG and TIFF; it keeps them after the app quits.
    fn copy_image(&self, image: &ClipboardImage) -> Result<()> {
        put_image(&NSPasteboard::generalPasteboard(), image)
    }
}

/// An address handed to its default handler (`NSWorkspace openURL:`, C# `Process.Start` with `UseShellExecute`). A `file:` address is
/// refused: a file goes through [`MacDesktop::open`], which refuses what runs code.
pub fn open_url(url: &str) -> std::result::Result<(), String> {
    let Some(address) = NSURL::URLWithString(&NSString::from_str(url)) else { return Err("not an address".into()) };
    if address.scheme().is_none_or(|scheme| scheme.to_string().eq_ignore_ascii_case("file")) {
        return Err("not an address the default handler may open".into());
    }
    if NSWorkspace::sharedWorkspace().openURL(&address) { Ok(()) } else { Err("no app opens the address".into()) }
}

/// A folder shown in a Finder window of its own (Swift `selectFile(nil, inFileViewerRootedAtPath:)`).
pub fn show_in_explorer(folder: &Path) -> std::result::Result<(), String> {
    if !folder.is_dir() {
        return Err(format!("not a folder: {}", folder.display()));
    }
    let root = NSString::from_str(&folder.to_string_lossy());
    if NSWorkspace::sharedWorkspace().selectFile_inFileViewerRootedAtPath(None, &root) {
        Ok(())
    } else {
        Err(format!("Finder did not show {}", folder.display()))
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use super::*;

    /// A fresh folder under the Mac's temp folder (`/var/folders/…`, which the file system calls `/private/var/folders/…`).
    fn folder(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("mighty-files-{name}-{}-{:?}", std::process::id(), std::thread::current().id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    /// An open file and its folder name themselves as the file system does: the temp folder's `/var` comes back as `/private/var`
    /// (what git and `realpath` say), so a folder and a file under it compare the same way (delegation's `may_touch`, the files pane's
    /// containment).
    #[test]
    fn the_handle_names_the_canonical_path() {
        let root = folder("canonical");
        let file = root.join("a.txt");
        std::fs::write(&file, b"a").unwrap();
        let canonical = std::fs::canonicalize(&root).unwrap();
        assert_eq!(MacFileHandles.final_directory_path(&root), Some(canonical.clone()), "the folder's final name");
        let opened = File::open(&file).unwrap();
        assert_eq!(MacFileHandles.final_path(&opened), Some(canonical.join("a.txt")), "the file's final name");
        assert!(MacFileHandles.asks_handles() && MacFileHandles.is_disk(&opened));
        assert!(MacFileHandles.is_disk(&File::open(&root).unwrap()), "a folder is on the disk");
        assert!(!MacFileHandles.is_disk(&File::open("/dev/null").unwrap()), "a device is not");
        assert_eq!(MacFileHandles.final_directory_path(&root.join("missing")), None);
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Plan §1.7 (W8 review L1): one name, a regular file; a hard link elsewhere or a folder is not the profile's own.
    #[test]
    fn single_link_is_true_only_for_a_regular_file_with_one_link() {
        let root = folder("links");
        let file = root.join("style.json");
        std::fs::write(&file, b"{}").unwrap();
        assert!(MacFileHandles.single_link(&File::open(&file).unwrap()));
        std::fs::hard_link(&file, root.join("other.json")).unwrap();
        assert!(!MacFileHandles.single_link(&File::open(&file).unwrap()), "hard-linked from elsewhere");
        assert!(!MacFileHandles.single_link(&File::open(&root).unwrap()), "a folder");
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// The gap between walking a path and opening it: a folder on the way swapped for a link to somewhere else after the walk is
    /// caught, because the handle names where the open really went.
    #[test]
    fn the_handle_names_the_file_after_a_swap_to_a_link_is_caught() {
        let root = folder("swap");
        let (inside, outside) = (root.join("workspace"), root.join("elsewhere"));
        std::fs::create_dir_all(inside.join("sub")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(inside.join("sub/a.txt"), b"in").unwrap();
        std::fs::write(outside.join("a.txt"), b"out").unwrap();
        let walked = std::fs::canonicalize(inside.join("sub/a.txt")).unwrap();
        // After the walk, `sub` becomes a link out of the workspace.
        std::fs::remove_dir_all(inside.join("sub")).unwrap();
        std::os::unix::fs::symlink(&outside, inside.join("sub")).unwrap();
        let opened = File::open(inside.join("sub/a.txt")).unwrap();
        let named = MacFileHandles.final_path(&opened).unwrap();
        assert_ne!(named, walked, "the handle does not say the walked path");
        assert_eq!(named, std::fs::canonicalize(&outside).unwrap().join("a.txt"), "it says where the open went");
        let workspace = MacFileHandles.final_directory_path(&inside).unwrap();
        assert!(!named.starts_with(&workspace), "so the file is outside the workspace");
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// A one-page PDF (200 × 100 points, `/Rotate rotate`) with its cross-reference table, a black box over its left half.
    fn pdf(rotate: i32) -> Vec<u8> {
        let content = "0 0 0 rg 0 0 100 100 re f";
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Rotate {rotate} /Contents 4 0 R >>"),
            format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        ];
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offsets = vec![];
        for (index, body) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", index + 1).as_bytes());
        }
        let table = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
        for offset in offsets {
            out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{table}\n%%EOF\n", objects.len() + 1).as_bytes());
        out
    }

    fn png_size(png: &[u8]) -> (u32, u32) {
        let word = |at: usize| u32::from_be_bytes([png[at], png[at + 1], png[at + 2], png[at + 3]]);
        (word(16), word(20))
    }

    /// The pixel at (x, y) from the top left of a PNG, read back through `NSBitmapImageRep`: (r, g, b).
    fn pixel(png: &[u8], x: isize, y: isize) -> (u8, u8, u8) {
        let rep = NSBitmapImageRep::imageRepWithData(&NSData::with_bytes(png)).unwrap();
        let (data, row) = (rep.bitmapData(), rep.bytesPerRow());
        let samples = rep.samplesPerPixel();
        // SAFETY: inside the rep's own buffer (row × pixelsHigh bytes).
        unsafe {
            let at = data.offset(y * row + x * samples);
            (*at, *at.add(1), *at.add(2))
        }
    }

    /// The first page's size is in DIPs as Windows.Data.Pdf reports it (96 to the inch), and it is drawn at exactly the size the
    /// shared sizing picks, as a PNG, white under the page; a turned page is sized and drawn turned; nothing is drawn when no size
    /// is asked for, and a document that does not load is an error.
    #[test]
    fn a_pdf_first_page_rasters_to_the_fitted_size() {
        let page = MacPdf.first_page(&pdf(0), &|width, height| Some(((width / 2.0) as u32, (height / 2.0) as u32))).unwrap().unwrap();
        assert!((page.width - 200.0 * 96.0 / 72.0).abs() < 0.01 && (page.height - 100.0 * 96.0 / 72.0).abs() < 0.01, "{page:?}");
        let png = page.png.unwrap();
        assert!(png.starts_with(&[0x89, b'P', b'N', b'G']), "a PNG");
        assert_eq!(png_size(&png), ((page.width / 2.0) as u32, (page.height / 2.0) as u32), "drawn at the size asked for");
        let (w, h) = png_size(&png);
        assert_eq!(pixel(&png, 5, (h / 2) as isize), (0, 0, 0), "the box on the left");
        assert_eq!(pixel(&png, (w - 5) as isize, (h / 2) as isize), (255, 255, 255), "white on the right");

        let turned = MacPdf.first_page(&pdf(90), &|width, height| Some((width as u32, height as u32))).unwrap().unwrap();
        assert!((turned.width - 100.0 * 96.0 / 72.0).abs() < 0.01 && (turned.height - 200.0 * 96.0 / 72.0).abs() < 0.01, "{turned:?}");
        let png = turned.png.unwrap();
        let (w, h) = png_size(&png);
        assert_eq!((w, h), (turned.width as u32, turned.height as u32));
        assert_eq!(pixel(&png, (w / 2) as isize, 5), (0, 0, 0), "turned clockwise, the left half is on top");
        assert_eq!(pixel(&png, (w / 2) as isize, (h - 5) as isize), (255, 255, 255));

        assert_eq!(MacPdf.first_page(&pdf(0), &|_, _| None).unwrap().unwrap().png, None, "not drawn when no size is asked for");
        assert!(MacPdf.first_page(b"not a pdf", &|_, _| None).is_err(), "a document that does not load");
    }

    /// As Windows refuses programs: an app bundle (a folder), an executable file and a script Terminal would run are never opened,
    /// and nothing reaches LaunchServices for them; the rule itself refuses the types and launchers that run what they hold.
    #[test]
    fn desktop_refuses_an_app_bundle_and_an_executable() {
        let root = folder("desktop");
        let bundle = root.join("Tool.app");
        std::fs::create_dir_all(bundle.join("Contents/MacOS")).unwrap();
        assert!(!MacDesktop.open(&bundle).unwrap(), "an app bundle");
        let program = root.join("program");
        std::fs::File::create(&program).unwrap().write_all(b"#!/bin/sh\necho no\n").unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(!MacDesktop.open(&program).unwrap(), "an executable file");
        let script = root.join("run.command");
        std::fs::write(&script, b"echo no\n").unwrap();
        assert!(!MacDesktop.open(&script).unwrap(), "a script Terminal runs, executable bit or not");
        assert!(!MacDesktop.open(&root.join("missing.txt")).unwrap(), "a missing file");
        assert!(runs_code(&root.join("a.txt"), 0o744));
        for (extension, handler) in
            [("pkg", None), ("JAR", None), ("webloc", None), ("py", Some("Python Launcher.app")), ("x", Some("Terminal.app"))]
        {
            assert!(opens_as_code(extension, handler), "{extension} {handler:?}");
        }
        for (extension, handler) in [("txt", Some("TextEdit.app")), ("pdf", Some("Preview.app")), ("html", None), ("png", None)] {
            assert!(!opens_as_code(extension, handler), "{extension} {handler:?}");
        }
        assert!(MacDesktop.reveal(&root.join("missing.txt")).is_err(), "nothing to show");
        assert!(show_in_explorer(&script).is_err() && show_in_explorer(&root.join("missing")).is_err(), "not a folder");
        assert!(open_url("file:///Applications/Calculator.app").is_err(), "a file address");
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// The picture goes on a pasteboard as PNG (the bytes given) and TIFF; a pasteboard of the test's own, never the user's.
    #[test]
    fn a_picture_goes_on_the_pasteboard_as_png_and_tiff() {
        let pixels = vec![255u8, 0, 0, 255, 0, 0, 255, 255];
        let png = png_of(&pixels, 2, 1).unwrap();
        let image = ClipboardImage { width: 2, height: 1, rgba: pixels, png: png.clone() };
        let pasteboard = NSPasteboard::pasteboardWithUniqueName();
        let put = put_image(&pasteboard, &image);
        // SAFETY: AppKit's own pasteboard type constants.
        let (got_png, got_tiff) = unsafe { (pasteboard.dataForType(NSPasteboardTypePNG), pasteboard.dataForType(NSPasteboardTypeTIFF)) };
        let refused = put_image(&pasteboard, &ClipboardImage { png: b"no".to_vec(), ..image });
        // SAFETY: `releaseGlobally` takes nothing and returns nothing; the pasteboard is not used after it.
        let () = unsafe { objc2::msg_send![&pasteboard, releaseGlobally] };
        put.unwrap();
        assert_eq!(got_png.map(|d| d.to_vec()), Some(png));
        assert!(got_tiff.is_some_and(|d| d.len() > 8));
        assert!(refused.is_err(), "bytes that are no PNG");
    }
}
