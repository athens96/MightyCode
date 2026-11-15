//! Links libghostty for the C-API route, and with `MIGHTY_SPIKE_SWIFT=1` also builds and links the Swift bridge (the fallback route).
//!
//! `MIGHTY_GHOSTTY_XCFRAMEWORK` names the unzipped `GhosttyKit.xcframework` (scripts/fetch-ghosttykit.sh): the same prebuilt
//! static library the Swift app links through SwiftPM (native/macos/Vendor/libghostty-spm/Package.swift, by URL and checksum).

use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo::rustc-check-cfg=cfg(spike_swift)");
    println!("cargo:rerun-if-env-changed=MIGHTY_GHOSTTY_XCFRAMEWORK");
    println!("cargo:rerun-if-env-changed=MIGHTY_SPIKE_SWIFT");
    tauri_build::build();
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    // A cargo check without the framework (the development PC) type-checks only; a build needs it to link.
    let Ok(xcframework) = std::env::var("MIGHTY_GHOSTTY_XCFRAMEWORK") else {
        println!("cargo:warning=MIGHTY_GHOSTTY_XCFRAMEWORK is not set: libghostty is not linked (type-check only)");
        return;
    };
    let slice = PathBuf::from(&xcframework).join("macos-arm64_x86_64");
    assert!(slice.join("libghostty.a").is_file(), "no libghostty.a under {}", slice.display());
    println!("cargo:rustc-link-search=native={}", slice.display());
    println!("cargo:rustc-link-lib=static=ghostty");
    println!("cargo:rustc-link-lib=c++");
    for framework in [
        "AppKit",
        "Carbon",
        "CoreFoundation",
        "CoreGraphics",
        "CoreText",
        "CoreVideo",
        "Foundation",
        "IOSurface",
        "Metal",
        "QuartzCore",
        "UniformTypeIdentifiers",
    ] {
        println!("cargo:rustc-link-lib=framework={framework}");
    }

    if std::env::var("MIGHTY_SPIKE_SWIFT").as_deref() == Ok("1") {
        swift_bridge();
    }
}

/// `swift build` of `swift/` (the static library `MightyGhosttySpike` over the vendored GhosttyTerminal), linked with the Swift
/// runtime of the selected Xcode.
fn swift_bridge() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let package = manifest.join("swift");
    let scratch = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("swift-build");
    println!("cargo:rerun-if-changed={}", package.join("Package.swift").display());
    println!("cargo:rerun-if-changed={}", package.join("Sources").display());
    let run = |args: &[&str]| -> String {
        let output = Command::new("swift").args(args).output().expect("swift runs");
        if !output.status.success() {
            panic!("swift {args:?} failed:\n{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        }
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    };
    let common = ["--package-path", package.to_str().unwrap(), "--scratch-path", scratch.to_str().unwrap(), "-c", "release"];
    let mut build = vec!["build", "--product", "MightyGhosttySpike"];
    build.extend_from_slice(&common);
    run(&build);
    let mut show = vec!["build", "--show-bin-path"];
    show.extend_from_slice(&common);
    let bin = run(&show);
    println!("cargo:rustc-link-search=native={bin}");
    println!("cargo:rustc-link-lib=static=MightyGhosttySpike");
    // The Swift runtime: the OS's (/usr/lib/swift) at run time, the toolchain's compatibility libraries at link time.
    let toolchain = Command::new("xcrun").args(["--find", "swift"]).output().expect("xcrun runs");
    let swift = PathBuf::from(String::from_utf8_lossy(&toolchain.stdout).trim().to_string());
    let lib = swift.parent().and_then(|p| p.parent()).map(|usr| usr.join("lib/swift/macosx")).expect("the toolchain's lib");
    let sdk = Command::new("xcrun").args(["--show-sdk-path"]).output().expect("xcrun runs");
    let sdk = String::from_utf8_lossy(&sdk.stdout).trim().to_string();
    println!("cargo:rustc-link-search=native={}", lib.display());
    println!("cargo:rustc-link-search=native={sdk}/usr/lib/swift");
    println!("cargo:rustc-link-search=native=/usr/lib/swift");
    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    for framework in ["SwiftUI", "Combine", "MetalKit", "GameController"] {
        println!("cargo:rustc-link-lib=framework={framework}");
    }
}
