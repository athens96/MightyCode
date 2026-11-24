//! MC-47: pane worktrees and the work map of an SSH workspace, on the far side. The pure parts: the remote scripts (each a POSIX
//! script for [`sh`]) and the reading of their output. Nothing here runs anything; mighty-run `ssh_git` sends them, mighty-delegation
//! `machine::Remote` and mighty-workmap's remote repository use them, so the worktree rules stay the local code's.
//!
//! - [`git_script`]: one git as the app runs it here (mighty-run `git`): no program a repository's config could start
//!   (`core.fsmonitor=false`, `core.hooksPath=/dev/null`, the signature programs `/dev/null`, no automatic maintenance), no inherited
//!   `GIT_*` variable, no prompt, `LC_ALL=C`, and no transport (`protocol.allow=never`, `GIT_NO_LAZY_FETCH`) for
//!   everything but a push. A deadline of its own on the far side ends git even when this computer's ssh was cut.
//! - [`probe_script`], [`read_inside_script`], [`mkdir_script`], [`clear_links_script`], [`unlink_script`], [`remove_script`],
//!   [`free_script`]: the file system questions and removals the worktree code asks of a folder (mighty-delegation `Machine`).
//! - [`copy_script`], [`share_script`], [`exclude_script`]: a new worktree's copied files and shared folders, with the same refusals as
//!   the local copy (`missing`, `link`, `outside`, `exists`, `limit`, `failed`; `notDirectory`, `notIgnored`).
//! - [`listening_script`]: the ports something listens on over there (a remote port block skips them).
//! - [`command_script`]: a worktree's setup or teardown command, run as the local one is (`/bin/sh -c`, `-ec` for several lines) with
//!   the login shell's `PATH`, in the worktree, with the worktree's values.
//!
//! Paths on the far side are POSIX whatever this computer is ([`posix`]): a Windows app never reads them with its own path rules.

use std::collections::BTreeSet;

use super::command::{login_exec, quote, quote_dir, valid_env_name};
use super::valid_remote_dir;

/// A git that wrote more than this (standard output and error together) gives no result, as here (mighty-run `git::MAXIMUM_BYTES`).
pub const MAXIMUM_GIT_BYTES: usize = 1_048_576;
/// The folder git was to run in is gone (no git started).
pub const FOLDER_GONE: i32 = 250;
/// There is no git on the far side's `PATH`.
pub const NO_GIT: i32 = 251;
/// The far side's deadline ended git.
pub const TIMED_OUT: i32 = 252;
/// The most paths one probe names (a longer list goes in several).
pub const PROBE_CHUNK: usize = 200;

/// POSIX paths of the far side, read the same on every OS this app runs on.
pub mod posix {
    /// Absolute (`/…`).
    pub fn absolute(path: &str) -> bool {
        path.starts_with('/')
    }

    /// `path` with `.`, `..` and repeated separators taken out and no closing separator (the root stays `/`); a relative path is
    /// returned as it is.
    pub fn standardized(path: &str) -> String {
        if !absolute(path) {
            return path.to_string();
        }
        let mut parts: Vec<&str> = vec![];
        for part in path.split('/') {
            match part {
                "" | "." => {}
                ".." => {
                    parts.pop();
                }
                other => parts.push(other),
            }
        }
        format!("/{}", parts.join("/"))
    }

    /// `a/b` (`Path.Combine` as mighty-run's `os::combine` does it, with `/`).
    pub fn combine(a: &str, b: &str) -> String {
        if a.is_empty() {
            b.to_string()
        } else if a.ends_with('/') {
            format!("{a}{b}")
        } else {
            format!("{a}/{b}")
        }
    }

    /// Below `folder` (not beside it, not itself).
    pub fn is_inside(path: &str, folder: &str) -> bool {
        if folder.ends_with('/') { path.starts_with(folder) && path.len() > folder.len() } else { path.starts_with(&format!("{folder}/")) }
    }

    /// The last name.
    pub fn file_name(path: &str) -> Option<&str> {
        let trimmed = path.trim_end_matches('/');
        trimmed.rsplit('/').next().filter(|name| !name.is_empty() && *name != "." && *name != "..")
    }

    /// The folder holding `path`.
    pub fn parent(path: &str) -> Option<String> {
        let standard = standardized(path);
        let (head, _) = standard.rsplit_once('/')?;
        if standard == "/" {
            return None;
        }
        Some(if head.is_empty() { "/".to_string() } else { head.to_string() })
    }
}

/// Which settings a remote git takes: delegation's (a worktree's git), a push (delegation's with a transport), or the work map's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitKind {
    Delegation,
    Push,
    WorkMap,
}

/// The `-c` settings before the command (mighty-run `git::delegation_settings` and the work map's, with `/dev/null` as the hooks
/// folder and every signature program, as the Mac's runner has them), plus `protocol.allow=never` for all but a push.
fn settings(kind: GitKind) -> Vec<&'static str> {
    let mut list = vec!["core.fsmonitor=false", "core.hooksPath=/dev/null", "gc.auto=0"];
    if kind != GitKind::WorkMap {
        list.push("maintenance.auto=false");
    }
    list.extend(["log.showSignature=false", "gpg.program=/dev/null", "gpg.ssh.program=/dev/null", "gpg.x509.program=/dev/null"]);
    if kind != GitKind::Push {
        list.push("protocol.allow=never");
    }
    list
}

/// The far side's environment for git: every inherited `GIT_*` variable taken out, then the app's own.
fn git_environment(kind: GitKind) -> String {
    let lazy = if kind == GitKind::Push { "" } else { " GIT_NO_LAZY_FETCH=1" };
    format!(
        "for v in $(env | sed -n 's/^\\(GIT_[A-Za-z0-9_]*\\)=.*/\\1/p'); do unset \"$v\"; done
export GIT_OPTIONAL_LOCKS=0 GIT_TERMINAL_PROMPT=0 LC_ALL=C NO_COLOR=1{lazy}"
    )
}

/// The function `G` that runs git with `kind`'s settings (for the scripts that ask git a question of their own).
fn git_function(kind: GitKind) -> String {
    let settings: Vec<String> = settings(kind).iter().map(|s| format!("-c {s}")).collect();
    format!("G() {{ git {} \"$@\"; }}", settings.join(" "))
}

/// One git in `dir` (an absolute folder or one under the far side's home) with `kind`'s settings and `args`, ended by the far side
/// after `seconds`. Its standard input is this script's. Exit codes: git's own (one of 250 and above reads 249), [`FOLDER_GONE`],
/// [`NO_GIT`], [`TIMED_OUT`] (the deadline's `KILL`, as here the runner kills git; a git killed otherwise reads the same). None for a folder or an argument the far side may not be given.
pub fn git_script(dir: &str, kind: GitKind, args: &[&str], seconds: u64) -> Option<String> {
    if !valid_remote_dir(dir) || args.iter().any(|a| a.contains('\0')) {
        return None;
    }
    let settings: Vec<String> = settings(kind).iter().map(|s| format!("-c {}", quote(s))).collect();
    let args: Vec<String> = args.iter().map(|a| quote(a)).collect();
    Some(format!(
        "cd {dir} 2>/dev/null || exit {FOLDER_GONE}
command -v git >/dev/null 2>&1 || exit {NO_GIT}
{environment}
exec 3<&0 </dev/null
git {settings} {args} <&3 3<&- &
p=$!
exec 3<&-
( sleep {seconds}; kill -KILL $p ) >/dev/null 2>&1 &
w=$!
wait $p
c=$?
kill $w 2>/dev/null
if [ $c = 137 ]; then c={TIMED_OUT}; elif [ $c -ge {FOLDER_GONE} ]; then c=249; fi
exit $c",
        dir = quote_dir(dir),
        environment = git_environment(kind),
        settings = settings.join(" "),
        args = args.join(" "),
        seconds = seconds.max(1),
    ))
}

