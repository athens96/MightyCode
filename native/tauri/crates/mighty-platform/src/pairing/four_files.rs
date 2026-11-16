//! The Mac's pairing layout ([`crate::PairingLayout::FourFiles`]): the phone's pairing identity in the Swift app's files under
//! `<data>/mobile-remote/` (folder 0700, files 0600), read and written in Swift's formats so a phone paired with 0.4.x keeps working
//! after the update, and the files this writes still read in the Swift app (`tools/parity-mac pairing read`). Shared code, no `cfg`:
//! chosen by the profile and tested on both OSes; the Mac's owner-only modes come in through [`PrivateFiles`] (mighty-platform-macos
//! `pairing.rs`). Owner: M-B.
//!
//! | file | Swift | what |
//! |---|---|---|
//! | `host-id` | `MobileRemoteService.stableHostId` (:349-356) | the host's id, plain text (a `CoreValidation.identifier`) |
//! | `mobile-remote.key` | `loadOrCreateKey`, `rotateKey` (:393-442) | the pairing key, plain text, `^[a-zA-Z0-9_-]{43,128}$` |
//! | `relay-host-token.json` | `loadOrCreateControlToken` (:368-389) | the relay host token, plain hex text (`^[A-Fa-f0-9]{32,64}$`) despite the name |
//! | `relay-keypair.json` | `RelayKeypair.load` (RelayChannel.swift:86-104) | `{"publicKeyB64","secretKeyB64","v":1}`, sorted keys, `JSONSerialization`'s `\/` |
//! | `devices.json` | `MobileDeviceRegistry` (:428-486) | `[MobileDevice{id, name, tokenHash?, firstSeen, lastSeen, push?}]`, `JSONEncoder`'s bytes |
//!
//! Reading is Swift's, which mends rather than refuses (the port's `Err` is left for what the disk refuses): a damaged key is a
//! rotation (a new key, the phones' list emptied), a damaged host id, token or key pair is made anew, an unreadable phone list
//! starts empty and its bytes are kept aside as `devices.json.corrupt-<unix seconds>` before the first write over it
//! (`MobileRemoteExtensionTests.swift:1229`). Without the pairing key nothing was ever paired: `Ok(None)`, and the caller makes the
//! identity. What a load mends is written at once, as Swift makes each file the first time it is needed.
//!
//! A save writes only the files whose bytes change, each atomically (a temporary beside it, moved over it); when one of them fails,
//! the ones already written are put back as they were, so a failed rotation never leaves a new key with the old phones' list.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use base64::Engine;
use serde_json::Value;

use crate::{PairedDevice, PairingIdentity, PairingStore, PlatformError, PushRegistration, Result};

/// The folder under the data folder.
pub const FOLDER: &str = "mobile-remote";
/// The host's id: plain text.
pub const HOST_ID: &str = "host-id";
/// The pairing key: base64url text, mode 0600.
pub const PAIRING_KEY: &str = "mobile-remote.key";
/// The relay host token: plain hex text (`^[A-Fa-f0-9]{32,64}$`) despite the name.
pub const HOST_TOKEN: &str = "relay-host-token.json";
/// The relay key pair: `{"v":1,"publicKeyB64","secretKeyB64"}` with sorted keys.
pub const KEY_PAIR: &str = "relay-keypair.json";
/// The paired phones: an array of `MobileDevice{id, name, tokenHash?, firstSeen, lastSeen, push?}`.
pub const DEVICES: &str = "devices.json";
/// `CLIAccountSupport.boundedData(url, maximumBytes: 256 * 1024)`: a larger phone list is not read (and not kept aside).
pub const DEVICES_MAXIMUM_BYTES: u64 = 256 * 1024;
/// A larger host id, key, token or key pair file is damaged (Swift reads them whole; none of them is near this).
pub const FILE_MAXIMUM_BYTES: u64 = 64 * 1024;
/// `MobileDeviceRegistry.maximum`.
pub const MAXIMUM_DEVICES: usize = 32;
/// `MobileDeviceRegistry.legacyId`.
pub const LEGACY: &str = "legacy";

