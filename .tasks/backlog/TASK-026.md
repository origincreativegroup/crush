# TASK-026: Pipeline ops — photo analyze staleness, cancellable renders, photo job logging

Agent: Codex. Branch: `task/26-pipeline-ops`. Depends: 025 (store helpers).

Windows-safe parts: `cargo check`/`cargo test` for pipeline pure-Rust paths; ffmpeg/sips/macOS-ImageIO behaviors gate on CI (macOS).

Findings verified against commit fe611cd; file:line references are current-state.

## Instructions

1. **F1 — photo re-analysis blowup (crates/pipeline/src/lib.rs:268-318).** `analyze_photos` runs on
   every photo ingest (lib.rs:135), decodes the entire library's thumbnails into memory up front
   (270-287), and has no staleness/model-version gate; the video path has
   `video_assessments_current` (lib.rs:1015-1029). Coordinate naming with the TASK-025 store
   helpers: add a store query (working name `photos_for_analysis(owner_id, model_version)`)
   returning Done photos with no `aesthetic_assessments` row or a row whose `model_version` differs
   from `strong-shot-v1`, ordered like `photos()` (path, id) so sequence order is stable. Backfill
   only missing/stale photos, decoding thumbnails in bounded windows (e.g. 64) instead of the whole
   library; preserve `AnalysisContext` global `index`/`sequence_len` semantics and adjacent-neighbor
   evidence across window seams (one-photo overlap). Add a test proving a second ingest with no
   changes performs zero re-analysis work: in
   `photo_ingest_is_idempotent_and_searchable` (crates/pipeline/tests/ingest_fixtures.rs:66),
   capture the photo assessment's `assessed_at` after the first ingest and assert it is unchanged
   after the second.

2. **F2 — sips hangs (crates/pipeline/src/source.rs:267-273, 473-476, 504-521).** All three
   `/usr/bin/sips` invocations use `Command::output()` with no timeout and no cancellation; the
   ingest token is only checked between files (lib.rs:115-118). Thread the pipeline
   `CancellationToken` into `decode_photo` (signature change) and replace all three call sites with
   a shared spawn + poll (`try_wait`) + kill helper mirroring the `ffmpeg::run_progress` pattern
   (crates/stage-split/src/ffmpeg.rs:715-810; non-unix branch kills the child directly), with a
   120 s per-invocation timeout. These paths are macOS-only; unit-test the helper's timeout and
   cancel behavior with a `sleep` stand-in, gated `#[cfg(target_os = "macos")]` for CI.

3. **F3 — photo job logging violates HANDOFF (lib.rs:115-134).** Photo ingest creates no job
   records, and the failure span (lib.rs:129) carries neither `job_id` nor `stage`; HANDOFF.md
   requires both on every stage span. Give photos a job lifecycle mirroring video jobs: extend
   `Stage` in crates/core/src/job.rs:6-11 (e.g. `photo_ingest`; photo analysis can reuse
   `analyze`). Note the jobs table (crates/store/migrations/0004_strong_shot.sql:31-46) has
   `video_id TEXT NOT NULL`, a stage CHECK constraint, and a composite FK to `videos` with the
   `foreign_keys` pragma enforced (crates/store/src/lib.rs:2049-2060) — so a schema migration
   rebuilding `jobs` is required: add the new stage to the CHECK, make `video_id` nullable and add
   a nullable `photo_id` with the same composite FK pattern as `photos(id, owner_id)`, plus an
   exactly-one-of check. Stuffing photo ids into `video_id` would violate the FK and is not
   acceptable. Spans carry `job_id` + `stage`; photo ingest becomes resumable through the jobs
   table via `fail_running_jobs_as_interrupted` like video stages.

4. **F4 — skip check incomplete (lib.rs:156-177 photo, 404-421 video).** Fidelity-completeness on
   skip verifies only the proxy hash. A missing/truncated thumbnail therefore yields
   "skip: photo already indexed", leaving a Done asset whose thumbnail `analyze_photos` silently
   drops (lib.rs:282-287). Persist a thumbnail hash (photo `PhotoSourceMetadata` currently stores
   only `proxy_sha256`; metadata_json is an acceptable home) and include thumbnail existence+hash
   in the photo fidelity check (156-168); for video, include every shot's `thumb_rel` existence
   (hash if stored) in the video check (404-421). A Done asset with a missing/truncated thumbnail
   must be re-indexed, not skipped.

5. **F5 — extension-only decoder gate (crates/pipeline/src/video_source.rs:28-43).**
   `validate_decoder_policy` gates on extension only, so BRAW/R3D inside an allowed container
   (.mov) bypasses the precise capability reason and dies in ffmpeg with a generic error. Re-run
   the gate in `proxy_policy` (video_source.rs:45-98) against probed `codec_name`/
   `codec_tag_string`/`profile` — ProRes RAW is already covered there (61-68); add codec-level
   BRAW/R3D matching that emits the exact licensing messages from `validate_decoder_policy`.
   Confirm the real ffprobe tag strings on macOS CI against BRAW/R3D fixtures; unit-test with
   synthetic `Probe`s alongside `proprietary_formats_never_masquerade_as_preview_support`
   (video_source.rs:152-165).