// ── the machine ─────────────────────────────────────────────────────────────────────────────────────────────────────────

/// What the far side says of itself: its home's real path and whether it has git.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Info {
    pub home: String,
    pub git: bool,
}

/// The home's real path (`pwd -P`) and whether git is on the `PATH`.
pub fn info_script() -> String {
    "cd ~ 2>/dev/null || cd \"$HOME\" || exit 3
printf 'home=%s\\n' \"$(pwd -P)\"
if command -v git >/dev/null 2>&1; then printf 'git=1\\n'; else printf 'git=0\\n'; fi
printf 'done=1\\n'"
        .to_string()
}

/// [`info_script`]'s answer; None without its last line or with a home that is no absolute path.
pub fn parse_info(stdout: &str) -> Option<Info> {
    let mut home = None;
    let mut git = None;
    let mut done = false;
    for line in stdout.lines() {
        if let Some(rest) = line.strip_prefix("home=") {
            home = Some(rest.to_string());
        } else if let Some(rest) = line.strip_prefix("git=") {
            git = Some(rest == "1");
        } else if line == "done=1" {
            done = true;
        }
    }
    let home = home.filter(|h| posix::absolute(h) && !h.chars().any(char::is_control))?;
    (done).then_some(Info { home, git: git? })
}

/// What a probe read of one path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Probe {
    /// `-h`: a symbolic link (itself).
    pub link: bool,
    /// `-d`: a folder, a link to one too.
    pub folder: bool,
    /// `-e`: there (a link to something there too).
    pub exists: bool,
    /// A plain file's size (not a link); None for anything else or an unreadable one.
    pub size: Option<u64>,
}

/// A path the probe and the removals may name: absolute, no control character.
pub fn valid_path(path: &str) -> bool {
    posix::absolute(path) && path.encode_utf16().count() <= 4096 && !path.chars().any(char::is_control)
}

