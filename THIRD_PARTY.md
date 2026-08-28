# Third-party components

| Component | License | Notes |
|---|---|---|
| ffmpeg / ffprobe (bundled static, arm64) | LGPL 2.1+ | **Must be an LGPL build (no --enable-gpl).** Source URL + sha256 recorded in sidecars/SOURCES.md. |
| CLIP ViT-B/32 weights (OpenAI) | MIT | Exported to ONNX by reference/export_clip_onnx.py |
| Whisper weights (OpenAI, ggml conversion) | MIT | |
| image 0.25.10 and enabled JPEG/PNM codec dependencies | MIT / Apache-2.0 / BSD-3-Clause / Zlib / Unlicense | Rust-only sampled-frame decoding for scene detection. |
| ort, whisper-rs, rusqlite, tauri, clap, tracing | MIT / Apache-2.0 | |
