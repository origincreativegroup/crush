# TASK-017: General strong-shot and explainable aesthetic analysis

Branch: `task/17-strong-shot-analysis`. Depends: Task 016.

Depends: Task 016.

This is the independent cold-start judgment layer. It must work without user examples, feedback,
identity recognition, or an active personal profile.

## Acceptance

- [x] Score technical quality separately: focus, blur, exposure, clipping, noise, compression,
      resolution, motion stability, and duplicate/near-duplicate confidence.
- [x] Score composition/design separately: hierarchy, balance, subject placement, negative space,
      leading lines, symmetry, contrast, color harmony, and crop potential.
- [x] Add moment/story and sequence features for expressions, gestures, action, novelty, pacing, and
      repetition without treating identity recognition as quality.
- [x] Store component values, confidence, model version, and plain-language evidence.
- [x] Human-reviewed still and video fixtures catch regressions and calibration drift.
- [x] A no-profile acceptance test produces useful strong-shot ordering and plain-language reasons.

## Evidence

- `crush-stage-aesthetic` computes deterministic pixel measurements and optional identity-free CLIP
  concept comparisons without reading feedback or a style profile.
- Schema v4 stores technical, composition, moment, sequence, risk, confidence, explanation, and
  model-version fields while preserving the existing design-score API.
- Photo ingest scores source-resolution-aware stills; video analysis uses representative frames,
  two boundary-safe within-shot samples, adjacent-shot repetition evidence, and an explicit
  resumable `analyze` job stage.
- Re-ingest checks `strong-shot-v1` and backfills missing/stale video assessments without rebuilding
  an already indexed source.
- `fixtures/aesthetic/human-reviewed-v1.json` and the human-review integration test bound component
  drift across still and video controls. The no-profile unit test verifies useful ordering and
  identity-free plain-language evidence.
- Mixed-media search uses a small centered general-quality adjustment; the UI displays general,
  technical, design, and moment scores separately from editorial and future personal-style scores.
- `cargo test --workspace`, strict workspace clippy, the rendered photo/video detail harness, and a
  debug `Crush.app` build passed; the app and bundled FFmpeg/FFprobe satisfy strict codesign
  verification.

## Boundary

The first model is an auditable candidate ranker, not an autonomous publish decision. Temporal
frame difference is imperfect for intentional camera moves, and CLIP concept comparison is not a
dedicated expression/pose detector. These limits are explicit in `docs/strong-shot-analysis.md` and
must be revisited through versioned calibration rather than hidden score changes.
