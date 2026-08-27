# TASK-004: Bundled ffmpeg + wrapper
Agent: Cursor on the Mac. Branch: task/04-ffmpeg. Depends: 001.

## Goal
`crush-stage-split::ffmpeg` module: resolves the bundled binary and exposes exactly five operations, each logging its full command line.

## Instructions
1. Download a static **LGPL** arm64 ffmpeg + ffprobe into `sidecars/` (git-ignored). Write `sidecars/SOURCES.md`: URL, version, license confirmation (`ffmpeg -version` must NOT show `--enable-gpl`), sha256. Add `scripts/get-sidecars.sh` that fetches and verifies.
2. `resolve() -> Resolved { path, source: Bundled | DevSidecarDir | Path }`. Order: next to the executable (Tauri bundle) → `sidecars/` (dev) → PATH (dev only, warn loudly). `doctor` prints `source`.
3. Operations (each returns the command as a String for logging and debug dumps):
   - `probe(path) -> Probe { duration_s, fps, width, height, has_audio }` via `ffprobe -v error -print_format json -show_streams -show_format`.
   - `sample_frames(path, fps, out_dir)`: `-i IN -vf "fps=FPS,scale=-2:480" -q:v 3 out_dir/f%06d.jpg`. Downscale in ffmpeg, never in Rust.
   - `extract_audio(path, out_wav)`: `-i IN -vn -ac 1 -ar 16000 -c:a pcm_s16le OUT` (what whisper wants).
   - `frame_at(path, t_s, out_jpg)`: `-ss T -i IN -frames:v 1 -q:v 2 OUT`. **`-ss` before `-i`** for speed; verify accuracy on fixtures is within 1 frame — if not, use `-ss` after `-i` for this op and note it.
   - `export_clip(path, start_s, end_s, out)`: `-ss S -to E -i IN -c copy OUT`; if `-c copy` produces a broken start (keyframe issue), fallback re-encode `-c:v libx264 -preset veryfast -crf 18 -c:a aac`. Always try copy first and log which path was taken.
4. Run ffmpeg with `nice`-equivalent priority (spawn with lowered `QOS_CLASS_UTILITY` via `-threads` cap from config; on macOS use `nice -n 10` prefix).
5. Progress: run long ffmpeg ops with `-progress pipe:1 -nostats` and parse the `out_time_us=` lines from stdout for percent complete. Never parse stderr text — it drifts between versions.
6. Cancel: spawn ffmpeg in its own process group; on cancel send SIGINT to the group first, wait up to 3 s, then SIGKILL. SIGINT lets an in-progress MP4 finish writing its moov header instead of leaving a corrupt file.
7. With `--debug`, keep every produced file and write `commands.txt`.

## Acceptance
- [ ] `doctor` shows ffmpeg source and version; on a non-dev machine source = Bundled
- [ ] Tests on fixtures: probe values match ffprobe run by hand; sampled frame count ≈ duration×fps ±1; wav is 16 kHz mono; frame_at(1.0) matches the golden `frame.png` visually (assert image dims and mean pixel within 2%)
- [ ] `export_clip` output plays and starts within 1 frame of requested start
- [ ] Every command logged verbatim at INFO
- [ ] Progress callback fires during a 30 s export; cancelling an export leaves either a playable file or no file, never a corrupt one

## Do not
- Use ffmpeg bindings. Use PATH ffmpeg silently.

## Human review
Open one thumbnail next to the clip at that timecode.
