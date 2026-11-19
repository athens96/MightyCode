//! M-G: the files pane's ports on the Mac (plan §1.7; contract `mighty_platform::files`), where Windows has
//! `GetFinalPathNameByHandleW`, `Windows.Data.Pdf` and the shell's file actions:
//! - [`MacFileHandles`]: an open file's path from `fcntl(F_GETPATH)` (what the file system calls it: `/var/…` is `/private/var/…`),
//!   a folder's through an `O_EVTONLY` handle, `single_link` when `fstat` gives `st_nlink == 1 && S_IFREG`; `asks_handles` true; a
//!   file opened to read without blocking and without following a last link, so a FIFO cannot hang the reader.
//! - [`MacPdf`]: the first page through `CGPDFDocument`, drawn into a bitmap at the size asked for and sized in DIPs (96 to the
//!   inch) as `Windows.Data.Pdf` reports it, so the shared sizing is the same on both.
//! - [`MacDesktop`]: show in Finder (`activateFileViewerSelectingURLs`, Swift `FilePaneView`), open with the default app (`NSWorkspace
//!   openURL`) only for a plain document ([`refusal`]: decided by what the file system and LaunchServices say the file is, its name
//!   second), as Windows refuses programs; a picture on the pasteboard as PNG and TIFF.
//! - [`open_url`] (web and mail addresses only) and [`show_in_explorer`] (Swift `selectFile(nil, inFileViewerRootedAtPath:)`).
//!
//! Every AppKit and Foundation call here runs on the caller's thread (a blocking worker) inside an autorelease pool of its own.

use std::ffi::{OsStr, c_void};
use std::fs::File;
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};
use std::os::unix::io::AsRawFd as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use core_foundation::base::TCFType as _;
use core_foundation::data::CFData;
use core_foundation::string::CFString;
use mighty_platform::{ClipboardImage, Desktop, FileHandles, FilePorts, PdfPage, PdfRaster, PlatformError, Result};
use objc2::AllocAnyThread as _;
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::AnyObject;
use objc2_app_kit::{
    NSBitmapImageFileType, NSBitmapImageRep, NSDeviceRGBColorSpace, NSImage, NSPasteboard, NSPasteboardTypePNG, NSPasteboardTypeTIFF,
    NSWorkspace,
};
use objc2_foundation::{
    NSArray, NSBundle, NSData, NSDictionary, NSFileManager, NSNumber, NSString, NSURL, NSURLIsAliasFileKey, NSURLIsApplicationKey,
    NSURLIsExecutableKey, NSURLIsPackageKey, NSURLIsRegularFileKey, NSURLIsSymbolicLinkKey, NSURLResourceKey,
};

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

    /// `O_RDONLY | O_NONBLOCK | O_NOFOLLOW | O_CLOEXEC` (std adds the last), then blocking again: a FIFO or a device a name opened
    /// returns at once (and [`FileHandles::is_disk`] refuses it), and a last part swapped for a link since the walk is not followed.
    fn open_read(&self, path: &Path) -> std::io::Result<File> {
        let file = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW).open(path)?;
        let fd = file.as_raw_fd();
        // SAFETY: the file's own open descriptor; F_GETFL and F_SETFL read and set its status flags.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags == -1 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags & !libc::O_NONBLOCK) } == -1 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(file)
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
    fn CGDataProviderCreateWithCFData(data: *const c_void) -> Ref;
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
/// The most a drawn page or a picture may take in memory (RGBA bytes): a page asked for at a larger size is refused.
const MAXIMUM_PIXEL_BYTES: usize = 64 << 20;
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

/// The bytes of `width` × `height` RGBA pixels, when they fit in [`MAXIMUM_PIXEL_BYTES`].
fn pixel_bytes(width: usize, height: usize) -> Option<usize> {
    width.checked_mul(height)?.checked_mul(4).filter(|&bytes| bytes <= MAXIMUM_PIXEL_BYTES)
}

/// RGBA pixels (rows from the top, alpha 255) as PNG, through `NSBitmapImageRep`.
fn png_of(rgba: &[u8], width: usize, height: usize) -> Option<Vec<u8>> {
    let bytes = pixel_bytes(width, height).filter(|&bytes| bytes == rgba.len())?;
    autoreleasepool(|_| png_in_pool(rgba, width, height, bytes))
}

