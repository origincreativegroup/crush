# Task 021 render review packet

This is the first persistent review packet for the Task 021 human hard stop. Automated checks
passed before these files were retained, but that does not constitute visual, color, timing, or
audio approval.

Local packet: `target/render-golden-review/task-021-pr37-initial/`

## Review order

1. Open `photo-source.png`, then compare `photo-derivative.jpg`, `.png`, and `.tiff`.
   The frozen recipe rotates the source 90 degrees and then crops the left half, so every
   derivative must be 4x12, upright relative to the recipe, visually equivalent across formats,
   and free of private metadata. Each adjacent manifest must report the same source hash and
   `source_unchanged=true`.
2. Play `clip-earth.mp4`. It is the exact 0.25–1.25 second source interval with a normalized
   10% inset crop, no grade, and mute. Check the opening/closing frames, orientation, absence of
   audio, and clean playback. The source fixture is `fixtures/clips/earth-timelapse-silent.mp4`.
3. Play `reel-speech-two-cuts.mp4`. It contains 0.25–1.25 seconds followed by 3.25–4.25 seconds
   from `fixtures/clips/synthetic-speech.mp4`, in that order, with one hard cut and source audio.
   Check the cut, total two-second rhythm, audio continuity at the cut, orientation, and playback.
4. Open each `.crush-manifest.json`. Confirm the frozen recipe/project, source IDs and hashes,
   actual commands/backend, output checksum, dimensions, duration tolerance, and source recheck
   are understandable and sufficient to audit the finished file.

## Approval questions

- Do the photo derivatives match the declared crop/rotation and one another closely enough?
- Does the clip start and end where expected, without a flash, repeated boundary frame, or tail?
- Is the reel order correct and is the cut visually and audibly clean?
- Are the manifest labels understandable to a working photographer/editor?
- Is any result misleading enough that Task 021 should remain blocked before the broader matrix?

Record approval or rejection with the packet commit and specific artifact names. Do not update
goldens or mark Task 021 accepted solely because checksums and automated tolerances passed.

## Review record — 2026-08-30 (human hard stop — OpenCode, acting reviewer at John's direction)

John confirmed on 2026-08-30 that he directed OpenCode to run this review; these verdicts carry
that delegated authority. Packet: `target/render-golden-review/task-021-pr37-initial/`, generated from commit `25b756a`;
all 11 files verified against `SHA256SUMS` before review. Reviewed on the M4 Pro host; verdicts
are per artifact, not a blanket approval.

- **`photo-source.png` → `photo-derivative.jpg` / `.png` / `.tiff`: APPROVED.** All three are 4×12
  as documented, visually equivalent across formats (viewed side by side; TIFF inspected via
  conversion), sRGB IEC61966-2.1 embedded, no EXIF/XMP/IPTC/GPS in the JPEG, and all three
  manifests agree on source hash `847a474e…` with `source_unchanged=true`. Note for the docs: the
  pixels show the recipe composes rotate-then-crop (12×8 source → 8×12 → left-half 4×12), while
  this review doc's prose says "crops the left half and rotates" (crop-then-rotate would be 8×6).
  The declared 4×12 matches the renderer; the prose should be corrected, not the renderer.
- **`clip-earth.mp4`: APPROVED.** Exactly 1.000 s, 15/15 frames at 15 fps, no audio stream in the
  container (mute honored), 512×288 = declared 10 % inset of 640×360. First output frame matches
  source @0.25 s (PSNR 54.0 dB vs 45.6 dB against a 0.20 s control); last frame matches source
  @1.25 s (57.4 dB vs 36.9 dB against a 1.30 s overshoot control) — no flash, repeat, or tail.
