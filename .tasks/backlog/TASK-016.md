# TASK-016: RAW/photo formats + production-video source support

Depends: Task 015. This is the next implementation milestone.

## Goal

Ingest professional still and video sources without changing originals. Produce searchable,
color-aware working proxies and retain enough metadata to render trustworthy derivatives later.

## Acceptance

- [ ] Publish a fixture-backed support matrix for JPEG/PNG, HEIC/HEIF, TIFF, DNG, CR2/CR3, NEF,
      ARW, ORF, RAF, and RW2; unsupported variants report a precise reason.
- [ ] Apply EXIF orientation and retain capture time, dimensions, camera/lens, exposure, GPS policy,
      embedded preview provenance, bit depth, and ICC/color metadata.
- [ ] Generate deterministic thumbnails and working proxies without modifying originals; content
      hash and re-ingest behavior remain idempotent.
- [ ] Probe MOV/MP4/M4V/MXF and common ProRes, H.264, and H.265 acquisition media; generate a proxy
      only when native decode/edit cost requires one.
- [ ] Record an explicit decoder and licensing decision for BRAW, R3D, and ProRes RAW. Do not claim
      support through embedded-preview extraction alone.
- [ ] Packaged-app test indexes a representative still/video fixture set and records time, memory,
      failures, and visual orientation/color checks.

