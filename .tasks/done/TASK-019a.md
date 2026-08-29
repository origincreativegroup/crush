# TASK-019: Mixed-media review and DAM organization

Depends: Task 016; supplies training evidence to Task 018.

## Acceptance

- [ ] Review photos and video shots with picks, rejects, stars, pairwise compare, tags, notes,
      privacy/safety flags, crops, grades, and undoable actions.
- [ ] Add collections, saved searches, duplicate groups, version stacks, and missing-file relink.
- [ ] Collections can be designated as previous-work reference sets with an explicit context and
      whole-set versus selected-example meaning.
- [ ] Preserve append-only feedback provenance while current annotations remain editable.
- [ ] Make original, proxy, recipe, and rendered derivative relationships visible.
- [ ] Keyboard-first batch review works on representative mixed-media folders.

## Record (PR 019a of 2, merged as PR #30)

Implemented by the agent team 2026-08-29 from .tasks/backlog/TASK-019-impl-plan.md. 0008_collections.sql
(schema v8): owner-scoped collections with items/context keys/marked flags, version stacks with
one-original partial unique index, saved searches, and a single set_safety_flags write path. Collection
designation as reference set fills the reserved source_collection_id and materializes items at confirm
(whole_set or selected/marked). browse_assets/library_counts give one UNION read path for mixed browsing;
search_assets_in_context untouched for saved-search replay. bulk_review runs one Immediate transaction
and appends pick/reject/rating signals via the shared append_feedback invariants; safety flags are
state-only writes and export_clip refuses unsafe exports pre-flight. 8 new store tests (28 total);
cargo test -p crush-store green on both CI runners. Integration note: app command surface rebuilt via
a clean re-apply after two flawed union merges (final file: rustfmt-parseable, braces/parens balanced,
46 registered commands).
