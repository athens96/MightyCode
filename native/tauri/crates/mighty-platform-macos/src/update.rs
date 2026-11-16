//! M-H: the Mac's update installer (Swift `MightyCore/AppUpdate.swift` `launchInstaller`, `MightyCode/AppStore+AppUpdate.swift`
//! `prepareAppUpdateInstall`; contract `mighty_platform::update`, `UpdateInstaller`) and where a release build counts as installed (plan
//! D7).
//!
//! The script's text and the checks before it are mighty-updates' (`app_update::install_script`, byte for byte Swift's); here is only
//! what the OS knows and does: the running `.app` ([`MacUpdateInstaller::install_location`]), the macOS version, `access(2)` for the
//! writable folder and the runnable executable, and [`MacUpdateInstaller::start_install_script`]: `install.sh` written 0700,
//! `install.log` made 0600, `/bin/sh install.sh` started detached through the process host's `start_detached` (an update module: the
//! source guard names this file) with exactly `PATH=/usr/bin:/bin` and `HOME`, its output into the log. The Mac has no `--update-helper`
//! (the 0.4.x Mac app swaps with its own script, plan fact 1), so the helper's seams (`helper_mutex`, `wait_for_exit`, `running_from`,
//! the uninstall entry) answer `Unsupported` by design, and `hand_off` too: the Mac's hand-off is `start_install_script`.

use std::collections::BTreeMap;
use std::ffi::CString;
use std::io::Write as _;
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mighty_platform::{
    DetachedEnvironment, DetachedRequest, HelperMutex, PlatformError, ProcessHost, Result, StagedUpdate, UpdateInstaller,
};

/// The script's and its log's names (Swift `launchInstaller`), as mighty-updates' `install_script` names them.
const SCRIPT_NAME: &str = "install.sh";
const LOG_NAME: &str = "install.log";

/// The shell that runs the script; it hands the script its empty input and its log (Swift's `Process` set them as handles).
const SHELL: &str = "/bin/sh";
const RUN_INTO_LOG: &str = "exec /bin/sh \"$0\" </dev/null >>\"$1\" 2>&1";

fn unsupported(operation: &'static str) -> PlatformError {
    PlatformError::unsupported("UpdateInstaller", operation)
}

/// `access(path, mode) == 0`.
fn access(path: &Path, mode: libc::c_int) -> bool {
    let Ok(text) = CString::new(path.as_os_str().as_bytes()) else { return false };
    // SAFETY: `text` is a NUL-terminated string that outlives the call; access(2) only reads it.
    unsafe { libc::access(text.as_ptr(), mode) == 0 }
}

/// The `.app` an executable runs from (`<bundle>.app/Contents/MacOS/<exe>`), or None.
pub fn bundle_of(executable: &Path) -> Option<PathBuf> {
    let macos = executable.parent()?;
    let contents = macos.parent()?;
    let bundle = contents.parent()?;
    let named = |path: &Path, name: &str| path.file_name().and_then(|n| n.to_str()) == Some(name);
    (named(macos, "MacOS") && named(contents, "Contents") && bundle.extension().and_then(|e| e.to_str()) == Some("app"))
        .then(|| bundle.to_path_buf())
}

/// The Mac's `UpdateInstaller`.
pub struct MacUpdateInstaller {
    processes: Arc<dyn ProcessHost>,
}

impl MacUpdateInstaller {
    /// Over `processes`, whose `start_detached` starts the script.
    pub fn new(processes: Arc<dyn ProcessHost>) -> MacUpdateInstaller {
        MacUpdateInstaller { processes }
    }
}

impl std::fmt::Debug for MacUpdateInstaller {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MacUpdateInstaller")
    }
}

impl UpdateInstaller for MacUpdateInstaller {
    /// The running `.app` (Swift `Bundle.main.bundleURL` when it is one); None for an executable outside a bundle (a development build).
    fn install_location(&self) -> Option<PathBuf> {
        std::env::current_exe().ok().as_deref().and_then(bundle_of)
    }

    fn hand_off(&self, _update: &StagedUpdate, _relaunch: bool) -> Result<()> {
        Err(unsupported("hand_off"))
    }

    fn helper_mutex(&self, _name: &str) -> Result<Option<HelperMutex>> {
        Err(unsupported("helper_mutex"))
    }

    fn wait_for_exit(&self, _pid: u32, _timeout: std::time::Duration) -> Result<bool> {
        Err(unsupported("wait_for_exit"))
    }

    fn running_from(&self, _folder: &Path, _except_self: bool) -> Result<bool> {
        Err(unsupported("running_from"))
    }

    /// None: a Mac app never starts from a bare environment (the 0.4.x Mac helper relaunches with `open -n`).
    fn fresh_environment(&self) -> Option<BTreeMap<String, String>> {
        None
    }

    fn sync_display_version(&self, _app_directory: &Path, _version: &str) -> Result<bool> {
        Err(unsupported("sync_display_version"))
    }

    fn refresh_renamed_install(&self, _app_directory: &Path) -> Result<()> {
        Err(unsupported("refresh_renamed_install"))
    }

