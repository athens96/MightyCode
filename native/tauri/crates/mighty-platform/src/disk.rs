//! The free space of a volume (C# `ChildWorktreeMaker.AvailableBytes`, `GetDiskFreeSpaceExW`, in DelegationWorktree.cs's check: a
//! child worktree needs 10,000,000,000 bytes; the Mac's `volumeAvailableCapacityForImportantUsage`). Owner from W7 Step 0: W7-B.

use std::path::Path;

use crate::Result;

/// How much the current user may still write on the volume that holds a path.
pub trait DiskSpace: Send + Sync {
    /// The bytes available to the caller (quotas counted) on the volume of `path`, or of its nearest existing parent folder when it
    /// does not exist yet (the worktree root before the first child). An error when no folder on the way exists or the OS cannot say:
    /// delegation counts that as too little.
    fn available_bytes(&self, path: &Path) -> Result<u64>;
}
