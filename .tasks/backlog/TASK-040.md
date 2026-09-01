# TASK-040: Backend contracts for the UX track follow-ups
Agent: Backend lane (OpenCode). Branch: task/40-ux-contracts. Depends: 021 merged. Source:
docs/ux-enhancement-proposal.md Track C backend items + TASK-039 wave discoveries. Each item is
small and independent; land as one PR or split if review prefers.

## Acceptance
- [ ] `search` command accepts an optional `kind` argument (photo | video | all) so the UI
      kind-filter can filter server-side instead of client-side (proposal C8). Default `all`
      preserves the current contract; harness mock parity; documented in the Tauri spec table.
- [ ] `list_videos` (or the asset-list response) exposes a thumbnail reference for video rows so
      the Library can show real thumbnails for videos, not a placeholder (proposal C7). Thumb
      path rules follow the existing photo thumb discipline; no thumbnail fabrication for assets
      that have none — honest placeholder stays.
- [x] Render progress events reach the UI (proposal B10): wire the existing ffmpeg progress
      callbacks (currently `|_| {}` in the render executors — see TASK-035's plan, which also
      covers this) to `render_job_set_progress` and a Tauri event the UI already listens to, so
      clip/reel renders show real percentages. Coordinate with TASK-035 to avoid double work —
      if 035 lands first, this item is done by it; verify and check off either way.
      (Checked off 2026-09-01 via TASK-035: the executor callbacks now feed real, throttled,
      monotonic ffmpeg progress into `render_job_set_progress` — the job/attempt rows carry
      live 0.1–0.75 values — see `JobProgressWriter` in `crates/pipeline/src/render.rs`. Scope
      note: the UI currently shows an indeterminate busy state during renders and listens to no
      render-progress event, so no UI change was possible without inventing one; when the UX
      track wants live percentages, read the durable progress this wiring already produces.)
- [ ] DECISION NEEDED FROM JOHN before implementing: whole-video collection membership. Today
      collections and feedback are photo/shot-scoped (`parse_library_kind` maps video→shot), so
      the Library batch bar honestly disables those ops for video rows (TASK-039 wave 3). Options:
      (a) keep as-is — video shots are collected/reviewed at shot level (recommended: matches the
      shot-identity model); (b) allow whole-video membership (store change: collection items
      kind for videos). Record the decision in this file when made.

## Rules
- No golden edits; render output byte-stable (the progress wiring touches executor callbacks —
  same constraint as TASK-035).
- Full gates: fmt, warnings-denied clippy, workspace tests, browser harness (mock parity for any
  command shape change).
