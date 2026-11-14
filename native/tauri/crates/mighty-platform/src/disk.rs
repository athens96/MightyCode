//! The free space of a volume (C# `GetDiskFreeSpaceExW` in DelegationWorktree.cs's check: a child worktree needs 10,000,000,000 bytes;
//! the Mac's `volumeAvailableCapacityForImportantUsage`). Owner from W7 Step 0: W7-B.

use std::path::Path;

use crate::Result;

/// How much the current user may still write on the volume that holds a path.
pub trait DiskSpace: Send + Sync {
    /// The bytes available to the caller (quotas counted) on the volume of `path`, an existing folder.
    fn available_bytes(&self, path: &Path) -> Result<u64>;
}