/// How a store keeps its folder and files to their owner: the Mac's 0700 and 0600; [`PlainFiles`] where modes mean nothing (tests on
/// Windows).
pub trait PrivateFiles: Send + Sync {
    /// The folder, made when missing, this user's only.
    fn private_folder(&self, folder: &Path) -> std::io::Result<()>;
    /// A new file nobody else may read (it must not exist yet).
    fn private_file(&self, path: &Path) -> std::io::Result<fs::File>;
    /// An existing folder or file made this user's only again (Swift sets 0600 on each file it reads: `MobileRemoteService.swift:372,
    /// :396`, `RelayChannel.swift:92`). Never follows a link.
    fn tighten(&self, _path: &Path) -> std::io::Result<()> {
        Ok(())
    }
}

/// No modes: the folder made and files created new, nothing more.
pub struct PlainFiles;

impl PrivateFiles for PlainFiles {
    fn private_folder(&self, folder: &Path) -> std::io::Result<()> {
        fs::create_dir_all(folder)
    }
    fn private_file(&self, path: &Path) -> std::io::Result<fs::File> {
        fs::OpenOptions::new().write(true).create_new(true).open(path)
    }
}

/// The phone pairing identity in the Swift app's files.
pub struct FourFiles {
    folder: PathBuf,
    files: Box<dyn PrivateFiles>,
    /// The unreadable phone list's bytes, kept aside before the next write over it.
    corrupt: Mutex<Option<Vec<u8>>>,
}

