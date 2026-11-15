#!/bin/bash
# Downloads, checks and unzips the prebuilt libghostty (GhosttyKit.xcframework) the Mac's terminal links: the binary target of the
# vendored package (native/macos/Vendor/libghostty-spm/Package.swift), read from there by URL and checksum so the Swift app and the
# Rust one always link the same library. Prints the unzipped xcframework's path (what MIGHTY_GHOSTTY_XCFRAMEWORK takes).
#
#   scripts/fetch-ghosttykit.sh [cache folder]      default ~/Library/Caches/MightyCode/ghosttykit (MIGHTY_GHOSTTY_CACHE overrides)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PACKAGE="$ROOT/native/macos/Vendor/libghostty-spm/Package.swift"
URL="$(sed -n 's/.*url: "\(https:[^"]*GhosttyKit\.xcframework\.zip\)".*/\1/p' "$PACKAGE" | head -n 1)"
SUM="$(sed -n 's/.*checksum: "\([0-9a-f]\{64\}\)".*/\1/p' "$PACKAGE" | head -n 1)"
[ -n "$URL" ] && [ -n "$SUM" ] || { echo "no GhosttyKit URL and checksum in $PACKAGE" >&2; exit 1; }
CACHE="${1:-${MIGHTY_GHOSTTY_CACHE:-$HOME/Library/Caches/MightyCode/ghosttykit}}/${SUM:0:16}"
mkdir -p "$CACHE"
ZIP="$CACHE/GhosttyKit.xcframework.zip"
if [ ! -f "$ZIP" ] || [ "$(shasum -a 256 "$ZIP" | awk '{print $1}')" != "$SUM" ]; then
    echo "Downloading $URL" >&2
    curl -fSL --retry 3 --max-time 600 -o "$ZIP.tmp" "$URL"
    mv "$ZIP.tmp" "$ZIP"
fi
ACTUAL="$(shasum -a 256 "$ZIP" | awk '{print $1}')"
[ "$ACTUAL" = "$SUM" ] || { echo "GhosttyKit checksum mismatch: expected $SUM, got $ACTUAL" >&2; rm -f "$ZIP"; exit 1; }
if [ ! -f "$CACHE/GhosttyKit.xcframework/macos-arm64_x86_64/libghostty.a" ]; then
    rm -rf "$CACHE/GhosttyKit.xcframework"
    (cd "$CACHE" && unzip -q -o "$ZIP" 'GhosttyKit.xcframework/Info.plist' 'GhosttyKit.xcframework/macos-arm64_x86_64/*')
fi
echo "$CACHE/GhosttyKit.xcframework"
