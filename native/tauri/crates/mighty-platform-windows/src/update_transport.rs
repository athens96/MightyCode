//! The app update's HTTPS on Windows (W9-B; plan §1.3, §W9-B, D7; C# `AppUpdateService`'s `HttpClient` over
//! `SocketsHttpHandler{AllowAutoRedirect = false, UseProxy = false}`): a WinHTTP session with `WINHTTP_ACCESS_TYPE_NO_PROXY`, redirects
//! off (`WINHTTP_OPTION_REDIRECT_POLICY_NEVER`, `WINHTTP_DISABLE_REDIRECTS`), no cookies, no automatic logon, no User-Agent (C#'s
//! `HttpClient` sends none), TLS from the system; an address that is not `https://` refused before WinHTTP is opened. `get_capped`
//! reads a 2xx body up to its cap; `download` streams a 2xx body to its file, each read checked against the limit and the cancel flag,
//! progress after each. A 3xx comes back with its `Location` for the caller's redirect rule (https only, 5 hops: mighty-updates'
//! service). No address is put in an error or in Debug output (it may carry a token). Not [`crate::http::WinHttpGet`]: that one takes
//! the system proxy and answers text.

use std::ffi::c_void;
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mighty_platform::{BoxFuture, DownloadRequest, Downloaded, PlatformError, Result, UpdateResponse, UpdateTransport};
use windows_sys::Win32::Networking::WinHttp::{
    ERROR_WINHTTP_HEADER_NOT_FOUND, WINHTTP_ACCESS_TYPE_NO_PROXY, WINHTTP_AUTOLOGON_SECURITY_LEVEL_HIGH, WINHTTP_DISABLE_COOKIES,
    WINHTTP_DISABLE_REDIRECTS, WINHTTP_FLAG_SECURE, WINHTTP_OPTION_AUTOLOGON_POLICY, WINHTTP_OPTION_DISABLE_FEATURE,
    WINHTTP_OPTION_REDIRECT_POLICY, WINHTTP_OPTION_REDIRECT_POLICY_NEVER, WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_LOCATION,
    WINHTTP_QUERY_STATUS_CODE, WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest, WinHttpQueryDataAvailable,
    WinHttpQueryHeaders, WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest, WinHttpSetOption, WinHttpSetTimeouts,
};

use crate::win::{check, last_error, wide};

/// The longest a single WinHTTP phase (a connect, one read) may take; the whole request keeps its own timeout, and the watchdog ends a
/// stalled phase at once at a cancel or the deadline.
const PHASE_LIMIT: Duration = Duration::from_secs(60);

/// How often the watchdog looks at the cancel flag and the clock.
const WATCH_POLL: Duration = Duration::from_millis(50);

/// What one request goes to: an `https://` address split.
#[derive(Clone, PartialEq, Eq)]
struct Target {
    host: String,
    port: u16,
    /// Starts with `/`; the fragment is never sent.
    path_and_query: String,
}

/// The host and port only: the path may carry a token.
impl std::fmt::Debug for Target {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Target").field("host", &self.host).field("port", &self.port).finish_non_exhaustive()
    }
}

/// An `https://host[:port][/path][?query][#fragment]` address as its parts (a host name of ASCII letters, digits, dots and hyphens, an
/// IPv4 address, or an IPv6 one in brackets; the port 1–65535, 443 when absent); anything else — another scheme, user info, white space
/// or a control character, no host — refused before anything is opened. The words never repeat the address.
fn split(url: &str) -> Result<Target> {
    let refused = || PlatformError::Invalid("the update transport takes https:// addresses only".into());
    if url.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(refused());
    }
    let scheme = url.get(..8).filter(|scheme| scheme.eq_ignore_ascii_case("https://")).ok_or_else(refused)?;
    let rest = &url[scheme.len()..];
    let rest = rest.split('#').next().unwrap_or_default();
    let end = rest.find(['/', '?']).unwrap_or(rest.len());
    let (authority, tail) = (&rest[..end], &rest[end..]);
    if authority.contains('@') || authority.contains('\\') {
        return Err(refused());
    }
    let (host, port) = if let Some(inside) = authority.strip_prefix('[') {
        let (address, after) = inside.split_once(']').ok_or_else(refused)?;
        address.parse::<std::net::Ipv6Addr>().map_err(|_| refused())?;
        let port = match after {
            "" => None,
            after => Some(after.strip_prefix(':').ok_or_else(refused)?),
        };
        (address.to_string(), port)
    } else {
        match authority.split_once(':') {
            Some((host, port)) => (host.to_ascii_lowercase(), Some(port)),
            None => (authority.to_ascii_lowercase(), None),
        }
    };
    let named = !host.is_empty() && (host.contains(':') || host.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-'));
    if !named {
        return Err(refused());
    }
    let port = match port {
        Some(port) => port.parse::<u16>().ok().filter(|port| *port != 0).ok_or_else(refused)?,
        None => 443,
    };
    let path_and_query = match tail {
        "" => "/".to_string(),
        tail if tail.starts_with('?') => format!("/{tail}"),
        tail => tail.to_string(),
    };
    Ok(Target { host, port, path_and_query })
}

