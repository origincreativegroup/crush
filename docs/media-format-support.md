# Media source support and fidelity policy

This document extends the engineering architecture in `project-blueprint.md`; it does not replace
the product plan. Source handling is infrastructure for cataloging, general visual-quality
assessment, editorial selection, and optional user-style learning.

## Still images

| Source | Decoder | Status | Derivative provenance |
|---|---|---|---|
| JPEG, PNG, TIFF | pinned `image-rs` build | Bundled full decode on every supported platform | `decoded_original` |
| HEIC/HEIF | macOS ImageIO (`sips`) | Accepted only when the installed OS advertises the extension and completes a full render | `full_render` |
| DNG, CR2/CR3, NEF, ARW, ORF, RAF, RW2 | macOS ImageIO (`sips`) | Camera/OS conditional; accepted only after a runtime capability check and successful full render | `full_render` |

An extension is never treated as proof of decodability. Every ImageIO-backed file is checked
against the formats reported by the installed OS, then fully rendered. A failure includes the
extension and the decoder that was unavailable or failed. Embedded previews are not used to claim
RAW support.

Crush retains the source content hash, byte size, dimensions, original EXIF orientation, capture
time, camera and lens, exposure fields, bit depth where reported, EXIF color space, and ICC profile
name/hash where the decoder exposes them. Working proxies and thumbnails have normalized
orientation. Capture times without an EXIF UTC offset are explicitly marked as assumed UTC.

GPS privacy is presence-only by default: Crush records that GPS metadata existed, but does not
persist latitude, longitude, altitude, or a human-readable location.

When a decoder exposes an ICC profile, Crush attaches the same profile to the resized JPEG proxy
and thumbnail and records its SHA-256. ImageIO-rendered RAW/HEIF proxies retain the rendered working
profile. This avoids silently relabeling source-profile RGB values as sRGB.

## Video

MOV, MP4, M4V, and MXF are containers, not blanket codec promises. The bundled LGPL FFprobe path
records the actual codec/profile, pixel format, bit depth, color space/primaries/transfer/range, and
rotation before processing.

| Codec/source | Policy |
|---|---|
| ProRes (non-RAW), DNxHD/DNxHR | Direct processing; these are edit-friendly acquisition codecs |
| H.264 up to 3840×2160, 60 fps, and 8-bit | Direct processing |
| Higher-cost H.264 and unknown codecs | Generate a 1080p-or-smaller H.264 working proxy |
| H.265/HEVC | Generate a seek-friendly H.264 working proxy |
| BRAW | Disabled: Blackmagic RAW SDK distribution/licensing has not been approved |
| R3D | Disabled: RED SDK integration and redistribution licensing have not been approved |
| ProRes RAW | Disabled: the bundled LGPL FFmpeg build is not an approved full decoder |

BRAW, R3D, and ProRes RAW errors explicitly state that embedded-preview extraction would not count
as full support. Source files are never rewritten. Video proxies have content-derived stable paths,
are used for visual analysis/playback, and the original remains the source for audio and final clip
export.

## Evidence

`fixtures/source-formats/support-matrix.json` is the machine-checked contract. The accompanying
`macos-imageio-task-016.txt` is the relevant subset of `sips --formats` captured on the Task 016
test Mac. Runtime checks remain authoritative because OS and camera support can change.
