# Render presets

Render recipes describe the media result, never the hardware used to produce it. The output
manifest records the actual decoder, executor and backend. A preset name is a versioned contract;
changing any behavior below requires a new preset version.

## Photo presets v1

All three presets decode the full original, apply source EXIF orientation once, then apply the
recipe's right-angle rotation, normalized crop and basic grade. The long edge is reduced to 4096
pixels when necessary and is never enlarged. Resizing uses Lanczos3.

| Preset | Container | Pixels | Compression | Alpha |
| --- | --- | --- | --- | --- |
| `jpeg-srgb-v1` | JPEG | 8-bit RGB, sRGB IEC61966-2.1 | quality 92 | rejected |
| `png-srgb-v1` | PNG | 8-bit RGB/RGBA, sRGB IEC61966-2.1 | best, adaptive filter | preserved |
| `tiff-srgb-v1` | TIFF | 8-bit RGB/RGBA, sRGB IEC61966-2.1 | lossless | preserved |

Embedded ICC and declared CICP sources are converted into sRGB; the renderer does not merely
retag their pixels. Named non-sRGB inputs without convertible ICC/CICP evidence fail. Truly
untagged pixels are treated as assumed sRGB and that assumption is explicit in the manifest.
Inputs whose declared or decoded channel depth exceeds 8 bits fail rather than being silently
reduced. A future high-depth contract will use a new preset version.

EXIF, IPTC, XMP and GPS metadata are stripped. A deterministic sRGB output profile is embedded.
Source identity and hash remain in the separate manifest, not in private image metadata.

## Grade v1

The `none` mode leaves color values unchanged after color-space conversion. `basic` supports:

- exposure: -5 to +5 EV;
- contrast: -1 to +1 around linear-light middle gray;
- saturation: 0 to 2 in linear light;
- temperature and tint: -1 to +1 bounded channel adjustments.

## Publication and manifests

Rendering occurs in a private, owner/job/attempt-marked directory on the destination filesystem.
The manifest and output are flushed and published with exclusive hard links; an existing output,
manifest, symlink or race is never overwritten. The manifest records frozen source IDs/hashes,
recipe and optional project revision, model identities, actual tool/backend versions, output
checksum, dimensions, color/depth policy and source-before/source-after hash verification.

On startup, a complete verifying publication is checksummed and finalized in SQLite. Unverified
staging is removed only when its marker exactly matches the tracked owner, job, attempt and
destination. Unknown directories and mismatched user files are preserved. Interrupted jobs become
explicitly failed and can start a new numbered attempt.

## Video clip presets v1

`mp4-h264-sdr-v1` and `mov-h264-sdr-v1` always encode boundary-sensitive clip recipes; they never
stream-copy and then imply that crop, grade, audio or exact boundaries were applied. Both produce
8-bit H.264 `yuv420p`, limited-range BT.709 with AAC source audio when present and requested, or no
audio for mute. The MP4 and MOV containers carry `avc1` and fast-start metadata.

Normalized crops are interpreted in displayed/autorotated source space and quantized outward to
even chroma-aligned pixels. Basic grade controls use the same declared ranges as photo v1 and map
to explicit FFmpeg filters. The v1 backend accepts known 8-bit BT.709 SDR or wholly untagged input;
untagged input is recorded as assumed BT.709. HDR, wide-gamut, full-range, unknown-depth and
greater-than-8-bit sources fail until a real conversion/tone-map policy is versioned. Uncropped odd
dimensions also fail rather than being silently padded.

The actual backend and encoder are capability results in the manifest. The current macOS provider
is VideoToolbox; it is not part of recipe intent. Reels remain in progress under Task 021. Until
their ordered sequence, photo holds, transitions, crops, grades, captions and audio policies are
implemented together, the application must report reel rendering as unavailable rather than emit
an incomplete export.
