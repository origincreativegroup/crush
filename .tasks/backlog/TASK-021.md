# TASK-021: Non-destructive recipes and media rendering

Depends: Task 020.

## Acceptance

- [ ] Store versioned non-destructive recipes for photo crops/rotation/grade/output and video
      boundaries/crops/grade/transitions/audio/output.
- [ ] Render photo derivatives to documented JPEG, PNG, and TIFF presets with correct orientation,
      color conversion, metadata policy, dimensions, and quality settings.
- [ ] Render video clips and reels to documented MP4/MOV presets using the bundled FFmpeg path,
      with frame-accurate boundaries where the selected codec permits.
- [ ] Jobs are resumable and cancellable; partial outputs are removed or explicitly marked invalid.
- [ ] Every output has a manifest containing source IDs/hashes, recipe and model versions, tool
      versions, output checksum, and verification results.
- [ ] Golden tests verify dimensions, duration, frame boundary, audio, color/orientation, and that
      source files remain byte-identical.

