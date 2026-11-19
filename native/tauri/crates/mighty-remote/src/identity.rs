//! The phone pairing identity (C# R\MobileIdentity.cs; plan §1.2) over mighty-platform's `PairingStore` (Windows:
//! `mobile-identity.json`, codec `pairing::identity_file`): created and saved at first use (only when the app may write), the device
//! registry (a device token's SHA-256 only, compared in fixed time; the token returned once in `auth_ok`), the auth decision
//! (`malformed`, `device-revoked`, `pairing-key`, `legacy-refused`, `device-limit`: more than 8 registrations an hour, or 32 devices
//! with none idle 90 days), re-pairing a client id, `Touch` at most once a minute, rotation, push registrations, and the pairing URL
//! `mightycode://pair?v=2&sid=…&pk=…&relay=…&key=…&name=…[&direct=…]` (`Uri.EscapeDataString`). Owner: W8-A.
//!
//! A paired computer (the remote mirror through the relay, docs/remote-workspaces.md "페어링으로 연결") pairs with the same key and lives
//! in the same device list, but says `"role":"desktop"` in its auth and its token's hash is taken over [`DESKTOP_TOKEN_DOMAIN`] and the
//! token: a computer's token never opens a phone session and a phone's never a computer's, with no new member in either OS's identity
//! files (their codecs are byte for byte the older apps'), and an older app that reads the file meets only a token it cannot match.
//!
//! Every change is saved before it takes effect (C#'s `Save` keeps the old state when the write fails); an identity loaded without
//! `may_write` (verification mode) refuses every save, so it never writes the profile.
//!
//! Pairing (the security review's fix): a device registers only with a short-lived pairing code, never with a key that lives on.
//! There are two codes, held in memory only: the phone's (the QR and the 휴대폰 tab's link) and the computers' (원격 연결 › 이 컴퓨터
//! 공유's link), so a phone's link never pairs a computer nor the other way round. A code is made when Settings shows it, lives
//! [`PAIRING_CODE_LIFETIME`] from the last time it was shown (Settings draws it four times a second while it is on screen, which keeps
//! it alive there), and is spent by one registration, approved or refused: the next draw shows a new one. A registration with a code
//! is [`AuthDecision::Pending`] until the person at this computer allows it ([`Identity::register`]) or refuses it, or it times out
//! ([`Identity::refuse`]); the relay host asks. A device with its token never asks again. The key the identity file keeps
//! (`pairing_key`) is now only the legacy apps' (key-only auth, off unless Settings › 휴대폰 › 고급 turns it on).

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

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

/// The auth frame's `role` of another computer's Mighty Code.
pub const DESKTOP_ROLE: &str = "desktop";
/// What a computer's token is hashed after (`SHA-256(domain ‖ token)`): the role is bound to the token itself, so the device list needs no
/// member of its own and a phone's token can never be presented as a computer's (or the other way round).
pub const DESKTOP_TOKEN_DOMAIN: &str = "mighty-desktop-v1:";

/// How long a newly registered phone is marked as an arrival (`MobileIdentity.NewDeviceWindow`).
pub const NEW_DEVICE_WINDOW_MS: i64 = 24 * 60 * 60 * 1000;
/// New registrations an hour.
pub const REGISTRATIONS_PER_HOUR: usize = 8;
/// A device is replaced at the limit only when idle longer than this.
const IDLE_MS: i64 = 90 * 24 * 60 * 60 * 1000;
const HOUR_MS: i64 = 60 * 60 * 1000;
const MINUTE_MS: i64 = 60 * 1000;
/// How long a pairing code lives after it was last shown.
pub const PAIRING_CODE_LIFETIME: Duration = Duration::from_secs(10 * 60);
/// The file beside the identity that says which devices are computers (the identity files keep no such member).
pub const ROLES_FILE: &str = "device-roles.json";

/// Which pairing code: the phone's or the computers'.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PairingKind {
    Phone,
    Desktop,
}

