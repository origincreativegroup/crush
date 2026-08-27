# Third-party components

| Component | License | Notes |
|---|---|---|
| ffmpeg / ffprobe (bundled static, arm64) | LGPL 2.1+ | **Must be an LGPL build (no --enable-gpl).** Source URL + sha256 recorded in sidecars/SOURCES.md. |
| CLIP ViT-B/32 weights (OpenAI) | MIT | Exported to ONNX by reference/export_clip_onnx.py |
| Whisper weights (OpenAI, ggml conversion) | MIT | |
| ort, whisper-rs, rusqlite, tauri, clap, tracing | MIT / Apache-2.0 | |
