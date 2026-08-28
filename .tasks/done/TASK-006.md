# TASK-006: CLIP ONNX export + model downloader
Agent: Codex on the Mac. Branch: task/06-models. Depends: 003.

## Goal
Models published as release assets; Rust downloads, verifies, and records them.

## Instructions
1. Run `reference/export_clip_onnx.py`; confirm `onnxruntime` in Python gives the same embedding as PyTorch for one image (cos > 0.9999). Check the ONNX graph with `onnxruntime`'s CoreML provider in Python too — note any unsupported-op warnings.
2. Download whisper `ggml-base.bin` and `ggml-small.bin`. Record sha256s.
3. Create a GitHub Release `models-v1` with: clip-image.onnx, clip-text.onnx, BPE vocab, ggml-base.bin, ggml-small.bin, manifest.json (extend manifest with whisper entries). Total ≈ 700 MB; GitHub allows 2 GB/asset.
4. Rust `crush-core::models`: `ensure(models_dir, manifest_url, progress: impl Fn(Progress))`. Streams each file to `<name>.part`, verifies sha256, renames atomically. Resumes with `Range` if `.part` exists. Retries 3× with backoff. Uses `ureq` or `reqwest` (blocking) — pick one, pin it.
5. `doctor` reports each model: present/missing/sha-mismatch. On first success, write `embedding_meta` from manifest.

## Acceptance
- [x] Fresh models dir → `ensure` downloads all with progress callbacks firing
- [x] Kill mid-download → rerun resumes and completes (live release resumed at byte 234,881,024)
- [x] Corrupt one byte → sha mismatch reported, file re-downloaded
- [x] `doctor` green on models

## Do not
- Bundle models inside the .app (keeps the dmg small and lets models update independently).

## Human review
- [x] Six release assets exist at `origincreativegroup/crush` under the project organization.

## Implementation record

- Release: `https://github.com/origincreativegroup/crush/releases/tag/models-v1`
- Model bytes: 1,242,702,823 across five files; all SHA-256 values are in the tracked manifest.
- Real-image CPU parity: cosine 1.0; maximum absolute error 5.541369318962097e-07.
- Text CPU parity: cosine 1.0000001192092896; maximum absolute error 1.7881393432617188e-07.
- CoreML diagnostics: image 466/467 nodes and text 476/478 nodes supported; small CPU fallback recorded.
- Live acceptance: interrupted after 234,881,024 bytes of `clip-image.onnx`, resumed from the exact
  offset, repaired a one-byte vocabulary corruption, and confirmed the database metadata identity
  `c29c784ed9a2ee4de5e5856b6551b7fbe40e9b25673788eb4e58f538074acb3e`.
