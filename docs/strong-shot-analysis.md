# General strong-shot analysis

Task 017 adds the cold-start judgment layer described in the DAM feedback blueprint. It is a
separate, resumable pipeline stage and does not read a style profile, feedback history, named
identity, or prior work. Task 018 may learn a user-specific residual later; it must not replace
these general scores.

## Evidence groups

Every photo and video shot stores three independently inspectable groups plus an overall score:

- **Technical:** focus, blur control, exposure, clipping control, noise control, compression
  quality, source resolution, video motion stability, and duplicate confidence.
- **Composition/design:** hierarchy, balance, subject placement, negative space, leading-line
  coherence, symmetry, contrast, color harmony, crop potential, and visual clarity.
- **Moment/sequence:** identity-free CLIP comparisons for expression, gesture, action, and story;
  within-shot temporal action; neighboring-shot novelty/repetition; and shot pacing.

All values are normalized to `[0, 1]`. Higher is better except `duplicate_confidence` and
`repetition_risk`, whose names explicitly describe risk. Technical and design groups remain
separate even when the overall score blends them. Search applies only a small centered general
quality adjustment, so semantic relevance remains primary and personal style remains separately
reported.

## Explanations and confidence

`aesthetic_assessments.explanation_json` is versioned evidence, not a hidden rationale. It records
a plain-language summary, strengths, cautions, group components, source context, semantic
confidence, `independent_of_profile: true`, and `identity_used: false`. The photo and shot detail
views show the overall, technical, design, and moment groups plus the summary.

Pixel measurements remain available without the CLIP model. When semantic evidence is unavailable
or inconclusive, the scorer stores neutral moment components and says that semantic evidence was
unavailable instead of claiming to recognize an expression or story.

## Video sampling and recovery

Representative thumbnails drive design scoring. Two boundary-safe frames inside each shot provide
motion/action evidence, while adjacent shot thumbnails provide novelty and repetition evidence.
Those inputs are deliberately separate so a stable shot is not automatically called a duplicate.

Analysis jobs use the `analyze` job stage. A failed job restores the video to its completed embed
state. Re-ingesting an already indexed video checks the assessment model version and backfills only
missing or stale analysis rather than rebuilding the asset.

## Calibration evidence

`fixtures/aesthetic/human-reviewed-v1.json` records reviewer rationale and accepted component
windows for both still and video-shot cases. `crates/stage-aesthetic/tests/human_review.rs` fails on
calibration drift. The synthetic video fixture is explicitly a control: pixel heuristics may find
it crisp and balanced, but only semantic evidence may judge its story value.

The no-profile unit acceptance constructs a clear, deliberately composed still and a flat clipped
still, verifies the useful ordering, and verifies plain-language identity-free reasons. Pipeline
fixtures additionally prove persisted assessments for photos and shots, video motion sampling,
idempotence, and model-version backfill.

## Known limits

This first baseline is auditable rather than omniscient. Raw frame differences cannot reliably
separate every intentional pan from camera shake, and CLIP concepts are weaker than a dedicated
expression or pose model. Scores are candidate-selection evidence, not an autonomous publish
decision. Human feedback and prior-work learning belong to Task 018, privacy/safety flags remain
authoritative, and future model revisions must update `MODEL_VERSION` and the reviewed calibration
set.
