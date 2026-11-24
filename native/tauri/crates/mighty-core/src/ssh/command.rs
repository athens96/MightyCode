//! The far side's command lines and the local `ssh` argument arrays.
//!
//! Locally ssh always starts from an argument array (never a shell string). The one string is the remote command ssh hands to the far
//! side's login shell, always `/bin/sh -c '<script>'` ([`sh`]): one word every login shell (bash, zsh, fish, csh, tcsh) reads the
//! same, so the script is POSIX whatever that shell is. In the script every path, program and argument is single-quoted for a POSIX
//! shell ([`quote`]), and a folder under the remote home keeps its `~` outside the quotes so the far side expands it. Every array ends `-- <destination> <command>`, the
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

/// Whether `script` may go to the far side as it is inside [`word`]: no line break or other control character, no backslash and no
/// `!`. fish reads `\\` and `\'` inside single quotes, csh refuses a line break inside them and may read `!` as history; a script
/// with none of these reads the same in every login shell.
fn plain(script: &str) -> bool {
    !script.chars().any(|c| c.is_control() || c == '\\' || c == '!')
}

/// `text` single-quoted so that a POSIX shell, fish and csh/tcsh all read one word with exactly this text: a `'` is closed, written
/// as `"'"` and opened again (no backslash, which fish would read inside the quotes). Only for text [`plain`] allows.
fn word(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\"'\"'"))
}

/// The far side's decoding of a script sent as hex: `awk` (POSIX, in every system's base) turns the digits back into the script's
/// bytes (`LC_ALL=C`: one byte for each code), which `eval` runs in the same `/bin/sh`.
fn boot(script: &str) -> String {
    let hex: String = script.bytes().map(|b| format!("{b:02x}")).collect();
    format!(
        "eval \"$(LC_ALL=C awk -v s={hex} 'BEGIN{{h=\"0123456789abcdef\";n=length(s);for(i=1;i<n;i+=2)printf \"%c\",\
         (index(h,substr(s,i,1))-1)*16+index(h,substr(s,i+1,1))-1}}')\""
    )
}

/// `script` as the far side's login shell runs it, whatever that shell is (bash, zsh, fish, csh, tcsh): `/bin/sh -c '<script>'`, one
/// word with no line break, backslash or `!` in it. A script with any of those (most have line breaks) goes as hex that the far
/// side's `/bin/sh` decodes with `awk` and `eval`s, so its text never meets the login shell's quoting rules.
pub fn sh(script: &str) -> String {
    let body = if plain(script) { script.to_string() } else { boot(script) };
    format!("/bin/sh -c {}", word(&body))
}

/// The script inside a remote command [`sh`] made (for the checks' stand-in `ssh` and for reading what was sent); None for anything
/// else.
pub fn unwrap_sh(command: &str) -> Option<String> {
    let quoted = command.strip_prefix("/bin/sh -c '")?.strip_suffix('\'')?;
    let body = quoted.replace("'\"'\"'", "'");
    if body.contains('\'') && body.matches('\'').count() % 2 != 0 {
        return None;
    }
    let Some(rest) = body.strip_prefix("eval \"$(LC_ALL=C awk -v s=") else { return Some(body) };
    let hex = rest.split(' ').next()?;
    if hex.len() % 2 != 0 {
        return None;
    }
    let bytes: Option<Vec<u8>> = (0..hex.len()).step_by(2).map(|at| u8::from_str_radix(hex.get(at..at + 2)?, 16).ok()).collect();
    String::from_utf8(bytes?).ok()
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
    /// The login shell in the folder: `cd <dir> && exec "$SHELL" -l`.
    Login,
    /// Inside tmux, so the work survives a lost connection and a reconnect attaches again:
    /// `cd <dir> && exec tmux new-session -A -s <session>`.
    Tmux(String),
}

