//! The far side's command lines and the local `ssh` argument arrays.
//!
//! Locally ssh always starts from an argument array (never a shell string). The one string is the remote command ssh hands to the far
//! side's login shell; every path, program and argument in it is single-quoted for a POSIX shell ([`quote`]), and a folder under the
//! remote home keeps its `~` outside the quotes so the far side expands it. Every array ends `-- <destination> <command>`, the
//! destination validated ([`Target`]), so nothing the user typed can be an ssh option.
//!
//! Where the local ssh has ControlMaster ([`Mux`], not Windows' OpenSSH), every pane of a machine shares one connection:
//! `-o ControlMaster=auto -o ControlPath=<dir>/cm-%C -o ControlPersist=10m`.

use std::collections::BTreeMap;

use super::target::Target;

/// How long a shared connection stays after its last pane.
pub const CONTROL_PERSIST: &str = "10m";
/// The connection test's limit on reaching the host (seconds; the whole test has its own limit in the app).
pub const CONNECT_TIMEOUT_SECONDS: u32 = 10;
/// Keep-alives: a connection silent for 3 × 15 s is dropped, so a pane hears of a lost link without the app polling anything.
pub const ALIVE_INTERVAL_SECONDS: u32 = 15;
pub const ALIVE_COUNT: u32 = 3;
/// The exit code ssh gives when the connection failed or was lost (a remote command's own codes are passed through).
pub const SSH_FAILED: i32 = 255;

/// The ControlMaster socket template, or none (Windows' OpenSSH has no ControlMaster: one connection per pane).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mux {
    /// The folder the sockets live in (0700): `<folder>/cm-%C`.
    Shared(String),
    None,
}

impl Mux {
    fn args(&self) -> Vec<String> {
        match self {
            Mux::Shared(folder) => {
                let separator = if folder.contains('\\') && !folder.contains('/') { '\\' } else { '/' };
                let folder = folder.trim_end_matches(['/', '\\']);
                vec![
                    "-o".into(),
                    "ControlMaster=auto".into(),
                    "-o".into(),
                    format!("ControlPath={folder}{separator}cm-%C"),
                    "-o".into(),
                    format!("ControlPersist={CONTROL_PERSIST}"),
                ]
            }
            Mux::None => vec![],
        }
    }

    pub fn shared(&self) -> bool {
        matches!(self, Mux::Shared(_))
    }
}

/// `text` single-quoted for a POSIX shell: `'it'\''s'`. Always quoted, so an empty string stays one argument.
pub fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('\'');
    for c in text.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

/// A remote folder for `cd`: `~` and `~/rest` keep the tilde outside the quotes so the far side expands it; anything else is quoted
/// whole.
pub fn quote_dir(dir: &str) -> String {
    if dir == "~" {
        return "~".into();
    }
    if let Some(rest) = dir.strip_prefix("~/") {
        return if rest.is_empty() { "~/".into() } else { format!("~/{}", quote(rest)) };
    }
    quote(dir)
}

/// A program the far side finds on its `PATH`: a bare name (`claude`, `codex`, `agy`), no folder or shell character.
pub fn valid_program(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with(['-', '.'])
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// An environment name passed to the far side.
pub fn valid_env_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes.next().is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
        && name.len() <= 128
}

/// A tmux session name for a pane: `mighty-` and the pane id's letters and digits (at most 32).
pub fn tmux_session(pane: &str) -> String {
    let id: String = pane.chars().filter(char::is_ascii_alphanumeric).take(32).collect();
    format!("mighty-{id}")
}

/// What a terminal pane runs on the far side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shell {
    /// The login shell in the folder: `cd <dir> && exec $SHELL -l`.
    Login,
    /// Inside tmux, so the work survives a lost connection and a reconnect attaches again:
    /// `cd <dir> && exec tmux new-session -A -s <session>`.
    Tmux(String),
}

/// The remote command of a terminal pane.
pub fn shell_command(dir: &str, shell: &Shell) -> String {
    match shell {
        Shell::Login => format!("cd {} && exec $SHELL -l", quote_dir(dir)),
        Shell::Tmux(session) => format!("cd {} && exec tmux new-session -A -s {}", quote_dir(dir), quote(session)),
    }
}

/// The remote command of a run: the program found on the login shell's `PATH` (`$SHELL -lc`), started in the folder with `env`'s
/// values. Err when the program or a name is not one the far side may be given.
pub fn run_command(dir: &str, program: &str, args: &[String], env: &BTreeMap<String, String>) -> Result<String, &'static str> {
    if !valid_program(program) {
        return Err("program");
    }
    let mut inner = String::from("exec");
    if !env.is_empty() {
        inner.push_str(" env");
        for (name, value) in env {
            if !valid_env_name(name) || value.contains('\0') {
                return Err("environment");
            }
            inner.push(' ');
            inner.push_str(&quote(&format!("{name}={value}")));
        }
    }
    inner.push(' ');
    inner.push_str(&quote(program));
    for arg in args {
        if arg.contains('\0') {
            return Err("argument");
        }
        inner.push(' ');
        inner.push_str(&quote(arg));
    }
    Ok(format!("cd {} && exec $SHELL -lc {}", quote_dir(dir), quote(&inner)))
}