impl PairingKind {
    fn slot(self) -> usize {
        match self {
            PairingKind::Phone => 0,
            PairingKind::Desktop => 1,
        }
    }
}

/// What an auth may do besides a token: the legacy apps' key-only auth (Settings › 휴대폰 › 고급) and a computer's registration (원격
/// 연결 › 이 컴퓨터 공유's "페어링한 컴퓨터의 연결 받기", while sharing).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AuthPolicy {
    pub allow_legacy: bool,
    pub allow_desktops: bool,
}

/// A registration with a pairing code waiting for the person at this computer: who asks (its client id and name, a computer or a
/// phone, whether it would replace a device paired under the same id) and which code it presented. Never its code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingRegistration {
    pub client_id: String,
    pub name: String,
    pub desktop: bool,
    /// A device is paired under this id already: allowing it replaces that device (its token stops working).
    pub replaces: bool,
    serial: u64,
}

impl PendingRegistration {
    pub fn kind(&self) -> PairingKind {
        if self.desktop { PairingKind::Desktop } else { PairingKind::Phone }
    }
}

/// A live pairing code (in memory only).
struct Code {
    key: Zeroizing<String>,
    expires: Instant,
    serial: u64,
}

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
    /// `"role":"desktop"`: another computer's Mighty Code (the remote mirror through the relay), never a phone.
    pub desktop: bool,
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
            desktop: text("role").as_deref() == Some(DESKTOP_ROLE),
        }
    }

    /// The decrypted bytes as a request.
    pub fn from_bytes(plaintext: &[u8]) -> AuthRequest {
        serde_json::from_slice::<Value>(plaintext).map(|value| AuthRequest::from_json(&value)).unwrap_or_default()
    }
}

impl std::fmt::Debug for AuthRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthRequest")
            .field("client_id", &self.client_id)
            .field("client_name", &self.client_name)
            .field("desktop", &self.desktop)
            .finish_non_exhaustive()
    }
}

/// `MobileAuthResult.Error`'s words, and the two pairing ones: the person at this computer refused (or did not answer in time), and a
/// computer's registration while this computer takes none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthRefusal {
    Malformed,
    DeviceRevoked,
    PairingKey,
    LegacyRefused,
    DeviceLimit,
    ApprovalDenied,
    DesktopsOff,
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
            AuthRefusal::ApprovalDenied => "approval-denied",
            AuthRefusal::DesktopsOff => "desktops-off",
        }
    }
}

/// C# `MobileAuthResult(DeviceId, DeviceToken, Error)`: the device accepted (with its new token on first pairing or re-pairing), a
/// registration waiting for the person at this computer, or why not. The token never reaches `Debug`.
#[derive(Clone, PartialEq, Eq)]
pub enum AuthDecision {
    Accepted { device_id: String, device_token: Option<String> },
    Pending(PendingRegistration),
    Refused(AuthRefusal),
}

impl AuthDecision {
    /// The refusal's word, None when accepted.
    pub fn error(&self) -> Option<&'static str> {
        match self {
            AuthDecision::Refused(refusal) => Some(refusal.reason()),
            AuthDecision::Accepted { .. } | AuthDecision::Pending(_) => None,
        }
    }

    pub fn device_id(&self) -> Option<&str> {
        match self {
            AuthDecision::Accepted { device_id, .. } => Some(device_id),
            AuthDecision::Pending(_) | AuthDecision::Refused(_) => None,
        }
    }

    pub fn device_token(&self) -> Option<&str> {
        match self {
            AuthDecision::Accepted { device_token, .. } => device_token.as_deref(),
            AuthDecision::Pending(_) | AuthDecision::Refused(_) => None,
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
            AuthDecision::Pending(pending) => f.debug_tuple("Pending").field(pending).finish(),
            AuthDecision::Refused(refusal) => f.debug_tuple("Refused").field(refusal).finish(),
        }
    }
}

