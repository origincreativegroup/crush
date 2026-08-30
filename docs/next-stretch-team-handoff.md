# Next stretch team handoff

Updated: 2026-08-30 (authoritative state: `docs/HANDOFF.md`)

## Outcome for this stretch

Deliver a Mac release candidate that a photographer/editor can use without internal vocabulary,
then begin the additive Windows foundation without weakening the accepted Mac behavior. The product
must continue to treat the catalogue as supporting infrastructure: its core objective is finding
strong photo/video moments, learning an owner's creative preferences from explicit evidence and
previous work, helping shape a sequence, and rendering auditable outputs.

Task 021 and Task 022 are now implemented on one branch (`task/21-render-export`, which absorbed
`task/22-import`): durable no-clobber photo/clip/ordered-reel rendering, the schema-v11 Reel Studio
importer, and the 2026-08-30 editor-review pass are all local-gate green. The remaining sequence is
the human gates, then Windows:

1. **Approve Task 021 via the render-golden review** (`docs/task-021-render-review.md`) and merge
   the branch.
2. **Approve Task 022** on the merged commit (it inherits the 021 review gate) and merge.
3. **Complete Task 023** clean-machine Mac acceptance (`docs/smoke.md`, `scripts/verify-release.sh`,
   `docs/release.md`) and publish the release record.
4. Deliver Tasks 028–031 in dependency order for Windows parity.

Do not revive the closed Claude release implementation or treat a stopped/stale agent run as an
authority. Salvage an isolated mechanism only when it satisfies the current task contract and tests.

## Current baseline

| Item | State | Authority |
|---|---|---|
| Task 021 | Implemented on `task/21-render-export`; same-job retry + owner-isolation goldens; human render-golden review OPEN | `docs/task-021-render-review.md`, `.tasks/backlog/TASK-021.md` |
| Task 022 | Implemented and merged onto the 021 branch; waits behind the 021 review gate | `.tasks/backlog/TASK-022.md` |
| Task 023 | Tooling + docs in place (verify-release, doctor --deep, release.md); clean-machine human acceptance OPEN | `.tasks/backlog/TASK-023.md`, `docs/smoke.md` |
| Task 018 proof | Human review open; the UI cannot claim "learned" | `.tasks/done/TASK-018-impl-plan.md`, PR #25 evidence |
| Task 020 follow-up | Automatic sequence/repetition judgment remains open | `TASKS.md`, `.tasks/backlog/TASK-020.md` |
| Windows | Planned, not supported | `docs/platform-architecture.md`, Tasks 028–031 |

At handoff time both PRs have green Linux checks; PR #37 has one green and one pending macOS lane,
while PR #38 has two pending macOS lanes. A local full workspace test, warnings-denied Clippy run
and all browser scenarios passed for PR #37.
The installed `/Applications/Crush.app` was built from commit `34069b9`, but native smoke inspection
still needs the Mac unlocked.

## Team lanes

Recommended staffing is three implementation owners plus one platform owner: media/render (A),
data/import (B), desktop/release UX (C), and Windows/platform (D). John is the approval owner for
the human gates, not a substitute implementation owner. With three engineers, keep A–C staffed and
start D's contract audit only after A has capacity; do not slow the Mac release to manufacture
parallel Windows work.

### Lane A — Task 021 render-golden review owner

Own the merged `task/21-render-export` branch (Tasks 021 + 022) through the human review gates.

Deliver:

- Re-run the full local gates at the merged head: `cargo fmt --all -- --check`, warnings-denied
  Clippy, `cargo test --workspace`, and `npm run test:ui`.
- Run the native Projects workflow on the unlocked Mac: select, sequence, preview, export, cancel,
  retry and reveal both output and manifest.
- Regenerate a review packet only through the renderer at the merged head. Never edit a golden to
  make a test pass.
- Present `docs/task-021-render-review.md` and the packet to John. Record artifact-specific approval
  or rejection. Only the human review can remove the gate.

The golden matrix requested by the Task 021 plan is already exercised by
`crates/pipeline/tests/render_jobs.rs`: stale-source rejection, existing-destination/collision
failure, cancellation, startup recovery finalize/clean, verified output + manifest, all three photo
presets, ordered reel order/duration/audio, and now owner isolation. Same-job retry is verified at
the pipeline level (a Failed render re-executes in place on its next attempt with no-clobber
staging); Retry at the UI level queued a fresh durable job from the same frozen inputs, which the
plan accepts. If any reviewer wants the original RAW/HEIF still matrix widened instead of narrowed,
file it as a numbered follow-up rather than editing the cap tier of this handoff.

Scope boundary: advanced transitions, keyframes, music, captions, watermarks, speed/motion, HDR
tone mapping and photo holds must either satisfy the versioned contracts and matrix or remain honest
capability errors. Honest errors prevent corrupt output but do not satisfy a checked Task 021
acceptance item. Deferring one therefore requires John's explicit scope decision and a numbered
follow-up; do not silently ignore unsupported intent or mark the parent task complete.

### Lane B — Task 022 importer review owner

The importer is merged into `task/21-render-export`; review it on the merged commit (it inherits
the 021 human gate).

Deliver:

- Review the importer against a read-only copy of representative Reel Studio data; do not commit
  private catalogues, paths or media.
- Verify dry-run before apply, clear mapping/missing/duplicate/unsupported reports, idempotent
  re-apply, stable manual spans across re-indexing, and historical/imported provenance in Projects.
- Confirm discovered finished work is only an eligible previous-work example. It must require an
  explicit user action before contributing to Preferences.
