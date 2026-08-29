# TASK-016: RAW/photo formats + production-video source support

Branch: `task/16-source-fidelity`. Depends: Task 015.

## Goal

Ingest professional still and video sources without changing originals. Produce searchable,
color-aware working proxies and retain enough metadata to render trustworthy derivatives later.

## Acceptance

- [x] Publish a fixture-backed support matrix for JPEG/PNG, HEIC/HEIF, TIFF, DNG, CR2/CR3, NEF,
      ARW, ORF, RAF, and RW2; unsupported variants report a precise reason.
- [x] Apply EXIF orientation and retain capture time, dimensions, camera/lens, exposure, GPS policy,
      embedded preview provenance, bit depth, and ICC/color metadata.
- [x] Generate deterministic thumbnails and working proxies without modifying originals; content
      hash and re-ingest behavior remain idempotent.
- [x] Probe MOV/MP4/M4V/MXF and common ProRes, H.264, and H.265 acquisition media; generate a proxy
      only when native decode/edit cost requires one.
- [x] Record an explicit decoder and licensing decision for BRAW, R3D, and ProRes RAW. Do not claim
      support through embedded-preview extraction alone.
- [x] Packaged-app test indexes a representative still/video fixture set and records time, memory,
      failures, and visual orientation/color checks.

## Evidence

- Schema v3 keeps one-to-one photo/video source metadata and safe proxy paths without changing the
  stable base media records.
- `docs/media-format-support.md` and `fixtures/source-formats/support-matrix.json` define the
  machine-checked support boundary; captured ImageIO capabilities remain runtime-conditional.
- `crates/pipeline/tests/source_fidelity.rs` covers real TIFF/HEIC decode, all EXIF orientations,
  deterministic ICC-preserving derivatives, corrupt RAW errors, ProRes/H.264/DNxHD/HEVC probes,
  and the HEVC proxy path.
- The representative packaged-pipeline fixture indexed four stills and four production-video cases
  with zero failures in 11,084 ms and 985,481,216 peak resident bytes; details are recorded in
  `fixtures/source-formats/validation-report-task016.json`.
- `cargo tauri build --debug` produced and ad-hoc signed `Crush.app`; `codesign --verify --deep
  --strict` passed and its bundled FFprobe successfully probed the MP4 fixture.

## Boundary

Camera RAW support is conditional on the installed macOS ImageIO version and the exact camera
variant completing a full render. The matrix does not generalize from embedded previews. BRAW, R3D,
and ProRes RAW remain disabled pending approved SDK integration and redistribution licensing.
