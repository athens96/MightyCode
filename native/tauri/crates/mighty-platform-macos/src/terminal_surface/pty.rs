//! A minimal `forkpty` PTY host behind the `PtyHost` port, so the Ghostty view has a shell to draw while M-A's full port of Swift's
//! `PTYAgentTerminalPane` (`crate::pty`, plan §1.3) is written in parallel: M-A's `MacPtyHost` replaces it whole (`crate::pty::pty_host`
//! is the one place that names this module). What it does is the least of plan §1.3, as the Step 0 spike's `pty.rs` proved on CI:
//! `forkpty` at the request's grid, the child with SIGPIPE and the other signals at their defaults, the request's folder and whole
//! environment (with `TERM=xterm-256color`, `COLORTERM=truecolor`, `TERM_PROGRAM=ghostty` over it, and a UTF-8 `LANG` when none is
//! set), the master non-blocking and close-on-exec, the child's descriptors above stderr closed, writes retried for 2 s,
//! `TIOCSWINSZ`, the exit code (128 + signal), `busy` as `tcgetpgrp(master) != shell`, and a close that ends the shell's process
//! group (SIGHUP, SIGTERM, SIGKILL after 200 ms) while the exited shell still holds the group's id, then reaps it and stops reading
//! once the shell is gone.

use std::ffi::CString;
use std::os::fd::RawFd;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use mighty_platform::{BoxFuture, PlatformError, PtyControl, PtyHost, PtyRequest, PtySession, PtySize, Result};
use tokio::sync::{mpsc, watch};

/// The longest write taken at once, in UTF-16 units (the port's contract).
const WRITE_LIMIT: usize = 65_536;
/// How long a write may wait for the terminal to take it.
const WRITE_WAIT: Duration = Duration::from_secs(2);

static NEXT_ID: AtomicU32 = AtomicU32::new(1);
/// Held while a terminal's child is forked and its master marked close-on-exec.
static SPAWN: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The shell's reaping, held back while it is `Some`.
type Reap = std::sync::Mutex<Option<std::sync::mpsc::Sender<()>>>;

/// The fallback `PtyHost` (see the module comment).
pub fn pty_host() -> Arc<dyn PtyHost> {
    Arc::new(ForkPtyHost)
}

pub struct ForkPtyHost;

/// The shell's name without its folder.
fn shell_name(shell: &Path) -> String {
    shell.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default()
}

impl PtyHost for ForkPtyHost {
    /// `$SHELL` when it names a program, else `/bin/zsh` (Swift's).
    fn default_shell(&self) -> PathBuf {
        std::env::var_os("SHELL")
            .map(PathBuf::from)
            .filter(|shell| shell.is_absolute() && shell.is_file())
            .unwrap_or_else(|| PathBuf::from("/bin/zsh"))
    }

    /// An interactive login shell (`-il`), or `-f` / `--noprofile --norc` without the user's profile (a smoke); with a command, the
    /// shell runs it and exits.
    fn shell_arguments(&self, shell: &Path, command: Option<&str>, no_profile: bool) -> Vec<String> {
        let name = shell_name(shell);
        let mut arguments: Vec<String> = match (name.as_str(), no_profile) {
            ("zsh", true) => vec!["-f".into()],
            ("bash", true) => vec!["--noprofile".into(), "--norc".into()],
            (_, true) => Vec::new(),
            (_, false) => vec!["-l".into()],
        };
        match command {
            Some(command) => arguments.extend(["-c".to_string(), command.to_string()]),
            None => arguments.push("-i".into()),
        }
        arguments
    }

    fn open(&self, request: PtyRequest) -> BoxFuture<'_, Result<PtySession>> {
        Box::pin(async move { open(request) })
    }
}

fn c_string(text: &str) -> Result<CString> {
    CString::new(text).map_err(|_| PlatformError::Invalid("a terminal argument holds a NUL".into()))
}

