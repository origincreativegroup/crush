# Task 0 — Feasibility spike (run on the Mac, two days max)

Goal: prove the three risky pieces link and accelerate on Apple Silicon before any product code exists.

## Steps
1. `pip install open_clip_torch onnx onnxruntime` in `reference/.venv`; run `reference/export_clip_onnx.py` (Task 3/6 draft) or export ViT-B/32 by hand with opset 17, fixed shape [1,3,224,224].
2. Download `ggml-base.bin` from the whisper.cpp model repo into `models/`.
3. Download a static LGPL arm64 ffmpeg + ffprobe into `sidecars/`. Record source URL and sha256.
4. Fill in `Cargo.toml` deps with current versions from crates.io. Build with `cargo run --release`.
5. For ort: enable CoreML EP and confirm it is active — check the session's provider list or ort's verbose log. A run that quietly uses CPU is a FAIL for this spike.
6. For whisper-rs: build with `metal` feature; confirm Metal init in the log.
7. Print ms for: CLIP image encode ×10, whisper 10 s, ffmpeg spawn.

## Deliverable
`docs/versions.md` with: macOS version, chip, RAM, exact crate versions, build flags/env vars needed, ms per step, and any op-support warnings from CoreML.

## Go / no-go
- CoreML active and < 50 ms/frame: go.
- CoreML falls back to CPU: report; decide CPU-only (still fine for v1) vs. investigating ort's CoreML build.
- Anything fails to link: stop, report, do not start Task 1.
