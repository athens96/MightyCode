//! M-C: Agent IO over a Unix socket (Swift `AgentIOSocketServer.swift`'s `AgentIOWire.socketPath`, `AgentIOSocketServer.start`/`stop`
//! and `AgentIOSocketClient.exchange`). Contract `mighty_platform::agent_io`; the framing (one JSON line of at most 1 MiB, W7 D4), the
//! token, the limits and the deadlines are mighty-delegation's `agent_io`, chosen by `transport()` = `McpTransport::UnixSocket`, whose
//! environment key is `MIGHTY_AGENT_IO_SOCKET`.
//!
//! - The socket is `<data>/agent-io/io.sock`, or `$TMPDIR/mc-<uid>/<8 hex>.sock` when that path does not fit `sun_path` (104 bytes
//!   with its NUL), the hex being the low 32 bits of the FNV-1a 64 hash of the data folder: one short, stable name per profile.
//! - Its folder is made 0700 (and must be a real folder of this user's, never a link), any old socket file is removed, the socket is
//!   bound and made 0600 before it listens; `close` (and the listener's drop) removes it again.
//! - Both ends check the other's user with `getpeereid` (tokio's `peer_cred`): the listener drops a peer of another user and waits
//!   for the next, the connector refuses a socket another user serves.

use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use mighty_platform::{AgentIoConnection, AgentIoConnector, AgentIoListener, BoxFuture, McpTransport, PlatformError, Result};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Notify;

/// `sockaddr_un.sun_path`'s size on the Mac: a path must be shorter (its NUL ends it).
pub const SUN_PATH: usize = 104;
/// How often a missing or refusing socket is tried again while connecting (the Windows pipe's pace).
const RETRY: Duration = Duration::from_millis(50);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn user() -> u32 {
    // SAFETY: getuid has no preconditions and cannot fail.
    unsafe { libc::getuid() }
}

/// Whether `path` fits `sun_path` (Swift `AgentIOWire.address`): not empty and shorter than 104 bytes.
pub fn fits(path: &str) -> bool {
    !path.is_empty() && path.len() < SUN_PATH
}

/// Swift `AgentIOWire.socketPath(dataDirectory:temporaryDirectory:)`: `<data>/agent-io/io.sock` when it fits `sun_path`, else
/// `<temp>/mc-<uid>/<%08x>.sock`, the low 32 bits of the FNV-1a 64 hash of the data folder's path (trailing slashes aside).
pub fn socket_path(data_dir: &Path, temp_dir: &Path) -> PathBuf {
    let preferred = data_dir.join("agent-io").join("io.sock");
    if fits(&preferred.to_string_lossy()) {
        return preferred;
    }
    let text = data_dir.to_string_lossy();
    let text = match text.trim_end_matches('/') {
        "" => "/",
        trimmed => trimmed,
    };
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        hash = (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3);
    }
    temp_dir.join(format!("mc-{}", user())).join(format!("{:08x}.sock", hash as u32))
}

