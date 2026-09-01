# TASK-038: Rename survival and shot-identity hardening

Agent: OpenCode. Branch: task/38-rename-survival. Depends: 021 merged (post-merge stretch;
after TASK-037 so the migration sequence stays one-per-task — this task needs **no schema
migration** as scoped, see (c)).

Source: John's named key features, 2026-08-31 — *file renaming* and *shot identity*
(`docs/HANDOFF.md` § Product direction). Shot identity is content-addressed by design:
`stable_shot_id` from video sha256 + index + start (`crates/stage-split/src/scene.rs:244`),
media rows keyed owner+sha256 so renames/moves preserve identity. **Scope question answered
2026-08-31:** Crush *survives* renames for this task; performing renames is out of scope.

## What the code actually does today (verified 2026-08-31, `task/21-render-export` head)

- **Moved files already relink — silently.** Re-ingesting a moved file updates the path on
  the existing identity row: videos at `crates/pipeline/src/lib.rs:533-541` (sha match → path
  update on the same row), photos at `lib.rs:159-171`. But `IngestSummary`
  (`lib.rs:44-53`) has no `moved`/`renamed` outcome — the user sees "skipped"/"indexed" and a
  log line at best. The documented relink route is "Add the folder again"
  (`docs/release.md` § Relink a moved drive), and the smoke checklist's "exercise relink"
  (`docs/smoke.md:43`) has no first-class flow to exercise.
- **No missing-source detection exists anywhere.** `Store::integrity`
  (`crates/store/src/lib.rs:4998`) checks vectors and thumbnails, never source paths; the app
  surfaces a missing original only as a failure when preview/render/ingest touches it.
- **Same path, new content** keeps the old record and creates a second row at the same path
  (`lib.rs:553-561`) — honest, but nothing distinguishes it from a move in the report.
- **Path-keyed lookups are ingest dedup, not identity.** `photo_by_path`
  (`crates/store/src/lib.rs:922`) and `video_by_path` (`lib.rs:4119`) are used only by ingest
  dedup, CLI target resolution, and the importer's source matching — identity is
  content-addressed (`video-<sha32>`, `photo-<sha32>`, `stable_shot_id`). No fix needed
  there; the audit conclusion must be recorded so it is not re-litigated.
- **THE REAL DEFECT — re-index destroys shot-keyed evidence.** `replace_shots`
  (`crates/store/src/lib.rs:4282-4330`) does `DELETE FROM shots WHERE owner_id=? AND
  video_id=?` then re-inserts. The AFTER DELETE cleanup triggers —
  `shot_editorial_cleanup` (`migrations/0002_dam_feedback.sql:175-181`: annotations,
  aesthetic assessments, feedback events as media *and* as compared media),
  `shot_reference_cleanup` (`migrations/0007_reference_sets.sql`), and `shot_plan_cleanup`
  (`migrations/0011_reel_studio_import.sql`) — fire during that DELETE, and the 0005
  `feedback_events_no_delete` guard permits them (the shot row is already gone when it
  checks existence). The re-insert brings back the *same deterministic ids* — but the
  feedback, annotations, assessments, plan items, and reference items are already deleted.
  So `crushctl resplit` / re-index silently wipes shot evidence even though
  `stable_shot_id` was designed to guarantee survival. The existing test
  (`crates/pipeline/tests/ingest_fixtures.rs:336-346`) proves only that *ids* survive, not
  evidence — the gap went unnoticed because spans (which reference videos, not shots) were
  the only rows tested for survival (`crates/store/tests/store_roundtrip.rs:4919-4935`).

## Acceptance

- [ ] **(a) First-class relink flow.** A missing source is detectable and repairable through
      the UI: a relink action on a photo/video (and its shots) where the user locates the
      file, Crush verifies SHA-256 against the stored hash, and updates the path on the
      existing identity row — never creating a duplicate row, never touching the original.
      Hash mismatch is refused with a clear error. `crushctl doctor --deep` (and the Library)
      report missing sources honestly. `docs/release.md` § Relink and `docs/smoke.md`'s
      "exercise relink" item point at the real flow.
- [ ] **(b) Move/rename detection on ingest.** Ingesting a file whose owner+sha256 matches an
      existing row at a *new* path reports `moved`/`renamed` (per-file and counted in the
      summary; surfaced in the Library ingest progress) while keeping today's same-row path
      update. Same-path-new-content stays a separate, honestly-labeled outcome. No duplicate
      rows are created in either case.
- [ ] **(c) Shot-identity audit + the re-index fix.** The audit is recorded in the task
      record: path-keyed lookups are ingest dedup (fine); identity is content-addressed
      (fine). The defect is fixed: `replace_shots` preserves evidence for shots whose
      `stable_shot_id` re-appears in the new list — delete only the shots that genuinely
      vanished (so cleanup triggers fire for real removals only) and upsert the rest —
      no migration needed, the fix is in the store function. Proven by tests: feedback
      events (as media and as compared media), editorial annotations, aesthetic
      assessments, plan items, and reference-set items all survive a resplit that produces
      identical ids; a resplit with a changed cut list still cleans up evidence for vanished
      shots; vectors survive re-index (re-embedded).
- [ ] **(d) Rename survival proof.** A store/pipeline test proving the full posture: rename
      a file on disk (same bytes), re-ingest → same ids (video, photo, shot), same feedback/
      vectors/plan items, updated path, `moved` reported; relink flow on a file the user
      moved without re-adding a folder → same result through the explicit flow.
- [ ] Full gates: fmt, warnings-denied clippy, workspace tests, browser harness
      (`npm run test:ui`), macOS fixture runs pasted in the PR. No golden edits.

## Out of scope (tracked elsewhere)

- **Crush performing renames** — pending John's answer to the open scope question above; any
  such feature must be opt-in, previewed, and reversible, and must never modify originals
  without an explicit per-action confirmation.
- Span↔shot interval linkage and span aesthetic analysis — catalogue-unification follow-ups
  (TASK-034's open question; TASK-037).
- Batch relink of a whole remounted drive (the folder re-add path already covers it; a
  drive-level relink UI can ride on (a) later).

## Notes for the implementer

- The (c) fix must keep `replace_shots`'s existing validation and the "cannot replace with an
  empty shot list" guard; shots that persist but whose boundaries changed are updated in
  place — stored plan items keep their previously validated boundaries, and the render-time
  clamp (`crates/pipeline/src/render.rs:1353-1376`) still refuses honestly at render time if
  an item no longer fits its shot. State that in the docs.
- The relink command must verify the hash *before* writing the path, in one transaction, and
  must refuse when the stored row's sha256 is missing (should not happen; fail closed).
- Keep the ingest summary additive: `moved`/`renamed` counts must not change the meaning of
  `indexed`/`skipped` (the UI's existing progress copy keys on those).
