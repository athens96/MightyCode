//! The one place the host chooses the OS's implementation of the ports (docs/tauri-architecture.md §1, rule 3): Windows builds
//! use `mighty-platform-windows`, Mac builds `mighty-platform-macos`. Nothing else in the host names either crate or asks which
//! OS it runs on; what differs on purpose reaches the shared logic as `Platform::profile`.

use std::path::PathBuf;
use std::sync::Arc;

use mighty_platform::{FontCatalog, HttpsSend, Platform, Result, SurfaceFactory, TerminalLauncher};

#[cfg(windows)]
use mighty_platform_windows as os;

#[cfg(target_os = "macos")]
use mighty_platform_macos as os;

#[cfg(not(any(windows, target_os = "macos")))]
compile_error!("Mighty Code's host builds for Windows and macOS only.");

/// Every port of this OS; `profile` is `--profile`, `identifier` the app's (tauri.conf.json), which names its local data folder.
pub fn platform(profile: Option<PathBuf>, identifier: &str) -> Result<Platform> {
    os::platform(profile, identifier)
}

/// The native views over `window`'s page; `csharp_web_data` when the browser panes' data is C#'s own folder (W9-F's adopted folder,
/// `paths::web_folders`), whose views open with C#'s environment options.
pub fn surfaces(window: tauri::Window, csharp_web_data: bool) -> Arc<dyn SurfaceFactory> {
    os::surfaces(window, csharp_web_data)
}

/// W9-C: the main window whose session messages `Platform::session` reports (Windows subclasses it; call on the window's thread). The
/// Mac's session port answers `Unsupported` until its stage, so there is nothing to hand it.
pub fn watch_session(window: &tauri::Window) -> Result<()> {
    #[cfg(windows)]
    return os::session::watch_window(window);
    #[cfg(not(windows))]
    {
        let _ = window;
        Ok(())
    }
}

/// The system clipboard's text (a terminal's copy and paste).
pub fn clipboard() -> Arc<dyn mighty_platform::Clipboard> {
    os::clipboard()
}

/// The agents' MCP listener (one per app, W7-C's agent_io.rs): Windows' named pipe (a random name); the Mac's Unix socket, under the
/// profile's data folder `data_dir` (M-C: `<data>/agent-io/io.sock`, as the Swift app's).
pub fn agent_io_listener(data_dir: &std::path::Path) -> Arc<dyn mighty_platform::AgentIoListener> {
    #[cfg(windows)]
    {
        let _ = data_dir;
        os::agent_io_listener()
    }
    #[cfg(target_os = "macos")]
    {
        os::agent_io_listener(data_dir)
    }
}

/// The exe's MCP modes' client of that listener (`--agent-io-mcp`, `--agent-delegation-mcp`, main.rs).
pub fn agent_io_connector() -> Arc<dyn mighty_platform::AgentIoConnector> {
    os::agent_io_connector()
}

/// A volume's free space (delegation's worktree check, W7-B; the smoke gives unlimited space instead).
#[allow(dead_code, reason = "delegation.rs (W7-D) hands it to mighty-delegation's worktree maker")]
pub fn disk_space() -> Arc<dyn mighty_platform::DiskSpace> {
    os::disk_space()
}

/// The process host alone, for the exe's `--fake-claude` mode, which serves before any profile is read (main.rs).
pub fn process_host() -> Arc<dyn mighty_platform::ProcessHost> {
    os::process_host()
}

/// The update installer alone (the mutex, the process wait, the fresh environment, the uninstall entry), for the exe's
/// `--update-helper` mode, which runs before any profile is read (update_helper.rs, W9-B; with [`process_host`]'s `start_detached`).
#[allow(dead_code, reason = "update_helper.rs (W9-B) runs the helper with it")]
pub fn update_installer() -> Arc<dyn mighty_platform::UpdateInstaller> {
    os::update_installer()
}

/// W6's ports, managed by the app (`host.app.state::<Ports>()`): a terminal window that outlives the app (sign-ins, the setup
/// screen's install), the installed font families (Settings › 글자) and the account usage's HTTPS GET. A smoke gets the recording
/// fakes for the two that reach outside the app (no window opens, no request leaves), and the real font list, which only reads.
#[allow(dead_code, reason = "the W6 parts read them: W6-B and W6-C the terminal window, W6-C the HTTPS GET")]
pub struct Ports {
    pub terminals: Arc<dyn TerminalLauncher>,
    pub fonts: Arc<dyn FontCatalog>,
    pub https: Arc<dyn HttpsSend>,
    /// The completion notification (a smoke's records them: no toast shows).
    pub notifier: Arc<dyn mighty_platform::Notifier>,
}

pub fn ports(smoke: bool) -> Ports {
    if smoke {
        Ports {
            terminals: Arc::new(mighty_platform::fake::RecordingTerminalLauncher::default()),
            fonts: os::font_catalog(),
            https: Arc::new(mighty_platform::fake::RecordingHttps::default()),
            notifier: Arc::new(mighty_platform::fake::RecordingNotifier::default()),
        }
    } else {
        Ports { terminals: os::terminal_launcher(), fonts: os::font_catalog(), https: os::https_get(), notifier: os::notifier() }
    }
}

