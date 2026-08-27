# TASK-012a: Tauri shell + commands
Agent: Cursor on the Mac. Branch: task/12a-tauri. Depends: 011.

## Instructions
1. `cargo tauri init` into `crates/app` (Tauri 2, pinned). Front end: plain HTML/CSS/JS in `crates/app/ui/` — no framework, no bundler.
2. ffmpeg/ffprobe as Tauri **sidecars** (`externalBin`), and `resolve()` from Task 4 must find them via `tauri::path::resource_dir`/sidecar API when bundled.
3. Commands (`#[tauri::command]`), all calling the crates directly: `doctor`, `models_status`, `models_download` (emits `download-progress` events), `add_folder(path)`, `list_videos`, `job_status` (emits `ingest-progress`), `cancel_ingest`, `search(q, top)`, `shot_detail(id)`, `export_clip(id, out)`, `open_in_finder(path)`.
4. Long work runs on a background thread; commands return immediately with job ids; progress via events.
5. Blank page with a "Run doctor" button rendering the result as text.
6. App data dir = Tauri's app_data_dir (same location core resolves).

## Acceptance
- [ ] `cargo tauri dev` opens; doctor button shows output including `ffmpeg source=Bundled` when run from a built .app
- [ ] `cargo tauri build` produces a .app that launches on John's Mac
