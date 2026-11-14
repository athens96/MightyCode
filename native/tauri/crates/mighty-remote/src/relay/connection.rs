//! One phone's connection, shared by the relay and the direct path (C# `MobileRelayHost`'s per-connection half, R\MobileRelayHost.cs:
//! 590-714; plan §1.1): over any `WebSocketStream`, the plaintext hello and ready, the X25519 key, the encrypted `auth` with its 10 s
//! deadline (`auth_ok` / `auth_error` and a close; on the direct path the reason is always `unauthorized`), then the request pump with
//! 8 slots (a duplicate id or a ninth answers 429), one writer, `ping` → `pong`, the host's encrypted ping every 20 s with a 60 s read
//! idle limit, notify frames, and the host-level errors (414 path, 413 reply, 500). Owner: W8-A.
//!
//! Written whole in W8 Step 0: [`RequestHandler`] (what the m1 router is to a connection: `phone::router::Router` implements it, A's
//! round-trip test a stub), [`MobileReply`] and [`MobileDevice`]. The connection itself is A's.

use mighty_core::json::JsonOut;
use mighty_platform::BoxFuture;
use serde_json::value::RawValue;

/// What runs once a reply's frame was sent (C# `MobileReply.AfterSend`: an app update that restarts the app after answering).
pub type AfterSend = Box<dyn FnOnce() -> BoxFuture<'static, ()> + Send>;

/// C# `MobileReply(Status, Body, AfterSend?)`: a route's answer. `body` is the JSON text as C#'s `Wire.Json` writes it (written with
/// `mighty_core::json::JsonOut`, never serde: its escaping decides the 1 MiB limit, plan §5 R2).
pub struct MobileReply {
    pub status: u16,
    pub body: String,
    pub after_send: Option<AfterSend>,
}

impl MobileReply {
    pub fn new(status: u16, body: String) -> MobileReply {
        MobileReply { status, body, after_send: None }
    }

    /// `{protocol:1, error, code?}` (C# `Error(status, text, code)`; the host's errors, R\MobileRelayHost.cs:638-682, and the router's).
    pub fn error(status: u16, error: &str, code: Option<&str>) -> MobileReply {
        let mut out = JsonOut::new();
        out.begin_object();
        out.prop_int("protocol", 1);
        out.prop_str("error", Some(error));
        out.prop_str("code", code);
        out.end_object();
        MobileReply::new(status, out.finish())
    }
}

impl std::fmt::Debug for MobileReply {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MobileReply")
            .field("status", &self.status)
            .field("body_bytes", &self.body.len())
            .field("after_send", &self.after_send.is_some())
            .finish()
    }
}

/// The phone a request comes from, once its `auth` passed: its 22-character client id (or `legacy`), its name, and whether it came
/// over the direct path. Never its token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobileDevice {
    pub client_id: String,
    pub name: String,
    pub direct: bool,
}

/// What answers an authenticated phone's requests (C#'s `Func<string, string, JsonElement?, string, CancellationToken,
/// Task<MobileReply>>` handed to `MobileRelayHost`: WinUI `MobileRoute` over `MobileDesktopRouter.RouteAsync`).
///
/// Contract: `method` is `GET` or `POST`, `path` starts with `/` and is at most 16384 bytes (the connection answered 414 before);
/// `body` is the request's `body` member as the phone sent it (its raw JSON text, so the router can measure it), None when absent.
/// The future is `'static`: the connection runs it on a task of its own (8 at a time), and dropping it cancels the request (the
/// connection closed). It never panics on a phone's input; a failure is a reply (C#'s 500 "Desktop request failed.").
pub trait RequestHandler: Send + Sync {
    fn handle(&self, device: MobileDevice, method: String, path: String, body: Option<Box<RawValue>>) -> BoxFuture<'static, MobileReply>;
}
