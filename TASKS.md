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
| TASK-021 | Non-destructive recipes + photo/video render and export | Codex team (Mac) | in progress — photo + single-clip APPROVED (2026-08-30); ordered-reel defect fixed via TASK-036 (PR #41, merged into this branch as `4765ca0`), packet re-rendered and machine-verified; span rendering is executor-level only — app-level span reel/clip export lands with TASK-037; **only the human reel re-review remains** (`docs/task-021-render-review.md` § re-review request) |
| TASK-022 | Reel Studio catalogue and recipe importer | Codex (Mac) | merged onto the Task 021 branch — schema v11 spans/ledger/provenance, dry-run + idempotent importer, CLI + Library dialog, span rendering at the executor level (app-level span reel/clip export lands with TASK-037); waits behind 021 review |
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
| TASK-032 | Preference-learning evaluation remediation (018 prerequisite) | OpenCode | done — merged as PR #39 (`2766843`): media-disjoint split, production-scale composed-ranker gate, transactional withdrawal, netting probes; review fixes applied and confirmed |
| TASK-033 | Automatic sequence/repetition judgment (020 completion) | OpenCode | in review — PR #40 (`task/33-sequence`, stacked on #37): sequence signals + one-click suggestions + selects duplicate cap; reviewed MERGE, follow-ups applied (clique guard, CLI cap echo, median panic fix), CI green; merges after #37 |
| TASK-034 | Catalogue unification — span text in search, first-class spans (was: imported-evidence bridge) | OpenCode | backlog — reframed 2026-08-31 per John's direction (Crush and Reel Studio are one product lineage); after 021/022 merge; pairs with TASK-037 |
| TASK-035 | Render engineering follow-ups from the 2026-08-30 review | OpenCode | backlog — next stretch, after 021 merge; no golden changes |
| TASK-037 | First-class spans — adjustable boundaries, one catalogue (Reel Studio unification step 1) | OpenCode | backlog — impl plan ready (found: clamp lives in 4 places incl. migration 0011 SQL triggers → schema v12); after 021 merge; byte-stable for approved render paths |
| TASK-038 | Rename survival and shot-identity hardening (key feature) | OpenCode | implemented — `replace_shots` diffing replace stops resplit evidence loss (data-loss class), verified `crushctl relink` + app "Locate moved file…" flow, ingest `moved`/`renamed` reporting, identity audit recorded (path lookups are dedup/target resolution only); in review |
| TASK-039 | UX/UI enhancement track — full pass (craft + workflow) | Frontend lane | waves 1–3 done on `task/39-ux-wave1` as PR #42: collections reachable, AA contrast + focus, full keyboard, in-place search, reduced motion, design tokens (0 hex outside :root, 61 tokens), SF Mono actually rendering (latent stack bug), multi-select + batch ops, honest errors; reviewed each wave, review fixes applied, 29/29 harness; merges after #37; release DMG ships after this track (John's call) |
| TASK-040 | Backend contracts for UX follow-ups (search kind, video thumbs, render progress, video collection membership) | Backend lane | backlog — created 2026-08-31 from the UX track discoveries; one item needs John's decision (whole-video collection membership — recommendation: keep shot-level) |
| TASK-036 | Ordered-reel boundary-frame drops (021 review rejection) | OpenCode (Lane A idle) | engineering done — PR #41 squash-merged into the 021 branch (`4765ca0`): frame-exact items, exact-cut concat, silence-padded audio, fail-closed VIDEO-stream verification, fixture golden; packet re-rendered + machine-verified (see `docs/task-021-render-review.md`); **human reel re-review pending — gates 021** |

## OpenCode next stretch (2026-08-30)

Order: 032 → 033 → 034 → 035 (034/035 wait for the 021/022 merge; 032/033 can start from
`origin/main` conventions now, in their own worktrees). Every task: one branch, one PR, full gates
(fmt, warnings-denied clippy, workspace tests, `npm run test:ui`), truthful record in the task file.
Human gates are unchanged and none of these tasks may claim or bypass them.
