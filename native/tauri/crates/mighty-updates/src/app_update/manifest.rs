//! The signed manifest (C# `AppUpdateManifest.cs`: `AppUpdateAsset`, `AppUpdateManifest`, `AppUpdateAvailability`; plan §1.2). Owner:
//! W9-A.
//!
//! `parse` reads `latest.json`'s envelope (`format`, `payload`, `signature`) with the build's key: over 256 KiB `manifestFileTooLarge`,
//! not a JSON object `manifestFileNotJSON`, no payload or signature `unsigned`, another format `signatureFormat`, a payload or signature
//! that is not a string `signatureEncoding`, a bad signature `signatureMismatch`, then `parse_plain` of the payload. Base64 as .NET reads
//! it (ASCII whitespace skipped, then a strict standard decode); JSON as `JsonDocument`'s defaults (no comments, no trailing commas,
//! depth 64; the duplicate-key winner settled by the parity corpus).

use std::collections::BTreeMap;

use super::UpdateError;

/// The envelope's `format` (C# `AppUpdateManifest.EnvelopeFormat`).
pub const ENVELOPE_FORMAT: &str = "mightyclaude-update-v1";

/// The largest `latest.json` read (C#: 256 KiB, `manifestFileTooLarge`).
pub const MAX_MANIFEST_BYTES: usize = 256 * 1024;

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

/// C# `AppUpdateManifest.Parse(data, publicKey)`. Step 0 stub (W9-A writes it): every manifest is refused as unsigned.
pub fn parse(_data: &[u8], _public_key: &[u8; 32]) -> Result<Manifest, UpdateError> {
    Err(UpdateError::key("appUpdate.error.unsigned"))
}

/// C# `AppUpdateManifest.ParsePlain(payload)`: the payload's fields (version, assets, build, notes, date). Step 0 stub (W9-A writes
/// it): nothing reads.
pub fn parse_plain(_payload: &str) -> Result<Manifest, UpdateError> {
    Err(UpdateError::key("appUpdate.error.manifestFileNotJSON"))
}

/// C# `AppUpdateManifest.Allowed(url)`: an absolute https address with a host. Step 0 stub (W9-A writes it): none is allowed.
pub fn allowed(_url: &str) -> bool {
    false
}
