//! A port an OS has not written yet: every operation answers [`PlatformError::Unsupported`], except where the port's contract
//! says it never fails (the CLI environment falls back to this process's, a missing secret is None). The OS crates hand this out
//! until their stage fills the port in, so the shared logic sees one typed refusal instead of a panic.

use std::path::{Path, PathBuf};

use crate::notify::ActivationHandler;
use crate::*;

/// Every port, unsupported.
#[derive(Debug, Clone, Copy, Default)]
pub struct Unsupported;

fn no<T>(port: &'static str, operation: &'static str) -> Result<T> {
    Err(PlatformError::unsupported(port, operation))
}

impl ProcessHost for Unsupported {
    fn spawn(&self, _request: SpawnRequest) -> BoxFuture<'_, Result<Child>> {
        Box::pin(async { no("ProcessHost", "spawn") })
    }
}

impl PtyHost for Unsupported {
    /// None known: an empty path.
    fn default_shell(&self) -> PathBuf {
        PathBuf::new()
    }
    fn shell_arguments(&self, _shell: &Path, _command: Option<&str>, _no_profile: bool) -> Vec<String> {
        vec![]
    }
    fn open(&self, _request: PtyRequest) -> BoxFuture<'_, Result<PtySession>> {
        Box::pin(async { no("PtyHost", "open") })
    }
}

/// The process's own environment, as a failed capture gives.
impl ShellEnvironment for Unsupported {
    fn resolve(&self, _workspace: Option<&Path>, _force: bool) -> BoxFuture<'_, CliEnvironment> {
        Box::pin(async { self.current() })
    }
    fn current(&self) -> CliEnvironment {
        CliEnvironment { values: std::env::vars().collect(), source: EnvironmentSource::ProcessFallback }
    }
    fn invalidate(&self, _workspace: Option<&Path>) {}
}

impl SecretVault for Unsupported {
    fn load(&self, _id: &str) -> Option<String> {
        None
    }
    fn save(&self, _id: &str, _secret: &str) -> Result<()> {
        no("SecretVault", "save")
    }
    fn delete(&self, _id: &str) -> Result<()> {
        no("SecretVault", "delete")
    }
}

impl PrefsStore for Unsupported {
    fn get(&self, _key: &str) -> Result<Option<PrefValue>> {
        no("PrefsStore", "get")
    }
    fn set(&self, _key: &str, _value: PrefValue) -> Result<()> {
        no("PrefsStore", "set")
    }
    fn remove(&self, _key: &str) -> Result<()> {
        no("PrefsStore", "remove")
    }
}

impl PairingStore for Unsupported {
    fn load(&self) -> Result<Option<PairingIdentity>> {
        no("PairingStore", "load")
    }
    fn save(&self, _identity: &PairingIdentity) -> Result<()> {
        no("PairingStore", "save")
    }
}

impl Notifier for Unsupported {
    fn notify(&self, _notification: Notification) -> BoxFuture<'_, Result<()>> {
        Box::pin(async { no("Notifier", "notify") })
    }
    fn on_activated(&self, _handler: ActivationHandler) {}
}

impl Firewall for Unsupported {
    fn read(&self) -> BoxFuture<'_, Result<FirewallRule>> {
        Box::pin(async { no("Firewall", "read") })
    }
    fn apply(&self, _ports: Vec<u16>) -> BoxFuture<'_, Result<FirewallChange>> {
        Box::pin(async { no("Firewall", "apply") })
    }
}

impl Clipboard for Unsupported {
    fn read_text(&self) -> Result<Option<String>> {
        no("Clipboard", "read_text")
    }
    fn write_text(&self, _text: &str) -> Result<()> {
        no("Clipboard", "write_text")
    }
}

impl UpdateInstaller for Unsupported {
    fn install_location(&self) -> Option<PathBuf> {
        None
    }
    fn hand_off(&self, _update: &StagedUpdate, _relaunch: bool) -> Result<()> {
        no("UpdateInstaller", "hand_off")
    }
    fn helper_mutex(&self, _name: &str) -> Result<Option<HelperMutex>> {
        no("UpdateInstaller", "helper_mutex")
    }
    fn wait_for_exit(&self, _pid: u32, _timeout: std::time::Duration) -> Result<bool> {
        no("UpdateInstaller", "wait_for_exit")
    }
    fn running_from(&self, _folder: &Path, _except_self: bool) -> Result<bool> {
        no("UpdateInstaller", "running_from")
    }
    /// None: no environment is built.
    fn fresh_environment(&self) -> Option<std::collections::BTreeMap<String, String>> {
        None
    }
    fn sync_display_version(&self, _app_directory: &Path, _version: &str) -> Result<bool> {
        no("UpdateInstaller", "sync_display_version")
    }
    fn refresh_renamed_install(&self, _app_directory: &Path) -> Result<()> {
        no("UpdateInstaller", "refresh_renamed_install")
    }
}

