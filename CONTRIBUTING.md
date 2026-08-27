# Contributing

1. Read `docs/HANDOFF.md` first. It is short and it is the law.
2. Run `crushctl doctor` before reporting anything.
3. Golden tests define correctness. Never edit `fixtures/golden/` to make a test pass; fix the Rust.
4. Do not add crates from the blacklist in `Cargo.toml`. Pin exact versions for ort and whisper-rs.
5. Mac-specific work (CoreML, Metal, ffmpeg sidecar, Tauri) must be tested on a Mac. "It compiles on Linux" is not a test.
6. One task per PR. Say what you tested by hand in the PR description.
