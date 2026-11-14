//! `DelegationPaths` (C# Core/DelegationWorktree.cs:327-379): folder paths as delegation compares them on Windows: full, with one kind
//! of separator and no closing one, and without regard to case. Where the Mac resolves a path with realpath, [`DelegationPaths::canonical`] asks
//! the OS for the folder's final name (mighty-platform's `FileHandles::final_directory_path`, `GetFinalPathNameByHandleW` on Windows),
//! the way git lists worktrees: short 8.3 names are spelled out and junctions and links followed, as far as the path exists. Owner:
//! W7-B.

use std::cmp::Ordering;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use mighty_core::normalize::ordinal_ignore_case;
use mighty_platform::FileHandles;
use mighty_run::os;

/// `DelegationPaths.Standardized`: the full path with the platform's separators and no closing one (a root keeps its own); as it is
/// when it names no path.
pub fn standardized(path: &str) -> String {
    if path.is_empty() || path.contains('\0') {
        return path.to_string();
    }
    let Ok(full) = std::path::absolute(path) else { return path.to_string() };
    // `Path.GetFullPath` takes `.` and `..` out on every OS (std's absolute does on Windows only), and leaves no closing separator
    // once the components are joined again.
    let mut joined = PathBuf::new();
    for component in full.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if joined.parent().is_some() {
                    joined.pop();
                }
            }
            other => joined.push(other.as_os_str()),
        }
    }
    joined.to_string_lossy().into_owned()
}

/// Whether `path` itself is a symbolic link, a junction or another link (read without following it): a worktree folder git must
/// never be pointed through.
pub fn is_link(path: &str) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink())
}

/// `DelegationPaths.Same`: equal, ignoring case on Windows.
pub fn same(first: &str, second: &str) -> bool {
    if os::windows() { ordinal_ignore_case(first, second) == Ordering::Equal } else { first == second }
}

/// `StartsWith` under `DelegationPaths.Comparison`.
fn starts_with(text: &str, start: &str) -> bool {
    if !os::windows() {
        return text.starts_with(start);
    }
    let count = start.chars().count();
    let head: String = text.chars().take(count).collect();
    head.chars().count() == count && same(&head, start)
}

/// `Path.DirectorySeparatorChar`.
fn separator() -> char {
    if os::windows() { '\\' } else { '/' }
}

/// `Path.EndsInDirectorySeparator`.
fn ends_in_separator(path: &str) -> bool {
    path.ends_with(separator()) || os::windows() && path.ends_with('/')
}

/// `DelegationPaths.IsInside`: whether `path` lies inside `folder` (below it, not beside it, not itself), both already standardized or
/// canonical.
pub fn is_inside(path: &str, folder: &str) -> bool {
    if ends_in_separator(folder) { starts_with(path, folder) } else { starts_with(path, &format!("{folder}{}", separator())) }
}

/// The OS's final names of paths (`DelegationPaths.Canonical`).
#[derive(Clone)]
pub struct DelegationPaths {
    handles: Arc<dyn FileHandles>,
}

impl DelegationPaths {
    /// `handles`: the platform's (`WindowsFileHandles` on Windows; an OS that does not ask handles keeps every existing path as it is,
    /// as C# off Windows does).
    pub fn new(handles: Arc<dyn FileHandles>) -> DelegationPaths {
        DelegationPaths { handles }
    }

    /// `DelegationPaths.Canonical`: `path` standardized, with its links resolved as far as it exists (M/DelegationCleanup.swift:176-189).
    pub fn canonical(&self, path: &str) -> String {
        let standard = standardized(path);
        let mut existing = PathBuf::from(&standard);
        let mut missing: Vec<std::ffi::OsString> = vec![];
        loop {
            if let Some(resolved) = self.final_name(&existing) {
                return missing.iter().rev().fold(resolved, |folder, name| os::combine(&folder, &name.to_string_lossy()));
            }
            let (Some(parent), Some(name)) = (existing.parent(), existing.file_name()) else { return standard };
            if parent.as_os_str().is_empty() || parent == existing {
                return standard;
            }
            missing.push(name.to_os_string());
            existing = parent.to_path_buf();
        }
    }

    /// Where `path` itself lies: its parent folder [`DelegationPaths::canonical`] and its own last name as written, never resolved, so a
    /// worktree folder replaced by a link or junction is still named as the link, not as what it points to (beyond C#, whose
    /// `Canonical` follows it). A root is [`DelegationPaths::canonical`] itself.
    pub fn canonical_place(&self, path: &str) -> String {
        let standard = standardized(path);
        let place = Path::new(&standard);
        match (place.parent(), place.file_name()) {
            (Some(parent), Some(name)) if !parent.as_os_str().is_empty() => {
                os::combine(&self.canonical(&parent.to_string_lossy()), &name.to_string_lossy())
            }
            _ => self.canonical(&standard),
        }
    }

    /// `FinalName`: what the OS calls the file or folder at `path`, None when it does not exist or cannot be opened.
    fn final_name(&self, path: &Path) -> Option<String> {
        if !self.handles.asks_handles() {
            return path.exists().then(|| path.to_string_lossy().into_owned());
        }
        self.handles.final_directory_path(path).map(|found| found.to_string_lossy().into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inside_means_below_not_beside() {
        let root = if os::windows() { r"C:\x" } else { "/x" };
        let real = if os::windows() { r"C:\x\Real" } else { "/x/Real" };
        let below = format!("{real}{}in", separator());
        let folder = if os::windows() { real.to_uppercase() } else { real.to_string() };
        assert!(is_inside(&below, &folder));
        assert!(!is_inside(&format!("{real}2"), real));
        assert!(!is_inside(real, real));
        assert!(is_inside(real, root));
        if os::windows() {
            assert!(is_inside(r"C:\x", r"C:\"));
        }
    }
}
