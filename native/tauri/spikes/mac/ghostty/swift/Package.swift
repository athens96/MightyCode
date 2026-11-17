// swift-tools-version: 6.0
// The fallback route of spike D1 (plan D1 as first written): the vendored GhosttyTerminal view, reached from Rust through a few
// @_cdecl functions in a static library. Built by ../build.rs only with MIGHTY_SPIKE_SWIFT=1; native/macos is read, never edited.
import PackageDescription

let package = Package(
    name: "MightyGhosttySpike",
    platforms: [.macOS(.v14)],
    products: [
        .library(name: "MightyGhosttySpike", type: .static, targets: ["MightyGhosttySpike"]),
    ],
    dependencies: [
        .package(path: "../../../../../macos/Vendor/libghostty-spm"),
    ],
    targets: [
        .target(
            name: "MightyGhosttySpike",
            dependencies: [.product(name: "GhosttyTerminal", package: "libghostty-spm")]
        ),
    ],
    swiftLanguageModes: [.v5]
)
