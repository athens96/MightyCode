//! The `Host` aliases an ssh_config names, for the machine picker: `Host` lines' patterns that are plain names (a pattern with `*`,
//! `?` or `!` matches many hosts, so it is no machine), files pulled in by `Include` (relative to `~/.ssh`, `~` expanded, `*` and `?`
//! in the last part matched against that folder's files, at most 16 deep, each file once), `Match` blocks' contents left out. Only
//! names: no option value is ever read or passed on. Reading goes through [`ConfigSource`] so the tests use fixtures.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::target::Target;

/// ssh's own limit on nested `Include`s.
const MAXIMUM_DEPTH: usize = 16;
/// The most aliases listed.
pub const MAXIMUM_ALIASES: usize = 500;
/// A config file bigger than this is not read.
const MAXIMUM_FILE_BYTES: usize = 1024 * 1024;

/// Where the config's files are read from.
pub trait ConfigSource {
    /// The file's text, None when it cannot be read.
    fn read(&self, path: &Path) -> Option<String>;
    /// The files directly in `directory` (for an `Include` with a wildcard).
    fn list(&self, directory: &Path) -> Vec<PathBuf>;
}

/// The real files (the app's picker).
pub struct Disk;

impl ConfigSource for Disk {
    fn read(&self, path: &Path) -> Option<String> {
        let metadata = std::fs::metadata(path).ok()?;
        if !metadata.is_file() || metadata.len() as usize > MAXIMUM_FILE_BYTES {
            return None;
        }
        std::fs::read_to_string(path).ok()
    }

    fn list(&self, directory: &Path) -> Vec<PathBuf> {
        std::fs::read_dir(directory).map(|items| items.flatten().map(|item| item.path()).collect()).unwrap_or_default()
    }
}

/// The aliases of `main` (normally `~/.ssh/config`), in the order they appear, each once; `home` is where `~` points (`~/.ssh` is
/// where relative `Include`s are found).
pub fn host_aliases(main: &Path, home: &Path, source: &dyn ConfigSource) -> Vec<String> {
    let mut found = Vec::new();
    let mut seen = HashSet::new();
    let mut visited = HashSet::new();
    read_file(main, home, source, 0, &mut visited, &mut |alias| {
        if found.len() < MAXIMUM_ALIASES && seen.insert(alias.clone()) {
            found.push(alias);
        }
    });
    found
}

fn read_file(
    path: &Path,
    home: &Path,
    source: &dyn ConfigSource,
    depth: usize,
    visited: &mut HashSet<PathBuf>,
    add: &mut dyn FnMut(String),
) {
    if depth > MAXIMUM_DEPTH || !visited.insert(path.to_path_buf()) {
        return;
    }
    let Some(text) = source.read(path) else { return };
    let mut in_match = false;
    for line in text.lines() {
        let Some((keyword, words)) = split_line(line) else { continue };
        match keyword.to_ascii_lowercase().as_str() {
            "host" => {
                in_match = false;
                for pattern in words {
                    if let Some(alias) = plain_alias(&pattern) {
                        add(alias);
                    }
                }
            }
            "match" => in_match = true,
            "include" if !in_match => {
                for word in words {
                    for file in expand_include(&word, home, source) {
                        read_file(&file, home, source, depth + 1, visited, add);
                    }
                }
            }
            _ => {}
        }
    }
}

/// A line's keyword and its words (`Keyword word word`, `Keyword=word`, double-quoted words kept whole); None for a blank line or a
/// comment.
fn split_line(line: &str) -> Option<(String, Vec<String>)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let split = line.find(|c: char| c.is_whitespace() || c == '=').unwrap_or(line.len());
    let keyword = line[..split].to_string();
    let rest = line[split..].trim_start();
    let rest = rest.strip_prefix('=').unwrap_or(rest).trim_start();
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut any = false;
    for c in rest.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            '#' if !quoted && current.is_empty() => break,
            c if c.is_whitespace() && !quoted => {
                if any || !current.is_empty() {
                    words.push(std::mem::take(&mut current));
                    any = false;
                }
            }
            c => current.push(c),
        }
    }
    if any || !current.is_empty() {
        words.push(current);
    }
    Some((keyword, words))
}

/// A `Host` pattern that names one machine the app may pass to ssh: no wildcard or negation, and a valid alias.
fn plain_alias(pattern: &str) -> Option<String> {
    if pattern.contains(['*', '?', '!']) {
        return None;
    }
    match Target::parse(pattern) {
        Ok(Target::Alias(alias)) => Some(alias),
        _ => None,
    }
}

