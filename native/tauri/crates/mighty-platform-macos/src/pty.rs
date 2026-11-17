//! M-A: the PTY host, which also feeds the Ghostty view (plan D2: Rust owns every terminal's PTY). Swift
//! `MightyCore/PTYAgentTerminalPane.swift` (`launch`, `appendUserTyped`, `resize`, `drain`, `exited`), `ShellProcessProbe.swift` and
//! `ShellActivity.swift`; contract `mighty_platform::pty` (`PtyHost`, `PtyControl`).
//!
//! `forkpty` makes the shell a session leader whose controlling terminal is the PTY, so its group is the terminal's foreground group
//! and a typed Ctrl+C reaches it. Every signal stays blocked across the fork until the child has put its handlers back; the child sets
//! IUTF8, closes every descriptor from 3 up and `exec`s in the workspace folder; the open returns once it did (a ready pipe the child
//! closes just before `exec`, bounded at 30 s). The master is non-blocking; a write is retried for 2 s; a resize is `TIOCSWINSZ`.
//! The session's processes are its members (`getsid` = the shell's pid): what [`PtyControl::process_ids`] lists and a close ends
//! with SIGKILL, the counterpart of Windows' kill-on-close job. `busy` reads the terminal's foreground group against the shell's
//! (`tcgetpgrp`, Swift `ShellActivity.isBusy`), which needs no pid file now that the app owns the PTY (Swift's
//! `ShellProcessProbe.findShellPid` read one because Ghostty owned it).
//!
//! The terminal's environment is the request's with `TERM=xterm-256color` and `COLORTERM=truecolor` (Swift's agent pane). An
//! interactive shell also gets what libghostty's exec backend gave the Swift app's shell panes ([`shell_integration`]):
//! `TERM_PROGRAM=ghostty`, `GHOSTTY_RESOURCES_DIR`, `GHOSTTY_SHELL_FEATURES` and, for zsh, `ZDOTDIR` into Ghostty's shell
//! integration, from the resources the app bundle carries (`Contents/Resources/ghostty`, scripts/build-tauri-macos.sh); a
//! development build outside a bundle has none and starts its shells without them.

use std::collections::BTreeMap;
use std::ffi::CString;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use mighty_platform::{BoxFuture, PlatformError, PtyControl, PtyHost, PtyRequest, PtySession, PtySize, Result};
use tokio::sync::{mpsc, watch};

use crate::process::{Exited, FORK, cstring, decode, forget_session, last_error, lock, pipe_pair, reap, track_session, wait_exited};

/// The most input written at once, in UTF-16 units of its text (the port's 65,536, C# `TryWrite`).
const MAXIMUM_WRITE: usize = 65_536;
/// How long the output drain waits before it offers a chunk again while the output's 64 chunks are all taken up.
const OUTPUT_RETRY: Duration = Duration::from_millis(10);
/// How long one write keeps trying while the terminal's input is full (Swift `appendUserTyped`'s 2 s).
const WRITE_FOR: Duration = Duration::from_secs(2);
/// The longest the open waits for the child to be ready (Swift's `readyDeadline`).
const READY_WAIT: Duration = Duration::from_secs(30);
/// How long a close waits for the shell's exit after its SIGKILL.
const CLOSE_WAIT: Duration = Duration::from_secs(3);
/// How long the output drain reads on after the shell's exit or the close's start.
const DRAIN_AFTER_END: Duration = Duration::from_secs(1);

/// The Mac's `PtyHost`.
pub fn pty_host() -> Arc<dyn PtyHost> {
    Arc::new(MacPtyHost)
}

/// The user's login shell (`$SHELL`, when it names a file by its full path), else `/bin/zsh`, the Mac's default.
pub fn default_shell() -> PathBuf {
    std::env::var_os("SHELL")
        .map(PathBuf::from)
        .filter(|shell| shell.is_absolute() && shell.is_file())
        .unwrap_or_else(|| PathBuf::from("/bin/zsh"))
}

/// Ghostty's runtime resources in the app bundle around `exe` (scripts/build-tauri-macos.sh, Ghostty.app's own layout):
/// `Contents/Resources/ghostty` holding `shell-integration`, beside `Contents/Resources/terminfo`. None outside a bundle.
pub fn ghostty_resources(exe: &Path) -> Option<PathBuf> {
    let resources = exe.parent()?.parent()?.join("Resources");
    let ghostty = resources.join("ghostty");
    (ghostty.join("shell-integration").is_dir() && resources.join("terminfo").is_dir()).then_some(ghostty)
}

