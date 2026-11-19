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

/// This OS's deliberate differences (the same value `platform()` puts in `Platform::profile`), for code that runs before or without
/// the platform (M-C: the firewall rows read `direct_connection_firewall` from the first draw).
pub fn profile() -> mighty_platform::PlatformProfile {
    #[cfg(windows)]
    return mighty_platform::PlatformProfile::windows();
    #[cfg(target_os = "macos")]
    return mighty_platform::PlatformProfile::macos();
}

/// The native views over `window`'s page (the Mac's terminal, the companion pet).
pub fn surfaces(window: tauri::Window) -> Arc<dyn SurfaceFactory> {
    os::surfaces(window)
}

/// W9-C: the main window whose session messages `Platform::session` reports (Windows subclasses it; call on the window's thread). The
/// Mac's session port watches `NSWorkspace`'s notifications itself (M-F), so there is nothing to hand it.
pub fn watch_session(window: &tauri::Window) -> Result<()> {
    #[cfg(windows)]
    return os::session::watch_window(window);
    #[cfg(not(windows))]
    {
        let _ = window;
        Ok(())
    }
}

/// Where the Claude CLI keeps its sign-in besides its file (the account usage's Claude read, M-J): none on Windows, the Keychain on the
/// Mac (M-B's reader).
pub fn claude_credentials() -> Option<Arc<dyn mighty_platform::ClaudeCredentials>> {
    #[cfg(windows)]
    return None;
    #[cfg(target_os = "macos")]
    return Some(os::claude_credentials::claude_credentials());
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

/// The app's own process at launch, before its first child: on the Mac, ends what an earlier run left running when it crashed and
/// writes this run's agent process groups down under `local_dir` (`child-groups`) until it exits (mighty-platform-macos
/// `child_groups`). Windows needs none: its Job Object ends the children when the app's handle closes, a crash included.
pub fn record_child_groups(local_dir: &std::path::Path) {
    #[cfg(windows)]
    let _ = local_dir;
    #[cfg(target_os = "macos")]
    if let Err(error) = os::record_child_groups(&local_dir.join("child-groups")) {
        eprintln!("child groups: {error}");
    }
}

/// Whether names on the volume holding `path` compare ignoring case (a run's result files are looked up and listed once by it): the
/// Mac asks the volume (APFS is case-insensitive as installed, and can be formatted case-sensitive), Windows' always do.
pub fn names_ignore_case(path: &std::path::Path) -> bool {
    os::names_ignore_case(path)
}

/// The process host alone, for the exe's `--fake-claude` mode, which serves before any profile is read (main.rs).
pub fn process_host() -> Arc<dyn mighty_platform::ProcessHost> {
    os::process_host()
}

/// The Mac's PTY host alone, for the checks that type into a real shell (sign_in_pane.rs).
#[cfg(all(test, target_os = "macos"))]
pub fn pty_host() -> Arc<dyn mighty_platform::PtyHost> {
    os::pty::pty_host()
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
/// `WindowsSetupFiles`; the Mac's `MacSetupFiles`, M-G).
#[cfg(windows)]
pub fn setup_files() -> Arc<dyn mighty_platform::SetupFiles> {
    Arc::new(os::WindowsSetupFiles)
}

#[cfg(target_os = "macos")]
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

/// The picture of a box of the page (x, y, width, height in CSS pixels) as PNG bytes (the help capture's popovers): WKWebView's snapshot
/// of that rectangle (M-G); Windows clips through the DevTools protocol instead ([`page_devtools`]).
#[cfg(target_os = "macos")]
pub async fn page_picture_of(page: &tauri::Webview, clip: (f64, f64, f64, f64)) -> std::result::Result<Vec<u8>, String> {
    os::page::snapshot(page, Some(clip)).await
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

/// RenameMigration's names on this OS (mighty-core `rename_migration::Names`): C#'s on Windows, Swift's on the Mac (M-B).
pub fn rename_names() -> mighty_core::rename_migration::Names {
    #[cfg(windows)]
    return mighty_core::rename_migration::WINDOWS_NAMES;
    #[cfg(target_os = "macos")]
    return mighty_core::rename_migration::MAC_NAMES;
}

/// The settings from before the rename RenameMigration copies once: none on Windows (its registry step), the old bundle id's
/// UserDefaults domain into the app's on the Mac (Swift `RenameMigration.swift:47-53`, M-B).
pub fn rename_settings() -> Box<dyn mighty_core::rename_migration::LegacySettings> {
    #[cfg(windows)]
    return Box::new(mighty_core::rename_migration::NoSettings);
    #[cfg(target_os = "macos")]
    return Box::new(MacSettings {
        current: os::prefs::DefaultsPrefs::app(),
        legacy: os::prefs::DefaultsPrefs::domain(mighty_core::rename_migration::LEGACY_BUNDLE_IDENTIFIER),
    });
}

/// The two defaults domains as RenameMigration copies between them.
#[cfg(target_os = "macos")]
struct MacSettings {
    current: os::prefs::DefaultsPrefs,
    legacy: os::prefs::DefaultsPrefs,
}

#[cfg(target_os = "macos")]
impl mighty_core::rename_migration::LegacySettings for MacSettings {
    fn copied(&self) -> bool {
        self.current.stored(mighty_core::rename_migration::SETTINGS_MARKER).is_some()
    }
    fn legacy_keys(&self) -> Vec<String> {
        self.legacy.keys()
    }
    /// As Swift's `defaults.object(forKey:) != nil`: the app's own value or the global domain's (RenameMigration.swift:48).
    fn has(&self, key: &str) -> bool {
        self.current.visible(key)
    }
    fn copy(&self, key: &str) -> std::io::Result<()> {
        self.current.copy_from(&self.legacy, key).map_err(std::io::Error::other)
    }
    fn mark(&self) -> std::io::Result<()> {
        use mighty_platform::PrefsStore as _;
        self.current
            .set(mighty_core::rename_migration::SETTINGS_MARKER, mighty_platform::PrefValue::Bool(true))
            .map_err(std::io::Error::other)
    }
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

/// Whether another copy of the app (`identifier`) runs (Mac integration review 17): on the Mac the Swift 0.4.x app and this one share
/// their bundle identifier and every store, so a run that would write stays read-only while another runs. Windows answers false:
/// its C# and Rust apps keep their own single-instance rules, unchanged here.
pub fn another_instance_running(identifier: &str) -> bool {
    #[cfg(windows)]
    {
        let _ = identifier;
        false
    }
    #[cfg(target_os = "macos")]
    {
        os::session::another_instance_running(identifier)
    }
}

/// The OS's notification settings (the companion settings' button): Windows' Settings › Notifications, the Mac's System Settings ›
/// Notifications (Swift's address).
pub fn open_notification_settings() -> std::result::Result<(), String> {
    #[cfg(windows)]
    return os::open_settings_page("ms-settings:notifications");
    #[cfg(target_os = "macos")]
    return os::notify::open_notification_settings();
}

/// The settings' words: this OS's app's (C#'s on Windows, Swift's on the Mac; mighty-core `settings_words`).
pub fn words() -> mighty_core::app_update_view::Words {
    match profile().os {
        mighty_platform::Os::Windows => mighty_core::app_update_view::Words::Windows,
        mighty_platform::Os::Macos => mighty_core::app_update_view::Words::Mac,
    }
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

/// How a toolkit step's program is found along the CLI environment's PATH: Windows' C# `WindowsCommand.Find` (PATH x PATHEXT); the
/// Mac's Swift `ToolkitProcess.resolveExecutable` (PATH only, no extensions; M-A).
pub fn find_on_path(name: &str, environment: &std::collections::BTreeMap<String, String>) -> Option<String> {
    #[cfg(windows)]
    return mighty_tools::toolkit::runner::find_on_path(name, environment, &|path| std::path::Path::new(path).is_file());
    #[cfg(target_os = "macos")]
    return os::process::find_executable(name, environment);
}

/// The smoke's own limit with the stand-in CLI: the Mac's run (WebKit, Ghostty, the native panels, its own help capture) took about
/// 120–150 s on the runner at the Mac stage's integration, given room to about 400 s; Windows' 645 s (W9 plan R13).
pub fn smoke_fake_claude_seconds() -> u64 {
    if cfg!(target_os = "macos") { 400 } else { 645 }
}

/// A folder as the processes started in it read their own working folder: the Mac's temporary `/var/…` is `/private/var/…` there
/// (canonical); Windows' as it is (canonicalizing would add the verbatim prefix).
pub fn as_processes_see(folder: &std::path::Path) -> std::path::PathBuf {
    if cfg!(windows) { folder.to_path_buf() } else { std::fs::canonicalize(folder).unwrap_or_else(|_| folder.to_path_buf()) }
}

/// Whether the smoke's gate is every check outside smoke.rs' `MAC_PENDING_STEPS` (the Mac) rather than every check (Windows).
pub fn smoke_gate_skips_pending() -> bool {
    cfg!(target_os = "macos")
}

/// Whether the page's webview runs Tauri's initialization scripts in its frames too (WebView2 through wry, whatever "main frame only"
/// says); WKWebView keeps them in the main frame. Either way the host's nonce is defined only in the top frame (main.rs `HostNonce`).
pub fn frames_get_init_scripts() -> bool {
    cfg!(windows)
}

// ── M-F: the Mac's menu bar, input method and pet panel (menu.rs, smoke_companion.rs). Windows has none of them. ──

/// Whether the app has a menu bar of its own (the Mac; Windows' window has none).
pub fn has_menu_bar() -> bool {
    cfg!(target_os = "macos")
}

/// AppKit told which menus are the Window and Help menus (its window list and its help search go there).
pub fn name_standard_menus(window: &tauri::menu::Submenu<tauri::Wry>, help: &tauri::menu::Submenu<tauri::Wry>) -> tauri::Result<()> {
    #[cfg(target_os = "macos")]
    {
        window.set_as_windows_menu_for_nsapp()?;
        help.set_as_help_menu_for_nsapp()
    }
    #[cfg(windows)]
    {
        let _ = (window, help);
        Ok(())
    }
}

/// The menu bar as AppKit holds it (main thread): each item's menu, title, key equivalent and modifier bits (`os::menu`).
pub fn menu_entries() -> Option<Vec<(String, String, String, u64)>> {
    #[cfg(target_os = "macos")]
    return os::menu::entries().map(|items| items.into_iter().map(|i| (i.menu, i.title, i.key, i.modifiers)).collect());
    #[cfg(windows)]
    None
}

/// A key equivalent pressed through the menu bar (main thread): whether an item took it.
pub fn press_menu_key(key: &str, code: u16, modifiers: u64) -> Option<bool> {
    #[cfg(target_os = "macos")]
    return os::menu::press(key, code, modifiers);
    #[cfg(windows)]
    {
        let _ = (key, code, modifiers);
        None
    }
}

/// "Reconnect Input Method": the page's webview takes the keyboard back; the locale key of how it ended.
pub async fn reconnect_input_method(page: &tauri::Webview) -> std::result::Result<&'static str, String> {
    #[cfg(target_os = "macos")]
    return os::input_method::reconnect(page).await.map(|done| done.key()).map_err(|e| e.to_string());
    #[cfg(windows)]
    {
        let _ = page;
        Err("not on this platform".into())
    }
}

/// "Save Input Method Diagnostics": the snapshot written under `<data>/diagnostics`; its path.
pub async fn save_input_diagnostics(page: &tauri::Webview, data_dir: &std::path::Path) -> std::result::Result<PathBuf, String> {
    #[cfg(target_os = "macos")]
    return os::input_method::save_diagnostics(page, data_dir).await.map_err(|e| e.to_string());
    #[cfg(windows)]
    {
        let _ = (page, data_dir);
        Err("not on this platform".into())
    }
}

/// The last pet panel's facts as the smoke reads them back (main thread): what Swift's `CompanionPanel` sets, each true when it
/// holds. None where the pet is no panel (Windows: its window's own `focus` says what it can).
pub fn pet_panel_facts() -> Option<Vec<(&'static str, bool)>> {
    #[cfg(target_os = "macos")]
    return os::pet::panel_facts();
    #[cfg(windows)]
    None
}

/// A new file at `path` (never through an existing file or link) holding `bytes`, executable from the start (0755 on the Mac); removed
/// again if the write fails (shell_command.rs).
pub fn write_new_executable(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(target_os = "macos")]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o755);
    let mut file = options.open(path)?;
    if let Err(error) = file.write_all(bytes).and_then(|()| file.sync_all()) {
        drop(file);
        let _ = std::fs::remove_file(path);
        return Err(error);
    }
    Ok(())
}

/// Whether the OS lets the app notify, where the OS keeps its own answer (the Mac's notification settings): Some(Some(true)) allowed,
/// Some(Some(false)) denied, Some(None) not asked yet; None where the registration is the answer (Windows) or the OS cannot say.
pub async fn notification_authorization() -> Option<Option<bool>> {
    #[cfg(target_os = "macos")]
    return os::notify::authorized().await;
    #[cfg(windows)]
    None
}

/// A menu item (by its title) given a ⌘ character key equivalent, on the main thread after the menu is in (menu.rs).
pub fn set_menu_character_key(app: &tauri::AppHandle, title: &str, key: &str) {
    #[cfg(target_os = "macos")]
    {
        let (title, key) = (title.to_string(), key.to_string());
        let _ = app.run_on_main_thread(move || {
            os::menu::set_command_key(&title, &key);
        });
    }
    #[cfg(windows)]
    let _ = (app, title, key);
}
