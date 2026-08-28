# TASK-013: Build, sign, smoke, clean-machine test (superseded)

This video-only release target was written before the DAM pivot. PR #13 was closed rather than
merged. Reusable lessons are carried into Task 023: tagged/manual macOS builds, pinned Tauri CLI,
hash-verified FFmpeg sidecars, `CI=true` for headless DMG creation, optional Developer ID signing
and notarization, checksummed artifacts, install instructions, and clean-machine smoke records.

The new release gate must exercise RAW/still ingest, mixed-media review, and photo/video rendering.
The original checklist is preserved below as historical context.

## Instructions
1. `.github/workflows/release.yml` on `macos-latest`: rust stable, `cargo tauri build`, upload .dmg as release asset on tag `v*`.
2. Signing/notarization steps gated on secrets `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID` being present; otherwise produce an unsigned build and say so in the job summary. John adds secrets himself.
3. `docs/install.md`: open dmg, drag to Applications, first run (unsigned: right-click → Open).
4. `docs/smoke.md` template: 10 pre-written queries, hit/miss, wall time, laptop usable Y/N.

## Acceptance
- [ ] Tagged build produces a dmg
- [ ] On a **fresh macOS user account** with no dev tools: install, launch, doctor green, index a fixture folder, search works
- [ ] Smoke ≥ 8/10 recorded
