# TASK-006: CLIP ONNX export + model downloader
Agent: Cursor on the Mac. Branch: task/06-models. Depends: 003.

## Goal
Models published as release assets; Rust downloads, verifies, and records them.

## Instructions
1. Run `reference/export_clip_onnx.py`; confirm `onnxruntime` in Python gives the same embedding as PyTorch for one image (cos > 0.9999). Check the ONNX graph with `onnxruntime`'s CoreML provider in Python too — note any unsupported-op warnings.
2. Download whisper `ggml-base.bin` and `ggml-small.bin`. Record sha256s.
3. Create a GitHub Release `models-v1` with: clip-image.onnx, clip-text.onnx, bpe vocab, ggml-base.bin, ggml-small.bin, manifest.json (extend manifest with whisper entries). Total ≈ 700 MB; GitHub allows 2 GB/asset.
4. Rust `crush-core::models`: `ensure(models_dir, manifest_url, progress: impl Fn(Progress))`. Streams each file to `<name>.part`, verifies sha256, renames atomically. Resumes with `Range` if `.part` exists. Retries 3× with backoff. Uses `ureq` or `reqwest` (blocking) — pick one, pin it.
5. `doctor` reports each model: present/missing/sha-mismatch. On first success, write `embedding_meta` from manifest.

## Acceptance
- [ ] Fresh models dir → `ensure` downloads all with progress callbacks firing
- [ ] Kill mid-download → rerun resumes and completes
- [ ] Corrupt one byte → sha mismatch reported, file re-downloaded
- [ ] `doctor` green on models

## Do not
- Bundle models inside the .app (keeps the dmg small and lets models update independently).

## Human review
Release assets exist under the project org, not a personal account.
