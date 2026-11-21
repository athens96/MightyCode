//! Unpacking a downloaded package (C# `AppUpdateService.Stage`, AppUpdateService.cs:185-259; plan §1.3, fact 3). Owner: W9-A.
//!
//! The same refusals the 0.4.x stager makes, which also decide whether a 0.4.x app accepts the Rust package: no absolute (`/`, `\`,
//! `X:`), `..`, link (the Unix mode in the high 16 bits of the external attributes is `S_IFLNK`, whatever system made the entry) or
//! escaping entry; exactly one `MightyCode.exe` in the whole tree (the name matched without case, as Windows' file search matches it),
//! at the root or one folder down, whose PE machine is x64 (`0x8664`) or arm64 (`0xAA64`) as `architecture` asks. The answer is the
//! folder holding the exe. Entry names are read as UTF-8 whatever their flag says (.NET's default code page is UTF-8). The 512 MiB cap
//! is the download's ([`super::service::MAX_PACKAGE_BYTES`]). Checked against C#'s own `Stage` by `native/contracts/fixtures/update-stage`.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::UpdateError;
use super::dotnet;

/// The PE machine of an x64 executable.
pub const MACHINE_X64: u16 = 0x8664;

/// The PE machine of an arm64 executable.
pub const MACHINE_ARM64: u16 = 0xAA64;

/// Maximum total uncompressed bytes extracted from an update zip (1 GiB, preventing zip bombs).
pub const MAX_UNCOMPRESSED_BYTES: u64 = 1024 * 1024 * 1024;

/// Maximum number of entries allowed in an update zip.
pub const MAX_ENTRIES: usize = 10_000;

/// Sidecar filename written beside the downloaded package (outside `staged/`) recording the staged tree digest.
pub const STAGED_DIGEST_FILENAME: &str = "staged.tree.sha256";

#[cfg(windows)]
pub(crate) fn is_reparse_point(meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt as _;
    meta.file_attributes() & 0x0400 != 0
}

#[cfg(not(windows))]
pub(crate) fn is_reparse_point(_meta: &std::fs::Metadata) -> bool {
    false
}

/// Path to the staged-tree SHA-256 sidecar file beside `package`.
pub fn staged_digest_path(package: &Path) -> Option<PathBuf> {
    package.parent().map(|parent| parent.join(STAGED_DIGEST_FILENAME))
}

fn sha256_reader<R: Read>(mut reader: R) -> std::io::Result<String> {
    let mut digest = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        match reader.read(&mut buf)? {
            0 => break,
            n => digest.update(&buf[..n]),
        }
    }
    Ok(digest.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

fn collect_staged_files(root: &Path) -> std::io::Result<Option<BTreeMap<String, String>>> {
    let root_meta = std::fs::symlink_metadata(root)?;
    if root_meta.file_type().is_symlink() || is_reparse_point(&root_meta) || !root_meta.is_dir() {
        return Ok(None);
    }
    let mut files = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let path = entry?.path();
            let meta = std::fs::symlink_metadata(&path)?;
            if meta.file_type().is_symlink() || is_reparse_point(&meta) {
                return Ok(None);
            }
            if meta.is_dir() {
                pending.push(path);
            } else if meta.is_file() {
                let relative = path
                    .strip_prefix(root)
                    .map_err(|_| std::io::Error::other("staged file outside root"))?
                    .to_string_lossy()
                    .replace('\\', "/");
                let hash = sha256_reader(File::open(&path)?)?;
                files.insert(relative, hash);
            } else {
                return Ok(None);
            }
        }
    }
    Ok(Some(files))
}

fn digest_of_map(files: &BTreeMap<String, String>) -> String {
    let mut digest = Sha256::new();
    for (relative, file_hash) in files {
        digest.update(relative.as_bytes());
        digest.update(b"\0");
        digest.update(file_hash.as_bytes());
        digest.update(b"\n");
    }
    digest.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Computes a deterministic SHA-256 digest over all regular files under `root`, rejecting any symlink or Windows reparse point.
pub fn staged_tree_digest(root: &Path) -> std::io::Result<String> {
    let files = collect_staged_files(root)?
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "staged directory contains a symlink or non-regular entry"))?;
    Ok(digest_of_map(&files))
}

