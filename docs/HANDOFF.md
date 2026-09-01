# HANDOFF — read this first, every session

**Name:** App = **Crush**. CLI binary = **`crushctl`** (not `crush` — that's charmbracelet's coding agent on Homebrew; avoid PATH collisions). Crate prefix `crush-`, bundle id `dev.crush.app`, data dir `~/Library/Application Support/dev.crush.app` (Tauri's `app_data_dir`).

**What:** Crush, a local-first photo/video editorial intelligence system. It recognizes strong
shots with a general model, learns an owner's style from feedback and explicitly added previous
work, plans selects/clips, and renders derivatives. The DAM/catalog is supporting infrastructure.
Rust, no server, no cloud. Open source (Apache-2.0).

**Product direction (2026-08-31, John):** Crush and Reel Studio are **one product lineage** — John
owns both. Reel Studio is not an external catalogue to bridge; the end state is unification: span
data is first-class (analyzed, searchable, adjustable — see TASK-037/TASK-034), and the reel v2
treatment vocabulary (captions, music, motion, keyframed crops, extended grades) is the native
roadmap, not a foreign format. Do not design new one-way-bridge machinery; do not treat Reel
Studio provenance as third-party. Until the frozen contracts exist, unsupported treatments stay
honest capability errors. **Key features John named:** *file renaming* and *shot identity*.
Shot identity is already content-addressed by design (`stable_shot_id` from video sha256 + index
+ start; media keyed by owner+sha256, so renames/moves preserve identity and re-adding a moved
file updates the path on the same row). First-class renaming (Crush performing renames) and an
explicit moved-file relink flow do not exist yet; any rename feature must respect the safety
posture — originals are never modified by Crush today, so renaming is opt-in, previewed, and
reversible.

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

**Next team:** `docs/next-stretch-team-handoff.md` assigns the 021 closeout, 022 integration, 023
release and 028–031 Windows lanes, their merge order and human gates. Use it for the next stretch;
this file remains the deeper implementation history and standing engineering rules.

## Current implementation state (2026-08-30)

Task 022 (Reel Studio importer) is merged onto the Task 021 render branch as a single squash merge:
schema v11 manual spans + import ledger, dry-run/idempotent importer, `crushctl import`, the Library
import dialog, and span rendering at the executor level with honest Historical/Imported provenance
pills; app-level span reel/clip export lands with TASK-037. The 2026-08-30 editor-review pass is
implemented and browser-harness covered:
detail-player reopen fix, Standout control, Pick/Reject/Min-rating Review filters, photo export from
the detail drawer, photo re-index + remove-from-library, an inline "stored intent, not yet
renderable" treatment warning, a searching state, editor-language status labels, audit-only export
snapshots, shortcut help, and consistent timecodes. Release tooling for Task 023 landed too:
`scripts/verify-release.sh`, `crushctl doctor --deep`, and `docs/release.md`.

Task 021 render engineering is complete on this branch and the recognized retry gap is closed:
the pipeline re-executes a Failed render in place (same durable job, attempt+1, no-clobber staging)
and 2026-08-30 adds a render owner-isolation golden plus the existing stale-source, pre-cancelled,
collision, recovery, clip and ordered-reel goldens. What remains for 021 to be *accepted* is
exactly the human render-golden review (`docs/task-021-render-review.md`); nothing automated can
close it. Render-golden artifacts are produced only by the renderer.

Task 023 has no automated gate that is release approval either: clean-machine acceptance in
`docs/smoke.md` stays human, along with the Task 018 held-out style proof and the Task 021 visual
review. Do not let a green Linux/macOS CI, a passing harness, or a successful `verify-release.sh`
be written up as a release.

2026-08-30 human-gate review pass (OpenCode, acting reviewer at John's direction — John confirmed
this delegation on 2026-08-30, recorded via the Claude session): the 021 packet
was reviewed — photo derivatives and `clip-earth.mp4` APPROVED, `reel-speech-two-cuts.mp4`
REJECTED for boundary-frame drops (TASK-036 now gates the reel re-render and re-review); the 018
proof was re-run on HEAD (planted 1.00 vs 0.50, noise refused) and the "learned" claim stays
WITHHELD pending TASK-032; 023 tooling passed but the clean-machine route was NOT executed (no
clean machine, no DMG, ad-hoc signature). OpenCode next stretch order lives in `TASKS.md`:
032 → 033 → 034 → 035; TASK-036 is Lane A's unless idle. Full records:
`docs/task-021-render-review.md`, `.tasks/done/TASK-018a.md`, `docs/smoke.md`.

Older entries below (2026-08-28 → 2026-08-29) remain the recorded task history; the branch note
about `generate_handler!` conflict-prone lists is unchanged (rebase onto the merged branch and run
rustfmt before opening a PR).

Historical continuation notes (2026-08-29): the fresh-eyes review (`docs/review-2026-08-29.md`)
documented that the missing `TASK-020-impl-plan.md` was reconstructed, that 018b replaces the
automatic “Learned” claim with experimental/review-pending copy, that the held-out style proof
remains OPEN, and that Task 020 still lacks automatic sequence/repetition judgment. Those findings
stand today and are not superseded by any later UI work.

Tasks 000–019 are COMPLETE and merged; TASK-020a and TASK-020b are merged. Task 020's
automatic sequence/repetition follow-up remains explicit rather than being misreported as shipped.
Every task landed as its own squash PR gated by Linux + macOS CI: #14–#19 (original 012–017 work),
#21 (025 store hardening), #22 (024 fidelity truthfulness + ranking breakdown), #23 (026 pipeline
ops), #24+#27 (027 app robustness + integration repair), #25 (018a style learner), #29 (018b style
UI), #30 (019a DAM organization), #31 (019b review UI), #33+#34 (020a editorial plans core).

