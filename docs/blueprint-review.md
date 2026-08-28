# Crush — Blueprint Review and Implementation Protocol

Reviewer stance: adversarial. The goal is to find what breaks before an agent spends a week on it.

## Part 1 — What's wrong or thin in the plan

### Blockers (fix before Task 1)

**1. The riskiest technology is unproven and buried in Task 8.**
The whole plan rests on three things working together on one Mac: `ort` with the CoreML provider, `whisper-rs` with Metal, and Tauri's sidecar for ffmpeg. Each works alone. Together, in one Cargo workspace, with static linking on Apple Silicon, they are a build-system fight (`ort` needs a matching onnxruntime dylib or static lib with CoreML compiled in; `whisper-rs` needs cmake and the Metal feature; Tauri has its own bundling rules). If this fight is lost in week three, Tasks 1–7 were built on sand.
**Fix:** add **Task 0 — Feasibility spike.** One throwaway binary, no product code: load a CLIP ONNX with CoreML active, transcribe 10 s with Metal, spawn a bundled ffmpeg, print ms for each. Two days max. If it fails, we know the fallback is CPU-only or a different runtime before anything else exists.

**2. Orchestrator mismatch.**
The orchestrator skill assumes `nexuscore-services-os` on a Windows path and Codex running on ai-srv (Linux). This project is a new Mac repo. Codex on ai-srv cannot build or test anything CoreML, Metal, or Tauri. Dispatching those tasks there produces code that "compiles on Linux" and fails on the target.
**Fix:** new repo, and a hard routing rule (Part 2). Update the orchestrator skill with a per-project repo root, or the session start protocol will read the wrong `TASKS.md`.

**3. Task 12 (Tauri app) is three tasks in a trench coat.**
Shell + commands, first-run download screen, and four UI screens is not one review session.
**Fix:** split into 12a (Tauri shell + commands + a blank page that calls `doctor`), 12b (first-run and Library screens), 12c (Search and Shot detail).

### Gaps (specify before the relevant task)

- **Representative frame rule is unspecified.** Which frame of a shot gets embedded? Decide: the frame at 40% of the shot's duration (avoids fade-ins and the cut frame). Write it in §5.
- **CLIP tokenizer needs its vocab shipped.** The BPE merges/vocab file is a model asset like the ONNX files. Add it to Task 6's download manifest with a sha256, and to the answer key.
- **Hybrid ranking is hand-waved.** "Transcript hits boost visual results" needs a formula or a contributor will invent one. Start dumb and documented: `score = cosine + 0.15 × (1 if any query word appears in the shot's transcript)`. Tune later with the smoke table.
- **Cancel and pause.** A user will close the laptop mid-index. Ingest must be resumable per video and cancellable from the UI. Currently only "re-run resumes" is implied. Add to Task 11 acceptance.
- **CPU priority.** Indexing on a laptop must not make the machine unusable. Run the pipeline with lowered thread priority and cap ort/whisper threads to physical cores minus two.
- **Multi-file concurrency.** Decide: one video at a time in Phase 1. Simpler, and the GPU is the bottleneck anyway.
- **Golden tolerance for CoreML.** CoreML runs fp16 internally; cosine > 0.999 may fail legitimately on CoreML while passing on CPU. Set CPU tolerance 0.999 and CoreML tolerance 0.99, and write down why.
- **Thumbnail storage.** Thousands of JPEGs in one folder is fine on APFS, but name them by shot id and store the path relative to the app data dir so the library survives a rename.
- **Version pins.** `ort` and `whisper-rs` break APIs between minor versions. Task 1 pins exact versions in `Cargo.toml` and a `docs/versions.md` explains why each was chosen.
- **The Rust document's crate list should be ignored entirely.** It names dead or nonexistent crates. The blueprint already replaces it; say so in the repo so a contributor doesn't "helpfully" add `onnxruntime-rs`.

### Things that are fine and should not be argued with again

