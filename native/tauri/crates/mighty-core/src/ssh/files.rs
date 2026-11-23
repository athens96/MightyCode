//! File access on an SSH machine: the remote commands of a folder listing, a file read and the folder's git changes, each capped on
//! the far side (`head`), and their output read; and (MC-40) the editor's open, check and atomic write. Paths are relative to the
//! workspace's remote folder, never `..`, and always passed as `./<quoted>` so a name can never be an option. Every command is a
//! POSIX script under `/bin/sh -c` ([`sh`]), whatever the far side's login shell is.

use super::command::{quote, quote_dir, sh};

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
        .then(|| sh(&format!("cd {} && LC_ALL=C ls -1Apq {} | head -n {}", quote_dir(dir), under(relative), MAXIMUM_ENTRIES + 1)))
}

/// A regular file's first bytes, one more than the cap so a longer file is known to be cut.
pub fn read_command(dir: &str, relative: &str) -> Option<String> {
    (valid_relative(relative) && !relative.is_empty()).then(|| {
        let path = under(relative);
        sh(&format!("cd {} && test -f {path} && head -c {} {path}", quote_dir(dir), MAXIMUM_READ_BYTES + 1))
    })
}

/// The folder's uncommitted changes against `HEAD` as a patch, capped.
pub fn diff_command(dir: &str) -> String {
    sh(&format!("git -C {} diff --no-color --no-ext-diff HEAD | head -c {}", quote_dir(dir), MAXIMUM_DIFF_BYTES + 1))
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

// ── editing (MC-40) ──────────────────────────────────────────────────────────────────────────────────────────────────────────

/// The most bytes a file the editor opens or saves may have (the files pane's text cap).
pub const MAXIMUM_EDIT_BYTES: usize = 1024 * 1024;

/// The lines every editing script starts with: into the folder (exit 3 when it is gone), the file as `$p`, refused with exit 4 when
/// it is not a regular file or is a link.
fn file_prelude(dir: &str, relative: &str) -> String {
    format!(
        "umask 077\ncd {} || exit 3\np={}\nif [ -h \"$p\" ] || [ ! -f \"$p\" ]; then exit 4; fi\n\
         ck() {{ set -- $(LC_ALL=C cksum); printf '%s %s' \"$1\" \"$2\"; }}",
        quote_dir(dir),
        under(relative)
    )
}

/// A file opened for editing: its type and permissions (`ls -ldn`'s first ten characters) on the first line, then its first bytes
/// (one more than [`MAXIMUM_EDIT_BYTES`], so a longer file is known to be cut). Exit 4 when it is not a regular file.
pub fn open_command(dir: &str, relative: &str) -> Option<String> {
    (valid_relative(relative) && !relative.is_empty()).then(|| {
        sh(&format!("{}\nLC_ALL=C ls -ldn \"$p\" | cut -c 1-10\nhead -c {} \"$p\"", file_prelude(dir, relative), MAXIMUM_EDIT_BYTES + 1))
    })
}

/// The file's POSIX `cksum` (its CRC and size) only: whether it still is what the editor opened, without reading it here.
pub fn check_command(dir: &str, relative: &str) -> Option<String> {
    (valid_relative(relative) && !relative.is_empty()).then(|| sh(&format!("{}\nck < \"$p\"", file_prelude(dir, relative))))
}

/// What [`open_command`] read: the file's permission bits (setuid, setgid and sticky left out) and its bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Opened {
    pub mode: u32,
    pub bytes: Vec<u8>,
    /// The file is longer than [`MAXIMUM_EDIT_BYTES`] (and `bytes` were cut there).
    pub longer: bool,
}

/// [`open_command`]'s output read; None when its first line is not a regular file's.
pub fn parse_opened(mut stdout: Vec<u8>) -> Option<Opened> {
    let end = stdout.iter().position(|b| *b == b'\n')?;
    let line = std::str::from_utf8(&stdout[..end]).ok()?.trim_end_matches('\r').to_string();
    let mode = mode_bits(&line)?;
    let bytes = stdout.split_off(end + 1);
    let (bytes, longer) = cut(bytes, MAXIMUM_EDIT_BYTES);
    Some(Opened { mode, bytes, longer })
}

