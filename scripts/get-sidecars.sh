#!/bin/sh
set -eu

VERSION="9.0.1"
SOURCE_URL="https://ffmpeg.org/releases/ffmpeg-${VERSION}.tar.xz"
SOURCE_SHA256="cf38e0e28c7e5605942c4a77755349b0145804a397af37eb1fb4c77cb237f635"
FFMPEG_SHA256="73a2114706389cad8a87890bb77b0dbe2031647acf25d6dcf48baf32fae29d0d"
FFPROBE_SHA256="da8681f30f30c6b344a2e40899b5c5669d0e501712c1867305a5027b3d6380d8"

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPOSITORY_DIR=$(dirname "$SCRIPT_DIR")
DESTINATION_DIR="$REPOSITORY_DIR/sidecars"
FORCE=0

if [ "$#" -eq 1 ] && [ "$1" = "--force" ]; then
  FORCE=1
elif [ "$#" -ne 0 ]; then
  echo "usage: $0 [--force]" >&2
  exit 64
fi

if [ "$(uname -s)" != "Darwin" ] || [ "$(uname -m)" != "arm64" ]; then
  echo "error: the pinned sidecars build only on arm64 macOS" >&2
  exit 1
fi

hash_file() {
  shasum -a 256 "$1" | awk '{print $1}'
}

verify_binary() {
  binary=$1
  expected_hash=$2
  [ -x "$binary" ] || return 1
  file "$binary" | grep -q "Mach-O 64-bit executable arm64" || return 1
  [ "$(hash_file "$binary")" = "$expected_hash" ] || return 1
  version_output=$("$binary" -version 2>&1) || return 1
  case "$version_output" in
    *"ffmpeg version ${VERSION}"*|*"ffprobe version ${VERSION}"*) ;;
    *) return 1 ;;
  esac
  case "$version_output" in
    *--enable-gpl*|*--enable-nonfree*) return 1 ;;
  esac
}

if [ "$FORCE" -eq 0 ] \
  && verify_binary "$DESTINATION_DIR/ffmpeg" "$FFMPEG_SHA256" \
  && verify_binary "$DESTINATION_DIR/ffprobe" "$FFPROBE_SHA256"; then
  echo "FFmpeg ${VERSION} sidecars already match the pinned LGPL build"
  exit 0
fi

BUILD_DIR=$(mktemp -d "${TMPDIR:-/tmp}/crush-sidecars.XXXXXX")
trap 'rm -rf "$BUILD_DIR"' EXIT HUP INT TERM
ARCHIVE="$BUILD_DIR/ffmpeg-${VERSION}.tar.xz"

echo "Fetching $SOURCE_URL"
curl --fail --location --retry 3 --proto '=https' --tlsv1.2 --output "$ARCHIVE" "$SOURCE_URL"
actual_source_hash=$(hash_file "$ARCHIVE")
if [ "$actual_source_hash" != "$SOURCE_SHA256" ]; then
  echo "error: source sha256 mismatch: expected $SOURCE_SHA256, got $actual_source_hash" >&2
  exit 1
fi

tar -C "$BUILD_DIR" -xf "$ARCHIVE"
SOURCE_DIR="$BUILD_DIR/ffmpeg-${VERSION}"
cd "$SOURCE_DIR"
./configure \
  --arch=arm64 \
  --target-os=darwin \
  --cc=clang \
  --disable-shared \
  --enable-static \
  --disable-doc \
  --disable-debug \
  --disable-ffplay \
  --disable-autodetect \
  --enable-videotoolbox \
  --enable-audiotoolbox \
  --enable-pic

JOBS=$(/usr/sbin/sysctl -n hw.logicalcpu 2>/dev/null || echo 4)
make -j "$JOBS" ffmpeg ffprobe

for binary in ffmpeg ffprobe; do
  version_output=$("$SOURCE_DIR/$binary" -version)
  case "$version_output" in
    *--enable-gpl*|*--enable-nonfree*)
      echo "error: $binary unexpectedly enables GPL or nonfree components" >&2
      exit 1
      ;;
  esac
done

actual_ffmpeg_hash=$(hash_file "$SOURCE_DIR/ffmpeg")
actual_ffprobe_hash=$(hash_file "$SOURCE_DIR/ffprobe")
if [ "$actual_ffmpeg_hash" != "$FFMPEG_SHA256" ] || [ "$actual_ffprobe_hash" != "$FFPROBE_SHA256" ]; then
  echo "error: built sidecar hashes differ from the pinned Apple clang 17 build" >&2
  echo "ffmpeg expected=$FFMPEG_SHA256 actual=$actual_ffmpeg_hash" >&2
  echo "ffprobe expected=$FFPROBE_SHA256 actual=$actual_ffprobe_hash" >&2
  exit 1
fi

mkdir -p "$DESTINATION_DIR"
install -m 0755 "$SOURCE_DIR/ffmpeg" "$DESTINATION_DIR/ffmpeg"
install -m 0755 "$SOURCE_DIR/ffprobe" "$DESTINATION_DIR/ffprobe"
echo "Installed verified FFmpeg ${VERSION} LGPL sidecars in $DESTINATION_DIR"
