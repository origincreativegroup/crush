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
- TASK-002 is complete and was squash-merged as `58ac826`: bundled SQLite 0.40.2, transactional schema v1,
  owner-safe typed APIs, synchronized transcript FTS, exact little-endian vectors, jobs, cascading
  deletion, and deep integrity checks. Six store acceptance tests pass; the 1000×512 vector load
  measured 14.845 ms.
- TASK-003 was approved and squash-merged as `1c2750f`. Four license-safe fixtures total 5.15 MiB. The pinned
  Python 3.12 answer key exports the OpenAI CLIP ViT-B/32 QuickGELU encoders at opset 17 and produces
  scenes, transcripts, image tensors/embeddings, and text tokens/embeddings. Two full golden runs are
  byte-identical and `make -C reference check` passes. John approved the rocket-launch scene-boundary
  review on 2026-08-27.
- TASK-004 was squash-merged as `7e17792`. The mandatory LGPL build cannot use the task draft's GPL
  `libx264` fallback, so clip re-encoding uses `h264_videotoolbox` plus native AAC instead. The
  resolver, five operations, progress, process-group cancellation, debug command capture, exact
  source rebuild, Mac fixture tests, and release-layout `source=bundled` doctor check pass locally.
  GitHub Actions/Linux CI run 33135879446 passed before merge.
- TASK-005 is complete on `task/05-scenes`. The pure-Rust 480p HSV detector preserves the specified
  score formula, collapses multi-frame threshold runs, and recovers the tail of a sustained sampled
  fade without lowering threshold 27. All four PySceneDetect goldens pass at 4 fps with at most one
  unmatched cut per fixture minute. A no-cut clip yields one shot; thumbnails, SQLite shot rows, and
  `scores.csv` agree; `crushctl debug scenes <video>` writes and prints byte-identical CSV. The
  explicit release benchmark detects 2,400 decoded 480p frames in 3.47 s. The required score plot was
  reviewed at desktop and narrow widths: threshold 27 retains the approved 11.33 s cut while allowing
  one earlier false positive, so lowering it would reduce precision.
- TASK-006 is complete on `task/06-models`. The project-org `models-v1` release publishes five pinned
  model assets plus the tracked manifest (1,242,702,823 model bytes total). The reference export uses
  a real fixture and exceeds cosine 0.9999 on both CLIP encoders; CoreML supports 466/467 image nodes
  and 476/478 text nodes, with the remaining nodes explicitly falling back to CPU. The pinned `ureq`
  downloader streams to `.part`, resumes with Range, retries, verifies SHA-256, and renames atomically.
  A live acceptance run was interrupted at byte 234,881,024, resumed from that offset, verified all
  five release assets, detected a deliberately corrupted vocabulary as `sha-mismatch`, repaired only
  that file, and left `doctor` green. The first successful ensure recorded the combined CLIP model
  identity, dimension 512, and preprocessing version 1 in `embedding_meta`.
- TASK-006 passed Ubuntu CI run 33137930186 and PR #6 was squash-merged as `907cae5`.
- TASK-007 is complete on `task/07-preprocess`. All four lossless PPM fixture tensors match the Pillow
  answer key exactly (`max_abs_diff=0`) at the required `1e-3` threshold. `image` crate Catmull–Rom
  reached only 0.042660236 and Lanczos3 reached 0.10505438, so the production path implements
  Pillow's scale-aware BICUBIC coefficient and 22-bit fixed-point two-pass behavior directly in Rust.
  JPEG and PNG decode coverage passes. John ran the required command twice on his Mac on 2026-08-27;
  both runs passed two tests with every fixture reporting `max_abs_diff=0`, satisfying the hard stop.
