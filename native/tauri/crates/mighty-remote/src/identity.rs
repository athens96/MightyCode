//! The phone pairing identity (C# R\MobileIdentity.cs; plan §1.2) over mighty-platform's `PairingStore` (Windows:
//! `mobile-identity.json`, codec `pairing::identity_file`): created and saved at first use (only when the app may write), the device
//! registry (a device token's SHA-256 only, compared in fixed time; the token returned once in `auth_ok`), the auth decision
//! (`malformed`, `device-revoked`, `pairing-key`, `legacy-refused`, `device-limit`: more than 8 registrations an hour, or 32 devices
//! with none idle 90 days), re-pairing a client id, `Touch` at most once a minute, rotation, push registrations, and the pairing URL
//! `mightycode://pair?v=2&sid=…&pk=…&relay=…&key=…&name=…[&direct=…]` (`Uri.EscapeDataString`). Owner: W8-A.
//!
//! At rest (W8 Step 0): no identity is loaded or created ([`Identity::load_or_create`] answers `Unsupported`).

use std::sync::Arc;

use mighty_platform::{PairingStore, PlatformError, PushRegistration, Result};

/// The auth frame's members (`{type:"auth", clientId, clientName, pairingKey | deviceToken}`); its secrets never reach `Debug`.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct AuthRequest {
    pub client_id: Option<String>,
    pub client_name: Option<String>,
    pub pairing_key: Option<String>,
    pub device_token: Option<String>,
}

impl std::fmt::Debug for AuthRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthRequest").field("client_id", &self.client_id).field("client_name", &self.client_name).finish_non_exhaustive()
    }
}

/// `MobileAuthResult.Error`'s words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthRefusal {
    Malformed,
    DeviceRevoked,
    PairingKey,
    LegacyRefused,
    DeviceLimit,
}

impl AuthRefusal {
    /// The `reason` of `auth_error`.
    pub fn reason(self) -> &'static str {
        match self {
            AuthRefusal::Malformed => "malformed",
            AuthRefusal::DeviceRevoked => "device-revoked",
            AuthRefusal::PairingKey => "pairing-key",
            AuthRefusal::LegacyRefused => "legacy-refused",
            AuthRefusal::DeviceLimit => "device-limit",
        }
    }
}

/// C# `MobileAuthResult(DeviceId, DeviceToken, Error)`: the device accepted (with its new token on first pairing or re-pairing), or
/// why not. The token never reaches `Debug`.
#[derive(Clone, PartialEq, Eq)]
pub enum AuthDecision {
    Accepted { device_id: String, device_token: Option<String> },
    Refused(AuthRefusal),
}

impl std::fmt::Debug for AuthDecision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuthDecision::Accepted { device_id, device_token } => f
                .debug_struct("Accepted")
                .field("device_id", device_id)
                .field("device_token", &device_token.as_ref().map(|_| "<redacted>"))
                .finish(),
            AuthDecision::Refused(refusal) => f.debug_tuple("Refused").field(refusal).finish(),
        }
    }
}

/// C# `MobileDeviceInfo(Id, Name, FirstSeen, LastSeen, Legacy, Connected, IsNew)`: a paired phone as the settings list draws it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobileDeviceInfo {
    pub id: String,
    pub name: String,
    pub first_seen: String,
    pub last_seen: String,
    pub legacy: bool,
    pub connected: bool,
    pub is_new: bool,
}

/// C# `MobilePushRegistrationResult`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushRegistrationResult {
    Saved,
    NeedsToken,
    Unsaved,
}

/// The pairing identity and its devices.
pub struct Identity {
    store: Arc<dyn PairingStore>,
}

impl Identity {
    /// The saved identity, or a new one saved now when `may_write` (C#'s constructor); `Err` when the file is damaged (the caller
    /// shows it) or when there is none and the app may not write (verification mode: nothing is created).
    pub fn load_or_create(store: Arc<dyn PairingStore>, may_write: bool) -> Result<Identity> {
        let _ = (store, may_write);
        Err(PlatformError::unsupported("Identity", "load_or_create"))
    }

    /// `HostId`.
    pub fn host_id(&self) -> String {
        let _ = &self.store;
        String::new()
    }

    /// `Authenticate(auth, allowLegacy)`.
    pub fn authorize(&self, auth: &AuthRequest, allow_legacy: bool) -> AuthDecision {
        let _ = (auth, allow_legacy);
        AuthDecision::Refused(AuthRefusal::Malformed)
    }

    /// `PairingUrl(relay, name, direct)`.
    pub fn pairing_url(&self, relay: &str, name: &str, direct: &[String]) -> String {
        let _ = (relay, name, direct);
        String::new()
    }

    /// `Devices` (the legacy row named in the user's language by the caller).
    pub fn devices(&self) -> Vec<MobileDeviceInfo> {
        vec![]
    }

    /// A revoke is a rotation that drops every device (C#: `RotateKeyAsync(expectedDeviceId)`); true when it happened.
    pub fn revoke(&self, device_id: &str) -> Result<bool> {
        let _ = device_id;
        Ok(false)
    }

    /// `SetPush(id, handle, kinds)`.
    pub fn set_push(&self, device_id: &str, registration: PushRegistration) -> Result<PushRegistrationResult> {
        let _ = (device_id, registration);
        Ok(PushRegistrationResult::Unsaved)
    }

    /// `ClearPush(id, handle?)`: only that handle when one is given (Gone clears only that handle).
    pub fn clear_push(&self, device_id: &str, handle: Option<&str>) -> Result<bool> {
        let _ = (device_id, handle);
        Ok(false)
    }
}
