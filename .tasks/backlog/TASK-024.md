# TASK-024: Source-fidelity truthfulness + ranking breakdown export

Agent: Codex (Windows-safe: cargo check/test for store, stage-aesthetic, search, pipeline compile checks; ffmpeg/sips-dependent tests gate on CI). Branch: `task/24-fidelity-breakdown`. Depends: none.

## Goal

Three HIGH review findings plus related explanation-surface gaps: (1) the macOS ImageIO decode path
claims EXIF orientation is normalized without applying it, so rotated CR3/NEF/HEIC sources produce
sideways proxies labeled normalized; (2) the ICC round-trip assertion never executes because no
fixture carries a profile; (3) search exports a single mysterious score while
`docs/dam-feedback-blueprint.md:92-98` requires the ranking exposed in plain language with general
quality and personal taste displayed separately (lines 161, 166). Line numbers below are verified
against the current tree; re-check if the tree has moved.

## Instructions

1. Orientation truthfulness — `crates/pipeline/src/source.rs`. The image-rs path applies EXIF
   orientation at source.rs:219 (`image.apply_orientation(orientation)`), but
   `decode_with_macos_imageio` decodes the sips-rendered JPEG at source.rs:279-284 without applying
   orientation, while `decoded_photo` hardcodes `orientation_applied: true` (source.rs:334) and
   `metadata_json` claims `orientation_normalized_in_derivatives: true` (source.rs:325). Fix inside
   `decode_with_macos_imageio`: after opening the rendered JPEG, read its EXIF via
   `decoder.exif_metadata()?` and derive the orientation exactly like `decode_with_image_rs`
   (source.rs:206-216: `ExifReader::new().read_raw` → `extract_exif` →
   `Orientation::from_exif(...).unwrap_or(Orientation::NoTransforms)`), preferring the rendered
   JPEG's own tag and falling back to the container EXIF already captured into
   `extracted.orientation` (source.rs:249). Update `extracted.orientation` to the value actually
   applied so `DecodedPhoto.orientation` stays truthful, make the image binding mutable, and call
   `image.apply_orientation(orientation)` before building `DecodeFacts` — a `NoTransforms` no-op
   keeps `orientation_applied: true` accurate on every path. Never write to the container file.
2. Orientation test — `crates/pipeline/tests/source_fidelity.rs` (whole file is macOS-gated,
   `#![cfg(target_os = "macos")]` at line 1). Build a HEIC carrying an EXIF orientation tag by
   converting the existing `orientation-6.jpg` (created by `jpeg_with_orientation`, line 252) with
   `sips -s format heic`. Assert `decode_photo` on it produces upright pixels: dimensions (50, 80),
   `orientation_applied == true`, and sample pixel values matching the image-rs decode of
   `orientation-6.jpg` within a small per-channel tolerance (sips re-render may shift a few codes;
   do not assert byte-identity). The assertion must also pass when sips bakes the rotation and drops
   the tag (orientation `None`/`NoTransforms` with upright pixels), so a double rotation or a
   sideways proxy fails the test either way.
3. ICC fixture — same test file. Every current source (PNG/JPEG/TIFF via `base.save_with_format`,
   lines 50-55; HEIC via sips, lines 58-70) embeds no ICC profile, so `decoded.icc_profile` is
   always `None` and the round-trip assertion at lines 105-115 never runs. Load a known profile from
   `/System/Library/ColorSync/Profiles/sRGB Profile.icc` (skip with a clear message if absent) and
   add two fixtures: a JPEG with its APP2 ICC segment written via
   `image::codecs::jpeg::JpegEncoder::set_icc_profile` (the same API `write_jpeg_derivative` uses,
   source.rs:173-177), and a HEIC via `sips -s iccProfile <profile>`. Assert `decoded.icc_profile`
   is `Some` for both, and extend the lines 105-115 check so these fixtures prove the derivative
   carries the exact profile bytes. Add a mismatch case: encode one derivative with the Display P3
   profile (`/System/Library/ColorSync/Profiles/Display P3.icc`) while the source carries sRGB and
   assert the read-back profile bytes differ — proving the comparison can actually fail.
4. CI wiring — `.github/workflows/ci.yml` runs only `ingest_fixtures` from crush-pipeline on macOS
   (ci.yml:62), so source_fidelity tests never gate today. Add
   `cargo test -p crush-pipeline --test source_fidelity -- --nocapture` to the
   `test-macos-accelerated` job.
5. Ranking breakdown export — `crates/search/src/lib.rs`. `AssetSearchResult` (lib.rs:54-68)
   exposes only the combined `score`; the sum at lib.rs:458-461 (shots) and lib.rs:498-501 (photos)
   is `found.score` (cosine + FTS transcript boost) + `editorial_adjustment` (lib.rs:543-554)
   + `general_aesthetic_adjustment` (lib.rs:556-558) + `personal_style_score.unwrap_or(0.0) * 0.15`.
   Add `#[derive(Debug, Clone, Copy, PartialEq, Serialize)] pub struct ScoreBreakdown` with f32
   fields `semantic`, `transcript_boost`, `editorial`, `general_aesthetic`, `personal_style`, and a
   `breakdown: ScoreBreakdown` field on `AssetSearchResult`; capture each term into a local before
   summing so the math is untouched and `semantic + transcript_boost + editorial +
   general_aesthetic + personal_style == score` (float tolerance). Keep `cosine` and the raw
   `personal_style_score` (−1..1 affinity) as-is. `crushctl search --json`
   (crates/cli/src/main.rs:257) gains the fields for free; optionally print the components on the
   table detail line at main.rs:280-290.
