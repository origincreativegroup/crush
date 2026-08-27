# Reference kit (Python) — the answer key

Not part of the product. Defines what "correct" means for the Rust stages.

- `export_clip_onnx.py` — exports CLIP ViT-B/32 image + text encoders to ONNX (fixed shapes, opset 17), plus the BPE vocab/merges. Outputs sha256s.
- `reference_embed.py` — for an image or text: prints the preprocessed tensor (`--dump-tensor`) and the 512-d L2-normalized embedding. Writes `fixtures/golden/`.
- `reference_scenes.py` — PySceneDetect ContentDetector on fixture clips → expected cut timecodes.
- `reference_transcribe.py` — faster-whisper on fixture clips → expected segments.

Setup: `python3 -m venv .venv && . .venv/bin/activate && pip install -r requirements.txt`
`make golden` regenerates everything. Regenerate only deliberately, with a commit message saying why.