/// The WinHTTP handles of one request, closed in reverse order.
struct Handles(Vec<*mut c_void>);

impl Drop for Handles {
    fn drop(&mut self) {
        for handle in self.0.drain(..).rev() {
            // SAFETY: each handle came from WinHttpOpen, WinHttpConnect or WinHttpOpenRequest and is closed once.
            unsafe { WinHttpCloseHandle(handle) };
        }
    }
}

fn opened(handles: &mut Handles, handle: *mut c_void) -> Result<*mut c_void> {
    if handle.is_null() {
        return Err(last_error());
    }
    handles.0.push(handle);
    Ok(handle)
}

fn option(handle: *mut c_void, option: u32, value: u32) -> Result<()> {
    // SAFETY: `value` is a live u32 for the call, its size given.
    check(unsafe { WinHttpSetOption(handle, option, (&value as *const u32).cast(), 4) })
}

/// The request handle, shared with the [`Watchdog`] that closes it to end a blocking WinHTTP call at once (a cancel or the deadline:
/// WinHTTP ends a synchronous call on a handle another thread closes); None once closed. Kept as an address: a raw pointer is not `Send`.
type Slot = Arc<Mutex<Option<usize>>>;

fn close_slot(slot: &Slot) {
    if let Some(handle) = slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take() {
        // SAFETY: the request handle WinHttpOpenRequest gave, taken out of the slot so it is closed once.
        unsafe { WinHttpCloseHandle(handle as *mut c_void) };
    }
}

/// The request handle while it is open; an error once the watchdog closed it.
fn request_of(slot: &Slot) -> Result<*mut c_void> {
    slot.lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .map(|handle| handle as *mut c_void)
        .ok_or_else(|| PlatformError::Io(std::io::Error::from(std::io::ErrorKind::Interrupted)))
}

/// A sent GET whose status line and headers have arrived: the session and connection, and the request in its slot (closed first).
struct Answer {
    /// The session and connection, closed (after the request, in [`Drop`]) when the answer goes; only held.
    _handles: Handles,
    slot: Slot,
    status: u16,
    location: Option<String>,
}

impl Drop for Answer {
    fn drop(&mut self) {
        close_slot(&self.slot);
    }
}

impl Answer {
    /// Reads up to `buffer.len()` bytes of the body; 0 at its end.
    fn read(&self, buffer: &mut [u8]) -> Result<usize> {
        let mut available = 0u32;
        // SAFETY: a request handle whose response has arrived; `available` is writable.
        check(unsafe { WinHttpQueryDataAvailable(request_of(&self.slot)?, &mut available) })?;
        if available == 0 {
            return Ok(0);
        }
        let wanted = (available as usize).min(buffer.len()) as u32;
        let mut read = 0u32;
        // SAFETY: `buffer` holds at least `wanted` writable bytes.
        check(unsafe { WinHttpReadData(request_of(&self.slot)?, buffer.as_mut_ptr().cast(), wanted, &mut read) })?;
        Ok(read as usize)
    }
}

/// Why the watchdog closed the request.
const FIRED_CANCEL: u8 = 1;
const FIRED_DEADLINE: u8 = 2;