6. **F6 — silent skip of unknown extensions (lib.rs:1049-1129).** Files failing
   `is_photo`/`is_video` (source.rs:23-26, video_source.rs:8-10) vanish at discovery with no
   failure record, against docs/dam-feedback-blueprint.md:170 ("Unsupported RAW or acquisition
   formats fail with a precise capability reason"). Maintain a curated known-unsupported extension
   registry (AVIF, JXL, ERF, plus acquisition formats from docs/media-format-support.md; never
   flag arbitrary non-media files such as .txt or .DS_Store) and record each discovery in
   `IngestSummary::errors` (lib.rs:50) with a precise per-format reason, surfaced to the UI like
   other errors. Extend the support-matrix docs/fixtures to cover the registry.

7. **F7 — proxy recipes unpersisted (lib.rs:576-581, 211-225).** Encode settings live only in
   code: the video recipe (1920x1080-constrained scale, h264_videotoolbox, 12M/16M/24M rate
   control, aac 192k, +faststart — crates/stage-split/src/ffmpeg.rs:381-407) and the photo recipe
   (2560 px @ q92 proxy, 960 px @ q85 thumbnail — lib.rs:211-225). Record the recipe (or the
   verbatim ffmpeg/sips command) in `metadata_json` at write time: extend the video metadata_json
   object (lib.rs:576-581, currently decoder/codec_tag/proxy_policy_version) and the photo
   metadata_json (currently only gps/capture/orientation facts, source.rs:322-327) so derivatives
   are auditable and reproducible.

8. **F8 — color flags on edit proxy (ffmpeg.rs:381-407).** The edit-proxy filter chain
   (`scale=...,format=yuv420p`) sets no output `-color_primaries`/`-color_trc`/`-colorspace`, so
   10-bit bt2020/HLG HEVC sources get collapsed color without tonemapping. Decide and implement
   explicitly: pass the probed source color tags through to output flags (probe captures them at
   ffmpeg.rs:1063-1066), or insert an explicit tonemap-filter decision; after encoding, re-probe
   the proxy and record its color tags in the video source metadata_json (the proxy is never
   re-probed today, lib.rs:536-543). Add a fixture assertion (extend the HEVC proxy-path test in
   crates/pipeline/tests/source_fidelity.rs) if feasible on CI; it is macOS/videotoolbox-gated.

9. **F9 — fps==0 / unknown bit depth direct-path (video_source.rs:76-88 +
   ffmpeg.rs:1111-1126).** `probe.fps <= 60.0` passes for 0.0 (fps falls back to 0.0 at
   ffmpeg.rs:1034-1050 when no frame rate is parseable), and `infer_bit_depth` defaults any
   unrecognized pix_fmt to 8-bit (`Some(8)` at ffmpeg.rs:1124) which the gate then reads as
   `bit_depth.unwrap_or(8)` (video_source.rs:77). Treat fps==0/non-finite and unknown bit depth as
   proxy-required: `infer_bit_depth` returns None for unrecognized pix_fmts, and the direct-edit
   gate requires a known positive fps and a known bit depth. Unit tests with synthetic `Probe`s
   (fps 0, unknown pix_fmt) alongside `direct_edit_and_proxy_decisions_are_explicit`
   (video_source.rs:133-149).

10. **F10 — EmbeddedPreview unproducible (crates/store/src/lib.rs:79-83).**
    `PhotoProxyProvenance::EmbeddedPreview` is schema-legal (`photo_proxy_provenance_from_str`
    accepts "embedded_preview", 2427-2434) but nothing produces it — only `DecodedOriginal`
    (source.rs:232) and `FullRender` (source.rs:300) are constructed — so the "never label a
    thumbnail as full decode" rule is enforced only by code absence. Make it structural:
    `validate_photo_source_metadata` (store lib.rs:2591-2616) rejects EmbeddedPreview with a clear
    error, plus a store test asserting the rejection.

Constraints carried through every item: golden files untouched (fixtures/golden); originals
immutable — keep the re-hash before/after pattern (photo lib.rs:255-258, video lib.rs:624-627);
keep verbatim ffmpeg command logging (ffmpeg.rs:812-829); one task per PR on
`task/26-pipeline-ops`.

## Verification

- `cargo check -p crush-pipeline -p crush-stage-split`
- `cargo test -p crush-pipeline` — note: `source_fidelity.rs` compiles only on macOS
  (`#![cfg(target_os = "macos")]`); in `ingest_fixtures.rs` the fixture/smoke tests skip with a
  message when FFmpeg sidecars or models-v1 are missing (ingest_fixtures.rs:190-191, 399-400,
  618-619). The F1 staleness test and F4/F6/F9 unit tests are pure-Rust and run on Windows.
- `cargo test -p crush-store` for the new staleness query (F1) and the EmbeddedPreview rejection
  (F10).
- Workspace clippy with warnings denied and `cargo fmt --check` per HANDOFF.

## Acceptance

- [ ] Second ingest of an unchanged photo library performs zero re-analysis work (F1 test proves
      it); missing/stale `strong-shot-v1` photo assessments are backfilled in bounded windows.
- [ ] sips invocations are cancellable and time out at 120 s; canceling ingest mid-photo kills a
      running sips child instead of hanging on it.
- [ ] Photo ingest and photo analysis create job records; every photo stage span logs `job_id` and
      `stage`; interrupted photo jobs are recovered/resumable through the jobs table.
- [ ] A Done photo or video with a missing/truncated thumbnail is re-indexed rather than skipped.
- [ ] BRAW/R3D probed inside an allowed container fails with the named licensing-specific message.
- [ ] Discovered files with known-unsupported extensions appear in ingest summary errors with a
      precise reason.
- [ ] Video and photo proxy recipes are recorded in metadata_json.
- [ ] Edit proxies carry explicit output color tags (or an explicit tonemap decision) and the
      proxy's color tags are recorded.
- [ ] fps==0 and unknown-bit-depth sources require a proxy (unit-tested).
- [ ] EmbeddedPreview is rejected by store validation, proven by a test.
- [ ] Golden files untouched; source re-hash checks preserved; ffmpeg command lines still logged
      verbatim.
