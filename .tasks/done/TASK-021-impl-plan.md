# TASK-021 — implementation plan and progress

Status: **in progress, photo/clip and ordered clip-reel renderers plus durable recovery and Projects export implemented**. The parent acceptance in `TASK-021.md`
is unchanged. This extends the existing engineering and editorial/DAM blueprints; it does
not replace them. Task 022 stays next, after the render-golden human review.

## User-review UX requirements (2026-08-29)

These are release acceptance requirements, not optional follow-up polish:

- Replace the abstract **Plans** presentation with a clear **Projects / reel editor** workflow:
  create a project, find or add selects, arrange the sequence, preview the actual in/out edits,
  choose an export preset and destination, then render. The UI must always distinguish saved edit
  intent from rendered media.
- Make reel playback feel continuous and controllable. At minimum, provide an obvious play/pause
  control, scrubber/time readout, in/out preview, loop state and previous/next sequence navigation;
  playback must remain inside the selected boundaries and keyboard shortcuts must have visible
  equivalents.
- Reduce Review filter and dropdown clutter through progressive disclosure. Keep the common media,
  decision and search controls visible; move collection, version-stack, privacy and saved-search
  controls behind a clearly labeled secondary surface; show active filters as removable summaries
  and make reset behavior obvious.
- Make the launch/Search surface a media-first DAM browser rather than an empty query canvas.
  Indexed photos and video shots must be visible before a query, simple kind filters must not expose
  internal schema terms, semantic search must refine the same workspace, and opening the inspector
  must reflow rather than cover the candidate grid. The target interaction quality is a professional
  Bridge/Photos-style creative workspace, not a diagnostic search form.
- Rename the user-facing **Style** area to **Preferences** (with “creative taste” explanatory copy).
  “Style” is reserved for visual treatment such as filters and color grading, and the learning
  surface must not imply that it edits media appearance.
- Add deterministic browser-harness coverage for the renamed navigation, progressive filters and
  boundary-safe reel playback/editor interactions. The Task 023 clean-machine test must exercise
  the same natural path without requiring knowledge of internal terms such as plan, recipe or
  context key.

## Cross-platform architecture requirements (2026-08-29)

Task 021 remains the current Mac/product milestone, including its human render-golden stop, while
introducing the portable boundaries in `docs/platform-architecture.md`:

- Recipes and presets describe intended media results, never `videotoolbox`, CUDA, NVENC, CoreML,
  Metal or another platform backend. Manifests record the actual provider/encoder and fallback.
- CPU correctness is mandatory. macOS uses CoreML/Metal and VideoToolbox where validated; Windows
  may later use optional CUDA/DirectML and NVENC while retaining CPU/software fallbacks.
- PyTorch is a development/training/export tool. Shipped model identity is a validated ONNX
  artifact and installing Crush never requires Python, PyTorch, CUDA Toolkit or compiler tools.
- Source decode, media probe/render, process supervision and exclusive publication get narrow
  platform-neutral contracts. ImageIO and Unix process groups remain macOS adapters, not recipe
  semantics.
- Goldens assert versioned output properties and tolerances rather than incidental bytes from one
  hardware encoder. Source hashes, frozen intent, provenance and no-clobber behavior remain exact.

## Task 022 compatibility discovered from Reel Studio

The Task 021 schema/renderer must leave an honest path for the real Reel Studio recipe contract:

- global theme/vibe/music/target length/beat snap/aspect/music volume/watermark/cover;
- ordered per-item relative in/out, static and keyframed crop, caption/position, transition,
  speed, motion, natural-audio volume and grade controls; and
- exact conversion from segment-relative timing to original-source timing.

Unsupported treatments must remain explicit capability errors, but Task 021 cannot pass its final
gate while the documented fields needed by Task 022 are silently discarded. Task 022 must also add
first-class imported/manual source spans because a historical segment can cross auto scene cuts,
and an honest historical/imported provenance type rather than mislabeling prior human choices as
general or personalized. Those importer migrations stay in Task 022; this task keeps frozen recipe
and source contracts capable of carrying them.

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

## Implemented durable store slice

- Schema v10 stores append-only recipe versions, immutable owner-scoped frozen job inputs,
  lifecycle attempts, verified outputs and separately checksummed manifests.
- Queueing freezes a portable source snapshot, explicit model identities/`not_used` values,
  recipe identity/schema and an optional append-only plan revision. Reel recipes require a plan
  revision; photo/clip recipes reject one.
- Strict schema v1 validation accepts only documented crop/rotation/basic-grade/audio/cut/preset
  values and rejects unknown fields or treatments. Advanced reel semantics listed above require a
  later schema version before Task 021 can pass; they are not silently ignored.
- State transitions enforce queued -> running -> verifying -> done, with failed/cancelled attempts
  safely retryable under a new attempt number. Progress cannot move backward or reach 100% before
  verification. Terminal attempts and frozen job inputs are immutable at the database layer.
- Store integration tests cover migration, owner isolation, immutable inputs, portable snapshots,
  unsupported intent, progress/state guards, retry/cancel and verified output round trips.

## Remaining implementation, in order

### 1. Durable recipe, source snapshot and job contracts — foundation implemented

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

The foundation and photo execution path in this section are complete. Video clips and reels will
reuse the same source binding, state machine, manifest and recovery protocol.

Implemented in the second slice:

- Reel recipe schema v2 preserves Reel Studio's exact `sequence[].id`, `cover: {id,time}` and
  `crops` keys plus every documented global/item treatment. Historical and imported origins are
  distinct from general/personal provenance. Unknown fields and incoherent cross-references fail.
