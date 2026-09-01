# TASK-034 — Catalogue unification: span text in search, Review, and the confirmation flow

Implements `.tasks/backlog/TASK-034.md` (reframed) per the architect's plan in
`.tasks/backlog/TASK-034-impl-plan.md`, on top of TASK-037 (step 1, #46). Branch
`task/34-catalogue-unification` off `main` @ `5aeb231`. Schema v13 (037 took v12, as planned —
one migration each, never shared). The plan's flagged open question — aesthetic analysis over
span intervals — stayed OUT per John's planning-round default: catalogue text only.

## The schema decision (v13, recorded in the migration header)

Span confirmation enters as **span-keyed `reference_set_items`**; `feedback_events` stays
photo/shot. Why (now the header of `0013_span_reference_evidence.sql`): span evidence becomes
preference evidence only through the explicit confirmation, and confirmation must be reversible —
`feedback_events` is append-only (0005) and cannot be the vehicle. Confirmed imported evidence is
a named previous-work reference set whose items are the spans themselves, so the evidence keeps
its true identity and provenance instead of being mapped onto whatever shots overlap the interval
today (a mapping that fabricates evidence location and evaporates on resplit). The existing
confirm/disable/delete machinery — transactional profile invalidation included (TASK-032) — is the
whole withdrawal path; no new withdrawal code. Direct span pick/rate signals are deferred with the
reason recorded, because the trainer can only consume media with vectors.

## Acceptance

- [x] **Catalogue text joins search.** `manual_spans_fts` (FTS5, external content on
      `manual_spans` rowid) indexes description/subjects/action/tags/shot_type/camera_move
      (`migrations/0013_span_reference_evidence.sql:98`), kept in sync by the FTS5-documented
      external-content triggers (`:116-141` — insert / update-of-text-columns / delete with the
      `'delete'` command), which also cover the videos → manual_spans `ON DELETE CASCADE` where no
      Rust code runs, plus a migration-time backfill for v11/v12 imports (`:110`). Store read:
      `span_text_hits` (`crates/store/src/lib.rs:5059`), bm25-ordered with video context. Search:
      `search_assets_in_context` appends span results AFTER the semantic ranking as a distinct
      kind (`crates/search/src/lib.rs:432`, `span_text_results` `:1148`): `asset_type: "span"`,
      score/cosine 0.0 (no comparable score exists — fabricating one would be dishonest),
      `score_breakdown.text_match_only: true` (new `ScoreBreakdown` field, `:164`), matched
      catalogue text in `catalogue_snippet`, verbatim `provenance` (source/external_id/import_id/
      imported_at, `:128`), `thumb_path: None` — the honest placeholder rule holds. `selects_candidates`
      keeps spans out of the brief-driven plan candidate ordering (plan candidates must come from
      the composed ranker). CLI `crushctl search` prints span rows with an explicit
      `text-match-only` line and the provenance (`crates/cli/src/main.rs`). App `search` passes the
      new fields through; the UI renders the provenance pill, the "Catalogue text match" line, the
      no-preview state, and a Clips kind filter (`crates/app/ui/search.js`, `search.css`,
      `index.html`).
- [x] **Review filtering (third browse branch).** `browse_assets` gains a manual_spans JOIN videos
      UNION branch (`crates/store/src/lib.rs:4009`); the `MediaKind::Span` bail is gone. The span
      branch aliases `manual_spans` as `a` so the shared evidence clauses read the span row's own
      columns; collection/stack/context clauses can never match (those tables CHECK photo/shot) —
      the same alignment trick the pre-existing comment describes; the feedback arm maps to
      **confirmed reference-set membership** (not feedback events) per the v13 decision.
      `LibraryAsset`/`LibraryAssetView` carry `source`/`external_id`/`import_id`/`imported_at`.
      UI: kind filter gains "Imported clips" (`index.html`), tiles show a CLIP badge, provenance
      pill, interval, honest no-thumbnail state, and no batch checkbox (spans are evidence, not
      pick/reject/rating targets) (`crates/app/ui/library.js`); the pairwise compare pool excludes
      spans and says why when that empties the pool (`crates/app/ui/review.js`); a read-only span
      detail drawer shows the catalogue evidence + provenance + boundary note and plays the SOURCE
      video at the span interval (`span_detail` command, `crates/app/src-tauri/src/lib.rs:1089`;
      rendering in `crates/app/ui/search.js`).
- [x] **Preferences confirmation flow.** Store read `imported_evidence_spans`
      (`crates/store/src/lib.rs:8610`) derives the awaiting population from the span rows
      themselves (import lineage or quality/standout/used_in evidence — never the stale dry-run
      report) with real reference-set membership and confirmed state. Commands
      `imported_evidence_list` / `imported_evidence_confirm` (`crates/app/src-tauri/src/lib.rs:1629`,
      `:1665`): the confirm creates-or-extends a named previous-work set ("Reel Studio · imported
      evidence" by default) with span items, **status unconfirmed** — the second step is the
      ordinary `reference_set_confirm`, so the lifecycle (confirm / disable / delete with
      transactional profile invalidation) is one reversible code path. Provenance is retained on
      the derived record via the set name + description (sources, import ids, the honest inertness
      sentence) and each item's span id. Skip is local UI state only (localStorage), nothing written
      to the library, so re-imports can neither resurrect nor revoke it. Per-item and bulk
      Confirm/Skip in the new "Imported evidence" Preferences section (`crates/app/ui/style.js`,
      `index.html`). Honest inertness copy is part of the section and of every confirmation
      message: confirmed clips "do not train recommendations until clip analysis lands" — no
      surface claims "learned" (the learned wording still follows the recorded verdict conditions
      from #43/docs/style-proof-review.md, untouched). `record_feedback` now refuses spans with
      that message instead of tripping the photo/shot CHECK.
- [x] **Re-import after confirmation.** Store test
      `span_reference_evidence_admits_spans_survives_upsert_and_cleans_on_delete` proves the
      upsert path (idempotence key `(owner, source, external_id)`, stable span ids) leaves a
      confirmed set and its items untouched, and that span deletion (never the importer) cleans
      items via the new `span_reference_cleanup` trigger. Pipeline test
      `confirmed_span_evidence_survives_re_import_without_duplication_or_revocation`
      (`crates/pipeline/tests/reel_studio_import.rs`) proves it end to end: apply → confirm →
      re-apply identical catalogue → `unchanged`, no duplicate rows, set/items unchanged; then a
      CHANGED catalogue (one segment's evidence updated, one segment removed) → the refreshed span
      keeps its id and its confirmed item, and the removed segment's span row still exists
      (the importer never deletes) so its confirmed evidence survives — removal stays a human
      decision. Recorded as intended behavior in `docs/reel-studio-import.md`.
- [x] **Schema v13 upgrade from v12 with data.**
      `schema_v12_spans_upgrade_to_span_reference_and_fts_evidence`
      (`crates/store/tests/store_roundtrip.rs:6244`) builds a real v12 database (migrations 1–12),
      seeds video/span/photo/confirmed-set rows, opens it through `Store::open` → v13: rows intact,
      FTS backfilled (the pre-upgrade description is findable), photo item preserved through the
      table rebuild, span items now admitted, and `reference_set_confirmed_items` reads the span
      beside the photo. FTS sync covered by
      `manual_span_fts_stays_in_sync_with_span_writes` (upsert/refresh text/delete/cascade/owner
      isolation); browse branch by `browse_assets_returns_imported_spans_with_catalogue_filters`;
      evidence population by `imported_evidence_spans_lists_evidence_and_confirmation_state`.
- [x] **Browser-harness scenarios.** `search-span-text` (span result kind, pill, no fabricated
      thumbnail, catalogue snippet, read-only drawer, editorial machinery hidden, evidence
      add-to-set reachable), `review-spans` (Review tiles, kind filter reaching `library_browse`,
      no batch checkbox, compare-pool exclusion), `preferences-span-evidence` (honest copy,
      per-item confirm → unconfirmed set → set confirm → Confirmed state, skip stays local,
      idempotent bulk confirm, disable/delete withdrawal reflected in the evidence rows). All
      existing scenarios still pass (the DAM browser now includes imported clips; mock browse
      data extended accordingly).
- [x] **Store/pipeline tests for each piece.** Five new store roundtrips, two new search tests
      (`span_catalogue_text_surfaces_as_text_match_only_results` — the plan's "span surfaces where
      no shot does" case, plus provenance/breakdown honesty and the selects exclusion;
      `confirmed_span_reference_sets_are_inert_for_the_trainer` — confirmed span items read back,
      trainer skips them without crashing and without counting samples), one new pipeline test.

## Gates

`cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
`cargo test --workspace` (no golden edits; no render-path code touched — byte-stability trivially
preserved), `npm run test:ui` (39 scenarios, all green).

## Honest limits

- Span search results are text-match only (bm25); they do not join the cosine ranking until span
  vectors exist (the open question John parked).
- Confirmed span evidence does not train the current preference model — disclosed in the UI, never
  implied otherwise.
- Spans cannot enter collections/stacks; those filters are honest no-ops for spans. The Review
  editorial filter maps to confirmed-evidence membership for spans (documented at the clause).
- The pairwise compare dialog excludes spans (needs compared-media semantics + vectors).
- `feedback_events` stays photo/shot in v13; direct span pick/rate signals are deferred with the
  reason in the migration header.
