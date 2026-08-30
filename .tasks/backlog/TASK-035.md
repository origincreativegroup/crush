# TASK-035: Render engineering follow-ups from the 2026-08-30 review
Agent: OpenCode. Branch: task/35-render-followups. Depends: 021 merged. No golden edits; renderer
output must remain byte-stable or the render review packet is invalidated — coordinate with Lane A.

Remaining items from the PR #37 review (the executor refactor in 34069b9 already fixed the
cancel-vs-published ordering, swallowed transitions, orphan recipes, and queue-time photo-source
rejection). Verify each against the current merged head before coding; several may shrink.

## Acceptance
- [ ] One documented duration-tolerance rule shared by `ffmpeg.rs` (frame_tolerance + 0.05) and both
      executor re-checks in `render.rs`; a 60 fps AAC-priming case cannot pass the encoder check and
      then fail the executor.
- [ ] Startup render recovery leaves the Tauri setup thread (spawn_blocking + event/log), with a
      cheap size/metadata short-circuit before any full SHA-256 of published outputs.
- [ ] Reel source hashing memoized per resolved path for the before AND after passes; one ffprobe
      per distinct source per reel; ffmpeg `-encoders`/`-filters` capability listings cached per
      Runner.
- [ ] `render_job_set_progress` becomes a single guarded UPDATE (no full-row JSON deserialization);
      progress callbacks wired to real ffmpeg progress for clips/reels.
- [ ] Preset facts (extension/media-type/muxer/labels) defined once on the preset enums and exposed
      through a `list_render_presets` command the UI reads, replacing the drifting copies in
      plans.js/index.html/Tauri spec tables.
- [ ] Full gates: fmt, warnings-denied clippy, workspace tests, browser harness.
