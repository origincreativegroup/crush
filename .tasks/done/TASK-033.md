# TASK-033: Automatic sequence and repetition judgment (020 completion)
Agent: OpenCode. Branch: task/33-sequence. Depends: 020a/020b merged.

Close the open Task 020 acceptance: candidate ordering and project planning currently rank
individual assets; nothing judges the sequence as a sequence.

## Acceptance
- [x] Owner-scoped sequence scoring over an ordered plan: repetition (same subject/scene/near-
      duplicate embedding adjacency), pacing distribution, and coverage across source
      clips/exhibits, for photo and video items including imported spans.
- [x] Surfaced as explainable per-transition/per-item signals in Projects (plain language), never as
      an unlabeled automatic reorder; suggestions are one-click apply/undo and write normal plan
      state, not feedback.
- [x] `selects_candidates`/plan generation optionally diversifies (cap near-duplicates per source)
      with the cap visible and adjustable.
- [x] Deterministic tests on fixture embeddings; browser-harness scenario for the suggestion flow;
      no "optimized" claim anywhere.

## Implemented (2026-08-31, OpenCode, branch `task/33-sequence`)

- Realignment note: the plan said 033 can branch from `origin/main`, but the span half of the
  first checkbox needs `MediaKind::Span`/`provenance_json` (schema v11, Task 022), which only
  exists on the unmerged 021 branch. The branch therefore bases on `task/21-render-export`
  (a5d15d6) and stacks behind PR #37.
- `crates/search/src/sequence.rs` — deterministic, read-only engine:
  - Per-adjacent-pair transitions: embedding cosine when both sides have vectors
    (`NEAR_DUPLICATE_COSINE` 0.95, `REPETITION_COSINE` 0.85); same-source detection for shots
    (source video) and imported spans (`import_id` from the frozen item provenance, falling back
    to `external_id`; unimported manual spans group by themselves). Spans carry no embeddings, so
    their repetition evidence is provenance-only and `similarity` stays `None`.
  - Per-item notes in editor language ("Looks near-identical to the previous item."), a coverage
    summary (items vs distinct sources, busiest source), and a pacing summary over video item
    durations. Nothing is labeled "optimized" anywhere.
  - Suggestions: at most one per affected item; the move takes the later twin to the end of the
    sequence. A move that cannot separate the pair (two-item plans, twin already last) is
    refused rather than pretending (`sequence_suggestions_refuse_a_move_that_cannot_separate`).
- Projects UI (`plans.js` + `index.html`): a "Sequence notes" panel renders the summary, any
  transition notes, and one-click suggestion chips. Applying saves a "Before sequence
  suggestion" version first, then reorders through the existing `plan_reorder_items` — normal
  plan state, no feedback events; undo is the existing Versions restore.
- Diversification cap: `selects_candidates` gains `duplicate_cap` (echoed with
  `skipped_duplicates` for visibility). Shots count per source video; near-duplicate photos
  (cosine ≥ 0.95) share the first kept photo's bucket, so a burst counts as one exhibit.
  Surfaced as a visible/adjustable "Similar-shot cap" input in Projects, `--duplicate-cap` on
  `crushctl selects`, and `duplicateCap` on the Tauri command. The personalized list is never
  altered.
- Tauri commands `plan_sequence_signals` / `plan_sequence_suggestions` registered (read-only).
- Tests: near-duplicate adjacency + coverage, suggestion apply clears the flagged adjacency,
  span provenance grouping, cap diversification (per video + photo-burst bucketing), no-op-move
  refusal. Harness scenario `plans-sequence` covers the cap echo, the notes panel, and the
  apply-with-undo flow.
- Gates: fmt clean, workspace clippy `-D warnings` clean, full workspace tests green (34
  suites), browser harness green (23 oks).
