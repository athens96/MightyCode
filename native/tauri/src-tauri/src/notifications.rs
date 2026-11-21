//! Completion notifications (C# MainWindow.CompletionNotification.cs: `InitNotifierAsync`, `HandleRunEventForNotification`,
//! `FocusSession`; `RunCompletionNotificationSmoke`), over mighty-core's `notifications`. Owner: W6-A.
//!
//! A run seen running that completes, with the setting on, shows one toast (`Ports::notifier`): the localized title over "{title} has
//! finished.". Registering with the OS writes the user's registry, so it happens only where the app may change the machine (the write
//! policy, startup.rs: a release build, or `--allow-state-write`) and never in a smoke; otherwise the permission row reads "verification
//! mode" and no toast shows. A click on a toast brings the window forward, pushes `select{paneId}` and selects the pane.
//!
//! W9-F (plan §1.8, D10; W6 review M3): the registration is the toast activator's (`mighty_platform::ToastActivator`: the AUMID
//! `dev.mightycode.native` with its `CustomActivator` class, started as `"<exe>" -Embedding`), and the running app serves that class for
//! its life, so a click on a toast left in Action Center after the app quit starts the app (`-Embedding`, args.rs) and reaches
//! [`focus`] through COM, as a click on a live toast does through the notifier.

use std::path::Path;
use std::sync::{Arc, Mutex};

use mighty_core::locale::Locale;
#[cfg(feature = "dev-harness")]
use mighty_core::notifications::SmokeOutcome;
use mighty_core::notifications::{self, Decision};
use mighty_platform::activator::{AUMID, CLSID, session_id};
use mighty_platform::{Notification, Subscription, ToastActivator};
use tauri::Manager;

use crate::host::Host;

/// Whether the OS lets the app notify (C# `NotificationPermission`): `Verification` while the app may not register with the OS
/// (without `--allow-state-write`, and in a smoke), else allowed or needing the user's permission; on the Mac also refused in System
/// Settings (Swift `refreshNotificationStatus`: `.denied`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NotificationPermission {
    #[default]
    Verification,
    Allowed,
    NeedPermission,
    Denied,
}

/// Swift `refreshNotificationStatus` (AgentCompanion.swift:389): authorized is allowed, denied is denied, not yet asked needs the
/// person's permission.
pub fn permission_for(authorized: Option<bool>) -> NotificationPermission {
    match authorized {
        Some(true) => NotificationPermission::Allowed,
        Some(false) => NotificationPermission::Denied,
        None => NotificationPermission::NeedPermission,
    }
}

/// The OS's own answer replaces what the registration implied, except in the verification mode (nothing was asked of the OS).
fn apply_status(current: NotificationPermission, authorized: Option<bool>) -> NotificationPermission {
    if current == NotificationPermission::Verification { current } else { permission_for(authorized) }
}

