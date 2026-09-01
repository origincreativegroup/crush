# TASK-034 — implementation plan (catalogue unification: span text in search, Review, confirmation)

Status: **planned, not started**. Parent acceptance: `.tasks/backlog/TASK-034.md`. Branch
`task/34-span-evidence`, after the 021/022 merge; pairs with TASK-037 (037 is step 1 and is
assumed to land first and take schema v12 — this plan takes **v13**; if the order flips, swap
the numbers, one migration each, never shared). All file:line references verified against
`task/21-render-export` head `823c867` (schema v11).

## What exists today (verified)

- **FTS is transcripts-only.** One FTS5 table, `transcripts_fts` (`migrations/0001_init.sql:74`,
  external content on `transcripts`), synced inside the store (`crates/store/src/lib.rs:4512`,
  `:4543`, `:4570`). Search uses it only as an overlap boost: `transcript_shot_hits`
  (`lib.rs:4653`) joins transcript hits to overlapping shots, and
  `SearchEngine::search_assets_in_context` (`crates/search/src/lib.rs:410-`) feeds those into
  the vector ranker (`lib.rs:422-426`). No span text is indexed anywhere.
- **Span text lives on `manual_spans`** (v11, `migrations/0011_reel_studio_import.sql`):
  `description`, `shot_type`, `camera_move`, `subjects`, `action`, `tags` (plus
  `quality`/`standout`/`usable`/safety flags/`used_in`/`notes`). `editorial_annotations` and
  `aesthetic_assessments` still CHECK `media_kind IN ('photo','shot')`
  (`migrations/0002_dam_feedback.sql:39`, `:62`) — span evidence deliberately lives on the span
  row, not annotations.
- **Review filtering excludes spans by construction.** `browse_assets`
  (`crates/store/src/lib.rs:3872-3957`) is a photo/shot UNION; the `MediaKind::Span` arm bails
  with "imported spans are listed through manual_spans, not the library" (`lib.rs:3955-3957`).
  The Review filter UI is `crates/app/ui/library.js` (`review-filters`, `filterArgs` at `:115`,
  applied filters shared to the compare dialog at `:452`); the pairwise compare pool is
  `crates/app/ui/review.js` (`library_browse` at `:104`).
- **Feedback schema.** `feedback_events.media_kind` CHECK `('photo','shot')`
  (`migrations/0002_dam_feedback.sql:83`), `compared_media_kind` `:90`; immutability is
  schema-level from v5 (`migrations/0005_feedback_hardening.sql`): `feedback_events_no_update`
  (absolute) and `feedback_events_no_delete` (aborts only while the referenced photo/shot row
  still exists, so media-cleanup cascades pass). Its header warns that widening the CHECKs
  requires a table rebuild (four triggers + two indexes reference the table).
- **Reference sets.** `reference_set_items.media_kind` CHECK `('photo','shot')`
  (`migrations/0007_reference_sets.sql:32`), with target-existence and cleanup triggers.
  Confirmation flow exists end-to-end: store `reference_set_confirm/disable/delete`
  (`crates/store/src/lib.rs:1537-1620` — disable/delete are the *reversible withdrawal* path
  and invalidate the trained profile in the same transaction), commands
  `reference_set_create/add_item/confirm/disable/delete`
  (`crates/app/src-tauri/src/lib.rs:1249-1365`), Preferences UI `crates/app/ui/style.js`
  (`:141-173`). The importer never writes feedback or reference sets; it lists finished
  projects as `reference_set_candidates` (`crates/pipeline/src/reel_studio_import.rs:575-578`)
  and the import dialog renders them (`crates/app/ui/import.js:102`).
- **The style trainer needs vectors.** `load_sample` (`crates/search/src/style/trainer.rs:261-278`)
  returns `None` without a `shot_vectors`/`photo_vectors` row and an `aesthetic_assessments`
  row. Spans have neither (both tables CHECK photo/shot), so **span-keyed evidence is inert for
  the current learner** no matter which schema option is picked. This fact drives the schema
  decision below and the honest UI copy.

## Schema v13 decision (the "pick one, record why" item)

**Decision: span confirmation enters as span-keyed `reference_set_items`; `feedback_events`
stays photo/shot.** Recorded in the `0013` migration header, roughly:

