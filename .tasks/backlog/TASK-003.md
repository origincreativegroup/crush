# TASK-003: Reference kit + fixtures
Agent: Codex for scripts (already drafted, untested); **John supplies clips**. Branch: task/03-reference. Depends: 001.

## Goal
Working answer-key kit in `reference/` and committed golden outputs in `fixtures/golden/`.

## Instructions
1. John drops 3–5 clips in `fixtures/clips/` (see fixtures/README.md). ≤ 20 MB total.
2. On the Mac, in `reference/.venv`: run `export_clip_onnx.py` (models land in `models/`, git-ignored), then `make golden`.
3. Fix whatever the drafted scripts get wrong (they were written without execution). Keep the preprocessing in `reference_embed.py` EXACTLY as specified — it is the contract Rust will match: shorter side → 224 bicubic, center crop 224, RGB, /255, CLIP mean/std, CHW.
4. Verify determinism: run `make golden` twice, `git diff` is empty.
5. Commit `fixtures/golden/*.json` and `manifest.json` copy at `fixtures/golden/manifest.json`.

## Acceptance
- [ ] `make golden` runs clean twice with identical output
- [ ] Each image golden includes `tensor` (150528 floats) and `embedding` (512 floats, L2 norm ≈ 1.0 ± 1e-5)
- [ ] Each text golden includes `token_ids` (77 ints) and `embedding`
- [ ] Scenes and transcript goldens exist per clip
- [ ] README explains how to regenerate and that regenerating needs a reason in the commit message

## Human review
John eyeballs one scenes JSON against the clip.
