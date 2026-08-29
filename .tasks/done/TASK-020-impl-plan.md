# TASK-020 implementation contract (reconstructed 2026-08-29)

The handoff referenced this path, but the file was absent from main and PR #33. This is a
new reconstruction from the merged 020a code, TASK-020 acceptance, and John's 020b request;
it does not purport to recover an uncommitted OpenCode plan. The original engineering blueprint
and additive editorial/DAM blueprint remain authoritative.

## 020a — shipped core

- Schema v9: plans, items, append-only version snapshots. Owner scope throughout.
- `selects_candidates`: general quality ordering plus brief-driven search ordering.
- Plan create/list/get/update/delete, add/update/remove/reorder items, save/list/restore
  revisions, duplicate. Item boundaries remain inside their source shots.
- General origin carries no profile version; personal origin must carry one. Selection
  score and JSON evidence are frozen when added and survive edits/version restore.
- Plans are documents, not media. Editing them alone is not training evidence.

## 020b — UI contract

1. Plans navigation, plan list/create/reopen, name/description/brief editor and fixed context.
2. Side-by-side General and Personalized candidate columns. Explain different score scales;
   label brief-only fallback honestly. No “learned” claim before John's hard-stop sign-off.
3. Mixed photo/video cards, quality and signal breakdown, transcript and source ranges.
4. Add with frozen candidate, lane, ordinal, brief/context, score and effective profile
   ID/version/algorithm. Duplicate selection is blocked by the existing media-kind + ID key.
5. Editable shot boundaries, pacing, crop intent, grade JSON and rationale; reorder/remove.
   Treatment intent is not a rendered preview. Source validation remains backend-owned.
6. Save/version/restore/duplicate/delete through existing store APIs. Confirm destructive
   document replacement; retain drafts on failure; never silently lose another item's draft.
7. Explicit “Pick for this context” is separate from all document edits and uses the trainer's
   real context field. Removed items are not implicitly rejected.
8. Regression coverage in the stateful, real-DOM UI harness plus native bridge/search tests.
   Mac compile/lint and full existing test gates remain required.

## Boundaries carried forward

Automatic sequence repetition penalties are not supplied by 020a; don't invent that claim in
the UI. Rendered crops/grades/transitions/audio/output, recipes and manifests belong to 021.
Importer and release acceptance remain 022/023. See `docs/review-2026-08-29.md` for open
style-evaluation issues and human approval requirements.
