//! MC-40: an SSH machine whose login shell is not POSIX (fish, csh, tcsh). A stand-in `ssh` hands the remote command to the far
//! side's login shell as sshd does (`<shell> -c <command>`), in an environment of its own; the shells are a fish-like stand-in (it
//! runs only a one-word `/bin/sh -c '…'` with no backslash or line break in it, and fails on POSIX quoting handed to `-lc`), a
//! csh-like one (it refuses a line break or `!`), and this computer's real `/bin/csh` and `/bin/tcsh` where it has them. Through each:
//! a run's program gets its arguments, a shell pane's command runs, and the editor's open, write and check work. Never a real host.
#![cfg(unix)]

mod common;

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use mighty_core::ssh::files as remote;
use mighty_core::ssh::{Mux, Target};
use mighty_run::ssh::SshLaunch;

fn script(path: &Path, text: &str) {
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
}

struct Machine {
    root: PathBuf,
    ssh: PathBuf,
    dir: PathBuf,
}

impl Machine {
    /// `login`: the far side's login shell (a path).
    fn new(name: &str, login: &Path) -> Machine {
        let root = common::temp(name);
        let bin = root.join("remote-bin");
        std::fs::create_dir_all(&bin).unwrap();
        let dir = root.join("it's my \"app\"");
        std::fs::create_dir_all(&dir).unwrap();
        script(&bin.join("mcprobe"), "#!/bin/sh\nprintf '%s|' \"$@\"; pwd\n");
        let ssh = root.join("ssh");
        script(
            &ssh,
            &format!(
                "#!/bin/sh\n{decode}\n\
                 for a in \"$@\"; do printf '%s\\n' \"$a\"; done >> '{root}/ssh.args'\n\
                 while [ \"$#\" -gt 0 ]; do if [ \"$1\" = \"--\" ]; then shift; break; fi; shift; done\nshift\n\
                 case \"$(decode \"$1\")\" in *'/Applications/MightyCode'*|*MightyCode-local*) echo 'the installed app is never run' >&2; exit 99;; esac\n\
                 exec /usr/bin/env -i SHELL='{login}' PATH='{bin}':/usr/bin:/bin HOME='{root}' '{login}' -c \"$1\"\n",
                decode = common::DECODE_REMOTE,
                root = root.display(),
                login = login.display(),
                bin = bin.display(),
            ),
        );
        Machine { root, ssh, dir }
    }

    fn launch(&self) -> SshLaunch {
        SshLaunch {
            program: self.ssh.clone(),
            target: Target::parse("devbox").unwrap(),
            mux: Mux::Shared("/tmp/mc-ssh-shells".into()),
            dir: self.dir.to_string_lossy().into_owned(),
            local: self.root.clone(),
        }
    }
}

/// The stand-in login shells, written once in `folder`: a fish-like and a csh-like one.
fn stand_in_shells(folder: &Path) -> Vec<PathBuf> {
    std::fs::create_dir_all(folder).unwrap();
    let fish = folder.join("fish");
    script(
        &fish,
        "#!/bin/sh\ncase \"$1\" in\n\
         -c) case \"$2\" in '/bin/sh -c '*) ;; *) echo 'fish: not one sh word' >&2; exit 97;; esac\n\
         case \"$2\" in *\"\\\\\"*) echo 'fish: a backslash in quotes' >&2; exit 98;; esac\n\
         [ \"$(printf '%s' \"$2\" | wc -l | tr -d ' ')\" = 0 ] || { echo 'fish: a line break' >&2; exit 98; }\n\
         exec /bin/sh -c \"$2\";;\n\
         -lc) case \"$2\" in *\"'\\\\''\"*) echo 'fish: POSIX quoting' >&2; exit 96;; esac; echo 'Welcome to fish'; exec /bin/sh -c \"$2\";;\n\
         *) exit 95;;\nesac\n",
    );
    let csh = folder.join("csh");
    script(
        &csh,
        "#!/bin/sh\ncase \"$1\" in\n\
         -c) [ \"$(printf '%s' \"$2\" | wc -l | tr -d ' ')\" = 0 ] || { echo 'Unmatched '\"'\"'.' >&2; exit 1; }\n\
         case \"$2\" in *'!'*) echo 'Event not found.' >&2; exit 1;; esac\nexec /bin/sh -c \"$2\";;\n\
         -lc) echo 'Unknown option: -lc' >&2; exit 1;;\n\
         *) exit 95;;\nesac\n",
    );
    let mut shells = vec![fish, csh];
    for real in ["/bin/csh", "/bin/tcsh"] {
        if Path::new(real).is_file() {
            shells.push(PathBuf::from(real));
        }
    }
    shells
}

async fn exec(launch: &SshLaunch, command: String, input: Option<Vec<u8>>) -> (i32, Vec<u8>, String) {
    let request = launch.exec_request(command, &BTreeMap::new());
    let captured =
        mighty_platform::process::capture(common::processes().as_ref(), request, input, Duration::from_secs(20), 4 << 20).await.unwrap();
    (captured.exit_code, captured.stdout, String::from_utf8_lossy(&captured.stderr).into_owned())
}

async fn run(launch: &SshLaunch, request: mighty_platform::SpawnRequest) -> (i32, String, String) {
    let captured =
        mighty_platform::process::capture(common::processes().as_ref(), request, None, Duration::from_secs(20), 1 << 20).await.unwrap();
    let _ = launch;
    (captured.exit_code, String::from_utf8_lossy(&captured.stdout).into_owned(), String::from_utf8_lossy(&captured.stderr).into_owned())
}

