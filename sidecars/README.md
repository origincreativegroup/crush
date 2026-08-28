# sidecars

Static `ffmpeg` and `ffprobe` builds for arm64 macOS live here (git-ignored).
Fetch or reproduce them with `scripts/get-sidecars.sh`. The script verifies the official source
archive, exact output hashes, architecture, version, and that neither `--enable-gpl` nor
`--enable-nonfree` is present. Pass `--force` to rebuild even when the installed pair is valid.

The binaries are an **LGPL 2.1-or-later** build. See `SOURCES.md` for source, configuration, hashes,
and distribution notes.