/// The socket's folder, made if missing and set to 0700; refused unless it is a real folder (not a link) owned by this user.
fn private_folder(folder: &Path) -> Result<()> {
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(folder)?;
    let meta = std::fs::symlink_metadata(folder)?;
    if !meta.is_dir() || meta.file_type().is_symlink() || meta.uid() != user() {
        return Err(PlatformError::Refused("the agent IO socket folder is not this user's own folder".into()));
    }
    std::fs::set_permissions(folder, std::fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[derive(Default)]
struct State {
    path: Option<PathBuf>,
    /// Bound by `bind`, registered with tokio at the first `accept` (which runs on a runtime; `bind` may not).
    bound: Option<std::os::unix::net::UnixListener>,
    listener: Option<Arc<UnixListener>>,
    /// The socket file this listener put in place (device, inode): `close` removes the path only while it is still that file.
    file: Option<(u64, u64)>,
    /// The socket's folder when this listener made it: `close` removes it again once empty, so a run leaves the profile as it was.
    made: Option<PathBuf>,
}

/// The app's listener (one per app and profile).
pub struct UnixSocketListener {
    data_dir: PathBuf,
    temp_dir: PathBuf,
    state: Mutex<State>,
    closed: AtomicBool,
    close: Notify,
}

impl std::fmt::Debug for UnixSocketListener {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UnixSocketListener").field("path", &lock(&self.state).path).finish_non_exhaustive()
    }
}

impl UnixSocketListener {
    /// The listener for the profile whose data folder is `data_dir`; its fallback folder is under the system's temporary folder.
    pub fn new(data_dir: &Path) -> UnixSocketListener {
        UnixSocketListener::in_folders(data_dir, &std::env::temp_dir())
    }

    /// As [`UnixSocketListener::new`], with the fallback under `temp_dir`.
    pub fn in_folders(data_dir: &Path, temp_dir: &Path) -> UnixSocketListener {
        UnixSocketListener {
            data_dir: data_dir.to_path_buf(),
            temp_dir: temp_dir.to_path_buf(),
            state: Mutex::new(State::default()),
            closed: AtomicBool::new(false),
            close: Notify::new(),
        }
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

    fn listener(&self) -> Result<Arc<UnixListener>> {
        let mut state = lock(&self.state);
        if let Some(listener) = &state.listener {
            return Ok(listener.clone());
        }
        let bound = state.bound.take().ok_or_else(|| PlatformError::Invalid("the listener is not bound".into()))?;
        let listener = Arc::new(UnixListener::from_std(bound)?);
        state.listener = Some(listener.clone());
        Ok(listener)
    }

    async fn accept_one(&self) -> Result<AgentIoConnection> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(PlatformError::Io(std::io::ErrorKind::BrokenPipe.into()));
        }
        let listener = self.listener()?;
        loop {
            let accepted = tokio::select! {
                accepted = listener.accept() => accepted,
                _ = self.closed() => return Err(PlatformError::Io(std::io::ErrorKind::BrokenPipe.into())),
            };
            let stream = match accepted {
                Ok((stream, _)) => stream,
                // A peer that went before it was taken, or a moment without descriptors: the listener goes on (Swift's loop).
                Err(error)
                    if matches!(error.kind(), std::io::ErrorKind::ConnectionAborted | std::io::ErrorKind::Interrupted)
                        || error.raw_os_error() == Some(libc::EMFILE)
                        || error.raw_os_error() == Some(libc::ENFILE) =>
                {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    continue;
                }
                Err(error) => return Err(PlatformError::Io(error)),
            };
            // Swift: `getpeereid(client) == 0, uid == getuid()`, else the client is closed and the next one awaited.
            if stream.peer_cred().is_ok_and(|peer| peer.uid() == user()) {
                return Ok(Box::new(stream));
            }
        }
    }
}

impl AgentIoListener for UnixSocketListener {
    /// The socket's path, bound once (later calls give the same path).
    fn bind(&self) -> Result<String> {
        let mut state = lock(&self.state);
        if let Some(path) = &state.path {
            return Ok(path.to_string_lossy().into_owned());
        }
        if self.closed.load(Ordering::SeqCst) {
            return Err(PlatformError::Io(std::io::ErrorKind::BrokenPipe.into()));
        }
        let path = socket_path(&self.data_dir, &self.temp_dir);
        let text = path.to_string_lossy().into_owned();
        if !fits(&text) {
            return Err(PlatformError::Invalid("the agent IO socket path does not fit sun_path".into()));
        }
        let folder = path.parent().ok_or_else(|| PlatformError::Invalid("the agent IO socket has no folder".into()))?;
        let made = !folder.exists();
        private_folder(folder)?;
        // An old socket (a previous run that ended without closing: nothing answers it) is replaced; one another instance still
        // serves is never taken over, and a folder in its place is not removed.
        match std::fs::symlink_metadata(&path) {
            Ok(meta) if meta.is_dir() => return Err(PlatformError::Refused("a folder holds the agent IO socket's name".into())),
            Ok(_) if std::os::unix::net::UnixStream::connect(&path).is_ok() => {
                return Err(PlatformError::Refused("another instance serves this profile's agent IO socket".into()));
            }
            Ok(_) => std::fs::remove_file(&path)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(PlatformError::Io(error)),
        }
        // Bound, made 0600 and listening under a name of its own in the private folder, then renamed into place: a client finds the
        // socket only once it is listening and private (never bound but not yet listening, or still with the umask's mode).
        let staging = folder.join(format!(".{}", std::process::id()));
        let staging = if fits(&staging.to_string_lossy()) { staging } else { path.clone() };
        let _ = std::fs::remove_file(&staging);
        let bound = std::os::unix::net::UnixListener::bind(&staging)?;
        let ready = std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o600))
            .and_then(|()| bound.set_nonblocking(true))
            .and_then(|()| if staging == path { Ok(()) } else { std::fs::rename(&staging, &path) });
        if let Err(error) = ready {
            drop(bound);
            let _ = std::fs::remove_file(&staging);
            return Err(PlatformError::Io(error));
        }
        state.file = std::fs::symlink_metadata(&path).ok().map(|meta| (meta.dev(), meta.ino()));
        state.bound = Some(bound);
        state.made = made.then(|| folder.to_path_buf());
        state.path = Some(path);
        Ok(text)
    }

    fn accept(&self) -> BoxFuture<'_, Result<AgentIoConnection>> {
        Box::pin(self.accept_one())
    }

    /// Every waiting `accept` fails and the socket file is removed (Swift `stop`).
    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        let (path, file, bound, listener, made) = {
            let mut state = lock(&self.state);
            (state.path.clone(), state.file.take(), state.bound.take(), state.listener.take(), state.made.take())
        };
        drop(bound);
        drop(listener);
        self.close.notify_waiters();
        // Only the file this listener put there: a socket another instance has since bound under the name stays.
        if let (Some(path), Some(file)) = (path, file)
            && std::fs::symlink_metadata(&path).is_ok_and(|meta| (meta.dev(), meta.ino()) == file)
        {
            let _ = std::fs::remove_file(path);
        }
        // Only when empty: another instance's socket or anything else put there keeps it.
        if let Some(folder) = made {
            let _ = std::fs::remove_dir(folder);
        }
    }

    fn transport(&self) -> McpTransport {
        McpTransport::UnixSocket
    }
}

