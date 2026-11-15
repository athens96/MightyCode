//! The toast activator on Windows (W9-F; plan §1.8, D10; W6 review M3). C# used the Windows App SDK's `AppNotificationManager.Register`
//! (`MainWindow.CompletionNotification.cs`); an unpackaged Win32 app does the same by hand:
//!
//! - [`ToastActivator::register`] writes HKCU `Software\Classes\AppUserModelId\<aumid>` (`DisplayName`, `CustomActivator={CLSID}`, and
//!   `IconUri` when the app's icon file is beside the exe) and `Software\Classes\CLSID\{CLSID}\LocalServer32 = "<exe>" -Embedding` — a
//!   change to the machine, which the host makes only where `Host::may_change_machine` allows, never in a test or a smoke (the tests
//!   write a key of their own, [`register_under`]);
//! - [`ToastActivator::serve`] registers the running app's class object (`CoRegisterClassObject`, `REGCLS_MULTIPLEUSE`) from a thread
//!   of its own in the multithreaded apartment until the subscription drops, so COM hands a click on a toast left in Action Center to
//!   `INotificationActivationCallback::Activate(aumid, args)` here, and starts `MightyCode.exe -Embedding` only when no instance has
//!   the class (no single-instance guard is needed, plan D9).

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::sync::mpsc;

use mighty_platform::{PlatformError, Result, Subscription, ToastActivation, ToastActivator};
use windows::Win32::Foundation::{CLASS_E_NOAGGREGATION, E_POINTER};
use windows::Win32::System::Com::{
    CLSCTX_LOCAL_SERVER, COINIT_MULTITHREADED, CoInitializeEx, CoRegisterClassObject, CoRevokeClassObject, CoUninitialize, IClassFactory,
    IClassFactory_Impl, REGCLS_MULTIPLEUSE,
};
use windows::Win32::UI::Notifications::{
    INotificationActivationCallback, INotificationActivationCallback_Impl, NOTIFICATION_USER_INPUT_DATA,
};
use windows::core::{BOOL, GUID, IUnknown, Interface, PCWSTR, Ref, implement};

use crate::registry::CurrentUserRegistry;

/// Where the registration goes under HKEY_CURRENT_USER.
pub const CLASSES: &str = r"Software\Classes";

/// The name Action Center shows for the app's toasts.
pub const DISPLAY_NAME: &str = "Mighty Code";

/// The Windows toast activator.
#[derive(Debug, Clone, Copy, Default)]
pub struct ComActivator;

/// `{XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX}` as a GUID (braces required, as the registry writes a class id).
fn class_id(clsid: &str) -> Result<GUID> {
    clsid
        .strip_prefix('{')
        .and_then(|rest| rest.strip_suffix('}'))
        .and_then(|inner| GUID::try_from(inner).ok())
        .ok_or_else(|| PlatformError::Invalid(format!("not a class id: {clsid}")))
}

/// The icon Action Center shows beside the app's toasts, when a file of it ships beside the exe (C#'s layout kept it in `Assets\`).
fn icon(exe: &Path) -> Option<std::path::PathBuf> {
    let folder = exe.parent()?;
    [folder.join("Assets").join("MightyCode.ico"), folder.join("MightyCode.ico")].into_iter().find(|path| path.is_file())
}

/// [`ToastActivator::register`] with the keys under `classes` (`Software\Classes` in the app; a test's own key in tests).
pub fn register_under(classes: &str, aumid: &str, clsid: &str, exe: &Path) -> Result<()> {
    class_id(clsid)?;
    if aumid.is_empty() || aumid.contains('\\') {
        return Err(PlatformError::Invalid(format!("not an AppUserModelID: {aumid}")));
    }
    if !exe.is_absolute() {
        return Err(PlatformError::Invalid("the activator's executable needs an absolute path".into()));
    }
    let registry = CurrentUserRegistry::new();
    let refused = |error: std::io::Error| PlatformError::Refused(format!("registry: {error}"));
    let server = format!(r"{classes}\CLSID\{clsid}\LocalServer32");
    // The exe's path unit for unit (W9 review L5): a path that is not valid Unicode is never replaced through a lossy `String`.
    let mut command = std::ffi::OsString::from("\"");
    command.push(exe.as_os_str());
    command.push("\" -Embedding");
    registry.set_os_text(&server, "", &command).map_err(refused)?;
    let identity = format!(r"{classes}\AppUserModelId\{aumid}");
    registry.set_text(&identity, "DisplayName", DISPLAY_NAME).map_err(refused)?;
    if let Some(icon) = icon(exe) {
        registry.set_os_text(&identity, "IconUri", icon.as_os_str()).map_err(refused)?;
    }
    registry.set_text(&identity, "CustomActivator", clsid).map_err(refused)?;
    Ok(())
}

