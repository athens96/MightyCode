//! One `dotnet build` of tools/parity at a time across the workspace's test binaries. `cargo test` runs them in parallel and they all
//! build this project, sharing its `obj` folder (and some, `target/parity-cs`): two builds at once failed one of them ("the C# parity
//! tool did not build"). Each test that builds it includes this file (`#[path]`) and holds [`hold`]'s lock for the build: a lock
//! file in the target folder made with `create_new`, removed when dropped; one left by a run that died (older than ten minutes) is
//! taken over.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// The lock, held until dropped.
pub struct BuildLock(PathBuf);

impl Drop for BuildLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Waits for the build lock in `target` (the workspace's target folder) and takes it.
pub fn hold(target: &Path) -> BuildLock {
    let _ = std::fs::create_dir_all(target);
    let path = target.join("parity-cs-build.lock");
    loop {
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(_) => return BuildLock(path),
            // Windows answers PermissionDenied while the last holder's file is being deleted.
            Err(error) if matches!(error.kind(), std::io::ErrorKind::AlreadyExists | std::io::ErrorKind::PermissionDenied) => {
                let age = std::fs::metadata(&path).and_then(|m| m.modified()).ok().and_then(|at| SystemTime::now().duration_since(at).ok());
                if age.is_some_and(|age| age > Duration::from_secs(600)) {
                    let _ = std::fs::remove_file(&path);
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(error) => panic!("the parity build lock {}: {error}", path.display()),
        }
    }
}