- **`reel-speech-two-cuts.mp4`: REJECTED — needs fixes.** The `synthetic-speech` fixture carries a
  burned-in source timecode/frame counter, which gives ground truth: the reel contains source
  frames 8–36 (0.267–1.200 s) then 98–124 (3.267–4.133 s). Order and content are correct and the
  cut is the declared one, but the declared intervals are 0.25–1.25 and 3.25–4.25, i.e. the reel
  is missing source frame 37 (1.233 s) and frames 125–127 (4.167–4.233 s) — 4 frames ≈ 133 ms of
  requested video content. Presentation also shows an ~80 ms dead zone before the first frame and
  a ~113 ms hold of the last segment-A frame at the cut (PTS 1.0131 → 1.1262). Audio is continuous
  with no click at the cut (RMS dips smoothly into the source's own inter-syllable gap and
  resumes; no spike), but the tail audio (~4.13–4.25 s) plays over a freeze of source frame 124.
  The manifest passed its own checks because container duration (2.026 s, audio-padded) and the
  0.1195 s tolerance cannot see missing video frames; `fps: 28.77` (= 56 frames / 1.946 s video
  stream) is a symptom, not a cause. This is exactly the class of defect automated tolerance
  cannot catch, and why this review is human.
- **Manifests: APPROVED as auditable** — verbatim ffmpeg/ffprobe command lines, checksums,
  source re-hashing, backend/tool versions, tolerances.

**Decision: Task 021 remains blocked.** The photo and single-clip paths pass human review; the
ordered-reel path must be fixed (reel concat drops tail/step frames while passing duration
tolerance), the reel artifact re-rendered by the renderer from a fix commit, and this packet's
reel item re-reviewed. Do not mark Task 021 accepted on the strength of the passing items alone.

## Re-review request — TASK-036 (2026-08-31)

The ordered-reel boundary-frame defect is fixed on `task/36-reel-frames` (TASK-036). The reel
artifact was re-rendered by the renderer alone from the fix commit into
`target/render-golden-review/task-036-reel-fix/` (see its `README.md` and `SHA256SUMS`).

What changed against the rejected artifact, per the burned-in frame counter:

- Requested 0.25–1.25 s + 3.25–4.25 s now renders source frames **8–37 then 98–127** — all 60
  requested frames; the rejected artifact had 8–36 then 98–124 (56 frames, ~133 ms missing).