impl UpdateTransport for Unsupported {
    fn get_capped(&self, _url: &str, _cap: usize, _timeout: std::time::Duration) -> BoxFuture<'_, Result<UpdateResponse>> {
        Box::pin(async { no("UpdateTransport", "get_capped") })
    }
    fn download(&self, _request: DownloadRequest) -> BoxFuture<'_, Result<Downloaded>> {
        Box::pin(async { no("UpdateTransport", "download") })
    }
}

impl SessionWatch for Unsupported {
    fn on_ending(&self, _callback: SessionCallback) -> Result<Subscription> {
        no("SessionWatch", "on_ending")
    }
    fn since_last_input(&self) -> Result<std::time::Duration> {
        no("SessionWatch", "since_last_input")
    }
}

impl ToastActivator for Unsupported {
    fn register(&self, _aumid: &str, _clsid: &str, _exe: &Path) -> Result<()> {
        no("ToastActivator", "register")
    }
    fn serve(&self, _clsid: &str, _callback: ToastActivation) -> Result<Subscription> {
        no("ToastActivator", "serve")
    }
}

/// Opens nothing; says the page draws the terminal and the platform the browser (so the page reserves the slot).
impl SurfaceFactory for Unsupported {
    fn drawing(&self, kind: SurfaceKind) -> Drawing {
        match kind {
            SurfaceKind::Terminal => Drawing::Web,
            SurfaceKind::Browser => Drawing::Native,
        }
    }
    fn open_browser(&self, _options: BrowserOptions) -> BoxFuture<'_, Result<Box<dyn BrowserSurface>>> {
        Box::pin(async { no("SurfaceFactory", "open_browser") })
    }
    fn open_terminal(&self, _options: TerminalOptions) -> BoxFuture<'_, Result<Box<dyn TerminalSurface>>> {
        Box::pin(async { no("SurfaceFactory", "open_terminal") })
    }
    fn open_pet(&self) -> BoxFuture<'_, Result<Box<dyn PetWindow>>> {
        Box::pin(async { no("SurfaceFactory", "open_pet") })
    }
}

impl TerminalLauncher for Unsupported {
    fn open(&self, _launch: TerminalLaunch) -> Result<Box<dyn TerminalWindow>> {
        no("TerminalLauncher", "open")
    }
}

impl FontCatalog for Unsupported {
    fn families(&self) -> Result<Vec<FontFamily>> {
        no("FontCatalog", "families")
    }
}

impl HttpsSend for Unsupported {
    fn get(&self, _request: HttpsRequest, _max_body: usize) -> BoxFuture<'_, Result<HttpsResponse>> {
        Box::pin(async { no("HttpsSend", "get") })
    }
}

/// The Mac's Unix socket until its stage (W7 plan D4).
impl AgentIoListener for Unsupported {
    fn bind(&self) -> Result<String> {
        no("AgentIoListener", "bind")
    }
    fn accept(&self) -> BoxFuture<'_, Result<AgentIoConnection>> {
        Box::pin(async { no("AgentIoListener", "accept") })
    }
    fn close(&self) {}
}

impl AgentIoConnector for Unsupported {
    fn connect(&self, _name: &str, _timeout: std::time::Duration) -> BoxFuture<'_, Result<AgentIoConnection>> {
        Box::pin(async { no("AgentIoConnector", "connect") })
    }
}

impl DiskSpace for Unsupported {
    fn available_bytes(&self, _path: &Path) -> Result<u64> {
        no("DiskSpace", "available_bytes")
    }
}

/// No adapter, no listener, no notification until the OS's stage (W8-C on Windows, the Mac stage).
impl Network for Unsupported {
    fn adapters(&self) -> Result<Adapters> {
        no("Network", "adapters")
    }
    fn bind_exclusive(&self, _address: std::net::SocketAddr, _backlog: u32) -> Result<std::net::TcpListener> {
        no("Network", "bind_exclusive")
    }
    fn on_change(&self, _callback: NetworkCallback) -> Result<Subscription> {
        no("Network", "on_change")
    }
    fn on_resume(&self, _callback: NetworkCallback) -> Result<Subscription> {
        no("Network", "on_resume")
    }
}

impl ProxyResolver for Unsupported {
    fn resolve(&self, _url: &str) -> BoxFuture<'_, Result<Proxy>> {
        Box::pin(async { no("ProxyResolver", "resolve") })
    }
}