/// Each of `paths` read: a line `<link> <folder> <exists> <size or ->`. None when a path is not one it may name or there are more
/// than [`PROBE_CHUNK`].
pub fn probe_script(paths: &[String]) -> Option<String> {
    if paths.len() > PROBE_CHUNK || !paths.iter().all(|p| valid_path(p)) {
        return None;
    }
    let words: Vec<String> = paths.iter().map(|p| quote(p)).collect();
    Some(format!(
        "for p in {}; do
h=0; d=0; e=0; s=-
[ -h \"$p\" ] && h=1
[ -d \"$p\" ] && d=1
[ -e \"$p\" ] && e=1
if [ $h = 0 ] && [ -f \"$p\" ]; then s=$(wc -c < \"$p\" 2>/dev/null | tr -d ' '); fi
printf '%s %s %s %s\\n' $h $d $e \"${{s:--}}\"
done",
        words.join(" ")
    ))
}

/// [`probe_script`]'s answer for `count` paths.
pub fn parse_probe(stdout: &str, count: usize) -> Option<Vec<Probe>> {
    let probes: Vec<Probe> = stdout
        .lines()
        .map(|line| {
            let fields: Vec<&str> = line.split(' ').collect();
            let flag = |at: usize| fields.get(at).is_some_and(|f| *f == "1");
            Probe { link: flag(0), folder: flag(1), exists: flag(2), size: fields.get(3).and_then(|s| s.parse().ok()) }
        })
        .collect();
    (probes.len() == count).then_some(probes)
}

/// The folder `path` with every folder on the way made (`mkdir -p`).
pub fn mkdir_script(path: &str) -> Option<String> {
    valid_path(path).then(|| format!("mkdir -p -- {}", quote(path)))
}

/// The free bytes of the volume holding `path` (or its nearest folder that is there): `df -Pk`'s available 1024-blocks.
pub fn free_script(path: &str) -> Option<String> {
    valid_path(path).then(|| {
        format!(
            "p={}
while [ ! -d \"$p\" ]; do p=$(dirname -- \"$p\"); done
df -Pk -- \"$p\" 2>/dev/null | awk 'NR==2 {{print $4}}'",
            quote(path)
        )
    })
}

/// [`free_script`]'s answer in bytes.
pub fn parse_free(stdout: &str) -> Option<u64> {
    stdout.trim().parse::<u64>().ok().map(|blocks| blocks.saturating_mul(1024))
}

/// The file `relative` inside `root` (a plain file, no link, reached through no link out of `root`, at most `limit` bytes) written
/// out. Exit 3 when `root` is gone, 4 when it is no plain file, 5 when it lies outside, 6 when it is larger.
pub fn read_inside_script(root: &str, relative: &str, limit: u64) -> Option<String> {
    if !valid_path(root) || !super::files::valid_relative(relative) || relative.is_empty() {
        return None;
    }
    Some(format!(
        "cd -- {root} 2>/dev/null || exit 3
r=$(pwd -P)
p=./{path}
if [ -h \"$p\" ] || [ ! -f \"$p\" ]; then exit 4; fi
q=$(cd -P -- \"$(dirname -- \"$p\")\" 2>/dev/null && pwd -P) || exit 4
case \"$q/\" in \"$r\"/*) ;; *) exit 5 ;; esac
n=$(wc -c < \"$p\" | tr -d ' ')
[ \"$n\" -le {limit} ] || exit 6
cat -- \"$p\"",
        root = quote(root),
        path = quote(relative),
    ))
}

/// The first `limit` bytes of the file `relative` inside `root` (the work map's `VERSION` and release files): a plain file, no link,
/// reached through no link out of `root`. Exit 3 when `root` is gone, 4 when it is no plain file, 5 when it lies outside.
pub fn read_first_script(root: &str, relative: &str, limit: usize) -> Option<String> {
    if !valid_remote_dir(root) || !super::files::valid_relative(relative) || relative.is_empty() || limit == 0 {
        return None;
    }
    Some(format!(
        "cd {root} 2>/dev/null || exit 3
r=$(pwd -P)
p=./{path}
if [ -h \"$p\" ] || [ ! -f \"$p\" ]; then exit 4; fi
q=$(cd -P -- \"$(dirname -- \"$p\")\" 2>/dev/null && pwd -P) || exit 4
case \"$q/\" in \"$r\"/*) ;; *) exit 5 ;; esac
dd if=\"$p\" bs={limit} count=1 2>/dev/null",
        root = quote_dir(root),
        path = quote(relative),
    ))
}

/// The files `relative` of `dir` as they are now, written as mighty-core `ssh::review::read_script` writes a current file (a line `N`,
/// `L <mode>` or `B <mode> <size>`, then that many bytes; at most [`super::files::MAXIMUM_EDIT_BYTES`]), with no snapshot store (a
/// worktree's hunk review, MC-47). Exit 3 when the folder is gone.
pub fn current_script(dir: &str, relative: &[String]) -> Option<String> {
    if !valid_remote_dir(dir) || relative.iter().any(|r| r.is_empty() || !super::files::valid_relative(r)) {
        return None;
    }
    let lines: Vec<String> = relative.iter().map(|r| format!("file ./{}", quote(r))).collect();
    Some(super::command::sh(&format!(
        "cd {dir} 2>/dev/null || exit 3
file() {{ if [ -h \"$1\" ] || [ ! -f \"$1\" ]; then printf 'N\\n'; return; fi
mode=$(LC_ALL=C ls -ldn \"$1\" | cut -c 1-10)
s=$(wc -c < \"$1\" | tr -d ' ')
if [ \"$s\" -gt {cap} ]; then printf 'L %s\\n' \"$mode\"; return; fi
printf 'B %s %s\\n' \"$mode\" \"$s\"
if [ \"$s\" -gt 0 ]; then dd if=\"$1\" bs=\"$s\" count=1 2>/dev/null; fi; }}
{lines}",
        dir = quote_dir(dir),
        cap = super::files::MAXIMUM_EDIT_BYTES,
        lines = lines.join("\n"),
    )))
}

/// The directory links inside `folder` removed (never what they point to; a link to a file stays, as here: git never descends one),
/// then `1` when the folder itself is a link or a directory link is still inside (one made meanwhile), else `0`. Exit 5 when a
/// folder in it cannot be read.
pub fn clear_links_script(folder: &str) -> Option<String> {
    valid_path(folder).then(|| {
        format!(
            "f={}
[ -h \"$f\" ] && {{ echo 1; exit 0; }}
[ -d \"$f\" ] || {{ echo 0; exit 0; }}
find \"$f\" -type l -exec sh -c 'for l; do if [ -d \"$l\" ]; then rm -f -- \"$l\" || exit 1; fi; done' sh {{}} + || exit 5
[ -h \"$f\" ] && {{ echo 1; exit 0; }}
left=$(find \"$f\" -type l -exec sh -c 'for l; do [ -d \"$l\" ] && echo x; done; :' sh {{}} +)
if [ -n \"$left\" ]; then echo 1; else echo 0; fi",
            quote(folder)
        )
    })
}

/// The link at `path` itself removed (nothing when it is no link).
pub fn unlink_script(path: &str) -> Option<String> {
    valid_path(path).then(|| format!("p={}\nif [ -h \"$p\" ]; then rm -f -- \"$p\" || exit 5; fi", quote(path)))
}

/// What is at `path` removed with everything in it, a link itself and never followed; folders without the owner's write bit are
/// given it first (as here, where read-only entries are made writable). Exit 5 when it could not all go.
pub fn remove_script(path: &str) -> Option<String> {
    valid_path(path).then(|| {
        format!(
            "p={}
if [ -h \"$p\" ]; then rm -f -- \"$p\" || exit 5; exit 0; fi
[ -e \"$p\" ] || exit 0
[ -d \"$p\" ] && find \"$p\" -type d -exec chmod u+w {{}} + 2>/dev/null
rm -rf -- \"$p\" || exit 5",
            quote(path)
        )
    })
}

// ── copied files and shared folders ─────────────────────────────────────────────────────────────────────────────────────

/// The most bytes and files a worktree's copies take (mighty-delegation `pane_worktree::COPY_MAXIMUM_*`).
pub const COPY_MAXIMUM_BYTES: u64 = 50_000_000;
pub const COPY_MAXIMUM_FILES: u64 = 2_000;

/// One copy: from `from` under the source folder to `to` under the destination (both normalized relative paths).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyEntry {
    pub from: String,
    pub to: String,
}

/// The helpers the copy and share scripts share: `safe_dir <relative>` makes the folders of a relative path under `$D` that are
/// missing and fails when one on the way is a link or no folder (so nothing is ever written through a link out of the worktree);
/// `real <path>` the real path of a folder or a file's folder.
const HELPERS: &str = "safe_dir() {
[ \"$dl\" = 0 ] || return 1
c=\"$D\"; rest=\"$1\"
while [ -n \"$rest\" ]; do
part=${rest%%/*}
if [ \"$part\" = \"$rest\" ]; then rest=; else rest=${rest#*/}; fi
case \"$part\" in ''|.) continue ;; ..) return 1 ;; esac
c=\"$c/$part\"
[ -h \"$c\" ] && return 1
if [ ! -e \"$c\" ]; then mkdir -- \"$c\" 2>/dev/null || [ -d \"$c\" ] || return 1; fi
[ -d \"$c\" ] && [ ! -h \"$c\" ] || return 1
done
return 0
}
cpf() {
t=\"$D/$2\"
safe_dir \"$(dirname -- \"$2\")\" || return 1
if [ -e \"$t\" ] || [ -h \"$t\" ]; then return 1; fi
( set -C; : > \"$t\" ) 2>/dev/null || return 1
cat -- \"$1\" >> \"$t\" || { rm -f -- \"$t\"; return 1; }
}";

/// The opening of the copy and share scripts: `$S`/`$SR` the source folder and its real path, `$D`/`$DR` the destination's, `dl` 1
/// when the destination itself is a link (nothing is written then). Exit 3 when either is gone.
fn folders(source: &str, destination: &str) -> String {
    format!(
        "S={source}
D={destination}
SR=$(cd -P -- \"$S\" 2>/dev/null && pwd -P) || exit 3
[ -d \"$D\" ] || exit 3
dl=0; [ -h \"$D\" ] && dl=1
F='{helpers}'
eval \"$F\"
export D dl",
        source = quote(source),
        destination = quote(destination),
        helpers = HELPERS.replace('\'', "'\\''"),
    )
}

/// The first steps of one entry `$1` (its index), `$2` (from), `$3` (to): `missing`, `link`, `outside`, `exists`.
const ENTRY_CHECKS: &str = "i=$1; from=$2; to=$3; src=\"$S/$2\"; dst=\"$D/$3\"
if [ ! -e \"$src\" ] && [ ! -h \"$src\" ]; then printf 'S %s missing\\n' \"$i\"; return 0; fi
if [ -h \"$src\" ]; then printf 'S %s link\\n' \"$i\"; return 0; fi
if [ -d \"$src\" ]; then q=$(cd -P -- \"$src\" 2>/dev/null && pwd -P); else q=$(cd -P -- \"$(dirname -- \"$src\")\" 2>/dev/null && pwd -P); fi
case \"$q/\" in \"$SR\"/*) ;; *) printf 'S %s outside\\n' \"$i\"; return 0 ;; esac";

/// The worktree's copied files: each entry (in order) from `source` (the workspace folder) to `destination` (the pane's run folder):
/// plain files and folders inside the source only, never a link and nothing one leads to, never over a file the destination has, at
/// most [`COPY_MAXIMUM_BYTES`] in [`COPY_MAXIMUM_FILES`] files over all entries (the entry that passes it is `limit` and the rest are
/// not read). Prints `C <index>` or `S <index> <why>` per entry it read. None for a folder or entry it may not name.
pub fn copy_script(source: &str, destination: &str, entries: &[CopyEntry]) -> Option<String> {
    if !valid_path(source) || !valid_path(destination) {
        return None;
    }
    let mut calls = vec![];
    for (index, entry) in entries.iter().enumerate() {
        let fine = |p: &str| !p.is_empty() && super::files::valid_relative(p);
        if !fine(&entry.from) || !fine(&entry.to) {
            return None;
        }
        calls.push(format!("e {index} {} {} || exit 0", quote(&entry.from), quote(&entry.to)));
    }
    Some(format!(
        "{folders}
bytes=0; files=0
e() {{
{ENTRY_CHECKS}
if [ -e \"$dst\" ] || [ -h \"$dst\" ]; then printf 'S %s exists\\n' \"$i\"; return 0; fi
if [ -d \"$src\" ]; then
set -- $(find \"$src\" -type l -prune -o -type f -exec sh -c 'for f; do wc -c < \"$f\" 2>/dev/null || echo 0; done' sh {{}} + | awk '{{n++; s+=$1}} END {{printf \"%d %.0f\\n\", n, s}}')
n=${{1:-0}}; s=${{2:-0}}; folder=1
else
n=1; s=$(wc -c < \"$src\" | tr -d ' '); folder=0
fi
files=$((files + n)); bytes=$((bytes + s))
if [ $bytes -gt {bytes_max} ] || [ $files -gt {files_max} ]; then printf 'S %s limit\\n' \"$i\"; return 1; fi
if [ $folder = 1 ]; then
T=$to; export T
if safe_dir \"$to\" && (cd -- \"$src\" && find . -type l -prune -o -type f -exec sh -c \"$F\"'
ok=0; for f; do cpf \"$f\" \"$T/${{f#./}}\" || ok=1; done; exit $ok' sh {{}} +); then printf 'C %s\\n' \"$i\"; else printf 'S %s failed\\n' \"$i\"; fi
else
if cpf \"$src\" \"$to\"; then printf 'C %s\\n' \"$i\"; else printf 'S %s failed\\n' \"$i\"; fi
fi
return 0
}}
{calls}",
        folders = folders(source, destination),
        bytes_max = COPY_MAXIMUM_BYTES,
        files_max = COPY_MAXIMUM_FILES,
        calls = calls.join("\n"),
    ))
}

/// One shared folder's entry in the repository's `info/exclude`: its marker line and its pattern (mighty-delegation
/// `pane_worktree`'s `exclude_marker` and `escape_exclude_glob`, worked out by the caller).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exclude {
    pub marker: String,
    pub pattern: String,
}

/// The worktree's shared folders: each `path` of `source` (the workspace folder) the source repository already ignores, linked into
/// `destination` (a symbolic link to its real path) where nothing is, and its entry added to the worktree's and the common git
/// folder's `info/exclude` (`excludes[i]`, once). Prints `L <index>` or `S <index> <why>` (`missing`, `link`, `notDirectory`,
/// `outside`, `notIgnored`, `exists`, `failed`).
pub fn share_script(source: &str, destination: &str, top: &str, paths: &[String], excludes: &[Exclude]) -> Option<String> {
    if !valid_path(source) || !valid_path(destination) || !valid_path(top) || paths.len() != excludes.len() {
        return None;
    }
    let mut calls = vec![];
    for (index, (path, exclude)) in paths.iter().zip(excludes).enumerate() {
        if path.is_empty()
            || !super::files::valid_relative(path)
            || [&exclude.marker, &exclude.pattern].iter().any(|t| t.contains(['\n', '\r', '\0']))
        {
            return None;
        }
        calls.push(format!("e {index} {p} {p} {} {}", quote(&exclude.marker), quote(&exclude.pattern), p = quote(path)));
    }
    Some(format!(
        "{folders}
{git}
{environment}
admin() {{
a=$(cd -- {top} 2>/dev/null && G rev-parse --absolute-git-dir 2>/dev/null) || return 1
b=$(cd -- {top} 2>/dev/null && G rev-parse --git-common-dir 2>/dev/null) || return 1
b=$(cd -- {top} && cd -P -- \"$b\" 2>/dev/null && pwd -P) || return 1
}}
add() {{
[ -d \"$1\" ] && [ ! -h \"$1\" ] || return 0
if [ -h \"$1/info\" ]; then return 0; fi
[ -d \"$1/info\" ] || mkdir -- \"$1/info\" 2>/dev/null || return 0
x=\"$1/info/exclude\"
if [ -h \"$x\" ]; then return 0; fi
if [ -e \"$x\" ]; then [ -f \"$x\" ] || return 0; [ \"$(wc -c < \"$x\" | tr -d ' ')\" -le 1048576 ] || return 0; fi
if [ -f \"$x\" ] && M=\"$M\" P=\"$P\" awk 'BEGIN {{m=ENVIRON[\"M\"]; p=ENVIRON[\"P\"]}} prev==m && $0==p {{found=1}} {{prev=$0}} END {{exit !found}}' \"$x\"; then return 0; fi
if [ -s \"$x\" ] && [ -n \"$(tail -c 1 \"$x\")\" ]; then printf '\\n' >> \"$x\"; fi
printf '%s\\n%s\\n' \"$M\" \"$P\" >> \"$x\"
}}
e() {{
{ENTRY_CHECKS}
if [ ! -d \"$src\" ]; then printf 'S %s notDirectory\\n' \"$i\"; return 0; fi
if ! (cd -- \"$S\" && G check-ignore -q --no-index -- \"$from\") >/dev/null 2>&1; then printf 'S %s notIgnored\\n' \"$i\"; return 0; fi
if [ -e \"$dst\" ] || [ -h \"$dst\" ]; then printf 'S %s exists\\n' \"$i\"; return 0; fi
safe_dir \"$(dirname -- \"$to\")\" || {{ printf 'S %s outside\\n' \"$i\"; return 0; }}
if [ -e \"$dst\" ] || [ -h \"$dst\" ]; then printf 'S %s exists\\n' \"$i\"; return 0; fi
ln -s -- \"$q\" \"$dst\" 2>/dev/null || {{ printf 'S %s failed\\n' \"$i\"; return 0; }}
M=$4; P=$5
if admin; then add \"$b\"; [ \"$a\" = \"$b\" ] || add \"$a\"; fi
printf 'L %s\\n' \"$i\"
}}
{calls}",
        folders = folders(source, destination),
        git = git_function(GitKind::Delegation),
        environment = git_environment(GitKind::Delegation),
        top = quote(top),
        calls = calls.join("\n"),
    ))
}

/// What [`copy_script`] and [`share_script`] said of each entry: `(index, None)` done, `(index, Some(why))` skipped.
pub fn parse_entries(stdout: &str) -> Vec<(usize, Option<String>)> {
    stdout
        .lines()
        .filter_map(|line| {
            let mut fields = line.splitn(3, ' ');
            let (kind, index) = (fields.next()?, fields.next()?.parse::<usize>().ok()?);
            match kind {
                "C" | "L" => Some((index, None)),
                "S" => Some((index, Some(fields.next()?.to_string()))),
                _ => None,
            }
        })
        .collect()
}

/// The worktree's git folders (its own and the common one) and its top as `a=`, `b=` and `t=` lines, read before the worktree goes, so its
/// shared folders' exclude entries can be taken out after.
pub fn admin_script(top: &str) -> Option<String> {
    valid_path(top).then(|| {
        format!(
            "{git}
{environment}
cd -- {top} 2>/dev/null || exit 3
a=$(G rev-parse --absolute-git-dir 2>/dev/null) || exit 4
b=$(G rev-parse --git-common-dir 2>/dev/null) || exit 4
b=$(cd -P -- \"$b\" 2>/dev/null && pwd -P) || exit 4
t=$(G rev-parse --show-toplevel 2>/dev/null) || exit 4
printf 'a=%s\\nb=%s\\nt=%s\\n' \"$a\" \"$b\" \"$t\"",
            git = git_function(GitKind::Delegation),
            environment = git_environment(GitKind::Delegation),
            top = quote(top),
        )
    })
}

/// [`admin_script`]'s answer: `(git folder, common git folder, the worktree's top)`.
pub fn parse_admin(stdout: &str) -> Option<(String, String, String)> {
    let (mut a, mut b, mut t) = (None, None, None);
    for line in stdout.lines() {
        if let Some(rest) = line.strip_prefix("a=") {
            a = Some(rest.to_string());
        } else if let Some(rest) = line.strip_prefix("b=") {
            b = Some(rest.to_string());
        } else if let Some(rest) = line.strip_prefix("t=") {
            t = Some(rest.to_string());
        }
    }
    let (a, b, t) = (a?, b?, t?);
    (valid_path(&a) && valid_path(&b) && valid_path(&t)).then_some((a, b, t))
}

/// Every entry under `marker` (the marker line and the line after it) taken out of `<admin>/info/exclude` for each of `admins`
/// (a plain file below a plain `info` folder, at most 1 MiB; written back only when something went).
pub fn exclude_script(admins: &[String], marker: &str) -> Option<String> {
    if !admins.iter().all(|a| valid_path(a)) || marker.contains(['\n', '\r', '\0']) {
        return None;
    }
    let words: Vec<String> = admins.iter().map(|a| quote(a)).collect();
    Some(format!(
        "M={marker}; export M
for a in {admins}; do
x=\"$a/info/exclude\"
[ -d \"$a\" ] && [ ! -h \"$a\" ] && [ -d \"$a/info\" ] && [ ! -h \"$a/info\" ] && [ -f \"$x\" ] && [ ! -h \"$x\" ] || continue
[ \"$(wc -c < \"$x\" | tr -d ' ')\" -le 1048576 ] || continue
awk 'BEGIN {{m=ENVIRON[\"M\"]}} skip {{skip=0; next}} $0==m {{skip=1; next}} {{print}}' \"$x\" > \"$x.mighty-tmp\" || {{ rm -f -- \"$x.mighty-tmp\"; continue; }}
if cmp -s \"$x\" \"$x.mighty-tmp\"; then rm -f -- \"$x.mighty-tmp\"; else mv -f -- \"$x.mighty-tmp\" \"$x\"; fi
done",
        marker = quote(marker),
        admins = words.join(" "),
    ))
}

// ── the repository's config ─────────────────────────────────────────────────────────────────────────────────────────────

/// What [`config_script`] found of a config file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigFile {
    /// None of the names is there.
    Missing,
    /// The first one there is a link (refused, as here).
    Link(String),
    /// The first one there is no regular file.
    NotFile(String),
    /// The first one there is larger than the limit.
    TooLarge(String),
    /// Its name and bytes.
    File(String, Vec<u8>),
}

/// The first of `names` (a repository's `.mighty.yaml` names, in order) that is in `dir`: a regular file of at most `limit` bytes
/// written out after `F <name> <size>`, else what it is (`L`, `N`, `B` with its name; `M` when none is there).
pub fn config_script(dir: &str, names: &[&str], limit: usize) -> Option<String> {
    if !valid_remote_dir(dir) || names.iter().any(|n| n.is_empty() || n.contains(['/', '\n', '\0'])) {
        return None;
    }
    let words: Vec<String> = names.iter().map(|n| quote(n)).collect();
    Some(format!(
        "cd {dir} 2>/dev/null || exit 3
for n in {names}; do
p=./$n
if [ ! -e \"$p\" ] && [ ! -h \"$p\" ]; then continue; fi
if [ -h \"$p\" ]; then printf 'L %s\\n' \"$n\"; exit 0; fi
if [ ! -f \"$p\" ]; then printf 'N %s\\n' \"$n\"; exit 0; fi
s=$(wc -c < \"$p\" | tr -d ' ')
if [ \"$s\" -gt {limit} ]; then printf 'B %s\\n' \"$n\"; exit 0; fi
printf 'F %s %s\\n' \"$n\" \"$s\"
cat -- \"$p\"
exit 0
done
printf 'M\\n'",
        dir = quote_dir(dir),
        names = words.join(" "),
    ))
}

/// [`config_script`]'s answer; None when it cannot be read.
pub fn parse_config(stdout: &[u8]) -> Option<ConfigFile> {
    let end = stdout.iter().position(|b| *b == b'\n')?;
    let head = std::str::from_utf8(&stdout[..end]).ok()?;
    let rest = &stdout[end + 1..];
    let mut fields = head.splitn(3, ' ');
    match (fields.next()?, fields.next(), fields.next()) {
        ("M", None, None) => Some(ConfigFile::Missing),
        ("L", Some(name), None) => Some(ConfigFile::Link(name.to_string())),
        ("N", Some(name), None) => Some(ConfigFile::NotFile(name.to_string())),
        ("B", Some(name), None) => Some(ConfigFile::TooLarge(name.to_string())),
        ("F", Some(name), Some(size)) => {
            let size: usize = size.parse().ok()?;
            (rest.len() == size).then(|| ConfigFile::File(name.to_string(), rest.to_vec()))
        }
        _ => None,
    }
}

// ── ports and commands ──────────────────────────────────────────────────────────────────────────────────────────────────

/// The far side's listening TCP sockets: Linux's `/proc/net/tcp` and `tcp6` where they are, else `netstat -an`.
pub fn listening_script() -> String {
    "if [ -r /proc/net/tcp ]; then echo proc; cat /proc/net/tcp /proc/net/tcp6 2>/dev/null; else echo netstat; netstat -an 2>/dev/null; fi"
        .to_string()
}

/// The ports [`listening_script`] found listening; None when it read nothing it knows.
pub fn parse_listening(stdout: &str) -> Option<BTreeSet<u16>> {
    let mut lines = stdout.lines();
    let mut ports = BTreeSet::new();
    match lines.next()?.trim() {
        "proc" => {
            for line in lines {
                let fields: Vec<&str> = line.split_whitespace().collect();
                // sl local_address rem_address st …: `0A` is LISTEN.
                if fields.len() > 3
                    && fields[3] == "0A"
                    && let Some((_, port)) = fields[1].rsplit_once(':')
                    && let Ok(port) = u16::from_str_radix(port, 16)
                {
                    ports.insert(port);
                }
            }
        }
        "netstat" => {
            let mut read = false;
            for line in lines {
                let fields: Vec<&str> = line.split_whitespace().collect();
                if !fields.first().is_some_and(|f| f.starts_with("tcp")) || !line.contains("LISTEN") {
                    continue;
                }
                read = true;
                // macOS `127.0.0.1.41000` / `*.41000`, Linux `127.0.0.1:41000` / `:::41000`.
                if let Some(local) = fields.get(3)
                    && let Some(at) = local.rfind([':', '.'])
                    && let Ok(port) = local[at + 1..].parse::<u16>()
                {
                    ports.insert(port);
                }
            }
            if !read && ports.is_empty() && stdout.lines().count() <= 1 {
                return None;
            }
        }
        _ => return None,
    }
    Some(ports)
}

/// A worktree's setup or teardown command as the local one runs it (`/bin/sh -c`, `-ec` for several lines), in `dir` with `env`'s
/// values, its programs found on the login shell's `PATH`; its standard input is closed. None when a name or the folder may not be
/// given.
pub fn command_script(dir: &str, command: &str, env: &[(String, String)]) -> Option<String> {
    if !valid_remote_dir(dir) || command.contains('\0') {
        return None;
    }
    let mut inner = String::from("exec env");
    for (name, value) in env {
        if !valid_env_name(name) || value.contains('\0') {
            return None;
        }
        inner.push(' ');
        inner.push_str(&quote(&format!("{name}={value}")));
    }
    let flag = if command.contains('\n') { "-ec" } else { "-c" };
    inner.push_str(&format!(" /bin/sh {flag} {} </dev/null", quote(command)));
    Some(format!("cd {} || exit 1\n{}", quote_dir(dir), login_exec(&inner)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn far_side_paths_are_posix_on_every_os() {
        assert_eq!(posix::standardized("/home/me/./x//y/../z/"), "/home/me/x/z");
        assert_eq!(posix::standardized("/"), "/");
        assert_eq!(posix::standardized("/.."), "/");
        assert_eq!(posix::combine("/r", "id"), "/r/id");
        assert_eq!(posix::combine("/", "id"), "/id");
        assert!(posix::is_inside("/r/a", "/r") && !posix::is_inside("/r2", "/r") && !posix::is_inside("/r", "/r"));
        assert_eq!(posix::file_name("/r/abc/"), Some("abc"));
        assert_eq!(posix::parent("/r/abc"), Some("/r".into()));
        assert_eq!(posix::parent("/abc"), Some("/".into()));
        assert_eq!(posix::parent("/"), None);
    }

    #[test]
    fn scripts_refuse_what_they_may_not_name() {
        assert!(git_script("relative", GitKind::Delegation, &["status"], 10).is_none());
        assert!(git_script("/srv", GitKind::Delegation, &["a\0b"], 10).is_none());
        assert!(probe_script(&["relative".into()]).is_none());
        assert!(probe_script(&vec!["/x".to_string(); PROBE_CHUNK + 1]).is_none());
        assert!(read_inside_script("/r", "../x", 10).is_none());
        assert!(copy_script("/s", "/d", &[CopyEntry { from: "../x".into(), to: "x".into() }]).is_none());
        assert!(command_script("/w", "make", &[("BAD NAME".into(), "v".into())]).is_none());
        assert!(exclude_script(&["/a".into()], "two\nlines").is_none());
    }

    #[test]
    fn a_push_alone_may_reach_a_remote() {
        let script = git_script("/srv/app", GitKind::Delegation, &["merge-base", "--is-ancestor", "a", "b"], 10).unwrap();
        assert!(script.contains("'protocol.allow=never'") && script.contains("GIT_NO_LAZY_FETCH=1"), "{script}");
        assert!(script.contains("'core.hooksPath=/dev/null'") && script.contains("'core.fsmonitor=false'"));
        let push = git_script("/srv/app", GitKind::Push, &["push", "--porcelain", "origin", "x:x"], 10).unwrap();
        assert!(!push.contains("protocol.allow") && !push.contains("GIT_NO_LAZY_FETCH"), "{push}");
        assert!(push.contains("'core.hooksPath=/dev/null'"));
    }

    #[test]
    fn listening_ports_are_read_from_proc_and_netstat() {
        let proc = "proc
  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 0100007F:A028 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 1 1 0000000000000000 100 0 0 10 0
   1: 0100007F:A029 0100007F:A028 01 00000000:00000000 00:00000000 00000000  1000        0 1 1 0000000000000000 100 0 0 10 0
   0: 00000000000000000000000000000000:1F90 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000 0 0 1";
        assert_eq!(parse_listening(proc), Some(BTreeSet::from([41_000, 8080])));
        let mac = "netstat\nActive Internet connections (including servers)\nProto Recv-Q Send-Q  Local Address          Foreign Address        (state)\n\
tcp4       0      0  127.0.0.1.41003        *.*                    LISTEN\ntcp46      0      0  *.5173                 *.*                    LISTEN\n\
tcp4       0      0  10.0.0.2.50000         1.2.3.4.443            ESTABLISHED";
        assert_eq!(parse_listening(mac), Some(BTreeSet::from([41_003, 5173])));
        let linux = "netstat\ntcp        0      0 127.0.0.1:41004         0.0.0.0:*               LISTEN\ntcp6       0      0 :::3000                 :::*                    LISTEN";
        assert_eq!(parse_listening(linux), Some(BTreeSet::from([41_004, 3000])));
        assert_eq!(parse_listening("netstat\n"), None);
        assert_eq!(parse_listening(""), None);
    }

    #[test]
    fn answers_are_read_whole_or_not_at_all() {
        assert_eq!(parse_info("home=/home/me\ngit=1\ndone=1\n"), Some(Info { home: "/home/me".into(), git: true }));
        assert_eq!(parse_info("home=/home/me\ngit=1\n"), None);
        assert_eq!(parse_info("home=relative\ngit=0\ndone=1\n"), None);
        assert_eq!(
            parse_probe("1 1 1 -\n0 0 1 12\n0 0 0 -\n", 3),
            Some(vec![
                Probe { link: true, folder: true, exists: true, size: None },
                Probe { link: false, folder: false, exists: true, size: Some(12) },
                Probe::default(),
            ])
        );
        assert_eq!(parse_probe("0 0 0 -\n", 2), None);
        assert_eq!(parse_free("1024\n"), Some(1_048_576));
        assert_eq!(
            parse_entries("C 0\nS 1 missing\nL 2\nS 3 not Ignored\nX 4\n"),
            vec![(0, None), (1, Some("missing".into())), (2, None), (3, Some("not Ignored".into()))]
        );
        assert_eq!(
            parse_admin("a=/r/.git/worktrees/x\nb=/r/.git\nt=/w/x\n"),
            Some(("/r/.git/worktrees/x".into(), "/r/.git".into(), "/w/x".into()))
        );
    }

    /// The scripts through this computer's real `/bin/sh` (and `git`, where it is): exit codes, the deadline, the probe, the reads,
    /// the removals, the copies and the shared folders.
    #[cfg(unix)]
    mod shell {
        use std::path::{Path, PathBuf};

        use super::super::*;
        use crate::ssh::command::sh;

        struct Place(PathBuf);

        impl Drop for Place {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }

        fn place(name: &str) -> Place {
            let dir = std::env::temp_dir().join(format!("mc-ssh-worktree-{name}-{}", crate::wire::new_id()));
            std::fs::create_dir_all(&dir).unwrap();
            // The path as the far side's `pwd -P` names it (`/var` is `/private/var` on a Mac).
            Place(std::fs::canonicalize(&dir).unwrap())
        }

        fn text(path: &Path) -> String {
            path.to_string_lossy().into_owned()
        }

        /// The script as the far side runs it: the one word `sh` makes, through `/bin/sh -c` of the login shell.
        fn run(script: &str, input: Option<&[u8]>) -> (i32, String, String) {
            use std::io::Write as _;
            #[allow(clippy::disallowed_methods, reason = "the check's far side is a real shell here")]
            let mut child = std::process::Command::new("/bin/sh")
                .arg("-c")
                .arg(sh(script))
                .env("SHELL", "/bin/sh")
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            let mut stdin = child.stdin.take().unwrap();
            if let Some(input) = input {
                stdin.write_all(input).unwrap();
            }
            drop(stdin);
            let out = child.wait_with_output().unwrap();
            (
                out.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&out.stdout).into_owned(),
                String::from_utf8_lossy(&out.stderr).into_owned(),
            )
        }

        fn has_git() -> bool {
            std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).any(|p| p.join("git").is_file())
        }

        #[allow(clippy::disallowed_methods, reason = "a check makes its repository")]
        fn git(dir: &Path, args: &[&str]) -> String {
            let out = std::process::Command::new("git")
                .args(["-c", "user.name=t", "-c", "user.email=t@e", "-c", "commit.gpgsign=false", "-c", "init.defaultBranch=main"])
                .args(args)
                .current_dir(dir)
                .output()
                .unwrap();
            assert!(out.status.success(), "{out:?}");
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        }

        #[test]
        fn git_runs_with_its_exit_code_its_input_and_a_deadline() {
            if !has_git() {
                return;
            }
            let here = place("git");
            let repo = here.0.join("it's a repo");
            std::fs::create_dir_all(&repo).unwrap();
            git(&repo, &["init", "-q"]);
            std::fs::write(repo.join("a [1].txt"), "a\n").unwrap();
            git(&repo, &["add", "-A"]);
            git(&repo, &["commit", "-q", "-m", "one"]);
            let dir = text(&repo);
            let (code, out, _) = run(&git_script(&dir, GitKind::Delegation, &["rev-parse", "--is-inside-work-tree"], 10).unwrap(), None);
            assert_eq!((code, out.trim()), (0, "true"));
            let (code, _, _) =
                run(&git_script(&dir, GitKind::Delegation, &["merge-base", "--is-ancestor", "HEAD", "HEAD"], 10).unwrap(), None);
            assert_eq!(code, 0);
            let (code, _, err) = run(&git_script(&dir, GitKind::Delegation, &["rev-parse", "--verify", "nope"], 10).unwrap(), None);
            assert!(code == 128 && err.contains("fatal"), "{code} {err}");
            // The standard input reaches git; a pathspec the caller marks literal is read literally.
            std::fs::write(repo.join("a1.txt"), "x\n").unwrap();
            let (code, _, err) = run(
                &git_script(&dir, GitKind::Delegation, &["add", "--pathspec-from-file=-", "--pathspec-file-nul"], 10).unwrap(),
                Some(b"a1.txt"),
            );
            assert_eq!(code, 0, "{err}");
            assert_eq!(git(&repo, &["diff", "--cached", "--name-only"]), "a1.txt");
            let (code, out, _) =
                run(&git_script(&dir, GitKind::Delegation, &["--literal-pathspecs", "ls-files", "--", "a [1].txt"], 10).unwrap(), None);
            assert_eq!((code, out.trim()), (0, "a [1].txt"));
            // An inherited GIT_DIR never points the far side's git elsewhere.
            let (code, out, _) = run(
                &format!(
                    "GIT_DIR=/nowhere; export GIT_DIR\n{}",
                    git_script(&dir, GitKind::WorkMap, &["rev-parse", "--show-toplevel"], 10).unwrap()
                ),
                None,
            );
            assert_eq!((code, out.trim()), (0, dir.as_str()));
            let (code, _, _) = run(&git_script(&text(&here.0.join("gone")), GitKind::Delegation, &["status"], 10).unwrap(), None);
            assert_eq!(code, FOLDER_GONE);
            // A git that outlives its deadline is ended over there.
            let started = std::time::Instant::now();
            let (code, _, _) = run(
                &format!(
                    "git() {{ exec sleep 30; }}\n{}",
                    git_script(&dir, GitKind::Delegation, &["status"], 1).unwrap().replacen(
                        "command -v git >/dev/null 2>&1 || exit 251\n",
                        "",
                        1
                    )
                ),
                None,
            );
            assert_eq!(code, TIMED_OUT);
            assert!(started.elapsed() < std::time::Duration::from_secs(10), "{:?}", started.elapsed());
        }

        #[test]
        fn probes_reads_and_removals() {
            let here = place("fs");
            let root = here.0.join("root dir");
            std::fs::create_dir_all(root.join("sub")).unwrap();
            std::fs::write(root.join("sub/file.txt"), "12345").unwrap();
            std::fs::write(here.0.join("outside.txt"), "secret").unwrap();
            std::os::unix::fs::symlink(here.0.join("outside.txt"), root.join("link.txt")).unwrap();
            std::os::unix::fs::symlink(&here.0, root.join("up")).unwrap();
            let paths: Vec<String> = ["sub", "sub/file.txt", "link.txt", "up", "missing"].iter().map(|p| text(&root.join(p))).collect();
            let (code, out, _) = run(&probe_script(&paths).unwrap(), None);
            assert_eq!(code, 0);
            let probes = parse_probe(&out, 5).unwrap();
            assert!(probes[0].folder && !probes[0].link && probes[0].size.is_none());
            assert_eq!(probes[1].size, Some(5));
            assert!(probes[2].link && probes[2].exists && probes[2].size.is_none());
            assert!(probes[3].link && probes[3].folder);
            assert_eq!(probes[4], Probe::default());
            let r = text(&root);
            assert_eq!(run(&read_inside_script(&r, "sub/file.txt", 100).unwrap(), None).1, "12345");
            assert_eq!(run(&read_inside_script(&r, "sub/file.txt", 4).unwrap(), None).0, 6);
            assert_eq!(run(&read_inside_script(&r, "link.txt", 100).unwrap(), None).0, 4);
            assert_eq!(run(&read_inside_script(&r, "up/outside.txt", 100).unwrap(), None).0, 5);
            assert_eq!(run(&read_first_script(&r, "sub/file.txt", 3).unwrap(), None).1, "123");
            assert_eq!(run(&read_first_script(&r, "sub/file.txt", 100).unwrap(), None).1, "12345");
            assert_eq!(run(&read_first_script(&r, "link.txt", 100).unwrap(), None).0, 4);
            let current =
                crate::ssh::command::unwrap_sh(&current_script(&r, &["sub/file.txt".into(), "link.txt".into()]).unwrap()).unwrap();
            let (code, out, _) = run(&current, None);
            assert_eq!(code, 0);
            let got = crate::ssh::review::parse_reads(out.as_bytes(), 2).unwrap();
            assert!(matches!(&got[0], crate::ssh::review::Got::Bytes(Some(_), bytes) if bytes == b"12345"), "{got:?}");
            assert_eq!(got[1], crate::ssh::review::Got::None, "a link is never read");
            // Directory links go first (never what they point to); a file link stays.
            let (code, out, _) = run(&clear_links_script(&r).unwrap(), None);
            assert_eq!((code, out.trim()), (0, "0"));
            assert!(std::fs::symlink_metadata(root.join("up")).is_err() && here.0.join("outside.txt").is_file());
            assert!(std::fs::symlink_metadata(root.join("link.txt")).is_ok());
            // The whole folder goes, a read-only folder in it too; what a link pointed to stays.
            std::fs::create_dir_all(root.join("ro/inner")).unwrap();
            std::fs::write(root.join("ro/inner/f"), "f").unwrap();
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(root.join("ro/inner"), std::fs::Permissions::from_mode(0o555)).unwrap();
            assert_eq!(run(&remove_script(&r).unwrap(), None).0, 0);
            assert!(!root.exists() && here.0.join("outside.txt").is_file());
            // A place that is a link is removed itself.
            std::os::unix::fs::symlink(&here.0, here.0.join("place")).unwrap();
            assert_eq!(run(&unlink_script(&text(&here.0.join("place"))).unwrap(), None).0, 0);
            assert!(std::fs::symlink_metadata(here.0.join("place")).is_err() && here.0.is_dir());
            assert_eq!(run(&mkdir_script(&text(&here.0.join("a/b/c"))).unwrap(), None).0, 0);
            assert!(here.0.join("a/b/c").is_dir());
            let (code, out, _) = run(&free_script(&text(&here.0.join("a/b/c/not yet"))).unwrap(), None);
            assert!(code == 0 && parse_free(&out).is_some_and(|free| free > 0), "{out}");
            std::fs::create_dir_all(here.0.join("cfg")).unwrap();
            std::fs::write(here.0.join("cfg/.mighty.yml"), "worktree:\n  setup: npm ci\n").unwrap();
            let cfg = text(&here.0.join("cfg"));
            let read = |names: &[&str], limit| {
                use std::process::Stdio;
                #[allow(clippy::disallowed_methods, reason = "the check's far side is a real shell here")]
                let out = std::process::Command::new("/bin/sh")
                    .arg("-c")
                    .arg(sh(&config_script(&cfg, names, limit).unwrap()))
                    .stderr(Stdio::null())
                    .output()
                    .unwrap();
                parse_config(&out.stdout)
            };
            assert_eq!(
                read(&[".mighty.yaml", ".mighty.yml"], 1000),
                Some(ConfigFile::File(".mighty.yml".into(), b"worktree:\n  setup: npm ci\n".to_vec()))
            );
            assert_eq!(read(&[".mighty.yml"], 3), Some(ConfigFile::TooLarge(".mighty.yml".into())));
            assert_eq!(read(&[".mighty.local.yaml"], 1000), Some(ConfigFile::Missing));
            std::os::unix::fs::symlink(here.0.join("cfg/.mighty.yml"), here.0.join("cfg/.mighty.local.yaml")).unwrap();
            assert_eq!(read(&[".mighty.local.yaml"], 1000), Some(ConfigFile::Link(".mighty.local.yaml".into())));
            let (code, out, _) = run(&listening_script(), None);
            assert!(code == 0 && (out.starts_with("proc") || out.starts_with("netstat")), "{out}");
        }

        #[test]
        fn a_setup_command_runs_with_its_values_and_no_input() {
            let here = place("command");
            let dir = text(&here.0);
            let env = vec![("PORT".to_string(), "41000".to_string()), ("MIGHTY_X".to_string(), "it's $HOME".to_string())];
            let (code, out, _) = run(
                &command_script(&dir, "printf '%s|%s|%s' \"$PORT\" \"$MIGHTY_X\" \"$(pwd -P)\"; cat", &env).unwrap(),
                Some(b"never read"),
            );
            assert_eq!(code, 0);
            assert_eq!(out, format!("41000|it's $HOME|{dir}"));
            let (code, _, _) = run(&command_script(&dir, "true\nfalse\necho never", &[]).unwrap(), None);
            assert_eq!(code, 1, "several lines stop at the first that fails");
            assert_eq!(run(&command_script(&text(&here.0.join("gone")), "true", &[]).unwrap(), None).0, 1);
        }

        #[test]
        fn copies_keep_the_local_rules() {
            let here = place("copy");
            let (source, destination) = (here.0.join("source"), here.0.join("dest"));
            std::fs::create_dir_all(source.join("conf/deep")).unwrap();
            std::fs::create_dir_all(&destination).unwrap();
            std::fs::write(source.join(".env"), "A=1\n").unwrap();
            std::fs::write(source.join("conf/deep/x.json"), "{}").unwrap();
            std::fs::write(source.join("conf/a.txt"), "a").unwrap();
            std::fs::write(here.0.join("secret"), "s").unwrap();
            std::os::unix::fs::symlink(here.0.join("secret"), source.join("conf/link")).unwrap();
            std::os::unix::fs::symlink(here.0.join("secret"), source.join("top-link")).unwrap();
            std::fs::write(destination.join("taken"), "mine").unwrap();
            std::fs::write(source.join("taken"), "theirs").unwrap();
            let entries =
                |list: &[(&str, &str)]| list.iter().map(|(f, t)| CopyEntry { from: f.to_string(), to: t.to_string() }).collect::<Vec<_>>();
            let script = copy_script(
                &text(&source),
                &text(&destination),
                &entries(&[
                    (".env", ".env"),
                    ("conf", "conf"),
                    ("top-link", "top-link"),
                    ("nope", "nope"),
                    ("taken", "taken"),
                    (".env", "nested/dir/.env.copy"),
                ]),
            )
            .unwrap();
            let (code, out, err) = run(&script, None);
            assert_eq!(code, 0, "{err}");
            assert_eq!(
                parse_entries(&out),
                vec![(0, None), (1, None), (2, Some("link".into())), (3, Some("missing".into())), (4, Some("exists".into())), (5, None)]
            );
            assert_eq!(std::fs::read_to_string(destination.join(".env")).unwrap(), "A=1\n");
            assert_eq!(std::fs::read_to_string(destination.join("conf/deep/x.json")).unwrap(), "{}");
            assert_eq!(std::fs::read_to_string(destination.join("nested/dir/.env.copy")).unwrap(), "A=1\n");
            assert!(std::fs::symlink_metadata(destination.join("conf/link")).is_err(), "a link inside is never copied");
            assert_eq!(std::fs::read_to_string(destination.join("taken")).unwrap(), "mine", "never over a file the worktree has");
            // A folder on the way that is a link out of the worktree: nothing is written through it.
            std::os::unix::fs::symlink(&here.0, destination.join("out")).unwrap();
            let (_, out, _) = run(&copy_script(&text(&source), &text(&destination), &entries(&[(".env", "out/escaped")])).unwrap(), None);
            assert_eq!(parse_entries(&out), vec![(0, Some("failed".into()))]);
            assert!(!here.0.join("escaped").exists());
            // Over the limit: that entry is `limit` and the rest are not read.
            std::fs::write(source.join("big.bin"), vec![0u8; (COPY_MAXIMUM_BYTES + 1) as usize]).unwrap();
            let (_, out, _) = run(
                &copy_script(&text(&source), &text(&destination), &entries(&[("big.bin", "big.bin"), ("conf/a.txt", "a2.txt")])).unwrap(),
                None,
            );
            assert_eq!(parse_entries(&out), vec![(0, Some("limit".into()))]);
            assert!(!destination.join("big.bin").exists() && !destination.join("a2.txt").exists());
        }

        #[test]
        fn shared_folders_are_linked_and_their_entries_kept_and_taken_out() {
            if !has_git() {
                return;
            }
            let here = place("share");
            let repo = here.0.join("repo");
            std::fs::create_dir_all(repo.join("node_modules/pkg")).unwrap();
            std::fs::create_dir_all(repo.join("tracked")).unwrap();
            git(&repo, &["init", "-q"]);
            std::fs::write(repo.join(".gitignore"), "node_modules/\n").unwrap();
            std::fs::write(repo.join("tracked/a"), "a").unwrap();
            git(&repo, &["add", "-A"]);
            git(&repo, &["commit", "-q", "-m", "one"]);
            let tree = here.0.join("tree");
            git(&repo, &["worktree", "add", "-q", "-b", "mighty/x", &text(&tree), "HEAD"]);
            let top = text(&tree);
            let excludes = vec![
                Exclude { marker: format!("# mighty-code shared {top}"), pattern: "/node_modules".into() },
                Exclude { marker: format!("# mighty-code shared {top}"), pattern: "/tracked".into() },
                Exclude { marker: format!("# mighty-code shared {top}"), pattern: "/nope".into() },
            ];
            let paths = vec!["node_modules".to_string(), "tracked".to_string(), "nope".to_string()];
            let script = share_script(&text(&repo), &top, &top, &paths, &excludes).unwrap();
            let (code, out, err) = run(&script, None);
            assert_eq!(code, 0, "{err}");
            assert_eq!(parse_entries(&out), vec![(0, None), (1, Some("notIgnored".into())), (2, Some("missing".into()))]);
            assert!(std::fs::symlink_metadata(tree.join("node_modules")).unwrap().file_type().is_symlink());
            let common = repo.join(".git/info/exclude");
            let marker = format!("# mighty-code shared {top}");
            assert!(std::fs::read_to_string(&common).unwrap().contains(&format!("{marker}\n/node_modules\n")));
            // Once only.
            let again = share_script(&text(&repo), &top, &top, &paths[..1], &excludes[..1]).unwrap();
            assert_eq!(parse_entries(&run(&again, None).1), vec![(0, Some("exists".into()))]);
            assert_eq!(git(&tree, &["status", "--porcelain"]), "", "the linked folder never shows as a change");
            let (code, out, _) = run(&admin_script(&top).unwrap(), None);
            let (a, b, t) = parse_admin(&out).unwrap();
            assert!(code == 0 && b == text(&std::fs::canonicalize(repo.join(".git")).unwrap()) && a != b && t == top, "{out}");
            assert_eq!(run(&exclude_script(&[b.clone(), a], &marker).unwrap(), None).0, 0);
            assert!(!std::fs::read_to_string(&common).unwrap().contains(&marker));
        }
    }
}
