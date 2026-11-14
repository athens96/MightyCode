//! The relay's status (C# `MobileRelayStatus`, `MobileRelayStatusText`, R\MobileRelayHost.cs:29-52; plan §1.1): connected, the
//! connections, the detail (`disabled, connecting, connected, reconnecting, offline`) and, while reconnecting, why the last control
//! socket ended (`rejected` 4400, `host-id-taken` 4409, `closed`, `unresponsive`, `connect-failed`) and when it dials again. The
//! Settings line's words are A's (`MobileRelayStatusText`). Owner: W8-A.

/// C# `MobileRelayStatus(Connected, Connections, Detail, Error?, RetrySeconds?)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobileRelayStatus {
    pub connected: bool,
    pub connections: usize,
    pub detail: String,
    pub error: Option<String>,
    pub retry_seconds: Option<u32>,
}

impl MobileRelayStatus {
    /// C#'s first status: `new(false, 0, "disabled")`.
    pub fn disabled() -> MobileRelayStatus {
        MobileRelayStatus { connected: false, connections: 0, detail: "disabled".into(), error: None, retry_seconds: None }
    }
}

impl Default for MobileRelayStatus {
    fn default() -> Self {
        MobileRelayStatus::disabled()
    }
}
