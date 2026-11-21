//! M-F: completion notifications through `UNUserNotificationCenter` (objc2; plan D6): `.alert`/`.sound` asked once, sent only when
//! authorized, the pane id in `userInfo` (Swift's key `sessionID`), a click focusing the pane (`Notifier::on_activated`), shown as a
//! banner with its sound while the app is in front. Swift `AgentCompanion.swift:380-390,460-486` (`refreshNotificationStatus`,
//! `CompletionNotifications`). Contract `mighty_platform::notify`. A smoke's notifier is the fake (src-tauri platform.rs `ports`), and
//! the authorization is asked only where the write policy writes (notifications.rs), never in a smoke.
//!
//! The notification centre exists only for an app bundle: a bare executable (`target/debug/MightyCode`, a test binary) has no bundle
//! identifier and `currentNotificationCenter` would throw, so outside a `.app` every call refuses without touching it.

use std::ptr::NonNull;
use std::sync::{Arc, Mutex, OnceLock};

use block2::RcBlock;
use mighty_platform::{ActivationHandler, BoxFuture, Notification, Notifier, PlatformError, Result};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, NSObject, ProtocolObject};
use objc2::{AnyThread, define_class, msg_send};
use objc2_foundation::{NSBundle, NSDictionary, NSError, NSObjectProtocol, NSString, NSUUID};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNAuthorizationStatus, UNMutableNotificationContent, UNNotification, UNNotificationPresentationOptions,
    UNNotificationRequest, UNNotificationResponse, UNNotificationSettings, UNNotificationSound, UNUserNotificationCenter,
    UNUserNotificationCenterDelegate,
};

/// The `userInfo` key that carries the pane's id (Swift `CompletionNotifications.send`).
pub const PANE_KEY: &str = "sessionID";

/// The Mac's `Notifier`.
/// System Settings › Notifications (Swift's "Notification settings" button, AgentCompanionViews.swift:101): a fixed address, never one
/// a caller gives, so the files' `open_url` (the web and mail only) is not widened for it.
pub fn open_notification_settings() -> std::result::Result<(), String> {
    objc2::rc::autoreleasepool(|_| {
        let address = objc2_foundation::NSURL::URLWithString(&NSString::from_str(
            "x-apple.systempreferences:com.apple.Notifications-Settings.extension",
        ))
        .ok_or("not an address")?;
        if objc2_app_kit::NSWorkspace::sharedWorkspace().openURL(&address) { Ok(()) } else { Err("System Settings did not open".into()) }
    })
}

pub fn notifier() -> Arc<dyn Notifier> {
    Arc::new(MacNotifier)
}

/// The authorization request (Swift `refreshNotificationStatus(request: true)`: `.alert` and `.sound`, asked once; macOS asks the user
/// only while the answer is not determined). Only where the write policy writes, never in a smoke (notifications.rs). Returns once the
/// request is made; macOS answers later.
pub fn register_notifications() -> std::result::Result<(), String> {
    let center = center().map_err(|e| e.to_string())?;
    let done = RcBlock::new(|_granted: Bool, _error: *mut NSError| {});
    center.requestAuthorizationWithOptions_completionHandler(UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound, &done);
    Ok(())
}

/// The notification settings' answer (Swift `refreshNotificationStatus`): Some(true) authorized (or provisional), Some(false) denied,
/// None not asked yet; the outer None where there is no notification centre (outside an app bundle) or it did not answer.
pub async fn authorized() -> Option<Option<bool>> {
    let asked = authorization(&*center().ok()?);
    let status = asked.await.ok()?;
    Some(if status == UNAuthorizationStatus::Authorized || status == UNAuthorizationStatus::Provisional {
        Some(true)
    } else if status == UNAuthorizationStatus::Denied {
        Some(false)
    } else {
        None
    })
}

/// Whether this process is an app bundle the notification centre can serve (see the module comment).
fn bundled() -> bool {
    let bundle = NSBundle::mainBundle();
    bundle.bundleIdentifier().is_some() && bundle.bundlePath().to_string().ends_with(".app")
}

fn center() -> Result<Retained<UNUserNotificationCenter>> {
    if bundled() {
        Ok(UNUserNotificationCenter::currentNotificationCenter())
    } else {
        Err(PlatformError::Refused("notifications need the app bundle (MightyCode.app)".into()))
    }
}

