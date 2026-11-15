//! One helper per install (C# `AppUpdateHelperGuard`, AppUpdateReplacement.cs:138-159; plan §1.1). Owner: W9-A.
//!
//! The mutex name is `Local\MightyCode-update-helper-` + the first 32 lower-case hex characters of SHA-256(UTF-8 of
//! `GetFullPath(TrimEnd(dir, '\\', '/')).ToUpperInvariant()`), with .NET's `ToUpperInvariant` (one char to one char, simple mapping:
//! `ß` stays `ß`, unlike `str::to_uppercase`), so a C# helper and a Rust helper on one folder exclude each other (the `mutex-name`
//! parity corpus). The mutex itself is W9-B's (`UpdateInstaller::helper_mutex`).

use std::path::Path;

use super::replacement::ReplacementResult;

/// The name's fixed part (C# `MutexName`'s prefix).
pub const MUTEX_PREFIX: &str = "Local\\MightyCode-update-helper-";

/// The reason a second helper leaves with (C# `Refusal`), in `install.log` only.
pub const ALREADY_RUNNING: &str = "another update helper is already running for this install";

/// C# `AppUpdateHelperGuard.MutexName(installDirectory)`. Step 0 stub (W9-A writes it): the prefix alone.
pub fn mutex_name(_install_directory: &Path) -> String {
    MUTEX_PREFIX.to_string()
}

/// C# `Refusal(acquired)`: None when this helper holds the mutex; else the skipped result it leaves with (exit 0, nothing moved).
pub fn refusal(acquired: bool) -> Option<ReplacementResult> {
    (!acquired).then(|| ReplacementResult { error: Some(ALREADY_RUNNING.into()), skipped: true, ..ReplacementResult::default() })
}
