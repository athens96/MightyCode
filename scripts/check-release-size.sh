#!/bin/bash
# Release size gate for the release packages (Mac zip/dmg and self-update archives).
#
# Wrangler's `r2 object put` refuses any single object above 300 MiB (scripts/release.py
# MAX_UPLOAD_BYTES = 300 MiB), and the app refuses any self-update package above 512 MiB
# (MightyCore AppUpdate.maximumPackageBytes). The R2 single-object upload ceiling is the
# tighter cliff, so the gate here stops at 280 MiB (20 MiB headroom below the 300 MiB R2
# limit), catching growth before a build fails to upload or becomes un-installable. 0.5
# bundles no browser engine (no in-app browser; web pages open in the default browser), so
# the zip sits far below the gate; it stays a maximum, not a budget for any one part.
#
# Usage:
#   scripts/check-release-size.sh [package.zip ...]
#
# With no argument it checks release/MightyCode-macos.zip. Run it after packaging and
# before uploading anything to the release bucket (see docs/app-update.md).
set -euo pipefail

GATE_MIB=280
UPLOAD_CAP_MIB=300
UPDATER_CAP_MIB=512
MIB=1048576
GATE_BYTES=$((GATE_MIB * MIB))

PROJECT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"

file_bytes() {
  # Apparent size, which is what gets uploaded and downloaded; a sparse file still
  # costs its full length over the wire.
  if [ "$(uname -s)" = "Darwin" ]; then
    stat -f%z "$1"
  else
    stat -c%s "$1"
  fi
}

human_mib() {
  awk -v b="$1" -v m="$MIB" 'BEGIN { printf "%.1f", b / m }'
}

packages=("$@")
if [ "${#packages[@]}" -eq 0 ]; then
  packages=("$PROJECT_ROOT/release/MightyCode-macos.zip")
fi

failed=0
for path in "${packages[@]}"; do
  if [ ! -f "$path" ]; then
    echo "FAIL: $path does not exist, so its size cannot be gated" >&2
    failed=1
    continue
  fi
  bytes="$(file_bytes "$path")"
  size="$(human_mib "$bytes")"
  if [ "$bytes" -gt "$GATE_BYTES" ]; then
    echo "FAIL: $path is $size MiB, above the $GATE_MIB MiB release gate (R2 upload cap $UPLOAD_CAP_MIB MiB, updater cap $UPDATER_CAP_MIB MiB)." >&2
    echo "      Shrink the package before upload; wrangler r2 object put refuses files over $UPLOAD_CAP_MIB MiB and installs cannot update past $UPDATER_CAP_MIB MiB." >&2
    failed=1
  else
    echo "OK: $path is $size MiB, within the $GATE_MIB MiB release gate (R2 upload cap $UPLOAD_CAP_MIB MiB, updater cap $UPDATER_CAP_MIB MiB)"
  fi
done

if [ "$failed" -ne 0 ]; then
  exit 1
fi

echo "PASS: every release package is under the $GATE_MIB MiB gate"
