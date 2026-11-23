//! MC-40: an agent run on an SSH machine that outlives its connection. The CLI is not a child of the ssh that started it: a small
//! holder on the far side starts it detached (`setsid`, else `perl`'s `setsid`, else `nohup`) in a folder of its own,
//! `~/.cache/mighty-code/runs/<id>` (0700), with its standard input a FIFO (`in`) and its output appended to files (`out`, `err`).
//! A `sleep` holds the FIFO open for writing, so the CLI never reads an end of input when a connection goes; the exit code lands in
//! `status`.
//!
//! This computer reads the run through **attaches**: each a batch-mode ssh whose script copies ssh's standard input into the FIFO
//! (through `tee -a inlog`, so a later attach knows how many input bytes arrived) and follows `out` from a byte offset
//! (`tail -c +N -f`). Its first line says how many input bytes arrived (`mighty-attach <n>`); everything after is the CLI's output.
//! When the connection goes, the attach ends (its input reads an end without `in.closed`, or `tail` hits a closed pipe) and the next
//! attach starts where this computer stopped reading, sending again only the input that did not arrive. When the CLI ends, the attach
//! ends with code 0 and a [`drain_script`] reads the last bytes and the exit code; [`cleanup_script`] removes the folder.
//!
//! Every script is POSIX and goes under `/bin/sh -c` ([`super::command::sh`]). The run's id is 32 hex digits this computer chose; no
//! secret is on any command line (the run's command is the same one a direct run sends).

use super::command::{quote, sh};

/// The runs' folder under the far side's home.
pub const FOLDER: &str = ".cache/mighty-code/runs";
/// The first line of every attach's output: `mighty-attach <input bytes that arrived>`.
pub const ATTACH_HEADER: &str = "mighty-attach ";
/// An ended run's folder is removed by a later start after this many days (one this computer drained is removed at once).
pub const KEEP_DAYS: u32 = 7;

/// A run's id: 32 lower-case hex digits.
pub fn valid_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// The run's folder as a shell word (the home outside the quotes).
fn folder(id: &str) -> String {
    format!("\"$HOME\"/{}", quote(&format!("{FOLDER}/{id}")))
}

/// What the holder runs, in the run's folder: the keeper of the FIFO, the CLI (`./cmd`) reading it, the exit code written when the
/// CLI ends, then the keeper ended.
const HOLD: &str = "cd \"${0%/*}\" || exit 1
echo $$ > holder.pid
sleep 2147483647 > in &
echo $! > keeper.pid
/bin/sh ./cmd < in > out 2> err &
c=$!
echo \"$c\" > pid.tmp && mv pid.tmp pid
wait \"$c\"
s=$?
if [ -f snap ]; then /bin/sh ./snap >/dev/null 2>&1; fi
if [ -f after ]; then /bin/sh ./after >/dev/null 2>&1; fi
printf '%s\\n' \"$s\" > status.tmp && mv status.tmp status
kill \"$(cat keeper.pid)\" 2>/dev/null
exit 0";

/// The folder the attachments of SSH runs are uploaded to, under the far side's home (MC-40).
pub const ATTACHMENTS: &str = ".cache/mighty-code/attachments";

