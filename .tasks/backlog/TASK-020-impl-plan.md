# TASK-020 Implementation Plan: Strong-shot recognition + user-style selects and clip/reel planning

Task: `.tasks/backlog/TASK-020.md` (acceptance sketch — do not edit).
Branches: `task/20a-planning-core` (schema + store + candidate ranking + commands) and
`task/20b-planning-ui` (UI + harness), per §11. HANDOFF's "one task per PR" rule is honored by
treating 020a/020b as separate reviewable units against the same task, exactly as
018a/018b (`.tasks/done/TASK-018-impl-plan.md` §9) and 019a/019b
(`.tasks/backlog/TASK-019-impl-plan.md` §9) did.

Sources of truth: `docs/dam-feedback-blueprint.md` roadmap step 5 "Editorial planning" (lines
149–151), "What 'good' means" (lines 27–41), "Personal style model" (lines 79–103), the signal
table (lines 48–57), and "Data and privacy rules" (lines 123–130); `docs/strong-shot-analysis.md`
(Task 017 evidence contract); `docs/HANDOFF.md`; `.tasks/backlog/TASK-020.md`;
`.tasks/backlog/TASK-021.md` (render boundary); `.tasks/done/TASK-017.md`, `.tasks/done/TASK-018a.md`.

---

## 0. Verified current state and version reservation

- **Schema version: v8 is current. Next free migration is `0009_plans.sql` (v9).**
  Verified: `crates/store/src/lib.rs:18` (`CURRENT_SCHEMA_VERSION: i64 = 8`), `MIGRATIONS`
  lists `(1, 0001_init.sql)` … `(8, 0008_collections.sql)` at `crates/store/src/lib.rs:19-28`,
  runner `apply_migrations` at `crates/store/src/lib.rs:3508`, and `migrations/` contains
  `0001`–`0008` only. A repo-wide grep finds no `plan_`/`0009` claims anywhere. **This plan
  claims 0009 (schema v9).** The doctor test pins `schema=8` (`crates/app/src-tauri/src/lib.rs:2207`,
  string at `:338`) and moves to `schema=9` in 020a.
- **Ranking surfaces that already exist (018a + 024 merged):**
  - `search_assets_in_context(store, embedder, query, top_k, context_key)` —
    `crates/search/src/lib.rs:402-409`; the plain wrapper `search_assets` at `:389-396` passes
    `None`. A per-context profile adjusts ranking on top of the default-context profile.
  - `ScoreBreakdown` (`crates/search/src/lib.rs:117-137`): `semantic`, `transcript_boost`,
    `editorial`, `general_aesthetic`, `penalties`, `personal_affinity`, `context_fit`, `total`;
    exported on every `AssetSearchResult.score_breakdown` (`:95-109`), components always present
    (`0.0`, never `null`) and summing to `total` (`compose_score`, `lib.rs:763-791`).
  - `PersonalScorer` (`crates/search/src/lib.rs:650-747`) loads the default-context and the
    requested context profile; `gated_profile` (`:749-757`) re-checks the held-out gate at
    ranking time, so an unlearned profile contributes exactly `0.0`.
  - `personal_style_score(...)` (`crates/search/src/lib.rs:796`) exposes the raw gated affinity
    for detail views.
- **Store primitives that exist:** `aesthetic_assessments` with all strong-shot components,
  `overall`, `confidence`, `explanation_json`, `model_version`
  (`AestheticAssessment` struct `crates/store/src/lib.rs:169-208`; upsert/read at `:895`, `:1023`);
  the `aesthetic_assessments_strongest` index
  (`migrations/0004_strong_shot.sql:24-26`, `(owner_id, overall DESC, confidence DESC, …)`) —
  this is the general strong-shot candidate read path; `shots` with `start_s`/`end_s`/`rep_frame_s`
  (`Shot` at `store/src/lib.rs:485-496`; `shots_for_video` `:2573`, `shot_by_id` `:2586`);
  `browse_assets`/`library_counts` (`:2103`, `:2206`) with `AssetFilter` (`:399-411`);
  collections/stacks/saved searches (`:1451+`, migrations `0008_collections.sql:10-135`);
  `append_feedback` (`:1048`), `set_safety_flags` (`:1930`, state-only),
  `bulk_review` (`:1954`); `ensure_owner_matches` (`:4212`); migration runner
  `apply_migrations` (`:3508`); `schema_version` (`:623`).
- **Trainer/eval:** `retrain_style_profile` / `retrain_style_profile_for_context`
  (`crates/search/src/style/trainer.rs:42-51`), `DEFAULT_CONTEXT_KEY = "default"` (`:29`),
  held-out gate in `style/eval.rs:62` (`evaluate`) + ranking-time re-check
  `gated_profile` (`search/src/lib.rs:750-757`).
- **Pipeline:** video shots carry `start_s`/`end_s`/`rep_frame_s` and scene boundaries come from
  the `split` stage; `Stage::Analyze` (`crates/core/src/job.rs:9`) persists per-shot
  `aesthetic_assessments` via `crates/pipeline/src/lib.rs:327` (`analyze_photos`) and `:786`
  (`analyze_video_shots`). Transcripts overlap shots via `store.segments_overlapping`
  (used at `crates/search/src/lib.rs:441-448`).
