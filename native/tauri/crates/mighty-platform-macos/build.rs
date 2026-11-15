//! Links libghostty, the Mac's terminal (plan D1 as the user chose it: libghostty's C API driven from Rust), only for macOS builds
//! with `MIGHTY_MAC_NATIVE=1`, and does nothing elsewhere: until M-D lands, a plain `cargo test` on the Mac needs no download.
//!
//! `MIGHTY_GHOSTTY_XCFRAMEWORK` names the unzipped `GhosttyKit.xcframework` (`scripts/fetch-ghosttykit.sh`, by the URL and checksum
//! of `native/macos/Vendor/libghostty-spm/Package.swift`, the library the Swift app links). With it the crate is built with
//! `cfg(mighty_ghostty)`, which M-D's `terminal_surface` FFI sits behind. The frameworks are the Step 0 spike's
//! (`native/tauri/spikes/mac/ghostty/build.rs`); M-D keeps them in step.

fn main() {
    println!("cargo::rustc-check-cfg=cfg(mighty_ghostty)");
    println!("cargo:rerun-if-env-changed=MIGHTY_MAC_NATIVE");
    println!("cargo:rerun-if-env-changed=MIGHTY_GHOSTTY_XCFRAMEWORK");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") || std::env::var("MIGHTY_MAC_NATIVE").as_deref() != Ok("1") {
        return;
    }
    let xcframework = std::env::var("MIGHTY_GHOSTTY_XCFRAMEWORK")
        .expect("MIGHTY_MAC_NATIVE=1 links libghostty: set MIGHTY_GHOSTTY_XCFRAMEWORK to scripts/fetch-ghosttykit.sh's output");
    let slice = std::path::Path::new(&xcframework).join("macos-arm64_x86_64");
    assert!(slice.join("libghostty.a").is_file(), "no libghostty.a under {}", slice.display());
    println!("cargo:rustc-link-search=native={}", slice.display());
    println!("cargo:rustc-link-lib=static=ghostty");
    println!("cargo:rustc-link-lib=c++");
    for framework in
        ["AppKit", "Carbon", "CoreFoundation", "CoreGraphics", "CoreText", "CoreVideo", "Foundation", "IOSurface", "Metal", "QuartzCore"]
    {
        println!("cargo:rustc-link-lib=framework={framework}");
    }
    println!("cargo:rustc-cfg=mighty_ghostty");
}