/// The script of a terminal pane (under [`sh`]).
pub fn shell_script(dir: &str, shell: &Shell) -> String {
    match shell {
        Shell::Login => format!("cd {} && exec \"${{SHELL:-/bin/sh}}\" -l", quote_dir(dir)),
        Shell::Tmux(session) => format!("cd {} && exec tmux new-session -A -s {}", quote_dir(dir), quote(session)),
    }
}

/// The remote command of a terminal pane.
pub fn shell_command(dir: &str, shell: &Shell) -> String {
    sh(&shell_script(dir, shell))
}

/// The login shells that read POSIX quoting (`$SHELL -lc '<POSIX command>'`). Any other (fish, csh, tcsh, nushell, …) is never handed
/// a POSIX command: its `PATH` is read from it and `/bin/sh` runs the command.
const POSIX_SHELLS: &str = "sh|bash|zsh|dash|ksh|ksh93|mksh|pdksh|ash|yash|busybox";

/// The lines that run `inner` (a POSIX command) the way the person's login shell would find its program: through a POSIX login shell
/// itself (`-lc`, so its profile's environment applies), else with the `PATH` that shell reports (csh and tcsh take no `-l` with
/// `-c`) and `/bin/sh` running `inner`.
pub(crate) fn login_exec(inner: &str) -> String {
    format!(
        "s=${{SHELL:-/bin/sh}}\n\
         case \"${{s##*/}}\" in\n\
         {POSIX_SHELLS}) exec \"$s\" -lc {quoted} ;;\n\
         csh|tcsh) f=-c ;;\n\
         *) f=-lc ;;\n\
         esac\n\
         p=$(\"$s\" \"$f\" 'printf \"MIGHTYPATH=%s\" \"$PATH\"' </dev/null 2>/dev/null)\n\
         case \"$p\" in *MIGHTYPATH=*) PATH=${{p##*MIGHTYPATH=}}; export PATH ;; esac\n\
         {inner}",
        quoted = quote(inner)
    )
}

/// The script of a shell pane's one-shot command: the person's text, written for their login shell, run by it in the folder (csh and
/// tcsh with `-c`, which take no `-l` beside it).
pub fn shell_command_script(dir: &str, text: &str) -> String {
    format!(
        "cd {} || exit 1\ns=${{SHELL:-/bin/sh}}\ncase \"${{s##*/}}\" in csh|tcsh) exec \"$s\" -c {text} ;; esac\nexec \"$s\" -lc {text}",
        quote_dir(dir),
        text = quote(text)
    )
}

/// The script of a run (under [`sh`]): the program found on the login shell's `PATH`, started in the folder with `env`'s values.
/// Err when the program or a name is not one the far side may be given.
pub fn run_script(dir: &str, program: &str, args: &[String], env: &BTreeMap<String, String>) -> Result<String, &'static str> {
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
    Ok(format!("cd {} || exit 1\n{}", quote_dir(dir), login_exec(&inner)))
}

/// The remote command of a run ([`run_script`] under [`sh`]).
pub fn run_command(dir: &str, program: &str, args: &[String], env: &BTreeMap<String, String>) -> Result<String, &'static str> {
    run_script(dir, program, args, env).map(|script| sh(&script))
}