/// Verifies that `staged` contains no symlinks/reparse points, matches the recorded `staged.tree.sha256` sidecar when present, and
/// (when `package` is a ZIP archive) matches the exact file set and per-file SHA-256 digests of `package`.
pub fn verify_staged_tree(staged: &Path, package: &Path) -> std::io::Result<bool> {
    let Some(actual) = collect_staged_files(staged)? else {
        return Ok(false);
    };
    let actual_digest = digest_of_map(&actual);

    if let Some(sidecar) = staged_digest_path(package)
        && let Ok(meta) = std::fs::symlink_metadata(&sidecar)
    {
        if meta.file_type().is_symlink() || is_reparse_point(&meta) || !meta.is_file() || meta.len() > 256 {
            return Ok(false);
        }
        let mut recorded = String::new();
        File::open(&sidecar)?.take(256).read_to_string(&mut recorded)?;
        if !recorded.trim().eq_ignore_ascii_case(&actual_digest) {
            return Ok(false);
        }
    }

    if let Ok(pkg_meta) = std::fs::symlink_metadata(package)
        && !pkg_meta.file_type().is_symlink()
        && !is_reparse_point(&pkg_meta)
        && pkg_meta.is_file()
        && let Ok(mut zip) = zip::ZipArchive::new(File::open(package)?)
    {
        if zip.len() > MAX_ENTRIES {
            return Ok(false);
        }
        let mut total_bytes = 0u64;
        let mut zip_files = BTreeMap::new();
        for index in 0..zip.len() {
            let entry = zip.by_index(index).map_err(std::io::Error::other)?;
            let name = String::from_utf8_lossy(entry.name_raw()).into_owned();
            if name.is_empty() || name.ends_with('/') {
                continue;
            }
            if name.starts_with(['/', '\\'])
                || name.chars().nth(1) == Some(':')
                || name.split(['/', '\\']).any(|segment| segment == "..")
                || (entry.external_attributes() >> 16) & 0xF000 == 0xA000
            {
                return Ok(false);
            }
            let normalized = name.replace('\\', "/");
            let remaining = MAX_UNCOMPRESSED_BYTES.saturating_sub(total_bytes);
            let mut bounded = entry.take(remaining.saturating_add(1));
            let mut digest = Sha256::new();
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                match bounded.read(&mut buf)? {
                    0 => break,
                    n => {
                        total_bytes = total_bytes.saturating_add(n as u64);
                        if total_bytes > MAX_UNCOMPRESSED_BYTES {
                            return Ok(false);
                        }
                        digest.update(&buf[..n]);
                    }
                }
            }
            let hash: String = digest.finalize().iter().map(|b| format!("{b:02x}")).collect();
            zip_files.insert(normalized, hash);
        }
        // Locate the single MightyCode.exe inside the zip (either at root or one folder down).
        let exes: Vec<&String> = zip_files
            .keys()
            .filter(|k| k.rsplit('/').next().is_some_and(|name| dotnet::eq_ignore_case(name, super::EXECUTABLE_NAME)))
            .collect();
        if exes.len() != 1 {
            return Ok(false);
        }
        let prefix = match exes[0].rsplit_once('/') {
            None => String::new(),
            Some((dir, _)) if !dir.contains('/') => format!("{dir}/"),
            Some(_) => return Ok(false),
        };
        let mut expected = BTreeMap::new();
        for (path, hash) in zip_files {
            if let Some(stripped) = path.strip_prefix(&prefix) {
                expected.insert(stripped.to_string(), hash);
            }
        }
        return Ok(actual == expected);
    }

    Ok(true)
}

fn write_staged_digest(package: &Path, digest: &str) -> std::io::Result<()> {
    let Some(path) = staged_digest_path(package) else { return Ok(()) };
    if let Ok(meta) = std::fs::symlink_metadata(&path)
        && (meta.file_type().is_symlink() || is_reparse_point(&meta))
    {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "staged digest path is a symlink"));
    }
    let temporary = path.with_extension(format!("sha256.{}.tmp", std::process::id()));
    let _ = std::fs::remove_file(&temporary);
    let written = (|| -> std::io::Result<()> {
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&temporary)?;
        file.write_all(digest.as_bytes())?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, &path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    written
}

