//! The update's machine half on Windows (W9-B; C# `AppUpdateHelperGuard`'s mutex, `AppUpdateReplacement.WaitForExitAsync`,
//! `AnyRunningFrom`, `UserEnvironment.Fresh`, `InstallerRegistration.SyncDisplayVersion`, `WinUI\RenamedInstall.cs`; plan §W9-B):
//! - `helper_mutex`: `CreateMutexW` + `WaitForSingleObject(0)` (`WAIT_ABANDONED` counts as taken), released and closed when dropped;
//! - `wait_for_exit`: `OpenProcess(SYNCHRONIZE)` + `WaitForSingleObject`, a missing pid exited;
//! - `running_from`: `CreateToolhelp32Snapshot` for `MightyCode.exe` / `MightyClaude.exe`, `QueryFullProcessImageNameW`, the prefix
//!   compared without case, this process left out, one that cannot be opened (or has exited) skipped;
//! - `fresh_environment`: `OpenProcessToken(TOKEN_QUERY)` + `CreateEnvironmentBlock(inherit = FALSE)` ([`crate::user_environment`]);
//! - `sync_display_version`: HKCU's uninstall entry, written only when it names the folder and shows another version;
//! - `refresh_renamed_install`: `IShellLinkW` / `IPersistFile` over the Programs and Desktop shortcuts, the entry's icon and name.
//!
//! The hand-off itself (the helper's start) is the app's (`src-tauri` `app_update.rs`): it builds the plan and the helper's arguments
//! with mighty-updates' `app_update::plan` and starts the staged exe through `ProcessHost::start_detached`; this crate does not know the
//! plan, so [`WindowsUpdates::hand_off`] answers `Unsupported`.

use std::collections::BTreeMap;
use std::ffi::c_void;
use std::os::windows::ffi::OsStringExt;
use std::os::windows::io::OwnedHandle;
use std::path::{Path, PathBuf};
use std::time::Duration;

use mighty_platform::{HelperMutex, PlatformError, Result, StagedUpdate, UpdateInstaller};
use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_INVALID_PARAMETER, WAIT_ABANDONED, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::System::Threading::{
    CreateMutexW, GetCurrentProcessId, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, ReleaseMutex,
    WaitForSingleObject,
};

use crate::win::{equal_ignore_case, last_error, owned, raw, starts_with_ignore_case, wide};

/// The executables an install folder may hold (C# `AppUpdateService.InstalledExecutableNames`: this version's, then the one from
/// before the rename).
pub const INSTALLED_EXECUTABLE_NAMES: [&str; 2] = ["MightyCode.exe", "MightyClaude.exe"];

/// The setup's uninstall entry under HKCU (C# `InstallerRegistration.UninstallKey`; mighty-updates' `installer_entry::UNINSTALL_KEY`,
/// MightyCode.iss's AppId with Inno Setup's suffix).
pub const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\{F2F99AEF-FB35-4476-A3EE-49453FA0527F}_is1";

/// C# `RenamedInstall`'s names.
const OLD_EXECUTABLE: &str = "MightyClaude.exe";
const NEW_EXECUTABLE: &str = "MightyCode.exe";
const DISPLAY_NAME: &str = "Mighty Code";
const SHORTCUTS: [&str; 2] = ["Mighty Claude.lnk", "Mighty Code.lnk"];

/// The Windows update installer: the helper's seams and the start's housekeeping.
#[derive(Debug, Clone, Copy, Default)]
pub struct WindowsUpdates;

/// The helper's mutex while held: released (by the thread that took it, as Windows wants) and closed when dropped.
struct HeldMutex(OwnedHandle);

impl Drop for HeldMutex {
    fn drop(&mut self) {
        // SAFETY: a mutex this process owns; a release from another thread fails harmlessly and the close below abandons it, which the
        // next taker counts as taken.
        unsafe { ReleaseMutex(raw(&self.0)) };
    }
}

