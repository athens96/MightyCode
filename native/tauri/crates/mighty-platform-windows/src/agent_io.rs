//! The Agent IO transport on Windows (C# `AgentIOPipe`, Core/AgentIO/AgentIOPipe.cs:15-161): a named pipe `mighty-agent-io-` + a GUID
//! in `N` format. Owner from W7 Step 0: W7-C.
//!
//! .NET's `PipeOptions.CurrentUserOnly`, both ends:
//! - Each server instance is made with a security descriptor whose owner is the process token's owner (`WindowsIdentity.Owner`: the
//!   user, or the Administrators group for an elevated token, so the account *and* the elevation must match) and whose access list
//!   has one entry, full control (`PipeAccessRights.FullControl`, 0x1F019F) for that owner. Remote clients are refused. The first
//!   instance is made by `bind` itself with `FILE_FLAG_FIRST_PIPE_INSTANCE` and kept for the first `accept`, so a name another process
//!   already holds is never served (`bind` picks another name) and none can slip in between the name's choice and its first instance.
//! - The client opens with `SECURITY_IDENTIFICATION` (the server can identify it but never act as it), retries a busy or missing
//!   pipe until its deadline (`ConnectAsync`), then checks that the pipe's owner is its own token's owner and refuses it otherwise
//!   (`ValidateRemotePipeUser`).
//!
//! Everything above the bytes (framing, limits, token, busy count, deadlines) is mighty-delegation's `agent_io`.