#[tokio::test(flavor = "multi_thread")]
async fn non_posix_login_shells_run_every_remote_command() {
    let shells = stand_in_shells(&common::temp("ssh-login-shells"));
    for (index, login) in shells.iter().enumerate() {
        let machine = Machine::new(&format!("ssh-shell-{index}"), login);
        let launch = machine.launch();
        let name = login.display().to_string();

        // A run: the program on the login shell's PATH, its arguments exactly, in the remote folder.
        let args = vec!["it's".to_string(), "back\\slash\nline".to_string(), "$HOME!".to_string()];
        let request = launch.run_request("mcprobe", &args, &BTreeMap::new()).unwrap();
        let (code, out, err) = run(&launch, request).await;
        assert_eq!(
            (code, out.as_str()),
            (0, format!("it's|back\\slash\nline|$HOME!|{}\n", machine.dir.display()).as_str()),
            "{name}: {err}"
        );

        // A shell pane's command, written for the person's login shell.
        let request = launch.shell_request("printf here", &BTreeMap::new());
        let (code, out, err) = run(&launch, request).await;
        assert_eq!((code, out.trim_start_matches("Welcome to fish\n")), (0, "here"), "{name}: {err}");

        // The editor: open, write (whole, permissions kept), a conflicting write refused, the check.
        let file = machine.dir.join("notes 한글.txt");
        std::fs::write(&file, b"a\r\nb\r\n").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o640)).unwrap();
        let relative = "notes 한글.txt";
        let (code, out, err) = exec(&launch, remote::open_command(&launch.dir, relative).unwrap(), None).await;
        assert_eq!(code, 0, "{name}: {err}");
        let opened = remote::parse_opened(out).unwrap();
        assert_eq!((opened.mode, opened.bytes.as_slice()), (0o640, &b"a\r\nb\r\n"[..]), "{name}");
        let new = b"x\r\ny!\r\n".to_vec();
        let write = remote::Write {
            relative: relative.into(),
            tag: format!("{index:0>32}"),
            mode: opened.mode,
            expected: Some(remote::cksum(&opened.bytes)),
            written: remote::cksum(&new),
        };
        let (code, out, err) = exec(&launch, remote::write_command(&launch.dir, &write).unwrap(), Some(new.clone())).await;
        assert_eq!((code, out.as_slice()), (0, &b"saved"[..]), "{name}: {err}");
        assert_eq!(std::fs::read(&file).unwrap(), new);
        assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o640);
        let stale = remote::Write { written: remote::cksum(b"z"), ..write.clone() };
        let (code, _, _) = exec(&launch, remote::write_command(&launch.dir, &stale).unwrap(), Some(b"z".to_vec())).await;
        assert_eq!(remote::Ended::of(code), remote::Ended::Changed, "{name}");
        assert_eq!(std::fs::read(&file).unwrap(), new);
        let (code, out, _) = exec(&launch, remote::check_command(&launch.dir, relative).unwrap(), None).await;
        assert_eq!((code, remote::parse_check(&String::from_utf8_lossy(&out))), (0, Some(remote::cksum(&new))), "{name}");

        // Nothing of the file was ever on a command line.
        let sent = std::fs::read_to_string(machine.root.join("ssh.args")).unwrap();
        assert!(!sent.contains("y!") && !sent.contains("x\r\n"), "{name}");
        common::remove_when_free(&machine.root).await;
    }
}

/// MC-40: a held run through a non-POSIX login shell: its output read through an attach, its input closed, its exit code taken; a
/// held run stopped from here ends over there too.
#[tokio::test(flavor = "multi_thread")]
async fn a_held_run_works_through_a_non_posix_login_shell_and_is_stopped() {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let shells = stand_in_shells(&common::temp("ssh-held-shells"));
    let machine = Machine::new("ssh-held-fish", &shells[0]);
    let launch = machine.launch();
    let script = mighty_core::ssh::command::run_script(&launch.dir, "mcprobe", &["one".into()], &BTreeMap::new()).unwrap();
    let cat = format!("cd {} || exit 1\nprintf 'ready\\n'; cat; exit 4", mighty_core::ssh::command::quote_dir(&launch.dir));
    let mut child = mighty_run::ssh_held::spawn(common::processes(), launch.clone(), &cat, "test", None, None).await.unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(b"echoed\n").await.unwrap();
    drop(stdin);
    let mut out = String::new();
    child.stdout.take().unwrap().read_to_string(&mut out).await.unwrap();
    assert_eq!(out, "ready\nechoed\n");
    assert_eq!(child.wait().await.unwrap(), 4);

    let probe = mighty_run::ssh_held::spawn(common::processes(), launch.clone(), &script, "test", None, None).await.unwrap();
    let mut text = String::new();
    let mut stdout = probe.stdout.unwrap();
    stdout.read_to_string(&mut text).await.unwrap();
    assert_eq!(text, format!("one|{}\n", machine.dir.display()));

    let waiting = mighty_run::ssh_held::spawn(common::processes(), launch, "exec sleep 600", "test", None, None).await.unwrap();
    let control = waiting.control();
    tokio::time::timeout(Duration::from_secs(30), control.close()).await.unwrap().unwrap();
    assert_ne!(control.wait().await.unwrap(), 0);
    let runs = machine.root.join(".cache/mighty-code/runs");
    let left: Vec<_> = std::fs::read_dir(&runs).map(|r| r.flatten().collect()).unwrap_or_default();
    assert!(left.is_empty(), "every ended run's folder is removed: {left:?}");
    common::remove_when_free(&machine.root).await;
}
