# TASK-027: App robustness + honest UI harness
Agent: Codex. Branch: task/27-app-robustness. Depends: 024 (breakdown UI lands first).

Windows-safe: harness is browser-driven; Tauri commands verified by `cargo check -p crush-app` (may need tauri CLI) or deferred to CI if the webview stack cannot build locally.

## Instructions

All line numbers verified against the working tree on 2026-08-28.

1. **F1 — blank-screen startup failure (crates/app/ui/app.js).** `showLibrary()` (app.js:472-477) hides `#boot` at :473 before awaiting `refreshLibrary()` at :476; `refreshLibrary()` (app.js:384-390) has no internal catch, and `renderVideos()` (which shows `#empty-library`, app.js:219) never runs on failure. The rejection reaches the outer `initialize().catch` (app.js:527-529), which writes into the already-hidden boot `<p>`: the user sees an empty Library with no empty state and no error. Fix: wrap the `refreshLibrary()` call inside `showLibrary()` in try/catch — on failure call `showMessage(..., true)` and still call `renderVideos()` so the empty state (or last-known table) renders. Also harden the outer catch to un-hide `#boot` before writing its message.

2. **F2 — ingest slot leak (crates/app/src-tauri/src/lib.rs).** `add_folder` claims the `active_ingest` slot at lib.rs:294-303, then `insert_background(...)?` at lib.rs:304-313 can bail (realistic trigger: poisoned state lock via `lock()`, lib.rs:891-895), leaving the slot set — every future `add_folder`/`reindex_video` then fails with "ingest … is already running" (lib.rs:296-298, 440-442) until restart. `reindex_video` has the same pattern (slot at lib.rs:438-447, `insert_background` at lib.rs:448-458). Fix: on the error path after claiming, roll back — set `*active = None` only if it still holds this `job_id` (same guard as the completion paths at lib.rs:330-337 and 475-482), then return the error.

3. **F3 — manifest key panic (lib.rs:216).** `manifest.files[&check.name].bytes` in `models_status` (lib.rs:208-222) panics if the key is absent. `models::inspect` (crates/core/src/models.rs:101-120) currently derives names from the same manifest map, so this is latent, not reachable today — still convert to `manifest.files.get(&check.name).with_context(...)` (map to `.bytes`; `ModelFile` is not `Copy`, so no `.copied()`).

4. **F4 — record_feedback validation + sync retrain (lib.rs:663-705).** The command is a sync `fn` (lib.rs:664) — Tauri runs it on the main thread — and calls `retrain_style_profile` on every pick click (lib.rs:702). That retrain (crates/search/src/lib.rs:73-137) loads the entire unbounded, append-only `feedback_events` table (`store.feedback_events`, crates/store/src/lib.rs:897-906, no LIMIT) and does one vector load per event (`media_vector`, search lib.rs:93/101 → `vector_for_photo` store lib.rs:598 / `vector_for_shot` store lib.rs:1380). It also accepts any `Option<f64>` rating and never checks that `media_id` exists — and appended bad events are permanent. Fix: (a) validate — for `signal == "rating"` require `value` in 1.0..=5.0; for pick/reject require the fixed ±1.0 values; verify asset existence via `photo_by_id`/`shot_by_id` (store lib.rs:447/1346) and reject unknown ids; (b) make the command `async fn` with `tauri::async_runtime::spawn_blocking`, mirroring `search`/`export_clip` (lib.rs:494-533, 736-756); (c) debounce retrain — set a dirty flag (e.g. `AtomicBool` on `RuntimeState`) in `record_feedback` instead of retraning inline, and retrain inside the existing `spawn_blocking` on the next `search` (or library refresh) when the flag is set. Keep `retrain_style_profile` public and unchanged so tests and `crushctl` stay deterministic; do not change its math.

5. **F5 — list_videos N+1 (lib.rs:348-401).** The per-video `store.jobs(...)` query (lib.rs:352-359; store impl crates/store/src/lib.rs:1725-1744) runs once per video on every 850 ms poll tick (app.js:394). It exists only to compute `last_error` for failed videos. Fix: add one store method (e.g. `failed_job_errors(owner_id) -> Vec<(video_id, error)>` — `SELECT video_id, error … WHERE status='failed' ORDER BY started_at DESC`, first error per video wins) and use it in `list_videos`; drop the per-video loop. `job_status`'s full pipeline snapshot is unchanged.

6. **F6 — vector index reload per search (lib.rs:526).** `runtime.engine.reload(&store)?` runs on every search; `SearchEngine::reload` (search lib.rs:313-317) re-reads all shot and photo vectors from SQLite (`load_all_vectors` store lib.rs:1414, `load_all_photo_vectors` store lib.rs:610). Fix: cache the index and reload only when the store changed — add a cheap generation getter to `Store` (`PRAGMA data_version` works because every command opens its own connection) or compare `store.db_path()` (store lib.rs:362) mtime; keep the generation in `SearchRuntime` and skip `reload` when unchanged. Caching only — no ranking math changes.

7. **F7 — UI small fixes:**
   - app.js:162 — in `onDownloadProgress`'s `done` branch, `await refreshModels()` has no catch (async event-handler rejection is unobserved). Wrap in try/catch and surface via `state.modelFailure` + `renderModels()`.
   - app.js:464 — drag-drop indexes only the first path (`const [path] = event.payload.paths`). Queue all paths sequentially through `addPath` (it already guards `isIngestActive()`), or inform the user that only the first was added.
   - search.js:326 — the detail photo `el.photo.src = fileSrc(d.photoPath)` has no error handler (video does, search.js:391-393). Add an `error` listener on `#detail-photo` with an "Is the drive mounted?" style `showMessage(..., { error: true })`.
   - search.js:587-590 — the rating `change` handler never resets `el.feedbackRating.value` to `""`, so the same rating can't be recorded twice consecutively. Reset the select after the `record_feedback` call resolves (and on failure).