/// What the notification says (Swift `CompletionNotifications.send`): the title, the body, the default sound and the pane's id. It
/// never carries a prompt, tool arguments or a project path (the shared `Notification`).
pub fn content(notification: &Notification) -> Retained<UNMutableNotificationContent> {
    let content = UNMutableNotificationContent::new();
    content.setTitle(&NSString::from_str(&notification.title));
    content.setBody(&NSString::from_str(&notification.body));
    content.setSound(Some(&UNNotificationSound::defaultSound()));
    if let Some(pane) = &notification.pane_id {
        let key = NSString::from_str(PANE_KEY);
        let value = NSString::from_str(pane);
        let info = NSDictionary::<NSString, NSString>::from_slices(&[&*key], &[&*value]);
        // SAFETY: a dictionary of strings is a property-list dictionary, which is what `userInfo` takes (the cast only forgets the
        // generic types).
        unsafe { content.setUserInfo(&*(Retained::as_ptr(&info) as *const NSDictionary)) };
    }
    content
}

/// The pane a notification's `userInfo` names.
pub fn pane_of(info: &NSDictionary) -> Option<String> {
    let key = NSString::from_str(PANE_KEY);
    // SAFETY: the key is a string, as every key the app writes; the dictionary only reads it.
    let value = unsafe { info.objectForKey(&*(Retained::as_ptr(&key) as *const AnyObject)) }?;
    value.downcast::<NSString>().ok().map(|pane| pane.to_string())
}

struct MacNotifier;

fn guarded(f: impl FnOnce()) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
}

/// The user's answer to notifications (`getNotificationSettings`), when the centre gives it.
fn authorization(center: &UNUserNotificationCenter) -> tokio::sync::oneshot::Receiver<UNAuthorizationStatus> {
    let (answer, asked) = tokio::sync::oneshot::channel();
    let answer = Mutex::new(Some(answer));
    let settings = RcBlock::new(move |settings: NonNull<UNNotificationSettings>| {
        guarded(|| {
            // SAFETY: the settings object the framework hands the handler, alive for the call.
            let status = unsafe { settings.as_ref() }.authorizationStatus();
            if let Some(answer) = lock(&answer).take() {
                let _ = answer.send(status);
            }
        });
    });
    center.getNotificationSettingsWithCompletionHandler(&settings);
    asked
}

/// The notification handed to the centre (no trigger: shown at once); its error's words, if it refused.
fn add(center: &UNUserNotificationCenter, notification: &Notification) -> tokio::sync::oneshot::Receiver<Option<String>> {
    let content = content(notification);
    let identifier = NSUUID::new().UUIDString();
    let request = UNNotificationRequest::requestWithIdentifier_content_trigger(&identifier, &content, None);
    let (added, adding) = tokio::sync::oneshot::channel();
    let added = Mutex::new(Some(added));
    let done = RcBlock::new(move |error: *mut NSError| {
        guarded(|| {
            // SAFETY: null, or the error the framework hands the handler, alive for the call.
            let failure = unsafe { error.as_ref() }.map(|e| e.localizedDescription().to_string());
            if let Some(added) = lock(&added).take() {
                let _ = added.send(failure);
            }
        });
    });
    center.addNotificationRequest_withCompletionHandler(&request, Some(&done));
    adding
}

/// The app's one delegate, kept for the process (the centre holds its delegate weakly).
struct Sendable<T>(T);
// SAFETY: the delegate holds no state; the notification centre calls it on any thread and the app only hands it over.
unsafe impl<T> Send for Sendable<T> {}

