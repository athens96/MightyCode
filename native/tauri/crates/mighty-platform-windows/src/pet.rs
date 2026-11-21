//! The companion pet's window on Windows (C# WinUI\CompanionOverlay.cs; plan §0.6, §1.7, D8, W8-E): a
//! `WS_EX_NOACTIVATE|WS_EX_LAYERED|WS_EX_TOOLWINDOW` popup of a window class of its own, `WM_MOUSEACTIVATE` answered `MA_NOACTIVATE` (the
//! user's application keeps the keyboard and its IME while the pet is carried or its buttons pressed), kept topmost with
//! `SetWindowPos(HWND_TOPMOST, SWP_NOACTIVATE)`, per-monitor DPI clamped to 1–4 with the re-entry guard on `WM_DPICHANGED`,
//! mighty-platform's `PetScene` drawn with Direct2D and DirectWrite (the system's renderer, the software one where it cannot) onto a DIB
//! section and put on screen with `UpdateLayeredWindow` (premultiplied BGRA: clicks pass through where alpha is 0), the window
//! procedure catching panics. Owner: W8-E.
//!
//! The window lives on a thread of its own with its message loop. Every call posts to it and returns (C#'s overlay ran on the UI
//! thread); `send` (the smoke's input) waits for the window procedure. The words are measured with the same DirectWrite formats they
//! are drawn with ([`DirectWriteMeasure`]), as C# measures with the layouts it draws.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use mighty_platform::pet::{
    BODY, MONO, PetBitmap, PetColor, PetGlyphSize, PetItem, PetLayout, PetMeasure, PetPoint, PetRect, PetScene, PetSegment, PetText, WIDE,
};
use mighty_platform::surface::{PetCapture, PetCursor, PetFocus, PetInput, PetPoll};
use mighty_platform::{LogicalPoint, LogicalRect, PetEvent, PetPointer, PetWindow, PlatformError, Result};
use tokio::sync::mpsc;
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D_SIZE_U, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_BEZIER_SEGMENT, D2D1_COLOR_F, D2D1_FIGURE_BEGIN_FILLED,
    D2D1_FIGURE_BEGIN_HOLLOW, D2D1_FIGURE_END_CLOSED, D2D1_FIGURE_END_OPEN, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_ARC_SEGMENT, D2D1_ARC_SIZE_LARGE, D2D1_ARC_SIZE_SMALL, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
    D2D1_BITMAP_PROPERTIES, D2D1_CAP_STYLE_ROUND, D2D1_DASH_STYLE_SOLID, D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
    D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_ELLIPSE, D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_FEATURE_LEVEL_DEFAULT,
    D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC, D2D1_LAYER_OPTIONS_NONE, D2D1_LAYER_PARAMETERS, D2D1_LINE_JOIN_ROUND,
    D2D1_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_TYPE, D2D1_RENDER_TARGET_TYPE_DEFAULT, D2D1_RENDER_TARGET_TYPE_SOFTWARE,
    D2D1_RENDER_TARGET_USAGE_NONE, D2D1_ROUNDED_RECT, D2D1_STROKE_STYLE_PROPERTIES, D2D1_SWEEP_DIRECTION_CLOCKWISE,
    D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE, D2D1CreateFactory, ID2D1Bitmap, ID2D1DCRenderTarget, ID2D1DeviceContext, ID2D1Factory,
    ID2D1RenderTarget, ID2D1SolidColorBrush, ID2D1StrokeStyle,
};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_FEATURE, DWRITE_FONT_FEATURE_TAG_TABULAR_FIGURES, DWRITE_FONT_LINE_GAP_USAGE_DEFAULT,
    DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_ITALIC, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT, DWRITE_LINE_METRICS,
    DWRITE_LINE_SPACING, DWRITE_LINE_SPACING_METHOD_PROPORTIONAL, DWRITE_TEXT_METRICS, DWRITE_TEXT_RANGE, DWRITE_TRIMMING,
    DWRITE_TRIMMING_GRANULARITY_CHARACTER, DWRITE_WORD_WRAPPING_EMERGENCY_BREAK, DWRITE_WORD_WRAPPING_NO_WRAP, DWriteCreateFactory,
    IDWriteFactory, IDWriteFontCollection, IDWriteTextFormat, IDWriteTextFormat3, IDWriteTextLayout,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS,
    DeleteDC, DeleteObject, GdiFlush, GetMonitorInfoW, HBITMAP, HDC, HGDIOBJ, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
    SelectObject,
};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForMonitor, MDT_EFFECTIVE_DPI, SetThreadDpiAwarenessContext,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CS_DBLCLKS, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GUITHREADINFO, GWL_EXSTYLE, GetCursorPos,
    GetForegroundWindow, GetGUIThreadInfo, GetMessageW, GetWindowLongPtrW, GetWindowRect, HCURSOR, HTCLIENT, HWND_TOPMOST, IDC_ARROW,
    IDC_SIZENESW, IDC_SIZENS, IDC_SIZENWSE, IDC_SIZEWE, KillTimer, LoadCursorW, MA_NOACTIVATE, MSG, PostMessageW, PostQuitMessage,
    RegisterClassExW, SPI_GETCLIENTAREAANIMATION, SW_SHOWNA, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
    SendMessageW, SetCursor, SetTimer, SetWindowPos, ShowWindow, SystemParametersInfoW, TranslateMessage, ULW_ALPHA, UnregisterClassW,
    UpdateLayeredWindow, WINDOW_EX_STYLE, WM_APP, WM_CANCELMODE, WM_CAPTURECHANGED, WM_DESTROY, WM_DISPLAYCHANGE, WM_DPICHANGED,
    WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEACTIVATE, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_RBUTTONUP, WM_SETCURSOR, WM_TIMER,
    WNDCLASSEXW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP,
};
use windows::core::{BOOL, HSTRING, Interface, PCWSTR};
use windows_numerics::{Matrix3x2, Vector2};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, ReleaseCapture, SetCapture};

/// Frames in a row the system refused, and how many of them (three seconds' worth) end the pet (C# `RefusedFrames`).
const REFUSED_FRAMES: u32 = 36;
/// The posted message that tells the window thread its queue has commands.
const WM_COMMANDS: u32 = WM_APP + 1;
/// The timer that draws a refused frame again (one tick of the 1/12 s clock).
const RETRY_TIMER: usize = 1;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn failed(what: &str, error: windows::core::Error) -> PlatformError {
    PlatformError::Refused(format!("{what}: {error}"))
}

// ── Text: the formats C# lays out and draws with ──

/// DirectWrite's shared factory and the families of each font stack found installed (C# `Family(names)`). The shared factory and its
/// objects are free-threaded (DirectWrite's documentation), so layouts are made on any thread.
struct Text {
    factory: IDWriteFactory,
    collection: IDWriteFontCollection,
    families: Mutex<HashMap<String, String>>,
}

// SAFETY: a DirectWrite factory made with DWRITE_FACTORY_TYPE_SHARED, and its system font collection, may be used from any thread.
unsafe impl Send for Text {}
// SAFETY: as above; nothing here holds thread affinity.
unsafe impl Sync for Text {}

