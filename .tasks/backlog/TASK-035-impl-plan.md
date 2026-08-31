# TASK-035 — implementation plan (render engineering follow-ups)

Status: **planned, not started**. Parent acceptance: `.tasks/backlog/TASK-035.md`. Branch
`task/35-render-followups`, after the 021 merge. **No golden edits; renderer output must stay
byte-stable** — every step below carries a byte-stability note, and step 0 is the proof
harness. All file:line references re-verified against `task/21-render-export` head `823c867`;
where the task file's line numbers have drifted (they have — the 036 refactor moved code),
the current numbers are given.

## Step 0 — byte-stability proof harness (do this first)

Before touching anything, capture on macOS with the bundled sidecars:

- Render the 036 fixture ordered-reel (shots only), one clip, and one photo derivative; record
  output SHA-256s. Re-run each after **every** step below and assert identical output hashes —
  the deterministic-render pattern already used by
  `crates/pipeline/tests/render_jobs.rs:318-359` (`documented_photo_presets_are_deterministic_…`).
- Where a step legitimately changes *manifest* bytes (step 1 changes the `verification`
  tolerance fields), say so explicitly in the PR and the 021 re-review request; output bytes
  are the stability contract, manifests are per-render documents (they embed `created_at`).

## Item 1 — one documented duration-tolerance rule — STILL NEEDED (line refs drifted)

Verified current state (task file said `ffmpeg.rs ~1086`, `render.rs ~481, ~630` — all stale):

- Encoder-side checks in `crates/stage-split/src/ffmpeg.rs`:
  - clip render: `frame_tolerance = 1.0 / fps` (`:812-816`), then
    `verify_clip_render` at `:1869` rejects when
    `(output.duration_s - expected).abs() > frame_tolerance + 0.05` (`:1896`);
  - reel item source: `frame_tolerance = 1.0 / fps` (`:990-994`);
  - export path: `frame_tolerance + 0.05` at `:1363` and inside `copy_is_accurate` at `:1387`.
- Executor re-checks in `crates/pipeline/src/render.rs`:
  - clip: `duration_tolerance_s = (2.0 / fps).max(0.05)` (`:675-677`), enforced `:683-687`;
  - reel: `duration_tolerance_s = 0.05 + items / fps` (`:483-487`).

**The 60 fps AAC-priming defect is real:** at 60 fps the encoder accepts up to
`1/60 + 0.05 ≈ 0.0667 s` while the clip executor rejects beyond `max(2/60, 0.05) = 0.05 s` —
a container can pass `verify_clip_render` and then fail the executor's re-check.

Plan:
1. Define the rule once: **tolerance = `frame_tolerance + 0.05` where
   `frame_tolerance = 1.0 / fps` (fallback 1/30)** — a named function in `stage-split`
   (e.g. `pub fn duration_tolerance_s(fps: f64) -> f64`) with a doc comment stating the rule
   and the AAC-priming motivation; the reel executor keeps its per-item additive term
   (`0.05 + items/fps` is a *sum of per-item frame slacks*, consistent with the rule —
   document it as such).
2. Replace the clip executor's `(2.0/fps).max(0.05)` with the shared function
   (`render.rs:675-677`); leave the encoder checks numerically unchanged (they already follow
   the rule).
3. Test: a synthetic 60 fps fixture with AAC audio (generate in-test with the bundled ffmpeg,
   the 036 pattern from `crates/stage-split/tests/reel_fixtures.rs:319`) renders through the
   durable clip path without the pass-then-fail window; a unit test pins the shared function.
4. **Byte-stability:** output bytes unaffected (tolerance only gates failure); the clip
   manifest's `verification.duration_tolerance_s` value changes from `0.05` to `≈0.0667` for
   60 fps sources — flag in the PR per step 0.

## Item 2 — startup render recovery off the Tauri setup thread — STILL NEEDED

Verified: the recovery runs synchronously inside the `.setup(...)` closure
(`crates/app/src-tauri/src/lib.rs:3641-3661`), and `verified_publication_matches`
(`crates/pipeline/src/render.rs:1749-1761`) full-SHA-256s the published output *and* manifest,
checking size equality only *after* hashing.

