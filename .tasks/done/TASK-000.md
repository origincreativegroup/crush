# TASK-000: Feasibility spike
Agent: Cursor on the Mac. Branch: task/00-spike. Read docs/HANDOFF.md first.

## Goal
Prove ort+CoreML, whisper-rs+Metal, and a bundled static ffmpeg all build and run in one Rust binary on John's Mac. Throwaway code in `spike/`.

## Instructions
Follow `spike/README.md` exactly. Do not touch `crates/`.

## Acceptance
- [x] `cd spike && cargo run --release` exits 0 and prints ms for all three steps
- [x] CoreML confirmed ACTIVE (provider list or verbose log pasted in PR), not CPU fallback
- [x] Metal confirmed in whisper log
- [x] `docs/versions.md` written: macOS, chip, RAM, exact crate versions, build flags, ms per step, CoreML op warnings
- [x] `sidecars/SOURCES.md` records ffmpeg source URL, LGPL confirmation, sha256

## Do not
- Add ort/whisper-rs to the workspace Cargo.toml yet — that is Task 1's follow-up after go/no-go
- Spend more than two days. Report partial results instead.

## Human review
John reads docs/versions.md and gives go / no-go.

Status: approved GO by John on 2026-08-27; ready to merge.
