//! The Windows implementation of the `mighty-platform` ports. On any other OS this crate is empty (everything is behind
//! `cfg(windows)`), so the workspace builds everywhere; `src-tauri/src/platform.rs` picks it on Windows.
//!
//! Real so far: [`paths`] (`%APPDATA%\MightyCode`), [`RegistryPrefs`] reads (`HKCU\Software\MightyCode`), [`DpapiVault`] (the C#
//! `DpapiRemoteKeyVault` format), the native views ([`WindowsSurfaces`]: the companion pet; the page draws the terminal), and the
//! process layer (W1-C, the C# `ChildProcess`, `WindowsCommand`, `CliRunner`, `CliDiscovery`, `CliEnvironment`,
//! `WindowsShellEnvironment`, `UserEnvironment`, `PseudoTerminal` and `ShellActivity`): [`WindowsProcessHost`],
//! [`WindowsPtyHost`], [`WindowsShellEnvironment`]. W7's Agent IO pipe ([`agent_io`], W7-C) and free space ([`disk_space`], W7-B) are
//! stubs until their parts fill them. Every other port is `mighty_platform::unsupported::Unsupported` until its stage.
//!
//! W9 Step 0 added stubs that answer `Unsupported` until their parts fill them: [`update`] (the helper's mutex, wait, process list,
//! fresh environment, uninstall entry and renamed shortcuts; W9-B), [`update_transport`] (WinHTTP without a proxy; W9-B),
//! `WindowsProcessHost::start_detached` (W9-B), [`session`] (the session ending and the last input; W9-C) and [`activator`] (the COM
//! toast activator; W9-F).

#[cfg(windows)]
pub mod activator;
#[cfg(windows)]
pub mod agent_io;
#[cfg(windows)]
pub mod cli;
#[cfg(windows)]
mod clipboard;
#[cfg(windows)]
pub mod command;
#[cfg(windows)]
pub mod console;
#[cfg(windows)]
pub mod discovery;
#[cfg(windows)]
pub mod disk_space;
#[cfg(windows)]
mod dpapi;
#[cfg(windows)]
pub mod environment;
#[cfg(windows)]
pub mod files;
#[cfg(windows)]
mod firewall;
#[cfg(windows)]
pub mod fonts;
#[cfg(windows)]
pub mod http;
#[cfg(windows)]
pub mod listening;
#[cfg(windows)]
mod net;
#[cfg(windows)]
pub mod notify;
#[cfg(windows)]
pub mod page;
#[cfg(windows)]
mod pairing;
#[cfg(windows)]
mod pet;
#[cfg(windows)]
mod prefs;
#[cfg(windows)]
mod process;
#[cfg(windows)]
mod proxy;
#[cfg(windows)]
pub mod pty;
#[cfg(windows)]
pub mod registry;
#[cfg(windows)]
pub mod session;
#[cfg(windows)]
pub mod setup_files;
#[cfg(windows)]
pub mod setup_probe;
#[cfg(windows)]
pub mod shell_activity;
#[cfg(windows)]
pub mod speech;
#[cfg(windows)]
mod text;
#[cfg(windows)]
pub mod update;
#[cfg(windows)]
pub mod update_transport;
#[cfg(windows)]
pub mod user_environment;
#[cfg(windows)]
mod variables;
#[cfg(windows)]
mod win;

#[cfg(windows)]
pub use imp::*;

#[cfg(windows)]
mod imp {
    use std::path::PathBuf;
    use std::sync::Arc;

    use mighty_platform::{
        AppPaths, BoxFuture, Drawing, Paths, PetWindow, Platform, PlatformError, PlatformProfile, Result, SurfaceFactory, SurfaceKind,
        TerminalOptions, TerminalSurface,
    };

