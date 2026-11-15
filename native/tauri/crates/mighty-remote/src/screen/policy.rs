//! Screen share's records and policy (C# R\ScreenSharePolicy.cs; plan §1.8): the displays, the zoom region, the quality per network,
//! a device's grant as `screen-devices.json` keeps it, the session limits (1 control, 2 view), the watchers' timeouts, the capture
//! restarts, and the P-256 control key (its shape, fingerprint and the DER signature over the one-use challenge). Owner: W8-F.

use base64::Engine as _;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
use chrono::{DateTime, TimeDelta, Utc};
use mighty_core::json::{self, JsonOut, R, Wire};
use p256::ecdsa::signature::Verifier as _;
use p256::ecdsa::{DerSignature, VerifyingKey};
use serde_json::Value;
use sha2::{Digest as _, Sha256};

/// C# `ScreenDisplay(DisplayId, Width, Height, Main, Left, Top)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenDisplay {
    pub display_id: i32,
    pub width: i32,
    pub height: i32,
    pub main: bool,
    pub left: i32,
    pub top: i32,
}

impl ScreenDisplay {
    /// `new ScreenDisplay(id, width, height, main)` (left and top 0).
    pub fn new(display_id: i32, width: i32, height: i32, main: bool) -> ScreenDisplay {
        ScreenDisplay { display_id, width, height, main, left: 0, top: 0 }
    }
}

/// C# `ScreenRegion(X, Y, Width, Height)`: the part of a display a phone zoomed to, in fractions of it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenRegion {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Default for ScreenRegion {
    fn default() -> ScreenRegion {
        ScreenRegion { x: 0.0, y: 0.0, width: 1.0, height: 1.0 }
    }
}

impl ScreenRegion {
    /// `ScreenRegion.Parse`: an object of four finite numbers, each side at least 2 %, clipped to the display (still 2 % a side).
    pub fn parse(value: &Value) -> Option<ScreenRegion> {
        if !value.is_object() {
            return None;
        }
        let (x, y, w, h) = (number(value, "x")?, number(value, "y")?, number(value, "width")?, number(value, "height")?);
        if w < 0.02 || h < 0.02 {
            return None;
        }
        let left = x.clamp(0.0, 1.0);
        let top = y.clamp(0.0, 1.0);
        let width = f64::min(1.0, x + w) - left;
        let height = f64::min(1.0, y + h) - top;
        (width >= 0.02 && height >= 0.02).then_some(ScreenRegion { x: left, y: top, width, height })
    }
}

/// `ScreenRegion.Number`: a finite number property (`TryGetDouble` on a number token).
pub(crate) fn number(value: &Value, key: &str) -> Option<f64> {
    json::number(value, key)
}

/// C# `ScreenQuality(Width, Height, Fps, MaxBitrateKbps)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenQuality {
    pub width: i32,
    pub height: i32,
    pub fps: i32,
    pub max_bitrate_kbps: i32,
}

impl ScreenQuality {
    /// `ScreenQuality.For(cellular, relay)`.
    pub fn for_network(cellular: bool, relay: bool) -> ScreenQuality {
        if cellular {
            ScreenQuality { width: 1280, height: 720, fps: 15, max_bitrate_kbps: 1000 }
        } else {
            ScreenQuality { width: 1920, height: 1080, fps: 30, max_bitrate_kbps: if relay { 2000 } else { 6000 } }
        }
    }

    pub(crate) fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.prop_int("width", self.width.into());
        out.prop_int("height", self.height.into());
        out.prop_int("fps", self.fps.into());
        out.prop_int("maxBitrateKbps", self.max_bitrate_kbps.into());
        out.end_object();
    }
}

/// C# `ScreenDeviceGrant(DeviceId, Allowed, Grant, ControlKeyPublic)`: one phone's row of `screen-devices.json`. The key is public.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenDeviceGrant {
    pub device_id: String,
    pub allowed: bool,
    /// `none`, `view` or `control`.
    pub grant: String,
    /// The enrolled control key: base64 of the 65-byte uncompressed P-256 point.
    pub control_key_public: Option<String>,
}

impl ScreenDeviceGrant {
    /// `new ScreenDeviceGrant(deviceId)`: nothing allowed.
    pub fn new(device_id: &str) -> ScreenDeviceGrant {
        ScreenDeviceGrant { device_id: device_id.into(), allowed: false, grant: "none".into(), control_key_public: None }
    }