    /// `NSProcessInfo.operatingSystemVersion` as Swift's `AppVersion.systemVersion` prints it.
    fn system_version(&self) -> Option<String> {
        let version = objc2_foundation::NSProcessInfo::processInfo().operatingSystemVersion();
        Some(format!("{}.{}.{}", version.majorVersion, version.minorVersion, version.patchVersion))
    }

    fn may_write(&self, folder: &Path) -> bool {
        access(folder, libc::W_OK)
    }

    fn may_execute(&self, file: &Path) -> bool {
        access(file, libc::X_OK)
    }

    fn start_install_script(&self, script: &str, folder: &Path) -> Result<()> {
        // `Data.write(options: .atomic)` then mode 0700.
        let path = folder.join(SCRIPT_NAME);
        let partial = folder.join(format!(".{SCRIPT_NAME}.{}", std::process::id()));
        let written = (|| {
            let mut file = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o700).open(&partial)?;
            file.write_all(script.as_bytes())?;
            file.sync_all()?;
            std::fs::set_permissions(&partial, std::fs::Permissions::from_mode(0o700))?;
            std::fs::rename(&partial, &path)
        })();
        if let Err(error) = written {
            let _ = std::fs::remove_file(&partial);
            return Err(error.into());
        }
        // `createFile(atPath:contents:attributes: [.posixPermissions: 0o600])`: a fresh, empty log.
        let log = folder.join(LOG_NAME);
        let _ = std::fs::remove_file(&log);
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&log)
            .map_err(|_| PlatformError::Refused("the install log could not be created".into()))?;
        let home = std::env::var_os("HOME").map(PathBuf::from).or_else(dirs::home_dir).unwrap_or_else(|| PathBuf::from("/"));
        let environment =
            BTreeMap::from([("PATH".to_string(), "/usr/bin:/bin".to_string()), ("HOME".to_string(), home.to_string_lossy().into_owned())]);
        let request = DetachedRequest {
            program: PathBuf::from(SHELL),
            args: vec!["-c".into(), RUN_INTO_LOG.into(), path.to_string_lossy().into_owned(), log.to_string_lossy().into_owned()],
            cwd: folder.to_path_buf(),
            environment: DetachedEnvironment::Exactly(environment),
            hidden: true,
        };
        self.processes.start_detached(request).map(|_| ())
    }
}

/// The Mac's `UpdateInstaller`, over the Mac's process host; also for the exe's modes that run before any profile.
pub fn update_installer() -> Arc<dyn UpdateInstaller> {
    Arc::new(MacUpdateInstaller::new(crate::process::process_host()))
}

/// R6 (plan §7, mac-step0 "H"): a release Mac build writes by default only once M-B's Swift-state round trip (`tools/parity-mac
/// state-corpus`) proves a Rust save keeps everything the Swift app wrote. Until the integrator turns this on with that gate green, a
/// release Mac build writes only with `--allow-state-write`, as at Step 0.
pub const WRITES_BY_DEFAULT: bool = false;

/// Where a release build counts as installed, for the write policy (src-tauri `startup::installed` through `platform.rs`; plan D7):
/// the running exe's `Contents/MacOS` when its `.app` sits directly in `/Applications` or `~/Applications` (compared canonically, so a
/// translocated copy under `/private/var/folders/…/AppTranslocation` never counts). Empty while [`WRITES_BY_DEFAULT`] is off.
pub fn install_locations() -> Vec<PathBuf> {
    if !WRITES_BY_DEFAULT {
        return Vec::new();
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).or_else(dirs::home_dir);
    std::env::current_exe().ok().and_then(|exe| installed_folder(&exe, home.as_deref())).into_iter().collect()
}