6. UI breakdown — `crates/app/ui/search.js`. The score badge tooltip is raw cosine (search.js:225)
   and the "Style +42 · Strong 89" line is either/or with the transcript
   (`result.transcript_snippet ||`, search.js:240), so most video results hide it. In
   `renderResults` (search.js:191-251): render the transcript snippet and the style/aesthetic line
   as separate elements (never either/or), and add an expandable per-card
   `<details class="result-breakdown">` listing each component in plain language (semantic match,
   transcript match, general quality, your style, editorial context/penalty) with signed ×100
   values, omitting zero/absent components; general quality and personal taste remain separate
   items per `docs/dam-feedback-blueprint.md:161`. Update the badge tooltip to the same summary and
   add matching styles in `crates/app/ui/search.css`.
7. Detail views — `crates/app/src-tauri/src/lib.rs`. Make the private `personal_style_score`
   (crates/search/src/lib.rs:560-576) a `pub fn` that loads the active profile itself, reuse it at
   the existing internal call sites, then add `personal_style_score: Option<f32>` to
   `ShotDetailView` (lib.rs:132-151) and `PhotoDetailView` (lib.rs:153-170) and populate it in
   `shot_detail` (lib.rs:535-601) and `photo_detail` (lib.rs:603-653) with `DEFAULT_OWNER_ID`
   (HANDOFF: owner_id on every owned record). In `renderDetail` (search.js:301-357) append a "your
   style" item to both the photo scores (search.js:319-325) and the video analysis line
   (search.js:334-339) when finite — detail payloads are camelCase
   (`#[serde(rename_all = "camelCase")]`), unlike AssetSearchResult's snake_case.
8. Analyze stage label — `crates/core/src/job.rs:9` defines `Stage::Analyze` (serializes as
   `analyze`), but the stages map in `crates/app/ui/app.js:174-178` only knows split/embed/
   transcribe, so analyze jobs fall back to "Indexing". Add `analyze: ["Analyzing", 70],` between
   embed (56) and transcribe (84).
9. Harness — `crates/app/tests/ui-harness.html` mocks `search` (lines 77-88), `shot_detail`
   (90-97), and `photo_detail` (98-105). Extend those payloads with `breakdown` and
   `personalStyleScore`, then run the search, photo-detail, and video-detail scenarios in a browser
   to confirm the new elements render and existing checks still pass.

## Constraints

- Do not change the ranking math or any constant. The ±0.08 `general_aesthetic_adjustment` term
  (crates/search/src/lib.rs:556-558, `(overall - 0.5) * 0.16`) stays exactly as-is; it only gains
  test coverage. This task is export surface + UI/UX for the breakdown.
- `fixtures/golden/expected_search.json` is untouched. The golden gate drives `SearchEngine::search`
  via `SearchResult` (crates/cli/tests/search_fixtures.rs:130) and asserts shot membership only; no
  golden pins `AssetSearchResult` JSON.
- HANDOFF (docs/HANDOFF.md): owner_id on every owned record; golden tests are correctness; branch
  `task/24-fidelity-breakdown`; one task per PR.
- Windows dev machine: `source_fidelity.rs` is macOS-only and sips/ffmpeg-dependent — do not attempt
  to make those tests pass locally; the macOS CI job gates them (see step 4).

## Acceptance

- [ ] `cargo check -p crush-pipeline -p crush-search` passes on Windows; `cargo test -p crush-search`
      passes; `cargo clippy --workspace --all-targets -- -D warnings` clean.
- [ ] New unit tests pin the term without changing it:
      `general_aesthetic_adjustment_is_bounded_and_centered` asserts overall 1.0 → +0.08, 0.0 →
      −0.08, 0.5 → 0.0, `None` → 0.0, and |adjustment| ≤ 0.08 across 0.0/0.25/0.5/0.75/1.0
      (`cargo test -p crush-search general_aesthetic -- --nocapture`).
- [ ] `equal_cosine_assets_rank_by_general_aesthetic`: two shots with equal cosine (no style
      profile, no annotations, built on the `populated_store` helper plus
      `Store::upsert_aesthetic_assessment`) rank by `overall` 1.0 vs 0.0 and flip when swapped,
      with `breakdown.general_aesthetic` = ±0.08 in `AssetSearchResult`
      (`cargo test -p crush-search equal_cosine -- --nocapture`).
- [ ] macOS CI runs `cargo test -p crush-pipeline --test source_fidelity -- --nocapture` green:
      the oriented-HEIC fixture decodes to upright (50, 80) pixels with `orientation_applied == true`,
      the sRGB-profile JPEG and HEIC round-trip exact ICC bytes into derivatives, and the P3/sRGB
      mismatch case asserts inequality.
- [ ] `crushctl search --json` shows semantic, transcript, editorial, general-aesthetic, and
      personal-style components summing to `score` for real queries.
- [ ] Search cards show the transcript snippet and the style/aesthetic line independently, plus an
      expandable plain-language breakdown; photo and shot detail views show the personal style
      score separately from general quality (verify via `tests/ui-harness.html`; note in the PR
      that the harness is the only UI gate — no automated JS tests run in CI, only
      `cargo check -p crush-app`).
- [ ] Analyze-stage jobs render as "Analyzing" at 70% in the library list instead of "Indexing".
