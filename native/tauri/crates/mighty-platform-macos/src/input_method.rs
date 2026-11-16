//! M-F: the menu's input method items (plan D18). "Reconnect Input Method" re-activates the app, waits for the page's window to be
//! key and makes the page's webview first responder again (the AppKit half of Swift `InputSessionRecoveryCoordinator.swift`:
//! activate, `makeKey`, `makeFirstResponder(nil)` then the editor; its NSTextView half has no counterpart, the composer being the
//! page in WebKit). "Save Input Method Diagnostics" writes `<data>/diagnostics/ime-<stamp>-<id>.json` (folder 0700, file 0600) with
//! the app, window, input context and TIS input sources (`InputMethodDiagnostics.swift:186-246`, `persistSnapshot`).
//!
//! Not ported: `HangulComposer` (dead code, plan fact 13) and the composer's jamo detector (`InputMethodSymptom`, its NSTextView
//! monitor), so Swift's `InputMethodSymptomTests` have no counterpart here; WebKit composes and the page guards composition
//! (`isComposing || keyCode === 229`, `web/composer/keys.ts`).

use std::ffi::c_void;
use std::io::Write as _;
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mighty_platform::{PlatformError, Result};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{ClassType, MainThreadMarker, Message};
use objc2_app_kit::{NSApplication, NSTextInputContext, NSView, NSWindow, NSWorkspace};
use objc2_foundation::{NSBundle, NSString};
use serde_json::{Map, Value, json};

/// How a reconnection ended (Swift `InputSessionRecoveryCoordinator.State`'s ends; the words are the locale's `inputRecovery.*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reconnect {
    /// The app is active, the window key and the page's webview the first responder again.
    Reconnected,
    /// macOS did not activate the app in time.
    ActivationUnavailable,
    /// The window did not become key in time.
    WindowUnavailable,
    /// The window refused the page's webview as its first responder.
    ResponderRejected,
}

impl Reconnect {
    /// The locale key of what the user is told.
    pub fn key(self) -> &'static str {
        match self {
            Reconnect::Reconnected => "inputRecovery.reconnected",
            Reconnect::ActivationUnavailable => "inputRecovery.activationUnavailable",
            Reconnect::WindowUnavailable => "inputRecovery.windowUnavailable",
            Reconnect::ResponderRejected => "inputRecovery.responderRejected",
        }
    }
}

/// How long the app and its window are waited for (Swift's coordinator gives the same two seconds).
const RECONNECT_TIMEOUT: Duration = Duration::from_secs(2);

/// The page's window and webview, handed from the main thread to itself across the waits (only ever used there).
struct Page {
    window: Retained<NSWindow>,
    webview: Retained<NSView>,
}
// SAFETY: created, used and dropped only inside main-thread closures (`on_main`); the tokio task only carries it between them.
unsafe impl Send for Page {}

/// Runs `work` on the main thread and waits for its answer.
async fn on_main<T: Send + 'static>(webview: &tauri::Webview, work: impl FnOnce(MainThreadMarker) -> T + Send + 'static) -> Result<T> {
    let (answer, answered) = tokio::sync::oneshot::channel();
    webview
        .run_on_main_thread(move || {
            if let Some(mtm) = MainThreadMarker::new() {
                let _ = answer.send(work(mtm));
            }
        })
        .map_err(|e| PlatformError::Refused(e.to_string()))?;
    answered.await.map_err(|_| PlatformError::Refused("the main thread did not answer".into()))
}

/// The page's webview takes the keyboard back: the app activated, the window made key and the webview made its first responder again.
pub async fn reconnect(webview: &tauri::Webview) -> Result<Reconnect> {
    let (found, made) = tokio::sync::oneshot::channel::<Option<Page>>();
    webview
        .with_webview(move |platform| {
            let window = platform.ns_window() as *mut NSWindow;
            let view = platform.inner() as *mut NSView;
            // SAFETY: Tauri hands the live NSWindow and WKWebView of this webview on the main thread; each is retained here.
            let page = unsafe { Retained::retain(window).zip(Retained::retain(view)) }.map(|(window, webview)| Page { window, webview });
            let _ = found.send(page);
        })
        .map_err(|e| PlatformError::Refused(e.to_string()))?;
    let page = made.await.ok().flatten().ok_or_else(|| PlatformError::Refused("the page's webview is gone".into()))?;
    let mut page = on_main(webview, move |mtm| {
        let app = NSApplication::sharedApplication(mtm);
        if !app.isActive() {
            #[allow(deprecated, reason = "activate() is macOS 14's; activateIgnoringOtherApps: still answers there")]
            app.activateIgnoringOtherApps(true);
        }
        page.window.makeKeyAndOrderFront(None);
        page
    })
    .await?;
    let started = Instant::now();
    loop {
        let (state, back) = on_main(webview, move |mtm| {
            let app = NSApplication::sharedApplication(mtm);
            let state = if !app.isActive() {
                None.ok_or(Reconnect::ActivationUnavailable)
            } else if !page.window.isKeyWindow() {
                None.ok_or(Reconnect::WindowUnavailable)
            } else {
                // Swift `advance`: the responder released first, so the webview's input context is made anew when it takes it back.
                let released = page.window.makeFirstResponder(None);
                let taken = released && page.window.makeFirstResponder(Some(page.webview.as_super()));
                Ok(if taken { Reconnect::Reconnected } else { Reconnect::ResponderRejected })
            };
            (state, page)
        })
        .await?;
        page = back;
        match state {
            Ok(done) => return Ok(done),
            Err(waiting) if started.elapsed() >= RECONNECT_TIMEOUT => return Ok(waiting),
            Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
        }
    }
}