/// What libghostty's exec backend adds for the shell it starts (the Swift app's shell panes ran there), for an interactive shell
/// ([`shell_arguments`]' `-il` or `-i`; never a `-c` command, an agent's, zsh's `-f` or a CLI's own login) when the bundle carries
/// Ghostty's `resources`:
/// `TERM_PROGRAM=ghostty`, `GHOSTTY_RESOURCES_DIR`, `GHOSTTY_SHELL_FEATURES` (Ghostty's default `cursor,title`) and, for zsh,
/// `ZDOTDIR` pointing into the integration, the user's own `ZDOTDIR` kept in `GHOSTTY_ZSH_ZDOTDIR` (the integration's `.zshenv`
/// puts it back before anything else reads it, then runs the user's own).
pub fn shell_integration(environment: &mut BTreeMap<String, String>, program: &Path, args: &[String], resources: Option<&Path>) {
    let Some(resources) = resources else { return };
    let interactive = args.iter().any(|argument| argument == "-il" || argument == "-i");
    if !interactive || args.iter().any(|argument| argument == "-c") {
        return;
    }
    environment.insert("TERM_PROGRAM".into(), "ghostty".into());
    environment.insert("GHOSTTY_RESOURCES_DIR".into(), resources.to_string_lossy().into_owned());
    environment.insert("GHOSTTY_SHELL_FEATURES".into(), "cursor,title".into());
    if program.file_name().is_some_and(|name| name == "zsh") {
        let zsh = resources.join("shell-integration").join("zsh");
        if let Some(own) = environment.insert("ZDOTDIR".into(), zsh.to_string_lossy().into_owned()) {
            environment.insert("GHOSTTY_ZSH_ZDOTDIR".into(), own);
        }
    }
}

/// The shell's arguments: interactive as a login shell (`-il`, Swift's `/bin/zsh -il`), or without the profile (zsh `-f`, Swift's
/// smoke; bash `--noprofile --norc -i`; any other `-i`); a command runs as `-c <command>` (Swift's agent pane), without the profile
/// where the shell can skip it (zsh `-f`).
pub fn shell_arguments(shell: &Path, command: Option<&str>, no_profile: bool) -> Vec<String> {
    let name = shell.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let mut arguments: Vec<String> = Vec::new();
    match (name.as_str(), command, no_profile) {
        ("zsh", _, true) => arguments.push("-f".into()),
        ("bash", None, true) => arguments.extend(["--noprofile".into(), "--norc".into()]),
        _ => {}
    }
    match command {
        Some(command) => arguments.extend(["-c".to_string(), command.to_string()]),
        None if no_profile => {
            if name != "zsh" {
                arguments.push("-i".into());
            }
        }
        None => arguments.push("-il".into()),
    }
    arguments
}

/// Opens PTY sessions (Swift `PTYAgentTerminalPane.launch`). A session whose last control is dropped without [`PtyControl::close`]
/// is not torn down, as Windows' is not: its group is ended when the app exits.
#[derive(Debug, Clone, Copy, Default)]
pub struct MacPtyHost;

impl PtyHost for MacPtyHost {
    fn default_shell(&self) -> PathBuf {
        default_shell()
    }

    fn shell_arguments(&self, shell: &Path, command: Option<&str>, no_profile: bool) -> Vec<String> {
        shell_arguments(shell, command, no_profile)
    }

    fn open(&self, request: PtyRequest) -> BoxFuture<'_, Result<PtySession>> {
        Box::pin(start(request))
    }
}

/// Swift `ShellActivity.isBusy`: true only when the terminal names a foreground group and it is not the shell's own.
pub fn is_busy(shell_group: libc::pid_t, foreground_group: libc::pid_t) -> bool {
    foreground_group > 0 && foreground_group != shell_group
}

/// What the kernel says of one process (Swift `ShellProcessProbe.Groups`): its group, its terminal's foreground group (0 or less
/// without one), its parent and its start (so a pid handed to another process is noticed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Groups {
    pub pgid: libc::pid_t,
    pub tpgid: libc::pid_t,
    pub ppid: libc::pid_t,
    pub started: u64,
}

/// Swift `ShellProcessProbe.groups` (`proc_pidinfo`'s BSD info here): None when the process is gone or unreadable.
pub fn groups(pid: libc::pid_t) -> Option<Groups> {
    if pid <= 1 {
        return None;
    }
    // SAFETY: a plain struct proc_pidinfo fills.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = size_of::<libc::proc_bsdinfo>() as libc::c_int;
    // SAFETY: a buffer of `size` bytes.
    let read = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&raw mut info).cast(), size) };
    if read != size || info.pbi_pid != pid as u32 {
        return None;
    }
    Some(Groups {
        pgid: info.pbi_pgid as libc::pid_t,
        tpgid: info.e_tpgid as libc::pid_t,
        ppid: info.pbi_ppid as libc::pid_t,
        started: info.pbi_start_tvsec.wrapping_mul(1_000_000).wrapping_add(info.pbi_start_tvusec),
    })
}

