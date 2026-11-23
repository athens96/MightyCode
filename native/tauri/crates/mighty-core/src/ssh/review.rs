//! MC-40: what an SSH workspace's run changed, kept on the far side as git trees (the remote counterpart of MC-18's snapshots and
//! MC-30's originals). A snapshot is a tree of the workspace folder's files that git does not ignore, written with git's own
//! plumbing into a store of the app's, `~/.cache/mighty-code/snapshots/<cksum of the folder's path>` (0700), never into the
//! repository:
//!
//! - **a repository's folder** (`repo`): a private index (`GIT_INDEX_FILE`, the repository's own copied the first time so only files
//!   that changed are read again) and the store's own object folder (`GIT_OBJECT_DIRECTORY`) with the repository's objects as an
//!   alternate, exactly as MC-18 does here: `git add -A -- .` then `git write-tree`. The repository's index, objects and refs are
//!   never written.
//! - **any other folder** (`plain`): a bare git folder of the store's with the workspace folder as its work tree, at most
//!   [`MAXIMUM_PLAIN_FILES`] files (a larger folder has no snapshots).
//!
//! A held run's start script takes the `start` snapshot before the CLI starts, and its holder the `end` one after the CLI ended
//! ([`snapshot_script`]); each request's pair is kept as `runs/<pane>/<request>` (`<start> [<end>]`) and its order in
//! `runs/<pane>/order`. The review reads the pane's newest requests with what each changed ([`index_script`]: a request still
//! running is compared with the folder now), then the files' contents it needs ([`read_script`]: blobs of a tree and the files as
//! they are now, with their permissions). Writing back is mighty-core `ssh::files`' atomic write; a file the run made is moved into
//! the store's `trash` ([`trash_script`]), never deleted.
//!
//! git runs with no program a repository's config could start (`core.fsmonitor=false`, `core.hooksPath=/dev/null`, no automatic
//! maintenance, no transport, `GIT_NO_LAZY_FETCH`) and literal pathspecs.

use super::command::{quote, quote_dir, sh};
use super::files::{MAXIMUM_EDIT_BYTES, valid_relative};

/// The snapshot stores' folder under the far side's home.
pub const FOLDER: &str = ".cache/mighty-code/snapshots";
/// A folder outside a repository with more files than this has no snapshots.
pub const MAXIMUM_PLAIN_FILES: usize = 20_000;
/// The newest requests of a pane a review reads.
pub const MAXIMUM_RUNS: usize = 20;
/// The most changed paths read of one request.
pub const MAXIMUM_CHANGES: usize = 1_000;

/// A pane's or a request's id as it may name a file over there: the wire's identifiers (letters and digits first, then also `.`,
/// `_`, `:`, `-`), at most 128.
pub fn valid_id(id: &str) -> bool {
    crate::wire::identifier(id)
}