/// The connection test's remote command: the OS and machine, the login shell, and whether tmux is there.
pub const PROBE_COMMAND: &str = "uname -sm; echo \"$SHELL\"; command -v tmux || echo no-tmux";

/// The options every connection has: keep-alives and, for a run or a read, no prompt at all (batch mode: no password, no host-key
/// question; an unknown host key fails rather than being accepted).
fn base(target: &Target, mux: &Mux, batch: bool) -> Vec<String> {
    let mut args = Vec::new();
    if batch {
        args.extend(["-o".to_string(), "BatchMode=yes".to_string()]);
    }
    args.extend([
        "-o".to_string(),
        format!("ServerAliveInterval={ALIVE_INTERVAL_SECONDS}"),
        "-o".to_string(),
        format!("ServerAliveCountMax={ALIVE_COUNT}"),
    ]);
    args.extend(mux.args());
    args.extend(target.port_args());
    args
}

fn finish(mut args: Vec<String>, target: &Target, command: Option<String>) -> Vec<String> {
    args.push("--".into());
    args.push(target.destination());
    if let Some(command) = command {
        args.push(command);
    }
    args
}

/// A terminal pane: `-tt` (a terminal even though ssh's own input is the pane's PTY), prompts allowed in the pane (a password or a
/// new host's key the person answers there; nothing is stored or accepted for them).
pub fn terminal_args(target: &Target, mux: &Mux, dir: &str, shell: &Shell) -> Vec<String> {
    let mut args = base(target, mux, false);
    args.push("-tt".into());
    finish(args, target, Some(shell_command(dir, shell)))
}

/// An agent run: no terminal (`-T`, the CLI's JSON streams through the pipes as they are), batch mode.
pub fn run_args(target: &Target, mux: &Mux, remote_command: String) -> Vec<String> {
    let mut args = base(target, mux, true);
    args.push("-T".into());
    finish(args, target, Some(remote_command))
}

/// The connection test: batch mode, a connect timeout, never the shared connection (a fresh sign-in is what is tested).
pub fn test_args(target: &Target) -> Vec<String> {
    let mut args = base(target, &Mux::None, true);
    args.extend(["-o".to_string(), format!("ConnectTimeout={CONNECT_TIMEOUT_SECONDS}"), "-T".to_string()]);
    finish(args, target, Some(PROBE_COMMAND.into()))
}

/// A short read-only command (a file read, a listing, `git -C … diff`): batch mode, no terminal.
pub fn exec_args(target: &Target, mux: &Mux, remote_command: String) -> Vec<String> {
    let mut args = base(target, mux, true);
    args.extend(["-o".to_string(), format!("ConnectTimeout={CONNECT_TIMEOUT_SECONDS}"), "-T".to_string()]);
    finish(args, target, Some(remote_command))
}

/// The `-L` value: only this computer's loopback listens, the far side's `localhost:<remote>` is reached.
pub fn forward_spec(local: u16, remote: u16) -> String {
    format!("127.0.0.1:{local}:localhost:{remote}")
}

/// A port forward held by its own ssh (`-N`: no command), failing at once when the local port cannot be had.
pub fn forward_args(target: &Target, mux: &Mux, local: u16, remote: u16) -> Vec<String> {
    let mut args = base(target, mux, true);
    args.extend([
        "-N".to_string(),
        "-T".to_string(),
        "-o".to_string(),
        "ExitOnForwardFailure=yes".to_string(),
        "-L".to_string(),
        forward_spec(local, remote),
    ]);
    finish(args, target, None)
}

/// Taking a forward back from the shared connection (a forward a multiplexed client asked for belongs to the master and outlives the
/// client); None without ControlMaster, where ending the forward's own ssh is enough.
pub fn cancel_forward_args(target: &Target, mux: &Mux, local: u16, remote: u16) -> Option<Vec<String>> {
    if !mux.shared() {
        return None;
    }
    let mut args = mux.args();
    args.extend(target.port_args());
    args.extend(["-O".to_string(), "cancel".to_string(), "-L".to_string(), forward_spec(local, remote)]);
    Some(finish(args, target, None))
}

