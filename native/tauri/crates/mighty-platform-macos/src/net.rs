//! M-C: the network for the direct path and remote sharing (Swift `TailscaleAddresses.interfaceAddresses`, `MobileDirectListener`'s
//! and `HTTPServer`'s parameters, `RelayLink`'s path monitor and wake). Contract `mighty_platform::net` (`Network`).
//!
//! - `adapters`: `getifaddrs`, every interface with `IFF_UP` and its IPv4 and IPv6 addresses, grouped by name in the order read (a
//!   Tailscale tunnel is `utun…`, which `PlatformProfile::tailscale_needs_utun` asks for). `getifaddrs` reads the whole list or fails,
//!   so nothing counts as unreadable.
//! - `bind_exclusive`: a TCP socket with neither `SO_REUSEADDR` nor `SO_REUSEPORT` (Swift's `allowLocalEndpointReuse = false`: no
//!   second socket can share the address and port), IPv6 only on an IPv6 address, and pinned with `IP_BOUND_IF`/`IPV6_BOUND_IF` to the
//!   interface that carries the address (Swift's `requiredInterface`, the `utun` the address is on); loopback is never pinned. BSD has no
//!   `SO_EXCLUSIVEADDRUSE`: a wildcard listener another program makes with `SO_REUSEADDR` could still take the port's other addresses.
//! - `on_change`: a routing socket (`PF_ROUTE`) read on a thread of its own; an address added or removed (`RTM_NEWADDR`, `RTM_DELADDR`)
//!   or an interface going up or down (`RTM_IFINFO`) calls back. `NWPathMonitor` reports the same moments (and more) to Swift.
//! - `on_resume`: `NSWorkspaceDidWakeNotification` from the workspace's notification center (Swift's `NSWorkspace.didWakeNotification`
//!   observer), delivered on the thread that posts it (AppKit's main thread).

use std::collections::BTreeMap;
use std::ffi::CStr;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener};
use std::os::fd::{FromRawFd as _, OwnedFd};
use std::ptr::NonNull;
use std::sync::{Arc, Mutex};

use mighty_platform::{Adapter, Adapters, Network, NetworkCallback, PlatformError, Result, Subscription};

/// The Mac's `Network`.
#[derive(Debug, Default, Clone, Copy)]
pub struct MacNetwork;

/// The address a `sockaddr` holds, when it is IPv4 or IPv6.
///
/// # Safety
/// `address` is null or points at a socket address the OS wrote, as long as its family says.
unsafe fn read_address(address: *const libc::sockaddr) -> Option<IpAddr> {
    if address.is_null() {
        return None;
    }
    // SAFETY: the caller's contract; the family names which structure is there.
    unsafe {
        match i32::from((*address).sa_family) {
            libc::AF_INET => {
                let v4 = &*(address as *const libc::sockaddr_in);
                Some(IpAddr::V4(Ipv4Addr::from(u32::from_be(v4.sin_addr.s_addr))))
            }
            libc::AF_INET6 => {
                let v6 = &*(address as *const libc::sockaddr_in6);
                Some(IpAddr::V6(Ipv6Addr::from(v6.sin6_addr.s6_addr)))
            }
            _ => None,
        }
    }
}

/// Every up interface's IPv4 and IPv6 addresses (name, address), in the order `getifaddrs` lists them.
fn interface_addresses() -> Result<Vec<(String, IpAddr)>> {
    let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: getifaddrs writes a list it allocated into `head`, freed below.
    if unsafe { libc::getifaddrs(&mut head) } != 0 {
        return Err(PlatformError::Io(std::io::Error::last_os_error()));
    }
    let mut found = vec![];
    let mut cursor = head;
    while !cursor.is_null() {
        // SAFETY: a node of the list getifaddrs wrote, alive until freeifaddrs.
        let entry = unsafe { &*cursor };
        cursor = entry.ifa_next;
        if entry.ifa_flags & libc::IFF_UP as u32 == 0 || entry.ifa_name.is_null() {
            continue;
        }
        // SAFETY: the node's own socket address.
        let Some(address) = (unsafe { read_address(entry.ifa_addr) }) else { continue };
        // SAFETY: the node's NUL-terminated name.
        let name = unsafe { CStr::from_ptr(entry.ifa_name) }.to_string_lossy().into_owned();
        found.push((name, address));
    }
    // SAFETY: the list getifaddrs allocated, freed once.
    unsafe { libc::freeifaddrs(head) };
    Ok(found)
}

