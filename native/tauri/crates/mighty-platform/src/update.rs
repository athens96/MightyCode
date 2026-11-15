//! Replacing the installed app with a staged update (C# `AppUpdateReplacement` + `--update-helper`, docs/windows-app-update.md;
//! Swift `AppUpdate` + `install.sh`, docs/app-update.md). Checking, downloading and verifying the signed manifest are shared logic;
//! only the swap is the OS's.
//!
//! W9 (plan §0d, owner W9-B): the port grows to the helper's seams, the machine half of C#'s `AppUpdateReplacement`,
//! `InstallerRegistration`, `UserEnvironment` and `RenamedInstall`; mighty-updates' `app_update` holds the decisions. Windows writes
//! them (`mighty-platform-windows` `update.rs`); the Mac answers `Unsupported` until its stage.

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
}
