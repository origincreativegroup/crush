# MacBook user test — current pre-alpha

Prepared on 2026-08-29 for the Apple M4 Pro MacBook. This is a focused product test,
not Task 021 render-golden approval or Task 023 clean-machine acceptance.

## Installed build and preserved state

- App: `/Applications/Crush.app`
- Git commit: `9191ee4` on `task/21-render-export` (draft PR #37), based on merged PR #36.
- Build: release, arm64, Hardened Runtime, ad-hoc signed. The bundled FFmpeg and FFprobe
  signatures and the complete bundle pass `codesign --verify --deep --strict`.
- Not notarized and not a DMG. Those are Task 023 release gates, not defects in this local build.
- Existing database backup:
  `~/Library/Application Support/dev.crush.app/backups/pre-user-test-2026-08-29.db`
  (`SHA-256 2e0500e7d986e504afa34180a0aafa8320161ea2e577c7f51e24064129b1de55`).
- Post-migration schema-v10 checkpoint:
  `~/Library/Application Support/dev.crush.app/backups/pre-ui-smoke-0724f08.db`
  (`SHA-256 6404f06c93f04124398cf6f9c649d3be516c611517488b9ee281bcdc675d0d18`).
- Prepared catalogue: four license-safe videos and three derived license-safe photos are Done.
  They provide 19 video vectors, three photo vectors and 22 aesthetic assessments. SQLite
  integrity is clean at schema v10.
- One pre-existing offline record remains Failed:
  `/Volumes/Photos/Canon IMG Drop/101CANON/MVI_5342.MOV`. Keep it as the missing-drive case.
- Test photo sources are under `target/user-testing/media/`. They were generated from the
  checked-in golden PPM frames as JPEG, PNG and TIFF. The tracked video and PPM hashes did
  not change during preparation.

Preparation checks passed:

- The preceding installed build launched through the native accessibility tree. The current build
  is installed and code-signed; its final native smoke check is pending only because the Mac was
  locked during preparation.
- All 17 real-DOM UI harness scenarios passed, including project editing/provenance, preference copy,
  mixed-media review, failures and draft preservation.
- A 12-second video+AAC MOV was exported to
  `~/Desktop/Crush User Test Exports/synthetic-speech-shot-001.mov` and verified with the
  bundled FFprobe. Its SHA-256 is
  `7e6adedb5d333b5a556190d32a309fcdadfa6bf1891d10bfbd4c43722641e13e`.
- Repeating that export to the occupied path failed without changing the output hash. The
  source stayed at SHA-256
  `f9945a1e4298c50d5693de69e4657343f302666f1a4e6d6c7ff704e2432a065b`, and no hidden staging
  directory remained.

## Test boundary

It is safe to create collections, annotations, feedback, reference sets and projects in this
catalogue: the pre-test database is backed up. Crush must never write to an original media file.
Do not use personal or client media for this pass unless you intentionally want it catalogued.

Personalization is **experimental**. Nothing in this test is approval to call it “learned.”
The held-out preference proof still needs asset/project-disjoint evaluation and human sign-off.
Project crop/grade/pacing fields are stored intent unless the export card explicitly says it can
reproduce them. Verified photo and selected-clip exports are available; whole-project reel export
and the formal render-golden approval remain Task 021 work.

## 20–30 minute test route

Record surprises as you encounter them; first impressions are more valuable than trying to
make the app pass.

### 1. Launch and health

1. Open **Crush** from Applications.
2. Select **Run Doctor**.
3. Expect schema 10, `source=Bundled`, FFmpeg 9.0.1 and 5/5 models present.
4. Confirm the sidebar reports eight assets. The offline Canon record should remain visibly
   failed rather than disappearing or blocking the seven prepared assets.

### 2. Library and mixed media

1. Open **Library**. Check that video and PHOTO cards are visually distinguishable.
2. Find the vertical-night PNG and confirm it is portrait, not stretched or rotated.
3. Open the JPEG, PNG and TIFF photo details. Check the strong-shot explanation and metadata
   read naturally; note anything that feels machine-oriented or unexplained.
4. Open the offline Canon item. Its failure should be specific and recoverable, with no crash.

### 3. Search and explanations

Search should open on the populated **All assets** DAM browser, not a blank prompt. Switch between
All, Photos and Video; opening a detail must preserve useful thumbnails beside the inspector. Clear
any semantic query and confirm the complete local library returns immediately.

Try these one at a time:

- `a rocket launching into the sky`
- `the Earth's surface from space`
- `a colorful television test pattern`
- `bright engine flames`

The first search can be slower while the local encoder initializes; subsequent searches should
feel immediate. Open **Why this result?** on both a photo and a video. General quality and any
preference contribution must be separately named. A missing personal profile must not be presented
as personalization.

Open a video result, play it, move to the adjacent shot, copy its path/timecodes and reveal the
source. Open a photo result and confirm that video-only controls are absent.

### 4. Review and organization

1. In **Review**, compare a photo with a video shot.
2. Add a plain-language description, tags and notes to one photo; mark one prepared asset as a
   standout. Refresh/relaunch and verify those edits persist.
3. Create a collection named `MacBook user test` and add one photo and one video shot.
4. Confirm none of those document edits is described as preference training.

### 5. Experimental preference evidence

1. Open **Preferences** and inspect the empty/no-profile state. The copy should describe creative
   taste, not filters or color grading.
2. Create a reference set named `MacBook examples`, add two prepared assets and inspect the
   confirmation boundary before enabling them.
3. If you confirm/retrain, the resulting copy must remain **Experimental profile · human review
   pending**, never “Learned.”
4. Make one explicit Pick from a detail view. Ordinary tags, notes, crop intent and project edits
   must not imply that a Pick or Reject was recorded.

### 6. Projects

1. Create `MacBook test project` with context `user-test`.
2. Use a brief such as `short cinematic space sequence with a launch and a quiet ending`.
3. Refresh candidates. Check the side-by-side General and Personalized/brief columns. With no
   eligible profile, the second column must explicitly say it is General brief matching.
4. Add at least one photo and two video shots. Confirm the provenance pills are understandable.
5. Edit video In/Out and rationale; reorder the items. If you add pacing, crop intent, or an
   unsupported color-treatment object, selected-clip export must explain that it cannot reproduce
   the edit rather than silently dropping it.
6. Save a version, make another edit, restore the saved version, duplicate the project and relaunch.
   Verify saved state survives and originals remain untouched.

### 7. Safe photo and clip export

Use a new filename under `~/Desktop/Crush User Test Exports/`.

1. In a Project, preview a selected photo, choose JPEG or PNG, export it, and open the finished
   copy. Confirm the original remains unchanged and the verification manifest is shown separately.
2. Preview a selected video shot, choose MP4 or MOV and source sound or mute, then export and play
   it in QuickTime. Check beginning, end, orientation and audio.
3. Try exporting again to that exact existing path. If macOS asks to replace it, continue only
   for this generated test export. Crush must refuse to overwrite the existing file.
4. Exporting to an original, symlink or hard link must also fail. This is automated already;
   manual destructive experiments are not required.

Projects can render selected photo derivatives and selected clips. The durable backend can assemble
the narrow, frozen ordered-video reel contract, but whole-project reel export is not exposed in this
build and the UI must not imply that advanced Reel Studio intent is supported.

## Feedback to capture

For every issue, note the screen, exact action, expected result and actual result. Add a screenshot
when the issue is visual. Prioritize:

| Priority | Meaning |
|---|---|
| P0 | data loss, original changed, privacy leak or app cannot launch |
| P1 | primary workflow blocked or misleading learned/rendered claim |
| P2 | workflow works but is confusing, slow or error-prone |
| P3 | polish, wording or visual hierarchy |

Also record the three moments that felt best and the three moments that felt least trustworthy.
Those are direct inputs to the next product pass.

## Known open work (do not file as regressions)

- Preference proof remains unapproved; personalization is experimental.
- Automatic sequence/repetition judgment is not implemented.
- Project treatments are not render previews.
- Advanced mixed-media reel treatments and formal render-golden review remain Task 021.
- Reel Studio importing is Task 022.
- Notarized DMG and clean-machine acceptance are Task 023.
