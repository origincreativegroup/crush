# TASK-023: DAM release packaging and clean-machine acceptance

Depends: Tasks 016–022. Supersedes the pre-pivot Task 013 and obsolete routing PR #15.

## Retained engineering from the closed Claude release branch

Carry forward its useful mechanics when the end-to-end DAM is ready: tagged/manual macOS builds,
pinned Tauri CLI, hash-verified FFmpeg sidecars, `CI=true` for headless DMG creation, secret-gated
Developer ID signing/notarization, checksummed artifacts, install docs, and clean-machine smoke logs.
Also retain the UI-harness-in-CI idea from PR #15, but rewrite its video-only scenarios for the DAM.

## Acceptance

- [ ] CI runs Rust checks, mixed-media fixtures, UI harness, render goldens, and bundle verification.
- [ ] Tagged build produces a checksummed, signed/notarized DMG when secrets are present and labels
      an ad-hoc build unmistakably when they are not.
- [ ] Fresh macOS account completes first run, indexes representative RAW/still/video media,
      reviews assets, learns from sample feedback, and renders verified photo and video outputs.
- [ ] Install, privacy, format-support, data-location, backup, relink, and uninstall docs are current.
