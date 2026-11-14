//! The network port on Windows (plan §0.2, §1.4; C# `NetworkInterface`, `Socket.ExclusiveAddressUse`, `NetworkChange`,
//! `SystemEvents.PowerModeChanged`): the up adapters and their unicast addresses (`GetAdaptersAddresses`, an address that does not
//! read as IPv4 or IPv6 skipped), an exclusive listener (`SO_EXCLUSIVEADDRUSE` through socket2's raw socket, then bind and listen with
//! the backlog), address changes (`NotifyUnicastIpAddressChange`, what .NET's `NetworkAddressChanged` follows) and resumes
//! (`PowerRegisterSuspendResumeNotification`). Owner: W8-C.
//!
//! Nothing here listens by itself: the direct path binds only Tailscale's addresses and only when the app may listen
//! (`Host::may_listen`); the checks bind loopback through the fake network.

use std::ffi::c_void;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener};

use mighty_platform::{Adapter, Adapters, Network, NetworkCallback, PlatformError, Result, Subscription};
use socket2::{Domain, Protocol, Socket, Type};
use windows_sys::Win32::Foundation::{ERROR_BUFFER_OVERFLOW, ERROR_SUCCESS, HANDLE};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    CancelMibChangeNotify2, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST, GetAdaptersAddresses,
    IP_ADAPTER_ADDRESSES_LH, MIB_NOTIFICATION_TYPE, MIB_UNICASTIPADDRESS_ROW, NotifyUnicastIpAddressChange,
};
use windows_sys::Win32::NetworkManagement::Ndis::IfOperStatusUp;
use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6, AF_UNSPEC, SO_EXCLUSIVEADDRUSE, SOCKET, SOL_SOCKET, setsockopt};
use windows_sys::Win32::System::Power::{
    DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS, PowerRegisterSuspendResumeNotification, PowerUnregisterSuspendResumeNotification,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{DEVICE_NOTIFY_CALLBACK, PBT_APMRESUMEAUTOMATIC, PBT_APMRESUMESUSPEND};

#[derive(Debug, Default)]
pub struct WindowsNetwork;

/// The address a `SOCKET_ADDRESS` holds, when it is IPv4 or IPv6.
///
/// SAFETY: `address` points at `length` readable bytes of a socket address the OS wrote.
unsafe fn read_address(address: *const u8, length: i32) -> Option<IpAddr> {
    if address.is_null() || length < 2 {
        return None;
    }
    // SAFETY: the caller's contract; at least two bytes (the family) are there.
    let bytes = unsafe { std::slice::from_raw_parts(address, length as usize) };
    let family = u16::from_ne_bytes([bytes[0], bytes[1]]);
    if family == AF_INET && bytes.len() >= 8 {
        return Some(IpAddr::V4(Ipv4Addr::new(bytes[4], bytes[5], bytes[6], bytes[7])));
    }
    if family == AF_INET6 && bytes.len() >= 24 {
        let mut octets = [0u8; 16];
        octets.copy_from_slice(&bytes[8..24]);
        return Some(IpAddr::V6(Ipv6Addr::from(octets)));
    }
    None
}

/// UTF-16 up to its NUL.
///
/// SAFETY: `text` is null or a NUL-terminated UTF-16 string.
unsafe fn read_wide(text: *const u16) -> String {
    if text.is_null() {
        return String::new();
    }
    let mut length = 0;
    // SAFETY: NUL-terminated per the caller's contract.
    while unsafe { *text.add(length) } != 0 {
        length += 1;
    }
    // SAFETY: `length` units were just read.
    String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, length) })
}

fn read_adapters() -> Result<Adapters> {
    let flags = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
    // 15 KB first, as the documentation suggests; a list that grew meanwhile is asked for again with the size the OS named.
    let mut size: u32 = 15 * 1024;
    let mut buffer: Vec<u64>;
    let mut tries = 0;
    loop {
        buffer = vec![0u64; (size as usize).div_ceil(8)];
        // SAFETY: the buffer holds `size` bytes, 8-aligned as the structure needs.
        let result = unsafe { GetAdaptersAddresses(AF_UNSPEC as u32, flags, std::ptr::null(), buffer.as_mut_ptr().cast(), &mut size) };
        if result == ERROR_SUCCESS {
            break;
        }
        tries += 1;
        if result != ERROR_BUFFER_OVERFLOW || tries >= 4 {
            return Err(PlatformError::Io(std::io::Error::from_raw_os_error(result as i32)));
        }
    }
    let mut adapters = Adapters::default();
    let mut next = buffer.as_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
    while !next.is_null() {
        // SAFETY: a node of the list the OS wrote into `buffer`, which outlives this loop.
        let adapter = unsafe { &*next };
        next = adapter.Next;
        if adapter.OperStatus != IfOperStatusUp {
            continue;
        }
        // SAFETY: the OS's NUL-terminated friendly name.
        let name = unsafe { read_wide(adapter.FriendlyName) };
        let mut addresses = vec![];
        let mut unicast = adapter.FirstUnicastAddress;
        while !unicast.is_null() {
            // SAFETY: a node of the adapter's unicast list in `buffer`.
            let entry = unsafe { &*unicast };
            unicast = entry.Next;
            // SAFETY: the OS's socket address with its length.
            if let Some(address) = unsafe { read_address(entry.Address.lpSockaddr.cast(), entry.Address.iSockaddrLength) } {
                addresses.push(address);
            }
        }
        adapters.adapters.push(Adapter { name, addresses });
    }
    Ok(adapters)
}

