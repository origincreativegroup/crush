# TASK-011: Ingest orchestration, cancel/resume, clip export
Agent: Cursor on the Mac. Branch: task/11-ingest. Depends: 005, 008, 010.

## Goal
`crushctl ingest <path> [--debug]` runs the pipeline end to end, safely.

## State machine per video
`pending → split → embedded → transcribed → done`, or `failed` (with the job row holding the error). Each stage: create job row (running) → do work in a transaction where possible → job done + advance status. On startup, any job left `running` (crash) is marked `failed("interrupted")` and the video resumes from its last completed status.

## Instructions
1. Walk path (files with ext in mp4/mov/m4v/mkv/avi/mts, case-insensitive). sha256 each (stream, 1 MB chunks). Skip if `(owner, sha)` exists and status = done → log "skip: already indexed". Same path new sha → new video row, old one kept (log it).
2. Process one video at a time. Stages in order; embed and transcribe may run concurrently for the same video (GPU vs CPU) — do it only if simple; otherwise sequential.
3. Stable IDs: shot id = `blake3(video_sha256 || idx || start_s_ms)` truncated to 16 hex, transcript segment id likewise from (video_sha256, start_ms, end_ms). Re-indexing an unchanged file reproduces identical ids, so anything referencing a shot (exports, future tags) survives a resplit.
4. Cancel: an `AtomicBool` checked between frames/segments and forwarded to the ffmpeg process group (Task 4); on cancel, mark job cancelled, leave video at its last completed status. CLI: Ctrl-C triggers it and waits ≤ 5 s.
5. Thread caps and `nice` applied.
6. `--debug`: `<data_dir>/debug/<job_id>/` with frames, wav, scores.csv, vectors.json, commands.txt; job row gets `debug_dir`.
7. `crushctl jobs [--failed] [--video <path>]`, `crushctl resplit <video>`, `crushctl reembed --all|<video>`, `crushctl clip <shot_id> --out <path>` (uses `export_clip`).
8. After ingest, refresh the search index.

## Acceptance
- [ ] Ingest fixtures folder; second run skips everything
- [ ] `kill -9` mid-embed; rerun resumes from `split` with no duplicate shots or vectors
- [ ] Ctrl-C stops within 5 s; state consistent
- [ ] Inject a failing ffmpeg (bad file) → job failed with error text, other videos continue
- [ ] Resplit of an unchanged video yields identical shot ids
- [ ] `clip` exports a playable file at the right timecodes
- [ ] Laptop stays responsive during a 10-minute ingest (subjective; note in PR)

## Human review
End-to-end on real footage; then the first 5-hour smoke (docs/smoke.md).