/// Watches one request on a thread of its own: when `cancel` is set or `deadline` passes, it closes the request handle, so a connect,
/// a send or a read stalled inside WinHTTP ends at once rather than after its phase timeout. Stopped (and joined) when dropped.
struct Watchdog {
    done: Arc<AtomicBool>,
    fired: Arc<AtomicU8>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Watchdog {
    fn start(slot: Slot, cancel: Arc<AtomicBool>, deadline: Instant) -> Result<Watchdog> {
        let done = Arc::new(AtomicBool::new(false));
        let fired = Arc::new(AtomicU8::new(0));
        let (stop, reason) = (done.clone(), fired.clone());
        let thread = std::thread::Builder::new().name("mighty-update-watchdog".into()).spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                let why = if cancel.load(Ordering::SeqCst) {
                    FIRED_CANCEL
                } else if Instant::now() >= deadline {
                    FIRED_DEADLINE
                } else {
                    0
                };
                if why != 0 {
                    reason.store(why, Ordering::SeqCst);
                    close_slot(&slot);
                    return;
                }
                std::thread::sleep(WATCH_POLL);
            }
        })?;
        Ok(Watchdog { done, fired, thread: Some(thread) })
    }

    /// A failure as the caller should see it: the cancel or the deadline when the watchdog fired, else the failure itself.
    fn explain(&self, error: PlatformError) -> PlatformError {
        match self.fired.load(Ordering::SeqCst) {
            FIRED_CANCEL => Stop::Cancelled.into(),
            FIRED_DEADLINE => Stop::TimedOut.into(),
            _ => error,
        }
    }
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        self.done.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// A header's text, None when the response has none.
fn header(request: *mut c_void, level: u32) -> Result<Option<String>> {
    let mut size = 0u32;
    // SAFETY: a null buffer asks for the size, written to `size`.
    let ok = unsafe { WinHttpQueryHeaders(request, level, std::ptr::null(), std::ptr::null_mut(), &mut size, std::ptr::null_mut()) };
    if ok != 0 {
        return Ok(Some(String::new()));
    }
    if std::io::Error::last_os_error().raw_os_error() == Some(ERROR_WINHTTP_HEADER_NOT_FOUND as i32) {
        return Ok(None);
    }
    let mut buffer = vec![0u16; size as usize / 2 + 1];
    let mut length = (buffer.len() * 2) as u32;
    // SAFETY: `buffer` holds `length` writable bytes.
    check(unsafe { WinHttpQueryHeaders(request, level, std::ptr::null(), buffer.as_mut_ptr().cast(), &mut length, std::ptr::null_mut()) })?;
    Ok(Some(String::from_utf16_lossy(&buffer[..length as usize / 2])))
}

/// Opens a session without a proxy, sends the GET and receives the status and headers, each WinHTTP phase within `phase`; the request
/// handle goes into `slot` as soon as it exists, for the watchdog.
fn send(target: &Target, phase: Duration, slot: Slot) -> Result<Answer> {
    let mut handles = Handles(vec![]);
    // SAFETY: no agent (no User-Agent header, as C#'s HttpClient), no proxy at all.
    let session = opened(&mut handles, unsafe {
        WinHttpOpen(std::ptr::null(), WINHTTP_ACCESS_TYPE_NO_PROXY, std::ptr::null(), std::ptr::null(), 0)
    })?;
    let milliseconds = phase.as_millis().clamp(1, i32::MAX as u128) as i32;
    // SAFETY: a session handle this call owns.
    check(unsafe { WinHttpSetTimeouts(session, milliseconds, milliseconds, milliseconds, milliseconds) })?;
    let host = wide(&target.host);
    // SAFETY: `host` is NUL-terminated.
    let connection = opened(&mut handles, unsafe { WinHttpConnect(session, host.as_ptr(), target.port, 0) })?;
    let (verb, path) = (wide("GET"), wide(&target.path_and_query));
    // SAFETY: NUL-terminated strings; default version, no referrer, any accept type; TLS.
    let request = unsafe {
        WinHttpOpenRequest(
            connection,
            verb.as_ptr(),
            path.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            WINHTTP_FLAG_SECURE,
        )
    };
    if request.is_null() {
        return Err(last_error());
    }
    *slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(request as usize);
    // From here the answer owns the request: dropped on any failure below, it closes the request before the connection and session.
    let mut answer = Answer { _handles: handles, slot, status: 0, location: None };
    option(request, WINHTTP_OPTION_REDIRECT_POLICY, WINHTTP_OPTION_REDIRECT_POLICY_NEVER)?;
    option(request, WINHTTP_OPTION_DISABLE_FEATURE, WINHTTP_DISABLE_COOKIES | WINHTTP_DISABLE_REDIRECTS)?;
    option(request, WINHTTP_OPTION_AUTOLOGON_POLICY, WINHTTP_AUTOLOGON_SECURITY_LEVEL_HIGH)?;
    // SAFETY: no extra headers, no body.
    check(unsafe { WinHttpSendRequest(request, std::ptr::null(), 0, std::ptr::null(), 0, 0, 0) })?;
    // SAFETY: the request was sent on this handle.
    check(unsafe { WinHttpReceiveResponse(request, std::ptr::null_mut()) })?;
    let mut status = 0u32;
    let mut size = 4u32;
    // SAFETY: `status` is a writable u32 of `size` bytes.
    check(unsafe {
        WinHttpQueryHeaders(
            request,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            std::ptr::null(),
            (&mut status as *mut u32).cast(),
            &mut size,
            std::ptr::null_mut(),
        )
    })?;
    answer.status = status as u16;
    answer.location = if (300..400).contains(&status) { header(request, WINHTTP_QUERY_LOCATION)?.filter(|l| !l.is_empty()) } else { None };
    Ok(answer)
}

