# TASK-038 — Rename survival and shot-identity hardening (key feature)

Implements the contract in `.tasks/backlog/TASK-038.md`. Branch `task/38-shot-identity` off
`main` @ `cb7f9c6`. Scope decision honored: John confirmed (2026-08-31) that Crush *survives*
renames for this task — performing renames stays out of scope — and that shot-level
collection/feedback membership is the model (video rows keep their honest disabled batch-bar
state; no change made).

- [x] (c) Re-index evidence fix: `replace_shots` is now a diffing replace. Shots whose
      stable id returns are updated in place and never deleted, so the 0002/0007/0009+0011
      AFTER DELETE cleanup triggers fire only when a shot genuinely vanished. Identical
      resplit = zero evidence change. Validation and the "cannot replace with an empty shot
      list" guard kept; no migration. Store test
      `resplit_preserves_shot_evidence_and_cleans_only_vanished_shots` seeds every
      shot-keyed table (feedback as media + as compared media, annotations, assessments,
      vectors, plan items, reference items) and proves: identical resplit keeps all of it; a
      changed cut list removes only the vanished id's evidence (including its plan item —
      never silently rewritten; the render-time clamp still guards items that no longer
      fit). Pipeline test `renamed_and_moved_files_keep_identity_and_evidence` proves the
      same on real fixtures.
- [x] (a) First-class relink flow. `Store::relink_video`/`relink_photo` re-check the
      verified SHA-256 inside the transaction that writes the path; refuse on mismatch
      ("the file at <path> is not the same media Crush indexed — nothing was changed"),
      refuse a missing stored hash (fail closed), update only the path, never duplicate a
      row, never touch the original. `Pipeline::relink` resolves by asset id or stale path,
      canonicalizes like ingest, refuses a missing new path. `crushctl relink` (CLI shape
      test pinned). App: `list_videos.sourceMissing` → Library "Locate moved file…" action →
      `relink_asset` command; harness scenario `relocate-moved-file` drives the refusal and
      the verified relink. `docs/release.md` § Relink and `docs/smoke.md`'s exercise-relink
      item now describe the real flow.
- [x] (b) Move/rename detection on ingest. Both ingest paths classify same-content-at-a-new-
      path (old copy gone + same parent = renamed, old copy gone + new parent = moved, old
      copy still on disk = duplicate copy) and report it in `IngestSummary`
      (`moved`/`renamed`/`duplicated` counts + per-file `relinked` list), in the
      `crushctl ingest` report, and in the app's ingest progress (structured counts on the
      background task + an honest Library message, announced once per job).
      `indexed`/`skipped` meanings unchanged; same-path-new-content stays its own
      honestly-labeled outcome; no duplicate rows in any case.
- [x] (d) Rename survival proof: pipeline test `renamed_and_moved_files_keep_identity_and_
      evidence` — rename in place → `renamed` reported; move across directories → `moved`;
      explicit relink without re-adding a folder → verified and re-pointed; tampered file →
      SHA-256 refusal with the row untouched. Through every step: same video id, same shot
      ids, feedback/annotation/assessment/vector/plan item survive, one row only, original
      bytes unmodified (final hash re-check).
