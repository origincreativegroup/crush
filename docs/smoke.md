# Smoke and clean-machine acceptance log

Write the 10 queries **before** indexing. Score after. Use one complete section per run and attach
the `.dmg.sha256`, Doctor output, render manifests, and any screenshots to the release record.

## Run template — <date>, <footage set>, <hours>, build <sha>

| # | Query written beforehand | Expected shot (describe) | Hit in top 5? |
|---|---|---|---|
| 1 | | | |
| 2 | | | |
| 3 | | | |
| 4 | | | |
| 5 | | | |
| 6 | | | |
| 7 | | | |
| 8 | | | |
| 9 | | | |
| 10 | | | |

Score: __/10 (target 8). Wall time: __. Laptop usable during index: Y/N. Annoyances noticed:
-

## Clean-machine route (required for Task 023)

Environment: fresh macOS user account / VM: __. No Xcode, Homebrew, Rust, Node, FFmpeg, or source
checkout present: Y/N. Artifact label: signed-notarized / ad-hoc. DMG SHA-256 verified: Y/N.

- [ ] Install from DMG and launch through the path documented for its signing mode.
- [ ] Doctor reports bundled FFmpeg/FFprobe, verified models, current schema, and a usable runtime.
- [ ] Index representative JPEG/PNG/TIFF, supported RAW/HEIF, SDR/HDR video, and mixed audio.
- [ ] Review assets using the common filters; open More filters, remove active-filter summaries,
      compare photo/video, batch-rate, and record one explicit creative preference.
- [ ] Open Preferences and confirm its copy describes creative-taste evidence, not filters or
      color grading; no unapproved profile is labeled learned.
- [ ] Create a Project without needing internal terms; add photo/video selects, reorder them,
      adjust clip boundaries, scrub, play/pause, loop, and navigate the sequence.
- [ ] Render one photo derivative, one clip, and one multi-clip reel. Verify visible progress,
      cancellation/retry, output checksum, manifest, dimensions/duration/audio/color/orientation,
      and playback in a system app.
- [ ] Repeat a render to the occupied destination and confirm Crush refuses to overwrite it.
- [ ] Quit during a render, relaunch, and confirm recovery never marks a partial output verified.
- [ ] Move one fixture temporarily, confirm the missing source is honest, then exercise relink.
- [ ] Confirm originals hash identically before and after the full run.
- [ ] Follow backup/restore and uninstall documentation; verify originals and chosen outputs remain.

Result: Pass / Needs fixes / Blocked. Reviewer: __. Date: __. Release may not be published as
accepted until the Task 021 render-golden review and this human clean-machine record are complete.

## Pre-clean-machine tooling run — 2026-08-30, dev M4 Pro host, build 838d557 (not an acceptance run)

Environment: developer machine, NOT a fresh account/VM; Xcode/Rust/Node/source checkout present,
so the clean-machine route could not be honestly executed. Artifact: `/Applications/Crush.app`,
bundle version 0.0.1, **ad-hoc signature** (`flags=0x10002 adhoc,runtime`) — not notarized; no DMG
exists to verify a `.dmg.sha256` against.

- `scripts/verify-release.sh`: PASS — app sha256 `a2b18191f6135c57274f81d43a6b59bda1c38779f3bb86ac2275f196c70dad8b`,
  codesign deep-strict verify PASS (ad-hoc), bundled sidecars present, models green, database
  integrity clean, doctor `active=coreml providers=cpu,coreml`, whisper Metal, 24 GiB RAM.
  Note: on this host doctor resolved ffmpeg from the source checkout (`target/debug/ffmpeg`) —
  impossible on a clean machine, which is itself proof this was not a clean-machine run.

Clean-machine route: **NOT EXECUTED** — every checklist item above remains open. Result: **Blocked**
(not runnable here; also downstream-blocked by the Task 021 render-golden review, which rejected
the ordered-reel artifact on 2026-08-30 — see `docs/task-021-render-review.md`). Reviewer: OpenCode, acting for the human hard stop at John's direction. Date: 2026-08-30. No release claim is made or implied.