/// D7's rule for one executable: its folder when its bundle's folder is `/Applications` or `<home>/Applications` (canonical paths).
pub fn installed_folder(executable: &Path, home: Option<&Path>) -> Option<PathBuf> {
    let executable = std::fs::canonicalize(executable).ok()?;
    let bundle = bundle_of(&executable)?;
    let parent = bundle.parent()?;
    let applications = [Some(PathBuf::from("/Applications")), home.map(|home| home.join("Applications"))];
    let installed =
        applications.into_iter().flatten().filter_map(|folder| std::fs::canonicalize(folder).ok()).any(|folder| folder == parent);
    installed.then(|| executable.parent().map(Path::to_path_buf)).flatten()
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use mighty_platform::{BoxFuture, Child, SpawnRequest};

    use super::*;

    /// A fresh temporary folder (canonical: `/var` is `/private/var`), deleted when dropped.
    struct Temp(PathBuf);

    impl Temp {
        fn new(name: &str) -> Temp {
            let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
            let path = std::env::temp_dir().join(format!("mighty-update-{name}-{}-{nanos}", std::process::id()));
            std::fs::create_dir_all(&path).unwrap();
            Temp(std::fs::canonicalize(path).unwrap())
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// The checks' own detached start (M-A writes the Mac's process host): the program run with exactly the environment given, no
    /// handle of the test's, in a session of its own.
    struct StdDetached;

    impl ProcessHost for StdDetached {
        fn spawn(&self, _request: SpawnRequest) -> BoxFuture<'_, Result<Child>> {
            Box::pin(async { Err(PlatformError::unsupported("ProcessHost", "spawn")) })
        }
        #[allow(clippy::disallowed_methods, reason = "the checks' stand-in for M-A's process host, which this branch does not have")]
        fn start_detached(&self, request: DetachedRequest) -> Result<u32> {
            use std::os::unix::process::CommandExt as _;
            let DetachedEnvironment::Exactly(environment) = request.environment else { panic!("the script's environment is exact") };
            let mut command = std::process::Command::new(&request.program);
            command.args(&request.args).current_dir(&request.cwd).env_clear().envs(environment);
            command.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
            command.process_group(0);
            Ok(command.spawn()?.id())
        }
    }

    fn mode(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    fn wait_for(path: &Path, text: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let read = std::fs::read_to_string(path).unwrap_or_default();
            if read.contains(text) || Instant::now() > deadline {
                return read;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// AppUpdate.swift:406-425 `launchInstaller`: install.sh 0700, install.log 0600 and empty, the script run by /bin/sh with exactly
    /// PATH and HOME, its input empty, its output and errors in the log.
    #[test]
    fn the_install_script_starts_detached_with_only_path_and_home_into_its_log() {
        let folder = Temp::new("launch");
        std::fs::write(folder.0.join(LOG_NAME), "an older run's lines\n").unwrap();
        let script = "#!/bin/sh\nenv | sort\nread line && echo \"read: $line\"\necho oops >&2\necho done\n";
        MacUpdateInstaller::new(Arc::new(StdDetached)).start_install_script(script, &folder.0).unwrap();
        assert_eq!(std::fs::read_to_string(folder.0.join(SCRIPT_NAME)).unwrap(), script);
        assert_eq!(mode(&folder.0.join(SCRIPT_NAME)), 0o700);
        assert_eq!(mode(&folder.0.join(LOG_NAME)), 0o600);
        let log = wait_for(&folder.0.join(LOG_NAME), "done");
        let home = std::env::var("HOME").unwrap();
        let names: Vec<&str> = log.lines().filter_map(|line| line.split_once('=').map(|(name, _)| name)).collect();
        assert!(log.contains("PATH=/usr/bin:/bin\n") && log.contains(&format!("HOME={home}\n")), "{log}");
        // sh itself adds PWD, SHLVL and `_`; nothing of the test's environment arrives.
        assert!(names.iter().all(|name| ["PATH", "HOME", "PWD", "SHLVL", "_", "OLDPWD"].contains(name)), "{log}");
        assert!(!log.contains("read: ") && log.contains("oops\n") && !log.contains("an older run"), "{log}");
    }

    #[test]
    fn this_mac_names_its_version_and_access() {
        let installer = MacUpdateInstaller::new(Arc::new(StdDetached));
        let version = installer.system_version().unwrap();
        assert_eq!(version.split('.').count(), 3, "{version}");
        assert!(version.split('.').next().unwrap().parse::<u32>().unwrap() >= 14, "{version}");
        let folder = Temp::new("access");
        assert!(installer.may_write(&folder.0));
        assert!(installer.may_execute(Path::new("/bin/sh")) && !installer.may_execute(&folder.0.join("missing")));
        let plain = folder.0.join("plain");
        std::fs::write(&plain, "x").unwrap();
        assert!(!installer.may_execute(&plain));
        assert!(!installer.may_write(Path::new("/System")));
    }

    /// Plan D7: only a bundle directly in /Applications or ~/Applications counts, compared canonically.
    #[test]
    fn a_release_counts_as_installed_only_in_an_applications_folder() {
        let home = Temp::new("home");
        let exe = home.0.join("Applications/Mighty Code.app/Contents/MacOS/MightyCode");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, "").unwrap();
        assert_eq!(installed_folder(&exe, Some(&home.0)), Some(exe.parent().unwrap().to_path_buf()));
        let elsewhere = home.0.join("Downloads/MightyCode.app/Contents/MacOS/MightyCode");
        std::fs::create_dir_all(elsewhere.parent().unwrap()).unwrap();
        std::fs::write(&elsewhere, "").unwrap();
        assert_eq!(installed_folder(&elsewhere, Some(&home.0)), None);
        let nested = home.0.join("Applications/Tools/MightyCode.app/Contents/MacOS/MightyCode");
        std::fs::create_dir_all(nested.parent().unwrap()).unwrap();
        std::fs::write(&nested, "").unwrap();
        assert_eq!(installed_folder(&nested, Some(&home.0)), None);
        let bare = home.0.join("Applications/MightyCode");
        std::fs::write(&bare, "").unwrap();
        assert_eq!(installed_folder(&bare, Some(&home.0)), None);
        // R6: none while the write default waits for M-B's round trip (the test exe is no bundle either way).
        assert!(install_locations().is_empty());
    }
}
