//! M-A: the process host. Swift `MightyCore/ProcessRunner.swift` (`NativeChildProcess`, `ProcessCapture`, `LiveRunRegistry`) and
//! `ToolkitProcess*`; contract `mighty_platform::process` (`ProcessHost`, `ChildControl`, `start_detached*`).
//!
//! A child is started by `posix_spawn` as the leader of a process group of its own (`POSIX_SPAWN_SETPGROUP`, group 0), with an empty
//! signal mask, SIGPIPE back to its default, every descriptor of the app's closed but its three pipes (`POSIX_SPAWN_CLOEXEC_DEFAULT`)
//! and its folder set before `exec` (`posix_spawn_file_actions_addchdir_np`), the pipes moved to fd ≥ 3 first
//! (`ProcessRunner.swift:62-84`). The group is the Windows job object's counterpart:
//! - `kill` is SIGKILL to the group, sent only while the leader is unreaped (its pid may belong to another process after);
//! - once the leader is reaped, what it left in its group is ended (SIGTERM, then SIGKILL 150 ms later: `terminateGroup`), so a
//!   background child holding a pipe never keeps a reader waiting (Swift "testProcessGroupCleansBackgroundChildWhenParentExits");
//! - a close waits for the exit, then for the group to be empty;
//! - every group still alive when the app exits gets SIGKILL from an `atexit` handler (Swift's `LiveRunRegistry.killAll` at quit).
//!
//! A pipe a process outside the group still holds (a daemon started with `setsid`, a `set -m` job) is read for one second after the
//! leader's exit, plus up to 30 s the reader's consumer held it, and at most 16 MB; past either the app's end is closed and the
//! writer gets EPIPE (Swift's reader, `maximumDrainCredit`, `maximumBytesAfterExit`, `finalDrainBytes`).
//!
//! `start_detached*`: `POSIX_SPAWN_SETSID` (out of the app's session and group), standard streams on `/dev/null`, the environment
//! exactly as given, no descriptor of the app's; reaped on a thread of its own so it never stays a zombie (only the update modules
//! and the Ouroboros daemon call them: the CI guard).

use std::collections::{BTreeMap, HashSet};
use std::ffi::{CString, OsString};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::Path;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Once};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use mighty_platform::{
    BoxFuture, Child, ChildControl, DetachedChild, DetachedEnvironment, DetachedRequest, PlatformError, ProcessHost, Result, SpawnRequest,
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::unix::pipe;
use tokio::sync::watch;

/// `POSIX_SPAWN_SETSID` (spawn.h, macOS 10.15+), which the libc crate does not name.
const POSIX_SPAWN_SETSID: libc::c_int = 0x0400;
/// How long a close waits for the exit, then for the group to be empty (the port's 3 s, C# `ChildProcess.DisposeAsync`).
const CLOSE_WAIT: Duration = Duration::from_secs(3);
/// Between the SIGTERM and the SIGKILL to what a reaped leader left in its group (Swift `terminateGroup`'s `usleep(150_000)`).
const GROUP_GRACE: Duration = Duration::from_millis(150);
/// A pipe is read this long after the leader's exit (Swift's reader: "more than 1 s after exit").
const DRAIN_AFTER_EXIT: Duration = Duration::from_secs(1);
/// Time the reader's consumer held it after the exit counts as idle only past this (Swift `maximumDrainCredit`).
const MAXIMUM_DRAIN_CREDIT: Duration = Duration::from_secs(30);
/// At most this much is read from one pipe after the leader's exit (Swift `maximumBytesAfterExit`).
const MAXIMUM_BYTES_AFTER_EXIT: usize = 16 * 1_048_576;
/// At most this much is read in the last look at a pipe whose drain ended on time (Swift `finalDrainBytes`).
const FINAL_DRAIN_BYTES: usize = 65_536;

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The Mac's `ProcessHost`.
pub fn process_host() -> Arc<dyn ProcessHost> {
    Arc::new(MacProcessHost)
}

/// Starts children as Swift's `NativeChildProcess` does. See the module and [`mighty_platform::ProcessHost`] for the contract.
#[derive(Debug, Clone, Copy, Default)]
pub struct MacProcessHost;

impl MacProcessHost {
    pub fn new() -> MacProcessHost {
        MacProcessHost
    }
}

impl ProcessHost for MacProcessHost {
    fn spawn(&self, request: SpawnRequest) -> BoxFuture<'_, Result<Child>> {
        Box::pin(async move { start(&request) })
    }

    /// The update's `/bin/sh install.sh` and the relaunched app (Swift `AppUpdate.swift:408-425`): see the module.
    fn start_detached(&self, request: DetachedRequest) -> Result<u32> {
        let pid = spawn_detached(&request)?;
        reap_in_background(pid);
        Ok(pid as u32)
    }

    /// The Ouroboros dashboard daemon's start: [`ProcessHost::start_detached`], waited for and ended alone.
    fn start_detached_waitable(&self, request: DetachedRequest) -> Result<Box<dyn DetachedChild>> {
        let pid = spawn_detached(&request)?;
        let (sender, exit) = watch::channel(None);
        let shared = Arc::new(Waitable { pid, reaped: AtomicBool::new(false), exit });
        let reaper = shared.clone();
        std::thread::Builder::new()
            .name("mighty-detached-exit".into())
            .spawn(move || {
                let code = reap(pid);
                reaper.reaped.store(true, Ordering::SeqCst);
                sender.send_replace(Some(code));
            })
            .map_err(PlatformError::Io)?;
        Ok(Box::new(WaitableChild(shared)))
    }
}

