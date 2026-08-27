# TASK-008: Embed with ort (CoreML + CPU)
Agent: Cursor on the Mac. Branch: task/08-ort. Depends: 007 approved, 000 versions.

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
- [ ] Image golden: cos(rust, ref) > 0.999 on CPU provider, > 0.99 on CoreML — both run in the test (skip CoreML with a clear message if not on macOS)
- [ ] Text golden: token ids exact; cos > 0.999 CPU / > 0.99 CoreML
- [ ] `doctor` shows active provider and ms/frame (measure 20 frames)
- [ ] CI macOS job added running the golden tests on CPU

## Do not
- Batch-optimize. Quantize. Change model.

## Human review
Run doctor; confirm CoreML active and note ms/frame in docs/versions.md.
