# TASK-010: Transcribe + alignment
Agent: Codex (CPU build OK; Metal verified by Cursor in review). Branch: task/10-asr. Depends: 004, 002.

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
- [ ] Golden: WER < 15% vs `*.transcript.json` on speech fixtures (implement a simple word-level WER in the test)
- [ ] No-audio fixture: stage completes instantly with 0 segments
- [ ] `debug align` output correct on the speech fixture by eye
- [ ] Cursor confirms Metal active on the Mac and records ms per 10 s of audio

## Do not
- Use large models. Do word-level diarization.