/// Every process group a child of the app leads and that may still have members: what the `atexit` handler ends.
static LIVE_GROUPS: Mutex<Option<HashSet<libc::pid_t>>> = Mutex::new(None);
static AT_EXIT: Once = Once::new();

/// Ends every group still alive as the app exits (Swift `LiveRunRegistry.killAll` at the quit's deadline): a child must not outlive
/// the app. A lock held by another thread at exit is not waited for.
extern "C" fn kill_live_groups() {
    if let Ok(groups) = LIVE_GROUPS.try_lock()
        && let Some(groups) = groups.as_ref()
    {
        for group in groups {
            // SAFETY: a signal to a process group the app made.
            unsafe { libc::kill(-group, libc::SIGKILL) };
        }
    }
}

/// Counts `group` as alive until [`forget_group`] (the PTY host's sessions too).
pub(crate) fn track_group(group: libc::pid_t) {
    AT_EXIT.call_once(|| {
        // SAFETY: registers a plain `extern "C"` function.
        unsafe { libc::atexit(kill_live_groups) };
    });
    lock(&LIVE_GROUPS).get_or_insert_with(HashSet::new).insert(group);
}

pub(crate) fn forget_group(group: libc::pid_t) {
    if let Some(groups) = lock(&LIVE_GROUPS).as_mut() {
        groups.remove(&group);
    }
}

/// Whether any process is still in `group` (signal 0: EPERM still means one is there).
pub(crate) fn group_alive(group: libc::pid_t) -> bool {
    // SAFETY: signal 0 only checks.
    unsafe { libc::kill(-group, 0) == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM) }
}

/// Swift `terminateGroup`: what is left in `group` gets SIGTERM, then SIGKILL 150 ms later. Nothing when the group is empty.
pub(crate) fn terminate_group(group: libc::pid_t) {
    if group <= 0 || !group_alive(group) {
        return;
    }
    // SAFETY: signals to a process group the app made.
    unsafe { libc::kill(-group, libc::SIGTERM) };
    std::thread::sleep(GROUP_GRACE);
    // SAFETY: as above.
    unsafe { libc::kill(-group, libc::SIGKILL) };
}

