# TASK-034: Imported evidence — search, confirmation, and preference bridge (022 follow-up)
Agent: OpenCode. Branch: task/34-span-evidence. Depends: 021/022 merged (inherits the 021 gate).

The importer's two documented honest limits plus the explicit-confirmation path
(`docs/reel-studio-import.md` § Limits, `.tasks/backlog/TASK-022-impl-plan.md`).

## Acceptance
- [ ] Catalogue text (description/subjects/action/tags) on manual spans joins the search index and
      Review filtering; results show span provenance and never fabricate a thumbnail Crush lacks.
- [ ] Schema v12: feedback_events admits media_kind 'span' with the same immutability triggers, OR a
      documented decision maps span confirmation onto its overlapping video interval — pick one,
      record why in the migration header.
- [ ] Preferences gains an explicit "confirm imported evidence" flow: quality/standout/used_in on
      spans and imported finished projects become preference evidence or a named previous-work
      reference set ONLY through that user action; per-item and bulk confirm/skip, reversible, with
      imported/historical provenance retained on every derived event.
- [ ] Re-import after confirmation never duplicates or silently revokes confirmed evidence.
- [ ] Store/pipeline tests plus a browser-harness scenario for the confirmation flow.
