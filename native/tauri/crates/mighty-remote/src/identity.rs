//! The phone pairing identity (C# R\MobileIdentity.cs; plan §1.2) over mighty-platform's `PairingStore` (Windows:
//! `mobile-identity.json`, codec `pairing::identity_file`): created and saved at first use (only when the app may write), the device
//! registry (a device token's SHA-256 only, compared in fixed time; the token returned once in `auth_ok`), the auth decision
//! (`malformed`, `device-revoked`, `pairing-key`, `legacy-refused`, `device-limit`: more than 8 registrations an hour, or 32 devices
//! with none idle 90 days), re-pairing a client id, `Touch` at most once a minute, rotation, push registrations, and the pairing URL
//! `mightycode://pair?v=2&sid=…&pk=…&relay=…&key=…&name=…[&direct=…]` (`Uri.EscapeDataString`). Owner: W8-A.
//!
//! Every change is saved before it takes effect (C#'s `Save` keeps the old state when the write fails); an identity loaded without
//! `may_write` (verification mode) refuses every save, so it never writes the profile.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use base64::Engine;
use mighty_core::locale::Locale;
use mighty_core::{time, wire};
use mighty_platform::pairing::identity_file::{self, LEGACY, MAXIMUM_DEVICES};
use mighty_platform::{PairedDevice, PairingIdentity, PairingStore, PlatformError, PushRegistration, Result};
use serde_json::Value;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

use crate::push::message::KINDS;
use crate::relay::cipher;

/// How long a newly registered phone is marked as an arrival (`MobileIdentity.NewDeviceWindow`).
pub const NEW_DEVICE_WINDOW_MS: i64 = 24 * 60 * 60 * 1000;
/// New registrations an hour.
pub const REGISTRATIONS_PER_HOUR: usize = 8;
/// A device is replaced at the limit only when idle longer than this.
const IDLE_MS: i64 = 90 * 24 * 60 * 60 * 1000;
const HOUR_MS: i64 = 60 * 60 * 1000;
const MINUTE_MS: i64 = 60 * 1000;

/// The auth frame's members (`{type:"auth", clientId, clientName, pairingKey | deviceToken}`) as `JsonElementExtensions.Text` reads them
/// (a member that is not a string reads as absent); its secrets never reach `Debug`.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct AuthRequest {
    /// `type`; anything but `auth` is malformed.
    pub kind: Option<String>,
    /// Whether the frame has a `clientId` member at all (one that is there but no client id is malformed).
    pub client_id_given: bool,
    pub client_id: Option<String>,
    pub client_name: Option<String>,
    pub pairing_key: Option<String>,
    pub device_token: Option<String>,
}

impl AuthRequest {
    /// The decrypted auth frame; one that is not a JSON object reads as `{}` (malformed), as C#'s `AuthFrame` reads it.
    pub fn from_json(value: &Value) -> AuthRequest {
        let Some(object) = value.as_object() else { return AuthRequest::default() };
        let text = |name: &str| object.get(name).and_then(Value::as_str).map(str::to_string);
        AuthRequest {
            kind: text("type"),
            client_id_given: object.contains_key("clientId"),
            client_id: text("clientId"),
            client_name: text("clientName"),
            pairing_key: text("pairingKey"),
            device_token: text("deviceToken"),
        }
    }

    /// The decrypted bytes as a request.
    pub fn from_bytes(plaintext: &[u8]) -> AuthRequest {
        serde_json::from_slice::<Value>(plaintext).map(|value| AuthRequest::from_json(&value)).unwrap_or_default()
    }
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

impl AuthDecision {
    /// The refusal's word, None when accepted.
    pub fn error(&self) -> Option<&'static str> {
        match self {
            AuthDecision::Refused(refusal) => Some(refusal.reason()),
            AuthDecision::Accepted { .. } => None,
        }
    }

    pub fn device_id(&self) -> Option<&str> {
        match self {
            AuthDecision::Accepted { device_id, .. } => Some(device_id),
            AuthDecision::Refused(_) => None,
        }
    }

    pub fn device_token(&self) -> Option<&str> {
        match self {
            AuthDecision::Accepted { device_token, .. } => device_token.as_deref(),
            AuthDecision::Refused(_) => None,
        }
    }
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

/// C# `MobilePushRegistrationResult`. NeedsToken: an unknown phone or the legacy row, since the push key comes from a device token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushRegistrationResult {
    Saved,
    NeedsToken,
    Unsaved,
}

