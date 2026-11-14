//! The system proxy on Windows (plan §0.2, D5; .NET's `HttpClient.DefaultProxy` on Windows): the current user's settings
//! (`WinHttpGetIEProxyConfigForCurrentUser`: a manual proxy and its bypass list), then automatic detection and a PAC script
//! (`WinHttpGetProxyForUrl`), for the relay's WebSocket and push. Owner: W8-A.
//!
//! At rest (W8 Step 0): `resolve` answers `Unsupported`.

use mighty_platform::{BoxFuture, PlatformError, Proxy, ProxyResolver, Result};

#[derive(Debug, Default)]
pub struct WinHttpProxy;

impl ProxyResolver for WinHttpProxy {
    fn resolve(&self, _url: &str) -> BoxFuture<'_, Result<Proxy>> {
        Box::pin(async { Err(PlatformError::unsupported("ProxyResolver", "resolve")) })
    }
}
