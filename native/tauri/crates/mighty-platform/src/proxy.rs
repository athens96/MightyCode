//! The system proxy for a URL (W8, `.omc/research/tauri/w8-plan.md` §0.2, D5): the relay's WebSocket and push go through the proxy the
//! system names, as .NET's `ClientWebSocket` and `HttpClient` do (a `CONNECT` tunnel before TLS); remote workspaces v2 use none
//! (`UseProxy = false`, R\RemoteTransport.cs:71). Owner: W8-A (the Windows implementation: `WinHttpGetIEProxyConfigForCurrentUser`,
//! then `WinHttpGetProxyForUrl` for automatic configuration and PAC; the tunnel itself is mighty-remote's).

use crate::{BoxFuture, Result};

/// Where a connection to a URL goes first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Proxy {
    /// Straight to the URL's host (no proxy configured, or the URL is on the bypass list).
    Direct,
    /// Through an HTTP proxy: a `CONNECT host:port` tunnel, then TLS inside it.
    Http { host: String, port: u16 },
}

/// Contract: `resolve` answers the proxy the current user's system settings name for `url` (a manual proxy, the bypass list, automatic
/// detection and a PAC script); it may take seconds (WPAD), never prompts and never reaches the URL itself. A PAC the OS cannot run
/// fails as .NET's resolution would. `url` is never logged nor put into an error: the relay's carries the host token in its query.
pub trait ProxyResolver: Send + Sync {
    fn resolve(&self, url: &str) -> BoxFuture<'_, Result<Proxy>>;
}