    pub fn with(device_id: &str, allowed: bool, grant: &str) -> ScreenDeviceGrant {
        ScreenDeviceGrant { device_id: device_id.into(), allowed, grant: grant.into(), control_key_public: None }
    }
}

/// The read keeps C#'s nullable reference fields: a row whose `deviceId` or `grant` reads as null is dropped by the load's filter.
pub(crate) struct StoredGrant {
    pub device_id: Option<String>,
    pub allowed: bool,
    pub grant: Option<String>,
    pub control_key_public: Option<String>,
}

impl Wire for StoredGrant {
    fn read(value: &Value) -> R<StoredGrant> {
        let o = json::Obj::of(value, "ScreenDeviceGrant")?;
        Ok(StoredGrant {
            device_id: json::string(o.get("deviceId"), || None)?,
            allowed: json::boolean(o.get("allowed"), false)?,
            grant: json::string(o.get("grant"), || Some("none".into()))?,
            control_key_public: json::opt_string(o.get("controlKeyPublic"))?,
        })
    }

    fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.prop_str("deviceId", self.device_id.as_deref());
        out.prop_bool("allowed", self.allowed);
        out.prop_str("grant", self.grant.as_deref());
        out.prop_str("controlKeyPublic", self.control_key_public.as_deref());
        out.end_object();
    }
}

impl From<&ScreenDeviceGrant> for StoredGrant {
    fn from(grant: &ScreenDeviceGrant) -> StoredGrant {
        StoredGrant {
            device_id: Some(grant.device_id.clone()),
            allowed: grant.allowed,
            grant: Some(grant.grant.clone()),
            control_key_public: grant.control_key_public.clone(),
        }
    }
}

/// C# `ScreenLiveSession(SessionId, DeviceId, Mode, DisplayId, Codec, Quality, StartedAt)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenLiveSession {
    pub session_id: String,
    pub device_id: String,
    /// `view` or `control`.
    pub mode: String,
    pub display_id: i32,
    pub codec: String,
    pub quality: ScreenQuality,
    pub started_at: DateTime<Utc>,
}

/// `ScreenSharePolicy.ControlIdleSeconds`: a control session with no input ends after this.
pub const CONTROL_IDLE_SECONDS: i64 = 600;
/// `ScreenSharePolicy.ViewIdleSeconds`.
pub const VIEW_IDLE_SECONDS: i64 = 1800;
/// `ScreenSharePolicy.LocalInputPauseSeconds`: the PC's own keyboard or mouse pauses remote control this long.
pub const LOCAL_INPUT_PAUSE_SECONDS: i64 = 2;
/// `ScreenSharePolicy.BackgroundSeconds`: a phone in the background (or a peer that never connected) ends after this.
pub const BACKGROUND_SECONDS: i64 = 30;
/// `ScreenSharePolicy.CaptureRestarts`: capture that ended on its own is started again at most this often within the window; past
/// that the sessions end as `display-gone`.
pub const CAPTURE_RESTARTS: usize = 3;
/// `ScreenSharePolicy.CaptureRestartWindow`.
pub const CAPTURE_RESTART_WINDOW: TimeDelta = TimeDelta::seconds(10);

/// `ScreenSharePolicy.GrantAllows`: control includes viewing; nothing without `allowed`.
pub fn grant_allows(grant: Option<&ScreenDeviceGrant>, mode: &str) -> bool {
    grant.is_some_and(|g| g.allowed && matches!(mode, "view" | "control") && (g.grant == "control" || mode == "view" && g.grant == "view"))
}

/// `ScreenSharePolicy.DisplayFor`: the display a phone means, or the main display when that one is not attached (any longer), or the
/// first — never a stop. None only with no display at all.
pub fn display_for(displays: &[ScreenDisplay], display_id: i32) -> Option<ScreenDisplay> {
    displays.iter().find(|d| d.display_id == display_id).or_else(|| displays.iter().find(|d| d.main)).or_else(|| displays.first()).copied()
}

/// `ScreenSharePolicy.MayRestartCapture`: true, and counted in `recent`, when capture may be started again now.
pub fn may_restart_capture(recent: &mut Vec<DateTime<Utc>>, now: DateTime<Utc>) -> bool {
    recent.retain(|at| now - *at < CAPTURE_RESTART_WINDOW);
    if recent.len() >= CAPTURE_RESTARTS {
        return false;
    }
    recent.push(now);
    true
}

