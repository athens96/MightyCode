//! The ports between Mighty Code's shared logic and the operating system (docs/tauri-architecture.md §1–2).
//!
//! Every place the shared crates need the OS — start a process, open a pseudo console, read a secret, place a native view over the
//! page — is a trait here, with a written contract and a fake (`fake`) the shared tests run against, so they give the same result
//! on Windows and on the Mac. Each OS plugs in its own implementation (`mighty-platform-windows`, `mighty-platform-macos`);
//! `src-tauri/src/platform.rs` picks one by target, in one place.
//!
//! Rules (the architecture's): this crate has no `cfg(target_os)` and no OS call; a behaviour that differs on purpose between the
//! OSes is a named flag of [`PlatformProfile`], never an `if windows` in shared code; an operation an OS does not have (yet)
//! answers [`PlatformError::Unsupported`].

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

pub mod accounts;
pub mod activator;
pub mod agent_io;
pub mod clipboard;
pub mod console;
pub mod disk;
pub mod environment;
mod error;
pub mod fake;
pub mod files;
pub mod firewall;
pub mod fonts;
pub mod http;
pub mod net;
pub mod notify;
pub mod pairing;
pub mod paths;
pub mod pet;
pub mod prefs;
pub mod process;
pub mod profile;
pub mod proxy;
pub mod pty;
pub mod screen;
pub mod secrets;
pub mod session;
pub mod setup;
pub mod surface;
pub mod unsupported;
pub mod update;
pub mod update_transport;

pub use accounts::ClaudeCredentials;
pub use activator::{ToastActivation, ToastActivator};
pub use agent_io::{AgentIoConnection, AgentIoConnector, AgentIoListener, AgentIoStream};
pub use clipboard::Clipboard;
pub use console::{TerminalLaunch, TerminalLauncher, TerminalWindow};
pub use disk::DiskSpace;
pub use environment::{CliEnvironment, EnvironmentSource, ShellEnvironment};
pub use error::{PlatformError, Result};
pub use files::{ClipboardImage, Desktop, FileHandles, FilePorts, PathProbe, PdfPage, PdfRaster, SaveDialog, SaveRequest};
pub use firewall::{Firewall, FirewallChange, FirewallRule};
pub use fonts::{FontCatalog, FontFamily};
pub use http::{HttpsRequest, HttpsResponse, HttpsSend};
pub use net::{Adapter, Adapters, Network, NetworkCallback, Subscription};
pub use notify::{ActivationHandler, Notification, Notifier};
pub use pairing::{PairedDevice, PairingIdentity, PairingStore, PushRegistration};
pub use paths::{AppPaths, Paths, WebFolders};
pub use pet::PetScene;
pub use prefs::{PrefValue, PrefsStore};
pub use process::{Captured, Child, ChildControl, DetachedChild, DetachedEnvironment, DetachedRequest, ProcessHost, SpawnRequest, capture};
pub use profile::{
    AgentSignIn, CliUpdater, McpTransport, Os, PairingLayout, PathStyle, PlatformProfile, PluginListWhileReading, SettingsStore,
};
pub use proxy::{Proxy, ProxyResolver};
pub use pty::{PtyControl, PtyHost, PtyRequest, PtySession, PtySize, TerminalInput};
pub use screen::{RemoteInput, ScreenCapture};
pub use secrets::SecretVault;
pub use session::{SessionCallback, SessionEnding, SessionWatch};
pub use setup::SetupFiles;
pub use surface::{
    BrowserEvent, BrowserListener, BrowserOptions, BrowserSurface, DeviceRect, Drawing, LogicalPoint, LogicalRect, NativeSurface, PetEvent,
    PetPointer, PetWindow, SurfaceFactory, SurfaceKind, SurfaceState, TerminalEvent, TerminalListener, TerminalOptions, TerminalSurface,
};
pub use update::{HelperMutex, StagedUpdate, UpdateInstaller};
pub use update_transport::{DownloadProgress, DownloadRequest, Downloaded, UpdateResponse, UpdateTransport};

/// A boxed future, so the ports stay usable as trait objects (`Arc<dyn ProcessHost>`).
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Every port one OS implements, as the shared logic receives them. The surfaces need the app's window and come separately
/// ([`SurfaceFactory`]).
#[derive(Clone)]
pub struct Platform {
    pub profile: PlatformProfile,
    pub paths: Arc<dyn Paths>,
    pub processes: Arc<dyn ProcessHost>,
    pub ptys: Arc<dyn PtyHost>,
    pub environment: Arc<dyn ShellEnvironment>,
    pub secrets: Arc<dyn SecretVault>,
    pub prefs: Arc<dyn PrefsStore>,
    pub pairing: Arc<dyn PairingStore>,
    pub notifier: Arc<dyn Notifier>,
    pub firewall: Arc<dyn Firewall>,
    pub updates: Arc<dyn UpdateInstaller>,
    /// W8: the network's adapters, an exclusive listener, address changes and resume (the direct path's and remote sharing's).
    pub network: Arc<dyn Network>,
    /// W8: the system's proxy for a URL (the relay's WebSocket and push; remote v2 uses none).
    pub proxy: Arc<dyn ProxyResolver>,
    /// W8: HTTPS GET and POST (push; the account usage keeps its own guarded transport, src-tauri platform.rs `Ports`).
    pub https: Arc<dyn HttpsSend>,
    /// W9: the app update's HTTPS (no proxy, redirects handed back, a capped manifest read and a streamed download; W9-B).
    pub update_transport: Arc<dyn UpdateTransport>,
    /// W9: the session ending and the user's last input (the automatic update's; W9-C).
    pub session: Arc<dyn SessionWatch>,
    /// W9: the toast activator (AUMID, CLSID, `CoRegisterClassObject`; W9-F).
    pub activator: Arc<dyn ToastActivator>,
}

impl std::fmt::Debug for Platform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Platform").field("os", &self.profile.os).field("data_dir", &self.paths.data_dir()).finish_non_exhaustive()
    }
}