/// Waits for `pid` to end and reaps it: its exit code, or 128 + the signal that ended it (Swift's decoding); 127 when it cannot be
/// waited for.
pub(crate) fn reap(pid: libc::pid_t) -> i32 {
    let mut status: libc::c_int = 0;
    loop {
        // SAFETY: waits for a child of this process.
        let result = unsafe { libc::waitpid(pid, &mut status, 0) };
        if result == pid {
            break;
        }
        if result < 0 && io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
            return 127;
        }
    }
    decode(status)
}

/// A wait status as a code: the exit code, or 128 + the signal.
pub(crate) fn decode(status: libc::c_int) -> i32 {
    if status & 0x7f == 0 { (status >> 8) & 0xff } else { 128 + (status & 0x7f) }
}

fn reap_in_background(pid: libc::pid_t) {
    let started = std::thread::Builder::new().name("mighty-detached-reaper".into()).spawn(move || {
        reap(pid);
    });
    // Without the thread the process stays a zombie until the app exits; it still runs.
    drop(started);
}

pub(crate) fn last_error() -> PlatformError {
    PlatformError::Io(io::Error::last_os_error())
}

fn code(result: libc::c_int) -> Result<()> {
    if result == 0 { Ok(()) } else { Err(PlatformError::Io(io::Error::from_raw_os_error(result))) }
}

pub(crate) fn cstring(bytes: &[u8], what: &str) -> Result<CString> {
    CString::new(bytes).map_err(|_| PlatformError::Invalid(format!("{what} holds a NUL")))
}

fn validate(request: &SpawnRequest) -> Result<()> {
    if request.shell_command.is_some() {
        return Err(PlatformError::Invalid("a cmd.exe command line is Windows-only".into()));
    }
    if !request.program.is_absolute() || !request.cwd.is_absolute() {
        return Err(PlatformError::Invalid("the program and the working folder need absolute paths".into()));
    }
    let nul = |text: &str| text.contains('\0');
    if request.args.iter().any(|argument| nul(argument)) {
        return Err(PlatformError::Invalid("an argument holds a NUL".into()));
    }
    let bad_name = |name: &str| name.is_empty() || name.contains('=') || nul(name);
    if request.env.iter().any(|(name, value)| bad_name(name) || nul(value)) || request.env_remove.iter().any(|name| bad_name(name)) {
        return Err(PlatformError::Invalid("invalid child environment".into()));
    }
    Ok(())
}

/// `NAME=value` entries for `execve`.
fn environment_block(variables: impl IntoIterator<Item = (OsString, OsString)>) -> Result<Vec<CString>> {
    variables
        .into_iter()
        .map(|(name, value)| {
            let mut entry = name.into_vec();
            entry.push(b'=');
            entry.extend(value.into_vec());
            cstring(&entry, "the environment")
        })
        .collect()
}

/// This process's environment without `env_remove`, with `env` laid over it (names compared exactly, as the Mac does).
fn child_environment(request: &SpawnRequest) -> Result<Vec<CString>> {
    let mut variables: BTreeMap<OsString, OsString> = std::env::vars_os().collect();
    for name in &request.env_remove {
        variables.remove(std::ffi::OsStr::new(name));
    }
    for (name, value) in &request.env {
        variables.insert(name.into(), value.into());
    }
    environment_block(variables)
}

/// A pipe whose two ends are at fd 3 or above and close on `exec` (Swift `makePipe`): (read, write).
pub(crate) fn pipe_pair() -> Result<(OwnedFd, OwnedFd)> {
    let mut fds: [RawFd; 2] = [-1, -1];
    // SAFETY: an array of two descriptors.
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return Err(last_error());
    }
    // SAFETY: the two descriptors pipe just made, owned from here.
    let (read, write) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
    Ok((above_standard(read)?, above_standard(write)?))
}