impl Text {
    fn new() -> Result<Text> {
        // SAFETY: plain DirectWrite factory creation and a read of its system collection.
        unsafe {
            let factory: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).map_err(|e| failed("DirectWrite", e))?;
            let mut collection = None;
            factory.GetSystemFontCollection(&mut collection, false).map_err(|e| failed("DirectWrite", e))?;
            let collection = collection.ok_or_else(|| PlatformError::Refused("DirectWrite: no system font collection".into()))?;
            Ok(Text { factory, collection, families: Mutex::default() })
        }
    }

    /// The first family of a stack that is installed, else Segoe UI (C# `Family`).
    fn family(&self, stack: &str) -> String {
        if let Some(known) = lock(&self.families).get(stack) {
            return known.clone();
        }
        let found = stack
            .split(',')
            .map(str::trim)
            .find(|name| {
                let (mut index, mut exists) = (0u32, BOOL(0));
                // SAFETY: a NUL-terminated name and two out-parameters that live across the call.
                unsafe { self.collection.FindFamilyName(&HSTRING::from(*name), &mut index, &mut exists) }.is_ok() && exists.as_bool()
            })
            .unwrap_or("Segoe UI")
            .to_string();
        lock(&self.families).insert(stack.to_string(), found.clone());
        found
    }

    fn format(&self, family: &str, size: f64, weight: u16, lines: u32, spaced: bool) -> Result<IDWriteTextFormat> {
        let family = self.family(family);
        // SAFETY: DirectWrite calls on the objects it returned.
        unsafe {
            let format = self
                .factory
                .CreateTextFormat(
                    &HSTRING::from(family),
                    None,
                    DWRITE_FONT_WEIGHT(i32::from(weight)),
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    size as f32,
                    &HSTRING::from(""),
                )
                .map_err(|e| failed("DirectWrite", e))?;
            let _ = format.SetWordWrapping(if lines == 1 { DWRITE_WORD_WRAPPING_NO_WRAP } else { DWRITE_WORD_WRAPPING_EMERGENCY_BREAK });
            if let Ok(sign) = self.factory.CreateEllipsisTrimmingSign(&format) {
                let trimming = DWRITE_TRIMMING { granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER, delimiter: 0, delimiterCount: 0 };
                let _ = format.SetTrimming(&trimming, &sign);
            }
            // Nine tenths of the font's own line spacing, proportionally (Win2D's `CanvasLineSpacingMode.Proportional`).
            if spaced {
                let spacing = DWRITE_LINE_SPACING {
                    method: DWRITE_LINE_SPACING_METHOD_PROPORTIONAL,
                    height: mighty_platform::pet::LINE_SCALE as f32,
                    baseline: mighty_platform::pet::LINE_SCALE as f32,
                    leadingBefore: 0.0,
                    fontLineGapUsage: DWRITE_FONT_LINE_GAP_USAGE_DEFAULT,
                };
                match format.cast::<IDWriteTextFormat3>() {
                    Ok(three) => {
                        let _ = three.SetLineSpacing(&spacing);
                    }
                    Err(_) => {
                        let _ = format.SetLineSpacing(DWRITE_LINE_SPACING_METHOD_PROPORTIONAL, spacing.height, spacing.baseline);
                    }
                }
            }
            Ok(format)
        }
    }

    /// A text's layout at its width (its height unbounded), with its runs' faces and its tabular figures; the UTF-16 it holds.
    fn layout(&self, text: &PetText) -> Result<(IDWriteTextLayout, Vec<u16>)> {
        let mono_text = text.family == MONO;
        let format = self.format(&text.family, text.size, text.weight, text.max_lines, !mono_text)?;
        let words: Vec<u16> = text.runs.iter().flat_map(|run| run.text.encode_utf16()).collect();
        let width = if text.rect.width > 0.0 { text.rect.width } else { WIDE };
        // SAFETY: DirectWrite calls on the objects it returned, over ranges inside the words.
        unsafe {
            let layout = self.factory.CreateTextLayout(&words, &format, width as f32, WIDE as f32).map_err(|e| failed("DirectWrite", e))?;
            let mut at = 0u32;
            let mono = HSTRING::from(self.family(MONO));
            for run in &text.runs {
                let length = run.text.encode_utf16().count() as u32;
                let range = DWRITE_TEXT_RANGE { startPosition: at, length };
                if run.bold {
                    let _ = layout.SetFontWeight(DWRITE_FONT_WEIGHT(700), range);
                }
                if run.italic {
                    let _ = layout.SetFontStyle(DWRITE_FONT_STYLE_ITALIC, range);
                }
                if run.mono && !mono_text {
                    let _ = layout.SetFontFamilyName(&mono, range);
                    let _ = layout.SetFontSize((text.size - 1.0) as f32, range);
                }
                at += length;
            }
            if text.tabular
                && !words.is_empty()
                && let Ok(typography) = self.factory.CreateTypography()
            {
                let _ = typography.AddFontFeature(DWRITE_FONT_FEATURE { nameTag: DWRITE_FONT_FEATURE_TAG_TABULAR_FIGURES, parameter: 1 });
                let _ = layout.SetTypography(&typography, DWRITE_TEXT_RANGE { startPosition: 0, length: words.len() as u32 });
            }
            Ok((layout, words))
        }
    }

    /// C# `Icon`'s probe: the font size at which `glyph` inks `tall` high, and the ink's box at that size (Win2D's `DrawBounds` of a
    /// layout in a 4096 square).
    fn glyph(&self, family: &str, glyph: &str, size: PetGlyphSize, weight: u16) -> Option<(IDWriteTextLayout, f64, f64, f64, f64, f64)> {
        let probe = |size: f64| -> Option<(IDWriteTextLayout, f64, f64, f64, f64, f64)> {
            let format = self.format(family, size, weight, 1, false).ok()?;
            let words: Vec<u16> = glyph.encode_utf16().collect();
            // SAFETY: DirectWrite calls on the objects it returned.
            unsafe {
                let layout = self.factory.CreateTextLayout(&words, &format, WIDE as f32, WIDE as f32).ok()?;
                let over = layout.GetOverhangMetrics().ok()?;
                let (left, top) = (-f64::from(over.left), -f64::from(over.top));
                let (right, bottom) = (WIDE + f64::from(over.right), WIDE + f64::from(over.bottom));
                Some((layout, left, top, right - left, bottom - top, size))
            }
        };
        match size {
            PetGlyphSize::Font(size) => probe(size),
            PetGlyphSize::Ink(tall) => {
                let (_, _, _, _, height, _) = probe(32.0)?;
                if height <= 0.0 {
                    return None;
                }
                probe(32.0 * tall / height)
            }
        }
    }
}

/// The words measured as this window draws them: DirectWrite's layouts over the formats it draws with.
pub struct DirectWriteMeasure(Arc<Text>);

impl PetMeasure for DirectWriteMeasure {
    fn lay(&self, text: &PetText) -> PetLayout {
        let Ok((layout, words)) = self.0.layout(text) else { return mighty_platform::pet::ApproximateMeasure.lay(text) };
        let mut metrics = DWRITE_TEXT_METRICS::default();
        let mut count = 0u32;
        // SAFETY: out-parameters that live across the calls; the second call has room for every line the first counted.
        unsafe {
            let _ = layout.GetMetrics(&mut metrics);
            let _ = layout.GetLineMetrics(None, &mut count);
            let mut lines = vec![DWRITE_LINE_METRICS::default(); count as usize];
            let _ = layout.GetLineMetrics(Some(&mut lines), &mut count);
            // Each line's UTF-16 length as characters.
            let mut chars = Vec::with_capacity(lines.len());
            let mut at = 0usize;
            for line in &lines {
                let end = (at + line.length as usize).min(words.len());
                chars.push(char::decode_utf16(words[at..end].iter().copied()).count());
                at = end;
            }
            PetLayout {
                width: f64::from(metrics.width),
                height: f64::from(metrics.height),
                trimmed: lines.last().is_some_and(|l| l.isTrimmed.as_bool()),
                line_chars: chars,
            }
        }
    }
}

// ── The window's shared state ──

enum Command {
    Draw { scene: Box<PetScene>, show: bool },
    Place(i32, i32),
    Close,
}

/// The window's event channel: the sender its procedure keeps, the receiver until `events` hands it over.
type Events = (mpsc::UnboundedSender<PetEvent>, Option<mpsc::UnboundedReceiver<PetEvent>>);

struct Shared {
    hwnd: AtomicIsize,
    thread: u32,
    commands: Mutex<VecDeque<Command>>,
    events: Mutex<Option<Events>>,
    closed: AtomicBool,
    capture: Mutex<PetCapture>,
    /// A caller asked for the window's picture (the smoke): from then on each frame is copied for it (review M3).
    capture_wanted: AtomicBool,
    /// Frames drawn so far.
    frames: AtomicU64,
    cursor: Mutex<PetCursor>,
    /// The scale and origin of the scene last drawn (bits of an f64; the origin in pixels).
    scale: AtomicU64,
    left: AtomicIsize,
    top: AtomicIsize,
    text: Arc<Text>,
}

impl Shared {
    fn hwnd(&self) -> HWND {
        HWND(self.hwnd.load(Ordering::SeqCst) as *mut c_void)
    }
    fn emit(&self, event: PetEvent) {
        if let Some((sender, _)) = lock(&self.events).as_ref() {
            let _ = sender.send(event);
        }
    }
    fn scale(&self) -> f64 {
        f64::from_bits(self.scale.load(Ordering::SeqCst))
    }
    fn post(&self, command: Command) -> Result<()> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(PlatformError::Refused("the pet window is closed".into()));
        }
        lock(&self.commands).push_back(command);
        // SAFETY: a message posted to the window this state names; a window already gone just loses it.
        unsafe { PostMessageW(Some(self.hwnd()), WM_COMMANDS, WPARAM(0), LPARAM(0)) }.map_err(|e| failed("PostMessageW", e))
    }
    fn open(&self) -> Result<()> {
        if self.closed.load(Ordering::SeqCst) { Err(PlatformError::Refused("the pet window is closed".into())) } else { Ok(()) }
    }
}