/// The diagnostics file under `data_dir` (Swift `writeDiagnostics(editor:reason: "manual")`): its path.
pub async fn save_diagnostics(webview: &tauri::Webview, data_dir: &Path) -> Result<PathBuf> {
    let mut info = on_main(webview, snapshot).await?;
    info.insert("reason".into(), json!("manual"));
    persist(&info, &data_dir.join("diagnostics"), SystemTime::now()).map_err(PlatformError::Io)
}

/// Swift `snapshot(editor: nil)`, the AppKit facts that apply to the page: no prompt text, no path but the bundle's.
fn snapshot(mtm: MainThreadMarker) -> Map<String, Value> {
    let app = NSApplication::sharedApplication(mtm);
    let workspace = NSWorkspace::sharedWorkspace();
    let own = std::process::id() as i32;
    let front = workspace.frontmostApplication().map_or(0, |a| a.processIdentifier());
    let bundle = NSBundle::mainBundle();
    let version = bundle
        .infoDictionary()
        .and_then(|info| info.objectForKey(&NSString::from_str("CFBundleShortVersionString")))
        .and_then(|v| v.downcast::<NSString>().ok())
        .map_or_else(|| "?".to_string(), |v| v.to_string());
    let key = app.keyWindow();
    let responder = key.as_ref().and_then(|w| w.firstResponder()).map_or_else(|| "-".to_string(), |r| class_name(any(&r)));
    let context = NSTextInputContext::currentInputContext(mtm);
    let mut info = Map::new();
    info.insert("time".into(), json!(iso8601(SystemTime::now())));
    info.insert("bundlePath".into(), json!(bundle.bundlePath().to_string()));
    info.insert("version".into(), json!(version));
    info.insert("appIsActive".into(), json!(app.isActive()));
    info.insert("activationPolicy".into(), json!(app.activationPolicy().0));
    info.insert("appIsHidden".into(), json!(app.isHidden()));
    info.insert("frontmostIsOwnApplication".into(), json!(front == own));
    info.insert("frontmostProcessIdentifier".into(), json!(front));
    info.insert("keyWindowNumber".into(), json!(key.as_ref().map_or(0, |w| w.windowNumber())));
    info.insert("mainWindowNumber".into(), json!(app.mainWindow().map_or(0, |w| w.windowNumber())));
    info.insert("firstResponder".into(), json!(responder));
    info.insert("secureEventInput".into(), json!(secure_event_input()));
    info.insert("currentInputSource".into(), json!(input_source(InputSource::Keyboard).unwrap_or_else(|| "-".into())));
    info.insert("keyboardLayoutSource".into(), json!(input_source(InputSource::Layout).unwrap_or_else(|| "-".into())));
    info.insert(
        "textInputContextCurrent".into(),
        json!(context.as_ref().map_or_else(|| "-".to_string(), |c| format!("{:p}", Retained::as_ptr(c)))),
    );
    info.insert(
        "textInputContextClientClass".into(),
        json!(context.as_ref().map_or_else(|| "-".to_string(), |c| class_name(any(&c.client())))),
    );
    info.insert(
        "textInputContextSource".into(),
        json!(context.and_then(|c| c.selectedKeyboardInputSource()).map_or_else(|| "-".to_string(), |s| s.to_string())),
    );
    info
}

fn class_name(object: &AnyObject) -> String {
    object.class().name().to_string_lossy().into_owned()
}

/// Any object as the root object it is.
fn any<T: Message + ?Sized>(object: &Retained<T>) -> &AnyObject {
    // SAFETY: every Objective-C object is an AnyObject; the reference keeps the Retained's lifetime.
    unsafe { &*(Retained::as_ptr(object) as *const AnyObject) }
}

