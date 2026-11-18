//! `<profile>\mobile-remote\mobile-identity.json` as C# reads and writes it (R\MobileIdentity.cs:46-60, 150-166; plan 짠1.2, D14): plain
//! JSON through `Wire.Json` — `{version:1, hostId, hostToken, pairingKey, secretKeyB64, devices:[{id, name, tokenHash, firstSeen,
//! lastSeen, push?:{handle, kinds, updatedAt}}]}` — in an owner-only folder (no DPAPI). Shared and pure, so it is tested on both OSes;
//! the Windows crate does the file's IO and the folder's ACL (mighty-platform-windows `pairing.rs`). Owner: W8-A.
//!
//! Reading follows `JsonSerializer.Deserialize<Saved>(text, Wire.Json)` (web defaults: member names without regard to case, the last
//! of a repeated name winning, a number also read from a string) and then the constructor's checks, each a refusal (`Err`): version 1,
//! the host id an identifier, the host token 64 lower-case hex digits, the pairing key 43-128 base64url characters, a 32-byte secret,
//! at most 32 devices with distinct ids, each a 22-character client id with a token hash or the token-less `legacy` row, a name of at
//! most 160 UTF-16 units and dates that read. A push registration the row cannot use (no token hash, no handle or kinds, a handle
//! that is not 43 base64url characters) is dropped and the phone kept. .NET's `$` also matches before a final line feed, so the
//! patterns written with it (host token, pairing key, client id, token hash) take one too, as C# does.
//!
//! Writing is `JsonSerializer.Serialize(saved, Wire.Json)`: C#'s member order, nulls left out, strings escaped as `Utf8JsonWriter`
//! escapes them (non-ASCII and `" & ' + < > \` `` ` `` as `\uXXXX`).

use base64::Engine;
use serde_json::{Map, Value};

use crate::{PairedDevice, PairingIdentity, PlatformError, PushRegistration, Result};

/// The folder under the profile (C# `MobileIdentity(Path.Combine(dataDir, "mobile-remote"))`).
pub const FOLDER: &str = "mobile-remote";
/// The file in it.
pub const FILE: &str = "mobile-identity.json";
/// A larger file is refused unread (`MobileIdentity.cs:49`).
pub const MAXIMUM_BYTES: u64 = 128 * 1024;
/// At most this many phones (`state.Devices.Count > 32`).
pub const MAXIMUM_DEVICES: usize = 32;
/// The token-less row every app without a device token shares.
pub const LEGACY: &str = "legacy";

fn invalid() -> PlatformError {
    PlatformError::Invalid("Invalid mobile identity.".into())
}

