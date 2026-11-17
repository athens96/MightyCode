#!/bin/bash
# Builds the Rust + Tauri app (native/tauri) for release on the Mac in the 0.4.x shape (mac-plan.md M-I, D5): the counterpart of
# build-macos.sh, whose bundle (MightyCode.app, dev.mightycode.native, executable MightyCode), Info.plist keys, resources, signing and
# zip it keeps, so an installed Swift 0.4.x app updates itself to it. Tauri's own bundler stays off (tauri.conf.json bundle.active).
#
#   scripts/build-tauri-macos.sh [--version 0.5.0-rc.7] [--engine] [--output <new folder>] [--stager <parity-mac>]
#
# The release stamp comes from the environment, as for build-macos.sh and build-tauri-windows.ps1: MIGHTY_UPDATE_PUBLIC_KEY (32 bytes
# of standard base64, the public half only) and MIGHTY_UPDATE_URL (https). Both are required: this script makes release builds
# (MIGHTY_RELEASE=1, which src-tauri/build.rs refuses without them). It never reads a signing key. The Info.plist carries them too
# (MightyUpdateManifestURL, MightyUpdatePublicKey), as the Swift app's does.
#
# The version is src-tauri/Cargo.toml's, which must equal VERSION. --version builds a candidate instead (CI, the update rehearsal):
# newer than VERSION, so a 0.4.x install takes it as an update; Cargo.toml and Cargo.lock carry it for the build only and are put back
# afterwards.
#
# Steps:
#  1. the version check;
#  2. the stamp (MIGHTY_RELEASE=1, the key and the address), libghostty (scripts/fetch-ghosttykit.sh unless MIGHTY_GHOSTTY_XCFRAMEWORK
#     names it) linked with MIGHTY_MAC_NATIVE=1, macOS 14.0 as the deployment target (LSMinimumSystemVersion);
#  3. npm ci (the page, the Tauri CLI) and `tauri build --target aarch64-apple-darwin` (the page in tauri mode, then release);
#  4. the executable: arm64 only, built for macOS 14.0 at most, and linking nothing outside the system;
#  5. <output>/MightyCode.app: Contents/MacOS/MightyCode; the Info.plist of build-macos.sh (id, executable, LSMinimumSystemVersion
#     14.0, both URL schemes, the usage strings, NSLocalNetworkUsageDescription and Bonjour's _mightyclaude._tcp, CFBundleVersion the
#     commit count, the update address and key) without NSPrincipalClass and with MightyEngine=tauri; the *.lproj InfoPlist.strings,
#     MightyCode.icns, mightycode.png, ThirdPartyLicenses, Ghostty's runtime resources in Ghostty.app's layout (Resources/ghostty with
#     shell-integration, Resources/terminfo), pets, mods/mighty-bridge (without tests/) and Agents; with --engine CEF, MightyCEFBridge.dylib
#     and the four helpers as build-macos.sh:111-178 (and CEF's licence notices); no WebRTC.framework;
#  6. signed inside out by scripts/sign-macos.sh: MIGHTY_CODESIGN_IDENTITY (default ad hoc, "-"), under the hardened runtime with a
#     Developer ID identity or MIGHTY_HARDENED_RUNTIME=1;
#  7. --verify-resources run by the bundle's own executable (VERIFY_RESOURCES_OK);
#  8. release/MightyCode-macos.zip by `ditto -c -k --sequesterRsrc --keepParent` (as native-macos.yml packs), its .sha256, and the size
#     gate (scripts/check-release-size.sh);
#  9. the 0.4.x stager's verdict on the zip from Swift's own code (tools/parity-mac `stage`: AppUpdateService.stage and validate):
#     accepted as MightyCode.app, dev.mightycode.native, executable MightyCode, at this version. --stager names a built parity-mac;
#     otherwise it is built here (swift build -c release).
# Needs Xcode's toolchain (swift, clang, codesign, ditto, plutil), Node, the Rust toolchain with the aarch64-apple-darwin target and
# python3.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TAURI="$ROOT/native/tauri"
CARGO_TOML="$TAURI/src-tauri/Cargo.toml"
CARGO_LOCK="$TAURI/Cargo.lock"
TRIPLE=aarch64-apple-darwin
VERSION_ARG=""; ENGINE=0; OUTPUT=""; STAGER=""
while [ $# -gt 0 ]; do
    case "$1" in
        --version) VERSION_ARG="${2:?--version needs a version}"; shift 2 ;;
        --engine) ENGINE=1; shift ;;
        --output) OUTPUT="${2:?--output needs a folder}"; shift 2 ;;
        --stager) STAGER="${2:?--stager needs parity-mac}"; shift 2 ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done
