//! M-E: CEF's `CrAppProtocol` on tao's `NSApplication` subclass (`TaoApp`), added at run time before CEF initializes: the methods
//! `isHandlingSendEvent` and `setHandlingSendEvent:`, and `sendEvent:` wrapped to set the flag while the original runs (CEF's
//! `CefScopedSendingEvent`). Swift `MightyApplication.swift` subclassed `NSApplication` itself; the bridge (`cef_bridge.m`) checks
//! that `NSApp` answers both selectors before it registers `CefAppProtocol` and initializes. Step 0's cef spike added exactly these
//! to `TaoApp` on `macos-latest`.

use std::ffi::CString;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use mighty_platform::{PlatformError, Result};
use objc2::encode::Encode as _;
use objc2::runtime::{AnyClass, AnyObject, Bool, Imp, Sel};
use objc2::{MainThreadMarker, ffi, sel};
use objc2_app_kit::NSApplication;

/// Whether `sendEvent:` is running now (CEF asks while it handles an event).
static HANDLING: AtomicBool = AtomicBool::new(false);
/// The class's own `sendEvent:` before the wrapper (tao's, or NSApplication's).
static ORIGINAL_SEND_EVENT: OnceLock<Imp> = OnceLock::new();

unsafe extern "C-unwind" fn is_handling_send_event(_this: *mut AnyObject, _cmd: Sel) -> Bool {
    Bool::new(HANDLING.load(Ordering::SeqCst))
}

unsafe extern "C-unwind" fn set_handling_send_event(_this: *mut AnyObject, _cmd: Sel, handling: Bool) {
    HANDLING.store(handling.as_bool(), Ordering::SeqCst);
}

unsafe extern "C-unwind" fn send_event(this: *mut AnyObject, cmd: Sel, event: *mut AnyObject) {
    let previous = HANDLING.swap(true, Ordering::SeqCst);
    if let Some(original) = ORIGINAL_SEND_EVENT.get() {
        // SAFETY: the class's own `sendEvent:` implementation, called as AppKit calls it.
        let original = unsafe { std::mem::transmute::<Imp, unsafe extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject)>(*original) };
        unsafe { original(this, cmd, event) };
    }
    HANDLING.store(previous, Ordering::SeqCst);
}

fn types(text: String) -> Result<CString> {
    CString::new(text).map_err(|error| PlatformError::Invalid(error.to_string()))
}

/// Adds the methods to `NSApp`'s class, once. Call on the main thread, before the run loop and before CEF initializes.
pub fn add_cef_methods() -> Result<()> {
    let mtm = MainThreadMarker::new().ok_or_else(|| PlatformError::Refused("CEF's methods: not on the main thread".into()))?;
    let app = NSApplication::sharedApplication(mtm);
    let object: &AnyObject = &app;
    let class: &AnyClass = object.class();
    if ORIGINAL_SEND_EVENT.get().is_some() {
        return Ok(());
    }
    let original = class
        .instance_method(sel!(sendEvent:))
        .map(|method| method.implementation())
        .ok_or_else(|| PlatformError::Refused("CEF's methods: NSApp has no sendEvent:".into()))?;
    let _ = ORIGINAL_SEND_EVENT.set(original);
    let (object_type, selector_type) = (<*mut AnyObject>::ENCODING, Sel::ENCODING);
    let is_types = types(format!("{}{object_type}{selector_type}", Bool::ENCODING))?;
    let set_types = types(format!("v{object_type}{selector_type}{}", Bool::ENCODING))?;
    let send_types = types(format!("v{object_type}{selector_type}{object_type}"))?;
    let class = class as *const AnyClass as *mut AnyClass;
    // SAFETY: the Objective-C runtime on the main thread before the run loop; each implementation takes the arguments its type
    // string names.
    let (added_is, added_set) = unsafe {
        let is = std::mem::transmute::<unsafe extern "C-unwind" fn(*mut AnyObject, Sel) -> Bool, Imp>(is_handling_send_event);
        let set = std::mem::transmute::<unsafe extern "C-unwind" fn(*mut AnyObject, Sel, Bool), Imp>(set_handling_send_event);
        let send = std::mem::transmute::<unsafe extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject), Imp>(send_event);
        let added_is = ffi::class_addMethod(class, sel!(isHandlingSendEvent), is, is_types.as_ptr()).as_bool();
        let added_set = ffi::class_addMethod(class, sel!(setHandlingSendEvent:), set, set_types.as_ptr()).as_bool();
        ffi::class_replaceMethod(class, sel!(sendEvent:), send, send_types.as_ptr());
        (added_is, added_set)
    };
    if added_is && added_set { Ok(()) } else { Err(PlatformError::Refused("CEF's methods were not added to NSApp's class".into())) }
}
