//! One helper per install (C# `AppUpdateHelperGuard`, AppUpdateReplacement.cs:138-159; plan §1.1). Owner: W9-A.
//!
//! The mutex name is `Local\MightyCode-update-helper-` + the first 32 lower-case hex characters of SHA-256(UTF-8 of
//! `GetFullPath(TrimEnd(dir, '\\', '/')).ToUpperInvariant()`), with .NET's `ToUpperInvariant` (one char to one char, simple mapping:
//! `ß` stays `ß`, unlike `str::to_uppercase`), so a C# helper and a Rust helper on one folder exclude each other (the `mutex-name`
//! parity corpus, `native/contracts/fixtures/update-mutex.json`). The mutex itself is W9-B's (`UpdateInstaller::helper_mutex`).

use std::path::Path;

use sha2::{Digest, Sha256};

use super::dotnet;
use super::replacement::ReplacementResult;

/// The name's fixed part (C# `MutexName`'s prefix).
pub const MUTEX_PREFIX: &str = "Local\\MightyCode-update-helper-";

/// The reason a second helper leaves with (C# `Refusal`), in `install.log` only.
pub const ALREADY_RUNNING: &str = "another update helper is already running for this install";

/// C# `AppUpdateHelperGuard.MutexName(installDirectory)`: the same folder, however it is written (case, trailing separator), gives
/// the same name; another folder another one. (Where .NET's `GetFullPath` would throw, the trimmed path as given.)
pub fn mutex_name(install_directory: &Path) -> String {
    // `TrimEnd(DirectorySeparatorChar, AltDirectorySeparatorChar)`: `\` and `/` on Windows, `/` elsewhere.
    let trimmed = dotnet::trim_end(install_directory, &[dotnet::SEPARATOR, '/']);
    let full = dotnet::full_path(&trimmed).unwrap_or(trimmed);
    let folder = dotnet::to_upper_invariant(&full.to_string_lossy());
    let digest = Sha256::digest(folder.as_bytes());
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    format!("{MUTEX_PREFIX}{}", &hex[..32])
}

/// C# `Refusal(acquired)`: None when this helper holds the mutex; else the skipped result it leaves with (exit 0, nothing moved).
pub fn refusal(acquired: bool) -> Option<ReplacementResult> {
    (!acquired).then(|| ReplacementResult { error: Some(ALREADY_RUNNING.into()), skipped: true, ..ReplacementResult::default() })
}