impl UpdateInstaller for WindowsUpdates {
    /// The folder this exe runs from (C# `AppContext.BaseDirectory`, which the coordinator gets as the install folder).
    fn install_location(&self) -> Option<PathBuf> {
        std::env::current_exe().ok()?.parent().map(Path::to_path_buf)
    }

    fn hand_off(&self, _update: &StagedUpdate, _relaunch: bool) -> Result<()> {
        Err(PlatformError::unsupported("UpdateInstaller", "hand_off"))
    }

    /// C# `new Mutex(false, name)` + `WaitOne(0)`, an `AbandonedMutexException` read as taken.
    fn helper_mutex(&self, name: &str) -> Result<Option<HelperMutex>> {
        if name.is_empty() || name.contains('\0') {
            return Err(PlatformError::Invalid("a mutex needs a name".into()));
        }
        let name = wide(name);
        // SAFETY: default security, not owned on creation, a NUL-terminated name.
        let mutex = owned(unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) })?;
        // SAFETY: a mutex handle, no wait.
        match unsafe { WaitForSingleObject(raw(&mutex), 0) } {
            WAIT_OBJECT_0 | WAIT_ABANDONED => Ok(Some(HelperMutex::new(HeldMutex(mutex)))),
            WAIT_TIMEOUT => Ok(None),
            _ => Err(last_error()),
        }
    }

    /// C# `WaitForExitAsync(pid, timeout)` without the 200 ms polling: the process's own handle waited on, so a reused id is never
    /// mistaken for the app once it was opened. An id no process has is an exited app.
    fn wait_for_exit(&self, pid: u32, timeout: Duration) -> Result<bool> {
        // SAFETY: a wait-and-query open of a process id.
        let process = unsafe { OpenProcess(PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if process.is_null() {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(ERROR_INVALID_PARAMETER as i32) {
                return Ok(true);
            }
            return Err(PlatformError::Io(error));
        }
        let process = owned(process)?;
        let milliseconds = timeout.as_millis().min(u128::from(u32::MAX - 1)) as u32;
        // SAFETY: a process handle opened for SYNCHRONIZE.
        match unsafe { WaitForSingleObject(raw(&process), milliseconds) } {
            WAIT_OBJECT_0 => Ok(true),
            WAIT_TIMEOUT => Ok(false),
            _ => Err(last_error()),
        }
    }

    fn running_from(&self, folder: &Path, except_self: bool) -> Result<bool> {
        running_from(folder, except_self)
    }

    fn fresh_environment(&self) -> Option<BTreeMap<String, String>> {
        crate::user_environment::fresh().map(crate::Variables::into_map)
    }

    fn sync_display_version(&self, app_directory: &Path, version: &str) -> Result<bool> {
        match RegistryEntry::open(UNINSTALL_KEY)? {
            Some(mut entry) => sync_display_version_in(&mut entry, app_directory, version),
            None => Ok(false),
        }
    }

    /// C# `RenamedInstall.Refresh`: best effort, a shortcut or the entry that cannot be changed left as it is.
    fn refresh_renamed_install(&self, app_directory: &Path) -> Result<()> {
        let folders: Vec<PathBuf> =
            [known_folder(&windows::Win32::UI::Shell::FOLDERID_Programs), known_folder(&windows::Win32::UI::Shell::FOLDERID_Desktop)]
                .into_iter()
                .flatten()
                .collect();
        if !refresh_shortcuts_in(&folders, app_directory) {
            return Ok(());
        }
        let install = trim_separators(&app_directory.to_string_lossy()).to_string();
        if let Some(mut entry) = RegistryEntry::open(UNINSTALL_KEY)? {
            rename_uninstall_entry_in(&mut entry, &install, &format!("{install}\\{NEW_EXECUTABLE}"))?;
        }
        Ok(())
    }
}

/// `path.TrimEnd('\\', '/')`.
fn trim_separators(path: &str) -> &str {
    path.trim_end_matches(['\\', '/'])
}

/// C# `Path.GetFullPath(folder.TrimEnd('\\')) + '\\'`, the 8.3 parts written out long where the folder exists (a process's image path
/// is its long name: CI's `%TEMP%` is `C:\Users\RUNNER~1`).
fn folder_prefix(folder: &Path) -> String {
    let trimmed = PathBuf::from(trim_separators(&folder.to_string_lossy()));
    let full = std::path::absolute(&trimmed).unwrap_or(trimmed);
    let long = long_path(&full).unwrap_or(full);
    format!("{}\\", trim_separators(&long.to_string_lossy()))
}

/// `GetLongPathNameW`: None when the path does not exist or Windows will not say.
fn long_path(path: &Path) -> Option<PathBuf> {
    use windows_sys::Win32::Storage::FileSystem::GetLongPathNameW;
    let short = wide(path);
    let mut buffer = vec![0u16; 512];
    loop {
        // SAFETY: a NUL-terminated path; `buffer` holds `buffer.len()` writable units.
        let length = unsafe { GetLongPathNameW(short.as_ptr(), buffer.as_mut_ptr(), buffer.len() as u32) } as usize;
        if length == 0 {
            return None;
        }
        if length < buffer.len() {
            return Some(PathBuf::from(std::ffi::OsString::from_wide(&buffer[..length])));
        }
        buffer.resize(length + 1, 0);
    }
}

/// C# `AnyRunningFrom(folder)`: a running process named `MightyCode.exe` or `MightyClaude.exe` (other than this one when
/// `except_self`) whose image lies under `folder`, compared without case. A process that cannot be opened or has exited is skipped (C#
/// skips its `Win32Exception`); a process list Windows will not give is an error, which the callers read as "running".
pub fn running_from(folder: &Path, except_self: bool) -> Result<bool> {
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
    };
    let prefix = folder_prefix(folder);
    // SAFETY: CreateToolhelp32Snapshot of every process (the id is ignored for TH32CS_SNAPPROCESS).
    let snapshot = owned(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) })?;
    // SAFETY: plain call.
    let own = unsafe { GetCurrentProcessId() };
    // SAFETY: a zeroed plain-data struct whose size is set before use.
    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
    // SAFETY: a snapshot handle and an entry with its size set.
    let mut more = unsafe { Process32FirstW(raw(&snapshot), &mut entry) } != 0;
    while more {
        let length = entry.szExeFile.iter().position(|unit| *unit == 0).unwrap_or(entry.szExeFile.len());
        let name = String::from_utf16_lossy(&entry.szExeFile[..length]);
        let ours = INSTALLED_EXECUTABLE_NAMES.iter().any(|known| equal_ignore_case(known, &name));
        if ours
            && !(except_self && entry.th32ProcessID == own)
            && image(entry.th32ProcessID).is_some_and(|image| starts_with_ignore_case(&image, &prefix))
        {
            return Ok(true);
        }
        // SAFETY: as above.
        more = unsafe { Process32NextW(raw(&snapshot), &mut entry) } != 0;
    }
    Ok(false)
}

