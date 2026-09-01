# TASK-037 — implementation plan (first-class spans: adjustable boundaries)

Status: **planned, not started**. Parent acceptance: `.tasks/backlog/TASK-037.md`. Branch
`task/37-span-first-class`, after the 021/022 merge. Everything below was verified against the
current `task/21-render-export` head (`823c867`, schema v11) — file:line references are current.

## The defect, verified in code (four clamps, not two)

The task file names two clamps (store + executor). There are actually **four**, and one of them
is in SQL, so a store-code-only change is defeated by the database:

1. **Store API** — `validate_plan_item_against_media` (`crates/store/src/lib.rs:3648`), the
   `MediaKind::Span` arm at `lib.rs:3674-3692`: `ensure!(start_s >= span.start_s && end_s <=
   span.end_s && end_s > start_s, "plan item boundaries … must stay inside imported span …")`.
   Called from `plan_add_item` (`lib.rs:2349`), `plan_update_item` (`lib.rs:2433`), and the
   revision-restore path (`lib.rs:2668`).
2. **SQL triggers** — migration 0011 (`crates/store/migrations/0011_reel_studio_import.sql`)
   creates `plan_item_boundaries_insert` and `plan_item_boundaries_update`, whose `span` arms
   abort with `'plan item boundaries must stay inside the imported span'`. Any Rust-side
   loosening without rebuilding these triggers still aborts inside SQLite.
3. **Reel executor** — `resolve_reel_v1_sources` (`crates/pipeline/src/render.rs:1310`), span arm
   at `render.rs:1353-1376`: resolves `(video_id, bound_start, bound_end)` from the span row and
   `ensure!(item.start_s >= bound_start && item.end_s <= bound_end, "frozen reel boundaries
   must stay inside {} {}")`. (The executor separately checks `item.end_s <= video.duration_s`
   when duration is known, `render.rs:1391-1396` — the video range is already half-present.)
4. **Projects edit form** — `crates/app/ui/plans.js:476-478`: the In/Out inputs take
   `min: candidate.start_s, max: candidate.end_s` from the frozen `signals_json` candidate, and
   `previewRange` (`plans.js:228-239`) clamps scrubbing to the same candidate range. The
   candidate is frozen at import with `start_s`/`end_s` = the span's own boundaries
   (`crates/pipeline/src/reel_studio_import.rs:708-720`). The browser-harness mock enforces the
   same clamp (`crates/app/tests/mock-bridge.js:620`).

## Design decisions

- **Clamp target = the source video range `[0, video.duration_s]`**, mirroring the executor's
  existing duration check and the `manual_span_bounds_*` triggers (0011), which already clamp
  *spans* to `videos.duration_s + 0.001`. Shot and photo arms are untouched anywhere.
- **`video.duration_s` is required for span item edits.** The importer only creates spans for
  indexed videos (duration probed at ingest), so NULL duration is a degenerate case; refuse
  with a clear error rather than silently falling back to the span clamp (which would
  reintroduce the frozen-container defect for that edge).
- **Imported span boundaries stay the item's default.** The `signals_json` candidate keeps
  `start_s`/`end_s` = span boundaries (the "imported default" the UI shows), and the
  `boundary_basis`/`boundary_tolerance_s` note stays visible for `catalogue_tc` spans. No
  importer change to the candidate.
- **`adjusted` provenance is derived in the store, not passed in by callers.** Inside
  `plan_add_item`/`plan_update_item`, when `media_kind == Span` and the item's `(start_s, end_s)`
  differ from the span row's boundaries, the store writes `"adjusted": true` plus
  `"adjusted_at"` into `provenance_json` (removing the marker when boundaries return to the
  imported default). `validate_plan_item_provenance` (`lib.rs:7801-7830`) only *requires*
  `source`/`external_id` for historical/imported items and does not enforce an exact key set,
  so the extra key passes validation today — verified. Deriving it in the store means it cannot
  drift, be spoofed by the app layer, or be lost on a round trip.
- **Re-import never reverts an adjusted item — verified mechanics, plus one latent fix.**
  Today the importer never rewrites plan items of an existing project: recipes whose project
  name already exists are outcome `skipped` (`reel_studio_import.rs:558-571`), and spans refresh
  only when catalogue evidence changed (`span_evidence_equal`, `reel_studio_import.rs:520`,
  compared at `:941-962`). So adjusted boundaries survive re-apply by construction. The latent
  bug this task fixes: with the old span clamp, a *refreshed* span (evidence changed → span
  boundaries updated) could shrink below an item's saved boundaries and make the item
  unrenderable; with the clamp at the video range, span refreshes can no longer invalidate
  adjusted items. Note honestly: if the user *deletes* the project and re-imports, the recipe is
  recreated with the catalogue's original boundaries — the adjustment lived on the deleted
  project. That is acceptable and must be stated in the docs.
