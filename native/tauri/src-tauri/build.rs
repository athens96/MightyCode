//! Tauri's build step, then W9's release stamp (plan §1.2, D3; as scripts/build-windows.ps1 stamps C#'s assembly metadata):
//! `MIGHTY_RELEASE=1`, `MIGHTY_UPDATE_PUBLIC_KEY` (32 bytes of standard base64) and `MIGHTY_UPDATE_URL` (an https address with a host),
//! read from the environment and handed to `build_info.rs` as `MIGHTY_BUILD_*`. A malformed stamp fails the build, and so does a release
//! stamp without a valid key and address. No stamp is a development build (no key, no address, writes only with `--allow-state-write`).
//! After Step 0 this file is W9-F's.

use base64::Engine;

fn stamp(name: &str) -> Option<String> {
    println!("cargo:rerun-if-env-changed={name}");
    std::env::var(name).ok().map(|value| value.trim().to_string()).filter(|value| !value.is_empty())
}

fn main() {
    let key = stamp("MIGHTY_UPDATE_PUBLIC_KEY");
    if let Some(key) = &key {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(key)
            .unwrap_or_else(|_| panic!("MIGHTY_UPDATE_PUBLIC_KEY is not standard base64"));
        assert!(bytes.len() == 32, "MIGHTY_UPDATE_PUBLIC_KEY must be a raw 32-byte Ed25519 public key ({} bytes)", bytes.len());
    }
    let url = stamp("MIGHTY_UPDATE_URL");
    if let Some(url) = &url {
        let host = url.strip_prefix("https://").map(|rest| rest.split(['/', '?', '#']).next().unwrap_or(""));
        assert!(host.is_some_and(|host| !host.is_empty()), "MIGHTY_UPDATE_URL must be an https address with a host");
    }
    let release = match stamp("MIGHTY_RELEASE").as_deref() {
        None | Some("0") => false,
        Some("1") => true,
        Some(other) => panic!("MIGHTY_RELEASE must be 1 or 0, not {other:?}"),
    };
    assert!(
        !release || (key.is_some() && url.is_some()),
        "a release build (MIGHTY_RELEASE=1) needs MIGHTY_UPDATE_PUBLIC_KEY and MIGHTY_UPDATE_URL"
    );
    println!("cargo:rustc-env=MIGHTY_BUILD_RELEASE={}", if release { "1" } else { "0" });
    println!("cargo:rustc-env=MIGHTY_BUILD_UPDATE_PUBLIC_KEY={}", key.unwrap_or_default());
    println!("cargo:rustc-env=MIGHTY_BUILD_UPDATE_URL={}", url.unwrap_or_default());
    tauri_build::build()
}
