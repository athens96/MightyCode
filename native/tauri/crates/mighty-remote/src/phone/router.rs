//! The m1 router (C# `MobileDesktopRouter`, R\MobileDesktopRouter.cs:97-553; plan §1.5): `/m1/` paths only (no `#` or `\`), a
//! repeated query key or a POST with a query 400, unknown routes 404 `"Mobile route not found."`, errors `{protocol:1, error, code?}`,
//! C#'s exception mapping ([`crate::phone::desk::MobileError`]), a reply over `MAXIMUM_REPLY_BYTES` 413 (the structure shed first),
//! every route of the table with its statuses and codes, the long polls (`/m1/state`, `/m1/sessions/{id}`: 100 ms re-checks, 10 s at
//! most) on their own tasks, two submit slots, two file reads, the push routes, the screen-share routes 503 while F's switch is off.
//! It implements [`crate::relay::connection::RequestHandler`] for the relay and direct connections. Owner: W8-B.
//!
//! At rest (W8 Step 0): every route answers 404 and nothing is ever notified.

use std::sync::Arc;

use mighty_platform::BoxFuture;
use serde_json::value::RawValue;

use crate::phone::desk::PhoneDesk;
use crate::relay::connection::{MobileDevice, MobileReply, RequestHandler};

/// `MobileDesktopRouter.Capabilities` (`:37`).
pub const CAPABILITIES: [&str; 21] = [
    "submit-mode",
    "queue",
    "pane",
    "history",
    "settings",
    "commands",
    "mighty",
    "status",
    "attachments",
    "style",
    "files",
    "structure",
    "direct",
    "push",
    "fileChanges",
    "workMap",
    "appUpdate",
    "pi",
    "opencode",
    "hermes",
    "roles",
];

/// `MobileWire.MaximumReplyBytes`: 1 MiB − 32 KiB.
pub const MAXIMUM_REPLY_BYTES: usize = 1024 * 1024 - 32 * 1024;

/// The m1 router over a desk.
pub struct Router {
    desk: Arc<dyn PhoneDesk>,
}

impl Router {
    pub fn new(desk: Arc<dyn PhoneDesk>) -> Router {
        Router { desk }
    }

    /// `RouteAsync(method, path, body, device)`.
    pub fn route(
        &self,
        device: MobileDevice,
        method: String,
        path: String,
        body: Option<Box<RawValue>>,
    ) -> BoxFuture<'static, MobileReply> {
        let _ = (device, method, path, body, &self.desk);
        Box::pin(async { MobileReply::error(404, "Mobile route not found.", None) })
    }

    /// `Changed`: something a phone may show moved (a run event, a permission, delegation, an Antigravity card); the panes are read again
    /// at the next [`Router::observe`]. Cheap: the host calls it from its change hook.
    pub fn changed(&self) {}

    /// `Observe`: the panes read again when something moved, the revisions of what changed moved.
    pub fn observe(&self) {}

    /// `TakeNotifications`: the scopes whose revision moved since the last call, `state` before the sessions, each with its newest
    /// revision.
    pub fn take_notifications(&self) -> Vec<(String, i64)> {
        vec![]
    }
}

impl RequestHandler for Router {
    fn handle(&self, device: MobileDevice, method: String, path: String, body: Option<Box<RawValue>>) -> BoxFuture<'static, MobileReply> {
        self.route(device, method, path, body)
    }
}
