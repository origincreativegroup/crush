# TASK-017: General strong-shot and explainable aesthetic analysis

Depends: Task 016.

This is the independent cold-start judgment layer. It must work without user examples, feedback,
identity recognition, or an active personal profile.

## Acceptance

- [ ] Score technical quality separately: focus, blur, exposure, clipping, noise, compression,
      resolution, motion stability, and duplicate/near-duplicate confidence.
- [ ] Score composition/design separately: hierarchy, balance, subject placement, negative space,
      leading lines, symmetry, contrast, color harmony, and crop potential.
- [ ] Add moment/story and sequence features for expressions, gestures, action, novelty, pacing, and
      repetition without treating identity recognition as quality.
- [ ] Store component values, confidence, model version, and plain-language evidence.
- [ ] Human-reviewed still and video fixtures catch regressions and calibration drift.
- [ ] A no-profile acceptance test produces useful strong-shot ordering and plain-language reasons.
