# Crush DAM + Feedback Blueprint

## Product direction

Crush is a private, local-first digital asset manager for photos and video. It finds media by
meaning, helps a user make editorial selections, and learns the user's visual taste from the
decisions they make. The goal is not merely to identify who or what appears in a frame. The goal
is to rank the frames, photos, and clips the way that particular user would rank them.

Reel Studio is the editorial starting point. Its `quality`, `standout`, `usable`, privacy flags,
descriptions, tags, crops, grades, `used_in`, and final reel recipes are valuable training signals.
Its hard-coded paths, generated monolithic HTML, and archive-specific scripts are reference
material rather than the production runtime. Crush keeps the Rust/Tauri, SQLite, bundled ffmpeg,
CLIP, and on-device privacy architecture.

## What “good” means

Quality is multi-dimensional and should stay inspectable:

1. **Technical quality** — focus, motion blur, exposure, clipped highlights/shadows, noise,
   resolution, compression, and duplicate/near-duplicate status.
2. **Composition and design** — visual hierarchy, balance, subject placement, negative space,
   leading lines, symmetry, color harmony, contrast, figure/ground separation, and crop potential.
3. **Moment and storytelling** — expression, gesture, action, interaction, novelty, emotional
   clarity, and usefulness within a sequence.
4. **User taste** — the user's recurring preferences for framing, palette, distance, energy,
   subject matter, pacing, crops, grades, and acceptable technical trade-offs.
5. **Context and safety** — campaign/collection fit, repetition, prior usage, privacy flags, and
   whether a file is actually publishable.

These dimensions must not collapse into face recognition. People and objects remain useful
semantic search attributes, but identity is not the definition of style or quality.

## Feedback signals

The feedback store is append-only. Explicit signals are strongest; reversible workflow actions are
useful implicit evidence and remain distinguishable from explicit opinion.

| Signal | Meaning | Default strength |
|---|---|---:|
| pairwise preference | A is preferred to B in the same context | strongest |
| pick / reject | explicit editorial decision | strong |
| 1–5 rating | explicit quality judgment | strong |
| crop / grade edit | preferred treatment and framing | medium |
| export / publish / used in | asset survived a real workflow | medium |
| search click / detail view | weak interest only | weak |

Feedback may include a context such as “homepage hero,” “warm family reel,” or “event selects.” A
preference in one context must not silently become a universal rule.

## Personal style model

The first useful local model is deliberately small and auditable:

- normalized CLIP embedding features for semantic and visual affinity;
- normalized aesthetic/design features;
- an owner-specific linear ranking head trained from positive/negative and pairwise feedback;
- regularization toward the general model while the user has little evidence;
- versioned model snapshots with sample count, feature weights, and training metadata.

For a query and candidate asset, ranking is:

`semantic relevance + general aesthetic + personal style affinity + context fit - repetition/safety penalties`

The UI should expose that breakdown in plain language. The model must say “preferred warm palette
and close framing” or “sharp but repeatedly rejected in this context,” not present a mysterious
single score. Users can reset, disable, or retrain their local style profile.

Video uses the same visual model at shot/keyframe level. Shot quality aggregates representative
and boundary-safe frames, then adds motion stability, moment, transcript, pacing, and sequence fit.
Photo and video feedback therefore improve the same style profile without pretending they are
identical media.

## Cold start from Reel Studio

An importer maps existing Reel Studio data as follows:

- `quality`, `standout`, `usable` → explicit editorial annotation;
- descriptions, subjects, action, tags → searchable metadata;
- `faces_visible`, `nametags_visible`, `blur_required` → safety annotation;
- `crop_x`, recipe `crop_kf` → framing preference;
- recipe `grade` → color-treatment preference;
- `used_in` and recipe membership → strong positive workflow events;
- omitted/rejected alternatives from the same reviewed batch → negative or pairwise evidence only
  when that inference is explicitly confirmed.

Nothing in the real Reel Studio database or media library enters version control.

## Data and privacy rules

- Media, thumbnails, embeddings, feedback, and style models stay in Application Support.
- Feedback is owner-scoped and never pooled across users without explicit opt-in.
- Face detection may support privacy review or composition, but named identity recognition is a
  separate, opt-in future capability and is not required for quality ranking.
- Machine scores never clear a privacy flag. Publishing still requires review.
- Every learned profile is versioned, locally deletable, and reproducible from retained feedback.

## Execution roadmap

1. **Foundation (done):** unified photo/video records, vectors, annotations, assessments, feedback,
   style profiles, JPEG/PNG ingest, mixed search, photo detail, and explicit feedback.
2. **Source fidelity:** add HEIC/HEIF, TIFF, DNG and supported camera RAW stills with EXIF,
   orientation, timestamps, lens/camera data, embedded previews, ICC/color metadata, and cached
   working proxies. Expand production-video probing and proxy generation. Publish a tested format
   matrix; camera-RAW video such as BRAW/R3D/ProRes RAW requires an explicit decoder/licensing
   decision rather than silent fallback.
3. **Explainable judgment:** compute technical, composition/design, moment/story, and sequence
   features for stills and representative video frames. Preserve each component and confidence.
4. **Review and learning:** pairwise compare, picks/rejects, ratings, crops, grades, tags, notes,
   privacy flags, collections, version stacks, and saved searches. Train context-aware personal
   ranking and require held-out improvement before calling it learned.
5. **Editorial planning:** create ranked photo selects and video clip/reel plans in the user's
   style, with editable reasons, boundaries, pacing, crops, grades, and sequence order.
6. **Render and export:** keep originals immutable; store non-destructive recipes and render photo
   derivatives plus video clips/reels through resumable jobs. Exports include deterministic presets,
   color/orientation handling, metadata policy, cancellation, manifests, and output verification.
7. **Migration and release:** import Reel Studio catalogue/recipe evidence, then package and test
   the complete mixed-media workflow on a clean Mac.

## Acceptance principles

- Search and ingest work for both photos and video without uploading media.
- General quality and personal taste are displayed separately.
- A style profile cannot be called “learned” without held-out improvement over the general ranker.
- Pairwise and pick/reject feedback must change ranking predictably and reversibly.
- Every result can explain the signals that helped or hurt it.
- Existing video fixtures, goldens, and clean-machine guarantees continue to pass.
- Originals are never overwritten. Every render records its source assets, recipe, tool/model
  versions, output checksum, and failure/cancellation state.
- Unsupported RAW or acquisition formats fail with a precise capability reason; Crush never
  labels a low-fidelity thumbnail as a full-quality decode.
