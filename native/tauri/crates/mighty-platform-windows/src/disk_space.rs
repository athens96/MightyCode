//! A volume's free space on Windows (C# `ChildWorktreeMaker.AvailableBytes`, DelegationWorktree.cs:151-171: a child worktree needs
//! 10,000,000,000 bytes): `GetDiskFreeSpaceExW`'s `lpFreeBytesAvailableToCaller` for the path's nearest existing folder, so the
//! folder's own volume answers (a mount point or a UNC share decides, not the drive letter). Owner from W7 Step 0: W7-B.

use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use mighty_platform::{DiskSpace, PlatformError, Result};
use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

#[derive(Debug, Default, Clone, Copy)]
pub struct WindowsDiskSpace;

/// `Path.GetFullPath(path)`, then its parents until one is an existing folder; None when none is.
fn nearest_folder(path: &Path) -> Option<PathBuf> {
    let mut place = std::path::absolute(path).ok()?;
    while !place.is_dir() {
        place = place.parent()?.to_path_buf();
    }
    Some(place)
}

impl DiskSpace for WindowsDiskSpace {
    fn available_bytes(&self, path: &Path) -> Result<u64> {
        let folder = nearest_folder(path).ok_or_else(|| PlatformError::Invalid(format!("no folder holds {}", path.display())))?;
        // The folder with a closing separator, as GetDiskFreeSpaceExW asks of a directory name.
        let mut name: Vec<u16> = folder.as_os_str().encode_wide().collect();
        if name.last().is_none_or(|last| *last != u16::from(b'\\') && *last != u16::from(b'/')) {
            name.push(u16::from(b'\\'));
        }
        name.push(0);
        let mut available = 0u64;
        // SAFETY: a NUL-terminated name; the two other outputs are not asked for.
        let done = unsafe { GetDiskFreeSpaceExW(name.as_ptr(), &mut available, std::ptr::null_mut(), std::ptr::null_mut()) };
        if done == 0 {
            return Err(PlatformError::Io(std::io::Error::last_os_error()));
        }
        Ok(available)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Free space is the volume's, read from the nearest existing folder, and unknown where no folder is.
    #[test]
    fn free_space_is_the_volumes_and_unknown_where_there_is_none() {
        let here = std::env::temp_dir();
        assert!(WindowsDiskSpace.available_bytes(&here).is_ok_and(|bytes| bytes > 0));
        let missing = here.join(format!("mighty-missing-{}", std::process::id())).join("a").join("b");
        assert!(!missing.exists());
        assert!(WindowsDiskSpace.available_bytes(&missing).is_ok_and(|bytes| bytes > 0));
        if let Some(none) = ('D'..='Z').map(|letter| format!("{letter}:\\")).find(|drive| !Path::new(drive).is_dir()) {
            assert!(WindowsDiskSpace.available_bytes(&Path::new(&none).join("a").join("b")).is_err());
        }
    }
}