/// The file's text read as C#'s constructor reads it: `Err` for any damage it refuses (the port's contract: the caller decides),
/// a push registration the row cannot use dropped with the phone kept.
pub fn decode(text: &str) -> Result<PairingIdentity> {
    let root: Value = serde_json::from_str(text).map_err(|_| invalid())?;
    let root = root.as_object().ok_or_else(invalid)?;
    let version = number(member(root, "version"))?;
    let host_id = string(member(root, "hostId"))?.ok_or_else(invalid)?;
    let host_token = string(member(root, "hostToken"))?.ok_or_else(invalid)?;
    let pairing_key = string(member(root, "pairingKey"))?.ok_or_else(invalid)?;
    let secret = string(member(root, "secretKeyB64"))?.ok_or_else(invalid)?;
    let devices = match member(root, "devices") {
        Some(Value::Array(items)) => items.iter().map(device).collect::<Result<Vec<_>>>()?,
        _ => return Err(invalid()),
    };
    let secret_key: [u8; 32] =
        base64::engine::general_purpose::STANDARD.decode(secret.trim()).ok().and_then(|bytes| bytes.try_into().ok()).ok_or_else(invalid)?;
    let mut ids: Vec<&str> = devices.iter().map(|d| d.id.as_str()).collect();
    ids.sort_unstable();
    ids.dedup();
    if version != Some(1)
        || !identifier(&host_id)
        || !dollar(&host_token, |t| t.len() == 64 && t.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
        || !token_shape(&pairing_key)
        || devices.len() > MAXIMUM_DEVICES
        || ids.len() != devices.len()
        || devices.iter().any(|d| !device_valid(d))
    {
        return Err(invalid());
    }
    // A handle the row cannot use is dropped, the phone kept (M/MobileDeviceRegistry.swift load()).
    let devices = devices
        .into_iter()
        .map(|mut d| {
            if d.push.as_ref().is_some_and(|push| d.token_hash.is_none() || !valid_handle(&push.handle)) {
                d.push = None;
            }
            d
        })
        .collect();
    Ok(PairingIdentity { host_id, host_token, pairing_key, secret_key, devices })
}

/// The text C#'s `Save` writes for `identity` (`Wire.Json`: camelCase, nulls left out, C#'s member order and escaping).
pub fn encode(identity: &PairingIdentity) -> Result<String> {
    let mut out = String::with_capacity(512 + identity.devices.len() * 256);
    out.push_str("{\"version\":1,\"hostId\":");
    escape_into(&mut out, &identity.host_id);
    out.push_str(",\"hostToken\":");
    escape_into(&mut out, &identity.host_token);
    out.push_str(",\"pairingKey\":");
    escape_into(&mut out, &identity.pairing_key);
    out.push_str(",\"secretKeyB64\":");
    escape_into(&mut out, &base64::engine::general_purpose::STANDARD.encode(identity.secret_key));
    out.push_str(",\"devices\":[");
    for (index, device) in identity.devices.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str("{\"id\":");
        escape_into(&mut out, &device.id);
        out.push_str(",\"name\":");
        escape_into(&mut out, &device.name);
        if let Some(hash) = &device.token_hash {
            out.push_str(",\"tokenHash\":");
            escape_into(&mut out, hash);
        }
        out.push_str(",\"firstSeen\":");
        escape_into(&mut out, &device.first_seen);
        out.push_str(",\"lastSeen\":");
        escape_into(&mut out, &device.last_seen);
        if let Some(push) = &device.push {
            out.push_str(",\"push\":{\"handle\":");
            escape_into(&mut out, &push.handle);
            out.push_str(",\"kinds\":[");
            for (at, kind) in push.kinds.iter().enumerate() {
                if at > 0 {
                    out.push(',');
                }
                escape_into(&mut out, kind);
            }
            out.push_str("],\"updatedAt\":");
            escape_into(&mut out, &push.updated_at);
            out.push('}');
        }
        out.push('}');
    }
    out.push_str("]}");
    Ok(out)
}

/// `MobileIdentity.ClientId`: `^[A-Za-z0-9_-]{22}$` (a final line feed allowed, as .NET's `$` allows it).
pub fn client_id(value: &str) -> bool {
    dollar(value, |v| v.len() == 22 && v.bytes().all(base64url))
}

/// `MobilePushWire.ValidHandle`: `^[A-Za-z0-9_-]{43}\z`.
pub fn valid_handle(value: &str) -> bool {
    value.len() == 43 && value.bytes().all(base64url)
}

/// `MobileIdentity.TokenShape`: `^[A-Za-z0-9_-]{43,128}$`.
pub fn token_shape(value: &str) -> bool {
    dollar(value, |v| (43..=128).contains(&v.len()) && v.bytes().all(base64url))
}

/// A token hash as the file keeps it: `^[a-f0-9]{64}$`.
pub fn token_hash(value: &str) -> bool {
    dollar(value, |v| v.len() == 64 && v.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
}

/// `DateTimeOffset.TryParse` of the dates C# and the Mac write: `yyyy-MM-dd`, optionally `THH:mm`, `:ss`, a fraction, and `Z` or an
/// offset `짹HH:mm`, `짹HHmm` or `짹HH` (a space may stand for the `T`).
pub fn date(value: &str) -> bool {
    let b = value.trim().as_bytes();
    let mut at = 0;
    let digits = |at: &mut usize, count: usize| -> Option<u32> {
        let slice = b.get(*at..*at + count)?;
        slice.iter().all(u8::is_ascii_digit).then(|| {
            *at += count;
            slice.iter().fold(0, |n, d| n * 10 + (d - b'0') as u32)
        })
    };
    let ok = (|| {
        let _year = digits(&mut at, 4)?;
        (b.get(at) == Some(&b'-')).then_some(())?;
        at += 1;
        let month = digits(&mut at, 2)?;
        (b.get(at) == Some(&b'-')).then_some(())?;
        at += 1;
        let day = digits(&mut at, 2)?;
        if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
            return None;
        }
        if at == b.len() {
            return Some(());
        }
        matches!(b.get(at), Some(b'T' | b't' | b' ')).then_some(())?;
        at += 1;
        let hour = digits(&mut at, 2)?;
        (b.get(at) == Some(&b':')).then_some(())?;
        at += 1;
        let minute = digits(&mut at, 2)?;
        if hour > 23 || minute > 59 {
            return None;
        }
        if b.get(at) == Some(&b':') {
            at += 1;
            let second = digits(&mut at, 2)?;
            if second > 59 {
                return None;
            }
            if b.get(at) == Some(&b'.') {
                at += 1;
                let start = at;
                while b.get(at).is_some_and(u8::is_ascii_digit) {
                    at += 1;
                }
                (at > start).then_some(())?;
            }
        }
        match b.get(at) {
            None => Some(()),
            Some(b'Z' | b'z') if at + 1 == b.len() => Some(()),
            Some(b'+' | b'-') => {
                at += 1;
                let hours = digits(&mut at, 2)?;
                if b.get(at) == Some(&b':') {
                    at += 1;
                }
                if at < b.len() {
                    digits(&mut at, 2)?;
                }
                (hours <= 14 && at == b.len()).then_some(())
            }
            _ => None,
        }
    })();
    ok.is_some()
}