/// Why a body stopped being taken.
#[derive(Debug)]
enum Stop {
    TooLarge(u64),
    Cancelled,
    TimedOut,
    Failed(PlatformError),
}

impl From<Stop> for PlatformError {
    fn from(stop: Stop) -> PlatformError {
        match stop {
            Stop::TooLarge(limit) => PlatformError::TooLarge(usize::try_from(limit).unwrap_or(usize::MAX)),
            Stop::Cancelled => PlatformError::Refused("cancelled".into()),
            Stop::TimedOut => PlatformError::TimedOut("the update download"),
            Stop::Failed(error) => error,
        }
    }
}

/// Takes a body from `read` (0 = its end) into `sink`, counting: more than `limit` bytes stops it (`TooLarge`, the bytes past the limit
/// never written), `cancel` set stops it before the next read, `deadline` passed stops it; `progress` hears the bytes written so far
/// after each write. Returns how many bytes it wrote.
fn take_body(
    mut read: impl FnMut(&mut [u8]) -> Result<usize>,
    sink: &mut dyn Write,
    limit: u64,
    progress: &dyn Fn(u64),
    cancel: &AtomicBool,
    deadline: Instant,
) -> std::result::Result<u64, Stop> {
    let mut buffer = vec![0u8; 64 * 1024];
    let mut written = 0u64;
    loop {
        if cancel.load(Ordering::SeqCst) {
            return Err(Stop::Cancelled);
        }
        if Instant::now() >= deadline {
            return Err(Stop::TimedOut);
        }
        let count = read(&mut buffer).map_err(Stop::Failed)?;
        if count == 0 {
            return Ok(written);
        }
        if written + count as u64 > limit {
            return Err(Stop::TooLarge(limit));
        }
        sink.write_all(&buffer[..count]).map_err(|error| Stop::Failed(error.into()))?;
        written += count as u64;
        progress(written);
    }
}

fn get_capped_now(url: &str, cap: usize, timeout: Duration) -> Result<UpdateResponse> {
    let target = split(url)?;
    let deadline = Instant::now() + timeout;
    let slot = Slot::default();
    let never = Arc::new(AtomicBool::new(false));
    let watchdog = Watchdog::start(slot.clone(), never.clone(), deadline)?;
    let answer = send(&target, timeout.min(PHASE_LIMIT), slot).map_err(|error| watchdog.explain(error))?;
    let mut body = Vec::new();
    if (200..300).contains(&answer.status) {
        take_body(|buffer| answer.read(buffer), &mut body, cap as u64, &|_| {}, &never, deadline)
            .map_err(|stop| watchdog.explain(stop.into()))?;
    }
    Ok(UpdateResponse { status: answer.status, location: answer.location.clone(), body })
}

fn download_now(request: &DownloadRequest) -> Result<Downloaded> {
    let target = split(&request.url)?;
    let deadline = Instant::now() + request.timeout;
    if request.cancel.load(Ordering::SeqCst) {
        return Err(Stop::Cancelled.into());
    }
    let slot = Slot::default();
    let watchdog = Watchdog::start(slot.clone(), request.cancel.clone(), deadline)?;
    let answer = send(&target, request.timeout.min(PHASE_LIMIT), slot).map_err(|error| watchdog.explain(error))?;
    if !(200..300).contains(&answer.status) {
        return Ok(Downloaded { status: answer.status, location: answer.location.clone(), written: 0 });
    }
    let mut file = std::io::BufWriter::new(std::fs::File::create(&request.destination)?);
    let progress = request.progress.clone();
    let written = take_body(|buffer| answer.read(buffer), &mut file, request.limit, &*progress, &request.cancel, deadline)
        .map_err(|stop| watchdog.explain(stop.into()))?;
    file.flush()?;
    file.into_inner().map_err(|error| PlatformError::Io(error.into_error()))?.sync_all()?;
    Ok(Downloaded { status: answer.status, location: None, written })
}

