# TASK-002: SQLite store + migrations
Agent: Codex. Branch: task/02-store. Depends: 001. Read docs/HANDOFF.md.

## Goal
`crush-store` crate: opens `<data_dir>/library.db`, applies `migrations/*.sql` in order, exposes typed functions. Nothing outside this crate contains SQL.

## Instructions
1. Add `rusqlite = { version = "<pin latest 0.3x>", features = ["bundled"] }` to workspace deps. Bundled = SQLite compiled in; users install nothing.
2. `Store::open(path) -> Store`: apply migrations idempotently using `schema_version`. Enable WAL + foreign_keys on every connection.
3. Functions (all take `owner_id`): `upsert_video`, `video_by_sha`, `set_video_status`, `insert_shots(Vec<Shot>)` (single transaction), `shots_for_video`, `shot_by_id`, `put_vector(shot_id, &[f32])` (store f32 LE bytes), `load_all_vectors(owner_id) -> (Vec<String>, Vec<f32>)` (contiguous matrix, row-major), `insert_transcript_segments`, `segments_overlapping(video_id, start, end)`, `job_start/job_finish/job_fail/job_cancel`, `jobs(filter)`, `embedding_meta_get/set`, `delete_video_cascade`.
4. Keep FTS in sync: after inserting transcripts, `INSERT INTO transcripts_fts(rowid, text)`.
5. `Store::integrity() -> Vec<Problem>`: every shot has a vector (once embedded), every thumb_rel exists on disk, no orphan vectors. Used by `doctor --deep`.

## Acceptance
- [ ] Fresh DB: migrations apply; second open is a no-op; `schema_version` = 1
- [ ] Round-trip tests for every function above using a temp dir
- [ ] `load_all_vectors` for 1000 fake shots returns a 1000×512 matrix in < 50 ms
- [ ] Vector bytes are exactly `dim*4`; reading back equals input bit-for-bit
- [ ] FTS query for a word in an inserted segment returns that segment

## Do not
- Put SQL anywhere else. No ORM. No async.

## Human review
Schema matches blueprint §9; function names readable.
