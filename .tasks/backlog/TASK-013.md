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

## Implementation record (2026-08-28, Claude)

- `.github/workflows/release.yml`: runs on `macos-latest` for tags `v*` (and manual dispatch).
  Installs pinned `tauri-cli 2.11.4`, downloads and SHA-256-verifies the `sidecars-v1` ffmpeg/ffprobe
  into the `externalBin` layout, then `cargo tauri build --bundles dmg --ci`.
- Signing is gated on all five secrets being present. tauri-bundler rejects `APPLE_CERTIFICATE`
  when `tauri.macos.conf.json` pins `signingIdentity: "-"`, so the signed path passes
  `--config '{"bundle":{"macOS":{"signingIdentity":null}}}'` and lets the identity come from the
  imported certificate; notarization runs inside `tauri build` from `APPLE_ID`/`APPLE_PASSWORD`/
  `APPLE_TEAM_ID`. Without the secrets the DMG is ad-hoc signed and the job summary says
  **UNSIGNED** in bold with the right-click → Open instruction.
- Artifacts: `Crush-<version>-aarch64.dmg` plus `.sha256`, uploaded as a workflow artifact and to
  the (draft) GitHub release for the tag.
- `docs/install.md`: dmg → Applications, signed vs unsigned first launch, quarantine fix, model
  download, Doctor expectations, data locations, uninstall, and the clean-machine checklist.
- `docs/smoke.md`: ten pre-written default queries, run template with hit/miss, wall time,
  latency, laptop-usable Y/N, and an annoyances list.

Still needs John: push a `v*` tag, add the Apple secrets, and run the fresh-account acceptance.

Local verification (Apple M4 Pro, 2026-08-28): `CI=true cargo tauri build --bundles dmg --ci`
produced `Crush_0.0.1_aarch64.dmg` (35.5 MB). Mounted, it contains `Crush.app` (ad-hoc signed,
`codesign --verify --deep --strict` passes, `LSMinimumSystemVersion` 10.15) with both ffmpeg and
ffprobe sidecars plus the Applications drop link. Without `CI=true` the bundler's Finder
AppleScript step timed out (-1712) on this Mac, which is why the workflow sets `CI: "true"` explicitly.
