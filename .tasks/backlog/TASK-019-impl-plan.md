# TASK-019 Implementation Plan: Mixed-media review and DAM organization

Task: `.tasks/backlog/TASK-019.md` (acceptance sketch — do not edit).
Branches: `task/19a-dam-org` (schema + store APIs + commands) and `task/19b-review-ui` (UI),
per §10. HANDOFF's "one task per PR" rule is honored by treating 019a/019b as separate reviewable
units against the same task, exactly as 018a/018b did (`.tasks/done/TASK-018-impl-plan.md` §9).

Sources of truth: `docs/dam-feedback-blueprint.md` ("Feedback signals" lines 44–61, "Previous work
as style evidence" lines 62–76, "Data and privacy rules" lines 123–130, roadmap step 4 lines
145–148), `docs/HANDOFF.md`, `.tasks/backlog/TASK-019.md`, `.tasks/done/TASK-018-impl-plan.md`.

---

## 0. Verified current state and version reservation

- **Schema version: v7 is current. Next free migration is `0008_collections.sql` (v8).**
  Verified: `crates/store/src/lib.rs:18` (`const CURRENT_SCHEMA_VERSION: i64 = 7`),
  `MIGRATIONS` ends at `(7, 0007_reference_sets.sql)` (`lib.rs:19-27`), migration runner
  `apply_migrations` at `crates/store/src/lib.rs:2650`, and `migrations/` holds `0001`–`0007`
  only. Nothing in flight claims 0008 (TASKS.md: 018b UI in flight on `task/18b-style-ui` touches
  no migration; 018a already took v7). This plan claims **0008**.
- The Tauri checkout on this branch does **not** yet contain 018b's `reference_set_*` /
  `style_*` commands: the registered command list at `crates/app/src-tauri/src/lib.rs:1035-1051`
  is still `doctor … export_clip, open_in_finder`, and the doctor test at `lib.rs:1087` still
  pins `schema=6` (stale in this tree; 018b rebases it to 7). **019a is based on `main` after
  018b lands; if 018b is still in flight, 019a's store layer is independent and 019b rebases
  onto 018b's UI patterns** (§10). The doctor test's schema assertion moves to `schema=8` in
  019a.
- Store primitives that exist: `Photo`/`Video`/`Shot` records
  (`crates/store/src/lib.rs:40-52, 63-80, 304-315`), `editorial_annotations` with safety and
  treatment columns (`migrations/0002_dam_feedback.sql:37-56`, typed API
  `store/src/lib.rs:697-774`), append-only `feedback_events` (`0002:80-99`,
  `append_feedback` `store/src/lib.rs:929-973`), reference sets with the reserved
  `source_collection_id` column (`0007_reference_sets.sql:20`, struct field
  `store/src/lib.rs:287-288` documented as *"Reserved for TASK-019 collection designation"*),
  and the confirmed-only trainer read path `reference_set_confirmed_items`
  (`store/src/lib.rs:1349-1372`, consumed by `crates/search/src/style/trainer.rs:198`).