/// Swift `ShellProcessProbe.hasChildren`: whether the process has any child (a shell at its prompt still holding a background or a
/// stopped job). False when it has none or cannot be read.
pub fn has_children(pid: libc::pid_t) -> bool {
    if pid <= 1 {
        return false;
    }
    let mut children = [0 as libc::pid_t; 64];
    // SAFETY: a buffer of that many bytes.
    let count = unsafe { libc::proc_listchildpids(pid, children.as_mut_ptr().cast(), size_of_val(&children) as libc::c_int) };
    count > 0
}

/// The processes of `session` (those whose `getsid` is it): the terminal's shell and everything under it.
pub fn session_members(session: libc::pid_t) -> Vec<u32> {
    let mut capacity = 1024usize;
    loop {
        let mut pids = vec![0 as libc::pid_t; capacity];
        // SAFETY: a buffer of that many bytes.
        let count = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), (capacity * size_of::<libc::pid_t>()) as libc::c_int) };
        if count < 0 {
            return vec![];
        }
        let count = count as usize;
        if count >= capacity && capacity < 65_536 {
            capacity *= 4;
            continue;
        }
        pids.truncate(count.min(capacity));
        // SAFETY: getsid only reads.
        return pids.into_iter().filter(|&pid| pid > 0 && unsafe { libc::getsid(pid) } == session).map(|pid| pid as u32).collect();
    }
}

fn invalid(reason: &str) -> PlatformError {
    PlatformError::Invalid(reason.into())
}