#[derive(Default)]
struct State {
    decision: Decision,
    permission: NotificationPermission,
    /// The activator's class served for the app's life (dropped: revoked).
    served: Option<Subscription>,
    /// When the OS's notification settings were last asked for (the Mac), so a row drawn often asks at most every 2 s.
    asked: Option<std::time::Instant>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn with<T>(f: impl FnOnce(&mut State) -> T) -> T {
    let mut state = STATE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    f(state.get_or_insert_with(State::default))
}

/// What the toast activator gave [`start`]: the permission it implies, the class served, and why either failed (for the log).
struct Activated {
    permission: NotificationPermission,
    served: Option<Subscription>,
    errors: Vec<String>,
}

/// The activator's part of C#'s `AppNotificationManager.Register`: only where the app may change the machine, the AUMID registered and
/// the class served, each activation's `sessionId` handed to `focus`. Only the installed release on the default profile (`owns`,
/// `startup::owns_registrations`) registers the activator itself (the class and its `"<exe>" -Embedding`); any other writing run
/// registers the AUMID's name alone (`register_name`, so its toasts show) and names no exe of its own in the machine's registry (W9
/// review L1). Without the right: nothing registered or served, the verification mode. A failed registration needs the user's
/// permission; a class that is not served only loses the clicks from Action Center after a quit.
fn activate(
    activator: &dyn ToastActivator,
    may_change_machine: bool,
    owns: bool,
    exe: Option<&Path>,
    register_name: impl FnOnce() -> Result<(), String>,
    focus: Arc<dyn Fn(String) + Send + Sync>,
) -> Activated {
    if !may_change_machine {
        return Activated { permission: NotificationPermission::Verification, served: None, errors: Vec::new() };
    }
    let mut errors = Vec::new();
    let registered = match (owns, exe) {
        // A platform with no toast activator (the Mac: its notifications' clicks come back through the notifier) registers the
        // name its own way (`register_notifications`: the Mac asks for authorization), even for the installed release (review item 2).
        (true, Some(exe)) => match activator.register(AUMID, CLSID, exe) {
            Err(error) if error.is_unsupported() => register_name(),
            registered => registered.map_err(|error| error.to_string()),
        },
        (true, None) => Err("the app's own executable is not known".to_string()),
        (false, _) => register_name(),
    };
    let permission = match registered {
        Ok(()) => NotificationPermission::Allowed,
        Err(error) => {
            errors.push(format!("not registered: {error}"));
            NotificationPermission::NeedPermission
        }
    };
    let callback: mighty_platform::ToastActivation = Arc::new(move |_aumid: &str, arguments: &str| {
        if let Some(pane) = session_id(arguments) {
            focus(pane);
        }
    });
    let served = match activator.serve(CLSID, callback) {
        Ok(served) => Some(served),
        // Nothing to serve where there is no activator.
        Err(error) if error.is_unsupported() => None,
        Err(error) => {
            errors.push(format!("the activator is not served: {error}"));
            None
        }
    };
    Activated { permission, served, errors }
}

/// C# `InitNotifierAsync` (never in a smoke): registered with the OS (the toast activator, W9-F) where the app may change the machine,
/// and a click on a toast — a live one through the notifier, one left in Action Center through the activator — focuses its pane.
pub fn start(host: &Arc<Host>, platform: &mighty_platform::Platform) {
    let weak = Arc::downgrade(host);
    let focus_pane: Arc<dyn Fn(String) + Send + Sync> = Arc::new(move |pane: String| {
        if let Some(host) = weak.upgrade() {
            // Off COM's thread, which waits for the call to return.
            tauri::async_runtime::spawn_blocking(move || focus(&host, &pane));
        }
    });
    let exe = std::env::current_exe().ok();
    let activated = activate(
        platform.activator.as_ref(),
        host.may_change_machine(),
        crate::startup::owns_registrations(),
        exe.as_deref(),
        crate::platform::register_notifications,
        focus_pane,
    );
    for error in &activated.errors {
        host.log.line(&format!("notifications: {error}"));
    }
    with(|state| {
        state.permission = activated.permission;
        state.served = activated.served;
        state.asked = None;
    });
    refresh();
    if let Some(ports) = host.app.try_state::<crate::platform::Ports>() {
        let to = host.clone();
        ports.notifier.on_activated(Arc::new(move |pane| {
            if let Some(pane) = pane {
                focus(&to, &pane);
            }
        }));
    }
}

/// `notifications::permission`: what the companion box's permission row says. Where the OS keeps its own answer (the Mac), it is
/// asked again in the background (at most every 2 s) and the next read has it.
pub fn permission(_host: &Host) -> NotificationPermission {
    refresh();
    with(|state| state.permission)
}

/// The OS's notification settings read again (the Mac's `getNotificationSettings`; nothing on Windows, whose answer is the
/// registration's).
fn refresh() {
    let due = with(|state| {
        let due = state.permission != NotificationPermission::Verification
            && state.asked.is_none_or(|at| at.elapsed() >= std::time::Duration::from_secs(2));
        if due {
            state.asked = Some(std::time::Instant::now());
        }
        due
    });
    if due {
        tauri::async_runtime::spawn(async {
            if let Some(authorized) = crate::platform::notification_authorization().await {
                with(|state| state.permission = apply_status(state.permission, authorized));
            }
        });
    }
}

/// The permission row's words (C# `BuildCompanionSection`).
pub fn permission_text(locale: &Locale, permission: NotificationPermission) -> String {
    notifications::text(
        locale,
        match permission {
            NotificationPermission::Allowed => "StatusAllowed",
            NotificationPermission::NeedPermission => "StatusNeedPermission",
            NotificationPermission::Denied => "StatusDenied",
            NotificationPermission::Verification => "StatusVerificationMode",
        },
    )
}

/// C# `HandleRunEventForNotification` (the last thing the window does with a run event): a toast when the decision says so and the app
/// may notify; its body names the pane's title alone.
pub fn on_status(host: &Arc<Host>, session_id: &str, status: Option<&str>) {
    if host.closing() {
        return;
    }
    let (title, run_title) = {
        let view = host.view();
        let enabled = view.snapshot.completion_notifications_enabled;
        let title = with(|state| state.decision.process("status", status, session_id, &view.snapshot, enabled));
        let run_title = view.snapshot.session(session_id).map_or_else(|| session_id.to_string(), |s| s.title().to_string());
        (title, run_title)
    };
    if title.is_none() || permission(host) != NotificationPermission::Allowed {
        return;
    }
    let Some(ports) = host.app.try_state::<crate::platform::Ports>() else { return };
    let notifier = ports.notifier.clone();
    let notification = completion(&host.locale, session_id, &run_title);
    let log = host.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(error) = notifier.notify(notification).await {
            log.log.line(&format!("notifications: not shown: {error}"));
        }
    });
}

