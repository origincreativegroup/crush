# TASK-005: Scene detector
Agent: Codex. Branch: task/05-scenes. Depends: 004.

## Goal
Pure-Rust content-based cut detection matching PySceneDetect's ContentDetector closely enough to pass the golden tolerance.

## Algorithm (implement exactly)
1. Input: sampled frames from `sample_frames` (480p JPEGs at `split.sample_fps`, default 4). Load with `image` crate.
2. Convert each frame to HSV (f32, H in 0–360 scaled to 0–255, S and V 0–255).
3. For consecutive frames, compute mean absolute difference per channel over all pixels: `dH, dS, dV`. `score = (dH + dS + dV) / 3`. (This is PySceneDetect's `content_val`.)
4. A cut occurs at frame i when `score_i >= threshold` (default 27.0) AND at least `min_scene_len_s` since the last cut.
5. Shots = intervals between cuts, first starts at 0, last ends at `duration_s`. Shot k's `scene_score` = the score that opened it (0 for the first).
6. `rep_frame_s = start_s + (end_s - start_s) * rep_frame_pos`.
7. Write `scores.csv` (`t_s,score`) to the debug dir always when `--debug`, and expose `crushctl debug scenes <video>` which prints it.

## Instructions
- Function signature: `detect(frames: &[FramePath], fps: f32, cfg: &SplitConfig) -> Vec<ShotSpan>`. No I/O beyond reading frames.
- After detection, the stage writes shots to the store and calls `frame_at(rep_frame_s)` for thumbnails into `<data_dir>/thumbs/<shot_id>.jpg`.

## Acceptance
- [ ] Golden test: for each fixture, every reference cut has a detected cut within ±(2/sample_fps) s; extra cuts ≤ 1 per minute
- [ ] A clip with no cuts yields exactly one shot
- [ ] Performance: 10-minute 480p clip at 4 fps (2400 frames) detects in < 5 s on CPU
- [ ] `debug scenes` CSV matches what the store recorded

## Do not
- Add ML. Change the formula to "improve" it — match the reference first, tune later with the smoke table.

## Human review
Plot one CSV; confirm threshold 27 is sane for John's footage.
