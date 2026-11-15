//! The helper's `install.log` line (C# `WinUI\Program.cs:253-270`; plan §1.1 "After the result"). Owner: W9-A.
//!
//! One line appended to `<dir of --package>\install.log` (or `%TEMP%` without one):
//! `{UtcNow:O} replaced={True|False} rolledBack={…} relaunch={…} skipped={…} started={…} backupRemoved={…} {error}\n`, the time in
//! .NET's round-trip form for a UTC `DateTimeOffset` (`2026-11-15T03:04:05.1234567+00:00`), booleans as .NET prints them (no error:
//! the line ends with the space before it). An unexpected failure becomes `"<TypeName>: <message>"` with replaced and rolled back
//! false ([`unexpected`]). Checked against `tools/parity/Update.cs install-log` (`native/contracts/fixtures/update-install-log.json`).

use std::io::Write;
use std::path::{Path, PathBuf};

use super::plan::InstallPlan;
use super::replacement::ReplacementResult;

/// The log's file name beside the package.
pub const INSTALL_LOG: &str = "install.log";

/// C#'s line for `result` with the plan's relaunch at `now`. Without a plan, `relaunch` reads C#'s default for a list without
/// `--relaunch`: `True`.
pub fn line(result: &ReplacementResult, plan: Option<&InstallPlan>, now: chrono::DateTime<chrono::Utc>) -> String {
    format!(
        "{} replaced={} rolledBack={} relaunch={} skipped={} started={} backupRemoved={} {}\n",
        super::dotnet::round_trip(now),
        dotnet_bool(result.replaced),
        dotnet_bool(result.rolled_back),
        dotnet_bool(plan.is_none_or(|plan| plan.relaunch)),
        dotnet_bool(result.skipped),
        dotnet_bool(result.started),
        dotnet_bool(result.backup_removed),
        result.error.as_deref().unwrap_or_default(),
    )
}

/// .NET's `bool.ToString()`: `True` or `False`.
pub fn dotnet_bool(value: bool) -> &'static str {
    if value { "True" } else { "False" }
}

/// Where the line goes: `Path.Combine(Path.GetDirectoryName(plan.PackagePath) ?? Path.GetTempPath(), "install.log")`.
pub fn path(plan: &InstallPlan, temp_directory: &Path) -> PathBuf {
    plan.package_path.parent().unwrap_or(temp_directory).join(INSTALL_LOG)
}

/// `File.AppendAllText(path, line)`: created when missing, appended to otherwise, so a second helper's line and the first one's
/// outcome are both kept. The caller ignores a failure (the outcome must not depend on the log).
pub fn append(path: &Path, line: &str) -> std::io::Result<()> {
    std::fs::OpenOptions::new().create(true).append(true).open(path)?.write_all(line.as_bytes())
}

/// C#'s catch of an exception the replacement did not expect: `"<TypeName>: <message>"`, nothing replaced, rolled back, moved or
/// started (the helper's panic, caught with `catch_unwind`).
pub fn unexpected(type_name: &str, message: &str) -> ReplacementResult {
    ReplacementResult { error: Some(format!("{type_name}: {message}")), ..ReplacementResult::default() }
}