/// The read grouped by interface, in the order each name first appeared.
fn group(read: Vec<(String, IpAddr)>) -> Adapters {
    let mut order: Vec<String> = vec![];
    let mut by_name: BTreeMap<String, Vec<IpAddr>> = BTreeMap::new();
    for (name, address) in read {
        let addresses = by_name.entry(name.clone()).or_insert_with(|| {
            order.push(name);
            vec![]
        });
        if !addresses.contains(&address) {
            addresses.push(address);
        }
    }
    let adapters = order.into_iter().map(|name| Adapter { addresses: by_name.remove(&name).unwrap_or_default(), name }).collect();
    Adapters { adapters, unreadable: 0 }
}

/// The interface that carries `address` (Swift `TailscaleAddresses.interface(owning:)` over every interface), None for loopback.
fn owner(address: IpAddr, read: &[(String, IpAddr)]) -> Option<String> {
    if address.is_loopback() || address.is_unspecified() {
        return None;
    }
    read.iter().find(|(_, own)| *own == address).map(|(name, _)| name.clone())
}

fn last_error() -> PlatformError {
    PlatformError::Io(std::io::Error::last_os_error())
}

fn set_option(fd: &OwnedFd, level: i32, name: i32, value: i32) -> Result<()> {
    use std::os::fd::AsRawFd as _;
    // SAFETY: a live socket and a 4-byte option value.
    let set = unsafe { libc::setsockopt(fd.as_raw_fd(), level, name, (&value as *const i32).cast(), size_of::<i32>() as libc::socklen_t) };
    if set != 0 { Err(last_error()) } else { Ok(()) }
}

/// The socket address in the OS's layout.
fn raw_address(address: SocketAddr) -> (libc::sockaddr_storage, libc::socklen_t) {
    // SAFETY: an all-zero sockaddr_storage is a valid value.
    let mut storage: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
    let length = match address {
        SocketAddr::V4(v4) => {
            // SAFETY: sockaddr_storage is large and aligned enough for a sockaddr_in.
            let raw = unsafe { &mut *(&mut storage as *mut libc::sockaddr_storage as *mut libc::sockaddr_in) };
            raw.sin_len = size_of::<libc::sockaddr_in>() as u8;
            raw.sin_family = libc::AF_INET as libc::sa_family_t;
            raw.sin_port = v4.port().to_be();
            raw.sin_addr = libc::in_addr { s_addr: u32::from(*v4.ip()).to_be() };
            size_of::<libc::sockaddr_in>()
        }
        SocketAddr::V6(v6) => {
            // SAFETY: sockaddr_storage is large and aligned enough for a sockaddr_in6.
            let raw = unsafe { &mut *(&mut storage as *mut libc::sockaddr_storage as *mut libc::sockaddr_in6) };
            raw.sin6_len = size_of::<libc::sockaddr_in6>() as u8;
            raw.sin6_family = libc::AF_INET6 as libc::sa_family_t;
            raw.sin6_port = v6.port().to_be();
            raw.sin6_addr = libc::in6_addr { s6_addr: v6.ip().octets() };
            raw.sin6_scope_id = v6.scope_id();
            raw.sin6_flowinfo = v6.flowinfo();
            size_of::<libc::sockaddr_in6>()
        }
    };
    (storage, length as libc::socklen_t)
}