- Queueing now binds every source to an owner-scoped photo/video/shot row and exact current path
  and stored hash. Execution resolves the current owner-scoped row again, permitting a legitimate
  post-queue relink only when content identity is unchanged, and hashes source bytes before and
  after rendering.
- The pipeline executes frozen photo jobs through running/verifying/done, creates owner/job/attempt
  marked staging on the destination filesystem, publishes output plus manifest without overwrite,
  and finalizes or safely fails interrupted attempts at app startup. Complete verifying
  publications are recovered idempotently; unrecognized staging is preserved.

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

Implemented in the second slice. [`docs/render-presets.md`](../../docs/render-presets.md) is the
versioned contract. The CPU baseline performs actual ICC/CICP conversion through pinned pure-Rust
`moxcms`, emits deterministic sRGB profiles, applies crop/rotation/basic grade, and produces
exclusive-create JPEG/PNG/TIFF outputs. Unsupported high depth, unconvertible named color spaces,
and transparent JPEG intent fail explicitly. Pipeline/store/app tests cover successful publication,
source immutability, stale hashes, destination/manifest collisions, cancellation before start,
owned-marker recovery, preservation of unknown directories, and finalization after a simulated
post-publication crash.

### 3. Video clips and reels

- Use the bundled LGPL FFmpeg path and existing process-group cancellation. Document MP4
  and MOV preset codec/audio/color/dimension/frame-rate policies and exactness limits.
- Encode boundary-sensitive edits; permit stream copy only when verification establishes
  the documented accuracy. Ordered plan items, photo hold durations, transitions, crops,
  grades, and audio policy must produce the declared sequence, not independent loose clips.
- Apply real HDR/SDR and gamut conversion where supported; reject unsupported conversion
  with a capability error. Preserve source orientation and distinguish tone-mapped output.

Clip rendering is implemented in the third slice. The backend-neutral v1 request always encodes
through the bundled FFmpeg path, supports exact in/out, displayed-space normalized crop, basic
grade, cut, source/mute audio and MP4/MOV output, and returns actual command/backend plus measured
probe facts. Durable pipeline jobs recheck source bytes, verify duration within frame tolerance,
dimensions/audio/codec/color, write the manifest and publish through the common recovery protocol.
HDR, wide-gamut, full-range, unknown/high-depth sources and odd uncropped dimensions currently fail
explicitly. Reel parsing and exact source-span resolution are implemented, including preservation
of segment-relative crop-keyframe timing. The first durable ordered-reel executor now renders a
frozen project revision containing video shots with absolute source boundaries, zero-duration cuts,
no framing intent, supported basic grade, and uniform source audio or mute. It encodes every
frame-sensitive item and stream-copies only the verified same-topology final concat. Photos, fixed
formats, music, captions, watermarks, covers, motion, speed changes, crop keyframes, non-cut
transitions, mixed/fractional audio, and extended Reel Studio grade controls fail explicitly.
Reel Studio schema v2 remains parseable/resolvable but is not executable until those frozen asset
and treatment contracts exist; it is never downgraded to v1.

### 4. Verified, recoverable publication and UI

- Extend the private staging/no-clobber primitive to photo outputs, reels and manifests.
  Track managed staging paths durably; reconcile interrupted publication without deleting
  unrelated user files or leaving outputs falsely marked verified.
- Every output manifest includes source IDs/hashes, frozen recipe and plan revision,
  relevant model/tool versions, actual command/options, output checksum, and measured
  verification results. Validate manifests on resume and completed-output reuse.
- Expose render/preset/destination actions from Projects, plus job progress, cancellation,
  retry/resume, errors and verified output/manifest locations. Never imply that editing
  a plan already rendered the media. Exporting alone is not learning approval.

The selected-photo path is now exposed beside the Projects preview with plain JPEG/PNG/TIFF labels,
a native save destination, explicit Render action, busy/error/success states, Finder actions and
progressively disclosed verification. The app creates the frozen owner-scoped job and requires both
verified files before reporting success. It refuses unsafe photos and project framing/color edits
that the current photo UI cannot reproduce; it never silently drops those edits. The same compact
surface exports a selected shot to MP4/MOV with exact saved boundaries, supported basic treatment,
and source/muted audio. Saved pacing, scalar crop, or unknown treatment is an explicit capability
error. Projects now adds a fourth whole-reel export step for ordered clip-only sequences with
MP4/MOV, source/muted audio, a native destination, verified result/manifest, active cancellation
and direct retry after failure. A project containing photos is blocked with a precise message
because photo holds still need a versioned duration and framing contract; those photos remain
individually exportable. Same-job retry/resume history and the broader mixed-media treatment set
remain before final acceptance.

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
- Initial store slice: 35 tests passed, including the schema-v10 render contract, immutable
  frozen inputs, owner isolation, unsupported-treatment rejection, retry/cancel and verified
  output/manifest state.
- Second-slice verification: `cargo test -p crush-store` passes 37 tests; pipeline library tests
  pass 20 tests; durable render integration passes 6 tests; app library tests pass 3 tests; targeted
  store/pipeline clippy passes with warnings denied. These tests do not constitute the required
  visual render-golden approval.
- Third/fourth-slice verification: stage-split passes 22 library, 12 FFmpeg fixture, and two ordered
  reel fixture tests; pipeline passes 28 library and nine durable integration tests (including real
  frozen clip and two-item reel renders on macOS); store passes 39 tests; app tests pass five and all
  17 browser scenarios pass, including whole-project export. Workspace tests and Clippy with
  warnings denied pass. The bounded human review
  packet, broader color/orientation/audio matrix, and John's visual approval are still outstanding.