/// A running process's image path (`QueryFullProcessImageNameW`), None when it cannot be opened or has exited.
fn image(pid: u32) -> Option<String> {
    use windows_sys::Win32::Foundation::STILL_ACTIVE;
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, PROCESS_NAME_WIN32, QueryFullProcessImageNameW};
    // SAFETY: a query-only open of a process id.
    let process = owned(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) }).ok()?;
    let mut code = 0u32;
    // SAFETY: a process handle opened for query.
    if unsafe { GetExitCodeProcess(raw(&process), &mut code) } == 0 || code != STILL_ACTIVE as u32 {
        return None;
    }
    let mut buffer = vec![0u16; 1024];
    loop {
        let mut length = buffer.len() as u32;
        // SAFETY: `buffer` holds `length` writable units; `length` receives the units written.
        if unsafe { QueryFullProcessImageNameW(raw(&process), PROCESS_NAME_WIN32, buffer.as_mut_ptr(), &mut length) } != 0 {
            return Some(String::from_utf16_lossy(&buffer[..length as usize]));
        }
        if buffer.len() >= 32 * 1024 {
            return None;
        }
        buffer.resize(buffer.len() * 4, 0);
    }
}

/// The values of one registry entry (the uninstall key's, or a test's in memory: no test writes HKCU).
pub trait EntryValues {
    /// A text value (REG_SZ, or REG_EXPAND_SZ expanded, as .NET's `GetValue` reads it), None when it is missing or not text.
    fn text(&self, name: &str) -> Result<Option<String>>;
    /// Writes a REG_SZ value.
    fn set_text(&mut self, name: &str, value: &str) -> Result<()>;
}