fn base64url(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

/// A pattern ending in .NET's `$`: the whole text, or the text before one final line feed.
fn dollar(value: &str, rule: impl Fn(&str) -> bool) -> bool {
    rule(value) || value.strip_suffix('\n').is_some_and(rule)
}

/// `Wire.Identifier`: `^[a-zA-Z0-9][a-zA-Z0-9._:-]{0,127}\z`.
fn identifier(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 128
        && bytes[0].is_ascii_alphanumeric()
        && bytes[1..].iter().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'-'))
}

fn device_valid(d: &PairedDevice) -> bool {
    let legacy = d.id == LEGACY;
    (legacy || client_id(&d.id))
        && (!legacy || d.token_hash.is_none())
        && (legacy || d.token_hash.as_deref().is_some_and(token_hash))
        && d.name.encode_utf16().count() <= 160
        && date(&d.first_seen)
        && date(&d.last_seen)
}

/// A record member by name without regard to case (the web defaults), the last of a repeated name winning.
fn member<'a>(object: &'a Map<String, Value>, name: &str) -> Option<&'a Value> {
    object.iter().filter(|(key, _)| key.eq_ignore_ascii_case(name)).map(|(_, value)| value).next_back()
}

/// A string member: absent or null is None; any other kind of value is the serializer's refusal.
fn string(value: Option<&Value>) -> Result<Option<String>> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.clone())),
        Some(_) => Err(invalid()),
    }
}

/// An `int` member, also read from a string (`JsonNumberHandling.AllowReadingFromString`); absent is None (C#'s default 0).
fn number(value: Option<&Value>) -> Result<Option<i64>> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => n.as_i64().filter(|v| i32::try_from(*v).is_ok()).map(Some).ok_or_else(invalid),
        Some(Value::String(text)) => text.parse::<i32>().map(|v| Some(v as i64)).map_err(|_| invalid()),
        Some(_) => Err(invalid()),
    }
}

fn device(value: &Value) -> Result<PairedDevice> {
    let o = value.as_object().ok_or_else(invalid)?;
    let push = match member(o, "push") {
        None | Some(Value::Null) => None,
        Some(Value::Object(p)) => {
            let handle = string(member(p, "handle"))?;
            let kinds = match member(p, "kinds") {
                None | Some(Value::Null) => None,
                Some(Value::Array(items)) => {
                    Some(items.iter().map(|item| item.as_str().map(str::to_string).ok_or_else(invalid)).collect::<Result<Vec<_>>>()?)
                }
                Some(_) => return Err(invalid()),
            };
            let updated_at = string(member(p, "updatedAt"))?;
            // No handle or no kinds: the registration is dropped below, the phone kept (an empty handle never validates).
            match (handle, kinds) {
                (Some(handle), Some(kinds)) => Some(PushRegistration { handle, kinds, updated_at: updated_at.unwrap_or_default() }),
                _ => Some(PushRegistration { handle: String::new(), kinds: vec![], updated_at: String::new() }),
            }
        }
        Some(_) => return Err(invalid()),
    };
    Ok(PairedDevice {
        id: string(member(o, "id"))?.ok_or_else(invalid)?,
        name: string(member(o, "name"))?.ok_or_else(invalid)?,
        token_hash: string(member(o, "tokenHash"))?,
        first_seen: string(member(o, "firstSeen"))?.ok_or_else(invalid)?,
        last_seen: string(member(o, "lastSeen"))?.ok_or_else(invalid)?,
        push,
    })
}