impl Drop for UnixSocketListener {
    fn drop(&mut self) {
        if !self.closed.load(Ordering::SeqCst) && lock(&self.state).path.is_some() {
            self.close();
        }
    }
}

/// The MCP servers' client.
#[derive(Debug, Default, Clone, Copy)]
pub struct UnixSocketConnector;

impl UnixSocketConnector {
    async fn connect_one(name: String, timeout: Duration) -> Result<AgentIoConnection> {
        if !name.starts_with('/') || name.contains('\0') || !fits(&name) {
            return Err(PlatformError::Invalid("not an agent IO socket path".into()));
        }
        let deadline = tokio::time::Instant::now() + timeout;
        let stream = loop {
            let attempt = tokio::time::timeout_at(deadline, UnixStream::connect(&name)).await;
            match attempt {
                Ok(Ok(stream)) => break stream,
                // The app not listening yet (or between a stop and a start): tried again until the deadline, as the pipe is.
                Ok(Err(error))
                    if matches!(error.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused)
                        || error.raw_os_error() == Some(libc::EAGAIN) =>
                {
                    let now = tokio::time::Instant::now();
                    if now >= deadline {
                        return Err(PlatformError::TimedOut("the agent IO socket"));
                    }
                    tokio::time::sleep(RETRY.min(deadline - now)).await;
                }
                Ok(Err(error)) => return Err(PlatformError::Io(error)),
                Err(_) => return Err(PlatformError::TimedOut("the agent IO socket")),
            }
        };
        if !stream.peer_cred().is_ok_and(|peer| peer.uid() == user()) {
            return Err(PlatformError::Refused("the agent IO socket is not served by the current user".into()));
        }
        Ok(Box::new(stream))
    }
}

impl AgentIoConnector for UnixSocketConnector {
    fn connect(&self, name: &str, timeout: Duration) -> BoxFuture<'_, Result<AgentIoConnection>> {
        Box::pin(Self::connect_one(name.to_string(), timeout))
    }

    fn transport(&self) -> McpTransport {
        McpTransport::UnixSocket
    }
}

/// The app's listener for the profile whose data folder is `data_dir` (src-tauri platform.rs).
pub fn agent_io_listener(data_dir: &Path) -> Arc<dyn AgentIoListener> {
    Arc::new(UnixSocketListener::new(data_dir))
}

