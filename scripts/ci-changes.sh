#!/bin/bash
# Usage (a workflow's `changes` job): bash scripts/ci-changes.sh macos|windows
#
# Writes to $GITHUB_OUTPUT whether this run builds and tests that platform (`run`), whether it runs the shared Rust crates'
# fmt, clippy and tests on it (`rust`), and whether it captures the help screenshots (`help`, macOS only):
#   - workflow_dispatch: everything; help unless the `full` input is false (release.py dispatches a full run
#     when the build commit's push run skipped the build).
#   - pull_request: run, no help, no rust (native-tauri.yml runs the Rust checks on pull requests).
#   - push of a release commit ("(release) x.y.z: ..."): run, rust and help (the help site's pictures come from it).
#   - any other push: no help, and the platform is skipped when every changed file belongs to the other one only
#     (macOS: native/windows/ except native/windows/web/, the page both OSes ship, and relay/; Windows: native/macos/ or relay/).
#     rust runs with the platform when a changed file is one the Rust workspace builds, tests or reads (native/tauri, the page and
#     the C# and Swift sources its parity tools compile, native/contracts, locales, styles, assets, mods, docs/help, scripts/fixtures,
#     libghostty's fetch, VERSION, the two workflows and this script). Anything else, or a push whose range cannot be read (a new
#     branch, a force-push), runs both.
# Env: EVENT, BEFORE (github.event.before), MESSAGE (the head commit's message), FULL (the dispatch input), GITHUB_SHA.
set -euo pipefail

platform="${1:?macos or windows}"
case "$platform" in
  # other: the other platform's files; shared: files under `other` that this platform ships too.
  macos) other='^(native/windows/|relay/)'; shared='^native/windows/web/' ;;
  windows) other='^(native/macos/|relay/)'; shared='^$' ;;
  *) echo "unknown platform $platform" >&2; exit 2 ;;
esac
rust_paths='^(native/(tauri|windows|macos|contracts)/|locales/|styles/|assets/|mods/|docs/help/|scripts/fixtures/|scripts/fetch-ghosttykit\.sh$|scripts/ci-changes\.sh$|VERSION$|\.github/workflows/native-(windows|macos)\.yml$)'
run=true
rust=true
help=false

case "${EVENT:-push}" in
  workflow_dispatch)
    [[ "${FULL:-true}" != "false" ]] && help=true
    ;;
  pull_request)
    rust=false
    ;;
  *)
    if [[ "${MESSAGE:-}" == "(release) "* ]]; then
      help=true
    elif [[ -n "${BEFORE:-}" && ! "${BEFORE}" =~ ^0+$ ]] && git cat-file -e "${BEFORE}^{commit}" 2>/dev/null; then
      files="$(git diff --name-only "$BEFORE" "$GITHUB_SHA")"
      if [[ -n "$files" ]]; then
        mine="$( { grep -Ev "$other" <<<"$files" || true; grep -E "$shared" <<<"$files" || true; } )"
        if [[ -z "$mine" ]]; then
          run=false
        fi
        if ! grep -Eq "$rust_paths" <<<"$mine"; then
          rust=false
        fi
      fi
    fi
    ;;
esac

[[ "$run" == true ]] || rust=false
[[ "$platform" == macos ]] || help=false
echo "$platform: run=$run rust=$rust help=$help (event ${EVENT:-push})"
{
  echo "run=$run"
  echo "rust=$rust"
  echo "help=$help"
} >> "${GITHUB_OUTPUT:-/dev/stdout}"
