#!/bin/bash
# Mac Step 0, spike 4 (.omc/research/tauri/mac-plan.md §3 0b-4): the mechanics M-I's rehearsal will stand on, tried with a Swift →
# Swift update so nothing of the Rust bundle is needed yet. On a disposable runner only (MIGHTY_REHEARSAL_DISPOSABLE=1): it trusts a
# throwaway certificate in the System keychain (sudo) and writes the runner user's UserDefaults domain dev.mightycode.native.
#
#   rehearsal-mechanics.sh <tag> <work folder>
#
#  1. a throwaway Ed25519 key and a certificate for 127.0.0.1 (scripts/update-rehearsal-server.py), the certificate trusted with
#     `sudo security add-trusted-cert -d -r trustRoot -k /Library/Keychains/System.keychain`, the HTTPS server started (a free port);
#  2. the Swift app built from <tag> by that tag's scripts/build-macos.sh, stamped with the throwaway key and
#     MIGHTY_UPDATE_URL=https://127.0.0.1:<port>/latest.json (the stamped address wins on the Mac: AppStore+AppUpdate.swift rule 3);
#  3. the update: the same app with CFBundleShortVersionString 0.4.99, zipped as the release zips it, and latest.json signed with the
#     throwaway key (scripts/make-update-manifest.py);
#  4. the app copied to <work>/Applications/MightyCode.app and started on a fresh --profile: it must check, download and stage by itself;
#  5. quit with NSRunningApplication.terminate() (terminate.swift): the app must end in time with no consent asked, and its
#     install-on-quit helper must swap the bundle: install.log says "Installed:", the installed Info.plist says 0.4.99, no
#     .update-old/.update-new is left and the staged folder is gone.
#
# Writes <work>/report.json (each check, true or false) and exits 0 when every check held.
set -uo pipefail

TAG="${1:?tag}"
WORK="${2:?work folder}"
if [ "${MIGHTY_REHEARSAL_DISPOSABLE:-}" != "1" ]; then
    echo "This trusts a throwaway certificate system-wide and writes the user's defaults: run it on a disposable runner (MIGHTY_REHEARSAL_DISPOSABLE=1)." >&2
    exit 2
fi
ROOT="$(cd "$(dirname "$0")/../../../../.." && pwd)"
SERVER="$ROOT/scripts/update-rehearsal-server.py"
mkdir -p "$WORK"
WORK="$(cd "$WORK" && pwd -P)"
REPORT="$WORK/report.json"
CHECKS=()
check() { # name value(true|false) detail
    local detail="${3:-}"
    CHECKS+=("$(python3 -c 'import json,sys; print(json.dumps({"check": sys.argv[1], "passed": sys.argv[2] == "true", "detail": sys.argv[3]}))' "$1" "$2" "$detail")")
    echo "check: $1 = $2 ${detail:+($detail)}"
}
write_report() {
    printf '%s\n' "${CHECKS[@]}" | python3 -c 'import json,sys; rows=[json.loads(l) for l in sys.stdin if l.strip()]; json.dump({"checks": rows, "passed": bool(rows) and all(r["passed"] for r in rows)}, open(sys.argv[1], "w"), indent=2)' "$REPORT"
    cat "$REPORT"
}
CERT_NAME="Mighty Code update rehearsal (throwaway)"
SERVER_PID=""; APP_PID=""
cleanup() {
    [ -n "$APP_PID" ] && kill "$APP_PID" 2>/dev/null
    [ -n "$SERVER_PID" ] && kill "$SERVER_PID" 2>/dev/null
    sudo security delete-certificate -c "$CERT_NAME" /Library/Keychains/System.keychain >/dev/null 2>&1 || true
}
trap 'cleanup; write_report' EXIT

echo "== the throwaway key, certificate and server"
python3 "$SERVER" keygen "$WORK/key/update.key" > "$WORK/public-key.txt" || exit 1
PUBLIC_KEY="$(tail -n 1 "$WORK/public-key.txt" | tr -d '[:space:]')"
python3 "$SERVER" cert "$WORK/cert" > /dev/null || exit 1
if sudo security add-trusted-cert -d -r trustRoot -k /Library/Keychains/System.keychain "$WORK/cert/cert.pem"; then
    check "the throwaway certificate is trusted system-wide (sudo, no prompt)" true
else
    check "the throwaway certificate is trusted system-wide (sudo, no prompt)" false "add-trusted-cert failed"
    exit 1
