//! The update's machine half on Windows (W9-B; C# `AppUpdateHelperGuard`'s mutex, `AppUpdateReplacement.WaitForExitAsync`,
//! `AnyRunningFrom`, `UserEnvironment.Fresh`, `InstallerRegistration.SyncDisplayVersion`, `WinUI\RenamedInstall.cs`; plan §W9-B):
//! - `helper_mutex`: `CreateMutexW` + `WaitForSingleObject(0)` (`WAIT_ABANDONED` counts as taken);
//! - `wait_for_exit`: `OpenProcess(SYNCHRONIZE)` + `WaitForSingleObject`, a missing pid exited;
//! - `running_from`: `CreateToolhelp32Snapshot` for `MightyCode.exe` / `MightyClaude.exe`, `QueryFullProcessImageNameW`, the prefix
//!   compared without case, this process left out, one that cannot be opened skipped;
//! - `fresh_environment`: `OpenProcessToken(TOKEN_QUERY)` + `CreateEnvironmentBlock(inherit = FALSE)` (today's `user_environment`);
//! - `sync_display_version`: HKCU's uninstall entry (`mighty_updates::app_update::installer_entry`'s decision);
//! - `refresh_renamed_install`: `IShellLinkW` / `IPersistFile` over the Programs and Desktop shortcuts, the entry's icon and name.
//!
//! Step 0: every seam answers `Unsupported`, as the port did before W9.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use mighty_platform::{HelperMutex, PlatformError, Result, StagedUpdate, UpdateInstaller};

fn no<T>(operation: &'static str) -> Result<T> {
    Err(PlatformError::unsupported("UpdateInstaller", operation))
}

/// The Windows update installer (W9-B fills it).
#[derive(Debug, Clone, Copy, Default)]
pub struct WindowsUpdates;

impl UpdateInstaller for WindowsUpdates {
    fn install_location(&self) -> Option<PathBuf> {
        None
    }
    fn hand_off(&self, _update: &StagedUpdate, _relaunch: bool) -> Result<()> {
        no("hand_off")
    }
    fn helper_mutex(&self, _name: &str) -> Result<Option<HelperMutex>> {
        no("helper_mutex")
    }
    fn wait_for_exit(&self, _pid: u32, _timeout: Duration) -> Result<bool> {
        no("wait_for_exit")
    }
    fn running_from(&self, _folder: &Path, _except_self: bool) -> Result<bool> {
        no("running_from")
    }
    fn fresh_environment(&self) -> Option<BTreeMap<String, String>> {
        None
    }
    fn sync_display_version(&self, _app_directory: &Path, _version: &str) -> Result<bool> {
        no("sync_display_version")
    }
    fn refresh_renamed_install(&self, _app_directory: &Path) -> Result<()> {
        no("refresh_renamed_install")
    }
}
