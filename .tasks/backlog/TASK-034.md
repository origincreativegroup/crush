# TASK-034: Catalogue unification — span text in search and first-class spans (022 follow-up)
Agent: OpenCode. Branch: task/34-span-evidence. Depends: 021/022 merged (inherits the 021 gate).

Reframed 2026-08-31 per John's direction: Crush and Reel Studio are one product lineage ("Reel
Studio should be Crush and vice versa; I own both projects"). This is no longer an
"imported-evidence bridge" treating Reel Studio data as foreign; it is step 2 of unifying the
catalogue (TASK-037, adjustable boundaries, is step 1). The importer's two documented honest
limits plus the explicit-confirmation path (`docs/reel-studio-import.md` § Limits,
`.tasks/backlog/TASK-022-impl-plan.md`).

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