/// `persistSnapshot`: the snapshot as sorted, pretty JSON in `ime-<ISO 8601 without colons>-<8 of a UUID>.json`, the folder made 0700
/// and the file written 0600 (never readable by another user: it names the app's windows and input sources).
pub fn persist(info: &Map<String, Value>, directory: &Path, now: SystemTime) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(directory)?;
    std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))?;
    let stamp = iso8601(now).replace(':', "");
    let id: String = objc2_foundation::NSUUID::new().UUIDString().to_string().chars().take(8).collect();
    let file = directory.join(format!("ime-{stamp}-{id}.json"));
    // serde_json's map keeps its keys sorted (JSONSerialization's `.sortedKeys`).
    let text = serde_json::to_string_pretty(&Value::Object(info.clone())).map_err(std::io::Error::other)?;
    let mut out = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&file)?;
    out.write_all(text.as_bytes())?;
    Ok(file)
}

/// `ISO8601DateFormatter`'s default: `2026-11-16T09:05:03Z` (UTC, whole seconds).
pub fn iso8601(at: SystemTime) -> String {
    let seconds = at.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()) as i64;
    let (days, rest) = (seconds.div_euclid(86_400), seconds.rem_euclid(86_400));
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z", rest / 3600, rest / 60 % 60, rest % 60)
}

enum InputSource {
    Keyboard,
    Layout,
}

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn TISCopyCurrentKeyboardInputSource() -> *mut c_void;
    fn TISCopyCurrentKeyboardLayoutInputSource() -> *mut c_void;
    fn TISGetInputSourceProperty(source: *mut c_void, key: *const c_void) -> *const c_void;
    static kTISPropertyInputSourceID: *const c_void;
    fn IsSecureEventInputEnabled() -> u8;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(object: *const c_void);
}

/// `currentInputSourceID` / `currentLayoutSourceID`: the TIS input source's id (`com.apple.inputmethod.Korean.2SetKorean`, …).
fn input_source(which: InputSource) -> Option<String> {
    // SAFETY: TIS's copy functions return a retained source or null; the property is a CFString the source owns (get rule), copied
    // into a Rust string before the source is released.
    unsafe {
        let source = match which {
            InputSource::Keyboard => TISCopyCurrentKeyboardInputSource(),
            InputSource::Layout => TISCopyCurrentKeyboardLayoutInputSource(),
        };
        if source.is_null() {
            return None;
        }
        let id = TISGetInputSourceProperty(source, kTISPropertyInputSourceID) as *const NSString;
        let text = id.as_ref().map(|id| id.to_string());
        CFRelease(source);
        text
    }
}

fn secure_event_input() -> bool {
    // SAFETY: a plain query with no arguments.
    unsafe { IsSecureEventInputEnabled() != 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stamp_is_iso_8601_in_utc() {
        assert_eq!(iso8601(UNIX_EPOCH), "1970-01-01T00:00:00Z");
        assert_eq!(iso8601(UNIX_EPOCH + Duration::from_secs(1_794_819_903)), "2026-11-16T09:05:03Z");
        assert_eq!(iso8601(UNIX_EPOCH + Duration::from_secs(951_782_400)), "2000-02-29T00:00:00Z");
    }

    /// Swift `persistSnapshot`: the folder 0700, the file 0600 under `ime-<stamp without colons>-<8>.json`, sorted pretty JSON; two in
    /// the same second do not overwrite each other. Under a temporary folder only.
    #[test]
    fn the_diagnostics_file_is_owner_only() {
        let root = std::env::temp_dir().join(format!("mighty-ime-{}", objc2_foundation::NSUUID::new().UUIDString()));
        let folder = root.join("diagnostics");
        let mut info = Map::new();
        info.insert("reason".into(), json!("manual"));
        info.insert("appIsActive".into(), json!(true));
        let at = UNIX_EPOCH + Duration::from_secs(1_794_819_903);
        let first = persist(&info, &folder, at).unwrap();
        let second = persist(&info, &folder, at).unwrap();
        assert_ne!(first, second);
        let name = first.file_name().unwrap().to_string_lossy().into_owned();
        assert!(
            name.starts_with("ime-2026-11-16T090503Z-") && name.ends_with(".json") && name.len() == "ime-2026-11-16T090503Z-".len() + 13
        );
        assert_eq!(std::fs::metadata(&folder).unwrap().permissions().mode() & 0o777, 0o700);
        assert_eq!(std::fs::metadata(&first).unwrap().permissions().mode() & 0o777, 0o600);
        let text = std::fs::read_to_string(&first).unwrap();
        assert!(text.find("appIsActive").unwrap() < text.find("reason").unwrap(), "keys sorted: {text}");
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// The TIS reads answer on any Mac (the runner's ABC layout at least) and never fail the snapshot.
    #[test]
    fn the_input_sources_are_read() {
        let layout = input_source(InputSource::Layout);
        assert!(layout.as_deref().is_none_or(|id| id.starts_with("com.apple.")), "{layout:?}");
        let _ = secure_event_input();
        assert_eq!(Reconnect::WindowUnavailable.key(), "inputRecovery.windowUnavailable");
    }
}
