//! M-G: the setup screen's files port on the Mac (contract `mighty_platform::setup`), Swift `SetupScriptFiles` (SetupPlan.swift:198-245):
//! the script folder is this user's alone (made 0700, then checked: a real folder, not a link, owned by this user, closed to everyone
//! else), a marker counts only as a real folder, and leftovers are removed without following a link (Swift `SetupPlanTests.swift:163`
//! "theScriptFolderAndFilesRefuseLinksAndOtherUsers"). What is Windows' alone answers as nothing: no PowerShell runs here, and the
//! system folder is Windows'.

use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _};
use std::path::Path;
use std::sync::Arc;

use mighty_platform::{PlatformError, Result, SetupFiles};

/// The Mac's `SetupFiles`.
pub fn setup_files() -> Arc<dyn SetupFiles> {
    Arc::new(MacSetupFiles)
}

fn plain_folder(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_dir())
}

/// Swift `SetupScriptFiles`.
#[derive(Debug, Clone, Copy, Default)]
pub struct MacSetupFiles;

impl SetupFiles for MacSetupFiles {
    /// Swift `prepareDirectory`: `mkdir(path, 0700)` (its parents too, as the contract asks), then `lstat`: a folder, this user's,
    /// with no access for the group or others.
    fn prepare_folder(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        if let Err(error) = std::fs::DirBuilder::new().mode(0o700).create(path)
            && error.kind() != std::io::ErrorKind::AlreadyExists
        {
            return Err(error.into());
        }
        let meta = std::fs::symlink_metadata(path)?;
        let shown = String::from_utf8_lossy(path.as_os_str().as_bytes()).into_owned();
        if !meta.file_type().is_dir() {
            return Err(PlatformError::Refused(format!("setup folder is not a plain folder: {shown}")));
        }
        // SAFETY: getuid has no preconditions.
        if meta.uid() != unsafe { libc::getuid() } {
            return Err(PlatformError::Refused(format!("setup folder belongs to another user: {shown}")));
        }
        if meta.mode() & 0o077 != 0 {
            return Err(PlatformError::Refused(format!("setup folder is open to other users: {shown}")));
        }
        Ok(())
    }

    /// Swift `markerExists`: `lstat` says a folder (a link to one does not count).
    fn is_plain_folder(&self, path: &Path) -> bool {
        plain_folder(path)
    }

    /// Swift `removeLeftovers`: every entry removed; a link is unlinked, never followed.
    fn remove_leftovers(&self, folder: &Path) {
        let Ok(entries) = std::fs::read_dir(folder) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = std::fs::symlink_metadata(&path) else { continue };
            let _ = if meta.file_type().is_dir() { std::fs::remove_dir_all(&path) } else { std::fs::remove_file(&path) };
        }
    }

    /// Running as root (the Mac's elevated token).
    fn is_elevated(&self) -> bool {
        // SAFETY: geteuid has no preconditions.
        unsafe { libc::geteuid() == 0 }
    }

    /// Windows PowerShell never runs on the Mac.
    fn is_power_shell_running(&self, _id: u32) -> bool {
        false
    }

    /// Windows' system folder; the Mac names none.
    fn system_folder(&self) -> Option<String> {
        None
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;

    use super::*;

    /// Swift `SetupPlanTests.swift:163` "theScriptFolderAndFilesRefuseLinksAndOtherUsers": the folder is made closed to others and
    /// prepared again as it is; opened to others, or a link to a folder, it is refused; a marker that is a link to a folder does not
    /// count; the leftovers go, the links among them without being followed, and the folder around stays. (Swift's script file
    /// itself is mighty-tools' `write_script`, a new file in this folder.)
    #[test]
    fn the_script_folder_and_files_refuse_links_and_other_users() {
        let root = std::env::temp_dir().join(format!("setup-files-{}-{}", std::process::id(), line!()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let files = MacSetupFiles;
        let folder = root.join("setup");
        files.prepare_folder(&folder).unwrap();
        let mode = std::fs::symlink_metadata(&folder).unwrap().mode();
        assert_eq!(mode & 0o077, 0, "closed to others: {mode:o}");
        files.prepare_folder(&folder).unwrap();
        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(files.prepare_folder(&folder).is_err(), "open to others");
        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o700)).unwrap();
        let link = root.join("linked");
        std::os::unix::fs::symlink(&folder, &link).unwrap();
        assert!(files.prepare_folder(&link).is_err(), "a link to a folder");
        assert!(files.is_plain_folder(&folder) && !files.is_plain_folder(&link));

        let done = folder.join("done-1");
        assert!(!files.is_plain_folder(&done));
        std::os::unix::fs::symlink(&root, &done).unwrap();
        assert!(!files.is_plain_folder(&done), "a marker that is a link to a folder does not count");
        std::fs::create_dir(folder.join("started-1")).unwrap();
        std::fs::write(folder.join("started-1/pid"), b"1").unwrap();
        std::fs::write(folder.join("install-1.ps1"), b"echo one").unwrap();
        files.remove_leftovers(&folder);
        assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 0);
        assert!(root.join("setup").is_dir() && root.join("linked").exists(), "the link was not followed");

        assert!(!files.is_power_shell_running(std::process::id()) && files.system_folder().is_none());
        assert_eq!(files.is_elevated(), unsafe { libc::geteuid() } == 0);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