/// One attachment of a run uploaded through ssh's standard input into `~/.cache/mighty-code/attachments/<tag>/<name>` (0700 folders,
/// the file made with `set -C`, never an existing one): the first file makes the folder (and removes other runs' folders older than
/// [`KEEP_DAYS`]), the others need it to be there. The bytes' `cksum` must be `written` (exit 6, the file removed). Prints the folder's
/// real path. `name` is the app's own (`<index>-<hex><extension>`).
pub fn upload_script(tag: &str, name: &str, written: (u32, u64), first: bool) -> Option<String> {
    let safe = |n: &str| {
        !n.is_empty()
            && n.len() <= 80
            && !n.starts_with(['.', '-'])
            && n.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
    };
    if !valid_id(tag) || !safe(name) {
        return None;
    }
    let make = if first {
        format!(
            "for d in \"$R\"/*; do case \"${{d##*/}}\" in *[!0-9a-f]*) continue ;; esac\n\
             if [ -n \"$(find \"$d\" -prune -mtime +{KEEP_DAYS} 2>/dev/null)\" ]; then rm -rf \"$d\"; fi; done\nmkdir \"$A\" || exit 5"
        )
    } else {
        "[ -d \"$A\" ] && [ ! -h \"$A\" ] || exit 5".to_string()
    };
    let (crc, size) = written;
    Some(sh(&format!(
        "umask 077\nR=\"$HOME\"/{root}\nmkdir -p \"$R\" || exit 5\nA=\"$R\"/{tag}\n{make}\ncd \"$A\" || exit 5\nset -C\n\
         cat > ./{name} || exit 5\nset +C\nset -- $(LC_ALL=C cksum < ./{name})\n\
         [ \"$1 $2\" = '{crc} {size}' ] || {{ rm -f ./{name}; exit 6; }}\npwd -P",
        root = quote(ATTACHMENTS),
        tag = quote(tag),
        name = quote(name),
    )))
}

/// Where a held run's request keeps its snapshots (MC-40): the workspace folder, the pane and the request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshots {
    pub dir: String,
    pub pane: String,
    pub run: String,
}

/// Starts a held run of `run_script` (mighty-core `command::run_script`: the CLI in the workspace's folder) as `id`; prints
/// `started` once the CLI is running. Exit 5 when the folder cannot be made (or `id` is taken), 6 when the CLI did not start. Ended
/// runs older than [`KEEP_DAYS`] are removed on the way.
pub fn start_script(id: &str, run_script: &str, snapshots: Option<&Snapshots>) -> Option<String> {
    start_script_with(id, run_script, snapshots, None)
}

