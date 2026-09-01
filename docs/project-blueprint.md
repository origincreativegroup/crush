# Project Blueprint: Crush

Name is final: **Crush**. CLI binary `crushctl` (avoids charmbracelet/crush on Homebrew).

## 1. One-Line Concept

A Mac app: drop in footage, type what you're looking for in plain English, get back the exact shots with timecodes. Everything runs on the user's own machine.

## 2. Product Summary

Crush splits video files into shots, describes each shot with a visual embedding (CLIP) and a speech transcript (Whisper), stores them searchably, and returns ranked clips for a text query. It runs entirely on the user's MacBook: no server, no account, no upload. Phase 1 is John using it on his own footage. Phase 2 is shipping the same app to other users, which is a distribution and update problem rather than an accounts-and-isolation problem. It is written in Rust from day one because Rust is the long-term product; a small Python reference kit exists only to export models and to generate answer keys the Rust code is tested against.

## 3. Problem Statement

A video editor with hours of footage struggles to find "the shot where X happens" because the only index is memory and scrubbing. This costs hours per edit and means good shots get forgotten. A useful solution would let them search footage the way they search email, and hand back clips ready to drop on a timeline.

## 4. Target Users

| User | Need | Skill Level | Phase |
|---|---|---|---|
| John (owner) | Index own library, search, pull clips; also the tester | Technical; will use CLI and app | 1 |
| Editors / creators | Install the app, point it at their footage, search | Non-technical, Mac only | 2 |
| Windows / Linux users | Same | — | Not planned; Rust makes it possible later |

**Phase 2 readiness rule:** the app must work on a clean Mac with nothing preinstalled. Every dependency (ffmpeg, models) is bundled or fetched by the app itself. `owner_id` stays on records so an optional shared library or sync in Phase 2 doesn't need a migration.

## 5. Core Workflow

```
Video file(s)
  → SPLIT   ffmpeg + scene detection → list of shots (in/out timecodes) + thumbnail per shot
  → EMBED   one representative frame per shot (the frame at 40% of shot duration — avoids fade-ins and the cut frame) → CLIP vector (512 floats)
  → TRANSCRIBE  audio for the file → text, mapped onto shots by time
  → STORE   metadata and vectors → one SQLite file in the user's Library folder
  → SEARCH  text query → CLIP text vector → in-process cosine scan over all shot vectors, merged with transcript keyword hits
  → RESULT  ranked list: thumbnail, source file, in/out, transcript snippet, score
```

Each arrow is a **stage contract**: a stage reads only from the database (or a file path) and writes only to the database. Stages never pass in-memory objects to each other. This is what makes each stage independently replaceable and independently debuggable.

## 6. MVP Scope (Phase 1)

### Must Have
- `crushctl ingest <path>` — index a file or folder, idempotent (re-running skips already-indexed files by content hash)
- Shot splitting with tunable threshold, thumbnail per shot
- CLIP embedding per shot, matched against a Python answer key in tests
- Whisper transcript per file, aligned to shots
- SQLite for metadata and vectors, `owner_id` on everything
- `crushctl search "<query>"` — ranked results, top-N, with timecodes
- `crushctl clip <shot_id>` — exports that shot as a standalone file via ffmpeg
- Job log: every stage run recorded with status, duration, error text
- `--debug` mode that keeps all intermediate artifacts (frames, vectors, raw scene scores)
- Desktop window (Tauri): pick folder, watch progress, search, play, export
- Bundled ffmpeg; models downloaded on first launch with progress and a retry
- Runs on Apple Silicon with CoreML/Metal acceleration and a working CPU fallback

### Should Have
- Watch folder: new files in the chosen folder get indexed automatically
- Intel Mac build (CPU only, slow but works)
- Transcript keyword search combined with visual search (hybrid ranking)

### Could Have
- `crush assemble <query>` — concatenate top results into a rough-cut file
- Per-shot tags editable by hand
- Duplicate-shot detection

