#!/usr/bin/env bash
# Task 023 one-command macOS packaging: provenance-stamped .app + DMG, DMG
# checksum, and unmistakable ad-hoc labeling.
#
# Usage:
#   scripts/package-macos.sh
#
# Requires the pinned cargo-tauri 2.11.4 and the hash-verified sidecars in
# sidecars/ (digests recorded in .github/workflows/ci.yml). Signing follows
# tauri.macos.conf.json: ad-hoc ("-") unless a Developer ID identity is
# configured. Ad-hoc output is labeled with a BUILD-ADHOC.txt marker next to
# the DMG (see docs/release.md). A future tagged CI release workflow must
# export CRUSH_BUILD_COMMIT the same way this script does before
# `cargo tauri build`.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

# 1. Stamp honest build provenance: short commit, plus -dirty when tracked
#    files are modified (same semantics as `git describe --dirty`; untracked
#    files do not affect the build). No git (or no commits) -> unknown-local;
#    never a fake commit.
COMMIT=""
if git rev-parse --verify HEAD >/dev/null 2>&1; then
  COMMIT="$(git rev-parse --short HEAD)"
  if [ -n "$(git status --porcelain --untracked-files=no)" ]; then
    COMMIT="${COMMIT}-dirty"
  fi
fi
export CRUSH_BUILD_COMMIT="${COMMIT:-unknown-local}"
echo "==> build commit: $CRUSH_BUILD_COMMIT"

# 2. Build the .app and DMG. CI=true per TASK-023 for headless DMG creation;
#    the DMG target is passed explicitly because tauri.conf.json lists only "app".
CI=true cargo tauri build --bundles app,dmg

# Absolute paths: `defaults read` treats relative paths as domain names.
APP="$ROOT/target/release/bundle/macos/Crush.app"
DMG_DIR="$ROOT/target/release/bundle/dmg"
EXECUTABLE_NAME="$(defaults read "$APP/Contents/Info" CFBundleExecutable 2>/dev/null || true)"
if [ -z "$EXECUTABLE_NAME" ] || [ ! -x "$APP/Contents/MacOS/$EXECUTABLE_NAME" ]; then
  echo "error: expected bundle not found or incomplete at $APP" >&2
  exit 1
fi
VERSION="$(defaults read "$APP/Contents/Info" CFBundleShortVersionString)"
shopt -s nullglob
DMGS=("$DMG_DIR"/Crush_"$VERSION"_*.dmg)
shopt -u nullglob
if [ "${#DMGS[@]}" -ne 1 ]; then
  echo "error: expected exactly one Crush_${VERSION}_*.dmg in $DMG_DIR, found ${#DMGS[@]}" >&2
  exit 1
fi
DMG="${DMGS[0]}"

# 3. Checksum the DMG next to itself (release-record format used in docs/smoke.md).
( cd "$DMG_DIR" && shasum -a 256 "$(basename "$DMG")" > "$(basename "$DMG").sha256" )

# 4. Label ad-hoc builds unmistakably. A stale marker from an earlier ad-hoc
#    build is removed first so a signed build is never mislabeled.
MARKER="$DMG_DIR/BUILD-ADHOC.txt"
rm -f "$MARKER"
if codesign -dv --verbose=2 "$APP" 2>&1 | rg -q 'Signature=adhoc'; then
  {
    echo "This DMG is an AD-HOC signed build. It is NOT notarized and is NOT"
    echo "suitable for distribution beyond local development. Install it via"
    echo "right-click -> Open (see docs/release.md)."
    echo
    echo "built: $(date -u +%Y-%m-%dT%H:%MZ)"
    echo "build commit: $CRUSH_BUILD_COMMIT"
    echo "dmg: $(basename "$DMG")"
    echo "dmg sha256: $(cut -d' ' -f1 "$DMG.sha256")"
    echo
    echo "Signature evidence (codesign -dv --verbose=2 on the .app):"
    codesign -dv --verbose=2 "$APP" 2>&1 | rg 'Identifier|CodeDirectory|Signature|TeamIdentifier' || true
  } > "$MARKER"
  echo "==> ad-hoc build labeled: $MARKER"
else
  echo "==> signed build (not ad-hoc); no BUILD-ADHOC marker"
fi

echo
echo "Artifacts:"
echo "  $DMG"
echo "  $DMG.sha256"
if [ -f "$MARKER" ]; then
  echo "  $MARKER"
fi
echo
echo "Next: verify the bundle:"
echo "  CRUSH_APP=\"$APP\" CRUSHCTL=\"$ROOT/target/release/crushctl\" scripts/verify-release.sh"