use std::ffi::c_void;
use std::os::windows::io::AsRawHandle;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use mighty_platform::{AgentIoConnection, AgentIoConnector, AgentIoListener, BoxFuture, PlatformError, Result};
use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeServer, PipeMode, ServerOptions};
use tokio::sync::Notify;
use windows_sys::Win32::Foundation::{CloseHandle, ERROR_FILE_NOT_FOUND, ERROR_PIPE_BUSY, HANDLE, HLOCAL, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo, SDDL_REVISION_1, SE_KERNEL_OBJECT,
};
use windows_sys::Win32::Security::{
    EqualSid, GetLengthSid, GetTokenInformation, IsValidSid, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES,
    TOKEN_OWNER, TOKEN_QUERY, TokenOwner,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

use crate::win::wide;

/// `AgentIOPipe.Prefix`.
const PREFIX: &str = "mighty-agent-io-";
/// MC-9: the local control API's pipe, `mighty-control-` + 32 hex digits (a new name each run, written to the profile's
/// `control/endpoint.json` for the `mighty` CLI).
pub const CONTROL_PREFIX: &str = "mighty-control-";
/// MC-3: the background daemon's pipe, `mighty-daemon-` + 32 hex digits (a new name each daemon, written to the profile's
/// `daemon/endpoint.json` for the app).
pub const DAEMON_PREFIX: &str = "mighty-daemon-";
/// How often a busy or missing pipe is tried again while connecting.
const RETRY: Duration = Duration::from_millis(50);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn refused(why: &str) -> PlatformError {
    PlatformError::Refused(why.into())
}

/// `mighty-agent-io-` and exactly 32 hex digits (the client's `IsPipeName`).
#[cfg(test)]
fn is_pipe_name(name: &str) -> bool {
    is_pipe_name_with(PREFIX, name)
}

/// `prefix` and exactly 32 hex digits.
fn is_pipe_name_with(prefix: &str, name: &str) -> bool {
    name.strip_prefix(prefix).is_some_and(|hex| hex.len() == 32 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
}

fn path(name: &str) -> String {
    format!(r"\\.\pipe\{name}")
}

/// The owner of this process's token (`WindowsIdentity.GetCurrent().Owner`), as its bytes.
pub fn token_owner() -> Result<Vec<u8>> {
    let mut token: HANDLE = null_mut();
    // SAFETY: the token is closed below; the buffer is u64-aligned for TOKEN_OWNER and sized by the first call; the SID is copied
    // out before the buffer goes.
    unsafe {
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return Err(refused("no token owner"));
        }
        let mut length = 0u32;
        GetTokenInformation(token, TokenOwner, null_mut(), 0, &mut length);
        let mut buffer = vec![0u64; (length as usize).div_ceil(8).max(1)];
        let read = GetTokenInformation(token, TokenOwner, buffer.as_mut_ptr() as *mut c_void, length, &mut length);
        CloseHandle(token);
        if read == 0 {
            return Err(refused("no token owner"));
        }
        let owner = (*(buffer.as_ptr() as *const TOKEN_OWNER)).Owner;
        Ok(sid_bytes(owner))
    }
}

/// A SID's bytes.
///
/// # Safety
/// `sid` points to a valid SID.
unsafe fn sid_bytes(sid: PSID) -> Vec<u8> {
    // SAFETY: the caller's valid SID, GetLengthSid bytes long.
    unsafe { std::slice::from_raw_parts(sid as *const u8, GetLengthSid(sid) as usize).to_vec() }
}

/// A SID's SDDL text (`S-1-5-…`).
fn sid_text(sid: &[u8]) -> Result<String> {
    let mut text: *mut u16 = null_mut();
    // SAFETY: a valid SID's bytes; the text is freed with LocalFree.
    unsafe {
        if IsValidSid(sid.as_ptr() as PSID) == 0 || ConvertSidToStringSidW(sid.as_ptr() as PSID, &mut text) == 0 {
            return Err(refused("the owner's SID could not be read"));
        }
        let length = (0..).take_while(|&at| *text.add(at) != 0).count();
        let value = String::from_utf16_lossy(std::slice::from_raw_parts(text, length));
        LocalFree(text as HLOCAL);
        Ok(value)
    }
}

/// Whether two SIDs are the same.
pub fn same_sid(a: &[u8], b: &[u8]) -> bool {
    // SAFETY: both are SID bytes; IsValidSid is asked before EqualSid reads them.
    unsafe {
        IsValidSid(a.as_ptr() as PSID) != 0 && IsValidSid(b.as_ptr() as PSID) != 0 && EqualSid(a.as_ptr() as PSID, b.as_ptr() as PSID) != 0
    }
}

/// The owner of a kernel object (`PipeSecurity.GetOwner`).
pub fn object_owner(object: &impl AsRawHandle) -> Result<Vec<u8>> {
    let handle = object.as_raw_handle() as HANDLE;
    let mut owner: PSID = null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
    // SAFETY: a live handle opened with READ_CONTROL (GENERIC_READ); the owner points into the descriptor, which is copied out and
    // freed with LocalFree.
    unsafe {
        let error = GetSecurityInfo(
            handle,
            SE_KERNEL_OBJECT,
            OWNER_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            null_mut(),
            null_mut(),
            &mut descriptor,
        );
        if error != 0 || owner.is_null() {
            if !descriptor.is_null() {
                LocalFree(descriptor as HLOCAL);
            }
            return Err(refused("the pipe's owner could not be read"));
        }
        let bytes = sid_bytes(owner);
        LocalFree(descriptor as HLOCAL);
        Ok(bytes)
    }
}

/// A security descriptor made from SDDL, freed on drop.
pub(crate) struct Descriptor(PSECURITY_DESCRIPTOR);

impl Descriptor {
    /// `CurrentUserOnly`'s server side: `owner` the owner, one entry granting it `PipeAccessRights.FullControl`.
    pub(crate) fn owner_only(owner: &[u8]) -> Result<Descriptor> {
        let sid = sid_text(owner)?;
        let sddl = wide(format!("O:{sid}D:(A;;0x1f019f;;;{sid})"));
        let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
        // SAFETY: a NUL-terminated SDDL string; the descriptor is allocated with LocalAlloc and freed on drop.
        let ok =
            unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), SDDL_REVISION_1, &mut descriptor, null_mut()) };
        if ok == 0 {
            return Err(refused("the pipe's access list"));
        }
        Ok(Descriptor(descriptor))
    }

    pub(crate) fn as_ptr(&self) -> PSECURITY_DESCRIPTOR {
        self.0
    }
}

impl Drop for Descriptor {
    fn drop(&mut self) {
        // SAFETY: allocated by ConvertStringSecurityDescriptorToSecurityDescriptorW, freed once.
        unsafe { LocalFree(self.0 as HLOCAL) };
    }
}

/// How many random names `bind` tries before it gives up (another process holding one is already a sign of trouble).
const NAME_ATTEMPTS: usize = 8;

#[derive(Default)]
struct State {
    name: Option<String>,
    /// The first instance, made by `bind` with `FILE_FLAG_FIRST_PIPE_INSTANCE`, until the first `accept` takes it.
    first: Option<NamedPipeServer>,
}

