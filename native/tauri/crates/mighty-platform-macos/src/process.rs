//! M-A: the process host. Swift `MightyCore/ProcessRunner.swift` (`NativeChildProcess`, `ProcessCapture`, `LiveRunRegistry`) and
//! `ToolkitProcess*`; contract `mighty_platform::process` (`ProcessHost`, `ChildControl`, `start_detached*`).
//!
//! A child is started by `posix_spawn` as the leader of a process group of its own (`POSIX_SPAWN_SETPGROUP`, group 0), with an empty
//! signal mask, SIGPIPE back to its default, every descriptor of the app's closed but its three pipes (`POSIX_SPAWN_CLOEXEC_DEFAULT`)
//! and its folder set before `exec` (`posix_spawn_file_actions_addchdir_np`), the pipes moved to fd ≥ 3 first
//! (`ProcessRunner.swift:62-84`). The group is the Windows job object's counterpart:
//! - `kill` is SIGKILL to the group, sent only while the leader has not exited;
//! - once the leader has exited (seen by kqueue while it is still a zombie, so the group's id cannot pass to another process), what it
//!   left in its group is ended (SIGTERM, then SIGKILL 150 ms later: `terminateGroup`), then the leader is reaped, so a
//!   background child holding a pipe never keeps a reader waiting (Swift "testProcessGroupCleansBackgroundChildWhenParentExits");
//! - a close waits for the exit, then for the group to be empty;
//! - every group still alive when the app exits gets SIGKILL from an `atexit` handler (Swift's `LiveRunRegistry.killAll` at quit).
//!
//! A pipe a process outside the group still holds (a daemon started with `setsid`, a `set -m` job) is read for one second after the
//! leader's exit and at most 16 MB, then looked at once more (64 KB); past that the app's end is closed and the writer gets EPIPE
//! (Swift's reader, `maximumBytesAfterExit`, `finalDrainBytes`; see `ChildOutput`).
//!
//! `start_detached*`: `POSIX_SPAWN_SETSID` (out of the app's session and group), standard streams on `/dev/null`, the environment
//! exactly as given, no descriptor of the app's; reaped on a thread of its own so it never stays a zombie (only the update modules
//! and the Ouroboros daemon call them: the CI guard).

use std::collections::{BTreeMap, HashSet};
use std::ffi::{CString, OsString};
use std::future::Future as _;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::Path;
use std::pin::Pin;
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
        let shared = Arc::new(Waitable { pid, exited: Mutex::new(false), exit });
        let reaper = shared.clone();
        std::thread::Builder::new()
            .name("mighty-detached-exit".into())
            .spawn(move || {
                // Marked exited while it is still a zombie, so `terminate` never signals a pid handed to another process.
                let watched = wait_exited(pid);
                *lock(&reaper.exited) = true;
                let code = reap(pid);
                sender.send_replace(Some(watched.status().map(decode).unwrap_or(code)));
            })
            .map_err(PlatformError::Io)?;
        Ok(Box::new(WaitableChild(shared)))
    }
}

/// Swift `ToolkitProcess.resolveExecutable`: how a step's program is found on the Mac (no PATHEXT, unlike Windows'
/// `find_on_path`) — a full path as it is when it is an executable file; a bare name in each folder of `environment`'s PATH, the
/// first executable file of that name, full-path folders only; a relative name with a `/`, or a relative PATH folder, never (the
/// working folder never decides).
pub fn find_executable(name: &str, environment: &BTreeMap<String, String>) -> Option<String> {
    use std::os::unix::fs::PermissionsExt as _;
    let runnable = |path: &Path| std::fs::metadata(path).is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0);
    if name.starts_with('/') {
        return runnable(Path::new(name)).then(|| name.to_string());
    }
    if name.is_empty() || name.contains('/') || name.contains('\0') {
        return None;
    }
    let path = environment.get("PATH").map(String::as_str).unwrap_or("");
    path.split(':')
        .filter(|folder| folder.starts_with('/'))
        .map(|folder| Path::new(folder).join(name))
        .find(|candidate| runnable(candidate))
        .map(|found| found.to_string_lossy().into_owned())
}

/// What the app started that may still have members: the process groups its children lead, and the PTY sessions its shells lead
/// (whose jobs sit in groups of their own). The `atexit` handler ends them.
#[derive(Default)]
struct Live {
    groups: HashSet<libc::pid_t>,
    sessions: HashSet<libc::pid_t>,
}