/// A socket bound and listening on exactly `address`, pinned to `interface` when one is named.
fn listen_on(address: SocketAddr, backlog: u32, interface: Option<&str>) -> Result<TcpListener> {
    use std::os::fd::AsRawFd as _;
    let family = if address.is_ipv6() { libc::AF_INET6 } else { libc::AF_INET };
    // SAFETY: plain socket creation; the descriptor is owned below.
    let raw = unsafe { libc::socket(family, libc::SOCK_STREAM, libc::IPPROTO_TCP) };
    if raw < 0 {
        return Err(last_error());
    }
    // SAFETY: a descriptor this call just made, owned (and closed) from here on.
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };
    // SAFETY: a live descriptor; the Mac has no SOCK_CLOEXEC, so close-on-exec is set at once (no child spawned meanwhile keeps it).
    if unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } != 0 {
        return Err(last_error());
    }
    if address.is_ipv6() {
        set_option(&fd, libc::IPPROTO_IPV6, libc::IPV6_V6ONLY, 1)?;
    }
    if let Some(name) = interface {
        let name = std::ffi::CString::new(name).map_err(|_| PlatformError::Invalid("an interface name holds a NUL".into()))?;
        // SAFETY: a NUL-terminated name.
        let index = unsafe { libc::if_nametoindex(name.as_ptr()) };
        if index == 0 {
            return Err(PlatformError::Io(std::io::ErrorKind::AddrNotAvailable.into()));
        }
        let (level, option) =
            if address.is_ipv6() { (libc::IPPROTO_IPV6, libc::IPV6_BOUND_IF) } else { (libc::IPPROTO_IP, libc::IP_BOUND_IF) };
        set_option(&fd, level, option, index as i32)?;
    }
    let (storage, length) = raw_address(address);
    // SAFETY: the address structure and its length as built above.
    if unsafe { libc::bind(fd.as_raw_fd(), (&storage as *const libc::sockaddr_storage).cast(), length) } != 0 {
        return Err(last_error());
    }
    // SAFETY: a bound socket.
    if unsafe { libc::listen(fd.as_raw_fd(), i32::try_from(backlog).unwrap_or(i32::MAX)) } != 0 {
        return Err(last_error());
    }
    Ok(TcpListener::from(fd))
}

/// `RTM_NEWADDR`, `RTM_DELADDR` and `RTM_IFINFO`: the routing messages that change which addresses are up. A message's type is its
/// fourth byte (`rtm_msglen` u16, `rtm_version` u8, `rtm_type` u8, the same in every routing message header).
fn changes_addresses(message: &[u8]) -> bool {
    message.len() >= 4 && [libc::RTM_NEWADDR, libc::RTM_DELADDR, libc::RTM_IFINFO].contains(&i32::from(message[3]))
}

/// The routing socket's watcher: its thread, and the pipe that ends it.
struct Watcher {
    stop: OwnedFd,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Watcher {
    fn start(callback: NetworkCallback) -> Result<Watcher> {
        use std::os::fd::AsRawFd as _;
        // SAFETY: plain socket creation; owned below.
        let raw = unsafe { libc::socket(libc::AF_ROUTE, libc::SOCK_RAW, libc::AF_UNSPEC) };
        if raw < 0 {
            return Err(last_error());
        }
        // SAFETY: the descriptor just made.
        let route = unsafe { OwnedFd::from_raw_fd(raw) };
        let mut ends = [0i32; 2];
        // SAFETY: two descriptors are written into `ends`.
        if unsafe { libc::pipe(ends.as_mut_ptr()) } != 0 {
            return Err(last_error());
        }
        // SAFETY: the two ends just made, owned from here on.
        let (wake, stop) = unsafe { (OwnedFd::from_raw_fd(ends[0]), OwnedFd::from_raw_fd(ends[1])) };
        for fd in [&route, &wake, &stop] {
            // SAFETY: live descriptors.
            unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) };
        }
        let thread = std::thread::Builder::new().name("dev.mightycode.network".into()).spawn(move || {
            let mut buffer = [0u8; 2048];
            loop {
                let mut polled = [
                    libc::pollfd { fd: route.as_raw_fd(), events: libc::POLLIN, revents: 0 },
                    libc::pollfd { fd: wake.as_raw_fd(), events: libc::POLLIN, revents: 0 },
                ];
                // SAFETY: two pollfd entries.
                let ready = unsafe { libc::poll(polled.as_mut_ptr(), 2, -1) };
                if ready < 0 {
                    if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                        continue;
                    }
                    return;
                }
                if polled[1].revents != 0 {
                    return;
                }
                if polled[0].revents & libc::POLLIN == 0 {
                    if polled[0].revents != 0 {
                        return;
                    }
                    continue;
                }
                // SAFETY: `buffer` holds its length in writable bytes; one routing message per read.
                let read = unsafe { libc::read(route.as_raw_fd(), buffer.as_mut_ptr().cast(), buffer.len()) };
                if read < 0 {
                    let error = std::io::Error::last_os_error();
                    // A burst the socket could not hold (ENOBUFS) still means the addresses changed.
                    if error.raw_os_error() == Some(libc::ENOBUFS) {
                        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| callback.call()));
                        continue;
                    }
                    if error.kind() == std::io::ErrorKind::Interrupted {
                        continue;
                    }
                    return;
                }
                if changes_addresses(&buffer[..read as usize]) {
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| callback.call()));
                }
            }
        })?;
        Ok(Watcher { stop, thread: Some(thread) })
    }

    /// Ends the thread and waits for it (unless this runs on it: a callback that drops its own subscription), so no call follows.
    fn end(mut self) {
        use std::os::fd::AsRawFd as _;
        // SAFETY: one byte from a live buffer into the pipe's write end.
        unsafe { libc::write(self.stop.as_raw_fd(), [1u8].as_ptr().cast(), 1) };
        if let Some(thread) = self.thread.take()
            && thread.thread().id() != std::thread::current().id()
        {
            let _ = thread.join();
        }
    }
}