/// Ending a machine's shared connection (when it is removed): `-O exit`. None without ControlMaster.
pub fn exit_master_args(target: &Target, mux: &Mux) -> Option<Vec<String>> {
    if !mux.shared() {
        return None;
    }
    let mut args = mux.args();
    args.extend(target.port_args());
    args.extend(["-O".to_string(), "exit".to_string()]);
    Some(finish(args, target, None))
}

/// What the connection test read: `uname -sm`, the login shell, tmux's path when it is there.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Probe {
    pub system: String,
    pub shell: String,
    pub tmux: bool,
}

/// The test's output read (a login banner before it is skipped: the last three lines count).
pub fn parse_probe(stdout: &str) -> Option<Probe> {
    let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    if lines.len() < 3 {
        return None;
    }
    let tail = &lines[lines.len() - 3..];
    let clean = |text: &str| text.chars().filter(|c| !c.is_control()).take(120).collect::<String>();
    Some(Probe { system: clean(tail[0]), shell: clean(tail[1]), tmux: tail[2] != "no-tmux" && tail[2].contains("tmux") })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alias() -> Target {
        Target::parse("devbox").unwrap()
    }

    #[test]
    fn quoting_survives_spaces_and_quotes() {
        assert_eq!(quote("plain"), "'plain'");
        assert_eq!(quote(""), "''");
        assert_eq!(quote("it's"), "'it'\\''s'");
        assert_eq!(quote("a \"b\" $c `d`"), "'a \"b\" $c `d`'");
        assert_eq!(quote_dir("/srv/my app"), "'/srv/my app'");
        assert_eq!(quote_dir("~"), "~");
        assert_eq!(quote_dir("~/src/o'neil dir"), "~/'src/o'\\''neil dir'");
        assert_eq!(quote_dir("/x/$(rm -rf ~)"), "'/x/$(rm -rf ~)'");
    }

    /// The quoting run through a real POSIX shell: what the far side's `sh -c` sees is the path as it was.
    #[cfg(unix)]
    #[test]
    fn quoted_paths_reach_the_shell_unchanged() {
        for path in ["/tmp/my dir", "/tmp/it's", "/tmp/\"q\" $HOME `x` \\n;|&", "/tmp/한글 폴더"] {
            #[allow(clippy::disallowed_methods, reason = "a test reads how /bin/sh parses the quoting")]
            let output = std::process::Command::new("/bin/sh").arg("-c").arg(format!("printf %s {}", quote(path))).output().unwrap();
            assert_eq!(String::from_utf8(output.stdout).unwrap(), path);
        }
    }

    #[test]
    fn terminal_args_share_the_connection_and_quote_the_folder() {
        let mux = Mux::Shared("/data/ssh".into());
        let args = terminal_args(&alias(), &mux, "/srv/my 'app'", &Shell::Login);
        assert_eq!(
            args,
            [
                "-o",
                "ServerAliveInterval=15",
                "-o",
                "ServerAliveCountMax=3",
                "-o",
                "ControlMaster=auto",
                "-o",
                "ControlPath=/data/ssh/cm-%C",
                "-o",
                "ControlPersist=10m",
                "-tt",
                "--",
                "devbox",
                "cd '/srv/my '\\''app'\\''' && exec $SHELL -l",
            ]
        );
        let direct = Target::parse("me@box:2222").unwrap();
        let args = terminal_args(&direct, &Mux::None, "~", &Shell::Tmux(tmux_session("ab-12")));
        assert!(!args.iter().any(|a| a.starts_with("ControlMaster")), "no ControlMaster where ssh has none");
        assert_eq!(&args[args.len() - 6..], ["-p", "2222", "-tt", "--", "me@box", "cd ~ && exec tmux new-session -A -s 'mighty-ab12'"]);
    }

    #[test]
    fn a_windows_control_folder_keeps_its_separator() {
        let args = Mux::Shared("C:\\Users\\me\\ssh\\".into()).args();
        assert_eq!(args[3], "ControlPath=C:\\Users\\me\\ssh\\cm-%C");
    }

    #[test]
    fn run_commands_quote_every_word_twice_over() {
        let mut env = BTreeMap::new();
        env.insert("NO_COLOR".to_string(), "1".to_string());
        let command = run_command("~/my app", "claude", &["--print".into(), "it's".into()], &env).unwrap();
        assert_eq!(
            command,
            "cd ~/'my app' && exec $SHELL -lc 'exec env '\\''NO_COLOR=1'\\'' '\\''claude'\\'' '\\''--print'\\'' '\\''it'\\''\\'\\'''\\''s'\\'''"
        );
        assert_eq!(run_command("/x", "../claude", &[], &BTreeMap::new()), Err("program"));
        assert_eq!(run_command("/x", "-oops", &[], &BTreeMap::new()), Err("program"));
        let mut bad = BTreeMap::new();
        bad.insert("A B".to_string(), "x".to_string());
        assert_eq!(run_command("/x", "claude", &[], &bad), Err("environment"));
        let args = run_args(&alias(), &Mux::None, command);
        assert_eq!(&args[..2], ["-o", "BatchMode=yes"]);
        assert!(args.contains(&"-T".to_string()) && !args.contains(&"-tt".to_string()));
    }

    /// The run command through two real shells (the outer one ssh's far side starts, the inner `$SHELL -lc`): the CLI gets its
    /// arguments exactly.
    #[cfg(unix)]
    #[test]
    fn run_commands_reach_the_program_unchanged() {
        let dir = std::env::temp_dir().join(format!("mc-ssh-quote-{}", std::process::id()));
        let folder = dir.join("it's a \"dir\"");
        std::fs::create_dir_all(&folder).unwrap();
        let mut env = BTreeMap::new();
        env.insert("MC_VALUE".to_string(), "a 'b' $c".to_string());
        let args = vec!["one two".to_string(), "it's".to_string(), "$HOME".to_string(), String::new()];
        let command = run_command(&folder.to_string_lossy(), "printf", &args, &env).unwrap();
        // printf's format is the first argument: print it and the rest one per line, then the folder and the value.
        let command = command.replace(
            "'\\''printf'\\''",
            "'\\''sh'\\'' '\\''-c'\\'' '\\''printf \"%s|\" \"$@\"; pwd; printf %s \"$MC_VALUE\"'\\'' '\\''x'\\''",
        );
        #[allow(clippy::disallowed_methods, reason = "a test reads how /bin/sh parses the quoting")]
        let output = std::process::Command::new("/bin/sh").env("SHELL", "/bin/sh").arg("-c").arg(&command).output().unwrap();
        let text = String::from_utf8(output.stdout).unwrap();
        assert_eq!(text, format!("one two|it's|$HOME||{}\na 'b' $c", folder.display()));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_test_never_shares_a_connection_and_never_prompts() {
        let args = test_args(&Target::parse("me@box:2200").unwrap());
        assert_eq!(
            args,
            [
                "-o",
                "BatchMode=yes",
                "-o",
                "ServerAliveInterval=15",
                "-o",
                "ServerAliveCountMax=3",
                "-p",
                "2200",
                "-o",
                "ConnectTimeout=10",
                "-T",
                "--",
                "me@box",
                PROBE_COMMAND,
            ]
        );
        assert!(!args.iter().any(|a| a.contains("StrictHostKeyChecking")), "host keys are never accepted for the user");
    }

    #[test]
    fn forwards_listen_on_loopback_and_are_taken_back_from_the_master() {
        let mux = Mux::Shared("/d/ssh".into());
        let args = forward_args(&alias(), &mux, 43001, 3000);
        assert!(args.windows(2).any(|w| w == ["-L", "127.0.0.1:43001:localhost:3000"]));
        assert!(args.contains(&"-N".to_string()) && args.contains(&"ExitOnForwardFailure=yes".to_string()));
        assert_eq!(&args[args.len() - 2..], ["--", "devbox"]);
        let cancel = cancel_forward_args(&alias(), &mux, 43001, 3000).unwrap();
        assert_eq!(
            cancel,
            [
                "-o",
                "ControlMaster=auto",
                "-o",
                "ControlPath=/d/ssh/cm-%C",
                "-o",
                "ControlPersist=10m",
                "-O",
                "cancel",
                "-L",
                "127.0.0.1:43001:localhost:3000",
                "--",
                "devbox",
            ]
        );
        assert_eq!(cancel_forward_args(&alias(), &Mux::None, 1, 2), None);
        assert_eq!(exit_master_args(&alias(), &mux).unwrap()[6..], ["-O", "exit", "--", "devbox"]);
    }

    #[test]
    fn tmux_sessions_are_named_by_the_pane() {
        assert_eq!(tmux_session("3f2a-77"), "mighty-3f2a77");
        assert_eq!(tmux_session("x'; rm -rf /"), "mighty-xrmrf");
        assert_eq!(shell_command("/srv/a b", &Shell::Tmux("mighty-1".into())), "cd '/srv/a b' && exec tmux new-session -A -s 'mighty-1'");
    }

    #[test]
    fn the_probe_is_read_after_any_banner() {
        let probe = parse_probe("Welcome!\n\nLinux x86_64\n/bin/bash\n/usr/bin/tmux\n").unwrap();
        assert_eq!(probe, Probe { system: "Linux x86_64".into(), shell: "/bin/bash".into(), tmux: true });
        assert!(!parse_probe("Darwin arm64\n/bin/zsh\nno-tmux\n").unwrap().tmux);
        assert_eq!(parse_probe("Linux\n"), None);
    }
}