/// `work` on a blocking thread, awaited to its end (it bounds itself: the watchdog closes the request at a cancel or the deadline),
/// so a caller that removes the partial file afterwards finds it closed.
async fn blocking<T: Send + 'static>(work: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    match tokio::task::spawn_blocking(work).await {
        Ok(answer) => answer,
        Err(_) => Err(PlatformError::Io(std::io::Error::other("the update transport's thread ended"))),
    }
}

/// The Windows update transport. See [`mighty_platform::UpdateTransport`] for the contract.
#[derive(Debug, Clone, Copy, Default)]
pub struct WinHttpUpdates;

impl UpdateTransport for WinHttpUpdates {
    fn get_capped(&self, url: &str, cap: usize, timeout: Duration) -> BoxFuture<'_, Result<UpdateResponse>> {
        let url = url.to_string();
        Box::pin(async move {
            split(&url)?;
            blocking(move || get_capped_now(&url, cap, timeout)).await
        })
    }

    fn download(&self, request: DownloadRequest) -> BoxFuture<'_, Result<Downloaded>> {
        Box::pin(async move {
            split(&request.url)?;
            blocking(move || download_now(&request)).await
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only an `https://` address goes out; its host, port and path as WinHTTP takes them, the fragment never sent.
    #[test]
    fn only_an_https_address_is_split() {
        let at = |url: &str| split(url).unwrap();
        assert_eq!(
            at("https://pub-0123.r2.dev/latest.json?x=1#top"),
            Target { host: "pub-0123.r2.dev".into(), port: 443, path_and_query: "/latest.json?x=1".into() }
        );
        assert_eq!(at("HTTPS://Example.Invalid").path_and_query, "/");
        assert_eq!(at("https://example.invalid?a=b").path_and_query, "/?a=b");
        assert_eq!(at("https://localhost:8443/m/latest.json").port, 8443, "the rehearsal's local server");
        assert_eq!(at("https://127.0.0.1:9/x").host, "127.0.0.1");
        assert_eq!(at("https://[::1]:9/x").host, "::1");
        for bad in [
            "http://example.invalid/latest.json",
            "ftp://example.invalid/x",
            "https://",
            "https:///x",
            "https://user:token@example.invalid/x",
            "https://example.invalid:0/x",
            "https://example.invalid:70000/x",
            "https://example.invalid:x/x",
            "https://exa mple.invalid/x",
            "https://example.invalid/x\r\nHost: evil",
            "https://example.invalid\\evil.test/x",
            "https://[::1/x",
            "https://[::1]x/x",
            "https://\u{0430}.example/x",
            "example.invalid/x",
        ] {
            assert!(split(bad).is_err(), "{bad}");
        }
        let shown = format!("{:?}", at("https://example.invalid/latest.json?token=SECRET"));
        assert!(!shown.contains("SECRET") && !shown.contains("latest.json"), "Debug shows no path: {shown}");
    }

    /// A refused address is refused before WinHTTP opens anything, and the refusal does not repeat it.
    #[tokio::test]
    async fn an_address_that_is_not_https_is_refused_before_anything_is_opened() {
        let answer = WinHttpUpdates.get_capped("http://example.invalid/latest.json?token=SECRET", 10, Duration::from_secs(1)).await;
        match answer {
            Err(PlatformError::Invalid(words)) => assert!(!words.contains("SECRET") && !words.contains("example")),
            other => panic!("{other:?}"),
        }
    }

    /// The body's counting: the limit (no byte past it written), the cancel flag, the deadline, and progress after each write.
    #[test]
    fn a_body_is_taken_counting_against_its_limit_and_the_cancel() {
        let chunks = |sizes: Vec<usize>| {
            let mut sizes = sizes.into_iter();
            move |buffer: &mut [u8]| -> Result<usize> {
                let size = sizes.next().unwrap_or(0).min(buffer.len());
                buffer[..size].fill(7);
                Ok(size)
            }
        };
        let never = AtomicBool::new(false);
        let later = Instant::now() + Duration::from_secs(60);
        let seen = std::sync::Mutex::new(Vec::new());
        let mut sink = Vec::new();
        let written = take_body(chunks(vec![3, 4, 5]), &mut sink, 12, &|n| seen.lock().unwrap().push(n), &never, later).unwrap();
        assert_eq!((written, sink.len()), (12, 12), "exactly the limit is fine");
        assert_eq!(*seen.lock().unwrap(), vec![3, 7, 12], "progress after each write");

        let mut sink = Vec::new();
        let stop = take_body(chunks(vec![3, 4, 6]), &mut sink, 12, &|_| {}, &never, later);
        assert!(matches!(stop, Err(Stop::TooLarge(12))) && sink.len() == 7, "nothing past the limit is written");
        assert!(matches!(PlatformError::from(Stop::TooLarge(12)), PlatformError::TooLarge(12)));

        let cancel = AtomicBool::new(false);
        let mut sink = Vec::new();
        let mut reads = 0;
        let stop = take_body(
            |buffer: &mut [u8]| {
                reads += 1;
                if reads == 2 {
                    cancel.store(true, Ordering::SeqCst);
                }
                buffer[0] = 1;
                Ok(1)
            },
            &mut sink,
            100,
            &|_| {},
            &cancel,
            later,
        );
        assert!(matches!(stop, Err(Stop::Cancelled)) && sink.len() == 2, "the cancel is seen before the next read");
        assert!(matches!(PlatformError::from(Stop::Cancelled), PlatformError::Refused(words) if words == "cancelled"));

        let past = Instant::now();
        assert!(matches!(take_body(chunks(vec![1]), &mut Vec::new(), 10, &|_| {}, &never, past), Err(Stop::TimedOut)));
    }

    /// Review L2: a request stalled inside WinHTTP (a server that takes the connection and never answers the TLS handshake) ends at once
    /// when the download is cancelled, as `Refused("cancelled")`, and at its deadline as `TimedOut`, not after WinHTTP's phase timeout.
    #[tokio::test]
    async fn a_stalled_request_ends_at_the_cancel_and_at_the_deadline() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        // Accepts and holds every connection, answering nothing, while the test runs.
        let held = Arc::new(Mutex::new(Vec::new()));
        let keep = held.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                keep.lock().unwrap().push(stream);
            }
        });
        let folder = std::env::temp_dir().join(format!("mighty-update-transport-{}-{port}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let request = DownloadRequest {
            url: format!("https://127.0.0.1:{port}/package.zip"),
            destination: folder.join("package.zip"),
            limit: 1024,
            timeout: Duration::from_secs(600),
            progress: Arc::new(|_| {}),
            cancel: cancel.clone(),
        };
        let flag = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(500));
            flag.store(true, Ordering::SeqCst);
        });
        let started = Instant::now();
        let answer = WinHttpUpdates.download(request).await;
        assert!(matches!(&answer, Err(PlatformError::Refused(words)) if words == "cancelled"), "{answer:?}");
        assert!(started.elapsed() < Duration::from_secs(10), "ended at the cancel: {:?}", started.elapsed());

        let started = Instant::now();
        let answer = WinHttpUpdates.get_capped(&format!("https://127.0.0.1:{port}/latest.json"), 1024, Duration::from_millis(700)).await;
        assert!(matches!(answer, Err(PlatformError::TimedOut(_))), "{answer:?}");
        assert!(started.elapsed() < Duration::from_secs(10), "ended at the deadline: {:?}", started.elapsed());
        drop(held);
        std::fs::remove_dir_all(&folder).ok();
    }

    /// No proxy: the request goes straight to the address's own host and port (here a loopback listener of the test's own, which
    /// receives a TLS ClientHello itself rather than a proxy's CONNECT), and a server that is not TLS ends it with an error, not a hang.
    #[tokio::test]
    async fn the_request_goes_straight_to_its_host_without_a_proxy() {
        use std::io::Read;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let server = std::thread::spawn(move || {
            let end = Instant::now() + Duration::from_secs(10);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(_) if Instant::now() < end => std::thread::sleep(Duration::from_millis(20)),
                    Err(error) => panic!("nothing connected to the address itself: {error}"),
                }
            };
            stream.set_nonblocking(false).unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            let mut first = [0u8; 1];
            stream.read_exact(&mut first).unwrap();
            first[0]
        });
        let answer = WinHttpUpdates.get_capped(&format!("https://127.0.0.1:{port}/latest.json"), 1024, Duration::from_secs(10)).await;
        assert_eq!(server.join().unwrap(), 0x16, "a TLS handshake record arrived at the address itself");
        assert!(answer.is_err(), "{answer:?}");
    }
}
