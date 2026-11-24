//! MC-43: the user's PATH (`HKCU\Environment\Path`) as it is stored (unexpanded, with its own type), written back with that type, and
//! the change announced so terminals opened afterwards see it (Explorer re-reads the environment on `WM_SETTINGCHANGE`). The
//! `mighty` command's folder goes in here, never in the machine's PATH (no administrator).

use std::io;

use windows_sys::Win32::Foundation::LPARAM;
use windows_sys::Win32::System::Registry::{REG_EXPAND_SZ, REG_SZ};
use windows_sys::Win32::UI::WindowsAndMessaging::{HWND_BROADCAST, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_SETTINGCHANGE};

use crate::registry::CurrentUserRegistry;
use crate::win::{equal_ignore_case, wide};

const KEY: &str = "Environment";
const NAME: &str = "Path";

/// The user's PATH as stored: its value's name as written (`Path`, `PATH`, …), whether it is `REG_EXPAND_SZ`, and its text. None
/// when the user has no PATH of their own; an error for a value of another type, which is left alone.
pub fn read() -> io::Result<Option<(String, bool, String)>> {
    let found = CurrentUserRegistry::new().values(KEY)?.into_iter().find(|value| equal_ignore_case(&value.name, NAME));
    let Some(value) = found else { return Ok(None) };
    if value.kind != REG_SZ && value.kind != REG_EXPAND_SZ {
        return Err(io::Error::other("the user's PATH is not text"));
    }
    let units: Vec<u16> = value.data.as_chunks::<2>().0.iter().map(|pair| u16::from_le_bytes(*pair)).collect();
    let end = units.iter().position(|unit| *unit == 0).unwrap_or(units.len());
    Ok(Some((value.name, value.kind == REG_EXPAND_SZ, String::from_utf16_lossy(&units[..end]))))
}

/// Writes the user's PATH under `name` (as [`read`] found it, or `Path`) with its type kept (`REG_EXPAND_SZ` for a new one, as
/// Windows makes it).
pub fn write(name: Option<&str>, expandable: bool, text: &str) -> io::Result<()> {
    let bytes: Vec<u8> = text.encode_utf16().chain(std::iter::once(0)).flat_map(u16::to_le_bytes).collect();
    let kind = if expandable { REG_EXPAND_SZ } else { REG_SZ };
    CurrentUserRegistry::new().set_value(KEY, name.unwrap_or(NAME), kind, &bytes)
}

/// Tells the windows that the environment changed (at most a second for each one that does not answer).
pub fn announce() {
    let environment = wide(KEY);
    // SAFETY: a NUL-terminated string that lives until the call returns; the result is not read.
    unsafe {
        SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            0,
            environment.as_ptr() as LPARAM,
            SMTO_ABORTIFHUNG,
            1000,
            std::ptr::null_mut(),
        );
    }
}
