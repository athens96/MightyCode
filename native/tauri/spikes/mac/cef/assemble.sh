#!/bin/bash
# Puts the CEF spike's binary in an app bundle with CEF, the Swift app's C bridge and the four helper apps, as scripts/build-macos.sh
# does for the Swift app (its lines 111-178, with the same names, which cef_bridge.m expects), and signs it ad hoc inside out
# (scripts/sign-macos.sh). Needs a warm engine cache (scripts/fetch-browser-engine.sh).
#
#   assemble.sh <cef-spike binary> <CefSpike.app to make>
set -euo pipefail

BINARY="${1:?binary}"
APP="${2:?app}"
ROOT="$(cd "$(dirname "$0")/../../../../.." && pwd)"
LOCK_FILE="$ROOT/native/macos/BrowserEngine.lock"
CEF_URL="$(python3 -c "import json,sys; print(json.load(open(sys.argv[1]))['cef']['url'])" "$LOCK_FILE")"
LOCK_HASH="$(shasum -a 256 "$LOCK_FILE" | awk '{print $1}')"
ENGINE_CACHE="${MIGHTY_BROWSER_ENGINE_CACHE:-$HOME/Library/Caches/MightyCode/browser-engine/${LOCK_HASH:0:16}}"
CEF_ARCHIVE="$ENGINE_CACHE/$(basename "$CEF_URL")"
[ -f "$CEF_ARCHIVE" ] || { echo "no CEF archive in $ENGINE_CACHE: run scripts/fetch-browser-engine.sh" >&2; exit 1; }

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Frameworks" "$APP/Contents/Resources"
cp "$BINARY" "$APP/Contents/MacOS/cef-spike"
cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>dev.mightycode.spike.cef</string>
<key>CFBundleExecutable</key><string>cef-spike</string>
<key>CFBundleName</key><string>CefSpike</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>0.1.0</string>
<key>CFBundleVersion</key><string>1</string>
<key>LSMinimumSystemVersion</key><string>14.0</string>
<key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
CEF_TOP="$(tar -tjf "$CEF_ARCHIVE" | head -1 | cut -d/ -f1)"
tar -xjf "$CEF_ARCHIVE" -C "$WORK" 2>/dev/null
FW_DIR="$APP/Contents/Frameworks"
ditto "$WORK/$CEF_TOP/Release/Chromium Embedded Framework.framework" "$FW_DIR/Chromium Embedded Framework.framework"
BRIDGE="$ROOT/native/macos/BrowserBridge"
xcrun clang -o "$WORK/helper_bin" -arch arm64 -mmacosx-version-min=14.0 -I "$WORK/$CEF_TOP" "$BRIDGE/helper.c"
xcrun clang -dynamiclib -o "$FW_DIR/MightyCEFBridge.dylib" -arch arm64 -mmacosx-version-min=14.0 \
    -I "$WORK/$CEF_TOP" -framework CoreFoundation -framework AppKit -install_name "@rpath/MightyCEFBridge.dylib" "$BRIDGE/cef_bridge.m"
make_helper() {
    local name="$1" bundle_id="$2"
    local happ="$FW_DIR/${name}.app"
    mkdir -p "$happ/Contents/MacOS"
    cp "$WORK/helper_bin" "$happ/Contents/MacOS/$name"
    cat > "$happ/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>$bundle_id</string>
<key>CFBundleExecutable</key><string>$name</string>
<key>CFBundleName</key><string>$name</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleVersion</key><string>1</string>
<key>CFBundleShortVersionString</key><string>0.1.0</string>
<key>LSMinimumSystemVersion</key><string>14.0</string>
</dict></plist>
PLIST
}
make_helper "MightyCode Helper" "dev.mightycode.spike.cef.helper"
make_helper "MightyCode Helper (GPU)" "dev.mightycode.spike.cef.helper.gpu"
make_helper "MightyCode Helper (Renderer)" "dev.mightycode.spike.cef.helper.renderer"
make_helper "MightyCode Helper (Plugin)" "dev.mightycode.spike.cef.helper.plugin"
bash "$ROOT/scripts/sign-macos.sh" "$APP" -
du -sh "$APP"
