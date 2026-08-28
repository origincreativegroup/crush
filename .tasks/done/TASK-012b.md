# TASK-012b: First-run + Library screens
Agent: Codex on the Mac. Branch: task/12b-library. Depends: 012a. Status: done.

## Acceptance
- [x] Fresh data dir → first-run screen shows download progress per file, retry on failure, then proceeds
- [x] Library: Add Folder (native picker + drag-drop), list of videos with status pill and progress bar, per-video error text expandable, Cancel button, Re-index
- [x] Indexing a fixture folder shows live progress; cancel works
- [x] Empty state text per blueprint §7

## Implementation record

Completed on 2026-08-28. The plain HTML/CSS/JavaScript Tauri front end now implements the first-run
model flow and the full Library workspace from the UX specification.

- First run renders each manifest asset with byte progress, keeps Continue disabled until every
  checksum-verified file is present, resumes partial downloads through the existing core downloader,
  and exposes a clear failure message with Retry. A real application run resumed and completed the
  model set in Tauri's app-data directory and recorded the expected embedding metadata.
- Library supports the native Tauri directory picker and window drag/drop, an empty state with both
  GUI and `crushctl ingest` paths, sortable visual rows with duration/resolution/shot count, status
  pills and stage progress, row selection, Cancel, and per-video Re-index.
- Pipeline and background-task snapshots drive the UI while work is running. Failed rows expose the
  job, stage, retained log path, full error, and a native clipboard action. Startup recovers abandoned
  jobs as interrupted failures so a killed process never leaves a permanently active-looking row.
- A deterministic browser harness exercises first-run progress, first-run failure/retry, empty,
  populated, active/cancellable, and failed/expanded states using the production CSS and JavaScript.
  The release app also exercised real re-indexing against an existing video, including interruption
  and launch-time recovery.
- `node --check`, `cargo fmt --all -- --check`, workspace clippy with warnings denied, and the complete
  workspace test suite pass. `cargo tauri build --bundles app` produced the release bundle, and
  `codesign --verify --deep --strict` validates the app and both bundled FFmpeg sidecars.