/// An `Include` word's files: `~/` from home, a relative one under `~/.ssh`, `*` and `?` in its last part matched (sorted, as glob).
fn expand_include(word: &str, home: &Path, source: &dyn ConfigSource) -> Vec<PathBuf> {
    let path = if let Some(rest) = word.strip_prefix("~/") {
        home.join(rest)
    } else if Path::new(word).is_absolute() || word.starts_with('/') {
        PathBuf::from(word)
    } else {
        home.join(".ssh").join(word)
    };
    let path = lexical(&path);
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if !name.contains(['*', '?']) {
        return vec![path];
    }
    let Some(directory) = path.parent() else { return vec![] };
    let mut files: Vec<PathBuf> =
        source.list(directory).into_iter().filter(|file| file.file_name().is_some_and(|n| glob(&name, &n.to_string_lossy()))).collect();
    files.sort();
    files
}

/// `a/b/../c` as `a/c`, so a file reached two ways is read once.
fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// `*` and `?` against a file name (a leading `.` is matched only by a leading `.`, as glob does).
fn glob(pattern: &str, name: &str) -> bool {
    if name.starts_with('.') && !pattern.starts_with('.') {
        return false;
    }
    fn matches(p: &[char], n: &[char]) -> bool {
        match (p.first(), n.first()) {
            (None, None) => true,
            (Some('*'), _) => matches(&p[1..], n) || (!n.is_empty() && matches(p, &n[1..])),
            (Some('?'), Some(_)) => matches(&p[1..], &n[1..]),
            (Some(a), Some(b)) if a == b => matches(&p[1..], &n[1..]),
            _ => false,
        }
    }
    let (p, n): (Vec<char>, Vec<char>) = (pattern.chars().collect(), name.chars().collect());
    matches(&p, &n)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    struct Fixture(HashMap<PathBuf, String>);

    impl ConfigSource for Fixture {
        fn read(&self, path: &Path) -> Option<String> {
            self.0.get(path).cloned()
        }
        fn list(&self, directory: &Path) -> Vec<PathBuf> {
            self.0.keys().filter(|p| p.parent() == Some(directory)).cloned().collect()
        }
    }

    fn fixture(files: &[(&str, &str)]) -> Fixture {
        Fixture(files.iter().map(|(path, text)| (PathBuf::from(path), text.to_string())).collect())
    }

    #[test]
    fn host_lines_give_plain_aliases_only() {
        let source = fixture(&[(
            "/home/me/.ssh/config",
            "# work\nHost devbox build-1 *.corp !bastion\n  HostName 10.0.0.2\n  ProxyCommand nc %h %p\nHost=gpu  \"quoted box\"\n\
             host  web?.example\nHOST staging # trailing comment\nHost -oProxyCommand=x\nHost devbox\n",
        )]);
        let aliases = host_aliases(Path::new("/home/me/.ssh/config"), Path::new("/home/me"), &source);
        assert_eq!(aliases, ["devbox", "build-1", "gpu", "staging"]);
    }

    #[test]
    fn includes_are_followed_and_match_blocks_left_out() {
        let source = fixture(&[
            (
                "/home/me/.ssh/config",
                "Include config.d/*\nInclude ~/extra.conf /etc/ssh/missing\nHost main\nMatch host x\n  Include hidden\nHost after-match\n",
            ),
            ("/home/me/.ssh/config.d/10-work", "Host work-a work-b\n"),
            ("/home/me/.ssh/config.d/20-home", "Host pi\nInclude config.d/../config\n"),
            ("/home/me/.ssh/config.d/.hidden", "Host secret-dot\n"),
            ("/home/me/extra.conf", "Host extra\n"),
            ("/home/me/.ssh/hidden", "Host from-match\n"),
        ]);
        let aliases = host_aliases(Path::new("/home/me/.ssh/config"), Path::new("/home/me"), &source);
        assert_eq!(aliases, ["work-a", "work-b", "pi", "extra", "main", "after-match"]);
    }

    #[test]
    fn a_missing_config_has_no_aliases() {
        let source = fixture(&[]);
        assert!(host_aliases(Path::new("/nowhere/.ssh/config"), Path::new("/nowhere"), &source).is_empty());
    }

    #[test]
    fn globs_match_like_the_shell() {
        assert!(glob("*", "a"));
        assert!(glob("*.conf", "x.conf"));
        assert!(!glob("*.conf", "x.conf.bak"));
        assert!(glob("1?", "10"));
        assert!(!glob("*", ".dot"));
        assert!(glob(".*", ".dot"));
    }
}
