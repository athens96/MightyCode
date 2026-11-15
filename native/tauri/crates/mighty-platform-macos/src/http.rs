//! M-C: HTTPS GET and POST (the push's POST, the account usage's GET) over hyper and native-tls through the system proxy, as
//! mighty-remote's relay already does (plan D17; Swift's URLSession posts). Contract `mighty_platform::http` (`HttpsSend`).

use std::sync::Arc;

use mighty_platform::HttpsSend;
use mighty_platform::unsupported::Unsupported;

/// The Platform's HTTPS (Unsupported until M-C).
pub fn https() -> Arc<dyn HttpsSend> {
    Arc::new(Unsupported)
}

/// The account usage's guarded GET (src-tauri platform.rs `Ports`; Unsupported until M-C).
pub fn https_get() -> Arc<dyn HttpsSend> {
    Arc::new(Unsupported)
}
