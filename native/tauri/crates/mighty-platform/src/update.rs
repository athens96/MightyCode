//! Replacing the installed app with a staged update (C# `AppUpdateReplacement` + `--update-helper`, docs/windows-app-update.md;
//! Swift `AppUpdate` + `install.sh`, docs/app-update.md). Checking, downloading and verifying the signed manifest are shared logic;
//! only the swap is the OS's.
//!
//! W9 (plan §0d, owner W9-B): the port grows to the helper's seams, the machine half of C#'s `AppUpdateReplacement`,
//! `InstallerRegistration`, `UserEnvironment` and `RenamedInstall`; mighty-updates' `app_update` holds the decisions. Windows writes
//! them (`mighty-platform-windows` `update.rs`); the Mac answers `Unsupported` until its stage.
//!
//! M-H (the Mac stage) refines the contract with four defaulted methods the Mac's update needs and Windows never calls
//! ([`UpdateInstaller::system_version`], [`UpdateInstaller::may_write`], [`UpdateInstaller::may_execute`],
//! [`UpdateInstaller::start_install_script`]): the Mac's swap is Swift's `install.sh`, whose text and checks are mighty-updates'
//! (`app_update::install_script`); the platform only says what the OS knows and starts the script.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::Result;

/// A verified package, unpacked into the updates folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedUpdate {
    pub version: String,
    /// The downloaded archive.
    pub package: PathBuf,
    /// The new app inside the staging folder: `MightyCode.exe` (Windows) or `MightyCode.app` (the Mac).
    pub staged: PathBuf,
    /// The package's SHA-256 (lower-case hex) and size, which the Windows helper checks again before the swap.
    pub sha256: String,
    pub size: u64,
}

/// The helper's mutex while it runs (C# `Mutex` held in `RunUpdateHelper`): released when dropped.
pub struct HelperMutex(#[allow(dead_code, reason = "held only to be dropped: dropping it releases the mutex")] Box<dyn Send>);

impl HelperMutex {
    /// Holds `release` (the OS handle's owner) until dropped.
    pub fn new(release: impl Send + 'static) -> HelperMutex {
        HelperMutex(Box::new(release))
    }
}

impl std::fmt::Debug for HelperMutex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HelperMutex")
    }
}

/// Contract: `hand_off` starts the OS's helper and returns once it runs; the caller then quits. The helper waits for this process
/// to exit (5 min at most), swaps the installed app for `update.staged` by two renames with a rollback (Windows also restores an
/// interrupted install first, re-checks the SHA-256, syncs the uninstall entry's `DisplayVersion` and appends `install.log`), and
/// starts the new app with a fresh user environment when `relaunch`. The 0.4.x helper arguments stay valid (`--update-helper
/// --staged --install --backup --package --sha256 --size --wait-pid --relaunch`).
///
/// The helper's seams (W9): none elevates, none shows a window, none writes outside the install's folders but the uninstall entry.
pub trait UpdateInstaller: Send + Sync {
    /// The installed app this copy would replace, or None when it cannot update itself (a development build, a folder it cannot write).
    fn install_location(&self) -> Option<PathBuf>;
    fn hand_off(&self, update: &StagedUpdate, relaunch: bool) -> Result<()>;

    /// The named mutex `name` (`Local\MightyCode-update-helper-…`), taken without waiting: Some while this process holds it (an
    /// abandoned one counts as taken), None when another holds it (C# `WaitOne(0)`).
    fn helper_mutex(&self, name: &str) -> Result<Option<HelperMutex>>;
    /// Waits until process `pid` has exited, at most `timeout`: true once it has (a pid that is gone has), false when it still runs.
    fn wait_for_exit(&self, pid: u32, timeout: Duration) -> Result<bool>;
    /// Whether a process other than this one (when `except_self`) runs an image under `folder` (`MightyCode.exe`, `MightyClaude.exe`;
    /// the image path compared without case); a process that cannot be opened is skipped (C# `AnyRunningFrom`).
    fn running_from(&self, folder: &Path, except_self: bool) -> Result<bool>;
    /// The signed-in user's own environment as Windows builds it for a new logon (C# `UserEnvironment.Fresh`), or None when it cannot
    /// be built.
    fn fresh_environment(&self) -> Option<BTreeMap<String, String>>;
    /// C# `InstallerRegistration.SyncDisplayVersion`: the uninstall entry's `DisplayVersion` brought to `version` when the entry names
    /// `app_directory`; true when it wrote one. Never in a test (the registry port's fake there).
    fn sync_display_version(&self, app_directory: &Path, version: &str) -> Result<bool>;
    /// C# `RenamedInstall.Refresh`: the Programs and Desktop shortcuts that start `MightyClaude.exe` re-pointed to `MightyCode.exe`, and
    /// the uninstall entry's icon and name when they still name the old exe.
    fn refresh_renamed_install(&self, app_directory: &Path) -> Result<()>;

    /// M-H, the Mac: the OS's version as `major.minor.patch` (Swift `AppVersion.systemVersion`), which a manifest's
    /// `minimumSystemVersion` is compared with. None by default (Windows' package names no minimum).
    fn system_version(&self) -> Option<String> {
        None
    }

    /// M-H, the Mac: whether this user may write into `folder` (`access(2)` with `W_OK`, Swift's `isWritableFile`): the installed app's
    /// folder must be, or the swap cannot happen. False by default.
    fn may_write(&self, _folder: &Path) -> bool {
        false
    }

    /// M-H, the Mac: whether this user may run `file` (`access(2)` with `X_OK`, Swift's `isExecutableFile`): a staged bundle's
    /// executable must be. False by default.
    fn may_execute(&self, _file: &Path) -> bool {
        false
    }

    /// M-H, the Mac's hand-off (Swift `AppUpdateService.launchInstaller`, AppUpdate.swift:406-425): `script` written to
    /// `folder/install.sh` (replaced whole, mode 0700), `folder/install.log` made empty with mode 0600, then `/bin/sh install.sh`
    /// started detached (it outlives the app) with exactly `PATH=/usr/bin:/bin` and `HOME`, its standard input empty and its output into
    /// `install.log`. Returns once it runs; the caller then quits (with a relaunch) or goes on quitting. `Unsupported` by default.
    fn start_install_script(&self, _script: &str, _folder: &Path) -> Result<()> {
        Err(crate::PlatformError::unsupported("UpdateInstaller", "start_install_script"))
    }
}