[ "$(uname -s)" = Darwin ] || { echo "the Mac build runs on macOS" >&2; exit 1; }
fail() { echo "build-tauri-macos: $*" >&2; exit 1; }

# --- 1. the version
CARGO_VERSION="$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$CARGO_TOML" | head -n 1)"
REPOSITORY_VERSION="$(tr -d '[:space:]' < "$ROOT/VERSION")"
[[ "$REPOSITORY_VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || fail "VERSION must be x.y.z; got '$REPOSITORY_VERSION'"
if [ -n "$VERSION_ARG" ]; then
    [[ "$VERSION_ARG" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]] || fail "--version must be x.y.z or x.y.z-pre; got '$VERSION_ARG'"
    python3 -c 'import sys; a, b = (tuple(map(int, v.split("-")[0].split("."))) for v in sys.argv[1:]); sys.exit(0 if a > b else 1)' \
        "$VERSION_ARG" "$REPOSITORY_VERSION" || fail "--version $VERSION_ARG is not newer than VERSION $REPOSITORY_VERSION: a 0.4.x install would not take it as an update"
    APP_VERSION="$VERSION_ARG"
else
    [ "$CARGO_VERSION" = "$REPOSITORY_VERSION" ] || fail "src-tauri/Cargo.toml's version $CARGO_VERSION is not VERSION $REPOSITORY_VERSION (--version builds a candidate)"
    APP_VERSION="$CARGO_VERSION"
fi

# --- 2. the stamp (build.rs checks it again)
UPDATE_KEY="$(printf '%s' "${MIGHTY_UPDATE_PUBLIC_KEY:-}" | tr -d '[:space:]')"
UPDATE_URL="$(printf '%s' "${MIGHTY_UPDATE_URL:-}" | tr -d '[:space:]')"
[ -n "$UPDATE_KEY" ] && [ -n "$UPDATE_URL" ] || fail "MIGHTY_UPDATE_PUBLIC_KEY and MIGHTY_UPDATE_URL are required: this is a release build (MIGHTY_RELEASE=1)"
[ "$(printf '%s' "$UPDATE_KEY" | base64 -D 2>/dev/null | wc -c | tr -d ' ')" = 32 ] || fail "MIGHTY_UPDATE_PUBLIC_KEY must be a raw 32-byte Ed25519 public key in base64"
[[ "$UPDATE_URL" =~ ^https://[^/?#]+ ]] || fail "MIGHTY_UPDATE_URL must be an https address with a host"

# --- where it goes: a new or empty folder, never an old zip overwritten
OUTPUT="${OUTPUT:-$ROOT/release/tauri-macos}"
mkdir -p "$OUTPUT"
OUTPUT="$(cd "$OUTPUT" && pwd -P)"
[ -z "$(ls -A "$OUTPUT")" ] || fail "give a new or empty folder for the build: $OUTPUT"
RELEASE_DIR="$(dirname "$OUTPUT")"
ARCHIVE="$RELEASE_DIR/MightyCode-macos.zip"
[ ! -e "$ARCHIVE" ] || fail "an earlier zip is never overwritten: $ARCHIVE"
APP="$OUTPUT/MightyCode.app"

CARGO_TOML_SAVED="$(mktemp)"; CARGO_LOCK_SAVED="$(mktemp)"; WORK="$(mktemp -d)"
cp "$CARGO_TOML" "$CARGO_TOML_SAVED"; cp "$CARGO_LOCK" "$CARGO_LOCK_SAVED"
restore() {
    cp "$CARGO_TOML_SAVED" "$CARGO_TOML"; cp "$CARGO_LOCK_SAVED" "$CARGO_LOCK"
    rm -rf "$CARGO_TOML_SAVED" "$CARGO_LOCK_SAVED" "$WORK"
}
trap restore EXIT
if [ "$APP_VERSION" != "$CARGO_VERSION" ]; then
    # A candidate's version, for this build only (put back on exit): the executable's own version.
    python3 - "$CARGO_TOML" "$CARGO_LOCK" "$APP_VERSION" <<'PY'
import re, sys
toml, lock, version = sys.argv[1:]
text = open(toml, encoding="utf-8").read()
open(toml, "w", encoding="utf-8", newline="").write(re.sub(r'(?m)^version = "[^"]+"', f'version = "{version}"', text, count=1))
text = open(lock, encoding="utf-8").read()
open(lock, "w", encoding="utf-8", newline="").write(re.sub(r'(?m)^(name = "mightycode"\r?\nversion = )"[^"]+"', rf'\g<1>"{version}"', text, count=1))
PY
fi
echo "Mighty Code $APP_VERSION (arm64, $TRIPLE): release build$([ "$ENGINE" = 1 ] && echo ', with the browser engine')"

export MIGHTY_RELEASE=1 MIGHTY_UPDATE_PUBLIC_KEY="$UPDATE_KEY" MIGHTY_UPDATE_URL="$UPDATE_URL"
export MIGHTY_MAC_NATIVE=1
if [ -z "${MIGHTY_GHOSTTY_XCFRAMEWORK:-}" ]; then
    MIGHTY_GHOSTTY_XCFRAMEWORK="$(bash "$ROOT/scripts/fetch-ghosttykit.sh")"
fi
export MIGHTY_GHOSTTY_XCFRAMEWORK
# LSMinimumSystemVersion 14.0 (review M-E: below 14 WKWebView shares one data store): the executable is built for it too.
export MACOSX_DEPLOYMENT_TARGET=14.0

# --- 3. the page and the release build
npm ci --no-audit --no-fund --prefix "$ROOT/native/windows/web"
npm ci --no-audit --no-fund --prefix "$TAURI"
(cd "$TAURI" && npm run tauri build -- --target "$TRIPLE")
TARGET="${CARGO_TARGET_DIR:-$TAURI/target}"
case "$TARGET" in /*) ;; *) TARGET="$TAURI/src-tauri/$TARGET" ;; esac
BUILT="$TARGET/$TRIPLE/release/MightyCode"
[ -f "$BUILT" ] || fail "the build left no executable at $BUILT"

# --- 4. the executable: arm64, macOS 14.0, nothing linked from outside the system
[ "$(lipo -archs "$BUILT")" = arm64 ] || fail "the executable is $(lipo -archs "$BUILT"), not arm64"
MINOS="$(otool -l "$BUILT" | awk '/LC_BUILD_VERSION/ { found = 1 } found && $1 == "minos" { print $2; exit }')"
python3 -c 'import sys; sys.exit(0 if tuple(map(int, sys.argv[1].split("."))) <= (14, 0) else 1)' "${MINOS:-99}" \
    || fail "the executable needs macOS ${MINOS:-?}, above LSMinimumSystemVersion 14.0"
FOREIGN="$(otool -L "$BUILT" | tail -n +2 | awk '{ print $1 }' | grep -Ev '^(/System/Library/|/usr/lib/)' || true)"
[ -z "$FOREIGN" ] || fail "the executable links libraries outside the system: $FOREIGN"

# --- 5. the bundle
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources/mods"
cp "$BUILT" "$APP/Contents/MacOS/MightyCode"
RESOURCES="$APP/Contents/Resources"
cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleName</key><string>Mighty Code</string>
<key>CFBundleDisplayName</key><string>Mighty Code</string>
<key>LSHasLocalizedDisplayName</key><true/>
<key>CFBundleIdentifier</key><string>dev.mightycode.native</string>
<key>CFBundleDevelopmentRegion</key><string>en</string>
<key>CFBundleLocalizations</key><array><string>ko</string><string>en</string><string>zh-Hans</string><string>ja</string></array>
<key>CFBundleExecutable</key><string>MightyCode</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleIconFile</key><string>MightyCode.icns</string>
<key>CFBundleShortVersionString</key><string>0.1.0</string>
<key>CFBundleVersion</key><string>1</string>
<key>LSMinimumSystemVersion</key><string>14.0</string>
<key>NSHighResolutionCapable</key><true/>
<key>NSAppTransportSecurity</key><dict><key>NSAllowsLocalNetworking</key><true/></dict>
<key>NSLocalNetworkUsageDescription</key><string>같은 Wi-Fi의 휴대폰과 화면 공유를 연결하려면 로컬 네트워크 접근이 필요합니다.</string>
<key>NSBonjourServices</key><array><string>_mightyclaude._tcp</string></array>
<key>CFBundleURLTypes</key><array><dict><key>CFBundleURLName</key><string>dev.mightycode.native.link</string><key>CFBundleURLSchemes</key><array><string>mightycode</string><string>mightyclaude</string></array></dict></array>
<key>NSAppleEventsUsageDescription</key><string>터미널에서 실행한 명령이 다른 앱을 제어하려고 합니다.</string>
<key>NSMicrophoneUsageDescription</key><string>터미널에서 실행한 명령이 마이크를 사용하려고 합니다.</string>
<key>NSCameraUsageDescription</key><string>터미널에서 실행한 명령이 카메라를 사용하려고 합니다.</string>
<key>MightyEngine</key><string>tauri</string>
</dict></plist>
PLIST
BUILD_NUMBER="${MIGHTY_BUILD_NUMBER:-$(git -C "$ROOT" rev-list --count HEAD 2>/dev/null || echo 1)}"
plutil -replace CFBundleShortVersionString -string "$APP_VERSION" "$APP/Contents/Info.plist"
plutil -replace CFBundleVersion -string "$BUILD_NUMBER" "$APP/Contents/Info.plist"
plutil -replace MightyUpdateManifestURL -string "$UPDATE_URL" "$APP/Contents/Info.plist"
plutil -replace MightyUpdatePublicKey -string "$UPDATE_KEY" "$APP/Contents/Info.plist"
plutil -lint "$APP/Contents/Info.plist" >/dev/null
# The languages the app speaks, so AppKit draws its own menus in the user's language.
for language in ko en zh-Hans ja; do
    mkdir -p "$RESOURCES/$language.lproj"
    printf '"CFBundleDisplayName" = "Mighty Code";\n' > "$RESOURCES/$language.lproj/InfoPlist.strings"
done
ditto "$ROOT/native/licenses" "$RESOURCES/ThirdPartyLicenses"
cp "$ROOT/assets/icons/MightyCode.icns" "$RESOURCES/MightyCode.icns"
cp "$ROOT/assets/icons/mightycode.png" "$RESOURCES/mightycode.png"
# Ghostty's shell integration and terminfo (the vendored package's resources, which the Swift app carried in its SwiftPM bundle), in
# Ghostty.app's own layout: what --verify-resources and the PTY host's shell integration look for.
GHOSTTY_RESOURCES="$ROOT/native/macos/Vendor/libghostty-spm/Sources/GhosttyTerminal/Resources"
ditto "$GHOSTTY_RESOURCES/Ghostty" "$RESOURCES/ghostty"
ditto "$GHOSTTY_RESOURCES/terminfo" "$RESOURCES/terminfo"
ditto "$ROOT/assets/pets" "$RESOURCES/pets"
# tests/ holds the mod's dev-only checks (and node_modules after npm ci); never ship it.
rsync -a --exclude /tests "$ROOT/mods/mighty-bridge/" "$RESOURCES/mods/mighty-bridge/"
# The agents' files (Tauri's resource folder is Contents/Resources: runs.rs `agents_directory`).
mkdir -p "$RESOURCES/Agents"
find "$ROOT/native/macos/Sources/MightyCore/Resources/Agents" -maxdepth 1 -type f -exec cp {} "$RESOURCES/Agents/" \;

if [ "$ENGINE" = 1 ]; then
    # The browser engine as build-macos.sh:111-178 bundles it (the names cef_bridge.m expects). Needs a warm engine cache
    # (scripts/fetch-browser-engine.sh). The pinned Node is not bundled, as there.
    LOCK_FILE="$ROOT/native/macos/BrowserEngine.lock"
    CEF_URL="$(python3 -c "import json,sys; print(json.load(open(sys.argv[1]))['cef']['url'])" "$LOCK_FILE")"
    LOCK_HASH="$(shasum -a 256 "$LOCK_FILE" | awk '{print $1}')"
    ENGINE_CACHE="${MIGHTY_BROWSER_ENGINE_CACHE:-$HOME/Library/Caches/MightyCode/browser-engine/${LOCK_HASH:0:16}}"
    CEF_ARCHIVE="$ENGINE_CACHE/$(basename "$CEF_URL")"
    [ -f "$CEF_ARCHIVE" ] || fail "browser engine not in cache — run scripts/fetch-browser-engine.sh first"
    FW_DIR="$APP/Contents/Frameworks"
    mkdir -p "$FW_DIR"
    CEF_TOP="$(tar -tjf "$CEF_ARCHIVE" | head -1 | cut -d/ -f1)"
    tar -xjf "$CEF_ARCHIVE" -C "$WORK" 2>/dev/null
    ditto "$WORK/$CEF_TOP/Release/Chromium Embedded Framework.framework" "$FW_DIR/Chromium Embedded Framework.framework"
    cp "$WORK/$CEF_TOP/LICENSE.txt" "$RESOURCES/ThirdPartyLicenses/CEF-LICENSE.txt"
    cp "$WORK/$CEF_TOP/CREDITS.html" "$RESOURCES/ThirdPartyLicenses/Chromium-CREDITS.html"
    BRIDGE="$ROOT/native/macos/BrowserBridge"
    xcrun clang -o "$WORK/helper_bin" -arch arm64 -mmacosx-version-min=14.0 -I "$WORK/$CEF_TOP" "$BRIDGE/helper.c"
    xcrun clang -dynamiclib -o "$FW_DIR/MightyCEFBridge.dylib" -arch arm64 -mmacosx-version-min=14.0 \
        -I "$WORK/$CEF_TOP" -framework CoreFoundation -framework AppKit -install_name "@rpath/MightyCEFBridge.dylib" "$BRIDGE/cef_bridge.m"
    make_helper() {
        local name="$1" bundle_id="$2"
        local helper="$FW_DIR/${name}.app"
        mkdir -p "$helper/Contents/MacOS"
        cp "$WORK/helper_bin" "$helper/Contents/MacOS/$name"
        cat > "$helper/Contents/Info.plist" <<PLIST
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
    make_helper "MightyCode Helper" "dev.mightycode.native.helper"
    make_helper "MightyCode Helper (GPU)" "dev.mightycode.native.helper.gpu"
    make_helper "MightyCode Helper (Renderer)" "dev.mightycode.native.helper.renderer"
    make_helper "MightyCode Helper (Plugin)" "dev.mightycode.native.helper.plugin"
fi

# What the bundle must hold (the 0.4.x stager and --verify-resources check part of it again), and no link anywhere in it (ditto keeps
# CEF's framework links, which the Swift app's bundle has too, so only those are allowed).
for required in Contents/MacOS/MightyCode Contents/Info.plist Contents/Resources/MightyCode.icns Contents/Resources/ko.lproj/InfoPlist.strings \
    Contents/Resources/ghostty/shell-integration/zsh/.zshenv Contents/Resources/ghostty/shell-integration/bash/ghostty.bash \
    Contents/Resources/terminfo/78/xterm-ghostty Contents/Resources/pets/mighty-raccoon/pet.json \
    Contents/Resources/mods/mighty-bridge/.claude-plugin/plugin.json Contents/Resources/Agents/pi-ask-user.js \
    Contents/Resources/Agents/opencode-questions.md Contents/Resources/ThirdPartyLicenses/Ghostty-LICENSE; do
    [ -f "$APP/$required" ] || fail "the bundle lacks $required"
done
[ ! -e "$APP/Contents/Resources/mods/mighty-bridge/tests" ] || fail "the mod's tests/ were bundled"
[ ! -e "$APP/Contents/Frameworks/WebRTC.framework" ] || fail "WebRTC.framework is not bundled (plan D13)"
LINKS="$(find "$APP" -type l ! -path '*/Chromium Embedded Framework.framework/*' -print)"
[ -z "$LINKS" ] || fail "the bundle holds links: $LINKS"

# --- 6. signing (nested code first; scripts/sign-macos.sh verifies --deep --strict at its end)
IDENTITY="${MIGHTY_CODESIGN_IDENTITY:--}"
HARDENED=""
case "$IDENTITY" in "Developer ID Application"*) HARDENED="--hardened-runtime" ;; esac
[ "${MIGHTY_HARDENED_RUNTIME:-}" = 1 ] && HARDENED="--hardened-runtime"
bash "$ROOT/scripts/sign-macos.sh" "$APP" "$IDENTITY" $HARDENED

# --- 7. the bundle's own check of its resources
(cd /tmp && "$APP/Contents/MacOS/MightyCode" --verify-resources) || fail "--verify-resources failed in the packaged app"

# --- 8. the zip, its digest and the size gate
(cd "$OUTPUT" && ditto -c -k --sequesterRsrc --keepParent MightyCode.app "$ARCHIVE")
HASH="$(shasum -a 256 "$ARCHIVE" | awk '{print $1}')"
printf '%s  %s\n' "$HASH" "$(basename "$ARCHIVE")" > "$ARCHIVE.sha256"
bash "$ROOT/scripts/check-release-size.sh" "$ARCHIVE"
echo "package: $ARCHIVE ($(stat -f%z "$ARCHIVE") bytes, $(du -sh "$APP" | awk '{print $1}') unpacked)"

# --- 9. the 0.4.x stager's verdict, from Swift's own code
if [ -z "$STAGER" ]; then
    swift build -c release --package-path "$TAURI/tools/parity-mac"
    STAGER="$TAURI/tools/parity-mac/.build/release/parity-mac"
fi
VERDICT="$("$STAGER" stage "$ARCHIVE")"
echo "0.4.x stager: $VERDICT"
python3 - "$VERDICT" "$APP_VERSION" <<'PY' || fail "the 0.4.x stager did not accept the package as built"
import json, sys
verdict, version = json.loads(sys.argv[1]), sys.argv[2]
expected = {"verdict": "ok", "app": "MightyCode.app", "bundleIdentifier": "dev.mightycode.native", "executable": "MightyCode", "shortVersion": version}
sys.exit(0 if all(verdict.get(key) == value for key, value in expected.items()) else 1)
PY
