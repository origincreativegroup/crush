# TASK-015: First tagged release and clean-machine follow-through
Agent: OpenCode (Mac). Branch: task/15-release-v0.0.1. Depends: 013 merged.

## Instructions
1. After John pushes `v0.0.1`, watch the `release` workflow (`.github/workflows/release.yml`); fix anything that
   fails on the GitHub runner (sidecar download, tauri-cli install, DMG bundling). Locally it passed with `CI=true`.
2. Verify the uploaded DMG: sha256 matches, mounts, app launches, `doctor` shows `ffmpeg source=Bundled`.
3. Sit with John for the fresh-macOS-user-account run in `docs/install.md` § Clean-machine acceptance; record
   the result in `docs/smoke.md`. Fix install-doc gaps you hit.

## Acceptance
- [ ] `v0.0.1` release has `Crush-0.0.1-aarch64.dmg` + `.sha256`
- [ ] Fresh-account run recorded with doctor output pasted
