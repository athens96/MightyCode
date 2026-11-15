//! The helper's `install.log` line (C# `WinUI\Program.cs:253-270`; plan §1.1 "After the result"). Owner: W9-A.
//!
//! One line appended to `<dir of --package>\install.log` (or `%TEMP%` without one):
//! `{UtcNow:O} replaced={True|False} rolledBack={…} relaunch={…} skipped={…} started={…} backupRemoved={…} {error}\n`, the time in
//! .NET's round-trip form for a UTC `DateTimeOffset` (`2026-11-15T03:04:05.1234567+00:00`), booleans as .NET prints them. An unexpected
//! panic becomes `"<TypeName>: <message>"` with replaced and rolled back false. Checked against `tools/parity/Update.cs install-log`.

use super::plan::InstallPlan;
use super::replacement::ReplacementResult;

/// The log's file name beside the package.
pub const INSTALL_LOG: &str = "install.log";

/// C#'s line for `result` (with the plan's relaunch) at `now`. Step 0 stub (W9-A writes it): empty.
pub fn line(_result: &ReplacementResult, _plan: Option<&InstallPlan>, _now: chrono::DateTime<chrono::Utc>) -> String {
    String::new()
}

/// .NET's `bool.ToString()`: `True` or `False`.
pub fn dotnet_bool(value: bool) -> &'static str {
    if value { "True" } else { "False" }
}