- **App/UI today:** 46 registered Tauri commands (`crates/app/src-tauri/src/lib.rs:2124-2171`);
  the `search` command (`:627-683`) calls `search_assets` **without** a context key — no UI path
  to `search_assets_in_context` exists yet; `style_profile_status_view` (`:1013-1050`) reports the
  active profile + learned gate; doctor pins `schema=8` (`:2207`, string at `:338`). UI has three
  nav views (search/library/style, `crates/app/ui/index.html:45-59, 67, 116, 152`); the 019b
  review-grid UI is in flight (its `library_browse`/`collection_*`/`review_batch` commands exist in
  the backend at `lib.rs:2124-2171`, and `crates/app/tests/mock-bridge.js:380-461` does not yet
  mock them — 020b rebases onto 019b's harness mocks when it lands, per §11).
- **What is missing (TASK-020's actual scope):** no plans tables, no Selects/planning surface,
  no general-vs-personalized side-by-side ranking UI, no clip boundary/reason/pacing/crop/grade
  recipe documents, no plan lifecycle.

## 1. Scope decisions (read before implementing)

- **No new crates, no new external dependencies, no rendering.** SQL stays exclusively in
  `crates/store`; ranking helpers stay in `crates/search`; `crates/stage-aesthetic` gains **zero**
  changes (cold-start purity, stage-aesthetic lib.rs:1-5; MODEL_VERSION `strong-shot-v1` at
  `stage-aesthetic/src/lib.rs:11`). Plans are **documents**: rows in SQLite referencing existing
  media; no media files are written, no ffmpeg call is added, `export_clip`
  (`crates/app/src-tauri/src/lib.rs:1879`) is untouched. Render consumption is TASK-021
  (`.tasks/backlog/TASK-021.md`) — 020 writes recipes/plans only.
- **Plans are current state, feedback stays append-only.** Adding/removing/reordering plan items,
  editing reasons/boundaries/pacing/crops/grades are `plans`/`plan_items` mutations (0009 tables)
  and append **nothing** to `feedback_events` (0002:80-99). The only way planning activity becomes
  training evidence is an **explicit user action** through the existing paths: the detail drawer's
  Pick/Reject/Rating controls (`index.html:222-231`, `record_feedback`
  `crates/app/src-tauri/src/lib.rs:828`) or `review_batch` (`app/src-tauri/src/lib.rs:1788` →
  `bulk_review` `store/src/lib.rs:1954`). The plan UI may *offer* that button ("Send pick to
  review evidence"), but a plan edit never silently becomes a signal. This honors TASK-020
  acceptance row 5 and the blueprint rule that workflow state stays distinguishable from opinion
  (blueprint lines 44-57).
- **Baseline and personalized remain distinguishable by construction** (acceptance row 2 and
  HANDOFF's task title): every candidate response carries **both** orderings; every plan item
  records which ordering produced it, the profile version used, and the full
  `ScoreBreakdown` at add time. Nothing ever merges the two into one number.
- **No ranking composition changes.** `search_assets_in_context` and `compose_score`
  (`search/src/lib.rs:763-791`) are consumed, not modified. `crates/pipeline` and
  `crates/stage-aesthetic` gain no dependencies and no write paths (the 019a CI grep rule,
  `.tasks/backlog/TASK-019-impl-plan.md` §1, applies verbatim). `fixtures/golden/` is untouched
  because the no-profile composition is bit-identical by construction (already test-enforced).
- **Originals immutable:** no 0009 API writes to `photos`/`videos`/`shots` rows. Clip boundaries
  are recipe values in plan items, never edits to `shots`.

## 2. Schema — `crates/store/migrations/0009_plans.sql` (v9; next free version, verified §0)

All tables STRICT and owner-scoped, mirroring the 0007/0008 conventions (composite
`FOREIGN KEY (x, owner_id)` targets, target-existence triggers, cleanup triggers — pattern of
`0007_reference_sets.sql:9-65` and `0008_collections.sql:10-135`).

```sql
-- Editorial plans: photo selects and clip/reel plans. Plans are documents (blueprint roadmap
-- step 5): they reference originals, never modify them, and never trigger renders.
CREATE TABLE plans (
  id          TEXT PRIMARY KEY,
  owner_id    TEXT NOT NULL REFERENCES owners(id),
  name        TEXT NOT NULL,
  -- 'photo_selects' (personalized photo ordering for a context) or
  -- 'reel' (ordered video clip/reel plan).
  plan_kind   TEXT NOT NULL CHECK (plan_kind IN ('photo_selects', 'reel')),
  -- Creative scope: which personal profile applies and what the user asked for.
  -- 'default' = default-context profile (may be absent -> general model only).
  context_key TEXT NOT NULL DEFAULT 'default',
  -- Optional candidate-pool scope: a collection the plan draws from (organizational only;
  -- never a training signal).
  collection_id TEXT,
  -- The user-supplied creative brief (free text). Drives semantic ranking for candidates.
  brief       TEXT NOT NULL DEFAULT '',
  -- Lifecycle: draft (editable) -> reviewed (editable until locked) -> locked (frozen).
  status      TEXT NOT NULL DEFAULT 'draft' CHECK (status IN ('draft', 'reviewed', 'locked')),
  created_at  TEXT NOT NULL,
  updated_at  TEXT NOT NULL,
  UNIQUE(owner_id, name),
  UNIQUE(id, owner_id)
) STRICT;

CREATE INDEX plans_owner ON plans(owner_id, name);

CREATE TABLE plan_items (
  id          TEXT PRIMARY KEY,
  owner_id    TEXT NOT NULL REFERENCES owners(id),
  plan_id     TEXT NOT NULL,
  media_kind  TEXT NOT NULL CHECK (media_kind IN ('photo', 'shot')),
  media_id    TEXT NOT NULL,
  -- Sequence order within the plan; the app renumbers inside one transaction (plan_item_reorder).
  position    INTEGER NOT NULL,
  -- Clip boundaries for shots, seconds within the SHOT interval (boundary-safe: validated
  -- against shots.start_s/end_s by trigger and API; NULL means the full shot for reel plans
  -- and is rejected for shots in photo_selects plans). NULL/NULL for photos.
  start_s     REAL,
  end_s       REAL,
  -- Editable treatment recipe (render happens in Task 021, never here).
  pacing_json TEXT NOT NULL DEFAULT '{}',   -- e.g. {"target_duration_s":3.5,"speed":1.0}
  crop_json   TEXT,                          -- CropSpec projection; NULL = no crop decided
  grade_json  TEXT,                          -- grade recipe JSON; NULL = no grade decided
  reason      TEXT NOT NULL DEFAULT '',      -- editable plain-language reason
  -- Why this item is here: frozen ScoreBreakdown components + assessment/annotation evidence
  -- (§6). Written once at add time; never rewritten by later re-rankings.
  signals_json TEXT NOT NULL DEFAULT '{}',
  -- Distinction guarantee (acceptance row: baseline vs personalized remain distinguishable).
  origin      TEXT NOT NULL CHECK (origin IN ('manual', 'general_ranking', 'personal_ranking')),
  -- Rank in the general strong-shot list at add time (1-based; NULL for manual adds).
  general_rank INTEGER,
  -- Rank in the personalized ordering at add time (1-based; NULL unless origin='personal_ranking').
  personal_rank INTEGER,
  -- Non-null iff origin = 'personal_ranking': exactly which profile contributed.
  profile_id            TEXT,
  profile_version       INTEGER,
  profile_algorithm_version TEXT,
  added_at    TEXT NOT NULL,
  updated_at  TEXT NOT NULL,
  UNIQUE(owner_id, plan_id, media_kind, media_id),
  UNIQUE(owner_id, plan_id, position),
  FOREIGN KEY(plan_id, owner_id) REFERENCES plans(id, owner_id) ON DELETE CASCADE
) STRICT;

CREATE INDEX plan_items_plan ON plan_items(owner_id, plan_id, position);

-- Append-only revision snapshots: every mutation of plan_items writes the frozen item set here
-- before mutating, so any revision can be reopened verbatim (TASK-020 acceptance row 6:
-- "saved, versioned, reopened"). Never updated, never deleted while the plan exists.
CREATE TABLE plan_revisions (
  owner_id   TEXT NOT NULL REFERENCES owners(id),
  plan_id    TEXT NOT NULL,
  revision   INTEGER NOT NULL,
  status     TEXT NOT NULL,
  items_json TEXT NOT NULL,               -- frozen projection of plan_items (stable schema)
  note       TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL,
  PRIMARY KEY(owner_id, plan_id, revision),
  FOREIGN KEY(plan_id, owner_id) REFERENCES plans(id, owner_id) ON DELETE CASCADE
) STRICT;

-- Target-existence + cleanup triggers, mirroring 0007/0008:
--   plan_item_target_insert: BEFORE INSERT — photo/shot must exist for (owner_id, media_kind, media_id).
--   photo_plan_cleanup / shot_plan_cleanup: AFTER DELETE ON photos/shots, delete dangling items.
-- Boundary safety for clips: a shot trigger cannot read the parent row's interval cleanly, so
-- containment is validated in the store API (§2) AND by a BEFORE INSERT/UPDATE trigger
-- plan_item_shot_boundary that SELECTs shots.start_s/end_s and RAISE(ABORT) unless
-- new.start_s IS NULL OR (start >= shots.start_s AND end <= shots.end_s AND start < end).
```

Design decisions:

- **`origin` + nullable profile columns make the distinction a data invariant, not a convention.**
  A CHECK trigger (`plan_item_personal_provenance`) enforces: `origin='personal_ranking'` ⇔
  `profile_id`, `profile_version`, `profile_algorithm_version` are all non-null;
  `origin='manual'|'general_ranking'` ⇔ they are all NULL. `general_rank` is always recorded when
  the item came from any ranking, so a personalized pick can always be compared against its
  general rank later — the acceptance requirement that the general strong-shot assessment is
  never hidden.
- **`plan_revisions` is append-only JSON, not mutable history.** Keeping `plan_items` as editable
  current state (mirroring the editorial-annotations split: current state editable, provenance
  append-only) keeps the API simple; snapshots make every prior version byte-reproducible for
  reopen and for TASK-021's manifest provenance.
- **Locked is a real gate, not a UI convention:** every item-mutating store API refuses when
  `plans.status = 'locked'` (store-level check, so no future caller can bypass it). Lifecycle:
  `draft → reviewed → locked`, plus `reviewed → draft` (revert). A locked plan is only ever
  superseded by a new plan (or a restored revision copied into a new draft — §3 `plan_duplicate`).
- **No `feedback_events` writes anywhere in 0009 or the new APIs.** Plan mutations are
  organizational state, like collections (0008). The only feedback writes remain the existing
  `append_feedback` paths (`store/src/lib.rs:1048`), reachable from the plan UI **only** through
  the already-existing explicit `record_feedback`/`review_batch` actions.
- **`context_key` semantics** mirror `style_profiles.context_key` (`0007:68`): `'default'` or a
  named context. It selects which per-context profile `search_assets_in_context` loads via
  `PersonalScorer::load` (`search/src/lib.rs:430`, `:657-677`). A plan named for one context never
  silently ranks with another context's profile.

## 3. Store APIs — `crates/store/src/lib.rs` (SQL stays exclusively in this crate)

New types (owner-scoped, mirroring `Collection`/`CollectionItem` at `store/src/lib.rs:306-326`):

```rust
pub enum PlanKind { PhotoSelects, Reel }
pub enum PlanStatus { Draft, Reviewed, Locked }
pub enum PlanItemOrigin { Manual, GeneralRanking, PersonalRanking }

pub struct Plan { id, owner_id, name, plan_kind, context_key, brief, status,
                  created_at, updated_at }
pub struct PlanItem {
    pub id, pub owner_id, pub plan_id: String,
    pub plan_kind: PlanKind,               // denormalized for validation messages
    pub media_kind: MediaKind,             // photo | shot
    pub media_id: String,
    pub position: i64,
    pub start_s: Option<f64>, pub end_s: Option<f64>,
    pub pacing_json: String, pub crop_json: Option<String>, pub grade_json: Option<String>,
    pub reason: String,
    pub signals_json: String,              // §6
    pub origin: PlanItemOrigin,
    pub general_rank: Option<i64>,
    pub personal_rank: Option<i64>,
    pub profile_id: Option<String>, pub profile_version: Option<i64>,
    pub profile_algorithm_version: Option<String>,
    pub added_at: DateTime<Utc>, pub updated_at: DateTime<Utc>,
}
pub struct PlanItemDraft { /* everything optional except media refs; used by plan_item_add */ }
pub struct PlanRevision { pub owner_id, pub plan_id, pub revision, pub status,
                          pub items_json: String, pub note, pub created_at }
```

APIs (all in `crates/store/src/lib.rs`, following the reference-set/collection block patterns at
`:1451-1954`; owner checks via `ensure_owner_matches` `:4212`; writes inside
`TransactionBehavior::Immediate` — pattern of `bulk_review` `store/src/lib.rs:1954`):

- `plan_create(&mut self, owner_id, plan: &Plan) -> ()` — INSERT; `updated_at = created_at`.
- `plan_list(owner_id) -> Vec<Plan>`; `plan_get(owner_id, plan_id) -> (Plan, Vec<PlanItem>)`
  (items ordered by `position ASC, id ASC`); `plan_delete` (cascades).
- `plan_set_status(&mut self, owner_id, plan_id, status)` — legal transitions
  `draft → reviewed`, `reviewed → draft`, `reviewed → locked`; `locked` is terminal in 020
  (TASK-021 render, then TASK-022-era revision flows may revisit). Writes `updated_at`, appends a
  `plan_revisions` row on every transition.
- `plan_item_add(&mut self, owner_id, plan_id, item: &PlanItem) -> ()` — enforces:
  - plan exists and `status != 'locked'`;
  - `media_kind` matches the plan's kind (`photo_selects` ⇒ photo-only, `reel` ⇒ shot-only)
    — a reel plan is built from shots, honoring the pipeline's scene units (`Shot`
    `store/src/lib.rs:485-496`);
  - **boundary safety:** for shots, `start_s`/`end_s` must be `NULL` (full shot) or satisfy
    `shot.start_s <= start_s < end_s <= shot.end_s` (read via `shot_by_id`
    `store/src/lib.rs:2586`); photos must carry `NULL` boundaries. The SQL trigger (§2) enforces
    the same invariant against direct SQL, mirroring `aesthetic_assessment_target_insert`
    (`0002:136-145`);
  - UNIQUE(owner_id, plan_id, media_kind, media_id) — no duplicate asset per plan;
  - after any items mutation, bumps `plans.updated_at` and appends a `plan_revisions` snapshot
    (revision = previous max + 1) inside the same transaction.
- `plan_item_update(&mut self, owner_id, plan_id, item_id, patch: &PlanItemPatch) -> ()` —
  editable fields: `reason`, `start_s`/`end_s` (re-validated against the shot row), `pacing_json`
  (must be a JSON object), `crop_json`, `grade_json`. `signals_json`, `origin`, and the profile
  columns are **never** writable after insert (frozen evidence). Draft/reviewed only.
- `plan_item_remove`, `plan_item_reorder(&mut self, owner_id, plan_id, ordered_ids: &[String])`
  — renumbers `position` 0..n-1 in one immediate transaction; rejects ids not in the plan.
- `plan_snapshot(owner_id, plan_id, note) -> i64` — explicit user "save version" action;
  `plan_revisions` list API `plan_revisions(owner_id, plan_id) -> Vec<PlanRevision>`.
- `plan_restore_revision(&mut self, owner_id, plan_id, revision) -> ()` — draft only: replaces
  current items with the snapshot's items (re-inserted with fresh ids at snapshot positions,
  `origin`/signals preserved verbatim) and appends a new snapshot row documenting the restore.
- `plan_duplicate(owner_id, plan_id, new_name) -> Plan` — copy of a locked plan back to draft
  (revisions keep the lineage; the original stays frozen). This is the only sanctioned way to
  revise a locked plan.
- Cleanup triggers: `photo_plan_cleanup` / `shot_plan_cleanup` delete dangling `plan_items`
  (pattern of `photo_collection_cleanup` in `0008_collections.sql`), so deleting media never
  breaks a plan's ordering.

## 4. Strong-shot candidates surface ("Selects" flow entry)

**No ranking change.** Two existing surfaces become the candidate generators; a new thin store API
and one new search helper assemble them:

1. **General strong-shot list (no query needed)** — new store API
   `strong_shot_candidates(&self, owner_id, kind: Option<MediaKind>, min_overall: Option<f64>,
   limit: usize) -> Vec<StrongShotCandidate>` where `StrongShotScores { media_kind, media_id,
   overall, confidence, technical_quality, composition_quality, moment_story, repetition_risk,
   model_version }`. One SQL projection over `aesthetic_assessments` using the
   `aesthetic_assessments_strongest` index (`0004_strong_shot.sql:24-26`), LEFT JOIN
   `editorial_annotations` for `usable`/flags (badges only, never filters by machine score —
   "machine scores never clear a privacy flag", blueprint line 129), ordered by
   `overall DESC, confidence DESC`. This is the Task 017 cold-start list: works with **no**
   profile, no feedback, no reference sets.
2. **Ranked candidates from a brief** — `SearchEngine::search_assets` (general) and
   `search_assets_in_context` (`search/src/lib.rs:402-409`) with the user's creative brief as the
   query. The personalized path is exactly the 018a integration: `PersonalScorer::load`
   (`:430`, `:656-677`) applies the gated default + context profiles and returns a per-result
   `ScoreBreakdown` (`:117-137`) whose `personal_affinity`/`context_fit` terms are `0.0` when no
   gated profile exists — the ranking is then bit-identical to the general ranker (test-enforced
   at `search/src/lib.rs:1215-1216` and `:1527`).

The Tauri command `selects_candidates` (§7) returns **both** orderings in one response so the UI
can always show them side by side:

```rust
struct SelectsCandidatesView {
    brief: String,
    context_key: String,                 // resolved ('default' when absent)
    general: Vec<AssetSearchResult>,     // search_assets (context_key = None)
    personalized: Vec<AssetSearchResult>,// search_assets_in_context(context_key) — present
                                         // only when a gated profile exists for the key
    profile: Option<ProfileStamp>,       // id, version, context_key, algorithm_version,
                                         // learned, held_out vs baseline (drives the UI badge)
}
```

`ProfileStamp` is a small public struct added to the search crate
(`pub fn profile_stamp(store, owner_id, context_key) -> Option<ProfileStamp>` reusing
`gated_profile` `search/src/lib.rs:750-757` so the gate has exactly one implementation).
`AssetSearchResult` already carries `start_s`/`end_s` for shots (`search/src/lib.rs:99-100`),
which is what clip candidates need.

**Picking the context.** The UI's plan setup offers: (a) a context key (free text, defaulting to
`default`, mirroring `reference_sets.context_key` `0007:15` and the style panel input
`index.html:176`); (b) optionally restricting the candidate pool to a collection (`collection_id`,
reusing `AssetFilter.collection_id` `store/src/lib.rs:406` via `browse_assets` `:2103` for the
pool before ranking). Reference sets are **evidence**, not candidate pools — they train the
profile; the plan's pool is collections/browse, per blueprint line 71 ("previous work is an
explicit evidence role, not a separate hidden library").

## 5. Video clip/reel planning (planning only — render is TASK-021)

A `reel` plan is an ordered list of shot items; each item is a **recipe entry**, not media:

- `start_s`/`end_s` default to the full shot interval from `shots` (`store/src/lib.rs:490-491`) —
  the boundary-safe default — and may be tightened but never widened past the shot boundaries
  (§2 trigger + store validation). This is the "respect shot boundaries from the pipeline" rule;
  the `split` stage's boundaries are the only legal cut points.
- `pacing_json` initial shape: `{"target_duration_s": <f64>, "speed": 1.0,
  "transition_in": "cut"|"dissolve", "transition_out": "cut"|"dissolve"}` — free-form JSON so 021's
  recipe vocabulary can extend it without a migration.
- `crop_json` / `grade_json` — treatment drafts; the plan item stores them as recipe values. They
  are **not** written to `editorial_annotations` and do not append crop/grade feedback (state vs
  feedback, §1); a user who wants the treatment remembered as taste uses the existing
  `set_annotation` command (`app/src-tauri/src/lib.rs:1673`), which already appends the
  `crop`/`grade` feedback signals.
- **Evidence fields** at add time (§6): transcript snippet via `segments_overlapping`, moment
  evidence (aesthetic components), `repetition_risk` from the assessment.
- **Repetition/sequence penalty (acceptance row 3):** new search module
  `crates/search/src/planning.rs` (no new crate; `search → store` only) with
  `pub fn sequence_penalty(prev: Option<&AssetSearchResult>, candidate: &AssetSearchResult) -> f32`
  — deterministic, from persisted data only:
  same video as previous item → `-0.5 * repetition_risk` (assessment column, `0004:22`);
  near-duplicate (`duplicate_confidence > 0.5`) → `-0.5`;
  plus a small novelty bonus from the assessment's `novelty` component. This is a *planning-time*
  re-ranker for reel plans only; `search_assets*` is untouched, so
  `fixtures/golden/expected_search.json` is untouched (HANDOFF: "Golden tests are correctness").
  The command `plan_candidates` for reel plans returns the shot list with
  `sequence_penalty` per candidate so the UI explains why an order is suggested.
- **No rendering:** no command in 020 calls ffmpeg or writes a derivative file. `plan_get` exposes
  the item fields; TASK-021's recipe tables will reference `(plan_id, plan_item_id, revision)` as
  the hand-off (recorded in that task's spec, not here).

## 6. Explainability and the distinction guarantee

Every plan item freezes, at add time, exactly which signals contributed (acceptance row 4):

```json
// plan_items.signals_json
{
  "breakdown": {            // the ScoreBreakdown of the ranked result at add time
    "semantic": 0.31, "transcript_boost": 0.0, "editorial": 0.02,
    "general_aesthetic": 0.05, "penalties": 0.0,
    "personal_affinity": 0.07, "context_fit": 0.04, "total": 0.41
  },
  "personal_style_score": 0.42,        // raw gated affinity (Option → omitted when None)
  "assessment": { "overall": 0.72, "confidence": 0.8, "model_version": "strong-shot-v1",
                  "explanation_summary": "…", "moment_story": 0.7, "repetition_risk": 0.1,
                  "duplicate_confidence": 0.0 },
  "annotation": { "quality": 4, "standout": true, "usable": true },
  "transcript_evidence": "…snippet overlapping the clip interval…",   // shots only
  "sequence_penalty": -0.05,           // reel plans only
  "ranked_by": "general" | "personalized",
  "query": "warm family reel"          // the brief used, if any
}
```

Provenance columns (`origin`, `general_rank`, `personal_rank`, `profile_id`, `profile_version`,
`profile_algorithm_version`) plus `plans.context_key` answer, from data alone, for every item:
was the general model or a personal profile the deciding surface, and exactly which profile
version. UI labeling rule (020b): items added from the personalized list render a **"Personalized
(profile vN)"** pill and the personalized column is only shown next to the general column — the
general strong-shot assessment is never hidden (TASK-020 acceptance row 2; blueprint lines
87–90 "the non-personalized ranker remains a first-class model and fallback").

Reset/retrain safety: plan items keep their frozen signals even if the profile is later reset
(`style_profile_reset` exists, `app/src-tauri/src/lib.rs:1196`); a plan item's explanation is
historical evidence, not a live score. Re-ranking a plan (future explicit action) writes new
snapshot rows rather than rewriting provenance.

## 7. Tauri commands — `crates/app/src-tauri/src/lib.rs` (020a)

All follow the house pattern (`CommandResult<T>`, `spawn_blocking` for search, per-call
`Store::open`, `DEFAULT_OWNER_ID`, camelCase args — pattern of `record_feedback`
`lib.rs:828`, `library_browse` `:1294`):

- `selects_candidates(query, contextKey, kind, collectionId?, topK) -> CandidatesView` —
  returns `{ general: Vec<AssetSearchResult>, personalized: Vec<AssetSearchResult>,
  profile: Option<ProfileStampView>, sequence: Option<…> }` (§4). `kind` selects photo vs shot
  filtering. When no gated profile exists, `personalized` mirrors `general` and
  `profile = None` — the UI shows "General model only".
- `plan_create(name, planKind, contextKey, brief, collectionId?)`, `plan_list`,
  `plan_get(id)` → plan + items (signals parsed for the UI), `plan_delete(id)`,
  `plan_set_status(id, status)` (validated transitions), `plan_duplicate(id, name)`.
- `plan_add_item(planId, assetType, mediaId, origin, startS?, endS?, reason?, breakdown?,
  profileMeta?, generalRank?, personalRank?)` — the UI passes the ranked result it acted on;
  the command stamps the frozen `signals_json` (§6) from the provided breakdown + a fresh
  `store.aesthetic_assessment(...)` read (`store/src/lib.rs:1023`).
- `plan_update_item(itemId, reason?, startS?, endS?, pacingJson?, cropJson?, gradeJson?)`,
  `plan_remove_item(planId, itemId)`, `plan_reorder(planId, orderedItemIds)`.
- `plan_save_version(planId, note) -> revision`, `plan_revisions(planId)`,
  `plan_restore_version(planId, revision)` (draft only).
- `plan_set_status` enforces the lifecycle; `plan_add_item`/`update`/`remove`/`reorder` refuse on
  `locked` (defense in depth with the store check).
- Registration: extend `generate_handler!` (`lib.rs:2124-2171`, currently 46 commands).
- Update the doctor pinned string `schema=8` → `schema=9` (`lib.rs:2207`; string at `:338`).

## 8. UI — `crates/app/ui/` (020b, branch `task/20b-planning-ui`)

New nav item `#nav-plans` + `#plans-view` section in `crates/app/ui/index.html` (after
`#style-view`, lines 152–195), wired in `search.js`'s `showView` map (`search.js:109`,
`638-641`) or a new `plans.js` module following `style.js`'s ownership pattern
(`style.js:1`). Conventions: DOM via `textContent` helpers (`app.js:215` `cell()`),
no `innerHTML`, no network, `invoke` only.

- **Plan setup panel:** name, kind (Photo selects / Reel), context key (defaults from the
  selected reference set's context, `style.js` patterns), brief textarea, optional collection
  filter, and the candidate split view.
- **Candidates view:** two columns — "General strong shots" and "Personalized"
  (labeled "General model only" when `profile` is absent). Each card reuses the
  `resultBreakdownRows`/`buildBreakdown` pattern (`search.js:209-244`) so the
  `ScoreBreakdown` is rendered identically to search; the personalized column additionally shows
  `context_fit`. Shots carry in/out timecodes. "Add to plan" writes `plan_add_item` with the
  item's `origin` derived from which column the user clicked.
- **Plan editor:** ordered item list (reorder ↑/↓ buttons calling `plan_reorder`), per-item
  reason textarea, boundary editor for shots (in/out inputs with the shot interval shown;
  out-of-range input is disabled client-side *and* rejected server-side), pacing fields,
  crop/grade fields (JSON text inputs initially), status buttons (Mark reviewed / Lock), and a
  versions dropdown listing `plan_revisions` with Restore.
- **Explainability per item:** a "Why?" details block per plan item rendering the frozen
  `signals_json` (breakdown rows + assessment summary), styled after
  `buildBreakdown` (`search.js:229-244`).
- **Distinction badges:** plan items show `origin` pills; the header shows which profile version
  was active ("Profile v3 · learned" or "General model only") — same vocabulary as
  `style_profile_status_view` (`lib.rs:1013-1050`).
- No render/export button on plans (TASK-021 territory). The detail drawer's existing
  "Export clip…" (`index.html:217`, `export_clip` `lib.rs:1879`) remains the only media-writing
  path and is untouched.

## 9. Test plan

Windows-safe store tests (`crates/store/tests/store_roundtrip.rs`; TempDir harness `:24-42`,
owner isolation `:1741`, cascade patterns `:2288`, browse filters `:3042`, cleanup-trigger
patterns `:1957-2096`):

- Migration: fresh v9 migrates once; v8→v9 preserves photos/shots/annotations/feedback/reference/
  collection rows (pattern of `schema_v7_upgrades_to_collections_without_losing_rows` `:3228`).
- Plans: CRUD round trip + owner isolation; UNIQUE(owner_id, name); status CHECK lifecycle
  (locked rejects item mutations at the store level); delete cascades items and revisions.
- Items: position ordering survives insert/remove/reorder; duplicate-asset rejection; boundary
  trigger aborts `start_s`/`end_s` outside the shot interval or non-NULL boundaries on photos;
  photo/shot cleanup triggers remove dangling items; plan rows survive.
- Provenance: `origin='personal_ranking'` without profile meta is rejected by trigger;
  `signals_json` round-trips.
- Revisions: every mutation appends one `plan_revisions` row; `plan_restore_revision` reproduces
  snapshot items; revisions are immutable (UPDATE/DELETE attempts fail).
- Feedback separation: plan mutations append zero `feedback_events` rows (extends the
  state-only pattern of `safety_flags_write_path_is_state_only_and_never_appends_feedback`
  `:2787`), while `record_feedback`/`bulk_review` still append (":2894").

Search tests (Windows-safe, synthetic vectors + hand-built assessments — pattern of
`crates/search/src/lib.rs` tests at `:1040-1049`, `:1215`, `:1527`):

- `plan_candidates` returns both orderings; with no gated profile the personalized list equals
  the general list (bit-identical ordering; extends the no-profile equivalence tests at
  `search/src/lib.rs:1215`, `:1527`).
- `sequence_penalty` determinism: same-video neighbor with `repetition_risk=1.0` scores exactly
  `-0.5` lower; near-duplicate penalty fires; penalty is `0.0` across distinct videos.
- Breakdown completeness: every exported `ScoreBreakdown` field is finite and components sum to
  `total` (already asserted at `:1040-1041` pattern) — reused in the planning surface test.

UI harness (020b; `scripts/ui-harness.mjs` tests map `:42-248`, frozen clock `:275`; add
`invoke` cases to the switch in `crates/app/tests/mock-bridge.js:380-461` and library data to the
map at `:61-77`):

- `plans-panel`: create a photo_selects plan, see both candidate columns (general + personalized
  with profile badge), add an item from each, verify the provenance pill and the "Why?" rows.
- `plan-clip-boundaries`: reel plan item shows shot timecodes; editing an out-of-bounds in-point
  is refused (mock returns the store's error) and the reason field edits persist.
- `plan-versions`: two edits → two revision rows listed; restore returns the earlier ordering.

CI: Linux + macOS matrix unchanged; no new deps; doctor test pinned `schema=9` (`lib.rs:2207`).
Goldens untouched: no ranking-composition change anywhere (`fixtures/golden/expected_search.json`
is exercised by unchanged paths only).

## 10. Acceptance mapping (`.tasks/backlog/TASK-020.md`)

Row 1 (useful ranked selects/clips with no profile) → §4 general strong-shot list +
`search_assets` general column + §5 reel candidates; row 2 (separately explainable personalized
ordering without hiding the general assessment) → §4 two-column candidates + §6 frozen
breakdowns + §8 UI labeling; row 2's "user-supplied creative brief" → plan `brief` field drives
the candidate query; row 3 (video candidates with editable boundaries, pacing, transcript/moment
evidence, repetition/sequence penalties) → §2 item columns + §5 `planning.rs` penalties +
§6 evidence; row 4 (explain why chosen, general vs personal) → §6 signals/provenance; row 5
(edits feed back as explicit evidence only) → §1 state/feedback split; row 6 (saved, versioned,
reopened, handed to render) → §3 plan_revisions + §7 commands (hand-off to 021 is the recipe
fields + revisions, no render code here).

## 11. Sequencing, branches, and constraints

- **020a — `task/20a-planning-core`:** migration `0009_plans.sql`
  (`CURRENT_SCHEMA_VERSION` 8 → 9, `store/src/lib.rs:18-28`), store APIs (§3, §4), search
  planning helper (§5), Tauri commands + `selects_candidates` (§7), small `crushctl plan list/show`
  debug subcommands (mirrors the `crushctl style` surface of 018a), all Windows-safe tests.
  Mergeable alone: no behavior change without the UI; goldens untouched.
- **020b — `task/20b-planning-ui`** (based on 020a, and rebased onto 019b's review UI when it
  lands): plans view, candidates columns, item editor, provenance pills, harness scenarios (§8).
  If 019b is still in flight, 020b rebases onto its library-grid/review patterns rather than
  duplicating index.html sidebar work.
- Constraints honored: `owner_id` on every 0009 row and every API (HANDOFF line 22), applied via
  `ensure_owner_matches` (`store/src/lib.rs:4212`); goldens untouched; no new external crates
  (SQL + rusqlite + serde_json only); no server; blacklist respected; `stage-aesthetic` untouched
  (cold-start purity); originals immutable (plans reference media, never mutate it); `feedback_events`
  stays insert-only via `append_feedback` (`store/src/lib.rs:1048`); machine paths (pipeline,
  stage-*) gain no plan writer — plan tables are written only by the §7 app commands.

## 12. PR/branch notes

- 020a lands first; 020b rebases onto it plus 019b's UI (if landed) to avoid sidebar churn, exactly
  as 019b rebased onto 018b (`TASK-019-impl-plan.md` §9).
- Human hard stops: none new (018's held-out proof and 021's render-golden review are the
  neighbors); 020 adds no learned-status claims. Doctor output pasted in the PR per HANDOFF.

## 13. Verified current-code anchors (this checkout, 2026-08-29)

- `crates/store/src/lib.rs:18-28` — `CURRENT_SCHEMA_VERSION = 8`, MIGRATIONS 0001–0008
  (**next free migration: `0009_plans.sql`, schema v9**)
- `crates/store/src/lib.rs:3508` — `apply_migrations`; `:623` `schema_version()`
- `crates/store/src/lib.rs:169-208` — `AestheticAssessment` (overall, confidence,
  explanation_json, model_version, repetition_risk, duplicate_confidence)
- `crates/store/src/lib.rs:239-256` — `StyleProfile` (version, learned, held_out_metric,
  baseline_metric, context_key)
- `crates/store/src/lib.rs:399-452` — `AssetFilter` / `LibraryAsset` / `LibraryCounts`
- `crates/store/src/lib.rs:485-496` — `Shot` (start_s, end_s, rep_frame_s)
- `crates/store/src/lib.rs:895, 1023` — `upsert_aesthetic_assessment` / `aesthetic_assessment`
- `crates/store/src/lib.rs:1048` — `append_feedback` (append-only; untouched by 0009)
- `crates/store/src/lib.rs:1156, 1160` — `active_style_profile(_for_context)`
- `crates/store/src/lib.rs:1451+` — collection APIs; `:1930` `set_safety_flags`; `:1954` `bulk_review`
- `crates/store/src/lib.rs:2103, 2206` — `browse_assets` / `library_counts` (grid read path;
  candidates pool filter)
- `crates/store/src/lib.rs:2573, 2586` — `shots_for_video` / `shot_by_id` (boundary validation)
- `crates/store/src/lib.rs:4212` — `ensure_owner_matches` idiom
- `crates/store/migrations/0004_strong_shot.sql:24-26` — `aesthetic_assessments_strongest` index
- `crates/store/migrations/0007_reference_sets.sql:9-65`,
  `0008_collections.sql:10-135` — STRICT/trigger conventions to mirror
- `crates/search/src/lib.rs:95-109, 117-137` — `AssetSearchResult`, `ScoreBreakdown`
- `crates/search/src/lib.rs:389-409, 430` — `search_assets(_in_context)`, `PersonalScorer::load`
- `crates/search/src/lib.rs:650-757` — `PersonalScorer` + `gated_profile`
- `crates/search/src/lib.rs:763-791` — `compose_score` (terms the plan signals freeze)
- `crates/search/src/style/trainer.rs:29, 42-51` — `DEFAULT_CONTEXT_KEY`, retrain entry points
- `crates/search/src/style/eval.rs:62` — held-out `evaluate`
- `crates/core/src/job.rs:6-13` — `Stage` enum (`Analyze` at :9; no render stages — 021's)
- `crates/pipeline/src/lib.rs:327, 786` — `analyze_photos` / `analyze_video_shots`
- `crates/app/src-tauri/src/lib.rs:2124-2171` — 46 registered commands (append plan commands here)
- `crates/app/src-tauri/src/lib.rs:627-683` — `search` command (no context_key yet — 020a adds the
  context-aware candidates command)
- `crates/app/src-tauri/src/lib.rs:1013-1050` — `style_profile_status_view`; `:1196` reset;
  `:1204` retrain
- `crates/app/src-tauri/src/lib.rs:338, 2207` — doctor `schema=8` string + pinned test (moves to 9)
- `crates/app/src-tauri/src/lib.rs:1879` — `export_clip` (existing ad-hoc path; not a plan renderer)
- `crates/app/ui/index.html:45-59` — nav (nav-search/library/style); `:152-195` style view;
  `:198-245` detail drawer
- `crates/app/ui/search.js:209-244` — breakdown rows/"Why this result?" builder to reuse;
  `:638-641` nav wiring; `crates/app/ui/app.js:215` — `cell()` helper
- `crates/app/tests/mock-bridge.js:61-77, 380-461` — library map + invoke switch (020b adds plan
  cases); `scripts/ui-harness.mjs:42-248` — scenario map (`style-panel` `:171`,
  `style-add-item` `:218`), clock install `:275`
- `crates/store/tests/store_roundtrip.rs:24` TempDir harness; `:138` fresh-migrate;
  `:161/:3228` upgrade patterns; `:1741` owner isolation; `:2288` collections cascade;
  `:2787` state-only flags; `:2894` bulk review atomicity; `:3042` browse filters
- `docs/dam-feedback-blueprint.md:48-57` (signal strengths), `:87-98` (general model first-class,
  breakdown in plain language), `:149-151` (roadmap step 5 = this task)
- `docs/strong-shot-analysis.md` — evidence contract 020 consumes (assessments are candidate
  evidence, not publish decisions, `:66-68`)
- `docs/HANDOFF.md:19-33` — owner_id, goldens, blacklist, branch convention
- `TASK-021.md:3-17` — render/export acceptance (020 deliberately owns none of it: plans write
  recipes, never renders)
