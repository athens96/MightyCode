//! The folders around the install (C# `AppUpdateReplacement.RestoreInterruptedInstall`, `PruneBackups`, `StagingBusy`, `TreeMismatch`,
//! `VerifyInstallTree`, `InstalledExecutable`; plan §1.1 "App-start housekeeping", §1.3 `StagingBusy`). Owner: W9-A.
//!
//! Run by the helper (restore, tree mismatch) and at app start outside a smoke where the app may change the machine (W9-C: restore,
//! prune, then `SyncDisplayVersion`). A process check C# makes itself (`AnyRunningFrom`) is passed in here: it is the platform's
//! (`UpdateInstaller::running_from`, W9-B).

use std::path::{Path, PathBuf};
use std::time::Duration;

/// A `.update-new` copy written this recently may still be filling (C# `NewCopyGrace`): pruning and staging leave it alone.
pub const NEW_COPY_GRACE: Duration = Duration::from_secs(10 * 60);

/// C# `VerifyInstallTree`: the folder holds `MightyCode.exe` or `MightyClaude.exe`.
pub fn verify_install_tree(install_directory: &Path) -> bool {
    super::INSTALLED_EXECUTABLE_NAMES.iter().any(|name| install_directory.join(name).is_file())
}

/// C# `InstalledExecutable`: the exe the install folder holds (this version's name first), this version's name when neither is there.
pub fn installed_executable(install_directory: &Path) -> PathBuf {
    super::INSTALLED_EXECUTABLE_NAMES
        .iter()
        .map(|name| install_directory.join(name))
        .find(|path| path.is_file())
        .unwrap_or_else(|| install_directory.join(super::EXECUTABLE_NAME))
}

/// C# `RestoreInterruptedInstall`: the newest whole `<name>.backup-*` moved back when the install is missing or holds no exe. Never
/// fails. Step 0 stub (W9-A writes it): nothing is put back.
pub fn restore_interrupted_install(_install_directory: &Path) -> bool {
    false
}

/// C# `PruneBackups`: every `<name>.backup-*` deleted, and `<name>.update-new` unless written within [`NEW_COPY_GRACE`] of `now` (an
/// unreadable one kept); how many backups went. Step 0 stub (W9-A writes it): nothing is removed.
pub fn prune_backups(_install_directory: &Path, _now: chrono::DateTime<chrono::Utc>) -> usize {
    0
}

/// C# `StagingBusy`: a process other than this one runs from under `updates_directory` (`running_from`, an error reads as busy), or
/// `<install>.update-new` was written within [`NEW_COPY_GRACE`] (an unreadable copy is not busy). Step 0 stub (W9-A writes it): never
/// busy.
pub fn staging_busy(
    _updates_directory: &Path,
    _install_directory: Option<&Path>,
    _now: chrono::DateTime<chrono::Utc>,
    _running_from: &dyn Fn(&Path) -> std::io::Result<bool>,
) -> bool {
    false
}

/// C# `TreeMismatch(expected, actual)`: why the copy is not the staged tree (`missing <file>`, `unexpected <file>`, `size differs for
/// <file>`; relative paths compared ordinal), or None. Step 0 stub (W9-A writes it): no mismatch is found.
pub fn tree_mismatch(_expected: &Path, _actual: &Path) -> std::io::Result<Option<String>> {
    Ok(None)
}
