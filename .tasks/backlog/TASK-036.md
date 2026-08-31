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
- [x] Per-item renders deliver every requested frame: for in/out at frame boundaries, item k yields
      round((out-in)*fps) frames starting at the requested first frame; no lead dead zone.
      Implemented in `crates/stage-split/src/reel.rs` (`plan_item_frames`) and
      `crates/stage-split/src/ffmpeg.rs` (`render_reel_item_with_control`): input-side seek with a
      2 µs boundary epsilon, `-frames:v` pinning the exact count, `setpts=PTS-STARTPTS` zeroing
      the item timeline. Unit-tested in `reel.rs` and golden-tested against the fixture.
- [x] Concat introduces no PTS gaps/holds: cut lands exactly at the previous item's video duration;
      audio never outlasts video within an item (pad video or trim audio — document which).
      Decision: TRIM AUDIO to the item's exact video duration — the requested frames are the
      content contract; padding video would invent unrequested frames. Documented in the
      `reel.rs` module docs and the assembly comment. The concat demuxer consumes verified
      video-only item copies so per-file offsets stay on frame boundaries; the reel audio is
      decoded from the items, trimmed of AAC priming frames, joined with the concat filter and
      encoded once (stream-copying item audio carries the priming packet that caused the
      reel-wide head dead zone and cut drift).
- [x] Verification counts VIDEO stream frames and duration per item AND for the concat, not just
      container duration; the synthetic-speech frame-counter fixture becomes an automated golden
      asserting exact first/last source frames per segment.
      `Probe` now carries `video_frame_count`/`video_duration_s`/`audio_duration_s`; the renderer
      fails closed per item, per video-only copy, and for the concat; the manifest records
      per-item first/last source frames (8–37 and 98–127 for the golden intervals). Golden:
      `crates/pipeline/tests/render_jobs.rs::frozen_ordered_reel_job_renders_project_order_and_publishes_one_manifest`
      asserts frame identity by decoded-plane nearest-match against the burned-in counter, plus
      exact cut PTS (frame 29 at 0.9667 s, frame 30 at 1.000 s) and head PTS 0.
- [x] Re-render the review packet's reel artifact from the fix commit via the renderer only;
      request human re-review of that one item (docs/task-021-render-review.md). No golden edits.
      Packet: `target/render-golden-review/task-036-reel-fix/` (reel + manifest + README +
      SHA256SUMS), produced by the reel render test with `CRUSH_RENDER_REVIEW_DIR`. The re-review
      request is recorded in `docs/task-021-render-review.md` § Re-review request — TASK-036.
      The human re-review itself remains OPEN.
- [x] Full gates: fmt, warnings-denied clippy, workspace tests, browser harness.
      `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
      `cargo test --workspace` (34 suites, 0 failures), `npm run test:ui` (22 scenarios) all pass
      on the Apple Silicon Mac with the bundled sidecars.