Plan:
1. Move the `recover_interrupted_renders` call into
   `tauri::async_runtime::spawn_blocking` inside setup; keep the non-fatal semantics exactly
   (log, never brick launch — the 022 review fix), emit a log line / event when done.
2. Short-circuit in `verified_publication_matches`: stat both paths first; if either is
   missing or `len() != size_bytes` → return `false` without hashing; hash only when size
   matches. (Do not use mtime as evidence — size + hash only, matching the verification
   posture.)
3. Tests: extend `startup_render_recovery_accepts_an_empty_library`
   (`crates/app/src-tauri/src/lib.rs:3820`); add a pipeline test where the published output
   was truncated → short-circuit returns false with no full hash (assert via a large file and
   a sentinel: the test asserts the function's result and that the intact case still
   finalizes).
4. **Byte-stability:** recovery never renders; no output bytes involved.

## Item 3 — reel source-hash memoization + capability-listing cache — STILL NEEDED

Verified: the before-pass hashes every item's source
(`render.rs:1398-1404` inside `resolve_reel_v1_sources`) and the after-pass re-hashes every
snapshot (`render.rs:459-476`); a reel cutting several items from one source video hashes that
file once per item, twice per attempt. `render_reel` probes each item source separately
(`ffmpeg.rs:975-1002`). `require_ffmpeg_component` (`ffmpeg.rs:1155-1177`) spawns a fresh
`ffmpeg -encoders`/`-filters` listing per call — a clip render makes up to 2 + N_filters
listing invocations (`:803`, `:825`, `:827`; reel: `:975`, `:999`, `:1001`).

Plan:
1. Memoize source SHA-256 by resolved path per attempt: a `HashMap<PathBuf, String>` shared by
   the before-pass (build it in `resolve_reel_v1_sources`) and the after-pass (pass it into
   `execute_reel_attempt`); one hash per distinct source per attempt. Keep the *evidence*
   identical: each snapshot still records its `hash_after` (the memoized value) — manifest
   bytes unchanged.
2. One ffprobe per distinct source per reel: cache `Probe` by path inside `render_reel`
   (or resolve probes once in the executor and pass them down); the per-item
   `source_probe` reads (`ffmpeg.rs:980-994`) hit the cache.
3. Cache capability listings per `Runner`: list `-encoders` and `-filters` once per Runner
   lifetime (`RefCell<HashMap<&'static str, String>>` or a `OnceLock` pair) and make
   `require_ffmpeg_component` consult the cache. Runner is per-job today
   (`render.rs:441-445`, `:657-660`), so the cache bounds work per render — that satisfies
   "per Runner" as written.
4. Tests: a reel fixture with two items from one source — assert one hash pass per distinct
   path (instrument via a counting wrapper or assert identical evidence); capability test
   asserts a missing component still fails closed.
5. **Byte-stability:** pure performance; ffmpeg command lines unchanged; prove with the
   step-0 hashes.

## Item 4 — `render_job_set_progress` — PARTIALLY SHRUNK (verified)

The task file's "no full-row JSON deserialization" is stale: the function
(`crates/store/src/lib.rs:3244-3283`) is already two guarded UPDATEs inside one immediate
transaction. What remains:

1. The pre-read `render_job_by_id` (`lib.rs:3138-3155`) selects the whole row (including the
   frozen JSON columns) just to check status + monotonicity. Fold the guards into the
   statements: one UPDATE per table with
   `WHERE … AND status IN ('running','verifying') AND progress <= ?new`, `ensure!(changed ==
   1)` for the same error messages, drop the pre-read. Keep the `progress < 1.0` API check.
2. **Wire real ffmpeg progress — still needed:** the progress callbacks are no-ops today
   (`render.rs:447` reel `|_| {}`, `:661-663` clip `|_| {}`; photo jobs keep their 0.1/0.75 at
   `:814-818`). `run_progress` already parses `-progress pipe:1`
   (`ffmpeg.rs:1465-1480`); thread the callback from `render_clip_with_control` /
   `render_reel_with_control` into a throttled, monotonic
   `store.render_job_set_progress` call (e.g. map `Progress` to `0.1 + 0.65 * fraction`,
   never reaching 1.0 — the store already refuses 1.0 pre-verification, `lib.rs:3251-3254`).
   Keep the final 0.75 write.
