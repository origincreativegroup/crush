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
| TASK-021 | Non-destructive recipes + photo/video render and export | Codex team (Mac) | done — merged to main as PR #37 (`d915f3d`); render-golden review PASSED (2026-08-30 review + 2026-08-31 delegated reel re-review, `docs/task-021-render-review.md`); advanced treatment matrix deferred to the unification roadmap as honest capability errors |
| TASK-022 | Reel Studio catalogue and recipe importer | Codex (Mac) | done — shipped in PR #37; PR #38 closed as fully contained |
| TASK-023 | DAM release packaging, UI CI, and clean-machine acceptance | Codex (Mac) | in progress — tooling complete (one-command `scripts/package-macos.sh`, build provenance, ad-hoc labeling, fail-closed verify); final DMG cut after the current task flood; **clean-machine human acceptance remains (John)** |
| TASK-024 | Source-fidelity truthfulness + ranking breakdown export | OpenCode | done — PR #22; orientation truthful, real ICC tests, plain-language breakdown |
| TASK-025 | Store hardening (feedback immutability, owner-safe upserts, integrity) | OpenCode | done — PR #21; schema v5 triggers, owner isolation tests |
| TASK-026 | Pipeline ops (analyze staleness, cancellable renders, photo jobs) | OpenCode | done — PR #23; schema v6 photo job lifecycle |
| TASK-027 | App robustness + honest UI harness | OpenCode | done — PR #24 (+ integration fix #27); CSP, async feedback, iframe harness |
| TASK-028 | Cross-platform contracts, Windows shell, and CI | Codex team | backlog — begins after Task 021 interfaces stabilize; CPU baseline |
| TASK-029 | Windows source decoding and media rendering backends | Codex team | backlog — portable software path plus optional NVENC |
| TASK-030 | Portable model runtime and optional Windows acceleration | Codex team | backlog — ONNX CPU/CoreML/CUDA/DirectML; PyTorch training/export only |
| TASK-031 | Windows packaging and clean-machine parity acceptance | Codex team + John (review) | backlog — after Tasks 023 and 028–030 |

## Current state (2026-09-01)

The Mac release candidate is assembled on main: Tasks 021+022+036 (durable photo/clip/ordered-reel
rendering, schema v11–v12, the Reel Studio importer with adjustable first-class spans), 032/033
(style-eval remediation, sequence judgment), 035 (render follow-ups), 038 (rename survival + shot
identity), 039 (full UX/UI pass + compare auto-advance + learned wording), and 040's render-progress
item are all merged through reviewed PRs. The render-golden review and the 018 style proof are
RECORDED (delegated reviewer per John's 2026-08-31 directive — see
`docs/task-021-render-review.md` and `docs/style-proof-review.md`; John may amend either).
Packaging is one command (`scripts/package-macos.sh`) with build provenance and honest ad-hoc
labeling.

Remaining before the release: TASK-037 merge (PR #46), TASK-034 (catalogue unification — span text
into search, confirmation flow, schema v13), TASK-040 (search kind argument, video thumbnails),
the final DMG from merged main, and John's clean-machine acceptance (`docs/smoke.md`) — the one
human gate that cannot be delegated. 028–031 remain the additive Windows track.
`docs/platform-architecture.md`: PyTorch stays training/evaluation-only; the shipped app retains a
CPU path.
| TASK-032 | Preference-learning evaluation remediation (018 prerequisite) | OpenCode | done — merged as PR #39 (`2766843`) |
| TASK-033 | Automatic sequence/repetition judgment (020 completion) | OpenCode | done — merged as PR #40 (`cb7f9c6`) |
| TASK-034 | Catalogue unification — span text in search, first-class spans | OpenCode | in progress — dispatching post-037 (schema v13 serializes after v12) |
| TASK-035 | Render engineering follow-ups from the 2026-08-30 review | OpenCode | done — merged as PR #45 (`cfd52c3`): shared duration-tolerance rule, recovery off the setup thread, memoized hashing/probes, real ffmpeg progress, preset single-source; byte-stability proven |
| TASK-037 | First-class spans — adjustable boundaries, one catalogue (Reel Studio unification step 1) | OpenCode | in review — PR #46: schema v12, all four clamps → source-video range, derived `adjusted` provenance, re-import never reverts, span reel AND clip export unblocked from the app; byte-stable |
| TASK-038 | Rename survival and shot-identity hardening (key feature) | OpenCode | done — merged as PR #44 (`22e3984`): resplit evidence-loss fix (data-loss class), hash-verified relink (CLI + app), ingest moved/renamed/duplicate reporting, identity audit |
| TASK-039 | UX/UI enhancement track — full pass (craft + workflow) | Frontend lane | done — merged as PR #42 (`dafdd9d`) + follow-up PR #43: collections, AA contrast, focus, full keyboard, in-place search, reduced motion, design tokens, SF Mono fix, multi-select + batch ops, compare auto-advance (John: yes), learned-profile wording per the recorded verdict |
| TASK-040 | Backend contracts for UX follow-ups | Backend lane | backlog — search kind argument, video thumbnails; render progress done via #45; video collection membership DECIDED by John 2026-08-31: shot-level (a) — video rows keep the honest hint |

## Next (2026-09-01)

Finish TASK-037 (PR #46) → TASK-034 → TASK-040 → final DMG → clean-machine acceptance. Every task:
one branch, one PR, full gates (fmt, warnings-denied clippy, workspace tests, `npm run test:ui`),
truthful record in the task file, reviewer pass before merge.
