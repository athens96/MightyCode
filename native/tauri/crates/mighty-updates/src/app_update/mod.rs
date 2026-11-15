//! The app's own update (W9-A, `.omc/research/tauri/w9-plan.md` §1.1–§1.4): the signed manifest, the version rules, Ed25519, the
//! build's key and address, the check, the download and the staging over the `UpdateTransport` port, the coordinator and the automatic
//! policy, and the hand-off's pure half — the helper's plan, its mutex name, the replacement algorithm over injected seams, the startup
//! housekeeping, `install.log`, the uninstall entry's version and the bare-environment rules. Pure and cross-platform: std::fs for the
//! folders, every process, registry or network step behind a seam the app supplies, so the replacement runs on macOS with temporary
//! folders as C#'s does. No `cfg(target_os)`.
//!
//! | C# | here | owner |
//! |---|---|---|
//! | `AppUpdateManifest.cs` (`AppUpdateAsset`, `AppUpdateManifest`, `AppUpdateAvailability`) | [`manifest`] | A |
//! | `AppUpdateManifest.cs` (`AppVersion`) | [`version`] | A |
//! | `Ed25519Verify.cs` | [`signature`] | A |
//! | `AppUpdateCoordinator.cs:13-41` (`AppUpdateBuild`) | [`build`] | A |
//! | `AppUpdateService.cs` (check, download, kept version, SHA-256) | [`service`] | A |
//! | `AppUpdateService.cs:185-259` (`Stage`) | [`stage`] | A |
//! | `AppUpdateCoordinator.cs:43-463` (`AppUpdateState`, `AppUpdateKeptCheck`, `AppUpdateCoordinator`) | [`coordinator`] | A |
//! | `AppUpdateCoordinator.cs:465-556` (`AppUpdateSmokeOutcome`, `AppUpdateSmoke`) | [`smoke`] | A |
//! | `AppUpdateAutoPolicy.cs` | [`auto`] | A |
//! | `AppUpdateReplacement.cs:9-124` (`AppUpdateInstallPlan`) | [`plan`] | A |
//! | `AppUpdateReplacement.cs:138-159` (`AppUpdateHelperGuard`) | [`guard`] | A |
//! | `AppUpdateReplacement.cs:5-7,126-136,161-581` (`AppUpdateMove`, `AppUpdateReplacementResult`, `RunAsync`) | [`replacement`] | A |
//! | `AppUpdateReplacement.cs` (`RestoreInterruptedInstall`, `PruneBackups`, `StagingBusy`, `TreeMismatch`) | [`housekeeping`] | A |
//! | `InstallerRegistration.cs:14-28` (`UninstallKey`, `VersionToWrite`) | [`installer_entry`] | A |
//! | `UserEnvironment.cs:14-41` (`IsBare`, `Missing`, `Parse`) | [`environment`] | A |
//! | `WinUI\Program.cs:253-270` (the `install.log` line) | [`log`] | A |
//!
//! W9 Step 0 wrote the public types whole and every function as a stub at rest (nothing parses, checks, downloads or replaces until
//! A fills it). The Windows halves (the mutex, the process wait, the detached start, WinHTTP, the registry) are W9-B's
//! (mighty-platform's `UpdateInstaller`, `UpdateTransport` and `ProcessHost::start_detached`); the app's host is W9-C's.

pub mod auto;
pub mod build;
pub mod coordinator;
pub mod environment;
pub mod guard;
pub mod housekeeping;
pub mod installer_entry;
pub mod log;
pub mod manifest;
pub mod plan;
pub mod replacement;
pub mod service;
pub mod signature;
pub mod smoke;
pub mod stage;
pub mod version;

pub use mighty_core::app_update_view::{Phase, State};

/// The app's executable, the one name a package may hold (C# `AppUpdateService.ExecutableName`, `AcceptedExecutableNames`).
pub const EXECUTABLE_NAME: &str = "MightyCode.exe";

/// The executable an install from before the rename holds (C# `LegacyExecutableName`).
pub const LEGACY_EXECUTABLE_NAME: &str = "MightyClaude.exe";

/// Either name marks a whole install (C# `InstalledExecutableNames`), in this order.
pub const INSTALLED_EXECUTABLE_NAMES: [&str; 2] = [EXECUTABLE_NAME, LEGACY_EXECUTABLE_NAME];

/// A refusal as C# throws it: a locale key with its arguments (`appUpdate.error.*`, `settings.appUpdate.*`; worded in the reader's
/// language by [`UpdateError::message`]), or a plain sentence that only reaches `install.log` (C#'s untranslated ones).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateError {
    Key { key: &'static str, args: Vec<(&'static str, String)> },
    Plain(String),
}

impl UpdateError {
    pub fn key(key: &'static str) -> UpdateError {
        UpdateError::Key { key, args: Vec::new() }
    }

    /// The sentence the reader sees, in `locale`'s language.
    pub fn message(&self, locale: &mighty_core::locale::Locale) -> String {
        match self {
            UpdateError::Key { key, args } => {
                let pairs: Vec<(&str, &str)> = args.iter().map(|(name, value)| (*name, value.as_str())).collect();
                locale.get_with(key, &pairs)
            }
            UpdateError::Plain(text) => text.clone(),
        }
    }
}

impl std::fmt::Display for UpdateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UpdateError::Key { key, .. } => f.write_str(key),
            UpdateError::Plain(text) => f.write_str(text),
        }
    }
}

impl std::error::Error for UpdateError {}