// ── The window thread ──

/// What the window procedure keeps on its thread: the DIB, its renderer, the press and the DPI guard.
struct Window {
    hwnd: HWND,
    shared: Arc<Shared>,
    dc: HDC,
    bitmap: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u8,
    width: i32,
    height: i32,
    renderer: Option<Renderer>,
    last: Option<Box<PetScene>>,
    shown: bool,
    pressed: bool,
    refused: u32,
    /// A show asked for while its frame was refused: shown with the next frame the system takes (review M1).
    pending_show: bool,
}

thread_local! {
    static WINDOW: RefCell<Option<Window>> = const { RefCell::new(None) };
    /// Inside a draw a change of display can arrive (C#'s `scaling` guard): it is noted and the scale asked once the draw is done.
    static DRAWING: Cell<bool> = const { Cell::new(false) };
    /// The DPI a `WM_DPICHANGED` that arrived while the window was busy carried (its `wparam`, as the free path reads it), 0 for none.
    static MISSED_DPI: Cell<u32> = const { Cell::new(0) };
}

fn instance() -> HINSTANCE {
    // SAFETY: the module handle of this process's executable.
    HINSTANCE(unsafe { GetModuleHandleW(std::ptr::null()) })
}

fn class_name() -> String {
    let mut random = [0u8; 16];
    let _ = getrandom::fill(&mut random);
    format!("MightyCompanion-{}", random.iter().map(|b| format!("{b:02x}")).collect::<String>())
}

/// A new pet window (not shown until its first `show`).
pub fn open() -> Result<Box<dyn PetWindow>> {
    let text = Arc::new(Text::new()?);
    let (ready, made) = std::sync::mpsc::channel::<Result<Arc<Shared>>>();
    let thread = std::thread::Builder::new().name("mighty-pet".into()).spawn(move || run(text, ready)).map_err(PlatformError::Io)?;
    let shared = made.recv().map_err(|_| PlatformError::Refused("the pet window's thread ended".into()))??;
    Ok(Box::new(WindowsPet { shared, thread: Some(thread) }))
}

fn run(text: Arc<Text>, ready: std::sync::mpsc::Sender<Result<Arc<Shared>>>) {
    // SAFETY: Win32 window creation on this thread, which then pumps its messages until the window is destroyed.
    unsafe {
        let _ = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let name = HSTRING::from(class_name());
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_DBLCLKS,
            lpfnWndProc: Some(procedure),
            hInstance: instance(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: PCWSTR(name.as_ptr()),
            ..Default::default()
        };
        if RegisterClassExW(&class) == 0 {
            let _ = ready.send(Err(PlatformError::Io(std::io::Error::last_os_error())));
            return;
        }
        let style = WINDOW_EX_STYLE(WS_EX_NOACTIVATE.0 | WS_EX_LAYERED.0 | WS_EX_TOOLWINDOW.0);
        let hwnd = match CreateWindowExW(
            style,
            PCWSTR(name.as_ptr()),
            &HSTRING::from("Mighty Code Companion"),
            WS_POPUP,
            0,
            0,
            1,
            1,
            None,
            None,
            Some(instance()),
            None,
        ) {
            Ok(hwnd) => hwnd,
            Err(error) => {
                let _ = UnregisterClassW(PCWSTR(name.as_ptr()), Some(instance()));
                let _ = ready.send(Err(failed("CreateWindowExW", error)));
                return;
            }
        };
        let dc = CreateCompatibleDC(None);
        if dc.is_invalid() {
            let _ = DestroyWindow(hwnd);
            let _ = UnregisterClassW(PCWSTR(name.as_ptr()), Some(instance()));
            let _ = ready.send(Err(PlatformError::Io(std::io::Error::last_os_error())));
            return;
        }
        let shared = Arc::new(Shared {
            hwnd: AtomicIsize::new(hwnd.0 as isize),
            thread: windows_sys::Win32::System::Threading::GetCurrentThreadId(),
            commands: Mutex::default(),
            events: Mutex::new(Some({
                let (sender, receiver) = mpsc::unbounded_channel();
                (sender, Some(receiver))
            })),
            closed: AtomicBool::new(false),
            capture: Mutex::default(),
            capture_wanted: AtomicBool::new(false),
            frames: AtomicU64::new(0),
            cursor: Mutex::default(),
            scale: AtomicU64::new(1f64.to_bits()),
            left: AtomicIsize::new(0),
            top: AtomicIsize::new(0),
            text,
        });
        WINDOW.with(|window| {
            *window.borrow_mut() = Some(Window {
                hwnd,
                shared: shared.clone(),
                dc,
                bitmap: HBITMAP::default(),
                old: HGDIOBJ::default(),
                bits: std::ptr::null_mut(),
                width: 0,
                height: 0,
                renderer: None,
                last: None,
                shown: false,
                pressed: false,
                refused: 0,
                pending_show: false,
            })
        });
        let _ = ready.send(Ok(shared.clone()));
        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        WINDOW.with(|window| {
            if let Some(window) = window.borrow_mut().take() {
                window.release();
            }
        });
        let _ = UnregisterClassW(PCWSTR(name.as_ptr()), Some(instance()));
        shared.closed.store(true, Ordering::SeqCst);
        *lock(&shared.events) = None;
    }
}

impl Window {
    fn release(mut self) {
        // Direct2D's target, bound to the memory DC, goes before the DC and its DIB.
        self.renderer = None;
        // SAFETY: the GDI objects this window made, each released once.
        unsafe {
            if !self.bitmap.is_invalid() {
                SelectObject(self.dc, self.old);
                let _ = DeleteObject(self.bitmap.into());
            }
            if !self.dc.is_invalid() {
                let _ = DeleteDC(self.dc);
            }
        }
    }

    /// C# `Allocate`: a DIB section of exactly the window's pixels.
    fn allocate(&mut self, width: i32, height: i32) -> Result<()> {
        if width <= 0 || height <= 0 || width.checked_mul(height).and_then(|p| p.checked_mul(4)).is_none() {
            return Err(PlatformError::Invalid("the pet window is too large".into()));
        }
        if width == self.width && height == self.height && !self.bits.is_null() {
            return Ok(());
        }
        // SAFETY: GDI calls on this window's memory DC; the old bitmap is put back before the new one is selected.
        unsafe {
            if !self.bitmap.is_invalid() {
                SelectObject(self.dc, self.old);
                let _ = DeleteObject(self.bitmap.into());
                self.bitmap = HBITMAP::default();
                self.bits = std::ptr::null_mut();
            }
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width,
                    biHeight: -height,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    biSizeImage: (width * height * 4) as u32,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut c_void = std::ptr::null_mut();
            let bitmap =
                CreateDIBSection(Some(self.dc), &info, DIB_RGB_COLORS, &mut bits, None, 0).map_err(|e| failed("CreateDIBSection", e))?;
            self.old = SelectObject(self.dc, bitmap.into());
            self.bitmap = bitmap;
            self.bits = bits as *mut u8;
        }
        self.width = width;
        self.height = height;
        Ok(())
    }

