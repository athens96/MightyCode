//! The Windows clipboard's text (C# `Clipboard.SetContent(DataPackage.SetText)` and `Clipboard.GetContent().GetTextAsync()`,
//! MainWindow.cs:285, MainWindow.Terminal.cs:254-262), through the Win32 clipboard in `CF_UNICODETEXT`.

use std::time::Duration;

use mighty_platform::{Clipboard, PlatformError, Result};
use windows_sys::Win32::Foundation::GlobalFree;
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows_sys::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock};

/// `CF_UNICODETEXT` (Win32_System_Ole's constant, kept here so that feature is not needed): NUL-terminated UTF-16.
const CF_UNICODETEXT: u32 = 13;
/// How often, and how far apart, an open is tried while another program holds the clipboard.
const OPEN_TRIES: u32 = 10;
const OPEN_PAUSE: Duration = Duration::from_millis(15);

/// The user's clipboard.
#[derive(Debug, Clone, Copy, Default)]
pub struct WindowsClipboard;

/// The clipboard opened (with no owner window, as WinRT's own does for an app without one), closed again when dropped.
struct Opened;

impl Opened {
    fn open() -> Result<Opened> {
        for attempt in 0..OPEN_TRIES {
            // SAFETY: a null owner; the clipboard is closed by `Drop`.
            if unsafe { OpenClipboard(std::ptr::null_mut()) } != 0 {
                return Ok(Opened);
            }
            if attempt + 1 < OPEN_TRIES {
                std::thread::sleep(OPEN_PAUSE);
            }
        }
        Err(PlatformError::Refused(format!("the clipboard is in use ({})", std::io::Error::last_os_error())))
    }
}

impl Drop for Opened {
    fn drop(&mut self) {
        // SAFETY: the clipboard this thread opened.
        unsafe { CloseClipboard() };
    }
}

impl Clipboard for WindowsClipboard {
    fn read_text(&self) -> Result<Option<String>> {
        // SAFETY: a format query, no clipboard open needed.
        if unsafe { IsClipboardFormatAvailable(CF_UNICODETEXT) } == 0 {
            return Ok(None);
        }
        let _open = Opened::open()?;
        // SAFETY: the clipboard is open; the handle belongs to the clipboard and is only read under its lock.
        let handle = unsafe { GetClipboardData(CF_UNICODETEXT) };
        if handle.is_null() {
            return Ok(None);
        }
        // SAFETY: the clipboard's own memory, locked for the read and unlocked after it.
        unsafe {
            let size = GlobalSize(handle) / 2;
            let data = GlobalLock(handle) as *const u16;
            if data.is_null() {
                return Err(PlatformError::Io(std::io::Error::last_os_error()));
            }
            let units = std::slice::from_raw_parts(data, size);
            let end = units.iter().position(|unit| *unit == 0).unwrap_or(units.len());
            let text = String::from_utf16_lossy(&units[..end]);
            GlobalUnlock(handle);
            Ok(Some(text))
        }
    }

    fn write_text(&self, text: &str) -> Result<()> {
        let units: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        let _open = Opened::open()?;
        // SAFETY: the clipboard is open.
        if unsafe { EmptyClipboard() } == 0 {
            return Err(PlatformError::Io(std::io::Error::last_os_error()));
        }
        put(CF_UNICODETEXT, &units)
    }

    /// C# `CopyRemoteKeyAsync`'s data package: the text with `ExcludeClipboardContentFromMonitorProcessing` (any data),
    /// `CanIncludeInClipboardHistory` and `CanUploadToCloudClipboard` (a DWORD 0 each) beside it, all in one open.
    fn write_secret(&self, text: &str) -> Result<()> {
        let units: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        let formats = [
            ("ExcludeClipboardContentFromMonitorProcessing", 0u32),
            ("CanIncludeInClipboardHistory", 0u32),
            ("CanUploadToCloudClipboard", 0u32),
        ]
        .map(|(name, value)| (register(name), value));
        if formats.iter().any(|(format, _)| *format == 0) {
            return Err(PlatformError::Io(std::io::Error::last_os_error()));
        }
        let _open = Opened::open()?;
        // SAFETY: the clipboard is open.
        if unsafe { EmptyClipboard() } == 0 {
            return Err(PlatformError::Io(std::io::Error::last_os_error()));
        }
        for (format, value) in formats {
            put(format, &[value])?;
        }
        put(CF_UNICODETEXT, &units)
    }
}

/// A registered clipboard format's id (0 when Windows refused it).
fn register(name: &str) -> u32 {
    let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: a NUL-terminated UTF-16 name that outlives the call.
    unsafe { RegisterClipboardFormatW(wide.as_ptr()) }
}

/// `data` put on the open clipboard in `format`: a movable block of its size, filled under its lock. Once SetClipboardData takes it the
/// system owns it; until then it is freed here on failure.
fn put<T: Copy>(format: u32, data: &[T]) -> Result<()> {
    let bytes = std::mem::size_of_val(data);
    // SAFETY: the caller holds the clipboard open; the block is the data's size and only written under its lock.
    unsafe {
        let block = GlobalAlloc(GMEM_MOVEABLE, bytes);
        if block.is_null() {
            return Err(PlatformError::Io(std::io::Error::last_os_error()));
        }
        let target = GlobalLock(block) as *mut u8;
        if target.is_null() {
            let error = std::io::Error::last_os_error();
            GlobalFree(block);
            return Err(PlatformError::Io(error));
        }
        std::ptr::copy_nonoverlapping(data.as_ptr() as *const u8, target, bytes);
        GlobalUnlock(block);
        if SetClipboardData(format, block).is_null() {
            let error = std::io::Error::last_os_error();
            GlobalFree(block);
            return Err(PlatformError::Io(error));
        }
    }
    Ok(())
}
