//! The folders around the install (C# `AppUpdateReplacement.RestoreInterruptedInstall`, `PruneBackups`, `StagingBusy`, `TreeMismatch`,
//! `VerifyInstallTree`, `InstalledExecutable`; plan §1.1 "App-start housekeeping", §1.3 `StagingBusy`). Owner: W9-A.
//!
//! Run by the helper (restore, tree mismatch) and at app start outside a smoke where the app may change the machine (W9-C: restore,
//! prune, then `SyncDisplayVersion`). A process check C# makes itself (`AnyRunningFrom`) is passed in here: it is the platform's
//! (`UpdateInstaller::running_from`, W9-B).

use std::collections::{BTreeMap, VecDeque};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use chrono::{DateTime, Utc};

use super::dotnet;

/// A `.update-new` copy written this recently may still be filling (C# `NewCopyGrace`): pruning and staging leave it alone.
pub const NEW_COPY_GRACE: Duration = Duration::from_secs(10 * 60);

/// C# `VerifyInstallTree`: the folder holds `MightyCode.exe` or `MightyClaude.exe`.
pub fn verify_install_tree(install_directory: &Path) -> bool {
    super::INSTALLED_EXECUTABLE_NAMES.iter().any(|name| install_directory.join(name).is_file())
}

/// C# `InstalledExecutable`: the exe the install folder holds (this version's name first), this version's name when neither is there.
pub fn installed_executable(install_directory: &Path) -> PathBuf {
    super::INSTALLED_EXECUTABLE_NAMES
        .iter()
        .map(|name| install_directory.join(name))
        .find(|path| path.is_file())
        .unwrap_or_else(|| install_directory.join(super::EXECUTABLE_NAME))
}

/// `Path.GetFullPath(install.TrimEnd(separator))`.
fn full_install(install_directory: &Path) -> io::Result<PathBuf> {
    dotnet::full_path(&dotnet::trim_separator(install_directory))
}

/// The folders directly in `parent` whose names start with `prefix` (ordinal), as `Directory.GetDirectories` lists them (a link to a
/// folder counts as one).
fn folders_starting(parent: &Path, prefix: &str) -> io::Result<Vec<(String, PathBuf)>> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(parent)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with(prefix) && entry.path().is_dir() {
            found.push((name, entry.path()));
        }
    }
    Ok(found)
}

/// A folder moved as .NET's `Directory.Move` moves it: refused when anything is already at `to` (a plain rename may replace an empty
/// folder there).
pub fn move_directory(from: &Path, to: &Path) -> io::Result<()> {
    if std::fs::symlink_metadata(to).is_ok() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("Cannot create '{}' because a file or directory with the same name already exists.", to.display()),
        ));
    }
    std::fs::rename(from, to)
}

/// C# `RemoveFolder`: the folder deleted when it is there, a failure ignored.
pub fn remove_folder(path: &Path) {
    if path.is_dir() {
        let _ = std::fs::remove_dir_all(path);
    }
}

/// C# `RestoreInterruptedInstall`: when the install is missing or holds no exe, the newest whole `<name>.backup-*` (ordinal
/// descending) is moved back (a half install deleted first). True when one was put back; never fails.
pub fn restore_interrupted_install(install_directory: &Path) -> bool {
    let restore = || -> io::Result<bool> {
        let install = full_install(install_directory)?;
        if install.is_dir() && verify_install_tree(&install) {
            return Ok(false);
        }
        let Some(parent) = install.parent().filter(|parent| parent.is_dir()) else { return Ok(false) };
        let prefix = format!("{}.backup-", dotnet::file_name(&install));
        let newest = folders_starting(parent, &prefix)?
            .into_iter()
            .filter(|(_, folder)| verify_install_tree(folder))
            .max_by(|(a, _), (b, _)| dotnet::compare_ordinal(a, b));
        let Some((_, backup)) = newest else { return Ok(false) };
        if install.is_dir() {
            std::fs::remove_dir_all(&install)?;
        }
        move_directory(&backup, &install)?;
        Ok(true)
    };
    restore().unwrap_or(false)
}