/// The app's listener (one per app).
pub struct NamedPipeListener {
    /// The names' prefix: the agents' pipe, or the control API's (MC-9).
    prefix: &'static str,
    state: Mutex<State>,
    closed: AtomicBool,
    close: Notify,
}

impl std::fmt::Debug for NamedPipeListener {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NamedPipeListener").field("name", &lock(&self.state).name).finish_non_exhaustive()
    }
}

impl Default for NamedPipeListener {
    fn default() -> NamedPipeListener {
        NamedPipeListener { prefix: PREFIX, state: Mutex::default(), closed: AtomicBool::new(false), close: Notify::new() }
    }
}

impl NamedPipeListener {
    pub fn new() -> NamedPipeListener {
        NamedPipeListener::default()
    }

    /// MC-9: the local control API's listener: the same owner-only pipe, remote clients refused, under [`CONTROL_PREFIX`].
    pub fn control() -> NamedPipeListener {
        NamedPipeListener { prefix: CONTROL_PREFIX, ..NamedPipeListener::default() }
    }

    /// MC-3: the background daemon's listener: the same owner-only pipe, remote clients refused, under [`DAEMON_PREFIX`].
    pub fn daemon() -> NamedPipeListener {
        NamedPipeListener { prefix: DAEMON_PREFIX, ..NamedPipeListener::default() }
    }

    /// Ends once [`AgentIoListener::close`] was called.
    async fn closed(&self) {
        loop {
            let notified = self.close.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.closed.load(Ordering::SeqCst) {
                return;
            }
            notified.await;
        }
    }

    /// One server instance of `name`, with the owner-only descriptor (made and freed here, before any await).
    fn create(name: &str, first: bool) -> Result<NamedPipeServer> {
        let descriptor = Descriptor::owner_only(&token_owner()?)?;
        let mut attributes =
            SECURITY_ATTRIBUTES { nLength: size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: descriptor.0, bInheritHandle: 0 };
        // SAFETY: the attributes and their descriptor live until the call returns; the handle is not inheritable.
        let server = unsafe {
            ServerOptions::new()
                .first_pipe_instance(first)
                .reject_remote_clients(true)
                .pipe_mode(PipeMode::Byte)
                .access_inbound(true)
                .access_outbound(true)
                .create_with_security_attributes_raw(path(name), &mut attributes as *mut SECURITY_ATTRIBUTES as *mut c_void)
        }?;
        Ok(server)
    }

    /// `bind` with the names `next` gives: the first one whose first instance this process could make (none other holds it).
    fn bind_with(&self, mut next: impl FnMut() -> Result<String>) -> Result<String> {
        let mut state = lock(&self.state);
        if let Some(name) = &state.name {
            return Ok(name.clone());
        }
        let mut last = refused("no Agent IO pipe name was free");
        for _ in 0..NAME_ATTEMPTS {
            let name = next()?;
            match Self::create(&name, true) {
                Ok(first) => {
                    state.first = Some(first);
                    state.name = Some(name.clone());
                    return Ok(name);
                }
                Err(error) => last = error,
            }
        }
        Err(last)
    }

    async fn accept_one(&self) -> Result<AgentIoConnection> {
        let (name, first) = {
            let mut state = lock(&self.state);
            let name = state.name.clone().ok_or_else(|| PlatformError::Invalid("the listener is not bound".into()))?;
            (name, state.first.take())
        };
        if self.closed.load(Ordering::SeqCst) {
            return Err(PlatformError::Io(std::io::ErrorKind::BrokenPipe.into()));
        }
        let server = match first {
            Some(server) => server,
            None => Self::create(&name, false)?,
        };
        tokio::select! {
            connected = server.connect() => connected?,
            _ = self.closed() => return Err(PlatformError::Io(std::io::ErrorKind::BrokenPipe.into())),
        }
        Ok(Box::new(server))
    }
}

impl AgentIoListener for NamedPipeListener {
    /// A random name and its first instance (inside a tokio runtime: the instance is registered with it).
    fn bind(&self) -> Result<String> {
        let prefix = self.prefix;
        self.bind_with(|| {
            let mut bytes = [0u8; 16];
            getrandom::fill(&mut bytes).map_err(|_| refused("no random numbers"))?;
            Ok(format!("{prefix}{}", bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()))
        })
    }

    fn accept(&self) -> BoxFuture<'_, Result<AgentIoConnection>> {
        Box::pin(self.accept_one())
    }

    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        lock(&self.state).first.take();
        self.close.notify_waiters();
    }
}