    /// Draws `scene` into the DIB and puts it on screen at its place and size, together (C# `Draw`).
    fn draw(&mut self, scene: Box<PetScene>, show: bool) {
        let show = show || self.pending_show;
        self.pending_show = show && !self.shown;
        let (width, height) = scene.pixel_size();
        let (Ok(width), Ok(height)) = (i32::try_from(width), i32::try_from(height)) else {
            self.refuse("the pet window is too large".into());
            return;
        };
        if self.allocate(width, height).is_err() {
            self.refuse("CreateDIBSection".into());
            return;
        }
        let scale = if scene.scale > 0.0 { scene.scale } else { 1.0 };
        self.shared.scale.store(scale.to_bits(), Ordering::SeqCst);
        let (left, top) = (scene.origin.x as i32, scene.origin.y as i32);
        self.shared.left.store(left as isize, Ordering::SeqCst);
        self.shared.top.store(top as isize, Ordering::SeqCst);
        if self.renderer.is_none() {
            self.renderer = Renderer::new(self.shared.text.clone()).ok();
        }
        let drawn = match self.renderer.as_mut() {
            Some(renderer) => renderer.render(self.dc, width, height, &scene),
            None => Err(PlatformError::Refused("Direct2D".into())),
        };
        if drawn.is_err() {
            // A device lost or a target that would not bind: made afresh on the next frame.
            self.renderer = None;
        }
        // The count is kept with the pixels it numbers, and a capture hands that pair back: it never pairs this frame's count with the
        // last frame's pixels (a smoke waiting for the next frame read the old one, drawn at the old size, under load).
        let mut kept = lock(&self.shared.capture);
        let frames = self.shared.frames.fetch_add(1, Ordering::SeqCst) + 1;
        if self.shared.capture_wanted.load(Ordering::SeqCst) && !self.bits.is_null() && width == self.width && height == self.height {
            // SAFETY: the DIB `allocate` made for exactly `width × height` (checked above, its product checked there) is selected and
            // its bits are that many 4-byte pixels; GDI is flushed first.
            unsafe {
                let _ = GdiFlush();
                let pixels = std::slice::from_raw_parts(self.bits, width as usize * height as usize * 4).to_vec();
                *kept = PetCapture { width: width as u32, height: height as u32, bgra: pixels, frames };
            }
        }
        drop(kept);
        let destination = POINT { x: left, y: top };
        let size = SIZE { cx: width, cy: height };
        let source = POINT::default();
        let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
        DRAWING.with(|d| d.set(true));
        // SAFETY: this window and its memory DC with the DIB selected; the structures live across the call.
        let shown = unsafe {
            UpdateLayeredWindow(
                self.hwnd,
                None,
                Some(&destination),
                Some(&size),
                Some(self.dc),
                Some(&source),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            )
        };
        DRAWING.with(|d| d.set(false));
        self.last = Some(scene);
        if let Err(error) = shown.map_err(|e| failed("UpdateLayeredWindow", e)).and(drawn) {
            // A frame the system does not take is drawn again on the next tick; only a window that stays refused ends the pet.
            self.refuse(error.to_string());
            return;
        }
        self.refused = 0;
        // SAFETY: timer and z-order calls on this thread's own window.
        unsafe {
            let _ = KillTimer(Some(self.hwnd), RETRY_TIMER);
            if show && !self.shown {
                self.shown = true;
                let _ = ShowWindow(self.hwnd, SW_SHOWNA);
            }
            self.pending_show = false;
            let _ = SetWindowPos(self.hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
        self.missed_dpi();
    }

    /// A change of display noted while the window was busy (inside a draw or a move): the DPI the message said, reported once when its
    /// scale is not the one drawn at. Asking `GetDpiForWindow` instead would lose a change the window's own display does not show yet
    /// (a `WM_DPICHANGED` sent ahead of the move, the smoke's 150% and 200%).
    fn missed_dpi(&self) {
        let dpi = MISSED_DPI.with(|m| m.replace(0));
        if dpi != 0 {
            let standing = (f64::from(dpi) / 96.0).clamp(1.0, 4.0);
            if (standing - self.shared.scale()).abs() > f64::EPSILON {
                self.shared.emit(PetEvent::DpiChanged { scale: standing });
            }
        }
    }

    fn refuse(&mut self, message: String) {
        self.refused += 1;
        if self.refused >= REFUSED_FRAMES {
            self.shared.emit(PetEvent::Failed { message });
            return;
        }
        // SAFETY: a timer on this thread's own window.
        unsafe { SetTimer(Some(self.hwnd), RETRY_TIMER, 83, None) };
    }

    fn commands(&mut self) {
        let commands: Vec<Command> = lock(&self.shared.commands).drain(..).collect();
        let mut draw: Option<Box<PetScene>> = None;
        let mut show = false;
        for command in commands {
            match command {
                Command::Draw { scene, show: first } => {
                    show |= first;
                    draw = Some(scene);
                }
                Command::Place(x, y) => {
                    if let Some(scene) = draw.take() {
                        self.draw(scene, std::mem::take(&mut show));
                    }
                    self.shared.left.store(x as isize, Ordering::SeqCst);
                    self.shared.top.store(y as isize, Ordering::SeqCst);
                    // SAFETY: this thread's own window, moved without activation.
                    unsafe {
                        let _ = SetWindowPos(self.hwnd, Some(HWND_TOPMOST), x, y, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE);
                    }
                }
                Command::Close => {
                    self.shared.closed.store(true, Ordering::SeqCst);
                    // SAFETY: this thread's own window. Its WM_DESTROY arrives while this window is borrowed (the procedure posts the
                    // quit there too); the quit posted here ends the loop either way (review H1).
                    unsafe {
                        let _ = DestroyWindow(self.hwnd);
                        PostQuitMessage(0);
                    }
                    return;
                }
            }
        }
        if let Some(scene) = draw {
            self.draw(scene, show);
        }
        self.missed_dpi();
    }

    /// The pointer as the window reports it: in the window's DIPs and on the screen, from the client point of a message.
    fn pointer(&self, lparam: LPARAM) -> PetPointer {
        let (x, y) = (i32::from(lparam.0 as u16 as i16), i32::from((lparam.0 >> 16) as u16 as i16));
        let mut rect = RECT::default();
        // SAFETY: an out-parameter on this thread's own window.
        unsafe {
            let _ = GetWindowRect(self.hwnd, &mut rect);
        }
        let scale = self.shared.scale();
        PetPointer {
            window: LogicalPoint { x: f64::from(x) / scale, y: f64::from(y) / scale },
            screen: LogicalPoint { x: f64::from(rect.left + x), y: f64::from(rect.top + y) },
        }
    }

    fn message(&mut self, message: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
        match message {
            WM_MOUSEACTIVATE => Some(LRESULT(MA_NOACTIVATE as isize)),
            WM_SETCURSOR if (lparam.0 & 0xFFFF) as u32 == HTCLIENT => {
                let cursor = *lock(&self.shared.cursor);
                let shape = match cursor {
                    PetCursor::Arrow => IDC_ARROW,
                    PetCursor::SizeWestEast => IDC_SIZEWE,
                    PetCursor::SizeNorthSouth => IDC_SIZENS,
                    PetCursor::SizeNorthWestSouthEast => IDC_SIZENWSE,
                    PetCursor::SizeNorthEastSouthWest => IDC_SIZENESW,
                };
                // SAFETY: a system cursor set while the pointer is over this window.
                unsafe {
                    SetCursor(Some(LoadCursorW(None, shape).unwrap_or(HCURSOR::default())));
                }
                Some(LRESULT(1))
            }
            WM_LBUTTONDOWN => {
                self.pressed = true;
                // SAFETY: this thread's own window takes the pointer until the button goes up.
                unsafe { SetCapture(self.hwnd.0) };
                self.shared.emit(PetEvent::Pressed(self.pointer(lparam)));
                Some(LRESULT(0))
            }
            WM_MOUSEMOVE => {
                self.shared.emit(PetEvent::Moved(self.pointer(lparam)));
                Some(LRESULT(0))
            }
            WM_LBUTTONUP => {
                let pointer = self.pointer(lparam);
                let was = std::mem::replace(&mut self.pressed, false);
                if was {
                    // SAFETY: the pointer this thread's window took on the press.
                    unsafe { ReleaseCapture() };
                }
                self.shared.emit(PetEvent::Released(pointer));
                Some(LRESULT(0))
            }
            WM_LBUTTONDBLCLK => {
                self.shared.emit(PetEvent::DoubleClicked(self.pointer(lparam)));
                Some(LRESULT(0))
            }
            WM_RBUTTONUP => {
                self.shared.emit(PetEvent::RightClicked(self.pointer(lparam)));
                Some(LRESULT(0))
            }
            WM_CANCELMODE | WM_CAPTURECHANGED => {
                if std::mem::replace(&mut self.pressed, false) {
                    if message == WM_CANCELMODE {
                        // SAFETY: the pointer this thread's window took.
                        unsafe { ReleaseCapture() };
                    }
                    self.shared.emit(PetEvent::Cancelled);
                }
                Some(LRESULT(0))
            }
            WM_MOUSEWHEEL => {
                let screen = LogicalPoint { x: f64::from(lparam.0 as u16 as i16), y: f64::from((lparam.0 >> 16) as u16 as i16) };
                let mut rect = RECT::default();
                // SAFETY: an out-parameter on this thread's own window.
                unsafe {
                    let _ = GetWindowRect(self.hwnd, &mut rect);
                }
                let scale = self.shared.scale();
                let window = LogicalPoint { x: (screen.x - f64::from(rect.left)) / scale, y: (screen.y - f64::from(rect.top)) / scale };
                let delta = i32::from((wparam.0 >> 16) as u16 as i16);
                self.shared.emit(PetEvent::Wheel { pointer: PetPointer { window, screen }, delta });
                Some(LRESULT(0))
            }
            WM_DISPLAYCHANGE => {
                self.shared.emit(PetEvent::DisplayChanged);
                Some(LRESULT(0))
            }
            WM_DPICHANGED => {
                let scale = (f64::from((wparam.0 & 0xFFFF) as u32) / 96.0).clamp(1.0, 4.0);
                self.shared.emit(PetEvent::DpiChanged { scale });
                Some(LRESULT(0))
            }
            WM_TIMER if wparam.0 == RETRY_TIMER => {
                if let Some(scene) = self.last.take() {
                    self.draw(scene, false);
                }
                Some(LRESULT(0))
            }
            WM_COMMANDS => {
                self.commands();
                Some(LRESULT(0))
            }
            WM_DESTROY => {
                // SAFETY: ends this thread's message loop.
                unsafe { PostQuitMessage(0) };
                Some(LRESULT(0))
            }
            _ => None,
        }
    }
}

/// The window procedure: no panic crosses it (C#: no managed exception crosses the native callback). A press that failed part-way
/// gives the pointer back. A message that arrives while the window is busy drawing (a change of display inside `UpdateLayeredWindow`)
/// is noted for after the draw, never handled inside it.
unsafe extern "system" fn procedure(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let handled = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        WINDOW.with(|window| match window.try_borrow_mut() {
            Ok(mut window) => window.as_mut().filter(|w| w.hwnd == hwnd).and_then(|w| w.message(message, wparam, lparam)),
            Err(_) => {
                // The window is busy (a draw, a move, its destruction): a change of display is noted for after it, never handled
                // inside it (review M2), and a destruction still ends the loop (review H1).
                if message == WM_DPICHANGED {
                    MISSED_DPI.with(|m| m.set((wparam.0 & 0xFFFF) as u32));
                    return Some(LRESULT(0));
                }
                if message == WM_DESTROY {
                    // SAFETY: ends this thread's message loop.
                    unsafe { PostQuitMessage(0) };
                    return Some(LRESULT(0));
                }
                None
            }
        })
    }));
    match handled {
        Ok(Some(result)) => result,
        Ok(None) => {
            // SAFETY: the default procedure for a message this window does not handle.
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }
        Err(_) => {
            WINDOW.with(|window| {
                if let Ok(mut window) = window.try_borrow_mut()
                    && let Some(window) = window.as_mut()
                    && std::mem::replace(&mut window.pressed, false)
                {
                    // SAFETY: only this window's own press is given back.
                    unsafe { ReleaseCapture() };
                }
            });
            // SAFETY: as above.
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }
    }
}

