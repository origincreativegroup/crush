# TASK-007: Embed preprocessing — golden first  ⛔ HARD STOP AFTER
Agent: Codex on the Mac. Branch: task/07-preprocess. Depends: 003, 006.

## Goal
Rust produces the exact same `[1,3,224,224]` f32 tensor as `reference_embed.py`. This is the highest-risk task in the project. No ONNX session in this task.

## Exact steps to match
1. Decode JPEG/PNG → RGB8 (`image` crate). Note: JPEG decoders can differ by ±1 LSB from Pillow. Test on the PNG golden frame first; if JPEG differs, that is expected and documented, not a bug.
2. Resize so the shorter side = 224, **bicubic** (`image::imageops::FilterType::CatmullRom` is the closest to Pillow's BICUBIC; verify — if max diff > 1e-3, try `Lanczos3` and record which matched).
3. Center crop 224×224: `left = (w-224)/2`, `top = (h-224)/2` (integer division, same as reference).
4. `x = pixel/255.0`, then `(x - mean[c]) / std[c]` with CLIP constants.
5. Layout CHW, batch 1, f32.

## Instructions
- `preprocess(img: &DynamicImage) -> Tensor` in `crush-stage-embed::preprocess`, no ort dependency.
- Test `preprocess_golden`: for each `fixtures/golden/*.image.json`, load `frame.png`, compare all 150528 values; assert `max_abs_diff < 1e-3`; on failure print the first 10 mismatches with (c,y,x) coordinates — that pinpoints channel-order vs resize vs normalize bugs instantly.
- `crushctl debug frame <png>` prints tensor shape, min/max/mean per channel, and first 8 values, plus the same from the golden if given.

## Acceptance
- [x] `cargo test -p crush-stage-embed preprocess_golden` passes at 1e-3 on all golden frames
- [x] `docs/preprocess.md` records the resize filter that matched and any JPEG decoder delta observed

## Do not
- Start Task 8. Touch ort. Loosen the tolerance.

## Human review
**John runs the test himself on his Mac and posts the output before Task 8 is dispatched.**

## Implementation note

The committed lossless frames are `.frame.ppm`, as documented by the reference kit, rather than the
task draft's `frame.png`; PNG and JPEG decoding have separate coverage. Catmull–Rom and Lanczos3 did
not meet the fixed tolerance. The direct Pillow-compatible BICUBIC implementation produces
`max_abs_diff=0` for all four 150,528-value goldens without adding ort.
