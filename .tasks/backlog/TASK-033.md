# TASK-033: Automatic sequence and repetition judgment (020 completion)
Agent: OpenCode. Branch: task/33-sequence. Depends: 020a/020b merged.

Close the open Task 020 acceptance: candidate ordering and project planning currently rank
individual assets; nothing judges the sequence as a sequence.

## Acceptance
- [ ] Owner-scoped sequence scoring over an ordered plan: repetition (same subject/scene/near-
      duplicate embedding adjacency), pacing distribution, and coverage across source
      clips/exhibits, for photo and video items including imported spans.
- [ ] Surfaced as explainable per-transition/per-item signals in Projects (plain language), never as
      an unlabeled automatic reorder; suggestions are one-click apply/undo and write normal plan
      state, not feedback.
- [ ] `selects_candidates`/plan generation optionally diversifies (cap near-duplicates per source)
      with the cap visible and adjustable.
- [ ] Deterministic tests on fixture embeddings; browser-harness scenario for the suggestion flow;
      no "optimized" claim anywhere.
