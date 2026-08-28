# TASK-008: Embed with ort (CoreML + CPU)
Agent: Codex on the Mac. Branch: task/08-embed. Depends: 007 approved, 000 versions.

## Goal
Image and text embeddings from the ONNX models, on CoreML with CPU fallback, matching the answer key.

## Instructions
1. Add `ort` at the version proven in docs/versions.md, features `["coreml"]`. Pin exactly.
2. `Embedder::new(models_dir, provider_pref, threads)`: build session with CoreML EP first, CPU appended. Query the session for the active provider list and store it; `doctor` prints `active=coreml|cpu`. If config says coreml but active is cpu, log WARN with ort's reason.
3. `embed_image(&Tensor) -> [f32;512]`, `embed_text(&str) -> [f32;512]`. Model output is already L2-normalized (exported that way) — still re-normalize defensively and assert norm ≈ 1.
4. Tokenizer: port CLIP's BPE tokenizer (lowercase, ftfy-free basic cleaning, regex split, byte-level BPE with the vocab file, SOT/EOT tokens, pad to 77, truncate). Golden test asserts exact `token_ids` for all text goldens. If porting is slow, the `tokenizers` crate can load CLIP's tokenizer.json — but the vocab must then be exported in that format in Task 6; choose one and record it.
5. Stage: for each shot lacking a vector, load thumb → preprocess → embed → `put_vector`. Batch of 1 in v1.
6. `crushctl debug vector <shot_id>` prints norm, first 8 values, active provider.

## Acceptance
- [x] Image golden: cos(rust, ref) > 0.999 on CPU provider, > 0.99 on CoreML — both run in the test (skip CoreML with a clear message if not on macOS)
- [x] Text golden: token ids exact; cos > 0.999 CPU / > 0.99 CoreML
- [x] `doctor` shows active provider and ms/frame (measure 20 frames)
- [x] CI macOS job added running the golden tests on CPU

## Do not
- Batch-optimize. Quantize. Change model.

## Human review
Run doctor; confirm CoreML active and note ms/frame in docs/versions.md.

## Implementation record

- `ort`/`ort-sys` are pinned exactly at `2.0.0-rc.13` with `coreml`.
- The Rust CLIP BPE port matches all five 77-token golden arrays exactly.
- All four image and all five text embeddings produced cosine `1.000000000` on CPU and CoreML.
- Provider identity is taken from the finalized ONNX Runtime execution profile after real inference;
  a registration-only result is not treated as runtime proof. The released models use CoreML plus
  small CPU fallback partitions, so doctor reports `active=coreml providers=cpu,coreml`.
- Clean keyed CoreML acceptance compiled both sessions in 132.23 s. A subsequent production doctor
  reused the same SHA-named cache entries, initialized in 122.70 s, and measured 5.29 ms/frame across
  20 frames. CPU measured 9.38 ms/frame.
- The pinned release assets remain untouched. A derived ONNX copy appends only the official
  `COREML_CACHE_KEY` metadata property (the pinned model SHA-256), making both CoreML partitions
  reuse one stable cache across CLI launches and test binaries.