### Not Yet (Phase 2 or later)
- Accounts, login, cloud anything
- Sync or shared libraries between machines
- Billing / licensing
- Windows / Linux builds
- Face recognition, object detection (YOLO), segmentation
- Auto-editing beyond simple concatenation
- Mobile app

## 7. User Experience Requirements

- **Interface type:** native Mac window via Tauri (Phase 1 must). CLI kept for John and for tests.
- **Primary screens:** Library (folders being indexed, progress, job errors). Search (box, thumbnail grid). Shot detail (player, timecodes, transcript, copy path, export clip). First-run (model download).
- **Key actions:** add folder, search, play, copy source path + timecodes, export clip
- **Empty states:** "No footage indexed yet. Run `crushctl ingest`." / "No matches. Try broader words."
- **Error states:** show which stage failed and the job id; never a bare stack trace to a user
- **Success states:** results appear with a score; clip export shows the output path
- **Mobile/desktop priority:** Mac desktop only. Must feel native: Cmd-F focuses search, drag-and-drop folders, thumbnails load without jank.

## 8. Visual Direction

Utilitarian editor-tool feel. Dark UI, dense thumbnail grid, monospace timecodes, no marketing gloss. Reference vibe: DaVinci Resolve media pool, not a photo-sharing site.

## 9. Data Model

