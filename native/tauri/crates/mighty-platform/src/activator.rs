//! The toast activator (W9, plan §1.8, D10; W6 review M3; C# `MainWindow.CompletionNotification.cs`, which used the Windows App SDK's
//! `AppNotificationManager`). Owner: W9-F.
//!
//! A click on a completion toast left in Action Center after the app quit starts the app through COM: `register` writes the AUMID's
//! `CustomActivator` and the CLSID's `LocalServer32` (`"<exe>" -Embedding`) under HKCU — a change to the machine, so only where the app
//! may change it, never in a test or a smoke — and `serve` registers the running app's class object (`CoRegisterClassObject`,
//! `REGCLS_MULTIPLEUSE`) for the process's life, so COM hands `INotificationActivationCallback::Activate(aumid, args)` to it, and starts
//! `MightyCode.exe -Embedding` only when no instance has it. The Mac answers `Unsupported` until its stage.

use std::path::Path;
use std::sync::Arc;

use crate::{Result, Subscription};

/// One activation: the AUMID and the toast's arguments as COM handed them (`sessionId=<url-encoded>`).
pub type ToastActivation = Arc<dyn Fn(&str, &str) + Send + Sync>;

pub trait ToastActivator: Send + Sync {
    /// Registers `aumid` with the activator `clsid` (`{…}`) started as `"<exe>" -Embedding` (HKCU).
    fn register(&self, aumid: &str, clsid: &str, exe: &Path) -> Result<()>;
    /// Serves `clsid`'s class object until the subscription is dropped, calling `callback` with each activation.
    fn serve(&self, clsid: &str, callback: ToastActivation) -> Result<Subscription>;
}
