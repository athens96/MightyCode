//! The files a style reads (C# `StyleFiles` in `StyleStateEngine.cs`): read through the validated open handle, never by reopening a
//! checked path — relative paths only (no `..`, drive, UNC root), a single link, bounded; and the glob a file source names. The handle
//! checks are mighty-composer's `open_file` with the host's `FileHandles` port (as mighty-companion's `prefs::read_bounded`). Owner: W9-D.

use std::collections::BTreeSet;
use std::io::Read;
use std::path::Path;

use regex::Regex;

use crate::decoder::{MAXIMUM_BYTES, safe_relative_path};
use crate::manifest::ordinal;

/// `StyleFiles.Read(root, relative, maximum)`: the file at `relative` under `root`, opened there, a regular file with one link, at most
/// `maximum` bytes; None when any of that fails.
pub fn read(root: &Path, relative: &str, maximum: usize) -> Option<Vec<u8>> {
    if !safe_relative_path(relative) || root.to_string_lossy().starts_with("\\\\") {
        return None;
    }
    let handles = mighty_composer::files::file_handles();
    let opened = mighty_composer::files::open_file(relative, root, handles.as_ref()).ok()?;
    if handles.asks_handles() && !handles.single_link(&opened.file) {
        return None;
    }
    if !opened.file.metadata().ok()?.is_file() || opened.size > maximum as u64 {
        return None;
    }
    let mut bytes = Vec::new();
    opened.file.take(maximum as u64 + 1).read_to_end(&mut bytes).ok()?;
    (bytes.len() <= maximum).then_some(bytes)
}

/// [`read`] at the decoder's bound.
pub fn read_manifest_sized(root: &Path, relative: &str) -> Option<Vec<u8>> {
    read(root, relative, MAXIMUM_BYTES)
}

/// `StyleFiles.Matches(root, pattern)`: the files under `root` a `*`/`?`/`**` pattern names, at most 32, 1024 entries looked at,
/// 8 folders deep, dot files only for a dot pattern, every step resolved under the root; in ordinal order.
pub fn matches(root: &Path, pattern: &str) -> Vec<String> {
    if !safe_relative_path(pattern) || root.to_string_lossy().starts_with("\\\\") {
        return Vec::new();
    }
    let normalized = pattern.replace('\\', "/");
    let parts: Vec<&str> = normalized.split('/').filter(|p| !p.is_empty()).collect();
    let mut walk = Walk { root, parts: &parts, found: BTreeSet::new(), budget: 1024 };
    walk.walk("", 0, 0);
    let mut found: Vec<String> = walk.found.into_iter().collect();
    found.sort_by(|a, b| ordinal(a, b));
    found
}

struct Walk<'a> {
    root: &'a Path,
    parts: &'a [&'a str],
    found: BTreeSet<String>,
    budget: i64,
}

impl Walk<'_> {
    fn walk(&mut self, relative: &str, part: usize, depth: usize) {
        if part >= self.parts.len() || depth > 8 || self.found.len() >= 32 || self.budget <= 0 {
            return;
        }
        let Some(directory) = mighty_core::workspace_files::resolve(relative, self.root) else { return };
        if !directory.is_dir() {
            return;
        }
        let key = self.parts[part];
        if key == "**" {
            self.walk(relative, part + 1, depth);
        }
        let Ok(listing) = std::fs::read_dir(&directory) else { return };
        let entries: Vec<String> = listing
            .take(self.budget.max(0) as usize)
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        let shape = Regex::new(&format!("^{}$", regex::escape(key).replace("\\*", ".*").replace("\\?", "."))).ok();
        for name in entries {
            self.budget -= 1;
            if self.budget < 0 || self.found.len() >= 32 {
                return;
            }
            if name.starts_with('.') && !key.starts_with('.') {
                continue;
            }
            let rel = if relative.is_empty() { name.clone() } else { format!("{relative}/{name}") };
            let Some(safe) = mighty_core::workspace_files::resolve(&rel, self.root) else { continue };
            if key == "**" {
                if safe.is_dir() {
                    self.walk(&rel, part, depth + 1);
                }
                continue;
            }
            if !shape.as_ref().is_some_and(|s| s.is_match(&name)) {
                continue;
            }
            if part == self.parts.len() - 1 {
                if safe.is_file() {
                    self.found.insert(rel);
                }
            } else if safe.is_dir() {
                self.walk(&rel, part + 1, depth + 1);
            }
        }
    }
}