fn ensure_safe_parent(parent: &Path, root: &Path) -> std::io::Result<()> {
    let relative = parent.strip_prefix(root).map_err(|_| std::io::Error::other("parent outside staging root"))?;
    let mut cursor = root.to_path_buf();
    for component in relative.components() {
        if let std::path::Component::Normal(part) = component {
            cursor.push(part);
            match std::fs::symlink_metadata(&cursor) {
                Ok(meta) => {
                    if meta.file_type().is_symlink() || is_reparse_point(&meta) || !meta.is_dir() {
                        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "symlink in staging directory"));
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    std::fs::create_dir(&cursor)?;
                }
                Err(error) => return Err(error),
            }
        }
    }
    Ok(())
}

fn key(name: &'static str) -> UpdateError {
    UpdateError::key(name)
}

fn io(error: impl std::fmt::Display) -> UpdateError {
    UpdateError::Plain(error.to_string())
}

/// C# `RefuseUnsafeEntry`.
fn refuse_unsafe_entry(name: &str, external_attributes: u32, root: &Path) -> Result<(), UpdateError> {
    if name.starts_with(['/', '\\']) || name.chars().nth(1) == Some(':') {
        return Err(key("appUpdate.error.entryAbsolute"));
    }
    if name.split(['/', '\\']).any(|segment| segment == "..") {
        return Err(key("appUpdate.error.entryParent"));
    }
    // The Unix mode sits in the high 16 bits; S_IFLNK is 0xA000.
    if (external_attributes >> 16) & 0xF000 == 0xA000 {
        return Err(key("appUpdate.error.entryLink"));
    }
    let resolved = dotnet::full_path(&root.join(name.replace('/', &dotnet::SEPARATOR.to_string()))).map_err(io)?;
    let mut inside = root.as_os_str().to_owned();
    inside.push(dotnet::SEPARATOR.to_string());
    if !resolved.as_os_str().as_encoded_bytes().starts_with(inside.as_encoded_bytes()) {
        return Err(key("appUpdate.error.entryOutside"));
    }
    Ok(())
}

/// Every file named `name` (without case) under `root`, as `Directory.GetFiles(root, name, AllDirectories)` finds them.
fn find(root: &Path, name: &str) -> std::io::Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory)? {
            let path = entry?.path();
            let meta = std::fs::symlink_metadata(&path)?;
            if meta.file_type().is_symlink() || is_reparse_point(&meta) {
                return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "symlink in staged directory"));
            }
            if meta.is_dir() {
                pending.push(path);
            } else if dotnet::eq_ignore_case(&dotnet::file_name(&path), name) {
                found.push(path);
            }
        }
    }
    Ok(found)
}

/// C# `RefuseAnotherArchitecture`: the PE header's machine (the offset at 0x3c, then the machine 4 bytes past it) must be the one
/// `expected` names.
fn refuse_another_architecture(executable: &Path, expected: &str) -> Result<(), UpdateError> {
    let damaged = || key("appUpdate.error.executableDamaged");
    let mut file = File::open(executable).map_err(io)?;
    let length = file.metadata().map_err(io)?.len();
    let mut header = [0u8; 4];
    file.seek(SeekFrom::Start(0x3c)).map_err(io)?;
    if read_up_to(&mut file, &mut header).map_err(io)? < 4 {
        return Err(damaged());
    }
    let offset = i32::from_le_bytes(header);
    if offset <= 0 || i64::from(offset) > i64::try_from(length).unwrap_or(i64::MAX) - 6 {
        return Err(damaged());
    }
    file.seek(SeekFrom::Start(offset.unsigned_abs() as u64 + 4)).map_err(io)?;
    let mut machine = [0u8; 2];
    if read_up_to(&mut file, &mut machine).map_err(io)? < 2 {
        return Err(damaged());
    }
    let machine = u16::from_le_bytes(machine);
    let matches = match expected.to_ascii_lowercase().as_str() {
        "x64" => machine == MACHINE_X64,
        "arm64" => machine == MACHINE_ARM64,
        _ => false,
    };
    if !matches {
        return Err(UpdateError::Key {
            key: "appUpdate.error.architecture",
            args: vec![("architecture", expected.to_string()), ("machine", format!("0x{machine:04X}"))],
        });
    }
    Ok(())
}