- [x] Full gates: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --
      -D warnings`, `cargo test --workspace`, `npm run test:ui` (27 scenarios: 25 prior +
      `relocate-moved-file` + `ingest-relinked`). No golden edits.

## Identity audit (recorded so it is not re-litigated)

- `photo_by_path` (store lib.rs:939) and `video_by_path` (store lib.rs:4142) are ingest
  dedup and target resolution only: same-path-new-content logging during ingest
  (pipeline lib.rs:696), CLI/pipeline `<target>` resolution (id-first, path fallback), the
  Reel Studio importer's original-file matching (with the optional `--match-by-hash` verified
  fallback), and the new relink target resolution (id-first). None create or key evidence
  by path. No fix needed.
- Identity is content-addressed end to end: media rows unique on (owner_id, sha256);
  shot ids `stable_shot_id` = blake3(video sha256, index, start_ms) (stage-split scene.rs);
  photo ids `photo-<sha32>`. Every evidence table (0002 annotations/assessments/feedback,
  0007 reference items, 0009 plan items, 0008 collection/stack items, 0011 spans, vectors)
  keys on (owner_id, media_kind, media_id) or id FKs; SQL has no path-keyed or
  timestamp-keyed joins. Render outputs (0010) store derivative paths, not evidence links.
- The one real path-adjacent defect was the `replace_shots` delete-and-refill above; fixed.

## Honest limits

- The relink verification hashes the file immediately before the store's in-transaction
  re-check; a file swapped between the two steps within the same instant is outside any
  local-tooling threat model, but it is not cryptographically serialized.
- Vanished-shot vectors cascade away with the shot row and are re-embedded only if the shot
  id returns; `reembed --all` remains the rebuild-everything path.
- Batch relink of a whole remounted drive stays out of scope (folder re-add covers it);
  a drive-level UI can ride on the relink command later.
- `crushctl doctor --deep` integrity checks remain vector/thumbnail/proxy-focused; missing
  sources surface on the Library row as a plain-language "Source missing" pill (failed
  tone) that clears when a verified relink lands, via the row's "Locate moved file…"
  action, and at stage-failure time — not yet as an integrity problem kind (would require
  a host-FS scan inside the store's integrity pass). An earlier version of this record
  said only "surface via `sourceMissing`", which over-claimed what the UI showed: before
  the review fix the row rendered a bare green Done with no missing indicator, and
  `sourceMissing` drove nothing but the toolbar button's enabled state.

## Review fixes applied (2026-09-01)

A review returned MERGE with three should-fixes and four nits; all applied on this branch.

- **Stale vector on a changed survivor (correctness).** `stable_shot_id` covers index and
  start but not `end_s`/`rep_frame_s`, so a re-cut that moves a cut boundary changed the
  preceding shot's end while its id survived; `replace_shots` updated the row in place but
  the pre-recut vector stayed and kept serving. `replace_shots` now deletes the
  survivor's `shot_vectors` row **in the same transaction** when its `end_s` or
  `rep_frame_s` changed (same discipline as the vanished-shot cascade), so the next embed
  pass re-embeds it (`embed_missing_shots` skips shots that already have a vector).
  Asserted in `resplit_preserves_shot_evidence_and_cleans_only_vanished_shots` step 3.
  *Honest limit:* the aesthetic assessment describes the pre-recut rep frame too, but
  `video_assessments_current` keys only on assessment presence + model version — NOT on
  vectors — so the analyze-staleness machinery does **not** refresh it on its own;
  a re-analyze (or model bump) remains the manual path. Only the vector is automatically
  invalidated.
- **Prefer feedback with a vanished compared side.** Deleted whole, not partially
  rewritten: the schema's no-dangling-reference discipline removes a preference event when
  either side's shot disappears (feedback as media and as compared media both fire the
  cleanup triggers). Asserted by the same store test — `fb-ev-1` (prefer shot-ev-0 over
  shot-ev-1) is removed when shot-ev-1 vanishes, leaving `fb-ev-0` and `fb-ev-2`'s fates
  per their own sides.
- **Stale "moved or renamed" message re-announcing forever (UI honesty).** Finished
  background tasks are never pruned and `job_status` re-emits ingest-progress on every
  call, so the Library re-fired the summary on every event. The message now announces
  once per job id: announced ids are tracked in a bounded FIFO set in app state
  (`state.announcedIngestJobs`, evicting the oldest at 200); later events for an
  already-announced job are silent. Asserted in the `ingest-relinked` harness scenario,
  which re-fires the same finished job after the 5 s message window and requires the
  message to stay hidden.
- **`sourceMissing` never rendered on the row (UI honesty).** It only gated the toolbar
  button. Library rows now render a "Source missing" pill in the failed tone next to the
  status pill whenever `sourceMissing` is true (same treatment as a Failed row, plain
  language), and the pill clears when a verified relink lands. Asserted in the
  `relocate-moved-file` harness scenario.
- **Duplicate-copy honesty.** When ingest finds same content at a new path and the OLD
  file still exists on disk, the outcome is now reported as `duplicate copy`
  (`RelinkKind::DuplicateCopy`, counted additively in `IngestSummary.duplicated`) instead
  of claiming a move; `moved`/`renamed` now count only files whose old copy is really
  gone. Path update semantics unchanged. Asserted in
  `renamed_and_moved_files_keep_identity_and_evidence` step 2b.
- **Nits:** CLI relink line label corrected from `(relindex …)` to `(id …)`; the CLI and
  app summary text gained `duplicated={}`; `complete_ingest_background` no longer mirrors
  `complete_background` — it writes the structured counts onto the stored task, then
  delegates the status/detail/error transitions to `complete_background`; the relink store
  test now covers the empty-**stored**-hash branch (a verified caller hash still refuses a
  row with no recorded sha256, fail closed).
