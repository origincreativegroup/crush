# TASK-018: Previous-work examples and personal-style learner

Depends: Tasks 017 and 019. A feedback-centroid baseline already exists.

## Acceptance

- [ ] Users can create named, context-scoped reference sets from previous photo projects, selects,
      finished videos/reels, or individual examples and explicitly mark what represents their style.
- [ ] Uncurated folders are cataloged but contribute no positive training signal until confirmed.
- [ ] Train owner- and context-scoped ranking from pairwise preferences, picks/rejects, ratings,
      curated previous-work examples, crops, grades, exports, publishes, and confirmed Reel Studio
      evidence.
- [ ] Separate semantic relevance, general quality, personal affinity, context fit, and penalties in
      ranking and UI explanations.
- [ ] Version profiles with sample count, feature/model versions, metrics, and reversible reset.
- [ ] Held-out evaluation beats the non-personalized baseline before the UI says “learned.”
- [ ] Sparse-feedback behavior regularizes toward the general model and never invents certainty.
- [ ] Disabling/removing examples or resetting the profile falls back to Task 017 strong-shot
      ranking; the core system never relies solely on personal evidence.
