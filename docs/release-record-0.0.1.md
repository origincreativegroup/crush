# Release record — Crush 0.0.1 (first Mac release candidate)

This record presents the first Mac release-candidate artifact and the evidence produced with it.
It is NOT an acceptance record and grants nothing: the release gate is John's clean-machine run
of the checklist in `docs/smoke.md`. Nothing in this file is release-approval language.

## Build

- Built 2026-09-02 on the M4 Pro dev host (macOS 26.5.2, arm64) by `scripts/package-macos.sh`
  (pinned `cargo-tauri` 2.11.4; sidecars hash-verified against the CI digests per the packaging
  contract).
- **Commit built: `7d9b3b5`** (`7d9b3b595e635cbc6ba7b9575a079459732bc754`), branch `main`, tree
  clean — the current `origin/main` head. (The working checkout was found on a leftover branch
  whose content was already merged; local `main` was fast-forwarded to `origin/main` and the
  build made from that head. Nothing had landed after `7d9b3b5`.)
- Wall clock ~56 s end-to-end (release compile 26.4 s — largely incremental; a from-scratch or
  CI build will take much longer).
- `CRUSH_BUILD_COMMIT=7d9b3b5` stamped into both `crush-app` and `crushctl`; build log honestly
  records `Signing with identity "-"` and `skipping app notarization, no APPLE_ID…`.

## Artifact

| What | Value |
|---|---|
| DMG | `/Users/origin/GitHub/crush/target/release/bundle/dmg/Crush_0.0.1_aarch64.dmg` |
| DMG SHA-256 | `2c2cde1728b38b5ed2b740ab64c80e063b85d1c9f89d0ebe8f8a5f5be8a797a6` (recorded alongside as `Crush_0.0.1_aarch64.dmg.sha256`; recomputed digest matches) |
| DMG size | 37,691,008 bytes (~36 MB); `hdiutil verify`: checksum VALID |
| App bundle | `/Users/origin/GitHub/crush/target/release/bundle/macos/Crush.app` — bundle version 0.0.1 |
| App SHA-256 (full-bundle digest) | `2a74515c568c3577fcd28404125b9f3a67cb8454eadc46074424e3eb267bb0f1` |
| Release `crushctl` | `target/release/crushctl` (same provenance stamp) |
| Verify report | `/tmp/crush-0.0.1-release-report.txt` |

## `scripts/verify-release.sh` — key lines (verbatim, exit 0 = PASS)

```
Crush release verification 2026-09-02T05:35Z
app: /Users/origin/GitHub/crush/target/release/bundle/macos/Crush.app
bundle version: 0.0.1
bundle executable: crush-app
build commit: 7d9b3b5
app sha256: 2a74515c568c3577fcd28404125b9f3a67cb8454eadc46074424e3eb267bb0f1
codesign verify: PASS
  CodeDirectory v=20500 size=102286 flags=0x10002(adhoc,runtime) hashes=3190+3 location=embedded
  Signature=adhoc
sidecars: PASS (ffmpeg, ffprobe)
```

Doctor (deep) summary from the same run: bundled `ffmpeg` 9.0.1 / `ffprobe` resolved, five pinned
models present (`models: green`), `embed provider requested=coreml active=coreml
providers=cpu,coreml`, `whisper configured=auto selected=small backend=metal`, `integrity clean`,
24.0 GiB RAM.

Dev-host caveats on that doctor output: it ran against this machine's existing
`dev.crush.app` data directory (applying schema migrations v12→v13 with a pre-migration
snapshot) and resolved ffmpeg from the checkout's `target/release/` — both impossible on a clean
machine, which is precisely what the clean-machine run will exercise.

## Signature state — AD-HOC, unmistakably labeled (TASK-023)

**This build is ad-hoc signed. It is NOT notarized.** No Developer ID secrets exist on this
host. Per TASK-023 the packaging step wrote `BUILD-ADHOC.txt` next to the DMG so an ad-hoc
artifact can never be mistaken for a distributable release; a notarized release would have no
such marker and would carry a notarization ticket. Install per `docs/release.md`
(right-click → Open). This DMG must never be published as a release.

