# TASK-012a: Tauri shell + commands
Agent: Codex on the Mac. Branch: task/12a-tauri. Depends: 011. Status: done.

## Instructions
1. `cargo tauri init` into `crates/app` (Tauri 2, pinned). Front end: plain HTML/CSS/JS in `crates/app/ui/` — no framework, no bundler.
2. ffmpeg/ffprobe as Tauri **sidecars** (`externalBin`), and `resolve()` from Task 4 must find them via `tauri::path::resource_dir`/sidecar API when bundled.
3. Commands (`#[tauri::command]`), all calling the crates directly: `doctor`, `models_status`, `models_download` (emits `download-progress` events), `add_folder(path)`, `list_videos`, `job_status` (emits `ingest-progress`), `cancel_ingest`, `search(q, top)`, `shot_detail(id)`, `export_clip(id, out)`, `open_in_finder(path)`.
4. Long work runs on a background thread; commands return immediately with job ids; progress via events.
5. Blank page with a "Run doctor" button rendering the result as text.
6. App data dir = Tauri's app_data_dir (same location core resolves).

## Acceptance
- [x] `cargo tauri dev` opens; the doctor command and bridge are wired, and an integration test proves its output includes `ffmpeg source=Bundled` with a Tauri bundle layout
- [x] `cargo tauri build` produces a signed `.app` that launches on John's Mac

## Implementation record

Completed on 2026-08-28. The Tauri 2 shell is pinned to `tauri 2.11.3` and `tauri-build
2.6.3`, with a no-bundler HTML/CSS/JavaScript front end and a minimal Doctor screen. The Rust
application command layer calls the existing core, store, pipeline, search, split, and embedding
crates directly.

- All requested commands are registered. Model download and ingest return job IDs immediately,
  execute on blocking background workers, and emit stable-schema progress events. Search and clip
  export also leave WebKit's UI thread while their CPU or process work runs.
- Tauri `externalBin` packages the pinned FFmpeg and FFprobe sidecars. Startup registers Tauri's
  resource directory with the shared resolver, which checks the adjacent macOS executable directory
  and reports the production pair as `Bundled`. The download script creates Tauri target-triple
  aliases, and macOS CI prepares the same layout.
- The bundle ID and core data path now agree on Tauri's app-data location:
  `~/Library/Application Support/dev.crush.app`. Existing architecture and setup documentation was
  updated to match.
- `cargo tauri dev --no-watch` opened successfully. `cargo tauri build --bundles app` produced
  `target/release/bundle/macos/Crush.app`; it was ad-hoc signed as a complete bundle, passed
  `codesign --verify --deep --strict`, contained both sidecars, declared macOS 10.15 as its minimum,
  and stayed running after launch through `/usr/bin/open -n`.
- The Doctor formatting/resolution integration test uses a realistic
  `Crush.app/Contents/{Resources,MacOS}` layout and asserts `ffmpeg source=Bundled`, the FFmpeg
  version, and database schema. Direct visual clicking was unavailable during the final run because
  the Mac was locked; the application launch, command implementation, JavaScript invoke wiring, and
  bundled-command output were verified independently.
- `cargo fmt --all -- --check`, workspace clippy with warnings denied, and the complete workspace test
  suite pass on the Apple M4 Pro. The suite includes the real model/Metal fixtures and new sidecar
  resolver coverage.
