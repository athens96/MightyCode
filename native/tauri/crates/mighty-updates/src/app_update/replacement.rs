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

use super::housekeeping::{installed_executable, remove_folder, restore_interrupted_install, tree_mismatch, verify_install_tree};
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
    /// `Directory.Move`: refused when anything is already at `to`.
    fn move_dir(&self, from: &Path, to: &Path) -> std::io::Result<()> {
        super::housekeeping::move_directory(from, to)
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

/// C# `CopyDirectory`: the folder made, its files copied (an existing file is an error), then its folders copied the same way.
pub fn copy_directory(source: &Path, destination: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(destination)?;
    let mut folders = Vec::new();
    for entry in std::fs::read_dir(source)? {
        let path = entry?.path();
        let Some(name) = path.file_name() else { continue };
        let target = destination.join(name);
        if path.is_dir() {
            folders.push((path, target));
        } else {
            std::fs::OpenOptions::new().write(true).create_new(true).open(&target)?;
            std::fs::copy(&path, &target)?;
        }
    }
    for (folder, target) in folders {
        copy_directory(&folder, &target)?;
    }
    Ok(())
}

/// C#'s exception type name for an I/O failure it did not catch (`"<TypeName>: <message>"` in install.log).
fn exception_name(error: &std::io::Error) -> &'static str {
    if error.kind() == std::io::ErrorKind::PermissionDenied { "UnauthorizedAccessException" } else { "IOException" }
}

/// C# `MoveWithRetryAsync`: one rename through the seam, tried up to [`MOVE_ATTEMPTS`] times the seam's delay apart while it fails;
/// the last failure is returned. (C# retries `IOException` and `UnauthorizedAccessException`, which every `io::Error` stands for.)
fn move_with_retry(seams: &dyn Seams, from: &Path, to: &Path) -> std::io::Result<()> {
    let mut attempt = 1;
    loop {
        match seams.move_dir(from, to) {
            Ok(()) => return Ok(()),
            Err(error) if attempt >= MOVE_ATTEMPTS => return Err(error),
            Err(_) => {
                seams.sleep(seams.move_retry_delay());
                attempt += 1;
            }
        }
    }
}

/// C# `TryStartAsync`: whether the app at `executable` started.
fn try_start(seams: &dyn Seams, executable: &Path) -> bool {
    seams.start_app(executable).is_ok()
}

/// A result that replaced nothing.
fn refused(error: String, started: bool) -> ReplacementResult {
    ReplacementResult { error: Some(error), started, ..ReplacementResult::default() }
}

/// C# `VerifyBeforeSwapAsync`: None when the package's SHA-256 matches (ignoring case), the refusal sentence otherwise; Err for a
/// package that cannot be read (C# lets that exception out of `RunAsync`).
fn verify_before_swap(plan: &InstallPlan, locale: &Locale) -> std::io::Result<Option<String>> {
    if !plan.package_path.is_file() {
        return Ok(Some(locale.get("appUpdate.error.packageMissingBeforeSwap")));
    }
    let digest = super::service::sha256(&plan.package_path)?;
    Ok((!digest.eq_ignore_ascii_case(&plan.sha256)).then(|| locale.get("appUpdate.error.packageRecheckFailed")))
}

/// What the swap had done when it failed.
struct Swap {
    moves: Vec<Move>,
    backup_taken: bool,
    installed: bool,
}

/// The two renames (install → backup, copy → install) and, with a relaunch, the new app's start, recorded in `swap` as they happen.
fn swap_in(plan: &InstallPlan, seams: &dyn Seams, new_directory: &Path, swap: &mut Swap) -> std::io::Result<()> {
    let [backup_move, install_move] = planned_moves(plan);
    if plan.install_directory.is_dir() {
        move_with_retry(seams, &plan.install_directory, &plan.backup_directory)?;
        swap.moves.push(backup_move);
        swap.backup_taken = true;
    }
    move_with_retry(seams, new_directory, &plan.install_directory)?;
    swap.moves.push(install_move);
    swap.installed = true;
    if plan.relaunch {
        seams.start_app(&plan.new_executable)?;
    }
    Ok(())
}