/// `ls -l`'s type and permissions (`-rw-r--r--`) as permission bits (`0o644`); None for anything but a regular file. Setuid, setgid
/// and sticky are not carried to a saved file.
pub fn mode_bits(text: &str) -> Option<u32> {
    let chars: Vec<char> = text.chars().take(10).collect();
    if chars.len() != 10 || chars[0] != '-' {
        return None;
    }
    let mut mode = 0;
    for (index, c) in chars[1..].iter().enumerate() {
        let bit = 1 << (8 - index);
        let set = match index % 3 {
            0 => *c == 'r',
            1 => *c == 'w',
            _ => matches!(c, 'x' | 's' | 't'),
        };
        let known = match index % 3 {
            0 => matches!(c, 'r' | '-'),
            1 => matches!(c, 'w' | '-'),
            _ => matches!(c, 'x' | 's' | 't' | 'S' | 'T' | '-'),
        };
        if !known {
            return None;
        }
        if set {
            mode |= bit;
        }
    }
    Some(mode)
}

/// POSIX `cksum`'s CRC (polynomial 0x04C11DB7, the length appended, the result inverted) and the byte count, as `cksum` prints them:
/// what the far side's `cksum` says of these bytes.
pub fn cksum(bytes: &[u8]) -> (u32, u64) {
    fn step(crc: u32, byte: u8) -> u32 {
        let mut crc = crc ^ (u32::from(byte) << 24);
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 { (crc << 1) ^ 0x04C1_1DB7 } else { crc << 1 };
        }
        crc
    }
    let mut crc = bytes.iter().fold(0u32, |crc, b| step(crc, *b));
    let mut length = bytes.len() as u64;
    while length > 0 {
        crc = step(crc, (length & 0xff) as u8);
        length >>= 8;
    }
    (!crc, bytes.len() as u64)
}

/// [`check_command`]'s output read: the CRC and size.
pub fn parse_check(stdout: &str) -> Option<(u32, u64)> {
    let mut words = stdout.split_whitespace();
    let crc = words.next()?.parse().ok()?;
    let size = words.next()?.parse().ok()?;
    Some((crc, size))
}

/// One atomic write: the new bytes come through ssh's standard input into a new file beside the target (`.<name>.<tag>.edit.tmp`,
/// made with `set -C`, so never an existing file or link), are checked to have arrived whole (`cksum`), get the target's permission
/// bits, and replace it with `mv` (one rename in the same folder) — only while the target still is what `expected` says (its
/// `cksum`), else nothing is written (exit 7). The temporary file goes on any failure or a lost connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Write {
    pub relative: String,
    /// 32 hex digits, new for every write.
    pub tag: String,
    pub mode: u32,
    /// The target's `cksum` as it was read; None writes whatever it is now.
    pub expected: Option<(u32, u64)>,
    /// The new bytes' `cksum`.
    pub written: (u32, u64),
}

/// [`Write`]'s script (under [`sh`]); None for a path or tag a write may not name.
pub fn write_command(dir: &str, write: &Write) -> Option<String> {
    if !valid_relative(&write.relative)
        || write.relative.is_empty()
        || write.tag.len() != 32
        || !write.tag.bytes().all(|b| b.is_ascii_hexdigit())
        || write.mode > 0o777
    {
        return None;
    }
    let (parent, name) = match write.relative.rsplit_once('/') {
        Some((parent, name)) => (format!("{parent}/"), name),
        None => (String::new(), write.relative.as_str()),
    };
    let temporary = format!("{parent}.{name}.{}.edit.tmp", write.tag);
    let expected = match write.expected {
        Some((crc, size)) => format!("[ \"$(ck < \"$p\")\" = '{crc} {size}' ] || exit 7\n"),
        None => String::new(),
    };
    let (crc, size) = write.written;
    Some(sh(&format!(
        "{prelude}\nt={temporary}\ntrap 'rm -f \"$t\"' EXIT\ntrap 'exit 1' HUP INT TERM PIPE\n\
         set -C\n: > \"$t\" || exit 5\nset +C\ncat >| \"$t\" || exit 5\n\
         [ \"$(ck < \"$t\")\" = '{crc} {size}' ] || exit 6\nchmod {mode:o} \"$t\" || exit 5\n\
         {expected}mv -f \"$t\" \"$p\" || exit 5\ntrap - EXIT\nprintf saved",
        prelude = file_prelude(dir, &write.relative),
        temporary = under(&temporary),
        mode = write.mode,
    )))
}