    pub use crate::activator::ComActivator;
    pub use crate::agent_io::{ControlPipeConnector, DaemonPipeConnector, NamedPipeConnector, NamedPipeListener, lock_exclusive};
    pub use crate::cli::{CliRunResult, CliRunner};
    pub use crate::clipboard::WindowsClipboard;
    pub use crate::console::WindowsTerminalLauncher;
    pub use crate::disk_space::WindowsDiskSpace;
    pub use crate::dpapi::{DpapiVault, FOLDER as REMOTE_KEYS_FOLDER, file_name as remote_key_file_name};
    pub use crate::environment::{CliEnvironmentSnapshot, WindowsShellEnvironment};
    pub use crate::files::{
        WindowsDesktop, WindowsFileHandles, WindowsPathProbe, WindowsPdf, decimal_separator, file_ports, path_probe, short_date_time,
    };
    pub use crate::firewall::WindowsFirewall;
    pub use crate::fonts::{DirectWriteFonts, font_catalog};
    pub use crate::net::WindowsNetwork;
    pub use crate::pairing::WindowsPairing;
    pub use crate::prefs::RegistryPrefs;
    pub use crate::process::{KILLED_EXIT_CODE, WindowsProcessHost};
    pub use crate::proxy::WinHttpProxy;
    pub use crate::pty::WindowsPtyHost;
    pub use crate::session::WindowsSession;
    pub use crate::setup_files::WindowsSetupFiles;
    pub use crate::speech::{WindowsSpeech, speech_recognizer};
    pub use crate::update::WindowsUpdates;
    pub use crate::update_transport::WinHttpUpdates;
    pub use crate::variables::Variables;

    /// The folders of the app before its rename whose state a default profile without one takes, under `%APPDATA%` (C# `MainWindow`'s
    /// constructor, `MainWindow.cs:64`: the first holding a `workspace-state.json`, copied in by the store). C#'s other rename,
    /// `MightyClaudeNative` (moved whole with `.mightyclaude` and the HKCU key), is mighty-core's `rename_migration`, which startup.rs
    /// runs before the store reads anything; C# runs both, so both stay (W9-F).
    pub const LEGACY_FOLDERS: [&str; 2] = ["MightyClaude", "mighty-claude"];

    /// `%APPDATA%\MightyCode` (or `profile`), the pre-rename folders whose state is copied once, and `%LOCALAPPDATA%\<identifier>`.
    pub fn paths(profile: Option<PathBuf>, identifier: &str) -> Result<AppPaths> {
        AppPaths::resolve(profile, dirs::config_dir(), &LEGACY_FOLDERS, dirs::data_local_dir(), identifier)
    }

    /// The folder the setup program installs into (`MightyCode.iss` `DefaultDirName={userpf}\MightyCode`): `Programs\MightyCode` under
    /// the LocalAppData known folder (W9-F: a release build writes by default only when it runs from here, startup.rs).
    pub fn install_folder() -> Option<PathBuf> {
        dirs::data_local_dir().map(|local| local.join("Programs").join("MightyCode"))
    }

    /// The schemes [`open_url`] hands to their default handler: the web and mail (as the Mac's `open_url`). Every caller passes such an
    /// address; a file, a path, `ms-settings:` or an app's own protocol (`search-ms:`, `ms-msdt:`, a custom handler) is refused here.
    const URL_SCHEMES: [&str; 3] = ["http", "https", "mailto"];

    /// The scheme of `text` when it starts with one (RFC 3986: a letter, then letters, digits, `+`, `-` or `.`, then `:`), lowercased.
    /// One letter is a drive (`C:\x`), never a scheme; no control character or blank is let through anywhere in the text.
    fn scheme_of(text: &str) -> Option<String> {
        if text.chars().any(|c| c.is_control() || c.is_whitespace()) {
            return None;
        }
        let (scheme, _) = text.split_once(':')?;
        let mut chars = scheme.chars();
        let valid = scheme.len() >= 2
            && chars.next().is_some_and(|c| c.is_ascii_alphabetic())
            && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
        valid.then(|| scheme.to_ascii_lowercase())
    }

    /// C# `Process.Start(new ProcessStartInfo(url) { UseShellExecute = true })`: an http(s) or mailto address handed to the user's
    /// default handler through `ShellExecuteW` (the handler is never the app's child, so it outlives the app); any other text is refused
    /// before the shell sees it ([`URL_SCHEMES`]). The calling thread is in a single-threaded apartment for the call, as the shell wants
    /// (one already in another apartment stays as it is). It may wait on the shell: an async caller runs it on a blocking thread.
    pub fn open_url(url: &str) -> std::result::Result<(), String> {
        if !scheme_of(url).is_some_and(|scheme| URL_SCHEMES.contains(&scheme.as_str())) {
            return Err("not an address the default handler may open".into());
        }
        shell_open(url)
    }

