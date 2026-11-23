//! Read-only file access on an SSH machine: the remote commands of a folder listing, a file read and the folder's git changes, each
//! capped on the far side (`head`), and their output read. Paths are relative to the workspace's remote folder, never `..`, and
//! always passed as `./<quoted>` so a name can never be an option.

use super::command::{quote, quote_dir};

/// The most names a listing reads (the files pane's own limit).
pub const MAXIMUM_ENTRIES: usize = 5_000;
/// The most bytes a file read takes (the preview's text limit).
pub const MAXIMUM_READ_BYTES: usize = 1024 * 1024;
/// The most bytes of a diff.
pub const MAXIMUM_DIFF_BYTES: usize = 2 * 1024 * 1024;

/// A path under the workspace folder a read may name: empty (the folder itself) or `/`-separated parts, none empty, `.` or `..`, no
/// control character, not absolute.
pub fn valid_relative(relative: &str) -> bool {
    if relative.is_empty() {
        return true;
    }
    relative.encode_utf16().count() <= 4096
        && !relative.starts_with('/')
        && !relative.chars().any(char::is_control)
        && relative.split('/').all(|part| !part.is_empty() && part != "." && part != "..")
}

fn under(relative: &str) -> String {
    if relative.is_empty() { "'.'".into() } else { format!("./{}", quote(relative)) }
}

/// One folder's names, folders marked with a trailing `/` (`ls -1Apq`: hidden ones too, odd characters shown as `?`).
pub fn list_command(dir: &str, relative: &str) -> Option<String> {
    valid_relative(relative)
        .then(|| format!("cd {} && LC_ALL=C ls -1Apq {} | head -n {}", quote_dir(dir), under(relative), MAXIMUM_ENTRIES + 1))
}

/// A regular file's first bytes, one more than the cap so a longer file is known to be cut.
pub fn read_command(dir: &str, relative: &str) -> Option<String> {
    (valid_relative(relative) && !relative.is_empty()).then(|| {
        let path = under(relative);
        format!("cd {} && test -f {path} && head -c {} {path}", quote_dir(dir), MAXIMUM_READ_BYTES + 1)
    })
}

/// The folder's uncommitted changes against `HEAD` as a patch, capped.
pub fn diff_command(dir: &str) -> String {
    format!("git -C {} diff --no-color --no-ext-diff HEAD | head -c {}", quote_dir(dir), MAXIMUM_DIFF_BYTES + 1)
}

/// One listed name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteEntry {
    pub name: String,
    pub relative_path: String,
    pub is_directory: bool,
}

/// The listing read: folders first, then names (as the files pane sorts them itself, this only keeps the order stable); whether names
/// were left out.
pub fn parse_listing(stdout: &str, relative: &str) -> (Vec<RemoteEntry>, bool) {
    let mut entries: Vec<RemoteEntry> = stdout
        .lines()
        .filter(|line| !line.is_empty() && *line != "./" && *line != "../")
        .map(|line| {
            let (name, is_directory) = match line.strip_suffix('/') {
                Some(name) => (name, true),
                None => (line, false),
            };
            let relative_path = if relative.is_empty() { name.to_string() } else { format!("{relative}/{name}") };
            RemoteEntry { name: name.to_string(), relative_path, is_directory }
        })
        .filter(|entry| !entry.name.contains('/'))
        .collect();
    let truncated = entries.len() > MAXIMUM_ENTRIES;
    entries.truncate(MAXIMUM_ENTRIES);
    entries.sort_by(|a, b| b.is_directory.cmp(&a.is_directory).then_with(|| a.name.cmp(&b.name)));
    (entries, truncated)
}

/// A read's bytes and whether the file was longer than the cap.
pub fn cut(mut bytes: Vec<u8>, cap: usize) -> (Vec<u8>, bool) {
    let longer = bytes.len() > cap;
    bytes.truncate(cap);
    (bytes, longer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths_stay_under_the_folder() {
        for good in ["", "src", "src/main.rs", "a b/it's", "-rf"] {
            assert!(valid_relative(good), "{good}");
        }
        for bad in ["/etc/passwd", "..", "a/../b", "a//b", "./a", "a/", "a\nb"] {
            assert!(!valid_relative(bad), "{bad:?}");
        }
    }

    #[test]
    fn commands_quote_and_cap() {
        assert_eq!(list_command("~/app", "").unwrap(), "cd ~/'app' && LC_ALL=C ls -1Apq '.' | head -n 5001");
        assert_eq!(list_command("/srv/a b", "-rf dir").unwrap(), "cd '/srv/a b' && LC_ALL=C ls -1Apq ./'-rf dir' | head -n 5001");
        assert_eq!(read_command("/srv", "it's.txt").unwrap(), "cd '/srv' && test -f ./'it'\\''s.txt' && head -c 1048577 ./'it'\\''s.txt'");
        assert_eq!(read_command("/srv", ""), None);
        assert_eq!(list_command("/srv", "../x"), None);
        assert_eq!(diff_command("/srv/app"), "git -C '/srv/app' diff --no-color --no-ext-diff HEAD | head -c 2097153");
    }

    #[test]
    fn listings_are_read() {
        let (entries, truncated) = parse_listing("./\n../\nsrc/\nREADME.md\n.env\nnode_modules/\n", "app");
        assert!(!truncated);
        let names: Vec<(&str, bool)> = entries.iter().map(|e| (e.name.as_str(), e.is_directory)).collect();
        assert_eq!(names, [("node_modules", true), ("src", true), (".env", false), ("README.md", false)]);
        assert_eq!(entries[1].relative_path, "app/src");
        let many: String = (0..5001).map(|i| format!("f{i}\n")).collect();
        let (entries, truncated) = parse_listing(&many, "");
        assert!(truncated && entries.len() == 5000);
        assert_eq!(cut(vec![1, 2, 3], 2), (vec![1, 2], true));
        assert_eq!(cut(vec![1, 2], 2), (vec![1, 2], false));
    }
}