/// Swift `PTYAgentTerminalPane.launch` (PTYAgentTerminalPane.swift:66-147).
async fn start(request: PtyRequest) -> Result<PtySession> {
    if !request.cwd.is_absolute() || !request.cwd.is_dir() {
        return Err(invalid("a local workspace directory is required"));
    }
    if !request.program.is_absolute() || !request.program.is_file() {
        return Err(PlatformError::Io(io::Error::new(io::ErrorKind::NotFound, "the terminal shell was not found")));
    }
    if request.env.iter().any(|(name, value)| name.is_empty() || name.contains('=') || name.contains('\0') || value.contains('\0')) {
        return Err(invalid("invalid terminal environment"));
    }
    let (pid, master, ready_read) = fork(&request)?;
    // SAFETY: an open descriptor.
    unsafe {
        let flags = libc::fcntl(master.as_raw_fd(), libc::F_GETFL);
        libc::fcntl(master.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK);
    }
    track_session(pid);
    let master = Arc::new(master);
    let exited = Arc::new(AtomicBool::new(false));
    let closing = Arc::new(AtomicBool::new(false));
    let (exit_sender, exit) = watch::channel(None);
    // Dropped by the close once the session was ended, or with the terminal: until then an exited shell stays a zombie, so the session
    // and group ids `end_session` signals are never another process's.
    let (release, released) = std::sync::mpsc::channel::<()>();
    let reaped = exited.clone();
    let reaper = std::thread::Builder::new().name("mighty-pty-reaper".into()).spawn(move || {
        let watched = wait_exited(pid);
        let code = match watched {
            Exited::Status(status) => {
                reaped.store(true, Ordering::SeqCst);
                exit_sender.send_replace(Some(decode(status)));
                let _ = released.recv();
                reap(pid);
                None
            }
            // No status before the reap (it exited before the watch began, or could not be watched): reaped now to learn it.
            Exited::Before | Exited::Unwatched => Some(reap(pid)),
        };
        forget_session(pid);
        if let Some(code) = code {
            reaped.store(true, Ordering::SeqCst);
            exit_sender.send_replace(Some(code));
        }
    });
    let reaper = match reaper {
        Ok(reaper) => reaper,
        Err(error) => {
            end_session(pid);
            reap(pid);
            forget_session(pid);
            return Err(PlatformError::Io(error));
        }
    };
    let (output_sender, output) = mpsc::channel(64);
    let reader = {
        let (master, exited, closing) = (master.clone(), exited.clone(), closing.clone());
        std::thread::Builder::new()
            .name("mighty-pty-output".into())
            .spawn(move || drain(master.as_raw_fd(), &exited, &closing, output_sender))
            .map_err(PlatformError::Io)
    };
    let (writes, queue) = std::sync::mpsc::sync_channel::<Vec<u8>>(64);
    let writer = {
        let (master, exited) = (master.clone(), exited.clone());
        std::thread::Builder::new()
            .name("mighty-pty-input".into())
            .spawn(move || {
                while let Ok(bytes) = queue.recv() {
                    if !write_all(master.as_raw_fd(), &bytes, &exited) {
                        break;
                    }
                }
            })
            .map_err(PlatformError::Io)
    };
    let (reader, writer) = match (reader, writer) {
        (Ok(reader), Ok(writer)) => (reader, writer),
        (reader, writer) => {
            closing.store(true, Ordering::SeqCst);
            end_session(pid);
            let failure = [reader.err(), writer.err()].into_iter().flatten().next();
            return Err(failure.unwrap_or_else(|| PlatformError::Io(io::Error::other("terminal threads"))));
        }
    };
    // Return once the child leads the terminal's foreground group, so a stop or Ctrl+C sent right away reaches it: the end of `ready`
    // says so however long a busy machine takes; the bound only guards against a child that never runs. The reader already runs, so
    // a command that ends at once keeps its output.
    // SAFETY: an open descriptor.
    unsafe {
        let flags = libc::fcntl(ready_read.as_raw_fd(), libc::F_GETFL);
        libc::fcntl(ready_read.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK);
    }
    let deadline = Instant::now() + READY_WAIT;
    while Instant::now() < deadline {
        let mut byte = 0u8;
        // SAFETY: one byte into a local.
        let read = unsafe { libc::read(ready_read.as_raw_fd(), (&raw mut byte).cast(), 1) };
        let error = io::Error::last_os_error().raw_os_error();
        if read == 0 || (read < 0 && error != Some(libc::EAGAIN) && error != Some(libc::EINTR)) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    drop(ready_read);
    let (done, _) = watch::channel(false);
    let terminal = Terminal {
        pid,
        master,
        lifecycle: Mutex::new(Lifecycle { writes: Some(writes), release: Some(release), threads: Some([reader, writer, reaper]) }),
        closing,
        exited,
        exit,
        done: Arc::new(done),
    };
    Ok(PtySession { id: pid as u32, output, control: Arc::new(terminal) })
}

/// The fork itself: the shell's pid, the master and the read end of the ready pipe. Everything the child touches is made before the
/// fork: between it and `exec` the child only makes system calls on these pointers.
fn fork(request: &PtyRequest) -> Result<(libc::pid_t, OwnedFd, OwnedFd)> {
    let mut environment = request.env.clone();
    environment.insert("TERM".into(), "xterm-256color".into());
    environment.insert("COLORTERM".into(), "truecolor".into());
    let resources = std::env::current_exe().ok().and_then(|exe| ghostty_resources(&exe));
    shell_integration(&mut environment, &request.program, &request.args, resources.as_deref());
    let program = cstring(request.program.as_os_str().as_bytes(), "the shell")?;
    let mut args = vec![program.clone()];
    for argument in &request.args {
        args.push(cstring(argument.as_bytes(), "an argument")?);
    }
    let variables: Vec<CString> =
        environment.iter().map(|(name, value)| cstring(format!("{name}={value}").as_bytes(), "the environment")).collect::<Result<_>>()?;
    let folder = cstring(request.cwd.as_os_str().as_bytes(), "the workspace folder")?;
    let mut argv: Vec<*const libc::c_char> = args.iter().map(|a| a.as_ptr()).collect();
    argv.push(std::ptr::null());
    let mut envp: Vec<*const libc::c_char> = variables.iter().map(|v| v.as_ptr()).collect();
    envp.push(std::ptr::null());
    let failure = b"Mighty Code could not start the command in the workspace folder.\r\n";
    let size = request.size.clamped();
    let mut window = libc::winsize { ws_row: size.rows, ws_col: size.columns, ws_xpixel: 0, ws_ypixel: 0 };
    // The child closes its copy of `ready` (with every descriptor from 3) once its handlers are reset, just before exec.
    let (ready_read, ready_write) = pipe_pair()?;
    let mut master: RawFd = -1;
    // No descriptor of this crate is made while the child is forked, and the master is close-on-exec before another fork can run.
    let forking = FORK.write().unwrap_or_else(|poisoned| poisoned.into_inner());
    // SAFETY: sigset_t values sigfillset/sigemptyset fill in; forkpty with a valid winsize. The child only makes async-signal-safe
    // calls on memory made before the fork, then execs or _exits.
    let (pid, fork_error) = unsafe {
        let mut blocked: libc::sigset_t = std::mem::zeroed();
        let mut previous: libc::sigset_t = std::mem::zeroed();
        let mut none: libc::sigset_t = std::mem::zeroed();
        libc::sigfillset(&mut blocked);
        libc::sigemptyset(&mut none);
        libc::pthread_sigmask(libc::SIG_SETMASK, &blocked, &mut previous);
        let pid = libc::forkpty(&mut master, std::ptr::null_mut(), std::ptr::null_mut(), &mut window);
        if pid == 0 {
            let mut mode: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(libc::STDIN_FILENO, &mut mode) == 0 {
                mode.c_iflag |= libc::IUTF8;
                libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &mode);
            }
            for signal in [
                libc::SIGPIPE,
                libc::SIGINT,
                libc::SIGTERM,
                libc::SIGHUP,
                libc::SIGQUIT,
                libc::SIGCHLD,
                libc::SIGTSTP,
                libc::SIGTTIN,
                libc::SIGTTOU,
            ] {
                libc::signal(signal, libc::SIG_DFL);
            }
            libc::sigprocmask(libc::SIG_SETMASK, &none, std::ptr::null_mut());
            let limit = libc::getdtablesize().min(65_536);
            for fd in 3..limit {
                libc::close(fd);
            }
            if libc::chdir(folder.as_ptr()) == 0 {
                libc::execve(argv[0], argv.as_ptr(), envp.as_ptr());
            }
            libc::write(libc::STDERR_FILENO, failure.as_ptr().cast(), failure.len());
            libc::_exit(127);
        }
        let error = io::Error::last_os_error();
        if pid > 0 {
            libc::fcntl(master, libc::F_SETFD, libc::FD_CLOEXEC);
        }
        libc::pthread_sigmask(libc::SIG_SETMASK, &previous, std::ptr::null_mut());
        (pid, error)
    };
    drop(forking);
    drop(ready_write);
    if pid < 0 {
        return Err(PlatformError::Io(fork_error));
    }
    // SAFETY: the master forkpty opened, owned from here.
    Ok((pid, unsafe { OwnedFd::from_raw_fd(master) }, ready_read))
}

