//! M-C: the system proxy for a URL (`CFNetworkCopySystemProxies`, as URLSession uses it; plan D17). Contract
//! `mighty_platform::proxy` (`ProxyResolver`).

use std::sync::Arc;

use mighty_platform::ProxyResolver;
use mighty_platform::unsupported::Unsupported;

/// The Mac's `ProxyResolver` (Unsupported until M-C).
pub fn proxy() -> Arc<dyn ProxyResolver> {
    Arc::new(Unsupported)
}
