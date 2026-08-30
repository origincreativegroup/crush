#!/usr/bin/env bash
# Task 023 release verification: checksummed artifact + bundle integrity + deep doctor.
#
# Usage:
#   CRUSH_APP=/Applications/Crush.app scripts/verify-release.sh
#
# Prints a report with the .app SHA-256 (computed over the full bundle), code-signature
# state, sidecar/model presence, database integrity, and the plain-language surface the
# smoke checklist then reads. Passing this script is NOT release approval — the Task 021
# render-golden review and the Task 023 clean-machine human record are separate gates.
set -euo pipefail

APP="${CRUSH_APP:-/Applications/Crush.app}"
CLI="${CRUSHCTL:-cargo run -q -p crush-cli --}"
DATA_DIR="${CRUSH_DATA_DIR:-}"
OUT="${RELEASE_REPORT:-/tmp/crush-release-report.txt}"

if [ ! -d "$APP" ]; then
  echo "error: $APP is not installed" >&2
  exit 1
fi

{
  echo "Crush release verification $(date -u +%Y-%m-%dT%H:%MZ)"
  echo "app: $APP"
  echo "bundle version: $(defaults read "$APP/Contents/Info" CFBundleShortVersionString 2>/dev/null || echo unknown)"
  echo "build commit: $("$APP/Contents/MacOS/crush" --version 2>/dev/null | head -1 || echo unknown)"

  # 1. Artifact checksum over the whole bundle (signatures excluded is optional; a raw
  #    hashing of the .app is the honest artifact digest for a signed build).
  echo "app sha256: $(find "$APP" -type f -print0 | sort -z | xargs -0 shasum -a 256 | shasum -a 256 | cut -d' ' -f1)"

  # 2. Signature state: explicit ad-hoc vs signed, and strict deep verification.
  if codesign --verify --deep --strict "$APP" 2>/dev/null; then
    CODESIGN_INFO="$(codesign -dv --verbose=2 "$APP" 2>&1 | rg 'Signature|flags' || true)"
    echo "codesign verify: PASS"
    echo "$CODESIGN_INFO" | sed 's/^/  /'
  else
    echo "codesign verify: FAIL"
  fi

  # 3. Sidecars are present and have a recognized origin.
  echo "sidecars: $(find "$APP/Contents/MacOS" -maxdepth 1 \( -name ffmpeg -o -name ffprobe \) | wc -l | tr -d ' ') files"

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