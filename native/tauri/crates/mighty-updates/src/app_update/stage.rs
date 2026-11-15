//! Unpacking a downloaded package (C# `AppUpdateService.Stage`, AppUpdateService.cs:185-259; plan §1.3, fact 3). Owner: W9-A.
//!
//! The same refusals the 0.4.x stager makes, which also decide whether a 0.4.x app accepts the Rust package: no absolute (`/`, `\`,
//! `X:`), `..`, link (the Unix mode in the high 16 bits of the external attributes is `S_IFLNK`, whatever system made the entry) or
//! escaping entry; exactly one `MightyCode.exe` in the whole tree (the name matched without case, as Windows' file search matches it),
//! at the root or one folder down, whose PE machine is x64 (`0x8664`) or arm64 (`0xAA64`) as `architecture` asks. The answer is the
//! folder holding the exe. Entry names are read as UTF-8 whatever their flag says (.NET's default code page is UTF-8). The 512 MiB cap
//! is the download's ([`super::service::MAX_PACKAGE_BYTES`]). Checked against C#'s own `Stage` by `native/contracts/fixtures/update-stage`.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use super::UpdateError;
use super::dotnet;

/// The PE machine of an x64 executable.
pub const MACHINE_X64: u16 = 0x8664;

/// The PE machine of an arm64 executable.
pub const MACHINE_ARM64: u16 = 0xAA64;

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
            if path.is_dir() {
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
/// deleted first and made afresh. A package refused half way leaves what was unpacked, as C#'s does.
pub fn stage(package: &Path, destination: &Path, architecture: &str) -> Result<PathBuf, UpdateError> {
    if destination.is_dir() {
        std::fs::remove_dir_all(destination).map_err(io)?;
    }
    std::fs::create_dir_all(destination).map_err(io)?;
    let root = dotnet::trim_separator(&dotnet::full_path(destination).map_err(io)?);

    let mut zip = zip::ZipArchive::new(File::open(package).map_err(io)?).map_err(io)?;
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index).map_err(io)?;
        let name = String::from_utf8_lossy(entry.name_raw()).into_owned();
        if name.is_empty() {
            continue;
        }
        refuse_unsafe_entry(&name, entry.external_attributes(), &root)?;
        if name.ends_with('/') {
            continue;
        }
        let target = root.join(name.replace('/', &dotnet::SEPARATOR.to_string()));
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(io)?;
        }
        let mut file = File::create(&target).map_err(io)?;
        std::io::copy(&mut entry, &mut file).map_err(io)?;
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
    Ok(app_folder)
}
