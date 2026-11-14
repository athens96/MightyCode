//! Sending a push (C# `MobilePushTransport`, `MobilePushRelay`, R\MobilePush.cs; plan §1.3): `POST https://<relay host>/push/v1/send`
//! (`wss`→`https`, `ws`→`http`) through mighty-platform's `HttpsSend::post` with `Authorization: Bearer <hostToken>` and
//! `Content-Type: application/json`, body `{"serverId","handle","payload"}`, 15 s, no redirect, no cookies, the system proxy; 2xx Sent,
//! 403/404/410 Gone (only that handle cleared), 429/500/502/504 or a network error Retry (waits 2 s then 8 s; `Retry-After` in
//! seconds capped at 1 h, then 30 s by the hub), anything else Failed. Never a real push server in a test or the smoke. Owner: W8-A.

use std::time::Duration;

/// What one send came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushOutcome {
    Sent,
    Gone,
    Retry(Option<Duration>),
    Failed,
}