/// The MCP servers' client.
#[derive(Debug, Default, Clone, Copy)]
pub struct NamedPipeConnector;

impl NamedPipeConnector {
    async fn connect_one(name: String, timeout: Duration) -> Result<AgentIoConnection> {
        connect_with(PREFIX, name, timeout).await
    }
}

/// MC-9: the `mighty` CLI's client of the control API's pipe: only a [`CONTROL_PREFIX`] name, the owner checked as for Agent IO.
#[derive(Debug, Default, Clone, Copy)]
pub struct ControlPipeConnector;

impl AgentIoConnector for ControlPipeConnector {
    fn connect(&self, name: &str, timeout: Duration) -> BoxFuture<'_, Result<AgentIoConnection>> {
        Box::pin(connect_with(CONTROL_PREFIX, name.to_string(), timeout))
    }
}

/// MC-3: the app's client of the background daemon's pipe: only a [`DAEMON_PREFIX`] name, the owner checked as for Agent IO.
#[derive(Debug, Default, Clone, Copy)]
pub struct DaemonPipeConnector;

impl AgentIoConnector for DaemonPipeConnector {
    fn connect(&self, name: &str, timeout: Duration) -> BoxFuture<'_, Result<AgentIoConnection>> {
        Box::pin(connect_with(DAEMON_PREFIX, name.to_string(), timeout))
    }
}

/// MC-3: the background daemon's single-instance lock: `path` opened with no sharing at all, so a second open (another daemon of
/// the profile) fails with a sharing violation while this handle is open; the OS closes it with the process, a crash included.
pub fn lock_exclusive(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    use std::os::windows::fs::OpenOptionsExt as _;
    // FILE_FLAG_OPEN_REPARSE_POINT: a link is opened as itself, never followed.
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}

/// Opens the pipe `name` (which must be `prefix` and 32 hex digits) within `timeout` and checks that this user owns it.
async fn connect_with(prefix: &'static str, name: String, timeout: Duration) -> Result<AgentIoConnection> {
    if !is_pipe_name_with(prefix, &name) {
        return Err(PlatformError::Invalid("not an Agent IO pipe name".into()));
    }
    let deadline = tokio::time::Instant::now() + timeout;
    let client = loop {
        match ClientOptions::new().open(path(&name)) {
            Ok(client) => break client,
            Err(error)
                if error.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) || error.raw_os_error() == Some(ERROR_FILE_NOT_FOUND as i32) =>
            {
                let now = tokio::time::Instant::now();
                if now >= deadline {
                    return Err(PlatformError::TimedOut("the Agent IO pipe"));
                }
                tokio::time::sleep(RETRY.min(deadline - now)).await;
            }
            Err(error) => return Err(PlatformError::Io(error)),
        }
    };
    let owner = object_owner(&client)?;
    if !same_sid(&owner, &token_owner()?) {
        return Err(refused("the pipe is not owned by the current user"));
    }
    Ok(Box::new(client))
}

impl AgentIoConnector for NamedPipeConnector {
    fn connect(&self, name: &str, timeout: Duration) -> BoxFuture<'_, Result<AgentIoConnection>> {
        Box::pin(Self::connect_one(name.to_string(), timeout))
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    /// `S-1-5-18` (LocalSystem): an owner that is never this test's.
    const LOCAL_SYSTEM: [u8; 12] = [1, 1, 0, 0, 0, 0, 0, 5, 18, 0, 0, 0];