fn above_standard(fd: OwnedFd) -> Result<OwnedFd> {
    if fd.as_raw_fd() < 3 {
        // SAFETY: duplicates an open descriptor to the lowest free one from 3, close-on-exec.
        let moved = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 3) };
        if moved < 3 {
            return Err(last_error());
        }
        // SAFETY: the descriptor fcntl just made.
        return Ok(unsafe { OwnedFd::from_raw_fd(moved) });
    }
    // SAFETY: an open descriptor this function owns.
    unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) };
    Ok(fd)
}

/// `posix_spawn`'s file actions and attributes, destroyed when dropped.
struct Spawn {
    actions: libc::posix_spawn_file_actions_t,
    attributes: libc::posix_spawnattr_t,
}

impl Spawn {
    fn new() -> Result<Spawn> {
        // SAFETY: plain values posix_spawn_*_init fill in.
        let mut spawn = Spawn { actions: unsafe { std::mem::zeroed() }, attributes: unsafe { std::mem::zeroed() } };
        // SAFETY: initializes the two objects; each is destroyed in Drop (destroying an uninitialized one is avoided by order).
        code(unsafe { libc::posix_spawn_file_actions_init(&mut spawn.actions) })?;
        if let Err(error) = code(unsafe { libc::posix_spawnattr_init(&mut spawn.attributes) }) {
            // SAFETY: the actions initialized above.
            unsafe { libc::posix_spawn_file_actions_destroy(&mut spawn.actions) };
            std::mem::forget(spawn);
            return Err(error);
        }
        Ok(spawn)
    }

    fn dup2(&mut self, fd: RawFd, to: RawFd) -> Result<()> {
        // SAFETY: initialized actions; descriptors the caller keeps open until the spawn.
        code(unsafe { libc::posix_spawn_file_actions_adddup2(&mut self.actions, fd, to) })
    }

    fn open_null(&mut self, to: RawFd, flags: libc::c_int) -> Result<()> {
        // SAFETY: initialized actions and a NUL-terminated path.
        code(unsafe { libc::posix_spawn_file_actions_addopen(&mut self.actions, to, c"/dev/null".as_ptr(), flags, 0) })
    }

    fn chdir(&mut self, folder: &CString) -> Result<()> {
        // SAFETY: initialized actions and a NUL-terminated path (copied by the call).
        code(unsafe { libc::posix_spawn_file_actions_addchdir_np(&mut self.actions, folder.as_ptr()) })
    }

    /// The child's signals: none blocked, SIGPIPE (which the Rust runtime ignores, and an ignored signal stays ignored across
    /// `exec`) back to its default; `flags` plus `POSIX_SPAWN_SETSIGMASK | POSIX_SPAWN_SETSIGDEF | POSIX_SPAWN_CLOEXEC_DEFAULT`.
    fn attributes(&mut self, flags: libc::c_int, group: bool) -> Result<()> {
        // SAFETY: sigset_t values sigemptyset/sigaddset fill in; initialized attributes.
        unsafe {
            let mut none: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut none);
            let mut defaults: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut defaults);
            libc::sigaddset(&mut defaults, libc::SIGPIPE);
            code(libc::posix_spawnattr_setsigmask(&mut self.attributes, &none))?;
            code(libc::posix_spawnattr_setsigdefault(&mut self.attributes, &defaults))?;
            if group {
                code(libc::posix_spawnattr_setpgroup(&mut self.attributes, 0))?;
            }
            let all = flags | libc::POSIX_SPAWN_SETSIGMASK | libc::POSIX_SPAWN_SETSIGDEF | libc::POSIX_SPAWN_CLOEXEC_DEFAULT;
            code(libc::posix_spawnattr_setflags(&mut self.attributes, all as libc::c_short))
        }
    }

    fn run(&self, program: &CString, args: &[CString], environment: &[CString]) -> Result<libc::pid_t> {
        let mut argv: Vec<*mut libc::c_char> = args.iter().map(|a| a.as_ptr().cast_mut()).collect();
        argv.push(std::ptr::null_mut());
        let mut envp: Vec<*mut libc::c_char> = environment.iter().map(|e| e.as_ptr().cast_mut()).collect();
        envp.push(std::ptr::null_mut());
        let mut pid: libc::pid_t = 0;
        // SAFETY: NUL-terminated strings and NULL-terminated arrays that outlive the call; initialized actions and attributes.
        let result =
            unsafe { libc::posix_spawn(&mut pid, program.as_ptr(), &self.actions, &self.attributes, argv.as_ptr(), envp.as_ptr()) };
        code(result)?;
        Ok(pid)
    }
}

