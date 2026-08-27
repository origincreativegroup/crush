# Task 0 — Feasibility spike

This throwaway Mac-only binary proves that CoreML CLIP inference, Metal Whisper inference, and an
LGPL static-library FFmpeg sidecar can coexist. It exits nonzero if an artifact is missing, FFmpeg
is GPL/nonfree, CoreML is unavailable, no CoreML profile events are observed, an output is invalid,
or Whisper produces no transcript.

## Prerequisites and assets

Install Rust and CMake, then create the Python preparation environment from the repository root:

```sh
python3 -m venv reference/.venv
reference/.venv/bin/python -m pip install onnx==1.17.0 numpy==2.0.2 protobuf==6.33.6
```

Download the pinned model inputs:

```sh
curl -fL -o models/clip-vision-vit-b-32.onnx \
  https://huggingface.co/Xenova/clip-vit-base-patch32/resolve/d15189d7028b43f1d3e65039190477f6af591c2a/onnx/vision_model.onnx
curl -fL -o models/ggml-base.en.bin \
  https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-base.en.bin
reference/.venv/bin/python spike/fix_clip_shape.py
```

Build FFmpeg 9.0.1 from the official source archive using the exact flags in
`sidecars/SOURCES.md`, then copy the resulting `ffmpeg` and `ffprobe` executables into
`sidecars/`. The model files and binaries are intentionally ignored by Git.

## Run

From the repository root:

```sh
cargo run --release --manifest-path spike/Cargo.toml
```

The committed fixture is already at `fixtures/spike-jfk.wav`. A successful run prints
`FFMPEG_OK`, `COREML_ACTIVE=true`, `CLIP_OK`, the Metal initialization log, `WHISPER_OK`, and
finally `SPIKE_OK`. Full results, hashes, machine details, and known risks are in
`docs/versions.md`.