/// A wake observer's callback, taken out under its lock when the subscription ends (waiting out a call in flight on another thread),
/// so none follows. A callback that ends its own subscription (on the thread running it) does not wait for itself: the cell is marked
/// ended and the call in flight takes the callback out as it returns.
struct ResumeCell {
    callback: Mutex<Option<NetworkCallback>>,
    ended: std::sync::atomic::AtomicBool,
}

thread_local! {
    /// The cell whose callback this thread is running now (its address), 0 for none.
    static CALLING: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

impl ResumeCell {
    fn new(callback: NetworkCallback) -> ResumeCell {
        ResumeCell { callback: Mutex::new(Some(callback)), ended: std::sync::atomic::AtomicBool::new(false) }
    }

    fn address(&self) -> usize {
        self as *const ResumeCell as usize
    }

    fn call(&self) {
        use std::sync::atomic::Ordering;
        if self.ended.load(Ordering::SeqCst) {
            return;
        }
        let mut held = self.callback.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.ended.load(Ordering::SeqCst) {
            held.take();
            return;
        }
        if let Some(callback) = held.as_ref() {
            let outer = CALLING.with(|calling| calling.replace(self.address()));
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| callback.call()));
            CALLING.with(|calling| calling.set(outer));
        }
        if self.ended.load(Ordering::SeqCst) {
            held.take();
        }
    }

    /// Ends the cell: no call starts after this returns. From the callback itself, the callback is dropped as that call returns.
    fn end(&self) -> Option<NetworkCallback> {
        self.ended.store(true, std::sync::atomic::Ordering::SeqCst);
        if CALLING.with(|calling| calling.get()) == self.address() {
            return None;
        }
        self.callback.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take()
    }
}

/// The observer the notification center gave back, removed from any thread (NSNotificationCenter is thread-safe).
struct Observer(objc2::rc::Retained<objc2::runtime::ProtocolObject<dyn objc2_foundation::NSObjectProtocol>>);

// SAFETY: the observer token is only handed back to `removeObserver:`, which NSNotificationCenter allows from any thread.
unsafe impl Send for Observer {}

impl Observer {
    /// Removes the observer from the workspace's notification center: no call starts after this returns.
    fn remove(self) {
        let center = objc2_app_kit::NSWorkspace::sharedWorkspace().notificationCenter();
        // SAFETY: the token addObserverForName gave back.
        unsafe { center.removeObserver(AsRef::<objc2::runtime::AnyObject>::as_ref(&*self.0)) };
    }
}

/// Observes the workspace's wake notification with `cell`; the subscription removes the observer and empties the cell.
fn observe_wake(cell: Arc<ResumeCell>) -> Subscription {
    use objc2_app_kit::{NSWorkspace, NSWorkspaceDidWakeNotification};
    use objc2_foundation::NSNotification;
    let called = cell.clone();
    let block = block2::RcBlock::new(move |_note: NonNull<NSNotification>| called.call());
    let center = NSWorkspace::sharedWorkspace().notificationCenter();
    // SAFETY: the name is AppKit's own constant; no object filter; no queue (the poster's thread); the block only calls the cell,
    // which is Send + Sync.
    let observer =
        Observer(unsafe { center.addObserverForName_object_queue_usingBlock(Some(NSWorkspaceDidWakeNotification), None, None, &block) });
    Subscription::new(move || {
        observer.remove();
        drop(cell.end());
    })
}

impl Network for MacNetwork {
    fn adapters(&self) -> Result<Adapters> {
        Ok(group(interface_addresses()?))
    }

