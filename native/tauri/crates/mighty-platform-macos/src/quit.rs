//! MC-81: the app's own question before it quits. AppKit ends the app through `-[NSApplication terminate:]` (the menu's Quit ⌘Q,
//! the Dock's Quit, logging out), which asks its delegate `applicationShouldTerminate:` first; the event loop's delegate (tao's) does
//! not answer it, so the app used to end without the event its quit question waits on. [`hold`] gives that delegate the answer:
//! the app's `ask` runs, and when it holds the quit (it asks the person itself and quits once answered) AppKit is told to cancel.

use std::ffi::c_void;
use std::sync::OnceLock;

use objc2::runtime::{AnyClass, AnyObject, Imp, Sel};
use objc2::{MainThreadMarker, sel};
use objc2_app_kit::NSApplication;

type Ask = Box<dyn Fn() -> bool + Send + Sync>;

static ASK: OnceLock<Ask> = OnceLock::new();

/// `NSTerminateCancel`, `NSTerminateNow`.
const CANCEL: usize = 0;
const NOW: usize = 1;

/// The app's delegate asked from now on before every quit: `ask` answers true when it held the quit. Main thread only, once the
/// event loop runs (its delegate is set); false when it could not be given (no delegate, another thread, a second call).
pub fn hold(ask: impl Fn() -> bool + Send + Sync + 'static) -> bool {
    let Some(mtm) = MainThreadMarker::new() else { return false };
    let Some(delegate) = NSApplication::sharedApplication(mtm).delegate() else { return false };
    if ASK.set(Box::new(ask)).is_err() {
        return false;
    }
    let object: &AnyObject = (*delegate).as_ref();
    let class: *const AnyClass = object.class();
    // SAFETY: `should_terminate` has the selector's C signature (`NSApplicationTerminateReply (id, SEL, NSApplication *)`, the
    // types `Q@:@`), and the delegate's class is a live Objective-C class. `class_addMethod` adds the method only where the class
    // has none of its own (tao's has none), never replacing one.
    unsafe {
        let imp: Imp = std::mem::transmute::<extern "C-unwind" fn(*mut AnyObject, Sel, *mut c_void) -> usize, Imp>(should_terminate);
        objc2::ffi::class_addMethod(class as *mut AnyClass, sel!(applicationShouldTerminate:), imp, c"Q@:@".as_ptr()).as_bool()
    }
}

extern "C-unwind" fn should_terminate(_this: *mut AnyObject, _cmd: Sel, _app: *mut c_void) -> usize {
    let held = ASK.get().is_some_and(|ask| std::panic::catch_unwind(std::panic::AssertUnwindSafe(ask)).unwrap_or(false));
    if held { CANCEL } else { NOW }
}