/// The output drain (Swift `drain`): reads the master until every holder of the terminal closed it (end of file or EIO), or — once
/// the shell has exited — until nothing more is there (Swift's `exited` drains, then closes the master). While the output's 64 chunks
/// are all taken up it waits, but never once the close has begun: from then a chunk that finds no room is dropped, so a close never
/// waits on a consumer. When nobody takes the output any more it keeps reading and drops what it reads. From the shell's exit or the
/// close's start it reads for one more second at most, so a process outside the session that keeps writing to the terminal never
/// holds the close.
fn drain(master: RawFd, exited: &AtomicBool, closing: &AtomicBool, output: mpsc::Sender<Vec<u8>>) {
    let mut sender = Some(output);
    let mut buffer = vec![0u8; 16_384];
    let mut ending: Option<Instant> = None;
    loop {
        // Read before looking again: everything the shell wrote before it exited is in the master by the time its exit is seen.
        let ended = exited.load(Ordering::SeqCst);
        if ended || closing.load(Ordering::SeqCst) {
            let since = *ending.get_or_insert_with(Instant::now);
            if since.elapsed() >= DRAIN_AFTER_END {
                return;
            }
        }
        // SAFETY: a buffer of that length on the open master.
        let read = unsafe { libc::read(master, buffer.as_mut_ptr().cast(), buffer.len()) };
        if read > 0 {
            let mut chunk = buffer[..read as usize].to_vec();
            while let Some(channel) = &sender {
                match channel.try_send(chunk) {
                    Ok(()) => break,
                    Err(mpsc::error::TrySendError::Closed(_)) => {
                        sender = None;
                        break;
                    }
                    Err(mpsc::error::TrySendError::Full(_)) if closing.load(Ordering::SeqCst) => break,
                    Err(mpsc::error::TrySendError::Full(back)) => {
                        chunk = back;
                        // The close unparks this thread, so it stops waiting at once.
                        std::thread::park_timeout(OUTPUT_RETRY);
                    }
                }
            }
            continue;
        }
        let error = io::Error::last_os_error().raw_os_error();
        if read < 0 && error == Some(libc::EINTR) {
            continue;
        }
        if read < 0 && error == Some(libc::EAGAIN) {
            if ended {
                return;
            }
            let mut ready = libc::pollfd { fd: master, events: libc::POLLIN, revents: 0 };
            // SAFETY: one pollfd.
            unsafe { libc::poll(&mut ready, 1, 100) };
            continue;
        }
        // End of file or EIO: every holder of the terminal side closed it.
        return;
    }
}

