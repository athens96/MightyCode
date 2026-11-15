//! The replacement (C# `AppUpdateMove`, `AppUpdateReplacementResult`, `AppUpdateReplacement.RunAsync`, AppUpdateReplacement.cs; plan
//! §1.1 `RunAsync` order). Owner: W9-A.
//!
//! The whole sequence runs here over [`Seams`] — the process wait, the app's start, the copy, the rename and the running-again check —
//! so the same code that runs in the helper runs in the tests against temporary folders on Windows and macOS (C#'s "app update
//! replacement …" checks). In order: an invalid plan `planInvalid`; `RestoreInterruptedInstall`; the wait for `--wait-pid`
//! (`appStillRunning`); the package's SHA-256 again (`packageMissingBeforeSwap`, `packageRecheckFailed`); the staged folder
//! (`stagedFolderMissing`); the copy to `<install>.update-new` (`installedExecutableMissing`; on failure the copy removed and, when
//! relaunching, the installed app started); with `--relaunch 0` the running-again check (a check that fails counts as running: skipped);
//! `TreeMismatch`; the two renames through [`MOVE_ATTEMPTS`] tries [`MOVE_RETRY_DELAY`] apart on any I/O error; the start; any failure
//! there undone (the rollback is never cancelled: `rolledBack{reason}`); the backup deleted once the new app started. Synchronous: the
//! helper runs before any runtime exists.

use std::path::{Path, PathBuf};
use std::time::Duration;

use mighty_core::locale::Locale;

use super::plan::InstallPlan;

/// How long the helper waits for the app to quit (C# `DefaultExitTimeout`).
pub const DEFAULT_EXIT_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// How often the wait looks (C#: every 200 ms).
pub const EXIT_POLL: Duration = Duration::from_millis(200);

/// How often each rename is tried before its failure counts (C# `MoveAttempts`).
pub const MOVE_ATTEMPTS: u32 = 10;

/// How long apart (C# `MoveRetryDelay`).
pub const MOVE_RETRY_DELAY: Duration = Duration::from_millis(300);

/// How long an install on quit waits while the app runs from the install folder again (C#: 30 s, polled every 500 ms).
pub const RUNNING_AGAIN_TIMEOUT: Duration = Duration::from_secs(30);
pub const RUNNING_AGAIN_POLL: Duration = Duration::from_millis(500);

/// One rename, in the order performed (C# `AppUpdateMove`; `Kind` backup, install or rollback).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoveKind {
    Backup,
    Install,
    Rollback,
}

impl MoveKind {
    /// C#'s `Kind` text.
    pub fn name(self) -> &'static str {
        match self {
            MoveKind::Backup => "backup",
            MoveKind::Install => "install",
            MoveKind::Rollback => "rollback",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Move {
    pub kind: MoveKind,
    pub from: PathBuf,
    pub to: PathBuf,
}

/// What the replacement did (C# `AppUpdateReplacementResult`). `replaced` and `rolled_back` are never both true; `skipped`: an install
/// on quit found the app running again (or another helper held the mutex) and left everything for the next quit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReplacementResult {
    pub replaced: bool,
    pub rolled_back: bool,
    pub error: Option<String>,
    pub moves: Vec<Move>,
    pub started: bool,
    pub backup_removed: bool,
    pub skipped: bool,
}

/// What the replacement asks of the machine (C# `RunAsync`'s injected `waitForExit`, `startApp`, `copier`, `mover`, `runningAgain`).
/// The helper passes the real ones (W9-B: the platform's `UpdateInstaller` and `ProcessHost::start_detached`); tests pass fakes over
/// temporary folders. The defaults are C#'s nulls: a plain recursive copy, a plain rename, no running-again check.
pub trait Seams {
    /// Waits for the plan's `--wait-pid` to exit, at most [`DEFAULT_EXIT_TIMEOUT`]; false when it still runs.
    fn wait_for_exit(&self, plan: &InstallPlan) -> bool;
    /// Starts the app at `executable` detached, with the user's fresh environment, in its own folder.
    fn start_app(&self, executable: &Path) -> std::io::Result<()>;
    fn copy(&self, from: &Path, to: &Path) -> std::io::Result<()> {
        copy_directory(from, to)
    }
    fn move_dir(&self, from: &Path, to: &Path) -> std::io::Result<()> {
        std::fs::rename(from, to)
    }
    /// With `--relaunch 0`: whether a process other than this one runs from the install folder within [`RUNNING_AGAIN_TIMEOUT`]. An
    /// error counts as running.
    fn running_again(&self, _plan: &InstallPlan) -> std::io::Result<bool> {
        Ok(false)
    }
    /// The pause between two tries of a rename (tests pass zero).
    fn move_retry_delay(&self) -> Duration {
        MOVE_RETRY_DELAY
    }
    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
    fn now(&self) -> chrono::DateTime<chrono::Utc> {
        chrono::Utc::now()
    }
}

/// C# `AppUpdateReplacement.Plan(plan)`: the moves a successful replacement performs, in order.
pub fn planned_moves(plan: &InstallPlan) -> [Move; 2] {
    [
        Move { kind: MoveKind::Backup, from: plan.install_directory.clone(), to: plan.backup_directory.clone() },
        Move { kind: MoveKind::Install, from: plan.new_directory(), to: plan.install_directory.clone() },
    ]
}

/// C# `Rollback(plan)`: the move that undoes a failed replacement.
pub fn rollback(plan: &InstallPlan) -> Move {
    Move { kind: MoveKind::Rollback, from: plan.backup_directory.clone(), to: plan.install_directory.clone() }
}

/// C# `ShouldRollback`: the install was moved aside and the new folder is not in place.
pub fn should_rollback(backup_taken: bool, installed: bool) -> bool {
    backup_taken && !installed
}

/// C# `CopyDirectory`: the folder made, its files copied (an existing file is an error), its folders copied the same way.
pub fn copy_directory(source: &Path, destination: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(destination)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let target = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_directory(&entry.path(), &target)?;
        } else {
            std::fs::OpenOptions::new().write(true).create_new(true).open(&target)?;
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// C# `AppUpdateReplacement.RunAsync`, its sentences in `locale`'s language (the helper's: the OS's). Step 0 stub (W9-A writes it):
/// nothing moves, the result says the plan is invalid.
pub fn run(_plan: &InstallPlan, _seams: &dyn Seams, locale: &Locale) -> ReplacementResult {
    ReplacementResult { error: Some(locale.get("appUpdate.error.planInvalid")), ..ReplacementResult::default() }
}
