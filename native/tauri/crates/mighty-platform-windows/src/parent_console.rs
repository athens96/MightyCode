//! MC-43: the windowed exe's `--cli` mode on its caller's console. A release `MightyCode.exe` is a GUI-subsystem program: started
//! from a console (cmd, PowerShell, Windows Terminal, the `mighty` command) it gets no console of its own, and an output nobody
//! redirected goes nowhere. This mode attaches to its parent's console and points only the standard handles it was given none
//! usable for at `CONIN$`/`CONOUT$` (`mighty_platform::control_access::reopen_on_console`), so `> out.json`, `| jq`, `< in.txt`,
//! `> nul` and the handles the `mighty` command hands down are kept. Attaching also brings the console's Ctrl+C to this process.
//! Rust's standard streams look the handle up on every write, so nothing else changes.

use std::ptr::null_mut;

use mighty_platform::control_access::{StdHandle, reopen_on_console};
use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_TYPE_CHAR, FILE_TYPE_DISK, FILE_TYPE_PIPE, GetFileType, OPEN_EXISTING,
};
use windows_sys::Win32::System::Console::{
    ATTACH_PARENT_PROCESS, AttachConsole, GetConsoleWindow, GetStdHandle, STD_ERROR_HANDLE, STD_HANDLE, STD_INPUT_HANDLE,
    STD_OUTPUT_HANDLE, SetStdHandle,
};

use crate::win::wide;

/// What a standard handle is.
fn kind(handle: HANDLE) -> StdHandle {
    if handle.is_null() {
        return StdHandle::Missing;
    }
    if handle == INVALID_HANDLE_VALUE {
        return StdHandle::Invalid;
    }
    // SAFETY: GetFileType only reads the handle's type; an invalid handle answers FILE_TYPE_UNKNOWN.
    match unsafe { GetFileType(handle) } {
        FILE_TYPE_DISK => StdHandle::Disk,
        FILE_TYPE_PIPE => StdHandle::Pipe,
        FILE_TYPE_CHAR => StdHandle::Character,
        _ => StdHandle::Invalid,
    }
}

/// Attaches to the parent's console (nothing when it has none, or this process already has one) and gives each standard handle that
/// has nothing usable the console's input or output. Whether it attached.
pub fn attach() -> bool {
    // SAFETY: plain Win32 calls on this process's console and standard handles; the console handles opened here stay open for the
    // process's life (they are its standard handles).
    unsafe {
        if !GetConsoleWindow().is_null() || AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
            return false;
        }
        let streams: [(STD_HANDLE, &str); 3] =
            [(STD_INPUT_HANDLE, "CONIN$"), (STD_OUTPUT_HANDLE, "CONOUT$"), (STD_ERROR_HANDLE, "CONOUT$")];
        for (id, device) in streams {
            if !reopen_on_console(kind(GetStdHandle(id))) {
                continue;
            }
            let name = wide(device);
            let handle = CreateFileW(
                name.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                null_mut(),
                OPEN_EXISTING,
                0,
                null_mut(),
            );
            if handle != INVALID_HANDLE_VALUE {
                SetStdHandle(id, handle);
            }
        }
        true
    }
}