/// `ScreenSharePolicy.CanJoin`: one control session and two view sessions at most.
pub fn can_join<'a>(mode: &str, sessions: impl IntoIterator<Item = &'a ScreenLiveSession>) -> bool {
    let sessions: Vec<_> = sessions.into_iter().collect();
    match mode {
        "control" => sessions.iter().filter(|s| s.mode == "control").count() < 1,
        "view" => sessions.iter().filter(|s| s.mode == "view").count() < 2,
        _ => false,
    }
}

/// `ScreenSharePolicy.Fingerprint`: the key's SHA-256, its first 8 bytes in upper-case hex as four groups of four.
pub fn fingerprint(key: &[u8]) -> String {
    let hash = Sha256::digest(key);
    let hex: String = hash[..8].iter().map(|b| format!("{b:02X}")).collect();
    (0..4).map(|i| &hex[i * 4..i * 4 + 4]).collect::<Vec<_>>().join("-")
}

/// `ScreenSharePolicy.ValidKey`: an uncompressed P-256 point (65 bytes, `04 ‖ X ‖ Y`) on the curve.
pub fn valid_key(key: &[u8]) -> bool {
    key.len() == 65 && key[0] == 4 && VerifyingKey::from_sec1_bytes(key).is_ok()
}

/// `ScreenSharePolicy.Verify`: an RFC 3279 DER ECDSA signature over SHA-256 of `challenge`, as the phones' Secure Enclave and Keystore
/// sign it.
pub fn verify(challenge: &[u8], signature: &[u8], public_key: &[u8]) -> bool {
    if !valid_key(public_key) || !(8..=80).contains(&signature.len()) {
        return false;
    }
    let (Ok(key), Ok(signature)) = (VerifyingKey::from_sec1_bytes(public_key), DerSignature::from_bytes(signature)) else {
        return false;
    };
    key.verify(challenge, &signature).is_ok()
}

/// `Convert.FromBase64String`: the standard alphabet, padding required (the length a multiple of 4 once white space is gone), unused
/// trailing bits ignored; space, tab, CR and LF are skipped.
pub(crate) fn from_base64(text: &str) -> Option<Vec<u8>> {
    const DOTNET: GeneralPurpose = GeneralPurpose::new(
        &base64::alphabet::STANDARD,
        GeneralPurposeConfig::new().with_decode_allow_trailing_bits(true).with_decode_padding_mode(DecodePaddingMode::RequireCanonical),
    );
    let compact: Vec<u8> = text.bytes().filter(|b| !matches!(b, b' ' | b'\t' | b'\r' | b'\n')).collect();
    DOTNET.decode(compact).ok()
}

/// `Convert.ToBase64String`.
pub(crate) fn to_base64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// `MobileIdentity.ClientId`: `^[A-Za-z0-9_-]{22}$` (a final line feed allowed, as .NET's `$` allows it).
pub fn client_id(value: &str) -> bool {
    id_chars(value, 22, 22)
}

/// `^[A-Za-z0-9_-]{min,max}$` as .NET matches it: one final line feed is allowed by its `$`.
pub(crate) fn id_chars(value: &str, min: usize, max: usize) -> bool {
    let value = value.strip_suffix('\n').unwrap_or(value);
    (min..=max).contains(&value.len()) && value.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_reads_as_dotnet_reads_it() {
        assert_eq!(from_base64("YQ=="), Some(b"a".to_vec()));
        assert_eq!(from_base64(" Y Q\r\n= ="), Some(b"a".to_vec()));
        assert_eq!(from_base64("YQ"), None, "padding required");
        assert_eq!(from_base64("malformed"), None);
        assert_eq!(from_base64(""), Some(vec![]));
    }

    #[test]
    fn ids_match_as_dotnet_regexes_match() {
        assert!(client_id("abcdefghijklmnopqrstuv") && client_id("abcdefghijklmnopqrstuv\n"));
        assert!(!client_id("abcdefghijklmnopqrstu") && !client_id("abcdefghijklmnopqrstuv\n\n") && !client_id("abcdefghijklmnopqrst.v"));
    }

    #[test]
    fn a_fingerprint_is_four_groups_of_upper_case_hex() {
        let print = fingerprint(b"key");
        assert_eq!(print.len(), 19);
        assert!(print.split('-').all(|g| g.len() == 4 && g.bytes().all(|b| b.is_ascii_digit() || b.is_ascii_uppercase())));
    }
}