impl ToastActivator for ComActivator {
    fn register(&self, aumid: &str, clsid: &str, exe: &Path) -> Result<()> {
        register_under(CLASSES, aumid, clsid, exe)
    }

    fn serve(&self, clsid: &str, callback: ToastActivation) -> Result<Subscription> {
        let clsid = class_id(clsid)?;
        let (ready, registered) = mpsc::channel::<Result<()>>();
        let (stop, stopped) = mpsc::channel::<()>();
        let thread = std::thread::Builder::new()
            .name("toast-activator".into())
            .spawn(move || {
                // SAFETY: this thread's own apartment, left below.
                let entered = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
                let factory: IClassFactory = Factory { callback }.into();
                // SAFETY: a class id and a live class object; the cookie is revoked below on this thread.
                match unsafe { CoRegisterClassObject(&clsid, &factory, CLSCTX_LOCAL_SERVER, REGCLS_MULTIPLEUSE) } {
                    Ok(cookie) => {
                        let _ = ready.send(Ok(()));
                        // Until the subscription drops (its sender dropped).
                        let _ = stopped.recv();
                        // SAFETY: the cookie CoRegisterClassObject gave on this thread.
                        let _ = unsafe { CoRevokeClassObject(cookie) };
                    }
                    Err(error) => {
                        let _ = ready.send(Err(PlatformError::Refused(format!("CoRegisterClassObject: {error}"))));
                    }
                }
                drop(factory);
                if entered {
                    // SAFETY: paired with the CoInitializeEx above on this thread.
                    unsafe { CoUninitialize() };
                }
            })
            .map_err(|error| PlatformError::Refused(format!("the activator's thread: {error}")))?;
        registered.recv().map_err(|_| PlatformError::Refused("the activator's thread ended".into()))??;
        Ok(Subscription::new(move || {
            drop(stop);
            let _ = thread.join();
        }))
    }
}

/// The class object COM asks for an activation callback.
#[implement(IClassFactory)]
struct Factory {
    callback: ToastActivation,
}

impl IClassFactory_Impl for Factory_Impl {
    fn CreateInstance(&self, outer: Ref<'_, IUnknown>, iid: *const GUID, object: *mut *mut core::ffi::c_void) -> windows::core::Result<()> {
        if object.is_null() || iid.is_null() {
            return Err(E_POINTER.into());
        }
        // SAFETY: COM's out pointer, checked non-null above.
        unsafe { *object = std::ptr::null_mut() };
        if !outer.is_null() {
            return Err(CLASS_E_NOAGGREGATION.into());
        }
        let activation: INotificationActivationCallback = Activation { callback: self.callback.clone() }.into();
        // SAFETY: the interface id and out pointer as COM gave them (checked non-null above).
        unsafe { activation.query(iid, object).ok() }
    }

    fn LockServer(&self, _lock: BOOL) -> windows::core::Result<()> {
        Ok(())
    }
}

/// One activation callback: hands the AUMID and the toast's arguments to the served callback.
#[implement(INotificationActivationCallback)]
struct Activation {
    callback: ToastActivation,
}

/// A NUL-terminated wide string COM handed over, empty when null.
fn text(value: &PCWSTR) -> String {
    if value.is_null() {
        return String::new();
    }
    // SAFETY: a non-null NUL-terminated UTF-16 string that lives across the call.
    unsafe { value.to_string() }.unwrap_or_default()
}

