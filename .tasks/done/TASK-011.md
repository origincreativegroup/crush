# TASK-011: Ingest orchestration, cancel/resume, clip export
Agent: Codex on the Mac. Branch: task/11-ingest. Depends: 005, 008, 010. Status: done.

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
- [x] Ingest fixtures folder; second run skips everything
- [x] `kill -9` mid-embed; rerun resumes from `split` with no duplicate shots or vectors
- [x] Ctrl-C stops within 5 s; state consistent
- [x] Inject a failing ffmpeg (bad file) → job failed with error text, other videos continue
- [x] Resplit of an unchanged video yields identical shot ids
- [x] `clip` exports a playable file at the right timecodes
- [x] Laptop stays responsive during a 10-minute ingest (subjective; note in PR)

## Human review
End-to-end on real footage; then the first 5-hour smoke (docs/smoke.md).

## Implementation record

Completed on 2026-08-28. A reusable `crush-pipeline` crate now owns the state machine so the CLI
and the upcoming desktop application share identical ingest, recovery, reprocessing, and export
behavior. The CLI exposes `ingest`, `jobs`, `resplit`, `reembed`, and `clip`, with one shared Ctrl-C
token propagated through FFmpeg, frame processing, embedding, and Whisper.

- Files are discovered recursively in deterministic order and SHA-256 hashed with a 1 MiB streaming
  buffer. Completed content is skipped, changed content at the same path is retained as a new video,
  and one video is processed at a time under a reduced process priority.
- Shot and transcript identifiers use the requested truncated Blake3 inputs. Split replacement and
  vector/transcript replacement are idempotent, while abandoned running jobs become failed with an
  `interrupted` error and restore the exact last completed stage.
- Debug runs preserve the job directory and stage commands/intermediates. Search is reloaded after
  ingest, making new vectors immediately available and validating embedding metadata consistency.
- The fixture acceptance test indexed four videos, skipped all four on the second pass, simulated a
  killed embed with a partial vector set, preserved IDs through resplit, and exported a playable clip
  whose duration was within 0.25 seconds of the indexed timecodes. A corrupt MP4 produced a failed
  job with error text while the following valid file completed.
- The FFmpeg cancellation escalation test completes below the five-second contract even when the
  child ignores SIGINT. A pre-cancelled pipeline creates no jobs.
- The reproducible ignored 10-minute smoke expanded the silent Earth fixture to 600 seconds,
  processed 2,400 sampled frames in 118.26 seconds, indexed 90 vectors, and left the Mac interactive.
  The five-hour review on user-supplied real footage remains a release smoke recorded in
  `docs/smoke.md`; it is not represented by synthetic fixture results.