/// C# `MobilePushDevice(DeviceId, Handle, TokenHash, Kinds)`: a phone to push to, with what its key is made from. Never leaves the
/// host; `Debug` shows the device id and its kinds only.
#[derive(Clone, PartialEq, Eq)]
pub struct PushDevice {
    pub device_id: String,
    pub handle: String,
    pub token_hash: String,
    /// The known kinds it asked for, in the host's order.
    pub kinds: Vec<String>,
}

impl std::fmt::Debug for PushDevice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut kinds = self.kinds.clone();
        kinds.sort();
        write!(f, "PushDevice {{ device_id: {:?}, kinds: [{}] }}", self.device_id, kinds.join(", "))
    }
}

struct State {
    saved: PairingIdentity,
    /// When the registrations of the last hour were made (milliseconds since the epoch).
    registrations: Vec<i64>,
}

type Listener = Arc<dyn Fn() + Send + Sync>;

/// The pairing identity and its devices.
pub struct Identity {
    store: Arc<dyn PairingStore>,
    may_write: bool,
    locale: Locale,
    state: Mutex<State>,
    rotated: Mutex<Vec<(u64, Listener)>>,
    next_listener: AtomicU64,
}

/// A registration of [`Identity::on_rotated`]; dropping it unregisters.
pub struct RotationSubscription {
    identity: std::sync::Weak<Identity>,
    id: u64,
}