/// C# `InstallerRegistration.VersionToWrite` (the same decision as mighty-updates' `installer_entry::version_to_write`, which the shared
/// tests check; src-tauri's tests hold the two to one table): the version to write, or None (no entry, an entry for another folder —
/// trimmed and without trailing `\` or `/`, compared without case — or the same version).
pub fn version_to_write(
    install_location: Option<&str>,
    display_version: Option<&str>,
    app_directory: &str,
    version: &str,
) -> Option<String> {
    let location = install_location.filter(|location| !location.trim().is_empty())?;
    if version.trim().is_empty() {
        return None;
    }
    let folder = |path: &str| path.trim().trim_end_matches(['\\', '/']).to_string();
    if !equal_ignore_case(&folder(location), &folder(app_directory)) {
        return None;
    }
    (display_version != Some(version)).then(|| version.to_string())
}

/// C# `InstallerRegistration.SyncDisplayVersion` over `entry`: `DisplayVersion` brought to `version` when the entry names
/// `app_directory`; true when it wrote one.
pub fn sync_display_version_in(entry: &mut dyn EntryValues, app_directory: &Path, version: &str) -> Result<bool> {
    let location = entry.text("InstallLocation")?;
    let shown = entry.text("DisplayVersion")?;
    let Some(write) = version_to_write(location.as_deref(), shown.as_deref(), &app_directory.to_string_lossy(), version) else {
        return Ok(false);
    };
    entry.set_text("DisplayVersion", &write)?;
    Ok(true)
}

/// C# `RenamedInstall.UpdateUninstallEntry` over `entry`: when it is for `install` (its `InstallLocation` without a trailing separator,
/// compared without case) and its `DisplayIcon` still names `MightyClaude.exe`, the icon becomes `new_exe` and the name `Mighty Code`.
/// True when it changed them.
pub fn rename_uninstall_entry_in(entry: &mut dyn EntryValues, install: &str, new_exe: &str) -> Result<bool> {
    let location = entry.text("InstallLocation")?;
    if !location.is_some_and(|location| equal_ignore_case(trim_separators(&location), install)) {
        return Ok(false);
    }
    let names_old = entry.text("DisplayIcon")?.is_some_and(|icon| crate::win::fold(&icon).contains(&crate::win::fold(OLD_EXECUTABLE)));
    if !names_old {
        return Ok(false);
    }
    entry.set_text("DisplayIcon", new_exe)?;
    entry.set_text("DisplayName", DISPLAY_NAME)?;
    Ok(true)
}

/// A key under HKEY_CURRENT_USER, read with `RegGetValueW` and written only through a handle opened for the write (no key is ever
/// created).
struct RegistryEntry {
    key: Vec<u16>,
}

impl RegistryEntry {
    /// None when the key does not exist.
    fn open(path: &str) -> Result<Option<RegistryEntry>> {
        use windows_sys::Win32::System::Registry::{HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, RegCloseKey, RegOpenKeyExW};
        let key = wide(path);
        let mut handle: HKEY = std::ptr::null_mut();
        // SAFETY: a NUL-terminated subkey; `handle` receives a key handle, closed at once.
        let status = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, key.as_ptr(), 0, KEY_QUERY_VALUE, &mut handle) };
        if status == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        if status != 0 {
            return Err(PlatformError::Io(std::io::Error::from_raw_os_error(status as i32)));
        }
        // SAFETY: the handle just opened.
        unsafe { RegCloseKey(handle) };
        Ok(Some(RegistryEntry { key }))
    }
}

