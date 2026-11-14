//! Sending a push (C# `MobilePushTransport`, `MobilePushRelay`, `MobilePushSendOutcome`, `MobilePushSendRequest`, R\MobilePush.cs:
//! 309-385; plan §1.3): `POST https://<relay host>/push/v1/send` (`wss`→`https`, `ws`→`http`) through mighty-platform's
//! `HttpsSend::post` with `Authorization: Bearer <hostToken>` and `Content-Type: application/json`, body `{"serverId","handle",
//! "payload"}`, 15 s, no redirect, no cookies, the system proxy; 2xx Sent, 403/404/410 Gone (only that handle cleared), 429/500/502/504
//! or a network error Retry (waits 2 s then 8 s; `Retry-After` in seconds capped at 1 h, then 30 s by the hub), anything else Failed.
//! Never a real push server in a test or the smoke. Owner: W8-A.

use std::sync::Arc;
use std::time::Duration;

use mighty_core::json::JsonOut;
use mighty_platform::{BoxFuture, HttpsRequest, HttpsSend};

/// `MobilePushHttpTransport.MaximumRetryAfter`: the longest Retry-After taken from the relay, which clamps its own to the same.
pub const MAXIMUM_RETRY_AFTER: Duration = Duration::from_secs(60 * 60);
/// The whole request's timeout (C#'s `HttpClient.Timeout`).
pub const TIMEOUT: Duration = Duration::from_secs(15);
/// The most of an answer's body read (the push reads none of it; a 2xx's is bounded all the same).
const MAXIMUM_BODY: usize = 64 * 1024;

/// `MobilePushSendStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushStatus {
    Sent,
    Gone,
    Retry,
    Failed,
}

/// C# `MobilePushSendOutcome(Status, RetryAfter, Code)`: the relay's answer. Gone: the relay no longer knows the handle, it is another
/// host's, or FCM dropped the token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PushOutcome {
    pub status: PushStatus,
    pub retry_after: Option<Duration>,
    pub code: u16,
}

impl PushOutcome {
    pub fn new(status: PushStatus) -> PushOutcome {
        PushOutcome { status, retry_after: None, code: 0 }
    }

    /// `MobilePushSendOutcome.FromStatus`: the status's outcome, the code kept.
    pub fn from_status(code: u16, retry_after: Option<Duration>) -> PushOutcome {
        let (status, retry_after) = match code {
            200..=299 => (PushStatus::Sent, None),
            403 | 404 | 410 => (PushStatus::Gone, None),
            429 | 500 | 502 | 504 => (PushStatus::Retry, retry_after),
            _ => (PushStatus::Failed, None),
        };
        PushOutcome { status, retry_after, code }
    }

    pub fn retry(retry_after: Option<Duration>) -> PushOutcome {
        PushOutcome { status: PushStatus::Retry, retry_after, code: 0 }
    }
}

/// C# `MobilePushSendRequest(ServerId, Handle, Payload)`: the payload is the sealed envelope, never plaintext. `Debug` shows neither
/// the handle nor the envelope.
#[derive(Clone, PartialEq, Eq)]
pub struct PushSendRequest {
    pub server_id: String,
    pub handle: String,
    pub payload: String,
}

impl std::fmt::Debug for PushSendRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PushSendRequest {{ payload: {} characters }}", self.payload.len())
    }
}

impl PushSendRequest {
    /// `JsonSerializer.SerializeToUtf8Bytes(request, Wire.Json)`.
    pub fn body(&self) -> Vec<u8> {
        let mut out = JsonOut::new();
        out.begin_object();
        out.prop_str("serverId", Some(&self.server_id));
        out.prop_str("handle", Some(&self.handle));
        out.prop_str("payload", Some(&self.payload));
        out.end_object();
        out.finish().into_bytes()
    }
}

/// `MobilePushRelay.SendUri`: the relay's push route next to the WebSocket the host already dials (`wss`/`https` → `https`, `ws`/`http`
/// → `http`, the path and query replaced); None for anything else.
pub fn send_uri(relay: &str) -> Option<String> {
    let url = url::Url::parse(relay).ok()?;
    let host = url.host_str().filter(|h| !h.is_empty())?;
    let scheme = match url.scheme().to_ascii_lowercase().as_str() {
        "wss" | "https" => "https",
        "ws" | "http" => "http",
        _ => return None,
    };
    let default = if scheme == "https" { 443 } else { 80 };
    let port = url.port().filter(|p| *p != default).map(|p| format!(":{p}")).unwrap_or_default();
    Some(format!("{scheme}://{host}{port}/push/v1/send"))
}

/// `MobilePushHttpTransport.RetryAfter`: seconds only (an HTTP date counts as absent), finite, not negative, at most an hour.
pub fn retry_after(header: Option<&str>) -> Option<Duration> {
    let seconds: f64 = header?.trim_matches([' ', '\t']).parse().ok()?;
    if !seconds.is_finite() || seconds < 0.0 {
        return None;
    }
    Some(Duration::from_secs_f64(seconds.min(MAXIMUM_RETRY_AFTER.as_secs_f64())))
}

/// `IMobilePushTransport`: one send, its outcome; an error is what C#'s `SendAsync` throws (the hub tries it again). The hub's checks
/// give it a recording one.
pub trait PushTransport: Send + Sync {
    fn send(&self, request: PushSendRequest, url: String, host_token: String) -> BoxFuture<'_, mighty_platform::Result<PushOutcome>>;
}

/// `MobilePushHttpTransport` over mighty-platform's `HttpsSend::post`. A relay that answers 3xx is not followed (the port's contract):
/// the bearer token and the envelope go to the relay the user set and nowhere else, and the 3xx itself comes back as Failed. A
/// transport error (no network, a timeout, a reset) may pass, so it is a Retry; only its kind is logged, never the request.
pub struct HttpPushTransport {
    https: Arc<dyn HttpsSend>,
}

impl HttpPushTransport {
    pub fn new(https: Arc<dyn HttpsSend>) -> HttpPushTransport {
        HttpPushTransport { https }
    }
}

impl PushTransport for HttpPushTransport {
    fn send(&self, request: PushSendRequest, url: String, host_token: String) -> BoxFuture<'_, mighty_platform::Result<PushOutcome>> {
        Box::pin(async move {
            let request_head = HttpsRequest {
                url,
                headers: vec![("Authorization".into(), format!("Bearer {host_token}")), ("Content-Type".into(), "application/json".into())],
                timeout: TIMEOUT,
            };
            Ok(match self.https.post(request_head, request.body(), MAXIMUM_BODY).await {
                Ok(response) => PushOutcome::from_status(response.status, retry_after(response.retry_after.as_deref())),
                Err(error) => {
                    eprintln!("mobile push: relay unreachable ({})", crate::identity::error_kind(&error));
                    PushOutcome::new(PushStatus::Retry)
                }
            })
        })
    }
}