/// `Stream.Read`'s count for one read into `buffer` (a short read only at the end of the file).
fn read_up_to(file: &mut File, buffer: &mut [u8]) -> std::io::Result<usize> {
    let mut total = 0;
    while total < buffer.len() {
        match file.read(&mut buffer[total..])? {
            0 => break,
            read => total += read,
        }
    }
    Ok(total)
}

/// C# `AppUpdateService.Stage(package, architecture)`, unpacking into `destination` (C#: `staged` beside the package), which is
/// deleted first and made afresh. Rejects symlinks/reparse points at `destination`, caps zip entry count ([`MAX_ENTRIES`]) and total
/// uncompressed bytes ([`MAX_UNCOMPRESSED_BYTES`]), and records a staged-tree SHA-256 sidecar beside `package`. A package refused half
/// way leaves what was unpacked, as C#'s does.
pub fn stage(package: &Path, destination: &Path, architecture: &str) -> Result<PathBuf, UpdateError> {
    if let Ok(meta) = std::fs::symlink_metadata(destination) {
        if meta.file_type().is_symlink() || is_reparse_point(&meta) {
            return Err(key("appUpdate.error.entryLink"));
        }
        if meta.is_dir() {
            std::fs::remove_dir_all(destination).map_err(io)?;
        } else {
            return Err(io("staging destination is not a directory"));
        }
    }
    std::fs::create_dir_all(destination).map_err(io)?;
    let root_meta = std::fs::symlink_metadata(destination).map_err(io)?;
    if root_meta.file_type().is_symlink() || is_reparse_point(&root_meta) || !root_meta.is_dir() {
        return Err(key("appUpdate.error.entryLink"));
    }
    let root = dotnet::trim_separator(&dotnet::full_path(destination).map_err(io)?);

    let mut zip = zip::ZipArchive::new(File::open(package).map_err(io)?).map_err(io)?;
    if zip.len() > MAX_ENTRIES {
        return Err(key("appUpdate.error.packageTooLarge"));
    }
    let mut total_bytes = 0u64;
    for index in 0..zip.len() {
        let entry = zip.by_index(index).map_err(io)?;
        let name = String::from_utf8_lossy(entry.name_raw()).into_owned();
        if name.is_empty() {
            continue;
        }
        refuse_unsafe_entry(&name, entry.external_attributes(), &root)?;
        if name.ends_with('/') {
            continue;
        }
        let remaining = MAX_UNCOMPRESSED_BYTES.saturating_sub(total_bytes);
        if entry.size() > remaining {
            return Err(key("appUpdate.error.packageTooLarge"));
        }
        let target = root.join(name.replace('/', &dotnet::SEPARATOR.to_string()));
        if let Some(parent) = target.parent() {
            ensure_safe_parent(parent, &root).map_err(io)?;
        }
        if let Ok(existing) = std::fs::symlink_metadata(&target)
            && (existing.file_type().is_symlink() || is_reparse_point(&existing))
        {
            return Err(key("appUpdate.error.entryLink"));
        }
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&target).map_err(io)?;
        let mut bounded = entry.take(remaining.saturating_add(1));
        let copied = std::io::copy(&mut bounded, &mut file).map_err(io)?;
        total_bytes = total_bytes.saturating_add(copied);
        if total_bytes > MAX_UNCOMPRESSED_BYTES {
            drop(file);
            let _ = std::fs::remove_file(&target);
            return Err(key("appUpdate.error.packageTooLarge"));
        }
    }

    // The package shape: exactly one MightyCode.exe, at the expected place.
    let executables = find(&root, super::EXECUTABLE_NAME).map_err(io)?;
    if executables.len() != 1 {
        return Err(UpdateError::Key {
            key: "appUpdate.error.executableCount",
            args: vec![("name", super::EXECUTABLE_NAME.to_string()), ("count", executables.len().to_string())],
        });
    }
    let app_folder = executables[0].parent().map(Path::to_path_buf).unwrap_or_default();
    if app_folder != root && app_folder.parent() != Some(root.as_path()) {
        return Err(UpdateError::Key {
            key: "appUpdate.error.executableLocation",
            args: vec![("name", super::EXECUTABLE_NAME.to_string())],
        });
    }
    refuse_another_architecture(&executables[0], architecture)?;
    let digest = staged_tree_digest(&app_folder).map_err(io)?;
    write_staged_digest(package, &digest).map_err(io)?;
    Ok(app_folder)
}