fn png_in_pool(rgba: &[u8], width: usize, height: usize, bytes: usize) -> Option<Vec<u8>> {
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
    // SAFETY: the rep's own buffer holds width × height × 4 bytes (its row length as given), as does `rgba` (checked above).
    unsafe { std::ptr::copy_nonoverlapping(rgba.as_ptr(), target, bytes) };
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
        // The provider holds a CFData copy of its own (it retains it), never a borrow of `data` that CoreGraphics could outlive.
        let owned = CFData::from_buffer(data);
        // SAFETY: a valid CFData; the provider retains it.
        let provider = unsafe { CGDataProviderCreateWithCFData(owned.as_concrete_TypeRef().cast()) };
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
                let Some(bytes) = pixel_bytes(w, h) else {
                    release();
                    return Err(PlatformError::Refused(format!("the page is too large to draw at {w} × {h}")));
                };
                let mut pixels = vec![0u8; bytes];
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

/// Names that run what they hold when opened, whatever app opens them: scripts Terminal runs, installers, Automator actions and
/// workflows, Java archives, system plug-ins that install themselves, the location files that open whatever they name (an app among
/// them), web archives, and Windows' programs and scripts (CrossOver or Wine run them). The second layer: [`refusal`] decides by the
/// file's type first.
const CODE_TYPES: &[&str] = &[
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
    "ftploc",
    "vncloc",
    "mailloc",
    "newsloc",
    "atloc",
    "url",
    "webarchive",
    "exe",
    "msi",
    "bat",
    "cmd",
    "com",
    "lnk",
    "reg",
    "vbs",
    "ps1",
    "scr",
];

/// The types (UTIs) that run code or open something else, by conformance: programs, apps and bundles, scripts, aliases and links,
/// location files, installers, Java archives, Automator's files, web archives and Windows' programs.
const CODE_UTIS: &[&str] = &[
    "public.executable",
    "com.apple.application",
    "com.apple.bundle",
    "public.script",
    "com.apple.resolvable",
    "com.apple.alias-file",
    "public.symlink",
    "com.apple.internet-location",
    "com.apple.installer-package-archive",
    "com.sun.java-archive",
    "com.apple.automator-workflow",
    "com.apple.automator-action",
    "com.apple.webarchive",
    "com.microsoft.windows-executable",
];

/// The apps that run what they are handed (terminals, an installer, launchers, Windows' runners), by the start of their bundle
/// identifier in lower case: a file whose default app is one of these is never opened, whatever its type.
const LAUNCHER_IDS: &[&str] = &[
    "com.apple.terminal",
    "com.googlecode.iterm2",
    "com.apple.installer",
    "com.apple.jarlauncher",
    "org.python.pythonlauncher",
    "com.apple.automatorinstaller",
    "com.apple.systempreferences",
    "com.apple.scriptmenu",
    "com.mitchellh.ghostty",
    "com.github.wez.wezterm",
    "dev.warp.warp",
    "net.kovidgoyal.kitty",
    "org.alacritty",
    "io.alacritty",
    "co.zeit.hyper",
    "org.tabby",
    "com.codeweavers.crossover",
    "org.winehq",
    "com.isaacmarovitz.whisky",
];

/// The same apps by their bundle's name without `.app`, in lower case, for one whose identifier says nothing.
const LAUNCHERS: &[&str] = &[
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
    "ghostty",
    "wezterm",
    "warp",
    "kitty",
    "alacritty",
    "hyper",
    "tabby",
    "crossover",
    "wine",
    "whisky",
];

/// Whether a file named with `extension` (no dot) opened by the app `handler` (its bundle's file name, `Terminal.app`) with the bundle
/// identifier `bundle_id` would run code: the names and launchers layer of [`refusal`].
pub fn opens_as_code(extension: &str, handler: Option<&str>, bundle_id: Option<&str>) -> bool {
    CODE_TYPES.iter().any(|t| t.eq_ignore_ascii_case(extension))
        || bundle_id.is_some_and(|id| {
            let id = id.to_lowercase();
            LAUNCHER_IDS.iter().any(|launcher| id.starts_with(launcher))
        })
        || handler.is_some_and(|name| {
            let name = name.to_lowercase();
            LAUNCHERS.contains(&name.strip_suffix(".app").unwrap_or(&name))
        })
}

#[link(name = "CoreServices", kind = "framework")]
unsafe extern "C" {
    fn UTTypeConformsTo(uti: *const c_void, conforms_to: *const c_void) -> u8;
}

/// Whether the type `uti` is (or conforms to) `parent` (LaunchServices' declared types).
fn conforms(uti: &str, parent: &str) -> bool {
    let (uti, parent) = (CFString::new(uti), CFString::new(parent));
    // SAFETY: two valid CFStrings that live across the call.
    unsafe { UTTypeConformsTo(uti.as_concrete_TypeRef().cast(), parent.as_concrete_TypeRef().cast()) != 0 }
}

fn file_url(path: &Path) -> Retained<NSURL> {
    NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()))
}