fn open(request: PtyRequest) -> Result<PtySession> {
    if !request.cwd.is_absolute() || !request.cwd.is_dir() {
        return Err(PlatformError::Invalid("a terminal needs an existing absolute working folder".into()));
    }
    // Everything the child needs is made before the fork: after it only async-signal-safe calls run.
    let program = c_string(&request.program.to_string_lossy())?;
    let mut args = vec![program.clone()];
    for argument in &request.args {
        args.push(c_string(argument)?);
    }
    let mut env = request.env.clone();
    for (key, value) in [("TERM", "xterm-256color"), ("COLORTERM", "truecolor"), ("TERM_PROGRAM", "ghostty")] {
        env.insert(key.into(), value.into());
    }
    if !["LANG", "LC_ALL", "LC_CTYPE"].iter().any(|key| env.contains_key(*key)) {
        env.insert("LANG".into(), "en_US.UTF-8".into());
    }
    let mut environment = Vec::with_capacity(env.len());
    for (key, value) in &env {
        if key.is_empty() || key.contains('=') {
            return Err(PlatformError::Invalid("invalid terminal environment".into()));
        }
        environment.push(c_string(&format!("{key}={value}"))?);
    }
    let cwd = c_string(&request.cwd.to_string_lossy())?;
    let mut argv: Vec<*const libc::c_char> = args.iter().map(|a| a.as_ptr()).collect();
    argv.push(std::ptr::null());
    let mut envp: Vec<*const libc::c_char> = environment.iter().map(|e| e.as_ptr()).collect();
    envp.push(std::ptr::null());
    let size = request.size.clamped();
    let mut window = libc::winsize { ws_row: size.rows, ws_col: size.columns, ws_xpixel: 0, ws_ypixel: 0 };
    // The child closes every descriptor above stderr up to the process's limit (Swift's 3…65536).
    let mut limit = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
    // SAFETY: getrlimit into a valid struct.
    let highest =
        if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } == 0 { limit.rlim_cur.min(65_536) as libc::c_int } else { 65_536 };
    let mut master: RawFd = -1;
    // One terminal starts at a time, so no other terminal's child inherits a master before it is marked close-on-exec.
    let spawning = SPAWN.lock().unwrap_or_else(|p| p.into_inner());
    // SAFETY: forkpty with a valid winsize; the child calls only async-signal-safe functions before execve or _exit.
    let pid = unsafe { libc::forkpty(&mut master, std::ptr::null_mut(), std::ptr::null_mut(), &mut window) };
    if pid < 0 {
        return Err(PlatformError::Io(std::io::Error::last_os_error()));
    }
    if pid > 0 {
        // SAFETY: the master forkpty gave: closed on exec from now on.
        unsafe { libc::fcntl(master, libc::F_SETFD, libc::FD_CLOEXEC) };
    }
    drop(spawning);
    if pid == 0 {
        // SAFETY: the child after fork, with what was built before it.
        unsafe {
            let mut empty: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut empty);
            libc::sigprocmask(libc::SIG_SETMASK, &empty, std::ptr::null_mut());
            for signal in [libc::SIGPIPE, libc::SIGINT, libc::SIGQUIT, libc::SIGTERM, libc::SIGHUP, libc::SIGCHLD, libc::SIGTSTP] {
                libc::signal(signal, libc::SIG_DFL);
            }
            for fd in 3..highest.max(3) {
                libc::close(fd);
            }
            if libc::chdir(cwd.as_ptr()) != 0 {
                libc::_exit(126);
            }
            libc::execve(program.as_ptr(), argv.as_ptr(), envp.as_ptr());
            libc::_exit(127);
        }
    }
    // SAFETY: the master descriptor forkpty gave: UTF-8 input editing, non-blocking, closed on exec.
    unsafe {
        let mut attributes: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(master, &mut attributes) == 0 {
            attributes.c_iflag |= libc::IUTF8;
            libc::tcsetattr(master, libc::TCSANOW, &attributes);
        }
        let flags = libc::fcntl(master, libc::F_GETFL);
        libc::fcntl(master, libc::F_SETFL, flags | libc::O_NONBLOCK);
    }
    let (output_sender, output) = mpsc::channel::<Vec<u8>>(256);
    let (exit_sender, exit) = watch::channel::<Option<i32>>(None);
    let (reap_sender, reap) = std::sync::mpsc::channel::<()>();
    let master = Arc::new(Master(master));
    let control = Arc::new(ForkPty {
        master: master.clone(),
        pid,
        exit,
        closed: AtomicBool::new(false),
        stop: Arc::new(AtomicBool::new(false)),
        reap: std::sync::Mutex::new(Some(reap_sender)),
    });
    let stop = control.stop.clone();
    std::thread::Builder::new().name("pty-reader".into()).spawn(move || read(master, stop, output_sender)).map_err(PlatformError::Io)?;
    std::thread::Builder::new().name("pty-wait".into()).spawn(move || wait(pid, exit_sender, reap)).map_err(PlatformError::Io)?;
    Ok(PtySession { id: NEXT_ID.fetch_add(1, Ordering::Relaxed), output, control })
}

