# TASK-015: Photo ingest + mixed-media search

Branch: `product/dam-foundation`. Depends: Task 014.

## Acceptance

- [x] `crushctl ingest` discovers JPEG and PNG photos alongside supported video files.
- [x] Photo ingest is content-hash idempotent, writes a fixed-size thumbnail and CLIP vector, and
      records dimensions, format, status, and local source path.
- [x] Search embeds the query once, ranks photos and video shots together, and identifies each
      result's media type without breaking the existing shot-only search contract.
- [x] The Tauri Library lists photos and video, photo results open a real still-image detail panel,
      and Pick/Reject/1–5 feedback is persisted locally.
- [x] A real-model integration test ingests a generated JPEG, searches it, verifies its vector and
      thumbnail, and proves a second ingest skips it.
- [x] RAW/HEIF/TIFF and production-video support are separated into Task 016 so this baseline has
      a stable, tested JPEG/PNG boundary.

## Scope note

General aesthetic/design feature extraction is Task 017. The current UI can display stored design
scores but does not invent them. Personal ranking begins adapting from explicit feedback, while
held-out validation and richer context models remain Task 018.
