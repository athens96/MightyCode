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
/// folder, the same version). Step 0 stub (W9-A writes it): nothing to write.
pub fn version_to_write(
    _install_location: Option<&str>,
    _display_version: Option<&str>,
    _app_directory: &Path,
    _version: &str,
) -> Option<String> {
    None
}