/// W8's ports as the remote parts get them (mobile.rs, direct.rs, remote.rs): the platform's own, except in a smoke, which gets the
/// fakes for every one that reaches outside the process — a network that binds loopback only and never calls back, a firewall that
/// records (nothing elevates), HTTPS that records (no push leaves) and a direct proxy. The pairing store is the platform's (the profile's
/// own file; the smoke's profile in a smoke).
#[allow(dead_code, reason = "W8-A, W8-C and W8-D read them")]
pub struct RemotePorts {
    pub network: Arc<dyn mighty_platform::Network>,
    pub firewall: Arc<dyn mighty_platform::Firewall>,
    pub https: Arc<dyn HttpsSend>,
    pub proxy: Arc<dyn mighty_platform::ProxyResolver>,
    pub pairing: Arc<dyn mighty_platform::PairingStore>,
}

#[allow(dead_code, reason = "W8-A, W8-C and W8-D read them")]
pub fn remote_ports(platform: &Platform, smoke: bool) -> RemotePorts {
    if smoke {
        RemotePorts {
            network: Arc::new(mighty_platform::fake::FakeNetwork::default()),
            firewall: Arc::new(mighty_platform::fake::FakeFirewall::default()),
            https: Arc::new(mighty_platform::fake::RecordingHttps::default()),
            proxy: Arc::new(mighty_platform::fake::FakeProxy::default()),
            pairing: platform.pairing.clone(),
        }
    } else {
        RemotePorts {
            network: platform.network.clone(),
            firewall: platform.firewall.clone(),
            https: platform.https.clone(),
            proxy: platform.proxy.clone(),
            pairing: platform.pairing.clone(),
        }
    }
}

/// Screen share's capture and input (W8-F, D11): paused on every OS, so neither OS crate has them yet (after 0.5).
#[allow(dead_code, reason = "W8-F's screen share reads them once its switch is on")]
pub fn screen_capture() -> Arc<dyn mighty_platform::ScreenCapture> {
    Arc::new(mighty_platform::unsupported::Unsupported)
}

#[allow(dead_code, reason = "W8-F's screen share reads them once its switch is on")]
pub fn remote_input() -> Arc<dyn mighty_platform::RemoteInput> {
    Arc::new(mighty_platform::unsupported::Unsupported)
}

/// C# `DateTimeOffset.TryParse(updatedAt) … ToLocalTime().ToString("T", CurrentCulture)`: a reported time in the user's long time
/// format (the session info popover's "received"); none when it does not read or the OS does not say.
pub fn long_time(at: &str) -> Option<String> {
    let parsed = chrono::DateTime::parse_from_rfc3339(at).ok()?;
    let system = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(u64::try_from(parsed.timestamp_millis()).ok()?);
    os::long_time(system)
}

/// The setup screen's files port (setup.rs, W6-B): the script folder and its ACL, and whether the app runs elevated (Windows'
/// `WindowsSetupFiles`; the Mac `Unsupported`).
#[cfg(windows)]
#[allow(dead_code, reason = "setup.rs's files_port (W6-B) switches to it at integration")]
pub fn setup_files() -> Arc<dyn mighty_platform::SetupFiles> {
    Arc::new(os::WindowsSetupFiles)
}

#[cfg(target_os = "macos")]
#[allow(dead_code, reason = "setup.rs's files_port (W6-B) switches to it at integration")]
pub fn setup_files() -> Arc<dyn mighty_platform::SetupFiles> {
    os::setup::setup_files()
}

/// The page's whole picture as PNG bytes (the help capture, W9-H): WebView2's `CapturePreview` on Windows; WKWebView's snapshot on the
/// Mac (`mighty-platform-macos` `page`, M-G).
pub async fn page_picture(page: &tauri::Webview) -> std::result::Result<Vec<u8>, String> {
    #[cfg(windows)]
    return os::page::capture_preview(page).await.map_err(|e| e.to_string());
    #[cfg(target_os = "macos")]
    return os::page::snapshot(page, None).await;
}

/// A DevTools protocol call on the page (the help capture's clipped pictures and reduced motion, W9-H): Windows' WebView2 only; the
/// Mac's WKWebView has none (M-G clips its snapshot instead).
#[cfg(windows)]
pub async fn page_devtools(page: &tauri::Webview, method: &str, arguments: &str) -> std::result::Result<String, String> {
    os::page::devtools(page, method, arguments).await.map_err(|e| e.to_string())
}

/// Where a release build counts as installed (the write policy, startup.rs): the folder the setup program installs into on Windows
/// (`%LOCALAPPDATA%\Programs\MightyCode`, the known folder's); on the Mac the `.app`s M-H names (plan D7; none before then).
pub fn install_locations() -> Vec<PathBuf> {
    #[cfg(windows)]
    return os::install_folder().into_iter().collect();
    #[cfg(target_os = "macos")]
    return os::update::install_locations();
}

