#!/bin/bash
# The Mac update rehearsal (mac-plan.md M-I, R1): a real Swift 0.4.x app, built from its tag with a throwaway signing key and the
# local update address, updates itself to the Rust build served from a local HTTPS server; what it leaves is checked; then the
# relaunch path and a Rust → Rust update through the Rust app's own updater. The counterpart of update-rehearsal.ps1. On a disposable
# runner only (MIGHTY_REHEARSAL_DISPOSABLE=1): it trusts a throwaway certificate in the System keychain (sudo), writes the runner
# user's dev.mightycode.native defaults and starts the Rust app on its default profile. Nothing is published and no real key is used.
#
#   MIGHTY_REHEARSAL_DISPOSABLE=1 scripts/update-rehearsal-macos.sh --from v0.4.61 --rust-zip <MightyCode-macos.zip> \
#       --key <throwaway update.key> [--work <new folder>]
#
# The Rust zip comes from build-tauri-macos.sh stamped with the throwaway key's public half and https://127.0.0.1:<port>/latest.json
# (CI's package-macos job; the Mac's stamped address wins over a saved one, AppStore+AppUpdate.swift rule 3, so the address is fixed
# before the build and the server listens on its port). <key> is that key's seed (update-rehearsal-server.py keygen).
#
#  1. the Rust package: MightyEngine tauri, dev.mightycode.native, a version newer than the tag, the stamp naming this key and a
#     loopback address;
#  2. a throwaway certificate for 127.0.0.1 trusted in the System keychain, the server on the stamped port, Swift's own reading of the
#     signed manifest (tools/parity-mac `manifest`);
#  3. the Swift app from <tag> (a detached worktree, that tag's build-macos.sh, ad hoc) stamped with the same key and address, copied
#     to <work>/Applications/MightyCode.app (outside /Applications: the Rust app then writes only with --allow-state-write);
#  4. it starts on a new profile holding one workspace, with the setup sheet turned off (`setup.neverShow`: a sheet makes AppKit set a
#     quit aside, which kept the Step 0 spike's terminate() from quitting the app), and checks, downloads and stages by itself;
#  5. NSRunningApplication.terminate() quits it with no consent asked: the install-on-quit path; if it does not end, a picture of the
#     screen and a sample of the process are kept;
#  6. checked: install.log's `Installed: <the app>`, the app's Info.plist (MightyEngine tauri, the Rust version), no .update-old or
#     .update-new beside it, the staged folder gone, the server asked for latest.json and the zip, the installed app's signature;
#  7. the installed Rust app passes the smoke (native/tauri/scripts/smoke.ps1 -NoRuns, its own seeded profile);
#  8. it starts on the rehearsal profile with --read-only: its host.log loads the state with the one workspace, writes off, and the
#     state file is left as it was;
#  9. the relaunch path: the tag's app installed again, Swift's installScript for "restart now" (relaunch true; parity-mac
#     `install-script`, against a process that has ended) run as the Swift app runs it (/bin/sh, PATH and HOME only): install.log's
#     `Installed:`, the Rust app in place, started by the script's `open -n`, writing its host log on the default profile;
# 10. Rust → Rust: the same package as version <rust>.1 (its Info.plist, signed again ad hoc: the Rust stager checks the staged
#     bundle's signature) served with a new manifest; the installed Rust app on a new profile with --allow-state-write checks,
#     downloads and stages it, is quit by terminate() and installs it on quit, with the checks of 6.
# Every check is printed; the result is <work>/rehearsal-result.json. Exit 0 when all hold.
# Needs git (the tags), Xcode's toolchain, python3 with `cryptography`, pwsh and the Rust toolchain (the smoke seeds with cargo).
set -uo pipefail