/// Writes all of `bytes` (Swift `appendUserTyped`): a full terminal is waited on for up to 2 s; false once the master refuses.
fn write_all(master: RawFd, mut bytes: &[u8], exited: &AtomicBool) -> bool {
    let began = Instant::now();
    while !bytes.is_empty() && began.elapsed() < WRITE_FOR {
        if exited.load(Ordering::SeqCst) {
            return false;
        }
        // SAFETY: a buffer of that length on the open master.
        let written = unsafe { libc::write(master, bytes.as_ptr().cast(), bytes.len()) };
        if written > 0 {
            bytes = &bytes[written as usize..];
            continue;
        }
        match io::Error::last_os_error().raw_os_error() {
            Some(libc::EINTR) => {}
            Some(libc::EAGAIN) => {
                let mut ready = libc::pollfd { fd: master, events: libc::POLLOUT, revents: 0 };
                // SAFETY: one pollfd.
                unsafe { libc::poll(&mut ready, 1, 50) };
            }
            _ => return false,
        }
    }
    true
}

/// Whether `pid` is a zombie (exited, not yet reaped by its parent).
fn zombie(pid: u32) -> bool {
    // SAFETY: a plain struct proc_pidinfo fills.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = size_of::<libc::proc_bsdinfo>() as libc::c_int;
    // SAFETY: a buffer of `size` bytes.
    let read = unsafe { libc::proc_pidinfo(pid as libc::c_int, libc::PROC_PIDTBSDINFO, 0, (&raw mut info).cast(), size) };
    // SZOMB (sys/proc.h).
    read == size && info.pbi_status == 5
}

/// SIGKILL to every process of the session and to the shell's group (the kill-on-close job's counterpart), in up to three passes, until
/// no process of the session but a zombie is left (one forked between the listing and the signals is caught by the next pass).
pub(crate) fn end_session(session: libc::pid_t) {
    for pass in 0..3 {
        let members: Vec<u32> = session_members(session).into_iter().filter(|pid| !zombie(*pid)).collect();
        if members.is_empty() && pass > 0 {
            return;
        }
        for pid in members {
            // SAFETY: a signal to a process of the terminal's session.
            unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
        }
        // SAFETY: a signal to the group the shell leads.
        unsafe { libc::kill(-session, libc::SIGKILL) };
        std::thread::sleep(Duration::from_millis(20));
    }
}

struct Lifecycle {
    writes: Option<SyncSender<Vec<u8>>>,
    /// Dropped once the session was ended: the reaper may then reap the shell.
    release: Option<std::sync::mpsc::Sender<()>>,
    /// The output drain, the input writer and the reaper, joined by the close.
    threads: Option<[JoinHandle<()>; 3]>,
}

/// One session's input, size, processes and teardown.
struct Terminal {
    /// The shell's pid, which leads the session and its first group.
    pid: libc::pid_t,
    master: Arc<OwnedFd>,
    lifecycle: Mutex<Lifecycle>,
    closing: Arc<AtomicBool>,
    exited: Arc<AtomicBool>,
    exit: watch::Receiver<Option<i32>>,
    done: Arc<watch::Sender<bool>>,
}

impl PtyControl for Terminal {
    /// Text of at most 65,536 UTF-16 units, not once closing or once the shell has exited, and only while the 64-write queue has room.
    fn write(&self, bytes: &[u8]) -> bool {
        if bytes.len() > MAXIMUM_WRITE && String::from_utf8_lossy(bytes).encode_utf16().count() > MAXIMUM_WRITE {
            return false;
        }
        let state = lock(&self.lifecycle);
        if self.closing.load(Ordering::SeqCst) || self.exited.load(Ordering::SeqCst) {
            return false;
        }
        match state.writes.as_ref().map(|writes| writes.try_send(bytes.to_vec())) {
            Some(Ok(())) => true,
            Some(Err(TrySendError::Full(_) | TrySendError::Disconnected(_))) | None => false,
        }
    }

    /// `TIOCSWINSZ` (the kernel sends SIGWINCH to the foreground group), clamped to 2–1000 × 1–1000; nothing once closing.
    fn resize(&self, size: PtySize) -> Result<()> {
        let _state = lock(&self.lifecycle);
        if self.closing.load(Ordering::SeqCst) {
            return Ok(());
        }
        let size = size.clamped();
        let window = libc::winsize { ws_row: size.rows, ws_col: size.columns, ws_xpixel: 0, ws_ypixel: 0 };
        // SAFETY: TIOCSWINSZ with a valid winsize on the open master.
        if unsafe { libc::ioctl(self.master.as_raw_fd(), libc::TIOCSWINSZ, &window) } != 0 {
            return Err(last_error());
        }
        Ok(())
    }

