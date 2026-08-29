# TASK-020b — Plans UI

Implements the reconstructed contract in `TASK-020-impl-plan.md`.

- [x] General/Personalized columns with honest no-profile fallback and experimental status.
- [x] Effective profile provenance returned by the same ranking request and frozen on select.
- [x] Plan create/reopen/edit/duplicate/delete, mixed-media items and explicit scope.
- [x] Boundary, pacing, crop intent, grade JSON and rationale editing; reorder/remove.
- [x] Append-only saved versions and confirmed restore; duplicate source-kind/ID prevention.
- [x] Save failures preserve drafts; saving one item preserves other item drafts.
- [x] Explicit plan-context picks; ordinary document edits never imply feedback or rejection.
- [x] Stateful real-DOM tests: plans-editor, plans-general, plans-errors; existing suite retained.
- [x] Mac search/store/app tests and strict Clippy pass; browser layout review with synthetic data.

No rendering or human style approval is claimed. The parent Task 020 still has follow-up
scope for automatic sequence/repetition judgment (documented in the fresh-eyes review).
