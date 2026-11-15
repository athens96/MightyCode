//! The direct path's status (C# `MobileDirectStatus`, `MobileDirectError`, R\MobileDirect.cs; plan §1.4): `off` (the remote is off),
//! `disabled` (its direct switch is), `notFound` (no Tailscale address), `listening` (on its endpoints), `failed` (no address could be
//! bound, or the adapters could not be read: a code only — `addressInUse, accessDenied, bindFailed, adaptersUnreadable` —, the OS's own
//! words going to the log only). Owner: W8-C.

use mighty_platform::PlatformError;
use serde::Serialize;

/// `MobileDirectStatus.State`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DirectState {
    #[default]
    Off,
    Disabled,
    NotFound,
    Listening,
    Failed,
}

/// C# `MobileDirectStatus(State, Endpoints, Error?)`; serialized as C#'s `Wire.Json` writes it (`{"state":"off","endpoints":[]}`, the
/// error only when there is one, never the computed `listening`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct MobileDirectStatus {
    pub state: DirectState,
    pub endpoints: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl MobileDirectStatus {
    pub fn new(state: DirectState, endpoints: Vec<String>, error: Option<&str>) -> MobileDirectStatus {
        MobileDirectStatus { state, endpoints, error: error.map(str::to_string) }
    }

    /// `MobileDirectStatus.Off`.
    pub fn off() -> MobileDirectStatus {
        MobileDirectStatus::default()
    }

    /// `MobileDirectStatus.Listening`.
    pub fn listening(&self) -> bool {
        self.state == DirectState::Listening
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

/// `MobileDirectError.AddressInUse`: another program holds the port on that address.
pub const ADDRESS_IN_USE: &str = "addressInUse";
/// `MobileDirectError.AccessDenied`: the OS refused the port (a reserved range, or another program's exclusive use).
pub const ACCESS_DENIED: &str = "accessDenied";
/// `MobileDirectError.BindFailed`: any other reason binding failed.
pub const BIND_FAILED: &str = "bindFailed";
/// `MobileDirectError.AdaptersUnreadable`: this PC's network adapters could not be read.
pub const ADAPTERS_UNREADABLE: &str = "adaptersUnreadable";

/// `MobileDirectError.Of(SocketException)`: the code for a listener that could not start; the error's own words go to the log only.
pub fn error_of(error: &PlatformError) -> &'static str {
    match error {
        PlatformError::Io(io) => match io.kind() {
            std::io::ErrorKind::AddrInUse => ADDRESS_IN_USE,
            std::io::ErrorKind::PermissionDenied => ACCESS_DENIED,
            _ => BIND_FAILED,
        },
        _ => BIND_FAILED,
    }
}
