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
| TASK-021 | Non-destructive recipes + photo/video render and export | Codex team (Mac) | in progress — 2026-08-30 human render-golden review (OpenCode acting reviewer): photo + single-clip APPROVED, ordered-reel REJECTED for boundary-frame drops (TASK-036); reel re-render + human re-review required |
| TASK-022 | Reel Studio catalogue and recipe importer | Codex (Mac) | merged onto the Task 021 branch — schema v11 spans/ledger/provenance, dry-run + idempotent importer, CLI + Library dialog, span rendering; waits behind 021 review |
| TASK-023 | DAM release packaging, UI CI, and clean-machine acceptance | Codex (Mac) | in progress — `verify-release.sh`, `doctor --deep`, `docs/release.md`, real-language harness + smoke checklist in place; clean-machine human acceptance remains |
| TASK-024 | Source-fidelity truthfulness + ranking breakdown export | OpenCode | done — PR #22; orientation truthful, real ICC tests, plain-language breakdown |
| TASK-025 | Store hardening (feedback immutability, owner-safe upserts, integrity) | OpenCode | done — PR #21; schema v5 triggers, owner isolation tests |
| TASK-026 | Pipeline ops (analyze staleness, cancellable renders, photo jobs) | OpenCode | done — PR #23; schema v6 photo job lifecycle |
| TASK-027 | App robustness + honest UI harness | OpenCode | done — PR #24 (+ integration fix #27); CSP, async feedback, iframe harness |
| TASK-028 | Cross-platform contracts, Windows shell, and CI | Codex team | backlog — begins after Task 021 interfaces stabilize; CPU baseline |
| TASK-029 | Windows source decoding and media rendering backends | Codex team | backlog — portable software path plus optional NVENC |
| TASK-030 | Portable model runtime and optional Windows acceleration | Codex team | backlog — ONNX CPU/CoreML/CUDA/DirectML; PyTorch training/export only |
| TASK-031 | Windows packaging and clean-machine parity acceptance | Codex team + John (review) | backlog — after Tasks 023 and 028–030 |

## Current continuation (2026-08-30)

Task 022 (Reel Studio importer) is merged onto the Task 021 render branch: schema v11 manual spans
and an import ledger, dry-run + idempotent importer, `crushctl import`, the Library import dialog,
and span rendering through the reel executor, with provenance pills that never claim a preference
profile. Task 023 release tooling landed alongside: `scripts/verify-release.sh`, `crushctl doctor
--deep`, and `docs/release.md` (install/privacy/data-location/backup/relink/uninstall).

The editor-facing pass from the 2026-08-30 review is implemented and covered by the browser
harness: a detail-player reopen fix, a Standout control, Pick/Reject/Min-rating Review filters,
photo export straight from the detail drawer, photo re-index and remove-from-library (originals
untouched), an inline "stored intent, not yet renderable" warning for pacing/crop/grade, a visible
searching state, editor-language status labels, the consumer empty-state copy, hidden export audit
snapshots, help, and consistent timecodes.

Human hard stops remain exactly where they must: **021 render-golden review** (visual/color/
boundary/audio/manifest artifacts), **018 held-out style proof**, and **023 clean-machine
acceptance** (`docs/smoke.md`). No agent test or CI is release approval. 028–031 remain the additive
Windows track. See `docs/platform-architecture.md`. PyTorch is reserved for training/evaluation and
validated ONNX export; the shipped app always retains a CPU path.
| TASK-032 | Preference-learning evaluation remediation (018 prerequisite) | OpenCode | in progress — PR #39 (`task/32-style-eval`): media-disjoint split, composed-ranker gate, withdrawal invalidation; gates any "learned" claim |
| TASK-033 | Automatic sequence/repetition judgment (020 completion) | OpenCode | in progress — PR #40 (`task/33-sequence`, stacked on #37): sequence signals + one-click suggestions + selects duplicate cap |
| TASK-034 | Imported-evidence search + explicit confirmation bridge (022 follow-up) | OpenCode | backlog — next stretch, after 021/022 merge |
| TASK-035 | Render engineering follow-ups from the 2026-08-30 review | OpenCode | backlog — next stretch, after 021 merge; no golden changes |
| TASK-036 | Ordered-reel boundary-frame drops (021 review rejection) | Lane A (Codex; OpenCode if idle) | **open — gates 021**; reel artifact re-render + human re-review required |

## OpenCode next stretch (2026-08-30)

Order: 032 → 033 → 034 → 035 (034/035 wait for the 021/022 merge; 032/033 can start from
`origin/main` conventions now, in their own worktrees). Every task: one branch, one PR, full gates
(fmt, warnings-denied clippy, workspace tests, `npm run test:ui`), truthful record in the task file.
Human gates are unchanged and none of these tasks may claim or bypass them.