/// The completion notification of a pane (C# `HandleRunEventForNotification`'s toast): the localized title over "{title} has
/// finished.", the pane's id for its click, nothing of its prompt or folder.
pub fn completion(locale: &Locale, session_id: &str, run_title: &str) -> Notification {
    Notification {
        title: notifications::text(locale, "NotificationTitle"),
        body: notifications::body(locale, run_title),
        pane_id: Some(session_id.to_string()),
    }
}

/// C# `FocusSession`: the pane's window forward, `select{paneId}` pushed, the pane selected.
pub fn focus(host: &Arc<Host>, pane: &str) {
    if host.view().snapshot.session(pane).is_none() {
        return;
    }
    if let Some(window) = host.app.get_webview_window(crate::host::WINDOW) {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
    let mut view = host.view();
    let pane_json = serde_json::to_string(pane).unwrap_or_default();
    host.push(&view, "select", format!("{{\"kind\":\"select\",\"paneId\":{pane_json}}}"));
    if let Err(error) = crate::frame::select_session(&mut view, pane) {
        view.error = Some(error);
    }
}

/// The live smoke's `completionNotification` (C# `RunCompletionNotificationSmoke`): a smoke never registers with the OS (it would write
/// the user's registry), so it is unsupported there and skipped, as on CI's runners. Where it is supported, the fixture's toast is the
/// one a run's completion shows (the localized `NotificationTitle`, the body over `FixtureTitle`), and a send that fails is the smoke's
/// failure (the error comes back, C#'s throw), never a skip.
#[cfg(feature = "dev-harness")]
pub async fn smoke(host: &Arc<Host>) -> Result<SmokeOutcome, String> {
    let supported = host.may_change_machine();
    notifications::smoke_run(
        supported,
        async || crate::platform::register_notifications().err(),
        async |run_title: &str, id: &str| -> Result<(), String> {
            let ports = host.app.try_state::<crate::platform::Ports>().ok_or("no notifier")?;
            let notification = Notification {
                title: notifications::text(&host.locale, "NotificationTitle"),
                body: notifications::body(&host.locale, run_title),
                pane_id: Some(id.into()),
            };
            ports.notifier.notify(notification).await.map_err(|e| e.to_string())
        },
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    use mighty_platform::fake::RecordingActivator;

    type Focused = Arc<Mutex<Vec<String>>>;

    fn recording() -> (Focused, Arc<dyn Fn(String) + Send + Sync>) {
        let focused: Focused = Arc::default();
        let into = focused.clone();
        (focused, Arc::new(move |pane: String| into.lock().unwrap().push(pane)))
    }

    fn named() -> Result<(), String> {
        Ok(())
    }

    /// An activator the OS refuses (Windows' registry denied, the class not served).
    struct Refusing;

    impl ToastActivator for Refusing {
        fn register(&self, _aumid: &str, _clsid: &str, _exe: &Path) -> mighty_platform::Result<()> {
            Err(mighty_platform::PlatformError::Refused("registry: denied".into()))
        }
        fn serve(&self, _clsid: &str, _callback: mighty_platform::ToastActivation) -> mighty_platform::Result<Subscription> {
            Err(mighty_platform::PlatformError::Refused("CoRegisterClassObject: denied".into()))
        }
    }

    /// Where the app may not change the machine (a development build without the flag, `--read-only`, a smoke), nothing is registered
    /// or served: the verification mode.
    #[test]
    fn without_the_right_nothing_is_registered_or_served() {
        let activator = RecordingActivator::default();
        let (focused, focus) = recording();
        let activated = activate(&activator, false, true, Some(Path::new(r"C:\app\MightyCode.exe")), || panic!("no name"), focus);
        assert_eq!(activated.permission, NotificationPermission::Verification);
        assert!(activated.served.is_none() && activator.registered().is_empty());
        activator.activate(AUMID, "sessionId=x");
        assert!(focused.lock().unwrap().is_empty(), "nothing served, nothing focused");
    }

    /// The installed release on the default profile: the AUMID registered with the activator class started from this exe, the class
    /// served, and an activation that names `sessionId=%2F…` focuses the decoded pane; once the subscription drops, a click reaches
    /// nothing.
    #[test]
    fn an_activation_focuses_the_decoded_session() {
        let activator = RecordingActivator::default();
        let (focused, focus) = recording();
        let exe = Path::new(r"C:\Users\me\AppData\Local\Programs\MightyCode\MightyCode.exe");
        let activated = activate(&activator, true, true, Some(exe), || panic!("the activator registers the name too"), focus);
        assert_eq!(activated.permission, NotificationPermission::Allowed);
        assert!(activated.errors.is_empty());
        assert_eq!(activator.registered(), [(AUMID.to_string(), CLSID.to_string(), exe.to_path_buf())]);
        activator.activate(AUMID, "sessionId=%2Fpane+1");
        activator.activate(AUMID, "launch=other");
        assert_eq!(*focused.lock().unwrap(), ["/pane 1"], "the decoded pane; arguments without a session focus nothing");
        drop(activated.served);
        activator.activate(AUMID, "sessionId=after");
        assert_eq!(focused.lock().unwrap().len(), 1);
    }

    /// Review L1: any other writing run (a development build with the flag, `--profile`, a release candidate run with the flag from
    /// elsewhere) registers the AUMID's name alone, never the activator's exe, and may still serve the class while it runs.
    #[test]
    fn another_writing_run_registers_the_name_but_never_the_activator() {
        let activator = RecordingActivator::default();
        let (focused, focus) = recording();
        let mut asked = false;
        let activated = activate(
            &activator,
            true,
            false,
            Some(Path::new(r"C:\Users\me\Downloads\MightyCode.exe")),
            || {
                asked = true;
                Ok(())
            },
            focus,
        );
        assert!(asked && activator.registered().is_empty(), "the name only");
        assert_eq!(activated.permission, NotificationPermission::Allowed);
        activator.activate(AUMID, "sessionId=p");
        assert_eq!(*focused.lock().unwrap(), ["p"], "served while it runs");
        let (_, focus) = recording();
        let refused = activate(&activator, true, false, None, || Err("registry: denied".into()), focus);
        assert_eq!(refused.permission, NotificationPermission::NeedPermission);
    }

    /// A registration the OS refuses needs the user's permission (and says why); the class is still served for a click.
    #[test]
    fn a_refused_registration_needs_permission() {
        let (_, focus) = recording();
        let exe = Path::new(r"C:\app\MightyCode.exe");
        let activated = activate(&Refusing, true, true, Some(exe), named, focus.clone());
        assert_eq!(activated.permission, NotificationPermission::NeedPermission);
        assert_eq!(activated.errors.len(), 2, "{:?}", activated.errors);
        let activated = activate(&RecordingActivator::default(), true, true, None, named, focus);
        assert_eq!(activated.permission, NotificationPermission::NeedPermission, "no exe to start: not registered");
        assert!(activated.served.is_some());
    }

    /// Review item 2: where the platform has no toast activator (the Mac), even the installed release registers its notifications its
    /// own way (`register_notifications`, the authorization request) and serves nothing, with no error; and the OS's answer then names
    /// the row (Swift: authorized, denied, or not asked yet), never in the verification mode.
    #[test]
    fn without_an_activator_the_installed_release_asks_the_os() {
        let (_, focus) = recording();
        let mut asked = false;
        let activated = activate(
            &mighty_platform::unsupported::Unsupported,
            true,
            true,
            Some(Path::new("/Applications/MightyCode.app/Contents/MacOS/MightyCode")),
            || {
                asked = true;
                Ok(())
            },
            focus.clone(),
        );
        assert!(asked, "the name registered the platform's way");
        assert!(activated.errors.is_empty() && activated.served.is_none(), "{:?}", activated.errors);
        assert_eq!(activated.permission, NotificationPermission::Allowed);
        let refused = activate(&mighty_platform::unsupported::Unsupported, true, true, Some(Path::new("/x")), || Err("no".into()), focus);
        assert_eq!(refused.permission, NotificationPermission::NeedPermission);
        assert_eq!(apply_status(NotificationPermission::Allowed, Some(false)), NotificationPermission::Denied);
        assert_eq!(apply_status(NotificationPermission::Allowed, None), NotificationPermission::NeedPermission);
        assert_eq!(apply_status(NotificationPermission::NeedPermission, Some(true)), NotificationPermission::Allowed);
        assert_eq!(apply_status(NotificationPermission::Verification, Some(true)), NotificationPermission::Verification);
    }

    #[test]
    fn the_permission_row_reads_each_state() {
        let ko = Locale::new("ko");
        assert_eq!(permission_text(&ko, NotificationPermission::Verification), "검증 모드");
        assert_eq!(permission_text(&ko, NotificationPermission::Allowed), "허용됨");
        assert_eq!(permission_text(&ko, NotificationPermission::NeedPermission), "권한 필요");
        assert_eq!(permission_text(&Locale::new("en"), NotificationPermission::Denied), "Allow notifications in system settings");
    }
}
