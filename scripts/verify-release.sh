#!/usr/bin/env bash
# Task 023 release verification: checksummed artifact + bundle integrity + deep doctor.
#
# Usage:
#   CRUSH_APP=/Applications/Crush.app scripts/verify-release.sh
#
# Prints a report with the .app SHA-256 (computed over the full bundle), build commit,
# code-signature state, sidecar/model presence, database integrity, and the plain-language
# surface the smoke checklist then reads. Passing this script is NOT release approval — the
# Task 021 render-golden review and the Task 023 clean-machine human record are separate
# gates.
set -euo pipefail

APP="${CRUSH_APP:-/Applications/Crush.app}"
CLI="${CRUSHCTL:-cargo run -q -p crush-cli --}"
DATA_DIR="${CRUSH_DATA_DIR:-}"
OUT="${RELEASE_REPORT:-/tmp/crush-release-report.txt}"

if [ ! -d "$APP" ]; then
  echo "error: $APP is not installed" >&2
  exit 1
fi

EXECUTABLE_NAME="$(defaults read "$APP/Contents/Info" CFBundleExecutable 2>/dev/null || true)"
EXECUTABLE="$APP/Contents/MacOS/$EXECUTABLE_NAME"
if [ -z "$EXECUTABLE_NAME" ] || [ ! -x "$EXECUTABLE" ]; then
  echo "error: bundle executable is missing or not executable: $EXECUTABLE" >&2
  exit 1
fi

{
  echo "Crush release verification $(date -u +%Y-%m-%dT%H:%MZ)"
  echo "app: $APP"
  echo "bundle version: $(defaults read "$APP/Contents/Info" CFBundleShortVersionString 2>/dev/null || echo unknown)"
  echo "bundle executable: $EXECUTABLE_NAME"

  # 0. Build provenance: the bundle binary must self-report the commit stamped
  #    at build time (CRUSH_BUILD_COMMIT; see scripts/package-macos.sh and
  #    docs/release.md). An artifact that cannot self-report — or that honestly
  #    reports an unstamped local build — fails visibly, never a silent
  #    "unknown".
  BUILD_COMMIT="$("$EXECUTABLE" --build-info 2>/dev/null | sed -n 's/^build commit: //p' || true)"
  if [ -z "$BUILD_COMMIT" ]; then
    echo "build commit: FAIL ($EXECUTABLE_NAME does not self-report provenance; expected a 'build commit:' line from --build-info — rebuild with scripts/package-macos.sh)"
    exit 1
  fi
  if [ "$BUILD_COMMIT" = "unknown-local" ]; then
    echo "build commit: FAIL (bundle reports 'unknown-local' — unstamped build; export CRUSH_BUILD_COMMIT when building a release artifact)"
    exit 1
  fi
  echo "build commit: $BUILD_COMMIT"

  # 1. Artifact checksum over the whole bundle (signatures excluded is optional; a raw
  #    hashing of the .app is the honest artifact digest for a signed build).
  # Hash relative paths plus file contents so the digest identifies the bundle itself,
  # independent of whether it lives in the build tree, /Applications, or a mounted DMG.
  APP_SHA256="$(
    cd "$APP"
    find . -type f -print0 | sort -z | xargs -0 shasum -a 256 | shasum -a 256 | cut -d' ' -f1
  )"
  echo "app sha256: $APP_SHA256"

  # 2. Signature state: explicit ad-hoc vs signed, and strict deep verification.
  if codesign --verify --deep --strict "$APP" 2>/dev/null; then
    CODESIGN_INFO="$(codesign -dv --verbose=2 "$APP" 2>&1 | rg 'Signature|flags' || true)"
    echo "codesign verify: PASS"
    echo "$CODESIGN_INFO" | sed 's/^/  /'
  else
    echo "codesign verify: FAIL"
    exit 1
  fi

  # 3. Both required sidecars are present and executable. A count alone can pass with two
  #    copies of the same name or with non-executable placeholders.
  for sidecar in ffmpeg ffprobe; do
    if [ ! -x "$APP/Contents/MacOS/$sidecar" ]; then
      echo "sidecars: FAIL ($sidecar is missing or not executable)"
      exit 1
    fi
  done
  echo "sidecars: PASS (ffmpeg, ffprobe)"

  # 4. Deep runtime + library integrity.
  if [ -n "$DATA_DIR" ]; then
    CRUSH_DATA_DIR="$DATA_DIR" $CLI doctor --deep
  else
    $CLI doctor --deep
  fi

  echo "report: $OUT"
} 2>&1 | tee "$OUT"

echo
echo "This report is evidence for, not approval of, a release. Continue to the"
echo "clean-machine smoke checklist in docs/smoke.md before publishing."
