# TASK-021 — implementation plan and progress

Status: **in progress, safety foundation only**. The parent acceptance in `TASK-021.md`
is unchanged. This extends the existing engineering and editorial/DAM blueprints; it does
not replace them. Task 022 stays next, after the render-golden human review.

## Implemented first slice

- Clip exports reject existing destinations, including the original, hard links, symlinks,
  and dangling symlinks. The caller-selected path never receives FFmpeg's `-y` flag.
- FFmpeg writes inside a private temporary directory on the destination filesystem. Existing
  stream-copy/VideoToolbox validation runs there. Success flushes the staged file and publishes
  through an exclusive hard link. A destination created by another writer is never overwritten.
- Normal failure/cancellation removes staging and publishes no partial output. Existing public
  clip export entry points use this protection, including pipeline, CLI and Tauri callers.
- Regression tests cover original aliases, existing exports, a publication race, cancellation,
  failure cleanup, source hashes, and the existing stream-copy/re-encode fixture behavior.
  macOS CI now explicitly runs the FFmpeg fixture suite.

Limits: this is not a recipe renderer. A filesystem without hard-link support fails closed;
there is no overwriting-copy fallback. Process death can leave a hidden staging directory;
durable job recovery below must manage those directories before resumability is claimed.
The existing clip verifier does not constitute the full color/audio/frame golden matrix.

## Remaining implementation, in order

### 1. Durable recipe, source snapshot and job contracts

- Add an owner-scoped migration and typed store APIs for immutable, versioned recipes,
  render jobs/attempts and verified outputs. Keep append-only plan revision identity alongside
  a frozen ordered item snapshot; later plan edits cannot silently change a queued render.
- Validate a versioned recipe schema: photo crop/rotation/grade/output; video in/out,
  crop/grade/sequence/transitions/audio/output. Unknown/unsupported treatment must fail
  explicitly, never silently render untreated media as if the intent were fulfilled.
- Snapshot source kind/ID/hash, selection provenance, recipe/preset version, relevant model
  identities (or an explicit not-used value), and context. Resolve paths under owner scope,
  recheck source hashes at execution, and refuse stale/missing sources.
- Persist queued/running/verifying/done/failed/cancelled state and progress. Restart an
  incomplete attempt from the frozen recipe with tracked staging; never trust a partial
  output merely because its filename exists. Resume must be idempotent.

### 2. Photo derivatives and documented presets

- Use original full-resolution sources, not thumbnails/proxies. Reuse Task 016 capability
  gates for RAW/HEIF/TIFF; report unsupported full decode precisely.
- Apply EXIF orientation once, then explicit recipe rotation/crop/grade. Define crop
  coordinates after orientation and validate bounds and positive final dimensions.
- Provide versioned JPEG, PNG and TIFF presets documenting dimensions/resize policy,
  quality/compression/bit depth, output color space and metadata policy. Perform actual
  profile conversion; assigning a color tag is not conversion. Do not silently reduce
  high-bit-depth RAW/TIFF fidelity or claim unsupported source profiles are color-correct.
- Strip private metadata by default and document any opt-in preservation. Retain source
  provenance in the manifest independently of embedded EXIF/GPS policy.

### 3. Video clips and reels

- Use the bundled LGPL FFmpeg path and existing process-group cancellation. Document MP4
  and MOV preset codec/audio/color/dimension/frame-rate policies and exactness limits.
- Encode boundary-sensitive edits; permit stream copy only when verification establishes
  the documented accuracy. Ordered plan items, photo hold durations, transitions, crops,
  grades, and audio policy must produce the declared sequence, not independent loose clips.
- Apply real HDR/SDR and gamut conversion where supported; reject unsupported conversion
  with a capability error. Preserve source orientation and distinguish tone-mapped output.

### 4. Verified, recoverable publication and UI

- Extend the private staging/no-clobber primitive to photo outputs, reels and manifests.
  Track managed staging paths durably; reconcile interrupted publication without deleting
  unrelated user files or leaving outputs falsely marked verified.
- Every output manifest includes source IDs/hashes, frozen recipe and plan revision,
  relevant model/tool versions, actual command/options, output checksum, and measured
  verification results. Validate manifests on resume and completed-output reuse.
- Expose render/preset/destination actions from Plans, plus job progress, cancellation,
  retry/resume, errors and verified output/manifest locations. Never imply that editing
  a plan already rendered the media. Exporting alone is not learning approval.

### 5. Automated matrix, then John's human hard stop

- Golden fixtures cover JPEG/PNG/TIFF and supported RAW/HEIF; rotated/profiled/wide-gamut
  images; video boundary frames, duration, dimensions, audio, rotation and HDR handling;
  photo/video mixed reels; repeated export, collisions, cancellation, restart and manifests.
- Hash sources before/after every render matrix. Test owner isolation, malformed recipes,
  stale source hashes, unavailable decoder/codec and unsupported filesystems.
- Produce a review packet with source references, recipe JSON, rendered derivatives,
  manifests, visual comparisons and measured checks. Automated tests do not update the
  approved goldens or constitute a visual/color sign-off.
- **Stop for John to review render goldens.** Do not mark Task 021 accepted or proceed to
  importer/release acceptance on the strength of CI alone. Task 018's style proof and
  Task 023's clean-machine acceptance remain separate human gates.

## Verification recorded for the first slice

- `cargo test -p crush-stage-split --test ffmpeg_fixtures`: nine tests pass locally on macOS.
- `cargo clippy --workspace --all-targets -- -D warnings`: passes locally.
- Pipeline ingest/export integration: eight tests pass, one explicit ten-minute release
  smoke remains ignored. The targeted clip-export test passes again with source/existing
  output hash assertions. Format and diff checks pass. PR results are recorded separately
  from the unimplemented render-golden acceptance above.