/// C# `MobileDeviceInfo(Id, Name, FirstSeen, LastSeen, Legacy, Connected, IsNew)`: a paired phone as the settings list draws it.
/// `desktop`: another computer's Mighty Code, known once it has authenticated with `"role":"desktop"`; kept across restarts in
/// [`ROLES_FILE`] beside the identity (the identity files keep no such member).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobileDeviceInfo {
    pub id: String,
    pub name: String,
    pub first_seen: String,
    pub last_seen: String,
    pub legacy: bool,
    pub connected: bool,
    pub is_new: bool,
    pub desktop: bool,
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
    /// When the registrations of the last hour were asked for (milliseconds since the epoch).
    registrations: Vec<i64>,
    /// The ids that authenticated as another computer (kept in [`ROLES_FILE`] when the identity has one).
    desktops: HashSet<String>,
    /// The live pairing codes, the phone's and the computers' ([`PairingKind::slot`]).
    codes: [Option<Code>; 2],
    next_serial: u64,
}

/// Told after a rotation (`None`: every device is gone) or a single device's revoke (`Some(id)`, once per row removed).
type Listener = Arc<dyn Fn(Option<&str>) + Send + Sync>;

/// The pairing identity and its devices.
pub struct Identity {
    store: Arc<dyn PairingStore>,
    may_write: bool,
    locale: Locale,
    /// Where the computers' ids are kept ([`ROLES_FILE`]); none: in memory for this run.
    roles: Option<PathBuf>,
    code_lifetime: Duration,
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

/// The stored hash of a device token: a phone's as before, a computer's over [`DESKTOP_TOKEN_DOMAIN`].
fn token_hash(token: &str, desktop: bool) -> String {
    if desktop { hash(&format!("{DESKTOP_TOKEN_DOMAIN}{token}")) } else { hash(token) }
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

/// The device list without the row of `id` (a registration under it replaces it) with room for one more: at [`MAXIMUM_DEVICES`] the
/// device idle longest (over 90 days) makes way; None when none may.
fn room(devices: &[PairedDevice], id: &str, now: i64) -> Option<Vec<PairedDevice>> {
    let mut devices: Vec<PairedDevice> = devices.iter().filter(|d| d.id != id).cloned().collect();
    if devices.len() >= MAXIMUM_DEVICES {
        let old = devices
            .iter()
            .enumerate()
            .filter(|(_, d)| parse_ms(&d.last_seen).is_some_and(|last| now - last > IDLE_MS))
            .min_by(|(_, a), (_, b)| a.last_seen.cmp(&b.last_seen))
            .map(|(at, _)| at)?;
        devices.remove(old);
    }
    Some(devices)
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
            roles: None,
            code_lifetime: PAIRING_CODE_LIFETIME,
            state: Mutex::new(State { saved, registrations: vec![], desktops: HashSet::new(), codes: [None, None], next_serial: 1 }),
            rotated: Mutex::new(vec![]),
            next_listener: AtomicU64::new(1),
        })
    }

    /// The language the legacy row and an unnamed phone are named in (C#'s `Locale.Get`).
    pub fn with_locale(mut self, locale: Locale) -> Identity {
        self.locale = locale;
        self
    }

    /// The computers' ids kept in `path` ([`ROLES_FILE`] beside the identity): read now (a file that is missing or not this shape
    /// reads as none), written whenever a computer pairs or a device goes.
    pub fn with_roles_file(mut self, path: PathBuf) -> Identity {
        let known = std::fs::read(&path).ok().and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok()).map(|value| {
            value
                .get("desktops")
                .and_then(Value::as_array)
                .map(|ids| {
                    ids.iter()
                        .filter_map(Value::as_str)
                        .filter(|id| identity_file::client_id(id))
                        .map(str::to_string)
                        .collect::<Vec<String>>()
                })
                .unwrap_or_default()
        });
        if let Some(known) = known {
            let state = self.state.get_mut().unwrap_or_else(|poisoned| poisoned.into_inner());
            let paired: HashSet<String> = state.saved.devices.iter().map(|d| d.id.clone()).collect();
            state.desktops = known.into_iter().filter(|id| paired.contains(id)).collect();
        }
        self.roles = Some(path);
        self
    }

    /// How long a pairing code lives after it was last shown (the checks shorten it).
    pub fn with_code_lifetime(mut self, lifetime: Duration) -> Identity {
        self.code_lifetime = lifetime;
        self
    }

    fn state(&self) -> MutexGuard<'_, State> {
        lock(&self.state)
    }

    /// The computers' ids written to [`ROLES_FILE`] (only the paired ones; nothing in verification mode or without a file). A write
    /// that fails is no failure of the change: the icon reads as a phone's after a restart until the computer connects again.
    fn save_roles(&self, state: &State) {
        let Some(path) = self.roles.as_ref().filter(|_| self.may_write) else { return };
        let mut ids: Vec<&String> = state.desktops.iter().filter(|id| state.saved.devices.iter().any(|d| &d.id == *id)).collect();
        ids.sort();
        let text = serde_json::json!({"v": 1, "desktops": ids}).to_string();
        let temporary = path.with_extension("json.tmp");
        let written = path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::write(&temporary, text.as_bytes()));
        if written.and_then(|()| std::fs::rename(&temporary, path)).is_err() {
            let _ = std::fs::remove_file(&temporary);
            eprintln!("mobile remote: the device roles were not saved");
        }
    }

    /// Marks `id` a computer (or not) and keeps it when that changed.
    fn set_desktop(&self, state: &mut State, id: &str, desktop: bool) {
        let changed = if desktop { state.desktops.insert(id.to_string()) } else { state.desktops.remove(id) };
        if changed {
            self.save_roles(state);
        }
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

    /// The saved `PairingKey`: now only the legacy apps' (key-only auth while they are allowed), shown under Settings › 휴대폰 › 고급
    /// (never logged). Devices that register present a pairing code ([`Identity::pairing_code`]) instead.
    pub fn pairing_key(&self) -> Zeroizing<String> {
        Zeroizing::new(self.state().saved.pairing_key.clone())
    }

    /// The live pairing code of `kind`, made when there is none or it ran out, and kept alive for [`PAIRING_CODE_LIFETIME`] from now:
    /// what Settings shows (the QR, the links), so it stays valid while it is on screen (never logged).
    pub fn pairing_code(&self, kind: PairingKind) -> Zeroizing<String> {
        let mut state = self.state();
        let now = Instant::now();
        let expires = now + self.code_lifetime;
        let slot = kind.slot();
        if let Some(code) = state.codes[slot].as_mut().filter(|code| code.expires > now) {
            code.expires = code.expires.max(expires);
            return code.key.clone();
        }
        let serial = state.next_serial;
        state.next_serial += 1;
        let key = Zeroizing::new(random_token());
        state.codes[slot] = Some(Code { key: key.clone(), expires, serial });
        key
    }

    /// How long the live pairing code of `kind` has left; None when there is none (never shown, spent or run out).
    pub fn pairing_code_left(&self, kind: PairingKind) -> Option<Duration> {
        let now = Instant::now();
        self.state().codes[kind.slot()].as_ref().filter(|code| code.expires > now).map(|code| code.expires - now)
    }

    /// Both pairing codes forgotten: whatever was shown pairs nothing; the next draw shows new ones.
    pub fn forget_codes(&self) {
        self.state().codes = [None, None];
    }

    /// `Authenticate(auth, allowLegacy)`: a device token (its device touched and accepted), the legacy apps' key-only auth while
    /// `policy` allows it (against the saved key, as before), or a registration with a pairing code — the phone's for a phone, the
    /// computers' for a computer (refused while `policy` takes no computers) — which comes back [`AuthDecision::Pending`] for the
    /// person at this computer to allow ([`Identity::register`]) or refuse ([`Identity::refuse`]). `Err` when the change it makes (a
    /// `Touch`, the legacy row) could not be saved: C#'s `Save` throws there and the connection ends unanswered.
    pub fn authorize(&self, auth: &AuthRequest, policy: AuthPolicy) -> Result<AuthDecision> {
        use AuthDecision::{Accepted, Pending, Refused};
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
            let presented = token_hash(token, auth.desktop);
            let Some(device) = device.filter(|d| d.token_hash.as_deref().is_some_and(|stored| equal(stored, &presented))) else {
                return Ok(Refused(AuthRefusal::DeviceRevoked));
            };
            self.touch(&mut state, &device.id)?;
            if auth.desktop {
                self.set_desktop(&mut state, &device.id, true);
            }
            return Ok(Accepted { device_id: device.id, device_token: None });
        }
        let Some(presented) = presented.filter(|p| p.encode_utf16().count() <= 256) else {
            return Ok(Refused(AuthRefusal::Malformed));
        };
        // A computer always names itself: it keeps a token, never the legacy row.
        if auth.desktop && id.is_none() {
            return Ok(Refused(AuthRefusal::Malformed));
        }
        let Some(id) = id else {
            // The legacy apps (no client id, so no token): the saved key, while they are allowed, with no code and no approval.
            if !equal(&hash(presented), &hash(&state.saved.pairing_key)) {
                return Ok(Refused(AuthRefusal::PairingKey));
            }
            if !policy.allow_legacy {
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
        // A registration: the live code of its own kind (a phone's link never pairs a computer, nor the other way round).
        let kind = if auth.desktop { PairingKind::Desktop } else { PairingKind::Phone };
        let now_instant = Instant::now();
        let serial = state.codes[kind.slot()]
            .as_ref()
            .filter(|code| code.expires > now_instant && equal(&hash(presented), &hash(&code.key)))
            .map(|code| code.serial);
        let Some(serial) = serial else { return Ok(Refused(AuthRefusal::PairingKey)) };
        if auth.desktop && !policy.allow_desktops {
            return Ok(Refused(AuthRefusal::DesktopsOff));
        }
        let now = now_ms();
        state.registrations.retain(|at| now - at <= HOUR_MS);
        if state.registrations.len() >= REGISTRATIONS_PER_HOUR || room(&state.saved.devices, &id, now).is_none() {
            return Ok(Refused(AuthRefusal::DeviceLimit));
        }
        state.registrations.push(now);
        let given =
            auth.client_name.clone().filter(|n| !n.is_empty()).unwrap_or_else(|| self.locale.get("mobileRemote.device.defaultName"));
        Ok(Pending(PendingRegistration {
            client_id: id.clone(),
            name: wire::clean(&given, 40),
            desktop: auth.desktop,
            replaces: state.saved.devices.iter().any(|d| d.id == id),
            serial,
        }))
    }

    /// A pending registration the person at this computer allowed: the device registered with a new token, as a new device (first
    /// seen now; one paired under the same id before is replaced, its token and push handle gone), and the code it presented spent
    /// (the next draw shows a new one). Refused with `pairing-key` when that code was spent or replaced meanwhile (another device
    /// took it, a new key, a revoke); its time running out while the person decided does not count. `Err` when it could not be saved
    /// (nothing changes, the code stays).
    pub fn register(&self, pending: &PendingRegistration) -> Result<AuthDecision> {
        let mut state = self.state();
        let slot = pending.kind().slot();
        if state.codes[slot].as_ref().map(|code| code.serial) != Some(pending.serial) {
            return Ok(AuthDecision::Refused(AuthRefusal::PairingKey));
        }
        let now = now_ms();
        let Some(mut devices) = room(&state.saved.devices, &pending.client_id, now) else {
            return Ok(AuthDecision::Refused(AuthRefusal::DeviceLimit));
        };
        let fresh = random_token();
        let mark = time::now_o();
        devices.push(PairedDevice {
            id: pending.client_id.clone(),
            name: pending.name.clone(),
            token_hash: Some(token_hash(&fresh, pending.desktop)),
            first_seen: mark.clone(),
            last_seen: mark,
            push: None,
        });
        let next = PairingIdentity { devices, ..state.saved.clone() };
        self.save(&mut state, next)?;
        state.codes[slot] = None;
        let known: HashSet<String> = state.saved.devices.iter().map(|d| d.id.clone()).collect();
        state.desktops.retain(|id| known.contains(id));
        if pending.desktop {
            state.desktops.insert(pending.client_id.clone());
        } else {
            state.desktops.remove(&pending.client_id);
        }
        self.save_roles(&state);
        Ok(AuthDecision::Accepted { device_id: pending.client_id.clone(), device_token: Some(fresh) })
    }

    /// A pending registration refused (by the person, or by its time running out): the code it presented is spent, so whoever holds
    /// it cannot ask again; the next draw shows a new one. Nothing when that code is gone already.
    pub fn refuse(&self, pending: &PendingRegistration) {
        let mut state = self.state();
        let slot = pending.kind().slot();
        if state.codes[slot].as_ref().map(|code| code.serial) == Some(pending.serial) {
            state.codes[slot] = None;
        }
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

    /// `Rotated`: told after every rotation, once the new key is saved, with `None` (every phone, its token and its push handle are
    /// gone), and after a single device's revoke with `Some(id)` for each row it removed (the other devices stay paired). Dropping the
    /// subscription unregisters.
    pub fn on_rotated(self: &Arc<Self>, listener: impl Fn(Option<&str>) + Send + Sync + 'static) -> RotationSubscription {
        let id = self.next_listener.fetch_add(1, Ordering::SeqCst);
        lock(&self.rotated).push((id, Arc::new(listener)));
        RotationSubscription { identity: Arc::downgrade(self), id }
    }

    /// How many rotation listeners are registered (the checks: a disposed hub lets go of its own).
    pub fn rotation_listeners(&self) -> usize {
        lock(&self.rotated).len()
    }

    /// `Rotate` (새 키 만들기): every device dropped and the host made anew but for its id — a new legacy key, a new relay secret
    /// key (so a new public key), a new host token (so a new relay name, `ServerId`) — since every device pairs again anyway; the
    /// pairing codes forgotten; the listeners told. The relay host dials again under the new name ([`crate::relay::host::RelayHost::
    /// rotate`]).
    pub fn rotate(&self) -> Result<()> {
        {
            let mut state = self.state();
            let next = PairingIdentity {
                host_id: state.saved.host_id.clone(),
                host_token: hex(random::<32>().as_ref()),
                pairing_key: random_token(),
                secret_key: *random::<32>(),
                devices: vec![],
            };
            self.save(&mut state, next)?;
            state.registrations.clear();
            state.desktops.clear();
            state.codes = [None, None];
            self.save_roles(&state);
        }
        self.tell(None);
        Ok(())
    }

    /// One device unpaired (the settings row's 연결 해제): its row, token and push handle go, and the legacy key is made anew and the
    /// pairing codes forgotten so nothing it may still hold pairs again; every other device keeps its row and token (they authenticate by token, not by the key) and
    /// stays connected. The legacy row (apps without a device token, which authenticate by the key alone) goes with the old key. The
    /// ids removed (empty when `id` was not paired: nothing changes); `Err` when it could not be saved (nothing changes either).
    pub fn revoke(&self, id: &str) -> Result<Vec<String>> {
        let removed: Vec<String> = {
            let mut state = self.state();
            if !state.saved.devices.iter().any(|d| d.id == id) {
                return Ok(vec![]);
            }
            let (gone, kept): (Vec<PairedDevice>, Vec<PairedDevice>) =
                state.saved.devices.iter().cloned().partition(|d| d.id == id || d.id == LEGACY);
            let next = PairingIdentity { pairing_key: random_token(), devices: kept, ..state.saved.clone() };
            self.save(&mut state, next)?;
            let removed: Vec<String> = gone.into_iter().map(|d| d.id).collect();
            for gone in &removed {
                state.desktops.remove(gone);
            }
            state.codes = [None, None];
            self.save_roles(&state);
            removed
        };
        for gone in &removed {
            self.tell(Some(gone));
        }
        Ok(removed)
    }

    fn tell(&self, device: Option<&str>) {
        let listeners: Vec<Listener> = lock(&self.rotated).iter().map(|(_, l)| l.clone()).collect();
        for listener in listeners {
            listener(device);
        }
    }

    /// `Contains(id, legacyAllowed)`: a paired phone with a token, or the legacy row while legacy phones are allowed.
    pub fn contains(&self, id: &str, legacy_allowed: bool) -> bool {
        self.state().saved.devices.iter().any(|d| d.id == id && (d.token_hash.is_some() || (id == LEGACY && legacy_allowed)))
    }

    /// `PairingUrl(relay, name, direct)`: the phone's v2 pairing link with the phone's live pairing code (kept alive: Settings shows
    /// it); `direct` adds the direct path's endpoints as `direct=`, joined with commas and percent-encoded as the Mac writes it.
    pub fn pairing_url(&self, relay: &str, name: &str, direct: &[String]) -> String {
        let code = self.pairing_code(PairingKind::Phone);
        let mut url = self.link(relay, name, &code);
        if !direct.is_empty() {
            url.push_str("&direct=");
            url.push_str(&escape_data(&direct.join(",")));
        }
        url
    }

    /// The computers' pairing link (원격 연결 › 이 컴퓨터 공유): the phone's shape (an older computer reads it alike) with the
    /// computers' live code (kept alive) and `for=desktop`; no direct endpoints (a computer comes through the relay only).
    pub fn desktop_pairing_url(&self, relay: &str, name: &str) -> String {
        let code = self.pairing_code(PairingKind::Desktop);
        self.link(relay, name, &code) + "&for=desktop"
    }

    fn link(&self, relay: &str, name: &str, code: &str) -> String {
        let state = self.state();
        let public = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(cipher::public_key(&Zeroizing::new(state.saved.secret_key)));
        format!(
            "mightycode://pair?v=2&sid={}&pk={}&relay={}&key={}&name={}",
            hash(&state.saved.host_token),
            escape_data(&public),
            escape_data(relay),
            escape_data(code),
            escape_data(&wire::clean(name, 120))
        )
    }

    /// `Devices`: the paired phones as the settings list draws them (`connected` false: the relay host knows who is connected), the
    /// row that stands for every app without a device token named in the user's language.
    pub fn devices(&self) -> Vec<MobileDeviceInfo> {
        let now = now_ms();
        let state = self.state();
        state
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
                desktop: state.desktops.contains(&d.id),
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

/// `MobileQrSize`: the side the pairing QR is drawn nearest to, in DIPs.
pub const QR_SIZE: f64 = 200.0;

/// The pairing QR (C#'s QRCoder at ECC M, `PngByteQRCode(code).GetGraphic(perModule)`; D13): `link` at error correction M, the quiet
/// zone's four modules a side included, a whole number of pixels per module (at least 2) bringing the side nearest [`QR_SIZE`] at
/// `scale`, black on white, as a PNG; its side in pixels. The mask may differ from QRCoder's; the content is the pairing link.
pub fn pairing_qr(link: &str, scale: f64) -> Option<(Vec<u8>, u32)> {
    use image::ImageEncoder;
    let code = qrcode::QrCode::with_error_correction_level(link.as_bytes(), qrcode::EcLevel::M).ok()?;
    let width = code.width();
    let colors = code.to_colors();
    let modules = width + 8;
    let per_module = ((QR_SIZE * scale.max(0.1) / modules as f64).round() as usize).max(2);
    let side = modules * per_module;
    let mut pixels = vec![255u8; side * side];
    for y in 0..width {
        for x in 0..width {
            if colors[y * width + x] == qrcode::Color::Dark {
                for dy in 0..per_module {
                    let row = (y + 4) * per_module + dy;
                    let start = row * side + (x + 4) * per_module;
                    pixels[start..start + per_module].fill(0);
                }
            }
        }
    }
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png).write_image(&pixels, side as u32, side as u32, image::ExtendedColorType::L8).ok()?;
    Some((png, side as u32))
}