impl Drop for Spawn {
    fn drop(&mut self) {
        // SAFETY: both were initialized in `new`.
        unsafe {
            libc::posix_spawn_file_actions_destroy(&mut self.actions);
            libc::posix_spawnattr_destroy(&mut self.attributes);
        }
    }
}

/// The program, then the arguments, as `argv`.
fn arguments(program: &Path, args: &[String]) -> Result<(CString, Vec<CString>)> {
    let program = cstring(program.as_os_str().as_bytes(), "the program")?;
    let mut argv = vec![program.clone()];
    for argument in args {
        argv.push(cstring(argument.as_bytes(), "an argument")?);
    }
    Ok((program, argv))
}

/// Swift `NativeChildProcess.init` (ProcessRunner.swift:44-115).
fn start(request: &SpawnRequest) -> Result<Child> {
    validate(request)?;
    let (program, argv) = arguments(&request.program, &request.args)?;
    let folder = cstring(request.cwd.as_os_str().as_bytes(), "the working folder")?;
    let environment = child_environment(request)?;
    let (input_read, input_write) = pipe_pair()?;
    let (output_read, output_write) = pipe_pair()?;
    let (error_read, error_write) = pipe_pair()?;
    let mut spawn = Spawn::new()?;
    spawn.dup2(input_read.as_raw_fd(), libc::STDIN_FILENO)?;
    spawn.dup2(output_write.as_raw_fd(), libc::STDOUT_FILENO)?;
    spawn.dup2(error_write.as_raw_fd(), libc::STDERR_FILENO)?;
    spawn.chdir(&folder)?;
    spawn.attributes(libc::POSIX_SPAWN_SETPGROUP, true)?;
    let pid = spawn.run(&program, &argv, &environment)?;
    drop(spawn);
    drop((input_read, output_write, error_write));
    track_group(pid);
    // SAFETY: the app's write end of the child's stdin; a write after the child closed its end gets EPIPE, never SIGPIPE.
    unsafe { libc::fcntl(input_write.as_raw_fd(), libc::F_SETNOSIGPIPE, 1) };

    let (reaped_sender, reaped) = watch::channel(None);
    let (exit_sender, exit) = watch::channel(None);
    let shared = Arc::new(Shared { pid, state: Mutex::new(State::default()), reaped, exit, closed: tokio::sync::Mutex::new(false) });
    let reaper = shared.clone();
    let started = std::thread::Builder::new().name("mighty-child-reaper".into()).spawn(move || {
        let code = reap(pid);
        lock(&reaper.state).reaped = true;
        reaped_sender.send_replace(Some(Instant::now()));
        // The leader can exit while a background child of its group still holds a pipe.
        terminate_group(pid);
        forget_group(pid);
        exit_sender.send_replace(Some(code));
    });
    if let Err(error) = started {
        // SAFETY: the group this start made; nothing could wait for it.
        unsafe { libc::kill(-pid, libc::SIGKILL) };
        reap(pid);
        forget_group(pid);
        return Err(PlatformError::Io(error));
    }
    let input = pipe::Sender::from_owned_fd(input_write).map_err(PlatformError::Io)?;
    let output = pipe::Receiver::from_owned_fd(output_read).map_err(PlatformError::Io)?;
    let error = pipe::Receiver::from_owned_fd(error_read).map_err(PlatformError::Io)?;
    Ok(Child::new(
        pid as u32,
        Box::new(ClosingWriter(Some(input))),
        Box::new(ChildOutput::new(output, shared.clone())),
        Box::new(ChildOutput::new(error, shared.clone())),
        Arc::new(MacChild(shared)),
    ))
}

