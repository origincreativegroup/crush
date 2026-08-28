# TASK-016: UI harness in CI + docs refresh
Agent: OpenCode. Branch: task/16-ui-ci. Depends: 012c merged.

## Instructions
1. Move the headless-Chrome harness runner (see TASK-012c record; playwright-core driving the system Chrome)
   into `crates/app/tests/harness.mjs` with a tiny `package.json`, and run it in the macOS CI job. Pin versions.
2. Cover the 12b scenarios too (first-run progress/retry, Library rows, failed-row expand + Copy details, cancel).
3. Refresh `README.md` status (it still says "storage layer complete"), link `docs/install.md`, and confirm
   `THIRD_PARTY.md` lists everything the bundled app ships (ffmpeg LGPL, ONNX Runtime, whisper.cpp, models).

## Acceptance
- [ ] macOS CI runs the harness and fails on a broken UI check
- [ ] README status + install link current; THIRD_PARTY complete