/// Screen share is paused on every OS (W8-F, D11).
impl ScreenCapture for Unsupported {
    fn displays(&self) -> Result<Vec<screen::ScreenDisplay>> {
        no("ScreenCapture", "displays")
    }
    fn capture(&self, _display: &str) -> BoxFuture<'_, Result<screen::ScreenFrame>> {
        Box::pin(async { no("ScreenCapture", "capture") })
    }
}

impl RemoteInput for Unsupported {
    fn send(&self, _display: &str, _event: screen::RemoteInputEvent) -> Result<()> {
        no("RemoteInput", "send")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn every_operation_is_a_typed_refusal() {
        let port = Unsupported;
        assert!(TerminalLauncher::open(&port, TerminalLaunch::default()).err().is_some_and(|e| e.is_unsupported()));
        assert!(port.families().unwrap_err().is_unsupported());
        let request = HttpsRequest { url: "https://api.anthropic.com/".into(), headers: vec![], timeout: std::time::Duration::ZERO };
        assert!(HttpsSend::get(&port, request.clone(), 0).await.unwrap_err().is_unsupported());
        assert!(HttpsSend::post(&port, request, b"{}".to_vec(), 0).await.unwrap_err().is_unsupported());
        assert!(port.adapters().unwrap_err().is_unsupported());
        assert!(port.bind_exclusive("127.0.0.1:0".parse().unwrap(), 1).unwrap_err().is_unsupported());
        assert!(port.on_change(NetworkCallback::new(|| {})).err().is_some_and(|e| e.is_unsupported()));
        assert!(ProxyResolver::resolve(&port, "https://example.invalid/").await.unwrap_err().is_unsupported());
        assert!(port.displays().unwrap_err().is_unsupported());
        assert!(SecretVault::save(&port, "id", "x").unwrap_err().is_unsupported());
        assert_eq!(SecretVault::load(&port, "id"), None);
        assert!(PrefsStore::get(&port, "k").unwrap_err().is_unsupported());
        assert!(Firewall::read(&port).await.unwrap_err().is_unsupported());
        assert_eq!(ShellEnvironment::resolve(&port, None, false).await.source, EnvironmentSource::ProcessFallback);
        assert!(port.read_text().unwrap_err().is_unsupported() && port.write_text("x").unwrap_err().is_unsupported());
        assert!(port.bind().unwrap_err().is_unsupported());
        assert!(port.accept().await.err().is_some_and(|e| e.is_unsupported()));
        assert!(port.connect("x", std::time::Duration::ZERO).await.err().is_some_and(|e| e.is_unsupported()));
        assert!(port.available_bytes(Path::new("x")).unwrap_err().is_unsupported());
        // W9's ports and the detached start (Step 0): every one a typed refusal on an OS that has not written it.
        let detached = DetachedRequest {
            program: PathBuf::from("x"),
            args: vec![],
            cwd: PathBuf::from("y"),
            environment: DetachedEnvironment::Inherit,
            hidden: true,
        };
        assert!(port.start_detached(detached).unwrap_err().is_unsupported());
        assert!(port.helper_mutex("Local\\x").unwrap_err().is_unsupported());
        assert!(port.wait_for_exit(1, std::time::Duration::ZERO).unwrap_err().is_unsupported());
        assert!(port.running_from(Path::new("x"), true).unwrap_err().is_unsupported());
        assert_eq!(port.fresh_environment(), None);
        assert!(port.sync_display_version(Path::new("x"), "0.5.0").unwrap_err().is_unsupported());
        assert!(port.refresh_renamed_install(Path::new("x")).unwrap_err().is_unsupported());
        assert!(port.get_capped("https://example.invalid/latest.json", 1, std::time::Duration::ZERO).await.unwrap_err().is_unsupported());
        assert!(port.since_last_input().unwrap_err().is_unsupported());
        assert!(port.on_ending(std::sync::Arc::new(|_| {})).err().is_some_and(|e| e.is_unsupported()));
        assert!(port.register("dev.mightycode.native", "{0}", Path::new("x")).unwrap_err().is_unsupported());
        assert!(port.serve("{0}", std::sync::Arc::new(|_, _| {})).err().is_some_and(|e| e.is_unsupported()));
        assert!(
            port.open_browser(BrowserOptions {
                pane_id: "p".into(),
                url: "about:blank".into(),
                data_dir: PathBuf::new(),
                bounds: DeviceRect::default(),
                listener: None,
            })
            .await
            .err()
            .is_some_and(|e| e.is_unsupported())
        );
    }
}