/// C# `UndoAsync`: whatever failed, the machine ends with the version the user had. The rollback's rename is tried as every rename is,
/// and nothing here can cancel it.
fn undo(plan: &InstallPlan, seams: &dyn Seams, swap: Swap, reason: &str, locale: &Locale) -> ReplacementResult {
    let Swap { mut moves, backup_taken, installed } = swap;
    if installed && backup_taken {
        remove_folder(&plan.install_directory);
    }
    let mut restored = false;
    if backup_taken && plan.backup_directory.is_dir() {
        let put_back = || -> std::io::Result<()> {
            if plan.install_directory.is_dir() {
                std::fs::remove_dir_all(&plan.install_directory)?;
            }
            move_with_retry(seams, &plan.backup_directory, &plan.install_directory)
        };
        if put_back().is_ok() {
            moves.push(rollback(plan));
            restored = true;
        }
    }
    let started = (restored || (!backup_taken && plan.install_directory.is_dir()))
        && plan.relaunch
        && try_start(seams, &installed_executable(&plan.install_directory));
    ReplacementResult {
        replaced: false,
        rolled_back: restored,
        error: Some(locale.get_with("appUpdate.error.rolledBack", &[("reason", reason)])),
        moves,
        started,
        backup_removed: false,
        skipped: false,
    }
}

/// C# `AppUpdateReplacement.RunAsync`, its sentences in `locale`'s language (the helper's: the OS's). In order: an invalid plan
/// `planInvalid`; an interrupted install put back; the wait for `--wait-pid` (`appStillRunning`); the package's SHA-256 again
/// (`packageMissingBeforeSwap`, `packageRecheckFailed`); the staged folder (`stagedFolderMissing`); the copy beside the install
/// (`installedExecutableMissing`; on failure the copy removed and, when relaunching, the installed app started); with `--relaunch 0`
/// the running-again check (a check that fails counts as running: skipped, its reason the error); the copy compared with the staged
/// tree; the two renames; the start; a failure there undone (`rolledBack{reason}`); the backup deleted once the new app started (or,
/// without a relaunch, once the new folder is in place).
pub fn run(plan: &InstallPlan, seams: &dyn Seams, locale: &Locale) -> ReplacementResult {
    if !plan.is_valid() {
        return refused(locale.get("appUpdate.error.planInvalid"), false);
    }

    restore_interrupted_install(&plan.install_directory);

    if !seams.wait_for_exit(plan) {
        return refused(locale.get("appUpdate.error.appStillRunning"), false);
    }

    // The package is verified again right here, not when it was downloaded.
    match verify_before_swap(plan, locale) {
        Ok(None) => {}
        Ok(Some(refusal)) => return refused(refusal, false),
        Err(error) => return refused(format!("{}: {error}", exception_name(&error)), false),
    }

    if !plan.staged_directory.is_dir() {
        return refused(locale.get("appUpdate.error.stagedFolderMissing"), false);
    }

    // Copy (not move): the helper runs from the staged folder and cannot move the folder it runs from. The copy lands beside the
    // install, so a copy cut off half way never touches the install.
    let new_directory = plan.new_directory();
    remove_folder(&new_directory);
    let copied = match seams.copy(&plan.staged_directory, &new_directory) {
        Err(error) => Err(error.to_string()),
        Ok(()) if !verify_install_tree(&new_directory) => Err(locale.get("appUpdate.error.installedExecutableMissing")),
        Ok(()) => Ok(()),
    };
    if let Err(error) = copied {
        remove_folder(&new_directory);
        let started_old = plan.relaunch && try_start(seams, &installed_executable(&plan.install_directory));
        return refused(error, started_old);
    }

    if !plan.relaunch {
        // The app was opened again: the update waits for the next quit. A check that cannot tell is read the same way, and its reason
        // logged.
        let (again, unsure) = match seams.running_again(plan) {
            Ok(again) => (again, None),
            Err(error) => (true, Some(error.to_string())),
        };
        if again {
            remove_folder(&new_directory);
            return ReplacementResult { error: unsure, skipped: true, ..ReplacementResult::default() };
        }
    }

    // Right before the renames: the copy must be the staged tree, file for file and size for size. A copy cut short or a staged folder
    // changed under it is never swapped in; the staged update is kept. The reason goes to install.log only, so it is plain text.
    let mismatch = tree_mismatch(&plan.staged_directory, &new_directory).unwrap_or_else(|error| Some(error.to_string()));
    if let Some(mismatch) = mismatch {
        remove_folder(&new_directory);
        let started_old = plan.relaunch && try_start(seams, &installed_executable(&plan.install_directory));
        return refused(format!("the copy beside the install does not match the staged update: {mismatch}"), started_old);
    }

    let mut swap = Swap { moves: Vec::new(), backup_taken: false, installed: false };
    if let Err(error) = swap_in(plan, seams, &new_directory, &mut swap) {
        remove_folder(&new_directory);
        return undo(plan, seams, swap, &error.to_string(), locale);
    }

    // The backup is only removed once the new app has started, or, without a relaunch, once the new folder is in place.
    let backup_removed = plan.backup_directory.is_dir() && std::fs::remove_dir_all(&plan.backup_directory).is_ok();
    ReplacementResult {
        replaced: true,
        rolled_back: false,
        error: None,
        moves: swap.moves,
        started: plan.relaunch,
        backup_removed,
        skipped: false,
    }
}