impl INotificationActivationCallback_Impl for Activation_Impl {
    fn Activate(
        &self,
        aumid: &PCWSTR,
        arguments: &PCWSTR,
        _data: *const NOTIFICATION_USER_INPUT_DATA,
        _count: u32,
    ) -> windows::core::Result<()> {
        let (aumid, arguments) = (text(aumid), text(arguments));
        // A panic never crosses back into COM.
        let _ = catch_unwind(AssertUnwindSafe(|| (self.callback)(&aumid, &arguments)));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::registry::tests::TestKey;
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
    use windows::core::HSTRING;

    /// The registration: the AUMID names the activator, the class starts the exe with `-Embedding`, under a key of the test's own (never
    /// `Software\Classes`); a malformed class id, AUMID or relative exe writes nothing.
    #[test]
    fn the_registration_names_the_activator_and_its_exe() {
        let key = TestKey::new("activator");
        let classes = format!(r"{}\Classes", key.0);
        let exe = std::env::temp_dir().join("Mighty Code").join("MightyCode.exe");
        register_under(&classes, mighty_platform::activator::AUMID, mighty_platform::activator::CLSID, &exe).unwrap();
        let registry = CurrentUserRegistry::under(&classes);
        let utf16 = |text: &str| text.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect::<Vec<u8>>();
        let value =
            |path: &str, name: &str| registry.values(path).unwrap().into_iter().find(|value| value.name == name).map(|value| value.data);
        let identity = format!(r"AppUserModelId\{}", mighty_platform::activator::AUMID);
        assert_eq!(value(&identity, "DisplayName"), Some(utf16("Mighty Code")));
        assert_eq!(value(&identity, "CustomActivator"), Some(utf16(mighty_platform::activator::CLSID)));
        assert_eq!(value(&identity, "IconUri"), None, "no icon file beside the exe: none named");
        let server = format!(r"CLSID\{}\LocalServer32", mighty_platform::activator::CLSID);
        assert_eq!(value(&server, ""), Some(utf16(&format!("\"{}\" -Embedding", exe.display()))));

        let refused = format!(r"{}\Refused", key.0);
        assert!(register_under(&refused, "id", "E9E66915-2599-4971-9584-4825CAC5DEEE", &exe).is_err(), "braces required");
        assert!(register_under(&refused, "a\\b", mighty_platform::activator::CLSID, &exe).is_err());
        assert!(register_under(&refused, "id", mighty_platform::activator::CLSID, Path::new("MightyCode.exe")).is_err());
        assert!(!CurrentUserRegistry::new().key_exists(&refused), "a refused registration writes nothing");
    }

    /// The served class object takes an activation as COM hands it over: the callback gets the AUMID and the toast's arguments, and once
    /// the subscription drops the class is gone. In process (a class registered for a local server is this process's in-process one
    /// too), with a class id of the test's own, so nothing reaches the registry.
    #[test]
    fn a_served_class_hands_each_activation_to_the_callback() {
        let clsid_text = format!("{{{:?}}}", GUID::new().unwrap());
        let clsid = class_id(&clsid_text).unwrap();
        let seen: Arc<Mutex<Vec<(String, String)>>> = Arc::default();
        let into = seen.clone();
        let subscription = ComActivator
            .serve(&clsid_text, Arc::new(move |aumid: &str, arguments: &str| into.lock().unwrap().push((aumid.into(), arguments.into()))))
            .unwrap();
        // SAFETY: this test thread's apartment, left at the end.
        let entered = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
        // SAFETY: a class id this process serves; the interface asked for by its type.
        let activation: INotificationActivationCallback = unsafe { CoCreateInstance(&clsid, None, CLSCTX_INPROC_SERVER) }.unwrap();
        let (aumid, arguments) = (HSTRING::from(mighty_platform::activator::AUMID), HSTRING::from("sessionId=a+b%2Fc"));
        // SAFETY: two NUL-terminated strings that live across the call and no input data.
        unsafe { activation.Activate(&aumid, &arguments, &[]) }.unwrap();
        assert_eq!(*seen.lock().unwrap(), [(mighty_platform::activator::AUMID.to_string(), "sessionId=a+b%2Fc".to_string())]);
        drop(activation);
        drop(subscription);
        // SAFETY: as above.
        let gone: windows::core::Result<INotificationActivationCallback> = unsafe { CoCreateInstance(&clsid, None, CLSCTX_INPROC_SERVER) };
        assert!(gone.is_err(), "the class is revoked with the subscription");
        if entered {
            // SAFETY: paired with the CoInitializeEx above.
            unsafe { CoUninitialize() };
        }
    }
}