/// [`start_script`], and (MC-40) the run's uploaded attachments ([`upload_script`]'s folder `attachments`) removed once its CLI ended.
pub fn start_script_with(id: &str, run_script: &str, snapshots: Option<&Snapshots>, attachments: Option<&str>) -> Option<String> {
    if !valid_id(id) || attachments.is_some_and(|tag| !valid_id(tag)) {
        return None;
    }
    let after = match attachments {
        Some(tag) => format!("printf '%s\\n' {} > after\n", quote(&format!("rm -rf \"$HOME\"/{}", quote(&format!("{ATTACHMENTS}/{tag}"))))),
        None => String::new(),
    };
    // MC-40: the request's snapshots (mighty-core `ssh::review`): the start one before the CLI starts, the end one left for the holder.
    let (start, end) = match snapshots {
        Some(s) => (
            super::review::snapshot_script(&s.dir, &s.pane, &s.run, super::review::Moment::Start)?,
            super::review::snapshot_script(&s.dir, &s.pane, &s.run, super::review::Moment::End)?,
        ),
        None => (String::new(), String::new()),
    };
    let snapshots = if snapshots.is_some() {
        format!("printf '%s\\n' {} > snap\n(\n{}\n) >/dev/null 2>&1 </dev/null || :\n", quote(&end), start)
    } else {
        String::new()
    };
    Some(sh(&format!(
        "umask 077
R=\"$HOME\"/{root}
mkdir -p \"$R\" || exit 5
for d in \"$R\"/*; do
case \"${{d##*/}}\" in *[!0-9a-f]*) continue ;; esac
if [ -f \"$d/status\" ] && [ -n \"$(find \"$d/status\" -mtime +{days} 2>/dev/null)\" ]; then rm -rf \"$d\"; fi
done
H={folder}
mkdir \"$H\" || exit 5
cd \"$H\" || exit 5
printf '%s\\n' {cmd} > cmd
printf '%s\\n' {hold} > hold
{snapshots}{after}mkfifo in || exit 5
: > out
: > err
: > inlog
if command -v setsid >/dev/null 2>&1; then setsid /bin/sh ./hold </dev/null >/dev/null 2>&1 &
elif command -v perl >/dev/null 2>&1; then perl -e 'use POSIX; POSIX::setsid(); exec @ARGV' /bin/sh ./hold </dev/null >/dev/null 2>&1 &
else nohup /bin/sh ./hold </dev/null >/dev/null 2>&1 &
fi
i=0
while [ ! -f pid ] && [ \"$i\" -lt 100 ]; do sleep 0.1 2>/dev/null || sleep 1; i=$((i+1)); done
[ -f pid ] || exit 6
printf started",
        root = quote(FOLDER),
        days = KEEP_DAYS,
        folder = folder(id),
        cmd = quote(run_script),
        hold = quote(HOLD),
    )))
}

/// One attach from byte `offset` of the CLI's output (see the module). Exit 3 when the run's folder is gone, 4 when the connection
/// went (its input ended without the run's input being closed, or its output could not be written), 0 when the CLI ended.
pub fn attach_script(id: &str, offset: u64) -> Option<String> {
    if !valid_id(id) {
        return None;
    }
    Some(sh(&format!(
        "cd {folder} 2>/dev/null || exit 3
[ -f pid ] || exit 3
printf '{ATTACH_HEADER}%s\\n' \"$(wc -c < inlog | tr -d ' ')\"
if [ -f status ]; then tail -c +{from} out; exit 0; fi
c=
if [ ! -f in.closed ]; then
exec 3<&0
(cat <&3 | tee -a inlog > in) 2>/dev/null &
c=$!
exec 3<&-
fi
tail -c +{from} -f out &
t=$!
while [ ! -f status ]; do
if ! kill -0 \"$t\" 2>/dev/null; then kill $c 2>/dev/null; exit 4; fi
if [ -n \"$c\" ] && ! kill -0 \"$c\" 2>/dev/null; then
if [ -f in.closed ]; then c=; else kill \"$t\" 2>/dev/null; exit 4; fi
fi
sleep 1
done
sleep 1
kill \"$t\" $c 2>/dev/null
exit 0",
        folder = folder(id),
        from = offset + 1,
    )))
}

/// After an attach: `status=<code>` and the output from `offset` when the CLI ended (its last 64 KiB of standard error on standard
/// error), else `running`. Exit 3 when the folder is gone.
pub fn drain_script(id: &str, offset: u64) -> Option<String> {
    valid_id(id).then(|| {
        sh(&format!(
            "cd {folder} 2>/dev/null || exit 3\nif [ -f status ]; then printf 'status=%s\\n' \"$(cat status)\"; tail -c +{from} out; \
             tail -c 65536 err >&2; else printf 'running\\n'; fi",
            folder = folder(id),
            from = offset + 1,
        ))
    })
}

/// The run's input closed (the CLI reads its end once no attach writes): `in.closed` marked, the FIFO's keeper ended.
pub fn close_input_script(id: &str) -> Option<String> {
    valid_id(id).then(|| {
        sh(&format!("cd {} 2>/dev/null || exit 3\n: > in.closed\nkill \"$(cat keeper.pid 2>/dev/null)\" 2>/dev/null\nexit 0", folder(id)))
    })
}

/// Stops the CLI: `TERM`, then `KILL` after five seconds; the exit code written if the holder could not.
pub fn kill_script(id: &str) -> Option<String> {
    valid_id(id).then(|| {
        sh(&format!(
            "cd {} 2>/dev/null || exit 3
[ -f status ] && exit 0
: > in.closed
p=$(cat pid 2>/dev/null)
[ -n \"$p\" ] && kill -TERM \"$p\" 2>/dev/null
i=0
while [ ! -f status ] && [ \"$i\" -lt 5 ]; do sleep 1; i=$((i+1)); done
if [ ! -f status ]; then [ -n \"$p\" ] && kill -KILL \"$p\" 2>/dev/null; sleep 1; fi
[ -f status ] || printf '137\\n' > status
kill \"$(cat keeper.pid 2>/dev/null)\" 2>/dev/null
exit 0",
            folder(id)
        ))
    })
}

/// Removes an ended run's folder (never one still running).
pub fn cleanup_script(id: &str) -> Option<String> {
    valid_id(id).then(|| sh(&format!("cd {} 2>/dev/null || exit 0\n[ -f status ] || exit 0\ncd .. && rm -rf ./{id}", folder(id))))
}

/// An attach's first line read: how many input bytes arrived.
pub fn parse_header(line: &str) -> Option<u64> {
    line.trim_end().strip_prefix(ATTACH_HEADER)?.trim().parse().ok()
}

/// A drain's output read: the exit code and the output after its first line, or None while the CLI runs (or for anything else).
pub fn parse_drain(mut stdout: Vec<u8>) -> Option<(i32, Vec<u8>)> {
    let end = stdout.iter().position(|b| *b == b'\n')?;
    let line = String::from_utf8_lossy(&stdout[..end]).into_owned();
    let code = line.strip_prefix("status=")?.trim().parse().ok()?;
    Some((code, stdout.split_off(end + 1)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_hex_only_and_every_script_is_one_sh_word() {
        let id = "0123456789abcdef0123456789abcdef";
        assert!(valid_id(id) && !valid_id("0123") && !valid_id(&"G".repeat(32)) && !valid_id(&"A".repeat(32)));
        assert_eq!(start_script("x'; rm -rf ~", "true", None), None);
        for script in [
            start_script(id, "exec true", None).unwrap(),
            attach_script(id, 10).unwrap(),
            drain_script(id, 10).unwrap(),
            close_input_script(id).unwrap(),
            kill_script(id).unwrap(),
            cleanup_script(id).unwrap(),
        ] {
            assert!(script.starts_with("/bin/sh -c '"), "{script}");
            assert!(super::super::command::unwrap_sh(&script).is_some());
        }
        assert_eq!(parse_header("mighty-attach 42\n"), Some(42));
        assert_eq!(parse_header("hello"), None);
        assert_eq!(parse_drain(b"status=3\nlast".to_vec()), Some((3, b"last".to_vec())));
        assert_eq!(parse_drain(b"running\n".to_vec()), None);
    }

    /// The scripts in a real `/bin/sh` with a home of the check's own: a run started, attached, its connection "lost" (the attach's
    /// input ended without a close), attached again from where reading stopped, its input closed, drained, cleaned up.
    #[cfg(unix)]
    #[test]
    fn a_held_run_goes_on_without_its_connection() {
        use std::io::{Read as _, Write as _};
        #[allow(clippy::disallowed_methods, reason = "a test runs the far side's scripts in a real shell")]
        fn command(home: &std::path::Path, script: &str) -> std::process::Command {
            let mut command = std::process::Command::new("/bin/sh");
            command.arg("-c").arg(script).env("HOME", home);
            command
        }
        let home = std::env::temp_dir().join(format!("mc-ssh-held-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&home).unwrap();
        let id = uuid::Uuid::new_v4().simple().to_string();
        // The "CLI": echoes each input line as `got:<line>` until its input ends, then exits 3.
        let cli = "while IFS= read -r line; do printf 'got:%s\\n' \"$line\"; done; echo bye; exit 3";
        let output = command(&home, &start_script(&id, cli, None).unwrap()).output().unwrap();
        assert_eq!((output.status.code(), output.stdout.as_slice()), (Some(0), &b"started"[..]), "{output:?}");

        // First attach: one line in, its answer out, then the input ends (the connection went): the attach says so (4).
        let mut first = command(&home, &attach_script(&id, 0).unwrap())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        first.stdin.as_mut().unwrap().write_all(b"one\n").unwrap();
        let mut stdout = first.stdout.take().unwrap();
        let mut seen = Vec::new();
        let mut buffer = [0u8; 256];
        while !String::from_utf8_lossy(&seen).contains("got:one\n") {
            let n = stdout.read(&mut buffer).unwrap();
            assert!(n > 0, "{}", String::from_utf8_lossy(&seen));
            seen.extend_from_slice(&buffer[..n]);
        }
        assert!(seen.starts_with(b"mighty-attach 0\n"), "{}", String::from_utf8_lossy(&seen));
        drop(first.stdin.take());
        assert_eq!(first.wait().unwrap().code(), Some(4), "a lost input is a lost connection");
        let offset = (seen.len() - "mighty-attach 0\n".len()) as u64;

        // The run went on: a second attach knows the 4 input bytes arrived and reads on from the offset.
        let mut second = command(&home, &attach_script(&id, offset).unwrap())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        second.stdin.as_mut().unwrap().write_all(b"two\n").unwrap();
        // The input closed as the app closes it: marked first, then this attach's input ends; the CLI reads its end and exits.
        std::thread::sleep(std::time::Duration::from_millis(500));
        assert_eq!(command(&home, &close_input_script(&id).unwrap()).status().unwrap().code(), Some(0));
        drop(second.stdin.take());
        let out = second.wait_with_output().unwrap();
        assert_eq!(out.status.code(), Some(0));
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        assert!(text.starts_with("mighty-attach 4\n"), "{text}");
        let read = text.len() - "mighty-attach 4\n".len();
        let drained = command(&home, &drain_script(&id, offset + read as u64).unwrap()).output().unwrap();
        let (code, rest) = parse_drain(drained.stdout).unwrap();
        let whole = format!("{}{}", &text["mighty-attach 4\n".len()..], String::from_utf8_lossy(&rest));
        assert_eq!((code, whole.as_str()), (3, "got:two\nbye\n"));
        assert_eq!(command(&home, &cleanup_script(&id).unwrap()).status().unwrap().code(), Some(0));
        assert!(!home.join(FOLDER).join(&id).exists());
        std::fs::remove_dir_all(&home).unwrap();
    }

    /// A run stopped from here ends with its exit code written even when it ignores `TERM`.
    #[cfg(unix)]
    #[test]
    fn a_held_run_is_stopped() {
        #[allow(clippy::disallowed_methods, reason = "a test runs the far side's scripts in a real shell")]
        fn run(home: &std::path::Path, script: &str) -> std::process::Output {
            std::process::Command::new("/bin/sh").arg("-c").arg(script).env("HOME", home).output().unwrap()
        }
        let home = std::env::temp_dir().join(format!("mc-ssh-held-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&home).unwrap();
        let id = uuid::Uuid::new_v4().simple().to_string();
        assert_eq!(run(&home, &start_script(&id, "exec sleep 600", None).unwrap()).status.code(), Some(0));
        assert_eq!(run(&home, &kill_script(&id).unwrap()).status.code(), Some(0));
        let (code, _) = parse_drain(run(&home, &drain_script(&id, 0).unwrap()).stdout).unwrap();
        assert_ne!(code, 0);
        assert_eq!(run(&home, &start_script(&id, "true", None).unwrap()).status.code(), Some(5), "an id is never reused");
        run(&home, &cleanup_script(&id).unwrap());
        std::fs::remove_dir_all(&home).unwrap();
    }

    /// MC-40: a held run's request snapshotted before its CLI starts and after it ended: the review reads what the CLI changed.
    #[cfg(unix)]
    #[test]
    fn a_held_run_is_snapshotted_around_its_cli() {
        #[allow(clippy::disallowed_methods, reason = "a test runs the far side's scripts in a real shell")]
        fn run(home: &std::path::Path, script: &str) -> std::process::Output {
            std::process::Command::new("/bin/sh").arg("-c").arg(script).env("HOME", home).output().unwrap()
        }
        let home = std::env::temp_dir().join(format!("mc-ssh-held-snap-{}", uuid::Uuid::new_v4().simple()));
        let folder = home.join("work");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("a.txt"), "before\n").unwrap();
        let dir = folder.to_string_lossy().into_owned();
        let id = uuid::Uuid::new_v4().simple().to_string();
        let snapshots = Snapshots { dir: dir.clone(), pane: "pane-1".into(), run: "run-1".into() };
        let cli = format!("cd {} && printf 'after\\n' > a.txt && exit 0", super::super::command::quote_dir(&dir));
        let output = run(&home, &start_script(&id, &cli, Some(&snapshots)).unwrap());
        assert_eq!(output.status.code(), Some(0), "{output:?}");
        let mut ended = None;
        for _ in 0..100 {
            if let Some(found) = parse_drain(run(&home, &drain_script(&id, 0).unwrap()).stdout) {
                ended = Some(found);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        assert_eq!(ended.map(|(code, _)| code), Some(0));
        let index = run(&home, &super::super::review::index_script(&dir, "pane-1").unwrap());
        let (_, requests) = super::super::review::parse_index(&index.stdout).unwrap();
        assert!(!requests[0].running, "the end was snapshotted by the holder");
        assert_eq!(requests[0].changes, [('M', "a.txt".to_string())]);
        std::fs::remove_dir_all(&home).unwrap();
    }

    /// MC-40: attachments uploaded through standard input into a new 0700 folder (a torn upload refused and removed), and the folder
    /// gone once the run's CLI ended.
    #[cfg(unix)]
    #[test]
    fn attachments_are_uploaded_and_removed_with_their_run() {
        use std::io::Write as _;
        use std::os::unix::fs::PermissionsExt as _;
        #[allow(clippy::disallowed_methods, reason = "a test runs the far side's scripts in a real shell")]
        fn run(home: &std::path::Path, script: &str, input: &[u8]) -> (i32, String) {
            let mut child = std::process::Command::new("/bin/sh")
                .arg("-c")
                .arg(script)
                .env("HOME", home)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            let _ = child.stdin.take().unwrap().write_all(input);
            let output = child.wait_with_output().unwrap();
            (output.status.code().unwrap_or(-1), String::from_utf8_lossy(&output.stdout).into_owned())
        }
        let home = std::env::temp_dir().join(format!("mc-ssh-attach-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&home).unwrap();
        let tag = uuid::Uuid::new_v4().simple().to_string();
        let cksum = super::super::files::cksum;
        assert_eq!(upload_script(&tag, "../x", (0, 0), true), None);
        let (code, folder) = run(&home, &upload_script(&tag, "0-ab.png", cksum(b"png bytes"), true).unwrap(), b"png bytes");
        assert_eq!(code, 0);
        let folder = std::path::PathBuf::from(folder.trim());
        assert!(folder.ends_with(format!("{ATTACHMENTS}/{tag}")), "{folder:?}");
        assert_eq!(std::fs::read(folder.join("0-ab.png")).unwrap(), b"png bytes");
        assert_eq!(std::fs::metadata(&folder).unwrap().permissions().mode() & 0o777, 0o700);
        assert_eq!(run(&home, &upload_script(&tag, "1-cd.txt", cksum(b"whole"), false).unwrap(), b"who").0, 6);
        assert!(!folder.join("1-cd.txt").exists(), "a torn upload is removed");
        assert_eq!(run(&home, &upload_script(&tag, "0-ab.png", cksum(b"x"), false).unwrap(), b"x").0, 5, "never over a file");
        assert_eq!(run(&home, &upload_script(&tag, "2-ef.png", cksum(b"x"), true).unwrap(), b"x").0, 5, "the folder is made once");

        let id = uuid::Uuid::new_v4().simple().to_string();
        let (code, _) = run(&home, &start_script_with(&id, "exit 0", None, Some(&tag)).unwrap(), b"");
        assert_eq!(code, 0);
        for _ in 0..100 {
            if !folder.exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        assert!(!folder.exists(), "the attachments go when the run's CLI ended");
        std::fs::remove_dir_all(&home).unwrap();
    }
}
