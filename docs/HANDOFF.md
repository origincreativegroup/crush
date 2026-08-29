# HANDOFF — read this first, every session

**Name:** App = **Crush**. CLI binary = **`crushctl`** (not `crush` — that's charmbracelet's coding agent on Homebrew; avoid PATH collisions). Crate prefix `crush-`, bundle id `dev.crush.app`, data dir `~/Library/Application Support/dev.crush.app` (Tauri's `app_data_dir`).

**What:** Crush, a local-first photo/video editorial intelligence system. It recognizes strong
shots with a general model, learns an owner's style from feedback and explicitly added previous
work, plans selects/clips, and renders derivatives. The DAM/catalog is supporting infrastructure.
Rust, no server, no cloud. Open source (Apache-2.0).

**Source of truth:** `docs/project-blueprint.md` remains the engineering architecture and build
protocol, with `docs/blueprint-review.md` as its review discipline. The additive product expansion
is `docs/dam-feedback-blueprint.md`. Current sequencing and acceptance live in `TASKS.md` and
`.tasks/`; the DAM plan extends the original architecture rather than replacing it.

**Stack (do not change without a blueprint edit):** Rust workspace; bundled static ffmpeg via subprocess; `ort` (CoreML + CPU) for CLIP ONNX; `whisper-rs` (Metal); `rusqlite` for metadata AND vectors; in-process cosine search; `clap` CLI; Tauri 2 app.

**Blacklist:** `onnxruntime-rs`, `ffmpeg-next`, `tch`, Qdrant, Docker, anything server-shaped.

**Rules:**
- Stages read from the store or a path and write to the store. No in-memory hand-offs between stages.
- `owner_id` on every owned record.
- Golden tests are correctness. Never edit golden files to pass.
- Log `job_id` and `stage` on every stage span. Log ffmpeg command lines verbatim.
- Mac tasks are tested on the Mac. Paste `doctor` output in the PR.

**Routing:** The original routing remains the record for Tasks 0–13. Current Tasks 016–023 are
owned through the additive DAM/editorial plan. Do not revive closed implementation branches or
obsolete agent assignments without an explicit current task.

**Hard stops (human):** RAW/color fixture review in Task 016, held-out style proof in Task 018,
render-golden review in Task 021, and clean-machine acceptance before release in Task 023.

**Branch:** `task/NN-short-name`. One task per PR.

## Current implementation state (2026-08-29, agent-team session)

**Fresh-eyes continuation:** see `docs/review-2026-08-29.md` and Task 020b. The missing
`TASK-020-impl-plan.md` is now explicitly reconstructed. 018b was merged in #29; its automatic
“Learned” claim is replaced with experimental/review-pending copy. Style proof remains OPEN
(pair/asset leakage, residual-vs-composed evaluation, evidence withdrawal require follow-up).
020b provides plans UI, frozen profile provenance and truly context-scoped explicit picks.
Task 020 overall still lacks automatic sequence/repetition judgment; do not claim it exists.
Historical statements below describe the incoming handoff, not fresh acceptance of those claims.

Tasks 000–019 are COMPLETE and merged; TASK-020a is merged and TASK-020b is the next work item.
Every task landed as its own squash PR gated by Linux + macOS CI: #14–#19 (original 012–017 work),
#21 (025 store hardening), #22 (024 fidelity truthfulness + ranking breakdown), #23 (026 pipeline
ops), #24+#27 (027 app robustness + integration repair), #25 (018a style learner), #29 (018b style
UI), #30 (019a DAM organization), #31 (019b review UI), #33+#34 (020a editorial plans core).

Schema is at v9 (0009_plans.sql: plans / plan_items / plan_revisions with boundary-safe shot clamps,
append-only revisions, and a provenance invariant recording origin/rank/profile_version so baseline
and personalized results stay distinguishable in data). App command surface is 63 registered commands.
Branching note for future agents: the app `generate_handler!` list and the big command block in
`crates/app/src-tauri/src/lib.rs` conflict-prone across parallel branches — rebase onto main before
opening the PR and run rustfmt (it is the local syntax referee when no toolchain compiles).

Known caveats carried forward:
- The `sips -s iccProfile` HEIC sub-case in `source_fidelity.rs` skips on some runners; the JPEG ICC
  round-trip is the enforced coverage.
- The UI harness passed on the #35 macOS runner; Task 020b makes it blocking and adds native
  app tests/Clippy so Linux-only checks cannot hide macOS-gated bridge regressions.
- HUMAN HARD STOPS still open: Task 018 held-out style proof (eval output in PR #25 — John reviews
  before any UI may claim "learned"), Task 021 render-golden review, Task 023 clean-machine
  acceptance. Plan files and per-task acceptance records live in `.tasks/done/`.

Next: **TASK-020b** (plans UI: two-column General/Personalized candidates, plan editor, provenance
pills) per `.tasks/done/TASK-020-impl-plan.md`, then Tasks 021–023.

## Previous implementation state (2026-08-28)

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
- TASK-008 is complete on `task/08-embed`; PR #8 passed Linux and macOS CI runs 33168746223 and
  33168749709. The human doctor gate also passed on the Mac. The
  exact OpenCLIP BPE port matches all five token goldens; CPU and CoreML both return cosine
  `1.000000000` for all four image and five text fixtures. Runtime provider evidence comes from an
  ONNX Runtime profile after real inference. Doctor reports `active=coreml providers=cpu,coreml` and
  5.29 ms/frame over 20 frames (CPU: 9.38 ms/frame). Clean keyed CoreML image+text initialization
  measured 132.23 s. Derived ONNX copies append the official `COREML_CACHE_KEY` metadata using each
  pinned SHA, so CLI and test processes reuse the same two cache entries without changing releases.
  The missing-vector stage is resumable at batch one, and `crushctl debug vector` prints norm, first
  eight values, and verified provider.
- TASK-009 is complete on `task/09-search`. The owner-scoped in-memory cosine index, bounded top-K
  heap, FTS overlap boost, stale-model refusal, result hydration, and table/JSON CLI are implemented.
  The 10k×512 exact scan measured 7.55 ms; hybrid and workspace tests pass. A full fixture run
  produced visually correct candidates for all five canned queries. John approved those candidates
  on 2026-08-28, and they are committed as the enforced `fixtures/golden/expected_search.json` gate.
  Three additional human-review queries also returned their matching visual family at rank 1. The
  pinned Task 4 binaries are now published in the `sidecars-v1` release and hash-verified by macOS CI.
