//! The build's update key and manifest address (C# `AppUpdateBuild`, AppUpdateCoordinator.cs:13-41; plan §1.2). Owner: W9-A.
//!
//! src-tauri's `build.rs` stamps `MIGHTY_UPDATE_PUBLIC_KEY` and `MIGHTY_UPDATE_URL` (as `build-windows.ps1` stamps C#'s assembly
//! metadata) and `build_info.rs` hands the texts here: a key is 32 bytes of standard base64 (anything else reads as absent, Windows rule
//! 1: "no public key"), an address an absolute https one with a host (anything else absent).

/// The build's two stamps as read (C# `AppUpdateBuild.PublicKey`, `ManifestUrl`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Build {
    pub public_key: Option<[u8; 32]>,
    pub manifest_url: Option<String>,
}

impl Build {
    /// Whether this build can verify a manifest (C# `AppUpdate.HasPublicKey`).
    pub fn has_public_key(&self) -> bool {
        self.public_key.is_some()
    }
}

/// C# `AppUpdateBuild.Decode`: a 32-byte raw Ed25519 key in base64 (trimmed, read as .NET reads base64 into its 64-byte buffer);
/// anything else None.
pub fn public_key(text: Option<&str>) -> Option<[u8; 32]> {
    let text = text?.trim();
    if text.is_empty() {
        return None;
    }
    // `TryFromBase64String` into 64 bytes: a longer key does not fit and is refused like any other malformed one.
    let bytes = super::dotnet::base64(text).filter(|bytes| bytes.len() <= 64)?;
    <[u8; 32]>::try_from(bytes.as_slice()).ok()
}

/// C# `AppUpdateBuild.ManifestUrl`: a non-empty allowed address, else None.
pub fn manifest_url(text: Option<&str>) -> Option<String> {
    text.filter(|value| !value.is_empty() && super::manifest::allowed(value)).map(String::from)
}

/// The package this machine needs (C# `AppUpdateBuild.Architecture`): `arm64` on an arm64 process, else `x64`.
pub fn architecture(process_architecture: &str) -> &'static str {
    if process_architecture == "aarch64" { "arm64" } else { "x64" }
}
