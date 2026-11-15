//! M-C: the network for the direct path and remote sharing. Swift `RelayLink`'s `NWPathMonitor` and wake handling, `RemoteWorkspace`'s
//! interface selection: `getifaddrs`, a `utun*` carrying Tailscale's IPv6 prefix, `IP_BOUND_IF`/`IPV6_BOUND_IF` pinning, address
//! changes and wake events. Contract `mighty_platform::net` (`Network`).

use std::sync::Arc;

use mighty_platform::Network;
use mighty_platform::unsupported::Unsupported;

/// The Mac's `Network` (Unsupported until M-C).
pub fn network() -> Arc<dyn Network> {
    Arc::new(Unsupported)
}