static LIVE: Mutex<Option<Live>> = Mutex::new(None);
static AT_EXIT: Once = Once::new();

/// Ends everything still alive as the app exits (Swift `LiveRunRegistry.killAll` at the quit's deadline): a child must not outlive the
/// app. A lock another thread holds at exit is tried again for up to 100 ms, never waited on for ever.
extern "C" fn kill_live() {
    for _ in 0..10 {
        if let Ok(live) = LIVE.try_lock() {
            if let Some(live) = live.as_ref() {
                for group in &live.groups {
                    // SAFETY: a signal to a process group the app made.
                    unsafe { libc::kill(-group, libc::SIGKILL) };
                }
                for session in &live.sessions {
                    crate::pty::end_session(*session);
                }
            }
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn live() -> MutexGuard<'static, Option<Live>> {
    AT_EXIT.call_once(|| {
        // SAFETY: registers a plain `extern "C"` function.
        unsafe { libc::atexit(kill_live) };
    });
    lock(&LIVE)
}

/// Counts `group` as alive until [`forget_group`].
pub(crate) fn track_group(group: libc::pid_t) {
    live().get_or_insert_with(Live::default).groups.insert(group);
}

pub(crate) fn forget_group(group: libc::pid_t) {
    if let Some(live) = live().as_mut() {
        live.groups.remove(&group);
    }
}

/// Counts the PTY session `session` leads as alive until [`forget_session`].
pub(crate) fn track_session(session: libc::pid_t) {
    live().get_or_insert_with(Live::default).sessions.insert(session);
}

pub(crate) fn forget_session(session: libc::pid_t) {
    if let Some(live) = live().as_mut() {
        live.sessions.remove(&session);
    }
}

/// How a child's exit was seen before it was reaped ([`wait_exited`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Exited {
    /// It exited and is a zombie until reaped; the kernel gave its wait status.
    Status(libc::c_int),
    /// It had exited before the watch began (a zombie until reaped); its status comes with the reap.
    Before,
    /// It could not be watched: only the reap says when it ends.
    Unwatched,
}

impl Exited {
    pub(crate) fn status(self) -> Option<libc::c_int> {
        match self {
            Exited::Status(status) => Some(status),
            Exited::Before | Exited::Unwatched => None,
        }
    }

    /// Whether the child is a zombie now: its pid is still the app's, so the group and session it leads keep their ids.
    pub(crate) fn zombie(self) -> bool {
        self != Exited::Unwatched
    }
}

/// Waits until `pid` (a child of this process) has exited, without reaping it: kqueue's `EVFILT_PROC` with `NOTE_EXIT |
/// NOTE_EXITSTATUS`. Until the reap the pid stays the app's, so the group and session it leads can be signalled with no pid-reuse
/// window.
pub(crate) fn wait_exited(pid: libc::pid_t) -> Exited {
    // SAFETY: a new kqueue, owned below.
    let queue = unsafe { libc::kqueue() };
    if queue < 0 {
        return Exited::Unwatched;
    }
    // SAFETY: the descriptor kqueue just made.
    let queue = unsafe { OwnedFd::from_raw_fd(queue) };
    // SAFETY: a plain struct, set field by field below.
    let mut change: libc::kevent = unsafe { std::mem::zeroed() };
    change.ident = pid as libc::uintptr_t;
    change.filter = libc::EVFILT_PROC;
    change.flags = libc::EV_ADD | libc::EV_ONESHOT;
    change.fflags = libc::NOTE_EXIT | libc::NOTE_EXITSTATUS;
    loop {
        // SAFETY: a plain struct kevent fills.
        let mut event: libc::kevent = unsafe { std::mem::zeroed() };
        // SAFETY: one change and room for one event on an open kqueue; no timeout.
        let count = unsafe { libc::kevent(queue.as_raw_fd(), &change, 1, &mut event, 1, std::ptr::null()) };
        if count == 1 {
            if event.flags & libc::EV_ERROR != 0 {
                return if event.data as libc::c_int == libc::ESRCH { Exited::Before } else { Exited::Unwatched };
            }
            return Exited::Status(event.data as libc::c_int);
        }
        if count < 0 && io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
            continue;
        }
        return Exited::Unwatched;
    }
}

