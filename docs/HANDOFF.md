# HANDOFF — read this first, every session

**Name:** App = **Crush**. CLI binary = **`crushctl`** (not `crush` — that's charmbracelet's coding agent on Homebrew; avoid PATH collisions). Crate prefix `crush-`, bundle id `dev.crush.app`, data dir `~/Library/Application Support/Crush`.

**What:** Crush, a local Mac app that splits footage into shots, embeds them with CLIP, transcribes with Whisper, and searches by text. Rust, no server, no cloud. Open source (Apache-2.0).

**Source of truth:** `docs/project-blueprint.md`. Review + protocol: `docs/blueprint-review.md`. Tasks: `TASKS.md` and `.tasks/`.

**Stack (do not change without a blueprint edit):** Rust workspace; bundled static ffmpeg via subprocess; `ort` (CoreML + CPU) for CLIP ONNX; `whisper-rs` (Metal); `rusqlite` for metadata AND vectors; in-process cosine search; `clap` CLI; Tauri 2 app.

**Blacklist:** `onnxruntime-rs`, `ffmpeg-next`, `tch`, Qdrant, Docker, anything server-shaped.

**Rules:**
- Stages read from the store or a path and write to the store. No in-memory hand-offs between stages.
- `owner_id` on every owned record.
- Golden tests are correctness. Never edit golden files to pass.
- Log `job_id` and `stage` on every stage span. Log ffmpeg command lines verbatim.
- Mac tasks are tested on the Mac. Paste `doctor` output in the PR.

**Routing:** Tasks 0, 4, 6, 7, 8, 11, 12a–c, 13 → Cursor on the Mac. Tasks 1, 2, 3, 5, 9, 10 → Codex OK.

**Hard stops (human):** after Task 0, after Task 7, after Task 12c, before notarization.

**Branch:** `task/NN-short-name`. One task per PR.

## Current implementation state (2026-08-27)

- TASK-001 is complete: workspace build, all tests, Clippy with warnings denied, and rustfmt pass
  locally on macOS and in GitHub Actions/Linux CI run 33123337534.
- `crushctl doctor` was exercised against a temporary data directory. It creates
  `logs/crush.log` and emits JSON with both `job_id="doctor"` and `stage="doctor"`.
- Git is initialized on `main` and connected to the public repository
  `https://github.com/origincreativegroup/crush`.
- TASK-000 is implemented on `task/00-spike`. Its release run passed: FFmpeg spawn 4.98 ms,
  CoreML 685/685 nodes in one partition with no ONNX Runtime CPU-provider events and 5.86 ms mean
  CLIP inference, and Whisper Metal inference on 10 seconds of audio in 79.04 ms. The clean CoreML
  session compile took 201.11 seconds; see `docs/versions.md` for the caching requirement and full
  provenance.
- John approved GO for TASK-000 on 2026-08-27; PR #1 was squash-merged as `fc50fb8`.
- TASK-002 is complete on `task/02-store`: bundled SQLite 0.40.2, transactional schema v1,
  owner-safe typed APIs, synchronized transcript FTS, exact little-endian vectors, jobs, cascading
  deletion, and deep integrity checks. Six store acceptance tests pass; the 1000×512 vector load
  measured 14.845 ms. Merge its PR before activating TASK-003.
