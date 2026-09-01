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
      path (same parent = renamed, new parent = moved) and report it in `IngestSummary`
      (`moved`/`renamed` counts + per-file `relinked` list), in the `crushctl ingest` report,
      and in the app's ingest progress (structured counts on the background task + an honest
      Library message). `indexed`/`skipped` meanings unchanged; same-path-new-content stays
      its own honestly-labeled outcome; no duplicate rows in any case.
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
  sources surface in the Library via `sourceMissing` and at stage-failure time, not yet as
  an integrity problem kind (would require a host-FS scan inside the store's integrity pass).
