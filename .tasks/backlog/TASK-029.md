# TASK-029: Windows source decoding and media rendering backends

Depends: Tasks 021 and 028. Uses the portable contracts in `docs/platform-architecture.md` and the
fidelity rules in `docs/media-format-support.md`.

## Acceptance

- [ ] Bundle pinned, hash-verified Windows FFmpeg/FFprobe sidecars with a recorded configure recipe,
      source offer, license inventory, architecture, and bundle verification. Do not silently add
      GPL or redistribution-incompatible components to the declared distribution.
- [ ] Implement `MediaProbe`, `RenderBackend`, `ProcessSupervisor`, and exclusive publication for
      Windows. Cancellation terminates the complete child process tree, tracked staging is
      recoverable, and destination/source aliases are never overwritten.
- [ ] Replace hard-coded VideoToolbox selection with capability negotiation. Windows offers an
      approved software encoder baseline and optional NVENC when runtime checks pass; a failed or
      unavailable NVENC path falls back safely and records why.
- [ ] Implement and document a Windows full-resolution still decoder for each format Crush claims
      there. JPEG/PNG/TIFF retain the portable Rust path where valid; HEIC/HEIF and camera RAW are
      advertised only after real-file, color/profile, orientation, and bit-depth tests. Embedded
      previews never count as full decode.
- [ ] Apply the same versioned photo/video presets, recipe validation, color/orientation policy,
      metadata privacy policy, manifest schema, source-hash checks, and measured output tolerances
      used by Task 021.
- [ ] Run the shared golden matrix for photos, clips, mixed reels, boundary frames, audio, rotation,
      color/HDR behavior, collisions, cancellation, restart, unsupported capabilities, and source
      immutability using the software baseline.
- [ ] Record separate hardware evidence for NVENC output and forced NVENC failure/fallback. GPU
      output need not be byte-identical to software output, but both must meet the declared preset
      and verification tolerances.