- What is **missing** (TASK-019's actual scope): no collections, no version stacks, no saved
  searches, no pairwise UI, no bulk review, no unified browse, no safety enforcement points in
  browse/detail/export paths.
- Migration runner is strictly sequential (`apply_migrations` `store/src/lib.rs:2650`);
  `ensure_owner_matches` at `lib.rs:3220` is the owner-scoping idiom every new API must use.

## 1. Scope decisions (read before implementing)

- **No new crates.** SQL stays exclusively in `crates/store`. Browsing/filter logic is a store
  API, not a search-crate change, so the ranking composition and search goldens are untouched.
- **`feedback_events` stays append-only** (0002:80-96; append-only tests at
  `crates/store/tests/store_roundtrip.rs:1483`). Every review *signal* flows through
  `append_feedback`; every piece of *current organizational state* lives in new 0008 tables or
  the existing editable `editorial_annotations` row. Nothing in 0008 mutates or deletes a
  feedback row.
- **Machine paths can never write review state.** Pipeline stages call only
  `upsert_photo`/`upsert_video`/`insert_shots`/`put_vector`/`upsert_aesthetic_assessment`/
  job APIs; the trainer (`search/src/style/trainer.rs`) reads only confirmed reference items and
  feedback events. The 0008 APIs are only reachable from app commands and `crushctl`, never from
  `crates/pipeline`, `crates/stage-*`, or the trainer. §8 makes this enforceable, not aspirational.
- **Deferred out of 019 (recorded for the board, needs John's nod):** duplicate groups and
  missing-file relink (TASK-019.md acceptance row 2). `aesthetic_assessments.duplicate_confidence`
  and `repetition_risk` (0004) already surface the *evidence*; the group/relink workflows are
  derivative-file problems that land naturally with TASK-021's render/export provenance tables.
  019 ships the two 019.md rows that are unblocked today (review surfaces; collections/saved
  searches/stacks) and files duplicate-groups + relink as TASK-019c follow-ups.

## 2. Schema — `crates/store/migrations/0008_collections.sql`

All tables STRICT and owner-scoped, mirroring 0002/0007 conventions (composite
`FOREIGN KEY (x, owner_id)` targets, target-existence triggers, cleanup triggers).

```sql
-- Collections: owner-scoped named groupings. Organizational only; they carry no training
-- meaning until a reference set is explicitly derived from one (see §3).
CREATE TABLE collections (
  id          TEXT PRIMARY KEY,
  owner_id    TEXT NOT NULL REFERENCES owners(id),
  name        TEXT NOT NULL,
  description TEXT NOT NULL DEFAULT '',
  created_at  TEXT NOT NULL,
  UNIQUE(owner_id, name),
  UNIQUE(id, owner_id)
) STRICT;

CREATE INDEX collections_owner ON collections(owner_id, name);

CREATE TABLE collection_items (
  owner_id      TEXT NOT NULL REFERENCES owners(id),
  collection_id TEXT NOT NULL,
  media_kind    TEXT NOT NULL CHECK (media_kind IN ('photo', 'shot')),
  media_id      TEXT NOT NULL,
  -- Optional per-item context key (e.g. 'homepage-hero'); NULL inherits the set/collection
  -- level. Feeds saved-search and reference-set context defaults.
  context_key   TEXT,
  added_at      TEXT NOT NULL,
  PRIMARY KEY(owner_id, collection_id, media_kind, media_id),
  FOREIGN KEY(collection_id, owner_id) REFERENCES collections(id, owner_id) ON DELETE CASCADE
) STRICT;

CREATE INDEX collection_items_set ON collection_items(collection_id);
-- Mirrors 0007's reference_set_item_target_insert / photo_reference_cleanup pattern.
-- editorial_annotation_target_insert-style triggers:
--   collection_item_target_insert (photos/shots existence, abort)
--   photo_collection_cleanup / shot_collection_cleanup (delete dangling items)

-- Version stacks: one original + derived/alternate versions. Purely organizational metadata;
-- no API mutates the underlying media rows (originals stay immutable).
CREATE TABLE version_stacks (
  id         TEXT PRIMARY KEY,
  owner_id   TEXT NOT NULL REFERENCES owners(id),
  name       TEXT NOT NULL,
  created_at TEXT NOT NULL,
  UNIQUE(owner_id, name),
  UNIQUE(id, owner_id)
) STRICT;

CREATE TABLE stack_items (
  owner_id   TEXT NOT NULL REFERENCES owners(id),
  stack_id   TEXT NOT NULL,
  media_kind TEXT NOT NULL CHECK (media_kind IN ('photo', 'video')),
  media_id   TEXT NOT NULL,
  role       TEXT NOT NULL CHECK (role IN ('original', 'derived')),
  added_at   TEXT NOT NULL,
  PRIMARY KEY(owner_id, stack_id, media_kind, media_id),
  FOREIGN KEY(stack_id, owner_id) REFERENCES version_stacks(id, owner_id) ON DELETE CASCADE
) STRICT;

CREATE INDEX stack_items_stack ON stack_items(stack_id);
-- Exactly one original per stack; everything else is a derived/alternate version.
CREATE UNIQUE INDEX stack_one_original
ON stack_items(owner_id, stack_id) WHERE role = 'original';

-- (same trigger pattern as 0007) stack_item target existence for photos and videos,
-- photo_stack_cleanup / video_stack_cleanup on photo/video delete.

CREATE TABLE saved_searches (
  id           TEXT PRIMARY KEY,
  owner_id     TEXT NOT NULL REFERENCES owners(id),
  name         TEXT NOT NULL,
  query        TEXT NOT NULL,
  context_key  TEXT NOT NULL DEFAULT 'default',
  filters_json TEXT NOT NULL DEFAULT '{}',   -- AssetFilter projection, validated as JSON object
  created_at   TEXT NOT NULL,
  UNIQUE(owner_id, name)
) STRICT;
```

Decisions:

- **Collections are organizational; designation is a wrap, not a mutation.** The decision
  point the task poses — fill `source_collection_id` vs. create a linked set — resolves to
  **fill `source_collection_id` on a newly created reference set, and materialize its items**:
  - The trainer's only read path, `reference_set_confirmed_items`
    (`store/src/lib.rs:1349-1372`), keeps working unchanged; learning risk stays at zero.
  - `reference_set_items` already carries `role` (`positive`/`excluded`, `0007:34`) and the
    whole-set-vs-selected semantics live in `reference_sets.scope` (`0007:15`), so a designated
    collection reuses the whole confirm/disable/delete lifecycle (`store/src/lib.rs:1242-1284`)
    instead of growing a parallel status machine on collections.
  - Items are **materialized into `reference_set_items` at confirm time** (whole_set: all
    collection items; selected: only items the user marked) so a later collection edit cannot
    silently mutate confirmed training evidence — matching the blueprint rule that confidence
    and context stay attached to each signal.
  - SQLite cannot `ALTER TABLE ... ADD CONSTRAINT`, so 0008 adds `reference_set_designation`
    insert/update triggers validating `source_collection_id` against `collections(id, owner_id)`,
    and a `collection_reference_unset` trigger that NULLs `source_collection_id` when the
    collection is deleted (the set survives — it owns its materialized items; removal is
    reproducible from remaining evidence per blueprint line 76).
- **Stacks are a table, not a `parent_id` column.** `photos`/`videos` rows are immutable
  originals with `UNIQUE(owner_id, sha256)`; a parent column would imply reparenting and
  mutability, and would need polymorphic nullable columns for shot-level grouping we do not
  want. `stack_items.role` with the partial unique index keeps "one immutable original per
  stack" enforced in SQL. Photos: edited variants ingested as separate photo rows. Video:
  exports/alternates group at the `videos` level (shots stay scene units inside one video).
  Derivative *files* (renders, proxies) are lineage facts for TASK-021, not stack rows.
- **Originals immutable:** no 0008 API writes to `photos`/`videos`/`shots` rows; stacks and
  collections are separate rows with cleanup triggers.

## 2b. Browsing — new store API (SQL stays in `crates/store/src/lib.rs`)

```rust
pub struct AssetFilter {
    pub kind: Option<MediaKind>,            // photo | shot
    pub status: Option<String>,             // photo/video status strings
    pub usable: Option<bool>,
    pub faces_visible: Option<bool>,
    pub blur_required: Option<bool>,
    pub quality_min: Option<i64>,           // editorial_annotations.quality 1..5
    pub collection_id: Option<String>,
    pub stack_id: Option<String>,
    pub context_key: Option<String>,        // matches collection_items.context_key
    pub search: Option<String>,             // FTS/file-name substring
}
pub struct LibraryAsset { /* photo OR shot fields + owner path, thumb_rel, video parent,
                             annotation summary (quality/usable/flags/tags), membership ids */ }
pub fn browse_assets(&self, owner_id: &str, filter: &AssetFilter) -> anyhow::Result<Vec<LibraryAsset>>;
pub fn library_counts(&self, owner_id: &str) -> anyhow::Result<LibraryCounts>;
```

One SQL projection over `photos LEFT JOIN editorial_annotations` UNION `shots JOIN videos` with
the same joins for collections/stacks, ordered by `captured_at`/`indexed_at`. This is the only
new read path; `search_assets_in_context` (`crates/search/src/lib.rs:389-409`) stays the ranked
surface and **is not modified** in 019a (its `context_key` parameter already exists at
`search/src/lib.rs:402-408`). Saved searches persist `(query, context_key, filters_json)` and the
UI replays them through `search` + `library_browse`; no ranking change, so
`fixtures/golden/expected_search.json` is untouched.

## 3. Reference-set designation API

In `crates/store/src/lib.rs` (extends the 018a block at `lib.rs:1180-1372`):

- `collection_create/list/get/rename/delete` — delete cascades items; triggers scrub
  `reference_sets.source_collection_id` to NULL.
- `collection_add_item` / `collection_remove_item` / `collection_items` — mirror
  `reference_set_add_item` validation (`lib.rs:1286-1311`), including the target-existence
  trigger and a `context_key` that must be empty or a valid non-blank key when present.
- `collection_designate_as_reference_set(&mut self, owner_id, collection_id, name,
  context_key, scope)` → `ReferenceSet`:
  1. Creates a `ReferenceSet` with `status = 'unconfirmed'` and
     `source_collection_id = Some(collection_id)` (the reserved column, `0007:20`),
  2. For `WholeSet`, copies every current collection item into `reference_set_items` with
     `role='positive'`; for `Selected`, the user marks items first (UI) and only marked rows
     copy. The copy is a snapshot at confirm time, so later collection membership changes
     never rewrite confirmed evidence.
  3. Then the existing `reference_set_confirm` flow applies — the user's explicit confirm is
     the only thing that makes the set contribute positive signal (blueprint line 57:
     "uncurated imported folder … none"; `reference_set_confirmed_items` already enforces it).
  Re-designating an existing collection creates a *new* set (sets are versionless snapshots;
  `UNIQUE(owner_id, name)` still applies). This honors the reserved column's documented intent
  (`store/src/lib.rs:287-288`) without touching the trainer.

## 4. Review surfaces — store APIs

- `set_safety_flags(&self, owner_id, media_kind, media_id, flags: SafetyFlags) -> EditorialAnnotation`
  — reads the current `editorial_annotations` row (or defaults), overwrites **only**
  `faces_visible`, `nametags_visible`, `blur_required`, `usable`, bumps `updated_at`, and
  upserts. This is the *only* new write path for the safety columns; it is called only from the
  app command layer (§5) in response to an explicit user action. There is deliberately no
  store API that batches flag writes with score writes, so no machine path can clear a flag.
- `bulk_review(&mut self, owner_id, ops: &[ReviewOp]) -> usize` — one immediate transaction
  (`TransactionBehavior::Immediate`, pattern of `put_style_profile` `lib.rs:1016-1024`) applying
  pick/reject/rate/flag/add-to-collection ops; each op both upserts the annotation state and
  appends the corresponding `feedback_events` row via the existing `append_feedback` invariants
  (`lib.rs:929-973`: pick=+1, reject=−1, rating 1–5, `prefer` requires the compared id).
- **Metadata editing decision:** descriptions, subjects, action, tags, notes, crop, grade stay
  *editable current state* in `editorial_annotations` (0002:37-56), while each user action also
  appends an append-only `feedback_events` row (`tag`, `edit`, `crop`, `grade`, `rating`,
  `pick`, `reject`, `prefer`) — exactly the "append-only provenance + editable current state"
  split the blueprint draws (blueprint lines 44–57, acceptance row 4). Privacy flags are the
  exception: state-only, no feedback event, no machine writer (§6).
- **Undo:** annotations are reverted by re-upserting the prior annotation snapshot (they are
  editable by design); feedback is append-only, so an undo appends a corrective event
  (e.g. `reject` after a mistaken `pick`) rather than deleting history — reversible *workflow*,
  immutable *provenance*, per the blueprint's signal table. The UI keeps a bounded in-memory
  action stack per session to drive single-step undo (⌘Z / Ctrl+Z).

## 5. Tauri commands — `crates/app/src-tauri/src/lib.rs` (019a)

All commands follow the house pattern: `CommandResult<T>`, `spawn_blocking` for store work,
`DEFAULT_OWNER_ID`, per-call `Store::open` (pattern of `record_feedback`, `lib.rs:698-779`).

- `library_browse(filter) -> Vec<LibraryAsset>` (§2b) — the unified grid query.
- `collection_create/list/rename/delete`, `collection_add_items`, `collection_remove_item`,
  `collection_items`, `collection_designate_reference_set` (§3).
- `stack_create`, `stack_add_item`, `stack_remove_item`, `stacks_for_asset` (stack membership
  lookup joins the grid and detail drawer).
- `saved_search_create/list/delete`; `run` is the existing `search` command plus
  `context_key` + client-side `filters_json` application.
- `record_feedback` (existing, `lib.rs:698-779`) gains `"prefer"` support: new `comparedId` +
  `comparedAssetType` args mapped to `FeedbackSignal::Prefer` with the compared pair —
  store-side validation already enforces the rules (`append_feedback` `lib.rs:929-953`).
  Existing pick/reject/rating value conventions (`lib.rs:734-741`) are unchanged.
- `set_annotation(assetType, id, fields)` — description/tags/crop/grade/notes/quality editing;
  upserts `editorial_annotations` (store `lib.rs:697-754`) and appends the matching feedback
  signal (`tag`/`edit`/`crop`/`grade`).
- `set_safety_flags(assetType, id, facesVisible, nametagsVisible, blurRequired, usable)` — the
  only UI path to §2's flags; shows a distinct confirm step when *clearing* a flag.
- `review_batch(ops)` — bulk pick/reject/rate/flag/add-to-collection (§4).
- Registration: extend `generate_handler!` (`lib.rs:1035-1051`). 019b rebases onto 018b's
  command block (`reference_set_*`, `style_*` on `task/18b-style-ui`) rather than duplicating it.
- Update the doctor test's pinned string `schema=6` (`lib.rs:1087`) to `schema=8` in 019a
  (018b does the same for 7 — last writer wins, trivial rebase).

## 5b. Safety enforcement points (explicit list)

| State | Written by | Read/enforced at |
|---|---|---|
| `faces_visible`, `nametags_visible`, `blur_required`, `usable` | **only** `set_safety_flags` / `review_batch` after explicit user action | grid filters, detail drawer badges, search penalties, export gate |
| machine score paths | `upsert_aesthetic_assessment`, vectors, jobs | never touch `editorial_annotations` — no 019 command or stage call path exists that writes flags from scores |

- **Search:** `search_assets_in_context` (`crates/search/src/lib.rs:402-430`) keeps flags in the
  ranking's penalty term (usable=false already penalized per 018a); 019b additionally annotates
  results with `usable`/`blur_required` badges so review is visible at ranking time. Scores never
  flip flags — the trainer only *reads* feedback and confirmed reference items
  (`trainer.rs:157-223`).
- **Detail/browse:** drawer renders flag pills; blurred thumbnails served when
  `blur_required = 1` (thumb swap at render time, not a new flag source).
- **Export:** `export_clip` (`app/src-tauri/src/lib.rs:809`) gains a pre-flight refusal when the
  source shot's annotation has `usable = 0` or `blur_required = 1` without an explicit
  `allow_unsafe_export: true` argument (full publish gating is TASK-021's render/export gate;
  this is the earliest enforcement point that exists today).
- **Machine paths can never write:** `feedback_events` stays insert-only via `append_feedback`
  (`lib.rs:929`); 0008 tables are only written by the new app commands; `retrain_style_profile`
  writes only `style_profiles`; no code path in `crates/pipeline` or `crates/stage-*` gains an
  annotation writer (enforced by the CI grep in §9).

## 6. Mixed-media browsing UI (019b)

New Library grid view (`crates/app/ui/library.js` + `index.html` section, styled in
`search.css`/`styles.css` conventions): unified photo + shot grid with filter bar
(kind, status, pick/reject/rating state, flags, collection, stack, context), saved-search
dropdown, and pagination. Grid tiles use `convertFileSrc` thumbs; shot tiles show parent video +
timecode. Conventions: DOM built with `textContent` helpers (`app.js:215-219` `cell()`,
`search.js:87-105` `showMessage()`), status-pill classes reused (`app.js:274-275`), no
`innerHTML`, no network, `invoke` only.

Compare view (`review.js` + `index.html` section): A/B two-up with keyboard shortcuts —
`←/→` focus A/B, `p` pick focused, `x` reject, `1–5` rate, `Enter` record `prefer` for the
focused side, `f` flag panel, `⌥←/⌥→` batch-advance. Pick/reject/rate writes go through
`record_feedback` (extended) and `review_batch`; the drawer's existing Pick/Reject/Rating
controls (`index.html:169-178`, `search.js:547-562`) keep working unchanged.

## 7. Test plan

Windows-safe store tests (`crates/store/tests/store_roundtrip.rs`; TempDir harness at `:23-42`,
owner-isolation pattern at `:1740`, reference-set round-trip at `:1956`, confirmed-only reads at
`:2096`):

- Migration: fresh v8 database migrates once; v7→v8 upgrade preserves photos/shots/annotations/
  feedback/reference rows (pattern of `schema_v3_upgrades…` `:160`, `schema_v4_jobs…` `:222`).
- Collections: round-trip + owner isolation; cascade on collection delete; target-existence
  triggers abort unknown media; cleanup triggers on photo/shot delete.
- Designation: `whole_set` snapshots items; `selected` copies only marked rows; confirm is
  required before `reference_set_confirmed_items` returns anything; deleting the collection
  NULLs `source_collection_id` but keeps the confirmed set and its items (evidence survives).
- Stacks: one-original invariant (partial unique index rejects a second `role='original'`),
  cascade cleanup, photo/video delete removes stack items.
- Saved searches: round-trip, name uniqueness per owner, `filters_json` must be a JSON object.
- Interplay: append-only feedback still holds with bulk ops in play (extends `:1483`), and
  `set_safety_flags` never appends a feedback event while `bulk_review` always does.
- Command layer: `review_batch` transactionality (one bad op aborts the batch);
  `export_clip` refuses `usable=false`/`blur_required=1` fixtures.

UI harness (`scripts/ui-harness.mjs` + `crates/app/tests/mock-bridge.js`; add cases to the
`invoke` switch at `mock-bridge.js:300-340` and scenarios to `ui-harness.mjs:42-170`,
clock-free determinism via `page.clock.install()` `ui-harness.mjs:197`):

- `library-grid`: mixed photo/video rows render with kind pills and filters narrow the grid.
- `compare-view`: keyboard p/x/1–5 emit the right `record_feedback` calls (`mockCalls` pattern
  `ui-harness.mjs:149-163`).
- `collections-and-flags`: create collection, add item, designate → reference-set pill shows
  `unconfirmed`; clearing a privacy flag demands the confirm step and only then calls
  `set_safety_flags`.
- `saved-search`: save from the search bar, re-run from the Library sidebar.

CI: Linux + macOS matrix unchanged; no new deps; doctor test pinned to `schema=8`; UI harness
runs headless via system Chrome (`npm run test:ui`, `CRUSH_CHROME_PATH` override) — Windows devs
run store/search tests locally, Mac gates the app build.

## 8. Acceptance mapping (`.tasks/backlog/TASK-019.md`)

Row 1 → §4/§5/§6 (picks, rejects, stars, compare, tags, notes, flags, crops, grades, undo);
row 2 → §2/§2b (collections, saved searches, stacks; duplicate groups + relink deferred per §0);
row 3 → §3 designation + context + whole-set/selected; row 4 → §2b/§4 append-only split;
row 5 → §2b `LibraryAsset` parent/derivative columns + stacks (full render lineage in 021);
row 6 → §6 keyboard-first batch review + §7 harness scenarios.

## 9. Sequencing and branches

- **019a — `task/19a-dam-org`:** migration `0008_collections.sql` (`CURRENT_SCHEMA_VERSION = 8`,
  `store/src/lib.rs:18-27`), store APIs (§2b, §3, §4), Tauri + CLI commands (§5), all Windows-safe
  tests. Mergeable alone: no behavior change without the UI, goldens untouched.
- **019b — `task/19b-review-ui`** (based on 019a **and** 018b): Library grid, compare view,
  collections/stacks/saved-search UI, flag pills, harness scenarios (§6, §7). If 018b lands
  first (expected — it is in flight per TASKS.md), 019b rebases onto its `style.js`/pill
  patterns and registered commands; if not, 019b waits for 018b's merge to avoid two PRs
  touching `index.html`'s sidebar.
- Constraints honored: `owner_id` on every new record and every API (HANDOFF rule, applied via
  `ensure_owner_matches` `store/src/lib.rs:3220`); goldens untouched (`fixtures/golden/`
  has no reason to change — no ranking change in 019); no new crates or external deps (SQL +
  rusqlite + existing serde_json only); no server; blacklist respected.

## 10. Verified current-code anchors (this checkout, 2026-08-29, HEAD 7e2c62e)

- `crates/store/src/lib.rs:18-27` — `CURRENT_SCHEMA_VERSION = 7`, MIGRATIONS 0001–0007
  (**next free version: 8**)
- `crates/store/src/lib.rs:2650` — `apply_migrations`; `:442-450` `schema_version()`
- `crates/store/src/lib.rs:697-774` — editorial annotation upsert/read (editable current state)
- `crates/store/src/lib.rs:929-984` — `append_feedback` validation + `feedback_events` listing
- `crates/store/src/lib.rs:279-291` — `ReferenceSet` with `source_collection_id` reserved
  (doc comment at `:287-288`); `0007_reference_sets.sql:20` reserves the column
- `crates/store/src/lib.rs:1180-1372` — full reference-set API incl.
  `reference_set_confirmed_items` (confirmed-only read, `:1349-1372`)
- `crates/store/src/lib.rs:3220` — `ensure_owner_matches` idiom
- `crates/search/src/lib.rs:389-409` — `search_assets` / `search_assets_in_context(context_key)`
- `crates/search/src/style/trainer.rs:42-61, 157-223` — context-scoped trainer, confirmed-items
  read at `:198`
- `crates/app/src-tauri/src/lib.rs:1035-1051` — registered commands (no 018b commands yet;
  018b in flight on `task/18b-style-ui`)
- `crates/app/src-tauri/src/lib.rs:698-779` — `record_feedback` (pick/reject/rating; pick=1.0,
  reject=−1.0; append-only comment at `:722-723`); `:809` `export_clip`; doctor schema string
  `:209-218`, pinned test `:1087`
- `crates/app/ui/app.js:274-275` — status-pill pattern; `crates/app/ui/search.js:547-562` —
  `recordFeedback`; `crates/app/ui/index.html:147-186` — detail drawer + feedback buttons
- `crates/app/tests/mock-bridge.js:300-340` — mock `invoke` switch; `scripts/ui-harness.mjs:42-170, 197`
  — scenario pattern + frozen clock
- `crates/store/tests/store_roundtrip.rs:137, 160, 1483, 1740, 1956, 2096, 2159` — migration,
  append-only, owner-isolation, reference-set, and profile test patterns
- `crates/store/migrations/0002_dam_feedback.sql:37-56, 80-96` — editorial_annotations,
  feedback_events + append-only/CHECK conventions; `0007_reference_sets.sql:9-65` — set/trigger
  conventions to mirror
- `docs/dam-feedback-blueprint.md:44-57` (signal strengths), `:123-130` (privacy rules,
  "machine scores never clear a privacy flag"), `:64-76` (reference-set designation rules)
- `docs/HANDOFF.md:19-33` — owner_id rule, goldens, blacklist, branch convention
- `TASKS.md` rows TASK-018 (018b in flight) and TASK-019 (backlog)