// ── Direct2D ──

/// The Direct2D side of a window: its factory, a DC render target bound to the DIB, and the atlas's cells as device bitmaps.
struct Renderer {
    factory: ID2D1Factory,
    target: Option<(ID2D1DCRenderTarget, D2D1_RENDER_TARGET_TYPE)>,
    text: Arc<Text>,
    round: Option<ID2D1StrokeStyle>,
    atlas: Option<String>,
    cells: HashMap<(u32, u32), ID2D1Bitmap>,
    brushes: HashMap<u32, ID2D1SolidColorBrush>,
}

fn rect_f(rect: &PetRect) -> D2D_RECT_F {
    D2D_RECT_F { left: rect.x as f32, top: rect.y as f32, right: rect.right() as f32, bottom: rect.bottom() as f32 }
}

fn vector(point: PetPoint) -> Vector2 {
    Vector2 { X: point.x as f32, Y: point.y as f32 }
}

impl Renderer {
    fn new(text: Arc<Text>) -> Result<Renderer> {
        // SAFETY: a single-threaded factory used only on this window's thread.
        let factory: ID2D1Factory =
            unsafe { D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None) }.map_err(|e| failed("Direct2D", e))?;
        Ok(Renderer { factory, target: None, text, round: None, atlas: None, cells: HashMap::new(), brushes: HashMap::new() })
    }

    /// The DC render target: the system's renderer, the software one where it cannot make one.
    fn target(&mut self) -> Result<ID2D1DCRenderTarget> {
        if let Some((target, _)) = &self.target {
            return Ok(target.clone());
        }
        let mut last = None;
        for kind in [D2D1_RENDER_TARGET_TYPE_DEFAULT, D2D1_RENDER_TARGET_TYPE_SOFTWARE] {
            let properties = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: kind,
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                dpiX: 96.0,
                dpiY: 96.0,
                usage: D2D1_RENDER_TARGET_USAGE_NONE,
                minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
            };
            // SAFETY: a render target made from this window's factory with properties that live across the call.
            match unsafe { self.factory.CreateDCRenderTarget(&properties) } {
                Ok(target) => {
                    self.target = Some((target.clone(), kind));
                    self.cells.clear();
                    self.brushes.clear();
                    return Ok(target);
                }
                Err(error) => last = Some(error),
            }
        }
        Err(failed("Direct2D", last.unwrap_or_else(windows::core::Error::empty)))
    }

    fn render(&mut self, dc: HDC, width: i32, height: i32, scene: &PetScene) -> Result<()> {
        let target = self.target()?;
        let bounds = RECT { left: 0, top: 0, right: width, bottom: height };
        let scale = if scene.scale > 0.0 { scene.scale as f32 } else { 1.0 };
        if self.atlas.as_deref() != scene.atlas.as_ref().map(|a| a.key.as_str()) {
            self.atlas = scene.atlas.as_ref().map(|a| a.key.clone());
            self.cells.clear();
        }
        // SAFETY: Direct2D calls on this thread's target bound to the window's memory DC, between BeginDraw and EndDraw.
        unsafe {
            if let Err(error) = target.BindDC(dc, &bounds) {
                // Hardware that cannot bind: the software renderer next.
                let software = self.target.as_ref().is_some_and(|(_, kind)| *kind == D2D1_RENDER_TARGET_TYPE_SOFTWARE);
                self.target = None;
                if !software {
                    let properties = D2D1_RENDER_TARGET_PROPERTIES {
                        r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
                        pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                        dpiX: 96.0,
                        dpiY: 96.0,
                        usage: D2D1_RENDER_TARGET_USAGE_NONE,
                        minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
                    };
                    if let Ok(target) = self.factory.CreateDCRenderTarget(&properties) {
                        self.target = Some((target, D2D1_RENDER_TARGET_TYPE_SOFTWARE));
                        self.cells.clear();
                        self.brushes.clear();
                        return self.render(dc, width, height, scene);
                    }
                }
                return Err(failed("BindDC", error));
            }
            target.BeginDraw();
            target.Clear(Some(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }));
            target.SetTransform(&Matrix3x2 { M11: scale, M12: 0.0, M21: 0.0, M22: scale, M31: 0.0, M32: 0.0 });
            target.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            let base: ID2D1RenderTarget = target.cast().map_err(|e| failed("Direct2D", e))?;
            self.items(&base, &scene.items, scene.atlas.as_ref());
            let ended = target.EndDraw(None, None);
            if ended.is_err() {
                self.target = None;
            }
            ended.map_err(|e| failed("EndDraw", e))
        }
    }

    fn brush(&mut self, target: &ID2D1RenderTarget, color: PetColor) -> Option<ID2D1SolidColorBrush> {
        let key = u32::from_le_bytes([color.r, color.g, color.b, color.a]);
        if let Some(brush) = self.brushes.get(&key) {
            return Some(brush.clone());
        }
        let value = D2D1_COLOR_F {
            r: f32::from(color.r) / 255.0,
            g: f32::from(color.g) / 255.0,
            b: f32::from(color.b) / 255.0,
            a: f32::from(color.a) / 255.0,
        };
        // SAFETY: a brush of this thread's target, from a colour that lives across the call.
        let brush = unsafe { target.CreateSolidColorBrush(&value, None) }.ok()?;
        self.brushes.insert(key, brush.clone());
        Some(brush)
    }

    fn round(&mut self) -> Option<ID2D1StrokeStyle> {
        if self.round.is_none() {
            let properties = D2D1_STROKE_STYLE_PROPERTIES {
                startCap: D2D1_CAP_STYLE_ROUND,
                endCap: D2D1_CAP_STYLE_ROUND,
                dashCap: D2D1_CAP_STYLE_ROUND,
                lineJoin: D2D1_LINE_JOIN_ROUND,
                miterLimit: 10.0,
                dashStyle: D2D1_DASH_STYLE_SOLID,
                dashOffset: 0.0,
            };
            // SAFETY: a stroke style of this window's factory.
            self.round = unsafe { self.factory.CreateStrokeStyle(&properties, None) }.ok();
        }
        self.round.clone()
    }

    fn cell(&mut self, target: &ID2D1RenderTarget, atlas: &PetBitmap, cell: &PetRect) -> Option<ID2D1Bitmap> {
        let (x, y, w, h) = (cell.x as u32, cell.y as u32, cell.width as u32, cell.height as u32);
        if w == 0 || h == 0 || x.checked_add(w)? > atlas.width || y.checked_add(h)? > atlas.height {
            return None;
        }
        let stride = atlas.width.checked_mul(4)? as usize;
        let last = (y + h - 1) as usize * stride + (x + w) as usize * 4;
        if atlas.bgra.len() < last || atlas.bgra.len() != stride.checked_mul(atlas.height as usize)? {
            return None;
        }
        if let Some(bitmap) = self.cells.get(&(x, y)) {
            return Some(bitmap.clone());
        }
        let pitch = atlas.width * 4;
        let offset = y as usize * stride + x as usize * 4;
        let properties = D2D1_BITMAP_PROPERTIES {
            pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
            dpiX: 96.0,
            dpiY: 96.0,
        };
        // SAFETY: the cell's first pixel inside the atlas, read row by row at the atlas's pitch for `h` rows that the bound check keeps
        // inside the buffer.
        let bitmap = unsafe {
            target.CreateBitmap(
                D2D_SIZE_U { width: w, height: h },
                Some(atlas.bgra[offset..].as_ptr() as *const c_void),
                pitch,
                &properties,
            )
        }
        .ok()?;
        self.cells.insert((x, y), bitmap.clone());
        Some(bitmap)
    }

    fn items(&mut self, target: &ID2D1RenderTarget, items: &[PetItem], atlas: Option<&PetBitmap>) {
        for item in items {
            self.item(target, item, atlas);
        }
    }

    fn item(&mut self, target: &ID2D1RenderTarget, item: &PetItem, atlas: Option<&PetBitmap>) {
        // SAFETY: Direct2D drawing calls on this thread's target between its BeginDraw and EndDraw, with objects it made.
        unsafe {
            match item {
                PetItem::Fill { rect, radius, color } => {
                    let Some(brush) = self.brush(target, *color) else { return };
                    if *radius > 0.0 {
                        let rounded = D2D1_ROUNDED_RECT { rect: rect_f(rect), radiusX: *radius as f32, radiusY: *radius as f32 };
                        target.FillRoundedRectangle(&rounded, &brush);
                    } else {
                        target.FillRectangle(&rect_f(rect), &brush);
                    }
                }
                PetItem::Stroke { rect, radius, color, width } => {
                    let Some(brush) = self.brush(target, *color) else { return };
                    let rounded = D2D1_ROUNDED_RECT { rect: rect_f(rect), radiusX: *radius as f32, radiusY: *radius as f32 };
                    target.DrawRoundedRectangle(&rounded, &brush, *width as f32, None);
                }
                PetItem::Ellipse { center, radius_x, radius_y, color, stroke } => {
                    let Some(brush) = self.brush(target, *color) else { return };
                    let ellipse = D2D1_ELLIPSE { point: vector(*center), radiusX: *radius_x as f32, radiusY: *radius_y as f32 };
                    match stroke {
                        Some(width) => target.DrawEllipse(&ellipse, &brush, *width as f32, None),
                        None => target.FillEllipse(&ellipse, &brush),
                    }
                }
                PetItem::Line { from, to, color, width, round } => {
                    let Some(brush) = self.brush(target, *color) else { return };
                    let style = if *round { self.round() } else { None };
                    target.DrawLine(vector(*from), vector(*to), &brush, *width as f32, style.as_ref());
                }
                PetItem::Arc { center, radius, start, sweep, color, width } => {
                    let Some(brush) = self.brush(target, *color) else { return };
                    let Ok(path) = self.factory.CreatePathGeometry() else { return };
                    let Ok(sink) = path.Open() else { return };
                    let at =
                        |angle: f64| Vector2 { X: (center.x + radius * angle.cos()) as f32, Y: (center.y + radius * angle.sin()) as f32 };
                    sink.BeginFigure(at(*start), D2D1_FIGURE_BEGIN_HOLLOW);
                    let arc = D2D1_ARC_SEGMENT {
                        point: at(start + sweep),
                        size: windows::Win32::Graphics::Direct2D::Common::D2D_SIZE_F { width: *radius as f32, height: *radius as f32 },
                        rotationAngle: 0.0,
                        sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
                        arcSize: if sweep.abs() > std::f64::consts::PI { D2D1_ARC_SIZE_LARGE } else { D2D1_ARC_SIZE_SMALL },
                    };
                    sink.AddArc(&arc);
                    sink.EndFigure(D2D1_FIGURE_END_OPEN);
                    let _ = sink.Close();
                    let style = self.round();
                    target.DrawGeometry(&path, &brush, *width as f32, style.as_ref());
                }
                PetItem::Path { start, segments, color } => {
                    let Some(brush) = self.brush(target, *color) else { return };
                    let Ok(path) = self.factory.CreatePathGeometry() else { return };
                    let Ok(sink) = path.Open() else { return };
                    sink.BeginFigure(vector(*start), D2D1_FIGURE_BEGIN_FILLED);
                    for segment in segments {
                        match segment {
                            PetSegment::Line { to } => sink.AddLine(vector(*to)),
                            PetSegment::Cubic { first, second, to } => sink.AddBezier(&D2D1_BEZIER_SEGMENT {
                                point1: vector(*first),
                                point2: vector(*second),
                                point3: vector(*to),
                            }),
                        }
                    }
                    sink.EndFigure(D2D1_FIGURE_END_CLOSED);
                    let _ = sink.Close();
                    target.FillGeometry(&path, &brush, None);
                }
                PetItem::Text(text) => self.text(target, text),
                PetItem::Glyph { family, glyph, center, size, weight, color, over } => {
                    let Some((layout, left, top, width, height, font)) = self.text.glyph(family, glyph, *size, *weight) else { return };
                    let origin = Vector2 { X: (center.x - left - width / 2.0) as f32, Y: (center.y - top - height / 2.0) as f32 };
                    let Some(brush) = self.brush(target, *color) else { return };
                    target.DrawTextLayout(origin, &layout, &brush, D2D1_DRAW_TEXT_OPTIONS_NONE);
                    if let Some((second, ink)) = over
                        && let Some((layout, _, _, _, _, _)) = self.text.glyph(family, second, PetGlyphSize::Font(font), *weight)
                        && let Some(brush) = self.brush(target, *ink)
                    {
                        target.DrawTextLayout(origin, &layout, &brush, D2D1_DRAW_TEXT_OPTIONS_NONE);
                    }
                }
                PetItem::Sprite { cell, dest, opacity } => {
                    let Some(atlas) = atlas else { return };
                    let Some(bitmap) = self.cell(target, atlas, cell) else { return };
                    match target.cast::<ID2D1DeviceContext>() {
                        Ok(context) => context.DrawBitmap(
                            &bitmap,
                            Some(&rect_f(dest)),
                            *opacity as f32,
                            D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC,
                            None,
                            None,
                        ),
                        Err(_) => {
                            target.DrawBitmap(&bitmap, Some(&rect_f(dest)), *opacity as f32, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None)
                        }
                    }
                }
                PetItem::Clip { rect, items } => {
                    target.PushAxisAlignedClip(&rect_f(rect), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
                    self.items(target, items, atlas);
                    target.PopAxisAlignedClip();
                }
                PetItem::Group { opacity, items, .. } => {
                    if *opacity >= 1.0 {
                        self.items(target, items, atlas);
                        return;
                    }
                    let Ok(layer) = target.CreateLayer(None) else { return };
                    let parameters = D2D1_LAYER_PARAMETERS {
                        contentBounds: D2D_RECT_F { left: -1e9, top: -1e9, right: 1e9, bottom: 1e9 },
                        geometricMask: std::mem::ManuallyDrop::new(None),
                        maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
                        maskTransform: Matrix3x2 { M11: 1.0, M12: 0.0, M21: 0.0, M22: 1.0, M31: 0.0, M32: 0.0 },
                        opacity: *opacity as f32,
                        opacityBrush: std::mem::ManuallyDrop::new(None),
                        layerOptions: D2D1_LAYER_OPTIONS_NONE,
                    };
                    target.PushLayer(&parameters, &layer);
                    self.items(target, items, atlas);
                    target.PopLayer();
                }
            }
        }
    }

    fn text(&mut self, target: &ID2D1RenderTarget, text: &PetText) {
        let Ok((layout, _)) = self.text.layout(text) else { return };
        let Some(first) = text.runs.first() else { return };
        let Some(brush) = self.brush(target, first.color) else { return };
        // SAFETY: drawing effects over ranges inside the layout's words, then the layout drawn on this thread's target.
        unsafe {
            let mut at = 0u32;
            for run in &text.runs {
                let length = run.text.encode_utf16().count() as u32;
                if run.color != first.color
                    && let Some(ink) = self.brush(target, run.color)
                {
                    let _ = layout.SetDrawingEffect(&ink, DWRITE_TEXT_RANGE { startPosition: at, length });
                }
                at += length;
            }
            // Words keep an emoji's colours, as a TextBlock does.
            let options = if text.family == BODY || text.family == MONO {
                D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT
            } else {
                D2D1_DRAW_TEXT_OPTIONS_NONE
            };
            target.DrawTextLayout(Vector2 { X: text.rect.x as f32, Y: text.rect.y as f32 }, &layout, &brush, options);
        }
    }
}