/// A resource value of the file at `url` (Foundation's `getResourceValue:forKey:`), None when it does not read.
fn resource(url: &NSURL, key: &NSURLResourceKey) -> Option<Retained<AnyObject>> {
    let mut value = None;
    // SAFETY: `value` is an out-parameter of the type the method fills.
    unsafe { url.getResourceValue_forKey_error(&mut value, key) }.ok()?;
    value
}

fn flag(url: &NSURL, key: &NSURLResourceKey) -> Option<bool> {
    resource(url, key)?.downcast::<NSNumber>().ok().map(|number| number.as_bool())
}

/// The extended attribute by which a file names the app it opens with (Finder's "Open With" for this file alone).
const OPEN_WITH: &std::ffi::CStr = c"com.apple.LaunchServices.OpenWith";

/// Whether the file at `path` (not followed) carries [`OPEN_WITH`]; None when its attributes do not read.
fn opens_with_its_own_app(path: &Path) -> Option<bool> {
    let name = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: NUL-terminated strings; a null buffer of size 0 asks only whether the attribute is there.
    let found = unsafe { libc::getxattr(name.as_ptr(), OPEN_WITH.as_ptr(), std::ptr::null_mut(), 0, 0, libc::XATTR_NOFOLLOW) };
    if found >= 0 {
        return Some(true);
    }
    match std::io::Error::last_os_error().raw_os_error() {
        Some(libc::ENOATTR | libc::ENOTSUP) => Some(false),
        _ => None,
    }
}

/// Where an app the files pane may hand a document to is installed: the system's and the user's Applications folders (a link there
/// counts where it leads). An app LaunchServices found anywhere else (one a build wrote into a workspace) is not trusted to open it.
fn installed_app(app: &Path) -> bool {
    let Ok(real) = std::fs::canonicalize(app) else { return false };
    let home = dirs::home_dir().and_then(|home| std::fs::canonicalize(home.join("Applications")).ok());
    real.starts_with("/Applications") || real.starts_with("/System") || home.is_some_and(|folder| real.starts_with(folder))
}

/// Why the file at `path` is not opened with its default app, or None when it is a plain document an installed app opens. Decided
/// by what the file is, failing closed: not a regular file (a folder, an app bundle, a link), any execute bit, an app of its own
/// ([`OPEN_WITH`]), what Foundation says of it (an alias, a link, a package, an app, executable), its type when unknown (none, or a
/// `dyn.` one no app declares) or conforming to [`CODE_UTIS`]; then by name and app ([`opens_as_code`]); then the app itself: none,
/// or one not installed ([`installed_app`]).
pub fn refusal(path: &Path) -> Option<&'static str> {
    autoreleasepool(|_| {
        let Ok(meta) = std::fs::symlink_metadata(path) else { return Some("it is not there") };
        if !meta.file_type().is_file() {
            return Some("it is not a regular file");
        }
        if meta.mode() & 0o111 != 0 {
            return Some("it is executable");
        }
        match opens_with_its_own_app(path) {
            Some(false) => {}
            Some(true) => return Some("it names an app of its own"),
            None => return Some("its attributes do not read"),
        }
        let url = file_url(path);
        // The type as its identifier's text, which `UTTypeConformsTo` takes (`NSURLContentTypeKey` gives a `UTType` object instead).
        #[allow(deprecated, reason = "the string key still answers on every macOS the app supports")]
        let type_key = unsafe { objc2_foundation::NSURLTypeIdentifierKey };
        // SAFETY: Foundation's own resource key constants.
        let (regular, alias, link, package, application, executable, kind) = unsafe {
            (
                flag(&url, NSURLIsRegularFileKey),
                flag(&url, NSURLIsAliasFileKey),
                flag(&url, NSURLIsSymbolicLinkKey),
                flag(&url, NSURLIsPackageKey),
                flag(&url, NSURLIsApplicationKey),
                flag(&url, NSURLIsExecutableKey),
                resource(&url, type_key).and_then(|value| value.downcast::<NSString>().ok()).map(|uti| uti.to_string()),
            )
        };
        if regular != Some(true) {
            return Some("it is not a regular file");
        }
        if alias != Some(false) || link != Some(false) {
            return Some("it is an alias or a link");
        }
        if package != Some(false) || application != Some(false) || executable != Some(false) {
            return Some("it is a package, an app or executable");
        }
        let Some(kind) = kind.filter(|kind| !kind.is_empty() && !kind.starts_with("dyn.")) else {
            return Some("its type is unknown");
        };
        if CODE_UTIS.iter().any(|parent| conforms(&kind, parent)) {
            return Some("its type runs code");
        }
        let Some(handler) = NSWorkspace::sharedWorkspace().URLForApplicationToOpenURL(&url) else { return Some("no app opens it") };
        let name = handler.lastPathComponent().map(|name| name.to_string());
        let bundle_id = NSBundle::bundleWithURL(&handler).and_then(|bundle| bundle.bundleIdentifier()).map(|id| id.to_string());
        let extension = path.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
        if opens_as_code(&extension, name.as_deref(), bundle_id.as_deref()) {
            return Some("it runs code by its name or its app");
        }
        let app = handler.path().map(|app| PathBuf::from(app.to_string()));
        if !app.is_some_and(|app| installed_app(&app)) {
            return Some("its app is not an installed one");
        }
        None
    })
}

