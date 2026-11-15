// swift-tools-version: 5.9
// The Swift side of the Mac parity checks (.omc/research/tauri/mac-plan.md §3 Step 0e): Swift's own verdicts, from MightyCore's own
// code, for the Rust tests to compare with (MIGHTY_PARITY_MAC=require on the Mac runner). The counterpart of tools/parity (C#).
// native/macos is a path dependency, read and compiled, never edited.
import PackageDescription

let package = Package(
    name: "ParityMac",
    platforms: [.macOS(.v14)],
    dependencies: [
        .package(path: "../../../macos"),
    ],
    targets: [
        .executableTarget(
            name: "parity-mac",
            dependencies: [.product(name: "MightyCore", package: "macos")],
            path: "Sources/parity-mac"
        ),
    ]
)