fi
mkdir -p "$WORK/served"
# The first CI round (cfadcf51) found the server alive but its port file missing after 10 s, with nothing on stderr. The likely cause:
# http.server's server_bind asks socket.getfqdn("127.0.0.1"), a reverse lookup a macOS runner may answer only after its DNS times out.
# So: wait up to 60 s, and if it still does not listen, have faulthandler print where it hangs (SIGABRT) and start it again with
# getfqdn answered locally. Both outcomes are in the report (M-I's rehearsal takes whichever fix this shows).
serve_with() { # label, then the command that starts the server
    local label="$1"; shift
    rm -f "$WORK/port"
    echo "== server ($label)" >> "$WORK/server.err"
    PYTHONFAULTHANDLER=1 "$@" "$SERVER" serve "$WORK/served" --cert "$WORK/cert/cert.pem" --key "$WORK/cert/key.pem" --port-file "$WORK/port" --log "$WORK/server.log" >> "$WORK/server.err" 2>&1 &
    SERVER_PID=$!
    local started=$SECONDS
    for _ in $(seq 1 300); do
        [ -s "$WORK/port" ] && { check "the update server listens ($label)" true "after $((SECONDS - started)) s"; return 0; }
        kill -0 "$SERVER_PID" 2>/dev/null || break
        sleep 0.2
    done
    kill -ABRT "$SERVER_PID" 2>/dev/null; sleep 1; kill "$SERVER_PID" 2>/dev/null; SERVER_PID=""
    check "the update server listens ($label)" false "not after $((SECONDS - started)) s: $(tail -c 1500 "$WORK/server.err" | tr '\n' ' ')"
    return 1
}
cat > "$WORK/local-fqdn.py" <<'PYTHON'
import runpy, socket, sys
# http.server's bind asks getfqdn for its name; answer it without DNS.
socket.getfqdn = lambda name="": name or "localhost"
sys.argv = sys.argv[1:]
runpy.run_path(sys.argv[0], run_name="__main__")
PYTHON
serve_with "as it is" python3 || serve_with "getfqdn answered locally" python3 "$WORK/local-fqdn.py" || exit 1
PORT="$(cat "$WORK/port")"
BASE="https://127.0.0.1:$PORT"
echo ok > "$WORK/served/probe.txt"
# The Swift networking stack trusts the System keychain; a tiny Swift probe asks the server the way the app will.
cat > "$WORK/probe.swift" <<'SWIFT'
import Foundation
let url = URL(string: CommandLine.arguments[1])!
let done = DispatchSemaphore(value: 0)
var status = -1
URLSession.shared.dataTask(with: url) { _, response, error in
    status = (response as? HTTPURLResponse)?.statusCode ?? -1
    if let error { FileHandle.standardError.write(Data("\(error)\n".utf8)) }
    done.signal()
}.resume()
_ = done.wait(timeout: .now() + 20)
print(status)
SWIFT
PROBE="$(swift "$WORK/probe.swift" "$BASE/probe.txt" 2>"$WORK/probe.err")"
check "URLSession reaches the server over the trusted certificate" "$([ "$PROBE" = 200 ] && echo true || echo false)" "status $PROBE $(tr '\n' ' ' < "$WORK/probe.err" | cut -c1-300)"

echo "== the Swift app from $TAG, stamped with the throwaway key and $BASE/latest.json"
git -C "$ROOT" worktree add --detach "$WORK/src" "$TAG" >/dev/null 2>&1 || { check "the $TAG worktree" false; exit 1; }
BUILT="$WORK/build/MightyCode.app"
if MIGHTY_UPDATE_URL="$BASE/latest.json" MIGHTY_UPDATE_PUBLIC_KEY="$PUBLIC_KEY" MIGHTY_CODESIGN_IDENTITY=- MIGHTY_MACOS_APP_PATH="$BUILT" \
    MIGHTY_BUILD_SCRATCH="$WORK/swift-build" bash "$WORK/src/scripts/build-macos.sh" > "$WORK/build.log" 2>&1; then
    check "the $TAG app builds with the stamp" true
else
    check "the $TAG app builds with the stamp" false "see build.log"
    tail -n 40 "$WORK/build.log"
    exit 1