/// The connection test's remote command: the OS and machine, the login shell, and whether tmux is there (under `/bin/sh`, so a csh or
/// fish login shell answers the same).
pub const PROBE_COMMAND: &str = "/bin/sh -c 'uname -sm; echo \"$SHELL\"; command -v tmux || echo no-tmux'";

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
            args[..args.len() - 1],
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
            ]
        );
        assert_eq!(unwrap_sh(args.last().unwrap()).unwrap(), "cd '/srv/my '\\''app'\\''' && exec \"${SHELL:-/bin/sh}\" -l");
        let direct = Target::parse("me@box:2222").unwrap();
        let args = terminal_args(&direct, &Mux::None, "~", &Shell::Tmux(tmux_session("ab-12")));
        assert!(!args.iter().any(|a| a.starts_with("ControlMaster")), "no ControlMaster where ssh has none");
        assert_eq!(
            &args[args.len() - 6..],
            ["-p", "2222", "-tt", "--", "me@box", "/bin/sh -c 'cd ~ && exec tmux new-session -A -s '\"'\"'mighty-ab12'\"'\"''"]
        );
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
        let script = run_script("~/my app", "claude", &["--print".into(), "it's".into()], &env).unwrap();
        assert!(script.starts_with("cd ~/'my app' || exit 1\n"), "{script}");
        assert!(
            script.contains(
                "exec \"$s\" -lc 'exec env '\\''NO_COLOR=1'\\'' '\\''claude'\\'' '\\''--print'\\'' '\\''it'\\''\\'\\'''\\''s'\\'''"
            ),
            "a POSIX login shell gets the command quoted once more: {script}"
        );
        assert!(script.ends_with("\nexec env 'NO_COLOR=1' 'claude' '--print' 'it'\\''s'"), "any other gets it from /bin/sh: {script}");
        let command = run_command("~/my app", "claude", &["--print".into(), "it's".into()], &env).unwrap();
        assert_eq!(unwrap_sh(&command).unwrap(), script);
        assert_eq!(run_command("/x", "../claude", &[], &BTreeMap::new()), Err("program"));
        assert_eq!(run_command("/x", "-oops", &[], &BTreeMap::new()), Err("program"));
        let mut bad = BTreeMap::new();
        bad.insert("A B".to_string(), "x".to_string());
        assert_eq!(run_command("/x", "claude", &[], &bad), Err("environment"));
        let args = run_args(&alias(), &Mux::None, command);
        assert_eq!(&args[..2], ["-o", "BatchMode=yes"]);
        assert!(args.contains(&"-T".to_string()) && !args.contains(&"-tt".to_string()));
    }

    /// The sent word never holds what a login shell other than a POSIX one reads differently.
    #[test]
    fn the_sent_word_is_one_every_login_shell_reads_alike() {
        for script in ["echo plain", "echo 'q'", "printf '%s\\n' a\nif true; then echo b; fi", "echo hi!", "echo 한글 $HOME `x`"] {
            let command = sh(script);
            let body = command.strip_prefix("/bin/sh -c ").unwrap();
            assert!(!body.contains('\\') && !body.contains('\n') && !body.contains('!'), "{command}");
            assert_eq!(unwrap_sh(&command).unwrap(), script);
            let posix = posix_words(&command);
            assert_eq!(posix.len(), 3, "{command}");
            assert_eq!(fish_words(&command).unwrap(), posix, "fish reads {command}");
            assert_eq!(csh_words(&command).unwrap(), posix, "csh reads {command}");
        }
        assert_eq!(sh("echo plain"), "/bin/sh -c 'echo plain'");
        assert_eq!(sh("echo 'q'"), "/bin/sh -c 'echo '\"'\"'q'\"'\"''");
        assert!(sh("a\nb").starts_with("/bin/sh -c 'eval \"$(LC_ALL=C awk -v s=610a62 "));
        assert_eq!(unwrap_sh("cd x && y"), None);
    }

    /// A stand-in for how fish splits a command line into words (fish 3): single quotes take `\\` and `\'` as escapes, double quotes
    /// take `\\`, `\"`, `\$` and expand `$`, a backslash outside quotes escapes the next character. None where fish would expand
    /// something or fail.
    fn fish_words(line: &str) -> Option<Vec<String>> {
        let mut words = Vec::new();
        let mut current: Option<String> = None;
        let mut chars = line.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                ' ' => words.extend(current.take()),
                '\'' => {
                    let word = current.get_or_insert_with(String::new);
                    loop {
                        match chars.next()? {
                            '\'' => break,
                            '\\' if matches!(chars.peek(), Some('\\' | '\'')) => word.push(chars.next()?),
                            other => word.push(other),
                        }
                    }
                }
                '"' => {
                    let word = current.get_or_insert_with(String::new);
                    loop {
                        match chars.next()? {
                            '"' => break,
                            '$' => return None,
                            '\\' if matches!(chars.peek(), Some('\\' | '"' | '$')) => word.push(chars.next()?),
                            other => word.push(other),
                        }
                    }
                }
                '\\' => current.get_or_insert_with(String::new).push(chars.next()?),
                '$' | '(' | ')' | '*' | '?' | '{' | '}' | '~' | '#' | ';' | '|' | '&' | '<' | '>' => return None,
                other => current.get_or_insert_with(String::new).push(other),
            }
        }
        words.extend(current);
        Some(words)
    }

    /// A stand-in for how csh/tcsh split a command line: single quotes literal but never across a line break, double quotes literal
    /// but for `$`, `!` and backquotes, `!` anywhere a history event. None where csh would fail or expand.
    fn csh_words(line: &str) -> Option<Vec<String>> {
        if line.contains('!') || line.contains('\n') {
            return None;
        }
        let mut words = Vec::new();
        let mut current: Option<String> = None;
        let mut chars = line.chars();
        while let Some(c) = chars.next() {
            match c {
                ' ' => words.extend(current.take()),
                '\'' => {
                    let word = current.get_or_insert_with(String::new);
                    loop {
                        match chars.next()? {
                            '\'' => break,
                            other => word.push(other),
                        }
                    }
                }
                '"' => {
                    let word = current.get_or_insert_with(String::new);
                    loop {
                        match chars.next()? {
                            '"' => break,
                            '$' | '`' => return None,
                            other => word.push(other),
                        }
                    }
                }
                '\\' | '$' | '`' | '*' | '?' | '[' | '{' | '~' | ';' | '|' | '&' | '<' | '>' | '(' | ')' => return None,
                other => current.get_or_insert_with(String::new).push(other),
            }
        }
        words.extend(current);
        Some(words)
    }

    /// The words a POSIX shell reads from what [`sh`] writes (single quotes and `"'"` only, so csh's reading is the same).
    fn posix_words(line: &str) -> Vec<String> {
        csh_words(line).expect("a word sh() writes has nothing csh would refuse")
    }

    /// Every login shell this computer has reads the sent word as the script itself: what each prints is what `/bin/sh` prints for
    /// the script.
    #[cfg(unix)]
    #[test]
    fn real_login_shells_run_the_script_as_written() {
        #[allow(clippy::disallowed_methods, reason = "a test reads how real shells parse the sent word")]
        fn run(shell: &str, command: &str) -> (String, bool) {
            let output = std::process::Command::new(shell)
                .arg("-c")
                .arg(command)
                .env("HOME", "/tmp")
                .env("SHELL", shell)
                .stdin(std::process::Stdio::null())
                .output()
                .unwrap();
            (String::from_utf8_lossy(&output.stdout).into_owned(), output.status.success())
        }
        let scripts = [
            "printf '%s|' \"it's\" 'a\\b' 'c\\\\d' \"$HOME\"\nif [ -n \"$HOME\" ]; then echo yes; fi",
            "echo 한글 '폴더'; echo hi!",
            "x='a \"b\" $c `d`'; printf '%s\\n' \"$x\"",
        ];
        let mut shells: Vec<&str> = vec!["/bin/sh", "/bin/bash", "/bin/zsh", "/bin/dash", "/bin/csh", "/bin/tcsh"];
        shells.extend(["/opt/homebrew/bin/fish", "/usr/local/bin/fish", "/usr/bin/fish"]);
        let mut tried = 0;
        for shell in shells.into_iter().filter(|s| std::path::Path::new(s).is_file()) {
            tried += 1;
            for script in scripts {
                let (expected, ok) = run("/bin/sh", script);
                assert!(ok, "{script}");
                assert_eq!(run(shell, &sh(script)), (expected, true), "{shell} runs {script:?}");
            }
        }
        assert!(tried >= 1);
    }

    /// The run's script through the login shells: a POSIX one runs the program itself (its profile's environment), fish and csh only
    /// say their `PATH` and `/bin/sh` runs it. The program gets its arguments, folder and values exactly.
    #[cfg(unix)]
    #[test]
    fn run_commands_reach_the_program_unchanged() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = std::env::temp_dir().join(format!("mc-ssh-quote-{}", std::process::id()));
        let folder = dir.join("it's a \"dir\"");
        let bin = dir.join("bin");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::create_dir_all(&bin).unwrap();
        let executable = |path: &std::path::Path, text: &str| {
            std::fs::write(path, text).unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
        };
        executable(&bin.join("mcprobe"), "#!/bin/sh\nprintf '%s|' \"$@\"; pwd; printf %s \"$MC_VALUE\"\n");
        // A fish-like login shell: it runs only the one-word `/bin/sh -c '…'` (refusing a backslash or a line break in it, which fish's
        // quoting reads differently), answers the `PATH` question after a greeting, and refuses to be handed a POSIX command.
        let fish = dir.join("fish");
        executable(
            &fish,
            &format!(
                "#!/bin/sh\ncase \"$1\" in\n-c) case \"$2\" in '/bin/sh -c '*) ;; *) exit 97;; esac\n\
                 case \"$2\" in *\"\\\\\"*) exit 98;; esac\n\
                 [ \"$(printf '%s' \"$2\" | wc -l | tr -d ' ')\" = 0 ] || exit 98\nexec /bin/sh -c \"$2\";;\n\
                 -lc) [ \"$2\" = 'printf \"MIGHTYPATH=%s\" \"$PATH\"' ] || exit 96; echo 'Welcome to fish'; printf 'MIGHTYPATH=%s' '{bin}:/usr/bin:/bin';;\n\
                 *) exit 95;;\nesac\n",
                bin = bin.display()
            ),
        );
        let mut env = BTreeMap::new();
        env.insert("MC_VALUE".to_string(), "a 'b' $c\nline\\".to_string());
        let args = vec!["one two".to_string(), "it's".to_string(), "$HOME".to_string(), String::new(), "back\\slash\nnew".to_string()];
        let command = run_command(&folder.to_string_lossy(), "mcprobe", &args, &env).unwrap();
        let expected = format!("one two|it's|$HOME||back\\slash\nnew|{}\na 'b' $c\nline\\", folder.display());
        let with_bin = format!("{}:/usr/bin:/bin", bin.display());
        let mut shells =
            vec![("/bin/sh".to_string(), with_bin.clone()), (fish.to_string_lossy().into_owned(), "/usr/bin:/bin".to_string())];
        for csh in ["/bin/csh", "/bin/tcsh"] {
            if std::path::Path::new(csh).is_file() {
                shells.push((csh.to_string(), with_bin.clone()));
            }
        }
        for (shell, path) in shells {
            #[allow(clippy::disallowed_methods, reason = "a test reads how the login shells run the sent word")]
            let output = std::process::Command::new(&shell)
                .arg("-c")
                .arg(&command)
                .env_clear()
                .env("SHELL", &shell)
                .env("PATH", &path)
                .env("HOME", &dir)
                .output()
                .unwrap();
            let text = String::from_utf8(output.stdout).unwrap();
            assert_eq!(text, expected, "{shell}: {}", String::from_utf8_lossy(&output.stderr));
        }
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
        assert_eq!(
            unwrap_sh(&shell_command("/srv/a b", &Shell::Tmux("mighty-1".into()))).unwrap(),
            "cd '/srv/a b' && exec tmux new-session -A -s 'mighty-1'"
        );
    }

    #[test]
    fn the_probe_is_read_after_any_banner() {
        let probe = parse_probe("Welcome!\n\nLinux x86_64\n/bin/bash\n/usr/bin/tmux\n").unwrap();
        assert_eq!(probe, Probe { system: "Linux x86_64".into(), shell: "/bin/bash".into(), tmux: true });
        assert!(!parse_probe("Darwin arm64\n/bin/zsh\nno-tmux\n").unwrap().tmux);
        assert_eq!(parse_probe("Linux\n"), None);
    }
}