/// A git object name: 40 or 64 lower-case hex digits.
pub fn valid_object(name: &str) -> bool {
    matches!(name.len(), 40 | 64) && name.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Into the workspace folder (exit 3 when it is gone), the store `$S` named by the folder's real path, `G` as git with no program
/// a config could start, `ck` as a file's `cksum`.
fn prelude(dir: &str) -> String {
    format!(
        "umask 077
cd {dir} || exit 3
k=$(printf '%s' \"$PWD\" | LC_ALL=C cksum | tr ' ' '-')
S=\"$HOME\"/{folder}/\"$k\"
G() {{ git -c core.fsmonitor=false -c core.hooksPath=/dev/null -c gc.auto=0 -c maintenance.auto=false -c protocol.allow=never \
-c core.untrackedCache=false \"$@\"; }}
GIT_NO_LAZY_FETCH=1; GIT_TERMINAL_PROMPT=0; GIT_LITERAL_PATHSPECS=1; export GIT_NO_LAZY_FETCH GIT_TERMINAL_PROMPT GIT_LITERAL_PATHSPECS
ck() {{ set -- $(LC_ALL=C cksum); printf '%s %s' \"$1\" \"$2\"; }}",
        dir = quote_dir(dir),
        folder = quote(FOLDER),
    )
}

/// The store's git environment, as its `mode` file says (exit 4 when there is no store, 8 without git).
const USE_STORE: &str = "command -v git >/dev/null 2>&1 || exit 8
m=$(sed -n 1p \"$S/mode\" 2>/dev/null)
case \"$m\" in
repo) GIT_INDEX_FILE=\"$S/index\"; GIT_OBJECT_DIRECTORY=\"$S/objects\"; GIT_ALTERNATE_OBJECT_DIRECTORIES=$(sed -n 2p \"$S/mode\")
mkdir -p \"$S/objects\" || exit 5
export GIT_INDEX_FILE GIT_OBJECT_DIRECTORY GIT_ALTERNATE_OBJECT_DIRECTORIES ;;
plain) GIT_DIR=\"$S/git\"; GIT_WORK_TREE=\"$PWD\"; GIT_INDEX_FILE=\"$S/index\"; export GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE ;;
*) exit 4 ;;
esac";

/// Which snapshot of a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Moment {
    /// Before its CLI starts (kept once: a request's later processes keep its first start).
    Start,
    /// After its CLI ended (each end replaces the last).
    End,
}

/// The snapshot of the folder `dir` for the request `run` of the pane `pane` (a plain script: the held run embeds it). Exit 8
/// without git, 9 for a folder outside a repository with too many files, 5/6 when the store or the tree could not be written.
pub fn snapshot_script(dir: &str, pane: &str, run: &str, moment: Moment) -> Option<String> {
    if !valid_id(pane) || !valid_id(run) {
        return None;
    }
    let record = match moment {
        Moment::Start => {
            format!("if [ ! -f \"$f\" ]; then printf '%s\\n' \"$t\" > \"$f\"; printf '%s\\n' {run} >> \"$d/order\"; fi", run = quote(run))
        }
        Moment::End => {
            "if [ -f \"$f\" ]; then read s rest < \"$f\"; printf '%s %s\\n' \"$s\" \"$t\" > \"$f.tmp\" && mv -f \"$f.tmp\" \"$f\"; fi"
                .to_string()
        }
    };
    Some(format!(
        "{prelude}
command -v git >/dev/null 2>&1 || exit 8
mkdir -p \"$S\" || exit 5
if [ -f \"$S/dir\" ]; then [ \"$(cat \"$S/dir\")\" = \"$PWD\" ] || exit 5; else printf '%s' \"$PWD\" > \"$S/dir\"; fi
if [ \"$(G rev-parse --is-inside-work-tree 2>/dev/null)\" = true ] && o=$(G rev-parse --git-path objects 2>/dev/null); then
case \"$o\" in /*) ;; *) o=\"$PWD/$o\" ;; esac
printf 'repo\\n%s\\n' \"$o\" > \"$S/mode.tmp\" && mv -f \"$S/mode.tmp\" \"$S/mode\"
if [ ! -f \"$S/index\" ]; then i=$(G rev-parse --git-path index); case \"$i\" in /*) ;; *) i=\"$PWD/$i\" ;; esac; cp \"$i\" \"$S/index\" 2>/dev/null || :; fi
else
n=$(find . -name .git -prune -o -type f -print 2>/dev/null | head -n {over} | wc -l | tr -d ' ')
[ \"$n\" -le {most} ] || exit 9
[ -d \"$S/git\" ] || G init -q --bare \"$S/git\" >/dev/null 2>&1 || exit 5
printf 'plain\\n' > \"$S/mode\"
fi
{USE_STORE}
G add -A -- . >/dev/null 2>&1 || exit 6
t=$(G write-tree 2>/dev/null) || exit 6
d=\"$S/runs\"/{pane}
f=\"$d\"/{run}
mkdir -p \"$d\" || exit 5
{record}
printf 'tree=%s\\n' \"$t\"",
        prelude = prelude(dir),
        over = MAXIMUM_PLAIN_FILES + 1,
        most = MAXIMUM_PLAIN_FILES,
        pane = quote(pane),
        run = quote(run),
    ))
}

/// The pane's newest requests (at most [`MAXIMUM_RUNS`], oldest first) with what each changed: a first line `P <prefix>` (the
/// workspace folder's place in its repository, empty outside one), then for each request `R <run> <start> <end> <ended|running>
/// <bytes>` and that many bytes of `git diff-tree -r -z --no-renames --name-status`. A request still running is compared with a tree
/// of the folder now. Exit 4 when the pane has no snapshots.
pub fn index_script(dir: &str, pane: &str) -> Option<String> {
    valid_id(pane).then(|| {
        sh(&format!(
            "{prelude}
{USE_STORE}
d=\"$S/runs\"/{pane}
[ -f \"$d/order\" ] || exit 4
p=
[ \"$m\" = repo ] && p=$(G rev-parse --show-prefix 2>/dev/null)
printf 'P %s\\n' \"$p\"
now=
tmp=\"$S/index-out.$$\"
for r in $(tail -n {runs} \"$d/order\"); do
case \"$r\" in *[!A-Za-z0-9._:-]*) continue ;; esac
[ -f \"$d/$r\" ] || continue
read s e < \"$d/$r\"
o=ended
if [ -z \"$e\" ]; then
if [ -z \"$now\" ]; then G add -A -- . >/dev/null 2>&1; now=$(G write-tree 2>/dev/null); fi
e=$now; o=running
fi
[ -n \"$e\" ] || continue
G diff-tree -r -z --no-renames --name-status \"$s\" \"$e\" -- . > \"$tmp\" 2>/dev/null || : > \"$tmp\"
printf 'R %s %s %s %s %s\\n' \"$r\" \"$s\" \"$e\" \"$o\" \"$(wc -c < \"$tmp\" | tr -d ' ')\"
cat \"$tmp\"
done
rm -f \"$tmp\"",
            prelude = prelude(dir),
            pane = quote(pane),
            runs = MAXIMUM_RUNS,
        ))
    })
}

/// One request's snapshots and what it changed (paths relative to the workspace folder).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub run: String,
    pub start: String,
    pub end: String,
    pub running: bool,
    /// `(status, path)`: `A` added, `D` deleted, `M` modified, `T` type changed, in git's order.
    pub changes: Vec<(char, String)>,
}

/// [`index_script`]'s output read: the folder's prefix in its repository and the requests, newest first. Paths outside the folder
/// or that the files pane could not name are left out.
pub fn parse_index(stdout: &[u8]) -> Option<(String, Vec<Request>)> {
    fn line(bytes: &[u8], at: &mut usize) -> Option<String> {
        let end = bytes[*at..].iter().position(|b| *b == b'\n')? + *at;
        let text = String::from_utf8(bytes[*at..end].to_vec()).ok()?;
        *at = end + 1;
        Some(text)
    }
    let mut at = 0;
    let prefix = line(stdout, &mut at)?.strip_prefix("P ")?.to_string();
    let mut requests = Vec::new();
    while at < stdout.len() {
        let head = line(stdout, &mut at)?;
        let words: Vec<&str> = head.strip_prefix("R ")?.split(' ').collect();
        let [run, start, end, state, size] = words.as_slice() else { return None };
        let size: usize = size.parse().ok()?;
        let body = stdout.get(at..at + size)?;
        at += size;
        if !valid_id(run) || !valid_object(start) || !valid_object(end) {
            continue;
        }
        let mut changes = Vec::new();
        let mut parts = body.split(|b| *b == 0);
        while let (Some(status), Some(path)) = (parts.next(), parts.next()) {
            let (Some(&kind), Ok(path)) = (status.first(), std::str::from_utf8(path)) else { continue };
            let Some(relative) = path.strip_prefix(prefix.as_str()) else { continue };
            if !valid_relative(relative) || relative.is_empty() || changes.len() >= MAXIMUM_CHANGES {
                continue;
            }
            changes.push((kind as char, relative.to_string()));
        }
        requests.push(Request {
            run: run.to_string(),
            start: start.to_string(),
            end: end.to_string(),
            running: *state == "running",
            changes,
        });
    }
    requests.reverse();
    Some((prefix, requests))
}

/// One thing [`read_script`] reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Read {
    /// A file of a snapshot: its tree and its path in the repository (the prefix included).
    Blob { tree: String, path: String },
    /// A file of the workspace folder as it is now.
    Current(String),
}

/// What [`read_script`] read of one thing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Got {
    /// Not there (or a link, a folder, not a blob).
    None,
    /// Larger than [`MAXIMUM_EDIT_BYTES`]; a current file says its permissions.
    TooLarge(Option<u32>),
    /// Its bytes; a current file says its permissions.
    Bytes(Option<u32>, Vec<u8>),
}

/// Reads `reads` in one go: for each a line (`N`, `L [<mode>]`, `B <mode or -> <size>`) and that many bytes. A current file is
/// copied first, so its size and bytes agree. Exit 4 when there is no store.
pub fn read_script(dir: &str, reads: &[Read]) -> Option<String> {
    let mut lines = Vec::new();
    for read in reads {
        match read {
            Read::Blob { tree, path } => {
                if !valid_object(tree) || path.is_empty() || path.chars().any(char::is_control) {
                    return None;
                }
                lines.push(format!("blob {}", quote(&format!("{tree}:{path}"))));
            }
            Read::Current(relative) => {
                if !valid_relative(relative) || relative.is_empty() {
                    return None;
                }
                lines.push(format!("file ./{}", quote(relative)));
            }
        }
    }
    Some(sh(&format!(
        "{prelude}
{USE_STORE}
c=\"$S/read.$$\"
trap 'rm -f \"$c\"' EXIT
blob() {{ if [ \"$(G cat-file -t \"$1\" 2>/dev/null)\" != blob ]; then printf 'N\\n'; return; fi
s=$(G cat-file -s \"$1\" 2>/dev/null) || {{ printf 'N\\n'; return; }}
if [ \"$s\" -gt {cap} ]; then printf 'L\\n'; return; fi
G cat-file blob \"$1\" > \"$c\" 2>/dev/null || {{ printf 'N\\n'; return; }}
printf 'B - %s\\n' \"$(wc -c < \"$c\" | tr -d ' ')\"; cat \"$c\"; }}
file() {{ if [ -h \"$1\" ] || [ ! -f \"$1\" ]; then printf 'N\\n'; return; fi
mode=$(LC_ALL=C ls -ldn \"$1\" | cut -c 1-10)
cp \"$1\" \"$c\" 2>/dev/null || {{ printf 'N\\n'; return; }}
s=$(wc -c < \"$c\" | tr -d ' ')
if [ \"$s\" -gt {cap} ]; then printf 'L %s\\n' \"$mode\"; return; fi
printf 'B %s %s\\n' \"$mode\" \"$s\"; cat \"$c\"; }}
{lines}",
        prelude = prelude(dir),
        cap = MAXIMUM_EDIT_BYTES,
        lines = lines.join("\n"),
    )))
}

/// [`read_script`]'s output read, one [`Got`] for each read asked; None when the output does not hold them all.
pub fn parse_reads(stdout: &[u8], count: usize) -> Option<Vec<Got>> {
    let mut at = 0;
    let mut got = Vec::with_capacity(count);
    for _ in 0..count {
        let end = stdout[at..].iter().position(|b| *b == b'\n')? + at;
        let head = std::str::from_utf8(&stdout[at..end]).ok()?.to_string();
        at = end + 1;
        let words: Vec<&str> = head.split(' ').collect();
        let mode = |word: &str| (word != "-").then(|| super::files::mode_bits(word)).flatten();
        match words.as_slice() {
            ["N"] => got.push(Got::None),
            ["L"] => got.push(Got::TooLarge(None)),
            ["L", m] => got.push(Got::TooLarge(mode(m))),
            ["B", m, size] => {
                let size: usize = size.parse().ok()?;
                let bytes = stdout.get(at..at + size)?.to_vec();
                at += size;
                got.push(Got::Bytes(mode(m), bytes));
            }
            _ => return None,
        }
    }
    Some(got)
}

/// Moves a file a run made out of the folder into the store's `trash/<tag>/` (never deleted), only while it is still what
/// `expected` (its `cksum`) says: exit 7 when it changed, 4 when it is not a regular file.
pub fn trash_script(dir: &str, relative: &str, tag: &str, expected: (u32, u64)) -> Option<String> {
    if !valid_relative(relative) || relative.is_empty() || tag.len() != 32 || !tag.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let parent = relative.rsplit_once('/').map(|(parent, _)| parent.to_string()).unwrap_or_default();
    let (crc, size) = expected;
    Some(sh(&format!(
        "{prelude}
p=./{path}
if [ -h \"$p\" ] || [ ! -f \"$p\" ]; then exit 4; fi
[ \"$(ck < \"$p\")\" = '{crc} {size}' ] || exit 7
t=\"$S/trash\"/{tag}
mkdir -p \"$t\"/{parent} || exit 5
mv -f \"$p\" \"$t\"/{path} || exit 5
printf trashed",
        prelude = prelude(dir),
        path = quote(relative),
        parent = quote(&parent),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_objects_and_paths_are_checked() {
        assert!(valid_object(&"a".repeat(40)) && valid_object(&"0".repeat(64)) && !valid_object(&"A".repeat(40)));
        assert_eq!(snapshot_script("/srv", "pane'; rm", "run", Moment::Start), None);
        assert_eq!(read_script("/srv", &[Read::Current("../x".into())]), None);
        assert_eq!(read_script("/srv", &[Read::Blob { tree: "HEAD".into(), path: "a".into() }]), None);
        assert_eq!(trash_script("/srv", "a", "x", (1, 1)), None);
        assert_eq!(
            parse_reads(b"N\nB -rw-r--r-- 3\nabcL -rw-------\nB - 0\n", 4),
            Some(vec![Got::None, Got::Bytes(Some(0o644), b"abc".to_vec()), Got::TooLarge(Some(0o600)), Got::Bytes(None, vec![])])
        );
        assert_eq!(parse_reads(b"B - 9\nabc", 1), None);
        let tree = "1".repeat(40);
        let index = format!("P sub/\nR r1 {tree} {tree} ended 24\nM\0sub/a.txt\0A\0other.txt\0");
        let (prefix, requests) = parse_index(index.as_bytes()).unwrap();
        assert_eq!(prefix, "sub/");
        assert_eq!(requests[0].changes, [('M', "a.txt".to_string())], "a path outside the folder is left out");
    }

    /// The scripts with this computer's real `/bin/sh` and `git`, a home of the check's own: a repository's folder and a plain one
    /// snapshotted before and after a "run", what it changed read back, the original and current files read, a created file trashed.
    /// The repository's own index and objects are never written.
    #[cfg(unix)]
    #[test]
    fn snapshots_read_back_what_a_run_changed() {
        #[allow(clippy::disallowed_methods, reason = "a test runs the far side's scripts in a real shell")]
        fn run(home: &std::path::Path, command: &str) -> (i32, Vec<u8>) {
            let output = std::process::Command::new("/bin/sh").arg("-c").arg(command).env("HOME", home).output().unwrap();
            (output.status.code().unwrap_or(-1), output.stdout)
        }
        #[allow(clippy::disallowed_methods, reason = "a test makes a repository")]
        fn git(dir: &std::path::Path, args: &[&str]) {
            let status = std::process::Command::new("git")
                .args(["-c", "user.name=t", "-c", "user.email=t@e", "-c", "commit.gpgsign=false"])
                .args(args)
                .current_dir(dir)
                .output()
                .unwrap();
            assert!(status.status.success(), "{status:?}");
        }
        if !std::path::Path::new("/usr/bin/git").exists() && which_git().is_none() {
            return;
        }
        let root = std::env::temp_dir().join(format!("mc-ssh-review-{}", uuid::Uuid::new_v4().simple()));
        let home = root.join("home");
        let repo = root.join("repo it's");
        let folder = repo.join("sub dir");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("a.txt"), "one\ntwo\n").unwrap();
        std::fs::write(folder.join(".gitignore"), "ignored.log\n").unwrap();
        git(&repo, &["init", "-q"]);
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-q", "-m", "base"]);
        let index_before = std::fs::read(repo.join(".git/index")).unwrap();
        let dir = folder.to_string_lossy().into_owned();

        let (code, out) = run(&home, &sh(&snapshot_script(&dir, "p1", "r1", Moment::Start).unwrap()));
        assert_eq!(code, 0, "{}", String::from_utf8_lossy(&out));
        std::fs::write(folder.join("a.txt"), "one\nTWO\n").unwrap();
        std::fs::write(folder.join("new.txt"), "made\n").unwrap();
        std::fs::write(folder.join("ignored.log"), "noise\n").unwrap();
        assert_eq!(run(&home, &sh(&snapshot_script(&dir, "p1", "r1", Moment::End).unwrap())).0, 0);
        assert_eq!(std::fs::read(repo.join(".git/index")).unwrap(), index_before, "the repository's index is never written");

        let (code, out) = run(&home, &index_script(&dir, "p1").unwrap());
        assert_eq!(code, 0, "{}", String::from_utf8_lossy(&out));
        let (prefix, requests) = parse_index(&out).unwrap();
        assert_eq!(prefix, "sub dir/");
        assert_eq!(requests.len(), 1);
        assert!(!requests[0].running);
        assert_eq!(requests[0].changes, [('M', "a.txt".to_string()), ('A', "new.txt".to_string())]);

        let reads = [
            Read::Blob { tree: requests[0].start.clone(), path: "sub dir/a.txt".into() },
            Read::Blob { tree: requests[0].start.clone(), path: "sub dir/new.txt".into() },
            Read::Current("a.txt".into()),
            Read::Current("gone.txt".into()),
        ];
        let (code, out) = run(&home, &read_script(&dir, &reads).unwrap());
        assert_eq!(code, 0);
        let got = parse_reads(&out, reads.len()).unwrap();
        assert_eq!(got[0], Got::Bytes(None, b"one\ntwo\n".to_vec()));
        assert_eq!(got[1], Got::None, "a file the run made has no original");
        assert!(matches!(&got[2], Got::Bytes(Some(_), bytes) if bytes == b"one\nTWO\n"));
        assert_eq!(got[3], Got::None);

        let tag = "0".repeat(32);
        let made = super::super::files::cksum(b"made\n");
        assert_eq!(run(&home, &trash_script(&dir, "new.txt", &tag, (1, 1)).unwrap()).0, 7, "a changed file is not moved");
        assert_eq!(run(&home, &trash_script(&dir, "new.txt", &tag, made).unwrap()).0, 0);
        assert!(!folder.join("new.txt").exists());

        // A folder outside any repository: a store of its own.
        let plain = root.join("plain");
        std::fs::create_dir_all(&plain).unwrap();
        std::fs::write(plain.join("b.txt"), "x\n").unwrap();
        let plain_dir = plain.to_string_lossy().into_owned();
        assert_eq!(run(&home, &sh(&snapshot_script(&plain_dir, "p2", "r2", Moment::Start).unwrap())).0, 0);
        std::fs::write(plain.join("b.txt"), "y\n").unwrap();
        // Still running: compared with the folder now.
        let (_, out) = run(&home, &index_script(&plain_dir, "p2").unwrap());
        let (prefix, requests) = parse_index(&out).unwrap();
        assert_eq!((prefix.as_str(), requests[0].running), ("", true));
        assert_eq!(requests[0].changes, [('M', "b.txt".to_string())]);
        assert!(!plain.join(".git").exists(), "a plain folder gets no repository");
        assert_eq!(run(&home, &index_script(&plain_dir, "nobody").unwrap()).0, 4);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(unix)]
    fn which_git() -> Option<std::path::PathBuf> {
        std::env::split_paths(&std::env::var_os("PATH")?).map(|p| p.join("git")).find(|p| p.is_file())
    }
}
