# Task 021 render review packet

This is the first persistent review packet for the Task 021 human hard stop. Automated checks
passed before these files were retained, but that does not constitute visual, color, timing, or
audio approval.

Local packet: `target/render-golden-review/task-021-0724f08/`

## Review order

1. Open `photo-source.png`, then compare `photo-derivative.jpg`, `.png`, and `.tiff`.
   The frozen recipe crops the left half and rotates it 90 degrees, so every derivative must be
   4x12, upright relative to the recipe, visually equivalent across formats, and free of private
   metadata. Each adjacent manifest must report the same source hash and `source_unchanged=true`.
2. Play `clip-earth.mp4`. It is the exact 0.25–1.25 second source interval with a normalized
   10% inset crop, no grade, and mute. Check the opening/closing frames, orientation, absence of
   audio, and clean playback. The source fixture is `fixtures/clips/earth-timelapse-silent.mp4`.
3. Play `reel-speech-two-cuts.mp4`. It contains 0.25–1.25 seconds followed by 3.25–4.25 seconds
   from `fixtures/clips/synthetic-speech.mp4`, in that order, with one hard cut and source audio.
   Check the cut, total two-second rhythm, audio continuity at the cut, orientation, and playback.
4. Open each `.crush-manifest.json`. Confirm the frozen recipe/project, source IDs and hashes,
   actual commands/backend, output checksum, dimensions, duration tolerance, and source recheck
   are understandable and sufficient to audit the finished file.

## Approval questions

- Do the photo derivatives match the declared crop/rotation and one another closely enough?
- Does the clip start and end where expected, without a flash, repeated boundary frame, or tail?
- Is the reel order correct and is the cut visually and audibly clean?
- Are the manifest labels understandable to a working photographer/editor?
- Is any result misleading enough that Task 021 should remain blocked before the broader matrix?

Record approval or rejection with the packet commit and specific artifact names. Do not update
goldens or mark Task 021 accepted solely because checksums and automated tolerances passed.

## Deliberate limits of this packet

This packet does not approve advanced mixed-media reels, photo holds, fixed social canvases, music,
captions, watermarks, covers, speed/motion, crop keyframes, transitions, HDR/tone mapping, wide
gamut, mixed item volume, silence insertion, or the full Reel Studio grade vocabulary. Those are
explicit capability errors today. The broader photo color/orientation and video transition/audio
matrix described in `.tasks/backlog/TASK-021-impl-plan.md` remains before final Task 021 acceptance.