> Span evidence becomes preference evidence only through the explicit confirmation flow, and
> confirmation must be reversible (TASK-034 acceptance). feedback_events is append-only
> (0005) and therefore cannot be the vehicle. Confirmed imported evidence is a named
> previous-work reference set whose items are the spans themselves; reference_set_items admits
> media_kind 'span' so the evidence keeps its true identity and provenance instead of being
> mapped onto whatever shots happen to overlap the interval today (a mapping that both
> fabricates evidence location and silently evaporates when a resplit rebuilds shots — the
> cleanup triggers delete shot-keyed rows). feedback_events stays photo/shot: direct span
> signals (pick/rate on a span) are deferred until span interval analysis exists
> (TASK-034 open question), because the trainer can only consume media with vectors.

Why not the alternative (admit `'span'` in `feedback_events`): it costs the warned-about table
rebuild, produces rows the current trainer silently skips, and offers no reversibility — three
costs for no user-visible benefit today. Why not mapping onto the overlapping video interval:
dishonest evidence location, and fragile against resplit (see the trigger analysis in
TASK-038 — shot-keyed dependent rows are deleted when shots are rebuilt).

Migration `0013_span_reference_evidence.sql`:
- Rebuild `reference_set_items` with `media_kind CHECK ('photo','shot','span')` (drop/recreate
  its two triggers and index around the rebuild, exactly the pattern 0011 used for
  `plan_items`); recreate `reference_set_item_target_insert` with a `manual_spans` arm and add
  `span_reference_cleanup AFTER DELETE ON manual_spans` (mirrors `photo_reference_cleanup` /
  `shot_reference_cleanup`, `0007`).
- Create `manual_spans_fts` (FTS5, external content on `manual_spans` rowid) over
  description/subjects/action/tags/shot_type/camera_move, with sync handled in the store API
  (next section) — FTS tables are not STRICT-rebuild hazards, but the sync points must be
  transactional with the span writes.
- No change to `feedback_events`, `editorial_annotations`, or `aesthetic_assessments`.

## Steps

### 1. Migration + store APIs

- Migration 0013 (above); bump `CURRENT_SCHEMA_VERSION` to 13 (`crates/store/src/lib.rs:18`).
- `manual_span_upsert` (`lib.rs:7874`) and `manual_span_delete` (`lib.rs:8012`): keep the FTS
  row in sync inside the same transaction (insert/update/delete `manual_spans_fts`), the same
  discipline as transcript sync (`lib.rs:4512-4570`).
- New store read API `span_text_hits(owner, fts_query)` → span + video context (span id,
  video_id, video path, start/end, text columns, provenance fields), bm25-ordered, plus
  `spans_for_review(owner, &AssetFilter)`-shaped access for the browse branch (step 3).
- `reference_set_add_item` (`lib.rs` near `:1303` command) admits spans end-to-end
  (`MediaKind::Span` already round-trips through `media_kind_from_str`, `lib.rs:6025`).

Tests: v11→v13 round trip; span reference item target-existence trigger; span reference
cleanup on span delete; FTS sync on upsert/update/delete; owner isolation.

### 2. Search: span text results

- `SearchEngine::search_assets_in_context` (`crates/search/src/lib.rs:410-`): after the
  shot/photo passes, run the FTS query against `span_text_hits` and append span results as a
  distinct result kind on `AssetSearchResult` (span id, source video path, interval, matched
  text snippet, provenance `source`/`external_id`/`imported_at`). Spans have no vectors, so
  they are text-match results ranked by bm25 — never mixed into the cosine score, and the
  breakdown must say so (`ScoreBreakdown` gains an honest text-match-only marker).
- No fabricated thumbnails: spans have none; the result carries the source video path +
  interval and the UI shows a provenance pill ("Imported · Reel Studio") with an honest
  "no thumbnail yet" state. Preview plays the source video at the span interval (the asset
  protocol already exposes video paths).
- CLI `crushctl search` table gains span rows; the app `search` command
  (`crates/app/src-tauri/src/lib.rs:805`) passes them through; `crates/app/ui/search.js`
  renders the new kind.

Tests: store FTS hit test; search test with a span whose description matches and whose video
has no matching transcript (span surfaces, shot does not); provenance fields present;
`npm run test:ui` search scenario extended.

### 3. Review filtering: spans in the library browse

- `browse_assets` (`lib.rs:3872`): replace the `MediaKind::Span` bail (`lib.rs:3955-3957`) with
  a third UNION branch over `manual_spans JOIN videos` (and `LEFT JOIN` nothing — span evidence
  columns live on the span row). Filter mapping: `quality_min`/`usable`/`faces_visible`/
  `blur_required`/`standout` from span columns; `feedback` filter via span reference-set
  membership (not feedback events — per the schema decision); `search` (path substring) over
  the video path; `collection_id`/`stack_id`/`context_key` cannot apply to spans (those tables
  CHECK photo/shot) — the branch binds the same parameters with always-false clauses for the
  three, exactly the alignment trick the existing comment at `lib.rs:3877-3880` describes.
  `kind: "span"` becomes a valid `LibraryFilterArgs.kind`.
