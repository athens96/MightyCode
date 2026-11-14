//! A volume's free space on Windows (C# `GetDiskFreeSpaceExW` in DelegationWorktree.cs's check: a child worktree needs 10,000,000,000
//! bytes). Owner from W7 Step 0: W7-B, which fills it with `GetDiskFreeSpaceExW`'s `lpFreeBytesAvailableToCaller` (windows-sys
//! `Win32_Storage_FileSystem`). Until then it refuses as `Unsupported`.

use std::path::Path;

use mighty_platform::{DiskSpace, PlatformError, Result};

#[derive(Debug, Default, Clone, Copy)]
pub struct WindowsDiskSpace;

impl DiskSpace for WindowsDiskSpace {
    fn available_bytes(&self, _path: &Path) -> Result<u64> {
        Err(PlatformError::unsupported("DiskSpace", "available_bytes"))
    }
}
