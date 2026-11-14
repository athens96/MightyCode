//! The sharing host (C# `RemoteHostService`, R\RemoteHostService.cs; plan §1.6): the key (32 random bytes, base64url, new on every
//! Start, in memory only, revoked at Stop; compared as SHA-256 in fixed time), the status order of `Authorize` (503, 403, 429 per peer,
//! 401, 429 overall, 426) and of the routes, `GET /v2/info`, the runs (`runs`), the 30 s heartbeat. Started only from the Start button
//! when the app may listen (`Host::may_listen`). Owner: W8-D.
//!
//! At rest (W8 Step 0): a host that never shares.

pub mod runs;

use crate::remote::wire::RemoteHostState;

/// The sharing host.
#[derive(Default)]
pub struct RemoteHost {}

impl RemoteHost {
    /// What "이 컴퓨터 공유" shows (the key never leaves through it but to the copy button).
    pub fn state(&self) -> RemoteHostState {
        RemoteHostState::default()
    }
}
