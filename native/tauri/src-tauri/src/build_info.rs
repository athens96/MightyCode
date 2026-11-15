//! What this build was stamped with (build.rs; plan §1.2, D3): whether it is a release build, and the update's public key and manifest
//! address (C# `AppUpdateBuild`'s assembly metadata). A development build has neither: no update check at all (Windows rule 1), and
//! writes only with `--allow-state-write`. Owner: Step 0, then W9-F.

/// `MIGHTY_RELEASE=1` at build time (build.rs refuses it without a valid key and address).
pub const RELEASE: bool = matches!(env!("MIGHTY_BUILD_RELEASE").as_bytes(), b"1");

/// The stamped update key's text (standard base64 of 32 bytes; build.rs checked it), empty without one.
const PUBLIC_KEY_TEXT: &str = env!("MIGHTY_BUILD_UPDATE_PUBLIC_KEY");

/// The stamped manifest address (an https address with a host; build.rs checked it), empty without one.
const MANIFEST_URL_TEXT: &str = env!("MIGHTY_BUILD_UPDATE_URL");

/// The update's public key (C# `AppUpdateBuild.PublicKey`), None in a build without one.
#[allow(dead_code, reason = "app_update.rs (W9-C) hands it to mighty-updates' coordinator")]
pub fn public_key() -> Option<[u8; 32]> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD.decode(PUBLIC_KEY_TEXT).ok()?;
    bytes.try_into().ok()
}

/// The built-in manifest address (C# `AppUpdateBuild.ManifestUrl`), None in a build without one.
#[allow(dead_code, reason = "app_update.rs (W9-C) hands it to mighty-updates' coordinator")]
pub fn manifest_url() -> Option<&'static str> {
    (!MANIFEST_URL_TEXT.is_empty()).then_some(MANIFEST_URL_TEXT)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A build without a stamp (every test build, the CI's debug build) is a development build: no key, no address.
    #[test]
    fn an_unstamped_build_has_no_key_and_no_address() {
        if PUBLIC_KEY_TEXT.is_empty() {
            assert_eq!(public_key(), None);
        } else {
            assert!(public_key().is_some(), "build.rs let only a 32-byte key through");
        }
        assert_eq!(manifest_url().is_some(), !MANIFEST_URL_TEXT.is_empty());
        assert!(!RELEASE || (public_key().is_some() && manifest_url().is_some()), "a release build carries both");
    }
}
