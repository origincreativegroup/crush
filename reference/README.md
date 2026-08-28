# Reference kit (Python) — the answer key

This is not product code. It defines the outputs that the Rust stages must match.

- `export_clip_onnx.py` exports the pinned OpenAI CLIP ViT-B/32 QuickGELU image and text encoders
  with fixed shapes and opset 17, validates them, checks ONNX Runtime parity, copies the BPE data,
  and writes a hashed manifest.
- `reference_embed.py` implements the exact image contract: resize the shorter side to 224 with
  bicubic interpolation, center-crop to 224×224, convert to RGB, divide by 255, apply CLIP mean/std,
  then produce CHW float32. It also produces 77-token text inputs and normalized 512-D embeddings.
- `reference_scenes.py` runs PySceneDetect's content detector.
- `reference_transcribe.py` runs pinned faster-whisper `small` on CPU with deterministic settings.
- `generate_goldens.py` regenerates every committed artifact atomically.
- `verify_goldens.py` checks the complete file set, fixture hashes/durations, tensor/token dimensions,
  embedding dimensions/norms, queries, and the copied model manifest.

## Setup and regeneration

Python 3.12 is required. The lock file pins the complete environment.

```sh
make -C reference setup
make -C reference models
make -C reference golden
make -C reference check
```

`models/` and the faster-whisper cache are intentionally git-ignored. `fixtures/golden/` is committed.
The frame samples are lossless PPM files because the pinned minimal FFmpeg sidecar does not include a
PNG encoder.

Before committing a golden change, run `make -C reference golden` twice and confirm the second run is
byte-identical. Regenerate only for a deliberate contract, model, toolchain, or fixture change, and
state that reason in the commit message. Never hand-edit a golden to make a test pass.
