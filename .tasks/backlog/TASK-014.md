# TASK-014: Photo/video DAM + editorial feedback foundation

Branch: `product/dam-foundation`. Product direction: `docs/dam-feedback-blueprint.md`.

## Goal

Preserve Crush's local Rust/Tauri pipeline while adopting Reel Studio's editorial metadata and
feedback history as the basis for a photo/video DAM that can learn an owner's style.

## Acceptance

- [x] Migration adds owner-scoped photos, photo vectors, editorial annotations, explainable
      aesthetic assessments, append-only feedback events, and versioned style profiles.
- [x] Store APIs enforce media kinds, score ranges, owner isolation, and vector integrity.
- [x] Round-trip and migration tests cover photo records, Reel Studio-style editorial fields,
      pairwise preference, aesthetics, and an active learned profile.
- [x] Product blueprint distinguishes general visual quality, personal taste, semantics, and
      privacy rather than equating quality with identity recognition.
- [x] Existing store and workspace tests remain green.

## Implementation record (2026-08-28)

- Schema v2 adds photos, photo vectors, Reel Studio-compatible editorial annotations, aesthetic
  feature records, append-only feedback including pairwise preference, and versioned style profiles.
- Store triggers validate polymorphic photo/shot targets and clean feedback when media is removed.
- The first personalizer is an auditable normalized feedback centroid. Picks/rejects/ratings and
  workflow evidence produce separate personal-style affinity in mixed-media ranking. It remains a
  baseline, not a validated learned model, until Task 017 records held-out improvement.
- Store round trips, style-rank ordering, strict workspace Clippy, and workspace tests pass.

## Follow-on hard gates

Task 015 must prove real photo ingest/search. Task 017 must show held-out ranking improvement before
the UI describes the profile as learned. Real Reel Studio catalogue/media stays local and ignored.