/// The regular file at `path` as it is now: its real path and its status (a link at `path` itself is refused).
fn regular_file(path: &Path) -> Option<(PathBuf, std::fs::Metadata)> {
    if !std::fs::symlink_metadata(path).ok()?.file_type().is_file() {
        return None;
    }
    let real = std::fs::canonicalize(path).ok()?;
    let meta = std::fs::symlink_metadata(&real).ok()?;
    meta.file_type().is_file().then_some((real, meta))
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
        autoreleasepool(|_| {
            NSWorkspace::sharedWorkspace().activateFileViewerSelectingURLs(&NSArray::from_retained_slice(&[file_url(path)]));
        });
        Ok(())
    }

    /// The default app through `NSWorkspace openURL`, for a regular file (no link) that [`refusal`] lets through; just before the
    /// open the file is looked at again and must still be the same one at the same real path, else it is not opened.
    fn open(&self, path: &Path) -> Result<bool> {
        let Some((real, before)) = regular_file(path) else { return Ok(false) };
        if refusal(&real).is_some() {
            return Ok(false);
        }
        match regular_file(path) {
            Some((again, now)) if again == real && now.dev() == before.dev() && now.ino() == before.ino() => {}
            _ => return Ok(false),
        }
        Ok(autoreleasepool(|_| NSWorkspace::sharedWorkspace().openURL(&file_url(&real))))
    }

    /// The general pasteboard gets the picture as PNG and TIFF; it keeps them after the app quits.
    fn copy_image(&self, image: &ClipboardImage) -> Result<()> {
        autoreleasepool(|_| put_image(&NSPasteboard::generalPasteboard(), image))
    }

    /// `FileManager.trashItem(at:resultingItemURL:)`: the file (a link itself, never what it points at) in the Trash, where Finder's Put
    /// Back returns it.
    fn trash(&self, path: &Path) -> Result<()> {
        std::fs::symlink_metadata(path)?;
        autoreleasepool(|_| {
            NSFileManager::defaultManager()
                .trashItemAtURL_resultingItemURL_error(&file_url(path), None)
                .map_err(|error| PlatformError::Refused(error.localizedDescription().to_string()))
        })
    }
}

/// The schemes [`open_url`] hands to their default handler: the web and mail.
const URL_SCHEMES: [&str; 3] = ["http", "https", "mailto"];

/// A web or mail address handed to its default handler (`NSWorkspace openURL:`, C# `Process.Start` with `UseShellExecute`); any other
/// scheme (`file:`, an app's own) is refused: a file goes through [`MacDesktop::open`], which refuses what runs code.
pub fn open_url(url: &str) -> std::result::Result<(), String> {
    autoreleasepool(|_| {
        let Some(address) = NSURL::URLWithString(&NSString::from_str(url)) else { return Err("not an address".into()) };
        if !address.scheme().is_some_and(|scheme| URL_SCHEMES.iter().any(|allowed| scheme.to_string().eq_ignore_ascii_case(allowed))) {
            return Err("not an address the default handler may open".into());
        }
        if NSWorkspace::sharedWorkspace().openURL(&address) { Ok(()) } else { Err("no app opens the address".into()) }
    })
}