impl Notifier for MacNotifier {
    fn notify(&self, notification: Notification) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            // Sent only when the user allowed it (Swift checks the settings before each one). The centre and the blocks stay inside
            // the two calls: only their answers cross the waits.
            let asked = authorization(&*center()?);
            let status = asked.await.map_err(|_| PlatformError::Refused("the notification settings were not read".into()))?;
            if status != UNAuthorizationStatus::Authorized && status != UNAuthorizationStatus::Provisional {
                return Err(PlatformError::Refused("notifications are not allowed for Mighty Code".into()));
            }
            let adding = add(&*center()?, &notification);
            match adding.await {
                Ok(None) => Ok(()),
                Ok(Some(error)) => Err(PlatformError::Refused(error)),
                Err(_) => Err(PlatformError::Refused("the notification was not added".into())),
            }
        })
    }

    fn on_activated(&self, handler: ActivationHandler) {
        *lock(handlers()) = Some(handler);
        let Ok(center) = center() else { return };
        let delegate = DELEGATE.get_or_init(|| Sendable(Delegate::new()));
        center.setDelegate(Some(ProtocolObject::from_ref(&*delegate.0)));
    }
}

fn handlers() -> &'static Mutex<Option<ActivationHandler>> {
    static HANDLER: Mutex<Option<ActivationHandler>> = Mutex::new(None);
    &HANDLER
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

// SAFETY: as for Send.
unsafe impl<T> Sync for Sendable<T> {}

/// The centre keeps its delegate weakly: the app's one delegate lives for the process.
static DELEGATE: OnceLock<Sendable<Retained<Delegate>>> = OnceLock::new();

define_class!(
    // SAFETY: NSObject has no subclassing requirement this class breaks; it does not implement Drop.
    #[unsafe(super(NSObject))]
    #[name = "MightyNotificationDelegate"]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl UNUserNotificationCenterDelegate for Delegate {
        /// Shown as a banner with its sound while the app is in front (Swift `willPresent`).
        #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
        fn will_present(
            &self,
            _center: &UNUserNotificationCenter,
            _notification: &UNNotification,
            completion: &block2::DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
        ) {
            guarded(|| {
                completion.call((UNNotificationPresentationOptions::Banner | UNNotificationPresentationOptions::Sound,));
            });
        }

        /// A click: the pane named in `userInfo` is brought forward (Swift `didReceive`).
        #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
        fn did_receive(
            &self,
            _center: &UNUserNotificationCenter,
            response: &UNNotificationResponse,
            completion: &block2::DynBlock<dyn Fn()>,
        ) {
            guarded(|| {
                let pane = pane_of(&response.notification().request().content().userInfo());
                let handler = lock(handlers()).clone();
                // The framework is told at once; the focus runs on a thread of its own, so a slow or failing handler never holds or
                // unwinds through AppKit's delegate call (review item 5).
                completion.call(());
                if let Some(handler) = handler
                    && pane.is_some()
                {
                    let _ = std::thread::Builder::new().name("mighty-notification-click".into()).spawn(move || handler(pane));
                }
            });
        }
    }
);

impl Delegate {
    fn new() -> Retained<Delegate> {
        let this = Delegate::alloc().set_ivars(());
        // SAFETY: NSObject's designated initializer.
        unsafe { msg_send![super(this), init] }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a completion notification carries, built without the notification centre (no bundle, no request reaches macOS): the
    /// shared title and body, the default sound, and the pane's id under Swift's `sessionID` key, read back as a click reads it.
    #[test]
    fn a_notification_carries_its_words_and_its_pane() {
        let notification = Notification { title: "Done".into(), body: "Claude has finished.".into(), pane_id: Some("pane-1".into()) };
        let made = content(&notification);
        assert_eq!(made.title().to_string(), "Done");
        assert_eq!(made.body().to_string(), "Claude has finished.");
        assert!(made.sound().is_some());
        assert_eq!(pane_of(&made.userInfo()).as_deref(), Some("pane-1"));
        let silent = content(&Notification { pane_id: None, ..notification });
        assert_eq!(pane_of(&silent.userInfo()), None);
    }

    /// A test binary is no app bundle: the notifier refuses without touching the notification centre (which would throw), and the
    /// authorization is never asked.
    #[test]
    fn outside_an_app_bundle_nothing_reaches_the_notification_centre() {
        assert!(!bundled());
        let refused = tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(MacNotifier.notify(Notification {
            title: "t".into(),
            body: "b".into(),
            pane_id: None,
        }));
        assert!(matches!(refused, Err(PlatformError::Refused(_))));
        assert!(register_notifications().is_err());
        MacNotifier.on_activated(Arc::new(|_| {}));
        assert!(DELEGATE.get().is_none(), "no delegate is made without the notification centre");
    }
}
