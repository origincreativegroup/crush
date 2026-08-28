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
| TASK-013 | Pre-pivot release workflow and smoke plan | Claude (Mac) | superseded — useful packaging notes retained in TASK-023; PR #13 closed |
| TASK-014 | Photo/video DAM schema + Reel Studio editorial feedback foundation | Codex (Mac) | done — merged through DAM foundation |
| TASK-015 | JPEG/PNG ingest, thumbnails, vectors, and mixed-media search | Codex (Mac) | done — real-model photo vertical slice |
| TASK-016 | RAW/HEIF/TIFF photo ingest + production-video source support | Codex (Mac) | next — format support matrix, metadata, color, proxies |
| TASK-017 | Explainable technical and aesthetic analysis for photos/video | Codex (Mac) | backlog — depends TASK-016 |
| TASK-018 | Personal-style learner + held-out ranking evaluation | Codex (Mac) | backlog — centroid baseline exists; proof and controls required |
| TASK-019 | Mixed-media review and DAM organization | Codex (Mac) | backlog — comparisons, collections, versions, metadata, safety |
| TASK-020 | User-style photo selection and video clip/reel planning | Codex (Mac) | backlog — depends TASK-017–019 |
| TASK-021 | Non-destructive recipes + photo/video render and export | Codex (Mac) | backlog — rendered derivatives only; originals stay immutable |
| TASK-022 | Reel Studio catalogue and recipe importer | Codex (Mac) | backlog — import confirmed editorial evidence into the unified schema |
| TASK-023 | DAM release packaging, UI CI, and clean-machine acceptance | Codex (Mac) | backlog — after end-to-end photo/video render workflow |