/// The master descriptor, closed once nothing reads or writes it any more (the reader and the control each hold it), so its number
/// is never reused under either.
struct Master(RawFd);

impl Drop for Master {
    fn drop(&mut self) {
        // SAFETY: the descriptor forkpty gave, closed once.
        unsafe { libc::close(self.0) };
    }
}

/// Reads the master until the terminal closes (every slave gone) or the close stops it: the output channel ends there.
fn read(master: Arc<Master>, stop: Arc<AtomicBool>, output: mpsc::Sender<Vec<u8>>) {
    let master_fd = master.0;
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let mut poll = libc::pollfd { fd: master_fd, events: libc::POLLIN, revents: 0 };
        // SAFETY: one valid pollfd.
        let ready = unsafe { libc::poll(&mut poll, 1, 200) };
        if ready == 0 {
            if stop.load(Ordering::SeqCst) {
                break;
            }
            continue;
        }
        // SAFETY: a valid descriptor and buffer.
        let read = unsafe { libc::read(master_fd, buffer.as_mut_ptr().cast(), buffer.len()) };
        if read > 0 {
            if output.blocking_send(buffer[..read as usize].to_vec()).is_err() && stop.load(Ordering::SeqCst) {
                break;
            }
            continue;
        }
        let error = std::io::Error::last_os_error();
        if read < 0 && matches!(error.kind(), std::io::ErrorKind::Interrupted | std::io::ErrorKind::WouldBlock) {
            continue;
        }
        // 0 or EIO: the terminal closed.
        break;
    }
}

/// Waits for the shell and says its exit code (128 + signal when a signal ended it) without reaping it: the exited shell keeps its
/// pid, and so its process group's id, until `reap` says the close is done with the group (or the control is gone), so a signal to
/// the group never reaches a process that reused the number.
fn wait(pid: libc::pid_t, exit: watch::Sender<Option<i32>>, reap: std::sync::mpsc::Receiver<()>) {
    // SAFETY: an all-zero siginfo_t is a valid out value.
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    loop {
        // SAFETY: waiting for our own child, leaving it unreaped.
        let waited = unsafe { libc::waitid(libc::P_PID, pid as libc::id_t, &mut info, libc::WEXITED | libc::WNOWAIT) };
        if waited == 0 {
            break;
        }
        if std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
            exit.send_replace(Some(-1));
            return;
        }
    }
    // SAFETY: the fields waitid filled for an exited child.
    let (code, status) = (info.si_code, unsafe { info.si_status() });
    let code = match code {
        libc::CLD_EXITED => status,
        libc::CLD_KILLED | libc::CLD_DUMPED => 128 + status,
        _ => -1,
    };
    exit.send_replace(Some(code));
    // Until the close is done with the group (or every sender is gone).
    let _ = reap.recv();
    let mut status = 0;
    // SAFETY: reaping our own exited child, once.
    unsafe { libc::waitpid(pid, &mut status, 0) };
}

struct ForkPty {
    master: Arc<Master>,
    pid: libc::pid_t,
    exit: watch::Receiver<Option<i32>>,
    closed: AtomicBool,
    stop: Arc<AtomicBool>,
    /// Dropped once the group may no longer be signalled: the shell is then reaped.
    reap: Reap,
}

