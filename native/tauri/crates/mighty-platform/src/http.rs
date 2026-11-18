//! The HTTPS port, redirects refused (C# AccountUsageRuntime.cs `RawSendAsync`, `AccountUsageHttpRequest`, `AccountUsageHttpResponse`;
//! W8: MobilePush.cs `MobilePushTransport`): the account usage's direct lookup (GET) and the phone's push (POST). Owner: W6-A (the
//! port, GET); its Windows implementation (WinHTTP) and the guard over it (https on 443 only, the token only in Authorization) are
//! W6-C's. W8 Step 0 renamed `HttpsGet` to `HttpsSend` and added `post`; its WinHTTP body is W8-A's (push).

use std::time::Duration;

use crate::{BoxFuture, Result};

/// One request (C# `AccountUsageHttpRequest`; a POST's body is passed beside it).
#[derive(Clone, PartialEq, Eq)]
pub struct HttpsRequest {
    pub url: String,
    /// Sent as given (C# `TryAddWithoutValidation`). A value here never reaches a log, an error or a snapshot (its `Debug` hides them).
    pub headers: Vec<(String, String)>,
    pub timeout: Duration,
}

impl std::fmt::Debug for HttpsRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names: Vec<&str> = self.headers.iter().map(|(name, _)| name.as_str()).collect();
        f.debug_struct("HttpsRequest").field("url", &self.url).field("headers", &names).field("timeout", &self.timeout).finish()
    }
}

/// Its answer (C# `AccountUsageHttpResponse`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HttpsResponse {
    pub status: u16,
    /// The body, read only for a 2xx; else empty.
    pub body: String,
    pub retry_after: Option<String>,
    /// A 3xx's `Location` (`refused` when it named none): never followed.
    pub redirect_location: Option<String>,
}

/// Contract: one request, no retry, no cookies, no stored credentials or automatic logon, no redirect followed, the system's proxy;
/// at most `max_body` bytes of a body (C# `MaxResponseContentBufferSize`); it fails on a transport error or the timeout, and never
/// puts a header's value (or a body) into its error.
pub trait HttpsSend: Send + Sync {
    fn get(&self, request: HttpsRequest, max_body: usize) -> BoxFuture<'_, Result<HttpsResponse>>;

    /// One POST of `body` (W8-A's push: `POST https://<relay>/push/v1/send`, C# `MobilePushTransport`), under the same contract as
    /// [`HttpsSend::get`]; the push reads the status and `Retry-After` of any answer. A transport that only GETs (the usage's guard,
    /// the checks' handlers) keeps this default, which refuses every POST without reaching a network.
    fn post(&self, request: HttpsRequest, body: Vec<u8>, max_body: usize) -> BoxFuture<'_, Result<HttpsResponse>> {
        let _ = (request, body, max_body);
        Box::pin(async { Err(crate::PlatformError::unsupported("HttpsSend", "post")) })
    }
}
