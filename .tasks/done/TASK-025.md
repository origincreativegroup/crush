# TASK-025: Store hardening — feedback immutability, owner-safe upserts, integrity coverage

Agent: Codex. Branch: task/25-store-hardening. Depends: none.

Windows-safe: all work verifiable with `cargo test -p crush-store`.

Constraints: golden files untouched, no ranking changes, one task per PR, HANDOFF.md rules apply
(owner_id on every owned record), and never edit an applied migration (0001_init.sql:2) — 0005 is
additive.

## Instructions

1. **Feedback append-only at the schema level (F1).** docs/dam-feedback-blueprint.md:45 declares the
   feedback store append-only, but only the Rust API respects it (lib.rs has INSERT at 877 and SELECT
   at 901 against feedback_events; no UPDATE/DELETE paths). Add
   `crates/store/migrations/0005_feedback_hardening.sql` with two triggers:
   - `feedback_events_no_update`: BEFORE UPDATE ON feedback_events →
     `RAISE(ABORT, 'feedback_events is append-only')`.
   - `feedback_events_no_delete`: BEFORE DELETE guard that permits only the existing cleanup paths.
     `photo_editorial_cleanup` / `shot_editorial_cleanup` (0002_dam_feedback.sql:168-180) delete
     feedback rows AFTER their media row is already gone, including through FK-cascade chains
     (`delete_video_cascade`, lib.rs:1844-1870, cascades videos→shots, which fires
     shot_editorial_cleanup). So abort only when every media row referenced by OLD still exists:
     RAISE(ABORT, 'feedback_events is append-only') only if the (media_kind, media_id, owner_id)
     target and — when present — the (compared_media_kind, compared_media_id, owner_id) target all
     still exist. Permit when any referenced target is missing (this is what lets cleanup-trigger
     deletes pass, including the mixed photo/shot-comparison case, and leaves orphan cleanup
     possible). Study 0002:168-180 before finalizing the predicate.
   Register `(5, include_str!("../migrations/0005_feedback_hardening.sql"))` in MIGRATIONS
   (lib.rs:18-23), bump CURRENT_SCHEMA_VERSION to 5 (lib.rs:17), and update the fresh-open
   assertions (store_roundtrip.rs:79, 90). Tests: direct UPDATE/DELETE of a live feedback row via an
   audit connection (`Connection::open(store.db_path())`, pattern at store_roundtrip.rs:91, 565)
   must error; deleting a photo via raw SQL (fires photo_editorial_cleanup) and deleting a video via
   `delete_video_cascade` must still succeed and remove dependent feedback rows — extend the
   cascade test (store_roundtrip.rs:424-534) with feedback rows, including one whose
   compared_media_id references the deleted media.

2. **Validate context_json (F2).** feedback_events.context_json (0002_dam_feedback.sql:92) has no
   `json_valid` CHECK and append_feedback (lib.rs:852-895) inserts it raw at 890. Enforce at the API
   like exposure_json/metadata_json: call `validate_json_object(event.context_json, "context_json")`
   (helper at lib.rs:2677-2682; existing callers at lib.rs:2604-2605, 2631) before the INSERT.
   A table rebuild to add `CHECK(json_valid(context_json))` is optional — prefer API enforcement
   (plus the F1 no-update trigger freezing existing rows) and document the choice in a migration
   comment. Tests: append_feedback rejects non-JSON and non-object context_json; the `'{}'` default
   round-trips.

3. **Owner-safe style profile upsert (F3).** put_style_profile (lib.rs:908-976) conflicts on
   `ON CONFLICT(id)` (lib.rs:948) where id is the global PK, and the DO UPDATE SET clause
   (949-958) omits owner_id — so a profile whose id already belongs to a DIFFERENT owner overwrites
   that owner's content while keeping their owner_id. ensure_owner_matches (lib.rs:913, helper at
   2542-2548) only checks the struct. style_profiles already has `UNIQUE(id, owner_id)`
   (0002_dam_feedback.sql:114), so no new index is needed: change the conflict target to
   `(id, owner_id)`; same-owner upserts still update, while a cross-owner id collision falls through
   to the `PRIMARY KEY(id)` violation and fails closed. After commit, read the row back and verify
   its stored owner_id equals the requested owner. Regression test with two owners (fixture pattern
   from instruction 6): owner B upserting an id owned by A must error and A's row must be unchanged.

