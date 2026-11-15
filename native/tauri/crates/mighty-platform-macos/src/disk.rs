//! M-A: a volume's free space for delegation's worktree check (`volumeAvailableCapacityForImportantUsage`). Swift
//! `DelegationWorktree.swift:75,119`; contract `mighty_platform::disk`.

use std::sync::Arc;

use mighty_platform::DiskSpace;
use mighty_platform::unsupported::Unsupported;

/// The Mac's `DiskSpace` (Unsupported until M-A).
pub fn disk_space() -> Arc<dyn DiskSpace> {
    Arc::new(Unsupported)
}
