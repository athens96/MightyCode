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

/// Whether names on a volume compare ignoring case, from what the volume said: `Some(true)` when it called itself case-sensitive,
/// `Some(false)` when not, None when it could not say. Unsaid counts as ignoring case, the default of the Mac's APFS and HFS+ and of
/// Windows' NTFS: a reference that differs only in case is then still found and listed once (`StdFiles::ignores_case`).
pub fn names_ignore_case(case_sensitive: Option<bool>) -> bool {
    !case_sensitive.unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_volume_ignores_case_unless_it_says_it_does_not() {
        assert!(!names_ignore_case(Some(true)), "a case-sensitive volume (a case-sensitive APFS, most Linux file systems)");
        assert!(names_ignore_case(Some(false)), "APFS and HFS+ as installed, NTFS");
        assert!(names_ignore_case(None), "a volume that cannot say reads as the Mac's and Windows' default");
    }
}