    fn bind_exclusive(&self, address: SocketAddr, backlog: u32) -> Result<TcpListener> {
        let interface = owner(address.ip(), &interface_addresses().unwrap_or_default());
        listen_on(address, backlog, interface.as_deref())
    }

    fn on_change(&self, callback: NetworkCallback) -> Result<Subscription> {
        let watcher = Watcher::start(callback)?;
        Ok(Subscription::new(move || watcher.end()))
    }

    fn on_resume(&self, callback: NetworkCallback) -> Result<Subscription> {
        Ok(observe_wake(Arc::new(ResumeCell::new(callback))))
    }
}

/// The Mac's `Network`.
pub fn network() -> Arc<dyn Network> {
    Arc::new(MacNetwork)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    /// The wake tests post `NSWorkspaceDidWakeNotification` on the workspace's shared centre, which every wake observer of the test
    /// binary hears; run side by side, one test's post would call the other's observer, so they take turns.
    static WAKE_POSTS: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// The adapters read here are this Mac's up interfaces: loopback is among them (`lo0` with 127.0.0.1), each name once, each
    /// address once per name.
    #[test]
    fn the_adapters_are_the_up_interfaces_and_their_addresses() {
        let read = MacNetwork.adapters().unwrap();
        assert_eq!(read.unreadable, 0);
        assert!(
            read.adapters.iter().any(|a| a.name == "lo0" && a.addresses.contains(&IpAddr::V4(Ipv4Addr::LOCALHOST))),
            "loopback: {read:?}"
        );
        let mut names: Vec<&str> = read.adapters.iter().map(|a| a.name.as_str()).collect();
        let count = names.len();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), count, "one entry per interface");
        for adapter in &read.adapters {
            let mut addresses = adapter.addresses.clone();
            addresses.sort();
            addresses.dedup();
            assert_eq!(addresses.len(), adapter.addresses.len(), "{adapter:?}");
        }
    }

    /// Grouping keeps the order of the first appearance and drops a repeated address; the owner of an address is the interface it was
    /// read on, never loopback's.
    #[test]
    fn the_read_is_grouped_by_interface_and_an_address_names_its_owner() {
        let ip = |text: &str| text.parse::<IpAddr>().unwrap();
        let read = vec![
            ("en0".to_string(), ip("192.168.0.5")),
            ("utun4".to_string(), ip("fd7a:115c:a1e0::1")),
            ("en0".to_string(), ip("fe80::1")),
            ("utun4".to_string(), ip("100.101.102.103")),
            ("utun4".to_string(), ip("100.101.102.103")),
            ("lo0".to_string(), ip("127.0.0.1")),
        ];
        let grouped = group(read.clone());
        assert_eq!(
            grouped.adapters,
            vec![
                Adapter { name: "en0".into(), addresses: vec![ip("192.168.0.5"), ip("fe80::1")] },
                Adapter { name: "utun4".into(), addresses: vec![ip("fd7a:115c:a1e0::1"), ip("100.101.102.103")] },
                Adapter { name: "lo0".into(), addresses: vec![ip("127.0.0.1")] },
            ]
        );
        assert_eq!(owner(ip("100.101.102.103"), &read).as_deref(), Some("utun4"));
        assert_eq!(owner(ip("fd7a:115c:a1e0::1"), &read).as_deref(), Some("utun4"));
        assert_eq!(owner(ip("127.0.0.1"), &read), None, "loopback is never pinned");
        assert_eq!(owner(ip("10.9.9.9"), &read), None, "an address no interface carries");
    }

    /// The listener is exclusive (a second bind of the same address and port is refused as `AddrInUse`), accepts, takes IPv6 only on
    /// an IPv6 address, and is pinned when its interface is named.
    #[test]
    fn a_listener_is_exclusive_and_takes_connections() {
        let first = MacNetwork.bind_exclusive("127.0.0.1:0".parse().unwrap(), 4).unwrap();
        let address = first.local_addr().unwrap();
        let second = MacNetwork.bind_exclusive(address, 4).unwrap_err();
        assert!(matches!(&second, PlatformError::Io(e) if e.kind() == std::io::ErrorKind::AddrInUse), "{second:?}");
        let client = std::net::TcpStream::connect(address).unwrap();
        let (accepted, peer) = first.accept().unwrap();
        assert_eq!(peer, client.local_addr().unwrap());
        drop(accepted);
        if let Ok(v6) = MacNetwork.bind_exclusive("[::1]:0".parse().unwrap(), 4) {
            let port = v6.local_addr().unwrap().port();
            assert!(std::net::TcpStream::connect(("::1", port)).is_ok());
            assert!(std::net::TcpStream::connect(("127.0.0.1", port)).is_err(), "IPv6 only: no IPv4 on the same socket");
        }
        let pinned = listen_on("127.0.0.1:0".parse().unwrap(), 4, Some("lo0")).unwrap();
        assert!(std::net::TcpStream::connect(pinned.local_addr().unwrap()).is_ok(), "pinned to loopback's interface, still reached");
        assert!(listen_on("127.0.0.1:0".parse().unwrap(), 4, Some("utun-none")).is_err(), "an interface that is not there");
    }

    /// The routing messages that change the addresses are told apart from the rest.
    #[test]
    fn address_changes_are_the_routing_messages_that_call_back() {
        let message = |kind: i32| vec![0u8, 0, 5, kind as u8, 0, 0];
        assert!(changes_addresses(&message(libc::RTM_NEWADDR)) && changes_addresses(&message(libc::RTM_DELADDR)));
        assert!(changes_addresses(&message(libc::RTM_IFINFO)));
        // RTM_ADD (1) and RTM_GET (4): routes, not addresses.
        assert!(!changes_addresses(&message(0x1)) && !changes_addresses(&message(0x4)) && !changes_addresses(&[0, 0]));
    }

    /// A change subscription runs its watcher until dropped, and its drop returns only once the watcher has ended.
    #[test]
    fn a_change_subscription_ends_when_dropped() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = calls.clone();
        let subscription = MacNetwork
            .on_change(NetworkCallback::new(move || {
                counted.fetch_add(1, Ordering::SeqCst);
            }))
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        drop(subscription);
        let after = calls.load(Ordering::SeqCst);
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert_eq!(calls.load(Ordering::SeqCst), after, "no call once the drop returned");
    }

    /// A wake callback that ends its own subscription returns (no deadlock on its own cell), and no call follows.
    #[test]
    fn a_wake_callback_may_end_its_own_subscription() {
        let _turn = WAKE_POSTS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        use objc2_app_kit::{NSWorkspace, NSWorkspaceDidWakeNotification};
        let held: Arc<Mutex<Option<Subscription>>> = Arc::new(Mutex::new(None));
        let calls = Arc::new(AtomicUsize::new(0));
        let (inner, counted) = (held.clone(), calls.clone());
        let subscription = MacNetwork
            .on_resume(NetworkCallback::new(move || {
                counted.fetch_add(1, Ordering::SeqCst);
                let taken = inner.lock().unwrap().take();
                drop(taken);
            }))
            .unwrap();
        *held.lock().unwrap() = Some(subscription);
        let post = || {
            let center = NSWorkspace::sharedWorkspace().notificationCenter();
            // SAFETY: AppKit's own notification name, no object.
            unsafe { center.postNotificationName_object(NSWorkspaceDidWakeNotification, None) };
        };
        post();
        post();
        assert_eq!(calls.load(Ordering::SeqCst), 1, "it returned and ended itself");
        assert!(held.lock().unwrap().is_none());
    }

    /// The wake observer calls back when the workspace posts `NSWorkspaceDidWakeNotification` (posted here by the test, on its own
    /// thread, as AppKit posts it on the main thread after a sleep), and never once its subscription is dropped.
    #[test]
    fn a_wake_calls_back_until_the_subscription_ends() {
        let _turn = WAKE_POSTS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        use objc2_app_kit::{NSWorkspace, NSWorkspaceDidWakeNotification};
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = calls.clone();
        let subscription = MacNetwork
            .on_resume(NetworkCallback::new(move || {
                counted.fetch_add(1, Ordering::SeqCst);
            }))
            .unwrap();
        let post = || {
            let center = NSWorkspace::sharedWorkspace().notificationCenter();
            // SAFETY: AppKit's own notification name, no object.
            unsafe { center.postNotificationName_object(NSWorkspaceDidWakeNotification, None) };
        };
        post();
        assert_eq!(calls.load(Ordering::SeqCst), 1, "a wake calls back");
        drop(subscription);
        post();
        assert_eq!(calls.load(Ordering::SeqCst), 1, "no call once the subscription is dropped");
    }
}
