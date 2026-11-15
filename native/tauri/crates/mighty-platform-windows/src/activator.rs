//! The toast activator on Windows (W9-F; plan §1.8, D10; W6 review M3): HKCU `Software\Classes\AppUserModelId\<aumid>`
//! (`DisplayName`, `IconUri`, `CustomActivator={CLSID}`) and `Software\Classes\CLSID\{CLSID}\LocalServer32 = "<exe>" -Embedding`, written
//! only where the app may change the machine; `CoRegisterClassObject(REGCLS_MULTIPLEUSE)` of a class factory whose
//! `INotificationActivationCallback::Activate(aumid, args)` calls the served callback, revoked when the subscription drops.
//!
//! Step 0: both answer `Unsupported`.

use std::path::Path;

use mighty_platform::{PlatformError, Result, Subscription, ToastActivation, ToastActivator};

/// The Windows toast activator (W9-F fills it).
#[derive(Debug, Clone, Copy, Default)]
pub struct ComActivator;

impl ToastActivator for ComActivator {
    fn register(&self, _aumid: &str, _clsid: &str, _exe: &Path) -> Result<()> {
        Err(PlatformError::unsupported("ToastActivator", "register"))
    }
    fn serve(&self, _clsid: &str, _callback: ToastActivation) -> Result<Subscription> {
        Err(PlatformError::unsupported("ToastActivator", "serve"))
    }
}
