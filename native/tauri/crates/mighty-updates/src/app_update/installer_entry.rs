//! The setup's uninstall entry, seen from the app (C# `InstallerRegistration.cs:14-28`; plan §1.1 "After the result"). Owner: W9-A (the
//! decision); W9-B writes it through the platform (`UpdateInstaller::sync_display_version`, HKCU, never in a test).
//!
//! The app writes its own `major.minor.patch` into `DisplayVersion` only when the entry's `InstallLocation` (trimmed of `\` and `/`,
//! compared without case) is the folder it runs from and the version differs, so Settings › Apps shows the version an in-app update put
//! there.

use std::path::Path;

/// The setup's AppId with Inno Setup's suffix, under HKCU (C# `UninstallKey`; it must match MightyCode.iss).
pub const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\{F2F99AEF-FB35-4476-A3EE-49453FA0527F}_is1";

/// C# `VersionToWrite(installLocation, displayVersion, appDirectory, version)`: the version to write, or None (no entry, another
/// folder, the same version). Folders compare without case and without a trailing `\` or `/` (both, on every OS: the Mac runs these
/// checks on Windows paths too).
pub fn version_to_write(
    install_location: Option<&str>,
    display_version: Option<&str>,
    app_directory: &Path,
    version: &str,
) -> Option<String> {
    let install_location = install_location.filter(|location| !location.trim().is_empty())?;
    if version.trim().is_empty() {
        return None;
    }
    let folder = |path: &str| path.trim().trim_end_matches(['\\', '/']).to_string();
    if !super::dotnet::eq_ignore_case(&folder(install_location), &folder(&app_directory.to_string_lossy())) {
        return None;
    }
    (display_version != Some(version)).then(|| version.to_string())
}

/// The helper's own version as C#'s `Assembly.GetName().Version.ToString(3)` writes it, from Cargo's parts (`0.5.0-rc.1` →
/// `0.5.0`): pass `env!("CARGO_PKG_VERSION_MAJOR")`, `…_MINOR`, `…_PATCH`.
pub fn display_version(major: &str, minor: &str, patch: &str) -> String {
    format!("{major}.{minor}.{patch}")
}