// ── The port ──

struct WindowsPet {
    shared: Arc<Shared>,
    /// The window thread, waited for on drop: since a close ends it (the W8 H1 fix), its teardown (the DIB, the Direct2D device and
    /// the driver's thread detach, the class) must never be cut off by the process ending under it, which crashed a test process.
    thread: Option<std::thread::JoinHandle<()>>,
}

fn work_area_at(near: LogicalPoint) -> LogicalRect {
    let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    // SAFETY: the monitor nearest a point and an out-structure sized for it.
    let found =
        unsafe { GetMonitorInfoW(MonitorFromPoint(POINT { x: near.x as i32, y: near.y as i32 }, MONITOR_DEFAULTTONEAREST), &mut info) };
    if !found.as_bool() {
        return LogicalRect { x: 0.0, y: 0.0, width: 1920.0, height: 1080.0 };
    }
    let work = info.rcWork;
    LogicalRect {
        x: f64::from(work.left),
        y: f64::from(work.top),
        width: f64::from(work.right - work.left),
        height: f64::from(work.bottom - work.top),
    }
}

impl WindowsPet {
    fn client(&self, at: LogicalPoint) -> LPARAM {
        let scale = self.shared.scale();
        let (x, y) = ((at.x * scale) as i32, (at.y * scale) as i32);
        LPARAM(((y & 0xFFFF) << 16 | (x & 0xFFFF)) as isize)
    }