impl EntryValues for RegistryEntry {
    fn text(&self, name: &str) -> Result<Option<String>> {
        use windows_sys::Win32::Foundation::ERROR_MORE_DATA;
        use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ, RegGetValueW};
        let value = wide(name);
        let mut buffer = vec![0u16; 260];
        loop {
            let mut size = (buffer.len() * 2) as u32;
            // SAFETY: NUL-terminated strings; `buffer` holds `size` writable bytes. A REG_EXPAND_SZ is expanded (RRF's default).
            let status = unsafe {
                RegGetValueW(
                    HKEY_CURRENT_USER,
                    self.key.as_ptr(),
                    value.as_ptr(),
                    RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ,
                    std::ptr::null_mut(),
                    buffer.as_mut_ptr().cast::<c_void>(),
                    &mut size,
                )
            };
            match status {
                0 => {
                    let units = (size as usize / 2).min(buffer.len());
                    let end = buffer[..units].iter().position(|unit| *unit == 0).unwrap_or(units);
                    return Ok(Some(String::from_utf16_lossy(&buffer[..end])));
                }
                ERROR_MORE_DATA => buffer.resize(size as usize / 2 + 1, 0),
                // Missing, or not text (.NET's `as string` is null then).
                ERROR_FILE_NOT_FOUND | windows_sys::Win32::Foundation::ERROR_UNSUPPORTED_TYPE => return Ok(None),
                error => return Err(PlatformError::Io(std::io::Error::from_raw_os_error(error as i32))),
            }
        }
    }

    fn set_text(&mut self, name: &str, value: &str) -> Result<()> {
        use windows_sys::Win32::System::Registry::{
            HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_SZ, RegCloseKey, RegOpenKeyExW, RegSetValueExW,
        };
        let mut handle: HKEY = std::ptr::null_mut();
        // SAFETY: a NUL-terminated subkey; `handle` receives a key handle opened for the write, closed below.
        let status = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, self.key.as_ptr(), 0, KEY_SET_VALUE, &mut handle) };
        if status != 0 {
            return Err(PlatformError::Io(std::io::Error::from_raw_os_error(status as i32)));
        }
        let (name, data) = (wide(name), wide(value));
        // SAFETY: an open key; the data is `data.len() * 2` readable bytes, its NUL included, as REG_SZ wants.
        let status = unsafe { RegSetValueExW(handle, name.as_ptr(), 0, REG_SZ, data.as_ptr().cast(), (data.len() * 2) as u32) };
        // SAFETY: the handle opened above.
        unsafe { RegCloseKey(handle) };
        if status != 0 {
            return Err(PlatformError::Io(std::io::Error::from_raw_os_error(status as i32)));
        }
        Ok(())
    }
}

/// A known folder's path (C# `Environment.GetFolderPath`), None when Windows has none.
fn known_folder(id: &windows::core::GUID) -> Option<PathBuf> {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{KF_FLAG_DEFAULT, SHGetKnownFolderPath};
    // SAFETY: a known folder id; the returned string is freed below.
    let path = unsafe { SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None) }.ok()?;
    // SAFETY: a NUL-terminated string Windows allocated.
    let text = unsafe { path.to_string() }.ok();
    // SAFETY: the string SHGetKnownFolderPath allocated, freed once.
    unsafe { CoTaskMemFree(Some(path.0 as *const c_void)) };
    text.map(PathBuf::from)
}

/// The COM apartment for the shortcuts, left again when it was entered here.
struct Apartment(bool);

impl Apartment {
    fn enter() -> Apartment {
        use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx};
        // SAFETY: this thread's apartment; a thread already in another apartment stays as it is (the call fails, nothing to undo).
        Apartment(unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) }.is_ok())
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        if self.0 {
            // SAFETY: paired with the CoInitializeEx that succeeded on this thread.
            unsafe { windows::Win32::System::Com::CoUninitialize() };
        }
    }
}