## Pre-merge packaging trial — 2026-08-31, dev M4 Pro host, commit 4765ca0 on `task/21-render-export` (TOOLING TRIAL — not acceptance, not a release claim)

**This is a tooling trial from the pre-merge branch head. It is NOT clean-machine acceptance
and NOT a release claim.** Its only purpose is to prove the DMG packaging pipeline works
end-to-end before merge, so the final post-merge build is low-risk. Every clean-machine checklist
item above remains open, and the Task 021 render-golden review gate is unchanged.

Environment: developer machine (Xcode/Rust/Node/source checkout present) — same host limitation
as the 2026-08-30 tooling run. Pinned `cargo-tauri` 2.11.4. Sidecars verified against the pinned
CI digests before building: `ffmpeg` `73a21147…fae29d0d`, `ffprobe` `da8681f3…b3d6380d` (match
`.github/workflows/ci.yml`).

Commands used (exact):

```sh
CI=true cargo tauri build --bundles app,dmg
# config bundle targets list only "app", so the DMG target is passed explicitly;
# CI=true per TASK-023 for headless DMG creation. Ad-hoc signing comes from
# tauri.macos.conf.json "signingIdentity": "-" (no Developer ID secrets on this host).
CRUSH_APP="$PWD/target/release/bundle/macos/Crush.app" \
  RELEASE_REPORT=/tmp/crush-packaging-trial-report.txt scripts/verify-release.sh
cd target/release/bundle/dmg && shasum -a 256 Crush_0.0.1_aarch64.dmg \
  > Crush_0.0.1_aarch64.dmg.sha256
```

Artifacts:

- DMG: `target/release/bundle/dmg/Crush_0.0.1_aarch64.dmg` — 37,512,760 bytes (~36 MB)
- DMG SHA-256: `327e75ac0de87691919efa790097489fe442685b5e22e65495a2925803ce0b45`
  (recorded alongside as `Crush_0.0.1_aarch64.dmg.sha256`)
- App bundle: `target/release/bundle/macos/Crush.app`, bundle version 0.0.1
- App SHA-256 (verify-release.sh full-bundle digest): `385a8db1af1bfd590c5051045969c10b3a132b36322ca20150c5c2dff912ae9b`
- The `.app` inside the mounted DMG was confirmed file-for-file and hash-for-hash identical
  to the verified bundle (mounted read-only via `hdiutil`; per-file sha256 lists diff clean).
- Wall clock: ~39 s end-to-end, but the release profile was largely incremental (compile step
  19.3 s); a clean-machine or CI build from scratch will take much longer.

`scripts/verify-release.sh` verdict: **PASS** — run against the freshly built `.app` via
`CRUSH_APP` (the script verifies an .app bundle, not a DMG; `/Applications/Crush.app` was not
touched). Summary: codesign deep-strict verify PASS (ad-hoc), 2 sidecars present in
`Contents/MacOS`, models green, database integrity clean, doctor `active=coreml
providers=cpu,coreml`, whisper Metal, 24 GiB RAM. Full report: `/tmp/crush-packaging-trial-report.txt`.

Ad-hoc signature evidence (`codesign -dv --verbose=2` on the built bundle):

```
Identifier=dev.crush.app
CodeDirectory v=20500 size=101614 flags=0x10002(adhoc,runtime) hashes=3169+3 location=embedded
Signature=adhoc
TeamIdentifier=not set
```

Build log also states `Signing with identity "-"` and `skipping app notarization, no APPLE_ID…`.
**Ad-hoc labeling gap (honest caveat):** the DMG filename `Crush_0.0.1_aarch64.dmg` and bundle
metadata do NOT themselves say "ad-hoc" — TASK-023's "labels an ad-hoc build unmistakably"
requirement is not yet implemented in the build tooling (no release workflow exists yet). For
this trial the ad-hoc status is evidenced only by the codesign output above and the recorded
checksums. The final post-merge build should add the label (e.g., rename or a sidecar marker).

