//! The helper's plan (C# `AppUpdateInstallPlan`, AppUpdateReplacement.cs:9-124; plan §1.1). Owner: W9-A.
//!
//! The argument grammar is the 0.4.x app's, which starts this exe from its staged folder: `--update-helper`, then pairs whose keys start
//! with `--` (a key without makes the plan None), a later duplicate overwriting, an odd trailing argument ignored; required `--staged`,
//! `--install`, `--backup`, `--package`, `--sha256` (lower-cased), `--size` (> 0), `--wait-pid` (> 0), the numbers read as .NET's
//! `long.TryParse`/`int.TryParse` read them; `--relaunch` optional (absent = `1`, else `1` or `0`). `is_valid`: non-empty paths, 64
//! lower-case hex, size and pid positive, and the backup not inside the install folder (full paths, ordinal, with a trailing
//! separator). Checked against C# by `native/contracts/fixtures/update-plan.json`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::dotnet;

/// The first argument of the helper mode (C# `HelperFlag`).
pub const HELPER_FLAG: &str = "--update-helper";

/// The optional last pair's key (C# `RelaunchFlag`).
pub const RELAUNCH_FLAG: &str = "--relaunch";

/// The whole environment the helper is started with (C# `HelperEnvironment`): nothing inherited, no token, nothing about the user.
pub const HELPER_ENVIRONMENT: [(&str, &str); 1] = [("SystemRoot", "C:\\Windows")];

/// Everything the helper needs, from its arguments alone (C# `AppUpdateInstallPlan`). `relaunch` false is the install on quit.
#[derive(Clone, PartialEq, Eq)]
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

/// No folder: the paths name the user.
impl std::fmt::Debug for InstallPlan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InstallPlan")
            .field("sha256", &self.sha256)
            .field("size", &self.size)
            .field("app_process_id", &self.app_process_id)
            .field("relaunch", &self.relaunch)
            .finish_non_exhaustive()
    }
}

impl InstallPlan {
    /// C# `AppUpdateInstallPlan.Create`: the backup beside the install as `<name>.backup-<yyyyMMddTHHmmssfff>` (UTC `now`) unless given
    /// (beside the system's temporary folder when the install has no parent, as C#'s `?? Path.GetTempPath()`). Relaunches.
    #[allow(clippy::too_many_arguments)]
    pub fn create(
        staged: &Path,
        install: &Path,
        package: &Path,
        sha256: &str,
        size: i64,
        app_process_id: i32,
        backup: Option<&Path>,
        now: chrono::DateTime<chrono::Utc>,
    ) -> InstallPlan {
        let backup = backup.map(Path::to_path_buf).unwrap_or_else(|| {
            let trimmed = dotnet::trim_separator(install);
            let parent = trimmed.parent().map(Path::to_path_buf).unwrap_or_else(std::env::temp_dir);
            parent.join(format!("{}.backup-{}", dotnet::file_name(&trimmed), now.format("%Y%m%dT%H%M%S%3f")))
        });
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

    /// C# `StagedHelperExecutable`: the staged version's own exe, which the running app starts as the helper (C# looks for each
    /// accepted name, of which there is one, and falls back to the same path).
    pub fn staged_helper_executable(&self) -> PathBuf {
        self.staged_directory.join(super::EXECUTABLE_NAME)
    }

    /// C# `NewDirectory`: `Path.GetFullPath(install.TrimEnd(separator)) + ".update-new"`, beside the install. (Where .NET's
    /// `GetFullPath` would throw, the trimmed path as given.)
    pub fn new_directory(&self) -> PathBuf {
        let trimmed = dotnet::trim_separator(&self.install_directory);
        dotnet::with_suffix(&dotnet::full_path(&trimmed).unwrap_or(trimmed), ".update-new")
    }

    /// C# `IsValid`.
    pub fn is_valid(&self) -> bool {
        let empty = |p: &Path| p.as_os_str().is_empty();
        if empty(&self.staged_directory) || empty(&self.install_directory) || empty(&self.backup_directory) || empty(&self.package_path) {
            return false;
        }
        let hex = self.sha256.len() == 64 && self.sha256.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if !hex || self.size <= 0 || self.app_process_id <= 0 {
            return false;
        }
        // The backup must not sit inside the folder that is about to be moved.
        let (Ok(backup), Ok(install)) = (dotnet::full_path(&self.backup_directory), dotnet::full_path(&self.install_directory)) else {
            return false;
        };
        let mut prefix = dotnet::trim_separator(&install).into_os_string();
        prefix.push(dotnet::SEPARATOR.to_string());
        !backup.as_os_str().as_encoded_bytes().starts_with(prefix.as_encoded_bytes())
    }
}

/// C# `AppUpdateInstallPlan.TryParse(arguments)`: the plan, or None (the arguments then go to the normal parser, which refuses
/// `--update-helper`, as C#'s does).
pub fn try_parse(arguments: &[String]) -> Option<InstallPlan> {
    if arguments.first().map(String::as_str) != Some(HELPER_FLAG) {
        return None;
    }
    let mut values: HashMap<&str, &str> = HashMap::new();
    let mut index = 1;
    while index + 1 < arguments.len() {
        if !arguments[index].starts_with("--") {
            return None;
        }
        values.insert(&arguments[index], &arguments[index + 1]);
        index += 2;
    }
    let staged = values.get("--staged")?;
    let install = values.get("--install")?;
    let backup = values.get("--backup")?;
    let package = values.get("--package")?;
    let sha256 = values.get("--sha256")?;
    let size = values.get("--size").and_then(|text| dotnet::parse_i64(text)).filter(|size| *size > 0)?;
    let pid = values.get("--wait-pid").and_then(|text| dotnet::parse_i32(text)).filter(|pid| *pid > 0)?;
    let relaunch = match values.get(RELAUNCH_FLAG) {
        None => true,
        Some(&"1") => true,
        Some(&"0") => false,
        Some(_) => return None,
    };
    let (install, backup) = (PathBuf::from(install), PathBuf::from(backup));
    let plan = InstallPlan {
        staged_directory: PathBuf::from(staged),
        new_executable: install.join(super::EXECUTABLE_NAME),
        previous_executable: backup.join(super::EXECUTABLE_NAME),
        install_directory: install,
        backup_directory: backup,
        package_path: PathBuf::from(package),
        // `ToLowerInvariant`: only an all-ASCII hex text makes a valid plan, and no other character lowers to a hex digit.
        sha256: sha256.to_ascii_lowercase(),
        size,
        app_process_id: pid,
        relaunch,
    };
    plan.is_valid().then_some(plan)
}
