# TASK-014: 12c hard-stop fixes
Agent: OpenCode (Mac). Branch: task/14-smoke-fixes. Depends: 012c merged + John's smoke notes in docs/smoke.md.

## Instructions
1. Read the "Annoyances noticed" list in `docs/smoke.md` (John's ten minutes on real footage after 12c).
2. One PR per annoyance cluster; each fix must add or extend a check in `crates/app/tests/ui-harness.html`
   (see TASK-012c record for the headless-Chrome harness runner pattern).
3. Do not change search ranking or goldens; UI/UX only. Anything ranking-related becomes a new task.

## Acceptance
- [ ] Every annoyance line has a PR, a "won't fix" note with a reason, or a new task ID next to it in docs/smoke.md
- [ ] Harness checks pass; `cargo clippy -p crush-app --all-targets -- -D warnings` clean
