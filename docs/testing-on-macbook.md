# Testing on John's MacBook

This is the primary test machine for the whole project. Fill in the header once.

| | |
|---|---|
| Chip | Apple M4 Pro |
| RAM | 24 GB |
| macOS | 26.5.2 |
| Xcode CLT | `/Library/Developer/CommandLineTools` |
| Rust | `rustc 1.98.0 (88d9e12ae 2026-08-18)` |

## One-time setup (~20 min)
1. `xcode-select --install`
2. Rust: `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh` then restart the terminal.
3. `brew install cmake` (whisper-rs builds whisper.cpp from source).
4. Python for the answer key only: `cd reference && python3 -m venv .venv && . .venv/bin/activate && pip install -r requirements.txt`.
5. Clone the repo, `cargo build --workspace`, `cargo run -p crush-cli -- doctor`. You should see the doctor stub print your data dir under `~/Library/Application Support/Crush`.

## What gets tested here, and when

| After task | What John runs | Pass means |
|---|---|---|
| 0 | `cd spike && cargo run --release` | CoreML active, Metal active, ffmpeg spawns; ms recorded in docs/versions.md |
| 1 | `cargo run -p crush-cli -- doctor` | prints, log file appears in `.../Crush/logs/` |
| 3 | `cd reference && make golden` | fixtures/golden/ populated, deterministic on second run |
| 4 | `cargo test -p crush-stage-split` and eyeball a thumbnail vs its timecode | thumbnail matches the frame you expect |
| 5 | `crushctl debug scenes fixtures/hardcuts.mp4` | CSV opens in Numbers; cuts land where your eyes say |
| 7 | `cargo test -p crush-stage-embed preprocess_golden` | **passes at 1e-3. This is the hard stop.** |
| 8 | same, plus `doctor` | golden passes on CoreML and CPU; doctor shows `active=coreml` |
| 9 | `crushctl search "..."` on fixtures | your 3 queries return the shot you meant in top 3 |
| 10 | `crushctl debug align fixtures/speech.mp4` | transcript sits on the right shots |
| 11 | ingest a folder twice; kill it mid-run; re-run | second run skips; killed run resumes cleanly |
| 12a–c | open the .app | ten minutes on real footage; write down every annoyance |
| 13 | install the .dmg on a **fresh macOS user account** | doctor green with zero dev tools present |

## The 5-hour smoke (after Task 11, again after 12c)
Write 10 queries **before** running, in `docs/smoke.md`. Index ~5 h of real footage. Score each query: hit in top 5 = 1. Target 8/10. Record wall-clock time and whether the laptop stayed usable.

## When something is wrong
1. `crushctl doctor`
2. `crushctl jobs --failed`
3. Find the symptom in blueprint §11.2 and run the one check it names.
4. Open a fix task with the failing test name and the log lines. Do not debug in chat.
