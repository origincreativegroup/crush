# TASK-018 Implementation Plan: Previous-work examples + personal-style learner

Task: `.tasks/backlog/TASK-018.md` (acceptance sketch — do not edit).
Branch: `task/18-style-learner`. **This work is split into two PRs** (see §9): the store/trainer/eval
slice and the UI/command slice. HANDOFF rule "one task per PR" is honored by treating 018a/018b as
separate reviewable units against the same task.

Sources of truth: `docs/dam-feedback-blueprint.md` ("Feedback signals", "Previous work as style
evidence", "Personal style model"), `docs/HANDOFF.md`, `.tasks/done/TASK-014.md`.

---

## 0. TASK-019 dependency analysis (no hard block)

TASK-019 would supply: review UI (picks/rejects/stars/pairwise compare), collections, saved
searches, and "collections designated as previous-work reference sets with whole-set vs
selected-example meaning".

What TASK-018 truly needs vs. what 019 would provide:

| 018 need | Already exists | 019 surface needed? |
|---|---|---|
| Pick/reject/rating/prefer evidence | `feedback_events` + `Store::append_feedback` (store/src/lib.rs:852) + `record_feedback` Tauri command (app/src-tauri/src/lib.rs, search.js:503) | No |
| Curated previous-work examples | Nothing today | No — 018 owns its own `reference_sets` schema and confirm flow (§2). When 019 lands, its "designate collection as reference set" UI writes into 018's tables; 018 reserves a nullable `source_collection_id` column for that later wrap. |
| Pairwise-compare UI | None | No — the trainer consumes existing `prefer` events; the compare UI can arrive with 019 |
| Whole-set vs selected-example meaning | None | No — `scope` column in 018's schema (§2) |

**Conclusion: 018 does not hard-block on 019.** The minimal subset 018 borrows is *nothing* at the
API level; the only coupling is stylistic (019's collections may later point at reference sets via
`source_collection_id`). TASK-022 (Reel Studio importer) will later feed confirmed evidence through
the same trainer inputs; no interface change required for it either.

TASK-024 (landing in parallel) exports a `ScoreBreakdown`. It does **not exist in the tree yet**
(grep confirmed). §5 designs search's breakdown so it can adopt 024's struct or be merged into it
without reworking: the requirement is that the exported breakdown carries `personal_affinity` and
`context_fit` as first-class fields. Whichever lands second renames/adopts; no logic churn.

---

## 1. Current-state anchors (verified)

- Feedback-centroid baseline trainer: `crates/search/src/lib.rs:73-137` (`retrain_style_profile`),
  signal strengths at `lib.rs:81-91`: Pick/Publish `1.0`, Reject `-1.0`, Rating `(v-3)/2`,
  Prefer `+1/-1` on compared asset, Export `0.5`, Crop/Grade/Tag/Edit `0.25`.
- Ranking composition: `crates/search/src/lib.rs:458-461` and `:498-501` —
  `score = cosine + transcript boost + editorial_adjustment + general_aesthetic_adjustment
  + personal_style_score * 0.15` (the `0.15` is a magic constant to formalize).
- Personal affinity = `dot_512(vector, profile.embedding_weights)`
  (`crates/search/src/lib.rs:560-576`). Aesthetic features are read per asset via
  `store.aesthetic_assessment` (store/src/lib.rs:830-850).
- Schema v2 (`crates/store/migrations/0002_dam_feedback.sql`): `feedback_events` (lines 80-99,
  CHECK-constrained signal enum, `context_json` free-text), `style_profiles` (lines 101-119,
  `UNIQUE(owner_id, name, version)`, one active per owner, `held_out_metric REAL` already present),
  append-only cleanup triggers (168-180).
- Migration runner: `crates/store/src/lib.rs:18-22` (`MIGRATIONS` array), `apply_migrations`
  (lib.rs:2065-2101) — strictly sequential, schema_version table. **Next free version is 5;
  TASK-024 may claim 0005. This plan claims `0006_reference_sets.sql`** and must rebase if 024
  takes 0006 first.
- Profile APIs: `put_style_profile` (store/src/lib.rs:908, deactivates prior active inside an
  immediate transaction), `active_style_profile` (lib.rs:978). No list-versions, reset, or
  reactivate API yet.
- Strong-shot model surface: `crates/stage-aesthetic/src/lib.rs` — `MODEL_VERSION = "strong-shot-v1"`
  (line 11), `StrongShotScores` (lines 58-91: 20+ named components + overall + confidence +
  explanation_json), `analyze()` (line 114). Module doc (lines 1-5) guarantees it "never uses
  identity or owner feedback" — **stage-aesthetic must gain zero new dependencies; it stays
  cold-start pure.** The trainer consumes *persisted* `aesthetic_assessments` rows, never the
  stage crate.
- Tauri pattern: `crates/app/src-tauri/src/lib.rs` — everything macOS-gated (`#[cfg(target_os =
  "macos")]`), `CommandResult<T> = Result<T, String>`, `RuntimeState`, camelCase serde.
- UI pattern: `crates/app/ui/search.js` / `app.js` — DOM built with `textContent` helpers
  (search.js:90), `invoke(...)` only, **no `innerHTML`, no `fetch`/network**.
- Test layout: `crates/search/src/lib.rs:665-908` shows the Windows-safe pattern (TempDir + Store
  + synthetic vectors, `explicit_feedback_trains_a_personal_ranker_that_changes_order` at :783).
  Golden: `fixtures/golden/expected_search.json` (HANDOFF: "Golden tests are correctness").

---

## 2. Schema — `crates/store/migrations/0006_reference_sets.sql`

New tables (STRICT, owner-scoped like every 0002 table):

```sql
CREATE TABLE reference_sets (
  id            TEXT PRIMARY KEY,
  owner_id      TEXT NOT NULL REFERENCES owners(id),
  name          TEXT NOT NULL,
  context_key   TEXT NOT NULL,            -- e.g. 'default', 'homepage-hero'
  description   TEXT NOT NULL DEFAULT '',
  scope         TEXT NOT NULL CHECK (scope IN ('whole_set', 'selected')),
  -- 'unconfirmed' until the user explicitly confirms; 'disabled' mutes without deleting;
  -- removal is a real DELETE (see below).
  status        TEXT NOT NULL DEFAULT 'unconfirmed'
                CHECK (status IN ('unconfirmed', 'confirmed', 'disabled')),
  source_collection_id TEXT,              -- reserved for TASK-019 collection designation
  created_at    TEXT NOT NULL,
  confirmed_at  TEXT,
  UNIQUE(owner_id, name)
) STRICT;

CREATE TABLE reference_set_items (
  owner_id    TEXT NOT NULL REFERENCES owners(id),
  set_id      TEXT NOT NULL,
  media_kind  TEXT NOT NULL CHECK (media_kind IN ('photo', 'shot')),
  media_id    TEXT NOT NULL,
  role        TEXT NOT NULL DEFAULT 'positive' CHECK (role IN ('positive', 'excluded')),
  added_at    TEXT NOT NULL,
  PRIMARY KEY(owner_id, set_id, media_kind, media_id),
  FOREIGN KEY(set_id, owner_id) REFERENCES reference_sets(id, owner_id) ON DELETE CASCADE
) STRICT;
```

Style-profile extensions (versioning + eval gate):

```sql
ALTER TABLE style_profiles ADD COLUMN context_key TEXT NOT NULL DEFAULT 'default';
ALTER TABLE style_profiles ADD COLUMN baseline_metric REAL;          -- non-personalized baseline on the same split
ALTER TABLE style_profiles ADD COLUMN metrics_json TEXT NOT NULL DEFAULT '{}';
ALTER TABLE style_profiles ADD COLUMN learned INTEGER NOT NULL DEFAULT 0 CHECK (learned IN (0,1));
CREATE INDEX style_profiles_owner_context ON style_profiles(owner_id, context_key, version);
```

Design decisions:

- **Uncurated folders contribute NO positive signal** by construction: the trainer reads
  `reference_set_items` only via `JOIN reference_sets ... WHERE status = 'confirmed'`.
  `unconfirmed`/`disabled` sets are inert. `scope='whole_set'` means every item row (which the
  confirm flow bulk-inserts) is positive; `scope='selected'` means only items marked `positive`
  count. There is no code path where mere ingest creates positive evidence.
- **Signal-strength reuse:** the blueprint's signal table (blueprint lines 48-57) has no
  `curated` value in the `feedback_events` CHECK enum, and 0002's enum is frozen. Rather than
  mutate an append-only enum, confirmed reference-set examples are read *directly* by the trainer
  as positive samples with curated strength `1.0` (same tier as pick/publish in
  search/src/lib.rs:81-91). `feedback_events` stays purely user-action evidence; curation is a
  separate auditable table. This preserves the append-only guarantee without a data migration.
- **Context scoping:** `context_key` mirrors the canonical key the trainer accepts inside
  `feedback_events.context_json` (`{"context": "<key>"}` — undefined/`"default"` collapses to
  `"default"`). A preference in one context never becomes a universal rule because samples are
  partitioned by `context_key` before training (§3).
- **Removal + reproducibility:** deleting a reference set cascades its items; the next retrain
  reproduces the profile from remaining evidence (blueprint line 76). Feedback events survive set
  deletion (they are the user's actions, not the set's) — intentional.
- Cleanup triggers mirroring 0002's pattern are added for photo/shot deletion so items cannot
  dangle.

## 3. Trainer — `crates/search/src/style/trainer.rs` (new module in `crush-search`)

Keep `retrain_style_profile` as the name callers use; move implementation into
`crates/search/src/style/` (`mod.rs`, `trainer.rs`, `eval.rs`). **No new crate** — search already
depends on store, and keeping the trainer beside the scoring composition keeps the dep graph
`search → store` only. **No new external dependencies**: the head is hand-written f64 gradient
descent (fixed iteration count, deterministic), not `tch`/`ndarray` (blacklist + auditability).

- Replace `algorithm_version = "feedback-centroid-v1"` with `"personal-residual-v1"`. The general
  model stays intact and first-class: the personal head is a **residual** over it. Residual starts
  at zero, so with no evidence the ranker is exactly the current general ranker.
- Feature spaces (both already persisted per asset):
  1. CLIP residual `w_v ∈ R^512` stored in `style_profiles.embedding_weights` (dimension unchanged;
     `dot_512` path and `personal_style_score` in search/src/lib.rs:560-576 keep working).
  2. Aesthetic-feature residual `w_a` over the named `aesthetic_assessments` columns
     (technical_quality, composition_quality, moment_story, etc. — the StrongShotScores components
     in stage-aesthetic lib.rs:58-91), centered/scaled to [0,1]→[-0.5,0.5], stored in
     `feature_weights_json` as `{"feature_weights": {...}, "lambda": ..., "clip_residual_norm":
     ...}` (the column already exists and is currently `"{}"` at search/src/lib.rs:129).
- Objective per (context_key): logistic pairwise loss
  `Σ log(1 + exp(-y · ⟨w, x⁺ - x⁻⟩)) + λ·‖w‖²`, where pairs come from:
  - `prefer` events (strongest; reuse existing strengths),
  - pick/reject and rating pairs (rating vs. rating across assets; pick vs. reject),
  - confirmed reference-set items as positives vs. **negatives drawn from the same owner's
    rejected/unrated pool** — never from other owners (privacy rule, blueprint line 126),
  - crop/grade/export/publish as weak-positive single-sample evidence with the existing
    coefficients (0.25/0.5) folded as soft labels.
- **Regularization toward the general model when sparse:** `λ = λ0 / (1 + sample_count)`, plus a
  hard cap on `‖w‖` and a minimum-samples floor (default 6 samples for `default`, 4 for a named
  context) below which the trainer returns `Ok(None)` and leaves the previous profile untouched —
  sparse feedback regularizes toward zero residual (i.e., toward the general model) and never
  invents certainty.
- The trainer never deactivates or deletes the general components (`editorial_adjustment`,
  `general_aesthetic_adjustment` in search/src/lib.rs:543-558); it only ever writes the residual.
- Per-context profiles: `name = context_key`, one active profile per `(owner, context_key)` — the
  0006 index supports this; the "one active per owner" partial unique index from 0002 stays for
  `default`. Store API change: uniqueness enforcement moves from "one active per owner" to "one
  active per (owner, context_key)" via a new partial index `WHERE active = 1 AND context_key =
  'default'` semantics — implemented as: `put_style_profile` deactivates the prior active row for
  `(owner_id, profile.context_key)` instead of owner-wide (store/src/lib.rs:936-941).

## 4. Versioning + reversible reset — store APIs

Extend `crates/store/src/lib.rs` (SQL stays exclusively in this crate):

- `style_profiles(owner_id) -> Vec<StyleProfile>` — all versions, ordered.
- `style_profiles_for_context(owner_id, context_key)`.
- `activate_style_profile(owner_id, profile_id)` — reversible: flips `active` between retained
  versions inside one transaction; old rows are never deleted.
- `reset_style_profiles(owner_id) -> usize` — sets `active = 0` on every row (reversible: re-run
  `activate_style_profile` or retrain); returns count for the UI.
- Profile status surface for the UI: `StyleProfile` gains no new Rust fields beyond the columns in
  §2 (`context_key`, `baseline_metric`, `metrics_json`, `learned` — all parsed in
  `style_profile_from_row`).

"Versioned with sample count, feature/model versions, metrics" is satisfied by: `sample_count`
(exists), `algorithm_version` + `feature_weights_json` content (§3), `held_out_metric` +
`baseline_metric` + `metrics_json` (§5), monotonic `version` (exists, search/src/lib.rs:119-127
continues to increment from the previous row).

## 5. Held-out evaluation gate — `crates/search/src/style/eval.rs`

- **Metric:** held-out pairwise ranking accuracy — fraction of held-out preference pairs where the
  personalized residual ranks `x⁺` above `x⁻`. **Baseline:** the same pairs scored by the
  non-personalized ranker (residual forced to zero: ties broken deterministically → 0.5; when
  aesthetic assessments exist for both sides, general `overall` ordering counts as the baseline
  vote). A profile is `learned` iff:
  `held_out_pairs >= 4` AND `personal_accuracy > baseline_accuracy` AND
  `personal_accuracy >= 0.6` (strict improvement over the non-personalized baseline, per
  acceptance; never equal, never invented — ties count as failures).
- **Split:** deterministic leave-one-out over prefer pairs ordered by `(created_at, id)`; every
  k-th pair (k=3) is held out, the rest train. Split is recomputed at each training run from the
  append-only event log, so the same evidence produces the same profile bytes.
- **Where stored:** `style_profiles.held_out_metric` (exists), `baseline_metric`, and
  `metrics_json` = `{"held_out_pairs": n, "personal_accuracy": x, "baseline_accuracy": y,
  "learned": bool, "split": "loo-every-3rd", "trainer": "personal-residual-v1"}`.
- **Enforcement points (defense in depth):** the gate is set at train time, and *re-checked* at
  ranking time (§6): a profile with `learned = 0` or `held_out_metric <= baseline_metric` is
  ignored by scoring even if somehow active.
- **Harness:** `cargo test` based, Windows-safe: synthetic owners/assets with seeded PRNG vectors
  and hand-built `aesthetic_assessments` in TempDir stores (pattern of search/src/lib.rs:833-900).
  A test where the planted style direction beats baseline and one where noise feedback correctly
  refuses to mark `learned`. No network, no model downloads.
- **Human hard stop:** HANDOFF lists "held-out style proof in Task 018" as a human gate. The PR
  pastes the eval harness output; John reviews before merge.

## 6. Ranking integration — `crates/search/src/lib.rs`

- New `pub struct RankFactors` (serde snake_case) with fields
  `semantic_relevance, general_quality, personal_affinity, context_fit, penalties, total`, plus
  `AssetSearchResult.score_breakdown: Option<RankFactors>`. This mirrors TASK-024's ScoreBreakdown
  contract (§0): identical field vocabulary so the parallel landing merges structurally, and
  `personal_affinity` / `context_fit` are exported from day one.
- Decompose the current magic sum (search/src/lib.rs:458-461):
  - `semantic_relevance` = cosine + transcript boost,
  - `general_quality` = `editorial_adjustment + general_aesthetic_adjustment`,
  - `personal_affinity` = `dot_512(vector, active profile weights) * PERSONAL_WEIGHT` (const
    `0.15`, named) **only when** an active profile for the request context (or `default`) exists
    and passes the §5 gate re-check; otherwise `0.0` and the field is exported as `0.0`, never
    `null`-invented certainty,
  - `context_fit` = difference between the active per-context profile affinity and the default
    profile affinity when the request carries a context key; `0.0` when no context is requested
    (initially always — context arrives with 019/020 UI),
  - `penalties` = `usable=false` → `-1.0` moved out of `editorial_adjustment` into its own term
    (plus repetition_risk and privacy as future additions — columns already exist in 0004).
- `search_assets` gains an optional `context_key: Option<&str>` parameter (threaded from CLI and
  Tauri with `None` default, so existing call sites and goldens are untouched).
- **Fallback enforcement:** when `active_style_profile` returns `None` (fresh owner, or after
  reset — §4), the scoring path must be bit-identical to today's general path minus the
  personal term. A dedicated test asserts `search_assets` output equals the no-profile output
  after `reset_style_profiles`. The core never relies solely on personal evidence because the
  residual is bounded and the general terms always contribute.

## 7. Tauri + UI surface

Commands (all in `crates/app/src-tauri/src/lib.rs`, macOS cfg, `CommandResult`, camelCase):

- `reference_set_create(name, contextKey, description, scope)` / `reference_set_list` /
  `reference_set_add_item(setId, mediaKind, mediaId)` / `reference_set_remove_item(...)` /
  `reference_set_confirm(setId)` (bulk-inserts whole_set items if scoped so, sets status) /
  `reference_set_disable(setId)` / `reference_set_delete(setId)`.
- `style_profile_status` — active profile + learned flag + metrics + reference-set counts (drives
  the "learned vs. baseline" badge).
- `style_profile_reset` — calls `reset_style_profiles`; UI then shows "using general model".
- `style_profile_retrain` — invokes the trainer (replacing today's call of `retrain_style_profile`
  in lib.rs:15); returns metrics.

UI (`crates/app/ui/`, following app.js/search.js conventions — DOM via `textContent` helpers, no
innerHTML, no network):

- New "Style" section in `index.html` + `styles.js` (or extend `search.js`): list reference sets
  with status pills (reuse the pill pattern at app.js:270), add-current-asset-to-set button in the
  detail drawer, Confirm / Disable / Delete actions, profile status line ("Learned · held-out
  0.78 vs baseline 0.61" or "General model only"), Reset button with confirm.
- Extend `crates/app/tests/ui-harness.html` with DOM assertions for the new panel (no network,
  mocked `invoke`).

## 8. Test plan

Windows-safe (run everywhere; pure store/search, TempDir, synthetic vectors — pattern of
search/src/lib.rs:665-908):

- Store: reference-set CRUD round-trip, owner isolation, cascade on set delete, unconfirmed set
  yields no trainer samples, cleanup triggers on photo/shot delete, profile version list /
  activate / reset round-trips.
- Trainer: determinism (two runs → identical weights), sparse-feedback returns None and leaves
  previous profile untouched, regularization shrinks residual vs. centroid, context partitioning
  (a preference in context A does not move profile `default`).
- Eval gate: planted-style fixture marks `learned` and beats baseline; noise fixture refuses.
- Ranking: breakdown fields exported and sum to `total`; no-profile path bit-identical to the
  current general path; post-reset fallback identical to Task 017 strong-shot ordering; gate
  re-check ignores an unlearned-but-active profile.

CI-gated / Mac: Tauri command compile+run, UI harness additions, any ffmpeg-adjacent path (none
here). Golden discipline: **no edits to `fixtures/golden/expected_search.json`** — golden runs have
no active profile, and the no-profile path is byte-identical by construction (test in the list
above proves it). New deterministic fixtures (synthetic vectors, not media) are allowed.

Acceptance mapping: rows 1-2 → §2/§7; row 3 → §3; row 4 → §6 + TASK-024 alignment; row 5 → §4;
row 6 → §5 (+ human hard stop); row 7 → §3 regularization + §6 never-invent rule; row 8 → §6
fallback + §4 reset.

## 9. PR split and dependency rules

- **PR 018a** (`task/18a-style-learner`): migration 0006, store APIs (§2, §4), trainer + eval
  (§3, §5), search integration + breakdown (§6), CLI `crushctl style retrain/status/reset`
  (small, mirrors existing debug subcommands), all Windows-safe tests. Mergeable alone: behavior
  unchanged without a profile.
- **PR 018b** (`task/18b-style-ui`, based on 018a): Tauri commands + UI (§7) + harness tests.
- Crate dependency rules (unchanged): `store` → nothing; `search` → `store` only;
  `stage-aesthetic` → **must not** gain store/search/feedback deps (cold-start purity,
  stage-aesthetic lib.rs:1-5); `app` → may use both. No new external crates; no blacklisted deps;
  no server; no Docker.

## 10. Verified current-code anchors

- `crates/search/src/lib.rs:73-137` — `retrain_style_profile` baseline + signal coefficients
  (81-91)
- `crates/search/src/lib.rs:458-461, 498-501` — ranking composition, `0.15` personal weight
- `crates/search/src/lib.rs:543-576` — editorial/aesthetic adjustments, `personal_style_score`
- `crates/search/src/lib.rs:665-908` — Windows-safe test pattern incl. feedback-trained ranker
- `crates/store/src/lib.rs:18-22, 2065-2101` — migration runner/sequence
- `crates/store/src/lib.rs:220-246` — `FeedbackEvent` / `StyleProfile` structs
- `crates/store/src/lib.rs:852-906` — `append_feedback` validation + `feedback_events`
- `crates/store/src/lib.rs:908-990` — `put_style_profile` / `active_style_profile`
- `crates/store/migrations/0002_dam_feedback.sql:80-119` — feedback_events, style_profiles
- `crates/store/migrations/0004_strong_shot.sql` — strong-shot columns + jobs rebuild
- `crates/stage-aesthetic/src/lib.rs:11, 58-91, 114` — MODEL_VERSION, StrongShotScores, analyze
- `crates/app/src-tauri/src/lib.rs:1-45` — macOS cfg, RuntimeState, command patterns
- `crates/app/ui/search.js:90, 166, 503` — textContent helpers, invoke, record_feedback
- `docs/dam-feedback-blueprint.md:44-98` — signal table, previous-work rules, ranking formula
- `docs/HANDOFF.md:19-33` — rules, blacklist, hard stops, branch convention
- `.tasks/done/TASK-014.md` — feedback/profile foundation record
