//! M-F: the session's end and the user's last input for the app update (with M-H): `NSWorkspaceWillPowerOffNotification` for a
//! logout, restart or shutdown (tao owns `applicationShouldTerminate:`, so Swift's `kAEQuitReason` is not read), the last input from
//! `CGEventSourceSecondsSinceLastEventType(.combinedSessionState, ~0)`. Swift `AppStore+AppUpdate.swift:161`,
//! `MightyCodeApp.swift` `applicationShouldTerminate`. Contract `mighty_platform::session` (`SessionWatch`).
//!
//! The Mac says only that the session is going (there is no "cancelled" message as Windows' `WM_ENDSESSION` false): the power-off
//! notice is [`SessionEnding::Query`], which keeps a staged update from installing on this quit (app_update.rs). A logout the user
//! cancels leaves it set, as Swift's quit reason was read from that quit alone: the update then installs on the next quit after a
//! restart instead. Sleep and wake are the network port's (M-C, `Network::on_resume`); a countdown the Mac slept through is the
//! shared rule (`mighty_updates::app_update::auto`'s `COUNTDOWN_GRACE`).

use std::ptr::NonNull;
use std::sync::Arc;
use std::time::Duration;

use block2::RcBlock;
use mighty_platform::{Result, SessionCallback, SessionEnding, SessionWatch, Subscription};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObjectProtocol, ProtocolObject};
use objc2_app_kit::{NSWorkspace, NSWorkspaceWillPowerOffNotification};
use objc2_core_graphics::{CGEventSource, CGEventSourceStateID, CGEventType};
use objc2_foundation::{NSNotification, NSNotificationCenter};

/// `kCGAnyInputEventType` (`~0`): any keyboard, mouse or tablet input.
const ANY_INPUT: CGEventType = CGEventType(u32::MAX);

/// The Mac's `SessionWatch`.
pub fn session() -> Arc<dyn SessionWatch> {
    Arc::new(MacSession)
}

struct MacSession;

/// An observer token of the workspace's notification centre: removed when the subscription ends. NSNotificationCenter is thread-safe
/// (Apple's documentation), so the token may be removed from whichever thread drops the subscription.
struct Observer {
    center: Retained<NSNotificationCenter>,
    token: Retained<ProtocolObject<dyn NSObjectProtocol>>,
}

// SAFETY: NSNotificationCenter and its observer tokens may be used from any thread (see above); nothing else is held.
unsafe impl Send for Observer {}

impl Drop for Observer {
    fn drop(&mut self) {
        // SAFETY: the token this centre returned for the observation.
        unsafe { self.center.removeObserver(&*(Retained::as_ptr(&self.token) as *const AnyObject)) };
    }
}

/// The power-off notice of `center` delivered to `callback` until the subscription is dropped.
fn watch(center: Retained<NSNotificationCenter>, callback: SessionCallback) -> Subscription {
    let block = RcBlock::new(move |_note: NonNull<NSNotification>| callback(SessionEnding::Query));
    // SAFETY: a block that only calls a Send + Sync callback; no queue (it runs where AppKit posts the notice: the main thread).
    let token = unsafe { center.addObserverForName_object_queue_usingBlock(Some(NSWorkspaceWillPowerOffNotification), None, None, &block) };
    let observer = Observer { center, token };
    Subscription::new(move || drop(observer))
}

impl SessionWatch for MacSession {
    fn on_ending(&self, callback: SessionCallback) -> Result<Subscription> {
        Ok(watch(NSWorkspace::sharedWorkspace().notificationCenter(), callback))
    }

    fn since_last_input(&self) -> Result<Duration> {
        let seconds = CGEventSource::seconds_since_last_event_type(CGEventSourceStateID::CombinedSessionState, ANY_INPUT);
        Ok(if seconds.is_finite() && seconds >= 0.0 { Duration::from_secs_f64(seconds) } else { Duration::ZERO })
    }
}

/// Whether another process of the app `identifier` runs on this Mac (the Swift 0.4.x app and this one share
/// `dev.mightycode.native`): both would write Application Support, UserDefaults and the Keychain under the same names (Mac
/// integration review 17). Swift had no rule of its own beyond LaunchServices activating a running copy.
pub fn another_instance_running(identifier: &str) -> bool {
    let me = std::process::id() as i32;
    objc2::rc::autoreleasepool(|_| {
        objc2_app_kit::NSRunningApplication::runningApplicationsWithBundleIdentifier(&objc2_foundation::NSString::from_str(identifier))
            .iter()
            .any(|app| app.processIdentifier() != me && !app.isTerminated())
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use objc2_foundation::NSString;

    use super::*;

    /// Swift `AppUpdateAutoStepsTests.swift:44` "onlyAQuitWithoutAReasonOrAPlainQuitReasonInstalls" (the session half): a logout,
    /// restart or shutdown notice is the session ending, and nothing else is; after the subscription ends nothing arrives. Posted on a
    /// centre of the test's own, never the workspace's.
    #[test]
    fn only_a_quit_without_a_reason_or_a_plain_quit_reason_installs() {
        let seen: Arc<Mutex<Vec<SessionEnding>>> = Arc::default();
        let center = NSNotificationCenter::new();
        let recorded = seen.clone();
        let subscription = watch(center.clone(), Arc::new(move |event| recorded.lock().unwrap().push(event)));
        // SAFETY: notices posted on the test's own centre, with no object.
        unsafe {
            center.postNotificationName_object(&NSString::from_str("NSWorkspaceDidWakeNotification"), None);
            center.postNotificationName_object(NSWorkspaceWillPowerOffNotification, None);
        }
        assert_eq!(*seen.lock().unwrap(), [SessionEnding::Query]);
        drop(subscription);
        // SAFETY: as above.
        unsafe { center.postNotificationName_object(NSWorkspaceWillPowerOffNotification, None) };
        assert_eq!(seen.lock().unwrap().len(), 1, "an ended watch hears nothing more");
    }

    #[test]
    fn the_last_input_is_read_from_the_session() {
        let since = MacSession.since_last_input().unwrap();
        assert!(since < Duration::from_secs(60 * 60 * 24 * 365), "a time since the last input: {since:?}");
        assert!(MacSession.on_ending(Arc::new(|_| {})).is_ok());
    }
}