/// The exe's MCP modes' client (`--agent-io-mcp`, `--agent-delegation-mcp`).
pub fn agent_io_connector() -> Arc<dyn AgentIoConnector> {
    Arc::new(UnixSocketConnector)
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    /// A short, fresh folder under `/tmp` (Swift's `shortTemporaryDirectory`), removed by the caller.
    fn short_folder(name: &str) -> PathBuf {
        let folder = PathBuf::from(format!("/tmp/mc-test-{name}-{}-{}", std::process::id(), uuid_like()));
        std::fs::create_dir_all(&folder).unwrap();
        folder
    }

    fn uuid_like() -> String {
        let mut bytes = [0u8; 4];
        // SAFETY: a 4-byte buffer for arc4random_buf.
        unsafe { libc::arc4random_buf(bytes.as_mut_ptr().cast(), bytes.len()) };
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    fn mode(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    /// `AgentIOMCPServerTests.swift:315 "socketPathStaysUnderTheUnixLimitAndFallsBackToAShortTmpFolder"`.
    #[test]
    fn socket_path_stays_under_the_unix_limit_and_falls_back_to_a_short_tmp_folder() {
        let temp = Path::new("/var/folders/xy/abcdefgh/T/");
        assert_eq!(socket_path(Path::new("/tmp/mc-profile"), temp), PathBuf::from("/tmp/mc-profile/agent-io/io.sock"));
        let long = format!("/Users/someone/Library/Application Support/{}", "MightyCode Profile ".repeat(5));
        let fallback = socket_path(Path::new(&long), temp);
        let text = fallback.to_string_lossy().into_owned();
        assert!(text.len() < SUN_PATH, "{text}");
        assert!(text.starts_with(&format!("/var/folders/xy/abcdefgh/T/mc-{}/", user())), "{text}");
        assert_eq!(fallback, socket_path(Path::new(&long), temp), "one stable name per profile");
        assert_ne!(fallback, socket_path(Path::new(&format!("{long}Other")), temp), "another profile, another name");
        assert_eq!(socket_path(Path::new(&format!("{long}/")), temp), fallback, "a trailing slash names the same folder");
        // Swift's FNV-1a 64, its low 32 bits: the name a 0.4.x build would also have chosen for the same folder.
        let mut hash: u64 = 0xcbf29ce484222325;
        for byte in long.trim_end_matches('/').bytes() {
            hash = (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
        }
        assert!(text.ends_with(&format!("/{:08x}.sock", hash as u32)));
    }

    /// The platform half of `AgentIOMCPServerTests.swift:327 "socketIsPrivateResolvesTokensToTheirPaneAndIsRemovedOnStop"` (the token
    /// half runs over this socket in mighty-delegation's tests/agent_io.rs): the folder is 0700, the socket 0600, one peer of this user
    /// is served, a stale socket file is replaced, and the socket goes with its listener.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_socket_is_private_and_is_removed_on_close() {
        let folder = short_folder("private");
        // A stale socket file from an earlier run is replaced.
        let path = socket_path(&folder, &std::env::temp_dir());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"stale").unwrap();
        let listener = UnixSocketListener::new(&folder);
        let name = listener.bind().unwrap();
        assert_eq!(name, path.to_string_lossy(), "the profile's socket");
        assert_eq!(listener.bind().unwrap(), name, "one name per listener");
        assert!(listener.transport() == McpTransport::UnixSocket && UnixSocketConnector.transport() == McpTransport::UnixSocket);
        assert_eq!(mode(path.parent().unwrap()), 0o700, "the folder is the user's alone");
        assert_eq!(mode(&path), 0o600, "the socket is the user's alone");

        let (server, client) = tokio::join!(listener.accept(), UnixSocketConnector.connect(&name, Duration::from_secs(5)));
        let (mut server, mut client) = (server.unwrap(), client.unwrap());
        client.write_all(b"{\"ping\":1}\n").await.unwrap();
        let mut read = [0u8; 11];
        server.read_exact(&mut read).await.unwrap();
        assert_eq!(&read, b"{\"ping\":1}\n");

        drop(listener);
        assert!(!path.exists(), "the socket is removed once its listener goes");
        assert!(path.parent().unwrap().exists(), "a folder it found stays");
        let _ = std::fs::remove_dir_all(&folder);
    }

    /// Mac stage integration (the CEF smoke's read-only profile): a socket folder the listener made goes with it once empty, so a run
    /// leaves the profile as it found it.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_socket_folder_it_made_goes_on_close() {
        let folder = short_folder("made");
        let path = socket_path(&folder, &std::env::temp_dir());
        assert!(!path.parent().unwrap().exists());
        let listener = UnixSocketListener::new(&folder);
        listener.bind().unwrap();
        assert!(path.exists());
        listener.close();
        assert!(!path.parent().unwrap().exists(), "the folder it made is gone");
        let _ = std::fs::remove_dir_all(&folder);
    }

    /// `close` wakes every waiting `accept` with an error, removes the socket, and a closed listener binds no more.
    #[tokio::test(flavor = "multi_thread")]
    async fn closing_fails_every_waiting_accept_and_removes_the_socket() {
        let folder = short_folder("close");
        let listener = Arc::new(UnixSocketListener::new(&folder));
        let name = listener.bind().unwrap();
        let waiting: Vec<_> = (0..4)
            .map(|_| {
                let listener = listener.clone();
                tokio::spawn(async move { listener.accept().await.map(|_| ()) })
            })
            .collect();
        tokio::time::sleep(Duration::from_millis(100)).await;
        listener.close();
        for task in waiting {
            assert!(tokio::time::timeout(Duration::from_secs(5), task).await.unwrap().unwrap().is_err(), "a waiting accept fails");
        }
        assert!(!Path::new(&name).exists(), "the socket is removed on close");
        assert!(listener.accept().await.is_err(), "a closed listener takes no one");
        assert!(
            matches!(UnixSocketConnector.connect(&name, Duration::from_millis(150)).await, Err(PlatformError::TimedOut(_))),
            "nothing answers once it is closed"
        );
        let _ = std::fs::remove_dir_all(&folder);
    }

    /// A socket another listener still serves is never taken over, and a listener's close leaves a socket file it no longer owns.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_live_socket_is_never_taken_over_and_close_removes_only_its_own_file() {
        let folder = short_folder("live");
        let first = UnixSocketListener::new(&folder);
        let name = first.bind().unwrap();
        let second = UnixSocketListener::new(&folder);
        assert!(matches!(second.bind(), Err(PlatformError::Refused(_))), "the first one still serves it");
        assert!(Path::new(&name).exists(), "the live socket stays");
        drop(second);
        assert!(Path::new(&name).exists(), "a listener that never bound removes nothing");
        // The name replaced under the first listener (a later instance after a stale socket): the first one's close leaves it.
        std::fs::remove_file(&name).unwrap();
        let third = UnixSocketListener::new(&folder);
        assert_eq!(third.bind().unwrap(), name);
        first.close();
        assert!(Path::new(&name).exists(), "the newer socket is not the first listener's to remove");
        third.close();
        assert!(!Path::new(&name).exists());
        let _ = std::fs::remove_dir_all(&folder);
    }

    /// A long data folder falls back to the short folder under the temporary one, which is made 0700 too.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_long_profile_folder_listens_in_the_short_fallback() {
        let folder = short_folder("long");
        let long = folder.join("p".repeat(120));
        let temp = short_folder("temp");
        let listener = UnixSocketListener::in_folders(&long, &temp);
        let name = listener.bind().unwrap();
        assert!(name.starts_with(&format!("{}/mc-{}/", temp.display(), user())) && fits(&name), "{name}");
        assert_eq!(mode(Path::new(&name).parent().unwrap()), 0o700);
        let (server, client) = tokio::join!(listener.accept(), UnixSocketConnector.connect(&name, Duration::from_secs(5)));
        assert!(server.is_ok() && client.is_ok());
        listener.close();
        let _ = std::fs::remove_dir_all(&folder);
        let _ = std::fs::remove_dir_all(&temp);
    }

    /// The connector takes only an absolute path that fits `sun_path`, waits for a listener that is not there yet until its deadline,
    /// and a folder linked in place of the socket's own is refused.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_connector_takes_only_a_socket_path_and_waits_out_its_deadline() {
        for bad in ["io.sock", "", "/tmp/a\0b", &format!("/{}", "x".repeat(SUN_PATH))] {
            assert!(matches!(UnixSocketConnector.connect(bad, Duration::from_secs(1)).await, Err(PlatformError::Invalid(_))), "{bad:?}");
        }
        let folder = short_folder("late");
        let listener = Arc::new(UnixSocketListener::new(&folder));
        let name = socket_path(&folder, &std::env::temp_dir()).to_string_lossy().into_owned();
        let late = {
            let listener = listener.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(300)).await;
                listener.bind().unwrap();
                // The accepted end is kept until the client has checked its peer: a peer already gone has no credentials to read.
                let accepted = listener.accept().await;
                tokio::time::sleep(Duration::from_millis(500)).await;
                accepted.map(|_| ())
            })
        };
        let reached = UnixSocketConnector.connect(&name, Duration::from_secs(5)).await;
        assert!(reached.is_ok(), "a listener that comes late is reached: {:?}", reached.err());
        assert!(late.await.unwrap().is_ok());
        listener.close();

        let linked = short_folder("linked");
        let target = short_folder("target");
        std::os::unix::fs::symlink(&target, linked.join("agent-io")).unwrap();
        assert!(matches!(UnixSocketListener::new(&linked).bind(), Err(PlatformError::Refused(_))), "a linked folder is refused");
        for folder in [folder, linked, target] {
            let _ = std::fs::remove_dir_all(folder);
        }
    }
}