/// Whether `Utf8JsonWriter` (`JavaScriptEncoder.Default`) keeps an ASCII character as it is.
fn allowed(c: u8) -> bool {
    (0x20..0x7f).contains(&c) && !matches!(c, b'"' | b'&' | b'\'' | b'+' | b'<' | b'>' | b'\\' | b'`')
}

/// A string as JSON text (quotes included), escaped as `Utf8JsonWriter` escapes it (mighty-core's `json::escape_into`; this crate
/// depends on nothing of the app's).
fn escape_into(out: &mut String, value: &str) {
    out.push('"');
    for c in value.chars() {
        let code = c as u32;
        if code < 0x80 && allowed(code as u8) {
            out.push(c);
            continue;
        }
        match c {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            _ => {
                let mut units = [0u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    out.push_str(&format!("\\u{unit:04X}"));
                }
            }
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOST_TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const TOKEN_HASH: &str = "5723c1b7256eba050781d42f7d4fd96201c4b75d7673b1a82ffbcba6dc2e1c23";
    const KEY: &str = "AQgPFh0kKzI5QEdOVVxjanF4f4aNlJuiqbC3vsXM09o";

    fn sample() -> PairingIdentity {
        PairingIdentity {
            host_id: "4e0c0a3f-8a59-4a0f-9a57-0a3c1b2d3e4f".into(),
            host_token: HOST_TOKEN.into(),
            pairing_key: KEY.into(),
            secret_key: [7; 32],
            devices: vec![
                PairedDevice {
                    id: LEGACY.into(),
                    name: "Legacy app".into(),
                    token_hash: None,
                    first_seen: "2026-10-23T01:02:03.0000000+00:00".into(),
                    last_seen: "2026-10-23T01:02:03.0000000+00:00".into(),
                    push: None,
                },
                PairedDevice {
                    id: "AAECAwQFBgcICQoLDA0ODw".into(),
                    name: "민수의 <iPhone> & \"Pro\"".into(),
                    token_hash: Some(TOKEN_HASH.into()),
                    first_seen: "2026-10-23T01:02:03.0000000+00:00".into(),
                    last_seen: "2026-10-24T05:06:07.1234567+00:00".into(),
                    push: Some(PushRegistration {
                        handle: "__79_Pv6-fj39vX08_Lx8O_u7ezr6uno5-bl5OPi4eA".into(),
                        kinds: vec!["completed".into(), "permission".into()],
                        updated_at: "2026-10-24T05:06:07.1234567+00:00".into(),
                    }),
                },
            ],
        }
    }

    /// The bytes `JsonSerializer.Serialize(saved, Wire.Json)` writes (member order, nulls left out, `Utf8JsonWriter`'s escapes), and
    /// they read back to the same identity.
    #[test]
    fn the_file_is_written_as_csharp_writes_it_and_reads_back() {
        let text = encode(&sample()).unwrap();
        // `~` stands for a backslash in the expected text.
        let expected = concat!(
            r#"{"version":1,"hostId":"4e0c0a3f-8a59-4a0f-9a57-0a3c1b2d3e4f","hostToken":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef","#,
            r#""pairingKey":"AQgPFh0kKzI5QEdOVVxjanF4f4aNlJuiqbC3vsXM09o","secretKeyB64":"BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=","devices":["#,
            r#"{"id":"legacy","name":"Legacy app","firstSeen":"2026-10-23T01:02:03.0000000~u002B00:00","lastSeen":"2026-10-23T01:02:03.0000000~u002B00:00"},"#,
            r#"{"id":"AAECAwQFBgcICQoLDA0ODw","name":"~uBBFC~uC218~uC758 ~u003CiPhone~u003E ~u0026 ~u0022Pro~u0022","#,
            r#""tokenHash":"5723c1b7256eba050781d42f7d4fd96201c4b75d7673b1a82ffbcba6dc2e1c23","firstSeen":"2026-10-23T01:02:03.0000000~u002B00:00","#,
            r#""lastSeen":"2026-10-24T05:06:07.1234567~u002B00:00","push":{"handle":"__79_Pv6-fj39vX08_Lx8O_u7ezr6uno5-bl5OPi4eA","#,
            r#""kinds":["completed","permission"],"updatedAt":"2026-10-24T05:06:07.1234567~u002B00:00"}}]}"#
        );
        assert_eq!(text, expected.replace('~', "\\"));
        assert_eq!(decode(&text).unwrap(), sample());
        assert!(!format!("{:?}", sample()).contains(HOST_TOKEN) && !format!("{:?}", sample()).contains(KEY), "secrets stay out of Debug");
    }

    /// The web defaults: member names without regard to case, a version given as a string, unknown members ignored.
    #[test]
    fn the_web_defaults_read_what_csharp_reads() {
        let text = format!(
            r#"{{"Version":"1","HOSTID":"h-1","hostToken":"{HOST_TOKEN}","pairingKey":"{KEY}","secretKeyB64":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=","devices":[],"later":true}}"#
        );
        let read = decode(&text).unwrap();
        assert_eq!((read.host_id.as_str(), read.devices.len()), ("h-1", 0));
    }

    /// Every damage C#'s constructor refuses is an `Err`; a push row the phone cannot use is dropped with the phone kept.
    #[test]
    fn damage_is_refused_and_a_bad_push_row_is_dropped() {
        let good = encode(&sample()).unwrap();
        let swap = |from: &str, to: &str| good.replacen(from, to, 1);
        let damaged = [
            "not json".to_string(),
            "[]".to_string(),
            swap("\"version\":1", "\"version\":2"),
            swap("\"version\":1,", ""),
            swap("4e0c0a3f-8a59-4a0f-9a57-0a3c1b2d3e4f", "-bad"),
            swap(HOST_TOKEN, &HOST_TOKEN.to_uppercase()),
            swap(KEY, "short"),
            swap("BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=", "BwcH"),
            swap("AAECAwQFBgcICQoLDA0ODw", "legacy"),
            swap("AAECAwQFBgcICQoLDA0ODw", "too-short"),
            swap(
                "{\"id\":\"legacy\",\"name\":\"Legacy app\",",
                &format!("{{\"id\":\"legacy\",\"name\":\"Legacy app\",\"tokenHash\":\"{TOKEN_HASH}\","),
            ),
            swap(&format!(",\"tokenHash\":\"{TOKEN_HASH}\""), ""),
            swap("Legacy app", &"x".repeat(161)),
            swap("2026-10-24T05:06:07.1234567\\u002B00:00\",\"push\"", "not a date\",\"push\""),
            swap("\"devices\":[", "\"devices\":null,\"x\":["),
            swap("\"name\":\"Legacy app\"", "\"name\":7"),
        ];
        for text in damaged {
            assert!(decode(&text).is_err(), "refused: {text}");
        }
        let many = PairingIdentity {
            devices: (0..33)
                .map(|n| PairedDevice {
                    id: format!("{n:a>22}"),
                    name: "p".into(),
                    token_hash: Some(TOKEN_HASH.into()),
                    first_seen: "2026-10-23T00:00:00Z".into(),
                    last_seen: "2026-10-23T00:00:00Z".into(),
                    push: None,
                })
                .collect(),
            ..sample()
        };
        assert!(decode(&encode(&many).unwrap()).is_err(), "33 devices");
        assert!(
            decode(&swap("\"id\":\"legacy\"", "\"id\":\"AAECAwQFBgcICQoLDA0ODw\"").replacen(
                "\"name\":\"Legacy app\"",
                &format!("\"name\":\"Legacy app\",\"tokenHash\":\"{TOKEN_HASH}\""),
                1
            ))
            .is_err()
        );
        let bad_handle = decode(&swap("__79_Pv6-fj39vX08_Lx8O_u7ezr6uno5-bl5OPi4eA", "not a handle")).unwrap();
        assert!(bad_handle.devices[1].push.is_none() && bad_handle.devices.len() == 2, "a malformed handle is dropped, the phone kept");
        let no_kinds = decode(&swap(",\"kinds\":[\"completed\",\"permission\"]", "")).unwrap();
        assert!(no_kinds.devices[1].push.is_none(), "a registration without kinds is dropped");
        assert!(decode(&swap(HOST_TOKEN, &format!("{HOST_TOKEN}\\n"))).is_ok(), ".NET's $ takes a final line feed");
    }

    #[test]
    fn dates_read_as_the_apps_write_them() {
        for good in
            ["2026-10-23", "2026-10-23T01:02", "2026-10-23T01:02:03Z", "2026-10-23T01:02:03.1234567+09:00", "2026-10-23 01:02:03-0500"]
        {
            assert!(date(good), "{good}");
        }
        for bad in ["", "not a date", "2026-13-01", "2026-10-23T25:00", "2026-10-23T01:02:03+", "2026-10-23Tx"] {
            assert!(!date(bad), "{bad}");
        }
    }
}