/// The registry RenameMigration reads and writes (C# `CurrentUserRegistry`): HKEY_CURRENT_USER on Windows; a registry without keys
/// on the Mac, whose half of the migration (folders, the defaults domain) is M-B's seams of `mighty-core`'s `rename_migration`.
pub fn rename_registry() -> Arc<dyn mighty_core::rename_migration::RegistryStore> {
    #[cfg(windows)]
    return Arc::new(CurrentUser(os::registry::CurrentUserRegistry::new()));
    #[cfg(target_os = "macos")]
    return Arc::new(mighty_core::rename_migration::NoRegistry);
}

/// The Windows registry as RenameMigration reads and writes it.
#[cfg(windows)]
struct CurrentUser(os::registry::CurrentUserRegistry);

#[cfg(windows)]
impl mighty_core::rename_migration::RegistryStore for CurrentUser {
    fn key_exists(&self, path: &str) -> bool {
        self.0.key_exists(path)
    }
    fn sub_key_names(&self, path: &str) -> std::io::Result<Vec<String>> {
        self.0.sub_key_names(path)
    }
    fn values(&self, path: &str) -> std::io::Result<Vec<mighty_core::rename_migration::RegistryValue>> {
        let values = self.0.values(path)?.into_iter();
        Ok(values
            .map(|value| mighty_core::rename_migration::RegistryValue { name: value.name, kind: value.kind, data: value.data })
            .collect())
    }
    fn create_key(&self, path: &str) -> std::io::Result<()> {
        self.0.create_key(path)
    }
    fn set_value(&self, path: &str, value: &mighty_core::rename_migration::RegistryValue) -> std::io::Result<()> {
        self.0.set_value(path, &value.name, value.kind, &value.data)
    }
    fn delete_tree(&self, path: &str) -> std::io::Result<()> {
        self.0.delete_tree(path)
    }
}

/// The style trust store's Windows checks (C# `Private`/`Restrict`, styles.rs `WindowsAccess`): not a reparse point, this user the
/// owner, no other account than LocalSystem or the Administrators able to write it.
#[cfg(windows)]
pub fn trust_private(path: &std::path::Path) -> bool {
    os::setup_files::private(path)
}

/// Written with a protected list of this user alone (styles.rs `WindowsAccess`).
#[cfg(windows)]
pub fn trust_restrict(path: &std::path::Path, directory: bool) -> std::io::Result<()> {
    os::setup_files::restrict(path, directory).map_err(|e| std::io::Error::other(e.to_string()))
}

/// Registers the app for its notifications with the OS (Windows: the AUMID under HKCU); the caller decides whether it may write.
pub fn register_notifications() -> std::result::Result<(), String> {
    os::register_notifications()
}

/// An http(s) or mailto address opened with the user's default handler (C# `Process.Start` with `UseShellExecute`).
pub fn open_url(url: &str) -> std::result::Result<(), String> {
    os::open_url(url)
}

/// The user's number format (C# `CultureInfo.CurrentCulture`'s group separator and sizes), when the OS says.
pub fn number_grouping() -> Option<(String, Vec<usize>)> {
    os::number_grouping()
}

/// The files pane's ports (an open file's handle, a PDF's first page, show in the file manager, open with the default app, the
/// clipboard).
pub fn file_ports() -> mighty_platform::FilePorts {
    os::file_ports()
}

/// What the Antigravity rule safety asks the disk about a path agy named (a link, what the file system calls it, a fixed drive).
pub fn path_probe() -> Arc<dyn mighty_platform::PathProbe> {
    os::path_probe()
}

/// C# `ToLocalTime().ToString("g", CultureInfo.CurrentCulture)`, when the OS says.
pub fn short_date_time(at: std::time::SystemTime) -> Option<String> {
    os::short_date_time(at)
}

/// The user's decimal separator, when the OS says.
pub fn decimal_separator() -> Option<String> {
    os::decimal_separator()
}

/// A workspace's folder opened in the OS's file manager (C# `ShowWorkspaceInExplorer`).
pub fn show_in_explorer(folder: &std::path::Path) -> std::result::Result<(), String> {
    os::show_in_explorer(folder)
}

/// Each copy of the command `name` this computer has along `environment`'s PATH (C# `SetupProber.Executables`: with PATHEXT and the
/// installers' folders on Windows).
pub fn cli_copies(name: &str, environment: &std::collections::BTreeMap<String, String>) -> Vec<String> {
    os::cli_copies(name, environment)
}

/// The browser engine's installed version (C# `WebView2RuntimeLocator`), or none when the pane must offer its install.
pub fn browser_runtime() -> Option<String> {
    os::browser_runtime()
}

/// The browser engine's installer (C# `WebView2BootstrapperInstaller`), for mighty-core's `browser::install`.
pub struct BrowserInstaller;

impl mighty_core::browser::BrowserInstaller for BrowserInstaller {
    fn download(&self, path: &std::path::Path) -> std::result::Result<(), String> {
        os::download_bootstrapper(path)
    }
    fn verify_microsoft_signature(&self, path: &std::path::Path) -> bool {
        os::is_microsoft_signed(path)
    }
    fn run(&self, path: &std::path::Path) -> std::result::Result<i32, String> {
        os::run_installer(path)
    }
}