8. **F8 — CSP hardening (crates/app/src-tauri/tauri.conf.json:23).** `"csp": null` today. Set a strict CSP, e.g. `default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' asset: http://asset.localhost https://asset.localhost; media-src 'self' asset: http://asset.localhost https://asset.localhost; connect-src 'self' ipc: http://ipc.localhost`. Verify each exception against real usage before keeping it: `convertFileSrc` feeds result thumbs (search.js:209), the detail photo (search.js:326), and the video element (search.js:350-353), so img/media need the asset protocol hosts (Windows uses `http://asset.localhost`, macOS `asset://`); `ipc:`/`http://ipc.localhost` is the Tauri 2 IPC requirement; `'unsafe-inline'` in style-src covers JS-set progress-fill widths (app.js:118, 277) — drop it by refactoring those fills to classes only if trivial. Document the exact final CSP string in the PR; if a directive breaks thumbnails or the asset protocol on the Mac, narrow it rather than removing the CSP.

9. **F9 — harness rework (crates/app/tests/ui-harness.html:133-232).** The harness hand-duplicates `index.html` and has drifted: "No footage yet" (ui-harness.html:149) vs "No media yet" (index.html:80, body copy differs too); search empty-state copy differs (ui-harness.html:173 vs index.html:132); missing `type="button"` (ui-harness.html:145, 147), `aria-hidden` on brand marks/nav icons/status dot/drop overlay (ui-harness.html:137, 145, 150), `aria-label="Close"` on the doctor close button and `sr-only` column-header spans (ui-harness.html:149, 231). It also has no scripted assertions — first-run retry, cancel click/completion, failed-row expand, photo Library rows, search-error state, and feedback/export flows are manual or unreachable. Fix: delete the duplicated DOM; load `../ui/index.html` in an iframe and inject the mock bridge into the iframe instead. There is no existing runner to reuse (crates/app/tests/ holds only this HTML; scripts/ holds only get-sidecars.sh and publish-models-v1.sh), so add a minimal playwright-core runner (e.g. `scripts/ui-harness.mjs`) driving the system Chrome via channel/executablePath — no browser download — and install the extracted mock bridge (`crates/app/tests/mock-bridge.js`, split out of the current harness) via `page.addInitScript` so it exists in the iframe before app.js/search.js parse. Extend the mock for scripted transitions (e.g. cancel → completion flip) and a photo Library row. Cover at least: first-run retry, ingest cancel completion, failed-row expand, one photo Library row (`Shots` shows "—"), search error state (`#search-error` visible), record feedback + rating reset after record. Keep the harness deterministic: mock clocks/timers (playwright clock) for the 850 ms poll and 5 s messages, no real media (SVG data-URI thumbs stay).

Standing guardrails for every change: no network egress from the UI; build DOM with `textContent`/`createElement` only (no innerHTML); machine scores never clear privacy flags; golden files untouched; no ranking-math changes — TASK-024 owns the breakdown export, and this task only consumes it if already merged, otherwise stub the touchpoint. One task per PR on `task/27-app-robustness`.

## Acceptance

- [ ] A startup `list_videos`/`job_status` failure shows the Library with the empty state and an error message instead of a blank screen (F1).
- [ ] A failed `insert_background` after claiming the ingest slot releases it; ingest and re-index remain usable (add_folder and reindex_video paths) (F2).
- [ ] `models_status` returns a proper error for a missing manifest key instead of panicking (F3).
- [ ] `record_feedback` rejects out-of-range ratings and unknown media ids, runs off the main thread via `spawn_blocking`, and defers retraining to the next search/refresh; `retrain_style_profile` math and tests unchanged (F4).
- [ ] `list_videos` issues one failed-jobs query, not one query per video (F5).
- [ ] Consecutive searches skip the vector reload when the store generation is unchanged and reload when it changes (F6).
- [ ] Model-done failure, multi-file drop, photo load failure, and rating-reselect all behave as specified (F7).
- [ ] `tauri.conf.json` ships a strict CSP with each exception documented and verified against thumbnails, photo/video detail, and IPC (F8).
- [ ] Harness renders the real `../ui/index.html` with no duplicated DOM and the scripted scenarios (first-run retry, cancel completion, failed-row expand, photo row, search error, feedback + rating reset) pass locally via system Chrome (F9).
- [ ] Harness asserts shipped copy (e.g. "No media yet") so drift like F9 cannot recur; privacy and goldens guardrails hold; `cargo check -p crush-app` (or CI) is green.

## Record (merged as PR #24)

Implemented by the agent team 2026-08-29. Startup failure now renders the Library with empty state +
error instead of a blank screen; failed insert_background releases the ingest slot (add_folder and
reindex_video); models_status returns an error for a missing manifest key instead of panicking;
record_feedback validates rating range/pick values/media existence, runs via spawn_blocking, and
defers retraining to the next search through a dirty flag (retrain_style_profile math unchanged);
list_videos batches failed-job errors in one query; the vector index reloads only when the store
data_version changes; UI small fixes (model-done catch, multi-file drop notice, photo load error,
rating select reset); strict CSP shipped and documented in docs/app-csp.md; harness rebuilt as an
iframe loading the real index.html with playwright-core driving system Chrome (no browser download)
and 7/7 scripted scenarios passing locally; CI harness step is continue-on-error until proven on the
runner. Integration note: post-merge semantic collisions with Task 024 (models_status ?-operator in a
non-Result closure, SearchRuntime destructure missing generation) were repaired on main by PR #27.
