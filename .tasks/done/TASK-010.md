# TASK-010: Transcribe + alignment
Agent: Codex (Mac). Branch: task/10-asr. Depends: 004, 002. Status: done.

## Goal
`crush-stage-asr`: whisper-rs on the 16 kHz wav, segments to store, aligned to shots.

## Instructions
1. `whisper-rs` pinned per docs/versions.md, features `["metal"]` under `#[cfg(target_os="macos")]`.
2. Skip the stage (status transcribed, zero segments, job done) when `probe.has_audio == false`.
3. Read wav → f32 mono samples. Run with `n_threads = limits.threads`, language from config or auto, `token_timestamps` on, greedy sampling (fast, good enough).
4. Insert segments (start_s, end_s, text, avg token prob as confidence). Update FTS.
5. Alignment is at query time via `segments_overlapping` — no alignment table. `crushctl debug align <video>` prints `shot idx | start–end | segments overlapping | text`.
6. Model choice by RAM: `doctor` reports total RAM; < 12 GB → base, else small. Config overrides.

## Acceptance
- [x] Golden: WER < 15% vs `*.transcript.json` on speech fixtures (implement a simple word-level WER in the test)
- [x] No-audio fixture: stage completes instantly with 0 segments
- [x] `debug align` output correct on the speech fixture by eye
- [x] Metal active on the Mac and ms per 10 s of audio recorded

## Do not
- Use large models. Do word-level diarization.

## Implementation record

Completed on 2026-08-28. The workspace pins `whisper-rs 0.16.0` and `hound 3.5.1`; macOS builds
enable Metal and Linux builds retain the CPU backend. The stage reads signed 16-bit, 16 kHz mono
WAV, uses greedy decoding with token timestamps, respects the language and thread configuration,
and stores deterministic segments with average token probability as confidence.

- Both speech fixtures produced WER 0.000 with the pinned small model. A 0.78 confidence floor
  removes low-confidence music hallucinations from the montage fixture.
- The real silent fixture finished in 0.149 ms, marked the video transcribed, stored zero segments,
  and did not open either the deliberately missing WAV or model path.
- Runtime logs reported `GPU name: Apple M4 Pro` and `using Metal backend`. Normalized inference
  measured 105.26 and 138.86 ms per 10 seconds of audio across the two speech fixtures.
- Transcript replacement updates SQLite and the external-content FTS table atomically, making the
  next task's resume path idempotent. Alignment remains a query-time interval overlap.
- `doctor` reports total RAM, automatic base/small selection, the compiled backend, and selected
  model availability. `[asr].model` accepts `auto`, `base`, or `small`.
