//! M-A: a volume's free space for delegation's worktree check. Swift `DelegationWorktree.swift:75,119`
//! (`volumeAvailableCapacityForImportantUsage`); contract `mighty_platform::disk`.

use std::path::Path;
use std::sync::Arc;

use mighty_platform::{DiskSpace, PlatformError, Result};
use objc2_foundation::{
    NSArray, NSNumber, NSString, NSURL, NSURLVolumeAvailableCapacityForImportantUsageKey, NSURLVolumeAvailableCapacityKey,
};

/// The Mac's `DiskSpace`.
pub fn disk_space() -> Arc<dyn DiskSpace> {
    Arc::new(MacDiskSpace)
}

#[derive(Debug, Default)]
pub struct MacDiskSpace;

impl DiskSpace for MacDiskSpace {
    /// The volume's capacity for important usage (what the system would free for the user, purgeable space included, as Swift reads
    /// it) for `path`, or its nearest existing parent folder. When the volume answers 0 or nothing for it (seen on a CI runner's volume
    /// between two reads), its plain available capacity (`volumeAvailableCapacity`) answers instead, so a volume with room is never
    /// called full.
    fn available_bytes(&self, path: &Path) -> Result<u64> {
        let folder =
            path.ancestors().find(|folder| folder.is_dir()).ok_or_else(|| PlatformError::Invalid("no folder on the way exists".into()))?;
        let url = NSURL::fileURLWithPath(&NSString::from_str(&folder.to_string_lossy()));
        // SAFETY: Foundation constants, valid for the process's life.
        let (important, plain) = unsafe { (NSURLVolumeAvailableCapacityForImportantUsageKey, NSURLVolumeAvailableCapacityKey) };
        let values = url
            .resourceValuesForKeys_error(&NSArray::from_slice(&[important, plain]))
            .map_err(|error| PlatformError::Refused(error.localizedDescription().to_string()))?;
        let read = |key| {
            values
                .objectForKey(key)
                .and_then(|value| value.downcast::<NSNumber>().ok())
                .and_then(|number| u64::try_from(number.longLongValue()).ok())
                .filter(|bytes| *bytes > 0)
        };
        read(important).or_else(|| read(plain)).ok_or_else(|| PlatformError::Refused("the volume did not say its free space".into()))
    }
}

/// Whether names on the volume holding `path` (or its nearest existing parent folder) compare ignoring case:
/// `pathconf(_PC_CASE_SENSITIVE)` asks the volume, as APFS can be formatted either way (case-insensitive as installed); a volume that
/// cannot say reads as ignoring case ([`mighty_platform::disk::names_ignore_case`]).
pub fn names_ignore_case(path: &Path) -> bool {
    mighty_platform::disk::names_ignore_case(path.ancestors().find(|folder| folder.is_dir()).and_then(case_sensitive))
}

/// `pathconf(_PC_CASE_SENSITIVE)` of an existing folder: 1 case-sensitive, 0 not, None when the call fails or the volume has no answer.
fn case_sensitive(folder: &Path) -> Option<bool> {
    use std::os::unix::ffi::OsStrExt;
    let name = std::ffi::CString::new(folder.as_os_str().as_bytes()).ok()?;
    // SAFETY: a NUL-terminated path that lives for the call; pathconf only reads it.
    match unsafe { libc::pathconf(name.as_ptr(), libc::_PC_CASE_SENSITIVE) } {
        1 => Some(true),
        0 => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Swift `DelegationWorktreeTests.swift:119 "defaultRootAndFreeSpaceReader"`, its reader half (the default root,
    /// `~/.mightycode/worktrees`, is mighty-delegation's): the temporary folder's volume says a free space, and a folder not made yet is
    /// read through its nearest existing parent, which is left as it was.
    #[test]
    fn default_root_and_free_space_reader() {
        let temp = std::env::temp_dir();
        let free = MacDiskSpace.available_bytes(&temp).unwrap();
        assert!(free > 0);
        let missing = temp.join("mighty-disk-not-made").join("child");
        let through_parent = MacDiskSpace.available_bytes(&missing).unwrap();
        assert!(through_parent > 0);
        assert!(!missing.exists(), "nothing is made");
    }

    /// The volume's answer matches what the volume does: a file made as `Case.txt` is found as `CASE.TXT` exactly when the temporary
    /// folder's volume says it ignores case; a folder not made yet is asked through its parent.
    #[test]
    fn the_volume_says_whether_names_ignore_case() {
        let folder = std::env::temp_dir().join(format!("mighty-case-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("Case.txt"), b"x").unwrap();
        let found_otherwise = folder.join("CASE.TXT").is_file();
        assert_eq!(names_ignore_case(&folder), found_otherwise, "pathconf agrees with the disk");
        assert_eq!(names_ignore_case(&folder.join("not-made").join("child")), found_otherwise, "through the nearest parent");
        assert!(case_sensitive(&folder.join("not-made")).is_none(), "a missing folder has no answer");
        std::fs::remove_dir_all(&folder).unwrap();
    }
}