fi
STAMPED_URL="$(/usr/libexec/PlistBuddy -c 'Print :MightyUpdateManifestURL' "$BUILT/Contents/Info.plist" 2>/dev/null)"
check "the build carries the local address" "$([ "$STAMPED_URL" = "$BASE/latest.json" ] && echo true || echo false)" "$STAMPED_URL"
FROM_VERSION="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$BUILT/Contents/Info.plist")"

echo "== the update package (0.4.99) and its signed manifest"
mkdir -p "$WORK/package"
ditto "$BUILT" "$WORK/package/MightyCode.app"
/usr/libexec/PlistBuddy -c 'Set :CFBundleShortVersionString 0.4.99' "$WORK/package/MightyCode.app/Contents/Info.plist"
codesign --force --sign - "$WORK/package/MightyCode.app/Contents/MacOS/MightyCode" >/dev/null 2>&1 || true
(cd "$WORK/package" && ditto -c -k --sequesterRsrc --keepParent MightyCode.app "$WORK/served/MightyCode-macos.zip")
MIGHTY_SIGNER=cryptography python3 "$ROOT/scripts/make-update-manifest.py" --macos "$WORK/served/MightyCode-macos.zip" --base-url "$BASE" \
    --out "$WORK/served/latest.json" --sign-key "$WORK/key/update.key" --version 0.4.99 > "$WORK/manifest.log" 2>&1
check "the manifest is signed" "$([ -s "$WORK/served/latest.json" ] && echo true || echo false)" "$(tail -n 1 "$WORK/manifest.log")"

echo "== the installed app on a fresh profile"
mkdir -p "$WORK/Applications" "$WORK/profile"
ditto "$BUILT" "$WORK/Applications/MightyCode.app"
APP="$WORK/Applications/MightyCode.app"
"$APP/Contents/MacOS/MightyCode" --profile "$WORK/profile" > "$WORK/app.log" 2>&1 &
APP_PID=$!
STAGED="$WORK/profile/updates/0.4.99/staged/MightyCode.app"
for _ in $(seq 1 360); do [ -d "$STAGED" ] && break; kill -0 "$APP_PID" 2>/dev/null || break; sleep 0.5; done
grep -q 'latest.json' "$WORK/server.log" 2>/dev/null && ASKED=true || ASKED=false
grep -q 'MightyCode-macos.zip' "$WORK/server.log" 2>/dev/null && DOWNLOADED=true || DOWNLOADED=false
check "the app asked for latest.json by itself" "$ASKED"
check "the app downloaded the zip" "$DOWNLOADED"
check "the app staged the update" "$([ -d "$STAGED" ] && echo true || echo false)" "$STAGED"
sleep 2

echo "== quit through NSRunningApplication.terminate()"
swiftc -O "$(dirname "$0")/terminate.swift" -o "$WORK/terminate" > "$WORK/terminate-build.log" 2>&1
TERMINATED="$("$WORK/terminate" "$APP_PID" 30)"
echo "$TERMINATED"
check "terminate() quits the app in time with no consent asked" "$(echo "$TERMINATED" | grep -q '"terminated":true' && echo true || echo false)" "$TERMINATED"
APP_PID=""
LOG="$WORK/profile/updates/0.4.99/install.log"
for _ in $(seq 1 120); do grep -q 'Installed:' "$LOG" 2>/dev/null && break; sleep 0.5; done
check "install on quit swapped the bundle (install.log)" "$(grep -q 'Installed:' "$LOG" 2>/dev/null && echo true || echo false)" "$(tr '\n' ' ' < "$LOG" 2>/dev/null | cut -c1-400)"
NOW_VERSION="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$APP/Contents/Info.plist" 2>/dev/null)"
check "the installed app is now 0.4.99 (was $FROM_VERSION)" "$([ "$NOW_VERSION" = 0.4.99 ] && echo true || echo false)" "$NOW_VERSION"
LEFT="$(ls -d "$APP".update-* 2>/dev/null)"
check "no .update-old or .update-new is left" "$([ -z "$LEFT" ] && echo true || echo false)" "$LEFT"
check "the staged folder is gone" "$([ ! -e "$WORK/profile/updates/0.4.99/staged" ] && echo true || echo false)"
cp "$LOG" "$WORK/install.log" 2>/dev/null || true
git -C "$ROOT" worktree remove --force "$WORK/src" >/dev/null 2>&1 || true
trap - EXIT
cleanup
write_report
python3 -c 'import json,sys; sys.exit(0 if json.load(open(sys.argv[1]))["passed"] else 1)' "$REPORT"