4. **Extend integrity() (F4).** integrity() (lib.rs:1873-1993) already covers shots MissingVector
   (1876-1890), shots.thumb_rel path/existence (1892-1918), photo+video proxy path/existence
   (1920-1953), shot_vectors orphans (1955-1968), and shot_vectors byte length (1970-1990). Add the
   missing checks, reusing existing ProblemKind variants (lib.rs:317-325; no new variants needed):
   - photo_vectors: orphan rows (OrphanVector, mirror 1955-1968) and `length(vec) != dim * 4`
     (InvalidVectorBytes, mirror 1970-1990).
   - photos: thumb_rel unsafe path (UnsafeThumbnailPath) / missing file under data_dir/thumbs
     (MissingThumbnail), mirror 1892-1918.
   - photos with status 'embedded' or 'done' lacking a photo_vectors row (MissingVector).
   - style_profiles: `length(embedding_weights) % 4 != 0` or
     `length(embedding_weights) != embedding_dim * 4` (InvalidVectorBytes; detail names the profile).
   Tests: the integrity test (store_roundtrip.rs:887-929) currently asserts only MissingVector and
   MissingThumbnail — extend it so MissingProxy, UnsafeProxyPath, and InvalidVectorBytes outputs are
   asserted (metadata with proxy_rel, an unsafe proxy_rel, and a raw-SQL-corrupted vector blob), and
   add assertions for each new check above.

5. **Rating range stays API-enforced (F5).** The 1..5 rating rule lives only in append_feedback
   (lib.rs:868-875); feedback_events.value (0002_dam_feedback.sql:89) has no CHECK. A schema CHECK
   would require a table rebuild (indexes at 0002:98-99 and four triggers reference the table) —
   skip the rebuild and document API-only enforcement in the 0005 migration comment. Tests that
   append_feedback rejects: rating outside 1..5 (including NaN, which already fails the
   `(1.0..=5.0).contains` check), prefer without a compared asset (rule at 864-867), compared asset
   on a non-prefer signal (860-863), and a duplicate event id (plain INSERT at 877 → constraint
   error; the original row must be untouched afterward).