FROM=""; RUST_ZIP=""; KEY=""; WORK=""
while [ $# -gt 0 ]; do
    case "$1" in
        --from) FROM="${2:?}"; shift 2 ;;
        --rust-zip) RUST_ZIP="${2:?}"; shift 2 ;;
        --key) KEY="${2:?}"; shift 2 ;;
        --work) WORK="${2:?}"; shift 2 ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done
[[ "$FROM" =~ ^v0\.4\.[0-9]+$ ]] || { echo "--from v0.4.N is required" >&2; exit 2; }
[ -f "$RUST_ZIP" ] && [ -f "$KEY" ] || { echo "--rust-zip and --key must name files" >&2; exit 2; }
if [ "${MIGHTY_REHEARSAL_DISPOSABLE:-}" != "1" ]; then
    echo "The rehearsal trusts a throwaway certificate system-wide, writes the user's defaults and starts the app on the default profile: run it on a disposable runner (MIGHTY_REHEARSAL_DISPOSABLE=1)." >&2
    exit 2
fi
[ "$(uname -s)" = Darwin ] || { echo "the Mac rehearsal runs on macOS" >&2; exit 2; }
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SERVER="$ROOT/scripts/update-rehearsal-server.py"
WORK="${WORK:-$(mktemp -d "${TMPDIR:-/tmp}/mighty-rehearsal-XXXXXX")}"
mkdir -p "$WORK"
WORK="$(cd "$WORK" && pwd -P)"
[ -z "$(ls -A "$WORK")" ] || { echo "give a new or empty folder for --work: $WORK" >&2; exit 2; }
RUST_ZIP="$(cd "$(dirname "$RUST_ZIP")" && pwd -P)/$(basename "$RUST_ZIP")"
KEY="$(cd "$(dirname "$KEY")" && pwd -P)/$(basename "$KEY")"
FROM_VERSION="${FROM#v}"
DOMAIN=dev.mightycode.native
APPS="$WORK/Applications"
APP="$APPS/MightyCode.app"
EXE="$APP/Contents/MacOS/MightyCode"
SERVED="$WORK/served"
DEFAULT_PROFILE="$HOME/Library/Application Support/MightyCode"
LOCAL_LOGS="$HOME/Library/Application Support/$DOMAIN/logs"
CERT_SHA1=""