impl Drop for RotationSubscription {
    fn drop(&mut self) {
        if let Some(identity) = self.identity.upgrade() {
            lock(&identity.rotated).retain(|(id, _)| *id != self.id);
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn now_ms() -> i64 {
    time::clock_now().timestamp_millis()
}

fn parse_ms(value: &str) -> Option<i64> {
    time::parse(Some(value)).map(|at| at.to_chrono().timestamp_millis())
}

fn random<const N: usize>() -> Zeroizing<[u8; N]> {
    let mut bytes = Zeroizing::new([0u8; N]);
    getrandom::fill(bytes.as_mut()).expect("the OS random source");
    bytes
}

/// `MobileIdentity.RandomToken(32)`: 32 random bytes, base64url without padding.
pub fn random_token() -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(random::<32>().as_ref())
}

/// `MobileIdentity.Hash`: SHA-256 of the UTF-8 text, lower-case hex.
pub fn hash(text: &str) -> String {
    hex(&Sha256::digest(text.as_bytes()))
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn equal(first: &str, second: &str) -> bool {
    first.len() == second.len() && bool::from(first.as_bytes().ct_eq(second.as_bytes()))
}

/// `Uri.EscapeDataString`: every byte of the UTF-8 text but the unreserved `A-Z a-z 0-9 - _ . ~` as `%XX` (upper-case hex).
pub fn escape_data(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// `MobileIdentity.IsNew`: first seen less than a day ago (and not in the future).
pub fn is_new(first_seen: &str, now_ms: i64) -> bool {
    parse_ms(first_seen).is_some_and(|seen| (0..NEW_DEVICE_WINDOW_MS).contains(&(now_ms - seen)))
}

impl Identity {
    /// The saved identity, or a new one saved now when `may_write` (C#'s constructor); `Err` when the file is damaged (the caller
    /// shows it) or when there is none and the app may not write (verification mode: nothing is created).
    pub fn load_or_create(store: Arc<dyn PairingStore>, may_write: bool) -> Result<Identity> {
        let saved = match store.load()? {
            Some(saved) => saved,
            None if !may_write => return Err(PlatformError::Refused("verification mode: no pairing identity is made".into())),
            None => {
                let fresh = PairingIdentity {
                    host_id: wire::new_id(),
                    host_token: hex(random::<32>().as_ref()),
                    pairing_key: random_token(),
                    secret_key: *random::<32>(),
                    devices: vec![],
                };
                store.save(&fresh)?;
                fresh
            }
        };
        Ok(Identity {
            store,
            may_write,
            locale: Locale::new("en"),
            state: Mutex::new(State { saved, registrations: vec![] }),
            rotated: Mutex::new(vec![]),
            next_listener: AtomicU64::new(1),
        })
    }

    /// The language the legacy row and an unnamed phone are named in (C#'s `Locale.Get`).
    pub fn with_locale(mut self, locale: Locale) -> Identity {
        self.locale = locale;
        self
    }

    fn state(&self) -> MutexGuard<'_, State> {
        lock(&self.state)
    }

    /// `Save`: the store writes `next`, then it is the state; a write that fails (or verification mode) keeps the old state.
    fn save(&self, state: &mut State, next: PairingIdentity) -> Result<()> {
        if !self.may_write {
            return Err(PlatformError::Refused("verification mode: the pairing identity is not written".into()));
        }
        self.store.save(&next)?;
        state.saved = next;
        Ok(())
    }

    /// `HostId`.
    pub fn host_id(&self) -> String {
        self.state().saved.host_id.clone()
    }

    /// `ServerId`: SHA-256 of the host token, the relay's name for this host.
    pub fn server_id(&self) -> String {
        hash(&self.state().saved.host_token)
    }

    /// `HostToken` (the relay's bearer; never logged).
    pub(crate) fn host_token(&self) -> Zeroizing<String> {
        Zeroizing::new(self.state().saved.host_token.clone())
    }

    /// `Secret`: the relay's X25519 secret key.
    pub(crate) fn secret(&self) -> Zeroizing<[u8; 32]> {
        Zeroizing::new(self.state().saved.secret_key)
    }

    /// `PublicKey`: the host's X25519 public key, standard base64.
    pub fn public_key(&self) -> String {
        base64::engine::general_purpose::STANDARD.encode(cipher::public_key(&self.secret()))
    }

    /// `PairingKey`, for the settings box's masked key and its copy (never logged).
    pub fn pairing_key(&self) -> Zeroizing<String> {
        Zeroizing::new(self.state().saved.pairing_key.clone())
    }

    /// `Authenticate(auth, allowLegacy)`. `Err` when the change it makes (a `Touch`, a new device) could not be saved: C#'s `Save`
    /// throws there and the connection ends unanswered.
    pub fn authorize(&self, auth: &AuthRequest, allow_legacy: bool) -> Result<AuthDecision> {
        use AuthDecision::{Accepted, Refused};
        let mut state = self.state();
        if auth.kind.as_deref() != Some("auth") {
            return Ok(Refused(AuthRefusal::Malformed));
        }
        let id = auth.client_id.clone();
        if auth.client_id_given && !id.as_deref().is_some_and(identity_file::client_id) {
            return Ok(Refused(AuthRefusal::Malformed));
        }
        let presented = auth.pairing_key.as_deref();
        if presented.is_none()
            && let Some(token) = auth.device_token.as_deref().filter(|t| t.encode_utf16().count() <= 256)
        {
            let device = state.saved.devices.iter().find(|d| Some(&d.id) == id.as_ref()).cloned();
            let Some(device) = device.filter(|d| d.token_hash.as_deref().is_some_and(|stored| equal(stored, &hash(token)))) else {
                return Ok(Refused(AuthRefusal::DeviceRevoked));
            };
            self.touch(&mut state, &device.id)?;
            return Ok(Accepted { device_id: device.id, device_token: None });
        }
        let Some(presented) = presented.filter(|p| p.encode_utf16().count() <= 256) else {
            return Ok(Refused(AuthRefusal::Malformed));
        };
        if !equal(&hash(presented), &hash(&state.saved.pairing_key)) {
            return Ok(Refused(AuthRefusal::PairingKey));
        }
        let now = now_ms();
        let Some(id) = id else {
            if !allow_legacy {
                return Ok(Refused(AuthRefusal::LegacyRefused));
            }
            if state.saved.devices.iter().any(|d| d.id == LEGACY) {
                self.touch(&mut state, LEGACY)?;
            } else if state.saved.devices.len() >= MAXIMUM_DEVICES {
                return Ok(Refused(AuthRefusal::DeviceLimit));
            } else {
                let mark = time::now_o();
                let mut next = state.saved.clone();
                next.devices.push(PairedDevice {
                    id: LEGACY.into(),
                    name: "Legacy app".into(),
                    token_hash: None,
                    first_seen: mark.clone(),
                    last_seen: mark,
                    push: None,
                });
                self.save(&mut state, next)?;
            }
            return Ok(Accepted { device_id: LEGACY.into(), device_token: None });
        };
        // Pairing again with the key under an id that holds a token (the phone scanned the QR anew, say after the relay moved): the
        // row is the same phone's, so it gets a new token and the old one stops working (M/MobileDeviceRegistry.swift issueToken). Its
        // first-seen time stays; its push handle goes, since the phone registers one again for the new pairing.
        let earlier = state.saved.devices.iter().find(|d| d.id == id && d.token_hash.is_some()).map(|d| d.first_seen.clone());
        state.registrations.retain(|at| now - at <= HOUR_MS);
        if state.registrations.len() >= REGISTRATIONS_PER_HOUR {
            return Ok(Refused(AuthRefusal::DeviceLimit));
        }
        let mut devices: Vec<PairedDevice> = state.saved.devices.iter().filter(|d| d.id != id).cloned().collect();
        if devices.len() >= MAXIMUM_DEVICES {
            let old = devices
                .iter()
                .enumerate()
                .filter(|(_, d)| parse_ms(&d.last_seen).is_some_and(|last| now - last > IDLE_MS))
                .min_by(|(_, a), (_, b)| a.last_seen.cmp(&b.last_seen))
                .map(|(at, _)| at);
            let Some(old) = old else { return Ok(Refused(AuthRefusal::DeviceLimit)) };
            devices.remove(old);
        }
        let fresh = random_token();
        let given =
            auth.client_name.clone().filter(|n| !n.is_empty()).unwrap_or_else(|| self.locale.get("mobileRemote.device.defaultName"));
        let mark = time::now_o();
        devices.push(PairedDevice {
            id: id.clone(),
            name: wire::clean(&given, 40),
            token_hash: Some(hash(&fresh)),
            first_seen: earlier.unwrap_or_else(|| mark.clone()),
            last_seen: mark,
            push: None,
        });
        let next = PairingIdentity { devices, ..state.saved.clone() };
        self.save(&mut state, next)?;
        state.registrations.push(now);
        Ok(Accepted { device_id: id, device_token: Some(fresh) })
    }

    /// `Touch`: `lastSeen` written at most once a minute.
    fn touch(&self, state: &mut State, id: &str) -> Result<()> {
        let Some(device) = state.saved.devices.iter().find(|d| d.id == id) else { return Ok(()) };
        if parse_ms(&device.last_seen).is_some_and(|seen| now_ms() - seen <= MINUTE_MS) {
            return Ok(());
        }
        let mark = time::now_o();
        let mut next = state.saved.clone();
        for d in next.devices.iter_mut().filter(|d| d.id == id) {
            d.last_seen = mark.clone();
        }
        self.save(state, next)
    }

    /// `Rotated`: told after every rotation (a revoke included), once the new key is saved; every phone, its token and its push handle
    /// are gone. Dropping the subscription unregisters.
    pub fn on_rotated(self: &Arc<Self>, listener: impl Fn() + Send + Sync + 'static) -> RotationSubscription {
        let id = self.next_listener.fetch_add(1, Ordering::SeqCst);
        lock(&self.rotated).push((id, Arc::new(listener)));
        RotationSubscription { identity: Arc::downgrade(self), id }
    }

    /// How many rotation listeners are registered (the checks: a disposed hub lets go of its own).
    pub fn rotation_listeners(&self) -> usize {
        lock(&self.rotated).len()
    }

    /// `Rotate`: a new pairing key, every device dropped; the listeners told.
    pub fn rotate(&self) -> Result<()> {
        {
            let mut state = self.state();
            let next = PairingIdentity { pairing_key: random_token(), devices: vec![], ..state.saved.clone() };
            self.save(&mut state, next)?;
            state.registrations.clear();
        }
        let listeners: Vec<Listener> = lock(&self.rotated).iter().map(|(_, l)| l.clone()).collect();
        for listener in listeners {
            listener();
        }
        Ok(())
    }

    /// `Contains(id, legacyAllowed)`: a paired phone with a token, or the legacy row while legacy phones are allowed.
    pub fn contains(&self, id: &str, legacy_allowed: bool) -> bool {
        self.state().saved.devices.iter().any(|d| d.id == id && (d.token_hash.is_some() || (id == LEGACY && legacy_allowed)))
    }

    /// `PairingUrl(relay, name, direct)`: the v2 pairing link; `direct` adds the direct path's endpoints as `direct=`, joined with
    /// commas and percent-encoded as the Mac writes it.
    pub fn pairing_url(&self, relay: &str, name: &str, direct: &[String]) -> String {
        let state = self.state();
        let public = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(cipher::public_key(&Zeroizing::new(state.saved.secret_key)));
        let mut url = format!(
            "mightycode://pair?v=2&sid={}&pk={}&relay={}&key={}&name={}",
            hash(&state.saved.host_token),
            escape_data(&public),
            escape_data(relay),
            escape_data(&state.saved.pairing_key),
            escape_data(&wire::clean(name, 120))
        );
        if !direct.is_empty() {
            url.push_str("&direct=");
            url.push_str(&escape_data(&direct.join(",")));
        }
        url
    }

    /// `Devices`: the paired phones as the settings list draws them (`connected` false: the relay host knows who is connected), the
    /// row that stands for every app without a device token named in the user's language.
    pub fn devices(&self) -> Vec<MobileDeviceInfo> {
        let now = now_ms();
        self.state()
            .saved
            .devices
            .iter()
            .map(|d| MobileDeviceInfo {
                id: d.id.clone(),
                name: if d.id == LEGACY { self.locale.get("mobileRemote.device.legacyName") } else { d.name.clone() },
                first_seen: d.first_seen.clone(),
                last_seen: d.last_seen.clone(),
                legacy: d.token_hash.is_none(),
                connected: false,
                is_new: is_new(&d.first_seen, now),
            })
            .collect()
    }

    /// `SetPush(id, handle, kinds)`: saved, or why not (a phone without a token; a write that failed keeps the old registration).
    pub fn set_push(&self, device_id: &str, handle: &str, kinds: &[String]) -> PushRegistrationResult {
        let mut state = self.state();
        if !state.saved.devices.iter().any(|d| d.id == device_id && d.token_hash.is_some()) {
            return PushRegistrationResult::NeedsToken;
        }
        let mut next = state.saved.clone();
        let registration = PushRegistration { handle: handle.into(), kinds: kinds.to_vec(), updated_at: time::now_o() };
        for d in next.devices.iter_mut().filter(|d| d.id == device_id) {
            d.push = Some(registration.clone());
        }
        match self.save(&mut state, next) {
            Ok(()) => PushRegistrationResult::Saved,
            Err(error) => {
                eprintln!("mobile push: registration not saved: {}", error_kind(&error));
                PushRegistrationResult::Unsaved
            }
        }
    }

    /// `ClearPush(id, handle?)`: forgets a phone's handle; with `handle`, only while it is still that one (the relay said it is gone).
    /// False only when the file could not be written, so the handle is still there; nothing to clear is true.
    pub fn clear_push(&self, device_id: &str, handle: Option<&str>) -> bool {
        let mut state = self.state();
        let current = state.saved.devices.iter().find(|d| d.id == device_id).and_then(|d| d.push.as_ref());
        match current {
            None => return true,
            Some(push) if handle.is_some_and(|h| push.handle != h) => return true,
            Some(_) => {}
        }
        let mut next = state.saved.clone();
        for d in next.devices.iter_mut().filter(|d| d.id == device_id) {
            d.push = None;
        }
        match self.save(&mut state, next) {
            Ok(()) => true,
            Err(error) => {
                eprintln!("mobile push: handle not cleared: {}", error_kind(&error));
                false
            }
        }
    }

    /// `PushDevices`: the phones to push to, with what their key is made from (their kinds the known ones, in the host's order).
    pub fn push_devices(&self) -> Vec<PushDevice> {
        self.state()
            .saved
            .devices
            .iter()
            .filter_map(|d| {
                let push = d.push.as_ref()?;
                Some(PushDevice {
                    device_id: d.id.clone(),
                    handle: push.handle.clone(),
                    token_hash: d.token_hash.clone()?,
                    kinds: KINDS.iter().filter(|k| push.kinds.iter().any(|w| w == *k)).map(|k| k.to_string()).collect(),
                })
            })
            .collect()
    }
}

/// A platform error as the one word a log may carry (C# logs `ex.GetType().Name`): never a path or a value.
pub fn error_kind(error: &PlatformError) -> &'static str {
    match error {
        PlatformError::Io(_) => "IOException",
        PlatformError::Refused(_) => "UnauthorizedAccessException",
        _ => "Exception",
    }
}
