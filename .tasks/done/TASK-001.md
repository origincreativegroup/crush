# TASK-001: Workspace, config, tracing, job log, repo hygiene
Agent: Codex (Linux OK). Branch: task/01-skeleton. Read docs/HANDOFF.md first.

## State
Completed on 2026-08-27. The repository is initialized on `main`, connected to GitHub, and green
on both local macOS verification and GitHub Actions/Linux CI.

## Verification
- [x] `cargo build --workspace` passes on macOS
- [x] `cargo test --workspace` passes, including `config_smoke`
- [x] `cargo clippy --workspace --all-targets -- -D warnings` passes
- [x] `cargo fmt --all -- --check` passes
- [x] `crushctl doctor` exits 0 and writes JSON containing `job_id="doctor"` and `stage="doctor"`
- [x] `LICENSE` is the complete Apache-2.0 text
- [x] Initialize the local Git repository on `main`
- [x] Create/connect `https://github.com/origincreativegroup/crush`
- [x] Linux CI run `33123337534` completed successfully

## Instructions
1. `cargo build --workspace` and fix anything that does not compile. Keep the structure; do not redesign.
2. `cargo run -p crush-cli -- doctor` prints the stub and creates `logs/crush.log` under the data dir.
3. Add `cargo clippy --workspace -- -D warnings` and `cargo fmt --check` to CI; make them pass.
4. Add a `tests/` smoke in `crates/core` that loads `crush.example.toml` and asserts defaults.
5. Replace the `LICENSE` placeholder with the full Apache-2.0 text if it is not already the full text.
6. Set `repository` in Cargo.toml to the real GitHub URL.

## Acceptance
- [x] `cargo build --workspace` clean on Linux and macOS
- [x] CI green
- [x] `doctor` runs; JSON log line appears in logs/crush.log
- [x] Config test passes
- [x] No new dependencies beyond workspace list

## Do not
- Add stage logic, ort, whisper-rs, rusqlite, or tauri
- Change the crate layout

## Human review
Crate layout matches blueprint §10; config keys readable; CI green.
