//! The signed manifest (C# `AppUpdateManifest.cs`: `AppUpdateAsset`, `AppUpdateManifest`, `AppUpdateAvailability`; plan §1.2). Owner:
//! W9-A.
//!
//! `parse` reads `latest.json`'s envelope (`format`, `payload`, `signature`) with the build's key: over 256 KiB `manifestFileTooLarge`,
//! not a JSON object `manifestFileNotJSON`, no payload or signature `unsigned`, another format `signatureFormat`, a payload or signature
//! that is a JSON null `signatureEncoding`, a payload that is not base64 `signatureEncoding`, a bad signature `signatureMismatch`, then
//! `parse_plain` of the payload. Base64 as .NET reads it ([`super::dotnet::base64`]); JSON as `JsonDocument`'s defaults
//! ([`super::json`]).
//!
//! Where C# fails with .NET's own exception rather than a locale sentence, the refusal is the same and so is its text when .NET's text
//! is fixed: a `format`, `payload` or `signature` (or an asset's `size`, or `build`) of the wrong JSON kind gives .NET's
//! `InvalidOperationException` text, a signature that is not base64 .NET's `FormatException` text. A document .NET cannot parse
//! (`JsonException`, whose text names a line and byte) reads `manifestFileNotJSON` for the envelope and `manifestNotJSON` for the
//! payload. The parity corpus `native/contracts/fixtures/update-manifests` holds C#'s verdict for each case.

use std::collections::BTreeMap;

use super::UpdateError;
use super::json::Json;

/// The envelope's `format` (C# `AppUpdateManifest.EnvelopeFormat`).
pub const ENVELOPE_FORMAT: &str = "mightyclaude-update-v1";

/// The largest `latest.json` read (C#: 256 KiB, `manifestFileTooLarge`).
pub const MAX_MANIFEST_BYTES: usize = 256 * 1024;

/// .NET's `FormatException` text for a string `Convert.FromBase64String` refuses.
pub const BASE64_FORMAT_ERROR: &str = "The input is not a valid Base-64 string as it contains a non-base 64 character, more than two padding characters, or an illegal character among the padding characters.";

/// The Windows architectures a manifest names, in C#'s order.
pub const ARCHITECTURES: [&str; 2] = ["x64", "arm64"];

/// One Windows package: an https address, its SHA-256 (64 lower-case hex) and its size, all three required (Windows rule 2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    pub url: String,
    pub sha256: String,
    pub size: u64,
}

/// A verified manifest (C# `AppUpdateManifest`): the normalized version, the build number, the notes, the publication date and the
/// Windows assets by architecture (`x64`, `arm64`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub version: String,
    pub build: Option<i32>,
    pub notes: Option<String>,
    pub published_at: Option<String>,
    pub windows: BTreeMap<String, Asset>,
}

impl Manifest {
    /// `Windows.TryGetValue(architecture)` (C#'s dictionary ignores case).
    pub fn asset(&self, architecture: &str) -> Option<&Asset> {
        self.windows.iter().find(|(name, _)| super::dotnet::eq_ignore_case(name, architecture)).map(|(_, asset)| asset)
    }
}

/// A check's answer (C# `AppUpdateAvailability`): this build's version beside the manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Availability {
    pub current: String,
    pub manifest: Manifest,
}

impl Availability {
    /// Whether the manifest's version is newer than this build's (`AppVersion.IsNewer`).
    pub fn is_newer(&self) -> bool {
        super::version::is_newer(&self.manifest.version, &self.current)
    }
}

fn key(name: &'static str) -> UpdateError {
    UpdateError::key(name)
}