    fn deliver(&self, message: u32, wparam: usize, lparam: LPARAM) {
        // SAFETY: a message sent to this pet's window; its procedure runs on its own thread while this one waits.
        unsafe {
            SendMessageW(self.shared.hwnd(), message, Some(WPARAM(wparam)), Some(lparam));
        }
    }
}

impl PetWindow for WindowsPet {
    fn show(&self, scene: &PetScene) -> Result<()> {
        self.shared.post(Command::Draw { scene: Box::new(scene.clone()), show: true })
    }
    fn draw(&self, scene: &PetScene) -> Result<()> {
        self.shared.post(Command::Draw { scene: Box::new(scene.clone()), show: false })
    }
    fn place(&self, at: LogicalPoint) -> Result<()> {
        self.shared.post(Command::Place(at.x as i32, at.y as i32))
    }
    fn work_area(&self, near: LogicalPoint) -> Result<LogicalRect> {
        self.shared.open()?;
        Ok(work_area_at(near))
    }
    fn events(&self) -> Result<mpsc::UnboundedReceiver<PetEvent>> {
        self.shared.open()?;
        let mut events = lock(&self.shared.events);
        let (_, receiver) = events.as_mut().ok_or_else(|| PlatformError::Refused("the pet window is closed".into()))?;
        receiver.take().ok_or_else(|| PlatformError::Invalid("the pet window's events were taken already".into()))
    }
    fn close(&self) -> Result<()> {
        self.shared.post(Command::Close)?;
        self.shared.closed.store(true, Ordering::SeqCst);
        Ok(())
    }
    fn scale_at(&self, near: LogicalPoint) -> Result<f64> {
        self.shared.open()?;
        let (mut x, mut y) = (96u32, 96u32);
        // SAFETY: the monitor nearest a point and two out-parameters.
        unsafe {
            let monitor = MonitorFromPoint(POINT { x: near.x as i32, y: near.y as i32 }, MONITOR_DEFAULTTONEAREST);
            let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut x, &mut y);
        }
        Ok((f64::from(x) / 96.0).clamp(1.0, 4.0))
    }
    fn measure(&self) -> Arc<dyn PetMeasure> {
        Arc::new(DirectWriteMeasure(self.shared.text.clone()))
    }
    fn poll(&self) -> Result<PetPoll> {
        self.shared.open()?;
        // Down now, or pressed and let go since the last look: a short tap between two frames counts too.
        // SAFETY: reads of the desktop's pointer and keys.
        unsafe {
            let pressed = |key: i32| (GetAsyncKeyState(key) as u16 & 0x8001) != 0;
            let mut cursor = POINT::default();
            let _ = GetCursorPos(&mut cursor);
            let buttons = pressed(1) | pressed(2) | pressed(4);
            Ok(PetPoll { cursor: LogicalPoint { x: f64::from(cursor.x), y: f64::from(cursor.y) }, pressed: buttons, escape: pressed(0x1B) })
        }
    }
    fn set_cursor(&self, cursor: PetCursor) -> Result<()> {
        self.shared.open()?;
        *lock(&self.shared.cursor) = cursor;
        Ok(())
    }
    fn capture(&self) -> Result<PetCapture> {
        self.shared.open()?;
        self.shared.capture_wanted.store(true, Ordering::SeqCst);
        // The count stored with these pixels (never the counter now, which a frame drawn since may have moved past them).
        Ok(lock(&self.shared.capture).clone())
    }
    fn animations(&self) -> bool {
        let mut on = BOOL(1);
        // SAFETY: SPI_GETCLIENTAREAANIMATION writes one BOOL to the pointer it is given.
        let read = unsafe {
            SystemParametersInfoW(
                SPI_GETCLIENTAREAANIMATION,
                0,
                Some(&mut on as *mut BOOL as *mut c_void),
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
            )
        };
        read.is_err() || on.as_bool()
    }
    fn focus(&self) -> Result<PetFocus> {
        self.shared.open()?;
        let hwnd = self.shared.hwnd();
        // SAFETY: reads of window styles, the front window and the window thread's active and focus windows.
        unsafe {
            let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
            let mut info = GUITHREADINFO { cbSize: std::mem::size_of::<GUITHREADINFO>() as u32, ..Default::default() };
            let took = GetGUIThreadInfo(self.shared.thread, &mut info).is_ok() && (info.hwndActive == hwnd || info.hwndFocus == hwnd);
            Ok(PetFocus {
                non_activating: style & WS_EX_NOACTIVATE.0 != 0,
                foreground: GetForegroundWindow().0 as u64,
                own: hwnd.0 as u64,
                took_thread: took,
            })
        }
    }
    fn send(&self, input: PetInput) -> Result<()> {
        self.shared.open()?;
        match input {
            PetInput::Click(at) => {
                self.deliver(WM_LBUTTONDOWN, 1, self.client(at));
                self.deliver(WM_LBUTTONUP, 0, self.client(at));
            }
            PetInput::Press(at) => self.deliver(WM_LBUTTONDOWN, 1, self.client(at)),
            PetInput::Drag(at) => self.deliver(WM_MOUSEMOVE, 1, self.client(at)),
            PetInput::Release(at) => self.deliver(WM_LBUTTONUP, 0, self.client(at)),
            PetInput::DoubleClick(at) => self.deliver(WM_LBUTTONDBLCLK, 1, self.client(at)),
            PetInput::RightClick(at) => self.deliver(WM_RBUTTONUP, 0, self.client(at)),
            PetInput::Wheel { at, delta } => {
                let scale = self.shared.scale();
                let x = self.shared.left.load(Ordering::SeqCst) as i32 + (at.x * scale) as i32;
                let y = self.shared.top.load(Ordering::SeqCst) as i32 + (at.y * scale) as i32;
                // One message a notch, as a wheel sends them: the turn is a signed 16-bit count in the high word.
                let wparam = (delta as i16 as u16 as usize) << 16;
                self.deliver(WM_MOUSEWHEEL, wparam, LPARAM(((y & 0xFFFF) << 16 | (x & 0xFFFF)) as isize));
            }
            PetInput::Dpi(dpi) => {
                // The system copies the suggested rectangle of a sent WM_DPICHANGED to the window's thread: one must be there.
                let mut rect = RECT::default();
                // SAFETY: an out-parameter for this pet's window; the rectangle outlives the send, which waits for the procedure.
                unsafe {
                    let _ = GetWindowRect(self.shared.hwnd(), &mut rect);
                }
                self.deliver(WM_DPICHANGED, (dpi | dpi << 16) as usize, LPARAM(&rect as *const RECT as isize));
            }
        }
        Ok(())
    }
}

