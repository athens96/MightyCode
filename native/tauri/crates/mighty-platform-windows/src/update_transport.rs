//! The app update's HTTPS on Windows (W9-B; plan §W9-B, D7): a WinHTTP session with `WINHTTP_ACCESS_TYPE_NO_PROXY`, redirects off
//! (`WINHTTP_OPTION_DISABLE_FEATURE` with `WINHTTP_DISABLE_REDIRECTS`), TLS from the system, `http://` refused before connecting;
//! `get_capped` and a streamed `download` writing to its file with progress and cancel, each read checked against the caps. Not
//! [`crate::http::WinHttpGet`] (the system proxy, string bodies).
//!
//! Step 0: both operations answer `Unsupported`.

use std::time::Duration;

use mighty_platform::{BoxFuture, DownloadRequest, Downloaded, PlatformError, Result, UpdateResponse, UpdateTransport};

/// The Windows update transport (W9-B fills it).
#[derive(Debug, Clone, Copy, Default)]
pub struct WinHttpUpdates;

impl UpdateTransport for WinHttpUpdates {
    fn get_capped(&self, _url: &str, _cap: usize, _timeout: Duration) -> BoxFuture<'_, Result<UpdateResponse>> {
        Box::pin(async { Err(PlatformError::unsupported("UpdateTransport", "get_capped")) })
    }
    fn download(&self, _request: DownloadRequest) -> BoxFuture<'_, Result<Downloaded>> {
        Box::pin(async { Err(PlatformError::unsupported("UpdateTransport", "download")) })
    }
}