impl ForkPty {
    fn exited(&self) -> bool {
        self.exit.borrow().is_some()
    }

    /// Signals the shell's process group (its session's leader is the shell), only while the shell is unreaped (its pid, the group's
    /// id, cannot have been reused).
    fn signal(&self, signal: libc::c_int) {
        let reap = self.reap.lock().unwrap_or_else(|p| p.into_inner());
        if reap.is_none() {
            return;
        }
        // SAFETY: a signal to our own child's process group.
        unsafe { libc::kill(-self.pid, signal) };
    }
}

impl PtyControl for ForkPty {
    fn write(&self, bytes: &[u8]) -> bool {
        if self.closed.load(Ordering::SeqCst) || self.exited() || String::from_utf8_lossy(bytes).encode_utf16().count() > WRITE_LIMIT {
            return false;
        }
        let deadline = Instant::now() + WRITE_WAIT;
        let mut rest = bytes;
        while !rest.is_empty() {
            // SAFETY: a valid descriptor and buffer.
            let written = unsafe { libc::write(self.master.0, rest.as_ptr().cast(), rest.len()) };
            if written > 0 {
                rest = &rest[written as usize..];
                continue;
            }
            let error = std::io::Error::last_os_error();
            match error.kind() {
                std::io::ErrorKind::Interrupted => continue,
                std::io::ErrorKind::WouldBlock if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
                _ => return false,
            }
        }
        true
    }

