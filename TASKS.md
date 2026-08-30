# Crush — task board

| ID | Task | Agent | Status |
|---|---|---|---|
| TASK-000 | Feasibility spike (CoreML / Metal / ffmpeg sidecar) | Codex (Mac) | done — human GO received 2026-08-27 |
| TASK-001 | Workspace, config, tracing, job log, repo hygiene | Codex | done — macOS local gates + Linux CI green |
| TASK-002 | SQLite store + migrations | Codex | done — typed API, FTS, vectors, jobs, integrity checks |
| TASK-003 | Reference kit + fixtures | Codex + John (review) | done — deterministic goldens and scene review approved |
| TASK-004 | Bundled ffmpeg + wrapper | Codex (Mac) | done — PR #4 squash-merged as `7e17792` |
| TASK-005 | Scene detector | Codex | done — four goldens, store/CSV parity, and 3.47 s CPU acceptance pass |
| TASK-006 | CLIP ONNX export + model downloader | Codex (Mac) | done — `models-v1` release and live resume/corruption acceptance pass |
| TASK-007 | Embed preprocessing golden | Codex (Mac) | done — exact goldens and John’s two-run Mac approval |
| TASK-008 | Embed with ort (CoreML + CPU) | Codex (Mac) | done — CPU/CoreML cosines 1.0; doctor and Linux/macOS CI green |
| TASK-009 | Search + hybrid ranking | Codex | done — five approved fixture queries, hybrid ranking, and 7.55 ms scan |
| TASK-010 | Transcribe + alignment | Codex (Mac) | done — WER 0.000; Metal active; silent fast path 0.149 ms |
| TASK-011 | Ingest orchestration, cancel/resume, clip export | Codex (Mac) | done — resumable pipeline, cancellation, reprocessing, clip export |
| TASK-012a | Tauri shell + commands | Codex (Mac) | done — signed `.app`, bundled sidecars, command bridge verified |
| TASK-012b | First-run + Library screens | Codex (Mac) | done — native Library workflow, recoverable ingest, and UI states verified |
| TASK-012c | Search + Shot detail screens | Codex (Mac) | done — merged as PR #14 |
| TASK-013 | Build, sign, smoke, clean-machine test | Codex (Mac) | deferred — original plan retained and broadened by TASK-023; obsolete PR #13 closed |
| TASK-014 | Photo/video DAM schema + Reel Studio editorial feedback foundation | Codex (Mac) | done — merged through DAM foundation |
| TASK-015 | JPEG/PNG ingest, thumbnails, vectors, and mixed-media search | Codex (Mac) | done — real-model photo vertical slice |
| TASK-016 | RAW/HEIF/TIFF photo ingest + production-video source support | Codex (Mac) | done — full-decode capability gates, source metadata, color-aware proxies |
| TASK-017 | General strong-shot and explainable aesthetic analysis | Codex (Mac) | done — cold-start technical/design/moment evidence, calibrated and backfillable |
| TASK-018 | Previous-work examples + personal-style learner/evaluation | OpenCode | 018a/b merged (#25/#29); human style proof OPEN — fresh review found evaluation gaps; UI experimental |
| TASK-019 | Mixed-media review and DAM organization | OpenCode | done — 019a PR #30, 019b PR #31 |
| TASK-020 | Strong-shot recognition + user-style selects and clip/reel planning | OpenCode + Codex | 020a merged (#33/#34); 020b UI merged (#36); automatic sequence/repetition judgment remains open |
| TASK-020b | Plans UI + selection provenance | Codex (Mac) | done — PR #36; Linux/macOS CI and local/browser acceptance green |
| TASK-021 | Non-destructive recipes + photo/video render and export | Codex team (Mac) | in progress — no-clobber + schema-v10 durable recipe/job foundation; Projects/reel playback/progressive filters/Preferences UX underway; renderer/goldens remain |
| TASK-022 | Reel Studio catalogue and recipe importer | Claude (Mac) | in review — `task/22-import` (stacked on #37): schema v11 spans/ledger/provenance, dry-run + idempotent importer, CLI, Library import dialog, span rendering; John's 021 render review still gates acceptance |
| TASK-023 | DAM release packaging, UI CI, and clean-machine acceptance | Codex (Mac) | backlog — after end-to-end photo/video render workflow |
| TASK-024 | Source-fidelity truthfulness + ranking breakdown export | OpenCode | done — PR #22; orientation truthful, real ICC tests, plain-language breakdown |
| TASK-025 | Store hardening (feedback immutability, owner-safe upserts, integrity) | OpenCode | done — PR #21; schema v5 triggers, owner isolation tests |
| TASK-026 | Pipeline ops (analyze staleness, cancellable renders, photo jobs) | OpenCode | done — PR #23; schema v6 photo job lifecycle |
| TASK-027 | App robustness + honest UI harness | OpenCode | done — PR #24 (+ integration fix #27); CSP, async feedback, iframe harness |
| TASK-028 | Cross-platform contracts, Windows shell, and CI | Codex team | backlog — begins after Task 021 interfaces stabilize; CPU baseline |
| TASK-029 | Windows source decoding and media rendering backends | Codex team | backlog — portable software path plus optional NVENC |
| TASK-030 | Portable model runtime and optional Windows acceleration | Codex team | backlog — ONNX CPU/CoreML/CUDA/DirectML; PyTorch training/export only |
| TASK-031 | Windows packaging and clean-machine parity acceptance | Codex team + John (review) | backlog — after Tasks 023 and 028–030 |

## Current continuation (2026-08-29)

Codex reviewed the merged OpenCode work and is continuing in John's order: 020b, 021, 022, 023.
See `docs/review-2026-08-29.md` for findings and reproduced style probes. Each task gets its own
PR and current Linux/macOS verification. Hard stops stay human: 018 held-out style proof,
021 render-golden review, 023 clean-machine acceptance. Synthetic probes are not style approval.

User review added to the 021–023 completion plan on 2026-08-29: smooth boundary-safe reel
playback, a natural Projects workflow, progressive disclosure instead of a wall of filter
dropdowns, and Preferences/creative-taste language in place of the ambiguous Style label. These
are acceptance criteria for the remaining work and are covered by the UI harness and final smoke.

Cross-platform planning was added without replacing the DAM roadmap. Tasks 021–023 remain the
ordered product/Mac release path while introducing portable recipe, backend, path and manifest
contracts. Tasks 028–031 then deliver the Windows shell/CI, media backends, optional CUDA/DirectML
acceleration and clean-machine parity. See `docs/platform-architecture.md`. PyTorch is reserved for
training/evaluation and validated ONNX export; the shipped app always retains a CPU path.