/// C# `RenamedInstall.Refresh`'s shortcuts over `folders` (the app passes Programs and Desktop; tests their own folders): in each,
/// `Mighty Claude.lnk` and `Mighty Code.lnk` that start `install\MightyClaude.exe` become `Mighty Code.lnk` starting
/// `install\MightyCode.exe` (working folder the install, its icon, the description `Mighty Code`), the old one deleted when its name
/// differs. Nothing happens when `install\MightyCode.exe` does not exist (answers false); a shortcut that cannot be read or saved is
/// left as it is.
pub fn refresh_shortcuts_in(folders: &[PathBuf], install_directory: &Path) -> bool {
    let install = trim_separators(&install_directory.to_string_lossy()).to_string();
    let (old_exe, new_exe) = (format!("{install}\\{OLD_EXECUTABLE}"), format!("{install}\\{NEW_EXECUTABLE}"));
    if !Path::new(&new_exe).is_file() {
        return false;
    }
    let _apartment = Apartment::enter();
    for folder in folders {
        for name in SHORTCUTS {
            let existing = folder.join(name);
            let target = folder.join(format!("{DISPLAY_NAME}.lnk"));
            let _ = recreate(&existing, &target, &old_exe, &new_exe, &install);
        }
    }
    true
}

/// C# `RenamedInstall.Recreate`.
fn recreate(existing: &Path, target: &Path, old_exe: &str, new_exe: &str, install: &str) -> windows::core::Result<()> {
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistFile};
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
    use windows::core::{HSTRING, Interface};
    if !existing.is_file() || !shortcut_target(existing).is_some_and(|path| equal_ignore_case(&path, old_exe)) {
        return Ok(());
    }
    // SAFETY: COM calls on a ShellLink object this function created, in this thread's apartment.
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        link.SetPath(&HSTRING::from(new_exe))?;
        link.SetWorkingDirectory(&HSTRING::from(install))?;
        link.SetIconLocation(&HSTRING::from(new_exe), 0)?;
        link.SetDescription(&HSTRING::from(DISPLAY_NAME))?;
        link.cast::<IPersistFile>()?.Save(&HSTRING::from(target.as_os_str()), false)?;
    }
    if !equal_ignore_case(&existing.to_string_lossy(), &target.to_string_lossy()) {
        let _ = std::fs::remove_file(existing);
    }
    Ok(())
}

/// C# `RenamedInstall.TargetOf`: the path a shortcut starts (`IShellLinkW::GetPath`), None when it cannot be read.
pub fn shortcut_target(shortcut: &Path) -> Option<String> {
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistFile, STGM_READ};
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
    use windows::core::{HSTRING, Interface};
    let _apartment = Apartment::enter();
    // SAFETY: COM calls on a ShellLink object this function created; the buffer and its length in units.
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        link.cast::<IPersistFile>().ok()?.Load(&HSTRING::from(shortcut.as_os_str()), STGM_READ).ok()?;
        let mut path = [0u16; 1024];
        link.GetPath(&mut path, std::ptr::null_mut(), 0).ok()?;
        let end = path.iter().position(|unit| *unit == 0).unwrap_or(path.len());
        Some(String::from_utf16_lossy(&path[..end]))
    }
}

/// A shortcut at `shortcut` that starts `target` (tests: the shortcuts a 0.4.x install left, in a folder of the test's own).
pub fn create_shortcut(shortcut: &Path, target: &Path) -> windows::core::Result<()> {
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistFile};
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
    use windows::core::{HSTRING, Interface};
    let _apartment = Apartment::enter();
    // SAFETY: COM calls on a ShellLink object this function created.
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        link.SetPath(&HSTRING::from(target.as_os_str()))?;
        link.cast::<IPersistFile>()?.Save(&HSTRING::from(shortcut.as_os_str()), true)
    }
}
