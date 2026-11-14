//! The relay's status (C# `MobileRelayStatus`, `MobileRelayStatusText`, R\MobileRelayHost.cs:29-52; plan §1.1): connected, the
//! connections, the detail (`disabled, connecting, connected, reconnecting, offline`) and, while reconnecting, why the last control
//! socket ended (`rejected` 4400, `host-id-taken` 4409, `closed`, `unresponsive`, `connect-failed`) and when it dials again. The
//! Settings line's words are A's (`MobileRelayStatusText`). Owner: W8-A.

use mighty_core::locale::Locale;

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
        MobileRelayStatus::new(false, 0, "disabled", None, None)
    }

    pub fn new(connected: bool, connections: usize, detail: &str, error: Option<&str>, retry_seconds: Option<u32>) -> MobileRelayStatus {
        MobileRelayStatus { connected, connections, detail: detail.into(), error: error.map(str::to_string), retry_seconds }
    }
}

impl Default for MobileRelayStatus {
    fn default() -> Self {
        MobileRelayStatus::disabled()
    }
}

/// Why a control socket ended, by its close code (`relay/src/protocol.ts`): 4400 rejected, 4409 the host id taken by another host
/// with the same token, anything else closed; no close frame after connecting is unresponsive, a dial that failed connect-failed.
pub fn close_reason(close: Option<u16>, connected: bool) -> &'static str {
    match close {
        Some(4400) => "rejected",
        Some(4409) => "host-id-taken",
        Some(_) => "closed",
        None if connected => "unresponsive",
        None => "connect-failed",
    }
}

/// `MobileRelayStatusText.Reason`: the words for why the last control socket ended.
pub fn reason(locale: &Locale, error: Option<&str>) -> Option<String> {
    let key = match error? {
        "rejected" => "mobileRemote.error.relayRejected",
        "host-id-taken" => "mobileRemote.error.hostIdTaken",
        "closed" => "mobileRemote.error.relayClosed",
        "unresponsive" => "settings.mobileRemote.detail.relayUnresponsive",
        "connect-failed" => "mobileRemote.error.relayConnectFailed",
        _ => return None,
    };
    Some(locale.get(key))
}

/// `MobileRelayStatusText.Detail`: the line under the relay's state in Settings, as the Mac words it.
pub fn detail(locale: &Locale, status: &MobileRelayStatus) -> String {
    match status.detail.as_str() {
        "disabled" => locale.get("mobileRemote.status.off"),
        "connected" => locale.get("mobileRemote.status.scanQR"),
        "offline" => locale.get("settings.mobileRemote.detail.networkOffline"),
        "reconnecting" => {
            let lead = match reason(locale, status.error.as_deref()) {
                Some(reason) => reason + " · ",
                None => locale.get("mobileRemote.status.disconnected") + " ",
            };
            lead + &locale.get_with("mobileRemote.status.retryIn", &[("seconds", &status.retry_seconds.unwrap_or(0).to_string())])
        }
        _ => locale.get("mobileRemote.status.connecting"),
    }
}