impl std::fmt::Debug for FourFiles {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FourFiles").field("folder", &self.folder).finish_non_exhaustive()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn random<const N: usize>() -> [u8; N] {
    let mut bytes = [0u8; N];
    getrandom::fill(&mut bytes).expect("the OS random source");
    bytes
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `UUID().uuidString.lowercased()`: a version 4 UUID.
fn new_host_id() -> String {
    let mut b = random::<16>();
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h = hex(&b);
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

/// `MobilePairing.generateKey()`: 32 random bytes, base64url without padding.
fn new_pairing_key() -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(random::<32>())
}

/// `CoreValidation.identifier`: `^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$`.
pub fn identifier(value: &str) -> bool {
    let b = value.as_bytes();
    !b.is_empty()
        && b.len() <= 128
        && b[0].is_ascii_alphanumeric()
        && b[1..].iter().all(|c| c.is_ascii_alphanumeric() || b"._:-".contains(c))
}

/// `RemoteValidation.token`: `^[a-zA-Z0-9_-]{43,128}$`.
pub fn pairing_key(value: &str) -> bool {
    (43..=128).contains(&value.len()) && value.bytes().all(base64url)
}

/// `loadOrCreateControlToken`'s check: `^[A-Fa-f0-9]{32,64}$`.
pub fn host_token(value: &str) -> bool {
    (32..=64).contains(&value.len()) && value.bytes().all(|b| b.is_ascii_hexdigit())
}

/// `MobileDeviceRegistry.validClientId`: `^[A-Za-z0-9_-]{16,64}$`.
pub fn client_id(value: &str) -> bool {
    (16..=64).contains(&value.len()) && value.bytes().all(base64url)
}

/// `MobilePushWire.validHandle`: 43 base64url characters.
pub fn valid_handle(value: &str) -> bool {
    value.len() == 43 && value.bytes().all(base64url)
}

/// A stored token hash: `^[0-9a-f]{64}$`.
fn token_hash(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn base64url(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

/// Swift's `trimmingCharacters(in: .whitespacesAndNewlines)` for the plain-text files.
fn trimmed(text: &str) -> &str {
    text.trim_matches(|c: char| c.is_whitespace())
}

/// The relay key pair file's secret key, as `RelayKeypair.load` takes it: an object with `v` the number 1 and `secretKeyB64` the
/// standard base64 of 32 bytes.
pub fn decode_key_pair(bytes: &[u8]) -> Option<[u8; 32]> {
    let root: Value = serde_json::from_slice(bytes).ok()?;
    let object = root.as_object()?;
    let version = object.get("v")?;
    if !(version.as_i64() == Some(1) || version.as_f64() == Some(1.0)) {
        return None;
    }
    let secret = object.get("secretKeyB64")?.as_str()?;
    base64::engine::general_purpose::STANDARD.decode(secret).ok()?.try_into().ok()
}

/// The text `JSONSerialization.data(withJSONObject: ["v": 1, "publicKeyB64": …, "secretKeyB64": …], options: [.sortedKeys])` writes:
/// sorted keys, and `/` escaped as `\/` (base64 holds `/`).
pub fn encode_key_pair(secret: &[u8; 32]) -> String {
    let b64 = base64::engine::general_purpose::STANDARD;
    let public = x25519_dalek::PublicKey::from(&x25519_dalek::StaticSecret::from(*secret)).to_bytes();
    let mut out = String::from("{\"publicKeyB64\":");
    swift_string(&mut out, &b64.encode(public));
    out.push_str(",\"secretKeyB64\":");
    swift_string(&mut out, &b64.encode(secret));
    out.push_str(",\"v\":1}");
    out
}

/// A string as Foundation's JSON writers write it by default: `"` and `\` escaped, `/` as `\/`, the short forms of `\b \f \n \r \t`,
/// other control characters as `\u00xx`, everything else (non-ASCII too) as it is.
fn swift_string(out: &mut String, value: &str) {
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '/' => out.push_str("\\/"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// A UTC time as the Swift registry writes it (`ISO8601DateFormatter` with fractional seconds: `2026-11-01T05:06:07.123Z`), from the
/// shared identity's .NET `"O"` text (`2026-11-01T05:06:07.1234567+00:00`), so the Swift app reads it if the user goes back to it. A
/// text of any other shape (another offset, Swift's own) is kept as it is.
pub fn swift_timestamp(value: &str) -> String {
    let b = value.as_bytes();
    let shaped = b.len() >= 19
        && b[..19].iter().enumerate().all(|(i, c)| match i {
            4 | 7 => *c == b'-',
            10 => *c == b'T',
            13 | 16 => *c == b':',
            _ => c.is_ascii_digit(),
        });
    if !shaped {
        return value.to_string();
    }
    let rest = &value[19..];
    let (fraction, zone) = match rest.strip_prefix('.') {
        Some(after) => {
            let digits = after.bytes().take_while(u8::is_ascii_digit).count();
            (&after[..digits], &after[digits..])
        }
        None => ("", rest),
    };
    if !matches!(zone, "+00:00" | "-00:00" | "Z") || (fraction.is_empty() && zone == "Z") {
        return value.to_string();
    }
    let millis: String = fraction.chars().chain(std::iter::repeat('0')).take(3).collect();
    format!("{}.{millis}Z", &value[..19])
}

/// The phone list as `JSONEncoder().encode([MobileDevice])` writes it: compact, each row's keys in their declaration order, a nil
/// optional left out, Foundation's escaping, the times in the registry's own form ([`swift_timestamp`]).
pub fn encode_devices(devices: &[PairedDevice]) -> String {
    let mut out = String::from("[");
    for (index, device) in devices.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str("{\"id\":");
        swift_string(&mut out, &device.id);
        out.push_str(",\"name\":");
        swift_string(&mut out, &device.name);
        if let Some(hash) = &device.token_hash {
            out.push_str(",\"tokenHash\":");
            swift_string(&mut out, hash);
        }
        out.push_str(",\"firstSeen\":");
        swift_string(&mut out, &swift_timestamp(&device.first_seen));
        out.push_str(",\"lastSeen\":");
        swift_string(&mut out, &swift_timestamp(&device.last_seen));
        if let Some(push) = &device.push {
            out.push_str(",\"push\":{\"handle\":");
            swift_string(&mut out, &push.handle);
            out.push_str(",\"kinds\":[");
            for (at, kind) in push.kinds.iter().enumerate() {
                if at > 0 {
                    out.push(',');
                }
                swift_string(&mut out, kind);
            }
            out.push_str("],\"updatedAt\":");
            swift_string(&mut out, &swift_timestamp(&push.updated_at));
            out.push('}');
        }
        out.push('}');
    }
    out.push(']');
    out
}

/// `JSONDecoder().decode([MobileDevice].self, …)` and then `MobileDeviceRegistry.load()`'s filters: None when the text does not
/// decode (a wrong type anywhere, a required key missing), else the rows kept: an id that is `legacy` or a client id, the first of a
/// repeated id, a token hash of 64 lower-case hex digits (none only on the legacy row), a push registration the row can use, at most
/// 32.
pub fn decode_devices(bytes: &[u8]) -> Option<Vec<PairedDevice>> {
    let root: Value = serde_json::from_slice(bytes).ok()?;
    let mut rows = Vec::new();
    for item in root.as_array()? {
        let o = item.as_object()?;
        let text = |name: &str| o.get(name).and_then(Value::as_str).map(str::to_string);
        let optional = |name: &str| match o.get(name) {
            None | Some(Value::Null) => Some(None),
            Some(Value::String(text)) => Some(Some(text.clone())),
            Some(_) => None,
        };
        let push = match o.get("push") {
            None | Some(Value::Null) => None,
            Some(Value::Object(p)) => {
                let handle = p.get("handle")?.as_str()?.to_string();
                let kinds = p.get("kinds")?.as_array()?.iter().map(|k| k.as_str().map(str::to_string)).collect::<Option<Vec<_>>>()?;
                let updated_at = p.get("updatedAt")?.as_str()?.to_string();
                Some(PushRegistration { handle, kinds, updated_at })
            }
            Some(_) => return None,
        };
        rows.push(PairedDevice {
            id: text("id")?,
            name: text("name")?,
            token_hash: optional("tokenHash")?,
            first_seen: text("firstSeen")?,
            last_seen: text("lastSeen")?,
            push,
        });
    }
    let mut seen = std::collections::HashSet::new();
    let mut kept: Vec<PairedDevice> = rows
        .into_iter()
        .filter(|d| (d.id == LEGACY || client_id(&d.id)) && seen.insert(d.id.clone()))
        .filter(|d| match &d.token_hash {
            Some(hash) => token_hash(hash),
            None => d.id == LEGACY,
        })
        .map(|mut d| {
            if d.push.as_ref().is_some_and(|p| d.token_hash.is_none() || !valid_handle(&p.handle)) {
                d.push = None;
            }
            d
        })
        .collect();
    kept.truncate(MAXIMUM_DEVICES);
    Some(kept)
}

/// A file's bytes, None when it is missing; larger than `limit` is `Ok(Some(None))`.
fn read_bounded(path: &Path, limit: u64) -> Result<Option<Option<Vec<u8>>>> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    Ok(Some((bytes.len() as u64 <= limit).then_some(bytes)))
}

/// A plain-text file's trimmed text when it passes `valid`.
fn read_text(path: &Path, valid: impl Fn(&str) -> bool) -> Result<Option<String>> {
    Ok(read_bounded(path, FILE_MAXIMUM_BYTES)?
        .flatten()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .map(|text| trimmed(&text).to_string())
        .filter(|text| valid(text)))
}

fn unix_seconds() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl FourFiles {
    /// The files in `folder` (the profile's `mobile-remote`).
    pub fn new(folder: PathBuf, files: Box<dyn PrivateFiles>) -> FourFiles {
        FourFiles { folder, files, corrupt: Mutex::new(None) }
    }

    pub fn folder(&self) -> &Path {
        &self.folder
    }

    /// The files' contents for `identity`, in the order a save writes them (the key first, the phones' list last).
    fn contents(identity: &PairingIdentity) -> [(&'static str, Vec<u8>); 5] {
        [
            (PAIRING_KEY, identity.pairing_key.clone().into_bytes()),
            (HOST_ID, identity.host_id.clone().into_bytes()),
            (HOST_TOKEN, identity.host_token.clone().into_bytes()),
            (KEY_PAIR, encode_key_pair(&identity.secret_key).into_bytes()),
            (DEVICES, encode_devices(&identity.devices).into_bytes()),
        ]
    }

    /// One file replaced atomically: `<name>.<random>` created new and private, written, flushed, moved over it.
    fn write_file(&self, name: &str, bytes: &[u8]) -> std::io::Result<()> {
        let path = self.folder.join(name);
        let temporary = self.folder.join(format!("{name}.{}", hex(&random::<16>())));
        let written = (|| {
            let mut file = self.files.private_file(&temporary)?;
            file.write_all(bytes)?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temporary, &path)
        })();
        if written.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        written
    }

    /// The named files written together: each that changes, in order; a failure puts back the ones already written.
    fn write_all(&self, files: &[(&'static str, Vec<u8>)]) -> Result<()> {
        // A link where the folder should be is refused before anything is made or changed through it.
        match fs::symlink_metadata(&self.folder) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(PlatformError::Refused("Refusing linked mobile remote directory.".into()));
            }
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error.into()),
            _ => {}
        }
        self.files.private_folder(&self.folder)?;
        // What each file held, read before anything is written: only a missing file is "nothing" (a rollback removes it); a file that
        // cannot be read stops the save before it writes, so a rollback never removes what it could not keep.
        let mut before: Vec<Option<Vec<u8>>> = Vec::with_capacity(files.len());
        for (name, _) in files {
            match fs::read(self.folder.join(name)) {
                Ok(bytes) => before.push(Some(bytes)),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => before.push(None),
                Err(error) => return Err(error.into()),
            }
        }
        let mut done: Vec<(&'static str, Option<Vec<u8>>)> = Vec::new();
        for ((name, bytes), before) in files.iter().zip(before) {
            if before.as_deref() == Some(bytes.as_slice()) {
                continue;
            }
            if *name == DEVICES {
                self.keep_corrupt_aside();
            }
            if let Err(error) = self.write_file(name, bytes) {
                for (name, before) in done.into_iter().rev() {
                    let _ = match before {
                        Some(bytes) => self.write_file(name, &bytes),
                        None => fs::remove_file(self.folder.join(name)),
                    };
                }
                return Err(error.into());
            }
            done.push((name, before));
        }
        Ok(())
    }

    /// `backupCorruptFile`: the unreadable list's bytes copied aside, private, before the first write over it; kept for the next
    /// write when the copy fails.
    fn keep_corrupt_aside(&self) {
        let mut corrupt = lock(&self.corrupt);
        let Some(old) = corrupt.as_ref() else { return };
        let path = self.folder.join(format!("{DEVICES}.corrupt-{}", unix_seconds()));
        let kept = self.files.private_file(&path).and_then(|mut file| file.write_all(old).and_then(|()| file.sync_all()));
        if kept.is_ok() {
            *corrupt = None;
        } else {
            let _ = fs::remove_file(&path);
        }
    }
}
impl PairingStore for FourFiles {
    fn load(&self) -> Result<Option<PairingIdentity>> {
        match fs::symlink_metadata(&self.folder) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(PlatformError::Refused("Refusing linked mobile remote directory.".into()));
            }
            Ok(_) => {}
        }
        let key_path = self.folder.join(PAIRING_KEY);
        if read_bounded(&key_path, FILE_MAXIMUM_BYTES)?.is_none() {
            return Ok(None);
        }
        // Swift makes each file it reads owner-only again (and the folder, when it writes the key).
        let _ = self.files.tighten(&self.folder);
        for name in [HOST_ID, PAIRING_KEY, HOST_TOKEN, KEY_PAIR, DEVICES] {
            let path = self.folder.join(name);
            if fs::symlink_metadata(&path).is_ok_and(|meta| meta.is_file()) {
                let _ = self.files.tighten(&path);
            }
        }
        let mut mended: Vec<&'static str> = Vec::new();
        let mut devices = match read_bounded(&self.folder.join(DEVICES), DEVICES_MAXIMUM_BYTES)? {
            None | Some(None) => vec![],
            Some(Some(bytes)) => match decode_devices(&bytes) {
                Some(devices) => devices,
                None => {
                    *lock(&self.corrupt) = Some(bytes);
                    vec![]
                }
            },
        };
        let pairing_key = match read_text(&key_path, pairing_key)? {
            Some(key) => key,
            None => {
                // `rotateKey` over an unreadable key file: every token issued under the old key goes.
                mended.extend([PAIRING_KEY, DEVICES]);
                devices.clear();
                new_pairing_key()
            }
        };
        let host_id = read_text(&self.folder.join(HOST_ID), identifier)?.unwrap_or_else(|| {
            mended.push(HOST_ID);
            new_host_id()
        });
        let host_token = read_text(&self.folder.join(HOST_TOKEN), host_token)?.unwrap_or_else(|| {
            mended.push(HOST_TOKEN);
            hex(&random::<32>())
        });
        let secret_key = read_bounded(&self.folder.join(KEY_PAIR), FILE_MAXIMUM_BYTES)?
            .flatten()
            .and_then(|b| decode_key_pair(&b))
            .unwrap_or_else(|| {
                mended.push(KEY_PAIR);
                random::<32>()
            });
        let identity = PairingIdentity { host_id, host_token, pairing_key, secret_key, devices };
        if !mended.is_empty() {
            let files: Vec<(&'static str, Vec<u8>)> =
                Self::contents(&identity).into_iter().filter(|(name, _)| mended.contains(name)).collect();
            self.write_all(&files)?;
        }
        Ok(Some(identity))
    }

    fn save(&self, identity: &PairingIdentity) -> Result<()> {
        self.write_all(&Self::contents(identity))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "AQgPFh0kKzI5QEdOVVxjanF4f4aNlJuiqbC3vsXM09o";
    const HASH: &str = "5723c1b7256eba050781d42f7d4fd96201c4b75d7673b1a82ffbcba6dc2e1c23";
    const HANDLE: &str = "__79_Pv6-fj39vX08_Lx8O_u7ezr6uno5-bl5OPi4eA";

    fn folder(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        std::env::temp_dir().join(format!("mighty-four-files-{name}-{}-{nanos}", std::process::id())).join(FOLDER)
    }

    fn store(folder: &Path) -> FourFiles {
        FourFiles::new(folder.to_path_buf(), Box::new(PlainFiles))
    }

    fn identity() -> PairingIdentity {
        PairingIdentity {
            host_id: "4e0c0a3f-8a59-4a0f-9a57-0a3c1b2d3e4f".into(),
            host_token: "0123456789abcdef".repeat(4),
            pairing_key: KEY.into(),
            secret_key: [7; 32],
            devices: vec![
                PairedDevice {
                    id: LEGACY.into(),
                    name: "Legacy app".into(),
                    token_hash: None,
                    first_seen: "2026-10-23T01:02:03.000Z".into(),
                    last_seen: "2026-10-23T01:02:03.000Z".into(),
                    push: None,
                },
                PairedDevice {
                    id: "cGhvbmUtb25lLTAwMDAwMDA".into(),
                    name: "민수의 \"iPhone\" a/b".into(),
                    token_hash: Some(HASH.into()),
                    first_seen: "2026-10-23T01:02:03.000Z".into(),
                    last_seen: "2026-10-24T05:06:07.123Z".into(),
                    push: Some(PushRegistration {
                        handle: HANDLE.into(),
                        kinds: vec!["completed".into(), "permission".into()],
                        updated_at: "2026-10-24T05:06:07.123Z".into(),
                    }),
                },
            ],
        }
    }

    /// `JSONEncoder`'s bytes for the phone list (declaration order, nil left out, `\/`, raw UTF-8) and `JSONSerialization`'s sorted,
    /// slash-escaped key pair, with the public key X25519's of the secret.
    #[test]
    fn the_files_are_written_in_swifts_formats() {
        let devices = encode_devices(&identity().devices);
        assert_eq!(
            devices,
            concat!(
                r#"[{"id":"legacy","name":"Legacy app","firstSeen":"2026-10-23T01:02:03.000Z","lastSeen":"2026-10-23T01:02:03.000Z"},"#,
                r#"{"id":"cGhvbmUtb25lLTAwMDAwMDA","name":"민수의 \"iPhone\" a\/b","tokenHash":"5723c1b7256eba050781d42f7d4fd96201c4b75d7673b1a82ffbcba6dc2e1c23","#,
                r#""firstSeen":"2026-10-23T01:02:03.000Z","lastSeen":"2026-10-24T05:06:07.123Z","push":{"handle":"__79_Pv6-fj39vX08_Lx8O_u7ezr6uno5-bl5OPi4eA","#,
                r#""kinds":["completed","permission"],"updatedAt":"2026-10-24T05:06:07.123Z"}}]"#
            )
        );
        assert_eq!(decode_devices(devices.as_bytes()).unwrap(), identity().devices);
        // RFC 7748's first test vector's secret: the public key is X25519's.
        let secret: [u8; 32] = hex_bytes("a546e36bf0527c9d3b16154b82465edd62144c0ac1fc5a18506a2244ba449ac4");
        let pair = encode_key_pair(&secret);
        assert!(pair.starts_with("{\"publicKeyB64\":\"") && pair.ends_with("\",\"v\":1}") && pair.contains(",\"secretKeyB64\":\""));
        assert_eq!(decode_key_pair(pair.as_bytes()), Some(secret));
        let public = x25519_dalek::PublicKey::from(&x25519_dalek::StaticSecret::from(secret)).to_bytes();
        assert!(pair.contains(&base64::engine::general_purpose::STANDARD.encode(public).replace('/', "\\/")));
        assert_eq!(decode_key_pair(b"{\"v\":2,\"secretKeyB64\":\"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=\"}"), None);
    }

    fn hex_bytes<const N: usize>(text: &str) -> [u8; N] {
        let mut out = [0u8; N];
        for (i, byte) in out.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).unwrap();
        }
        out
    }

    /// `MobileDeviceRegistry.load()`'s rules: a wrong type is an unreadable list; bad ids, repeated ids (the first wins), a bad hash, a
    /// token-less row that is not the legacy one are dropped; a push the row cannot use is dropped with the phone kept.
    #[test]
    fn the_phone_list_reads_as_swift_reads_it() {
        assert_eq!(decode_devices(b"{ not json"), None);
        assert_eq!(decode_devices(br#"[{"id":"legacy","name":7,"firstSeen":"a","lastSeen":"b"}]"#), None);
        assert_eq!(decode_devices(br#"[{"id":"legacy","firstSeen":"a","lastSeen":"b"}]"#), None);
        let text = format!(
            r#"[{{"id":"../etc/passwd/xx","name":"a","tokenHash":"{HASH}","firstSeen":"a","lastSeen":"b"}},
               {{"id":"Zm9vYmFyYmF6cXV4MDA","name":"first","tokenHash":"{HASH}","firstSeen":"a","lastSeen":"b","push":{{"handle":"short","kinds":[],"updatedAt":"c"}}}},
               {{"id":"Zm9vYmFyYmF6cXV4MDA","name":"second","tokenHash":"{HASH}","firstSeen":"a","lastSeen":"b"}},
               {{"id":"b3RoZXItcGhvbmUtMDAwMA","name":"upper","tokenHash":"{}","firstSeen":"a","lastSeen":"b"}},
               {{"id":"bm8tdG9rZW4tcGhvbmUtMDA","name":"none","firstSeen":"a","lastSeen":"b"}},
               {{"id":"legacy","name":"L","tokenHash":null,"firstSeen":"a","lastSeen":"b","push":{{"handle":"{HANDLE}","kinds":["completed"],"updatedAt":"c"}}}}]"#,
            HASH.to_uppercase()
        );
        let read = decode_devices(text.as_bytes()).unwrap();
        assert_eq!(read.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(), ["first", "L"]);
        assert!(read.iter().all(|d| d.push.is_none()), "a short handle, and any handle on the legacy row, are dropped");
    }

    /// The shared identity's .NET times are written in the registry's own form; Swift's own and other shapes are kept.
    #[test]
    fn times_are_written_as_the_swift_registry_writes_them() {
        assert_eq!(swift_timestamp("2026-11-01T05:06:07.1234567+00:00"), "2026-11-01T05:06:07.123Z");
        assert_eq!(swift_timestamp("2026-11-01T05:06:07+00:00"), "2026-11-01T05:06:07.000Z");
        assert_eq!(swift_timestamp("2026-11-01T05:06:07.1Z"), "2026-11-01T05:06:07.100Z");
        for kept in ["2026-11-01T05:06:07Z", "2026-11-01T05:06:07.123Z", "2026-11-01T05:06:07.1234567+09:00", "not a time", ""] {
            assert_eq!(swift_timestamp(kept), kept);
        }
    }

    /// Nothing is made before there is a key; a save writes Swift's files, only those that change, and they read back.
    #[test]
    fn the_identity_round_trips_through_the_four_files() {
        let folder = folder("roundtrip");
        let store = store(&folder);
        assert!(store.load().unwrap().is_none() && !folder.exists(), "a load makes nothing");
        store.save(&identity()).unwrap();
        assert_eq!(store.load().unwrap(), Some(identity()));
        assert_eq!(fs::read_to_string(folder.join(PAIRING_KEY)).unwrap(), KEY);
        assert_eq!(fs::read_to_string(folder.join(HOST_TOKEN)).unwrap(), "0123456789abcdef".repeat(4));
        assert_eq!(fs::read_to_string(folder.join(HOST_ID)).unwrap(), "4e0c0a3f-8a59-4a0f-9a57-0a3c1b2d3e4f");
        assert_eq!(fs::read_to_string(folder.join(DEVICES)).unwrap(), encode_devices(&identity().devices));
        let mut names: Vec<String> =
            fs::read_dir(&folder).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        assert_eq!(names, [DEVICES, HOST_ID, PAIRING_KEY, HOST_TOKEN, KEY_PAIR], "no temporary file is left");
        // Swift's own key pair bytes stay as they are when the secret does not change.
        let swift_pair = r#"{"publicKeyB64":"x","secretKeyB64":"BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=","v":1}"#;
        fs::write(folder.join(KEY_PAIR), swift_pair).unwrap();
        let mut touched = identity();
        touched.devices[1].last_seen = "2026-11-01T00:00:00.000Z".into();
        store.save(&touched).unwrap();
        assert_eq!(fs::read_to_string(folder.join(KEY_PAIR)).unwrap(), encode_key_pair(&[7; 32]), "the key pair is Swift's text");
        assert_eq!(store.load().unwrap(), Some(touched));
        let _ = fs::remove_dir_all(folder.parent().unwrap());
    }

    /// Swift mends what it cannot read: a damaged token, host id or key pair is made anew and kept, a damaged key is a rotation that
    /// empties the list; a key written by Swift with a final line feed reads trimmed.
    #[test]
    fn damaged_files_are_mended_as_swift_mends_them() {
        let folder = folder("mended");
        let store = store(&folder);
        store.save(&identity()).unwrap();
        fs::write(folder.join(HOST_TOKEN), "not hex").unwrap();
        fs::write(folder.join(HOST_ID), "-bad").unwrap();
        fs::write(folder.join(KEY_PAIR), "garbage").unwrap();
        fs::write(folder.join(PAIRING_KEY), format!("{KEY}\n")).unwrap();
        let mended = store.load().unwrap().unwrap();
        assert_eq!((mended.pairing_key.as_str(), mended.devices.len()), (KEY, 2));
        assert!(host_token(&mended.host_token) && identifier(&mended.host_id) && mended.secret_key != [7; 32]);
        assert_eq!(store.load().unwrap(), Some(mended.clone()), "what was mended was written");
        fs::write(folder.join(PAIRING_KEY), "short").unwrap();
        let rotated = store.load().unwrap().unwrap();
        assert!(pairing_key(&rotated.pairing_key) && rotated.pairing_key != KEY && rotated.devices.is_empty());
        assert_eq!(fs::read_to_string(folder.join(DEVICES)).unwrap(), "[]");
        let _ = fs::remove_dir_all(folder.parent().unwrap());
    }

    /// `MobileRemoteExtensionTests.swift:1229 "anUnreadableListStartsOverAndKeepsTheOldBytesAside"`: an unreadable `devices.json` reads
    /// as no phones, and its bytes are copied aside before the first write over it.
    #[test]
    fn an_unreadable_list_starts_over_and_keeps_the_old_bytes_aside() {
        let folder = folder("corrupt");
        let store = store(&folder);
        store.save(&identity()).unwrap();
        fs::write(folder.join(DEVICES), "{ not json").unwrap();
        let read = store.load().unwrap().unwrap();
        assert!(read.devices.is_empty());
        let mut next = read.clone();
        next.devices.push(identity().devices[1].clone());
        store.save(&next).unwrap();
        let kept: Vec<String> = fs::read_dir(&folder)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".corrupt-"))
            .collect();
        assert_eq!(kept.len(), 1);
        assert_eq!(fs::read_to_string(folder.join(&kept[0])).unwrap(), "{ not json");
        assert_eq!(store.load().unwrap().unwrap().devices.len(), 1);
        let _ = fs::remove_dir_all(folder.parent().unwrap());
    }

    /// A save that fails part way puts back what it had written: here the phones' list cannot be replaced (a folder stands in its
    /// place), so the rotated key is taken back too.
    #[test]
    fn a_save_that_fails_part_way_leaves_the_files_as_they_were() {
        let folder = folder("partial");
        let store = store(&folder);
        store.save(&identity()).unwrap();
        fs::remove_file(folder.join(DEVICES)).unwrap();
        fs::create_dir(folder.join(DEVICES)).unwrap();
        fs::write(folder.join(DEVICES).join("x"), "x").unwrap();
        let rotated = PairingIdentity { pairing_key: "B".repeat(43), devices: vec![], ..identity() };
        assert!(store.save(&rotated).is_err());
        assert_eq!(fs::read_to_string(folder.join(PAIRING_KEY)).unwrap(), KEY, "the key is put back");
        let _ = fs::remove_dir_all(folder.parent().unwrap());
    }
}