- No vector database. Brute-force cosine over ≤100k × 512 floats is trivial.
- ffmpeg as subprocess. Do not reopen bindings until decode is measured as the bottleneck.
- Answer-key tests as the definition of correct. This is the single best decision in the plan.
- SQLite for everything. One file, backup by copying it.

## Part 2 — How Claude implements it

Claude here means Cowork acting as orchestrator and reviewer. Claude does not write product code. Cursor and Codex do.

### Routing rule (non-negotiable)

| Task | Agent | Why |
|---|---|---|
| 0 spike | **Cursor on the Mac** | Only the Mac can test CoreML/Metal/Tauri |
| 1, 2, 3, 5, 9, 10 | Codex | CPU-only Rust and Python; testable on Linux |
| 4, 6, 7, 8, 11, 12a–c, 13 | **Cursor on the Mac** | Bundled arm64 ffmpeg, CoreML, Tauri, signing |
| Review, task specs, merges | Cowork | Per orchestrator skill |

If Codex is ever tempted for a Mac task, the answer is no. "It compiles on Linux" is not evidence.

### Repo and board

- New repo `crushctl`, cloned on the Mac. Codex clones it on ai-srv separately.
- `TASKS.md` and `.tasks/` per the orchestrator convention. Task files are the §14 tasks, one file each, with the Part 1 gaps folded into the relevant task.
- `docs/HANDOFF.md` is the first thing every agent reads: stack, routing rule, golden-test rule, "never edit golden files", pinned versions, and the crate blacklist.
- One branch per task: `task/07-preprocess`. Agents open a PR. Nothing merges without Cowork review.

### Cowork's review checklist per PR

1. Did CI pass — unit and golden on CPU?
2. For Mac tasks: did the agent paste `doctor` output showing CoreML/Metal active, with ms per stage?
3. Any golden file modified? Reject unless the commit explains a deliberate regeneration.
4. Any new dependency? Check it's alive, pinned, and license-compatible.
5. Did the agent add anything not in the task's acceptance criteria? Reject the extra, keep the rest.
6. Are the ffmpeg command lines logged verbatim?
7. Does the PR description say what was tested by hand?

### Hard stops (human, not Cowork)

- After Task 0: go / no-go on the runtime stack.
- After Task 7: John runs the preprocessing golden test on his Mac himself.
- After Task 12c: John uses the app for ten minutes on real footage before Task 13.
- Before Task 13's notarization: signing credentials go into GitHub secrets by John, never via an agent.

### Keeping Claude usage minimal

- Cowork writes one task spec per dispatch, in the Task format from the architect skill, with the acceptance criteria copied verbatim from the blueprint plus the Part 1 additions. No prose beyond that.
- Cowork does not debug. A failed PR becomes a fix task with the exact failing test and log lines pasted in.
- Cowork reads `doctor` output and test output, not source files, unless the checklist item 5 flags scope creep.
- The blueprint and this review go in `docs/`. When a decision changes, edit the blueprint; do not re-explain in chat.

### Order of dispatch

```
0 (Cursor, spike) → human go/no-go
1 → 2 → 3 (Codex, can run 2 and 3 in parallel after 1)
4 (Cursor) ‖ 5 (Codex)
6 → 7 (Cursor) → human review of golden → 8 (Cursor)
9 (Codex, needs 8 merged) ‖ 10 (Codex, needs 4 merged)
11 (Cursor) → 12a → 12b → 12c (Cursor) → human ten-minute test
13 (Cursor) → clean-machine test
```

## Part 3 — Blueprint edits to make now

1. Add Task 0 spike before Task 1.
2. Split Task 12 into 12a/12b/12c.
3. Add to §5: representative frame = frame at 40% of shot duration.
4. Add to Task 6: tokenizer vocab as a model asset.
5. Add to Task 9: hybrid formula.
6. Add to Task 11: cancel/resume, one-video-at-a-time, thread caps.
7. Add to §11.1: CoreML tolerance 0.99, CPU 0.999.
8. Add to §10: pinned versions rule and crate blacklist (`onnxruntime-rs`, `ffmpeg-next`, `tch`, anything from the original Rust doc not in the blueprint).
9. Add routing rule to §13.