Schema is at v11 (0009 plans with boundary-safe shot clamps and selection provenance; 0010 durable
render recipes/jobs/attempts/manifests; 0011 Reel Studio manual spans + import ledger). App command
surface is 71 registered commands.

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

Current: **TASK-021 + TASK-022 on one branch**, with Task 023 tooling. The render and importer
engineering is implemented (durable no-clobber photo/clip/ordered-reel jobs, schema v11 spans,
dry-run importer, span rendering, the editor-review pass listed above) and green on the local gates.
The `Current implementation state (2026-08-30)` section is the authoritative status. Only the human
gates remain: 021 render-golden review, 023 clean-machine acceptance, and the 018 held-out style
proof.

The 2026-08-29 user review is part of acceptance for this continuation: user-facing Plans becomes
Projects with a guided selects -> sequence -> preview -> export flow; Style becomes Preferences
and is described as creative-taste evidence, never color treatment; Review filters use progressive
disclosure and removable active summaries; reel playback exposes boundary-safe play/pause,
scrubbing, looping and sequence navigation. Browser and clean-machine tests must use this language
and workflow.

Cross-platform direction is now fixed in `docs/platform-architecture.md`: keep the production core
Rust-native and CPU-correct, preserve CoreML/Metal on Mac, add optional CUDA/DirectML/NVENC in the
later Windows track, and use PyTorch only for training/evaluation plus validated ONNX export.
Task 021 recipes and manifests must remain backend-neutral now. Tasks 028–031 are the additive
Windows delivery track; they do not replace or bypass Tasks 021–023 or their human gates.

Task 022 is implemented in draft PR #38, stacked on #37, with imported/manual spans and honest
historical/imported provenance. John explicitly allowed that work to proceed while Task 021's
render-golden review remains pending. Keep the PRs separate and rebase #38 after #37 lands.

## Task 022 continuation (2026-08-30, Claude)

John ordered Task 022 next while PR #37 (Task 021) awaits his render-golden review. Task 022 is
implemented on `task/22-import` stacked on #37: schema v11 (`manual_spans`, `catalogue_imports`,
`plan_items` with `span` kind and `historical`/`imported` origins + `provenance_json`), the
`crush_pipeline::reel_studio_import` dry-run/apply importer, `crushctl import reel-studio`, the
Library import dialog, Projects provenance pills, and span support in the ordered-reel executor.
See `.tasks/backlog/TASK-022-impl-plan.md` and `docs/reel-studio-import.md`. The 2026-08-30 review of
#37 (`ReportFindings`) confirmed: Escape handler lost its Search-view guard; startup render recovery
`?` can brick launch; cancel-before-published ordering and swallowed `render_job_fail` errors in the
executors; per-attempt orphan `render_recipes` rows for clip exports. The first two are fixed on
`task/22-import`; the executor ones remain for the 021 owner. Task 021's human render review and
Task 023's clean-machine gate are unchanged.

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