One SQLite file at `~/Library/Application Support/dev.crush.app/library.db` (Tauri's `app_data_dir`). Vectors are stored in SQLite too.

| Entity | Fields | Notes |
|---|---|---|
| `owners` | id, name, created_at | One row (John) in Phase 1. Auth attaches here in Phase 2. |
| `videos` | id, owner_id, path, sha256, duration_s, fps, width, height, indexed_at, status | `sha256` makes ingest idempotent. |
| `shots` | id, video_id, owner_id, idx, start_s, end_s, thumb_path, rep_frame_s, scene_score | `rep_frame_s` = which frame was embedded. `scene_score` = detector value at the cut, kept for tuning. |
| `transcripts` | id, video_id, start_s, end_s, text, confidence | Segment-level from Whisper; joined to shots by time overlap at query time. |
| `jobs` | id, owner_id, video_id, stage, status, started_at, finished_at, error, debug_dir | The debugging spine. Every stage run gets a row. |
| `shot_vectors` | shot_id, owner_id, vec BLOB (512 × f32, L2-normalized) | Loaded into memory on app start; ~2 KB per shot, so 100k shots ≈ 200 MB RAM ceiling before we'd need an index. |
| `embedding_meta` | model_name, model_sha256, dim, preprocess_version | One row. If it changes, all vectors are stale — search refuses to run until re-embed. |

**Search:** brute-force cosine over the in-memory vector matrix. No vector database. If a library ever exceeds ~100k shots, add `usearch` (embeddable HNSW) behind the same `search` trait; nothing else changes.

## 10. Technical Architecture

### Recommended Stack

- **Language:** Rust (stable, 2021 edition). Cargo workspace.
- **Video I/O:** `ffmpeg` and `ffprobe` **bundled inside the app** (static arm64 build, resolved via Tauri's sidecar mechanism) and invoked as subprocesses. **Not** `ffmpeg-next` bindings in v1 — the CLI is the most reliable video tool that exists, and bindings are where Rust video projects stall. Port the decode stage to bindings only if measured decode time justifies it.
- **Shot detection:** own implementation of content-based scene detection (HSV histogram delta between consecutive downscaled frames, threshold + min-scene-length). Same algorithm PySceneDetect uses; small enough to own.
- **Visual embedding:** CLIP ViT-B/32 exported to ONNX, run with the `ort` crate using the **CoreML execution provider**, CPU fallback. Image and text encoders as two ONNX files.
- **Speech:** `whisper-rs` built with the **Metal** feature, `small` GGML model by default (`base` on 8 GB machines, chosen by `doctor`).
- **Database:** `rusqlite` (bundled SQLite), metadata + vectors. Migrations as numbered SQL files applied at startup.
- **Vector search:** in-process cosine scan (`ndarray` or plain slices). No server.
- **Desktop shell:** **Tauri 2** — Rust backend is the same crates; front end is one HTML/JS page. Chosen over Electron (100 MB+ bundle, JS backend) and over a pure-Rust GUI (immature for media grids).
- **CLI:** `clap`, same crates, for John and for tests.
- **Models:** downloaded on first launch from a URL John controls (GitHub release assets), sha256-verified, stored in Application Support. The CLIP BPE vocab/merges file is a model asset like the ONNX files.
- **Version pins:** `ort` and `whisper-rs` are pinned to exact versions; `docs/versions.md` records why. **Crate blacklist:** `onnxruntime-rs`, `ffmpeg-next`, `tch`, and anything from the original Rust pitch document not named here. Contributors must not add them.
- **Resource limits:** one video at a time; ort and whisper thread count capped at physical cores minus two; pipeline runs at lowered priority. Ingest is cancellable from the UI and resumable per video.
- **Logging:** `tracing` + `tracing-subscriber`, JSON to file, pretty to terminal. Every log line carries `job_id` and `stage`.
- **Config:** one `crush.toml` + env overrides. Paths, thresholds, model paths.
- **Distribution:** signed and notarized `.app` in a `.dmg`, built by GitHub Actions on a macOS runner. Requires an Apple Developer account (~$99/yr). Phase 2 adds Tauri's built-in updater.
- **Auth:** none, ever, for the local app.

### Python reference kit (`reference/`, not part of the product)

- `export_clip_onnx.py` — exports the image and text encoders to ONNX with fixed input shapes. Run once. Output committed to a models folder (or downloaded by a script), never rebuilt casually.
- `reference_embed.py` — given an image path, prints the exact preprocessing steps and the resulting vector. Generates `fixtures/golden/*.json`.
- `reference_scenes.py` — runs PySceneDetect on the test clips, writes expected cut timecodes.
- `reference_transcribe.py` — runs faster-whisper on test clips, writes expected segments.

This kit is the **answer key**. It never runs in production.

### Workspace layout

```
crush/
  Cargo.toml                (workspace)
  crates/
    core/        contracts: types, config, errors, job log, tracing setup
    store/       SQLite migrations + queries, vector load/save
    stage-split/ ffmpeg wrapper, scene detector, thumbnails
    stage-embed/ ort session, preprocessing, image + text encode
    stage-asr/   whisper-rs wrapper, alignment to shots
    search/      query → vectors → cosine scan + transcript merge → ranked results
    cli/         clap binary `crushctl`
    app/         Tauri app: commands wrapping the crates + one HTML page
  sidecars/      static ffmpeg/ffprobe binaries (arm64), sha256 pinned
  reference/     Python answer-key kit
  fixtures/      3–5 short test clips (≤30 s each, ≤20 MB total) + golden outputs
  models/        .gitignored; fetched by scripts/get-models.sh
  .github/workflows/   macOS build + sign
  docs/
```

### Architecture Notes (plain language)

- **Stages are separate crates with one job each.** If shot detection is wrong, you open `stage-split` and nothing else.
- **The database is the only shared thing.** A stage can be re-run on its own by pointing it at a `video_id`.
- **The answer key is the safety net.** Rust does not get to decide what "correct" is; the Python reference does. Tests assert Rust output equals the reference within tolerance.
- **Nothing to install.** ffmpeg rides inside the app; models fetch themselves; the database is one file. A user's only job is "open app, pick folder".

## 11. Debugging Plan

This is a first-class deliverable because it is the real cost of choosing Rust before the pipeline is proven.

### 11.1 The answer-key contract

| Stage | Reference | Rust must match | Tolerance |
|---|---|---|---|
| Preprocessing (resize, center-crop, normalize) | `reference_embed.py --dump-tensor` | pixel tensor before the model | max abs diff < 1e-3 |
| Image embedding | `reference_embed.py` | 512-vector | cosine similarity > 0.999 |
| Text embedding | `reference_embed.py --text` | 512-vector | cosine similarity > 0.999 |
| Tokenizer | reference token ids | token id array | exact |

Tolerances above are for the CPU provider. On CoreML (fp16 internally) use cosine > 0.99; both providers are tested in Task 8.
| Scene cuts | `reference_scenes.py` | cut timecodes | every reference cut within ±2 frames; no more than 1 extra cut per minute |
| Transcript | `reference_transcribe.py` | segment text | word error rate < 15% on fixture clips |

Golden outputs are generated once, committed under `fixtures/golden/`, and regenerated only by a deliberate script run with a commit message saying why.

### 11.2 Where bugs will actually live, and how to tell which one you have

| Symptom | Most likely cause | First check |
|---|---|---|
| Search returns unrelated shots for everything | Preprocessing (wrong channel order, wrong normalization, wrong resize) | `cargo test -p stage-embed preprocess_golden` — this test exists precisely so you never guess |
| Search is "sort of right" but weaker than expected | Tokenizer mismatch, or wrong ONNX export (dynamic shapes, wrong opset) | text-embedding golden test; compare `models/*.onnx` sha256 to `embedding_meta` |
| Every result score is identical or NaN | Vectors not L2-normalized, or vector matrix loaded with wrong stride | `crushctl debug vector <shot_id>` prints norm and first 8 values |
| Works on John's Mac, fails on another Mac | Something not bundled (ffmpeg found on PATH, model in a dev folder), or CoreML op unsupported on older macOS | Test on a clean macOS VM or a fresh user account; `doctor` reports where each dependency was resolved from |
| Embedding is slow (seconds per frame) | CoreML provider silently fell back to CPU, or the ONNX graph has ops CoreML rejects | `doctor` prints the active execution provider and per-stage ms; if CPU, check ort's CoreML op-support log |
| Way too many shots / too few | Threshold or min-scene-length | `crushctl debug scenes <video>` writes a CSV of per-frame scores; plot it, pick threshold, don't guess |
| Shots split fine but thumbnails wrong | ffmpeg seek accuracy (`-ss` before vs after `-i`) | `--debug` keeps the extracted frame; compare to timecode by eye |
| Transcript text on the wrong shot | Time alignment overlap logic | `crushctl debug align <video>` prints segment ↔ shot table |
| Transcript garbage | Audio resample not 16 kHz mono, or wrong model file | `--debug` keeps the `.wav` ffmpeg produced; play it |
| Works on fixtures, fails on real 4K | Memory or decode time | job log duration column; downscale in the ffmpeg command, not in Rust |
| Ingest silently does nothing | sha256 already present | `crushctl jobs --video <path>` shows the skip reason |

### 11.3 Debug tooling built into the product

- `--debug` flag on ingest: writes `debug/<job_id>/` containing extracted frames, the resampled wav, per-frame scene scores CSV, raw vectors as JSON, and the exact ffmpeg command lines used. Job row stores `debug_dir`.
- `crushctl debug` subcommands: `vector`, `scenes`, `align`, `frame`, `ffmpeg-cmd` — each prints one stage's raw output for one item.
- `crushctl jobs` — list/filter the job log; `--failed` shows error text.
- `crushctl doctor` — checks bundled ffmpeg resolves and runs, models present with expected sha256, ort's active execution provider (CoreML vs CPU), whisper Metal enabled, available RAM, SQLite migrations applied, embedding_meta matches models. Run this first on any "it doesn't work".
- `crushctl reembed --all` and `crushctl resplit <video>` — re-run one stage without reingesting.
  Resplit is evidence-neutral for shots whose content-addressed ids return: `replace_shots`
  deletes only the shots that genuinely vanished and updates the survivors in place, so shot-keyed
  feedback, annotations, assessments, vectors, and reference items survive. Plan items on vanished
  shots are cleaned up (never silently rewritten); stored plan items keep their previously
  validated boundaries, and the render-time clamp still refuses honestly at render time if an item
  no longer fits its shot.

### 11.4 Testing layers

1. **Unit tests per crate**, run in CI without GPU (ort CPU provider is fine for fixtures).
2. **Golden tests** (11.1) — the ones that catch the expensive bugs.
3. **Fixture integration test** — ingest all fixture clips end to end into a temp SQLite, then run 5 canned queries and assert the expected shot is in the top 3. This is the "does search actually work" test and the acceptance gate for Phase 1.
4. **Real-footage smoke** — manual, documented in `docs/smoke.md`: 5 hours of real footage, 10 queries John writes before running, score recorded in a table.
5. **Clean-machine install test** — the `.dmg` on a Mac that has never seen the project (a fresh user account is acceptable). This is the Phase 2 gate.

### 11.5 Rules for the contractor agents

- A stage is not done until its golden test passes. No exceptions, no "close enough".
- When a golden test fails, fix Rust; never edit the golden file to make it pass.
- Every stage logs its inputs' identifiers and its outputs' counts at INFO. Silence is a bug.
- ffmpeg commands are built in one function per purpose and logged verbatim so a human can paste them into a terminal.

## 12. Integrations

| Integration | Purpose | Required for MVP? | Notes |
|---|---|---:|---|
| Bundled ffmpeg / ffprobe | decode, thumbnails, audio extract, clip export | Yes | Static arm64 build, sha256 pinned, LGPL build to keep licensing simple |
| Apple CoreML / Metal | acceleration for CLIP and Whisper | Yes for speed | CPU fallback must work |
| GitHub Releases | model downloads and app updates | Yes | John's repo; models are ~500 MB, host them as release assets |
| Apple Developer Program | signing and notarization | Yes for Phase 2, optional for John's own build | $99/yr |
| NexusCore (Qdrant, ai-srv, NAS) | none | No | Not used by the product. John can still index NAS footage by mounting it on his Mac. |

## 13. Agent Handoff Plan

| Agent | Responsibility | Input | Output |
|---|---|---|---|
| Contractor (Cursor/Codex) | Build tasks 0–13 in order | This blueprint, Task packs | Working repo, passing tests |

**Routing rule:** Tasks 0, 4, 6, 7, 8, 11, 12a–c, 13 go to **Cursor on the Mac** only — they need CoreML, Metal, arm64 ffmpeg, or Tauri. Tasks 1, 2, 3, 5, 9, 10 may go to Codex on Linux. "Compiles on Linux" is not evidence for a Mac task. Full protocol in `docs/blueprint-review.md`.
| QA | Run §11.4 layers 1–3, then smoke | Repo, fixtures | qa-report.md |
| Docs | README, setup, smoke doc | Repo | docs/ |
| UX | Tauri app screens per §7 | §7, §8 | screen spec + single HTML page |
| Launch | none in Phase 1 | — | — |

## 14. Contractor Build Plan

Tasks are ordered so nothing depends on something unbuilt. Each is one review session.

### Task 0: Feasibility spike (Cursor, Mac, throwaway)
- Goal: one binary outside the workspace proving the runtime stack on John's Mac: load a CLIP ONNX with `ort` and confirm CoreML provider is active; transcribe 10 s with `whisper-rs` Metal; spawn a bundled static ffmpeg. Print ms for each.
- Acceptance: all three run; a written note of versions that worked and any build flags. Two days max.
- Do not: write product code; do not merge the spike into the workspace.
- Human review: **go / no-go.** If CoreML or Metal fails, decide CPU-only vs. a different runtime before Task 1.

### Task 1: Workspace, config, tracing, job log, repo hygiene
- Goal: empty but runnable skeleton with all crates, `crush --version`, `crushctl doctor` stubbed; `LICENSE` (Apache-2.0), `CONTRIBUTING.md`, `THIRD_PARTY.md`, `.github/workflows/ci.yml` running `cargo test` on CPU.
- Files: `Cargo.toml`, `crates/core/*`, `crates/cli/*`, `crush.example.toml`, repo root files.
- Acceptance: `cargo build` clean on Linux; `crushctl doctor` prints checks (may all be "unchecked"); tracing writes JSON log with `job_id` field.
- Do not: add any stage logic yet.
- Human review: crate layout matches §10; config keys are readable.

### Task 2: SQLite store + migrations
- Goal: `store` crate with migrations for every table in §9 including `shot_vectors`, typed query functions, `owner_id` on all owned tables, app-data directory resolution for macOS.
- Acceptance: migrations apply on fresh DB and are no-ops on second run; tests insert/read a video, shot, job.
- Do not: put SQL strings anywhere outside `store`.
- Human review: schema matches §9 exactly.

### Task 3: Python reference kit + fixtures
- Goal: `reference/` scripts and 3–5 short fixture clips with golden outputs committed.
- Acceptance: `make golden` regenerates all files deterministically; README explains each script; fixture total ≤ 20 MB.
- Do not: pull fixture clips from copyrighted sources — use John's own or public-domain footage.
- Human review: John supplies or approves the clips.

### Task 4: Bundled ffmpeg + wrapper
- Goal: `sidecars/` with static ffmpeg/ffprobe and a fetch script; resolver that prefers the bundled binary and records where it resolved from; functions for probe, extract downscaled frames at N fps, extract 16 kHz mono wav, single-frame thumbnail, clip export. Each builds one command line and logs it.
- Acceptance: unit tests on fixtures; `--debug` keeps outputs; commands pasteable into a terminal and work.
- Do not: use ffmpeg bindings.
- Human review: seek accuracy — thumbnail matches timecode.

### Task 5: Scene detector
- Goal: HSV histogram delta detector with threshold + min-scene-length; `crushctl debug scenes` CSV.
- Acceptance: scene golden test passes (§11.1 tolerance); shots written to SQLite with `scene_score`.
- Do not: add ML-based detection.
- Human review: plot one CSV, sanity-check the threshold.

### Task 6: CLIP ONNX export + model downloader
- Goal: `export_clip_onnx.py` producing image and text encoder ONNX with fixed shapes and only CoreML-friendly ops; models published as release assets; Rust downloader with progress, sha256 check, resume.
- Acceptance: `onnxruntime` in Python loads both and matches PyTorch output; downloader survives a killed connection; `embedding_meta` row populated by doctor.
- Human review: model choice confirmed (see Open Questions).

### Task 7: Embed stage — preprocessing golden first
- Goal: Rust preprocessing (resize, center-crop, RGB, normalize with CLIP mean/std) matching the reference tensor.
- Acceptance: `preprocess_golden` passes at 1e-3. **Stop here and get review before Task 8.**
- Do not: touch the ONNX session yet.
- Human review: this is the highest-risk task in the project.

### Task 8: Embed stage — image + text encode with ort
- Goal: ort sessions (CoreML with CPU fallback), image and text embeddings, tokenizer, L2 normalize, write to `shot_vectors`.
- Acceptance: image and text golden tests pass (cos > 0.999) on **both** CoreML and CPU providers; tokenizer exact; `crushctl debug vector` works.
- Do not: batch-optimize yet.
- Human review: run doctor, confirm CoreML is actually active and note ms per frame.

### Task 9: Search
- Goal: load vectors into memory on start; query → text vector → cosine scan top-K filtered by `owner_id` → join SQLite → ranked result struct; CLI output table. Hybrid formula (start dumb, documented): `score = cosine + 0.15 × (1 if any query word appears in the shot's transcript else 0)`.
- Acceptance: fixture integration test — 5 canned queries, expected shot in top 3.
- Human review: try 3 queries of your own on fixtures.

### Task 10: Transcribe stage + alignment
- Goal: whisper-rs on the wav from Task 4, segments to SQLite, overlap alignment to shots, `crushctl debug align`.
- Acceptance: WER golden passes; alignment table correct on fixtures.
- Human review: read one transcript.

### Task 11: Hybrid ranking + clip export + ingest orchestration
- Goal: `ingest` runs stages in order with job rows, idempotent by sha256, `resplit`/`reembed` subcommands; transcript keyword hits boost visual results; `clip` exports a shot.
- Acceptance: ingest a fixture folder twice — second run skips; `jobs --failed` shows injected failure correctly; killing the process mid-video and re-running resumes that video without duplicates; cancel from CLI stops within 5 s; one video at a time; thread caps applied.
- Human review: end-to-end on fixtures.

### Task 12a: Tauri shell + commands
- Goal: Tauri 2 project in `crates/app`; ffmpeg as sidecar; commands `add_folder`, `job_status`, `cancel`, `search`, `shot_detail`, `export_clip`, `doctor`; a blank page that calls `doctor` and shows the result.
- Acceptance: app launches; `doctor` output visible in the window; sidecar ffmpeg resolves from the bundle.
- Do not: build screens yet; do not call a CLI binary from the app — call the crates directly.

### Task 12b: First-run and Library screens
- Goal: model download screen with progress and retry; Library screen with folders, per-video progress, cancel, and job errors.
- Acceptance: fresh app data dir → download completes → fixture folder indexes with visible progress → cancel works.

### Task 12c: Search and Shot detail screens
- Goal: search box (Cmd-F focuses), thumbnail grid, shot detail with player, timecodes, transcript, copy path, export clip.
- Acceptance: search returns results with thumbnails, a shot plays, a clip exports to a chosen location.
- Human review: **use it for ten minutes on real footage before Task 13.**

### Task 13: Build, sign, smoke
- Goal: GitHub Actions macOS workflow producing a `.dmg`; codesign + notarize when credentials are present, unsigned dev build otherwise; `docs/smoke.md`.
- Acceptance: `.dmg` installs and runs `doctor` green on a fresh macOS user account with no dev tools; 5-hour smoke completed and scored.
- Human review: smoke score table; clean-machine test result.

**Should-have tasks (after 13):** 14 watch folder, 15 Tauri updater, 16 Intel build.

## 15. QA Plan

- Happy path: ingest fixtures, search 5 queries, export a clip, play it.
- Edge: file with no audio; file with one continuous shot; vertical video; very long file (2 h); filename with spaces/unicode; re-ingest unchanged file; ingest modified file with same name.
- Failure: app killed mid-ingest (job row shows error, no partial shots left orphaned, re-run resumes); missing model file (doctor fails clearly); ffmpeg missing; GPU absent (CPU fallback works, warns).
- Data: every shot has a thumbnail file that exists; every shot has a vector row and vice versa (`doctor --deep`).
- Regression: golden tests on every commit.

## 16. Documentation Plan

- README: what it is, install (open the dmg), first folder, first search; developer setup separately.
- `docs/debugging.md`: §11 expanded with real examples once found.
- `docs/smoke.md`: the manual real-footage test and its score history.
- `docs/phase2.md`: what shipping to other users needs (signing, updater, crash reporting opt-in, support path) — one page.
- Known limitations: CLIP weak on specific people and on-screen text; Whisper on music-heavy audio; no support for RAW/ProRes edge codecs beyond what ffmpeg handles.

## 17. Risks and Decisions

| Decision / Risk | Recommendation | Why |
|---|---|---|
| Rust before pipeline is proven | Accepted; answer-key kit + Task 7 hard stop mitigates | Rust is the long game; the debugging trap is the real cost and is addressed directly |
| ffmpeg bindings vs subprocess | Subprocess | Reliability; bindings later only if decode is measured as the bottleneck |
| CLIP model size | ViT-B/32 to start | Fast, 512-dim, good enough to prove search; swap to ViT-L or SigLIP later via `embedding_meta` + `reembed` |
| Whisper model size | `small` default, `base` on 8 GB Macs | Metal makes small fast enough; medium risks memory pressure alongside CLIP and the app |
| 4K decode time on a laptop | Downscale in ffmpeg to 480p for detection and 224px for embedding; index at low priority | Never decode full-res; don't cook the user's laptop |
| No vector database | In-process cosine scan | Simpler, nothing to install, fast enough to 100k shots; `usearch` behind the same trait if ever needed |
| CoreML op coverage | Export ONNX with a conservative opset; test both providers | CoreML silently falls back to CPU on unsupported ops — doctor must expose this |
| Bundling ffmpeg | LGPL static build as Tauri sidecar | Users won't install it; GPL build would complicate distribution |
| Apple signing | Needed before anyone but John installs it | Unsigned apps are blocked by Gatekeeper for normal users |
| Phase 2 migration debt | `owner_id` kept; clean-machine test in Phase 1 | Makes Phase 2 a distribution problem, not a rewrite |
| Crate churn (`ort`, `whisper-rs`) | Pin exact versions; record in docs | These crates move fast and break APIs |

## 18. Version Roadmap

- **V0 (Tasks 0–9):** ingest fixtures, visual search works, golden tests green.
- **V1 (Tasks 10–13):** transcripts, hybrid search, clip export, Tauri app, signed dmg, smoke and clean-machine tests passed. **Phase 1 done.**
- **V1.5:** watch folder, updater, assemble rough-cut, Intel build.
- **V2 (Phase 2):** ship to other users — support docs, opt-in crash reports, optional shared/synced libraries, Windows if demanded, Rust ffmpeg bindings if decode is the bottleneck.

## 19. Open Questions

1. John's Mac: chip and RAM — sets the default model sizes and is the primary dev/test machine.
2. Fixture clips — John supplies 3–5 short clips he owns.
3. Minimum macOS version to support — decides CoreML features available. Recommend macOS 13+.
4. Does John have an Apple Developer account, or should signing wait until Phase 2?
6. Project name and GitHub org — "Crush" is a placeholder; check it's not taken.
5. Who the Phase 2 users are — decides how polished onboarding must be and whether Intel support matters.

## 20. Final Build Instruction (paste to Contractor)

Build Crush per `project-blueprint.md`. Rust workspace, bundled ffmpeg via subprocess, `ort` (CoreML + CPU) for CLIP ONNX, `whisper-rs` (Metal), `rusqlite` for metadata and vectors, in-process cosine search, `clap` CLI, Tauri 2 app. No servers, no Docker, no Qdrant. Complete tasks 0–13 in order; hard stops for human review after Task 0, after Task 7, and after Task 12c. Respect the routing rule in §13. A stage is done only when its golden test passes; never edit golden files to pass. Every stage logs job_id and stage at INFO. Put `owner_id` on every owned record. Do not add features not in §6 Must Have. Show a summary and test output after each task.

## 21. Open Source

- **License:** Apache-2.0 (permissive, patent grant, matches the Rust ecosystem). MIT acceptable. Not GPL.
- **Third-party:** CLIP and Whisper weights are MIT; `ort`, Tauri, `whisper-rs`, `rusqlite` are MIT/Apache. ffmpeg must be the **LGPL** build — never `--enable-gpl`. All recorded in `THIRD_PARTY.md`.
- **Models:** hosted under the project's own GitHub Releases or Hugging Face org, sha256 in the repo. Never a personal bucket.
- **Privacy:** no telemetry, no crash reporting, no network calls except the model download. If ever added, opt-in with a visible toggle and a one-line privacy note in the README.
- **Contributors:** CI runs unit + golden tests on CPU for every PR; the answer key is how a reviewer proves a PR didn't break embeddings. `CONTRIBUTING.md` says: run `doctor`, run golden tests, never edit golden files.
- **Secrets:** signing and notarization credentials exist only in GitHub Actions secrets. An unsigned local build must be one command (`cargo tauri build`).
- **Governance:** John is sole maintainer in Phase 1. Revisit if outside PRs start landing.

## 22. Prior art reviewed

- **nanzhi84/Rushes** (Go/React, conversational editing agent, cloud LLM, no license): not reusable, different product. Adopted as ideas only: ffmpeg `-progress pipe:1`, SIGINT-to-process-group cancel, stable content-derived shot/segment ids, worker-writes-terminal-state rule. Folded into Tasks 4 and 11.
- **rushes.cc** (Vimeo alternative, 2026): name collision that ruled out "Rushes".
