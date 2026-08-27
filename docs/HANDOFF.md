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

- TASK-001 is implementation-complete and locally green on macOS: workspace build, all tests,
  Clippy with warnings denied, and rustfmt check pass.
- `crushctl doctor` was exercised against a temporary data directory. It creates
  `logs/crush.log` and emits JSON with both `job_id="doctor"` and `stage="doctor"`.
- The local Git repository is initialized on `main`. GitHub creation/connection and the first CI
  run are still pending at `https://github.com/origincreativegroup/crush`.
- Do not advance the implementation sequence past the Task 0 hard stop. The next runtime task is
  TASK-000 in `spike/`, owned by Cursor on the Mac, followed by John's explicit go/no-go. Once
  approved, finish TASK-001's Linux CI evidence and begin TASK-002.