/// A detached start (see the module): its process id.
fn spawn_detached(request: &DetachedRequest) -> Result<libc::pid_t> {
    if !request.program.is_absolute() || !request.cwd.is_absolute() {
        return Err(PlatformError::Invalid("the program and the working folder need absolute paths".into()));
    }
    let (program, argv) = arguments(&request.program, &request.args)?;
    let folder = cstring(request.cwd.as_os_str().as_bytes(), "the working folder")?;
    let environment = match &request.environment {
        DetachedEnvironment::Inherit => environment_block(std::env::vars_os())?,
        DetachedEnvironment::Exactly(values) => {
            if values.keys().any(|name| name.is_empty() || name.contains('=')) {
                return Err(PlatformError::Invalid("invalid detached environment".into()));
            }
            environment_block(values.iter().map(|(name, value)| (OsString::from(name), OsString::from(value))))?
        }
    };
    let mut spawn = Spawn::new()?;
    spawn.open_null(libc::STDIN_FILENO, libc::O_RDONLY)?;
    spawn.open_null(libc::STDOUT_FILENO, libc::O_WRONLY)?;
    spawn.open_null(libc::STDERR_FILENO, libc::O_WRONLY)?;
    spawn.chdir(&folder)?;
    spawn.attributes(POSIX_SPAWN_SETSID, false)?;
    spawn.run(&program, &argv, &environment)
}

#[derive(Default)]
struct State {
    /// The leader has been reaped: its pid is no longer the app's to signal.
    reaped: bool,
    /// The app ended the child (its output's rest is dropped on purpose, so no drain credit).
    killed: bool,
}

/// One child's group and exit, shared by its control and its two output readers.
struct Shared {
    pid: libc::pid_t,
    state: Mutex<State>,
    /// When the leader was reaped.
    reaped: watch::Receiver<Option<Instant>>,
    /// The exit code, published once what the leader left in its group was ended.
    exit: watch::Receiver<Option<i32>>,
    /// Held by the close that runs; true once one finished.
    closed: tokio::sync::Mutex<bool>,
}

impl Shared {
    fn kill(&self) {
        let mut state = lock(&self.state);
        state.killed = true;
        if !state.reaped {
            // SAFETY: a signal to the group of an unreaped child of the app.
            unsafe { libc::kill(-self.pid, libc::SIGKILL) };
        }
    }
}

async fn exited(exit: &watch::Receiver<Option<i32>>) -> Result<i32> {
    let mut exit = exit.clone();
    match exit.wait_for(Option::is_some).await {
        Ok(code) => Ok(code.unwrap_or(-1)),
        Err(_) => Err(PlatformError::Io(io::Error::other("the process's wait ended without its exit code"))),
    }
}

/// The control of one child (Swift `NativeChildProcess` past its start).
struct MacChild(Arc<Shared>);

impl ChildControl for MacChild {
    /// SIGKILL to the child's group, unless the leader was already reaped (what it left was ended then).
    fn kill(&self) {
        self.0.kill();
    }

    fn wait(&self) -> BoxFuture<'_, Result<i32>> {
        Box::pin(exited(&self.0.exit))
    }

    /// `kill`; up to 3 s for the exit; then up to 3 s until the group is empty, so no descendant still has the workspace's files
    /// open. Closes run one at a time; one after a finished close does nothing.
    fn close(&self) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            let mut closed = self.0.closed.lock().await;
            if *closed {
                return Ok(());
            }
            self.0.kill();
            match tokio::time::timeout(CLOSE_WAIT, exited(&self.0.exit)).await {
                Ok(result) => result?,
                Err(_) => return Err(PlatformError::TimedOut("the child to exit")),
            };
            let started = Instant::now();
            while group_alive(self.0.pid) {
                if started.elapsed() >= CLOSE_WAIT {
                    return Err(PlatformError::TimedOut("the child's descendants to close"));
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            *closed = true;
            Ok(())
        })
    }
}