/// How a write or an editing read ended on the far side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ended {
    Done,
    /// The workspace's folder is gone (3).
    FolderGone,
    /// Not a regular file, or a link (4).
    NotAFile,
    /// The bytes did not arrive whole (6).
    Incomplete,
    /// The file changed since it was read (7).
    Changed,
    /// Anything else the script refused (a temporary file it could not make, a rename that failed).
    Failed(i32),
}

impl Ended {
    pub fn of(code: i32) -> Ended {
        match code {
            0 => Ended::Done,
            3 => Ended::FolderGone,
            4 => Ended::NotAFile,
            6 => Ended::Incomplete,
            7 => Ended::Changed,
            other => Ended::Failed(other),
        }
    }
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
        let script = |command: Option<String>| super::super::command::unwrap_sh(&command.unwrap()).unwrap();
        assert_eq!(script(list_command("~/app", "")), "cd ~/'app' && LC_ALL=C ls -1Apq '.' | head -n 5001");
        assert_eq!(script(list_command("/srv/a b", "-rf dir")), "cd '/srv/a b' && LC_ALL=C ls -1Apq ./'-rf dir' | head -n 5001");
        assert_eq!(script(read_command("/srv", "it's.txt")), "cd '/srv' && test -f ./'it'\\''s.txt' && head -c 1048577 ./'it'\\''s.txt'");
        assert_eq!(read_command("/srv", ""), None);
        assert_eq!(list_command("/srv", "../x"), None);
        assert_eq!(script(Some(diff_command("/srv/app"))), "git -C '/srv/app' diff --no-color --no-ext-diff HEAD | head -c 2097153");
        assert_eq!(open_command("/srv", "../x"), None);
        assert_eq!(check_command("/srv", ""), None);
        let write = |relative: &str, tag: &str, mode: u32| {
            write_command("/srv", &Write { relative: relative.into(), tag: tag.into(), mode, expected: None, written: (1, 1) })
        };
        assert!(write("a.txt", &"a".repeat(32), 0o644).is_some());
        assert_eq!(write("a.txt", "x'; rm -rf /", 0o644), None);
        assert_eq!(write("a.txt", &"a".repeat(32), 0o4755), None);
        assert_eq!(write("../a.txt", &"a".repeat(32), 0o644), None);
    }

    #[test]
    fn permissions_are_read_from_ls() {
        assert_eq!(mode_bits("-rw-r--r--"), Some(0o644));
        assert_eq!(mode_bits("-rwxr-x---"), Some(0o750));
        assert_eq!(mode_bits("-rwsr-xr-t"), Some(0o755));
        assert_eq!(mode_bits("-rwSr--r-T"), Some(0o644));
        assert_eq!(mode_bits("lrwxrwxrwx"), None);
        assert_eq!(mode_bits("drwxr-xr-x"), None);
        assert_eq!(mode_bits("-rw"), None);
        assert_eq!(parse_check("4038471504 6\n"), Some((4038471504, 6)));
        let opened = parse_opened(b"-rw-------\nhello\nworld".to_vec()).unwrap();
        assert_eq!(opened, Opened { mode: 0o600, bytes: b"hello\nworld".to_vec(), longer: false });
        assert_eq!(parse_opened(b"drwx------\n".to_vec()), None);
        assert_eq!(Ended::of(7), Ended::Changed);
    }

    /// The CRC is the far side's `cksum`'s: a file the editor read is known again by it without reading it here.
    #[cfg(unix)]
    #[test]
    fn the_crc_is_posix_cksums() {
        let long: Vec<u8> = (0..300_000u32).map(|i| (i * 7 % 251) as u8).collect();
        for bytes in [&b""[..], b"a", b"hello\n", "한글 텍스트\r\n".as_bytes(), &long] {
            #[allow(clippy::disallowed_methods, reason = "a test compares with the system's cksum")]
            let mut child = std::process::Command::new("/bin/sh")
                .arg("-c")
                .arg("LC_ALL=C cksum")
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            use std::io::Write as _;
            child.stdin.take().unwrap().write_all(bytes).unwrap();
            let output = child.wait_with_output().unwrap();
            assert_eq!(parse_check(&String::from_utf8_lossy(&output.stdout)), Some(cksum(bytes)), "{} bytes", bytes.len());
        }
    }

    /// The editing scripts through a real `/bin/sh`, in a folder whose name needs quoting: the open reads permissions and bytes, a
    /// write lands whole with the permissions kept, a changed file or bytes that did not arrive whole are left as they were, and no
    /// temporary file stays.
    #[cfg(unix)]
    #[test]
    fn the_editing_scripts_write_atomically_in_a_real_shell() {
        use std::os::unix::fs::PermissionsExt as _;
        #[allow(clippy::disallowed_methods, reason = "a test runs the far side's scripts in a real shell")]
        fn run(command: &str, input: &[u8]) -> (i32, Vec<u8>) {
            use std::io::Write as _;
            let mut child = std::process::Command::new("/bin/sh")
                .arg("-c")
                .arg(command)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            let mut stdin = child.stdin.take().unwrap();
            let _ = stdin.write_all(input);
            drop(stdin);
            let output = child.wait_with_output().unwrap();
            (output.status.code().unwrap_or(-1), output.stdout)
        }
        let root = std::env::temp_dir().join(format!("mc-ssh-edit-{}", uuid::Uuid::new_v4().simple()));
        let dir = root.join("it's \"my\" app");
        std::fs::create_dir_all(dir.join("src dir")).unwrap();
        let file = dir.join("src dir/a b.txt");
        std::fs::write(&file, b"one\r\ntwo\r\n").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o751)).unwrap();
        let folder = dir.to_string_lossy().into_owned();
        let relative = "src dir/a b.txt";

        let (code, out) = run(&open_command(&folder, relative).unwrap(), b"");
        assert_eq!(code, 0);
        let opened = parse_opened(out).unwrap();
        assert_eq!((opened.mode, opened.bytes.as_slice(), opened.longer), (0o751, &b"one\r\ntwo\r\n"[..], false));
        let (code, out) = run(&check_command(&folder, relative).unwrap(), b"");
        assert_eq!((code, parse_check(&String::from_utf8_lossy(&out))), (0, Some(cksum(b"one\r\ntwo\r\n"))));

        let tag = |n: u8| format!("{n:0>32}");
        let new = b"uno\r\ndos\r\n";
        let write =
            Write { relative: relative.into(), tag: tag(1), mode: opened.mode, expected: Some(cksum(&opened.bytes)), written: cksum(new) };
        let (code, out) = run(&write_command(&folder, &write).unwrap(), new);
        assert_eq!((code, out.as_slice()), (0, &b"saved"[..]));
        assert_eq!(std::fs::read(&file).unwrap(), new);
        assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o751);

        // The file changed since it was read: nothing is written.
        let stale = Write { tag: tag(2), written: cksum(b"x"), ..write.clone() };
        assert_eq!(Ended::of(run(&write_command(&folder, &stale).unwrap(), b"x").0), Ended::Changed);
        assert_eq!(std::fs::read(&file).unwrap(), new);
        // Bytes that did not arrive whole: nothing is written.
        let cut = Write { tag: tag(3), expected: Some(cksum(new)), written: cksum(b"complete"), ..write.clone() };
        assert_eq!(Ended::of(run(&write_command(&folder, &cut).unwrap(), b"compl").0), Ended::Incomplete);
        assert_eq!(std::fs::read(&file).unwrap(), new);
        // A link is never written through.
        let outside = root.join("outside.txt");
        std::fs::write(&outside, b"secret").unwrap();
        std::os::unix::fs::symlink(&outside, dir.join("link.txt")).unwrap();
        let link = Write { relative: "link.txt".into(), tag: tag(4), expected: None, written: cksum(b"x"), ..write.clone() };
        assert_eq!(Ended::of(run(&write_command(&folder, &link).unwrap(), b"x").0), Ended::NotAFile);
        assert_eq!(Ended::of(run(&open_command(&folder, "link.txt").unwrap(), b"").0), Ended::NotAFile);
        assert_eq!(std::fs::read(&outside).unwrap(), b"secret");
        // A folder that is gone.
        assert_eq!(Ended::of(run(&open_command(&root.join("gone").to_string_lossy(), "a").unwrap(), b"").0), Ended::FolderGone);

        let left: Vec<String> = std::fs::read_dir(dir.join("src dir"))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".edit.tmp"))
            .collect();
        assert!(left.is_empty(), "no temporary file stays: {left:?}");
        std::fs::remove_dir_all(&root).unwrap();
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
