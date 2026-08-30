# TASK-022 — implementation plan and progress

Status: **in progress** (started 2026-08-30 after John ordered Task 022 next). The parent acceptance in
`TASK-022.md` is unchanged. Task 021's render-golden human review is still a separate open gate;
this task does not mark 021 accepted, and imported recipes execute only through 021's frozen-job path.

## What Reel Studio actually stores (reconnaissance, `/Users/origin/GitHub/reel-studio-main`)

- `clips.db` (SQLite): `source_clips(clip_id, source_file, duration, resolution, fps, size_bytes,
  exhibit, theme, has_audio, …)` and `segments(segment_id, clip_id, tc_in, tc_out, description,
  shot_type, camera_move, subjects, action, tags, quality 1–5, standout, faces_visible,
  nametags_visible, blur_required, usable, used_in "a,b", library_file, thumb, preview, notes,
  crop_x, vertical_file)` plus an FTS5 index. Nothing of the real database or media may enter the repo.
- Recipes: JSON `{ "reel": { theme, vibe, music, target_seconds, beat_snap, format, music_volume,
  watermark, cover{id,time}, sequence[{id, in, out, crop_x, crop_kf[{t,x}], caption, cap_pos,
  transition, speed, motion, clip_volume, grade{b,c,s,t,h,v,sh,hl}}], crops{id: x} } }`.
  `in`/`out`/`t`/`cover.time` are **seconds within the library clip**, not the original.
- **Timing basis caveat.** 4K library clips were cut by `dev_cut4.py` with `-ss tc_in -i src -c copy
  -avoid_negative_ts make_zero`: a keyframe-aligned stream copy. The library clip therefore starts at
  the keyframe at or before `tc_in`, so library-relative seconds can be offset from
  `tc_in + in` by up to one GOP. 1080p browse copies (`cut_library.py`) are re-encoded and exact.
  The importer must not pretend either basis is frame-exact: every imported span records
  `boundary_basis` (`catalogue_tc` = `tc_in + offset`; `library_probe` when the library file is
  available and its duration/first-PTS lets us measure the pre-roll) and a `boundary_tolerance_s`.
  Dry-run reports the basis per segment; the render path already verifies duration within frame
  tolerance and refuses stale sources.

## Schema v11 (`0011_reel_studio_import.sql`)

1. `plan_items.origin` gains `'historical'` and `'imported'` (table rebuild to widen the CHECK;
   `profile_version` stays NULL for both; new `provenance_json` column carries
   `{source, external_id, boundary_basis, boundary_tolerance_s, imported_at, import_id}`).
   `PlanOrigin` gains `Historical`/`Imported`; the Projects pill labels them "Historical · Reel Studio"
   / "Imported", never General or Preference-assisted.
2. `manual_spans` (owner-scoped, first-class imported/manual video spans):
   `id, owner_id, video_id, external_id (segment_id), source, start_s, end_s, boundary_basis,
   boundary_tolerance_s, library_relative_offset_s, imported_at, import_id`, UNIQUE(owner_id, source,
   external_id). Spans reference `videos`, not `shots`, so `resplit`/re-index cannot delete them
   (shots are rebuilt; spans are not). Boundaries must lie inside the video duration.
   `plan_items.media_kind` gains `'span'` so a plan can sequence a manual span directly; the existing
   shot-boundary triggers are extended to clamp spans to their own row.
3. `catalogue_imports` ledger: `id, owner_id, source ('reel_studio'), catalogue_path, catalogue_sha256,
   recipe_paths_json, started_at, finished_at, mode ('dry_run'|'apply'), report_json`. Re-runs look
   up prior applied rows by (owner, source, external_id) and skip/refresh without duplicating.
4. `editorial_annotations` are keyed by media; imported rows use media_kind `'span'` for segments
   and are written with `source='reel_studio'` inside `notes`-adjacent `provenance_json` (new column)
   so the UI can show "catalogue" evidence separately from the user's own Crush annotations.

## Importer (`crates/pipeline/src/reel_studio_import.rs`)

- Input: `--catalogue clips.db`, `--originals <dir>...` (where `source_clips.source_file` resolves),
  optional `--library <dir>` (library clips for probe-based boundary basis), `--recipe <json>...`,
  `--owner`, `--dry-run` (default) / `--apply`, `--context <key>`.
- Match `source_clips` → Crush `videos` by file name under `--originals` and then by SHA-256 when
  the file is readable; unmatched sources are reported as `missing_source` and their segments are
  skipped in apply mode (never created as dangling spans).
- Each segment → `manual_spans` + `editorial_annotations` (quality/standout/usable/description/
  subjects/action/tags/safety flags/crop_x/notes). Segment FTS content is indexed through the existing
  transcript/annotation search path for `search`.
- `used_in` → feedback `publish` events with `context_json {source:"reel_studio", import_id,
  external_id, used_in}`; idempotent by checking existing events with the same source+external_id+value.
  `quality`/`standout` are annotation evidence only — **not** `pick`/`rating` feedback unless the user
  confirms in a later explicit step (acceptance: discovery never trains the personal model).
- Each recipe → `render_recipes` row (kind `reel`, schema v2 via `parse_frozen_reel_recipe_v2`) with
  provenance `{origin:"historical", source:"reel_studio", external_id:<recipe file name>}` and a
  `plans` row + `plan_items` (origin `historical`, `provenance_json` carrying the span basis) + one
  `plan_revisions` snapshot. Segment spans resolve through the imported `manual_spans` table so
  `resolve_reel_recipe` gets exact `SegmentSourceSpan`s. Unsupported/unknown recipe fields fail the
  recipe in dry-run errors and are not written.
- `--dry-run` report (JSON + table): sources matched/missing, segments new/updated/unchanged,
  duplicates (same external_id seen twice, same span already imported from another catalogue),
  unsupported data (unknown transitions/motion/vibe/format, out-of-range grades, recipes referencing
  unknown segments), planned writes per table, and boundary-basis summary.
- Previous-work reference sets are **not** created; the report lists finished projects (recipes with
  `used_in`) as "eligible" and points to the existing explicit `reference_sets` confirmation flow.

## CLI and app

- `crushctl import reel-studio …` with `--dry-run/--apply/--json`.
- Tauri: `import_reel_studio_dry_run` / `import_reel_studio_apply` commands behind a Library →
  "Import Reel Studio catalogue…" action that shows the dry-run report and requires a second click
  to apply. Projects pills for `historical`/`imported`.

## Tests (no private data)

- `fixtures/reel-studio/`: a synthetic `clips.db` built from Reel Studio's `schema.sql` with two
  invented segments over the existing `fixtures/clips/*.mp4`, plus `example_recipe.json` adapted to
  those ids. Golden dry-run report JSON.
- Store: migration v11, span survives `resplit`, plan origin round trip, ledger idempotency,
  annotation/feedback idempotency, unsupported provenance combos rejected.
- Pipeline: dry-run report golden; apply then re-apply produces zero new writes; imported recipe
  queues and renders through the existing durable reel path on macOS (fixture clips); keyframe
  pre-roll case with a stream-copied fixture reports `boundary_basis=library_probe` and tolerance.

## Order

1. Migration + store types/APIs + tests.
2. Importer core + dry-run report + CLI.
3. Recipe → plan/recipe rows + render round trip.
4. Tauri commands + Library UI + harness scenario + Projects pills.
5. Docs: `docs/reel-studio-import.md`, HANDOFF, TASKS.