/// The child's stdin: `shutdown` closes it, so the child reads its end (a pipe's own shutdown does nothing).
struct ClosingWriter(Option<pipe::Sender>);

impl AsyncWrite for ClosingWriter {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, bytes: &[u8]) -> Poll<io::Result<usize>> {
        match self.0.as_mut() {
            Some(pipe) => Pin::new(pipe).poll_write(cx, bytes),
            None => Poll::Ready(Err(io::ErrorKind::BrokenPipe.into())),
        }
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.0.as_mut() {
            Some(pipe) => Pin::new(pipe).poll_flush(cx),
            None => Poll::Ready(Ok(())),
        }
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if let Some(pipe) = self.0.as_mut() {
            std::task::ready!(Pin::new(pipe).poll_flush(cx))?;
        }
        self.0 = None;
        Poll::Ready(Ok(()))
    }
}

/// One of the child's outputs, read until its end — or, once the leader has exited, for one more second (plus the time the consumer
/// held it, up to 30 s) and at most 16 MB, after which the app's end is closed (Swift's reader, ProcessRunner.swift:117-170).
struct ChildOutput {
    pipe: Option<pipe::Receiver>,
    shared: Arc<Shared>,
    /// Wakes the reader when the leader is reaped, while it waits for bytes.
    reaped: Option<BoxFuture<'static, ()>>,
    /// Wakes the reader when the drain after the exit ends.
    deadline: Option<Pin<Box<tokio::time::Sleep>>>,
    after_exit: usize,
    /// When bytes were last handed to the consumer: the time until the next read is the consumer's.
    handed: Option<Instant>,
    credit: Duration,
    /// The last look's budget, once the drain ended on time.
    last_look: Option<usize>,
}

impl ChildOutput {
    fn new(pipe: pipe::Receiver, shared: Arc<Shared>) -> ChildOutput {
        ChildOutput {
            pipe: Some(pipe),
            shared,
            reaped: None,
            deadline: None,
            after_exit: 0,
            handed: None,
            credit: Duration::ZERO,
            last_look: None,
        }
    }

    /// Reads without waiting during the last look: bytes are handed on; the end of the pipe, nothing more to read or the budget spent
    /// end it.
    fn look_once_more(&mut self, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        let Some(pipe) = self.pipe.as_ref() else { return Poll::Ready(Ok(())) };
        let budget = self.last_look.unwrap_or(0);
        if budget == 0 {
            self.pipe = None;
            return Poll::Ready(Ok(()));
        }
        let room = buf.remaining().min(budget);
        let mut bytes = vec![0u8; room];
        match pipe.try_read(&mut bytes) {
            Ok(read @ 1..) => {
                buf.put_slice(&bytes[..read]);
                self.last_look = Some(budget - read);
                Poll::Ready(Ok(()))
            }
            Ok(_) | Err(_) => {
                self.pipe = None;
                Poll::Ready(Ok(()))
            }
        }
    }
}