/// A folder shown in a Finder window of its own (Swift `selectFile(nil, inFileViewerRootedAtPath:)`).
pub fn show_in_explorer(folder: &Path) -> std::result::Result<(), String> {
    if !folder.is_dir() {
        return Err(format!("not a folder: {}", folder.display()));
    }
    let root = NSString::from_str(&folder.to_string_lossy());
    if autoreleasepool(|_| NSWorkspace::sharedWorkspace().selectFile_inFileViewerRootedAtPath(None, &root)) {
        Ok(())
    } else {
        Err(format!("Finder did not show {}", folder.display()))
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;
    use std::os::unix::fs::PermissionsExt as _;

    use objc2_foundation::NSURLBookmarkCreationOptions;

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

    /// As Windows refuses programs: an app bundle (a folder), an executable file, a script Terminal would run, a link and a type no app
    /// declares are never opened, and nothing reaches LaunchServices' open for them; the names and launchers layer refuses the
    /// types and the apps (by bundle identifier or name) that run what they hold.
    #[test]
    fn desktop_refuses_an_app_bundle_and_an_executable() {
        let root = folder("desktop");
        let bundle = tool_app(&root);
        assert!(!MacDesktop.open(&bundle).unwrap(), "an app bundle");
        let program = root.join("program");
        std::fs::File::create(&program).unwrap().write_all(b"#!/bin/sh\necho no\n").unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(!MacDesktop.open(&program).unwrap(), "an executable file");
        let script = root.join("run.command");
        std::fs::write(&script, b"echo no\n").unwrap();
        assert!(!MacDesktop.open(&script).unwrap(), "a script Terminal runs, executable bit or not");
        assert!(refusal(&script).is_some());
        let document = root.join("notes.txt");
        std::fs::write(&document, b"notes").unwrap();
        let link = root.join("link.txt");
        std::os::unix::fs::symlink(&document, &link).unwrap();
        assert!(!MacDesktop.open(&link).unwrap(), "a link, even to a document");
        let unknown = root.join("data.mightyunknowntype");
        std::fs::write(&unknown, b"?").unwrap();
        assert_eq!(refusal(&unknown), Some("its type is unknown"), "a type no app declares fails closed");
        assert!(!MacDesktop.open(&root.join("missing.txt")).unwrap(), "a missing file");
        for (extension, handler, id) in [
            ("pkg", None, None),
            ("JAR", None, None),
            ("webloc", None, None),
            ("exe", None, None),
            ("ps1", None, None),
            ("py", Some("Python Launcher.app"), None),
            ("x", Some("Terminal.app"), None),
            ("x", Some("Renamed.app"), Some("com.apple.Terminal")),
            ("x", Some("Some Shell.app"), Some("com.mitchellh.ghostty")),
            ("x", Some("Bottle.app"), Some("com.isaacmarovitz.Whisky")),
        ] {
            assert!(opens_as_code(extension, handler, id), "{extension} {handler:?} {id:?}");
        }
        for (extension, handler, id) in [
            ("txt", Some("TextEdit.app"), Some("com.apple.TextEdit")),
            ("pdf", Some("Preview.app"), Some("com.apple.Preview")),
            ("html", None, None),
            ("png", None, None),
        ] {
            assert!(!opens_as_code(extension, handler, id), "{extension} {handler:?} {id:?}");
        }
        assert!(MacDesktop.reveal(&root.join("missing.txt")).is_err(), "nothing to show");
        assert!(show_in_explorer(&script).is_err() && show_in_explorer(&root.join("missing")).is_err(), "not a folder");
        for address in ["file:///Applications/Calculator.app", "x-apple.systempreferences:com.apple.preference", "ms-settings:x", "nope"] {
            assert!(open_url(address).is_err(), "{address}: only web and mail addresses");
        }
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// An app bundle of the test's own (`Tool.app`, an executable script inside), as a build in a workspace could write one.
    fn tool_app(root: &Path) -> PathBuf {
        let bundle = root.join("Tool.app");
        std::fs::create_dir_all(bundle.join("Contents/MacOS")).unwrap();
        std::fs::write(
            bundle.join("Contents/Info.plist"),
            concat!(
                r#"<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict>"#,
                "<key>CFBundleExecutable</key><string>tool</string>",
                "<key>CFBundleIdentifier</key><string>dev.mightycode.test.tool</string>",
                "<key>CFBundlePackageType</key><string>APPL</string>",
                "</dict></plist>"
            ),
        )
        .unwrap();
        let executable = bundle.join("Contents/MacOS/tool");
        std::fs::write(&executable, b"#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        bundle
    }

    /// Review M-G 1: a Finder alias is decided by what it is, not its name. One named `notes.txt` that LaunchServices would resolve to
    /// the test's own `Tool.app` is refused, and `open` never hands it on.
    #[test]
    fn an_alias_named_as_a_document_is_refused() {
        let root = folder("alias");
        let bundle = tool_app(&root);
        let alias = root.join("notes.txt");
        let options = NSURLBookmarkCreationOptions::SuitableForBookmarkFile;
        let data =
            file_url(&bundle).bookmarkDataWithOptions_includingResourceValuesForKeys_relativeToURL_error(options, None, None).unwrap();
        NSURL::writeBookmarkData_toURL_options_error(&data, &file_url(&alias), options.bits()).unwrap();
        assert!(std::fs::symlink_metadata(&alias).unwrap().file_type().is_file(), "an alias is a regular file to the file system");
        let url = file_url(&alias);
        // SAFETY: Foundation's own resource key constant.
        assert_eq!(unsafe { flag(&url, NSURLIsAliasFileKey) }, Some(true), "Foundation calls it an alias");
        assert!(refusal(&alias).is_some(), "{:?}", refusal(&alias));
        assert!(!MacDesktop.open(&alias).unwrap());
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// A document that names an app of its own (Finder's "Open With" for this file: `com.apple.LaunchServices.OpenWith`) is refused,
    /// whatever app it names; a plain one an installed app opens is not.
    #[test]
    fn a_document_naming_its_own_app_is_refused() {
        let root = folder("open-with");
        let plain = root.join("plain.txt");
        std::fs::write(&plain, b"plain").unwrap();
        assert_eq!(refusal(&plain), None, "a text file TextEdit opens");
        let named = root.join("named.txt");
        std::fs::write(&named, b"named").unwrap();
        let (path, value) = (std::ffi::CString::new(named.as_os_str().as_bytes()).unwrap(), b"bplist00");
        // SAFETY: NUL-terminated strings and a buffer of the length given.
        let set = unsafe { libc::setxattr(path.as_ptr(), OPEN_WITH.as_ptr(), value.as_ptr().cast(), value.len(), 0, 0) };
        assert_eq!(set, 0, "{}", std::io::Error::last_os_error());
        assert_eq!(refusal(&named), Some("it names an app of its own"));
        assert!(!MacDesktop.open(&named).unwrap());
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Review M-G 4: a FIFO a name opened does not hang the reader (it opens at once and is no disk file), the descriptor of a file
    /// blocks again once open, and a last part that is a link is not followed.
    #[test]
    fn open_read_does_not_block_on_a_fifo_or_follow_a_link() {
        let root = folder("fifo");
        let fifo = root.join("pipe");
        let name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        // SAFETY: a NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        let opened = MacFileHandles.open_read(&fifo).unwrap();
        assert!(!MacFileHandles.is_disk(&opened), "a FIFO is no disk file");
        let file = root.join("a.txt");
        std::fs::write(&file, b"a").unwrap();
        let opened = MacFileHandles.open_read(&file).unwrap();
        // SAFETY: the file's own descriptor.
        assert_eq!(unsafe { libc::fcntl(opened.as_raw_fd(), libc::F_GETFL) } & libc::O_NONBLOCK, 0, "blocking again");
        std::os::unix::fs::symlink(&file, root.join("link")).unwrap();
        assert!(MacFileHandles.open_read(&root.join("link")).is_err(), "a link is not followed");
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Review M-G 7: a size whose pixels would overflow or pass the memory cap is refused, never allocated.
    #[test]
    fn a_page_too_large_to_draw_is_refused() {
        assert_eq!(pixel_bytes(usize::MAX, 2), None);
        assert_eq!(pixel_bytes(5_000, 5_000), None, "100 MB");
        assert_eq!(pixel_bytes(2_000, 1_000), Some(8_000_000));
        assert!(MacPdf.first_page(&pdf(0), &|_, _| Some((100_000, 100_000))).is_err());
        assert_eq!(png_of(&[0; 4], 2, 1), None, "fewer bytes than the size says");
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