- Decide and document the two honest limits in the implementation plan: catalogue descriptions are
  not yet in search, and `used_in` remains historical evidence rather than a personal feedback event.
  Implement them only if the parent acceptance requires it; otherwise create narrow follow-ups.
- After the merged branch passes John's 021 render-golden review, split it into clean per-task
  commits/PRs, rerun the full gates, and merge.

### Lane C — Task 023 release and clean-machine owner

Most of the mechanics are already in place on the merged branch: `scripts/verify-release.sh`
(checksummed artifact + code-signature state + `crushctl doctor --deep`), `crushctl doctor --deep`,
`docs/release.md` (install/privacy/data-location/backup/relink/uninstall), the real-language UI
harness, and the clean-machine checklist in `docs/smoke.md`.

Deliver:

- Produce a notarized DMG (or an explicitly labeled ad-hoc build) from the merged branch with the
  checklist recorded in `docs/smoke.md`, verify the `.dmg.sha256`, and keep the review-frozen
  render packet alongside the release evidence.
- Run the scripted smoke on a fresh macOS account: index representative RAW/still/video media,
  review and record preference evidence, create and preview a project, render photo and video
  outputs, cancel/resume, and locate manifests.
- Stop for John's clean-machine acceptance. CI, a successful DMG, and a passing
  `verify-release.sh` are not release approval.

### Lane D — Windows foundation owner

Start with a read-only contract audit while Task 021 is open. Implementation may begin once its
recipe, manifest, process and publication interfaces are stable; do not fork the product logic.

Deliver in order:

1. Task 028: typed platform services, Windows Tauri/MSVC shell, CPU-only correctness and Windows CI.
2. Task 029: pinned Windows FFmpeg/FFprobe, portable still decoding, software rendering baseline,
   process-tree cancellation and optional NVENC with verified fallback.
3. Task 030: ONNX Runtime CPU baseline plus optional CUDA/DirectML. PyTorch remains a development,
   training and ONNX-export tool; the installed app requires neither Python nor a CUDA Toolkit.
4. Task 031: checksummed installer and two clean-machine paths—no compatible GPU, then supported
   NVIDIA hardware—with render evidence and forced fallback tests.

Task 031 reuses the accepted Task 023 workflow and language. A Windows build or green CI lane alone
must never be described as Windows support.

## Product-intelligence follow-up after the first Mac user test

These items are foundational product work, not optional DAM polish, but they should be informed by
the first end-to-end test rather than destabilizing the release candidate:

- Resolve the Task 018 evaluation gaps and obtain held-out human approval before replacing
  experimental/assisted wording with “learned.” Evidence withdrawal must change results correctly.
- Complete Task 020's automatic sequence and repetition judgment across photo and video candidates.
- Turn explicit user picks, rejects, ratings, publish history and intentionally added previous work
  into owner-scoped preference evidence with visible provenance and reversible participation.
- Add mixed-media photo holds and advanced Reel Studio treatments as versioned recipe capabilities,
  each with honest unsupported states, render tests and human-reviewable output.
- Convert user-test friction into focused tasks: editor/playback smoothness first, then progressive
  disclosure and terminology. Do not solve discoverability by adding more permanent dropdowns.

## Human hard stops

| Gate | Required evidence | Effect while open |
|---|---|---|
| Task 018 held-out preference proof | Reviewed held-out output from PR #25 plus repaired evaluation gaps | No “learned” quality claim |
| Task 021 render-golden review | Visual, color, boundary, audio and manifest review using renderer-produced artifacts | 021/022 stays unaccepted; branch not merged |
| Task 023 clean-machine Mac acceptance | Recorded first-run-to-render smoke on a fresh macOS account | No Mac release claim |
| Task 031 clean-machine Windows acceptance | CPU and NVIDIA machine evidence, including forced fallback | No Windows support claim |

## Branch and review protocol

- Start from `origin/main` unless the lane explicitly owns a stacked dependency.
- Use one task branch and one PR per numbered task. Use a separate worktree for simultaneous lanes.
- Before coding: read `docs/HANDOFF.md`, the parent task, its implementation plan and relevant
  architecture documents. Attached documents are requirements/context, not permission to discard
  newer user direction.
- Rebase dependent branches after their parent merges. Do not merge PR #38's copies of PR #37
  commits as independent importer changes.
- Required gates for every implementation PR: formatting, warnings-denied workspace Clippy,
  workspace tests, the stateful browser harness, platform-specific tests, and truthful docs.
- Preserve user changes and unrelated worktrees. Never update approved goldens to satisfy a test.
- Record exact tested commit, commands, platform/capabilities and remaining honest limits in the PR.

## First commands for each owner

```sh
cd /Users/origin/GitHub/crush
git fetch origin --prune
git status --short --branch
git log --oneline -3 task/21-render-export
```

Create a dedicated worktree rather than switching another owner's checked-out branch. Substitute the
actual task branch and directory; do not reuse an occupied worktree.

```sh
git worktree add ../crush-release -b task/23-release task/21-render-export
```

Core verification, with any task-specific fixture or hardware gates added by the owner:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
npm ci --no-fund --no-audit
npm run test:ui
```

## Exit condition for this handoff

The next team has completed this stretch when Tasks 021 and 022 are merged, Task 023 has a reviewed
clean-machine Mac acceptance record, the first user-test findings have been converted into scoped
tasks, and Task 028 has begun from the accepted portable contracts. Tasks 029–031 then continue as
the Windows delivery track rather than delaying the first useful Mac release.