/// C# `AppUpdateManifest.Parse(data, publicKey)`.
pub fn parse(data: &[u8], public_key: &[u8; 32]) -> Result<Manifest, UpdateError> {
    if data.len() > MAX_MANIFEST_BYTES {
        return Err(key("appUpdate.error.manifestFileTooLarge"));
    }
    let root = super::json::parse(data).ok_or_else(|| key("appUpdate.error.manifestFileNotJSON"))?;
    if !root.is_object() {
        return Err(key("appUpdate.error.manifestFileNotJSON"));
    }
    let (Some(payload), Some(signature)) = (root.get("payload"), root.get("signature")) else {
        return Err(key("appUpdate.error.unsigned"));
    };
    let format = match root.get("format") {
        Some(format) => format.get_string().map_err(UpdateError::Plain)?,
        None => None,
    };
    if format != Some(ENVELOPE_FORMAT) {
        return Err(key("appUpdate.error.signatureFormat"));
    }
    let payload = payload.get_string().map_err(UpdateError::Plain)?.ok_or_else(|| key("appUpdate.error.signatureEncoding"))?;
    let signature = signature.get_string().map_err(UpdateError::Plain)?.ok_or_else(|| key("appUpdate.error.signatureEncoding"))?;
    let payload = super::dotnet::base64(payload).ok_or_else(|| key("appUpdate.error.signatureEncoding"))?;
    let signature = super::dotnet::base64(signature).ok_or_else(|| UpdateError::Plain(BASE64_FORMAT_ERROR.into()))?;
    if !super::signature::verify(public_key, &payload, &signature) {
        return Err(key("appUpdate.error.signatureMismatch"));
    }
    parse_plain(&payload)
}

/// C# `AppUpdateManifest.ParsePlain(payload)`: the payload's fields (version, assets, build, notes, date).
pub fn parse_plain(payload: &[u8]) -> Result<Manifest, UpdateError> {
    if payload.len() > MAX_MANIFEST_BYTES {
        return Err(key("appUpdate.error.manifestNotJSON"));
    }
    let root = super::json::parse(payload).ok_or_else(|| key("appUpdate.error.manifestNotJSON"))?;
    if !root.is_object() {
        return Err(key("appUpdate.error.manifestNotJSON"));
    }
    let raw_version = root.string("version").or_else(|| root.string("latest")).or_else(|| root.string("latestVersion"));
    let version = raw_version.and_then(super::version::normalized).ok_or_else(|| key("appUpdate.error.noVersion"))?;

    let mut windows = BTreeMap::new();
    if let Some(platform) = root.get("windows").filter(|value| value.is_object()) {
        for architecture in ARCHITECTURES {
            let Some(asset) = platform.get(architecture) else { continue };
            if let Some(asset) = parse_asset(asset).map_err(UpdateError::Plain)? {
                windows.insert(architecture.to_string(), asset);
            }
        }
    }

    // `build`, else `buildNumber`, each an int ≥ 0; one present but not a number throws as .NET's TryGetInt32 does.
    let mut build = None;
    for name in ["build", "buildNumber"] {
        if let Some(value) = root.get(name)
            && let Some(number) = value.try_i32().map_err(UpdateError::Plain)?
            && number >= 0
        {
            build = Some(number);
            break;
        }
    }

    Ok(Manifest {
        version,
        build,
        notes: root.string("notes").or_else(|| root.string("releaseNotes")).or_else(|| root.string("changelog")).map(String::from),
        published_at: root.string("publishedAt").or_else(|| root.string("date")).map(String::from),
        windows,
    })
}

/// C#'s `ParseAsset` (Windows rule 2): an https address, a SHA-256 and a size > 0, or None; Err with .NET's text when `size` is
/// there but not a number (C#'s `TryGetInt64` throws).
fn parse_asset(asset: &Json) -> Result<Option<Asset>, String> {
    if !asset.is_object() {
        return Ok(None);
    }
    let url = ["url", "download", "downloadUrl", "href"].iter().find_map(|name| asset.string(name));
    let Some(url) = url.filter(|url| allowed(url)) else { return Ok(None) };
    let Some(sha256) = asset.string("sha256").and_then(clean_sha256) else { return Ok(None) };
    let Some(size) = asset.get("size") else { return Ok(None) };
    match size.try_i64()? {
        Some(size) if size > 0 => Ok(Some(Asset { url: url.to_string(), sha256, size: size.unsigned_abs() })),
        _ => Ok(None),
    }
}

/// `sha256.ToLowerInvariant().Trim().Replace("sha256:", "")`, kept only as 64 lower-case hex. A text with any character outside
/// ASCII is refused whatever .NET's lower case makes of it: no such character lowers to a hex digit.
fn clean_sha256(text: &str) -> Option<String> {
    let clean = text.to_ascii_lowercase().trim().replace("sha256:", "");
    (clean.len() == 64 && clean.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))).then_some(clean)
}

/// C# `AppUpdateManifest.Allowed(url)`: an absolute https address with a host.
pub fn allowed(url: &str) -> bool {
    super::address::allowed(url)
}