/// Ends a child and releases it when the task that owned it is dropped half-way (an aborted capture): the kill is immediate, the close
/// (which waits) runs on the runtime. Windows' `CloseOnDrop`.
pub(crate) struct CloseOnDrop(pub(crate) Option<Arc<dyn ChildControl>>);

impl CloseOnDrop {
    pub(crate) fn disarm(mut self) -> Arc<dyn ChildControl> {
        self.0.take().expect("armed")
    }
}

impl Drop for CloseOnDrop {
    fn drop(&mut self) {
        if let Some(control) = self.0.take() {
            control.kill();
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    let _ = control.close().await;
                });
            }
        }
    }
}

/// Process-wide: a descriptor of this crate is made and marked close-on-exec under the read side, a `fork` (the PTY host's) runs under
/// the write side, so no fork sees a descriptor not yet marked (macOS has no `pipe2`). `posix_spawn` needs neither side:
/// `POSIX_SPAWN_CLOEXEC_DEFAULT` hands its child only what its file actions name. A fork by code outside this crate is not covered.
pub(crate) static FORK: std::sync::RwLock<()> = std::sync::RwLock::new(());

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
    let _making = FORK.read().unwrap_or_else(|poisoned| poisoned.into_inner());
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
        // The leader stays a zombie until what it left in its group is ended, so the group's id is never another process's.
        let watched = wait_exited(pid);
        let early = if watched.zombie() { None } else { Some(reap(pid)) };
        lock(&reaper.state).reaped = true;
        reaped_sender.send_replace(Some(Instant::now()));
        // The leader can exit while a background child of its group still holds a pipe.
        terminate_group(pid);
        let code = early.unwrap_or_else(|| {
            let code = reap(pid);
            watched.status().map(decode).unwrap_or(code)
        });
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
    /// The leader has exited (set while it is a zombie, before the reap): `kill` signals the group no more.
    reaped: bool,
    /// The app ended the child while it ran (its output's rest is dropped on purpose: no last look).
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
        if !state.reaped {
            state.killed = true;
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

/// One of the child's outputs, read until its end — or, once the leader has exited, for one more second and at most 16 MB, then a
/// last look of at most 64 KB without waiting, after which the app's end is closed (Swift's reader, ProcessRunner.swift:117-170). The
/// group is gone by then, so what the last look finds is the child's own tail (a pipe holds no more than 64 KB): a consumer slowed by a
/// loaded machine still gets it, and only a writer outside the group loses the rest. Swift also gave a slow consumer up to 30 s more;
/// the run engine (C#'s) waits 3 s for the pipes after the exit, so the drain here stays within that.
struct ChildOutput {
    pipe: Option<pipe::Receiver>,
    shared: Arc<Shared>,
    /// Wakes the reader when the leader is reaped, while it waits for bytes.
    reaped: Option<BoxFuture<'static, ()>>,
    /// Wakes the reader when the drain after the exit ends.
    deadline: Option<Pin<Box<tokio::time::Sleep>>>,
    after_exit: usize,
    /// The last look's budget, once the drain ended on time.
    last_look: Option<usize>,
}

impl ChildOutput {
    fn new(pipe: pipe::Receiver, shared: Arc<Shared>) -> ChildOutput {
        ChildOutput { pipe: Some(pipe), shared, reaped: None, deadline: None, after_exit: 0, last_look: None }
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
                let stopped = lock(&this.shared.state).killed;
                let end = at + DRAIN_AFTER_EXIT;
                let over = this.after_exit > MAXIMUM_BYTES_AFTER_EXIT;
                if over || now >= end {
                    // A stop drops the rest on purpose, and past the byte cap a stray writer is reading's end; otherwise look once more,
                    // so a reader slowed by a loaded machine still hands on the child's own unread tail.
                    if stopped || over {
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

/// A detached process the app waits on.
struct Waitable {
    pid: libc::pid_t,
    /// Set while it is still a zombie, before the reap: `terminate` checks it under this lock.
    exited: Mutex<bool>,
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

    /// SIGKILL to this process alone (C#'s `Kill(entireProcessTree: false)`), while it has not exited: what it started keeps running.
    fn terminate(&self) {
        let exited = lock(&self.0.exited);
        if !*exited {
            // SAFETY: a signal to a child of the app that has not exited (the lock keeps the reaper from reaping it meanwhile).
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
