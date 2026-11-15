//! M-H: the Mac's update installer: the port of Swift's `installScript` (byte for byte against `tools/parity-mac install-script`),
//! started detached as `/bin/sh install.sh` with exactly `PATH=/usr/bin:/bin` and `HOME`, `install.log` made 0600, staging through
//! `/usr/bin/ditto` (a Rust zip reader could break framework links and code signature resources). Swift `MightyCore/AppUpdate.swift`
//! (440 lines), `AppUpdateAuto*.swift`, `MightyCode/AppStore+AppUpdate.swift` (461). The 0.4.x → 0.5 hand-off needs none of it: Swift's
//! own script installs the Rust bundle (plan fact 1). Contract `mighty_platform::update` (`UpdateInstaller`).

use std::path::PathBuf;
use std::sync::Arc;

use mighty_platform::UpdateInstaller;
use mighty_platform::unsupported::Unsupported;

/// The Mac's `UpdateInstaller` (Unsupported until M-H), also for the exe's modes that run before any profile.
pub fn update_installer() -> Arc<dyn UpdateInstaller> {
    Arc::new(Unsupported)
}

/// Where a release build counts as installed, for the write policy (src-tauri `startup::installed` through `platform.rs`; plan D7):
/// the exe's folder must be one of these, compared canonically. M-H fills it (`Contents/MacOS` of an `.app` in `/Applications` or
/// `~/Applications`, never under App Translocation); none until then, so a release Mac build writes only with
/// `--allow-state-write`.
pub fn install_locations() -> Vec<PathBuf> {
    Vec::new()
}
