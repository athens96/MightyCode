//! M-H: the update's HTTPS on the Mac (hyper and native-tls through the system proxy, as URLSession does; plan D17), with the shared
//! contract's rules: https only, redirects handed back (5 at most, https only), a capped manifest read, a streamed download with
//! progress and cancel, the URL never in `Debug`. Contract `mighty_platform::update_transport` (`UpdateTransport`).

use std::sync::Arc;

use mighty_platform::UpdateTransport;
use mighty_platform::unsupported::Unsupported;

/// The Mac's `UpdateTransport` (Unsupported until M-H).
pub fn update_transport() -> Arc<dyn UpdateTransport> {
    Arc::new(Unsupported)
}
