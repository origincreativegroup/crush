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
- [ ] Render one photo derivative, one clip, and one mixed reel. Verify visible progress,
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