Other honest caveats for the final post-merge build:

- `verify-release.sh` printed `build commit: unknown`: it probes
  `Contents/MacOS/crush --version`, but the bundled binary is named `crush-app`. Script/bundle
  naming mismatch to reconcile before the release build.
- Doctor resolved ffmpeg from the source checkout (`target/debug/ffmpeg`), impossible on a
  clean machine — further proof this was a dev-host tooling trial, not acceptance.
- Doctor ran against this machine's existing `dev.crush.app` data directory, not a fresh one.
- Notarization, Developer ID signing, and the tagged release workflow remain unexercised
  (secrets only exist in CI per the blueprint).

Result: packaging pipeline **works end-to-end from the branch head** (app + DMG + checksum +
verify script). Clean-machine route: **STILL NOT EXECUTED**. No release claim is made or implied.
Reviewer: OpenCode (Lane C tooling trial). Date: 2026-08-31.

## Tooling completion — 2026-08-31, dev M4 Pro host, commit e071484 (TOOLING RUN — not acceptance, not a release claim)

Closes the three gaps the packaging trial found, so the final post-merge DMG build is one
command (`scripts/package-macos.sh`). **This is a tooling run on a developer machine; every
clean-machine checklist item above remains open, and the Task 021 render-golden review gate is
unchanged.**

What changed:

- **Build provenance (gap: `build commit: unknown`):** `CRUSH_BUILD_COMMIT` is stamped at build
  time into `crush-core` (build.rs + `BUILD_COMMIT` const) and surfaced by
  `crush-app --build-info` and `crushctl --version`. Unset builds honestly report
  `unknown-local`, never a fake commit. `scripts/verify-release.sh` now reads the commit from
  the Info.plist-resolved bundle executable (`crush-app`, not the old wrong probe) and fails
  visibly on any artifact that cannot self-report a stamped commit.
- **Ad-hoc labeling (gap: unlabeled ad-hoc DMG):** `scripts/package-macos.sh` writes an
  unmissable `BUILD-ADHOC.txt` marker next to the DMG (ad-hoc codesign evidence + build date +
  build commit + DMG checksum) and removes any stale marker on signed builds. Documented in
  `docs/release.md` ("Telling an ad-hoc build from a notarized one before installing").
- **One command:** `scripts/package-macos.sh` = provenance stamp → `CI=true cargo tauri build
  --bundles app,dmg` → DMG `.sha256` → ad-hoc label → release `crushctl` build.

New verify-release.sh output lines against the fresh `.app` (full report:
`/tmp/crush-tooling-completion-report.txt`):

```
bundle executable: crush-app
build commit: e071484
codesign verify: PASS
  CodeDirectory v=20500 size=101646 flags=0x10002(adhoc,runtime) hashes=3170+3 location=embedded
  Signature=adhoc
```

Artifacts: `Crush_0.0.1_aarch64.dmg` (sha256
`7c338bcbeacb7f2be69b014c83ef52c2da845befa439c1241c101171fcd7f8ef`, recorded alongside as
`.sha256`), `BUILD-ADHOC.txt` marker, app sha256
`8e4e73974850130adf3edcf483e90155e7e12668dec250459aa7026bc4967974`. Same dev-host caveats as
the trial apply (doctor resolved ffmpeg from the source checkout; existing data directory;
notarization/Developer ID/tagged workflow unexercised — no release workflow exists in CI yet,
and a future one must export `CRUSH_BUILD_COMMIT` the same way the packaging script does).

Result: tooling gaps closed; verify script PASS with real provenance. Clean-machine route:
**STILL NOT EXECUTED**. No release claim is made or implied. Reviewer: OpenCode (Lane C
tooling). Date: 2026-08-31.
