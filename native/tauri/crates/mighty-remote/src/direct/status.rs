//! The direct path's status (C# `MobileDirectStatus`, R\MobileDirect.cs; plan §1.4): `off, disabled, notFound, listening, failed`,
//! the error codes `addressInUse, accessDenied, bindFailed, adaptersUnreadable` (the OS's own words only in the log), and the endpoints
//! advertised. Owner: W8-C.

/// `MobileDirectStatus.State`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DirectState {
    #[default]
    Off,
    Disabled,
    NotFound,
    Listening,
    Failed,
}

/// C# `MobileDirectStatus(State, Endpoints, Error?)`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MobileDirectStatus {
    pub state: DirectState,
    pub endpoints: Vec<String>,
    pub error: Option<String>,
}
