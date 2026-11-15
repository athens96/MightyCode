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

/// C# `AppUpdateBuild.Decode`: a 32-byte raw Ed25519 key in base64 (trimmed); anything else None. Step 0 stub (W9-A writes it): no key.
pub fn public_key(_text: Option<&str>) -> Option<[u8; 32]> {
    None
}

/// C# `AppUpdateBuild.ManifestUrl`: a non-empty allowed address, else None. Step 0 stub (W9-A writes it): no address.
pub fn manifest_url(_text: Option<&str>) -> Option<String> {
    None
}

/// The package this machine needs (C# `AppUpdateBuild.Architecture`): `arm64` on an arm64 process, else `x64`.
pub fn architecture(process_architecture: &str) -> &'static str {
    if process_architecture == "aarch64" { "arm64" } else { "x64" }
}
