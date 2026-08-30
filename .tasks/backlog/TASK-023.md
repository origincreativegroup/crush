# TASK-023: DAM release packaging and clean-machine acceptance

Depends: Tasks 016–022. Extends the retained Task 013 release plan to the complete product and
supersedes only the obsolete implementation/routing PRs #13 and #15.

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
      reviews assets with the progressive filter workflow, records sample creative preferences,
      creates and previews a reel in Projects, and renders verified photo and video outputs. The
      tester must not need internal vocabulary such as plan, recipe, context key, or style profile.
- [ ] Reel playback has visible controls for play/pause, scrubbing, in/out preview, looping, and
      sequence navigation; the browser harness and clean-machine smoke both exercise them.
- [ ] Primary navigation calls the creation workflow Projects and the learning/evidence workflow
      Preferences; no user-facing “Style” label suggests filters or color grading.
- [ ] Install, privacy, format-support, data-location, backup, relink, and uninstall docs are current.
- [ ] Bundle assembly consumes a versioned platform manifest for sidecars, models, hashes,
      licenses and capability smokes; shell actions and data paths use platform services so the
      accepted Mac workflow can be reused by the additive Windows Tasks 028–031.
