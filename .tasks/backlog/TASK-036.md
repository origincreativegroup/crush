# TASK-036: Ordered-reel renderer drops boundary frames (021 review rejection)
Agent: Lane A render owner (Codex; OpenCode may take it if Lane A is idle). Branch: task/36-reel-frames.
Priority: gates Task 021 — the render-golden review rejected the reel artifact on this defect.

Source: docs/task-021-render-review.md § Review record 2026-08-30. Ground truth from the burned-in
frame counter in fixtures/clips/synthetic-speech.mp4: requested 0.25–1.25 + 3.25–4.25 rendered as
source frames 8–36 then 98–124 — missing frame 37 (1.233 s) and frames 125–127 (4.167–4.233 s),
~133 ms of requested content; ~80 ms dead zone before the first frame; ~113 ms freeze of the last
segment-A frame at the cut (PTS 1.0131 → 1.1262); tail audio plays over a frozen frame 124.
Container duration passed tolerance because audio padding hides missing video frames (fps 28.77
symptom).

## Acceptance
- [ ] Per-item renders deliver every requested frame: for in/out at frame boundaries, item k yields
      round((out-in)*fps) frames starting at the requested first frame; no lead dead zone.
- [ ] Concat introduces no PTS gaps/holds: cut lands exactly at the previous item's video duration;
      audio never outlasts video within an item (pad video or trim audio — document which).
- [ ] Verification counts VIDEO stream frames and duration per item and for the concat, not just
      container duration; the synthetic-speech frame-counter fixture becomes an automated golden
      asserting exact first/last source frames per segment.
- [ ] Re-render the review packet's reel artifact from the fix commit via the renderer only;
      request human re-review of that one item (docs/task-021-render-review.md). No golden edits.
- [ ] Full gates: fmt, warnings-denied clippy, workspace tests, browser harness.
