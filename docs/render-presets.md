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

## Video presets

`mp4-h264-sdr-v1` and `mov-h264-sdr-v1` are valid frozen recipe intents, but their full clip/reel
renderers and golden verification remain in progress under Task 021. The application must report
them as unavailable until crop, grade, boundary, audio, HDR/SDR and sequence semantics are all
implemented; it must not silently emit an untreated export.