/// What a callback's context holds: the app's callback, boxed so its address is stable until the registration ends.
struct Registered(NetworkCallback);

unsafe extern "system" fn address_changed(context: *const c_void, _row: *const MIB_UNICASTIPADDRESS_ROW, _kind: MIB_NOTIFICATION_TYPE) {
    if context.is_null() {
        return;
    }
    // SAFETY: the context is the `Registered` box, alive until `CancelMibChangeNotify2` (which waits for running callbacks) returned.
    let registered = unsafe { &*(context as *const Registered) };
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| registered.0.call()));
}

unsafe extern "system" fn power_changed(context: *const c_void, kind: u32, _setting: *const c_void) -> u32 {
    if !context.is_null() && (kind == PBT_APMRESUMEAUTOMATIC || kind == PBT_APMRESUMESUSPEND) {
        // SAFETY: the context is the `Registered` box, alive until the registration is ended.
        let registered = unsafe { &*(context as *const Registered) };
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| registered.0.call()));
    }
    ERROR_SUCCESS
}

/// A handle that crosses to the subscription's end (raw handles are not `Send`).
struct Handle(usize);

impl Network for WindowsNetwork {
    fn adapters(&self) -> Result<Adapters> {
        read_adapters()
    }

    /// `SO_EXCLUSIVEADDRUSE` before the bind (no other socket may share the address and port), IPv6 only on an IPv6 address, then
    /// listen with `backlog`. `WSAEADDRINUSE` reads as `AddrInUse` and `WSAEACCES` as `PermissionDenied` (std's mapping).
    fn bind_exclusive(&self, address: SocketAddr, backlog: u32) -> Result<TcpListener> {
        let socket = Socket::new(Domain::for_address(address), Type::STREAM, Some(Protocol::TCP))?;
        let on: i32 = 1;
        use std::os::windows::io::AsRawSocket;
        // SAFETY: a live socket and a 4-byte option value.
        let set = unsafe {
            setsockopt(
                socket.as_raw_socket() as SOCKET,
                SOL_SOCKET,
                SO_EXCLUSIVEADDRUSE,
                (&on as *const i32).cast(),
                std::mem::size_of::<i32>() as i32,
            )
        };
        if set != 0 {
            return Err(PlatformError::Io(std::io::Error::last_os_error()));
        }
        if address.is_ipv6() {
            socket.set_only_v6(true)?;
        }
        socket.bind(&address.into())?;
        socket.listen(i32::try_from(backlog).unwrap_or(i32::MAX))?;
        Ok(socket.into())
    }

    fn on_change(&self, callback: NetworkCallback) -> Result<Subscription> {
        let context = Box::into_raw(Box::new(Registered(callback)));
        let mut handle: HANDLE = std::ptr::null_mut();
        // SAFETY: the callback is a plain function; the context lives until the registration ends below.
        let result =
            unsafe { NotifyUnicastIpAddressChange(AF_UNSPEC, Some(address_changed), context as *const c_void, false, &mut handle) };
        if result != ERROR_SUCCESS {
            // SAFETY: never registered: the box is still ours.
            drop(unsafe { Box::from_raw(context) });
            return Err(PlatformError::Io(std::io::Error::from_raw_os_error(result as i32)));
        }
        let (handle, context) = (Handle(handle as usize), Handle(context as usize));
        Ok(Subscription::new(move || {
            // SAFETY: the handle registered above, cancelled once; the cancel waits for a running callback, so the box is free after.
            unsafe {
                CancelMibChangeNotify2(handle.0 as HANDLE);
                drop(Box::from_raw(context.0 as *mut Registered));
            }
        }))
    }

    fn on_resume(&self, callback: NetworkCallback) -> Result<Subscription> {
        let context = Box::into_raw(Box::new(Registered(callback)));
        let mut parameters = DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS { Callback: Some(power_changed), Context: context.cast() };
        let mut registration: *mut c_void = std::ptr::null_mut();
        // SAFETY: the parameters live across the call (the OS copies them); the context lives until the registration ends below.
        let result = unsafe {
            PowerRegisterSuspendResumeNotification(
                DEVICE_NOTIFY_CALLBACK,
                (&mut parameters as *mut DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS).cast(),
                &mut registration,
            )
        };
        if result != ERROR_SUCCESS {
            // SAFETY: never registered: the box is still ours.
            drop(unsafe { Box::from_raw(context) });
            return Err(PlatformError::Io(std::io::Error::from_raw_os_error(result as i32)));
        }
        let registration = Handle(registration as usize);
        Ok(Subscription::new(move || {
            // SAFETY: the registration made above, ended once. The OS does not say that a callback already running has returned when
            // it does, so the context (a few bytes, one per subscription) is left alive rather than freed under it; the callback it
            // holds is never called again (the app's own end of it is dropped with the subscription's owner).
            unsafe {
                PowerUnregisterSuspendResumeNotification(registration.0 as isize);
            }
        }))
    }
}
