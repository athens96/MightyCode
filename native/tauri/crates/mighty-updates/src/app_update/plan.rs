//! The helper's plan (C# `AppUpdateInstallPlan`, AppUpdateReplacement.cs:9-124; plan §1.1). Owner: W9-A.
//!
//! The argument grammar is the 0.4.x app's, which starts this exe from its staged folder: `--update-helper`, then pairs whose keys start
//! with `--` (a key without makes the plan None), a later duplicate overwriting, an odd trailing argument ignored; required `--staged`,
//! `--install`, `--backup`, `--package`, `--sha256` (lower-cased), `--size` (> 0), `--wait-pid` (> 0); `--relaunch` optional (absent =
//! `1`, else `1` or `0`). `is_valid`: non-empty paths, 64 lower-case hex, size and pid positive, and the backup not inside the install
//! folder (full paths, ordinal, with a trailing separator).

use std::path::{Path, PathBuf};

/// The first argument of the helper mode (C# `HelperFlag`).
pub const HELPER_FLAG: &str = "--update-helper";

/// The optional last pair's key (C# `RelaunchFlag`).
pub const RELAUNCH_FLAG: &str = "--relaunch";

/// The whole environment the helper is started with (C# `HelperEnvironment`): nothing inherited, no token, nothing about the user.
pub const HELPER_ENVIRONMENT: [(&str, &str); 1] = [("SystemRoot", "C:\\Windows")];

/// Everything the helper needs, from its arguments alone (C# `AppUpdateInstallPlan`). `relaunch` false is the install on quit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallPlan {
    pub staged_directory: PathBuf,
    pub install_directory: PathBuf,
    pub backup_directory: PathBuf,
    pub package_path: PathBuf,
    pub sha256: String,
    pub size: i64,
    pub app_process_id: i32,
    /// `install\MightyCode.exe`.
    pub new_executable: PathBuf,
    /// `backup\MightyCode.exe`.
    pub previous_executable: PathBuf,
    pub relaunch: bool,
}

impl InstallPlan {
    /// C# `AppUpdateInstallPlan.Create`: the backup beside the install as `<name>.backup-<yyyyMMddTHHmmssfff>` (UTC `now`) unless given.
    /// Step 0 stub (W9-A writes it): the paths as given, the backup `backup` or the install's own path.
    #[allow(clippy::too_many_arguments)]
    pub fn create(
        staged: &Path,
        install: &Path,
        package: &Path,
        sha256: &str,
        size: i64,
        app_process_id: i32,
        backup: Option<&Path>,
        _now: chrono::DateTime<chrono::Utc>,
    ) -> InstallPlan {
        let backup = backup.unwrap_or(install).to_path_buf();
        InstallPlan {
            staged_directory: staged.to_path_buf(),
            install_directory: install.to_path_buf(),
            new_executable: install.join(super::EXECUTABLE_NAME),
            previous_executable: backup.join(super::EXECUTABLE_NAME),
            backup_directory: backup,
            package_path: package.to_path_buf(),
            sha256: sha256.to_string(),
            size,
            app_process_id,
            relaunch: true,
        }
    }

    /// C# `HelperArguments`: the list the helper is started with, every value it needs.
    pub fn helper_arguments(&self) -> Vec<String> {
        let path = |p: &Path| p.to_string_lossy().into_owned();
        vec![
            HELPER_FLAG.into(),
            "--staged".into(),
            path(&self.staged_directory),
            "--install".into(),
            path(&self.install_directory),
            "--backup".into(),
            path(&self.backup_directory),
            "--package".into(),
            path(&self.package_path),
            "--sha256".into(),
            self.sha256.clone(),
            "--size".into(),
            self.size.to_string(),
            "--wait-pid".into(),
            self.app_process_id.to_string(),
            RELAUNCH_FLAG.into(),
            if self.relaunch { "1" } else { "0" }.into(),
        ]
    }

    /// C# `StagedHelperExecutable`: the staged version's own exe, which the running app starts as the helper. Step 0 stub (W9-A writes
    /// it): `staged\MightyCode.exe`.
    pub fn staged_helper_executable(&self) -> PathBuf {
        self.staged_directory.join(super::EXECUTABLE_NAME)
    }

    /// C# `NewDirectory`: `<full install path>.update-new`, beside the install. Step 0 stub (W9-A writes it): the install's path as given
    /// with the suffix.
    pub fn new_directory(&self) -> PathBuf {
        let mut name = self.install_directory.as_os_str().to_owned();
        name.push(".update-new");
        PathBuf::from(name)
    }

    /// C# `IsValid`. Step 0 stub (W9-A writes it): no plan is valid.
    pub fn is_valid(&self) -> bool {
        false
    }
}

/// C# `AppUpdateInstallPlan.TryParse(arguments)`: the plan, or None (the arguments then go to the normal parser, which refuses
/// `--update-helper`, as C#'s does). Step 0 stub (W9-A writes it): None, so the exe behaves as before W9.
pub fn try_parse(_arguments: &[String]) -> Option<InstallPlan> {
    None
}