3. Tests: store test for the guarded UPDATE (backwards progress refused, wrong status
   refused, same messages); pipeline test asserting a clip job's progress advances beyond
   0.1 during a real fixture render (poll the job row).
4. **Byte-stability:** progress writes touch no render bytes; manifests unchanged.

## Item 5 — preset facts on the enums + `list_render_presets` — STILL NEEDED

Verified drifting copies (the task file said "find them" — here they are):

- `crates/app/ui/plans.js:70-74` — extension + save-dialog filter per preset;
- `crates/app/ui/plans.js:260-261` — photo/clip preset label lists;
- `crates/app/ui/index.html:205-207`, `:261-262`, `:571-573` — three copies of the option
  labels (detail drawer, clip options, Projects photo export);
- `crates/app/src-tauri/src/lib.rs:2000-2013` `clip_preset_spec` and `:2076-2093`
  `photo_preset_spec` — names/extensions/presets;
- media types duplicated three times in the pipeline: `render.rs:555-560` (reel),
  `:716-721` (clip), `:1612-1618` `preset_media_type` (photo);
- the enums themselves carry only partial facts: `ClipOutputPreset`
  (`crates/stage-split/src/ffmpeg.rs:188-204`) has `as_str` + `muxer`;
  `PhotoOutputPreset` (`crates/pipeline/src/source.rs:131-140`) has `as_str` only.
- No `list_render_presets` command exists (verified: no match anywhere in the workspace).

Plan:
1. Put the facts on the enums: `ClipOutputPreset` gains `extension()`, `media_type()`,
   `label()` (in `stage-split`, next to `as_str`/`muxer`); `PhotoOutputPreset` gains the same
   (in `pipeline/source.rs`). Replace the three media-type matches in `render.rs` and the
   extension tables (`source.rs:1336` area) with the enum methods. The preset *ids* strings
   (`mp4-h264-sdr-v1`, …) are frozen contract values — unchanged.
2. New Tauri command `list_render_presets` returning `{photo: […], clip: […]}` with
   id/label/extension/media-type per preset, built from the enums (register in the
   `generate_handler!` list — rebase note from HANDOFF applies: that list is conflict-prone).
3. UI: `plans.js` builds the preset `<option>`s and the extension/filter map from the command
   (one fetch at panel load, cached); `index.html`'s three static option blocks become
   populated selects. Delete the drifting copies.
4. Tests: app test asserting the command's facts match the enums (the existing
   `clip_preset_spec` extension assertions at `lib.rs:3865-3877` move to the new source of
   truth); harness scenario asserting the Projects export options render from the command.
5. **Byte-stability:** labels and UI only; recipe `schema_json` preset strings and all ffmpeg
   args unchanged — prove with the step-0 hashes.

## Item 6 — gates

Full gates: `cargo fmt`, warnings-denied clippy, workspace tests, `npm run test:ui`; macOS
fixture renders with before/after output hashes pasted in the PR. No golden edits. The 021
human render-golden review is untouched; if step 1's manifest tolerance-field change needs
mention in the re-review request, add it there — do not silently let a manifest diff surprise
the reviewer.

## Order

0 → 1 → 2 → 3 → 4 → 5 → 6 (step 0 re-run after each of 1-5).

## Honest limits

- Step 1 changes the clip manifest's `duration_tolerance_s` value for ≥34 fps sources
  (0.05 → frame-slack + 0.05); output bytes are unaffected. Flagged, not hidden.
- Capability caching is per-Runner (per render job), not process-wide — a process-wide cache
  would outlive ffmpeg bundle swaps; per-Runner matches the task text and is safer.
- Real ffmpeg progress is throttled and monotonic; it is still an estimate (out_time vs
  wall clock), not a frame count — the manifest's verification facts remain the contract.
