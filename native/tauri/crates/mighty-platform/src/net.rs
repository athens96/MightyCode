//! The network port (W8, `.omc/research/tauri/w8-plan.md` §0.2): the adapters that are up and their addresses, an exclusive listener,
//! and the two moments a relay link or a direct listener looks again — an address change and a resume. C# reads these through .NET
//! (`NetworkInterface.GetAllNetworkInterfaces`, `Socket.ExclusiveAddressUse`, `NetworkChange.NetworkAddressChanged`,
//! `SystemEvents.PowerModeChanged`; R\TailscaleAddresses.cs, R\MobileDirectListener.cs, R\RemoteHttpListener.cs, R\RelayLink.cs).
//! Owner: W8-C (the Windows implementation: `GetAdaptersAddresses`, `SO_EXCLUSIVEADDRUSE`, `NotifyIpInterfaceChange`,
//! `PowerRegisterSuspendResumeNotification`); the Mac's (`getifaddrs`, `NWPathMonitor`, `NSWorkspace.didWake`) is the Mac stage's.

use std::net::{IpAddr, SocketAddr, TcpListener};
use std::sync::Arc;

use crate::Result;

/// One adapter that is up, with its unicast addresses (C# a `NetworkInterface` whose `OperationalStatus` is `Up`, its
/// `GetIPProperties().UnicastAddresses`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Adapter {
    /// The OS's name for it. The Mac's Tailscale rule needs `utun…` (the fixture's `darwin` column); Windows never reads it.
    pub name: String,
    pub addresses: Vec<IpAddr>,
}

/// One read of the adapters: the readable ones, and how many could not be read (C# skips an adapter whose properties throw; when
/// none could be read the direct path's status is `failed/adaptersUnreadable`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Adapters {
    pub adapters: Vec<Adapter>,
    pub unreadable: usize,
}

/// Told on an address change or a resume. Called on an OS thread or a task the port owns: it must return quickly and never block.
#[derive(Clone)]
pub struct NetworkCallback(Arc<dyn Fn() + Send + Sync>);

impl NetworkCallback {
    pub fn new(callback: impl Fn() + Send + Sync + 'static) -> NetworkCallback {
        NetworkCallback(Arc::new(callback))
    }

    pub fn call(&self) {
        (self.0)();
    }
}

impl std::fmt::Debug for NetworkCallback {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("NetworkCallback")
    }
}

/// A registration ([`Network::on_change`], [`Network::on_resume`]) that ends when dropped: the OS's notification is unregistered and
/// the callback is never called again once the drop returned.
pub struct Subscription(Option<Box<dyn FnOnce() + Send>>);

impl Subscription {
    /// Runs `end` when dropped.
    pub fn new(end: impl FnOnce() + Send + 'static) -> Subscription {
        Subscription(Some(Box::new(end)))
    }

    /// Nothing to end (a port that never calls back).
    pub fn none() -> Subscription {
        Subscription(None)
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        if let Some(end) = self.0.take() {
            end();
        }
    }
}

impl std::fmt::Debug for Subscription {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Subscription")
    }
}

/// Contract:
/// - `adapters` reads the adapters that are up now; it may take a while (the OS walks every adapter), so a caller holding a lock or
///   on the UI thread runs it elsewhere first (C# L503: the read happens outside the admission lock). An adapter it cannot read counts
///   in `unreadable` and is otherwise skipped; it fails only when the OS refuses the whole read.
/// - `bind_exclusive` listens on exactly `address` (never a wildcard unless asked; callers never ask) with no other socket allowed on
///   the same address and port (Windows `SO_EXCLUSIVEADDRUSE`), `backlog` pending connections, and returns the listener blocking
///   (the caller turns it into a tokio listener). An address in use is `PlatformError::Io` of kind `AddrInUse`, a refusal
///   `PermissionDenied`.
/// - `on_change` calls back after any unicast address appears or goes; `on_resume` after the machine wakes from sleep. Callbacks may
///   come in bursts; the callers pace themselves (`RelayLinkPolicy`, the direct path's 15 s re-detection).
pub trait Network: Send + Sync {
    fn adapters(&self) -> Result<Adapters>;
    fn bind_exclusive(&self, address: SocketAddr, backlog: u32) -> Result<TcpListener>;
    fn on_change(&self, callback: NetworkCallback) -> Result<Subscription>;
    fn on_resume(&self, callback: NetworkCallback) -> Result<Subscription>;
}