CHECKS=()
FAILED=()
check() { # name, true|false, detail
    local name="$1" value="$2" detail="${3:-}"
    CHECKS+=("$(python3 -c 'import json,sys; print(json.dumps({"check": sys.argv[1], "passed": sys.argv[2] == "true", "detail": sys.argv[3]}))' "$name" "$value" "$detail")")
    if [ "$value" = true ]; then echo "  ok   $name"; else echo "  FAIL $name ${detail:+($detail)}"; FAILED+=("$name"); fi
}
holds() { if "$@"; then echo true; else echo false; fi }
# The last command's status as true or false (after a chain of tests).
verdict() { if [ $? -eq 0 ]; then echo true; else echo false; fi }
step() { echo; echo "== $*"; }
plist() { /usr/libexec/PlistBuddy -c "Print :$2" "$1/Contents/Info.plist" 2>/dev/null; }
wait_for() { # seconds, then a test command; 0 once it holds
    local until=$((SECONDS + $1)); shift
    while [ $SECONDS -lt $until ]; do "$@" && return 0; sleep 0.5; done
    "$@"
}
# Until <file> is an executable, or the process <pid> ended.
ready_or_gone() { test -x "$1" || ! kill -0 "$2" 2>/dev/null; }
# Until <file> holds <text>, or the process <pid> ended.
written_or_gone() { grep -q "$2" "$1" 2>/dev/null || ! kill -0 "$3" 2>/dev/null; }
# The apps this rehearsal started (anything run from <work>), by their executables' paths.
apps_running() { pgrep -f "^$(printf '%s' "$WORK/" | sed 's/[][\.*^$+?(){}|]/\\&/g')" 2>/dev/null; }
# The bundle's own check of its resources, run from a folder outside it.
verify_resources() { (cd /tmp && "$1" --verify-resources > /dev/null); }
SERVER_PID=""; WORKTREE=""
cleanup() {
    step "clean up"
    for pid in $(apps_running); do kill -9 "$pid" 2>/dev/null; done
    [ -n "$SERVER_PID" ] && kill "$SERVER_PID" 2>/dev/null
    # The throwaway certificate, by its SHA-1 (never by name), which takes its trust setting with it. Bounded and non-interactive:
    # `security remove-trusted-cert` can wait for an authorization no runner gives (native-tauri.yml 44112802032).
    if [ -n "$CERT_SHA1" ]; then
        perl -e 'alarm shift; exec @ARGV' 30 sudo -n security delete-certificate -Z "$CERT_SHA1" /Library/Keychains/System.keychain             < /dev/null > /dev/null 2>&1 || true
    fi
    rm -f "$WORK/cert/key.pem"
    [ -n "$WORKTREE" ] && git -C "$ROOT" worktree remove --force "$WORKTREE" >/dev/null 2>&1
    # What a failed run is read from (CI keeps <work>/logs): install.log and install.sh, the apps' logs, the Rust app's host logs.
    mkdir -p "$WORK/logs"
    for profile in profile rust-profile; do
        [ -d "$WORK/$profile" ] || continue
        (cd "$WORK" && find "$profile" \( -name '*.log' -o -name 'install.sh' -o -name '*.json' \) -type f -not -path '*/staged/*' -print0) |
            while IFS= read -r -d '' file; do mkdir -p "$(dirname "$WORK/logs/$file")"; cp "$WORK/$file" "$WORK/logs/$file"; done
    done
    [ -d "$DEFAULT_PROFILE/tauri-logs" ] && ditto "$DEFAULT_PROFILE/tauri-logs" "$WORK/logs/default-profile-tauri-logs" 2>/dev/null
    [ -d "$LOCAL_LOGS/default" ] && ditto "$LOCAL_LOGS/default" "$WORK/logs/default-profile-local-logs" 2>/dev/null
    python3 - "$WORK/rehearsal-result.json" "$FROM" "${RUST_VERSION:-}" ${CHECKS[@]+"${CHECKS[@]}"} <<'PY'
import json, sys
path, start, to, *rows = sys.argv[1:]
checks = [json.loads(row) for row in rows]
json.dump({"from": start, "to": to, "passed": bool(checks) and all(c["passed"] for c in checks), "checks": checks}, open(path, "w"), indent=2)
PY
    echo
    if [ ${#FAILED[@]} -gt 0 ]; then echo "rehearsal from $FROM: ${#FAILED[@]} check(s) failed: $(IFS='; '; echo "${FAILED[*]}")"; else echo "rehearsal from $FROM: passed (${#CHECKS[@]} checks)"; fi
}
finish() { trap - EXIT INT TERM; cleanup; [ ${#FAILED[@]} -eq 0 ] && [ ${#CHECKS[@]} -gt 0 ]; exit $?; }
# A step whose failure leaves nothing to check after it ends the run here.
abort() { check "$1" false "${2:-}"; finish; }
trap 'check "the rehearsal ran to its end" false; finish' EXIT INT TERM

# The quit through NSRunningApplication.terminate(): the polite quit that takes an app's install-on-quit path, with no Apple Events
# script (whose Automation consent a runner could not give). Prints one JSON line; exit 0 once the app ended within <seconds>.
cat > "$WORK/terminate.swift" <<'SWIFT'
import AppKit
let arguments = CommandLine.arguments
guard arguments.count == 3, let pid = pid_t(arguments[1]), let limit = Double(arguments[2]) else { print(#"{"error":"usage"}"#); exit(2) }
guard let app = NSRunningApplication(processIdentifier: pid) else { print(#"{"error":"no running application with that pid"}"#); exit(1) }
let sent = app.terminate()
let start = Date()
while !app.isTerminated && Date().timeIntervalSince(start) < limit { RunLoop.current.run(until: Date().addingTimeInterval(0.25)) }
print("{\"bundleIdentifier\":\"\(app.bundleIdentifier ?? "")\",\"terminateSent\":\(sent),\"terminated\":\(app.isTerminated),\"seconds\":\(String(format: "%.1f", Date().timeIntervalSince(start)))}")
exit(app.isTerminated ? 0 : 1)
SWIFT
# quit <pid> <seconds> <label>: terminate(); when it does not end, what it was doing is kept and it is killed so the run goes on.
quit() {
    local pid="$1" seconds="$2" label="$3" answer
    answer="$("$WORK/terminate" "$pid" "$seconds" 2>&1)"
    echo "  terminate($pid): $answer" >&2
    kill -0 "$pid" 2>/dev/null || return 0
    mkdir -p "$WORK/logs"
    screencapture -x "$WORK/logs/$label-not-quit.png" > /dev/null 2>&1
    sample "$pid" 3 -file "$WORK/logs/$label-not-quit-sample.txt" > /dev/null 2>&1
    kill -9 "$pid" 2>/dev/null
    return 1
}
# A profile holding one workspace (the state's shape both apps read: StateRepository, mighty-core).
make_profile() { # folder
    mkdir -p "$1" "$WORK/workspace"
    python3 - "$1/workspace-state.json" "$WORK/workspace" <<'PY'
import json, sys
json.dump({"version": 1, "workspaces": [{"id": "rehearsal-workspace", "name": "Rehearsal", "path": sys.argv[2], "createdAt": "2026-11-15T00:00:00Z"}],
           "sessions": []}, open(sys.argv[1], "w"), indent=2)
PY
}
# The update's checks after an install on quit (step 6, step 10).
installed_checks() { # version, profile, label
    local version="$1" profile="$2" label="$3" log line left
    log="$profile/updates/$version/install.log"
    wait_for 120 grep -q 'Installed:' "$log" 2>/dev/null
    line="$(grep 'Installed:' "$log" 2>/dev/null | tail -n 1)"
    echo "  install.log: $(tr '\n' ' ' < "$log" 2>/dev/null | cut -c1-600)"
    check "$label: install.log says Installed: $APP" "$(holds [ "$line" = "Installed: $APP" ])" "$line"
    check "$label: the app is the Rust app at $version" "$([ "$(plist "$APP" MightyEngine)" = tauri ] && [ "$(plist "$APP" CFBundleShortVersionString)" = "$version" ]; verdict)" \
        "MightyEngine $(plist "$APP" MightyEngine), version $(plist "$APP" CFBundleShortVersionString)"
    left="$(ls -d "$APP".update-* "$APPS"/*.update-* 2>/dev/null | sort -u | tr '\n' ' ')"
    check "$label: no .update-old or .update-new is left" "$(holds [ -z "$left" ])" "$left"
    check "$label: the staged folder is gone" "$(holds [ ! -e "$profile/updates/$version/staged" ])"
    check "$label: the installed app's signature holds" "$(holds codesign --verify --deep --strict "$APP")"
}

step "the Rust package $RUST_ZIP"
mkdir -p "$WORK/rust" "$APPS" "$SERVED"
ditto -x -k "$RUST_ZIP" "$WORK/rust" || abort "the Rust zip unpacks"
RUST_APP="$WORK/rust/MightyCode.app"
RUST_VERSION="$(plist "$RUST_APP" CFBundleShortVersionString)"
STAMPED_URL="$(plist "$RUST_APP" MightyUpdateManifestURL)"
PUBLIC_KEY="$(python3 - "$KEY" <<'PY'
import base64, sys
from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
seed = base64.b64decode(open(sys.argv[1]).read().strip())
public = Ed25519PrivateKey.from_private_bytes(seed).public_key().public_bytes(serialization.Encoding.Raw, serialization.PublicFormat.Raw)
print(base64.b64encode(public).decode())
PY
)"
echo "  version $RUST_VERSION, engine $(plist "$RUST_APP" MightyEngine), address $STAMPED_URL"
PORT="$(printf '%s' "$STAMPED_URL" | sed -n 's|^https://127\.0\.0\.1:\([0-9]*\)/latest\.json$|\1|p')"
check "the package is the Rust app (dev.mightycode.native, MightyEngine tauri)" \
    "$([ "$(plist "$RUST_APP" CFBundleIdentifier)" = $DOMAIN ] && [ "$(plist "$RUST_APP" MightyEngine)" = tauri ] && [ "$(plist "$RUST_APP" CFBundleExecutable)" = MightyCode ]; verdict)"
check "the package is stamped with this key and a loopback address" "$([ -n "$PORT" ] && [ "$(plist "$RUST_APP" MightyUpdatePublicKey)" = "$PUBLIC_KEY" ]; verdict)" "$STAMPED_URL"
python3 -c 'import sys; v = lambda t: tuple(map(int, t.split("-")[0].split("."))); sys.exit(0 if v(sys.argv[1]) > v(sys.argv[2]) else 1)' "$RUST_VERSION" "$FROM_VERSION" \
    || abort "the package's version $RUST_VERSION is newer than $FROM_VERSION"
[ -n "$PORT" ] || abort "the package's address names a port on 127.0.0.1" "$STAMPED_URL"
BASE="https://127.0.0.1:$PORT"

step "Swift's own tools (tools/parity-mac) and the terminate helper"
swift build -c release --package-path "$ROOT/native/tauri/tools/parity-mac" > "$WORK/parity-mac-build.log" 2>&1 || abort "parity-mac builds" "see parity-mac-build.log"
PARITY="$ROOT/native/tauri/tools/parity-mac/.build/release/parity-mac"
swiftc -O "$WORK/terminate.swift" -o "$WORK/terminate" > "$WORK/terminate-build.log" 2>&1 || abort "the terminate helper builds"

step "the throwaway certificate and the update server on port $PORT"
CERT_SHA1="$(python3 "$SERVER" cert "$WORK/cert" | tail -n 1)" || abort "the throwaway certificate"
# Trusted for TLS to 127.0.0.1 only (a leaf, not a CA: update-rehearsal-server.py).
sudo security add-trusted-cert -d -r trustRoot -p ssl -s 127.0.0.1 -k /Library/Keychains/System.keychain "$WORK/cert/cert.pem"     || abort "the throwaway certificate is trusted for 127.0.0.1"
# http.server's bind asks getfqdn for its name, a reverse lookup a runner may answer only after its DNS times out (the Step 0 spike's
# 35 s): answered here without DNS.
cat > "$WORK/serve.py" <<'PY'
import runpy, socket, sys
socket.getfqdn = lambda name="": name or "localhost"
sys.argv = sys.argv[1:]
runpy.run_path(sys.argv[0], run_name="__main__")
PY
python3 "$WORK/serve.py" "$SERVER" serve "$SERVED" --cert "$WORK/cert/cert.pem" --key "$WORK/cert/key.pem" --port-file "$WORK/server.port" \
    --log "$WORK/server.log" --port "$PORT" > "$WORK/server.out" 2> "$WORK/server.err" &
SERVER_PID=$!
wait_for 60 test -s "$WORK/server.port" || abort "the update server listens" "$(tail -c 600 "$WORK/server.err")"
# The server holds its key in memory once it listens: the file goes.
rm -f "$WORK/cert/key.pem"
cp "$RUST_ZIP" "$SERVED/MightyCode-macos.zip"
serve_manifest() { # version
    rm -f "$SERVED/latest.json" "$SERVED/latest.unsigned.json"
    MIGHTY_SIGNER=cryptography python3 "$ROOT/scripts/make-update-manifest.py" --macos "$SERVED/MightyCode-macos.zip" --base-url "$BASE" \
        --out "$SERVED/latest.json" --sign-key "$KEY" --version "$1" > "$WORK/manifest-$1.log" 2>&1
    local verdict
    verdict="$("$PARITY" manifest "$SERVED/latest.json" "$PUBLIC_KEY")"
    check "Swift's AppUpdateManifest.parse reads the served manifest as $1 with its Mac package" \
        "$(holds python3 -c 'import json,sys; v=json.loads(sys.argv[1]); sys.exit(0 if v.get("verdict")=="ok" and v.get("version")==sys.argv[2] and v["macos"]["url"]==sys.argv[3] and v.get("signed") else 1)' "$verdict" "$1" "$BASE/MightyCode-macos.zip")" "$verdict"
}
serve_manifest "$RUST_VERSION"
check "the server answers over the trusted certificate" "$(holds curl -fsS -o /dev/null "$BASE/latest.json")"

step "the Swift app from $FROM, stamped with the throwaway key and $BASE/latest.json"
WORKTREE="$WORK/src"
git -C "$ROOT" worktree add --detach "$WORKTREE" "$FROM" > /dev/null 2>&1 || abort "the $FROM worktree"
SWIFT_APP="$WORK/swift/MightyCode.app"
MIGHTY_UPDATE_URL="$BASE/latest.json" MIGHTY_UPDATE_PUBLIC_KEY="$PUBLIC_KEY" MIGHTY_CODESIGN_IDENTITY=- MIGHTY_MACOS_APP_PATH="$SWIFT_APP" \
    MIGHTY_BUILD_SCRATCH="$WORK/swift-build" bash "$WORKTREE/scripts/build-macos.sh" > "$WORK/swift-build.log" 2>&1 \
    || { tail -n 40 "$WORK/swift-build.log"; abort "the $FROM app builds with the stamp" "see swift-build.log"; }
check "the $FROM app is $FROM_VERSION with the local address" \
    "$([ "$(plist "$SWIFT_APP" CFBundleShortVersionString)" = "$FROM_VERSION" ] && [ "$(plist "$SWIFT_APP" MightyUpdateManifestURL)" = "$BASE/latest.json" ]; verdict)"

step "$FROM updates itself on quit"
ditto "$SWIFT_APP" "$APP"
make_profile "$WORK/profile"
# The setup sheet (no agent CLI on the runner) would come up at launch, and AppKit sets a quit aside under a sheet.
defaults write $DOMAIN setup.neverShow -bool true
defaults write $DOMAIN appUpdate.automatic -bool true
defaults delete $DOMAIN appUpdate.lastCheck > /dev/null 2>&1
STAGED="$WORK/profile/updates/$RUST_VERSION/staged/MightyCode.app"
"$EXE" --profile "$WORK/profile" > "$WORK/swift-app.log" 2>&1 &
SWIFT_PID=$!
wait_for 300 ready_or_gone "$STAGED/Contents/MacOS/MightyCode" "$SWIFT_PID"
check "the $FROM app staged the update by itself" "$(holds test -x "$STAGED/Contents/MacOS/MightyCode")" "$STAGED"
# Staging ends with the phase ready (the install on quit's condition) right after the bundle is checked.
sleep 5
check "the $FROM app quits through terminate() with no consent asked" "$(holds quit "$SWIFT_PID" 60 swift)"
installed_checks "$RUST_VERSION" "$WORK/profile" "$FROM → $RUST_VERSION"
check "the server was asked for latest.json and the zip" "$(grep -q 'GET /latest.json' "$WORK/server.log" && grep -q 'GET /MightyCode-macos.zip' "$WORK/server.log"; verdict)"
check "--verify-resources holds in the installed app" "$(holds verify_resources "$EXE")"

step "the installed Rust app passes the smoke"
pwsh -NoProfile -File "$ROOT/native/tauri/scripts/smoke.ps1" -Exe "$EXE" -NoRuns -Out "$WORK/smoke"
SMOKE_EXIT=$?
check "smoke passed" "$(holds [ "$SMOKE_EXIT" = 0 ])" "exit $SMOKE_EXIT"

step "the Rust app reads the rehearsal profile (read-only)"
STATE_BEFORE="$(shasum -a 256 "$WORK/profile/workspace-state.json" | awk '{print $1}')"
HOST_LOG="$WORK/read-only-logs/host.log"
"$EXE" --profile "$WORK/profile" --read-only --log-dir "$WORK/read-only-logs" > "$WORK/read-only-app.log" 2>&1 &
READER_PID=$!
wait_for 120 written_or_gone "$HOST_LOG" ' start ' "$READER_PID"
sleep 5
START_LINE="$(grep -m 1 ' start ' "$HOST_LOG" 2>/dev/null)"
echo "  host.log: $START_LINE"
check "the Rust app loaded the 0.4.x state with its workspace, writes off" \
    "$(holds python3 -c 'import re,sys; l,v=sys.argv[1:]; sys.exit(0 if f"start {v} " in l and "load state (1 workspaces" in l and l.rstrip().endswith("writes off") else 1)' "$START_LINE" "$RUST_VERSION")" "$START_LINE"
check "the Rust app ran until quit" "$(holds kill -0 "$READER_PID")"
check "the read-only Rust app quits through terminate()" "$(holds quit "$READER_PID" 60 read-only)"
check "the read-only run left the state file as it was" "$(holds [ "$(shasum -a 256 "$WORK/profile/workspace-state.json" | awk '{print $1}')" = "$STATE_BEFORE" ])"

step "the relaunch path: $FROM again, then Swift's install script for \"restart now\""
rm -rf "$APP"
ditto "$SWIFT_APP" "$APP"
check "$FROM installed again" "$([ "$(plist "$APP" CFBundleShortVersionString)" = "$FROM_VERSION" ] && [ -z "$(plist "$APP" MightyEngine)" ]; verdict)"
mkdir -p "$WORK/relaunch/staged"
ditto -x -k "$RUST_ZIP" "$WORK/relaunch/staged"
sh -c 'exit 0' & GONE=$!; wait "$GONE"
python3 - "$WORK/relaunch/cases.json" "$WORK/relaunch/staged/MightyCode.app" "$APP" "$GONE" <<'PY'
import json, sys
path, staged, destination, pid = sys.argv[1:]
json.dump({"cases": [{"name": "restart", "stagedApp": staged, "destination": destination, "runningExecutable": "MightyCode",
                      "pid": int(pid), "relaunch": True}]}, open(path, "w"))
PY
"$PARITY" install-script "$WORK/relaunch/cases.json" | python3 -c 'import json,sys; sys.stdout.write(json.load(sys.stdin)["cases"][0]["script"])' > "$WORK/relaunch/install.sh"
chmod 700 "$WORK/relaunch/install.sh"
# Where the relaunched app logs (startup.rs `log_dir`): an app outside /Applications and ~/Applications writes nothing (D7), so its
# log goes to this machine's local folder for the default profile, not the profile's tauri-logs (which a writing run uses).
DEFAULT_LOG="$LOCAL_LOGS/default/host.log"
LOG_LENGTH="$(stat -f%z "$DEFAULT_LOG" 2>/dev/null || echo 0)"
# As AppUpdateService.launchInstaller runs it: /bin/sh, PATH and HOME only, its output in install.log.
env -i PATH=/usr/bin:/bin HOME="$HOME" /bin/sh "$WORK/relaunch/install.sh" < /dev/null > "$WORK/relaunch/install.log" 2>&1
SCRIPT_EXIT=$?
echo "  install.log: $(tr '\n' ' ' < "$WORK/relaunch/install.log" | cut -c1-600)"
check "the install script exited with 0" "$(holds [ "$SCRIPT_EXIT" = 0 ])" "exit $SCRIPT_EXIT"
check "install.log says Installed: $APP" "$(holds grep -qx "Installed: $APP" "$WORK/relaunch/install.log")"
check "the relaunch installed the Rust app at $RUST_VERSION" "$([ "$(plist "$APP" MightyEngine)" = tauri ] && [ "$(plist "$APP" CFBundleShortVersionString)" = "$RUST_VERSION" ]; verdict)"
new_start_line() { [ -f "$DEFAULT_LOG" ] && tail -c +$((LOG_LENGTH + 1)) "$DEFAULT_LOG" | grep -m 1 " start $RUST_VERSION "; }
wait_for 120 new_start_line > /dev/null
STARTED="$(new_start_line)"
echo "  host.log: $STARTED"
check "the relaunched Rust app started on the default profile and wrote its host log"     "$(holds python3 -c 'import sys; l, p = sys.argv[1:]; sys.exit(0 if f"profile {p} " in l else 1)' "$STARTED" "$DEFAULT_PROFILE")" "$STARTED"
check "the relaunched app, outside /Applications, writes nothing (D7)" "$(holds python3 -c 'import sys; sys.exit(0 if sys.argv[1].rstrip().endswith("writes off") else 1)' "$STARTED")"
RELAUNCHED="$(pgrep -f "^$EXE" | head -n 1)"
check "the relaunched app runs from the install" "$(holds [ -n "$RELAUNCHED" ])"
[ -n "$RELAUNCHED" ] && quit "$RELAUNCHED" 60 relaunched

step "Rust → Rust: $RUST_VERSION updates itself to $RUST_VERSION.1"
NEXT_VERSION="$RUST_VERSION.1"
mkdir -p "$WORK/next"
ditto -x -k "$RUST_ZIP" "$WORK/next"
plutil -replace CFBundleShortVersionString -string "$NEXT_VERSION" "$WORK/next/MightyCode.app/Contents/Info.plist"
bash "$ROOT/scripts/sign-macos.sh" "$WORK/next/MightyCode.app" - > "$WORK/next-sign.log" 2>&1 || abort "the next package is signed again (ad hoc)" "see next-sign.log"
rm -f "$SERVED/MightyCode-macos.zip"
(cd "$WORK/next" && ditto -c -k --sequesterRsrc --keepParent MightyCode.app "$SERVED/MightyCode-macos.zip")
serve_manifest "$NEXT_VERSION"
make_profile "$WORK/rust-profile"
# The Swift app's last check (shared defaults) would keep the Rust app from checking for six hours.
defaults delete $DOMAIN appUpdate.lastCheck > /dev/null 2>&1
REQUESTS_BEFORE="$(grep -c 'GET /MightyCode-macos.zip' "$WORK/server.log")"
STAGED="$WORK/rust-profile/updates/$NEXT_VERSION/staged/MightyCode.app"
"$EXE" --profile "$WORK/rust-profile" --allow-state-write --log-dir "$WORK/rust-update-logs" > "$WORK/rust-app.log" 2>&1 &
RUST_PID=$!
wait_for 300 ready_or_gone "$STAGED/Contents/MacOS/MightyCode" "$RUST_PID"
check "the Rust app staged the update by itself" "$(holds test -x "$STAGED/Contents/MacOS/MightyCode")" "$STAGED"
sleep 5
check "the Rust app quits through terminate() with no consent asked" "$(holds quit "$RUST_PID" 60 rust)"
installed_checks "$NEXT_VERSION" "$WORK/rust-profile" "$RUST_VERSION → $NEXT_VERSION"
check "the Rust app asked the server for the new zip" "$(holds [ "$(grep -c 'GET /MightyCode-macos.zip' "$WORK/server.log")" -gt "$REQUESTS_BEFORE" ])"

finish
