#!/usr/bin/env bash
# Usage: scripts/site/sync-help.sh
#
# Copies the built help (docs/help/site, from `node scripts/help/build.mjs [--shots <dir>]`) into the website as
# site/help/<lang>/, so the site's 문서 links show the same pages as the app's help. Run it after a help build, then
# deploy the site from its own folder: `cd site && npx wrangler deploy`.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
SRC="$ROOT/docs/help/site"
DEST="$ROOT/site/help"
if [ ! -f "$SRC/ko/index.html" ]; then
  echo "sync-help: $SRC/ko/index.html not found; run node scripts/help/build.mjs first" >&2
  exit 1
fi
node "$ROOT/scripts/help/build.mjs" --check >/dev/null
mkdir -p "$DEST"
rsync -a --delete "$SRC/" "$DEST/"
echo "sync-help: copied $(find "$DEST" -type f | wc -l | tr -d ' ') files to site/help"