`BUILD-ADHOC.txt` contents (verbatim):

```
This DMG is an AD-HOC signed build. It is NOT notarized and is NOT
suitable for distribution beyond local development. Install it via
right-click -> Open (see docs/release.md).

built: 2026-09-02T05:35Z
build commit: 7d9b3b5
dmg: Crush_0.0.1_aarch64.dmg
dmg sha256: 2c2cde1728b38b5ed2b740ab64c80e063b85d1c9f89d0ebe8f8a5f5be8a797a6

Signature evidence (codesign -dv --verbose=2 on the .app):
Identifier=dev.crush.app
CodeDirectory v=20500 size=102286 flags=0x10002(adhoc,runtime) hashes=3190+3 location=embedded
Signature=adhoc
TeamIdentifier=not set
```

## What is in the release

Crush 0.0.1 is a local-first Mac library for a photographer/editor: import photos and videos (or
a finished Reel Studio project, whose imported clips stay adjustable — you can move their
in/out boundaries after import) into a catalogue that never touches your originals; search it by
what is in the frame and what is said on camera, including text carried in from imported clips;
review and compare candidates with auto-advance through the review pool and batch actions over a
multi-selection; set preferences that describe creative-taste evidence — with an explicit
confirmation flow — and see a profile marked "Learned" only when the recorded conditional
verdict's conditions are met; and build photo derivatives, clips, and multi-clip reels with
progress, cancellation, checksums, and audit manifests. Moved or renamed files survive: Crush
reports them honestly and repairs the catalogue by verified relink (SHA-256-checked, never a
duplicate, never touching the original) or re-adding the folder. The whole UI has been through
the recorded design-system pass.

## Human gates recorded before this build

- **Task 021 render-golden review** — `docs/task-021-render-review.md`: initial review
  REJECTED the ordered-reel artifact (missing boundary frames, head dead zone, cut hold, tail
  audio over a frozen frame); after the TASK-036 fix and machine verification of the re-rendered
  packet, the re-review APPROVED the reel item and the earlier photo/clip/manifest verdicts stand
  (decision: Task 021 render-golden review PASSES, 2026-08-31).
- **Task 018 style proof** — `docs/style-proof-review.md`: APPROVE "learned" wording —
  conditional (2026-08-31): the label only for profiles that pass the held-out training gate,
  with plain-language scope beside it, and any stronger claim deferred until project-level
  grouping and real feedback volume exist. Implemented in the TASK-039 follow-up pass.
- Both verdicts were recorded by OpenCode as acting reviewer under John's delegated direction
  (2026-08-30/31, recorded in each document). **John may reverse or amend either verdict**; the
  authority delegation does not transfer his clean-machine acceptance.

## Honest limits

- **Ad-hoc signed, not notarized.** No Developer ID or notarization was possible on this host;
  the artifact is labeled by `BUILD-ADHOC.txt` (above) and must be installed via right-click →
  Open. It is not distributable.
- **Clean-machine acceptance has NOT been executed.** Every checklist item in the clean-machine
  route of `docs/smoke.md` remains open. That human run — John's gate — is what completes
  acceptance; this record presents the artifact and evidence only.
- **Harness vs WKWebView.** Automated UI coverage ran in the browser/DOM harness
  (`scripts/ui-harness.mjs` with the mock bridge), not in the real WKWebView the app ships in.
  Real-app visual/interaction behavior has at best been eyeballed on this dev machine; judging
  it on a clean machine is part of the outstanding acceptance run.
- **Clip-encode byte nondeterminism (pre-existing).** VideoToolbox clip output bytes are not
  deterministic run-to-run; the goldens therefore assert declared output *properties* — frame
  counts, durations, dimensions, audio presence — with tolerances, and the clip/reel properties
  are test-enforced rather than byte-compared (photo derivatives do re-render byte-identical).
  Rendered clips are verified by manifest properties, not by expecting identical bytes.

## Status

Presented for the clean-machine acceptance run. No acceptance, approval, or publish decision is
made or implied by this record.