    fn resize(&self, size: PtySize) -> Result<()> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(PlatformError::Invalid("the terminal has closed".into()));
        }
        let size = size.clamped();
        let window = libc::winsize { ws_row: size.rows, ws_col: size.columns, ws_xpixel: 0, ws_ypixel: 0 };
        // SAFETY: TIOCSWINSZ with a valid winsize on the master.
        if unsafe { libc::ioctl(self.master.0, libc::TIOCSWINSZ, &window) } != 0 {
            return Err(PlatformError::Io(std::io::Error::last_os_error()));
        }
        Ok(())
    }

    fn process_ids(&self) -> Option<Vec<u32>> {
        None
    }

    /// Another process group than the shell's owns the terminal: a command runs (Swift `ShellActivity.isBusy`).
    fn busy(&self) -> Option<bool> {
        if self.closed.load(Ordering::SeqCst) || self.exited() {
            return None;
        }
        // SAFETY: the terminal's foreground group, read from the master.
        let foreground = unsafe { libc::tcgetpgrp(self.master.0) };
        (foreground > 0).then_some(foreground != self.pid)
    }

    fn wait(&self) -> BoxFuture<'_, Result<i32>> {
        let mut exit = self.exit.clone();
        Box::pin(async move {
            let code = exit.wait_for(Option::is_some).await.map_err(|_| PlatformError::Invalid("the terminal ended unread".into()))?;
            Ok(code.unwrap_or(-1))
        })
    }

    fn close(&self) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            if !self.closed.swap(true, Ordering::SeqCst) && !self.exited() {
                self.signal(libc::SIGHUP);
                self.signal(libc::SIGTERM);
                let mut exit = self.exit.clone();
                if tokio::time::timeout(Duration::from_millis(200), exit.wait_for(Option::is_some)).await.is_err() {
                    self.signal(libc::SIGKILL);
                }
            }
            let mut exit = self.exit.clone();
            let _ = exit.wait_for(Option::is_some).await;
            // What the shell left running in its group goes too (the exited shell still holds the group's id), then the shell is
            // reaped, and the reading stops once it found nothing more.
            self.signal(libc::SIGKILL);
            self.reap.lock().unwrap_or_else(|p| p.into_inner()).take();
            self.stop.store(true, Ordering::SeqCst);
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn read_until(output: &mut mpsc::Receiver<Vec<u8>>, needle: &str) -> String {
        let mut seen = String::new();
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline && !seen.contains(needle) {
            match tokio::time::timeout(Duration::from_millis(500), output.recv()).await {
                Ok(Some(bytes)) => seen.push_str(&String::from_utf8_lossy(&bytes)),
                Ok(None) => break,
                Err(_) => {}
            }
        }
        seen
    }

    fn request(args: Vec<String>) -> PtyRequest {
        PtyRequest {
            program: PathBuf::from("/bin/zsh"),
            args,
            cwd: std::env::temp_dir().canonicalize().unwrap(),
            env: [("PATH".to_string(), "/usr/bin:/bin".to_string()), ("HOME".to_string(), std::env::temp_dir().display().to_string())]
                .into_iter()
                .collect(),
            size: PtySize { columns: 91, rows: 27 },
        }
    }

    /// Swift `ShellActivityTests.swift:75` "thePaneCommandNamesTheShellItBecomes", for D2's shells: the PTY host starts the shell
    /// itself (no wrapper writing a pid file), so the pid the terminal's session reports is the shell's, a pane's shell is a login
    /// shell (`-il`; `-f` without the profile in a smoke), and the shell sees the terminal's grid and `TERM`.
    #[tokio::test]
    async fn the_pane_command_names_the_shell_it_becomes() {
        let host = crate::pty::pty_host();
        let zsh = Path::new("/bin/zsh");
        assert_eq!(host.shell_arguments(zsh, None, false), ["-l", "-i"]);
        assert_eq!(host.shell_arguments(zsh, None, true), ["-f", "-i"]);
        assert_eq!(host.shell_arguments(zsh, Some("echo hi"), true), ["-f", "-c", "echo hi"]);
        let mut session = host.open(request(host.shell_arguments(zsh, None, true))).await.unwrap();
        assert!(session.control.write(b"print -r -- PID=$$ LOGIN=${options[login]} SIZE=$(/bin/stty size) TERM=$TERM\r"));
        let seen = read_until(&mut session.output, "TERM=xterm-256color").await;
        assert!(seen.contains("SIZE=27 91"), "{seen}");
        assert!(seen.contains("LOGIN=off"), "a smoke's shell skips the profile: {seen}");
        session.control.close().await.unwrap();
        let mut login = host.open(request(host.shell_arguments(zsh, None, false))).await.unwrap();
        assert!(login.control.write(b"print -r -- LOGIN=${options[login]}\r"));
        let seen = read_until(&mut login.output, "LOGIN=on").await;
        assert!(seen.contains("LOGIN=on"), "a pane's shell is a login shell: {seen}");
        login.control.close().await.unwrap();
    }

    /// The fallback's terminal: busy while another process group owns it and idle at the prompt, Ctrl+C ends the foreground command,
    /// a resize reaches the shell, the exit code comes back, and a close ends the shell and the output.
    #[tokio::test]
    async fn the_fallback_terminal_runs_a_shell_end_to_end() {
        let host = pty_host();
        let mut session = host.open(request(host.shell_arguments(Path::new("/bin/zsh"), None, true))).await.unwrap();
        let control = session.control.clone();
        assert!(control.write(b"print -r -- READY\r"));
        read_until(&mut session.output, "READY\r\n").await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(control.busy(), Some(false), "at the prompt");
        assert!(control.write(b"/bin/sleep 30\r"));
        let mut busy = false;
        for _ in 0..50 {
            if control.busy() == Some(true) {
                busy = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(busy, "a command owns the terminal");
        assert!(control.write(b"\x03"));
        let mut idle = false;
        for _ in 0..50 {
            if control.busy() == Some(false) {
                idle = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(idle, "Ctrl+C ended it");
        control.resize(PtySize { columns: 70, rows: 20 }).unwrap();
        assert!(control.write(b"print -r -- GRID=$(/bin/stty size)\r"));
        assert!(read_until(&mut session.output, "GRID=20 70").await.contains("GRID=20 70"));
        assert!(control.write(b"exit 3\r"));
        assert_eq!(control.wait().await.unwrap(), 3);
        control.close().await.unwrap();
        assert!(!control.write(b"x") && control.busy().is_none(), "closed");
        let ended = tokio::time::timeout(Duration::from_secs(5), async { while session.output.recv().await.is_some() {} }).await;
        assert!(ended.is_ok(), "the output ends once the shell is gone");
    }
}