    /// The pipe is this user's alone: its owner is the token's owner, a client checks it, a name another instance already holds is
    /// never served, and a name that is not an Agent IO pipe is never opened.
    #[tokio::test]
    async fn the_pipe_is_the_current_users_alone() {
        let listener = NamedPipeListener::new();
        let name = listener.bind().unwrap();
        assert!(is_pipe_name(&name) && listener.bind().unwrap() == name, "one name per listener: {name}");
        let (server, client) = tokio::join!(listener.accept(), NamedPipeConnector.connect(&name, Duration::from_secs(5)));
        let (mut server, mut client) = (server.unwrap(), client.unwrap());
        client.write_all(b"ping").await.unwrap();
        let mut read = [0u8; 4];
        server.read_exact(&mut read).await.unwrap();
        assert_eq!(&read, b"ping");
        let owner = token_owner().unwrap();
        // The one instance is busy with the client above: the owner is read on a fresh one.
        let (_second, opened) = tokio::join!(listener.accept(), async {
            loop {
                match ClientOptions::new().open(path(&name)) {
                    Ok(client) => break client,
                    Err(_) => tokio::time::sleep(RETRY).await,
                }
            }
        });
        assert!(same_sid(&object_owner(&opened).unwrap(), &owner), "the pipe's owner is the token's owner");
        assert!(!same_sid(&owner, &LOCAL_SYSTEM) && same_sid(&LOCAL_SYSTEM, &LOCAL_SYSTEM), "another owner is told apart");
        assert!(matches!(NamedPipeConnector.connect("not-a-pipe", Duration::from_secs(1)).await, Err(PlatformError::Invalid(_))));
        let missing = format!("{PREFIX}{}", "0".repeat(32));
        assert!(matches!(NamedPipeConnector.connect(&missing, Duration::from_millis(120)).await, Err(PlatformError::TimedOut(_))));
        listener.close();
        assert!(listener.accept().await.is_err(), "a closed listener takes no one");

        // Squatting: a name another instance already holds is never joined; bind takes the next name, whose first instance it made.
        let held = format!("{PREFIX}{}", "a1".repeat(16));
        let free = format!("{PREFIX}{}", "b2".repeat(16));
        let squatter = ServerOptions::new().create(path(&held)).unwrap();
        let mut names = vec![free.clone(), held.clone()];
        let listener = NamedPipeListener::new();
        assert_eq!(listener.bind_with(|| Ok(names.pop().unwrap())).unwrap(), free, "the held name is passed over");
        let (accepted, client) = tokio::join!(listener.accept(), NamedPipeConnector.connect(&free, Duration::from_secs(5)));
        assert!(accepted.is_ok() && client.is_ok(), "the free name is served");
        let (squatted, joined) = tokio::join!(squatter.connect(), async { ClientOptions::new().open(path(&held)) });
        assert!(squatted.is_ok() && joined.is_ok(), "a client of the held name reaches the squatter, never this listener");
        let all_held = NamedPipeListener::new();
        let _taken = ServerOptions::new().create(path(&free)).unwrap();
        assert!(all_held.bind_with(|| Ok(free.clone())).is_err() && all_held.accept().await.is_err(), "no free name, no listener");
    }

    /// MC-3: the daemon's pipe is the same owner-only pipe under its own prefix; its lock has one holder at a time.
    #[tokio::test]
    async fn the_daemon_pipe_and_lock_are_the_current_users_alone() {
        let listener = NamedPipeListener::daemon();
        let name = listener.bind().unwrap();
        assert!(is_pipe_name_with(DAEMON_PREFIX, &name) && !is_pipe_name_with(CONTROL_PREFIX, &name), "{name}");
        let (server, client) = tokio::join!(listener.accept(), DaemonPipeConnector.connect(&name, Duration::from_secs(5)));
        assert!(server.is_ok() && client.is_ok());
        assert!(matches!(ControlPipeConnector.connect(&name, Duration::from_secs(1)).await, Err(PlatformError::Invalid(_))));
        listener.close();
        let folder = std::env::temp_dir().join(format!("mc-daemon-lock-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join("daemon.lock");
        let held = lock_exclusive(&path).unwrap();
        assert!(lock_exclusive(&path).is_err(), "a second holder is refused");
        drop(held);
        assert!(lock_exclusive(&path).is_ok());
        let _ = std::fs::remove_dir_all(&folder);
    }

    /// MC-9: the control API's pipe is the same owner-only pipe under its own prefix; each connector takes only its own names.
    #[tokio::test]
    async fn the_control_pipe_is_the_current_users_alone_under_its_own_prefix() {
        let listener = NamedPipeListener::control();
        let name = listener.bind().unwrap();
        assert!(is_pipe_name_with(CONTROL_PREFIX, &name) && !is_pipe_name(&name), "{name}");
        let (server, client) = tokio::join!(listener.accept(), ControlPipeConnector.connect(&name, Duration::from_secs(5)));
        assert!(server.is_ok() && client.is_ok());
        assert!(matches!(NamedPipeConnector.connect(&name, Duration::from_secs(1)).await, Err(PlatformError::Invalid(_))));
        let agents = format!("{PREFIX}{}", "0".repeat(32));
        assert!(matches!(ControlPipeConnector.connect(&agents, Duration::from_secs(1)).await, Err(PlatformError::Invalid(_))));
        listener.close();
    }
}