    fn process_ids(&self) -> Option<Vec<u32>> {
        if self.closing.load(Ordering::SeqCst) || self.exited.load(Ordering::SeqCst) {
            return None;
        }
        Some(session_members(self.pid))
    }

    /// Swift `ShellActivity.isBusy` over the terminal's foreground group (`tcgetpgrp` on the master, else the shell's own reading of
    /// its terminal) and the shell's group; None once closed, exited or unreadable.
    fn busy(&self) -> Option<bool> {
        if self.closing.load(Ordering::SeqCst) || self.exited.load(Ordering::SeqCst) {
            return None;
        }
        // SAFETY: the open master.
        let foreground = unsafe { libc::tcgetpgrp(self.master.as_raw_fd()) };
        let foreground = if foreground > 0 { foreground } else { groups(self.pid)?.tpgid };
        Some(is_busy(self.pid, foreground))
    }

    fn wait(&self) -> BoxFuture<'_, Result<i32>> {
        let mut exit = self.exit.clone();
        Box::pin(async move {
            match exit.wait_for(Option::is_some).await {
                Ok(code) => Ok(code.unwrap_or(-1)),
                Err(_) => Err(PlatformError::Io(io::Error::other("the terminal's wait ended without its exit code"))),
            }
        })
    }

    /// On a thread of its own, so it never blocks the caller: the input queue is completed, every process of the session gets SIGKILL,
    /// the shell's exit is awaited (up to 3 s), the drain reads what is left and ends, the threads are joined. Every call waits for the
    /// same close; from its start the drain drops what finds the output full.
    fn close(&self) -> BoxFuture<'_, Result<()>> {
        let mut done = self.done.subscribe();
        {
            let mut state = lock(&self.lifecycle);
            if !self.closing.swap(true, Ordering::SeqCst) {
                let writes = state.writes.take();
                let release = state.release.take();
                let threads = state.threads.take();
                let finished = self.done.clone();
                let (pid, exited) = (self.pid, self.exited.clone());
                let teardown = move || {
                    if let Some([reader, ..]) = &threads {
                        reader.thread().unpark();
                    }
                    drop(writes);
                    end_session(pid);
                    // The session is ended: an exited shell may be reaped now.
                    drop(release);
                    let began = Instant::now();
                    while !exited.load(Ordering::SeqCst) && began.elapsed() < CLOSE_WAIT {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    if exited.load(Ordering::SeqCst) {
                        for thread in threads.into_iter().flatten() {
                            let _ = thread.join();
                        }
                    }
                    finished.send_replace(true);
                };
                let teardown = Arc::new(Mutex::new(Some(teardown)));
                let handed = teardown.clone();
                let started = std::thread::Builder::new().name("mighty-pty-close".into()).spawn(move || {
                    let teardown = lock(&handed).take();
                    if let Some(teardown) = teardown {
                        teardown();
                    }
                });
                if started.is_err() {
                    let teardown = lock(&teardown).take();
                    if let Some(teardown) = teardown {
                        drop(state);
                        teardown();
                    }
                }
            }
        }
        Box::pin(async move {
            let _ = done.wait_for(|finished| *finished).await;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shell_runs_as_a_login_shell_or_without_its_profile() {
        let zsh = Path::new("/bin/zsh");
        assert_eq!(shell_arguments(zsh, None, false), ["-il"]);
        assert_eq!(shell_arguments(zsh, None, true), ["-f"]);
        assert_eq!(shell_arguments(zsh, Some("ls -la"), true), ["-f", "-c", "ls -la"]);
        assert_eq!(shell_arguments(zsh, Some("ls"), false), ["-c", "ls"]);
        let bash = Path::new("/bin/bash");
        assert_eq!(shell_arguments(bash, None, true), ["--noprofile", "--norc", "-i"]);
        assert_eq!(shell_arguments(bash, Some("ls"), true), ["-c", "ls"]);
        assert_eq!(shell_arguments(Path::new("/opt/homebrew/bin/fish"), None, false), ["-il"]);
        assert_eq!(shell_arguments(Path::new("/bin/sh"), None, true), ["-i"]);
    }

    /// The bundle's Ghostty resources (Ghostty.app's layout) are found only whole.
    #[test]
    fn ghostty_resources_are_the_bundles_ghostty_folder_beside_its_terminfo() {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let root = std::env::temp_dir().join(format!("mighty-ghostty-resources-{}-{nanos}", std::process::id()));
        let exe = root.join("MightyCode.app").join("Contents").join("MacOS").join("MightyCode");
        let resources = root.join("MightyCode.app").join("Contents").join("Resources");
        std::fs::create_dir_all(resources.join("ghostty").join("shell-integration").join("zsh")).unwrap();
        assert_eq!(ghostty_resources(&exe), None, "terminfo too");
        std::fs::create_dir_all(resources.join("terminfo")).unwrap();
        assert_eq!(ghostty_resources(&exe), Some(resources.join("ghostty")));
        assert_eq!(ghostty_resources(Path::new("/MightyCode")), None, "outside a bundle");
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// libghostty's exec environment for an interactive shell only, zsh's `ZDOTDIR` into the integration with the user's kept.
    #[test]
    fn an_interactive_shell_gets_ghosttys_shell_integration() {
        let resources = Path::new("/Applications/MightyCode.app/Contents/Resources/ghostty");
        let zsh = Path::new("/bin/zsh");
        let start = |program: &Path, args: &[&str], resources: Option<&Path>, own: Option<&str>| {
            let mut environment = BTreeMap::from([("PATH".to_string(), "/usr/bin".to_string())]);
            if let Some(own) = own {
                environment.insert("ZDOTDIR".into(), own.into());
            }
            let args: Vec<String> = args.iter().map(|argument| argument.to_string()).collect();
            shell_integration(&mut environment, program, &args, resources);
            environment
        };
        let login = start(zsh, &["-il"], Some(resources), None);
        assert_eq!(login["TERM_PROGRAM"], "ghostty");
        assert_eq!(login["GHOSTTY_RESOURCES_DIR"], resources.to_string_lossy());
        assert_eq!(login["GHOSTTY_SHELL_FEATURES"], "cursor,title");
        assert_eq!(login["ZDOTDIR"], resources.join("shell-integration").join("zsh").to_string_lossy());
        assert!(!login.contains_key("GHOSTTY_ZSH_ZDOTDIR"), "no ZDOTDIR of the user's to put back");
        let own = start(zsh, &["-il"], Some(resources), Some("/Users/me/.config/zsh"));
        assert_eq!(own["GHOSTTY_ZSH_ZDOTDIR"], "/Users/me/.config/zsh");
        let bash = start(Path::new("/bin/bash"), &["-il"], Some(resources), None);
        assert_eq!((bash["TERM_PROGRAM"].as_str(), bash.contains_key("ZDOTDIR")), ("ghostty", false), "zsh alone by ZDOTDIR");
        let untouched = BTreeMap::from([("PATH".to_string(), "/usr/bin".to_string())]);
        assert_eq!(start(zsh, &["-c", "ls"], Some(resources), None), untouched, "an agent's command");
        assert_eq!(start(zsh, &["-f"], Some(resources), None), untouched, "no startup files");
        assert_eq!(start(zsh, &["-il"], None, None), untouched, "no resources outside a bundle");
        assert_eq!(start(Path::new("/opt/homebrew/bin/claude"), &["setup-token"], Some(resources), None), untouched, "a CLI's login");
        assert_eq!(start(Path::new("/bin/sh"), &["-i"], Some(resources), None)["TERM_PROGRAM"], "ghostty");
    }

    /// Swift `ShellActivityTests.swift:9 "busyOnlyWhileAnotherGroupOwnsTheTerminal"`.
    #[test]
    fn busy_only_while_another_group_owns_the_terminal() {
        assert!(!is_busy(100, 100), "the shell's own group at its prompt");
        assert!(is_busy(100, 200), "a job's group runs in the foreground");
        assert!(!is_busy(100, 0), "no foreground group");
        assert!(!is_busy(100, -1), "no terminal");
    }

    /// Swift `ShellActivityTests.swift:17 "groupsReadThisProcess"`.
    #[test]
    fn groups_read_this_process() {
        let me = std::process::id() as libc::pid_t;
        let groups = groups(me).expect("this process can be read");
        // SAFETY: plain reads of this process's own ids.
        let (group, parent) = unsafe { (libc::getpgrp(), libc::getppid()) };
        assert_eq!(groups.pgid, group);
        assert_eq!(groups.ppid, parent);
        assert!(groups.started > 0);
        assert_eq!(super::groups(me).map(|again| again.started), Some(groups.started), "the start time is stable");
        assert!(super::groups(0).is_none() && super::groups(1).is_none(), "no reading of the kernel or launchd");
    }
}