6. **Multi-owner isolation tests (F6).** store_roundtrip.rs uses DEFAULT_OWNER_ID everywhere; there
   is no second-owner test and no owner-creation API. Create owners through a raw audit connection
   (`Connection::open(store.db_path())` → `INSERT INTO owners ...`, pattern at store_roundtrip.rs:91
   and 565). Add a second-owner isolation test covering the main read/mutation APIs — photos
   (upsert_photo / photo_by_path), videos and shots (upsert_video / insert_shots / *_by_id), feedback
   (append_feedback / feedback_events), style profiles (put_style_profile /
   active_style_profile), and vectors (put_vector, put_photo_vector, vector_for_shot,
   vector_for_photo, load_all_vectors, load_all_photo_vectors) — proving owner A cannot read owner B
   rows (owner-scoped reads return None/empty for the other owner's ids) and cannot write them
   (mutations targeting the other owner's media fail FK checks or affect zero rows; helper structs
   hardcode DEFAULT_OWNER_ID at store_roundtrip.rs:44-73, so construct records with the explicit
   owner id).

7. **Small hygiene (F7).**
   - fail_running_jobs_as_interrupted (lib.rs:1746-1760) loops job_fail (1699-1714, via
     complete_job) + set_video_status (1133-1150) per job in separate implicit transactions. Wrap
     the whole pass in one `transaction_with_behavior(TransactionBehavior::Immediate)` — either
     inline the two UPDATE statements into the transaction or extract connection-scoped helpers
     shared with complete_job/set_video_status. Count, statuses, and error text must be unchanged;
     the interrupted-jobs test (store_roundtrip.rs:821-885) still passes.
   - put_vector (lib.rs:1360-1377) hand-rolls little-endian encoding (1362-1365); call vector_bytes
     (2692-2698) instead. (put_photo_vector at 574-596 already uses the helper — no change there.)
   - Non-finite policy: style profiles already reject non-finite weights (lib.rs:926-931), but
     vectors accept NaN (asserted at store_roundtrip.rs:466-475). Decision: reject non-finite values
     at the store for vectors too — add the check to put_vector and put_photo_vector — and document
     this as the single non-finite policy. Handle the fallout: update the NaN round-trip test
     (store_roundtrip.rs:466-475) to assert rejection while keeping the `-0.0` bit-exact assertions
     (473-474); the vector-matrix test (store_roundtrip.rs:536-595) generates values via
     `f32::from_bits(...wrapping_mul(2_654_435_761))` (line 556), which can produce NaN/Inf bit
     patterns — fix its generator to emit finite bits only while still testing exact round-trip.
   - v4→v5 upgrade test: mirror `schema_v3_upgrades_to_strong_shot_components_without_losing_jobs`
     (store_roundtrip.rs:98-152): apply migrations 0001-0004 into a fresh db, insert a photo, photo
     vector, feedback event, and style profile, then `Store::open` → assert schema_version() == 5,
     the data survived, and the new F1 guards are enforced on the upgraded database.

## Acceptance

- [ ] Migration 0005 adds feedback_events no-update/no-delete triggers; photo-cleanup and
      delete_video_cascade paths still remove dependent feedback; direct UPDATE/DELETE of live
      feedback rows is rejected; v4→v5 upgrade test preserves existing data.
- [ ] append_feedback validates context_json as a JSON object and rejects invalid payloads.
- [ ] put_style_profile conflicts on (id, owner_id) with read-back verification; a cross-owner id
      collision fails closed and leaves the existing owner's row unchanged (two-owner regression
      test).
- [ ] integrity() reports photo_vectors orphans/byte-length, photo thumb_rel issues, missing vectors
      for embedded/done photos, and invalid style-profile weights; MissingProxy, UnsafeProxyPath,
      and InvalidVectorBytes outputs are covered by tests.
- [ ] Rating 1..5 enforcement documented as API-only; tests cover out-of-range ratings,
      prefer-without-comparison, compared-asset-on-non-prefer, and duplicate event id rejection.
- [ ] Second-owner isolation tests prove owner A cannot read or write owner B rows across photos,
      videos, shots, feedback, style profiles, and vectors.
- [ ] fail_running_jobs_as_interrupted runs in one Immediate transaction; put_vector uses
      vector_bytes; put_vector/put_photo_vector reject non-finite values; affected tests updated.
- [ ] `cargo test -p crush-store` passes with all existing and new tests green.

## Record (merged as PR #21)

Implemented by the agent team 2026-08-29. 0005_feedback_hardening.sql: feedback_events no-update and
no-delete triggers (cleanup paths permitted, direct history rewriting rejected at the SQL level);
context_json validated as a JSON object by append_feedback; put_style_profile conflicts on (id, owner_id)
with post-commit owner read-back (cross-owner id collision fails closed, regression-tested);
integrity() extended to photo_vectors orphans/byte-length, photo thumb paths/existence, embedded/done
photos missing vectors, and style-profile weight blobs; multi-owner isolation test suite added;
fail_running_jobs_as_interrupted is one Immediate transaction; vectors reject non-finite values (NaN
test inverted, matrix generator masked to finite bits). Rating 1..5 documented as API-only (no table
rebuild). cargo test -p crush-store green on Linux CI.