- **Byte-stability.** Only the span arm changes; the shot arm of every clamp (store, triggers,
  executor) and the whole photo/clip path are untouched. The reel manifest's `source_evidence`
  and `verification` blocks do not include span boundaries, so manifest bytes for existing
  paths are unaffected. Proven: (a) code-path inspection — no shot/photo arm changes; (b) a
  fixture render hash comparison (below).

## Found in code — contradicts/extends the task file (flagged)

- **The app's Projects export paths do not support span items at all**, which the task file's
  acceptance (and the 022 record's "span rendering" phrasing) glosses over:
  - `render_project_reel_job` rejects every non-shot item with a wrong message —
    `crates/app/src-tauri/src/lib.rs:2509-2512` (`ensure!(item.media_kind == MediaKind::Shot,
    "item {} is a photo; …")` — a span item gets "is a photo").
  - `render_project_clip_job` only finds shot items — `lib.rs:2369` (`.find(|item|
    item.media_kind == MediaKind::Shot && item.media_id == shot_id)`), so the clip-export
    offer the UI makes for spans (`plans.js:253` marks spans exportable-as-clip) always fails
    backend-side with "clip … is not selected in this project".
  - The reel-export UI disables span projects with "This sequence includes photos"
    (`plans.js:316-321`).
  The executor itself resolves spans fine (`render.rs:1353`), and the importer's pipeline test
  renders a span project through the durable reel path — the gap is only in the two app export
  commands and their UI gating. **Recommendation:** fold a minimal unblock into 037 step 5
  (admit spans in `render_project_reel_job` with span-aware source snapshots; extend
  `render_project_clip_job`/`resolve_video_source` (`render.rs:1496-1535`, currently
  `unreachable!("snapshot parser accepts video or shot")`) with a span arm; fix the UI copy).
  If the orchestrator wants 037 kept exactly at the task file's scope, defer to a follow-up —
  but then "first-class spans" still cannot be exported from Projects, which is odd enough that
  John should hear it flagged.

## Steps

### 1. Migration 0012 — rebuild the span boundary triggers

`crates/store/migrations/0012_span_item_video_range.sql` (new; bump
`CURRENT_SCHEMA_VERSION` to 12 in `crates/store/src/lib.rs:18` and add to `MIGRATIONS` at
`lib.rs:19-34`):

- Recreate `plan_item_boundaries_insert` / `plan_item_boundaries_update` with the span arm
  clamping to the source video: join `manual_spans → videos` and abort when
  `NEW.start_s < 0` or `NEW.end_s > videos.duration_s + 0.001` or `NEW.end_s <= NEW.start_s`
  (mirror the `manual_span_bounds_*` pattern from 0011, including the +0.001 duration slack).
  When `videos.duration_s` is NULL the trigger cannot check the upper bound — leave the upper
  check to the store API (which refuses NULL duration for span items, above) and let the
  trigger enforce only `start_s >= 0` and `end_s > start_s` in that case.
- The **shot arms of both triggers are recreated byte-identically** (they must keep enforcing
  the shot clamp — shot paths are the approved render paths).
- Migration header records why: imported span boundaries are catalogue defaults, not physical
  limits; the source video is in the library.

Tests (`crates/store/tests/store_roundtrip.rs`, next to
`imported_spans_survive_resplit_and_carry_historical_plan_provenance` at `:4823`):
extend-beyond-span-but-inside-video accepted; extend past `duration_s` refused by the trigger;
shrink inside the span still works; shot clamp unchanged (existing shot boundary tests must
pass untouched); v11→v12 migration round trip.

### 2. Store — span arm clamps to the video range; `adjusted` provenance

- `validate_plan_item_against_media` span arm (`lib.rs:3674-3692`): load the span (ownership +
  existence unchanged), then the video via `span.video_id`; require `video.duration_s` (clear
  error when NULL); `ensure!(start_s >= 0.0 && end_s <= duration + 0.001 && end_s > start_s)`.
  Return the span so callers can derive the marker.
- `plan_add_item` (`lib.rs:2342`) / `plan_update_item` (`lib.rs:2402`): after validation, when
  `media_kind == Span`, compare the item's boundaries to the span row's; write/clear
  `"adjusted": true` + `"adjusted_at"` in `provenance_json` (JSON merge, preserving
  `source`/`external_id`/`import_id`/`boundary_basis`/`boundary_tolerance_s` lineage). The
  revision snapshot/restore path (`plan_save_revision`/`plan_restore_revision`) round-trips
  `provenance_json` unchanged, so a restore of a pre-adjustment revision clears the marker via
  the normal update path — add a test for exactly that.

Tests: adjusted marker recorded on first divergent save; cleared on return-to-default; survives
revision save/restore; re-import (pipeline test, step 4) does not revert.

### 3. Executor — span resolution clamps to the video range

- `resolve_reel_v1_sources` span arm (`render.rs:1353-1376`): keep the span lookup (ownership +
  "not owned by this owner" error) and the `video_id == snapshot.source_id` check; replace the
  span-bounds `ensure!` with the video-range check (`item.start_s >= 0.0 && item.end_s <=
  video.duration_s`, requiring duration — the video row is already loaded at `render.rs:1378`).
  The existing separate duration check at `render.rs:1391-1396` folds into this.
- **Byte-stability proof** (coordinate with the 021 owner as TASK-035 does): on macOS, render
  the 036 fixture ordered-reel (shots only) and one photo + one clip derivative before and
  after the change; assert identical output SHA-256 (the deterministic-render pattern from
  `crates/pipeline/tests/render_jobs.rs:318-359`). Add a pipeline test rendering a span project
  with unadjusted boundaries before/after — identical bytes (the clamp only loosens a
  precondition; no command line changes).

### 4. Importer — no boundary changes; idempotence test

- No importer code change for defaults (candidate stays span boundaries). Add a pipeline test
  (`crates/pipeline/tests/reel_studio_import.rs`): apply → adjust an item's boundaries beyond
  the span (inside the video) → re-apply the same catalogue → segment outcome `unchanged`
  (evidence equal), recipe outcome `skipped` (project exists), item boundaries and `adjusted`
  marker unchanged. Also: re-apply after the *catalogue* changed a segment's boundaries → span
  refreshes, adjusted item keeps its boundaries and still validates (the latent-bug fix).

### 5. App + UI — form ranges from the video

- `plan_items` command (`crates/app/src-tauri/src/lib.rs`, `plan_item_view` ~`:2646`): for span
  items, attach the source video range (e.g. `sourceRange: { startS: 0.0, endS:
  video.duration_s }`) resolved through `manual_span_by_id → video_by_id`. (If step 5's
  optional export unblock is taken, it lands here too — see the flagged finding above.)
- `crates/app/ui/plans.js`: for span items, In/Out `min`/`max` come from the video range
  (`:476-478`), the "Available source …" line quotes the video range and keeps the
  boundary-basis/tolerance sentence for `catalogue_tc` spans, and `previewRange` (`:228-239`)
  clamps span scrubbing to the video range. Shot items keep the candidate (shot boundaries).
- `crates/app/tests/mock-bridge.js:603-620`: mock span items carry the video range;
  `plan_update_item` mock validation switches to the video range.
- Harness (`scripts/ui-harness.mjs`, `plans-historical` at `:121`): extend the scenario — fill
  In/Out past the imported boundaries (inside the video), save succeeds, tolerance note still
  renders, historical pill unchanged.

### 6. Docs + gates

- `docs/reel-studio-import.md` § Limits: remove "you can only shrink" phrasing (if present),
  document the video-range clamp, the `adjusted` marker, and the deleted-project caveat.
- Full gates: `cargo fmt`, warnings-denied clippy, workspace tests, `npm run test:ui`; macOS
  render-fixture runs pasted into the PR. No golden edits; the 021 human render-golden review
  is untouched by this task.

## Order

1 → 2 → 3 (byte-stability proof before touching the app) → 4 → 5 → 6.

## Honest limits

- Span projects still cannot be exported from Projects unless the flagged export unblock is
  taken (see above) — the task file's acceptance does not require it; the recommendation is to
  include it, the decision is the orchestrator's/John's.
- NULL-duration videos cannot host span item edits (refused with a clear error).
- Deleting and re-importing a project resets its items to catalogue defaults (by design).
- Running Crush's aesthetic analysis over span intervals remains out of scope (TASK-034's open
  question).