impl AsyncRead for ChildOutput {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        loop {
            if this.pipe.is_none() {
                return Poll::Ready(Ok(()));
            }
            if this.last_look.is_some() {
                return this.look_once_more(buf);
            }
            let reaped = *this.shared.reaped.borrow();
            let now = Instant::now();
            let mut drain_end = None;
            if let Some(at) = reaped {
                if let Some(handed) = this.handed.take() {
                    this.credit += now.saturating_duration_since(handed.max(at));
                }
                let stopped = lock(&this.shared.state).killed;
                let credit = if stopped { Duration::ZERO } else { this.credit.min(MAXIMUM_DRAIN_CREDIT) };
                let end = at + DRAIN_AFTER_EXIT + credit;
                let over = this.after_exit > MAXIMUM_BYTES_AFTER_EXIT;
                if over || now >= end {
                    // A stop drops the rest on purpose, and past the byte cap a stray writer is reading's end; otherwise look once more,
                    // so a reader slowed by a loaded machine still hands on the child's own unread tail.
                    if stopped || over || this.credit > MAXIMUM_DRAIN_CREDIT {
                        this.pipe = None;
                        return Poll::Ready(Ok(()));
                    }
                    this.last_look = Some(FINAL_DRAIN_BYTES);
                    continue;
                }
                drain_end = Some(end);
            }
            let before = buf.filled().len();
            let pipe = this.pipe.as_mut().expect("checked above");
            match Pin::new(pipe).poll_read(cx, buf) {
                Poll::Ready(Ok(())) => {
                    let read = buf.filled().len() - before;
                    if read == 0 {
                        this.pipe = None;
                    } else {
                        if reaped.is_some() {
                            this.after_exit += read;
                        }
                        this.handed = Some(Instant::now());
                    }
                    return Poll::Ready(Ok(()));
                }
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                Poll::Pending => {}
            }
            match drain_end {
                Some(end) => {
                    let deadline = this.deadline.get_or_insert_with(|| Box::pin(tokio::time::sleep_until(end.into())));
                    if deadline.deadline() != end.into() {
                        deadline.as_mut().reset(end.into());
                    }
                    if deadline.as_mut().poll(cx).is_ready() {
                        continue;
                    }
                    return Poll::Pending;
                }
                None => {
                    let waiting = this.reaped.get_or_insert_with(|| {
                        let mut reaped = this.shared.reaped.clone();
                        Box::pin(async move {
                            let _ = reaped.wait_for(Option::is_some).await;
                        })
                    });
                    if waiting.as_mut().poll(cx).is_ready() {
                        this.reaped = None;
                        continue;
                    }
                    return Poll::Pending;
                }
            }
        }
    }
}

use std::future::Future as _;

/// A detached process the app waits on.
struct Waitable {
    pid: libc::pid_t,
    reaped: AtomicBool,
    exit: watch::Receiver<Option<i32>>,
}

struct WaitableChild(Arc<Waitable>);

impl DetachedChild for WaitableChild {
    fn id(&self) -> u32 {
        self.0.pid as u32
    }

    fn wait(&self) -> BoxFuture<'_, Result<i32>> {
        Box::pin(exited(&self.0.exit))
    }

    /// SIGKILL to this process alone (C#'s `Kill(entireProcessTree: false)`), while it is unreaped: what it started keeps running.
    fn terminate(&self) {
        if !self.0.reaped.load(Ordering::SeqCst) {
            // SAFETY: a signal to an unreaped child of the app.
            unsafe { libc::kill(self.0.pid, libc::SIGKILL) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wait_status_reads_as_the_exit_code_or_128_plus_the_signal() {
        assert_eq!(decode(3 << 8), 3);
        assert_eq!(decode(libc::SIGKILL), 128 + 9);
        assert_eq!(decode(0), 0);
    }

    #[test]
    fn a_cmd_line_a_relative_path_and_a_nul_are_refused() {
        let mut request = SpawnRequest::new("/bin/echo", ["x"], "/tmp");
        request.shell_command = Some("echo".into());
        assert!(matches!(validate(&request), Err(PlatformError::Invalid(_))));
        assert!(matches!(validate(&SpawnRequest::new("echo", ["x"], "/tmp")), Err(PlatformError::Invalid(_))));
        assert!(matches!(validate(&SpawnRequest::new("/bin/echo", ["a\0b"], "/tmp")), Err(PlatformError::Invalid(_))));
        assert!(matches!(validate(&SpawnRequest::new("/bin/echo", ["x"], "/tmp").env("A=B", "1")), Err(PlatformError::Invalid(_))));
        assert!(validate(&SpawnRequest::new("/bin/echo", ["x"], "/tmp").env("EMPTY", "")).is_ok());
    }
}