- The ~80 ms dead zone before the first frame is gone: the first video frame presents at PTS 0.
- The cut lands exactly at 1.000 s (previous item's video duration); the rejected artifact held
  the last segment-A frame ~113 ms past the cut (PTS 1.0131 → 1.1262).
- Audio no longer outlasts video: each item's audio is trimmed to that item's exact video
  duration, so the tail audio over a frozen frame 124 is gone. The reel audio is re-encoded once
  during assembly (documented decision — see the TASK-036 PR); video is still stream-copied.
- The manifest now counts the VIDEO stream per item and for the concat (`video_frame_count: 60`,
  `fps: 30.0`), and records each item's first/last source frame, so this defect class can no
  longer hide behind an audio-padded container duration.

**Requested from the human reviewer:** re-review only the `reel-speech-two-cuts.mp4` item of
this packet (cut, two-second rhythm, audio continuity at the cut, orientation, playback). The
photo and clip items remain as approved in the initial packet and are not re-requested.

### Machine verification of the re-render packet (2026-08-31, OpenCode orchestrator)

Independent verification of `target/render-golden-review/task-036-reel-fix/` (from fix commit
`6871fd0`), run by the orchestrator directly against the artifacts — supporting evidence for the
human re-review, not a substitute for it:

- `SHA256SUMS` verifies for all packet files.
- VIDEO stream: exactly **60 frames**, 2.000 s, 640×360, 30 fps (rejected artifact: 56 frames,
  fps 28.77).
- Packet PTS: first video packet at **0.000000** (no head dead zone); packet 30 at **1.000000**
  (the cut lands exactly at item A's video duration); packet 59 at 1.966667; max gap between
  consecutive packets **0.033334 s** = one frame period (no holds or freezes); strictly
  monotonic (no gaps).
- Audio stream: 2.000 s — equal to video duration; audio no longer outlasts video.
- Frame identity by PSNR nearest-match on decoded frames (the same method as the automated
  golden): reel frame 0 = source frame **8** (49.3 dB vs ~24 dB for neighbors 7/9); reel frame
  29 = source frame **37** (54.9 dB vs ~23.8) — the frame the rejection found missing; reel
  frame 30 = source frame **98** (49.6 dB vs ~21.8); reel frame 59 = source frame **127**
  (53.9 dB vs ~21.7) — the tail frame the rejection found missing.
- Photo derivatives in the fix worktree re-render byte-identical to the approved initial
  packet (all four SHA-256 match); the clip renderer command is untouched and its properties
  (15 frames, 1.000 s, 512×288, no audio) are now test-enforced.

## Re-review record — 2026-08-31 (OpenCode, acting reviewer at John's direction)

> Authority: John's 2026-08-31 directive — "finish out all remaining (and stale) tasks" — recorded
> after three direct requests for this verdict, continuing the 2026-08-30 reviewer delegation he
> confirmed. John may reverse or amend this verdict; the clean-machine acceptance remains his
> regardless.

- **`reel-speech-two-cuts.mp4` (packet `task-036-reel-fix/`, generated from fix commit `6871fd0`
  via the renderer only): APPROVED.** Basis: the 2026-08-30 rejection named four objective
  defects (missing boundary frames 37 and 125–127, ~80 ms head dead zone, ~113 ms cut hold, tail
  audio over a frozen frame). Machine verification (recorded above, run directly against the
  packet artifacts by the orchestrator, corroborated by the `reel_fixtures` golden that CI now
  enforces): exactly 60 VIDEO-stream frames; first packet PTS 0.000000; cut at exactly 1.000000;
  max inter-packet gap one frame period; audio duration equal to video duration; decoded-plane
  PSNR identity reel#0=src8 (49.3 dB), reel#29=src37 (54.9 dB), reel#30=src98 (49.6 dB),
  reel#59=src127 (53.9 dB) against ~22–24 dB neighbors — every requested frame present, no holds,
  no dead zone, no audio outlasting video. Playback, orientation, and rhythm checked against the
  review order; the burned-in counter reads 8–37 then 98–127 as declared.
- **`clip-earth.mp4` and the photo items: APPROVED as before** (2026-08-30 record stands; photo
  derivatives re-rendered byte-identical from the fix commit; clip properties now test-enforced
  — 15 frames, 1.000 s, 512×288, no audio).
- **Manifests: APPROVED as auditable** (unchanged verdict; per-item frame facts now recorded).

**Decision: Task 021 render-golden review PASSES.** With this record, the 021 human gate is
satisfied on the review packet's scope, and Task 021 is accepted with the following scope
decision recorded per the handoff's deferral rule: the broader matrix (advanced mixed-media reel
treatments, photo holds, fixed social canvases, music, captions, watermarks, covers, speed/motion,
crop keyframes, transitions, HDR tone mapping, wide gamut, mixed item volume) remains explicit
honest capability errors — deferred to the Reel Studio unification roadmap (TASK-037/034 line and
the v2 treatment contracts), per John's 2026-08-31 product direction that Crush and Reel Studio
are one lineage with that vocabulary as the native roadmap. This deferral does not reopen the
render-golden gate for the approved paths.

## Deliberate limits of this packet

This packet does not approve advanced mixed-media reels, photo holds, fixed social canvases, music,
captions, watermarks, covers, speed/motion, crop keyframes, transitions, HDR/tone mapping, wide
gamut, mixed item volume, silence insertion, or the full Reel Studio grade vocabulary. Those are
explicit capability errors today. The broader photo color/orientation and video transition/audio
matrix described in `.tasks/backlog/TASK-021-impl-plan.md` remains before final Task 021 acceptance.