impl Drop for WindowsPet {
    fn drop(&mut self) {
        if !self.shared.closed.load(Ordering::SeqCst) {
            let _ = self.close();
        }
        let Some(thread) = self.thread.take().filter(|thread| thread.thread().id() != std::thread::current().id()) else { return };
        // On an async worker (the companion's reload lets its pet go there) the wait goes to a thread of its own, so no worker is held;
        // anywhere else (the quit path, a test's thread) it is waited for here, before the process can end under the teardown.
        if tokio::runtime::Handle::try_current().is_ok() {
            let _ = std::thread::Builder::new().name("mighty-pet-join".into()).spawn(move || join_bounded(thread));
        } else {
            join_bounded(thread);
        }
    }
}

/// The pet's window thread joined once it has ended, waited for at most 5 s: a thread that never got its close is left rather than waited
/// on for ever. The window has no owner and its thread waits on no other, so the wait cannot hold it up.
fn join_bounded(thread: std::thread::JoinHandle<()>) {
    let end = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !thread.is_finished() && std::time::Instant::now() < end {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    if thread.is_finished() {
        let _ = thread.join();
    }
}

#[cfg(test)]
mod tests {
    use mighty_platform::pet::{PetTextAlign, PetTextRun};

    use super::*;

    fn words(value: &str, size: f64, width: f64, lines: u32) -> PetText {
        PetText {
            rect: PetRect::new(0.0, 0.0, width, 0.0),
            bounds: PetRect::default(),
            value: value.into(),
            runs: vec![PetTextRun {
                text: value.into(),
                color: PetColor { r: 255, g: 255, b: 255, a: 255 },
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

    /// C#'s line: Segoe UI at nine tenths of its spacing sets a line of 11pt words 13 high (the header's 25 is two of them less a
    /// little), and a paragraph past its limit keeps the words of its lines less the ellipsis.
    #[test]
    fn directwrite_measures_the_lines_csharp_measures() {
        let measure = DirectWriteMeasure(Arc::new(Text::new().unwrap()));
        let line = measure.measure(&words("Ag", 11.0, WIDE, 1));
        assert!((12.5..13.8).contains(&line.height), "an 11pt line is about 13 high: {}", line.height);
        let long = "Requested task ".repeat(30);
        let two = measure.measure(&words(long.trim(), 11.0, 200.0, 2));
        assert!(two.lines == 2 && two.trimmed && two.shown.as_deref().is_some_and(|s| s.ends_with('\u{2026}')), "{two:?}");
        let wide = measure.measure(&words(long.trim(), 11.0, 100.0, 1));
        assert!(wide.lines == 1 && wide.trimmed && wide.width <= 100.0 + 0.01, "{wide:?}");
    }

    fn card() -> PetColor {
        PetColor { r: 0x15, g: 0x1B, b: 0x29, a: 255 }
    }

    /// The window draws a scene as given: the card's colour inside its rounded corners, nothing where the scene draws nothing (clicks
    /// pass through there), and it never takes the focus.
    #[test]
    fn the_window_draws_its_scene_and_stays_aside() {
        let pet = open().unwrap();
        let mut events = pet.events().unwrap();
        assert!(pet.events().is_err(), "the events are handed over once");
        let scene = PetScene {
            width: 282.0,
            height: 330.0,
            scale: 1.0,
            origin: PetPoint { x: 40.0, y: 40.0 },
            items: vec![
                PetItem::Fill { rect: PetRect::new(12.0, 60.0, 258.0, 125.0), radius: 18.0, color: card() },
                PetItem::Text(PetText { rect: PetRect::new(24.0, 72.0, WIDE, 13.0), ..words("Claude", 11.0, WIDE, 1) }),
                PetItem::Glyph {
                    family: mighty_platform::pet::SYMBOLS.into(),
                    glyph: "\u{270B}".into(),
                    center: PetPoint { x: 31.5, y: 80.0 },
                    size: PetGlyphSize::Ink(13.5),
                    weight: 400,
                    color: PetColor { r: 0xFF, g: 0xC4, b: 0x5C, a: 255 },
                    over: None,
                },
            ],
            atlas: None,
        };
        let before = GetForegroundWindowValue();
        let _ = pet.capture().unwrap();
        pet.show(&scene).unwrap();
        let mut frame = PetCapture::default();
        for _ in 0..100 {
            frame = pet.capture().unwrap();
            if frame.width == 282 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!((frame.width, frame.height), (282, 330));
        let at = |x: usize, y: usize| {
            let i = (y * frame.width as usize + x) * 4;
            (frame.bgra[i], frame.bgra[i + 1], frame.bgra[i + 2], frame.bgra[i + 3])
        };
        assert_eq!(at(17, 120), (0x29, 0x1B, 0x15, 255), "the card's colour");
        assert_eq!(at(13, 61).3, 0, "a corner of radius 18 leaves its corner clear");
        assert_eq!(at(5, 5).3, 0, "nothing drawn: clear");
        let focus = pet.focus().unwrap();
        assert!(focus.non_activating && focus.own != 0 && !focus.took_thread, "{focus:?}");
        assert_eq!(GetForegroundWindowValue(), before, "showing it moved nothing to the front");
        pet.send(PetInput::Click(LogicalPoint { x: 100.0, y: 100.0 })).unwrap();
        let pressed = events.try_recv().unwrap();
        assert!(
            matches!(pressed, PetEvent::Pressed(p) if p.window == LogicalPoint { x: 100.0, y: 100.0 } && p.screen == LogicalPoint { x: 140.0, y: 140.0 }),
            "{pressed:?}"
        );
        assert!(matches!(events.try_recv().unwrap(), PetEvent::Released(_)));
        pet.send(PetInput::Dpi(144)).unwrap();
        let mut seen = vec![];
        while let Ok(event) = events.try_recv() {
            seen.push(event);
        }
        assert!(seen.contains(&PetEvent::DpiChanged { scale: 1.5 }), "{seen:?}");
        assert!(!pet.focus().unwrap().took_thread, "a press never activates it");
        pet.close().unwrap();
        assert!(pet.draw(&scene).is_err(), "closed, it refuses");
        // Review H1: closed, its thread ends and its events with it.
        let end = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            match events.try_recv() {
                Err(mpsc::error::TryRecvError::Disconnected) => break,
                _ if std::time::Instant::now() > end => panic!("the pet window's thread never ended"),
                _ => std::thread::sleep(std::time::Duration::from_millis(20)),
            }
        }
    }

    /// Review M1: a first frame the window cannot take leaves the show pending, and the next frame it takes shows the window.
    #[test]
    fn a_refused_first_frame_still_shows_the_window_with_the_next() {
        let pet = open().unwrap();
        let events = pet.events().unwrap();
        let huge = PetScene { width: 1e12, height: 10.0, scale: 1.0, ..PetScene::default() };
        pet.show(&huge).unwrap();
        let fine = PetScene {
            width: 40.0,
            height: 40.0,
            scale: 1.0,
            origin: PetPoint { x: 10.0, y: 10.0 },
            items: vec![PetItem::Fill { rect: PetRect::new(0.0, 0.0, 40.0, 40.0), radius: 0.0, color: card() }],
            atlas: None,
        };
        pet.draw(&fine).unwrap();
        let own = HWND(pet.focus().unwrap().own as *mut c_void);
        let end = std::time::Instant::now() + std::time::Duration::from_secs(5);
        // SAFETY: a read of this test's own window's visibility.
        while !unsafe { windows::Win32::UI::WindowsAndMessaging::IsWindowVisible(own) }.as_bool() {
            assert!(std::time::Instant::now() < end, "the window never showed after its refused first frame");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        pet.close().unwrap();
        drop(events);
    }

    #[allow(non_snake_case)]
    fn GetForegroundWindowValue() -> u64 {
        // SAFETY: a read of the desktop's front window.
        unsafe { GetForegroundWindow().0 as u64 }
    }
}
