//! The network port on Windows (plan §0.2, §1.4; C# `NetworkInterface`, `Socket.ExclusiveAddressUse`, `NetworkChange`,
//! `SystemEvents.PowerModeChanged`): the up adapters and their unicast addresses (`GetAdaptersAddresses`, one adapter's failure kept
//! apart), an exclusive listener (`SO_EXCLUSIVEADDRUSE` through socket2's raw setsockopt, then bind and listen with the backlog),
//! address changes (`NotifyIpInterfaceChange`) and resumes (`PowerRegisterSuspendResumeNotification`). Owner: W8-C.
//!
//! At rest (W8 Step 0): every call answers `Unsupported`.

use mighty_platform::{Adapters, Network, NetworkCallback, PlatformError, Result, Subscription};

#[derive(Debug, Default)]
pub struct WindowsNetwork;

impl Network for WindowsNetwork {
    fn adapters(&self) -> Result<Adapters> {
        Err(PlatformError::unsupported("Network", "adapters"))
    }
    fn bind_exclusive(&self, _address: std::net::SocketAddr, _backlog: u32) -> Result<std::net::TcpListener> {
        Err(PlatformError::unsupported("Network", "bind_exclusive"))
    }
    fn on_change(&self, _callback: NetworkCallback) -> Result<Subscription> {
        Err(PlatformError::unsupported("Network", "on_change"))
    }
    fn on_resume(&self, _callback: NetworkCallback) -> Result<Subscription> {
        Err(PlatformError::unsupported("Network", "on_resume"))
    }
}