- UI (`crates/app/ui/library.js`, `review.js`, `index.html`): span cards in the grid
  (provenance pill, interval, source video name, honest no-thumbnail state), the kind filter
  gains "Spans", and the existing progressive-disclosure filters work against span columns.
  The pairwise compare dialog (`review.js`) **excludes spans** — `prefer` needs compared-media
  semantics and vectors; state that in the UI copy and the honest limits.
- Detail drawer for a span: show the catalogue evidence (description/subjects/action/tags/
  quality/standout/used_in) read-only with provenance, plus the source video preview.

Tests: store browse tests (span branch filters, kind=span, always-false clause alignment);
harness scenario (filter to spans, provenance pill visible, no thumbnail fabricated).

### 4. Preferences: the explicit "confirm imported evidence" flow

- New store-backed read: "imported evidence awaiting decision" = spans with
  `quality`/`standout`/`used_in` evidence or `import_id` lineage, plus imported finished
  projects (the same population the importer's `reference_set_candidates` reports — derive it
  from spans/recipes, do not trust the stale dry-run report).
- Preferences (`crates/app/ui/style.js`) gains an "Imported evidence" section: per-item and
  bulk **Confirm** / **Skip**. Confirm creates (or adds into) a named previous-work reference
  set — e.g. "Reel Studio · <project/theme>" — with `media_kind='span'` items, status
  `unconfirmed` until the existing `reference_set_confirm` runs (two-step, matching the
  existing set lifecycle), provenance retained via the set name/description plus each item's
  span id. Skip records the decision in local UI state only (no store write — re-import must
  not resurrect a skipped item as "new", see step 5).
- **Honest inertness copy:** confirmed span sets do not train the current model (no vectors —
  verified `trainer.rs:261-278`). The panel must say exactly that: "Confirmed. Spans influence
  recommendations once span analysis lands; today they are catalogued evidence." Never imply
  learned.
- Reversibility is the existing machinery: disable/delete the set
  (`reference_set_disable`/`_delete`, `lib.rs:1543-1558` + `:1693`), which already invalidates
  the affected profile transactionally. No new withdrawal code.
- Commands: `imported_evidence_list`, `imported_evidence_confirm` (creates set + adds items),
  reusing `reference_set_confirm` for the second click.

Tests: store test — confirmed span set is read by `reference_set_confirmed_items`, trainer
tolerates span items without crashing and without counting them as samples; disable removes
it; harness scenario for the two-step confirm + honest copy.

### 5. Re-import after confirmation

- Verified safety: the importer never writes feedback/reference sets, span ids are stable by
  `(owner, source, external_id)` (`manual_span_upsert`, `lib.rs:7874-7893`), and re-import
  never deletes spans. So confirmed evidence keyed by span id survives re-apply by
  construction. Tests to prove it: apply → confirm → re-apply (same catalogue) → set/items
  unchanged, no duplicates; apply → confirm → re-apply with a *changed* catalogue (segment
  evidence updated, one segment removed) → confirmed items for surviving spans unchanged,
  the removed segment's span row still exists (importer never deletes) so its confirmed item
  stays — record that as intended behavior in the docs.

### 6. Gates

Full gates: fmt, warnings-denied clippy, workspace tests, `npm run test:ui`. No golden edits;
no render-path changes at all in this task (byte-stability trivially preserved — no executor
code touched).

## Open question for John (flagged, with recommended default)

**Run Crush's aesthetic analysis over span intervals?** (compute cost per clip — a CLIP +
aesthetic pass per span; would also put spans into strong-shot candidates and give them the
vectors the trainer needs). **Recommended default: catalogue text first** — ship 034 as
scoped above (search + Review + confirmation), and file span interval analysis as its own
follow-up task so 034 stays shippable and the compute-cost decision is made deliberately,
not smuggled in. If John says yes mid-task, the honest-limit copy in step 4 changes but the
schema decision does not.

## Honest limits

- Span search results are text-match only (bm25); they do not join the cosine ranking until
  span vectors exist (the open question).
- Confirmed span evidence does not train the current preference model — disclosed in the UI,
  never implied otherwise.
- Spans cannot enter collections/stacks yet (those tables CHECK photo/shot) — the browse
  branch treats those filters as always-false for spans; widening them is a follow-up.
- The pairwise compare dialog excludes spans (needs compared-media semantics + vectors).
- `feedback_events` stays photo/shot in v13; direct span pick/rate signals are deferred with
  the reason recorded in the migration header.