    /// Windows' Settings at one of its own pages (`ms-settings:notifications`): only an `ms-settings:` address, which [`open_url`]
    /// refuses.
    pub fn open_settings_page(address: &str) -> std::result::Result<(), String> {
        if scheme_of(address).as_deref() != Some("ms-settings") {
            return Err("not a Settings page".into());
        }
        shell_open(address)
    }

    /// `ShellExecuteW(open)` of an address its caller checked.
    fn shell_open(url: &str) -> std::result::Result<(), String> {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::System::Com::{COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize};
        use windows_sys::Win32::UI::Shell::ShellExecuteW;
        use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
        let wide = |text: &str| std::ffi::OsStr::new(text).encode_wide().chain(std::iter::once(0)).collect::<Vec<u16>>();
        let (verb, target) = (wide("open"), wide(url));
        // SAFETY: this thread's apartment, left again below when it was entered here.
        let initialized = unsafe { CoInitializeEx(std::ptr::null(), (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32) } >= 0;
        // SAFETY: two NUL-terminated strings that live across the call; the other pointers are null as documented.
        let done = unsafe {
            ShellExecuteW(std::ptr::null_mut(), verb.as_ptr(), target.as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL)
        };
        let error = std::io::Error::last_os_error();
        if initialized {
            // SAFETY: paired with the CoInitializeEx above on this thread.
            unsafe { CoUninitialize() };
        }
        // ShellExecute's result is greater than 32 when it succeeded.
        if done as isize > 32 { Ok(()) } else { Err(error.to_string()) }
    }

    /// C# `ShowWorkspaceInExplorer` (MainWindow.Navigation.cs:330, `explorer.exe <folder>`): the folder handed to the shell's
    /// `explore` verb, so its name is never an argument Explorer parses (a folder named `a,b` or `/select,x` is that folder, where
    /// `explorer.exe a,b` reads switches). The shell starts Explorer, never the app's child. Only a folder is explored.
    pub fn show_in_explorer(folder: &std::path::Path) -> std::result::Result<(), String> {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::UI::Shell::ShellExecuteW;
        use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
        if !folder.is_dir() {
            return Err(format!("not a folder: {}", folder.display()));
        }
        let wide = |text: &std::ffi::OsStr| text.encode_wide().chain(std::iter::once(0)).collect::<Vec<u16>>();
        let (verb, target) = (wide("explore".as_ref()), wide(folder.as_os_str()));
        // SAFETY: NUL-terminated strings that live across the call; the other pointers are null as documented.
        let done = unsafe {
            ShellExecuteW(std::ptr::null_mut(), verb.as_ptr(), target.as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL)
        };
        if done as isize > 32 { Ok(()) } else { Err(std::io::Error::last_os_error().to_string()) }
    }

    /// The number format C#'s `CultureInfo.CurrentCulture` gives `{value:N0}`: the user's regional format with their own changes
    /// (`GetLocaleInfoEx` of the user's default locale): its group separator, and `LOCALE_SGROUPING` as .NET's `NumberGroupSizes`
    /// ("3;0" is `[3]`, India's "3;2;0" `[3, 2]`, "3" alone `[3, 0]`). None when Windows does not say.
    pub fn number_grouping() -> Option<(String, Vec<usize>)> {
        use windows_sys::Win32::Globalization::{GetLocaleInfoEx, LOCALE_SGROUPING, LOCALE_STHOUSAND};
        let read = |kind: u32| -> Option<String> {
            let mut buffer = [0u16; 32];
            // SAFETY: a null name is LOCALE_NAME_USER_DEFAULT; the buffer and its length in UTF-16 units, NUL included.
            let written = unsafe { GetLocaleInfoEx(std::ptr::null(), kind, buffer.as_mut_ptr(), buffer.len() as i32) };
            (written > 0).then(|| String::from_utf16_lossy(&buffer[..written as usize - 1]))
        };
        let separator = read(LOCALE_STHOUSAND)?;
        Some((separator, group_sizes(&read(LOCALE_SGROUPING)?)?))
    }

    /// `LOCALE_SGROUPING` as .NET's `NumberGroupSizes`: a trailing 0 repeats the size before it (dropped), without one the grouping
    /// stops after the sizes given (a 0 added).
    fn group_sizes(grouping: &str) -> Option<Vec<usize>> {
        let mut sizes: Vec<usize> = grouping.split(';').map(|size| size.trim().parse().ok()).collect::<Option<_>>()?;
        match sizes.last() {
            Some(0) if sizes.len() > 1 => {
                sizes.pop();
            }
            Some(_) => sizes.push(0),
            None => return None,
        }
        Some(sizes)
    }

    /// Every port for this run. W8's (the pairing file, the firewall rule, the network, the proxy, the push's POST) are stubs that
    /// answer `Unsupported` until their parts fill them (W8 Step 0), as the pairing and firewall ports did before.
    pub fn platform(profile: Option<PathBuf>, identifier: &str) -> Result<Platform> {
        let paths = paths(profile, identifier)?;
        let vault = DpapiVault::new(paths.data_dir().to_path_buf());
        // The phone pairing's secrets in DPAPI beside the remote keys (`remote-keys\<sha256(account)>.bin`), the file naming them.
        let pairing = WindowsPairing::with_vault(
            paths.data_dir().join(mighty_platform::pairing::identity_file::FOLDER),
            Arc::new(DpapiVault::new(paths.data_dir().to_path_buf())),
        );
        let processes: Arc<dyn mighty_platform::ProcessHost> = Arc::new(WindowsProcessHost::new());
        Ok(Platform {
            profile: PlatformProfile::windows(),
            paths: Arc::new(paths),
            processes: processes.clone(),
            ptys: Arc::new(WindowsPtyHost::new()),
            environment: Arc::new(WindowsShellEnvironment::new()),
            secrets: Arc::new(vault),
            prefs: Arc::new(RegistryPrefs::app()),
            pairing: Arc::new(pairing),
            notifier: notifier(),
            firewall: Arc::new(WindowsFirewall::new(processes)),
            updates: Arc::new(WindowsUpdates),
            network: Arc::new(WindowsNetwork),
            proxy: Arc::new(WinHttpProxy),
            https: Arc::new(crate::http::WinHttpGet),
            // W9's (Step 0 stubs that answer `Unsupported`, as the update port did before): W9-B, W9-C, W9-F.
            update_transport: Arc::new(WinHttpUpdates),
            session: Arc::new(WindowsSession),
            activator: Arc::new(ComActivator),
        })
    }

    /// The update installer alone, for the exe's `--update-helper` mode, which runs before any profile is read (W9-B).
    pub fn update_installer() -> Arc<dyn mighty_platform::UpdateInstaller> {
        Arc::new(WindowsUpdates)
    }

    /// Windows' native views: the companion pet's window only. The terminal is the page's (xterm.js over ConPTY), and 0.5 has no
    /// browser pane (a web page opens in the default browser, [`open_url`]).
    pub struct WindowsSurfaces;

    impl SurfaceFactory for WindowsSurfaces {
        fn drawing(&self, kind: SurfaceKind) -> Drawing {
            match kind {
                SurfaceKind::Terminal => Drawing::Web,
            }
        }

        fn open_terminal(&self, _options: TerminalOptions) -> BoxFuture<'_, Result<Box<dyn TerminalSurface>>> {
            Box::pin(async { Err(PlatformError::unsupported("SurfaceFactory", "open_terminal")) })
        }

        /// The companion's window (pet.rs, W8-E).
        fn open_pet(&self) -> BoxFuture<'_, Result<Box<dyn PetWindow>>> {
            Box::pin(async { crate::pet::open() })
        }
    }

    /// The native views over the main window's page ([`WindowsSurfaces`]: none of them is a child of `window`).
    pub fn surfaces(window: tauri::Window) -> Arc<dyn SurfaceFactory> {
        let _ = window;
        Arc::new(WindowsSurfaces)
    }

    /// The user's clipboard (a terminal's copy and paste).
    pub fn clipboard() -> Arc<dyn mighty_platform::Clipboard> {
        Arc::new(WindowsClipboard)
    }

    /// The Agent IO listener (one per app, W7-C): the named pipe.
    pub fn agent_io_listener() -> Arc<dyn mighty_platform::AgentIoListener> {
        Arc::new(NamedPipeListener::new())
    }

    /// The Agent IO client of the exe's MCP modes (`--agent-io-mcp`, `--agent-delegation-mcp`, W7-C): the named pipe.
    pub fn agent_io_connector() -> Arc<dyn mighty_platform::AgentIoConnector> {
        Arc::new(NamedPipeConnector)
    }

    /// MC-9: the local control API's listener: an owner-only named pipe `mighty-control-<32 hex>`, remote clients refused.
    pub fn control_listener() -> Arc<dyn mighty_platform::AgentIoListener> {
        Arc::new(NamedPipeListener::control())
    }

    /// MC-9: the `mighty` CLI's client of that pipe.
    pub fn control_connector() -> Arc<dyn mighty_platform::AgentIoConnector> {
        Arc::new(ControlPipeConnector)
    }

    /// MC-3: the background daemon's listener: an owner-only named pipe `mighty-daemon-<32 hex>`, remote clients refused.
    pub fn daemon_listener() -> Arc<dyn mighty_platform::AgentIoListener> {
        Arc::new(NamedPipeListener::daemon())
    }

    /// MC-3: the app's client of that pipe.
    pub fn daemon_connector() -> Arc<dyn mighty_platform::AgentIoConnector> {
        Arc::new(DaemonPipeConnector)
    }

    /// A volume's free space (delegation's worktree check, W7-B).
    pub fn disk_space() -> Arc<dyn mighty_platform::DiskSpace> {
        Arc::new(WindowsDiskSpace)
    }

    /// Whether names on the volume holding `path` compare ignoring case (the result-file lookup): Windows' file systems do, as C#'s
    /// `PathComparison` takes them to.
    pub fn names_ignore_case(_path: &std::path::Path) -> bool {
        true
    }

    /// The process host alone, for the exe's `--fake-claude` mode, which starts before any profile is read (W7-C).
    pub fn process_host() -> Arc<dyn mighty_platform::ProcessHost> {
        Arc::new(WindowsProcessHost::new())
    }

    /// A command in a terminal window of its own that outlives the app (C# `CliAccountTerminal`): Windows Terminal, else the console
    /// host (W6: sign-ins, the setup screen's install).
    pub fn terminal_launcher() -> Arc<dyn mighty_platform::TerminalLauncher> {
        Arc::new(WindowsTerminalLauncher)
    }

    /// C# `ToLocalTime().ToString("T", CultureInfo.CurrentCulture)`: the user's long time format (`GetTimeFormatEx` with no flags) of a
    /// moment in the local zone; None when Windows does not say.
    pub fn long_time(at: std::time::SystemTime) -> Option<String> {
        use windows_sys::Win32::Foundation::{FILETIME, SYSTEMTIME};
        use windows_sys::Win32::Globalization::GetTimeFormatEx;
        use windows_sys::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
        let since = at.duration_since(std::time::SystemTime::UNIX_EPOCH).ok()?;
        let ticks = since.as_nanos() / 100 + 116_444_736_000_000_000;
        let file_time = FILETIME { dwLowDateTime: ticks as u32, dwHighDateTime: (ticks >> 32) as u32 };
        // SAFETY: plain structures the calls write.
        let (mut utc, mut local): (SYSTEMTIME, SYSTEMTIME) = unsafe { (std::mem::zeroed(), std::mem::zeroed()) };
        // SAFETY: valid pointers to the structures above; a null zone is the current one.
        if unsafe { FileTimeToSystemTime(&file_time, &mut utc) } == 0
            || unsafe { SystemTimeToTzSpecificLocalTime(std::ptr::null(), &utc, &mut local) } == 0
        {
            return None;
        }
        let mut time = [0u16; 128];
        // SAFETY: a null locale is the user's default; the buffer and its length in units.
        let length = unsafe { GetTimeFormatEx(std::ptr::null(), 0, &local, std::ptr::null(), time.as_mut_ptr(), time.len() as i32) };
        (length > 1).then(|| String::from_utf16_lossy(&time[..length as usize - 1]))
    }

    /// The completion toast (`mighty_platform::Notifier`, C# `WindowsAppNotifier`).
    pub fn notifier() -> Arc<dyn mighty_platform::Notifier> {
        Arc::new(crate::notify::ToastNotifier::default())
    }

    /// Registers the app's AUMID for its toasts (HKCU; the host calls it only with `--allow-state-write`).
    pub fn register_notifications() -> std::result::Result<(), String> {
        crate::notify::register().map_err(|e| e.to_string())
    }

    /// The account usage's HTTPS GET (`mighty_platform::HttpsSend`): WinHTTP (http.rs: system proxy, redirects, cookies and automatic
    /// logon off, the body only for a 2xx). Which URLs may be asked is the caller's guard (mighty-accounts' `usage::guard`).
    pub fn https_get() -> Arc<dyn mighty_platform::HttpsSend> {
        Arc::new(crate::http::WinHttpGet)
    }

    /// Each copy of the command `name` this PC has (C# `SetupProber.Executables`): along the environment's PATH with PATHEXT, then
    /// in the installers' folders.
    pub fn cli_copies(name: &str, environment: &std::collections::BTreeMap<String, String>) -> Vec<String> {
        crate::setup_probe::executables(name, &Variables::from(environment))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// The user's number format reads as .NET's `NumberGroupSizes`: a list whose sizes are its groups from the right.
        #[test]
        fn the_user_number_format_reads() {
            let (separator, sizes) = number_grouping().expect("Windows names the user's number format");
            assert!(separator.encode_utf16().count() <= 4, "a separator, not a sentence: {separator:?}");
            assert!(!sizes.is_empty() && sizes.iter().all(|size| *size <= 9), "group sizes {sizes:?}");
        }

        /// Review L7: the grouping read as .NET reads it — "3;0" is [3], India's "3;2;0" [3, 2], "3" alone [3, 0], words none.
        #[test]
        fn the_grouping_reads_as_dotnet_reads_it() {
            assert_eq!(group_sizes("3;0"), Some(vec![3]));
            assert_eq!(group_sizes("3;2;0"), Some(vec![3, 2]));
            assert_eq!(group_sizes("3"), Some(vec![3, 0]));
            assert_eq!(group_sizes("0"), Some(vec![0, 0]));
            assert_eq!(group_sizes("3;x"), None);
        }

        /// Only a web or mail address reaches the shell from `open_url`, only a Settings page from `open_settings_page`: a path, a
        /// file address or another protocol's handler is refused before `ShellExecuteW` (nothing here is opened).
        #[test]
        fn only_web_and_mail_addresses_are_opened() {
            for address in ["https://example.com/a?b=c", "HTTP://localhost:3000", "mailto:someone@example.com"] {
                assert!(scheme_of(address).is_some_and(|scheme| URL_SCHEMES.contains(&scheme.as_str())), "{address}");
            }
            for address in [
                r"C:\Windows\System32\calc.exe",
                r"\\server\share\x.exe",
                "file:///C:/Windows/System32/calc.exe",
                "ms-settings:notifications",
                "search-ms:query=x",
                "ms-msdt:/id x",
                "javascript:alert(1)",
                "calc.exe",
                "https://example.com/\nx",
                " https://example.com",
                "",
            ] {
                assert!(open_url(address).is_err(), "{address:?} is refused");
            }
            assert!(open_settings_page("https://example.com").is_err() && open_settings_page("calc.exe").is_err());
            assert_eq!(scheme_of("ms-settings:notifications").as_deref(), Some("ms-settings"));
        }

        /// Only a folder is explored (a file of the same name is never handed to the shell).
        #[test]
        fn only_a_folder_is_shown_in_explorer() {
            let file = std::env::temp_dir().join(format!("mighty-explore-{},x.txt", std::process::id()));
            std::fs::write(&file, "x").unwrap();
            assert!(show_in_explorer(&file).is_err());
            std::fs::remove_file(&file).unwrap();
            assert!(show_in_explorer(std::path::Path::new(r"C:\no\such\folder,x")).is_err());
        }

        #[test]
        fn the_default_profile_is_appdata_mightycode() {
            let paths = paths(None, "dev.mightycode.native").unwrap();
            assert_eq!(paths.data_dir(), dirs::config_dir().unwrap().join("MightyCode"));
            let roaming = dirs::config_dir().unwrap();
            assert_eq!(paths.legacy_data_dirs(), [roaming.join("MightyClaude"), roaming.join("mighty-claude")], "C# MainWindow's");
            assert_eq!(paths.local_dir(), dirs::data_local_dir().unwrap().join("dev.mightycode.native"));
        }
    }
}