/// C# `WrittenSince`: whether the folder, or anything in it, was created or written after `since` (copied files keep their source's
/// write time, so creation times count too). A folder that is not there was not; one that cannot be read answers `when_unreadable`.
fn written_since(folder: &Path, since: SystemTime, when_unreadable: bool) -> bool {
    if !folder.is_dir() {
        return false;
    }
    let latest = |metadata: &std::fs::Metadata| -> io::Result<SystemTime> {
        let written = metadata.modified()?;
        Ok(metadata.created().map_or(written, |created| created.max(written)))
    };
    let walk = || -> io::Result<bool> {
        if latest(&std::fs::metadata(folder)?)? > since {
            return Ok(true);
        }
        let mut pending = VecDeque::from([folder.to_path_buf()]);
        while let Some(directory) = pending.pop_front() {
            for entry in std::fs::read_dir(&directory)? {
                let entry = entry?;
                let metadata = std::fs::symlink_metadata(entry.path())?;
                if latest(&metadata)? > since {
                    return Ok(true);
                }
                if metadata.is_dir() {
                    pending.push_back(entry.path());
                }
            }
        }
        Ok(false)
    };
    walk().unwrap_or(when_unreadable)
}

fn grace_start(now: DateTime<Utc>) -> SystemTime {
    SystemTime::from(now) - NEW_COPY_GRACE
}

/// C# `PruneBackups`: `<name>.update-new` deleted unless written within [`NEW_COPY_GRACE`] of `now` (an unreadable one kept), then
/// every `<name>.backup-*`; how many backups went.
pub fn prune_backups(install_directory: &Path, now: DateTime<Utc>) -> usize {
    let Ok(install) = full_install(install_directory) else { return 0 };
    let Some(parent) = install.parent().filter(|parent| parent.is_dir()) else { return 0 };
    let prefix = format!("{}.backup-", dotnet::file_name(&install));
    let copy = dotnet::with_suffix(&install, ".update-new");
    if !written_since(&copy, grace_start(now), true) {
        remove_folder(&copy);
    }
    let Ok(backups) = folders_starting(parent, &prefix) else { return 0 };
    backups.iter().filter(|(_, folder)| std::fs::remove_dir_all(folder).is_ok()).count()
}

/// C# `StagingBusy`: a process other than this one runs from under `updates_directory` (`running_from`; an error reads as busy), or
/// `<install>.update-new` was written within [`NEW_COPY_GRACE`] (an unreadable copy is not busy: it would hold staging forever, and
/// the helper's own [`tree_mismatch`] refuses a copy that is not the staged tree).
pub fn staging_busy(
    updates_directory: &Path,
    install_directory: Option<&Path>,
    now: DateTime<Utc>,
    running_from: &dyn Fn(&Path) -> io::Result<bool>,
) -> bool {
    if updates_directory.is_dir() && running_from(updates_directory).unwrap_or(true) {
        return true;
    }
    match install_directory.map(full_install) {
        None => false,
        Some(Ok(install)) => written_since(&dotnet::with_suffix(&install, ".update-new"), grace_start(now), false),
        Some(Err(_)) => true,
    }
}

/// Every file under `root` (relative path → size), folders listed one level after another as .NET enumerates them.
fn files(root: &Path) -> io::Result<Vec<(String, u64)>> {
    let mut found = Vec::new();
    let mut pending = VecDeque::from([root.to_path_buf()]);
    while let Some(directory) = pending.pop_front() {
        let mut folders = Vec::new();
        for entry in std::fs::read_dir(&directory)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                folders.push(path);
            } else {
                let relative = path.strip_prefix(root).unwrap_or(&path).to_string_lossy().into_owned();
                found.push((relative, std::fs::metadata(&path)?.len()));
            }
        }
        pending.extend(folders);
    }
    Ok(found)
}

/// C# `TreeMismatch(expected, actual)`: why the copy is not the staged tree (`missing <file>`, `unexpected <file>`, `size differs for
/// <file>`; relative paths compared ordinal), or None.
pub fn tree_mismatch(expected: &Path, actual: &Path) -> io::Result<Option<String>> {
    let want = files(expected)?;
    let have_listed = files(actual)?;
    let wanted: BTreeMap<&str, u64> = want.iter().map(|(file, size)| (file.as_str(), *size)).collect();
    let have: BTreeMap<&str, u64> = have_listed.iter().map(|(file, size)| (file.as_str(), *size)).collect();
    if let Some((missing, _)) = want.iter().find(|(file, _)| !have.contains_key(file.as_str())) {
        return Ok(Some(format!("missing {missing}")));
    }
    if let Some((extra, _)) = have_listed.iter().find(|(file, _)| !wanted.contains_key(file.as_str())) {
        return Ok(Some(format!("unexpected {extra}")));
    }
    Ok(want.iter().find(|(file, size)| have.get(file.as_str()) != Some(size)).map(|(file, _)| format!("size differs for {file}")))
}
