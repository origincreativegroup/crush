# TASK-013: Build, sign, smoke, clean-machine test
Agent: Cursor on the Mac. Branch: task/13-release. Depends: 012c approved.

## Instructions
1. `.github/workflows/release.yml` on `macos-latest`: rust stable, `cargo tauri build`, upload .dmg as release asset on tag `v*`.
2. Signing/notarization steps gated on secrets `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID` being present; otherwise produce an unsigned build and say so in the job summary. John adds secrets himself.
3. `docs/install.md`: open dmg, drag to Applications, first run (unsigned: right-click → Open).
4. `docs/smoke.md` template: 10 pre-written queries, hit/miss, wall time, laptop usable Y/N.

## Acceptance
- [ ] Tagged build produces a dmg
- [ ] On a **fresh macOS user account** with no dev tools: install, launch, doctor green, index a fixture folder, search works
- [ ] Smoke ≥ 8/10 recorded
